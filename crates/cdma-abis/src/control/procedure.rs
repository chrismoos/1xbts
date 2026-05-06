//! Local procedure/state helpers for Abis control flows.

use std::collections::BTreeSet;

use super::ies::ElementId;
use super::messages::MessageType;
use super::{
    AbisAckNotify, AbisDestinationId, AbisMessage, AchMessageTransferMessage,
    AirInterfaceMessagePayload, AuthenticationChallengeParameter, BtsReleaseAckMessage,
    BtsReleaseMessage, BtsReleaseRequestMessage, BtsSetupAckMessage, BtsSetupMessage,
    BurstCommitMessage, BurstRequestMessage, BurstResponseMessage, CallConnectionReference, CellId,
    ConnectAckMessage, ConnectMessage, CorrelationId, ForwardBurstRadioInfo,
    Layer2AckRequestResults, MobileIdentity, PacaActionRequired, PacaUpdateMessage,
    PchMessageTransferAckMessage, PchMessageTransferMessage, RemoveAckMessage, RemoveMessage,
    ReverseBurstRadioInfo, TrafficChannelStatusMessage,
};
use crate::{Error, Result};

/// Abis control-plane timer identifiers defined by the Abis procedure sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AbisTimerKind {
    /// BTS timer waiting for `Connect Ack` after `Connect`.
    Tconnb,
    /// BSC timer waiting for progress after `BTS Setup`.
    Tsetupb,
    /// BSC timer waiting for `Traffic Channel Status`.
    Tchanstatb,
    /// BTS timer waiting for `Remove Ack`.
    Tdisconb,
    /// BSC timer waiting for `BTS Release Ack`.
    Tdrptgtb,
    /// BSC timer waiting for `Burst Response`.
    Tbstreqb,
    /// BTS timer waiting for `Burst Commit`.
    Tbstcomb,
    /// BTS timer waiting for `BTS Release` after `BTS Release Request`.
    Trelreqb,
}

/// Static timer metadata taken from the Abis timer table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerDefinition {
    /// Default timer value in milliseconds.
    pub default_ms: u16,
    /// Minimum allowed timer value in milliseconds.
    pub min_ms: u16,
    /// Maximum allowed timer value in milliseconds.
    pub max_ms: u16,
    /// Timer granularity in milliseconds.
    pub granularity_ms: u16,
}

impl AbisTimerKind {
    /// Returns the table-defined timer metadata for this timer.
    pub const fn definition(self) -> TimerDefinition {
        match self {
            AbisTimerKind::Tconnb => TimerDefinition {
                default_ms: 100,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
            AbisTimerKind::Tsetupb => TimerDefinition {
                default_ms: 100,
                min_ms: 0,
                max_ms: 500,
                granularity_ms: 100,
            },
            AbisTimerKind::Tchanstatb => TimerDefinition {
                default_ms: 500,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
            AbisTimerKind::Tdisconb => TimerDefinition {
                default_ms: 100,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
            AbisTimerKind::Tdrptgtb => TimerDefinition {
                default_ms: 500,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
            AbisTimerKind::Tbstreqb => TimerDefinition {
                default_ms: 500,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
            AbisTimerKind::Tbstcomb => TimerDefinition {
                default_ms: 500,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
            AbisTimerKind::Trelreqb => TimerDefinition {
                default_ms: 100,
                min_ms: 0,
                max_ms: 1000,
                granularity_ms: 100,
            },
        }
    }
}

/// Setup/connect procedure state for a single traffic connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficSetupState {
    /// No setup/connect exchange is active.
    Idle,
    /// A `BTS Setup` request has been issued and the procedure is waiting for `Connect`.
    AwaitingConnect,
    /// `Connect` was accepted and the procedure is waiting for `Connect Ack`.
    AwaitingConnectAck,
    /// `Connect Ack` was sent and the procedure is waiting for final setup completion.
    AwaitingSetupCompletion,
    /// The setup/connect exchange completed successfully.
    Connected,
    /// The setup/connect exchange failed and must be restarted.
    Failed,
}

/// Outcome of processing a `BTS Setup Ack`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAckOutcome {
    /// The setup request succeeded and the procedure can advance to `Connect`.
    Accepted,
    /// The setup request failed with the supplied cause value.
    Rejected { cause: u8 },
}

/// Action implied by expiry of a setup/connect timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficSetupTimeoutAction {
    /// The BSC may resend `BTS Setup`.
    ResendBtsSetup,
    /// The BTS may resend `Connect`.
    ResendConnect,
    /// The BSC may release cells that never reported traffic-channel status.
    ReleaseUnreportedCells,
}

/// Crate-local state tracker for `BTS Setup` / `Connect` / `Traffic Channel Status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficSetupProcedure {
    call_connection_reference: CallConnectionReference,
    state: TrafficSetupState,
    expected_correlation_id: Option<CorrelationId>,
    last_status: Option<TrafficChannelStatusMessage>,
    active_timers: BTreeSet<AbisTimerKind>,
    pending_status_cells: Vec<(u16, u8)>,
    setup_ack_received: bool,
}

impl TrafficSetupProcedure {
    /// Creates a new setup/connect procedure tracker for a single call reference.
    pub fn new(call_connection_reference: CallConnectionReference) -> Self {
        Self {
            call_connection_reference,
            state: TrafficSetupState::Idle,
            expected_correlation_id: None,
            last_status: None,
            active_timers: BTreeSet::new(),
            pending_status_cells: Vec::new(),
            setup_ack_received: false,
        }
    }

    /// Returns the call reference bound to this procedure instance.
    pub fn call_connection_reference(&self) -> CallConnectionReference {
        self.call_connection_reference
    }

    /// Returns the current setup/connect state.
    pub fn state(&self) -> TrafficSetupState {
        self.state
    }

    /// Returns the most recent traffic-channel status message accepted by the procedure.
    pub fn last_status(&self) -> Option<&TrafficChannelStatusMessage> {
        self.last_status.as_ref()
    }

    /// Returns the timers currently active for this setup/connect exchange.
    pub fn active_timers(&self) -> Vec<AbisTimerKind> {
        self.active_timers.iter().copied().collect()
    }

    /// Returns the number of cell-status reports still expected by the BSC.
    pub fn pending_status_count(&self) -> usize {
        self.pending_status_cells.len()
    }

    /// Starts the procedure with a `BTS Setup` request.
    pub fn start_setup(&mut self, message: &BtsSetupMessage) -> Result<()> {
        self.ensure_state(
            TrafficSetupState::Idle,
            MessageType::BtsSetup,
            "setup procedure already active",
        )?;
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::BtsSetup,
        )?;
        self.expected_correlation_id = None;
        self.last_status = None;
        self.active_timers.clear();
        self.active_timers.insert(AbisTimerKind::Tsetupb);
        self.pending_status_cells.clear();
        self.setup_ack_received = false;
        self.state = TrafficSetupState::AwaitingConnect;
        Ok(())
    }

    /// Processes a `BTS Setup Ack` response and returns whether setup succeeded.
    pub fn on_setup_ack(&mut self, message: &BtsSetupAckMessage) -> Result<SetupAckOutcome> {
        self.ensure_state(
            TrafficSetupState::AwaitingSetupCompletion,
            MessageType::BtsSetupAck,
            "unexpected BTS Setup Ack for current procedure state",
        )?;
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::BtsSetupAck,
        )?;
        if let Some(cause) = message.cause {
            self.state = TrafficSetupState::Failed;
            return Ok(SetupAckOutcome::Rejected { cause });
        }
        self.setup_ack_received = true;
        if !self.active_timers.contains(&AbisTimerKind::Tchanstatb) {
            self.state = TrafficSetupState::Connected;
        }
        Ok(SetupAckOutcome::Accepted)
    }

