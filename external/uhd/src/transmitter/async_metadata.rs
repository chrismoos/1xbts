use std::ptr;

use crate::error::{check_status, Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransmitAsyncEvent {
    pub code: u32,
}

#[derive(Debug)]
pub(crate) struct AsyncMetadata {
    handle: uhd_sys::uhd_async_metadata_handle,
}

impl AsyncMetadata {
    pub(crate) fn new() -> Result<Self, Error> {
        let mut handle = ptr::null_mut();
        check_status(unsafe { uhd_sys::uhd_async_metadata_make(&mut handle) })?;
        Ok(Self { handle })
    }

    pub(crate) fn handle_mut(&mut self) -> &mut uhd_sys::uhd_async_metadata_handle {
        &mut self.handle
    }

    pub(crate) fn event(&self) -> Result<TransmitAsyncEvent, Error> {
        let mut code = 0;
        check_status(unsafe { uhd_sys::uhd_async_metadata_event_code(self.handle, &mut code) })?;
        Ok(TransmitAsyncEvent { code })
    }
}

impl Drop for AsyncMetadata {
    fn drop(&mut self) {
        let _ = unsafe { uhd_sys::uhd_async_metadata_free(&mut self.handle) };
    }
}
