use std::cell::RefCell;

/// Inline Hamming distance for small fixed-size arrays (N=2,3,4).
#[inline(always)]
fn branch_hamming<const N: usize>(a: &[u8; N], b: &[u8; N]) -> u32 {
    let mut dist = 0u32;
    let mut i = 0;
    while i < N {
        dist += (a[i] ^ b[i]) as u32;
        i += 1;
    }
    dist
}

#[derive(Clone, Copy)]
pub struct Encoder<const K: usize, const R: usize> {
    window: u32,
    generators: [u32; R],
}

impl<const K: usize, const R: usize> Encoder<K, R> {
    pub fn new(generators: [u32; R]) -> Encoder<K, R> {
        assert!(K > 0 && K <= 32);
        Encoder {
            window: 0,
            generators,
        }
    }

    pub fn encode(&mut self, value: u8) -> [u8; R] {
        self.window = (self.window >> 1) & ((1 << (K - 1)) - 1);
        self.window |= (value as u32) << (K - 1);
        let mut parity = [0u8; R];

        for x in 0..R {
            parity[x] = ((self.window & self.generators[x]).count_ones() & 1) as u8;
        }
        parity
    }

    pub fn state(&self) -> u32 {
        self.window
    }

    pub fn set_state(&mut self, state: u32) {
        self.window = state;
    }

    pub fn reset(&mut self) {
        self.window = 0;
    }
}

pub fn get_1_2_k9_encoder() -> Encoder<9, 2> {
    Encoder::new([0x1eb, 0x171])
}

/// C.S0002-E 3.1.3.1.5.1.3 (SR1/SR3 reverse-link channels): K=9, R=1/3.
/// Generator polynomials (octal): g0=557, g1=663, g2=711.
pub fn get_1_3_k9_encoder() -> Encoder<9, 3> {
    Encoder::new([0o557, 0o663, 0o711])
}

pub fn get_1_3_k9_viterbi_decoder() -> ViterbiDecoder<9, 3> {
    ViterbiDecoder::new(get_1_3_k9_encoder())
}

/// C.S0002-E 3.1.3.1.5.1.2 (RC3 forward/reverse channels): K=9, R=1/4.
/// Generator polynomials (octal): g0=765, g1=671, g2=513, g3=473.
pub fn get_1_4_k9_encoder() -> Encoder<9, 4> {
    Encoder::new([0o765, 0o671, 0o513, 0o473])
}

pub fn get_1_4_k9_viterbi_decoder() -> ViterbiDecoder<9, 4> {
    ViterbiDecoder::new(get_1_4_k9_encoder())
}

pub fn get_1_4_k9_soft_viterbi_decoder() -> SoftViterbiDecoder<9, 4> {
    SoftViterbiDecoder::new(get_1_4_k9_encoder())
}

fn encode_from_window<const N: usize>(window: u32, generators: &[u32; N]) -> [u8; N] {
    let mut parity = [0u8; N];
    for x in 0..N {
        parity[x] = ((window & generators[x]).count_ones() & 1) as u8;
    }
    parity
}

/// Shared traceback storage for Viterbi decoders.
///
/// Both hard-decision and soft-decision decoders use identical traceback
/// logic over the same data types (prev-state u16, input-bit u8).
struct TrellisTraceback {
    num_states: usize,
    traceback_depth: usize,
    steps: usize,
    trace_prev_states: Vec<u16>,
    trace_input_bits: Vec<u8>,
    scratch_prev_state: Vec<u16>,
    scratch_bit: Vec<u8>,
}

impl TrellisTraceback {
    fn new(num_states: usize, traceback_depth: usize) -> Self {
        Self {
            num_states,
            traceback_depth,
            steps: 0,
            trace_prev_states: Vec::new(),
            trace_input_bits: Vec::new(),
            scratch_prev_state: vec![0u16; num_states],
            scratch_bit: vec![0u8; num_states],
        }
    }

    fn traceback_bit(&self, end_state: usize, end_step: usize, depth: usize) -> u8 {
        let mut state = end_state;
        let mut oldest = 0u8;
        for i in 0..depth {
            let step = end_step - i;
            let idx = step * self.num_states + state;
            oldest = self.trace_input_bits[idx];
            state = self.trace_prev_states[idx] as usize;
        }
        oldest
    }

    fn traceback_all_from_state(&self, end_state: usize) -> Vec<u8> {
        if self.steps == 0 {
            return Vec::new();
        }

        let end_step = self.steps - 1;
        let mut out = Vec::with_capacity(self.steps);
        for depth in (1..=self.steps).rev() {
            out.push(self.traceback_bit(end_state, end_step, depth));
        }
        out
    }

