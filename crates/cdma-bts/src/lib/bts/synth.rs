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

/// Per-channel amplitudes for the active traffic channels. Active channels
/// split the allotment evenly (capped per channel), and forward power
/// control scales each around its share. If boosts overflow the allotment,
/// only the boosted channels are scaled back — unboosted channels and the
/// overhead channels are untouched.
pub(super) fn traffic_amplitudes(
    traffic_fraction: f32,
    max_channel_fraction: f32,
    blocks: &[(f32, bool, Vec<Complex32>)],
    out: &mut Vec<f32>,
) {
    out.clear();
    let active = blocks.iter().filter(|(_, active, _)| *active).count();
    if active == 0 || traffic_fraction <= 0.0 {
        out.resize(blocks.len(), 0.0);
        return;
    }
    let share = (traffic_fraction / active as f32).min(max_channel_fraction);
    let mut total = 0.0f32;
    let mut boosted_total = 0.0f32;
    for (weight, active, _) in blocks {
        let power = if *active {
            share * weight * weight
        } else {
            0.0
        };
        total += power;
        if *weight > 1.0 {
            boosted_total += power;
        }
        out.push(power);
    }
    let boost_squeeze = if total > traffic_fraction && boosted_total > 0.0 {
        (traffic_fraction - (total - boosted_total)) / boosted_total
    } else {
        1.0
    };
    for (power, (weight, _, _)) in out.iter_mut().zip(blocks) {
        if *weight > 1.0 {
            *power *= boost_squeeze;
        }
        *power = power.sqrt();
    }
}

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
        state.scratch_tc_blocks.push((0.0, false, Vec::new()));
    }
    state
        .scratch_tc_blocks
        .truncate(state.scratch_tc_snapshot.len());
    let mut tc_sum_us = 0u64;
    let mut tc_max_us = 0u64;
    for (i, (gain, ch)) in state.scratch_tc_snapshot.iter().enumerate() {
        let (g_slot, active_slot, buf) = &mut state.scratch_tc_blocks[i];
        *g_slot = *gain;
        buf.clear();
        let tc_start = Instant::now();
        *active_slot = ch.next_block_into_with_activity(buf, block_size, frame_system_time);
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

    // Channel chips are unit amplitude and the Walsh covers are orthogonal,
    // so a channel's power is its amplitude squared and the fractions add.
    let pilot_amp = runtime.downlink.pilot.power_fraction.sqrt() * tx_scale;
    let sync_amp = runtime.downlink.sync.power_fraction.sqrt() * tx_scale;
    let paging_amp = runtime.downlink.paging.power_fraction.sqrt() * tx_scale;
    traffic_amplitudes(
        runtime.downlink.traffic.power_fraction,
        runtime.downlink.traffic.max_channel_power_fraction,
        &state.scratch_tc_blocks,
        &mut state.scratch_tc_amps,
    );
    for amp in state.scratch_tc_amps.iter_mut() {
        *amp *= tx_scale;
    }

    let pilot_block = &state.scratch_pilot;
    let sync_block = &state.scratch_sync;
    let paging_block = &state.scratch_paging;
    let tc_blocks = &state.scratch_tc_blocks;
    let tc_amps = &state.scratch_tc_amps;

    let t0 = Instant::now();
    for x in 0..block_size {
        let mut re = pilot_block[x].re * pilot_amp
            + sync_block[x].re * sync_amp
            + paging_block[x].re * paging_amp;
        let mut im = pilot_block[x].im * pilot_amp
            + sync_block[x].im * sync_amp
            + paging_block[x].im * paging_amp;

        for ((_, active, tc_samples), amp) in tc_blocks.iter().zip(tc_amps) {
            if !active {
                continue;
            }
            re += tc_samples[x].re * amp;
            im += tc_samples[x].im * amp;
        }

        synth_block[x] = spreader.spread(&Complex32::new(re, im));
    }
    state.synth_spread_us += t0.elapsed().as_micros() as u64;

    state.synth_time_sum_us += synth_start.elapsed().as_micros() as u64;

    let gen_us = gen_start.elapsed().as_micros() as u64;
    state.gen_time_sum_us += gen_us;
    state.gen_time_max_us = state.gen_time_max_us.max(gen_us);
    state.synth_blocks += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(weight: f32, active: bool) -> (f32, bool, Vec<Complex32>) {
        (weight, active, vec![Complex32::new(1.0, 0.0)])
    }

    fn powers(traffic: f32, cap: f32, blocks: &[(f32, bool, Vec<Complex32>)]) -> Vec<f32> {
        let mut amps = Vec::new();
        traffic_amplitudes(traffic, cap, blocks, &mut amps);
        amps.iter().map(|a| a * a).collect()
    }

    #[test]
    fn single_channel_is_capped_at_the_per_channel_limit() {
        let p = powers(0.5647, 0.2, &[block(1.0, true)]);
        assert!((p[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn active_channels_split_the_allotment_evenly() {
        let blocks = [
            block(1.0, true),
            block(1.0, true),
            block(1.0, true),
            block(1.0, true),
        ];
        let p = powers(0.5647, 0.2, &blocks);
        for v in &p {
            assert!((v - 0.5647 / 4.0).abs() < 1e-6);
        }
    }

    #[test]
    fn dtx_channel_does_not_consume_the_allotment() {
        let p = powers(0.5647, 0.2, &[block(1.0, true), block(1.998, false)]);
        assert!((p[0] - 0.2).abs() < 1e-6);
        assert_eq!(p[1], 0.0);
    }

    #[test]
    fn power_control_weight_moves_a_channel_relative_to_its_share() {
        let blocks = [
            block(1.0, true),
            block(0.5, true),
            block(1.0, true),
            block(1.0, true),
        ];
        let p = powers(0.5647, 0.2, &blocks);
        let share = 0.5647 / 4.0;
        assert!((p[0] - share).abs() < 1e-6);
        assert!((p[1] - share * 0.25).abs() < 1e-6);
    }

    #[test]
    fn boosted_channels_are_squeezed_back_into_the_allotment() {
        let blocks = [
            block(2.0, true),
            block(2.0, true),
            block(2.0, true),
            block(2.0, true),
        ];
        let p = powers(0.5647, 0.2, &blocks);
        let total: f32 = p.iter().sum();
        assert!((total - 0.5647).abs() < 1e-5);
        assert!((p[0] - 0.5647 / 4.0).abs() < 1e-6);
    }

    #[test]
    fn overflowing_boost_does_not_shrink_unboosted_channels() {
        let blocks = [
            block(1.0, true),
            block(1.0, true),
            block(1.0, true),
            block(2.0, true),
        ];
        let p = powers(0.5647, 0.2, &blocks);
        let share = 0.5647 / 4.0;
        for v in &p[..3] {
            assert!((v - share).abs() < 1e-6);
        }
        // The boosted channel gets only what the allotment has left.
        assert!((p[3] - share).abs() < 1e-6);
        let total: f32 = p.iter().sum();
        assert!(total <= 0.5647 + 1e-5);
    }

    #[test]
    fn no_active_channels_yields_zero_amplitudes() {
        let p = powers(0.5647, 0.2, &[block(1.0, false)]);
        assert_eq!(p, vec![0.0]);
        assert!(powers(0.5647, 0.2, &[]).is_empty());
    }
}
