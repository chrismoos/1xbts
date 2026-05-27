/// IP address allocation for packet data sessions.
///
/// The `IpAllocator` trait abstracts address assignment so it can be
/// backed by a simple in-memory pool, a database, or an external DHCP
/// server without changing the session lifecycle code.
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::mobile_ip::MobileIpConfig;
use crate::ppp::ipcp::IpcpConfig;

const DEFAULT_RELEASE_GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub enum IpClaimResult {
    Claimed(IpcpConfig),
    AlreadyOwned(IpcpConfig),
    Conflict { current_owner: String },
    OutOfPool,
}

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

    /// Claim a peer IP observed from the mobile during PPP resume.
    fn claim_peer_ip(&self, session_id: &str, peer_ip: Ipv4Addr) -> IpClaimResult;
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
    /// allocation key → last assigned host octet.  Some mobiles keep using
    /// their last IPCP address across rapid packet-data reconnects.
    sticky: HashMap<String, u8>,
    /// Next host octet to try. This avoids immediate reuse of a recently
    /// released mobile IP while old network-side packets may still be in flight.
    next_host: u8,
    /// Recently released host octets that should only be reclaimable by their
    /// previous owner until the grace period expires.
    recently_released: HashMap<u8, (String, Instant)>,
    release_grace: Duration,
    /// DNS servers to include in IPCP config.
    primary_dns: Ipv4Addr,
    secondary_dns: Ipv4Addr,
    request_vj: bool,
    mobile_ip: MobileIpConfig,
}

impl SubnetIpAllocator {
    /// Create a pool allocator for the given `/24` subnet.
    ///
    /// `gateway` must end in `.1`; mobile addresses are `.2`–`.254`.
    ///
    /// # Panics
    /// Panics if the gateway's last octet is not `1`.
    pub fn new(gateway: Ipv4Addr, primary_dns: Ipv4Addr, secondary_dns: Ipv4Addr) -> Self {
        Self::new_with_vj_compression_default(gateway, primary_dns, secondary_dns, false)
    }

    /// Create a pool allocator and set whether local IPCP requests advertise VJ by default.
    ///
    /// `gateway` must end in `.1`; mobile addresses are `.2`–`.254`.
    ///
    /// # Panics
    /// Panics if the gateway's last octet is not `1`.
    pub fn new_with_vj_compression_default(
        gateway: Ipv4Addr,
        primary_dns: Ipv4Addr,
        secondary_dns: Ipv4Addr,
        request_vj: bool,
    ) -> Self {
        Self::new_with_packet_options(
            gateway,
            primary_dns,
            secondary_dns,
            request_vj,
            MobileIpConfig::default(),
        )
    }

    /// Create a pool allocator with packet-data options shared by all assignments.
    ///
    /// `gateway` must end in `.1`; mobile addresses are `.2`–`.254`.
    ///
    /// # Panics
    /// Panics if the gateway's last octet is not `1`.
    pub fn new_with_packet_options(
        gateway: Ipv4Addr,
        primary_dns: Ipv4Addr,
        secondary_dns: Ipv4Addr,
        request_vj: bool,
        mobile_ip: MobileIpConfig,
    ) -> Self {
        let octets = gateway.octets();
        assert_eq!(octets[3], 1, "gateway must be x.x.x.1");
        let prefix = [octets[0], octets[1], octets[2]];
        let free: HashSet<u8> = (2..=254).collect();
        Self {
            inner: Mutex::new(SubnetIpAllocatorInner {
                prefix,
                free,
                assigned: HashMap::new(),
                sticky: HashMap::new(),
                next_host: 2,
                recently_released: HashMap::new(),
                release_grace: DEFAULT_RELEASE_GRACE,
                primary_dns,
                secondary_dns,
                request_vj,
                mobile_ip,
            }),
        }
    }

    #[cfg(test)]
    fn with_release_grace(
        gateway: Ipv4Addr,
        primary_dns: Ipv4Addr,
        secondary_dns: Ipv4Addr,
        release_grace: Duration,
    ) -> Self {
        let allocator = Self::new(gateway, primary_dns, secondary_dns);
        allocator.inner.lock().unwrap().release_grace = release_grace;
        allocator
    }

    /// Create a pool with default settings: `10.55.0.0/24`, gateway DNS.
    pub fn default_subnet() -> Self {
        Self::new(
            Ipv4Addr::new(10, 55, 0, 1),
            Ipv4Addr::new(10, 55, 0, 1),
            Ipv4Addr::new(10, 55, 0, 1),
        )
    }
}

