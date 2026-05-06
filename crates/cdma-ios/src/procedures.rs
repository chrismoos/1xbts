//! A1/IOS procedure state machines layered on top of the typed messages.

use crate::{
    AlertWithInformationMessage, AssignmentCompleteMessage, AssignmentFailureMessage,
    AssignmentRequestMessage, AuthenticationRequestMessage, AuthenticationResponseMessage,
    BaseStationChallengeMessage, BaseStationChallengeResponseMessage, BsServiceRequestMessage,
    BsServiceResponseMessage, ClearCommandMessage, ClearCompleteMessage, ClearRequestMessage,
    CmServiceRequestMessage, CompleteLayer3InformationMessage, ConnectMessage,
    HandoffCommandMessage, HandoffCommencedMessage, HandoffCompleteMessage, HandoffFailureMessage,
    HandoffPerformedMessage, HandoffRequestAcknowledgeMessage, HandoffRequestMessage,
    HandoffRequiredMessage, HandoffRequiredRejectMessage, LocationUpdatingAcceptMessage,
    LocationUpdatingRejectMessage, LocationUpdatingRequestMessage, PagingRequestMessage,
    PagingResponseMessage, ParameterUpdateConfirmMessage, ParameterUpdateRequestMessage,
    PrivacyModeCommandMessage, PrivacyModeCompleteMessage, ProgressMessage,
    SsdUpdateRequestMessage, SsdUpdateResponseMessage, UserZoneUpdateMessage,
};

/// Errors returned by the A1 procedure state machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcedureError {
    /// The requested transition is not valid from the current procedure state.
    InvalidTransition {
        /// Procedure name for diagnostics.
        procedure: &'static str,
        /// Current state when the invalid transition was attempted.
        state: &'static str,
        /// Human-readable reason for the rejection.
        reason: &'static str,
    },
    /// The message direction does not match the A1 interface contract.
    InvalidDirection {
        /// Procedure-engine context for diagnostics.
        procedure: &'static str,
        /// The message name that was rejected.
        message: &'static str,
        /// The expected direction for the message.
        expected: ProcedureDirection,
        /// The applied direction that was rejected.
        actual: ProcedureDirection,
    },
}

/// Result type used by the procedure state machines.
pub type Result<T> = std::result::Result<T, ProcedureError>;

/// Timer action emitted by a procedure transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerAction<T> {
    /// Start or refresh the named timer.
    Arm(T),
    /// Cancel the named timer.
    Cancel(T),
}

/// State transition emitted by a procedure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition<S, T> {
    /// State before the transition.
    pub previous_state: S,
    /// State after the transition.
    pub new_state: S,
    /// Timer side effects emitted by the transition in order.
    pub timer_actions: Vec<TimerAction<T>>,
}

/// Normal A1 direction for a typed procedure message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcedureDirection {
    /// Message flows from the BSC to the MSC.
    BscToMsc,
    /// Message flows from the MSC to the BSC.
    MscToBsc,
}

fn invalid_transition(
    procedure: &'static str,
    state: &'static str,
    reason: &'static str,
) -> ProcedureError {
    ProcedureError::InvalidTransition {
        procedure,
        state,
        reason,
    }
}

fn transition<S: Copy, T>(
    previous_state: S,
    new_state: S,
    timer_action: Option<TimerAction<T>>,
) -> Transition<S, T> {
    Transition {
        previous_state,
        new_state,
        timer_actions: timer_action.into_iter().collect(),
    }
}

fn transition_many<S: Copy, T>(
    previous_state: S,
    new_state: S,
    timer_actions: Vec<TimerAction<T>>,
) -> Transition<S, T> {
    Transition {
        previous_state,
        new_state,
        timer_actions,
    }
}

/// High-level A1 call-control state for access, assignment, alerting, connect, and clear flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallControlState {
    Idle,
    Paging,
    AccessPending,
    AssignmentPending,
    Assigned,
    Alerting,
    Connected,
    Clearing,
    Released,
    AssignmentFailed,
    TimedOut,
}

impl CallControlState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Paging => "Paging",
            Self::AccessPending => "AccessPending",
            Self::AssignmentPending => "AssignmentPending",
            Self::Assigned => "Assigned",
            Self::Alerting => "Alerting",
            Self::Connected => "Connected",
            Self::Clearing => "Clearing",
            Self::Released => "Released",
            Self::AssignmentFailed => "AssignmentFailed",
            Self::TimedOut => "TimedOut",
        }
    }
}

/// Timers driven by the call-control state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallControlTimer {
    Assignment,
    Clear,
}

/// Tracks the high-level A1 call-control sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallControlProcedure {
    state: CallControlState,
}

impl CallControlProcedure {
    /// Creates a new call-control procedure in the `Idle` state.
    pub fn new() -> Self {
        Self {
            state: CallControlState::Idle,
        }
    }

    /// Returns the current call-control state.
    pub fn state(&self) -> CallControlState {
        self.state
    }

    /// Applies `Complete Layer 3 Information` for a mobile-originated access.
    pub fn on_complete_layer3_information(
        &mut self,
        _message: &CompleteLayer3InformationMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        self.enter_access_pending("Complete Layer 3 Information")
    }

    /// Applies `CM Service Request` for a mobile-originated access.
    pub fn on_cm_service_request(
        &mut self,
        _message: &CmServiceRequestMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        self.enter_access_pending("CM Service Request")
    }

