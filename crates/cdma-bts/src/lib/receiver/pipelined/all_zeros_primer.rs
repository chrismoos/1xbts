use super::{PipelineProcessor, SampleBlock};

/// Gate processor that suppresses output until an all-zero block is seen.
///
/// This mirrors the legacy sync decode behavior that waits for an all-zero
/// deinterleaver block before enabling downstream Viterbi decode.
pub struct AllZerosPrimer {
    primed: bool,
}

impl AllZerosPrimer {
    pub fn new() -> Self {
        Self { primed: false }
    }
}

impl PipelineProcessor for AllZerosPrimer {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.primed {
            return vec![block];
        }

        let all_zeros = block.samples.iter().all(|s| s.re as u8 == 0);
        if all_zeros {
            self.primed = true;
            vec![block]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::AllZerosPrimer;
    use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

    #[test]
    fn test_all_zeros_primer_blocks_until_zero_block() {
        let mut p = AllZerosPrimer::new();

        let nonzero = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 8], 0);
        assert!(p.process_block(nonzero).is_empty());

        let zeros = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 8], 8);
        let out = p.process_block(zeros);
        assert_eq!(1, out.len());
        assert_eq!(8, out[0].len());

        let next = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 4], 16);
        let out2 = p.process_block(next);
        assert_eq!(1, out2.len());
        assert_eq!(4, out2[0].len());
    }
}
