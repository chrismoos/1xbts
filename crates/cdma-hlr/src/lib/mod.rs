pub mod config;
pub mod model;
pub mod prl_proto;
pub mod repository;
pub mod service;

pub use config::HlrNodeConfig;
pub use model::MobileSeenUpsert;

// Nested layout matches the proto package hierarchy so prost's
// generated `super::super::events::v1` paths resolve.
pub mod proto_root {
    pub mod events {
        pub mod v1 {
            tonic::include_proto!("events.v1");
        }
    }
    pub mod hlr {
        pub mod v1 {
            tonic::include_proto!("hlr.v1");
        }
    }
}

pub use proto_root::hlr::v1 as proto;
