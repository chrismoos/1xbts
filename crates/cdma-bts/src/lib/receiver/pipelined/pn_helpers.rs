use num_complex::Complex32;
use sdr::FIR;

use crate::phy::spread::PnSequence;
use crate::sdr::cdma2000_baseband_filter_taps_f64;

use super::CDMA_CHIP_RATE;

/// Compute the number of chip-rate chips per output sample given the output
/// sample rate. Returns 1 if sample_rate_hz is zero or negative.
pub fn chips_per_sample(sample_rate_hz: f64) -> usize {
    if sample_rate_hz > 0.0 {
        (CDMA_CHIP_RATE / sample_rate_hz).round() as usize
    } else {
        1
    }
}

pub(crate) fn build_fft_search_pn_samples(output_len: usize, oversample: usize) -> Vec<Complex32> {
    let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
    (0..output_len).map(|_| pn.generate_iq()).collect()
}

pub(crate) fn build_oqpsk_pn_samples(output_len: usize, oversample: usize) -> Vec<Complex32> {
    assert_eq!(
        0,
        oversample % 2,
        "OQPSK half-chip delay requires even oversample"
    );
    let q_delay_samples = oversample / 2;
    let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
    let mut pn_i = Vec::with_capacity(output_len);
    let mut pn_q = Vec::with_capacity(output_len);
    for _ in 0..output_len {
        let s = pn.generate_iq();
        pn_i.push(s.re);
        pn_q.push(s.im);
    }

    (0..output_len)
        .map(|k| {
            let q_idx = k.saturating_sub(q_delay_samples);
            Complex32::new(pn_i[k], pn_q[q_idx])
        })
        .collect()
}

pub(crate) fn build_matched_pn_reference(
    output_len: usize,
    oversample: usize,
    filter_passes: usize,
) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    let mut ref_matched_i = (0..filter_passes)
        .map(|_| FIR::new(&taps, 1, 1))
        .collect::<Vec<_>>();
    let mut ref_matched_q = (0..filter_passes)
        .map(|_| FIR::new(&taps, 1, 1))
        .collect::<Vec<_>>();
    let pn_oqpsk = build_oqpsk_pn_samples(output_len, oversample);

    // Forward-link PN convention is PN_I - j*PN_Q.
    pn_oqpsk
        .into_iter()
        .map(|s| {
            let mut i = s.re;
            let mut q = -s.im;
            for filter in &mut ref_matched_i {
                i = filter.process(&[i])[0];
            }
            for filter in &mut ref_matched_q {
                q = filter.process(&[q])[0];
            }
            Complex32::new(i, q)
        })
        .collect()
}
