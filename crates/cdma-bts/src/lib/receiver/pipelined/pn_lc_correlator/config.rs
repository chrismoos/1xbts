use super::super::gardner_timing_recovery::GardnerTimingConfig;

/// Tuning parameters for [`PnLcCorrelator`].
#[derive(Debug, Clone)]
pub struct PnLcConfig {
    pub oversample: usize,

    /// Coherent integration window in chips (= FFT size / oversample).
    pub coherent_chips: usize,

    /// One-sided LC phase search range in chips.
    /// Total sweep = 2 × lc_half_span + 1 hypotheses.
    pub lc_half_span: i32,

    /// SNR pre-filter (best_power / avg_power).  Candidates below this
    /// threshold are rejected before the more expensive coherence check.
    pub snr_threshold: f32,

    /// Minimum ratio of best LC phase power to second-best.
    /// Discriminates the true LC phase from noise/sidelobes.
    pub lc_best_over_second_min: f32,

    /// Minimum coherent magnitude after PN despread + LC removal.
    pub preamble_coh_norm_min: f32,

    /// Consecutive stage-2 hits required before a finger is spawned.
    pub preamble_hits_required: u32,

    /// Number of preamble Walsh symbols to replay ahead of the first verified
    /// PN+LC hit when seeding a new finger.
    pub replay_preamble_symbols: usize,

    /// Number of sub-windows for noncoherent FFT accumulation.
    ///
    /// The coherent window is split into this many equal segments.  Each
    /// segment is FFT-correlated independently and the per-delay powers are
    /// summed noncoherently.  This makes the search tolerant to CFO: phase
    /// rotation within each short segment is small, while the noncoherent
    /// sum preserves the full integration gain.
    ///
    /// Set to 1 to disable (fully coherent, fastest but CFO-sensitive).
    /// Default is 1; increase to 2 or 4 for CFO-tolerant acquisition at the
    /// cost of reduced SNR (divided by n_seg).
    pub noncoherent_segments: usize,

    /// Minimum sample separation between two distinct detections.
    /// Should be ≥ 1 chip × oversample to avoid duplicate fingers for the
    /// same path.
    pub peak_suppress_samples: i32,

    /// Combined TX + RX matched-filter group delay in samples.
    /// Used to convert sample positions to absolute TX chip indices.
    pub composite_filter_delay: usize,

    /// Optional override for the chip-center sample phase within each
    /// oversample period. When `None`, uses `composite_filter_delay % oversample`.
    pub center_offset_override: Option<usize>,

    /// Chips per output block pushed to the sub-chain (one Walsh symbol).
    pub chip_block_size: usize,

    /// Run the joint search once per this many input windows.
    pub search_interval_windows: u64,

    /// When true, use separate PN references for coarse FFT search and finger
    /// despread (`fft` for coarse, OQPSK for despread). When false, use the
    /// coarse FFT PN reference for both stages.
    pub split_pn_reference: bool,

    /// Re-anchor the absolute sample origin on every block using the
    /// hardware-timestamp-derived `absolute_sample_start` tag.
    ///
    /// When `true`, the correlator corrects for SDR sample drops (overflow)
    /// that cause the internal sample counter to drift behind the real
    /// hardware clock.  Large gaps also flush stale buffer data.
    ///
    /// When `false` (default), the origin is latched once from the first
    /// block and never updated — the old behaviour.
    pub reanchor_origin: bool,

    /// Long-code decimation factor for HPSK (complex spreading) modes.
    ///
    /// IS-2000 RC3+ reverse link uses HPSK where the long code is decimated:
    /// h_I(n) = (-1)^LC[2n] (even chips), h_Q(n) = (-1)^LC[2n+1] (odd chips).
    /// Set to 2 for RC3+ HPSK; leave at 1 for IS-95/RC1/RC2.
    pub lc_decimation: usize,

    /// When true, skip the expensive FFT search once a hard-validated finger
    /// exists. The search resumes automatically if the finger is lost.
    /// Use this for traffic channels where only one signal is expected.
    /// Leave false for access channels that must detect new probes continuously.
    pub suppress_search_when_locked: bool,

    /// When true, do not spawn a new finger if an existing active finger's
    /// acquisition delay is within `active_finger_delay_suppress_samples`.
    ///
    /// This is intentionally off by default because traffic channels can use
    /// timing diversity differently from bursty access channels.
    pub suppress_active_finger_delay_overlap: bool,

    /// One-sided sample threshold for active-finger delay overlap suppression.
    pub active_finger_delay_suppress_samples: i32,

    /// Enable instrumentation-only early/prompt/late chip-timing measurement
    /// on every finger spawned by this correlator, once each finger crosses
    /// hard-validation. Pure measurement: never moves `despread_phase`, never
    /// affects the sub-chain output, never feeds back into anything. Just
    /// accumulates per-tap energy and prints periodic stats.
    ///
    /// Currently used by the RC1 reverse traffic builder. Leave false for
    /// access channel and RC3 traffic.
    pub enable_epl_tracking: bool,

