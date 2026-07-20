//! Session Configuration Protocol — negotiated protocol subtypes.
//!
//! C.S0024-500 (Session Configuration) negotiates a per-protocol subtype ID
//! across 12 protocol slots (C.S0024-400 Table 1.5-1). Rev 0 baseline is the
//! "Default" subtype for every slot.

use serde::{Deserialize, Serialize};

/// Subtype ID negotiated for a single protocol slot.
///
/// Rev 0 only supports "Default" for every slot. Variants will be added as
/// later revisions / enhanced subtypes (e.g. Enhanced Idle State, Enhanced
/// Access Channel MAC) are wired in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolSubtype {
    /// Rev 0 default subtype for this protocol slot.
    Default,
}

/// The 12 protocol slots negotiated by the Session Configuration Protocol.
///
/// Slot ordering follows C.S0024-400 §1.5 (the AT/AN protocol stack).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NegotiatedProtocols {
    pub stream: ProtocolSubtype,
    pub signaling: ProtocolSubtype,
    pub connection: ProtocolSubtype,
    pub session_management: ProtocolSubtype,
    pub idle_state: ProtocolSubtype,
    pub connected_state: ProtocolSubtype,
    pub route_update: ProtocolSubtype,
    pub air_link_management: ProtocolSubtype,
    pub security: ProtocolSubtype,
    pub authentication: ProtocolSubtype,
    pub encryption: ProtocolSubtype,
    pub key_exchange: ProtocolSubtype,
}

impl NegotiatedProtocols {
    /// Returns each slot as a `(name, subtype)` pair in protocol-stack order.
    /// Useful for diagnostics and verifying slot count.
    pub fn slots(&self) -> [(&'static str, ProtocolSubtype); 12] {
        [
            ("stream", self.stream),
            ("signaling", self.signaling),
            ("connection", self.connection),
            ("session_management", self.session_management),
            ("idle_state", self.idle_state),
            ("connected_state", self.connected_state),
            ("route_update", self.route_update),
            ("air_link_management", self.air_link_management),
            ("security", self.security),
            ("authentication", self.authentication),
            ("encryption", self.encryption),
            ("key_exchange", self.key_exchange),
        ]
    }
}

/// Rev 0 baseline: every slot pinned to its Default subtype.
pub const REV0_DEFAULTS: NegotiatedProtocols = NegotiatedProtocols {
    stream: ProtocolSubtype::Default,
    signaling: ProtocolSubtype::Default,
    connection: ProtocolSubtype::Default,
    session_management: ProtocolSubtype::Default,
    idle_state: ProtocolSubtype::Default,
    connected_state: ProtocolSubtype::Default,
    route_update: ProtocolSubtype::Default,
    air_link_management: ProtocolSubtype::Default,
    security: ProtocolSubtype::Default,
    authentication: ProtocolSubtype::Default,
    encryption: ProtocolSubtype::Default,
    key_exchange: ProtocolSubtype::Default,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev0_defaults_has_twelve_slots_all_default() {
        let slots = REV0_DEFAULTS.slots();
        assert_eq!(slots.len(), 12);
        for (_, subtype) in slots {
            assert_eq!(subtype, ProtocolSubtype::Default);
        }
    }

    #[test]
    fn slot_names_are_unique() {
        let slots = REV0_DEFAULTS.slots();
        let mut names: Vec<_> = slots.iter().map(|(n, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 12);
    }
}
