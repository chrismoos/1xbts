use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cdma_hlr::model::*;
use cdma_hlr::repository::HlrRepository;
use cdma_interconnect::{InterconnectEgress, InterconnectError, InterconnectSms};
use cdma_smsc::delivery::{AttemptResult, DeliveryHub, PushResult};
use cdma_smsc::dispatcher::{Deferral, Dispatcher};
use cdma_smsc::model::{
    DeliveryAttemptState, MoSmsFingerprint, SmsDeliveryAttempt, SmsDestination, SmsState,
    SmsSubmission,
};
use cdma_smsc::repository::{CreateSubmissionOptions, DeliveryCompletion, SmscRepository};
use cdma_smsc::routing::Router;
use cdma_smsc::scheduler::{Scheduler, SweepConfig};
use chrono::Utc;
use uuid::Uuid;

const SUBMIT_MESSAGE_TYPE: u8 = 0x02;

struct OneSubscriber {
    number: String,
    subscriber_id: Uuid,
    binding: Mutex<Option<RegistrationBinding>>,
}

impl OneSubscriber {
    fn registered(number: &str) -> Self {
        let subscriber_id = Uuid::new_v4();
        Self {
            number: number.to_string(),
            subscriber_id,
            binding: Mutex::new(Some(RegistrationBinding {
                subscriber_id,
                serving_bs_id: "bs-1".into(),
                state: RegistrationState::Registered,
                imsi: Some("310120000000001".into()),
                esn: None,
                meid: None,
                mob_p_rev: None,
                pgslot: None,
                slot_cycle_index: None,
                last_msg_seq: None,
                last_registered_at: Utc::now(),
                last_seen_at: Utc::now(),
                updated_at: Utc::now(),
            })),
        }
    }

    fn offline(number: &str) -> Self {
        let one = Self::registered(number);
        *one.binding.lock().unwrap() = None;
        one
    }

    fn registers(&self) {
        let restored = Self::registered(&self.number);
        let mut binding = restored.binding.lock().unwrap().clone();
        if let Some(binding) = binding.as_mut() {
            binding.subscriber_id = self.subscriber_id;
        }
        *self.binding.lock().unwrap() = binding;
    }

    fn resolved(&self) -> ResolvedSubscriber {
        ResolvedSubscriber {
            subscriber: Subscriber {
                subscriber_id: self.subscriber_id,
                phone_number: self.number.clone(),
                display_name: "test".into(),
                status: SubscriberStatus::Active,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                number_type: NumberType::National,
                number_plan: NumberPlan::IsdnE164,
                has_ringtone: false,
                ringtone_duration_ms: None,
                prl_override_id: None,
                service_programming_code: None,
                firstchp_override: None,
            },
            identities: Vec::new(),
            primary_identity: None,
            binding: self.binding.lock().unwrap().clone(),
        }
    }
}

