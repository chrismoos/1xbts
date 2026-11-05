use std::net::SocketAddr;

use cdma_common::error::Error;
use log::{error, info};
use tokio::sync::watch;
use tonic::{Request, Response, Status};

use crate::hrpd_a9_service::HrpdA10SessionView;

pub mod proto {
    tonic::include_proto!("pcf.v1");
}

use proto::pcf_service_server::{PcfService, PcfServiceServer};

pub struct PcfManagement {
    sessions: watch::Receiver<Vec<HrpdA10SessionView>>,
}

impl PcfManagement {
    pub fn new(sessions: watch::Receiver<Vec<HrpdA10SessionView>>) -> Self {
        Self { sessions }
    }
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn to_proto(view: &HrpdA10SessionView) -> proto::A10Session {
    proto::A10Session {
        pcf_session_id: view.pcf_session_id,
        mn_session_reference_id: u32::from(view.mn_session_reference_id),
        a8_key: view.a8_key,
        correlation_id: view.correlation_id,
        con_ref: u32::from(view.con_ref),
        imsi: view.imsi.clone().unwrap_or_default(),
        esn: view.esn,
        meid: view.meid.map(|meid| hex_upper(&meid)).unwrap_or_default(),
        pdsn_address: view
            .pdsn_address
            .map(|octets| std::net::Ipv4Addr::from(octets).to_string())
            .unwrap_or_default(),
        an_address: view.an_address.to_string(),
    }
}

#[tonic::async_trait]
impl PcfService for PcfManagement {
    async fn list_a10_sessions(
        &self,
        _request: Request<proto::ListA10SessionsRequest>,
    ) -> Result<Response<proto::ListA10SessionsResponse>, Status> {
        let sessions = self.sessions.borrow().iter().map(to_proto).collect();
        Ok(Response::new(proto::ListA10SessionsResponse { sessions }))
    }
}

pub async fn spawn_pcf_management(
    bind_addr: SocketAddr,
    sessions: watch::Receiver<Vec<HrpdA10SessionView>>,
) -> Result<SocketAddr, Error> {
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|err| Error::from(format!("failed to bind PCF management {bind_addr}: {err}")))?;
    let local_addr = listener
        .local_addr()
        .map_err(|err| Error::from(format!("failed to read PCF management local addr: {err}")))?;
    let service = PcfServiceServer::new(PcfManagement::new(sessions));
    tokio::spawn(async move {
        info!("PCF management gRPC listening on {local_addr}");
        if let Err(err) = tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
        {
            error!("PCF management gRPC server error: {err}");
        }
    });
    Ok(local_addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> HrpdA10SessionView {
        HrpdA10SessionView {
            pcf_session_id: 2,
            mn_session_reference_id: 1,
            a8_key: 0x0039_7dd2,
            correlation_id: Some(0x0039_7dd2),
            con_ref: 12,
            imsi: Some("310006054426442".to_string()),
            esn: None,
            meid: Some([0xA0, 0x00, 0x00, 0x4C, 0xDF, 0x53, 0xAB]),
            pdsn_address: Some([127, 0, 0, 1]),
            an_address: "127.0.0.1:40000".parse().unwrap(),
        }
    }

    #[tokio::test]
    async fn lists_the_current_a10_sessions() {
        let (tx, rx) = watch::channel(vec![view()]);
        let management = PcfManagement::new(rx);
        let response = management
            .list_a10_sessions(Request::new(proto::ListA10SessionsRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.sessions.len(), 1);
        let session = &response.sessions[0];
        assert_eq!(session.pcf_session_id, 2);
        assert_eq!(session.a8_key, 0x0039_7dd2);
        assert_eq!(session.correlation_id, Some(0x0039_7dd2));
        assert_eq!(session.imsi, "310006054426442");
        assert_eq!(session.meid, "A000004CDF53AB");
        assert_eq!(session.pdsn_address, "127.0.0.1");

        tx.send(Vec::new()).unwrap();
        let emptied = management
            .list_a10_sessions(Request::new(proto::ListA10SessionsRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert!(emptied.sessions.is_empty());
    }
}
