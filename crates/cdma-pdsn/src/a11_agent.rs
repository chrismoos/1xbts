//! PCF-facing A11 agent for the PDSN.

use std::collections::VecDeque;

use crate::session::{PdsnEvent, PdsnSessionManager, Result};

/// Lightweight A11 ingress result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A11AgentEvent {
    A11MessageQueued { queue_depth: usize },
    SessionEvent(PdsnEvent),
}

/// PDSN-side A11 agent.
#[derive(Debug, Default)]
pub struct A11Agent {
    manager: PdsnSessionManager,
    inbound_a11: VecDeque<cdma_a11::Message>,
}

impl A11Agent {
    /// Creates an A11 agent with an empty PDSN session manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the owned session manager.
    pub fn manager(&self) -> &PdsnSessionManager {
        &self.manager
    }

    /// Returns the owned session manager mutably.
    pub fn manager_mut(&mut self) -> &mut PdsnSessionManager {
        &mut self.manager
    }

    /// Queues one decoded A11 message received from the PCF.
    pub fn accept_a11(&mut self, message: cdma_a11::Message) -> A11AgentEvent {
        self.inbound_a11.push_back(message);
        A11AgentEvent::A11MessageQueued {
            queue_depth: self.inbound_a11.len(),
        }
    }

    /// Applies a decoded A11 message directly to the PDSN session manager.
    pub fn apply_a11(
        &mut self,
        now_seconds: u64,
        direction: cdma_a11::Direction,
        message: &cdma_a11::Message,
    ) -> Result<A11AgentEvent> {
        self.manager
            .apply_a11(now_seconds, direction, message)
            .map(A11AgentEvent::SessionEvent)
    }

    /// Pops the next decoded A11 message awaiting procedure handling.
    pub fn pop_inbound_a11(&mut self) -> Option<cdma_a11::Message> {
        self.inbound_a11.pop_front()
    }
}