    /// Processes the `Connect` message that follows `BTS Setup`.
    pub fn on_connect(&mut self, message: &ConnectMessage) -> Result<()> {
        self.ensure_state(
            TrafficSetupState::AwaitingConnect,
            MessageType::Connect,
            "unexpected Connect for current procedure state",
        )?;
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::Connect,
        )?;
        self.active_timers.remove(&AbisTimerKind::Tsetupb);
        self.active_timers.insert(AbisTimerKind::Tconnb);
        self.expected_correlation_id = message.correlation_id;
        self.pending_status_cells = message
            .connect_information
            .iter()
            .flat_map(|information| information.cell_info_records.iter())
            .filter(|record| record.new_cell)
            .map(|record| (record.cell.cell, record.cell.sector))
            .collect();
        self.state = TrafficSetupState::AwaitingConnectAck;
        Ok(())
    }

    /// Completes the procedure with `Connect Ack`.
    pub fn on_connect_ack(&mut self, message: &ConnectAckMessage) -> Result<()> {
        self.ensure_state(
            TrafficSetupState::AwaitingConnectAck,
            MessageType::ConnectAck,
            "unexpected Connect Ack for current procedure state",
        )?;
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::ConnectAck,
        )?;
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::ConnectAck,
        )?;
        self.active_timers.remove(&AbisTimerKind::Tconnb);
        if message
            .connect_ack_information
            .iter()
            .any(|information| information.transmit_tch_status)
            && !self.pending_status_cells.is_empty()
        {
            self.active_timers.insert(AbisTimerKind::Tchanstatb);
        } else {
            self.active_timers.remove(&AbisTimerKind::Tchanstatb);
            self.pending_status_cells.clear();
        }
        if self.setup_ack_received && !self.active_timers.contains(&AbisTimerKind::Tchanstatb) {
            self.state = TrafficSetupState::Connected;
        } else {
            self.state = TrafficSetupState::AwaitingSetupCompletion;
        }
        Ok(())
    }

    /// Records the latest `Traffic Channel Status` after the connection is established.
    pub fn on_traffic_channel_status(
        &mut self,
        message: &TrafficChannelStatusMessage,
    ) -> Result<()> {
        if self.state != TrafficSetupState::AwaitingSetupCompletion
            && self.state != TrafficSetupState::Connected
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::TrafficChannelStatus.value(),
                reason: "traffic channel status is only valid after Connect Ack",
            });
        }
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::TrafficChannelStatus,
        )?;
        self.last_status = Some(message.clone());
        for cell in &message.cell_identifier_list {
            self.pending_status_cells
                .retain(|candidate| *candidate != (cell.cell, cell.sector));
        }
        if self.pending_status_cells.is_empty() {
            self.active_timers.remove(&AbisTimerKind::Tchanstatb);
            if self.setup_ack_received {
                self.state = TrafficSetupState::Connected;
            }
        }
        Ok(())
    }

    /// Processes expiry of a setup/connect timer.
    pub fn on_timer_expiry(&mut self, timer: AbisTimerKind) -> Result<TrafficSetupTimeoutAction> {
        if !self.active_timers.contains(&timer) {
            return Err(Error::InvalidValue {
                context: "Traffic setup timer",
                reason: "timer is not active",
            });
        }
        match timer {
            AbisTimerKind::Tsetupb => Ok(TrafficSetupTimeoutAction::ResendBtsSetup),
            AbisTimerKind::Tconnb => Ok(TrafficSetupTimeoutAction::ResendConnect),
            AbisTimerKind::Tchanstatb => {
                self.active_timers.remove(&AbisTimerKind::Tchanstatb);
                Ok(TrafficSetupTimeoutAction::ReleaseUnreportedCells)
            }
            _ => Err(Error::InvalidValue {
                context: "Traffic setup timer",
                reason: "timer is not valid for setup/connect procedure",
            }),
        }
    }

    fn ensure_state(
        &self,
        expected: TrafficSetupState,
        message_type: MessageType,
        reason: &'static str,
    ) -> Result<()> {
        if self.state != expected {
            return Err(Error::InvalidMessage {
                message_type: message_type.value(),
                reason,
            });
        }
        Ok(())
    }
}

/// Release/remove procedure state for a single traffic connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficReleaseState {
    /// No release/remove exchange is active.
    Idle,
    /// A `BTS Release` request is outstanding.
    AwaitingBtsReleaseAck,
    /// The procedure received `BTS Release Request` and is waiting for `Remove`.
    AwaitingRemove,
    /// `Remove` was received and the local side must answer with `Remove Ack`.
    AwaitingRemoveAck,
    /// The release/remove exchange completed successfully.
    Released,
}

