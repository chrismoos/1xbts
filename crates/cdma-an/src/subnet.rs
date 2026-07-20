//! UATI subnet and per-sector allocator.
//!
//! C.S0024 Address Management assigns a 128-bit UATI as `UATI104 | UATI024`.
//! This allocator owns only the 24-bit `UATI024` space for one AN process. A
//! real multi-sector deployment must persist allocations and coordinate the
//! shared UATI024 space across sectors that advertise the same UATI104.

use crate::uati::Uati;
use std::collections::HashSet;
use thiserror::Error;

/// UATI subnet configuration for a sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UatiSubnet {
    /// Color code (C.S0024-400 §8.2): tags the subnet so ATs can detect
    /// inter-subnet handoff.
    pub color_code: u8,
    /// UATI[127:24] assigned when the AN includes explicit UATI104 in
    /// UATIAssignment.
    pub uati104: [u8; 13],
    /// Full 128-bit UATI subnet mask length.
    pub subnet_mask: u8,
}

impl UatiSubnet {
    /// Number of usable UATI024 values. UATI024=0 is reserved as null/broadcast.
    pub fn capacity(&self) -> u64 {
        0x00ff_ffff
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AllocatorError {
    #[error("UATI024 space exhausted")]
    Exhausted,
    #[error("failed to allocate a random UATI024 after retrying collisions")]
    CollisionRetryLimit,
    #[error("UATI {0} not allocated")]
    NotAllocated(Uati),
    #[error("UATI {0} does not match this allocator")]
    OutsideAllocator(Uati),
}

const RANDOM_ALLOCATE_MAX_ATTEMPTS: usize = 128;

/// Random UATI024 allocator with a free-list backed by a `HashSet`.
#[derive(Debug)]
pub struct UatiAllocator {
    subnet: UatiSubnet,
    issued: HashSet<u32>,
    #[cfg(test)]
    force_exhausted: bool,
    #[cfg(test)]
    next_test_uati024: u32,
}

impl UatiAllocator {
    pub fn new(subnet: UatiSubnet) -> Self {
        Self {
            subnet,
            issued: HashSet::new(),
            #[cfg(test)]
            force_exhausted: false,
            #[cfg(test)]
            next_test_uati024: 0x0005_8001,
        }
    }

    pub fn subnet(&self) -> &UatiSubnet {
        &self.subnet
    }

    /// Allocate a random non-zero UATI024. Collisions are retried a bounded
    /// number of times so allocator bugs or near-exhaustion fail explicitly.
    pub fn allocate(&mut self) -> Result<Uati, AllocatorError> {
        #[cfg(test)]
        if self.force_exhausted {
            return Err(AllocatorError::Exhausted);
        }
        if (self.issued.len() as u64) >= self.subnet.capacity() {
            return Err(AllocatorError::Exhausted);
        }

        #[cfg(test)]
        {
            for _ in 0..RANDOM_ALLOCATE_MAX_ATTEMPTS {
                let candidate_uati024 = self.next_test_uati024 & 0x00ff_ffff;
                self.next_test_uati024 = if self.next_test_uati024 >= 0x00ff_ffff {
                    1
                } else {
                    self.next_test_uati024 + 1
                };
                if candidate_uati024 == 0 {
                    continue;
                }
                let uati = Uati::from_compact(
                    candidate_uati024,
                    self.subnet.uati104,
                    self.subnet.color_code,
                    self.subnet.subnet_mask,
                );
                if self.issued.insert(uati.as_u32()) {
                    return Ok(uati);
                }
            }
            Err(AllocatorError::CollisionRetryLimit)
        }

        #[cfg(not(test))]
        for _ in 0..RANDOM_ALLOCATE_MAX_ATTEMPTS {
            let Some(candidate_uati024) = random_uati024() else {
                continue;
            };
            let uati = Uati::from_compact(
                candidate_uati024,
                self.subnet.uati104,
                self.subnet.color_code,
                self.subnet.subnet_mask,
            );
            if self.issued.insert(uati.as_u32()) {
                return Ok(uati);
            }
        }
        #[cfg(not(test))]
        Err(AllocatorError::CollisionRetryLimit)
    }

    /// Return a previously issued UATI to the free pool.
    pub fn release(&mut self, uati: Uati) -> Result<(), AllocatorError> {
        if self.issued.remove(&uati.as_u32()) {
            Ok(())
        } else {
            Err(AllocatorError::NotAllocated(uati))
        }
    }

    pub fn reserve(&mut self, uati: Uati) -> Result<(), AllocatorError> {
        if !self.contains(uati) {
            return Err(AllocatorError::OutsideAllocator(uati));
        }
        self.issued.insert(uati.as_u32());
        Ok(())
    }

    pub fn contains(&self, uati: Uati) -> bool {
        let full = uati.full();
        full.uati024() != 0
            && uati.as_u32() == full.uati024()
            && full.value()[..13] == self.subnet.uati104
            && full.color_code() == self.subnet.color_code
            && full.subnet_mask() == self.subnet.subnet_mask
    }

    pub fn issued_count(&self) -> usize {
        self.issued.len()
    }

    #[cfg(test)]
    pub fn force_exhausted_for_test(&mut self) {
        self.force_exhausted = true;
    }
}

#[cfg(not(test))]
fn random_uati024() -> Option<u32> {
    let mut bytes = [0u8; 4];
    getrandom::getrandom(&mut bytes).ok()?;
    let value = u32::from_be_bytes(bytes) & 0x00ff_ffff;
    (value != 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_subnet() -> UatiSubnet {
        UatiSubnet {
            color_code: 7,
            uati104: [0; 13],
            subnet_mask: 26,
        }
    }

    fn u(v: u32) -> Uati {
        Uati::from_compact(v, [0; 13], 7, 26)
    }

    #[test]
    fn allocates_unique_uatis() {
        let mut alloc = UatiAllocator::new(small_subnet());
        let a = alloc.allocate().unwrap();
        let b = alloc.allocate().unwrap();
        let c = alloc.allocate().unwrap();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        for u in [a, b, c] {
            assert!(u.as_u32() > 0);
            assert!(u.as_u32() <= 0x00ff_ffff);
            assert!(alloc.contains(u));
        }
    }

    #[test]
    fn release_returns_to_pool() {
        let mut alloc = UatiAllocator::new(small_subnet());
        let a = alloc.allocate().unwrap();
        alloc.release(a).unwrap();
        let reissued = alloc.allocate().unwrap();
        assert!(alloc.contains(reissued));
    }

    #[test]
    fn release_unknown_uati_errors() {
        let mut alloc = UatiAllocator::new(small_subnet());
        let err = alloc.release(u(0xDEAD_BEEF)).unwrap_err();
        assert_eq!(err, AllocatorError::NotAllocated(u(0xDEAD_BEEF)));
    }

    #[test]
    fn reserve_cached_uati_in_subnet() {
        let mut alloc = UatiAllocator::new(small_subnet());
        alloc.reserve(u(0x0000_0001)).unwrap();
        assert_eq!(alloc.issued_count(), 1);
        let wrong_uati104 = Uati::from_compact(0x0000_0002, [1; 13], 7, 26);
        assert_eq!(
            alloc.reserve(wrong_uati104),
            Err(AllocatorError::OutsideAllocator(wrong_uati104))
        );
    }

    #[test]
    fn zero_uati024_is_outside_allocator() {
        let mut alloc = UatiAllocator::new(small_subnet());
        assert_eq!(
            alloc.reserve(u(0)),
            Err(AllocatorError::OutsideAllocator(u(0)))
        );
    }
}
