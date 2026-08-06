//! Shared scan loop for HRPD reverse-link FFT-driven correlators.
//!
//! The HRPD reverse access and reverse traffic correlators both wrap the
//! shared [`HrpdReverseFftPilotSearcher`] primitive with channel-specific
//! reference generation, finger construction, and timing refinement, but
//! they share the same outer shell: stitch incoming IQ blocks into a
//! contiguous buffer, advance an FFT window by `frame_samples` per scan,
//! dispatch hits to the channel-specific strategy, and forward finger
//! lifecycle notifications.
//!
//! [`HrpdReverseCorrelatorBase`] owns the searcher, the stitched buffer, and
//! the scan loop. [`HrpdReverseFingerSpawnStrategy`] supplies the
//! channel-specific knobs: reference, spawn-gating, finger construction, and
//! lifecycle hooks.
//!
//! The strategy's `spawn_finger` returns `Defer` when it would like to defer
//! the spawn (e.g. the buffer doesn't yet hold enough samples past the hit
//! for sub-chip refinement). In that case the base preserves the current
//! `next_scan_offset` so the next call to `correlate` revisits the same
//! window after more samples arrive. On the second visit the strategy may
//! still defer or finally skip; the base does not itself enforce a "drop
//! after one retry" rule (the strategy owns that decision via its internal
//! state).
//!
//! The strategy chooses how much history to retain after a scan. The base
//! applies that policy to a shared contiguous ring buffer and rebases every
//! buffer-relative cursor together.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use log::{debug, trace};
use num_complex::Complex32;

use cdma_common::contiguous_ring_buffer::ContiguousRingBuffer;

use crate::receiver::hrpd::reverse_fft_pilot_search::{
    HrpdReverseFftPilotHit, HrpdReverseFftPilotSearchConfig, HrpdReverseFftPilotSearcher,
    HrpdReversePilotReference,
};
use crate::receiver::pipelined::generic_rake_receiver::{Correlator, RakeFinger};
use crate::receiver::pipelined::{PipelineProcessorShared, SampleBlock};

/// Cumulative timing counters published by [`HrpdReverseCorrelatorBase`].
///
/// All times are in nanoseconds (Instant::elapsed().as_nanos as u64). Call
/// counts are zero until the first `correlate()` invocation that exercises
/// the matching section.
#[derive(Debug, Default)]
pub struct HrpdReverseCorrelatorMetrics {
    pub append_block_ns: u64,
    pub append_block_calls: u64,
    pub fft_scan_ns: u64,
    pub fft_scan_calls: u64,
    pub spawn_finger_ns: u64,
    pub spawn_finger_calls: u64,
    pub per_block_total_ns: u64,
    pub per_block_total_calls: u64,
    /// FFT searcher internal sub-section counters (snapshotted from the
    /// searcher after each `correlate` call). Reference setup is keyed on
    /// `searcher_ref_setup_calls`; signal FFT / IFFT+mult / peak find share
    /// the `searcher_scan_window_calls` denominator.
    pub searcher_ref_setup_ns: u64,
    pub searcher_ref_setup_calls: u64,
    pub searcher_signal_fft_ns: u64,
    pub searcher_ifft_mult_ns: u64,
    pub searcher_peak_find_ns: u64,
    pub searcher_scan_window_calls: u64,
}

impl HrpdReverseCorrelatorMetrics {
    pub fn fft_scan_avg_us(&self) -> u64 {
        avg_us(self.fft_scan_ns, self.fft_scan_calls)
    }
    pub fn spawn_finger_avg_us(&self) -> u64 {
        avg_us(self.spawn_finger_ns, self.spawn_finger_calls)
    }
    pub fn append_block_avg_us(&self) -> u64 {
        avg_us(self.append_block_ns, self.append_block_calls)
    }
    pub fn per_block_avg_us(&self) -> u64 {
        avg_us(self.per_block_total_ns, self.per_block_total_calls)
    }
    /// Average reference-template build + FFT per rebuild (cache miss).
    pub fn ref_setup_avg_us(&self) -> u64 {
        avg_us(self.searcher_ref_setup_ns, self.searcher_ref_setup_calls)
    }
    /// Average forward signal FFT per window scan.
    pub fn signal_fft_avg_us(&self) -> u64 {
        avg_us(self.searcher_signal_fft_ns, self.searcher_scan_window_calls)
    }
    /// Average correlation (pointwise multiply + inverse FFT) per window scan.
    pub fn ifft_mult_avg_us(&self) -> u64 {
        avg_us(self.searcher_ifft_mult_ns, self.searcher_scan_window_calls)
    }
    /// Average peak-find per window scan.
    pub fn peak_find_avg_us(&self) -> u64 {
        avg_us(self.searcher_peak_find_ns, self.searcher_scan_window_calls)
    }
    /// Total nanoseconds attributed to FFT-searcher sub-stages, for computing
    /// each stage's share.
    pub fn searcher_total_ns(&self) -> u64 {
        self.searcher_ref_setup_ns
            .saturating_add(self.searcher_signal_fft_ns)
            .saturating_add(self.searcher_ifft_mult_ns)
            .saturating_add(self.searcher_peak_find_ns)
    }
}

