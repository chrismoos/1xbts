/// IP address allocation for packet data sessions.
///
/// The `IpAllocator` trait abstracts address assignment so it can be
/// backed by a simple in-memory pool, a database, or an external DHCP
/// server without changing the session lifecycle code.
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::Mutex;

use crate::ppp::ipcp::IpcpConfig;

/// Allocates IP configurations for packet data sessions.
///
/// Implementations must be `Send + Sync` (shared via `Arc` across
/// session tasks). Use interior mutability (`Mutex`, etc.) for state.
pub trait IpAllocator: Send + Sync {
    /// Allocate an IP configuration for the given session.
    /// Returns `None` if the pool is exhausted.
    fn allocate(&self, session_id: &str) -> Option<IpcpConfig>;

    /// Release the allocation for the given session. No-op if the
    /// session was never allocated or already released.
    fn release(&self, session_id: &str);
}

/// In-memory pool allocator that hands out unique `/32` addresses from
/// a configurable `/24` subnet.
///
/// Gateway sits at `.1`, mobile addresses are `.2` through `.254`.
/// Thread-safe via internal `Mutex`.
pub struct SubnetIpAllocator {
    inner: Mutex<SubnetIpAllocatorInner>,
}

struct SubnetIpAllocatorInner {
    /// First three octets of the subnet (e.g., [10, 55, 0]).
    prefix: [u8; 3],
    /// Available host octets (2..=254).
    free: HashSet<u8>,
    /// session_id → assigned host octet.
    assigned: HashMap<String, u8>,
    /// DNS servers to include in IPCP config.
    primary_dns: Ipv4Addr,
    secondary_dns: Ipv4Addr,
}

impl SubnetIpAllocator {
    /// Create a pool allocator for the given `/24` subnet.
    ///
    /// `gateway` must end in `.1`; mobile addresses are `.2`–`.254`.
    ///
    /// # Panics
    /// Panics if the gateway's last octet is not `1`.
    pub fn new(gateway: Ipv4Addr, primary_dns: Ipv4Addr, secondary_dns: Ipv4Addr) -> Self {
        let octets = gateway.octets();
        assert_eq!(octets[3], 1, "gateway must be x.x.x.1");
        let prefix = [octets[0], octets[1], octets[2]];
        let free: HashSet<u8> = (2..=254).collect();
        Self {
            inner: Mutex::new(SubnetIpAllocatorInner {
                prefix,
                free,
                assigned: HashMap::new(),
                primary_dns,
                secondary_dns,
            }),
        }
    }

    /// Create a pool with default settings: `10.55.0.0/24`, Google DNS.
    pub fn default_subnet() -> Self {
        Self::new(
            Ipv4Addr::new(10, 55, 0, 1),
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(8, 8, 4, 4),
        )
    }
}

impl IpAllocator for SubnetIpAllocator {
    fn allocate(&self, session_id: &str) -> Option<IpcpConfig> {
        let mut inner = self.inner.lock().unwrap();

        // If this session already has an allocation, return it.
        if let Some(&host) = inner.assigned.get(session_id) {
            let p = inner.prefix;
            return Some(IpcpConfig {
                our_ip: Ipv4Addr::new(p[0], p[1], p[2], 1),
                peer_ip: Ipv4Addr::new(p[0], p[1], p[2], host),
                primary_dns: inner.primary_dns,
                secondary_dns: inner.secondary_dns,
            });
        }

        // Pick the lowest available host octet for determinism.
        let host = inner.free.iter().copied().min()?;
        inner.free.remove(&host);
        inner.assigned.insert(session_id.to_string(), host);

        let p = inner.prefix;
        Some(IpcpConfig {
            our_ip: Ipv4Addr::new(p[0], p[1], p[2], 1),
            peer_ip: Ipv4Addr::new(p[0], p[1], p[2], host),
            primary_dns: inner.primary_dns,
            secondary_dns: inner.secondary_dns,
        })
    }

    fn release(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(host) = inner.assigned.remove(session_id) {
            inner.free.insert(host);
            log::info!(
                "ip_allocator: released {}.{}.{}.{} for session {}",
                inner.prefix[0],
                inner.prefix[1],
                inner.prefix[2],
                host,
                session_id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_returns_unique_ips() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        let c2 = alloc.allocate("s2").unwrap();
        assert_ne!(c1.peer_ip, c2.peer_ip);
        assert_eq!(c1.our_ip, c2.our_ip); // same gateway
        assert_eq!(c1.peer_ip, Ipv4Addr::new(10, 55, 0, 2));
        assert_eq!(c2.peer_ip, Ipv4Addr::new(10, 55, 0, 3));
    }

    #[test]
    fn allocate_same_session_returns_same_ip() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        let c2 = alloc.allocate("s1").unwrap();
        assert_eq!(c1.peer_ip, c2.peer_ip);
    }

    #[test]
    fn release_returns_ip_to_pool() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        let ip = c1.peer_ip;
        alloc.release("s1");

        // Next allocation should get the same IP back (lowest available).
        let c2 = alloc.allocate("s2").unwrap();
        assert_eq!(c2.peer_ip, ip);
    }

    #[test]
    fn exhaust_pool() {
        let alloc = SubnetIpAllocator::default_subnet();
        // 253 addresses available (.2 through .254)
        for i in 0..253 {
            assert!(
                alloc.allocate(&format!("s{}", i)).is_some(),
                "failed at {}",
                i
            );
        }
        // 254th should fail
        assert!(alloc.allocate("overflow").is_none());
    }

    #[test]
    fn release_nonexistent_is_noop() {
        let alloc = SubnetIpAllocator::default_subnet();
        alloc.release("never_allocated"); // should not panic
    }
}
