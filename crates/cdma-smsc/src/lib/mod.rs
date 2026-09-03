pub mod config;
pub mod model;
pub mod node;
pub mod repository;
pub mod service;

pub use config::SmscNodeConfig;
pub use node::{resolve_config_dir, run_node};

pub mod proto {
    tonic::include_proto!("smsc.v1");
}
