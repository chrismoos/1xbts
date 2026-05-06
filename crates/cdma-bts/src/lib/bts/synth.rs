use std::time::Instant;

use cdma_common::{error::Error, time};
use num::complex::Complex32;

use crate::{
    channels::Channel,
    phy::spread::{PnSequence, Spreader},
};

use super::{
    PagingWalshChannel, PilotWalshChannel, SyncWalshChannel, TrafficChannelPool, TxLoopState,
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

/// Align any unstarted traffic channels to the next frame boundary, mark
/// `frame_align_verified` on first use, and return a snapshot of active
/// channel handles with their gains.  The pool lock is held only for this
/// brief metadata pass; `next_block()` is called by the caller after the
/// lock has been released.
fn snapshot_traffic_channels(
    traffic_channels: &TrafficChannelPool,
    chip_cursor: u64,
    pilot_offset_chips: u64,
) -> Vec<(f32, TrafficChannelWrapper)> {
    let mut tc_pool = traffic_channels.lock();
    for slot in tc_pool.iter_mut() {
        if !slot.lc_aligned {
            let offset = (chip_cursor - pilot_offset_chips) % SR1_CHIPS_PER_FRAME;
            let start_chip = if offset == 0 {
                chip_cursor
            } else {
                chip_cursor + (SR1_CHIPS_PER_FRAME - offset)
            };
            log::info!(
                "bts_tx: aligning traffic channel walsh={} start_chip={} chip_cursor={}",
                slot.walsh_code,
                start_chip,
                chip_cursor,
            );
            slot.channel.advance_lc_to_chip(start_chip);
            slot.start_chip = Some(start_chip);
            slot.lc_aligned = true;
        }
    }
    tc_pool
        .iter_mut()
        .filter_map(|slot| {
            let start = slot.start_chip?;
            if chip_cursor < start {
                return None;
            }
            if !slot.frame_align_verified {
                assert_eq!(
                    chip_cursor,
                    start,
                    "traffic channel walsh={} missed frame boundary: \
                     chip_cursor={} start_chip={} overshoot={}",
                    slot.walsh_code,
                    chip_cursor,
                    start,
                    chip_cursor - start,
                );
                slot.frame_align_verified = true;
            }
            Some((slot.gain, slot.channel.clone()))
        })
        .collect()
}

pub(super) fn synthesize_block(
    runtime: &BtsRuntimeSettings,
    traffic_channels: &TrafficChannelPool,
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
    let pilot_block = pch.next_block(block_size, frame_system_time);
    state.synth_pilot_us += t0.elapsed().as_micros() as u64;

    let t0 = Instant::now();
    let sync_block = fsch.next_block(block_size, frame_system_time);
    state.synth_fsch_us += t0.elapsed().as_micros() as u64;

    let t0 = Instant::now();
    let paging_block = fpch.next_block(block_size, frame_system_time);
    state.synth_fpch_us += t0.elapsed().as_micros() as u64;

    let t0 = Instant::now();
    let tc_snapshots =
        snapshot_traffic_channels(traffic_channels, chip_cursor, state.pilot_offset_chips);
    let tc_blocks: Vec<(f32, Vec<Complex32>)> = tc_snapshots
        .iter()
        .map(|(gain, ch)| (*gain, ch.next_block(block_size, frame_system_time)))
        .collect();
    state.synth_ftch_us += t0.elapsed().as_micros() as u64;

    let pilot_gain = runtime.downlink.pilot.gain;
    let sync_gain = runtime.downlink.sync.gain;
    let paging_gain = runtime.downlink.paging.gain;
    let tc_gain_sum: f32 = tc_blocks.iter().map(|(g, _)| *g).sum();
    let inv_gain_sum = 1.0 / (pilot_gain + sync_gain + paging_gain + tc_gain_sum);

    let t0 = Instant::now();
    for x in 0..block_size {
        let mut re = pilot_block[x].re * pilot_gain
            + sync_block[x].re * sync_gain
            + paging_block[x].re * paging_gain;
        let mut im = pilot_block[x].im * pilot_gain
            + sync_block[x].im * sync_gain
            + paging_block[x].im * paging_gain;

        for (tc_gain, tc_samples) in &tc_blocks {
            re += tc_samples[x].re * tc_gain;
            im += tc_samples[x].im * tc_gain;
        }

        let combined = Complex32::new(
            re * inv_gain_sum * runtime.tx_digital_backoff,
            im * inv_gain_sum * runtime.tx_digital_backoff,
        );
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
