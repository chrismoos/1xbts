use cdma_common::consts::SERVICE_OPTION_BASIC_VOICE;
use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use tokio::sync::mpsc;

use crate::a1_edge::{EncodedA1Message, MscClient};
use cdma_hlr::model::{
    RegistrationBinding, ResolvedSubscriber, Subscriber, SubscriberIdentity, SubscriberStatus,
};
use cdma_hlr::repository::HlrRepository;
use uuid::Uuid;

use super::{Bsc, SmsRequest};

pub struct AutoAssignmentMscClient {
    inbound_tx: mpsc::Sender<EncodedA1Message>,
    inbound_rx: tokio::sync::Mutex<mpsc::Receiver<EncodedA1Message>>,
}

impl AutoAssignmentMscClient {
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(32);
        Self {
            inbound_tx,
            inbound_rx: tokio::sync::Mutex::new(inbound_rx),
        }
    }
}

impl Default for AutoAssignmentMscClient {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl MscClient for AutoAssignmentMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), cdma_ios::A1TransportError> {
        if message.message_type() == cdma_ios::MessageType::CompleteLayer3Information {
            let call_id = message.call_id();
            let requested_service_option = message
                .decode()
                .ok()
                .and_then(|decoded| {
                    cdma_ios::CompleteLayer3InformationMessage::decode(&decoded.payload).ok()
                })
                .and_then(|cli3| cli3.layer3_information.decode_cm_service_request().ok())
                .and_then(|request| request.service_option)
                .unwrap_or(cdma_ios::ServiceOption(SERVICE_OPTION_BASIC_VOICE));
            let service_option = requested_service_option;
            let assignment = cdma_ios::AssignmentRequestMessage {
                channel_type: cdma_ios::ChannelType {
                    speech_or_data_indicator: 0x01,
                    channel_rate_and_type: 0x08,
                    coding: 0x05,
                },
                circuit_identity_code: cdma_ios::CircuitIdentityCode {
                    pcm_multiplexer: 0,
                    timeslot: 1,
                },
                encryption_information: None,
                service_option: Some(service_option),
                signals: Vec::new(),
                ms_information_records: None,
                priority: None,
                paca_timestamp: None,
                quality_of_service_parameters: None,
                a2p_bearer_session_params: None,
                a2p_bearer_format_params: None,
            };
            let encoded = EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(
                    cdma_ios::MessageType::AssignmentRequest,
                    assignment
                        .encode()
                        .map_err(cdma_ios::A1TransportError::Codec)?,
                ),
                call_id,
            );
            self.inbound_tx
                .send(encoded)
                .await
                .map_err(|_| cdma_ios::A1TransportError::Closed)?;
        }
        Ok(())
    }

    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, cdma_ios::A1TransportError> {
        Ok(self.inbound_rx.lock().await.recv().await)
    }
}

impl Bsc {
    pub async fn inject_access_event(&mut self, event: AccessChannelEvent) {
        let event = self.enrich_uplink_event(event);
        if !event.is_traffic_phy_status {
            self.events.publish_access_event(event.clone());
        }
        self.handle_access_event(event).await;
        if let Ok(Ok(Some(message))) = tokio::time::timeout(
            std::time::Duration::from_millis(1),
            self.config.msc_client.poll_a1(),
        )
        .await
        {
            self.handle_incoming_a1_message(message).await;
        }
    }

    pub fn inject_sms_request(&mut self, sms_req: SmsRequest) {
        self.handle_sms_request(sms_req);
    }

    pub fn trigger_page_retry(&mut self) -> bool {
        if self.paging.has_pending_sms_page() {
            self.handle_page_retry();
            true
        } else {
            false
        }
    }

    pub fn has_pending_page(&self) -> bool {
        self.paging.has_pending_page()
    }

