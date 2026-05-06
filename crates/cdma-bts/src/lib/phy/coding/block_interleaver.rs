pub struct BitReversalInterleaver {
    params: InterleaverParams,
}

impl BitReversalInterleaver {
    pub fn new(params: InterleaverParams) -> BitReversalInterleaver {
        BitReversalInterleaver { params }
    }

    pub fn block_len(&self) -> usize {
        self.params.block_size
    }

    pub fn encode(&mut self, block: &[u8]) -> Vec<u8> {
        assert_eq!(self.params.block_size, block.len());

        let mut output = vec![];
        for i in 0..self.params.block_size {
            let index = 2u32.pow(self.params.m as u32) as usize * (i % self.params.j)
                + bro(self.params.m, i / self.params.j);
            //debug!("index: {}", index);
            output.push(block[index]);
        }
        output
    }

    pub fn decode(&mut self, block: &[u8]) -> Vec<u8> {
        assert_eq!(self.params.block_size, block.len());

        let mut output = vec![0u8; self.params.block_size];
        for i in 0..self.params.block_size {
            let index = (2u32.pow(self.params.m as u32) as usize * (i % self.params.j))
                + bro(self.params.m, i / self.params.j);
            //output.push(block[index]);
            output[index] = block[i];
        }
        output
    }

    /// Deinterleave soft (f32) values — same permutation as `decode` but
    /// preserves continuous values for soft-decision decoding.
    pub fn decode_soft(&self, block: &[f32]) -> Vec<f32> {
        assert_eq!(self.params.block_size, block.len());

        let mut output = vec![0.0f32; self.params.block_size];
        for i in 0..self.params.block_size {
            let index = (2u32.pow(self.params.m as u32) as usize * (i % self.params.j))
                + bro(self.params.m, i / self.params.j);
            output[index] = block[i];
        }
        output
    }
}

pub struct ForwardBackwardsBitReversalInterleaver {
    params: InterleaverParams,
}

impl ForwardBackwardsBitReversalInterleaver {
    pub fn new(params: InterleaverParams) -> ForwardBackwardsBitReversalInterleaver {
        ForwardBackwardsBitReversalInterleaver { params }
    }

    pub fn encode(&mut self, block: &[u8]) -> Vec<u8> {
        assert_eq!(self.params.block_size, block.len());

        let mut output = vec![];
        for i in 0..self.params.block_size {
            if i % 2 == 0 {
                let index = (2u32.pow(self.params.m as u32) as usize * ((i / 2) % self.params.j))
                    + bro(self.params.m, (i / 2) / self.params.j);
                output.push(block[index]);
            } else {
                let index = (2u32.pow(self.params.m as u32) as usize
                    * ((self.params.block_size - ((i + 1) / 2)) % self.params.j))
                    + bro(
                        self.params.m,
                        (self.params.block_size - ((i + 1) / 2)) / self.params.j,
                    );
                output.push(block[index]);
            }
        }
        output
    }

    pub fn decode(&self, block: &[u8]) -> Vec<u8> {
        assert_eq!(self.params.block_size, block.len());

        let mut output = vec![0u8; self.params.block_size];
        for i in 0..self.params.block_size {
            let index = if i % 2 == 0 {
                (2u32.pow(self.params.m as u32) as usize * ((i / 2) % self.params.j))
                    + bro(self.params.m, (i / 2) / self.params.j)
            } else {
                (2u32.pow(self.params.m as u32) as usize
                    * ((self.params.block_size - ((i + 1) / 2)) % self.params.j))
                    + bro(
                        self.params.m,
                        (self.params.block_size - ((i + 1) / 2)) / self.params.j,
                    )
            };
            output[index] = block[i];
        }
        output
    }

