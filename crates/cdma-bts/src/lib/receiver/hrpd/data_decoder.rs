//! HRPD Rev 0 Reverse Data Channel decoder.
//!
//! Spec references (C.S0024-0 v4.0):
//! - §9.2.1.3.3.5 Data Channel (Walsh cover `W_2^4`, slot/frame alignment).
//! - §9.2.1.3.1   Reverse Channel Structure (Data sub-channel on Q arm, see
//!   Figure 9.2.1.3.1-3).
//! - Table 9.2.1.3.1.1-1 Modulation Parameters for the Access Channel and
//!   the Reverse Traffic Channel (data rate ↔ bits per physical-layer
//!   packet ↔ code rate).
//! - §9.2.1.3.4.1 / Table 9.2.1.3.4.1-1 Parameters for the Reverse Link
//!   Encoder (turbo encoder input symbols, encoder output block length).
//! - §9.2.1.3.4.2 Turbo Encoding.
//! - §9.2.1.3.5   Channel Interleaving (reverse link bit-reversal).
//! - §9.2.1.3.6   Sequence Repetition.
//!
//! The Rev 0 reverse chain is encoder → channel interleaver → sequence
//! repetition → modulation: there is **no data scrambling** on the reverse
//! link (scrambling exists only on the forward link, §9.3.1.3.2.3.3). The
//! interlace-seeded reverse scrambler belongs to the subtype-2 physical
//! layer (C.S0024-200-C §2.3.1.3.5) and never applies to a default-subtype
//! session — applying it here made every live reverse traffic frame
//! undecodable.
//!
//! Per Table 9.2.1.3.1.1-1, the five Reverse Data Channel rates carry the
//! following physical-layer packet sizes (information bits including FCS
//! and tail, before turbo encoding):
//!
//! |   Rate (kbps) | Reverse Rate Index | Bits / PHY packet | Code rate |
//! |---------------|--------------------|-------------------|-----------|
//! |          9.6  |          1         |        256        |   1/4     |
//! |         19.2  |          2         |        512        |   1/4     |
//! |         38.4  |          3         |       1024        |   1/4     |
//! |         76.8  |          4         |       2048        |   1/4     |
//! |        153.6  |          5         |       4096        |   1/2     |
//!
//! Each frame is 16 slots / 26.66… ms (§9.2.1.3.1).

use num::complex::Complex32;

use cdma_common::hrpd::{
    air::HrpdTrafficEvent,
    traffic::{
        DEFAULT_PACKET_STREAM_ID, DEFAULT_PACKET_STREAM2_ID, TrafficFrameError,
        parse_connection_layer_packets, parse_default_packet_rlp_packet_bits,
        parse_reverse_stream1_packets, parse_reverse_traffic_mac_packet_for_subtype,
        parse_stream_layer_packet_bytes, physical_crc24,
    },
};

use crate::phy::hrpd::crc::physical_crc16;
#[cfg(test)]
use crate::phy::hrpd::interleaver::{channel_deinterleave, channel_interleave};
use crate::phy::hrpd::scrambler::HrpdForwardScrambler;
#[cfg(test)]
use crate::phy::hrpd::turbo::HrpdTurboEncoder;
use crate::phy::hrpd::turbo_decoder::HrpdTurboDecoder;
use crate::phy::walsh::WalshDecoder;

/// Length of the Reverse Data Channel Walsh cover (`W_2^4`) per
/// C.S0024-0 v4.0 §9.2.1.3.3.5.
pub const DATA_WALSH_LEN: usize = 4;

/// Walsh index `i` in `W_i^4` for the Data Channel cover
/// (C.S0024-0 v4.0 §9.2.1.3.3.5: "orthogonally spread by Walsh function
/// `W_2^4`").
pub const DATA_WALSH_INDEX: u8 = 2;

/// Reverse Data Channel modulation symbols in one 16-slot frame after W2^4
/// decovering: 16 slots × 2048 chips / 4 chips per data symbol.
pub const REVERSE_DATA_FRAME_SYMBOLS: usize = 8192;
/// Subtype-2 reverse data subpacket spans one 4-slot subframe.
pub const REVERSE_DATA_SUBFRAME_SYMBOLS: usize = REVERSE_DATA_FRAME_SYMBOLS / 4;

/// Rev 0 physical-layer FCS bits carried after the Reverse Traffic Channel MAC
/// packet (C.S0024-200-C §1.2.2.4).
pub const REVERSE_DATA_FCS_BITS: usize = 16;

/// Subtype-2 reverse physical-layer FCS bits
/// (C.S0024-200-C §2.3.1.3.4.1).
pub const REVERSE_DATA_SUBTYPE2_FCS_BITS: usize = 24;

/// Physical-layer encoder tail bits. These are not turbo encoded and shall be
/// all zero (C.S0024-200-C §1.2.2.4 / §1.3.1.3.4.1).
pub const REVERSE_DATA_TAIL_BITS: usize = 6;

/// Supported Reverse Data Channel rates (C.S0024-0 v4.0 §9.2.1.3.3,
/// Table 9.2.1.3.1.1-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverseDataRate {
    /// 9.6 kbps, RRI = 1, 256-bit physical-layer packet, code rate 1/4.
    Kbps9_6,
    /// 19.2 kbps, RRI = 2, 512-bit physical-layer packet, code rate 1/4.
    Kbps19_2,
    /// 38.4 kbps, RRI = 3, 1024-bit physical-layer packet, code rate 1/4.
    Kbps38_4,
    /// 76.8 kbps, RRI = 4, 2048-bit physical-layer packet, code rate 1/4.
    Kbps76_8,
    /// 153.6 kbps, RRI = 5, 4096-bit physical-layer packet, code rate 1/2.
    Kbps153_6,
}

impl ReverseDataRate {
    /// Bits per physical-layer packet for this rate
    /// (C.S0024-0 v4.0 Table 9.2.1.3.1.1-1 / Table 9.2.1.3.4.1-1).
    pub fn payload_bits(&self) -> u32 {
        match self {
            Self::Kbps9_6 => 256,
            Self::Kbps19_2 => 512,
            Self::Kbps38_4 => 1024,
            Self::Kbps76_8 => 2048,
            Self::Kbps153_6 => 4096,
        }
    }

    /// Reverse Rate Index (RRI) symbol used on the RRI Channel for this
    /// rate (C.S0024-0 v4.0 Table 9.2.1.3.3.2-1).
    pub fn rri_index(&self) -> u8 {
        match self {
            Self::Kbps9_6 => 1,
            Self::Kbps19_2 => 2,
            Self::Kbps38_4 => 3,
            Self::Kbps76_8 => 4,
            Self::Kbps153_6 => 5,
        }
    }

    /// Turbo encoder code rate numerator/denominator for this data rate
    /// (C.S0024-0 v4.0 Table 9.2.1.3.4.1-1).
    pub fn code_rate(&self) -> (u8, u8) {
        self.code_rate_for_physical_layer_subtype(0)
    }

    /// Turbo encoder code rate numerator/denominator for this data rate under
    /// the negotiated Physical Layer subtype. Subtype 2 B4 reverse traffic
    /// uses mother rate 1/5 for the payload sizes this decoder supports
    /// (C.S0024-200-C Table 2.3.1.3.4-2/-3).
    pub fn code_rate_for_physical_layer_subtype(&self, physical_layer_subtype: u16) -> (u8, u8) {
        if physical_layer_subtype == 2 {
            return (1, 5);
        }
        match self {
            Self::Kbps9_6 | Self::Kbps19_2 | Self::Kbps38_4 | Self::Kbps76_8 => (1, 4),
            Self::Kbps153_6 => (1, 2),
        }
    }

    /// Post-rate-match encoder output block length in channel symbols, per
    /// C.S0024-0 v4.0 Table 9.2.1.3.4.1-1: `payload_bits / (num/den) =
    /// payload_bits * den / num`.
    pub fn encoder_block_symbols(&self) -> u32 {
        self.encoder_block_symbols_for_physical_layer_subtype(0)
    }

