use std::pin::Pin;
use std::sync::Arc;

use cdma_rpc_error::{Domain, ErrorReason};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

const FEMTO: Domain = Domain("femto");

use cdma_cwmp::digest::DigestParams;

use crate::cell::FemtoCellState;
use crate::config::AcsBackgroundSyncConfig;

const STATUS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
const MINIMUM_SYNC_DURATION: std::time::Duration = std::time::Duration::from_secs(1);

pub mod grpc {
    pub mod bsc {
        pub mod v1 {
            tonic::include_proto!("bsc.v1");
        }
    }

    pub mod bsc_management {
        pub mod v1 {
            tonic::include_proto!("bsc_management.v1");
        }
    }

    pub mod bts_management {
        pub mod v1 {
            tonic::include_proto!("bts_management.v1");
        }
    }
    pub mod femto_management {
        pub mod v1 {
            tonic::include_proto!("femto_management.v1");
        }
    }

    pub mod management {
        pub mod v1 {
            tonic::include_proto!("management.v1");
        }
    }
}

use bts_management::bts_management_service_server::{
    BtsManagementService, BtsManagementServiceServer,
};
use facade::management_facade_service_server::{
    ManagementFacadeService, ManagementFacadeServiceServer,
};
use femto_management::femto_management_service_server::{
    FemtoManagementService, FemtoManagementServiceServer,
};
use grpc::bsc::v1 as bsc;
use grpc::bsc_management::v1 as management;
use grpc::bts_management::v1 as bts_management;
use grpc::femto_management::v1 as femto_management;
use grpc::management::v1 as facade;
use management::bsc_management_service_server::{BscManagementService, BscManagementServiceServer};

#[derive(Clone)]
pub struct FemtoCellManagementService {
    state: Arc<FemtoCellState>,
    fap_registry: crate::fap::SharedFapRegistry,
    acs_credentials: Arc<std::sync::RwLock<Vec<crate::config::CpeCredential>>>,
    acs_term_auth_psk: Arc<std::sync::RwLock<Option<String>>>,
    acs_a11_provisioning: Arc<std::sync::RwLock<Option<crate::config::A11Provisioning>>>,
    served_cells: crate::base_station::SharedServedCells,
    sip_users: crate::sip::SharedSipUsers,
}

impl FemtoCellManagementService {
    pub(crate) fn new(
        state: Arc<FemtoCellState>,
        fap_registry: crate::fap::SharedFapRegistry,
        acs_credentials: Arc<std::sync::RwLock<Vec<crate::config::CpeCredential>>>,
        acs_term_auth_psk: Arc<std::sync::RwLock<Option<String>>>,
        acs_a11_provisioning: Arc<std::sync::RwLock<Option<crate::config::A11Provisioning>>>,
        served_cells: crate::base_station::SharedServedCells,
        sip_users: crate::sip::SharedSipUsers,
    ) -> Self {
        Self {
            state,
            fap_registry,
            acs_credentials,
            acs_term_auth_psk,
            acs_a11_provisioning,
            served_cells,
            sip_users,
        }
    }
    pub fn into_server(self) -> FemtoManagementServiceServer<Self> {
        FemtoManagementServiceServer::new(self)
    }

    fn selected(
        &self,
        request: &bts_management::CellRequest,
    ) -> Result<crate::cell::FemtoCellSnapshot, Status> {
        let cells = self.state.cells();
        if let Some(peer_id) = request.peer_id.as_deref() {
            return cells
                .into_iter()
                .find(|cell| cell.feid == peer_id)
                .ok_or_else(|| {
                    FEMTO
                        .not_found(
                            ErrorReason::FemtocellNotFound,
                            format!("Femtocell {peer_id} is not known"),
                        )
                        .with("femtocell", peer_id)
                        .status()
                });
        }
        if let Some(wanted) = &request.cell {
            return cells
                .into_iter()
                .find(|cell| {
                    u32::from(cell.cell_id) == wanted.cell
                        && u32::from(cell.sector) == wanted.sector
                })
                .ok_or_else(|| {
                    FEMTO
                        .not_found(
                            ErrorReason::FemtocellNotFound,
                            format!("No femtocell serves cell {}/{}", wanted.cell, wanted.sector),
                        )
                        .with("cell", format!("{}/{}", wanted.cell, wanted.sector))
                        .status()
                });
        }
        if cells.len() == 1 {
            return Ok(cells.into_iter().next().expect("one cell"));
        }
        Err(FEMTO
            .error(
                tonic::Code::InvalidArgument,
                ErrorReason::FemtocellRequired,
                "More than one femtocell is configured, so choose one",
            )
            .status())
    }

