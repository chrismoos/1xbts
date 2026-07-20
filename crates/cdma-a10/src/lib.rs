//! GRE/IP bearer primitives for the A10 user plane.
//!
//! This crate wraps the shared keyed-GRE bearer implementation from [`cdma_a8`] with the
//! A10-specific IPv4 endpoint shape and session terminology used between the PCF and PDSN.
//!
//! Implemented wire surface:
//! - keyed GRE session binding via the GRE `Key` field
//! - optional RFC 2890 GRE sequencing
//! - bearer-table ownership of endpoint and session bindings
//!
//! The current in-repo source text for `A.S0017` / `X.S0011-*` still leaves the exact
//! non-RFC GRE attribute bit layout unresolved for short-data indication, GRE segmentation,
//! and A10 flow-control/duration signaling. Those negotiated capabilities are therefore
//! retained in [`BearerProfile`] as session metadata only; this crate emits and accepts only
//! keyed GRE plus optional RFC 2890 sequencing on the wire.

use std::net::{IpAddr, Ipv4Addr};

pub use cdma_a8::{
    BearerProfile, BearerTransportConfig, BearerTransportMode, Error, GrePacket, GreProtocolType,
    RebindMode, Result, SequencingMode,
};

/// A10 bearer endpoint binding between the PCF and PDSN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerEndpoint {
    /// Local IPv4 address for the bearer socket.
    pub local_ipv4: [u8; 4],
    /// Remote IPv4 address expected on the bearer socket.
    pub remote_ipv4: [u8; 4],
}

impl BearerEndpoint {
    /// Creates a bearer endpoint binding.
    pub fn new(local_ipv4: [u8; 4], remote_ipv4: [u8; 4]) -> Self {
        Self {
            local_ipv4,
            remote_ipv4,
        }
    }
}

/// A10 session keyed by a local control-plane identifier and directional GRE keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerSession {
    /// Stable control-plane identifier used to manage the A10 bearer session locally.
    pub session_id: u32,
    /// GRE key the peer uses when sending traffic toward this bearer.
    pub inbound_session_key: u32,
    /// GRE key used by this bearer when sending traffic toward the peer.
    pub outbound_session_key: u32,
    /// Transport endpoint bound to this session.
    pub endpoint: BearerEndpoint,
    /// Wire-profile semantics negotiated for the bearer.
    pub profile: BearerProfile,
}

impl BearerSession {
    /// Creates an A10 bearer session description using a symmetric GRE key and the standard packet-data profile.
    pub fn new(session_key: u32, endpoint: BearerEndpoint) -> Self {
        Self {
            session_id: session_key,
            inbound_session_key: session_key,
            outbound_session_key: session_key,
            endpoint,
            profile: BearerProfile::standard_packet_data(),
        }
    }

    /// Creates an A10 bearer session description with a symmetric GRE key and explicit wire profile.
    pub fn with_profile(
        session_key: u32,
        endpoint: BearerEndpoint,
        profile: BearerProfile,
    ) -> Self {
        Self {
            session_id: session_key,
            inbound_session_key: session_key,
            outbound_session_key: session_key,
            endpoint,
            profile,
        }
    }

    /// Creates an A10 bearer session description with explicit control-plane identifier and directional GRE keys.
    pub fn with_directional_keys(
        session_id: u32,
        inbound_session_key: u32,
        outbound_session_key: u32,
        endpoint: BearerEndpoint,
        profile: BearerProfile,
    ) -> Self {
        Self {
            session_id,
            inbound_session_key,
            outbound_session_key,
            endpoint,
            profile,
        }
    }
}

/// A10 inbound user-plane packet after session validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundPacket {
    /// Control-plane session identifier resolved for the GRE packet.
    pub session_id: u32,
    /// GRE key extracted from the received GRE header.
    pub gre_key: u32,
    /// Endpoint the packet was accepted against.
    pub endpoint: BearerEndpoint,
    /// Monotonic receive ordinal assigned by the local bearer table.
    pub rx_ordinal: u64,
    /// GRE sequence number carried by the packet, if present.
    pub gre_sequence: Option<u32>,
    /// Decapsulated bearer payload.
    pub payload: Vec<u8>,
}

