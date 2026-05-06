//! Walsh code generation, despreading, and transform helpers.

use num::complex::Complex32;

/// Despreads a Walsh-coded symbol using one Walsh row.
pub struct WalshDecoder {
    /// The selected Walsh row as `+1/-1` chips.
    code: Vec<i8>,
}

impl WalshDecoder {
    /// Create a decoder for Walsh row `row` from a square matrix of size `SIZE`.
    pub fn new<const SIZE: usize>(row: usize) -> WalshDecoder {
        assert!(SIZE & (SIZE - 1) == 0);
        assert!(row < SIZE);
        WalshDecoder {
            code: WalshGenerator::generate_matrix::<SIZE>()[row].to_vec(),
        }
    }

    /// Correlate one Walsh-length symbol and return the normalized despread value.
    pub fn process_symbol(&self, block: &[Complex32]) -> Complex32 {
        assert_eq!(block.len(), self.code.len());

        let mut acc = Complex32::new(0.0, 0.0);

        for (sample, &chip) in block.iter().zip(self.code.iter()) {
            if chip > 0 {
                acc += *sample;
            } else {
                acc -= *sample;
            }
        }

        let scale = 1.0 / (block.len() as f32);
        acc * scale
    }

    /// Despread a block and return the single resulting symbol.
    pub fn process(&self, block: &[Complex32]) -> Vec<Complex32> {
        vec![self.process_symbol(block)]
    }
}

/// Generates Walsh-coded chip streams from complex input symbols.
pub struct WalshGenerator {
    /// The selected Walsh row as `+1/-1` chips.
    code: Vec<i8>,
    repetition: usize,
}

impl WalshGenerator {
    /// Create a generator for Walsh row `row` from a matrix of size `SIZE`.
    ///
    /// `repetition` repeats each emitted Walsh codeword this many times.
    pub fn new<const SIZE: usize>(row: usize, repetition: usize) -> WalshGenerator {
        assert!(SIZE & (SIZE - 1) == 0);
        assert!(row < SIZE);
        WalshGenerator {
            code: Self::generate_matrix::<SIZE>()[row].to_vec(),
            repetition,
        }
    }

    /// Spread one complex symbol into Walsh chips.
    pub fn feed(&self, sample: Complex32) -> Vec<Complex32> {
        (0..self.repetition)
            .flat_map(|_| {
                self.code
                    .iter()
                    .map(|c| Complex32::new(*c as f32 * sample.re, *c as f32 * sample.im))
            })
            .collect::<Vec<_>>()
    }

    /// Spread a slice of complex symbols into a contiguous Walsh chip stream.
    pub fn feed_many(&self, bits: &[Complex32]) -> Vec<Complex32> {
        bits.iter().flat_map(|b| self.feed(*b)).collect()
    }

    /// In-place Fast Walsh-Hadamard Transform on a power-of-two slice of
    /// Complex32 values.  After the transform, `values[k]` contains the
    /// Walsh-row-k correlation (unnormalised).  O(N log N) vs O(N²) for
    /// the brute-force matrix multiply.
    pub fn fwht(values: &mut [Complex32]) {
        let n = values.len();
        debug_assert!(
            n > 0 && n & (n - 1) == 0,
            "FWHT requires power-of-two length"
        );
        let mut span = 1usize;
        while span < n {
            let step = span * 2;
            for base in (0..n).step_by(step) {
                for idx in 0..span {
                    let a = values[base + idx];
                    let b = values[base + idx + span];
                    values[base + idx] = a + b;
                    values[base + idx + span] = a - b;
                }
            }
            span <<= 1;
        }
    }

    /// In-place FWHT on a fixed-size array (avoids slice overhead for hot paths).
    pub fn fwht_fixed<const N: usize>(values: &mut [Complex32; N]) {
        debug_assert!(N > 0 && N & (N - 1) == 0);
        let mut span = 1usize;
        while span < N {
            let step = span * 2;
            for base in (0..N).step_by(step) {
                for idx in 0..span {
                    let a = values[base + idx];
                    let b = values[base + idx + span];
                    values[base + idx] = a + b;
                    values[base + idx + span] = a - b;
                }
            }
            span <<= 1;
        }
    }

    /// Generate the `L x L` Walsh-Hadamard matrix as `+1/-1` chips.
    pub fn generate_matrix<const L: usize>() -> [[i8; L]; L] {
        assert!(L & (L - 1) == 0);
        let mut matrix = [[1i8; L]; L];
        let mut x = 1;
        while x < L {
            // loop through a quarter
            for i in 0..x {
                for j in 0..x {
                    matrix[i][x + j] = matrix[i][j];
                    matrix[x + i][j] = matrix[i][j];
                    matrix[x + i][x + j] = -matrix[i][j];
                }
            }
            x += x;
        }
        matrix
    }
}

#[cfg(test)]
mod tests {
    use num::complex::Complex32;

    use super::WalshGenerator;

    #[test]
    pub fn test_walsh_generate() {
        let walsh_generator = WalshGenerator::new::<64>(0, 1);
        assert_eq!(
            &(0..64)
                .map(|_| Complex32::new(1.0, 0.0))
                .collect::<Vec<Complex32>>(),
            walsh_generator
                .feed_many(&[Complex32::new(1.0, 0.0)])
                .as_slice()
        );
    }

    #[test]
    pub fn test_walsh_generate_tables() {
        let wn1 = WalshGenerator::generate_matrix::<1>();
        assert_eq!([[1]], wn1);

        let wn2 = WalshGenerator::generate_matrix::<2>();
        assert_eq!([[1, 1], [1, -1]], wn2);

        let wn4 = WalshGenerator::generate_matrix::<4>();
        assert_eq!(
            [[1, 1, 1, 1], [1, -1, 1, -1], [1, 1, -1, -1], [1, -1, -1, 1]],
            wn4
        );

        let wn8 = WalshGenerator::generate_matrix::<8>();
        assert_eq!(
            [
                [1, 1, 1, 1, 1, 1, 1, 1],
                [1, -1, 1, -1, 1, -1, 1, -1],
                [1, 1, -1, -1, 1, 1, -1, -1],
                [1, -1, -1, 1, 1, -1, -1, 1],
                [1, 1, 1, 1, -1, -1, -1, -1],
                [1, -1, 1, -1, -1, 1, -1, 1],
                [1, 1, -1, -1, -1, -1, 1, 1],
                [1, -1, -1, 1, -1, 1, 1, -1]
            ],
            wn8
        );
    }
}
