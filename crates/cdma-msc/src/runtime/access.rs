use cdma_ios::EncodedA1Message;
use log::{debug, info, warn};

use crate::admission::{
    AccessSubject, Admission, AdmissionClass, AdmissionReason, AdmissionService, CredentialState,
};
use crate::base_station::BaseStationId;
use crate::call_control::CallId;
use crate::grpc::events_proto::v1 as events_proto;

use super::{MscA1Endpoint, MscRuntime};

/// A.S0014-D §4.2.16 Cause "Call processing". The reason is in Cause Layer 3.
const CAUSE_CALL_PROCESSING: u8 = 0x09;
/// ITU-T Q.850 cause 21 "Call rejected".
const Q850_CALL_REJECTED: u8 = 21;
const Q850_CODING_STANDARD_ITU: u8 = 0;
const Q850_LOCATION_PUBLIC_NETWORK_LOCAL_USER: u8 = 2;

/// The subscriber and credential state a provisioning call was authorized
/// against. The session starts only if both still hold.
#[derive(Clone, Debug)]
pub(crate) struct ProvisioningAuthorization {
    pub subscriber_id: uuid::Uuid,
    pub identity: cdma_hlr::model::MobileIdentityKey,
    pub credentials: CredentialState,
}

impl MscRuntime {
    pub(crate) fn access_closed(&self) -> bool {
        self.config.access.closed()
    }

    pub(crate) async fn decide_admission(&self, subject: &AccessSubject<'_>) -> Admission {
        if !self.access_closed() {
            return Admission::open();
        }
        crate::admission::classify(self.config.hlr_repo.as_ref(), subject).await
    }

