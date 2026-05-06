use std::collections::HashMap;

/// Per-processor timing accumulator for pipeline profiling.
#[derive(Default)]
pub(super) struct PipelineTimingStats {
    /// (processor_name, stage_index) → (total_ns, block_count, total_samples)
    pub(super) stages: HashMap<(&'static str, usize), (u64, u64, u64)>,
    pub(super) total_batches: u64,
}

impl PipelineTimingStats {
    pub(super) fn record(
        &mut self,
        name: &'static str,
        stage_idx: usize,
        elapsed_ns: u64,
        sample_count: u64,
    ) {
        let entry = self.stages.entry((name, stage_idx)).or_insert((0, 0, 0));
        entry.0 += elapsed_ns;
        entry.1 += 1;
        entry.2 += sample_count;
    }

    pub(super) fn report(&self, chain_idx: usize) {
        if self.stages.is_empty() {
            return;
        }
        let mut entries: Vec<_> = self.stages.iter().collect();
        entries.sort_by_key(|&(&(_, stage_idx), _)| stage_idx);

        let total_ns: u64 = entries.iter().map(|(_, (ns, _, _))| ns).sum();
        eprintln!("\n=== Pipeline Timing Report (chain {}) ===", chain_idx);
        eprintln!(
            "{:<4} {:<45} {:>10} {:>10} {:>10} {:>8}",
            "stg", "processor", "total_ms", "calls", "avg_us", "pct%"
        );
        eprintln!("{}", "-".repeat(92));
        for &(&(name, stage_idx), &(ns, blocks, _samples)) in &entries {
            let total_ms = ns as f64 / 1_000_000.0;
            let avg_us = if blocks > 0 {
                ns as f64 / blocks as f64 / 1_000.0
            } else {
                0.0
            };
            let pct = if total_ns > 0 {
                ns as f64 / total_ns as f64 * 100.0
            } else {
                0.0
            };
            eprintln!(
                "{:<4} {:<45} {:>10.1} {:>10} {:>10.1} {:>7.1}%",
                stage_idx, name, total_ms, blocks, avg_us, pct,
            );
        }
        eprintln!("{}", "-".repeat(92));
        eprintln!(
            "     {:<45} {:>10.1}",
            "TOTAL",
            total_ns as f64 / 1_000_000.0,
        );
        eprintln!("     batches processed: {}", self.total_batches);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PipelineRunStats {
    pub(crate) total_batches: u64,
    pub(crate) max_batch_elapsed_ns: u64,
}
