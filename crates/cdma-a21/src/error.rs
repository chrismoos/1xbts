//! A21 codec and transport errors.

#[derive(Debug, thiserror::Error)]
pub enum A21Error {
    #[error("A21 I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown A21 message type 0x{0:02x}")]
    UnknownMessageType(u8),
    #[error("{context} truncated: need {needed} octets, have {actual}")]
    Truncated {
        context: &'static str,
        needed: usize,
        actual: usize,
    },
    #[error("{context} has length {actual}, expected {expected}")]
    InvalidLength {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("invalid {context}: {reason}")]
    InvalidValue {
        context: &'static str,
        reason: &'static str,
    },
}

pub type Result<T> = std::result::Result<T, A21Error>;
