//! Aggregated network-event bus.
//!
//! Components (BTS/BSC/MSC/PCF/PDSN/HLR/SMSC) publish events to a single
//! gRPC `EventService` hosted by `cdma-nib`. Subscribers receive a unified,
//! well-typed stream via `ListenEvents`. Producers communicate only over
//! gRPC — there is no in-process bus — so the same publish path works
//! whether the producer is in the same process or remote.

pub mod proto {
    tonic::include_proto!("events.v1");
}

mod enricher;
mod node_config;
mod publisher;
mod server;

pub use enricher::{
    CachingHlrEnricher, HlrEnricher, build_default_enricher, identity_is_empty,
    subscriber_is_unresolved,
};
pub use node_config::EventsNodeConfig;
pub use publisher::{EventPublisher, EventPublisherConfig, EventPublisherError};
pub use server::{EventBusConfig, EventBusServer, serve};

pub use proto::event_service_client::EventServiceClient;
pub use proto::event_service_server::{EventService, EventServiceServer};
pub use proto::{
    EventSource, ListenEventsRequest, MobileIdentity, NetworkEvent, PacketSessionBound,
    PacketSessionUnbindReason, PacketSessionUnbound, PdsnNetworkEvent, PublishAck, PublishRequest,
    Subscriber, SubscriberStatus, network_event, pdsn_network_event,
};
