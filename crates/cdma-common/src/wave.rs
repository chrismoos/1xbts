use std::{fs::File, io::BufReader};

use hound::WavReader;
use num_complex::Complex32;

use crate::error::Error;

pub struct ComplexWaveReader {
    reader: WavReader<BufReader<File>>,
}

impl ComplexWaveReader {
    pub fn iter(path: &str) -> Result<ComplexWaveReader, Error> {
        let reader = hound::WavReader::open(path).unwrap();
        Ok(ComplexWaveReader { reader })
    }

    pub fn sample_rate(&self) -> u32 {
        self.reader.spec().sample_rate
    }
}

impl Iterator for ComplexWaveReader {
    type Item = Complex32;

    fn next(&mut self) -> Option<Self::Item> {
        let i = self.reader.samples::<i16>().next().and_then(|r| r.ok());

        let q = self.reader.samples::<i16>().next().and_then(|r| r.ok());

        if let Some(i) = i
            && let Some(q) = q
        {
            return Some(Complex32::new(
                i as f32 / i16::MAX as f32,
                q as f32 / i16::MAX as f32,
            ));
        }

        None
    }
}
