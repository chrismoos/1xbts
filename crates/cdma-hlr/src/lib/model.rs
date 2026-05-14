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
}

/// AWIM Calling Party Number Type per C.S0005-E 3.7.5.10 / ANSI T1.607.
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

/// AWIM Calling Party Numbering Plan per C.S0005-E 3.7.5.10 / ANSI T1.607.
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

/// A single identity (IMSI or ESN) associated with a subscriber.
#[derive(Debug, Clone)]
pub struct SubscriberIdentity {
    pub subscriber_identity_id: Uuid,
    pub subscriber_id: Uuid,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
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
    use super::{validate_imsi, validate_phone_number};

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
