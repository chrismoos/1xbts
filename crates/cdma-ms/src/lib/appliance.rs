use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use std::path::Path;

use crate::engine::{Diagnostics, EngineConfig, ScanConfig};
use crate::event::BroadcastSink;
use crate::ms::MsEvent;
use crate::radio::Radio;
use crate::station::{MobileStation, StationStats};

pub trait LoopbackControl: Send + Sync {
    fn page_with_sms(&self, esn: u32, text: &str) -> Result<(), String>;
    fn mobiles(&self) -> Vec<(String, String, String)>;
}

#[derive(Debug, Clone, Default)]
pub struct MsStatusSnapshot {
    pub state: String,
    pub pilot_pn: u16,
    pub sid: u16,
    pub nid: u16,
    pub base_id: u16,
    pub registered: String,
    pub system_time_chips: u64,
    pub sync_decoded: u64,
    pub overhead_updated: u64,
    pub access_probes: u64,
    pub call_state: String,
    pub voice_service_option: u16,
    pub caller_number: String,
}

/// Only voice service options may enter the incoming-call state. SMS and data pages must not ring.
fn is_voice_service_option(service_option: u16) -> bool {
    cdma_voice::VoiceCodec::from_service_option(service_option).is_some()
}

fn fold(s: &mut MsStatusSnapshot, ev: &MsEvent) {
    match ev {
        MsEvent::StateChange { to, .. } => s.state = to.clone(),
        MsEvent::PilotAcquired { pn } => s.pilot_pn = *pn as u16,
        MsEvent::SyncDecoded(sync) => {
            s.pilot_pn = sync.pilot_pn;
            s.system_time_chips = sync.sys_time;
            s.sid = sync.sid;
            s.nid = sync.nid;
            s.sync_decoded += 1;
        }
        MsEvent::OverheadUpdated { sid, nid, base_id } => {
            s.sid = *sid;
            s.nid = *nid;
            s.base_id = *base_id;
            s.overhead_updated += 1;
        }
        MsEvent::AccessStarted { reason } => {
            s.access_probes += 1;
            if reason.starts_with("registration") {
                s.registered = "pending".to_string();
            }
        }
        MsEvent::RegistrationAccepted => s.registered = "yes".to_string(),
        MsEvent::RegistrationRejected { .. } => s.registered = "no".to_string(),
        MsEvent::AccessFailed { reason, .. } if reason.starts_with("registration") => {
            s.registered = "no".to_string();
        }
        MsEvent::PageReceived { service_option, .. }
            if is_voice_service_option(*service_option) =>
        {
            s.call_state = "incoming".to_string();
            s.voice_service_option = *service_option;
            s.caller_number.clear();
        }
        MsEvent::CallRinging {
            caller_number,
            service_option,
        } if is_voice_service_option(*service_option) => {
            s.call_state = "ringing".to_string();
            s.voice_service_option = *service_option;
            s.caller_number = caller_number.clone().unwrap_or_default();
        }
        MsEvent::CallAnswered => s.call_state = "connected".to_string(),
        MsEvent::ServiceConnected { service_option }
            if is_voice_service_option(*service_option)
                && !matches!(s.call_state.as_str(), "incoming" | "ringing") =>
        {
            s.call_state = "connected".to_string();
            s.voice_service_option = *service_option;
        }
        MsEvent::TrafficReleased { .. } => {
            s.call_state = "idle".to_string();
            s.voice_service_option = 0;
            s.caller_number.clear();
        }
        _ => {}
    }
}

pub struct MsAppliance {
    station: MobileStation,
    events: broadcast::Sender<MsEvent>,
    status: Arc<Mutex<MsStatusSnapshot>>,
}

