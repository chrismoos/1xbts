//! HRPD Forward MAC channel encoder.
//!
//! C.S0024-200-C §1.4.1.3.2.2 / §2.4.1.3.2.2. Per slot the MAC channel carries:
//! - **RA** (Reverse Activity), 1 bit broadcast on MACIndex 4.
//! - **RPC** and **DRCLock**, on the MAC index's reserved Walsh row.
//!
//! Default/subtype-1 physical uses 64-ary Walsh covers repeated four times
//! per slot. Subtype 2+ uses 128-ary Walsh covers repeated twice per slot.
//! The chip output is the sum of every contributor scaled by
//! `1/sqrt(N_active)` so the per-chip variance stays unit-bounded; caller can
//! renormalize at the slot modulator if needed.
//!
//! This encoder supports the RA broadcast bit and the currently-active ATs.
//! For default/subtype-1, RPC is transmitted in every slot except the
//! DRCLockPeriod puncture slot. For subtype 2+, RPC and DRCLock are
//! transmitted together in slots satisfying `(T - FrameOffset) mod 4 = 3`.

use num::complex::Complex32;
use std::sync::Arc;

use crate::phy::walsh::WalshGenerator;

use super::HarqBus;
use super::harq_bus::{ArqLevel, ArqSlot};

/// Reserved MACIndex for the RA broadcast (C.S0024-200-C §2.4.1.3.2.2).
pub const RA_MAC_INDEX: u8 = 4;
/// Walsh cover length on the default/subtype-1 forward MAC channel.
pub const SUBTYPE0_MAC_WALSH_LEN: usize = 64;
/// Walsh cover length on the subtype-2 forward MAC channel.
pub const SUBTYPE2_MAC_WALSH_LEN: usize = 128;
/// Test-facing alias for the subtype-2 MAC Walsh length.
pub const MAC_WALSH_LEN: usize = SUBTYPE2_MAC_WALSH_LEN;
/// Number of physical 64-chip MAC bursts per slot.
pub const MAC_BURSTS_PER_SLOT: usize = 4;
/// Chips in each physical MAC burst.
pub const MAC_BURST_CHIPS: usize = 64;
/// Number of 128-chip Walsh-symbol repetitions per slot.
pub const MAC_SYMBOL_REPETITIONS_PER_SLOT: usize = 2;
/// Total MAC chips per slot.
pub const MAC_CHIPS_PER_SLOT: usize = MAC_BURSTS_PER_SLOT * MAC_BURST_CHIPS;
/// Default DRCLockPeriod attribute value: 16 slots.
pub const DEFAULT_DRC_LOCK_PERIOD_SLOTS: u64 = 16;
const RPC_DRCLOCK_SLOT_PERIOD: u64 = 4;
const RPC_DRCLOCK_SLOT_PHASE: u64 = 3;

/// One active MAC index's power-control + DRC-lock state for the slot.
#[derive(Debug, Clone, Copy)]
pub struct ActiveMac {
    /// MAC index (W_i^64 row, must be in `5..64` for live ATs).
    pub mac_index: u8,
    /// RPC bit (0 → power up, 1 → power down per §9.3.1.2.2). Mapped to ±1.
    pub rpc: bool,
    /// Fail-safe mode before measured reverse-pilot feedback is available:
    /// alternate RPC UP/DOWN on consecutive RPC/DRCLock slots so we do not
    /// rail the AT's reverse power in either direction.
    pub rpc_alternating: bool,
    /// DRCLock bit (0 → unlocked, 1 → locked). Mapped to ±1.
    pub drclock: bool,
    /// TrafficChannelAssignment FrameOffset. RPC and DRCLock are sent when
    /// required by the active physical subtype.
    pub frame_offset: u8,
    /// Negotiated Physical Layer subtype currently in use for this assignment.
    /// Default/subtype-1 uses 64-ary MAC; subtype 2+ uses 128-ary MAC.
    pub physical_layer_subtype: u16,
}

