use parking_lot::Mutex;
use std::collections::VecDeque;

use cdma_common::time::CdmaSystemTime;
use log::trace;
use num::complex::Complex32;

use crate::{
    mac::types::DataRequest,
    phy::coding::{
        block_interleaver::BitReversalInterleaver, convolutional::Encoder,
        symbol_repeat::SymbolRepetition,
    },
};

use super::Channel;

/// Sync channel encoder configuration.
pub struct Config<const EK: usize, const ER: usize> {
    pub data_rate: usize,
    pub encoder: Encoder<EK, ER>,
    pub symbol_repeat: SymbolRepetition,
    pub interleaver: BitReversalInterleaver,
    pub pn_pilot_offset: usize,
}
/// Forward Sync Channel encoder state.
pub struct ForwardSyncChannel<const EK: usize, const ER: usize> {
    config: Mutex<Config<EK, ER>>,
    fragments: Mutex<VecDeque<DataRequest>>,
}

impl<const EK: usize, const ER: usize> ForwardSyncChannel<EK, ER> {
    /// Create a new forward sync-channel encoder.
    pub fn new(config: Config<EK, ER>) -> ForwardSyncChannel<EK, ER> {
        ForwardSyncChannel {
            fragments: Mutex::new(VecDeque::new()),
            config: Mutex::new(config),
        }
    }

    /// Queue a fragment for sync-channel transmission.
    pub fn send_fragment(&self, fragment: DataRequest) {
        self.fragments.lock().push_back(fragment);
    }

    /**
       27 3.1.3.3.3 Sync Channel Convolutional Encoding
       28 The Sync Channel data shall be convolutionally encoded prior to transmission, as specified
       29 in 3.1.3.1.5. The state of the Sync Channel convolutional encoder shall not be reset
       30 between Sync Channel frames.
    */
    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut config = self.config.lock();

        let mut block = Vec::new();

        let mut logged = vec![];

        // loop through bits until we have enough for a block
        while block.len() < config.interleaver.block_len() {
            let mut bit = 0;

            let mut fragments = self.fragments.lock();
            if let Some(fragment) = fragments.front_mut() {
                let _ = current_system_time;
                // log first
                if fragment.size == fragment.data.len() {
                    trace!(
                        "F-sync fragment sending {:?}",
                        fragment
                            .data
                            .bits()
                            .iter()
                            .map(|s| format!("{}", s))
                            .collect::<Vec<_>>()
                            .join("")
                    );
                }
                if let Some(next) = fragment.data.take_next() {
                    bit = next;
                }
                // finished, remove
                if fragment.data.len() == 0 {
                    trace!("F-sync fragment of size {} sent fully", fragment.size,);
                    let _ = fragments.pop_front();
                }
            }

            logged.push(bit);

            config.encoder.encode(bit).iter().for_each(|b| {
                config.symbol_repeat.feed(*b);
            });

            let repeated = config.symbol_repeat.take_all();
            block.extend(repeated);
        }

        trace!("SYNC BLOCK, total {}", logged.len());
        for chunk in logged.chunks_exact(32) {
            trace!(
                "{}",
                chunk
                    .iter()
                    .map(|n| format!("{}", *n))
                    .collect::<Vec<_>>()
                    .join("")
            );
        }

        trace!(
            "block: {}",
            block
                .iter()
                .map(|s| format!("{}", s))
                .collect::<Vec<_>>()
                .join("")
        );

        config
            .interleaver
            .encode(&block)
            .into_iter()
            .map(|b| Complex32::new(if b == 0 { 1.0 } else { -1.0 }, 0.0))
            .collect::<Vec<_>>()
    }
}

impl<const EK: usize, const ER: usize> Channel for ForwardSyncChannel<EK, ER> {
    //#[tracing::instrument(skip(self))]
    fn next_block(
        &self,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) -> Vec<num::complex::Complex32> {
        let mut output = Vec::with_capacity(num_samples);
        self.next_block_into(&mut output, num_samples, system_time);
        output
    }

    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) {
        let start = out.len();
        while out.len() - start < num_samples {
            out.extend(self.next(system_time));
        }
    }
}