    fn enter_access_pending(
        &mut self,
        source: &'static str,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Idle => {
                let previous = self.state;
                self.state = CallControlState::AccessPending;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                match source {
                    "CM Service Request" => "CM Service Request is only valid from Idle",
                    _ => "Complete Layer 3 Information is only valid from Idle",
                },
            )),
        }
    }

    /// Applies `Paging Request` for a mobile-terminated access.
    pub fn on_paging_request(
        &mut self,
        _message: &PagingRequestMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Idle => {
                let previous = self.state;
                self.state = CallControlState::Paging;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Paging Request is only valid from Idle",
            )),
        }
    }

    /// Applies `Paging Response` after an outstanding page.
    pub fn on_paging_response(
        &mut self,
        _message: &PagingResponseMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Paging => {
                let previous = self.state;
                self.state = CallControlState::AccessPending;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Paging Response is only valid from Paging",
            )),
        }
    }

    /// Applies `Assignment Request` and arms the assignment timer.
    pub fn on_assignment_request(
        &mut self,
        _message: &AssignmentRequestMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::AccessPending => {
                let previous = self.state;
                self.state = CallControlState::AssignmentPending;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(CallControlTimer::Assignment)),
                ))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Assignment Request is only valid from AccessPending",
            )),
        }
    }

    /// Applies `Assignment Complete` and cancels the assignment timer.
    pub fn on_assignment_complete(
        &mut self,
        _message: &AssignmentCompleteMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::AssignmentPending => {
                let previous = self.state;
                self.state = CallControlState::Assigned;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(CallControlTimer::Assignment)),
                ))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Assignment Complete is only valid from AssignmentPending",
            )),
        }
    }

    /// Applies `Assignment Failure` and cancels the assignment timer.
    pub fn on_assignment_failure(
        &mut self,
        _message: &AssignmentFailureMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::AssignmentPending => {
                let previous = self.state;
                self.state = CallControlState::AssignmentFailed;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(CallControlTimer::Assignment)),
                ))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Assignment Failure is only valid from AssignmentPending",
            )),
        }
    }

    /// Applies `Progress`, moving the call into `Alerting`.
    pub fn on_progress(
        &mut self,
        _message: &ProgressMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Assigned | CallControlState::Alerting => {
                let previous = self.state;
                self.state = CallControlState::Alerting;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Progress is only valid from Assigned or Alerting",
            )),
        }
    }

    /// Applies `Alert With Information`, moving the call into `Alerting`.
    pub fn on_alert_with_information(
        &mut self,
        _message: &AlertWithInformationMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Assigned | CallControlState::Alerting => {
                let previous = self.state;
                self.state = CallControlState::Alerting;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Alert With Information is only valid from Assigned or Alerting",
            )),
        }
    }

    /// Applies `Connect`, moving the call into `Connected`.
    pub fn on_connect(
        &mut self,
        _message: &ConnectMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Assigned | CallControlState::Alerting => {
                let previous = self.state;
                self.state = CallControlState::Connected;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Connect is only valid from Assigned or Alerting",
            )),
        }
    }

    /// Applies `Clear Request` and arms the clear timer.
    pub fn on_clear_request(
        &mut self,
        _message: &ClearRequestMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        self.enter_clearing("Clear Request")
    }

    /// Applies `Clear Command` and arms the clear timer.
    pub fn on_clear_command(
        &mut self,
        _message: &ClearCommandMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        self.enter_clearing("Clear Command")
    }

    /// Applies `Clear Complete` and cancels the clear timer.
    pub fn on_clear_complete(
        &mut self,
        _message: &ClearCompleteMessage,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::Clearing => {
                let previous = self.state;
                self.state = CallControlState::Released;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(CallControlTimer::Clear)),
                ))
            }
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Clear Complete is only valid from Clearing",
            )),
        }
    }

    /// Applies timer expiry for the current call-control state.
    pub fn on_timer_expired(
        &mut self,
        timer: CallControlTimer,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match (self.state, timer) {
            (CallControlState::AssignmentPending, CallControlTimer::Assignment) => {
                let previous = self.state;
                self.state = CallControlState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (CallControlState::Clearing, CallControlTimer::Clear) => {
                let previous = self.state;
                self.state = CallControlState::Released;
                Ok(transition(previous, self.state, None))
            }
            (state, CallControlTimer::Assignment) => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Assignment timer is only valid from AssignmentPending",
            )),
            (state, CallControlTimer::Clear) => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                "Clear timer is only valid from Clearing",
            )),
        }
    }

    fn enter_clearing(
        &mut self,
        source: &'static str,
    ) -> Result<Transition<CallControlState, CallControlTimer>> {
        match self.state {
            CallControlState::AccessPending
            | CallControlState::AssignmentPending
            | CallControlState::Assigned
            | CallControlState::Alerting
            | CallControlState::Connected
            | CallControlState::AssignmentFailed => {
                let previous = self.state;
                self.state = CallControlState::Clearing;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(CallControlTimer::Clear)),
                ))
            }
            CallControlState::Clearing => Ok(transition(self.state, self.state, None)),
            state => Err(invalid_transition(
                "CallControl",
                state.as_str(),
                match source {
                    "Clear Request" => {
                        "Clear Request is only valid from an active call or access state"
                    }
                    _ => "Clear Command is only valid from an active call or access state",
                },
            )),
        }
    }
}

impl Default for CallControlProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// High-level A1 BS service state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsServiceState {
    Idle,
    Requested,
    Responded,
    TimedOut,
}

impl BsServiceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Requested => "Requested",
            Self::Responded => "Responded",
            Self::TimedOut => "TimedOut",
        }
    }
}

/// Timers driven by the BS service procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BsServiceTimer {
    Response,
}

/// Tracks the BS service request/response exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BsServiceProcedure {
    state: BsServiceState,
}

impl BsServiceProcedure {
    /// Creates a new BS service procedure in the `Idle` state.
    pub fn new() -> Self {
        Self {
            state: BsServiceState::Idle,
        }
    }

    /// Returns the current BS service state.
    pub fn state(&self) -> BsServiceState {
        self.state
    }

