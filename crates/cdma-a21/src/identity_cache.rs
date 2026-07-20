//! Local cache of HRPD-attached IMSIs fed by inbound A21 messages.
//!
//! Any A21 peer (1x BSC, MSC, NIB, …) can wire this cache into a recv loop
//! and query "is this IMSI HRPD-attached?" synchronously from its own paging
//! / routing paths. The AN owns the IMSI↔UATI mapping; peers only track
//! presence.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// One attach entry announced by the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedIdentity {
    pub imsi: u64,
}

/// Thread-safe identity cache. Cheap clones share state.
#[derive(Debug, Clone, Default)]
pub struct HybridIdentityCache {
    inner: Arc<RwLock<HashSet<u64>>>,
}

impl HybridIdentityCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&self, c: CachedIdentity) {
        self.inner
            .write()
            .expect("identity cache write")
            .insert(c.imsi);
    }

    /// Removes a binding. Returns `true` if the IMSI was present.
    pub fn release_by_imsi(&self, imsi: u64) -> bool {
        self.inner
            .write()
            .expect("identity cache write")
            .remove(&imsi)
    }

    pub fn is_hrpd_attached(&self, imsi: u64) -> bool {
        self.inner
            .read()
            .expect("identity cache read")
            .contains(&imsi)
    }

    pub fn len(&self) -> usize {
        self.inner.read().expect("identity cache read").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> Vec<CachedIdentity> {
        self.inner
            .read()
            .expect("identity cache read")
            .iter()
            .map(|&imsi| CachedIdentity { imsi })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_and_query() {
        let c = HybridIdentityCache::new();
        c.bind(CachedIdentity { imsi: 42 });
        assert!(c.is_hrpd_attached(42));
        assert!(!c.is_hrpd_attached(99));
    }

    #[test]
    fn release_clears_entry() {
        let c = HybridIdentityCache::new();
        c.bind(CachedIdentity { imsi: 1 });
        assert!(c.release_by_imsi(1));
        assert!(!c.is_hrpd_attached(1));
    }

    #[test]
    fn cheap_clone_shares_state() {
        let a = HybridIdentityCache::new();
        let b = a.clone();
        a.bind(CachedIdentity { imsi: 7 });
        assert!(b.is_hrpd_attached(7));
    }
}
