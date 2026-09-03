//! Standalone PDSN node. Serves the packet gRPC service, A11 signaling
//! toward the PCF, and the A10 bearer, configured by `config/pdsn.json`.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "1xBTS PDSN node.")]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    let config_dir = cdma_pdsn::resolve_config_dir(cli.config_dir);
    cdma_pdsn::run_node(&config_dir).await
}