#[async_trait]
impl HlrRepository for OneSubscriber {
    async fn get_subscriber_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        if phone_number == "unavailable" {
            return Err("HLR unavailable".into());
        }
        Ok((phone_number == self.number).then(|| self.resolved()))
    }

    async fn resolve_by_identity(
        &self,
        _: &MobileIdentityKey,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        Ok(Some(self.resolved()))
    }

    async fn upsert_subscriber(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: NumberType,
        _: NumberPlan,
    ) -> Result<Subscriber, String> {
        unimplemented!("upsert_subscriber is not part of these tests")
    }

    async fn get_subscriber_by_id(&self, _: Uuid) -> Result<Option<ResolvedSubscriber>, String> {
        unimplemented!("get_subscriber_by_id is not part of these tests")
    }

    async fn update_subscriber(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        _: &str,
        _: NumberType,
        _: NumberPlan,
    ) -> Result<Option<Subscriber>, String> {
        unimplemented!("update_subscriber is not part of these tests")
    }

    async fn list_subscribers(&self, _: u32, _: u32) -> Result<(Vec<Subscriber>, u32), String> {
        unimplemented!("list_subscribers is not part of these tests")
    }

    async fn delete_subscriber(&self, _: Uuid) -> Result<bool, String> {
        unimplemented!("delete_subscriber is not part of these tests")
    }

    async fn upsert_identity(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        unimplemented!("upsert_identity is not part of these tests")
    }

    async fn replace_primary_identity(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        unimplemented!("replace_primary_identity is not part of these tests")
    }

    async fn get_identities_for_subscriber(
        &self,
        _: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String> {
        unimplemented!("get_identities_for_subscriber is not part of these tests")
    }

    async fn resolve_by_hardware_identity(
        &self,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<Option<ResolvedSubscriber>, cdma_hlr::repository::HardwareLookupError> {
        unimplemented!("resolve_by_hardware_identity is not part of these tests")
    }

    async fn upsert_mobile_seen(
        &self,
        _: &MobileIdentityKey,
        _: Option<u8>,
    ) -> Result<MobileSeenUpsert, String> {
        unimplemented!("upsert_mobile_seen is not part of these tests")
    }

    async fn upsert_registration_binding(
        &self,
        _: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        unimplemented!("upsert_registration_binding is not part of these tests")
    }

    async fn get_registration_binding(
        &self,
        _: Uuid,
    ) -> Result<Option<RegistrationBinding>, String> {
        unimplemented!("get_registration_binding is not part of these tests")
    }

    async fn set_ringtone(
        &self,
        _: Uuid,
        _: Vec<u8>,
        _: &str,
    ) -> Result<SetRingtoneOutcome, String> {
        unimplemented!("set_ringtone is not part of these tests")
    }

    async fn clear_ringtone(&self, _: Uuid) -> Result<(), String> {
        unimplemented!("clear_ringtone is not part of these tests")
    }

    async fn get_ringtone_codec(
        &self,
        _: Uuid,
        _: &str,
    ) -> Result<Option<SubscriberRingtoneCodecBlob>, String> {
        unimplemented!("get_ringtone_codec is not part of these tests")
    }

    async fn list_prls(&self, _: u32, _: u32, _: PrlListFilter) -> Result<(Vec<Prl>, u32), String> {
        unimplemented!("list_prls is not part of these tests")
    }

    async fn get_prl(&self, _: Uuid) -> Result<Option<Prl>, String> {
        unimplemented!("get_prl is not part of these tests")
    }

    async fn get_default_prl(&self) -> Result<Option<Prl>, String> {
        unimplemented!("get_default_prl is not part of these tests")
    }

    async fn create_prl(&self, _: &str, _: &[u8], _: i32, _: i16, _: &str) -> Result<Prl, String> {
        unimplemented!("create_prl is not part of these tests")
    }

    async fn update_prl(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<&[u8]>,
        _: Option<(i32, i16)>,
        _: Option<&str>,
    ) -> Result<Prl, String> {
        unimplemented!("update_prl is not part of these tests")
    }

    async fn soft_delete_prl(&self, _: Uuid) -> Result<Result<(), PrlDeleteBlocked>, String> {
        unimplemented!("soft_delete_prl is not part of these tests")
    }

    async fn set_default_prl(&self, _: Uuid) -> Result<(), String> {
        unimplemented!("set_default_prl is not part of these tests")
    }

    async fn set_subscriber_prl_override(&self, _: Uuid, _: Option<Uuid>) -> Result<(), String> {
        unimplemented!("set_subscriber_prl_override is not part of these tests")
    }

    async fn set_subscriber_spc(&self, _: Uuid, _: Option<String>) -> Result<(), String> {
        unimplemented!("set_subscriber_spc is not part of these tests")
    }

    async fn set_subscriber_firstchp_override(
        &self,
        _: Uuid,
        _: Option<u16>,
    ) -> Result<(), String> {
        unimplemented!("set_subscriber_firstchp_override is not part of these tests")
    }

    async fn save_otasp_session(&self, _: &OtaspSessionRow) -> Result<(), String> {
        unimplemented!("save_otasp_session is not part of these tests")
    }

    async fn list_otasp_sessions(
        &self,
        _: OtaspSessionFilter,
        _: u32,
        _: u32,
    ) -> Result<(Vec<OtaspSessionRow>, u32), String> {
        unimplemented!("list_otasp_sessions is not part of these tests")
    }

    async fn get_otasp_session(&self, _: Uuid) -> Result<Option<OtaspSessionRow>, String> {
        unimplemented!("get_otasp_session is not part of these tests")
    }
}

#[derive(Default)]
struct Store {
    submissions: Mutex<Vec<SmsSubmission>>,
    attempts: Mutex<Vec<SmsDeliveryAttempt>>,
}

impl Store {
    fn accept(&self, from: &str, to: &str) -> SmsSubmission {
        let submission = SmsSubmission {
            sms_id: Uuid::new_v4(),
            originating_number: from.to_string(),
            destination_number: Some(to.to_string()),
            destination_esn: None,
            destination_imsi: None,
            originating_subscriber_id: None,
            destination_subscriber_id: None,
            text: "hello".into(),
            state: SmsState::Accepted,
            failure_reason: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            teleservice_id: None,
            raw_user_data: None,
            from_interconnect: false,
        };
        self.submissions.lock().unwrap().push(submission.clone());
        submission
    }

    fn accept_from_interconnect(&self, from: &str, to: &str) -> SmsSubmission {
        let submission = self.accept(from, to);
        let mut rows = self.submissions.lock().unwrap();
        let row = rows
            .iter_mut()
            .find(|s| s.sms_id == submission.sms_id)
            .unwrap();
        row.from_interconnect = true;
        row.clone()
    }

    fn state_of(&self, sms_id: Uuid) -> SmsState {
        self.submissions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.sms_id == sms_id)
            .map(|s| s.state.clone())
            .expect("a submission")
    }
}