    /// 5.x Airave firmware serves an unauthenticated `GET /acscall` on port 80.
    /// 2.x firmware instead advertises its own `ConnectionRequestURL` in the
    /// Inform and answers `401 WWW-Authenticate: Digest` there.
    pub(crate) async fn request_connection(
        cell: &crate::cell::FemtoCellSnapshot,
    ) -> Result<(), Status> {
        let peer = cell.inner_peer_address.ok_or_else(|| {
            FEMTO
                .precondition(
                    ErrorReason::FemtocellNoManagementPath,
                    "The femtocell has no management tunnel yet",
                )
                .status()
        })?;

        let (port, path, host_header) = cell
            .connection_request_url
            .as_deref()
            .and_then(parse_connection_request_url)
            .unwrap_or_else(|| (80, "/acscall".to_string(), peer.to_string()));

        let first = http_connection_request(peer, port, &path, &host_header, None).await?;
        if first.is_success() {
            return Ok(());
        }
        if first.status_code != 401 {
            return Err(femtocell_unreachable(format!(
                "The femtocell refused the connection request (HTTP {})",
                first.status_code
            )));
        }

        let Some(creds) = cell.connection_request_credentials.as_ref() else {
            return Err(femtocell_auth_failed(
                "The femtocell wants credentials, but its profile has none",
            ));
        };
        let challenge_header = first.www_authenticate.ok_or_else(|| {
            femtocell_auth_failed("The femtocell asked for credentials without a Digest challenge")
        })?;
        let challenge = cdma_cwmp::digest::parse_challenge(&challenge_header);
        let (Some(realm), Some(nonce)) = (challenge.realm.as_deref(), challenge.nonce.as_deref())
        else {
            return Err(femtocell_auth_failed(
                "The femtocell's Digest challenge has no realm or nonce",
            ));
        };
        let cnonce = connection_request_cnonce();
        let uses_qop = challenge.qop.is_some();
        let authorization = cdma_cwmp::digest::digest_authorization(&DigestParams {
            username: &creds.username,
            password: &creds.password,
            realm,
            nonce,
            method: "GET",
            uri: &path,
            qop: challenge.qop.as_deref(),
            cnonce: uses_qop.then_some(cnonce.as_str()),
            nc: uses_qop.then_some("00000001"),
            opaque: challenge.opaque.as_deref(),
        });

        let authed =
            http_connection_request(peer, port, &path, &host_header, Some(&authorization)).await?;
        if authed.is_success() {
            return Ok(());
        }
        Err(femtocell_auth_failed(format!(
            "The femtocell refused the credentials (HTTP {})",
            authed.status_code
        )))
    }
}

struct ConnectionRequestResponse {
    status_code: u16,
    www_authenticate: Option<String>,
}

impl ConnectionRequestResponse {
    /// TR-069 CPEs answer a valid connection request with 200. Some legacy
    /// stacks reply 204, so treat any 2xx as success.
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }
}

fn parse_connection_request_url(url: &str) -> Option<(u16, String, String)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("HTTP://"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let authority = authority.split('@').next_back().unwrap_or(authority);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (authority, 80),
    };
    if host.is_empty() {
        return None;
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    Some((port, path, authority.to_string()))
}

fn connection_request_cnonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{nanos:032x}")
}

