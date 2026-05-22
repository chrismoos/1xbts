use std::time::Instant;

use crate::access::{AccessMessage, RdschPdu};
use crate::lac::message_types::MessageId;
use crate::time::CdmaSystemTime;

/// Decoded access channel event surfaced from the RX pipeline to higher layers.
#[derive(Debug, Clone)]
pub struct AccessChannelEvent {
    /// Backend-stamped event ID assigned when the event is created.
    pub event_id: String,
    /// Chip position of the access message.
    pub chip_start: usize,
    /// Absolute chip position of the access message, if known.
    pub absolute_chip_start: Option<u64>,
    /// CDMA system time corresponding to `absolute_chip_start`, if known.
    pub receive_time: Option<CdmaSystemTime>,
    /// Number of preamble frames preceding the message body.
    pub preamble_frames: i64,
    /// Protocol discriminator (PD field from the PDU header).
    pub pd: u8,
    /// Decoded message identity.
    pub message_id: MessageId,
    /// Human-readable message type name (e.g. "Registration Message").
    pub msg_type_name: String,
    /// Addressing summary from the LAC PDU (IMSI/ESN/MEID), if available.
    pub address: Option<String>,
    /// Canonical forward-link address for the resolved mobile, if known.
    /// Unlike `address`, this is stamped by the BSC after mobile matching and
    /// is shared across access and reverse-traffic events.
    pub resolved_address: Option<String>,
    /// HLR subscriber ID for the resolved mobile, if known.
    pub subscriber_id: Option<String>,
    /// Layer-3 SDU one-line summary (decoded message fields).
    pub l3_summary: Option<String>,
    /// Structured Layer-3 access message, if decoding succeeded.
    pub decoded_l3: Option<AccessMessage>,
    /// Full LAC PDU one-line summary (ARQ, addressing, auth, RER, SDU).
    pub pdu_summary: String,
    /// ARQ msg_seq from the access probe (for ACK piggybacking).
    pub msg_seq: Option<u8>,
    /// ARQ ack_seq from the access probe.
    pub ack_seq: Option<u8>,
    /// Whether the mobile requests an acknowledgment (ARQ ack_req).
    pub ack_req: bool,
    /// Whether the mobile's ACK is valid (ARQ valid_ack).
    pub valid_ack: bool,
    /// MSID type from LAC addressing fields.
    pub msid_type: Option<u8>,
    /// Decoded ESN, if present in the addressing fields.
    pub esn: Option<u32>,
    /// Full decoded IMSI string, if the access identity plus overhead context
    /// provide all digits.
    pub imsi: Option<String>,
    /// Decoded MEID, if present in extended MSID addressing fields.
    pub meid: Option<String>,
    /// Decoded IMSI_M_S1, if present in the addressing fields.
    pub imsi_m_s1: Option<u32>,
    /// Decoded IMSI_M_S2, if present in the addressing fields.
    pub imsi_m_s2: Option<u16>,
    /// Decoded IMSI class from full-IMSI addressing fields.
    pub imsi_class: Option<u8>,
    /// Number of IMSI_O address digits for class-1 IMSI, if present.
    pub imsi_addr_num: Option<u8>,
    /// Encoded MCC_M/MCC_T field from the IMSI, if present in the addressing fields.
    pub imsi_mcc: Option<u16>,
    /// Encoded IMSI_M_11_12/IMSI_T_11_12 field from the IMSI, if present.
    pub imsi_11_12: Option<u8>,
    /// MOB_P_REV from the L3 message (registration/page response).
    pub mob_p_rev: Option<u8>,
    /// SLOT_CYCLE_INDEX from the L3 message (registration/origination/page response).
    pub slot_cycle_index: Option<u8>,
    /// SCM (Station Class Mark) from the L3 message.
    pub scm: Option<u8>,
    /// Wall-clock UTC microseconds since epoch, stamped at RX event creation.
    pub wall_clock_us: u64,
    /// Wall-clock instant when the RX pipeline produced this event (for latency tracing).
    pub rx_wall_time: Option<Instant>,
    /// Hardware time (ns) of the RX batch that produced this event.
    pub rx_hw_time_ns: Option<u64>,
    /// Finger detection SNR in dB (10*log10 of correlation peak / noise floor).
    pub snr_db: Option<f32>,
    /// Mean despread chip power in dB (relative to full-scale).
    pub signal_power_db: Option<f32>,
    /// Estimated reverse pilot Ec/Io in dB from the validated traffic finger.
    pub reverse_pilot_ec_io_db: Option<f32>,
    /// Raw input power in dB (pre-despread, relative to full-scale ADC).
    pub raw_power_db: Option<f32>,
    /// Demod quality percentage: 100 = all soft decisions confident, 0 = all weak.
    pub demod_quality_pct: Option<f32>,
    /// Per-PCG reverse-link quality estimate in dB (16 entries, one per PCG).
    pub pcg_signal_snr_db: Option<Vec<f32>>,
    /// Active-PCG mask for a reverse traffic frame.
    pub active_pcg_mask: Option<[bool; 16]>,
    /// Traffic frame PHY-valid flag from the reverse traffic frame aligner.
    pub traffic_phy_valid: Option<bool>,
    /// Traffic frame FQI/CRC-valid flag from the reverse traffic frame aligner.
    pub traffic_fqi_valid: Option<bool>,
    /// Traffic frame encoder-tail-valid flag from the reverse traffic frame aligner.
    pub traffic_tail_valid: Option<bool>,
    /// Number of FQI bits carried by the decoded traffic frame rate.
    pub traffic_fqi_bits: Option<u8>,
    /// True iff the RC1 reverse traffic frame aligner's unconstrained Viterbi ML
    /// best terminal state equals 0 (the encoder terminates via 8 zero tail bits).
    pub traffic_ml_tail_match: Option<bool>,
    /// Data Burst burst_type field (6 bits). 3 = SMS per C.S0015-B.
    pub burst_type: Option<u8>,
    /// Data Burst CHARi payload bytes (Transport Layer message for SMS).
    pub data_burst_fields: Option<Vec<u8>>,
    /// Data Burst NUM_MSGS field.
    pub data_burst_num_msgs: Option<u8>,
    /// Data Burst MSG_NUMBER field.
    pub data_burst_msg_number: Option<u8>,
    /// Order code from Order Message (6 bits), if this is an Order message.
    pub order_code: Option<u8>,
    /// Service option from Origination or Page Response (e.g. 6 = SMS, 1 = voice).
    pub service_option: Option<u16>,
    /// Preferred forward Radio Configuration from Origination or Page Response, if reported.
    pub for_rc_pref: Option<u8>,
    /// Preferred reverse Radio Configuration from Origination or Page Response, if reported.
    pub rev_rc_pref: Option<u8>,
    /// Mobile requested reverse FCH eighth-rate gating from Origination or Page Response.
    pub rev_fch_gating_req: Option<bool>,
    /// Walsh code of the traffic channel, if this event came from reverse traffic.
    pub traffic_walsh_code: Option<u8>,
    /// True if this is a preamble-only acquisition event (no decoded frame yet).
    pub is_preamble_only: bool,
    /// True if this event carries a single per-PCG reverse-link power-control measurement.
    pub is_traffic_pcg_measurement: bool,
    /// True if this event carries reverse traffic PHY frame validity only.
    pub is_traffic_phy_status: bool,
    /// Age of a per-PCG traffic measurement, in reverse-link chips.
    pub traffic_measurement_age_chips: Option<u64>,
    /// Forward Radio Configurations supported by the mobile.
    pub for_supported_rcs: Vec<u8>,
    /// Reverse Radio Configurations supported by the mobile.
    pub rev_supported_rcs: Vec<u8>,
    /// Decoded r-dsch PDU for traffic channel events.
    pub decoded_rdsch: Option<RdschPdu>,
    /// Reverse traffic primary payload bits for non-signaling frames.
    pub traffic_primary_bits: Option<Vec<u8>>,
    /// Rate of `traffic_primary_bits` in bps (9600/4800/2400/1200).
    pub traffic_primary_rate_bps: Option<u32>,
    /// True when BTS RX already emitted this primary frame on the reverse Abis bearer.
    pub traffic_primary_bearer_routed: bool,
    /// Reverse traffic voice/info bits for primary-traffic frames.
    pub traffic_voice_bits: Option<Vec<u8>>,
    /// Rate of `traffic_voice_bits` in bps (9600/4800/2400/1200).
    pub traffic_voice_rate_bps: Option<u32>,
    /// Raw access channel PDU bits as received from the air interface.
    pub raw_pdu_bits: Option<Vec<u8>>,
}
