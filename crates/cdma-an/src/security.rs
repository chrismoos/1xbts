//! Security Layer — pass-through framing for Rev 0 default protocols.
//!
//! C.S0024-400 §9 defines the Security Layer as the composition of three
//! protocol slots: Key Exchange, Authentication, and Encryption. Rev 0
//! baseline negotiates the "Default" subtype for all three. Per
//! C.S0024-400 §9.3 (Default Key Exchange), §9.4 (Default Authentication),
//! and §9.5 (Default Encryption), each Default protocol forwards the SDU it
//! receives to the next layer unchanged: there are no header bytes, no
//! trailer, no MAC, and no transformation. The Security Layer therefore
//! reduces to identity framing over Stream Layer PDUs in Rev 0.
//!
//! Real authentication / key agreement (CAVE-derived keys, AKA per
//! X.S0011-005) is out of scope for Rev 0 and is deferred.

use serde::{Deserialize, Serialize};

/// Subtype negotiated for a Security-Layer protocol slot.
///
/// Rev 0 only supports `Default`. Future enhanced subtypes (e.g. SHA-1
/// authentication, AES encryption per X.S0011-005) will appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecuritySubtype {
    /// Default (no-op) subtype per C.S0024-400 §9.3 / §9.4 / §9.5.
    Default,
}

/// Security Layer state: negotiated subtype for each of the three slots.
///
/// Layout follows C.S0024-400 §9.1. The composition is
/// `KeyExchange ∘ Authentication ∘ Encryption`; under Rev 0 defaults every
/// stage is identity, so the composed transform is also identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityLayer {
    pub key_exchange: SecuritySubtype,
    pub authentication: SecuritySubtype,
    pub encryption: SecuritySubtype,
}

/// Errors produced by Security Layer encapsulation / decapsulation.
///
/// Rev 0 Default subtypes cannot fail (identity framing); variants exist so
/// future non-Default subtypes (AKA, AES) can report integrity / decrypt
/// failures without an API break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityError {
    /// A non-Default subtype is negotiated but its implementation has not
    /// been wired in yet.
    UnsupportedSubtype,
    /// Authentication / integrity check failed (reserved for future use).
    AuthenticationFailed,
    /// Decryption failed (reserved for future use).
    DecryptionFailed,
}

impl SecurityLayer {
    /// Rev 0 baseline: every slot pinned to Default (no-op).
    pub const fn rev0_default() -> Self {
        Self {
            key_exchange: SecuritySubtype::Default,
            authentication: SecuritySubtype::Default,
            encryption: SecuritySubtype::Default,
        }
    }

    /// Encapsulate a Stream Layer PDU into a Security Layer PDU.
    ///
    /// For Rev 0 Default subtypes this is identity: no header, no trailer.
    /// See C.S0024-400 §9.3 / §9.4 / §9.5.
    pub fn encapsulate(&self, stream_pdu: &[u8]) -> Vec<u8> {
        match (self.key_exchange, self.authentication, self.encryption) {
            (SecuritySubtype::Default, SecuritySubtype::Default, SecuritySubtype::Default) => {
                stream_pdu.to_vec()
            } // TODO(X.S0011-005): dispatch to AKA Authentication / encryption
              // subtypes here once real security is wired in.
        }
    }

    /// Decapsulate a Security Layer PDU back to a Stream Layer PDU.
    ///
    /// Inverse of [`Self::encapsulate`]. For Rev 0 Default subtypes this is
    /// identity and cannot fail.
    pub fn decapsulate(&self, security_pdu: &[u8]) -> Result<Vec<u8>, SecurityError> {
        match (self.key_exchange, self.authentication, self.encryption) {
            (SecuritySubtype::Default, SecuritySubtype::Default, SecuritySubtype::Default) => {
                Ok(security_pdu.to_vec())
            } // TODO(X.S0011-005): dispatch to AKA Authentication / decryption
              // subtypes here once real security is wired in.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev0_default_sets_all_three_slots_to_default() {
        let s = SecurityLayer::rev0_default();
        assert_eq!(s.key_exchange, SecuritySubtype::Default);
        assert_eq!(s.authentication, SecuritySubtype::Default);
        assert_eq!(s.encryption, SecuritySubtype::Default);
    }

    #[test]
    fn rev0_default_round_trips_nonempty_pdu() {
        let s = SecurityLayer::rev0_default();
        let pdu = b"abc";
        let wrapped = s.encapsulate(pdu);
        assert_eq!(wrapped, pdu);
        let unwrapped = s.decapsulate(&wrapped).expect("decapsulate");
        assert_eq!(unwrapped, pdu);
    }

    #[test]
    fn rev0_default_round_trips_empty_pdu() {
        let s = SecurityLayer::rev0_default();
        let wrapped = s.encapsulate(&[]);
        assert!(wrapped.is_empty());
        let unwrapped = s.decapsulate(&wrapped).expect("decapsulate");
        assert!(unwrapped.is_empty());
    }

    #[test]
    fn rev0_default_encapsulate_is_identity_for_arbitrary_bytes() {
        let s = SecurityLayer::rev0_default();
        let pdu: Vec<u8> = (0u8..=255).collect();
        let wrapped = s.encapsulate(&pdu);
        assert_eq!(wrapped, pdu);
        assert_eq!(s.decapsulate(&wrapped).unwrap(), pdu);
    }
}
