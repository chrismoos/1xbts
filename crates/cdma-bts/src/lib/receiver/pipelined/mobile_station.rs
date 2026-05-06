use crate::phy::coding::long_code::LongCodeGenerator;
use log::debug;

use super::{
    PipelineProcessor, PipelineProcessorShared, SampleBlock, flush_sub_chain, run_sub_chain,
};

// ---------------------------------------------------------------------------
// LC state timing constants (per C.S0005 Sync Channel timing)
// ---------------------------------------------------------------------------

use cdma_common::consts::SR1_CHIPS_320MS;

// ---------------------------------------------------------------------------
// Sync info extracted from sync channel events
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct SyncInfo {
    pilot_pn: u16,
    lc_state: u64,
    prat: u8,
    /// Pipeline chip at which the paging chain should begin (chip-rate units).
    ///
    /// Computed as:
    ///   last_superframe_end_chip + 320ms_chips - pilot_pn_offset_chips
    ///
    /// Per spec, SYS_TIME corresponds to 320 ms past the end of the last
    /// superframe minus the pilot PN offset. So the LC_STATE is valid at
    /// exactly this pipeline chip position.
    paging_start_chip: usize,
}

// ---------------------------------------------------------------------------
// MobileStation orchestrator
// ---------------------------------------------------------------------------

/// State machine for the mobile station orchestrator.
#[derive(Debug, PartialEq, Eq)]
enum MsState {
    /// Waiting for first sync channel decode.
    Acquiring,
    /// Sync obtained, waiting for paging chain start chip.
    SyncLocked,
    /// Paging chain is running.
    PagingActive,
}

/// Mobile station orchestrator that manages sync and paging channel decoding.
///
/// Receives despread chip-rate samples (post-PN-despreading, post-decimation)
/// and internally forks them to:
///   - A **sync sub-chain**: Walsh 32 decode → unrepeater → deinterleaver →
///     Viterbi → SyncChannelProcessor
///   - A **paging sub-chain** (constructed lazily after first sync): Walsh 1
///     decode → unrepeater → LC descrambler → deinterleaver → Viterbi →
///     PagingChannelProcessor
///
/// The paging chain is constructed once the sync channel provides `lc_state`,
/// `pilot_pn`, and `sys_time`. The LC generator is seeded at construction
/// time; if upstream lock is lost, both chains are torn down and rebuilt.
pub struct MobileStation {
    sync_chain: Vec<PipelineProcessorShared>,
    paging_chain: Option<Vec<PipelineProcessorShared>>,
    paging_chain_builder: Box<dyn Fn(u16, u64, PagingRate) -> Vec<PipelineProcessorShared> + Send>,
    state: MsState,
    sync_info: Option<SyncInfo>,
    force_start_paging_on_sync_lock: bool,
    /// LC generator used purely for verification — seeded from first sync
    /// event's lc_state and advanced to cross-check subsequent sync events.
    lc_verify: Option<LongCodeGenerator>,
    /// paging_start_chip from the previous sync event (LC_STATE validity point).
    lc_verify_prev_chip: usize,
}

/// Paging channel data rate passed to the paging chain builder.
#[derive(Clone, Copy, Debug)]
pub enum PagingRate {
    Rate4800,
    Rate9600,
}

impl MobileStation {
    /// Create a new MobileStation.
    ///
    /// - `sync_chain`: the sync channel sub-chain (from chip-rate samples to
    ///   sync events). Must end with `SyncChannelProcessor`.
    /// - `paging_chain_builder`: closure that builds the paging sub-chain
    ///   given `(pilot_pn, lc_state, paging_rate)`. The builder receives an
    ///   already-seeded `LongCodeGenerator` state. The chain should start from
    ///   chip-rate samples (WalshPilotCombiner) and end with
    ///   `PagingChannelProcessor`.
    pub fn new(
        sync_chain: Vec<PipelineProcessorShared>,
        paging_chain_builder: Box<
            dyn Fn(u16, u64, PagingRate) -> Vec<PipelineProcessorShared> + Send,
        >,
    ) -> Self {
        Self {
            sync_chain,
            paging_chain: None,
            paging_chain_builder,
            state: MsState::Acquiring,
            sync_info: None,
            force_start_paging_on_sync_lock: false,
            lc_verify: None,
            lc_verify_prev_chip: 0,
        }
    }

    /// Start the paging chain immediately after sync lock instead of waiting
    /// for the sync-advertised paging start chip.
    pub fn with_force_start_paging_on_sync_lock(mut self, force: bool) -> Self {
        self.force_start_paging_on_sync_lock = force;
        self
    }

