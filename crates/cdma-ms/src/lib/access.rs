//! Access Channel procedures from C.S0003-E §2.2.1.1.2.2.3, C.S0004-E §2.1.1.2.2, and C.S0002-E §2.1.2.3.1.1.

use cdma_common::band_class::BandClass;

use crate::ms::AccessParameters;

pub const CHIPS_PER_FRAME: u64 = 24_576;
/// Chips per 80 ms, the unit of ACC_TMO.
pub const CHIPS_PER_80MS: u64 = 98_304;
pub const CHIP_RATE_HZ: f64 = 1_228_800.0;
/// Largest mean output power a class III mobile transmits (C.S0057).
pub const MAX_OUTPUT_POWER_DBM: f32 = 23.0;
pub const MIN_OUTPUT_POWER_DBM: f32 = -50.0;
const PSIST_DENIED: u8 = 63;
/// Multiplier of the C.S0005-E §2.6.9 pseudorandom number generator.
const PRNG_MULTIPLIER: u64 = 16_807;
const PRNG_MODULUS: u64 = 2_147_483_647;
/// Hash multiplier of C.S0003-E §2.2.1.1.2.2.3.4.
const HASH_MULTIPLIER: u32 = 40_503;
/// DECORR for Access Channel PN randomization is 14 times the low 12 bits of
/// the hash key (C.S0003-E Table 2-40).
const PN_RANDOMIZATION_DECORR_FACTOR: u32 = 14;
const NOM_PWR_EXT_DB: f32 = 16.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeParams {
    pub nom_pwr: i8,
    pub nom_pwr_ext: u8,
    pub init_pwr: i8,
    pub pwr_step: u8,
    pub num_step: u8,
    pub pam_sz: u8,
    pub max_cap_sz: u8,
    pub acc_chan: u8,
    pub probe_pn_ran: u8,
    pub acc_tmo: u8,
    pub probe_bkoff: u8,
    pub bkoff: u8,
    pub max_req_seq: u8,
    pub max_rsp_seq: u8,
    pub psist: u8,
    pub msg_psist: u8,
    pub reg_psist: u8,
}

impl From<&AccessParameters> for ProbeParams {
    fn from(a: &AccessParameters) -> Self {
        ProbeParams {
            nom_pwr: a.nom_pwr,
            nom_pwr_ext: a.nom_pwr_ext,
            init_pwr: a.init_pwr,
            pwr_step: a.pwr_step,
            num_step: a.num_step,
            pam_sz: a.pam_sz,
            max_cap_sz: a.max_cap_sz,
            acc_chan: a.acc_chan,
            probe_pn_ran: a.probe_pn_ran,
            acc_tmo: a.acc_tmo,
            probe_bkoff: a.probe_bkoff,
            bkoff: a.bkoff,
            max_req_seq: a.max_req_seq,
            max_rsp_seq: a.max_rsp_seq,
            psist: a.psist_0_9,
            msg_psist: a.msg_psist,
            reg_psist: a.reg_psist,
        }
    }
}

impl ProbeParams {
    pub fn slot_frames(&self) -> u64 {
        4 + self.max_cap_sz as u64 + self.pam_sz as u64
    }

    pub fn slot_chips(&self) -> u64 {
        self.slot_frames() * CHIPS_PER_FRAME
    }

    pub fn preamble_frames(&self) -> usize {
        1 + self.pam_sz as usize
    }

    pub fn max_capsule_frames(&self) -> usize {
        3 + self.max_cap_sz as usize
    }

    /// TA, the acknowledgment timeout: (2 + ACC_TMO) × 80 ms.
    pub fn ack_timeout_chips(&self) -> u64 {
        (2 + self.acc_tmo as u64) * CHIPS_PER_80MS
    }

