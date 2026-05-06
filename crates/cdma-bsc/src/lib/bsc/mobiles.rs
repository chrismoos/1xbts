//! BSC mobile-station registry queries and lifecycle bookkeeping.
//!
//! WS-0 PR3 sibling module per
//! `docs/architecture-update/09-pr3-method-map.md`.

use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(test)]
use std::ops::{Index, IndexMut};

use cdma_abis::control::typed::CallConnectionReference;
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::paging_messages::{MsAddress, MsPageAddress};
use cdma_common::metrics::{RxMeasurement, RxMeasurementKey};
use cdma_common::paging::{imsi_11_12_to_digits, imsi_s_to_digits_checked, mcc_to_digits};
use cdma_voice::VoiceCodec;
use log::{info, warn};
use uuid::Uuid;

use crate::addressing::{format_ms_address, format_ms_page_address, is_packet_data_so};
use crate::power_control::TrafficChannelPowerSnapshot;

use super::{
    Bsc, EventService, TrafficChannelInfo, VoiceLegRole, mark_reverse_regular_msg_seq_received,
    traffic_channel_power_snapshot,
};

/// Public snapshot of a registered mobile, for gRPC/UI consumption.
#[derive(Debug, Clone, Default)]
pub struct MobileInfo {
    /// Forward-link address summary (e.g. "ESN:0x12345678")
    pub address: String,
    /// Page address summary (e.g. "IMSI_S:s1=123,s2=456")
    pub page_address: String,
    /// Structured forward-link address actually chosen for signaling.
    pub forward_address: Option<MsAddress>,
    /// Computed page address (fully resolved IMSI components).
    pub page_address_detail: Option<MsPageAddress>,
    pub state: String,
    pub mob_p_rev: u8,
    // Structured identity fields
    pub esn: Option<u32>,
    pub imsi: Option<String>,
    /// Computed paging slot (0..2047) per C.S0005-E 2.6.7.1.
    pub pgslot: Option<u16>,
    /// Slot cycle index from the mobile's registration.
    pub slot_cycle_index: u8,
    /// Signal quality from last access probe.
    pub snr_db: Option<f32>,
    pub signal_power_db: Option<f32>,
    pub demod_quality_pct: Option<f32>,
    /// Rx power in dBm (absolute), only present when rx_reference_dbm is configured.
    pub rx_power_dbm: Option<f32>,
    /// Rx level in dBFS (relative to ADC full-scale), referred back to the ADC
    /// input by subtracting the RX matched-filter gain. Always populated when
    /// the finger has accumulated raw input power, regardless of whether
    /// rx_reference_dbm is configured.
    pub rx_level_dbfs: Option<f32>,
    /// Milliseconds since Unix epoch of last access probe.
    pub last_heard_ms: Option<u64>,
    /// Subscriber phone number from HLR (if resolved).
    pub phone_number: Option<String>,
    /// Subscriber display name from HLR (if resolved).
    pub subscriber_display_name: Option<String>,
    /// HLR subscriber ID (UUID string).
    pub subscriber_id: Option<String>,
    /// Abis call connection reference (if a traffic channel is assigned).
    pub traffic_call_connection_ref: Option<CallConnectionReference>,
    /// Traffic channel Walsh code (if assigned).
    pub traffic_walsh_code: Option<u8>,
    /// Traffic channel service option (if assigned).
    pub traffic_service_option: Option<u16>,
    /// Voice call sub-state label (None for non-voice calls).
    pub voice_call_state: Option<String>,
    /// Closed-loop power control snapshot. Present iff the mobile has an
    /// active reverse traffic channel.
    pub traffic_power: Option<TrafficChannelPowerSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MsState {
    Registered,
    Paged,
    PageResponseReceived,
    /// Traffic channel has been assigned; waiting for MS to tune and send preamble.
    TrafficAssigning,
    /// Traffic channel is active (preamble received or frames flowing).
    TrafficActive,
}

#[derive(Debug)]
pub(crate) struct MobileStation {
    pub(crate) fwd_address: MsAddress,
    /// Known reverse-link identities, stored separately from the chosen
    /// forward/page address so the UI can show the full identity set.
    pub(crate) esn: Option<u32>,
    pub(crate) imsi: Option<String>,
    /// Cached IMSI components derived from the full IMSI string.
    /// Used to compute page addresses and OTA encoding without re-parsing.
    imsi_m_s1: Option<u32>,
    imsi_m_s2: Option<u16>,
    imsi_mcc: Option<u16>,
    imsi_11_12: Option<u8>,
    pub(crate) mob_p_rev: u8,
    pub(crate) state: MsState,
    pub(crate) last_msg_seq: u8,
    /// Reverse access channel (r-csch) duplicate detection per C.S0004-E 3.1.1.2.2.2.
    /// MSG_SEQ_RCVD[n] = true means MSG_SEQ n has already been processed.
    pub(crate) access_msg_seq_rcvd: [bool; 8],
    /// When the last r-csch PDU was received from this mobile.
    /// Per C.S0004-E 3.1.1.2.2.2, the BS considers an MS inactive on the
    /// r-csch after an implementation-defined timeout and clears
    /// MSG_SEQ_RCVD on the next received PDU.
    pub(crate) last_access_activity: Option<Instant>,
    /// Slot cycle index reported by the mobile (3 bits, 0..7).
    pub(crate) slot_cycle_index: u8,
    /// Computed paging slot (0..2047) per C.S0005-E 2.6.7.1, if IMSI is known.
    pub(crate) pgslot: Option<u16>,
    /// Signal quality from last access probe.
    pub(crate) snr_db: Option<f32>,
    pub(crate) signal_power_db: Option<f32>,
    pub(crate) raw_power_db: Option<f32>,
    pub(crate) demod_quality_pct: Option<f32>,
    /// Milliseconds since Unix epoch when last access probe was received.
    pub(crate) last_heard_ms: Option<u64>,
    /// Subscriber phone number from HLR (if resolved).
    pub(crate) phone_number: Option<String>,
    /// Subscriber display name from HLR (if resolved).
    pub(crate) subscriber_display_name: Option<String>,
    /// HLR subscriber ID.
    pub(crate) subscriber_id: Option<Uuid>,
    /// Canonical IMSI string from HLR (e.g. "310001234567890").
    pub(crate) canonical_imsi: Option<String>,
    /// Forward Radio Configurations supported by the mobile (from FCH capability).
    pub(crate) for_supported_rcs: Vec<u8>,
    /// Reverse Radio Configurations supported by the mobile (from FCH capability).
    pub(crate) rev_supported_rcs: Vec<u8>,
    /// Preferred forward Radio Configuration from Origination, if reported.
    pub(crate) for_preferred_rc: Option<u8>,
    /// Preferred reverse Radio Configuration from Origination, if reported.
    pub(crate) rev_preferred_rc: Option<u8>,
    /// Mobile requested reverse FCH eighth-rate gating (from Origination).
    pub(crate) rev_fch_gating_req: bool,
    /// Single traffic channel, per C.S0005-E. New allocation tears down any
    /// existing channel first.
    pub(crate) traffic_channel: Option<TrafficChannelInfo>,
}

impl MobileStation {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_for_test(
        fwd_address: MsAddress,
        esn: Option<u32>,
        imsi: Option<String>,
        mob_p_rev: u8,
        state: MsState,
        slot_cycle_index: u8,
        pgslot: Option<u16>,
    ) -> Self {
        let (imsi_m_s1, imsi_m_s2) = imsi
            .as_deref()
            .and_then(cdma_common::paging::imsi_s_from_imsi)
            .unzip();
        let imsi_mcc = imsi.as_deref().and_then(|s| {
            if s.len() >= 3 {
                cdma_common::paging::mcc_from_digits(&s[..3])
            } else {
                None
            }
        });
        let imsi_11_12 = imsi.as_deref().and_then(|s| {
            if s.len() >= 5 {
                cdma_common::paging::imsi_11_12_from_digits(&s[3..5])
            } else {
                None
            }
        });
        Self {
            fwd_address,
            esn,
            imsi,
            imsi_m_s1,
            imsi_m_s2,
            imsi_mcc,
            imsi_11_12,
            mob_p_rev,
            state,
            last_msg_seq: 0,
            access_msg_seq_rcvd: [false; 8],
            last_access_activity: None,
            slot_cycle_index,
            pgslot,
            snr_db: None,
            signal_power_db: None,
            raw_power_db: None,
            demod_quality_pct: None,
            last_heard_ms: None,
            phone_number: None,
            subscriber_display_name: None,
            subscriber_id: None,
            canonical_imsi: None,
            for_supported_rcs: vec![],
            rev_supported_rcs: vec![],
            for_preferred_rc: None,
            rev_preferred_rc: None,
            rev_fch_gating_req: false,
            traffic_channel: None,
        }
    }

