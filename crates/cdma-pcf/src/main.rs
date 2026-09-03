//! Standalone PCF node. Runs A9 signaling toward the AN, A11 signaling
//! toward the PDSN, and the A8/A10 bearer relay between them, configured by
//! `config/pcf.json`.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "1xBTS PCF node.")]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    let config_dir = cdma_pcf::resolve_config_dir(cli.config_dir);
    cdma_pcf::run_node(&config_dir).await
}
