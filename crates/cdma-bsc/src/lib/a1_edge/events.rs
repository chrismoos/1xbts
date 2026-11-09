use std::num::NonZeroUsize;

use lru::LruCache;
use parking_lot::Mutex;

use cdma_events::EventPublisher;
use cdma_events::a1::{A1Context, a1_event_from_wire};
use cdma_events::proto::{
    A1Direction, A1MobileIdentity, EventSource, MobileIdentity, network_event,
};
use cdma_ios::{EncodedA1Message, MessageType};

const CALL_HISTORY_CAPACITY: usize = 1024;

#[derive(Clone, Default)]
struct CallRecord {
    context: A1Context,
    identity: Option<A1MobileIdentity>,
}

pub(super) struct A1EventSink {
    publisher: EventPublisher,
    node_id: String,
    calls: Mutex<LruCache<u64, CallRecord>>,
}

impl A1EventSink {
    pub(super) fn new(publisher: EventPublisher, node_id: String) -> Self {
        Self {
            publisher,
            node_id,
            calls: Mutex::new(LruCache::new(
                NonZeroUsize::new(CALL_HISTORY_CAPACITY).expect("call history capacity is nonzero"),
            )),
        }
    }

    pub(super) fn clear(&self) {
        self.calls.lock().clear();
    }

    pub(super) fn publish(&self, message: &EncodedA1Message, direction: A1Direction) {
        let mut calls = self.calls.lock();
        let call_key = correlation_key(message);
        let mut record = call_key
            .and_then(|id| calls.get(&id).cloned())
            .unwrap_or_default();
        let context = &mut record.context;
        context.direction = direction;
        context.a1_call_id = message.call_id();
        let Some(mut event) = a1_event_from_wire(EventSource::Bsc, &self.node_id, message, context)
        else {
            return;
        };
        if let Some(network_event::Body::A1(a1)) = event.body.as_mut() {
            if let Some(previous) = record.identity.as_ref() {
                let identity = a1.mobile_identity.get_or_insert_with(|| previous.clone());
                if identity.imsi == previous.imsi {
                    if identity.esn == 0 {
                        identity.esn = previous.esn;
                    }
                    if identity.meid.is_empty() {
                        identity.meid.clone_from(&previous.meid);
                    }
                }
                event.identity = Some(MobileIdentity {
                    imsi: identity.imsi.clone(),
                    esn: identity.esn,
                    meid: identity.meid.clone(),
                });
            }
            record.identity = a1.mobile_identity.clone();
        }
        if let Some(id) = call_key {
            if let Some(network_event::Body::A1(a1)) = &event.body {
                if let Some(identity) = &a1.mobile_identity {
                    context.correlated_imsi =
                        (!identity.imsi.is_empty()).then(|| identity.imsi.clone());
                }
                context.cell = a1.cell.as_ref().and_then(|cell| {
                    Some(cdma_ios::CellId {
                        cell: cell.cell.try_into().ok()?,
                        sector: cell.sector.try_into().ok()?,
                    })
                });
            }
            if message.message_type() == MessageType::ClearComplete {
                calls.pop(&id);
            } else {
                calls.put(id, record);
            }
        }
        drop(calls);
        self.publisher.publish(event);
    }
}

/// A connectionless Paging Request belongs to the call with its Tag.
fn correlation_key(message: &EncodedA1Message) -> Option<u64> {
    message.call_id().or_else(|| {
        if message.message_type() != MessageType::PagingRequest {
            return None;
        }
        let decoded = message.decode().ok()?;
        cdma_ios::PagingRequestMessage::decode(&decoded.payload)
            .ok()?
            .tag
            .map(|tag| u64::from(tag.0))
    })
}