    /// Compute the page address for this mobile given current overhead.
    ///
    /// The mobile's stored MCC/IMSI_11_12 are always fully resolved (never
    /// "implied by overhead"). The paging encoder compares against current
    /// overhead at send time to select the minimum GPM subclass.
    pub(crate) fn page_address(&self) -> Option<MsPageAddress> {
        if let (Some(s1), Some(s2)) = (self.imsi_m_s1, self.imsi_m_s2) {
            Some(MsPageAddress::ImsiS {
                imsi_m_s1: s1,
                imsi_m_s2: s2,
                mcc: self.imsi_mcc,
                imsi_11_12: self.imsi_11_12,
            })
        } else if let Some(esn) = self.esn {
            Some(MsPageAddress::Esn(esn))
        } else {
            None
        }
    }

    pub(crate) fn imsi_s_components(&self) -> Option<(u32, u16)> {
        match (self.imsi_m_s1, self.imsi_m_s2) {
            (Some(s1), Some(s2)) => Some((s1, s2)),
            _ => None,
        }
    }

    pub(crate) fn matches_imsi_s(&self, s1: u32, s2: u16) -> bool {
        self.imsi_m_s1 == Some(s1) && self.imsi_m_s2 == Some(s2)
    }

    pub(crate) fn traffic_channel(&self) -> Option<&TrafficChannelInfo> {
        self.traffic_channel.as_ref()
    }

    pub(crate) fn traffic_channel_mut(&mut self) -> Option<&mut TrafficChannelInfo> {
        self.traffic_channel.as_mut()
    }

    pub(crate) fn has_traffic_channel(&self) -> bool {
        self.traffic_channel.is_some()
    }

    pub(crate) fn current_traffic_walsh(&self) -> Option<u8> {
        self.traffic_channel.as_ref().map(|tc| tc.walsh_code)
    }

    pub(crate) fn traffic_service_option_or(&self, default: u16) -> u16 {
        self.traffic_channel()
            .map(|tc| tc.service_option)
            .unwrap_or(default)
    }

    pub(crate) fn traffic_voice_context(&self) -> Option<(Option<Uuid>, Option<VoiceLegRole>)> {
        self.traffic_channel
            .as_ref()
            .map(|tc| (tc.voice_session_id, tc.voice_leg_role))
    }

    pub(crate) fn traffic_voice_context_by_walsh(
        &self,
        walsh_code: u8,
    ) -> Option<(Option<Uuid>, Option<VoiceLegRole>)> {
        self.find_traffic_channel_by_walsh(walsh_code)
            .map(|tc| (tc.voice_session_id, tc.voice_leg_role))
    }

    pub(crate) fn pending_traffic_assignment(&self) -> Option<&TrafficChannelInfo> {
        if self.state == MsState::TrafficAssigning {
            self.traffic_channel()
        } else {
            None
        }
    }

    pub(crate) fn pending_packet_traffic_assignment(&self) -> Option<&TrafficChannelInfo> {
        self.pending_traffic_assignment()
            .filter(|tc| is_packet_data_so(tc.service_option))
    }

    pub(crate) fn assign_traffic_channel(&mut self, channel: TrafficChannelInfo) {
        self.traffic_channel = Some(channel);
    }

    pub(crate) fn mark_traffic_channel_assigned(&mut self, walsh_code: u8) -> bool {
        let Some(tc) = self.find_traffic_channel_by_walsh_mut(walsh_code) else {
            return false;
        };
        tc.mark_assigned();
        true
    }

    pub(crate) fn active_voice_walsh(&self) -> Option<u8> {
        self.traffic_channel
            .as_ref()
            .filter(|tc| VoiceCodec::from_service_option(tc.service_option).is_some())
            .map(|tc| tc.walsh_code)
    }