impl IpAllocator for SubnetIpAllocator {
    fn allocate(&self, session_id: &str) -> Option<IpcpConfig> {
        let mut inner = self.inner.lock().unwrap();
        inner.expire_recently_released();

        // If this session already has an allocation, return it.
        if let Some(&host) = inner.assigned.get(session_id) {
            let p = inner.prefix;
            return Some(IpcpConfig {
                our_ip: Ipv4Addr::new(p[0], p[1], p[2], 1),
                peer_ip: Ipv4Addr::new(p[0], p[1], p[2], host),
                primary_dns: inner.primary_dns,
                secondary_dns: inner.secondary_dns,
                request_vj: inner.request_vj,
                mobile_ip: inner.mobile_ip.clone(),
            });
        }

        let host = if let Some(&host) = inner.sticky.get(session_id) {
            if inner.free.contains(&host) {
                host
            } else {
                inner.allocate_host()?
            }
        } else {
            inner.allocate_host()?
        };
        inner.free.remove(&host);
        inner.recently_released.remove(&host);
        inner.assigned.insert(session_id.to_string(), host);
        inner.sticky.insert(session_id.to_string(), host);

        let p = inner.prefix;
        Some(IpcpConfig {
            our_ip: Ipv4Addr::new(p[0], p[1], p[2], 1),
            peer_ip: Ipv4Addr::new(p[0], p[1], p[2], host),
            primary_dns: inner.primary_dns,
            secondary_dns: inner.secondary_dns,
            request_vj: inner.request_vj,
            mobile_ip: inner.mobile_ip.clone(),
        })
    }

    fn release(&self, session_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(host) = inner.assigned.remove(session_id) {
            inner.free.insert(host);
            inner
                .recently_released
                .insert(host, (session_id.to_string(), Instant::now()));
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

    fn claim_peer_ip(&self, session_id: &str, peer_ip: Ipv4Addr) -> IpClaimResult {
        let mut inner = self.inner.lock().unwrap();
        inner.expire_recently_released();
        let Some(host) = inner.host_for_peer_ip(peer_ip) else {
            log::warn!(
                "ip_allocator: cannot claim {} for session {}, address is outside pool",
                peer_ip,
                session_id
            );
            return IpClaimResult::OutOfPool;
        };

        if let Some((owner, _)) = inner.recently_released.get(&host)
            && owner != session_id
        {
            log::warn!(
                "ip_allocator: cannot claim {} for session {}, address is reserved for recently released owner {}",
                peer_ip,
                session_id,
                owner
            );
            return IpClaimResult::Conflict {
                current_owner: owner.clone(),
            };
        }

        if let Some((owner, _)) = inner
            .assigned
            .iter()
            .find(|(key, assigned_host)| key.as_str() != session_id && **assigned_host == host)
        {
            log::warn!(
                "ip_allocator: cannot claim {} for session {}, address is already assigned to {}",
                peer_ip,
                session_id,
                owner
            );
            return IpClaimResult::Conflict {
                current_owner: owner.clone(),
            };
        }

        if inner.assigned.get(session_id) == Some(&host) {
            return IpClaimResult::AlreadyOwned(inner.config_for_host(host));
        }

        if let Some(old_host) = inner.assigned.insert(session_id.to_string(), host) {
            if old_host != host {
                inner.free.insert(old_host);
                inner
                    .recently_released
                    .insert(old_host, (session_id.to_string(), Instant::now()));
            }
        }
        inner.free.remove(&host);
        inner.recently_released.remove(&host);
        inner.sticky.insert(session_id.to_string(), host);

        log::info!(
            "ip_allocator: claimed {} for session {}",
            peer_ip,
            session_id
        );
        IpClaimResult::Claimed(inner.config_for_host(host))
    }
}

impl SubnetIpAllocatorInner {
    fn config_for_host(&self, host: u8) -> IpcpConfig {
        let p = self.prefix;
        IpcpConfig {
            our_ip: Ipv4Addr::new(p[0], p[1], p[2], 1),
            peer_ip: Ipv4Addr::new(p[0], p[1], p[2], host),
            primary_dns: self.primary_dns,
            secondary_dns: self.secondary_dns,
            request_vj: self.request_vj,
            mobile_ip: self.mobile_ip.clone(),
        }
    }

    fn host_for_peer_ip(&self, peer_ip: Ipv4Addr) -> Option<u8> {
        let octets = peer_ip.octets();
        if [octets[0], octets[1], octets[2]] != self.prefix {
            return None;
        }
        let host = octets[3];
        if !(2..=254).contains(&host) {
            return None;
        }
        Some(host)
    }

    fn allocate_host(&mut self) -> Option<u8> {
        for host in self.next_host..=254 {
            if self.free.contains(&host) && !self.recently_released.contains_key(&host) {
                self.next_host = if host == 254 { 2 } else { host + 1 };
                return Some(host);
            }
        }
        for host in 2..self.next_host {
            if self.free.contains(&host) && !self.recently_released.contains_key(&host) {
                self.next_host = if host == 254 { 2 } else { host + 1 };
                return Some(host);
            }
        }
        None
    }

    fn expire_recently_released(&mut self) {
        if self.release_grace.is_zero() {
            self.recently_released.clear();
            return;
        }
        let now = Instant::now();
        let release_grace = self.release_grace;
        self.recently_released
            .retain(|_, (_, released_at)| now.duration_since(*released_at) < release_grace);
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
        assert_eq!(c1.primary_dns, Ipv4Addr::new(10, 55, 0, 1));
        assert_eq!(c1.secondary_dns, Ipv4Addr::new(10, 55, 0, 1));
    }

    #[test]
    fn allocation_preserves_vj_request_default() {
        let alloc = SubnetIpAllocator::new_with_vj_compression_default(
            Ipv4Addr::new(10, 55, 0, 1),
            Ipv4Addr::new(10, 55, 0, 1),
            Ipv4Addr::new(10, 55, 0, 1),
            true,
        );

        let cfg = alloc.allocate("s1").unwrap();
        assert!(cfg.request_vj);
    }

    #[test]
    fn allocate_same_session_returns_same_ip() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        let c2 = alloc.allocate("s1").unwrap();
        assert_eq!(c1.peer_ip, c2.peer_ip);
    }

    #[test]
    fn released_session_reuses_sticky_ip() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        let ip = c1.peer_ip;
        alloc.release("s1");

        let c2 = alloc.allocate("s1").unwrap();
        assert_eq!(c2.peer_ip, ip);
    }

