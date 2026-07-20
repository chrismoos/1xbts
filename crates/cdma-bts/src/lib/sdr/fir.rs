use num_complex::Complex32;

#[inline]
fn symmetric_complex_accumulate(
    taps: &[f32],
    delay_re: &[f32],
    delay_im: &[f32],
    head: usize,
    last: usize,
) -> Complex32 {
    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { symmetric_complex_accumulate_neon(taps, delay_re, delay_im, head, last) };
    }
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe { symmetric_complex_accumulate_sse(taps, delay_re, delay_im, head, last) };
    }
    #[allow(unreachable_code)]
    symmetric_complex_accumulate_scalar(taps, delay_re, delay_im, head, last)
}

#[inline]
fn symmetric_complex_accumulate_scalar(
    taps: &[f32],
    delay_re: &[f32],
    delay_im: &[f32],
    head: usize,
    last: usize,
) -> Complex32 {
    let mut re = 0.0f32;
    let mut im = 0.0f32;
    for i in 0..taps.len() {
        unsafe {
            let coeff = *taps.get_unchecked(i);
            re += (*delay_re.get_unchecked(head + i) + *delay_re.get_unchecked(last - i)) * coeff;
            im += (*delay_im.get_unchecked(head + i) + *delay_im.get_unchecked(last - i)) * coeff;
        }
    }
    Complex32::new(re, im)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn symmetric_complex_accumulate_neon(
    taps: &[f32],
    delay_re: &[f32],
    delay_im: &[f32],
    head: usize,
    last: usize,
) -> Complex32 {
    use std::arch::aarch64::{vaddq_f32, vaddvq_f32, vdupq_n_f32, vfmaq_f32, vld1q_f32};

    let chunks = taps.len() / 4;
    let mut acc_re = vdupq_n_f32(0.0);
    let mut acc_im = vdupq_n_f32(0.0);
    for chunk in 0..chunks {
        let i = chunk * 4;
        let coeff = unsafe { vld1q_f32(taps.as_ptr().add(i)) };
        let front_re = unsafe { vld1q_f32(delay_re.as_ptr().add(head + i)) };
        let front_im = unsafe { vld1q_f32(delay_im.as_ptr().add(head + i)) };
        let back_re = unsafe { reverse_f32x4_neon(vld1q_f32(delay_re.as_ptr().add(last - i - 3))) };
        let back_im = unsafe { reverse_f32x4_neon(vld1q_f32(delay_im.as_ptr().add(last - i - 3))) };
        acc_re = vfmaq_f32(acc_re, coeff, vaddq_f32(front_re, back_re));
        acc_im = vfmaq_f32(acc_im, coeff, vaddq_f32(front_im, back_im));
    }
    let mut re = vaddvq_f32(acc_re);
    let mut im = vaddvq_f32(acc_im);
    for i in (chunks * 4)..taps.len() {
        unsafe {
            let coeff = *taps.get_unchecked(i);
            re += (*delay_re.get_unchecked(head + i) + *delay_re.get_unchecked(last - i)) * coeff;
            im += (*delay_im.get_unchecked(head + i) + *delay_im.get_unchecked(last - i)) * coeff;
        }
    }
    Complex32::new(re, im)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn reverse_f32x4_neon(
    v: std::arch::aarch64::float32x4_t,
) -> std::arch::aarch64::float32x4_t {
    use std::arch::aarch64::{vcombine_f32, vget_high_f32, vget_low_f32, vrev64q_f32};

    let swapped_pairs = vrev64q_f32(v);
    vcombine_f32(vget_high_f32(swapped_pairs), vget_low_f32(swapped_pairs))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
#[inline]
unsafe fn symmetric_complex_accumulate_sse(
    taps: &[f32],
    delay_re: &[f32],
    delay_im: &[f32],
    head: usize,
    last: usize,
) -> Complex32 {
    use std::arch::x86_64::{
        _mm_add_ps, _mm_add_ss, _mm_cvtss_f32, _mm_loadu_ps, _mm_movehl_ps, _mm_mul_ps,
        _mm_setzero_ps, _mm_shuffle_ps,
    };

    let chunks = taps.len() / 4;
    let mut acc_re = _mm_setzero_ps();
    let mut acc_im = _mm_setzero_ps();
    for chunk in 0..chunks {
        let i = chunk * 4;
        let coeff = unsafe { _mm_loadu_ps(taps.as_ptr().add(i)) };
        let front_re = unsafe { _mm_loadu_ps(delay_re.as_ptr().add(head + i)) };
        let front_im = unsafe { _mm_loadu_ps(delay_im.as_ptr().add(head + i)) };
        let back_re = unsafe { _mm_loadu_ps(delay_re.as_ptr().add(last - i - 3)) };
        let back_im = unsafe { _mm_loadu_ps(delay_im.as_ptr().add(last - i - 3)) };
        let back_re = _mm_shuffle_ps::<0x1b>(back_re, back_re);
        let back_im = _mm_shuffle_ps::<0x1b>(back_im, back_im);
        acc_re = _mm_add_ps(acc_re, _mm_mul_ps(coeff, _mm_add_ps(front_re, back_re)));
        acc_im = _mm_add_ps(acc_im, _mm_mul_ps(coeff, _mm_add_ps(front_im, back_im)));
    }

    let mut re = hsum_f32x4_sse(acc_re);
    let mut im = hsum_f32x4_sse(acc_im);

    for i in (chunks * 4)..taps.len() {
        unsafe {
            let coeff = *taps.get_unchecked(i);
            re += (*delay_re.get_unchecked(head + i) + *delay_re.get_unchecked(last - i)) * coeff;
            im += (*delay_im.get_unchecked(head + i) + *delay_im.get_unchecked(last - i)) * coeff;
        }
    }
    Complex32::new(re, im)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
#[inline]
unsafe fn hsum_f32x4_sse(v: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::{_mm_add_ps, _mm_add_ss, _mm_cvtss_f32, _mm_movehl_ps, _mm_shuffle_ps};

    let high = _mm_movehl_ps(v, v);
    let pair_sums = _mm_add_ps(v, high);
    let lane_1 = _mm_shuffle_ps::<0x55>(pair_sums, pair_sums);
    _mm_cvtss_f32(_mm_add_ss(pair_sums, lane_1))
}

/// Stateful f32 FIR for the CDMA pulse-shaping filters used by this crate.
///
/// This intentionally supports only the modes we use here: decimation is
/// always 1, with optional polyphase interpolation for TX pulse shaping.
pub struct Fir32 {
    taps: Vec<f32>,
    delay: Vec<f32>,
    head: usize,
    delay_len: usize,
    interpolate: usize,
}

impl Fir32 {
    pub fn new(taps: &[f64]) -> Self {
        Self::with_interpolate(taps, 1)
    }

    pub fn with_interpolate(taps: &[f64], interpolate: usize) -> Self {
        assert!(!taps.is_empty());
        assert!(interpolate > 0);

        let mut taps = taps.iter().map(|&tap| tap as f32).collect::<Vec<_>>();
        let rem = taps.len() % interpolate;
        if rem != 0 {
            taps.resize(taps.len() + (interpolate - rem), 0.0);
        }
        let delay_len = taps.len() / interpolate;
        Self {
            taps,
            delay: vec![0.0; delay_len * 2],
            head: 1 % delay_len,
            delay_len,
            interpolate,
        }
    }

    #[inline]
    pub fn process_sample(&mut self, sample: f32) -> f32 {
        debug_assert_eq!(self.interpolate, 1);
        self.push_sample(sample);
        self.accumulate_phase_1()
    }

    pub fn process_sample_interpolated(&mut self, sample: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.interpolate);
        self.process_sample_interpolated_into(sample, &mut out);
        out
    }

    #[inline]
    pub fn process_sample_interpolated_into(&mut self, sample: f32, out: &mut Vec<f32>) {
        for phase in 0..self.interpolate {
            if phase == 0 {
                self.push_sample(sample);
            }
            out.push(self.accumulate_phase(phase));
        }
    }

    pub fn process_block(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len() * self.interpolate);
        self.process_block_into(input, &mut out);
        out
    }

    pub fn process_block_into(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(input.len() * self.interpolate);
        if self.interpolate == 1 {
            for &sample in input {
                self.push_sample(sample);
                out.push(self.accumulate_phase_1());
            }
            return;
        }

        for &sample in input {
            self.process_sample_interpolated_into(sample, out);
        }
    }

    #[inline]
    fn push_sample(&mut self, sample: f32) {
        self.head = if self.head == 0 {
            self.delay_len - 1
        } else {
            self.head - 1
        };
        self.delay[self.head] = sample;
        self.delay[self.head + self.delay_len] = sample;
    }

    #[inline]
    fn accumulate_phase_1(&self) -> f32 {
        debug_assert_eq!(self.interpolate, 1);
        debug_assert!(self.head + self.delay_len <= self.delay.len());
        debug_assert_eq!(self.delay_len, self.taps.len());

        let mut acc = 0.0f32;
        let mut delay = unsafe { self.delay.as_ptr().add(self.head) };
        let mut tap = self.taps.as_ptr();
        for _ in 0..self.delay_len {
            unsafe {
                acc += *delay * *tap;
                delay = delay.add(1);
                tap = tap.add(1);
            }
        }
        acc
    }

    #[inline]
    fn accumulate_phase(&self, phase: usize) -> f32 {
        debug_assert!(phase < self.interpolate);
        debug_assert!(self.head + self.delay_len <= self.delay.len());
        debug_assert_eq!(self.delay_len * self.interpolate, self.taps.len());

        let mut acc = 0.0f32;
        let mut tap_idx = phase;
        let mut delay = unsafe { self.delay.as_ptr().add(self.head) };
        while tap_idx < self.taps.len() {
            unsafe {
                acc += *delay * *self.taps.as_ptr().add(tap_idx);
                delay = delay.add(1);
            }
            tap_idx += self.interpolate;
        }
        acc * self.interpolate as f32
    }
}

/// Dot a real tap vector against the real and imaginary parts of a contiguous
/// complex delay window.
#[inline]
fn dot_re_im(taps: &[f32], re: &[f32], im: &[f32]) -> (f32, f32) {
    debug_assert!(re.len() >= taps.len() && im.len() >= taps.len());
    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { dot_re_im_neon(taps, re, im) };
    }
    #[cfg(target_arch = "x86_64")]
    {
        return unsafe { dot_re_im_sse(taps, re, im) };
    }
    #[allow(unreachable_code)]
    dot_re_im_scalar(taps, re, im)
}

#[inline]
fn dot_re_im_scalar(taps: &[f32], re: &[f32], im: &[f32]) -> (f32, f32) {
    let mut sum_re = 0.0f32;
    let mut sum_im = 0.0f32;
    for i in 0..taps.len() {
        unsafe {
            let coeff = *taps.get_unchecked(i);
            sum_re += *re.get_unchecked(i) * coeff;
            sum_im += *im.get_unchecked(i) * coeff;
        }
    }
    (sum_re, sum_im)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn dot_re_im_neon(taps: &[f32], re: &[f32], im: &[f32]) -> (f32, f32) {
    use std::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32, vld1q_f32};

    let chunks = taps.len() / 4;
    let mut acc_re = vdupq_n_f32(0.0);
    let mut acc_im = vdupq_n_f32(0.0);
    for chunk in 0..chunks {
        let i = chunk * 4;
        let coeff = unsafe { vld1q_f32(taps.as_ptr().add(i)) };
        let lane_re = unsafe { vld1q_f32(re.as_ptr().add(i)) };
        let lane_im = unsafe { vld1q_f32(im.as_ptr().add(i)) };
        acc_re = vfmaq_f32(acc_re, coeff, lane_re);
        acc_im = vfmaq_f32(acc_im, coeff, lane_im);
    }
    let mut sum_re = vaddvq_f32(acc_re);
    let mut sum_im = vaddvq_f32(acc_im);
    for i in (chunks * 4)..taps.len() {
        unsafe {
            let coeff = *taps.get_unchecked(i);
            sum_re += *re.get_unchecked(i) * coeff;
            sum_im += *im.get_unchecked(i) * coeff;
        }
    }
    (sum_re, sum_im)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse")]
#[inline]
unsafe fn dot_re_im_sse(taps: &[f32], re: &[f32], im: &[f32]) -> (f32, f32) {
    use std::arch::x86_64::{_mm_add_ps, _mm_loadu_ps, _mm_mul_ps, _mm_setzero_ps};

    let chunks = taps.len() / 4;
    let mut acc_re = _mm_setzero_ps();
    let mut acc_im = _mm_setzero_ps();
    for chunk in 0..chunks {
        let i = chunk * 4;
        let coeff = unsafe { _mm_loadu_ps(taps.as_ptr().add(i)) };
        let lane_re = unsafe { _mm_loadu_ps(re.as_ptr().add(i)) };
        let lane_im = unsafe { _mm_loadu_ps(im.as_ptr().add(i)) };
        acc_re = _mm_add_ps(acc_re, _mm_mul_ps(coeff, lane_re));
        acc_im = _mm_add_ps(acc_im, _mm_mul_ps(coeff, lane_im));
    }
    let mut sum_re = hsum_f32x4_sse(acc_re);
    let mut sum_im = hsum_f32x4_sse(acc_im);
    for i in (chunks * 4)..taps.len() {
        unsafe {
            let coeff = *taps.get_unchecked(i);
            sum_re += *re.get_unchecked(i) * coeff;
            sum_im += *im.get_unchecked(i) * coeff;
        }
    }
    (sum_re, sum_im)
}

#[derive(Clone, Copy, Default)]
struct ComplexDelay {
    re: f32,
    im: f32,
}

pub struct ComplexFir32 {
    taps: Vec<f32>,
    delay: Vec<ComplexDelay>,
    head: usize,
    delay_len: usize,
    interpolate: usize,
}

impl ComplexFir32 {
    pub fn new(taps: &[f64]) -> Self {
        Self::with_interpolate(taps, 1)
    }

    pub fn with_interpolate(taps: &[f64], interpolate: usize) -> Self {
        assert!(!taps.is_empty());
        assert!(interpolate > 0);

        let mut taps = taps.iter().map(|&tap| tap as f32).collect::<Vec<_>>();
        let rem = taps.len() % interpolate;
        if rem != 0 {
            taps.resize(taps.len() + (interpolate - rem), 0.0);
        }
        let delay_len = taps.len() / interpolate;
        Self {
            taps,
            delay: vec![ComplexDelay::default(); delay_len * 2],
            head: 1 % delay_len,
            delay_len,
            interpolate,
        }
    }

    #[inline]
    pub fn process_sample(&mut self, sample: Complex32) -> Complex32 {
        debug_assert_eq!(self.interpolate, 1);
        self.push_sample(sample);
        self.accumulate_phase_1()
    }

    #[inline]
    pub fn process_sample_if(&mut self, sample: Complex32, emit: bool) -> Option<Complex32> {
        debug_assert_eq!(self.interpolate, 1);
        self.push_sample(sample);
        emit.then(|| self.accumulate_phase_1())
    }

    pub fn process_sample_interpolated(&mut self, sample: Complex32) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(self.interpolate);
        self.process_sample_interpolated_into(sample, &mut out);
        out
    }

    pub fn process_sample_interpolated_into(
        &mut self,
        sample: Complex32,
        out: &mut Vec<Complex32>,
    ) {
        for phase in 0..self.interpolate {
            if phase == 0 {
                self.push_sample(sample);
            }
            out.push(self.accumulate_phase(phase));
        }
    }

    pub fn process_block(&mut self, input: &[Complex32]) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(input.len() * self.interpolate);
        self.process_block_into(input, &mut out);
        out
    }

    pub fn process_block_into(&mut self, input: &[Complex32], out: &mut Vec<Complex32>) {
        out.clear();
        out.reserve(input.len() * self.interpolate);
        if self.interpolate == 1 {
            for &sample in input {
                self.push_sample(sample);
                out.push(self.accumulate_phase_1());
            }
            return;
        }

        for &sample in input {
            self.process_sample_interpolated_into(sample, out);
        }
    }

    #[inline]
    fn push_sample(&mut self, sample: Complex32) {
        self.head = if self.head == 0 {
            self.delay_len - 1
        } else {
            self.head - 1
        };
        let sample = ComplexDelay {
            re: sample.re,
            im: sample.im,
        };
        self.delay[self.head] = sample;
        self.delay[self.head + self.delay_len] = sample;
    }

    #[inline]
    fn accumulate_phase_1(&self) -> Complex32 {
        debug_assert_eq!(self.interpolate, 1);
        debug_assert!(self.head + self.delay_len <= self.delay.len());
        debug_assert_eq!(self.delay_len, self.taps.len());

        let mut re = 0.0f32;
        let mut im = 0.0f32;
        let mut delay = unsafe { self.delay.as_ptr().add(self.head) };
        let mut tap = self.taps.as_ptr();
        for _ in 0..self.delay_len {
            unsafe {
                let sample = *delay;
                let coeff = *tap;
                re += sample.re * coeff;
                im += sample.im * coeff;
                delay = delay.add(1);
                tap = tap.add(1);
            }
        }
        Complex32::new(re, im)
    }

    #[inline]
    fn accumulate_phase(&self, phase: usize) -> Complex32 {
        debug_assert!(phase < self.interpolate);
        debug_assert!(self.head + self.delay_len <= self.delay.len());
        debug_assert_eq!(self.delay_len * self.interpolate, self.taps.len());

        let mut re = 0.0f32;
        let mut im = 0.0f32;
        let mut tap_idx = phase;
        let mut delay = unsafe { self.delay.as_ptr().add(self.head) };
        while tap_idx < self.taps.len() {
            unsafe {
                let sample = *delay;
                let coeff = *self.taps.as_ptr().add(tap_idx);
                re += sample.re * coeff;
                im += sample.im * coeff;
                delay = delay.add(1);
            }
            tap_idx += self.interpolate;
        }
        let gain = self.interpolate as f32;
        Complex32::new(re * gain, im * gain)
    }
}