    pub(crate) fn voice_traffic_channel(&self) -> Option<&TrafficChannelInfo> {
        self.traffic_channel()
            .filter(|tc| VoiceCodec::from_service_option(tc.service_option).is_some())
    }

    pub(crate) fn voice_release_target(&self) -> Option<VoiceReleaseTarget> {
        let tc = self.traffic_channel()?;
        if tc.is_releasing() {
            return None;
        }
        Some(VoiceReleaseTarget {
            walsh_code: tc.walsh_code,
            release_voice_service_only: tc.voice_service_option.is_some()
                && is_packet_data_so(tc.service_option),
            a1_call_id: tc.a1_call_id,
            a1_clear_state: tc.a1_clear_state,
        })
    }

    pub(crate) fn existing_voice_walsh_for_assignment(
        &self,
        bind_existing_traffic: bool,
        session_id: Uuid,
        leg_role: VoiceLegRole,
    ) -> Option<u8> {
        let tc = self.traffic_channel()?;
        if bind_existing_traffic
            || (tc.voice_session_id == Some(session_id) && tc.voice_leg_role == Some(leg_role))
        {
            Some(tc.walsh_code)
        } else {
            None
        }
    }

    pub(crate) fn has_voice_session(&self, session_id: Uuid) -> bool {
        self.traffic_channel
            .as_ref()
            .is_some_and(|tc| tc.voice_session_id == Some(session_id))
    }

    pub(crate) fn msc_circuit_walsh(&self, circuit_id: u16) -> Option<u8> {
        self.traffic_channel.as_ref().and_then(|tc| {
            if tc.msc_circuit_id == Some(circuit_id) {
                Some(tc.walsh_code)
            } else {
                None
            }
        })
    }

    pub(crate) fn msc_circuit_id(&self) -> Option<u16> {
        self.traffic_channel
            .as_ref()
            .and_then(|tc| tc.msc_circuit_id)
    }

    pub(crate) fn set_msc_circuit_id(
        &mut self,
        circuit_id: u16,
    ) -> Option<&mut TrafficChannelInfo> {
        let tc = self.traffic_channel_mut()?;
        tc.msc_circuit_id = Some(circuit_id);
        Some(tc)
    }

    pub(crate) fn set_msc_bearer_local_addr(&mut self, addr: std::net::SocketAddr) -> bool {
        let Some(tc) = self.traffic_channel_mut() else {
            return false;
        };
        tc.msc_bearer_local_addr = Some(addr);
        true
    }

    pub(crate) fn a1_call_walsh(&self, call_id: u64) -> Option<u8> {
        self.traffic_channel.as_ref().and_then(|tc| {
            if tc.a1_call_id == Some(call_id) {
                Some(tc.walsh_code)
            } else {
                None
            }
        })
    }

    pub(crate) fn is_voice_connected(&self) -> bool {
        self.traffic_channel
            .as_ref()
            .is_some_and(|tc| tc.is_voice_connected())
    }

    pub(crate) fn has_msc_media_for_session(&self, session_id: Uuid) -> bool {
        self.traffic_channel.as_ref().is_some_and(|tc| {
            tc.voice_session_id == Some(session_id) && tc.msc_circuit_id.is_some()
        })
    }

    /// Find the traffic channel if its walsh code matches.
    pub(crate) fn find_traffic_channel_by_walsh(
        &self,
        walsh_code: u8,
    ) -> Option<&TrafficChannelInfo> {
        self.traffic_channel
            .as_ref()
            .filter(|tc| tc.walsh_code == walsh_code)
    }

    /// Find the traffic channel (mutable) if its walsh code matches.
    pub(crate) fn find_traffic_channel_by_walsh_mut(
        &mut self,
        walsh_code: u8,
    ) -> Option<&mut TrafficChannelInfo> {
        self.traffic_channel
            .as_mut()
            .filter(|tc| tc.walsh_code == walsh_code)
    }

    /// Remove the traffic channel if its walsh code matches, returning it.
    pub(crate) fn remove_traffic_channel_by_walsh(
        &mut self,
        walsh_code: u8,
        bearer: Option<&Arc<cdma_ios::VoiceBearerManager>>,
    ) -> Option<TrafficChannelInfo> {
        if self
            .traffic_channel
            .as_ref()
            .is_some_and(|tc| tc.walsh_code == walsh_code)
        {
            let tc = self.traffic_channel.take();
            if let Some(tc_ref) = &tc {
                if let (Some(cid), Some(bearer)) = (tc_ref.msc_circuit_id, bearer) {
                    bearer.close_circuit(cid);
                }
            }
            tc
        } else {
            None
        }
    }

    /// Resolve a pending traffic retry target to the traffic channel.
    /// Transition to a new state.
    ///
    /// r-csch duplicate state is intentionally not cleared here. Per
    /// C.S0004-E 3.1.1.2.2.2, MSG_SEQ_RCVD should be cleared when the MS is
    /// inactive on the common channel, not merely because Layer 3 moved the
    /// mobile back to `Registered`. This allows an immediate retransmission of
    /// the same access PDU (for example a Page Response retry) to be detected
    /// and acknowledged as a duplicate.
    pub(crate) fn set_state(&mut self, new_state: MsState) {
        self.state = new_state;
    }
}

#[derive(Debug, Default)]
pub(crate) struct MobileRegistry {
    entries: Vec<MobileStation>,
}

pub(crate) struct MobileRegistryService {
    registry: MobileRegistry,
    events: EventService,
}

pub(crate) struct StaleTrafficChannel {
    /// `walsh_code` is the unique stable key for the traffic channel; the
    /// owning mobile is resolved at teardown time via the registry.
    pub(crate) walsh_code: u8,
    pub(crate) inactive_secs: u64,
    pub(crate) channel_state_label: Option<&'static str>,
    pub(crate) voice_session_id: Option<Uuid>,
    pub(crate) voice_leg_role: Option<VoiceLegRole>,
    pub(crate) a1_call_id: Option<u64>,
    pub(crate) a1_clear_state: super::A1ClearState,
}

pub(crate) struct AccessRegistrationUpdate {
    pub(crate) fwd_address: MsAddress,
    pub(crate) registration_imsi: Option<String>,
    pub(crate) imsi_mcc: Option<u16>,
    pub(crate) imsi_11_12: Option<u8>,
    pub(crate) mob_p_rev: u8,
    pub(crate) last_msg_seq: u8,
    pub(crate) slot_cycle_index: u8,
    pub(crate) pgslot: Option<u16>,
    pub(crate) activity_now: Instant,
    pub(crate) last_heard_ms: u64,
    pub(crate) explicit_registration: bool,
}

pub(crate) struct VoiceReleaseTarget {
    pub(crate) walsh_code: u8,
    pub(crate) release_voice_service_only: bool,
    pub(crate) a1_call_id: Option<u64>,
    pub(crate) a1_clear_state: super::A1ClearState,
}

/// Outcome of `apply_access_registration`. Currently just the stable
/// `fwd_address` the entry is keyed on; an `is_new` flag was previously
/// here to gate MSC notification, but the BSC now notifies on every event
/// (the MSC dedupes via `upsert_mobile_seen`).
pub(crate) struct RegistrationOutcome {
    pub(crate) fwd_address: MsAddress,
}

impl MobileRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl MobileRegistryService {
    pub(crate) fn new(events: EventService) -> Self {
        Self {
            registry: MobileRegistry::new(),
            events,
        }
    }

