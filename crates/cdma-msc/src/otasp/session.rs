//! OTASP session state machine (BSC-side driver, per C.S0016-D §3.2.1 + §3.5).
//!
//! Drives the user-initiated `*228` programming sequence:
//!   Protocol Capability → Verify SPC → for each enabled write block
//!   (Config → Download) → Commit → release.
//!
//! Pure logic over OTASP message bytes. The transport (ADDS Deliver out / ADDS
//! Transfer in) is the caller's job; this module only encodes/decodes OTASP
//! Data Messages and emits an event stream plus a `SessionOutcome`.
//!
//! Per-step failures tear the session down on the first non-Accept result code
//! per `docs/otasp-plan.md` decision 6. No in-session retries.

use cdma_otasp::message::commit::{CommitRequest, CommitResponse};
use cdma_otasp::message::configuration::{ConfigurationRequest, ConfigurationResponse};
use cdma_otasp::message::download::{DownloadParamBlock, DownloadRequest, DownloadResponse};
use cdma_otasp::message::protocol_capability::{
    ProtocolCapabilityRequest, ProtocolCapabilityResponse,
};
use cdma_otasp::message::result_code::ResultCode;
use cdma_otasp::message::sspr::{SsprConfigurationRequest, SsprConfigurationResponse};
use cdma_otasp::message::system_tag::{
    SystemTagConfigRequest, SystemTagConfigResponse, SystemTagDownloadRequest,
    SystemTagDownloadResponse,
};
use cdma_otasp::message::validation::{
    ValidationParamBlock, ValidationRequest, ValidationResponse,
};
use cdma_otasp::param::nam_cdma::NamCdma;
use cdma_otasp::param::nam_cdma_analog::NamCdmaAnalog;
use cdma_otasp::param::verify_spc::VerifySpc;

use crate::config::{BtsOverheadConfig, OtaspConfig};
use crate::otasp::event::{
    BlockFeature, HardwareIdentity, OtaspEvent, PrlOutcome, PrlReadback, SessionOutcomeKind,
};
use crate::otasp::nam::{AssembledNam, NamReadback, ResolvedSubscriberInput, assemble_nam};

/// One outbound OTASP Data Message, ready for the caller to wrap in an ADDS
/// Deliver with `burst_type = 0x04`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundOtasp {
    pub bytes: Vec<u8>,
}

/// Trait the caller implements to hand the session events and outbound bytes.
pub trait OtaspTransport {
    fn send(&mut self, message: OutboundOtasp);
    fn emit(&mut self, event: OtaspEvent);
}

/// Terminal outcome of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOutcome {
    pub kind: SessionOutcomeKind,
    pub completed_blocks: u32,
}

/// NAM block IDs.
const BLOCK_CDMA_ANALOG_NAM: u8 = 0x00;
const BLOCK_MDN: u8 = 0x01;
const BLOCK_CDMA_NAM: u8 = 0x02;
const BLOCK_HOME_SYSTEM_TAG: u8 = 0x00;
const BLOCK_MMS_URI: u8 = cdma_otasp::message::mms::BLOCK_MMS_URI;

/// Validation block IDs.
const VBLOCK_VERIFY_SPC: u8 = 0x00;

/// Enumerated planned-write target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteTarget {
    CdmaAnalogNam,
    Mdn,
    CdmaNam,
    HomeSystemTag,
    MmsUri,
    Prl,
}

impl WriteTarget {
    /// On-wire BLOCK_ID at session-driver dispatch time. For PRL the
    /// 0x00/0x01 selection happens at push start based on
    /// `sspr_p_rev`; this returns 0x00 (the classic form) as the
    /// default for label/event emission before the PRL phase begins.
    fn block_id(self) -> u8 {
        match self {
            Self::CdmaAnalogNam => BLOCK_CDMA_ANALOG_NAM,
            Self::Mdn => BLOCK_MDN,
            Self::CdmaNam => BLOCK_CDMA_NAM,
            Self::HomeSystemTag => BLOCK_HOME_SYSTEM_TAG,
            Self::MmsUri => BLOCK_MMS_URI,
            Self::Prl => cdma_otasp::message::sspr::BLOCK_PRL_CLASSIC,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::CdmaAnalogNam => "CDMA/Analog NAM",
            Self::Mdn => "Mobile Directory Number",
            Self::CdmaNam => "CDMA NAM",
            Self::HomeSystemTag => "Home System Tag",
            Self::MmsUri => "MMS URI",
            Self::Prl => "PRL",
        }
    }

    fn feature(self) -> BlockFeature {
        match self {
            Self::CdmaAnalogNam | Self::Mdn | Self::CdmaNam => BlockFeature::Nam,
            Self::HomeSystemTag => BlockFeature::SystemTag,
            Self::MmsUri => BlockFeature::MmsUri,
            Self::Prl => BlockFeature::Prl,
        }
    }
}

/// One step in the per-call write plan. `do_download = false` means the block
/// is read-only this session — we still issue a Configuration Request to
/// show the current NAM contents but skip the Download Request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlanStep {
    target: WriteTarget,
    do_download: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingProtocolCapability,
    AwaitingValidation,
    AwaitingPrlDimensions,
    AwaitingPrlExtendedDimensions,
    AwaitingPrlSegment,
    AwaitingNamConfig(WriteTarget),
    AwaitingNamDownload(WriteTarget),
    AwaitingSystemTagConfig,
    AwaitingSystemTagDownload,
    AwaitingMmsConfig,
    AwaitingMmsDownload,
    AwaitingSsprDownloadResponse,
    AwaitingCommit,
    Terminated,
}

/// Cap on the number of SSPR segment requests per session. Defends
/// against a misbehaving MS that never sets `LAST_SEGMENT = 1`.
const PRL_SEGMENT_LIMIT: u32 = 64;

/// Maximum PRL bytes the BS asks for per segment request. `u8` cap is
/// 255 but a comfortable margin under that leaves room for the SSPR
/// Configuration Response framing.
const PRL_SEGMENT_REQUEST_SIZE: u8 = 200;

/// Outbound PRL push segment size (C.S0016-D §4.5.3.1). Matches
/// `PRL_SEGMENT_REQUEST_SIZE`; same headroom argument.
const PRL_DOWNLOAD_SEGMENT_SIZE: u8 = 200;

/// Safety cap on outbound push segments per session.
const PRL_DOWNLOAD_SEGMENT_LIMIT: u32 = 64;

/// State accumulated while fetching a PRL segment-by-segment.
///
/// `cur_sspr_p_rev` is set after the Dimensions exchange. The classic
/// flow leaves it at `1`; the extended retry path sets it to the
/// `CUR_SSPR_P_REV` reported in the Extended PRL Dimensions block,
/// which then selects the decoder (`prl::decode` vs `prl_ext::decode`).
#[derive(Debug, Clone)]
struct PrlFetchState {
    dimensions: Option<cdma_otasp::param::prl_dimensions::PrlDimensions>,
    buffer: Vec<u8>,
    next_offset: u16,
    segments_fetched: u32,
    cur_sspr_p_rev: u8,
}

impl Default for PrlFetchState {
    fn default() -> Self {
        Self {
            dimensions: None,
            buffer: Vec::new(),
            next_offset: 0,
            segments_fetched: 0,
            cur_sspr_p_rev: 1,
        }
    }
}

/// State accumulated while pushing the PRL out segment-by-segment
/// (C.S0016-D §4.5.1.9). `block_id` is chosen at push start based on
/// the resolved PRL's `sspr_p_rev` (0x00 classic / 0x01 extended) and
/// then echoed unchanged on every Download Request for the same PRL.
#[derive(Debug, Clone)]
struct PrlPushState {
    block_id: u8,
    bytes: Vec<u8>,
    next_offset: u16,
    segments_sent: u32,
}

/// OTASP session driver. One per `*228` call.
pub struct OtaspSession {
    cfg: OtaspConfig,
    bts_overhead: BtsOverheadConfig,
    device: HardwareIdentity,
    feature_code: String,
    service_option: u16,
    hlr: Option<ResolvedSubscriberInput>,
    readback: Option<NamReadback>,
    plan: Vec<PlanStep>,
    plan_idx: usize,
    completed_blocks: u32,
    phase: Phase,
    /// Feature IDs the MS advertised in its Protocol Capability Response.
    /// Used to skip writes whose feature isn't supported (e.g. System Tag on
    /// an OTASP_P_REV=1-only device).
    advertised_features: Vec<u8>,
    /// Segmented PRL retrieval state. `Some` only between the first
    /// SSPR Configuration Request and the resulting `PrlReadback` event.
    prl_fetch: Option<PrlFetchState>,
    /// Segmented PRL push state. `Some` only while a Download is in
    /// flight (i.e. waiting on an SSPR Download Response).
    prl_push: Option<PrlPushState>,
    /// Field list for the in-flight NAM Download Request, kept around
    /// so the `BlockDownloaded` event can carry the same operator-
    /// visible values shown on `NamReadback`. `Some` between
    /// `send_nam_download` and the matching `handle_nam_download`.
    pending_download_fields: Option<Vec<(String, String)>>,
}

impl OtaspSession {
    /// Create a new session. Caller must immediately call [`start`].
    pub fn new(
        cfg: OtaspConfig,
        bts_overhead: BtsOverheadConfig,
        device: HardwareIdentity,
        feature_code: String,
        service_option: u16,
        hlr: Option<ResolvedSubscriberInput>,
    ) -> Self {
        // Always include NAM blocks in the plan for read-back (Configuration
        // Request only). `do_download` controls whether we also send a
        // Download Request after. Home System Tag is only included when the
        // operator wants to write it — no read-only diagnostic value yet.
        let mut plan = vec![
            PlanStep {
                target: WriteTarget::CdmaAnalogNam,
                do_download: cfg.writes.cdma_analog_nam,
            },
            PlanStep {
                target: WriteTarget::Mdn,
                do_download: cfg.writes.mdn,
            },
            PlanStep {
                target: WriteTarget::CdmaNam,
                do_download: cfg.writes.cdma_nam,
            },
        ];
        if cfg.writes.home_system_tag {
            plan.push(PlanStep {
                target: WriteTarget::HomeSystemTag,
                do_download: true,
            });
        }
        if cfg.writes.mms_uri && !cfg.mms.uri.is_empty() {
            plan.push(PlanStep {
                target: WriteTarget::MmsUri,
                do_download: true,
            });
        }
        // PRL push: only include when we actually have bytes to send.
        // `coordinator::resolve_prl_for_subscriber` populates
        // `hlr.prl_bytes` when an override or default PRL is configured.
        if cfg.writes.prl
            && hlr
                .as_ref()
                .and_then(|h| h.prl_bytes.as_ref())
                .is_some_and(|b| !b.is_empty())
        {
            plan.push(PlanStep {
                target: WriteTarget::Prl,
                do_download: true,
            });
        }
        Self {
            cfg,
            bts_overhead,
            device,
            feature_code,
            service_option,
            hlr,
            readback: None,
            plan,
            plan_idx: 0,
            completed_blocks: 0,
            phase: Phase::AwaitingProtocolCapability,
            advertised_features: Vec::new(),
            prl_fetch: None,
            prl_push: None,
            pending_download_fields: None,
        }
    }

    /// Kicks off the session: emits `SessionStart` and sends the Protocol
    /// Capability Request.
    pub fn start<T: OtaspTransport>(&mut self, t: &mut T) {
        t.emit(OtaspEvent::SessionStart {
            device: self.device.clone(),
            feature_code: self.feature_code.clone(),
            service_option: self.service_option,
        });
        let req = ProtocolCapabilityRequest::basic();
        match req.encode() {
            Ok(bytes) => t.send(OutboundOtasp { bytes }),
            Err(_) => self.terminate(t, SessionOutcomeKind::ProtocolError),
        }
    }

