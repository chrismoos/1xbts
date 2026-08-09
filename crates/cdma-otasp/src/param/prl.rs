//! Preferred Roaming List (classic SSPR_P_REV = 1) — C.S0016-D §3.5.5.
//!
//! Encoder/decoder for the on-wire PRL the BS reassembles across SSPR
//! Configuration Response segments and re-assembles when pushed via
//! SSPR Download.
//!
//! Extended PRL (SSPR_P_REV ≥ 2) lives in a separate module so the
//! classic path stays focused.
//!
//! The system record list deliberately reuses the spec's wire-bit
//! parsing rules: NID is omitted unless `NID_INCL = 01`, PRI / ROAM_IND
//! are omitted when `PREF_NEG = 0`, etc. Field optionality is encoded
//! in the Rust types.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8, read_u16, read_u32};

// ---------------------------------------------------------------------------
// Spec-defined constants (C.S0016-D §3.5.5)
// ---------------------------------------------------------------------------

/// Bit widths and type codes from C.S0016-D §3.5.5.
mod wire {
    // Header field widths (§3.5.5 Preferred Roaming List table, SSPR_P_REV = 1)
    pub(super) const BITS_PR_LIST_SIZE: usize = 16;
    pub(super) const BITS_PR_LIST_ID: usize = 16;
    pub(super) const BITS_PREF_ONLY: usize = 1;
    pub(super) const BITS_DEF_ROAM_IND: usize = 8;
    pub(super) const BITS_NUM_ACQ_RECS: usize = 9;
    pub(super) const BITS_NUM_SYS_RECS: usize = 14;
    pub(super) const BITS_CRC: usize = 16;

    // 16+16+1+8+9+14 = 64 bits = 8 octets of header before any record;
    // plus the trailing CRC means PRL is at least ~10 octets minimum.
    pub(super) const MIN_PRL_BYTES: usize = 9;

    // Acquisition record framing (§3.5.5.2.1)
    pub(super) const BITS_ACQ_TYPE: usize = 4;
    pub(super) const BITS_AB: usize = 2;
    pub(super) const BITS_PRI_SEC: usize = 2;
    pub(super) const BITS_NUM_BLOCKS: usize = 3;
    pub(super) const BITS_PCS_BLOCK: usize = 3;
    pub(super) const BITS_NUM_CHANS: usize = 5;
    pub(super) const BITS_CHAN_NUMBER_11: usize = 11;

    // ACQ_TYPE values (Table 3.5.5.2-1)
    pub(super) const ACQ_TYPE_CELLULAR_ANALOG: u8 = 0x01;
    pub(super) const ACQ_TYPE_CELLULAR_CDMA_STANDARD: u8 = 0x02;
    pub(super) const ACQ_TYPE_CELLULAR_CDMA_CUSTOM: u8 = 0x03;
    pub(super) const ACQ_TYPE_CELLULAR_CDMA_PREFERRED: u8 = 0x04;
    pub(super) const ACQ_TYPE_PCS_CDMA_USING_BLOCKS: u8 = 0x05;
    pub(super) const ACQ_TYPE_PCS_CDMA_USING_CHANNELS: u8 = 0x06;
    pub(super) const ACQ_TYPE_JTACS_CDMA_STANDARD: u8 = 0x07;
    pub(super) const ACQ_TYPE_JTACS_CDMA_CUSTOM: u8 = 0x08;
    pub(super) const ACQ_TYPE_BAND_CLASS_6: u8 = 0x09;

    // System record framing (§3.5.5.3.1)
    pub(super) const BITS_SID: usize = 15;
    pub(super) const BITS_NID_INCL: usize = 2;
    pub(super) const BITS_NID: usize = 16;
    pub(super) const BITS_PREF_NEG: usize = 1;
    pub(super) const BITS_GEO: usize = 1;
    pub(super) const BITS_PRI: usize = 1;
    pub(super) const BITS_ACQ_INDEX: usize = 9;
    pub(super) const BITS_ROAM_IND: usize = 8;

    // C.R1001 roaming indicator values referenced by C.S0016-D.
    pub(super) const ROAM_IND_INDICATOR_ON: u8 = 0;
    pub(super) const ROAM_IND_INDICATOR_OFF: u8 = 1;
    pub(super) const ROAM_IND_INDICATOR_FLASHING: u8 = 2;
    pub(super) const ROAM_IND_OUT_OF_NEIGHBORHOOD: u8 = 3;
    pub(super) const ROAM_IND_OUT_OF_BUILDING: u8 = 4;
    pub(super) const ROAM_IND_PREFERRED_SYSTEM: u8 = 5;
    pub(super) const ROAM_IND_AVAILABLE_SYSTEM: u8 = 6;
    pub(super) const ROAM_IND_ALLIANCE_PARTNER: u8 = 7;
    pub(super) const ROAM_IND_PREMIUM_PARTNER: u8 = 8;
    pub(super) const ROAM_IND_FULL_SERVICE: u8 = 9;
    pub(super) const ROAM_IND_PARTIAL_SERVICE: u8 = 10;
    pub(super) const ROAM_IND_BANNER_ON: u8 = 11;
    pub(super) const ROAM_IND_BANNER_OFF: u8 = 12;
}

use wire::*;

pub(crate) const BITS_PER_OCTET: usize = 8;

/// Top-level decoded classic PRL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicPrl {
    /// Total size in octets including PR_LIST_SIZE and PR_LIST_CRC.
    pub pr_list_size: u16,
    pub pr_list_id: u16,
    pub pref_only: bool,
    /// Roaming indicator the MS should display when on a system not in
    /// SYS_TABLE. Decoded into [`RoamingIndicator`] (C.R1001 values).
    pub def_roam_ind: RoamingIndicator,
    pub acquisition_records: Vec<AcquisitionRecord>,
    pub system_records: Vec<SystemRecord>,
    /// CRC reported by the MS.
    pub pr_list_crc: u16,
    /// CRC the BS computed over the bytes (excluding the trailing
    /// CRC field itself).
    pub computed_crc: u16,
}

impl ClassicPrl {
    pub fn crc_ok(&self) -> bool {
        self.pr_list_crc == self.computed_crc
    }

