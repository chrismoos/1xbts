//! HRPD (1xEV-DO Rev 0) Access Network process.
//!
//! Owns the session layer (UATI assignment, protocol negotiation), connection
//! layer (idle/connected state, route update, overhead delivery), security
//! pass-through, and stream layer for an HRPD sector. Runs as its own process
//! and talks to BTS / PCF / 1x BSC only over network reference points
//! (A8/A9/A21).

pub mod a8_runtime;
pub mod air;
pub mod connection;
pub mod events;
pub mod grpc;
pub mod hrpd_a9_client;
pub mod hrpd_identity;
pub mod identity_broker;
pub mod idle;
pub mod packet;
pub mod protocols;
pub mod rlp;
pub mod route_update;
pub mod security;
pub mod session;
pub mod state_machine;
pub mod stream;
pub mod subnet;
pub mod uati;

pub use a8_runtime::{HrpdAnA8Runtime, HrpdAnForwardTrafficPacket, spawn_hrpd_an_a8_runtime};
pub use hrpd_a9_client::{
    HrpdA9ReleaseContext, HrpdAnA9Client, build_setup_a8, validate_connect_a8,
};
pub use hrpd_identity::{
    HrpdA9MobileIdentity, HrpdDerivedImsiConfig, HrpdHardwareIdentity, derive_hrpd_imsi,
    hardware_identity_from_response, resolve_hrpd_a9_identity,
};
pub use protocols::{NegotiatedProtocols, ProtocolSubtype, REV0_DEFAULTS};
pub use security::{SecurityError, SecurityLayer, SecuritySubtype};
pub use session::{Session, SessionState};
pub use state_machine::{
    InboundSessionMessage, OutboundSessionMessage, SessionStateMachine, StateMachineError,
};
pub use subnet::{AllocatorError, UatiAllocator, UatiSubnet};
pub use uati::Uati;

pub mod proto {
    pub mod an {
        pub mod v1 {
            tonic::include_proto!("an.v1");
        }
    }
    pub mod events {
        pub mod v1 {
            tonic::include_proto!("events.v1");
        }
    }
}

#[cfg(test)]
mod proto_tests {
    use super::proto::an::v1::{Session, SessionState};
    use prost::Message;

    #[test]
    fn session_roundtrips_through_prost() {
        let session = Session {
            uati: 0x1234_5678,
            color_code: 42,
            state: SessionState::Open as i32,
            protocols: None,
            hardware_id_response: None,
            full_uati: None,
        };
        let bytes = session.encode_to_vec();
        let decoded = Session::decode(&*bytes).expect("decode");
        assert_eq!(decoded.uati, 0x1234_5678);
        assert_eq!(decoded.color_code, 42);
        assert_eq!(decoded.state(), SessionState::Open);
    }
}
