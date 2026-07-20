//! Per-AT session state.
//!
//! C.S0024-400 §7 Session Management Protocol: a session is the shared state
//! between an AT and the AN, comprising the UATI, negotiated protocol
//! subtypes, and the current session state (Closed / AMP Setup / Open /
//! Closing).

use crate::protocols::NegotiatedProtocols;
use crate::uati::Uati;
use serde::{Deserialize, Serialize};

/// Session state, per C.S0024-400 §7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// No session exists for this AT.
    Closed,
    /// Address Management Protocol setup in progress (UATI assigned, awaiting
    /// UATIComplete and protocol negotiation).
    AmpSetup,
    /// Session fully open: UATI assigned, protocols negotiated.
    Open,
    /// Tearing down.
    Closing,
}

/// Per-AT session bookkeeping.
#[derive(Debug, Clone)]
pub struct Session {
    pub uati: Uati,
    pub color_code: u8,
    pub state: SessionState,
    pub protocols: NegotiatedProtocols,
}

impl Session {
    pub fn new(uati: Uati, color_code: u8, protocols: NegotiatedProtocols) -> Self {
        Self {
            uati,
            color_code,
            state: SessionState::Closed,
            protocols,
        }
    }
}