    fn handle_sync_event(&mut self, blk: &SampleBlock) {
        let pilot_pn = blk.tags.get("sync_pilot_pn").copied().unwrap_or(0) as u16;
        let lc_state = blk.tags.get("sync_lc_state").copied().unwrap_or(0) as u64;
        let sys_time = blk.tags.get("sync_sys_time").copied().unwrap_or(0) as u64;
        let prat = blk.tags.get("sync_prat").copied().unwrap_or(0) as u8;
        let last_superframe_end_chip = blk
            .tags
            .get("sync_last_superframe_end_chip")
            .copied()
            .unwrap_or(0) as usize;
        let superframe_start_chip =
            blk.tags.get("sync_som_start_chip").copied().unwrap_or(0) as usize;
        let sync_frame_count = blk.tags.get("sync_frame_count").copied().unwrap_or(0) as usize;

        let pilot_pn_offset_chips = (pilot_pn as usize) * 64;

        // Paging start: LC_STATE is valid 320ms - pilot_PN_offset past the
        // end of the last superframe containing the sync message.
        // SyncChannelProcessor provides last_superframe_end_chip in chip-rate
        // units, so this arithmetic stays in the same coordinate system.
        let paging_start_chip =
            last_superframe_end_chip + SR1_CHIPS_320MS as usize - pilot_pn_offset_chips;

        debug!(
            "mobile_station: sync event pilot_pn={} lc_state=0x{:x} sys_time={} prat={} \
             last_sf_end={}, first_sf_start={}, sync_frame_count={}, paging_start_chip={}",
            pilot_pn,
            lc_state,
            sys_time,
            prat,
            last_superframe_end_chip,
            superframe_start_chip,
            sync_frame_count,
            paging_start_chip
        );

        // --- LC state verification ---
        // LC_STATE is valid at paging_start_chip. Seed on first sync event;
        // advance by chip delta on subsequent ones to cross-check.
        if let Some(ref mut lc_gen) = self.lc_verify {
            let delta_chips = paging_start_chip.saturating_sub(self.lc_verify_prev_chip);
            lc_gen.advance_chips(delta_chips);
            let expected = lc_gen.state();
            if expected == lc_state {
                debug!(
                    "mobile_station: LC CHECK OK  expected=0x{:x} got=0x{:x} (delta_chips={})",
                    expected, lc_state, delta_chips
                );
            } else {
                debug!(
                    "mobile_station: LC CHECK FAIL expected=0x{:x} got=0x{:x} (delta_chips={})",
                    expected, lc_state, delta_chips
                );
            }
            self.lc_verify_prev_chip = paging_start_chip;
        } else {
            // First sync event — seed the verification generator.
            let mut lc_gen = LongCodeGenerator::new(0);
            lc_gen.set_state(lc_state);
            self.lc_verify = Some(lc_gen);
            self.lc_verify_prev_chip = paging_start_chip;
            debug!(
                "mobile_station: LC verify seeded with state=0x{:x} at paging_start_chip={}",
                lc_state, paging_start_chip
            );
        }

        // Only latch sync info on the first sync event; subsequent sync
        // messages would push the target forward indefinitely.
        if self.state == MsState::Acquiring {
            self.sync_info = Some(SyncInfo {
                pilot_pn,
                lc_state,
                prat,
                paging_start_chip,
            });
            self.state = MsState::SyncLocked;
        }
    }

    fn try_start_paging(&mut self, current_chip: usize) {
        let info = match &self.sync_info {
            Some(i) => i.clone(),
            None => return,
        };

        // Normal mode waits until the sync-advertised paging start chip.
        // Debug mode starts the paging chain immediately after sync lock,
        // which is useful for isolating startup/timing issues after PN align.
        if !self.force_start_paging_on_sync_lock && current_chip < info.paging_start_chip {
            if current_chip % 100_000 < 100 {
                debug!(
                    "mobile_station: waiting for paging start: current={} target={}",
                    current_chip, info.paging_start_chip
                );
            }
            return;
        }

        let paging_rate = match info.prat {
            0 => PagingRate::Rate9600,
            1 => PagingRate::Rate4800,
            _ => {
                println!(
                    "mobile_station: reserved PRAT={}, defaulting to 9600",
                    info.prat
                );
                PagingRate::Rate9600
            }
        };

        // Advance LC state from paging_start_chip to current_chip so the
        // descrambler is aligned with the first data it will actually receive.
        let lc_gap_chips = current_chip.saturating_sub(info.paging_start_chip);
        let mut lc_gen = LongCodeGenerator::new(0);
        lc_gen.set_state(info.lc_state);
        if lc_gap_chips > 0 {
            lc_gen.advance_chips(lc_gap_chips);
        }
        let advanced_lc_state = lc_gen.state();

        println!(
            "mobile_station: starting paging chain at chip {} (paging_start={} lc_gap={})",
            current_chip, info.paging_start_chip, lc_gap_chips
        );

        let chain = (self.paging_chain_builder)(info.pilot_pn, advanced_lc_state, paging_rate);
        self.paging_chain = Some(chain);
        self.state = MsState::PagingActive;
    }
}

impl PipelineProcessor for MobileStation {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let mut emitter = super::VecEmitter::new();
        let mut output = Vec::new();

        // 1. Always feed sync chain
        let sync_output = run_sub_chain(&mut self.sync_chain, block.clone(), &mut emitter);

        // 2. Check for sync events
        for blk in &sync_output {
            if blk.tags.get("ms_sync_event") == Some(&1) {
                self.handle_sync_event(blk);
            }
        }

        // Forward sync output
        output.extend(sync_output);

        // 3. Try to start paging if we're in SyncLocked
        if self.state == MsState::SyncLocked {
            self.try_start_paging(block.chip_start);
        }

        // 4. Feed paging chain if active — tag blocks with LC checkpoint
        //    so the LongCodeDescrambler can verify its own generator state.
        if let Some(ref mut paging_chain) = self.paging_chain {
            let mut paging_block = block;
            if let Some(ref info) = self.sync_info {
                paging_block
                    .tags
                    .insert("expected_lc_state", info.lc_state as i64);
                paging_block
                    .tags
                    .insert("expected_lc_chip", info.paging_start_chip as i64);
            }
            let paging_output = run_sub_chain(paging_chain, paging_block, &mut emitter);
            output.extend(paging_output);
        }

        output.extend(emitter.blocks);
        output
    }

    fn name(&self) -> &'static str {
        "MobileStation"
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        let mut emitter = super::VecEmitter::new();
        let mut out = flush_sub_chain(&mut self.sync_chain, &mut emitter);
        if let Some(ref mut paging_chain) = self.paging_chain {
            out.extend(flush_sub_chain(paging_chain, &mut emitter));
        }
        out.extend(emitter.blocks);
        out
    }
}