    pub(crate) async fn provisioning_target(&self, subject: &AccessSubject<'_>) -> Admission {
        crate::admission::classify(self.config.hlr_repo.as_ref(), subject).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn report_admission(
        &mut self,
        node: &BaseStationId,
        procedure: events_proto::AuthenticationProcedure,
        service: AdmissionService,
        subject: &AccessSubject<'_>,
        admission: &Admission,
        rejected: bool,
        reason: Option<AdmissionReason>,
    ) {
        let reason = reason.or(admission.reason);
        let event = crate::admission::admission_event(
            procedure, service, subject, admission, reason, rejected, &node.0,
        );
        let changed = if let Some(events_proto::msc_network_event::Event::Admission(verdict)) =
            &event.event
        {
            self.admission_history.update_current(verdict)
        } else {
            false
        };
        if admission.class == AdmissionClass::Admitted && !rejected && !changed {
            return;
        }
        let who = subject.imsi.unwrap_or("unknown IMSI");
        if rejected {
            warn!(
                "MSC: access refused for {} {} ({}): class={} reason={}",
                service.label(),
                who,
                subject_hardware(subject),
                admission.class.label(),
                reason.map(|r| r.config_name()).unwrap_or("-"),
            );
        } else {
            info!(
                "MSC: access for {} {} ({}): class={} reason={}",
                service.label(),
                who,
                subject_hardware(subject),
                admission.class.label(),
                reason.map(|r| r.config_name()).unwrap_or("-"),
            );
        }
        if let Some(events_proto::msc_network_event::Event::Admission(verdict)) = &event.event {
            self.admission_history.record(verdict.clone());
        }
        let _ = self.otasp_event_tx.send(event);
        let Some(identity) = subject.identity_key() else {
            debug!(
                "MSC: admission decision for {who} not stored, the mobile sent no full identity"
            );
            return;
        };
        if let Some(mut receiver) = self.admission_persistence_rx.take() {
            let hlr = self.config.hlr_repo.clone();
            tokio::spawn(async move {
                while let Some(write) = receiver.recv().await {
                    let (identity, admission) = match write {
                        crate::admission::AdmissionWrite::Record {
                            identity,
                            admission,
                        } => (identity, admission),
                        #[cfg(test)]
                        crate::admission::AdmissionWrite::Flush(done) => {
                            let _ = done.send(());
                            continue;
                        }
                    };
                    match tokio::time::timeout(
                        crate::admission::ADMISSION_PERSISTENCE_TIMEOUT,
                        hlr.record_mobile_admission(&identity, &admission),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => warn!("MSC: failed to store admission decision: {error}"),
                        Err(_) => warn!("MSC: admission persistence timed out"),
                    }
                }
            });
        }
        if let Err(error) =
            self.admission_persistence
                .try_send(crate::admission::AdmissionWrite::Record {
                    identity,
                    admission: crate::admission::hlr_admission(admission, reason, &node.0),
                })
        {
            warn!("MSC: admission history not stored: {error}");
        }
    }

    #[cfg(test)]
    pub(crate) async fn flush_admission_history(&self) {
        if self.admission_persistence_rx.is_none() {
            let (done, wait) = tokio::sync::oneshot::channel();
            self.admission_persistence
                .send(crate::admission::AdmissionWrite::Flush(done))
                .await
                .unwrap();
            wait.await.unwrap();
        }
    }

    pub(crate) async fn send_location_updating_reject(
        &self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        transport_call_id: Option<u64>,
        cause: u8,
    ) {
        let reject = cdma_ios::LocationUpdatingRejectMessage {
            reject_cause: cdma_ios::RejectCause(cause),
        };
        match reject.encode() {
            Ok(payload) => {
                if let Err(error) = a1
                    .send_to_node(
                        node,
                        EncodedA1Message::from_message_for_call(
                            &cdma_ios::Message::new(
                                cdma_ios::MessageType::LocationUpdatingReject,
                                payload,
                            ),
                            transport_call_id,
                        ),
                    )
                    .await
                {
                    warn!("MSC: failed to send Location Updating Reject: {error}");
                }
            }
            Err(error) => warn!("MSC: failed to encode Location Updating Reject: {error}"),
        }
    }

    pub(crate) async fn send_location_updating_accept(
        &self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        transport_call_id: Option<u64>,
        location_area_identification: Option<cdma_ios::LocationAreaIdentification>,
    ) -> bool {
        let accept = cdma_ios::LocationUpdatingAcceptMessage {
            location_area_identification,
        };
        let payload = match accept.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!("MSC: failed to encode Location Updating Accept: {error}");
                return false;
            }
        };
        if let Err(error) = a1
            .send_to_node(
                node,
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(cdma_ios::MessageType::LocationUpdatingAccept, payload),
                    transport_call_id,
                ),
            )
            .await
        {
            warn!("MSC: failed to send Location Updating Accept: {error}");
            return false;
        }
        true
    }

    pub(crate) async fn reject_access_for_admission(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
    ) {
        let clear_command = cdma_ios::ClearCommandMessage {
            cause: cdma_ios::Cause(CAUSE_CALL_PROCESSING),
            cause_layer3: Some(cdma_ios::CauseLayer3 {
                coding_standard: Q850_CODING_STANDARD_ITU,
                location: Q850_LOCATION_PUBLIC_NETWORK_LOCAL_USER,
                cause_value: Q850_CALL_REJECTED,
            }),
        };
        match clear_command.encode() {
            Ok(payload) => {
                if let Err(error) = a1
                    .send(EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::ClearCommand, payload),
                        Some(call_id.0),
                    ))
                    .await
                {
                    warn!(
                        "MSC: failed to send access-refused ClearCommand call_id={}: {error}",
                        call_id.0
                    );
                }
            }
            Err(error) => warn!("MSC: failed to encode ClearCommand: {error}"),
        }
        self.provisioning_authorizations.remove(&call_id);
        self.provisioning_challenges.remove(&call_id);
        a1.release_call(call_id);
    }

    /// Rechecks the provisioning target right before its session starts. The
    /// subscriber must still be Active and its A-key/SSD presence unchanged.
    pub(crate) async fn provisioning_still_authorized(&mut self, call_id: CallId) -> bool {
        let Some(authorization) = self.provisioning_authorizations.remove(&call_id) else {
            return true;
        };
        let hlr = self.config.hlr_repo.as_ref();
        match hlr.get_subscriber_by_id(authorization.subscriber_id).await {
            Ok(Some(resolved))
                if resolved.subscriber.status == cdma_hlr::model::SubscriberStatus::Active => {}
            Ok(_) => {
                warn!(
                    "MSC: provisioning call_id={} refused, subscriber {} is no longer active",
                    call_id.0, authorization.subscriber_id
                );
                return false;
            }
            Err(error) => {
                warn!(
                    "MSC: provisioning call_id={} refused, subscriber recheck failed: {error}",
                    call_id.0
                );
                return false;
            }
        }
        match crate::admission::credential_state(hlr, &authorization.identity).await {
            Ok(state) if state == authorization.credentials => true,
            Ok(_) => {
                warn!(
                    "MSC: provisioning call_id={} refused, subscriber {} A-key/SSD presence changed since the call was authorized",
                    call_id.0, authorization.subscriber_id
                );
                false
            }
            Err(error) => {
                warn!(
                    "MSC: provisioning call_id={} refused, credential recheck failed: {error}",
                    call_id.0
                );
                false
            }
        }
    }
}

