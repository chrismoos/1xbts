//! Packet-data orchestration glue for the BSC.
//!
//! Track C moves durable packet anchoring out of the BSC. This sibling module
//! remains as the radio-edge home for BSC-side packet assignment helpers; the
//! actual packet control boundary lives at [`crate::packet::PcfClient`].

use cdma_abis::control::typed::ForwardBurstRadioInfo;
use cdma_common::channel::TrafficRate;
use cdma_common::consts::{
    SERVICE_OPTION_ASYNC_DATA, SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_PACKET_DATA,
};
use cdma_common::sch::Rc3FschProfile;
use log::{debug, info, warn};
use uuid::Uuid;

use crate::abis_edge::ForwardBearerQueue;
use crate::addressing::{format_ms_address, is_packet_data_so};

use super::traffic_bearer::{
    send_forward_fch_bits_with_bearer_client, send_forward_sch_bits_with_bearer_client,
};
use super::traffic_forward::fsch_escam_start_time_mod32;
use super::{Bsc, TrafficChannelInfo, VoiceLegRole};

/// Request to initiate a BS-originated data call to a subscriber.
pub struct DataCallRequest {
    pub subscriber_id: Uuid,
    /// Service option: 33 (high-rate packet), 12 (asynchronous data), or 7 (packet data).
    pub service_option: u16,
}

#[derive(Default)]
pub(crate) struct PacketService;

fn packet_fch_rate(rate_bps: u32) -> Option<TrafficRate> {
    super::voice::traffic_rate_from_bps(rate_bps)
}

fn packet_primary_mux_bits(for_rc: u8, rate: TrafficRate, primary_bits: &[u8]) -> Vec<u8> {
    if for_rc == 2 || rate == TrafficRate::Full {
        let mut bits = Vec::with_capacity(primary_bits.len() + 1);
        bits.push(0);
        bits.extend_from_slice(primary_bits);
        bits
    } else {
        primary_bits.to_vec()
    }
}

impl PacketService {
    pub(crate) fn detach_session(&self, tc: &mut TrafficChannelInfo) -> Option<String> {
        Self::detach_session_inner(tc)
    }

    pub(crate) fn detach_session_inner(tc: &mut TrafficChannelInfo) -> Option<String> {
        let packet_session_id = tc.packet_session_id.take();
        if let Some(task) = tc.packet_downlink_task.take() {
            task.abort();
        }
        tc.packet_uplink_tx = None;
        packet_session_id
    }
}

impl Bsc {
    /// Initiate a BS-originated data call. Pages the mobile, and on
    /// page response assigns a traffic channel with the requested
    /// packet-data service option. The packet session starts
    /// automatically when Service Connect completes.
    pub(crate) fn initiate_bs_data_call(&mut self, req: DataCallRequest) {
        if self.paging.has_pending_page() {
            warn!("BSC: page already in progress - deferring BS-originated data call");
            return;
        }
        let Some(fwd_address) = self
            .mobiles
            .get_by_subscriber_id(req.subscriber_id)
            .map(|ms| ms.fwd_address.clone())
        else {
            warn!(
                "BSC: cannot initiate BS-originated data call, subscriber {} is not currently registered",
                req.subscriber_id
            );
            return;
        };
        let service_option = match req.service_option {
            SERVICE_OPTION_PACKET_DATA => SERVICE_OPTION_PACKET_DATA,
            SERVICE_OPTION_ASYNC_DATA => SERVICE_OPTION_ASYNC_DATA,
            _ => SERVICE_OPTION_HIGH_RATE_PACKET_DATA,
        };
        info!(
            "BSC: initiating BS-originated data call subscriber={} SO={}",
            req.subscriber_id, service_option,
        );

        // Reuse the voice page machinery: it pages the mobile and assigns a
        // traffic channel with the requested SO on page response. No voice
        // session is needed; packet bearer setup starts at Service Connect
        // Completion.
        self.queue_voice_page_for_mobile(
            &fwd_address,
            Uuid::nil(),
            service_option,
            VoiceLegRole::Callee,
            None,
            None,
            None,
        );
    }

