//! Shared CDMA2000 SR1 physical-layer constants.

/// SO0: service option rejection in a Page Response or Service Option Response.
pub const SERVICE_OPTION_REJECTED: u16 = 0;

/// SO1: default Basic Variable Rate Voice Service.
pub const SERVICE_OPTION_BASIC_VOICE: u16 = 1;

/// SO2: Mobile Station Loopback, Rate Set 1.
pub const SERVICE_OPTION_LOOPBACK_RS1: u16 = 2;

/// SO3: EVRC-A / IS-127 narrowband.
pub const SERVICE_OPTION_EVRC_A: u16 = 3;

/// SO4: asynchronous data service, Rate Set 1.
pub const SERVICE_OPTION_ASYNC_DATA_RS1: u16 = 4;

/// SO6: Short Message Services.
pub const SERVICE_OPTION_SMS: u16 = 6;

/// SO7: packet data bearer service using RLP Type 1 and Rate Set 1 by default.
pub const SERVICE_OPTION_PACKET_DATA: u16 = 7;

/// SO9: Mobile Station Loopback, Rate Set 2.
pub const SERVICE_OPTION_LOOPBACK_RS2: u16 = 9;

/// SO12: asynchronous data service using RLP Type 1 and Rate Set 2 by default.
pub const SERVICE_OPTION_ASYNC_DATA: u16 = 12;

/// SO14: Short Message Services, Rate Set 2.
pub const SERVICE_OPTION_SMS_RS2: u16 = 14;

/// SO15: packet data bearer service, Rate Set 2.
pub const SERVICE_OPTION_PACKET_DATA_RS2: u16 = 15;

/// SO17: IS-733 high-rate (13 kbps) speech, Rate Set 2.
pub const SERVICE_OPTION_HIGH_RATE_VOICE: u16 = 17;

/// SO18: OTASP (C.S0016-D §1.2).
pub const SERVICE_OPTION_OTASP: u16 = 18;

/// SO19: OTASP, Rate Set 2 (C.S0016-D §1.2).
pub const SERVICE_OPTION_OTASP_RS2: u16 = 19;

/// SO33: High-rate packet data service.
pub const SERVICE_OPTION_PACKET_DATA_RC3: u16 = 33;

/// A.S0008-C §1.11.1. Never sent over the air.
pub const SERVICE_OPTION_HRPD_PACKET_DATA: u16 = 59;

/// SO68: EVRC-B narrowband.
pub const SERVICE_OPTION_EVRC_B: u16 = 68;

/// SO70: EVRC-WB.
pub const SERVICE_OPTION_EVRC_WB: u16 = 70;

/// SO32768: Qualcomm QCELP-13K variable-rate speech.
pub const SERVICE_OPTION_QCELP13: u16 = 32768;

/// Data Burst Message BURST_TYPE for SMS (TIA-637).
pub const BURST_TYPE_SMS: u8 = 0b000011;

/// Data Burst Message BURST_TYPE for OTASP/OTAPA payloads (C.S0016-D §2.3).
pub const BURST_TYPE_OTASP: u8 = 0b000100;

/// SR1 chip rate in chips per second (C.S0002-E §1.1).
pub const SR1_CHIP_RATE_HZ: u64 = 1_228_800;

/// Chips in a 20 ms traffic/signaling frame (SR1_CHIP_RATE_HZ × 0.020).
pub const SR1_CHIPS_PER_FRAME: u64 = 24_576;

/// Power-control groups per 20 ms frame (20 ms / 1.25 ms).
pub const SR1_PCGS_PER_FRAME: usize = 16;

/// REV_PWR_CNTL_DELAY advertised for gated RC3 reverse power control.
/// A value of 3 places feedback five PCGs after the start of the gated-on
/// reverse PCG, making a nine-PCG software scheduling lead land on a valid
/// forward power-control subchannel slot.
pub const RC3_GATED_REV_PWR_CNTL_DELAY: u8 = 3;

const REVERSE_FCH_GATING_MIN_RC: u8 = 3;
const REVERSE_FCH_GATING_MAX_RC: u8 = 6;

/// Return whether the reverse radio configuration supports FCH gating.
pub const fn reverse_fch_gating_supported(rev_rc: u8) -> bool {
    rev_rc >= REVERSE_FCH_GATING_MIN_RC && rev_rc <= REVERSE_FCH_GATING_MAX_RC
}

/// Chips in one 80 ms paging slot (SR1_CHIP_RATE_HZ × 0.080).
pub const SR1_CHIPS_PER_80MS: u64 = 98_304;

/// Chips in 320 ms (SR1_CHIP_RATE_HZ × 0.320).
pub const SR1_CHIPS_320MS: u64 = 393_216;

// --- RC1 / access-channel spreading geometry (C.S0002-E §2.1.2) ---

/// PN chips per 64-ary Walsh chip on RC1 and the reverse access channel.
pub const RC1_PN_CHIPS_PER_WALSH_CHIP: usize = 4;

/// Walsh chips per 64-ary orthogonal symbol (W₆ code length).
pub const RC1_WALSH_CHIPS_PER_SYMBOL: usize = 64;

/// Soft bits per 64-ary symbol (log₂ 64).
pub const RC1_SOFT_BITS_PER_SYMBOL: usize = 6;

/// Walsh symbols per 20 ms RC1 frame (RC1_SYMBOLS_PER_PCG × SR1_PCGS_PER_FRAME).
pub const RC1_SYMBOLS_PER_FRAME: usize = 96;

/// Walsh symbols per power-control group on RC1 and the access channel.
pub const RC1_SYMBOLS_PER_PCG: usize = 6;