/// Summary of a received `BTS Release Request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BtsReleaseRequestDisposition {
    /// Optional cause carried by the request.
    pub cause: Option<u8>,
    /// Whether manufacturer-specific records were present.
    pub has_manufacturer_specific_records: bool,
}

/// Action implied by expiry of a release/remove timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficReleaseTimeoutAction {
    /// The BSC may resend `BTS Release`.
    ResendBtsRelease,
    /// The BTS may resend `Remove`.
    ResendRemove,
    /// The BTS may resend `BTS Release Request`.
    ResendBtsReleaseRequest,
}

/// Crate-local state tracker for `BTS Release`, `BTS Release Ack`, `BTS Release Request`,
/// `Remove`, and `Remove Ack`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficReleaseProcedure {
    call_connection_reference: CallConnectionReference,
    state: TrafficReleaseState,
    expected_correlation_id: Option<CorrelationId>,
    active_timers: BTreeSet<AbisTimerKind>,
    release_ack_pending: bool,
    remove_ack_pending: bool,
}

impl TrafficReleaseProcedure {
    /// Creates a new release/remove procedure tracker for a single call reference.
    pub fn new(call_connection_reference: CallConnectionReference) -> Self {
        Self {
            call_connection_reference,
            state: TrafficReleaseState::Idle,
            expected_correlation_id: None,
            active_timers: BTreeSet::new(),
            release_ack_pending: false,
            remove_ack_pending: false,
        }
    }

    /// Returns the current release/remove state.
    pub fn state(&self) -> TrafficReleaseState {
        self.state
    }

    /// Returns the timers currently active for this release/remove exchange.
    pub fn active_timers(&self) -> Vec<AbisTimerKind> {
        self.active_timers.iter().copied().collect()
    }

    /// Starts a BSC-originated release with `BTS Release`.
    pub fn start_release(&mut self, message: &BtsReleaseMessage) -> Result<()> {
        if self.state != TrafficReleaseState::Idle
            && self.state != TrafficReleaseState::AwaitingRemove
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::BtsRelease.value(),
                reason: "release procedure already active",
            });
        }
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::BtsRelease,
        )?;
        self.expected_correlation_id = message.correlation_id;
        self.active_timers.remove(&AbisTimerKind::Trelreqb);
        self.active_timers.insert(AbisTimerKind::Tdrptgtb);
        self.release_ack_pending = true;
        self.state = TrafficReleaseState::AwaitingBtsReleaseAck;
        Ok(())
    }

    /// Completes a BSC-originated release when `BTS Release Ack` arrives.
    pub fn on_release_ack(&mut self, message: &BtsReleaseAckMessage) -> Result<()> {
        if !self.release_ack_pending {
            return Err(Error::InvalidMessage {
                message_type: MessageType::BtsReleaseAck.value(),
                reason: "unexpected BTS Release Ack for current procedure state",
            });
        }
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::BtsReleaseAck,
        )?;
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::BtsReleaseAck,
        )?;
        self.active_timers.remove(&AbisTimerKind::Tdrptgtb);
        self.release_ack_pending = false;
        self.state = if self.remove_ack_pending {
            TrafficReleaseState::AwaitingRemoveAck
        } else {
            TrafficReleaseState::Released
        };
        Ok(())
    }

    /// Processes `BTS Release Request` and marks the procedure as waiting for `Remove`.
    pub fn on_release_request(
        &mut self,
        message: &BtsReleaseRequestMessage,
    ) -> Result<BtsReleaseRequestDisposition> {
        self.ensure_state(
            TrafficReleaseState::Idle,
            MessageType::BtsReleaseRequest,
            "unexpected BTS Release Request for current procedure state",
        )?;
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::BtsReleaseRequest,
        )?;
        self.active_timers.insert(AbisTimerKind::Trelreqb);
        self.state = TrafficReleaseState::AwaitingRemove;
        Ok(BtsReleaseRequestDisposition {
            cause: message.cause,
            has_manufacturer_specific_records: message.manufacturer_specific_records.is_some(),
        })
    }

    /// Processes `Remove` from the BTS and records the expected correlation for `Remove Ack`.
    pub fn on_remove(&mut self, message: &RemoveMessage) -> Result<()> {
        if self.state != TrafficReleaseState::Idle
            && self.state != TrafficReleaseState::AwaitingRemove
            && self.state != TrafficReleaseState::AwaitingBtsReleaseAck
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::Remove.value(),
                reason: "unexpected Remove for current procedure state",
            });
        }
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::Remove,
        )?;
        self.expected_correlation_id = message.correlation_id;
        self.active_timers.insert(AbisTimerKind::Tdisconb);
        self.remove_ack_pending = true;
        self.state = TrafficReleaseState::AwaitingRemoveAck;
        Ok(())
    }

    /// Completes a BTS-originated release when the local side emits `Remove Ack`.
    pub fn on_remove_ack(&mut self, message: &RemoveAckMessage) -> Result<()> {
        self.ensure_state(
            TrafficReleaseState::AwaitingRemoveAck,
            MessageType::RemoveAck,
            "unexpected Remove Ack for current procedure state",
        )?;
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::RemoveAck,
        )?;
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::RemoveAck,
        )?;
        self.active_timers.remove(&AbisTimerKind::Tdisconb);
        self.remove_ack_pending = false;
        self.state = if self.release_ack_pending {
            TrafficReleaseState::AwaitingBtsReleaseAck
        } else {
            TrafficReleaseState::Released
        };
        Ok(())
    }

    /// Processes expiry of a release/remove timer.
    pub fn on_timer_expiry(&mut self, timer: AbisTimerKind) -> Result<TrafficReleaseTimeoutAction> {
        if !self.active_timers.contains(&timer) {
            return Err(Error::InvalidValue {
                context: "Traffic release timer",
                reason: "timer is not active",
            });
        }
        match timer {
            AbisTimerKind::Tdrptgtb => Ok(TrafficReleaseTimeoutAction::ResendBtsRelease),
            AbisTimerKind::Tdisconb => Ok(TrafficReleaseTimeoutAction::ResendRemove),
            AbisTimerKind::Trelreqb => Ok(TrafficReleaseTimeoutAction::ResendBtsReleaseRequest),
            _ => Err(Error::InvalidValue {
                context: "Traffic release timer",
                reason: "timer is not valid for release/remove procedure",
            }),
        }
    }

    fn ensure_state(
        &self,
        expected: TrafficReleaseState,
        message_type: MessageType,
        reason: &'static str,
    ) -> Result<()> {
        if self.state != expected {
            return Err(Error::InvalidMessage {
                message_type: message_type.value(),
                reason,
            });
        }
        Ok(())
    }
}

