//! Independent A1 call contexts for mobile-to-mobile voice.
//!
//! Both assignments run concurrently. Alerting waits for both services, and
//! bearer forwarding starts only after the callee answers. Clearing either
//! context releases both mobiles.

use cdma_ios::{CallControlState, ConnectMessage, ProcedureMessage};
use log::{info, warn};

use crate::call_control::CallId;
use crate::circuit::M2mCall;
use crate::media_gateway_service::{
    send_alert_with_information, send_progress_ringback, send_progress_tones_off,
};

use super::{MscA1Endpoint, MscRuntime};

impl MscRuntime {
    pub(super) async fn start_m2m_callee(
        &mut self,
        a1: &dyn MscA1Endpoint,
        caller: CallId,
        destination: cdma_hlr::model::ResolvedSubscriber,
        calling_number: Option<String>,
    ) {
        if self
            .media_gw
            .is_subscriber_busy(destination.subscriber.subscriber_id)
        {
            info!("MSC: M2M destination busy caller_call_id={}", caller.0);
            self.send_clear_command(a1, caller).await;
            return;
        }
        let callee = match self
            .start_mt_call(a1, destination, calling_number, None)
            .await
        {
            Ok(callee) => callee,
            Err(error) => {
                warn!(
                    "MSC: M2M page failed caller_call_id={}: {:?}",
                    caller.0, error
                );
                self.send_clear_command(a1, caller).await;
                return;
            }
        };
        let call = M2mCall { caller, callee };
        self.circuits.m2m_calls.insert(caller, call);
        self.circuits.m2m_calls.insert(callee, call);
        info!(
            "MSC: M2M setup running independently caller_call_id={} callee_call_id={}",
            caller.0, callee.0
        );
    }

    fn m2m_leg_ready(&self, call_id: CallId) -> bool {
        matches!(
            self.controller.state(call_id),
            Some(CallControlState::Assigned | CallControlState::Alerting)
        )
    }

    pub(super) async fn complete_m2m_assignment(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call: M2mCall,
        completed: CallId,
    ) {
        info!(
            "MSC: M2M service ready call_id={} caller_call_id={} callee_call_id={}",
            completed.0, call.caller.0, call.callee.0
        );
        if completed == call.caller {
            self.media.start_ringback_for_call(
                call.caller,
                &self.controller,
                &self.circuits,
                self.config.voice_bearer.as_ref(),
                self.config.media_ringback_enabled,
                self.config.media_ringback_type,
                Some(&self.config.hlr_repo),
            );
            if self.config.send_tones_alert {
                send_progress_ringback(a1, call.caller, &mut self.controller).await;
            }
        }
        if !self.m2m_leg_ready(call.caller) || !self.m2m_leg_ready(call.callee) {
            return;
        }
        let digits = self.mt_call.caller_numbers.get(&call.callee).cloned();
        let records = crate::mt_call::build_calling_party_ms_information_records(
            digits.as_deref(),
            &self.config.hlr_repo,
        )
        .await;
        send_alert_with_information(
            a1,
            call.callee,
            &mut self.controller,
            &mut self.media_gw.alert_sent,
            records,
        )
        .await;
    }

    pub(super) async fn connect_m2m_call(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call: M2mCall,
        answering: CallId,
    ) {
        if answering != call.callee
            || !self.m2m_leg_ready(call.caller)
            || self.controller.state(call.callee) != Some(CallControlState::Alerting)
        {
            warn!(
                "MSC: ignoring M2M Connect outside callee alerting call_id={}",
                answering.0
            );
            return;
        }
        send_progress_tones_off(a1, call.caller, &mut self.controller).await;
        for leg in [call.caller, call.callee] {
            if let Err(error) = self
                .controller
                .apply_from_bsc(leg, &ProcedureMessage::Connect(ConnectMessage))
            {
                warn!(
                    "MSC: failed to connect M2M leg call_id={}: {}",
                    leg.0, error
                );
                self.send_clear_command(a1, leg).await;
                return;
            }
        }
        self.media
            .stop_ringback_for_call(call.caller, &self.circuits);
        info!(
            "MSC: M2M answered caller_call_id={} callee_call_id={}",
            call.caller.0, call.callee.0
        );
    }

    pub(super) async fn send_clear_command(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let Some(call) = self.circuits.m2m_calls.get(&call_id).copied() else {
            self.send_clear_command_for_leg(a1, call_id).await;
            return;
        };
        info!(
            "MSC: clearing M2M caller_call_id={} callee_call_id={} released_call_id={}",
            call.caller.0, call.callee.0, call_id.0
        );
        for leg in [call.caller, call.callee] {
            self.send_clear_command_for_leg(a1, leg).await;
            self.mt_page_retry.cancel(leg);
            self.mt_call.mt_plans.remove(&(leg.0 as u32));
            // A paged mobile may never answer, so cleanup cannot depend on ClearComplete.
            self.controller.remove_call(leg);
            self.stop_media_for_call(leg);
            a1.release_call(leg);
        }
    }
}
