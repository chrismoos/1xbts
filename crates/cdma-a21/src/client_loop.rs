//! Convenience wrapper that owns an A21Client, maintains a
//! HybridIdentityCache, and runs an async recv loop applying inbound
//! IdentityBinding / IdentityRelease messages to the cache.

use std::net::SocketAddr;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::client::A21Client;
use crate::error::Result;
use crate::identity_cache::{CachedIdentity, HybridIdentityCache};
use crate::message::A21Message;

/// Background task driving a single A21 client connection. The owned cache
/// reflects the union of all `IdentityBinding` messages received from the
/// peer (minus any `IdentityRelease`).
pub struct A21ClientLoop {
    cache: HybridIdentityCache,
    client: std::sync::Arc<Mutex<A21Client>>,
    recv_task: JoinHandle<()>,
}

impl A21ClientLoop {
    /// Connect to the given A21 peer and spawn the recv loop.
    pub async fn connect(addr: SocketAddr) -> Result<Self> {
        let client = A21Client::connect(addr).await?;
        let client = std::sync::Arc::new(Mutex::new(client));
        let cache = HybridIdentityCache::new();

        let recv_task = {
            let cache = cache.clone();
            let client = std::sync::Arc::clone(&client);
            tokio::spawn(async move {
                loop {
                    let msg = {
                        let mut guard = client.lock().await;
                        match guard.recv().await {
                            Ok(m) => m,
                            Err(_e) => break,
                        }
                    };
                    apply(&cache, &msg);
                }
            })
        };

        Ok(Self {
            cache,
            client,
            recv_task,
        })
    }

    /// Cheap clone of the identity cache for use anywhere in the host
    /// process. Backed by `Arc<RwLock<…>>` internally.
    pub fn cache(&self) -> HybridIdentityCache {
        self.cache.clone()
    }

    /// Send one message back to the A21 peer (e.g. a CrossPageRequest).
    pub async fn send(&self, msg: &A21Message) -> Result<()> {
        let mut guard = self.client.lock().await;
        guard.send(msg).await
    }

    /// Abort the recv loop. Idempotent.
    pub fn shutdown(&self) {
        self.recv_task.abort();
    }
}

impl Drop for A21ClientLoop {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn apply(cache: &HybridIdentityCache, msg: &A21Message) {
    match msg {
        A21Message::IdentityBinding { imsi } => {
            cache.bind(CachedIdentity { imsi: *imsi });
        }
        A21Message::IdentityRelease { imsi } => {
            cache.release_by_imsi(*imsi);
        }
        // Cross-page traffic and suppression are pure forwarding; the cache
        // doesn't track them — the host process handles those via its own
        // callback layer.
        A21Message::CrossPageRequest { .. }
        | A21Message::CrossPageAck { .. }
        | A21Message::SuppressionStart { .. }
        | A21Message::SuppressionEnd { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_binding_updates_cache() {
        let c = HybridIdentityCache::new();
        apply(&c, &A21Message::IdentityBinding { imsi: 1 });
        assert!(c.is_hrpd_attached(1));
    }

    #[test]
    fn apply_release_removes_entry() {
        let c = HybridIdentityCache::new();
        apply(&c, &A21Message::IdentityBinding { imsi: 1 });
        apply(&c, &A21Message::IdentityRelease { imsi: 1 });
        assert!(!c.is_hrpd_attached(1));
    }

    #[test]
    fn apply_unrelated_messages_are_noops() {
        let c = HybridIdentityCache::new();
        apply(
            &c,
            &A21Message::CrossPageRequest {
                imsi: 5,
                source: crate::message::PagingSource::OneX,
                payload: vec![1, 2, 3],
            },
        );
        assert!(c.is_empty());
    }
}
