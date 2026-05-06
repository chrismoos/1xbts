pub mod config;
pub mod model;
pub mod repository;
pub mod service;

pub use config::SmscNodeConfig;

pub mod proto {
    tonic::include_proto!("smsc.v1");
}
