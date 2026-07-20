//! Session state machine driven by decoded inbound session messages.
//!
//! C.S0024-400 §7 defines the message-driven transitions between Closed,
//! AMP Setup, Open, and Closing. `InboundSessionMessage` is the typed boundary
//! after access/traffic decode, so the transition logic stays independent of
//! the bit-level decoders.

use crate::protocols::{NegotiatedProtocols, REV0_DEFAULTS};
use crate::session::{Session, SessionState};
use crate::subnet::{AllocatorError, UatiAllocator};
use crate::uati::Uati;
use thiserror::Error;

/// Inbound session-layer messages from the AT (post-decode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundSessionMessage {
    /// AT requests a UATI (C.S0024-400 §8.3 UATIRequest).
    UatiRequest,
    /// AT requests session configuration / protocol negotiation.
    SessionConfigurationRequest,
    /// AT confirms acceptance of assigned UATI (§8.3 UATIComplete).
    UatiComplete,
    /// AT (or AN-side decision) requests session teardown.
    ConnectionClose,
}

/// Outbound session-layer messages from the AN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundSessionMessage {
    /// Assign a UATI to the AT (§8.3 UATIAssignment).
    UatiAssignment(Uati),
    /// Respond with the negotiated protocol set.
    SessionConfigurationResponse(NegotiatedProtocols),
    /// Tear-down acknowledgement.
    SessionClose,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateMachineError {
    #[error("UATI allocator failed: {0}")]
    Allocator(#[from] AllocatorError),
}

/// Drives a single AT session through the C.S0024-400 §7 state diagram.
#[derive(Debug)]
pub struct SessionStateMachine {
    session: Option<Session>,
    color_code: u8,
}

impl SessionStateMachine {
    pub fn new(color_code: u8) -> Self {
        Self {
            session: None,
            color_code,
        }
    }

    pub fn state(&self) -> SessionState {
        self.session
            .as_ref()
            .map(|s| s.state)
            .unwrap_or(SessionState::Closed)
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn restore_open_session(&mut self, uati: Uati) {
        self.session = Some(Session {
            uati,
            color_code: self.color_code,
            state: SessionState::Open,
            protocols: REV0_DEFAULTS,
        });
    }

    /// Feed an inbound message; returns any outbound messages produced.
    ///
    /// Unhandled (state, message) pairs are dropped silently — Rev 0 only
    /// wires the UATI assignment / session configuration / teardown path.
    pub fn on_message(
        &mut self,
        msg: InboundSessionMessage,
        allocator: &mut UatiAllocator,
    ) -> Result<Vec<OutboundSessionMessage>, StateMachineError> {
        let state = self.state();
        match (state, msg) {
            (SessionState::Closed, InboundSessionMessage::UatiRequest) => {
                self.assign_new_uati(allocator)
            }
            (SessionState::AmpSetup, InboundSessionMessage::UatiRequest) => {
                let uati = self
                    .session
                    .as_ref()
                    .expect("AmpSetup implies session")
                    .uati;
                Ok(vec![OutboundSessionMessage::UatiAssignment(uati)])
            }
            (SessionState::Open, InboundSessionMessage::UatiRequest) => {
                let session = self.session.as_mut().expect("Open implies session");
                session.state = SessionState::AmpSetup;
                Ok(vec![OutboundSessionMessage::UatiAssignment(session.uati)])
            }
            (SessionState::AmpSetup, InboundSessionMessage::SessionConfigurationRequest) => {
                let session = self.session.as_mut().expect("AmpSetup implies session");
                session.protocols = REV0_DEFAULTS;
                Ok(vec![OutboundSessionMessage::SessionConfigurationResponse(
                    session.protocols,
                )])
            }
            (SessionState::AmpSetup, InboundSessionMessage::UatiComplete) => {
                let session = self.session.as_mut().expect("AmpSetup implies session");
                session.state = SessionState::Open;
                Ok(Vec::new())
            }
            (SessionState::Open, InboundSessionMessage::ConnectionClose)
            | (SessionState::AmpSetup, InboundSessionMessage::ConnectionClose) => {
                if let Some(session) = self.session.take() {
                    let uati = session.uati;
                    // Best-effort release; an unknown-UATI error here only
                    // indicates the allocator already reclaimed it.
                    let _ = allocator.release(uati);
                }
                Ok(vec![OutboundSessionMessage::SessionClose])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn assign_new_uati(
        &mut self,
        allocator: &mut UatiAllocator,
    ) -> Result<Vec<OutboundSessionMessage>, StateMachineError> {
        let uati = allocator.allocate()?;
        self.session = Some(Session {
            uati,
            color_code: self.color_code,
            state: SessionState::AmpSetup,
            protocols: REV0_DEFAULTS,
        });
        tracing::debug!(uati = %uati, "assigned UATI");
        Ok(vec![OutboundSessionMessage::UatiAssignment(uati)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subnet::UatiSubnet;

    fn allocator() -> UatiAllocator {
        UatiAllocator::new(UatiSubnet {
            color_code: 1,
            uati104: [0; 13],
            subnet_mask: 24,
        })
    }

    #[test]
    fn happy_path_closed_to_open_to_closing() {
        let mut sm = SessionStateMachine::new(1);
        let mut alloc = allocator();
        assert_eq!(sm.state(), SessionState::Closed);

        // Closed -> AmpSetup
        let out = sm
            .on_message(InboundSessionMessage::UatiRequest, &mut alloc)
            .unwrap();
        assert_eq!(out.len(), 1);
        let assigned = match out[0] {
            OutboundSessionMessage::UatiAssignment(u) => u,
            _ => panic!("expected UatiAssignment"),
        };
        assert_eq!(sm.state(), SessionState::AmpSetup);
        assert_eq!(sm.session().unwrap().uati, assigned);

        // AmpSetup: SessionConfigurationRequest -> response with defaults
        let out = sm
            .on_message(
                InboundSessionMessage::SessionConfigurationRequest,
                &mut alloc,
            )
            .unwrap();
        assert_eq!(
            out,
            vec![OutboundSessionMessage::SessionConfigurationResponse(
                REV0_DEFAULTS
            )]
        );
        assert_eq!(sm.state(), SessionState::AmpSetup);

        // AmpSetup -> Open via UatiComplete
        let out = sm
            .on_message(InboundSessionMessage::UatiComplete, &mut alloc)
            .unwrap();
        assert!(out.is_empty());
        assert_eq!(sm.state(), SessionState::Open);

        // Open -> Closing
        let out = sm
            .on_message(InboundSessionMessage::ConnectionClose, &mut alloc)
            .unwrap();
        assert_eq!(out, vec![OutboundSessionMessage::SessionClose]);
        assert_eq!(sm.state(), SessionState::Closed);
        // UATI returned to pool.
        assert_eq!(alloc.issued_count(), 0);
    }

    #[test]
    fn unexpected_messages_in_closed_are_ignored() {
        let mut sm = SessionStateMachine::new(1);
        let mut alloc = allocator();
        for msg in [
            InboundSessionMessage::SessionConfigurationRequest,
            InboundSessionMessage::UatiComplete,
            InboundSessionMessage::ConnectionClose,
        ] {
            let out = sm.on_message(msg, &mut alloc).unwrap();
            assert!(out.is_empty());
            assert_eq!(sm.state(), SessionState::Closed);
        }
    }

    #[test]
    fn uati_request_in_open_reassigns_current_uati() {
        let mut sm = SessionStateMachine::new(1);
        let mut alloc = allocator();
        let out = sm
            .on_message(InboundSessionMessage::UatiRequest, &mut alloc)
            .unwrap();
        let first = match out[0] {
            OutboundSessionMessage::UatiAssignment(uati) => uati,
            _ => panic!("expected UatiAssignment"),
        };
        sm.on_message(InboundSessionMessage::UatiComplete, &mut alloc)
            .unwrap();
        assert_eq!(sm.state(), SessionState::Open);

        let out = sm
            .on_message(InboundSessionMessage::UatiRequest, &mut alloc)
            .unwrap();
        let second = match out[0] {
            OutboundSessionMessage::UatiAssignment(uati) => uati,
            _ => panic!("expected UatiAssignment"),
        };

        assert_eq!(sm.state(), SessionState::AmpSetup);
        assert_eq!(first, second);
        assert_eq!(alloc.issued_count(), 1);
    }

    #[test]
    fn allocator_exhaustion_surfaces_as_error() {
        let mut alloc = UatiAllocator::new(UatiSubnet {
            color_code: 1,
            uati104: [0; 13],
            subnet_mask: 24,
        });
        alloc.force_exhausted_for_test();
        let mut sm = SessionStateMachine::new(1);
        let err = sm
            .on_message(InboundSessionMessage::UatiRequest, &mut alloc)
            .unwrap_err();
        assert!(matches!(err, StateMachineError::Allocator(_)));
        assert_eq!(sm.state(), SessionState::Closed);
    }

    #[test]
    fn repeated_uati_request_in_amp_setup_retransmits_same_assignment() {
        let mut sm = SessionStateMachine::new(1);
        let mut alloc = allocator();
        let first = sm
            .on_message(InboundSessionMessage::UatiRequest, &mut alloc)
            .unwrap();
        let retry = sm
            .on_message(InboundSessionMessage::UatiRequest, &mut alloc)
            .unwrap();

        assert_eq!(first, retry);
        assert_eq!(alloc.issued_count(), 1);
        assert_eq!(sm.state(), SessionState::AmpSetup);
    }
}
