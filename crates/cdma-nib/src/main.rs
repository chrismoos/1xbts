use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use cdma_an::HrpdDerivedImsiConfig;
use cdma_bsc::{
    config::{self, BscNodeConfig, ManagementConfig, validate_page_chan_alignment},
    grpc::run_grpc_server,
};
use cdma_bts::bts::{
    BtsLaunchOptions, BtsNodeConfig, RadioBuildOptions, build_bts_launch_parts,
    build_radio_from_config, evdo, load_radio_from_path, resolve_reverse_rx_plan,
    spawn_configured_local_abis_endpoint,
};
use cdma_common::error::Error;
use cdma_events::EventsNodeConfig;
use cdma_hlr::{HlrNodeConfig, repository::GrpcHlrRepository};
use cdma_msc::{MscRuntime, MscRuntimeConfig, StaticVoicePolicy};
use cdma_pcf::spawn_hrpd_pcf_a9_service;
use cdma_pdsn::{PdsnNodeConfig, spawn_hrpd_pdsn_a11_service};
use cdma_smsc::{SmscNodeConfig, repository::GrpcSmscRepository};
use clap::Parser;
use log::{info, warn};
use tracing_subscriber::{EnvFilter, prelude::*, util::SubscriberInitExt};

mod debug_dump;
use cdma_nib::hrpd_bridge::*;

const DEFAULT_LOG_FILTER: &str = "info";
const DEFAULT_LOG_CLAMPS: &[&str] = &[
    "sqlx=warn",
    "h2=warn",
    "hyper=warn",
    "hyper_util=warn",
    "tower=warn",
    "tonic=warn",
];
const DEFAULT_GLOBAL_DEBUG_PROFILE: &[&str] = &[
    DEFAULT_LOG_FILTER,
    "cdma_packet=debug",
    "cdma_an=debug",
    "cdma_bsc::bsc::packet=debug",
    "cdma_bsc::bsc::traffic_forward=debug",
    "cdma_bsc::bsc::traffic_signaling=debug",
    "cdma_bsc::bsc::access=debug",
    "cdma_bts::bts::abis_agent=debug",
    "cdma_bts::bts::evdo=debug",
    "cdma_bts::bts::hrpd=debug",
    "cdma_bts::receiver::hrpd::reverse_traffic_rake=debug",
    "cdma_abis::transport=debug",
    "cdma_pcf=debug",
    "cdma_pdsn=debug",
];
// Rev 0 Forward Traffic MAC packets have 1000 security-layer bits after the
// two-bit MAC trailer. Format-B length + Stream header + 22-bit RLP sequence
// leaves 121 octets for one Default Packet RLP payload.

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

    /// Path to a BTS config JSON. Overrides `<config-dir>/bts.json`.
    #[arg(long, value_name = "CONFIG")]
    bts_config: Option<PathBuf>,

    /// Named BTS profile applied after the base config and its local override.
    #[arg(long, value_name = "PROFILE")]
    bts_profile: Option<String>,

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

fn resolve_bts_profile_path(config_dir: &Path, profile: &str) -> Result<PathBuf, Error> {
    if profile.is_empty()
        || !profile
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "invalid BTS profile {profile:?}; use lowercase letters, digits, and hyphens"
        )
        .into());
    }
    Ok(config_dir.join(format!("bts.{profile}.json")))
}

fn effective_log_filter() -> String {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_string());
    apply_default_log_clamps(&filter)
}

