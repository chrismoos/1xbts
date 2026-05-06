use std::path::PathBuf;

use cdma_voice_gw::config::VoiceGatewayConfig;
use cdma_voice_gw::service::{VoiceGatewayService, run_grpc_server};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about = "Run the CDMA voice SIP gateway.")]
struct Cli {
    /// Path to the voice gateway config JSON.
    #[arg(long, value_name = "CONFIG")]
    config: Option<PathBuf>,
}

fn resolve_config_path(cli: &Cli) -> Option<PathBuf> {
    cli.config.clone().or_else(|| {
        std::env::var("VOICE_GATEWAY_CONFIG_JSON")
            .ok()
            .map(PathBuf::from)
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();

    let cli = Cli::parse();
    let config = match resolve_config_path(&cli) {
        Some(path) => {
            log::info!("loading voice gateway config from {}", path.display());
            VoiceGatewayConfig::load_from_path(&path)?
        }
        None => VoiceGatewayConfig::default(),
    };
    let addr = config.grpc.listen_addr.parse()?;
    let service = VoiceGatewayService::try_new_with_libre(config.clone()).map_err(|err| {
        eprintln!("ERROR: failed to initialize voice gateway SIP backend: {err}");
        log::error!("failed to initialize voice gateway SIP backend: {err}");
        err
    })?;

    log::info!("Voice gateway gRPC listening on {}", addr);
    run_grpc_server(addr, service).await?;

    Ok(())
}
