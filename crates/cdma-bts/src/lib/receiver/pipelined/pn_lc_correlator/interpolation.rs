use num_complex::Complex32;

pub(super) fn catmull_rom_f32(p0: f32, p1: f32, p2: f32, p3: f32, mu: f32) -> f32 {
    let mu2 = mu * mu;
    let mu3 = mu2 * mu;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * mu
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * mu2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * mu3)
}

pub(super) fn interp_complex_wrapped(samples: &[Complex32], t: f32) -> Complex32 {
    let len = samples.len();
    debug_assert!(len > 0);
    let wrapped = t.rem_euclid(len as f32);
    let i1 = wrapped.floor() as usize % len;
    let mu = wrapped - wrapped.floor();
    let i0 = (i1 + len - 1) % len;
    let i2 = (i1 + 1) % len;
    let i3 = (i1 + 2) % len;
    Complex32::new(
        catmull_rom_f32(
            samples[i0].re,
            samples[i1].re,
            samples[i2].re,
            samples[i3].re,
            mu,
        ),
        catmull_rom_f32(
            samples[i0].im,
            samples[i1].im,
            samples[i2].im,
            samples[i3].im,
            mu,
        ),
    )
}

pub(super) fn interp_complex_contiguous(samples: &[Complex32], t: f32) -> Option<Complex32> {
    if samples.is_empty() || t < 0.0 || t >= samples.len() as f32 {
        return None;
    }
    let i1 = t.floor() as usize;
    let mu = t - t.floor();
    if mu <= f32::EPSILON {
        return samples.get(i1).copied();
    }
    if i1 == 0 || i1 + 2 >= samples.len() {
        let s1 = samples[i1];
        let s2 = samples.get(i1 + 1).copied().unwrap_or(s1);
        return Some(s1 + (s2 - s1) * mu);
    }
    let i0 = i1 - 1;
    let i2 = i1 + 1;
    let i3 = i1 + 2;
    Some(Complex32::new(
        catmull_rom_f32(
            samples[i0].re,
            samples[i1].re,
            samples[i2].re,
            samples[i3].re,
            mu,
        ),
        catmull_rom_f32(
            samples[i0].im,
            samples[i1].im,
            samples[i2].im,
            samples[i3].im,
            mu,
        ),
    ))
}

pub(super) fn interp_complex_clamped(samples: &[Complex32], t: f32) -> Complex32 {
    debug_assert!(!samples.is_empty());
    if t <= 0.0 {
        return samples[0];
    }
    let last = samples.len() - 1;
    if t >= last as f32 {
        return samples[last];
    }
    interp_complex_contiguous(samples, t).unwrap_or(samples[last])
}
