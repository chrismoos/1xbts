pub mod message_types;
pub mod paging_messages;

use crate::{bits::Bitstream, mac::ChannelType, time::CdmaSystemTime};
use message_types::MessageId;
use paging_messages::MsAddress;

/// Opaque service data unit type: a raw bitstream payload.
pub type Sdu = Bitstream;

/// Control and status metadata for a directed or broadcast LAC PDU.
///
/// Carries addressing, ARQ fields, and scheduling hints used by the LAC
/// sublayer to sequence, retransmit, and prioritize PDUs on the F-PCH and
/// F-TCH/F-DSCH channels.
#[derive(Debug, Clone)]
pub struct MessageControlStatusBlock {
    pub channel: ChannelType,
    pub mobile_p_rev: Option<u8>,
    pub extended_encryption: bool,
    pub message_id: MessageId,
    pub length_bits: usize,
    pub requested_tx_time: Option<CdmaSystemTime>,
    /// Soft deadline for prioritizing urgent directed PDUs such as
    /// acknowledgments to recent access attempts.
    pub tx_deadline: Option<CdmaSystemTime>,
    /// Forward-link address for directed PDUs (None = broadcast).
    pub address: Option<MsAddress>,
    /// ARQ acknowledgment sequence number (3 bits).
    pub ack_seq: u8,
    /// ARQ message sequence number (3 bits).
    pub msg_seq: u8,
    /// Whether the MS should ACK this PDU.
    pub ack_req: bool,
    /// Whether this PDU carries an ACK for the MS.
    pub valid_ack: bool,
    /// Overhead MCC for IMSI class-0 OTA address compression at encoding time.
    pub overhead_mcc: u16,
    /// Overhead IMSI_11_12 for IMSI class-0 OTA address compression at encoding time.
    pub overhead_imsi_11_12: u8,
}

/// A request from the LAC sublayer to transmit one SDU.
#[derive(Debug, Clone)]
pub struct DataRequest {
    pub sdu: Sdu,
    pub mcsb: MessageControlStatusBlock,
}
