use thiserror::Error as ThisError;

#[derive(ThisError, Debug)]
pub enum Error {
    #[error("LimeSuite error: {0}")]
    Lms(String),

    #[error("LimeSuite: device not found")]
    DeviceNotFound,

    #[error("LimeSuite: stream not active")]
    StreamNotActive,
}

/// Check a LimeSuite return code. Returns Ok(()) if >= 0, Err otherwise.
/// LMS functions return 0 on success, -1 on failure.
pub(crate) fn check_lms(ret: i32, context: &str) -> Result<(), Error> {
    if ret < 0 {
        // Try to get last error message from LimeSuite
        let msg = unsafe {
            let ptr = limesuite_sys::LMS_GetLastErrorMessage();
            if ptr.is_null() {
                context.to_string()
            } else {
                let cstr = std::ffi::CStr::from_ptr(ptr);
                format!("{}: {}", context, cstr.to_string_lossy())
            }
        };
        Err(Error::Lms(msg))
    } else {
        Ok(())
    }
}