/// HRPD forward MAC channel encoder. Holds the per-slot state and emits 256
/// MAC chips per slot.
#[derive(Debug, Clone)]
pub struct HrpdForwardMacEncoder {
    ra: bool,
    actives: Vec<ActiveMac>,
    harq_bus: Option<Arc<HarqBus>>,
}

impl Default for HrpdForwardMacEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl HrpdForwardMacEncoder {
    pub fn new() -> Self {
        Self {
            // BPSK bit 1 maps to -1 SoftRAB, which subtype-3 RTC MAC treats
            // as Unloaded. Advertising bit 0 (+1/Loaded) drains T2P inflow.
            // TODO: This is set to false now to indicate "not busy" so reverse rate
            // can move up. At some point based on BTS load this can be set dynamically.
            ra: false,
            actives: Vec::new(),
            harq_bus: None,
        }
    }

    pub fn set_ra(&mut self, ra: bool) {
        self.ra = ra;
    }

    pub fn ra(&self) -> bool {
        self.ra
    }

    pub fn set_actives(&mut self, actives: Vec<ActiveMac>) {
        self.actives = actives;
    }

    pub fn set_harq_bus(&mut self, bus: Arc<HarqBus>) {
        self.harq_bus = Some(bus);
    }

    pub fn actives(&self) -> &[ActiveMac] {
        &self.actives
    }

    /// Emit the slot's 256 MAC chips (two W128 repetitions split across four
    /// 64-chip bursts) at slot 0.
    ///
    /// Test-only convenience that emits at slot 0; production calls
    /// [`next_slot_chips_at_slot`] so DRCLock/RPC time-division uses the live
    /// HRPD slot number.
    pub fn next_slot_chips(&self) -> Vec<Complex32> {
        self.next_slot_chips_at_slot(0)
    }

    /// Emit the slot's 256 MAC chips. Each 128-chip symbol repetition is the
    /// Walsh-summed contribution of every active MAC index plus the RA
    /// broadcast on MACIndex 4; the caller consumes the output as four
    /// consecutive 64-chip MAC bursts.
    pub fn next_slot_chips_at_slot(&self, slot_index: u64) -> Vec<Complex32> {
        let mut out = vec![Complex32::new(0.0, 0.0); MAC_CHIPS_PER_SLOT];
        self.fill_slot_chips(slot_index, &mut out);
        out
    }

    /// Fill `out` (cleared and resized to 256 zeros) with the slot's MAC chips.
    /// The TX hot path passes its reusable buffer so the slot does not allocate.
    pub fn next_slot_chips_into(&self, slot_index: u64, out: &mut Vec<Complex32>) {
        out.clear();
        out.resize(MAC_CHIPS_PER_SLOT, Complex32::new(0.0, 0.0));
        self.fill_slot_chips(slot_index, out);
    }

    fn arq_for_slot(&self, active: &ActiveMac, slot_index: u64) -> Option<ArqSlot> {
        self.harq_bus
            .as_ref()
            .and_then(|bus| bus.arq_at_slot(active.mac_index, slot_index))
    }

