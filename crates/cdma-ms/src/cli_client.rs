use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "sim")]
use cdma_ms::appliance::LoopbackControl;
use cdma_ms::appliance::MsAppliance;
use cdma_ms::config::MsNodeConfig;
use cdma_ms::engine::EngineConfig;
use cdma_ms::forward_rx::ForwardRxConfig;
use cdma_ms::grpc::proto::ms_service_client::MsServiceClient;
use cdma_ms::grpc::{MsServiceImpl, serve_on_listener};
use cdma_ms::ms::MsEvent;
use cdma_ms::prl_scan::PrlScanPlan;
use tonic::transport::Channel;

use crate::cli_radio;

type Error = Box<dyn std::error::Error + Send + Sync>;

const CONNECT_ATTEMPTS: usize = 40;
const CONNECT_RETRY: Duration = Duration::from_millis(50);

pub struct Session {
    pub client: MsServiceClient<Channel>,
    pub addr: String,
    pub embedded: bool,
    _server: Option<tokio::task::JoinHandle<()>>,
}

pub fn build_service(
    config: &MsNodeConfig,
    radio_arg: Option<&str>,
    prl: Option<&Path>,
) -> Result<MsServiceImpl, Error> {
    let config = config_with_radio(config, radio_arg)?;
    let identity = config.identity.access_identity()?;
    let built = cli_radio::build_radio(&config.radio, &config)?;
    log::info!("cdma-ms: {} radio ready", built.kind);
    let plan = match prl {
        Some(path) => {
            let plan = PrlScanPlan::load(path)?;
            log::info!("ms_scan: {}", plan.summary());
            Some(Arc::new(plan))
        }
        None => None,
    };
    let scan = plan
        .clone()
        .map(|plan| cli_radio::scan_config(&config, plan, None));
    let rx_reference_dbm = built
        .radio
        .rx_reference_dbm()
        .unwrap_or_else(|| cli_radio::rx_reference_dbm_for(&config.radio, &config));
    let appliance = MsAppliance::start(
        built.radio,
        EngineConfig {
            identity,
            sample_rate_hz: cli_radio::SAMPLE_RATE_HZ,
            rx_reference_dbm,
            forward_rx: ForwardRxConfig::default(),
            scan,
            power_up_delay: config.power_up_delay(),
        },
    );
    let mut service = MsServiceImpl::new(appliance, Arc::new(config));
    if let Some(plan) = plan {
        service = service.with_plan(plan);
    }
    #[cfg(feature = "sim")]
    if let Some(sim) = built.sim {
        service = service.with_loopback(Arc::new(sim) as Arc<dyn LoopbackControl>);
    }
    Ok(service)
}

fn config_with_radio(
    config: &MsNodeConfig,
    radio_arg: Option<&str>,
) -> Result<MsNodeConfig, Error> {
    let mut selected = config.clone();
    selected.radio = cli_radio::select_radio(config, radio_arg)?;
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_override_selects_network_scan_dwell() {
        let config: MsNodeConfig = serde_json::from_str(
            r#"{"identity":{"esn":305419896,"imsi":"310001234567890"},"channel":{"band_class":"bc1","cdma_channel":50},"radio":{"kind":"sim"}}"#,
        )
        .unwrap();
        let radio_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/radio_network.json");
        let selected = config_with_radio(&config, radio_path.to_str()).unwrap();

        assert_eq!(
            cli_radio::default_dwell_ms(&config),
            cdma_ms::config::DEFAULT_SCAN_DWELL_MS
        );
        assert_eq!(
            cli_radio::default_dwell_ms(&selected),
            cdma_ms::config::DEFAULT_SDR_SCAN_DWELL_MS
        );
    }
}

impl Session {
    pub async fn open(
        connect: Option<&str>,
        config: &MsNodeConfig,
        radio_arg: Option<&str>,
        prl: Option<&Path>,
    ) -> Result<Session, Error> {
        if let Some(addr) = connect {
            let endpoint = if addr.starts_with("http://") || addr.starts_with("https://") {
                addr.to_string()
            } else {
                format!("http://{addr}")
            };
            let client = MsServiceClient::connect(endpoint.clone()).await?;
            return Ok(Session {
                client,
                addr: endpoint,
                embedded: false,
                _server: None,
            });
        }
        let service = build_service(config, radio_arg, prl)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let local = listener.local_addr()?;
        let server = tokio::spawn(async move {
            if let Err(e) = serve_on_listener(listener, service).await {
                log::error!("cdma-ms: embedded gRPC server stopped: {}", e);
            }
        });
        let endpoint = format!("http://{local}");
        let mut client = None;
        for _ in 0..CONNECT_ATTEMPTS {
            if let Ok(c) = MsServiceClient::connect(endpoint.clone()).await {
                client = Some(c);
                break;
            }
            tokio::time::sleep(CONNECT_RETRY).await;
        }
        let client = client.ok_or("could not connect to the embedded gRPC server")?;
        Ok(Session {
            client,
            addr: endpoint,
            embedded: true,
            _server: Some(server),
        })
    }
}

pub fn decode_event(ev: &cdma_ms::grpc::proto::MsEvent) -> Option<MsEvent> {
    serde_json::from_str(&ev.detail).ok()
}