fn avg_us(ns: u64, calls: u64) -> u64 {
    if calls == 0 { 0 } else { ns / calls / 1000 }
}

pub type SharedHrpdReverseCorrelatorMetrics = Arc<Mutex<HrpdReverseCorrelatorMetrics>>;

/// Process-wide registry of correlator metrics handles, keyed by label.
/// Tests use this to recover the metrics for the correlator that was
/// constructed inside a chain factory or worker thread without having to
/// thread an explicit handle through every API.
fn registry() -> &'static Mutex<HashMap<String, SharedHrpdReverseCorrelatorMetrics>> {
    static REG: OnceLock<Mutex<HashMap<String, SharedHrpdReverseCorrelatorMetrics>>> =
        OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fetch the most recently registered metrics handle for `label`. Returns
/// `None` if no correlator with that label has been constructed yet (or if
/// its handle has been replaced and the previous instance was dropped).
pub fn get_metrics_handle(label: &str) -> Option<SharedHrpdReverseCorrelatorMetrics> {
    registry().lock().ok()?.get(label).cloned()
}

fn register_metrics_handle(label: &str, handle: SharedHrpdReverseCorrelatorMetrics) {
    if let Ok(mut g) = registry().lock() {
        g.insert(label.to_string(), handle);
    }
}

/// Outcome of `spawn_finger`: either spawn a (finger, sub-chain) pair, defer
/// the spawn for this hit (so the base re-tries the same window next call),
/// or skip the hit and continue with the next.
pub enum SpawnOutcome<F: RakeFinger> {
    Spawn(F, Vec<PipelineProcessorShared>),
    Defer,
    Skip,
}

/// Channel-specific behaviour plugged into [`HrpdReverseCorrelatorBase`].
pub trait HrpdReverseFingerSpawnStrategy: Send {
    type Finger: RakeFinger + 'static;
    type Reference: HrpdReversePilotReference;

    /// FFT primitive configuration. Called once at base construction.
    fn fft_config(&self) -> HrpdReverseFftPilotSearchConfig;

    /// Reference template provider, passed by reference to each FFT scan.
    fn reference(&self) -> &Self::Reference;

    /// Maximum hits to request from the FFT primitive per window.
    fn max_hits_per_window(&self) -> usize;

    /// Whether the strategy currently suppresses further spawn attempts
    /// (e.g. single-AT traffic correlator with an active finger).
    fn search_suppressed(&self) -> bool {
        false
    }

    /// Resolve `block.tags["absolute_sample_start"]` (or fall back to
    /// `chip_start * oversample`) into a u64 absolute sample index, and
    /// update any sample-rate state the strategy maintains.
    fn absolute_sample_start_for_block(&mut self, block: &SampleBlock) -> u64;

    /// Hook for the strategy to react to an FFT hit and construct a finger.
    /// Returning `Defer` re-tries this same window on the next correlate
    /// call (without advancing past it again). Returning `Skip` moves on.
    fn spawn_finger(
        &mut self,
        buffer: &[Complex32],
        buffer_abs_sample: u64,
        hit: &HrpdReverseFftPilotHit,
    ) -> SpawnOutcome<Self::Finger>;

    /// Number of samples to discard after a window scan and any spawn
    /// attempts. The shared base applies the trim and rebases all offsets.
    fn buffer_trim_count_after_scan(
        &self,
        buffer_len: usize,
        next_scan_offset: usize,
        window_samples: usize,
    ) -> usize;