    /// Accumulate the slot's MAC chips into `out`, which must be `MAC_CHIPS_PER_SLOT`
    /// long and zeroed (the Walsh contributions are summed in place).
    fn fill_slot_chips(&self, slot_index: u64, out: &mut [Complex32]) {
        let emit_subtype0_ra = self.actives.is_empty()
            || self
                .actives
                .iter()
                .any(|active| !uses_subtype2_mac(active.physical_layer_subtype));
        let emit_subtype2_ra = self
            .actives
            .iter()
            .any(|active| uses_subtype2_mac(active.physical_layer_subtype));
        let mut n_contrib = 0usize;
        if emit_subtype0_ra {
            n_contrib += 1;
        }
        if emit_subtype2_ra {
            n_contrib += 1;
        }
        for active in &self.actives {
            if uses_subtype2_mac(active.physical_layer_subtype) {
                if subtype2_mac_channel_covers(active.mac_index).is_none() {
                    continue;
                }
                if subtype2_control_slot(slot_index, active.frame_offset) {
                    n_contrib += 2;
                } else if let Some(arq) = self.arq_for_slot(active, slot_index) {
                    n_contrib += usize::from(arq.h_or_l != ArqLevel::Off)
                        + usize::from(arq.p != ArqLevel::Off);
                }
            } else if subtype0_rpc_slot(slot_index, active.frame_offset)
                || subtype0_drclock_slot(slot_index, active.frame_offset)
            {
                if subtype0_mac_channel_cover(active.mac_index).is_some() {
                    n_contrib += 1;
                }
            }
        }
        let n_contrib = n_contrib.max(1);
        let scale = 1.0_f32 / (n_contrib as f32).sqrt();

        if emit_subtype0_ra {
            let ra_sym = bpsk(self.ra) * scale;
            let (row, on_q) =
                subtype0_mac_channel_cover(RA_MAC_INDEX).expect("RA MACIndex must be valid");
            add_walsh_repeated::<SUBTYPE0_MAC_WALSH_LEN>(
                out,
                MAC_CHIPS_PER_SLOT / SUBTYPE0_MAC_WALSH_LEN,
                row,
                on_q,
                ra_sym,
            );
        }
        if emit_subtype2_ra {
            let ra_sym = bpsk(self.ra) * scale;
            let (row, on_q, _) =
                subtype2_mac_channel_covers(RA_MAC_INDEX).expect("RA MACIndex must be valid");
            add_walsh_repeated::<SUBTYPE2_MAC_WALSH_LEN>(
                out,
                MAC_SYMBOL_REPETITIONS_PER_SLOT,
                row,
                on_q,
                ra_sym,
            );
        }

        for active in &self.actives {
            if uses_subtype2_mac(active.physical_layer_subtype) {
                let Some((row, rpc_on_q, drclock_on_q)) =
                    subtype2_mac_channel_covers(active.mac_index)
                else {
                    continue;
                };
                if !subtype2_control_slot(slot_index, active.frame_offset) {
                    // ARQ slots: H/L-ARQ rides the RPC phase, P-ARQ the
                    // DRCLock phase (C.S0024-A §13.3.1.3.2.2.4).
                    let Some(arq) = self.arq_for_slot(active, slot_index) else {
                        continue;
                    };
                    if arq.h_or_l != ArqLevel::Off {
                        add_walsh_repeated::<SUBTYPE2_MAC_WALSH_LEN>(
                            out,
                            MAC_SYMBOL_REPETITIONS_PER_SLOT,
                            row,
                            rpc_on_q,
                            arq.h_or_l.amplitude() * scale,
                        );
                    }
                    if arq.p != ArqLevel::Off {
                        add_walsh_repeated::<SUBTYPE2_MAC_WALSH_LEN>(
                            out,
                            MAC_SYMBOL_REPETITIONS_PER_SLOT,
                            row,
                            drclock_on_q,
                            arq.p.amplitude() * scale,
                        );
                    }
                    continue;
                }
                let rpc_sym =
                    bpsk(rpc_bit_for_slot(active, self.harq_bus.as_ref(), slot_index)) * scale;
                let drclock_sym = bpsk(active.drclock) * scale;
                add_walsh_repeated::<SUBTYPE2_MAC_WALSH_LEN>(
                    out,
                    MAC_SYMBOL_REPETITIONS_PER_SLOT,
                    row,
                    rpc_on_q,
                    rpc_sym,
                );
                add_walsh_repeated::<SUBTYPE2_MAC_WALSH_LEN>(
                    out,
                    MAC_SYMBOL_REPETITIONS_PER_SLOT,
                    row,
                    drclock_on_q,
                    drclock_sym,
                );
            } else if subtype0_rpc_slot(slot_index, active.frame_offset) {
                let Some((row, on_q)) = subtype0_mac_channel_cover(active.mac_index) else {
                    continue;
                };
                let rpc_sym =
                    bpsk(rpc_bit_for_slot(active, self.harq_bus.as_ref(), slot_index)) * scale;
                add_walsh_repeated::<SUBTYPE0_MAC_WALSH_LEN>(
                    out,
                    MAC_CHIPS_PER_SLOT / SUBTYPE0_MAC_WALSH_LEN,
                    row,
                    on_q,
                    rpc_sym,
                );
            } else if subtype0_drclock_slot(slot_index, active.frame_offset) {
                let Some((row, on_q)) = subtype0_mac_channel_cover(active.mac_index) else {
                    continue;
                };
                let drclock_sym = bpsk(active.drclock) * scale;
                add_walsh_repeated::<SUBTYPE0_MAC_WALSH_LEN>(
                    out,
                    MAC_CHIPS_PER_SLOT / SUBTYPE0_MAC_WALSH_LEN,
                    row,
                    on_q,
                    drclock_sym,
                );
            }
        }
    }
}

