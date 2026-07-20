//! AN-side adapter to the aggregated event bus.
//!
//! Publishes HRPD session/access/traffic events to the bus via gRPC
//! (`cdma_events::EventPublisher`), never in-process. The gRPC service builds
//! the typed `events.v1` HRPD messages from session and air state and hands
//! them here; this adapter wraps them in the bus envelope and publishes.
//!
//! Per-AT traffic telemetry (DRC, reverse-pilot SNR) is rate-limited here: the
//! per-slot DRC/pilot streams are far too fast for the event bus, so a periodic
//! sample is emitted at most once per second with the latest DRC and pilot SNR.
//! DRC changes are still published separately, also rate-limited, so operator
//! logs show rate transitions without waiting for the next sample tick.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdma_events::EventPublisher;
use cdma_events::proto::{
    AnNetworkEvent, EventSource, HrpdAccessEvent, HrpdSessionEvent, HrpdTrafficEvent,
    HrpdTrafficReason, MobileIdentity, NetworkEvent, an_network_event, network_event,
};

/// Most recent buffered events kept per UATI for the session-history query.
/// 256 covers the web timeline's 200-row cap with headroom.
const RECENT_EVENTS_PER_UATI: usize = 256;
/// Bound on the number of distinct UATIs retained, so a churn of short-lived
/// sessions can't grow the buffer without limit. A single active terminal never
/// hits this.
const RECENT_EVENT_UATIS_MAX: usize = 32;
/// DRC can flap several times inside one active data burst. Keep transitions
/// visible, but do not let them dominate the user-facing event bus.
const DRC_EVENT_MIN_INTERVAL: Duration = Duration::from_millis(500);
const TELEMETRY_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct AtTelemetry {
    last_drc: Option<u32>,
    last_drc_event_at: Option<Instant>,
    last_snr_db_tenths: i32,
    last_mac_index: u32,
    last_sample_event_at: Option<Instant>,
}

/// One buffered event in receive order. `received_ms` is the AN's receive-time
/// stamp (for ordering on the history query); the published/bus event itself is
/// not modified by buffering.
#[derive(Clone)]
pub struct RecentRecord {
    pub received_ms: u64,
    pub kind: RecentKind,
}

#[derive(Clone)]
pub enum RecentKind {
    Session(HrpdSessionEvent),
    Access(HrpdAccessEvent),
    Traffic(HrpdTrafficEvent),
}

/// Publishes HRPD events to the aggregated event bus on behalf of the AN, and
/// keeps a bounded per-UATI ring of recent session/traffic events so a freshly
/// opened session view can load history before streaming live.
pub struct AnEventSink {
    publisher: EventPublisher,
    color_code: u32,
    telemetry: Mutex<HashMap<u32, AtTelemetry>>,
    history: Mutex<HashMap<u32, VecDeque<RecentRecord>>>,
    identities: Mutex<HashMap<u32, MobileIdentity>>,
    full_uatis: Mutex<HashMap<u32, cdma_events::proto::HrpdUati>>,
}

impl AnEventSink {
    pub fn new(publisher: EventPublisher, color_code: u32) -> Self {
        Self {
            publisher,
            color_code,
            telemetry: Mutex::new(HashMap::new()),
            history: Mutex::new(HashMap::new()),
            identities: Mutex::new(HashMap::new()),
            full_uatis: Mutex::new(HashMap::new()),
        }
    }

    /// Sector color code that scopes UATIs on this AN. Producers stamp it on
    /// events that don't otherwise carry the sector.
    pub fn color_code(&self) -> u32 {
        self.color_code
    }

    pub fn session(&self, mut event: HrpdSessionEvent) {
        stamp_timestamp(&mut event.timestamp_ns);
        self.record_full_uati(event.uati, event.full_uati.clone());
        self.buffer(event.uati, RecentKind::Session(event.clone()));
        self.publish(an_network_event::Event::Session(event));
    }

    pub fn access(&self, mut event: HrpdAccessEvent) {
        stamp_timestamp(&mut event.timestamp_ns);
        self.enrich_access_identity(&mut event);
        self.record_full_uati(event.uati, event.full_uati.clone());
        self.record_full_uati(event.receive_ati, event.full_uati.clone());
        if event.uati != 0 {
            self.buffer(event.uati, RecentKind::Access(event.clone()));
        }
        self.publish(an_network_event::Event::Access(event));
    }

    pub fn traffic(&self, mut event: HrpdTrafficEvent) {
        stamp_timestamp(&mut event.timestamp_ns);
        self.enrich_traffic_identity(&mut event);
        self.record_full_uati(event.uati, event.full_uati.clone());
        self.record_full_uati(event.receive_ati, event.full_uati.clone());
        self.buffer(event.uati, RecentKind::Traffic(event.clone()));
        self.publish(an_network_event::Event::Traffic(event));
    }