    pub fn encoder_block_symbols_for_physical_layer_subtype(
        &self,
        physical_layer_subtype: u16,
    ) -> u32 {
        let (num, den) = self.code_rate_for_physical_layer_subtype(physical_layer_subtype);
        self.payload_bits() * u32::from(den) / u32::from(num)
    }

    /// Reverse Traffic Channel MAC-layer packet bits carried inside this PHY
    /// packet before the 16-bit physical FCS and six zero tail bits.
    pub fn mac_packet_bits(&self) -> u32 {
        self.mac_packet_bits_for_physical_layer_subtype(0)
    }

    /// Reverse Traffic Channel MAC-layer packet bits for this PHY packet
    /// under the negotiated Physical Layer subtype.
    pub fn mac_packet_bits_for_physical_layer_subtype(&self, physical_layer_subtype: u16) -> u32 {
        self.payload_bits()
            - reverse_data_fcs_bits_for_physical_layer_subtype(physical_layer_subtype) as u32
            - REVERSE_DATA_TAIL_BITS as u32
    }

    /// Recover a [`ReverseDataRate`] from a Reverse Rate Index, if any.
    pub fn from_rri(rri: u8) -> Option<Self> {
        match rri {
            1 => Some(Self::Kbps9_6),
            2 => Some(Self::Kbps19_2),
            3 => Some(Self::Kbps38_4),
            4 => Some(Self::Kbps76_8),
            5 => Some(Self::Kbps153_6),
            _ => None,
        }
    }
}

/// BPSK soft-bit demap from baseband samples on the Q arm
/// (C.S0024-0 v4.0 §9.2.1.3.1: Reverse Data sub-channel is BPSK on the Q
/// branch, §9.2.1.3.3.5).
///
/// Convention: spec maps "0 → +1, 1 → −1" on the Q-arm (Figure 9.2.1.3.1-3
/// modulator inputs). The matched-filter output therefore has positive `im`
/// for transmitted bit 0 and negative `im` for transmitted bit 1, so we emit
/// the Q value directly as a soft LLR: positive ⇒ bit 0, negative ⇒ bit 1.
pub fn demap_bpsk_q(samples: &[Complex32]) -> Vec<f32> {
    samples.iter().map(|s| s.im).collect()
}

/// Accumulate soft chips across each Walsh period and emit one soft symbol
/// per period (C.S0024-0 v4.0 §9.2.1.3.3.5 Data Channel Walsh cover
/// `W_2^4`). `walsh_len` is the cover length (4 for the Data Channel) and
/// `walsh_row` selects the row index `i` of `W_i^{walsh_len}` to despread
/// against.
///
/// The output is `±walsh_len`-scaled (sum, not mean) so that the SNR scales
/// with cover length; downstream stages treat the values as soft LLR
/// proxies and only sign and relative magnitude matter.
pub fn walsh_decover(soft_chips: &[f32], walsh_len: usize, walsh_row: usize) -> Vec<f32> {
    assert!(
        walsh_len.is_power_of_two(),
        "walsh_len must be power of two"
    );
    assert!(walsh_row < walsh_len, "walsh_row out of range");

    // Build the Walsh row via WalshDecoder's matrix generator. We only need
    // the +1/−1 chip pattern, not its complex despread path, so reuse the
    // decoder against a Complex32 stage to extract the row pattern.
    let row = walsh_row_pattern(walsh_len, walsh_row);

    soft_chips
        .chunks_exact(walsh_len)
        .map(|chunk| {
            let mut acc = 0.0f32;
            for (s, &c) in chunk.iter().zip(row.iter()) {
                acc += s * c as f32;
            }
            acc
        })
        .collect()
}

/// Produce a Walsh row's ±1 chip pattern by despreading a synthetic
/// unit-impulse complex frame through [`WalshDecoder`]; this lets us reuse
/// the existing matrix generator without re-implementing it. The pattern
/// returned is length `walsh_len` with entries in `{-1, +1}`.
fn walsh_row_pattern(walsh_len: usize, walsh_row: usize) -> Vec<i8> {
    // Probe each chip position with a unit Q-arm impulse; the sign of the
    // resulting correlation reveals the row's chip polarity at that index.
    // (We avoid exposing WalshDecoder's internal `code` field by sampling
    // its behaviour.)
    let mut pattern = Vec::with_capacity(walsh_len);
    let decoder = build_walsh_decoder(walsh_len, walsh_row);
    for k in 0..walsh_len {
        let mut probe = vec![Complex32::new(0.0, 0.0); walsh_len];
        probe[k] = Complex32::new(0.0, 1.0);
        let out = decoder.process_symbol(&probe);
        // process_symbol returns acc * (1/walsh_len); positive im ⇒ +1 chip.
        pattern.push(if out.im > 0.0 { 1 } else { -1 });
    }
    pattern
}

fn build_walsh_decoder(walsh_len: usize, walsh_row: usize) -> WalshDecoder {
    match walsh_len {
        2 => WalshDecoder::new::<2>(walsh_row),
        4 => WalshDecoder::new::<4>(walsh_row),
        8 => WalshDecoder::new::<8>(walsh_row),
        16 => WalshDecoder::new::<16>(walsh_row),
        32 => WalshDecoder::new::<32>(walsh_row),
        64 => WalshDecoder::new::<64>(walsh_row),
        128 => WalshDecoder::new::<128>(walsh_row),
        // walsh_len comes from the validated reverse-data rate table, never arbitrary.
        _ => unreachable!("unsupported walsh_len {walsh_len}"),
    }
}

/// Soft-symbol channel deinterleaver (C.S0024-0 v4.0 §9.2.1.3.4.5,
/// referencing §9.2.1.3.5: reverse-link channel interleaver is a pure
/// bit-reversal permutation on `2^L` symbol positions). Inverts the
/// permutation performed by `channel_interleave` so that soft values land
/// back in encoder-output order.
pub fn deinterleave(soft_symbols: Vec<f32>, block_size: usize) -> Vec<f32> {
    assert_eq!(
        soft_symbols.len(),
        block_size,
        "deinterleave: input length {} != block_size {}",
        soft_symbols.len(),
        block_size
    );
    // We reuse `channel_deinterleave`'s permutation by mapping each soft
    // value to an index, deinterleaving the index array, and reading back
    // the f32s in the new order. The u8 carrier is just a permutation
    // proxy and the block_size is at most 4096 (≤ u16 would suffice), but
    // we shrink larger blocks by chunking through the u8 permutation
    // indirectly via a usize-based mirror of `channel_deinterleave`.
    bit_reversal_deinterleave_f32(soft_symbols, block_size)
}

/// Mirror of `phy::hrpd::interleaver::channel_deinterleave` for `f32`
/// soft values (C.S0024-0 v4.0 §9.2.1.3.5). Kept local because the shared
/// helper is `u8`-only.
fn bit_reversal_deinterleave_f32(interleaved: Vec<f32>, block_size: usize) -> Vec<f32> {
    let l = ceil_log2(block_size);
    let padded = 1usize << l;
    let mut output = vec![0.0f32; block_size];
    let mut next_out: usize = 0;
    for i in 0..padded {
        let a = bit_reverse_u32(i as u32, l) as usize;
        if a < block_size {
            output[a] = interleaved[next_out];
            next_out += 1;
        }
    }
    debug_assert_eq!(next_out, block_size);
    output
}

fn ceil_log2(n: usize) -> u32 {
    assert!(n >= 1, "block size must be ≥ 1");
    if n == 1 { 0 } else { (n - 1).ilog2() + 1 }
}

fn bit_reverse_u32(value: u32, bits: u32) -> u32 {
    if bits == 0 {
        0
    } else {
        value.reverse_bits() >> (32 - bits)
    }
}

/// Turbo decode a rate-1/5 mother-stream of soft LLRs (C.S0024-0 v4.0
/// §9.2.1.3.4.2). The reverse data path always depunctures up to mother
/// rate 1/5 before calling this, so `code_rate` is informational and
/// reserved for future rate-aware optimisations.
/// Turbo decoder iterations for the reverse data channel.
const REVERSE_DATA_TURBO_ITERATIONS: usize = 8;
/// Soft-decision scale applied to reverse-data turbo LLRs. Unity: the
/// depunctured LLRs feed the decoder as-is.
const REVERSE_DATA_LLR_SCALE: f32 = 1.0;