fn uses_subtype2_mac(physical_layer_subtype: u16) -> bool {
    physical_layer_subtype >= 2
}

fn rpc_bit_for_slot(active: &ActiveMac, bus: Option<&Arc<HarqBus>>, slot_index: u64) -> bool {
    if let Some(rpc_bit) = bus.and_then(|bus| bus.rpc_at_slot(active.mac_index, slot_index)) {
        rpc_bit & 0x01 != 0
    } else if active.rpc_alternating {
        ((slot_index / RPC_DRCLOCK_SLOT_PERIOD) & 0x01) != 0
    } else {
        active.rpc
    }
}

pub(crate) fn mac_rpc_slot(slot_index: u64, frame_offset: u8, physical_layer_subtype: u16) -> bool {
    if uses_subtype2_mac(physical_layer_subtype) {
        subtype2_control_slot(slot_index, frame_offset)
    } else {
        subtype0_rpc_slot(slot_index, frame_offset)
    }
}

fn subtype0_rpc_slot(slot_index: u64, frame_offset: u8) -> bool {
    let offset = u64::from(frame_offset & 0x0f);
    (slot_index + DEFAULT_DRC_LOCK_PERIOD_SLOTS - offset) % DEFAULT_DRC_LOCK_PERIOD_SLOTS != 0
}

fn subtype0_drclock_slot(slot_index: u64, frame_offset: u8) -> bool {
    let offset = u64::from(frame_offset & 0x0f);
    (slot_index + DEFAULT_DRC_LOCK_PERIOD_SLOTS - offset) % DEFAULT_DRC_LOCK_PERIOD_SLOTS == 0
}

fn subtype2_control_slot(slot_index: u64, frame_offset: u8) -> bool {
    let offset = u64::from(frame_offset & 0x03);
    (slot_index + RPC_DRCLOCK_SLOT_PERIOD - offset) % RPC_DRCLOCK_SLOT_PERIOD
        == RPC_DRCLOCK_SLOT_PHASE
}

fn subtype0_mac_channel_cover(mac_index: u8) -> Option<(usize, bool)> {
    if mac_index >= SUBTYPE0_MAC_WALSH_LEN as u8 {
        return None;
    }
    if mac_index == RA_MAC_INDEX {
        return Some((2, false));
    }
    if (6..=62).contains(&mac_index) && mac_index & 1 == 0 {
        Some((usize::from(mac_index / 2), false))
    } else if (5..=63).contains(&mac_index) && mac_index & 1 == 1 {
        Some((usize::from((mac_index - 1) / 2) + 32, true))
    } else {
        None
    }
}

