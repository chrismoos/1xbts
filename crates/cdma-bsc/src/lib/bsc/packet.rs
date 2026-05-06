//! Packet-data orchestration glue for the BSC.
//!
//! Track C moves durable packet anchoring out of the BSC. This sibling module
//! remains as the radio-edge home for BSC-side packet assignment helpers; the
//! actual packet control boundary lives at [`crate::packet::PcfClient`].

use cdma_common::channel::TrafficRate;
use log::{debug, info, warn};
use uuid::Uuid;

use crate::abis_edge::ForwardBearerQueue;
use crate::addressing::{format_ms_address, is_packet_data_so};

use super::traffic_bearer::send_forward_fch_bits_with_bearer_client;
use super::{Bsc, TrafficChannelInfo, VOICE_REPLACEMENT_CON_REF, VoiceLegRole};

/// Request to initiate a BS-originated data call (SO 7 or SO 33) to a subscriber.
pub struct DataCallRequest {
    pub subscriber_id: Uuid,
    /// Service option: 33 (high-rate packet) or 7 (async data). Defaults to 33.
    pub service_option: u16,
}

#[derive(Default)]
pub(crate) struct PacketService;

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
    /// packet-data SO (7 or 33). The packet session starts
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
        let service_option = if req.service_option == 7 { 7u16 } else { 33u16 };
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
            subscriber_id: ms.subscriber_id.unwrap_or_default().to_string(),
            phone_number: ms.phone_number.clone().unwrap_or_default(),
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

        // Issue-tracked: F-SCH allocation remains disabled until ESCAM is
        // validated end-to-end. Re-enable allocate_sch_rc3() + ESCAM send once
        // the mobile accepts the supplemental channel assignment.
        let sch_bearer_for_task: Option<(u8, u32)> = None;
        let bts_client = self.config.bts_client.clone();
        let walsh_for_log = walsh_code;
        let dl_task = tokio::spawn(async move {
            let mut dl_count: u64 = 0;
            while let Some(frame) = downlink_rx.recv().await {
                if frame.rate_bps == 19200 {
                    if let Some((w32_code, _bearer_id)) = sch_bearer_for_task {
                        warn!(
                            "BSC: SCH packet DL bearer send not active for w32={}",
                            w32_code
                        );
                    }
                    continue;
                }

                let rate = match frame.rate_bps {
                    9600 => TrafficRate::Full,
                    4800 => TrafficRate::Half,
                    2700 | 2400 => TrafficRate::Quarter,
                    1500 | 1200 => TrafficRate::Eighth,
                    _ => TrafficRate::Eighth,
                };
                // MuxPDU Type 1: full-rate frames need MM=0 prepended
                // (primary traffic only).
                let mux_bits = if rate == TrafficRate::Full {
                    let mut bits = Vec::with_capacity(172);
                    bits.push(0u8);
                    bits.extend_from_slice(&frame.bits);
                    bits
                } else {
                    frame.bits
                };
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
                if !is_packet_data_so(tc.service_option)
                    || tc.voice_connection_ref != Some(VOICE_REPLACEMENT_CON_REF)
                {
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
