pub mod config;
pub mod media;
pub mod service;
pub mod sip;
pub(crate) mod stun;

pub mod proto {
    tonic::include_proto!("voice_gateway.v1");
}