fn subtype2_mac_channel_covers(mac_index: u8) -> Option<(usize, bool, bool)> {
    if mac_index >= SUBTYPE2_MAC_WALSH_LEN as u8 {
        return None;
    }
    if mac_index == RA_MAC_INDEX {
        return Some((2, false, false));
    }
    if (6..=62).contains(&mac_index) && mac_index & 1 == 0 {
        Some((usize::from(mac_index / 2), false, true))
    } else if (5..=63).contains(&mac_index) && mac_index & 1 == 1 {
        Some((usize::from((mac_index - 1) / 2) + 32, true, false))
    } else if (72..=126).contains(&mac_index) && mac_index & 1 == 0 {
        Some((usize::from(mac_index / 2) + 32, false, true))
    } else if (73..=127).contains(&mac_index) && mac_index & 1 == 1 {
        Some((usize::from((mac_index - 1) / 2) + 64, true, false))
    } else {
        None
    }
}

fn add_walsh_repeated<const N: usize>(
    out: &mut [Complex32],
    repetitions: usize,
    row: usize,
    on_q: bool,
    sym: f32,
) {
    let walsh_gen = WalshGenerator::new::<N>(row, 1);
    let input = if on_q {
        Complex32::new(0.0, sym)
    } else {
        Complex32::new(sym, 0.0)
    };
    let chips = walsh_gen.feed(input);
    for repeat in 0..repetitions {
        for (i, c) in chips.iter().enumerate() {
            out[repeat * N + i] += *c;
        }
    }
}

