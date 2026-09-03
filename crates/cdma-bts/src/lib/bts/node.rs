//! Startup for the BTS component.
//!
//! One function brings up everything a BTS process owns — radio, PHY/MAC/LAC,
//! the Abis endpoint, the operations plane, and the fused HRPD Access Network
//! — so the standalone `cdma-bts` binary and the all-in-one launcher run the
//! same sequence.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use cdma_a8::{BearerEndpoint, HrpdA9ClientConfig};
use cdma_common::error::Error;
use cdma_hlr::repository::{GrpcHlrRepository, HlrRepository};
use log::{info, warn};

use crate::{
    lac,
    mac::Layer2MacRef,
    startup::{BTS_CONFIG_FILENAME, resolve_bts_profile_path},
};

use super::{
    Bts, BtsHandle, BtsLaunchOptions, BtsLaunchParts, BtsManagementState, BtsNodeConfig,
    RadioBuildOptions, apply_derived_channel_overhead, build_bts_launch_parts,
    build_radio_from_config, hrpd_an, load_radio_from_path, resolve_reverse_rx_plan,
    spawn_bts_management_service, spawn_configured_local_abis_endpoint,
    validate_page_chan_alignment,
};

/// Directory IQ captures are written to when `management.json` names none.
const DEFAULT_IQ_CAPTURE_DIR: &str = "capture-iq-wav";

/// Config-file selections a caller makes on the command line, overriding what
/// the config directory would otherwise supply.
#[derive(Debug, Default, Clone)]
pub struct BtsCliOverrides {
    /// Radio-only config JSON replacing the radio config `bts.json` references.
    pub radio_config: Option<PathBuf>,
    /// BTS config JSON replacing `<config-dir>/bts.json`.
    pub bts_config: Option<PathBuf>,
    /// Named BTS profile applied after the base config and its local override.
    pub bts_profile: Option<String>,
    /// Use a null radio that drops all TX samples and provides no RX.
    pub null_radio: bool,
}

/// Load the BTS config, start the node, and run it until it stops.
///
/// The caller owns logging setup. This only reads config and runs services.
pub async fn run_node(config_dir: &Path, overrides: BtsCliOverrides) -> Result<(), Error> {
    let bts_config_path = overrides
        .bts_config
        .clone()
        .unwrap_or_else(|| config_dir.join(BTS_CONFIG_FILENAME));
    let bts_profile_path = overrides
        .bts_profile
        .as_deref()
        .map(|profile| resolve_bts_profile_path(config_dir, profile))
        .transpose()?;
    let radio_override = overrides
        .radio_config
        .as_deref()
        .map(load_radio_from_path)
        .transpose()?;
    let bts_config = BtsNodeConfig::load_from_path_with_overrides(
        &bts_config_path,
        bts_profile_path.as_deref(),
        radio_override,
    )?;

    info!("Loading BTS config from {}", bts_config_path.display());
    if let (Some(profile), Some(path)) = (&overrides.bts_profile, &bts_profile_path) {
        info!("BTS profile {profile} applied from {}", path.display());
    }
    if overrides.radio_config.is_some() {
        info!("Radio config overridden from CLI");
    }
    if cfg!(debug_assertions) {
        warn!("WARNING: cdma-bts is running in a Rust debug build.");
        warn!("Timing, throughput, and RF behavior will not match `--release`.");
        warn!("Rebuild with --release for real-time work.");
    }

    let mut hrpd_a9_config = None;
    let mut hrpd_hlr_repo = None;
    if bts_config.evdo.enabled {
        hrpd_a9_config = Some(hrpd_a9_client_config(&bts_config)?);
        if let Some(endpoint) = bts_config.hlr_endpoint.clone() {
            // Lazy so the radio never waits on the subscriber database. The AN
            // consults the HLR only to map an ESN/MEID to an MN ID at packet
            // session setup, and falls back to a derived MN ID when the lookup
            // fails, so an unreachable HLR must not hold up 1x service.
            match GrpcHlrRepository::connect_lazy(&endpoint) {
                Ok(repo) => {
                    info!("HRPD AN: HLR client targeting {endpoint}");
                    hrpd_hlr_repo = Some(Arc::new(repo) as Arc<dyn HlrRepository>);
                }
                Err(e) => {
                    warn!("HRPD AN: invalid hlr_endpoint {endpoint}: {e}; deriving MN IDs locally")
                }
            }
        }
    }

    let iq_capture_dir = iq_capture_dir(&bts_config);
    let node = start_bts_node(
        bts_config,
        BtsNodeOptions {
            null_radio: overrides.null_radio,
            iq_capture_dir,
            hrpd_a9_config,
            hrpd_hlr_repo,
        },
    )
    .await?;
    info!(
        "BTS running: abis={} management={}",
        node.abis_bind_addr, node.management_addr
    );

    node.run().await
}