fn apply_default_log_clamps(filter: &str) -> String {
    let requested_directives = filter
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let global_debug_requested = requested_directives
        .iter()
        .any(|directive| matches!(directive.as_str(), "debug" | "trace"));
    let mut directives = Vec::new();

    if global_debug_requested {
        directives.extend(
            DEFAULT_GLOBAL_DEBUG_PROFILE
                .iter()
                .map(|entry| entry.to_string()),
        );
    }

    directives.extend(
        requested_directives
            .into_iter()
            .filter(|directive| !matches!(directive.as_str(), "debug" | "trace")),
    );

    for clamp in DEFAULT_LOG_CLAMPS {
        let target = clamp
            .split_once('=')
            .map(|(target, _)| target)
            .unwrap_or(clamp);
        let target_already_configured = directives.iter().any(|directive| {
            directive
                .split_once('=')
                .map(|(configured_target, _)| configured_target.trim() == target)
                .unwrap_or(false)
        });
        if !target_already_configured {
            directives.push((*clamp).to_string());
        }
    }

    directives.join(",")
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

    let bts_config_path = cli
        .bts_config
        .clone()
        .unwrap_or_else(|| config_dir.join(config::BTS_CONFIG_FILENAME));
    let bts_profile_path = cli
        .bts_profile
        .as_deref()
        .map(|profile| resolve_bts_profile_path(&config_dir, profile))
        .transpose()?;
    let radio_override = cli
        .radio_config
        .as_deref()
        .map(load_radio_from_path)
        .transpose()?;
    let bts_config = BtsNodeConfig::load_from_path_with_overrides(
        &bts_config_path,
        bts_profile_path.as_deref(),
        radio_override,
    )?;
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
    let events_config_path = config_dir.join(config::EVENTS_CONFIG_FILENAME);
    let events_config = if events_config_path.exists() {
        Some(
            EventsNodeConfig::load_from_path(&events_config_path)
                .map_err(|e| Error::from(format!("load events config: {e}")))?,
        )
    } else {
        None
    };

    validate_page_chan_alignment(
        bts_config.overhead.page_chan,
        bts_config.runtime.downlink.paging.paging_channel_number,
    )?;

    let iq_capture_dir = mgmt_config.iq_capture_dir.clone();

    init_logging(mgmt_config.tokio_console);
    debug_dump::install_stack_dump_on_sigusr1();

    info!("Loading per-node configs from {}", config_dir.display());
    if let Some(path) = &cli.bts_config {
        info!("BTS config overridden from {}", path.display());
    }
    if let (Some(profile), Some(path)) = (&cli.bts_profile, &bts_profile_path) {
        info!("BTS profile {profile} applied from {}", path.display());
    }
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
    let reverse_rx_plan = resolve_reverse_rx_plan(&bts_config)?;
    let radio = build_radio_from_config(
        &bts_config.radio,
        reverse_rx_plan.center_frequency_hz,
        RadioBuildOptions {
            null_radio: cli.null_radio,
            configure_rx: reverse_rx_plan.configure_rx,
            tx_sample_rate_hz: bts_config.runtime.tx_sample_rate_hz,
            rx_sample_rate_hz: reverse_rx_plan.sample_rate_hz,
            rx_bandwidth_hz: reverse_rx_plan.bandwidth_hz,
            realtime: bts_config.runtime.realtime.clone(),
        },
    )?;
    let bts_parts = build_bts_launch_parts(
        bts_config.clone(),
        radio,
        BtsLaunchOptions {
            paging_ack_timeout_ms: bsc_config.paging_retry.ack_timeout_ms,
            paging_max_retries: bsc_config.paging_retry.max_retries,
        },
    )?;
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
        hrpd_access_events,
        hrpd_traffic_events,
        commands: bts_commands,
        hrpd_forward_signaling,
        hrpd_traffic_assignments,
        hrpd_traffic_releases,
        hrpd_forward_traffic,
        power_control: bts_power_control,
        ..
    } = bts_handle;
    info!("Packet data transport: {:?}", pdsn_config.packet.transport);
    let lifecycle_sink: Option<Arc<dyn cdma_packet::session_lifecycle::SessionLifecycleSink>> =
        match pdsn_config.events_endpoint.as_deref() {
            Some(endpoint) => {
                let publisher = cdma_events::EventPublisher::spawn(
                    cdma_events::EventPublisherConfig::new(endpoint.to_string(), "pdsn-0"),
                )
                .map_err(|e| Error::from(format!("invalid pdsn.events_endpoint: {e}")))?;
                info!("PDSN packet-session events publishing to {endpoint}");
                Some(Arc::new(cdma_pdsn::events::PdsnLifecycleSink::new(
                    publisher,
                )))
            }
            None => None,
        };
    let packet_service = cdma_pdsn::build_packet_service_with_sink(&pdsn_config, lifecycle_sink)
        .map_err(Error::from)?;
    let _hrpd_pdsn_a11_addr = if bts_config.evdo.enabled {
        Some(spawn_hrpd_pdsn_a11_service(pdsn_config.clone(), packet_service.clone()).await?)
    } else {
        None
    };
    let hrpd_a9_config = if bts_config.evdo.enabled {
        Some(spawn_hrpd_pcf_a9_service(pcf_config.clone()).await?)
    } else {
        None
    };
    let an_service = spawn_nib_an_service(&bts_config, pdsn_config.events_endpoint.as_deref())?;
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
    if let Some((an_addr, air, uati)) = an_service {
        let color_code = bts_config.evdo.overhead.resolve()?.color_code;
        let derived_imsi_config = hrpd_derived_imsi_config_from_bts(&bts_config)?;
        spawn_hrpd_air_bridge(
            an_addr,
            air,
            uati,
            hrpd_access_events,
            hrpd_traffic_events,
            hrpd_forward_signaling,
            hrpd_traffic_assignments,
            hrpd_traffic_releases,
            hrpd_forward_traffic,
            hrpd_a9_config,
            Some(hlr_repo.clone()),
            color_code,
            derived_imsi_config,
        );
    }

    // Aggregated event bus (subscribed via gRPC ListenEvents; producers
    // publish via gRPC Publish — no in-process bus by design). The bus
    // owns its own HLR client per `events.json` and enriches events
    // before fan-out; nib just hands it the config.
    if let Some(cfg) = events_config.as_ref() {
        let addr = cfg.grpc_listen_addr;
        let bus_cfg = cdma_events::EventBusConfig {
            subscriber_queue_capacity: cfg.subscriber_queue_capacity,
        };
        let enricher = cdma_events::build_default_enricher(cfg)
            .await
            .map_err(|e| Error::from(format!("event bus HLR enricher: {e}")))?;
        let mut bus = cdma_events::EventBusServer::new(bus_cfg);
        if let Some(enricher) = enricher {
            bus = bus.with_enricher(enricher);
            info!(
                "Event bus HLR enrichment enabled via {:?}",
                cfg.hlr_endpoint
            );
        }
        tokio::spawn(async move {
            if let Err(err) = tonic::transport::Server::builder()
                .add_service(bus.into_service())
                .serve(addr)
                .await
            {
                log::error!("event bus gRPC server error: {err}");
            }
        });
        info!("Event bus gRPC service listening on {addr}");
    } else {
        info!(
            "Event bus disabled (no {} in {})",
            config::EVENTS_CONFIG_FILENAME,
            config_dir.display()
        );
    }

    // Packet data
    let packet_addr = pdsn_config.packet_grpc_listen_addr;
    let packet_endpoint = cdma_pdsn::packet_grpc_endpoint(packet_addr);
    let packet_service_for_server = packet_service.clone();
    let _packet_server = tokio::spawn(async move {
        if let Err(error) =
            cdma_pdsn::run_packet_grpc_server(packet_addr, (*packet_service_for_server).clone())
                .await
        {
            log::error!("packet gRPC server error: {error}");
        }
    });
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
    if msc_config.otasp.enabled {
        msc_runtime_config.bts_overhead = Some(bts_overhead_from_node_configs(&bts_config)?);
    }
    tokio::spawn(async move {
        info!("MSC management gRPC server on {msc_mgmt_addr}");
        let mut runtime = MscRuntime::new(msc_runtime_config);
        runtime.run_with_grpc(msc_mgmt_addr, &msc_a1_endpoint).await;
    });
    // BSC runtime and management state
    let tx_center_frequency_hz = bts_config
        .runtime
        .tx_freq_hz_override
        .unwrap_or_else(|| bts_config.channel.downlink_hz() as usize);
    let rx_center_frequency_hz = bts_config.channel.uplink_hz() as usize;
    // Resolved EV-DO carrier for the management plane (None when EV-DO is off).
    let evdo_carrier = evdo::resolve_evdo_config(
        &bts_config.evdo,
        bts_config.pilot_offset,
        bts_config.channel,
        bts_config.runtime.tx_sample_rate_hz,
        bts_config.runtime.tx_bandwidth_hz,
    )
    .ok()
    .flatten();
    let bsc_parts = cdma_bsc::bsc::build_bsc_launch_parts(cdma_bsc::bsc::BscLaunchInputs {
        pilot_offset: bts_config.pilot_offset,
        channel: bts_config.channel,
        tx_center_frequency_hz,
        rx_center_frequency_hz,
        evdo: evdo_carrier,
        overhead: overhead_params,
        timezone: bts_config.timezone.clone(),
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
        an_a21_addr: bsc_config.an_a21_addr,
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

/// Build the OTASP `BtsOverheadConfig` MSC needs for NAM assembly from
/// `bts.json`. SID/NID/MCC/IMSI_11_12 come from the BTS overhead block
/// (the same source the BTS broadcasts from) so a `*228` write matches
/// what the cell advertises.
fn bts_overhead_from_node_configs(
    bts_config: &BtsNodeConfig,
) -> Result<cdma_msc::BtsOverheadConfig, Error> {
    let derived_imsi_config = hrpd_derived_imsi_config_from_bts(bts_config)?;
    Ok(cdma_msc::BtsOverheadConfig {
        mcc: derived_imsi_config.mcc,
        imsi_11_12: derived_imsi_config.imsi_11_12,
        sid: bts_config.overhead.sid,
        nid: bts_config.overhead.nid,
        paging_channel_number: bts_config.runtime.downlink.paging.paging_channel_number as u16,
    })
}

fn hrpd_derived_imsi_config_from_bts(
    bts_config: &BtsNodeConfig,
) -> Result<HrpdDerivedImsiConfig, Error> {
    let esp = &bts_config
        .runtime
        .downlink
        .paging
        .message_defaults
        .extended_system_parameters;
    let mcc = cdma_common::paging::mcc_to_digits(esp.mcc)
        .ok_or_else(|| Error::from(format!("invalid BTS overhead MCC encoding {}", esp.mcc)))?;
    let imsi_11_12 =
        cdma_common::paging::imsi_11_12_to_digits(esp.imsi_11_12).ok_or_else(|| {
            Error::from(format!(
                "invalid BTS overhead IMSI_11_12 encoding {}",
                esp.imsi_11_12
            ))
        })?;
    Ok(HrpdDerivedImsiConfig { mcc, imsi_11_12 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_debug_log_filter_uses_targeted_profile() {
        let filter = apply_default_log_clamps("debug");
        let directives = filter.split(',').collect::<Vec<_>>();

        assert!(directives.contains(&DEFAULT_LOG_FILTER));
        assert!(directives.contains(&"cdma_an=debug"));
        assert!(directives.contains(&"cdma_packet=debug"));
        assert!(directives.contains(&"cdma_bts::receiver::hrpd::reverse_traffic_rake=debug"));
        assert!(!directives.contains(&"debug"));
        assert!(directives.contains(&"tonic=warn"));
    }

    #[test]
    fn explicit_log_targets_survive_default_clamps() {
        let filter = apply_default_log_clamps("cdma_bts::receiver=trace,debug");

        assert!(filter.contains("cdma_bts::receiver=trace"));
        assert!(filter.contains("cdma_an=debug"));
        assert!(!filter.split(',').any(|directive| directive == "debug"));
    }

    #[test]
    fn cli_accepts_named_bts_profile() {
        let cli = Cli::try_parse_from(["cdma-nib", "--bts-profile", "sprint"])
            .expect("parse BTS profile");

        assert_eq!(cli.bts_profile.as_deref(), Some("sprint"));
    }

    #[test]
    fn bts_profile_path_uses_config_directory() {
        let path = resolve_bts_profile_path(Path::new("site-config"), "sprint")
            .expect("resolve BTS profile");

        assert_eq!(path, Path::new("site-config/bts.sprint.json"));
    }

    #[test]
    fn bts_profile_name_rejects_paths() {
        let error = resolve_bts_profile_path(Path::new("config"), "../sprint")
            .expect_err("reject profile path");

        assert!(error.to_string().contains("invalid BTS profile"));
    }
}