    pub fn send_sync_frame_once(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

pub struct RecordingMscClient {
    inner: AutoAssignmentMscClient,
    adds_deliver: std::sync::Mutex<Vec<AddsDeliverRecord>>,
}

#[derive(Debug, Clone)]
pub struct AddsDeliverRecord {
    pub burst_type: u8,
    pub data: Vec<u8>,
}

impl RecordingMscClient {
    pub fn new() -> Self {
        Self {
            inner: AutoAssignmentMscClient::new(),
            adds_deliver: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn adds_deliver(&self) -> Vec<AddsDeliverRecord> {
        self.adds_deliver.lock().unwrap().clone()
    }
}

impl Default for RecordingMscClient {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl MscClient for RecordingMscClient {
    async fn send_a1(&self, message: EncodedA1Message) -> Result<(), cdma_ios::A1TransportError> {
        if message.message_type() == cdma_ios::MessageType::AddsDeliver {
            if let Ok(decoded) = message.decode() {
                if let Ok(adds) = cdma_ios::AddsDeliverMessage::decode(&decoded.payload) {
                    self.adds_deliver.lock().unwrap().push(AddsDeliverRecord {
                        burst_type: adds.adds_user_part.burst_type,
                        data: adds.adds_user_part.data,
                    });
                }
            }
        }
        self.inner.send_a1(message).await
    }

    async fn poll_a1(&self) -> Result<Option<EncodedA1Message>, cdma_ios::A1TransportError> {
        self.inner.poll_a1().await
    }
}

pub struct LoopbackHlrRepository {
    subscriber: Subscriber,
}

impl LoopbackHlrRepository {
    pub fn new(phone_number: &str) -> Self {
        let now = chrono::Utc::now();
        Self {
            subscriber: Subscriber {
                subscriber_id: Uuid::new_v4(),
                phone_number: phone_number.to_string(),
                display_name: "Loopback Subscriber".to_string(),
                status: SubscriberStatus::Active,
                created_at: now,
                updated_at: now,
                number_type: cdma_hlr::model::NumberType::NetworkSpecific,
                number_plan: cdma_hlr::model::NumberPlan::IsdnE164,
                has_ringtone: false,
                ringtone_duration_ms: None,
                prl_override_id: None,
                service_programming_code: None,
                firstchp_override: None,
            },
        }
    }

    fn resolved(&self) -> ResolvedSubscriber {
        ResolvedSubscriber {
            subscriber: self.subscriber.clone(),
            identities: Vec::new(),
            primary_identity: None,
            binding: None,
        }
    }
}

#[tonic::async_trait]
impl HlrRepository for LoopbackHlrRepository {
    async fn upsert_subscriber(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: cdma_hlr::model::NumberType,
        _: cdma_hlr::model::NumberPlan,
    ) -> Result<Subscriber, String> {
        Ok(self.subscriber.clone())
    }
    async fn get_subscriber_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        Ok((self.subscriber.phone_number == phone_number).then(|| self.resolved()))
    }
    async fn get_subscriber_by_id(&self, id: Uuid) -> Result<Option<ResolvedSubscriber>, String> {
        Ok((self.subscriber.subscriber_id == id).then(|| self.resolved()))
    }
    async fn update_subscriber(
        &self,
        _: Uuid,
        _: &str,
        _: &str,
        _: &str,
        _: cdma_hlr::model::NumberType,
        _: cdma_hlr::model::NumberPlan,
    ) -> Result<Option<Subscriber>, String> {
        Ok(Some(self.subscriber.clone()))
    }
    async fn list_subscribers(&self, _: u32, _: u32) -> Result<(Vec<Subscriber>, u32), String> {
        Ok((vec![self.subscriber.clone()], 1))
    }
    async fn delete_subscriber(&self, _: Uuid) -> Result<bool, String> {
        Ok(false)
    }
    async fn upsert_identity(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        Err("not provisioned".to_string())
    }
    async fn replace_primary_identity(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        Err("not provisioned".to_string())
    }
    async fn get_identities_for_subscriber(
        &self,
        _: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String> {
        Ok(Vec::new())
    }
    async fn resolve_by_identity(
        &self,
        _: &cdma_hlr::model::MobileIdentityKey,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        Ok(Some(self.resolved()))
    }
    async fn resolve_by_hardware_identity(
        &self,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        Ok(Some(self.resolved()))
    }
    async fn upsert_mobile_seen(
        &self,
        _: &cdma_hlr::model::MobileIdentityKey,
        _: Option<u8>,
    ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
        Ok(cdma_hlr::MobileSeenUpsert {
            is_new: true,
            previous_last_seen_at: None,
        })
    }
    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        Ok(binding)
    }
    async fn get_registration_binding(
        &self,
        _: Uuid,
    ) -> Result<Option<RegistrationBinding>, String> {
        Ok(None)
    }
    async fn set_ringtone(
        &self,
        _: Uuid,
        _: Vec<u8>,
        _: &str,
    ) -> Result<cdma_hlr::model::SetRingtoneOutcome, String> {
        Ok(cdma_hlr::model::SetRingtoneOutcome {
            codecs: vec![],
            duration_ms: 0,
        })
    }
    async fn clear_ringtone(&self, _: Uuid) -> Result<(), String> {
        Ok(())
    }
    async fn get_ringtone_codec(
        &self,
        _: Uuid,
        _: &str,
    ) -> Result<Option<cdma_hlr::model::SubscriberRingtoneCodecBlob>, String> {
        Ok(None)
    }
    async fn list_prls(
        &self,
        _: u32,
        _: u32,
        _: cdma_hlr::model::PrlListFilter,
    ) -> Result<(Vec<cdma_hlr::model::Prl>, u32), String> {
        Ok((vec![], 0))
    }
    async fn get_prl(&self, _: Uuid) -> Result<Option<cdma_hlr::model::Prl>, String> {
        Ok(None)
    }
    async fn get_default_prl(&self) -> Result<Option<cdma_hlr::model::Prl>, String> {
        Ok(None)
    }
    async fn create_prl(
        &self,
        _: &str,
        _: &[u8],
        _: i32,
        _: i16,
        _: &str,
    ) -> Result<cdma_hlr::model::Prl, String> {
        Err("not provisioned".to_string())
    }
    async fn update_prl(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<&[u8]>,
        _: Option<(i32, i16)>,
        _: Option<&str>,
    ) -> Result<cdma_hlr::model::Prl, String> {
        Err("not provisioned".to_string())
    }
    async fn soft_delete_prl(
        &self,
        _: Uuid,
    ) -> Result<Result<(), cdma_hlr::model::PrlDeleteBlocked>, String> {
        Ok(Ok(()))
    }
    async fn set_default_prl(&self, _: Uuid) -> Result<(), String> {
        Ok(())
    }
    async fn set_subscriber_prl_override(&self, _: Uuid, _: Option<Uuid>) -> Result<(), String> {
        Ok(())
    }
    async fn set_subscriber_spc(&self, _: Uuid, _: Option<String>) -> Result<(), String> {
        Ok(())
    }
    async fn set_subscriber_firstchp_override(
        &self,
        _: Uuid,
        _: Option<u16>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn save_otasp_session(&self, _: &cdma_hlr::model::OtaspSessionRow) -> Result<(), String> {
        Ok(())
    }
    async fn list_otasp_sessions(
        &self,
        _: cdma_hlr::model::OtaspSessionFilter,
        _: u32,
        _: u32,
    ) -> Result<(Vec<cdma_hlr::model::OtaspSessionRow>, u32), String> {
        Ok((Vec::new(), 0))
    }
    async fn get_otasp_session(
        &self,
        _: Uuid,
    ) -> Result<Option<cdma_hlr::model::OtaspSessionRow>, String> {
        Ok(None)
    }
}