pub fn turbo_decode(soft: &[f32], payload_bits: u32, _code_rate: (u8, u8)) -> Option<Vec<u8>> {
    let mut llrs = soft.to_vec();
    if (REVERSE_DATA_LLR_SCALE - 1.0).abs() > f32::EPSILON {
        for llr in &mut llrs {
            *llr *= REVERSE_DATA_LLR_SCALE;
        }
    }
    let decoder =
        HrpdTurboDecoder::new(payload_bits)?.with_iterations(REVERSE_DATA_TURBO_ITERATIONS);
    Some(decoder.decode(&llrs))
}

/// Reverse Data Channel decoder. End-to-end pipeline holder
/// (C.S0024-0 v4.0 §9.2.1.3.3.5 / §9.2.1.3.4).
#[derive(Debug, Clone)]
pub struct DataDecoder {
    /// Negotiated / detected data rate for this frame.
    rate: ReverseDataRate,
    physical_layer_subtype: u16,
}

impl Default for DataDecoder {
    fn default() -> Self {
        Self::new(ReverseDataRate::Kbps9_6)
    }
}

impl DataDecoder {
    /// Construct a Data Channel decoder.
    pub fn new(rate: ReverseDataRate) -> Self {
        Self {
            rate,
            physical_layer_subtype: 0,
        }
    }

    /// Construct a Data Channel decoder for the negotiated Physical Layer
    /// subtype. Subtype 2 keeps the same rate/packet-size table for the rates
    /// this receiver supports, but uses the subtype-2 reverse physical FCS.
    pub fn for_physical_layer_subtype(rate: ReverseDataRate, physical_layer_subtype: u16) -> Self {
        Self {
            rate,
            physical_layer_subtype,
        }
    }

    /// Current configured rate.
    pub fn rate(&self) -> ReverseDataRate {
        self.rate
    }

    pub fn physical_layer_subtype(&self) -> u16 {
        self.physical_layer_subtype
    }

    /// Decode one 16-slot Reverse Data Channel frame. Returns the validated
    /// Reverse Traffic Channel MAC-layer packet bits, MSB-first.
    pub fn decode_frame(&self, samples: &[Complex32]) -> Option<Vec<u8>> {
        let frame =
            ReverseDataDecoder::for_physical_layer_subtype(self.rate, self.physical_layer_subtype)
                .decode_data_frame(samples);
        frame.crc_ok.then_some(frame.payload)
    }

    /// Decode one aligned 16-slot reverse Data Channel frame and unwrap valid
    /// Stream 1 Default Packet Application payloads into AN-facing traffic
    /// events. Pilot acquisition, RRI detection, and frame alignment remain
    /// caller responsibilities.
    pub fn decode_stream1_events(
        &self,
        uati: u32,
        mac_index: u8,
        samples: &[Complex32],
    ) -> Result<Vec<HrpdTrafficEvent>, TrafficFrameError> {
        let Some(mac_packet) = self.decode_frame(samples) else {
            return Ok(Vec::new());
        };
        stream1_events_from_mac_packet(uati, mac_index, &mac_packet)
    }

    pub fn decode_events_for_reverse_mac_subtype(
        &self,
        uati: u32,
        mac_index: u8,
        samples: &[Complex32],
        reverse_traffic_mac_subtype: u16,
    ) -> Result<Vec<HrpdTrafficEvent>, TrafficFrameError> {
        let Some(mac_packet) = self.decode_frame(samples) else {
            return Ok(Vec::new());
        };
        traffic_events_from_mac_packet_for_reverse_mac_subtype(
            uati,
            mac_index,
            &mac_packet,
            reverse_traffic_mac_subtype,
        )
    }
}

pub fn traffic_events_from_mac_packet(
    uati: u32,
    mac_index: u8,
    mac_packet_bits: &[u8],
) -> Result<Vec<HrpdTrafficEvent>, TrafficFrameError> {
    traffic_events_from_mac_packet_for_reverse_mac_subtype(uati, mac_index, mac_packet_bits, 0)
}

pub fn traffic_events_from_mac_packet_for_reverse_mac_subtype(
    uati: u32,
    mac_index: u8,
    mac_packet_bits: &[u8],
    reverse_traffic_mac_subtype: u16,
) -> Result<Vec<HrpdTrafficEvent>, TrafficFrameError> {
    let mac =
        parse_reverse_traffic_mac_packet_for_subtype(mac_packet_bits, reverse_traffic_mac_subtype)?;

    let mut out = Vec::new();
    for session_packet in
        parse_connection_layer_packets(mac.connection_layer_format_b, &mac.security_payload_bits)?
    {
        let stream = parse_stream_layer_packet_bytes(&session_packet)?;
        match stream.stream_id {
            0 => out.push(HrpdTrafficEvent::Stream0Signaling {
                uati,
                payload: session_packet,
            }),
            DEFAULT_PACKET_STREAM_ID | DEFAULT_PACKET_STREAM2_ID => {
                let packet = parse_default_packet_rlp_packet_bits(&stream.application_packet_bits)?;
                if !packet.payload.is_empty() {
                    out.push(HrpdTrafficEvent::Stream1Packet {
                        uati,
                        sequence: packet.sequence,
                        payload: packet.payload,
                        decoded_at: Some(std::time::Instant::now()),
                        air_frame_end_received_at: None,
                    });
                }
            }
            _ => {
                log::debug!(
                    "HRPD reverse traffic: ignoring unsupported stream {} for UATI=0x{:08x} MAC={}",
                    stream.stream_id,
                    uati,
                    mac_index
                );
            }
        }
    }
    Ok(out)
}

pub fn stream1_events_from_mac_packet(
    uati: u32,
    _mac_index: u8,
    mac_packet_bits: &[u8],
) -> Result<Vec<HrpdTrafficEvent>, TrafficFrameError> {
    Ok(parse_reverse_stream1_packets(mac_packet_bits)?
        .into_iter()
        .filter(|packet| !packet.payload.is_empty())
        .map(|packet| HrpdTrafficEvent::Stream1Packet {
            uati,
            sequence: packet.sequence,
            payload: packet.payload,
            decoded_at: Some(std::time::Instant::now()),
            air_frame_end_received_at: None,
        })
        .collect())
}

/// Decoded reverse-data PHY frame. `crc_ok` is only true after the physical
/// packet FCS has been checked; unverified turbo output is not exposed as a
/// usable payload.
#[derive(Debug, Clone, PartialEq)]
pub struct DataChannelFrame {
    pub rate: ReverseDataRate,
    /// Full decoded physical-layer packet bits. This is exposed for
    /// diagnostics; callers must check `crc_ok` before consuming `payload`.
    pub physical_bits: Vec<u8>,
    /// Validated Reverse Traffic Channel MAC-layer packet bits. Empty when
    /// the physical FCS or tail check fails.
    pub payload: Vec<u8>,
    pub crc_ok: bool,
    pub expected_fcs: u32,
    pub observed_fcs: u32,
    pub tail_ones: usize,
}

/// End-to-end Reverse Data Channel decoder wrapper exposing a single
/// `decode_data_frame` entry point. Pipeline: BPSK-Q demap → walsh decover
/// (W_2^4) → channel deinterleave (sized by `encoder_block_symbols`) →
/// depuncture to mother-rate-1/5 LLRs → turbo decode → physical FCS/tail
/// check. The Rev 0 reverse link has no data scrambling stage.
#[derive(Debug, Clone)]
pub struct ReverseDataDecoder {
    rate: ReverseDataRate,
    physical_layer_subtype: u16,
}

impl ReverseDataDecoder {
    pub fn new(rate: ReverseDataRate) -> Self {
        Self {
            rate,
            physical_layer_subtype: 0,
        }
    }

    pub fn for_physical_layer_subtype(rate: ReverseDataRate, physical_layer_subtype: u16) -> Self {
        Self {
            rate,
            physical_layer_subtype,
        }
    }

