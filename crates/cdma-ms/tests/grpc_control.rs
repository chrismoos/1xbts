mod support;

use std::sync::Arc;
use std::time::Duration;

use tokio_stream::StreamExt;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use cdma_common::band_class::BandClass;
use cdma_ms::appliance::MsAppliance;
use cdma_ms::config::{MsChannel, MsIdentity, MsNodeConfig, MsRadioConfig};
use cdma_ms::engine::EngineConfig;
use cdma_ms::grpc::MsServiceImpl;
use cdma_ms::grpc::proto::ms_service_client::MsServiceClient;
use cdma_ms::grpc::proto::ms_service_server::MsServiceServer;
use cdma_ms::grpc::proto::{
    MsState, PowerOnRequest, PrlVerdictRequest, ScanChannelSpec, StartScanRequest,
};

fn config() -> MsNodeConfig {
    MsNodeConfig {
        identity: MsIdentity {
            esn: 0x1234_5678,
            imsi: "310001234567890".to_string(),
            ..Default::default()
        },
        channel: MsChannel {
            band_class: BandClass::Bc0,
            band_subclass: Some(0),
            cdma_channel: 384,
        },
        radio: MsRadioConfig::default(),
        acquisition: Default::default(),
        calibration: Default::default(),
        grpc_listen: "[::1]:0".to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_drives_the_ms() {
    let cfg = config();
    let identity = cfg.identity.access_identity().unwrap();
    let (radio, _sim) = support::boot_sim(40_000);
    let appliance = MsAppliance::start(
        Box::new(radio),
        EngineConfig {
            identity,
            sample_rate_hz: support::SAMPLE_RATE_HZ as f64,
            ..Default::default()
        },
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = MsServiceImpl::new(appliance, Arc::new(cfg));
    tokio::spawn(async move {
        Server::builder()
            .add_service(MsServiceServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let mut client = None;
    for _ in 0..20 {
        if let Ok(c) = MsServiceClient::connect(format!("http://{addr}")).await {
            client = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut client = client.expect("connect to MS gRPC");

    let status = client.get_status(()).await.unwrap().into_inner();
    assert_eq!(status.state(), MsState::Off);
    assert_eq!(status.registered, "no");

    let invalid = client
        .send_dtmf_burst(cdma_ms::grpc::proto::SendDtmfBurstRequest {
            digits: "12A".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
    let idle = client
        .send_dtmf_burst(cdma_ms::grpc::proto::SendDtmfBurstRequest {
            digits: "123#".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(idle.code(), tonic::Code::FailedPrecondition);

    let mut events = client.stream_events(()).await.unwrap().into_inner();
    client.power_on(PowerOnRequest {}).await.unwrap();

    let ev = tokio::time::timeout(Duration::from_secs(3), events.next())
        .await
        .expect("event within timeout")
        .expect("stream item")
        .expect("event ok");
    assert_eq!(ev.event_type, "state_change");

    let left_off = poll_status(&mut client, Duration::from_secs(10), |s| {
        s.state() != MsState::Off
    })
    .await;
    assert!(left_off, "station should acquire and leave the off state");

    let mut diagnostics = None;
    for _ in 0..200 {
        let d = client.get_diagnostics(()).await.unwrap().into_inner();
        if d.sync.is_some()
            && d.overhead
                .as_ref()
                .is_some_and(|o| !o.system_parameters.is_empty())
        {
            diagnostics = Some(d);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let d = diagnostics.expect("sync and overhead within the sim run");
    let sync = d.sync.unwrap();
    assert_eq!(
        (sync.sid, sync.nid),
        (support::SID as u32, support::NID as u32)
    );
    assert!(sync.system_time.starts_with("1980-01-06"));
    let pilot = d.pilot.unwrap();
    assert!(pilot.locked && pilot.measured, "pilot {pilot:?}");
    let overhead = d.overhead.unwrap();
    assert!(overhead.received.contains(&"SPM".to_string()));
    let spm: serde_json::Value = serde_json::from_str(&overhead.system_parameters).unwrap();
    assert_eq!(spm["sid"], support::SID);
    assert!(d.paging.unwrap().crc_valid > 0);
    assert!(d.radio.unwrap().samples_fed > 0);

    let refused = client
        .prl_verdict(PrlVerdictRequest { sid: 42, nid: 7 })
        .await;
    assert_eq!(refused.unwrap_err().code(), tonic::Code::FailedPrecondition);

    let started = client
        .start_scan(StartScanRequest {
            channels: vec![ScanChannelSpec {
                band_class: "bc0".to_string(),
                channel: support::CDMA_FREQ as u32,
            }],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(started.channels, 1);
    let mut camped = false;
    for _ in 0..300 {
        let d = client.get_diagnostics(()).await.unwrap().into_inner();
        if d.scan.as_ref().is_some_and(|s| s.result == "camped") {
            camped = true;
            let scan = d.scan.unwrap();
            assert_eq!(scan.channels[0].pilot, "found");
            assert!(scan.channels[0].verdict.as_ref().unwrap().permitted);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(camped, "scan over the API should camp on the node");
    let channels = client.list_channels(()).await.unwrap().into_inner();
    assert_eq!(channels.channels.len(), 1);
    assert_eq!(channels.channels[0].channel, support::CDMA_FREQ as u32);

    client.power_off(()).await.unwrap();
    let back_off = poll_status(&mut client, Duration::from_secs(5), |s| {
        s.state() == MsState::Off
    })
    .await;
    assert!(back_off, "power off should return the station to off");
}

async fn poll_status(
    client: &mut MsServiceClient<tonic::transport::Channel>,
    timeout: Duration,
    pred: impl Fn(&cdma_ms::grpc::proto::MsStatus) -> bool,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = client.get_status(()).await.unwrap().into_inner();
        if pred(&status) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
