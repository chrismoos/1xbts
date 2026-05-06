use crate::{
    error::{check_status, Error},
    motherboard_eeprom::MotherboardEeprom,
    range::MetaRange,
    stream::{Item, StreamArgs, StreamArgsC},
    string_vector::StringVector,
    utils::copy_string,
    DaughterBoardEeprom, ReceiveInfo, ReceiveStreamer, TimeSpec, TransmitInfo, TransmitStreamer,
    TuneRequest, TuneResult,
};

use std::convert::TryInto;
use std::ffi::CString;
use std::ptr;
/// A connection to a USRP device
pub struct Usrp(uhd_sys::uhd_usrp_handle);

impl Usrp {
    pub fn find(args: &str) -> Result<Vec<String>, Error> {
        let args = CString::new(args)?;
        let mut addresses = StringVector::new()?;
        check_status(unsafe { uhd_sys::uhd_usrp_find(args.as_ptr(), addresses.handle_mut()) })?;
        Ok(addresses.into())
    }

    /// Opens a connection to a USRP
    ///
    /// args: A string with parameters for the USRP connection. If this is an empty string,
    /// one available USRP will be opened with the default settings. Arguments can be specified
    /// with the syntax `key=value`, with key-value pairs separated by commas.
    ///
    /// Frequently used arguments:
    /// * `addr`: The IP address of the USRP
    /// * `type`: The type of the USRP (allowed values include `usrp2` and others)
    ///
    pub fn open(args: &str) -> Result<Self, Error> {
        let mut handle: uhd_sys::uhd_usrp_handle = ptr::null_mut();
        let args_c = CString::new(args)?;
        check_status(unsafe { uhd_sys::uhd_usrp_make(&mut handle, args_c.as_ptr()) })?;
        Ok(Usrp(handle))
    }

