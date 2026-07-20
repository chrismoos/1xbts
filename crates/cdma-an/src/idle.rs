//! Default Idle State Protocol (C.S0024-400 §6 / Default Idle State Protocol).
//!
//! Models the AT's air-link state from the AN's point of view while it is
//! camped on a sector: Idle -> ConnectionSetup -> Active, plus a teardown back
//! to Idle. This is the minimal state machine the Air-Link Management Protocol
//! glue needs to drive connection establishment; overhead handling,
//! redirection, and slotted-mode behavior are out of scope.

use crate::subnet::UatiAllocator;
use crate::uati::Uati;

/// AT-side connection state as tracked by the AN.
///
/// C.S0024-400 §6: the Default Idle State Protocol defines the Idle /
/// ConnectionSetup transitions; the Active state is the entry point of the
/// Default Connected State Protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleState {
    /// AT is monitoring the Control Channel; no connection assigned.
    Idle,
    /// AT issued a ConnectionRequest; AN allocated resources and is awaiting
    /// the AT's acknowledgement of the ConnectionAssignment.
    ConnectionSetup,
    /// AT is in the Active state with a live connection.
    Active,
}

impl Default for IdleState {
    fn default() -> Self {
        IdleState::Idle
    }
}

/// Inbound idle-protocol messages from the AT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundIdleMessage {
    /// AT requests a connection (C.S0024-400 §6, ConnectionRequest message).
    ConnectionRequest,
    /// AT acknowledges the AN's ConnectionAssignment, moving to Active.
    ConnectionAssignment,
    /// AT (or AN-initiated) connection close while in setup or active.
    Close,
}

/// Outbound idle-protocol messages from the AN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundIdleMessage {
    /// AN assigns a UATI and forward/reverse channel resources
    /// (C.S0024-400 §6, ConnectionAssignment message).
    ConnectionAssignment { uati: Uati },
    /// AN tears the connection down back to Idle.
    ConnectionClose,
    /// AN denies a ConnectionRequest (e.g. UATI pool exhausted). The reason
    /// code is opaque here.
    ConnectionDeny { reason: u8 },
}

/// Default Idle State Protocol state machine (C.S0024-400 §6).
#[derive(Debug, Default)]
pub struct IdleStateProtocol {
    state: IdleState,
    uati: Option<Uati>,
}

impl IdleStateProtocol {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> IdleState {
        self.state
    }

    pub fn uati(&self) -> Option<Uati> {
        self.uati
    }

    /// Drive the state machine with an inbound message.
    ///
    /// Unhandled (state, message) pairs are dropped silently; only the
    /// happy-path Idle -> ConnectionSetup -> Active and close-from-anywhere
    /// transitions are modelled here.
    pub fn on_message(
        &mut self,
        msg: InboundIdleMessage,
        allocator: &mut UatiAllocator,
    ) -> Vec<OutboundIdleMessage> {
        match (self.state, msg) {
            (IdleState::Idle, InboundIdleMessage::ConnectionRequest) => {
                match allocator.allocate() {
                    Ok(uati) => {
                        self.uati = Some(uati);
                        self.state = IdleState::ConnectionSetup;
                        vec![OutboundIdleMessage::ConnectionAssignment { uati }]
                    }
                    Err(_) => vec![OutboundIdleMessage::ConnectionDeny { reason: 1 }],
                }
            }
            (IdleState::ConnectionSetup, InboundIdleMessage::ConnectionAssignment) => {
                self.state = IdleState::Active;
                Vec::new()
            }
            (IdleState::ConnectionSetup, InboundIdleMessage::Close)
            | (IdleState::Active, InboundIdleMessage::Close) => {
                if let Some(uati) = self.uati.take() {
                    let _ = allocator.release(uati);
                }
                self.state = IdleState::Idle;
                vec![OutboundIdleMessage::ConnectionClose]
            }
            _ => Vec::new(),
        }
    }
}

