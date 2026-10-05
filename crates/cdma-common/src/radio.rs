use num_complex::Complex32;

use crate::error::Error;

pub struct RxReadResult {
    pub samples_read: usize,
    pub time_ticks: u64,
    pub overflow: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TxRadioHealth {
    pub underflows: u64,
    pub late_packets: u64,
    pub sequence_errors: u64,
    pub burst_acks: u64,
    pub dropped_packets: u64,
    pub unknown_events: u64,
}

pub trait Radio: Send {
    fn tick_rate(&self) -> u64;
    /// Signed TX timing correction in four-samples-per-chip units.
    fn tx_sample_delay(&self) -> Option<i64> {
        None
    }
    fn tx_full_scale_power_estimate_dbm(&self) -> Option<f32> {
        None
    }
    fn rx_reference_dbm(&self) -> Option<f32> {
        None
    }
    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error>;
    /// Returns the rate the hardware actually applied.
    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<usize, Error>;
    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error>;
    fn set_tx_lo_offset_hz(&mut self, _offset_hz: i64) -> Result<(), Error> {
        Ok(())
    }
    fn setup_rx(
        &mut self,
        _channel: usize,
        _antenna: &str,
        _frequency_hz: f64,
        _sample_rate_hz: f64,
        _bandwidth_hz: f64,
        _gain_db: Option<f64>,
    ) -> Result<(), Error> {
        Err("RX not supported by this radio".into())
    }
    /// Consume this radio and split into TX and RX halves.
    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error>;
}

/// Samples are at the final radio rate. Preserve their count and timestamp the first sample.
pub trait RadioTx: Send {
    fn tick_rate(&self) -> u64;
    fn get_hardware_time(&self) -> Result<u64, Error>;
    fn set_hardware_time(&self, _ticks: u64) -> Result<(), Error> {
        Ok(())
    }
    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error>;
    fn prepare_transmit(&mut self, _max_samples: usize) -> Result<(), Error> {
        Ok(())
    }
    fn transmit_at(&mut self, samples: &[Complex32], _tick: Option<u64>) -> Result<(), Error> {
        self.transmit(samples)
    }
    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error>;
    fn enable_transmit_at(&mut self, enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        self.enable_transmit(enable)
    }
    fn tx_health(&mut self) -> Result<TxRadioHealth, Error> {
        Ok(TxRadioHealth::default())
    }
    fn set_tx_frequency_hz(&mut self, _frequency_hz: f64) -> Result<(), Error> {
        Err("TX retune not supported by this radio".into())
    }
    /// Transmit already pulse-shaped samples at the final radio rate.
    fn transmit_shaped_at(
        &mut self,
        samples: &[Complex32],
        tick: Option<u64>,
    ) -> Result<(), Error> {
        self.transmit_at(samples, tick)
    }
}

pub trait RadioRx: Send {
    fn tick_rate(&self) -> u64;
    fn get_hardware_time(&self) -> Result<u64, Error>;
    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error>;
    fn rx_activate(&mut self, time_ticks: Option<u64>) -> Result<(), Error>;
    fn rx_deactivate(&mut self) -> Result<(), Error>;
    fn set_rx_frequency(&mut self, _frequency_hz: f64) -> Result<(), Error> {
        Err("RX retune not supported by this radio".into())
    }
    fn set_rx_gain(&mut self, _gain_db: f64) -> Result<(), Error> {
        Err("RX gain control not supported by this radio".into())
    }
}

impl RadioTx for Box<dyn RadioTx> {
    fn tick_rate(&self) -> u64 {
        (**self).tick_rate()
    }
    fn get_hardware_time(&self) -> Result<u64, Error> {
        (**self).get_hardware_time()
    }
    fn set_hardware_time(&self, ticks: u64) -> Result<(), Error> {
        (**self).set_hardware_time(ticks)
    }
    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        (**self).transmit(samples)
    }
    fn prepare_transmit(&mut self, max_samples: usize) -> Result<(), Error> {
        (**self).prepare_transmit(max_samples)
    }
    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        (**self).transmit_at(samples, tick)
    }
    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        (**self).enable_transmit(enable)
    }
    fn enable_transmit_at(&mut self, enable: bool, tick: Option<u64>) -> Result<(), Error> {
        (**self).enable_transmit_at(enable, tick)
    }
    fn tx_health(&mut self) -> Result<TxRadioHealth, Error> {
        (**self).tx_health()
    }
    fn set_tx_frequency_hz(&mut self, frequency_hz: f64) -> Result<(), Error> {
        (**self).set_tx_frequency_hz(frequency_hz)
    }
    fn transmit_shaped_at(
        &mut self,
        samples: &[Complex32],
        tick: Option<u64>,
    ) -> Result<(), Error> {
        (**self).transmit_shaped_at(samples, tick)
    }
}