impl MscRuntime {
    pub(crate) async fn start_provisioning_session(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        initiation: crate::otasp::session::SessionInitiation,
        pending: super::PendingOtaspOrigination,
    ) {
        let target = self
            .provisioning_authorizations
            .get(&call_id)
            .map(|authorization| (authorization.subscriber_id, authorization.identity.clone()));
        if (target.is_none()
            && (self.access_closed()
                || initiation == crate::otasp::session::SessionInitiation::Network))
            || !self.provisioning_still_authorized(call_id).await
        {
            self.send_clear_command(a1, call_id).await;
            return;
        }
        let Some(otasp) = self.otasp.as_mut() else {
            return;
        };
        let started = otasp
            .begin_session_with_initiation(
                initiation,
                pending.device,
                pending.feature_code,
                pending.service_option,
                pending.mobile_identity_imsi,
                pending.origination_verified,
                call_id.0,
                a1,
                target,
            )
            .await;
        if !started {
            self.send_clear_command(a1, call_id).await;
        }
    }
}

impl MscRuntime {
    /// Applies the keyed-device rule to an OTAPA page response. Returns false
    /// after failing the call. A response without proof, which includes the
    /// one a base station sends for a mobile already on a traffic channel,
    /// leaves a unique challenge to run before the session starts.
    pub(crate) async fn authorize_otapa_response(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        call_id: CallId,
        subject: &AccessSubject<'_>,
        target: Option<cdma_hlr::model::MobileIdentityKey>,
        decision: &crate::authentication::AuthenticationDecision,
    ) -> bool {
        let subscriber_id = self
            .mt_call
            .mt_plans
            .get(&(call_id.0 as u32))
            .map(|plan| plan.subscriber_id);
        let (Some(target), Some(subscriber_id)) = (target, subscriber_id) else {
            warn!(
                "MSC: OTAPA call_id={} has no provisioned target identity",
                call_id.0
            );
            self.fail_mt_call(a1, call_id, super::SIP_STATUS_FORBIDDEN)
                .await;
            return false;
        };
        let credentials = match crate::admission::credential_state(
            self.config.hlr_repo.as_ref(),
            &target,
        )
        .await
        {
            Ok(credentials) => credentials,
            Err(error) => {
                warn!("MSC: OTAPA credential lookup failed: {error}");
                self.fail_mt_call(a1, call_id, super::SIP_STATUS_FORBIDDEN)
                    .await;
                return false;
            }
        };
        let check = crate::admission::provisioning_check(
            self.config.authentication.mode,
            credentials,
            decision.result,
            true,
        );
        let admission = Admission {
            class: AdmissionClass::Admitted,
            reason: None,
            subscriber: None,
            identity: Some(target.clone()),
        };
        match check {
            crate::admission::ProvisioningCheck::Refused(reason) => {
                self.report_admission(
                    node,
                    events_proto::AuthenticationProcedure::PageResponse,
                    AdmissionService::Otasp,
                    subject,
                    &admission,
                    true,
                    Some(reason),
                )
                .await;
                self.fail_mt_call(a1, call_id, super::SIP_STATUS_FORBIDDEN)
                    .await;
                return false;
            }
            crate::admission::ProvisioningCheck::ChallengeRequired => {
                let (Some(imsi), Some(esn)) = (subject.imsi, subject.esn) else {
                    warn!(
                        "MSC: OTAPA call_id={} cannot be challenged without IMSI and ESN",
                        call_id.0
                    );
                    self.fail_mt_call(a1, call_id, super::SIP_STATUS_FORBIDDEN)
                        .await;
                    return false;
                };
                info!(
                    "MSC: OTAPA call_id={} carried no authentication, a unique challenge runs before provisioning",
                    call_id.0
                );
                self.provisioning_challenges.insert(
                    call_id,
                    crate::authentication::CallAuthContext {
                        identity: target.clone(),
                        imsi: imsi.to_string(),
                        esn,
                        global_result: crate::authentication::AuthenticationResult::NotRequested,
                    },
                );
            }
            crate::admission::ProvisioningCheck::Allowed
            | crate::admission::ProvisioningCheck::Verified => {}
        }
        self.provisioning_authorizations.insert(
            call_id,
            ProvisioningAuthorization {
                subscriber_id,
                identity: target,
                credentials,
            },
        );
        true
    }

