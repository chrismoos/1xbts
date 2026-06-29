use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Result of upserting a row in `mobiles_seen`.
#[derive(Debug, Clone)]
pub struct MobileSeenUpsert {
    /// True if this is the first time we've seen this mobile.
    pub is_new: bool,
    /// The `last_seen_at` value before this upsert (None if new).
    pub previous_last_seen_at: Option<DateTime<Utc>>,
}

pub const IMSI_LEN: usize = 15;
pub const MEID_HEX_LEN: usize = 14;
pub const MAX_PHONE_LEN: usize = 15;

/// A provisioned subscriber record from the HLR.
#[derive(Debug, Clone)]
pub struct Subscriber {
    pub subscriber_id: Uuid,
    pub phone_number: String,
    pub display_name: String,
    pub status: SubscriberStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub number_type: NumberType,
    pub number_plan: NumberPlan,
    /// True when a custom ringtone is stored for this subscriber.
    pub has_ringtone: bool,
    /// Duration of the stored ringtone in milliseconds (if any).
    pub ringtone_duration_ms: Option<u64>,
    /// Optional per-subscriber PRL override. When set, OTASP `*228`
    /// pushes this PRL instead of the system default.
    pub prl_override_id: Option<Uuid>,
    /// Custom 6-digit Service Programming Code for this subscriber's
    /// device. `None` means the device uses the IS-95 default "000000"
    /// and OTASP Verify SPC will use that.
    pub service_programming_code: Option<String>,
    /// Per-subscriber FIRSTCHP override (analog first paging/control
    /// channel, 0–2047). `None` means OTASP preserves the handset's
    /// existing value instead of overwriting it.
    pub firstchp_override: Option<u16>,
}

/// A row from the `prls` table — the canonical artifact + cached
/// columns used for list filtering. The decoded tree is recomputed on
/// demand via `cdma_otasp::param::prl::decode` / `prl_ext::decode`.
#[derive(Debug, Clone)]
pub struct Prl {
    pub prl_id: Uuid,
    pub name: String,
    pub pr_list_id: i32,
    pub sspr_p_rev: i16,
    pub is_default: bool,
    pub raw_bytes: Vec<u8>,
    pub notes: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Reasons a soft-delete might be refused, surfaced via gRPC
/// `FAILED_PRECONDITION` with structured details.
#[derive(Debug, Clone)]
pub enum PrlDeleteBlocked {
    /// At least one subscriber references this PRL as their override.
    /// Sample IDs are returned for the operator UI to display.
    Referenced { count: u32, sample: Vec<Uuid> },
}

/// Reasons a CreatePrl / UpdatePrl byte payload was rejected.
#[derive(Debug, Clone)]
pub enum PrlValidationFailure {
    DecodeFailed(String),
    CrcMismatch { ms_crc: u16, computed_crc: u16 },
    UnsupportedRev(u8),
    EncodeFailed(String),
}

/// Filter passed to `list_prls`.
#[derive(Debug, Clone, Default)]
pub struct PrlListFilter {
    pub pr_list_id: Option<u32>,
    pub sspr_p_rev: Option<u32>,
}

/// One row of the `otasp_sessions` table. Cached columns let list
/// queries run without unpacking `events_proto`; the blob carries the
/// prost-encoded `events.v1.OtaspRecordedEvents` for the timeline.
#[derive(Debug, Clone)]
pub struct OtaspSessionRow {
    pub session_id: Uuid,
    pub subscriber_id: Option<Uuid>,
    pub esn: Option<u32>,
    pub meid: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// `events.v1.OtaspSessionOutcome` discriminant.
    pub outcome: i16,
    pub feature_code: Option<String>,
    pub service_option: Option<i32>,
    pub completed_blocks: i32,
    pub event_count: i32,
    pub events_proto: Vec<u8>,
}

/// Filter passed to `list_otasp_sessions`. Empty filter returns every
/// session newest-first.
#[derive(Debug, Clone, Default)]
pub struct OtaspSessionFilter {
    pub subscriber_id: Option<Uuid>,
    pub esn: Option<u32>,
    pub meid: Option<String>,
}

/// AWIM Calling Party Number Type per C.S0005-E 3.7.5.3 / ANSI T1.607.
/// `to_wire()` returns the 3-bit on-air value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "number_type", rename_all = "snake_case")]
pub enum NumberType {
    Unknown,
    International,
    National,
    NetworkSpecific,
    Subscriber,
    Abbreviated,
}

impl NumberType {
    pub fn to_wire(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::International => 1,
            Self::National => 2,
            Self::NetworkSpecific => 3,
            Self::Subscriber => 4,
            Self::Abbreviated => 6,
        }
    }
}