    /// Feed one inbound OTASP Data Message (extracted from ADDS Transfer
    /// `adds_user_part.data`) into the session. Returns `Some(outcome)` if the
    /// session has reached a terminal state.
    pub fn on_inbound<T: OtaspTransport>(
        &mut self,
        bytes: &[u8],
        t: &mut T,
    ) -> Option<SessionOutcome> {
        if matches!(self.phase, Phase::Terminated) {
            return Some(self.outcome(SessionOutcomeKind::ProtocolError));
        }
        if bytes.is_empty() {
            self.terminate(t, SessionOutcomeKind::ProtocolError);
            return Some(self.outcome(SessionOutcomeKind::ProtocolError));
        }
        let result = match self.phase {
            Phase::AwaitingProtocolCapability => self.handle_protocol_capability(bytes, t),
            Phase::AwaitingValidation => self.handle_validation(bytes, t),
            Phase::AwaitingPrlDimensions => self.handle_prl_dimensions(bytes, t),
            Phase::AwaitingPrlExtendedDimensions => self.handle_prl_extended_dimensions(bytes, t),
            Phase::AwaitingPrlSegment => self.handle_prl_segment(bytes, t),
            Phase::AwaitingNamConfig(target) => self.handle_nam_config(target, bytes, t),
            Phase::AwaitingNamDownload(target) => self.handle_nam_download(target, bytes, t),
            Phase::AwaitingSystemTagConfig => self.handle_system_tag_config(bytes, t),
            Phase::AwaitingSystemTagDownload => self.handle_system_tag_download(bytes, t),
            Phase::AwaitingMmsConfig => self.handle_mms_config(bytes, t),
            Phase::AwaitingMmsDownload => self.handle_mms_download(bytes, t),
            Phase::AwaitingSsprDownloadResponse => self.handle_sspr_download_response(bytes, t),
            Phase::AwaitingCommit => self.handle_commit(bytes, t),
            Phase::Terminated => unreachable!(),
        };
        match result {
            StepResult::Continue => None,
            StepResult::Terminal(kind) => Some(self.outcome(kind)),
        }
    }

    fn outcome(&mut self, kind: SessionOutcomeKind) -> SessionOutcome {
        self.phase = Phase::Terminated;
        SessionOutcome {
            kind,
            completed_blocks: self.completed_blocks,
        }
    }

