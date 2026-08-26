use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use num_complex::Complex32;

/// Monotonic host-time anchor for an absolute RX sample position.
#[derive(Clone, Copy, Debug)]
pub struct RxSampleTimeAnchor {
    pub absolute_sample_end: u64,
    pub received_at: Instant,
}

impl RxSampleTimeAnchor {
    /// Estimate when an earlier sample reached the host from this block-end anchor.
    pub fn received_at_sample(self, absolute_sample: u64, sample_rate_hz: f64) -> Option<Instant> {
        if absolute_sample > self.absolute_sample_end
            || !sample_rate_hz.is_finite()
            || sample_rate_hz <= 0.0
        {
            return None;
        }
        let age = Duration::from_secs_f64(
            self.absolute_sample_end.saturating_sub(absolute_sample) as f64 / sample_rate_hz,
        );
        self.received_at.checked_sub(age)
    }
}

/// A contiguous block of samples with shared block-level metadata.
///
/// Tags are per-block (e.g. acquisition state), not per-sample.
/// `chip_start` is the chip index of `samples[0]`.
#[derive(Clone, Debug)]
pub struct SampleBlock {
    pub samples: Vec<Complex32>,
    pub chip_start: usize,
    /// Effective sample rate for this block's `samples`.
    pub sample_rate_hz: f64,
    /// Absolute RX sample/host-time mapping for latency diagnostics.
    pub rx_sample_time: Option<RxSampleTimeAnchor>,
    pub tags: HashMap<&'static str, i64>,
    /// Per-Power-Control-Group signal metric, in dB. Reverse traffic decoders
    /// populate this on decoded 20 ms frames with 16 entries (one per PCG,
    /// traffic Eb/Nt for active PCGs) and on `traffic_pcg_measurement` event
    /// blocks with one entry for the measured PCG. For RC3 per-PCG
    /// measurements the metric is pilot symbol SINR. Unbiased pilot Ec/Io is
    /// carried separately as `traffic_pcg_pilot_ec_io_true_mdb`.
    /// Consumed by the BSC closed-loop power control path. `None` for blocks
    /// that don't carry traffic measurements.
    pub pcg_signal_snr_db: Option<Vec<f32>>,
    /// Exact or best-available reverse traffic active-PCG mask for a decoded
    /// 20 ms frame. `true` means the mobile transmitted in that PCG for the
    /// decoded frame rate. `None` for blocks that don't carry frame-level
    /// traffic measurements.
    pub active_pcg_mask: Option<[bool; 16]>,
    /// Per-PCG pilot metrics from the RC3 despreader for accurate Eb/Nt
    /// estimation in pilot-coherent mode.  Each entry is
    /// `(pilot_norm_sq, pilot_sym_power_sum, traffic_power_sum, chip_power_sum)`:
    ///   - `pilot_norm_sq` = |Σ pilot_k|² (coherent sum power over the PCG)
    ///   - `pilot_sym_power_sum` = Σ |pilot_k|² (incoherent sum of per-symbol powers)
    ///   - `traffic_power_sum` = Σ |traffic_k|² (raw Walsh-4 symbol power, pre-cross-product)
    ///   - `chip_power_sum` = Σ |chip|² (total wideband chip power for Io estimate)
    /// From the pilot pair the frame aligner derives per-symbol noise variance:
    ///   noise_var = pilot_sym_power_sum/N - pilot_norm_sq/N²
    /// which is uncontaminated by the traffic signal (Walsh orthogonality).
    /// The raw traffic power, combined with the noise estimate, gives the
    /// traffic signal energy without cross-product contamination.
    /// The chip_power_sum enables per-PCG Ec/Io computation for inner-loop
    /// power control: Ec/Io = pilot_norm_sq / (N² × 16 × chip_power_sum / N_chips).
    pub pcg_pilot_metrics: Option<Vec<(f32, f32, f32, f32)>>,
}

impl SampleBlock {
    pub fn new(samples: Vec<Complex32>, chip_start: usize) -> Self {
        Self {
            samples,
            chip_start,
            sample_rate_hz: 0.0,
            rx_sample_time: None,
            tags: HashMap::new(),
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            pcg_pilot_metrics: None,
        }
    }

    pub fn with_sample_rate_hz(mut self, sample_rate_hz: f64) -> Self {
        self.sample_rate_hz = sample_rate_hz;
        self
    }

    pub fn with_tags(mut self, tags: HashMap<&'static str, i64>) -> Self {
        self.tags = tags;
        self
    }

    /// Returns true only when both samples and tags are empty. A block with
    /// tags but no samples (an event block) is considered non-empty.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty() && self.tags.is_empty()
    }

    /// Returns the number of samples. Note: `len() == 0` does not imply
    /// `is_empty()` since tags may be present.
    pub fn len(&self) -> usize {
        self.samples.len()
    }
}