/// The AN's A9 signaling target and A8 bearer binding, read from `evdo.a9`.
fn hrpd_a9_client_config(bts_config: &BtsNodeConfig) -> Result<HrpdA9ClientConfig, Error> {
    let a9 = bts_config.evdo.a9.as_ref().ok_or_else(|| {
        Error::from(
            "evdo.a9 is required when EVDO is enabled: it addresses this AN's own A8 bearer",
        )
    })?;
    let ipv4 = |addr: &SocketAddr, field: &str| -> Result<[u8; 4], Error> {
        match addr.ip() {
            std::net::IpAddr::V4(v4) => Ok(v4.octets()),
            std::net::IpAddr::V6(_) => Err(Error::from(format!("evdo.a9.{field} must be IPv4"))),
        }
    };
    Ok(HrpdA9ClientConfig {
        pcf_addr: a9.pcf_addr,
        a8_peer_ipv4: ipv4(&a9.pcf_a8_addr, "pcf_a8_addr")?,
        an_a8_bearer: cdma_a8::BearerTransportConfig {
            mode: cdma_a8::BearerTransportMode::UdpEncapsulatedGre,
            udp_bind_addr: Some(a9.a8_bind_addr),
            udp_peer_addr: Some(a9.pcf_a8_addr),
        },
        an_a8_endpoint: BearerEndpoint::new(
            ipv4(&a9.a8_bind_addr, "a8_bind_addr")?,
            ipv4(&a9.pcf_a8_addr, "pcf_a8_addr")?,
        ),
    })
}

/// Directory IQ captures are written to.
fn iq_capture_dir(bts_config: &BtsNodeConfig) -> PathBuf {
    bts_config
        .iq_capture_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_IQ_CAPTURE_DIR))
}

/// Per-process inputs the BTS cannot read out of `bts.json`.
pub struct BtsNodeOptions {
    /// Replace the configured radio with one that drops all TX samples and
    /// paces a silent RX.
    pub null_radio: bool,
    /// Directory IQ captures requested over the management plane are written to.
    pub iq_capture_dir: PathBuf,
    /// A9/A8 client configuration for the fused AN. Without it the AN runs but
    /// cannot open packet-data bearers toward the PCF.
    pub hrpd_a9_config: Option<HrpdA9ClientConfig>,
    /// HLR the AN resolves HRPD subscriber identities against.
    pub hrpd_hlr_repo: Option<Arc<dyn HlrRepository>>,
}

/// A started BTS, ready to run its TX/RX loop.
pub struct BtsNode {
    bts: Bts,
    lac_layer: lac::Layer2LacRef,
    mac_layer: Layer2MacRef,
    /// Abis signaling listener the BSC connects to.
    pub abis_bind_addr: SocketAddr,
    /// Operations-plane listener the BSC enrolls against.
    pub management_addr: SocketAddr,
    /// The fused HRPD Access Network, when EV-DO is enabled.
    pub hrpd_an: Option<hrpd_an::HrpdAn>,
}

impl BtsNode {
    /// Start the LAC and MAC threads, then run the TX/RX loop until it stops.
    pub async fn run(self) -> Result<(), Error> {
        let lac_layer = self.lac_layer;
        thread::spawn(move || {
            if let Err(e) = lac_layer.start() {
                log::error!("LAC thread exited with error: {e}");
            }
        });

        let mac_layer = self.mac_layer;
        thread::spawn(move || {
            if let Err(e) = mac_layer.start() {
                log::error!("MAC thread exited with error: {e}");
            }
        });

        self.bts.start().await
    }
}

