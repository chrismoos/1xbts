use std::ffi::NulError;
use std::io;

use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("native libre/re library is unavailable")]
    NativeUnavailable,

    #[error("libre/re operation {operation} failed with status {status}")]
    Native {
        operation: &'static str,
        status: i32,
    },

    #[error("invalid C string")]
    InvalidCString(#[from] NulError),

    #[error("failed to spawn libre/re event loop thread")]
    Spawn(#[source] io::Error),

    #[error("libre/re event loop thread panicked")]
    EventLoopPanicked,
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn native_status(operation: &'static str, status: i32) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Native { operation, status })
    }
}