    pub fn rate(&self) -> ReverseDataRate {
        self.rate
    }

    pub fn physical_layer_subtype(&self) -> u16 {
        self.physical_layer_subtype
    }

    pub fn decode_data_frame(&self, samples: &[Complex32]) -> DataChannelFrame {
        self.decode_data_frame_with_timing(samples, 0, 0)
    }

    pub fn decode_data_frame_with_timing(
        &self,
        samples: &[Complex32],
        frame_start_slot: u64,
        frame_offset: u8,
    ) -> DataChannelFrame {
        if self.physical_layer_subtype == 2 {
            return self.decode_subtype2_b4_data_frame(samples, frame_start_slot, frame_offset);
        }

        let block = self.rate.encoder_block_symbols() as usize;
        let soft_chips = demap_bpsk_q(samples);
        let soft_symbols =
            walsh_decover(&soft_chips, DATA_WALSH_LEN, usize::from(DATA_WALSH_INDEX));

        // Trim to encoder block; bail out as crc_ok=false if too short.
        if soft_symbols.len() < block {
            return DataChannelFrame {
                rate: self.rate,
                physical_bits: Vec::new(),
                payload: Vec::new(),
                crc_ok: false,
                expected_fcs: 0,
                observed_fcs: 0,
                tail_ones: 0,
            };
        }
        let Some(combined) = combine_sequence_repetitions(&soft_symbols, block) else {
            return DataChannelFrame {
                rate: self.rate,
                physical_bits: Vec::new(),
                payload: Vec::new(),
                crc_ok: false,
                expected_fcs: 0,
                observed_fcs: 0,
                tail_ones: 0,
            };
        };
        let deinterleaved = soft_block_deinterleave(combined, block);
        let mother_llrs = depuncture_to_mother_rate_1_5(&deinterleaved, self.rate);
        match turbo_decode(
            &mother_llrs,
            self.rate.payload_bits(),
            self.rate.code_rate(),
        ) {
            Some(bits) => {
                data_channel_frame_from_bits(self.rate, self.physical_layer_subtype, bits)
            }
            None => DataChannelFrame {
                rate: self.rate,
                physical_bits: Vec::new(),
                payload: Vec::new(),
                crc_ok: false,
                expected_fcs: 0,
                observed_fcs: 0,
                tail_ones: 0,
            },
        }
    }

    fn decode_subtype2_b4_data_frame(
        &self,
        samples: &[Complex32],
        frame_start_slot: u64,
        frame_offset: u8,
    ) -> DataChannelFrame {
        let packet_bits = self.rate.payload_bits() as usize;
        // This decoder implements the subtype-2 B4 formats. Higher payload
        // sizes use Q4/Q2/E4E2 and need separate demappers.
        if !matches!(packet_bits, 256 | 512 | 1024) {
            return empty_data_channel_frame(self.rate);
        }
        let block = self
            .rate
            .encoder_block_symbols_for_physical_layer_subtype(self.physical_layer_subtype)
            as usize;
        let soft_chips = demap_bpsk_q(samples);
        let soft_symbols =
            walsh_decover(&soft_chips, DATA_WALSH_LEN, usize::from(DATA_WALSH_INDEX));
        if soft_symbols.len() < REVERSE_DATA_SUBFRAME_SYMBOLS {
            return empty_data_channel_frame(self.rate);
        }

        let mut best = empty_data_channel_frame(self.rate);
        for start_subframe in 0..4usize {
            let start_symbol = start_subframe * REVERSE_DATA_SUBFRAME_SYMBOLS;
            if start_symbol + REVERSE_DATA_SUBFRAME_SYMBOLS > soft_symbols.len() {
                continue;
            }
            let packet_start_slot = frame_start_slot + (start_subframe as u64 * 4);
            let interlace_offset =
                reverse_link_interlace_offset(packet_start_slot, u64::from(frame_offset));
            let available = subtype2_interlaced_subframe_indices(start_subframe);
            for count in 1..=available.len() {
                let combined =
                    combine_subtype2_sequence_selection(&soft_symbols, block, &available[..count]);
                let deinterleaved = subtype2_rate_1_5_deinterleave_b4(combined, packet_bits);
                let descrambled =
                    subtype2_reverse_descramble_llrs(deinterleaved, packet_bits, interlace_offset);
                if let Some(bits) = turbo_decode(
                    &descrambled,
                    self.rate.payload_bits(),
                    self.rate
                        .code_rate_for_physical_layer_subtype(self.physical_layer_subtype),
                ) {
                    let frame =
                        data_channel_frame_from_bits(self.rate, self.physical_layer_subtype, bits);
                    if frame.crc_ok {
                        return frame;
                    }
                    if best.physical_bits.is_empty() {
                        best = frame;
                    }
                }
            }
        }
        best
    }
}

/// Reverse traffic keeps a fixed 307.2-ksps Data Channel. For rates below
/// 76.8 kbps, §9.2.1.3.6 repeats the whole interleaved encoder-output block
/// across the frame. Combine those repetitions in soft space before
/// deinterleaving so the turbo decoder sees the full processing gain.
fn combine_sequence_repetitions(soft_symbols: &[f32], block: usize) -> Option<Vec<f32>> {
    if soft_symbols.len() < block {
        return None;
    }
    let usable = soft_symbols.len().min(REVERSE_DATA_FRAME_SYMBOLS);
    let repeats = (usable / block).max(1);
    let mut combined = vec![0.0f32; block];
    for chunk in soft_symbols[..repeats * block].chunks_exact(block) {
        for (dst, src) in combined.iter_mut().zip(chunk) {
            *dst += *src;
        }
    }
    Some(combined)
}

fn combine_subtype2_sequence_selection(
    frame_soft_symbols: &[f32],
    block: usize,
    subframes: &[usize],
) -> Vec<f32> {
    let mut combined = vec![0.0f32; block];
    for (packet_subframe_idx, &frame_subframe_idx) in subframes.iter().enumerate() {
        let start = frame_subframe_idx * REVERSE_DATA_SUBFRAME_SYMBOLS;
        let end = start + REVERSE_DATA_SUBFRAME_SYMBOLS;
        if end > frame_soft_symbols.len() {
            continue;
        }
        for (j, &soft) in frame_soft_symbols[start..end].iter().enumerate() {
            let k = (j + packet_subframe_idx * REVERSE_DATA_SUBFRAME_SYMBOLS) % block;
            combined[k] += soft;
        }
    }
    combined
}

fn subtype2_interlaced_subframe_indices(start_subframe: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut idx = start_subframe;
    while idx < 4 {
        out.push(idx);
        idx += 3;
    }
    out
}

fn reverse_link_interlace_offset(packet_start_slot: u64, frame_offset: u64) -> u8 {
    (((packet_start_slot.saturating_sub(frame_offset)) / 4) % 3) as u8
}

fn subtype2_rate_1_5_deinterleave_b4(interleaved: Vec<f32>, packet_bits: usize) -> Vec<f32> {
    debug_assert_eq!(interleaved.len(), packet_bits * 5);
    debug_assert!(packet_bits.is_power_of_two());
    let (u_part, rest) = interleaved.split_at(packet_bits);
    let (v0_v0p_part, v1_v1p_part) = rest.split_at(packet_bits * 2);

    let u = bit_reversal_deinterleave_f32(u_part.to_vec(), packet_bits);
    let v0_v0p = bit_reversal_deinterleave_f32(v0_v0p_part.to_vec(), packet_bits * 2);
    let v1_v1p = bit_reversal_deinterleave_f32(v1_v1p_part.to_vec(), packet_bits * 2);
    let (v0, v0p) = v0_v0p.split_at(packet_bits);
    let (v1, v1p) = v1_v1p.split_at(packet_bits);

    let mut out = Vec::with_capacity(packet_bits * 5);
    for idx in 0..packet_bits {
        out.extend_from_slice(&[u[idx], v0[idx], v1[idx], v0p[idx], v1p[idx]]);
    }
    out
}