/// Outbound A10 packet resolved through the bearer table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundPacket {
    /// Control-plane session identifier used to resolve the packet.
    pub session_id: u32,
    /// GRE key encoded into the outbound GRE header.
    pub gre_key: u32,
    /// Endpoint selected from the installed bearer session.
    pub endpoint: BearerEndpoint,
    /// Monotonic transmit ordinal assigned by the local bearer table.
    pub tx_ordinal: u64,
    /// GRE sequence number assigned to the packet, if enabled for the session.
    pub gre_sequence: Option<u32>,
    /// Number of bearer payload bytes carried in `wire_bytes`.
    pub payload_len: usize,
    /// Serialized GRE packet ready to send on the bearer socket.
    pub wire_bytes: Vec<u8>,
}

/// A10 bearer statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BearerStats {
    /// Successfully encoded transmit packets.
    pub tx_packets: u64,
    /// Successfully encoded transmit payload bytes.
    pub tx_bytes: u64,
    /// Successfully accepted receive packets.
    pub rx_packets: u64,
    /// Successfully accepted receive payload bytes.
    pub rx_bytes: u64,
    /// GRE packets rejected before a session could be resolved.
    pub malformed_packets: u64,
    /// Packets carrying an unknown inbound GRE key.
    pub unknown_session_packets: u64,
    /// Packets dropped after a session lookup succeeded.
    pub dropped_packets: u64,
    /// Packets dropped specifically because the peer endpoint did not match.
    pub endpoint_mismatch_packets: u64,
    /// Receive packets that arrived with a duplicate GRE sequence number.
    pub duplicate_sequence_packets: u64,
    /// Receive packets that arrived behind the highest seen GRE sequence number.
    pub reordered_sequence_packets: u64,
    /// Receive packets that advanced the GRE sequence number by more than one.
    pub sequence_gap_events: u64,
    /// Successful control-plane session installs.
    pub sessions_created: u64,
    /// Successful control-plane endpoint and/or key changes on existing sessions.
    pub sessions_rebound: u64,
    /// Successful rebinds that used dormant-style immediate cutover.
    pub sessions_dormant_rebound: u64,
    /// Successful rebinds that installed mobility overlap state.
    pub sessions_mobility_rebound: u64,
    /// Successful rebinds that installed hard-handoff overlap state.
    pub sessions_hard_handoff_rebound: u64,
    /// Successful control-plane session removals.
    pub sessions_removed: u64,
    /// Successful transition completions that retired the previous endpoint.
    pub transitions_completed: u64,
    /// Number of currently installed bearer sessions.
    pub active_sessions: u64,
    /// Receive packets accepted on a draining endpoint during transition overlap.
    pub transition_rx_packets: u64,
}

/// Per-session counters retained with the A10 session binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStats {
    /// Successfully encoded transmit packets for the session.
    pub tx_packets: u64,
    /// Successfully encoded transmit payload bytes for the session.
    pub tx_bytes: u64,
    /// Successfully accepted receive packets for the session.
    pub rx_packets: u64,
    /// Successfully accepted receive payload bytes for the session.
    pub rx_bytes: u64,
    /// Packets dropped for this session because the endpoint binding mismatched.
    pub endpoint_mismatch_packets: u64,
    /// Total dropped packets attributed to this session.
    pub dropped_packets: u64,
    /// Receive packets accepted on the draining endpoint during a transition.
    pub transition_rx_packets: u64,
    /// Last transmit ordinal assigned by the local bearer table.
    pub last_tx_ordinal: u64,
    /// Last receive ordinal assigned by the local bearer table.
    pub last_rx_ordinal: u64,
    /// Last GRE sequence number transmitted for the session.
    pub last_tx_sequence: Option<u32>,
    /// Last GRE sequence number accepted for the session.
    pub last_rx_sequence: Option<u32>,
    /// Receive packets that arrived with a duplicate GRE sequence number.
    pub duplicate_sequence_packets: u64,
    /// Receive packets that arrived behind the highest seen GRE sequence number.
    pub reordered_sequence_packets: u64,
    /// Receive packets that advanced the GRE sequence number by more than one.
    pub sequence_gap_events: u64,
}

