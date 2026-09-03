//! Standalone HLR node. Serves the HLR gRPC service configured by
//! `config/hlr.json`.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "1xBTS HLR node.")]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    let config_dir = cdma_hlr::resolve_config_dir(cli.config_dir);
    cdma_hlr::run_node(&config_dir).await
}