/// Parsed Abis paging request fields relevant to page/ack control behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagingRequest {
    /// Optional correlation identifier used to match `PCH Msg Transfer Ack`.
    pub correlation_id: Option<CorrelationId>,
    /// Whether the request included `Layer 2 Ack Request Results`.
    pub layer2_ack_results_requested: bool,
    /// Whether the request included `Abis Ack Notify`.
    pub abis_ack_notify: bool,
    /// Number of mobile identities carried in the request.
    pub mobile_identity_count: usize,
}

impl PagingRequest {
    /// Returns whether the request requires an explicit `PCH Msg Transfer Ack`.
    pub fn ack_expected(&self) -> bool {
        self.layer2_ack_results_requested || self.abis_ack_notify
    }
}

impl TryFrom<&AbisMessage> for PagingRequest {
    type Error = Error;

    /// Extracts paging-control fields from a decoded `PCH Msg Transfer`.
    fn try_from(message: &AbisMessage) -> Result<Self> {
        let access_transfer = AccessTransferMessage::try_from(message)?;
        if access_transfer.kind != AccessTransferKind::PagingChannel {
            return Err(Error::InvalidMessage {
                message_type: message.message_type.value(),
                reason: "expected PCH Msg Transfer",
            });
        }
        Ok(Self {
            correlation_id: access_transfer.correlation_id,
            layer2_ack_results_requested: access_transfer.layer2_ack_results_requested,
            abis_ack_notify: access_transfer.abis_ack_notify,
            mobile_identity_count: access_transfer.mobile_identity_count,
        })
    }
}

/// Result of starting a paging procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingDispatch {
    /// No explicit ack was requested for the page.
    FireAndForget,
    /// A matching `PCH Msg Transfer Ack` is expected.
    AwaitingAck,
}

/// Paging procedure state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagingState {
    /// No paging procedure is active.
    Idle,
    /// Waiting for a matching `PCH Msg Transfer Ack`.
    AwaitingAck,
    /// The current paging procedure is complete.
    Completed,
}

/// Outcome of a completed paging acknowledgement exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagingAckOutcome {
    /// Optional cause returned by the BTS.
    pub cause: Option<u8>,
    /// Optional Layer-2 termination result returned by the BTS.
    pub bts_l2_termination: Option<bool>,
}

/// Crate-local state tracker for `PCH Msg Transfer` / `PCH Msg Transfer Ack`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagingProcedure {
    state: PagingState,
    expected_correlation_id: Option<CorrelationId>,
    last_outcome: Option<PagingAckOutcome>,
}

impl PagingProcedure {
    /// Creates an empty paging procedure tracker.
    pub fn new() -> Self {
        Self {
            state: PagingState::Idle,
            expected_correlation_id: None,
            last_outcome: None,
        }
    }

    /// Returns the current paging state.
    pub fn state(&self) -> PagingState {
        self.state
    }

    /// Returns the most recent paging-ack outcome, if one exists.
    pub fn last_outcome(&self) -> Option<PagingAckOutcome> {
        self.last_outcome
    }

    /// Starts paging control from a parsed request.
    pub fn start_request(&mut self, request: &PagingRequest) -> Result<PagingDispatch> {
        if self.state == PagingState::AwaitingAck {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "paging ack already outstanding",
            });
        }

        self.last_outcome = None;
        if request.ack_expected() {
            if request.correlation_id.is_none() {
                return Err(Error::InvalidMessage {
                    message_type: MessageType::PchMessageTransfer.value(),
                    reason: "paging ack tracking requires a correlation identifier",
                });
            }
            self.expected_correlation_id = request.correlation_id;
            self.state = PagingState::AwaitingAck;
            return Ok(PagingDispatch::AwaitingAck);
        }

        self.expected_correlation_id = None;
        self.state = PagingState::Completed;
        Ok(PagingDispatch::FireAndForget)
    }

    /// Starts paging control directly from a decoded `PCH Msg Transfer`.
    pub fn start_message(&mut self, message: &AbisMessage) -> Result<PagingDispatch> {
        let request = PagingRequest::try_from(message)?;
        self.start_request(&request)
    }

    /// Completes the paging procedure with a matching `PCH Msg Transfer Ack`.
    pub fn on_ack(&mut self, message: &PchMessageTransferAckMessage) -> Result<PagingAckOutcome> {
        if self.state != PagingState::AwaitingAck {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransferAck.value(),
                reason: "unexpected PCH Msg Transfer Ack for current procedure state",
            });
        }
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::PchMessageTransferAck,
        )?;
        let outcome = PagingAckOutcome {
            cause: message.cause,
            bts_l2_termination: message.bts_l2_termination,
        };
        self.last_outcome = Some(outcome);
        self.state = PagingState::Completed;
        Ok(outcome)
    }
}

impl Default for PagingProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// Burst reservation/allocation procedure state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstAllocationState {
    /// No burst-reservation exchange is active.
    Idle,
    /// A `Burst Request` was issued and a matching `Burst Response` is pending.
    AwaitingResponse,
    /// A `Burst Response` was accepted and the procedure is waiting for `Burst Commit`.
    AwaitingCommit,
    /// The reservation/allocation flow completed successfully.
    Committed,
}