impl Default for NumberType {
    fn default() -> Self {
        Self::NetworkSpecific
    }
}

/// AWIM Calling Party Numbering Plan per C.S0005-E 3.7.5.3 / ANSI T1.607.
/// `to_wire()` returns the 4-bit on-air value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "number_plan", rename_all = "snake_case")]
pub enum NumberPlan {
    Unknown,
    IsdnE164,
    Data,
    Telex,
    Private,
}

impl NumberPlan {
    pub fn to_wire(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::IsdnE164 => 1,
            Self::Data => 3,
            Self::Telex => 4,
            Self::Private => 9,
        }
    }
}

impl Default for NumberPlan {
    fn default() -> Self {
        Self::IsdnE164
    }
}

/// Per-codec ringtone blob row.
#[derive(Debug, Clone)]
pub struct SubscriberRingtoneCodecBlob {
    pub codec: String,
    pub encoded_frames: Vec<u8>,
    pub frame_count: u64,
    pub duration_ms: u64,
}

/// Per-codec summary returned by `set_ringtone` after preencode.
#[derive(Debug, Clone)]
pub struct SetRingtoneCodecOutcome {
    pub codec: String,
    pub encoded_bytes: u32,
    pub frame_count: u64,
}

/// Result of a successful `set_ringtone`: per-codec summaries plus the
/// canonical duration of the stored audio in milliseconds.
#[derive(Debug, Clone)]
pub struct SetRingtoneOutcome {
    pub codecs: Vec<SetRingtoneCodecOutcome>,
    pub duration_ms: u64,
}

/// Lifecycle status of a subscriber account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriberStatus {
    Active,
    Suspended,
    Disabled,
}

impl SubscriberStatus {
    /// Returns the canonical DB/wire string for this status.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Disabled => "disabled",
        }
    }

    /// Parses a status string as stored in the database.
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "disabled" => Ok(Self::Disabled),
            _ => Err(format!("unknown subscriber status: {s}")),
        }
    }
}

/// A resolved subscriber plus the auxiliary data callers typically need
/// in the same breath: the full identity list, the primary identity
/// (convenience accessor — same one that's in `identities`), and the
/// current registration binding. Returned by the read methods on
/// `HlrRepository` so callers don't have to chase a second RPC.
#[derive(Debug, Clone)]
pub struct ResolvedSubscriber {
    pub subscriber: Subscriber,
    pub identities: Vec<SubscriberIdentity>,
    pub primary_identity: Option<SubscriberIdentity>,
    pub binding: Option<RegistrationBinding>,
}

/// Complete mobile identity forms that can identify a subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIdentityKey {
    ImsiEsn {
        imsi: String,
        esn: u32,
    },
    ImsiMeid {
        imsi: String,
        meid: String,
    },
    ImsiEsnMeid {
        imsi: String,
        esn: u32,
        meid: String,
    },
}