    fn finish(&self, best_state: usize) -> Vec<u8> {
        if self.steps == 0 {
            return Vec::new();
        }

        let remaining = if self.steps >= self.traceback_depth {
            self.traceback_depth - 1
        } else {
            self.steps
        };
        if remaining == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(remaining);
        let end_step = self.steps - 1;
        for depth in (1..=remaining).rev() {
            out.push(self.traceback_bit(best_state, end_step, depth));
        }
        out
    }

    fn record_step(&mut self) {
        self.trace_prev_states
            .extend_from_slice(&self.scratch_prev_state);
        self.trace_input_bits.extend_from_slice(&self.scratch_bit);
        self.steps += 1;
    }

    fn trim_history(&mut self) {
        let max_history = self.traceback_depth * 2;
        if self.steps > max_history {
            let trim = self.steps - max_history;
            let trim_entries = trim * self.num_states;
            if trim_entries <= self.trace_prev_states.len() {
                self.trace_prev_states.drain(0..trim_entries);
                self.trace_input_bits.drain(0..trim_entries);
                self.steps -= trim;
            }
        }
    }

    fn reset(&mut self) {
        self.steps = 0;
        self.trace_prev_states.clear();
        self.trace_input_bits.clear();
    }

    fn prepare_block(&mut self, total_symbols: usize) {
        self.steps = 0;
        self.trace_prev_states.clear();
        self.trace_input_bits.clear();
        let total_entries = total_symbols * self.num_states;
        self.trace_prev_states.reserve(total_entries);
        self.trace_input_bits.reserve(total_entries);
    }
}

pub struct ViterbiDecoder<const K: usize, const N: usize> {
    num_states: usize,
    path_metrics: Vec<u32>,
    next_metrics: Vec<u32>,
    branch_next: Vec<[u16; 2]>,
    branch_out: Vec<[[u8; N]; 2]>,
    /// Per-step best-path cost (min metric before normalization).
    /// This is the instantaneous "error rate" — high values mean
    /// the input doesn't match valid codewords.
    last_step_cost: u32,
    tb: TrellisTraceback,
}

impl<const K: usize, const N: usize> ViterbiDecoder<K, N> {
    const INF_METRIC: u32 = u32::MAX / 4;

    pub fn new(encoder: Encoder<K, N>) -> ViterbiDecoder<K, N> {
        assert!(K >= 2 && K <= 16);
        let num_states = 1usize << (K - 1);
        let mut path_metrics = vec![Self::INF_METRIC; num_states];
        path_metrics[0] = 0;

        let mut branch_next = vec![[0u16; 2]; num_states];
        let mut branch_out = vec![[[0u8; N]; 2]; num_states];

        for state in 0..num_states {
            for input_bit in 0..=1u8 {
                let window = (state as u32) | ((input_bit as u32) << (K - 1));
                branch_out[state][input_bit as usize] =
                    encode_from_window(window, &encoder.generators);
                let next_state = ((state >> 1) | ((input_bit as usize) << (K - 2))) as u16;
                branch_next[state][input_bit as usize] = next_state;
            }
        }

        ViterbiDecoder {
            num_states,
            path_metrics,
            next_metrics: vec![Self::INF_METRIC; num_states],
            branch_next,
            branch_out,
            last_step_cost: 0,
            tb: TrellisTraceback::new(num_states, 5 * K),
        }
    }