    pub fn slot_start_at_or_after(&self, chip: u64) -> u64 {
        let slot = self.slot_chips();
        chip.div_ceil(slot) * slot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Registration,
    Origination,
    Message,
    Response,
    /// An unassured acknowledgment order: one probe, no acknowledgment
    /// expected (C.S0004-E §2.1.1.2.2, unassured transmission).
    Acknowledgment,
}

impl AccessKind {
    pub fn label(self) -> &'static str {
        match self {
            AccessKind::Registration => "registration",
            AccessKind::Origination => "origination",
            AccessKind::Message => "message",
            AccessKind::Response => "response",
            AccessKind::Acknowledgment => "acknowledgment",
        }
    }

    fn is_unassured(self) -> bool {
        matches!(self, AccessKind::Acknowledgment)
    }

    fn is_request(self) -> bool {
        matches!(
            self,
            AccessKind::Registration | AccessKind::Origination | AccessKind::Message
        )
    }

    /// The persistence probability P (C.S0004-E §2.1.1.2.2.1.1). Responses
    /// are not subject to the persistence test.
    pub fn persistence(self, p: &ProbeParams) -> f64 {
        if !self.is_request() {
            return 1.0;
        }
        if p.psist == PSIST_DENIED {
            return 0.0;
        }
        let base = 2f64.powf(-(p.psist as f64) / 4.0);
        let modifier = match self {
            AccessKind::Registration => p.reg_psist,
            AccessKind::Message => p.msg_psist,
            _ => 0,
        };
        base * 2f64.powi(-(modifier as i32))
    }

    fn max_sequences(self, p: &ProbeParams) -> u32 {
        if self.is_request() {
            p.max_req_seq as u32
        } else {
            p.max_rsp_seq as u32
        }
    }
}

/// The C.S0005-E §2.6.9 multiplicative congruential generator.
#[derive(Debug, Clone)]
pub struct SpecRng {
    z: u64,
}

impl SpecRng {
    pub fn new(seed: u64) -> Self {
        let z = seed % PRNG_MODULUS;
        SpecRng {
            z: if z == 0 { 1 } else { z },
        }
    }

    pub fn next_unit(&mut self) -> f64 {
        self.z = (PRNG_MULTIPLIER * self.z) % PRNG_MODULUS;
        self.z as f64 / PRNG_MODULUS as f64
    }

    pub fn next_up_to(&mut self, max: u32) -> u32 {
        ((self.next_unit() * (max as f64 + 1.0)) as u32).min(max)
    }
}

/// The hash function of C.S0003-E §2.2.1.1.2.2.3.4 over `n` resources.
pub fn hash(hash_key: u32, n: u32, decorr: u16) -> u32 {
    let l = hash_key & 0xffff;
    let h = (hash_key >> 16) & 0xffff;
    let mixed = (HASH_MULTIPLIER.wrapping_mul(l ^ h ^ decorr as u32)) & 0xffff;
    ((n as u64 * mixed as u64) >> 16) as u32
}

/// RN, the PN randomization delay in chips for this mobile (C.S0003-E
/// §2.2.1.1.2.2.3).
pub fn pn_randomization_chips(esn: u32, probe_pn_ran: u8) -> u64 {
    let n = 1u32 << probe_pn_ran.min(9);
    let decorr = (PN_RANDOMIZATION_DECORR_FACTOR * (esn & 0x0fff)) as u16;
    hash(esn, n, decorr) as u64
}

/// The offset power term of the open-loop estimate for a band class
/// (C.S0057 Table 2.3.1-1): -73 dBm for the cellular bands and -76 dBm for
/// the PCS-like bands.
pub fn band_offset_power_dbm(band_class: BandClass) -> f32 {
    match band_class {
        BandClass::Bc1
        | BandClass::Bc4
        | BandClass::Bc6
        | BandClass::Bc8
        | BandClass::Bc14
        | BandClass::Bc15 => -76.0,
        _ => -73.0,
    }
}

