use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use cdma_common::error::Error;
use num_complex::Complex32;

use super::timing::{PipelineRunStats, PipelineTimingStats};
use super::trace::ChainTraceWriters;
use super::{PipelineProcessorShared, SampleBlock};

struct PipelineChain {
    processors: Vec<PipelineProcessorShared>,
    tx: Sender<Vec<SampleBlock>>,
    trace: Option<ChainTraceWriters>,
    timing: PipelineTimingStats,
}

pub struct PipelinedReceiver<S> {
    stream: S,
    chains: Vec<PipelineChain>,
    batch_size: usize,
    input_sample_rate_hz: f64,
    trace_root: Option<PathBuf>,
    absolute_sample_start: Option<u64>,
}

impl<S> PipelinedReceiver<S>
where
    S: Iterator<Item = Complex32>,
{
    /// Create a new batched pipeline runner from a complex-sample stream.
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            chains: Vec::new(),
            batch_size: 4096,
            input_sample_rate_hz: 0.0,
            trace_root: None,
            absolute_sample_start: None,
        }
    }

    /// Set sample batch size used to feed pipeline processors.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Set the input stream sample rate. This is copied into source blocks.
    pub fn with_input_sample_rate_hz(mut self, sample_rate_hz: f64) -> Self {
        self.input_sample_rate_hz = sample_rate_hz.max(0.0);
        self
    }

    /// Set the absolute sample index for the first sample in the stream.
    /// This is propagated as the `absolute_sample_start` tag on blocks so
    /// downstream processors (e.g. rake receiver) can compute absolute chip
    /// positions.
    pub fn with_absolute_sample_start(mut self, start: u64) -> Self {
        self.absolute_sample_start = Some(start);
        self
    }

    /// Enable per-stage WAV trace files for each pipeline chain.
    pub fn with_trace_files(mut self, root: impl Into<PathBuf>) -> Self {
        self.trace_root = Some(root.into());
        self
    }

    /// Add a processing chain and return a receiver for its emitted blocks.
    pub fn add_pipeline(
        &mut self,
        chain: Vec<PipelineProcessorShared>,
    ) -> Receiver<Vec<SampleBlock>> {
        let (tx, rx) = mpsc::channel();
        let chain_idx = self.chains.len();
        let trace = self
            .trace_root
            .as_ref()
            .map(|root| ChainTraceWriters::new(root, chain_idx, &chain));
        self.chains.push(PipelineChain {
            processors: chain,
            tx,
            trace,
            timing: PipelineTimingStats::default(),
        });
        rx
    }

    pub fn run_pipeline(self) -> Result<(), Error> {
        self.run_pipeline_with_stats().map(|_| ())
    }

    pub(crate) fn run_pipeline_with_stats(mut self) -> Result<PipelineRunStats, Error> {
        let mut chip = 0usize;
        let mut batch_start = 0usize;
        let mut batch = Vec::with_capacity(self.batch_size);
        let abs_start = self.absolute_sample_start;
        let mut run_stats = PipelineRunStats::default();
        while let Some(sample) = self.stream.next() {
            batch.push(sample);
            chip += 1;

            if batch.len() >= self.batch_size {
                let mut block = SampleBlock::new(std::mem::take(&mut batch), batch_start)
                    .with_sample_rate_hz(self.input_sample_rate_hz);
                if let Some(abs) = abs_start {
                    block.tags.insert(
                        "absolute_sample_start",
                        abs.saturating_add(batch_start as u64) as i64,
                    );
                }
                batch.reserve(self.batch_size);
                batch_start = chip;
                let batch_start_time = std::time::Instant::now();
                self.process_block(block)?;
                let elapsed_ns = batch_start_time.elapsed().as_nanos() as u64;
                run_stats.total_batches += 1;
                run_stats.max_batch_elapsed_ns = run_stats.max_batch_elapsed_ns.max(elapsed_ns);
            }
        }
        if !batch.is_empty() {
            let mut block =
                SampleBlock::new(batch, batch_start).with_sample_rate_hz(self.input_sample_rate_hz);
            if let Some(abs) = abs_start {
                block.tags.insert(
                    "absolute_sample_start",
                    abs.saturating_add(batch_start as u64) as i64,
                );
            }
            let batch_start_time = std::time::Instant::now();
            self.process_block(block)?;
            let elapsed_ns = batch_start_time.elapsed().as_nanos() as u64;
            run_stats.total_batches += 1;
            run_stats.max_batch_elapsed_ns = run_stats.max_batch_elapsed_ns.max(elapsed_ns);
        }
        for chain in &mut self.chains {
            let tail = flush_chain(&mut chain.processors, chain.trace.as_mut());
            for blk in &tail {
                if !blk.is_empty() {
                    chain.tx.send(vec![blk.clone()])?;
                }
            }
            if let Some(trace) = chain.trace.as_mut() {
                trace.finalize();
            }
        }
        for (idx, chain) in self.chains.iter().enumerate() {
            chain.timing.report(idx);
        }
        Ok(run_stats)
    }

    fn process_block(&mut self, block: SampleBlock) -> Result<(), Error> {
        let mut emitter = super::VecEmitter::new();
        for chain in self.chains.iter_mut() {
            let mut results = run_chain_once(
                &mut chain.processors,
                block.clone(),
                chain.trace.as_mut(),
                Some(&mut chain.timing),
                &mut emitter,
            );
            results.extend(std::mem::take(&mut emitter.blocks));
            chain.timing.total_batches += 1;
            if !results.is_empty() {
                chain.tx.send(results)?;
            }
        }
        Ok(())
    }
}