/// Build the radio and the BTS, bring up Abis and the operations plane, and
/// start the fused HRPD AN. The returned node still has to be `run`.
///
/// The operations plane is listening before this returns, which is what lets a
/// BSC enroll and then bring up Abis without any ordering negotiation.
pub async fn start_bts_node(
    mut bts_config: BtsNodeConfig,
    options: BtsNodeOptions,
) -> Result<BtsNode, Error> {
    apply_derived_channel_overhead(&mut bts_config);
    validate_page_chan_alignment(
        bts_config.overhead.page_chan,
        bts_config.runtime.downlink.paging.paging_channel_number,
    )?;

    let reverse_rx_plan = resolve_reverse_rx_plan(&bts_config)?;
    let radio = build_radio_from_config(
        &bts_config.radio,
        reverse_rx_plan.center_frequency_hz,
        RadioBuildOptions {
            null_radio: options.null_radio,
            configure_rx: reverse_rx_plan.configure_rx,
            tx_sample_rate_hz: bts_config.runtime.tx_sample_rate_hz,
            rx_sample_rate_hz: reverse_rx_plan.sample_rate_hz,
            rx_bandwidth_hz: reverse_rx_plan.bandwidth_hz,
            realtime: bts_config.runtime.realtime.clone(),
        },
    )?;

    let BtsLaunchParts {
        bts,
        handle,
        resource_controller,
        lac_layer,
        mac_layer,
        reverse_bearer_rx,
        traffic_ack_seq_rx,
        paging_state,
        pch_transmit_tx,
        overhead: _,
        paging_settings: _,
    } = build_bts_launch_parts(
        bts_config.clone(),
        radio,
        BtsLaunchOptions {
            paging_ack_timeout_ms: bts_config.paging_retry.ack_timeout_ms,
            paging_max_retries: bts_config.paging_retry.max_retries,
        },
    )?;
    let BtsHandle {
        tx_metrics,
        rx_metrics,
        config: bts_runtime_config,
        access_events,
        hrpd_access_events,
        hrpd_traffic_events,
        commands,
        hrpd_forward_signaling,
        hrpd_traffic_assignments,
        hrpd_traffic_releases,
        hrpd_forward_traffic,
        power_control,
        ..
    } = handle;

    let hrpd_an = hrpd_an::spawn_hrpd_an(
        &bts_config,
        bts_config.events_endpoint.as_deref(),
        options.hrpd_a9_config,
        options.hrpd_hlr_repo,
        hrpd_an::hrpd_derived_imsi_config(&bts_config)?,
        hrpd_an::HrpdAirChannels {
            access_events: hrpd_access_events,
            traffic_events: hrpd_traffic_events,
            forward_signaling: hrpd_forward_signaling,
            traffic_assignments: hrpd_traffic_assignments,
            traffic_releases: hrpd_traffic_releases,
            forward_traffic: hrpd_forward_traffic,
        },
    )
    .await?;
    if let Some(an) = &hrpd_an {
        info!("HRPD AN session service listening on {}", an.grpc_addr);
    }

    let abis = spawn_configured_local_abis_endpoint(
        &bts_config,
        resource_controller,
        reverse_bearer_rx,
        traffic_ack_seq_rx,
        paging_state,
        access_events,
    )
    .await?;
    let abis_bind_addr = abis.bind_addr;
    info!("Abis TCP: BTS listener={abis_bind_addr}");

    let mut management_state = BtsManagementState::from_node_config(
        &bts_config,
        bts_runtime_config,
        tx_metrics,
        rx_metrics,
        commands,
        power_control,
        options.iq_capture_dir,
        pch_transmit_tx,
    )?;
    management_state.bearer = Some(abis.bearer);
    management_state.abis_connected = Some(abis.connected);
    management_state.an_grpc_addr = hrpd_an.as_ref().map(|an| {
        bts_config
            .evdo
            .an_grpc_advertise_addr
            .unwrap_or(an.grpc_addr)
            .to_string()
    });
    let management_addr =
        spawn_bts_management_service(bts_config.management.bind_addr, Arc::new(management_state))
            .await?;
    info!("BTS management gRPC service listening on {management_addr}");

    Ok(BtsNode {
        bts,
        lac_layer,
        mac_layer,
        abis_bind_addr,
        management_addr,
        hrpd_an,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The BSC builds the Extended Channel Assignment Message from the overhead
    /// it is served over the operations plane, and sets `FREQ_INCL` whenever it
    /// has a frequency. A config that leaves the fields unset has to reach the
    /// operations plane already resolved, or the mobile is assigned a channel
    /// the ECAM never names and rejects the assignment.
    #[test]
    fn node_startup_resolves_overhead_frequency_from_the_channel_plan() {
        let mut config = BtsNodeConfig::default();
        config.channel.cdma_channel = 691;
        config.overhead.cdma_freq = None;
        config.overhead.ext_cdma_freq = None;
        config.overhead.band_class = None;

        apply_derived_channel_overhead(&mut config);

        assert_eq!(config.overhead.cdma_freq, Some(691));
        assert_eq!(config.overhead.ext_cdma_freq, Some(691));
        assert_eq!(config.overhead.band_class, Some(0));
    }

    #[test]
    fn explicitly_configured_overhead_frequency_is_left_alone() {
        let mut config = BtsNodeConfig::default();
        config.channel.cdma_channel = 691;
        config.overhead.cdma_freq = Some(100);
        config.overhead.band_class = Some(1);

        apply_derived_channel_overhead(&mut config);

        assert_eq!(config.overhead.cdma_freq, Some(100));
        assert_eq!(config.overhead.band_class, Some(1));
    }
}
