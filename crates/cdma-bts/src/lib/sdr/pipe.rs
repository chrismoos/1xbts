use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Instant;

use cdma_common::{consts::SR1_CHIP_RATE_HZ, error::Error};
use num_complex::Complex32;

use super::{FILE_OUTPUT_TARGET_PEAK, Radio, RadioTx, TX_SAMPLE_RATE};
use crate::bts::rx::{self, InjectedRxBlock, InjectedRxReceiver};

/// A block of final SDR-rate TX output samples.
pub struct TxOutputBlock {
    pub samples: Vec<Complex32>,
    pub tick: Option<u64>,
    pub chip_count: usize,
}

/// In-memory Radio implementation for testing.
/// Makes final SDR-rate TX samples available via RadioPipeHandle.
/// Also carries an InjectedRxReceiver for the BTS to consume.
pub struct RadioPipe {
    tx_output: mpsc::SyncSender<TxOutputBlock>,
    injected_rx: Option<InjectedRxReceiver>,
    tx_sample_rate_hz: Arc<AtomicUsize>,
    clock_start: Instant,
}

/// Handle for test code to read TX output and inject RX samples.
pub struct RadioPipeHandle {
    tx_output_rx: mpsc::Receiver<TxOutputBlock>,
    injected_rx_tx: Option<rx::InjectedRxSender>,
    tx_sample_rate_hz: Arc<AtomicUsize>,
}

impl RadioPipe {
    pub fn new(tx_buffer_depth: usize) -> (RadioPipe, RadioPipeHandle) {
        let (tx_out_tx, tx_out_rx) = mpsc::sync_channel(tx_buffer_depth);
        let (injected_tx, injected_rx) = rx::injected_rx_channel(32);
        let tx_sample_rate_hz = Arc::new(AtomicUsize::new(TX_SAMPLE_RATE));

        let pipe = RadioPipe {
            tx_output: tx_out_tx,
            injected_rx: Some(injected_rx),
            tx_sample_rate_hz: tx_sample_rate_hz.clone(),
            clock_start: Instant::now(),
        };

        let handle = RadioPipeHandle {
            tx_output_rx: tx_out_rx,
            injected_rx_tx: Some(injected_tx),
            tx_sample_rate_hz,
        };

        (pipe, handle)
    }

    /// Take the InjectedRxReceiver out of this pipe.
    /// Called by Bts::new_with_radio_pipe to extract it.
    pub fn take_injected_rx(&mut self) -> Option<InjectedRxReceiver> {
        self.injected_rx.take()
    }
}

impl Radio for RadioPipe {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn set_tx_frequency(&mut self, _: usize) -> Result<(), Error> {
        Ok(())
    }
    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<(), Error> {
        self.tx_sample_rate_hz.store(sample_rate, Ordering::Relaxed);
        Ok(())
    }
    fn set_tx_bandwidth(&mut self, _: usize) -> Result<(), Error> {
        Ok(())
    }

    fn split(
        self: Box<Self>,
    ) -> Result<(Box<dyn RadioTx>, Option<Box<dyn super::RadioRx>>), Error> {
        let tx = PipeTxHalf {
            tx_output: self.tx_output,
            tx_sample_rate_hz: self.tx_sample_rate_hz,
            clock_start: self.clock_start,
        };
        Ok((Box::new(tx), None))
    }
}

struct PipeTxHalf {
    tx_output: mpsc::SyncSender<TxOutputBlock>,
    tx_sample_rate_hz: Arc<AtomicUsize>,
    clock_start: Instant,
}

impl RadioTx for PipeTxHalf {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock_start.elapsed().as_nanos() as u64)
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.transmit_at(samples, None)
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        let sample_rate_hz = self.tx_sample_rate_hz.load(Ordering::Relaxed);
        let chip_rate_hz = SR1_CHIP_RATE_HZ as usize;
        let oversample = if sample_rate_hz >= chip_rate_hz && sample_rate_hz % chip_rate_hz == 0 {
            sample_rate_hz / chip_rate_hz
        } else {
            0
        };
        let _ = self.tx_output.try_send(TxOutputBlock {
            samples: samples.to_vec(),
            tick,
            chip_count: if oversample > 0 {
                samples.len() / oversample
            } else {
                0
            },
        });
        Ok(())
    }

    fn enable_transmit(&mut self, _: bool) -> Result<(), Error> {
        Ok(())
    }
}

impl RadioPipeHandle {
    /// Read the next final SDR-rate TX output block. Blocks until available.
    pub fn recv_tx(&self) -> Option<TxOutputBlock> {
        self.tx_output_rx.recv().ok()
    }

    /// Try to read TX output without blocking.
    pub fn try_recv_tx(&self) -> Option<TxOutputBlock> {
        self.tx_output_rx.try_recv().ok()
    }

    /// Drain all available TX output blocks into a single sample vector.
    pub fn drain_tx_samples(&self) -> Vec<Complex32> {
        let mut all = Vec::new();
        while let Ok(block) = self.tx_output_rx.try_recv() {
            all.extend(block.samples);
        }
        all
    }

    /// Inject pulse-shaped RX samples into the BTS receiver pipeline.
    pub fn inject_rx(&self, block: InjectedRxBlock) -> Result<(), Error> {
        self.injected_rx_tx
            .as_ref()
            .ok_or_else(|| Error::from("RadioPipe RX channel already closed"))?
            .send(block)
            .map_err(|_| "RadioPipe RX channel disconnected".into())
    }

    /// Close the RX injection channel, signaling end-of-stream to the BTS RX
    /// path. The handle remains usable for reading TX output and dumping WAV.
    pub fn close_rx(&mut self) {
        self.injected_rx_tx.take();
    }

    /// Write drained TX samples to a WAV file for debugging.
    pub fn dump_tx_to_wav<W: std::io::Write + std::io::Seek>(
        &self,
        writer: W,
    ) -> Result<(), Error> {
        let samples = self.drain_tx_samples();
        let sample_rate_hz = self.tx_sample_rate_hz.load(Ordering::Relaxed);
        let mut wav = hound::WavWriter::new(
            writer,
            hound::WavSpec {
                channels: 2,
                sample_rate: sample_rate_hz as u32,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        for s in &samples {
            let re = s.re * FILE_OUTPUT_TARGET_PEAK;
            let im = s.im * FILE_OUTPUT_TARGET_PEAK;
            wav.write_sample((re * (i16::MAX as f32)) as i16)?;
            wav.write_sample((im * (i16::MAX as f32)) as i16)?;
        }
        wav.finalize()?;
        Ok(())
    }
}