/// Summary of a received `Burst Response`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BurstResponseDisposition {
    /// Number of committed cells carried by the response.
    pub committed_cells: usize,
    /// Number of uncommitted cells carried by the response.
    pub uncommitted_cells: usize,
    /// Whether more `Burst Response` messages are still required to account for all cells.
    pub awaiting_more_cells: bool,
}

/// Action implied by expiry of a burst-allocation timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstAllocationTimeoutAction {
    /// The BSC may resend `Burst Request`.
    ResendBurstRequest,
    /// The BTS may decommit reserved resources and wait for a new reservation cycle.
    DecommitReservedResources,
}

/// Crate-local state tracker for `Burst Request` / `Burst Response` / `Burst Commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstAllocationProcedure {
    state: BurstAllocationState,
    expected_call_connection_reference: Option<CallConnectionReference>,
    expected_correlation_id: Option<CorrelationId>,
    expected_abis_destination_id: Option<AbisDestinationId>,
    requested_cells: Vec<CellId>,
    response_cells: Vec<CellId>,
    last_forward_burst_radio_info: Option<ForwardBurstRadioInfo>,
    last_reverse_burst_radio_info: Option<ReverseBurstRadioInfo>,
    active_timers: BTreeSet<AbisTimerKind>,
}

impl BurstAllocationProcedure {
    /// Creates an empty burst-allocation procedure tracker.
    pub fn new() -> Self {
        Self {
            state: BurstAllocationState::Idle,
            expected_call_connection_reference: None,
            expected_correlation_id: None,
            expected_abis_destination_id: None,
            requested_cells: Vec::new(),
            response_cells: Vec::new(),
            last_forward_burst_radio_info: None,
            last_reverse_burst_radio_info: None,
            active_timers: BTreeSet::new(),
        }
    }

    /// Returns the current burst-allocation state.
    pub fn state(&self) -> BurstAllocationState {
        self.state
    }

    /// Returns the timers currently active for this burst-allocation exchange.
    pub fn active_timers(&self) -> Vec<AbisTimerKind> {
        self.active_timers.iter().copied().collect()
    }

    /// Starts burst reservation from a `Burst Request`.
    pub fn start_request(&mut self, message: &BurstRequestMessage) -> Result<()> {
        self.ensure_state(
            BurstAllocationState::Idle,
            MessageType::BurstRequest,
            "burst allocation procedure already active",
        )?;
        if message.call_connection_reference.is_none() && message.correlation_id.is_none() {
            return Err(Error::InvalidMessage {
                message_type: MessageType::BurstRequest.value(),
                reason: "burst reservation tracking requires call reference or correlation identifier",
            });
        }
        self.expected_call_connection_reference = message.call_connection_reference;
        self.expected_correlation_id = message.correlation_id;
        self.expected_abis_destination_id = message.abis_destination_id.clone();
        self.requested_cells = message.cell_identifier_list.clone().unwrap_or_default();
        self.response_cells.clear();
        self.last_forward_burst_radio_info = None;
        self.last_reverse_burst_radio_info = None;
        self.active_timers.clear();
        self.active_timers.insert(AbisTimerKind::Tbstreqb);
        self.state = BurstAllocationState::AwaitingResponse;
        Ok(())
    }

    /// Accepts a matching `Burst Response` and records the cells eligible for commit.
    pub fn on_response(
        &mut self,
        message: &BurstResponseMessage,
    ) -> Result<BurstResponseDisposition> {
        if self.state != BurstAllocationState::AwaitingResponse
            && self.state != BurstAllocationState::AwaitingCommit
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::BurstResponse.value(),
                reason: "unexpected Burst Response for current procedure state",
            });
        }
        ensure_optional_call_reference(
            self.expected_call_connection_reference,
            message.call_connection_reference,
            MessageType::BurstResponse,
        )?;
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::BurstResponse,
        )?;
        ensure_optional_destination_id(
            self.expected_abis_destination_id.clone(),
            message.abis_destination_id.clone(),
            MessageType::BurstResponse,
        )?;

        let committed_cells = message
            .committed_cell_identifier_list
            .as_ref()
            .map_or(0, Vec::len);
        let uncommitted_cells = message
            .uncommitted_cell_identifier_list
            .as_ref()
            .map_or(0, Vec::len);
        if committed_cells == 0 && uncommitted_cells == 0 {
            return Err(Error::InvalidMessage {
                message_type: MessageType::BurstResponse.value(),
                reason: "Burst Response must include at least one committed or uncommitted cell",
            });
        }

        if let Some(cells) = &message.committed_cell_identifier_list {
            extend_unique_cells(&mut self.response_cells, cells);
        }
        if let Some(cells) = &message.uncommitted_cell_identifier_list {
            extend_unique_cells(&mut self.response_cells, cells);
        }
        if message.forward_burst_radio_info.is_some() {
            self.last_forward_burst_radio_info = message.forward_burst_radio_info;
        }
        if message.reverse_burst_radio_info.is_some() {
            self.last_reverse_burst_radio_info = message.reverse_burst_radio_info;
        }
        let awaiting_more_cells = !self.requested_cells.is_empty()
            && !self
                .requested_cells
                .iter()
                .all(|cell| self.response_cells.contains(cell));
        if awaiting_more_cells {
            self.state = BurstAllocationState::AwaitingResponse;
        } else {
            self.active_timers.remove(&AbisTimerKind::Tbstreqb);
            self.active_timers.insert(AbisTimerKind::Tbstcomb);
            self.state = BurstAllocationState::AwaitingCommit;
        }
        Ok(BurstResponseDisposition {
            committed_cells,
            uncommitted_cells,
            awaiting_more_cells,
        })
    }

    /// Completes burst allocation with a matching `Burst Commit`.
    pub fn on_commit(&mut self, message: &BurstCommitMessage) -> Result<()> {
        self.ensure_state(
            BurstAllocationState::AwaitingCommit,
            MessageType::BurstCommit,
            "unexpected Burst Commit for current procedure state",
        )?;
        ensure_optional_call_reference(
            self.expected_call_connection_reference,
            message.call_connection_reference,
            MessageType::BurstCommit,
        )?;
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::BurstCommit,
        )?;
        ensure_optional_destination_id(
            self.expected_abis_destination_id.clone(),
            message.abis_destination_id.clone(),
            MessageType::BurstCommit,
        )?;

        ensure_burst_commit_cells(
            &self.response_cells,
            message.forward_cell_identifier_list.as_deref(),
            MessageType::BurstCommit,
            "forward",
        )?;
        ensure_burst_commit_cells(
            &self.response_cells,
            message.reverse_cell_identifier_list.as_deref(),
            MessageType::BurstCommit,
            "reverse",
        )?;
        ensure_forward_burst_rate(
            self.last_forward_burst_radio_info,
            message.forward_burst_radio_info,
            MessageType::BurstCommit,
        )?;
        ensure_reverse_burst_rate(
            self.last_reverse_burst_radio_info,
            message.reverse_burst_radio_info,
            MessageType::BurstCommit,
        )?;

        self.state = BurstAllocationState::Committed;
        self.active_timers.remove(&AbisTimerKind::Tbstcomb);
        Ok(())
    }

    /// Processes expiry of a burst-allocation timer.
    pub fn on_timer_expiry(
        &mut self,
        timer: AbisTimerKind,
    ) -> Result<BurstAllocationTimeoutAction> {
        if !self.active_timers.contains(&timer) {
            return Err(Error::InvalidValue {
                context: "Burst allocation timer",
                reason: "timer is not active",
            });
        }
        match timer {
            AbisTimerKind::Tbstreqb => Ok(BurstAllocationTimeoutAction::ResendBurstRequest),
            AbisTimerKind::Tbstcomb => {
                self.active_timers.remove(&AbisTimerKind::Tbstcomb);
                self.state = BurstAllocationState::Idle;
                Ok(BurstAllocationTimeoutAction::DecommitReservedResources)
            }
            _ => Err(Error::InvalidValue {
                context: "Burst allocation timer",
                reason: "timer is not valid for burst-allocation procedure",
            }),
        }
    }

    fn ensure_state(
        &self,
        expected: BurstAllocationState,
        message_type: MessageType,
        reason: &'static str,
    ) -> Result<()> {
        if self.state != expected {
            return Err(Error::InvalidMessage {
                message_type: message_type.value(),
                reason,
            });
        }
        Ok(())
    }
}

