use std::sync::atomic::Ordering;
use std::sync::mpsc;

use cdma_abis::{
    bearer::{ChannelFamily, Direction, FrameContent, ReverseFchDcchFrame, TrafficFrame},
    udp_bearer::UdpBearerDatagram,
};
use log::{info, warn};

use super::super::AccessChannelEvent;
use super::REVERSE_BEARER_SEQ;

pub(super) fn reverse_frame_content_from_rate_bps(rate_bps: u32) -> FrameContent {
    use cdma_abis::bearer::{
        REVERSE_FRAME_CONTENT_EIGHTH_RATE, REVERSE_FRAME_CONTENT_FULL_RATE,
        REVERSE_FRAME_CONTENT_HALF_RATE, REVERSE_FRAME_CONTENT_QUARTER_RATE,
    };
    match rate_bps {
        9600 => REVERSE_FRAME_CONTENT_FULL_RATE,
        4800 => REVERSE_FRAME_CONTENT_HALF_RATE,
        2700 | 2400 => REVERSE_FRAME_CONTENT_QUARTER_RATE,
        1500 | 1200 => REVERSE_FRAME_CONTENT_EIGHTH_RATE,
        _ => FrameContent::Idle,
    }
}

pub(super) fn reverse_frame_content_from_event(event: &AccessChannelEvent) -> FrameContent {
    reverse_frame_content_from_rate_bps(event.traffic_primary_rate_bps.unwrap_or(0))
}

pub(super) fn emit_reverse_primary_bearer(
    tx: &Option<mpsc::Sender<UdpBearerDatagram>>,
    event: &AccessChannelEvent,
    bts_id: u32,
    cell_id: u32,
) -> bool {
    let Some(tx) = tx else {
        warn!("emit_reverse_primary_bearer: no bearer tx configured");
        return false;
    };
    let (Some(walsh_code), Some(bits), Some(rate_bps)) = (
        event.traffic_walsh_code,
        event.traffic_primary_bits.as_ref(),
        event.traffic_primary_rate_bps,
    ) else {
        return false;
    };
    // `decoded_rdsch` events carry post-SAR LAC payloads for the local BTS
    // event path. The Abis bearer must carry the raw reverse traffic
    // information bits, emitted by `traffic_phy_frame`, so the BSC can parse
    // the MUX header and reassemble signaling itself.
    if event.decoded_rdsch.is_some() {
        return false;
    }

    // Forward every decoded primary traffic frame over bearer. The Frame
    // Content value tells the BSC how many raw information bits/MUX bits are
    // present and which rate-specific decode path applies.
    let frame_content = reverse_frame_content_from_event(event);
    if frame_content == FrameContent::Idle {
        return false;
    }
    let frame = TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: event
            .traffic_fqi_valid
            .unwrap_or(event.traffic_phy_valid.unwrap_or(true)),
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content,
        fpc_s: 0,
        eib: false,
        reverse_link_information: bits.clone(),
        message_crc: 0,
    });
    let payload = match frame.encode() {
        Ok(payload) => payload,
        Err(e) => {
            warn!(
                "rx_traffic[w{}]: failed to encode reverse bearer frame: {}",
                walsh_code, e
            );
            return false;
        }
    };
    let sent = tx
        .send(UdpBearerDatagram {
            flags: 0,
            channel_family: ChannelFamily::Fch,
            direction: Direction::Reverse,
            bts_id,
            cell_id,
            bearer_id: walsh_code as u32,
            sequence_no: REVERSE_BEARER_SEQ.fetch_add(1, Ordering::Relaxed) as u32,
            tx_frame_number: event.absolute_chip_start.unwrap_or_default() as u32,
            payload,
        })
        .is_ok();
    log::trace!(
        "emit_reverse_primary_bearer: walsh={} rate={} bits={} sent={}",
        walsh_code,
        rate_bps,
        bits.len(),
        sent
    );
    sent
}

/// Send a preamble notification as an FCH Rvs null frame (frame_content=0x7F)
/// over the Abis UDP bearer.
pub(super) fn emit_reverse_preamble_bearer(
    tx: &Option<mpsc::Sender<UdpBearerDatagram>>,
    walsh_code: u8,
    abs_chip: u64,
    bts_id: u32,
    cell_id: u32,
) {
    let Some(tx) = tx else {
        return;
    };
    let frame = TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: false,
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content: cdma_abis::bearer::REVERSE_FRAME_CONTENT_NULL,
        fpc_s: 0,
        eib: false,
        reverse_link_information: Vec::new(),
        message_crc: 0,
    });
    let payload = match frame.encode() {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "rx_traffic[w{}]: failed to encode preamble bearer frame: {}",
                walsh_code, e
            );
            return;
        }
    };
    match tx.send(UdpBearerDatagram {
        flags: 0,
        channel_family: ChannelFamily::Fch,
        direction: Direction::Reverse,
        bts_id,
        cell_id,
        bearer_id: walsh_code as u32,
        sequence_no: REVERSE_BEARER_SEQ.fetch_add(1, Ordering::Relaxed) as u32,
        tx_frame_number: abs_chip as u32,
        payload,
    }) {
        Ok(()) => info!(
            "rx_traffic[w{}]: preamble FCH Rvs null frame sent via bearer",
            walsh_code
        ),
        Err(e) => warn!(
            "rx_traffic[w{}]: failed to send preamble bearer frame: {}",
            walsh_code, e
        ),
    }
}