fn run_chain_once(
    chain: &mut [PipelineProcessorShared],
    input: SampleBlock,
    mut trace: Option<&mut ChainTraceWriters>,
    mut timing: Option<&mut PipelineTimingStats>,
    emitter: &mut dyn super::PipelineEmitter,
) -> Vec<SampleBlock> {
    let mut blocks = vec![input];
    for (stage_idx, processor) in chain.iter_mut().enumerate() {
        let mut next = Vec::new();
        let stage_start = std::time::Instant::now();
        let mut sample_count = 0u64;
        for blk in blocks {
            if blk.is_empty() {
                continue;
            }
            sample_count += blk.samples.len() as u64;
            next.extend(processor.process_block_emitting(blk, emitter));
        }
        if let Some(ref mut t) = timing {
            let elapsed_ns = stage_start.elapsed().as_nanos() as u64;
            t.record(processor.name(), stage_idx, elapsed_ns, sample_count);
        }
        if let Some(trace_writers) = trace.as_mut() {
            trace_writers.write_stage(stage_idx, &next);
        }
        blocks = next;
    }
    blocks.retain(|b| !b.is_empty());
    blocks
}

/// Run a single input block through a sub-chain of processors, returning all
/// output blocks. Processors may call `emitter.emit()` during
/// `process_block_emitting` to send blocks directly to the pipeline
/// output, bypassing all downstream processors in the chain.
pub fn run_sub_chain(
    chain: &mut [PipelineProcessorShared],
    input: SampleBlock,
    emitter: &mut dyn super::PipelineEmitter,
) -> Vec<SampleBlock> {
    let mut blocks = vec![input];
    for processor in chain.iter_mut() {
        let mut next = Vec::new();
        for blk in blocks {
            if blk.is_empty() {
                continue;
            }
            next.extend(processor.process_block_emitting(blk, emitter));
        }
        blocks = next;
    }
    blocks.retain(|b| !b.is_empty());
    blocks
}

/// Cascade flush through a sub-chain: when processor N flushes, its output
/// passes through downstream processors via `process_block_emitting`.
pub fn flush_sub_chain(
    chain: &mut [PipelineProcessorShared],
    emitter: &mut dyn super::PipelineEmitter,
) -> Vec<SampleBlock> {
    let mut output = Vec::new();
    for idx in 0..chain.len() {
        let (head, tail) = chain.split_at_mut(idx + 1);
        let mut acc = head[idx].flush();
        for processor in tail.iter_mut() {
            let mut next = Vec::new();
            for blk in acc {
                if blk.is_empty() {
                    continue;
                }
                next.extend(processor.process_block_emitting(blk, emitter));
            }
            acc = next;
        }
        output.extend(acc);
    }
    output
}

fn flush_chain(
    chain: &mut [PipelineProcessorShared],
    mut trace: Option<&mut ChainTraceWriters>,
) -> Vec<SampleBlock> {
    let mut output = Vec::new();
    for idx in 0..chain.len() {
        let (head, tail) = chain.split_at_mut(idx + 1);
        let mut acc = head[idx].flush();
        if let Some(trace_writers) = trace.as_mut() {
            trace_writers.write_stage(idx, &acc);
        }
        for processor in tail.iter_mut() {
            let mut next = Vec::new();
            for blk in acc {
                if blk.is_empty() {
                    continue;
                }
                next.extend(processor.process_block(blk));
            }
            acc = next;
        }
        output.extend(acc);
    }
    output
}