fn subtype2_reverse_descramble_llrs(
    mut scrambled: Vec<f32>,
    packet_bits: usize,
    interlace_offset: u8,
) -> Vec<f32> {
    let payload_code = subtype2_payload_size_code(packet_bits);
    let state =
        (0x7ffu32 << 6) | ((u32::from(interlace_offset & 0x03)) << 4) | u32::from(payload_code);
    let mut scrambler = HrpdForwardScrambler::with_initial_state(state);
    for llr in &mut scrambled {
        if scrambler.next_bit() {
            *llr = -*llr;
        }
    }
    scrambled
}

fn subtype2_payload_size_code(packet_bits: usize) -> u8 {
    match packet_bits {
        128 => 0x0,
        256 => 0x1,
        512 => 0x2,
        768 => 0x3,
        1024 => 0x4,
        1536 => 0x5,
        2048 => 0x6,
        3072 => 0x7,
        4096 => 0x8,
        6144 => 0x9,
        8192 => 0xa,
        12288 => 0xb,
        _ => 0xf,
    }
}

fn empty_data_channel_frame(rate: ReverseDataRate) -> DataChannelFrame {
    DataChannelFrame {
        rate,
        physical_bits: Vec::new(),
        payload: Vec::new(),
        crc_ok: false,
        expected_fcs: 0,
        observed_fcs: 0,
        tail_ones: 0,
    }
}

fn data_channel_frame_from_bits(
    rate: ReverseDataRate,
    physical_layer_subtype: u16,
    bits: Vec<u8>,
) -> DataChannelFrame {
    let packet_bits = rate.payload_bits() as usize;
    if bits.len() != packet_bits {
        return DataChannelFrame {
            rate,
            physical_bits: bits,
            payload: Vec::new(),
            crc_ok: false,
            expected_fcs: 0,
            observed_fcs: 0,
            tail_ones: 0,
        };
    }

    let fcs_bits = reverse_data_fcs_bits_for_physical_layer_subtype(physical_layer_subtype);
    let mac_bits = rate.mac_packet_bits_for_physical_layer_subtype(physical_layer_subtype) as usize;
    let fcs_start = mac_bits;
    let fcs_end = fcs_start + fcs_bits;
    let observed_fcs = reverse_data_physical_fcs(physical_layer_subtype, &bits[..mac_bits]);
    let expected_fcs = pack_u32_msb(&bits[fcs_start..fcs_end]);
    let tail_ones = bits[fcs_end..].iter().filter(|&&bit| bit != 0).count();
    let crc_ok = observed_fcs == expected_fcs && tail_ones == 0;
    let payload = if crc_ok {
        bits[..mac_bits].to_vec()
    } else {
        Vec::new()
    };

    DataChannelFrame {
        rate,
        physical_bits: bits,
        payload,
        crc_ok,
        expected_fcs,
        observed_fcs,
        tail_ones,
    }
}

/// Build one Reverse Traffic Channel PHY packet from a MAC-layer packet.
///
/// The result is `MAC packet | 16-bit physical FCS | 6 zero tail bits`, per
/// C.S0024-200-C §1.2.2.4. This helper deliberately stops before turbo
/// encoding so tests and higher layers can keep the PHY packet format
/// distinct from modulation.
pub fn build_reverse_data_phy_bits(rate: ReverseDataRate, mac_bits: &[u8]) -> Vec<u8> {
    build_reverse_data_phy_bits_for_physical_layer_subtype(rate, 0, mac_bits)
}

pub fn build_reverse_data_phy_bits_for_physical_layer_subtype(
    rate: ReverseDataRate,
    physical_layer_subtype: u16,
    mac_bits: &[u8],
) -> Vec<u8> {
    assert_eq!(
        mac_bits.len(),
        rate.mac_packet_bits_for_physical_layer_subtype(physical_layer_subtype) as usize,
        "reverse data MAC packet length must match selected rate",
    );
    let mut out = Vec::with_capacity(rate.payload_bits() as usize);
    out.extend(mac_bits.iter().map(|b| b & 1));
    push_fcs_msb(
        &mut out,
        reverse_data_fcs_bits_for_physical_layer_subtype(physical_layer_subtype),
        reverse_data_physical_fcs(physical_layer_subtype, mac_bits),
    );
    out.extend(std::iter::repeat_n(0u8, REVERSE_DATA_TAIL_BITS));
    debug_assert_eq!(out.len(), rate.payload_bits() as usize);
    out
}

fn reverse_data_fcs_bits_for_physical_layer_subtype(physical_layer_subtype: u16) -> usize {
    if physical_layer_subtype == 2 {
        REVERSE_DATA_SUBTYPE2_FCS_BITS
    } else {
        REVERSE_DATA_FCS_BITS
    }
}

fn reverse_data_physical_fcs(physical_layer_subtype: u16, bits: &[u8]) -> u32 {
    if physical_layer_subtype == 2 {
        physical_crc24(bits)
    } else {
        u32::from(physical_crc16(bits))
    }
}

fn push_fcs_msb(bits: &mut Vec<u8>, fcs_bits: usize, value: u32) {
    for shift in (0..fcs_bits).rev() {
        bits.push(((value >> shift) & 1) as u8);
    }
}

fn pack_u32_msb(bits: &[u8]) -> u32 {
    bits.iter()
        .fold(0u32, |acc, &bit| (acc << 1) | u32::from(bit & 1))
}

/// Depuncture a rate-matched reverse-link block back to the mother-rate 1/5
/// LLR layout consumed by [`HrpdTurboDecoder`]. Missing punctured positions
/// are inserted as zero-LLR erasures.
pub fn depuncture_to_mother_rate_1_5(rate_matched: &[f32], rate: ReverseDataRate) -> Vec<f32> {
    depuncture_to_mother_rate_1_5_for_physical_layer_subtype(rate_matched, rate, 0)
}

fn depuncture_to_mother_rate_1_5_for_physical_layer_subtype(
    rate_matched: &[f32],
    rate: ReverseDataRate,
    physical_layer_subtype: u16,
) -> Vec<f32> {
    let payload_bits = rate.payload_bits() as usize;
    let n_turbo = payload_bits
        .checked_sub(REVERSE_DATA_TAIL_BITS)
        .expect("payload includes tail");
    assert_eq!(
        rate_matched.len(),
        rate.encoder_block_symbols_for_physical_layer_subtype(physical_layer_subtype) as usize,
        "rate-matched stream length must match selected reverse data rate",
    );
    assert_eq!(
        n_turbo % 2,
        0,
        "reverse data puncturing tables are read over pairs of data bit periods",
    );

    match rate.code_rate_for_physical_layer_subtype(physical_layer_subtype) {
        (1, 5) => rate_matched.to_vec(),
        (1, 4) => depuncture_rate_1_4(rate_matched, payload_bits, n_turbo),
        (1, 2) => depuncture_rate_1_2(rate_matched, payload_bits, n_turbo),
        // code_rate_for_physical_layer_subtype only returns the three rates above.
        other => unreachable!("unsupported reverse data code rate {other:?}"),
    }
}

fn depuncture_rate_1_4(rate14: &[f32], payload_bits: usize, n_turbo: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(payload_bits * 5);
    let data_symbols = n_turbo * 4;
    let mut idx = 0usize;
    while idx < data_symbols {
        // First bit period in each pair: [X,Y0,Y1,Y'1].
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            rate14[idx + 2],
            0.0,
            rate14[idx + 3],
        ]);
        idx += 4;
        // Second bit period: [X,Y0,Y'0,Y'1].
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            0.0,
            rate14[idx + 2],
            rate14[idx + 3],
        ]);
        idx += 4;
    }

    // CE1 tail: [X,X,Y0,Y1] -> mother-rate tail cell [X,X,Y0,Y1,erasure].
    for _ in 0..3 {
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            rate14[idx + 2],
            rate14[idx + 3],
            0.0,
        ]);
        idx += 4;
    }
    // CE2 tail: [X',X',Y'0,Y'1] -> second half of mother-rate tail section.
    for _ in 0..3 {
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            rate14[idx + 2],
            rate14[idx + 3],
            0.0,
        ]);
        idx += 4;
    }
    debug_assert_eq!(idx, rate14.len());
    debug_assert_eq!(out.len(), payload_bits * 5);
    out
}