    /// Returns the antennas available for transmission
    pub fn get_tx_antennas(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_antennas(self.0, channel as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the selected antenna for transmission
    pub fn get_tx_antenna(&self, channel: usize) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_usrp_get_tx_antenna(self.0, channel as _, buffer, length as _)
        })
    }

    /// Returns the antennas available for receiving
    pub fn get_rx_antennas(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_antennas(self.0, channel as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the selected antenna for receiving
    pub fn get_rx_antenna(&self, channel: usize) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_usrp_get_rx_antenna(self.0, channel as _, buffer, length as _)
        })
    }

    /// Returns the current receive front-end bandwidth
    pub fn get_rx_bandwidth(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_bandwidth(self.0, channel as _, &mut value)
        })?;
        Ok(value)
    }

    /// Returns the supported range of receive front-end bandwidth
    pub fn get_rx_bandwidth_range(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_bandwidth_range(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the current transmit front-end bandwidth
    pub fn get_tx_bandwidth(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_bandwidth(self.0, channel as _, &mut value)
        })?;
        Ok(value)
    }

    /// Returns the supported range of transmit front-end bandwidth
    pub fn get_tx_bandwidth_range(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_bandwidth_range(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the current receive frequency
    pub fn get_rx_frequency(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe { uhd_sys::uhd_usrp_get_rx_freq(self.0, channel as _, &mut value) })?;
        Ok(value)
    }

    /// Returns the supported range of receive frequencies
    pub fn get_rx_frequency_range(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_freq_range(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the current transmit frequency
    pub fn get_tx_frequency(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe { uhd_sys::uhd_usrp_get_tx_freq(self.0, channel as _, &mut value) })?;
        Ok(value)
    }

    /// Returns the supported range of transmit frequencies
    pub fn get_tx_frequency_range(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_freq_range(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the current gain of the gain element with the specified name
    pub fn get_rx_gain(&self, channel: usize, name: &str) -> Result<f64, Error> {
        let name = CString::new(name)?;
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_gain(self.0, channel as _, name.as_ptr(), &mut value)
        })?;
        Ok(value)
    }
    /// Returns the names of controllable gain elements
    pub fn get_rx_gain_names(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut names = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_gain_names(self.0, channel as _, names.handle_mut())
        })?;
        Ok(names.into())
    }

    /// Returns the range(s) of gains for a gain element
    pub fn get_rx_gain_range(&self, channel: usize, name: &str) -> Result<MetaRange, Error> {
        let name = CString::new(name)?;
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_gain_range(self.0, name.as_ptr(), channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the current gain of the gain element with the specified name
    pub fn get_tx_gain(&self, channel: usize, name: &str) -> Result<f64, Error> {
        let name = CString::new(name)?;
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_gain(self.0, channel as _, name.as_ptr(), &mut value)
        })?;
        Ok(value)
    }
    /// Returns the names of controllable gain elements
    pub fn get_tx_gain_names(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut names = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_gain_names(self.0, channel as _, names.handle_mut())
        })?;
        Ok(names.into())
    }

    /// Returns the range(s) of gains for a gain element
    pub fn get_tx_gain_range(&self, channel: usize, name: &str) -> Result<MetaRange, Error> {
        let name = CString::new(name)?;
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_gain_range(self.0, name.as_ptr(), channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Clears the command time (?), causing stream commands to be sent immediately
    pub fn clear_command_time(&mut self, mboard: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_clear_command_time(self.0, mboard as _) })
    }

    /// Gets the ranges of front-end frequencies for a receive channel
    pub fn get_fe_rx_freq_range(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_fe_rx_freq_range(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Gets the ranges of front-end frequencies for a transmit channel
    pub fn get_fe_tx_freq_range(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_fe_tx_freq_range(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Sets the frequency of the master clock
    pub fn set_master_clock_rate(&mut self, rate: f64, mboard: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_set_master_clock_rate(self.0, rate, mboard as _) })
    }

    /// Returns the frequency of the master clock
    pub fn get_master_clock_rate(&self, mboard: usize) -> Result<f64, Error> {
        let mut rate = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_master_clock_rate(self.0, mboard as _, &mut rate)
        })?;
        Ok(rate)
    }

    /// Returns the name of the motherboard
    pub fn get_motherboard_name(&self, mboard: usize) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_usrp_get_mboard_name(self.0, mboard as _, buffer, length as _)
        })
    }

    /// Returns the transmit gain, normalized to [0, 1]
    pub fn get_normalized_tx_gain(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_normalized_tx_gain(self.0, channel as _, &mut value)
        })?;
        Ok(value)
    }

    /// Returns the receive gain, normalized to [0, 1]
    pub fn get_normalized_rx_gain(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_normalized_rx_gain(self.0, channel as _, &mut value)
        })?;
        Ok(value)
    }

    /// Returns the number of motherboards that this Usrp object provides access to
    pub fn get_num_motherboards(&self) -> Result<usize, Error> {
        let mut value = 0usize;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_num_mboards(self.0, &mut value as *mut usize as *mut _)
        })?;
        Ok(value)
    }

    /// Returns the number of transmit channels
    pub fn get_num_tx_channels(&self) -> Result<usize, Error> {
        let mut value = 0usize;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_num_channels(self.0, &mut value as *mut usize as *mut _)
        })?;
        Ok(value)
    }

    /// Returns the number of receive channels
    pub fn get_num_rx_channels(&self) -> Result<usize, Error> {
        let mut value = 0usize;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_num_channels(self.0, &mut value as *mut usize as *mut _)
        })?;
        Ok(value)
    }

    /// Writes a user register on the USRP
    ///
    /// address: The address of the register
    /// value: The value to write
    /// mboard: The index of the board to write to (normally 0 when there is only one USRP)
    pub fn set_user_register(
        &mut self,
        address: u8,
        value: u32,
        mboard: usize,
    ) -> Result<(), Error> {
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_user_register(self.0, address, value, mboard as _)
        })
    }

    /// Returns the current clock source
    pub fn get_clock_source(&self, mboard: usize) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_usrp_get_clock_source(self.0, mboard as _, buffer, length as _)
        })
    }
    /// Returns the available clock sources
    pub fn get_clock_sources(&self, mboard: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_clock_sources(self.0, mboard as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the current time source
    pub fn get_time_source(&self, mboard: usize) -> Result<String, Error> {
        copy_string(|buffer, length| unsafe {
            uhd_sys::uhd_usrp_get_time_source(self.0, mboard as _, buffer, length as _)
        })
    }

    /// Returns the available time sources
    pub fn get_time_sources(&self, mboard: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_time_sources(self.0, mboard as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the available sensors on the motherboard
    pub fn get_mboard_sensor_names(&self, mboard: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_mboard_sensor_names(self.0, mboard as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the values stored in the motherboard EEPROM
    pub fn get_motherboard_eeprom(&self, mboard: usize) -> Result<MotherboardEeprom, Error> {
        let mut eeprom = MotherboardEeprom::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_mboard_eeprom(self.0, eeprom.handle(), mboard as _)
        })?;
        Ok(eeprom)
    }

    /// Returns the values stored in a daughter board EEPROM
    ///
    /// Values for unit and slot can be determined by running uhd_usrp_probe --tree.
    /// As an example, the entry `/mboards/0/dboards/A/rx_eeprom` corresponds to unit `rx` and
    /// slot `A` of mboard 1.
    pub fn get_daughter_board_eeprom(
        &self,
        unit: &str,
        slot: &str,
        mboard: usize,
    ) -> Result<DaughterBoardEeprom, Error> {
        let unit = CString::new(unit)?;
        let slot = CString::new(slot)?;

        let mut eeprom = DaughterBoardEeprom::new()?;

        check_status(unsafe {
            uhd_sys::uhd_usrp_get_dboard_eeprom(
                self.0,
                eeprom.handle(),
                unit.as_ptr(),
                slot.as_ptr(),
                mboard as _,
            )
        })?;

        Ok(eeprom)
    }

    /// Gets information about the receive configuration of a channel
    pub fn get_rx_info(&self, channel: usize) -> Result<ReceiveInfo, Error> {
        let mut info_c = uhd_sys::uhd_usrp_rx_info_t {
            mboard_id: ptr::null_mut(),
            mboard_name: ptr::null_mut(),
            mboard_serial: ptr::null_mut(),
            rx_id: ptr::null_mut(),
            rx_subdev_name: ptr::null_mut(),
            rx_subdev_spec: ptr::null_mut(),
            rx_serial: ptr::null_mut(),
            rx_antenna: ptr::null_mut(),
        };
        unsafe {
            check_status(uhd_sys::uhd_usrp_get_rx_info(
                self.0,
                channel as _,
                &mut info_c,
            ))?;
            let info = ReceiveInfo::from_c(&info_c)?;
            uhd_sys::uhd_usrp_rx_info_free(&mut info_c);
            Ok(info)
        }
    }

    /// Gets information about the transmit configuration of a channel
    pub fn get_tx_info(&self, channel: usize) -> Result<TransmitInfo, Error> {
        let mut info_c = uhd_sys::uhd_usrp_tx_info_t {
            mboard_id: ptr::null_mut(),
            mboard_name: ptr::null_mut(),
            mboard_serial: ptr::null_mut(),
            tx_id: ptr::null_mut(),
            tx_subdev_name: ptr::null_mut(),
            tx_subdev_spec: ptr::null_mut(),
            tx_serial: ptr::null_mut(),
            tx_antenna: ptr::null_mut(),
        };
        unsafe {
            check_status(uhd_sys::uhd_usrp_get_tx_info(
                self.0,
                channel as _,
                &mut info_c,
            ))?;
            let info = TransmitInfo::from_c(&info_c)?;
            uhd_sys::uhd_usrp_tx_info_free(&mut info_c);
            Ok(info)
        }
    }

    /// Returns true if the provided local oscillator is exported
    pub fn get_rx_lo_export_enabled(&self, channel: usize, name: &str) -> Result<bool, Error> {
        let name = CString::new(name)?;
        let mut enabled = false;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_lo_export_enabled(
                self.0,
                name.as_ptr(),
                channel as _,
                &mut enabled,
            )
        })?;
        Ok(enabled)
    }

    /// Returns true if the provided local oscillator is exported
    pub fn get_tx_lo_export_enabled(&self, channel: usize, name: &str) -> Result<bool, Error> {
        let name = CString::new(name)?;
        let mut enabled = false;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_lo_export_enabled(
                self.0,
                name.as_ptr(),
                channel as _,
                &mut enabled,
            )
        })?;
        Ok(enabled)
    }

    /// Returns the frequency of a local oscillator
    pub fn get_rx_lo_frequency(&self, channel: usize, name: &str) -> Result<f64, Error> {
        let name = CString::new(name)?;
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_lo_freq(self.0, name.as_ptr(), channel as _, &mut value)
        })?;
        Ok(value)
    }

    /// Returns the names of local oscillators
    pub fn get_rx_lo_names(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_lo_names(self.0, channel as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the names of sensors that relate to receiving
    pub fn get_rx_sensor_names(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_sensor_names(self.0, channel as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the frequency of a local oscillator
    pub fn get_tx_lo_frequency(&self, channel: usize, name: &str) -> Result<f64, Error> {
        let name = CString::new(name)?;
        let mut value = 0.0;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_lo_freq(self.0, name.as_ptr(), channel as _, &mut value)
        })?;
        Ok(value)
    }

    /// Returns the names of local oscillators
    pub fn get_tx_lo_names(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_lo_names(self.0, channel as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Returns the names of sensors that relate to transmitting
    pub fn get_tx_sensor_names(&self, channel: usize) -> Result<Vec<String>, Error> {
        let mut vector = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_sensor_names(self.0, channel as _, vector.handle_mut())
        })?;
        Ok(vector.into())
    }

    /// Opens a stream that can be used to receive samples
    pub fn get_rx_stream<I>(&mut self, args: &StreamArgs<I>) -> Result<ReceiveStreamer<I>, Error>
    where
        I: Item,
    {
        // Convert arguments
        let args: StreamArgsC = args.try_into()?;
        // Convert some *T pointers to *mut T pointers. The C API doesn't mark them const, but
        // appears to not write to them.
        let mut args_c = uhd_sys::uhd_stream_args_t {
            cpu_format: args.host_format.as_ptr() as *mut _,
            otw_format: args.wire_format.as_ptr() as *mut _,
            args: args.args.as_ptr() as *mut _,
            channel_list: args.channels.as_ptr() as *mut _,
            n_channels: args
                .channels
                .len()
                .try_into()
                .expect("Number of channels too large"),
        };

        // Create a streamer
        let mut streamer = ReceiveStreamer::new();
        check_status(unsafe { uhd_sys::uhd_rx_streamer_make(streamer.handle_mut()) })?;
        // Associate streamer with USRP
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_stream(self.0, &mut args_c, streamer.handle())
        })?;

        Ok(streamer)
    }

    /// Opens a stream that can be used to transmit samples
    pub fn get_tx_stream<I>(&mut self, args: &StreamArgs<I>) -> Result<TransmitStreamer<I>, Error>
    where
        I: Item,
    {
        // Convert arguments
        let args: StreamArgsC = args.try_into()?;
        // Convert some *T pointers to *mut T pointers. The C API doesn't mark them const, but
        // appears to not write to them.
        let mut args_c = uhd_sys::uhd_stream_args_t {
            cpu_format: args.host_format.as_ptr() as *mut _,
            otw_format: args.wire_format.as_ptr() as *mut _,
            args: args.args.as_ptr() as *mut _,
            channel_list: args.channels.as_ptr() as *mut _,
            n_channels: args
                .channels
                .len()
                .try_into()
                .expect("Number of channels too large"),
        };

        // Create a streamer
        let mut streamer = TransmitStreamer::new();
        check_status(unsafe { uhd_sys::uhd_tx_streamer_make(streamer.handle_mut()) })?;
        // Associate streamer with USRP
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_stream(self.0, &mut args_c, streamer.handle())
        })?;

        Ok(streamer)
    }

    /// Returns the current receive sample rate in samples/second
    pub fn get_rx_sample_rate(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe { uhd_sys::uhd_usrp_get_rx_rate(self.0, channel as _, &mut value) })?;
        Ok(value)
    }

    /// Returns the ranges of supported sample rates
    pub fn get_rx_sample_rates(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_rx_rates(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the current transmit sample rate in samples/second
    pub fn get_tx_sample_rate(&self, channel: usize) -> Result<f64, Error> {
        let mut value = 0.0;
        check_status(unsafe { uhd_sys::uhd_usrp_get_tx_rate(self.0, channel as _, &mut value) })?;
        Ok(value)
    }

    /// Returns the ranges of supported sample rates
    pub fn get_tx_sample_rates(&self, channel: usize) -> Result<MetaRange, Error> {
        let mut range = MetaRange::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_tx_rates(self.0, channel as _, range.handle())
        })?;
        Ok(range)
    }

    /// Returns the USRP's current time. Commands can be scheduled relative to this time.
    pub fn get_current_time(&self, mboard: usize) -> Result<TimeSpec, Error> {
        let mut time = TimeSpec::default();
        let mut seconds_time_t: libc::time_t = Default::default();

        check_status(unsafe {
            uhd_sys::uhd_usrp_get_time_now(
                self.0,
                mboard as _,
                &mut seconds_time_t,
                &mut time.fraction,
            )
        })?;
        time.seconds = seconds_time_t;
        Ok(time)
    }

    /// Sets the current clock source
    pub fn set_clock_source(&self, source: &str, mboard: usize) -> Result<(), Error> {
        let source = CString::new(source)?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_clock_source(self.0, source.as_ptr(), mboard as _)
        })
    }

    /// Sets the current time source
    pub fn set_time_source(&self, source: &str, mboard: usize) -> Result<(), Error> {
        let source = CString::new(source)?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_time_source(self.0, source.as_ptr(), mboard as _)
        })
    }

    /// Synchronize the times across all motherboards in this configuration.
    pub fn set_time_unknown_pps(&self, full_secs: i64, frac_secs: f64) -> Result<(), Error> {
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_time_unknown_pps(self.0, full_secs, frac_secs)
        })?;

        Ok(())
    }

    /// Set the time registers on the USRP at the next PPS rising edge.
    pub fn set_time_next_pps(
        &self,
        full_secs: i64,
        frac_secs: f64,
        mboard: usize,
    ) -> Result<(), Error> {
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_time_next_pps(self.0, full_secs, frac_secs, mboard as _)
        })?;

        Ok(())
    }

    /// Checks whether the times across all motherboards in this configuration are synchronized
    pub fn get_time_synchronized(&self) -> Result<bool, Error> {
        let mut result = false;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_time_synchronized(self.0, &mut result as *mut _)
        })?;

        Ok(result)
    }

    /// Enables or disables the receive automatic gain control
    pub fn set_rx_agc_enabled(&mut self, enabled: bool, channel: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_set_rx_agc(self.0, enabled, channel as _) })
    }

    /// Sets the antenna used to receive
    pub fn set_rx_antenna(&mut self, antenna: &str, channel: usize) -> Result<(), Error> {
        let antenna = CString::new(antenna)?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_rx_antenna(self.0, antenna.as_ptr(), channel as _)
        })
    }

    /// Sets the receive bandwidth
    pub fn set_rx_bandwidth(&mut self, bandwidth: f64, channel: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_set_rx_bandwidth(self.0, bandwidth, channel as _) })
    }

    /// Enables or disables DC offset correction
    pub fn set_rx_dc_offset_enabled(&mut self, enabled: bool, channel: usize) -> Result<(), Error> {
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_rx_dc_offset_enabled(self.0, enabled, channel as _)
        })
    }

    /// Sets the receive center frequency
    pub fn set_rx_frequency(
        &mut self,
        request: &TuneRequest,
        channel: usize,
    ) -> Result<TuneResult, Error> {
        let args = CString::new(&*request.args)?;
        let mut request_c = uhd_sys::uhd_tune_request_t {
            target_freq: request.target_frequency,
            rf_freq_policy: request.rf.c_policy(),
            rf_freq: request.rf.frequency(),
            dsp_freq_policy: request.dsp.c_policy(),
            dsp_freq: request.dsp.frequency(),
            // Unsafe cast *const c_char to *mut c_char
            // The C++ code probably won't modify this.
            args: args.as_ptr() as *mut _,
        };

        let mut result = TuneResult::default();
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_rx_freq(self.0, &mut request_c, channel as _, result.inner_mut())
        })?;

        Ok(result)
    }

    /// Sets the receive gain
    pub fn set_rx_gain(&mut self, gain: f64, channel: usize, name: &str) -> Result<(), Error> {
        let name = CString::new(name)?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_rx_gain(self.0, gain, channel as _, name.as_ptr())
        })
    }

    /// Sets the receive sample rate
    pub fn set_rx_sample_rate(&mut self, rate: f64, channel: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_set_rx_rate(self.0, rate, channel as _) })
    }

    /// Sets the antenna used to transmit
    pub fn set_tx_antenna(&mut self, antenna: &str, channel: usize) -> Result<(), Error> {
        let antenna = CString::new(antenna)?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_tx_antenna(self.0, antenna.as_ptr(), channel as _)
        })
    }

    /// Sets the transmit bandwidth
    pub fn set_tx_bandwidth(&mut self, bandwidth: f64, channel: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_set_tx_bandwidth(self.0, bandwidth, channel as _) })
    }

    /// Sets the transmit center frequency
    pub fn set_tx_frequency(
        &mut self,
        request: &TuneRequest,
        channel: usize,
    ) -> Result<TuneResult, Error> {
        let args = CString::new(&*request.args)?;
        let mut request_c = uhd_sys::uhd_tune_request_t {
            target_freq: request.target_frequency,
            rf_freq_policy: request.rf.c_policy(),
            rf_freq: request.rf.frequency(),
            dsp_freq_policy: request.dsp.c_policy(),
            dsp_freq: request.dsp.frequency(),
            // Unsafe cast *const c_char to *mut c_char
            // The C++ code probably won't modify this.
            args: args.as_ptr() as *mut _,
        };

        let mut result = TuneResult::default();
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_tx_freq(self.0, &mut request_c, channel as _, result.inner_mut())
        })?;

        Ok(result)
    }

    /// Sets the transmit gain
    pub fn set_tx_gain(&mut self, gain: f64, channel: usize, name: &str) -> Result<(), Error> {
        let name = CString::new(name)?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_set_tx_gain(self.0, gain, channel as _, name.as_ptr())
        })
    }

    /// Sets the transmit sample rate
    pub fn set_tx_sample_rate(&mut self, rate: f64, channel: usize) -> Result<(), Error> {
        check_status(unsafe { uhd_sys::uhd_usrp_set_tx_rate(self.0, rate, channel as _) })
    }

    /// Returns the available GPIO banks
    pub fn get_gpio_banks(&self, mboard: usize) -> Result<Vec<String>, Error> {
        let mut banks = StringVector::new()?;
        check_status(unsafe {
            uhd_sys::uhd_usrp_get_gpio_banks(self.0, mboard as _, banks.handle_mut())
        })?;
        Ok(banks.into())
    }
}

impl Drop for Usrp {
    fn drop(&mut self) {
        // Ignore error (what errors could really happen that can be handled?)
        let _ = unsafe { uhd_sys::uhd_usrp_free(&mut self.0) };
    }
}

// Thread safety: see https://files.ettus.com/manual/page_general.html#general_threading
// All functions associated with the Usrp struct are thread-safe
unsafe impl Send for Usrp {}
unsafe impl Sync for Usrp {}