fn bpsk(bit: bool) -> f32 {
    if bit { -1.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subtype2_active(mac_index: u8) -> ActiveMac {
        ActiveMac {
            mac_index,
            rpc: false,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 2,
        }
    }

    fn subtype0_active(mac_index: u8) -> ActiveMac {
        ActiveMac {
            mac_index,
            rpc: false,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 0,
        }
    }

    #[test]
    fn emits_exactly_256_chips() {
        let m = HrpdForwardMacEncoder::new();
        assert_eq!(m.next_slot_chips().len(), MAC_CHIPS_PER_SLOT);
    }

    #[test]
    fn default_idle_ra_uses_subtype0_walsh64() {
        let m = HrpdForwardMacEncoder::new();
        let chips = m.next_slot_chips();
        let symbol: Vec<Complex32> = chips[..SUBTYPE0_MAC_WALSH_LEN].to_vec();
        let row_2 = WalshGenerator::new::<SUBTYPE0_MAC_WALSH_LEN>(2, 1);
        let row_4 = WalshGenerator::new::<SUBTYPE0_MAC_WALSH_LEN>(4, 1);
        let probe_2 = row_2.feed(Complex32::new(1.0, 0.0));
        let probe_4 = row_4.feed(Complex32::new(1.0, 0.0));
        let corr = |chips: &[Complex32], probe: &[Complex32]| -> f32 {
            chips
                .iter()
                .zip(probe.iter())
                .map(|(a, b)| a.re * b.re)
                .sum()
        };
        let c2 = corr(&symbol, &probe_2);
        let c4 = corr(&symbol, &probe_4);
        assert!(c2.abs() > 10.0 * c4.abs().max(1e-3));
    }

    #[test]
    fn subtype2_ra_uses_walsh128_when_subtype2_active() {
        let mut m = HrpdForwardMacEncoder::new();
        m.set_actives(vec![subtype2_active(10)]);
        let chips = m.next_slot_chips_at_slot(3);
        let symbol: Vec<Complex32> = chips[..SUBTYPE2_MAC_WALSH_LEN].to_vec();
        let row_2 = WalshGenerator::new::<SUBTYPE2_MAC_WALSH_LEN>(2, 1);
        let row_4 = WalshGenerator::new::<SUBTYPE2_MAC_WALSH_LEN>(4, 1);
        let probe_2 = row_2.feed(Complex32::new(1.0, 0.0));
        let probe_4 = row_4.feed(Complex32::new(1.0, 0.0));
        let corr = |chips: &[Complex32], probe: &[Complex32]| -> f32 {
            chips
                .iter()
                .zip(probe.iter())
                .map(|(a, b)| a.re * b.re)
                .sum()
        };
        let c2 = corr(&symbol, &probe_2);
        let c4 = corr(&symbol, &probe_4);
        assert!(c2.abs() > 10.0 * c4.abs().max(1e-3));
        assert!(c2 > 0.0, "default RA must advertise not busy (0), got {c2}");
    }

    #[test]
    fn odd_subtype2_active_mac_uses_q_phase_for_rpc_and_i_phase_for_drclock() {
        let mut m = HrpdForwardMacEncoder::new();
        m.set_actives(vec![subtype2_active(5)]);
        let chips = m.next_slot_chips_at_slot(3);
        let symbol: Vec<Complex32> = chips[..MAC_WALSH_LEN].to_vec();
        let row_34 = WalshGenerator::new::<MAC_WALSH_LEN>(34, 1);
        let probe_q = row_34.feed(Complex32::new(0.0, 1.0));
        let probe_i = row_34.feed(Complex32::new(1.0, 0.0));
        let c_q: f32 = symbol
            .iter()
            .zip(probe_q.iter())
            .map(|(a, b)| a.im * b.im)
            .sum();
        let c_i: f32 = symbol
            .iter()
            .zip(probe_i.iter())
            .map(|(a, b)| a.re * b.re)
            .sum();
        assert!(
            c_q > 5.0 && c_i < -5.0,
            "expected odd MAC RPC on Q and DRCLock on I, got q={c_q} i={c_i}"
        );
    }

    #[test]
    fn even_subtype2_active_mac_uses_i_phase_for_rpc_and_q_phase_for_drclock() {
        let mut a = HrpdForwardMacEncoder::new();
        a.set_actives(vec![subtype2_active(10)]);
        let chips = a.next_slot_chips_at_slot(3);
        let symbol: Vec<Complex32> = chips[..MAC_WALSH_LEN].to_vec();
        let row = WalshGenerator::new::<MAC_WALSH_LEN>(5, 1);
        let probe_i = row.feed(Complex32::new(1.0, 0.0));
        let probe_q = row.feed(Complex32::new(0.0, 1.0));
        let corr_rpc: f32 = symbol
            .iter()
            .zip(probe_i.iter())
            .map(|(x, y)| x.re * y.re)
            .sum();
        let corr_drclock: f32 = symbol
            .iter()
            .zip(probe_q.iter())
            .map(|(x, y)| x.im * y.im)
            .sum();
        assert!(
            corr_rpc > 5.0 && corr_drclock < -5.0,
            "expected even MAC RPC on I and DRCLock on Q, got rpc={corr_rpc} drclock={corr_drclock}"
        );
    }

    #[test]
    fn subtype0_rpc_uses_64ary_cover_and_drclock_puncture() {
        let mut m = HrpdForwardMacEncoder::new();
        m.set_actives(vec![subtype0_active(10)]);
        let row = WalshGenerator::new::<SUBTYPE0_MAC_WALSH_LEN>(5, 1);
        let probe = row.feed(Complex32::new(1.0, 0.0));
        let corr = |slot: u64| -> f32 {
            m.next_slot_chips_at_slot(slot)[..SUBTYPE0_MAC_WALSH_LEN]
                .iter()
                .zip(probe.iter())
                .map(|(x, y)| x.re * y.re)
                .sum()
        };

        assert!(corr(1) > 5.0, "subtype0 RPC transmits in slot 1");
        assert!(
            corr(0) < -5.0,
            "slot 0 is the DRCLock puncture for FrameOffset 0"
        );
    }

    #[test]
    fn rpc_false_is_power_up_symbol() {
        let mut m = HrpdForwardMacEncoder::new();
        m.set_actives(vec![subtype2_active(10)]);
        let chips = m.next_slot_chips_at_slot(3);
        let row = WalshGenerator::new::<MAC_WALSH_LEN>(5, 1);
        let probe = row.feed(Complex32::new(1.0, 0.0));
        let corr: f32 = chips[..MAC_WALSH_LEN]
            .iter()
            .zip(probe.iter())
            .map(|(x, y)| x.re * y.re)
            .sum();
        assert!(corr > 0.0, "RPC=false must encode the power-up symbol");
    }

    #[test]
    fn scheduled_subtype2_rpc_uses_exact_slot_and_spec_polarity() {
        let bus = Arc::new(HarqBus::new());
        bus.schedule_rpc_at_slot(10, 3, 0);
        bus.schedule_rpc_at_slot(10, 7, 1);

        let mut m = HrpdForwardMacEncoder::new();
        m.set_harq_bus(bus);
        let mut active = subtype2_active(10);
        active.rpc = true;
        active.drclock = false;
        m.set_actives(vec![active]);
        let row = WalshGenerator::new::<MAC_WALSH_LEN>(5, 1);
        let probe = row.feed(Complex32::new(1.0, 0.0));
        let corr = |slot: u64| -> f32 {
            m.next_slot_chips_at_slot(slot)[..MAC_WALSH_LEN]
                .iter()
                .zip(probe.iter())
                .map(|(x, y)| x.re * y.re)
                .sum()
        };

        assert!(corr(3) > 0.0, "scheduled RPC bit 0 commands power up");
        assert!(corr(7) < 0.0, "scheduled RPC bit 1 commands power down");
        assert!(
            corr(11) < 0.0,
            "unscheduled RPC slots use the installed fallback bit"
        );
        assert!(
            corr(4).abs() < 1e-3,
            "non-RPC slots must not transmit the active RPC row"
        );
    }

    #[test]
    fn scheduled_subtype0_rpc_uses_exact_slot_and_spec_polarity() {
        let bus = Arc::new(HarqBus::new());
        bus.schedule_rpc_at_slot(10, 1, 0);
        bus.schedule_rpc_at_slot(10, 2, 1);

        let mut m = HrpdForwardMacEncoder::new();
        m.set_harq_bus(bus);
        let mut active = subtype0_active(10);
        active.rpc = true;
        active.drclock = false;
        m.set_actives(vec![active]);
        let row = WalshGenerator::new::<SUBTYPE0_MAC_WALSH_LEN>(5, 1);
        let probe = row.feed(Complex32::new(1.0, 0.0));
        let corr = |slot: u64| -> f32 {
            m.next_slot_chips_at_slot(slot)[..SUBTYPE0_MAC_WALSH_LEN]
                .iter()
                .zip(probe.iter())
                .map(|(x, y)| x.re * y.re)
                .sum()
        };

        assert!(corr(1) > 0.0, "scheduled RPC bit 0 commands power up");
        assert!(corr(2) < 0.0, "scheduled RPC bit 1 commands power down");
        assert!(
            corr(3) < 0.0,
            "unscheduled RPC slots use the installed fallback bit"
        );
    }

    #[test]
    fn subtype2_non_control_slots_do_not_emit_active_rpc_or_drclock() {
        let mut m = HrpdForwardMacEncoder::new();
        m.set_actives(vec![subtype2_active(10)]);
        let row = WalshGenerator::new::<MAC_WALSH_LEN>(5, 1);
        let probe_i = row.feed(Complex32::new(1.0, 0.0));
        let probe_q = row.feed(Complex32::new(0.0, 1.0));
        let chips = m.next_slot_chips_at_slot(2);
        let corr_i: f32 = chips[..MAC_WALSH_LEN]
            .iter()
            .zip(probe_i.iter())
            .map(|(x, y)| x.re * y.re)
            .sum();
        let corr_q: f32 = chips[..MAC_WALSH_LEN]
            .iter()
            .zip(probe_q.iter())
            .map(|(x, y)| x.im * y.im)
            .sum();
        assert!(
            corr_i.abs() < 1e-3 && corr_q.abs() < 1e-3,
            "non-control slot should not carry active MAC control, got i={corr_i} q={corr_q}"
        );
    }

    #[test]
    fn subtype2_rpc_alternating_mode_flips_control_slots_only() {
        let mut m = HrpdForwardMacEncoder::new();
        let mut active = subtype2_active(10);
        active.rpc_alternating = true;
        active.drclock = false;
        m.set_actives(vec![active]);
        let row = WalshGenerator::new::<MAC_WALSH_LEN>(5, 1);
        let probe = row.feed(Complex32::new(1.0, 0.0));
        let corr = |slot: u64| -> f32 {
            m.next_slot_chips_at_slot(slot)[..MAC_WALSH_LEN]
                .iter()
                .zip(probe.iter())
                .map(|(x, y)| x.re * y.re)
                .sum()
        };

        assert!(corr(3) > 0.0, "first RPC control slot transmits UP");
        assert!(corr(7) < 0.0, "next RPC control slot transmits DOWN");
        assert!(corr(11) > 0.0, "alternating RPC returns to UP");
        assert!(corr(4).abs() < 1e-3, "non-control slots are silent");
    }

    #[test]
    fn arq_levels_transmit_on_non_control_slots_with_spec_phases() {
        let bus = Arc::new(HarqBus::new());
        // H-ARQ ACK (+1) and P-ARQ NAK (−1) scheduled for slot 4 (a
        // non-control slot for FrameOffset 0); nothing for slot 5.
        bus.schedule_arq_at_slot(10, 4, ArqLevel::Plus, ArqLevel::Minus);
        // L-ARQ NAK on slot 6, P-ARQ off.
        bus.schedule_arq_at_slot(10, 6, ArqLevel::Minus, ArqLevel::Off);

        let mut m = HrpdForwardMacEncoder::new();
        m.set_harq_bus(bus);
        m.set_actives(vec![subtype2_active(10)]);
        let row = WalshGenerator::new::<MAC_WALSH_LEN>(5, 1);
        let probe_i = row.feed(Complex32::new(1.0, 0.0));
        let probe_q = row.feed(Complex32::new(0.0, 1.0));
        let corr = |slot: u64, on_q: bool| -> f32 {
            let chips = m.next_slot_chips_at_slot(slot);
            chips[..MAC_WALSH_LEN]
                .iter()
                .zip(if on_q { probe_q.iter() } else { probe_i.iter() })
                .map(|(x, y)| if on_q { x.im * y.im } else { x.re * y.re })
                .sum()
        };

        // MAC 10 is even: H/L-ARQ on I (RPC phase), P-ARQ on Q.
        assert!(corr(4, false) > 5.0, "H-ARQ ACK is +1 on I");
        assert!(corr(4, true) < -5.0, "P-ARQ NAK is −1 on Q");
        assert!(corr(6, false) < -5.0, "L-ARQ NAK is −1 on I");
        assert!(corr(6, true).abs() < 1e-3, "P-ARQ off transmits nothing");
        assert!(
            corr(5, false).abs() < 1e-3 && corr(5, true).abs() < 1e-3,
            "unscheduled ARQ slot stays silent"
        );
    }

    #[test]
    fn arq_merge_keeps_existing_channel_when_off() {
        let bus = Arc::new(HarqBus::new());
        bus.schedule_arq_at_slot(10, 8, ArqLevel::Plus, ArqLevel::Off);
        bus.schedule_arq_at_slot(10, 8, ArqLevel::Off, ArqLevel::Minus);
        let got = bus.arq_at_slot(10, 8).expect("slot scheduled");
        assert_eq!(got.h_or_l, ArqLevel::Plus);
        assert_eq!(got.p, ArqLevel::Minus);
    }

    #[test]
    fn invalid_mac_index_is_dropped() {
        let mut m = HrpdForwardMacEncoder::new();
        let mut active = subtype2_active(200);
        active.drclock = false;
        m.set_actives(vec![active]);
        // Falls back to RA-only output.
        let only_ra = HrpdForwardMacEncoder::new().next_slot_chips();
        let with_invalid = m.next_slot_chips();
        assert_eq!(with_invalid.len(), only_ra.len());
    }
}