async fn http_connection_request(
    peer: std::net::IpAddr,
    port: u16,
    path: &str,
    host_header: &str,
    authorization: Option<&str>,
) -> Result<ConnectionRequestResponse, Status> {
    let address = std::net::SocketAddr::new(peer, port);
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        TcpStream::connect(address),
    )
    .await
    .map_err(|_| femtocell_timeout())?
    .map_err(|error| {
        femtocell_unreachable(format!("The femtocell could not be reached: {error}"))
    })?;

    let mut request =
        format!("GET {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n");
    if let Some(authorization) = authorization {
        request.push_str(&format!("Authorization: {authorization}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| {
            femtocell_unreachable(format!("The femtocell could not be reached: {error}"))
        })?;

    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 512];
    loop {
        let read = tokio::time::timeout(std::time::Duration::from_secs(3), stream.read(&mut chunk))
            .await
            .map_err(|_| femtocell_timeout())?
            .map_err(|error| {
                femtocell_unreachable(format!("The femtocell could not be reached: {error}"))
            })?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") || buffer.len() > 8192 {
            break;
        }
    }

    let text = String::from_utf8_lossy(&buffer);
    let status_code = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| {
            femtocell_unreachable("The femtocell sent a response that could not be read")
        })?;
    let www_authenticate = text
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("WWW-Authenticate"))
        })
        .map(|(_, value)| value.trim().to_string());
    Ok(ConnectionRequestResponse {
        status_code,
        www_authenticate,
    })
}

pub fn spawn_background_sync(
    config: AcsBackgroundSyncConfig,
    state: Arc<FemtoCellState>,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.enabled {
        log::info!("ACS background sync disabled");
        return None;
    }

    Some(tokio::spawn(async move {
        let startup_delay = std::time::Duration::from_secs(config.startup_delay_seconds);
        let interval =
            std::time::Duration::from_secs(config.interval_seconds).max(MINIMUM_SYNC_DURATION);
        let timeout = std::time::Duration::from_secs(config.per_fap_timeout_seconds)
            .max(MINIMUM_SYNC_DURATION);
        tokio::time::sleep(startup_delay).await;

        loop {
            let mut cells = state.cells();
            cells.sort_by(|left, right| left.feid.cmp(&right.feid));
            log::info!(
                "ACS background sync starting serial refresh of {} configured FAPs",
                cells.len()
            );
            for cell in cells {
                let feid = cell.feid.clone();
                let initial_generation = state
                    .status_readback_state(&feid)
                    .map_or(0, |(generation, _)| generation);
                let refresh = async {
                    FemtoCellManagementService::request_connection(&cell).await?;
                    loop {
                        if let Some((generation, succeeded)) = state.status_readback_state(&feid)
                            && generation > initial_generation
                        {
                            return if succeeded {
                                Ok(())
                            } else {
                                Err(FEMTO
                                    .unavailable(
                                        ErrorReason::FemtocellStatusUnavailable,
                                        "The femtocell did not report its status",
                                    )
                                    .status())
                            };
                        }
                        tokio::time::sleep(STATUS_POLL_INTERVAL).await;
                    }
                };

                match tokio::time::timeout(timeout, refresh).await {
                    Ok(Ok(())) => log::info!("ACS background sync refreshed FAP {feid}"),
                    Ok(Err(error)) => {
                        log::warn!("ACS background sync could not request FAP {feid}: {error}")
                    }
                    Err(_) => log::warn!(
                        "ACS background sync timed out refreshing FAP {feid} after {}s",
                        timeout.as_secs()
                    ),
                }
            }
            tokio::time::sleep(interval).await;
        }
    }))
}