impl Default for BurstAllocationProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// Access-transfer family discriminant for raw `ACH` / `PCH` control messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTransferKind {
    /// `ACH Msg Transfer`.
    AccessChannel,
    /// `PCH Msg Transfer`.
    PagingChannel,
}

/// Parsed Abis access-transfer fields that can be validated locally in this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTransferMessage {
    /// Whether the message originated on `ACH` or `PCH`.
    pub kind: AccessTransferKind,
    /// Optional correlation identifier carried by the message.
    pub correlation_id: Option<CorrelationId>,
    /// Number of mobile identities carried by the message.
    pub mobile_identity_count: usize,
    /// Whether a single-cell identifier is present.
    pub has_cell_identifier: bool,
    /// Whether a cell-identifier list is present.
    pub has_cell_identifier_list: bool,
    /// Whether the message carries an air-interface payload.
    pub has_air_interface_message: bool,
    /// Whether an authentication challenge parameter is present.
    pub has_authentication_challenge: bool,
    /// Whether `Layer 2 Ack Request Results` is present.
    pub layer2_ack_results_requested: bool,
    /// Whether `Abis Ack Notify` is present.
    pub abis_ack_notify: bool,
    /// Optional `BTS L2 Termination` value.
    pub bts_l2_termination: Option<bool>,
}

impl AccessTransferMessage {
    /// Returns whether the message requests an explicit Abis paging acknowledgement.
    pub fn ack_expected(&self) -> bool {
        self.layer2_ack_results_requested || self.abis_ack_notify
    }
}

impl TryFrom<&AbisMessage> for AccessTransferMessage {
    type Error = Error;

    /// Extracts local access-transfer semantics from decoded `ACH Msg Transfer` and
    /// `PCH Msg Transfer` messages.
    fn try_from(message: &AbisMessage) -> Result<Self> {
        let kind = match message.message_type {
            MessageType::AchMessageTransfer => AccessTransferKind::AccessChannel,
            MessageType::PchMessageTransfer => AccessTransferKind::PagingChannel,
            other => {
                return Err(Error::InvalidMessage {
                    message_type: other.value(),
                    reason: "expected ACH Msg Transfer or PCH Msg Transfer",
                });
            }
        };

        let mut correlation_id = None;
        let mut mobile_identity_count = 0usize;
        let mut has_cell_identifier = false;
        let mut has_cell_identifier_list = false;
        let mut has_air_interface_message = false;
        let mut has_authentication_challenge = false;
        let mut saw_layer2_ack_results = false;
        let mut layer2_ack_results_requested = false;
        let mut abis_ack_notify = false;
        let mut bts_l2_termination = None;

        for element in &message.elements {
            match element.id {
                ElementId::CorrelationId => {
                    correlation_id = Some(CorrelationId::decode(&element.value)?);
                }
                ElementId::MobileIdentity => {
                    let _ = MobileIdentity::decode(&element.value)?;
                    mobile_identity_count += 1;
                }
                ElementId::CellIdentifier => has_cell_identifier = true,
                ElementId::CellIdentifierList => has_cell_identifier_list = true,
                ElementId::AirInterfaceMessage => {
                    let _ = AirInterfaceMessagePayload::decode(&element.value)?;
                    has_air_interface_message = true;
                }
                ElementId::AuthenticationChallengeParameter => {
                    let _ = AuthenticationChallengeParameter::decode(&element.value)?;
                    has_authentication_challenge = true;
                }
                ElementId::Layer2AckRequestResults => {
                    saw_layer2_ack_results = true;
                    layer2_ack_results_requested =
                        Layer2AckRequestResults::decode(&element.value)?.layer2_ack;
                }
                ElementId::AbisAckNotify => {
                    let _ = AbisAckNotify::decode(&element.value)?;
                    abis_ack_notify = true;
                }
                ElementId::BtsL2Termination => {
                    if element.value.len() != 1 {
                        return Err(Error::InvalidLength {
                            context: "BTS L2 Termination",
                            expected: 1,
                            actual: element.value.len(),
                        });
                    }
                    if element.value[0] != 0x01 {
                        return Err(Error::InvalidValue {
                            context: "BTS L2 Termination",
                            reason: "shall be set to one in this release",
                        });
                    }
                    bts_l2_termination = Some(true);
                }
                _ => {}
            }
        }

        if kind == AccessTransferKind::AccessChannel
            && has_authentication_challenge
            && mobile_identity_count == 0
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::AchMessageTransfer.value(),
                reason: "ACH Msg Transfer with authentication challenge requires a mobile identity",
            });
        }
        if kind == AccessTransferKind::PagingChannel && mobile_identity_count > 1 {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "PCH Msg Transfer may carry at most one mobile identity",
            });
        }
        if kind == AccessTransferKind::PagingChannel
            && saw_layer2_ack_results
            && !layer2_ack_results_requested
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "Layer 2 Ack Request/Results must request acknowledgement when present",
            });
        }
        if kind == AccessTransferKind::PagingChannel && abis_ack_notify && !saw_layer2_ack_results {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "Abis Ack Notify requires Layer 2 Ack Request/Results",
            });
        }
        if kind == AccessTransferKind::PagingChannel
            && (layer2_ack_results_requested || abis_ack_notify)
            && correlation_id.is_none()
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "paging ack tracking requires a correlation identifier",
            });
        }

        Ok(Self {
            kind,
            correlation_id,
            mobile_identity_count,
            has_cell_identifier,
            has_cell_identifier_list,
            has_air_interface_message,
            has_authentication_challenge,
            layer2_ack_results_requested,
            abis_ack_notify,
            bts_l2_termination,
        })
    }
}