    pub(crate) async fn hold_for_provisioning_challenge(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        call_id: CallId,
        held: super::HeldAssignment,
    ) -> bool {
        let Some(context) = self.provisioning_challenges.remove(&call_id) else {
            return false;
        };
        let mut procedures = crate::authentication::TrafficAuthentication::provisioning_challenge(
            context,
            node.0.clone(),
        );
        let action = procedures
            .advance(&self.config.authentication, self.config.hlr_repo.as_ref())
            .await;
        self.traffic_auth_started.insert(call_id);
        self.traffic_auth.insert(call_id, procedures);
        self.provisioning_holds.insert(call_id, held);
        Box::pin(self.apply_traffic_action(a1, call_id, action)).await;
        true
    }
}

impl MscRuntime {
    pub(crate) async fn initiate_data_call(
        &mut self,
        a1: &dyn MscA1Endpoint,
        subscriber_id: uuid::Uuid,
        service_option: u16,
    ) -> Result<crate::management::InitiateCallAccepted, crate::management::ManagementError> {
        use crate::management::ManagementError;
        if !super::is_packet_data_service_option(service_option) {
            return Err(ManagementError::Rejected(
                "unsupported data service option".into(),
            ));
        }
        let resolved = self
            .config
            .hlr_repo
            .get_subscriber_by_id(subscriber_id)
            .await
            .map_err(ManagementError::Rejected)?
            .ok_or(ManagementError::UnknownSubscriber(subscriber_id))?;
        let call_id = self
            .start_mt_service(a1, resolved, None, None, Some(service_option))
            .await
            .map_err(super::MtSetupError::into_management_error)?;
        Ok(crate::management::InitiateCallAccepted { call_id })
    }

    pub(crate) async fn admit_traffic_sms(
        &mut self,
        node: &BaseStationId,
        call_id: CallId,
    ) -> bool {
        if !self.access_closed() {
            return true;
        }
        let snapshot = self.controller.snapshot(call_id);
        let imsi = snapshot
            .as_ref()
            .and_then(|snapshot| match snapshot.mobile_identity.as_ref() {
                Some(cdma_ios::MobileIdentity::Imsi(imsi)) => Some(imsi.as_str()),
                _ => None,
            });
        let esn =
            snapshot
                .as_ref()
                .and_then(|snapshot| match snapshot.mobile_identity_esn.as_ref() {
                    Some(cdma_ios::MobileIdentity::Esn(esn)) => Some(*esn),
                    _ => None,
                });
        let esn = esn.or_else(|| {
            self.call_auth_context
                .get(&call_id)
                .map(|context| context.esn)
        });
        let meid = self
            .call_auth_context
            .get(&call_id)
            .and_then(|context| context.identity.meid())
            .map(str::to_owned);
        let subject = AccessSubject {
            imsi,
            esn,
            meid: meid.as_deref(),
        };
        let admission = self.decide_admission(&subject).await;
        let allowed = admission.class.permits(AdmissionService::Sms);
        self.report_admission(
            node,
            events_proto::AuthenticationProcedure::DataBurst,
            AdmissionService::Sms,
            &subject,
            &admission,
            !allowed,
            (!allowed).then(|| admission.refusal_reason(AdmissionService::Sms)),
        )
        .await;
        allowed
    }
}

fn subject_hardware(subject: &AccessSubject<'_>) -> String {
    match (subject.esn, subject.meid) {
        (Some(esn), Some(meid)) => format!("ESN 0x{esn:08X} MEID {meid}"),
        (Some(esn), None) => format!("ESN 0x{esn:08X}"),
        (None, Some(meid)) => format!("MEID {meid}"),
        (None, None) => "no hardware identity".to_string(),
    }
}
