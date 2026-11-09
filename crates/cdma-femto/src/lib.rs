pub mod acs;
pub mod base_station;
pub mod cell;
pub mod config;
pub mod event;
pub mod fap;
pub mod fcs;
pub mod management;
pub mod packet;
pub mod segw;
pub mod sip;
pub mod voice;

pub use config::{FemtoConfig, PacketAdapter, SecurityProfile};

#[derive(Debug, thiserror::Error)]
#[error("femtocell subsystem not yet operational: {0}")]
pub struct NotOperational(pub &'static str);

pub struct FemtoService {
    config: FemtoConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("invalid listen address {field}={value}: {source}")]
    ListenAddr {
        field: &'static str,
        value: String,
        source: std::net::AddrParseError,
    },
    #[error(transparent)]
    FapRegistry(#[from] fap::FapRegistryError),
    #[error("SIP user {identity} has no FEID")]
    MissingSipFeid { identity: String },
    #[error("CWMP credential {username} has no FEID")]
    MissingCwmpFeid { username: String },
    #[error("binding FCS A1 listener {addr}: {source}")]
    A1Bind {
        addr: String,
        source: std::io::Error,
    },
}

impl FemtoService {
    pub fn new(config: FemtoConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &FemtoConfig {
        &self.config
    }

    pub async fn run(self) -> Result<(), ServiceError> {
        let config = self.config;
        log::info!(
            "femtocell service starting: home_domain={} segw={} acs={} sip={} adapter={:?}",
            config.home_domain,
            config.segw.listen,
            config.acs.listen,
            config.sip.listen,
            config.packet.adapter,
        );

        let acs_addr = parse_addr("acs.listen", &config.acs.listen)?;
        let sip_addr = parse_addr("sip.listen", &config.sip.listen)?;
        let management_addr = parse_addr("fcs.management_listen", &config.fcs.management_listen)?;
        let fap_registry = std::sync::Arc::new(fap::FapRegistry::from_config(
            &config.fap_profiles,
            &config.flat_sites,
            &config.faps,
        )?);
        log::info!(
            "femtocell implementation registry loaded {} configured FAPs",
            fap_registry.len()
        );
        for connection in &config.segw.connections {
            fap_registry.validate_security_profile(&connection.feid, connection.profile)?;
        }
        for user in &config.sip.users {
            let feid = user
                .feid
                .as_deref()
                .ok_or_else(|| ServiceError::MissingSipFeid {
                    identity: user.impu.clone(),
                })?;
            fap_registry.require(feid)?;
        }
        for credential in &config.acs.cpe_credentials {
            let feid = credential
                .feid
                .as_deref()
                .ok_or_else(|| ServiceError::MissingCwmpFeid {
                    username: credential.username.clone(),
                })?;
            fap_registry.require(feid)?;
        }
        let cell_state = cell::FemtoCellState::from_config(&config);

        let cell_identifier = cdma_ios::CellId {
            cell: config.fcs.cell_id,
            sector: config.fcs.sector,
        };
        let bridge = match config.fcs.msc_a1_addr.clone() {
            Some(address) => fcs::spawn_msc_bridge(address, config.fcs.a1p.clone()),
            None => {
                let address = parse_addr("fcs.a1_listen", &config.fcs.a1_listen)?;
                fcs::spawn_msc_listener(address, config.fcs.a1p.clone())
                    .await
                    .map_err(|source| ServiceError::A1Bind {
                        addr: address.to_string(),
                        source,
                    })?
            }
        };
        let mut ims = sip::ImsCore::with_msc_bridge_cell_state_and_faps(
            &config.sip,
            &config.fcs,
            cell_identifier,
            bridge,
            cell_state.clone(),
            fap_registry.clone(),
        );
        match cdma_events::EventPublisher::spawn(cdma_events::EventPublisherConfig::new(
            config.event_bus_addr.clone(),
            "femto",
        )) {
            Ok(publisher) => {
                log::info!(
                    "femtocell publishing A1 signaling events to {}",
                    config.event_bus_addr
                );
                ims = ims.with_event_publisher(publisher);
            }
            Err(error) => log::warn!(
                "femtocell event bus unavailable at {}: {error}; A1 signaling events disabled",
                config.event_bus_addr
            ),
        }
        let shared_registry: fap::SharedFapRegistry =
            std::sync::Arc::new(std::sync::RwLock::new(fap_registry.clone()));
        let acs = acs::AcsServer::with_cell_state(&config.acs, cell_state.clone())
            .with_fap_registry(shared_registry.clone())
            .with_learned_sip_passwords(ims.learned_passwords())
            .with_fap_route_hints(ims.fap_route_hint_sender())
            .with_term_auth_psk(config.term_auth_psk.clone())
            .with_a11_provisioning(config.a11_provisioning.clone());
        let acs_credentials = acs.credentials_handle();
        let acs_term_auth_psk = acs.term_auth_psk_handle();
        let ingress = packet::FemtoPacketIngress::new(&config.packet);
        let base_station = base_station::FemtoBaseStationService::new(&config);
        let served_cells = base_station.cells_handle();
        let bsc_management = management::FemtoBscManagementService::new(cell_state.clone());
        let bts_management =
            management::FemtoBtsManagementService::new(&config, cell_state.clone());
        let femto_management = management::FemtoCellManagementService::new(
            cell_state.clone(),
            shared_registry.clone(),
            acs_credentials,
            acs_term_auth_psk,
            acs.a11_provisioning_handle(),
            served_cells,
            ims.sip_users_handle(),
        );
        let _background_sync = management::spawn_background_sync(
            config.acs.background_sync.clone(),
            cell_state.clone(),
        );

        let acs_task = tokio::spawn(async move {
            if let Err(err) = acs.serve(acs_addr).await {
                log::error!("ACS server exited: {err}");
            }
        });
        let ims_task = tokio::spawn(async move {
            if let Err(err) = ims.serve(sip_addr).await {
                log::error!("IMS/SIP server exited: {err}");
            }
        });
        let packet_task = tokio::spawn(async move {
            if let Err(err) = ingress.start().await {
                log::error!("packet ingress exited: {err}");
            }
        });
        let management_task = tokio::spawn(async move {
            log::info!("FCS cell management gRPC listening on {management_addr}");
            if let Err(error) = tonic::transport::Server::builder()
                .add_service(
                    base_station::proto::base_station_service_server::BaseStationServiceServer::new(
                        base_station,
                    ),
                )
                .add_service(bsc_management.clone().into_bsc_server())
                .add_service(bsc_management.into_facade_server())
                .add_service(bts_management.into_server())
                .add_service(femto_management.into_server())
                .serve(management_addr)
                .await
            {
                log::error!("FCS cell management server exited: {error}");
            }
        });

        tokio::select! {
            _ = acs_task => log::warn!("ACS task finished"),
            _ = ims_task => log::warn!("IMS/SIP task finished"),
            _ = packet_task => log::warn!("packet ingress task finished"),
            _ = management_task => log::warn!("cell management task finished"),
        }
        Ok(())
    }
}

fn parse_addr(field: &'static str, value: &str) -> Result<std::net::SocketAddr, ServiceError> {
    value.parse().map_err(|source| ServiceError::ListenAddr {
        field,
        value: value.to_string(),
        source,
    })
}