    pub fn decode_soft(&self, block: &[f32]) -> Vec<f32> {
        assert_eq!(self.params.block_size, block.len());

        let mut output = vec![0.0f32; self.params.block_size];
        for i in 0..self.params.block_size {
            let index = if i % 2 == 0 {
                (2u32.pow(self.params.m as u32) as usize * ((i / 2) % self.params.j))
                    + bro(self.params.m, (i / 2) / self.params.j)
            } else {
                (2u32.pow(self.params.m as u32) as usize
                    * ((self.params.block_size - ((i + 1) / 2)) % self.params.j))
                    + bro(
                        self.params.m,
                        (self.params.block_size - ((i + 1) / 2)) / self.params.j,
                    )
            };
            output[index] = block[i];
        }
        output
    }
}

fn bro(m: usize, val: usize) -> usize {
    ((val as u32).reverse_bits() >> (32 - m)) as usize
}

#[derive(Clone, Copy)]
pub struct InterleaverParams {
    pub block_size: usize,
    pub m: usize,
    pub j: usize,
}

pub const SR1_PARAMS_48: InterleaverParams = InterleaverParams {
    block_size: 48,
    m: 4,
    j: 3,
};

pub const SR1_PARAMS_96: InterleaverParams = InterleaverParams {
    block_size: 96,
    m: 5,
    j: 3,
};

pub const SR1_PARAMS_128: InterleaverParams = InterleaverParams {
    block_size: 128,
    m: 7,
    j: 1,
};

pub const SR1_PARAMS_192: InterleaverParams = InterleaverParams {
    block_size: 192,
    m: 6,
    j: 3,
};

pub const SR1_PARAMS_384: InterleaverParams = InterleaverParams {
    block_size: 384,
    m: 6,
    j: 6,
};

pub const SR1_PARAMS_576: InterleaverParams = InterleaverParams {
    block_size: 576,
    m: 5,
    j: 18,
};

pub const SR1_PARAMS_768: InterleaverParams = InterleaverParams {
    block_size: 768,
    m: 6,
    j: 12,
};

/// Per C.S0002-E Table 2.1.3.1.8-1: block size 1536 → m=6, J=24.
pub const SR1_PARAMS_1536: InterleaverParams = InterleaverParams {
    block_size: 1536,
    m: 6,
    j: 24,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rc12ReverseTrafficRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl Rc12ReverseTrafficRate {
    pub const fn repetition_factor(self) -> usize {
        match self {
            Self::Full => 1,
            Self::Half => 2,
            Self::Quarter => 4,
            Self::Eighth => 8,
        }
    }

    fn row_order(self) -> &'static [usize; 32] {
        match self {
            Self::Full => &[
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                23, 24, 25, 26, 27, 28, 29, 30, 31,
            ],
            Self::Half => &[
                0, 2, 1, 3, 4, 6, 5, 7, 8, 10, 9, 11, 12, 14, 13, 15, 16, 18, 17, 19, 20, 22, 21,
                23, 24, 26, 25, 27, 28, 30, 29, 31,
            ],
            Self::Quarter => &[
                0, 4, 1, 5, 2, 6, 3, 7, 8, 12, 9, 13, 10, 14, 11, 15, 16, 20, 17, 21, 18, 22, 19,
                23, 24, 28, 25, 29, 26, 30, 27, 31,
            ],
            Self::Eighth => &[
                0, 8, 1, 9, 2, 10, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15, 16, 24, 17, 25, 18, 26, 19,
                27, 20, 28, 21, 29, 22, 30, 23, 31,
            ],
        }
    }
}

/// Reverse traffic interleaver for Radio Configurations 1 and 2.
///
/// The input block is written by columns into a 32x18 matrix and read out by
/// rows using the rate-specific row orders from C.S0002-E 2.1.3.1.8.1.
pub struct Rc12ReverseTrafficInterleaver {
    rate: Rc12ReverseTrafficRate,
}

impl Rc12ReverseTrafficInterleaver {
    const ROWS: usize = 32;
    const COLS: usize = 18;
    const BLOCK_LEN: usize = Self::ROWS * Self::COLS;

