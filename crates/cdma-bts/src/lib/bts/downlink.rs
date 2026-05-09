use cdma_common::{bits::Bitstream, error::Error, time};
use log::{info, trace};

use crate::{
    channels::{
        WalshChannel,
        fpch::{self, ForwardPagingChannel},
        fsch::{self, ForwardSyncChannel},
        pilot::ForwardPilotChannel,
    },
    lac::{Layer2Lac, MessageControlStatusBlock},
    mac::{
        self,
        types::{AvailabilityIndication, DataRequest, MacMessage},
    },
    phy::coding::{
        block_interleaver::BitReversalInterleaver, convolutional::get_1_2_k9_encoder,
        long_code::LongCodeGenerator, symbol_repeat::SymbolRepetition,
    },
    phy::walsh::WalshGenerator,
};

use super::{
    Config, PagingWalshChannel, PilotWalshChannel, SyncWalshChannel, TxLoopState,
    settings::BtsRuntimeSettings,
};

pub(super) fn build_channels(
    config: &Config,
    runtime: &BtsRuntimeSettings,
) -> Result<(PilotWalshChannel, SyncWalshChannel, PagingWalshChannel), Error> {
    runtime.validate()?;
    let downlink = &runtime.downlink;

    let pch = WalshChannel::new(
        WalshGenerator::new::<64>(downlink.pilot.walsh_code, 1),
        ForwardPilotChannel::new(),
    );

    let fsch = WalshChannel::new(
        WalshGenerator::new::<64>(downlink.sync.walsh_code, downlink.sync.walsh_repetition),
        ForwardSyncChannel::new(fsch::Config {
            data_rate: downlink.sync.data_rate_bps,
            encoder: get_1_2_k9_encoder(),
            symbol_repeat: SymbolRepetition::new(downlink.sync.symbol_repeat),
            interleaver: BitReversalInterleaver::new(downlink.sync.interleaver.as_params()),
            pn_pilot_offset: config.pilot_offset,
        }),
    );

    let fpch = WalshChannel::new(
        WalshGenerator::new::<64>(downlink.paging.walsh_code, 1),
        ForwardPagingChannel::new(fpch::Config {
            data_rate: downlink.paging.data_rate_bps,
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(downlink.paging.interleaver.as_params()),
            long_code_generator: LongCodeGenerator::new_paging_channel(
                downlink.paging.paging_channel_number,
                config.pilot_offset as u16,
            ),
            bypass_long_code: downlink.paging.bypass_long_code,
            pn_pilot_offset: config.pilot_offset,
            force_zero_payload_bits: downlink.paging.force_zero_payload_bits,
            lc_chip_cursor: 0,
            debug_windows_left: 64,
        }),
    );

    Ok((pch, fsch, fpch))
}

pub(super) fn handle_sync_frame(
    config: &Config,
    runtime: &BtsRuntimeSettings,
    state: &mut TxLoopState,
    fsch: &SyncWalshChannel,
    chip_cursor: u64,
) -> Result<(), Error> {
    state.sync_requested_fragments += 1;

    let template = config
        .sync_channel_template
        .as_ref()
        .expect("sync_channel_template must be set");

    if state
        .current_sync_pdu
        .as_ref()
        .is_some_and(|p| p.e_pdu.len() == 0)
    {
        state.current_sync_pdu = None;
    }

    if state.current_sync_pdu.is_none() {
        let max_size = runtime.downlink.sync.availability_max_size_bits;
        let pch_num = runtime.downlink.paging.paging_channel_number;
        let resolved = resolve_timezone_cached(state, &config.timezone, &config.overhead);
        let mut tmpl = template.clone();
        tmpl.ltm_off = resolved.ltm_off;
        tmpl.daylt = resolved.daylt;
        tmpl.lp_sec = resolved.lp_sec;
        let stamped = Layer2Lac::stamp_and_serialize_sync(tmpl, chip_cursor, max_size, pch_num)?;
        state.current_sync_pdu = Some(Layer2Lac::assemble_pdu(stamped)?);
    }

    let pdu = state
        .current_sync_pdu
        .as_mut()
        .expect("sync PDU must be available at sync frame boundary");

    let max_size = runtime.downlink.sync.availability_max_size_bits;
    let frag = pdu.get_fragment(max_size);
    let dr = DataRequest {
        channel_type: mac::types::ChannelType::FSync,
        size: frag.len(),
        data: frag,
        mcsb: MessageControlStatusBlock {
            channel: mac::types::ChannelType::FSync,
            mobile_p_rev: None,
            extended_encryption: false,
            message_id: crate::lac::message_types::MessageId::SyncChannelMessage,
            length_bits: 0,
            requested_tx_time: None,
            tx_deadline: None,
            address: None,
            ack_seq: 0,
            msg_seq: 0,
            ack_req: false,
            valid_ack: false,
            overhead_mcc: 0x03ff,
            overhead_imsi_11_12: 0x7f,
        },
    };
    state.sync_sent_fragments += 1;
    fsch.channel.send_fragment(dr);
    trace!("send f-sch fragment at chip cursor {}", chip_cursor);

    Ok(())
}

