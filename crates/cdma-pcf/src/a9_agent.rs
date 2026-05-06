//! BSC-facing A9 agent for the PCF.

use std::collections::VecDeque;

use crate::session::{PcfEvent, PcfSessionManager, Result};

/// Lightweight A9 ingress result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A9AgentEvent {
    A9MessageAccepted { queue_depth: usize },
    SessionEvent(PcfEvent),
}

/// PCF-side A9 agent.
#[derive(Debug, Default)]
pub struct A9Agent {
    manager: PcfSessionManager,
    inbound_a9: VecDeque<cdma_a9::Message>,
}

impl A9Agent {
    /// Creates an A9 agent with an empty PCF session manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the owned session manager.
    pub fn manager(&self) -> &PcfSessionManager {
        &self.manager
    }

    /// Returns the owned session manager mutably.
    pub fn manager_mut(&mut self) -> &mut PcfSessionManager {
        &mut self.manager
    }

    /// Accepts one decoded A9 message from the BSC.
    pub fn accept_a9(&mut self, message: cdma_a9::Message) -> A9AgentEvent {
        self.inbound_a9.push_back(message);
        A9AgentEvent::A9MessageAccepted {
            queue_depth: self.inbound_a9.len(),
        }
    }

    /// Starts a PCF session for an accepted A9 setup path.
    pub fn create_session(&mut self, mobile_identity: Option<Vec<u8>>) -> Result<A9AgentEvent> {
        self.manager
            .create_from_a9(mobile_identity)
            .map(A9AgentEvent::SessionEvent)
    }

    /// Applies one inbound typed A9 procedure message from the BSC.
    pub fn apply_inbound_a9(
        &mut self,
        message: cdma_a9::ProcedureMessage,
    ) -> std::result::Result<A9AgentEvent, cdma_a9::Error> {
        self.manager
            .apply_inbound_a9(message)
            .map(A9AgentEvent::SessionEvent)
    }

    /// Applies one outbound typed A9 procedure message toward the BSC.
    pub fn apply_outbound_a9(
        &mut self,
        message: cdma_a9::ProcedureMessage,
    ) -> std::result::Result<A9AgentEvent, cdma_a9::Error> {
        self.manager
            .apply_outbound_a9(message)
            .map(A9AgentEvent::SessionEvent)
    }

    /// Pops the next decoded A9 message awaiting PCF procedure handling.
    pub fn pop_inbound_a9(&mut self) -> Option<cdma_a9::Message> {
        self.inbound_a9.pop_front()
    }
}
