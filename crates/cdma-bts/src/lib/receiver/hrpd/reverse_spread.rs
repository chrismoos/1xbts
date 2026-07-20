//! HRPD reverse-link quadrature spreading helpers.
//!
//! C.S0024-0 v4.0 §9.2.1.3.8 defines the same quadrature spreading operation
//! for reverse Access and reverse Traffic.  The channel-specific difference is
//! the long-code mask source: access uses `MIACMAC/MQACMAC`; traffic uses
//! `MIRTCMAC/MQRTCMAC`.

use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::spread::HrpdAccessTerminalPnSequence;
use crate::receiver::hrpd::long_code::HRPD_LONG_CODE_INITIAL_STATE;

#[derive(Clone, Copy, Debug)]
pub struct HrpdReversePilotReferenceConfig {
    pub start_chip: u64,
    pub len: usize,
    pub i_mask: u64,
    pub q_mask: u64,
    pub reference_chip_offset: i32,
    pub pn_phase_offset_chips: i32,
    pub lc_phase_offset_chips: i32,
    /// Sample/sign convention for the Q arm. Production HRPD keeps this fixed;
    /// diagnostics may flip it to model SDR I/Q conventions.
    pub q_sign: f32,
    /// Q decimator pair boundary. Per spec the retained value aligns with the
    /// first chip of a slot; since slots are 2048 chips, production uses 0.
    pub q_pair_phase: u64,
}

pub fn hrpd_reverse_pilot_reference_chips(cfg: HrpdReversePilotReferenceConfig) -> Vec<Complex32> {
    if cfg.len == 0 {
        return Vec::new();
    }

    let first_ref_chip = (cfg.start_chip as i64 + i64::from(cfg.reference_chip_offset)).max(0);
    let first_pair_chip = if ((first_ref_chip as u64) & 1) == (cfg.q_pair_phase & 1) {
        first_ref_chip
    } else {
        first_ref_chip.saturating_sub(1)
    };
    let ref_origin = first_ref_chip.min(first_pair_chip).max(0) as u64;
    let last_ref_chip =
        (cfg.start_chip + cfg.len as u64 - 1) as i64 + i64::from(cfg.reference_chip_offset);
    let ref_len = (last_ref_chip.max(0) as u64)
        .saturating_sub(ref_origin)
        .saturating_add(4) as usize;

    let pn_start =
        (ref_origin as i64 + i64::from(cfg.pn_phase_offset_chips)).rem_euclid(32768) as u64;
    let lc_start =
        (ref_origin as i64 + i64::from(cfg.lc_phase_offset_chips)).rem_euclid(32768) as u64;
    let pn = hrpd_reverse_short_pn_signs(pn_start, ref_len);
    let lc_i = hrpd_reverse_long_code_signs_at_phase(cfg.i_mask, lc_start, ref_len);
    let lc_q = hrpd_reverse_long_code_signs_at_phase(cfg.q_mask, lc_start, ref_len);

    let mut out = Vec::with_capacity(cfg.len);
    for k in 0..cfg.len {
        let chip = cfg.start_chip + k as u64;
        let ref_chip = (chip as i64 + i64::from(cfg.reference_chip_offset)).max(0) as u64;
        let ref_idx = ref_chip.saturating_sub(ref_origin) as usize;
        let phase_chip =
            (ref_chip as i64 + i64::from(cfg.pn_phase_offset_chips)).rem_euclid(32768) as u64;
        let pair_ref_chip = if (phase_chip & 1) == (cfg.q_pair_phase & 1) {
            ref_chip
        } else {
            ref_chip.saturating_sub(1)
        };
        let pair_idx = pair_ref_chip.saturating_sub(ref_origin) as usize;
        out.push(hrpd_reverse_pilot_reference_from_signs(
            phase_chip,
            pn[ref_idx].0,
            pn[pair_idx].1,
            lc_i[ref_idx],
            lc_q[pair_idx],
            cfg.q_sign,
            cfg.q_pair_phase,
        ));
    }
    out
}

pub fn hrpd_reverse_pilot_reference_from_signs(
    phase_chip: u64,
    pi: f32,
    pq_dec: f32,
    ui: f32,
    uq_dec: f32,
    q_sign: f32,
    q_pair_phase: u64,
) -> Complex32 {
    // §9.2.1.3.8:
    //   PNI = PI * UI
    //   PNQ = PNI * W12 * Decim2(PQ * UQ)
    // where W12 is (+ -) and the retained Q decimator value aligns with the
    // first chip of a slot.
    let pni = pi * ui;
    let w12 = if (phase_chip & 1) == (q_pair_phase & 1) {
        1.0
    } else {
        -1.0
    };
    let pnq = pni * w12 * q_sign.signum() * pq_dec * uq_dec;
    Complex32::new(pni, pnq)
}

pub fn hrpd_reverse_short_pn_signs(start_chip: u64, len: usize) -> Vec<(f32, f32)> {
    let mut pn = HrpdAccessTerminalPnSequence::new(0, 32768);
    pn.advance_chips(start_chip % 32768);
    (0..len)
        .map(|_| {
            let v = pn.generate_iq();
            (v.re, v.im)
        })
        .collect()
}

pub fn hrpd_reverse_long_code_signs_at_phase(mask: u64, start_chip: u64, len: usize) -> Vec<f32> {
    let mut lc = LongCodeGenerator::new(mask);
    lc.set_state(HRPD_LONG_CODE_INITIAL_STATE);
    let mut phase = (start_chip % 32768) as usize;
    lc.advance_chips(phase);
    let mut out = Vec::with_capacity(len);
    for idx in 0..len {
        if idx > 0 && phase == 0 {
            lc.set_state(HRPD_LONG_CODE_INITIAL_STATE);
        }
        out.push(if lc.next_chip() == 1 { -1.0 } else { 1.0 });
        phase = (phase + 1) & 0x7fff;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receiver::hrpd::long_code::{HrpdAccessLongCodeMask, derive_q_mask};

    #[test]
    fn pilot_reference_matches_spec_pnq_construction() {
        let i_mask = HrpdAccessLongCodeMask {
            access_cycle_number: 128,
            sector_id_lsb: 0,
            color_code: 26,
        }
        .to_mask();
        let q_mask = derive_q_mask(i_mask);
        let chips = hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
            start_chip: 2048,
            len: 64,
            i_mask,
            q_mask,
            reference_chip_offset: 0,
            pn_phase_offset_chips: 0,
            lc_phase_offset_chips: 0,
            q_sign: -1.0,
            q_pair_phase: 0,
        });
        assert_eq!(chips.len(), 64);
        assert!(chips.iter().all(|c| c.re.abs() == 1.0 && c.im.abs() == 1.0));
        for pair in chips.chunks_exact(2) {
            assert_eq!(pair[0].im / pair[0].re, -pair[1].im / pair[1].re);
        }
    }
}
