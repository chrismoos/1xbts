use std::ffi::CString;
use std::ptr;

use crate::error::{check_lms, Error};

/// An open connection to a LimeSDR device.
pub struct Device(pub(crate) *mut limesuite_sys::lms_device_t);

impl Device {
    /// List available LimeSDR devices. Returns device info strings.
    pub fn list() -> Result<Vec<String>, Error> {
        // First call with NULL to get count
        let count = unsafe { limesuite_sys::LMS_GetDeviceList(ptr::null_mut()) };
        if count < 0 {
            return Err(Error::Lms("LMS_GetDeviceList failed".into()));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut list: Vec<limesuite_sys::lms_info_str_t> = vec![[0; 256]; count as usize];
        let ret = unsafe { limesuite_sys::LMS_GetDeviceList(list.as_mut_ptr()) };
        if ret < 0 {
            return Err(Error::Lms("LMS_GetDeviceList failed".into()));
        }
        let result = list
            .iter()
            .take(ret as usize)
            .map(|info| {
                let cstr = unsafe { std::ffi::CStr::from_ptr(info.as_ptr()) };
                cstr.to_string_lossy().into_owned()
            })
            .collect();
        Ok(result)
    }

    /// Open a LimeSDR device. Pass None to open the first available device.
    pub fn open(info: Option<&str>) -> Result<Self, Error> {
        let mut handle: *mut limesuite_sys::lms_device_t = ptr::null_mut();
        let ret = match info {
            Some(s) => {
                let c = CString::new(s).map_err(|_| Error::Lms("invalid device string".into()))?;
                unsafe {
                    limesuite_sys::LMS_Open(&mut handle, c.as_ptr() as *const _, ptr::null_mut())
                }
            }
            None => unsafe { limesuite_sys::LMS_Open(&mut handle, ptr::null(), ptr::null_mut()) },
        };
        check_lms(ret, "LMS_Open")?;
        if handle.is_null() {
            return Err(Error::DeviceNotFound);
        }
        Ok(Device(handle))
    }

    /// Initialize the device with default configuration.
    pub fn init(&mut self) -> Result<(), Error> {
        check_lms(unsafe { limesuite_sys::LMS_Init(self.0) }, "LMS_Init")
    }

    /// Enable or disable a channel.
    pub fn enable_channel(
        &mut self,
        dir_tx: bool,
        chan: usize,
        enabled: bool,
    ) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_EnableChannel(self.0, dir_tx, chan, enabled) },
            "LMS_EnableChannel",
        )
    }

    /// Set the sample rate for both TX and RX. oversample=0 for auto.
    pub fn set_sample_rate(&mut self, rate: f64, oversample: usize) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_SetSampleRate(self.0, rate, oversample) },
            "LMS_SetSampleRate",
        )
    }

    /// Get the host sample rate for a channel.
    pub fn get_sample_rate(&self, dir_tx: bool, chan: usize) -> Result<f64, Error> {
        let mut host_hz: f64 = 0.0;
        let mut rf_hz: f64 = 0.0;
        check_lms(
            unsafe {
                limesuite_sys::LMS_GetSampleRate(self.0, dir_tx, chan, &mut host_hz, &mut rf_hz)
            },
            "LMS_GetSampleRate",
        )?;
        Ok(host_hz)
    }

    /// Set the LO frequency for a channel.
    pub fn set_lo_frequency(&mut self, dir_tx: bool, chan: usize, freq: f64) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_SetLOFrequency(self.0, dir_tx, chan, freq) },
            "LMS_SetLOFrequency",
        )
    }

    /// Get the LO frequency for a channel.
    pub fn get_lo_frequency(&self, dir_tx: bool, chan: usize) -> Result<f64, Error> {
        let mut freq: f64 = 0.0;
        check_lms(
            unsafe { limesuite_sys::LMS_GetLOFrequency(self.0, dir_tx, chan, &mut freq) },
            "LMS_GetLOFrequency",
        )?;
        Ok(freq)
    }

    /// Set the antenna index for a channel.
    pub fn set_antenna(&mut self, dir_tx: bool, chan: usize, index: usize) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_SetAntenna(self.0, dir_tx, chan, index) },
            "LMS_SetAntenna",
        )
    }

    /// Get the current antenna index.
    pub fn get_antenna(&self, dir_tx: bool, chan: usize) -> Result<usize, Error> {
        let ret = unsafe { limesuite_sys::LMS_GetAntenna(self.0, dir_tx, chan) };
        if ret < 0 {
            return Err(Error::Lms("LMS_GetAntenna failed".into()));
        }
        Ok(ret as usize)
    }

    /// Set gain in dB for a channel.
    pub fn set_gain_db(&mut self, dir_tx: bool, chan: usize, gain: u32) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_SetGaindB(self.0, dir_tx, chan, gain) },
            "LMS_SetGaindB",
        )
    }

    /// Set the analog LPF bandwidth.
    pub fn set_lpf_bw(&mut self, dir_tx: bool, chan: usize, bw: f64) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_SetLPFBW(self.0, dir_tx, chan, bw) },
            "LMS_SetLPFBW",
        )
    }

    /// Run automatic calibration.
    pub fn calibrate(&mut self, dir_tx: bool, chan: usize, bw: f64) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_Calibrate(self.0, dir_tx, chan, bw, 0) },
            "LMS_Calibrate",
        )
    }

    /// Set a clock frequency. clk_id: 0=REF, 3=CGEN, etc.
    pub fn set_clock_freq(&mut self, clk_id: usize, freq: f64) -> Result<(), Error> {
        check_lms(
            unsafe { limesuite_sys::LMS_SetClockFreq(self.0, clk_id, freq) },
            "LMS_SetClockFreq",
        )
    }

    /// Get a clock frequency.
    pub fn get_clock_freq(&self, clk_id: usize) -> Result<f64, Error> {
        let mut freq: f64 = 0.0;
        check_lms(
            unsafe { limesuite_sys::LMS_GetClockFreq(self.0, clk_id, &mut freq) },
            "LMS_GetClockFreq",
        )?;
        Ok(freq)
    }

    /// Get the number of channels for a direction.
    pub fn num_channels(&self, dir_tx: bool) -> Result<usize, Error> {
        let ret = unsafe { limesuite_sys::LMS_GetNumChannels(self.0, dir_tx) };
        if ret < 0 {
            return Err(Error::Lms("LMS_GetNumChannels failed".into()));
        }
        Ok(ret as usize)
    }

    /// List available antenna names for a direction/channel.
    pub fn antenna_list(&self, dir_tx: bool, chan: usize) -> Result<Vec<String>, Error> {
        let count = unsafe {
            limesuite_sys::LMS_GetAntennaList(self.0, dir_tx, chan, std::ptr::null_mut())
        };
        if count <= 0 {
            return Ok(Vec::new());
        }
        let mut names: Vec<limesuite_sys::lms_name_t> = vec![[0; 16]; count as usize];
        let ret =
            unsafe { limesuite_sys::LMS_GetAntennaList(self.0, dir_tx, chan, names.as_mut_ptr()) };
        if ret <= 0 {
            return Ok(Vec::new());
        }
        let result = names
            .iter()
            .take(ret as usize)
            .map(|entry| {
                let cstr = unsafe { std::ffi::CStr::from_ptr(entry.as_ptr()) };
                cstr.to_string_lossy().into_owned()
            })
            .collect();
        Ok(result)
    }

    /// Raw device pointer for stream setup.
    pub(crate) fn raw(&mut self) -> *mut limesuite_sys::lms_device_t {
        self.0
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { limesuite_sys::LMS_Close(self.0) };
        }
    }
}

// SAFETY: LimeSuite API calls are externally synchronized by the caller (the
// BTS RX thread owns the device exclusively). The raw pointer is only freed in
// Drop, which runs once.
unsafe impl Send for Device {}
unsafe impl Sync for Device {}
