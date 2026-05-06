//! Proprietary UDP bearer envelope for Abis traffic payloads.
//!
//! A.S0003-A mandates ATM/AAL2 as the transport for Abis traffic sub-channels,
//! where each FCH/SCH/DCCH bearer gets its own AAL2 CID within a negotiated VCC
//! — the circuit itself provides routing and ordering. No UDP framing is defined
//! by the spec (§4.5.6.2 lists UDP as an optional alternative with no datagram
//! format). This module defines a custom envelope that substitutes for ATM
//! VCC-per-bearer routing when all bearers share a single UDP socket:
//!
//! - `bts_id` / `cell_id`: replaces ATM VPI/VCI identification of the BTS/sector
//! - `bearer_id`: Walsh code — replaces AAL2 CID identifying the traffic sub-channel
//! - `sequence_no`: replaces AAL2 sequence-sensitive delivery ordering/dedup
//!
//! This framing is not interoperable with any other Abis implementation.

use std::collections::{BTreeMap, VecDeque};

use crate::bearer::{ChannelFamily, Direction};
use crate::{Error, Result};

/// Current UDP bearer wrapper version.
pub const VERSION: u8 = 1;
/// Fixed UDP bearer wrapper header length in octets.
pub const HEADER_LEN: usize = 28;
const RESERVED_LEN: usize = 4;
const DEFAULT_DUPLICATE_HISTORY: usize = 32;

/// Transport envelope for a single Abis bearer payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpBearerDatagram {
    pub flags: u8,
    pub channel_family: ChannelFamily,
    pub direction: Direction,
    pub bts_id: u32,
    pub cell_id: u32,
    pub bearer_id: u32,
    pub sequence_no: u32,
    pub tx_frame_number: u32,
    pub payload: Vec<u8>,
}

impl UdpBearerDatagram {
    /// Returns the bearer-routing key implied by the datagram header fields.
    pub fn route_key(&self) -> BearerRouteKey {
        BearerRouteKey {
            channel_family: self.channel_family,
            direction: self.direction,
            bts_id: self.bts_id,
            cell_id: self.cell_id,
            bearer_id: self.bearer_id,
        }
    }

    /// Encodes the version-1 UDP bearer wrapper.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(VERSION);
        out.push(self.flags);
        out.push(encode_family(self.channel_family));
        out.push(encode_direction(self.direction));
        out.extend_from_slice(&self.bts_id.to_be_bytes());
        out.extend_from_slice(&self.cell_id.to_be_bytes());
        out.extend_from_slice(&self.bearer_id.to_be_bytes());
        out.extend_from_slice(&self.sequence_no.to_be_bytes());
        out.extend_from_slice(&self.tx_frame_number.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Decodes the version-1 UDP bearer wrapper.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < HEADER_LEN {
            return Err(Error::Truncated {
                context: "Abis UDP bearer header",
                needed: HEADER_LEN,
                actual: input.len(),
            });
        }
        if input[0] != VERSION {
            return Err(Error::ReservedValue {
                context: "Abis UDP bearer version",
                value: input[0],
            });
        }
        if input[24..28] != [0; RESERVED_LEN] {
            return Err(Error::InvalidValue {
                context: "Abis UDP bearer header",
                reason: "reserved header octets must be zero",
            });
        }
        Ok(Self {
            flags: input[1],
            channel_family: decode_family(input[2])?,
            direction: decode_direction(input[3])?,
            bts_id: u32::from_be_bytes(input[4..8].try_into().unwrap()),
            cell_id: u32::from_be_bytes(input[8..12].try_into().unwrap()),
            bearer_id: u32::from_be_bytes(input[12..16].try_into().unwrap()),
            sequence_no: u32::from_be_bytes(input[16..20].try_into().unwrap()),
            tx_frame_number: u32::from_be_bytes(input[20..24].try_into().unwrap()),
            payload: input[HEADER_LEN..].to_vec(),
        })
    }
}

/// Stable routing key for a bearer stream inside the UDP wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BearerRouteKey {
    /// Encoded Abis bearer family.
    pub channel_family: ChannelFamily,
    /// Encoded bearer direction.
    pub direction: Direction,
    /// Logical BTS identifier assigned by the local deployment.
    pub bts_id: u32,
    /// Logical cell identifier assigned by the local deployment.
    pub cell_id: u32,
    /// Logical bearer identifier assigned by the local deployment.
    pub bearer_id: u32,
}

/// Disposition assigned to a received UDP bearer datagram after routing and sequence checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpBearerRouteOutcome {
    /// The datagram belongs to a known bearer and advances the accepted sequence stream.
    Accepted,
    /// The datagram sequence number was already accepted for this bearer.
    DuplicateDrop,
    /// The datagram sequence number is older than the most recently accepted sequence.
    LateDrop,
}

/// A routed bearer datagram paired with its route key and receive disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedBearerDatagram {
    /// The bearer route selected from the UDP wrapper header.
    pub key: BearerRouteKey,
    /// The received datagram.
    pub datagram: UdpBearerDatagram,
    /// The sequence/routing disposition.
    pub outcome: UdpBearerRouteOutcome,
}

/// Per-route receive counters maintained by [`UdpBearerRouter`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpBearerRouteCounters {
    /// Accepted in-order or wrap-forward datagrams.
    pub accepted: u64,
    /// Datagrams dropped because their sequence number was already seen.
    pub duplicate_drop: u64,
    /// Datagrams dropped because their sequence number was older than the current receive point.
    pub late_drop: u64,
}

