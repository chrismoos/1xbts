//! Default Connected State Protocol (C.S0024-400 §6 / Default Connected State
//! Protocol).
//!
//! While the AT is in the Active state the AN handles KeepAlive exchanges and
//! Close requests. This module captures only the message-driven happy path:
//! `KeepAlive` updates the last-seen slot and yields a `KeepAliveAck`, `Close`
//! deactivates the connection and yields a `Close` reply.

/// Default Connected State Protocol context (C.S0024-400 §6).
#[derive(Debug, Default)]
pub struct ConnectedStateProtocol {
    active: bool,
    last_keepalive_slot: u64,
}

/// Inbound connected-protocol messages from the AT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundConnectedMessage {
    /// AT periodic KeepAlive while the connection is up.
    KeepAlive,
    /// AT-initiated connection teardown.
    Close,
}

/// Outbound connected-protocol messages from the AN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundConnectedMessage {
    /// AN acknowledges the AT's KeepAlive.
    KeepAliveAck,
    /// AN acknowledges (or initiates) connection close.
    Close,
}

impl ConnectedStateProtocol {
    /// Build a fresh, inactive context. Transition to active on the first
    /// inbound message; the AT is assumed to already be in the Active state
    /// when this protocol is driven (after the Default Idle State Protocol
    /// reached `IdleState::Active`).
    pub fn new() -> Self {
        Self {
            active: true,
            last_keepalive_slot: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn last_keepalive_slot(&self) -> u64 {
        self.last_keepalive_slot
    }

    /// Drive the protocol with an inbound message at the given slot.
    pub fn on_message(
        &mut self,
        msg: InboundConnectedMessage,
        slot: u64,
    ) -> Vec<OutboundConnectedMessage> {
        if !self.active {
            return Vec::new();
        }
        match msg {
            InboundConnectedMessage::KeepAlive => {
                self.last_keepalive_slot = slot;
                vec![OutboundConnectedMessage::KeepAliveAck]
            }
            InboundConnectedMessage::Close => {
                self.active = false;
                vec![OutboundConnectedMessage::Close]
            }
        }
    }
}

/// Pure-function Connection Layer state machine (C.S0024-400 §6). Models the
/// connection lifecycle from idle → setup → open → closing → closed.
pub mod sm {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ConnectionStateMachine {
        Closed,
        Setup,
        Open,
        Closing,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum InboundEvent {
        AssignmentFromIdle,
        AssignmentAck,
        CloseReceived,
        CloseAck,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum OutboundEvent {
        OpenTrafficChannel,
        CloseTrafficChannel,
        SendClose,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Error {
        IllegalTransition,
    }

    pub fn next(
        state: ConnectionStateMachine,
        event: InboundEvent,
    ) -> Result<(ConnectionStateMachine, Vec<OutboundEvent>), Error> {
        use ConnectionStateMachine::*;
        use InboundEvent::*;
        match (state, event) {
            (Closed, AssignmentFromIdle) => Ok((Setup, vec![OutboundEvent::OpenTrafficChannel])),
            (Setup, AssignmentAck) => Ok((Open, vec![])),
            (Open, CloseReceived) => Ok((
                Closing,
                vec![OutboundEvent::SendClose, OutboundEvent::CloseTrafficChannel],
            )),
            (Setup, CloseReceived) => Ok((Closing, vec![OutboundEvent::CloseTrafficChannel])),
            (Closing, CloseAck) => Ok((Closed, vec![])),
            _ => Err(Error::IllegalTransition),
        }
    }

    #[cfg(test)]
    mod sm_tests {
        use super::*;

        #[test]
        fn happy_path_closed_setup_open_closing_closed() {
            let (s, e) = next(
                ConnectionStateMachine::Closed,
                InboundEvent::AssignmentFromIdle,
            )
            .unwrap();
            assert_eq!(s, ConnectionStateMachine::Setup);
            assert_eq!(e, vec![OutboundEvent::OpenTrafficChannel]);
            let (s, e) = next(s, InboundEvent::AssignmentAck).unwrap();
            assert_eq!(s, ConnectionStateMachine::Open);
            assert!(e.is_empty());
            let (s, e) = next(s, InboundEvent::CloseReceived).unwrap();
            assert_eq!(s, ConnectionStateMachine::Closing);
            assert!(e.contains(&OutboundEvent::SendClose));
            let (s, _) = next(s, InboundEvent::CloseAck).unwrap();
            assert_eq!(s, ConnectionStateMachine::Closed);
        }

        #[test]
        fn close_while_in_setup_tears_down_without_send() {
            let (s, e) = next(ConnectionStateMachine::Setup, InboundEvent::CloseReceived).unwrap();
            assert_eq!(s, ConnectionStateMachine::Closing);
            assert_eq!(e, vec![OutboundEvent::CloseTrafficChannel]);
        }

        #[test]
        fn assignment_ack_in_closed_is_illegal() {
            assert_eq!(
                next(ConnectionStateMachine::Closed, InboundEvent::AssignmentAck),
                Err(Error::IllegalTransition)
            );
        }

        #[test]
        fn close_ack_in_open_is_illegal() {
            assert_eq!(
                next(ConnectionStateMachine::Open, InboundEvent::CloseAck),
                Err(Error::IllegalTransition)
            );
        }

        #[test]
        fn assignment_from_idle_in_open_is_illegal() {
            assert_eq!(
                next(
                    ConnectionStateMachine::Open,
                    InboundEvent::AssignmentFromIdle
                ),
                Err(Error::IllegalTransition)
            );
        }

        #[test]
        fn close_received_in_closed_is_illegal() {
            assert_eq!(
                next(ConnectionStateMachine::Closed, InboundEvent::CloseReceived),
                Err(Error::IllegalTransition)
            );
        }

        #[test]
        fn close_received_in_closing_is_illegal() {
            assert_eq!(
                next(ConnectionStateMachine::Closing, InboundEvent::CloseReceived),
                Err(Error::IllegalTransition)
            );
        }

        #[test]
        fn assignment_ack_in_open_is_illegal() {
            assert_eq!(
                next(ConnectionStateMachine::Open, InboundEvent::AssignmentAck),
                Err(Error::IllegalTransition)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_bumps_slot_and_acks() {
        let mut proto = ConnectedStateProtocol::new();
        assert!(proto.is_active());
        let out = proto.on_message(InboundConnectedMessage::KeepAlive, 1_234);
        assert_eq!(out, vec![OutboundConnectedMessage::KeepAliveAck]);
        assert_eq!(proto.last_keepalive_slot(), 1_234);

        let out = proto.on_message(InboundConnectedMessage::KeepAlive, 5_678);
        assert_eq!(out, vec![OutboundConnectedMessage::KeepAliveAck]);
        assert_eq!(proto.last_keepalive_slot(), 5_678);
    }

    #[test]
    fn close_deactivates_and_emits_close() {
        let mut proto = ConnectedStateProtocol::new();
        let out = proto.on_message(InboundConnectedMessage::Close, 42);
        assert_eq!(out, vec![OutboundConnectedMessage::Close]);
        assert!(!proto.is_active());

        // Subsequent messages while inactive are dropped.
        let out = proto.on_message(InboundConnectedMessage::KeepAlive, 100);
        assert!(out.is_empty());
    }
}
