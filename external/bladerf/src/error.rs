use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum Error {
    #[error("bladeRF error: {0}")]
    BladeRf(String),

    #[error("bladeRF: device not found")]
    DeviceNotFound,

    #[error("bladeRF: stream not active")]
    StreamNotActive,
}

/// Check a libbladeRF return code. Returns Ok(()) if 0, Err otherwise.
pub(crate) fn check_bladerf(ret: i32, context: &str) -> Result<(), Error> {
    if ret != 0 {
        let msg = unsafe {
            let ptr = bladerf_sys::bladerf_strerror(ret);
            if ptr.is_null() {
                context.to_string()
            } else {
                let cstr = std::ffi::CStr::from_ptr(ptr);
                format!("{}: {}", context, cstr.to_string_lossy())
            }
        };
        Err(Error::BladeRf(msg))
    } else {
        Ok(())
    }
}
