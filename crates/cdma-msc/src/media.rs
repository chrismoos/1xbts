//! Media feeder management for the MSC runtime.
//!
//! Owns active voice feeder tasks (ringback, WAV playback) and delayed WAV
//! start scheduling.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{debug, info, warn};

use cdma_voice::tone_player::{RingbackToneKind, RingbackTonePlayer};

use cdma_ios::{VoiceBearerFrame, VoiceBearerManager};

use crate::call_control::{CallDirection, CallId, MscCallController};
use crate::circuit::{CircuitService, MscVoiceLeg};
use crate::config::MediaRingbackType;

/// Active MSC-owned voice media feeder that sends generated frames via voice bearer.
pub(crate) struct ActiveVoiceFeeder {
    pub(crate) kind: ActiveVoiceFeederKind,
    shutdown: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

/// Discriminant for the type of active feeder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveVoiceFeederKind {
    /// NANP/ETSI ringback tone.
    Ringback,
    /// WAV file playback.
    Wav,
}

/// Manages active voice feeder tasks and delayed WAV start scheduling.
pub(crate) struct MediaService {
    /// Active voice feeder tasks, keyed by circuit_id.
    pub(crate) feeders: HashMap<u16, ActiveVoiceFeeder>,
    /// Local WAV calls waiting for simulated answer before media starts.
    pub(crate) delayed_wav_starts: HashMap<CallId, tokio::time::Instant>,
}

impl MediaService {
    pub(crate) fn new() -> Self {
        Self {
            feeders: HashMap::new(),
            delayed_wav_starts: HashMap::new(),
        }
    }

    pub(crate) fn start_ringback_for_call(
        &mut self,
        call_id: CallId,
        controller: &MscCallController,
        circuits: &CircuitService,
        voice_bearer: Option<&Arc<VoiceBearerManager>>,
        media_ringback_enabled: bool,
        media_ringback_type: MediaRingbackType,
    ) {
        if !media_ringback_enabled {
            return;
        }
        if !controller
            .snapshot(call_id)
            .is_some_and(|snapshot| snapshot.direction == CallDirection::MobileOriginated)
        {
            return;
        }
        let Some((&circuit_id, session)) = circuits.circuits.iter().find(|(_, session)| {
            session.call_id == call_id && session.leg_role == MscVoiceLeg::Primary
        }) else {
            return;
        };
        if self.feeders.contains_key(&circuit_id) {
            return;
        }
        let service_option = session.service_option;
        let Some(bearer) = voice_bearer else {
            debug!(
                "MSC: no voice bearer configured, cannot start ringback for call_id={}",
                call_id.0
            );
            return;
        };
        let Some(codec) = cdma_voice::VoiceCodec::from_service_option(service_option) else {
            warn!("MSC: unsupported service option {service_option} for ringback");
            return;
        };
        let kind = media_ringback_kind(media_ringback_type);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let bearer_ref = bearer.clone();

        let handle = tokio::task::spawn(async move {
            let mut player = match RingbackTonePlayer::new_with_codec(kind, codec) {
                Ok(player) => player,
                Err(error) => {
                    warn!("MSC: failed to initialize ringback feeder: {error}");
                    return;
                }
            };
            info!(
                "MSC: started ringback feeder for call_id={} circuit_id={} kind={:?}",
                call_id.0, circuit_id, kind
            );
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(20));
            loop {
                interval.tick().await;
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                let frame = player.next_frame();
                let bearer_frame = VoiceBearerFrame {
                    circuit_id,
                    rate_bps: voice_rate_bps(frame.rate),
                    payload: frame.bits,
                };
                let _ = bearer_ref.try_send_frame(&bearer_frame);
            }
            info!(
                "MSC: stopped ringback feeder for call_id={} circuit_id={}",
                call_id.0, circuit_id
            );
        });