    fn best_state(&self) -> usize {
        self.path_metrics
            .iter()
            .enumerate()
            .min_by_key(|(_, metric)| **metric)
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    /// Trace back the entire decoded bitstream from a known terminal state.
    pub fn traceback_all_from_state(&self, end_state: usize) -> Vec<u8> {
        self.tb.traceback_all_from_state(end_state)
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let best = self.best_state();
        self.tb.finish(best)
    }

    /// Decode a full terminated hard-decision block without incremental traceback
    /// or history trimming.
    pub fn decode_block_from_state(&mut self, inputs: &[[u8; N]], end_state: usize) -> Vec<u8> {
        self.path_metrics.fill(Self::INF_METRIC);
        self.path_metrics[0] = 0;
        self.next_metrics.fill(Self::INF_METRIC);
        self.tb.prepare_block(inputs.len());

        for input in inputs {
            self.next_metrics.fill(Self::INF_METRIC);
            self.tb.scratch_prev_state.fill(0);
            self.tb.scratch_bit.fill(0);

            for prev_state in 0..self.num_states {
                let prev_metric = self.path_metrics[prev_state];
                if prev_metric >= Self::INF_METRIC {
                    continue;
                }

                for input_bit in 0..=1usize {
                    let next_state = self.branch_next[prev_state][input_bit] as usize;
                    let bm = branch_hamming(&self.branch_out[prev_state][input_bit], input) as u32;
                    let cand = prev_metric.saturating_add(bm);
                    if cand < self.next_metrics[next_state] {
                        self.next_metrics[next_state] = cand;
                        self.tb.scratch_prev_state[next_state] = prev_state as u16;
                        self.tb.scratch_bit[next_state] = input_bit as u8;
                    }
                }
            }

            let best_next = self
                .next_metrics
                .iter()
                .copied()
                .min()
                .unwrap_or(Self::INF_METRIC);
            self.last_step_cost = best_next;
            for metric in &mut self.next_metrics {
                if *metric < Self::INF_METRIC {
                    *metric = metric.saturating_sub(best_next);
                }
            }

            std::mem::swap(&mut self.path_metrics, &mut self.next_metrics);
            self.tb.record_step();
        }

        self.tb.traceback_all_from_state(end_state)
    }

    /// Returns the per-step cost of the best trellis path (min metric
    /// before normalization at the last step). For correct polarity on
    /// a clean signal, this is 0. For inverted polarity it rises toward
    /// N (=2 for rate 1/2) because every branch is wrong.
    pub fn last_step_cost(&self) -> u32 {
        self.last_step_cost
    }

    /// Reset the decoder trellis to initial state (state 0 with metric 0).
    /// Call this when switching input polarity to let the decoder re-converge.
    pub fn reset_trellis(&mut self) {
        self.path_metrics.fill(Self::INF_METRIC);
        self.path_metrics[0] = 0;
        self.tb.reset();
    }

    pub fn process(&mut self, input: &[u8; N]) -> Option<u8> {
        assert!(
            input.iter().all(|b| *b <= 1),
            "input must be hard bits (0/1)"
        );

        self.next_metrics.fill(Self::INF_METRIC);
        self.tb.scratch_prev_state.fill(0);
        self.tb.scratch_bit.fill(0);

        for prev_state in 0..self.num_states {
            let prev_metric = self.path_metrics[prev_state];
            if prev_metric >= Self::INF_METRIC {
                continue;
            }

            for input_bit in 0..=1usize {
                let next_state = self.branch_next[prev_state][input_bit] as usize;
                let bm = branch_hamming(&self.branch_out[prev_state][input_bit], input);
                let cand = prev_metric + bm;
                if cand < self.next_metrics[next_state] {
                    self.next_metrics[next_state] = cand;
                    self.tb.scratch_prev_state[next_state] = prev_state as u16;
                    self.tb.scratch_bit[next_state] = input_bit as u8;
                }
            }
        }

        core::mem::swap(&mut self.path_metrics, &mut self.next_metrics);

        let min_metric = *self.path_metrics.iter().min().unwrap_or(&0);
        self.last_step_cost = min_metric;
        if min_metric > 0 {
            for m in self.path_metrics.iter_mut() {
                if *m < Self::INF_METRIC {
                    *m -= min_metric;
                }
            }
        }

        self.tb.record_step();
        self.tb.trim_history();

        if self.tb.steps >= self.tb.traceback_depth {
            let best = self.best_state();
            Some(
                self.tb
                    .traceback_bit(best, self.tb.steps - 1, self.tb.traceback_depth),
            )
        } else {
            None
        }
    }

    /// Process multiple symbols at once, returning all decoded bits.
    ///
    /// This is much faster than calling `process()` in a loop because:
    /// - ACS inner loop runs without traceback overhead per symbol
    /// - Traceback is done in bulk at the end
    /// - Trace trimming happens once instead of per-symbol
    pub fn process_batch(&mut self, symbols: &[[u8; N]]) -> Vec<u8> {
        let initial_steps = self.tb.steps;
        let tb_depth = self.tb.traceback_depth;

        for input in symbols {
            debug_assert!(input.iter().all(|b| *b <= 1));

            self.next_metrics.fill(Self::INF_METRIC);
            self.tb.scratch_prev_state.fill(0);
            self.tb.scratch_bit.fill(0);

            for prev_state in 0..self.num_states {
                let prev_metric = self.path_metrics[prev_state];
                if prev_metric >= Self::INF_METRIC {
                    continue;
                }

                for input_bit in 0..=1usize {
                    let next_state = self.branch_next[prev_state][input_bit] as usize;
                    let bm = branch_hamming(&self.branch_out[prev_state][input_bit], input);
                    let cand = prev_metric + bm;
                    if cand < self.next_metrics[next_state] {
                        self.next_metrics[next_state] = cand;
                        self.tb.scratch_prev_state[next_state] = prev_state as u16;
                        self.tb.scratch_bit[next_state] = input_bit as u8;
                    }
                }
            }

            core::mem::swap(&mut self.path_metrics, &mut self.next_metrics);

            let min_metric = *self.path_metrics.iter().min().unwrap_or(&0);
            self.last_step_cost = min_metric;
            if min_metric > 0 {
                for m in self.path_metrics.iter_mut() {
                    if *m < Self::INF_METRIC {
                        *m -= min_metric;
                    }
                }
            }

            self.tb.record_step();
        }

        let mut out = Vec::new();
        let first_emittable = if initial_steps >= tb_depth {
            initial_steps
        } else {
            tb_depth
        };
        if self.tb.steps > first_emittable {
            let start_step = if initial_steps >= tb_depth {
                initial_steps
            } else {
                tb_depth - 1
            };
            for step in start_step..self.tb.steps {
                let best = self.best_state();
                out.push(self.tb.traceback_bit(best, step, tb_depth));
            }
        }

        self.tb.trim_history();

        out
    }
}

pub struct SoftViterbiDecoder<const K: usize, const N: usize> {
    num_states: usize,
    path_metrics: Vec<f64>,
    next_metrics: Vec<f64>,
    branch_next: Vec<[u16; 2]>,
    /// Branch-metric table index. All states share only `1 << N` expected
    /// tuples between them.
    branch_pattern: Vec<[u8; 2]>,
    /// Squared error per expected tuple for the current symbol.
    pattern_metrics: Vec<f64>,
    tb: TrellisTraceback,
}

impl<const K: usize, const N: usize> SoftViterbiDecoder<K, N> {
    const INF_METRIC: f64 = f64::MAX / 4.0;

