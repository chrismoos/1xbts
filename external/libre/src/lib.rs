pub mod error;
pub mod runtime;
pub mod sip;

pub use error::{Error, Result};
pub use runtime::{EventLoop, Library, ThreadGuard};
pub use sip::{
    InboundListener, InboundSipMessage, InboundSipSession, InboundSipSessionEventHandler,
    InboundSipSessionHandler, OutboundSipSession, OutboundSipSessionConfig,
    OutboundSipSessionEvent, OutboundSipSessionHandler, SipCredentials, SipRegistration,
    SipRegistrationConfig, SipRegistrationEvent, SipRegistrationHandler, SipSessionSocket,
    SipStack, SipTraceEvent, SipTraceHandler, SipUserAgentConfig, SocketAddress, Transport,
    sip_treply,
};

pub fn native_available() -> bool {
    libre_sys::LIBRE_AVAILABLE
}