/// C.S0002-E §2.1.2.3.1.1. NaN Ec/Io disables interference correction.
pub fn open_loop_probe_power_dbm(
    rx_input_dbm: f32,
    ec_io_db: f32,
    band_offset_dbm: f32,
    p: &ProbeParams,
    pwr_lvl: u8,
) -> f32 {
    let interference_correction = if ec_io_db.is_nan() {
        0.0
    } else {
        (-7.0 - ec_io_db).clamp(0.0, 7.0)
    };
    let power = -rx_input_dbm
        + band_offset_dbm
        + interference_correction
        + access_probe_correction_db(p, pwr_lvl);
    power.clamp(MIN_OUTPUT_POWER_DBM, MAX_OUTPUT_POWER_DBM)
}

pub fn access_probe_correction_db(p: &ProbeParams, pwr_lvl: u8) -> f32 {
    p.nom_pwr as f32 - NOM_PWR_EXT_DB * p.nom_pwr_ext as f32
        + p.init_pwr as f32
        + pwr_lvl as f32 * p.pwr_step as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessFailure {
    MaxProbes,
    AccessDenied,
    CapsuleTooLong,
    Cancelled,
}

impl AccessFailure {
    pub fn label(self) -> &'static str {
        match self {
            AccessFailure::MaxProbes => "max_probes",
            AccessFailure::AccessDenied => "access_denied",
            AccessFailure::CapsuleTooLong => "capsule_too_long",
            AccessFailure::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessOutcome {
    Acknowledged,
    Failed(AccessFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    WaitSlot { earliest: u64, persist: bool },
    Transmitting { ends: u64 },
    AwaitAck { until: u64 },
    Done(AccessOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbePlan {
    /// System time chip of the first preamble chip (slot start + RN).
    pub start_chip: u64,
    /// System time chip for the access PN and long-code phase.
    pub code_start_chip: u64,
    /// System time chip after the last capsule chip.
    pub end_chip: u64,
    pub sequence: u32,
    pub pwr_lvl: u8,
    pub preamble_frames: usize,
    pub acn: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessStatus {
    pub kind: &'static str,
    pub phase: &'static str,
    pub probes_sent: u32,
    pub sequence: u32,
    pub pwr_lvl: u8,
    pub rn_chips: u64,
    pub outcome: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct AccessAttempt {
    kind: AccessKind,
    params: ProbeParams,
    capsule_frames: usize,
    rn_chips: u64,
    acn: u8,
    probes_sent: u32,
    phase: Phase,
    rng: SpecRng,
}

impl AccessAttempt {
    pub fn new(
        kind: AccessKind,
        params: ProbeParams,
        capsule_frames: usize,
        esn: u32,
        now: u64,
    ) -> Self {
        let mut rng = SpecRng::new((esn as u64) ^ now);
        let rn_chips = pn_randomization_chips(esn, params.probe_pn_ran);
        let acn = rng.next_up_to(params.acc_chan as u32) as u8;
        let phase = if capsule_frames > params.max_capsule_frames() {
            Phase::Done(AccessOutcome::Failed(AccessFailure::CapsuleTooLong))
        } else if kind.persistence(&params) == 0.0 {
            Phase::Done(AccessOutcome::Failed(AccessFailure::AccessDenied))
        } else {
            Phase::WaitSlot {
                earliest: now,
                persist: kind.is_request(),
            }
        };
        AccessAttempt {
            kind,
            params,
            capsule_frames,
            rn_chips,
            acn,
            probes_sent: 0,
            phase,
            rng,
        }
    }

    pub fn kind(&self) -> AccessKind {
        self.kind
    }

    pub fn outcome(&self) -> Option<AccessOutcome> {
        match self.phase {
            Phase::Done(o) => Some(o),
            _ => None,
        }
    }

    pub fn is_done(&self) -> bool {
        self.outcome().is_some()
    }

    fn probes_per_sequence(&self) -> u32 {
        self.params.num_step as u32 + 1
    }

    pub fn poll(&mut self, transmit_now_chip: u64, lead_chips: u64) -> Option<ProbePlan> {
        match self.phase {
            Phase::WaitSlot { earliest, persist } => {
                let slot = self
                    .params
                    .slot_start_at_or_after(earliest.max(transmit_now_chip + lead_chips));
                if persist {
                    let p = self.kind.persistence(&self.params);
                    if self.rng.next_unit() >= p {
                        self.phase = Phase::WaitSlot {
                            earliest: slot + self.params.slot_chips(),
                            persist,
                        };
                        return None;
                    }
                }
                let seq_len = self.probes_per_sequence();
                let pwr_lvl = (self.probes_sent % seq_len) as u8;
                let sequence = self.probes_sent / seq_len;
                let start_chip = slot + self.rn_chips;
                let frames = self.params.preamble_frames() + self.capsule_frames;
                let end_chip = start_chip + frames as u64 * CHIPS_PER_FRAME;
                if self.kind.is_unassured() {
                    self.phase = Phase::Done(AccessOutcome::Acknowledged);
                } else {
                    self.phase = Phase::Transmitting { ends: end_chip };
                }
                self.probes_sent += 1;
                Some(ProbePlan {
                    start_chip,
                    code_start_chip: slot,
                    end_chip,
                    sequence,
                    pwr_lvl,
                    preamble_frames: self.params.preamble_frames(),
                    acn: self.acn,
                })
            }
            Phase::Transmitting { ends } => {
                if transmit_now_chip >= ends {
                    self.phase = Phase::AwaitAck {
                        until: ends + self.params.ack_timeout_chips(),
                    };
                }
                None
            }
            Phase::AwaitAck { until } => {
                // TA uses air time. Waiting for RX decode latency would miss the next probe slot.
                if transmit_now_chip >= until {
                    self.schedule_next_probe(until);
                }
                None
            }
            Phase::Done(_) => None,
        }
    }

    fn schedule_next_probe(&mut self, from: u64) {
        let seq_len = self.probes_per_sequence();
        let slot = self.params.slot_chips();
        if self.probes_sent % seq_len != 0 {
            // Next probe of this sequence after RT slots, uniform over 0 to
            // PROBE_BKOFF. A cell that sends 0 is asking for no backoff at
            // all, so the range has to include 0 and stop at what it sent.
            let rt = self.rng.next_up_to(self.params.probe_bkoff as u32) as u64;
            self.phase = Phase::WaitSlot {
                earliest: from + rt * slot,
                persist: false,
            };
            return;
        }
        let sequences_done = self.probes_sent / seq_len;
        if sequences_done >= self.kind.max_sequences(&self.params) {
            self.phase = Phase::Done(AccessOutcome::Failed(AccessFailure::MaxProbes));
            return;
        }
        let rs = self.rng.next_up_to(self.params.bkoff as u32) as u64;
        self.acn = self.rng.next_up_to(self.params.acc_chan as u32) as u8;
        self.phase = Phase::WaitSlot {
            earliest: from + rs * slot,
            persist: self.kind.is_request(),
        };
    }

    pub fn staged_probe(&self, transmit_now_chip: u64, lead_chips: u64) -> Option<ProbePlan> {
        let ends = match self.phase {
            Phase::Transmitting { ends } => ends,
            Phase::AwaitAck { until } => until - self.params.ack_timeout_chips(),
            _ => return None,
        };
        let seq_len = self.probes_per_sequence();
        if self.probes_sent % seq_len == 0 {
            return None;
        }
        let earliest = ends + self.params.ack_timeout_chips();
        let slot = self
            .params
            .slot_start_at_or_after(earliest.max(transmit_now_chip + lead_chips));
        let start_chip = slot + self.rn_chips;
        let frames = self.params.preamble_frames() + self.capsule_frames;
        Some(ProbePlan {
            start_chip,
            code_start_chip: slot,
            end_chip: start_chip + frames as u64 * CHIPS_PER_FRAME,
            sequence: self.probes_sent / seq_len,
            pwr_lvl: (self.probes_sent % seq_len) as u8,
            preamble_frames: self.params.preamble_frames(),
            acn: self.acn,
        })
    }

    pub fn on_ack(&mut self) {
        if !self.is_done() {
            self.phase = Phase::Done(AccessOutcome::Acknowledged);
        }
    }

    pub fn cancel(&mut self) {
        if !self.is_done() {
            self.phase = Phase::Done(AccessOutcome::Failed(AccessFailure::Cancelled));
        }
    }

    pub fn awaiting_ack(&self) -> bool {
        matches!(
            self.phase,
            Phase::Transmitting { .. } | Phase::AwaitAck { .. }
        ) || (self.probes_sent > 0 && matches!(self.phase, Phase::WaitSlot { .. }))
    }

    pub fn probes_sent(&self) -> u32 {
        self.probes_sent
    }

    /// PWR_LVL of the last probe sent, which carries into the traffic
    /// channel's initial power.
    pub fn last_pwr_lvl(&self) -> u8 {
        if self.probes_sent == 0 {
            0
        } else {
            ((self.probes_sent - 1) % self.probes_per_sequence()) as u8
        }
    }

    pub fn status(&self) -> AccessStatus {
        let phase = match self.phase {
            Phase::WaitSlot { .. } => "waiting_slot",
            Phase::Transmitting { .. } => "transmitting",
            Phase::AwaitAck { .. } => "awaiting_ack",
            Phase::Done(_) => "done",
        };
        AccessStatus {
            kind: self.kind.label(),
            phase,
            probes_sent: self.probes_sent,
            sequence: self.probes_sent / self.probes_per_sequence(),
            pwr_lvl: self.last_pwr_lvl(),
            rn_chips: self.rn_chips,
            outcome: self.outcome().map(|o| match o {
                AccessOutcome::Acknowledged => "acknowledged",
                AccessOutcome::Failed(f) => f.label(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ProbeParams {
        ProbeParams {
            nom_pwr: 0,
            nom_pwr_ext: 0,
            init_pwr: 0,
            pwr_step: 1,
            num_step: 3,
            pam_sz: 2,
            max_cap_sz: 3,
            acc_chan: 0,
            probe_pn_ran: 4,
            acc_tmo: 1,
            probe_bkoff: 0,
            bkoff: 0,
            max_req_seq: 2,
            max_rsp_seq: 2,
            psist: 0,
            msg_psist: 0,
            reg_psist: 0,
        }
    }

    #[test]
    fn hash_matches_the_spec_formula() {
        let key = 0x1234_5678;
        let l = 0x5678u32;
        let h = 0x1234u32;
        let expected = ((512u64 * ((40503u32.wrapping_mul(l ^ h) & 0xffff) as u64)) >> 16) as u32;
        assert_eq!(hash(key, 512, 0), expected);
        assert!(pn_randomization_chips(key, 9) < 512);
        assert_eq!(pn_randomization_chips(key, 0), 0);
    }

    #[test]
    fn probes_ramp_within_a_sequence_and_land_on_slots() {
        let p = params();
        let esn = 0x00ab_cdef;
        let mut a = AccessAttempt::new(AccessKind::Registration, p.clone(), 1, esn, 1_000);
        let lead = 4 * CHIPS_PER_FRAME;
        let first = a.poll(1_000, lead).expect("first probe");
        assert_eq!(first.pwr_lvl, 0);
        assert_eq!(first.sequence, 0);
        assert_eq!(first.code_start_chip % p.slot_chips(), 0);
        assert_eq!(first.start_chip - first.code_start_chip, a.rn_chips);
        assert!(first.start_chip >= 1_000 + lead);
        assert_eq!(
            first.end_chip - first.start_chip,
            (p.preamble_frames() as u64 + 1) * CHIPS_PER_FRAME
        );
        assert!(a.poll(first.start_chip, lead).is_none());
        assert!(a.poll(first.end_chip, lead).is_none());
        let ta_end = first.end_chip + p.ack_timeout_chips();
        assert!(a.poll(ta_end - 1, lead).is_none());
        assert!(a.poll(ta_end, lead).is_none());
        let second = a.poll(ta_end + 1, lead).expect("second probe");
        assert_eq!(second.pwr_lvl, 1);
        assert!(second.start_chip >= ta_end);
        assert!(!a.is_done());
    }

    #[test]
    fn retry_follows_the_transmit_clock_not_the_decode_position() {
        let p = params();
        let lead = CHIPS_PER_FRAME;
        let mut a = AccessAttempt::new(AccessKind::Registration, p.clone(), 1, 7, 0);
        let first = a.poll(0, lead).expect("first probe");
        let ta_end = first.end_chip + p.ack_timeout_chips();
        assert!(a.poll(ta_end - 1, lead).is_none());
        assert!(a.poll(ta_end, lead).is_none());
        let second = a.poll(ta_end, lead).expect("probe once TA has run out");
        assert_eq!(second.pwr_lvl, 1);
        assert!(second.start_chip >= ta_end);
        a.on_ack();
        assert_eq!(a.outcome(), Some(AccessOutcome::Acknowledged));
    }

    #[test]
    fn attempt_ends_on_ack_and_after_the_probe_budget() {
        let p = params();
        let mut a = AccessAttempt::new(AccessKind::Response, p.clone(), 1, 7, 0);
        let lead = CHIPS_PER_FRAME;
        let plan = a.poll(0, lead).expect("probe");
        a.poll(plan.end_chip, lead);
        a.on_ack();
        assert_eq!(a.outcome(), Some(AccessOutcome::Acknowledged));

        let mut b = AccessAttempt::new(AccessKind::Response, p.clone(), 1, 7, 0);
        let mut now = 0;
        let mut probes = 0;
        for _ in 0..1_000 {
            if let Some(plan) = b.poll(now, lead) {
                probes += 1;
                now = plan.end_chip;
            }
            now += CHIPS_PER_FRAME;
            if b.is_done() {
                break;
            }
        }
        assert_eq!(probes, (p.num_step as u32 + 1) * p.max_rsp_seq as u32);
        assert_eq!(
            b.outcome(),
            Some(AccessOutcome::Failed(AccessFailure::MaxProbes))
        );
    }

    #[test]
    fn persistence_denies_or_defers() {
        let mut p = params();
        p.psist = PSIST_DENIED;
        let a = AccessAttempt::new(AccessKind::Origination, p.clone(), 1, 7, 0);
        assert_eq!(
            a.outcome(),
            Some(AccessOutcome::Failed(AccessFailure::AccessDenied))
        );
        let r = AccessAttempt::new(AccessKind::Response, p.clone(), 1, 7, 0);
        assert!(!r.is_done());
        assert_eq!(AccessKind::Response.persistence(&p), 1.0);
        p.psist = 8;
        p.reg_psist = 1;
        let base = 2f64.powf(-2.0);
        assert!((AccessKind::Origination.persistence(&p) - base).abs() < 1e-9);
        assert!((AccessKind::Registration.persistence(&p) - base / 2.0).abs() < 1e-9);
    }

    #[test]
    fn open_loop_power_follows_the_formula() {
        let mut p = params();
        let power = open_loop_probe_power_dbm(-80.0, -3.0, -73.0, &p, 2);
        assert!((power - (80.0 - 73.0 + 0.0 + 2.0)).abs() < 1e-6);
        let weak = open_loop_probe_power_dbm(-80.0, -12.0, -73.0, &p, 0);
        assert!((weak - (80.0 - 73.0 + 5.0)).abs() < 1e-6);
        let clamped = open_loop_probe_power_dbm(-120.0, f32::NAN, -73.0, &p, 0);
        assert_eq!(clamped, MAX_OUTPUT_POWER_DBM);

        p.nom_pwr = 3;
        p.nom_pwr_ext = 1;
        p.init_pwr = -4;
        assert_eq!(access_probe_correction_db(&p, 2), -15.0);
    }

    #[test]
    fn capsule_longer_than_the_slot_allows_fails_at_once() {
        let p = params();
        let a = AccessAttempt::new(AccessKind::Message, p.clone(), 7, 7, 0);
        assert_eq!(
            a.outcome(),
            Some(AccessOutcome::Failed(AccessFailure::CapsuleTooLong))
        );
    }
}