/// Interpolating polyphase complex FIR matching `ComplexFir32::with_interpolate`
/// semantics, restructured so each output phase is one contiguous
/// [`dot_re_im`] over de-interleaved delay lines.
pub struct PolyphaseComplexFir32 {
    /// Row `p`, column `j` holds `padded_taps[p + j * interpolate]`.
    phase_taps: Vec<f32>,
    delay_re: Vec<f32>,
    delay_im: Vec<f32>,
    head: usize,
    delay_len: usize,
    row_len: usize,
    interpolate: usize,
}

impl PolyphaseComplexFir32 {
    pub fn with_interpolate(taps: &[f64], interpolate: usize) -> Self {
        assert!(!taps.is_empty());
        assert!(interpolate > 0);

        let mut taps = taps.iter().map(|&tap| tap as f32).collect::<Vec<_>>();
        let rem = taps.len() % interpolate;
        if rem != 0 {
            taps.resize(taps.len() + (interpolate - rem), 0.0);
        }
        let delay_len = taps.len() / interpolate;
        let row_len = delay_len.next_multiple_of(4);
        let mut phase_taps = vec![0.0f32; interpolate * row_len];
        for (idx, &tap) in taps.iter().enumerate() {
            let phase = idx % interpolate;
            let column = idx / interpolate;
            phase_taps[phase * row_len + column] = tap;
        }
        // Reads span `head..head + row_len` with `head < delay_len`, so the
        // doubled ring needs a zero tail of `row_len - delay_len` behind it.
        let delay_size = delay_len * 2 + (row_len - delay_len);
        Self {
            phase_taps,
            delay_re: vec![0.0; delay_size],
            delay_im: vec![0.0; delay_size],
            head: 1 % delay_len,
            delay_len,
            row_len,
            interpolate,
        }
    }

