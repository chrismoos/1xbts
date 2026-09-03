//! Attaching the BSC to each configured BTS.
//!
//! The BSC initiates: it dials the peer's OAM gRPC channel, enrolls to learn
//! the cell identity and radio parameters, then brings up Abis for the cell
//! that those parameters name. Ordering therefore needs no negotiation. Every
//! peer is retried forever with backoff, so elements can start in any order
//! and a BTS restarted with an edited configuration is picked up on
//! re-enrollment.

use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use tokio::sync::mpsc;

use cdma_common::band_class::{BandClass, ChannelPlan};
use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use cdma_common::overhead::OverheadParameters;
use cdma_common::timezone::{TimezoneConfig, TimezoneSource};

use crate::abis_edge::network::{NetworkBtsControlClient, NetworkClientConfig};
use crate::config::BtsPeerConfig;
use crate::grpc::bts_management_proto::{
    CellRequest, EnrollRequest, EnrollResponse,
    bts_management_service_client::BtsManagementServiceClient,
};

use super::bts_registry::{
    AccessCellId, BtsCellParams, BtsOamClient, BtsRegistry, IMSI_11_12_UNSPECIFIED, MCC_UNSPECIFIED,
};

/// First delay after a failed or dropped attach.
const RETRY_MIN: Duration = Duration::from_secs(1);
/// Ceiling the retry delay backs off to.
const RETRY_MAX: Duration = Duration::from_secs(30);
/// How often an attached peer is probed over the operations plane. Abis
/// carries no traffic on an idle cell, so this is what notices a BTS that
/// disappeared without closing its connection.
const LIVENESS_POLL: Duration = Duration::from_secs(15);
/// Idle time before the OAM channel sends a TCP/HTTP2 keepalive probe.
const OAM_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// An unanswered keepalive probe closes the OAM channel after this long.
const OAM_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Ceiling on any single OAM request, so a peer that stops answering fails
/// the request instead of parking it in the send buffer until the OS gives
/// up on retransmits.
const OAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Start one attach task per configured peer. Each runs until the process
/// exits.
pub fn spawn_bts_attach(
    peers: Vec<BtsPeerConfig>,
    bts: Arc<BtsRegistry>,
    access_event_tx: mpsc::UnboundedSender<AccessChannelEvent>,
    cell_detach_tx: mpsc::UnboundedSender<AccessCellId>,
) {
    for peer in peers {
        let bts = bts.clone();
        let access_event_tx = access_event_tx.clone();
        let cell_detach_tx = cell_detach_tx.clone();
        tokio::spawn(async move {
            attach_peer_forever(peer, bts, access_event_tx, cell_detach_tx).await
        });
    }
}