#[tonic::async_trait]
impl FemtoManagementService for FemtoCellManagementService {
    async fn get_femto_cell_status(
        &self,
        request: Request<bts_management::CellRequest>,
    ) -> Result<Response<femto_management::FemtoCellStatus>, Status> {
        let request = request.into_inner();
        let cell = self.selected(&request)?;
        Ok(Response::new(femto_management::FemtoCellStatus {
            cell: Some(request),
            feid: cell.feid,
            profile: cell.profile,
            inner_peer_address: cell.inner_peer_address.map(|peer| peer.to_string()),
            sip_public_identity: cell.sip_public_identity,
            sip_registered: cell.sip_peer.is_some(),
            sip_expires_at_ms: None,
            acs_configured: cell.acs_configured,
            acs_provisioned: cell.acs_provisioned,
            last_acs_inform_ms: cell.last_acs_inform_ms,
            gps_status: cell.gps_status,
            timing_status: cell.timing_status,
            location_validation_status: cell.location_validation_status,
            radio_operational_state: cell.radio_operational_state,
            ipsec_established: None,
            last_updated_ms: cell.last_updated_ms,
            gps_latitude: cell.gps_latitude,
            gps_longitude: cell.gps_longitude,
        }))
    }

    async fn set_femto_radio_administrative_state(
        &self,
        request: Request<femto_management::SetFemtoRadioAdministrativeStateRequest>,
    ) -> Result<Response<femto_management::FemtoCommandResponse>, Status> {
        let request = request.into_inner();
        let cell = self.selected(
            request
                .cell
                .as_ref()
                .ok_or_else(|| FEMTO.invalid_field("cell", "A femtocell is required"))?,
        )?;
        let enabled = match femto_management::FemtoRadioAdministrativeState::try_from(request.state)
        {
            Ok(femto_management::FemtoRadioAdministrativeState::Enabled) => true,
            Ok(femto_management::FemtoRadioAdministrativeState::Disabled) => false,
            _ => {
                return Err(FEMTO.invalid_field("state", "Choose enabled or disabled"));
            }
        };
        self.state.request_radio_state(&cell.feid, enabled);
        if let Err(error) = Self::request_connection(&cell).await {
            self.state.complete_radio_state(&cell.feid);
            return Err(error);
        }
        Ok(Response::new(femto_management::FemtoCommandResponse {
            accepted: true,
            message: "radio state change queued for the CWMP session".to_string(),
        }))
    }

    async fn request_provisioning_sync(
        &self,
        request: Request<bts_management::CellRequest>,
    ) -> Result<Response<femto_management::FemtoCommandResponse>, Status> {
        let cell = self.selected(&request.into_inner())?;
        Self::request_connection(&cell).await?;
        Ok(Response::new(femto_management::FemtoCommandResponse {
            accepted: true,
            message: "provisioning sync requested".to_string(),
        }))
    }

    async fn reboot_femtocell(
        &self,
        request: Request<bts_management::CellRequest>,
    ) -> Result<Response<femto_management::FemtoCommandResponse>, Status> {
        let cell = self.selected(&request.into_inner())?;
        self.state.request_reboot(&cell.feid);
        if let Err(error) = Self::request_connection(&cell).await {
            self.state.complete_reboot(&cell.feid);
            return Err(error);
        }
        Ok(Response::new(femto_management::FemtoCommandResponse {
            accepted: true,
            message: "reboot queued for the CWMP session".to_string(),
        }))
    }

    async fn reload_config(
        &self,
        _request: Request<femto_management::ReloadConfigRequest>,
    ) -> Result<Response<femto_management::ReloadConfigResponse>, Status> {
        let path =
            std::env::var("FEMTO_CONFIG").unwrap_or_else(|_| "config/femto.json".to_string());
        let config = crate::config::FemtoConfig::load(&path).map_err(|error| {
            config_reload_failed(format!("The femtocell config could not be read: {error}"))
        })?;
        let registry = crate::fap::FapRegistry::from_config(
            &config.fap_profiles,
            &config.flat_sites,
            &config.faps,
        )
        .map_err(|error| {
            config_reload_failed(format!("The femtocell config is not valid: {error}"))
        })?;
        let fap_count = registry.len() as u32;
        let site_count = config.sites.len() as u32;
        *self
            .acs_a11_provisioning
            .write()
            .expect("A11 provisioning lock") = config.a11_provisioning.clone();
        self.state.reload_from_config(&config);
        *self.fap_registry.write().expect("fap registry lock") = std::sync::Arc::new(registry);
        *self.acs_credentials.write().expect("acs credentials lock") =
            config.acs.cpe_credentials.clone();
        *self.acs_term_auth_psk.write().expect("term auth psk lock") = config.term_auth_psk.clone();
        *self.served_cells.write().expect("served cells lock") =
            std::sync::Arc::new(crate::base_station::served_cells(&config));
        *self.sip_users.write().expect("sip users lock") = crate::sip::build_ims_users(&config.sip);
        log::info!(
            "ReloadConfig: reloaded provisioning config from {path} ({fap_count} FAPs, {site_count} sites)"
        );
        Ok(Response::new(femto_management::ReloadConfigResponse {
            reloaded: true,
            message: format!(
                "reloaded {fap_count} FAP(s), {site_count} site(s); ACS registry, cell state, served cells, CWMP credentials, A12 PSK, and SIP registrar users swapped. Listener sockets are unchanged; existing SIP registrations for removed users expire on their own; the SeGW reloads itself."
            ),
            fap_count,
            site_count,
        }))
    }
}

