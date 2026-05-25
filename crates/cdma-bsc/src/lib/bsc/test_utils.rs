use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use tokio::sync::mpsc;

use crate::a1_edge::{EncodedA1Message, MscClient};

use super::{Bsc, SmsRequest};

/// Loopback MSC client for integration tests. Accepts `CompleteLayer3Information`
/// from the BSC and synthesises an `AssignmentRequest` reply with the same
/// service option, so origination flows that require an MSC peer can run.
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
            let service_option = message
                .decode()
                .ok()
                .and_then(|decoded| {
                    cdma_ios::CompleteLayer3InformationMessage::decode(&decoded.payload).ok()
                })
                .and_then(|cli3| cli3.layer3_information.decode_cm_service_request().ok())
                .and_then(|request| request.service_option)
                .unwrap_or(cdma_ios::ServiceOption::EVRC_A);
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

    pub fn send_paging_frame_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_system_parameters_message())
    }

    pub fn send_system_parameters_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_system_parameters_message())
    }

    pub fn send_access_parameters_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_access_parameters_message())
    }

    pub fn send_neighbor_list_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_neighbor_list_message())
    }

    pub fn send_cdma_channel_list_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_cdma_channel_list_message())
    }

    pub fn send_general_page_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_general_page_message())
    }

    pub fn send_extended_system_parameters_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_extended_system_parameters_message())
    }

    pub fn send_order_message_once(&mut self) -> Result<(), Error> {
        self.send_paging_message(self.build_order_message())
    }

    pub fn send_next_default_paging_message_once(&mut self) -> Result<(), Error> {
        self.send_next_default_paging_message()
    }
}