async fn attach_peer_forever(
    peer: BtsPeerConfig,
    bts: Arc<BtsRegistry>,
    access_event_tx: mpsc::UnboundedSender<AccessChannelEvent>,
    cell_detach_tx: mpsc::UnboundedSender<AccessCellId>,
) {
    let mut retry = RETRY_MIN;
    // The bearer socket outlives any single attach. Rebinding the address
    // fails while a packet-session task still holds the previous socket, and
    // the socket carries no per-connection state, so a re-attach of the same
    // cell reuses it.
    let mut bearer_cache: Option<(AccessCellId, BearerHandle)> = None;
    loop {
        match attach_peer_once(&peer, &bts, &access_event_tx, &mut bearer_cache).await {
            Ok(attached) => {
                retry = RETRY_MIN;
                bts.set_peer_in_service(&peer.oam_endpoint, attached.cell);
                info!(
                    "BSC: cell {:?} in service (oam={} abis={})",
                    attached.cell, peer.oam_endpoint, peer.abis_addr
                );
                let mut link_down = attached.link_down;
                let mut oam = attached.oam;
                // A BTS that loses power sends no FIN, so waiting only on the
                // Abis link can leave a dead cell in service indefinitely.
                // Poll the operations plane alongside it.
                let reason = loop {
                    tokio::select! {
                        // `changed()` errors only if the client was dropped,
                        // which also means the link is gone.
                        _ = link_down.changed() => break "Abis link closed",
                        _ = tokio::time::sleep(LIVENESS_POLL) => {
                            let probe = oam.get_bts_status(CellRequest { cell: None, peer_id: None });
                            match tokio::time::timeout(OAM_REQUEST_TIMEOUT, probe).await {
                                Ok(Ok(_)) => {}
                                Ok(Err(e)) => {
                                    warn!(
                                        "BSC: cell {:?} stopped answering the operations plane: {e}",
                                        attached.cell
                                    );
                                    break "operations plane unreachable";
                                }
                                Err(_) => {
                                    warn!(
                                        "BSC: cell {:?} operations-plane probe timed out",
                                        attached.cell
                                    );
                                    break "operations plane unreachable";
                                }
                            }
                        }
                    }
                };
                if let Some(entry) = bts.get(attached.cell) {
                    entry.set_detached(reason);
                }
                bts.set_peer_offline(&peer.oam_endpoint, reason);
                // The run loop releases every call, traffic channel, and
                // packet session bound to the departed cell. A send fails
                // only when the run loop itself is gone at shutdown.
                let _ = cell_detach_tx.send(attached.cell);
                warn!(
                    "BSC: cell {:?} detached ({reason}) — re-enrolling",
                    attached.cell
                );
            }
            Err(e) => {
                bts.set_peer_offline(&peer.oam_endpoint, e.to_string());
                warn!("BSC: attach to BTS at {} failed: {e}", peer.oam_endpoint);
                retry = (retry * 2).min(RETRY_MAX);
            }
        }
        tokio::time::sleep(retry).await;
    }
}

struct AttachedPeer {
    cell: AccessCellId,
    link_down: tokio::sync::watch::Receiver<bool>,
    /// Kept so the attach task can probe the peer while the link is quiet.
    oam: BtsOamClient,
}

type BearerHandle = Arc<cdma_abis::bearer_transport::BearerTransport>;

async fn attach_peer_once(
    peer: &BtsPeerConfig,
    bts: &Arc<BtsRegistry>,
    access_event_tx: &mpsc::UnboundedSender<AccessChannelEvent>,
    bearer_cache: &mut Option<(AccessCellId, BearerHandle)>,
) -> Result<AttachedPeer, Error> {
    let channel = tonic::transport::Endpoint::from_shared(peer.oam_endpoint.clone())
        .map_err(|e| format!("OAM endpoint: {e}"))?
        .tcp_keepalive(Some(OAM_KEEPALIVE_INTERVAL))
        .http2_keep_alive_interval(OAM_KEEPALIVE_INTERVAL)
        .keep_alive_timeout(OAM_KEEPALIVE_TIMEOUT)
        .keep_alive_while_idle(true)
        .timeout(OAM_REQUEST_TIMEOUT)
        .connect()
        .await
        .map_err(|e| format!("OAM connect: {e}"))?;
    let mut oam: BtsOamClient = BtsManagementServiceClient::new(channel);
    let response = oam
        .enroll(EnrollRequest {})
        .await
        .map_err(|e| format!("Enroll: {e}"))?
        .into_inner();
    let mut params = cell_params_from_enrollment(&response)?;
    params.an_grpc_addr = resolve_an_grpc_addr(params.an_grpc_addr.take(), &peer.oam_endpoint);
    let cell = params.cell;
    // tonic clients are cheap clones over one connection.
    let oam_for_liveness = oam.clone();
    let entry = bts
        .enroll(params.clone(), Some(oam), Some(peer.oam_endpoint.as_str()))
        .map_err(|conflict| format!("enrollment refused for cell {cell}: {conflict}"))?;

    let bearer_config = cdma_abis::bearer_transport::BearerTransportConfig {
        bind_addr: peer.bearer_bind_addr,
        remote_addr: peer.bearer_remote_addr,
        bts_id: cell.cell as u32,
        cell_id: cell.sector as u32,
    };
    let bearer = match bearer_cache.take() {
        Some((cached_cell, bearer)) if cached_cell == cell => bearer,
        // A different cell id means new bts_id/cell_id framing. Release the
        // old socket before binding the replacement.
        stale => {
            drop(stale);
            Arc::new(
                cdma_abis::bearer_transport::BearerTransport::new(&bearer_config)
                    .map_err(|e| format!("bearer transport: {e}"))?,
            )
        }
    };
    bearer_cache.replace((cell, bearer.clone()));
    let net_config = NetworkClientConfig {
        cell_id: cdma_abis::control::typed::CellId {
            cell: cell.cell,
            sector: cell.sector,
        },
        mscid: params.overhead.sid as u32,
        pilot_pn: params.pilot_offset as u16,
        auth_mode: params.overhead.auth_mode,
        p_rev_in_use: params.overhead.p_rev,
        market_id: params.overhead.sid,
        generating_entity_id: params.overhead.base_id,
    };
    let control = NetworkBtsControlClient::connect_with_bearer_and_access(
        peer.abis_addr,
        net_config,
        bearer,
        access_event_tx.clone(),
    )
    .await
    .map_err(|e| {
        entry.set_abis_down(format!("Abis connect: {e}"));
        format!("Abis connect: {e}")
    })?;
    let link_down = control.link_down();
    entry.set_control(Arc::new(control));
    Ok(AttachedPeer {
        cell,
        link_down,
        oam: oam_for_liveness,
    })
}