    /// Primary search hook. Default delegates to the FFT searcher; strategies
    /// may override to use an alternative searcher (e.g. multi-tier non-
    /// coherent). The returned hits are fed through `spawn_finger` exactly
    /// like FFT hits.
    fn primary_scan_window(
        &mut self,
        searcher: &mut HrpdReverseFftPilotSearcher,
        buffer_slice: &[Complex32],
        window_abs_sample: u64,
    ) -> Vec<HrpdReverseFftPilotHit> {
        let max_hits = self.max_hits_per_window().max(1);
        searcher.scan_top_hits(buffer_slice, window_abs_sample, max_hits, self.reference())
    }

    /// Sample step between successive primary scans. Default is one frame
    /// (matches the FFT searcher's overlapping behavior). Strategies using
    /// an exhaustive per-slot NC scan can step closer to the full window
    /// size, since the NC scan tests every slot in the window directly.
    fn primary_scan_step_samples(&self, default_step: usize) -> usize {
        default_step
    }

    /// Align scan windows to the absolute frame grid (window start chip a
    /// multiple of the frame length). Strategies whose reference depends on
    /// the window's absolute position (the access preamble template's access
    /// cycle number is set by the probe's frame-aligned start) need each
    /// candidate burst to appear at the start of some window, where the
    /// window-derived template parameters match the burst's.
    fn align_scan_to_frame_grid(&self) -> bool {
        false
    }

    fn notify_hard_validated(&mut self, _finger_id: u64) {}
    fn notify_finger_state(
        &mut self,
        _finger_id: u64,
        _hard_validated: bool,
        _idle_chips: u64,
        _signal_lost_chips: u64,
        _crc_miss_count: u64,
        _post_walsh_no_event_ms: u64,
    ) {
    }
    fn notify_finger_removed(&mut self, _finger_id: u64) {}
}

/// Generic FFT-driven HRPD reverse correlator. Owns the FFT searcher and
/// IQ stitching buffer; delegates channel-specific decisions to `S`.
pub struct HrpdReverseCorrelatorBase<S: HrpdReverseFingerSpawnStrategy> {
    strategy: S,
    searcher: HrpdReverseFftPilotSearcher,
    buffer: ContiguousRingBuffer<Complex32>,
    buffer_abs_sample: Option<u64>,
    next_scan_offset: usize,
    /// Hits left over from a deferred window. Revisits reuse these instead
    /// of re-running the FFT scan on the same samples.
    deferred_hits: Option<(usize, Vec<HrpdReverseFftPilotHit>)>,
    label: &'static str,
    metrics: SharedHrpdReverseCorrelatorMetrics,
    log_every: u64,
}

impl<S: HrpdReverseFingerSpawnStrategy> HrpdReverseCorrelatorBase<S> {
    pub fn new(strategy: S, label: &'static str) -> Self {
        let cfg = strategy.fft_config();
        let searcher = HrpdReverseFftPilotSearcher::new(cfg);
        let metrics: SharedHrpdReverseCorrelatorMetrics =
            Arc::new(Mutex::new(HrpdReverseCorrelatorMetrics::default()));
        register_metrics_handle(label, metrics.clone());
        Self {
            strategy,
            searcher,
            buffer: ContiguousRingBuffer::new(),
            buffer_abs_sample: None,
            next_scan_offset: 0,
            deferred_hits: None,
            label,
            metrics,
            log_every: 64,
        }
    }

    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    pub fn searcher(&self) -> &HrpdReverseFftPilotSearcher {
        &self.searcher
    }

    pub fn metrics_handle(&self) -> SharedHrpdReverseCorrelatorMetrics {
        self.metrics.clone()
    }

    fn append_block(&mut self, block: &SampleBlock) {
        let abs = self.strategy.absolute_sample_start_for_block(block);
        // Samples received while a live finger suppresses acquisition are
        // stale by the time that finger is retired. Keep only the current block
        // so a rearmed search starts near real time and accumulates a fresh
        // window instead of FFT-scanning the accumulated call history.
        if self.strategy.search_suppressed() {
            self.buffer.clear();
            self.buffer_abs_sample = Some(abs);
            self.next_scan_offset = 0;
            self.deferred_hits = None;
            self.buffer.extend_from_slice(&block.samples);
            return;
        }
        if let Some(buf_abs) = self.buffer_abs_sample {
            let expected = buf_abs + self.buffer.len() as u64;
            if expected != abs {
                debug!(
                    "{}: sample discontinuity expected={} got={}, resetting acquisition buffer",
                    self.label, expected, abs
                );
                self.buffer.clear();
                self.next_scan_offset = 0;
                self.deferred_hits = None;
                self.buffer_abs_sample = Some(abs);
            }
        } else {
            self.buffer_abs_sample = Some(abs);
        }
        self.buffer.extend_from_slice(&block.samples);
    }