/// Snapshot of an A10 session and its counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// Installed bearer session binding.
    pub session: BearerSession,
    /// Counters accumulated while the session was installed.
    pub stats: SessionStats,
    /// Active transition state, if the session is being rebound across endpoints.
    pub transition: Option<SessionTransition>,
}

/// Outcome of applying a control-plane A10 session binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplySessionOutcome {
    /// A new session was installed.
    Created,
    /// The existing session already matched the requested endpoint, keys, and profile.
    Unchanged,
    /// The existing session was rebound to a new endpoint, keys, and/or profile.
    Rebound {
        /// Endpoint that was replaced by the control-plane update.
        previous_endpoint: BearerEndpoint,
        /// Inbound GRE key that was replaced by the control-plane update.
        previous_inbound_session_key: u32,
        /// Outbound GRE key that was replaced by the control-plane update.
        previous_outbound_session_key: u32,
        /// Session profile that was replaced by the control-plane update.
        previous_profile: BearerProfile,
    },
}

/// Active transition state retained while a session is moving between endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTransition {
    /// Transition mode governing overlap behavior.
    pub mode: RebindMode,
    /// Endpoint that is still temporarily accepted while the transition drains.
    pub previous_endpoint: BearerEndpoint,
}

/// Outcome of an explicit rebind request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebindOutcome {
    /// The current session binding already matched the requested endpoint.
    Unchanged,
    /// The session was rebound to the new endpoint.
    Rebound {
        /// Endpoint that was replaced by the control-plane update.
        previous_endpoint: BearerEndpoint,
        /// Transition semantics applied to the rebind.
        mode: RebindMode,
    },
}

/// A10 bearer session table wrapping the GRE/session binding rules.
#[derive(Debug, Default)]
pub struct BearerTable {
    inner: cdma_a8::BearerTable,
}

impl BearerTable {
    /// Creates an empty A10 bearer table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of currently installed A10 sessions.
    pub fn session_count(&self) -> usize {
        self.inner.session_count()
    }

    /// Returns `true` when the control-plane session identifier is installed.
    pub fn has_session(&self, session_id: u32) -> bool {
        self.inner.has_session(session_id)
    }

    /// Registers a new A10 bearer session.
    pub fn create_session(&mut self, session: BearerSession) -> Result<()> {
        self.inner.create_session(into_a8_session(session))
    }

    /// Applies a control-plane session binding to the bearer table.
    ///
    /// This performs immediate endpoint replacement. For mobility-aware overlap semantics, use
    /// [`Self::rebind_session_with_mode`]. If a transition is already active for the session,
    /// this method rejects the update with [`Error::TransitionInProgress`].
    pub fn apply_session(&mut self, session: BearerSession) -> Result<ApplySessionOutcome> {
        match self.inner.apply_session(into_a8_session(session))? {
            cdma_a8::ApplySessionOutcome::Created => Ok(ApplySessionOutcome::Created),
            cdma_a8::ApplySessionOutcome::Unchanged => Ok(ApplySessionOutcome::Unchanged),
            cdma_a8::ApplySessionOutcome::Rebound {
                previous_endpoint,
                previous_inbound_session_key,
                previous_outbound_session_key,
                previous_profile,
            } => Ok(ApplySessionOutcome::Rebound {
                previous_endpoint: from_a8_endpoint(previous_endpoint)?,
                previous_inbound_session_key,
                previous_outbound_session_key,
                previous_profile,
            }),
        }
    }

    /// Rebinds an existing A10 session using dormant-resume semantics.
    ///
    /// This performs an immediate endpoint cutover with no overlap window for the previous
    /// endpoint. For mobility-aware overlap, use [`Self::rebind_session_with_mode`].
    pub fn rebind_session(&mut self, session_id: u32, endpoint: BearerEndpoint) -> Result<()> {
        self.inner
            .rebind_session(session_id, into_a8_endpoint(endpoint))
    }