    pub(crate) fn publish_snapshot(&self, rx_reference_dbm: Option<f64>) {
        self.events
            .publish_mobile_snapshot(self.registry.snapshot(rx_reference_dbm));
    }

    pub(crate) fn apply_access_registration(
        &mut self,
        event: &AccessChannelEvent,
        update: AccessRegistrationUpdate,
    ) -> RegistrationOutcome {
        self.registry.apply_access_registration(event, update)
    }

    pub(crate) fn apply_subscriber_resolution(
        &mut self,
        fwd_address: &MsAddress,
        subscriber_id: Uuid,
        phone_number: String,
        display_name: Option<String>,
        canonical_imsi: Option<String>,
    ) -> bool {
        self.registry.apply_subscriber_resolution(
            fwd_address,
            subscriber_id,
            phone_number,
            display_name,
            canonical_imsi,
        )
    }

    pub(crate) fn apply_origination_capabilities(
        &mut self,
        addr: &MsAddress,
        event: &AccessChannelEvent,
    ) -> bool {
        self.registry.apply_origination_capabilities(addr, event)
    }

    pub(crate) fn mark_page_response_received(
        &mut self,
        fwd_address: Option<&MsAddress>,
    ) -> Option<MsAddress> {
        self.registry.mark_page_response_received(fwd_address)
    }

    pub(crate) fn mark_registered(&mut self, fwd_address: &MsAddress) -> bool {
        self.registry.mark_registered(fwd_address)
    }

    pub(crate) fn mark_page_pending(&mut self, fwd_address: &MsAddress) -> bool {
        self.registry.mark_page_pending(fwd_address)
    }

    pub(crate) fn resolve_originating_sms_sender(
        &self,
        fwd_address: &MsAddress,
    ) -> Option<(String, Option<Uuid>)> {
        self.registry.resolve_originating_sms_sender(fwd_address)
    }
}

impl Deref for MobileRegistryService {
    type Target = MobileRegistry;

    fn deref(&self) -> &Self::Target {
        &self.registry
    }
}

impl DerefMut for MobileRegistryService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.registry
    }
}

impl<'a> IntoIterator for &'a MobileRegistryService {
    type Item = &'a MobileStation;
    type IntoIter = std::slice::Iter<'a, MobileStation>;

    fn into_iter(self) -> Self::IntoIter {
        self.registry.iter()
    }
}

// Mutable iteration over the service is intentionally not provided.
// Use `update`, `update_tc`, or named command methods so mutations stay
// scoped and discoverable.

// `MobileRegistry` deliberately does NOT impl `Deref<Target=[MobileStation]>`.
// Indexing into the registry is forbidden in production code so that no
// caller can hold a `usize` across mutations of the underlying storage.
// Tests retain index-based access via the `cfg(test)` impls below.
#[cfg(test)]
impl Index<usize> for MobileRegistry {
    type Output = MobileStation;

    fn index(&self, idx: usize) -> &Self::Output {
        &self.entries[idx]
    }
}

#[cfg(test)]
impl IndexMut<usize> for MobileRegistry {
    fn index_mut(&mut self, idx: usize) -> &mut Self::Output {
        &mut self.entries[idx]
    }
}

impl MobileRegistry {
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn push(&mut self, mobile: MobileStation) {
        self.entries.push(mobile);
    }

    /// Count of currently-tracked mobiles (any state). Replaces the old
    /// `len()` slice-deref method to make the call site read intentionally
    /// rather than as if poking a Vec.
    pub(crate) fn tracked_count(&self) -> usize {
        self.entries.len()
    }

    /// Read-only iteration over registered mobiles. Used by the gRPC snapshot
    /// builder, the stale-channel scan, and other read-only sweeps.
    /// Mutating iteration is intentionally not exposed — see `update`,
    /// `update_tc`, `for_each_active_voice`, and `release_paged_without_tc`.
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, MobileStation> {
        self.entries.iter()
    }

    fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&MobileStation) -> bool,
    {
        self.entries.retain(f);
    }

    // ---- lookups by stable key (return refs, never indexes) ----

    pub(crate) fn get(&self, addr: &MsAddress) -> Option<&MobileStation> {
        let idx = self.position(addr)?;
        Some(&self.entries[idx])
    }

    pub(crate) fn get_by_imsi(&self, imsi: &str) -> Option<&MobileStation> {
        self.entries.iter().find(|ms| {
            ms.canonical_imsi.as_deref() == Some(imsi) || ms.imsi.as_deref() == Some(imsi)
        })
    }

    pub(crate) fn get_by_subscriber_id(&self, subscriber_id: Uuid) -> Option<&MobileStation> {
        self.entries
            .iter()
            .find(|ms| ms.subscriber_id == Some(subscriber_id))
    }

    pub(crate) fn get_by_session_leg(
        &self,
        session_id: Uuid,
        leg_role: VoiceLegRole,
    ) -> Option<&MobileStation> {
        self.entries.iter().find(|ms| {
            ms.traffic_channel.as_ref().is_some_and(|tc| {
                tc.voice_session_id == Some(session_id) && tc.voice_leg_role == Some(leg_role)
            })
        })
    }

    pub(crate) fn get_by_walsh(&self, walsh_code: u8) -> Option<&MobileStation> {
        self.entries
            .iter()
            .find(|ms| ms.find_traffic_channel_by_walsh(walsh_code).is_some())
    }

    /// Read-only access to a traffic channel by its (unique) Walsh code.
    pub(crate) fn get_traffic_channel(&self, walsh_code: u8) -> Option<&TrafficChannelInfo> {
        self.entries
            .iter()
            .find_map(|ms| ms.find_traffic_channel_by_walsh(walsh_code))
    }

    /// Mutable access to a traffic channel by its (unique) Walsh code.
    /// `&mut TrafficChannelInfo` is the only mutable reference the registry
    /// hands out — the parent `MobileStation` is reachable for mutation only
    /// via `update` / `update_tc` (which scope the borrow to a closure).
    pub(crate) fn get_traffic_channel_mut(
        &mut self,
        walsh_code: u8,
    ) -> Option<&mut TrafficChannelInfo> {
        self.entries
            .iter_mut()
            .find_map(|ms| ms.find_traffic_channel_by_walsh_mut(walsh_code))
    }

    /// Resolve a stable address from a Walsh code (e.g. for log lines or
    /// call sites that need the mobile address downstream).
    pub(crate) fn address_by_walsh(&self, walsh_code: u8) -> Option<MsAddress> {
        self.get_by_walsh(walsh_code)
            .map(|ms| ms.fwd_address.clone())
    }

    /// Resolve `(fwd_address, walsh_code)` for an A1 call. Returns the
    /// stable keys; callers do not see the registry's internal index.
    pub(crate) fn locate_a1_call(&self, call_id: u64) -> Option<(MsAddress, u8)> {
        self.entries.iter().find_map(|ms| {
            ms.a1_call_walsh(call_id)
                .map(|walsh| (ms.fwd_address.clone(), walsh))
        })
    }

    /// Resolve `(fwd_address, walsh_code)` for an MSC bearer circuit ID.
    pub(crate) fn locate_msc_circuit(&self, circuit_id: u16) -> Option<(MsAddress, u8)> {
        self.entries.iter().find_map(|ms| {
            ms.msc_circuit_walsh(circuit_id)
                .map(|walsh| (ms.fwd_address.clone(), walsh))
        })
    }

    /// Walsh codes of all traffic channels currently carrying voice.
    /// Replaces `active_voice_entries` (which leaked indexes).
    pub(crate) fn active_voice_walsh_codes(&self) -> Vec<u8> {
        self.entries
            .iter()
            .filter_map(|ms| ms.active_voice_walsh())
            .collect()
    }

    pub(crate) fn has_msc_media_for_session(&self, session_id: Uuid) -> bool {
        self.entries
            .iter()
            .any(|ms| ms.has_msc_media_for_session(session_id))
    }

    pub(crate) fn has_voice_session(&self, session_id: Uuid) -> bool {
        self.entries
            .iter()
            .any(|ms| ms.has_voice_session(session_id))
    }

    // ---- closure-based mutation (replace caller-held idx + [idx]) ----

    /// Run `f` against the addressed mobile if it exists. The mutable borrow
    /// is bounded by the closure body, so callers cannot stash a
    /// `&mut MobileStation` outside this scope.
    pub(crate) fn update<R>(
        &mut self,
        addr: &MsAddress,
        f: impl FnOnce(&mut MobileStation) -> R,
    ) -> Option<R> {
        let idx = self.position(addr)?;
        Some(f(&mut self.entries[idx]))
    }

    /// Same as `update` but keyed by Walsh code and exposing the parent
    /// mobile *and* its traffic channel together for sites that need both.
    pub(crate) fn update_tc<R>(
        &mut self,
        walsh_code: u8,
        f: impl FnOnce(&mut MobileStation, &mut TrafficChannelInfo) -> R,
    ) -> Option<R> {
        for ms in self.entries.iter_mut() {
            if ms
                .traffic_channel
                .as_ref()
                .is_some_and(|tc| tc.walsh_code == walsh_code)
            {
                // Split the borrow safely by first taking the TC out, calling
                // `f` on (`ms`, `tc`), then putting the TC back. This sidesteps
                // the borrow checker's inability to see that `ms` and
                // `ms.traffic_channel` are disjoint sub-fields when both are
                // re-borrowed from `ms`.
                let mut tc = ms.traffic_channel.take().expect("checked above");
                let result = f(ms, &mut tc);
                ms.traffic_channel = Some(tc);
                return Some(result);
            }
        }
        None
    }

    /// Iterate every mobile that currently has an active voice traffic
    /// channel, calling `f(&mut ms, walsh)` for each. The borrow is scoped
    /// per-iteration; no caller-held idx, no slice escape.
    pub(crate) fn for_each_active_voice(&mut self, mut f: impl FnMut(&mut MobileStation, u8)) {
        for ms in self.entries.iter_mut() {
            if let Some(walsh) = ms.active_voice_walsh() {
                f(ms, walsh);
            }
        }
    }

    /// Move every paged mobile without a traffic channel back to
    /// `Registered`. Used when an A1 voice page is cancelled before any
    /// mobile has answered. Replaces a hand-rolled `for ms in &mut self.mobiles`
    /// loop in a1.rs.
    pub(crate) fn release_paged_without_tc(&mut self) {
        for ms in self.entries.iter_mut() {
            if matches!(ms.state, MsState::Paged) && !ms.has_traffic_channel() {
                ms.set_state(MsState::Registered);
            }
        }
    }

    pub(crate) fn stale_traffic_channels<F>(
        &self,
        timeout: Duration,
        mut last_bts_enqueue_at: F,
    ) -> Vec<StaleTrafficChannel>
    where
        F: FnMut(u8) -> Option<Instant>,
    {
        self.entries
            .iter()
            .filter_map(|ms| {
                let tc = ms.traffic_channel()?;
                if tc.is_releasing() {
                    return None;
                }
                let latest = tc.latest_activity_for_idle_check(last_bts_enqueue_at(tc.walsh_code));
                if latest.elapsed() <= timeout {
                    return None;
                }
                Some(StaleTrafficChannel {
                    walsh_code: tc.walsh_code,
                    inactive_secs: tc.last_activity_at.elapsed().as_secs(),
                    channel_state_label: Some(tc.channel_state.label()),
                    voice_session_id: tc.voice_session_id,
                    voice_leg_role: tc.voice_leg_role,
                    a1_call_id: tc.a1_call_id,
                    a1_clear_state: tc.a1_clear_state,
                })
            })
            .collect()
    }
}