#[derive(Clone)]
pub struct FemtoBtsManagementService {
    state: Arc<FemtoCellState>,
    cells: Vec<FemtoCell>,
    management_endpoint: String,
}

#[derive(Clone)]
struct FemtoCell {
    feid: String,
    name: Option<String>,
    cell: bsc::CellId,
    sid: u32,
    nid: u32,
    band_class: u32,
    channel: u32,
    pilot_pn: u32,
    evdo_enabled: bool,
    evdo_color_code: Option<u32>,
}

impl FemtoBtsManagementService {
    pub fn new(config: &crate::config::FemtoConfig, state: Arc<FemtoCellState>) -> Self {
        let cells = config
            .faps
            .iter()
            .filter_map(|(feid, definition)| {
                let site = config.flat_sites.get(&definition.site)?;
                Some(FemtoCell {
                    feid: feid.clone(),
                    name: definition.name.clone(),
                    cell: bsc::CellId {
                        cell: u32::from(site.cell_id),
                        sector: u32::from(site.sector),
                    },
                    sid: site.sid,
                    nid: site.nid,
                    band_class: site.band_class,
                    channel: site.cdma_channel,
                    pilot_pn: site.pn_offset.unwrap_or_default(),
                    evdo_enabled: site.evdo_enabled,
                    evdo_color_code: site.evdo_color_code,
                })
            })
            .collect();
        Self {
            state,
            cells,
            management_endpoint: config.fcs.management_listen.clone(),
        }
    }

    pub fn into_server(self) -> BtsManagementServiceServer<Self> {
        BtsManagementServiceServer::new(self)
    }
}

fn unsupported<T>(operation: &str) -> Result<Response<T>, Status> {
    Err(FEMTO.not_supported(operation, "Femtocells do not support this"))
}

#[tonic::async_trait]
impl BtsManagementService for FemtoBtsManagementService {
    async fn enroll(
        &self,
        _: Request<bts_management::EnrollRequest>,
    ) -> Result<Response<bts_management::EnrollResponse>, Status> {
        unsupported("single-cell enrollment")
    }

    async fn list_bts(&self, _: Request<()>) -> Result<Response<bts_management::BtsList>, Status> {
        let snapshots = self.state.cells();
        let mobiles = self.state.mobiles();
        let bts = self
            .cells
            .iter()
            .map(|configured| {
                let in_service = snapshots
                    .iter()
                    .find(|cell| cell.feid == configured.feid)
                    .is_some_and(|cell| cell.sip_peer.is_some());
                let status_detail = (!in_service).then(|| "SIP not registered".to_string());
                bts_management::BtsSummary {
                    peer_id: configured.feid.clone(),
                    management_endpoint: self.management_endpoint.clone(),
                    cell: Some(configured.cell.clone()),
                    state: if in_service {
                        bts_management::BtsAttachState::InService as i32
                    } else {
                        bts_management::BtsAttachState::Enrolled as i32
                    },
                    pilot_pn: configured.pilot_pn,
                    band_class: format!("BC{}", configured.band_class),
                    cdma_channel: configured.channel,
                    sid: configured.sid,
                    nid: configured.nid,
                    evdo_enabled: configured.evdo_enabled,
                    served_mobiles: Some(
                        mobiles
                            .iter()
                            .filter(|mobile| mobile.feid == configured.feid)
                            .count() as u32,
                    ),
                    status_detail,
                    evdo_color_code: configured.evdo_color_code,
                    an_grpc_addr: None,
                    kind: bts_management::CellKind::Femtocell as i32,
                    name: configured.name.clone(),
                    capabilities: vec![bts_management::CellCapability::FemtoManagement as i32],
                    software_version: snapshots
                        .iter()
                        .find(|cell| cell.feid == configured.feid)
                        .and_then(|cell| cell.software_version.clone()),
                }
            })
            .collect();
        Ok(Response::new(bts_management::BtsList { bts }))
    }

