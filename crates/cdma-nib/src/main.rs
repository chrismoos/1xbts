use std::{net::SocketAddr, path::PathBuf, sync::Arc, thread};

use cdma_bsc::{
    config::{self, BscNodeConfig, ManagementConfig, validate_page_chan_alignment},
    grpc::run_grpc_server,
};
use cdma_bts::bts::{
    BtsLaunchOptions, BtsNodeConfig, RadioBuildOptions, build_bts_launch_parts,
    build_radio_from_config, load_radio_from_path, spawn_configured_local_abis_endpoint,
};
use cdma_common::error::Error;
use cdma_hlr::{HlrNodeConfig, repository::GrpcHlrRepository};
use cdma_msc::{MscRuntime, MscRuntimeConfig, StaticVoicePolicy};
use cdma_pdsn::PdsnNodeConfig;
use cdma_smsc::{SmscNodeConfig, repository::GrpcSmscRepository};
use clap::Parser;
use log::{info, warn};
use tracing_subscriber::{EnvFilter, prelude::*, util::SubscriberInitExt};

mod debug_dump;

const DEFAULT_LOG_FILTER: &str = "info";

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "1xBTS network-in-a-box: launches BTS, BSC, and MSC nodes with real transports."
)]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// Path to a radio-only config JSON. Overrides the radio config referenced by `bts.json`.
    #[arg(long, value_name = "CONFIG")]
    radio_config: Option<PathBuf>,

    /// Use a null radio that drops all TX samples and provides no RX.
    #[arg(long)]
    null_radio: bool,

    /// A1 signaling listen address for the MSC. The BSC connects here.
    /// Defaults to `127.0.0.1:17017`.
    #[arg(long, value_name = "ADDR")]
    a1_addr: Option<SocketAddr>,

    /// MSC management gRPC listen address. Defaults to `127.0.0.1:17017`.
    #[arg(long, value_name = "ADDR")]
    msc_mgmt_addr: Option<SocketAddr>,
}

fn resolve_config_dir(cli: &Cli) -> PathBuf {
    if let Some(dir) = cli.config_dir.clone() {
        return dir;
    }
    if let Ok(dir) = std::env::var("CDMA_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(config::DEFAULT_CONFIG_DIR)
}

fn effective_log_filter() -> String {
    std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_string())
}

