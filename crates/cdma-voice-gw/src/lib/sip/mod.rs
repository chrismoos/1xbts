mod libre_backend;

use std::net::SocketAddr;

use async_trait::async_trait;
use thiserror::Error as ThisError;
use tokio::sync::{broadcast, mpsc};

use crate::proto::{AirVoiceFrame, GatewayVoiceFrame, ReleaseReason};

pub use libre_backend::LibreSipBackend;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundSipCall {
    pub session_id: String,
    pub called_number: String,
    pub caller_number: String,
    pub service_option: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SipBackendEvent {
    Ringing {
        session_id: String,
        sip_status: u16,
        /// SDP-derived codec; empty if provisional had no SDP.
        codec: String,
    },
    Answered {
        session_id: String,
        sip_status: u16,
        codec: String,
    },
    Failed {
        session_id: String,
        sip_status: Option<u16>,
        reason: String,
    },
    Released {
        session_id: String,
        reason: ReleaseReason,
    },
    InboundInvite {
        session_id: String,
        called_number: String,
        caller_number: String,
        caller_display: String,
        offered_codecs: Vec<String>,
    },
    InboundCancel {
        session_id: String,
    },
}

pub type SipBackendEventSender = mpsc::UnboundedSender<SipBackendEvent>;

#[derive(Debug, ThisError)]
pub enum SipBackendError {
    #[error("SIP backend is unavailable: {0}")]
    Unavailable(String),

    #[error("SIP backend configuration error: {0}")]
    Config(String),

    #[error("failed to listen for SIP on {transport} {listen_addr}: {source}")]
    Listen {
        listen_addr: SocketAddr,
        transport: String,
        #[source]
        source: libre::Error,
    },

    #[error("failed to listen for SIP on {transport} {listen_addr}: {source}")]
    ListenPreflight {
        listen_addr: SocketAddr,
        transport: String,
        #[source]
        source: std::io::Error,
    },

    #[error("native libre/re error: {0}")]
    Libre(#[from] libre::Error),

    #[error("media error: {0}")]
    Media(String),
}

pub type Result<T> = std::result::Result<T, SipBackendError>;

#[async_trait]
pub trait SipBackend: Send + Sync + 'static {
    async fn start_outbound_call(
        &self,
        call: OutboundSipCall,
        events: SipBackendEventSender,
    ) -> Result<()>;

    async fn release_call(&self, session_id: &str, reason: ReleaseReason) -> Result<()>;

    async fn handle_air_frame(&self, frame: AirVoiceFrame) -> Result<()>;

    fn subscribe_gateway_voice_frames(&self) -> broadcast::Receiver<GatewayVoiceFrame>;

    async fn register_inbound_handler(&self, _events: SipBackendEventSender) -> Result<()> {
        Ok(())
    }

    async fn inbound_progress(&self, _session_id: &str) -> Result<()> {
        Err(SipBackendError::Unavailable(
            "inbound SIP not available — no SIP backend configured".to_string(),
        ))
    }

    async fn inbound_answer(&self, _session_id: &str, _codec: &str) -> Result<()> {
        Err(SipBackendError::Unavailable(
            "inbound SIP not available — no SIP backend configured".to_string(),
        ))
    }

    async fn inbound_reject(&self, _session_id: &str, _sip_status: u16) -> Result<()> {
        Ok(())
    }

    async fn send_dtmf(&self, _event: SendDtmfRequest) -> Result<()> {
        Ok(())
    }
}

/// RFC 4733 telephone-event request for the gateway's SIP RTP stream.
#[derive(Debug, Clone)]
pub struct SendDtmfRequest {
    pub session_id: String,
    pub event_code: u8,
    pub volume: u8,
    pub duration_samples: u16,
    pub end: bool,
    pub start_of_event: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UnavailableSipBackend;

#[async_trait]
impl SipBackend for UnavailableSipBackend {
    async fn start_outbound_call(
        &self,
        call: OutboundSipCall,
        _events: SipBackendEventSender,
    ) -> Result<()> {
        Err(SipBackendError::Unavailable(format!(
            "cannot start outbound SIP call for session {} because no SIP backend is configured",
            call.session_id
        )))
    }

    async fn release_call(&self, session_id: &str, _reason: ReleaseReason) -> Result<()> {
        log::debug!(
            "ignoring release for SIP session {} because no SIP backend is configured",
            session_id
        );
        Ok(())
    }

    async fn handle_air_frame(&self, frame: AirVoiceFrame) -> Result<()> {
        log::trace!(
            "dropping air voice frame for session {} because SIP backend is unavailable",
            frame.session_id
        );
        Ok(())
    }

    fn subscribe_gateway_voice_frames(&self) -> broadcast::Receiver<GatewayVoiceFrame> {
        let (_tx, rx) = broadcast::channel(1);
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sip_listen_error_display_includes_bind_context_and_native_source() {
        let err = SipBackendError::Listen {
            listen_addr: "127.0.0.1:5060".parse().unwrap(),
            transport: "udp".to_string(),
            source: libre::Error::Native {
                operation: "cdma_libre_sip_transp_add",
                status: 48,
            },
        };

        let message = err.to_string();
        assert!(message.contains("failed to listen for SIP on udp 127.0.0.1:5060"));
        assert!(message.contains("cdma_libre_sip_transp_add"));
        assert!(message.contains("48"));
    }

    #[test]
    fn generic_libre_error_display_includes_native_source() {
        let err = SipBackendError::Libre(libre::Error::Native {
            operation: "cdma_libre_sipsess_listen",
            status: 98,
        });

        let message = err.to_string();
        assert!(message.contains("native libre/re error"));
        assert!(message.contains("cdma_libre_sipsess_listen"));
        assert!(message.contains("98"));
    }
}