        self.feeders.insert(
            circuit_id,
            ActiveVoiceFeeder {
                kind: ActiveVoiceFeederKind::Ringback,
                shutdown,
                handle,
            },
        );
    }

    pub(crate) fn stop_ringback_for_call(&mut self, call_id: CallId, circuits: &CircuitService) {
        let circuit_ids: Vec<u16> = circuits
            .circuits
            .iter()
            .filter_map(|(&cid, session)| {
                (session.call_id == call_id && session.leg_role == MscVoiceLeg::Primary)
                    .then_some(cid)
            })
            .collect();
        for circuit_id in circuit_ids {
            let is_ringback = self
                .feeders
                .get(&circuit_id)
                .is_some_and(|feeder| feeder.kind == ActiveVoiceFeederKind::Ringback);
            if is_ringback {
                if let Some(feeder) = self.feeders.remove(&circuit_id) {
                    feeder.shutdown.store(true, Ordering::Relaxed);
                    feeder.handle.abort();
                    info!(
                        "MSC: stopped ringback media for call_id={} circuit_id={}",
                        call_id.0, circuit_id
                    );
                }
            }
        }
    }

    pub(crate) fn schedule_delayed_wav_start(
        &mut self,
        call_id: CallId,
        local_answer_delay_ms: u64,
        controller: &MscCallController,
        circuits: &CircuitService,
        voice_bearer: Option<&Arc<VoiceBearerManager>>,
        media_ringback_enabled: bool,
        media_ringback_type: MediaRingbackType,
    ) {
        if self.delayed_wav_starts.contains_key(&call_id) {
            return;
        }
        let delay = std::time::Duration::from_millis(local_answer_delay_ms);
        let deadline = tokio::time::Instant::now() + delay;
        self.delayed_wav_starts.insert(call_id, deadline);
        self.start_ringback_for_call(
            call_id,
            controller,
            circuits,
            voice_bearer,
            media_ringback_enabled,
            media_ringback_type,
        );
        info!(
            "MSC: scheduled local WAV playback for call_id={} after {}ms",
            call_id.0,
            delay.as_millis()
        );
    }

    pub(crate) fn handle_due_delayed_wav_starts(
        &mut self,
        circuits: &CircuitService,
        voice_bearer: Option<&Arc<VoiceBearerManager>>,
    ) {
        let now = tokio::time::Instant::now();
        let due: Vec<CallId> = self
            .delayed_wav_starts
            .iter()
            .filter_map(|(call_id, deadline)| (*deadline <= now).then_some(*call_id))
            .collect();
        for call_id in due {
            self.delayed_wav_starts.remove(&call_id);
            self.start_media_for_call(call_id, circuits, voice_bearer);
        }
    }

    /// Start media sourcing for a connected call.
    pub(crate) fn start_media_for_call(
        &mut self,
        call_id: CallId,
        circuits: &CircuitService,
        voice_bearer: Option<&Arc<VoiceBearerManager>>,
    ) {
        self.delayed_wav_starts.remove(&call_id);
        self.stop_ringback_for_call(call_id, circuits);
        let Some((&circuit_id, session)) =
            circuits.circuits.iter().find(|(_, s)| s.call_id == call_id)
        else {
            return;
        };
        let audio_file = match &session.audio_file {
            Some(f) => f.clone(),
            None => return,
        };
        let service_option = session.service_option;
        if self.feeders.contains_key(&circuit_id) {
            return;
        }

        let Some(bearer) = voice_bearer else {
            warn!(
                "MSC: no voice bearer configured, cannot start media for circuit_id={circuit_id}"
            );
            return;
        };

        let codec = match cdma_voice::VoiceCodec::from_service_option(service_option) {
            Some(c) => c,
            None => {
                warn!("MSC: unsupported service option {service_option} for WAV playback");
                return;
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let bearer_ref = bearer.clone();

        let handle = tokio::task::spawn(async move {
            let mut player =
                match cdma_voice::wav_player::WavVoicePlayer::open_with_codec(&audio_file, codec) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("MSC: failed to open WAV {audio_file}: {e}");
                        return;
                    }
                };
            info!("MSC: started WAV feeder for circuit_id={circuit_id} file={audio_file}");
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(20));
            loop {
                interval.tick().await;
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                let Some(frame) = player.next_frame() else {
                    break;
                };
                let bearer_frame = VoiceBearerFrame {
                    circuit_id,
                    rate_bps: voice_rate_bps(frame.rate),
                    payload: frame.bits,
                };
                let _ = bearer_ref.try_send_frame(&bearer_frame);
            }
            info!("MSC: stopped WAV feeder for circuit_id={circuit_id}");
        });

        self.feeders.insert(
            circuit_id,
            ActiveVoiceFeeder {
                kind: ActiveVoiceFeederKind::Wav,
                shutdown,
                handle,
            },
        );
    }

    /// Clean up all media state associated with a call.
    pub(crate) fn cleanup_call(&mut self, call_id: CallId, circuit_ids: &[u16]) {
        self.delayed_wav_starts.remove(&call_id);
        for cid in circuit_ids {
            if let Some(feeder) = self.feeders.remove(cid) {
                feeder.shutdown.store(true, Ordering::Relaxed);
                feeder.handle.abort();
            }
        }
    }

    /// Returns the next delayed WAV start deadline, if any.
    pub(crate) fn next_delayed_wav_deadline(&self) -> Option<tokio::time::Instant> {
        self.delayed_wav_starts.values().min().copied()
    }
}

fn media_ringback_kind(media_ringback_type: MediaRingbackType) -> RingbackToneKind {
    match media_ringback_type {
        MediaRingbackType::Nanp => RingbackToneKind::Nanp,
        MediaRingbackType::Etsi => RingbackToneKind::Etsi,
    }
}

pub(crate) fn voice_rate_bps(rate: cdma_voice::VoiceRate) -> u32 {
    match rate {
        cdma_voice::VoiceRate::Full => 9600,
        cdma_voice::VoiceRate::Half => 4800,
        cdma_voice::VoiceRate::Quarter => 2400,
        cdma_voice::VoiceRate::Eighth => 1200,
    }
}
