//! The BSC network element as its own process: it attaches to every
//! configured BTS, runs call control, and serves the management gRPC.

use std::path::PathBuf;

use cdma_common::error::Error;
use cdma_common::startup::{init_logging, resolve_config_dir};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "1xBTS base station controller: BTS enrollment, call control, and the management plane."
)]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();
    let config_dir = resolve_config_dir(cli.config_dir);

    init_logging();

    cdma_bsc::run_node(&config_dir).await
}