    /// Applies `BS Service Request` and arms the response timer.
    pub fn on_request(
        &mut self,
        _message: &BsServiceRequestMessage,
    ) -> Result<Transition<BsServiceState, BsServiceTimer>> {
        match self.state {
            BsServiceState::Idle => {
                let previous = self.state;
                self.state = BsServiceState::Requested;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(BsServiceTimer::Response)),
                ))
            }
            state => Err(invalid_transition(
                "BsService",
                state.as_str(),
                "BS Service Request is only valid from Idle",
            )),
        }
    }

    /// Applies `BS Service Response` and cancels the response timer.
    pub fn on_response(
        &mut self,
        _message: &BsServiceResponseMessage,
    ) -> Result<Transition<BsServiceState, BsServiceTimer>> {
        match self.state {
            BsServiceState::Requested => {
                let previous = self.state;
                self.state = BsServiceState::Responded;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(BsServiceTimer::Response)),
                ))
            }
            state => Err(invalid_transition(
                "BsService",
                state.as_str(),
                "BS Service Response is only valid from Requested",
            )),
        }
    }

    /// Applies BS-service timer expiry.
    pub fn on_timer_expired(
        &mut self,
        timer: BsServiceTimer,
    ) -> Result<Transition<BsServiceState, BsServiceTimer>> {
        match (self.state, timer) {
            (BsServiceState::Requested, BsServiceTimer::Response) => {
                let previous = self.state;
                self.state = BsServiceState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (state, BsServiceTimer::Response) => Err(invalid_transition(
                "BsService",
                state.as_str(),
                "BS Service response timer is only valid from Requested",
            )),
        }
    }
}

impl Default for BsServiceProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// High-level A1 mobility-management state for authentication, SSD update,
/// registration, parameter update, privacy mode, and user-zone update flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobilityManagementState {
    Idle,
    AwaitingAuthenticationResponse,
    AwaitingBaseStationChallenge,
    AwaitingSsdUpdateResponse,
    AwaitingLocationUpdatingResult,
    AwaitingParameterUpdateConfirm,
    AwaitingPrivacyModeComplete,
    Updated,
    Rejected,
    TimedOut,
}

impl MobilityManagementState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::AwaitingAuthenticationResponse => "AwaitingAuthenticationResponse",
            Self::AwaitingBaseStationChallenge => "AwaitingBaseStationChallenge",
            Self::AwaitingSsdUpdateResponse => "AwaitingSsdUpdateResponse",
            Self::AwaitingLocationUpdatingResult => "AwaitingLocationUpdatingResult",
            Self::AwaitingParameterUpdateConfirm => "AwaitingParameterUpdateConfirm",
            Self::AwaitingPrivacyModeComplete => "AwaitingPrivacyModeComplete",
            Self::Updated => "Updated",
            Self::Rejected => "Rejected",
            Self::TimedOut => "TimedOut",
        }
    }
}

/// Timers driven by the mobility-management procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobilityManagementTimer {
    T3210LocationUpdating,
    T3220ParameterUpdate,
    T3260Authentication,
    T3270BaseStationChallenge,
    T3271SsdUpdateResponse,
    T3280PrivacyMode,
}

/// Tracks the A1 mobility-management exchanges currently modeled by the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobilityManagementProcedure {
    state: MobilityManagementState,
}

impl MobilityManagementProcedure {
    /// Creates a new mobility-management procedure in the `Idle` state.
    pub fn new() -> Self {
        Self {
            state: MobilityManagementState::Idle,
        }
    }

    /// Returns the current mobility-management state.
    pub fn state(&self) -> MobilityManagementState {
        self.state
    }

    /// Applies `Authentication Request` and arms timer `T3260`.
    pub fn on_authentication_request(
        &mut self,
        _message: &AuthenticationRequestMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        self.enter_awaiting_state(
            MobilityManagementState::AwaitingAuthenticationResponse,
            MobilityManagementTimer::T3260Authentication,
            "Authentication Request is only valid from Idle, Updated, Rejected, or TimedOut",
        )
    }