/// Crate-local receive router for project-defined Abis UDP bearer streams.
///
/// The router does not perform socket I/O. It validates that a datagram belongs
/// to a registered bearer route and applies a serial-number receive policy that
/// accepts forward progress across 32-bit wrap, drops exact duplicates, and
/// drops late packets for already-established routes.
#[derive(Debug, Clone)]
pub struct UdpBearerRouter {
    duplicate_history: usize,
    routes: BTreeMap<BearerRouteKey, SequenceWindow>,
}

impl Default for UdpBearerRouter {
    fn default() -> Self {
        Self::new(DEFAULT_DUPLICATE_HISTORY).expect("default duplicate history is valid")
    }
}

impl UdpBearerRouter {
    /// Creates a router with the given accepted-sequence history depth per bearer.
    pub fn new(duplicate_history: usize) -> Result<Self> {
        if duplicate_history == 0 {
            return Err(Error::InvalidValue {
                context: "Abis UDP bearer router",
                reason: "duplicate history must be at least one packet",
            });
        }
        Ok(Self {
            duplicate_history,
            routes: BTreeMap::new(),
        })
    }

    /// Registers or resets tracking state for a bearer route.
    pub fn register_route(&mut self, key: BearerRouteKey) {
        self.routes
            .insert(key, SequenceWindow::new(self.duplicate_history));
    }

    /// Removes a bearer route and returns whether one was present.
    pub fn unregister_route(&mut self, key: BearerRouteKey) -> bool {
        self.routes.remove(&key).is_some()
    }

    /// Returns the current per-route receive counters, if the route is registered.
    pub fn counters(&self, key: BearerRouteKey) -> Option<UdpBearerRouteCounters> {
        self.routes.get(&key).map(|window| window.counters)
    }

    /// Routes a received datagram to its bearer stream and applies receive ordering checks.
    pub fn route(&mut self, datagram: UdpBearerDatagram) -> Result<RoutedBearerDatagram> {
        let key = datagram.route_key();
        let Some(window) = self.routes.get_mut(&key) else {
            return Err(Error::InvalidValue {
                context: "Abis UDP bearer route",
                reason: "datagram does not match a registered bearer",
            });
        };
        let outcome = window.observe(datagram.sequence_no);
        Ok(RoutedBearerDatagram {
            key,
            datagram,
            outcome,
        })
    }
}

#[derive(Debug, Clone)]
struct SequenceWindow {
    duplicate_history: usize,
    last_accepted: Option<u32>,
    recently_accepted: VecDeque<u32>,
    counters: UdpBearerRouteCounters,
}

impl SequenceWindow {
    fn new(duplicate_history: usize) -> Self {
        Self {
            duplicate_history,
            last_accepted: None,
            recently_accepted: VecDeque::with_capacity(duplicate_history),
            counters: UdpBearerRouteCounters::default(),
        }
    }

    fn observe(&mut self, sequence_no: u32) -> UdpBearerRouteOutcome {
        if self.recently_accepted.contains(&sequence_no) {
            self.counters.duplicate_drop += 1;
            return UdpBearerRouteOutcome::DuplicateDrop;
        }
        let outcome = match self.last_accepted {
            None => UdpBearerRouteOutcome::Accepted,
            Some(last_accepted) => match compare_sequence(last_accepted, sequence_no) {
                SequenceComparison::Newer => UdpBearerRouteOutcome::Accepted,
                SequenceComparison::Equal => UdpBearerRouteOutcome::DuplicateDrop,
                SequenceComparison::Older => UdpBearerRouteOutcome::LateDrop,
            },
        };
        if outcome == UdpBearerRouteOutcome::Accepted {
            self.last_accepted = Some(sequence_no);
            self.recently_accepted.push_back(sequence_no);
            self.counters.accepted += 1;
            while self.recently_accepted.len() > self.duplicate_history {
                self.recently_accepted.pop_front();
            }
        } else if outcome == UdpBearerRouteOutcome::LateDrop {
            self.counters.late_drop += 1;
        }
        outcome
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceComparison {
    Older,
    Equal,
    Newer,
}

fn compare_sequence(last_accepted: u32, candidate: u32) -> SequenceComparison {
    let delta = candidate.wrapping_sub(last_accepted);
    if delta == 0 {
        SequenceComparison::Equal
    } else if delta < 0x8000_0000 {
        SequenceComparison::Newer
    } else {
        SequenceComparison::Older
    }
}

fn encode_family(family: ChannelFamily) -> u8 {
    match family {
        ChannelFamily::Fch => 1,
        ChannelFamily::Sch => 2,
        ChannelFamily::Dcch => 3,
    }
}

fn decode_family(value: u8) -> Result<ChannelFamily> {
    match value {
        1 => Ok(ChannelFamily::Fch),
        2 => Ok(ChannelFamily::Sch),
        3 => Ok(ChannelFamily::Dcch),
        other => Err(Error::ReservedValue {
            context: "Abis UDP bearer channel_family",
            value: other,
        }),
    }
}

fn encode_direction(direction: Direction) -> u8 {
    match direction {
        Direction::Forward => 1,
        Direction::Reverse => 2,
    }
}

fn decode_direction(value: u8) -> Result<Direction> {
    match value {
        1 => Ok(Direction::Forward),
        2 => Ok(Direction::Reverse),
        other => Err(Error::ReservedValue {
            context: "Abis UDP bearer direction",
            value: other,
        }),
    }
}