fn depuncture_rate_1_2(rate12: &[f32], payload_bits: usize, n_turbo: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(payload_bits * 5);
    let data_symbols = n_turbo * 2;
    let mut idx = 0usize;
    while idx < data_symbols {
        // First bit period in each pair: [X,Y0].
        out.extend_from_slice(&[rate12[idx], rate12[idx + 1], 0.0, 0.0, 0.0]);
        idx += 2;
        // Second bit period: [X,Y'0].
        out.extend_from_slice(&[rate12[idx], 0.0, 0.0, rate12[idx + 1], 0.0]);
        idx += 2;
    }

    // CE1 tail: [X,Y0].
    for _ in 0..3 {
        out.extend_from_slice(&[rate12[idx], 0.0, rate12[idx + 1], 0.0, 0.0]);
        idx += 2;
    }
    // CE2 tail: [X',Y'0].
    for _ in 0..3 {
        out.extend_from_slice(&[rate12[idx], 0.0, rate12[idx + 1], 0.0, 0.0]);
        idx += 2;
    }
    debug_assert_eq!(idx, rate12.len());
    debug_assert_eq!(out.len(), payload_bits * 5);
    out
}

/// Soft-block deinterleave; thin wrapper over `bit_reversal_deinterleave_f32`.
fn soft_block_deinterleave(soft_symbols: Vec<f32>, block_size: usize) -> Vec<f32> {
    bit_reversal_deinterleave_f32(soft_symbols, block_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_walsh_cover_matches_spec_9_2_1_3_3_5() {
        assert_eq!(DATA_WALSH_LEN, 4);
        assert_eq!(DATA_WALSH_INDEX, 2);
    }

    #[test]
    fn payload_bits_match_spec_table_9_2_1_3_1_1_1() {
        assert_eq!(ReverseDataRate::Kbps9_6.payload_bits(), 256);
        assert_eq!(ReverseDataRate::Kbps19_2.payload_bits(), 512);
        assert_eq!(ReverseDataRate::Kbps38_4.payload_bits(), 1024);
        assert_eq!(ReverseDataRate::Kbps76_8.payload_bits(), 2048);
        assert_eq!(ReverseDataRate::Kbps153_6.payload_bits(), 4096);
    }

    #[test]
    fn mac_packet_bits_match_reverse_phy_packet_format() {
        assert_eq!(ReverseDataRate::Kbps9_6.mac_packet_bits(), 234);
        assert_eq!(ReverseDataRate::Kbps19_2.mac_packet_bits(), 490);
        assert_eq!(ReverseDataRate::Kbps38_4.mac_packet_bits(), 1002);
        assert_eq!(ReverseDataRate::Kbps76_8.mac_packet_bits(), 2026);
        assert_eq!(ReverseDataRate::Kbps153_6.mac_packet_bits(), 4074);
    }

    #[test]
    fn subtype2_mac_packet_bits_use_crc24_physical_fcs() {
        assert_eq!(
            ReverseDataRate::Kbps9_6.mac_packet_bits_for_physical_layer_subtype(2),
            226
        );
        assert_eq!(
            ReverseDataRate::Kbps19_2.mac_packet_bits_for_physical_layer_subtype(2),
            482
        );
        assert_eq!(
            ReverseDataRate::Kbps38_4.mac_packet_bits_for_physical_layer_subtype(2),
            994
        );
    }

    #[test]
    fn rri_index_matches_spec_table_9_2_1_3_3_2_1() {
        assert_eq!(ReverseDataRate::Kbps9_6.rri_index(), 1);
        assert_eq!(ReverseDataRate::Kbps19_2.rri_index(), 2);
        assert_eq!(ReverseDataRate::Kbps38_4.rri_index(), 3);
        assert_eq!(ReverseDataRate::Kbps76_8.rri_index(), 4);
        assert_eq!(ReverseDataRate::Kbps153_6.rri_index(), 5);
    }

    #[test]
    fn code_rate_matches_spec_table_9_2_1_3_4_1_1() {
        assert_eq!(ReverseDataRate::Kbps9_6.code_rate(), (1, 4));
        assert_eq!(ReverseDataRate::Kbps19_2.code_rate(), (1, 4));
        assert_eq!(ReverseDataRate::Kbps38_4.code_rate(), (1, 4));
        assert_eq!(ReverseDataRate::Kbps76_8.code_rate(), (1, 4));
        assert_eq!(ReverseDataRate::Kbps153_6.code_rate(), (1, 2));
    }

    #[test]
    fn from_rri_round_trips_known_indices() {
        for rate in [
            ReverseDataRate::Kbps9_6,
            ReverseDataRate::Kbps19_2,
            ReverseDataRate::Kbps38_4,
            ReverseDataRate::Kbps76_8,
            ReverseDataRate::Kbps153_6,
        ] {
            assert_eq!(ReverseDataRate::from_rri(rate.rri_index()), Some(rate));
        }
        assert_eq!(ReverseDataRate::from_rri(0), None);
        assert_eq!(ReverseDataRate::from_rri(6), None);
        assert_eq!(ReverseDataRate::from_rri(7), None);
    }

    /// BPSK polarity: per §9.2.1.3.1 / Figure 9.2.1.3.1-3 the Q-arm
    /// transmitter maps bit 0 → +1 and bit 1 → −1, so a received Q value
    /// of +1 must demap to a positive LLR (bit 0) and −1 to a negative
    /// LLR (bit 1).
    #[test]
    fn bpsk_q_demap_polarity_matches_spec_9_2_1_3_1() {
        let samples = [
            Complex32::new(0.0, 1.0),
            Complex32::new(0.0, -1.0),
            Complex32::new(123.0, 0.5),
        ];
        let soft = demap_bpsk_q(&samples);
        assert!(soft[0] > 0.0, "Q=+1 must produce positive LLR (bit 0)");
        assert!(soft[1] < 0.0, "Q=-1 must produce negative LLR (bit 1)");
        assert_eq!(soft[2], 0.5);
    }

    /// Walsh decover round-trip: spread a known soft-symbol stream with
    /// the Walsh row pattern, then despread and confirm the original
    /// values are recovered (up to a non-negative scale).
    #[test]
    fn walsh_decover_recovers_spread_symbols() {
        let symbols = [1.0f32, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
        let row = walsh_row_pattern(DATA_WALSH_LEN, usize::from(DATA_WALSH_INDEX));
        // Spread each symbol over the Walsh row.
        let mut chips: Vec<f32> = Vec::with_capacity(symbols.len() * DATA_WALSH_LEN);
        for s in &symbols {
            for &c in &row {
                chips.push(s * c as f32);
            }
        }
        let recovered = walsh_decover(&chips, DATA_WALSH_LEN, usize::from(DATA_WALSH_INDEX));
        assert_eq!(recovered.len(), symbols.len());
        for (r, s) in recovered.iter().zip(symbols.iter()) {
            // Decover sums DATA_WALSH_LEN chips; row * row = +1 per chip.
            assert!((r - s * DATA_WALSH_LEN as f32).abs() < 1e-6);
        }
    }

    /// Deinterleave soft-symbol variant must invert `channel_interleave`.
    #[test]
    fn deinterleave_inverts_channel_interleave() {
        let block_size = 64usize;
        let bits: Vec<u8> = (0..block_size).map(|i| (i & 1) as u8).collect();
        let interleaved_bits = channel_interleave(block_size, &bits);
        // u8 round-trip sanity (matches existing channel module tests).
        let recovered_bits = channel_deinterleave(block_size, &interleaved_bits);
        assert_eq!(recovered_bits, bits);

        // Map bits → ±1.0 soft symbols, run through soft deinterleave.
        let soft_interleaved: Vec<f32> = interleaved_bits
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        let recovered_soft = deinterleave(soft_interleaved, block_size);
        let expected_soft: Vec<f32> = bits
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        assert_eq!(recovered_soft, expected_soft);
    }

    #[test]
    fn data_decoder_returns_none_for_fcs_corrupt_frame() {
        let rate = ReverseDataRate::Kbps9_6;
        let mac = deterministic_mac_bits(rate.mac_packet_bits() as usize);
        let mut frame_bits = build_reverse_data_phy_bits(rate, &mac);
        frame_bits[rate.mac_packet_bits() as usize] ^= 1;
        let samples = encode_reverse_data_physical_samples(rate, &frame_bits);
        let dec = DataDecoder::new(rate);
        assert!(dec.decode_frame(&samples).is_none());
    }

    #[test]
    fn data_decoder_default_is_kbps9_6() {
        let d = DataDecoder::default();
        assert_eq!(d.rate(), ReverseDataRate::Kbps9_6);
    }

    #[test]
    fn encoder_block_symbols_table_matches_spec_9_2_1_3_4_1_1() {
        assert_eq!(ReverseDataRate::Kbps9_6.encoder_block_symbols(), 1024);
        assert_eq!(ReverseDataRate::Kbps19_2.encoder_block_symbols(), 2048);
        assert_eq!(ReverseDataRate::Kbps38_4.encoder_block_symbols(), 4096);
        assert_eq!(ReverseDataRate::Kbps76_8.encoder_block_symbols(), 8192);
        assert_eq!(ReverseDataRate::Kbps153_6.encoder_block_symbols(), 8192);
    }

    #[test]
    fn subtype2_encoder_block_symbols_match_spec_table_2_3_1_3_4_2() {
        assert_eq!(
            ReverseDataRate::Kbps9_6.encoder_block_symbols_for_physical_layer_subtype(2),
            1280
        );
        assert_eq!(
            ReverseDataRate::Kbps19_2.encoder_block_symbols_for_physical_layer_subtype(2),
            2560
        );
        assert_eq!(
            ReverseDataRate::Kbps38_4.encoder_block_symbols_for_physical_layer_subtype(2),
            5120
        );
    }

    #[test]
    fn depuncture_rate_1_4_matches_spec_puncturing_table() {
        let r = ReverseDataRate::Kbps9_6;
        let input = (1..=r.encoder_block_symbols())
            .map(|v| v as f32)
            .collect::<Vec<_>>();
        let out = depuncture_to_mother_rate_1_5(&input, r);
        assert_eq!(out.len(), (r.payload_bits() * 5) as usize);
        // Rate 1/4 data bit periods are read in pairs:
        // [X,Y0,Y1,Y'1], then [X,Y0,Y'0,Y'1].
        assert_eq!(
            &out[..10],
            &[1.0, 2.0, 3.0, 0.0, 4.0, 5.0, 6.0, 0.0, 7.0, 8.0]
        );
    }

    #[test]
    fn depuncture_rate_1_2_matches_spec_puncturing_table() {
        let r = ReverseDataRate::Kbps153_6;
        let input = (1..=r.encoder_block_symbols())
            .map(|v| v as f32)
            .collect::<Vec<_>>();
        let out = depuncture_to_mother_rate_1_5(&input, r);
        assert_eq!(out.len(), (r.payload_bits() * 5) as usize);
        // Rate 1/2 data bit periods are read in pairs:
        // [X,Y0], then [X,Y'0].
        assert_eq!(
            &out[..10],
            &[1.0, 2.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 4.0, 0.0]
        );
    }

    #[test]
    fn reverse_data_decoder_rejects_fcs_corrupt_frame() {
        let rate = ReverseDataRate::Kbps9_6;
        let mac = deterministic_mac_bits(rate.mac_packet_bits() as usize);
        let mut frame_bits = build_reverse_data_phy_bits(rate, &mac);
        frame_bits[rate.mac_packet_bits() as usize] ^= 1;
        let samples = encode_reverse_data_physical_samples(rate, &frame_bits);
        let dec = ReverseDataDecoder::new(rate);
        let frame = dec.decode_data_frame(&samples);
        assert_eq!(frame.rate, rate);
        assert!(!frame.crc_ok);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn reverse_data_decoder_round_trips_rate_1_4_packet() {
        let rate = ReverseDataRate::Kbps38_4;
        let mac = deterministic_mac_bits(rate.mac_packet_bits() as usize);
        let samples = encode_reverse_data_samples(rate, &mac);
        let frame = ReverseDataDecoder::new(rate).decode_data_frame(&samples);
        assert!(
            frame.crc_ok,
            "FCS mismatch expected=0x{:04x} observed=0x{:04x} tail_ones={}",
            frame.expected_fcs, frame.observed_fcs, frame.tail_ones
        );
        assert_eq!(frame.payload, mac);
        assert_eq!(frame.physical_bits.len(), rate.payload_bits() as usize);
    }

    #[test]
    fn reverse_data_decoder_round_trips_subtype2_crc24_packet() {
        let rate = ReverseDataRate::Kbps9_6;
        let physical_layer_subtype = 2;
        let mac = deterministic_mac_bits(
            rate.mac_packet_bits_for_physical_layer_subtype(physical_layer_subtype) as usize,
        );
        let frame_bits = build_reverse_data_phy_bits_for_physical_layer_subtype(
            rate,
            physical_layer_subtype,
            &mac,
        );
        let samples = encode_reverse_data_physical_samples_for_physical_layer_subtype(
            rate,
            physical_layer_subtype,
            &frame_bits,
        );
        let frame = ReverseDataDecoder::for_physical_layer_subtype(rate, physical_layer_subtype)
            .decode_data_frame_with_timing(&samples, 0, 0);

        assert!(
            frame.crc_ok,
            "FCS mismatch expected=0x{:06x} observed=0x{:06x} tail_ones={}",
            frame.expected_fcs, frame.observed_fcs, frame.tail_ones
        );
        assert_eq!(frame.payload, mac);
    }

    #[test]
    fn reverse_data_decoder_round_trips_rate_1_2_packet() {
        let rate = ReverseDataRate::Kbps153_6;
        let mac = deterministic_mac_bits(rate.mac_packet_bits() as usize);
        let samples = encode_reverse_data_samples(rate, &mac);
        let frame = ReverseDataDecoder::new(rate).decode_data_frame(&samples);
        assert!(
            frame.crc_ok,
            "FCS mismatch expected=0x{:04x} observed=0x{:04x} tail_ones={}",
            frame.expected_fcs, frame.observed_fcs, frame.tail_ones
        );
        assert_eq!(frame.payload, mac);
        assert_eq!(frame.physical_bits.len(), rate.payload_bits() as usize);
    }

    #[test]
    fn data_decoder_decode_frame_returns_valid_mac_bits() {
        let rate = ReverseDataRate::Kbps9_6;
        let mac = deterministic_mac_bits(rate.mac_packet_bits() as usize);
        let samples = encode_reverse_data_samples(rate, &mac);
        let decoded = DataDecoder::new(rate)
            .decode_frame(&samples)
            .expect("valid reverse data frame");
        assert_eq!(decoded, mac);
    }

    #[test]
    fn stream1_events_from_mac_packet_unwraps_default_packet_payload() {
        let mac = stream1_mac_bits(ReverseDataRate::Kbps9_6, 0x1f_f001, &[0x7e, 0xff, 0x03]);

        let events = stream1_events_from_mac_packet(0x8005_8001, 5, &mac).unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            HrpdTrafficEvent::Stream1Packet {
                uati: 0x8005_8001,
                sequence: 0x1f_f001,
                payload,
                decoded_at: Some(_),
                ..
            } if payload == &[0x7e, 0xff, 0x03]
        ));
    }

    #[test]
    fn traffic_events_from_subtype3_format_b_stream2_unwraps_default_packet_payload() {
        let mac = stream2_subtype3_mac_bits(
            ReverseDataRate::Kbps9_6,
            0x1f_f002,
            &[0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e],
        );

        let events = traffic_events_from_mac_packet_for_reverse_mac_subtype(
            0x8005_8001,
            5,
            &mac,
            cdma_common::hrpd::traffic::REVERSE_TRAFFIC_MAC_SUBTYPE3,
        )
        .unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            HrpdTrafficEvent::Stream1Packet {
                uati: 0x8005_8001,
                sequence: 0x1f_f002,
                payload,
                decoded_at: Some(_),
                ..
            } if payload == &[0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e]
        ));
    }

    #[test]
    fn traffic_events_from_mac_packet_accepts_format_a_stream0_tcc() {
        let rate = ReverseDataRate::Kbps9_6;
        let mut session_packet = cdma_common::hrpd::air::encode_default_signaling_packet(
            cdma_common::hrpd::air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x5a],
        );
        session_packet.resize((rate.mac_packet_bits() as usize - 2) / 8, 0);

        let mut mac = cdma_common::bits::Bitstream::new_bytes(&session_packet)
            .bits()
            .to_vec();
        mac.push(0); // ConnectionLayerFormat = Format A.
        mac.push(1); // MACLayerFormat = valid payload.

        let events = traffic_events_from_mac_packet(0x8005_8001, 5, &mac).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            HrpdTrafficEvent::Stream0Signaling {
                uati: 0x8005_8001,
                payload: session_packet,
            }
        );
    }

    #[test]
    fn data_decoder_decode_stream1_events_requires_valid_physical_fcs() {
        let rate = ReverseDataRate::Kbps9_6;
        let mac = stream1_mac_bits(rate, 0x22_1100, &[0x7e, 0xc0, 0x21, 0x7e]);
        let samples = encode_reverse_data_samples(rate, &mac);

        let events = DataDecoder::new(rate)
            .decode_stream1_events(0x8005_8001, 5, &samples)
            .unwrap();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            HrpdTrafficEvent::Stream1Packet {
                uati: 0x8005_8001,
                sequence: 0x22_1100,
                payload,
                decoded_at: Some(_),
                ..
            } if payload == &[0x7e, 0xc0, 0x21, 0x7e]
        ));
    }

    fn deterministic_mac_bits(len: usize) -> Vec<u8> {
        let mut s = 0x1357_2468u32;
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((s >> 31) & 1) as u8
            })
            .collect()
    }

    fn stream1_mac_bits(rate: ReverseDataRate, seq: u32, payload: &[u8]) -> Vec<u8> {
        let rlp = cdma_common::hrpd::traffic::default_packet_rlp_packet_bits(seq, payload);
        let stream = cdma_common::hrpd::traffic::stream_layer_packet_bytes(
            cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM_ID,
            &rlp,
        )
        .unwrap();
        let security_capacity = rate.mac_packet_bits() as usize - 2;
        let mut mac =
            cdma_common::hrpd::traffic::connection_format_b_bits(&[stream], security_capacity)
                .unwrap();
        mac.push(1);
        mac.push(1);
        mac
    }

    fn stream2_subtype3_mac_bits(rate: ReverseDataRate, seq: u32, payload: &[u8]) -> Vec<u8> {
        let rlp = cdma_common::hrpd::traffic::default_packet_rlp_packet_bits(seq, payload);
        let stream = cdma_common::hrpd::traffic::stream_layer_packet_bytes(
            cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_ID,
            &rlp,
        )
        .unwrap();
        let security_capacity = rate.mac_packet_bits() as usize - 2;
        let mut mac =
            cdma_common::hrpd::traffic::connection_format_b_bits(&[stream], security_capacity)
                .unwrap();
        mac.push(1); // ConnectionLayerFormat = Format B.
        mac.push(0); // TransmissionMode = High Capacity.
        mac
    }

    fn encode_reverse_data_samples(rate: ReverseDataRate, mac_bits: &[u8]) -> Vec<Complex32> {
        let frame_bits = build_reverse_data_phy_bits(rate, mac_bits);
        encode_reverse_data_physical_samples(rate, &frame_bits)
    }

    fn encode_reverse_data_physical_samples(
        rate: ReverseDataRate,
        frame_bits: &[u8],
    ) -> Vec<Complex32> {
        encode_reverse_data_physical_samples_for_physical_layer_subtype(rate, 0, frame_bits)
    }

    fn encode_reverse_data_physical_samples_for_physical_layer_subtype(
        rate: ReverseDataRate,
        physical_layer_subtype: u16,
        frame_bits: &[u8],
    ) -> Vec<Complex32> {
        if physical_layer_subtype == 2 {
            return encode_subtype2_reverse_data_physical_samples(rate, frame_bits);
        }

        assert_eq!(frame_bits.len(), rate.payload_bits() as usize);
        let (_, den) = rate.code_rate();
        let encoder = HrpdTurboEncoder::new(rate.payload_bits()).expect("reverse data turbo block");
        let coded = encoder.encode(frame_bits, 1, den);
        assert_eq!(coded.len(), rate.encoder_block_symbols() as usize);
        let interleaved = channel_interleave(coded.len(), &coded);
        let soft = interleaved
            .iter()
            .map(|&bit| if bit == 0 { 4.0 } else { -4.0 })
            .collect::<Vec<_>>();

        let row = walsh_row_pattern(DATA_WALSH_LEN, usize::from(DATA_WALSH_INDEX));
        let mut chips = Vec::with_capacity(soft.len() * DATA_WALSH_LEN);
        for symbol in soft {
            for &w in &row {
                chips.push(Complex32::new(0.0, symbol * f32::from(w)));
            }
        }
        chips
    }

    fn encode_subtype2_reverse_data_physical_samples(
        rate: ReverseDataRate,
        frame_bits: &[u8],
    ) -> Vec<Complex32> {
        assert_eq!(frame_bits.len(), rate.payload_bits() as usize);
        let encoder = HrpdTurboEncoder::new(rate.payload_bits()).expect("reverse data turbo block");
        let mut coded = encoder.encode(frame_bits, 1, 5);
        assert_eq!(
            coded.len(),
            rate.encoder_block_symbols_for_physical_layer_subtype(2) as usize
        );

        // Test packet starts at slot 0 with FrameOffset 0, so the reverse
        // interlace offset bits are zero.
        let mut scrambler = HrpdForwardScrambler::with_initial_state(
            (0x7ffu32 << 6) | u32::from(subtype2_payload_size_code(rate.payload_bits() as usize)),
        );
        scrambler.apply_bits(&mut coded);

        let interleaved =
            subtype2_rate_1_5_interleave_b4_bits(&coded, rate.payload_bits() as usize);
        let mut soft = vec![0.0f32; REVERSE_DATA_FRAME_SYMBOLS];
        for j in 0..REVERSE_DATA_SUBFRAME_SYMBOLS {
            soft[j] = if interleaved[j % interleaved.len()] == 0 {
                4.0
            } else {
                -4.0
            };
        }

        let row = walsh_row_pattern(DATA_WALSH_LEN, usize::from(DATA_WALSH_INDEX));
        let mut chips = Vec::with_capacity(soft.len() * DATA_WALSH_LEN);
        for symbol in soft {
            for &w in &row {
                chips.push(Complex32::new(0.0, symbol * f32::from(w)));
            }
        }
        chips
    }

    fn subtype2_rate_1_5_interleave_b4_bits(coded: &[u8], packet_bits: usize) -> Vec<u8> {
        assert_eq!(coded.len(), packet_bits * 5);
        let mut u = Vec::with_capacity(packet_bits);
        let mut v0 = Vec::with_capacity(packet_bits);
        let mut v1 = Vec::with_capacity(packet_bits);
        let mut v0p = Vec::with_capacity(packet_bits);
        let mut v1p = Vec::with_capacity(packet_bits);

        for chunk in coded.chunks_exact(5) {
            u.push(chunk[0]);
            v0.push(chunk[1]);
            v1.push(chunk[2]);
            v0p.push(chunk[3]);
            v1p.push(chunk[4]);
        }

        let u = channel_interleave(packet_bits, &u);
        let v0_v0p = channel_interleave(packet_bits * 2, &[v0, v0p].concat());
        let v1_v1p = channel_interleave(packet_bits * 2, &[v1, v1p].concat());
        [u, v0_v0p, v1_v1p].concat()
    }
}