    pub fn new(encoder: Encoder<K, N>) -> SoftViterbiDecoder<K, N> {
        assert!(K >= 2 && K <= 16);
        let num_states = 1usize << (K - 1);
        let mut path_metrics = vec![Self::INF_METRIC; num_states];
        path_metrics[0] = 0.0;

        let mut branch_next = vec![[0u16; 2]; num_states];
        let mut branch_pattern = vec![[0u8; 2]; num_states];

        for state in 0..num_states {
            for input_bit in 0..=1u8 {
                let window = (state as u32) | ((input_bit as u32) << (K - 1));
                let hard = encode_from_window(window, &encoder.generators);
                let mut pattern = 0usize;
                for i in 0..N {
                    pattern |= ((hard[i] & 1) as usize) << i;
                }
                branch_pattern[state][input_bit as usize] = pattern as u8;
                let next_state = ((state >> 1) | ((input_bit as usize) << (K - 2))) as u16;
                branch_next[state][input_bit as usize] = next_state;
            }
        }

        assert!(N <= 8, "the branch-metric table indexes patterns in a u8");

        SoftViterbiDecoder {
            num_states,
            path_metrics,
            next_metrics: vec![Self::INF_METRIC; num_states],
            branch_next,
            branch_pattern,
            pattern_metrics: vec![0.0; 1usize << N],
            tb: TrellisTraceback::new(num_states, 5 * K),
        }
    }

    fn best_state(&self) -> usize {
        self.path_metrics
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(idx, _)| idx)
            .unwrap_or(0)
    }

    /// Trace back the entire decoded bitstream from a known terminal state.
    pub fn traceback_all_from_state(&self, end_state: usize) -> Vec<u8> {
        self.tb.traceback_all_from_state(end_state)
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let best = self.best_state();
        self.tb.finish(best)
    }

