/// Errors returned by Abis codec and bearer helpers.
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyMessage,
    UnknownMessageType(u8),
    UnknownInformationElement(u8),
    InvalidMessage {
        message_type: u8,
        reason: &'static str,
    },
    InvalidLength {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    Truncated {
        context: &'static str,
        needed: usize,
        actual: usize,
    },
    OutOfOrderElement {
        message_type: u8,
        id: u8,
    },
    MissingRequiredElement {
        message_type: u8,
        id: u8,
    },
    DuplicateElement {
        message_type: u8,
        id: u8,
    },
    ReservedValue {
        context: &'static str,
        value: u8,
    },
    InvalidValue {
        context: &'static str,
        reason: &'static str,
    },
}

/// Result type used by the `cdma-abis` crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::EmptyMessage => f.write_str("empty Abis message"),
            Error::UnknownMessageType(value) => {
                write!(f, "unknown Abis message type 0x{value:02x}")
            }
            Error::UnknownInformationElement(value) => {
                write!(f, "unknown Abis information element 0x{value:02x}")
            }
            Error::InvalidMessage {
                message_type,
                reason,
            } => write!(f, "invalid Abis message 0x{message_type:02x}: {reason}"),
            Error::InvalidLength {
                context,
                expected,
                actual,
            } => write!(f, "{context} length {actual}, expected {expected}"),
            Error::Truncated {
                context,
                needed,
                actual,
            } => write!(f, "{context} truncated: need {needed}, have {actual}"),
            Error::OutOfOrderElement { message_type, id } => write!(
                f,
                "information element 0x{id:02x} is out of order for message 0x{message_type:02x}"
            ),
            Error::MissingRequiredElement { message_type, id } => write!(
                f,
                "message 0x{message_type:02x} missing required information element 0x{id:02x}"
            ),
            Error::DuplicateElement { message_type, id } => write!(
                f,
                "message 0x{message_type:02x} contains duplicate singleton element 0x{id:02x}"
            ),
            Error::ReservedValue { context, value } => {
                write!(f, "{context} has reserved value 0x{value:02x}")
            }
            Error::InvalidValue { context, reason } => write!(f, "{context}: {reason}"),
        }
    }
}

impl std::error::Error for Error {}