#[async_trait]
impl SmscRepository for Store {
    async fn claim_delivery(
        &self,
        sms_id: Uuid,
        target_subscriber_id: Option<Uuid>,
    ) -> Result<Option<SmsDeliveryAttempt>, String> {
        let mut submissions = self.submissions.lock().unwrap();
        let submission = submissions
            .iter_mut()
            .find(|s| s.sms_id == sms_id)
            .ok_or("unknown submission")?;
        if submission.state != SmsState::Accepted {
            return Ok(None);
        }
        submission.state = SmsState::Paging;
        let mut attempts = self.attempts.lock().unwrap();
        let now = Utc::now() - chrono::Duration::seconds(60);
        let attempt = SmsDeliveryAttempt {
            sms_delivery_attempt_id: Uuid::new_v4(),
            sms_id,
            attempt_number: attempts.iter().filter(|a| a.sms_id == sms_id).count() as u32 + 1,
            state: DeliveryAttemptState::Paging,
            target_subscriber_id,
            failure_reason: None,
            requested_at: now,
            completed_at: None,
            created_at: now,
            updated_at: now,
        };
        attempts.push(attempt.clone());
        Ok(Some(attempt))
    }

    async fn finish_delivery(
        &self,
        sms_id: Uuid,
        attempt_id: Uuid,
        outcome: DeliveryCompletion,
    ) -> Result<bool, String> {
        let mut submissions = self.submissions.lock().unwrap();
        let submission = submissions
            .iter_mut()
            .find(|s| s.sms_id == sms_id)
            .ok_or("unknown submission")?;
        let mut attempts = self.attempts.lock().unwrap();
        let Some(attempt) = attempts
            .iter_mut()
            .filter(|a| a.sms_id == sms_id)
            .max_by_key(|a| a.attempt_number)
        else {
            return Ok(false);
        };
        if submission.state != SmsState::Paging
            || attempt.sms_delivery_attempt_id != attempt_id
            || attempt.state != DeliveryAttemptState::Paging
        {
            return Ok(false);
        }
        let states = outcome.states();
        submission.state = states.submission;
        submission.failure_reason = states.reason.clone();
        submission.updated_at = Utc::now();
        attempt.state = states.attempt;
        attempt.failure_reason = states.reason;
        attempt.updated_at = Utc::now() - chrono::Duration::seconds(60);
        attempt.completed_at = Some(attempt.updated_at);
        Ok(true)
    }

