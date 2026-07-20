//! A21 codec and transport errors.

use std::fmt::{Display, Formatter};

/// Errors returned by A21 codec and transport helpers.
#[derive(Debug)]
pub enum A21Error {
    /// Underlying I/O failure on the TCP socket.
    Io(std::io::Error),
    /// Wire frame failed to decode (unknown discriminant, truncation, bad utf-8, etc).
    Decode(String),
    /// Remote peer closed the connection cleanly.
    Closed,
}

impl Display for A21Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            A21Error::Io(e) => write!(f, "a21 io: {e}"),
            A21Error::Decode(m) => write!(f, "a21 decode: {m}"),
            A21Error::Closed => write!(f, "a21 connection closed"),
        }
    }
}

impl std::error::Error for A21Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            A21Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for A21Error {
    fn from(value: std::io::Error) -> Self {
        A21Error::Io(value)
    }
}

/// Result alias used by A21 codec and transport helpers.
pub type Result<T> = std::result::Result<T, A21Error>;