    async fn check_health(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }

    async fn get_bts_config(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bsc::BtsConfig>, Status> {
        unsupported("radio configuration readback")
    }

    async fn get_radio_metrics(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bsc::RadioMetrics>, Status> {
        unsupported("radio metrics")
    }

    type StreamRadioMetricsStream = ManagementStream<bsc::RadioMetrics>;

    async fn stream_radio_metrics(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        unsupported("radio metric streaming")
    }

    async fn get_iq_capture_status(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        unsupported("IQ capture")
    }

    async fn start_iq_capture(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        unsupported("IQ capture")
    }

    async fn stop_iq_capture(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        unsupported("IQ capture")
    }

    async fn list_local_radio_resources(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bsc::ChannelList>, Status> {
        unsupported("local radio resource enumeration")
    }

    async fn get_reverse_power_control(
        &self,
        _: Request<bts_management::ReversePowerControlRequest>,
    ) -> Result<Response<bsc::TrafficChannelPower>, Status> {
        unsupported("reverse power control")
    }

    async fn list_reverse_power_controls(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bts_management::ReversePowerControlList>, Status> {
        unsupported("reverse power control")
    }

    async fn list_access_measurements(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<bts_management::AccessMeasurementList>, Status> {
        unsupported("access measurements")
    }

    async fn set_reverse_power_control_override(
        &self,
        _: Request<bsc::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<bsc::SetTrafficChannelPowerOverrideResponse>, Status> {
        unsupported("reverse power-control overrides")
    }

    type StreamPchTransmissionsStream = ManagementStream<bsc::PagingEvent>;

    async fn stream_pch_transmissions(
        &self,
        _: Request<bts_management::CellRequest>,
    ) -> Result<Response<Self::StreamPchTransmissionsStream>, Status> {
        unsupported("paging-channel event streaming")
    }
}

type ManagementStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[derive(Clone)]
pub struct FemtoBscManagementService {
    state: Arc<FemtoCellState>,
}

impl FemtoBscManagementService {
    pub fn new(state: Arc<FemtoCellState>) -> Self {
        Self { state }
    }

    pub fn into_bsc_server(self) -> BscManagementServiceServer<Self> {
        BscManagementServiceServer::new(self)
    }

    pub fn into_facade_server(self) -> ManagementFacadeServiceServer<Self> {
        ManagementFacadeServiceServer::new(self)
    }
}

fn cell_health(cell: crate::cell::FemtoCellSnapshot) -> facade::NodeHealth {
    let healthy = cell.sip_peer.is_some();
    facade::NodeHealth {
        node_id: cell.feid,
        node_type: "Femtocell".to_string(),
        healthy,
        message: if healthy { "operational" } else { "starting" }.to_string(),
    }
}

#[tonic::async_trait]
impl BscManagementService for FemtoBscManagementService {
    async fn list_mobiles(&self, _: Request<()>) -> Result<Response<bsc::MobileList>, Status> {
        let mobiles = self
            .state
            .mobiles()
            .into_iter()
            .map(|mobile| bsc::MobileInfo {
                address: mobile.imsi.clone(),
                page_address: mobile.imsi.clone(),
                state: mobile.state,
                mob_p_rev: 0,
                esn: mobile.esn,
                imsi: Some(mobile.imsi),
                meid: mobile.meid,
                last_heard_ms: Some(mobile.last_seen_ms),
                serving_cell: self
                    .state
                    .cells()
                    .into_iter()
                    .find(|cell| cell.feid == mobile.feid)
                    .map(|cell| bsc::CellId {
                        cell: u32::from(cell.cell_id),
                        sector: u32::from(cell.sector),
                    }),
                ..bsc::MobileInfo::default()
            })
            .collect();
        Ok(Response::new(bsc::MobileList { mobiles }))
    }

    async fn list_channels(&self, _: Request<()>) -> Result<Response<bsc::ChannelList>, Status> {
        Err(FEMTO.not_supported(
            "channel_allocation_state",
            "Femtocells do not report channel allocation state",
        ))
    }

    type StreamAccessEventsStream = ManagementStream<bsc::AccessEvent>;

    async fn stream_access_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamAccessEventsStream>, Status> {
        Err(FEMTO.not_supported(
            "reverse_access_events",
            "Femtocells do not report reverse access events",
        ))
    }

    type StreamPagingEventsStream = ManagementStream<bsc::PagingEvent>;

    async fn stream_paging_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamPagingEventsStream>, Status> {
        Err(FEMTO.not_supported(
            "paging_channel_events",
            "Femtocells do not report paging-channel events",
        ))
    }

    type StreamTrafficEventsStream = ManagementStream<bsc::TrafficEvent>;

    async fn stream_traffic_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamTrafficEventsStream>, Status> {
        Err(FEMTO.not_supported(
            "traffic_channel_events",
            "Femtocells do not report traffic-channel events",
        ))
    }

    async fn set_traffic_channel_power_override(
        &self,
        _: Request<bsc::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<bsc::SetTrafficChannelPowerOverrideResponse>, Status> {
        Err(FEMTO.not_supported(
            "reverse_power_control",
            "Femtocells do not report reverse power control",
        ))
    }
}

#[tonic::async_trait]
impl ManagementFacadeService for FemtoBscManagementService {
    async fn get_system_overview(
        &self,
        _: Request<()>,
    ) -> Result<Response<facade::SystemOverview>, Status> {
        let nodes = self.state.cells().into_iter().map(cell_health).collect();
        Ok(Response::new(facade::SystemOverview { nodes }))
    }

    type StreamSystemEventsStream = ManagementStream<facade::ManagementEvent>;

    async fn stream_system_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamSystemEventsStream>, Status> {
        const HEALTH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
        const EVENT_QUEUE_CAPACITY: usize = 16;

        let state = Arc::clone(&self.state);
        let (tx, rx) = tokio::sync::mpsc::channel(EVENT_QUEUE_CAPACITY);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(HEALTH_INTERVAL);
            loop {
                interval.tick().await;
                for cell in state.cells() {
                    let health = cell_health(cell);
                    let event = facade::ManagementEvent {
                        source_node_id: health.node_id.clone(),
                        source_node_type: health.node_type.clone(),
                        classification: "health".to_string(),
                        body: Some(facade::management_event::Body::NodeHealth(health)),
                    };
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}

fn femtocell_unreachable(message: impl Into<String>) -> Status {
    FEMTO
        .unavailable(ErrorReason::FemtocellUnreachable, message)
        .status()
}

fn femtocell_auth_failed(message: impl Into<String>) -> Status {
    FEMTO
        .unavailable(ErrorReason::FemtocellAuthFailed, message)
        .status()
}

fn femtocell_timeout() -> Status {
    FEMTO
        .error(
            tonic::Code::DeadlineExceeded,
            ErrorReason::FemtocellUnreachable,
            "The femtocell did not answer in time",
        )
        .status()
}

fn config_reload_failed(message: impl Into<String>) -> Status {
    FEMTO
        .error(
            tonic::Code::Internal,
            ErrorReason::ConfigReloadFailed,
            message,
        )
        .status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_ios::MobileIdentity;
    use tokio_stream::StreamExt;
    use tonic::Code;

    #[tokio::test]
    async fn lists_femto_mobiles_through_bsc_management() {
        let state = FemtoCellState::untracked();
        state.observe_mobile(
            "test",
            "310005551239999",
            Some(&MobileIdentity::Esn(0x80ab_cdef)),
            None,
            "Registered",
        );
        let service = FemtoBscManagementService::new(state);

        let mobiles = service
            .list_mobiles(Request::new(()))
            .await
            .unwrap()
            .into_inner()
            .mobiles;

        assert_eq!(mobiles.len(), 1);
        assert_eq!(mobiles[0].address, "310005551239999");
        assert_eq!(mobiles[0].imsi.as_deref(), Some("310005551239999"));
        assert_eq!(mobiles[0].esn, Some(0x80ab_cdef));
    }

    #[tokio::test]
    async fn lists_configured_faps_as_standard_cells() {
        let mut config: crate::config::FemtoConfig =
            serde_json::from_str(include_str!("../../../config/femto.json")).unwrap();
        let test_fap: crate::config::UnifiedSite = serde_json::from_str(
            r#"{"feid":"0005B9AABBCC","profile":"airave-25-5x","name":"TestFAP",
                "cell":{"mcc":"310","imsi_11_12":"00","cell_id":403},
                "1x":{"sid":4107,"nid":65535,"band_class":1,"channel":50,"pn_offset":510},
                "evdo":{"enabled":true,"color_code":58},
                "acs":{"username":"x","password":"x"},"sip":{"password":"x"},"segw":{}}"#,
        )
        .unwrap();
        config.sites.insert("test-fap".to_string(), test_fap);
        config.normalize();
        let state = FemtoCellState::from_config(&config);
        state.observe_software_version("0005B9AABBCC", "5.0.02.094".to_string());
        let service = FemtoBtsManagementService::new(&config, state);

        let cells = service
            .list_bts(Request::new(()))
            .await
            .unwrap()
            .into_inner()
            .bts;

        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].peer_id, "0005B9AABBCC");
        assert_eq!(cells[0].cell.as_ref().unwrap().cell, 403);
        assert_eq!(cells[0].cdma_channel, 50);
        assert_eq!(cells[0].pilot_pn, 510);
        assert!(cells[0].evdo_enabled);
        assert_eq!(cells[0].evdo_color_code, Some(58));
        assert_eq!(cells[0].sid, 4107);
        assert_eq!(cells[0].name.as_deref(), Some("TestFAP"));
        assert_eq!(cells[0].software_version.as_deref(), Some("5.0.02.094"));
    }

    #[tokio::test]
    async fn rejects_bsc_only_channel_operations_explicitly() {
        let service = FemtoBscManagementService::new(FemtoCellState::untracked());

        let error = service.list_channels(Request::new(())).await.unwrap_err();

        assert_eq!(error.code(), Code::Unimplemented);
        assert_eq!(
            cdma_rpc_error::error_info(&error).map(|info| info.reason),
            Some(ErrorReason::NotSupported)
        );
    }

    #[tokio::test]
    async fn streams_femto_health_through_the_management_facade() {
        let state = FemtoCellState::untracked();
        state.register_fap(
            "test",
            "sip:test@example.test".to_string(),
            "10.55.100.1:5060".parse().unwrap(),
        );
        let service = FemtoBscManagementService::new(state);

        let overview = service
            .get_system_overview(Request::new(()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(overview.nodes.len(), 1);
        assert!(overview.nodes[0].healthy);

        let mut stream = service
            .stream_system_events(Request::new(()))
            .await
            .unwrap()
            .into_inner();
        let event = stream.next().await.unwrap().unwrap();
        assert_eq!(event.source_node_id, "test");
        assert_eq!(event.classification, "health");
        assert!(matches!(
            event.body,
            Some(facade::management_event::Body::NodeHealth(
                facade::NodeHealth { healthy: true, .. }
            ))
        ));
    }
}