    /// Enable ACTIVE sub-chip timing correction using the EPL
    /// discriminator. Requires `enable_epl_tracking == true`.
    ///
    /// When set, the 4-chip coherent (PN+LC aware) discriminator
    /// `(E-L)/P` is smoothed via an IIR filter, integrated into a
    /// fractional sub-chip accumulator, and whenever the accumulator
    /// crosses ±1 the finger slews its `despread_phase` and
    /// `next_prompt_offset` by one sub-sample (bidirectional). A
    /// dead-zone suppresses noise-driven slews and a minimum spacing
    /// between slews prevents runaway. Warmup period delays the first
    /// slew until enough windows have accumulated.
    pub enable_epl_slew: bool,

    /// Use pilot-coherent 16-chip Walsh 0 accumulation for the EPL
    /// discriminator instead of the generic 4-chip coherent metric.
    /// Designed for RC3+ reverse traffic where Walsh 0 (all +1, 16 chips)
    /// is an always-on ungated pilot.
    pub epl_pilot: bool,

    /// Enable the access-channel CFO tracker: locks during preamble
    /// (Walsh 0 coherent), coasts during data (noncoherent).
    /// Set true for reverse access fingers; false for traffic.
    pub access_cfo: bool,

    /// Refine the integer FFT delay to a fractional sample position during
    /// candidate verification, then use interpolation in each spawned finger.
    ///
    /// This is open-loop timing recovery: acquisition chooses a fixed
    /// fractional prompt (`timing_mu_samples`) from PN+LC preamble coherence,
    /// and the finger keeps that fractional offset for the burst.  Closed-loop
    /// EPL slewing can still make later integer sub-sample nudges if enabled.
    pub fractional_timing_recovery: bool,

    /// One-sided fractional timing search span in samples around the integer
    /// FFT delay.  At 4x oversample, 0.5 sample = 1/8 chip.
    pub fractional_timing_half_samples: f32,

    /// Fractional timing search step in samples.
    pub fractional_timing_step_samples: f32,

    /// One-sided timing diversity span in samples around the acquisition-chosen
    /// prompt when spawning a finger. 0.0 preserves the single-finger behavior.
    pub finger_timing_half_samples: f32,

    /// Timing diversity spacing in samples for adjacent spawned fingers.
    pub finger_timing_step_samples: f32,

    /// When true, spawn both early and late timing hypotheses around the chosen
    /// prompt. When false, spawn one adjacent hypothesis chosen by the adaptive
    /// reverse-access SNR rule.
    pub finger_timing_symmetric: bool,

    /// Emit every PN×LC-despread sub-sample in each chip interval instead of
    /// collapsing immediately to one prompt sample per chip.
    ///
    /// Reverse-access downstream stages already understand `access_oversample`
    /// and can choose the strongest Walsh phase per symbol.  Keeping the
    /// polyphase samples here avoids making a hard timing decision in the
    /// finger and gives the Walsh demodulator the timing diversity.
    pub output_oversampled_chips: bool,

    /// Despread every oversampled sample in the chip interval and sum the
    /// results before applying the long-code sign.
    ///
    /// This recovers the 4× pulse energy that a single prompt sample discards,
    /// while still emitting one chip-rate value downstream.
    pub integrate_and_dump: bool,

    /// Optional closed-loop Gardner timing recovery for each spawned finger.
    ///
    /// Acquisition still provides the initial integer/fractional timing. When
    /// enabled, Gardner applies small per-chip prompt nudges on top of that
    /// initial lock using matched-filtered raw samples.
    pub gardner_timing: GardnerTimingConfig,
}

impl PnLcConfig {
    /// Sensible defaults for 4× oversample (4.9152 MHz sample rate).
    pub fn default_4x() -> Self {
        Self {
            oversample: 4,
            coherent_chips: 2048,
            lc_half_span: 4,
            snr_threshold: 8.0,
            lc_best_over_second_min: 1.3,
            preamble_coh_norm_min: 0.20,
            preamble_hits_required: 1,
            replay_preamble_symbols: 16,
            noncoherent_segments: 1,
            // 4 chips worth of guard at 4× oversample
            peak_suppress_samples: 16,
            // Two matched-filter passes, each 48 taps with group delay
            // (48-1)/2 = 23.5 samples per pass: 23.5 × 2 = 47 total.
            composite_filter_delay: 47,
            center_offset_override: None,
            chip_block_size: 256,
            search_interval_windows: 16,
            split_pn_reference: true,
            reanchor_origin: false,
            lc_decimation: 1,
            suppress_search_when_locked: false,
            suppress_active_finger_delay_overlap: false,
            active_finger_delay_suppress_samples: 0,
            enable_epl_tracking: false,
            enable_epl_slew: false,
            epl_pilot: false,
            access_cfo: false,
            fractional_timing_recovery: false,
            fractional_timing_half_samples: 0.5,
            fractional_timing_step_samples: 0.125,
            finger_timing_half_samples: 0.0,
            finger_timing_step_samples: 0.25,
            finger_timing_symmetric: true,
            output_oversampled_chips: false,
            integrate_and_dump: false,
            gardner_timing: GardnerTimingConfig::disabled(),
        }
    }