    /// Applies `Authentication Response` and cancels timer `T3260`.
    pub fn on_authentication_response(
        &mut self,
        _message: &AuthenticationResponseMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingAuthenticationResponse => {
                let previous = self.state;
                self.state = MobilityManagementState::Updated;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(
                        MobilityManagementTimer::T3260Authentication,
                    )),
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Authentication Response is only valid from AwaitingAuthenticationResponse",
            )),
        }
    }

    /// Applies `SSD Update Request` and arms timer `T3270`.
    pub fn on_ssd_update_request(
        &mut self,
        _message: &SsdUpdateRequestMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        self.enter_awaiting_state(
            MobilityManagementState::AwaitingBaseStationChallenge,
            MobilityManagementTimer::T3270BaseStationChallenge,
            "SSD Update Request is only valid from Idle, Updated, Rejected, or TimedOut",
        )
    }

    /// Applies `Base Station Challenge`, cancels `T3270`, and arms `T3271`.
    pub fn on_base_station_challenge(
        &mut self,
        _message: &BaseStationChallengeMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingBaseStationChallenge => {
                let previous = self.state;
                self.state = MobilityManagementState::AwaitingSsdUpdateResponse;
                Ok(transition_many(
                    previous,
                    self.state,
                    vec![
                        TimerAction::Cancel(MobilityManagementTimer::T3270BaseStationChallenge),
                        TimerAction::Arm(MobilityManagementTimer::T3271SsdUpdateResponse),
                    ],
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Base Station Challenge is only valid from AwaitingBaseStationChallenge",
            )),
        }
    }

    /// Applies `Base Station Challenge Response`.
    pub fn on_base_station_challenge_response(
        &mut self,
        _message: &BaseStationChallengeResponseMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingSsdUpdateResponse => {
                Ok(transition(self.state, self.state, None))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Base Station Challenge Response is only valid from AwaitingSsdUpdateResponse",
            )),
        }
    }

    /// Applies `SSD Update Response` and cancels `T3271`.
    pub fn on_ssd_update_response(
        &mut self,
        _message: &SsdUpdateResponseMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingSsdUpdateResponse => {
                let previous = self.state;
                self.state = MobilityManagementState::Updated;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(
                        MobilityManagementTimer::T3271SsdUpdateResponse,
                    )),
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "SSD Update Response is only valid from AwaitingSsdUpdateResponse",
            )),
        }
    }

    /// Applies `Location Updating Request` and arms timer `T3210`.
    pub fn on_location_updating_request(
        &mut self,
        _message: &LocationUpdatingRequestMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        self.enter_awaiting_state(
            MobilityManagementState::AwaitingLocationUpdatingResult,
            MobilityManagementTimer::T3210LocationUpdating,
            "Location Updating Request is only valid from Idle, Updated, Rejected, or TimedOut",
        )
    }

    /// Applies `Location Updating Accept` and cancels timer `T3210`.
    pub fn on_location_updating_accept(
        &mut self,
        _message: &LocationUpdatingAcceptMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingLocationUpdatingResult => {
                let previous = self.state;
                self.state = MobilityManagementState::Updated;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(
                        MobilityManagementTimer::T3210LocationUpdating,
                    )),
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Location Updating Accept is only valid from AwaitingLocationUpdatingResult",
            )),
        }
    }

    /// Applies `Location Updating Reject` and cancels timer `T3210`.
    pub fn on_location_updating_reject(
        &mut self,
        _message: &LocationUpdatingRejectMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingLocationUpdatingResult => {
                let previous = self.state;
                self.state = MobilityManagementState::Rejected;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(
                        MobilityManagementTimer::T3210LocationUpdating,
                    )),
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Location Updating Reject is only valid from AwaitingLocationUpdatingResult",
            )),
        }
    }

    /// Applies `User Zone Update`.
    pub fn on_user_zone_update(
        &mut self,
        _message: &UserZoneUpdateMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        let previous = self.state;
        self.state = MobilityManagementState::Updated;
        Ok(transition(previous, self.state, None))
    }

    /// Applies `Parameter Update Request` and arms timer `T3220`.
    pub fn on_parameter_update_request(
        &mut self,
        _message: &ParameterUpdateRequestMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        self.enter_awaiting_state(
            MobilityManagementState::AwaitingParameterUpdateConfirm,
            MobilityManagementTimer::T3220ParameterUpdate,
            "Parameter Update Request is only valid from Idle, Updated, Rejected, or TimedOut",
        )
    }

    /// Applies `Parameter Update Confirm` and cancels timer `T3220`.
    pub fn on_parameter_update_confirm(
        &mut self,
        _message: &ParameterUpdateConfirmMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingParameterUpdateConfirm => {
                let previous = self.state;
                self.state = MobilityManagementState::Updated;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(
                        MobilityManagementTimer::T3220ParameterUpdate,
                    )),
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Parameter Update Confirm is only valid from AwaitingParameterUpdateConfirm",
            )),
        }
    }

    /// Applies `Privacy Mode Command` and arms timer `T3280`.
    pub fn on_privacy_mode_command(
        &mut self,
        _message: &PrivacyModeCommandMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        self.enter_awaiting_state(
            MobilityManagementState::AwaitingPrivacyModeComplete,
            MobilityManagementTimer::T3280PrivacyMode,
            "Privacy Mode Command is only valid from Idle, Updated, Rejected, or TimedOut",
        )
    }

    /// Applies `Privacy Mode Complete` and cancels `T3280` when it was active.
    pub fn on_privacy_mode_complete(
        &mut self,
        _message: &PrivacyModeCompleteMessage,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::AwaitingPrivacyModeComplete => {
                let previous = self.state;
                self.state = MobilityManagementState::Updated;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(
                        MobilityManagementTimer::T3280PrivacyMode,
                    )),
                ))
            }
            MobilityManagementState::Idle
            | MobilityManagementState::Updated
            | MobilityManagementState::Rejected
            | MobilityManagementState::TimedOut => {
                let previous = self.state;
                self.state = MobilityManagementState::Updated;
                Ok(transition(previous, self.state, None))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                "Privacy Mode Complete is not valid while another mobility procedure is active",
            )),
        }
    }

    /// Applies mobility-management timer expiry.
    pub fn on_timer_expired(
        &mut self,
        timer: MobilityManagementTimer,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match (self.state, timer) {
            (
                MobilityManagementState::AwaitingLocationUpdatingResult,
                MobilityManagementTimer::T3210LocationUpdating,
            )
            | (
                MobilityManagementState::AwaitingParameterUpdateConfirm,
                MobilityManagementTimer::T3220ParameterUpdate,
            )
            | (
                MobilityManagementState::AwaitingAuthenticationResponse,
                MobilityManagementTimer::T3260Authentication,
            )
            | (
                MobilityManagementState::AwaitingBaseStationChallenge,
                MobilityManagementTimer::T3270BaseStationChallenge,
            )
            | (
                MobilityManagementState::AwaitingSsdUpdateResponse,
                MobilityManagementTimer::T3271SsdUpdateResponse,
            )
            | (
                MobilityManagementState::AwaitingPrivacyModeComplete,
                MobilityManagementTimer::T3280PrivacyMode,
            ) => {
                let previous = self.state;
                self.state = MobilityManagementState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (state, timer) => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                match timer {
                    MobilityManagementTimer::T3210LocationUpdating => {
                        "T3210 is only valid from AwaitingLocationUpdatingResult"
                    }
                    MobilityManagementTimer::T3220ParameterUpdate => {
                        "T3220 is only valid from AwaitingParameterUpdateConfirm"
                    }
                    MobilityManagementTimer::T3260Authentication => {
                        "T3260 is only valid from AwaitingAuthenticationResponse"
                    }
                    MobilityManagementTimer::T3270BaseStationChallenge => {
                        "T3270 is only valid from AwaitingBaseStationChallenge"
                    }
                    MobilityManagementTimer::T3271SsdUpdateResponse => {
                        "T3271 is only valid from AwaitingSsdUpdateResponse"
                    }
                    MobilityManagementTimer::T3280PrivacyMode => {
                        "T3280 is only valid from AwaitingPrivacyModeComplete"
                    }
                },
            )),
        }
    }

    fn enter_awaiting_state(
        &mut self,
        new_state: MobilityManagementState,
        timer: MobilityManagementTimer,
        reason: &'static str,
    ) -> Result<Transition<MobilityManagementState, MobilityManagementTimer>> {
        match self.state {
            MobilityManagementState::Idle
            | MobilityManagementState::Updated
            | MobilityManagementState::Rejected
            | MobilityManagementState::TimedOut => {
                let previous = self.state;
                self.state = new_state;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(timer)),
                ))
            }
            state => Err(invalid_transition(
                "MobilityManagement",
                state.as_str(),
                reason,
            )),
        }
    }
}

impl Default for MobilityManagementProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// Source-side A1 handoff state for the BSC that asks the MSC for handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceHandoffState {
    Idle,
    Required,
    Commanded,
    Commenced,
    Cleared,
    Rejected,
    TimedOut,
}

