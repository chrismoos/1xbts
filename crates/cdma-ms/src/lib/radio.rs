use cdma_common::error::Error;
use num_complex::Complex32;

use crate::engine::ReverseBurst;

pub trait Radio: Send {
    fn sample_rate_hz(&self) -> f64;

    /// Tune the forward-link receiver to `frequency_hz`. Returns once the
    /// radio is on the new frequency and its stream has settled.
    fn tune_forward(&mut self, frequency_hz: f64) -> Result<(), Error>;

    fn tune_reverse(&mut self, _frequency_hz: f64) -> Result<(), Error> {
        Ok(())
    }

    /// Pull the next forward-link (BTS→MS) samples into `out`. May block briefly
    /// and may return `out` empty so the driver can service commands.
    fn read_forward(&mut self, out: &mut Vec<Complex32>);

    /// The last read begins after samples were lost without replacement.
    fn forward_discontinuity(&self) -> bool {
        false
    }

    fn forward_exhausted(&self) -> bool {
        false
    }

    fn rx_overflows(&self) -> u64 {
        0
    }

    /// Input power corresponding to 0 dBFS for the active radio configuration.
    fn rx_reference_dbm(&self) -> Option<f32> {
        None
    }

    /// Forward samples buffered but not yet handed to the engine. The engine
    /// is this far behind the hardware clock, so reverse bursts are scheduled
    /// past it. Zero for radios with no buffering (the loopback and captures).
    fn forward_backlog_samples(&self) -> u64 {
        0
    }

    fn set_rx_gain(&mut self, _gain_db: f64) -> Result<(), Error> {
        Err("RX gain control not supported by this radio".into())
    }

    fn write_reverse(&mut self, burst: &ReverseBurst);

    fn set_tx_calibration(&mut self, _trim: &TxTrim) -> Result<(), Error> {
        Err("TX calibration not supported by this radio".into())
    }

    fn tx_calibration(&self) -> Option<TxCalibration> {
        None
    }

    fn start(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn stop(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TxCalibration {
    /// Mean output power, in dBm, of a burst whose samples have unit RMS.
    /// Only meaningful when `power_control` is set.
    pub tx_reference_dbm: f32,
    /// Approximate RF connector power for unit-RMS samples, for reporting only.
    pub estimated_full_scale_dbm: Option<f32>,
    /// Samples added to the receive-timeline position of every burst, to
    /// cancel the radio's own receive-to-transmit delay.
    pub tx_delay_samples: i64,
    pub power_control: bool,
    /// Maximum complex sample magnitude sent to the SDR.
    pub peak_limit: f32,
    pub relative_access_power: bool,
    pub access_initial_backoff_db: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TxTrim {
    pub tx_reference_dbm: Option<f32>,
    pub tx_delay_samples: Option<i64>,
    pub power_control: Option<bool>,
}
