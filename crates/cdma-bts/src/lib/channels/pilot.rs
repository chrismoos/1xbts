use cdma_common::time::CdmaSystemTime;
use num::complex::Complex32;

use super::Channel;

pub struct ForwardPilotChannel;

impl ForwardPilotChannel {
    pub fn new() -> ForwardPilotChannel {
        ForwardPilotChannel
    }

    // Pilot channel is all 0s, mapped to +1 before Walsh and short-code spreading.
    pub fn next(&self) -> Complex32 {
        Complex32::new(1.0, 0.0)
    }
}

impl Channel for ForwardPilotChannel {
    fn next_block(&self, num_samples: usize, _system_time: CdmaSystemTime) -> Vec<Complex32> {
        vec![Complex32::new(1.0, 0.0); num_samples]
    }

    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        _system_time: CdmaSystemTime,
    ) {
        out.resize(out.len() + num_samples, Complex32::new(1.0, 0.0));
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::{
        channels::{Channel, WalshAndSpreadChannel},
        phy::spread::{PnSequence, Spreader},
        phy::walsh::WalshGenerator,
    };

    use super::ForwardPilotChannel;

    #[test]
    pub fn test_pilot_signal_generate() {
        let fpch = ForwardPilotChannel::new();

        let samples = 32768 * 2;

        let spreader = WalshAndSpreadChannel::new(
            WalshGenerator::new::<64>(0, 1),
            Spreader::new(PnSequence::new(0, 32768)),
            fpch,
        );
        let output = spreader.next_block(samples, Utc::now());

        assert_eq!(samples, output.len());
    }
}