impl SourceHandoffState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Required => "Required",
            Self::Commanded => "Commanded",
            Self::Commenced => "Commenced",
            Self::Cleared => "Cleared",
            Self::Rejected => "Rejected",
            Self::TimedOut => "TimedOut",
        }
    }
}

/// Timers driven by the source-side handoff procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceHandoffTimer {
    Command,
    Clear,
}

/// Tracks the source-side `Handoff Required` to `Handoff Command` exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceHandoffProcedure {
    state: SourceHandoffState,
}

impl SourceHandoffProcedure {
    /// Creates a new source-side handoff procedure in the `Idle` state.
    pub fn new() -> Self {
        Self {
            state: SourceHandoffState::Idle,
        }
    }

    /// Returns the current source-side handoff state.
    pub fn state(&self) -> SourceHandoffState {
        self.state
    }

    /// Applies `Handoff Required` and arms the command timer.
    pub fn on_handoff_required(
        &mut self,
        _message: &HandoffRequiredMessage,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match self.state {
            SourceHandoffState::Idle => {
                let previous = self.state;
                self.state = SourceHandoffState::Required;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(SourceHandoffTimer::Command)),
                ))
            }
            state => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff Required is only valid from Idle",
            )),
        }
    }

    /// Applies `Handoff Command` and cancels the command timer.
    pub fn on_handoff_command(
        &mut self,
        _message: &HandoffCommandMessage,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match self.state {
            SourceHandoffState::Required => {
                let previous = self.state;
                self.state = SourceHandoffState::Commanded;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(SourceHandoffTimer::Command)),
                ))
            }
            state => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff Command is only valid from Required",
            )),
        }
    }

    /// Applies `Handoff Commenced` and arms the clear timer (`T306`).
    pub fn on_handoff_commenced(
        &mut self,
        _message: &HandoffCommencedMessage,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match self.state {
            SourceHandoffState::Commanded => {
                let previous = self.state;
                self.state = SourceHandoffState::Commenced;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(SourceHandoffTimer::Clear)),
                ))
            }
            state => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff Commenced is only valid from Commanded",
            )),
        }
    }

    /// Applies `Handoff Required Reject` and cancels the command timer.
    pub fn on_handoff_required_reject(
        &mut self,
        _message: &HandoffRequiredRejectMessage,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match self.state {
            SourceHandoffState::Required => {
                let previous = self.state;
                self.state = SourceHandoffState::Rejected;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(SourceHandoffTimer::Command)),
                ))
            }
            state => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff Required Reject is only valid from Required",
            )),
        }
    }

    /// Applies `Clear Command` and cancels the clear timer (`T306`).
    pub fn on_clear_command(
        &mut self,
        _message: &ClearCommandMessage,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match self.state {
            SourceHandoffState::Commenced => {
                let previous = self.state;
                self.state = SourceHandoffState::Cleared;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(SourceHandoffTimer::Clear)),
                ))
            }
            state => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Clear Command is only valid from Commenced",
            )),
        }
    }

    /// Applies `Handoff Performed` without altering the current handoff state.
    pub fn on_handoff_performed(
        &mut self,
        _message: &HandoffPerformedMessage,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match self.state {
            SourceHandoffState::Idle
            | SourceHandoffState::Required
            | SourceHandoffState::Commanded
            | SourceHandoffState::Commenced
            | SourceHandoffState::Cleared => Ok(transition(self.state, self.state, None)),
            state => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff Performed is not valid after handoff rejection or timeout",
            )),
        }
    }

    /// Applies source-side handoff timer expiry.
    pub fn on_timer_expired(
        &mut self,
        timer: SourceHandoffTimer,
    ) -> Result<Transition<SourceHandoffState, SourceHandoffTimer>> {
        match (self.state, timer) {
            (SourceHandoffState::Required, SourceHandoffTimer::Command) => {
                let previous = self.state;
                self.state = SourceHandoffState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (SourceHandoffState::Commenced, SourceHandoffTimer::Clear) => {
                let previous = self.state;
                self.state = SourceHandoffState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (state, SourceHandoffTimer::Command) => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff command timer is only valid from Required",
            )),
            (state, SourceHandoffTimer::Clear) => Err(invalid_transition(
                "SourceHandoff",
                state.as_str(),
                "Handoff clear timer is only valid from Commenced",
            )),
        }
    }
}

impl Default for SourceHandoffProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// Target-side A1 handoff state for the BSC that receives `Handoff Request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetHandoffState {
    Idle,
    Requested,
    AwaitingArrival,
    Completed,
    Failed,
    TimedOut,
}

impl TargetHandoffState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Requested => "Requested",
            Self::AwaitingArrival => "AwaitingArrival",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::TimedOut => "TimedOut",
        }
    }
}

/// Timers driven by the target-side handoff procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetHandoffTimer {
    Response,
    Arrival,
}

/// Tracks the target-side `Handoff Request` to acknowledge or failure exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetHandoffProcedure {
    state: TargetHandoffState,
}

impl TargetHandoffProcedure {
    /// Creates a new target-side handoff procedure in the `Idle` state.
    pub fn new() -> Self {
        Self {
            state: TargetHandoffState::Idle,
        }
    }

    /// Returns the current target-side handoff state.
    pub fn state(&self) -> TargetHandoffState {
        self.state
    }