    pub(crate) async fn start_packet_session_after_service_connect(&mut self, walsh_code: u8) {
        let Some(ms) = self.mobiles.get_by_walsh(walsh_code) else {
            return;
        };
        let Some(tc) = ms.find_traffic_channel_by_walsh(walsh_code) else {
            return;
        };
        if !is_packet_data_so(tc.service_option) {
            return;
        }
        if tc.packet_session_id.is_some() {
            info!(
                "BSC: packet session already active on walsh={}, ignoring duplicate Service Connect Completion",
                walsh_code
            );
            return;
        }
        let for_rc = tc.for_rc;
        let Some(pcf_client) = self.config.pcf_client.clone() else {
            return;
        };

        let service_option = tc.service_option;
        let pcf_metadata = crate::packet::PacketSessionMetadata {
            mobile_address: format_ms_address(&ms.fwd_address),
            // None preserves the unprovisioned/roamer case all the way to
            // the event bus, which can then forward-enrich from IMSI/ESN
            // (or ship the event with no subscriber if HLR has nothing).
            subscriber_id: ms.subscriber_id,
            phone_number: ms.phone_number.clone().unwrap_or_default(),
            imsi: ms.imsi.clone(),
            esn: ms.esn,
            traffic_walsh_code: walsh_code as u32,
        };
        let session_id = Uuid::new_v4().to_string();

        let (uplink_tx, mut downlink_rx) = match pcf_client
            .open_packet_session(session_id.clone(), service_option as u32, pcf_metadata)
            .await
        {
            Ok(channels) => channels,
            Err(e) => {
                warn!(
                    "BSC: failed to start packet session for walsh={}: {}",
                    walsh_code, e
                );
                return;
            }
        };

        // Abis Burst allocates the SCH code; ESCAM activates it on the MS.
        let bts_client = self.config.bts_client.clone();
        let sch_code: Option<u8> = self.try_activate_fsch(walsh_code).await;
        // After ESCAM, enable rate-matched SCH frames in the packet session.
        let f_sch_rate_bps = self.config.traffic_assignment.f_sch_rate_bps;
        if let Some(sch_code) = sch_code {
            if let Err(e) = pcf_client
                .set_sch_active(&session_id, true, f_sch_rate_bps)
                .await
            {
                warn!(
                    "BSC: F-SCH on walsh={} ESCAM sent but PCF set_sch_active(true) failed: {}; \
                     releasing SCH",
                    walsh_code, e
                );
                let profile = Rc3FschProfile::from_rate_bps(f_sch_rate_bps)
                    .unwrap_or_else(Rc3FschProfile::default_19k2);
                self.release_fsch_allocation(walsh_code, sch_code, profile, true, "PCF failure")
                    .await;
            }
        }
        // Re-read after the possible rollback above.
        let sch_code: Option<u8> = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .and_then(|tc| tc.sch_walsh_code);
        let sch_bearer_for_task: Option<(u8, u32)> = sch_code.map(|code| (code, 0));
        let walsh_for_log = walsh_code;
        let dl_task = tokio::spawn(async move {
            let mut dl_count: u64 = 0;
            while let Some(frame) = downlink_rx.recv().await {
                if frame.rate_bps == f_sch_rate_bps {
                    let Some((sch_code, _bearer_id)) = sch_bearer_for_task else {
                        // No SCH allocated for this call — drop silently.
                        // This happens when enable_f_sch is off or the
                        // mobile is ineligible.
                        continue;
                    };
                    if let Err(e) = send_forward_sch_bits_with_bearer_client(
                        bts_client.as_ref(),
                        0,
                        sch_code,
                        frame.rate_bps,
                        frame.bits,
                    ) {
                        warn!(
                            "BSC: failed to send SCH DL frame sch_code={}: {}",
                            sch_code, e
                        );
                    }
                    continue;
                }

                let Some(rate) = packet_fch_rate(frame.rate_bps) else {
                    warn!(
                        "BSC: dropping packet DL frame with unsupported FCH rate {}",
                        frame.rate_bps
                    );
                    continue;
                };
                let mux_bits = packet_primary_mux_bits(for_rc, rate, &frame.bits);
                dl_count += 1;
                if dl_count <= 10 || dl_count % 250 == 0 {
                    let hex: String = mux_bits
                        .chunks(8)
                        .map(|byte_bits| {
                            let mut v = 0u8;
                            for (i, &b) in byte_bits.iter().enumerate() {
                                v |= (b & 1) << (7 - i);
                            }
                            format!("{:02x}", v)
                        })
                        .collect();
                    debug!(
                        "BSC: packet DL frame #{} walsh={} rate={} mux_len={} hex={}",
                        dl_count,
                        walsh_for_log,
                        frame.rate_bps,
                        mux_bits.len(),
                        hex
                    );
                }
                if let Err(e) = send_forward_fch_bits_with_bearer_client(
                    bts_client.as_ref(),
                    0,
                    walsh_for_log,
                    for_rc,
                    mux_bits,
                    rate,
                    ForwardBearerQueue::Traffic,
                ) {
                    warn!(
                        "BSC: failed to send packet DL frame over bearer walsh={}: {}",
                        walsh_for_log, e
                    );
                }
            }
        });

        let dl_task_cell = std::sync::Mutex::new(Some(dl_task));
        let installed = self
            .mobiles
            .update_tc(walsh_code, |_, tc| {
                if tc.packet_session_id.is_some() {
                    return false;
                }
                tc.packet_session_id = Some(session_id.clone());
                tc.packet_uplink_tx = Some(uplink_tx);
                tc.packet_downlink_task = dl_task_cell.lock().unwrap().take();
                info!(
                    "BSC: packet session {} created for SO{} walsh={}",
                    session_id, tc.service_option, walsh_code
                );
                true
            })
            .unwrap_or(false);
        if !installed {
            if let Some(task) = dl_task_cell.lock().unwrap().take() {
                task.abort();
            }
        }
    }