pub(super) fn handle_paging_frame(
    config: &Config,
    runtime: &BtsRuntimeSettings,
    state: &mut TxLoopState,
    fpch: &PagingWalshChannel,
    chip_cursor: u64,
    hw_tick: u64,
) -> Result<(), Error> {
    let paging_frame_bits =
        runtime.downlink.paging.availability_max_size_bits * state.paging_fragments_per_frame;
    state.paging_requested_fragments += state.paging_fragments_per_frame;
    let mac_fragment = config
        .mac_layer
        .get_fragment(mac::types::ChannelType::FPch)?;
    let from_mac = mac_fragment.is_some();
    let fragment = mac_fragment.unwrap_or_else(|| DataRequest {
        channel_type: mac::types::ChannelType::FPch,
        size: paging_frame_bits,
        data: Bitstream::new_init(&vec![0u8; paging_frame_bits]),
        mcsb: MessageControlStatusBlock {
            channel: mac::types::ChannelType::FPch,
            mobile_p_rev: None,
            extended_encryption: false,
            message_id: crate::lac::message_types::MessageId::SyncChannelMessage,
            length_bits: paging_frame_bits,
            requested_tx_time: None,
            tx_deadline: None,
            address: None,
            ack_seq: 0,
            msg_seq: 0,
            ack_req: false,
            valid_ack: false,
            overhead_mcc: 0x03ff,
            overhead_imsi_11_12: 0x7f,
        },
    });
    if from_mac && fragment.mcsb.address.is_some() {
        info!(
            "bts_fpch_tx: tag={} size={} half_frames={} tx_chip={} tx_hw_tick={}",
            fragment.mcsb.message_id.tag(),
            fragment.size,
            state.paging_fragments_per_frame,
            chip_cursor,
            hw_tick,
        );
    }
    state.paging_sent_fragments += state.paging_fragments_per_frame;
    if state.paging_sent_fragments <= 8 {
        let bits = fragment
            .data
            .bits()
            .iter()
            .map(|b| if *b == 0 { '0' } else { '1' })
            .collect::<String>();
        trace!(
            "bts_paging_fragment#{} size={} bits={}",
            state.paging_sent_fragments, fragment.size, bits
        );
    }
    fpch.channel.send_fragment(fragment);
    Ok(())
}

/// Recompute the resolved timezone at most once per second. The Sync
/// Channel Message is rebuilt at superframe rate (~80 ms); refreshing every
/// frame would call `chrono::Utc::now()` plus `chrono-tz` lookups on the hot
/// path. One second is well below the 80 ms PDU boundary at which a DST
/// transition could shift the broadcast value mid-PDU.
fn resolve_timezone_cached(
    state: &mut TxLoopState,
    cfg: &cdma_common::timezone::TimezoneConfig,
    overhead: &cdma_common::overhead::OverheadParameters,
) -> cdma_common::timezone::ResolvedTimezone {
    let now = std::time::Instant::now();
    if let Some((at, cached)) = &state.timezone_cache
        && now.duration_since(*at) < std::time::Duration::from_secs(1)
    {
        return *cached;
    }
    let resolved = cdma_common::timezone::resolve(cfg, overhead, chrono::Utc::now());
    state.timezone_cache = Some((now, resolved));
    resolved
}

pub(super) fn send_availability_indications(
    config: &Config,
    runtime: &BtsRuntimeSettings,
    sync_frame_boundary: bool,
    frame_system_time: time::CdmaSystemTime,
    chip_cursor: u64,
) -> Result<(), Error> {
    if sync_frame_boundary && config.sync_channel_template.is_none() {
        config
            .mac_layer
            .send_mac_message(MacMessage::AvailabilityIndication(AvailabilityIndication {
                channel_type: mac::types::ChannelType::FSync,
                max_size: runtime.downlink.sync.availability_max_size_bits,
                system_time: frame_system_time,
                sync_superframe_start: true,
                chip_cursor,
            }))?;
    }

    Ok(())
}

pub(super) fn send_paging_frame_availability(
    config: &Config,
    runtime: &BtsRuntimeSettings,
    state: &TxLoopState,
    frame_system_time: time::CdmaSystemTime,
    chip_cursor: u64,
) -> Result<(), Error> {
    config
        .mac_layer
        .send_mac_message(MacMessage::AvailabilityIndication(AvailabilityIndication {
            channel_type: mac::types::ChannelType::FPch,
            max_size: runtime.downlink.paging.availability_max_size_bits
                * state.paging_fragments_per_frame,
            system_time: frame_system_time,
            sync_superframe_start: false,
            chip_cursor,
        }))?;
    Ok(())
}