/// Make the AN address a peer advertised dialable from this host.
///
/// A loopback or unspecified host only works on the AN's own machine. The
/// host that answered enrollment is the machine the AN runs on, so its host
/// replaces the undialable one while the AN's port is kept.
fn resolve_an_grpc_addr(advertised: Option<String>, oam_endpoint: &str) -> Option<String> {
    let advertised = advertised?;
    let addr: std::net::SocketAddr = match advertised.parse() {
        Ok(addr) => addr,
        Err(_) => return Some(advertised),
    };
    if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
        return Some(advertised);
    }
    match endpoint_host(oam_endpoint) {
        Some(host) => Some(format!("{host}:{}", addr.port())),
        None => Some(advertised),
    }
}

/// Host portion of a gRPC endpoint like `http://10.0.0.2:17024`.
fn endpoint_host(endpoint: &str) -> Option<&str> {
    let rest = endpoint
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(endpoint);
    let authority = rest.split('/').next()?;
    let host = authority
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority);
    if host.is_empty() { None } else { Some(host) }
}

/// Map a BTS enrollment onto the parameters the BSC keys call control on.
fn cell_params_from_enrollment(response: &EnrollResponse) -> Result<BtsCellParams, Error> {
    let cell = response.cell.ok_or("enrollment carried no cell identity")?;
    let config = response
        .config
        .as_ref()
        .ok_or("enrollment carried no radio configuration")?;
    let overhead_config = config
        .overhead
        .as_ref()
        .ok_or("enrollment carried no overhead configuration")?;

    let band_class = band_class_from_label(&config.band_class)
        .ok_or_else(|| format!("unknown band class {:?}", config.band_class))?;

    let overhead = OverheadParameters {
        sid: overhead_config.sid as u16,
        nid: overhead_config.nid as u16,
        base_id: overhead_config.base_id as u16,
        reg_zone: overhead_config.reg_zone as u16,
        total_zones: overhead_config.total_zones as u8,
        zone_timer: overhead_config.zone_timer as u8,
        max_slot_cycle_index: overhead_config.max_slot_cycle_index as u8,
        page_chan: overhead_config.page_chan as u8,
        config_seq: overhead_config.config_seq as u8,
        acc_config_seq: overhead_config.acc_config_seq as u8,
        power_up_reg: overhead_config.power_up_reg,
        parameter_reg: overhead_config.parameter_reg,
        auth_mode: overhead_config.auth_mode as u8,
        p_rev: overhead_config.p_rev as u8,
        min_p_rev: overhead_config.min_p_rev as u8,
        lp_sec: overhead_config.lp_sec as u8,
        ltm_off: overhead_config.ltm_off as i8,
        daylt: overhead_config.daylt as u8,
        cdma_freq: overhead_config.cdma_freq.map(|v| v as u16),
        ext_cdma_freq: overhead_config.ext_cdma_freq.map(|v| v as u16),
        band_class: overhead_config.band_class_override.map(|v| v as u8),
        ..OverheadParameters::default()
    };

    let sector = u8::try_from(cell.sector)
        .ok()
        .filter(|sector| *sector != 0)
        .ok_or_else(|| format!("BTS reported unusable sector {}", cell.sector))?;

    Ok(BtsCellParams {
        cell: AccessCellId {
            cell: cell.cell as u16,
            sector,
        },
        overhead,
        pilot_offset: config.pilot_offset as usize,
        rx_reference_dbm: response.rx_reference_dbm,
        channel: ChannelPlan::new(
            band_class,
            config.band_subclass as u8,
            config.cdma_channel as u16,
        ),
        tx_center_frequency_hz: config.tx_center_frequency_hz as usize,
        rx_center_frequency_hz: config.rx_center_frequency_hz as usize,
        an_grpc_addr: response.an_grpc_addr.clone(),
        paging_retry: response
            .paging_retry
            .as_ref()
            .map(|retry| crate::config::PagingRetryConfig {
                ack_timeout_ms: u64::from(retry.ack_timeout_ms),
                max_retries: retry.max_retries,
            })
            .unwrap_or_default(),
        evdo: config.evdo.clone(),
        timezone: timezone_from_proto(config.timezone.as_ref()),
        mcc: cdma_common::paging::mcc_from_digits(&overhead_config.mcc_digits)
            .unwrap_or(MCC_UNSPECIFIED),
        imsi_11_12: cdma_common::paging::imsi_11_12_from_digits(&overhead_config.imsi_11_12_digits)
            .unwrap_or(IMSI_11_12_UNSPECIFIED),
    })
}