    fn scan_new_windows(&mut self) -> Vec<(S::Finger, Vec<PipelineProcessorShared>)> {
        let Some(mut buffer_abs_sample) = self.buffer_abs_sample else {
            return Vec::new();
        };
        let window_samples = self.searcher.window_samples();
        let frame_samples = self.searcher.frame_samples();
        let mut detections = Vec::new();
        let mut fft_scan_ns: u64 = 0;
        let mut fft_scan_calls: u64 = 0;
        let mut spawn_ns: u64 = 0;
        let mut spawn_calls: u64 = 0;

        // Snap the scan cursor forward to the next frame-grid-aligned window
        // start when the strategy requires it. Steps are whole frames, so the
        // grid holds once established; buffer trims re-derive it from the
        // absolute sample position.
        if self.strategy.align_scan_to_frame_grid() && frame_samples > 0 {
            let window_abs = buffer_abs_sample + self.next_scan_offset as u64;
            let rem = (window_abs % frame_samples as u64) as usize;
            if rem != 0 {
                self.next_scan_offset += frame_samples - rem;
            }
        }

        while self.next_scan_offset + window_samples <= self.buffer.len() {
            if self.strategy.search_suppressed() {
                break;
            }
            let window_abs = buffer_abs_sample + self.next_scan_offset as u64;
            let t = Instant::now();
            // A deferred window already paid for its FFT scan; reuse its
            // remaining hits instead of re-scanning the same samples.
            let cached = match self.deferred_hits.take() {
                Some((offset, hits)) if offset == self.next_scan_offset => Some(hits),
                _ => None,
            };
            let hits = match cached {
                Some(hits) => hits,
                None => self.strategy.primary_scan_window(
                    &mut self.searcher,
                    &self.buffer.as_slice()
                        [self.next_scan_offset..self.next_scan_offset + window_samples],
                    window_abs,
                ),
            };
            fft_scan_ns += t.elapsed().as_nanos() as u64;
            fft_scan_calls += 1;

            let mut deferred = false;
            for (hit_idx, hit) in hits.iter().enumerate() {
                if hit.snr < self.searcher.snr_threshold() {
                    continue;
                }
                let t = Instant::now();
                let outcome =
                    self.strategy
                        .spawn_finger(self.buffer.as_slice(), buffer_abs_sample, hit);
                spawn_ns += t.elapsed().as_nanos() as u64;
                spawn_calls += 1;
                match outcome {
                    SpawnOutcome::Spawn(finger, chain) => {
                        detections.push((finger, chain));
                        if self.strategy.search_suppressed() {
                            break;
                        }
                    }
                    SpawnOutcome::Defer => {
                        // Keep this hit and the rest for the revisit.
                        self.deferred_hits =
                            Some((self.next_scan_offset, hits[hit_idx..].to_vec()));
                        deferred = true;
                        break;
                    }
                    SpawnOutcome::Skip => continue,
                }
            }

            if deferred {
                // Leave `next_scan_offset` unchanged so we revisit this
                // window after more samples arrive.
                break;
            }

            self.next_scan_offset += self.strategy.primary_scan_step_samples(frame_samples);

            if self.strategy.search_suppressed() {
                break;
            }
        }

        // Run trim once per `correlate` call, AFTER all in-flight scans
        // complete. Per-scan trimming inside the loop would eat the head-
        // room the spawn strategy needs for sub-chip refinement (sample-
        // delay search reaches back ~32 samples before the FFT-detected
        // frame start), and would defer-loop indefinitely.
        let drop = self
            .strategy
            .buffer_trim_count_after_scan(self.buffer.len(), self.next_scan_offset, window_samples)
            .min(self.buffer.len())
            .min(self.next_scan_offset);
        if drop > 0 {
            self.buffer.discard_front(drop);
            buffer_abs_sample = buffer_abs_sample.saturating_add(drop as u64);
            self.next_scan_offset -= drop;
            if let Some((offset, _)) = &mut self.deferred_hits {
                *offset = offset.saturating_sub(drop);
            }
        }
        self.buffer_abs_sample = Some(buffer_abs_sample);

        // Commit timing counters under one lock acquisition. Also snapshot
        // the FFT searcher's internal sub-section stats so the test can see
        // where inside the FFT scan the heat goes.
        let searcher_stats = self.searcher.stats();
        if let Ok(mut m) = self.metrics.lock() {
            m.fft_scan_ns = m.fft_scan_ns.saturating_add(fft_scan_ns);
            m.fft_scan_calls = m.fft_scan_calls.saturating_add(fft_scan_calls);
            m.spawn_finger_ns = m.spawn_finger_ns.saturating_add(spawn_ns);
            m.spawn_finger_calls = m.spawn_finger_calls.saturating_add(spawn_calls);
            m.searcher_ref_setup_ns = searcher_stats.ref_setup_ns;
            m.searcher_ref_setup_calls = searcher_stats.ref_setup_calls;
            m.searcher_signal_fft_ns = searcher_stats.signal_fft_ns;
            m.searcher_ifft_mult_ns = searcher_stats.ifft_mult_ns;
            m.searcher_peak_find_ns = searcher_stats.peak_find_ns;
            m.searcher_scan_window_calls = searcher_stats.scan_window_calls;
        }

        detections
    }