    async fn create_submission(
        &self,
        _: &str,
        _: SmsDestination,
        _: &str,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: CreateSubmissionOptions<'_>,
    ) -> Result<SmsSubmission, String> {
        unimplemented!("not part of these tests")
    }

    async fn create_or_get_recent_mo_submission(
        &self,
        originating_number: &str,
        destination_number: &str,
        _: &str,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: &MoSmsFingerprint,
    ) -> Result<(SmsSubmission, bool), String> {
        Ok((self.accept(originating_number, destination_number), true))
    }

    async fn update_submission_state(
        &self,
        sms_id: Uuid,
        state: SmsState,
        failure_reason: Option<String>,
    ) -> Result<SmsSubmission, String> {
        let mut rows = self.submissions.lock().unwrap();
        let row = rows
            .iter_mut()
            .find(|s| s.sms_id == sms_id)
            .ok_or("no such submission")?;
        row.state = state;
        row.failure_reason = failure_reason;
        row.updated_at = Utc::now();
        Ok(row.clone())
    }

    async fn get_submission(&self, sms_id: Uuid) -> Result<Option<SmsSubmission>, String> {
        Ok(self
            .submissions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.sms_id == sms_id)
            .cloned())
    }

    async fn list_submissions(
        &self,
        limit: u32,
        offset: u32,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
        state: Option<&str>,
        _: Option<&str>,
    ) -> Result<(Vec<SmsSubmission>, u32), String> {
        let rows = self.submissions.lock().unwrap();
        let matching: Vec<SmsSubmission> = rows
            .iter()
            .filter(|s| state.is_none_or(|want| s.state.as_str() == want))
            .cloned()
            .collect();
        let total = matching.len() as u32;
        Ok((
            matching
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect(),
            total,
        ))
    }

    async fn create_delivery_attempt(
        &self,
        sms_id: Uuid,
        target_subscriber_id: Option<Uuid>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let mut rows = self.attempts.lock().unwrap();
        let attempt = SmsDeliveryAttempt {
            sms_delivery_attempt_id: Uuid::new_v4(),
            sms_id,
            attempt_number: rows.iter().filter(|a| a.sms_id == sms_id).count() as u32 + 1,
            state: DeliveryAttemptState::Queued,
            target_subscriber_id,
            failure_reason: None,
            requested_at: Utc::now(),
            completed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        rows.push(attempt.clone());
        Ok(attempt)
    }

    async fn update_delivery_attempt_state(
        &self,
        attempt_id: Uuid,
        state: DeliveryAttemptState,
        failure_reason: Option<String>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let mut rows = self.attempts.lock().unwrap();
        let row = rows
            .iter_mut()
            .find(|a| a.sms_delivery_attempt_id == attempt_id)
            .ok_or("no such attempt")?;
        row.state = state;
        row.failure_reason = failure_reason;
        row.updated_at = Utc::now() - chrono::Duration::seconds(60);
        row.completed_at = Some(row.updated_at);
        Ok(row.clone())
    }

    async fn get_delivery_attempts(&self, sms_id: Uuid) -> Result<Vec<SmsDeliveryAttempt>, String> {
        Ok(self
            .attempts
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.sms_id == sms_id)
            .cloned()
            .collect())
    }

    async fn update_destination_subscriber(&self, _: Uuid, _: Uuid) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct Hub {
    available: bool,
    taken: Mutex<Vec<InterconnectSms>>,
    refuse: Option<InterconnectError>,
}

#[async_trait]
impl InterconnectEgress for Hub {
    fn is_available(&self) -> bool {
        self.available
    }

    async fn submit(&self, message: InterconnectSms) -> Result<(), InterconnectError> {
        if let Some(refusal) = self.refuse.clone() {
            return Err(refusal);
        }
        self.taken.lock().unwrap().push(message);
        Ok(())
    }
}