    /// Decode a full terminated block without incremental traceback or history trimming.
    pub fn decode_block_from_state(&mut self, inputs: &[[f32; N]], end_state: usize) -> Vec<u8> {
        self.path_metrics.fill(Self::INF_METRIC);
        self.path_metrics[0] = 0.0;
        self.next_metrics.fill(Self::INF_METRIC);
        self.tb.prepare_block(inputs.len());

        for input in inputs {
            self.fill_pattern_metrics(input);
            self.next_metrics.fill(Self::INF_METRIC);
            self.tb.scratch_prev_state.fill(0);
            self.tb.scratch_bit.fill(0);

            for prev_state in 0..self.num_states {
                let prev_metric = self.path_metrics[prev_state];
                if prev_metric >= Self::INF_METRIC {
                    continue;
                }

                for input_bit in 0..=1usize {
                    let next_state = self.branch_next[prev_state][input_bit] as usize;
                    let bm =
                        self.pattern_metrics[self.branch_pattern[prev_state][input_bit] as usize];
                    let cand = prev_metric + bm;
                    if cand < self.next_metrics[next_state] {
                        self.next_metrics[next_state] = cand;
                        self.tb.scratch_prev_state[next_state] = prev_state as u16;
                        self.tb.scratch_bit[next_state] = input_bit as u8;
                    }
                }
            }

            core::mem::swap(&mut self.path_metrics, &mut self.next_metrics);
            self.tb.record_step();
        }

        self.tb.traceback_all_from_state(end_state)
    }

    /// Returns the trellis state with the minimum path metric after the most
    /// recent forward pass.
    pub fn ml_best_terminal_state(&self) -> usize {
        self.best_state()
    }

    /// Bit `i` of a pattern index is the expected value of parity `i`.
    #[inline]
    fn fill_pattern_metrics(&mut self, received: &[f32; N]) {
        for (pattern, metric) in self.pattern_metrics.iter_mut().enumerate() {
            let mut sum = 0.0f64;
            for (i, received_i) in received.iter().enumerate() {
                let expected = ((pattern >> i) & 1) as f32;
                let d = (expected - *received_i) as f64;
                sum += d * d;
            }
            *metric = sum;
        }
    }

    /// Process one symbol of N soft values (each in ~0.0..1.0 range).
    /// Returns a decoded bit once traceback depth is reached.
    pub fn process(&mut self, input: &[f32; N]) -> Option<u8> {
        self.fill_pattern_metrics(input);
        self.next_metrics.fill(Self::INF_METRIC);
        self.tb.scratch_prev_state.fill(0);
        self.tb.scratch_bit.fill(0);

        for prev_state in 0..self.num_states {
            let prev_metric = self.path_metrics[prev_state];
            if prev_metric >= Self::INF_METRIC {
                continue;
            }

            for input_bit in 0..=1usize {
                let next_state = self.branch_next[prev_state][input_bit] as usize;
                let bm = self.pattern_metrics[self.branch_pattern[prev_state][input_bit] as usize];
                let cand = prev_metric + bm;
                if cand < self.next_metrics[next_state] {
                    self.next_metrics[next_state] = cand;
                    self.tb.scratch_prev_state[next_state] = prev_state as u16;
                    self.tb.scratch_bit[next_state] = input_bit as u8;
                }
            }
        }

        core::mem::swap(&mut self.path_metrics, &mut self.next_metrics);
        self.tb.record_step();
        self.tb.trim_history();

        if self.tb.steps >= self.tb.traceback_depth {
            let best = self.best_state();
            Some(
                self.tb
                    .traceback_bit(best, self.tb.steps - 1, self.tb.traceback_depth),
            )
        } else {
            None
        }
    }
}

pub fn get_1_3_k9_soft_viterbi_decoder() -> SoftViterbiDecoder<9, 3> {
    SoftViterbiDecoder::new(get_1_3_k9_encoder())
}

pub fn get_1_2_k9_soft_viterbi_decoder() -> SoftViterbiDecoder<9, 2> {
    SoftViterbiDecoder::new(get_1_2_k9_encoder())
}

thread_local! {
    static R12_K9_SOFT_DECODER: RefCell<SoftViterbiDecoder<9, 2>> =
        RefCell::new(get_1_2_k9_soft_viterbi_decoder());
    static R13_K9_SOFT_DECODER: RefCell<SoftViterbiDecoder<9, 3>> =
        RefCell::new(get_1_3_k9_soft_viterbi_decoder());
    static R14_K9_SOFT_DECODER: RefCell<SoftViterbiDecoder<9, 4>> =
        RefCell::new(get_1_4_k9_soft_viterbi_decoder());
}

/// Rate-1/2 K=9 counterpart of [`with_1_3_k9_soft_viterbi_decoder`].
pub fn with_1_2_k9_soft_viterbi_decoder<R>(
    f: impl FnOnce(&mut SoftViterbiDecoder<9, 2>) -> R,
) -> R {
    R12_K9_SOFT_DECODER.with(|decoder| f(&mut decoder.borrow_mut()))
}

