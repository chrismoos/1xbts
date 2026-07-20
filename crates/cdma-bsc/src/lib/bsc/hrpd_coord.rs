//! HRPD-side A21 coordination handle held by the BSC.
//!
//! Bundles the read-side `HybridIdentityCache` with a fire-and-forget send
//! channel for emitting `A21Message::CrossPageRequest` / `SuppressionStart`
//! / etc. from synchronous BSC code paths. The actual TCP connection lives
//! in an `A21ClientLoop` spawned by the launcher.

use cdma_a21::{A21Message, HybridIdentityCache};
use tokio::sync::mpsc;

/// Lightweight read+write handle into the BSC's view of the HRPD AN.
#[derive(Debug, Clone)]
pub struct HrpdCoord {
    /// Cache of inbound IdentityBinding/Release messages from the AN.
    pub cache: HybridIdentityCache,
    /// Outbound A21 message sink. Drops are fatal-soft (we log and continue
    /// 1x paging) so the BSC stays alive if the AN process is down.
    pub send: mpsc::UnboundedSender<A21Message>,
}

impl HrpdCoord {
    pub fn new(cache: HybridIdentityCache, send: mpsc::UnboundedSender<A21Message>) -> Self {
        Self { cache, send }
    }

    /// Parse a decimal-IMSI string (15 digits, MCC+MNC+MSIN) into the u64
    /// form expected by the A21 wire protocol + HybridIdentityCache.
    /// Returns None if `imsi` isn't all digits or overflows u64.
    pub fn parse_imsi(imsi: &str) -> Option<u64> {
        let s = imsi.trim();
        if s.is_empty() {
            return None;
        }
        if !s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<u64>().ok()
    }

    /// `true` when the named IMSI is currently HRPD-attached per the A21
    /// cache.
    pub fn is_hrpd_attached(&self, imsi: &str) -> bool {
        match Self::parse_imsi(imsi) {
            Some(v) => self.cache.is_hrpd_attached(v),
            None => false,
        }
    }

    /// Fire-and-forget: emit an A21 CrossPageRequest. Returns `false` if
    /// the channel is closed (AN side gone).
    pub fn emit_cross_page(
        &self,
        imsi: &str,
        source: cdma_a21::PagingSource,
        payload: Vec<u8>,
    ) -> bool {
        let Some(v) = Self::parse_imsi(imsi) else {
            return false;
        };
        self.send
            .send(A21Message::CrossPageRequest {
                imsi: v,
                source,
                payload,
            })
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_a21::CachedIdentity;

    fn coord() -> (HrpdCoord, mpsc::UnboundedReceiver<A21Message>) {
        let cache = HybridIdentityCache::new();
        let (tx, rx) = mpsc::unbounded_channel();
        (HrpdCoord::new(cache, tx), rx)
    }

    #[test]
    fn parse_imsi_accepts_15_digit_strings() {
        assert_eq!(
            HrpdCoord::parse_imsi("001010123456789"),
            Some(1_010_123_456_789u64)
        );
        assert_eq!(HrpdCoord::parse_imsi(""), None);
        assert_eq!(HrpdCoord::parse_imsi("abc"), None);
        assert_eq!(HrpdCoord::parse_imsi("123-456"), None);
    }

    #[test]
    fn is_hrpd_attached_routes_through_cache() {
        let (c, _rx) = coord();
        c.cache.bind(CachedIdentity { imsi: 7 });
        assert!(c.is_hrpd_attached("7"));
        assert!(!c.is_hrpd_attached("8"));
    }

    #[test]
    fn emit_cross_page_sends_message() {
        let (c, mut rx) = coord();
        assert!(c.emit_cross_page("42", cdma_a21::PagingSource::OneX, vec![1, 2, 3]));
        let m = rx.try_recv().unwrap();
        match m {
            A21Message::CrossPageRequest { imsi, payload, .. } => {
                assert_eq!(imsi, 42);
                assert_eq!(payload, vec![1, 2, 3]);
            }
            _ => panic!("wrong message: {m:?}"),
        }
    }

    #[test]
    fn emit_cross_page_rejects_non_numeric_imsi() {
        let (c, _rx) = coord();
        assert!(!c.emit_cross_page("xyz", cdma_a21::PagingSource::OneX, vec![]));
    }
}