fn dispatcher(store: Arc<Store>, hlr: Arc<OneSubscriber>, hub: Arc<DeliveryHub>) -> Dispatcher {
    Dispatcher::new(store, Router::new(hlr), hub)
}

#[tokio::test]
async fn a_message_for_a_registered_subscriber_goes_on_the_air() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let hub = DeliveryHub::new();
    let mut deliveries = hub.attach().await;
    let dispatcher = dispatcher(store.clone(), hlr, hub);

    let submission = store.accept("4155550118", "7025550100");
    dispatcher.dispatch(&submission).await.unwrap();

    let command = deliveries.recv().await.expect("a delivery");
    assert_eq!(command.sms_id, submission.sms_id.to_string());
    assert_eq!(store.state_of(submission.sms_id), SmsState::Paging);
}

#[tokio::test]
async fn a_message_for_an_offline_subscriber_waits_rather_than_failing() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::offline("7025550100"));
    let hub = DeliveryHub::new();
    let _deliveries = hub.attach().await;
    let dispatcher = dispatcher(store.clone(), hlr, hub);

    let submission = store.accept("4155550118", "7025550100");
    assert_eq!(
        dispatcher.dispatch(&submission).await,
        Err(Deferral::Offline)
    );
    assert_eq!(store.state_of(submission.sms_id), SmsState::Accepted);
}

#[tokio::test]
async fn a_number_nobody_holds_fails_with_a_cause_when_there_is_no_interconnect() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let hub = DeliveryHub::new();
    let _deliveries = hub.attach().await;
    let dispatcher = dispatcher(store.clone(), hlr, hub);

    let submission = store.accept("7025550100", "9995550000");
    let Err(Deferral::Undeliverable(reason)) = dispatcher.dispatch(&submission).await else {
        panic!("a number nobody holds must not be left waiting");
    };
    assert!(reason.contains("no interconnect is available"), "{reason}");
    assert_eq!(store.state_of(submission.sms_id), SmsState::Failed);
}

#[tokio::test]
async fn a_number_nobody_holds_goes_to_the_interconnect_when_one_is_up() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let interconnect = Arc::new(Hub {
        available: true,
        ..Hub::default()
    });
    let dispatcher =
        dispatcher(store.clone(), hlr, DeliveryHub::new()).with_interconnect(interconnect.clone());

    let submission = store.accept("7025550100", "4155550118");
    dispatcher.dispatch(&submission).await.unwrap();

    let taken = interconnect.taken.lock().unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].destination, "4155550118");
    assert_eq!(store.state_of(submission.sms_id), SmsState::Forwarded);
}

#[tokio::test]
async fn a_hub_that_will_not_take_it_leaves_the_message_here() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let interconnect = Arc::new(Hub {
        available: true,
        refuse: Some(InterconnectError::NotConnected),
        ..Hub::default()
    });
    let dispatcher =
        dispatcher(store.clone(), hlr, DeliveryHub::new()).with_interconnect(interconnect);

    let submission = store.accept("7025550100", "4155550118");
    assert!(matches!(
        dispatcher.dispatch(&submission).await,
        Err(Deferral::Interconnect(_))
    ));
    assert_eq!(store.state_of(submission.sms_id), SmsState::Accepted);
}

#[tokio::test]
async fn nothing_goes_out_while_no_msc_is_connected() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let hub = DeliveryHub::new();
    let dispatcher = dispatcher(store.clone(), hlr, hub.clone());

    let submission = store.accept("4155550118", "7025550100");
    assert_eq!(dispatcher.dispatch(&submission).await, Err(Deferral::NoMsc));
    assert_eq!(store.state_of(submission.sms_id), SmsState::Accepted);
    assert_eq!(hub.push(Default::default()).await, PushResult::NoMsc);
}

