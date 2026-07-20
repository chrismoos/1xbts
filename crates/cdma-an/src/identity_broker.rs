//! Hybrid-AT identity broker. C.S0024-0 v4.0 + A.S0017-D.
//!
//! Tracks the IMSI ↔ UATI ↔ color_code mapping so the 1x BSC can ask "is this
//! IMSI currently HRPD-attached?" and the HRPD AN can answer
//! cross-page requests with the right air-link page payload.
//!
//! Synchronization is left to the caller (wrap it in a `Mutex` or `RwLock` at
//! the application layer); the type itself is `&mut self`-mutated.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentityBinding {
    pub imsi: u64,
    pub uati: u32,
    pub color_code: u8,
}

#[derive(Debug, Default)]
pub struct IdentityBroker {
    by_imsi: HashMap<u64, IdentityBinding>,
    by_uati: HashMap<u32, u64>,
}

impl IdentityBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a binding. Returns the previous binding if any.
    pub fn bind(&mut self, b: IdentityBinding) -> Option<IdentityBinding> {
        // Drop any previous UATI for this IMSI from the reverse map.
        if let Some(prev) = self.by_imsi.get(&b.imsi).copied() {
            self.by_uati.remove(&prev.uati);
        }
        self.by_uati.insert(b.uati, b.imsi);
        self.by_imsi.insert(b.imsi, b)
    }

    /// Release the binding for `imsi`, if any. Returns the removed binding.
    pub fn release_by_imsi(&mut self, imsi: u64) -> Option<IdentityBinding> {
        let removed = self.by_imsi.remove(&imsi)?;
        self.by_uati.remove(&removed.uati);
        Some(removed)
    }

    /// Release the binding for `uati`, if any.
    pub fn release_by_uati(&mut self, uati: u32) -> Option<IdentityBinding> {
        let imsi = self.by_uati.remove(&uati)?;
        self.by_imsi.remove(&imsi)
    }

    pub fn lookup_by_imsi(&self, imsi: u64) -> Option<IdentityBinding> {
        self.by_imsi.get(&imsi).copied()
    }

    pub fn lookup_by_uati(&self, uati: u32) -> Option<IdentityBinding> {
        let imsi = *self.by_uati.get(&uati)?;
        self.by_imsi.get(&imsi).copied()
    }

    pub fn is_hrpd_attached(&self, imsi: u64) -> bool {
        self.by_imsi.contains_key(&imsi)
    }

    pub fn len(&self) -> usize {
        self.by_imsi.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_imsi.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = IdentityBinding> + '_ {
        self.by_imsi.values().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(imsi: u64, uati: u32, cc: u8) -> IdentityBinding {
        IdentityBinding {
            imsi,
            uati,
            color_code: cc,
        }
    }

    #[test]
    fn bind_and_lookup() {
        let mut br = IdentityBroker::new();
        assert!(br.bind(b(1001, 0xA, 7)).is_none());
        assert_eq!(br.lookup_by_imsi(1001).unwrap().uati, 0xA);
        assert_eq!(br.lookup_by_uati(0xA).unwrap().imsi, 1001);
        assert!(br.is_hrpd_attached(1001));
        assert!(!br.is_hrpd_attached(9999));
    }

    #[test]
    fn release_by_imsi_clears_both_maps() {
        let mut br = IdentityBroker::new();
        br.bind(b(2, 0x20, 3));
        let removed = br.release_by_imsi(2).unwrap();
        assert_eq!(removed.uati, 0x20);
        assert!(br.lookup_by_imsi(2).is_none());
        assert!(br.lookup_by_uati(0x20).is_none());
    }

    #[test]
    fn release_by_uati_clears_both_maps() {
        let mut br = IdentityBroker::new();
        br.bind(b(2, 0x20, 3));
        let removed = br.release_by_uati(0x20).unwrap();
        assert_eq!(removed.imsi, 2);
        assert!(br.is_empty());
    }

    #[test]
    fn rebinding_imsi_releases_prior_uati() {
        let mut br = IdentityBroker::new();
        br.bind(b(7, 0x70, 1));
        let prev = br.bind(b(7, 0x71, 1)).unwrap();
        assert_eq!(prev.uati, 0x70);
        assert!(br.lookup_by_uati(0x70).is_none());
        assert_eq!(br.lookup_by_imsi(7).unwrap().uati, 0x71);
    }

    #[test]
    fn release_unknown_is_none() {
        let mut br = IdentityBroker::new();
        assert!(br.release_by_imsi(123).is_none());
        assert!(br.release_by_uati(0xDEAD).is_none());
    }
}
