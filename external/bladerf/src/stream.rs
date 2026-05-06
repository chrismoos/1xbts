use std::mem::MaybeUninit;
use std::sync::Arc;

use num_complex::Complex32;

use crate::device::Device;
use crate::error::{check_bladerf, Error};

/// SC16 Q11 sample: interleaved I16 I/Q pairs used by bladeRF hardware.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Sc16Q11 {
    pub i: i16,
    pub q: i16,
}

impl Sc16Q11 {
    /// Convert from Complex32 (float -1.0..1.0) to SC16 Q11 (signed 12-bit in 16-bit).
    pub fn from_complex32(s: Complex32) -> Self {
        const SCALE: f32 = 2047.0;
        Sc16Q11 {
            i: (s.re * SCALE).round().clamp(-2048.0, 2047.0) as i16,
            q: (s.im * SCALE).round().clamp(-2048.0, 2047.0) as i16,
        }
    }

    /// Convert to Complex32.
    pub fn to_complex32(self) -> Complex32 {
        const INV_SCALE: f32 = 1.0 / 2047.0;
        Complex32::new(self.i as f32 * INV_SCALE, self.q as f32 * INV_SCALE)
    }
}

/// Metadata for timestamped synchronous operations.
#[derive(Debug, Clone, Default)]
pub struct StreamMeta {
    pub timestamp: u64,
    pub flags: u32,
    pub status: u32,
    pub actual_count: u32,
}

/// Synchronous TX stream.
///
/// Holds an `Arc<Device>` to prevent the device from being dropped while
/// this stream is alive.
pub struct TxSync {
    device: *mut bladerf_sys::bladerf,
    _owner: Arc<Device>,
}

impl TxSync {
    /// Create a TX sync interface. The device must already have sync_config called for TX.
    pub fn new(device: &Arc<Device>) -> Self {
        TxSync {
            device: device.raw(),
            _owner: Arc::clone(device),
        }
    }

    /// Send samples with optional metadata. Returns Ok(()) on success.
    pub fn send(
        &self,
        samples: &[Sc16Q11],
        meta: Option<&mut StreamMeta>,
        timeout_ms: u32,
    ) -> Result<(), Error> {
        match meta {
            Some(m) => {
                let mut c_meta = bladerf_sys::bladerf_metadata {
                    timestamp: m.timestamp,
                    flags: m.flags,
                    status: 0,
                    actual_count: 0,
                    reserved: [0u8; 32],
                };
                check_bladerf(
                    unsafe {
                        bladerf_sys::bladerf_sync_tx(
                            self.device,
                            samples.as_ptr() as *const _,
                            samples.len() as u32,
                            &mut c_meta,
                            timeout_ms,
                        )
                    },
                    "bladerf_sync_tx",
                )?;
                m.status = c_meta.status;
                m.actual_count = c_meta.actual_count;
                Ok(())
            }
            None => check_bladerf(
                unsafe {
                    bladerf_sys::bladerf_sync_tx(
                        self.device,
                        samples.as_ptr() as *const _,
                        samples.len() as u32,
                        std::ptr::null_mut(),
                        timeout_ms,
                    )
                },
                "bladerf_sync_tx",
            ),
        }
    }
}

/// Synchronous RX stream.
///
/// Holds an `Arc<Device>` to prevent the device from being dropped while
/// this stream is alive.
pub struct RxSync {
    device: *mut bladerf_sys::bladerf,
    _owner: Arc<Device>,
}

impl RxSync {
    /// Create an RX sync interface. The device must already have sync_config called for RX.
    pub fn new(device: &Arc<Device>) -> Self {
        RxSync {
            device: device.raw(),
            _owner: Arc::clone(device),
        }
    }

    /// Receive samples with metadata. Returns the number of samples received.
    pub fn recv(
        &self,
        buf: &mut [Sc16Q11],
        meta: &mut StreamMeta,
        timeout_ms: u32,
    ) -> Result<usize, Error> {
        let mut c_meta =
            unsafe { MaybeUninit::<bladerf_sys::bladerf_metadata>::zeroed().assume_init() };
        c_meta.flags = meta.flags;

        check_bladerf(
            unsafe {
                bladerf_sys::bladerf_sync_rx(
                    self.device,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    &mut c_meta,
                    timeout_ms,
                )
            },
            "bladerf_sync_rx",
        )?;
        meta.timestamp = c_meta.timestamp;
        meta.status = c_meta.status;
        meta.actual_count = c_meta.actual_count;
        Ok(c_meta.actual_count as usize)
    }
}

unsafe impl Send for TxSync {}
unsafe impl Send for RxSync {}
