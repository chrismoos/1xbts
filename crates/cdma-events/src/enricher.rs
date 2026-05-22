//! HLR enrichment hook for the event bus.
//!
//! Producers publish events with whatever subscriber context they have at
//! the event site — raw radio identifiers (IMSI/ESN), or a resolved HLR
//! subscriber UUID, or both. The bus calls into an `HlrEnricher` before
//! fan-out to fill in the missing side: forward-resolve `identity` to a
//! `Subscriber` record, or reverse-resolve a `subscriber_id` to its
//! primary `MobileIdentity`.
//!
//! The default `CachingHlrEnricher` impl lives here, takes an HLR gRPC
//! endpoint string, and stands up its own `HlrServiceClient`. cdma-events
//! is therefore self-contained: hand the bus an HLR endpoint and it
//! handles its own enrichment plumbing.
//!
//! The trait stays public so callers can substitute custom enrichers —
//! useful for testing or alternate backends.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdma_hlr::proto::hlr_service_client::HlrServiceClient;
use cdma_hlr::proto::{
    GetSubscriberRequest, MobileIdentityKey, ResolveSubscriberByIdentityRequest,
};
use lru::LruCache;
use parking_lot::Mutex;
use tonic::transport::{Channel, Endpoint};

use crate::proto::{MobileIdentity, Subscriber};

/// Best-effort, in-place enrichment of an event's subscriber context.
///
/// Implementations should:
/// - Skip when both `identity` and `subscriber.subscriber_id` are set
///   (producer already knew everything).
/// - Skip when neither is set (system event, nothing to resolve).
/// - On HLR error, log and leave the inputs unchanged. The event still
///   ships with whatever fields the producer provided.
#[async_trait::async_trait]
pub trait HlrEnricher: Send + Sync {
    async fn enrich(&self, identity: &mut MobileIdentity, subscriber: &mut Subscriber);
}

/// Convenience: identity carries no resolvable hint.
pub fn identity_is_empty(id: &MobileIdentity) -> bool {
    id.imsi.is_empty() && id.esn == 0 && id.meid.is_empty()
}

/// Convenience: subscriber carries no resolved UUID.
pub fn subscriber_is_unresolved(sub: &Subscriber) -> bool {
    sub.subscriber_id.is_empty()
}

// ─── Default implementation: gRPC-backed, LRU-cached ───────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CacheKey {
    ByIdentity {
        imsi: String,
        esn: u32,
        meid: String,
    },
    BySubscriberId(String),
}

#[derive(Clone)]
struct CacheEntry {
    /// `None` = HLR miss (negative cache).
    record: Option<CachedRecord>,
    inserted_at: Instant,
}

#[derive(Clone)]
struct CachedRecord {
    subscriber: Subscriber,
    identity: MobileIdentity,
}

/// Default enricher: holds one gRPC `HlrServiceClient` plus an LRU cache.
///
/// Forward and reverse lookups each fire a single HLR RPC because the HLR
/// read responses bundle `primary_identity` directly. Negative results
/// (HLR miss) are cached too so an unprovisioned roamer doesn't keep
/// re-querying every event.
pub struct CachingHlrEnricher {
    client: HlrServiceClient<Channel>,
    cache: Mutex<LruCache<CacheKey, CacheEntry>>,
    ttl: Duration,
}

impl CachingHlrEnricher {
    /// Connect to an HLR gRPC endpoint with the default cache (1024
    /// entries, 5-minute TTL). The channel is lazy — connection happens
    /// on first RPC.
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self, String> {
        Self::connect_with_config(
            endpoint,
            NonZeroUsize::new(1024).unwrap(),
            Duration::from_secs(300),
        )
        .await
    }

    pub async fn connect_with_config(
        endpoint: impl Into<String>,
        capacity: NonZeroUsize,
        ttl: Duration,
    ) -> Result<Self, String> {
        let endpoint = endpoint.into();
        let channel = Endpoint::from_shared(endpoint.clone())
            .map_err(|e| format!("invalid HLR endpoint {endpoint:?}: {e}"))?
            .connect_lazy();
        Ok(Self {
            client: HlrServiceClient::new(channel),
            cache: Mutex::new(LruCache::new(capacity)),
            ttl,
        })
    }

    fn lookup_cache(&self, key: &CacheKey) -> Option<Option<CachedRecord>> {
        let mut cache = self.cache.lock();
        let entry = cache.get(key)?;
        if entry.inserted_at.elapsed() >= self.ttl {
            cache.pop(key);
            return None;
        }
        Some(entry.record.clone())
    }

    fn insert_cache(&self, key: CacheKey, record: Option<CachedRecord>) {
        self.cache.lock().put(
            key,
            CacheEntry {
                record,
                inserted_at: Instant::now(),
            },
        );
    }
}

/// Map an HLR proto `Subscriber` (string-encoded UUID, enum-coded
/// status) into the events bus `Subscriber` (same proto package shape
/// but with `subscriber_id` as a String here too, since enum values
/// line up).
fn hlr_subscriber_to_bus(hlr: cdma_hlr::proto::Subscriber) -> Subscriber {
    Subscriber {
        subscriber_id: hlr.subscriber_id,
        phone_number: hlr.phone_number,
        display_name: hlr.display_name,
        // Both enums use the same wire values
        // (UNSPECIFIED/ACTIVE/SUSPENDED/DISABLED at 0..=3).
        status: hlr.status,
    }
}