    pub fn with_fractional_timing_recovery(mut self, enable: bool) -> Self {
        self.fractional_timing_recovery = enable;
        self
    }

    pub fn with_fractional_timing_search(mut self, half_samples: f32, step_samples: f32) -> Self {
        self.fractional_timing_half_samples = half_samples.max(0.0);
        self.fractional_timing_step_samples = step_samples.max(1e-3);
        self
    }

    pub fn with_finger_timing_search(mut self, half_samples: f32, step_samples: f32) -> Self {
        self.finger_timing_half_samples = half_samples.max(0.0);
        self.finger_timing_step_samples = step_samples.max(1e-3);
        self.finger_timing_symmetric = true;
        self
    }

    pub fn with_finger_timing_adaptive_search(
        mut self,
        half_samples: f32,
        step_samples: f32,
    ) -> Self {
        self.finger_timing_half_samples = half_samples.max(0.0);
        self.finger_timing_step_samples = step_samples.max(1e-3);
        self.finger_timing_symmetric = false;
        self
    }

    pub fn with_integrate_and_dump(mut self, enable: bool) -> Self {
        self.integrate_and_dump = enable;
        self
    }

    pub fn with_output_oversampled_chips(mut self, enable: bool) -> Self {
        self.output_oversampled_chips = enable;
        self
    }

    pub fn with_gardner_timing_recovery(mut self, enable: bool) -> Self {
        self.gardner_timing = if enable {
            GardnerTimingConfig::reverse_access_4x()
        } else {
            GardnerTimingConfig::disabled()
        };
        self
    }

    pub fn with_gardner_timing(mut self, cfg: GardnerTimingConfig) -> Self {
        self.gardner_timing = cfg;
        self
    }

    /// Enable instrumentation-only early/prompt/late chip-timing measurement
    /// for fingers spawned by this correlator. See [`PnLcConfig::enable_epl_tracking`].
    pub fn with_epl_tracking(mut self, enable: bool) -> Self {
        self.enable_epl_tracking = enable;
        self
    }

    /// Enable ACTIVE sub-chip timing slewing driven by the EPL
    /// discriminator. Implies `enable_epl_tracking = true`.
    pub fn with_epl_slew(mut self, enable: bool) -> Self {
        self.enable_epl_slew = enable;
        if enable {
            self.enable_epl_tracking = true;
        }
        self
    }

    /// Use pilot-coherent 16-chip Walsh 0 accumulation for the EPL
    /// discriminator. Implies `enable_epl_tracking = true`. Typically
    /// paired with `with_epl_slew(true)` for active timing correction.
    pub fn with_epl_pilot(mut self, enable: bool) -> Self {
        self.epl_pilot = enable;
        if enable {
            self.enable_epl_tracking = true;
        }
        self
    }

    pub fn with_access_cfo(mut self, enable: bool) -> Self {
        self.access_cfo = enable;
        self
    }

    pub fn with_reanchor_origin(mut self, reanchor: bool) -> Self {
        self.reanchor_origin = reanchor;
        self
    }

    pub fn with_lc_decimation(mut self, decimation: usize) -> Self {
        self.lc_decimation = decimation.max(1);
        self
    }

    /// When true, skip FFT search once a hard-validated finger exists.
    /// Use for traffic channels; leave false for access channels.
    pub fn with_suppress_search_when_locked(mut self, suppress: bool) -> Self {
        self.suppress_search_when_locked = suppress;
        self
    }

    pub fn with_active_finger_delay_suppression(
        mut self,
        enable: bool,
        suppress_samples: i32,
    ) -> Self {
        self.suppress_active_finger_delay_overlap = enable;
        self.active_finger_delay_suppress_samples = suppress_samples.max(0);
        self
    }

    pub fn with_lc_half_span(mut self, lc_half_span: i32) -> Self {
        self.lc_half_span = lc_half_span;
        self
    }

    pub fn with_snr_threshold(mut self, snr_threshold: f32) -> Self {
        self.snr_threshold = snr_threshold;
        self
    }

    pub fn with_lc_best_over_second_min(mut self, min: f32) -> Self {
        self.lc_best_over_second_min = min;
        self
    }

    pub fn with_preamble_coh_norm_min(mut self, min: f32) -> Self {
        self.preamble_coh_norm_min = min;
        self
    }

    pub fn with_preamble_hits_required(mut self, hits: u32) -> Self {
        self.preamble_hits_required = hits;
        self
    }

    pub fn with_noncoherent_segments(mut self, noncoherent_segments: usize) -> Self {
        self.noncoherent_segments = noncoherent_segments.max(1);
        self
    }

    pub fn with_search_interval_windows(mut self, search_interval_windows: u64) -> Self {
        self.search_interval_windows = search_interval_windows.max(1);
        self
    }

    pub fn with_center_offset_override(mut self, center_offset_override: usize) -> Self {
        self.center_offset_override = Some(center_offset_override % self.oversample.max(1));
        self
    }

    pub fn with_split_pn_reference(mut self, split_pn_reference: bool) -> Self {
        self.split_pn_reference = split_pn_reference;
        self
    }
}
