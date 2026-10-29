use tonic::{Request, Response, Status};

pub mod grpc {
    pub mod bsc {
        pub mod v1 {
            tonic::include_proto!("bsc.v1");
        }
    }

    pub mod base_station {
        pub mod v1 {
            tonic::include_proto!("base_station.v1");
        }
    }
}

pub use grpc::base_station::v1 as proto;

pub type SharedServedCells =
    std::sync::Arc<std::sync::RwLock<std::sync::Arc<Vec<proto::ServedCell>>>>;

pub fn served_cells(config: &crate::config::FemtoConfig) -> Vec<proto::ServedCell> {
    config
        .flat_sites
        .values()
        .map(|site| proto::ServedCell {
            cell: Some(grpc::bsc::v1::CellId {
                cell: u32::from(site.cell_id),
                sector: u32::from(site.sector),
            }),
            sid: site.sid,
            nid: site.nid,
            mcc_digits: site.mcc.clone(),
            imsi_11_12_digits: site.imsi_11_12.clone(),
            in_service: true,
        })
        .collect()
}

#[derive(Clone)]
pub struct FemtoBaseStationService {
    node_id: String,
    a1_addr: String,
    cells: SharedServedCells,
}

impl FemtoBaseStationService {
    pub fn new(config: &crate::config::FemtoConfig) -> Self {
        Self {
            node_id: config.fcs.node_id.clone(),
            a1_addr: config.fcs.a1_advertise_addr.clone(),
            cells: std::sync::Arc::new(std::sync::RwLock::new(std::sync::Arc::new(served_cells(
                config,
            )))),
        }
    }

    pub fn cells_handle(&self) -> SharedServedCells {
        self.cells.clone()
    }
}

#[tonic::async_trait]
impl proto::base_station_service_server::BaseStationService for FemtoBaseStationService {
    async fn enroll(
        &self,
        _: Request<proto::EnrollRequest>,
    ) -> Result<Response<proto::EnrollResponse>, Status> {
        Ok(Response::new(proto::EnrollResponse {
            node_id: self.node_id.clone(),
            a1_addr: self.a1_addr.clone(),
            cells: self
                .cells
                .read()
                .expect("served cells lock")
                .as_ref()
                .clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::base_station_service_server::BaseStationService;

    #[tokio::test]
    async fn enrollment_reports_the_configured_femto_cell_and_a1_endpoint() {
        let mut config: crate::config::FemtoConfig =
            serde_json::from_str(include_str!("../../../config/femto.json")).unwrap();
        let test_fap: crate::config::UnifiedSite = serde_json::from_str(
            r#"{"feid":"0005B9AABBCC","profile":"airave-25-5x","name":"TestFAP",
                "cell":{"mcc":"310","imsi_11_12":"00","cell_id":403},
                "1x":{"sid":4107,"nid":65535,"band_class":1,"channel":50},
                "acs":{"username":"x","password":"x"},"sip":{"password":"x"},"segw":{}}"#,
        )
        .unwrap();
        config.sites.insert("test-fap".to_string(), test_fap);
        config.normalize();
        let response = FemtoBaseStationService::new(&config)
            .enroll(Request::new(proto::EnrollRequest::default()))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.node_id, "femto-fcs-1");
        assert_eq!(response.a1_addr, "172.31.0.2:17013");
        assert_eq!(response.cells.len(), 1);
        assert_eq!(response.cells[0].sid, 4107);
        assert_eq!(response.cells[0].nid, 65_535);
        assert_eq!(response.cells[0].mcc_digits, "310");
        assert_eq!(response.cells[0].imsi_11_12_digits, "00");
    }
}