/// Pure-function state machine variant of the Idle protocol. Drives the FSM
/// without owning a UATI allocator.
pub mod sm {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum IdleStateMachine {
        Inactive,
        Sleep,
        Monitor,
        ConnectionSetup,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum InboundEvent {
        PageReceived,
        SlotTick,
        ConnectionRequestReceived,
        EnterSleep,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum OutboundEvent {
        WakeMonitor,
        SendConnectionAssignment,
        EnterConnectionSetup,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        IllegalTransition,
    }

    pub fn next(
        state: IdleStateMachine,
        event: InboundEvent,
    ) -> Result<(IdleStateMachine, Vec<OutboundEvent>), Error> {
        use IdleStateMachine as S;
        use InboundEvent as E;
        match (state, event) {
            (S::Inactive, E::PageReceived) | (S::Sleep, E::PageReceived) => {
                Ok((S::Monitor, vec![OutboundEvent::WakeMonitor]))
            }
            (S::Monitor, E::SlotTick) => Ok((S::Monitor, vec![])),
            (S::Inactive, E::SlotTick) | (S::Sleep, E::SlotTick) => Ok((state, vec![])),
            (S::Monitor, E::ConnectionRequestReceived)
            | (S::Inactive, E::ConnectionRequestReceived) => Ok((
                S::ConnectionSetup,
                vec![
                    OutboundEvent::EnterConnectionSetup,
                    OutboundEvent::SendConnectionAssignment,
                ],
            )),
            (S::Monitor, E::EnterSleep) | (S::Inactive, E::EnterSleep) => Ok((S::Sleep, vec![])),
            (S::ConnectionSetup, _) => Err(Error::IllegalTransition),
            _ => Err(Error::IllegalTransition),
        }
    }

    #[cfg(test)]
    mod sm_tests {
        use super::*;

        #[test]
        fn page_in_inactive_wakes_monitor() {
            let (s, e) = next(IdleStateMachine::Inactive, InboundEvent::PageReceived).unwrap();
            assert_eq!(s, IdleStateMachine::Monitor);
            assert_eq!(e, vec![OutboundEvent::WakeMonitor]);
        }

        #[test]
        fn page_in_sleep_wakes_monitor() {
            let (s, _) = next(IdleStateMachine::Sleep, InboundEvent::PageReceived).unwrap();
            assert_eq!(s, IdleStateMachine::Monitor);
        }

        #[test]
        fn slot_tick_in_monitor_keeps_monitor() {
            let (s, e) = next(IdleStateMachine::Monitor, InboundEvent::SlotTick).unwrap();
            assert_eq!(s, IdleStateMachine::Monitor);
            assert!(e.is_empty());
        }

        #[test]
        fn slot_tick_in_inactive_idempotent() {
            let (s, _) = next(IdleStateMachine::Inactive, InboundEvent::SlotTick).unwrap();
            assert_eq!(s, IdleStateMachine::Inactive);
        }

        #[test]
        fn connection_request_from_monitor_enters_setup() {
            let (s, e) = next(
                IdleStateMachine::Monitor,
                InboundEvent::ConnectionRequestReceived,
            )
            .unwrap();
            assert_eq!(s, IdleStateMachine::ConnectionSetup);
            assert!(e.contains(&OutboundEvent::EnterConnectionSetup));
            assert!(e.contains(&OutboundEvent::SendConnectionAssignment));
        }

        #[test]
        fn sleep_from_monitor_transitions_to_sleep() {
            let (s, _) = next(IdleStateMachine::Monitor, InboundEvent::EnterSleep).unwrap();
            assert_eq!(s, IdleStateMachine::Sleep);
        }

        #[test]
        fn page_in_connection_setup_is_illegal() {
            let err = next(
                IdleStateMachine::ConnectionSetup,
                InboundEvent::PageReceived,
            );
            assert_eq!(err, Err(Error::IllegalTransition));
        }

        #[test]
        fn slot_tick_in_connection_setup_is_illegal() {
            let err = next(IdleStateMachine::ConnectionSetup, InboundEvent::SlotTick);
            assert_eq!(err, Err(Error::IllegalTransition));
        }
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
    fn connection_request_in_idle_allocates_uati_and_moves_to_setup() {
        let mut proto = IdleStateProtocol::new();
        let mut alloc = allocator();
        assert_eq!(proto.state(), IdleState::Idle);

        let out = proto.on_message(InboundIdleMessage::ConnectionRequest, &mut alloc);
        assert_eq!(out.len(), 1);
        let assigned = match out[0] {
            OutboundIdleMessage::ConnectionAssignment { uati } => uati,
            ref other => panic!("unexpected outbound: {other:?}"),
        };
        assert_eq!(proto.state(), IdleState::ConnectionSetup);
        assert_eq!(proto.uati(), Some(assigned));
    }

    #[test]
    fn connection_assignment_ack_moves_to_active() {
        let mut proto = IdleStateProtocol::new();
        let mut alloc = allocator();
        proto.on_message(InboundIdleMessage::ConnectionRequest, &mut alloc);
        let out = proto.on_message(InboundIdleMessage::ConnectionAssignment, &mut alloc);
        assert!(out.is_empty());
        assert_eq!(proto.state(), IdleState::Active);
    }

    #[test]
    fn close_from_active_returns_to_idle_and_releases_uati() {
        let mut proto = IdleStateProtocol::new();
        let mut alloc = allocator();
        proto.on_message(InboundIdleMessage::ConnectionRequest, &mut alloc);
        proto.on_message(InboundIdleMessage::ConnectionAssignment, &mut alloc);
        assert_eq!(alloc.issued_count(), 1);

        let out = proto.on_message(InboundIdleMessage::Close, &mut alloc);
        assert_eq!(out, vec![OutboundIdleMessage::ConnectionClose]);
        assert_eq!(proto.state(), IdleState::Idle);
        assert_eq!(proto.uati(), None);
        assert_eq!(alloc.issued_count(), 0);
    }

    #[test]
    fn allocator_exhaustion_emits_deny_and_stays_idle() {
        let mut alloc = UatiAllocator::new(UatiSubnet {
            color_code: 1,
            uati104: [0; 13],
            subnet_mask: 24,
        });
        alloc.force_exhausted_for_test();
        let mut proto = IdleStateProtocol::new();
        let out = proto.on_message(InboundIdleMessage::ConnectionRequest, &mut alloc);
        assert_eq!(out, vec![OutboundIdleMessage::ConnectionDeny { reason: 1 }]);
        assert_eq!(proto.state(), IdleState::Idle);
    }
}