#[tokio::test]
async fn a_message_for_a_switched_off_handset_arrives_when_it_registers() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::offline("7025550100"));
    let hub = DeliveryHub::new();
    let mut deliveries = hub.attach().await;
    let dispatcher = Arc::new(dispatcher(store.clone(), hlr.clone(), hub));

    let submission = store.accept("4155550118", "7025550100");
    assert_eq!(
        dispatcher.dispatch(&submission).await,
        Err(Deferral::Offline)
    );
    assert!(
        store.attempts.lock().unwrap().is_empty(),
        "an unreachable subscriber should not leave an attempt behind"
    );

    hlr.registers();
    Scheduler::new(store.clone(), dispatcher, SweepConfig::default())
        .retry_sweep()
        .await;

    let command = deliveries.recv().await.expect("a delivery");
    assert_eq!(command.sms_id, submission.sms_id.to_string());
    assert_eq!(store.state_of(submission.sms_id), SmsState::Paging);
}

#[tokio::test]
async fn the_sweep_frees_a_message_left_paging_by_an_msc_that_went_away() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let hub = DeliveryHub::new();
    let deliveries = hub.attach().await;
    let dispatcher = Arc::new(dispatcher(store.clone(), hlr, hub));

    let submission = store.accept("4155550118", "7025550100");
    dispatcher.dispatch(&submission).await.unwrap();
    assert_eq!(store.state_of(submission.sms_id), SmsState::Paging);
    drop(deliveries);

    Scheduler::new(
        store.clone(),
        dispatcher,
        SweepConfig {
            stale_after: Duration::from_secs(1),
            ..SweepConfig::default()
        },
    )
    .recover_stuck()
    .await;
    assert_eq!(store.state_of(submission.sms_id), SmsState::Accepted);
}

#[tokio::test]
async fn an_outcome_moves_the_submission_on() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let dispatcher = dispatcher(store.clone(), hlr, DeliveryHub::new());

    for (result, expected) in [
        (AttemptResult::Delivered, SmsState::Delivered),
        (AttemptResult::Failed(None), SmsState::Failed),
        (AttemptResult::NotReachable(None), SmsState::Accepted),
    ] {
        let submission = store.accept("4155550118", "7025550100");
        let attempt = store
            .claim_delivery(submission.sms_id, None)
            .await
            .unwrap()
            .unwrap();
        dispatcher
            .record_outcome(submission.sms_id, attempt.sms_delivery_attempt_id, result)
            .await;
        assert_eq!(store.state_of(submission.sms_id), expected);
    }
}

#[tokio::test]
async fn a_message_is_marked_paging_before_it_is_handed_over() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let hub = DeliveryHub::new();
    let mut deliveries = hub.attach().await;
    let dispatcher = dispatcher(store.clone(), hlr, hub);

    let submission = store.accept("4155550118", "7025550100");
    dispatcher.dispatch(&submission).await.unwrap();

    // Record the attempt before sending. An immediate outcome must not be overwritten.
    let _ = deliveries.recv().await.expect("a delivery");
    assert_eq!(store.state_of(submission.sms_id), SmsState::Paging);
}

#[tokio::test]
async fn a_message_that_could_not_be_handed_over_goes_back_to_accepted() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let dispatcher = dispatcher(store.clone(), hlr, DeliveryHub::new());

    let submission = store.accept("4155550118", "7025550100");
    assert_eq!(dispatcher.dispatch(&submission).await, Err(Deferral::NoMsc));
    assert_eq!(
        store.state_of(submission.sms_id),
        SmsState::Accepted,
        "a message nothing carried must not be left paging"
    );
}

#[tokio::test]
async fn a_message_from_another_operator_is_never_sent_back_to_one() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let interconnect = Arc::new(Hub {
        available: true,
        ..Hub::default()
    });
    let dispatcher =
        dispatcher(store.clone(), hlr, DeliveryHub::new()).with_interconnect(interconnect.clone());

    let submission = store.accept_from_interconnect("4155550118", "9995550000");
    let Err(Deferral::Undeliverable(reason)) = dispatcher.dispatch(&submission).await else {
        panic!("an inbound message with nowhere to go must fail");
    };
    assert!(reason.contains("arrived from another operator"), "{reason}");

    assert!(
        interconnect.taken.lock().unwrap().is_empty(),
        "an inbound message must not go back out to the hub"
    );
    assert_eq!(store.state_of(submission.sms_id), SmsState::Failed);
}