/// Local ordering state for paging-to-access transfer handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTransferState {
    /// No access-transfer exchange is active.
    Idle,
    /// A paging message was sent and an optional paging ack is pending.
    AwaitingPagingAck,
    /// The BTS/BSC path is waiting for an access-channel uplink indication.
    AwaitingAccessChannel,
    /// An access-channel message was received for the current sequence.
    AccessReceived,
}

/// Result of receiving a paging transfer with access-transfer expectations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTransferDispatch {
    /// No explicit paging ack is expected; the next step is access-channel reception.
    AwaitingAccessChannel,
    /// A paging ack is expected before access-channel reception.
    AwaitingPagingAck,
}

/// Crate-local ordering helper for `PCH Msg Transfer`, `PCH Msg Transfer Ack`, and
/// `ACH Msg Transfer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTransferProcedure {
    state: AccessTransferState,
    expected_correlation_id: Option<CorrelationId>,
}

impl AccessTransferProcedure {
    /// Creates an empty access-transfer ordering tracker.
    pub fn new() -> Self {
        Self {
            state: AccessTransferState::Idle,
            expected_correlation_id: None,
        }
    }

    /// Returns the current access-transfer state.
    pub fn state(&self) -> AccessTransferState {
        self.state
    }

    /// Starts an access-transfer sequence from a paging transfer.
    pub fn on_paging_transfer(
        &mut self,
        message: &PchMessageTransferMessage,
    ) -> Result<AccessTransferDispatch> {
        if self.state != AccessTransferState::Idle
            && self.state != AccessTransferState::AccessReceived
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "paging transfer is out of order for current access-transfer state",
            });
        }
        if (message.layer2_ack_request_results.is_some() || message.abis_ack_notify.is_some())
            && message.correlation_id.is_none()
        {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransfer.value(),
                reason: "paging ack tracking requires a correlation identifier",
            });
        }
        self.expected_correlation_id = message.correlation_id;
        if message.layer2_ack_request_results.is_some() || message.abis_ack_notify.is_some() {
            self.state = AccessTransferState::AwaitingPagingAck;
            Ok(AccessTransferDispatch::AwaitingPagingAck)
        } else {
            self.state = AccessTransferState::AwaitingAccessChannel;
            Ok(AccessTransferDispatch::AwaitingAccessChannel)
        }
    }

    /// Records a paging acknowledgement before access transfer proceeds.
    pub fn on_paging_ack(&mut self, message: &PchMessageTransferAckMessage) -> Result<()> {
        if self.state != AccessTransferState::AwaitingPagingAck {
            return Err(Error::InvalidMessage {
                message_type: MessageType::PchMessageTransferAck.value(),
                reason: "paging ack is out of order for current access-transfer state",
            });
        }
        if matches!(message.bts_l2_termination, Some(false)) {
            return Err(Error::InvalidValue {
                context: "BTS L2 Termination",
                reason: "shall be set to one in this release",
            });
        }
        ensure_correlation(
            self.expected_correlation_id,
            message.correlation_id,
            MessageType::PchMessageTransferAck,
        )?;
        self.state = AccessTransferState::AwaitingAccessChannel;
        Ok(())
    }

    /// Records an access-channel transfer after the expected paging stage.
    pub fn on_access_transfer(&mut self, message: &AchMessageTransferMessage) -> Result<()> {
        if self.state != AccessTransferState::AwaitingAccessChannel {
            return Err(Error::InvalidMessage {
                message_type: MessageType::AchMessageTransfer.value(),
                reason: "ACH transfer is out of order for current access-transfer state",
            });
        }
        if matches!(message.bts_l2_termination, Some(false)) {
            return Err(Error::InvalidValue {
                context: "BTS L2 Termination",
                reason: "shall be set to one in this release",
            });
        }
        if let Some(expected_correlation_id) = self.expected_correlation_id
            && let Some(actual_correlation_id) = message.correlation_id
        {
            ensure_correlation(
                Some(expected_correlation_id),
                Some(actual_correlation_id),
                MessageType::AchMessageTransfer,
            )?;
        }
        self.state = AccessTransferState::AccessReceived;
        Ok(())
    }
}

impl Default for AccessTransferProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// PACA queue-tracking state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacaState {
    /// No PACA action has been applied yet.
    Idle,
    /// The mobile remains on the queue and its position was updated.
    QueuePositionUpdated,
    /// The mobile was removed from the queue.
    Removed,
}