    /// Applies `Handoff Request` and arms the response timer.
    pub fn on_handoff_request(
        &mut self,
        _message: &HandoffRequestMessage,
    ) -> Result<Transition<TargetHandoffState, TargetHandoffTimer>> {
        match self.state {
            TargetHandoffState::Idle => {
                let previous = self.state;
                self.state = TargetHandoffState::Requested;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Arm(TargetHandoffTimer::Response)),
                ))
            }
            state => Err(invalid_transition(
                "TargetHandoff",
                state.as_str(),
                "Handoff Request is only valid from Idle",
            )),
        }
    }

    /// Applies `Handoff Request Acknowledge`, stops the response timer, and arms `T9`.
    pub fn on_handoff_request_acknowledge(
        &mut self,
        _message: &HandoffRequestAcknowledgeMessage,
    ) -> Result<Transition<TargetHandoffState, TargetHandoffTimer>> {
        match self.state {
            TargetHandoffState::Requested => {
                let previous = self.state;
                self.state = TargetHandoffState::AwaitingArrival;
                Ok(transition_many(
                    previous,
                    self.state,
                    vec![
                        TimerAction::Cancel(TargetHandoffTimer::Response),
                        TimerAction::Arm(TargetHandoffTimer::Arrival),
                    ],
                ))
            }
            state => Err(invalid_transition(
                "TargetHandoff",
                state.as_str(),
                "Handoff Request Acknowledge is only valid from Requested",
            )),
        }
    }

    /// Applies `Handoff Complete` and cancels `T9`.
    pub fn on_handoff_complete(
        &mut self,
        _message: &HandoffCompleteMessage,
    ) -> Result<Transition<TargetHandoffState, TargetHandoffTimer>> {
        match self.state {
            TargetHandoffState::AwaitingArrival => {
                let previous = self.state;
                self.state = TargetHandoffState::Completed;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(TargetHandoffTimer::Arrival)),
                ))
            }
            state => Err(invalid_transition(
                "TargetHandoff",
                state.as_str(),
                "Handoff Complete is only valid from AwaitingArrival",
            )),
        }
    }

    /// Applies `Handoff Failure` and cancels the active target-handoff timer.
    pub fn on_handoff_failure(
        &mut self,
        _message: &HandoffFailureMessage,
    ) -> Result<Transition<TargetHandoffState, TargetHandoffTimer>> {
        match self.state {
            TargetHandoffState::Requested => {
                let previous = self.state;
                self.state = TargetHandoffState::Failed;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(TargetHandoffTimer::Response)),
                ))
            }
            TargetHandoffState::AwaitingArrival => {
                let previous = self.state;
                self.state = TargetHandoffState::Failed;
                Ok(transition(
                    previous,
                    self.state,
                    Some(TimerAction::Cancel(TargetHandoffTimer::Arrival)),
                ))
            }
            state => Err(invalid_transition(
                "TargetHandoff",
                state.as_str(),
                "Handoff Failure is only valid from Requested or AwaitingArrival",
            )),
        }
    }

    /// Applies target-side handoff timer expiry.
    pub fn on_timer_expired(
        &mut self,
        timer: TargetHandoffTimer,
    ) -> Result<Transition<TargetHandoffState, TargetHandoffTimer>> {
        match (self.state, timer) {
            (TargetHandoffState::Requested, TargetHandoffTimer::Response) => {
                let previous = self.state;
                self.state = TargetHandoffState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (TargetHandoffState::AwaitingArrival, TargetHandoffTimer::Arrival) => {
                let previous = self.state;
                self.state = TargetHandoffState::TimedOut;
                Ok(transition(previous, self.state, None))
            }
            (state, TargetHandoffTimer::Response) => Err(invalid_transition(
                "TargetHandoff",
                state.as_str(),
                "Handoff response timer is only valid from Requested",
            )),
            (state, TargetHandoffTimer::Arrival) => Err(invalid_transition(
                "TargetHandoff",
                state.as_str(),
                "Handoff arrival timer is only valid from AwaitingArrival",
            )),
        }
    }
}

impl Default for TargetHandoffProcedure {
    fn default() -> Self {
        Self::new()
    }
}

/// Typed A1 messages accepted by the procedure engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcedureMessage {
    CompleteLayer3Information(CompleteLayer3InformationMessage),
    CmServiceRequest(CmServiceRequestMessage),
    PagingRequest(PagingRequestMessage),
    PagingResponse(PagingResponseMessage),
    AuthenticationRequest(AuthenticationRequestMessage),
    AuthenticationResponse(AuthenticationResponseMessage),
    SsdUpdateRequest(SsdUpdateRequestMessage),
    BaseStationChallenge(BaseStationChallengeMessage),
    BaseStationChallengeResponse(BaseStationChallengeResponseMessage),
    SsdUpdateResponse(SsdUpdateResponseMessage),
    LocationUpdatingRequest(LocationUpdatingRequestMessage),
    Connect(ConnectMessage),
    Progress(ProgressMessage),
    AssignmentRequest(AssignmentRequestMessage),
    AssignmentComplete(AssignmentCompleteMessage),
    AssignmentFailure(AssignmentFailureMessage),
    ClearRequest(ClearRequestMessage),
    ClearCommand(ClearCommandMessage),
    ClearComplete(ClearCompleteMessage),
    AlertWithInformation(AlertWithInformationMessage),
    BsServiceRequest(BsServiceRequestMessage),
    BsServiceResponse(BsServiceResponseMessage),
    UserZoneUpdate(UserZoneUpdateMessage),
    ParameterUpdateRequest(ParameterUpdateRequestMessage),
    ParameterUpdateConfirm(ParameterUpdateConfirmMessage),
    PrivacyModeCommand(PrivacyModeCommandMessage),
    PrivacyModeComplete(PrivacyModeCompleteMessage),
    LocationUpdatingAccept(LocationUpdatingAcceptMessage),
    LocationUpdatingReject(LocationUpdatingRejectMessage),
    HandoffRequired(HandoffRequiredMessage),
    HandoffRequest(HandoffRequestMessage),
    HandoffRequestAcknowledge(HandoffRequestAcknowledgeMessage),
    HandoffFailure(HandoffFailureMessage),
    HandoffCommand(HandoffCommandMessage),
    HandoffRequiredReject(HandoffRequiredRejectMessage),
    HandoffCommenced(HandoffCommencedMessage),
    HandoffComplete(HandoffCompleteMessage),
    HandoffPerformed(HandoffPerformedMessage),
}