    /// Encode back to on-wire bytes with a freshly computed CRC.
    /// Round-trip guarantee: `decode(b).encode() == b` for any
    /// spec-conformant input.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let max_acq = 1usize << BITS_NUM_ACQ_RECS;
        let max_sys = 1usize << BITS_NUM_SYS_RECS;
        if self.acquisition_records.len() >= max_acq {
            return Err("NUM_ACQ_RECS overflow".into());
        }
        if self.system_records.len() >= max_sys {
            return Err("NUM_SYS_RECS overflow".into());
        }
        let mut bs = Bitstream::new();
        // PR_LIST_SIZE placeholder, patched after sizing.
        bs.write_u32(0, BITS_PR_LIST_SIZE);
        bs.write_u32(self.pr_list_id as u32, BITS_PR_LIST_ID);
        bs.write_u8(self.pref_only as u8, BITS_PREF_ONLY);
        bs.write_u8(self.def_roam_ind.raw(), BITS_DEF_ROAM_IND);
        bs.write_u32(self.acquisition_records.len() as u32, BITS_NUM_ACQ_RECS);
        bs.write_u32(self.system_records.len() as u32, BITS_NUM_SYS_RECS);
        for r in &self.acquisition_records {
            r.encode_into(&mut bs)?;
        }
        for r in &self.system_records {
            r.encode_into(&mut bs)?;
        }
        // Trailing RESERVED so the CRC starts on an octet boundary.
        let pad = (BITS_PER_OCTET - (bs.len() + BITS_CRC) % BITS_PER_OCTET) % BITS_PER_OCTET;
        if pad != 0 {
            bs.write_u32(0, pad);
        }
        // CRC placeholder.
        bs.write_u32(0, BITS_CRC);

        let mut bytes = bs.to_packed_bytes();
        let total = bytes.len() as u16;
        bytes[0..2].copy_from_slice(&total.to_be_bytes());
        let crc = compute_prl_crc(&bytes);
        let n = bytes.len();
        bytes[n - 2..n].copy_from_slice(&crc.to_be_bytes());
        Ok(bytes)
    }
}

/// Tracks decode progress so EOF errors can name the failing stage,
/// the bit offset, and (when relevant) the first unknown ACQ_TYPE we
/// saw earlier in the table — classic records have no length field, so
/// an unknown type is usually the real cause of a downstream EOF.
struct DecodeCtx {
    total_bits: usize,
    first_unknown: Option<u8>,
}

impl DecodeCtx {
    fn new(total_bits: usize) -> Self {
        Self {
            total_bits,
            first_unknown: None,
        }
    }

    fn err(&self, stage: &str, bs: &Bitstream, inner: Error) -> Error {
        let pos = self.total_bits - bs.len();
        let mut msg = format!(
            "EOF decoding classic PRL: {} at bit offset {}/{} (octet {}): {}",
            stage,
            pos,
            self.total_bits,
            pos / BITS_PER_OCTET,
            inner
        );
        if let Some(raw) = self.first_unknown {
            msg.push_str(&format!(
                " — first unknown ACQ_TYPE was 0x{raw:02x}; \
                 classic records have no length field so an unknown type \
                 desynchronises the remaining records"
            ));
            if raw > 0x09 {
                msg.push_str(
                    " (raw exceeds 4-bit classic range — check SSPR_P_REV from PRL Dimensions; \
                     this may be an Extended PRL)",
                );
            }
        }
        msg.into()
    }
}

/// Parse the classic PRL on-wire bytes (as reassembled from segments).
///
/// Decode errors include the failing stage and bit offset. Classic
/// records have no length field, so the first unknown ACQ_TYPE
/// encountered is also reported.
pub fn decode(bytes: &[u8]) -> Result<ClassicPrl, Error> {
    if bytes.len() < MIN_PRL_BYTES {
        return Err(format!(
            "PRL too short: {} octets, need at least {}",
            bytes.len(),
            MIN_PRL_BYTES
        )
        .into());
    }
    let mut bs = from_bytes(bytes);
    let mut cx = DecodeCtx::new(bs.len());

    let pr_list_size =
        read_u16(&mut bs, BITS_PR_LIST_SIZE).map_err(|e| cx.err("PR_LIST_SIZE", &bs, e))?;
    let pr_list_id =
        read_u16(&mut bs, BITS_PR_LIST_ID).map_err(|e| cx.err("PR_LIST_ID", &bs, e))?;
    let pref_only = read_bool(&mut bs).map_err(|e| cx.err("PREF_ONLY", &bs, e))?;
    let def_roam_ind = RoamingIndicator::from_u8(
        read_u8(&mut bs, BITS_DEF_ROAM_IND).map_err(|e| cx.err("DEF_ROAM_IND", &bs, e))?,
    );
    let num_acq_recs =
        read_u16(&mut bs, BITS_NUM_ACQ_RECS).map_err(|e| cx.err("NUM_ACQ_RECS", &bs, e))? as usize;
    let num_sys_recs =
        read_u32(&mut bs, BITS_NUM_SYS_RECS).map_err(|e| cx.err("NUM_SYS_RECS", &bs, e))? as usize;

    let mut acquisition_records = Vec::with_capacity(num_acq_recs);
    for i in 0..num_acq_recs {
        let stage = format!("acquisition record {} of {}", i, num_acq_recs);
        let rec = AcquisitionRecord::decode(&mut bs).map_err(|e| cx.err(&stage, &bs, e))?;
        if cx.first_unknown.is_none() && matches!(rec.body, AcquisitionBody::Unknown) {
            cx.first_unknown = Some(rec.acq_type_raw);
        }
        acquisition_records.push(rec);
    }
    let mut system_records = Vec::with_capacity(num_sys_recs);
    for i in 0..num_sys_recs {
        let stage = format!("system record {} of {}", i, num_sys_recs);
        system_records.push(SystemRecord::decode(&mut bs).map_err(|e| cx.err(&stage, &bs, e))?);
    }
    // Skip the trailing RESERVED bit-padding so the CRC starts on an
    // octet boundary. `bs.len()` reports remaining bits in the stream;
    // the CRC is the last 16, so anything beyond that is padding.
    let remaining = bs.len();
    if remaining < BITS_CRC {
        return Err(cx.err(
            "PRL truncated before CRC",
            &bs,
            format!("{} bits remain, need {}", remaining, BITS_CRC).into(),
        ));
    }
    let pad = remaining - BITS_CRC;
    if pad >= BITS_PER_OCTET {
        return Err(cx.err(
            "PRL padding too large",
            &bs,
            format!("{} pad bits, max {}", pad, BITS_PER_OCTET - 1).into(),
        ));
    }
    if pad != 0 {
        let _ = read_u8(&mut bs, pad).map_err(|e| cx.err("RESERVED padding", &bs, e))?;
    }
    let pr_list_crc = read_u16(&mut bs, BITS_CRC).map_err(|e| cx.err("PR_LIST_CRC", &bs, e))?;

    let computed_crc = compute_prl_crc(bytes);

    Ok(ClassicPrl {
        pr_list_size,
        pr_list_id,
        pref_only,
        def_roam_ind,
        acquisition_records,
        system_records,
        pr_list_crc,
        computed_crc,
    })
}