    #[test]
    fn released_ip_is_not_immediately_reused_for_different_key() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        let ip = c1.peer_ip;
        alloc.release("s1");

        let c2 = alloc.allocate("s2").unwrap();
        assert_ne!(c2.peer_ip, ip);
        assert_eq!(c2.peer_ip, Ipv4Addr::new(10, 55, 0, 3));
    }

    #[test]
    fn claim_peer_ip_reassigns_key_when_address_is_free() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        assert_eq!(c1.peer_ip, Ipv4Addr::new(10, 55, 0, 2));

        let claimed = alloc.claim_peer_ip("s1", Ipv4Addr::new(10, 55, 0, 9));
        assert!(matches!(
            claimed,
            IpClaimResult::Claimed(IpcpConfig {
                peer_ip,
                ..
            }) if peer_ip == Ipv4Addr::new(10, 55, 0, 9)
        ));

        let c2 = alloc.allocate("s2").unwrap();
        assert_eq!(c2.peer_ip, Ipv4Addr::new(10, 55, 0, 3));
    }

    #[test]
    fn claim_peer_ip_rejects_address_assigned_to_another_key() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        assert_eq!(c1.peer_ip, Ipv4Addr::new(10, 55, 0, 2));

        let claimed = alloc.claim_peer_ip("s2", c1.peer_ip);
        assert!(matches!(claimed, IpClaimResult::Conflict { .. }));
    }

    #[test]
    fn released_ip_claim_rejects_different_key_during_grace() {
        let alloc = SubnetIpAllocator::default_subnet();
        let c1 = alloc.allocate("s1").unwrap();
        alloc.release("s1");

        let claimed = alloc.claim_peer_ip("s2", c1.peer_ip);
        assert!(matches!(claimed, IpClaimResult::Conflict { .. }));
        assert!(matches!(
            alloc.claim_peer_ip("s1", c1.peer_ip),
            IpClaimResult::Claimed(_) | IpClaimResult::AlreadyOwned(_)
        ));
    }

    #[test]
    fn released_ip_grace_can_expire() {
        let alloc = SubnetIpAllocator::with_release_grace(
            Ipv4Addr::new(10, 55, 0, 1),
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(8, 8, 4, 4),
            Duration::ZERO,
        );
        let c1 = alloc.allocate("s1").unwrap();
        alloc.release("s1");

        assert!(matches!(
            alloc.claim_peer_ip("s2", c1.peer_ip),
            IpClaimResult::Claimed(_)
        ));
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
