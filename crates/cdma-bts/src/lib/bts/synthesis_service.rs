//! Thin service wrapper around the forward-link synthesis functions.

use std::time::Instant;

use cdma_common::{error::Error, time};
use num::complex::Complex32;

use crate::phy::spread::Spreader;

use super::{
    PagingWalshChannel, PilotWalshChannel, SyncWalshChannel, TxLoopState,
    settings::BtsRuntimeSettings,
};

/// Service boundary for forward-link waveform synthesis.
///
/// This is a zero-state facade that delegates to the free functions in
/// [`super::synth`]. It exists to establish a named service boundary so
/// that callers interact with synthesis through a single type rather than
/// bare module-level functions.
pub struct SynthesisService;

impl SynthesisService {
    /// Create a new synthesis service instance.
    pub fn new() -> Self {
        Self
    }

    /// Synthesize one block of forward-link IQ samples.
    ///
    /// Combines pilot, sync, paging, and traffic channel contributions,
    /// applies gain normalisation and PN spreading, then writes the result
    /// into `synth_block`. Delegates to [`super::synth::synthesize_block`].
    #[allow(dead_code)]
    pub(crate) fn synthesize_block(
        &self,
        runtime: &BtsRuntimeSettings,
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
        super::synth::synthesize_block(
            runtime,
            state,
            gen_start,
            pch,
            fsch,
            fpch,
            spreader,
            synth_block,
            block_size,
            frame_system_time,
            chip_cursor,
        )
    }

    /// Create a PN spreader aligned to the given chip cursor.
    ///
    /// Delegates to [`super::synth::aligned_spreader`].
    pub fn aligned_spreader(
        &self,
        pilot_offset: usize,
        short_code_length_chips: usize,
        chip_cursor: u64,
    ) -> Spreader {
        super::synth::aligned_spreader(pilot_offset, short_code_length_chips, chip_cursor)
    }
}