// Read-only iteration is provided via the inherent `iter()` method below.
// Mutable iteration is intentionally not provided — use the `update*` /
// `for_each_*` closure forms so the registry can scope every mutation.
impl<'a> IntoIterator for &'a MobileRegistry {
    type Item = &'a MobileStation;
    type IntoIter = std::slice::Iter<'a, MobileStation>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl MobileRegistry {
    pub(crate) fn snapshot(&self, rx_reference_dbm: Option<f64>) -> Vec<MobileInfo> {
        self.iter()
            .map(|ms| MobileInfo {
                address: format_ms_address(&ms.fwd_address),
                page_address: ms
                    .page_address()
                    .as_ref()
                    .map_or("none".to_string(), format_ms_page_address),
                forward_address: Some(ms.fwd_address.clone()),
                page_address_detail: ms.page_address(),
                state: format!("{:?}", ms.state),
                mob_p_rev: ms.mob_p_rev,
                esn: ms.esn,
                imsi: ms.imsi.clone(),
                pgslot: ms.pgslot,
                slot_cycle_index: ms.slot_cycle_index,
                snr_db: ms.snr_db,
                signal_power_db: ms.signal_power_db,
                demod_quality_pct: ms.demod_quality_pct,
                rx_power_dbm: ms
                    .raw_power_db
                    .and_then(|rp| rx_reference_dbm.map(|ref_dbm| rp + ref_dbm as f32)),
                rx_level_dbfs: ms.raw_power_db,
                last_heard_ms: ms.last_heard_ms,
                phone_number: ms.phone_number.clone(),
                subscriber_display_name: ms.subscriber_display_name.clone(),
                subscriber_id: ms.subscriber_id.map(|id| id.to_string()),
                traffic_call_connection_ref: ms
                    .traffic_channel
                    .as_ref()
                    .map(|tc| tc.call_connection_ref),
                traffic_walsh_code: ms.traffic_channel.as_ref().map(|tc| tc.walsh_code),
                traffic_service_option: ms.traffic_channel.as_ref().map(|tc| tc.service_option),
                voice_call_state: ms
                    .traffic_channel
                    .as_ref()
                    .map(|tc| tc.channel_state.label().to_string()),
                traffic_power: ms
                    .traffic_channel
                    .as_ref()
                    .map(traffic_channel_power_snapshot),
            })
            .collect()
    }

    /// Refresh per-mobile activity bookkeeping after an access event lands.
    /// Returns `true` if the mobile was found and updated.
    pub(crate) fn record_activity(
        &mut self,
        addr: &MsAddress,
        event: &AccessChannelEvent,
        last_heard_ms: u64,
        now: Instant,
    ) -> bool {
        let Some(idx) = self.position(addr) else {
            return false;
        };
        let ms = &mut self.entries[idx];
        ms.last_access_activity = Some(now);
        ms.last_heard_ms = Some(last_heard_ms);
        ms.snr_db = event.snr_db.or(ms.snr_db);
        ms.signal_power_db = event.signal_power_db.or(ms.signal_power_db);
        ms.raw_power_db = event.raw_power_db.or(ms.raw_power_db);
        ms.demod_quality_pct = event.demod_quality_pct.or(ms.demod_quality_pct);
        true
    }

    pub(crate) fn apply_origination_capabilities(
        &mut self,
        addr: &MsAddress,
        event: &AccessChannelEvent,
    ) -> bool {
        let Some(idx) = self.position(addr) else {
            return false;
        };
        let ms = &mut self.entries[idx];
        if !event.for_supported_rcs.is_empty() {
            ms.for_supported_rcs = event.for_supported_rcs.clone();
        }
        if !event.rev_supported_rcs.is_empty() {
            ms.rev_supported_rcs = event.rev_supported_rcs.clone();
        }
        if event.for_rc_pref.is_some() {
            ms.for_preferred_rc = event.for_rc_pref;
        }
        if event.rev_rc_pref.is_some() {
            ms.rev_preferred_rc = event.rev_rc_pref;
        }
        if let Some(gating_req) = event.rev_fch_gating_req {
            ms.rev_fch_gating_req = gating_req;
        }
        true
    }

    pub(crate) fn apply_subscriber_resolution(
        &mut self,
        fwd_address: &MsAddress,
        subscriber_id: Uuid,
        phone_number: String,
        display_name: Option<String>,
        canonical_imsi: Option<String>,
    ) -> bool {
        let Some(idx) = self.position(fwd_address) else {
            return false;
        };
        let ms = &mut self.entries[idx];
        ms.phone_number = Some(phone_number);
        ms.subscriber_display_name = display_name;
        ms.subscriber_id = Some(subscriber_id);
        ms.canonical_imsi = canonical_imsi;
        true
    }

    /// Mark a page response received. Resolves the responding mobile by
    /// `fwd_address` if provided (preferring a Paged entry), falling back
    /// to the first mobile in `Paged` state. Returns the resolved address
    /// so callers can act on it without holding an index.
    pub(crate) fn mark_page_response_received(
        &mut self,
        fwd_address: Option<&MsAddress>,
    ) -> Option<MsAddress> {
        let idx = if let Some(addr) = fwd_address {
            self.position(addr)
                .filter(|&i| self.entries[i].state == MsState::Paged)
                .or_else(|| {
                    self.entries
                        .iter()
                        .position(|ms| ms.state == MsState::Paged)
                })
        } else {
            self.entries
                .iter()
                .position(|ms| ms.state == MsState::Paged)
        }?;
        self.entries[idx].set_state(MsState::PageResponseReceived);
        Some(self.entries[idx].fwd_address.clone())
    }

    pub(crate) fn mark_registered(&mut self, fwd_address: &MsAddress) -> bool {
        self.set_state(fwd_address, MsState::Registered)
    }

    pub(crate) fn mark_page_pending(&mut self, fwd_address: &MsAddress) -> bool {
        self.set_state(fwd_address, MsState::Paged)
    }