/// Result of applying a PACA update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacaDisposition {
    /// The update carried no action and only refreshed local identity binding.
    NoAction,
    /// The mobile queue position must be updated.
    UpdateQueuePosition,
    /// The mobile must be removed from the queue.
    RemoveMsFromQueue,
}

/// Crate-local state tracker for `PACA Update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacaProcedure {
    call_connection_reference: CallConnectionReference,
    state: PacaState,
    mobile_identity_imsi: Option<String>,
}

impl PacaProcedure {
    /// Creates a new PACA procedure tracker for a single call reference.
    pub fn new(call_connection_reference: CallConnectionReference) -> Self {
        Self {
            call_connection_reference,
            state: PacaState::Idle,
            mobile_identity_imsi: None,
        }
    }

    /// Returns the current PACA state.
    pub fn state(&self) -> PacaState {
        self.state
    }

    /// Returns the bound IMSI, if PACA updates have identified a mobile.
    pub fn mobile_identity_imsi(&self) -> Option<&str> {
        self.mobile_identity_imsi.as_deref()
    }

    /// Applies a `PACA Update` and returns the local queue action it implies.
    pub fn apply_update(&mut self, message: &PacaUpdateMessage) -> Result<PacaDisposition> {
        ensure_call_reference(
            self.call_connection_reference,
            message.call_connection_reference,
            MessageType::PacaUpdate,
        )?;

        if let Some(identity) = &message.mobile_identity_imsi {
            match identity {
                MobileIdentity::Imsi(imsi) => {
                    if let Some(existing) = &self.mobile_identity_imsi {
                        if existing != imsi {
                            return Err(Error::InvalidMessage {
                                message_type: MessageType::PacaUpdate.value(),
                                reason: "PACA Update IMSI does not match existing queue binding",
                            });
                        }
                    } else {
                        self.mobile_identity_imsi = Some(imsi.clone());
                    }
                }
                MobileIdentity::Esn(_) => {
                    return Err(Error::InvalidMessage {
                        message_type: MessageType::PacaUpdate.value(),
                        reason: "PACA Update mobile identity must be IMSI when present",
                    });
                }
            }
        }

        match message.action_required {
            None => Ok(PacaDisposition::NoAction),
            Some(PacaActionRequired::UpdateQueuePosition) => {
                if self.state == PacaState::Removed {
                    return Err(Error::InvalidMessage {
                        message_type: MessageType::PacaUpdate.value(),
                        reason: "queue-position update is invalid after PACA removal",
                    });
                }
                self.state = PacaState::QueuePositionUpdated;
                Ok(PacaDisposition::UpdateQueuePosition)
            }
            Some(PacaActionRequired::RemoveMsFromQueue) => {
                self.state = PacaState::Removed;
                Ok(PacaDisposition::RemoveMsFromQueue)
            }
        }
    }
}

fn ensure_call_reference(
    expected: CallConnectionReference,
    actual: CallConnectionReference,
    message_type: MessageType,
) -> Result<()> {
    if expected != actual {
        return Err(Error::InvalidMessage {
            message_type: message_type.value(),
            reason: "call connection reference mismatch",
        });
    }
    Ok(())
}

fn ensure_correlation(
    expected: Option<CorrelationId>,
    actual: Option<CorrelationId>,
    message_type: MessageType,
) -> Result<()> {
    if expected != actual {
        return Err(Error::InvalidMessage {
            message_type: message_type.value(),
            reason: "correlation identifier mismatch",
        });
    }
    Ok(())
}

fn ensure_optional_call_reference(
    expected: Option<CallConnectionReference>,
    actual: Option<CallConnectionReference>,
    message_type: MessageType,
) -> Result<()> {
    if expected != actual {
        return Err(Error::InvalidMessage {
            message_type: message_type.value(),
            reason: "call connection reference mismatch",
        });
    }
    Ok(())
}

fn ensure_optional_destination_id(
    expected: Option<AbisDestinationId>,
    actual: Option<AbisDestinationId>,
    message_type: MessageType,
) -> Result<()> {
    if expected != actual {
        return Err(Error::InvalidMessage {
            message_type: message_type.value(),
            reason: "Abis destination identifier mismatch",
        });
    }
    Ok(())
}

fn ensure_burst_commit_cells(
    response_cells: &[CellId],
    commit_cells: Option<&[CellId]>,
    message_type: MessageType,
    direction: &'static str,
) -> Result<()> {
    if let Some(commit_cells) = commit_cells {
        for cell in commit_cells {
            if !response_cells.contains(cell) {
                return Err(Error::InvalidMessage {
                    message_type: message_type.value(),
                    reason: match direction {
                        "forward" => "forward burst commit cell was not offered in Burst Response",
                        "reverse" => "reverse burst commit cell was not offered in Burst Response",
                        _ => "burst commit cell was not offered in Burst Response",
                    },
                });
            }
        }
    }
    Ok(())
}

fn extend_unique_cells(target: &mut Vec<CellId>, incoming: &[CellId]) {
    for cell in incoming {
        if !target.contains(cell) {
            target.push(*cell);
        }
    }
}

fn ensure_forward_burst_rate(
    response: Option<ForwardBurstRadioInfo>,
    commit: Option<ForwardBurstRadioInfo>,
    message_type: MessageType,
) -> Result<()> {
    if let (Some(response), Some(commit)) = (response, commit)
        && commit.forward_supplemental_channel_rate > response.forward_supplemental_channel_rate
    {
        return Err(Error::InvalidMessage {
            message_type: message_type.value(),
            reason: "forward burst commit rate exceeds Burst Response",
        });
    }
    Ok(())
}

fn ensure_reverse_burst_rate(
    response: Option<ReverseBurstRadioInfo>,
    commit: Option<ReverseBurstRadioInfo>,
    message_type: MessageType,
) -> Result<()> {
    if let (Some(response), Some(commit)) = (response, commit)
        && commit.reverse_supplemental_channel_rate > response.reverse_supplemental_channel_rate
    {
        return Err(Error::InvalidMessage {
            message_type: message_type.value(),
            reason: "reverse burst commit rate exceeds Burst Response",
        });
    }
    Ok(())
}