fn hlr_identity_to_bus(hlr: cdma_hlr::proto::SubscriberIdentity) -> MobileIdentity {
    MobileIdentity {
        imsi: hlr.imsi.unwrap_or_default(),
        esn: hlr.esn.unwrap_or(0),
        meid: hlr.meid.unwrap_or_default(),
    }
}

#[async_trait::async_trait]
impl HlrEnricher for CachingHlrEnricher {
    async fn enrich(&self, identity: &mut MobileIdentity, subscriber: &mut Subscriber) {
        let has_identity = !identity_is_empty(identity);
        let has_subscriber = !subscriber_is_unresolved(subscriber);

        // Forward: producer gave us identity but no subscriber → resolve.
        if has_identity && !has_subscriber {
            let key = CacheKey::ByIdentity {
                imsi: identity.imsi.clone(),
                esn: identity.esn,
                meid: identity.meid.clone(),
            };
            let cached = self.lookup_cache(&key);
            let record = if let Some(record) = cached {
                record
            } else {
                let identity_key = match cdma_hlr::model::MobileIdentityKey::from_parts(
                    (!identity.imsi.is_empty()).then_some(identity.imsi.as_str()),
                    (identity.esn != 0).then_some(identity.esn),
                    (!identity.meid.is_empty()).then_some(identity.meid.as_str()),
                ) {
                    Ok(identity_key) => MobileIdentityKey {
                        imsi: Some(identity_key.imsi().to_string()),
                        esn: identity_key.esn(),
                        meid: identity_key.meid().map(ToOwned::to_owned),
                    },
                    Err(_) => return,
                };
                let mut client = self.client.clone();
                match client
                    .resolve_subscriber_by_identity(ResolveSubscriberByIdentityRequest {
                        identity: Some(identity_key),
                    })
                    .await
                {
                    Ok(resp) => {
                        let inner = resp.into_inner();
                        let record = inner.subscriber.map(|s| CachedRecord {
                            subscriber: hlr_subscriber_to_bus(s),
                            // The lookup-key identity is sufficient for
                            // cache: we asked HLR by it, so we know the
                            // mobile carries it. HLR's primary may
                            // differ (alternate identity / SIM swap) —
                            // future reverse-direction events will
                            // fetch the primary explicitly.
                            identity: inner
                                .primary_identity
                                .map(hlr_identity_to_bus)
                                .unwrap_or_else(|| MobileIdentity {
                                    imsi: identity.imsi.clone(),
                                    esn: identity.esn,
                                    meid: identity.meid.clone(),
                                }),
                        });
                        self.insert_cache(key, record.clone());
                        record
                    }
                    Err(err) => {
                        log::warn!("cdma-events: HLR ResolveSubscriberByIdentity failed: {err}");
                        None
                    }
                }
            };
            if let Some(r) = record {
                *subscriber = r.subscriber;
            }
            return;
        }

        // Reverse: producer gave us a subscriber UUID but no identity → fill.
        if has_subscriber {
            let key = CacheKey::BySubscriberId(subscriber.subscriber_id.clone());
            let cached = self.lookup_cache(&key);
            let record = if let Some(record) = cached {
                record
            } else {
                let mut client = self.client.clone();
                match client
                    .get_subscriber(GetSubscriberRequest {
                        subscriber_id: subscriber.subscriber_id.clone(),
                    })
                    .await
                {
                    Ok(resp) => {
                        let inner = resp.into_inner();
                        let record = inner.subscriber.map(|s| CachedRecord {
                            subscriber: hlr_subscriber_to_bus(s),
                            identity: inner
                                .primary_identity
                                .map(hlr_identity_to_bus)
                                .unwrap_or_default(),
                        });
                        self.insert_cache(key, record.clone());
                        record
                    }
                    Err(err) => {
                        // tonic NotFound is a normal "no such subscriber"
                        // result. Anything else is logged as a warning.
                        if err.code() != tonic::Code::NotFound {
                            log::warn!("cdma-events: HLR GetSubscriber failed: {err}");
                        }
                        self.insert_cache(key.clone(), None);
                        None
                    }
                }
            };
            if let Some(r) = record {
                if subscriber.phone_number.is_empty() {
                    subscriber.phone_number = r.subscriber.phone_number;
                }
                if subscriber.display_name.is_empty() {
                    subscriber.display_name = r.subscriber.display_name;
                }
                if subscriber.status == 0 {
                    subscriber.status = r.subscriber.status;
                }
                if !has_identity && !identity_is_empty(&r.identity) {
                    *identity = r.identity;
                }
            }
        }
    }
}

/// Spawn a `CachingHlrEnricher` from an `EventsNodeConfig`. Returns
/// `None` if the config didn't specify an HLR endpoint.
pub async fn build_default_enricher(
    config: &crate::EventsNodeConfig,
) -> Result<Option<Arc<dyn HlrEnricher>>, String> {
    let Some(endpoint) = config.hlr_endpoint.as_deref() else {
        return Ok(None);
    };
    let enricher = CachingHlrEnricher::connect(endpoint).await?;
    Ok(Some(Arc::new(enricher)))
}
