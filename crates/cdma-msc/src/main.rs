//! Standalone MSC node. Runs the MSC call-control runtime, attaching to
//! every configured access node, with its management gRPC service,
//! configured by `config/msc.json`.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "1xBTS MSC node.")]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// MSC management gRPC listen address. Overrides `mgmt_grpc_addr` from
    /// `msc.json`.
    #[arg(long, value_name = "ADDR")]
    msc_mgmt_addr: Option<SocketAddr>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    let config_dir = cdma_msc::resolve_config_dir(cli.config_dir);
    cdma_msc::run_node(
        &config_dir,
        cdma_msc::MscCliOverrides {
            mgmt_addr: cli.msc_mgmt_addr,
        },
    )
    .await
}
