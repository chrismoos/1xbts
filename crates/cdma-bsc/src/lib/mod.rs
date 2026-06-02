pub mod a1_edge;
pub mod a9_edge;
pub mod abis_edge;
pub(crate) mod addressing;
pub mod bsc;
#[path = "../config.rs"]
pub mod config;
pub mod grpc;
// Sibling to `grpc` because the generated `msc_management.v1` server code
// references `crate::events::v1::MscNetworkEvent` (proto package `events.v1`).
#[allow(missing_docs)]
pub mod events {
    pub mod v1 {
        tonic::include_proto!("events.v1");
    }
}
pub mod packet;
#[allow(dead_code)]
pub(crate) mod power_control;
#[allow(dead_code)]
pub(crate) mod voice_bearer_bits;
