pub mod error;
pub mod runtime;
pub mod sip;

pub use error::{Error, Result};
pub use runtime::{EventLoop, Library, ThreadGuard};
pub use sip::{
    OutboundSipSession, OutboundSipSessionConfig, OutboundSipSessionEvent,
    OutboundSipSessionHandler, SipCredentials, SipRegistration, SipRegistrationConfig,
    SipRegistrationEvent, SipRegistrationHandler, SipSessionSocket, SipStack, SipTraceEvent,
    SipTraceHandler, SipUserAgentConfig, SocketAddress, Transport,
};

pub fn native_available() -> bool {
    libre_sys::LIBRE_AVAILABLE
}
