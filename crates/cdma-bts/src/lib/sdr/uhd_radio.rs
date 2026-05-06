use std::sync::Arc;

use cdma_common::error::Error;
use log::{debug, info};
use num_complex::Complex32;
use uhd::{
    ReceiveStreamer, StreamArgs, StreamCommand, StreamCommandType, StreamTime, TimeSpec,
    TransmitMetadata, TransmitStreamer, TuneRequest, Usrp,
};

use super::{Radio, RadioRx, RadioTx, RxReadResult, TX_SAMPLE_RATE, TxPulseShaper};

use cdma_common::consts::SR1_CHIP_RATE_HZ;

/// Default master clock rate: 49.152 MHz = 10 ticks per TX sample, 40 ticks per chip.
const DEFAULT_MASTER_CLOCK_RATE: u64 = 49_152_000;

/// Convert a u64 tick count at `tick_rate` Hz into a UHD `TimeSpec`.
fn ticks_to_timespec(ticks: u64, tick_rate: u64) -> TimeSpec {
    let full_secs = (ticks / tick_rate) as i64;
    let frac_ticks = ticks % tick_rate;
    let frac_secs = frac_ticks as f64 / tick_rate as f64;
    TimeSpec {
        seconds: full_secs,
        fraction: frac_secs,
    }
}

/// Convert a UHD `TimeSpec` back to a u64 tick count at `tick_rate` Hz.
/// Uses the same split-integer-and-fraction algorithm as UHD's `to_ticks()`.
fn timespec_to_ticks(ts: &TimeSpec, tick_rate: u64) -> u64 {
    let rate_i = tick_rate as i64;
    let ticks_full = ts.seconds * rate_i;
    let ticks_frac = (ts.fraction * tick_rate as f64).round() as i64;
    (ticks_full + ticks_frac) as u64
}

pub struct UhdRadio {
    usrp: Usrp,
    channel: usize,
    master_clock_rate: u64,
    tx_shaper: TxPulseShaper,
    tx_lo_offset_hz: f64,
    tx_streamer: Option<TransmitStreamer<Complex32>>,
    rx_streamer: Option<ReceiveStreamer<Complex32>>,
}

impl UhdRadio {
    pub fn new(
        device_args: &str,
        channel: usize,
        tx_antenna: &str,
        tx_gain_db: f64,
        master_clock_rate: Option<u64>,
    ) -> Result<UhdRadio, Error> {
        let mut usrp = Usrp::open(device_args)
            .map_err(|e| Error::from(format!("UHD: failed to open device: {}", e)))?;

        let mcr = master_clock_rate.unwrap_or(DEFAULT_MASTER_CLOCK_RATE);
        info!("UHD: device opened, args={}", device_args);

        // Set master clock rate before anything else.
        usrp.set_master_clock_rate(mcr as f64, 0)
            .map_err(|e| Error::from(format!("UHD: set master clock rate: {}", e)))?;

        let actual_mcr = usrp
            .get_master_clock_rate(0)
            .map_err(|e| Error::from(format!("UHD: get master clock rate: {}", e)))?;
        info!(
            "UHD: master_clock_rate requested={} actual={}",
            mcr, actual_mcr
        );

        // Validate tick alignment.
        let ticks_per_sample = mcr / TX_SAMPLE_RATE as u64;
        let remainder = mcr % TX_SAMPLE_RATE as u64;
        if remainder != 0 {
            log::warn!(
                "UHD: master_clock_rate {} is NOT an integer multiple of TX sample rate {} \
                 (remainder={}). Tick alignment will have sub-tick jitter.",
                mcr,
                TX_SAMPLE_RATE,
                remainder,
            );
        } else {
            info!(
                "UHD: tick alignment: {}/{}={} ticks/sample, {} ticks/chip",
                mcr,
                TX_SAMPLE_RATE,
                ticks_per_sample,
                mcr / SR1_CHIP_RATE_HZ,
            );
        }

        usrp.set_tx_antenna(tx_antenna, channel)
            .map_err(|e| Error::from(format!("UHD: set TX antenna: {}", e)))?;
        usrp.set_tx_gain(tx_gain_db, channel, "")
            .map_err(|e| Error::from(format!("UHD: set TX gain: {}", e)))?;

        let mb_name = usrp.get_motherboard_name(0).unwrap_or_default();
        info!(
            "UHD: motherboard={} channel={} antenna={} gain={}dB",
            mb_name, channel, tx_antenna, tx_gain_db
        );

        // Set TX sample rate and create TX streamer.
        usrp.set_tx_sample_rate(TX_SAMPLE_RATE as f64, channel)
            .map_err(|e| Error::from(format!("UHD: set TX sample rate: {}", e)))?;
        let tx_streamer = usrp
            .get_tx_stream(&StreamArgs::<Complex32>::new("sc16"))
            .map_err(|e| Error::from(format!("UHD: create TX stream: {}", e)))?;

        Ok(UhdRadio {
            usrp,
            channel,
            master_clock_rate: mcr,
            tx_shaper: TxPulseShaper::new(),
            tx_lo_offset_hz: 0.0,
            tx_streamer: Some(tx_streamer),
            rx_streamer: None,
        })
    }