    /// Transition a mobile to a new MS-level state. Returns `true` if the
    /// mobile was found.
    pub(crate) fn set_state(&mut self, fwd_address: &MsAddress, state: MsState) -> bool {
        let Some(idx) = self.position(fwd_address) else {
            return false;
        };
        self.entries[idx].set_state(state);
        true
    }

    pub(crate) fn resolve_originating_sms_sender(
        &self,
        fwd_address: &MsAddress,
    ) -> Option<(String, Option<Uuid>)> {
        let ms = self.get(fwd_address)?;
        let originating_number = ms.phone_number.clone()?;
        let originating_subscriber_id = ms.subscriber_id;
        Some((originating_number, originating_subscriber_id))
    }

    /// Apply an access-channel registration update. Returns the stable
    /// `MsAddress` the entry is keyed on plus an `is_new` flag so callers
    /// can distinguish a brand-new mobile (notify MSC, trigger welcome SMS)
    /// from a refresh of an already-known one.
    pub(crate) fn apply_access_registration(
        &mut self,
        event: &AccessChannelEvent,
        update: AccessRegistrationUpdate,
    ) -> RegistrationOutcome {
        if let Some(idx) = self.position(&update.fwd_address) {
            let ms = &mut self.entries[idx];
            Self::update_identity(
                ms,
                event,
                update.registration_imsi,
                update.imsi_mcc,
                update.imsi_11_12,
            );
            ms.mob_p_rev = update.mob_p_rev;
            if update.explicit_registration {
                ms.set_state(MsState::Registered);
            }
            ms.last_msg_seq = update.last_msg_seq;
            ms.slot_cycle_index = update.slot_cycle_index;
            if update.pgslot.is_some() {
                ms.pgslot = update.pgslot;
            }
            ms.snr_db = event.snr_db.or(ms.snr_db);
            ms.signal_power_db = event.signal_power_db.or(ms.signal_power_db);
            ms.raw_power_db = event.raw_power_db.or(ms.raw_power_db);
            ms.demod_quality_pct = event.demod_quality_pct.or(ms.demod_quality_pct);
            ms.last_heard_ms = Some(update.last_heard_ms);
            ms.last_access_activity = Some(update.activity_now);
            if update.explicit_registration {
                if !event.for_supported_rcs.is_empty() {
                    ms.for_supported_rcs = event.for_supported_rcs.clone();
                }
                if !event.rev_supported_rcs.is_empty() {
                    ms.rev_supported_rcs = event.rev_supported_rcs.clone();
                }
                if event.for_rc_pref.is_some() {
                    ms.for_preferred_rc = event.for_rc_pref;
                }
                if event.rev_rc_pref.is_some() {
                    ms.rev_preferred_rc = event.rev_rc_pref;
                }
            }
            info!(
                "BSC: {}registration update for {} (mob_p_rev={} pgslot={:?} slot_cycle_index={})",
                if update.explicit_registration {
                    ""
                } else {
                    "implicit "
                },
                format_ms_address(&update.fwd_address),
                update.mob_p_rev,
                ms.pgslot,
                update.slot_cycle_index,
            );
            return RegistrationOutcome {
                fwd_address: update.fwd_address,
            };
        }

        info!(
            "BSC: {}registration (new) addr={} mob_p_rev={} pgslot={:?} slot_cycle_index={}",
            if update.explicit_registration {
                ""
            } else {
                "implicit "
            },
            format_ms_address(&update.fwd_address),
            update.mob_p_rev,
            update.pgslot,
            update.slot_cycle_index,
        );
        let new_addr = update.fwd_address.clone();
        self.entries.push(MobileStation {
            fwd_address: update.fwd_address,
            esn: event.esn,
            imsi: update.registration_imsi,
            imsi_m_s1: event.imsi_m_s1,
            imsi_m_s2: event.imsi_m_s2,
            imsi_mcc: update.imsi_mcc,
            imsi_11_12: update.imsi_11_12,
            mob_p_rev: update.mob_p_rev,
            state: MsState::Registered,
            last_msg_seq: update.last_msg_seq,
            access_msg_seq_rcvd: [false; 8],
            last_access_activity: Some(update.activity_now),
            slot_cycle_index: update.slot_cycle_index,
            pgslot: update.pgslot,
            snr_db: event.snr_db,
            signal_power_db: event.signal_power_db,
            raw_power_db: event.raw_power_db,
            demod_quality_pct: event.demod_quality_pct,
            last_heard_ms: Some(update.last_heard_ms),
            phone_number: None,
            subscriber_display_name: None,
            subscriber_id: None,
            canonical_imsi: None,
            for_supported_rcs: event.for_supported_rcs.clone(),
            rev_supported_rcs: event.rev_supported_rcs.clone(),
            for_preferred_rc: event.for_rc_pref,
            rev_preferred_rc: event.rev_rc_pref,
            rev_fch_gating_req: event.rev_fch_gating_req.unwrap_or(false),
            traffic_channel: None,
        });
        let idx = self.entries.len() - 1;
        if let Some(msg_seq) = event.msg_seq {
            mark_reverse_regular_msg_seq_received(
                &mut self.entries[idx].access_msg_seq_rcvd,
                msg_seq,
            );
        }
        RegistrationOutcome {
            fwd_address: new_addr,
        }
    }

    pub(crate) fn merge_rx_measurements(
        &mut self,
        measurements: Vec<(RxMeasurementKey, RxMeasurement)>,
    ) -> bool {
        let mut updated = false;
        for (key, meas) in measurements {
            let pos = match &key {
                RxMeasurementKey::Esn(esn) => {
                    self.entries.iter().position(|ms| ms.esn == Some(*esn))
                }
                RxMeasurementKey::Imsi(imsi) => self
                    .entries
                    .iter()
                    .position(|ms| ms.imsi.as_deref() == Some(imsi.as_str())),
            };
            if let Some(i) = pos {
                let ms = &mut self.entries[i];
                ms.snr_db = meas.snr_db.or(ms.snr_db);
                ms.signal_power_db = meas.signal_power_db.or(ms.signal_power_db);
                ms.raw_power_db = meas.raw_power_db.or(ms.raw_power_db);
                ms.demod_quality_pct = meas.demod_quality_pct.or(ms.demod_quality_pct);
                updated = true;
            }
        }
        updated
    }

