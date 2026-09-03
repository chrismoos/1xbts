//! Standalone aggregated event-bus node. Serves `events.v1.EventService`
//! configured by `config/events.json`.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "1xBTS aggregated event bus node.")]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    let config_dir = cdma_events::resolve_config_dir(cli.config_dir);
    cdma_events::run_node(&config_dir).await
}
