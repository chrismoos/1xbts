//! CDMA2000 Reverse Common Signaling Channel Layer 3 decoder.
//!
//! This module decodes a Layer-3 SDU after the surrounding LAC PDU has already
//! been stripped.
//!
//! Note: the raw reassembled Access Channel payload emitted by `AccessFrameReader`
//! is still a full r-csch LAC PDU. It begins with PD(2) | MSG_ID(6), but on
//! modern revisions the PDU also carries ARQ, addressing, authentication /
//! message integrity, extended-encryption, and radio-environment-report fields
//! ahead of the Layer-3 SDU. Feeding that raw PDU directly into this decoder is
//! therefore not faithful beyond the message-type header.

use crate::bits::Bitstream;
use log::info;

use crate::lac::message_types::{MessageId, WireChannel};

/// Reverse access-channel Layer 3 decode context.
///
/// Some messages, notably Page Response, carry optional tail fields whose
/// presence depends on system state such as `P_REV_IN_USEs` and `AUTH_MODE`,
/// not just the bits carried in the message itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccessDecodeContext {
    pub auth_mode: Option<u8>,
    pub p_rev_in_use: Option<u8>,
}

impl AccessDecodeContext {
    /// Build a decode context from the serving system state.
    pub const fn new(auth_mode: Option<u8>, p_rev_in_use: Option<u8>) -> Self {
        Self {
            auth_mode,
            p_rev_in_use,
        }
    }
}

pub fn access_message_type_name(raw: u8) -> &'static str {
    MessageId::from_wire(WireChannel::ReverseCommon, raw).map_or("Unknown", |m| m.name())
}

#[derive(Debug, Clone)]
pub struct AccessMessageHeader {
    pub pd: u8,
    pub message_id: MessageId,
}

#[derive(Debug, Clone)]
pub struct RegistrationMessage {
    pub header: AccessMessageHeader,
    pub reg_type: u8,
    pub slot_cycle_index: u8,
    pub mob_p_rev: u8,
    pub scm: u8,
    pub mob_term: bool,
    pub return_cause: u8,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct OriginationMessage {
    pub header: AccessMessageHeader,
    pub mob_term: bool,
    pub slot_cycle_index: u8,
    pub mob_p_rev: u8,
    pub scm: u8,
    pub request_mode: u8,
    pub special_service: bool,
    pub service_option: Option<u16>,
    pub pm: bool,
    pub digit_mode: bool,
    pub number_type: Option<u8>,
    pub number_plan: Option<u8>,
    pub more_fields: bool,
    pub num_fields: u8,
    pub digits: Vec<u8>,
    pub nar_an_cap: bool,
    pub paca_reorig: bool,
    pub return_cause: u8,
    pub more_records: bool,
    pub encryption_supported: Option<u8>,
    pub paca_supported: bool,
    pub num_alt_so: u8,
    pub alt_service_options: Vec<u16>,
    pub drs: Option<bool>,
    pub uzid_incl: Option<bool>,
    pub uzid: Option<u16>,
    pub ch_ind: Option<u8>,
    pub sr_id: Option<u8>,
    pub otd_supported: Option<bool>,
    pub qpch_supported: Option<bool>,
    pub enhanced_rc: Option<bool>,
    pub for_rc_pref: Option<u8>,
    pub rev_rc_pref: Option<u8>,
    pub fch_supported: Option<bool>,
    pub fch_capability: Option<FchTypeSpecificFields>,
    pub dcch_supported: Option<bool>,
    pub dcch_capability: Option<DcchTypeSpecificFields>,
    pub geo_loc_incl: Option<bool>,
    pub geo_loc_type: Option<u8>,
    pub rev_fch_gating_req: Option<bool>,
    pub orig_reason: Option<bool>,
    pub orig_count: Option<u8>,
    pub sts_supported: Option<bool>,
    pub cch_3x_supported: Option<bool>,
    pub wll_incl: Option<bool>,
    pub wll_device_type: Option<u8>,
    pub global_emergency_call: Option<bool>,
    pub ms_init_pos_loc_ind: Option<bool>,
    pub qos_parms_incl: Option<bool>,
    pub qos_parms_len: Option<u8>,
    pub qos_parms: Vec<u8>,
    pub enc_info_incl: Option<bool>,
    pub sig_encrypt_sup: Option<u8>,
    pub d_sig_encrypt_req: Option<bool>,
    pub c_sig_encrypt_req: Option<bool>,
    pub new_sseq_h: Option<u32>,
    pub new_sseq_h_sig: Option<u8>,
    pub ui_encrypt_req: Option<bool>,
    pub ui_encrypt_sup: Option<u8>,
    pub sync_id_incl: Option<bool>,
    pub sync_id_len: Option<u8>,
    pub sync_id: Option<u32>,
    pub prev_sid_incl: Option<bool>,
    pub prev_sid: Option<u16>,
    pub prev_nid_incl: Option<bool>,
    pub prev_nid: Option<u16>,
    pub prev_pzid_incl: Option<bool>,
    pub prev_pzid: Option<u8>,
    pub so_bitmap_ind: Option<u8>,
    pub so_group_num: Option<u8>,
    pub so_bitmap: Option<u16>,
    pub sdb_desired_only: Option<bool>,
    pub alt_band_class_sup: Option<bool>,
    pub msg_int_info_incl: Option<bool>,
    pub sig_integrity_sup_incl: Option<bool>,
    pub sig_integrity_sup: Option<u8>,
    pub sig_integrity_req: Option<u8>,
    pub new_key_id: Option<u8>,
    pub new_sseq_h_incl: Option<bool>,
    pub for_pdch_supported: Option<bool>,
    pub for_pdch_capability: Option<ForPdchTypeSpecificFields>,
    pub ext_ch_ind: Option<u8>,
    pub sign_slot_cycle_index: Option<bool>,
    pub add_serv_instance_incl: Option<bool>,
    pub add_service_instances: Vec<OriginationAdditionalServiceInstance>,
    pub bcmc_incl: Option<bool>,
    pub bcmc: Option<OriginationBcmcFields>,
    pub rev_pdch_supported: Option<bool>,
    pub rev_pdch_capability: Option<RevPdchTypeSpecificFields>,
    pub band_sub_rep_incl: Option<bool>,
    pub num_band_subclass: Option<u8>,
    pub band_subclass_sup: Vec<u8>,
    pub add_geo_loc_incl: Option<bool>,
    pub add_geo_loc_type_len_ind: Option<bool>,
    pub add_geo_loc_type: Option<u32>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct FchTypeSpecificFields {
    pub frame_size_5ms_supported: bool,
    pub for_fch_len: u8,
    pub for_fch_rc_map_raw: Bitstream,
    pub for_supported_rcs: Vec<u8>,
    pub rev_fch_len: u8,
    pub rev_fch_rc_map_raw: Bitstream,
    pub rev_supported_rcs: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct DcchTypeSpecificFields {
    pub frame_size_mode: u8,
    pub for_dcch_len: u8,
    pub for_dcch_rc_map_raw: Bitstream,
    pub for_supported_rcs: Vec<u8>,
    pub rev_dcch_len: u8,
    pub rev_dcch_rc_map_raw: Bitstream,
    pub rev_supported_rcs: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ForPdchTypeSpecificFields {
    pub ack_delay: bool,
    pub num_arq_chan: u8,
    pub for_pdch_len: u8,
    pub for_pdch_rc_map_raw: Bitstream,
    pub for_pdch_supported_rcs: Vec<u8>,
    pub ch_config_sup_map_len: u8,
    pub ch_config_sup_map_raw: Bitstream,
    pub ch_config_supported: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RevPdchTypeSpecificFields {
    pub rev_pdch_len: u8,
    pub rev_pdch_rc_map_raw: Bitstream,
    pub rev_pdch_supported_rcs: Vec<u8>,
    pub rev_pdch_ch_config_sup_map_len: u8,
    pub rev_pdch_ch_config_sup_map_raw: Bitstream,
    pub rev_pdch_ch_config_supported: Vec<u8>,
    pub rev_pdch_max_size_supported_encoder_packet: u8,
}

#[derive(Debug, Clone)]
pub struct OriginationAdditionalServiceInstance {
    pub add_sr_id: u8,
    pub add_drs: bool,
    pub add_service_option_incl: Option<bool>,
    pub add_service_option: Option<u16>,
    pub add_qos_parms_incl: Option<bool>,
    pub add_qos_parms_len: Option<u8>,
    pub add_qos_parms: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FundicatedBcmcTypeSpecificFields {
    pub fundicated_bcmc_ch_sup_map_len: u8,
    pub fundicated_bcmc_ch_sup_map_raw: Bitstream,
    pub supported_configurations: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct OriginationBcmcFields {
    pub bcmc_orig_only_ind: bool,
    pub fundicated_bcmc_supported: bool,
    pub fundicated_bcmc_capability: Option<FundicatedBcmcTypeSpecificFields>,
    pub auth_signature_incl: bool,
    pub time_stamp_short_length: Option<u8>,
    pub time_stamp_short: Bitstream,
    pub num_bcmc_programs: u8,
    pub programs: Vec<OriginationBcmcProgram>,
}

#[derive(Debug, Clone)]
pub struct OriginationBcmcProgram {
    pub bcmc_program_id_len: u8,
    pub bcmc_program_id: Bitstream,
    pub bcmc_flow_discriminator_len: u8,
    pub num_flow_discriminator: Option<u32>,
    pub flows: Vec<OriginationBcmcFlow>,
}

#[derive(Debug, Clone)]
pub struct OriginationBcmcFlow {
    pub bcmc_flow_discriminator: Bitstream,
    pub bcmc_pref: Option<bool>,
    pub auth_signature_ind: Option<bool>,
    pub auth_signature_same_ind: Option<bool>,
    pub bak_id: Option<u8>,
    pub auth_signature: Option<u32>,
}

/// Reverse Page Response Message (PRM). C.S0005-E 2.7.1.3.2.5.
#[derive(Debug, Clone)]
pub struct PageResponseMessage {
    /// Shared reverse-common-signaling header.
    pub header: AccessMessageHeader,
    /// Mobile-terminated indicator.
    pub mob_term: bool,
    /// Slot cycle index selected by the mobile.
    pub slot_cycle_index: u8,
    /// Mobile protocol revision carried by the message.
    pub mob_p_rev: u8,
    /// Station class mark reported by the mobile.
    pub scm: u8,
    /// Requested assignment mode.
    pub request_mode: u8,
    /// Requested service option.
    pub service_option: u16,
    /// Privacy mode indicator.
    pub pm: bool,
    /// Narrow analog capability indicator.
    pub nar_an_cap: bool,
    /// Legacy encryption-capability bitmap when that field is present.
    pub encryption_supported: Option<u8>,
    /// Number of alternate service options present in `alt_service_options`.
    pub num_alt_so: u8,
    /// Alternate service options reported by the mobile.
    pub alt_service_options: Vec<u16>,
    /// Whether the mobile included a User Zone Identifier.
    pub uzid_incl: Option<bool>,
    /// User Zone Identifier when `uzid_incl` is `Some(true)`.
    pub uzid: Option<u16>,
    /// Requested traffic-channel indicator.
    pub ch_ind: Option<u8>,
    /// One-touch data support indicator.
    pub otd_supported: Option<bool>,
    /// Quick Paging Channel support indicator.
    pub qpch_supported: Option<bool>,
    /// Enhanced radio-configuration support indicator.
    pub enhanced_rc: Option<bool>,
    /// Preferred forward radio configuration.
    pub for_rc_pref: Option<u8>,
    /// Preferred reverse radio configuration.
    pub rev_rc_pref: Option<u8>,
    /// Fundamental channel support indicator.
    pub fch_supported: Option<bool>,
    /// Fundamental-channel capability details when `fch_supported` is `Some(true)`.
    pub fch_capability: Option<FchTypeSpecificFields>,
    /// Dedicated control channel support indicator.
    pub dcch_supported: Option<bool>,
    /// Dedicated-control-channel capability details when `dcch_supported` is `Some(true)`.
    pub dcch_capability: Option<DcchTypeSpecificFields>,
    /// Requested reverse FCH eighth-rate gating mode.
    pub rev_fch_gating_req: Option<bool>,
    /// STS (Supplemental Traffic Subchannels) support indicator.
    pub sts_supported: Option<bool>,
    /// 3X Common Control Channel support indicator.
    pub cch_3x_supported: Option<bool>,
    /// Wireless Local Loop included indicator.
    pub wll_incl: Option<bool>,
    /// Wireless Local Loop device type.
    pub wll_device_type: Option<u8>,
    /// Hook status for WLL devices.
    pub hook_status: Option<u8>,
    /// Encryption information included indicator (p_rev_in_use >= 7).
    pub enc_info_incl: Option<bool>,
    /// Signaling encryption supported bitmap.
    pub sig_encrypt_sup: Option<u8>,
    /// Downlink signaling encryption request.
    pub d_sig_encrypt_req: Option<u8>,
    /// Common signaling encryption request.
    pub c_sig_encrypt_req: Option<u8>,
    /// Crypto-sync 24 MSB initializer (ENC_INFO path).
    pub new_sseq_h: Option<u32>,
    /// New SSD hash signature.
    pub new_sseq_h_sig: Option<u32>,
    /// User-information encryption request.
    pub ui_encrypt_req: Option<u8>,
    /// User-information encryption support.
    pub ui_encrypt_sup: Option<u8>,
    /// Sync ID included indicator.
    pub sync_id_incl: Option<bool>,
    /// Sync ID length.
    pub sync_id_len: Option<u8>,
    /// Sync ID value.
    pub sync_id: Option<u32>,
    /// Service option bitmap indicator.
    pub so_bitmap_ind: Option<u8>,
    /// Service option group number.
    pub so_group_num: Option<u8>,
    /// Service option bitmap.
    pub so_bitmap: Option<u16>,
    /// Alternate band class support indicator (p_rev_in_use >= 8).
    pub alt_band_class_sup: Option<bool>,
    /// Message integrity information included indicator (p_rev_in_use >= 9).
    pub msg_int_info_incl: Option<bool>,
    /// Signaling integrity support included indicator.
    pub sig_integrity_sup_incl: Option<bool>,
    /// Signaling integrity support bitmap.
    pub sig_integrity_sup: Option<u8>,
    /// Signaling integrity request bitmap.
    pub sig_integrity_req: Option<u8>,
    /// New key ID.
    pub new_key_id: Option<u8>,
    /// New SSEQ_H included indicator.
    pub new_sseq_h_incl: Option<bool>,
    /// Forward PDCH support indicator (p_rev_in_use >= 9).
    pub for_pdch_supported: Option<bool>,
    /// Forward PDCH capability details when `for_pdch_supported` is `Some(true)`.
    pub for_pdch_capability: Option<ForPdchTypeSpecificFields>,
    /// Extended channel indicator.
    pub ext_ch_ind: Option<u8>,
    /// Signed slot cycle index.
    pub sign_slot_cycle_index: Option<bool>,
    /// BCMC information included indicator.
    pub bcmc_incl: Option<bool>,
    /// BCMC preference included indicator.
    pub bcmc_pref_incl: Option<bool>,
    /// BCMC program/flow details.
    pub bcmc: Option<OriginationBcmcFields>,
    /// Reverse PDCH support indicator.
    pub rev_pdch_supported: Option<bool>,
    /// Reverse PDCH capability details when `rev_pdch_supported` is `Some(true)`.
    pub rev_pdch_capability: Option<RevPdchTypeSpecificFields>,
    /// Band subclass reporting included indicator.
    pub band_sub_rep_incl: Option<bool>,
    /// Number of band subclasses.
    pub num_band_subclass: Option<u8>,
    /// Band subclass support bitmask.
    pub band_subclass_sup: Option<Vec<u8>>,
    /// Bits left undecoded after the message body.
    pub remaining_bits: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessInfoRecord {
    pub record_type: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct StatusResponseMessage {
    pub header: AccessMessageHeader,
    pub qual_info_type: u8,
    pub qual_info: Vec<u8>,
    pub records: Vec<AccessInfoRecord>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct ExtendedStatusResponseMessage {
    pub header: AccessMessageHeader,
    pub qual_info_type: u8,
    pub qual_info: Vec<u8>,
    pub num_info_records: u8,
    pub records: Vec<AccessInfoRecord>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct DeviceInformationMessage {
    pub header: AccessMessageHeader,
    pub wll_device_type: u8,
    pub num_info_records: u8,
    pub records: Vec<AccessInfoRecord>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct SecurityModeRequestMessage {
    pub header: AccessMessageHeader,
    pub ui_encrypt_sup: Option<u8>,
    pub ui_encrypt_records: Vec<SecurityModeUiEncryptRecord>,
    pub sig_encrypt_sup: Option<u8>,
    pub c_sig_encrypt_req: Option<bool>,
    pub d_sig_encrypt_req: Option<bool>,
    pub new_sseq_h: Option<u32>,
    pub new_sseq_h_sig: Option<u8>,
    pub msg_int_info_incl: Option<bool>,
    pub sig_integrity_sup_incl: Option<bool>,
    pub sig_integrity_sup: Option<u8>,
    pub sig_integrity_req: Option<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct SecurityModeUiEncryptRecord {
    pub con_ref: u8,
    pub ui_encrypt_req: bool,
}

#[derive(Debug, Clone)]
pub struct ReconnectMessage {
    pub header: AccessMessageHeader,
    pub orig_ind: bool,
    pub sync_id_incl: bool,
    pub sync_id_len: Option<u8>,
    pub sync_id: Vec<u8>,
    pub service_option: Option<u16>,
    pub sr_id: Option<u8>,
    pub add_serv_instance_incl: Option<bool>,
    pub add_sr_ids: Vec<u8>,
    pub sdb_incl: Option<bool>,
    pub sdb_fields: Vec<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct RadioEnvironmentMessage {
    pub header: AccessMessageHeader,
    pub mode_disabled: bool,
    pub tkz_mode_ind: bool,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct AuthChallengeResponseMessage {
    pub header: AccessMessageHeader,
    pub authu: u32,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct AuthResponseMessage {
    pub header: AccessMessageHeader,
    pub res: Vec<u8>,
    pub sig_integrity_sup: Option<u8>,
    pub sig_integrity_req: Option<u8>,
    pub new_key_id: u8,
    pub new_sseq_h: u32,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct AuthResyncMessage {
    pub header: AccessMessageHeader,
    pub con_ms_sqn: Vec<u8>,
    pub mac_s: Vec<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct GeneralExtensionMessage {
    pub header: AccessMessageHeader,
    pub num_ge_records: u8,
    pub records: Vec<AccessInfoRecord>,
    pub message_type: u8,
    pub message_record: Bitstream,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct FlashWithInfoMessage {
    pub header: AccessMessageHeader,
    pub records: Vec<AccessInfoRecord>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct SendBurstDtmfMessage {
    pub header: AccessMessageHeader,
    pub digits: Vec<u8>,
    pub dtmf_on_length: u8,
    pub dtmf_off_length: u8,
    pub con_ref: Option<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub header: AccessMessageHeader,
    pub record: AccessInfoRecord,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct OriginationContinuationMessage {
    pub header: AccessMessageHeader,
    pub digit_mode: bool,
    pub digits: Vec<u8>,
    pub records: Vec<AccessInfoRecord>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct HandoffCompletionMessage {
    pub header: AccessMessageHeader,
    pub last_hdm_seq: u8,
    pub pilot_pns: Vec<u16>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct ParametersResponseRecord {
    pub parameter_id: u16,
    pub parameter_len: u16,
    pub parameter: Bitstream,
}

#[derive(Debug, Clone)]
pub struct ParametersResponseMessage {
    pub header: AccessMessageHeader,
    pub records: Vec<ParametersResponseRecord>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct ServiceOptionControlMessage {
    pub header: AccessMessageHeader,
    pub con_ref: u8,
    pub service_option: u16,
    pub control_record: Vec<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct SupplementalChannelPilotRecord {
    pub pilot_rec_type: u8,
    pub type_specific_fields: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SupplementalChannelPilotReport {
    pub pn_phase: u16,
    pub pilot_strength: u8,
    pub pilot_record: Option<SupplementalChannelPilotRecord>,
}

#[derive(Debug, Clone)]
pub struct SupplementalChannelRequestMeasurements {
    pub ref_pn: u16,
    pub pilot_strength: u8,
    pub active_pilots: Vec<SupplementalChannelPilotReport>,
    pub neighbor_pilots: Option<Vec<SupplementalChannelPilotReport>>,
    pub ref_pilot_record: Option<SupplementalChannelPilotRecord>,
}

#[derive(Debug, Clone)]
pub struct SupplementalChannelRequestMessage {
    pub header: AccessMessageHeader,
    pub req_blob: Vec<u8>,
    pub scrm_seq_num: Option<u8>,
    pub measurements: Option<SupplementalChannelRequestMeasurements>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct CandidateFreqSearchResponseMessage {
    pub header: AccessMessageHeader,
    pub last_cfsrm_seq: u8,
    pub total_off_time_fwd: u8,
    pub max_off_time_fwd: u8,
    pub total_off_time_rev: u8,
    pub max_off_time_rev: u8,
    pub pcg_off_times: bool,
    pub align_timing_used: bool,
    pub max_num_visits: Option<u8>,
    pub inter_visit_time: Option<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct CandidateFreqSearchReportPilot {
    pub pilot_pn_phase: u16,
    pub pilot_strength: u8,
    pub pilot_record: Option<SupplementalChannelPilotRecord>,
}

#[derive(Debug, Clone)]
pub struct CandidateFreqSearchCdmaPilots {
    pub band_class: u8,
    pub cdma_freq: u16,
    pub sf_total_rx_pwr: u8,
    pub cf_total_rx_pwr: u8,
    pub pilots: Vec<CandidateFreqSearchReportPilot>,
}

#[derive(Debug, Clone)]
pub enum CandidateFreqSearchReportModeSpecific {
    CdmaPilots(CandidateFreqSearchCdmaPilots),
    ExternalDsNeighbor(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct CandidateFreqSearchReportMessage {
    pub header: AccessMessageHeader,
    pub last_srch_msg: bool,
    pub last_srch_msg_seq: u8,
    pub search_mode: u8,
    pub mode_specific: CandidateFreqSearchReportModeSpecific,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct PeriodicPsmmPilot {
    pub pilot_pn_phase: u16,
    pub pilot_strength: u8,
    pub keep: bool,
    pub pilot_record: Option<SupplementalChannelPilotRecord>,
}

#[derive(Debug, Clone)]
pub struct PeriodicPsmmSchSetpoint {
    pub sch_id: u8,
    pub fpc_sch_curr_setpt: u8,
}

#[derive(Debug, Clone)]
pub struct PeriodicPsmmSetpoints {
    pub fpc_fch_curr_setpt: Option<u8>,
    pub fpc_dcch_curr_setpt: Option<u8>,
    pub sch_setpoints: Vec<PeriodicPsmmSchSetpoint>,
}

#[derive(Debug, Clone)]
pub struct PeriodicPsmmMessage {
    pub header: AccessMessageHeader,
    pub ref_pn: u16,
    pub pilot_strength: u8,
    pub keep: bool,
    pub sf_rx_pwr: u8,
    pub pilots: Vec<PeriodicPsmmPilot>,
    pub setpoints: Option<PeriodicPsmmSetpoints>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct OuterLoopReportMessage {
    pub header: AccessMessageHeader,
    pub fpc_fch_curr_setpt: Option<u8>,
    pub fpc_dcch_curr_setpt: Option<u8>,
    pub sch_setpoints: Vec<PeriodicPsmmSchSetpoint>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct ResourceRequestMessage {
    pub header: AccessMessageHeader,
    pub ch_ind: Option<u8>,
    pub ext_ch_ind: Option<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct ExtReleaseResponseMessage {
    pub header: AccessMessageHeader,
    pub rsc_mode_ind: bool,
    pub rsci: Option<u8>,
    pub rsc_end_time_unit: Option<u8>,
    pub rsc_end_time_value: Option<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone)]
pub struct NoFieldAccessMessage {
    pub header: AccessMessageHeader,
    pub remaining_bits: usize,
}

/// Reverse Order Message (r-csch). C.S0005-E 2.7.1.3.2.2.
/// On the access channel the message carries ORDER (6 bits) +
/// ADD_RECORD_LEN (3 bits) + order-specific fields (8 × ADD_RECORD_LEN).
/// Note: ORDQ is only present in r-dsch messages, not r-csch.
#[derive(Debug, Clone)]
pub struct OrderMessage {
    pub header: AccessMessageHeader,
    pub order: u8,
    pub add_record_len: u8,
    pub order_specific: Vec<u8>,
    pub remaining_bits: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileStationRejectOrderDetail {
    pub ordq: u8,
    pub rejected_type: u8,
    pub rejected_order: Option<u8>,
    pub rejected_ordq: Option<u8>,
    pub rejected_param_id: Option<u16>,
    pub rejected_record: Option<u8>,
    pub con_ref: Option<u8>,
    pub tag: Option<u8>,
    pub rejected_pdu_type: Option<u8>,
    pub trailing_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedSlotCycleOrderDetail {
    pub order: u8,
    pub ordq: u8,
    pub rsc_mode_ind: bool,
    pub rsci: Option<u8>,
    pub rsc_end_time_unit: Option<u8>,
    pub rsc_end_time_value: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverseOrderDetail {
    NoAdditionalFields { order: u8 },
    QualificationOnly { order: u8, ordq: u8 },
    BaseStationChallenge { randbs: u32 },
    ServiceOptionRequest { service_option: u16 },
    ServiceOptionResponse { service_option: u16 },
    MobileStationReject(MobileStationRejectOrderDetail),
    ReducedSlotCycle(ReducedSlotCycleOrderDetail),
}

impl OrderMessage {
    pub fn order_name(&self) -> &'static str {
        reverse_order_name(self.order)
    }

    pub fn reverse_detail(
        &self,
        forward_channel: WireChannel,
    ) -> Result<ReverseOrderDetail, String> {
        match self.order {
            0b000010 => self.parse_base_station_challenge_order(),
            0b010011 => self.parse_reverse_service_option_order(true),
            0b010100 => self.parse_reverse_service_option_order(false),
            0b011111 => self
                .parse_mobile_station_reject_order_strict(forward_channel)
                .map(ReverseOrderDetail::MobileStationReject),
            0b010101 if self.order_specific.first() == Some(&0x03) => {
                self.parse_reduced_slot_cycle_order(0x03)
            }
            0b100010 => {
                let ordq = *self
                    .order_specific
                    .first()
                    .ok_or("Fast Call Setup Order requires ORDQ")?;
                if !matches!(ordq, 0x00 | 0x01) {
                    return Err(format!("Fast Call Setup ORDQ 0x{ordq:02X} is reserved"));
                }
                self.parse_reduced_slot_cycle_order(ordq)
            }
            _ if self.order_specific.is_empty() => {
                Ok(ReverseOrderDetail::NoAdditionalFields { order: self.order })
            }
            _ if self.order_specific.len() == 1 => Ok(ReverseOrderDetail::QualificationOnly {
                order: self.order,
                ordq: self.order_specific[0],
            }),
            _ => Err(format!(
                "typed reverse Order detail is not implemented for ORDER=0b{:06b} ({}) with {} order-specific octets",
                self.order,
                self.order_name(),
                self.order_specific.len()
            )),
        }
    }

    pub fn from_reverse_detail(
        header: AccessMessageHeader,
        detail: &ReverseOrderDetail,
    ) -> Result<Self, String> {
        detail.to_order_message(header)
    }

    pub fn parse_mobile_station_reject_order(
        &self,
        forward_channel: WireChannel,
    ) -> Option<MobileStationRejectOrderDetail> {
        if self.order != 0b011111 || self.order_specific.len() < 2 {
            return None;
        }

        let mut consumed = 2usize;
        let mut rejected_order = None;
        let mut rejected_ordq = None;
        let mut rejected_param_id = None;
        let mut rejected_record = None;
        let mut con_ref = None;
        let mut tag = None;
        let mut rejected_pdu_type = None;

        let ordq = self.order_specific[0];
        let rejected_type = self.order_specific[1];

        if rejected_type == MessageId::Order.wire_type(forward_channel).unwrap()
            && self.order_specific.len() > consumed
        {
            rejected_order = Some(self.order_specific[consumed] & 0x3f);
            consumed += 1;
            if self.order_specific.len() > consumed {
                rejected_ordq = Some(self.order_specific[consumed]);
                consumed += 1;
            }
        }

        if forward_channel == WireChannel::ForwardDedicated
            && rejected_type == 0x0c
            && self.order_specific.len() >= consumed + 2
        {
            rejected_param_id = Some(
                ((self.order_specific[consumed] as u16) << 8)
                    | self.order_specific[consumed + 1] as u16,
            );
            consumed += 2;
        }

        let includes_rejected_record = match forward_channel {
            WireChannel::ForwardCommon => {
                rejected_type
                    == MessageId::FeatureNotification
                        .wire_type(forward_channel)
                        .unwrap_or(0xFF)
            }
            WireChannel::ForwardDedicated => matches!(rejected_type, 0x03 | 0x0e | 0x28 | 0x2a),
            _ => false,
        };
        if includes_rejected_record && self.order_specific.len() > consumed {
            rejected_record = Some(self.order_specific[consumed]);
            consumed += 1;
        }

        if matches!(ordq, 0x10 | 0x11 | 0x12) && self.order_specific.len() > consumed {
            con_ref = Some(self.order_specific[consumed]);
            consumed += 1;
        }

        if ordq == 0x13 && self.order_specific.len() > consumed {
            con_ref = Some(self.order_specific[consumed]);
            consumed += 1;
            if self.order_specific.len() > consumed {
                let packed = self.order_specific[consumed];
                tag = Some(packed >> 4);
                if packed & 0x0f != 0 {
                    rejected_pdu_type = Some((packed >> 2) & 0x03);
                }
                consumed += 1;
            }
        }

        Some(MobileStationRejectOrderDetail {
            ordq,
            rejected_type,
            rejected_order,
            rejected_ordq,
            rejected_param_id,
            rejected_record,
            con_ref,
            tag,
            rejected_pdu_type,
            trailing_bytes: self.order_specific[consumed..].to_vec(),
        })
    }

    pub fn parse_mobile_station_reject_order_strict(
        &self,
        forward_channel: WireChannel,
    ) -> Result<MobileStationRejectOrderDetail, String> {
        if self.order != 0b011111 {
            return Err("not a Mobile Station Reject Order".to_string());
        }
        if self.order_specific.len() < 2 {
            return Err("Mobile Station Reject Order requires ORDQ and REJECTED_TYPE".to_string());
        }

        let mut consumed = 2usize;
        let mut rejected_order = None;
        let mut rejected_ordq = None;
        let mut rejected_param_id = None;
        let mut rejected_record = None;
        let mut con_ref = None;
        let mut tag = None;
        let mut rejected_pdu_type = None;

        let ordq = self.order_specific[0];
        if !is_mobile_station_reject_ordq(ordq) {
            return Err(format!(
                "Mobile Station Reject ORDQ 0x{ordq:02X} is reserved"
            ));
        }
        let rejected_type = self.order_specific[1];

        if rejected_type == MessageId::Order.wire_type(forward_channel).unwrap() {
            let packed = *self
                .order_specific
                .get(consumed)
                .ok_or("Mobile Station Reject missing REJECTED_ORDER")?;
            if packed >> 6 != 0 {
                return Err("Mobile Station Reject RESERVED_1 bits must be zero".to_string());
            }
            rejected_order = Some(packed & 0x3f);
            consumed += 1;
            rejected_ordq = Some(
                *self
                    .order_specific
                    .get(consumed)
                    .ok_or("Mobile Station Reject missing REJECTED_ORDQ")?,
            );
            consumed += 1;
        }

        if forward_channel == WireChannel::ForwardDedicated && rejected_type == 0x0c {
            if self.order_specific.len() < consumed + 2 {
                return Err("Mobile Station Reject missing REJECTED_PARAM_ID".to_string());
            }
            rejected_param_id = Some(
                ((self.order_specific[consumed] as u16) << 8)
                    | self.order_specific[consumed + 1] as u16,
            );
            consumed += 2;
        }

        let includes_rejected_record = match forward_channel {
            WireChannel::ForwardCommon => {
                rejected_type
                    == MessageId::FeatureNotification
                        .wire_type(forward_channel)
                        .unwrap_or(0xFF)
            }
            WireChannel::ForwardDedicated => matches!(rejected_type, 0x03 | 0x0e | 0x28 | 0x2a),
            _ => false,
        };
        if includes_rejected_record {
            rejected_record = Some(
                *self
                    .order_specific
                    .get(consumed)
                    .ok_or("Mobile Station Reject missing REJECTED_RECORD")?,
            );
            consumed += 1;
        }

        if matches!(ordq, 0x10 | 0x11 | 0x12) {
            con_ref = Some(
                *self
                    .order_specific
                    .get(consumed)
                    .ok_or("Mobile Station Reject missing CON_REF")?,
            );
            consumed += 1;
        }

        if ordq == 0x13 {
            con_ref = Some(
                *self
                    .order_specific
                    .get(consumed)
                    .ok_or("Mobile Station Reject missing CON_REF")?,
            );
            consumed += 1;
            let packed = *self
                .order_specific
                .get(consumed)
                .ok_or("Mobile Station Reject missing TAG")?;
            tag = Some(packed >> 4);
            let pdu_and_reserved = packed & 0x0f;
            if pdu_and_reserved != 0 {
                if pdu_and_reserved & 0x03 != 0 {
                    return Err("Mobile Station Reject RESERVED_2 bits must be zero".to_string());
                }
                let pdu_type = (pdu_and_reserved >> 2) & 0x03;
                if pdu_type > 0b01 {
                    return Err(format!(
                        "Mobile Station Reject REJECTED_PDU_TYPE {pdu_type:#04b} is reserved"
                    ));
                }
                rejected_pdu_type = Some(pdu_type);
            }
            consumed += 1;
        }

        if consumed != self.order_specific.len() {
            return Err(format!(
                "Mobile Station Reject has {} trailing order-specific octets",
                self.order_specific.len() - consumed
            ));
        }

        Ok(MobileStationRejectOrderDetail {
            ordq,
            rejected_type,
            rejected_order,
            rejected_ordq,
            rejected_param_id,
            rejected_record,
            con_ref,
            tag,
            rejected_pdu_type,
            trailing_bytes: Vec::new(),
        })
    }

    fn parse_base_station_challenge_order(&self) -> Result<ReverseOrderDetail, String> {
        ensure_reverse_order_specific_len(self, 5, "Base Station Challenge")?;
        ensure_reverse_ordq(self, 0, "Base Station Challenge")?;
        let randbs = ((self.order_specific[1] as u32) << 24)
            | ((self.order_specific[2] as u32) << 16)
            | ((self.order_specific[3] as u32) << 8)
            | self.order_specific[4] as u32;
        Ok(ReverseOrderDetail::BaseStationChallenge { randbs })
    }

    fn parse_reverse_service_option_order(
        &self,
        request: bool,
    ) -> Result<ReverseOrderDetail, String> {
        ensure_reverse_order_specific_len(self, 3, "Service Option Order")?;
        ensure_reverse_ordq(self, 0, "Service Option Order")?;
        let service_option = ((self.order_specific[1] as u16) << 8) | self.order_specific[2] as u16;
        if request {
            Ok(ReverseOrderDetail::ServiceOptionRequest { service_option })
        } else {
            Ok(ReverseOrderDetail::ServiceOptionResponse { service_option })
        }
    }

    fn parse_reduced_slot_cycle_order(&self, ordq: u8) -> Result<ReverseOrderDetail, String> {
        if self.order_specific.len() < 2 {
            return Err("Reduced slot cycle Order requires ORDQ and RSC_MODE_IND".to_string());
        }
        let mut bs = Bitstream::new_bytes(&self.order_specific[1..]);
        let rsc_mode_ind = bs.read_bits(1).map_err(|e| e.to_string())? != 0;
        let (rsci, rsc_end_time_unit, rsc_end_time_value) = if rsc_mode_ind {
            let rsci = bs.read_bits(4).map_err(|e| e.to_string())? as u8;
            if !is_valid_rsci(rsci) {
                return Err(format!("Reduced slot cycle RSCI 0b{rsci:04b} is reserved"));
            }
            let rsc_end_time_unit = bs.read_bits(2).map_err(|e| e.to_string())? as u8;
            if rsc_end_time_unit == 0b11 {
                return Err("Reduced slot cycle RSC_END_TIME_UNIT 0b11 is reserved".to_string());
            }
            let rsc_end_time_value = bs.read_bits(4).map_err(|e| e.to_string())? as u8;
            (
                Some(rsci),
                Some(rsc_end_time_unit),
                Some(rsc_end_time_value),
            )
        } else {
            (None, None, None)
        };
        ensure_access_reserved_zero(&mut bs, "Reduced slot cycle Order RESERVED")?;
        Ok(ReverseOrderDetail::ReducedSlotCycle(
            ReducedSlotCycleOrderDetail {
                order: self.order,
                ordq,
                rsc_mode_ind,
                rsci,
                rsc_end_time_unit,
                rsc_end_time_value,
            },
        ))
    }

    /// Decode order-specific fields for known order types.
    /// `forward_channel` indicates which forward channel the rejected message was on
    /// (ForwardCommon for access channel context, ForwardDedicated for traffic channel).
    pub fn order_detail(&self, forward_channel: WireChannel) -> String {
        match self.order {
            // IS-95 MS Reject Order: ORDQ (3 bits from first order-specific byte)
            0b010010 => {
                let ordq = self.order_specific.first().map_or(0, |b| b >> 5);
                let reason = match ordq {
                    0b000 => "rejected - capability not supported by mobile",
                    0b001 => "rejected - previously rejected",
                    0b010 => "rejected - message not valid in this state",
                    0b011 => "rejected - other",
                    0b100 => "rejected - capability not currently available",
                    _ => "rejected - unknown reason",
                };
                format!("ORDQ={:#05b} ({})", ordq, reason)
            }
            // IS-2000 MS Reject Order: ORDQ is 8 bits
            // C.S0005-E Table 2.7.3-1 (Parts 2-6)
            0b011111 => {
                if let Some(detail) = self.parse_mobile_station_reject_order(forward_channel) {
                    let mut parts = vec![
                        format!(
                            "ORDQ=0x{:02X} ({})",
                            detail.ordq,
                            mobile_station_reject_reason(detail.ordq)
                        ),
                        format!(
                            "REJECTED_TYPE=0x{:02X} ({})",
                            detail.rejected_type,
                            MessageId::from_wire(forward_channel, detail.rejected_type)
                                .map_or("Unknown", |m| m.name())
                        ),
                    ];
                    if let Some(rejected_order) = detail.rejected_order {
                        parts.push(format!(
                            "REJECTED_ORDER=0b{:06b} ({})",
                            rejected_order,
                            forward_order_name(rejected_order)
                        ));
                    }
                    if let Some(rejected_ordq) = detail.rejected_ordq {
                        parts.push(format!("REJECTED_ORDQ=0x{:02X}", rejected_ordq));
                    }
                    if let Some(rejected_param_id) = detail.rejected_param_id {
                        parts.push(format!("REJECTED_PARAM_ID=0x{:04X}", rejected_param_id));
                    }
                    if let Some(rejected_record) = detail.rejected_record {
                        parts.push(format!("REJECTED_RECORD=0x{:02X}", rejected_record));
                    }
                    if let Some(con_ref) = detail.con_ref {
                        parts.push(format!("CON_REF=0x{:02X}", con_ref));
                    }
                    if let Some(tag) = detail.tag {
                        parts.push(format!("TAG=0x{:X}", tag));
                    }
                    if let Some(rejected_pdu_type) = detail.rejected_pdu_type {
                        parts.push(format!(
                            "REJECTED_PDU_TYPE=0b{:02b} ({})",
                            rejected_pdu_type,
                            rejected_pdu_type_name(rejected_pdu_type)
                        ));
                    }
                    if !detail.trailing_bytes.is_empty() {
                        parts.push(format!("trailing={:02X?}", detail.trailing_bytes));
                    }
                    parts.join(", ")
                } else {
                    format!("order_specific={:02X?}", self.order_specific)
                }
            }
            // MS Acknowledgment: ORDQ should be 0
            0b010000 => "ACK".to_string(),
            // SO Request/Response: ORDQ(8) + SERVICE_OPTION(16)
            0b010011 | 0b010100 => match self.reverse_detail(forward_channel) {
                Ok(ReverseOrderDetail::ServiceOptionRequest { service_option })
                | Ok(ReverseOrderDetail::ServiceOptionResponse { service_option }) => {
                    format!("ORDQ=0x00, SO={}", service_option)
                }
                _ => format!("order_specific={:02X?}", self.order_specific),
            },
            _ => {
                if self.order_specific.is_empty() {
                    String::new()
                } else {
                    format!("order_specific={:02X?}", self.order_specific)
                }
            }
        }
    }
}

impl ReverseOrderDetail {
    pub fn to_order_message(&self, header: AccessMessageHeader) -> Result<OrderMessage, String> {
        let (order, order_specific) = match self {
            ReverseOrderDetail::NoAdditionalFields { order } => (*order, Vec::new()),
            ReverseOrderDetail::QualificationOnly { order, ordq } => (*order, vec![*ordq]),
            ReverseOrderDetail::BaseStationChallenge { randbs } => {
                let mut fields = vec![0];
                fields.extend_from_slice(&randbs.to_be_bytes());
                (0b000010, fields)
            }
            ReverseOrderDetail::ServiceOptionRequest { service_option } => {
                let mut fields = vec![0];
                fields.extend_from_slice(&service_option.to_be_bytes());
                (0b010011, fields)
            }
            ReverseOrderDetail::ServiceOptionResponse { service_option } => {
                let mut fields = vec![0];
                fields.extend_from_slice(&service_option.to_be_bytes());
                (0b010100, fields)
            }
            ReverseOrderDetail::MobileStationReject(detail) => {
                (0b011111, encode_mobile_station_reject_order_detail(detail)?)
            }
            ReverseOrderDetail::ReducedSlotCycle(detail) => (
                detail.order,
                encode_reduced_slot_cycle_order_detail(detail)?,
            ),
        };
        ensure_count("ADD_RECORD_LEN", order_specific.len(), 7)?;
        Ok(OrderMessage {
            header,
            order,
            add_record_len: order_specific.len() as u8,
            order_specific,
            remaining_bits: 0,
        })
    }
}

fn encode_mobile_station_reject_order_detail(
    detail: &MobileStationRejectOrderDetail,
) -> Result<Vec<u8>, String> {
    if !is_mobile_station_reject_ordq(detail.ordq) {
        return Err(format!(
            "Mobile Station Reject ORDQ 0x{:02X} is reserved",
            detail.ordq
        ));
    }
    if !detail.trailing_bytes.is_empty() {
        return Err(
            "Mobile Station Reject typed encoder does not allow trailing bytes".to_string(),
        );
    }

    let mut fields = vec![detail.ordq, detail.rejected_type];
    match (detail.rejected_order, detail.rejected_ordq) {
        (Some(order), Some(ordq)) => {
            if order > 0x3f {
                return Err("Mobile Station Reject REJECTED_ORDER exceeds 6 bits".to_string());
            }
            fields.push(order);
            fields.push(ordq);
        }
        (None, None) => {}
        _ => {
            return Err(
                "Mobile Station Reject REJECTED_ORDER and REJECTED_ORDQ must be encoded together"
                    .to_string(),
            );
        }
    }
    if let Some(param_id) = detail.rejected_param_id {
        fields.extend_from_slice(&param_id.to_be_bytes());
    }
    if let Some(record) = detail.rejected_record {
        fields.push(record);
    }
    if let Some(con_ref) = detail.con_ref {
        fields.push(con_ref);
    }
    if let Some(tag) = detail.tag {
        if tag > 0x0f {
            return Err("Mobile Station Reject TAG exceeds 4 bits".to_string());
        }
        let pdu_bits = match detail.rejected_pdu_type {
            Some(pdu_type) if pdu_type <= 0b01 => pdu_type << 2,
            Some(pdu_type) => {
                return Err(format!(
                    "Mobile Station Reject REJECTED_PDU_TYPE {pdu_type:#04b} is reserved"
                ));
            }
            None => 0,
        };
        fields.push((tag << 4) | pdu_bits);
    } else if detail.rejected_pdu_type.is_some() {
        return Err("Mobile Station Reject REJECTED_PDU_TYPE requires TAG".to_string());
    }

    Ok(fields)
}

fn encode_reduced_slot_cycle_order_detail(
    detail: &ReducedSlotCycleOrderDetail,
) -> Result<Vec<u8>, String> {
    if !matches!(detail.order, 0b010101 | 0b100010) {
        return Err(
            "Reduced slot cycle detail only applies to Release/Fast Call Setup".to_string(),
        );
    }
    if detail.order == 0b010101 && detail.ordq != 0x03 {
        return Err("Release reduced slot cycle detail requires ORDQ=0x03".to_string());
    }
    if detail.order == 0b100010 && !matches!(detail.ordq, 0x00 | 0x01) {
        return Err(format!(
            "Fast Call Setup ORDQ 0x{:02X} is reserved",
            detail.ordq
        ));
    }
    let mut body = Bitstream::new();
    body.write_u8(detail.rsc_mode_ind as u8, 1);
    if detail.rsc_mode_ind {
        let rsci = detail
            .rsci
            .ok_or("Reduced slot cycle RSC_MODE_IND=1 requires RSCI")?;
        let unit = detail
            .rsc_end_time_unit
            .ok_or("Reduced slot cycle RSC_MODE_IND=1 requires RSC_END_TIME_UNIT")?;
        let value = detail
            .rsc_end_time_value
            .ok_or("Reduced slot cycle RSC_MODE_IND=1 requires RSC_END_TIME_VALUE")?;
        if !is_valid_rsci(rsci) {
            return Err(format!("Reduced slot cycle RSCI 0b{rsci:04b} is reserved"));
        }
        if unit > 0b10 {
            return Err("Reduced slot cycle RSC_END_TIME_UNIT 0b11 is reserved".to_string());
        }
        if value > 0x0f {
            return Err("Reduced slot cycle RSC_END_TIME_VALUE exceeds 4 bits".to_string());
        }
        body.write_u8(rsci, 4);
        body.write_u8(unit, 2);
        body.write_u8(value, 4);
    } else if detail.rsci.is_some()
        || detail.rsc_end_time_unit.is_some()
        || detail.rsc_end_time_value.is_some()
    {
        return Err("Reduced slot cycle optional fields require RSC_MODE_IND=1".to_string());
    }
    pad_access_reserved_to_octet(&mut body);
    let mut fields = vec![detail.ordq];
    fields.extend_from_slice(&body.to_packed_bytes());
    Ok(fields)
}

fn ensure_reverse_ordq(message: &OrderMessage, expected: u8, name: &str) -> Result<(), String> {
    let ordq = *message
        .order_specific
        .first()
        .ok_or_else(|| format!("{name} requires ORDQ"))?;
    if ordq != expected {
        return Err(format!(
            "{name} ORDQ must be 0x{expected:02X}, got 0x{ordq:02X}"
        ));
    }
    Ok(())
}

fn ensure_reverse_order_specific_len(
    message: &OrderMessage,
    expected: usize,
    name: &str,
) -> Result<(), String> {
    if message.order_specific.len() != expected {
        return Err(format!(
            "{name} requires {expected} order-specific octets, got {}",
            message.order_specific.len()
        ));
    }
    Ok(())
}

fn ensure_access_reserved_zero(bs: &mut Bitstream, name: &str) -> Result<(), String> {
    while !bs.is_empty() {
        if bs.read_bits(1).map_err(|e| e.to_string())? != 0 {
            return Err(format!("{name} contains non-zero reserved bits"));
        }
    }
    Ok(())
}

fn validate_sig_encrypt_sup(value: u8) -> Result<(), String> {
    if value & 0b1000_0000 == 0 {
        return Err("SIG_ENCRYPT_SUP CMEA subfield must be 1".to_string());
    }
    if value & 0b0001_1111 != 0 {
        return Err("SIG_ENCRYPT_SUP RESERVED subfield must be zero".to_string());
    }
    Ok(())
}

fn validate_ui_encrypt_sup(value: u8) -> Result<(), String> {
    if value & 0b0011_1111 != 0 {
        return Err("UI_ENCRYPT_SUP RESERVED subfield must be zero".to_string());
    }
    Ok(())
}

fn validate_sig_integrity_fields(sig_sup: u8, sig_req: u8) -> Result<(), String> {
    if sig_sup != 0 {
        return Err("SIG_INTEGRITY_SUP RESERVED subfield must be zero".to_string());
    }
    if sig_req != 0 {
        return Err("SIG_INTEGRITY_REQ reserved value".to_string());
    }
    Ok(())
}

fn pad_access_reserved_to_octet(bs: &mut Bitstream) {
    let pad_bits = (8 - (bs.len() % 8)) % 8;
    if pad_bits > 0 {
        bs.write_u8(0, pad_bits);
    }
}

fn is_mobile_station_reject_ordq(ordq: u8) -> bool {
    matches!(
        ordq,
        0x01..=0x0d | 0x0e | 0x10..=0x13 | 0x14..=0x16 | 0x18..=0x20
    )
}

fn reverse_order_name(order: u8) -> &'static str {
    match order {
        0b000001 => "Abbreviated Alert",
        0b000010 => "Base Station Challenge",
        0b000011 => "SSD Update Confirmation/Rejection",
        0b000101 => "Parameter Update Confirmation",
        0b000110 => "TMSI Assignment Completion",
        0b001000 => "Local Control",
        0b001100 => "Slotted Mode Request",
        0b010000 => "Mobile Station Acknowledgment",
        0b010010 => "Mobile Station Reject",
        0b010011 => "Service Option Request",
        0b010100 => "Service Option Response",
        0b010101 => "Release",
        0b010111 => "Long Code Transition",
        0b011000 => "Connect",
        0b011001 => "Continuous DTMF Tone",
        0b011010 => "Continuous DTMF Tone Stop",
        0b011101 => "Service Option Control",
        0b011110 => "PACA Cancel",
        0b011111 => "Mobile Station Reject Order",
        _ => "Unknown Order",
    }
}

fn forward_order_name(order: u8) -> &'static str {
    match order {
        0 => "Abbreviated Alert",
        1 => "Base Station Acknowledgment",
        2 => "Base Station Challenge Confirmation",
        0b011011 => "Registration Accepted",
        0b011100 => "Registration Rejected",
        5 => "Maintenance Required",
        6 => "Lock Until Power-Cycled",
        7 => "Unlock",
        8 => "Parameter Update",
        18 => "Release",
        _ => "Unknown Order",
    }
}

fn mobile_station_reject_reason(ordq: u8) -> &'static str {
    match ordq {
        0x01 => "unspecified reason",
        0x02 => "message not accepted in this state",
        0x03 => "message structure not acceptable",
        0x04 => "message field not in valid range",
        0x05 => "message type or order code not understood",
        0x06 => "capability not supported by mobile station",
        0x07 => "cannot be handled by current mobile station configuration",
        0x08 => "response message would exceed allowable length",
        0x09 => "info record not supported for specified band class/operating mode",
        0x0A => "search set not specified",
        0x0B => "invalid search request",
        0x0C => "invalid Frequency Assignment",
        0x0D => "search period too short",
        0x0E => "RC does not match DEFAULT_CONFIG value",
        0x10 => "call assignment not accepted",
        0x11 => "no call control instance with specified identifier",
        0x12 => "call control instance already present with specified identifier",
        0x13 => "TAG received does not match any stored TAG",
        0x14 => "UAK not supported",
        0x15 => "stored configuration already restored at channel assignment",
        0x16 => "MAC-I field is missing",
        0x18 => "MAC-I field is present but invalid",
        0x19 => "security sequence number is invalid",
        0x1A => "message cannot be decrypted",
        0x1B => "requested stored service configuration not available",
        0x1C => "PLCM_TYPE mismatch",
        0x1D => "General Extension Record contains unsupported record type",
        0x1E => "General Extension Record field value outside permissible range",
        0x1F => "General Extension Record field value not supported",
        0x20 => "General Extension Record not acceptable, unspecified reason",
        _ => "unknown ORDQ",
    }
}

fn rejected_pdu_type_name(rejected_pdu_type: u8) -> &'static str {
    match rejected_pdu_type {
        0b00 => "20 ms regular message",
        0b01 => "5 ms mini message",
        _ => "reserved",
    }
}

/// Reverse Data Burst Message (r-csch). C.S0005-E 2.7.1.3.2.3.
#[derive(Debug, Clone)]
pub struct DataBurstMessage {
    pub header: AccessMessageHeader,
    pub msg_number: u8,
    pub burst_type: u8,
    pub num_msgs: u8,
    pub num_fields: u8,
    pub fields: Vec<u8>,
    pub remaining_bits: usize,
}

impl DataBurstMessage {
    pub fn burst_type_name(&self) -> &'static str {
        match self.burst_type {
            0b000000 => "Unknown burst type",
            0b000001 => "Asynchronous Data Services",
            0b000010 => "Group-3 Facsimile",
            0b000011 => "Short Message Services",
            0b000100 => "Over-the-Air Service Provisioning",
            0b000101 => "Position Determination Services",
            0b000110 => "Short Data Bursts",
            0b000111 => "HRPD Packet Data Service Notification",
            0b001000 => "Broadcast Multicast Service",
            0b111110 => "Extended Burst Type - International",
            0b111111 => "Extended Burst Type",
            _ => "Reserved",
        }
    }
}

/// Service Connect Completion Message (r-dsch). C.S0004-E 2.7.2.3.2.13.
#[derive(Debug, Clone)]
pub struct ServiceConnectCompletionMessage {
    pub serv_con_seq: u8,
}

/// Power Measurement Report Message (PMRM). C.S0005-E 2.7.2.3.2.6.
///
/// Sent by the mobile on the reverse dedicated channel to report frame error
/// statistics and active-set pilot strengths.
#[derive(Debug, Clone)]
pub struct PowerMeasurementReportMessage {
    /// Number of bad frames on FCH (or DCCH if only DCCH assigned). 5 bits.
    pub errors_detected: u8,
    /// Total frame count on FCH (or DCCH if only DCCH assigned). 10 bits.
    pub pwr_meas_frames: u16,
    /// Last Handoff Direction Message sequence number. 2 bits. 3 = none received.
    pub last_hdm_seq: u8,
    /// Pilot strengths for each pilot in the Active Set. NUM_PILOTS x 6 bits.
    pub pilot_strengths: Vec<u8>,
    /// 1 if both FCH and DCCH are assigned.
    pub dcch_pwr_meas_incl: bool,
    /// DCCH frame count. 10 bits. Present if dcch_pwr_meas_incl.
    pub dcch_pwr_meas_frames: Option<u16>,
    /// DCCH bad frame count. 5 bits. Present if dcch_pwr_meas_incl.
    pub dcch_errors_detected: Option<u8>,
    /// 1 if FOR_SCH_FER_REP=1 and reporting SCH.
    pub sch_pwr_meas_incl: bool,
    /// Supplemental channel ID. 1 bit. Present if sch_pwr_meas_incl.
    pub sch_id: Option<u8>,
    /// SCH frame count. 16 bits. Present if sch_pwr_meas_incl.
    pub sch_pwr_meas_frames: Option<u16>,
    /// SCH bad frame count. 10 bits. Present if sch_pwr_meas_incl.
    pub sch_errors_detected: Option<u16>,
}

/// Reverse Service Request Message (SRQM). C.S0005-E 2.7.2.3.2.12.
#[derive(Debug, Clone)]
pub struct ServiceRequestMessage {
    pub serv_req_seq: u8,
    pub req_purpose: u8,
    /// Service Configuration record, present when req_purpose == 0b0010 (propose).
    pub service_config: Option<ServiceConfigRecord>,
}

/// Reverse Service Response Message (SRPM). C.S0005-E 2.7.2.3.2.13.
#[derive(Debug, Clone)]
pub struct ServiceResponseMessage {
    pub serv_req_seq: u8,
    pub resp_purpose: u8,
    /// Service Configuration record, present when resp_purpose == 0b0010 (counter-propose).
    pub service_config: Option<ServiceConfigRecord>,
}

/// A single pilot report within a Pilot Strength Measurement Message.
#[derive(Debug, Clone)]
pub struct PilotReport {
    pub pilot_pn_phase: u16, // 15-bit PN offset in chips
    pub pilot_strength: u8,  // 6-bit Ec/Io (0.5 dB steps)
    pub keep: bool,
}

/// Pilot Strength Measurement Message (PSMM). C.S0004-E 2.7.2.3.2.4.
#[derive(Debug, Clone)]
pub struct PilotStrengthMeasurementMessage {
    pub ref_pn: u16,
    pub pilot_strength: u8,
    pub keep: bool,
    pub pilots: Vec<PilotReport>,
}

#[derive(Debug, Clone)]
pub enum AccessMessage {
    Registration(RegistrationMessage),
    Origination(OriginationMessage),
    PageResponse(PageResponseMessage),
    Order(OrderMessage),
    DataBurst(DataBurstMessage),
    AuthChallengeResponse(AuthChallengeResponseMessage),
    StatusResponse(StatusResponseMessage),
    TmsiAssignmentCompletion(NoFieldAccessMessage),
    PacaCancel(NoFieldAccessMessage),
    ExtStatusResponse(ExtendedStatusResponseMessage),
    DeviceInformation(DeviceInformationMessage),
    SecurityModeRequest(SecurityModeRequestMessage),
    AuthResponse(AuthResponseMessage),
    AuthResync(AuthResyncMessage),
    Reconnect(ReconnectMessage),
    RadioEnvironment(RadioEnvironmentMessage),
    CallRecoveryRequest(NoFieldAccessMessage),
    GeneralExtension(GeneralExtensionMessage),
    FlashWithInfo(FlashWithInfoMessage),
    SendBurstDtmf(SendBurstDtmfMessage),
    Status(StatusMessage),
    OriginationContinuation(OriginationContinuationMessage),
    HandoffCompletion(HandoffCompletionMessage),
    ParametersResponse(ParametersResponseMessage),
    ServiceOptionControl(ServiceOptionControlMessage),
    SupplementalChannelRequest(SupplementalChannelRequestMessage),
    CandidateFreqSearchResponse(CandidateFreqSearchResponseMessage),
    CandidateFreqSearchReport(CandidateFreqSearchReportMessage),
    PeriodicPsmm(PeriodicPsmmMessage),
    OuterLoopReport(OuterLoopReportMessage),
    ResourceRequest(ResourceRequestMessage),
    ExtReleaseResponse(ExtReleaseResponseMessage),
    ServiceConnectCompletion(ServiceConnectCompletionMessage),
    PilotStrengthMeasurement(PilotStrengthMeasurementMessage),
    PowerMeasurementReport(PowerMeasurementReportMessage),
    ServiceRequest(ServiceRequestMessage),
    ServiceResponse(ServiceResponseMessage),
}

fn format_origination_digits(digit_mode: bool, digits: &[u8]) -> String {
    if digits.is_empty() {
        return String::new();
    }

    if digit_mode {
        return String::from_utf8_lossy(digits).to_string();
    }

    let raw = digits
        .iter()
        .map(|d| format!("{:x}", d))
        .collect::<Vec<_>>()
        .join("");
    let rendered = digits
        .iter()
        .map(|d| match d & 0x0f {
            0x1..=0x9 => char::from(b'0' + (d & 0x0f)),
            0x0a => '0',
            0x0b => '*',
            0x0c => '#',
            _ => '?',
        })
        .collect::<String>();
    format!("{}(raw={})", rendered, raw)
}

impl AccessMessage {
    pub fn decode(data: &Bitstream) -> Result<Self, String> {
        Self::decode_with_context(data, AccessDecodeContext::default())
    }

    /// Decode a reverse common-channel Layer 3 message with serving-system
    /// context needed for conditionally-present fields.
    pub fn decode_with_context(data: &Bitstream, ctx: AccessDecodeContext) -> Result<Self, String> {
        let mut bs = data.clone();
        if bs.len() < 8 {
            return Err(format!("PDU too short: {} bits", bs.len()));
        }

        let pd_and_type = bs.read_bits(8).map_err(|e| e.to_string())? as u8;
        let raw_tag = pd_and_type & 0x3f;
        let message_id = MessageId::from_wire(WireChannel::ReverseCommon, raw_tag)
            .ok_or_else(|| format!("unsupported r-csch MSG_TAG 0x{raw_tag:02X}"))?;
        let header = AccessMessageHeader {
            pd: pd_and_type >> 6,
            message_id,
        };

        match message_id {
            MessageId::Registration => decode_registration(header, &mut bs),
            MessageId::Order => decode_order(header, &mut bs),
            MessageId::DataBurst => decode_data_burst(header, &mut bs),
            MessageId::Origination => decode_origination(header, &mut bs),
            MessageId::PageResponse => decode_page_response(header, &mut bs, ctx),
            MessageId::AuthChallengeResponse => decode_auth_challenge_response(header, &mut bs),
            MessageId::StatusResponse => decode_status_response(header, &mut bs),
            MessageId::TmsiAssignmentCompletion => {
                decode_no_field_message(header, &mut bs, AccessMessage::TmsiAssignmentCompletion)
            }
            MessageId::PacaCancel => {
                decode_no_field_message(header, &mut bs, AccessMessage::PacaCancel)
            }
            MessageId::ExtStatusResponse => decode_extended_status_response(header, &mut bs),
            MessageId::DeviceInformation => decode_device_information(header, &mut bs),
            MessageId::SecurityModeRequest => decode_security_mode_request(header, &mut bs),
            MessageId::AuthResponse => decode_auth_response(header, &mut bs),
            MessageId::AuthResync => decode_auth_resync(header, &mut bs),
            MessageId::Reconnect => decode_reconnect(header, &mut bs, ctx),
            MessageId::RadioEnvironment => decode_radio_environment(header, &mut bs),
            MessageId::CallRecoveryRequest => {
                decode_no_field_message(header, &mut bs, AccessMessage::CallRecoveryRequest)
            }
            MessageId::GeneralExtension => decode_general_extension(header, &mut bs),
            _ => Err(format!(
                "unsupported r-csch body decode for {}",
                message_id.tag()
            )),
        }
    }

    pub fn decode_sdu(header: AccessMessageHeader, data: &Bitstream) -> Result<Self, String> {
        Self::decode_sdu_with_context(header, data, AccessDecodeContext::default())
    }

    /// Decode an already-delimited Layer 3 SDU with serving-system context
    /// needed for conditionally-present fields.
    pub fn decode_sdu_with_context(
        header: AccessMessageHeader,
        data: &Bitstream,
        ctx: AccessDecodeContext,
    ) -> Result<Self, String> {
        let mut bs = data.clone();
        match header.message_id {
            MessageId::Registration => decode_registration(header, &mut bs),
            MessageId::Order => decode_order(header, &mut bs),
            MessageId::DataBurst => decode_data_burst(header, &mut bs),
            MessageId::Origination => decode_origination(header, &mut bs),
            MessageId::PageResponse => decode_page_response(header, &mut bs, ctx),
            MessageId::AuthChallengeResponse => decode_auth_challenge_response(header, &mut bs),
            MessageId::StatusResponse => decode_status_response(header, &mut bs),
            MessageId::TmsiAssignmentCompletion => {
                decode_no_field_message(header, &mut bs, AccessMessage::TmsiAssignmentCompletion)
            }
            MessageId::PacaCancel => {
                decode_no_field_message(header, &mut bs, AccessMessage::PacaCancel)
            }
            MessageId::ExtStatusResponse => decode_extended_status_response(header, &mut bs),
            MessageId::DeviceInformation => decode_device_information(header, &mut bs),
            MessageId::SecurityModeRequest => decode_security_mode_request(header, &mut bs),
            MessageId::AuthResponse => decode_auth_response(header, &mut bs),
            MessageId::AuthResync => decode_auth_resync(header, &mut bs),
            MessageId::Reconnect => decode_reconnect(header, &mut bs, ctx),
            MessageId::RadioEnvironment => decode_radio_environment(header, &mut bs),
            MessageId::CallRecoveryRequest => {
                decode_no_field_message(header, &mut bs, AccessMessage::CallRecoveryRequest)
            }
            MessageId::GeneralExtension => decode_general_extension(header, &mut bs),
            _ => Err(format!(
                "unsupported r-csch body decode for {}",
                header.message_id.tag()
            )),
        }
    }

    /// Encode the Layer 3 message body without the reverse-common PD/MSG_TAG octet.
    pub fn to_sdu(&self) -> Result<Bitstream, String> {
        self.to_sdu_with_context(AccessDecodeContext::default())
    }

    /// Encode a reverse dedicated signaling-channel Layer 3 message body.
    ///
    /// Most reverse dedicated bodies share their body layout with the access
    /// encoder. Messages with channel-specific layouts are handled here.
    pub fn to_rdsch_sdu(&self) -> Result<Bitstream, String> {
        let mut bs = Bitstream::new();
        match self {
            AccessMessage::SecurityModeRequest(m) => {
                encode_rdsch_security_mode_request_body(&mut bs, m)?;
                Ok(bs)
            }
            _ => self.to_sdu(),
        }
    }

    /// Encode the Layer 3 message body without the reverse-common PD/MSG_TAG octet.
    ///
    /// `ctx` supplies serving-system state for fields whose presence is not
    /// self-describing on the wire, notably Page Response and Reconnect tails.
    pub fn to_sdu_with_context(&self, ctx: AccessDecodeContext) -> Result<Bitstream, String> {
        encode_access_message_body(self, ctx)
    }

    /// Encode a complete r-csch Layer 3 PDU: PD(2) | MSG_TAG(6) | body.
    pub fn to_reverse_common_pdu(&self) -> Result<Bitstream, String> {
        self.to_reverse_common_pdu_with_context(AccessDecodeContext::default())
    }

    /// Encode a complete r-csch Layer 3 PDU: PD(2) | MSG_TAG(6) | body.
    pub fn to_reverse_common_pdu_with_context(
        &self,
        ctx: AccessDecodeContext,
    ) -> Result<Bitstream, String> {
        let header = self
            .header()
            .ok_or_else(|| "message has no reverse-common Layer 3 header".to_string())?;
        let wire_type = header
            .message_id
            .wire_type(WireChannel::ReverseCommon)
            .ok_or_else(|| format!("{} has no reverse-common MSG_TAG", header.message_id.name()))?;
        let mut pdu = Bitstream::new();
        pdu.write_u8((header.pd << 6) | (wire_type & 0x3f), 8);
        pdu.extend(&self.to_sdu_with_context(ctx)?);
        Ok(pdu)
    }

    pub fn header(&self) -> Option<&AccessMessageHeader> {
        match self {
            AccessMessage::Registration(m) => Some(&m.header),
            AccessMessage::Origination(m) => Some(&m.header),
            AccessMessage::PageResponse(m) => Some(&m.header),
            AccessMessage::Order(m) => Some(&m.header),
            AccessMessage::DataBurst(m) => Some(&m.header),
            AccessMessage::AuthChallengeResponse(m) => Some(&m.header),
            AccessMessage::StatusResponse(m) => Some(&m.header),
            AccessMessage::TmsiAssignmentCompletion(m) => Some(&m.header),
            AccessMessage::PacaCancel(m) => Some(&m.header),
            AccessMessage::ExtStatusResponse(m) => Some(&m.header),
            AccessMessage::DeviceInformation(m) => Some(&m.header),
            AccessMessage::SecurityModeRequest(m) => Some(&m.header),
            AccessMessage::AuthResponse(m) => Some(&m.header),
            AccessMessage::AuthResync(m) => Some(&m.header),
            AccessMessage::Reconnect(m) => Some(&m.header),
            AccessMessage::RadioEnvironment(m) => Some(&m.header),
            AccessMessage::CallRecoveryRequest(m) => Some(&m.header),
            AccessMessage::GeneralExtension(m) => Some(&m.header),
            AccessMessage::FlashWithInfo(m) => Some(&m.header),
            AccessMessage::SendBurstDtmf(m) => Some(&m.header),
            AccessMessage::Status(m) => Some(&m.header),
            AccessMessage::OriginationContinuation(m) => Some(&m.header),
            AccessMessage::HandoffCompletion(m) => Some(&m.header),
            AccessMessage::ParametersResponse(m) => Some(&m.header),
            AccessMessage::ServiceOptionControl(m) => Some(&m.header),
            AccessMessage::SupplementalChannelRequest(m) => Some(&m.header),
            AccessMessage::CandidateFreqSearchResponse(m) => Some(&m.header),
            AccessMessage::CandidateFreqSearchReport(m) => Some(&m.header),
            AccessMessage::PeriodicPsmm(m) => Some(&m.header),
            AccessMessage::OuterLoopReport(m) => Some(&m.header),
            AccessMessage::ResourceRequest(m) => Some(&m.header),
            AccessMessage::ExtReleaseResponse(m) => Some(&m.header),
            AccessMessage::ServiceConnectCompletion(_)
            | AccessMessage::PilotStrengthMeasurement(_)
            | AccessMessage::PowerMeasurementReport(_)
            | AccessMessage::ServiceRequest(_)
            | AccessMessage::ServiceResponse(_) => None,
        }
    }

    /// Extract MOB_P_REV from message types that carry it.
    pub fn mob_p_rev(&self) -> Option<u8> {
        match self {
            AccessMessage::Registration(m) => Some(m.mob_p_rev),
            AccessMessage::Origination(m) => Some(m.mob_p_rev),
            AccessMessage::PageResponse(m) => Some(m.mob_p_rev),
            _ => None,
        }
    }

    /// Extract SLOT_CYCLE_INDEX from message types that carry it.
    pub fn slot_cycle_index(&self) -> Option<u8> {
        match self {
            AccessMessage::Registration(m) => Some(m.slot_cycle_index),
            AccessMessage::Origination(m) => Some(m.slot_cycle_index),
            AccessMessage::PageResponse(m) => Some(m.slot_cycle_index),
            _ => None,
        }
    }

    /// Extract SCM (Station Class Mark) from message types that carry it.
    pub fn scm(&self) -> Option<u8> {
        match self {
            AccessMessage::Registration(m) => Some(m.scm),
            AccessMessage::Origination(m) => Some(m.scm),
            AccessMessage::PageResponse(m) => Some(m.scm),
            _ => None,
        }
    }

    /// Extract `SERVICE_OPTION` from access messages that carry it.
    pub fn service_option(&self) -> Option<u16> {
        match self {
            AccessMessage::Origination(m) => m.service_option,
            AccessMessage::PageResponse(m) => Some(m.service_option),
            AccessMessage::Reconnect(m) => m.service_option,
            AccessMessage::ServiceRequest(m) => m
                .service_config
                .as_ref()
                .and_then(|cfg| cfg.connection_records.first())
                .map(|cr| cr.service_option),
            _ => None,
        }
    }

    /// Extract forward supported Radio Configurations from FCH capability.
    /// Returns the list of forward RCs the mobile supports (e.g. `[1, 3]` for RC1+RC3).
    pub fn for_supported_rcs(&self) -> Vec<u8> {
        match self {
            AccessMessage::Origination(m) => m
                .fch_capability
                .as_ref()
                .map_or_else(Vec::new, |cap| cap.for_supported_rcs.clone()),
            AccessMessage::PageResponse(m) => m
                .fch_capability
                .as_ref()
                .map_or_else(Vec::new, |cap| cap.for_supported_rcs.clone()),
            _ => Vec::new(),
        }
    }

    /// Extract reverse supported Radio Configurations from FCH capability.
    pub fn rev_supported_rcs(&self) -> Vec<u8> {
        match self {
            AccessMessage::Origination(m) => m
                .fch_capability
                .as_ref()
                .map_or_else(Vec::new, |cap| cap.rev_supported_rcs.clone()),
            AccessMessage::PageResponse(m) => m
                .fch_capability
                .as_ref()
                .map_or_else(Vec::new, |cap| cap.rev_supported_rcs.clone()),
            _ => Vec::new(),
        }
    }

    /// Extract Data Burst fields (burst_type, msg_number, num_msgs, CHARi payload).
    pub fn data_burst_fields(&self) -> Option<(u8, u8, u8, &[u8])> {
        match self {
            AccessMessage::DataBurst(m) => {
                Some((m.burst_type, m.msg_number, m.num_msgs, &m.fields))
            }
            _ => None,
        }
    }

    /// Extract the ORDER code from an Order Message, if this is one.
    pub fn order_code(&self) -> Option<u8> {
        match self {
            AccessMessage::Order(m) => Some(m.order),
            _ => None,
        }
    }

    /// Extract SERV_CON_SEQ from a Service Connect Completion, if this is one.
    pub fn serv_con_seq(&self) -> Option<u8> {
        match self {
            AccessMessage::ServiceConnectCompletion(m) => Some(m.serv_con_seq),
            _ => None,
        }
    }

    pub fn summary(&self) -> String {
        self.summary_with_rejected_forward_channel(WireChannel::ForwardCommon)
    }

    pub fn summary_with_rejected_forward_channel(
        &self,
        rejected_forward_channel: WireChannel,
    ) -> String {
        match self {
            AccessMessage::Registration(m) => format!(
                "Registration(reg_type={}, slot_cycle_index={}, mob_p_rev={}, scm=0x{:02x}, mob_term={}, return_cause={}, remaining_bits={})",
                m.reg_type,
                m.slot_cycle_index,
                m.mob_p_rev,
                m.scm,
                m.mob_term as u8,
                m.return_cause,
                m.remaining_bits,
            ),
            AccessMessage::Origination(m) => {
                let digits = format_origination_digits(m.digit_mode, &m.digits);
                let alt_service_options = if m.alt_service_options.is_empty() {
                    String::new()
                } else {
                    format!("{:?}", m.alt_service_options)
                };
                let fch_summary = m.fch_capability.as_ref().map_or(String::new(), |cap| {
                    format!(
                        " fch(frame5ms={}, for_rcs={:?}, rev_rcs={:?})",
                        cap.frame_size_5ms_supported as u8,
                        cap.for_supported_rcs,
                        cap.rev_supported_rcs,
                    )
                });
                let dcch_summary = m.dcch_capability.as_ref().map_or(String::new(), |cap| {
                    format!(
                        " dcch(frame_mode=0b{:02b}, for_rcs={:?}, rev_rcs={:?})",
                        cap.frame_size_mode, cap.for_supported_rcs, cap.rev_supported_rcs,
                    )
                });
                format!(
                    "Origination(mob_term={}, slot_cycle_index={}, mob_p_rev={}, scm=0x{:02x}, request_mode={}, special_service={}, service_option={:?}, pm={}, digit_mode={}, number_type={:?}, number_plan={:?}, more_fields={}, num_fields={}, digits={}, nar_an_cap={}, paca_reorig={}, return_cause={}, more_records={}, encryption_supported={:?}, paca_supported={}, num_alt_so={}, alt_so={}, drs={:?}, uzid={:?}, ch_ind={:?}, sr_id={:?}, otd_supported={:?}, qpch_supported={:?}, enhanced_rc={:?}, for_rc_pref={:?}, rev_rc_pref={:?}, geo_loc_type={:?}, rev_fch_gating_req={:?}, orig_reason={:?}, orig_count={:?}, remaining_bits={}{}{})",
                    m.mob_term as u8,
                    m.slot_cycle_index,
                    m.mob_p_rev,
                    m.scm,
                    m.request_mode,
                    m.special_service as u8,
                    m.service_option,
                    m.pm as u8,
                    m.digit_mode as u8,
                    m.number_type,
                    m.number_plan,
                    m.more_fields as u8,
                    m.num_fields,
                    digits,
                    m.nar_an_cap as u8,
                    m.paca_reorig as u8,
                    m.return_cause,
                    m.more_records as u8,
                    m.encryption_supported,
                    m.paca_supported as u8,
                    m.num_alt_so,
                    alt_service_options,
                    m.drs,
                    m.uzid,
                    m.ch_ind,
                    m.sr_id,
                    m.otd_supported,
                    m.qpch_supported,
                    m.enhanced_rc,
                    m.for_rc_pref,
                    m.rev_rc_pref,
                    m.geo_loc_type,
                    m.rev_fch_gating_req,
                    m.orig_reason,
                    m.orig_count,
                    m.remaining_bits,
                    fch_summary,
                    dcch_summary,
                )
            }
            AccessMessage::PageResponse(m) => {
                let fch_summary = m
                    .fch_capability
                    .as_ref()
                    .map(|cap| {
                        format!(
                            ", fch(frame5ms={}, for_rcs={:?}, rev_rcs={:?})",
                            cap.frame_size_5ms_supported,
                            cap.for_supported_rcs,
                            cap.rev_supported_rcs,
                        )
                    })
                    .unwrap_or_default();
                let dcch_summary = m
                    .dcch_capability
                    .as_ref()
                    .map(|cap| {
                        format!(
                            ", dcch(frame_mode=0b{:02b}, for_rcs={:?}, rev_rcs={:?})",
                            cap.frame_size_mode, cap.for_supported_rcs, cap.rev_supported_rcs,
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "PageResponse(mob_term={}, slot_cycle_index={}, mob_p_rev={}, scm=0x{:02x}, request_mode={}, service_option={}, pm={}, nar_an_cap={}, encryption_supported={:?}, num_alt_so={}, alt_service_options={:?}, uzid={:?}, ch_ind={:?}, otd_supported={:?}, qpch_supported={:?}, enhanced_rc={:?}, for_rc_pref={:?}, rev_rc_pref={:?}, rev_fch_gating_req={:?}, sts_supported={:?}, cch_3x_supported={:?}, wll_device_type={:?}, hook_status={:?}, remaining_bits={}{}{})",
                    m.mob_term as u8,
                    m.slot_cycle_index,
                    m.mob_p_rev,
                    m.scm,
                    m.request_mode,
                    m.service_option,
                    m.pm as u8,
                    m.nar_an_cap as u8,
                    m.encryption_supported,
                    m.num_alt_so,
                    m.alt_service_options,
                    m.uzid,
                    m.ch_ind,
                    m.otd_supported,
                    m.qpch_supported,
                    m.enhanced_rc,
                    m.for_rc_pref,
                    m.rev_rc_pref,
                    m.rev_fch_gating_req,
                    m.sts_supported,
                    m.cch_3x_supported,
                    m.wll_device_type,
                    m.hook_status,
                    m.remaining_bits,
                    fch_summary,
                    dcch_summary,
                )
            }
            AccessMessage::Order(m) => {
                let detail = m.order_detail(rejected_forward_channel);
                let detail_str = if detail.is_empty() {
                    String::new()
                } else {
                    format!(", {}", detail)
                };
                format!(
                    "Order(order=0b{:06b} {}, add_record_len={}, order_specific=[{:02X?}]{})",
                    m.order,
                    m.order_name(),
                    m.add_record_len,
                    m.order_specific,
                    detail_str,
                )
            }
            AccessMessage::DataBurst(m) => format!(
                "DataBurst(msg_number={}, burst_type=0b{:06b} {}, num_msgs={}, num_fields={}, remaining_bits={})",
                m.msg_number,
                m.burst_type,
                m.burst_type_name(),
                m.num_msgs,
                m.num_fields,
                m.remaining_bits,
            ),
            AccessMessage::AuthChallengeResponse(m) => format!(
                "AuthChallengeResponse(authu=0x{:05x}, remaining_bits={})",
                m.authu, m.remaining_bits
            ),
            AccessMessage::StatusResponse(m) => format!(
                "StatusResponse(qual_info_type=0x{:02x}, qual_info_len={}, records={}, remaining_bits={})",
                m.qual_info_type,
                m.qual_info.len(),
                m.records.len(),
                m.remaining_bits
            ),
            AccessMessage::TmsiAssignmentCompletion(m)
            | AccessMessage::PacaCancel(m)
            | AccessMessage::CallRecoveryRequest(m) => format!(
                "{}(remaining_bits={})",
                m.header.message_id.name(),
                m.remaining_bits
            ),
            AccessMessage::ExtStatusResponse(m) => format!(
                "ExtendedStatusResponse(qual_info_type=0x{:02x}, qual_info_len={}, num_info_records={}, remaining_bits={})",
                m.qual_info_type,
                m.qual_info.len(),
                m.num_info_records,
                m.remaining_bits
            ),
            AccessMessage::DeviceInformation(m) => format!(
                "DeviceInformation(wll_device_type={}, num_info_records={}, remaining_bits={})",
                m.wll_device_type, m.num_info_records, m.remaining_bits
            ),
            AccessMessage::SecurityModeRequest(m) => format!(
                "SecurityModeRequest(ui_encrypt_sup={:?}, sig_encrypt_sup={:?}, c_sig_encrypt_req={:?}, msg_int_info_incl={:?}, sig_integrity_sup_incl={:?}, sig_integrity_sup={:?}, sig_integrity_req={:?}, remaining_bits={})",
                m.ui_encrypt_sup,
                m.sig_encrypt_sup,
                m.c_sig_encrypt_req,
                m.msg_int_info_incl,
                m.sig_integrity_sup_incl,
                m.sig_integrity_sup,
                m.sig_integrity_req,
                m.remaining_bits
            ),
            AccessMessage::AuthResponse(m) => format!(
                "AuthResponse(res_len={}, sig_integrity_sup={:?}, sig_integrity_req={:?}, new_key_id={}, new_sseq_h=0x{:06x}, remaining_bits={})",
                m.res.len(),
                m.sig_integrity_sup,
                m.sig_integrity_req,
                m.new_key_id,
                m.new_sseq_h,
                m.remaining_bits
            ),
            AccessMessage::AuthResync(m) => format!(
                "AuthResync(con_ms_sqn_len={}, mac_s_len={}, remaining_bits={})",
                m.con_ms_sqn.len(),
                m.mac_s.len(),
                m.remaining_bits
            ),
            AccessMessage::Reconnect(m) => format!(
                "Reconnect(orig_ind={}, sync_id_incl={}, sync_id_len={:?}, service_option={:?}, sr_id={:?}, add_sr_ids={:?}, sdb_len={}, remaining_bits={})",
                m.orig_ind as u8,
                m.sync_id_incl as u8,
                m.sync_id_len,
                m.service_option,
                m.sr_id,
                m.add_sr_ids,
                m.sdb_fields.len(),
                m.remaining_bits
            ),
            AccessMessage::RadioEnvironment(m) => format!(
                "RadioEnvironment(mode_disabled={}, tkz_mode_ind={}, remaining_bits={})",
                m.mode_disabled as u8, m.tkz_mode_ind as u8, m.remaining_bits
            ),
            AccessMessage::GeneralExtension(m) => format!(
                "GeneralExtension(num_ge_records={}, message_type=0x{:02x}, message_record_bits={}, remaining_bits={})",
                m.num_ge_records,
                m.message_type,
                m.message_record.len(),
                m.remaining_bits
            ),
            AccessMessage::FlashWithInfo(m) => format!(
                "FlashWithInfo(records={}, remaining_bits={})",
                m.records.len(),
                m.remaining_bits
            ),
            AccessMessage::SendBurstDtmf(m) => format!(
                "SendBurstDtmf(num_digits={}, dtmf_on_length={}, dtmf_off_length={}, con_ref={:?}, remaining_bits={})",
                m.digits.len(),
                m.dtmf_on_length,
                m.dtmf_off_length,
                m.con_ref,
                m.remaining_bits
            ),
            AccessMessage::Status(m) => format!(
                "Status(record_type=0x{:02x}, record_len={}, remaining_bits={})",
                m.record.record_type,
                m.record.data.len(),
                m.remaining_bits
            ),
            AccessMessage::OriginationContinuation(m) => format!(
                "OriginationContinuation(digit_mode={}, num_fields={}, records={}, remaining_bits={})",
                m.digit_mode as u8,
                m.digits.len(),
                m.records.len(),
                m.remaining_bits
            ),
            AccessMessage::HandoffCompletion(m) => format!(
                "HandoffCompletion(last_hdm_seq={}, pilot_pns={:?}, remaining_bits={})",
                m.last_hdm_seq, m.pilot_pns, m.remaining_bits
            ),
            AccessMessage::ParametersResponse(m) => format!(
                "ParametersResponse(records={}, remaining_bits={})",
                m.records.len(),
                m.remaining_bits
            ),
            AccessMessage::ServiceOptionControl(m) => format!(
                "ServiceOptionControl(con_ref={}, service_option={}, ctl_rec_len={}, remaining_bits={})",
                m.con_ref,
                m.service_option,
                m.control_record.len(),
                m.remaining_bits
            ),
            AccessMessage::SupplementalChannelRequest(m) => format!(
                "SupplementalChannelRequest(req_blob_len={}, use_scrm_seq_num={}, measurements={}, remaining_bits={})",
                m.req_blob.len(),
                m.scrm_seq_num.is_some() as u8,
                m.measurements.is_some() as u8,
                m.remaining_bits
            ),
            AccessMessage::CandidateFreqSearchResponse(m) => format!(
                "CandidateFreqSearchResponse(last_cfsrm_seq={}, total_off_time_fwd={}, max_off_time_fwd={}, total_off_time_rev={}, max_off_time_rev={}, pcg_off_times={}, align_timing_used={}, max_num_visits={:?}, inter_visit_time={:?}, remaining_bits={})",
                m.last_cfsrm_seq,
                m.total_off_time_fwd,
                m.max_off_time_fwd,
                m.total_off_time_rev,
                m.max_off_time_rev,
                m.pcg_off_times as u8,
                m.align_timing_used as u8,
                m.max_num_visits,
                m.inter_visit_time,
                m.remaining_bits
            ),
            AccessMessage::CandidateFreqSearchReport(m) => {
                let mode_len = match &m.mode_specific {
                    CandidateFreqSearchReportModeSpecific::CdmaPilots(mode) => mode.pilots.len(),
                    CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(bytes) => bytes.len(),
                };
                format!(
                    "CandidateFreqSearchReport(last_srch_msg={}, last_srch_msg_seq={}, search_mode=0x{:x}, mode_len={}, remaining_bits={})",
                    m.last_srch_msg as u8,
                    m.last_srch_msg_seq,
                    m.search_mode,
                    mode_len,
                    m.remaining_bits
                )
            }
            AccessMessage::PeriodicPsmm(m) => format!(
                "PeriodicPsmm(ref_pn={}, pilot_strength={}, keep={}, sf_rx_pwr={}, pilots={}, setpt_incl={}, remaining_bits={})",
                m.ref_pn,
                m.pilot_strength,
                m.keep as u8,
                m.sf_rx_pwr,
                m.pilots.len(),
                m.setpoints.is_some() as u8,
                m.remaining_bits
            ),
            AccessMessage::OuterLoopReport(m) => format!(
                "OuterLoopReport(fch_incl={}, dcch_incl={}, num_sup={}, remaining_bits={})",
                m.fpc_fch_curr_setpt.is_some() as u8,
                m.fpc_dcch_curr_setpt.is_some() as u8,
                m.sch_setpoints.len(),
                m.remaining_bits
            ),
            AccessMessage::ResourceRequest(m) => format!(
                "ResourceRequest(ch_ind={:?}, ext_ch_ind={:?}, remaining_bits={})",
                m.ch_ind, m.ext_ch_ind, m.remaining_bits
            ),
            AccessMessage::ExtReleaseResponse(m) => format!(
                "ExtReleaseResponse(rsc_mode_ind={}, rsci={:?}, rsc_end_time_unit={:?}, rsc_end_time_value={:?}, remaining_bits={})",
                m.rsc_mode_ind as u8,
                m.rsci,
                m.rsc_end_time_unit,
                m.rsc_end_time_value,
                m.remaining_bits
            ),
            AccessMessage::ServiceConnectCompletion(m) => {
                format!("ServiceConnectCompletion(serv_con_seq={})", m.serv_con_seq,)
            }
            AccessMessage::PilotStrengthMeasurement(m) => {
                let pilot_list: Vec<String> = m
                    .pilots
                    .iter()
                    .map(|p| {
                        format!(
                            "PN={} str={} keep={}",
                            p.pilot_pn_phase >> 6, // PN offset = phase / 64
                            p.pilot_strength,
                            p.keep as u8,
                        )
                    })
                    .collect();
                format!(
                    "PSMM(ref_pn={}, str={}, keep={}, pilots=[{}])",
                    m.ref_pn,
                    m.pilot_strength,
                    m.keep as u8,
                    pilot_list.join(", "),
                )
            }
            AccessMessage::PowerMeasurementReport(m) => {
                let dcch_str = if m.dcch_pwr_meas_incl {
                    format!(
                        ", dcch_frames={}, dcch_errors={}",
                        m.dcch_pwr_meas_frames.unwrap_or(0),
                        m.dcch_errors_detected.unwrap_or(0),
                    )
                } else {
                    String::new()
                };
                let sch_str = if m.sch_pwr_meas_incl {
                    format!(
                        ", sch_id={}, sch_frames={}, sch_errors={}",
                        m.sch_id.unwrap_or(0),
                        m.sch_pwr_meas_frames.unwrap_or(0),
                        m.sch_errors_detected.unwrap_or(0),
                    )
                } else {
                    String::new()
                };
                format!(
                    "PMRM(errors={}, frames={}, last_hdm_seq={}, pilots={:?}{}{})",
                    m.errors_detected,
                    m.pwr_meas_frames,
                    m.last_hdm_seq,
                    m.pilot_strengths,
                    dcch_str,
                    sch_str,
                )
            }
            AccessMessage::ServiceRequest(m) => {
                let purpose = match m.req_purpose {
                    0b0000 => "accept",
                    0b0001 => "reject",
                    0b0010 => "propose",
                    _ => "unknown",
                };
                let so_str = m
                    .service_config
                    .as_ref()
                    .and_then(|cfg| cfg.connection_records.first())
                    .map(|cr| format!(", SO={}", cr.service_option))
                    .unwrap_or_default();
                format!(
                    "ServiceRequest(serv_req_seq={}, purpose={}{})",
                    m.serv_req_seq, purpose, so_str
                )
            }
            AccessMessage::ServiceResponse(m) => {
                let purpose = match m.resp_purpose {
                    0b0000 => "accept",
                    0b0001 => "reject",
                    0b0010 => "counter-propose",
                    _ => "unknown",
                };
                let so_str = m
                    .service_config
                    .as_ref()
                    .and_then(|cfg| cfg.connection_records.first())
                    .map(|cr| format!(", SO={}", cr.service_option))
                    .unwrap_or_default();
                format!(
                    "ServiceResponse(serv_req_seq={}, purpose={}{})",
                    m.serv_req_seq, purpose, so_str
                )
            }
        }
    }

    pub fn print(&self) {
        match self {
            AccessMessage::Registration(m) => {
                info!("  Message: Registration Message (RGM)");
                info!("  PD: {}", m.header.pd);
                info!("  REG_TYPE: {}", m.reg_type);
                info!("  SLOT_CYCLE_INDEX: {}", m.slot_cycle_index);
                info!("  MOB_P_REV: {}", m.mob_p_rev);
                info!("  SCM: 0x{:02x}", m.scm);
                info!("  MOB_TERM: {}", m.mob_term);
                info!("  RETURN_CAUSE: {}", m.return_cause);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::Origination(m) => {
                info!("  Message: Origination Message (ORM)");
                info!("  PD: {}", m.header.pd);
                info!("  MOB_TERM: {}", m.mob_term);
                info!("  SLOT_CYCLE_INDEX: {}", m.slot_cycle_index);
                info!("  MOB_P_REV: {}", m.mob_p_rev);
                info!("  SCM: 0x{:02x}", m.scm);
                info!("  REQUEST_MODE: {}", m.request_mode);
                info!("  SPECIAL_SERVICE: {}", m.special_service);
                info!("  SERVICE_OPTION: {:?}", m.service_option);
                info!("  PM: {}", m.pm);
                info!("  DIGIT_MODE: {}", m.digit_mode);
                info!("  NUMBER_TYPE: {:?}", m.number_type);
                info!("  NUMBER_PLAN: {:?}", m.number_plan);
                info!("  MORE_FIELDS: {}", m.more_fields);
                info!("  NUM_FIELDS: {}", m.num_fields);
                info!("  DIGITS: {:?}", m.digits);
                info!("  NAR_AN_CAP: {}", m.nar_an_cap);
                info!("  PACA_REORIG: {}", m.paca_reorig);
                info!("  RETURN_CAUSE: {}", m.return_cause);
                info!("  MORE_RECORDS: {}", m.more_records);
                info!("  ENCRYPTION_SUPPORTED: {:?}", m.encryption_supported);
                info!("  PACA_SUPPORTED: {}", m.paca_supported);
                info!("  NUM_ALT_SO: {}", m.num_alt_so);
                if !m.alt_service_options.is_empty() {
                    info!("  ALT_SERVICE_OPTIONS: {:?}", m.alt_service_options);
                }
                info!("  DRS: {:?}", m.drs);
                info!("  UZID_INCL: {:?}", m.uzid_incl);
                info!("  UZID: {:?}", m.uzid);
                info!("  CH_IND: {:?}", m.ch_ind);
                info!("  SR_ID: {:?}", m.sr_id);
                info!("  OTD_SUPPORTED: {:?}", m.otd_supported);
                info!("  QPCH_SUPPORTED: {:?}", m.qpch_supported);
                info!("  ENHANCED_RC: {:?}", m.enhanced_rc);
                info!("  FOR_RC_PREF: {:?}", m.for_rc_pref);
                info!("  REV_RC_PREF: {:?}", m.rev_rc_pref);
                if let Some(cap) = &m.fch_capability {
                    info!(
                        "  FCH_CAPABILITY: frame5ms={} for_rcs={:?} rev_rcs={:?}",
                        cap.frame_size_5ms_supported, cap.for_supported_rcs, cap.rev_supported_rcs
                    );
                }
                if let Some(cap) = &m.dcch_capability {
                    info!(
                        "  DCCH_CAPABILITY: frame_mode=0b{:02b} for_rcs={:?} rev_rcs={:?}",
                        cap.frame_size_mode, cap.for_supported_rcs, cap.rev_supported_rcs
                    );
                }
                info!("  GEO_LOC_INCL: {:?}", m.geo_loc_incl);
                info!("  GEO_LOC_TYPE: {:?}", m.geo_loc_type);
                info!("  REV_FCH_GATING_REQ: {:?}", m.rev_fch_gating_req);
                info!("  ORIG_REASON: {:?}", m.orig_reason);
                info!("  ORIG_COUNT: {:?}", m.orig_count);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::PageResponse(m) => {
                info!("  Message: Page Response Message (PRM)");
                info!("  PD: {}", m.header.pd);
                info!("  MOB_TERM: {}", m.mob_term);
                info!("  SLOT_CYCLE_INDEX: {}", m.slot_cycle_index);
                info!("  MOB_P_REV: {}", m.mob_p_rev);
                info!("  SCM: 0x{:02x}", m.scm);
                info!("  REQUEST_MODE: {}", m.request_mode);
                info!("  SERVICE_OPTION: {}", m.service_option);
                info!("  PM: {}", m.pm);
                info!("  NAR_AN_CAP: {}", m.nar_an_cap);
                info!("  ENCRYPTION_SUPPORTED: {:?}", m.encryption_supported);
                info!("  NUM_ALT_SO: {}", m.num_alt_so);
                info!("  ALT_SERVICE_OPTIONS: {:?}", m.alt_service_options);
                info!("  UZID_INCL: {:?}", m.uzid_incl);
                info!("  UZID: {:?}", m.uzid);
                info!("  CH_IND: {:?}", m.ch_ind);
                info!("  OTD_SUPPORTED: {:?}", m.otd_supported);
                info!("  QPCH_SUPPORTED: {:?}", m.qpch_supported);
                info!("  ENHANCED_RC: {:?}", m.enhanced_rc);
                info!("  FOR_RC_PREF: {:?}", m.for_rc_pref);
                info!("  REV_RC_PREF: {:?}", m.rev_rc_pref);
                if let Some(cap) = &m.fch_capability {
                    info!(
                        "  FCH_CAPABILITY: frame5ms={} for_rcs={:?} rev_rcs={:?}",
                        cap.frame_size_5ms_supported, cap.for_supported_rcs, cap.rev_supported_rcs
                    );
                }
                if let Some(cap) = &m.dcch_capability {
                    info!(
                        "  DCCH_CAPABILITY: frame_mode=0b{:02b} for_rcs={:?} rev_rcs={:?}",
                        cap.frame_size_mode, cap.for_supported_rcs, cap.rev_supported_rcs
                    );
                }
                info!("  REV_FCH_GATING_REQ: {:?}", m.rev_fch_gating_req);
                info!("  STS_SUPPORTED: {:?}", m.sts_supported);
                info!("  3X_CCH_SUPPORTED: {:?}", m.cch_3x_supported);
                info!("  WLL_INCL: {:?}", m.wll_incl);
                info!("  WLL_DEVICE_TYPE: {:?}", m.wll_device_type);
                info!("  HOOK_STATUS: {:?}", m.hook_status);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::Order(m) => {
                info!("  Message: Order Message (ORDM) - {}", m.order_name());
                info!("  PD: {}", m.header.pd);
                info!("  ORDER: 0b{:06b} ({})", m.order, m.order_name());
                info!("  ADD_RECORD_LEN: {}", m.add_record_len);
                if !m.order_specific.is_empty() {
                    info!("  ORDER_SPECIFIC: {:02x?}", m.order_specific);
                }
                let detail = m.order_detail(WireChannel::ForwardCommon);
                if !detail.is_empty() {
                    info!("  ORDER_DETAIL: {}", detail);
                }
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::DataBurst(m) => {
                info!(
                    "  Message: Data Burst Message (DBM) - {}",
                    m.burst_type_name()
                );
                info!("  PD: {}", m.header.pd);
                info!("  MSG_NUMBER: {}", m.msg_number);
                info!(
                    "  BURST_TYPE: 0b{:06b} ({})",
                    m.burst_type,
                    m.burst_type_name()
                );
                info!("  NUM_MSGS: {}", m.num_msgs);
                info!("  NUM_FIELDS: {}", m.num_fields);
                if !m.fields.is_empty() {
                    info!("  DATA: {:02x?}", m.fields);
                }
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::AuthChallengeResponse(m) => {
                info!("  Message: Authentication Challenge Response Message (AUCRM)");
                info!("  PD: {}", m.header.pd);
                info!("  AUTHU: 0x{:05x}", m.authu);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::StatusResponse(m) => {
                info!("  Message: Status Response Message (SRM)");
                info!("  PD: {}", m.header.pd);
                info!("  QUAL_INFO_TYPE: 0x{:02x}", m.qual_info_type);
                info!("  QUAL_INFO: {:02x?}", m.qual_info);
                info!("  INFO_RECORDS: {}", m.records.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::TmsiAssignmentCompletion(m)
            | AccessMessage::PacaCancel(m)
            | AccessMessage::CallRecoveryRequest(m) => {
                info!("  Message: {}", m.header.message_id.name());
                info!("  PD: {}", m.header.pd);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::ExtStatusResponse(m) => {
                info!("  Message: Extended Status Response Message (ESRM)");
                info!("  PD: {}", m.header.pd);
                info!("  QUAL_INFO_TYPE: 0x{:02x}", m.qual_info_type);
                info!("  QUAL_INFO: {:02x?}", m.qual_info);
                info!("  NUM_INFO_RECORDS: {}", m.num_info_records);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::DeviceInformation(m) => {
                info!("  Message: Device Information Message (DIM)");
                info!("  PD: {}", m.header.pd);
                info!("  WLL_DEVICE_TYPE: {}", m.wll_device_type);
                info!("  NUM_INFO_RECORDS: {}", m.num_info_records);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::SecurityModeRequest(m) => {
                info!("  Message: Security Mode Request Message (SMRM)");
                info!("  PD: {}", m.header.pd);
                info!("  UI_ENCRYPT_SUP: {:?}", m.ui_encrypt_sup);
                info!("  SIG_ENCRYPT_SUP: {:?}", m.sig_encrypt_sup);
                info!("  C_SIG_ENCRYPT_REQ: {:?}", m.c_sig_encrypt_req);
                info!("  NEW_SSEQ_H: {:?}", m.new_sseq_h);
                info!("  NEW_SSEQ_H_SIG: {:?}", m.new_sseq_h_sig);
                info!("  MSG_INT_INFO_INCL: {:?}", m.msg_int_info_incl);
                info!("  SIG_INTEGRITY_SUP_INCL: {:?}", m.sig_integrity_sup_incl);
                info!("  SIG_INTEGRITY_SUP: {:?}", m.sig_integrity_sup);
                info!("  SIG_INTEGRITY_REQ: {:?}", m.sig_integrity_req);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::AuthResponse(m) => {
                info!("  Message: Authentication Response Message (AURSPM)");
                info!("  PD: {}", m.header.pd);
                info!("  RES: {:02x?}", m.res);
                info!("  SIG_INTEGRITY_SUP: {:?}", m.sig_integrity_sup);
                info!("  SIG_INTEGRITY_REQ: {:?}", m.sig_integrity_req);
                info!("  NEW_KEY_ID: {}", m.new_key_id);
                info!("  NEW_SSEQ_H: 0x{:06x}", m.new_sseq_h);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::AuthResync(m) => {
                info!("  Message: Authentication Resynchronization Message (AURSYNM)");
                info!("  PD: {}", m.header.pd);
                info!("  CON_MS_SQN: {:02x?}", m.con_ms_sqn);
                info!("  MAC_S: {:02x?}", m.mac_s);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::Reconnect(m) => {
                info!("  Message: Reconnect Message (RCNM)");
                info!("  PD: {}", m.header.pd);
                info!("  ORIG_IND: {}", m.orig_ind);
                info!("  SYNC_ID_INCL: {}", m.sync_id_incl);
                info!("  SYNC_ID_LEN: {:?}", m.sync_id_len);
                info!("  SYNC_ID: {:02x?}", m.sync_id);
                info!("  SERVICE_OPTION: {:?}", m.service_option);
                info!("  SR_ID: {:?}", m.sr_id);
                info!("  ADD_SERV_INSTANCE_INCL: {:?}", m.add_serv_instance_incl);
                info!("  ADD_SR_ID: {:?}", m.add_sr_ids);
                info!("  SDB_INCL: {:?}", m.sdb_incl);
                info!("  SDB_FIELDS: {:02x?}", m.sdb_fields);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::RadioEnvironment(m) => {
                info!("  Message: Radio Environment Message (REM)");
                info!("  PD: {}", m.header.pd);
                info!("  MODE_DISABLED: {}", m.mode_disabled);
                info!("  TKZ_MODE_IND: {}", m.tkz_mode_ind);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::GeneralExtension(m) => {
                info!("  Message: General Extension Message (GEM)");
                info!("  PD: {}", m.header.pd);
                info!("  NUM_GE_REC: {}", m.num_ge_records);
                info!("  MESSAGE_TYPE: 0x{:02x}", m.message_type);
                info!("  MESSAGE_REC_BITS: {}", m.message_record.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::FlashWithInfo(m) => {
                info!("  Message: Flash With Information Message (FWIM)");
                info!("  PD: {}", m.header.pd);
                info!("  Information records: {}", m.records.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::SendBurstDtmf(m) => {
                info!("  Message: Send Burst DTMF Message (BDTMFM)");
                info!("  PD: {}", m.header.pd);
                info!("  NUM_DIGITS: {}", m.digits.len());
                info!("  DTMF_ON_LENGTH: {}", m.dtmf_on_length);
                info!("  DTMF_OFF_LENGTH: {}", m.dtmf_off_length);
                info!("  DIGITS: {:x?}", m.digits);
                info!("  CON_REF: {:?}", m.con_ref);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::Status(m) => {
                info!("  Message: Status Message (STM)");
                info!("  PD: {}", m.header.pd);
                info!("  RECORD_TYPE: 0x{:02x}", m.record.record_type);
                info!("  RECORD_LEN: {}", m.record.data.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::OriginationContinuation(m) => {
                info!("  Message: Origination Continuation Message (ORCM)");
                info!("  PD: {}", m.header.pd);
                info!("  DIGIT_MODE: {}", m.digit_mode);
                info!("  NUM_FIELDS: {}", m.digits.len());
                info!("  DIGITS: {:02x?}", m.digits);
                info!("  Information records: {}", m.records.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::HandoffCompletion(m) => {
                info!("  Message: Handoff Completion Message (HOCM)");
                info!("  PD: {}", m.header.pd);
                info!("  LAST_HDM_SEQ: {}", m.last_hdm_seq);
                info!("  PILOT_PN: {:?}", m.pilot_pns);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::ParametersResponse(m) => {
                info!("  Message: Parameters Response Message (PRSM)");
                info!("  PD: {}", m.header.pd);
                info!("  Parameter records: {}", m.records.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::ServiceOptionControl(m) => {
                info!("  Message: Service Option Control Message (SOCM)");
                info!("  PD: {}", m.header.pd);
                info!("  CON_REF: {}", m.con_ref);
                info!("  SERVICE_OPTION: {}", m.service_option);
                info!("  CTL_REC_LEN: {}", m.control_record.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::SupplementalChannelRequest(m) => {
                info!("  Message: Supplemental Channel Request Message (SCRM)");
                info!("  PD: {}", m.header.pd);
                info!("  SIZE_OF_REQ_BLOB: {}", m.req_blob.len());
                info!("  USE_SCRM_SEQ_NUM: {}", m.scrm_seq_num.is_some() as u8);
                if let Some(seq_num) = m.scrm_seq_num {
                    info!("  SCRM_SEQ_NUM: {}", seq_num);
                }
                if let Some(measurements) = &m.measurements {
                    info!("  REF_PN: {}", measurements.ref_pn);
                    info!("  PILOT_STRENGTH: {}", measurements.pilot_strength);
                    info!("  NUM_ACT_PN: {}", measurements.active_pilots.len());
                    info!(
                        "  NUM_NGHBR_PN: {:?}",
                        measurements.neighbor_pilots.as_ref().map(Vec::len)
                    );
                }
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::CandidateFreqSearchResponse(m) => {
                info!("  Message: Candidate Frequency Search Response Message (CFSRSM)");
                info!("  PD: {}", m.header.pd);
                info!("  LAST_CFSRM_SEQ: {}", m.last_cfsrm_seq);
                info!("  TOTAL_OFF_TIME_FWD: {}", m.total_off_time_fwd);
                info!("  MAX_OFF_TIME_FWD: {}", m.max_off_time_fwd);
                info!("  TOTAL_OFF_TIME_REV: {}", m.total_off_time_rev);
                info!("  MAX_OFF_TIME_REV: {}", m.max_off_time_rev);
                info!("  PCG_OFF_TIMES: {}", m.pcg_off_times as u8);
                info!("  ALIGN_TIMING_USED: {}", m.align_timing_used as u8);
                info!("  MAX_NUM_VISITS: {:?}", m.max_num_visits);
                info!("  INTER_VISIT_TIME: {:?}", m.inter_visit_time);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::CandidateFreqSearchReport(m) => {
                info!("  Message: Candidate Frequency Search Report Message (CFSRPM)");
                info!("  PD: {}", m.header.pd);
                info!("  LAST_SRCH_MSG: {}", m.last_srch_msg as u8);
                info!("  LAST_SRCH_MSG_SEQ: {}", m.last_srch_msg_seq);
                info!("  SEARCH_MODE: 0x{:x}", m.search_mode);
                match &m.mode_specific {
                    CandidateFreqSearchReportModeSpecific::CdmaPilots(mode) => {
                        info!("  BAND_CLASS: {}", mode.band_class);
                        info!("  CDMA_FREQ: {}", mode.cdma_freq);
                        info!("  NUM_PILOTS: {}", mode.pilots.len());
                    }
                    CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(bytes) => {
                        info!("  External mode-specific octets: {}", bytes.len());
                    }
                }
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::PeriodicPsmm(m) => {
                info!("  Message: Periodic Pilot Strength Measurement Message (PPSMM)");
                info!("  PD: {}", m.header.pd);
                info!("  REF_PN: {}", m.ref_pn);
                info!("  PILOT_STRENGTH: {}", m.pilot_strength);
                info!("  KEEP: {}", m.keep as u8);
                info!("  SF_RX_PWR: {}", m.sf_rx_pwr);
                info!("  NUM_PILOT: {}", m.pilots.len());
                info!("  SETPT_INCL: {}", m.setpoints.is_some() as u8);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::OuterLoopReport(m) => {
                info!("  Message: Outer Loop Report Message (OLRM)");
                info!("  PD: {}", m.header.pd);
                info!("  FCH_INCL: {}", m.fpc_fch_curr_setpt.is_some() as u8);
                info!("  DCCH_INCL: {}", m.fpc_dcch_curr_setpt.is_some() as u8);
                info!("  NUM_SUP: {}", m.sch_setpoints.len());
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::ResourceRequest(m) => {
                info!("  Message: Resource Request Message (RRM)");
                info!("  PD: {}", m.header.pd);
                info!("  CH_IND_INCL: {}", m.ch_ind.is_some() as u8);
                info!("  CH_IND: {:?}", m.ch_ind);
                info!("  EXT_CH_IND: {:?}", m.ext_ch_ind);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::ExtReleaseResponse(m) => {
                info!("  Message: Extended Release Response Message (ERRM)");
                info!("  PD: {}", m.header.pd);
                info!("  RSC_MODE_IND: {}", m.rsc_mode_ind as u8);
                info!("  RSCI: {:?}", m.rsci);
                info!("  RSC_END_TIME_UNIT: {:?}", m.rsc_end_time_unit);
                info!("  RSC_END_TIME_VALUE: {:?}", m.rsc_end_time_value);
                info!("  Remaining undecoded bits: {}", m.remaining_bits);
            }
            AccessMessage::ServiceConnectCompletion(m) => {
                info!("  Message: Service Connect Completion Message");
                info!("  SERV_CON_SEQ: {}", m.serv_con_seq);
            }
            AccessMessage::PilotStrengthMeasurement(m) => {
                info!("  Message: Pilot Strength Measurement Message (PSMM)");
                info!("  REF_PN: {}", m.ref_pn);
                info!("  PILOT_STRENGTH: {}", m.pilot_strength);
                info!("  KEEP: {}", m.keep);
                for (i, p) in m.pilots.iter().enumerate() {
                    info!(
                        "  Pilot {}: PN_PHASE={} (PN={}) STRENGTH={} KEEP={}",
                        i,
                        p.pilot_pn_phase,
                        p.pilot_pn_phase >> 6,
                        p.pilot_strength,
                        p.keep
                    );
                }
            }
            AccessMessage::PowerMeasurementReport(m) => {
                info!("  Message: Power Measurement Report Message (PMRM)");
                info!("  ERRORS_DETECTED: {}", m.errors_detected);
                info!("  PWR_MEAS_FRAMES: {}", m.pwr_meas_frames);
                info!("  LAST_HDM_SEQ: {}", m.last_hdm_seq);
                info!("  NUM_PILOTS: {}", m.pilot_strengths.len());
                for (i, &strength) in m.pilot_strengths.iter().enumerate() {
                    info!("  PILOT_STRENGTH[{}]: {}", i, strength);
                }
                info!("  DCCH_PWR_MEAS_INCL: {}", m.dcch_pwr_meas_incl as u8);
                if m.dcch_pwr_meas_incl {
                    info!(
                        "  DCCH_PWR_MEAS_FRAMES: {}",
                        m.dcch_pwr_meas_frames.unwrap_or(0)
                    );
                    info!(
                        "  DCCH_ERRORS_DETECTED: {}",
                        m.dcch_errors_detected.unwrap_or(0)
                    );
                }
                info!("  SCH_PWR_MEAS_INCL: {}", m.sch_pwr_meas_incl as u8);
                if m.sch_pwr_meas_incl {
                    info!("  SCH_ID: {}", m.sch_id.unwrap_or(0));
                    info!(
                        "  SCH_PWR_MEAS_FRAMES: {}",
                        m.sch_pwr_meas_frames.unwrap_or(0)
                    );
                    info!(
                        "  SCH_ERRORS_DETECTED: {}",
                        m.sch_errors_detected.unwrap_or(0)
                    );
                }
            }
            AccessMessage::ServiceRequest(m) => {
                info!("  Message: Service Request Message (SRQM)");
                info!("  SERV_REQ_SEQ: {}", m.serv_req_seq);
                info!(
                    "  REQ_PURPOSE: {} (0b{:04b})",
                    match m.req_purpose {
                        0b0000 => "accept",
                        0b0001 => "reject",
                        0b0010 => "propose",
                        _ => "unknown",
                    },
                    m.req_purpose
                );
                if let Some(ref cfg) = m.service_config {
                    info!("  FOR_MUX_OPTION: 0x{:04X}", cfg.for_mux_option);
                    info!("  REV_MUX_OPTION: 0x{:04X}", cfg.rev_mux_option);
                    info!("  FOR_RATES: 0x{:02X}", cfg.for_rates);
                    info!("  REV_RATES: 0x{:02X}", cfg.rev_rates);
                    for (i, cr) in cfg.connection_records.iter().enumerate() {
                        info!(
                            "  Connection {}: CON_REF={} SO={} FOR_TRAFFIC={} REV_TRAFFIC={} SR_ID={}",
                            i,
                            cr.con_ref,
                            cr.service_option,
                            cr.for_traffic,
                            cr.rev_traffic,
                            cr.sr_id
                        );
                    }
                    if let Some(rc) = cfg.for_fch_rc {
                        info!("  FOR_FCH_RC: {}", rc);
                    }
                    if let Some(rc) = cfg.rev_fch_rc {
                        info!("  REV_FCH_RC: {}", rc);
                    }
                }
            }
            AccessMessage::ServiceResponse(m) => {
                info!("  Message: Service Response Message (SRPM)");
                info!("  SERV_REQ_SEQ: {}", m.serv_req_seq);
                info!(
                    "  RESP_PURPOSE: {} (0b{:04b})",
                    match m.resp_purpose {
                        0b0000 => "accept",
                        0b0001 => "reject",
                        0b0010 => "counter-propose",
                        _ => "unknown",
                    },
                    m.resp_purpose
                );
                if let Some(ref cfg) = m.service_config {
                    info!("  FOR_MUX_OPTION: 0x{:04X}", cfg.for_mux_option);
                    info!("  REV_MUX_OPTION: 0x{:04X}", cfg.rev_mux_option);
                    info!("  FOR_RATES: 0x{:02X}", cfg.for_rates);
                    info!("  REV_RATES: 0x{:02X}", cfg.rev_rates);
                    for (i, cr) in cfg.connection_records.iter().enumerate() {
                        info!(
                            "  Connection {}: CON_REF={} SO={} FOR_TRAFFIC={} REV_TRAFFIC={} SR_ID={}",
                            i,
                            cr.con_ref,
                            cr.service_option,
                            cr.for_traffic,
                            cr.rev_traffic,
                            cr.sr_id
                        );
                    }
                    if let Some(rc) = cfg.for_fch_rc {
                        info!("  FOR_FCH_RC: {}", rc);
                    }
                    if let Some(rc) = cfg.rev_fch_rc {
                        info!("  REV_FCH_RC: {}", rc);
                    }
                }
            }
        }
    }
}

fn read(bs: &mut Bitstream, bits: usize, name: &str) -> Result<u64, String> {
    bs.read_bits(bits)
        .map_err(|_| format!("EOF reading {} ({} bits)", name, bits))
}

fn read_octets(bs: &mut Bitstream, count: usize, name: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(count);
    for idx in 0..count {
        out.push(read(bs, 8, &format!("{name}[{idx}]"))? as u8);
    }
    Ok(out)
}

fn read_info_records(
    bs: &mut Bitstream,
    count: usize,
    prefix: &str,
) -> Result<Vec<AccessInfoRecord>, String> {
    let mut records = Vec::with_capacity(count);
    for idx in 0..count {
        let record_type = read(bs, 8, &format!("{prefix}[{idx}].RECORD_TYPE"))? as u8;
        let record_len = read(bs, 8, &format!("{prefix}[{idx}].RECORD_LEN"))? as usize;
        let data = read_octets(bs, record_len, &format!("{prefix}[{idx}].RECORD"))?;
        records.push(AccessInfoRecord { record_type, data });
    }
    Ok(records)
}

fn read_info_records_until_padding(
    bs: &mut Bitstream,
    prefix: &str,
) -> Result<Vec<AccessInfoRecord>, String> {
    let mut records = Vec::new();
    while bs.len() >= 16 {
        let idx = records.len();
        let record_type = read(bs, 8, &format!("{prefix}[{idx}].RECORD_TYPE"))? as u8;
        let record_len = read(bs, 8, &format!("{prefix}[{idx}].RECORD_LEN"))? as usize;
        let data = read_octets(bs, record_len, &format!("{prefix}[{idx}].RECORD"))?;
        records.push(AccessInfoRecord { record_type, data });
    }
    consume_zero_pdu_padding(bs, prefix)?;
    Ok(records)
}

fn consume_zero_pdu_padding(bs: &mut Bitstream, prefix: &str) -> Result<(), String> {
    if bs.len() > 7 {
        return Err(format!(
            "{prefix} trailing bits exceed PDU padding: {} bits",
            bs.len()
        ));
    }
    if bs.bits().iter().any(|bit| *bit != 0) {
        return Err(format!(
            "{prefix} PDU padding contains non-zero bits: {}",
            bs
        ));
    }
    if !bs.is_empty() {
        let padding_bits = bs.len();
        bs.drain(0..padding_bits);
    }
    Ok(())
}

fn finish_rdsch_access_padding(
    mut message: AccessMessage,
    bs: &mut Bitstream,
    prefix: &str,
) -> Result<AccessMessage, String> {
    consume_zero_pdu_padding(bs, prefix)?;
    match &mut message {
        AccessMessage::AuthChallengeResponse(m) => m.remaining_bits = bs.len(),
        AccessMessage::StatusResponse(m) => m.remaining_bits = bs.len(),
        AccessMessage::TmsiAssignmentCompletion(m) => m.remaining_bits = bs.len(),
        AccessMessage::DeviceInformation(m) => m.remaining_bits = bs.len(),
        AccessMessage::SecurityModeRequest(m) => m.remaining_bits = bs.len(),
        AccessMessage::AuthResponse(m) => m.remaining_bits = bs.len(),
        AccessMessage::AuthResync(m) => m.remaining_bits = bs.len(),
        _ => {}
    }
    Ok(message)
}

fn ensure_count(name: &str, len: usize, max: usize) -> Result<(), String> {
    if len > max {
        Err(format!("{name} length {len} exceeds {max}"))
    } else {
        Ok(())
    }
}

fn write_octets(bs: &mut Bitstream, bytes: &[u8]) {
    for &byte in bytes {
        bs.write_u8(byte, 8);
    }
}

fn write_info_records(
    bs: &mut Bitstream,
    records: &[AccessInfoRecord],
    len_bits: usize,
) -> Result<(), String> {
    for record in records {
        ensure_count("RECORD_LEN", record.data.len(), (1usize << len_bits) - 1)?;
        bs.write_u8(record.record_type, 8);
        bs.write_u64(record.data.len() as u64, len_bits);
        write_octets(bs, &record.data);
    }
    Ok(())
}

fn validate_dtmf_digit(digit: u8) -> Result<(), String> {
    match digit {
        0x01..=0x09 | 0x0A..=0x0C => Ok(()),
        _ => Err(format!(
            "BDTMFM DIGIT code 0x{digit:x} is reserved by C.S0005-E Table 2.7.1.3.2.4-4"
        )),
    }
}

fn validate_send_burst_dtmf_fields(
    dtmf_on_length: u8,
    dtmf_off_length: u8,
    digits: &[u8],
) -> Result<(), String> {
    if dtmf_on_length > 0b101 {
        return Err(format!(
            "BDTMFM DTMF_ON_LENGTH=0b{dtmf_on_length:03b} is reserved by C.S0005-E Table 2.7.2.3.2.7-1"
        ));
    }
    if dtmf_off_length > 0b011 {
        return Err(format!(
            "BDTMFM DTMF_OFF_LENGTH=0b{dtmf_off_length:03b} is reserved by C.S0005-E Table 2.7.2.3.2.7-2"
        ));
    }
    ensure_count("NUM_DIGITS", digits.len(), u8::MAX as usize)?;
    for &digit in digits {
        validate_dtmf_digit(digit)?;
    }
    Ok(())
}

fn validate_origination_continuation_fields(digit_mode: bool, digits: &[u8]) -> Result<(), String> {
    ensure_count("NUM_FIELDS", digits.len(), u8::MAX as usize)?;
    if digit_mode {
        for &digit in digits {
            if digit & 0x80 != 0 {
                return Err(format!(
                    "ORCM ASCII CHAR 0x{digit:02x} has MSB set; C.S0005-E 2.7.2.3.2.9 requires MSB zero"
                ));
            }
        }
    } else {
        for &digit in digits {
            validate_dtmf_digit(digit)?;
        }
    }
    Ok(())
}

fn validate_handoff_completion_fields(last_hdm_seq: u8, pilot_pns: &[u16]) -> Result<(), String> {
    if last_hdm_seq > 0b11 {
        return Err(format!(
            "HOCM LAST_HDM_SEQ value {last_hdm_seq} exceeds 2 bits"
        ));
    }
    if pilot_pns.is_empty() {
        return Err("HOCM requires one or more PILOT_PN fields".to_string());
    }
    for &pilot_pn in pilot_pns {
        if pilot_pn > 0x01ff {
            return Err(format!("HOCM PILOT_PN value {pilot_pn} exceeds 9 bits"));
        }
    }
    Ok(())
}

fn validate_parameters_response_records(
    records: &[ParametersResponseRecord],
) -> Result<(), String> {
    if records.is_empty() {
        return Err("PRSM requires one or more parameter records".to_string());
    }
    for record in records {
        if record.parameter_len > 0x03ff {
            return Err(format!(
                "PRSM PARAMETER_LEN value {} exceeds 10 bits",
                record.parameter_len
            ));
        }
        if record.parameter_len == 0x03ff {
            if !record.parameter.is_empty() {
                return Err(
                    "PRSM PARAMETER must be omitted when PARAMETER_LEN is all ones".to_string(),
                );
            }
        } else {
            let expected_bits = record.parameter_len as usize + 1;
            if record.parameter.len() != expected_bits {
                return Err(format!(
                    "PRSM PARAMETER length {} does not match PARAMETER_LEN+1 ({expected_bits})",
                    record.parameter.len()
                ));
            }
        }
    }
    Ok(())
}

fn validate_service_option_control_record(control_record: &[u8]) -> Result<(), String> {
    if control_record.len() > u8::MAX as usize {
        return Err(format!(
            "SOCM CTL_REC_LEN {} exceeds 8 bits",
            control_record.len()
        ));
    }
    Ok(())
}

fn validate_aux_pilot_record(
    prefix: &str,
    record: &SupplementalChannelPilotRecord,
    field_name: &str,
) -> Result<(), String> {
    if record.pilot_rec_type != 0 {
        return Err(format!(
            "{prefix} {field_name} reserved PILOT_REC_TYPE 0b{:03b}",
            record.pilot_rec_type
        ));
    }
    if record.type_specific_fields.len() > 7 {
        return Err(format!(
            "{prefix} {field_name} RECORD_LEN {} exceeds 3 bits",
            record.type_specific_fields.len()
        ));
    }

    let mut bs = Bitstream::new_bytes(&record.type_specific_fields);
    let _qof = read(&mut bs, 2, &format!("{field_name}.QOF"))?;
    let walsh_length = read(&mut bs, 3, &format!("{field_name}.WALSH_LENGTH"))? as u8;
    if walsh_length > 0b011 {
        return Err(format!(
            "{prefix} {field_name} WALSH_LENGTH 0b{walsh_length:03b} is reserved"
        ));
    }
    let pilot_walsh_bits = walsh_length as usize + 6;
    let _pilot_walsh = read(
        &mut bs,
        pilot_walsh_bits,
        &format!("{field_name}.PILOT_WALSH"),
    )?;
    if bs.bits().iter().any(|bit| *bit != 0) {
        return Err(format!("{prefix} {field_name} RESERVED bits must be zero"));
    }
    Ok(())
}

fn validate_scrm_pilot_record(
    record: &SupplementalChannelPilotRecord,
    field_name: &str,
) -> Result<(), String> {
    validate_aux_pilot_record("SCRM", record, field_name)
}

fn validate_scrm_pilot_report(
    report: &SupplementalChannelPilotReport,
    field_name: &str,
) -> Result<(), String> {
    if report.pn_phase > 0x7fff {
        return Err(format!(
            "SCRM {field_name} PN_PHASE value {} exceeds 15 bits",
            report.pn_phase
        ));
    }
    if report.pilot_strength > 0x3f {
        return Err(format!(
            "SCRM {field_name} PILOT_STRENGTH value {} exceeds 6 bits",
            report.pilot_strength
        ));
    }
    if let Some(record) = &report.pilot_record {
        validate_scrm_pilot_record(record, &format!("{field_name}.PILOT_REC"))?;
    }
    Ok(())
}

fn validate_supplemental_channel_request(
    msg: &SupplementalChannelRequestMessage,
) -> Result<(), String> {
    if msg.req_blob.len() > 0x0f {
        return Err(format!(
            "SCRM SIZE_OF_REQ_BLOB {} exceeds 4 bits",
            msg.req_blob.len()
        ));
    }
    if let Some(seq_num) = msg.scrm_seq_num {
        if seq_num > 0x0f {
            return Err(format!("SCRM SCRM_SEQ_NUM value {seq_num} exceeds 4 bits"));
        }
    }

    let Some(measurements) = &msg.measurements else {
        if msg.req_blob.is_empty() && msg.scrm_seq_num.is_none() {
            return Ok(());
        }
        return Err(
            "SCRM pilot measurements are required unless SIZE_OF_REQ_BLOB=0 and USE_SCRM_SEQ_NUM=0"
                .to_string(),
        );
    };

    if msg.req_blob.is_empty() && msg.scrm_seq_num.is_none() {
        return Err(
            "SCRM pilot measurements must be omitted when SIZE_OF_REQ_BLOB=0 and USE_SCRM_SEQ_NUM=0"
                .to_string(),
        );
    }
    if measurements.ref_pn > 0x01ff {
        return Err(format!(
            "SCRM REF_PN value {} exceeds 9 bits",
            measurements.ref_pn
        ));
    }
    if measurements.pilot_strength > 0x3f {
        return Err(format!(
            "SCRM PILOT_STRENGTH value {} exceeds 6 bits",
            measurements.pilot_strength
        ));
    }
    if measurements.active_pilots.len() > 7 {
        return Err(format!(
            "SCRM NUM_ACT_PN {} exceeds 3 bits",
            measurements.active_pilots.len()
        ));
    }
    for (idx, pilot) in measurements.active_pilots.iter().enumerate() {
        validate_scrm_pilot_report(pilot, &format!("ACT_PN[{idx}]"))?;
    }

    match &measurements.neighbor_pilots {
        Some(neighbor_pilots) => {
            if msg.req_blob.is_empty() {
                return Err("SCRM NUM_NGHBR_PN must be omitted when SIZE_OF_REQ_BLOB=0".to_string());
            }
            if neighbor_pilots.len() > 7 {
                return Err(format!(
                    "SCRM NUM_NGHBR_PN {} exceeds 3 bits",
                    neighbor_pilots.len()
                ));
            }
            if measurements.active_pilots.len() + neighbor_pilots.len() > 8 {
                return Err(format!(
                    "SCRM NUM_ACT_PN + NUM_NGHBR_PN {} exceeds 8",
                    measurements.active_pilots.len() + neighbor_pilots.len()
                ));
            }
            for (idx, pilot) in neighbor_pilots.iter().enumerate() {
                validate_scrm_pilot_report(pilot, &format!("NGHBR_PN[{idx}]"))?;
            }
        }
        None if !msg.req_blob.is_empty() => {
            return Err(
                "SCRM NUM_NGHBR_PN is required when SIZE_OF_REQ_BLOB is nonzero".to_string(),
            );
        }
        None => {}
    }

    if msg.req_blob.is_empty() {
        if measurements.ref_pilot_record.is_some() {
            return Err(
                "SCRM REF_PILOT_REC_INCL must be omitted when SIZE_OF_REQ_BLOB=0".to_string(),
            );
        }
    } else if let Some(record) = &measurements.ref_pilot_record {
        validate_scrm_pilot_record(record, "REF_PILOT_REC")?;
    }

    Ok(())
}

fn validate_candidate_freq_search_response(
    msg: &CandidateFreqSearchResponseMessage,
) -> Result<(), String> {
    if msg.last_cfsrm_seq > 0b11 {
        return Err(format!(
            "CFSRSM LAST_CFSRM_SEQ value {} exceeds 2 bits",
            msg.last_cfsrm_seq
        ));
    }
    for (name, value) in [
        ("TOTAL_OFF_TIME_FWD", msg.total_off_time_fwd),
        ("MAX_OFF_TIME_FWD", msg.max_off_time_fwd),
        ("TOTAL_OFF_TIME_REV", msg.total_off_time_rev),
        ("MAX_OFF_TIME_REV", msg.max_off_time_rev),
    ] {
        if value > 0x3f {
            return Err(format!("CFSRSM {name} value {value} exceeds 6 bits"));
        }
    }

    if msg.align_timing_used {
        let max_num_visits = msg.max_num_visits.ok_or_else(|| {
            "CFSRSM MAX_NUM_VISITS is required when ALIGN_TIMING_USED=1".to_string()
        })?;
        if max_num_visits > 0x1f {
            return Err(format!(
                "CFSRSM MAX_NUM_VISITS value {max_num_visits} exceeds 5 bits"
            ));
        }
        if max_num_visits == 0 {
            if msg.inter_visit_time.is_some() {
                return Err(
                    "CFSRSM INTER_VISIT_TIME must be omitted when MAX_NUM_VISITS=0".to_string(),
                );
            }
        } else {
            let inter_visit_time = msg.inter_visit_time.ok_or_else(|| {
                "CFSRSM INTER_VISIT_TIME is required when MAX_NUM_VISITS is nonzero".to_string()
            })?;
            if inter_visit_time > 0x3f {
                return Err(format!(
                    "CFSRSM INTER_VISIT_TIME value {inter_visit_time} exceeds 6 bits"
                ));
            }
        }
    } else {
        if msg.max_num_visits.is_some() {
            return Err(
                "CFSRSM MAX_NUM_VISITS must be omitted when ALIGN_TIMING_USED=0".to_string(),
            );
        }
        if msg.inter_visit_time.is_some() {
            return Err(
                "CFSRSM INTER_VISIT_TIME must be omitted when ALIGN_TIMING_USED=0".to_string(),
            );
        }
    }

    Ok(())
}

fn validate_candidate_freq_search_report_pilot(
    pilot: &CandidateFreqSearchReportPilot,
    idx: usize,
) -> Result<(), String> {
    if pilot.pilot_pn_phase > 0x7fff {
        return Err(format!(
            "CFSRPM PILOT_PN_PHASE[{idx}] value {} exceeds 15 bits",
            pilot.pilot_pn_phase
        ));
    }
    if pilot.pilot_strength > 0x3f {
        return Err(format!(
            "CFSRPM PILOT_STRENGTH[{idx}] value {} exceeds 6 bits",
            pilot.pilot_strength
        ));
    }
    if let Some(record) = &pilot.pilot_record {
        validate_aux_pilot_record("CFSRPM", record, &format!("PILOT_REC[{idx}]"))?;
    }
    Ok(())
}

fn validate_candidate_freq_search_cdma_mode(
    mode: &CandidateFreqSearchCdmaPilots,
) -> Result<(), String> {
    if mode.band_class > 0x1f {
        return Err(format!(
            "CFSRPM BAND_CLASS value {} exceeds 5 bits",
            mode.band_class
        ));
    }
    if mode.cdma_freq > 0x07ff {
        return Err(format!(
            "CFSRPM CDMA_FREQ value {} exceeds 11 bits",
            mode.cdma_freq
        ));
    }
    if mode.sf_total_rx_pwr > 0x1f {
        return Err(format!(
            "CFSRPM SF_TOTAL_RX_PWR value {} exceeds 5 bits",
            mode.sf_total_rx_pwr
        ));
    }
    if mode.cf_total_rx_pwr > 0x1f {
        return Err(format!(
            "CFSRPM CF_TOTAL_RX_PWR value {} exceeds 5 bits",
            mode.cf_total_rx_pwr
        ));
    }
    if mode.pilots.len() > 0x3f {
        return Err(format!(
            "CFSRPM NUM_PILOTS {} exceeds 6 bits",
            mode.pilots.len()
        ));
    }
    for (idx, pilot) in mode.pilots.iter().enumerate() {
        validate_candidate_freq_search_report_pilot(pilot, idx)?;
    }
    Ok(())
}

fn validate_candidate_freq_search_report(
    msg: &CandidateFreqSearchReportMessage,
) -> Result<(), String> {
    if msg.last_srch_msg_seq > 0b11 {
        return Err(format!(
            "CFSRPM LAST_SRCH_MSG_SEQ value {} exceeds 2 bits",
            msg.last_srch_msg_seq
        ));
    }
    match (&msg.search_mode, &msg.mode_specific) {
        (0, CandidateFreqSearchReportModeSpecific::CdmaPilots(mode)) => {
            validate_candidate_freq_search_cdma_mode(mode)
        }
        (2, CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(bytes)) => {
            if bytes.len() > u8::MAX as usize {
                return Err(format!(
                    "CFSRPM MODE_SPECIFIC_LEN {} exceeds 8 bits",
                    bytes.len()
                ));
            }
            Ok(())
        }
        (0, _) => Err("CFSRPM SEARCH_MODE=0000 requires CDMA pilot mode fields".to_string()),
        (2, _) => Err("CFSRPM SEARCH_MODE=0010 requires external DS neighbor bytes".to_string()),
        (search_mode, _) => Err(format!(
            "CFSRPM SEARCH_MODE 0b{search_mode:04b} is reserved"
        )),
    }
}

fn write_candidate_freq_search_cdma_mode(
    bs: &mut Bitstream,
    mode: &CandidateFreqSearchCdmaPilots,
) -> Result<(), String> {
    validate_candidate_freq_search_cdma_mode(mode)?;
    bs.write_u8(mode.band_class, 5);
    bs.write_u32(mode.cdma_freq as u32, 11);
    bs.write_u8(mode.sf_total_rx_pwr, 5);
    bs.write_u8(mode.cf_total_rx_pwr, 5);
    bs.write_u8(mode.pilots.len() as u8, 6);
    for pilot in &mode.pilots {
        bs.write_u32(pilot.pilot_pn_phase as u32, 15);
        bs.write_u8(pilot.pilot_strength, 6);
        bs.write_u8(0, 3);
    }
    for pilot in &mode.pilots {
        write_scrm_pilot_record(bs, &pilot.pilot_record);
    }
    pad_access_reserved_to_octet(bs);
    Ok(())
}

fn validate_periodic_psmm_pilot(pilot: &PeriodicPsmmPilot, idx: usize) -> Result<(), String> {
    if pilot.pilot_pn_phase > 0x7fff {
        return Err(format!(
            "PPSMM PILOT_PN_PHASE[{idx}] value {} exceeds 15 bits",
            pilot.pilot_pn_phase
        ));
    }
    if pilot.pilot_strength > 0x3f {
        return Err(format!(
            "PPSMM PILOT_STRENGTH[{idx}] value {} exceeds 6 bits",
            pilot.pilot_strength
        ));
    }
    if let Some(record) = &pilot.pilot_record {
        validate_aux_pilot_record("PPSMM", record, &format!("PILOT_REC[{idx}]"))?;
    }
    Ok(())
}

fn validate_periodic_psmm(msg: &PeriodicPsmmMessage) -> Result<(), String> {
    if msg.ref_pn > 0x01ff {
        return Err(format!("PPSMM REF_PN value {} exceeds 9 bits", msg.ref_pn));
    }
    if msg.pilot_strength > 0x3f {
        return Err(format!(
            "PPSMM PILOT_STRENGTH value {} exceeds 6 bits",
            msg.pilot_strength
        ));
    }
    if msg.sf_rx_pwr > 0x1f {
        return Err(format!(
            "PPSMM SF_RX_PWR value {} exceeds 5 bits",
            msg.sf_rx_pwr
        ));
    }
    if msg.pilots.len() > 0x0f {
        return Err(format!(
            "PPSMM NUM_PILOT {} exceeds 4 bits",
            msg.pilots.len()
        ));
    }
    for (idx, pilot) in msg.pilots.iter().enumerate() {
        validate_periodic_psmm_pilot(pilot, idx)?;
    }
    if let Some(setpoints) = &msg.setpoints {
        if setpoints.sch_setpoints.len() > 0b11 {
            return Err(format!(
                "PPSMM NUM_SUP {} exceeds 2 bits",
                setpoints.sch_setpoints.len()
            ));
        }
        for (idx, sch) in setpoints.sch_setpoints.iter().enumerate() {
            if sch.sch_id > 1 {
                return Err(format!(
                    "PPSMM SCH_ID[{idx}] value {} exceeds 1 bit",
                    sch.sch_id
                ));
            }
        }
    }
    Ok(())
}

fn validate_outer_loop_sch_setpoints(
    prefix: &str,
    sch_setpoints: &[PeriodicPsmmSchSetpoint],
) -> Result<(), String> {
    if sch_setpoints.len() > 0b11 {
        return Err(format!(
            "{prefix} NUM_SUP {} exceeds 2 bits",
            sch_setpoints.len()
        ));
    }
    for (idx, sch) in sch_setpoints.iter().enumerate() {
        if sch.sch_id > 1 {
            return Err(format!(
                "{prefix} SCH_ID[{idx}] value {} exceeds 1 bit",
                sch.sch_id
            ));
        }
    }
    Ok(())
}

fn validate_outer_loop_report(msg: &OuterLoopReportMessage) -> Result<(), String> {
    validate_outer_loop_sch_setpoints("OLRM", &msg.sch_setpoints)
}

fn validate_resource_request(msg: &ResourceRequestMessage) -> Result<(), String> {
    match msg.ch_ind {
        None => {
            if msg.ext_ch_ind.is_some() {
                return Err("RRM EXT_CH_IND requires CH_IND=00".to_string());
            }
        }
        Some(ch_ind) if ch_ind > 0b11 => {
            return Err(format!("RRM CH_IND value {ch_ind:#04b} exceeds 2 bits"));
        }
        Some(0) => {
            let ext_ch_ind = msg
                .ext_ch_ind
                .ok_or_else(|| "RRM CH_IND=00 requires EXT_CH_IND".to_string())?;
            if !is_valid_origination_ext_ch_ind(ext_ch_ind) {
                return Err(format!(
                    "RRM EXT_CH_IND value {ext_ch_ind:#07b} is reserved or invalid"
                ));
            }
        }
        Some(_) => {
            if msg.ext_ch_ind.is_some() {
                return Err("RRM EXT_CH_IND is only present when CH_IND=00".to_string());
            }
        }
    }
    Ok(())
}

fn validate_ext_release_response(msg: &ExtReleaseResponseMessage) -> Result<(), String> {
    if msg.rsc_mode_ind {
        let rsci = msg
            .rsci
            .ok_or_else(|| "ERRM RSC_MODE_IND=1 requires RSCI".to_string())?;
        let unit = msg
            .rsc_end_time_unit
            .ok_or_else(|| "ERRM RSC_MODE_IND=1 requires RSC_END_TIME_UNIT".to_string())?;
        let value = msg
            .rsc_end_time_value
            .ok_or_else(|| "ERRM RSC_MODE_IND=1 requires RSC_END_TIME_VALUE".to_string())?;
        if !is_valid_rsci(rsci) {
            return Err(format!("ERRM RSCI 0b{rsci:04b} is reserved"));
        }
        if unit > 0b10 {
            return Err("ERRM RSC_END_TIME_UNIT 0b11 is reserved".to_string());
        }
        if value > 0x0f {
            return Err("ERRM RSC_END_TIME_VALUE exceeds 4 bits".to_string());
        }
    } else if msg.rsci.is_some()
        || msg.rsc_end_time_unit.is_some()
        || msg.rsc_end_time_value.is_some()
    {
        return Err("ERRM reduced-slot-cycle fields require RSC_MODE_IND=1".to_string());
    }
    Ok(())
}

fn write_scrm_pilot_record(bs: &mut Bitstream, record: &Option<SupplementalChannelPilotRecord>) {
    if let Some(record) = record {
        bs.write_u8(1, 1);
        bs.write_u8(record.pilot_rec_type, 3);
        bs.write_u8(record.type_specific_fields.len() as u8, 3);
        for &byte in &record.type_specific_fields {
            bs.write_u8(byte, 8);
        }
    } else {
        bs.write_u8(0, 1);
    }
}

fn write_fch_type_specific_fields(
    bs: &mut Bitstream,
    cap: &FchTypeSpecificFields,
) -> Result<(), String> {
    let for_bits = cap.for_fch_len as usize * 3;
    let rev_bits = cap.rev_fch_len as usize * 3;
    if cap.for_fch_rc_map_raw.len() != for_bits {
        return Err(format!(
            "FOR_FCH_RC_MAP length {} does not match FOR_FCH_LEN {}",
            cap.for_fch_rc_map_raw.len(),
            cap.for_fch_len
        ));
    }
    if cap.rev_fch_rc_map_raw.len() != rev_bits {
        return Err(format!(
            "REV_FCH_RC_MAP length {} does not match REV_FCH_LEN {}",
            cap.rev_fch_rc_map_raw.len(),
            cap.rev_fch_len
        ));
    }
    bs.write_u8(cap.frame_size_5ms_supported as u8, 1);
    bs.write_u8(cap.for_fch_len, 3);
    bs.extend(&cap.for_fch_rc_map_raw);
    bs.write_u8(cap.rev_fch_len, 3);
    bs.extend(&cap.rev_fch_rc_map_raw);
    Ok(())
}

fn write_dcch_type_specific_fields(
    bs: &mut Bitstream,
    cap: &DcchTypeSpecificFields,
) -> Result<(), String> {
    let for_bits = cap.for_dcch_len as usize * 3;
    let rev_bits = cap.rev_dcch_len as usize * 3;
    if cap.for_dcch_rc_map_raw.len() != for_bits {
        return Err(format!(
            "FOR_DCCH_RC_MAP length {} does not match FOR_DCCH_LEN {}",
            cap.for_dcch_rc_map_raw.len(),
            cap.for_dcch_len
        ));
    }
    if cap.rev_dcch_rc_map_raw.len() != rev_bits {
        return Err(format!(
            "REV_DCCH_RC_MAP length {} does not match REV_DCCH_LEN {}",
            cap.rev_dcch_rc_map_raw.len(),
            cap.rev_dcch_len
        ));
    }
    bs.write_u8(cap.frame_size_mode, 2);
    bs.write_u8(cap.for_dcch_len, 3);
    bs.extend(&cap.for_dcch_rc_map_raw);
    bs.write_u8(cap.rev_dcch_len, 3);
    bs.extend(&cap.rev_dcch_rc_map_raw);
    Ok(())
}

fn write_for_pdch_type_specific_fields(
    bs: &mut Bitstream,
    cap: &ForPdchTypeSpecificFields,
) -> Result<(), String> {
    if cap.num_arq_chan == 0b11 {
        return Err("NUM_ARQ_CHAN value 0b11 is reserved".to_string());
    }
    let rc_bits = (cap.for_pdch_len as usize + 1) * 3;
    if cap.for_pdch_rc_map_raw.len() != rc_bits {
        return Err(format!(
            "FOR_PDCH_RC_MAP length {} does not match FOR_PDCH_LEN {}",
            cap.for_pdch_rc_map_raw.len(),
            cap.for_pdch_len
        ));
    }
    if cap
        .for_pdch_rc_map_raw
        .bits()
        .iter()
        .skip(1)
        .any(|bit| *bit != 0)
    {
        return Err("FOR_PDCH_RC_MAP reserved bits must be zero".to_string());
    }
    let config_bits = (cap.ch_config_sup_map_len as usize + 1) * 3;
    if cap.ch_config_sup_map_raw.len() != config_bits {
        return Err(format!(
            "CH_CONFIG_SUP_MAP length {} does not match CH_CONFIG_SUP_MAP_LEN {}",
            cap.ch_config_sup_map_raw.len(),
            cap.ch_config_sup_map_len
        ));
    }
    validate_for_pdch_channel_config_map(&cap.ch_config_sup_map_raw)?;

    bs.write_u8(cap.ack_delay as u8, 1);
    bs.write_u8(cap.num_arq_chan, 2);
    bs.write_u8(cap.for_pdch_len, 2);
    bs.extend(&cap.for_pdch_rc_map_raw);
    bs.write_u8(cap.ch_config_sup_map_len, 2);
    bs.extend(&cap.ch_config_sup_map_raw);
    Ok(())
}

fn write_rev_pdch_type_specific_fields(
    bs: &mut Bitstream,
    cap: &RevPdchTypeSpecificFields,
) -> Result<(), String> {
    let rc_bits = (cap.rev_pdch_len as usize + 1) * 3;
    if cap.rev_pdch_rc_map_raw.len() != rc_bits {
        return Err(format!(
            "REV_PDCH_RC_MAP length {} does not match REV_PDCH_LEN {}",
            cap.rev_pdch_rc_map_raw.len(),
            cap.rev_pdch_len
        ));
    }
    if cap
        .rev_pdch_rc_map_raw
        .bits()
        .iter()
        .skip(1)
        .any(|bit| *bit != 0)
    {
        return Err("REV_PDCH_RC_MAP reserved bits must be zero".to_string());
    }
    let config_bits = (cap.rev_pdch_ch_config_sup_map_len as usize + 1) * 3;
    if cap.rev_pdch_ch_config_sup_map_raw.len() != config_bits {
        return Err(format!(
            "REV_PDCH_CH_CONFIG_SUP_MAP length {} does not match REV_PDCH_CH_CONFIG_SUP_MAP_LEN {}",
            cap.rev_pdch_ch_config_sup_map_raw.len(),
            cap.rev_pdch_ch_config_sup_map_len
        ));
    }
    validate_rev_pdch_channel_config_map(&cap.rev_pdch_ch_config_sup_map_raw)?;
    if cap.rev_pdch_max_size_supported_encoder_packet == 0b11 {
        return Err(
            "REV_PDCH_MAX_SIZE_SUPPORTED_ENCODER_PACKET value 0b11 is reserved".to_string(),
        );
    }

    bs.write_u8(cap.rev_pdch_len, 2);
    bs.extend(&cap.rev_pdch_rc_map_raw);
    bs.write_u8(cap.rev_pdch_ch_config_sup_map_len, 2);
    bs.extend(&cap.rev_pdch_ch_config_sup_map_raw);
    bs.write_u8(cap.rev_pdch_max_size_supported_encoder_packet, 2);
    Ok(())
}

fn validate_fundicated_bcmc_fields(cap: &FundicatedBcmcTypeSpecificFields) -> Result<(), String> {
    let bits = (cap.fundicated_bcmc_ch_sup_map_len as usize + 1) * 3;
    if cap.fundicated_bcmc_ch_sup_map_raw.len() != bits {
        return Err(format!(
            "FUNDICATED_BCMC_CH_SUP_MAP length {} does not match FUNDICATED_BCMC_CH_SUP_MAP_LEN {}",
            cap.fundicated_bcmc_ch_sup_map_raw.len(),
            cap.fundicated_bcmc_ch_sup_map_len
        ));
    }
    if cap
        .fundicated_bcmc_ch_sup_map_raw
        .bits()
        .iter()
        .take(5)
        .all(|bit| *bit == 0)
    {
        return Err("FUNDICATED_BCMC_CH_SUP_MAP must not be all zero".to_string());
    }
    if cap
        .fundicated_bcmc_ch_sup_map_raw
        .bits()
        .iter()
        .skip(5)
        .any(|bit| *bit != 0)
    {
        return Err("FUNDICATED_BCMC_CH_SUP_MAP reserved bits must be zero".to_string());
    }
    Ok(())
}

fn write_fundicated_bcmc_type_specific_fields(
    bs: &mut Bitstream,
    cap: &FundicatedBcmcTypeSpecificFields,
) -> Result<(), String> {
    validate_fundicated_bcmc_fields(cap)?;
    bs.write_u8(cap.fundicated_bcmc_ch_sup_map_len, 2);
    bs.extend(&cap.fundicated_bcmc_ch_sup_map_raw);
    Ok(())
}

fn write_origination_bcmc_fields(
    bs: &mut Bitstream,
    fields: &OriginationBcmcFields,
) -> Result<(), String> {
    write_bcmc_fields(bs, fields, true, false)
}

fn write_page_response_bcmc_fields(
    bs: &mut Bitstream,
    fields: &OriginationBcmcFields,
    bcmc_pref_incl: bool,
) -> Result<(), String> {
    write_bcmc_fields(bs, fields, false, bcmc_pref_incl)
}

fn write_bcmc_fields(
    bs: &mut Bitstream,
    fields: &OriginationBcmcFields,
    include_orig_only_ind: bool,
    bcmc_pref_incl: bool,
) -> Result<(), String> {
    ensure_count("NUM_BCMC_PROGRAMS", fields.programs.len(), 8)?;
    if fields.programs.is_empty() {
        return Err("BCMC_INCL requires at least one BCMC program".to_string());
    }

    if include_orig_only_ind {
        bs.write_u8(fields.bcmc_orig_only_ind as u8, 1);
    }
    bs.write_u8(fields.fundicated_bcmc_supported as u8, 1);
    if fields.fundicated_bcmc_supported {
        write_fundicated_bcmc_type_specific_fields(
            bs,
            fields.fundicated_bcmc_capability.as_ref().ok_or_else(|| {
                "FUNDICATED_BCMC_SUPPORTED set but capability missing".to_string()
            })?,
        )?;
    }

    bs.write_u8(fields.auth_signature_incl as u8, 1);
    if fields.auth_signature_incl {
        ensure_count(
            "TIME_STAMP_SHORT_LENGTH",
            fields.time_stamp_short.len(),
            u8::MAX as usize,
        )?;
        let len = fields
            .time_stamp_short_length
            .unwrap_or(fields.time_stamp_short.len() as u8);
        if len as usize != fields.time_stamp_short.len() {
            return Err(format!(
                "TIME_STAMP_SHORT_LENGTH={} does not match {} TIME_STAMP_SHORT bits",
                len,
                fields.time_stamp_short.len()
            ));
        }
        if !fields.programs.iter().any(|program| {
            program
                .flows
                .iter()
                .any(|flow| flow.auth_signature_ind == Some(true))
        }) {
            return Err(
                "AUTH_SIGNATURE_INCL set but no BCMC flow has AUTH_SIGNATURE_IND=1".to_string(),
            );
        }
        bs.write_u8(len, 8);
        bs.extend(&fields.time_stamp_short);
    }

    let num_programs = fields
        .num_bcmc_programs
        .checked_add(1)
        .ok_or_else(|| "NUM_BCMC_PROGRAMS overflow".to_string())?;
    if num_programs as usize != fields.programs.len() {
        return Err(format!(
            "NUM_BCMC_PROGRAMS={} does not match {} programs",
            fields.num_bcmc_programs,
            fields.programs.len()
        ));
    }
    bs.write_u8(fields.num_bcmc_programs, 3);
    for (program_idx, program) in fields.programs.iter().enumerate() {
        write_bcmc_program(
            bs,
            program,
            bcmc_pref_incl,
            fields.auth_signature_incl,
            program_idx,
        )?;
    }
    Ok(())
}

fn write_bcmc_program(
    bs: &mut Bitstream,
    program: &OriginationBcmcProgram,
    bcmc_pref_incl: bool,
    auth_signature_incl: bool,
    program_idx: usize,
) -> Result<(), String> {
    let program_id_bits = program.bcmc_program_id_len as usize + 1;
    if program.bcmc_program_id.len() != program_id_bits {
        return Err(format!(
            "BCMC_PROGRAM_ID[{program_idx}] length {} does not match BCMC_PROGRAM_ID_LEN {}",
            program.bcmc_program_id.len(),
            program.bcmc_program_id_len
        ));
    }
    if program.flows.is_empty() {
        return Err(format!(
            "BCMC program {program_idx} requires at least one BCMC flow"
        ));
    }
    let flow_bits = program.bcmc_flow_discriminator_len as usize;
    if flow_bits == 0 {
        if program.flows.len() != 1 {
            return Err(format!(
                "BCMC_FLOW_DISCRIMINATOR_LEN=0 requires exactly one flow, got {}",
                program.flows.len()
            ));
        }
        if program.num_flow_discriminator.is_some() {
            return Err(
                "NUM_FLOW_DISCRIMINATOR must be omitted when BCMC_FLOW_DISCRIMINATOR_LEN=0"
                    .to_string(),
            );
        }
    } else {
        let num = program
            .num_flow_discriminator
            .unwrap_or((program.flows.len() - 1) as u32);
        if num as usize + 1 != program.flows.len() {
            return Err(format!(
                "NUM_FLOW_DISCRIMINATOR={} does not match {} flows",
                num,
                program.flows.len()
            ));
        }
        if num >= (1u32 << flow_bits) {
            return Err(format!(
                "NUM_FLOW_DISCRIMINATOR={} does not fit {} bits",
                num, flow_bits
            ));
        }
    }

    bs.write_u8(program.bcmc_program_id_len, 5);
    bs.extend(&program.bcmc_program_id);
    bs.write_u8(program.bcmc_flow_discriminator_len, 3);
    if flow_bits > 0 {
        bs.write_u32(
            program
                .num_flow_discriminator
                .unwrap_or((program.flows.len() - 1) as u32),
            flow_bits,
        );
    }
    for (flow_idx, flow) in program.flows.iter().enumerate() {
        if flow.bcmc_flow_discriminator.len() != flow_bits {
            return Err(format!(
                "BCMC_FLOW_DISCRIMINATOR[{program_idx}][{flow_idx}] length {} does not match BCMC_FLOW_DISCRIMINATOR_LEN {}",
                flow.bcmc_flow_discriminator.len(),
                program.bcmc_flow_discriminator_len
            ));
        }
        bs.extend(&flow.bcmc_flow_discriminator);
        if bcmc_pref_incl {
            bs.write_u8(flow.bcmc_pref.unwrap_or(false) as u8, 1);
        }
        if auth_signature_incl {
            let auth_ind = flow
                .auth_signature_ind
                .unwrap_or(flow.auth_signature.is_some());
            bs.write_u8(auth_ind as u8, 1);
            if auth_ind {
                let same_ind = flow.auth_signature_same_ind.unwrap_or(false);
                if program_idx == 0 && flow_idx == 0 && same_ind {
                    return Err("first BCMC flow AUTH_SIGNATURE_SAME_IND must be zero".to_string());
                }
                bs.write_u8(same_ind as u8, 1);
                if !same_ind {
                    bs.write_u8(flow.bak_id.unwrap_or(0), 4);
                    bs.write_u32(
                        flow.auth_signature.ok_or_else(|| {
                            "AUTH_SIGNATURE_IND set but AUTH_SIGNATURE missing".to_string()
                        })?,
                        32,
                    );
                }
            }
        }
    }
    Ok(())
}

fn encode_access_message_body(
    msg: &AccessMessage,
    ctx: AccessDecodeContext,
) -> Result<Bitstream, String> {
    let mut bs = Bitstream::new();
    match msg {
        AccessMessage::Registration(m) => {
            bs.write_u8(m.reg_type, 4);
            bs.write_u8(m.slot_cycle_index, 3);
            bs.write_u8(m.mob_p_rev, 8);
            bs.write_u8(m.scm, 8);
            bs.write_u8(m.mob_term as u8, 1);
            bs.write_u8(m.return_cause, 4);
        }
        AccessMessage::Order(m) => {
            ensure_count("ADD_RECORD_LEN", m.order_specific.len(), 7)?;
            bs.write_u8(m.order, 6);
            bs.write_u8(m.order_specific.len() as u8, 3);
            write_octets(&mut bs, &m.order_specific);
        }
        AccessMessage::DataBurst(m) => {
            ensure_count("NUM_FIELDS", m.fields.len(), u8::MAX as usize)?;
            bs.write_u8(m.msg_number, 8);
            bs.write_u8(m.burst_type, 6);
            bs.write_u8(m.num_msgs, 8);
            bs.write_u8(m.fields.len() as u8, 8);
            write_octets(&mut bs, &m.fields);
        }
        AccessMessage::Origination(m) => encode_origination_body(&mut bs, m)?,
        AccessMessage::PageResponse(m) => encode_page_response_body(&mut bs, m, ctx)?,
        AccessMessage::AuthChallengeResponse(m) => {
            bs.write_u32(m.authu, 18);
        }
        AccessMessage::StatusResponse(m) => {
            ensure_count("QUAL_INFO_LEN", m.qual_info.len(), 7)?;
            if m.records.is_empty() {
                return Err("Status Response requires at least one info record".to_string());
            }
            bs.write_u8(m.qual_info_type, 8);
            bs.write_u8(m.qual_info.len() as u8, 3);
            write_octets(&mut bs, &m.qual_info);
            write_info_records(&mut bs, &m.records, 8)?;
        }
        AccessMessage::TmsiAssignmentCompletion(_)
        | AccessMessage::PacaCancel(_)
        | AccessMessage::CallRecoveryRequest(_) => {}
        AccessMessage::ExtStatusResponse(m) => {
            ensure_count("QUAL_INFO_LEN", m.qual_info.len(), 7)?;
            ensure_count("NUM_INFO_RECORDS", m.records.len(), 15)?;
            bs.write_u8(m.qual_info_type, 8);
            bs.write_u8(m.qual_info.len() as u8, 3);
            write_octets(&mut bs, &m.qual_info);
            bs.write_u8(m.records.len() as u8, 4);
            write_info_records(&mut bs, &m.records, 8)?;
        }
        AccessMessage::DeviceInformation(m) => {
            ensure_count("NUM_INFO_RECORDS", m.records.len(), 31)?;
            bs.write_u8(m.wll_device_type, 3);
            bs.write_u8(m.records.len() as u8, 5);
            write_info_records(&mut bs, &m.records, 8)?;
        }
        AccessMessage::SecurityModeRequest(m) => encode_security_mode_request_body(&mut bs, m)?,
        AccessMessage::AuthResponse(m) => {
            if m.res.len() != 16 {
                return Err(format!("RES length {} must be 16 octets", m.res.len()));
            }
            write_octets(&mut bs, &m.res);
            let sig_integrity_sup_incl = m.sig_integrity_sup.is_some();
            bs.write_u8(sig_integrity_sup_incl as u8, 1);
            if sig_integrity_sup_incl {
                let sig_sup = m.sig_integrity_sup.unwrap_or(0);
                let sig_req = m.sig_integrity_req.unwrap_or(0);
                validate_sig_integrity_fields(sig_sup, sig_req)?;
                bs.write_u8(sig_sup, 8);
                bs.write_u8(sig_req, 3);
            }
            bs.write_u8(m.new_key_id, 2);
            bs.write_u32(m.new_sseq_h, 24);
        }
        AccessMessage::AuthResync(m) => {
            if m.con_ms_sqn.len() != 6 {
                return Err(format!(
                    "CON_MS_SQN length {} must be 6 octets",
                    m.con_ms_sqn.len()
                ));
            }
            if m.mac_s.len() != 8 {
                return Err(format!("MAC_S length {} must be 8 octets", m.mac_s.len()));
            }
            write_octets(&mut bs, &m.con_ms_sqn);
            write_octets(&mut bs, &m.mac_s);
        }
        AccessMessage::Reconnect(m) => encode_reconnect_body(&mut bs, m, ctx)?,
        AccessMessage::RadioEnvironment(m) => {
            bs.write_u8(m.mode_disabled as u8, 1);
            bs.write_u8(m.tkz_mode_ind as u8, 1);
        }
        AccessMessage::GeneralExtension(m) => {
            ensure_count("NUM_GE_REC", m.records.len(), u8::MAX as usize)?;
            if m.records.is_empty() {
                return Err("General Extension requires at least one GE record".to_string());
            }
            bs.write_u8(m.records.len() as u8, 8);
            write_info_records(&mut bs, &m.records, 8)?;
            bs.write_u8(m.message_type, 8);
            bs.extend(&m.message_record);
        }
        AccessMessage::FlashWithInfo(m) => {
            write_info_records(&mut bs, &m.records, 8)?;
        }
        AccessMessage::SendBurstDtmf(m) => {
            validate_send_burst_dtmf_fields(m.dtmf_on_length, m.dtmf_off_length, &m.digits)?;
            bs.write_u8(m.digits.len() as u8, 8);
            bs.write_u8(m.dtmf_on_length, 3);
            bs.write_u8(m.dtmf_off_length, 3);
            for &digit in &m.digits {
                bs.write_u8(digit, 4);
            }
            bs.write_u8(m.con_ref.is_some() as u8, 1);
            if let Some(con_ref) = m.con_ref {
                bs.write_u8(con_ref, 8);
            }
        }
        AccessMessage::Status(m) => {
            write_info_records(&mut bs, std::slice::from_ref(&m.record), 8)?;
        }
        AccessMessage::OriginationContinuation(m) => {
            validate_origination_continuation_fields(m.digit_mode, &m.digits)?;
            bs.write_u8(m.digit_mode as u8, 1);
            bs.write_u8(m.digits.len() as u8, 8);
            let char_bits = if m.digit_mode { 8 } else { 4 };
            for &digit in &m.digits {
                bs.write_u8(digit, char_bits);
            }
            write_info_records(&mut bs, &m.records, 8)?;
        }
        AccessMessage::HandoffCompletion(m) => {
            validate_handoff_completion_fields(m.last_hdm_seq, &m.pilot_pns)?;
            bs.write_u8(m.last_hdm_seq, 2);
            for &pilot_pn in &m.pilot_pns {
                bs.write_u32(pilot_pn as u32, 9);
            }
        }
        AccessMessage::ParametersResponse(m) => {
            validate_parameters_response_records(&m.records)?;
            for record in &m.records {
                bs.write_u32(record.parameter_id as u32, 16);
                bs.write_u32(record.parameter_len as u32, 10);
                if record.parameter_len != 0x03ff {
                    bs.extend(&record.parameter);
                }
            }
        }
        AccessMessage::ServiceOptionControl(m) => {
            validate_service_option_control_record(&m.control_record)?;
            bs.write_u8(m.con_ref, 8);
            bs.write_u32(m.service_option as u32, 16);
            bs.write_u8(0, 7);
            bs.write_u8(m.control_record.len() as u8, 8);
            for &byte in &m.control_record {
                bs.write_u8(byte, 8);
            }
        }
        AccessMessage::SupplementalChannelRequest(m) => {
            validate_supplemental_channel_request(m)?;
            bs.write_u8(m.req_blob.len() as u8, 4);
            for &byte in &m.req_blob {
                bs.write_u8(byte, 8);
            }
            bs.write_u8(m.scrm_seq_num.is_some() as u8, 1);
            if let Some(seq_num) = m.scrm_seq_num {
                bs.write_u8(seq_num, 4);
            }
            if let Some(measurements) = &m.measurements {
                bs.write_u32(measurements.ref_pn as u32, 9);
                bs.write_u8(measurements.pilot_strength, 6);
                bs.write_u8(measurements.active_pilots.len() as u8, 3);
                for pilot in &measurements.active_pilots {
                    bs.write_u32(pilot.pn_phase as u32, 15);
                    bs.write_u8(pilot.pilot_strength, 6);
                }
                if let Some(neighbor_pilots) = &measurements.neighbor_pilots {
                    bs.write_u8(neighbor_pilots.len() as u8, 3);
                    for pilot in neighbor_pilots {
                        bs.write_u32(pilot.pn_phase as u32, 15);
                        bs.write_u8(pilot.pilot_strength, 6);
                    }
                }
                if !m.req_blob.is_empty() {
                    write_scrm_pilot_record(&mut bs, &measurements.ref_pilot_record);
                }
                for pilot in &measurements.active_pilots {
                    write_scrm_pilot_record(&mut bs, &pilot.pilot_record);
                }
                if let Some(neighbor_pilots) = &measurements.neighbor_pilots {
                    for pilot in neighbor_pilots {
                        write_scrm_pilot_record(&mut bs, &pilot.pilot_record);
                    }
                }
            }
        }
        AccessMessage::CandidateFreqSearchResponse(m) => {
            validate_candidate_freq_search_response(m)?;
            bs.write_u8(m.last_cfsrm_seq, 2);
            bs.write_u8(m.total_off_time_fwd, 6);
            bs.write_u8(m.max_off_time_fwd, 6);
            bs.write_u8(m.total_off_time_rev, 6);
            bs.write_u8(m.max_off_time_rev, 6);
            bs.write_u8(m.pcg_off_times as u8, 1);
            bs.write_u8(m.align_timing_used as u8, 1);
            if let Some(max_num_visits) = m.max_num_visits {
                bs.write_u8(max_num_visits, 5);
                if let Some(inter_visit_time) = m.inter_visit_time {
                    bs.write_u8(inter_visit_time, 6);
                }
            }
        }
        AccessMessage::CandidateFreqSearchReport(m) => {
            validate_candidate_freq_search_report(m)?;
            bs.write_u8(m.last_srch_msg as u8, 1);
            bs.write_u8(m.last_srch_msg_seq, 2);
            bs.write_u8(m.search_mode, 4);
            match &m.mode_specific {
                CandidateFreqSearchReportModeSpecific::CdmaPilots(mode) => {
                    let mut mode_bits = Bitstream::new();
                    write_candidate_freq_search_cdma_mode(&mut mode_bits, mode)?;
                    bs.write_u8((mode_bits.len() / 8) as u8, 8);
                    bs.extend(&mode_bits);
                }
                CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(bytes) => {
                    bs.write_u8(bytes.len() as u8, 8);
                    for &byte in bytes {
                        bs.write_u8(byte, 8);
                    }
                }
            }
        }
        AccessMessage::PeriodicPsmm(m) => {
            validate_periodic_psmm(m)?;
            bs.write_u32(m.ref_pn as u32, 9);
            bs.write_u8(m.pilot_strength, 6);
            bs.write_u8(m.keep as u8, 1);
            bs.write_u8(m.sf_rx_pwr, 5);
            bs.write_u8(m.pilots.len() as u8, 4);
            for pilot in &m.pilots {
                bs.write_u32(pilot.pilot_pn_phase as u32, 15);
                bs.write_u8(pilot.pilot_strength, 6);
                bs.write_u8(pilot.keep as u8, 1);
            }
            for pilot in &m.pilots {
                write_scrm_pilot_record(&mut bs, &pilot.pilot_record);
            }
            bs.write_u8(m.setpoints.is_some() as u8, 1);
            if let Some(setpoints) = &m.setpoints {
                bs.write_u8(setpoints.fpc_fch_curr_setpt.is_some() as u8, 1);
                if let Some(setpt) = setpoints.fpc_fch_curr_setpt {
                    bs.write_u8(setpt, 8);
                }
                bs.write_u8(setpoints.fpc_dcch_curr_setpt.is_some() as u8, 1);
                if let Some(setpt) = setpoints.fpc_dcch_curr_setpt {
                    bs.write_u8(setpt, 8);
                }
                bs.write_u8(setpoints.sch_setpoints.len() as u8, 2);
                for sch in &setpoints.sch_setpoints {
                    bs.write_u8(sch.sch_id, 1);
                    bs.write_u8(sch.fpc_sch_curr_setpt, 8);
                }
            }
        }
        AccessMessage::OuterLoopReport(m) => {
            validate_outer_loop_report(m)?;
            bs.write_u8(m.fpc_fch_curr_setpt.is_some() as u8, 1);
            if let Some(setpt) = m.fpc_fch_curr_setpt {
                bs.write_u8(setpt, 8);
            }
            bs.write_u8(m.fpc_dcch_curr_setpt.is_some() as u8, 1);
            if let Some(setpt) = m.fpc_dcch_curr_setpt {
                bs.write_u8(setpt, 8);
            }
            bs.write_u8(m.sch_setpoints.len() as u8, 2);
            for sch in &m.sch_setpoints {
                bs.write_u8(sch.sch_id, 1);
                bs.write_u8(sch.fpc_sch_curr_setpt, 8);
            }
        }
        AccessMessage::ResourceRequest(m) => {
            validate_resource_request(m)?;
            bs.write_u8(m.ch_ind.is_some() as u8, 1);
            if let Some(ch_ind) = m.ch_ind {
                bs.write_u8(ch_ind, 2);
                if ch_ind == 0 {
                    let ext_ch_ind = m
                        .ext_ch_ind
                        .ok_or_else(|| "RRM CH_IND=00 requires EXT_CH_IND".to_string())?;
                    bs.write_u8(ext_ch_ind, 5);
                }
            }
        }
        AccessMessage::ExtReleaseResponse(m) => {
            validate_ext_release_response(m)?;
            bs.write_u8(m.rsc_mode_ind as u8, 1);
            if m.rsc_mode_ind {
                let rsci = m
                    .rsci
                    .ok_or_else(|| "ERRM RSC_MODE_IND=1 requires RSCI".to_string())?;
                let unit = m
                    .rsc_end_time_unit
                    .ok_or_else(|| "ERRM RSC_MODE_IND=1 requires RSC_END_TIME_UNIT".to_string())?;
                let value = m
                    .rsc_end_time_value
                    .ok_or_else(|| "ERRM RSC_MODE_IND=1 requires RSC_END_TIME_VALUE".to_string())?;
                bs.write_u8(rsci, 4);
                bs.write_u8(unit, 2);
                bs.write_u8(value, 4);
            }
        }
        AccessMessage::ServiceConnectCompletion(m) => {
            bs.write_u8(0, 1);
            bs.write_u8(m.serv_con_seq, 3);
        }
        AccessMessage::PilotStrengthMeasurement(m) => {
            bs.write_u32(m.ref_pn as u32, 9);
            bs.write_u8(m.pilot_strength, 6);
            bs.write_u8(m.keep as u8, 1);
            for pilot in &m.pilots {
                bs.write_u32(pilot.pilot_pn_phase as u32, 15);
                bs.write_u8(pilot.pilot_strength, 6);
                bs.write_u8(pilot.keep as u8, 1);
            }
        }
        AccessMessage::PowerMeasurementReport(m) => {
            ensure_count("NUM_PILOTS", m.pilot_strengths.len(), 15)?;
            bs.write_u8(m.errors_detected, 5);
            bs.write_u32(m.pwr_meas_frames as u32, 10);
            bs.write_u8(m.last_hdm_seq, 2);
            bs.write_u8(m.pilot_strengths.len() as u8, 4);
            for &strength in &m.pilot_strengths {
                bs.write_u8(strength, 6);
            }
            bs.write_u8(m.dcch_pwr_meas_incl as u8, 1);
            if m.dcch_pwr_meas_incl {
                bs.write_u32(m.dcch_pwr_meas_frames.unwrap_or(0) as u32, 10);
                bs.write_u8(m.dcch_errors_detected.unwrap_or(0), 5);
            }
            bs.write_u8(m.sch_pwr_meas_incl as u8, 1);
            if m.sch_pwr_meas_incl {
                bs.write_u8(m.sch_id.unwrap_or(0), 1);
                bs.write_u32(m.sch_pwr_meas_frames.unwrap_or(0) as u32, 16);
                bs.write_u32(m.sch_errors_detected.unwrap_or(0) as u32, 10);
            }
        }
        AccessMessage::ServiceRequest(m) => {
            bs.write_u8(m.serv_req_seq, 3);
            bs.write_u8(m.req_purpose, 4);
            if m.req_purpose == 0b0010 {
                let cfg = m
                    .service_config
                    .as_ref()
                    .ok_or_else(|| "Service Request propose requires service_config".to_string())?;
                let raw = encode_service_config_record(cfg)?;
                ensure_count("RECORD_LEN", raw.len(), u8::MAX as usize)?;
                bs.write_u8(0x07, 8);
                bs.write_u8(raw.len() as u8, 8);
                write_octets(&mut bs, &raw);
            }
        }
        AccessMessage::ServiceResponse(m) => {
            bs.write_u8(m.serv_req_seq, 3);
            bs.write_u8(m.resp_purpose, 4);
            if m.resp_purpose == 0b0010 {
                let cfg = m.service_config.as_ref().ok_or_else(|| {
                    "Service Response counter-propose requires service_config".to_string()
                })?;
                let raw = encode_service_config_record(cfg)?;
                ensure_count("RECORD_LEN", raw.len(), u8::MAX as usize)?;
                bs.write_u8(0x07, 8);
                bs.write_u8(raw.len() as u8, 8);
                write_octets(&mut bs, &raw);
            }
        }
    }
    Ok(bs)
}

fn encode_service_config_record(cfg: &ServiceConfigRecord) -> Result<Vec<u8>, String> {
    ensure_count(
        "NUM_CON_REC",
        cfg.connection_records.len(),
        u8::MAX as usize,
    )?;
    let mut bs = Bitstream::new();
    bs.write_u32(cfg.for_mux_option as u32, 16);
    bs.write_u32(cfg.rev_mux_option as u32, 16);
    bs.write_u8(cfg.for_rates, 8);
    bs.write_u8(cfg.rev_rates, 8);
    bs.write_u8(cfg.connection_records.len() as u8, 8);

    for conn in &cfg.connection_records {
        let mut rec = Bitstream::new();
        rec.write_u8(conn.con_ref, 8);
        rec.write_u32(conn.service_option as u32, 16);
        rec.write_u8(conn.for_traffic, 4);
        rec.write_u8(conn.rev_traffic, 4);
        rec.write_u8(conn.ui_encrypt_mode, 3);
        rec.write_u8(conn.sr_id, 3);
        rec.write_u8(conn.rlp_info_incl as u8, 1);
        if conn.rlp_info_incl {
            let blob = conn.rlp_blob.as_deref().unwrap_or(&[]);
            ensure_count("RLP_BLOB_LEN", blob.len(), 15)?;
            rec.write_u8(blob.len() as u8, 4);
            write_octets(&mut rec, blob);
        }

        let qos = conn.qos_parms.as_deref().unwrap_or(&[]);
        rec.write_u8((!qos.is_empty()) as u8, 1);
        if !qos.is_empty() {
            ensure_count("QOS_PARMS_LEN", qos.len(), 31)?;
            rec.write_u8(qos.len() as u8, 5);
            write_octets(&mut rec, qos);
        }
        if rec.len() % 8 != 0 {
            rec.write_u8(0, 8 - (rec.len() % 8));
        }

        let rec_bytes = rec.to_packed_bytes();
        ensure_count("CON_RECORD_LEN", rec_bytes.len() + 1, u8::MAX as usize)?;
        bs.write_u8((rec_bytes.len() + 1) as u8, 8);
        write_octets(&mut bs, &rec_bytes);
    }

    bs.write_u8(cfg.fch_cc_incl as u8, 1);
    if cfg.fch_cc_incl {
        bs.write_u8(cfg.fch_frame_size.unwrap_or(0), 1);
        bs.write_u8(cfg.for_fch_rc.unwrap_or(0), 5);
        bs.write_u8(cfg.rev_fch_rc.unwrap_or(0), 5);
    }
    bs.write_u8(cfg.dcch_cc_incl as u8, 1);
    if cfg.dcch_cc_incl {
        return Err("DCCH service configuration encoding needs DCCH RC fields".to_string());
    }
    bs.write_u8(cfg.for_sch_cc_incl as u8, 1);
    bs.write_u8(cfg.rev_sch_cc_incl as u8, 1);
    bs.write_u8(0, 1);

    if bs.len() % 8 != 0 {
        bs.write_u8(0, 8 - (bs.len() % 8));
    }
    Ok(bs.to_packed_bytes())
}

fn encode_origination_body(bs: &mut Bitstream, m: &OriginationMessage) -> Result<(), String> {
    ensure_count("NUM_FIELDS", m.digits.len(), u8::MAX as usize)?;
    ensure_count("NUM_ALT_SO", m.alt_service_options.len(), 7)?;
    bs.write_u8(m.mob_term as u8, 1);
    bs.write_u8(m.slot_cycle_index, 3);
    bs.write_u8(m.mob_p_rev, 8);
    bs.write_u8(m.scm, 8);
    bs.write_u8(m.request_mode, 3);
    bs.write_u8(m.special_service as u8, 1);
    if m.special_service {
        bs.write_u32(m.service_option.unwrap_or(0) as u32, 16);
    }
    bs.write_u8(m.pm as u8, 1);
    bs.write_u8(m.digit_mode as u8, 1);
    if m.digit_mode || m.mob_p_rev >= 11 {
        bs.write_u8(m.number_type.unwrap_or(0), 3);
    }
    if m.digit_mode {
        bs.write_u8(m.number_plan.unwrap_or(0), 4);
    }
    bs.write_u8(m.more_fields as u8, 1);
    bs.write_u8(m.digits.len() as u8, 8);
    let char_bits = if m.digit_mode { 8 } else { 4 };
    for &digit in &m.digits {
        bs.write_u8(digit, char_bits);
    }
    bs.write_u8(m.nar_an_cap as u8, 1);
    bs.write_u8(m.paca_reorig as u8, 1);
    bs.write_u8(m.return_cause, 4);
    bs.write_u8(m.more_records as u8, 1);

    if let Some(encryption_supported) = m.encryption_supported {
        bs.write_u8(encryption_supported, 4);
    }
    bs.write_u8(m.paca_supported as u8, 1);
    bs.write_u8(m.alt_service_options.len() as u8, 3);
    for &so in &m.alt_service_options {
        bs.write_u32(so as u32, 16);
    }

    if m.mob_p_rev >= 6 {
        bs.write_u8(m.drs.unwrap_or(false) as u8, 1);
        let uzid_incl = m.uzid_incl.unwrap_or(m.uzid.is_some());
        bs.write_u8(uzid_incl as u8, 1);
        if uzid_incl {
            bs.write_u32(m.uzid.unwrap_or(0) as u32, 16);
        }
        bs.write_u8(m.ch_ind.unwrap_or(0), 2);
        bs.write_u8(m.sr_id.unwrap_or(0), 3);
        bs.write_u8(m.otd_supported.unwrap_or(false) as u8, 1);
        bs.write_u8(m.qpch_supported.unwrap_or(false) as u8, 1);
        bs.write_u8(m.enhanced_rc.unwrap_or(false) as u8, 1);
        bs.write_u8(m.for_rc_pref.unwrap_or(0), 5);
        bs.write_u8(m.rev_rc_pref.unwrap_or(0), 5);

        let fch_supported = m.fch_supported.unwrap_or(m.fch_capability.is_some());
        bs.write_u8(fch_supported as u8, 1);
        if fch_supported {
            write_fch_type_specific_fields(
                bs,
                m.fch_capability
                    .as_ref()
                    .ok_or_else(|| "FCH_SUPPORTED set but FCH capability missing".to_string())?,
            )?;
        }

        let dcch_supported = m.dcch_supported.unwrap_or(m.dcch_capability.is_some());
        bs.write_u8(dcch_supported as u8, 1);
        if dcch_supported {
            write_dcch_type_specific_fields(
                bs,
                m.dcch_capability
                    .as_ref()
                    .ok_or_else(|| "DCCH_SUPPORTED set but DCCH capability missing".to_string())?,
            )?;
        }

        let geo_loc_incl = m.geo_loc_incl.unwrap_or(m.geo_loc_type.is_some());
        bs.write_u8(geo_loc_incl as u8, 1);
        if geo_loc_incl {
            bs.write_u8(m.geo_loc_type.unwrap_or(0), 3);
        }
        bs.write_u8(m.rev_fch_gating_req.unwrap_or(false) as u8, 1);
    }

    if m.mob_p_rev >= 7 {
        bs.write_u8(m.orig_reason.unwrap_or(false) as u8, 1);
        bs.write_u8(m.orig_count.unwrap_or(0), 2);
        bs.write_u8(m.sts_supported.unwrap_or(false) as u8, 1);
        bs.write_u8(m.cch_3x_supported.unwrap_or(false) as u8, 1);
        let wll_incl = m.wll_incl.unwrap_or(m.wll_device_type.is_some());
        bs.write_u8(wll_incl as u8, 1);
        if wll_incl {
            bs.write_u8(m.wll_device_type.unwrap_or(0), 3);
        }
        let global_emergency_call = m.global_emergency_call.unwrap_or(false);
        bs.write_u8(global_emergency_call as u8, 1);
        if global_emergency_call {
            bs.write_u8(m.ms_init_pos_loc_ind.unwrap_or(false) as u8, 1);
        }

        let qos_parms_incl = m.qos_parms_incl.unwrap_or(!m.qos_parms.is_empty());
        bs.write_u8(qos_parms_incl as u8, 1);
        if qos_parms_incl {
            let qos_len = m.qos_parms_len.unwrap_or(m.qos_parms.len() as u8);
            ensure_count("QOS_PARMS_LEN", qos_len as usize, 31)?;
            if qos_len as usize != m.qos_parms.len() {
                return Err(format!(
                    "QOS_PARMS_LEN={} does not match {} QOS_PARMS octets",
                    qos_len,
                    m.qos_parms.len()
                ));
            }
            bs.write_u8(qos_len, 5);
            write_octets(bs, &m.qos_parms);
        }

        let enc_info_incl = m.enc_info_incl.unwrap_or(m.sig_encrypt_sup.is_some());
        bs.write_u8(enc_info_incl as u8, 1);
        if enc_info_incl {
            let sig_sup = m.sig_encrypt_sup.unwrap_or(0b1000_0000);
            validate_sig_encrypt_sup(sig_sup)?;
            bs.write_u8(sig_sup, 8);
            bs.write_u8(m.d_sig_encrypt_req.unwrap_or(false) as u8, 1);
            bs.write_u8(m.c_sig_encrypt_req.unwrap_or(false) as u8, 1);
            let ecmea = (sig_sup >> 6) & 1;
            let rea = (sig_sup >> 5) & 1;
            if ecmea == 1 || rea == 1 {
                bs.write_u32(m.new_sseq_h.unwrap_or(0), 24);
                bs.write_u8(m.new_sseq_h_sig.unwrap_or(0), 8);
            }
            bs.write_u8(m.ui_encrypt_req.unwrap_or(false) as u8, 1);
            let ui_sup = m.ui_encrypt_sup.unwrap_or(0);
            validate_ui_encrypt_sup(ui_sup)?;
            bs.write_u8(ui_sup, 8);
        }

        let sync_id_incl = m.sync_id_incl.unwrap_or(m.sync_id.is_some());
        bs.write_u8(sync_id_incl as u8, 1);
        if sync_id_incl {
            let len = m.sync_id_len.unwrap_or_else(|| {
                m.sync_id
                    .map(|value| if value <= 0xff { 1 } else { 4 })
                    .unwrap_or(0)
            });
            ensure_count("SYNC_ID_LEN", len as usize, 15)?;
            if len > 4 {
                return Err(format!("SYNC_ID_LEN={} exceeds local u32 storage", len));
            }
            bs.write_u8(len, 4);
            if len > 0 {
                bs.write_u32(m.sync_id.unwrap_or(0), len as usize * 8);
            }
        }

        let prev_sid_incl = m.prev_sid_incl.unwrap_or(m.prev_sid.is_some());
        bs.write_u8(prev_sid_incl as u8, 1);
        if prev_sid_incl {
            bs.write_u32(m.prev_sid.unwrap_or(0) as u32, 15);
        }
        let prev_nid_incl = m.prev_nid_incl.unwrap_or(m.prev_nid.is_some());
        bs.write_u8(prev_nid_incl as u8, 1);
        if prev_nid_incl {
            bs.write_u32(m.prev_nid.unwrap_or(0) as u32, 16);
        }
        let prev_pzid_incl = m.prev_pzid_incl.unwrap_or(m.prev_pzid.is_some());
        bs.write_u8(prev_pzid_incl as u8, 1);
        if prev_pzid_incl {
            bs.write_u8(m.prev_pzid.unwrap_or(0), 8);
        }

        let so_bitmap_ind = m.so_bitmap_ind.unwrap_or(0);
        bs.write_u8(so_bitmap_ind, 2);
        if so_bitmap_ind > 0 {
            bs.write_u8(m.so_group_num.unwrap_or(0), 5);
            let bitmap_bits = 1usize << (1 + so_bitmap_ind as usize);
            bs.write_u32(m.so_bitmap.unwrap_or(0) as u32, bitmap_bits);
        }
    }

    if m.mob_p_rev >= 8 {
        bs.write_u8(m.sdb_desired_only.unwrap_or(false) as u8, 1);
        bs.write_u8(m.alt_band_class_sup.unwrap_or(false) as u8, 1);
    }

    if m.mob_p_rev >= 9 {
        let msg_int_info_incl = m.msg_int_info_incl.unwrap_or(
            m.sig_integrity_sup_incl.is_some()
                || m.sig_integrity_sup.is_some()
                || m.new_key_id.is_some(),
        );
        bs.write_u8(msg_int_info_incl as u8, 1);
        if msg_int_info_incl {
            let sig_integrity_sup_incl = m
                .sig_integrity_sup_incl
                .unwrap_or(m.sig_integrity_sup.is_some());
            bs.write_u8(sig_integrity_sup_incl as u8, 1);
            if sig_integrity_sup_incl {
                let sig_sup = m.sig_integrity_sup.unwrap_or(0);
                let sig_req = m.sig_integrity_req.unwrap_or(0);
                validate_sig_integrity_fields(sig_sup, sig_req)?;
                bs.write_u8(sig_sup, 8);
                bs.write_u8(sig_req, 3);
            }
            bs.write_u8(m.new_key_id.unwrap_or(0), 2);
            let new_sseq_h_incl = m.new_sseq_h_incl.unwrap_or(m.new_sseq_h.is_some());
            bs.write_u8(new_sseq_h_incl as u8, 1);
            if new_sseq_h_incl {
                bs.write_u32(m.new_sseq_h.unwrap_or(0), 24);
                bs.write_u8(m.new_sseq_h_sig.unwrap_or(0), 8);
            }
        }

        let for_pdch_supported = m
            .for_pdch_supported
            .unwrap_or(m.for_pdch_capability.is_some());
        bs.write_u8(for_pdch_supported as u8, 1);
        if for_pdch_supported {
            write_for_pdch_type_specific_fields(
                bs,
                m.for_pdch_capability.as_ref().ok_or_else(|| {
                    "FOR_PDCH_SUPPORTED set but FOR_PDCH capability missing".to_string()
                })?,
            )?;
        }

        if m.ch_ind == Some(0) {
            let ext_ch_ind = m
                .ext_ch_ind
                .ok_or_else(|| "CH_IND=00 requires EXT_CH_IND".to_string())?;
            if !is_valid_origination_ext_ch_ind(ext_ch_ind) {
                return Err(format!("EXT_CH_IND=0b{ext_ch_ind:05b} is reserved"));
            }
            bs.write_u8(ext_ch_ind, 5);
        }
    }

    if m.mob_p_rev >= 11 {
        if m.slot_cycle_index != 0 {
            bs.write_u8(m.sign_slot_cycle_index.unwrap_or(false) as u8, 1);
        }

        if m.sr_id != Some(0b111) {
            let add_serv_instance_incl = m
                .add_serv_instance_incl
                .unwrap_or(!m.add_service_instances.is_empty());
            bs.write_u8(add_serv_instance_incl as u8, 1);
            if add_serv_instance_incl {
                ensure_count("NUM_ADD_SERV_INSTANCE", m.add_service_instances.len(), 7)?;
                bs.write_u8(m.add_service_instances.len() as u8, 3);
                let sync_id_incl = m.sync_id_incl == Some(true);
                for record in &m.add_service_instances {
                    bs.write_u8(record.add_sr_id, 3);
                    bs.write_u8(record.add_drs as u8, 1);
                    if !sync_id_incl {
                        let add_service_option_incl = record
                            .add_service_option_incl
                            .unwrap_or(record.add_service_option.is_some());
                        bs.write_u8(add_service_option_incl as u8, 1);
                        if add_service_option_incl {
                            bs.write_u32(record.add_service_option.unwrap_or(0) as u32, 16);
                        }

                        let add_qos_parms_incl = record
                            .add_qos_parms_incl
                            .unwrap_or(!record.add_qos_parms.is_empty());
                        bs.write_u8(add_qos_parms_incl as u8, 1);
                        if add_qos_parms_incl {
                            let len = record
                                .add_qos_parms_len
                                .unwrap_or(record.add_qos_parms.len() as u8);
                            ensure_count("ADD_QOS_PARMS_LEN", len as usize, 31)?;
                            if len as usize != record.add_qos_parms.len() {
                                return Err(format!(
                                    "ADD_QOS_PARMS_LEN={} does not match {} ADD_QOS_PARMS octets",
                                    len,
                                    record.add_qos_parms.len()
                                ));
                            }
                            bs.write_u8(len, 5);
                            write_octets(bs, &record.add_qos_parms);
                        }
                    }
                }
            }
        }

        let bcmc_incl = m.bcmc_incl.unwrap_or(m.bcmc.is_some());
        bs.write_u8(bcmc_incl as u8, 1);
        if bcmc_incl {
            write_origination_bcmc_fields(
                bs,
                m.bcmc
                    .as_ref()
                    .ok_or_else(|| "BCMC_INCL set but BCMC fields missing".to_string())?,
            )?;
        }

        if m.for_pdch_supported == Some(true) {
            let rev_pdch_supported = m
                .rev_pdch_supported
                .unwrap_or(m.rev_pdch_capability.is_some());
            bs.write_u8(rev_pdch_supported as u8, 1);
            if rev_pdch_supported {
                write_rev_pdch_type_specific_fields(
                    bs,
                    m.rev_pdch_capability.as_ref().ok_or_else(|| {
                        "REV_PDCH_SUPPORTED set but REV_PDCH capability missing".to_string()
                    })?,
                )?;
            }
        }

        let band_sub_rep_incl = m
            .band_sub_rep_incl
            .unwrap_or(!m.band_subclass_sup.is_empty());
        bs.write_u8(band_sub_rep_incl as u8, 1);
        if band_sub_rep_incl {
            ensure_count("NUM_BAND_SUBCLASS", m.band_subclass_sup.len(), 15)?;
            let num = m
                .num_band_subclass
                .unwrap_or(m.band_subclass_sup.len() as u8);
            if num as usize != m.band_subclass_sup.len() {
                return Err(format!(
                    "NUM_BAND_SUBCLASS={} does not match {} BAND_SUBCLASS_SUP bits",
                    num,
                    m.band_subclass_sup.len()
                ));
            }
            bs.write_u8(num, 4);
            for &subclass_sup in &m.band_subclass_sup {
                bs.write_u8(subclass_sup & 1, 1);
            }
        }
    }

    if m.mob_p_rev >= 12 {
        let add_geo_loc_incl = m.add_geo_loc_incl.unwrap_or(m.add_geo_loc_type.is_some());
        bs.write_u8(add_geo_loc_incl as u8, 1);
        if add_geo_loc_incl {
            let len_ind = m.add_geo_loc_type_len_ind.unwrap_or_else(|| {
                m.add_geo_loc_type
                    .is_some_and(|value| value > u16::MAX as u32)
            });
            bs.write_u8(len_ind as u8, 1);
            let bits = if len_ind { 24 } else { 16 };
            let value = m
                .add_geo_loc_type
                .ok_or_else(|| "ADD_GEO_LOC_INCL set but ADD_GEO_LOC_TYPE missing".to_string())?;
            if !len_ind && value > u16::MAX as u32 {
                return Err("ADD_GEO_LOC_TYPE does not fit 16-bit length".to_string());
            }
            if value >= (1 << bits) {
                return Err(format!("ADD_GEO_LOC_TYPE does not fit {bits} bits"));
            }
            bs.write_u32(value, bits);
        }
    }

    Ok(())
}

fn page_response_p_rev(_ctx: AccessDecodeContext, m: &PageResponseMessage) -> u8 {
    m.mob_p_rev
}

fn encode_page_response_body(
    bs: &mut Bitstream,
    m: &PageResponseMessage,
    ctx: AccessDecodeContext,
) -> Result<(), String> {
    ensure_count("NUM_ALT_SO", m.alt_service_options.len(), 7)?;
    let effective_p_rev_in_use = page_response_p_rev(ctx, m);
    let include_encryption_supported = match ctx.auth_mode {
        Some(auth_mode) if effective_p_rev_in_use < 7 => auth_mode != 0,
        _ => m.encryption_supported.is_some(),
    };

    bs.write_u8(m.mob_term as u8, 1);
    bs.write_u8(m.slot_cycle_index, 3);
    bs.write_u8(m.mob_p_rev, 8);
    bs.write_u8(m.scm, 8);
    bs.write_u8(m.request_mode, 3);
    bs.write_u32(m.service_option as u32, 16);
    bs.write_u8(m.pm as u8, 1);
    bs.write_u8(m.nar_an_cap as u8, 1);
    if include_encryption_supported {
        bs.write_u8(m.encryption_supported.unwrap_or(0), 4);
    }
    bs.write_u8(m.alt_service_options.len() as u8, 3);
    for &so in &m.alt_service_options {
        bs.write_u32(so as u32, 16);
    }

    if effective_p_rev_in_use >= 6 {
        let uzid_incl = m.uzid_incl.unwrap_or(m.uzid.is_some());
        bs.write_u8(uzid_incl as u8, 1);
        if uzid_incl {
            bs.write_u32(m.uzid.unwrap_or(0) as u32, 16);
        }
        bs.write_u8(m.ch_ind.unwrap_or(0), 2);
        bs.write_u8(m.otd_supported.unwrap_or(false) as u8, 1);
        bs.write_u8(m.qpch_supported.unwrap_or(false) as u8, 1);
        bs.write_u8(m.enhanced_rc.unwrap_or(false) as u8, 1);
        bs.write_u8(m.for_rc_pref.unwrap_or(0), 5);
        bs.write_u8(m.rev_rc_pref.unwrap_or(0), 5);

        let fch_supported = m.fch_supported.unwrap_or(m.fch_capability.is_some());
        bs.write_u8(fch_supported as u8, 1);
        if fch_supported {
            write_fch_type_specific_fields(
                bs,
                m.fch_capability
                    .as_ref()
                    .ok_or_else(|| "FCH_SUPPORTED set but FCH capability missing".to_string())?,
            )?;
        }

        let dcch_supported = m.dcch_supported.unwrap_or(m.dcch_capability.is_some());
        bs.write_u8(dcch_supported as u8, 1);
        if dcch_supported {
            write_dcch_type_specific_fields(
                bs,
                m.dcch_capability
                    .as_ref()
                    .ok_or_else(|| "DCCH_SUPPORTED set but DCCH capability missing".to_string())?,
            )?;
        }
        bs.write_u8(m.rev_fch_gating_req.unwrap_or(false) as u8, 1);
    }

    if effective_p_rev_in_use >= 7 {
        bs.write_u8(m.sts_supported.unwrap_or(false) as u8, 1);
        bs.write_u8(m.cch_3x_supported.unwrap_or(false) as u8, 1);
        let wll_incl = m.wll_incl.unwrap_or(m.wll_device_type.is_some());
        bs.write_u8(wll_incl as u8, 1);
        if wll_incl {
            bs.write_u8(m.wll_device_type.unwrap_or(0), 3);
            bs.write_u8(m.hook_status.unwrap_or(0), 4);
        }

        let enc_info_incl = m.enc_info_incl.unwrap_or(m.sig_encrypt_sup.is_some());
        bs.write_u8(enc_info_incl as u8, 1);
        if enc_info_incl {
            let sig_sup = m.sig_encrypt_sup.unwrap_or(0);
            bs.write_u8(sig_sup, 8);
            bs.write_u8(m.d_sig_encrypt_req.unwrap_or(0), 1);
            bs.write_u8(m.c_sig_encrypt_req.unwrap_or(0), 1);
            let ecmea = (sig_sup >> 6) & 1;
            let rea = (sig_sup >> 5) & 1;
            if ecmea == 1 || rea == 1 {
                bs.write_u32(m.new_sseq_h.unwrap_or(0), 24);
                bs.write_u32(m.new_sseq_h_sig.unwrap_or(0), 8);
            }
            bs.write_u8(m.ui_encrypt_req.unwrap_or(0), 1);
            bs.write_u8(m.ui_encrypt_sup.unwrap_or(0), 8);
        }

        let sync_id_incl = m.sync_id_incl.unwrap_or(m.sync_id.is_some());
        bs.write_u8(sync_id_incl as u8, 1);
        if sync_id_incl {
            let len = m.sync_id_len.unwrap_or_else(|| {
                m.sync_id
                    .map(|value| if value <= 0xff { 1 } else { 4 })
                    .unwrap_or(0)
            });
            bs.write_u8(len, 4);
            if len > 0 {
                bs.write_u32(m.sync_id.unwrap_or(0), len as usize * 8);
            }
        }

        let so_bitmap_ind = m.so_bitmap_ind.unwrap_or(0);
        bs.write_u8(so_bitmap_ind, 2);
        if so_bitmap_ind > 0 {
            bs.write_u8(m.so_group_num.unwrap_or(0), 5);
            let bitmap_bits = 1usize << (1 + so_bitmap_ind as usize);
            bs.write_u32(m.so_bitmap.unwrap_or(0) as u32, bitmap_bits);
        }
    }

    if effective_p_rev_in_use >= 8 {
        bs.write_u8(m.alt_band_class_sup.unwrap_or(false) as u8, 1);
    }

    if effective_p_rev_in_use >= 9 {
        let msg_int_info_incl = m
            .msg_int_info_incl
            .unwrap_or(m.sig_integrity_sup_incl.is_some() || m.new_key_id.is_some());
        bs.write_u8(msg_int_info_incl as u8, 1);
        if msg_int_info_incl {
            let sig_integrity_sup_incl = m
                .sig_integrity_sup_incl
                .unwrap_or(m.sig_integrity_sup.is_some());
            bs.write_u8(sig_integrity_sup_incl as u8, 1);
            if sig_integrity_sup_incl {
                let sig_sup = m.sig_integrity_sup.unwrap_or(0);
                let sig_req = m.sig_integrity_req.unwrap_or(0);
                validate_sig_integrity_fields(sig_sup, sig_req)?;
                bs.write_u8(sig_sup, 8);
                bs.write_u8(sig_req, 3);
            }
            bs.write_u8(m.new_key_id.unwrap_or(0), 2);
            let new_sseq_h_incl = m.new_sseq_h_incl.unwrap_or(m.new_sseq_h.is_some());
            bs.write_u8(new_sseq_h_incl as u8, 1);
            if new_sseq_h_incl {
                bs.write_u32(m.new_sseq_h.unwrap_or(0), 24);
                bs.write_u32(m.new_sseq_h_sig.unwrap_or(0), 8);
            }
        }
    }

    if effective_p_rev_in_use >= 9 {
        let for_pdch_supported = m
            .for_pdch_supported
            .unwrap_or(m.for_pdch_capability.is_some());
        bs.write_u8(for_pdch_supported as u8, 1);
        if for_pdch_supported {
            write_for_pdch_type_specific_fields(
                bs,
                m.for_pdch_capability.as_ref().ok_or_else(|| {
                    "FOR_PDCH_SUPPORTED set but FOR_PDCH capability missing".to_string()
                })?,
            )?;
        }
        if m.ch_ind == Some(0) {
            let ext_ch_ind = m.ext_ch_ind.unwrap_or(0);
            if !is_valid_origination_ext_ch_ind(ext_ch_ind) {
                return Err(format!(
                    "EXT_CH_IND value {ext_ch_ind:#07b} is reserved or invalid"
                ));
            }
            bs.write_u8(ext_ch_ind, 5);
        }
    }

    if effective_p_rev_in_use >= 11 {
        if m.slot_cycle_index != 0 {
            bs.write_u8(m.sign_slot_cycle_index.unwrap_or(false) as u8, 1);
        }

        let bcmc_incl = m.bcmc_incl.unwrap_or(m.bcmc.is_some());
        bs.write_u8(bcmc_incl as u8, 1);
        if bcmc_incl {
            let bcmc = m
                .bcmc
                .as_ref()
                .ok_or_else(|| "BCMC_INCL set but BCMC fields missing".to_string())?;
            let bcmc_pref_incl = m.bcmc_pref_incl.unwrap_or_else(|| {
                bcmc.programs
                    .iter()
                    .any(|program| program.flows.iter().any(|flow| flow.bcmc_pref.is_some()))
            });
            bs.write_u8(bcmc_pref_incl as u8, 1);
            write_page_response_bcmc_fields(bs, bcmc, bcmc_pref_incl)?;
        }

        if m.for_pdch_supported == Some(true) {
            let rev_pdch_supported = m
                .rev_pdch_supported
                .unwrap_or(m.rev_pdch_capability.is_some());
            bs.write_u8(rev_pdch_supported as u8, 1);
            if rev_pdch_supported {
                write_rev_pdch_type_specific_fields(
                    bs,
                    m.rev_pdch_capability.as_ref().ok_or_else(|| {
                        "REV_PDCH_SUPPORTED set but REV_PDCH capability missing".to_string()
                    })?,
                )?;
            }
        }

        let band_sub_rep_incl = m
            .band_sub_rep_incl
            .unwrap_or(m.band_subclass_sup.as_ref().is_some_and(|v| !v.is_empty()));
        bs.write_u8(band_sub_rep_incl as u8, 1);
        if band_sub_rep_incl {
            let subclasses = m.band_subclass_sup.as_deref().unwrap_or(&[]);
            ensure_count("NUM_BAND_SUBCLASS", subclasses.len(), 15)?;
            let num = m.num_band_subclass.unwrap_or(subclasses.len() as u8);
            if num as usize != subclasses.len() {
                return Err(format!(
                    "NUM_BAND_SUBCLASS={} does not match {} BAND_SUBCLASS_SUP bits",
                    num,
                    subclasses.len()
                ));
            }
            bs.write_u8(num, 4);
            for &subclass in subclasses {
                bs.write_u8(subclass & 1, 1);
            }
        }
    }

    Ok(())
}

fn encode_security_mode_request_body(
    bs: &mut Bitstream,
    m: &SecurityModeRequestMessage,
) -> Result<(), String> {
    bs.write_u8(m.ui_encrypt_sup.is_some() as u8, 1);
    if let Some(ui_encrypt_sup) = m.ui_encrypt_sup {
        validate_ui_encrypt_sup(ui_encrypt_sup)?;
        bs.write_u8(ui_encrypt_sup, 8);
    }

    bs.write_u8(m.sig_encrypt_sup.is_some() as u8, 1);
    if let Some(sig_encrypt_sup) = m.sig_encrypt_sup {
        validate_sig_encrypt_sup(sig_encrypt_sup)?;
        bs.write_u8(sig_encrypt_sup, 8);
        bs.write_u8(m.c_sig_encrypt_req.unwrap_or(false) as u8, 1);
    }

    bs.write_u8(m.new_sseq_h.is_some() as u8, 1);
    if let Some(new_sseq_h) = m.new_sseq_h {
        bs.write_u32(new_sseq_h, 24);
        bs.write_u8(m.new_sseq_h_sig.unwrap_or(0), 8);
    }

    let msg_int_info_incl = m.msg_int_info_incl.unwrap_or(
        m.sig_integrity_sup_incl.is_some()
            || m.sig_integrity_sup.is_some()
            || m.sig_integrity_req.is_some(),
    );
    bs.write_u8(msg_int_info_incl as u8, 1);
    if msg_int_info_incl {
        let sig_integrity_sup_incl = m
            .sig_integrity_sup_incl
            .unwrap_or(m.sig_integrity_sup.is_some() || m.sig_integrity_req.is_some());
        bs.write_u8(sig_integrity_sup_incl as u8, 1);
        if sig_integrity_sup_incl {
            let sig_sup = m.sig_integrity_sup.unwrap_or(0);
            let sig_req = m.sig_integrity_req.unwrap_or(0);
            validate_sig_integrity_fields(sig_sup, sig_req)?;
            bs.write_u8(sig_sup, 8);
            bs.write_u8(sig_req, 3);
        }
    }
    Ok(())
}

fn encode_rdsch_security_mode_request_body(
    bs: &mut Bitstream,
    m: &SecurityModeRequestMessage,
) -> Result<(), String> {
    let ui_enc_incl = m.ui_encrypt_sup.is_some() || !m.ui_encrypt_records.is_empty();
    bs.write_u8(ui_enc_incl as u8, 1);
    if ui_enc_incl {
        let ui_sup = m.ui_encrypt_sup.unwrap_or(0);
        validate_ui_encrypt_sup(ui_sup)?;
        bs.write_u8(ui_sup, 8);
        if m.ui_encrypt_records.is_empty() {
            return Err(
                "r-dsch SMRM UI_ENC_INCL requires at least one UI encryption record".into(),
            );
        }
        ensure_count("NUM_RECS", m.ui_encrypt_records.len(), 8)?;
        bs.write_u8((m.ui_encrypt_records.len() - 1) as u8, 3);
        for record in &m.ui_encrypt_records {
            bs.write_u8(record.con_ref, 8);
            bs.write_u8(record.ui_encrypt_req as u8, 1);
        }
    }

    bs.write_u8(m.sig_encrypt_sup.is_some() as u8, 1);
    if let Some(sig_encrypt_sup) = m.sig_encrypt_sup {
        validate_sig_encrypt_sup(sig_encrypt_sup)?;
        bs.write_u8(sig_encrypt_sup, 8);
        bs.write_u8(m.d_sig_encrypt_req.unwrap_or(false) as u8, 1);
    }

    bs.write_u8(m.new_sseq_h.is_some() as u8, 1);
    if let Some(new_sseq_h) = m.new_sseq_h {
        bs.write_u32(new_sseq_h, 24);
        bs.write_u8(m.new_sseq_h_sig.unwrap_or(0), 8);
    }

    let msg_int_info_incl = m.msg_int_info_incl.unwrap_or(
        m.sig_integrity_sup_incl.is_some()
            || m.sig_integrity_sup.is_some()
            || m.sig_integrity_req.is_some(),
    );
    bs.write_u8(msg_int_info_incl as u8, 1);
    if msg_int_info_incl {
        let sig_integrity_sup_incl = m
            .sig_integrity_sup_incl
            .unwrap_or(m.sig_integrity_sup.is_some() || m.sig_integrity_req.is_some());
        bs.write_u8(sig_integrity_sup_incl as u8, 1);
        if sig_integrity_sup_incl {
            let sig_sup = m.sig_integrity_sup.unwrap_or(0);
            let sig_req = m.sig_integrity_req.unwrap_or(0);
            validate_sig_integrity_fields(sig_sup, sig_req)?;
            bs.write_u8(sig_sup, 8);
            bs.write_u8(sig_req, 3);
        }
    }
    Ok(())
}

fn encode_reconnect_body(
    bs: &mut Bitstream,
    m: &ReconnectMessage,
    ctx: AccessDecodeContext,
) -> Result<(), String> {
    bs.write_u8(m.orig_ind as u8, 1);
    bs.write_u8(m.sync_id_incl as u8, 1);
    if m.sync_id_incl {
        ensure_count("SYNC_ID_LEN", m.sync_id.len(), 15)?;
        bs.write_u8(m.sync_id.len() as u8, 4);
        write_octets(bs, &m.sync_id);
    } else {
        bs.write_u32(m.service_option.unwrap_or(0) as u32, 16);
    }
    if m.orig_ind {
        bs.write_u8(m.sr_id.unwrap_or(0), 3);
    }

    let p_rev_in_use = ctx.p_rev_in_use.unwrap_or(6);
    if m.orig_ind && p_rev_in_use >= 11 && m.sync_id_incl && m.sr_id != Some(0b111) {
        let incl = m.add_serv_instance_incl.unwrap_or(!m.add_sr_ids.is_empty());
        bs.write_u8(incl as u8, 1);
        if incl {
            ensure_count("NUM_ADD_SERV_INSTANCE", m.add_sr_ids.len(), 7)?;
            bs.write_u8(m.add_sr_ids.len() as u8, 3);
            for &sr_id in &m.add_sr_ids {
                bs.write_u8(sr_id, 3);
            }
        }
    }

    if p_rev_in_use >= 11 {
        let sdb_incl = m.sdb_incl.unwrap_or(!m.sdb_fields.is_empty());
        bs.write_u8(sdb_incl as u8, 1);
        if sdb_incl {
            ensure_count("NUM_FIELDS", m.sdb_fields.len(), u8::MAX as usize)?;
            bs.write_u8(m.sdb_fields.len() as u8, 8);
            write_octets(bs, &m.sdb_fields);
        }
    }
    Ok(())
}

fn decode_registration(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    Ok(AccessMessage::Registration(RegistrationMessage {
        header,
        reg_type: read(bs, 4, "REG_TYPE")? as u8,
        slot_cycle_index: read(bs, 3, "SLOT_CYCLE_INDEX")? as u8,
        mob_p_rev: read(bs, 8, "MOB_P_REV")? as u8,
        scm: read(bs, 8, "SCM")? as u8,
        mob_term: read(bs, 1, "MOB_TERM")? == 1,
        return_cause: read(bs, 4, "RETURN_CAUSE")? as u8,
        remaining_bits: bs.len(),
    }))
}

fn decode_origination(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let mob_term = read(bs, 1, "MOB_TERM")? == 1;
    let slot_cycle_index = read(bs, 3, "SLOT_CYCLE_INDEX")? as u8;
    let mob_p_rev = read(bs, 8, "MOB_P_REV")? as u8;
    let scm = read(bs, 8, "SCM")? as u8;
    let request_mode = read(bs, 3, "REQUEST_MODE")? as u8;
    let special_service = read(bs, 1, "SPECIAL_SERVICE")? == 1;
    let service_option = if special_service {
        Some(read(bs, 16, "SERVICE_OPTION")? as u16)
    } else {
        None
    };
    let pm = read(bs, 1, "PM")? == 1;
    let digit_mode = read(bs, 1, "DIGIT_MODE")? == 1;
    let number_type = if digit_mode || mob_p_rev >= 11 {
        Some(read(bs, 3, "NUMBER_TYPE")? as u8)
    } else {
        None
    };
    let number_plan = if digit_mode {
        Some(read(bs, 4, "NUMBER_PLAN")? as u8)
    } else {
        None
    };
    let more_fields = read(bs, 1, "MORE_FIELDS")? == 1;
    let num_fields = read(bs, 8, "NUM_FIELDS")? as u8;
    let char_bits = if digit_mode { 8 } else { 4 };
    let mut digits = Vec::with_capacity(num_fields as usize);
    for idx in 0..num_fields {
        digits.push(read(bs, char_bits, &format!("CHAR[{idx}]"))? as u8);
    }
    // Legacy MOB_P_REV<6 mobiles can omit trailing C.S0005-E fields; treat
    // them as their default values when the SDU runs out.
    let nar_an_cap = read(bs, 1, "NAR_AN_CAP").unwrap_or(0) == 1;
    let paca_reorig = read(bs, 1, "PACA_REORIG").unwrap_or(0) == 1;
    let return_cause = read(bs, 4, "RETURN_CAUSE").unwrap_or(0) as u8;
    let more_records = read(bs, 1, "MORE_RECORDS").unwrap_or(0) == 1;
    let tail = bs.clone();

    let base = OriginationBaseFields {
        header,
        mob_term,
        slot_cycle_index,
        mob_p_rev,
        scm,
        request_mode,
        special_service,
        service_option,
        pm,
        digit_mode,
        number_type,
        number_plan,
        more_fields,
        num_fields,
        digits,
        nar_an_cap,
        paca_reorig,
        return_cause,
        more_records,
    };

    let encryption_options: &[bool] = if mob_p_rev < 7 {
        &[true, false]
    } else {
        &[false]
    };
    let mut best: Option<(OriginationMessage, i32)> = None;
    let mut last_err: Option<String> = None;
    for &include_encryption_supported in encryption_options {
        match decode_origination_tail(&base, tail.clone(), include_encryption_supported) {
            Ok(msg) => {
                let score = score_origination_candidate(&msg, include_encryption_supported);
                if best
                    .as_ref()
                    .map_or(true, |(_, best_score)| score > *best_score)
                {
                    best = Some((msg, score));
                }
            }
            Err(err) => {
                last_err = Some(err);
            }
        }
    }

    match best {
        Some((msg, _)) => Ok(AccessMessage::Origination(msg)),
        None => Err(last_err.unwrap_or_else(|| "failed to decode origination tail".to_string())),
    }
}

#[derive(Clone)]
struct OriginationBaseFields {
    header: AccessMessageHeader,
    mob_term: bool,
    slot_cycle_index: u8,
    mob_p_rev: u8,
    scm: u8,
    request_mode: u8,
    special_service: bool,
    service_option: Option<u16>,
    pm: bool,
    digit_mode: bool,
    number_type: Option<u8>,
    number_plan: Option<u8>,
    more_fields: bool,
    num_fields: u8,
    digits: Vec<u8>,
    nar_an_cap: bool,
    paca_reorig: bool,
    return_cause: u8,
    more_records: bool,
}

fn decode_origination_tail(
    base: &OriginationBaseFields,
    mut bs: Bitstream,
    include_encryption_supported: bool,
) -> Result<OriginationMessage, String> {
    let encryption_supported = if include_encryption_supported {
        Some(read(&mut bs, 4, "ENCRYPTION_SUPPORTED")? as u8)
    } else {
        None
    };

    // Legacy tail fields default to absent/false if the SDU ends here.
    let paca_supported = read(&mut bs, 1, "PACA_SUPPORTED").unwrap_or(0) == 1;
    let num_alt_so = read(&mut bs, 3, "NUM_ALT_SO").unwrap_or(0) as u8;
    let mut alt_service_options = Vec::with_capacity(num_alt_so as usize);
    for idx in 0..num_alt_so {
        match read(&mut bs, 16, &format!("ALT_SO[{idx}]")) {
            Ok(v) => alt_service_options.push(v as u16),
            Err(_) => break,
        }
    }

    let mut drs = None;
    let mut uzid_incl = None;
    let mut uzid = None;
    let mut ch_ind = None;
    let mut sr_id = None;
    let mut otd_supported = None;
    let mut qpch_supported = None;
    let mut enhanced_rc = None;
    let mut for_rc_pref = None;
    let mut rev_rc_pref = None;
    let mut fch_supported = None;
    let mut fch_capability = None;
    let mut dcch_supported = None;
    let mut dcch_capability = None;
    let mut geo_loc_incl = None;
    let mut geo_loc_type = None;
    let mut rev_fch_gating_req = None;
    let mut orig_reason = None;
    let mut orig_count = None;
    let mut sts_supported = None;
    let mut cch_3x_supported = None;
    let mut wll_incl = None;
    let mut wll_device_type = None;
    let mut global_emergency_call = None;
    let mut ms_init_pos_loc_ind = None;
    let mut qos_parms_incl = None;
    let mut qos_parms_len = None;
    let mut qos_parms = Vec::new();
    let mut enc_info_incl = None;
    let mut sig_encrypt_sup = None;
    let mut d_sig_encrypt_req = None;
    let mut c_sig_encrypt_req = None;
    let mut new_sseq_h = None;
    let mut new_sseq_h_sig = None;
    let mut ui_encrypt_req = None;
    let mut ui_encrypt_sup = None;
    let mut sync_id_incl = None;
    let mut sync_id_len = None;
    let mut sync_id = None;
    let mut prev_sid_incl = None;
    let mut prev_sid = None;
    let mut prev_nid_incl = None;
    let mut prev_nid = None;
    let mut prev_pzid_incl = None;
    let mut prev_pzid = None;
    let mut so_bitmap_ind = None;
    let mut so_group_num = None;
    let mut so_bitmap = None;
    let mut sdb_desired_only = None;
    let mut alt_band_class_sup = None;
    let mut msg_int_info_incl = None;
    let mut sig_integrity_sup_incl = None;
    let mut sig_integrity_sup = None;
    let mut sig_integrity_req = None;
    let mut new_key_id = None;
    let mut new_sseq_h_incl = None;
    let mut for_pdch_supported = None;
    let mut for_pdch_capability = None;
    let mut ext_ch_ind = None;
    let mut sign_slot_cycle_index = None;
    let mut add_serv_instance_incl = None;
    let mut add_service_instances = Vec::new();
    let mut bcmc_incl = None;
    let mut bcmc = None;
    let mut rev_pdch_supported = None;
    let mut rev_pdch_capability = None;
    let mut band_sub_rep_incl = None;
    let mut num_band_subclass = None;
    let mut band_subclass_sup = Vec::new();
    let mut add_geo_loc_incl = None;
    let mut add_geo_loc_type_len_ind = None;
    let mut add_geo_loc_type = None;

    if base.mob_p_rev >= 6 {
        drs = Some(read(&mut bs, 1, "DRS")? == 1);
        let uzid_incl_value = read(&mut bs, 1, "UZID_INCL")? == 1;
        uzid_incl = Some(uzid_incl_value);
        if uzid_incl_value {
            uzid = Some(read(&mut bs, 16, "UZID")? as u16);
        }
        ch_ind = Some(read(&mut bs, 2, "CH_IND")? as u8);
        sr_id = Some(read(&mut bs, 3, "SR_ID")? as u8);
        otd_supported = Some(read(&mut bs, 1, "OTD_SUPPORTED")? == 1);
        qpch_supported = Some(read(&mut bs, 1, "QPCH_SUPPORTED")? == 1);
        enhanced_rc = Some(read(&mut bs, 1, "ENHANCED_RC")? == 1);
        for_rc_pref = Some(read(&mut bs, 5, "FOR_RC_PREF")? as u8);
        rev_rc_pref = Some(read(&mut bs, 5, "REV_RC_PREF")? as u8);

        let fch_supported_value = read(&mut bs, 1, "FCH_SUPPORTED")? == 1;
        fch_supported = Some(fch_supported_value);
        if fch_supported_value {
            fch_capability = Some(decode_fch_type_specific_fields(&mut bs)?);
        }

        let dcch_supported_value = read(&mut bs, 1, "DCCH_SUPPORTED")? == 1;
        dcch_supported = Some(dcch_supported_value);
        if dcch_supported_value {
            dcch_capability = Some(decode_dcch_type_specific_fields(&mut bs)?);
        }

        let geo_loc_incl_value = read(&mut bs, 1, "GEO_LOC_INCL")? == 1;
        geo_loc_incl = Some(geo_loc_incl_value);
        if geo_loc_incl_value {
            geo_loc_type = Some(read(&mut bs, 3, "GEO_LOC_TYPE")? as u8);
        }
        rev_fch_gating_req = Some(read(&mut bs, 1, "REV_FCH_GATING_REQ")? == 1);
    }

    if base.mob_p_rev >= 7 {
        orig_reason = Some(read(&mut bs, 1, "ORIG_REASON")? == 1);
        orig_count = Some(read(&mut bs, 2, "ORIG_COUNT")? as u8);
        sts_supported = Some(read(&mut bs, 1, "STS_SUPPORTED")? == 1);
        cch_3x_supported = Some(read(&mut bs, 1, "3X_CCH_SUPPORTED")? == 1);

        let wll_incl_value = read(&mut bs, 1, "WLL_INCL")? == 1;
        wll_incl = Some(wll_incl_value);
        if wll_incl_value {
            wll_device_type = Some(read(&mut bs, 3, "WLL_DEVICE_TYPE")? as u8);
        }

        let global_emergency_call_value = read(&mut bs, 1, "GLOBAL_EMERGENCY_CALL")? == 1;
        global_emergency_call = Some(global_emergency_call_value);
        if global_emergency_call_value {
            ms_init_pos_loc_ind = Some(read(&mut bs, 1, "MS_INIT_POS_LOC_IND")? == 1);
        }

        let qos_parms_incl_value = read(&mut bs, 1, "QOS_PARMS_INCL")? == 1;
        qos_parms_incl = Some(qos_parms_incl_value);
        if qos_parms_incl_value {
            let len = read(&mut bs, 5, "QOS_PARMS_LEN")? as u8;
            qos_parms_len = Some(len);
            qos_parms = read_octets(&mut bs, len as usize, "QOS_PARMS")?;
        }

        let enc_info_incl_value = read(&mut bs, 1, "ENC_INFO_INCL")? == 1;
        enc_info_incl = Some(enc_info_incl_value);
        if enc_info_incl_value {
            let sig_sup = read(&mut bs, 8, "SIG_ENCRYPT_SUP")? as u8;
            validate_sig_encrypt_sup(sig_sup)?;
            sig_encrypt_sup = Some(sig_sup);
            d_sig_encrypt_req = Some(read(&mut bs, 1, "D_SIG_ENCRYPT_REQ")? == 1);
            c_sig_encrypt_req = Some(read(&mut bs, 1, "C_SIG_ENCRYPT_REQ")? == 1);
            let ecmea = (sig_sup >> 6) & 1;
            let rea = (sig_sup >> 5) & 1;
            if ecmea == 1 || rea == 1 {
                new_sseq_h = Some(read(&mut bs, 24, "NEW_SSEQ_H")? as u32);
                new_sseq_h_sig = Some(read(&mut bs, 8, "NEW_SSEQ_H_SIG")? as u8);
            }
            ui_encrypt_req = Some(read(&mut bs, 1, "UI_ENCRYPT_REQ")? == 1);
            let ui_sup = read(&mut bs, 8, "UI_ENCRYPT_SUP")? as u8;
            validate_ui_encrypt_sup(ui_sup)?;
            ui_encrypt_sup = Some(ui_sup);
        }

        let sync_id_incl_value = read(&mut bs, 1, "SYNC_ID_INCL")? == 1;
        sync_id_incl = Some(sync_id_incl_value);
        if sync_id_incl_value {
            let len = read(&mut bs, 4, "SYNC_ID_LEN")? as u8;
            sync_id_len = Some(len);
            if len > 4 {
                return Err(format!("SYNC_ID_LEN={} exceeds local u32 storage", len));
            }
            if len > 0 {
                sync_id = Some(read(&mut bs, len as usize * 8, "SYNC_ID")? as u32);
            }
        }

        let prev_sid_incl_value = read(&mut bs, 1, "PREV_SID_INCL")? == 1;
        prev_sid_incl = Some(prev_sid_incl_value);
        if prev_sid_incl_value {
            prev_sid = Some(read(&mut bs, 15, "PREV_SID")? as u16);
        }

        let prev_nid_incl_value = read(&mut bs, 1, "PREV_NID_INCL")? == 1;
        prev_nid_incl = Some(prev_nid_incl_value);
        if prev_nid_incl_value {
            prev_nid = Some(read(&mut bs, 16, "PREV_NID")? as u16);
        }

        let prev_pzid_incl_value = read(&mut bs, 1, "PREV_PZID_INCL")? == 1;
        prev_pzid_incl = Some(prev_pzid_incl_value);
        if prev_pzid_incl_value {
            prev_pzid = Some(read(&mut bs, 8, "PREV_PZID")? as u8);
        }

        let so_bitmap_ind_value = read(&mut bs, 2, "SO_BITMAP_IND")? as u8;
        so_bitmap_ind = Some(so_bitmap_ind_value);
        if so_bitmap_ind_value > 0 {
            so_group_num = Some(read(&mut bs, 5, "SO_GROUP_NUM")? as u8);
            let bitmap_bits = 1usize << (1 + so_bitmap_ind_value as usize);
            so_bitmap = Some(read(&mut bs, bitmap_bits, "SO_BITMAP")? as u16);
        }
    }

    if base.mob_p_rev >= 8 {
        sdb_desired_only = Some(read(&mut bs, 1, "SDB_DESIRED_ONLY")? == 1);
        alt_band_class_sup = Some(read(&mut bs, 1, "ALT_BAND_CLASS_SUP")? == 1);
    }

    if base.mob_p_rev >= 9 {
        let msg_int_info_incl_value = read(&mut bs, 1, "MSG_INT_INFO_INCL")? == 1;
        msg_int_info_incl = Some(msg_int_info_incl_value);
        if msg_int_info_incl_value {
            let sig_integrity_sup_incl_value = read(&mut bs, 1, "SIG_INTEGRITY_SUP_INCL")? == 1;
            sig_integrity_sup_incl = Some(sig_integrity_sup_incl_value);
            if sig_integrity_sup_incl_value {
                let sig_sup = read(&mut bs, 8, "SIG_INTEGRITY_SUP")? as u8;
                let sig_req = read(&mut bs, 3, "SIG_INTEGRITY_REQ")? as u8;
                validate_sig_integrity_fields(sig_sup, sig_req)?;
                sig_integrity_sup = Some(sig_sup);
                sig_integrity_req = Some(sig_req);
            }
            new_key_id = Some(read(&mut bs, 2, "NEW_KEY_ID")? as u8);
            let new_sseq_h_incl_value = read(&mut bs, 1, "NEW_SSEQ_H_INCL")? == 1;
            new_sseq_h_incl = Some(new_sseq_h_incl_value);
            if new_sseq_h_incl_value {
                new_sseq_h = Some(read(&mut bs, 24, "NEW_SSEQ_H")? as u32);
                new_sseq_h_sig = Some(read(&mut bs, 8, "NEW_SSEQ_H_SIG")? as u8);
            }
        }

        let for_pdch_supported_value = read(&mut bs, 1, "FOR_PDCH_SUPPORTED")? == 1;
        for_pdch_supported = Some(for_pdch_supported_value);
        if for_pdch_supported_value {
            for_pdch_capability = Some(decode_for_pdch_type_specific_fields(&mut bs)?);
        }

        if ch_ind == Some(0) {
            let value = read(&mut bs, 5, "EXT_CH_IND")? as u8;
            if !is_valid_origination_ext_ch_ind(value) {
                return Err(format!("EXT_CH_IND=0b{value:05b} is reserved"));
            }
            ext_ch_ind = Some(value);
        }
    }

    if base.mob_p_rev >= 11 {
        if base.slot_cycle_index != 0 {
            sign_slot_cycle_index = Some(read(&mut bs, 1, "SIGN_SLOT_CYCLE_INDEX")? == 1);
        }

        if sr_id != Some(0b111) {
            let add_serv_instance_incl_value = read(&mut bs, 1, "ADD_SERV_INSTANCE_INCL")? == 1;
            add_serv_instance_incl = Some(add_serv_instance_incl_value);
            if add_serv_instance_incl_value {
                let num_add_serv_instance = read(&mut bs, 3, "NUM_ADD_SERV_INSTANCE")? as u8;
                let sync_id_present = sync_id_incl == Some(true);
                for idx in 0..num_add_serv_instance {
                    let add_sr_id = read(&mut bs, 3, &format!("ADD_SR_ID[{idx}]"))? as u8;
                    let add_drs = read(&mut bs, 1, &format!("ADD_DRS[{idx}]"))? == 1;
                    let mut add_service_option_incl = None;
                    let mut add_service_option = None;
                    let mut add_qos_parms_incl = None;
                    let mut add_qos_parms_len = None;
                    let mut add_qos_parms = Vec::new();
                    if !sync_id_present {
                        let incl =
                            read(&mut bs, 1, &format!("ADD_SERVICE_OPTION_INCL[{idx}]"))? == 1;
                        add_service_option_incl = Some(incl);
                        if incl {
                            add_service_option =
                                Some(read(&mut bs, 16, &format!("ADD_SERVICE_OPTION[{idx}]"))?
                                    as u16);
                        }
                        let qos_incl =
                            read(&mut bs, 1, &format!("ADD_QOS_PARMS_INCL[{idx}]"))? == 1;
                        add_qos_parms_incl = Some(qos_incl);
                        if qos_incl {
                            let len = read(&mut bs, 5, &format!("ADD_QOS_PARMS_LEN[{idx}]"))? as u8;
                            add_qos_parms_len = Some(len);
                            add_qos_parms = read_octets(
                                &mut bs,
                                len as usize,
                                &format!("ADD_QOS_PARMS[{idx}]"),
                            )?;
                        }
                    }
                    add_service_instances.push(OriginationAdditionalServiceInstance {
                        add_sr_id,
                        add_drs,
                        add_service_option_incl,
                        add_service_option,
                        add_qos_parms_incl,
                        add_qos_parms_len,
                        add_qos_parms,
                    });
                }
            }
        }

        let bcmc_incl_value = read(&mut bs, 1, "BCMC_INCL")? == 1;
        bcmc_incl = Some(bcmc_incl_value);
        if bcmc_incl_value {
            bcmc = Some(decode_origination_bcmc_fields(&mut bs)?);
        }

        if for_pdch_supported == Some(true) {
            let rev_pdch_supported_value = read(&mut bs, 1, "REV_PDCH_SUPPORTED")? == 1;
            rev_pdch_supported = Some(rev_pdch_supported_value);
            if rev_pdch_supported_value {
                rev_pdch_capability = Some(decode_rev_pdch_type_specific_fields(&mut bs)?);
            }
        }

        let band_sub_rep_incl_value = read(&mut bs, 1, "BAND_SUB_REP_INCL")? == 1;
        band_sub_rep_incl = Some(band_sub_rep_incl_value);
        if band_sub_rep_incl_value {
            let num = read(&mut bs, 4, "NUM_BAND_SUBCLASS")? as u8;
            num_band_subclass = Some(num);
            for idx in 0..num {
                band_subclass_sup
                    .push(read(&mut bs, 1, &format!("BAND_SUBCLASS_SUP[{idx}]"))? as u8);
            }
        }
    }

    if base.mob_p_rev >= 12 {
        let add_geo_loc_incl_value = read(&mut bs, 1, "ADD_GEO_LOC_INCL")? == 1;
        add_geo_loc_incl = Some(add_geo_loc_incl_value);
        if add_geo_loc_incl_value {
            let len_ind = read(&mut bs, 1, "ADD_GEO_LOC_TYPE_LEN_IND")? == 1;
            add_geo_loc_type_len_ind = Some(len_ind);
            let bits = if len_ind { 24 } else { 16 };
            add_geo_loc_type = Some(read(&mut bs, bits, "ADD_GEO_LOC_TYPE")? as u32);
        }
    }

    Ok(OriginationMessage {
        header: base.header.clone(),
        mob_term: base.mob_term,
        slot_cycle_index: base.slot_cycle_index,
        mob_p_rev: base.mob_p_rev,
        scm: base.scm,
        request_mode: base.request_mode,
        special_service: base.special_service,
        service_option: base.service_option,
        pm: base.pm,
        digit_mode: base.digit_mode,
        number_type: base.number_type,
        number_plan: base.number_plan,
        more_fields: base.more_fields,
        num_fields: base.num_fields,
        digits: base.digits.clone(),
        nar_an_cap: base.nar_an_cap,
        paca_reorig: base.paca_reorig,
        return_cause: base.return_cause,
        more_records: base.more_records,
        encryption_supported,
        paca_supported,
        num_alt_so,
        alt_service_options,
        drs,
        uzid_incl,
        uzid,
        ch_ind,
        sr_id,
        otd_supported,
        qpch_supported,
        enhanced_rc,
        for_rc_pref,
        rev_rc_pref,
        fch_supported,
        fch_capability,
        dcch_supported,
        dcch_capability,
        geo_loc_incl,
        geo_loc_type,
        rev_fch_gating_req,
        orig_reason,
        orig_count,
        sts_supported,
        cch_3x_supported,
        wll_incl,
        wll_device_type,
        global_emergency_call,
        ms_init_pos_loc_ind,
        qos_parms_incl,
        qos_parms_len,
        qos_parms,
        enc_info_incl,
        sig_encrypt_sup,
        d_sig_encrypt_req,
        c_sig_encrypt_req,
        new_sseq_h,
        new_sseq_h_sig,
        ui_encrypt_req,
        ui_encrypt_sup,
        sync_id_incl,
        sync_id_len,
        sync_id,
        prev_sid_incl,
        prev_sid,
        prev_nid_incl,
        prev_nid,
        prev_pzid_incl,
        prev_pzid,
        so_bitmap_ind,
        so_group_num,
        so_bitmap,
        sdb_desired_only,
        alt_band_class_sup,
        msg_int_info_incl,
        sig_integrity_sup_incl,
        sig_integrity_sup,
        sig_integrity_req,
        new_key_id,
        new_sseq_h_incl,
        for_pdch_supported,
        for_pdch_capability,
        ext_ch_ind,
        sign_slot_cycle_index,
        add_serv_instance_incl,
        add_service_instances,
        bcmc_incl,
        bcmc,
        rev_pdch_supported,
        rev_pdch_capability,
        band_sub_rep_incl,
        num_band_subclass,
        band_subclass_sup,
        add_geo_loc_incl,
        add_geo_loc_type_len_ind,
        add_geo_loc_type,
        remaining_bits: bs.len(),
    })
}

fn decode_fch_type_specific_fields(bs: &mut Bitstream) -> Result<FchTypeSpecificFields, String> {
    let frame_size_5ms_supported = read(bs, 1, "FCH_FRAME_SIZE")? == 1;
    let for_fch_len = read(bs, 3, "FOR_FCH_LEN")? as u8;
    let for_map_bits = (for_fch_len as usize) * 3;
    if bs.len() < for_map_bits {
        return Err(format!(
            "EOF reading FOR_FCH_RC_MAP ({} bits)",
            for_map_bits
        ));
    }
    let for_fch_rc_map_raw = bs.drain(0..for_map_bits);
    let for_supported_rcs =
        decode_supported_rcs(&for_fch_rc_map_raw, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12]);
    let rev_fch_len = read(bs, 3, "REV_FCH_LEN")? as u8;
    let rev_map_bits = (rev_fch_len as usize) * 3;
    if bs.len() < rev_map_bits {
        return Err(format!(
            "EOF reading REV_FCH_RC_MAP ({} bits)",
            rev_map_bits
        ));
    }
    let rev_fch_rc_map_raw = bs.drain(0..rev_map_bits);
    let rev_supported_rcs = decode_supported_rcs(&rev_fch_rc_map_raw, &[1, 2, 3, 4, 5, 6, 8]);

    Ok(FchTypeSpecificFields {
        frame_size_5ms_supported,
        for_fch_len,
        for_fch_rc_map_raw,
        for_supported_rcs,
        rev_fch_len,
        rev_fch_rc_map_raw,
        rev_supported_rcs,
    })
}

fn decode_dcch_type_specific_fields(bs: &mut Bitstream) -> Result<DcchTypeSpecificFields, String> {
    let frame_size_mode = read(bs, 2, "DCCH_FRAME_SIZE")? as u8;
    let for_dcch_len = read(bs, 3, "FOR_DCCH_LEN")? as u8;
    let for_map_bits = (for_dcch_len as usize) * 3;
    if bs.len() < for_map_bits {
        return Err(format!(
            "EOF reading FOR_DCCH_RC_MAP ({} bits)",
            for_map_bits
        ));
    }
    let for_dcch_rc_map_raw = bs.drain(0..for_map_bits);
    let for_supported_rcs =
        decode_supported_rcs(&for_dcch_rc_map_raw, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12]);
    let rev_dcch_len = read(bs, 3, "REV_DCCH_LEN")? as u8;
    let rev_map_bits = (rev_dcch_len as usize) * 3;
    if bs.len() < rev_map_bits {
        return Err(format!(
            "EOF reading REV_DCCH_RC_MAP ({} bits)",
            rev_map_bits
        ));
    }
    let rev_dcch_rc_map_raw = bs.drain(0..rev_map_bits);
    let rev_supported_rcs = decode_supported_rcs(&rev_dcch_rc_map_raw, &[1, 2, 3, 4, 5, 6, 8]);

    Ok(DcchTypeSpecificFields {
        frame_size_mode,
        for_dcch_len,
        for_dcch_rc_map_raw,
        for_supported_rcs,
        rev_dcch_len,
        rev_dcch_rc_map_raw,
        rev_supported_rcs,
    })
}

fn decode_for_pdch_type_specific_fields(
    bs: &mut Bitstream,
) -> Result<ForPdchTypeSpecificFields, String> {
    let ack_delay = read(bs, 1, "ACK_DELAY")? == 1;
    let num_arq_chan = read(bs, 2, "NUM_ARQ_CHAN")? as u8;
    if num_arq_chan == 0b11 {
        return Err("NUM_ARQ_CHAN value 0b11 is reserved".to_string());
    }

    let for_pdch_len = read(bs, 2, "FOR_PDCH_LEN")? as u8;
    let rc_map_bits = (for_pdch_len as usize + 1) * 3;
    if bs.len() < rc_map_bits {
        return Err(format!(
            "EOF reading FOR_PDCH_RC_MAP ({} bits)",
            rc_map_bits
        ));
    }
    let for_pdch_rc_map_raw = bs.drain(0..rc_map_bits);
    if for_pdch_rc_map_raw
        .bits()
        .iter()
        .skip(1)
        .any(|bit| *bit != 0)
    {
        return Err("FOR_PDCH_RC_MAP reserved bits must be zero".to_string());
    }
    let for_pdch_supported_rcs = decode_supported_rcs(&for_pdch_rc_map_raw, &[10]);

    let ch_config_sup_map_len = read(bs, 2, "CH_CONFIG_SUP_MAP_LEN")? as u8;
    let config_map_bits = (ch_config_sup_map_len as usize + 1) * 3;
    if bs.len() < config_map_bits {
        return Err(format!(
            "EOF reading CH_CONFIG_SUP_MAP ({} bits)",
            config_map_bits
        ));
    }
    let ch_config_sup_map_raw = bs.drain(0..config_map_bits);
    validate_for_pdch_channel_config_map(&ch_config_sup_map_raw)?;
    let ch_config_supported = decode_supported_rcs(&ch_config_sup_map_raw, &[1, 2, 3, 4, 5, 6]);

    Ok(ForPdchTypeSpecificFields {
        ack_delay,
        num_arq_chan,
        for_pdch_len,
        for_pdch_rc_map_raw,
        for_pdch_supported_rcs,
        ch_config_sup_map_len,
        ch_config_sup_map_raw,
        ch_config_supported,
    })
}

fn decode_rev_pdch_type_specific_fields(
    bs: &mut Bitstream,
) -> Result<RevPdchTypeSpecificFields, String> {
    let rev_pdch_len = read(bs, 2, "REV_PDCH_LEN")? as u8;
    let rc_map_bits = (rev_pdch_len as usize + 1) * 3;
    if bs.len() < rc_map_bits {
        return Err(format!(
            "EOF reading REV_PDCH_RC_MAP ({} bits)",
            rc_map_bits
        ));
    }
    let rev_pdch_rc_map_raw = bs.drain(0..rc_map_bits);
    if rev_pdch_rc_map_raw
        .bits()
        .iter()
        .skip(1)
        .any(|bit| *bit != 0)
    {
        return Err("REV_PDCH_RC_MAP reserved bits must be zero".to_string());
    }
    let rev_pdch_supported_rcs = decode_supported_rcs(&rev_pdch_rc_map_raw, &[7]);

    let rev_pdch_ch_config_sup_map_len = read(bs, 2, "REV_PDCH_CH_CONFIG_SUP_MAP_LEN")? as u8;
    let config_map_bits = (rev_pdch_ch_config_sup_map_len as usize + 1) * 3;
    if bs.len() < config_map_bits {
        return Err(format!(
            "EOF reading REV_PDCH_CH_CONFIG_SUP_MAP ({} bits)",
            config_map_bits
        ));
    }
    let rev_pdch_ch_config_sup_map_raw = bs.drain(0..config_map_bits);
    validate_rev_pdch_channel_config_map(&rev_pdch_ch_config_sup_map_raw)?;
    let rev_pdch_ch_config_supported =
        decode_supported_rcs(&rev_pdch_ch_config_sup_map_raw, &[0, 1, 2, 3, 4, 5, 6]);

    let rev_pdch_max_size_supported_encoder_packet =
        read(bs, 2, "REV_PDCH_MAX_SIZE_SUPPORTED_ENCODER_PACKET")? as u8;
    if rev_pdch_max_size_supported_encoder_packet == 0b11 {
        return Err(
            "REV_PDCH_MAX_SIZE_SUPPORTED_ENCODER_PACKET value 0b11 is reserved".to_string(),
        );
    }

    Ok(RevPdchTypeSpecificFields {
        rev_pdch_len,
        rev_pdch_rc_map_raw,
        rev_pdch_supported_rcs,
        rev_pdch_ch_config_sup_map_len,
        rev_pdch_ch_config_sup_map_raw,
        rev_pdch_ch_config_supported,
        rev_pdch_max_size_supported_encoder_packet,
    })
}

fn decode_fundicated_bcmc_type_specific_fields(
    bs: &mut Bitstream,
) -> Result<FundicatedBcmcTypeSpecificFields, String> {
    let fundicated_bcmc_ch_sup_map_len = read(bs, 2, "FUNDICATED_BCMC_CH_SUP_MAP_LEN")? as u8;
    let map_bits = (fundicated_bcmc_ch_sup_map_len as usize + 1) * 3;
    if bs.len() < map_bits {
        return Err(format!(
            "EOF reading FUNDICATED_BCMC_CH_SUP_MAP ({} bits)",
            map_bits
        ));
    }
    let fundicated_bcmc_ch_sup_map_raw = bs.drain(0..map_bits);
    let supported_configurations =
        decode_supported_rcs(&fundicated_bcmc_ch_sup_map_raw, &[1, 2, 3, 4, 5]);
    let cap = FundicatedBcmcTypeSpecificFields {
        fundicated_bcmc_ch_sup_map_len,
        fundicated_bcmc_ch_sup_map_raw,
        supported_configurations,
    };
    validate_fundicated_bcmc_fields(&cap)?;
    Ok(cap)
}

fn decode_origination_bcmc_fields(bs: &mut Bitstream) -> Result<OriginationBcmcFields, String> {
    decode_bcmc_fields(bs, true, false)
}

fn decode_page_response_bcmc_fields(
    bs: &mut Bitstream,
    bcmc_pref_incl: bool,
) -> Result<OriginationBcmcFields, String> {
    decode_bcmc_fields(bs, false, bcmc_pref_incl)
}

fn decode_bcmc_fields(
    bs: &mut Bitstream,
    include_orig_only_ind: bool,
    bcmc_pref_incl: bool,
) -> Result<OriginationBcmcFields, String> {
    let bcmc_orig_only_ind = if include_orig_only_ind {
        read(bs, 1, "BCMC_ORIG_ONLY_IND")? == 1
    } else {
        false
    };
    let fundicated_bcmc_supported = read(bs, 1, "FUNDICATED_BCMC_SUPPORTED")? == 1;
    let fundicated_bcmc_capability = if fundicated_bcmc_supported {
        Some(decode_fundicated_bcmc_type_specific_fields(bs)?)
    } else {
        None
    };
    let auth_signature_incl = read(bs, 1, "AUTH_SIGNATURE_INCL")? == 1;
    let (time_stamp_short_length, time_stamp_short) = if auth_signature_incl {
        let len = read(bs, 8, "TIME_STAMP_SHORT_LENGTH")? as u8;
        if bs.len() < len as usize {
            return Err(format!("EOF reading TIME_STAMP_SHORT ({} bits)", len));
        }
        (Some(len), bs.drain(0..len as usize))
    } else {
        (None, Bitstream::new())
    };
    let num_bcmc_programs = read(bs, 3, "NUM_BCMC_PROGRAMS")? as u8;
    let count = num_bcmc_programs as usize + 1;
    let mut programs = Vec::with_capacity(count);
    for idx in 0..count {
        programs.push(decode_bcmc_program(
            bs,
            bcmc_pref_incl,
            auth_signature_incl,
            idx,
        )?);
    }
    if auth_signature_incl
        && !programs.iter().any(|program| {
            program
                .flows
                .iter()
                .any(|flow| flow.auth_signature_ind == Some(true))
        })
    {
        return Err(
            "AUTH_SIGNATURE_INCL set but no BCMC flow has AUTH_SIGNATURE_IND=1".to_string(),
        );
    }
    Ok(OriginationBcmcFields {
        bcmc_orig_only_ind,
        fundicated_bcmc_supported,
        fundicated_bcmc_capability,
        auth_signature_incl,
        time_stamp_short_length,
        time_stamp_short,
        num_bcmc_programs,
        programs,
    })
}

fn decode_bcmc_program(
    bs: &mut Bitstream,
    bcmc_pref_incl: bool,
    auth_signature_incl: bool,
    program_idx: usize,
) -> Result<OriginationBcmcProgram, String> {
    let bcmc_program_id_len = read(bs, 5, &format!("BCMC_PROGRAM_ID_LEN[{program_idx}]"))? as u8;
    let program_id_bits = bcmc_program_id_len as usize + 1;
    if bs.len() < program_id_bits {
        return Err(format!(
            "EOF reading BCMC_PROGRAM_ID[{program_idx}] ({} bits)",
            program_id_bits
        ));
    }
    let bcmc_program_id = bs.drain(0..program_id_bits);
    let bcmc_flow_discriminator_len = read(
        bs,
        3,
        &format!("BCMC_FLOW_DISCRIMINATOR_LEN[{program_idx}]"),
    )? as u8;
    let flow_bits = bcmc_flow_discriminator_len as usize;
    let (num_flow_discriminator, count) = if flow_bits == 0 {
        (None, 1usize)
    } else {
        let num = read(
            bs,
            flow_bits,
            &format!("NUM_FLOW_DISCRIMINATOR[{program_idx}]"),
        )? as u32;
        (Some(num), num as usize + 1)
    };
    let mut flows = Vec::with_capacity(count);
    for flow_idx in 0..count {
        flows.push(decode_bcmc_flow(
            bs,
            bcmc_pref_incl,
            auth_signature_incl,
            flow_bits,
            program_idx,
            flow_idx,
        )?);
    }
    Ok(OriginationBcmcProgram {
        bcmc_program_id_len,
        bcmc_program_id,
        bcmc_flow_discriminator_len,
        num_flow_discriminator,
        flows,
    })
}

fn decode_bcmc_flow(
    bs: &mut Bitstream,
    bcmc_pref_incl: bool,
    auth_signature_incl: bool,
    flow_bits: usize,
    program_idx: usize,
    flow_idx: usize,
) -> Result<OriginationBcmcFlow, String> {
    if bs.len() < flow_bits {
        return Err(format!(
            "EOF reading BCMC_FLOW_DISCRIMINATOR[{program_idx}][{flow_idx}] ({} bits)",
            flow_bits
        ));
    }
    let bcmc_flow_discriminator = bs.drain(0..flow_bits);
    let bcmc_pref = if bcmc_pref_incl {
        Some(read(bs, 1, &format!("BCMC_PREF[{program_idx}][{flow_idx}]"))? == 1)
    } else {
        None
    };
    let mut auth_signature_ind = None;
    let mut auth_signature_same_ind = None;
    let mut bak_id = None;
    let mut auth_signature = None;
    if auth_signature_incl {
        let auth_ind = read(
            bs,
            1,
            &format!("AUTH_SIGNATURE_IND[{program_idx}][{flow_idx}]"),
        )? == 1;
        auth_signature_ind = Some(auth_ind);
        if auth_ind {
            let same_ind = read(
                bs,
                1,
                &format!("AUTH_SIGNATURE_SAME_IND[{program_idx}][{flow_idx}]"),
            )? == 1;
            if program_idx == 0 && flow_idx == 0 && same_ind {
                return Err("first BCMC flow AUTH_SIGNATURE_SAME_IND must be zero".to_string());
            }
            auth_signature_same_ind = Some(same_ind);
            if !same_ind {
                bak_id = Some(read(bs, 4, &format!("BAK_ID[{program_idx}][{flow_idx}]"))? as u8);
                auth_signature = Some(read(
                    bs,
                    32,
                    &format!("AUTH_SIGNATURE[{program_idx}][{flow_idx}]"),
                )? as u32);
            }
        }
    }
    Ok(OriginationBcmcFlow {
        bcmc_flow_discriminator,
        bcmc_pref,
        auth_signature_ind,
        auth_signature_same_ind,
        bak_id,
        auth_signature,
    })
}

fn decode_supported_rcs(raw: &Bitstream, rc_positions: &[u8]) -> Vec<u8> {
    raw.bits()
        .iter()
        .zip(rc_positions.iter())
        .filter_map(|(&bit, &rc)| if bit == 1 { Some(rc) } else { None })
        .collect()
}

fn validate_for_pdch_channel_config_map(raw: &Bitstream) -> Result<(), String> {
    let bits = raw.bits();
    if bits.iter().skip(6).any(|bit| *bit != 0) {
        return Err("CH_CONFIG_SUP_MAP reserved bits must be zero".to_string());
    }
    let supported = |idx: usize| bits.get(idx).copied().unwrap_or(0) == 1;
    if !supported(0) && !supported(1) {
        return Err("CH_CONFIG_SUP_MAP must set F-PDCH_1 or F-PDCH_2".to_string());
    }
    if supported(0) != supported(2) {
        return Err("CH_CONFIG_SUP_MAP F-PDCH_1 and F-PDCH_3 must match".to_string());
    }
    if supported(1) != supported(3) {
        return Err("CH_CONFIG_SUP_MAP F-PDCH_2 and F-PDCH_4 must match".to_string());
    }
    Ok(())
}

fn validate_rev_pdch_channel_config_map(raw: &Bitstream) -> Result<(), String> {
    let bits = raw.bits();
    if bits.iter().skip(7).any(|bit| *bit != 0) {
        return Err("REV_PDCH_CH_CONFIG_SUP_MAP reserved bits must be zero".to_string());
    }
    let supported = |idx: usize| bits.get(idx).copied().unwrap_or(0) == 1;
    if !supported(0) {
        return Err("REV_PDCH_CH_CONFIG_SUP_MAP must set F/R-PDCH_0".to_string());
    }
    if !supported(1) && !supported(2) {
        return Err("REV_PDCH_CH_CONFIG_SUP_MAP must set F/R-PDCH_1 or F/R-PDCH_2".to_string());
    }
    if supported(1) != supported(3) {
        return Err("REV_PDCH_CH_CONFIG_SUP_MAP F/R-PDCH_1 and F/R-PDCH_3 must match".to_string());
    }
    if supported(2) != supported(4) {
        return Err("REV_PDCH_CH_CONFIG_SUP_MAP F/R-PDCH_2 and F/R-PDCH_4 must match".to_string());
    }
    Ok(())
}

fn is_valid_origination_ext_ch_ind(value: u8) -> bool {
    matches!(
        value,
        0b00001..=0b00110 | 0b01000..=0b10110
    )
}

fn is_valid_rsci(value: u8) -> bool {
    matches!(value, 0b0000..=0b0100 | 0b0111 | 0b1001..=0b1110)
}

fn score_origination_candidate(
    msg: &OriginationMessage,
    include_encryption_supported: bool,
) -> i32 {
    let mut score = 0i32;
    score -= msg.remaining_bits as i32;
    if msg.paca_supported {
        score += 40;
    }
    if msg.num_alt_so as usize == msg.alt_service_options.len() {
        score += 20;
    }
    if msg.ch_ind.is_some_and(|v| v <= 0b11) {
        score += 8;
    }
    if msg.sr_id.is_some_and(|v| v <= 0b111) {
        score += 8;
    }
    if msg.mob_p_rev == 6 && msg.geo_loc_incl == Some(false) {
        score += 25;
    }
    if msg.fch_supported == Some(true) && msg.fch_capability.is_some() {
        score += 10;
    }
    if msg.dcch_supported == Some(true) && msg.dcch_capability.is_some() {
        score += 10;
    }
    if msg.remaining_bits == 0 {
        score += 100;
    }
    if include_encryption_supported && msg.encryption_supported.is_some() {
        score += 2;
    }

    // Penalize invalid FOR_RC_PREF / REV_RC_PREF values.
    // Per C.S0005-E Table 3.7.2.3.2.21-4, valid RC values are 1–12
    // (encoded as 00001–01100). Values 0 or >12 are reserved/undefined
    // and strongly suggest a bit-alignment error (e.g. ENCRYPTION_SUPPORTED
    // was incorrectly included, shifting all subsequent fields by 4 bits).
    if let Some(rc) = msg.for_rc_pref {
        if rc == 0 || rc > 12 {
            score -= 80;
        }
    }
    if let Some(rc) = msg.rev_rc_pref {
        if rc == 0 || rc > 12 {
            score -= 80;
        }
    }

    // Penalize FCH_SUPPORTED=true with empty forward RC list — the mobile
    // would not declare FCH support without advertising at least one forward RC.
    if msg.fch_supported == Some(true) {
        if let Some(ref fch) = msg.fch_capability {
            if fch.for_supported_rcs.is_empty() {
                score -= 50;
            }
        }
    }

    score
}

fn decode_order(header: AccessMessageHeader, bs: &mut Bitstream) -> Result<AccessMessage, String> {
    let order = read(bs, 6, "ORDER")? as u8;
    let add_record_len = read(bs, 3, "ADD_RECORD_LEN")? as u8;
    let mut order_specific = Vec::with_capacity(add_record_len as usize);
    for idx in 0..add_record_len {
        order_specific.push(read(bs, 8, &format!("ORDFIELD[{idx}]"))? as u8);
    }
    Ok(AccessMessage::Order(OrderMessage {
        header,
        order,
        add_record_len,
        order_specific,
        remaining_bits: bs.len(),
    }))
}

fn decode_data_burst(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let msg_number = read(bs, 8, "MSG_NUMBER")? as u8;
    let burst_type = read(bs, 6, "BURST_TYPE")? as u8;
    let num_msgs = read(bs, 8, "NUM_MSGS")? as u8;
    let num_fields = read(bs, 8, "NUM_FIELDS")? as u8;
    let mut fields = Vec::with_capacity(num_fields as usize);
    for idx in 0..num_fields {
        fields.push(read(bs, 8, &format!("CHAR[{idx}]"))? as u8);
    }
    Ok(AccessMessage::DataBurst(DataBurstMessage {
        header,
        msg_number,
        burst_type,
        num_msgs,
        num_fields,
        fields,
        remaining_bits: bs.len(),
    }))
}

fn decode_no_field_message(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
    wrap: fn(NoFieldAccessMessage) -> AccessMessage,
) -> Result<AccessMessage, String> {
    Ok(wrap(NoFieldAccessMessage {
        header,
        remaining_bits: bs.len(),
    }))
}

fn decode_auth_challenge_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    Ok(AccessMessage::AuthChallengeResponse(
        AuthChallengeResponseMessage {
            header,
            authu: read(bs, 18, "AUTHU")? as u32,
            remaining_bits: bs.len(),
        },
    ))
}

fn decode_auth_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let res = read_octets(bs, 16, "RES")?;
    let sig_integrity_sup_incl = read(bs, 1, "SIG_INTEGRITY_SUP_INCL")? != 0;
    let (sig_integrity_sup, sig_integrity_req) = if sig_integrity_sup_incl {
        let sig_sup = read(bs, 8, "SIG_INTEGRITY_SUP")? as u8;
        let sig_req = read(bs, 3, "SIG_INTEGRITY_REQ")? as u8;
        validate_sig_integrity_fields(sig_sup, sig_req)?;
        (Some(sig_sup), Some(sig_req))
    } else {
        (None, None)
    };
    let new_key_id = read(bs, 2, "NEW_KEY_ID")? as u8;
    let new_sseq_h = read(bs, 24, "NEW_SSEQ_H")? as u32;
    Ok(AccessMessage::AuthResponse(AuthResponseMessage {
        header,
        res,
        sig_integrity_sup,
        sig_integrity_req,
        new_key_id,
        new_sseq_h,
        remaining_bits: bs.len(),
    }))
}

fn decode_auth_resync(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    Ok(AccessMessage::AuthResync(AuthResyncMessage {
        header,
        con_ms_sqn: read_octets(bs, 6, "CON_MS_SQN")?,
        mac_s: read_octets(bs, 8, "MAC_S")?,
        remaining_bits: bs.len(),
    }))
}

fn decode_general_extension(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let num_ge_records = read(bs, 8, "NUM_GE_REC")? as u8;
    if num_ge_records == 0 {
        return Err("General Extension requires NUM_GE_REC > 0".to_string());
    }
    let records = read_info_records(bs, num_ge_records as usize, "GE_REC")?;
    let message_type = read(bs, 8, "MESSAGE_TYPE")? as u8;
    let message_record = bs.drain(0..bs.len());
    Ok(AccessMessage::GeneralExtension(GeneralExtensionMessage {
        header,
        num_ge_records,
        records,
        message_type,
        message_record,
        remaining_bits: bs.len(),
    }))
}

fn decode_flash_with_info(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let records = read_info_records_until_padding(bs, "FWIM")?;
    Ok(AccessMessage::FlashWithInfo(FlashWithInfoMessage {
        header,
        records,
        remaining_bits: bs.len(),
    }))
}

fn decode_send_burst_dtmf(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let num_digits = read(bs, 8, "NUM_DIGITS")? as usize;
    let dtmf_on_length = read(bs, 3, "DTMF_ON_LENGTH")? as u8;
    let dtmf_off_length = read(bs, 3, "DTMF_OFF_LENGTH")? as u8;
    let mut digits = Vec::with_capacity(num_digits);
    for idx in 0..num_digits {
        digits.push(read(bs, 4, &format!("DIGIT[{idx}]"))? as u8);
    }
    validate_send_burst_dtmf_fields(dtmf_on_length, dtmf_off_length, &digits)?;
    let con_ref_incl = read(bs, 1, "CON_REF_INCL")? != 0;
    let con_ref = if con_ref_incl {
        Some(read(bs, 8, "CON_REF")? as u8)
    } else {
        None
    };
    consume_zero_pdu_padding(bs, "BDTMFM")?;
    Ok(AccessMessage::SendBurstDtmf(SendBurstDtmfMessage {
        header,
        digits,
        dtmf_on_length,
        dtmf_off_length,
        con_ref,
        remaining_bits: bs.len(),
    }))
}

fn decode_status_message(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let mut records = read_info_records(bs, 1, "STM")?;
    consume_zero_pdu_padding(bs, "STM")?;
    Ok(AccessMessage::Status(StatusMessage {
        header,
        record: records.remove(0),
        remaining_bits: bs.len(),
    }))
}

fn decode_origination_continuation(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let digit_mode = read(bs, 1, "DIGIT_MODE")? != 0;
    let num_fields = read(bs, 8, "NUM_FIELDS")? as usize;
    let char_bits = if digit_mode { 8 } else { 4 };
    let mut digits = Vec::with_capacity(num_fields);
    for idx in 0..num_fields {
        digits.push(read(bs, char_bits, &format!("CHAR[{idx}]"))? as u8);
    }
    validate_origination_continuation_fields(digit_mode, &digits)?;
    let records = read_info_records_until_padding(bs, "ORCM")?;
    Ok(AccessMessage::OriginationContinuation(
        OriginationContinuationMessage {
            header,
            digit_mode,
            digits,
            records,
            remaining_bits: bs.len(),
        },
    ))
}

fn decode_handoff_completion(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let last_hdm_seq = read(bs, 2, "LAST_HDM_SEQ")? as u8;
    let mut pilot_pns = Vec::new();
    while bs.len() >= 9 {
        let idx = pilot_pns.len();
        pilot_pns.push(read(bs, 9, &format!("PILOT_PN[{idx}]"))? as u16);
    }
    validate_handoff_completion_fields(last_hdm_seq, &pilot_pns)?;
    consume_zero_pdu_padding(bs, "HOCM")?;
    Ok(AccessMessage::HandoffCompletion(HandoffCompletionMessage {
        header,
        last_hdm_seq,
        pilot_pns,
        remaining_bits: bs.len(),
    }))
}

fn decode_parameters_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let mut records = Vec::new();
    while bs.len() >= 26 {
        let parameter_id = read(bs, 16, &format!("PARAMETER_ID[{}]", records.len()))? as u16;
        let parameter_len = read(bs, 10, &format!("PARAMETER_LEN[{}]", records.len()))? as u16;
        let parameter = if parameter_len == 0x03ff {
            Bitstream::new()
        } else {
            let parameter_bits = parameter_len as usize + 1;
            if bs.len() < parameter_bits {
                return Err(format!(
                    "EOF reading PARAMETER[{}] ({} bits)",
                    records.len(),
                    parameter_bits
                ));
            }
            bs.drain(0..parameter_bits)
        };
        records.push(ParametersResponseRecord {
            parameter_id,
            parameter_len,
            parameter,
        });
    }
    validate_parameters_response_records(&records)?;
    consume_zero_pdu_padding(bs, "PRSM")?;
    Ok(AccessMessage::ParametersResponse(
        ParametersResponseMessage {
            header,
            records,
            remaining_bits: bs.len(),
        },
    ))
}

fn decode_service_option_control(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let con_ref = read(bs, 8, "CON_REF")? as u8;
    let service_option = read(bs, 16, "SERVICE_OPTION")? as u16;
    let reserved = read(bs, 7, "RESERVED")? as u8;
    if reserved != 0 {
        return Err(format!("SOCM RESERVED must be zero, got 0b{reserved:07b}"));
    }
    let control_record_len = read(bs, 8, "CTL_REC_LEN")? as usize;
    let control_record = read_octets(bs, control_record_len, "CTL_REC")?;
    validate_service_option_control_record(&control_record)?;
    consume_zero_pdu_padding(bs, "SOCM")?;
    Ok(AccessMessage::ServiceOptionControl(
        ServiceOptionControlMessage {
            header,
            con_ref,
            service_option,
            control_record,
            remaining_bits: bs.len(),
        },
    ))
}

fn read_aux_pilot_record(
    prefix: &str,
    bs: &mut Bitstream,
    field_name: &str,
) -> Result<Option<SupplementalChannelPilotRecord>, String> {
    let included = read(bs, 1, &format!("{field_name}.PILOT_REC_INCL"))? != 0;
    if !included {
        return Ok(None);
    }
    let pilot_rec_type = read(bs, 3, &format!("{field_name}.PILOT_REC_TYPE"))? as u8;
    let record_len = read(bs, 3, &format!("{field_name}.RECORD_LEN"))? as usize;
    let type_specific_fields = read_octets(
        bs,
        record_len,
        &format!("{field_name}.type_specific_fields"),
    )?;
    let record = SupplementalChannelPilotRecord {
        pilot_rec_type,
        type_specific_fields,
    };
    validate_aux_pilot_record(prefix, &record, field_name)?;
    Ok(Some(record))
}

fn read_scrm_pilot_record(
    bs: &mut Bitstream,
    field_name: &str,
) -> Result<Option<SupplementalChannelPilotRecord>, String> {
    read_aux_pilot_record("SCRM", bs, field_name)
}

fn decode_supplemental_channel_request(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let req_blob_len = read(bs, 4, "SIZE_OF_REQ_BLOB")? as usize;
    let req_blob = read_octets(bs, req_blob_len, "REQ_BLOB")?;
    let use_scrm_seq_num = read(bs, 1, "USE_SCRM_SEQ_NUM")? != 0;
    let scrm_seq_num = if use_scrm_seq_num {
        Some(read(bs, 4, "SCRM_SEQ_NUM")? as u8)
    } else {
        None
    };

    let measurements = if req_blob.is_empty() && scrm_seq_num.is_none() {
        None
    } else {
        let ref_pn = read(bs, 9, "REF_PN")? as u16;
        let pilot_strength = read(bs, 6, "PILOT_STRENGTH")? as u8;
        let num_act_pn = read(bs, 3, "NUM_ACT_PN")? as usize;
        let mut active_pilots = Vec::with_capacity(num_act_pn);
        for idx in 0..num_act_pn {
            active_pilots.push(SupplementalChannelPilotReport {
                pn_phase: read(bs, 15, &format!("ACT_PN_PHASE[{idx}]"))? as u16,
                pilot_strength: read(bs, 6, &format!("ACT_PILOT_STRENGTH[{idx}]"))? as u8,
                pilot_record: None,
            });
        }

        let neighbor_pilots = if req_blob.is_empty() {
            None
        } else {
            let num_nghbr_pn = read(bs, 3, "NUM_NGHBR_PN")? as usize;
            let mut pilots = Vec::with_capacity(num_nghbr_pn);
            for idx in 0..num_nghbr_pn {
                pilots.push(SupplementalChannelPilotReport {
                    pn_phase: read(bs, 15, &format!("NGHBR_PN_PHASE[{idx}]"))? as u16,
                    pilot_strength: read(bs, 6, &format!("NGHBR_PILOT_STRENGTH[{idx}]"))? as u8,
                    pilot_record: None,
                });
            }
            Some(pilots)
        };

        let ref_pilot_record = if req_blob.is_empty() {
            None
        } else {
            read_scrm_pilot_record(bs, "REF_PILOT_REC")?
        };

        for (idx, pilot) in active_pilots.iter_mut().enumerate() {
            pilot.pilot_record = read_scrm_pilot_record(bs, &format!("ACT_PN[{idx}]"))?;
        }
        let mut neighbor_pilots = neighbor_pilots;
        if let Some(pilots) = &mut neighbor_pilots {
            for (idx, pilot) in pilots.iter_mut().enumerate() {
                pilot.pilot_record = read_scrm_pilot_record(bs, &format!("NGHBR_PN[{idx}]"))?;
            }
        }

        Some(SupplementalChannelRequestMeasurements {
            ref_pn,
            pilot_strength,
            active_pilots,
            neighbor_pilots,
            ref_pilot_record,
        })
    };

    let msg = SupplementalChannelRequestMessage {
        header,
        req_blob,
        scrm_seq_num,
        measurements,
        remaining_bits: 0,
    };
    validate_supplemental_channel_request(&msg)?;
    consume_zero_pdu_padding(bs, "SCRM")?;
    Ok(AccessMessage::SupplementalChannelRequest(
        SupplementalChannelRequestMessage {
            remaining_bits: bs.len(),
            ..msg
        },
    ))
}

fn decode_candidate_freq_search_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let last_cfsrm_seq = read(bs, 2, "LAST_CFSRM_SEQ")? as u8;
    let total_off_time_fwd = read(bs, 6, "TOTAL_OFF_TIME_FWD")? as u8;
    let max_off_time_fwd = read(bs, 6, "MAX_OFF_TIME_FWD")? as u8;
    let total_off_time_rev = read(bs, 6, "TOTAL_OFF_TIME_REV")? as u8;
    let max_off_time_rev = read(bs, 6, "MAX_OFF_TIME_REV")? as u8;
    let pcg_off_times = read(bs, 1, "PCG_OFF_TIMES")? != 0;
    let align_timing_used = read(bs, 1, "ALIGN_TIMING_USED")? != 0;
    let max_num_visits = if align_timing_used {
        Some(read(bs, 5, "MAX_NUM_VISITS")? as u8)
    } else {
        None
    };
    let inter_visit_time = if max_num_visits.is_some_and(|visits| visits != 0) {
        Some(read(bs, 6, "INTER_VISIT_TIME")? as u8)
    } else {
        None
    };

    let msg = CandidateFreqSearchResponseMessage {
        header,
        last_cfsrm_seq,
        total_off_time_fwd,
        max_off_time_fwd,
        total_off_time_rev,
        max_off_time_rev,
        pcg_off_times,
        align_timing_used,
        max_num_visits,
        inter_visit_time,
        remaining_bits: 0,
    };
    validate_candidate_freq_search_response(&msg)?;
    consume_zero_pdu_padding(bs, "CFSRSM")?;
    Ok(AccessMessage::CandidateFreqSearchResponse(
        CandidateFreqSearchResponseMessage {
            remaining_bits: bs.len(),
            ..msg
        },
    ))
}

fn decode_candidate_freq_search_cdma_mode(
    bs: &mut Bitstream,
) -> Result<CandidateFreqSearchCdmaPilots, String> {
    let band_class = read(bs, 5, "BAND_CLASS")? as u8;
    let cdma_freq = read(bs, 11, "CDMA_FREQ")? as u16;
    let sf_total_rx_pwr = read(bs, 5, "SF_TOTAL_RX_PWR")? as u8;
    let cf_total_rx_pwr = read(bs, 5, "CF_TOTAL_RX_PWR")? as u8;
    let num_pilots = read(bs, 6, "NUM_PILOTS")? as usize;
    let mut pilots = Vec::with_capacity(num_pilots);
    for idx in 0..num_pilots {
        let pilot_pn_phase = read(bs, 15, &format!("PILOT_PN_PHASE[{idx}]"))? as u16;
        let pilot_strength = read(bs, 6, &format!("PILOT_STRENGTH[{idx}]"))? as u8;
        let reserved_1 = read(bs, 3, &format!("RESERVED_1[{idx}]"))? as u8;
        if reserved_1 != 0 {
            return Err(format!(
                "CFSRPM RESERVED_1[{idx}] must be zero, got 0b{reserved_1:03b}"
            ));
        }
        pilots.push(CandidateFreqSearchReportPilot {
            pilot_pn_phase,
            pilot_strength,
            pilot_record: None,
        });
    }
    for (idx, pilot) in pilots.iter_mut().enumerate() {
        pilot.pilot_record = read_aux_pilot_record("CFSRPM", bs, &format!("PILOT_REC[{idx}]"))?;
    }
    consume_zero_pdu_padding(bs, "CFSRPM mode-specific")?;
    let mode = CandidateFreqSearchCdmaPilots {
        band_class,
        cdma_freq,
        sf_total_rx_pwr,
        cf_total_rx_pwr,
        pilots,
    };
    validate_candidate_freq_search_cdma_mode(&mode)?;
    Ok(mode)
}

fn decode_candidate_freq_search_report(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let last_srch_msg = read(bs, 1, "LAST_SRCH_MSG")? != 0;
    let last_srch_msg_seq = read(bs, 2, "LAST_SRCH_MSG_SEQ")? as u8;
    let search_mode = read(bs, 4, "SEARCH_MODE")? as u8;
    let mode_specific_len = read(bs, 8, "MODE_SPECIFIC_LEN")? as usize;
    let mode_specific_bytes = read_octets(bs, mode_specific_len, "MODE_SPECIFIC")?;
    let mode_specific = match search_mode {
        0 => {
            let mut mode_bs = Bitstream::new_bytes(&mode_specific_bytes);
            CandidateFreqSearchReportModeSpecific::CdmaPilots(
                decode_candidate_freq_search_cdma_mode(&mut mode_bs)?,
            )
        }
        2 => CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(mode_specific_bytes),
        _ => {
            return Err(format!(
                "CFSRPM SEARCH_MODE 0b{search_mode:04b} is reserved"
            ));
        }
    };

    let msg = CandidateFreqSearchReportMessage {
        header,
        last_srch_msg,
        last_srch_msg_seq,
        search_mode,
        mode_specific,
        remaining_bits: 0,
    };
    validate_candidate_freq_search_report(&msg)?;
    consume_zero_pdu_padding(bs, "CFSRPM")?;
    Ok(AccessMessage::CandidateFreqSearchReport(
        CandidateFreqSearchReportMessage {
            remaining_bits: bs.len(),
            ..msg
        },
    ))
}

fn decode_periodic_psmm(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let ref_pn = read(bs, 9, "REF_PN")? as u16;
    let pilot_strength = read(bs, 6, "PILOT_STRENGTH")? as u8;
    let keep = read(bs, 1, "KEEP")? != 0;
    let sf_rx_pwr = read(bs, 5, "SF_RX_PWR")? as u8;
    let num_pilot = read(bs, 4, "NUM_PILOT")? as usize;
    let mut pilots = Vec::with_capacity(num_pilot);
    for idx in 0..num_pilot {
        pilots.push(PeriodicPsmmPilot {
            pilot_pn_phase: read(bs, 15, &format!("PILOT_PN_PHASE[{idx}]"))? as u16,
            pilot_strength: read(bs, 6, &format!("PILOT_STRENGTH[{idx}]"))? as u8,
            keep: read(bs, 1, &format!("KEEP[{idx}]"))? != 0,
            pilot_record: None,
        });
    }
    for (idx, pilot) in pilots.iter_mut().enumerate() {
        pilot.pilot_record = read_aux_pilot_record("PPSMM", bs, &format!("PILOT_REC[{idx}]"))?;
    }
    let setpt_incl = read(bs, 1, "SETPT_INCL")? != 0;
    let setpoints = if setpt_incl {
        let fch_incl = read(bs, 1, "FCH_INCL")? != 0;
        let fpc_fch_curr_setpt = if fch_incl {
            Some(read(bs, 8, "FPC_FCH_CURR_SETPT")? as u8)
        } else {
            None
        };
        let dcch_incl = read(bs, 1, "DCCH_INCL")? != 0;
        let fpc_dcch_curr_setpt = if dcch_incl {
            Some(read(bs, 8, "FPC_DCCH_CURR_SETPT")? as u8)
        } else {
            None
        };
        let num_sup = read(bs, 2, "NUM_SUP")? as usize;
        let mut sch_setpoints = Vec::with_capacity(num_sup);
        for idx in 0..num_sup {
            sch_setpoints.push(PeriodicPsmmSchSetpoint {
                sch_id: read(bs, 1, &format!("SCH_ID[{idx}]"))? as u8,
                fpc_sch_curr_setpt: read(bs, 8, &format!("FPC_SCH_CURR_SETPT[{idx}]"))? as u8,
            });
        }
        Some(PeriodicPsmmSetpoints {
            fpc_fch_curr_setpt,
            fpc_dcch_curr_setpt,
            sch_setpoints,
        })
    } else {
        None
    };
    let msg = PeriodicPsmmMessage {
        header,
        ref_pn,
        pilot_strength,
        keep,
        sf_rx_pwr,
        pilots,
        setpoints,
        remaining_bits: 0,
    };
    validate_periodic_psmm(&msg)?;
    consume_zero_pdu_padding(bs, "PPSMM")?;
    Ok(AccessMessage::PeriodicPsmm(PeriodicPsmmMessage {
        remaining_bits: bs.len(),
        ..msg
    }))
}

fn decode_outer_loop_report(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let fch_incl = read(bs, 1, "FCH_INCL")? != 0;
    let fpc_fch_curr_setpt = if fch_incl {
        Some(read(bs, 8, "FPC_FCH_CURR_SETPT")? as u8)
    } else {
        None
    };
    let dcch_incl = read(bs, 1, "DCCH_INCL")? != 0;
    let fpc_dcch_curr_setpt = if dcch_incl {
        Some(read(bs, 8, "FPC_DCCH_CURR_SETPT")? as u8)
    } else {
        None
    };
    let num_sup = read(bs, 2, "NUM_SUP")? as usize;
    let mut sch_setpoints = Vec::with_capacity(num_sup);
    for idx in 0..num_sup {
        sch_setpoints.push(PeriodicPsmmSchSetpoint {
            sch_id: read(bs, 1, &format!("SCH_ID[{idx}]"))? as u8,
            fpc_sch_curr_setpt: read(bs, 8, &format!("FPC_SCH_CURR_SETPT[{idx}]"))? as u8,
        });
    }
    let msg = OuterLoopReportMessage {
        header,
        fpc_fch_curr_setpt,
        fpc_dcch_curr_setpt,
        sch_setpoints,
        remaining_bits: 0,
    };
    validate_outer_loop_report(&msg)?;
    consume_zero_pdu_padding(bs, "OLRM")?;
    Ok(AccessMessage::OuterLoopReport(OuterLoopReportMessage {
        remaining_bits: bs.len(),
        ..msg
    }))
}

fn decode_resource_request(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let ch_ind_incl = read(bs, 1, "CH_IND_INCL")? != 0;
    let (ch_ind, ext_ch_ind) = if ch_ind_incl {
        let ch_ind = read(bs, 2, "CH_IND")? as u8;
        let ext_ch_ind = if ch_ind == 0 {
            let value = read(bs, 5, "EXT_CH_IND")? as u8;
            if !is_valid_origination_ext_ch_ind(value) {
                return Err(format!(
                    "RRM EXT_CH_IND value {value:#07b} is reserved or invalid"
                ));
            }
            Some(value)
        } else {
            None
        };
        (Some(ch_ind), ext_ch_ind)
    } else {
        (None, None)
    };

    let msg = ResourceRequestMessage {
        header,
        ch_ind,
        ext_ch_ind,
        remaining_bits: 0,
    };
    validate_resource_request(&msg)?;
    consume_zero_pdu_padding(bs, "RRM")?;
    Ok(AccessMessage::ResourceRequest(ResourceRequestMessage {
        remaining_bits: bs.len(),
        ..msg
    }))
}

fn decode_ext_release_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let rsc_mode_ind = read(bs, 1, "RSC_MODE_IND")? != 0;
    let (rsci, rsc_end_time_unit, rsc_end_time_value) = if rsc_mode_ind {
        let rsci = read(bs, 4, "RSCI")? as u8;
        let unit = read(bs, 2, "RSC_END_TIME_UNIT")? as u8;
        if unit == 0b11 {
            return Err("ERRM RSC_END_TIME_UNIT 0b11 is reserved".to_string());
        }
        let value = read(bs, 4, "RSC_END_TIME_VALUE")? as u8;
        (Some(rsci), Some(unit), Some(value))
    } else {
        (None, None, None)
    };

    let msg = ExtReleaseResponseMessage {
        header,
        rsc_mode_ind,
        rsci,
        rsc_end_time_unit,
        rsc_end_time_value,
        remaining_bits: 0,
    };
    validate_ext_release_response(&msg)?;
    consume_zero_pdu_padding(bs, "ERRM")?;
    Ok(AccessMessage::ExtReleaseResponse(
        ExtReleaseResponseMessage {
            remaining_bits: bs.len(),
            ..msg
        },
    ))
}

fn decode_status_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let qual_info_type = read(bs, 8, "QUAL_INFO_TYPE")? as u8;
    let qual_info_len = read(bs, 3, "QUAL_INFO_LEN")? as usize;
    let qual_info = read_octets(bs, qual_info_len, "QUAL_INFO")?;
    let mut records = Vec::new();
    while bs.len() >= 16 {
        let record_type = read(bs, 8, "RECORD_TYPE")? as u8;
        let record_len = read(bs, 8, "RECORD_LEN")? as usize;
        let data = read_octets(bs, record_len, "RECORD")?;
        records.push(AccessInfoRecord { record_type, data });
    }
    if records.is_empty() {
        return Err("Status Response requires at least one info record".to_string());
    }
    Ok(AccessMessage::StatusResponse(StatusResponseMessage {
        header,
        qual_info_type,
        qual_info,
        records,
        remaining_bits: bs.len(),
    }))
}

fn decode_extended_status_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let qual_info_type = read(bs, 8, "QUAL_INFO_TYPE")? as u8;
    let qual_info_len = read(bs, 3, "QUAL_INFO_LEN")? as usize;
    let qual_info = read_octets(bs, qual_info_len, "QUAL_INFO")?;
    let num_info_records = read(bs, 4, "NUM_INFO_RECORDS")? as u8;
    let records = read_info_records(bs, num_info_records as usize, "INFO_RECORD")?;
    Ok(AccessMessage::ExtStatusResponse(
        ExtendedStatusResponseMessage {
            header,
            qual_info_type,
            qual_info,
            num_info_records,
            records,
            remaining_bits: bs.len(),
        },
    ))
}

fn decode_device_information(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let wll_device_type = read(bs, 3, "WLL_DEVICE_TYPE")? as u8;
    let num_info_records = read(bs, 5, "NUM_INFO_RECORDS")? as u8;
    let records = read_info_records(bs, num_info_records as usize, "INFO_RECORD")?;
    Ok(AccessMessage::DeviceInformation(DeviceInformationMessage {
        header,
        wll_device_type,
        num_info_records,
        records,
        remaining_bits: bs.len(),
    }))
}

fn decode_security_mode_request(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let ui_enc_incl = read(bs, 1, "UI_ENC_INCL")? != 0;
    let ui_encrypt_sup = if ui_enc_incl {
        let ui_sup = read(bs, 8, "UI_ENCRYPT_SUP")? as u8;
        validate_ui_encrypt_sup(ui_sup)?;
        Some(ui_sup)
    } else {
        None
    };

    let sig_enc_incl = read(bs, 1, "SIG_ENC_INCL")? != 0;
    let (sig_encrypt_sup, c_sig_encrypt_req) = if sig_enc_incl {
        let sig_sup = read(bs, 8, "SIG_ENCRYPT_SUP")? as u8;
        validate_sig_encrypt_sup(sig_sup)?;
        (Some(sig_sup), Some(read(bs, 1, "C_SIG_ENCRYPT_REQ")? != 0))
    } else {
        (None, None)
    };

    let new_sseq_h_incl = read(bs, 1, "NEW_SSEQ_H_INCL")? != 0;
    let (new_sseq_h, new_sseq_h_sig) = if new_sseq_h_incl {
        (
            Some(read(bs, 24, "NEW_SSEQ_H")? as u32),
            Some(read(bs, 8, "NEW_SSEQ_H_SIG")? as u8),
        )
    } else {
        (None, None)
    };

    let msg_int_info_incl = read(bs, 1, "MSG_INT_INFO_INCL")? != 0;
    let mut sig_integrity_sup_incl = None;
    let (sig_integrity_sup, sig_integrity_req) = if msg_int_info_incl {
        let incl = read(bs, 1, "SIG_INTEGRITY_SUP_INCL")? != 0;
        sig_integrity_sup_incl = Some(incl);
        if incl {
            let sig_sup = read(bs, 8, "SIG_INTEGRITY_SUP")? as u8;
            let sig_req = read(bs, 3, "SIG_INTEGRITY_REQ")? as u8;
            validate_sig_integrity_fields(sig_sup, sig_req)?;
            (Some(sig_sup), Some(sig_req))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    Ok(AccessMessage::SecurityModeRequest(
        SecurityModeRequestMessage {
            header,
            ui_encrypt_sup,
            ui_encrypt_records: Vec::new(),
            sig_encrypt_sup,
            c_sig_encrypt_req,
            d_sig_encrypt_req: None,
            new_sseq_h,
            new_sseq_h_sig,
            msg_int_info_incl: Some(msg_int_info_incl),
            sig_integrity_sup_incl,
            sig_integrity_sup,
            sig_integrity_req,
            remaining_bits: bs.len(),
        },
    ))
}

fn decode_rdsch_security_mode_request(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    let ui_enc_incl = read(bs, 1, "UI_ENC_INCL")? != 0;
    let mut ui_encrypt_records = Vec::new();
    let ui_encrypt_sup = if ui_enc_incl {
        let ui_sup = read(bs, 8, "UI_ENCRYPT_SUP")? as u8;
        validate_ui_encrypt_sup(ui_sup)?;
        let ui_encrypt_sup = Some(ui_sup);
        let num_recs = read(bs, 3, "NUM_RECS")? as usize;
        let count = num_recs + 1;
        ui_encrypt_records.reserve(count);
        for idx in 0..count {
            ui_encrypt_records.push(SecurityModeUiEncryptRecord {
                con_ref: read(bs, 8, &format!("CON_REF[{idx}]"))? as u8,
                ui_encrypt_req: read(bs, 1, &format!("UI_ENCRYPT_REQ[{idx}]"))? != 0,
            });
        }
        ui_encrypt_sup
    } else {
        None
    };

    let sig_enc_incl = read(bs, 1, "SIG_ENC_INCL")? != 0;
    let (sig_encrypt_sup, d_sig_encrypt_req) = if sig_enc_incl {
        let sig_sup = read(bs, 8, "SIG_ENCRYPT_SUP")? as u8;
        validate_sig_encrypt_sup(sig_sup)?;
        (Some(sig_sup), Some(read(bs, 1, "D_SIG_ENCRYPT_REQ")? != 0))
    } else {
        (None, None)
    };

    let new_sseq_h_incl = read(bs, 1, "NEW_SSEQ_H_INCL")? != 0;
    let (new_sseq_h, new_sseq_h_sig) = if new_sseq_h_incl {
        (
            Some(read(bs, 24, "NEW_SSEQ_H")? as u32),
            Some(read(bs, 8, "NEW_SSEQ_H_SIG")? as u8),
        )
    } else {
        (None, None)
    };

    let msg_int_info_incl = read(bs, 1, "MSG_INT_INFO_INCL")? != 0;
    let mut sig_integrity_sup_incl = None;
    let (sig_integrity_sup, sig_integrity_req) = if msg_int_info_incl {
        let incl = read(bs, 1, "SIG_INTEGRITY_SUP_INCL")? != 0;
        sig_integrity_sup_incl = Some(incl);
        if incl {
            let sig_sup = read(bs, 8, "SIG_INTEGRITY_SUP")? as u8;
            let sig_req = read(bs, 3, "SIG_INTEGRITY_REQ")? as u8;
            validate_sig_integrity_fields(sig_sup, sig_req)?;
            (Some(sig_sup), Some(sig_req))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    Ok(AccessMessage::SecurityModeRequest(
        SecurityModeRequestMessage {
            header,
            ui_encrypt_sup,
            ui_encrypt_records,
            sig_encrypt_sup,
            c_sig_encrypt_req: None,
            d_sig_encrypt_req,
            new_sseq_h,
            new_sseq_h_sig,
            msg_int_info_incl: Some(msg_int_info_incl),
            sig_integrity_sup_incl,
            sig_integrity_sup,
            sig_integrity_req,
            remaining_bits: bs.len(),
        },
    ))
}

fn decode_reconnect(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
    ctx: AccessDecodeContext,
) -> Result<AccessMessage, String> {
    let orig_ind = read(bs, 1, "ORIG_IND")? != 0;
    let sync_id_incl = read(bs, 1, "SYNC_ID_INCL")? != 0;
    let (sync_id_len, sync_id) = if sync_id_incl {
        let len = read(bs, 4, "SYNC_ID_LEN")? as u8;
        (Some(len), read_octets(bs, len as usize, "SYNC_ID")?)
    } else {
        (None, Vec::new())
    };
    let service_option = if sync_id_incl {
        None
    } else {
        Some(read(bs, 16, "SERVICE_OPTION")? as u16)
    };
    let sr_id = if orig_ind {
        Some(read(bs, 3, "SR_ID")? as u8)
    } else {
        None
    };

    let p_rev_in_use = ctx.p_rev_in_use.unwrap_or(6);
    let mut add_serv_instance_incl = None;
    let mut add_sr_ids = Vec::new();
    if orig_ind && p_rev_in_use >= 11 && sync_id_incl && sr_id != Some(0b111) {
        let incl = read(bs, 1, "ADD_SERV_INSTANCE_INCL")? != 0;
        add_serv_instance_incl = Some(incl);
        if incl {
            let count = read(bs, 3, "NUM_ADD_SERV_INSTANCE")? as u8;
            add_sr_ids.reserve(count as usize);
            for idx in 0..count {
                add_sr_ids.push(read(bs, 3, &format!("ADD_SR_ID[{idx}]"))? as u8);
            }
        }
    }

    let mut sdb_incl = None;
    let mut sdb_fields = Vec::new();
    if p_rev_in_use >= 11 {
        let incl = read(bs, 1, "SDB_INCL")? != 0;
        sdb_incl = Some(incl);
        if incl {
            let count = read(bs, 8, "NUM_FIELDS")? as usize;
            sdb_fields = read_octets(bs, count, "CHAR")?;
        }
    }

    Ok(AccessMessage::Reconnect(ReconnectMessage {
        header,
        orig_ind,
        sync_id_incl,
        sync_id_len,
        sync_id,
        service_option,
        sr_id,
        add_serv_instance_incl,
        add_sr_ids,
        sdb_incl,
        sdb_fields,
        remaining_bits: bs.len(),
    }))
}

fn decode_radio_environment(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
) -> Result<AccessMessage, String> {
    Ok(AccessMessage::RadioEnvironment(RadioEnvironmentMessage {
        header,
        mode_disabled: read(bs, 1, "MODE_DISABLED")? != 0,
        tkz_mode_ind: read(bs, 1, "TKZ_MODE_IND")? != 0,
        remaining_bits: bs.len(),
    }))
}

fn decode_page_response(
    header: AccessMessageHeader,
    bs: &mut Bitstream,
    ctx: AccessDecodeContext,
) -> Result<AccessMessage, String> {
    let mob_term = read(bs, 1, "MOB_TERM")? == 1;
    let slot_cycle_index = read(bs, 3, "SLOT_CYCLE_INDEX")? as u8;
    let mob_p_rev = read(bs, 8, "MOB_P_REV")? as u8;
    let scm = read(bs, 8, "SCM")? as u8;
    let request_mode = read(bs, 3, "REQUEST_MODE")? as u8;
    let service_option = read(bs, 16, "SERVICE_OPTION")? as u16;
    let pm = read(bs, 1, "PM")? == 1;
    let nar_an_cap = read(bs, 1, "NAR_AN_CAP")? == 1;
    let p_rev_in_use = mob_p_rev;
    let encryption_supported = if p_rev_in_use < 7 {
        match ctx.auth_mode {
            Some(0) => None,
            Some(_) => Some(read(bs, 4, "ENCRYPTION_SUPPORTED")? as u8),
            None => {
                return Err("Page Response decode requires AUTH_MODE context".to_string());
            }
        }
    } else {
        None
    };
    let num_alt_so = read(bs, 3, "NUM_ALT_SO")? as u8;
    let mut alt_service_options = Vec::with_capacity(num_alt_so as usize);
    for idx in 0..num_alt_so {
        alt_service_options.push(read(bs, 16, &format!("ALT_SO[{idx}]"))? as u16);
    }
    let mut uzid_incl = None;
    let mut uzid = None;
    let mut ch_ind = None;
    let mut otd_supported = None;
    let mut qpch_supported = None;
    let mut enhanced_rc = None;
    let mut for_rc_pref = None;
    let mut rev_rc_pref = None;
    let mut fch_supported = None;
    let mut fch_capability = None;
    let mut dcch_supported = None;
    let mut dcch_capability = None;
    let mut rev_fch_gating_req = None;
    let mut sts_supported = None;
    let mut cch_3x_supported = None;
    let mut wll_incl = None;
    let mut wll_device_type = None;
    let mut hook_status = None;
    let mut enc_info_incl = None;
    let mut sig_encrypt_sup = None;
    let mut d_sig_encrypt_req = None;
    let mut c_sig_encrypt_req = None;
    let mut new_sseq_h = None;
    let mut new_sseq_h_sig = None;
    let mut ui_encrypt_req = None;
    let mut ui_encrypt_sup = None;
    let mut sync_id_incl = None;
    let mut sync_id_len = None;
    let mut sync_id = None;
    let mut so_bitmap_ind = None;
    let mut so_group_num = None;
    let mut so_bitmap = None;
    let mut alt_band_class_sup = None;
    let mut msg_int_info_incl = None;
    let mut sig_integrity_sup_incl = None;
    let mut sig_integrity_sup = None;
    let mut sig_integrity_req = None;
    let mut new_key_id = None;
    let mut new_sseq_h_incl = None;
    let mut for_pdch_supported = None;
    let mut for_pdch_capability = None;
    let mut ext_ch_ind = None;
    let mut sign_slot_cycle_index = None;
    let mut bcmc_incl = None;
    let mut bcmc_pref_incl = None;
    let mut bcmc = None;
    let mut rev_pdch_supported = None;
    let mut rev_pdch_capability = None;
    let mut band_sub_rep_incl = None;
    let mut num_band_subclass = None;
    let mut band_subclass_sup = None;

    if p_rev_in_use >= 6 {
        let uzid_incl_value = read(bs, 1, "UZID_INCL")? == 1;
        uzid_incl = Some(uzid_incl_value);
        if uzid_incl_value {
            uzid = Some(read(bs, 16, "UZID")? as u16);
        }
        ch_ind = Some(read(bs, 2, "CH_IND")? as u8);
        otd_supported = Some(read(bs, 1, "OTD_SUPPORTED")? == 1);
        qpch_supported = Some(read(bs, 1, "QPCH_SUPPORTED")? == 1);
        enhanced_rc = Some(read(bs, 1, "ENHANCED_RC")? == 1);
        for_rc_pref = Some(read(bs, 5, "FOR_RC_PREF")? as u8);
        rev_rc_pref = Some(read(bs, 5, "REV_RC_PREF")? as u8);

        let fch_supported_value = read(bs, 1, "FCH_SUPPORTED")? == 1;
        fch_supported = Some(fch_supported_value);
        if fch_supported_value {
            fch_capability = Some(decode_fch_type_specific_fields(bs)?);
        }

        let dcch_supported_value = read(bs, 1, "DCCH_SUPPORTED")? == 1;
        dcch_supported = Some(dcch_supported_value);
        if dcch_supported_value {
            dcch_capability = Some(decode_dcch_type_specific_fields(bs)?);
        }

        rev_fch_gating_req = Some(read(bs, 1, "REV_FCH_GATING_REQ")? == 1);
    }

    if p_rev_in_use >= 7 {
        sts_supported = Some(read(bs, 1, "STS_SUPPORTED")? == 1);
        cch_3x_supported = Some(read(bs, 1, "3X_CCH_SUPPORTED")? == 1);
        let wll_incl_value = read(bs, 1, "WLL_INCL")? == 1;
        wll_incl = Some(wll_incl_value);
        if wll_incl_value {
            wll_device_type = Some(read(bs, 3, "WLL_DEVICE_TYPE")? as u8);
            hook_status = Some(read(bs, 4, "HOOK_STATUS")? as u8);
        }

        let enc_info_incl_value = read(bs, 1, "ENC_INFO_INCL")? == 1;
        enc_info_incl = Some(enc_info_incl_value);
        if enc_info_incl_value {
            let sig_sup = read(bs, 8, "SIG_ENCRYPT_SUP")? as u8;
            sig_encrypt_sup = Some(sig_sup);
            d_sig_encrypt_req = Some(read(bs, 1, "D_SIG_ENCRYPT_REQ")? as u8);
            c_sig_encrypt_req = Some(read(bs, 1, "C_SIG_ENCRYPT_REQ")? as u8);
            // SIG_ENCRYPT_SUP layout: CMEA(1)|ECMEA(1)|REA(1)|RESERVED(5)
            let ecmea = (sig_sup >> 6) & 1;
            let rea = (sig_sup >> 5) & 1;
            if ecmea == 1 || rea == 1 {
                new_sseq_h = Some(read(bs, 24, "NEW_SSEQ_H")? as u32);
                new_sseq_h_sig = Some(read(bs, 8, "NEW_SSEQ_H_SIG")? as u32);
            }
            ui_encrypt_req = Some(read(bs, 1, "UI_ENCRYPT_REQ")? as u8);
            ui_encrypt_sup = Some(read(bs, 8, "UI_ENCRYPT_SUP")? as u8);
        }

        let sync_id_incl_value = read(bs, 1, "SYNC_ID_INCL")? == 1;
        sync_id_incl = Some(sync_id_incl_value);
        if sync_id_incl_value {
            let len = read(bs, 4, "SYNC_ID_LEN")? as u8;
            sync_id_len = Some(len);
            if len > 0 {
                sync_id = Some(read(bs, len as usize * 8, "SYNC_ID")? as u32);
            }
        }

        let so_bitmap_ind_val = read(bs, 2, "SO_BITMAP_IND")? as u8;
        so_bitmap_ind = Some(so_bitmap_ind_val);
        if so_bitmap_ind_val > 0 {
            so_group_num = Some(read(bs, 5, "SO_GROUP_NUM")? as u8);
            let bitmap_bits = 1usize << (1 + so_bitmap_ind_val as usize);
            so_bitmap = Some(read(bs, bitmap_bits, "SO_BITMAP")? as u16);
        }
    }

    if p_rev_in_use >= 8 {
        alt_band_class_sup = Some(read(bs, 1, "ALT_BAND_CLASS_SUP")? == 1);
    }

    if p_rev_in_use >= 9 {
        let msg_int_info_incl_value = read(bs, 1, "MSG_INT_INFO_INCL")? == 1;
        msg_int_info_incl = Some(msg_int_info_incl_value);
        if msg_int_info_incl_value {
            let sig_integrity_sup_incl_value = read(bs, 1, "SIG_INTEGRITY_SUP_INCL")? == 1;
            sig_integrity_sup_incl = Some(sig_integrity_sup_incl_value);
            if sig_integrity_sup_incl_value {
                let sig_sup = read(bs, 8, "SIG_INTEGRITY_SUP")? as u8;
                let sig_req = read(bs, 3, "SIG_INTEGRITY_REQ")? as u8;
                validate_sig_integrity_fields(sig_sup, sig_req)?;
                sig_integrity_sup = Some(sig_sup);
                sig_integrity_req = Some(sig_req);
            }
            new_key_id = Some(read(bs, 2, "NEW_KEY_ID")? as u8);
            let new_sseq_h_incl_value = read(bs, 1, "NEW_SSEQ_H_INCL")? == 1;
            new_sseq_h_incl = Some(new_sseq_h_incl_value);
            if new_sseq_h_incl_value {
                new_sseq_h = Some(read(bs, 24, "NEW_SSEQ_H")? as u32);
                new_sseq_h_sig = Some(read(bs, 8, "NEW_SSEQ_H_SIG")? as u32);
            }
        }

        for_pdch_supported = Some(read(bs, 1, "FOR_PDCH_SUPPORTED")? == 1);
        if for_pdch_supported == Some(true) {
            for_pdch_capability = Some(decode_for_pdch_type_specific_fields(bs)?);
        }
        if ch_ind == Some(0) {
            let value = read(bs, 5, "EXT_CH_IND")? as u8;
            if !is_valid_origination_ext_ch_ind(value) {
                return Err(format!(
                    "EXT_CH_IND value {value:#07b} is reserved or invalid"
                ));
            }
            ext_ch_ind = Some(value);
        }
    }

    if p_rev_in_use >= 11 {
        if slot_cycle_index != 0 {
            sign_slot_cycle_index = Some(read(bs, 1, "SIGN_SLOT_CYCLE_INDEX")? == 1);
        }

        let bcmc_incl_value = read(bs, 1, "BCMC_INCL")? == 1;
        bcmc_incl = Some(bcmc_incl_value);
        if bcmc_incl_value {
            let bcmc_pref_incl_value = read(bs, 1, "BCMC_PREF_INCL")? == 1;
            bcmc_pref_incl = Some(bcmc_pref_incl_value);
            bcmc = Some(decode_page_response_bcmc_fields(bs, bcmc_pref_incl_value)?);
        }

        if for_pdch_supported == Some(true) {
            let rev_pdch_supported_value = read(bs, 1, "REV_PDCH_SUPPORTED")? == 1;
            rev_pdch_supported = Some(rev_pdch_supported_value);
            if rev_pdch_supported_value {
                rev_pdch_capability = Some(decode_rev_pdch_type_specific_fields(bs)?);
            }
        }

        let band_sub_rep_incl_value = read(bs, 1, "BAND_SUB_REP_INCL")? == 1;
        band_sub_rep_incl = Some(band_sub_rep_incl_value);
        if band_sub_rep_incl_value {
            let n = read(bs, 4, "NUM_BAND_SUBCLASS")? as u8;
            num_band_subclass = Some(n);
            let mut subs = Vec::with_capacity(n as usize);
            for i in 0..n {
                subs.push(read(bs, 1, &format!("BAND_SUBCLASS_SUP[{i}]"))? as u8);
            }
            band_subclass_sup = Some(subs);
        }
    }

    Ok(AccessMessage::PageResponse(PageResponseMessage {
        header,
        mob_term,
        slot_cycle_index,
        mob_p_rev,
        scm,
        request_mode,
        service_option,
        pm,
        nar_an_cap,
        encryption_supported,
        num_alt_so,
        alt_service_options,
        uzid_incl,
        uzid,
        ch_ind,
        otd_supported,
        qpch_supported,
        enhanced_rc,
        for_rc_pref,
        rev_rc_pref,
        fch_supported,
        fch_capability,
        dcch_supported,
        dcch_capability,
        rev_fch_gating_req,
        sts_supported,
        cch_3x_supported,
        wll_incl,
        wll_device_type,
        hook_status,
        enc_info_incl,
        sig_encrypt_sup,
        d_sig_encrypt_req,
        c_sig_encrypt_req,
        new_sseq_h,
        new_sseq_h_sig,
        ui_encrypt_req,
        ui_encrypt_sup,
        sync_id_incl,
        sync_id_len,
        sync_id,
        so_bitmap_ind,
        so_group_num,
        so_bitmap,
        alt_band_class_sup,
        msg_int_info_incl,
        sig_integrity_sup_incl,
        sig_integrity_sup,
        sig_integrity_req,
        new_key_id,
        new_sseq_h_incl,
        for_pdch_supported,
        for_pdch_capability,
        ext_ch_ind,
        sign_slot_cycle_index,
        bcmc_incl,
        bcmc_pref_incl,
        bcmc,
        rev_pdch_supported,
        rev_pdch_capability,
        band_sub_rep_incl,
        num_band_subclass,
        band_subclass_sup,
        remaining_bits: bs.len(),
    }))
}

#[cfg(test)]
mod tests {
    use crate::bits::Bitstream;
    use crate::lac::message_types::{MessageId, WireChannel};

    use super::{
        AccessDecodeContext, AccessInfoRecord, AccessMessage, AccessMessageHeader,
        AuthChallengeResponseMessage, AuthResponseMessage, AuthResyncMessage,
        CandidateFreqSearchCdmaPilots, CandidateFreqSearchReportMessage,
        CandidateFreqSearchReportModeSpecific, CandidateFreqSearchReportPilot,
        CandidateFreqSearchResponseMessage, DataBurstMessage, DeviceInformationMessage,
        ExtReleaseResponseMessage, FdschMessage, FdschPdu, FlashWithInfoMessage,
        ForPdchTypeSpecificFields, GeneralExtensionMessage, HandoffCompletionMessage,
        NoFieldAccessMessage, OrderMessage, OriginationAdditionalServiceInstance,
        OriginationContinuationMessage, OriginationMessage, OuterLoopReportMessage,
        ParametersResponseMessage, ParametersResponseRecord, PeriodicPsmmMessage,
        PeriodicPsmmPilot, PeriodicPsmmSchSetpoint, PeriodicPsmmSetpoints, PilotReport,
        PilotStrengthMeasurementMessage, PowerMeasurementReportMessage, RdschPdu,
        ReducedSlotCycleOrderDetail, ResourceRequestMessage, RevPdchTypeSpecificFields,
        ReverseOrderDetail, SecurityModeRequestMessage, SecurityModeUiEncryptRecord,
        SendBurstDtmfMessage, ServiceConfigRecord, ServiceConnectCompletionMessage,
        ServiceOptionControlMessage, ServiceRequestMessage, ServiceResponseMessage, StatusMessage,
        StatusResponseMessage, SupplementalChannelPilotRecord, SupplementalChannelPilotReport,
        SupplementalChannelRequestMeasurements, SupplementalChannelRequestMessage,
    };

    fn rcsch_wire(id: MessageId) -> u8 {
        match id {
            MessageId::Registration => 0x01,
            MessageId::Order => 0x02,
            MessageId::DataBurst => 0x03,
            MessageId::Origination => 0x04,
            MessageId::PageResponse => 0x05,
            MessageId::AuthChallengeResponse => 0x06,
            MessageId::StatusResponse => 0x07,
            MessageId::TmsiAssignmentCompletion => 0x08,
            MessageId::PacaCancel => 0x09,
            MessageId::ExtStatusResponse => 0x0A,
            MessageId::DeviceInformation => 0x0D,
            MessageId::SecurityModeRequest => 0x0E,
            MessageId::AuthResponse => 0x15,
            MessageId::AuthResync => 0x16,
            MessageId::Reconnect => 0x17,
            MessageId::RadioEnvironment => 0x18,
            MessageId::CallRecoveryRequest => 0x19,
            MessageId::GeneralExtension => 0x3F,
            _ => panic!("no reverse-common literal for {id:?}"),
        }
    }

    fn rdsch_wire(id: MessageId) -> u8 {
        id.wire_type(WireChannel::ReverseDedicated).unwrap()
    }

    fn rdsch_pdu(id: MessageId, body: Bitstream) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(rdsch_wire(id), 8);
        bits.write_u8(0, 3); // ACK_SEQ
        bits.write_u8(0, 3); // MSG_SEQ
        bits.write_u8(0, 1); // ACK_REQ
        bits.write_u8(0, 2); // ENCRYPTION
        bits.extend(&body);
        bits
    }

    fn rdsch_smrm_body(ui_sup: u8, sig_sup: u8, sig_int_sup: u8, sig_int_req: u8) -> Bitstream {
        let mut body = Bitstream::new();
        body.write_u8(1, 1); // UI_ENC_INCL
        body.write_u8(ui_sup, 8);
        body.write_u8(0, 3); // NUM_RECS: one record
        body.write_u8(0x12, 8); // CON_REF[0]
        body.write_u8(1, 1); // UI_ENCRYPT_REQ[0]
        body.write_u8(1, 1); // SIG_ENC_INCL
        body.write_u8(sig_sup, 8);
        body.write_u8(1, 1); // D_SIG_ENCRYPT_REQ
        body.write_u8(0, 1); // NEW_SSEQ_H_INCL
        body.write_u8(1, 1); // MSG_INT_INFO_INCL
        body.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
        body.write_u8(sig_int_sup, 8);
        body.write_u8(sig_int_req, 3);
        body
    }

    fn rdsch_aurspm_body(sig_int_sup: u8, sig_int_req: u8) -> Bitstream {
        let mut body = Bitstream::new();
        for byte in 0u8..16 {
            body.write_u8(byte, 8);
        }
        body.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
        body.write_u8(sig_int_sup, 8);
        body.write_u8(sig_int_req, 3);
        body.write_u8(0b10, 2); // NEW_KEY_ID
        body.write_u32(0x654321, 24); // NEW_SSEQ_H
        body
    }

    fn fdsch_pdu(id: MessageId, body: Bitstream) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(id.wire_type(WireChannel::ForwardDedicated).unwrap(), 8);
        bits.write_u8(0, 3); // ACK_SEQ
        bits.write_u8(0, 3); // MSG_SEQ
        bits.write_u8(0, 1); // ACK_REQ
        bits.write_u8(0, 2); // ENCRYPTION
        bits.extend(&body);
        bits
    }

    fn write_minimal_origination_p_rev7_tail(
        bits: &mut Bitstream,
        mob_p_rev: u8,
        ch_ind: u8,
        enc_info: Option<(u8, u8)>,
    ) {
        bits.write_u8(rcsch_wire(MessageId::Origination), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(mob_p_rev, 8); // MOB_P_REV
        bits.write_u8(0x2a, 8); // SCM
        bits.write_u8(0b001, 3); // REQUEST_MODE
        bits.write_u8(0, 1); // SPECIAL_SERVICE
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // DIGIT_MODE
        if mob_p_rev >= 11 {
            bits.write_u8(0, 3); // NUMBER_TYPE
        }
        bits.write_u8(0, 1); // MORE_FIELDS
        bits.write_u8(0, 8); // NUM_FIELDS
        bits.write_u8(0, 1); // NAR_AN_CAP
        bits.write_u8(0, 1); // PACA_REORIG
        bits.write_u8(0, 4); // RETURN_CAUSE
        bits.write_u8(0, 1); // MORE_RECORDS
        bits.write_u8(1, 1); // PACA_SUPPORTED
        bits.write_u8(0, 3); // NUM_ALT_SO
        bits.write_u8(0, 1); // DRS
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(ch_ind, 2); // CH_IND
        bits.write_u8(0, 3); // SR_ID
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(0, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(1, 5); // FOR_RC_PREF
        bits.write_u8(1, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // GEO_LOC_INCL
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        bits.write_u8(0, 1); // ORIG_REASON
        bits.write_u8(0, 2); // ORIG_COUNT
        bits.write_u8(0, 1); // STS_SUPPORTED
        bits.write_u8(0, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(0, 1); // WLL_INCL
        bits.write_u8(0, 1); // GLOBAL_EMERGENCY_CALL
        bits.write_u8(0, 1); // QOS_PARMS_INCL
        if let Some((sig_encrypt_sup, ui_encrypt_sup)) = enc_info {
            bits.write_u8(1, 1); // ENC_INFO_INCL
            bits.write_u8(sig_encrypt_sup, 8); // SIG_ENCRYPT_SUP
            bits.write_u8(0, 1); // D_SIG_ENCRYPT_REQ
            bits.write_u8(0, 1); // C_SIG_ENCRYPT_REQ
            if sig_encrypt_sup & 0b0110_0000 != 0 {
                bits.write_u32(0x123456, 24); // NEW_SSEQ_H
                bits.write_u8(0x78, 8); // NEW_SSEQ_H_SIG
            }
            bits.write_u8(0, 1); // UI_ENCRYPT_REQ
            bits.write_u8(ui_encrypt_sup, 8); // UI_ENCRYPT_SUP
        } else {
            bits.write_u8(0, 1); // ENC_INFO_INCL
        }
        bits.write_u8(0, 1); // SYNC_ID_INCL
        bits.write_u8(0, 1); // PREV_SID_INCL
        bits.write_u8(0, 1); // PREV_NID_INCL
        bits.write_u8(0, 1); // PREV_PZID_INCL
        bits.write_u8(0, 2); // SO_BITMAP_IND
        if mob_p_rev >= 8 {
            bits.write_u8(0, 1); // SDB_DESIRED_ONLY
            bits.write_u8(0, 1); // ALT_BAND_CLASS_SUP
        }
    }

    fn minimal_service_config() -> ServiceConfigRecord {
        ServiceConfigRecord {
            for_mux_option: 0x0001,
            rev_mux_option: 0x0001,
            for_rates: 0xff,
            rev_rates: 0xff,
            connection_records: Vec::new(),
            fch_cc_incl: false,
            fch_frame_size: None,
            for_fch_rc: None,
            rev_fch_rc: None,
            dcch_cc_incl: false,
            for_sch_cc_incl: false,
            rev_sch_cc_incl: false,
        }
    }

    fn service_request_propose_body(record_type: u8, record_len: u8, raw: &[u8]) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(5, 3); // SERV_REQ_SEQ
        bits.write_u8(0b0010, 4); // propose
        bits.write_u8(record_type, 8);
        bits.write_u8(record_len, 8);
        for byte in raw {
            bits.write_u8(*byte, 8);
        }
        bits
    }

    fn service_response_counter_propose_body(
        record_type: u8,
        record_len: u8,
        raw: &[u8],
    ) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(6, 3); // SERV_REQ_SEQ
        bits.write_u8(0b0010, 4); // counter-propose
        bits.write_u8(record_type, 8);
        bits.write_u8(record_len, 8);
        for byte in raw {
            bits.write_u8(*byte, 8);
        }
        bits
    }

    fn assert_reencodes(input: &Bitstream, msg: &AccessMessage, ctx: AccessDecodeContext) {
        let encoded = msg
            .to_reverse_common_pdu_with_context(ctx)
            .expect("encode reverse common pdu");
        assert_eq!(input.bits(), encoded.bits());
    }

    #[test]
    fn test_access_message_encoder_rdsch_bodies_roundtrip() {
        let aucrm = AccessMessage::AuthChallengeResponse(AuthChallengeResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::AuthChallengeResponse,
            },
            authu: 0x2aaaa,
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::AuthChallengeResponse,
            aucrm.to_sdu().expect("encode aucrm"),
        ))
        .expect("decode encoded aucrm");
        let AccessMessage::AuthChallengeResponse(decoded) = pdu.l3 else {
            panic!("expected AUCRM");
        };
        assert_eq!(0x2aaaa, decoded.authu);
        assert_eq!(0, decoded.remaining_bits);

        let fwim = AccessMessage::FlashWithInfo(FlashWithInfoMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::FlashWithInfo,
            },
            records: vec![
                AccessInfoRecord {
                    record_type: 0x01,
                    data: vec![0x12, 0x34],
                },
                AccessInfoRecord {
                    record_type: 0x04,
                    data: vec![0xab],
                },
            ],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::FlashWithInfo,
            fwim.to_sdu().expect("encode fwim"),
        ))
        .expect("decode encoded fwim");
        let AccessMessage::FlashWithInfo(decoded) = pdu.l3 else {
            panic!("expected FWIM");
        };
        assert_eq!(2, decoded.records.len());
        assert_eq!(0x01, decoded.records[0].record_type);
        assert_eq!(vec![0x12, 0x34], decoded.records[0].data);
        assert_eq!(0x04, decoded.records[1].record_type);
        assert_eq!(vec![0xab], decoded.records[1].data);
        assert_eq!(0, decoded.remaining_bits);

        let bdtmfm = AccessMessage::SendBurstDtmf(SendBurstDtmfMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::SendBurstDtmf,
            },
            digits: vec![5, 5, 5, 0x0a, 0x0b, 0x0c],
            dtmf_on_length: 0b010,
            dtmf_off_length: 0b011,
            con_ref: Some(0x42),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::SendBurstDtmf,
            bdtmfm.to_sdu().expect("encode bdtmfm"),
        ))
        .expect("decode encoded bdtmfm");
        let AccessMessage::SendBurstDtmf(decoded) = pdu.l3 else {
            panic!("expected BDTMFM");
        };
        assert_eq!(vec![5, 5, 5, 0x0a, 0x0b, 0x0c], decoded.digits);
        assert_eq!(0b010, decoded.dtmf_on_length);
        assert_eq!(0b011, decoded.dtmf_off_length);
        assert_eq!(Some(0x42), decoded.con_ref);
        assert_eq!(0, decoded.remaining_bits);

        let stm = AccessMessage::Status(StatusMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::Status,
            },
            record: AccessInfoRecord {
                record_type: 0x07,
                data: vec![0xde, 0xad, 0xbe, 0xef],
            },
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::Status,
            stm.to_sdu().expect("encode stm"),
        ))
        .expect("decode encoded stm");
        let AccessMessage::Status(decoded) = pdu.l3 else {
            panic!("expected STM");
        };
        assert_eq!(0x07, decoded.record.record_type);
        assert_eq!(vec![0xde, 0xad, 0xbe, 0xef], decoded.record.data);
        assert_eq!(0, decoded.remaining_bits);

        let orcm = AccessMessage::OriginationContinuation(OriginationContinuationMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::OriginationContinuation,
            },
            digit_mode: false,
            digits: vec![0x01, 0x02, 0x0a, 0x0b, 0x0c],
            records: vec![AccessInfoRecord {
                record_type: 0x10,
                data: vec![0x55],
            }],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::OriginationContinuation,
            orcm.to_sdu().expect("encode orcm"),
        ))
        .expect("decode encoded orcm");
        let AccessMessage::OriginationContinuation(decoded) = pdu.l3 else {
            panic!("expected ORCM");
        };
        assert!(!decoded.digit_mode);
        assert_eq!(vec![0x01, 0x02, 0x0a, 0x0b, 0x0c], decoded.digits);
        assert_eq!(1, decoded.records.len());
        assert_eq!(0x10, decoded.records[0].record_type);
        assert_eq!(vec![0x55], decoded.records[0].data);
        assert_eq!(0, decoded.remaining_bits);

        let hocm = AccessMessage::HandoffCompletion(HandoffCompletionMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::HandoffCompletion,
            },
            last_hdm_seq: 0b10,
            pilot_pns: vec![42, 84],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::HandoffCompletion,
            hocm.to_sdu().expect("encode hocm"),
        ))
        .expect("decode encoded hocm");
        let AccessMessage::HandoffCompletion(decoded) = pdu.l3 else {
            panic!("expected HOCM");
        };
        assert_eq!(0b10, decoded.last_hdm_seq);
        assert_eq!(vec![42, 84], decoded.pilot_pns);
        assert_eq!(0, decoded.remaining_bits);

        let mut parameter = Bitstream::new();
        parameter.write_u8(0b10101, 5);
        let prsm = AccessMessage::ParametersResponse(ParametersResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ParametersResponse,
            },
            records: vec![
                ParametersResponseRecord {
                    parameter_id: 0x1234,
                    parameter_len: 4,
                    parameter: parameter.clone(),
                },
                ParametersResponseRecord {
                    parameter_id: 0xbeef,
                    parameter_len: 0x03ff,
                    parameter: Bitstream::new(),
                },
            ],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ParametersResponse,
            prsm.to_sdu().expect("encode prsm"),
        ))
        .expect("decode encoded prsm");
        let AccessMessage::ParametersResponse(decoded) = pdu.l3 else {
            panic!("expected PRSM");
        };
        assert_eq!(2, decoded.records.len());
        assert_eq!(0x1234, decoded.records[0].parameter_id);
        assert_eq!(4, decoded.records[0].parameter_len);
        assert_eq!(parameter.bits(), decoded.records[0].parameter.bits());
        assert_eq!(0xbeef, decoded.records[1].parameter_id);
        assert_eq!(0x03ff, decoded.records[1].parameter_len);
        assert!(decoded.records[1].parameter.is_empty());
        assert_eq!(0, decoded.remaining_bits);

        let socm = AccessMessage::ServiceOptionControl(ServiceOptionControlMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ServiceOptionControl,
            },
            con_ref: 0x34,
            service_option: 0x1002,
            control_record: vec![0xaa, 0x55],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceOptionControl,
            socm.to_sdu().expect("encode socm"),
        ))
        .expect("decode encoded socm");
        let AccessMessage::ServiceOptionControl(decoded) = pdu.l3 else {
            panic!("expected SOCM");
        };
        assert_eq!(0x34, decoded.con_ref);
        assert_eq!(0x1002, decoded.service_option);
        assert_eq!(vec![0xaa, 0x55], decoded.control_record);
        assert_eq!(0, decoded.remaining_bits);

        let mut aux_record_bits = Bitstream::new();
        aux_record_bits.write_u8(0b01, 2); // QOF
        aux_record_bits.write_u8(0b000, 3); // WALSH_LENGTH: 64
        aux_record_bits.write_u8(0x15, 6); // PILOT_WALSH
        aux_record_bits.write_u8(0, 5); // RESERVED to octet-align
        let aux_record = SupplementalChannelPilotRecord {
            pilot_rec_type: 0,
            type_specific_fields: aux_record_bits.to_packed_bytes(),
        };
        let scrm = AccessMessage::SupplementalChannelRequest(SupplementalChannelRequestMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::SupplementalChannelRequest,
            },
            req_blob: vec![0x12, 0x34],
            scrm_seq_num: Some(0x0a),
            measurements: Some(SupplementalChannelRequestMeasurements {
                ref_pn: 42,
                pilot_strength: 21,
                active_pilots: vec![SupplementalChannelPilotReport {
                    pn_phase: 0x1234,
                    pilot_strength: 17,
                    pilot_record: None,
                }],
                neighbor_pilots: Some(vec![SupplementalChannelPilotReport {
                    pn_phase: 0x2345,
                    pilot_strength: 19,
                    pilot_record: Some(aux_record.clone()),
                }]),
                ref_pilot_record: Some(aux_record),
            }),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::SupplementalChannelRequest,
            scrm.to_sdu().expect("encode scrm"),
        ))
        .expect("decode encoded scrm");
        let AccessMessage::SupplementalChannelRequest(decoded) = pdu.l3 else {
            panic!("expected SCRM");
        };
        assert_eq!(vec![0x12, 0x34], decoded.req_blob);
        assert_eq!(Some(0x0a), decoded.scrm_seq_num);
        let measurements = decoded.measurements.expect("SCRM measurements");
        assert_eq!(42, measurements.ref_pn);
        assert_eq!(21, measurements.pilot_strength);
        assert_eq!(1, measurements.active_pilots.len());
        assert_eq!(0x1234, measurements.active_pilots[0].pn_phase);
        assert!(measurements.active_pilots[0].pilot_record.is_none());
        let neighbor_pilots = measurements.neighbor_pilots.expect("SCRM neighbors");
        assert_eq!(1, neighbor_pilots.len());
        assert_eq!(0x2345, neighbor_pilots[0].pn_phase);
        assert!(measurements.ref_pilot_record.is_some());
        assert!(neighbor_pilots[0].pilot_record.is_some());
        assert_eq!(0, decoded.remaining_bits);

        let cfsrsm =
            AccessMessage::CandidateFreqSearchResponse(CandidateFreqSearchResponseMessage {
                header: AccessMessageHeader {
                    pd: 0,
                    message_id: MessageId::CandidateFreqSearchResponse,
                },
                last_cfsrm_seq: 2,
                total_off_time_fwd: 12,
                max_off_time_fwd: 8,
                total_off_time_rev: 10,
                max_off_time_rev: 6,
                pcg_off_times: true,
                align_timing_used: true,
                max_num_visits: Some(3),
                inter_visit_time: Some(24),
                remaining_bits: 0,
            });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchResponse,
            cfsrsm.to_sdu().expect("encode cfsrsm"),
        ))
        .expect("decode encoded cfsrsm");
        let AccessMessage::CandidateFreqSearchResponse(decoded) = pdu.l3 else {
            panic!("expected CFSRSM");
        };
        assert_eq!(2, decoded.last_cfsrm_seq);
        assert_eq!(12, decoded.total_off_time_fwd);
        assert_eq!(8, decoded.max_off_time_fwd);
        assert_eq!(10, decoded.total_off_time_rev);
        assert_eq!(6, decoded.max_off_time_rev);
        assert!(decoded.pcg_off_times);
        assert!(decoded.align_timing_used);
        assert_eq!(Some(3), decoded.max_num_visits);
        assert_eq!(Some(24), decoded.inter_visit_time);
        assert_eq!(0, decoded.remaining_bits);

        let mut cfsrpm_record_bits = Bitstream::new();
        cfsrpm_record_bits.write_u8(0b10, 2); // QOF
        cfsrpm_record_bits.write_u8(0b001, 3); // WALSH_LENGTH: 128
        cfsrpm_record_bits.write_u8(0x2a, 7); // PILOT_WALSH
        cfsrpm_record_bits.write_u8(0, 4); // RESERVED to octet-align
        let cfsrpm = AccessMessage::CandidateFreqSearchReport(CandidateFreqSearchReportMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::CandidateFreqSearchReport,
            },
            last_srch_msg: false,
            last_srch_msg_seq: 1,
            search_mode: 0,
            mode_specific: CandidateFreqSearchReportModeSpecific::CdmaPilots(
                CandidateFreqSearchCdmaPilots {
                    band_class: 3,
                    cdma_freq: 384,
                    sf_total_rx_pwr: 17,
                    cf_total_rx_pwr: 19,
                    pilots: vec![
                        CandidateFreqSearchReportPilot {
                            pilot_pn_phase: 0x1234,
                            pilot_strength: 20,
                            pilot_record: None,
                        },
                        CandidateFreqSearchReportPilot {
                            pilot_pn_phase: 0x2345,
                            pilot_strength: 22,
                            pilot_record: Some(SupplementalChannelPilotRecord {
                                pilot_rec_type: 0,
                                type_specific_fields: cfsrpm_record_bits.to_packed_bytes(),
                            }),
                        },
                    ],
                },
            ),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchReport,
            cfsrpm.to_sdu().expect("encode cfsrpm"),
        ))
        .expect("decode encoded cfsrpm");
        let AccessMessage::CandidateFreqSearchReport(decoded) = pdu.l3 else {
            panic!("expected CFSRPM");
        };
        assert_eq!(1, decoded.last_srch_msg_seq);
        assert_eq!(0, decoded.search_mode);
        assert_eq!(0, decoded.remaining_bits);
        let CandidateFreqSearchReportModeSpecific::CdmaPilots(mode) = decoded.mode_specific else {
            panic!("expected CFSRPM CDMA pilot mode");
        };
        assert_eq!(3, mode.band_class);
        assert_eq!(384, mode.cdma_freq);
        assert_eq!(2, mode.pilots.len());
        assert!(mode.pilots[0].pilot_record.is_none());
        assert!(mode.pilots[1].pilot_record.is_some());

        let mut ppsmm_record_bits = Bitstream::new();
        ppsmm_record_bits.write_u8(0b01, 2); // QOF
        ppsmm_record_bits.write_u8(0b000, 3); // WALSH_LENGTH: 64
        ppsmm_record_bits.write_u8(0x15, 6); // PILOT_WALSH
        ppsmm_record_bits.write_u8(0, 5); // RESERVED to octet-align
        let ppsmm = AccessMessage::PeriodicPsmm(PeriodicPsmmMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::PeriodicPsmm,
            },
            ref_pn: 42,
            pilot_strength: 18,
            keep: true,
            sf_rx_pwr: 14,
            pilots: vec![PeriodicPsmmPilot {
                pilot_pn_phase: 0x1234,
                pilot_strength: 21,
                keep: false,
                pilot_record: Some(SupplementalChannelPilotRecord {
                    pilot_rec_type: 0,
                    type_specific_fields: ppsmm_record_bits.to_packed_bytes(),
                }),
            }],
            setpoints: Some(PeriodicPsmmSetpoints {
                fpc_fch_curr_setpt: Some(100),
                fpc_dcch_curr_setpt: None,
                sch_setpoints: vec![
                    PeriodicPsmmSchSetpoint {
                        sch_id: 0,
                        fpc_sch_curr_setpt: 110,
                    },
                    PeriodicPsmmSchSetpoint {
                        sch_id: 1,
                        fpc_sch_curr_setpt: 120,
                    },
                ],
            }),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::PeriodicPsmm,
            ppsmm.to_sdu().expect("encode ppsmm"),
        ))
        .expect("decode encoded ppsmm");
        let AccessMessage::PeriodicPsmm(decoded) = pdu.l3 else {
            panic!("expected PPSMM");
        };
        assert_eq!(42, decoded.ref_pn);
        assert_eq!(18, decoded.pilot_strength);
        assert!(decoded.keep);
        assert_eq!(14, decoded.sf_rx_pwr);
        assert_eq!(1, decoded.pilots.len());
        assert!(decoded.pilots[0].pilot_record.is_some());
        assert_eq!(0, decoded.remaining_bits);
        let setpoints = decoded.setpoints.expect("PPSMM setpoints");
        assert_eq!(Some(100), setpoints.fpc_fch_curr_setpt);
        assert_eq!(None, setpoints.fpc_dcch_curr_setpt);
        assert_eq!(2, setpoints.sch_setpoints.len());
        assert_eq!(120, setpoints.sch_setpoints[1].fpc_sch_curr_setpt);

        let olrm = AccessMessage::OuterLoopReport(OuterLoopReportMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::OuterLoopReport,
            },
            fpc_fch_curr_setpt: Some(90),
            fpc_dcch_curr_setpt: None,
            sch_setpoints: vec![PeriodicPsmmSchSetpoint {
                sch_id: 1,
                fpc_sch_curr_setpt: 100,
            }],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::OuterLoopReport,
            olrm.to_sdu().expect("encode olrm"),
        ))
        .expect("decode encoded olrm");
        let AccessMessage::OuterLoopReport(decoded) = pdu.l3 else {
            panic!("expected OLRM");
        };
        assert_eq!(Some(90), decoded.fpc_fch_curr_setpt);
        assert_eq!(None, decoded.fpc_dcch_curr_setpt);
        assert_eq!(1, decoded.sch_setpoints.len());
        assert_eq!(1, decoded.sch_setpoints[0].sch_id);
        assert_eq!(100, decoded.sch_setpoints[0].fpc_sch_curr_setpt);
        assert_eq!(0, decoded.remaining_bits);

        let rrm = AccessMessage::ResourceRequest(ResourceRequestMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ResourceRequest,
            },
            ch_ind: Some(0),
            ext_ch_ind: Some(0b01001),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ResourceRequest,
            rrm.to_sdu().expect("encode rrm"),
        ))
        .expect("decode encoded rrm");
        let AccessMessage::ResourceRequest(decoded) = pdu.l3 else {
            panic!("expected RRM");
        };
        assert_eq!(Some(0), decoded.ch_ind);
        assert_eq!(Some(0b01001), decoded.ext_ch_ind);
        assert_eq!(0, decoded.remaining_bits);

        let errm = AccessMessage::ExtReleaseResponse(ExtReleaseResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ExtReleaseResponse,
            },
            rsc_mode_ind: true,
            rsci: Some(2),
            rsc_end_time_unit: Some(1),
            rsc_end_time_value: Some(9),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ExtReleaseResponse,
            errm.to_sdu().expect("encode errm"),
        ))
        .expect("decode encoded errm");
        let AccessMessage::ExtReleaseResponse(decoded) = pdu.l3 else {
            panic!("expected ERRM");
        };
        assert!(decoded.rsc_mode_ind);
        assert_eq!(Some(2), decoded.rsci);
        assert_eq!(Some(1), decoded.rsc_end_time_unit);
        assert_eq!(Some(9), decoded.rsc_end_time_value);
        assert_eq!(0, decoded.remaining_bits);

        let strpm = AccessMessage::StatusResponse(StatusResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::StatusResponse,
            },
            qual_info_type: 1,
            qual_info: vec![0x02],
            records: vec![AccessInfoRecord {
                record_type: 0x13,
                data: vec![0xaa, 0x55],
            }],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::StatusResponse,
            strpm.to_sdu().expect("encode strpm"),
        ))
        .expect("decode encoded strpm");
        let AccessMessage::StatusResponse(decoded) = pdu.l3 else {
            panic!("expected STRPM");
        };
        assert_eq!(1, decoded.qual_info_type);
        assert_eq!(vec![0x02], decoded.qual_info);
        assert_eq!(1, decoded.records.len());
        assert_eq!(0x13, decoded.records[0].record_type);
        assert_eq!(vec![0xaa, 0x55], decoded.records[0].data);
        assert_eq!(0, decoded.remaining_bits);

        let tacm = AccessMessage::TmsiAssignmentCompletion(NoFieldAccessMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::TmsiAssignmentCompletion,
            },
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::TmsiAssignmentCompletion,
            tacm.to_sdu().expect("encode tacm"),
        ))
        .expect("decode encoded tacm");
        let AccessMessage::TmsiAssignmentCompletion(decoded) = pdu.l3 else {
            panic!("expected TACM");
        };
        assert_eq!(0, decoded.remaining_bits);

        let dim = AccessMessage::DeviceInformation(DeviceInformationMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::DeviceInformation,
            },
            wll_device_type: 0b101,
            num_info_records: 1,
            records: vec![AccessInfoRecord {
                record_type: 0x21,
                data: vec![0x01],
            }],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::DeviceInformation,
            dim.to_sdu().expect("encode dim"),
        ))
        .expect("decode encoded dim");
        let AccessMessage::DeviceInformation(decoded) = pdu.l3 else {
            panic!("expected DIM");
        };
        assert_eq!(0b101, decoded.wll_device_type);
        assert_eq!(1, decoded.num_info_records);
        assert_eq!(1, decoded.records.len());
        assert_eq!(0x21, decoded.records[0].record_type);
        assert_eq!(vec![0x01], decoded.records[0].data);
        assert_eq!(0, decoded.remaining_bits);

        let mut smrm_body = Bitstream::new();
        smrm_body.write_u8(1, 1); // UI_ENC_INCL
        smrm_body.write_u8(0b1100_0000, 8); // UI_ENCRYPT_SUP
        smrm_body.write_u8(0, 3); // NUM_RECS: one record
        smrm_body.write_u8(0x12, 8); // CON_REF[0]
        smrm_body.write_u8(1, 1); // UI_ENCRYPT_REQ[0]
        smrm_body.write_u8(1, 1); // SIG_ENC_INCL
        smrm_body.write_u8(0b1000_0000, 8); // SIG_ENCRYPT_SUP
        smrm_body.write_u8(1, 1); // D_SIG_ENCRYPT_REQ
        smrm_body.write_u8(1, 1); // NEW_SSEQ_H_INCL
        smrm_body.write_u32(0x123456, 24); // NEW_SSEQ_H
        smrm_body.write_u8(0x78, 8); // NEW_SSEQ_H_SIG
        smrm_body.write_u8(1, 1); // MSG_INT_INFO_INCL
        smrm_body.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
        smrm_body.write_u8(0, 8); // SIG_INTEGRITY_SUP
        smrm_body.write_u8(0, 3); // SIG_INTEGRITY_REQ
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::SecurityModeRequest,
            smrm_body.clone(),
        ))
        .expect("decode encoded smrm");
        let AccessMessage::SecurityModeRequest(decoded) = pdu.l3 else {
            panic!("expected SMRM");
        };
        assert_eq!(Some(0b1100_0000), decoded.ui_encrypt_sup);
        assert_eq!(1, decoded.ui_encrypt_records.len());
        assert_eq!(0x12, decoded.ui_encrypt_records[0].con_ref);
        assert!(decoded.ui_encrypt_records[0].ui_encrypt_req);
        assert_eq!(Some(0b1000_0000), decoded.sig_encrypt_sup);
        assert_eq!(None, decoded.c_sig_encrypt_req);
        assert_eq!(Some(true), decoded.d_sig_encrypt_req);
        assert_eq!(Some(0x123456), decoded.new_sseq_h);
        assert_eq!(Some(0x78), decoded.new_sseq_h_sig);
        assert_eq!(Some(true), decoded.msg_int_info_incl);
        assert_eq!(Some(true), decoded.sig_integrity_sup_incl);
        assert_eq!(Some(0), decoded.sig_integrity_sup);
        assert_eq!(Some(0), decoded.sig_integrity_req);
        assert_eq!(0, decoded.remaining_bits);
        assert_eq!(
            smrm_body.bits(),
            AccessMessage::SecurityModeRequest(decoded)
                .to_rdsch_sdu()
                .expect("encode r-dsch smrm")
                .bits()
        );

        let aurspm = AccessMessage::AuthResponse(AuthResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::AuthResponse,
            },
            res: (0u8..16).collect(),
            sig_integrity_sup: Some(0),
            sig_integrity_req: Some(0),
            new_key_id: 0b10,
            new_sseq_h: 0x654321,
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::AuthResponse,
            aurspm.to_sdu().expect("encode aurspm"),
        ))
        .expect("decode encoded aurspm");
        let AccessMessage::AuthResponse(decoded) = pdu.l3 else {
            panic!("expected AURSPM");
        };
        assert_eq!((0u8..16).collect::<Vec<u8>>(), decoded.res);
        assert_eq!(Some(0), decoded.sig_integrity_sup);
        assert_eq!(Some(0), decoded.sig_integrity_req);
        assert_eq!(0b10, decoded.new_key_id);
        assert_eq!(0x654321, decoded.new_sseq_h);
        assert_eq!(0, decoded.remaining_bits);

        let aursynm = AccessMessage::AuthResync(AuthResyncMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::AuthResync,
            },
            con_ms_sqn: vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60],
            mac_s: vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::AuthResync,
            aursynm.to_sdu().expect("encode aursynm"),
        ))
        .expect("decode encoded aursynm");
        let AccessMessage::AuthResync(decoded) = pdu.l3 else {
            panic!("expected AURSYNM");
        };
        assert_eq!(vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60], decoded.con_ms_sqn);
        assert_eq!(
            vec![0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
            decoded.mac_s
        );
        assert_eq!(0, decoded.remaining_bits);

        let mut message_record = Bitstream::new();
        message_record.write_u8(0b101, 3);
        let gem = AccessMessage::GeneralExtension(GeneralExtensionMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::GeneralExtension,
            },
            num_ge_records: 1,
            records: vec![AccessInfoRecord {
                record_type: 0,
                data: vec![0xf0],
            }],
            message_type: 0x2a,
            message_record: message_record.clone(),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::GeneralExtension,
            gem.to_sdu().expect("encode gem"),
        ))
        .expect("decode encoded gem");
        let AccessMessage::GeneralExtension(decoded) = pdu.l3 else {
            panic!("expected GEM");
        };
        assert_eq!(1, decoded.num_ge_records);
        assert_eq!(1, decoded.records.len());
        assert_eq!(0, decoded.records[0].record_type);
        assert_eq!(vec![0xf0], decoded.records[0].data);
        assert_eq!(0x2a, decoded.message_type);
        assert_eq!(message_record.bits(), decoded.message_record.bits());
        assert_eq!(0, decoded.remaining_bits);

        let scc = AccessMessage::ServiceConnectCompletion(ServiceConnectCompletionMessage {
            serv_con_seq: 5,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceConnectCompletion,
            scc.to_sdu().expect("encode scc"),
        ))
        .expect("decode encoded scc");
        let AccessMessage::ServiceConnectCompletion(decoded) = pdu.l3 else {
            panic!("expected SCCM");
        };
        assert_eq!(5, decoded.serv_con_seq);

        let psmm = AccessMessage::PilotStrengthMeasurement(PilotStrengthMeasurementMessage {
            ref_pn: 42,
            pilot_strength: 17,
            keep: true,
            pilots: vec![PilotReport {
                pilot_pn_phase: 64,
                pilot_strength: 21,
                keep: false,
            }],
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::Psmm,
            psmm.to_sdu().expect("encode psmm"),
        ))
        .expect("decode encoded psmm");
        let AccessMessage::PilotStrengthMeasurement(decoded) = pdu.l3 else {
            panic!("expected PSMM");
        };
        assert_eq!(42, decoded.ref_pn);
        assert_eq!(1, decoded.pilots.len());

        let pmrm = AccessMessage::PowerMeasurementReport(PowerMeasurementReportMessage {
            errors_detected: 3,
            pwr_meas_frames: 511,
            last_hdm_seq: 2,
            pilot_strengths: vec![10, 20],
            dcch_pwr_meas_incl: true,
            dcch_pwr_meas_frames: Some(300),
            dcch_errors_detected: Some(4),
            sch_pwr_meas_incl: true,
            sch_id: Some(1),
            sch_pwr_meas_frames: Some(1024),
            sch_errors_detected: Some(9),
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::PowerMeasurementReport,
            pmrm.to_sdu().expect("encode pmrm"),
        ))
        .expect("decode encoded pmrm");
        let AccessMessage::PowerMeasurementReport(decoded) = pdu.l3 else {
            panic!("expected PMRM");
        };
        assert_eq!(vec![10, 20], decoded.pilot_strengths);
        assert_eq!(Some(300), decoded.dcch_pwr_meas_frames);
        assert_eq!(Some(1024), decoded.sch_pwr_meas_frames);

        let srqm = AccessMessage::ServiceRequest(ServiceRequestMessage {
            serv_req_seq: 3,
            req_purpose: 0,
            service_config: None,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceRequest,
            srqm.to_sdu().expect("encode srqm"),
        ))
        .expect("decode encoded srqm");
        let AccessMessage::ServiceRequest(decoded) = pdu.l3 else {
            panic!("expected SRQM");
        };
        assert_eq!(3, decoded.serv_req_seq);
        assert_eq!(0, decoded.req_purpose);

        let srpm = AccessMessage::ServiceResponse(ServiceResponseMessage {
            serv_req_seq: 4,
            resp_purpose: 1,
            service_config: None,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceResponse,
            srpm.to_sdu().expect("encode srpm"),
        ))
        .expect("decode encoded srpm");
        let AccessMessage::ServiceResponse(decoded) = pdu.l3 else {
            panic!("expected SRPM");
        };
        assert_eq!(4, decoded.serv_req_seq);
        assert_eq!(1, decoded.resp_purpose);
    }

    #[test]
    fn test_rdsch_unsupported_body_returns_error() {
        let mut body = Bitstream::new();
        body.write_u8(0, 1); // minimum payload bit so the PDU clears header-length validation
        let pdu = rdsch_pdu(MessageId::EnhancedOrigination, body);

        let err = RdschPdu::decode(&pdu).unwrap_err();

        assert!(err.contains("unsupported r-dsch body decode"));
        assert!(err.contains("EOM"));
    }

    #[test]
    fn test_rdsch_flash_with_info_rejects_nonzero_padding() {
        let mut body = Bitstream::new();
        body.write_u8(1, 1);
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::FlashWithInfo, body)).unwrap_err();

        assert!(err.contains("FWIM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_shared_bodies_validate_pdu_padding() {
        let mut aucrm = Bitstream::new();
        aucrm.write_u32(0x2aaaa, 18);
        aucrm.write_u8(0, 5);
        let pdu = RdschPdu::decode(&rdsch_pdu(MessageId::AuthChallengeResponse, aucrm))
            .expect("decode AUCRM with zero padding");
        let AccessMessage::AuthChallengeResponse(decoded) = pdu.l3 else {
            panic!("expected AUCRM");
        };
        assert_eq!(0, decoded.remaining_bits);

        let mut aucrm = Bitstream::new();
        aucrm.write_u32(0x2aaaa, 18);
        aucrm.write_u8(1, 1);
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::AuthChallengeResponse, aucrm)).unwrap_err();
        assert!(err.contains("AUCRM PDU padding contains non-zero bits"));

        let mut tacm = Bitstream::new();
        tacm.write_u8(1, 1);
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::TmsiAssignmentCompletion, tacm)).unwrap_err();
        assert!(err.contains("TACM PDU padding contains non-zero bits"));

        let mut strpm = Bitstream::new();
        strpm.write_u8(1, 8); // QUAL_INFO_TYPE
        strpm.write_u8(0, 3); // QUAL_INFO_LEN
        strpm.write_u8(0x13, 8); // RECORD_TYPE
        strpm.write_u8(0, 8); // RECORD_LEN
        strpm.write_u8(1, 1);
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::StatusResponse, strpm)).unwrap_err();
        assert!(err.contains("STRPM PDU padding contains non-zero bits"));

        let mut dim = Bitstream::new();
        dim.write_u8(0b101, 3); // WLL_DEVICE_TYPE
        dim.write_u8(0, 5); // NUM_INFO_RECORDS
        dim.write_u8(1, 1);
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::DeviceInformation, dim)).unwrap_err();
        assert!(err.contains("DIM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_handoff_completion_rejects_missing_pilot_and_nonzero_padding() {
        let mut no_pilot = Bitstream::new();
        no_pilot.write_u8(0b10, 2); // LAST_HDM_SEQ
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::HandoffCompletion, no_pilot)).unwrap_err();
        assert!(err.contains("HOCM requires one or more PILOT_PN fields"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(0b10, 2); // LAST_HDM_SEQ
        nonzero_padding.write_u32(42, 9); // PILOT_PN[0]
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::HandoffCompletion, nonzero_padding))
            .unwrap_err();
        assert!(err.contains("HOCM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_parameters_response_rejects_missing_record_and_nonzero_padding() {
        let empty = Bitstream::new();
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::ParametersResponse, empty)).unwrap_err();
        assert!(err.contains("PRSM requires one or more parameter records"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u32(0x1234, 16); // PARAMETER_ID
        nonzero_padding.write_u32(0x03ff, 10); // PARAMETER_LEN all ones, parameter omitted
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::ParametersResponse, nonzero_padding))
            .unwrap_err();
        assert!(err.contains("PRSM PDU padding contains non-zero bits"));

        let mut truncated_parameter = Bitstream::new();
        truncated_parameter.write_u32(0x1234, 16); // PARAMETER_ID
        truncated_parameter.write_u32(4, 10); // PARAMETER_LEN: 5 parameter bits
        truncated_parameter.write_u8(0b101, 3); // truncated PARAMETER
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::ParametersResponse,
            truncated_parameter,
        ))
        .unwrap_err();
        assert!(err.contains("EOF reading PARAMETER[0] (5 bits)"));
    }

    #[test]
    fn test_rdsch_service_option_control_rejects_reserved_length_and_padding() {
        let mut reserved = Bitstream::new();
        reserved.write_u8(0x34, 8); // CON_REF
        reserved.write_u32(0x1002, 16); // SERVICE_OPTION
        reserved.write_u8(1, 7); // RESERVED must be zero
        reserved.write_u8(0, 8); // CTL_REC_LEN
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ServiceOptionControl, reserved)).unwrap_err();
        assert!(err.contains("SOCM RESERVED must be zero"));

        let mut truncated = Bitstream::new();
        truncated.write_u8(0x34, 8); // CON_REF
        truncated.write_u32(0x1002, 16); // SERVICE_OPTION
        truncated.write_u8(0, 7); // RESERVED
        truncated.write_u8(2, 8); // CTL_REC_LEN
        truncated.write_u8(0xaa, 8); // only one CTL_REC octet
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ServiceOptionControl, truncated)).unwrap_err();
        assert!(err.contains("EOF reading CTL_REC"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(0x34, 8); // CON_REF
        nonzero_padding.write_u32(0x1002, 16); // SERVICE_OPTION
        nonzero_padding.write_u8(0, 7); // RESERVED
        nonzero_padding.write_u8(0, 8); // CTL_REC_LEN
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::ServiceOptionControl, nonzero_padding))
            .unwrap_err();
        assert!(err.contains("SOCM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_supplemental_channel_request_rejects_invalid_pilot_records() {
        let minimal =
            AccessMessage::SupplementalChannelRequest(SupplementalChannelRequestMessage {
                header: AccessMessageHeader {
                    pd: 0,
                    message_id: MessageId::SupplementalChannelRequest,
                },
                req_blob: Vec::new(),
                scrm_seq_num: None,
                measurements: None,
                remaining_bits: 0,
            });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::SupplementalChannelRequest,
            minimal.to_sdu().expect("encode minimal scrm"),
        ))
        .expect("decode minimal scrm");
        let AccessMessage::SupplementalChannelRequest(decoded) = pdu.l3 else {
            panic!("expected SCRM");
        };
        assert!(decoded.req_blob.is_empty());
        assert!(decoded.scrm_seq_num.is_none());
        assert!(decoded.measurements.is_none());

        let mut missing_tail = Bitstream::new();
        missing_tail.write_u8(0, 4); // SIZE_OF_REQ_BLOB
        missing_tail.write_u8(1, 1); // USE_SCRM_SEQ_NUM
        missing_tail.write_u8(3, 4); // SCRM_SEQ_NUM, then missing REF_PN
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SupplementalChannelRequest,
            missing_tail,
        ))
        .unwrap_err();
        assert!(err.contains("EOF reading REF_PN"));

        let mut reserved_type = Bitstream::new();
        reserved_type.write_u8(1, 4); // SIZE_OF_REQ_BLOB
        reserved_type.write_u8(0xaa, 8); // REQ_BLOB
        reserved_type.write_u8(0, 1); // USE_SCRM_SEQ_NUM
        reserved_type.write_u32(42, 9); // REF_PN
        reserved_type.write_u8(21, 6); // PILOT_STRENGTH
        reserved_type.write_u8(0, 3); // NUM_ACT_PN
        reserved_type.write_u8(0, 3); // NUM_NGHBR_PN
        reserved_type.write_u8(1, 1); // REF_PILOT_REC_INCL
        reserved_type.write_u8(1, 3); // reserved REF_PILOT_REC_TYPE
        reserved_type.write_u8(0, 3); // REF_RECORD_LEN
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SupplementalChannelRequest,
            reserved_type,
        ))
        .unwrap_err();
        assert!(err.contains("reserved PILOT_REC_TYPE"));

        let mut bad_reserved_tail = Bitstream::new();
        bad_reserved_tail.write_u8(1, 4); // SIZE_OF_REQ_BLOB
        bad_reserved_tail.write_u8(0xaa, 8); // REQ_BLOB
        bad_reserved_tail.write_u8(0, 1); // USE_SCRM_SEQ_NUM
        bad_reserved_tail.write_u32(42, 9); // REF_PN
        bad_reserved_tail.write_u8(21, 6); // PILOT_STRENGTH
        bad_reserved_tail.write_u8(0, 3); // NUM_ACT_PN
        bad_reserved_tail.write_u8(0, 3); // NUM_NGHBR_PN
        bad_reserved_tail.write_u8(1, 1); // REF_PILOT_REC_INCL
        bad_reserved_tail.write_u8(0, 3); // REF_PILOT_REC_TYPE
        bad_reserved_tail.write_u8(2, 3); // REF_RECORD_LEN
        bad_reserved_tail.write_u8(0, 2); // QOF
        bad_reserved_tail.write_u8(0, 3); // WALSH_LENGTH
        bad_reserved_tail.write_u8(0x15, 6); // PILOT_WALSH
        bad_reserved_tail.write_u8(1, 5); // RESERVED must be zero
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SupplementalChannelRequest,
            bad_reserved_tail,
        ))
        .unwrap_err();
        assert!(err.contains("RESERVED bits must be zero"));
    }

    #[test]
    fn test_rdsch_candidate_freq_search_response_rejects_optional_tail_and_padding() {
        let no_alignment =
            AccessMessage::CandidateFreqSearchResponse(CandidateFreqSearchResponseMessage {
                header: AccessMessageHeader {
                    pd: 0,
                    message_id: MessageId::CandidateFreqSearchResponse,
                },
                last_cfsrm_seq: 1,
                total_off_time_fwd: 2,
                max_off_time_fwd: 3,
                total_off_time_rev: 4,
                max_off_time_rev: 5,
                pcg_off_times: false,
                align_timing_used: false,
                max_num_visits: None,
                inter_visit_time: None,
                remaining_bits: 0,
            });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchResponse,
            no_alignment.to_sdu().expect("encode cfsrsm"),
        ))
        .expect("decode cfsrsm without alignment tail");
        let AccessMessage::CandidateFreqSearchResponse(decoded) = pdu.l3 else {
            panic!("expected CFSRSM");
        };
        assert!(!decoded.align_timing_used);
        assert_eq!(None, decoded.max_num_visits);
        assert_eq!(None, decoded.inter_visit_time);

        let err = AccessMessage::CandidateFreqSearchResponse(CandidateFreqSearchResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::CandidateFreqSearchResponse,
            },
            last_cfsrm_seq: 1,
            total_off_time_fwd: 2,
            max_off_time_fwd: 3,
            total_off_time_rev: 4,
            max_off_time_rev: 5,
            pcg_off_times: false,
            align_timing_used: true,
            max_num_visits: Some(2),
            inter_visit_time: None,
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("INTER_VISIT_TIME is required"));

        let mut truncated = Bitstream::new();
        truncated.write_u8(1, 2); // LAST_CFSRM_SEQ
        truncated.write_u8(2, 6); // TOTAL_OFF_TIME_FWD
        truncated.write_u8(3, 6); // MAX_OFF_TIME_FWD
        truncated.write_u8(4, 6); // TOTAL_OFF_TIME_REV
        truncated.write_u8(5, 6); // MAX_OFF_TIME_REV
        truncated.write_u8(0, 1); // PCG_OFF_TIMES
        truncated.write_u8(1, 1); // ALIGN_TIMING_USED, missing MAX_NUM_VISITS
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchResponse,
            truncated,
        ))
        .unwrap_err();
        assert!(err.contains("EOF reading MAX_NUM_VISITS"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(1, 2); // LAST_CFSRM_SEQ
        nonzero_padding.write_u8(2, 6); // TOTAL_OFF_TIME_FWD
        nonzero_padding.write_u8(3, 6); // MAX_OFF_TIME_FWD
        nonzero_padding.write_u8(4, 6); // TOTAL_OFF_TIME_REV
        nonzero_padding.write_u8(5, 6); // MAX_OFF_TIME_REV
        nonzero_padding.write_u8(0, 1); // PCG_OFF_TIMES
        nonzero_padding.write_u8(0, 1); // ALIGN_TIMING_USED
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchResponse,
            nonzero_padding,
        ))
        .unwrap_err();
        assert!(err.contains("CFSRSM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_candidate_freq_search_report_rejects_reserved_and_padding() {
        let external = AccessMessage::CandidateFreqSearchReport(CandidateFreqSearchReportMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::CandidateFreqSearchReport,
            },
            last_srch_msg: true,
            last_srch_msg_seq: 2,
            search_mode: 2,
            mode_specific: CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(vec![
                0xde, 0xad, 0xbe, 0xef,
            ]),
            remaining_bits: 0,
        });
        let pdu = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchReport,
            external.to_sdu().expect("encode external cfsrpm"),
        ))
        .expect("decode external cfsrpm");
        let AccessMessage::CandidateFreqSearchReport(decoded) = pdu.l3 else {
            panic!("expected CFSRPM");
        };
        assert!(decoded.last_srch_msg);
        assert_eq!(2, decoded.search_mode);
        let CandidateFreqSearchReportModeSpecific::ExternalDsNeighbor(bytes) =
            decoded.mode_specific
        else {
            panic!("expected external CFSRPM mode");
        };
        assert_eq!(vec![0xde, 0xad, 0xbe, 0xef], bytes);

        let mut reserved_mode = Bitstream::new();
        reserved_mode.write_u8(0, 1); // LAST_SRCH_MSG
        reserved_mode.write_u8(0, 2); // LAST_SRCH_MSG_SEQ
        reserved_mode.write_u8(1, 4); // reserved SEARCH_MODE
        reserved_mode.write_u8(0, 8); // MODE_SPECIFIC_LEN
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchReport,
            reserved_mode,
        ))
        .unwrap_err();
        assert!(err.contains("SEARCH_MODE 0b0001 is reserved"));

        let mut mode = Bitstream::new();
        mode.write_u8(3, 5); // BAND_CLASS
        mode.write_u32(384, 11); // CDMA_FREQ
        mode.write_u8(17, 5); // SF_TOTAL_RX_PWR
        mode.write_u8(19, 5); // CF_TOTAL_RX_PWR
        mode.write_u8(1, 6); // NUM_PILOTS
        mode.write_u32(0x1234, 15); // PILOT_PN_PHASE[0]
        mode.write_u8(20, 6); // PILOT_STRENGTH[0]
        mode.write_u8(1, 3); // RESERVED_1[0] must be zero
        mode.write_u8(0, 1); // PILOT_REC_INCL[0]
        mode.write_u8(0, 7); // padding to MODE_SPECIFIC_LEN
        let mut reserved_1 = Bitstream::new();
        reserved_1.write_u8(0, 1); // LAST_SRCH_MSG
        reserved_1.write_u8(0, 2); // LAST_SRCH_MSG_SEQ
        reserved_1.write_u8(0, 4); // SEARCH_MODE
        reserved_1.write_u8(8, 8); // MODE_SPECIFIC_LEN
        reserved_1.extend(&mode);
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::CandidateFreqSearchReport, reserved_1))
            .unwrap_err();
        assert!(err.contains("RESERVED_1[0] must be zero"));

        let mut mode = Bitstream::new();
        mode.write_u8(3, 5); // BAND_CLASS
        mode.write_u32(384, 11); // CDMA_FREQ
        mode.write_u8(17, 5); // SF_TOTAL_RX_PWR
        mode.write_u8(19, 5); // CF_TOTAL_RX_PWR
        mode.write_u8(1, 6); // NUM_PILOTS
        mode.write_u32(0x1234, 15); // PILOT_PN_PHASE[0]
        mode.write_u8(20, 6); // PILOT_STRENGTH[0]
        mode.write_u8(0, 3); // RESERVED_1[0]
        mode.write_u8(0, 1); // PILOT_REC_INCL[0]
        mode.write_u8(1, 1); // invalid mode-specific padding
        mode.write_u8(0, 6); // remaining padding
        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(0, 1); // LAST_SRCH_MSG
        nonzero_padding.write_u8(0, 2); // LAST_SRCH_MSG_SEQ
        nonzero_padding.write_u8(0, 4); // SEARCH_MODE
        nonzero_padding.write_u8(8, 8); // MODE_SPECIFIC_LEN
        nonzero_padding.extend(&mode);
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::CandidateFreqSearchReport,
            nonzero_padding,
        ))
        .unwrap_err();
        assert!(err.contains("CFSRPM mode-specific PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_periodic_psmm_rejects_invalid_records_and_padding() {
        let err = AccessMessage::PeriodicPsmm(PeriodicPsmmMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::PeriodicPsmm,
            },
            ref_pn: 42,
            pilot_strength: 18,
            keep: true,
            sf_rx_pwr: 14,
            pilots: Vec::new(),
            setpoints: Some(PeriodicPsmmSetpoints {
                fpc_fch_curr_setpt: None,
                fpc_dcch_curr_setpt: None,
                sch_setpoints: vec![PeriodicPsmmSchSetpoint {
                    sch_id: 2,
                    fpc_sch_curr_setpt: 100,
                }],
            }),
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("SCH_ID[0] value 2 exceeds 1 bit"));

        let mut reserved_type = Bitstream::new();
        reserved_type.write_u32(42, 9); // REF_PN
        reserved_type.write_u8(18, 6); // PILOT_STRENGTH
        reserved_type.write_u8(1, 1); // KEEP
        reserved_type.write_u8(14, 5); // SF_RX_PWR
        reserved_type.write_u8(1, 4); // NUM_PILOT
        reserved_type.write_u32(0x1234, 15); // PILOT_PN_PHASE[0]
        reserved_type.write_u8(21, 6); // PILOT_STRENGTH[0]
        reserved_type.write_u8(1, 1); // KEEP[0]
        reserved_type.write_u8(1, 1); // PILOT_REC_INCL[0]
        reserved_type.write_u8(1, 3); // reserved PILOT_REC_TYPE
        reserved_type.write_u8(0, 3); // RECORD_LEN
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::PeriodicPsmm, reserved_type)).unwrap_err();
        assert!(err.contains("PPSMM PILOT_REC[0] reserved PILOT_REC_TYPE"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u32(42, 9); // REF_PN
        nonzero_padding.write_u8(18, 6); // PILOT_STRENGTH
        nonzero_padding.write_u8(1, 1); // KEEP
        nonzero_padding.write_u8(14, 5); // SF_RX_PWR
        nonzero_padding.write_u8(0, 4); // NUM_PILOT
        nonzero_padding.write_u8(0, 1); // SETPT_INCL
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::PeriodicPsmm, nonzero_padding)).unwrap_err();
        assert!(err.contains("PPSMM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_outer_loop_report_rejects_invalid_sch_and_padding() {
        let err = AccessMessage::OuterLoopReport(OuterLoopReportMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::OuterLoopReport,
            },
            fpc_fch_curr_setpt: None,
            fpc_dcch_curr_setpt: None,
            sch_setpoints: vec![PeriodicPsmmSchSetpoint {
                sch_id: 2,
                fpc_sch_curr_setpt: 100,
            }],
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("OLRM SCH_ID[0] value 2 exceeds 1 bit"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(0, 1); // FCH_INCL
        nonzero_padding.write_u8(0, 1); // DCCH_INCL
        nonzero_padding.write_u8(0, 2); // NUM_SUP
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::OuterLoopReport, nonzero_padding)).unwrap_err();
        assert!(err.contains("OLRM PDU padding contains non-zero bits"));

        let mut truncated = Bitstream::new();
        truncated.write_u8(0, 1); // FCH_INCL
        truncated.write_u8(0, 1); // DCCH_INCL
        truncated.write_u8(1, 2); // NUM_SUP
        truncated.write_u8(1, 1); // SCH_ID[0], missing FPC_SCH_CURR_SETPT[0]
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::OuterLoopReport, truncated)).unwrap_err();
        assert!(err.contains("EOF reading FPC_SCH_CURR_SETPT[0]"));
    }

    #[test]
    fn test_rdsch_resource_request_rejects_invalid_ext_ch_ind_and_padding() {
        let err = AccessMessage::ResourceRequest(ResourceRequestMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ResourceRequest,
            },
            ch_ind: None,
            ext_ch_ind: Some(0b01001),
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("RRM EXT_CH_IND requires CH_IND=00"));

        let err = AccessMessage::ResourceRequest(ResourceRequestMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ResourceRequest,
            },
            ch_ind: Some(0),
            ext_ch_ind: Some(0),
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("RRM EXT_CH_IND value 0b00000 is reserved or invalid"));

        let mut reserved_ext = Bitstream::new();
        reserved_ext.write_u8(1, 1); // CH_IND_INCL
        reserved_ext.write_u8(0, 2); // CH_IND=00 includes EXT_CH_IND
        reserved_ext.write_u8(0b00111, 5); // reserved EXT_CH_IND
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ResourceRequest, reserved_ext)).unwrap_err();
        assert!(err.contains("RRM EXT_CH_IND value 0b00111 is reserved or invalid"));

        let mut truncated_ext = Bitstream::new();
        truncated_ext.write_u8(1, 1); // CH_IND_INCL
        truncated_ext.write_u8(0, 2); // CH_IND=00 includes EXT_CH_IND
        truncated_ext.write_u8(0b010, 3); // truncated EXT_CH_IND
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ResourceRequest, truncated_ext)).unwrap_err();
        assert!(err.contains("EOF reading EXT_CH_IND"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(0, 1); // CH_IND_INCL
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ResourceRequest, nonzero_padding)).unwrap_err();
        assert!(err.contains("RRM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_ext_release_response_rejects_reserved_fields_and_padding() {
        let err = AccessMessage::ExtReleaseResponse(ExtReleaseResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ExtReleaseResponse,
            },
            rsc_mode_ind: true,
            rsci: Some(1),
            rsc_end_time_unit: Some(0b11),
            rsc_end_time_value: Some(2),
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("ERRM RSC_END_TIME_UNIT 0b11 is reserved"));

        let err = AccessMessage::ExtReleaseResponse(ExtReleaseResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ExtReleaseResponse,
            },
            rsc_mode_ind: true,
            rsci: Some(0b0101),
            rsc_end_time_unit: Some(0),
            rsc_end_time_value: Some(2),
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("ERRM RSCI 0b0101 is reserved"));

        let err = AccessMessage::ExtReleaseResponse(ExtReleaseResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::ExtReleaseResponse,
            },
            rsc_mode_ind: false,
            rsci: Some(1),
            rsc_end_time_unit: None,
            rsc_end_time_value: None,
            remaining_bits: 0,
        })
        .to_sdu()
        .unwrap_err();
        assert!(err.contains("ERRM reduced-slot-cycle fields require RSC_MODE_IND=1"));

        let mut reserved_unit = Bitstream::new();
        reserved_unit.write_u8(1, 1); // RSC_MODE_IND
        reserved_unit.write_u8(1, 4); // RSCI
        reserved_unit.write_u8(0b11, 2); // reserved RSC_END_TIME_UNIT
        reserved_unit.write_u8(2, 4); // RSC_END_TIME_VALUE
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ExtReleaseResponse, reserved_unit)).unwrap_err();
        assert!(err.contains("ERRM RSC_END_TIME_UNIT 0b11 is reserved"));

        let mut reserved_rsci = Bitstream::new();
        reserved_rsci.write_u8(1, 1); // RSC_MODE_IND
        reserved_rsci.write_u8(0b0101, 4); // reserved RSCI
        reserved_rsci.write_u8(0, 2); // RSC_END_TIME_UNIT
        reserved_rsci.write_u8(2, 4); // RSC_END_TIME_VALUE
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ExtReleaseResponse, reserved_rsci)).unwrap_err();
        assert!(err.contains("ERRM RSCI 0b0101 is reserved"));

        let mut truncated = Bitstream::new();
        truncated.write_u8(1, 1); // RSC_MODE_IND
        truncated.write_u8(1, 4); // RSCI
        truncated.write_u8(0, 2); // RSC_END_TIME_UNIT
        truncated.write_u8(0b101, 3); // truncated RSC_END_TIME_VALUE
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ExtReleaseResponse, truncated)).unwrap_err();
        assert!(err.contains("EOF reading RSC_END_TIME_VALUE"));

        let mut nonzero_padding = Bitstream::new();
        nonzero_padding.write_u8(0, 1); // RSC_MODE_IND
        nonzero_padding.write_u8(1, 1); // invalid PDU padding
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::ExtReleaseResponse, nonzero_padding))
            .unwrap_err();
        assert!(err.contains("ERRM PDU padding contains non-zero bits"));
    }

    #[test]
    fn test_rdsch_security_messages_reject_reserved_fields() {
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SecurityModeRequest,
            rdsch_smrm_body(0b1100_0001, 0b1000_0000, 0, 0),
        ))
        .unwrap_err();
        assert!(err.contains("UI_ENCRYPT_SUP RESERVED subfield must be zero"));

        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SecurityModeRequest,
            rdsch_smrm_body(0b1100_0000, 0, 0, 0),
        ))
        .unwrap_err();
        assert!(err.contains("SIG_ENCRYPT_SUP CMEA subfield must be 1"));

        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SecurityModeRequest,
            rdsch_smrm_body(0b1100_0000, 0b1000_0000, 1, 0),
        ))
        .unwrap_err();
        assert!(err.contains("SIG_INTEGRITY_SUP RESERVED subfield must be zero"));

        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::SecurityModeRequest,
            rdsch_smrm_body(0b1100_0000, 0b1000_0000, 0, 1),
        ))
        .unwrap_err();
        assert!(err.contains("SIG_INTEGRITY_REQ reserved value"));

        let err = RdschPdu::decode(&rdsch_pdu(MessageId::AuthResponse, rdsch_aurspm_body(1, 0)))
            .unwrap_err();
        assert!(err.contains("SIG_INTEGRITY_SUP RESERVED subfield must be zero"));

        let err = RdschPdu::decode(&rdsch_pdu(MessageId::AuthResponse, rdsch_aurspm_body(0, 1)))
            .unwrap_err();
        assert!(err.contains("SIG_INTEGRITY_REQ reserved value"));
    }

    #[test]
    fn test_rdsch_security_encoders_reject_reserved_fields() {
        let smrm = AccessMessage::SecurityModeRequest(SecurityModeRequestMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::SecurityModeRequest,
            },
            ui_encrypt_sup: Some(0b1100_0001),
            ui_encrypt_records: vec![SecurityModeUiEncryptRecord {
                con_ref: 0x12,
                ui_encrypt_req: true,
            }],
            sig_encrypt_sup: Some(0b1000_0000),
            c_sig_encrypt_req: None,
            d_sig_encrypt_req: Some(true),
            new_sseq_h: None,
            new_sseq_h_sig: None,
            msg_int_info_incl: Some(false),
            sig_integrity_sup_incl: None,
            sig_integrity_sup: None,
            sig_integrity_req: None,
            remaining_bits: 0,
        });
        let err = smrm.to_rdsch_sdu().unwrap_err();
        assert!(err.contains("UI_ENCRYPT_SUP RESERVED subfield must be zero"));

        let aurspm = AccessMessage::AuthResponse(AuthResponseMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::AuthResponse,
            },
            res: (0u8..16).collect(),
            sig_integrity_sup: Some(1),
            sig_integrity_req: Some(0),
            new_key_id: 0,
            new_sseq_h: 0,
            remaining_bits: 0,
        });
        let err = aurspm.to_sdu().unwrap_err();
        assert!(err.contains("SIG_INTEGRITY_SUP RESERVED subfield must be zero"));
    }

    #[test]
    fn test_rdsch_send_burst_dtmf_rejects_reserved_values() {
        let mut reserved_on = Bitstream::new();
        reserved_on.write_u8(1, 8); // NUM_DIGITS
        reserved_on.write_u8(0b110, 3); // reserved DTMF_ON_LENGTH
        reserved_on.write_u8(0, 3); // DTMF_OFF_LENGTH
        reserved_on.write_u8(1, 4); // DIGIT[0]
        reserved_on.write_u8(0, 1); // CON_REF_INCL
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::SendBurstDtmf, reserved_on)).unwrap_err();
        assert!(err.contains("DTMF_ON_LENGTH=0b110 is reserved"));

        let mut reserved_digit = Bitstream::new();
        reserved_digit.write_u8(1, 8); // NUM_DIGITS
        reserved_digit.write_u8(0, 3); // DTMF_ON_LENGTH
        reserved_digit.write_u8(0, 3); // DTMF_OFF_LENGTH
        reserved_digit.write_u8(0, 4); // reserved DIGIT[0]
        reserved_digit.write_u8(0, 1); // CON_REF_INCL
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::SendBurstDtmf, reserved_digit)).unwrap_err();
        assert!(err.contains("DIGIT code 0x0 is reserved"));
    }

    #[test]
    fn test_rdsch_status_rejects_truncated_record() {
        let mut body = Bitstream::new();
        body.write_u8(0x07, 8); // RECORD_TYPE without RECORD_LEN
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::Status, body)).unwrap_err();

        assert!(err.contains("EOF reading STM[0].RECORD_LEN"));
    }

    #[test]
    fn test_rdsch_origination_continuation_rejects_reserved_digits() {
        let mut body = Bitstream::new();
        body.write_u8(0, 1); // DIGIT_MODE = DTMF
        body.write_u8(1, 8); // NUM_FIELDS
        body.write_u8(0, 4); // reserved DTMF digit
        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::OriginationContinuation, body)).unwrap_err();

        assert!(err.contains("DIGIT code 0x0 is reserved"));
    }

    #[test]
    fn test_fdsch_unsupported_body_returns_error() {
        let mut bits = Bitstream::new();
        bits.write_u8(
            MessageId::AuthChallenge
                .wire_type(WireChannel::ForwardDedicated)
                .unwrap(),
            8,
        );
        bits.write_u8(0, 3); // ACK_SEQ
        bits.write_u8(0, 3); // MSG_SEQ
        bits.write_u8(0, 1); // ACK_REQ
        bits.write_u8(0, 2); // ENCRYPTION
        bits.write_u8(0, 1); // minimum payload bit so the PDU clears header-length validation

        let err = super::FdschPdu::decode(&bits).unwrap_err();

        assert!(err.contains("unsupported f-dsch body decode"));
        assert!(err.contains("AUCM"));
    }

    #[test]
    fn test_fdsch_service_request_response_roundtrip() {
        let srqm = AccessMessage::ServiceRequest(ServiceRequestMessage {
            serv_req_seq: 3,
            req_purpose: 1,
            service_config: None,
        });
        let pdu = FdschPdu::decode(&fdsch_pdu(
            MessageId::ServiceRequest,
            srqm.to_sdu().expect("encode f-dsch srqm body"),
        ))
        .expect("decode f-dsch srqm");
        let FdschMessage::ServiceRequest(decoded) = pdu.body else {
            panic!("expected f-dsch SRQM");
        };
        assert_eq!(3, decoded.serv_req_seq);
        assert_eq!(1, decoded.req_purpose);

        let srpm = AccessMessage::ServiceResponse(ServiceResponseMessage {
            serv_req_seq: 4,
            resp_purpose: 2,
            service_config: Some(minimal_service_config()),
        });
        let pdu = FdschPdu::decode(&fdsch_pdu(
            MessageId::ServiceResponse,
            srpm.to_sdu().expect("encode f-dsch srpm body"),
        ))
        .expect("decode f-dsch srpm");
        let FdschMessage::ServiceResponse(decoded) = pdu.body else {
            panic!("expected f-dsch SRPM");
        };
        assert_eq!(4, decoded.serv_req_seq);
        assert_eq!(2, decoded.resp_purpose);
        assert!(decoded.service_config.is_some());
    }

    #[test]
    fn test_rdsch_service_request_propose_uses_reverse_service_config_record_type() {
        let raw = super::encode_service_config_record(&minimal_service_config())
            .expect("encode service config");
        let body = service_request_propose_body(0x13, raw.len() as u8, &raw);

        let pdu = RdschPdu::decode(&rdsch_pdu(MessageId::ServiceRequest, body))
            .expect("decode reverse SRQM propose");

        let AccessMessage::ServiceRequest(decoded) = pdu.l3 else {
            panic!("expected reverse SRQM");
        };
        assert_eq!(5, decoded.serv_req_seq);
        assert_eq!(0b0010, decoded.req_purpose);
        assert!(decoded.service_config.is_some());

        let wrong_body = service_request_propose_body(0x07, raw.len() as u8, &raw);
        let err = RdschPdu::decode(&rdsch_pdu(MessageId::ServiceRequest, wrong_body)).unwrap_err();
        assert!(err.contains("RECORD_TYPE=0x13"));
        assert!(err.contains("got 0x07"));
    }

    #[test]
    fn test_fdsch_service_request_propose_uses_forward_service_config_record_type() {
        let raw = super::encode_service_config_record(&minimal_service_config())
            .expect("encode service config");
        let body = service_request_propose_body(0x07, raw.len() as u8, &raw);

        let pdu = FdschPdu::decode(&fdsch_pdu(MessageId::ServiceRequest, body))
            .expect("decode forward SRQM propose");

        let FdschMessage::ServiceRequest(decoded) = pdu.body else {
            panic!("expected forward SRQM");
        };
        assert_eq!(5, decoded.serv_req_seq);
        assert_eq!(0b0010, decoded.req_purpose);
        assert!(decoded.service_config.is_some());

        let wrong_body = service_request_propose_body(0x13, raw.len() as u8, &raw);
        let err = FdschPdu::decode(&fdsch_pdu(MessageId::ServiceRequest, wrong_body)).unwrap_err();
        assert!(err.contains("RECORD_TYPE=0x07"));
        assert!(err.contains("got 0x13"));
    }

    #[test]
    fn test_service_response_counter_propose_uses_channel_record_type() {
        let raw = super::encode_service_config_record(&minimal_service_config())
            .expect("encode service config");

        let reverse = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceResponse,
            service_response_counter_propose_body(0x13, raw.len() as u8, &raw),
        ))
        .expect("decode reverse SRPM counter-propose");
        let AccessMessage::ServiceResponse(decoded) = reverse.l3 else {
            panic!("expected reverse SRPM");
        };
        assert!(decoded.service_config.is_some());

        let err = FdschPdu::decode(&fdsch_pdu(
            MessageId::ServiceResponse,
            service_response_counter_propose_body(0x13, raw.len() as u8, &raw),
        ))
        .unwrap_err();
        assert!(err.contains("RECORD_TYPE=0x07"));
        assert!(err.contains("got 0x13"));
    }

    #[test]
    fn test_service_propose_requires_record_header_and_exact_length() {
        let mut missing_record = Bitstream::new();
        missing_record.write_u8(1, 3); // SERV_REQ_SEQ
        missing_record.write_u8(0b0010, 4); // propose

        let err =
            RdschPdu::decode(&rdsch_pdu(MessageId::ServiceRequest, missing_record)).unwrap_err();
        assert!(err.contains("requires Service Configuration record header"));

        let raw = super::encode_service_config_record(&minimal_service_config())
            .expect("encode service config");
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceRequest,
            service_request_propose_body(0x13, raw.len() as u8 + 1, &raw),
        ))
        .unwrap_err();
        assert!(err.contains("exceeds remaining bits"));

        let mut raw_with_extra_octet = raw.clone();
        raw_with_extra_octet.push(0);
        let err = RdschPdu::decode(&rdsch_pdu(
            MessageId::ServiceRequest,
            service_request_propose_body(
                0x13,
                raw_with_extra_octet.len() as u8,
                &raw_with_extra_octet,
            ),
        ))
        .unwrap_err();
        assert!(err.contains("trailing octets"));
    }

    #[test]
    fn test_fdsch_service_request_rejects_reserved_accept_purpose() {
        let body = AccessMessage::ServiceRequest(ServiceRequestMessage {
            serv_req_seq: 3,
            req_purpose: 0,
            service_config: None,
        })
        .to_sdu()
        .expect("encode reserved f-dsch srqm body");

        let err = FdschPdu::decode(&fdsch_pdu(MessageId::ServiceRequest, body)).unwrap_err();

        assert!(err.contains("REQ_PURPOSE=0b0000 is reserved"));
        assert!(err.contains("3.7.3.3.2.18"));
    }

    #[test]
    fn test_rcsch_unmapped_tag_returns_error() {
        let mut bits = Bitstream::new();
        bits.write_u8(0x0b, 8); // reserved/unmapped C.S0004-E r-csch MSG_TAG
        bits.write_u8(0, 1);

        let err = AccessMessage::decode(&bits).unwrap_err();

        assert!(err.contains("unsupported r-csch MSG_TAG 0x0B"));
    }

    #[test]
    fn test_rcsch_unsupported_sdu_body_returns_error() {
        let header = super::AccessMessageHeader {
            pd: 0,
            message_id: MessageId::FlashWithInfo,
        };
        let mut body = Bitstream::new();
        body.write_u8(0, 1);

        let err = AccessMessage::decode_sdu(header, &body).unwrap_err();

        assert!(err.contains("unsupported r-csch body decode"));
        assert!(err.contains("FWIM"));
    }

    #[test]
    fn test_access_message_decoder_registration_prefix() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Registration), 8);
        bits.write_u8(0b0011, 4);
        bits.write_u8(0b010, 3);
        bits.write_u8(12, 8);
        bits.write_u8(0xa5, 8);
        bits.write_u8(1, 1);
        bits.write_u8(0b0010, 4);

        let msg = AccessMessage::decode(&bits).expect("decode registration");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::Registration(msg) = msg else {
            panic!("expected registration message");
        };
        assert_eq!(0, msg.header.pd);
        assert_eq!(0b0011, msg.reg_type);
        assert_eq!(0b010, msg.slot_cycle_index);
        assert_eq!(12, msg.mob_p_rev);
        assert_eq!(0xa5, msg.scm);
        assert!(msg.mob_term);
        assert_eq!(0b0010, msg.return_cause);
    }

    #[test]
    fn test_access_message_decoder_reverse_common_no_field_messages() {
        for id in [
            MessageId::TmsiAssignmentCompletion,
            MessageId::PacaCancel,
            MessageId::CallRecoveryRequest,
        ] {
            let mut bits = Bitstream::new();
            bits.write_u8(rcsch_wire(id), 8);

            let msg = AccessMessage::decode(&bits).expect("decode no-field message");
            assert_reencodes(&bits, &msg, AccessDecodeContext::default());
            match (id, msg) {
                (
                    MessageId::TmsiAssignmentCompletion,
                    AccessMessage::TmsiAssignmentCompletion(m),
                )
                | (MessageId::PacaCancel, AccessMessage::PacaCancel(m))
                | (MessageId::CallRecoveryRequest, AccessMessage::CallRecoveryRequest(m)) => {
                    assert_eq!(id, m.header.message_id);
                    assert_eq!(0, m.remaining_bits);
                }
                _ => panic!("unexpected decoded no-field message"),
            }
        }
    }

    #[test]
    fn test_access_message_decoder_auth_challenge_response() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::AuthChallengeResponse), 8);
        bits.write_u32(0x2aaaa, 18);

        let msg = AccessMessage::decode(&bits).expect("decode auth challenge response");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::AuthChallengeResponse(msg) = msg else {
            panic!("expected auth challenge response");
        };
        assert_eq!(0x2aaaa, msg.authu);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_auth_response() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::AuthResponse), 8);
        for byte in 0u8..16 {
            bits.write_u8(byte, 8);
        }
        bits.write_u8(1, 1);
        bits.write_u8(0, 8);
        bits.write_u8(0, 3);
        bits.write_u8(0b10, 2);
        bits.write_u32(0x654321, 24);

        let msg = AccessMessage::decode(&bits).expect("decode auth response");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::AuthResponse(msg) = msg else {
            panic!("expected auth response");
        };
        assert_eq!((0u8..16).collect::<Vec<u8>>(), msg.res);
        assert_eq!(Some(0), msg.sig_integrity_sup);
        assert_eq!(Some(0), msg.sig_integrity_req);
        assert_eq!(0b10, msg.new_key_id);
        assert_eq!(0x654321, msg.new_sseq_h);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_auth_resync() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::AuthResync), 8);
        for byte in [1, 2, 3, 4, 5, 6] {
            bits.write_u8(byte, 8);
        }
        for byte in [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7] {
            bits.write_u8(byte, 8);
        }

        let msg = AccessMessage::decode(&bits).expect("decode auth resync");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::AuthResync(msg) = msg else {
            panic!("expected auth resync");
        };
        assert_eq!(vec![1, 2, 3, 4, 5, 6], msg.con_ms_sqn);
        assert_eq!(
            vec![0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7],
            msg.mac_s
        );
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_general_extension_envelope() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::GeneralExtension), 8);
        bits.write_u8(1, 8); // NUM_GE_REC
        bits.write_u8(0, 8); // GE_REC_TYPE
        bits.write_u8(1, 8); // GE_REC_LEN
        bits.write_u8(0xf0, 8); // GE_REC
        bits.write_u8(rcsch_wire(MessageId::Reconnect), 8); // MESSAGE_TYPE
        bits.write_u8(1, 1); // MESSAGE_REC bit 0
        bits.write_u8(0, 1); // MESSAGE_REC bit 1

        let msg = AccessMessage::decode(&bits).expect("decode general extension");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::GeneralExtension(msg) = msg else {
            panic!("expected general extension");
        };
        assert_eq!(1, msg.num_ge_records);
        assert_eq!(0, msg.records[0].record_type);
        assert_eq!(vec![0xf0], msg.records[0].data);
        assert_eq!(rcsch_wire(MessageId::Reconnect), msg.message_type);
        assert_eq!(2, msg.message_record.len());
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_status_response_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::StatusResponse), 8);
        bits.write_u8(0x21, 8);
        bits.write_u8(2, 3);
        bits.write_u8(0xaa, 8);
        bits.write_u8(0xbb, 8);
        bits.write_u8(0x41, 8);
        bits.write_u8(2, 8);
        bits.write_u8(0x11, 8);
        bits.write_u8(0x22, 8);

        let msg = AccessMessage::decode(&bits).expect("decode status response");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::StatusResponse(msg) = msg else {
            panic!("expected status response");
        };
        assert_eq!(0x21, msg.qual_info_type);
        assert_eq!(vec![0xaa, 0xbb], msg.qual_info);
        assert_eq!(1, msg.records.len());
        assert_eq!(0x41, msg.records[0].record_type);
        assert_eq!(vec![0x11, 0x22], msg.records[0].data);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_rejects_status_response_without_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::StatusResponse), 8);
        bits.write_u8(0x21, 8);
        bits.write_u8(0, 3);

        let err = AccessMessage::decode(&bits).expect_err("status response needs record");
        assert!(err.contains("at least one info record"));
    }

    #[test]
    fn test_access_message_decoder_rejects_general_extension_without_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::GeneralExtension), 8);
        bits.write_u8(0, 8);

        let err = AccessMessage::decode(&bits).expect_err("gem needs GE record");
        assert!(err.contains("NUM_GE_REC > 0"));
    }

    #[test]
    fn test_access_message_decoder_extended_status_response_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::ExtStatusResponse), 8);
        bits.write_u8(0x22, 8);
        bits.write_u8(1, 3);
        bits.write_u8(0xee, 8);
        bits.write_u8(1, 4);
        bits.write_u8(0x30, 8);
        bits.write_u8(1, 8);
        bits.write_u8(0x44, 8);

        let msg = AccessMessage::decode(&bits).expect("decode extended status response");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::ExtStatusResponse(msg) = msg else {
            panic!("expected extended status response");
        };
        assert_eq!(0x22, msg.qual_info_type);
        assert_eq!(vec![0xee], msg.qual_info);
        assert_eq!(1, msg.num_info_records);
        assert_eq!(0x30, msg.records[0].record_type);
        assert_eq!(vec![0x44], msg.records[0].data);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_device_information_records() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::DeviceInformation), 8);
        bits.write_u8(0b101, 3);
        bits.write_u8(1, 5);
        bits.write_u8(0x70, 8);
        bits.write_u8(2, 8);
        bits.write_u8(0x12, 8);
        bits.write_u8(0x34, 8);

        let msg = AccessMessage::decode(&bits).expect("decode device information");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::DeviceInformation(msg) = msg else {
            panic!("expected device information");
        };
        assert_eq!(0b101, msg.wll_device_type);
        assert_eq!(1, msg.num_info_records);
        assert_eq!(0x70, msg.records[0].record_type);
        assert_eq!(vec![0x12, 0x34], msg.records[0].data);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_security_mode_request() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::SecurityModeRequest), 8);
        bits.write_u8(1, 1); // UI_ENC_INCL
        bits.write_u8(0b1100_0000, 8); // UI_ENCRYPT_SUP
        bits.write_u8(1, 1); // SIG_ENC_INCL
        bits.write_u8(0b1000_0000, 8); // SIG_ENCRYPT_SUP
        bits.write_u8(1, 1); // C_SIG_ENCRYPT_REQ
        bits.write_u8(1, 1); // NEW_SSEQ_H_INCL
        bits.write_u32(0x123456, 24);
        bits.write_u8(0x7b, 8);
        bits.write_u8(1, 1); // MSG_INT_INFO_INCL
        bits.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
        bits.write_u8(0, 8);
        bits.write_u8(0, 3);

        let msg = AccessMessage::decode(&bits).expect("decode security mode request");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::SecurityModeRequest(msg) = msg else {
            panic!("expected security mode request");
        };
        assert_eq!(Some(0b1100_0000), msg.ui_encrypt_sup);
        assert_eq!(Some(0b1000_0000), msg.sig_encrypt_sup);
        assert_eq!(Some(true), msg.c_sig_encrypt_req);
        assert_eq!(Some(0x123456), msg.new_sseq_h);
        assert_eq!(Some(0x7b), msg.new_sseq_h_sig);
        assert_eq!(Some(true), msg.msg_int_info_incl);
        assert_eq!(Some(true), msg.sig_integrity_sup_incl);
        assert_eq!(Some(0), msg.sig_integrity_sup);
        assert_eq!(Some(0), msg.sig_integrity_req);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_security_mode_request_default_integrity_roundtrip() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::SecurityModeRequest), 8);
        bits.write_u8(0, 1); // UI_ENC_INCL
        bits.write_u8(0, 1); // SIG_ENC_INCL
        bits.write_u8(0, 1); // NEW_SSEQ_H_INCL
        bits.write_u8(1, 1); // MSG_INT_INFO_INCL
        bits.write_u8(0, 1); // SIG_INTEGRITY_SUP_INCL

        let msg = AccessMessage::decode(&bits).expect("decode security mode request");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::SecurityModeRequest(msg) = msg else {
            panic!("expected security mode request");
        };
        assert_eq!(Some(true), msg.msg_int_info_incl);
        assert_eq!(Some(false), msg.sig_integrity_sup_incl);
        assert_eq!(None, msg.sig_integrity_sup);
        assert_eq!(None, msg.sig_integrity_req);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_reconnect_origination_minimal() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Reconnect), 8);
        bits.write_u8(1, 1); // ORIG_IND
        bits.write_u8(0, 1); // SYNC_ID_INCL
        bits.write_u32(33, 16); // SERVICE_OPTION
        bits.write_u8(0b010, 3); // SR_ID

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(0), Some(6)))
                .expect("decode reconnect");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(0), Some(6)));
        let AccessMessage::Reconnect(msg) = msg else {
            panic!("expected reconnect");
        };
        assert!(msg.orig_ind);
        assert!(!msg.sync_id_incl);
        assert_eq!(Some(33), msg.service_option);
        assert_eq!(Some(0b010), msg.sr_id);
        assert_eq!(None, msg.sdb_incl);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_reconnect_page_response_p_rev11_sdb() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Reconnect), 8);
        bits.write_u8(0, 1); // ORIG_IND
        bits.write_u8(1, 1); // SYNC_ID_INCL
        bits.write_u8(2, 4); // SYNC_ID_LEN
        bits.write_u8(0xab, 8);
        bits.write_u8(0xcd, 8);
        bits.write_u8(1, 1); // SDB_INCL
        bits.write_u8(2, 8); // NUM_FIELDS
        bits.write_u8(0x11, 8);
        bits.write_u8(0x22, 8);

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(0), Some(11)))
                .expect("decode reconnect p_rev11");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(0), Some(11)));
        let AccessMessage::Reconnect(msg) = msg else {
            panic!("expected reconnect");
        };
        assert!(!msg.orig_ind);
        assert!(msg.sync_id_incl);
        assert_eq!(Some(2), msg.sync_id_len);
        assert_eq!(vec![0xab, 0xcd], msg.sync_id);
        assert_eq!(None, msg.service_option);
        assert_eq!(None, msg.sr_id);
        assert_eq!(Some(true), msg.sdb_incl);
        assert_eq!(vec![0x11, 0x22], msg.sdb_fields);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_radio_environment() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::RadioEnvironment), 8);
        bits.write_u8(1, 1);
        bits.write_u8(0, 1);

        let msg = AccessMessage::decode(&bits).expect("decode radio environment");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::RadioEnvironment(msg) = msg else {
            panic!("expected radio environment");
        };
        assert!(msg.mode_disabled);
        assert!(!msg.tkz_mode_ind);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_origination_p_rev6_complete() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Origination), 8);
        bits.write_u8(1, 1);
        bits.write_u8(0b011, 3);
        bits.write_u8(6, 8);
        bits.write_u8(0x5a, 8);
        bits.write_u8(0b000, 3);
        bits.write_u8(1, 1);
        bits.write_u32(1, 16);
        bits.write_u8(0, 1);
        bits.write_u8(0, 1);
        bits.write_u8(0, 1);
        bits.write_u8(3, 8);
        bits.write_u8(1, 4);
        bits.write_u8(2, 4);
        bits.write_u8(3, 4);
        bits.write_u8(1, 1);
        bits.write_u8(0, 1);
        bits.write_u8(0, 4);
        bits.write_u8(0, 1);
        bits.write_u8(0b0001, 4); // ENCRYPTION_SUPPORTED
        bits.write_u8(1, 1); // PACA_SUPPORTED
        bits.write_u8(1, 3); // NUM_ALT_SO
        bits.write_u32(0x22, 16); // ALT_SO[0]
        bits.write_u8(1, 1); // DRS
        bits.write_u8(1, 1); // UZID_INCL
        bits.write_u32(0x1234, 16); // UZID
        bits.write_u8(0b11, 2); // CH_IND
        bits.write_u8(0b011, 3); // SR_ID
        bits.write_u8(1, 1); // OTD_SUPPORTED
        bits.write_u8(1, 1); // QPCH_SUPPORTED
        bits.write_u8(1, 1); // ENHANCED_RC
        bits.write_u8(0b00101, 5); // FOR_RC_PREF
        bits.write_u8(0b00011, 5); // REV_RC_PREF
        bits.write_u8(1, 1); // FCH_SUPPORTED
        bits.write_u8(1, 1); // FCH_FRAME_SIZE
        bits.write_u8(1, 3); // FOR_FCH_LEN
        bits.write_u8(0b101, 3); // FOR_FCH_RC_MAP
        bits.write_u8(1, 3); // REV_FCH_LEN
        bits.write_u8(0b110, 3); // REV_FCH_RC_MAP
        bits.write_u8(1, 1); // DCCH_SUPPORTED
        bits.write_u8(0b01, 2); // DCCH_FRAME_SIZE
        bits.write_u8(1, 3); // FOR_DCCH_LEN
        bits.write_u8(0b010, 3); // FOR_DCCH_RC_MAP
        bits.write_u8(1, 3); // REV_DCCH_LEN
        bits.write_u8(0b001, 3); // REV_DCCH_RC_MAP
        bits.write_u8(0, 1); // GEO_LOC_INCL
        bits.write_u8(1, 1); // REV_FCH_GATING_REQ

        let msg = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::Origination(msg) = msg else {
            panic!("expected origination message");
        };
        assert!(msg.mob_term);
        assert_eq!(3, msg.slot_cycle_index);
        assert_eq!(6, msg.mob_p_rev);
        assert_eq!(0x5a, msg.scm);
        assert_eq!(0, msg.request_mode);
        assert!(msg.special_service);
        assert_eq!(Some(1), msg.service_option);
        assert!(!msg.pm);
        assert!(!msg.digit_mode);
        assert_eq!(None, msg.number_type);
        assert!(!msg.more_fields);
        assert_eq!(vec![1, 2, 3], msg.digits);
        assert!(msg.nar_an_cap);
        assert!(!msg.paca_reorig);
        assert_eq!(0, msg.return_cause);
        assert!(!msg.more_records);
        assert_eq!(Some(0b0001), msg.encryption_supported);
        assert!(msg.paca_supported);
        assert_eq!(1, msg.num_alt_so);
        assert_eq!(vec![0x22], msg.alt_service_options);
        assert_eq!(Some(true), msg.drs);
        assert_eq!(Some(true), msg.uzid_incl);
        assert_eq!(Some(0x1234), msg.uzid);
        assert_eq!(Some(0b11), msg.ch_ind);
        assert_eq!(Some(0b011), msg.sr_id);
        assert_eq!(Some(true), msg.otd_supported);
        assert_eq!(Some(true), msg.qpch_supported);
        assert_eq!(Some(true), msg.enhanced_rc);
        assert_eq!(Some(0b00101), msg.for_rc_pref);
        assert_eq!(Some(0b00011), msg.rev_rc_pref);
        assert_eq!(Some(true), msg.fch_supported);
        assert_eq!(Some(true), msg.dcch_supported);
        assert_eq!(Some(false), msg.geo_loc_incl);
        assert_eq!(Some(true), msg.rev_fch_gating_req);
        assert_eq!(0, msg.remaining_bits);
        let fch = msg.fch_capability.as_ref().expect("fch capability");
        assert!(fch.frame_size_5ms_supported);
        assert_eq!(vec![1, 3], fch.for_supported_rcs);
        assert_eq!(vec![1, 2], fch.rev_supported_rcs);
        let dcch = msg.dcch_capability.as_ref().expect("dcch capability");
        assert_eq!(0b01, dcch.frame_size_mode);
        assert_eq!(vec![2], dcch.for_supported_rcs);
        assert_eq!(vec![3], dcch.rev_supported_rcs);
    }

    #[test]
    fn test_origination_p_rev7_optional_fields_roundtrip() {
        let msg = AccessMessage::Origination(OriginationMessage {
            header: AccessMessageHeader {
                pd: 1,
                message_id: MessageId::Origination,
            },
            mob_term: true,
            slot_cycle_index: 2,
            mob_p_rev: 7,
            scm: 0x2a,
            request_mode: 1,
            special_service: true,
            service_option: Some(33),
            pm: false,
            digit_mode: false,
            number_type: None,
            number_plan: None,
            more_fields: false,
            num_fields: 0,
            digits: Vec::new(),
            nar_an_cap: false,
            paca_reorig: false,
            return_cause: 0,
            more_records: false,
            encryption_supported: None,
            paca_supported: true,
            num_alt_so: 1,
            alt_service_options: vec![7],
            drs: Some(true),
            uzid_incl: Some(true),
            uzid: Some(0x1234),
            ch_ind: Some(0b01),
            sr_id: Some(3),
            otd_supported: Some(true),
            qpch_supported: Some(false),
            enhanced_rc: Some(true),
            for_rc_pref: Some(3),
            rev_rc_pref: Some(3),
            fch_supported: Some(false),
            fch_capability: None,
            dcch_supported: Some(false),
            dcch_capability: None,
            geo_loc_incl: Some(true),
            geo_loc_type: Some(0b101),
            rev_fch_gating_req: Some(true),
            orig_reason: Some(false),
            orig_count: Some(0b10),
            sts_supported: Some(true),
            cch_3x_supported: Some(false),
            wll_incl: Some(true),
            wll_device_type: Some(0b101),
            global_emergency_call: Some(true),
            ms_init_pos_loc_ind: Some(true),
            qos_parms_incl: Some(true),
            qos_parms_len: Some(2),
            qos_parms: vec![0xa5, 0x00],
            enc_info_incl: Some(true),
            sig_encrypt_sup: Some(0b1100_0000),
            d_sig_encrypt_req: Some(true),
            c_sig_encrypt_req: Some(false),
            new_sseq_h: Some(0x00ab_cdef),
            new_sseq_h_sig: Some(0x5a),
            ui_encrypt_req: Some(true),
            ui_encrypt_sup: Some(0b1000_0000),
            sync_id_incl: Some(true),
            sync_id_len: Some(2),
            sync_id: Some(0xbeef),
            prev_sid_incl: Some(true),
            prev_sid: Some(0x1234),
            prev_nid_incl: Some(true),
            prev_nid: Some(0x4567),
            prev_pzid_incl: Some(true),
            prev_pzid: Some(0x89),
            so_bitmap_ind: Some(0b10),
            so_group_num: Some(0x12),
            so_bitmap: Some(0x00a5),
            sdb_desired_only: None,
            alt_band_class_sup: None,
            msg_int_info_incl: None,
            sig_integrity_sup_incl: None,
            sig_integrity_sup: None,
            sig_integrity_req: None,
            new_key_id: None,
            new_sseq_h_incl: None,
            for_pdch_supported: None,
            for_pdch_capability: None,
            ext_ch_ind: None,
            sign_slot_cycle_index: None,
            add_serv_instance_incl: None,
            add_service_instances: Vec::new(),
            bcmc_incl: None,
            bcmc: None,
            rev_pdch_supported: None,
            rev_pdch_capability: None,
            band_sub_rep_incl: None,
            num_band_subclass: None,
            band_subclass_sup: Vec::new(),
            add_geo_loc_incl: None,
            add_geo_loc_type_len_ind: None,
            add_geo_loc_type: None,
            remaining_bits: 0,
        });

        let mut missing_cmea = msg.clone();
        if let AccessMessage::Origination(orig) = &mut missing_cmea {
            orig.sig_encrypt_sup = Some(0b0100_0000);
        }
        let err = missing_cmea
            .to_reverse_common_pdu()
            .expect_err("reject SIG_ENCRYPT_SUP with CMEA=0");
        assert!(err.contains("SIG_ENCRYPT_SUP CMEA"));

        let mut nonzero_sig_reserved = msg.clone();
        if let AccessMessage::Origination(orig) = &mut nonzero_sig_reserved {
            orig.sig_encrypt_sup = Some(0b1100_0001);
        }
        let err = nonzero_sig_reserved
            .to_reverse_common_pdu()
            .expect_err("reject SIG_ENCRYPT_SUP reserved bits");
        assert!(err.contains("SIG_ENCRYPT_SUP RESERVED"));

        let mut nonzero_ui_reserved = msg.clone();
        if let AccessMessage::Origination(orig) = &mut nonzero_ui_reserved {
            orig.ui_encrypt_sup = Some(0b1000_0001);
        }
        let err = nonzero_ui_reserved
            .to_reverse_common_pdu()
            .expect_err("reject UI_ENCRYPT_SUP reserved bits");
        assert!(err.contains("UI_ENCRYPT_SUP RESERVED"));

        let bits = msg.to_reverse_common_pdu().expect("encode origination");
        let decoded = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &decoded, AccessDecodeContext::default());
        let AccessMessage::Origination(orig) = decoded else {
            panic!("expected origination");
        };

        assert_eq!(Some(true), orig.sts_supported);
        assert_eq!(Some(false), orig.cch_3x_supported);
        assert_eq!(Some(true), orig.wll_incl);
        assert_eq!(Some(0b101), orig.wll_device_type);
        assert_eq!(Some(true), orig.global_emergency_call);
        assert_eq!(Some(true), orig.ms_init_pos_loc_ind);
        assert_eq!(Some(true), orig.qos_parms_incl);
        assert_eq!(Some(2), orig.qos_parms_len);
        assert_eq!(vec![0xa5, 0x00], orig.qos_parms);
        assert_eq!(Some(true), orig.enc_info_incl);
        assert_eq!(Some(0b1100_0000), orig.sig_encrypt_sup);
        assert_eq!(Some(true), orig.d_sig_encrypt_req);
        assert_eq!(Some(false), orig.c_sig_encrypt_req);
        assert_eq!(Some(0x00ab_cdef), orig.new_sseq_h);
        assert_eq!(Some(0x5a), orig.new_sseq_h_sig);
        assert_eq!(Some(true), orig.ui_encrypt_req);
        assert_eq!(Some(0b1000_0000), orig.ui_encrypt_sup);
        assert_eq!(Some(true), orig.sync_id_incl);
        assert_eq!(Some(2), orig.sync_id_len);
        assert_eq!(Some(0xbeef), orig.sync_id);
        assert_eq!(Some(true), orig.prev_sid_incl);
        assert_eq!(Some(0x1234), orig.prev_sid);
        assert_eq!(Some(true), orig.prev_nid_incl);
        assert_eq!(Some(0x4567), orig.prev_nid);
        assert_eq!(Some(true), orig.prev_pzid_incl);
        assert_eq!(Some(0x89), orig.prev_pzid);
        assert_eq!(Some(0b10), orig.so_bitmap_ind);
        assert_eq!(Some(0x12), orig.so_group_num);
        assert_eq!(Some(0x00a5), orig.so_bitmap);
        assert_eq!(0, orig.remaining_bits);
    }

    #[test]
    fn test_origination_rejects_p_rev7_security_reserved_values_on_decode() {
        for (sig_encrypt_sup, ui_encrypt_sup, expected) in [
            (0b0100_0000, 0b1000_0000, "SIG_ENCRYPT_SUP CMEA"),
            (0b1100_0001, 0b1000_0000, "SIG_ENCRYPT_SUP RESERVED"),
            (0b1100_0000, 0b1000_0001, "UI_ENCRYPT_SUP RESERVED"),
        ] {
            let mut bits = Bitstream::new();
            write_minimal_origination_p_rev7_tail(
                &mut bits,
                7,
                0b01,
                Some((sig_encrypt_sup, ui_encrypt_sup)),
            );

            let err = AccessMessage::decode(&bits).expect_err("reject invalid encryption fields");
            assert!(
                err.contains(expected),
                "expected {expected:?} in error {err:?}"
            );
        }
    }

    #[test]
    fn test_origination_p_rev8_sdb_and_alt_band_fields_roundtrip() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Origination), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(8, 8); // MOB_P_REV
        bits.write_u8(0x2a, 8); // SCM
        bits.write_u8(0b001, 3); // REQUEST_MODE
        bits.write_u8(0, 1); // SPECIAL_SERVICE
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // DIGIT_MODE
        bits.write_u8(0, 1); // MORE_FIELDS
        bits.write_u8(0, 8); // NUM_FIELDS
        bits.write_u8(0, 1); // NAR_AN_CAP
        bits.write_u8(0, 1); // PACA_REORIG
        bits.write_u8(0, 4); // RETURN_CAUSE
        bits.write_u8(0, 1); // MORE_RECORDS
        bits.write_u8(1, 1); // PACA_SUPPORTED
        bits.write_u8(0, 3); // NUM_ALT_SO
        bits.write_u8(0, 1); // DRS
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(0b01, 2); // CH_IND
        bits.write_u8(0, 3); // SR_ID
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(0, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(1, 5); // FOR_RC_PREF
        bits.write_u8(1, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // GEO_LOC_INCL
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        bits.write_u8(0, 1); // ORIG_REASON
        bits.write_u8(0, 2); // ORIG_COUNT
        bits.write_u8(0, 1); // STS_SUPPORTED
        bits.write_u8(0, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(0, 1); // WLL_INCL
        bits.write_u8(0, 1); // GLOBAL_EMERGENCY_CALL
        bits.write_u8(0, 1); // QOS_PARMS_INCL
        bits.write_u8(0, 1); // ENC_INFO_INCL
        bits.write_u8(0, 1); // SYNC_ID_INCL
        bits.write_u8(0, 1); // PREV_SID_INCL
        bits.write_u8(0, 1); // PREV_NID_INCL
        bits.write_u8(0, 1); // PREV_PZID_INCL
        bits.write_u8(0, 2); // SO_BITMAP_IND
        bits.write_u8(1, 1); // SDB_DESIRED_ONLY
        bits.write_u8(1, 1); // ALT_BAND_CLASS_SUP

        let msg = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::Origination(orig) = msg else {
            panic!("expected origination");
        };
        assert_eq!(Some(true), orig.sdb_desired_only);
        assert_eq!(Some(true), orig.alt_band_class_sup);
        assert_eq!(0, orig.remaining_bits);
    }

    #[test]
    fn test_origination_p_rev9_integrity_pdch_and_ext_channel_roundtrip() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Origination), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(9, 8); // MOB_P_REV
        bits.write_u8(0x2a, 8); // SCM
        bits.write_u8(0b001, 3); // REQUEST_MODE
        bits.write_u8(0, 1); // SPECIAL_SERVICE
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // DIGIT_MODE
        bits.write_u8(0, 1); // MORE_FIELDS
        bits.write_u8(0, 8); // NUM_FIELDS
        bits.write_u8(0, 1); // NAR_AN_CAP
        bits.write_u8(0, 1); // PACA_REORIG
        bits.write_u8(0, 4); // RETURN_CAUSE
        bits.write_u8(0, 1); // MORE_RECORDS
        bits.write_u8(1, 1); // PACA_SUPPORTED
        bits.write_u8(0, 3); // NUM_ALT_SO
        bits.write_u8(0, 1); // DRS
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(0b00, 2); // CH_IND, so EXT_CH_IND is present
        bits.write_u8(0, 3); // SR_ID
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(0, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(1, 5); // FOR_RC_PREF
        bits.write_u8(1, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // GEO_LOC_INCL
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        bits.write_u8(0, 1); // ORIG_REASON
        bits.write_u8(0, 2); // ORIG_COUNT
        bits.write_u8(0, 1); // STS_SUPPORTED
        bits.write_u8(0, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(0, 1); // WLL_INCL
        bits.write_u8(0, 1); // GLOBAL_EMERGENCY_CALL
        bits.write_u8(0, 1); // QOS_PARMS_INCL
        bits.write_u8(0, 1); // ENC_INFO_INCL
        bits.write_u8(0, 1); // SYNC_ID_INCL
        bits.write_u8(0, 1); // PREV_SID_INCL
        bits.write_u8(0, 1); // PREV_NID_INCL
        bits.write_u8(0, 1); // PREV_PZID_INCL
        bits.write_u8(0, 2); // SO_BITMAP_IND
        bits.write_u8(0, 1); // SDB_DESIRED_ONLY
        bits.write_u8(0, 1); // ALT_BAND_CLASS_SUP
        bits.write_u8(1, 1); // MSG_INT_INFO_INCL
        bits.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
        bits.write_u8(0, 8); // SIG_INTEGRITY_SUP
        bits.write_u8(0, 3); // SIG_INTEGRITY_REQ
        bits.write_u8(0b01, 2); // NEW_KEY_ID
        bits.write_u8(1, 1); // NEW_SSEQ_H_INCL
        bits.write_u32(0x123456, 24); // NEW_SSEQ_H
        bits.write_u8(0x78, 8); // NEW_SSEQ_H_SIG
        bits.write_u8(1, 1); // FOR_PDCH_SUPPORTED
        bits.write_u8(1, 1); // ACK_DELAY
        bits.write_u8(0b01, 2); // NUM_ARQ_CHAN
        bits.write_u8(0, 2); // FOR_PDCH_LEN
        bits.write_u8(0b100, 3); // FOR_PDCH_RC_MAP: RC10 plus reserved zeroes
        bits.write_u8(1, 2); // CH_CONFIG_SUP_MAP_LEN
        bits.write_u8(0b101000, 6); // F-PDCH_1/F-PDCH_3 supported
        bits.write_u8(0b00001, 5); // EXT_CH_IND

        let msg = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let mut invalid_sup = msg.clone();
        if let AccessMessage::Origination(orig) = &mut invalid_sup {
            orig.sig_integrity_sup = Some(0x80);
        }
        let err = invalid_sup
            .to_reverse_common_pdu()
            .expect_err("reject SIG_INTEGRITY_SUP reserved bits");
        assert!(err.contains("SIG_INTEGRITY_SUP RESERVED"));

        let mut invalid_req = msg.clone();
        if let AccessMessage::Origination(orig) = &mut invalid_req {
            orig.sig_integrity_req = Some(0b001);
        }
        let err = invalid_req
            .to_reverse_common_pdu()
            .expect_err("reject SIG_INTEGRITY_REQ reserved value");
        assert!(err.contains("SIG_INTEGRITY_REQ"));

        let AccessMessage::Origination(orig) = msg else {
            panic!("expected origination");
        };
        assert_eq!(Some(true), orig.msg_int_info_incl);
        assert_eq!(Some(true), orig.sig_integrity_sup_incl);
        assert_eq!(Some(0), orig.sig_integrity_sup);
        assert_eq!(Some(0), orig.sig_integrity_req);
        assert_eq!(Some(0b01), orig.new_key_id);
        assert_eq!(Some(true), orig.new_sseq_h_incl);
        assert_eq!(Some(0x123456), orig.new_sseq_h);
        assert_eq!(Some(0x78), orig.new_sseq_h_sig);
        assert_eq!(Some(true), orig.for_pdch_supported);
        let cap = orig.for_pdch_capability.expect("for-pdch capability");
        assert!(cap.ack_delay);
        assert_eq!(0b01, cap.num_arq_chan);
        assert_eq!(vec![10], cap.for_pdch_supported_rcs);
        assert_eq!(vec![1, 3], cap.ch_config_supported);
        assert_eq!(Some(0b00001), orig.ext_ch_ind);
        assert_eq!(0, orig.remaining_bits);
    }

    #[test]
    fn test_origination_p_rev11_service_rev_pdch_and_band_fields_roundtrip() {
        let msg = AccessMessage::Origination(OriginationMessage {
            header: AccessMessageHeader {
                pd: 1,
                message_id: MessageId::Origination,
            },
            mob_term: true,
            slot_cycle_index: 2,
            mob_p_rev: 11,
            scm: 0x2a,
            request_mode: 1,
            special_service: true,
            service_option: Some(33),
            pm: false,
            digit_mode: false,
            number_type: Some(0),
            number_plan: None,
            more_fields: false,
            num_fields: 0,
            digits: Vec::new(),
            nar_an_cap: false,
            paca_reorig: false,
            return_cause: 0,
            more_records: false,
            encryption_supported: None,
            paca_supported: true,
            num_alt_so: 0,
            alt_service_options: Vec::new(),
            drs: Some(true),
            uzid_incl: Some(false),
            uzid: None,
            ch_ind: Some(0),
            sr_id: Some(3),
            otd_supported: Some(false),
            qpch_supported: Some(false),
            enhanced_rc: Some(true),
            for_rc_pref: Some(3),
            rev_rc_pref: Some(3),
            fch_supported: Some(false),
            fch_capability: None,
            dcch_supported: Some(false),
            dcch_capability: None,
            geo_loc_incl: Some(false),
            geo_loc_type: None,
            rev_fch_gating_req: Some(false),
            orig_reason: Some(false),
            orig_count: Some(0),
            sts_supported: Some(false),
            cch_3x_supported: Some(false),
            wll_incl: Some(false),
            wll_device_type: None,
            global_emergency_call: Some(false),
            ms_init_pos_loc_ind: None,
            qos_parms_incl: Some(false),
            qos_parms_len: None,
            qos_parms: Vec::new(),
            enc_info_incl: Some(false),
            sig_encrypt_sup: None,
            d_sig_encrypt_req: None,
            c_sig_encrypt_req: None,
            new_sseq_h: None,
            new_sseq_h_sig: None,
            ui_encrypt_req: None,
            ui_encrypt_sup: None,
            sync_id_incl: Some(false),
            sync_id_len: None,
            sync_id: None,
            prev_sid_incl: Some(false),
            prev_sid: None,
            prev_nid_incl: Some(false),
            prev_nid: None,
            prev_pzid_incl: Some(false),
            prev_pzid: None,
            so_bitmap_ind: Some(0),
            so_group_num: None,
            so_bitmap: None,
            sdb_desired_only: Some(false),
            alt_band_class_sup: Some(false),
            msg_int_info_incl: Some(false),
            sig_integrity_sup_incl: None,
            sig_integrity_sup: None,
            sig_integrity_req: None,
            new_key_id: None,
            new_sseq_h_incl: None,
            for_pdch_supported: Some(true),
            for_pdch_capability: Some(ForPdchTypeSpecificFields {
                ack_delay: true,
                num_arq_chan: 1,
                for_pdch_len: 0,
                for_pdch_rc_map_raw: Bitstream::new_init(&[1, 0, 0]),
                for_pdch_supported_rcs: vec![10],
                ch_config_sup_map_len: 1,
                ch_config_sup_map_raw: Bitstream::new_init(&[1, 0, 1, 0, 0, 0]),
                ch_config_supported: vec![1, 3],
            }),
            ext_ch_ind: Some(0b01001),
            sign_slot_cycle_index: Some(true),
            add_serv_instance_incl: Some(true),
            add_service_instances: vec![OriginationAdditionalServiceInstance {
                add_sr_id: 4,
                add_drs: true,
                add_service_option_incl: Some(true),
                add_service_option: Some(7),
                add_qos_parms_incl: Some(true),
                add_qos_parms_len: Some(1),
                add_qos_parms: vec![0xa5],
            }],
            bcmc_incl: Some(false),
            bcmc: None,
            rev_pdch_supported: Some(true),
            rev_pdch_capability: Some(RevPdchTypeSpecificFields {
                rev_pdch_len: 0,
                rev_pdch_rc_map_raw: Bitstream::new_init(&[1, 0, 0]),
                rev_pdch_supported_rcs: vec![7],
                rev_pdch_ch_config_sup_map_len: 2,
                rev_pdch_ch_config_sup_map_raw: Bitstream::new_init(&[1, 1, 0, 1, 0, 0, 0, 0, 0]),
                rev_pdch_ch_config_supported: vec![0, 1, 3],
                rev_pdch_max_size_supported_encoder_packet: 0b10,
            }),
            band_sub_rep_incl: Some(true),
            num_band_subclass: Some(3),
            band_subclass_sup: vec![1, 0, 1],
            add_geo_loc_incl: None,
            add_geo_loc_type_len_ind: None,
            add_geo_loc_type: None,
            remaining_bits: 0,
        });

        let bits = msg.to_reverse_common_pdu().expect("encode origination");
        let decoded = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &decoded, AccessDecodeContext::default());
        let AccessMessage::Origination(orig) = decoded else {
            panic!("expected origination");
        };
        assert_eq!(Some(true), orig.sign_slot_cycle_index);
        assert_eq!(Some(true), orig.add_serv_instance_incl);
        assert_eq!(1, orig.add_service_instances.len());
        assert_eq!(Some(false), orig.bcmc_incl);
        assert_eq!(Some(true), orig.rev_pdch_supported);
        let rev = orig.rev_pdch_capability.expect("rev-pdch capability");
        assert_eq!(vec![7], rev.rev_pdch_supported_rcs);
        assert_eq!(vec![0, 1, 3], rev.rev_pdch_ch_config_supported);
        assert_eq!(0b10, rev.rev_pdch_max_size_supported_encoder_packet);
        assert_eq!(Some(true), orig.band_sub_rep_incl);
        assert_eq!(Some(3), orig.num_band_subclass);
        assert_eq!(vec![1, 0, 1], orig.band_subclass_sup);
        assert_eq!(0, orig.remaining_bits);
    }

    #[test]
    fn test_origination_p_rev11_bcmc_fields_roundtrip() {
        let mut bits = Bitstream::new();
        write_minimal_origination_p_rev7_tail(&mut bits, 11, 0b01, None);
        bits.write_u8(0, 1); // MSG_INT_INFO_INCL
        bits.write_u8(0, 1); // FOR_PDCH_SUPPORTED
        bits.write_u8(0, 1); // ADD_SERV_INSTANCE_INCL
        bits.write_u8(1, 1); // BCMC_INCL
        bits.write_u8(1, 1); // BCMC_ORIG_ONLY_IND
        bits.write_u8(1, 1); // FUNDICATED_BCMC_SUPPORTED
        bits.write_u8(1, 2); // FUNDICATED_BCMC_CH_SUP_MAP_LEN: six bits
        bits.write_u8(0b100000, 6); // config 1 supported, reserved bit zero
        bits.write_u8(1, 1); // AUTH_SIGNATURE_INCL
        bits.write_u8(4, 8); // TIME_STAMP_SHORT_LENGTH
        bits.write_u8(0b1010, 4); // TIME_STAMP_SHORT
        bits.write_u8(0, 3); // NUM_BCMC_PROGRAMS: one program
        bits.write_u8(3, 5); // BCMC_PROGRAM_ID_LEN: four bits
        bits.write_u8(0b1011, 4); // BCMC_PROGRAM_ID
        bits.write_u8(2, 3); // BCMC_FLOW_DISCRIMINATOR_LEN
        bits.write_u8(1, 2); // NUM_FLOW_DISCRIMINATOR: two flows
        bits.write_u8(0b01, 2); // BCMC_FLOW_DISCRIMINATOR[0]
        bits.write_u8(1, 1); // AUTH_SIGNATURE_IND[0]
        bits.write_u8(0, 1); // AUTH_SIGNATURE_SAME_IND[0]
        bits.write_u8(0x0a, 4); // BAK_ID[0]
        bits.write_u32(0x1234_5678, 32); // AUTH_SIGNATURE[0]
        bits.write_u8(0b10, 2); // BCMC_FLOW_DISCRIMINATOR[1]
        bits.write_u8(1, 1); // AUTH_SIGNATURE_IND[1]
        bits.write_u8(1, 1); // AUTH_SIGNATURE_SAME_IND[1]
        bits.write_u8(0, 1); // BAND_SUB_REP_INCL

        let msg = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let mut invalid_fundicated_reserved = msg.clone();
        if let AccessMessage::Origination(orig) = &mut invalid_fundicated_reserved {
            let bcmc = orig.bcmc.as_mut().expect("bcmc fields");
            let cap = bcmc
                .fundicated_bcmc_capability
                .as_mut()
                .expect("fundicated capability");
            cap.fundicated_bcmc_ch_sup_map_raw = Bitstream::new_init(&[1, 0, 0, 0, 0, 1]);
        }
        let err = invalid_fundicated_reserved
            .to_reverse_common_pdu()
            .expect_err("reject fundicated BCMC reserved bit");
        assert!(err.contains("FUNDICATED_BCMC_CH_SUP_MAP reserved bits"));

        let AccessMessage::Origination(orig) = msg else {
            panic!("expected origination");
        };
        assert_eq!(Some(true), orig.bcmc_incl);
        let bcmc = orig.bcmc.expect("bcmc fields");
        assert!(bcmc.bcmc_orig_only_ind);
        assert!(bcmc.fundicated_bcmc_supported);
        assert!(bcmc.auth_signature_incl);
        assert_eq!(Some(4), bcmc.time_stamp_short_length);
        assert_eq!(&[1, 0, 1, 0], bcmc.time_stamp_short.bits());
        assert_eq!(0, bcmc.num_bcmc_programs);
        let cap = bcmc
            .fundicated_bcmc_capability
            .expect("fundicated capability");
        assert_eq!(1, cap.fundicated_bcmc_ch_sup_map_len);
        assert_eq!(vec![1], cap.supported_configurations);
        assert_eq!(1, bcmc.programs.len());
        let program = &bcmc.programs[0];
        assert_eq!(3, program.bcmc_program_id_len);
        assert_eq!(&[1, 0, 1, 1], program.bcmc_program_id.bits());
        assert_eq!(2, program.bcmc_flow_discriminator_len);
        assert_eq!(Some(1), program.num_flow_discriminator);
        assert_eq!(2, program.flows.len());
        assert_eq!(&[0, 1], program.flows[0].bcmc_flow_discriminator.bits());
        assert_eq!(Some(true), program.flows[0].auth_signature_ind);
        assert_eq!(Some(false), program.flows[0].auth_signature_same_ind);
        assert_eq!(Some(0x0a), program.flows[0].bak_id);
        assert_eq!(Some(0x1234_5678), program.flows[0].auth_signature);
        assert_eq!(&[1, 0], program.flows[1].bcmc_flow_discriminator.bits());
        assert_eq!(Some(true), program.flows[1].auth_signature_ind);
        assert_eq!(Some(true), program.flows[1].auth_signature_same_ind);
        assert_eq!(Some(false), orig.band_sub_rep_incl);
        assert_eq!(0, orig.remaining_bits);
    }

    #[test]
    fn test_origination_p_rev12_additional_geo_location_roundtrip() {
        let mut bits = Bitstream::new();
        write_minimal_origination_p_rev7_tail(&mut bits, 12, 0b01, None);
        bits.write_u8(0, 1); // MSG_INT_INFO_INCL
        bits.write_u8(0, 1); // FOR_PDCH_SUPPORTED
        bits.write_u8(0, 1); // ADD_SERV_INSTANCE_INCL
        bits.write_u8(0, 1); // BCMC_INCL
        bits.write_u8(0, 1); // BAND_SUB_REP_INCL
        bits.write_u8(1, 1); // ADD_GEO_LOC_INCL
        bits.write_u8(1, 1); // ADD_GEO_LOC_TYPE_LEN_IND
        bits.write_u32(0x01_abcd, 24); // ADD_GEO_LOC_TYPE

        let msg = AccessMessage::decode(&bits).expect("decode origination");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let mut invalid_16_bit_len = msg.clone();
        if let AccessMessage::Origination(orig) = &mut invalid_16_bit_len {
            orig.add_geo_loc_type_len_ind = Some(false);
        }
        let err = invalid_16_bit_len
            .to_reverse_common_pdu()
            .expect_err("reject 24-bit geo type with 16-bit length");
        assert!(err.contains("ADD_GEO_LOC_TYPE does not fit 16-bit length"));

        let AccessMessage::Origination(orig) = msg else {
            panic!("expected origination");
        };
        assert_eq!(Some(0), orig.number_type);
        assert_eq!(Some(false), orig.msg_int_info_incl);
        assert_eq!(Some(false), orig.for_pdch_supported);
        assert_eq!(Some(false), orig.add_serv_instance_incl);
        assert_eq!(Some(false), orig.bcmc_incl);
        assert_eq!(Some(false), orig.band_sub_rep_incl);
        assert_eq!(Some(true), orig.add_geo_loc_incl);
        assert_eq!(Some(true), orig.add_geo_loc_type_len_ind);
        assert_eq!(Some(0x01_abcd), orig.add_geo_loc_type);
        assert_eq!(0, orig.remaining_bits);
    }

    #[test]
    fn test_origination_rejects_p_rev9_integrity_reserved_values_on_decode() {
        for (sig_integrity_sup, sig_integrity_req, expected) in [
            (0x80, 0, "SIG_INTEGRITY_SUP RESERVED"),
            (0, 0b001, "SIG_INTEGRITY_REQ"),
        ] {
            let mut bits = Bitstream::new();
            write_minimal_origination_p_rev7_tail(&mut bits, 9, 0b01, None);
            bits.write_u8(1, 1); // MSG_INT_INFO_INCL
            bits.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
            bits.write_u8(sig_integrity_sup, 8); // SIG_INTEGRITY_SUP
            bits.write_u8(sig_integrity_req, 3); // SIG_INTEGRITY_REQ
            bits.write_u8(0, 2); // NEW_KEY_ID
            bits.write_u8(0, 1); // NEW_SSEQ_H_INCL
            bits.write_u8(0, 1); // FOR_PDCH_SUPPORTED

            let err = AccessMessage::decode(&bits).expect_err("reject invalid integrity fields");
            assert!(
                err.contains(expected),
                "expected {expected:?} in error {err:?}"
            );
        }
    }

    #[test]
    fn test_access_message_decoder_page_response_p_rev5_prefix() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(0, 1); // MOB_TERM
        bits.write_u8(0b001, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(5, 8); // MOB_P_REV
        bits.write_u8(0x42, 8); // SCM
        bits.write_u8(0b000, 3); // REQUEST_MODE
        bits.write_u32(1, 16); // SERVICE_OPTION
        bits.write_u8(0, 1); // PM
        bits.write_u8(1, 1); // NAR_AN_CAP
        // ENCRYPTION_SUPPORTED omitted: P_REV_IN_USE >= 7 OR AUTH_MODE == 0
        bits.write_u8(2, 3); // NUM_ALT_SO
        bits.write_u32(3, 16); // ALT_SO[0]
        bits.write_u32(4096, 16); // ALT_SO[1]

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(0), Some(5)))
                .expect("decode page response");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(0), Some(5)));
        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response message");
        };
        assert!(!msg.mob_term);
        assert_eq!(1, msg.slot_cycle_index);
        assert_eq!(5, msg.mob_p_rev);
        assert_eq!(0x42, msg.scm);
        assert_eq!(1, msg.service_option);
        assert!(!msg.pm);
        assert!(msg.nar_an_cap);
        assert_eq!(None, msg.encryption_supported);
        assert_eq!(2, msg.num_alt_so);
        assert_eq!(vec![3, 4096], msg.alt_service_options);
        assert_eq!(None, msg.ch_ind);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_page_response_uses_payload_p_rev_for_tail_gating() {
        let mut pdu = Bitstream::new_bytes(&[
            0x05, 0x13, 0x4e, 0xac, 0xf5, 0x7d, 0x61, 0x88, 0x63, 0xc6, 0xa4, 0x3a, 0x44, 0x49,
            0x03, 0x6a, 0x60, 0x00, 0xc0, 0x00,
        ]);
        let sdu = pdu.drain(108..154);
        let header = AccessMessageHeader {
            pd: 0,
            message_id: MessageId::PageResponse,
        };

        let msg = AccessMessage::decode_sdu_with_context(
            header,
            &sdu,
            AccessDecodeContext::new(Some(0), Some(6)),
        )
        .expect("decode IS-95 page response under P_REV 6 cell context");

        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response message");
        };
        assert!(msg.mob_term);
        assert_eq!(1, msg.slot_cycle_index);
        assert_eq!(3, msg.mob_p_rev);
        assert_eq!(0x6a, msg.scm);
        assert_eq!(3, msg.request_mode);
        assert_eq!(6, msg.service_option);
        assert!(!msg.pm);
        assert!(!msg.nar_an_cap);
        assert_eq!(None, msg.encryption_supported);
        assert_eq!(0, msg.num_alt_so);
        assert_eq!(None, msg.ch_ind);
        assert_eq!(2, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_page_response_p_rev6_capabilities() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(1, 1);
        bits.write_u8(0b010, 3);
        bits.write_u8(6, 8);
        bits.write_u8(0x3a, 8);
        bits.write_u8(0b001, 3);
        bits.write_u32(6, 16);
        bits.write_u8(0, 1);
        bits.write_u8(0, 1);
        bits.write_u8(0b0001, 4); // ENCRYPTION_SUPPORTED
        bits.write_u8(1, 3); // NUM_ALT_SO
        bits.write_u32(0x22, 16); // ALT_SO[0]
        bits.write_u8(1, 1); // UZID_INCL
        bits.write_u32(0x1234, 16); // UZID
        bits.write_u8(0b11, 2); // CH_IND
        bits.write_u8(1, 1); // OTD_SUPPORTED
        bits.write_u8(1, 1); // QPCH_SUPPORTED
        bits.write_u8(1, 1); // ENHANCED_RC
        bits.write_u8(0b00101, 5); // FOR_RC_PREF
        bits.write_u8(0b00011, 5); // REV_RC_PREF
        bits.write_u8(1, 1); // FCH_SUPPORTED
        bits.write_u8(1, 1); // FCH_FRAME_SIZE
        bits.write_u8(1, 3); // FOR_FCH_LEN
        bits.write_u8(0b101, 3); // FOR_FCH_RC_MAP
        bits.write_u8(1, 3); // REV_FCH_LEN
        bits.write_u8(0b110, 3); // REV_FCH_RC_MAP
        bits.write_u8(1, 1); // DCCH_SUPPORTED
        bits.write_u8(0b01, 2); // DCCH_FRAME_SIZE
        bits.write_u8(1, 3); // FOR_DCCH_LEN
        bits.write_u8(0b010, 3); // FOR_DCCH_RC_MAP
        bits.write_u8(1, 3); // REV_DCCH_LEN
        bits.write_u8(0b001, 3); // REV_DCCH_RC_MAP
        bits.write_u8(1, 1); // REV_FCH_GATING_REQ

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(1), Some(6)))
                .expect("decode page response rev6");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(1), Some(6)));
        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response message");
        };
        assert!(msg.mob_term);
        assert_eq!(2, msg.slot_cycle_index);
        assert_eq!(6, msg.mob_p_rev);
        assert_eq!(0x3a, msg.scm);
        assert_eq!(1, msg.request_mode);
        assert_eq!(6, msg.service_option);
        assert_eq!(Some(0b0001), msg.encryption_supported);
        assert_eq!(1, msg.num_alt_so);
        assert_eq!(vec![0x22], msg.alt_service_options);
        assert_eq!(Some(true), msg.uzid_incl);
        assert_eq!(Some(0x1234), msg.uzid);
        assert_eq!(Some(0b11), msg.ch_ind);
        assert_eq!(Some(true), msg.otd_supported);
        assert_eq!(Some(true), msg.qpch_supported);
        assert_eq!(Some(true), msg.enhanced_rc);
        assert_eq!(Some(0b00101), msg.for_rc_pref);
        assert_eq!(Some(0b00011), msg.rev_rc_pref);
        assert_eq!(Some(true), msg.fch_supported);
        assert_eq!(Some(true), msg.dcch_supported);
        assert_eq!(Some(true), msg.rev_fch_gating_req);
        assert_eq!(0, msg.remaining_bits);
        let fch = msg.fch_capability.as_ref().expect("fch capability");
        assert!(fch.frame_size_5ms_supported);
        assert_eq!(vec![1, 3], fch.for_supported_rcs);
        assert_eq!(vec![1, 2], fch.rev_supported_rcs);
        let dcch = msg.dcch_capability.as_ref().expect("dcch capability");
        assert_eq!(0b01, dcch.frame_size_mode);
        assert_eq!(vec![2], dcch.for_supported_rcs);
        assert_eq!(vec![3], dcch.rev_supported_rcs);

        let access = AccessMessage::PageResponse(msg);
        assert_eq!(Some(6), access.service_option());
        assert_eq!(vec![1, 3], access.for_supported_rcs());
        assert_eq!(vec![1, 2], access.rev_supported_rcs());
    }

    #[test]
    fn test_access_message_decoder_page_response_rev6_capability_tail() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0b011, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(6, 8); // MOB_P_REV
        bits.write_u8(0x5a, 8); // SCM
        bits.write_u8(0b000, 3); // REQUEST_MODE
        bits.write_u32(1, 16); // SERVICE_OPTION
        bits.write_u8(0, 1); // PM
        bits.write_u8(1, 1); // NAR_AN_CAP
        bits.write_u8(0b0001, 4); // ENCRYPTION_SUPPORTED
        bits.write_u8(1, 3); // NUM_ALT_SO
        bits.write_u32(0x22, 16); // ALT_SO[0]
        bits.write_u8(1, 1); // UZID_INCL
        bits.write_u32(0x1234, 16); // UZID
        bits.write_u8(0b11, 2); // CH_IND
        bits.write_u8(1, 1); // OTD_SUPPORTED
        bits.write_u8(1, 1); // QPCH_SUPPORTED
        bits.write_u8(1, 1); // ENHANCED_RC
        bits.write_u8(0b00101, 5); // FOR_RC_PREF
        bits.write_u8(0b00011, 5); // REV_RC_PREF
        bits.write_u8(1, 1); // FCH_SUPPORTED
        bits.write_u8(1, 1); // FCH_FRAME_SIZE
        bits.write_u8(1, 3); // FOR_FCH_LEN
        bits.write_u8(0b101, 3); // FOR_FCH_RC_MAP
        bits.write_u8(1, 3); // REV_FCH_LEN
        bits.write_u8(0b110, 3); // REV_FCH_RC_MAP
        bits.write_u8(1, 1); // DCCH_SUPPORTED
        bits.write_u8(0b01, 2); // DCCH_FRAME_SIZE
        bits.write_u8(1, 3); // FOR_DCCH_LEN
        bits.write_u8(0b010, 3); // FOR_DCCH_RC_MAP
        bits.write_u8(1, 3); // REV_DCCH_LEN
        bits.write_u8(0b001, 3); // REV_DCCH_RC_MAP
        bits.write_u8(1, 1); // REV_FCH_GATING_REQ

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(1), Some(6)))
                .expect("decode page response rev6");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(1), Some(6)));
        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response message");
        };
        assert_eq!(Some(0b0001), msg.encryption_supported);
        assert_eq!(Some(true), msg.uzid_incl);
        assert_eq!(Some(0x1234), msg.uzid);
        assert_eq!(Some(0b11), msg.ch_ind);
        assert_eq!(Some(true), msg.otd_supported);
        assert_eq!(Some(true), msg.qpch_supported);
        assert_eq!(Some(true), msg.enhanced_rc);
        assert_eq!(Some(0b00101), msg.for_rc_pref);
        assert_eq!(Some(0b00011), msg.rev_rc_pref);
        assert_eq!(Some(true), msg.fch_supported);
        assert_eq!(Some(true), msg.dcch_supported);
        assert_eq!(Some(true), msg.rev_fch_gating_req);
        assert_eq!(0, msg.remaining_bits);
        let fch = msg.fch_capability.expect("fch capability");
        assert!(fch.frame_size_5ms_supported);
        assert_eq!(vec![1, 3], fch.for_supported_rcs);
        assert_eq!(vec![1, 2], fch.rev_supported_rcs);
        let dcch = msg.dcch_capability.expect("dcch capability");
        assert_eq!(0b01, dcch.frame_size_mode);
        assert_eq!(vec![2], dcch.for_supported_rcs);
        assert_eq!(vec![3], dcch.rev_supported_rcs);
    }

    #[test]
    fn test_access_message_decoder_page_response_rev7_wll_tail() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0b010, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(7, 8); // MOB_P_REV
        bits.write_u8(0x42, 8); // SCM
        bits.write_u8(0b000, 3); // REQUEST_MODE
        bits.write_u32(1, 16); // SERVICE_OPTION
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // NAR_AN_CAP
        bits.write_u8(0, 3); // NUM_ALT_SO
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(0b01, 2); // CH_IND
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(1, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(0b00001, 5); // FOR_RC_PREF
        bits.write_u8(0b00011, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        bits.write_u8(1, 1); // STS_SUPPORTED
        bits.write_u8(1, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(1, 1); // WLL_INCL
        bits.write_u8(0b101, 3); // WLL_DEVICE_TYPE
        bits.write_u8(0b0110, 4); // HOOK_STATUS
        bits.write_u8(0, 1); // ENC_INFO_INCL = 0
        bits.write_u8(0, 1); // SYNC_ID_INCL = 0
        bits.write_u8(0b00, 2); // SO_BITMAP_IND = 0

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(0), Some(7)))
                .expect("decode page response rev7");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(0), Some(7)));
        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response message");
        };
        assert_eq!(None, msg.encryption_supported);
        assert_eq!(Some(true), msg.sts_supported);
        assert_eq!(Some(true), msg.cch_3x_supported);
        assert_eq!(Some(true), msg.wll_incl);
        assert_eq!(Some(0b101), msg.wll_device_type);
        assert_eq!(Some(0b0110), msg.hook_status);
        assert_eq!(Some(false), msg.enc_info_incl);
        assert_eq!(Some(false), msg.sync_id_incl);
        assert_eq!(Some(0), msg.so_bitmap_ind);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_page_response_rev7_enc_info_and_so_bitmap() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(0, 1); // MOB_TERM
        bits.write_u8(0b010, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(7, 8); // MOB_P_REV
        bits.write_u8(0x42, 8); // SCM
        bits.write_u8(0b000, 3); // REQUEST_MODE
        bits.write_u32(1, 16); // SERVICE_OPTION
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // NAR_AN_CAP
        // ENCRYPTION_SUPPORTED omitted (p_rev >= 7)
        bits.write_u8(0, 3); // NUM_ALT_SO
        // p_rev_in_use >= 6 block
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(0b01, 2); // CH_IND
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(0, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(0b00001, 5); // FOR_RC_PREF
        bits.write_u8(0b00001, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        // p_rev_in_use >= 7 block
        bits.write_u8(0, 1); // STS_SUPPORTED
        bits.write_u8(0, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(0, 1); // WLL_INCL = 0
        // ENC_INFO_INCL = 1 with ECMEA set
        bits.write_u8(1, 1); // ENC_INFO_INCL
        bits.write_u8(0b01000000, 8); // SIG_ENCRYPT_SUP (ECMEA=1)
        bits.write_u8(1, 1); // D_SIG_ENCRYPT_REQ
        bits.write_u8(0, 1); // C_SIG_ENCRYPT_REQ
        bits.write_u32(0xABCDEF, 24); // NEW_SSEQ_H
        bits.write_u8(0x42, 8); // NEW_SSEQ_H_SIG
        bits.write_u8(1, 1); // UI_ENCRYPT_REQ
        bits.write_u8(0b01100000, 8); // UI_ENCRYPT_SUP
        // SYNC_ID_INCL = 0
        bits.write_u8(0, 1);
        // SO_BITMAP_IND = 1 (4-bit bitmap)
        bits.write_u8(0b01, 2); // SO_BITMAP_IND
        bits.write_u8(0b00011, 5); // SO_GROUP_NUM
        bits.write_u8(0b1010, 4); // SO_BITMAP (2^(1+1)=4 bits)

        let msg =
            AccessMessage::decode_with_context(&bits, AccessDecodeContext::new(Some(0), Some(7)))
                .expect("decode page response rev7 enc_info");
        assert_reencodes(&bits, &msg, AccessDecodeContext::new(Some(0), Some(7)));
        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response message");
        };
        assert_eq!(Some(true), msg.enc_info_incl);
        assert_eq!(Some(0b01000000), msg.sig_encrypt_sup);
        assert_eq!(Some(1), msg.d_sig_encrypt_req);
        assert_eq!(Some(0), msg.c_sig_encrypt_req);
        assert_eq!(Some(0xABCDEF), msg.new_sseq_h);
        assert_eq!(Some(0x42), msg.new_sseq_h_sig);
        assert_eq!(Some(1), msg.ui_encrypt_req);
        assert_eq!(Some(0b01100000), msg.ui_encrypt_sup);
        assert_eq!(Some(false), msg.sync_id_incl);
        assert_eq!(Some(1), msg.so_bitmap_ind);
        assert_eq!(Some(3), msg.so_group_num);
        assert_eq!(Some(0b1010), msg.so_bitmap);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_page_response_rev9_integrity_pdch_and_ext_channel_roundtrip() {
        let ctx = AccessDecodeContext::new(Some(0), Some(9));
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0, 3); // SLOT_CYCLE_INDEX
        bits.write_u8(9, 8); // MOB_P_REV
        bits.write_u8(0x2a, 8); // SCM
        bits.write_u8(0b001, 3); // REQUEST_MODE
        bits.write_u32(33, 16); // SERVICE_OPTION
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // NAR_AN_CAP
        // ENCRYPTION_SUPPORTED omitted (p_rev >= 7)
        bits.write_u8(0, 3); // NUM_ALT_SO
        // p_rev_in_use >= 6 block
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(0b00, 2); // CH_IND, so EXT_CH_IND is present
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(0, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(1, 5); // FOR_RC_PREF
        bits.write_u8(1, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        // p_rev_in_use >= 7 block
        bits.write_u8(0, 1); // STS_SUPPORTED
        bits.write_u8(0, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(0, 1); // WLL_INCL
        bits.write_u8(0, 1); // ENC_INFO_INCL
        bits.write_u8(0, 1); // SYNC_ID_INCL
        bits.write_u8(0, 2); // SO_BITMAP_IND
        // p_rev_in_use >= 8 block
        bits.write_u8(0, 1); // ALT_BAND_CLASS_SUP
        // p_rev_in_use >= 9 block
        bits.write_u8(1, 1); // MSG_INT_INFO_INCL
        bits.write_u8(1, 1); // SIG_INTEGRITY_SUP_INCL
        bits.write_u8(0, 8); // SIG_INTEGRITY_SUP
        bits.write_u8(0, 3); // SIG_INTEGRITY_REQ
        bits.write_u8(0b01, 2); // NEW_KEY_ID
        bits.write_u8(1, 1); // NEW_SSEQ_H_INCL
        bits.write_u32(0x123456, 24); // NEW_SSEQ_H
        bits.write_u8(0x78, 8); // NEW_SSEQ_H_SIG
        bits.write_u8(1, 1); // FOR_PDCH_SUPPORTED
        bits.write_u8(1, 1); // ACK_DELAY
        bits.write_u8(0b01, 2); // NUM_ARQ_CHAN
        bits.write_u8(0, 2); // FOR_PDCH_LEN
        bits.write_u8(0b100, 3); // FOR_PDCH_RC_MAP: RC10 plus reserved zeroes
        bits.write_u8(1, 2); // CH_CONFIG_SUP_MAP_LEN
        bits.write_u8(0b101000, 6); // F-PDCH_1/F-PDCH_3 supported
        bits.write_u8(0b01001, 5); // EXT_CH_IND

        let msg =
            AccessMessage::decode_with_context(&bits, ctx).expect("decode page response rev9");
        assert_reencodes(&bits, &msg, ctx);

        let mut invalid_sup = msg.clone();
        if let AccessMessage::PageResponse(page_response) = &mut invalid_sup {
            page_response.sig_integrity_sup = Some(0x80);
        }
        let err = invalid_sup
            .to_reverse_common_pdu_with_context(ctx)
            .expect_err("reject SIG_INTEGRITY_SUP reserved bits");
        assert!(err.contains("SIG_INTEGRITY_SUP RESERVED"));

        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response");
        };
        assert_eq!(Some(true), msg.msg_int_info_incl);
        assert_eq!(Some(true), msg.sig_integrity_sup_incl);
        assert_eq!(Some(0), msg.sig_integrity_sup);
        assert_eq!(Some(0), msg.sig_integrity_req);
        assert_eq!(Some(0b01), msg.new_key_id);
        assert_eq!(Some(true), msg.new_sseq_h_incl);
        assert_eq!(Some(0x123456), msg.new_sseq_h);
        assert_eq!(Some(0x78), msg.new_sseq_h_sig);
        assert_eq!(Some(true), msg.for_pdch_supported);
        let cap = msg
            .for_pdch_capability
            .as_ref()
            .expect("FOR_PDCH capability");
        assert!(cap.ack_delay);
        assert_eq!(0b01, cap.num_arq_chan);
        assert_eq!(0, cap.for_pdch_len);
        assert_eq!(vec![10], cap.for_pdch_supported_rcs);
        assert_eq!(1, cap.ch_config_sup_map_len);
        assert_eq!(vec![1, 3], cap.ch_config_supported);
        assert_eq!(Some(0b01001), msg.ext_ch_ind);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_page_response_rev11_bcmc_rev_pdch_and_band_fields_roundtrip() {
        let ctx = AccessDecodeContext::new(Some(0), Some(11));
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(1, 1); // MOB_TERM
        bits.write_u8(0b010, 3); // SLOT_CYCLE_INDEX, so SIGN_SLOT_CYCLE_INDEX is present
        bits.write_u8(11, 8); // MOB_P_REV
        bits.write_u8(0x2a, 8); // SCM
        bits.write_u8(0b001, 3); // REQUEST_MODE
        bits.write_u32(33, 16); // SERVICE_OPTION
        bits.write_u8(0, 1); // PM
        bits.write_u8(0, 1); // NAR_AN_CAP
        bits.write_u8(0, 3); // NUM_ALT_SO
        bits.write_u8(0, 1); // UZID_INCL
        bits.write_u8(0b00, 2); // CH_IND
        bits.write_u8(0, 1); // OTD_SUPPORTED
        bits.write_u8(0, 1); // QPCH_SUPPORTED
        bits.write_u8(0, 1); // ENHANCED_RC
        bits.write_u8(1, 5); // FOR_RC_PREF
        bits.write_u8(1, 5); // REV_RC_PREF
        bits.write_u8(0, 1); // FCH_SUPPORTED
        bits.write_u8(0, 1); // DCCH_SUPPORTED
        bits.write_u8(0, 1); // REV_FCH_GATING_REQ
        bits.write_u8(0, 1); // STS_SUPPORTED
        bits.write_u8(0, 1); // 3X_CCH_SUPPORTED
        bits.write_u8(0, 1); // WLL_INCL
        bits.write_u8(0, 1); // ENC_INFO_INCL
        bits.write_u8(0, 1); // SYNC_ID_INCL
        bits.write_u8(0, 2); // SO_BITMAP_IND
        bits.write_u8(0, 1); // ALT_BAND_CLASS_SUP
        bits.write_u8(0, 1); // MSG_INT_INFO_INCL
        bits.write_u8(1, 1); // FOR_PDCH_SUPPORTED
        bits.write_u8(1, 1); // ACK_DELAY
        bits.write_u8(0b01, 2); // NUM_ARQ_CHAN
        bits.write_u8(0, 2); // FOR_PDCH_LEN
        bits.write_u8(0b100, 3); // FOR_PDCH_RC_MAP
        bits.write_u8(1, 2); // CH_CONFIG_SUP_MAP_LEN
        bits.write_u8(0b101000, 6); // CH_CONFIG_SUP_MAP
        bits.write_u8(0b01001, 5); // EXT_CH_IND
        bits.write_u8(1, 1); // SIGN_SLOT_CYCLE_INDEX
        bits.write_u8(1, 1); // BCMC_INCL
        bits.write_u8(1, 1); // BCMC_PREF_INCL
        bits.write_u8(1, 1); // FUNDICATED_BCMC_SUPPORTED
        bits.write_u8(1, 2); // FUNDICATED_BCMC_CH_SUP_MAP_LEN
        bits.write_u8(0b101000, 6); // FUNDICATED_BCMC_CH_SUP_MAP
        bits.write_u8(1, 1); // AUTH_SIGNATURE_INCL
        bits.write_u8(4, 8); // TIME_STAMP_SHORT_LENGTH
        bits.write_u8(0b1010, 4); // TIME_STAMP_SHORT
        bits.write_u8(0, 3); // NUM_BCMC_PROGRAMS
        bits.write_u8(3, 5); // BCMC_PROGRAM_ID_LEN
        bits.write_u8(0b1011, 4); // BCMC_PROGRAM_ID
        bits.write_u8(2, 3); // BCMC_FLOW_DISCRIMINATOR_LEN
        bits.write_u8(0, 2); // NUM_FLOW_DISCRIMINATOR
        bits.write_u8(0b10, 2); // BCMC_FLOW_DISCRIMINATOR
        bits.write_u8(1, 1); // BCMC_PREF
        bits.write_u8(1, 1); // AUTH_SIGNATURE_IND
        bits.write_u8(0, 1); // AUTH_SIGNATURE_SAME_IND
        bits.write_u8(0x0a, 4); // BAK_ID
        bits.write_u32(0x12345678, 32); // AUTH_SIGNATURE
        bits.write_u8(1, 1); // REV_PDCH_SUPPORTED
        bits.write_u8(0, 2); // REV_PDCH_LEN
        bits.write_u8(0b100, 3); // REV_PDCH_RC_MAP
        bits.write_u8(1, 2); // REV_PDCH_CH_CONFIG_SUP_MAP_LEN
        bits.write_u8(0b110100, 6); // REV_PDCH_CH_CONFIG_SUP_MAP
        bits.write_u8(0b01, 2); // REV_PDCH_MAX_SIZE_SUPPORTED_ENCODER_PACKET
        bits.write_u8(1, 1); // BAND_SUB_REP_INCL
        bits.write_u8(2, 4); // NUM_BAND_SUBCLASS
        bits.write_u8(1, 1); // BAND_SUBCLASS_SUP[0]
        bits.write_u8(0, 1); // BAND_SUBCLASS_SUP[1]

        let msg =
            AccessMessage::decode_with_context(&bits, ctx).expect("decode page response rev11");
        assert_reencodes(&bits, &msg, ctx);

        let mut invalid_band = msg.clone();
        if let AccessMessage::PageResponse(page_response) = &mut invalid_band {
            page_response.num_band_subclass = Some(3);
        }
        let err = invalid_band
            .to_reverse_common_pdu_with_context(ctx)
            .expect_err("reject band subclass count mismatch");
        assert!(err.contains("NUM_BAND_SUBCLASS=3"));

        let AccessMessage::PageResponse(msg) = msg else {
            panic!("expected page response");
        };
        assert_eq!(Some(true), msg.sign_slot_cycle_index);
        assert_eq!(Some(true), msg.bcmc_incl);
        assert_eq!(Some(true), msg.bcmc_pref_incl);
        let bcmc = msg.bcmc.expect("BCMC fields");
        assert_eq!(Some(4), bcmc.time_stamp_short_length);
        assert_eq!(4, bcmc.time_stamp_short.len());
        assert_eq!(0, bcmc.num_bcmc_programs);
        assert_eq!(1, bcmc.programs.len());
        let flow = &bcmc.programs[0].flows[0];
        assert_eq!(Some(true), flow.bcmc_pref);
        assert_eq!(Some(true), flow.auth_signature_ind);
        assert_eq!(Some(false), flow.auth_signature_same_ind);
        assert_eq!(Some(0x0a), flow.bak_id);
        assert_eq!(Some(0x12345678), flow.auth_signature);
        assert_eq!(Some(true), msg.rev_pdch_supported);
        let rev = msg.rev_pdch_capability.expect("REV_PDCH capability");
        assert_eq!(vec![7], rev.rev_pdch_supported_rcs);
        assert_eq!(vec![0, 1, 3], rev.rev_pdch_ch_config_supported);
        assert_eq!(0b01, rev.rev_pdch_max_size_supported_encoder_packet);
        assert_eq!(Some(true), msg.band_sub_rep_incl);
        assert_eq!(Some(2), msg.num_band_subclass);
        assert_eq!(Some(vec![1, 0]), msg.band_subclass_sup);
        assert_eq!(0, msg.remaining_bits);
    }

    #[test]
    fn test_access_message_decoder_page_response_requires_auth_context() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::PageResponse), 8);
        bits.write_u8(0, 1);
        bits.write_u8(0b001, 3);
        bits.write_u8(5, 8);
        bits.write_u8(0x42, 8);
        bits.write_u8(0b000, 3);
        bits.write_u32(1, 16);
        bits.write_u8(0, 1);
        bits.write_u8(1, 1);
        bits.write_u8(2, 3);
        bits.write_u32(3, 16);
        bits.write_u32(4096, 16);

        assert!(
            AccessMessage::decode(&bits)
                .unwrap_err()
                .contains("requires AUTH_MODE context")
        );
    }

    #[test]
    fn test_access_message_decoder_order() {
        let mut bits = Bitstream::new();
        // PD=0, MSG_TYPE=0b000010 (Order)
        bits.write_u8(rcsch_wire(MessageId::Order), 8);
        // ORDER = 0b010000 (MS Acknowledgment)
        bits.write_u8(0b010000, 6);
        // ADD_RECORD_LEN = 0 (no extra fields)
        bits.write_u8(0, 3);

        let msg = AccessMessage::decode(&bits).expect("decode order");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::Order(msg) = msg else {
            panic!("expected order message");
        };
        assert_eq!(0b010000, msg.order);
        assert_eq!(0, msg.add_record_len);
        assert!(msg.order_specific.is_empty());
        assert_eq!("Mobile Station Acknowledgment", msg.order_name());
    }

    #[test]
    fn test_access_message_decoder_order_with_fields() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Order), 8);
        // ORDER = 0b000010 (Base Station Challenge)
        bits.write_u8(0b000010, 6);
        // ADD_RECORD_LEN = 3
        bits.write_u8(3, 3);
        // 3 order-specific bytes
        bits.write_u8(0xAA, 8);
        bits.write_u8(0xBB, 8);
        bits.write_u8(0xCC, 8);

        let msg = AccessMessage::decode(&bits).expect("decode order with fields");
        let AccessMessage::Order(msg) = msg else {
            panic!("expected order message");
        };
        assert_eq!(0b000010, msg.order);
        assert_eq!(3, msg.add_record_len);
        assert_eq!(vec![0xAA, 0xBB, 0xCC], msg.order_specific);
    }

    #[test]
    fn test_reverse_order_detail_roundtrips_challenge_and_service_option() {
        let header = AccessMessageHeader {
            pd: 0,
            message_id: MessageId::Order,
        };
        let challenge = ReverseOrderDetail::BaseStationChallenge {
            randbs: 0x1234_5678,
        };
        let challenge_msg = OrderMessage::from_reverse_detail(header.clone(), &challenge)
            .expect("encode challenge");
        assert_eq!(challenge_msg.order, 0b000010);
        assert_eq!(
            challenge_msg.order_specific,
            vec![0x00, 0x12, 0x34, 0x56, 0x78]
        );
        assert_eq!(
            challenge_msg
                .reverse_detail(WireChannel::ForwardCommon)
                .expect("parse challenge"),
            challenge
        );

        let so_request = ReverseOrderDetail::ServiceOptionRequest { service_option: 33 };
        let so_msg = OrderMessage::from_reverse_detail(header, &so_request)
            .expect("encode service option request");
        assert_eq!(so_msg.order, 0b010011);
        assert_eq!(so_msg.order_specific, vec![0x00, 0x00, 0x21]);
        assert_eq!(
            so_msg
                .reverse_detail(WireChannel::ForwardCommon)
                .expect("parse service option"),
            so_request
        );
        assert_eq!(
            "ORDQ=0x00, SO=33",
            so_msg.order_detail(WireChannel::ForwardCommon)
        );
    }

    #[test]
    fn test_access_message_decoder_mobile_station_reject_order_details() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Order), 8);
        bits.write_u8(0b011111, 6);
        bits.write_u8(3, 3);
        bits.write_u8(0x04, 8);
        bits.write_u8(0x15, 8);
        bits.write_u8(0x00, 8);

        let msg = AccessMessage::decode(&bits).expect("decode mobile station reject order");
        let AccessMessage::Order(msg) = msg else {
            panic!("expected order message");
        };
        let detail = msg
            .parse_mobile_station_reject_order(WireChannel::ForwardCommon)
            .expect("parse reject details");

        assert_eq!(0x04, detail.ordq);
        assert_eq!(0x15, detail.rejected_type);
        assert_eq!(vec![0x00], detail.trailing_bytes);
        assert_eq!(
            "ORDQ=0x04 (message field not in valid range), REJECTED_TYPE=0x15 (Extended Channel Assignment Message), trailing=[00]",
            msg.order_detail(WireChannel::ForwardCommon)
        );
    }

    #[test]
    fn test_access_message_decoder_mobile_station_reject_order_fields_for_rejected_order() {
        let mut bits = Bitstream::new();
        bits.write_u8(rcsch_wire(MessageId::Order), 8);
        bits.write_u8(0b011111, 6);
        bits.write_u8(4, 3);
        bits.write_u8(0x04, 8);
        bits.write_u8(
            MessageId::Order
                .wire_type(WireChannel::ForwardCommon)
                .unwrap(),
            8,
        );
        bits.write_u8(0b00_010010, 8);
        bits.write_u8(0x00, 8);

        let msg = AccessMessage::decode(&bits).expect("decode reject order with order fields");
        let AccessMessage::Order(msg) = msg else {
            panic!("expected order message");
        };
        let detail = msg
            .parse_mobile_station_reject_order(WireChannel::ForwardCommon)
            .expect("parse reject details");

        assert_eq!(Some(0b010010), detail.rejected_order);
        assert_eq!(Some(0x00), detail.rejected_ordq);
        assert!(detail.trailing_bytes.is_empty());
        assert!(
            msg.order_detail(WireChannel::ForwardCommon)
                .contains("REJECTED_ORDER=0b010010 (Release)")
        );
        assert!(
            msg.order_detail(WireChannel::ForwardCommon)
                .contains("REJECTED_ORDQ=0x00")
        );
    }

    #[test]
    fn test_reverse_order_detail_rejects_mobile_station_reject_reserved_bits() {
        let msg = super::OrderMessage {
            header: super::AccessMessageHeader {
                pd: 0,
                message_id: MessageId::Order,
            },
            order: 0b011111,
            add_record_len: 4,
            order_specific: vec![
                0x04,
                MessageId::Order
                    .wire_type(WireChannel::ForwardCommon)
                    .unwrap(),
                0b10_010010,
                0x00,
            ],
            remaining_bits: 0,
        };

        assert!(
            msg.parse_mobile_station_reject_order_strict(WireChannel::ForwardCommon)
                .is_err(),
            "RESERVED_1 must be zero before REJECTED_ORDER"
        );
    }

    #[test]
    fn test_reverse_order_detail_roundtrips_reduced_slot_cycle_release() {
        let header = AccessMessageHeader {
            pd: 0,
            message_id: MessageId::Order,
        };
        let detail = ReverseOrderDetail::ReducedSlotCycle(ReducedSlotCycleOrderDetail {
            order: 0b010101,
            ordq: 0x03,
            rsc_mode_ind: true,
            rsci: Some(0x02),
            rsc_end_time_unit: Some(0x01),
            rsc_end_time_value: Some(0x0a),
        });

        let msg =
            OrderMessage::from_reverse_detail(header.clone(), &detail).expect("encode RSC release");

        assert_eq!(msg.order, 0b010101);
        assert_eq!(msg.order_specific, vec![0x03, 0x93, 0x40]);
        assert_eq!(
            msg.reverse_detail(WireChannel::ForwardDedicated)
                .expect("parse RSC release"),
            detail
        );

        let reserved_rsci = ReverseOrderDetail::ReducedSlotCycle(ReducedSlotCycleOrderDetail {
            order: 0b010101,
            ordq: 0x03,
            rsc_mode_ind: true,
            rsci: Some(0b0101),
            rsc_end_time_unit: Some(0),
            rsc_end_time_value: Some(0),
        });
        let err = OrderMessage::from_reverse_detail(header.clone(), &reserved_rsci).unwrap_err();
        assert!(err.contains("Reduced slot cycle RSCI 0b0101 is reserved"));

        let raw_reserved = OrderMessage {
            header,
            order: 0b010101,
            add_record_len: 3,
            order_specific: vec![0x03, 0b1010_1000, 0x00],
            remaining_bits: 0,
        };
        let err = raw_reserved
            .reverse_detail(WireChannel::ForwardDedicated)
            .unwrap_err();
        assert!(err.contains("Reduced slot cycle RSCI 0b0101 is reserved"));
    }

    #[test]
    fn test_rdsch_mobile_station_reject_uses_forward_dedicated_message_table() {
        use super::RdschPdu;

        let mut bits = Bitstream::new();
        bits.write_u8(
            MessageId::Order
                .wire_type(WireChannel::ReverseDedicated)
                .unwrap(),
            8,
        );
        bits.write_u8(1, 3); // ACK_SEQ acknowledges f-dsch MSG_SEQ=1
        bits.write_u8(1, 3); // MSG_SEQ
        bits.write_u8(0, 1); // ACK_REQ
        bits.write_u8(0, 2); // ENCRYPTION
        bits.write_u8(0b011111, 6); // Mobile Station Reject Order
        bits.write_u8(3, 3); // ADD_RECORD_LEN
        bits.write_u8(0x03, 8); // message structure not acceptable
        bits.write_u8(0x06, 8); // unknown on f-dsch; Page Message on f-csch
        bits.write_u8(0x00, 8);

        let pdu = RdschPdu::decode(&bits).expect("decode r-dsch reject order");

        assert!(pdu.summary().contains("REJECTED_TYPE=0x06 (Unknown)"));
        assert!(!pdu.summary().contains("Page Message"));
    }

    #[test]
    fn test_mobile_station_reject_order_forward_dedicated_awim_record() {
        let msg = super::OrderMessage {
            header: super::AccessMessageHeader {
                pd: 0,
                message_id: MessageId::Order,
            },
            order: 0b011111,
            add_record_len: 3,
            order_specific: vec![0x03, 0x03, 0x05],
            remaining_bits: 0,
        };

        let detail = msg
            .parse_mobile_station_reject_order(WireChannel::ForwardDedicated)
            .expect("parse reject details");

        assert_eq!(0x03, detail.ordq);
        assert_eq!(0x03, detail.rejected_type);
        assert_eq!(Some(0x05), detail.rejected_record);
        assert!(detail.trailing_bytes.is_empty());
        assert!(
            msg.order_detail(WireChannel::ForwardDedicated)
                .contains("REJECTED_TYPE=0x03 (Alert With Information Message)")
        );
    }

    #[test]
    fn test_access_message_decoder_data_burst() {
        let mut bits = Bitstream::new();
        // PD=0, MSG_TYPE=0b000011 (Data Burst)
        bits.write_u8(rcsch_wire(MessageId::DataBurst), 8);
        bits.write_u8(1, 8); // MSG_NUMBER
        bits.write_u8(0b000011, 6); // BURST_TYPE = SMS
        bits.write_u8(1, 8); // NUM_MSGS
        bits.write_u8(3, 8); // NUM_FIELDS
        bits.write_u8(0x48, 8); // 'H'
        bits.write_u8(0x69, 8); // 'i'
        bits.write_u8(0x21, 8); // '!'

        let msg = AccessMessage::decode(&bits).expect("decode data burst");
        assert_reencodes(&bits, &msg, AccessDecodeContext::default());
        let AccessMessage::DataBurst(msg) = msg else {
            panic!("expected data burst message");
        };
        assert_eq!(1, msg.msg_number);
        assert_eq!(0b000011, msg.burst_type);
        assert_eq!("Short Message Services", msg.burst_type_name());
        assert_eq!(1, msg.num_msgs);
        assert_eq!(3, msg.num_fields);
        assert_eq!(vec![0x48, 0x69, 0x21], msg.fields);
    }

    #[test]
    fn test_data_burst_type_names_match_c_r1001_table_4_1_1() {
        let header = AccessMessageHeader {
            pd: 0,
            message_id: MessageId::DataBurst,
        };
        let cases = [
            (0b000000, "Unknown burst type"),
            (0b000001, "Asynchronous Data Services"),
            (0b000010, "Group-3 Facsimile"),
            (0b000011, "Short Message Services"),
            (0b000100, "Over-the-Air Service Provisioning"),
            (0b000101, "Position Determination Services"),
            (0b000110, "Short Data Bursts"),
            (0b000111, "HRPD Packet Data Service Notification"),
            (0b001000, "Broadcast Multicast Service"),
            (0b001001, "Reserved"),
            (0b111110, "Extended Burst Type - International"),
            (0b111111, "Extended Burst Type"),
        ];

        for (burst_type, name) in cases {
            let msg = DataBurstMessage {
                header: header.clone(),
                msg_number: 0,
                burst_type,
                num_msgs: 1,
                num_fields: 0,
                fields: Vec::new(),
                remaining_bits: 0,
            };
            assert_eq!(name, msg.burst_type_name(), "BURST_TYPE={burst_type:06b}");
        }
    }

    #[test]
    fn test_format_origination_digits_dtmf_hash_777() {
        assert_eq!(
            "#777(raw=c777)",
            super::format_origination_digits(false, &[0x0c, 0x07, 0x07, 0x07])
        );
    }

    // -----------------------------------------------------------------------
    // SMS flow trace tests — RecNo 54-57 from real BTS capture 2026/03/23
    // -----------------------------------------------------------------------

    /// Build the f-dsch Service Connect Message bitstream matching trace RecNo 54.
    fn build_service_connect_bitstream() -> Bitstream {
        let mut bs = Bitstream::new();
        // -- PDU header --
        bs.write_u32(0x14, 8); // MSG_TYPE = Service Connect
        bs.write_u32(7, 3); // ACK_SEQ = 7
        bs.write_u32(1, 3); // MSG_SEQ = 1
        bs.write_u32(1, 1); // ACK_REQ = 1
        bs.write_u32(0, 2); // ENCRYPTION = 0

        // -- SDU header --
        bs.write_u32(0, 1); // USE_TIME = 0
        bs.write_u32(0, 6); // ACTION_TIME = 0
        bs.write_u32(0, 3); // SERV_CON_SEQ = 0
        bs.write_u32(0, 2); // RESERVED = 0
        bs.write_u32(0, 2); // USE_OLD_SERV_CONFIG = 0b00
        bs.write_u32(0, 1); // SYNC_ID_INCL = 0

        // -- Record 1: Service Configuration (type=0x07, len=15) --
        bs.write_u32(0x07, 8); // RECORD_TYPE
        bs.write_u32(0x10, 8); // RECORD_LEN = 16
        bs.write_u32(0x0001, 16); // FOR_MUX_OPTION = 1
        bs.write_u32(0x0001, 16); // REV_MUX_OPTION = 1
        bs.write_u32(0xF0, 8); // FOR_RATES = 0xF0
        bs.write_u32(0xF0, 8); // REV_RATES = 0xF0
        bs.write_u32(1, 8); // NUM_CON_REC = 1
        // Connection record 0
        bs.write_u32(7, 8); // RECORD_LEN includes the length octet
        bs.write_u32(0, 8); // CON_REF = 0
        bs.write_u32(6, 16); // SERVICE_OPTION = 6 (SO6 SMS)
        bs.write_u32(1, 4); // FOR_TRAFFIC = 1
        bs.write_u32(1, 4); // REV_TRAFFIC = 1
        bs.write_u32(0, 3); // UI_ENCRYPT_MODE = 0
        bs.write_u32(1, 3); // SR_ID = 1
        bs.write_u32(0, 1); // RLP_INFO_INCL = 0
        bs.write_u32(0, 1); // RESERVED
        bs.write_u32(0, 8); // Padding (7 octets total, 48 body bits)
        // Channel config
        bs.write_u32(1, 1); // FCH_CC_INCL = 1
        bs.write_u32(0, 1); // FCH_FRAME_SIZE = 0
        bs.write_u32(3, 5); // FOR_FCH_RC = 3
        bs.write_u32(3, 5); // REV_FCH_RC = 3
        bs.write_u32(0, 1); // DCCH_CC_INCL = 0
        bs.write_u32(0, 1); // FOR_SCH_CC_INCL = 0
        bs.write_u32(0, 1); // REV_SCH_CC_INCL = 0
        bs.write_u32(0, 1); // RESERVED

        // -- Record 2: Non-Neg Service Config (type=0x13, len=5) --
        // 5 raw bytes computed from trace fields:
        // FPC_INCL=1, FPC_PRI_CHAN=0, FPC_MODE=000, FPC_OLPC_FCH_INCL=1,
        // FPC_FCH_FER=00010, FPC_FCH_MIN_SETPT=0x00, FPC_FCH_MAX_SETPT=0x50,
        // FPC_OLPC_DCCH_INCL=0, GATING_RATE_INCL=1, PILOT_GATING_RATE=00,
        // RESERVED=00, LPM_IND=00, RESERVED=00000
        // Bytes: [0x84, 0x40, 0x0A, 0x08, 0x00]
        bs.write_u32(0x13, 8); // RECORD_TYPE
        bs.write_u32(0x05, 8); // RECORD_LEN = 5
        bs.write_u32(0x84, 8);
        bs.write_u32(0x40, 8);
        bs.write_u32(0x0A, 8);
        bs.write_u32(0x08, 8);
        bs.write_u32(0x00, 8);

        bs.write_u32(0, 1); // CC_INFO_INCL = 0
        bs.write_u32(0, 1); // USE_TYPE0_PLCM = 0

        bs
    }

    /// RecNo 54: f-dsch Service Connect Message (BTS→MS)
    #[test]
    fn test_fdsch_service_connect_message_trace_recno_54() {
        use super::{FdschMessage, FdschPdu, ServiceConnectRecord};

        let bs = build_service_connect_bitstream();
        let pdu = FdschPdu::decode(&bs).expect("decode Service Connect");

        // PDU header
        assert_eq!(0x14, pdu.raw_msg_type);
        assert_eq!("Service Connect Message", pdu.msg_type_name());
        assert_eq!(7, pdu.arq.ack_seq);
        assert_eq!(1, pdu.arq.msg_seq);
        assert!(pdu.arq.ack_req);
        assert_eq!(0, pdu.arq.encryption);

        // SDU
        let FdschMessage::ServiceConnect(ref sc) = pdu.body else {
            panic!("expected ServiceConnect body, got {:?}", pdu.body);
        };
        assert!(!sc.use_time);
        assert_eq!(0, sc.action_time);
        assert_eq!(0, sc.serv_con_seq);
        assert_eq!(0, sc.use_old_serv_config);
        assert!(sc.sync_id.is_none());
        assert_eq!(2, sc.records.len());
        assert!(sc.call_assignments.is_empty());
        assert!(!sc.use_type0_plcm);

        // Record 0: Service Configuration
        let ServiceConnectRecord::ServiceConfig(ref cfg) = sc.records[0] else {
            panic!("expected ServiceConfig record");
        };
        assert_eq!(1, cfg.for_mux_option);
        assert_eq!(1, cfg.rev_mux_option);
        assert_eq!(0xF0, cfg.for_rates);
        assert_eq!(0xF0, cfg.rev_rates);
        assert_eq!(1, cfg.connection_records.len());
        let cr = &cfg.connection_records[0];
        assert_eq!(0, cr.con_ref);
        assert_eq!(6, cr.service_option); // SO6 = SMS
        assert_eq!(1, cr.for_traffic);
        assert_eq!(1, cr.rev_traffic);
        assert_eq!(0, cr.ui_encrypt_mode);
        assert_eq!(1, cr.sr_id);
        assert!(!cr.rlp_info_incl);
        assert!(cfg.fch_cc_incl);
        assert_eq!(Some(0), cfg.fch_frame_size);
        assert_eq!(Some(3), cfg.for_fch_rc);
        assert_eq!(Some(3), cfg.rev_fch_rc);
        assert!(!cfg.dcch_cc_incl);
        assert!(!cfg.for_sch_cc_incl);
        assert!(!cfg.rev_sch_cc_incl);

        // Record 1: Non-Negotiable Service Config (raw bytes)
        let ServiceConnectRecord::NonNegServiceConfig(ref nn) = sc.records[1] else {
            panic!("expected NonNegServiceConfig record");
        };
        assert_eq!(5, nn.raw_bytes.len());
        assert_eq!(vec![0x84, 0x40, 0x0A, 0x08, 0x00], nn.raw_bytes);
    }

    /// RecNo 55: r-dsch Service Connect Completion (MS→BTS)
    /// MSG_TYPE=0x0E, ACK_SEQ=1, MSG_SEQ=0, ACK_REQ=1, ENCRYPTION=0
    /// RESERVED=0, SERV_CON_SEQ=0
    #[test]
    fn test_rdsch_service_connect_completion_trace_recno_55() {
        use super::RdschPdu;

        let mut bs = Bitstream::new();
        bs.write_u32(
            MessageId::ServiceConnectCompletion
                .wire_type(WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        );
        bs.write_u32(1, 3); // ACK_SEQ = 1
        bs.write_u32(0, 3); // MSG_SEQ = 0
        bs.write_u32(1, 1); // ACK_REQ = 1
        bs.write_u32(0, 2); // ENCRYPTION = 0
        bs.write_u32(0, 1); // RESERVED = 0
        bs.write_u32(0, 3); // SERV_CON_SEQ = 0
        bs.write_u32(0, 3); // PDU_PADDING

        let pdu = RdschPdu::decode(&bs).expect("decode Service Connect Completion");

        assert_eq!(MessageId::ServiceConnectCompletion, pdu.message_id);
        assert_eq!("Service Connect Completion Message", pdu.msg_type_name());
        assert_eq!(1, pdu.arq.ack_seq);
        assert_eq!(0, pdu.arq.msg_seq);
        assert!(pdu.arq.ack_req);
        assert_eq!(0, pdu.arq.encryption);
        assert_eq!(Some(0), pdu.l3.serv_con_seq());
        assert!(
            pdu.summary()
                .contains("ServiceConnectCompletion(serv_con_seq=0)")
        );
    }

    /// RecNo 56: f-dsch BS Ack Order (BTS→MS)
    /// MSG_TYPE=0x01, ACK_SEQ=0, MSG_SEQ=0, ACK_REQ=0, ENCRYPTION=0
    /// USE_TIME=0, ACTION_TIME=0, ORDER=16(0b010000), ADD_RECORD_LEN=0
    #[test]
    fn test_fdsch_bs_ack_order_trace_recno_56() {
        use super::{FdschMessage, FdschPdu};

        let mut bs = Bitstream::new();
        bs.write_u32(0x01, 8); // MSG_TYPE = Order
        bs.write_u32(0, 3); // ACK_SEQ = 0
        bs.write_u32(0, 3); // MSG_SEQ = 0
        bs.write_u32(0, 1); // ACK_REQ = 0
        bs.write_u32(0, 2); // ENCRYPTION = 0
        bs.write_u32(0, 1); // USE_TIME = 0
        bs.write_u32(0, 6); // ACTION_TIME = 0
        bs.write_u32(16, 6); // ORDER = 0b010000 = BS Ack
        bs.write_u32(0, 3); // ADD_RECORD_LEN = 0
        bs.write_u32(0, 7); // PDU_PADDING

        let pdu = FdschPdu::decode(&bs).expect("decode BS Ack Order");

        assert_eq!(0x01, pdu.raw_msg_type);
        assert_eq!("Order Message", pdu.msg_type_name());
        assert_eq!(0, pdu.arq.ack_seq);
        assert_eq!(0, pdu.arq.msg_seq);
        assert!(!pdu.arq.ack_req);
        assert_eq!(0, pdu.arq.encryption);

        let FdschMessage::Order(ref order) = pdu.body else {
            panic!("expected Order body, got {:?}", pdu.body);
        };
        assert!(!order.use_time);
        assert_eq!(0, order.action_time);
        assert_eq!(0b010000, order.order);
        assert_eq!("Base Station Acknowledgment", order.order_name());
        assert_eq!(0, order.add_record_len);
        assert!(order.order_specific.is_empty());
        assert_eq!(None, order.con_ref_incl);
        assert_eq!(None, order.con_ref);
    }

    #[test]
    fn test_fdsch_call_control_order_decodes_con_ref() {
        use super::{FdschMessage, FdschPdu};

        let mut bs = Bitstream::new();
        bs.write_u32(
            MessageId::Order
                .wire_type(WireChannel::ForwardDedicated)
                .unwrap() as u32,
            8,
        );
        bs.write_u32(0, 3); // ACK_SEQ
        bs.write_u32(2, 3); // MSG_SEQ
        bs.write_u32(1, 1); // ACK_REQ
        bs.write_u32(0, 2); // ENCRYPTION
        bs.write_u32(0, 1); // USE_TIME
        bs.write_u32(0, 6); // ACTION_TIME
        bs.write_u32(0b010101, 6); // Release
        bs.write_u32(1, 3); // ADD_RECORD_LEN
        bs.write_u32(0, 8); // ORDQ
        bs.write_u32(1, 1); // CON_REF_INCL
        bs.write_u32(7, 8); // CON_REF

        let pdu = FdschPdu::decode(&bs).expect("decode f-dsch call-control order");
        let FdschMessage::Order(ref order) = pdu.body else {
            panic!("expected Order body, got {:?}", pdu.body);
        };

        assert_eq!("Release", order.order_name());
        assert_eq!(vec![0], order.order_specific);
        assert_eq!(Some(true), order.con_ref_incl);
        assert_eq!(Some(7), order.con_ref);
        assert!(pdu.summary().contains("con_ref=7"));
    }

    /// RecNo 57: r-dsch Data Burst Message (MS→BTS) carrying SMS
    /// Simulated SMS content matching SO6 Data Burst format.
    #[test]
    fn test_rdsch_data_burst_sms_trace_recno_57() {
        use super::RdschPdu;

        let mut bs = Bitstream::new();
        bs.write_u32(
            MessageId::DataBurst
                .wire_type(WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        ); // MSG_TYPE = Data Burst
        bs.write_u32(0, 3); // ACK_SEQ = 0
        bs.write_u32(1, 3); // MSG_SEQ = 1
        bs.write_u32(1, 1); // ACK_REQ = 1
        bs.write_u32(0, 2); // ENCRYPTION = 0
        // Data Burst SDU (same layout as access channel)
        bs.write_u32(1, 8); // MSG_NUMBER = 1
        bs.write_u32(0b000011, 6); // BURST_TYPE = SMS (3)
        bs.write_u32(1, 8); // NUM_MSGS = 1
        bs.write_u32(5, 8); // NUM_FIELDS = 5
        // SMS payload: "Hello"
        for &ch in b"Hello" {
            bs.write_u32(ch as u32, 8);
        }

        let pdu = RdschPdu::decode(&bs).expect("decode Data Burst SMS");

        assert_eq!(MessageId::DataBurst, pdu.message_id);
        assert_eq!("Data Burst Message", pdu.msg_type_name());
        assert_eq!(0, pdu.arq.ack_seq);
        assert_eq!(1, pdu.arq.msg_seq);
        assert!(pdu.arq.ack_req);

        let (burst_type, msg_number, num_msgs, fields) = pdu
            .l3
            .data_burst_fields()
            .expect("expected data burst fields");
        assert_eq!(0b000011, burst_type);
        assert_eq!(1, msg_number);
        assert_eq!(1, num_msgs);
        assert_eq!(b"Hello".to_vec(), fields);
    }

    /// Full SMS flow: verify all 4 messages decode correctly in sequence.
    #[test]
    fn test_sms_flow_service_connect_to_data_burst() {
        use super::{FdschMessage, FdschPdu, RdschPdu, ServiceConnectRecord};

        // Step 1: BTS sends Service Connect (f-dsch)
        let sc_bs = build_service_connect_bitstream();
        let sc_pdu = FdschPdu::decode(&sc_bs).expect("step 1: Service Connect");
        let FdschMessage::ServiceConnect(ref sc) = sc_pdu.body else {
            panic!("expected ServiceConnect");
        };
        assert_eq!(0, sc.serv_con_seq);
        let ServiceConnectRecord::ServiceConfig(ref cfg) = sc.records[0] else {
            panic!("expected ServiceConfig");
        };
        assert_eq!(6, cfg.connection_records[0].service_option); // SO6

        // Step 2: MS responds with Service Connect Completion (r-dsch)
        let mut scc_bs = Bitstream::new();
        scc_bs.write_u32(
            MessageId::ServiceConnectCompletion
                .wire_type(WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        );
        scc_bs.write_u32(1, 3); // ACK_SEQ = 1 (acks BTS msg_seq=1)
        scc_bs.write_u32(0, 3); // MSG_SEQ = 0
        scc_bs.write_u32(1, 1); // ACK_REQ = 1
        scc_bs.write_u32(0, 2); // ENCRYPTION
        scc_bs.write_u32(0, 1); // RESERVED
        scc_bs.write_u32(0, 3); // SERV_CON_SEQ = 0 (must match SC msg)
        scc_bs.write_u32(0, 3); // PDU_PADDING
        let scc_pdu = RdschPdu::decode(&scc_bs).expect("step 2: SCC");
        assert_eq!(Some(0), scc_pdu.l3.serv_con_seq());
        // MS ACKs the BTS Service Connect (msg_seq=1) via ack_seq=1
        assert_eq!(1, scc_pdu.arq.ack_seq);

        // Step 3: BTS sends BS Ack Order (f-dsch) acknowledging SCC
        let mut ack_bs = Bitstream::new();
        ack_bs.write_u32(0x01, 8);
        ack_bs.write_u32(0, 3); // ACK_SEQ = 0 (acks MS msg_seq=0)
        ack_bs.write_u32(0, 3); // MSG_SEQ = 0
        ack_bs.write_u32(0, 1); // ACK_REQ = 0
        ack_bs.write_u32(0, 2);
        ack_bs.write_u32(0, 1); // USE_TIME
        ack_bs.write_u32(0, 6); // ACTION_TIME
        ack_bs.write_u32(16, 6); // ORDER = BS Ack
        ack_bs.write_u32(0, 3);
        ack_bs.write_u32(0, 7); // padding
        let ack_pdu = FdschPdu::decode(&ack_bs).expect("step 3: BS Ack");
        let FdschMessage::Order(ref order) = ack_pdu.body else {
            panic!("expected Order");
        };
        assert_eq!(0b010000, order.order);

        // Step 4: MS sends Data Burst with SMS (r-dsch)
        let mut db_bs = Bitstream::new();
        db_bs.write_u32(
            MessageId::DataBurst
                .wire_type(WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        );
        db_bs.write_u32(0, 3); // ACK_SEQ
        db_bs.write_u32(1, 3); // MSG_SEQ = 1
        db_bs.write_u32(1, 1); // ACK_REQ = 1
        db_bs.write_u32(0, 2);
        db_bs.write_u32(1, 8); // MSG_NUMBER
        db_bs.write_u32(0b000011, 6); // BURST_TYPE = SMS
        db_bs.write_u32(1, 8); // NUM_MSGS
        db_bs.write_u32(3, 8); // NUM_FIELDS
        for &ch in b"Hi!" {
            db_bs.write_u32(ch as u32, 8);
        }
        let db_pdu = RdschPdu::decode(&db_bs).expect("step 4: Data Burst");
        let (burst_type, _, _, fields) = db_pdu.l3.data_burst_fields().expect("data burst fields");
        assert_eq!(0b000011, burst_type); // SMS
        assert_eq!(b"Hi!".to_vec(), fields);
    }

    /// Round-trip: encode a Service Connect SDU with `ServiceConnectParams`,
    /// wrap it in the f-dsch PDU header, then decode and verify it matches the
    /// reference trace (RecNo 54).
    #[test]
    fn test_service_connect_encoder_roundtrip() {
        use super::{FdschMessage, FdschPdu, ServiceConnectRecord};
        use crate::lac::paging_messages::{
            NonNegServiceConfig, ServiceConnectConnectionRecord as EncConnRec, ServiceConnectParams,
        };

        let params = ServiceConnectParams {
            serv_con_seq: 0,
            use_old_serv_config: 0,
            for_mux_option: 0x0001,
            rev_mux_option: 0x0001,
            for_rates: 0xF0,
            rev_rates: 0xF0,
            sync_id: None,
            connections: vec![EncConnRec {
                con_ref: 0,
                service_option: 6,
                for_traffic: 1,
                rev_traffic: 1,
                ui_encrypt_mode: 0,
                sr_id: 1,
                rlp_info_incl: false,
                rlp_blob: None,
                qos_parms: None,
            }],
            fch_frame_size: 0,
            for_fch_rc: 3,
            rev_fch_rc: 3,
            call_assignments: Vec::new(),
            use_type0_plcm: false,
            non_neg: Some(NonNegServiceConfig::rc3_default()),
            for_sch_config: None,
        };

        let sdu = params.to_ftch_sdu();

        // Build full f-dsch PDU with header (mimicking send_traffic_signaling)
        let mut pdu = Bitstream::new();
        pdu.write_u32(0x14, 8); // MSG_TYPE = Service Connect
        pdu.write_u32(7, 3); // ACK_SEQ = 7
        pdu.write_u32(1, 3); // MSG_SEQ = 1
        pdu.write_u32(1, 1); // ACK_REQ = 1
        pdu.write_u32(0, 2); // ENCRYPTION = 0
        pdu.extend(&sdu);

        // Decode and verify matches the reference trace
        let decoded = FdschPdu::decode(&pdu).expect("decode encoded Service Connect");
        assert_eq!(0x14, decoded.raw_msg_type);
        assert_eq!(7, decoded.arq.ack_seq);
        assert_eq!(1, decoded.arq.msg_seq);

        let FdschMessage::ServiceConnect(ref sc) = decoded.body else {
            panic!("expected ServiceConnect body");
        };
        assert_eq!(0, sc.serv_con_seq);
        assert_eq!(0, sc.use_old_serv_config);
        assert!(sc.sync_id.is_none());
        assert_eq!(2, sc.records.len());
        assert!(sc.call_assignments.is_empty());
        assert!(!sc.use_type0_plcm);

        let ServiceConnectRecord::ServiceConfig(ref cfg) = sc.records[0] else {
            panic!("expected ServiceConfig");
        };
        assert_eq!(0x0001, cfg.for_mux_option);
        assert_eq!(0x0001, cfg.rev_mux_option);
        assert_eq!(0xF0, cfg.for_rates);
        assert_eq!(0xF0, cfg.rev_rates);
        assert_eq!(1, cfg.connection_records.len());
        assert_eq!(6, cfg.connection_records[0].service_option);
        assert_eq!(1, cfg.connection_records[0].for_traffic);
        assert_eq!(1, cfg.connection_records[0].rev_traffic);
        assert_eq!(0, cfg.connection_records[0].ui_encrypt_mode);
        assert_eq!(1, cfg.connection_records[0].sr_id);
        assert!(cfg.fch_cc_incl);
        assert_eq!(Some(0), cfg.fch_frame_size);
        assert_eq!(Some(3), cfg.for_fch_rc);
        assert_eq!(Some(3), cfg.rev_fch_rc);

        let ServiceConnectRecord::NonNegServiceConfig(ref nn) = sc.records[1] else {
            panic!("expected NonNegServiceConfig");
        };
        assert_eq!(vec![0x84, 0x40, 0x0A, 0x08, 0x00], nn.raw_bytes);
    }

    #[test]
    fn test_fdsch_service_connect_rejects_service_config_record_len_overrun() {
        use super::FdschPdu;
        use crate::lac::paging_messages::{
            ServiceConnectConnectionRecord as EncConnRec, ServiceConnectParams,
        };

        let params = ServiceConnectParams {
            serv_con_seq: 0,
            use_old_serv_config: 0,
            for_mux_option: 0x0001,
            rev_mux_option: 0x0001,
            for_rates: 0xF0,
            rev_rates: 0xF0,
            sync_id: None,
            connections: vec![EncConnRec {
                con_ref: 0,
                service_option: 6,
                for_traffic: 1,
                rev_traffic: 1,
                ui_encrypt_mode: 0,
                sr_id: 1,
                rlp_info_incl: false,
                rlp_blob: None,
                qos_parms: None,
            }],
            fch_frame_size: 0,
            for_fch_rc: 3,
            rev_fch_rc: 3,
            call_assignments: Vec::new(),
            use_type0_plcm: false,
            non_neg: None,
            for_sch_config: None,
        };
        let mut sdu_bits = params.to_ftch_sdu().bits().to_vec();

        let record_len_offset = 15 + 8; // SC fixed header + RECORD_TYPE
        let record_len = sdu_bits[record_len_offset..record_len_offset + 8]
            .iter()
            .fold(0u8, |acc, bit| (acc << 1) | (*bit & 1));
        for i in 0..8 {
            sdu_bits[record_len_offset + i] = ((record_len + 1) >> (7 - i)) & 1;
        }
        let service_config_end = record_len_offset + 8 + record_len as usize * 8;
        sdu_bits.splice(service_config_end..service_config_end, [0u8; 8]);

        let mut pdu = Bitstream::new();
        pdu.write_u32(
            MessageId::ServiceConnect
                .wire_type(WireChannel::ForwardDedicated)
                .unwrap() as u32,
            8,
        );
        pdu.write_u32(0, 3); // ACK_SEQ
        pdu.write_u32(0, 3); // MSG_SEQ
        pdu.write_u32(0, 1); // ACK_REQ
        pdu.write_u32(0, 2); // ENCRYPTION
        pdu.extend(&Bitstream::new_init(&sdu_bits));

        let err = FdschPdu::decode(&pdu).unwrap_err();

        assert!(err.contains("Service Configuration record has trailing octets"));
    }

    #[test]
    fn test_service_connect_encoder_roundtrip_with_packet_data_fields() {
        use super::{FdschMessage, FdschPdu, ServiceConnectRecord};
        use crate::lac::paging_messages::{
            NonNegServiceConfig, ServiceConnectConnectionRecord as EncConnRec, ServiceConnectParams,
        };

        let params = ServiceConnectParams {
            serv_con_seq: 3,
            use_old_serv_config: 0,
            for_mux_option: 0x0001,
            rev_mux_option: 0x0001,
            for_rates: 0xF0,
            rev_rates: 0xF0,
            sync_id: None,
            connections: vec![EncConnRec {
                con_ref: 0,
                service_option: 7,
                for_traffic: 1,
                rev_traffic: 1,
                ui_encrypt_mode: 0,
                sr_id: 2,
                rlp_info_incl: true,
                rlp_blob: Some(vec![0x12, 0x34, 0x56]),
                qos_parms: Some(vec![0xAA, 0xBB]),
            }],
            fch_frame_size: 0,
            for_fch_rc: 1,
            rev_fch_rc: 1,
            call_assignments: Vec::new(),
            use_type0_plcm: false,
            non_neg: Some(NonNegServiceConfig::rc1_default()),
            for_sch_config: None,
        };

        let sdu = params.to_ftch_sdu();

        let mut pdu = Bitstream::new();
        pdu.write_u32(0x14, 8); // MSG_TYPE = Service Connect
        pdu.write_u32(5, 3); // ACK_SEQ
        pdu.write_u32(2, 3); // MSG_SEQ
        pdu.write_u32(1, 1); // ACK_REQ
        pdu.write_u32(0, 2); // ENCRYPTION
        pdu.extend(&sdu);

        let decoded = FdschPdu::decode(&pdu).expect("decode encoded packet-data Service Connect");
        let FdschMessage::ServiceConnect(ref sc) = decoded.body else {
            panic!("expected ServiceConnect body");
        };
        assert_eq!(sc.use_old_serv_config, 0);
        assert!(sc.sync_id.is_none());
        assert!(sc.call_assignments.is_empty());
        assert!(!sc.use_type0_plcm);
        let ServiceConnectRecord::ServiceConfig(ref cfg) = sc.records[0] else {
            panic!("expected ServiceConfig");
        };
        let conn = cfg
            .connection_records
            .first()
            .expect("expected one connection record");

        assert_eq!(sc.serv_con_seq, 3);
        assert_eq!(conn.service_option, 7);
        assert_eq!(conn.sr_id, 2);
        assert!(conn.rlp_info_incl);
        assert_eq!(conn.rlp_blob.as_deref(), Some(&[0x12, 0x34, 0x56][..]));
        assert_eq!(conn.qos_parms.as_deref(), Some(&[0xAA, 0xBB][..]));
    }

    /// r-dsch Power Measurement Report Message (PMRM) - basic decode with
    /// 2 pilots, no DCCH, no SCH.
    #[test]
    fn test_rdsch_pmrm_basic() {
        use super::RdschPdu;

        let mut bs = Bitstream::new();
        // r-dsch header
        bs.write_u32(
            MessageId::PowerMeasurementReport
                .wire_type(WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        ); // MSG_TYPE = 0x06
        bs.write_u32(2, 3); // ACK_SEQ = 2
        bs.write_u32(1, 3); // MSG_SEQ = 1
        bs.write_u32(0, 1); // ACK_REQ = 0
        bs.write_u32(0, 2); // ENCRYPTION = 0

        // PMRM SDU
        bs.write_u32(3, 5); // ERRORS_DETECTED = 3
        bs.write_u32(100, 10); // PWR_MEAS_FRAMES = 100
        bs.write_u32(1, 2); // LAST_HDM_SEQ = 1
        bs.write_u32(2, 4); // NUM_PILOTS = 2
        bs.write_u32(40, 6); // PILOT_STRENGTH[0] = 40
        bs.write_u32(25, 6); // PILOT_STRENGTH[1] = 25
        bs.write_u32(0, 1); // DCCH_PWR_MEAS_INCL = 0
        bs.write_u32(0, 1); // SCH_PWR_MEAS_INCL = 0

        let pdu = RdschPdu::decode(&bs).expect("decode PMRM");
        assert_eq!(MessageId::PowerMeasurementReport, pdu.message_id);
        assert_eq!(0x06, pdu.raw_msg_type);
        assert_eq!("Power Measurement Report Message", pdu.msg_type_name());
        assert_eq!(2, pdu.arq.ack_seq);
        assert_eq!(1, pdu.arq.msg_seq);
        assert!(!pdu.arq.ack_req);

        let AccessMessage::PowerMeasurementReport(ref m) = pdu.l3 else {
            panic!("expected PowerMeasurementReport, got {:?}", pdu.l3);
        };
        assert_eq!(3, m.errors_detected);
        assert_eq!(100, m.pwr_meas_frames);
        assert_eq!(1, m.last_hdm_seq);
        assert_eq!(vec![40, 25], m.pilot_strengths);
        assert!(!m.dcch_pwr_meas_incl);
        assert_eq!(None, m.dcch_pwr_meas_frames);
        assert_eq!(None, m.dcch_errors_detected);
        assert!(!m.sch_pwr_meas_incl);
        assert_eq!(None, m.sch_id);
        assert_eq!(None, m.sch_pwr_meas_frames);
        assert_eq!(None, m.sch_errors_detected);

        let summary = pdu.summary();
        assert!(summary.contains("PMRM(errors=3, frames=100"));
    }

    /// r-dsch PMRM with DCCH and SCH fields present.
    #[test]
    fn test_rdsch_pmrm_dcch_and_sch() {
        use super::RdschPdu;

        let mut bs = Bitstream::new();
        bs.write_u32(0x06, 8); // MSG_TYPE = PMRM
        bs.write_u32(0, 3); // ACK_SEQ
        bs.write_u32(0, 3); // MSG_SEQ
        bs.write_u32(1, 1); // ACK_REQ = 1
        bs.write_u32(0, 2); // ENCRYPTION

        // PMRM SDU
        bs.write_u32(5, 5); // ERRORS_DETECTED = 5
        bs.write_u32(200, 10); // PWR_MEAS_FRAMES = 200
        bs.write_u32(3, 2); // LAST_HDM_SEQ = 3 (none received)
        bs.write_u32(1, 4); // NUM_PILOTS = 1
        bs.write_u32(63, 6); // PILOT_STRENGTH[0] = 63 (max)

        // DCCH fields
        bs.write_u32(1, 1); // DCCH_PWR_MEAS_INCL = 1
        bs.write_u32(50, 10); // DCCH_PWR_MEAS_FRAMES = 50
        bs.write_u32(2, 5); // DCCH_ERRORS_DETECTED = 2

        // SCH fields
        bs.write_u32(1, 1); // SCH_PWR_MEAS_INCL = 1
        bs.write_u32(0, 1); // SCH_ID = 0
        bs.write_u32(1000, 16); // SCH_PWR_MEAS_FRAMES = 1000
        bs.write_u32(10, 10); // SCH_ERRORS_DETECTED = 10

        let pdu = RdschPdu::decode(&bs).expect("decode PMRM with DCCH+SCH");
        let AccessMessage::PowerMeasurementReport(ref m) = pdu.l3 else {
            panic!("expected PowerMeasurementReport, got {:?}", pdu.l3);
        };
        assert_eq!(5, m.errors_detected);
        assert_eq!(200, m.pwr_meas_frames);
        assert_eq!(3, m.last_hdm_seq);
        assert_eq!(vec![63], m.pilot_strengths);

        assert!(m.dcch_pwr_meas_incl);
        assert_eq!(Some(50), m.dcch_pwr_meas_frames);
        assert_eq!(Some(2), m.dcch_errors_detected);

        assert!(m.sch_pwr_meas_incl);
        assert_eq!(Some(0), m.sch_id);
        assert_eq!(Some(1000), m.sch_pwr_meas_frames);
        assert_eq!(Some(10), m.sch_errors_detected);

        let summary = pdu.l3.summary();
        assert!(summary.contains("dcch_frames=50"));
        assert!(summary.contains("sch_frames=1000"));
    }

    /// r-dsch PMRM with zero pilots, no optional sections.
    #[test]
    fn test_rdsch_pmrm_zero_pilots() {
        use super::RdschPdu;

        let mut bs = Bitstream::new();
        bs.write_u32(0x06, 8); // MSG_TYPE = PMRM
        bs.write_u32(0, 3);
        bs.write_u32(0, 3);
        bs.write_u32(0, 1);
        bs.write_u32(0, 2);

        bs.write_u32(0, 5); // ERRORS_DETECTED = 0
        bs.write_u32(0, 10); // PWR_MEAS_FRAMES = 0
        bs.write_u32(3, 2); // LAST_HDM_SEQ = 3
        bs.write_u32(0, 4); // NUM_PILOTS = 0
        bs.write_u32(0, 1); // DCCH_PWR_MEAS_INCL = 0
        bs.write_u32(0, 1); // SCH_PWR_MEAS_INCL = 0

        let pdu = RdschPdu::decode(&bs).expect("decode PMRM zero pilots");
        let AccessMessage::PowerMeasurementReport(ref m) = pdu.l3 else {
            panic!("expected PowerMeasurementReport");
        };
        assert_eq!(0, m.errors_detected);
        assert_eq!(0, m.pwr_meas_frames);
        assert!(m.pilot_strengths.is_empty());
        assert!(!m.dcch_pwr_meas_incl);
        assert!(!m.sch_pwr_meas_incl);
    }
}

// ---------------------------------------------------------------------------
// r-dsch (Reverse Dedicated Signaling Channel) PDU decoder
// ---------------------------------------------------------------------------
//
// C.S0004-E 2.7.1.3.2.1: Regular PDUs on the r-dsch have a simpler format
// than access channel PDUs:
//   MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2)
//   + message-specific fields + PDU_PADDING
//
// No PD field, no addressing, no USE_TIME/ACTION_TIME.

// Wire MSG_TYPE values for r-dsch are now in MessageId::wire_reverse_dedicated()
// and MessageId::from_wire(WireChannel::ReverseDedicated, raw).

/// Decoded r-dsch ARQ header fields.
#[derive(Debug, Clone)]
pub struct RdschArqHeader {
    pub ack_seq: u8,
    pub msg_seq: u8,
    pub ack_req: bool,
    pub encryption: u8,
}

/// Decoded r-dsch PDU.
#[derive(Debug, Clone)]
pub struct RdschPdu {
    pub message_id: MessageId,
    /// Raw wire MSG_TYPE value from the r-dsch PDU.
    pub raw_msg_type: u8,
    pub arq: RdschArqHeader,
    pub l3: AccessMessage,
}

impl RdschPdu {
    /// Decode a reverse dedicated signaling channel PDU from raw bits
    /// (after SAR reassembly has stripped MSG_LENGTH and CRC-16).
    pub fn decode(data: &Bitstream) -> Result<Self, String> {
        let mut bs = data.clone();
        if bs.len() < 17 {
            return Err(format!("r-dsch PDU too short: {} bits", bs.len()));
        }

        let msg_type = bs.read_bits(8).map_err(|e| e.to_string())? as u8;
        let ack_seq = bs.read_bits(3).map_err(|e| e.to_string())? as u8;
        let msg_seq = bs.read_bits(3).map_err(|e| e.to_string())? as u8;
        let ack_req = bs.read_bits(1).map_err(|e| e.to_string())? != 0;
        let encryption = bs.read_bits(2).map_err(|e| e.to_string())? as u8;

        let arq = RdschArqHeader {
            ack_seq,
            msg_seq,
            ack_req,
            encryption,
        };

        let message_id = MessageId::from_wire(WireChannel::ReverseDedicated, msg_type)
            .ok_or_else(|| format!("unsupported r-dsch MSG_TYPE 0x{msg_type:02X}"))?;

        // Build a header for L3 decoders that expect an AccessMessageHeader.
        let header = AccessMessageHeader { pd: 0, message_id };

        let l3 = match message_id {
            MessageId::Order => decode_order(header, &mut bs)?,
            MessageId::AuthChallengeResponse => finish_rdsch_access_padding(
                decode_auth_challenge_response(header, &mut bs)?,
                &mut bs,
                "AUCRM",
            )?,
            MessageId::FlashWithInfo => decode_flash_with_info(header, &mut bs)?,
            MessageId::DataBurst => decode_data_burst(header, &mut bs)?,
            MessageId::SendBurstDtmf => decode_send_burst_dtmf(header, &mut bs)?,
            MessageId::Status => decode_status_message(header, &mut bs)?,
            MessageId::OriginationContinuation => decode_origination_continuation(header, &mut bs)?,
            MessageId::HandoffCompletion => decode_handoff_completion(header, &mut bs)?,
            MessageId::ParametersResponse => decode_parameters_response(header, &mut bs)?,
            MessageId::StatusResponse => finish_rdsch_access_padding(
                decode_status_response(header, &mut bs)?,
                &mut bs,
                "STRPM",
            )?,
            MessageId::TmsiAssignmentCompletion => finish_rdsch_access_padding(
                decode_no_field_message(header, &mut bs, AccessMessage::TmsiAssignmentCompletion)?,
                &mut bs,
                "TACM",
            )?,
            MessageId::DeviceInformation => finish_rdsch_access_padding(
                decode_device_information(header, &mut bs)?,
                &mut bs,
                "DIM",
            )?,
            MessageId::SecurityModeRequest => finish_rdsch_access_padding(
                decode_rdsch_security_mode_request(header, &mut bs)?,
                &mut bs,
                "SMRM",
            )?,
            MessageId::AuthResponse => finish_rdsch_access_padding(
                decode_auth_response(header, &mut bs)?,
                &mut bs,
                "AURSPM",
            )?,
            MessageId::AuthResync => finish_rdsch_access_padding(
                decode_auth_resync(header, &mut bs)?,
                &mut bs,
                "AURSYNM",
            )?,
            MessageId::ServiceOptionControl => decode_service_option_control(header, &mut bs)?,
            MessageId::SupplementalChannelRequest => {
                decode_supplemental_channel_request(header, &mut bs)?
            }
            MessageId::CandidateFreqSearchResponse => {
                decode_candidate_freq_search_response(header, &mut bs)?
            }
            MessageId::CandidateFreqSearchReport => {
                decode_candidate_freq_search_report(header, &mut bs)?
            }
            MessageId::PeriodicPsmm => decode_periodic_psmm(header, &mut bs)?,
            MessageId::OuterLoopReport => decode_outer_loop_report(header, &mut bs)?,
            MessageId::ResourceRequest => decode_resource_request(header, &mut bs)?,
            MessageId::ExtReleaseResponse => decode_ext_release_response(header, &mut bs)?,
            MessageId::GeneralExtension => decode_general_extension(header, &mut bs)?,
            MessageId::ServiceConnectCompletion => {
                let _reserved = read(&mut bs, 1, "RESERVED")?;
                let serv_con_seq = read(&mut bs, 3, "SERV_CON_SEQ")? as u8;
                AccessMessage::ServiceConnectCompletion(ServiceConnectCompletionMessage {
                    serv_con_seq,
                })
            }
            MessageId::Psmm => decode_rdsch_psmm(&mut bs)?,
            MessageId::PowerMeasurementReport => decode_rdsch_pmrm(&mut bs)?,
            MessageId::ServiceRequest => decode_rdsch_service_request(&mut bs)?,
            MessageId::ServiceResponse => decode_rdsch_service_response(&mut bs)?,
            _ => {
                return Err(format!(
                    "unsupported r-dsch body decode for {}",
                    message_id.tag()
                ));
            }
        };

        Ok(RdschPdu {
            message_id,
            raw_msg_type: msg_type,
            arq,
            l3,
        })
    }

    pub fn msg_type_name(&self) -> &'static str {
        self.message_id.name()
    }

    pub fn summary(&self) -> String {
        let l3_sum = self
            .l3
            .summary_with_rejected_forward_channel(WireChannel::ForwardDedicated);
        format!(
            "r-dsch {}(0x{:02X}) ack_seq={} msg_seq={} ack_req={} enc={} | {}",
            self.msg_type_name(),
            self.raw_msg_type,
            self.arq.ack_seq,
            self.arq.msg_seq,
            self.arq.ack_req as u8,
            self.arq.encryption,
            l3_sum,
        )
    }
}

pub fn rdsch_msg_type_name(raw: u8) -> &'static str {
    MessageId::from_wire(WireChannel::ReverseDedicated, raw)
        .map_or("Unknown r-dsch Message", |m| m.name())
}

/// Decode Pilot Strength Measurement Message (PSMM).
/// C.S0004-E 2.7.2.3.2.4.
///
/// Fields: REF_PN(9) + PILOT_STRENGTH(6) + KEEP(1) +
///   { PILOT_PN_PHASE(15) + PILOT_STRENGTH(6) + KEEP(1) }*
fn decode_rdsch_psmm(bs: &mut Bitstream) -> Result<AccessMessage, String> {
    if bs.len() < 16 {
        return Err(format!("PSMM too short: {} bits", bs.len()));
    }
    let ref_pn = read(bs, 9, "REF_PN")? as u16;
    let pilot_strength = read(bs, 6, "PILOT_STRENGTH")? as u8;
    let keep = read(bs, 1, "KEEP")? != 0;

    let mut pilots = Vec::new();
    // Each additional pilot report is 22 bits (15 + 6 + 1)
    while bs.len() >= 22 {
        let pilot_pn_phase = read(bs, 15, "PILOT_PN_PHASE")? as u16;
        let ps = read(bs, 6, "PILOT_STRENGTH")? as u8;
        let k = read(bs, 1, "KEEP")? != 0;
        pilots.push(PilotReport {
            pilot_pn_phase,
            pilot_strength: ps,
            keep: k,
        });
    }

    Ok(AccessMessage::PilotStrengthMeasurement(
        PilotStrengthMeasurementMessage {
            ref_pn,
            pilot_strength,
            keep,
            pilots,
        },
    ))
}

/// Decode Power Measurement Report Message (PMRM).
/// C.S0005-E 2.7.2.3.2.6.
///
/// Fields:
///   ERRORS_DETECTED(5) + PWR_MEAS_FRAMES(10) + LAST_HDM_SEQ(2) +
///   NUM_PILOTS(4) + { PILOT_STRENGTH(6) } * NUM_PILOTS +
///   DCCH_PWR_MEAS_INCL(1) +
///   [ DCCH_PWR_MEAS_FRAMES(10) + DCCH_ERRORS_DETECTED(5) if incl ] +
///   SCH_PWR_MEAS_INCL(1) +
///   [ SCH_ID(1) + SCH_PWR_MEAS_FRAMES(16) + SCH_ERRORS_DETECTED(10) if incl ]
fn decode_rdsch_pmrm(bs: &mut Bitstream) -> Result<AccessMessage, String> {
    // Minimum: 5+10+2+4+1+1 = 23 bits (0 pilots, no DCCH, no SCH)
    if bs.len() < 23 {
        return Err(format!("PMRM too short: {} bits", bs.len()));
    }

    let errors_detected = read(bs, 5, "ERRORS_DETECTED")? as u8;
    let pwr_meas_frames = read(bs, 10, "PWR_MEAS_FRAMES")? as u16;
    let last_hdm_seq = read(bs, 2, "LAST_HDM_SEQ")? as u8;
    let num_pilots = read(bs, 4, "NUM_PILOTS")? as usize;

    let mut pilot_strengths = Vec::with_capacity(num_pilots);
    for i in 0..num_pilots {
        let ps = read(bs, 6, &format!("PILOT_STRENGTH[{}]", i))? as u8;
        pilot_strengths.push(ps);
    }

    let dcch_pwr_meas_incl = read(bs, 1, "DCCH_PWR_MEAS_INCL")? != 0;
    let (dcch_pwr_meas_frames, dcch_errors_detected) = if dcch_pwr_meas_incl {
        let frames = read(bs, 10, "DCCH_PWR_MEAS_FRAMES")? as u16;
        let errors = read(bs, 5, "DCCH_ERRORS_DETECTED")? as u8;
        (Some(frames), Some(errors))
    } else {
        (None, None)
    };

    let sch_pwr_meas_incl = read(bs, 1, "SCH_PWR_MEAS_INCL")? != 0;
    let (sch_id, sch_pwr_meas_frames, sch_errors_detected) = if sch_pwr_meas_incl {
        let id = read(bs, 1, "SCH_ID")? as u8;
        let frames = read(bs, 16, "SCH_PWR_MEAS_FRAMES")? as u16;
        let errors = read(bs, 10, "SCH_ERRORS_DETECTED")? as u16;
        (Some(id), Some(frames), Some(errors))
    } else {
        (None, None, None)
    };

    Ok(AccessMessage::PowerMeasurementReport(
        PowerMeasurementReportMessage {
            errors_detected,
            pwr_meas_frames,
            last_hdm_seq,
            pilot_strengths,
            dcch_pwr_meas_incl,
            dcch_pwr_meas_frames,
            dcch_errors_detected,
            sch_pwr_meas_incl,
            sch_id,
            sch_pwr_meas_frames,
            sch_errors_detected,
        },
    ))
}

// ---------------------------------------------------------------------------
// f-dsch (Forward Dedicated Signaling Channel) PDU decoder
// ---------------------------------------------------------------------------
//
// C.S0004-E 3.7.3.3.2: Regular PDUs on the f-dsch use the same ARQ header
// as r-dsch:
//   MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2)
//
// Unlike r-dsch, many f-dsch message SDUs include USE_TIME(1) + ACTION_TIME(6)
// before the message-specific fields.

// Wire MSG_TYPE values for f-dsch are now in MessageId::wire_forward_dedicated()
// and MessageId::from_wire(WireChannel::ForwardDedicated, raw).

/// Connection record in a Service Connect Message.
#[derive(Debug, Clone)]
pub struct ServiceConnectConnectionRecord {
    pub con_ref: u8,
    pub service_option: u16,
    pub for_traffic: u8,
    pub rev_traffic: u8,
    pub ui_encrypt_mode: u8,
    pub sr_id: u8,
    pub rlp_info_incl: bool,
    pub rlp_blob: Option<Vec<u8>>,
    pub qos_parms: Option<Vec<u8>>,
}

/// Call-assignment entry carried in a Service Connect Message.
#[derive(Debug, Clone)]
pub struct ServiceConnectCallAssignment {
    pub con_ref: u8,
    pub response_ind: bool,
    pub tag: Option<u8>,
    pub bypass_alert_answer: Option<bool>,
}

/// Service configuration record (RECORD_TYPE=0x07) in a Service Connect Message.
#[derive(Debug, Clone)]
pub struct ServiceConfigRecord {
    pub for_mux_option: u16,
    pub rev_mux_option: u16,
    pub for_rates: u8,
    pub rev_rates: u8,
    pub connection_records: Vec<ServiceConnectConnectionRecord>,
    pub fch_cc_incl: bool,
    pub fch_frame_size: Option<u8>,
    pub for_fch_rc: Option<u8>,
    pub rev_fch_rc: Option<u8>,
    pub dcch_cc_incl: bool,
    pub for_sch_cc_incl: bool,
    pub rev_sch_cc_incl: bool,
}

/// Non-negotiable service configuration record (RECORD_TYPE=0x13).
#[derive(Debug, Clone)]
pub struct NonNegServiceConfigRecord {
    pub raw_bytes: Vec<u8>,
}

/// Type-length-value record in a Service Connect Message.
#[derive(Debug, Clone)]
pub enum ServiceConnectRecord {
    ServiceConfig(ServiceConfigRecord),
    NonNegServiceConfig(NonNegServiceConfigRecord),
    Unknown { record_type: u8, data: Vec<u8> },
}

/// Decoded Service Connect Message SDU.
#[derive(Debug, Clone)]
pub struct ServiceConnectMessage {
    pub use_time: bool,
    pub action_time: u8,
    pub serv_con_seq: u8,
    pub use_old_serv_config: u8,
    pub sync_id: Option<Vec<u8>>,
    pub records: Vec<ServiceConnectRecord>,
    pub call_assignments: Vec<ServiceConnectCallAssignment>,
    pub use_type0_plcm: bool,
}

/// Decoded f-dsch order SDU (includes USE_TIME/ACTION_TIME).
#[derive(Debug, Clone)]
pub struct FdschOrderMessage {
    pub use_time: bool,
    pub action_time: u8,
    pub order: u8,
    pub add_record_len: u8,
    pub order_specific: Vec<u8>,
    pub con_ref_incl: Option<bool>,
    pub con_ref: Option<u8>,
}

impl FdschOrderMessage {
    pub fn order_name(&self) -> &'static str {
        forward_dedicated_order_name(self.order)
    }
}

/// Decoded f-dsch message body.
#[derive(Debug, Clone)]
pub enum FdschMessage {
    Order(FdschOrderMessage),
    DataBurst(DataBurstMessage),
    ServiceRequest(ServiceRequestMessage),
    ServiceResponse(ServiceResponseMessage),
    ServiceConnect(ServiceConnectMessage),
}

/// Decoded f-dsch PDU.
#[derive(Debug, Clone)]
pub struct FdschPdu {
    pub message_id: MessageId,
    /// Raw wire MSG_TYPE value from the f-dsch PDU.
    pub raw_msg_type: u8,
    pub arq: RdschArqHeader,
    pub body: FdschMessage,
}

impl FdschPdu {
    /// Decode a forward dedicated signaling channel PDU from raw bits
    /// (after SAR reassembly has stripped MSG_LENGTH and CRC-16).
    pub fn decode(data: &Bitstream) -> Result<Self, String> {
        let mut bs = data.clone();
        if bs.len() < 18 {
            return Err(format!("f-dsch PDU too short: {} bits", bs.len()));
        }

        let msg_type = bs.read_bits(8).map_err(|e| e.to_string())? as u8;
        let ack_seq = bs.read_bits(3).map_err(|e| e.to_string())? as u8;
        let msg_seq = bs.read_bits(3).map_err(|e| e.to_string())? as u8;
        let ack_req = bs.read_bits(1).map_err(|e| e.to_string())? != 0;
        let encryption = bs.read_bits(2).map_err(|e| e.to_string())? as u8;

        let arq = RdschArqHeader {
            ack_seq,
            msg_seq,
            ack_req,
            encryption,
        };

        let message_id = MessageId::from_wire(WireChannel::ForwardDedicated, msg_type)
            .ok_or_else(|| format!("unsupported f-dsch MSG_TYPE 0x{msg_type:02X}"))?;

        let body = match message_id {
            MessageId::Order => decode_fdsch_order(&mut bs)?,
            MessageId::DataBurst => {
                let header = AccessMessageHeader {
                    pd: 0,
                    message_id: MessageId::DataBurst,
                };
                match decode_data_burst(header, &mut bs)? {
                    AccessMessage::DataBurst(m) => FdschMessage::DataBurst(m),
                    _ => return Err("f-dsch DBM decoder returned non-DBM body".to_string()),
                }
            }
            MessageId::ServiceRequest => FdschMessage::ServiceRequest(decode_service_request_body(
                &mut bs,
                ServiceNegotiationDirection::Forward,
            )?),
            MessageId::ServiceResponse => FdschMessage::ServiceResponse(
                decode_service_response_body(&mut bs, ServiceNegotiationDirection::Forward)?,
            ),
            MessageId::ServiceConnect => decode_fdsch_service_connect(&mut bs)?,
            _ => {
                return Err(format!(
                    "unsupported f-dsch body decode for {}",
                    message_id.tag()
                ));
            }
        };

        Ok(FdschPdu {
            message_id,
            raw_msg_type: msg_type,
            arq,
            body,
        })
    }

    pub fn msg_type_name(&self) -> &'static str {
        self.message_id.name()
    }

    pub fn summary(&self) -> String {
        let body_sum = match &self.body {
            FdschMessage::Order(m) => {
                let con_ref = match (m.con_ref_incl, m.con_ref) {
                    (Some(incl), Some(value)) => {
                        format!(", con_ref_incl={}, con_ref={}", incl as u8, value)
                    }
                    (Some(incl), None) => format!(", con_ref_incl={}", incl as u8),
                    (None, _) => String::new(),
                };
                format!(
                    "Order(use_time={}, action_time={}, order=0b{:06b} {}, add_record_len={}, order_specific=[{:02X?}]{})",
                    m.use_time as u8,
                    m.action_time,
                    m.order,
                    m.order_name(),
                    m.add_record_len,
                    m.order_specific,
                    con_ref,
                )
            }
            FdschMessage::DataBurst(m) => format!(
                "DataBurst(msg_number={}, burst_type=0b{:06b} {}, num_msgs={}, num_fields={})",
                m.msg_number,
                m.burst_type,
                m.burst_type_name(),
                m.num_msgs,
                m.num_fields,
            ),
            FdschMessage::ServiceRequest(m) => {
                let purpose = match m.req_purpose {
                    0b0001 => "reject",
                    0b0010 => "propose",
                    _ => "reserved",
                };
                let so_str = m
                    .service_config
                    .as_ref()
                    .and_then(|cfg| cfg.connection_records.first())
                    .map(|cr| format!(", SO={}", cr.service_option))
                    .unwrap_or_default();
                format!(
                    "ServiceRequest(serv_req_seq={}, purpose={}{})",
                    m.serv_req_seq, purpose, so_str
                )
            }
            FdschMessage::ServiceResponse(m) => {
                let purpose = match m.resp_purpose {
                    0b0001 => "reject",
                    0b0010 => "propose",
                    _ => "reserved",
                };
                let so_str = m
                    .service_config
                    .as_ref()
                    .and_then(|cfg| cfg.connection_records.first())
                    .map(|cr| format!(", SO={}", cr.service_option))
                    .unwrap_or_default();
                format!(
                    "ServiceResponse(serv_req_seq={}, purpose={}{})",
                    m.serv_req_seq, purpose, so_str
                )
            }
            FdschMessage::ServiceConnect(m) => format!(
                "ServiceConnect(use_time={}, action_time={}, serv_con_seq={}, records={})",
                m.use_time as u8,
                m.action_time,
                m.serv_con_seq,
                m.records.len(),
            ),
        };
        format!(
            "f-dsch {}(0x{:02X}) ack_seq={} msg_seq={} ack_req={} enc={} | {}",
            self.msg_type_name(),
            self.raw_msg_type,
            self.arq.ack_seq,
            self.arq.msg_seq,
            self.arq.ack_req as u8,
            self.arq.encryption,
            body_sum,
        )
    }
}

fn decode_fdsch_order(bs: &mut Bitstream) -> Result<FdschMessage, String> {
    let use_time = read(bs, 1, "USE_TIME")? != 0;
    let action_time = read(bs, 6, "ACTION_TIME")? as u8;
    let order = read(bs, 6, "ORDER")? as u8;
    let add_record_len = read(bs, 3, "ADD_RECORD_LEN")? as u8;
    let mut order_specific = Vec::with_capacity(add_record_len as usize);
    for idx in 0..add_record_len {
        order_specific.push(read(bs, 8, &format!("ORDFIELD[{idx}]"))? as u8);
    }
    let con_ref_incl = if is_forward_call_control_order(order) && bs.len() > 0 {
        Some(read(bs, 1, "CON_REF_INCL")? != 0)
    } else {
        None
    };
    let con_ref = if con_ref_incl == Some(true) {
        Some(read(bs, 8, "CON_REF")? as u8)
    } else {
        None
    };
    Ok(FdschMessage::Order(FdschOrderMessage {
        use_time,
        action_time,
        order,
        add_record_len,
        order_specific,
        con_ref_incl,
        con_ref,
    }))
}

fn is_forward_call_control_order(order: u8) -> bool {
    matches!(
        order,
        0b000001 // Abbreviated Alert
            | 0b010101 // Release
            | 0b011001 // Continuous DTMF Tone
            | 0b100001 // Connect
    )
}

fn forward_dedicated_order_name(order: u8) -> &'static str {
    match order {
        0b000001 => "Abbreviated Alert",
        0b010000 => "Base Station Acknowledgment",
        0b010001 => "Pilot Measurement Request",
        0b010010 => "Lock Until Power-Cycled",
        0b010011 => "Maintenance Required",
        0b010100 => "Unlock",
        0b010101 => "Release",
        0b010110 => "Outer Loop Report Request",
        0b010111 => "Long Code Transition",
        0b011001 => "Continuous DTMF Tone",
        0b011010 => "Status Request",
        0b011011 => "Registration Accepted",
        0b011100 => "Registration Rejected",
        0b011110 => "Local Control",
        0b100001 => "Connect",
        _ => "Unknown Order",
    }
}

/// Decode a Service Configuration Record body from a bitstream.
/// Shared between forward Service Connect and service negotiation decoders.
fn decode_service_config_record(bs: &mut Bitstream) -> Result<ServiceConfigRecord, String> {
    let for_mux_option = read(bs, 16, "FOR_MUX_OPTION")? as u16;
    let rev_mux_option = read(bs, 16, "REV_MUX_OPTION")? as u16;
    let for_rates = read(bs, 8, "FOR_RATES")? as u8;
    let rev_rates = read(bs, 8, "REV_RATES")? as u8;
    let num_con_rec = read(bs, 8, "NUM_CON_REC")? as u8;

    let mut connection_records = Vec::with_capacity(num_con_rec as usize);
    for _ in 0..num_con_rec {
        let rec_len = read(bs, 8, "CON_RECORD_LEN")? as usize;
        if rec_len == 0 {
            return Err("CON_RECORD_LEN must include at least the length octet".to_string());
        }
        let body_len = rec_len - 1;
        let mut rec_bytes = Vec::with_capacity(body_len);
        for _ in 0..body_len {
            rec_bytes.push(read(bs, 8, "CON_RECORD_BYTE")? as u8);
        }
        let mut rec_bs = Bitstream::new_bytes(&rec_bytes);

        let con_ref = read(&mut rec_bs, 8, "CON_REF")? as u8;
        let service_option = read(&mut rec_bs, 16, "SERVICE_OPTION")? as u16;
        let for_traffic = read(&mut rec_bs, 4, "FOR_TRAFFIC")? as u8;
        let rev_traffic = read(&mut rec_bs, 4, "REV_TRAFFIC")? as u8;
        let ui_encrypt_mode = read(&mut rec_bs, 3, "UI_ENCRYPT_MODE")? as u8;
        let sr_id = read(&mut rec_bs, 3, "SR_ID")? as u8;
        let rlp_info_incl = read(&mut rec_bs, 1, "RLP_INFO_INCL")? != 0;
        let rlp_blob = if rlp_info_incl {
            let len = read(&mut rec_bs, 4, "RLP_BLOB_LEN")? as usize;
            let mut blob = Vec::with_capacity(len);
            for _ in 0..len {
                blob.push(read(&mut rec_bs, 8, "RLP_BLOB_BYTE")? as u8);
            }
            Some(blob)
        } else {
            None
        };
        let qos_parms_incl = read(&mut rec_bs, 1, "QOS_PARMS_INCL")? != 0;
        let qos_parms = if qos_parms_incl {
            let len = read(&mut rec_bs, 5, "QOS_PARMS_LEN")? as usize;
            let mut qos = Vec::with_capacity(len);
            for _ in 0..len {
                qos.push(read(&mut rec_bs, 8, "QOS_PARMS_BYTE")? as u8);
            }
            Some(qos)
        } else {
            None
        };

        connection_records.push(ServiceConnectConnectionRecord {
            con_ref,
            service_option,
            for_traffic,
            rev_traffic,
            ui_encrypt_mode,
            sr_id,
            rlp_info_incl,
            rlp_blob,
            qos_parms,
        });
    }

    let fch_cc_incl = read(bs, 1, "FCH_CC_INCL")? != 0;
    let (fch_frame_size, for_fch_rc, rev_fch_rc) = if fch_cc_incl {
        let frame_size = read(bs, 1, "FCH_FRAME_SIZE")? as u8;
        let for_rc = read(bs, 5, "FOR_FCH_RC")? as u8;
        let rev_rc = read(bs, 5, "REV_FCH_RC")? as u8;
        (Some(frame_size), Some(for_rc), Some(rev_rc))
    } else {
        (None, None, None)
    };

    let dcch_cc_incl = read(bs, 1, "DCCH_CC_INCL")? != 0;
    if dcch_cc_incl {
        // Skip DCCH fields — not needed for SO6 SMS flow
        let _frame_size_mode = read(bs, 2, "DCCH_FRAME_SIZE_MODE")?;
        let _for_rc = read(bs, 5, "FOR_DCCH_RC")?;
        let _rev_rc = read(bs, 5, "REV_DCCH_RC")?;
    }

    let for_sch_cc_incl = read(bs, 1, "FOR_SCH_CC_INCL")? != 0;
    let rev_sch_cc_incl = read(bs, 1, "REV_SCH_CC_INCL")? != 0;
    let _reserved = read(bs, 1, "RESERVED")?;

    Ok(ServiceConfigRecord {
        for_mux_option,
        rev_mux_option,
        for_rates,
        rev_rates,
        connection_records,
        fch_cc_incl,
        fch_frame_size,
        for_fch_rc,
        rev_fch_rc,
        dcch_cc_incl,
        for_sch_cc_incl,
        rev_sch_cc_incl,
    })
}

fn decode_service_config_record_with_len(
    bs: &mut Bitstream,
    record_len: u8,
    context: &str,
) -> Result<ServiceConfigRecord, String> {
    let record_bits = record_len as usize * 8;
    if bs.len() < record_bits {
        return Err(format!(
            "{context} RECORD_LEN={} exceeds remaining bits {}",
            record_len,
            bs.len()
        ));
    }

    let mut record_bs = bs.drain(0..record_bits);
    let config = decode_service_config_record(&mut record_bs)?;
    if record_bs.len() >= 8 {
        return Err(format!(
            "{context} Service Configuration record has trailing octets"
        ));
    }
    while !record_bs.is_empty() {
        if record_bs
            .read_bits(1)
            .map_err(|e| format!("{context} trailing padding read failed: {e}"))?
            != 0
        {
            return Err(format!(
                "{context} Service Configuration record padding must be zero"
            ));
        }
    }
    Ok(config)
}

/// Decode Reverse Service Request Message (SRQM). C.S0005-E 2.7.2.3.2.12.
fn decode_rdsch_service_request(bs: &mut Bitstream) -> Result<AccessMessage, String> {
    Ok(AccessMessage::ServiceRequest(decode_service_request_body(
        bs,
        ServiceNegotiationDirection::Reverse,
    )?))
}

#[derive(Debug, Clone, Copy)]
enum ServiceNegotiationDirection {
    Reverse,
    Forward,
}

impl ServiceNegotiationDirection {
    fn service_config_record_type(self) -> u8 {
        match self {
            ServiceNegotiationDirection::Reverse => 0x13,
            ServiceNegotiationDirection::Forward => 0x07,
        }
    }
}

fn decode_service_request_body(
    bs: &mut Bitstream,
    direction: ServiceNegotiationDirection,
) -> Result<ServiceRequestMessage, String> {
    let serv_req_seq = read(bs, 3, "SERV_REQ_SEQ")? as u8;
    let req_purpose = read(bs, 4, "REQ_PURPOSE")? as u8;
    validate_service_purpose(
        "REQ_PURPOSE",
        req_purpose,
        direction,
        "C.S0005-E 2.7.2.3.2.12/3.7.3.3.2.18",
    )?;

    let service_config = if req_purpose == 0b0010 {
        if bs.len() < 16 {
            return Err("SRQM propose requires Service Configuration record header".to_string());
        }
        let record_type = read(bs, 8, "RECORD_TYPE")? as u8;
        let record_len = read(bs, 8, "RECORD_LEN")? as u8;
        let expected = direction.service_config_record_type();
        if record_type != expected {
            return Err(format!(
                "SRQM {:?} propose requires Service Configuration RECORD_TYPE=0x{expected:02X}, got 0x{record_type:02X}",
                direction
            ));
        }
        Some(decode_service_config_record_with_len(
            bs, record_len, "SRQM",
        )?)
    } else {
        None
    };

    Ok(ServiceRequestMessage {
        serv_req_seq,
        req_purpose,
        service_config,
    })
}

/// Decode Reverse Service Response Message (SRPM). C.S0005-E 2.7.2.3.2.13.
fn decode_rdsch_service_response(bs: &mut Bitstream) -> Result<AccessMessage, String> {
    Ok(AccessMessage::ServiceResponse(
        decode_service_response_body(bs, ServiceNegotiationDirection::Reverse)?,
    ))
}

fn decode_service_response_body(
    bs: &mut Bitstream,
    direction: ServiceNegotiationDirection,
) -> Result<ServiceResponseMessage, String> {
    let serv_req_seq = read(bs, 3, "SERV_REQ_SEQ")? as u8;
    let resp_purpose = read(bs, 4, "RESP_PURPOSE")? as u8;
    validate_service_purpose(
        "RESP_PURPOSE",
        resp_purpose,
        direction,
        "C.S0005-E 2.7.2.3.2.13/3.7.3.3.2.19",
    )?;

    let service_config = if resp_purpose == 0b0010 {
        if bs.len() < 16 {
            return Err(
                "SRPM counter-propose requires Service Configuration record header".to_string(),
            );
        }
        let record_type = read(bs, 8, "RECORD_TYPE")? as u8;
        let record_len = read(bs, 8, "RECORD_LEN")? as u8;
        let expected = direction.service_config_record_type();
        if record_type != expected {
            return Err(format!(
                "SRPM {:?} counter-propose requires Service Configuration RECORD_TYPE=0x{expected:02X}, got 0x{record_type:02X}",
                direction
            ));
        }
        Some(decode_service_config_record_with_len(
            bs, record_len, "SRPM",
        )?)
    } else {
        None
    };

    Ok(ServiceResponseMessage {
        serv_req_seq,
        resp_purpose,
        service_config,
    })
}

fn validate_service_purpose(
    field: &str,
    purpose: u8,
    direction: ServiceNegotiationDirection,
    spec: &str,
) -> Result<(), String> {
    let valid = match direction {
        ServiceNegotiationDirection::Reverse => matches!(purpose, 0b0000 | 0b0001 | 0b0010),
        ServiceNegotiationDirection::Forward => matches!(purpose, 0b0001 | 0b0010),
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{field}=0b{purpose:04b} is reserved for {:?} service negotiation ({spec})",
            direction
        ))
    }
}

fn decode_fdsch_service_connect(bs: &mut Bitstream) -> Result<FdschMessage, String> {
    let use_time = read(bs, 1, "USE_TIME")? != 0;
    let action_time = read(bs, 6, "ACTION_TIME")? as u8;
    let serv_con_seq = read(bs, 3, "SERV_CON_SEQ")? as u8;
    let _reserved = read(bs, 2, "RESERVED")?;
    let use_old_serv_config = read(bs, 2, "USE_OLD_SERV_CONFIG")? as u8;
    let sr_id_restore = if use_old_serv_config == 0b11 {
        Some(read(bs, 3, "SR_ID")? as u8)
    } else {
        None
    };
    if let Some(0) = sr_id_restore {
        let _sr_id_restore_bitmap = read(bs, 6, "SR_ID_RESTORE_BITMAP")?;
    }
    let sync_id_incl = read(bs, 1, "SYNC_ID_INCL")? != 0;
    let sync_id = if sync_id_incl {
        let sync_id_len = read(bs, 4, "SYNC_ID_LEN")? as usize;
        let mut bytes = Vec::with_capacity(sync_id_len);
        for _ in 0..sync_id_len {
            bytes.push(read(bs, 8, "SYNC_ID_BYTE")? as u8);
        }
        Some(bytes)
    } else {
        None
    };

    let mut records = Vec::new();
    if use_old_serv_config != 0b01 && use_old_serv_config != 0b11 {
        while bs.len() >= 17 {
            let record_type = read(bs, 8, "RECORD_TYPE")? as u8;
            let record_len = read(bs, 8, "RECORD_LEN")? as u8;
            let record_bits = (record_len as usize) * 8;
            if bs.len() < record_bits {
                return Err(format!(
                    "RECORD_LEN={} exceeds remaining Service Connect bits {}",
                    record_len,
                    bs.len()
                ));
            }

            match record_type {
                0x07 => {
                    // Service Configuration Record
                    records.push(ServiceConnectRecord::ServiceConfig(
                        decode_service_config_record_with_len(bs, record_len, "ServiceConnect")?,
                    ));
                }
                0x13 => {
                    // Non-Negotiable Service Configuration Record — store raw bytes
                    let mut raw = Vec::with_capacity(record_len as usize);
                    for _ in 0..record_len {
                        raw.push(read(bs, 8, "NNSCREC_BYTE")? as u8);
                    }
                    records.push(ServiceConnectRecord::NonNegServiceConfig(
                        NonNegServiceConfigRecord { raw_bytes: raw },
                    ));
                }
                _ => {
                    let mut data = Vec::with_capacity(record_len as usize);
                    for _ in 0..record_len {
                        data.push(read(bs, 8, "REC_BYTE")? as u8);
                    }
                    records.push(ServiceConnectRecord::Unknown { record_type, data });
                }
            }

            if bs.len() < 1 {
                break;
            }
            let next_type = bs.clone().read_bits(8).ok();
            if let Some(next_type) = next_type {
                if next_type != 0x07 && next_type != 0x13 {
                    break;
                }
            } else {
                break;
            }
        }
    }

    let mut call_assignments = Vec::new();
    if use_old_serv_config == 0 {
        let cc_info_incl = read(bs, 1, "CC_INFO_INCL")? != 0;
        if cc_info_incl {
            let num_calls_assign = read(bs, 8, "NUM_CALLS_ASSIGN")? as usize;
            call_assignments.reserve(num_calls_assign);
            for _ in 0..num_calls_assign {
                let con_ref = read(bs, 8, "CALL_ASSIGN_CON_REF")? as u8;
                let response_ind = read(bs, 1, "CALL_ASSIGN_RESPONSE_IND")? != 0;
                let (tag, bypass_alert_answer) = if response_ind {
                    (Some(read(bs, 4, "CALL_ASSIGN_TAG")? as u8), None)
                } else {
                    (
                        None,
                        Some(read(bs, 1, "CALL_ASSIGN_BYPASS_ALERT_ANSWER")? != 0),
                    )
                };
                call_assignments.push(ServiceConnectCallAssignment {
                    con_ref,
                    response_ind,
                    tag,
                    bypass_alert_answer,
                });
            }
        }
    }
    let use_type0_plcm = read(bs, 1, "USE_TYPE0_PLCM")? != 0;
    if sync_id_incl && use_old_serv_config == 0b10 {
        let _sync_id_bs_initiated_ind = read(bs, 1, "SYNC_ID_BS_INITIATED_IND")?;
    }
    if use_old_serv_config == 0b11 {
        let sr_id_release_bitmap_incl = read(bs, 1, "SR_ID_RELEASE_BITMAP_INCL")? != 0;
        if sr_id_release_bitmap_incl {
            let _sr_id_release_bitmap = read(bs, 6, "SR_ID_RELEASE_BITMAP")?;
        }
    }

    Ok(FdschMessage::ServiceConnect(ServiceConnectMessage {
        use_time,
        action_time,
        serv_con_seq,
        use_old_serv_config,
        sync_id,
        records,
        call_assignments,
        use_type0_plcm,
    }))
}

pub fn fdsch_msg_type_name(raw: u8) -> &'static str {
    MessageId::from_wire(WireChannel::ForwardDedicated, raw)
        .map_or("Unknown f-dsch Message", |m| m.name())
}
