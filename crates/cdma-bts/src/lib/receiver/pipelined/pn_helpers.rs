use num_complex::Complex32;

use crate::phy::spread::{HrpdAccessTerminalPnSequence, PnChipSource, PnSequence};
use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortCodeReferenceKind {
    Cdma2000,
    HrpdAccessTerminal,
}

pub(crate) fn build_fft_search_pn_samples(output_len: usize, oversample: usize) -> Vec<Complex32> {
    build_fft_search_pn_samples_with_kind(output_len, oversample, ShortCodeReferenceKind::Cdma2000)
}

pub(crate) fn build_fft_search_pn_samples_with_kind(
    output_len: usize,
    oversample: usize,
    kind: ShortCodeReferenceKind,
) -> Vec<Complex32> {
    match kind {
        ShortCodeReferenceKind::Cdma2000 => build_pn_samples(
            PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1)),
            output_len,
        ),
        ShortCodeReferenceKind::HrpdAccessTerminal => build_pn_samples(
            HrpdAccessTerminalPnSequence::new_repeat(0, 32768, oversample.saturating_sub(1)),
            output_len,
        ),
    }
}

pub(crate) fn build_oqpsk_pn_samples(output_len: usize, oversample: usize) -> Vec<Complex32> {
    build_oqpsk_pn_samples_with_kind(output_len, oversample, ShortCodeReferenceKind::Cdma2000)
}

pub(crate) fn build_oqpsk_pn_samples_with_kind(
    output_len: usize,
    oversample: usize,
    kind: ShortCodeReferenceKind,
) -> Vec<Complex32> {
    assert_eq!(
        0,
        oversample % 2,
        "OQPSK half-chip delay requires even oversample"
    );
    let q_delay_samples = oversample / 2;
    let pn = build_fft_search_pn_samples_with_kind(output_len, oversample, kind);
    let pn_i = pn.iter().map(|s| s.re).collect::<Vec<_>>();
    let pn_q = pn.iter().map(|s| s.im).collect::<Vec<_>>();

    (0..output_len)
        .map(|k| {
            let q_idx = k.saturating_sub(q_delay_samples);
            Complex32::new(pn_i[k], pn_q[q_idx])
        })
        .collect()
}

fn build_pn_samples<P: PnChipSource>(mut pn: P, output_len: usize) -> Vec<Complex32> {
    (0..output_len).map(|_| pn.generate_iq()).collect()
}

pub(crate) fn build_matched_pn_reference(
    output_len: usize,
    oversample: usize,
    filter_passes: usize,
) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    let mut ref_matched = (0..filter_passes)
        .map(|_| ComplexFir32::new(&taps))
        .collect::<Vec<_>>();
    let pn_oqpsk = build_oqpsk_pn_samples(output_len, oversample);

    // Forward-link PN convention is PN_I - j*PN_Q.
    pn_oqpsk
        .into_iter()
        .map(|s| {
            let mut sample = Complex32::new(s.re, -s.im);
            for filter in &mut ref_matched {
                sample = filter.process_sample(sample);
            }
            sample
        })
        .collect()
}