    /// Set clock source (e.g. "internal", "external", "gpsdo").
    pub fn set_clock_source(&self, source: &str) -> Result<(), Error> {
        self.usrp
            .set_clock_source(source, 0)
            .map_err(|e| Error::from(format!("UHD: set clock source: {}", e)))?;
        info!("UHD: clock source set to '{}'", source);
        Ok(())
    }

    /// Set time source (e.g. "internal", "external", "gpsdo").
    pub fn set_time_source(&self, source: &str) -> Result<(), Error> {
        self.usrp
            .set_time_source(source, 0)
            .map_err(|e| Error::from(format!("UHD: time source: {}", e)))?;
        info!("UHD: time source set to '{}'", source);
        Ok(())
    }
}

impl Radio for UhdRadio {
    fn tick_rate(&self) -> u64 {
        self.master_clock_rate
    }

    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error> {
        let request = if self.tx_lo_offset_hz != 0.0 {
            TuneRequest::with_frequency_lo(center_frequency as f64, self.tx_lo_offset_hz)
        } else {
            TuneRequest::with_frequency(center_frequency as f64)
        };
        let result = self
            .usrp
            .set_tx_frequency(&request, self.channel)
            .map_err(|e| Error::from(format!("UHD: set TX freq: {}", e)))?;
        debug!(
            "UHD: TX tune target_rf={} actual_rf={} dsp={}",
            result.target_rf_freq(),
            result.actual_rf_freq(),
            result.actual_dsp_freq()
        );
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<(), Error> {
        self.usrp
            .set_tx_sample_rate(sample_rate as f64, self.channel)
            .map_err(|e| Error::from(format!("UHD: set TX rate: {}", e)))?;
        Ok(())
    }

    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error> {
        self.usrp
            .set_tx_bandwidth(bandwidth as f64, self.channel)
            .map_err(|e| Error::from(format!("UHD: set TX bandwidth: {}", e)))?;
        Ok(())
    }

    fn set_tx_lo_offset_hz(&mut self, offset_hz: i64) -> Result<(), Error> {
        self.tx_lo_offset_hz = offset_hz as f64;
        Ok(())
    }

    fn setup_rx(
        &mut self,
        channel: usize,
        antenna: &str,
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        gain_db: Option<f64>,
    ) -> Result<(), Error> {
        self.usrp
            .set_rx_antenna(antenna, channel)
            .map_err(|e| Error::from(format!("UHD: set RX antenna: {}", e)))?;
        let tune = TuneRequest::with_frequency(frequency_hz);
        self.usrp
            .set_rx_frequency(&tune, channel)
            .map_err(|e| Error::from(format!("UHD: set RX freq: {}", e)))?;
        self.usrp
            .set_rx_sample_rate(sample_rate_hz, channel)
            .map_err(|e| Error::from(format!("UHD: set RX rate: {}", e)))?;
        self.usrp
            .set_rx_bandwidth(bandwidth_hz, channel)
            .map_err(|e| Error::from(format!("UHD: set RX bandwidth: {}", e)))?;
        if let Some(gain) = gain_db {
            self.usrp
                .set_rx_gain(gain, channel, "")
                .map_err(|e| Error::from(format!("UHD: set RX gain: {}", e)))?;
        }
        let rx_streamer = self
            .usrp
            .get_rx_stream(&StreamArgs::<Complex32>::new("sc16"))
            .map_err(|e| Error::from(format!("UHD: create RX stream: {}", e)))?;
        self.rx_streamer = Some(rx_streamer);
        info!(
            "UHD: RX configured antenna={} freq={} rate={} bw={} gain={:?}",
            antenna, frequency_hz, sample_rate_hz, bandwidth_hz, gain_db
        );
        Ok(())
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        let usrp = Arc::new(self.usrp);
        let tx = UhdTxHalf {
            usrp: usrp.clone(),
            tx_streamer: self
                .tx_streamer
                .ok_or_else(|| Error::from("UHD: TX streamer not initialized"))?,
            master_clock_rate: self.master_clock_rate,
            tx_shaper: self.tx_shaper,
            start_of_burst: true,
        };
        let rx = self.rx_streamer.map(|s| -> Box<dyn RadioRx> {
            Box::new(UhdRxHalf {
                usrp: usrp.clone(),
                rx_streamer: s,
                master_clock_rate: self.master_clock_rate,
            })
        });
        Ok((Box::new(tx), rx))
    }
}

// ---------------------------------------------------------------------------
// TX half
// ---------------------------------------------------------------------------

struct UhdTxHalf {
    usrp: Arc<Usrp>,
    tx_streamer: TransmitStreamer<Complex32>,
    master_clock_rate: u64,
    tx_shaper: TxPulseShaper,
    start_of_burst: bool,
}

impl RadioTx for UhdTxHalf {
    fn tick_rate(&self) -> u64 {
        self.master_clock_rate
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        let ts = self
            .usrp
            .get_current_time(0)
            .map_err(|e| Error::from(format!("UHD: get_current_time: {}", e)))?;
        Ok(timespec_to_ticks(&ts, self.master_clock_rate))
    }

