use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

use super::{PipelineProcessorShared, SampleBlock};

pub(super) struct StageTraceWriter {
    path: PathBuf,
    tmp_path: PathBuf,
    tmp_writer: Option<BufWriter<std::fs::File>>,
    sample_rate_hz: Option<f64>,
    min_val: f32,
    max_val: f32,
    disabled: bool,
}

impl StageTraceWriter {
    pub(super) fn new(path: PathBuf) -> Self {
        let mut tmp_path = path.clone();
        tmp_path.set_extension("tmpf32");
        Self {
            path,
            tmp_path,
            tmp_writer: None,
            sample_rate_hz: None,
            min_val: f32::INFINITY,
            max_val: f32::NEG_INFINITY,
            disabled: false,
        }
    }

    pub(super) fn sanitize_name(name: &str) -> String {
        let short = name.rsplit("::").next().unwrap_or(name);
        short
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn ensure_writer(&mut self, sample_rate_hz: f64) {
        if self.disabled || self.tmp_writer.is_some() || sample_rate_hz <= 0.0 {
            return;
        }
        if let Some(parent) = self.path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            eprintln!("trace_writer: failed to create dir {parent:?}: {e}");
            self.disabled = true;
            return;
        }

        self.sample_rate_hz = Some(sample_rate_hz);
        let _ = fs::remove_file(&self.tmp_path);
        match std::fs::File::create(&self.tmp_path) {
            Ok(file) => self.tmp_writer = Some(BufWriter::new(file)),
            Err(e) => {
                eprintln!("trace_writer: failed to create {:?}: {e}", self.tmp_path);
                self.disabled = true;
            }
        }
    }

    pub(super) fn write_blocks(&mut self, blocks: &[SampleBlock]) {
        if self.disabled || blocks.is_empty() {
            return;
        }
        let sample_rate_hz = blocks
            .iter()
            .find_map(|b| (b.sample_rate_hz > 0.0).then_some(b.sample_rate_hz))
            .unwrap_or(0.0);
        self.ensure_writer(sample_rate_hz);
        let Some(writer) = self.tmp_writer.as_mut() else {
            return;
        };
        for block in blocks {
            for s in &block.samples {
                self.min_val = self.min_val.min(s.re).min(s.im);
                self.max_val = self.max_val.max(s.re).max(s.im);
                if let Err(e) = writer.write_all(&s.re.to_le_bytes()) {
                    eprintln!(
                        "trace_writer: failed writing I sample to {:?}: {e}",
                        self.path
                    );
                    self.disabled = true;
                    self.tmp_writer = None;
                    return;
                }
                if let Err(e) = writer.write_all(&s.im.to_le_bytes()) {
                    eprintln!(
                        "trace_writer: failed writing Q sample to {:?}: {e}",
                        self.path
                    );
                    self.disabled = true;
                    self.tmp_writer = None;
                    return;
                }
            }
        }
    }

    pub(super) fn finalize(&mut self) {
        if let Some(mut writer) = self.tmp_writer.take()
            && let Err(e) = writer.flush()
        {
            eprintln!("trace_writer: failed to flush {:?}: {e}", self.tmp_path);
            self.disabled = true;
            return;
        }
        let Some(sample_rate_hz) = self.sample_rate_hz else {
            return;
        };
        if self.disabled {
            let _ = fs::remove_file(&self.tmp_path);
            return;
        }

        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: sample_rate_hz.round().max(1.0) as u32,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut wav_writer = match hound::WavWriter::create(&self.path, spec) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("trace_writer: failed to create {:?}: {e}", self.path);
                let _ = fs::remove_file(&self.tmp_path);
                return;
            }
        };

        let peak = self.min_val.abs().max(self.max_val.abs());
        let scale = if peak.is_finite() && peak > 0.0 {
            0.99 / peak
        } else {
            1.0
        };

        let file = match std::fs::File::open(&self.tmp_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("trace_writer: failed to reopen {:?}: {e}", self.tmp_path);
                let _ = fs::remove_file(&self.tmp_path);
                return;
            }
        };
        let mut reader = BufReader::new(file);
        loop {
            let mut ib = [0u8; 4];
            match reader.read_exact(&mut ib) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => {
                    eprintln!(
                        "trace_writer: failed reading I sample from {:?}: {e}",
                        self.tmp_path
                    );
                    break;
                }
            }
            let mut qb = [0u8; 4];
            if let Err(e) = reader.read_exact(&mut qb) {
                eprintln!(
                    "trace_writer: failed reading Q sample from {:?}: {e}",
                    self.tmp_path
                );
                break;
            }

            let i = f32::from_le_bytes(ib) * scale;
            let q = f32::from_le_bytes(qb) * scale;
            if let Err(e) = wav_writer.write_sample(i) {
                eprintln!(
                    "trace_writer: failed writing I sample to {:?}: {e}",
                    self.path
                );
                break;
            }
            if let Err(e) = wav_writer.write_sample(q) {
                eprintln!(
                    "trace_writer: failed writing Q sample to {:?}: {e}",
                    self.path
                );
                break;
            }
        }
        if let Err(e) = wav_writer.finalize() {
            eprintln!("trace_writer: failed to finalize {:?}: {e}", self.path);
        }
        let _ = fs::remove_file(&self.tmp_path);
    }
}

pub(super) struct ChainTraceWriters {
    stages: Vec<StageTraceWriter>,
}

impl ChainTraceWriters {
    pub(super) fn new(root: &PathBuf, chain_idx: usize, chain: &[PipelineProcessorShared]) -> Self {
        let mut stages = Vec::with_capacity(chain.len());
        for (stage_idx, processor) in chain.iter().enumerate() {
            let stage_name = StageTraceWriter::sanitize_name(processor.name());
            let path = root.join(format!(
                "chain{chain_idx:02}_stage{stage_idx:02}_{stage_name}.wav"
            ));
            stages.push(StageTraceWriter::new(path));
        }
        Self { stages }
    }

    pub(super) fn write_stage(&mut self, stage_idx: usize, blocks: &[SampleBlock]) {
        if let Some(stage) = self.stages.get_mut(stage_idx) {
            stage.write_blocks(blocks);
        }
    }

    pub(super) fn finalize(&mut self) {
        for stage in &mut self.stages {
            stage.finalize();
        }
    }
}
