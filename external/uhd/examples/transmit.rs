use core::f32::consts;
use std::env::set_var;

use anyhow::{Context, Result};
use num_complex::{Complex, Complex32};
use uhd::{self, TuneRequest, Usrp};

const CHANNEL: usize = 0;
const NUM_SAMPLES: usize = 1_000_000;

pub fn main() -> Result<()> {
    set_var("RUST_LOG", "DEBUG");
    env_logger::init();

    log::info!("Starting transmit test");

    let addr = Usrp::find("")
        .context("Failed to open device list")?
        .drain(..)
        .next()
        .context("Failed to find a valid USRP to attach to")?;
    let mut usrp = Usrp::open(&addr).context("Failed to find properly open the USRP")?;

    // Set properties
    usrp.set_tx_sample_rate(1e6, CHANNEL)?;
    usrp.set_tx_gain(77.5, CHANNEL, "PGA")?; // -10dB gain
    usrp.set_tx_frequency(&TuneRequest::with_frequency(2.404e9), CHANNEL)?;

    // Check properties
    log::info!("Tx gain {}", usrp.get_tx_gain(CHANNEL, "PGA")?);
    log::info!("Tx freq {}", usrp.get_tx_frequency(CHANNEL)?);

    // Get TransmitStreamer
    let mut transmitter = usrp
        .get_tx_stream(&uhd::StreamArgs::<Complex<i16>>::new("sc16"))
        .unwrap();

    // Generate a sine wave at Fs/4
    let mut single_chan = uhd::alloc_boxed_slice::<Complex<i16>, NUM_SAMPLES>();
    for i in 0..NUM_SAMPLES {
        let t = i as f32 / 4.;
        // z = e^j*2π*theta
        let z = (Complex32::i() * 2. * consts::PI * t).expf(consts::E);
        single_chan[i] = Complex::new((8192. * z.re) as i16, (8192. * z.im) as i16);
    }

    // Transmit
    log::info!("Transmitting..");
    let stat = transmitter.transmit_simple(single_chan.as_mut())?;
    log::info!("{:?}", stat);

    Ok(())
}