    pub(crate) fn update_identity(
        ms: &mut MobileStation,
        event: &AccessChannelEvent,
        registration_imsi: Option<String>,
        resolved_mcc: Option<u16>,
        resolved_imsi_11_12: Option<u8>,
    ) {
        ms.esn = event.esn.or(ms.esn);
        if let Some(imsi) = registration_imsi {
            ms.imsi = Some(imsi);
        }
        if let Some(imsi_m_s1) = event.imsi_m_s1 {
            ms.imsi_m_s1 = Some(imsi_m_s1);
        }
        if let Some(imsi_m_s2) = event.imsi_m_s2 {
            ms.imsi_m_s2 = Some(imsi_m_s2);
        }
        if let Some(mcc) = resolved_mcc {
            ms.imsi_mcc = Some(mcc);
        }
        if let Some(imsi_11_12) = resolved_imsi_11_12 {
            ms.imsi_11_12 = Some(imsi_11_12);
        }
    }

    /// Internal: position of a mobile in `entries`. Indexes never escape
    /// this file — callers outside `mobiles.rs` use stable keys via the
    /// `get*` / `update*` API.
    fn position(&self, addr: &MsAddress) -> Option<usize> {
        self.entries.iter().position(|ms| ms.fwd_address == *addr)
    }

    pub(crate) fn evict_idle(&mut self, timeout: Duration) -> usize {
        let before = self.entries.len();
        self.entries.retain(|ms| {
            if ms.traffic_channel.is_some() {
                return true;
            }
            ms.last_access_activity
                .is_some_and(|last| last.elapsed() < timeout)
        });
        before - self.entries.len()
    }
}

impl Bsc {
    /// Publish a snapshot of the registered-mobile table on the
    /// `mobiles_tx` watch channel for UI / management consumers. No-op
    /// when no transmitter is configured.
    pub(crate) fn publish_mobiles(&self) {
        self.mobiles.publish_snapshot(self.config.rx_reference_dbm);
    }

    /// Refresh activity bookkeeping for a registered mobile after an
    /// access event lands. Updates per-mobile signal metrics and the
    /// idle-eviction timestamp. Returns `true` if the mobile was found.
    pub(crate) fn record_mobile_activity(
        &mut self,
        addr: &MsAddress,
        event: &AccessChannelEvent,
        now: Instant,
    ) -> bool {
        let last_heard_ms = super::access::event_last_heard_ms(event);
        self.mobiles
            .record_activity(addr, event, last_heard_ms, now)
    }

    /// Drain BTS access-channel signal quality measurements and merge them
    /// into the registered mobile table by matching on ESN or IMSI.
    pub(crate) fn apply_rx_measurements(&mut self) {
        let measurements = match self.config.bts_client.as_ref() {
            Some(client) => client.drain_rx_measurements(),
            None => return,
        };
        if measurements.is_empty() {
            return;
        }
        if self.mobiles.merge_rx_measurements(measurements) {
            self.publish_mobiles();
        }
    }

    /// Construct a full current registration IMSI string from access-channel
    /// identity fields when the mobile provides enough digits. For class-0
    /// IMSIs, MCC and IMSI_11_12 may be omitted from the access message when
    /// they are implied by extended-system-parameters overhead.
    pub(crate) fn derive_registration_imsi(&self, event: &AccessChannelEvent) -> Option<String> {
        if let Some(imsi) = event.imsi.as_ref() {
            return Some(imsi.clone());
        }

        let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) else {
            warn!(
                "BSC: cannot derive IMSI — access event missing IMSI_M_S1/S2 \
                 (esn={:?}, imsi_class={:?})",
                event.esn, event.imsi_class,
            );
            return None;
        };
        let Some(imsi_s) = imsi_s_to_digits_checked(s1, s2) else {
            warn!(
                "BSC: cannot derive IMSI — IMSI_S digit conversion failed \
                 (s1=0x{:06X}, s2=0x{:03X})",
                s1, s2,
            );
            return None;
        };

        let defaults = &self
            .config
            .paging
            .message_defaults
            .extended_system_parameters;
        // Class 0 and legacy MSID formats (imsi_class == None) use overhead
        // MCC/IMSI_11_12 for the digits the mobile omitted. Class 1 always
        // carries its own, so no fallback needed.
        let use_overhead = event.imsi_class.is_none() || event.imsi_class == Some(0);
        let fallback_mcc = if use_overhead && defaults.mcc <= 999 {
            Some(defaults.mcc)
        } else {
            None
        };
        let fallback_imsi_11_12 = if use_overhead && defaults.imsi_11_12 <= 99 {
            Some(defaults.imsi_11_12)
        } else {
            None
        };

        let resolved_mcc = event.imsi_mcc.or(fallback_mcc);
        let resolved_imsi_11_12 = event.imsi_11_12.or(fallback_imsi_11_12);

        let Some(mcc) = resolved_mcc.and_then(mcc_to_digits) else {
            warn!(
                "BSC: cannot derive IMSI — no MCC available \
                 (event_mcc={:?}, fallback={:?}, overhead_mcc={}, imsi_class={:?})",
                event.imsi_mcc, fallback_mcc, defaults.mcc, event.imsi_class,
            );
            return None;
        };
        let Some(imsi_11_12) = resolved_imsi_11_12.and_then(imsi_11_12_to_digits) else {
            warn!(
                "BSC: cannot derive IMSI — no IMSI_11_12 available \
                 (event={:?}, fallback={:?}, overhead={}, imsi_class={:?})",
                event.imsi_11_12, fallback_imsi_11_12, defaults.imsi_11_12, event.imsi_class,
            );
            return None;
        };
        Some(format!("{mcc}{imsi_11_12}{imsi_s}"))
    }

    /// Drop registered mobiles that have been silent past
    /// `mobile_idle_timeout_s` and have no active traffic channel.
    /// Returns the number of mobiles evicted. No-op when the timeout
    /// is zero.
    pub(crate) fn evict_idle_mobiles(&mut self) -> usize {
        if self.config.mobile_idle_timeout_s == 0 {
            return 0;
        }

        let mobile_timeout = Duration::from_secs(self.config.mobile_idle_timeout_s);
        self.mobiles.evict_idle(mobile_timeout)
    }
}