    /// Called by the coordinator when the BSC reports an `AddsDeliverAck`
    /// with a failure `cause` for an outbound OTASP DBM. Lets the session
    /// advance immediately rather than wait on the 5 s inbound-silence
    /// timeout.
    ///
    /// For phases that have a clearly identifiable block target (NAM
    /// Config/Download, System Tag, MMS URI, PRL push) we emit a
    /// `BlockSkipped` for that block and advance to the next plan
    /// step. For phases without a block target (Protocol Capability,
    /// Verify SPC, PRL read-back, Commit) we terminate the session
    /// with `ProtocolError` since there's no clean way to recover.
    pub fn on_dbm_failed<T: OtaspTransport>(&mut self, cause: u8, t: &mut T) {
        if matches!(self.phase, Phase::Terminated) {
            return;
        }
        let phase = self.phase;
        match phase {
            Phase::AwaitingNamConfig(target) | Phase::AwaitingNamDownload(target) => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: target.block_id(),
                    reason: format!(
                        "{} skipped — BSC reported DBM delivery failure (A1 cause 0x{:02x})",
                        target.label(),
                        cause
                    ),
                    feature: target.feature(),
                });
                self.plan_idx += 1;
                let _ = self.advance_to_next_target(t);
            }
            Phase::AwaitingSystemTagConfig | Phase::AwaitingSystemTagDownload => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: BLOCK_HOME_SYSTEM_TAG,
                    reason: format!(
                        "Home System Tag skipped — BSC reported DBM delivery failure (A1 cause 0x{:02x})",
                        cause
                    ),
                    feature: BlockFeature::SystemTag,
                });
                self.plan_idx += 1;
                let _ = self.advance_to_next_target(t);
            }
            Phase::AwaitingMmsConfig | Phase::AwaitingMmsDownload => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: BLOCK_MMS_URI,
                    reason: format!(
                        "MMS URI skipped — BSC reported DBM delivery failure (A1 cause 0x{:02x})",
                        cause
                    ),
                    feature: BlockFeature::MmsUri,
                });
                self.plan_idx += 1;
                let _ = self.advance_to_next_target(t);
            }
            Phase::AwaitingSsprDownloadResponse => {
                let block_id = self
                    .prl_push
                    .as_ref()
                    .map(|s| s.block_id)
                    .unwrap_or(cdma_otasp::message::sspr::BLOCK_PRL_CLASSIC);
                t.emit(OtaspEvent::BlockSkipped {
                    block_id,
                    reason: format!(
                        "PRL push skipped — BSC reported DBM delivery failure (A1 cause 0x{:02x})",
                        cause
                    ),
                    feature: BlockFeature::Prl,
                });
                self.prl_push = None;
                self.plan_idx += 1;
                let _ = self.advance_to_next_target(t);
            }
            // Read-back / control phases without a block target — no
            // clean recovery, terminate the session.
            Phase::AwaitingProtocolCapability
            | Phase::AwaitingValidation
            | Phase::AwaitingPrlDimensions
            | Phase::AwaitingPrlExtendedDimensions
            | Phase::AwaitingPrlSegment
            | Phase::AwaitingCommit => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
            }
            Phase::Terminated => {}
        }
    }

    /// Drop plan entries whose feature isn't in the MS's advertised list,
    /// emitting a `BlockSkipped` event for each one so the operator sees why.
    /// Per C.S0016-D Table 3.5.1.7-1:
    ///   FEATURE_ID 0x00 = NAM Download   (DATA_P_REV)
    ///   FEATURE_ID 0x09 = System Tag Download (TAG_P_REV)
    fn prune_plan_for_unsupported_features<T: OtaspTransport>(&mut self, t: &mut T) {
        use cdma_otasp::message::protocol_capability::feature_id;
        let advertised = &self.advertised_features;
        let mut kept = Vec::with_capacity(self.plan.len());
        for step in self.plan.drain(..) {
            let needed_feature: u8 = match step.target {
                WriteTarget::CdmaAnalogNam | WriteTarget::Mdn | WriteTarget::CdmaNam => {
                    feature_id::NAM_DOWNLOAD
                }
                WriteTarget::HomeSystemTag => feature_id::SYSTEM_TAG_DOWNLOAD,
                WriteTarget::MmsUri => feature_id::MMS,
                WriteTarget::Prl => feature_id::SSPR,
            };
            if !advertised.contains(&needed_feature) {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: step.target.block_id(),
                    reason: format!(
                        "{} skipped — MS did not advertise FEATURE_ID=0x{:02x} in Protocol Capability Response",
                        step.target.label(),
                        needed_feature
                    ),
                    feature: step.target.feature(),
                });
            } else {
                kept.push(step);
            }
        }
        self.plan = kept;
    }

    fn terminate<T: OtaspTransport>(&mut self, t: &mut T, kind: SessionOutcomeKind) {
        t.emit(OtaspEvent::SessionEnded {
            completed_blocks: self.completed_blocks,
            outcome: kind,
        });
        self.phase = Phase::Terminated;
    }

    fn handle_protocol_capability<T: OtaspTransport>(
        &mut self,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        let resp = match ProtocolCapabilityResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let otasp_p_rev = resp
            .features
            .iter()
            .find(|f| f.feature_id == cdma_otasp::message::protocol_capability::feature_id::OTASP)
            .map(|f| f.feature_p_rev);
        self.advertised_features = resp.features.iter().map(|f| f.feature_id).collect();
        let features_pairs: Vec<(u8, u8)> = resp
            .features
            .iter()
            .map(|f| (f.feature_id, f.feature_p_rev))
            .collect();
        t.emit(OtaspEvent::ProtocolCapabilityReceived {
            mob_firm_rev: resp.mob_firm_rev,
            mob_model: resp.mob_model,
            band_mode_cap: crate::otasp::event::BandModeCapView::from_byte(resp.band_mode_cap.raw),
            otasp_p_rev,
            features: features_pairs,
        });
        self.prune_plan_for_unsupported_features(t);
        // Verify SPC: subscriber's stored SPC if set, else IS-95 default.
        let spc = self
            .hlr
            .as_ref()
            .and_then(|h| h.service_programming_code.clone())
            .unwrap_or_else(|| "000000".to_string());
        let verify = VerifySpc::new(spc);
        let param_data = match verify.encode() {
            Ok(b) => b,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let req = ValidationRequest {
            blocks: vec![ValidationParamBlock {
                block_id: VBLOCK_VERIFY_SPC,
                param_data,
            }],
        };
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingValidation;
        StepResult::Continue
    }

    fn handle_validation<T: OtaspTransport>(&mut self, bytes: &[u8], t: &mut T) -> StepResult {
        let resp = match ValidationResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let accepted = resp
            .results
            .iter()
            .find(|(bid, _)| *bid == VBLOCK_VERIFY_SPC)
            .map(|(_, r)| r.is_accepted())
            .unwrap_or(false);
        if !accepted {
            t.emit(OtaspEvent::SpcMismatch);
            self.terminate(t, SessionOutcomeKind::SpcRejected);
            return StepResult::Terminal(SessionOutcomeKind::SpcRejected);
        }
        t.emit(OtaspEvent::SpcVerified);

        // HLR lookup is performed by the caller before constructing the session,
        // but we honor a `None` value at this point as the equivalent of an
        // HLR miss because nothing further in the plan needs the subscriber.
        if self.hlr.is_none() {
            t.emit(OtaspEvent::HlrMiss {
                device: self.device.clone(),
            });
            self.terminate(t, SessionOutcomeKind::HlrUnknown);
            return StepResult::Terminal(SessionOutcomeKind::HlrUnknown);
        }

        self.start_prl_phase_or_advance(t)
    }

    /// Kick off the SSPR PRL read if the MS advertised the SSPR feature.
    /// Otherwise emit a `FeatureNotAdvertised` PRL outcome and move on.
    fn start_prl_phase_or_advance<T: OtaspTransport>(&mut self, t: &mut T) -> StepResult {
        use cdma_otasp::message::protocol_capability::feature_id;
        if !self.advertised_features.contains(&feature_id::SSPR) {
            t.emit(OtaspEvent::PrlReadback(PrlReadback {
                pr_list_id: None,
                max_pr_list_size: 0,
                cur_pr_list_size: 0,
                segment_count: 0,
                outcome: PrlOutcome::FeatureNotAdvertised,
            }));
            return self.advance_to_next_target(t);
        }
        self.prl_fetch = Some(PrlFetchState::default());
        let req = SsprConfigurationRequest::dimensions();
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingPrlDimensions;
        StepResult::Continue
    }

    fn handle_prl_dimensions<T: OtaspTransport>(&mut self, bytes: &[u8], t: &mut T) -> StepResult {
        let resp = match SsprConfigurationResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                return self.finish_prl_decode_failure(
                    t,
                    "SSPR Configuration Response decode failed".into(),
                    bytes.to_vec(),
                );
            }
        };
        // Per C.S0016-D §3.5.1.8: result code `0x23` ("Rejected – PRL
        // format mismatch") means the MS stores SSPR_P_REV >= 3 and
        // the BS needs to switch to the Extended PRL Dimensions
        // request (BLOCK_ID = 0x02, §3.5.3.3).
        if resp.result_code.to_u8() == 0x23 {
            return self.send_extended_dimensions_request(t);
        }
        if !resp.result_code.is_accepted() {
            return self.finish_prl_outcome(
                t,
                PrlOutcome::Rejected {
                    block_id: resp.block_id,
                    result_code: resp.result_code.to_u8(),
                },
                None,
            );
        }
        let dims = match cdma_otasp::param::prl_dimensions::PrlDimensions::decode(&resp.param_data)
        {
            Ok(d) => d,
            Err(_) => {
                return self.finish_prl_decode_failure(
                    t,
                    "PRL Dimensions decode failed".into(),
                    resp.param_data,
                );
            }
        };
        if dims.cur_pr_list_size == 0 {
            return self.finish_prl_outcome(t, PrlOutcome::Absent, Some(dims));
        }
        let state = self
            .prl_fetch
            .as_mut()
            .expect("PRL fetch state initialized");
        state.dimensions = Some(dims);
        state.next_offset = 0;
        self.send_next_prl_segment_request(t)
    }

    fn send_extended_dimensions_request<T: OtaspTransport>(&mut self, t: &mut T) -> StepResult {
        let req = SsprConfigurationRequest::extended_dimensions();
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingPrlExtendedDimensions;
        StepResult::Continue
    }

    fn handle_prl_extended_dimensions<T: OtaspTransport>(
        &mut self,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        let resp = match SsprConfigurationResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                return self.finish_prl_decode_failure(
                    t,
                    "Extended SSPR Configuration Response decode failed".into(),
                    bytes.to_vec(),
                );
            }
        };
        if !resp.result_code.is_accepted() {
            return self.finish_prl_outcome(
                t,
                PrlOutcome::Rejected {
                    block_id: resp.block_id,
                    result_code: resp.result_code.to_u8(),
                },
                None,
            );
        }
        let ext = match cdma_otasp::param::prl_dimensions::ExtendedPrlDimensions::decode(
            &resp.param_data,
        ) {
            Ok(d) => d,
            Err(_) => {
                return self.finish_prl_decode_failure(
                    t,
                    "Extended PRL Dimensions decode failed".into(),
                    resp.param_data,
                );
            }
        };
        // Project the extended dimensions onto the existing
        // `PrlDimensions` shape used by the rest of the segment-fetch
        // bookkeeping. The detailed counts (NUM_COMMON_SUBNET_RECS,
        // NUM_EXT_SYS_RECS) appear in the decoded PRL itself.
        use cdma_otasp::param::prl_dimensions::ExtendedDimsCounts;
        let (num_acq_recs, num_sys_recs) = match ext.counts {
            ExtendedDimsCounts::Classic {
                num_acq_recs,
                num_sys_recs,
            } => (num_acq_recs, num_sys_recs),
            ExtendedDimsCounts::Extended {
                num_acq_recs,
                num_ext_sys_recs,
                ..
            } => (num_acq_recs, num_ext_sys_recs),
        };
        let dims = cdma_otasp::param::prl_dimensions::PrlDimensions {
            max_pr_list_size: ext.max_pr_list_size,
            cur_pr_list_size: ext.cur_pr_list_size,
            pr_list_id: ext.pr_list_id,
            num_acq_recs,
            num_sys_recs,
        };
        if dims.cur_pr_list_size == 0 {
            return self.finish_prl_outcome(t, PrlOutcome::Absent, Some(dims));
        }
        let state = self
            .prl_fetch
            .as_mut()
            .expect("PRL fetch state initialized");
        state.dimensions = Some(dims);
        state.cur_sspr_p_rev = ext.cur_sspr_p_rev;
        state.next_offset = 0;
        self.send_next_prl_segment_request(t)
    }

    fn send_next_prl_segment_request<T: OtaspTransport>(&mut self, t: &mut T) -> StepResult {
        {
            let state = self.prl_fetch.as_ref().expect("PRL fetch state present");
            if state.segments_fetched >= PRL_SEGMENT_LIMIT {
                let buffer = state.buffer.clone();
                return self.finish_prl_decode_failure(
                    t,
                    format!(
                        "exceeded {} SSPR segment requests without LAST_SEGMENT",
                        PRL_SEGMENT_LIMIT
                    ),
                    buffer,
                );
            }
        }
        let offset = self.prl_fetch.as_ref().unwrap().next_offset;
        let req = SsprConfigurationRequest::segment(offset, PRL_SEGMENT_REQUEST_SIZE);
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingPrlSegment;
        StepResult::Continue
    }

    fn handle_prl_segment<T: OtaspTransport>(&mut self, bytes: &[u8], t: &mut T) -> StepResult {
        let resp = match SsprConfigurationResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                return self.finish_prl_decode_failure(
                    t,
                    "SSPR segment response decode failed".into(),
                    bytes.to_vec(),
                );
            }
        };
        if !resp.result_code.is_accepted() {
            return self.finish_prl_outcome(
                t,
                PrlOutcome::Rejected {
                    block_id: resp.block_id,
                    result_code: resp.result_code.to_u8(),
                },
                self.prl_fetch.as_ref().and_then(|s| s.dimensions),
            );
        }
        let segment = match cdma_otasp::param::prl_segment::PrlSegment::decode(&resp.param_data) {
            Ok(s) => s,
            Err(_) => {
                return self.finish_prl_decode_failure(
                    t,
                    "PRL segment decode failed".into(),
                    resp.param_data,
                );
            }
        };
        let overflow = {
            let state = self.prl_fetch.as_mut().expect("PRL fetch state present");
            state.segments_fetched += 1;
            let overflow = match state.dimensions {
                Some(dims)
                    if state.buffer.len() + segment.segment_data.len()
                        > dims.cur_pr_list_size as usize =>
                {
                    Some(state.buffer.clone())
                }
                _ => None,
            };
            if overflow.is_none() {
                state.buffer.extend_from_slice(&segment.segment_data);
                state.next_offset = state
                    .next_offset
                    .saturating_add(segment.segment_data.len() as u16);
            }
            overflow
        };
        if let Some(buf) = overflow {
            return self.finish_prl_decode_failure(
                t,
                "SSPR segments exceeded CUR_PR_LIST_SIZE".into(),
                buf,
            );
        }
        if segment.last_segment {
            let state = self.prl_fetch.as_ref().expect("PRL fetch state present");
            let buffer = state.buffer.clone();
            let dims = state.dimensions;
            let cur_sspr_p_rev = state.cur_sspr_p_rev;
            let outcome = if cur_sspr_p_rev >= 3 {
                match cdma_otasp::param::prl_ext::decode(&buffer) {
                    Ok(prl) => {
                        if !prl.crc_ok() {
                            PrlOutcome::DecodeFailed {
                                reason: format!(
                                    "Extended PRL CRC mismatch: expected 0x{:04x} computed 0x{:04x}",
                                    prl.pr_list_crc, prl.computed_crc
                                ),
                                raw_bytes: buffer,
                            }
                        } else {
                            PrlOutcome::DecodedExtended {
                                prl,
                                raw_bytes: buffer,
                            }
                        }
                    }
                    Err(e) => PrlOutcome::DecodeFailed {
                        reason: e.to_string(),
                        raw_bytes: buffer,
                    },
                }
            } else {
                match cdma_otasp::param::prl::decode(&buffer) {
                    Ok(prl) => {
                        if !prl.crc_ok() {
                            PrlOutcome::DecodeFailed {
                                reason: format!(
                                    "PRL CRC mismatch: expected 0x{:04x} computed 0x{:04x}",
                                    prl.pr_list_crc, prl.computed_crc
                                ),
                                raw_bytes: buffer,
                            }
                        } else {
                            PrlOutcome::Decoded(prl)
                        }
                    }
                    Err(e) => PrlOutcome::DecodeFailed {
                        reason: e.to_string(),
                        raw_bytes: buffer,
                    },
                }
            };
            return self.finish_prl_outcome(t, outcome, dims);
        }
        self.send_next_prl_segment_request(t)
    }

    fn finish_prl_outcome<T: OtaspTransport>(
        &mut self,
        t: &mut T,
        outcome: PrlOutcome,
        dims: Option<cdma_otasp::param::prl_dimensions::PrlDimensions>,
    ) -> StepResult {
        let segment_count = self
            .prl_fetch
            .as_ref()
            .map(|s| s.segments_fetched)
            .unwrap_or(0);
        let pr_list_id = match &outcome {
            PrlOutcome::Decoded(p) => Some(p.pr_list_id),
            PrlOutcome::DecodedExtended { prl, .. } => Some(prl.pr_list_id),
            _ => dims.map(|d| d.pr_list_id),
        };
        t.emit(OtaspEvent::PrlReadback(PrlReadback {
            pr_list_id,
            max_pr_list_size: dims.map(|d| d.max_pr_list_size).unwrap_or(0),
            cur_pr_list_size: dims.map(|d| d.cur_pr_list_size).unwrap_or(0),
            segment_count,
            outcome,
        }));
        self.prl_fetch = None;
        self.advance_to_next_target(t)
    }

    fn finish_prl_decode_failure<T: OtaspTransport>(
        &mut self,
        t: &mut T,
        reason: String,
        raw_bytes: Vec<u8>,
    ) -> StepResult {
        let dims = self.prl_fetch.as_ref().and_then(|s| s.dimensions);
        self.finish_prl_outcome(t, PrlOutcome::DecodeFailed { reason, raw_bytes }, dims)
    }

    fn advance_to_next_target<T: OtaspTransport>(&mut self, t: &mut T) -> StepResult {
        if self.plan_idx >= self.plan.len() {
            // Always send Commit, even with zero blocks downloaded. Per
            // observed behavior, vintage MSes display "activation
            // unsuccessful" if the session ends without an explicit Commit.
            // A zero-blocks Commit is a no-op on the MS but signals
            // "session is over cleanly."
            let req = CommitRequest;
            t.send(OutboundOtasp {
                bytes: req.encode(),
            });
            self.phase = Phase::AwaitingCommit;
            return StepResult::Continue;
        }
        let target = self.plan[self.plan_idx].target;
        match target {
            WriteTarget::HomeSystemTag => {
                let req = SystemTagConfigRequest {
                    block_id: BLOCK_HOME_SYSTEM_TAG,
                    segment: None,
                };
                match req.encode() {
                    Ok(b) => t.send(OutboundOtasp { bytes: b }),
                    Err(_) => {
                        self.terminate(t, SessionOutcomeKind::ProtocolError);
                        return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
                    }
                }
                self.phase = Phase::AwaitingSystemTagConfig;
            }
            WriteTarget::MmsUri => {
                let req = cdma_otasp::message::mms::MmsConfigurationRequest {
                    block_ids: vec![BLOCK_MMS_URI],
                };
                match req.encode() {
                    Ok(b) => t.send(OutboundOtasp { bytes: b }),
                    Err(_) => {
                        self.terminate(t, SessionOutcomeKind::ProtocolError);
                        return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
                    }
                }
                self.phase = Phase::AwaitingMmsConfig;
            }
            WriteTarget::Prl => {
                return self.start_prl_push(t);
            }
            _ => {
                let req = ConfigurationRequest {
                    block_ids: vec![target.block_id()],
                };
                match req.encode() {
                    Ok(b) => t.send(OutboundOtasp { bytes: b }),
                    Err(_) => {
                        self.terminate(t, SessionOutcomeKind::ProtocolError);
                        return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
                    }
                }
                self.phase = Phase::AwaitingNamConfig(target);
            }
        }
        StepResult::Continue
    }

    fn handle_nam_config<T: OtaspTransport>(
        &mut self,
        target: WriteTarget,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        let resp = match ConfigurationResponse::decode(bytes) {
            Ok(r) => r,
            Err(e) => {
                log::warn!(
                    "OTASP: failed to decode Configuration Response for {}: {} (raw {} bytes: {})",
                    target.label(),
                    e,
                    bytes.len(),
                    hex_dump(bytes)
                );
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: target.block_id(),
                    reason: format!(
                        "{} read-back skipped — Configuration Response decode failed: {}",
                        target.label(),
                        e
                    ),
                    feature: target.feature(),
                });
                self.plan_idx += 1;
                return self.advance_to_next_target(t);
            }
        };
        let block = match resp.blocks.iter().find(|b| b.block_id == target.block_id()) {
            Some(b) => b,
            None => {
                let returned: Vec<String> = resp
                    .blocks
                    .iter()
                    .map(|b| format!("0x{:02x} (len={})", b.block_id, b.param_data.len()))
                    .collect();
                log::warn!(
                    "OTASP: Configuration Response for {} missing target block 0x{:02x}; returned blocks: [{}]",
                    target.label(),
                    target.block_id(),
                    returned.join(", ")
                );
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: target.block_id(),
                    reason: format!(
                        "{} read-back skipped — MS did not return BLOCK_ID 0x{:02x} (returned: [{}])",
                        target.label(),
                        target.block_id(),
                        returned.join(", ")
                    ),
                    feature: target.feature(),
                });
                self.plan_idx += 1;
                return self.advance_to_next_target(t);
            }
        };
        // Pull RO/RT echo from whichever block the MS returned. Both A.1 and
        // A.3 expose the fields we need.
        let readback = match target {
            WriteTarget::CdmaAnalogNam => match NamCdmaAnalog::decode(&block.param_data) {
                Ok(d) => NamReadback {
                    scm: d.scm,
                    mob_p_rev: d.mob_p_rev,
                    max_sid_nid: d.max_sid_nid,
                    slotted_mode: self
                        .readback
                        .as_ref()
                        .map(|r| r.slotted_mode)
                        .unwrap_or(false),
                    ex: d.ex,
                    local_control: d.local_control,
                    firstchp: d.firstchp,
                },
                Err(e) => {
                    log::warn!(
                        "OTASP: failed to decode {} block ({} bytes: {}): {}",
                        target.label(),
                        block.param_data.len(),
                        hex_dump(&block.param_data),
                        e
                    );
                    t.emit(OtaspEvent::BlockSkipped {
                        block_id: target.block_id(),
                        reason: format!(
                            "{} read-back skipped — block decode failed: {} (raw {} bytes: {})",
                            target.label(),
                            e,
                            block.param_data.len(),
                            hex_dump(&block.param_data)
                        ),
                        feature: target.feature(),
                    });
                    self.plan_idx += 1;
                    return self.advance_to_next_target(t);
                }
            },
            WriteTarget::CdmaNam => match NamCdma::decode(&block.param_data) {
                Ok(d) => NamReadback {
                    scm: self.readback.as_ref().map(|r| r.scm).unwrap_or(0),
                    mob_p_rev: d.mob_p_rev,
                    max_sid_nid: d.max_sid_nid,
                    slotted_mode: d.slotted_mode,
                    ex: self.readback.as_ref().map(|r| r.ex).unwrap_or(false),
                    local_control: d.local_control,
                    // FIRSTCHP lives only in the CDMA/Analog block; keep the
                    // value read there earlier in the session.
                    firstchp: self.readback.as_ref().map(|r| r.firstchp).unwrap_or(0),
                },
                Err(e) => {
                    log::warn!(
                        "OTASP: failed to decode {} block ({} bytes: {}): {}",
                        target.label(),
                        block.param_data.len(),
                        hex_dump(&block.param_data),
                        e
                    );
                    t.emit(OtaspEvent::BlockSkipped {
                        block_id: target.block_id(),
                        reason: format!(
                            "{} read-back skipped — block decode failed: {} (raw {} bytes: {})",
                            target.label(),
                            e,
                            block.param_data.len(),
                            hex_dump(&block.param_data)
                        ),
                        feature: target.feature(),
                    });
                    self.plan_idx += 1;
                    return self.advance_to_next_target(t);
                }
            },
            WriteTarget::Mdn
            | WriteTarget::HomeSystemTag
            | WriteTarget::MmsUri
            | WriteTarget::Prl => {
                // These blocks carry no RO/RT fields we care about; keep
                // prior readback (defaults if none) so NAM assembly still
                // works. (HomeSystemTag, MmsUri, and Prl are dispatched
                // through their own handlers and never land here in
                // practice.)
                self.readback.clone().unwrap_or_default()
            }
        };
        if matches!(target, WriteTarget::CdmaAnalogNam | WriteTarget::CdmaNam)
            && readback.max_sid_nid == 0
        {
            t.emit(OtaspEvent::NoNamCapacity {
                block_id: target.block_id(),
                feature: target.feature(),
            });
            self.terminate(t, SessionOutcomeKind::NoCapacity);
            return StepResult::Terminal(SessionOutcomeKind::NoCapacity);
        }
        // Merge into running readback so a later block benefits from RO fields
        // learned in an earlier block.
        self.readback = Some(merge_readback(self.readback.clone(), readback));

        // Decode the Station Class Mark once we see it. CDMA/Analog NAM is
        // the only block that carries SCM; CDMA NAM omits it because all RO
        // identity fields move there from §3.5.2.1 to §3.5.2.3.
        if matches!(target, WriteTarget::CdmaAnalogNam)
            && let Some(scm_byte) = self.readback.as_ref().map(|r| r.scm)
        {
            t.emit(OtaspEvent::StationClassMark(
                crate::otasp::event::StationClassMark::decode(scm_byte),
            ));
        }

        // Surface decoded NAM contents to the operator regardless of whether
        // we're going to write this block.
        let fields = decode_nam_block_to_fields(target, &block.param_data);
        t.emit(OtaspEvent::NamReadback {
            block_id: target.block_id(),
            label: target.label().to_string(),
            fields,
            feature: target.feature(),
        });

        // Read-only step? Skip the Download Request and move to the next plan
        // entry. Lookup is by plan_idx since handle_nam_config was dispatched
        // from the current step.
        let do_download = self
            .plan
            .get(self.plan_idx)
            .map(|s| s.do_download)
            .unwrap_or(false);
        if !do_download {
            self.plan_idx += 1;
            return self.advance_to_next_target(t);
        }

        self.send_nam_download(target, t)
    }

    fn send_nam_download<T: OtaspTransport>(
        &mut self,
        target: WriteTarget,
        t: &mut T,
    ) -> StepResult {
        let hlr = match self.hlr.as_ref() {
            Some(h) => h.clone(),
            None => {
                self.terminate(t, SessionOutcomeKind::HlrUnknown);
                return StepResult::Terminal(SessionOutcomeKind::HlrUnknown);
            }
        };
        let readback = self.readback.clone().unwrap_or_default();
        let nam = match assemble_nam(&hlr, &self.bts_overhead, &self.cfg, &readback) {
            Ok(n) => n,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let param_data = match target {
            WriteTarget::CdmaAnalogNam => nam.cdma_analog.encode(),
            WriteTarget::Mdn => nam.mdn.encode(),
            WriteTarget::CdmaNam => nam.cdma.encode(),
            WriteTarget::HomeSystemTag => unreachable!("Home System Tag uses its own message"),
            WriteTarget::MmsUri => unreachable!("MMS URI uses its own message"),
            WriteTarget::Prl => unreachable!("PRL uses its own SSPR Download message"),
        };
        self.pending_download_fields = Some(nam_download_fields(target, &nam));
        let param_data = match param_data {
            Ok(b) => b,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let req = DownloadRequest {
            blocks: vec![DownloadParamBlock {
                block_id: target.block_id(),
                param_data,
            }],
        };
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingNamDownload(target);
        StepResult::Continue
    }

    fn handle_nam_download<T: OtaspTransport>(
        &mut self,
        target: WriteTarget,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        let resp = match DownloadResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let (bid, code) = match resp.results.first() {
            Some(r) => *r,
            None => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let _ = bid;
        let fields = self.pending_download_fields.take().unwrap_or_default();
        if code.is_accepted() {
            t.emit(OtaspEvent::BlockDownloaded {
                block_id: target.block_id(),
                result_code: code.to_u8(),
                feature: target.feature(),
                fields,
            });
            self.completed_blocks += 1;
            self.plan_idx += 1;
            self.advance_to_next_target(t)
        } else {
            t.emit(OtaspEvent::BlockRejected {
                block_id: target.block_id(),
                result_code: code.to_u8(),
                feature: target.feature(),
            });
            self.terminate(t, SessionOutcomeKind::Rejected);
            StepResult::Terminal(SessionOutcomeKind::Rejected)
        }
    }

    fn handle_system_tag_config<T: OtaspTransport>(
        &mut self,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        let resp = match SystemTagConfigResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        if !resp.result.is_accepted() {
            t.emit(OtaspEvent::BlockRejected {
                block_id: BLOCK_HOME_SYSTEM_TAG,
                result_code: resp.result.to_u8(),
                feature: BlockFeature::SystemTag,
            });
            self.terminate(t, SessionOutcomeKind::Rejected);
            return StepResult::Terminal(SessionOutcomeKind::Rejected);
        }
        // HLR is required by NAM assembly because system_tag is built there.
        let hlr = match self.hlr.as_ref() {
            Some(h) => h.clone(),
            None => {
                self.terminate(t, SessionOutcomeKind::HlrUnknown);
                return StepResult::Terminal(SessionOutcomeKind::HlrUnknown);
            }
        };
        let readback = self.readback.clone().unwrap_or_default();
        let nam = match assemble_nam(&hlr, &self.bts_overhead, &self.cfg, &readback) {
            Ok(n) => n,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let param_data = match nam.home_system_tag.encode() {
            Ok(b) => b,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let req = SystemTagDownloadRequest {
            block_id: BLOCK_HOME_SYSTEM_TAG,
            param_data,
        };
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingSystemTagDownload;
        StepResult::Continue
    }

    fn handle_system_tag_download<T: OtaspTransport>(
        &mut self,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        let resp = match SystemTagDownloadResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        if resp.result.is_accepted() {
            t.emit(OtaspEvent::BlockDownloaded {
                block_id: BLOCK_HOME_SYSTEM_TAG,
                result_code: resp.result.to_u8(),
                feature: BlockFeature::SystemTag,
                fields: vec![("Tag".into(), self.cfg.system_tag.name.clone())],
            });
            self.completed_blocks += 1;
            self.plan_idx += 1;
            self.advance_to_next_target(t)
        } else {
            t.emit(OtaspEvent::BlockRejected {
                block_id: BLOCK_HOME_SYSTEM_TAG,
                result_code: resp.result.to_u8(),
                feature: BlockFeature::SystemTag,
            });
            self.terminate(t, SessionOutcomeKind::Rejected);
            StepResult::Terminal(SessionOutcomeKind::Rejected)
        }
    }

    fn handle_mms_config<T: OtaspTransport>(&mut self, bytes: &[u8], t: &mut T) -> StepResult {
        use cdma_otasp::message::mms::{
            MmsConfigurationResponse, MmsDownloadRequest, MmsParamBlock, MmsUriEntry,
            MmsUriParameters,
        };
        let resp = match MmsConfigurationResponse::decode(bytes) {
            Ok(r) => r,
            Err(e) => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: BLOCK_MMS_URI,
                    reason: format!(
                        "MMS URI read-back skipped — MmsConfigurationResponse decode failed: {e}"
                    ),
                    feature: BlockFeature::MmsUri,
                });
                self.plan_idx += 1;
                return self.advance_to_next_target(t);
            }
        };
        // Capture whatever the MS returned for visibility, then decide whether
        // to follow up with a Download.
        if let Some(block) = resp.blocks.iter().find(|b| b.block_id == BLOCK_MMS_URI) {
            let label = match MmsUriParameters::decode(&block.param_data) {
                Ok(parsed) => {
                    let entries = parsed
                        .entries
                        .iter()
                        .map(|e| format!("[{}]={}", e.entry_idx, e.uri))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if entries.is_empty() {
                        "(empty)".to_string()
                    } else {
                        entries
                    }
                }
                Err(e) => format!("(decode failed: {e})"),
            };
            t.emit(OtaspEvent::NamReadback {
                block_id: BLOCK_MMS_URI,
                label: "MMS URI".into(),
                fields: vec![("Current".into(), label)],
                feature: BlockFeature::MmsUri,
            });
        }
        let want_download = self
            .plan
            .get(self.plan_idx)
            .map(|s| s.do_download)
            .unwrap_or(false);
        if !want_download {
            self.plan_idx += 1;
            return self.advance_to_next_target(t);
        }
        let params = MmsUriParameters {
            entries: vec![MmsUriEntry {
                entry_idx: 0,
                uri: self.cfg.mms.uri.clone(),
            }],
        };
        let param_data = match params.encode() {
            Ok(b) => b,
            Err(e) => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: BLOCK_MMS_URI,
                    reason: format!("MMS URI write skipped — encoder failure: {e}"),
                    feature: BlockFeature::MmsUri,
                });
                self.plan_idx += 1;
                return self.advance_to_next_target(t);
            }
        };
        let dl = MmsDownloadRequest {
            blocks: vec![MmsParamBlock {
                block_id: BLOCK_MMS_URI,
                param_data,
            }],
        };
        match dl.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        self.phase = Phase::AwaitingMmsDownload;
        StepResult::Continue
    }

    fn handle_mms_download<T: OtaspTransport>(&mut self, bytes: &[u8], t: &mut T) -> StepResult {
        use cdma_otasp::message::mms::MmsDownloadResponse;
        let resp = match MmsDownloadResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let confirmation = match resp
            .confirmations
            .iter()
            .find(|c| c.block_id == BLOCK_MMS_URI)
        {
            Some(c) => c,
            None => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: BLOCK_MMS_URI,
                    reason: "MMS URI write skipped — MS Download Response missing target block"
                        .into(),
                    feature: BlockFeature::MmsUri,
                });
                self.plan_idx += 1;
                return self.advance_to_next_target(t);
            }
        };
        if confirmation.result.is_accepted() {
            t.emit(OtaspEvent::BlockDownloaded {
                block_id: BLOCK_MMS_URI,
                result_code: confirmation.result.to_u8(),
                feature: BlockFeature::MmsUri,
                fields: vec![("URI".into(), self.cfg.mms.uri.clone())],
            });
            self.completed_blocks += 1;
            self.plan_idx += 1;
            self.advance_to_next_target(t)
        } else {
            t.emit(OtaspEvent::BlockRejected {
                block_id: BLOCK_MMS_URI,
                result_code: confirmation.result.to_u8(),
                feature: BlockFeature::MmsUri,
            });
            // MMS rejection is non-fatal — phone keeps its existing
            // MMSC URL and the session moves on to Commit.
            self.plan_idx += 1;
            self.advance_to_next_target(t)
        }
    }

    /// Initialize the PRL push state from `hlr.prl_bytes` + `hlr.prl_meta`,
    /// emit the first SSPR Download Request, and set the phase.
    fn start_prl_push<T: OtaspTransport>(&mut self, t: &mut T) -> StepResult {
        use cdma_otasp::message::sspr::{BLOCK_PRL_CLASSIC, BLOCK_PRL_EXTENDED};
        let (bytes, sspr_p_rev) = match self.hlr.as_ref() {
            Some(h) => match (h.prl_bytes.as_ref(), h.prl_meta.as_ref()) {
                (Some(b), Some(m)) if !b.is_empty() => (b.clone(), m.sspr_p_rev),
                _ => {
                    // Shouldn't happen — plan inclusion already gated on
                    // these — but skip cleanly rather than panic.
                    t.emit(OtaspEvent::BlockSkipped {
                        block_id: BLOCK_PRL_CLASSIC,
                        reason: "PRL push skipped — no resolved PRL bytes available".into(),
                        feature: BlockFeature::Prl,
                    });
                    self.plan_idx += 1;
                    return self.advance_to_next_target(t);
                }
            },
            None => {
                t.emit(OtaspEvent::BlockSkipped {
                    block_id: BLOCK_PRL_CLASSIC,
                    reason: "PRL push skipped — no HLR-resolved subscriber".into(),
                    feature: BlockFeature::Prl,
                });
                self.plan_idx += 1;
                return self.advance_to_next_target(t);
            }
        };
        let block_id = if sspr_p_rev >= 3 {
            BLOCK_PRL_EXTENDED
        } else {
            BLOCK_PRL_CLASSIC
        };
        self.prl_push = Some(PrlPushState {
            block_id,
            bytes,
            next_offset: 0,
            segments_sent: 0,
        });
        self.send_next_prl_push_segment(t)
    }

    fn send_next_prl_push_segment<T: OtaspTransport>(&mut self, t: &mut T) -> StepResult {
        use cdma_otasp::message::sspr::{SsprDownloadRequest, encode_sspr_param_data};
        let state = match self.prl_push.as_mut() {
            Some(s) => s,
            None => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        if state.segments_sent >= PRL_DOWNLOAD_SEGMENT_LIMIT {
            log::warn!(
                "OTASP: PRL push exceeded {} segments without LAST_SEGMENT — aborting",
                PRL_DOWNLOAD_SEGMENT_LIMIT
            );
            self.prl_push = None;
            self.terminate(t, SessionOutcomeKind::ProtocolError);
            return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
        }
        let total = state.bytes.len();
        let offset = state.next_offset as usize;
        if offset > total {
            self.prl_push = None;
            self.terminate(t, SessionOutcomeKind::ProtocolError);
            return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
        }
        let remaining = total - offset;
        let take = remaining.min(PRL_DOWNLOAD_SEGMENT_SIZE as usize);
        let segment_data = &state.bytes[offset..offset + take];
        let last_segment = take == remaining;
        let param_data = match encode_sspr_param_data(last_segment, state.next_offset, segment_data)
        {
            Ok(b) => b,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let req = SsprDownloadRequest {
            block_id: state.block_id,
            param_data,
        };
        match req.encode() {
            Ok(b) => t.send(OutboundOtasp { bytes: b }),
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        }
        state.segments_sent += 1;
        self.phase = Phase::AwaitingSsprDownloadResponse;
        StepResult::Continue
    }

    fn handle_sspr_download_response<T: OtaspTransport>(
        &mut self,
        bytes: &[u8],
        t: &mut T,
    ) -> StepResult {
        use cdma_otasp::message::sspr::SsprDownloadResponse;
        let resp = match SsprDownloadResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        let (block_id, just_acked_offset, just_acked_size, total) = match self.prl_push.as_ref() {
            Some(s) => {
                let total = s.bytes.len();
                let offset = s.next_offset as usize;
                let size = (total - offset).min(PRL_DOWNLOAD_SEGMENT_SIZE as usize);
                (s.block_id, s.next_offset, size as u8, total)
            }
            None => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        if resp.block_id != block_id
            || resp.segment_offset != just_acked_offset
            || resp.segment_size != just_acked_size
        {
            log::warn!(
                "OTASP: SSPR Download Response mismatch (got block=0x{:02x} offset={} size={}, expected block=0x{:02x} offset={} size={})",
                resp.block_id,
                resp.segment_offset,
                resp.segment_size,
                block_id,
                just_acked_offset,
                just_acked_size
            );
            self.prl_push = None;
            self.terminate(t, SessionOutcomeKind::ProtocolError);
            return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
        }
        if !resp.result_code.is_accepted() {
            t.emit(OtaspEvent::BlockRejected {
                block_id,
                result_code: resp.result_code.to_u8(),
                feature: BlockFeature::Prl,
            });
            self.prl_push = None;
            self.terminate(t, SessionOutcomeKind::Rejected);
            return StepResult::Terminal(SessionOutcomeKind::Rejected);
        }
        // Accepted. Advance the offset and decide whether we just sent
        // the last segment.
        let new_offset = (just_acked_offset as usize) + (just_acked_size as usize);
        let last = new_offset >= total;
        if let Some(s) = self.prl_push.as_mut() {
            s.next_offset = new_offset as u16;
        }
        if last {
            let segments = self
                .prl_push
                .as_ref()
                .map(|s| s.segments_sent + 1)
                .unwrap_or(0);
            let prl_meta = self.hlr.as_ref().and_then(|h| h.prl_meta.as_ref());
            let mut fields = vec![
                (
                    "Variant".into(),
                    if block_id == 0x01 {
                        "Extended"
                    } else {
                        "Classic"
                    }
                    .into(),
                ),
                ("Bytes pushed".into(), total.to_string()),
                ("Segments".into(), segments.to_string()),
            ];
            if let Some(m) = prl_meta {
                fields.push(("PR_LIST_ID".into(), format!("0x{:04x}", m.pr_list_id)));
                fields.push(("SSPR_P_REV".into(), m.sspr_p_rev.to_string()));
            }
            t.emit(OtaspEvent::BlockDownloaded {
                block_id,
                result_code: resp.result_code.to_u8(),
                feature: BlockFeature::Prl,
                fields,
            });
            self.prl_push = None;
            self.completed_blocks += 1;
            self.plan_idx += 1;
            return self.advance_to_next_target(t);
        }
        self.send_next_prl_push_segment(t)
    }

    fn handle_commit<T: OtaspTransport>(&mut self, bytes: &[u8], t: &mut T) -> StepResult {
        let resp = match CommitResponse::decode(bytes) {
            Ok(r) => r,
            Err(_) => {
                self.terminate(t, SessionOutcomeKind::ProtocolError);
                return StepResult::Terminal(SessionOutcomeKind::ProtocolError);
            }
        };
        t.emit(OtaspEvent::CommitResult {
            result_code: resp.result.to_u8(),
        });
        let kind = if resp.result.is_accepted() {
            SessionOutcomeKind::Committed
        } else {
            SessionOutcomeKind::Rejected
        };
        self.terminate(t, kind);
        StepResult::Terminal(kind)
    }
}

enum StepResult {
    Continue,
    Terminal(SessionOutcomeKind),
}

/// Decode a NAM Configuration-Response param-data byte string into a
/// list of `(label, value)` pairs for operator display. Falls back to a
/// "raw hex" entry if the block can't be decoded.
/// Field list for a NAM Download Request — only the fields we
/// actually write to the MS (no `SCM`, `MOB_P_REV`, `MAX_SID_NID`,
/// `SLOTTED_MODE`, which are response-only / RO).
fn nam_download_fields(target: WriteTarget, nam: &AssembledNam) -> Vec<(String, String)> {
    use cdma_otasp::imsi::{imsi_11_12_to_digits, imsi_s_to_digits_checked, mcc_to_digits};
    fn yn(b: bool) -> &'static str {
        if b { "yes" } else { "no" }
    }
    match target {
        WriteTarget::CdmaAnalogNam => {
            let d = &nam.cdma_analog;
            let pairs = d
                .sid_nid_pairs
                .iter()
                .map(|p| format!("({},{})", p.sid, p.nid))
                .collect::<Vec<_>>()
                .join(" ");
            vec![
                (
                    "MCC".into(),
                    mcc_to_digits(d.mcc_m).unwrap_or_else(|| format!("0x{:03X}", d.mcc_m)),
                ),
                (
                    "IMSI_11_12".into(),
                    imsi_11_12_to_digits(d.imsi_m_11_12)
                        .unwrap_or_else(|| format!("0x{:02X}", d.imsi_m_11_12)),
                ),
                (
                    "IMSI_S".into(),
                    imsi_s_to_digits_checked(d.imsi_m_s as u32, (d.imsi_m_s >> 24) as u16)
                        .unwrap_or_else(|| format!("0x{:09X}", d.imsi_m_s)),
                ),
                ("HOME_SID".into(), d.home_sid.to_string()),
                ("FIRSTCHP".into(), d.firstchp.to_string()),
                ("ACCOLC".into(), d.accolc.to_string()),
                ("IMSI_M_CLASS".into(), (d.imsi_m_class as u8).to_string()),
                ("IMSI_M_ADDR_NUM".into(), d.imsi_m_addr_num.to_string()),
                ("EX".into(), yn(d.ex).to_string()),
                ("MOB_TERM_HOME".into(), yn(d.mob_term_home).to_string()),
                (
                    "MOB_TERM_FOR_SID".into(),
                    yn(d.mob_term_for_sid).to_string(),
                ),
                (
                    "MOB_TERM_FOR_NID".into(),
                    yn(d.mob_term_for_nid).to_string(),
                ),
                ("LOCAL_CONTROL".into(), yn(d.local_control).to_string()),
                (
                    "SID/NID pairs".into(),
                    if pairs.is_empty() {
                        "(none)".into()
                    } else {
                        pairs
                    },
                ),
            ]
        }
        WriteTarget::CdmaNam => {
            let d = &nam.cdma;
            let pairs = d
                .sid_nid_pairs
                .iter()
                .map(|p| format!("({},{})", p.sid, p.nid))
                .collect::<Vec<_>>()
                .join(" ");
            vec![
                (
                    "MCC".into(),
                    mcc_to_digits(d.mcc_m).unwrap_or_else(|| format!("0x{:03X}", d.mcc_m)),
                ),
                (
                    "IMSI_11_12".into(),
                    imsi_11_12_to_digits(d.imsi_m_11_12)
                        .unwrap_or_else(|| format!("0x{:02X}", d.imsi_m_11_12)),
                ),
                (
                    "IMSI_S".into(),
                    imsi_s_to_digits_checked(d.imsi_m_s as u32, (d.imsi_m_s >> 24) as u16)
                        .unwrap_or_else(|| format!("0x{:09X}", d.imsi_m_s)),
                ),
                ("ACCOLC".into(), d.accolc.to_string()),
                ("IMSI_M_CLASS".into(), (d.imsi_m_class as u8).to_string()),
                ("IMSI_M_ADDR_NUM".into(), d.imsi_m_addr_num.to_string()),
                ("MOB_TERM_HOME".into(), yn(d.mob_term_home).to_string()),
                (
                    "MOB_TERM_FOR_SID".into(),
                    yn(d.mob_term_for_sid).to_string(),
                ),
                (
                    "MOB_TERM_FOR_NID".into(),
                    yn(d.mob_term_for_nid).to_string(),
                ),
                ("LOCAL_CONTROL".into(), yn(d.local_control).to_string()),
                (
                    "SID/NID pairs".into(),
                    if pairs.is_empty() {
                        "(none)".into()
                    } else {
                        pairs
                    },
                ),
            ]
        }
        WriteTarget::Mdn => {
            let d = &nam.mdn;
            vec![
                ("Digits".into(), d.digits.clone()),
                ("Length".into(), d.digits.chars().count().to_string()),
            ]
        }
        WriteTarget::HomeSystemTag | WriteTarget::MmsUri | WriteTarget::Prl => Vec::new(),
    }
}

fn decode_nam_block_to_fields(target: WriteTarget, bytes: &[u8]) -> Vec<(String, String)> {
    use cdma_otasp::imsi::{imsi_11_12_to_digits, imsi_s_to_digits_checked, mcc_to_digits};
    fn yn(b: bool) -> &'static str {
        if b { "yes" } else { "no" }
    }
    match target {
        WriteTarget::CdmaAnalogNam => match NamCdmaAnalog::decode(bytes) {
            Ok(d) => {
                let mut v = Vec::new();
                v.push((
                    "MCC".into(),
                    mcc_to_digits(d.mcc_m).unwrap_or_else(|| format!("0x{:03X}", d.mcc_m)),
                ));
                v.push((
                    "IMSI_11_12".into(),
                    imsi_11_12_to_digits(d.imsi_m_11_12)
                        .unwrap_or_else(|| format!("0x{:02X}", d.imsi_m_11_12)),
                ));
                v.push((
                    "IMSI_S".into(),
                    imsi_s_to_digits_checked(d.imsi_m_s as u32, (d.imsi_m_s >> 24) as u16)
                        .unwrap_or_else(|| format!("0x{:09X}", d.imsi_m_s)),
                ));
                v.push(("HOME_SID".into(), d.home_sid.to_string()));
                v.push(("FIRSTCHP".into(), d.firstchp.to_string()));
                v.push(("ACCOLC".into(), d.accolc.to_string()));
                v.push(("IMSI_M_CLASS".into(), (d.imsi_m_class as u8).to_string()));
                v.push(("MOB_P_REV".into(), format!("{}", d.mob_p_rev)));
                v.push(("SCM".into(), format!("0x{:02X}", d.scm)));
                v.push(("EX".into(), yn(d.ex).to_string()));
                v.push(("MOB_TERM_HOME".into(), yn(d.mob_term_home).to_string()));
                v.push((
                    "MOB_TERM_FOR_SID".into(),
                    yn(d.mob_term_for_sid).to_string(),
                ));
                v.push((
                    "MOB_TERM_FOR_NID".into(),
                    yn(d.mob_term_for_nid).to_string(),
                ));
                v.push(("LOCAL_CONTROL".into(), yn(d.local_control).to_string()));
                v.push(("MAX_SID_NID".into(), d.max_sid_nid.to_string()));
                let pairs = d
                    .sid_nid_pairs
                    .iter()
                    .map(|p| format!("({},{})", p.sid, p.nid))
                    .collect::<Vec<_>>()
                    .join(" ");
                v.push((
                    "SID/NID pairs".into(),
                    if pairs.is_empty() {
                        "(none)".into()
                    } else {
                        pairs
                    },
                ));
                v
            }
            Err(_) => vec![("raw".into(), hex_dump(bytes))],
        },
        WriteTarget::CdmaNam => match NamCdma::decode(bytes) {
            Ok(d) => {
                let mut v = Vec::new();
                v.push((
                    "MCC".into(),
                    mcc_to_digits(d.mcc_m).unwrap_or_else(|| format!("0x{:03X}", d.mcc_m)),
                ));
                v.push((
                    "IMSI_11_12".into(),
                    imsi_11_12_to_digits(d.imsi_m_11_12)
                        .unwrap_or_else(|| format!("0x{:02X}", d.imsi_m_11_12)),
                ));
                v.push((
                    "IMSI_S".into(),
                    imsi_s_to_digits_checked(d.imsi_m_s as u32, (d.imsi_m_s >> 24) as u16)
                        .unwrap_or_else(|| format!("0x{:09X}", d.imsi_m_s)),
                ));
                v.push(("ACCOLC".into(), d.accolc.to_string()));
                v.push(("IMSI_M_CLASS".into(), (d.imsi_m_class as u8).to_string()));
                v.push(("MOB_P_REV".into(), format!("{}", d.mob_p_rev)));
                v.push(("SLOTTED_MODE".into(), yn(d.slotted_mode).to_string()));
                v.push(("MOB_TERM_HOME".into(), yn(d.mob_term_home).to_string()));
                v.push((
                    "MOB_TERM_FOR_SID".into(),
                    yn(d.mob_term_for_sid).to_string(),
                ));
                v.push((
                    "MOB_TERM_FOR_NID".into(),
                    yn(d.mob_term_for_nid).to_string(),
                ));
                v.push(("MAX_SID_NID".into(), d.max_sid_nid.to_string()));
                let pairs = d
                    .sid_nid_pairs
                    .iter()
                    .map(|p| format!("({},{})", p.sid, p.nid))
                    .collect::<Vec<_>>()
                    .join(" ");
                v.push((
                    "SID/NID pairs".into(),
                    if pairs.is_empty() {
                        "(none)".into()
                    } else {
                        pairs
                    },
                ));
                v
            }
            Err(_) => vec![("raw".into(), hex_dump(bytes))],
        },
        WriteTarget::Mdn => match cdma_otasp::param::mdn::MobileDirectoryNumber::decode(bytes) {
            Ok(d) => vec![
                ("Digits".into(), d.digits.clone()),
                ("Length".into(), d.digits.chars().count().to_string()),
            ],
            Err(_) => vec![("raw".into(), hex_dump(bytes))],
        },
        WriteTarget::HomeSystemTag => vec![("raw".into(), hex_dump(bytes))],
        WriteTarget::MmsUri => vec![("raw".into(), hex_dump(bytes))],
        WriteTarget::Prl => vec![("raw".into(), hex_dump(bytes))],
    }
}

