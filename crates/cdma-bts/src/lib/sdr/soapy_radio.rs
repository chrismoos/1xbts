use cdma_common::error::Error;
use log::{debug, info, warn};
use num_complex::Complex32;
use soapysdr::{Direction, RxStream, TxStream};

use super::{Radio, RadioRx, RadioTx, RxReadResult, TX_SAMPLE_RATE, TxRadioHealth};

/// `SOAPY_SDR_END_BURST`, which the `soapysdr` crate does not re-export.
const SOAPY_END_BURST: u32 = 2;

const MAX_TX_STATUS_EVENTS_PER_POLL: usize = 256;
const TX_WRITE_TIMEOUT_US: i64 = 1_000_000;

pub struct SoapySdrRadio {
    device: soapysdr::Device,
    channel: usize,
    rx_channel: usize,
    stream: TxStream<num_complex::Complex<f32>>,
    tx_lo_offset_hz: f64,
    tx_sample_rate_hz: f64,
    tx_nco_phase_rad: f64,
    rx_stream: Option<RxStream<Complex32>>,
}

impl SoapySdrRadio {
    pub fn new(
        device_str: &str,
        channel: usize,
        antenna: &str,
        tx_gain_db: f64,
    ) -> Result<SoapySdrRadio, Error> {
        let device = soapysdr::Device::new(device_str)?;
        for antenna in device.antennas(soapysdr::Direction::Tx, channel)? {
            debug!("Antenna: {}", antenna);
        }
        for format in device.stream_formats(soapysdr::Direction::Tx, channel)? {
            debug!("SDR format: {:?}", format);
        }

        device.set_gain(soapysdr::Direction::Tx, channel, tx_gain_db)?;
        device.set_antenna(soapysdr::Direction::Tx, channel, antenna)?;

        let stream = device.tx_stream::<_>(&[0])?;

        Ok(SoapySdrRadio {
            device,
            channel,
            rx_channel: channel,
            stream,
            tx_lo_offset_hz: 0.0,
            tx_sample_rate_hz: TX_SAMPLE_RATE as f64,
            tx_nco_phase_rad: 0.0,
            rx_stream: None,
        })
    }
}

impl SoapySdrRadio {
    pub fn setup_rx_stream(
        &self,
        channel: usize,
        antenna: &str,
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        gain_db: Option<f64>,
    ) -> Result<RxStream<Complex32>, Error> {
        debug!(
            "setup_rx: antenna={} freq={} rate={} bw={}",
            antenna, frequency_hz, sample_rate_hz, bandwidth_hz
        );
        self.device.set_antenna(Direction::Rx, channel, antenna)?;
        self.device
            .set_frequency(Direction::Rx, channel, frequency_hz, "")?;
        self.device
            .set_sample_rate(Direction::Rx, channel, sample_rate_hz)?;
        self.device
            .set_bandwidth(Direction::Rx, channel, bandwidth_hz)?;
        if let Some(gain) = gain_db {
            self.device.set_gain(Direction::Rx, channel, gain)?;
        }
        let actual_freq = self.device.frequency(Direction::Rx, channel)?;
        let actual_rate = self.device.sample_rate(Direction::Rx, channel)?;
        let actual_bw = self.device.bandwidth(Direction::Rx, channel)?;
        let actual_gain = self.device.gain(Direction::Rx, channel)?;
        let actual_ant = self.device.antenna(Direction::Rx, channel)?;
        info!(
            "setup_rx: actual antenna={} freq={} rate={} bw={} gain={}",
            actual_ant, actual_freq, actual_rate, actual_bw, actual_gain
        );
        Ok(self.device.rx_stream::<Complex32>(&[channel])?)
    }
}