#[tokio::test]
async fn a_message_from_another_operator_still_reaches_a_local_subscriber() {
    let store = Arc::new(Store::default());
    let hlr = Arc::new(OneSubscriber::registered("7025550100"));
    let hub = DeliveryHub::new();
    let mut deliveries = hub.attach().await;
    let dispatcher = dispatcher(store.clone(), hlr, hub).with_interconnect(Arc::new(Hub {
        available: true,
        ..Hub::default()
    }));

    let submission = store.accept_from_interconnect("4155550118", "7025550100");
    dispatcher.dispatch(&submission).await.unwrap();
    assert_eq!(
        deliveries.recv().await.expect("a delivery").sms_id,
        submission.sms_id.to_string()
    );
}

#[tokio::test]
async fn a_configured_but_disconnected_interconnect_keeps_messages_queued() {
    let store = Arc::new(Store::default());
    let dispatcher = dispatcher(
        store.clone(),
        Arc::new(OneSubscriber::registered("1000")),
        DeliveryHub::new(),
    )
    .with_interconnect(Arc::new(Hub::default()));
    let submission = store.accept("1000", "1001");
    assert!(matches!(
        dispatcher.dispatch(&submission).await,
        Err(Deferral::Interconnect(_))
    ));
    assert_eq!(store.state_of(submission.sms_id), SmsState::Accepted);
}

