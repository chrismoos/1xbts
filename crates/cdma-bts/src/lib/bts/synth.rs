use std::time::Instant;

use cdma_common::{error::Error, time};
use num::complex::Complex32;

use crate::{
    channels::Channel,
    phy::spread::{PnSequence, Spreader},
};

use super::{
    PagingWalshChannel, PilotWalshChannel, SyncWalshChannel, TxLoopState,
    handle::TrafficChannelWrapper, settings::BtsRuntimeSettings,
};

use cdma_common::consts::SR1_CHIPS_PER_FRAME;

pub(super) fn aligned_spreader(
    pilot_offset: usize,
    short_code_length_chips: usize,
    chip_cursor: u64,
) -> Spreader {
    let mut spreader = Spreader::new(PnSequence::new(pilot_offset, short_code_length_chips));
    spreader.align_to_chip(chip_cursor);
    spreader
}

/// Drain Add/Remove commands into the TX-private working list, align any
/// unstarted channels to the next 20 ms frame boundary, and produce a
/// `(gain, channel_handle)` snapshot ready for synthesis. No locks held.
fn snapshot_traffic_channels_into(
    tx_pool: &mut super::handle::TxPool,
    chip_cursor: u64,
    pilot_offset_chips: u64,
    out: &mut Vec<(f32, TrafficChannelWrapper)>,
) {
    tx_pool.drain_commands();
    out.clear();
    for tx_slot in tx_pool.slots_mut() {
        if !tx_slot.lc_aligned {
            let offset = (chip_cursor - pilot_offset_chips) % SR1_CHIPS_PER_FRAME;
            let start_chip = if offset == 0 {
                chip_cursor
            } else {
                chip_cursor + (SR1_CHIPS_PER_FRAME - offset)
            };
            log::info!(
                "bts_tx: aligning traffic channel walsh={} start_chip={} chip_cursor={}",
                tx_slot.walsh_code,
                start_chip,
                chip_cursor,
            );
            tx_slot.slot.channel.advance_lc_to_chip(start_chip);
            tx_slot.start_chip = Some(start_chip);
            tx_slot.lc_aligned = true;
        }
        let Some(start) = tx_slot.start_chip else {
            continue;
        };
        if chip_cursor < start {
            continue;
        }
        if !tx_slot.frame_align_verified {
            assert_eq!(
                chip_cursor,
                start,
                "traffic channel walsh={} missed frame boundary: \
                 chip_cursor={} start_chip={} overshoot={}",
                tx_slot.walsh_code,
                chip_cursor,
                start,
                chip_cursor - start,
            );
            tx_slot.frame_align_verified = true;
        }
        let gain = tx_slot.slot.gain();
        out.push((gain, tx_slot.slot.channel.clone()));
    }
}

pub(super) fn synthesize_block(
    runtime: &BtsRuntimeSettings,
    tx_scale: f32,
    state: &mut TxLoopState,
    gen_start: Instant,
    pch: &PilotWalshChannel,
    fsch: &SyncWalshChannel,
    fpch: &PagingWalshChannel,
    spreader: &mut Spreader,
    synth_block: &mut [Complex32],
    block_size: usize,
    frame_system_time: time::CdmaSystemTime,
    chip_cursor: u64,
) -> Result<(), Error> {
    let synth_start = Instant::now();

    let t0 = Instant::now();
    state.scratch_pilot.clear();
    pch.next_block_into(&mut state.scratch_pilot, block_size, frame_system_time);
    state.synth_pilot_us += t0.elapsed().as_micros() as u64;

    let t0 = Instant::now();
    state.scratch_sync.clear();
    fsch.next_block_into(&mut state.scratch_sync, block_size, frame_system_time);
    state.synth_fsch_us += t0.elapsed().as_micros() as u64;

    let t0 = Instant::now();
    state.scratch_paging.clear();
    fpch.next_block_into(&mut state.scratch_paging, block_size, frame_system_time);
    state.synth_fpch_us += t0.elapsed().as_micros() as u64;

    let t0 = Instant::now();
    let snap_start = Instant::now();
    snapshot_traffic_channels_into(
        &mut state.tx_pool,
        chip_cursor,
        state.pilot_offset_chips,
        &mut state.scratch_tc_snapshot,
    );
    let snap_us = snap_start.elapsed().as_micros() as u64;
    while state.scratch_tc_blocks.len() < state.scratch_tc_snapshot.len() {
        state.scratch_tc_blocks.push((0.0, Vec::new()));
    }
    state
        .scratch_tc_blocks
        .truncate(state.scratch_tc_snapshot.len());
    let mut tc_sum_us = 0u64;
    let mut tc_max_us = 0u64;
    for (i, (gain, ch)) in state.scratch_tc_snapshot.iter().enumerate() {
        let (g_slot, buf) = &mut state.scratch_tc_blocks[i];
        *g_slot = *gain;
        buf.clear();
        let tc_start = Instant::now();
        ch.next_block_into(buf, block_size, frame_system_time);
        let dt = tc_start.elapsed().as_micros() as u64;
        tc_sum_us += dt;
        if dt > tc_max_us {
            tc_max_us = dt;
        }
    }
    state.synth_ftch_us += t0.elapsed().as_micros() as u64;
    state.last_snap_us = snap_us;
    state.last_tc_n = state.scratch_tc_snapshot.len();
    state.last_tc_sum_us = tc_sum_us;
    state.last_tc_max_us = tc_max_us;

    let pilot_gain = runtime.downlink.pilot.gain;
    let sync_gain = runtime.downlink.sync.gain;
    let paging_gain = runtime.downlink.paging.gain;
    let tc_gain_sum: f32 = state.scratch_tc_blocks.iter().map(|(g, _)| *g).sum();
    let inv_gain_sum = 1.0 / (pilot_gain + sync_gain + paging_gain + tc_gain_sum);

    let pilot_block = &state.scratch_pilot;
    let sync_block = &state.scratch_sync;
    let paging_block = &state.scratch_paging;
    let tc_blocks = &state.scratch_tc_blocks;

    let t0 = Instant::now();
    for x in 0..block_size {
        let mut re = pilot_block[x].re * pilot_gain
            + sync_block[x].re * sync_gain
            + paging_block[x].re * paging_gain;
        let mut im = pilot_block[x].im * pilot_gain
            + sync_block[x].im * sync_gain
            + paging_block[x].im * paging_gain;

        for (tc_gain, tc_samples) in tc_blocks {
            re += tc_samples[x].re * tc_gain;
            im += tc_samples[x].im * tc_gain;
        }

        let combined = Complex32::new(re * inv_gain_sum * tx_scale, im * inv_gain_sum * tx_scale);
        synth_block[x] = spreader.spread(&combined);
    }
    state.synth_spread_us += t0.elapsed().as_micros() as u64;

    state.synth_time_sum_us += synth_start.elapsed().as_micros() as u64;

    let gen_us = gen_start.elapsed().as_micros() as u64;
    state.gen_time_sum_us += gen_us;
    state.gen_time_max_us = state.gen_time_max_us.max(gen_us);
    state.synth_blocks += 1;
    Ok(())
}
