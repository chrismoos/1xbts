use std::marker::PhantomData;
use std::os::raw::c_void;
use std::ptr;

use crate::{
    error::{check_status, Error},
    utils::check_equal_buffer_lengths,
    TransmitMetadata,
};

/// A streamer used to transmit samples from a USRP
///
/// The type parameter I is the type of sample that this streamer transmits.
#[derive(Debug)]
pub struct TransmitStreamer<I> {
    /// Streamer handle
    handle: uhd_sys::uhd_tx_streamer_handle,

    /// A vector of pointers to buffers (used in transmit() to convert `&mut [&[I]]` to `*mut *const I`
    /// without reallocating memory each time
    ///
    /// Invariant: If this is not empty, its length is equal to the value returned by
    /// self.num_channels().
    buffer_pointers: Vec<*const c_void>,
    /// Item type phantom data
    item_phantom: PhantomData<I>,
}

impl<I> TransmitStreamer<I> {
    /// Creates a transmit streamer with a null streamer handle (for internal use only)
    ///
    /// After creating a streamer with this function, its streamer handle must be initialized.
    pub(crate) fn new() -> Self {
        TransmitStreamer {
            handle: ptr::null_mut(),
            buffer_pointers: Vec::new(),
            item_phantom: PhantomData,
        }
    }

    /// Returns a reference to the streamer handle
    pub(crate) fn handle_mut(&mut self) -> &mut uhd_sys::uhd_tx_streamer_handle {
        &mut self.handle
    }
    /// Returns the streamer handle
    pub(crate) fn handle(&mut self) -> uhd_sys::uhd_tx_streamer_handle {
        self.handle
    }

    /// Returns the number of channels that this streamer is associated with
    pub fn num_channels(&self) -> Result<usize, Error> {
        let mut num_channels = 0usize;
        check_status(unsafe {
            uhd_sys::uhd_tx_streamer_num_channels(
                self.handle,
                &mut num_channels as *mut usize as *mut _,
            )
        })?;
        Ok(num_channels)
    }

    /// transmits samples from the USRP
    ///
    /// buffers: One or more buffers (one per channel) containing sample to transmit. All
    /// buffers should have the same length. This function will panic if the number of buffers
    /// is not equal to self.num_channels(), or if not all buffers have the same length.
    ///
    /// timeout: The timeout for the transmit operation, in seconds
    ///
    /// On success, this function returns a transmitMetadata object with information about
    /// the number of samples actually transmitd.
    pub fn transmit(
        &mut self,
        buffers: &mut [&[I]],
        timeout: f64,
    ) -> Result<TransmitMetadata, Error> {
        let mut metadata = TransmitMetadata::new()?;
        let mut samples_transmitted = 0usize;

        // Initialize buffer_pointers
        if self.buffer_pointers.is_empty() {
            self.buffer_pointers
                .resize(self.num_channels()?, ptr::null_mut());
        }
        // Now buffer_pointers.len() is equal to self.num_channels().
        assert_eq!(
            buffers.len(),
            self.buffer_pointers.len(),
            "Number of buffers is not equal to this streamer's number of channels"
        );
        // Check that all buffers have the same length
        let buffer_length = check_equal_buffer_lengths(buffers);

        // Copy buffer pointers into C-compatible form
        for (entry, buffer) in self.buffer_pointers.iter_mut().zip(buffers.iter_mut()) {
            *entry = buffer.as_ptr() as *mut c_void;
        }

        check_status(unsafe {
            uhd_sys::uhd_tx_streamer_send(
                self.handle,
                self.buffer_pointers.as_mut_ptr(),
                buffer_length as _,
                metadata.handle_mut(),
                timeout,
                &mut samples_transmitted as *mut usize as *mut _,
            )
        })?;
        metadata.set_samples(samples_transmitted);

        Ok(metadata)
    }

    /// transmits samples on a single channel with a timeout of 0.1 seconds and
    /// one_packet disabled
    pub fn transmit_simple(&mut self, buffer: &mut [I]) -> Result<TransmitMetadata, Error> {
        self.transmit(&mut [buffer], 0.1)
    }

    /// Transmits samples with caller-provided metadata (for timed transmission).
    ///
    /// buffers: One buffer per channel containing samples to transmit.
    /// metadata: Pre-built transmit metadata (e.g., with time spec for timed TX).
    /// timeout: The timeout for the transmit operation, in seconds.
    ///
    /// Returns the number of samples actually transmitted.
    pub fn send_with_metadata(
        &mut self,
        buffers: &mut [&[I]],
        metadata: &TransmitMetadata,
        timeout: f64,
    ) -> Result<usize, Error> {
        if self.buffer_pointers.is_empty() {
            self.buffer_pointers
                .resize(self.num_channels()?, ptr::null_mut());
        }
        assert_eq!(
            buffers.len(),
            self.buffer_pointers.len(),
            "Number of buffers is not equal to this streamer's number of channels"
        );
        let buffer_length = check_equal_buffer_lengths(buffers);

        for (entry, buffer) in self.buffer_pointers.iter_mut().zip(buffers.iter_mut()) {
            *entry = buffer.as_ptr() as *mut c_void;
        }

        let mut md_handle = metadata.handle();
        let mut samples_transmitted = 0usize;
        check_status(unsafe {
            uhd_sys::uhd_tx_streamer_send(
                self.handle,
                self.buffer_pointers.as_mut_ptr(),
                buffer_length as _,
                &mut md_handle,
                timeout,
                &mut samples_transmitted as *mut usize as *mut _,
            )
        })?;
        Ok(samples_transmitted)
    }
}

impl<I> Drop for TransmitStreamer<I> {
    fn drop(&mut self) {
        let _ = unsafe { uhd_sys::uhd_tx_streamer_free(&mut self.handle) };
    }
}

// Thread safety: see https://files.ettus.com/manual/page_general.html#general_threading
// All functions are thread-safe, except that the uhd_tx_streamer send(), uhd_tx_streamer recv(), and
// uhd_tx_streamer recv_async_msg() functions. The corresponding Rust wrapper functions take &mut
// self, which enforces single-thread access.
unsafe impl<I> Send for TransmitStreamer<I> {}
unsafe impl<I> Sync for TransmitStreamer<I> {}