impl MsAppliance {
    /// Take a radio and start the handset. Must run inside a Tokio runtime (the
    /// status-folding task is spawned there).
    pub fn start(radio: Box<dyn Radio>, config: EngineConfig) -> Self {
        let (events, _keep) = broadcast::channel(256);
        let station = MobileStation::start(radio, config);
        station.add_event_sink(BroadcastSink::new(events.clone()));

        let status = Arc::new(Mutex::new(MsStatusSnapshot {
            state: "off".to_string(),
            registered: "no".to_string(),
            call_state: "idle".to_string(),
            ..Default::default()
        }));
        {
            let mut rx = events.subscribe();
            let status = status.clone();
            tokio::spawn(async move {
                use broadcast::error::RecvError;
                loop {
                    match rx.recv().await {
                        Ok(ev) => fold(&mut status.lock().unwrap(), &ev),
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });
        }

        MsAppliance {
            station,
            events,
            status,
        }
    }

    pub fn power_on(&self) {
        self.station.power_on();
    }

    pub fn power_off(&self) {
        self.station.power_off();
    }

    pub fn register(&self) {
        self.station.register();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MsEvent> {
        self.events.subscribe()
    }

    pub fn status(&self) -> MsStatusSnapshot {
        self.status.lock().unwrap().clone()
    }

    pub fn status_handle(&self) -> Arc<Mutex<MsStatusSnapshot>> {
        self.status.clone()
    }

    pub fn diagnostics(&self) -> Result<Diagnostics, String> {
        self.station.diagnostics()
    }

    pub fn diagnostics_handle(&self) -> impl Fn() -> Result<Diagnostics, String> + Send + 'static {
        let station = self.station.query_handle();
        move || station.diagnostics()
    }

    pub fn set_scan(&self, scan: Option<ScanConfig>) {
        self.station.set_scan(scan);
    }

    pub fn set_rx_gain(&self, gain_db: f64) -> Result<(), String> {
        self.station.set_rx_gain(gain_db)
    }

    pub fn trim_tx_calibration(
        &self,
        trim: crate::radio::TxTrim,
    ) -> Result<Option<crate::radio::TxCalibration>, String> {
        self.station.trim_tx_calibration(trim)
    }

    pub fn dump_forward(&self, path: &Path, seconds: f64) -> Result<(), String> {
        self.station.dump_forward(path, seconds)
    }

    pub fn stats(&self) -> StationStats {
        self.station.stats()
    }

    pub fn originate_sms(&self, destination: String, text: String) {
        self.station.originate_sms(destination, text);
    }

    pub fn originate(&self, service_option: u16, digits: String) {
        self.station.originate(service_option, digits);
    }

    pub fn send_dtmf_burst(&self, burst: crate::traffic::DtmfBurst) -> Result<(), String> {
        self.station.send_dtmf_burst(burst)
    }

    pub fn hang_up(&self) {
        self.station.hang_up();
    }

    pub fn answer(&self) {
        self.station.answer();
    }

    pub fn push_voice_pcm(&self, pcm: [i16; cdma_voice::SAMPLES_PER_FRAME]) {
        self.station.push_voice_pcm(pcm);
    }

    pub fn subscribe_voice(&self) -> broadcast::Receiver<[i16; cdma_voice::SAMPLES_PER_FRAME]> {
        self.station.subscribe_voice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_call_status_stays_ringing_until_answered() {
        let mut status = MsStatusSnapshot {
            call_state: "idle".to_string(),
            ..Default::default()
        };
        fold(
            &mut status,
            &MsEvent::PageReceived {
                esn: 1,
                service_option: cdma_voice::SERVICE_OPTION_EVRC_A,
            },
        );
        fold(
            &mut status,
            &MsEvent::CallRinging {
                caller_number: Some("5551212".to_string()),
                service_option: cdma_voice::SERVICE_OPTION_EVRC_A,
            },
        );
        fold(
            &mut status,
            &MsEvent::ServiceConnected {
                service_option: cdma_voice::SERVICE_OPTION_EVRC_A,
            },
        );
        assert_eq!(status.call_state, "ringing");
        assert_eq!(status.caller_number, "5551212");

        fold(&mut status, &MsEvent::CallAnswered);
        assert_eq!(status.call_state, "connected");
    }

    #[test]
    fn sms_session_does_not_change_voice_call_state() {
        let mut status = MsStatusSnapshot {
            call_state: "idle".to_string(),
            ..Default::default()
        };
        fold(
            &mut status,
            &MsEvent::PageReceived {
                esn: 1,
                service_option: 0,
            },
        );
        fold(
            &mut status,
            &MsEvent::ServiceConnected {
                service_option: cdma_common::consts::SERVICE_OPTION_SMS,
            },
        );
        assert_eq!(status.call_state, "idle");
        assert_eq!(status.voice_service_option, 0);
    }

    #[test]
    fn accepted_registration_replaces_pending_status() {
        let mut status = MsStatusSnapshot::default();
        fold(
            &mut status,
            &MsEvent::AccessStarted {
                reason: "registration(type=1)".to_string(),
            },
        );
        assert_eq!(status.registered, "pending");
        fold(&mut status, &MsEvent::RegistrationAccepted);
        assert_eq!(status.registered, "yes");
    }
}