/// `BandClass` serializes as its snake-case variant name, so the "BC0" label
/// the management API renders round-trips through serde.
fn band_class_from_label(label: &str) -> Option<BandClass> {
    serde_json::from_value(serde_json::Value::String(label.to_ascii_lowercase())).ok()
}

fn timezone_from_proto(timezone: Option<&crate::grpc::proto::TimezoneConfig>) -> TimezoneConfig {
    let Some(timezone) = timezone else {
        return TimezoneConfig::default();
    };
    let source = match timezone.source.as_str() {
        "system" => TimezoneSource::System,
        "user" => match timezone.tz.clone() {
            Some(tz) => TimezoneSource::User { tz },
            None => TimezoneSource::Overhead,
        },
        _ => TimezoneSource::Overhead,
    };
    TimezoneConfig { source }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_band_class_label_round_trips() {
        assert_eq!(band_class_from_label("BC0"), Some(BandClass::Bc0));
        assert_eq!(band_class_from_label("BC10"), Some(BandClass::Bc10));
        assert_eq!(band_class_from_label("BC99"), None);
    }

    #[test]
    fn a_loopback_an_address_takes_the_enrolling_peers_host() {
        assert_eq!(
            resolve_an_grpc_addr(Some("127.0.0.1:17030".into()), "http://10.0.0.2:17024"),
            Some("10.0.0.2:17030".into())
        );
        assert_eq!(
            resolve_an_grpc_addr(Some("0.0.0.0:17030".into()), "http://bts-1:17024"),
            Some("bts-1:17030".into())
        );
        assert_eq!(
            resolve_an_grpc_addr(Some("10.0.0.5:17030".into()), "http://10.0.0.2:17024"),
            Some("10.0.0.5:17030".into())
        );
        assert_eq!(resolve_an_grpc_addr(None, "http://10.0.0.2:17024"), None);
    }

    #[test]
    fn enrollment_without_a_cell_identity_is_rejected() {
        let err = cell_params_from_enrollment(&EnrollResponse::default())
            .expect_err("cell identity is required");
        assert!(err.to_string().contains("cell identity"));
    }
}
