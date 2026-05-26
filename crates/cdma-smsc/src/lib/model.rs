use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Destination targeting for an SMS submission. Exactly one variant is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsDestination {
    /// Standard MT-SMS addressed to a phone number (e.g. "3105551234").
    PhoneNumber(String),
    /// Addressed to an unprovisioned or roaming MS by 32-bit ESN.
    Esn(u32),
    /// Addressed by IMSI or IMSI_S digits (10 or 15 ASCII decimal digits).
    Imsi(String),
}

/// TIA/EIA-637 teleservice layer fields used to deduplicate MO SMS submissions.
///
/// The combination of `teleservice_id`, `message_type`, and `message_id` uniquely
/// identifies a single SMS transmission attempt; retransmissions carry the same values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoSmsFingerprint {
    pub teleservice_id: u16,
    pub message_type: u8,
    pub message_id: u16,
}

/// An SMS submission tracked by the SMSC from origination through delivery.
#[derive(Debug, Clone)]
pub struct SmsSubmission {
    pub sms_id: Uuid,
    pub originating_number: String,
    /// Set when targeting by phone number.
    pub destination_number: Option<String>,
    /// Set when targeting by ESN.
    pub destination_esn: Option<u32>,
    /// Set when targeting by IMSI / IMSI_S digits.
    pub destination_imsi: Option<String>,
    pub originating_subscriber_id: Option<Uuid>,
    pub destination_subscriber_id: Option<Uuid>,
    pub text: String,
    pub state: SmsState,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// C.S0015-B teleservice ID. `None` selects WMT (0x1002).
    pub teleservice_id: Option<u16>,
    /// Opaque User Data bytes for non-text teleservices. When set, the BSC
    /// emits these verbatim as the bearer-data User Data sub-parameter
    /// (MSG_ENCODING=0x00 octet) instead of encoding `text`.
    pub raw_user_data: Option<Vec<u8>>,
}

/// Lifecycle state of an SMS submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsState {
    Accepted,
    Paging,
    PageResponseReceived,
    Sent,
    Delivered,
    Failed,
    Expired,
}

impl SmsState {
    /// Returns the canonical DB/wire string for this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Paging => "paging",
            Self::PageResponseReceived => "page_response_received",
            Self::Sent => "sent",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }

    /// Parses a state string as stored in the database.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "accepted" => Some(Self::Accepted),
            "paging" => Some(Self::Paging),
            "page_response_received" => Some(Self::PageResponseReceived),
            "sent" => Some(Self::Sent),
            "delivered" => Some(Self::Delivered),
            "failed" => Some(Self::Failed),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// A single delivery attempt for an SMS submission.
#[derive(Debug, Clone)]
pub struct SmsDeliveryAttempt {
    pub sms_delivery_attempt_id: Uuid,
    pub sms_id: Uuid,
    pub attempt_number: u32,
    pub state: DeliveryAttemptState,
    /// None for unprovisioned MS with no HLR subscriber record.
    pub target_subscriber_id: Option<Uuid>,
    pub failure_reason: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lifecycle state of a single SMS delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryAttemptState {
    Queued,
    Paging,
    PageResponseReceived,
    ForwardSent,
    Failed,
    Expired,
}

impl DeliveryAttemptState {
    /// Returns the canonical DB/wire string for this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Paging => "paging",
            Self::PageResponseReceived => "page_response_received",
            Self::ForwardSent => "forward_sent",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }

    /// Parses a state string as stored in the database.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "paging" => Some(Self::Paging),
            "page_response_received" => Some(Self::PageResponseReceived),
            "forward_sent" => Some(Self::ForwardSent),
            "failed" => Some(Self::Failed),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}