    fn set_hardware_time(&self, ticks: u64) -> Result<(), Error> {
        let ts = ticks_to_timespec(ticks, self.master_clock_rate);
        self.usrp
            .set_time_unknown_pps(ts.seconds, ts.fraction)
            .map_err(|e| Error::from(format!("UHD: set_time_unknown_pps: {}", e)))?;
        Ok(())
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.transmit_at(samples, None)
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        let shaped = self.tx_shaper.shape(samples);
        // No software NCO rotation — UHD handles LO offset in hardware
        // via TuneRequest::with_frequency_lo() in set_tx_frequency().

        let metadata = match tick {
            Some(t) => {
                let ts = ticks_to_timespec(t, self.master_clock_rate);
                TransmitMetadata::with_time(ts.seconds, ts.fraction, self.start_of_burst, false)
                    .map_err(|e| Error::from(format!("UHD: TX metadata: {}", e)))?
            }
            None => TransmitMetadata::new()
                .map_err(|e| Error::from(format!("UHD: TX metadata: {}", e)))?,
        };

        self.tx_streamer
            .send_with_metadata(&mut [&shaped], &metadata, 1.0)
            .map_err(|e| Error::from(format!("UHD: TX send: {}", e)))?;
        self.start_of_burst = false;
        Ok(())
    }

    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        self.enable_transmit_at(enable, None)
    }

    fn enable_transmit_at(&mut self, enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        if enable {
            self.start_of_burst = true;
        } else {
            // Send end-of-burst to flush the TX streamer.
            let eob = TransmitMetadata::with_time(0, 0.0, false, true)
                .map_err(|e| Error::from(format!("UHD: TX metadata: {}", e)))?;
            let empty: &[Complex32] = &[];
            let _ = self.tx_streamer.send_with_metadata(&mut [empty], &eob, 0.1);
            self.start_of_burst = false;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// RX half
// ---------------------------------------------------------------------------

struct UhdRxHalf {
    usrp: Arc<Usrp>,
    rx_streamer: ReceiveStreamer<Complex32>,
    master_clock_rate: u64,
}

impl RadioRx for UhdRxHalf {
    fn tick_rate(&self) -> u64 {
        self.master_clock_rate
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        let ts = self
            .usrp
            .get_current_time(0)
            .map_err(|e| Error::from(format!("UHD: get_current_time: {}", e)))?;
        Ok(timespec_to_ticks(&ts, self.master_clock_rate))
    }

    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error> {
        let timeout_s = timeout_us as f64 / 1_000_000.0;
        let md = self
            .rx_streamer
            .receive(&mut [buf], timeout_s, false)
            .map_err(|e| Error::from(format!("UHD: RX recv: {}", e)))?;

        // Check for errors in the metadata.
        if let Some(err) = md
            .last_error()
            .map_err(|e| Error::from(format!("UHD: RX metadata: {}", e)))?
        {
            use uhd::ReceiveErrorKind;
            match err.kind() {
                ReceiveErrorKind::Timeout => {
                    return Ok(RxReadResult {
                        samples_read: 0,
                        time_ticks: 0,
                        overflow: false,
                    });
                }
                ReceiveErrorKind::Overflow => {
                    return Ok(RxReadResult {
                        samples_read: md.samples(),
                        time_ticks: 0,
                        overflow: true,
                    });
                }
                _ => {
                    log::warn!("UHD: RX error: {:?}", err);
                }
            }
        }

        let time_ticks = md
            .time_spec()
            .map_err(|e| Error::from(format!("UHD: RX metadata: {}", e)))?
            .map(|ts| timespec_to_ticks(&ts, self.master_clock_rate))
            .unwrap_or(0);

        Ok(RxReadResult {
            samples_read: md.samples(),
            time_ticks,
            overflow: false,
        })
    }

    fn rx_activate(&mut self, _time_ticks: Option<u64>) -> Result<(), Error> {
        self.rx_streamer
            .send_command(&StreamCommand {
                time: StreamTime::Now,
                command_type: StreamCommandType::StartContinuous,
            })
            .map_err(|e| Error::from(format!("UHD: RX activate: {}", e)))?;
        Ok(())
    }

    fn rx_deactivate(&mut self) -> Result<(), Error> {
        self.rx_streamer
            .send_command(&StreamCommand {
                time: StreamTime::Now,
                command_type: StreamCommandType::StopContinuous,
            })
            .map_err(|e| Error::from(format!("UHD: RX deactivate: {}", e)))?;
        Ok(())
    }
}
