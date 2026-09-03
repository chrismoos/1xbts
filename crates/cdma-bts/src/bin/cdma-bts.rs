//! The BTS component as its own process: radio, PHY/MAC/LAC, the Abis
//! endpoint the BSC connects to, the operations plane it enrolls against, and
//! the fused HRPD Access Network.

use std::path::PathBuf;

use cdma_bts::BtsCliOverrides;
use cdma_bts::startup::{init_logging, resolve_config_dir};
use cdma_common::error::Error;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "1xBTS base transceiver station: radio, PHY/MAC/LAC, Abis, and the fused HRPD access network."
)]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// Path to a radio-only config JSON. Overrides the radio config referenced by `bts.json`.
    #[arg(long, value_name = "CONFIG")]
    radio_config: Option<PathBuf>,

    /// Path to a BTS config JSON. Overrides `<config-dir>/bts.json`.
    #[arg(long, value_name = "CONFIG")]
    bts_config: Option<PathBuf>,

    /// Named BTS profile applied after the base config and its local override.
    #[arg(long, value_name = "PROFILE")]
    bts_profile: Option<String>,

    /// Use a null radio that drops all TX samples and provides no RX.
    #[arg(long)]
    null_radio: bool,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();
    let config_dir = resolve_config_dir(cli.config_dir);

    init_logging();
    cdma_bts::debug_dump::install_stack_dump_on_sigusr1();

    cdma_bts::run_node(
        &config_dir,
        BtsCliOverrides {
            radio_config: cli.radio_config,
            bts_config: cli.bts_config,
            bts_profile: cli.bts_profile,
            null_radio: cli.null_radio,
        },
    )
    .await
}