    /// Rebinds an existing A10 session with explicit transition semantics.
    pub fn rebind_session_with_mode(
        &mut self,
        session_id: u32,
        endpoint: BearerEndpoint,
        mode: RebindMode,
    ) -> Result<RebindOutcome> {
        match self
            .inner
            .rebind_session_with_mode(session_id, into_a8_endpoint(endpoint), mode)?
        {
            cdma_a8::RebindOutcome::Unchanged => Ok(RebindOutcome::Unchanged),
            cdma_a8::RebindOutcome::Rebound {
                previous_endpoint,
                mode,
            } => Ok(RebindOutcome::Rebound {
                previous_endpoint: from_a8_endpoint(previous_endpoint)?,
                mode,
            }),
        }
    }

    /// Finalizes an active transition and retires the draining endpoint.
    ///
    /// Returns `Ok(true)` when a transition was present and completed, `Ok(false)` when the
    /// session had no active transition, and `Err` when the session does not exist.
    pub fn finalize_rebind(&mut self, session_id: u32) -> Result<bool> {
        self.inner.finalize_rebind(session_id)
    }

    /// Removes a session from the table.
    pub fn remove_session(&mut self, session_id: u32) -> Result<BearerSession> {
        let session = self.inner.remove_session(session_id)?;
        from_a8_session(session)
    }

    /// Removes a session if it is present and returns the removed binding.
    pub fn remove_session_if_present(&mut self, session_id: u32) -> Option<BearerSession> {
        self.inner
            .remove_session_if_present(session_id)
            .and_then(|s| from_a8_session(s).ok())
    }

    /// Returns the registered session, if present.
    pub fn session(&self, session_id: u32) -> Option<BearerSession> {
        self.inner
            .session(session_id)
            .copied()
            .and_then(|s| from_a8_session(s).ok())
    }

    /// Returns the session binding and per-session counters, if present.
    pub fn session_snapshot(&self, session_id: u32) -> Option<SessionSnapshot> {
        self.inner
            .session_snapshot(session_id)
            .and_then(|snapshot| {
                Some(SessionSnapshot {
                    session: from_a8_session(snapshot.session).ok()?,
                    stats: SessionStats {
                        tx_packets: snapshot.stats.tx_packets,
                        tx_bytes: snapshot.stats.tx_bytes,
                        rx_packets: snapshot.stats.rx_packets,
                        rx_bytes: snapshot.stats.rx_bytes,
                        endpoint_mismatch_packets: snapshot.stats.endpoint_mismatch_packets,
                        dropped_packets: snapshot.stats.dropped_packets,
                        transition_rx_packets: snapshot.stats.transition_rx_packets,
                        last_tx_ordinal: snapshot.stats.last_tx_ordinal,
                        last_rx_ordinal: snapshot.stats.last_rx_ordinal,
                        last_tx_sequence: snapshot.stats.last_tx_sequence,
                        last_rx_sequence: snapshot.stats.last_rx_sequence,
                        duplicate_sequence_packets: snapshot.stats.duplicate_sequence_packets,
                        reordered_sequence_packets: snapshot.stats.reordered_sequence_packets,
                        sequence_gap_events: snapshot.stats.sequence_gap_events,
                    },
                    transition: snapshot.transition.and_then(|transition| {
                        Some(SessionTransition {
                            mode: transition.mode,
                            previous_endpoint: from_a8_endpoint(transition.previous_endpoint)
                                .ok()?,
                        })
                    }),
                })
            })
    }