impl Radio for SoapySdrRadio {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error> {
        self.device.set_frequency(
            soapysdr::Direction::Tx,
            self.channel,
            center_frequency as f64 + self.tx_lo_offset_hz,
            "",
        )?;
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<usize, Error> {
        self.device
            .set_sample_rate(soapysdr::Direction::Tx, self.channel, sample_rate as f64)?;
        self.tx_sample_rate_hz = self
            .device
            .sample_rate(soapysdr::Direction::Tx, self.channel)?;
        self.tx_nco_phase_rad = 0.0;
        Ok(self.tx_sample_rate_hz.round() as usize)
    }

    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error> {
        self.device
            .set_bandwidth(soapysdr::Direction::Tx, self.channel, bandwidth as f64)?;
        Ok(())
    }

    fn set_tx_lo_offset_hz(&mut self, offset_hz: i64) -> Result<(), Error> {
        self.tx_lo_offset_hz = offset_hz as f64;
        self.tx_nco_phase_rad = 0.0;
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
        self.rx_channel = channel;
        let stream = self.setup_rx_stream(
            channel,
            antenna,
            frequency_hz,
            sample_rate_hz,
            bandwidth_hz,
            gain_db,
        )?;
        self.rx_stream = Some(stream);
        Ok(())
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        let device = std::sync::Arc::new(self.device);
        let tx = SoapyTxHalf {
            device: device.clone(),
            stream: self.stream,
            tx_lo_offset_hz: self.tx_lo_offset_hz,
            tx_sample_rate_hz: self.tx_sample_rate_hz,
            tx_nco_phase_rad: self.tx_nco_phase_rad,
            channel: self.channel,
            health: TxRadioHealth::default(),
            burst_pending: false,
        };
        let rx_channel = self.rx_channel;
        let rx = self.rx_stream.map(|s| -> Box<dyn RadioRx> {
            Box::new(SoapyRxHalf {
                device: device.clone(),
                stream: s,
                channel: rx_channel,
            })
        });
        Ok((Box::new(tx), rx))
    }
}

pub struct SoapyTxHalf {
    device: std::sync::Arc<soapysdr::Device>,
    stream: TxStream<Complex32>,
    tx_lo_offset_hz: f64,
    tx_sample_rate_hz: f64,
    tx_nco_phase_rad: f64,
    channel: usize,
    health: TxRadioHealth,
    burst_pending: bool,
}

impl SoapyTxHalf {
    fn finish_transmit(&mut self, tick: Option<u64>) -> Result<(), Error> {
        let end_result = if std::mem::take(&mut self.burst_pending) {
            // Some drivers ignore empty END_BURST writes.
            let silence = [Complex32::new(0.0, 0.0)];
            self.stream
                .write_all(&[&silence], None, true, TX_WRITE_TIMEOUT_US)
        } else {
            Ok(())
        };
        let deactivate_result = self.stream.deactivate(tick.map(|t| t as i64));
        deactivate_result?;
        end_result?;
        Ok(())
    }

    // Successful writes do not guarantee playback. Late drops arrive through asynchronous status.
    fn drain_tx_status(&mut self) {
        for _ in 0..MAX_TX_STATUS_EVENTS_PER_POLL {
            let mut chan_mask = 0usize;
            let mut flags = 0i32;
            let mut time_ns = 0i64;
            match self
                .stream
                .read_status(&mut chan_mask, &mut flags, &mut time_ns, 0)
            {
                Ok(_) => {
                    if flags & (SOAPY_END_BURST as i32) != 0 {
                        self.health.burst_acks += 1;
                    }
                }
                Err(err) => match err.code {
                    soapysdr::ErrorCode::Timeout => return,
                    soapysdr::ErrorCode::TimeError => {
                        self.health.late_packets += 1;
                        warn!(
                            "soapy tx: device discarded a burst scheduled for {time_ns} ns, \
                             its tick had already passed"
                        );
                    }
                    soapysdr::ErrorCode::Underflow => self.health.underflows += 1,
                    soapysdr::ErrorCode::Corruption => self.health.sequence_errors += 1,
                    soapysdr::ErrorCode::Overflow => self.health.dropped_packets += 1,
                    _ => self.health.unknown_events += 1,
                },
            }
        }
    }