fn hex_dump(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

fn merge_readback(existing: Option<NamReadback>, new: NamReadback) -> NamReadback {
    let mut out = existing.unwrap_or_default();
    if new.scm != 0 {
        out.scm = new.scm;
    }
    if new.mob_p_rev != 0 {
        out.mob_p_rev = new.mob_p_rev;
    }
    if new.max_sid_nid != 0 {
        out.max_sid_nid = new.max_sid_nid;
    }
    out.slotted_mode = new.slotted_mode || out.slotted_mode;
    out.ex = new.ex;
    out.local_control = new.local_control;
    // FIRSTCHP comes only from the CDMA/Analog block; 0 means "not read
    // yet", so keep a previously-learned value rather than clobbering it.
    if new.firstchp != 0 {
        out.firstchp = new.firstchp;
    }
    out
}

/// Borrow checker workaround: silence the unused `ResultCode` import warning
/// if linting becomes aggressive. (Kept available for future use.)
#[allow(dead_code)]
fn _ensure_result_code_visible(r: ResultCode) -> u8 {
    r.to_u8()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NamDefaultsConfig, OtaspWritesConfig, SystemTagConfig};
    use cdma_otasp::message::protocol_capability::{BandModeCap, ProtocolCapabilityResponse};

    #[derive(Default)]
    struct Recorder {
        outbound: Vec<OutboundOtasp>,
        events: Vec<OtaspEvent>,
    }

    impl OtaspTransport for Recorder {
        fn send(&mut self, message: OutboundOtasp) {
            self.outbound.push(message);
        }
        fn emit(&mut self, event: OtaspEvent) {
            self.events.push(event);
        }
    }

    /// Every writable block flipped on. Production default is all-off;
    /// these tests exercise the write paths so they need this opt-in.
    fn writes_all_on() -> OtaspWritesConfig {
        OtaspWritesConfig {
            cdma_analog_nam: true,
            mdn: true,
            cdma_nam: true,
            home_system_tag: true,
            mms_uri: false,
            prl: false,
        }
    }

    fn cfg(writes: OtaspWritesConfig) -> OtaspConfig {
        OtaspConfig {
            enabled: true,
            feature_codes: vec!["*228".to_string()],
            spc_policy: "leave_default".to_string(),
            system_tag: SystemTagConfig {
                name: "1xBTS".to_string(),
                tag_p_rev: 1,
            },
            nam_defaults: NamDefaultsConfig::default(),
            mms: crate::config::MmsConfig::default(),
            writes,
        }
    }

    fn overhead() -> BtsOverheadConfig {
        BtsOverheadConfig {
            mcc: "310".to_string(),
            imsi_11_12: "55".to_string(),
            sid: 22,
            nid: 1,
            paging_channel_number: 1,
        }
    }

    fn hlr() -> ResolvedSubscriberInput {
        ResolvedSubscriberInput {
            imsi: "310550123456789".to_string(),
            phone_number: "5551234567".to_string(),
            prl_bytes: None,
            prl_meta: None,
            service_programming_code: None,
            firstchp_override: None,
        }
    }

    fn device() -> HardwareIdentity {
        HardwareIdentity {
            esn: Some(0x12345678),
            meid: None,
        }
    }

    fn pcap_resp_bytes() -> Vec<u8> {
        use cdma_otasp::message::protocol_capability::{FeatureCapability, feature_id};
        ProtocolCapabilityResponse {
            mob_firm_rev: 0x0001,
            mob_model: 0x42,
            features: vec![
                FeatureCapability {
                    feature_id: feature_id::NAM_DOWNLOAD,
                    feature_p_rev: 2,
                },
                FeatureCapability {
                    feature_id: feature_id::OTASP,
                    feature_p_rev: 1,
                },
                FeatureCapability {
                    feature_id: feature_id::SYSTEM_TAG_DOWNLOAD,
                    feature_p_rev: 1,
                },
            ],
            band_mode_cap: BandModeCap::from_byte(0b0100_0000),
            additional_trailing: vec![],
        }
        .encode()
        .unwrap()
    }

    fn validation_resp_accept_bytes() -> Vec<u8> {
        ValidationResponse {
            results: vec![(VBLOCK_VERIFY_SPC, ResultCode::Accepted)],
        }
        .encode()
        .unwrap()
    }

    fn nam_config_resp_bytes(block_id: u8, param: Vec<u8>) -> Vec<u8> {
        ConfigurationResponse {
            blocks: vec![
                cdma_otasp::message::configuration::ConfigurationParamBlock {
                    block_id,
                    param_data: param,
                },
            ],
            results: vec![ResultCode::Accepted],
        }
        .encode()
        .unwrap()
    }

    fn cdma_analog_block_with_max_sid_nid(max_sid_nid: u8) -> Vec<u8> {
        NamCdmaAnalog {
            firstchp: 1,
            home_sid: 22,
            ex: false,
            scm: 0x52,
            mob_p_rev: 6,
            imsi_m_class: false,
            imsi_m_addr_num: 0,
            mcc_m: 209,
            imsi_m_11_12: 44,
            imsi_m_s: 0,
            accolc: 7,
            local_control: false,
            mob_term_home: true,
            mob_term_for_sid: true,
            mob_term_for_nid: true,
            max_sid_nid,
            sid_nid_pairs: vec![],
        }
        .encode_configuration_response()
        .unwrap()
    }

    fn cdma_nam_block_with_max_sid_nid(max_sid_nid: u8) -> Vec<u8> {
        NamCdma {
            slotted_mode: true,
            mob_p_rev: 6,
            imsi_m_class: false,
            imsi_m_addr_num: 0,
            mcc_m: 209,
            imsi_m_11_12: 44,
            imsi_m_s: 0,
            accolc: 7,
            local_control: false,
            mob_term_home: true,
            mob_term_for_sid: true,
            mob_term_for_nid: true,
            max_sid_nid,
            sid_nid_pairs: vec![],
        }
        .encode_configuration_response()
        .unwrap()
    }

    fn mdn_block_bytes() -> Vec<u8> {
        cdma_otasp::param::mdn::MobileDirectoryNumber::new("5550000000")
            .encode()
            .unwrap()
    }

    fn download_resp_accept(block_id: u8) -> Vec<u8> {
        DownloadResponse {
            results: vec![(block_id, ResultCode::Accepted)],
        }
        .encode()
        .unwrap()
    }

    fn system_tag_config_resp_accept() -> Vec<u8> {
        SystemTagConfigResponse {
            block_id: BLOCK_HOME_SYSTEM_TAG,
            result: ResultCode::Accepted,
            param_data: vec![],
        }
        .encode()
        .unwrap()
    }

    fn system_tag_download_resp_accept() -> Vec<u8> {
        SystemTagDownloadResponse {
            block_id: BLOCK_HOME_SYSTEM_TAG,
            result: ResultCode::Accepted,
            segment_progress: None,
        }
        .encode()
        .unwrap()
    }

    fn commit_resp_accept() -> Vec<u8> {
        CommitResponse {
            result: ResultCode::Accepted,
        }
        .encode()
    }

    fn start_session(writes: OtaspWritesConfig) -> (OtaspSession, Recorder) {
        let mut sess = OtaspSession::new(
            cfg(writes),
            overhead(),
            device(),
            "*228".to_string(),
            18,
            Some(hlr()),
        );
        let mut rec = Recorder::default();
        sess.start(&mut rec);
        (sess, rec)
    }

    #[test]
    fn all_on_session_walks_full_sequence() {
        let (mut sess, mut rec) = start_session(writes_all_on());
        assert_eq!(rec.outbound.len(), 1);
        let outcome = sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        assert!(outcome.is_none());
        let outcome = sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        assert!(outcome.is_none());
        // CDMA/Analog NAM Config → Download
        let outcome = sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            &mut rec,
        );
        assert!(outcome.is_none());
        let outcome = sess.on_inbound(&download_resp_accept(BLOCK_CDMA_ANALOG_NAM), &mut rec);
        assert!(outcome.is_none());
        // MDN
        let outcome = sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_MDN, mdn_block_bytes()),
            &mut rec,
        );
        assert!(outcome.is_none());
        let outcome = sess.on_inbound(&download_resp_accept(BLOCK_MDN), &mut rec);
        assert!(outcome.is_none());
        // CDMA NAM
        let outcome = sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_NAM, cdma_nam_block_with_max_sid_nid(4)),
            &mut rec,
        );
        assert!(outcome.is_none());
        let outcome = sess.on_inbound(&download_resp_accept(BLOCK_CDMA_NAM), &mut rec);
        assert!(outcome.is_none());
        // System Tag
        let outcome = sess.on_inbound(&system_tag_config_resp_accept(), &mut rec);
        assert!(outcome.is_none());
        let outcome = sess.on_inbound(&system_tag_download_resp_accept(), &mut rec);
        assert!(outcome.is_none());
        // Commit
        let outcome = sess.on_inbound(&commit_resp_accept(), &mut rec).unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::Committed);
        assert_eq!(outcome.completed_blocks, 4);
        assert!(
            rec.events
                .iter()
                .any(|e| matches!(e, OtaspEvent::CommitResult { result_code: 0 }))
        );
    }

    #[test]
    fn only_system_tag_skips_nam_blocks() {
        let writes = OtaspWritesConfig {
            cdma_analog_nam: false,
            mdn: false,
            cdma_nam: false,
            home_system_tag: true,
            mms_uri: false,
            prl: false,
        };
        let (mut sess, mut rec) = start_session(writes);
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        // NAM blocks still walked for read-back, no Download issued.
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            &mut rec,
        );
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_MDN, mdn_block_bytes()),
            &mut rec,
        );
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_NAM, cdma_nam_block_with_max_sid_nid(4)),
            &mut rec,
        );
        sess.on_inbound(&system_tag_config_resp_accept(), &mut rec);
        sess.on_inbound(&system_tag_download_resp_accept(), &mut rec);
        let outcome = sess.on_inbound(&commit_resp_accept(), &mut rec).unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::Committed);
        assert_eq!(outcome.completed_blocks, 1);
    }

    #[test]
    fn only_nam_no_system_tag() {
        let writes = OtaspWritesConfig {
            cdma_analog_nam: true,
            mdn: true,
            cdma_nam: true,
            home_system_tag: false,
            mms_uri: false,
            prl: false,
        };
        let (mut sess, mut rec) = start_session(writes);
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            &mut rec,
        );
        sess.on_inbound(&download_resp_accept(BLOCK_CDMA_ANALOG_NAM), &mut rec);
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_MDN, mdn_block_bytes()),
            &mut rec,
        );
        sess.on_inbound(&download_resp_accept(BLOCK_MDN), &mut rec);
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_NAM, cdma_nam_block_with_max_sid_nid(4)),
            &mut rec,
        );
        sess.on_inbound(&download_resp_accept(BLOCK_CDMA_NAM), &mut rec);
        let outcome = sess.on_inbound(&commit_resp_accept(), &mut rec).unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::Committed);
        assert_eq!(outcome.completed_blocks, 3);
    }

    #[test]
    fn all_off_still_sends_commit() {
        // All writes off — the session still issues Configuration Requests
        // for each NAM block for read-back diagnostic value, then sends
        // Commit. (Vintage MS firmwares need a Commit to show "successful.")
        let writes = OtaspWritesConfig {
            cdma_analog_nam: false,
            mdn: false,
            cdma_nam: false,
            home_system_tag: false,
            mms_uri: false,
            prl: false,
        };
        let (mut sess, mut rec) = start_session(writes);
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        // Each NAM block: feed a Config Response. No Download follows since
        // writes are off; the driver advances to the next block.
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            &mut rec,
        );
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_MDN, mdn_block_bytes()),
            &mut rec,
        );
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_NAM, cdma_nam_block_with_max_sid_nid(4)),
            &mut rec,
        );
        let any_commit = rec
            .outbound
            .iter()
            .any(|m| m.bytes == CommitRequest.encode());
        assert!(
            any_commit,
            "Commit must be sent even when nothing was written"
        );
        // Read-back events emitted for the three NAM blocks.
        let readback_count = rec
            .events
            .iter()
            .filter(|e| matches!(e, OtaspEvent::NamReadback { .. }))
            .count();
        assert_eq!(readback_count, 3);
        let outcome = sess
            .on_inbound(&commit_resp_accept(), &mut rec)
            .expect("commit response terminates");
        assert_eq!(outcome.kind, SessionOutcomeKind::Committed);
        assert_eq!(outcome.completed_blocks, 0);
    }

    #[test]
    fn spc_mismatch_terminates() {
        let (mut sess, mut rec) = start_session(writes_all_on());
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        let bad = ValidationResponse {
            results: vec![(VBLOCK_VERIFY_SPC, ResultCode::RejectedInvalidSpc)],
        }
        .encode()
        .unwrap();
        let outcome = sess.on_inbound(&bad, &mut rec).unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::SpcRejected);
        assert!(
            rec.events
                .iter()
                .any(|e| matches!(e, OtaspEvent::SpcMismatch))
        );
    }

    #[test]
    fn hlr_miss_terminates_after_validation() {
        let mut sess = OtaspSession::new(
            cfg(writes_all_on()),
            overhead(),
            device(),
            "*228".to_string(),
            18,
            None, // HLR miss
        );
        let mut rec = Recorder::default();
        sess.start(&mut rec);
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        let outcome = sess
            .on_inbound(&validation_resp_accept_bytes(), &mut rec)
            .unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::HlrUnknown);
        assert!(
            rec.events
                .iter()
                .any(|e| matches!(e, OtaspEvent::HlrMiss { .. }))
        );
    }

    #[test]
    fn no_capacity_terminates() {
        let (mut sess, mut rec) = start_session(writes_all_on());
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        let outcome = sess
            .on_inbound(
                &nam_config_resp_bytes(
                    BLOCK_CDMA_ANALOG_NAM,
                    cdma_analog_block_with_max_sid_nid(0),
                ),
                &mut rec,
            )
            .unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::NoCapacity);
        assert!(
            rec.events
                .iter()
                .any(|e| matches!(e, OtaspEvent::NoNamCapacity { .. }))
        );
    }

    #[test]
    fn first_download_rejected_skips_commit() {
        let (mut sess, mut rec) = start_session(writes_all_on());
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            &mut rec,
        );
        let bad = DownloadResponse {
            results: vec![(BLOCK_CDMA_ANALOG_NAM, ResultCode::RejectedInvalidParameter)],
        }
        .encode()
        .unwrap();
        let outcome = sess.on_inbound(&bad, &mut rec).unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::Rejected);
        let any_commit = rec
            .outbound
            .iter()
            .any(|m| m.bytes == CommitRequest.encode());
        assert!(!any_commit);
    }

    /// Protocol Capability Response that advertises SSPR (FEATURE_ID
    /// 0x02) on top of the defaults so the PRL push tests can exercise
    /// the write path. SSPR is gated on this feature ad in
    /// `prune_plan_for_unsupported_features`.
    fn pcap_resp_bytes_with_sspr() -> Vec<u8> {
        use cdma_otasp::message::protocol_capability::{FeatureCapability, feature_id};
        ProtocolCapabilityResponse {
            mob_firm_rev: 0x0001,
            mob_model: 0x42,
            features: vec![
                FeatureCapability {
                    feature_id: feature_id::NAM_DOWNLOAD,
                    feature_p_rev: 2,
                },
                FeatureCapability {
                    feature_id: feature_id::OTASP,
                    feature_p_rev: 1,
                },
                FeatureCapability {
                    feature_id: feature_id::SSPR,
                    feature_p_rev: 1,
                },
            ],
            band_mode_cap: BandModeCap::from_byte(0b0100_0000),
            additional_trailing: vec![],
        }
        .encode()
        .unwrap()
    }

    /// Helper: a `ResolvedSubscriberInput` carrying a PRL ready to push.
    fn hlr_with_prl(bytes: Vec<u8>, sspr_p_rev: u8) -> ResolvedSubscriberInput {
        use crate::otasp::nam::ResolvedPrlMeta;
        ResolvedSubscriberInput {
            imsi: "310550123456789".to_string(),
            phone_number: "5551234567".to_string(),
            prl_bytes: Some(bytes),
            prl_meta: Some(ResolvedPrlMeta {
                pr_list_id: 0,
                sspr_p_rev,
            }),
            service_programming_code: None,
            firstchp_override: None,
        }
    }

    fn writes_prl_only() -> OtaspWritesConfig {
        OtaspWritesConfig {
            cdma_analog_nam: false,
            mdn: false,
            cdma_nam: false,
            home_system_tag: false,
            mms_uri: false,
            prl: true,
        }
    }

    /// SSPR Configuration Response carrying PRL Dimensions with
    /// CUR_PR_LIST_SIZE = 0 → the read-back resolves as
    /// `PrlOutcome::Absent` and the session moves on to the write
    /// plan. Lets PRL push tests skip the read-back round.
    fn sspr_config_resp_absent() -> Vec<u8> {
        use cdma_otasp::param::prl_dimensions::PrlDimensions;
        let dims = PrlDimensions {
            max_pr_list_size: 4096,
            cur_pr_list_size: 0,
            pr_list_id: 0,
            num_acq_recs: 0,
            num_sys_recs: 0,
        };
        SsprConfigurationResponse {
            block_id: 0x00,
            result_code: ResultCode::Accepted,
            param_data: dims.encode().unwrap(),
        }
        .encode()
        .unwrap()
    }

    fn sspr_download_resp_accept(block_id: u8, segment_offset: u16, segment_size: u8) -> Vec<u8> {
        cdma_otasp::message::sspr::SsprDownloadResponse {
            block_id,
            result_code: ResultCode::Accepted,
            segment_offset,
            segment_size,
        }
        .encode()
        .unwrap()
    }

    fn sspr_download_resp_reject(block_id: u8, segment_offset: u16, segment_size: u8) -> Vec<u8> {
        cdma_otasp::message::sspr::SsprDownloadResponse {
            block_id,
            result_code: ResultCode::RejectedInvalidParameter,
            segment_offset,
            segment_size,
        }
        .encode()
        .unwrap()
    }

    /// Walk through the three NAM read-back-only stages (no Download
    /// follows since `do_download = false` for each). Helper used by
    /// the PRL push tests so they don't have to re-derive this flow.
    fn step_through_nam_readbacks(sess: &mut OtaspSession, rec: &mut Recorder) {
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            rec,
        );
        sess.on_inbound(&nam_config_resp_bytes(BLOCK_MDN, mdn_block_bytes()), rec);
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_NAM, cdma_nam_block_with_max_sid_nid(4)),
            rec,
        );
    }

    #[test]
    fn prl_push_walks_segments_and_completes() {
        use cdma_otasp::message::sspr::BLOCK_PRL_CLASSIC;
        // 450-octet PRL → 3 segments at 200 (200, 200, 50).
        let prl_bytes = (0..450).map(|i| (i & 0xff) as u8).collect::<Vec<_>>();
        let mut sess = OtaspSession::new(
            cfg(writes_prl_only()),
            overhead(),
            device(),
            "*228".to_string(),
            18,
            Some(hlr_with_prl(prl_bytes, 1)),
        );
        let mut rec = Recorder::default();
        sess.start(&mut rec);

        sess.on_inbound(&pcap_resp_bytes_with_sspr(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        // SSPR read-back fires first because the MS advertised SSPR.
        // CUR_PR_LIST_SIZE = 0 → outcome = Absent, session moves on.
        sess.on_inbound(&sspr_config_resp_absent(), &mut rec);
        step_through_nam_readbacks(&mut sess, &mut rec);

        // First two segments: offset 0 size 200, offset 200 size 200.
        let outcome = sess.on_inbound(
            &sspr_download_resp_accept(BLOCK_PRL_CLASSIC, 0, 200),
            &mut rec,
        );
        assert!(outcome.is_none());
        let outcome = sess.on_inbound(
            &sspr_download_resp_accept(BLOCK_PRL_CLASSIC, 200, 200),
            &mut rec,
        );
        assert!(outcome.is_none());
        // Final segment: offset 400 size 50 → BlockDownloaded fires.
        let outcome = sess.on_inbound(
            &sspr_download_resp_accept(BLOCK_PRL_CLASSIC, 400, 50),
            &mut rec,
        );
        assert!(outcome.is_none());
        // Commit closes the session.
        let outcome = sess.on_inbound(&commit_resp_accept(), &mut rec).unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::Committed);
        assert_eq!(outcome.completed_blocks, 1);
        assert_eq!(
            rec.events
                .iter()
                .filter(|e| matches!(
                    e,
                    OtaspEvent::BlockDownloaded {
                        feature: BlockFeature::Prl,
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn prl_push_terminates_on_reject() {
        use cdma_otasp::message::sspr::BLOCK_PRL_CLASSIC;
        // Single-segment PRL (50 bytes).
        let prl_bytes = vec![0xAAu8; 50];
        let mut sess = OtaspSession::new(
            cfg(writes_prl_only()),
            overhead(),
            device(),
            "*228".to_string(),
            18,
            Some(hlr_with_prl(prl_bytes, 1)),
        );
        let mut rec = Recorder::default();
        sess.start(&mut rec);

        sess.on_inbound(&pcap_resp_bytes_with_sspr(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        // SSPR read-back fires first because the MS advertised SSPR.
        // CUR_PR_LIST_SIZE = 0 → outcome = Absent, session moves on.
        sess.on_inbound(&sspr_config_resp_absent(), &mut rec);
        step_through_nam_readbacks(&mut sess, &mut rec);
        let outcome = sess
            .on_inbound(
                &sspr_download_resp_reject(BLOCK_PRL_CLASSIC, 0, 50),
                &mut rec,
            )
            .unwrap();
        assert_eq!(outcome.kind, SessionOutcomeKind::Rejected);
        assert!(rec.events.iter().any(|e| matches!(
            e,
            OtaspEvent::BlockRejected {
                feature: BlockFeature::Prl,
                ..
            }
        )));
        // No Commit was sent.
        let any_commit = rec
            .outbound
            .iter()
            .any(|m| m.bytes == CommitRequest.encode());
        assert!(!any_commit);
    }

    #[test]
    fn on_dbm_failed_during_nam_config_emits_block_skipped_and_advances() {
        // Walk to AwaitingNamConfig(CdmaAnalogNam), then simulate
        // BSC reporting an AddsDeliverAck failure. The session
        // should emit BlockSkipped + advance to MDN config.
        let (mut sess, mut rec) = start_session(writes_all_on());
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        // We're now in Phase::AwaitingNamConfig(CdmaAnalogNam).
        sess.on_dbm_failed(0x00, &mut rec);
        let skipped = rec
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    OtaspEvent::BlockSkipped {
                        feature: BlockFeature::Nam,
                        block_id,
                        ..
                    } if *block_id == BLOCK_CDMA_ANALOG_NAM
                )
            })
            .count();
        assert_eq!(skipped, 1);
        // Driver should have advanced to the next target (MDN) and
        // sent its Configuration Request.
        let request_was_mdn = rec.outbound.iter().any(|m| {
            // ConfigurationRequest msg_type=0x00 then NUM_BLOCKS=1 then BLOCK_ID=BLOCK_MDN
            m.bytes.len() >= 3 && m.bytes[0] == 0x00 && m.bytes[2] == BLOCK_MDN
        });
        assert!(
            request_was_mdn,
            "expected MDN ConfigurationRequest after skip"
        );
    }

    #[test]
    fn on_dbm_failed_during_commit_terminates_protocol_error() {
        // Walk past the writes so we're at the Commit phase.
        let writes = OtaspWritesConfig {
            cdma_analog_nam: false,
            mdn: false,
            cdma_nam: false,
            home_system_tag: false,
            mms_uri: false,
            prl: false,
        };
        let (mut sess, mut rec) = start_session(writes);
        sess.on_inbound(&pcap_resp_bytes(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_ANALOG_NAM, cdma_analog_block_with_max_sid_nid(4)),
            &mut rec,
        );
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_MDN, mdn_block_bytes()),
            &mut rec,
        );
        sess.on_inbound(
            &nam_config_resp_bytes(BLOCK_CDMA_NAM, cdma_nam_block_with_max_sid_nid(4)),
            &mut rec,
        );
        // Now in Phase::AwaitingCommit.
        sess.on_dbm_failed(0x00, &mut rec);
        let outcome = sess.on_inbound(&commit_resp_accept(), &mut rec);
        // The session was terminated; on_inbound after termination
        // returns Some(ProtocolError) per the driver contract.
        assert!(matches!(
            outcome.map(|o| o.kind),
            Some(SessionOutcomeKind::ProtocolError)
        ));
    }

    #[test]
    fn prl_push_extended_uses_block_id_01() {
        use cdma_otasp::message::sspr::{BLOCK_PRL_EXTENDED, SsprDownloadRequest};
        let prl_bytes = vec![0x55u8; 30];
        let mut sess = OtaspSession::new(
            cfg(writes_prl_only()),
            overhead(),
            device(),
            "*228".to_string(),
            18,
            Some(hlr_with_prl(prl_bytes, 3)),
        );
        let mut rec = Recorder::default();
        sess.start(&mut rec);
        sess.on_inbound(&pcap_resp_bytes_with_sspr(), &mut rec);
        sess.on_inbound(&validation_resp_accept_bytes(), &mut rec);
        // SSPR read-back fires first because the MS advertised SSPR.
        // CUR_PR_LIST_SIZE = 0 → outcome = Absent, session moves on.
        sess.on_inbound(&sspr_config_resp_absent(), &mut rec);
        step_through_nam_readbacks(&mut sess, &mut rec);
        // First outbound after the NAM read-back round is the SSPR
        // Download Request. Pull the last sent OTASP message and
        // confirm BLOCK_ID = 0x01.
        let last = rec.outbound.last().expect("at least one outbound");
        let req = SsprDownloadRequest::decode(&last.bytes).expect("decodes");
        assert_eq!(req.block_id, BLOCK_PRL_EXTENDED);
    }
}