    /// Most recent buffered records for a UATI, oldest first. `limit == 0`
    /// returns all buffered records (up to the per-UATI cap).
    pub fn recent(&self, uati: u32, limit: usize) -> Vec<RecentRecord> {
        let Ok(map) = self.history.lock() else {
            return Vec::new();
        };
        let Some(q) = map.get(&uati) else {
            return Vec::new();
        };
        let skip = if limit == 0 {
            0
        } else {
            q.len().saturating_sub(limit)
        };
        q.iter().skip(skip).cloned().collect()
    }

    pub fn record_identity(&self, uati: u32, identity: MobileIdentity) {
        if uati == 0 {
            return;
        }
        if let Ok(mut map) = self.identities.lock() {
            map.insert(uati, identity);
        }
    }

    /// Append one record to the per-UATI ring, dropping the oldest record (or
    /// the least-recently-active UATI) once a cap is reached.
    fn buffer(&self, uati: u32, kind: RecentKind) {
        let record = RecentRecord {
            received_ms: wall_clock_ms(),
            kind,
        };
        let Ok(mut map) = self.history.lock() else {
            return;
        };
        if !map.contains_key(&uati) && map.len() >= RECENT_EVENT_UATIS_MAX {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, q)| q.back().map(|r| r.received_ms).unwrap_or(0))
                .map(|(k, _)| *k)
            {
                map.remove(&oldest);
            }
        }
        let q = map.entry(uati).or_default();
        if q.len() >= RECENT_EVENTS_PER_UATI {
            q.pop_front();
        }
        q.push_back(record);
    }

    pub fn maybe_emit_reverse_pilot_snr(&self, uati: u32, mac_index: u32, snr_db_tenths: i32) {
        let Some((mac_index, drc, snr_db_tenths)) = ({
            let Ok(mut map) = self.telemetry.lock() else {
                return;
            };
            let at = map.entry(uati).or_default();
            at.last_mac_index = mac_index;
            at.last_snr_db_tenths = snr_db_tenths;
            let now = Instant::now();
            if at
                .last_sample_event_at
                .is_some_and(|last| now.duration_since(last) < TELEMETRY_SAMPLE_INTERVAL)
            {
                None
            } else {
                at.last_sample_event_at = Some(now);
                Some((
                    at.last_mac_index,
                    at.last_drc.unwrap_or(0),
                    at.last_snr_db_tenths,
                ))
            }
        }) else {
            return;
        };
        self.traffic(HrpdTrafficEvent {
            timestamp_ns: 0,
            uati,
            full_uati: self.full_uati_for(uati),
            receive_ati: self.receive_ati_for(uati),
            reason: HrpdTrafficReason::ReversePilotSnrUpdated as i32,
            mac_index,
            drc_value: drc,
            payload: Vec::new(),
            reverse_pilot_snr_db_tenths: snr_db_tenths,
            direction: cdma_events::proto::HrpdDirection::Rx as i32,
            decoded_messages: Vec::new(),
            payload_length_bytes: 0,
        });
    }

    /// Emit a DRC-updated traffic event only when the AT's reported DRC value
    /// changes, attaching the most recent reverse-pilot SNR. The per-slot DRC
    /// stream itself is never published.
    pub fn maybe_emit_drc(&self, uati: u32, mac_index: u32, drc_value: u32) {
        let mut periodic_sample = None;
        let changed_sample = {
            let Ok(mut map) = self.telemetry.lock() else {
                return;
            };
            let at = map.entry(uati).or_default();
            at.last_mac_index = mac_index;
            let now = Instant::now();
            if at
                .last_sample_event_at
                .is_none_or(|last| now.duration_since(last) >= TELEMETRY_SAMPLE_INTERVAL)
            {
                at.last_sample_event_at = Some(now);
                periodic_sample = Some((at.last_mac_index, drc_value, at.last_snr_db_tenths));
            }
            if at.last_drc == Some(drc_value) {
                None
            } else if at
                .last_drc_event_at
                .is_some_and(|last| now.duration_since(last) < DRC_EVENT_MIN_INTERVAL)
            {
                at.last_drc = Some(drc_value);
                None
            } else {
                at.last_drc = Some(drc_value);
                at.last_drc_event_at = Some(now);
                Some((at.last_mac_index, drc_value, at.last_snr_db_tenths))
            }
        };
        if changed_sample.is_some() {
            periodic_sample = None;
        }
        if let Some((mac_index, drc_value, snr)) = periodic_sample {
            self.traffic(HrpdTrafficEvent {
                timestamp_ns: 0,
                uati,
                full_uati: self.full_uati_for(uati),
                receive_ati: self.receive_ati_for(uati),
                reason: HrpdTrafficReason::ReversePilotSnrUpdated as i32,
                mac_index,
                drc_value,
                payload: Vec::new(),
                reverse_pilot_snr_db_tenths: snr,
                direction: cdma_events::proto::HrpdDirection::Rx as i32,
                decoded_messages: Vec::new(),
                payload_length_bytes: 0,
            });
        }
        if let Some((mac_index, drc_value, snr)) = changed_sample {
            self.traffic(HrpdTrafficEvent {
                timestamp_ns: 0,
                uati,
                full_uati: self.full_uati_for(uati),
                receive_ati: self.receive_ati_for(uati),
                reason: HrpdTrafficReason::DrcUpdated as i32,
                mac_index,
                drc_value,
                payload: Vec::new(),
                reverse_pilot_snr_db_tenths: snr,
                direction: cdma_events::proto::HrpdDirection::Rx as i32,
                decoded_messages: Vec::new(),
                payload_length_bytes: 0,
            });
        }
    }

    /// Drop cached telemetry for an AT on teardown so a reused UATI starts
    /// fresh (and re-emits its first DRC).
    pub fn forget(&self, uati: u32) {
        if let Ok(mut map) = self.telemetry.lock() {
            map.remove(&uati);
        }
        if let Ok(mut map) = self.identities.lock() {
            map.remove(&uati);
        }
        if let Ok(mut map) = self.full_uatis.lock() {
            map.remove(&uati);
        }
    }

    fn record_full_uati(&self, uati: u32, full_uati: Option<cdma_events::proto::HrpdUati>) {
        if uati == 0 {
            return;
        }
        let Some(full_uati) = full_uati else {
            return;
        };
        if let Ok(mut map) = self.full_uatis.lock() {
            map.insert(uati, full_uati.clone());
            let receive_ati =
                ((full_uati.color_code & 0xff) << 24) | (full_uati.compact_uati32 & 0x00ff_ffff);
            if receive_ati != 0 {
                map.insert(receive_ati, full_uati);
            }
        }
    }

    fn full_uati_for(&self, uati: u32) -> Option<cdma_events::proto::HrpdUati> {
        self.full_uatis
            .lock()
            .ok()
            .and_then(|map| map.get(&uati).cloned())
    }

    fn receive_ati_for(&self, uati: u32) -> u32 {
        self.full_uati_for(uati)
            .map(|full_uati| {
                ((full_uati.color_code & 0xff) << 24) | (full_uati.compact_uati32 & 0x00ff_ffff)
            })
            .filter(|receive_ati| *receive_ati != 0)
            .unwrap_or(uati)
    }

    fn enrich_access_identity(&self, event: &mut HrpdAccessEvent) {
        if event.full_uati.is_none() {
            event.full_uati = self
                .full_uati_for(event.uati)
                .or_else(|| self.full_uati_for(event.receive_ati));
        }
        if event.receive_ati == 0 {
            event.receive_ati = self.receive_ati_for(event.uati);
        }
    }

    fn enrich_traffic_identity(&self, event: &mut HrpdTrafficEvent) {
        if event.full_uati.is_none() {
            event.full_uati = self
                .full_uati_for(event.uati)
                .or_else(|| self.full_uati_for(event.receive_ati));
        }
        if event.receive_ati == 0 || event.receive_ati == event.uati {
            event.receive_ati = self.receive_ati_for(event.uati);
        }
    }

    fn publish(&self, event: an_network_event::Event) {
        let identity = self.identity_for_event(&event);
        self.publisher.publish(NetworkEvent {
            timestamp: None,
            source: EventSource::An as i32,
            sequence: 0,
            producer_instance: self.publisher.producer_instance().to_string(),
            identity,
            subscriber: None,
            body: Some(network_event::Body::An(AnNetworkEvent {
                event: Some(event),
            })),
        });
    }

    fn identity_for_event(&self, event: &an_network_event::Event) -> Option<MobileIdentity> {
        let uati = match event {
            an_network_event::Event::Session(event) => event.uati,
            an_network_event::Event::Access(event) => event.uati,
            an_network_event::Event::Traffic(event) => event.uati,
        };
        if uati == 0 {
            return None;
        }
        self.identities
            .lock()
            .ok()
            .and_then(|map| map.get(&uati).cloned())
    }
}

/// Wall-clock milliseconds since the Unix epoch, used only to display/order
/// buffered history records. Never written back onto a published event.
fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn wall_clock_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn stamp_timestamp(timestamp_ns: &mut u64) {
    if *timestamp_ns == 0 {
        *timestamp_ns = wall_clock_ns();
    }
}