    pub(crate) fn replace_packet_service_with_voice(&mut self, walsh_code: u8) -> Option<String> {
        self.mobiles
            .update_tc(walsh_code, |_, tc| {
                let voice_so = tc.voice_service_option?;
                if !is_packet_data_so(tc.service_option) {
                    return None;
                }
                let packet_session_id = PacketService::detach_session_inner(tc);
                tc.service_option = voice_so;
                tc.service_ref_id = tc.voice_service_ref_id.unwrap_or(tc.service_ref_id);
                tc.origination_service_option = Some(voice_so);
                packet_session_id
            })
            .flatten()
    }

    /// Allocate F-SCH through Abis Burst, then activate it with ESCAM.
    /// Returns the SCH Walsh code on success.
    pub(crate) async fn try_activate_fsch(&mut self, walsh_code: u8) -> Option<u8> {
        self.fsch_for_service_connect(walsh_code)?;
        let profile = Rc3FschProfile::from_rate_bps(self.config.traffic_assignment.f_sch_rate_bps)
            .unwrap_or_else(Rc3FschProfile::default_19k2);
        let bts_client = self.config.bts_client.clone()?;
        // The Abis reservation and ESCAM must carry the same modulo-32 start boundary.
        let start_time_mod32 = fsch_escam_start_time_mod32();
        let request = ForwardBurstRadioInfo {
            coding_indicator: profile.coding_indicator,
            qof_mask: 0,
            forward_code_channel_index: 0,
            pilot_pn_code: self.config.pilot_offset as u16,
            forward_supplemental_channel_rate: profile.num_bits_idx,
            forward_supplemental_channel_start_time: start_time_mod32,
            start_time_unit: 0,
            forward_supplemental_channel_duration: 0x0f,
        };
        let committed = bts_client
            .commit_forward_sch_burst(walsh_code, request)
            .await?;
        let sch_code = committed.forward_code_channel_index as u8;

        if let Err(e) = self.send_escam_for_fsch(walsh_code, sch_code, profile, start_time_mod32) {
            warn!(
                "BSC: F-SCH allocated (code={}) on walsh={} but ESCAM send failed: {}; \
                 releasing SCH",
                sch_code, walsh_code, e
            );
            self.release_fsch_allocation(
                walsh_code,
                sch_code,
                profile,
                false,
                "ESCAM send failure",
            )
            .await;
            return None;
        }
        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.sch_walsh_code = Some(sch_code);
            tc.sch_bearer_id = Some(sch_code as u32);
        });

        info!(
            "BSC: F-SCH activated walsh={} code={} rate={}",
            walsh_code, sch_code, profile.rate_bps
        );
        Some(sch_code)
    }

    pub(crate) async fn release_fsch_allocation(
        &mut self,
        walsh_code: u8,
        sch_code: u8,
        profile: Rc3FschProfile,
        notify_ms: bool,
        reason: &str,
    ) {
        if notify_ms && let Err(e) = self.send_escam_release_for_fsch(walsh_code, sch_code, profile)
        {
            warn!(
                "BSC: failed to send F-SCH release ESCAM walsh={} sch_code={} after {}: {}",
                walsh_code, sch_code, reason, e
            );
        }

        self.mobiles.update_tc(walsh_code, |_, tc| {
            if tc.sch_walsh_code == Some(sch_code) {
                tc.sch_walsh_code = None;
                tc.sch_bearer_id = None;
            }
        });

        let Some(bts_client) = self.config.bts_client.clone() else {
            return;
        };
        let release = ForwardBurstRadioInfo {
            coding_indicator: profile.coding_indicator,
            qof_mask: 0,
            forward_code_channel_index: sch_code as u16,
            pilot_pn_code: self.config.pilot_offset as u16,
            forward_supplemental_channel_rate: profile.num_bits_idx,
            forward_supplemental_channel_start_time: 0,
            start_time_unit: 0,
            forward_supplemental_channel_duration: 0,
        };
        if bts_client
            .commit_forward_sch_burst(walsh_code, release)
            .await
            .is_none()
        {
            warn!(
                "BSC: BTS did not confirm F-SCH release walsh={} sch_code={} after {}",
                walsh_code, sch_code, reason
            );
        }
    }

    pub(crate) fn close_packet_session_background(&self, walsh_code: u8, session_id: String) {
        info!(
            "BSC: replacing packet service with voice on walsh={}, closing packet session {}",
            walsh_code, session_id
        );
        if let Some(pcf_client) = self.config.pcf_client.clone() {
            tokio::spawn(async move {
                pcf_client.close_packet_session(&session_id).await;
            });
        }
    }

    pub(crate) async fn close_packet_session(&self, walsh_code: u8, session_id: &str) {
        info!(
            "BSC: closing packet session {} for walsh={}",
            session_id, walsh_code
        );
        if let Some(ref pcf_client) = self.config.pcf_client {
            pcf_client.close_packet_session(session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_set_two_packet_rates_map_to_rc2_tiers() {
        assert_eq!(packet_fch_rate(14_400), Some(TrafficRate::Full));
        assert_eq!(packet_fch_rate(7_200), Some(TrafficRate::Half));
        assert_eq!(packet_fch_rate(3_600), Some(TrafficRate::Quarter));
        assert_eq!(packet_fch_rate(1_800), Some(TrafficRate::Eighth));
    }

    #[test]
    fn rate_set_two_primary_frames_include_the_mux_header_at_every_rate() {
        for (rate, primary_bits, mux_bits) in [
            (TrafficRate::Full, 266, 267),
            (TrafficRate::Half, 124, 125),
            (TrafficRate::Quarter, 54, 55),
            (TrafficRate::Eighth, 20, 21),
        ] {
            let frame = packet_primary_mux_bits(2, rate, &vec![1; primary_bits]);
            assert_eq!(frame.len(), mux_bits);
            assert_eq!(frame[0], 0);
        }
    }
}
