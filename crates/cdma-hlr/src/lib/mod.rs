pub mod config;
pub mod model;
pub mod repository;
pub mod service;

pub use config::HlrNodeConfig;
pub use model::MobileSeenUpsert;

pub mod proto {
    tonic::include_proto!("hlr.v1");
}