/// One row of ACQ_TABLE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionRecord {
    pub acq_type_raw: u8,
    pub body: AcquisitionBody,
}

/// Decoded per-type body for an acquisition record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquisitionBody {
    /// §3.5.5.2.1.1 Cellular Analog (`ACQ_TYPE = 0001`).
    CellularAnalog { ab: AbSelection },
    /// §3.5.5.2.1.2 Cellular CDMA Standard Channels (`0010`).
    CellularCdmaStandard {
        ab: AbSelection,
        pri_sec: StandardChannelSelection,
    },
    /// §3.5.5.2.1.3 Cellular CDMA Custom Channels (`0011`).
    CellularCdmaCustom { channels: Vec<u16> },
    /// §3.5.5.2.1.4 Cellular CDMA Preferred (`0100`).
    CellularCdmaPreferred { ab: AbSelection },
    /// §3.5.5.2.1.5 PCS CDMA Using Blocks (`0101`).
    PcsCdmaUsingBlocks { blocks: Vec<PcsBlock> },
    /// §3.5.5.2.1.6 PCS CDMA / 2 GHz Band Using Channels (`0110`).
    PcsCdmaUsingChannels { channels: Vec<u16> },
    /// §3.5.5.2.1.7 JTACS CDMA Standard Channels (`0111`).
    JtacsCdmaStandard {
        ab: AbSelection,
        pri_sec: StandardChannelSelection,
    },
    /// §3.5.5.2.1.8 JTACS CDMA Custom Channels (`1000`).
    JtacsCdmaCustom { channels: Vec<u16> },
    /// §3.5.5.2.1.9 2 GHz Band CDMA Using Channels (`1001`).
    BandClass6UsingChannels { channels: Vec<u16> },
    /// Acquisition type the MS reported that this decoder doesn't yet
    /// understand. Keeps a record-count match for the rest of the
    /// table. The BS still shows the unknown type to the operator.
    Unknown,
}