    pub fn new(rate: Rc12ReverseTrafficRate) -> Self {
        Self { rate }
    }

    pub const fn block_len(&self) -> usize {
        Self::BLOCK_LEN
    }

    fn output_index_to_input_index(&self, output_index: usize) -> usize {
        let row_pos = output_index / Self::COLS;
        let col = output_index % Self::COLS;
        let row = self.rate.row_order()[row_pos];
        row + col * Self::ROWS
    }

    pub fn encode(&self, block: &[u8]) -> Vec<u8> {
        assert_eq!(Self::BLOCK_LEN, block.len());
        let mut out = vec![0u8; Self::BLOCK_LEN];
        for (output_index, slot) in out.iter_mut().enumerate() {
            *slot = block[self.output_index_to_input_index(output_index)];
        }
        out
    }

    pub fn decode(&self, block: &[u8]) -> Vec<u8> {
        assert_eq!(Self::BLOCK_LEN, block.len());
        let mut out = vec![0u8; Self::BLOCK_LEN];
        for (output_index, &value) in block.iter().enumerate() {
            out[self.output_index_to_input_index(output_index)] = value;
        }
        out
    }

    pub fn decode_soft(&self, block: &[f32]) -> Vec<f32> {
        assert_eq!(Self::BLOCK_LEN, block.len());
        let mut out = vec![0.0f32; Self::BLOCK_LEN];
        for (output_index, &value) in block.iter().enumerate() {
            out[self.output_index_to_input_index(output_index)] = value;
        }
        out
    }
}

#[cfg(test)]
pub mod test {
    use crate::phy::coding::block_interleaver::{ForwardBackwardsBitReversalInterleaver, bro};

    use super::{
        BitReversalInterleaver, Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
        SR1_PARAMS_48, SR1_PARAMS_384, SR1_PARAMS_576,
    };

    #[test]
    pub fn test_bro() {
        assert_eq!(3, bro(3, 6));
    }