/// Rate-1/4 K=9 counterpart of [`with_1_3_k9_soft_viterbi_decoder`].
pub fn with_1_4_k9_soft_viterbi_decoder<R>(
    f: impl FnOnce(&mut SoftViterbiDecoder<9, 4>) -> R,
) -> R {
    R14_K9_SOFT_DECODER.with(|decoder| f(&mut decoder.borrow_mut()))
}

/// Reusable rate-1/3 K=9 soft decoder, since building one fills a 256-state
/// trellis. `decode_block_from_state` clears the trellis on entry, so nothing
/// carries between borrows. `f` must not borrow the decoder again.
pub fn with_1_3_k9_soft_viterbi_decoder<R>(
    f: impl FnOnce(&mut SoftViterbiDecoder<9, 3>) -> R,
) -> R {
    R13_K9_SOFT_DECODER.with(|decoder| f(&mut decoder.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::{
        Encoder, SoftViterbiDecoder, ViterbiDecoder, get_1_2_k9_encoder, get_1_3_k9_encoder,
        get_1_3_k9_soft_viterbi_decoder, get_1_3_k9_viterbi_decoder,
    };

    fn spec_reference_encode<const K: usize, const R: usize>(
        input: &[u8],
        generators: [u32; R],
    ) -> Vec<[u8; R]> {
        let mut window = 0u32;
        let mut out = Vec::with_capacity(input.len());

        for &bit in input {
            window = (window >> 1) & ((1u32 << (K - 1)) - 1);
            window |= ((bit & 1) as u32) << (K - 1);

            let mut symbols = [0u8; R];
            for r in 0..R {
                let mut parity = 0u8;
                for k in 0..K {
                    if ((generators[r] >> k) & 1) != 0 {
                        parity ^= ((window >> k) & 1) as u8;
                    }
                }
                symbols[r] = parity;
            }
            out.push(symbols);
        }

        out
    }

    #[test]
    pub fn test_encoder() {
        let mut encoder = Encoder::<3, 2>::new([0b111, 0b110]);
        let test_cases = [
            (1, [1, 1]),
            (0, [1, 1]),
            (1, [0, 1]),
            (1, [0, 0]),
            (0, [0, 1]),
        ];

        for case in test_cases {
            assert_eq!(
                case.1,
                encoder.encode(case.0),
                "test case: {:?}, state: {:032b}",
                case,
                encoder.state()
            );
        }
    }

    fn decode_all<const K: usize, const N: usize>(
        decoder: &mut ViterbiDecoder<K, N>,
        symbols: &[u8],
    ) -> Vec<u8> {
        let mut out = symbols
            .chunks_exact(N)
            .filter_map(|d| decoder.process(d.try_into().unwrap()))
            .collect::<Vec<_>>();
        out.extend(decoder.finish());
        out
    }

    fn soft_decode_all<const K: usize, const N: usize>(
        decoder: &mut SoftViterbiDecoder<K, N>,
        symbols: &[f32],
    ) -> Vec<u8> {
        let mut out = symbols
            .chunks_exact(N)
            .filter_map(|d| decoder.process(d.try_into().unwrap()))
            .collect::<Vec<_>>();
        out.extend(decoder.finish());
        out
    }

    #[test]
    pub fn test_decoder_roundtrip_noiseless() {
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0,
        ];
        let mut encoder = get_1_2_k9_encoder();
        let sent = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .collect::<Vec<_>>();

        let mut decoder = ViterbiDecoder::new(get_1_2_k9_encoder());
        let decoded = decode_all(&mut decoder, &sent);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    pub fn test_decoder_roundtrip_with_bit_errors() {
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1, 1,
            0, 1, 1,
        ];
        let mut encoder = get_1_2_k9_encoder();
        let mut sent = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .collect::<Vec<_>>();

        // Introduce a few hard-decision symbol errors.
        for idx in [3usize, 9, 31, 44] {
            sent[idx] ^= 1;
        }

        let mut decoder = ViterbiDecoder::new(get_1_2_k9_encoder());
        let decoded = decode_all(&mut decoder, &sent);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    pub fn test_decoder_roundtrip_rate_third_noiseless() {
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0,
        ];
        let mut encoder = get_1_3_k9_encoder();
        let sent = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .collect::<Vec<_>>();

        let mut decoder = get_1_3_k9_viterbi_decoder();
        let decoded = decode_all(&mut decoder, &sent);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    fn test_spec_k9_rate_half_matches_reference_model() {
        // C.S0002-E 3.1.3.1.5.1.4: g0=753(octal), g1=561(octal), K=9
        let generators = [0o753, 0o561];
        let input = [1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 0, 0, 1, 1];

        let mut encoder = Encoder::<9, 2>::new(generators);
        let got = input.iter().map(|b| encoder.encode(*b)).collect::<Vec<_>>();
        let expected = spec_reference_encode::<9, 2>(&input, generators);

        assert_eq!(expected, got);
    }

    #[test]
    fn test_spec_k9_rate_third_matches_reference_model() {
        // C.S0002-E 3.1.3.1.5.1.3: g0=557(octal), g1=663(octal), g2=711(octal), K=9
        let generators = [0o557, 0o663, 0o711];
        let input = [1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 1, 1];

        let mut encoder = get_1_3_k9_encoder();
        let got = input.iter().map(|b| encoder.encode(*b)).collect::<Vec<_>>();
        let expected = spec_reference_encode::<9, 3>(&input, generators);

        assert_eq!(expected, got);
    }

    #[test]
    fn test_spec_k9_rate_quarter_matches_reference_model() {
        // C.S0002-E 3.1.3.1.5.1.2: g0=765(octal), g1=671(octal), g2=513(octal), g3=473(octal), K=9
        let generators = [0o765, 0o671, 0o513, 0o473];
        let input = [1, 1, 0, 1, 0, 1, 1, 0, 0, 1];

        let mut encoder = Encoder::<9, 4>::new(generators);
        let got = input.iter().map(|b| encoder.encode(*b)).collect::<Vec<_>>();
        let expected = spec_reference_encode::<9, 4>(&input, generators);

        assert_eq!(expected, got);
    }

    #[test]
    fn test_soft_decoder_decode_block_from_state_handles_long_terminated_frame() {
        let mut msg = vec![0u8; 88];
        for (i, b) in msg.iter_mut().enumerate() {
            *b = ((i * 7 + 3) % 11 >= 5) as u8;
        }
        let mut terminated = msg.clone();
        terminated.extend(std::iter::repeat_n(0u8, 8));

        let mut enc = get_1_3_k9_encoder();
        let encoded = terminated
            .iter()
            .flat_map(|b| enc.encode(*b))
            .map(|b| b as f32)
            .collect::<Vec<_>>();
        let soft_inputs = encoded
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect::<Vec<_>>();

        let mut dec = get_1_3_k9_soft_viterbi_decoder();
        let decoded = dec.decode_block_from_state(&soft_inputs, 0);

        assert_eq!(terminated, decoded);
    }

    #[test]
    fn test_spec_k9_rate_sixth_matches_reference_model() {
        // C.S0002-E 3.1.3.1.5.1.1: g0=457(octal), g1=755(octal), g2=551(octal), g3=637(octal), g4=625(octal), g5=727(octal), K=9
        let generators = [0o457, 0o755, 0o551, 0o637, 0o625, 0o727];
        let input = [1, 0, 0, 1, 1, 0, 1, 0];

        let mut encoder = Encoder::<9, 6>::new(generators);
        let got = input.iter().map(|b| encoder.encode(*b)).collect::<Vec<_>>();
        let expected = spec_reference_encode::<9, 6>(&input, generators);

        assert_eq!(expected, got);
    }

    #[test]
    fn test_spec_initial_state_all_zero_emits_zero_for_zero_input() {
        // C.S0002-E 3.1.3.1.5.1: all-zero initial state.
        let mut r12 = Encoder::<9, 2>::new([0o753, 0o561]);
        let mut r13 = Encoder::<9, 3>::new([0o557, 0o663, 0o711]);
        let mut r14 = Encoder::<9, 4>::new([0o765, 0o671, 0o513, 0o473]);
        let mut r16 = Encoder::<9, 6>::new([0o457, 0o755, 0o551, 0o637, 0o625, 0o727]);

        assert_eq!([0, 0], r12.encode(0));
        assert_eq!([0, 0, 0], r13.encode(0));
        assert_eq!([0, 0, 0, 0], r14.encode(0));
        assert_eq!([0, 0, 0, 0, 0, 0], r16.encode(0));
    }

    #[test]
    fn test_spec_output_symbol_order_follows_g0_g1_for_rate_half() {
        // C.S0002-E 3.1.3.1.5.1.4: c0 from g0 first, c1 from g1 second.
        let input = [1, 0, 1, 1, 0, 1, 0, 0, 1];

        let mut g0_g1 = Encoder::<9, 2>::new([0o753, 0o561]);
        let mut g1_g0 = Encoder::<9, 2>::new([0o561, 0o753]);

        let ordered = input.iter().map(|b| g0_g1.encode(*b)).collect::<Vec<_>>();
        let swapped = input.iter().map(|b| g1_g0.encode(*b)).collect::<Vec<_>>();

        assert_ne!(ordered, swapped);
    }

    #[test]
    fn test_spec_frame_reinitialization_behavior_with_tail_zeros() {
        // Channel-specific clauses require encoder re-initialization at frame boundaries for some channels.
        // This test verifies that reset() returns the encoder to all-zero state before a tail of zeros.
        let mut encoder = Encoder::<9, 2>::new([0o753, 0o561]);
        for bit in [1, 1, 0, 1, 0, 1, 1, 0, 1, 1] {
            let _ = encoder.encode(bit);
        }
        encoder.reset();

        let tail = (0..8).map(|_| encoder.encode(0)).collect::<Vec<_>>();
        assert!(tail.iter().all(|sym| *sym == [0, 0]));
    }

    // --- Soft Viterbi decoder tests ---

    #[test]
    fn test_soft_decoder_roundtrip_noiseless() {
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0,
        ];
        let mut encoder = get_1_2_k9_encoder();
        let sent: Vec<f32> = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .map(|b| b as f32)
            .collect();

        let mut decoder = SoftViterbiDecoder::new(get_1_2_k9_encoder());
        let decoded = soft_decode_all(&mut decoder, &sent);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    fn test_soft_decoder_roundtrip_rate_third_noiseless() {
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0,
        ];
        let mut encoder = get_1_3_k9_encoder();
        let sent: Vec<f32> = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .map(|b| b as f32)
            .collect();

        let mut decoder = get_1_3_k9_soft_viterbi_decoder();
        let decoded = soft_decode_all(&mut decoder, &sent);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    fn test_soft_decoder_with_noisy_soft_symbols() {
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1, 1,
            0, 1, 1,
        ];
        let mut encoder = get_1_2_k9_encoder();
        let mut sent: Vec<f32> = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .map(|b| b as f32)
            .collect();

        // Add noise that stays on the correct side of 0.5 threshold,
        // but with varying confidence. A hard decoder would also get these right,
        // but this tests the soft path accepts continuous values.
        for (i, s) in sent.iter_mut().enumerate() {
            let noise = ((i as f32 * 0.37).sin()) * 0.3;
            *s = (*s + noise).clamp(0.0, 1.0);
        }

        let mut decoder = SoftViterbiDecoder::new(get_1_2_k9_encoder());
        let decoded = soft_decode_all(&mut decoder, &sent);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    fn test_soft_decoder_corrects_errors_near_threshold() {
        // Soft decoding should correct errors that hard decoding would also correct,
        // and additionally benefit from confidence information.
        let msg = [
            1, 0, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 1, 0, 0, 1, 1,
            0, 1, 1,
        ];
        let mut encoder = get_1_2_k9_encoder();
        let clean: Vec<f32> = msg
            .iter()
            .flat_map(|s| encoder.encode(*s))
            .map(|b| b as f32)
            .collect();

        // Introduce soft errors: flip a few symbols to be just barely wrong
        // (e.g., a bit that should be 1.0 becomes 0.45 — wrong side of threshold
        // but barely so, giving the soft decoder a better chance).
        let mut noisy = clean.clone();
        for idx in [3usize, 9, 31, 44] {
            if noisy[idx] > 0.5 {
                noisy[idx] = 0.45; // just barely wrong
            } else {
                noisy[idx] = 0.55; // just barely wrong
            }
        }

        let mut decoder = SoftViterbiDecoder::new(get_1_2_k9_encoder());
        let decoded = soft_decode_all(&mut decoder, &noisy);
        assert_eq!(&msg[..], &decoded[..msg.len()]);
    }

    #[test]
    fn test_soft_decoder_matches_hard_on_clean_input() {
        // On perfectly clean (0.0/1.0) input, soft and hard should produce identical output.
        let msg = [1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 1, 0, 0, 1, 0, 1];
        let mut encoder = get_1_2_k9_encoder();
        let sent_hard: Vec<u8> = msg.iter().flat_map(|s| encoder.encode(*s)).collect();
        let sent_soft: Vec<f32> = sent_hard.iter().map(|b| *b as f32).collect();

        let mut hard_dec = ViterbiDecoder::new(get_1_2_k9_encoder());
        let hard_out = decode_all(&mut hard_dec, &sent_hard);

        let mut soft_dec = SoftViterbiDecoder::new(get_1_2_k9_encoder());
        let soft_out = soft_decode_all(&mut soft_dec, &sent_soft);

        assert_eq!(hard_out, soft_out);
    }
}