impl AcquisitionRecord {
    fn decode(bs: &mut Bitstream) -> Result<Self, Error> {
        let acq_type_raw = read_u8(bs, BITS_ACQ_TYPE)?;
        let body = match acq_type_raw {
            ACQ_TYPE_CELLULAR_ANALOG => AcquisitionBody::CellularAnalog {
                ab: AbSelection::from_u8(read_u8(bs, BITS_AB)?),
            },
            ACQ_TYPE_CELLULAR_CDMA_STANDARD => AcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::from_u8(read_u8(bs, BITS_AB)?),
                pri_sec: StandardChannelSelection::from_u8(read_u8(bs, BITS_PRI_SEC)?),
            },
            ACQ_TYPE_CELLULAR_CDMA_CUSTOM => AcquisitionBody::CellularCdmaCustom {
                channels: read_chan_list(bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_CELLULAR_CDMA_PREFERRED => AcquisitionBody::CellularCdmaPreferred {
                ab: AbSelection::from_u8(read_u8(bs, BITS_AB)?),
            },
            ACQ_TYPE_PCS_CDMA_USING_BLOCKS => {
                let n = read_u8(bs, BITS_NUM_BLOCKS)? as usize;
                let mut blocks = Vec::with_capacity(n);
                for _ in 0..n {
                    blocks.push(PcsBlock::from_u8(read_u8(bs, BITS_PCS_BLOCK)?));
                }
                AcquisitionBody::PcsCdmaUsingBlocks { blocks }
            }
            ACQ_TYPE_PCS_CDMA_USING_CHANNELS => AcquisitionBody::PcsCdmaUsingChannels {
                channels: read_chan_list(bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_JTACS_CDMA_STANDARD => AcquisitionBody::JtacsCdmaStandard {
                ab: AbSelection::from_u8(read_u8(bs, BITS_AB)?),
                pri_sec: StandardChannelSelection::from_u8(read_u8(bs, BITS_PRI_SEC)?),
            },
            ACQ_TYPE_JTACS_CDMA_CUSTOM => AcquisitionBody::JtacsCdmaCustom {
                channels: read_chan_list(bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            ACQ_TYPE_BAND_CLASS_6 => AcquisitionBody::BandClass6UsingChannels {
                channels: read_chan_list(bs, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?,
            },
            _ => AcquisitionBody::Unknown,
        };
        Ok(Self { acq_type_raw, body })
    }

    pub(crate) fn encode_into(&self, bs: &mut Bitstream) -> Result<(), Error> {
        match &self.body {
            AcquisitionBody::CellularAnalog { ab } => {
                bs.write_u8(ACQ_TYPE_CELLULAR_ANALOG, BITS_ACQ_TYPE);
                bs.write_u8(ab.to_u8(), BITS_AB);
            }
            AcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
                bs.write_u8(ACQ_TYPE_CELLULAR_CDMA_STANDARD, BITS_ACQ_TYPE);
                bs.write_u8(ab.to_u8(), BITS_AB);
                bs.write_u8(pri_sec.to_u8(), BITS_PRI_SEC);
            }
            AcquisitionBody::CellularCdmaCustom { channels } => {
                bs.write_u8(ACQ_TYPE_CELLULAR_CDMA_CUSTOM, BITS_ACQ_TYPE);
                write_chan_list(bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            AcquisitionBody::CellularCdmaPreferred { ab } => {
                bs.write_u8(ACQ_TYPE_CELLULAR_CDMA_PREFERRED, BITS_ACQ_TYPE);
                bs.write_u8(ab.to_u8(), BITS_AB);
            }
            AcquisitionBody::PcsCdmaUsingBlocks { blocks } => {
                bs.write_u8(ACQ_TYPE_PCS_CDMA_USING_BLOCKS, BITS_ACQ_TYPE);
                let max = 1usize << BITS_NUM_BLOCKS;
                if blocks.len() >= max {
                    return Err("NUM_BLOCKS overflow".into());
                }
                bs.write_u8(blocks.len() as u8, BITS_NUM_BLOCKS);
                for b in blocks {
                    bs.write_u8(b.to_u8(), BITS_PCS_BLOCK);
                }
            }
            AcquisitionBody::PcsCdmaUsingChannels { channels } => {
                bs.write_u8(ACQ_TYPE_PCS_CDMA_USING_CHANNELS, BITS_ACQ_TYPE);
                write_chan_list(bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            AcquisitionBody::JtacsCdmaStandard { ab, pri_sec } => {
                bs.write_u8(ACQ_TYPE_JTACS_CDMA_STANDARD, BITS_ACQ_TYPE);
                bs.write_u8(ab.to_u8(), BITS_AB);
                bs.write_u8(pri_sec.to_u8(), BITS_PRI_SEC);
            }
            AcquisitionBody::JtacsCdmaCustom { channels } => {
                bs.write_u8(ACQ_TYPE_JTACS_CDMA_CUSTOM, BITS_ACQ_TYPE);
                write_chan_list(bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            AcquisitionBody::BandClass6UsingChannels { channels } => {
                bs.write_u8(ACQ_TYPE_BAND_CLASS_6, BITS_ACQ_TYPE);
                write_chan_list(bs, channels, BITS_NUM_CHANS, BITS_CHAN_NUMBER_11)?;
            }
            AcquisitionBody::Unknown => {
                return Err(format!(
                    "cannot encode Unknown acquisition record (raw ACQ_TYPE=0x{:02x})",
                    self.acq_type_raw
                )
                .into());
            }
        }
        Ok(())
    }
}

fn write_chan_list(
    bs: &mut Bitstream,
    channels: &[u16],
    count_bits: usize,
    chan_bits: usize,
) -> Result<(), Error> {
    let max_n = 1usize << count_bits;
    if channels.len() >= max_n {
        return Err(format!(
            "channel count {} exceeds {} bits",
            channels.len(),
            count_bits
        )
        .into());
    }
    bs.write_u32(channels.len() as u32, count_bits);
    for c in channels {
        bs.write_u32(*c as u32, chan_bits);
    }
    Ok(())
}

fn read_chan_list(
    bs: &mut Bitstream,
    count_bits: usize,
    chan_bits: usize,
) -> Result<Vec<u16>, Error> {
    let n = read_u32(bs, count_bits)? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(read_u16(bs, chan_bits)?);
    }
    Ok(out)
}

/// Cellular A/B selection per Table 3.5.5.2.1.1-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbSelection {
    SystemA,
    SystemB,
    Reserved,
    EitherAOrB,
}

impl AbSelection {
    pub fn from_u8(v: u8) -> Self {
        match v & 0b11 {
            0b00 => Self::SystemA,
            0b01 => Self::SystemB,
            0b10 => Self::Reserved,
            _ => Self::EitherAOrB,
        }
    }
}

/// Standard CDMA channel selection per Table 3.5.5.2.1.2-1 / 3.5.5.2.1.7-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardChannelSelection {
    Reserved,
    Primary,
    Secondary,
    PrimaryOrSecondary,
}

impl StandardChannelSelection {
    pub fn from_u8(v: u8) -> Self {
        match v & 0b11 {
            0b00 => Self::Reserved,
            0b01 => Self::Primary,
            0b10 => Self::Secondary,
            _ => Self::PrimaryOrSecondary,
        }
    }
}

/// PCS CDMA frequency blocks per Table 3.5.5.2.1.5-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcsBlock {
    A,
    B,
    C,
    D,
    E,
    F,
    Reserved,
    AnyBlock,
}

impl PcsBlock {
    pub fn from_u8(v: u8) -> Self {
        match v & 0b111 {
            0b000 => Self::A,
            0b001 => Self::B,
            0b010 => Self::C,
            0b011 => Self::D,
            0b100 => Self::E,
            0b101 => Self::F,
            0b110 => Self::Reserved,
            _ => Self::AnyBlock,
        }
    }
}

/// One row of SYS_TABLE per §3.5.5.3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemRecord {
    pub sid: u16,
    pub nid_incl: NidInclusion,
    /// Carried only when `nid_incl == SingleNid`. Wildcard values
    /// `0x0000` (public NID) and `0xFFFF` (any NID) come through
    /// verbatim.
    pub nid: Option<u16>,
    /// `true` when the system is on the same geographical region as
    /// the previous system record. Always `false` for the first record.
    pub same_geo_as_prev: bool,
    pub pref_neg: PrefNeg,
    pub acq_index: u16,
    /// Roaming indicator displayed when camped on this system. Only
    /// included when `pref_neg = Preferred`.
    pub roaming_indicator: Option<RoamingIndicator>,
    /// Priority hint relative to the next system record. Only
    /// included when `pref_neg = Preferred`. The spec encodes it in a
    /// single bit. We store the decoded label.
    pub priority: Option<Priority>,
}

impl SystemRecord {
    fn decode(bs: &mut Bitstream) -> Result<Self, Error> {
        let sid = read_u16(bs, BITS_SID)?;
        let nid_incl_raw = read_u8(bs, BITS_NID_INCL)?;
        let nid_incl = NidInclusion::from_u8(nid_incl_raw);
        let nid = if matches!(nid_incl, NidInclusion::SingleNid) {
            Some(read_u16(bs, BITS_NID)?)
        } else {
            None
        };
        let pref_neg_bit = read_bool(bs)?;
        let pref_neg = if pref_neg_bit {
            PrefNeg::Preferred
        } else {
            PrefNeg::Negative
        };
        let same_geo_as_prev = read_bool(bs)?;
        let priority = if matches!(pref_neg, PrefNeg::Preferred) {
            Some(if read_bool(bs)? {
                Priority::MoreDesirable
            } else {
                Priority::EquallyDesirable
            })
        } else {
            None
        };
        let acq_index = read_u16(bs, BITS_ACQ_INDEX)?;
        let roaming_indicator = if matches!(pref_neg, PrefNeg::Preferred) {
            Some(RoamingIndicator::from_u8(read_u8(bs, BITS_ROAM_IND)?))
        } else {
            None
        };
        Ok(Self {
            sid,
            nid_incl,
            nid,
            same_geo_as_prev,
            pref_neg,
            acq_index,
            roaming_indicator,
            priority,
        })
    }

    pub(crate) fn encode_into(&self, bs: &mut Bitstream) -> Result<(), Error> {
        let max_sid = 1u16 << BITS_SID;
        if self.sid >= max_sid {
            return Err("SID exceeds 15 bits".into());
        }
        bs.write_u32(self.sid as u32, BITS_SID);
        bs.write_u8(self.nid_incl.to_u8(), BITS_NID_INCL);
        match (self.nid_incl, self.nid) {
            (NidInclusion::SingleNid, Some(n)) => bs.write_u32(n as u32, BITS_NID),
            (NidInclusion::SingleNid, None) => {
                return Err("nid_incl=SingleNid but nid is None".into());
            }
            (_, Some(_)) => {
                return Err("nid set but nid_incl != SingleNid".into());
            }
            (_, None) => {}
        }
        let pref_neg_bit = match self.pref_neg {
            PrefNeg::Preferred => 1,
            PrefNeg::Negative => 0,
        };
        bs.write_u8(pref_neg_bit, BITS_PREF_NEG);
        bs.write_u8(self.same_geo_as_prev as u8, BITS_GEO);
        // PRI is only included when PREF_NEG = Preferred per spec.
        match (self.pref_neg, self.priority) {
            (PrefNeg::Preferred, Some(p)) => {
                let bit = match p {
                    Priority::MoreDesirable => 1,
                    Priority::EquallyDesirable => 0,
                };
                bs.write_u8(bit, BITS_PRI);
            }
            (PrefNeg::Preferred, None) => {
                return Err("pref_neg=Preferred but priority is None".into());
            }
            (PrefNeg::Negative, Some(_)) => {
                return Err("priority set but pref_neg=Negative".into());
            }
            (PrefNeg::Negative, None) => {}
        }
        let max_acq_index = 1u16 << BITS_ACQ_INDEX;
        if self.acq_index >= max_acq_index {
            return Err("ACQ_INDEX exceeds 9 bits".into());
        }
        bs.write_u32(self.acq_index as u32, BITS_ACQ_INDEX);
        // ROAM_IND only included when PREF_NEG = Preferred.
        match (self.pref_neg, self.roaming_indicator) {
            (PrefNeg::Preferred, Some(r)) => bs.write_u8(r.raw(), BITS_ROAM_IND),
            (PrefNeg::Preferred, None) => {
                return Err("pref_neg=Preferred but roaming_indicator is None".into());
            }
            (PrefNeg::Negative, Some(_)) => {
                return Err("roaming_indicator set but pref_neg=Negative".into());
            }
            (PrefNeg::Negative, None) => {}
        }
        Ok(())
    }
}

/// NID_INCL per Table 3.5.5.3-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NidInclusion {
    /// `00` — NID omitted, MS assumes `0xFFFF` (any-NID wildcard).
    AnyNid,
    /// `01` — NID explicit on the wire.
    SingleNid,
    /// `10` — NID omitted, MS assumes `0x0000` (public NID).
    PublicNid,
    /// `11` — Reserved.
    Reserved,
}

impl NidInclusion {
    pub fn from_u8(v: u8) -> Self {
        match v & 0b11 {
            0b00 => Self::AnyNid,
            0b01 => Self::SingleNid,
            0b10 => Self::PublicNid,
            _ => Self::Reserved,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefNeg {
    /// MS may operate on this system.
    Preferred,
    /// MS shall not operate on this system.
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// This record is preferred over the next system record.
    MoreDesirable,
    /// This record is equally desirable to the next system record.
    EquallyDesirable,
}

/// Roaming indicator. Values from C.R1001 (the registry referenced by
/// C.S0016-D for the `ROAM_IND` / `DEF_ROAM_IND` fields). Reserved
/// numerics are preserved verbatim via [`RoamingIndicator::Other`] so
/// operator displays can still show the raw value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoamingIndicator {
    IndicatorOn,
    IndicatorOff,
    IndicatorFlashing,
    OutOfNeighborhood,
    OutOfBuilding,
    PreferredSystem,
    AvailableSystem,
    AlliancePartner,
    PremiumPartner,
    FullService,
    PartialService,
    BannerOn,
    BannerOff,
    Other(u8),
}

impl RoamingIndicator {
    pub fn from_u8(v: u8) -> Self {
        match v {
            ROAM_IND_INDICATOR_ON => Self::IndicatorOn,
            ROAM_IND_INDICATOR_OFF => Self::IndicatorOff,
            ROAM_IND_INDICATOR_FLASHING => Self::IndicatorFlashing,
            ROAM_IND_OUT_OF_NEIGHBORHOOD => Self::OutOfNeighborhood,
            ROAM_IND_OUT_OF_BUILDING => Self::OutOfBuilding,
            ROAM_IND_PREFERRED_SYSTEM => Self::PreferredSystem,
            ROAM_IND_AVAILABLE_SYSTEM => Self::AvailableSystem,
            ROAM_IND_ALLIANCE_PARTNER => Self::AlliancePartner,
            ROAM_IND_PREMIUM_PARTNER => Self::PremiumPartner,
            ROAM_IND_FULL_SERVICE => Self::FullService,
            ROAM_IND_PARTIAL_SERVICE => Self::PartialService,
            ROAM_IND_BANNER_ON => Self::BannerOn,
            ROAM_IND_BANNER_OFF => Self::BannerOff,
            other => Self::Other(other),
        }
    }

    pub fn raw(self) -> u8 {
        match self {
            Self::IndicatorOn => ROAM_IND_INDICATOR_ON,
            Self::IndicatorOff => ROAM_IND_INDICATOR_OFF,
            Self::IndicatorFlashing => ROAM_IND_INDICATOR_FLASHING,
            Self::OutOfNeighborhood => ROAM_IND_OUT_OF_NEIGHBORHOOD,
            Self::OutOfBuilding => ROAM_IND_OUT_OF_BUILDING,
            Self::PreferredSystem => ROAM_IND_PREFERRED_SYSTEM,
            Self::AvailableSystem => ROAM_IND_AVAILABLE_SYSTEM,
            Self::AlliancePartner => ROAM_IND_ALLIANCE_PARTNER,
            Self::PremiumPartner => ROAM_IND_PREMIUM_PARTNER,
            Self::FullService => ROAM_IND_FULL_SERVICE,
            Self::PartialService => ROAM_IND_PARTIAL_SERVICE,
            Self::BannerOn => ROAM_IND_BANNER_ON,
            Self::BannerOff => ROAM_IND_BANNER_OFF,
            Self::Other(v) => v,
        }
    }
}

/// CRC-16 per C.S0016-D §3.5.5.1.
///
/// Polynomial `x^16 + x^12 + x^5 + 1`, seed `0xFFFF`, MSB-first, no
/// reflection, **final XOR with `0xFFFF`** (the CRC-16/GENIBUS
/// variant). The spec wording is: "the switches are set in position B,
/// and the register is clocked an additional 16 times — the 16
/// additional output bits constitute the CRC." Position B inverts the
/// register on the way out, which is the GENIBUS reading. Verified
/// against 16 real-carrier PRLs from Verizon, Sprint, Boost, MetroPCS,
/// US Cellular, Qwest, Cricket, Bluegrass, nTelos, Pocket, Western
/// Wireless, Alltel and Appalachian Wireless — all produce a matching
/// CRC.
///
/// Computed over every bit of the PRL except the final 16 CRC bits.
/// The PRL is octet-aligned at this point (RESERVED padding has
/// already been written), so we run the CRC byte-wise.
pub fn compute_prl_crc(bytes: &[u8]) -> u16 {
    if bytes.len() < 2 {
        return 0;
    }
    let body = &bytes[..bytes.len() - 2];
    let mut crc: u16 = 0xFFFF;
    for &octet in body {
        crc ^= (octet as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc ^ 0xFFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_prl() -> Vec<u8> {
        // Two acquisition records (Cellular CDMA Standard + PCS Using
        // Blocks) + two system records (one preferred, one negative
        // wildcard).
        let mut bs = Bitstream::new();
        // Header: SIZE placeholder, ID, PREF_ONLY, DEF_ROAM_IND, counts.
        bs.write_u32(0, 16); // PR_LIST_SIZE — patched below
        bs.write_u32(0xCAFE, 16); // PR_LIST_ID
        bs.write_u8(0, 1); // PREF_ONLY = false
        bs.write_u8(0, 8); // DEF_ROAM_IND = indicator on
        bs.write_u32(2, 9); // NUM_ACQ_RECS
        bs.write_u32(2, 14); // NUM_SYS_RECS
        // ACQ #0: Cellular CDMA Standard, A/B = either, primary or secondary.
        bs.write_u8(0x02, 4);
        bs.write_u8(0b11, 2); // EitherAOrB
        bs.write_u8(0b11, 2); // PrimaryOrSecondary
        // ACQ #1: PCS Using Blocks, blocks = [A, B].
        bs.write_u8(0x05, 4);
        bs.write_u8(2, 3); // NUM_BLOCKS
        bs.write_u8(0b000, 3); // Block A
        bs.write_u8(0b001, 3); // Block B
        // SYS #0: SID=22, NID=65535 (single nid wildcard), pref=true,
        // priority=more desirable, acq=0, roaming indicator on.
        bs.write_u32(22, 15);
        bs.write_u8(0b01, 2); // SingleNid
        bs.write_u32(65535, 16);
        bs.write_u8(1, 1); // PREF_NEG = preferred
        bs.write_u8(0, 1); // GEO = first record, must be 0
        bs.write_u8(1, 1); // PRI = more desirable
        bs.write_u32(0, 9); // ACQ_INDEX
        bs.write_u8(0, 8); // ROAM_IND = indicator on
        // SYS #1: SID wildcard, negative.
        bs.write_u32(0, 15);
        bs.write_u8(0b00, 2); // AnyNid
        bs.write_u8(0, 1); // PREF_NEG = negative
        bs.write_u8(0, 1); // GEO
        bs.write_u32(1, 9); // ACQ_INDEX
        // No PRI / ROAM_IND because PREF_NEG=0.
        // RESERVED padding to next octet boundary, then PR_LIST_CRC.
        let pad = (8 - (bs.len() % 8)) % 8;
        if pad != 0 {
            bs.write_u8(0, pad);
        }
        // Patch PR_LIST_SIZE: total octets after CRC append.
        let mut bytes = bs.to_packed_bytes();
        let total = bytes.len() as u16 + 2;
        bytes[0..2].copy_from_slice(&total.to_be_bytes());
        let crc = compute_prl_crc(&[bytes.as_slice(), &[0, 0]].concat());
        bytes.extend_from_slice(&crc.to_be_bytes());
        bytes
    }

    #[test]
    fn parses_minimal_prl_and_crc_matches() {
        let bytes = build_minimal_prl();
        let prl = decode(&bytes).unwrap();
        assert_eq!(prl.pr_list_id, 0xCAFE);
        assert_eq!(prl.acquisition_records.len(), 2);
        assert_eq!(prl.system_records.len(), 2);
        assert!(prl.crc_ok(), "expected CRC to match: {prl:?}");
        match &prl.acquisition_records[0].body {
            AcquisitionBody::CellularCdmaStandard { ab, pri_sec } => {
                assert_eq!(*ab, AbSelection::EitherAOrB);
                assert_eq!(*pri_sec, StandardChannelSelection::PrimaryOrSecondary);
            }
            other => panic!("acq #0 wrong: {other:?}"),
        }
        match &prl.acquisition_records[1].body {
            AcquisitionBody::PcsCdmaUsingBlocks { blocks } => {
                assert_eq!(blocks, &vec![PcsBlock::A, PcsBlock::B]);
            }
            other => panic!("acq #1 wrong: {other:?}"),
        }
        assert_eq!(prl.system_records[0].sid, 22);
        assert_eq!(prl.system_records[0].nid, Some(65535));
        assert_eq!(prl.system_records[0].pref_neg, PrefNeg::Preferred);
        assert_eq!(prl.system_records[0].acq_index, 0);
        assert_eq!(
            prl.system_records[0].roaming_indicator,
            Some(RoamingIndicator::IndicatorOn)
        );
        assert_eq!(prl.system_records[1].sid, 0);
        assert_eq!(prl.system_records[1].nid_incl, NidInclusion::AnyNid);
        assert_eq!(prl.system_records[1].pref_neg, PrefNeg::Negative);
        assert!(prl.system_records[1].roaming_indicator.is_none());
        assert!(prl.system_records[1].priority.is_none());
    }

    #[test]
    fn crc_known_input() {
        // CRC-16/GENIBUS test vector for "123456789" is 0xD64E
        // (CCITT-FALSE 0x29B1 XOR 0xFFFF). The +2 padding is stripped
        // before CRCing — `compute_prl_crc` excludes the trailing CRC
        // field, so this is just the polynomial sanity check.
        let mut input = b"123456789".to_vec();
        input.extend_from_slice(&[0, 0]);
        assert_eq!(compute_prl_crc(&input), 0xD64E);
    }

    #[test]
    fn roaming_indicator_assignments_match_registry() {
        let assignments = [
            (ROAM_IND_INDICATOR_ON, RoamingIndicator::IndicatorOn),
            (ROAM_IND_INDICATOR_OFF, RoamingIndicator::IndicatorOff),
            (
                ROAM_IND_INDICATOR_FLASHING,
                RoamingIndicator::IndicatorFlashing,
            ),
            (
                ROAM_IND_OUT_OF_NEIGHBORHOOD,
                RoamingIndicator::OutOfNeighborhood,
            ),
            (ROAM_IND_OUT_OF_BUILDING, RoamingIndicator::OutOfBuilding),
            (ROAM_IND_PREFERRED_SYSTEM, RoamingIndicator::PreferredSystem),
            (ROAM_IND_AVAILABLE_SYSTEM, RoamingIndicator::AvailableSystem),
            (ROAM_IND_ALLIANCE_PARTNER, RoamingIndicator::AlliancePartner),
            (ROAM_IND_PREMIUM_PARTNER, RoamingIndicator::PremiumPartner),
            (ROAM_IND_FULL_SERVICE, RoamingIndicator::FullService),
            (ROAM_IND_PARTIAL_SERVICE, RoamingIndicator::PartialService),
            (ROAM_IND_BANNER_ON, RoamingIndicator::BannerOn),
            (ROAM_IND_BANNER_OFF, RoamingIndicator::BannerOff),
        ];

        for (raw, expected) in assignments {
            assert_eq!(RoamingIndicator::from_u8(raw), expected);
            assert_eq!(expected.raw(), raw);
        }
        for raw in [13, 64, u8::MAX] {
            assert_eq!(RoamingIndicator::from_u8(raw), RoamingIndicator::Other(raw));
        }
    }

    /// Append RESERVED bits to octet-align, patch PR_LIST_SIZE, append CRC.
    fn finalize_prl(mut bs: Bitstream) -> Vec<u8> {
        let pad = (8 - (bs.len() % 8)) % 8;
        if pad != 0 {
            bs.write_u8(0, pad);
        }
        let mut bytes = bs.to_packed_bytes();
        let total = bytes.len() as u16 + 2;
        bytes[0..2].copy_from_slice(&total.to_be_bytes());
        let crc = compute_prl_crc(&[bytes.as_slice(), &[0, 0]].concat());
        bytes.extend_from_slice(&crc.to_be_bytes());
        bytes
    }

    /// PRL built from C.S0016-D Table E.1.2-1 (Simplified cdma2000 PRL
    /// example for one GEO). Six system records, all preferred, alternating
    /// MORE/SAME priority; first record starts a new geo, the rest share it.
    /// One Cellular CDMA Standard acquisition record covers all of them.
    #[test]
    fn parses_spec_appendix_e_example() {
        let mut bs = Bitstream::new();
        bs.write_u32(0, 16); // PR_LIST_SIZE — patched
        bs.write_u32(0xE001, 16); // PR_LIST_ID
        bs.write_u8(0, 1); // PREF_ONLY = false
        bs.write_u8(1, 8); // DEF_ROAM_IND = indicator off
        bs.write_u32(1, 9); // NUM_ACQ_RECS = 1
        bs.write_u32(6, 14); // NUM_SYS_RECS = 6
        // ACQ #0: Cellular CDMA Standard, Either A/B, Primary-or-Secondary.
        bs.write_u8(0x02, 4);
        bs.write_u8(0b11, 2);
        bs.write_u8(0b11, 2);

        // Helper for one preferred sys record with a specific NID.
        let preferred = |bs: &mut Bitstream, sid: u32, nid: u32, geo: u8, more: u8| {
            bs.write_u32(sid, 15);
            bs.write_u8(0b01, 2); // NID_INCL = SingleNid
            bs.write_u32(nid, 16);
            bs.write_u8(1, 1); // PREF_NEG = preferred
            bs.write_u8(geo, 1);
            bs.write_u8(more, 1); // PRI: 1=MORE, 0=EQUAL
            bs.write_u32(0, 9); // ACQ_INDEX = 0
            bs.write_u8(0, 8); // ROAM_IND = indicator on
        };
        // Index 0: SID=1, NID=1, PREF, MORE, NEW geo
        preferred(&mut bs, 1, 1, 0, 1);
        // Index 1: SID=3, NID=40, PREF, EQUAL, same geo
        preferred(&mut bs, 3, 40, 1, 0);
        // Index 2: SID=3, NID=2, PREF, EQUAL, same geo
        preferred(&mut bs, 3, 2, 1, 0);
        // Index 3: SID=3, NID=15, PREF, MORE, same geo
        preferred(&mut bs, 3, 15, 1, 1);
        // Index 4: SID=5, NID=20, PREF, EQUAL, same geo
        preferred(&mut bs, 5, 20, 1, 0);
        // Index 5: SID=5, NID=75, PREF, MORE, same geo
        preferred(&mut bs, 5, 75, 1, 1);

        let bytes = finalize_prl(bs);
        let prl = decode(&bytes).unwrap();
        assert!(prl.crc_ok());
        assert_eq!(prl.pr_list_id, 0xE001);
        assert_eq!(prl.def_roam_ind, RoamingIndicator::IndicatorOff);
        assert_eq!(prl.acquisition_records.len(), 1);
        assert_eq!(prl.system_records.len(), 6);

        let s = &prl.system_records;
        assert_eq!(
            (s[0].sid, s[0].nid),
            (1, Some(1)),
            "index 0 expected SID=1 NID=1"
        );
        assert!(!s[0].same_geo_as_prev, "first record GEO must be false");
        assert_eq!(s[0].priority, Some(Priority::MoreDesirable));
        for sr in &s[1..] {
            assert!(sr.same_geo_as_prev, "all later records share geo");
            assert_eq!(sr.pref_neg, PrefNeg::Preferred);
        }
        assert_eq!(s[1].sid, 3);
        assert_eq!(s[1].nid, Some(40));
        assert_eq!(s[1].priority, Some(Priority::EquallyDesirable));
        assert_eq!(s[3].nid, Some(15));
        assert_eq!(s[3].priority, Some(Priority::MoreDesirable));
        assert_eq!(s[5].nid, Some(75));
        assert_eq!(s[5].priority, Some(Priority::MoreDesirable));
    }

    /// All 9 classic acquisition record types in one PRL, plus the system
    /// table cross-references them by index.
    #[test]
    fn decodes_every_acquisition_record_type() {
        let mut bs = Bitstream::new();
        bs.write_u32(0, 16); // PR_LIST_SIZE
        bs.write_u32(0xACE0, 16);
        bs.write_u8(0, 1);
        bs.write_u8(0, 8);
        bs.write_u32(9, 9);
        bs.write_u32(1, 14);
        // 1: Cellular Analog
        bs.write_u8(0x01, 4);
        bs.write_u8(0b00, 2); // SystemA
        // 2: Cellular CDMA Standard
        bs.write_u8(0x02, 4);
        bs.write_u8(0b01, 2); // SystemB
        bs.write_u8(0b01, 2); // Primary
        // 3: Cellular CDMA Custom — channels [283]
        bs.write_u8(0x03, 4);
        bs.write_u8(1, 5);
        bs.write_u32(283, 11);
        // 4: Cellular CDMA Preferred
        bs.write_u8(0x04, 4);
        bs.write_u8(0b11, 2); // Either
        // 5: PCS Using Blocks — [C, D, E]
        bs.write_u8(0x05, 4);
        bs.write_u8(3, 3);
        bs.write_u8(0b010, 3);
        bs.write_u8(0b011, 3);
        bs.write_u8(0b100, 3);
        // 6: PCS Using Channels — [25, 75]
        bs.write_u8(0x06, 4);
        bs.write_u8(2, 5);
        bs.write_u32(25, 11);
        bs.write_u32(75, 11);
        // 7: JTACS Standard
        bs.write_u8(0x07, 4);
        bs.write_u8(0b00, 2); // SystemA
        bs.write_u8(0b10, 2); // Secondary
        // 8: JTACS Custom — [600]
        bs.write_u8(0x08, 4);
        bs.write_u8(1, 5);
        bs.write_u32(600, 11);
        // 9: 2 GHz Using Channels — [1, 2]
        bs.write_u8(0x09, 4);
        bs.write_u8(2, 5);
        bs.write_u32(1, 11);
        bs.write_u32(2, 11);
        // One system record so num_sys_recs > 0
        bs.write_u32(123, 15);
        bs.write_u8(0b10, 2); // PublicNid (no NID on wire)
        bs.write_u8(1, 1); // pref
        bs.write_u8(0, 1); // geo
        bs.write_u8(0, 1); // PRI = EquallyDesirable
        bs.write_u32(8, 9); // ACQ_INDEX = 8 (last record)
        bs.write_u8(2, 8); // ROAM_IND = indicator flashing

        let bytes = finalize_prl(bs);
        let prl = decode(&bytes).unwrap();
        assert!(prl.crc_ok());
        assert_eq!(prl.acquisition_records.len(), 9);
        let bodies: Vec<&AcquisitionBody> =
            prl.acquisition_records.iter().map(|r| &r.body).collect();
        assert!(matches!(bodies[0], AcquisitionBody::CellularAnalog { .. }));
        assert!(matches!(
            bodies[1],
            AcquisitionBody::CellularCdmaStandard { .. }
        ));
        assert!(matches!(
            bodies[2],
            AcquisitionBody::CellularCdmaCustom { .. }
        ));
        assert!(matches!(
            bodies[3],
            AcquisitionBody::CellularCdmaPreferred { .. }
        ));
        if let AcquisitionBody::PcsCdmaUsingBlocks { blocks } = bodies[4] {
            assert_eq!(blocks, &vec![PcsBlock::C, PcsBlock::D, PcsBlock::E]);
        } else {
            panic!("acq 4 wrong");
        }
        if let AcquisitionBody::PcsCdmaUsingChannels { channels } = bodies[5] {
            assert_eq!(channels, &vec![25, 75]);
        } else {
            panic!("acq 5 wrong");
        }
        assert!(matches!(
            bodies[6],
            AcquisitionBody::JtacsCdmaStandard { .. }
        ));
        if let AcquisitionBody::JtacsCdmaCustom { channels } = bodies[7] {
            assert_eq!(channels, &vec![600]);
        } else {
            panic!("acq 7 wrong");
        }
        if let AcquisitionBody::BandClass6UsingChannels { channels } = bodies[8] {
            assert_eq!(channels, &vec![1, 2]);
        } else {
            panic!("acq 8 wrong");
        }

        // System record sanity.
        let s = &prl.system_records[0];
        assert_eq!(s.sid, 123);
        assert_eq!(s.nid_incl, NidInclusion::PublicNid);
        assert_eq!(s.nid, None);
        assert_eq!(s.acq_index, 8);
        assert_eq!(
            s.roaming_indicator,
            Some(RoamingIndicator::IndicatorFlashing)
        );
    }

    /// A flipped byte in the assembled buffer must make `crc_ok` false but
    /// must NOT cause decode to error — operators still see the partially
    /// decoded PRL alongside the CRC mismatch.
    #[test]
    fn crc_mismatch_detected_without_decode_error() {
        let bytes = build_minimal_prl();
        let mut tampered = bytes.clone();
        // Flip the PR_LIST_ID's low byte.
        tampered[3] ^= 0x01;
        let prl = decode(&tampered).unwrap();
        assert!(!prl.crc_ok(), "CRC should not match after tampering");
        assert_ne!(prl.pr_list_crc, prl.computed_crc);
    }

    /// Unknown ACQ_TYPE values must decode to `Unknown` (not error) so the
    /// rest of the table stays parseable. The decoder will still misalign
    /// the remaining bits since it can't know the unknown record's length,
    /// but the failure shows up as a downstream EOF rather than a panic.
    #[test]
    fn unknown_acq_type_yields_unknown_variant() {
        let mut bs = Bitstream::new();
        bs.write_u32(0, 16);
        bs.write_u32(0, 16);
        bs.write_u8(0, 1);
        bs.write_u8(0, 8);
        bs.write_u32(1, 9);
        bs.write_u32(0, 14);
        bs.write_u8(0b1111, 4); // Reserved ACQ_TYPE
        let bytes = finalize_prl(bs);
        let prl = decode(&bytes).unwrap();
        assert!(matches!(
            prl.acquisition_records[0].body,
            AcquisitionBody::Unknown
        ));
        assert_eq!(prl.acquisition_records[0].acq_type_raw, 0b1111);
    }
}
