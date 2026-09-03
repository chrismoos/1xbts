//! Standalone SMSC node. Serves the SMSC gRPC service configured by
//! `config/smsc.json`.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "1xBTS SMSC node.")]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    let config_dir = cdma_smsc::resolve_config_dir(cli.config_dir);
    cdma_smsc::run_node(&config_dir).await
}
