use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;

use super::{Bsc, SmsRequest};

impl Bsc {
    pub async fn inject_access_event(&mut self, event: AccessChannelEvent) {
        let event = self.enrich_uplink_event(event);
        if !event.is_traffic_phy_status {
            self.events.publish_access_event(event.clone());
        }
        self.handle_access_event(event).await;
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