    pub fn process_block(&mut self, input: &[Complex32]) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(input.len() * self.interpolate);
        self.process_block_into(input, &mut out);
        out
    }

    pub fn process_block_into(&mut self, input: &[Complex32], out: &mut Vec<Complex32>) {
        out.clear();
        out.reserve(input.len() * self.interpolate);
        let gain = self.interpolate as f32;
        for &sample in input {
            self.push_sample(sample);
            for phase in 0..self.interpolate {
                let row = &self.phase_taps[phase * self.row_len..(phase + 1) * self.row_len];
                let (re, im) = dot_re_im(
                    row,
                    &self.delay_re[self.head..self.head + self.row_len],
                    &self.delay_im[self.head..self.head + self.row_len],
                );
                out.push(Complex32::new(re * gain, im * gain));
            }
        }
    }

    #[inline]
    fn push_sample(&mut self, sample: Complex32) {
        self.head = if self.head == 0 {
            self.delay_len - 1
        } else {
            self.head - 1
        };
        self.delay_re[self.head] = sample.re;
        self.delay_im[self.head] = sample.im;
        self.delay_re[self.head + self.delay_len] = sample.re;
        self.delay_im[self.head + self.delay_len] = sample.im;
    }
}

/// Stateful complex FIR optimized for symmetric, non-interpolating real taps.
///
/// For the CDMA2000 matched filter the 48 real coefficients are mirrored, so
/// each output can be computed as `tap[i] * (x[n-i] + x[n-(N-1-i)])`. This
/// halves the number of coefficient multiplies while preserving the exact FIR
/// response.
pub struct SymmetricComplexFir32 {
    half_taps: Vec<f32>,
    center_tap: Option<f32>,
    delay_re: Vec<f32>,
    delay_im: Vec<f32>,
    head: usize,
    delay_len: usize,
}

