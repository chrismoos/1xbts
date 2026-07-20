//! Broadcast hub for A21 server-side connections.
//!
//! Tracks live `A21Connection` handles accepted by an `A21Server` and lets
//! the application push the same `A21Message` to every currently-connected
//! peer. Used by the HRPD AN to broadcast `IdentityBinding` /
//! `IdentityRelease` to all attached 1x BSC clients without polling.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::message::A21Message;
use crate::server::A21Connection;

#[derive(Clone, Default)]
pub struct A21Hub {
    inner: Arc<Mutex<Vec<A21Connection>>>,
}

impl A21Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a connection for future broadcasts.
    pub async fn register(&self, conn: A21Connection) {
        self.inner.lock().await.push(conn);
    }

    /// Best-effort fan-out: sends `msg` to every registered connection.
    /// Connections whose send fails (broken pipe, timed out, …) are dropped
    /// from the hub on the next broadcast. Returns the count delivered.
    pub async fn broadcast(&self, msg: A21Message) -> usize {
        let snapshot = self.inner.lock().await.clone();
        let mut alive: Vec<A21Connection> = Vec::with_capacity(snapshot.len());
        let mut delivered = 0usize;
        for c in snapshot {
            if c.send(&msg).await.is_ok() {
                alive.push(c);
                delivered += 1;
            }
        }
        // Replace the live set with survivors. Connections that newly
        // registered between snapshot and rebuild are preserved by appending
        // any extras at the tail.
        let mut g = self.inner.lock().await;
        let total = g.len();
        if total > alive.len() {
            // There are new registrations; keep them.
            let extras: Vec<A21Connection> = g.drain(alive.len()..).collect();
            *g = alive;
            g.extend(extras);
        } else {
            *g = alive;
        }
        delivered
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_hub_broadcasts_to_zero() {
        let h = A21Hub::new();
        let n = h.broadcast(A21Message::IdentityRelease { imsi: 1 }).await;
        assert_eq!(n, 0);
        assert!(h.is_empty().await);
    }
}
