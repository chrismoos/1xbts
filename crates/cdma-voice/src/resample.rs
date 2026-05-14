//! Minimal linear-interpolation resampler for ringtone preencoding.
//!
//! Good enough for short looped ringtones — we don't need a high-quality
//! sinc resampler here. Mono i16 in, mono i16 out.

/// Resample mono `input` from `src_rate` to `dst_rate` using linear
/// interpolation. Returns the output sample buffer.
pub fn resample_linear_mono(input: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let out_len = ((input.len() as u64) * (dst_rate as u64) / (src_rate as u64)) as usize;
    let mut out = Vec::with_capacity(out_len);
    let ratio = src_rate as f64 / dst_rate as f64;
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = input[idx.min(input.len() - 1)] as f64;
        let b = input[(idx + 1).min(input.len() - 1)] as f64;
        let v = a + (b - a) * frac;
        out.push(v.clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_rates_match() {
        let input = vec![1i16, 2, 3, 4];
        assert_eq!(resample_linear_mono(&input, 8000, 8000), input);
    }

    #[test]
    fn downsample_44100_to_8000_length() {
        let input = vec![0i16; 44100]; // 1 second
        let out = resample_linear_mono(&input, 44100, 8000);
        assert_eq!(out.len(), 8000);
    }

    #[test]
    fn upsample_8000_to_16000_length() {
        let input = vec![0i16; 800]; // 100 ms
        let out = resample_linear_mono(&input, 8000, 16000);
        assert_eq!(out.len(), 1600);
    }

    #[test]
    fn empty_input() {
        let out = resample_linear_mono(&[], 44100, 8000);
        assert!(out.is_empty());
    }
}