    fn log_periodic_summary(&self) {
        let Ok(m) = self.metrics.lock() else {
            return;
        };
        if m.per_block_total_calls == 0 || m.per_block_total_calls % self.log_every != 0 {
            return;
        }
        let total = m.searcher_total_ns().max(1);
        let pct = |ns: u64| ns.saturating_mul(100) / total;
        trace!(
            "hrpd_corr [{}]: per_block_avg={}us append_avg={}us fft_scan_avg={}us(n={}) spawn_avg={}us(n={}) | fft_stages(scan_n={}): ref_setup={}us({}%,miss={}) signal_fft={}us({}%) ifft_mult={}us({}%) peak_find={}us({}%)",
            self.label,
            m.per_block_avg_us(),
            m.append_block_avg_us(),
            m.fft_scan_avg_us(),
            m.fft_scan_calls,
            m.spawn_finger_avg_us(),
            m.spawn_finger_calls,
            m.searcher_scan_window_calls,
            m.ref_setup_avg_us(),
            pct(m.searcher_ref_setup_ns),
            m.searcher_ref_setup_calls,
            m.signal_fft_avg_us(),
            pct(m.searcher_signal_fft_ns),
            m.ifft_mult_avg_us(),
            pct(m.searcher_ifft_mult_ns),
            m.peak_find_avg_us(),
            pct(m.searcher_peak_find_ns),
        );
    }
}

impl<S: HrpdReverseFingerSpawnStrategy> Correlator for HrpdReverseCorrelatorBase<S> {
    type Finger = S::Finger;

    fn correlate(
        &mut self,
        block: &SampleBlock,
    ) -> Vec<(Self::Finger, Vec<PipelineProcessorShared>)> {
        let block_start = Instant::now();
        let t = Instant::now();
        self.append_block(block);
        let append_ns = t.elapsed().as_nanos() as u64;
        let detections = self.scan_new_windows();
        let total_ns = block_start.elapsed().as_nanos() as u64;
        if let Ok(mut m) = self.metrics.lock() {
            m.append_block_ns = m.append_block_ns.saturating_add(append_ns);
            m.append_block_calls = m.append_block_calls.saturating_add(1);
            m.per_block_total_ns = m.per_block_total_ns.saturating_add(total_ns);
            m.per_block_total_calls = m.per_block_total_calls.saturating_add(1);
        }
        self.log_periodic_summary();
        detections
    }

    fn search_suppressed(&self) -> bool {
        self.strategy.search_suppressed()
    }

    fn notify_hard_validated(&mut self, finger_id: u64) {
        self.strategy.notify_hard_validated(finger_id);
    }

    fn notify_finger_state(
        &mut self,
        finger_id: u64,
        hard_validated: bool,
        idle_chips: u64,
        signal_lost_chips: u64,
        crc_miss_count: u64,
        post_walsh_no_event_ms: u64,
    ) {
        self.strategy.notify_finger_state(
            finger_id,
            hard_validated,
            idle_chips,
            signal_lost_chips,
            crc_miss_count,
            post_walsh_no_event_ms,
        );
    }

    fn notify_finger_removed(&mut self, finger_id: u64) {
        self.strategy.notify_finger_removed(finger_id);
    }
}
