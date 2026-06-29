use num_complex::Complex32;

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

/// Stateful complex FIR optimized for symmetric, non-interpolating real taps.
///
/// For the CDMA2000 matched filter the 48 real coefficients are mirrored, so
/// each output can be computed as `tap[i] * (x[n-i] + x[n-(N-1-i)])`. This
/// halves the number of coefficient multiplies while preserving the exact FIR
/// response.
pub struct SymmetricComplexFir32 {
    taps: Vec<f32>,
    center_tap: Option<f32>,
    delay: Vec<ComplexDelay>,
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
            taps: taps[..half_len].to_vec(),
            center_tap,
            delay: vec![ComplexDelay::default(); delay_len * 2],
            head: 1 % delay_len,
            delay_len,
        }
    }

    #[inline]
    pub fn process_sample(&mut self, sample: Complex32) -> Complex32 {
        self.push_sample(sample);
        self.accumulate()
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
        let sample = ComplexDelay {
            re: sample.re,
            im: sample.im,
        };
        self.delay[self.head] = sample;
        self.delay[self.head + self.delay_len] = sample;
    }

    #[inline]
    fn accumulate(&self) -> Complex32 {
        debug_assert!(self.head + self.delay_len <= self.delay.len());

        let mut re = 0.0f32;
        let mut im = 0.0f32;
        let first = unsafe { self.delay.as_ptr().add(self.head) };
        let last = unsafe { self.delay.as_ptr().add(self.head + self.delay_len - 1) };
        for i in 0..self.taps.len() {
            unsafe {
                let a = *first.add(i);
                let b = *last.sub(i);
                let coeff = *self.taps.as_ptr().add(i);
                re += (a.re + b.re) * coeff;
                im += (a.im + b.im) * coeff;
            }
        }

        if let Some(coeff) = self.center_tap {
            unsafe {
                let sample = *first.add(self.taps.len());
                re += sample.re * coeff;
                im += sample.im * coeff;
            }
        }

        Complex32::new(re, im)
    }
}

#[cfg(test)]
mod tests {
    use super::{ComplexFir32, Fir32, SymmetricComplexFir32};
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