#[tokio::test]
async fn hlr_failure_is_not_an_authoritative_unknown_destination() {
    for inbound in [false, true] {
        let store = Arc::new(Store::default());
        let interconnect = Arc::new(Hub {
            available: true,
            ..Hub::default()
        });
        let dispatcher = dispatcher(
            store.clone(),
            Arc::new(OneSubscriber::registered("unavailable")),
            DeliveryHub::new(),
        )
        .with_interconnect(interconnect.clone());
        let submission = if inbound {
            store.accept_from_interconnect("1000", "unavailable")
        } else {
            store.accept("1000", "unavailable")
        };
        assert!(matches!(
            dispatcher.dispatch(&submission).await,
            Err(Deferral::Storage(_))
        ));
        assert_eq!(store.state_of(submission.sms_id), SmsState::Accepted);
        assert!(interconnect.taken.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn an_unreachable_attempt_is_completed_before_a_retry_is_dispatched() {
    let store = Arc::new(Store::default());
    let hub = DeliveryHub::new();
    let mut commands = hub.attach().await;
    let dispatcher = Arc::new(dispatcher(
        store.clone(),
        Arc::new(OneSubscriber::registered("1001")),
        hub,
    ));
    let submission = store.accept("1000", "1001");
    dispatcher.dispatch(&submission).await.unwrap();
    let first = commands.recv().await.unwrap();
    let first_id = Uuid::parse_str(&first.attempt_id).unwrap();
    dispatcher
        .record_outcome(
            submission.sms_id,
            first_id,
            AttemptResult::NotReachable(None),
        )
        .await;
    assert_eq!(
        store
            .get_delivery_attempts(submission.sms_id)
            .await
            .unwrap()[0]
            .state,
        DeliveryAttemptState::Failed
    );
    Scheduler::new(store.clone(), dispatcher.clone(), SweepConfig::default())
        .retry_sweep()
        .await;
    let next = commands.try_recv().expect("a retry command");
    assert_ne!(next.attempt_id, first.attempt_id);
    dispatcher
        .record_outcome(submission.sms_id, first_id, AttemptResult::Delivered)
        .await;
    assert_eq!(store.state_of(submission.sms_id), SmsState::Paging);
    dispatcher
        .record_outcome(
            submission.sms_id,
            Uuid::parse_str(&next.attempt_id).unwrap(),
            AttemptResult::Delivered,
        )
        .await;
    assert_eq!(store.state_of(submission.sms_id), SmsState::Delivered);
    dispatcher
        .record_outcome(
            submission.sms_id,
            first_id,
            AttemptResult::NotReachable(None),
        )
        .await;
    assert_eq!(store.state_of(submission.sms_id), SmsState::Delivered);
}

struct SlowHub {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: std::sync::atomic::AtomicUsize,
}

impl SlowHub {
    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl InterconnectEgress for SlowHub {
    fn is_available(&self) -> bool {
        true
    }
    async fn submit(&self, _: InterconnectSms) -> Result<(), InterconnectError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn an_overlapping_retry_sweep_cannot_forward_a_claimed_message_again() {
    let store = Arc::new(Store::default());
    let interconnect = Arc::new(SlowHub::new());
    let dispatcher = Arc::new(
        dispatcher(
            store.clone(),
            Arc::new(OneSubscriber::registered("1000")),
            DeliveryHub::new(),
        )
        .with_interconnect(interconnect.clone()),
    );
    let submission = store.accept("1000", "1001");
    let sending = dispatcher.clone();
    let task = tokio::spawn(async move { sending.dispatch(&submission).await });
    tokio::time::timeout(Duration::from_secs(1), interconnect.entered.notified())
        .await
        .unwrap();
    Scheduler::new(store.clone(), dispatcher, SweepConfig::default())
        .retry_sweep()
        .await;
    interconnect.release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(
        interconnect.calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn a_permanent_interconnect_rejection_is_terminal() {
    let store = Arc::new(Store::default());
    let dispatcher = Arc::new(
        dispatcher(
            store.clone(),
            Arc::new(OneSubscriber::registered("1000")),
            DeliveryHub::new(),
        )
        .with_interconnect(Arc::new(Hub {
            available: true,
            refuse: Some(InterconnectError::Rejected("invalid destination".into())),
            ..Hub::default()
        })),
    );
    let submission = store.accept("1000", "1001");
    assert!(matches!(
        dispatcher.dispatch(&submission).await,
        Err(Deferral::Undeliverable(_))
    ));
    Scheduler::new(store.clone(), dispatcher, SweepConfig::default())
        .retry_sweep()
        .await;
    assert_eq!(store.state_of(submission.sms_id), SmsState::Failed);
    assert_eq!(
        store
            .get_delivery_attempts(submission.sms_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn durable_acceptance_does_not_wait_for_the_smpp_response() {
    use cdma_smsc::proto::{
        MoSmsFingerprint as WireFingerprint, SubmitMoRequest, smsc_service_server::SmscService,
    };
    let store = Arc::new(Store::default());
    let interconnect = Arc::new(SlowHub::new());
    let dispatcher = Arc::new(
        dispatcher(
            store.clone(),
            Arc::new(OneSubscriber::registered("1000")),
            DeliveryHub::new(),
        )
        .with_interconnect(interconnect.clone()),
    );
    let service = cdma_smsc::service::SmscServiceImpl::with_dispatcher(store.clone(), dispatcher);
    let request = SubmitMoRequest {
        originating_number: "1000".into(),
        destination_number: "1001".into(),
        text: "hello".into(),
        fingerprint: Some(WireFingerprint {
            teleservice_id: u32::from(cdma_common::sms::TELESERVICE_WMT),
            message_type: u32::from(SUBMIT_MESSAGE_TYPE),
            message_id: 1,
        }),
        ..Default::default()
    };
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        service.submit_mo(tonic::Request::new(request)),
    )
    .await
    .unwrap()
    .unwrap()
    .into_inner();
    assert!(response.accepted);
    assert!(
        store
            .get_submission(Uuid::parse_str(&response.sms_id).unwrap())
            .await
            .unwrap()
            .is_some()
    );
    tokio::time::timeout(Duration::from_secs(1), interconnect.entered.notified())
        .await
        .unwrap();
    interconnect.release.notify_one();
}