    // Expected permutation for the bit-reversal rule with SR1_PARAMS_48 (m=4, j=3).
    #[test]
    pub fn test_br_interleaver_48() {
        let input = (0..48u8).collect::<Vec<_>>();
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_48);
        let result = interleaver.encode(&input);
        assert_eq!(
            &[
                0, 16, 32, 8, 24, 40, 4, 20, 36, 12, 28, 44, 2, 18, 34, 10, 26, 42, 6, 22, 38, 14,
                30, 46, 1, 17, 33, 9, 25, 41, 5, 21, 37, 13, 29, 45, 3, 19, 35, 11, 27, 43, 7, 23,
                39, 15, 31, 47
            ],
            &result[..]
        );
    }

    // Expected permutation for the forward/backward bit-reversal rule with SR1_PARAMS_48.
    #[test]
    pub fn test_fbbr_interleaver_48() {
        let input = (0..48u8).collect::<Vec<_>>();
        let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_48);
        let result = interleaver.encode(&input);
        assert_eq!(
            &[
                0, 47, 16, 31, 32, 15, 8, 39, 24, 23, 40, 7, 4, 43, 20, 27, 36, 11, 12, 35, 28, 19,
                44, 3, 2, 45, 18, 29, 34, 13, 10, 37, 26, 21, 42, 5, 6, 41, 22, 25, 38, 9, 14, 33,
                30, 17, 46, 1
            ],
            &result[..]
        );
    }

    #[test]
    pub fn test_br_interleaver_384_parameters() {
        // Test that SR1_PARAMS_384 has correct parameters for 9600 bps paging channel
        assert_eq!(384, SR1_PARAMS_384.block_size);
        assert_eq!(6, SR1_PARAMS_384.m);
        assert_eq!(6, SR1_PARAMS_384.j);

        // Verify that m and j satisfy the constraint: 2^m * j = block_size
        assert_eq!(
            2_usize.pow(SR1_PARAMS_384.m as u32) * SR1_PARAMS_384.j,
            SR1_PARAMS_384.block_size
        );
    }

    #[test]
    pub fn test_br_interleaver_576_parameters() {
        // Access channel (SR1) uses 576-symbol interleaver.
        assert_eq!(576, SR1_PARAMS_576.block_size);
        assert_eq!(5, SR1_PARAMS_576.m);
        assert_eq!(18, SR1_PARAMS_576.j);
        assert_eq!(
            2_usize.pow(SR1_PARAMS_576.m as u32) * SR1_PARAMS_576.j,
            SR1_PARAMS_576.block_size
        );
    }

    #[test]
    pub fn test_br_interleaver_384_encode_decode() {
        let input = (0..384).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);

        // Test encoding
        let encoded = interleaver.encode(&input);
        assert_eq!(384, encoded.len());

        // Test decoding - should restore original order
        let decoded = interleaver.decode(&encoded);
        assert_eq!(input, decoded);
    }

    #[test]
    pub fn test_br_interleaver_384_pattern() {
        // Test specific interleaving pattern for first few elements
        let input = (0..384).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);
        let result = interleaver.encode(&input);

        // Verify first few interleaved positions based on the bit-reversal algorithm
        // For SR1_PARAMS_384: m=6, j=6, so 2^6 = 64
        // index = 64 * (i % 6) + bro(6, i / 6)

        // i=0: index = 64 * (0 % 6) + bro(6, 0 / 6) = 64 * 0 + bro(6, 0) = 0 + 0 = 0
        assert_eq!(0, result[0]);

        // i=1: index = 64 * (1 % 6) + bro(6, 1 / 6) = 64 * 1 + bro(6, 0) = 64 + 0 = 64
        assert_eq!(64, result[1]);

        // i=6: index = 64 * (6 % 6) + bro(6, 6 / 6) = 64 * 0 + bro(6, 1) = 0 + 32 = 32
        assert_eq!(32, result[6]);

        // Verify all elements are present (permutation of input)
        let mut sorted_result = result.clone();
        sorted_result.sort();
        let mut sorted_input = input.clone();
        sorted_input.sort();
        assert_eq!(sorted_input, sorted_result);
    }

    #[test]
    pub fn test_fbbr_interleaver_384() {
        let input = (0..384).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_384);
        let result = interleaver.encode(&input);

        assert_eq!(384, result.len());

        // Verify all elements are present (permutation of input)
        let mut sorted_result = result.clone();
        sorted_result.sort();
        let mut sorted_input = input.clone();
        sorted_input.sort();
        assert_eq!(sorted_input, sorted_result);

        // Test that forward-backwards produces different pattern than standard bit-reversal
        let mut standard_interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);
        let standard_result = standard_interleaver.encode(&input);
        assert_ne!(result, standard_result);
    }

    #[test]
    pub fn test_br_interleaver_384_round_trip() {
        // Test multiple round trips to ensure stability
        let input = (0..384).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);

        let encoded1 = interleaver.encode(&input);
        let decoded1 = interleaver.decode(&encoded1);
        assert_eq!(input, decoded1);

        let encoded2 = interleaver.encode(&decoded1);
        let decoded2 = interleaver.decode(&encoded2);
        assert_eq!(input, decoded2);
        assert_eq!(encoded1, encoded2);
    }

    #[test]
    pub fn test_rc12_reverse_traffic_interleaver_round_trip() {
        let input = (0..576).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        for rate in [
            Rc12ReverseTrafficRate::Full,
            Rc12ReverseTrafficRate::Half,
            Rc12ReverseTrafficRate::Quarter,
            Rc12ReverseTrafficRate::Eighth,
        ] {
            let interleaver = Rc12ReverseTrafficInterleaver::new(rate);
            let encoded = interleaver.encode(&input);
            let decoded = interleaver.decode(&encoded);
            assert_eq!(input, decoded, "round trip failed for rate {:?}", rate);
        }
    }
}