impl ProcedureMessage {
    /// Returns the message name used in diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Self::CompleteLayer3Information(_) => "Complete Layer 3 Information",
            Self::CmServiceRequest(_) => "CM Service Request",
            Self::PagingRequest(_) => "Paging Request",
            Self::PagingResponse(_) => "Paging Response",
            Self::AuthenticationRequest(_) => "Authentication Request",
            Self::AuthenticationResponse(_) => "Authentication Response",
            Self::SsdUpdateRequest(_) => "SSD Update Request",
            Self::BaseStationChallenge(_) => "Base Station Challenge",
            Self::BaseStationChallengeResponse(_) => "Base Station Challenge Response",
            Self::SsdUpdateResponse(_) => "SSD Update Response",
            Self::LocationUpdatingRequest(_) => "Location Updating Request",
            Self::Connect(_) => "Connect",
            Self::Progress(_) => "Progress",
            Self::AssignmentRequest(_) => "Assignment Request",
            Self::AssignmentComplete(_) => "Assignment Complete",
            Self::AssignmentFailure(_) => "Assignment Failure",
            Self::ClearRequest(_) => "Clear Request",
            Self::ClearCommand(_) => "Clear Command",
            Self::ClearComplete(_) => "Clear Complete",
            Self::AlertWithInformation(_) => "Alert With Information",
            Self::BsServiceRequest(_) => "BS Service Request",
            Self::BsServiceResponse(_) => "BS Service Response",
            Self::UserZoneUpdate(_) => "User Zone Update",
            Self::ParameterUpdateRequest(_) => "Parameter Update Request",
            Self::ParameterUpdateConfirm(_) => "Parameter Update Confirm",
            Self::PrivacyModeCommand(_) => "Privacy Mode Command",
            Self::PrivacyModeComplete(_) => "Privacy Mode Complete",
            Self::LocationUpdatingAccept(_) => "Location Updating Accept",
            Self::LocationUpdatingReject(_) => "Location Updating Reject",
            Self::HandoffRequired(_) => "Handoff Required",
            Self::HandoffRequest(_) => "Handoff Request",
            Self::HandoffRequestAcknowledge(_) => "Handoff Request Acknowledge",
            Self::HandoffFailure(_) => "Handoff Failure",
            Self::HandoffCommand(_) => "Handoff Command",
            Self::HandoffRequiredReject(_) => "Handoff Required Reject",
            Self::HandoffCommenced(_) => "Handoff Commenced",
            Self::HandoffComplete(_) => "Handoff Complete",
            Self::HandoffPerformed(_) => "Handoff Performed",
        }
    }

    /// Returns the normal A1 direction for the message.
    pub fn expected_direction(&self) -> ProcedureDirection {
        match self {
            Self::CompleteLayer3Information(_)
            | Self::CmServiceRequest(_)
            | Self::PagingResponse(_)
            | Self::AuthenticationResponse(_)
            | Self::BaseStationChallenge(_)
            | Self::SsdUpdateResponse(_)
            | Self::LocationUpdatingRequest(_)
            | Self::Connect(_)
            | Self::AssignmentComplete(_)
            | Self::AssignmentFailure(_)
            | Self::ClearRequest(_)
            | Self::ClearComplete(_)
            | Self::BsServiceRequest(_)
            | Self::UserZoneUpdate(_)
            | Self::ParameterUpdateConfirm(_)
            | Self::PrivacyModeComplete(_)
            | Self::HandoffRequired(_)
            | Self::HandoffRequestAcknowledge(_)
            | Self::HandoffFailure(_)
            | Self::HandoffCommenced(_)
            | Self::HandoffComplete(_)
            | Self::HandoffPerformed(_) => ProcedureDirection::BscToMsc,
            Self::PagingRequest(_)
            | Self::AuthenticationRequest(_)
            | Self::SsdUpdateRequest(_)
            | Self::BaseStationChallengeResponse(_)
            | Self::AssignmentRequest(_)
            | Self::ClearCommand(_)
            | Self::HandoffRequest(_)
            | Self::HandoffCommand(_)
            | Self::HandoffRequiredReject(_)
            | Self::Progress(_)
            | Self::AlertWithInformation(_)
            | Self::ParameterUpdateRequest(_)
            | Self::PrivacyModeCommand(_)
            | Self::LocationUpdatingAccept(_)
            | Self::LocationUpdatingReject(_)
            | Self::BsServiceResponse(_) => ProcedureDirection::MscToBsc,
        }
    }
}

/// High-level event emitted by the composed A1 procedure engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    CallControl(Transition<CallControlState, CallControlTimer>),
    BsService(Transition<BsServiceState, BsServiceTimer>),
    MobilityManagement(Transition<MobilityManagementState, MobilityManagementTimer>),
    SourceHandoff(Transition<SourceHandoffState, SourceHandoffTimer>),
    TargetHandoff(Transition<TargetHandoffState, TargetHandoffTimer>),
}

/// Timer families consumed by the composed A1 procedure engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineTimer {
    CallControl(CallControlTimer),
    BsService(BsServiceTimer),
    MobilityManagement(MobilityManagementTimer),
    SourceHandoff(SourceHandoffTimer),
    TargetHandoff(TargetHandoffTimer),
}

/// Composes the A1 call-control, BS-service, and handoff procedures behind a
/// typed message router with direction validation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcedureEngine {
    call_control: CallControlProcedure,
    bs_service: BsServiceProcedure,
    mobility_management: MobilityManagementProcedure,
    source_handoff: SourceHandoffProcedure,
    target_handoff: TargetHandoffProcedure,
}

impl ProcedureEngine {
    /// Creates a fresh engine with all subprocedures in their initial state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current call-control subprocedure.
    pub fn call_control(&self) -> CallControlProcedure {
        self.call_control
    }

    /// Returns the current BS-service subprocedure.
    pub fn bs_service(&self) -> BsServiceProcedure {
        self.bs_service
    }

    /// Returns the current mobility-management subprocedure.
    pub fn mobility_management(&self) -> MobilityManagementProcedure {
        self.mobility_management
    }

    /// Returns the current source-side handoff subprocedure.
    pub fn source_handoff(&self) -> SourceHandoffProcedure {
        self.source_handoff
    }

    /// Returns the current target-side handoff subprocedure.
    pub fn target_handoff(&self) -> TargetHandoffProcedure {
        self.target_handoff
    }