    fn apply_tx_lo_offset(&mut self, samples: &mut [Complex32]) {
        if self.tx_lo_offset_hz == 0.0 || self.tx_sample_rate_hz <= 0.0 {
            return;
        }

        let phase_step =
            -2.0 * std::f64::consts::PI * self.tx_lo_offset_hz / self.tx_sample_rate_hz;
        let mut phase = self.tx_nco_phase_rad;

        for sample in samples.iter_mut() {
            let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
            *sample *= rot;
            phase += phase_step;
        }

        self.tx_nco_phase_rad = phase.rem_euclid(2.0 * std::f64::consts::PI);
    }
}

impl RadioTx for SoapyTxHalf {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.device.get_hardware_time(None)? as u64)
    }

    fn set_hardware_time(&self, ticks: u64) -> Result<(), Error> {
        self.device.set_hardware_time(None, ticks as i64)?;
        Ok(())
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.transmit_at(samples, None)
    }

    fn tx_health(&mut self) -> Result<TxRadioHealth, Error> {
        self.drain_tx_status();
        Ok(self.health)
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        let mut samples = samples.to_vec();
        self.apply_tx_lo_offset(&mut samples);
        self.burst_pending |= !samples.is_empty();
        self.stream.write_all(
            &[&samples],
            tick.map(|t| t as i64),
            false,
            TX_WRITE_TIMEOUT_US,
        )?;
        Ok(())
    }

    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        if enable {
            self.tx_nco_phase_rad = 0.0;
            self.stream.activate(None)?;
        } else {
            self.finish_transmit(None)?;
        }
        Ok(())
    }

    fn enable_transmit_at(&mut self, enable: bool, tick: Option<u64>) -> Result<(), Error> {
        if enable {
            self.tx_nco_phase_rad = 0.0;
            self.stream.activate(tick.map(|t| t as i64))?;
        } else {
            self.finish_transmit(tick)?;
        }
        Ok(())
    }

    fn set_tx_frequency_hz(&mut self, frequency_hz: f64) -> Result<(), Error> {
        self.device.set_frequency(
            soapysdr::Direction::Tx,
            self.channel,
            frequency_hz + self.tx_lo_offset_hz,
            "",
        )?;
        Ok(())
    }
}

pub struct SoapyRxHalf {
    device: std::sync::Arc<soapysdr::Device>,
    stream: RxStream<Complex32>,
    channel: usize,
}

impl RadioRx for SoapyRxHalf {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.device.get_hardware_time(None)? as u64)
    }

    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error> {
        match self.stream.read(&mut [buf], timeout_us) {
            Ok(n) => {
                let time_ticks = self.stream.time_ns() as u64;
                Ok(RxReadResult {
                    samples_read: n,
                    time_ticks,
                    overflow: false,
                })
            }
            Err(e) if e.code == soapysdr::ErrorCode::Timeout => Ok(RxReadResult {
                samples_read: 0,
                time_ticks: 0,
                overflow: false,
            }),
            Err(e) if e.code == soapysdr::ErrorCode::Overflow => Ok(RxReadResult {
                samples_read: 0,
                time_ticks: 0,
                overflow: true,
            }),
            Err(e) => Err(e.into()),
        }
    }

    fn rx_activate(&mut self, time_ticks: Option<u64>) -> Result<(), Error> {
        self.stream.activate(time_ticks.map(|t| t as i64))?;
        Ok(())
    }

    fn rx_deactivate(&mut self) -> Result<(), Error> {
        self.stream.deactivate(None)?;
        Ok(())
    }

    fn set_rx_frequency(&mut self, frequency_hz: f64) -> Result<(), Error> {
        self.device
            .set_frequency(Direction::Rx, self.channel, frequency_hz, "")?;
        Ok(())
    }

    fn set_rx_gain(&mut self, gain_db: f64) -> Result<(), Error> {
        self.device.set_gain(Direction::Rx, self.channel, gain_db)?;
        Ok(())
    }
}
