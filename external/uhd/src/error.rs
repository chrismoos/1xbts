use crate::utils::copy_string;
use std::ffi::NulError;
use std::str::Utf8Error;

use thiserror::Error as ThisError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(ThisError, Debug)]
pub enum Error {
    /// Used when errors need to propogate but are too unique to be typed
    #[error("{0}")]
    Unique(String),

    #[error("I/O Error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Invalid device arguments")]
    InvalidDevice,

    #[error("uhd::index_error - A sequence index is out of range")]
    Index,

    #[error("uhd::key_error - Invalid key")]
    Key,

    #[error("uhd::not_implemented_error - Not implemented")]
    NotImplemented,

    #[error("uhd::usb_error - USB communication problem")]
    Usb,

    #[error("uhd::io_error - Input/output error")]
    Io,

    #[error("uhd::os_error - System-related error")]
    Os,

    #[error("uhd::assertion_error - Assertion failed")]
    Assertion,

    #[error("uhd::lookup_error - Invalid index or key")]
    Lookup,

    #[error("uhd::type_error - Value has incorrect type")]
    Type,

    #[error("uhd::value_error - Invalid value")]
    Value,

    #[error("uhd::runtime_error - Other runtime error")]
    Runtime,

    #[error("uhd::environment_error - Environment error")]
    Environment,

    #[error("uhd::system_error - System-related error")]
    System,

    #[error("uhd::exception - Other UHD exception")]
    Except,

    #[error("A boost::exception was thrown")]
    BoostExcept,

    #[error("A std::exception was thrown")]
    StdExcept,

    /// A string containing a null byte was provided
    #[error("Null byte in input string")]
    NullByte,

    /// A string was too long to be copied out through C FFI
    #[error("String from FFI is too long")]
    StringLength,
    /// A string from C FFI contained invalid UTF-8
    #[error("String from FFI contains invalid UTF-8")]
    Utf8,

    #[error("Unknown error")]
    Unknown,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Returns a string copied using uhd_get_last_error()
pub fn last_error_message() -> Option<String> {
    copy_string(|buffer, length| unsafe { uhd_sys::uhd_get_last_error(buffer, length as _) }).ok()
}

/// Converts a status code into a result
pub(crate) fn check_status(status: uhd_sys::uhd_error::Type) -> Result<()> {
    use uhd_sys::uhd_error;
    use Error::*;
    let iserr = match status {
        uhd_error::UHD_ERROR_NONE => None,
        uhd_error::UHD_ERROR_INVALID_DEVICE => Some(InvalidDevice),
        uhd_error::UHD_ERROR_INDEX => Some(Index),
        uhd_error::UHD_ERROR_KEY => Some(Key),
        uhd_error::UHD_ERROR_NOT_IMPLEMENTED => Some(NotImplemented),
        uhd_error::UHD_ERROR_USB => Some(Usb),
        uhd_error::UHD_ERROR_IO => Some(Io),
        uhd_error::UHD_ERROR_OS => Some(Os),
        uhd_error::UHD_ERROR_ASSERTION => Some(Assertion),
        uhd_error::UHD_ERROR_LOOKUP => Some(Lookup),
        uhd_error::UHD_ERROR_TYPE => Some(Type),
        uhd_error::UHD_ERROR_VALUE => Some(Value),
        uhd_error::UHD_ERROR_RUNTIME => Some(Runtime),
        uhd_error::UHD_ERROR_ENVIRONMENT => Some(Environment),
        uhd_error::UHD_ERROR_SYSTEM => Some(System),
        uhd_error::UHD_ERROR_EXCEPT => Some(Except),
        uhd_error::UHD_ERROR_BOOSTEXCEPT => Some(BoostExcept),
        uhd_error::UHD_ERROR_STDEXCEPT => Some(StdExcept),
        uhd_error::UHD_ERROR_UNKNOWN => Some(Unknown),
        _ => Some(Unknown),
    };
    match iserr {
        std::option::Option::Some(e) => Err(e),
        std::option::Option::None => Ok(()),
    }
}

impl From<NulError> for Error {
    fn from(_: NulError) -> Self {
        Error::NullByte
    }
}

impl From<Utf8Error> for Error {
    fn from(_: Utf8Error) -> Self {
        Error::Utf8
    }
}