    /// Applies a typed message in the stated direction.
    pub fn apply(
        &mut self,
        direction: ProcedureDirection,
        message: &ProcedureMessage,
    ) -> Result<EngineEvent> {
        let expected = message.expected_direction();
        if direction != expected {
            return Err(ProcedureError::InvalidDirection {
                procedure: "A1ProcedureEngine",
                message: message.name(),
                expected,
                actual: direction,
            });
        }

        match message {
            ProcedureMessage::CompleteLayer3Information(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_complete_layer3_information(message)?,
            )),
            ProcedureMessage::CmServiceRequest(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_cm_service_request(message)?,
            )),
            ProcedureMessage::PagingRequest(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_paging_request(message)?,
            )),
            ProcedureMessage::PagingResponse(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_paging_response(message)?,
            )),
            ProcedureMessage::AuthenticationRequest(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_authentication_request(message)?,
                ))
            }
            ProcedureMessage::AuthenticationResponse(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_authentication_response(message)?,
                ))
            }
            ProcedureMessage::SsdUpdateRequest(message) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management.on_ssd_update_request(message)?,
            )),
            ProcedureMessage::BaseStationChallenge(message) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management
                    .on_base_station_challenge(message)?,
            )),
            ProcedureMessage::BaseStationChallengeResponse(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_base_station_challenge_response(message)?,
                ))
            }
            ProcedureMessage::SsdUpdateResponse(message) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management.on_ssd_update_response(message)?,
            )),
            ProcedureMessage::LocationUpdatingRequest(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_location_updating_request(message)?,
                ))
            }
            ProcedureMessage::Connect(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_connect(message)?,
            )),
            ProcedureMessage::Progress(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_progress(message)?,
            )),
            ProcedureMessage::AssignmentRequest(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_assignment_request(message)?,
            )),
            ProcedureMessage::AssignmentComplete(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_assignment_complete(message)?,
            )),
            ProcedureMessage::AssignmentFailure(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_assignment_failure(message)?,
            )),
            ProcedureMessage::ClearRequest(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_clear_request(message)?,
            )),
            ProcedureMessage::ClearCommand(message) => Ok(EngineEvent::CallControl({
                let transition = self.call_control.on_clear_command(message)?;
                if self.source_handoff.state() == SourceHandoffState::Commenced {
                    let _ = self.source_handoff.on_clear_command(message)?;
                }
                transition
            })),
            ProcedureMessage::ClearComplete(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_clear_complete(message)?,
            )),
            ProcedureMessage::AlertWithInformation(message) => Ok(EngineEvent::CallControl(
                self.call_control.on_alert_with_information(message)?,
            )),
            ProcedureMessage::BsServiceRequest(message) => {
                Ok(EngineEvent::BsService(self.bs_service.on_request(message)?))
            }
            ProcedureMessage::BsServiceResponse(message) => Ok(EngineEvent::BsService(
                self.bs_service.on_response(message)?,
            )),
            ProcedureMessage::UserZoneUpdate(message) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management.on_user_zone_update(message)?,
            )),
            ProcedureMessage::ParameterUpdateRequest(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_parameter_update_request(message)?,
                ))
            }
            ProcedureMessage::ParameterUpdateConfirm(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_parameter_update_confirm(message)?,
                ))
            }
            ProcedureMessage::PrivacyModeCommand(message) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management.on_privacy_mode_command(message)?,
            )),
            ProcedureMessage::PrivacyModeComplete(message) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management.on_privacy_mode_complete(message)?,
            )),
            ProcedureMessage::LocationUpdatingAccept(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_location_updating_accept(message)?,
                ))
            }
            ProcedureMessage::LocationUpdatingReject(message) => {
                Ok(EngineEvent::MobilityManagement(
                    self.mobility_management
                        .on_location_updating_reject(message)?,
                ))
            }
            ProcedureMessage::HandoffRequired(message) => Ok(EngineEvent::SourceHandoff(
                self.source_handoff.on_handoff_required(message)?,
            )),
            ProcedureMessage::HandoffRequest(message) => Ok(EngineEvent::TargetHandoff(
                self.target_handoff.on_handoff_request(message)?,
            )),
            ProcedureMessage::HandoffRequestAcknowledge(message) => Ok(EngineEvent::TargetHandoff(
                self.target_handoff
                    .on_handoff_request_acknowledge(message)?,
            )),
            ProcedureMessage::HandoffFailure(message) => Ok(EngineEvent::TargetHandoff(
                self.target_handoff.on_handoff_failure(message)?,
            )),
            ProcedureMessage::HandoffCommand(message) => Ok(EngineEvent::SourceHandoff(
                self.source_handoff.on_handoff_command(message)?,
            )),
            ProcedureMessage::HandoffRequiredReject(message) => Ok(EngineEvent::SourceHandoff(
                self.source_handoff.on_handoff_required_reject(message)?,
            )),
            ProcedureMessage::HandoffCommenced(message) => Ok(EngineEvent::SourceHandoff(
                self.source_handoff.on_handoff_commenced(message)?,
            )),
            ProcedureMessage::HandoffComplete(message) => Ok(EngineEvent::TargetHandoff(
                self.target_handoff.on_handoff_complete(message)?,
            )),
            ProcedureMessage::HandoffPerformed(message) => Ok(EngineEvent::SourceHandoff(
                self.source_handoff.on_handoff_performed(message)?,
            )),
        }
    }

    /// Applies timer expiry to one of the composed subprocedures.
    pub fn on_timer_expired(&mut self, timer: EngineTimer) -> Result<EngineEvent> {
        match timer {
            EngineTimer::CallControl(timer) => Ok(EngineEvent::CallControl(
                self.call_control.on_timer_expired(timer)?,
            )),
            EngineTimer::BsService(timer) => Ok(EngineEvent::BsService(
                self.bs_service.on_timer_expired(timer)?,
            )),
            EngineTimer::MobilityManagement(timer) => Ok(EngineEvent::MobilityManagement(
                self.mobility_management.on_timer_expired(timer)?,
            )),
            EngineTimer::SourceHandoff(timer) => Ok(EngineEvent::SourceHandoff(
                self.source_handoff.on_timer_expired(timer)?,
            )),
            EngineTimer::TargetHandoff(timer) => Ok(EngineEvent::TargetHandoff(
                self.target_handoff.on_timer_expired(timer)?,
            )),
        }
    }
}
