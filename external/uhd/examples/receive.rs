use std::env::set_var;

use anyhow::{Context, Result};
use num_complex::Complex;
use uhd::{self, StreamCommand, StreamCommandType, StreamTime, TuneRequest, Usrp};

const CHANNEL: usize = 0;
const NUM_SAMPLES: usize = 1000;

pub fn main() -> Result<()> {
    set_var("RUST_LOG", "DEBUG");
    env_logger::init();

    log::info!("Starting receive test");

    let addr = Usrp::find("")
        .context("Failed to open device list")?
        .drain(..)
        .next()
        .context("Failed to find a valid USRP to attach to")?;
    let mut usrp = Usrp::open(&addr).context("Failed to find properly open the USRP")?;

    let _ = usrp.set_clock_source("external", 0);
    let clock_source = usrp.get_clock_source(0).unwrap();
    println!("Clock source: {:?}", clock_source);
    assert_eq!(clock_source, "external");
    let _ = usrp.set_clock_source("internal", 0);
    let clock_source = usrp.get_clock_source(0).unwrap();
    println!("Clock source: {:?}", clock_source);
    assert_eq!(clock_source, "internal");

    usrp.set_rx_sample_rate(1e6, CHANNEL)?;
    usrp.set_rx_antenna("TX/RX", CHANNEL)?;
    usrp.set_rx_frequency(&TuneRequest::with_frequency(2.4e9), CHANNEL)?;

    let mut receiver = usrp
        .get_rx_stream(&uhd::StreamArgs::<Complex<i16>>::new("sc16"))
        .unwrap();

    let mut buffer = uhd::alloc_boxed_slice::<Complex<i16>, NUM_SAMPLES>();

    receiver.send_command(&StreamCommand {
        command_type: StreamCommandType::CountAndDone(buffer.len() as u64),
        time: StreamTime::Now,
    })?;
    let status = receiver.receive_simple(buffer.as_mut())?;

    log::info!("{:?}", status);
    log::info!("{:?}", &buffer[..16]);

    Ok(())
}