fn init_logging(enable_tokio_console: bool) {
    let filter = effective_log_filter();
    let fmt_filter = EnvFilter::builder().parse_lossy(&filter);

    if enable_tokio_console {
        let console_layer = console_subscriber::Builder::default()
            .server_addr(([127, 0, 0, 1], 17018))
            .with_default_env()
            .spawn();
        let _ = tracing_subscriber::registry()
            .with(console_layer)
            .with(tracing_subscriber::fmt::layer().with_filter(fmt_filter))
            .try_init();
    } else {
        let _ = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_filter(fmt_filter))
            .try_init();
    }

    let _ = env_logger::Builder::new()
        .parse_filters(&filter)
        .format_timestamp_millis()
        .try_init();
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();
    let config_dir = resolve_config_dir(&cli);

    let mut bts_config =
        BtsNodeConfig::load_from_path(&config_dir.join(config::BTS_CONFIG_FILENAME))?;
    if let Some(radio_config_path) = &cli.radio_config {
        bts_config.radio = load_radio_from_path(radio_config_path)?;
    }
    let bsc_config = BscNodeConfig::load_from_path(&config_dir.join(config::BSC_CONFIG_FILENAME))?;
    let msc_config =
        cdma_msc::MscNodeConfig::load_from_path(&config_dir.join(config::MSC_CONFIG_FILENAME))
            .map_err(|e| Error::from(format!("load msc config: {e}")))?;
    let pcf_config =
        cdma_pcf::PcfNodeConfig::load_from_path(&config_dir.join(config::PCF_CONFIG_FILENAME))
            .map_err(|e| Error::from(format!("load pcf config: {e}")))?;
    let pdsn_config =
        PdsnNodeConfig::load_from_path(&config_dir.join(config::PDSN_CONFIG_FILENAME))
            .map_err(|e| Error::from(format!("load pdsn config: {e}")))?;
    let hlr_config = HlrNodeConfig::load_from_path(&config_dir.join(config::HLR_CONFIG_FILENAME))
        .map_err(|e| Error::from(format!("load hlr config: {e}")))?;
    let smsc_config =
        SmscNodeConfig::load_from_path(&config_dir.join(config::SMSC_CONFIG_FILENAME))
            .map_err(|e| Error::from(format!("load smsc config: {e}")))?;
    let mgmt_config =
        ManagementConfig::load_from_path(&config_dir.join(config::MANAGEMENT_CONFIG_FILENAME))?;

    validate_page_chan_alignment(
        bts_config.overhead.page_chan,
        bts_config.runtime.downlink.paging.paging_channel_number,
    )?;

    let iq_capture_dir = mgmt_config.iq_capture_dir.clone();

    init_logging(mgmt_config.tokio_console);
    debug_dump::install_stack_dump_on_sigusr1();

    info!("Loading per-node configs from {}", config_dir.display());
    if cli.radio_config.is_some() {
        info!("Radio config overridden from CLI");
    }
    if cfg!(debug_assertions) {
        warn!("================================================================");
        warn!("WARNING: cdma-nib is running in a Rust debug build.");
        warn!("Timing, throughput, and RF behavior will not match `--release`.");
        warn!("Use `cargo run --release -p cdma-nib -- ...` for real-time work.");
        warn!("================================================================");
    }

    // A1 signaling transport (BSC <-> MSC)
    let a1_addr = cli.a1_addr.unwrap_or(msc_config.a1_listen_addr);
    let a1_listener = tokio::net::TcpListener::bind(a1_addr)
        .await
        .map_err(|e| Error::from(format!("failed to bind A1 signaling listener: {e}")))?;
    info!("A1 signaling listener bound on {a1_addr}");

    // Radio and BTS
    info!("Starting BTS/BSC/MSC stack (network-in-a-box)");
    let radio = build_radio_from_config(
        &bts_config.radio,
        RadioBuildOptions {
            null_radio: cli.null_radio,
        },
    )?;
    let bts_parts = build_bts_launch_parts(
        bts_config.clone(),
        radio,
        BtsLaunchOptions {
            paging_ack_timeout_ms: bsc_config.paging_retry.ack_timeout_ms,
            paging_max_retries: bsc_config.paging_retry.max_retries,
        },
    );
    let cdma_bts::bts::BtsLaunchParts {
        bts,
        handle: bts_handle,
        resource_controller: bts_resource_controller,
        lac_layer,
        mac_layer,
        reverse_bearer_rx,
        traffic_ack_seq_rx,
        paging_state,
        pch_transmit_tx,
        overhead: overhead_params,
        paging_settings,
    } = bts_parts;
    let cdma_bts::bts::BtsHandle {
        tx_metrics,
        rx_metrics,
        config: bts_runtime_config,
        access_events,
        commands: bts_commands,
        power_control: bts_power_control,
        ..
    } = bts_handle;
    // MSC management plane (gRPC hosted internally by MSC runtime)
    let msc_mgmt_addr = cli.msc_mgmt_addr.unwrap_or(msc_config.mgmt_grpc_addr);

    // HLR and SMSC services
    let hlr_addr = cdma_hlr::service::spawn_configured_hlr_service(hlr_config)
        .await
        .map_err(Error::from)?;
    let smsc_addr = cdma_smsc::service::spawn_configured_smsc_service(smsc_config)
        .await
        .map_err(Error::from)?;
    let hlr_repo: Arc<dyn cdma_hlr::repository::HlrRepository> = Arc::new(
        GrpcHlrRepository::connect_addr(hlr_addr)
            .await
            .map_err(Error::from)?,
    );
    let smsc_repo: Arc<dyn cdma_smsc::repository::SmscRepository> = Arc::new(
        GrpcSmscRepository::connect_addr(smsc_addr)
            .await
            .map_err(Error::from)?,
    );
    info!("HLR gRPC service listening on {hlr_addr}");
    info!("SMSC gRPC service listening on {smsc_addr}");

    // Packet data
    info!("Packet data transport: {:?}", pdsn_config.packet.transport);
    let (packet_endpoint, _packet_server) =
        cdma_pdsn::spawn_configured_packet_service(&pdsn_config).map_err(Error::from)?;
    let pcf_endpoint = pcf_config.packet_grpc_endpoint.clone();
    let pcf_client: Arc<dyn cdma_bsc::packet::PcfClient> =
        Arc::new(cdma_bsc::packet::GrpcPcfClient::new(pcf_endpoint.clone()));
    info!("Packet gRPC service listening at {packet_endpoint}; PCF client target={pcf_endpoint}");

    // BTS resource controller and Abis client
    let abis_bind_addr = spawn_configured_local_abis_endpoint(
        &bts_config,
        bts_resource_controller,
        reverse_bearer_rx,
        traffic_ack_seq_rx,
        paging_state,
        access_events,
    )
    .await?;
    info!(
        "Using Abis TCP: BTS listener={abis_bind_addr}; BSC target={}; bearer BSC={} -> BTS={}",
        bsc_config.abis.remote_addr, bsc_config.bearer.bind_addr, bsc_config.bearer.remote_addr
    );
    let (bts_client, abis_access_event_rx) =
        cdma_bsc::bsc::connect_configured_bts_client(&bsc_config, &bts_config)
            .await
            .map_err(Error::from)?;

    // A1 transport: accept BSC connection, create MSC endpoint
    // The MSC side accepts a TCP connection from the BSC. The BSC connects
    // via NetworkMscClient; the MSC uses the network MscA1Endpoint which
    // implements the cdma_msc::MscA1Endpoint trait.
    let msc_a1_accept = tokio::spawn(async move {
        cdma_bsc::a1_edge::network::MscA1Endpoint::accept_with_retry(&a1_listener).await
    });

    // BSC connects to MSC A1 signaling endpoint.
    let bsc_msc_client =
        cdma_bsc::a1_edge::network::NetworkMscClient::connect_with_reconnect(a1_addr)
            .await
            .map_err(|e| Error::from(format!("failed to connect BSC to MSC A1 signaling: {e}")))?;
    info!("BSC connected to MSC A1 signaling at {a1_addr}");

    let msc_a1_endpoint = msc_a1_accept
        .await
        .map_err(|e| Error::from(format!("A1 accept task failed: {e}")))?
        .map_err(|e| {
            Error::from(format!(
                "failed to accept A1 signaling connection from BSC: {e}"
            ))
        })?;
    info!("MSC accepted A1 signaling connection from BSC");

    // MSC runtime and management gRPC
    let mut msc_runtime_config = MscRuntimeConfig::from_node_config(&msc_config, hlr_repo.clone());
    msc_runtime_config.smsc_repo = Some(smsc_repo.clone());
    tokio::spawn(async move {
        info!("MSC management gRPC server on {msc_mgmt_addr}");
        let mut runtime = MscRuntime::new(msc_runtime_config);
        runtime.run_with_grpc(msc_mgmt_addr, &msc_a1_endpoint).await;
    });
    // BSC runtime and management state
    let bsc_parts = cdma_bsc::bsc::build_bsc_launch_parts(cdma_bsc::bsc::BscLaunchInputs {
        pilot_offset: bts_config.pilot_offset,
        overhead: overhead_params,
        paging: paging_settings.clone(),
        traffic_assignment: bsc_config.traffic_assignment.clone(),
        traffic_retry: bsc_config.traffic_retry.clone(),
        paging_retry: bsc_config.paging_retry.clone(),
        mobile_idle_timeout_s: bsc_config.mobile_idle_timeout_s,
        rx_reference_dbm: bts_config.radio.rx_reference_dbm(),
        access_event_rx: abis_access_event_rx,
        tx_metrics,
        rx_metrics,
        bts_config: bts_runtime_config,
        bts_commands,
        bts_power_control,
        iq_capture_dir: iq_capture_dir.clone(),
        hlr_repo: hlr_repo.clone(),
        smsc_repo: smsc_repo.clone(),
        packet_endpoint: pcf_endpoint.clone(),
        bts_client: bts_client.clone(),
        msc_client: Arc::new(bsc_msc_client),
        voice_policy: Arc::new(StaticVoicePolicy::new(msc_config.voice.clone())),
        pcf_client: pcf_client.clone(),
        pch_transmit_tx: pch_transmit_tx.clone(),
        voice_bearer_bind_ip: bsc_config.voice_bearer_bind_ip,
        node_id: bsc_config.node_id.clone(),
    });
    let bsc_state = bsc_parts.state.clone();
    let bsc = bsc_parts.bsc;
    tokio::spawn(async move {
        if let Err(e) = bsc.run().await {
            log::error!("BSC fatal error: {}", e);
            std::process::exit(1);
        }
    });

    // BSC management gRPC server

    let grpc_addr: SocketAddr = mgmt_config.grpc_listen_addr;
    let grpc_mtls = mgmt_config.mtls.clone();
    let grpc_state = bsc_state.clone();
    let packet_endpoint_for_grpc = pcf_endpoint.clone();
    tokio::spawn(async move {
        if let Err(e) =
            run_grpc_server(grpc_state, packet_endpoint_for_grpc, grpc_addr, grpc_mtls).await
        {
            log::error!("BSC gRPC server error: {}", e);
        }
    });

    // LAC / MAC threads
    thread::spawn(move || {
        if let Err(e) = lac_layer.start() {
            log::error!("LAC thread exited with error: {e}");
        }
    });

    thread::spawn(move || {
        if let Err(e) = mac_layer.start() {
            log::error!("MAC thread exited with error: {e}");
        }
    });

    // BTS (blocks on TX/RX)
    bts.start().await?;

    Ok(())
}