    /// Returns a snapshot of accumulated bearer counters.
    pub fn stats(&self) -> BearerStats {
        let stats = self.inner.stats();
        BearerStats {
            tx_packets: stats.tx_packets,
            tx_bytes: stats.tx_bytes,
            rx_packets: stats.rx_packets,
            rx_bytes: stats.rx_bytes,
            malformed_packets: stats.malformed_packets,
            unknown_session_packets: stats.unknown_session_packets,
            dropped_packets: stats.dropped_packets,
            endpoint_mismatch_packets: stats.endpoint_mismatch_packets,
            duplicate_sequence_packets: stats.duplicate_sequence_packets,
            reordered_sequence_packets: stats.reordered_sequence_packets,
            sequence_gap_events: stats.sequence_gap_events,
            sessions_created: stats.sessions_created,
            sessions_rebound: stats.sessions_rebound,
            sessions_dormant_rebound: stats.sessions_dormant_rebound,
            sessions_mobility_rebound: stats.sessions_mobility_rebound,
            sessions_hard_handoff_rebound: stats.sessions_hard_handoff_rebound,
            sessions_removed: stats.sessions_removed,
            transitions_completed: stats.transitions_completed,
            active_sessions: stats.active_sessions,
            transition_rx_packets: stats.transition_rx_packets,
        }
    }

    /// Resolves an outbound payload through the session table and updates transmit counters.
    pub fn build_outbound_packet(
        &mut self,
        session_id: u32,
        payload: impl Into<Vec<u8>>,
    ) -> Result<OutboundPacket> {
        let packet = self.inner.build_outbound_packet(session_id, payload)?;
        Ok(OutboundPacket {
            session_id: packet.session_id,
            gre_key: packet.gre_key,
            endpoint: from_a8_endpoint(packet.endpoint)?,
            tx_ordinal: packet.tx_ordinal,
            gre_sequence: packet.gre_sequence,
            payload_len: packet.payload_len,
            wire_bytes: packet.wire_bytes,
        })
    }

    /// Encodes a payload for a known A10 session and updates transmit counters.
    pub fn encode_for_session(
        &mut self,
        session_id: u32,
        payload: impl Into<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        self.build_outbound_packet(session_id, payload)
            .map(|packet| packet.wire_bytes)
    }

    /// Decodes and validates a received A10 GRE packet.
    pub fn decode_for_session(
        &mut self,
        endpoint: BearerEndpoint,
        input: &[u8],
    ) -> Result<InboundPacket> {
        let packet = self
            .inner
            .decode_for_session(into_a8_endpoint(endpoint), input)?;
        Ok(InboundPacket {
            session_id: packet.session_id,
            gre_key: packet.gre_key,
            endpoint: from_a8_endpoint(packet.endpoint)?,
            rx_ordinal: packet.rx_ordinal,
            gre_sequence: packet.gre_sequence,
            payload: packet.payload,
        })
    }
}

fn into_a8_endpoint(endpoint: BearerEndpoint) -> cdma_a8::BearerEndpoint {
    cdma_a8::BearerEndpoint::from_ip(
        IpAddr::V4(Ipv4Addr::from(endpoint.local_ipv4)),
        IpAddr::V4(Ipv4Addr::from(endpoint.remote_ipv4)),
    )
}

fn from_a8_endpoint(endpoint: cdma_a8::BearerEndpoint) -> Result<BearerEndpoint> {
    let IpAddr::V4(local_ip) = endpoint.local_ip else {
        return Err(Error::AddressFamilyMismatch { session_id: 0 });
    };
    let IpAddr::V4(remote_ip) = endpoint.remote_ip else {
        return Err(Error::AddressFamilyMismatch { session_id: 0 });
    };
    Ok(BearerEndpoint::new(local_ip.octets(), remote_ip.octets()))
}

fn into_a8_session(session: BearerSession) -> cdma_a8::BearerSession {
    cdma_a8::BearerSession::with_directional_keys(
        session.session_id,
        session.inbound_session_key,
        session.outbound_session_key,
        into_a8_endpoint(session.endpoint),
        session.profile,
    )
}

fn from_a8_session(session: cdma_a8::BearerSession) -> Result<BearerSession> {
    Ok(BearerSession {
        session_id: session.session_id,
        inbound_session_key: session.inbound_session_key,
        outbound_session_key: session.outbound_session_key,
        endpoint: from_a8_endpoint(session.endpoint)?,
        profile: session.profile,
    })
}