impl SymmetricComplexFir32 {
    pub fn new(taps: &[f64]) -> Self {
        assert!(!taps.is_empty());

        let taps = taps.iter().map(|&tap| tap as f32).collect::<Vec<_>>();
        for i in 0..taps.len() / 2 {
            let a = taps[i];
            let b = taps[taps.len() - 1 - i];
            assert!(
                (a - b).abs() <= 1e-6,
                "SymmetricComplexFir32 requires mirrored taps"
            );
        }

        let delay_len = taps.len();
        let half_len = delay_len / 2;
        let center_tap = (delay_len % 2 == 1).then(|| taps[half_len]);
        Self {
            half_taps: taps[..half_len].to_vec(),
            center_tap,
            delay_re: vec![0.0; delay_len * 2],
            delay_im: vec![0.0; delay_len * 2],
            head: 1 % delay_len,
            delay_len,
        }
    }

    #[inline]
    pub fn process_sample(&mut self, sample: Complex32) -> Complex32 {
        self.push_sample(sample);
        self.accumulate()
    }

    #[inline]
    pub fn process_sample_if(&mut self, sample: Complex32, emit: bool) -> Option<Complex32> {
        self.push_sample(sample);
        emit.then(|| self.accumulate())
    }

    pub fn process_block(&mut self, input: &[Complex32]) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(input.len());
        self.process_block_into(input, &mut out);
        out
    }

    pub fn process_block_into(&mut self, input: &[Complex32], out: &mut Vec<Complex32>) {
        out.clear();
        out.reserve(input.len());
        for &sample in input {
            out.push(self.process_sample(sample));
        }
    }

    #[inline]
    fn push_sample(&mut self, sample: Complex32) {
        self.head = if self.head == 0 {
            self.delay_len - 1
        } else {
            self.head - 1
        };
        self.delay_re[self.head] = sample.re;
        self.delay_re[self.head + self.delay_len] = sample.re;
        self.delay_im[self.head] = sample.im;
        self.delay_im[self.head + self.delay_len] = sample.im;
    }

    #[inline]
    fn accumulate(&mut self) -> Complex32 {
        let half = self.half_taps.len();
        let head = self.head;
        let last = head + self.delay_len - 1;

        let mut out = symmetric_complex_accumulate(
            &self.half_taps,
            &self.delay_re,
            &self.delay_im,
            head,
            last,
        );
        if let Some(coeff) = self.center_tap {
            out.re += self.delay_re[head + half] * coeff;
            out.im += self.delay_im[head + half] * coeff;
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::{ComplexFir32, Fir32, PolyphaseComplexFir32, SymmetricComplexFir32};
    use num_complex::Complex32;

    #[test]
    fn fir32_preserves_state_across_blocks() {
        let taps = [0.25, 0.5, 0.25];
        let mut one = Fir32::new(&taps);
        let mut split = Fir32::new(&taps);

        let all = one.process_block(&[1.0, 2.0, 3.0, 4.0]);
        let mut pieces = split.process_block(&[1.0, 2.0]);
        pieces.extend(split.process_block(&[3.0, 4.0]));

        assert_eq!(all, pieces);
        assert_eq!(all, vec![0.25, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn complex_fir_matches_real_iq_filtering() {
        let taps = [0.25, 0.5, 0.25];
        let samples = [
            Complex32::new(1.0, -1.0),
            Complex32::new(2.0, -2.0),
            Complex32::new(3.0, -3.0),
        ];
        let mut complex = ComplexFir32::new(&taps);
        let mut real = Fir32::new(&taps);
        let mut imag = Fir32::new(&taps);

        let complex_out = complex.process_block(&samples);
        let real_out = real.process_block(&[1.0, 2.0, 3.0]);
        let imag_out = imag.process_block(&[-1.0, -2.0, -3.0]);

        for (actual, (re, im)) in complex_out.iter().zip(real_out.into_iter().zip(imag_out)) {
            assert_eq!(*actual, Complex32::new(re, im));
        }
    }

    #[test]
    fn interpolation_uses_polyphase_taps() {
        let taps = [1.0, 2.0, 3.0, 4.0];
        let mut fir = Fir32::with_interpolate(&taps, 2);
        assert_eq!(fir.process_block(&[1.0, 10.0]), vec![2.0, 4.0, 26.0, 48.0]);
    }

    #[test]
    fn polyphase_complex_fir_matches_interpolation_reference() {
        let taps = [1.0, 2.0, 3.0, 4.0];
        let mut fir = PolyphaseComplexFir32::with_interpolate(&taps, 2);
        let out = fir.process_block(&[Complex32::new(1.0, -1.0), Complex32::new(10.0, -10.0)]);
        let want = [2.0, 4.0, 26.0, 48.0];
        assert_eq!(out.len(), want.len());
        for (got, want) in out.iter().zip(want) {
            assert_eq!(*got, Complex32::new(want, -want));
        }
    }

    #[test]
    fn polyphase_complex_fir_matches_general_complex_fir() {
        // SIMD reassociation forbids exact equality, so compare with a
        // relative epsilon.
        for (tap_count, interpolate) in [(48usize, 4usize), (23, 2), (12, 1)] {
            let taps = (0..tap_count)
                .map(|n| (n as f64 * 0.37).sin() / (1.0 + n as f64 * 0.11))
                .collect::<Vec<_>>();
            let input = (0..1024)
                .map(|n| {
                    Complex32::new(
                        (n as f32 * 0.113).sin() * 1.5,
                        (n as f32 * 0.071).cos() * 0.8,
                    )
                })
                .collect::<Vec<_>>();

            let mut reference = ComplexFir32::with_interpolate(&taps, interpolate);
            let mut fast = PolyphaseComplexFir32::with_interpolate(&taps, interpolate);

            // Feed in two chunks to exercise delay-ring continuity.
            let (first, second) = input.split_at(400);
            let mut want = reference.process_block(first);
            want.extend(reference.process_block(second));
            let mut got = fast.process_block(first);
            got.extend(fast.process_block(second));

            assert_eq!(got.len(), want.len());
            for (idx, (got, want)) in got.iter().zip(&want).enumerate() {
                let err = (got - want).norm();
                assert!(
                    err <= 1e-5 * (1.0 + want.norm()),
                    "taps={tap_count} interpolate={interpolate} sample {idx}: got {got}, want {want}, err {err}"
                );
            }
        }
    }

    #[test]
    fn symmetric_complex_fir_matches_general_complex_fir() {
        let taps = [0.125, 0.25, 0.375, 0.375, 0.25, 0.125];
        let samples = [
            Complex32::new(1.0, -2.0),
            Complex32::new(0.5, 3.0),
            Complex32::new(-4.0, 0.25),
            Complex32::new(2.5, -1.5),
            Complex32::new(0.0, 0.75),
            Complex32::new(5.0, -0.5),
            Complex32::new(-1.0, 1.0),
        ];
        let mut general = ComplexFir32::new(&taps);
        let mut symmetric = SymmetricComplexFir32::new(&taps);

        assert_eq!(
            general.process_block(&samples),
            symmetric.process_block(&samples)
        );
    }
}