impl MobileIdentityKey {
    pub fn from_parts(
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<Self, String> {
        let imsi = imsi.ok_or_else(|| "identity key requires IMSI".to_string())?;
        validate_imsi(imsi)?;
        let meid = meid.map(normalize_meid).transpose()?;
        match (esn, meid) {
            (Some(esn), Some(meid)) => Ok(Self::ImsiEsnMeid {
                imsi: imsi.to_string(),
                esn,
                meid,
            }),
            (Some(esn), None) => Ok(Self::ImsiEsn {
                imsi: imsi.to_string(),
                esn,
            }),
            (None, Some(meid)) => Ok(Self::ImsiMeid {
                imsi: imsi.to_string(),
                meid,
            }),
            (None, None) => Err("identity key requires ESN or MEID with IMSI".to_string()),
        }
    }

    pub fn imsi(&self) -> &str {
        match self {
            Self::ImsiEsn { imsi, .. }
            | Self::ImsiMeid { imsi, .. }
            | Self::ImsiEsnMeid { imsi, .. } => imsi,
        }
    }

    pub fn esn(&self) -> Option<u32> {
        match self {
            Self::ImsiEsn { esn, .. } | Self::ImsiEsnMeid { esn, .. } => Some(*esn),
            Self::ImsiMeid { .. } => None,
        }
    }

    pub fn meid(&self) -> Option<&str> {
        match self {
            Self::ImsiMeid { meid, .. } | Self::ImsiEsnMeid { meid, .. } => Some(meid.as_str()),
            Self::ImsiEsn { .. } => None,
        }
    }
}

/// A single complete identity associated with a subscriber.
#[derive(Debug, Clone)]
pub struct SubscriberIdentity {
    pub subscriber_identity_id: Uuid,
    pub subscriber_id: Uuid,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<String>,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

/// Validates an IMSI: exactly 15 decimal digits (3GPP TS 23.003 / E.212).
pub fn validate_imsi(imsi: &str) -> Result<(), String> {
    if imsi.len() != IMSI_LEN {
        return Err(format!(
            "IMSI must be exactly {} digits, got {}",
            IMSI_LEN,
            imsi.len()
        ));
    }
    if !imsi.bytes().all(|b| b.is_ascii_digit()) {
        return Err("IMSI must contain only decimal digits".to_string());
    }
    Ok(())
}

pub fn normalize_meid(meid: &str) -> Result<String, String> {
    let meid = meid.trim();
    if meid.len() != MEID_HEX_LEN {
        return Err(format!(
            "MEID must be exactly {} hex digits, got {}",
            MEID_HEX_LEN,
            meid.len()
        ));
    }
    if !meid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("MEID must contain only hexadecimal digits".to_string());
    }
    Ok(meid.to_ascii_lowercase())
}

/// Validates a phone number: 1–15 decimal digits (E.164 max).
pub fn validate_phone_number(phone_number: &str) -> Result<(), String> {
    if phone_number.is_empty() {
        return Err("phone number must contain at least one digit".to_string());
    }
    if phone_number.len() > MAX_PHONE_LEN {
        return Err(format!(
            "phone number must be at most {} digits, got {}",
            MAX_PHONE_LEN,
            phone_number.len()
        ));
    }
    if !phone_number.bytes().all(|b| b.is_ascii_digit()) {
        return Err("phone number must contain only decimal digits".to_string());
    }
    Ok(())
}

/// Current radio registration state of a subscriber at a BSC node.
#[derive(Debug, Clone)]
pub struct RegistrationBinding {
    pub subscriber_id: Uuid,
    pub serving_node_id: String,
    pub state: RegistrationState,
    /// Authoritative IMSI string, sourced from `SubscriberIdentity`.
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<String>,
    pub mob_p_rev: Option<u32>,
    pub pgslot: Option<u32>,
    pub slot_cycle_index: Option<u32>,
    pub last_msg_seq: Option<u32>,
    pub last_registered_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Registration lifecycle state stored in the HLR binding table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationState {
    Registered,
    Paged,
    PageResponseReceived,
    Stale,
}

impl RegistrationState {
    /// Returns the canonical DB/wire string for this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Paged => "paged",
            Self::PageResponseReceived => "page_response_received",
            Self::Stale => "stale",
        }
    }

    /// Parses a state string as stored in the database.
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "registered" => Ok(Self::Registered),
            "paged" => Ok(Self::Paged),
            "page_response_received" => Ok(Self::PageResponseReceived),
            "stale" => Ok(Self::Stale),
            other => Err(format!("unknown registration state: {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MobileIdentityKey, normalize_meid, validate_imsi, validate_phone_number};

    #[test]
    fn accepts_exact_15_digit_imsi() {
        assert!(validate_imsi("123456789012345").is_ok());
    }

    #[test]
    fn rejects_imsi_of_wrong_length() {
        assert!(validate_imsi("12345678901234").is_err());
        assert!(validate_imsi("1234567890").is_err());
        assert!(validate_imsi("1234567890123456").is_err());
    }

    #[test]
    fn rejects_non_digit_imsi() {
        assert!(validate_imsi("12345abcdef0123").is_err());
        assert!(validate_imsi("12345 6789012345").is_err());
    }

    #[test]
    fn normalizes_meid() {
        assert_eq!(normalize_meid("A000000123ABCD").unwrap(), "a000000123abcd");
    }

    #[test]
    fn rejects_partial_mobile_identity_key() {
        assert!(MobileIdentityKey::from_parts(Some("123456789012345"), None, None).is_err());
        assert!(MobileIdentityKey::from_parts(None, Some(0x12345678), None).is_err());
    }

    #[test]
    fn accepts_phone_number_up_to_15_digits() {
        assert!(validate_phone_number("1").is_ok());
        assert!(validate_phone_number("5550001").is_ok());
        assert!(validate_phone_number("123456789012345").is_ok());
    }

    #[test]
    fn rejects_phone_number_over_15_digits() {
        assert!(validate_phone_number("1234567890123456").is_err());
    }

    #[test]
    fn rejects_empty_or_non_digit_phone_number() {
        assert!(validate_phone_number("").is_err());
        assert!(validate_phone_number("555-0001").is_err());
        assert!(validate_phone_number("555 0001").is_err());
        assert!(validate_phone_number("abc").is_err());
    }
}
