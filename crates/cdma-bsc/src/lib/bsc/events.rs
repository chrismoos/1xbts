//! BSC-domain event publication.

use cdma_common::events::AccessChannelEvent;
use tokio::sync::{broadcast, watch};

use super::{MobileInfo, PagingEvent, TrafficEvent};

#[derive(Clone, Default)]
pub(crate) struct EventService {
    mobiles_tx: Option<watch::Sender<Vec<MobileInfo>>>,
    paging_tx: Option<broadcast::Sender<PagingEvent>>,
    traffic_tx: Option<broadcast::Sender<TrafficEvent>>,
    access_tx: Option<broadcast::Sender<AccessChannelEvent>>,
}

impl EventService {
    pub(crate) fn new(
        mobiles_tx: Option<watch::Sender<Vec<MobileInfo>>>,
        paging_tx: Option<broadcast::Sender<PagingEvent>>,
        traffic_tx: Option<broadcast::Sender<TrafficEvent>>,
        access_tx: Option<broadcast::Sender<AccessChannelEvent>>,
    ) -> Self {
        Self {
            mobiles_tx,
            paging_tx,
            traffic_tx,
            access_tx,
        }
    }

    pub(crate) fn publish_mobile_snapshot(&self, snapshot: Vec<MobileInfo>) {
        if let Some(tx) = self.mobiles_tx.as_ref() {
            let _ = tx.send(snapshot);
        }
    }

    pub(crate) fn publish_paging_event(&self, event: PagingEvent) {
        if let Some(tx) = self.paging_tx.as_ref() {
            let _ = tx.send(event);
        }
    }

    pub(crate) fn publish_traffic_event(&self, event: TrafficEvent) {
        if let Some(tx) = self.traffic_tx.as_ref() {
            let _ = tx.send(event);
        }
    }

    pub(crate) fn publish_access_event(&self, event: AccessChannelEvent) {
        if let Some(tx) = self.access_tx.as_ref() {
            let _ = tx.send(event);
        }
    }

    #[allow(dead_code)]
    pub(crate) fn subscribe_mobiles(&self) -> Option<watch::Receiver<Vec<MobileInfo>>> {
        self.mobiles_tx.as_ref().map(watch::Sender::subscribe)
    }

    #[allow(dead_code)]
    pub(crate) fn subscribe_paging(&self) -> Option<broadcast::Receiver<PagingEvent>> {
        self.paging_tx.as_ref().map(broadcast::Sender::subscribe)
    }

    #[allow(dead_code)]
    pub(crate) fn subscribe_traffic(&self) -> Option<broadcast::Receiver<TrafficEvent>> {
        self.traffic_tx.as_ref().map(broadcast::Sender::subscribe)
    }

    #[allow(dead_code)]
    pub(crate) fn subscribe_access(&self) -> Option<broadcast::Receiver<AccessChannelEvent>> {
        self.access_tx.as_ref().map(broadcast::Sender::subscribe)
    }
}
