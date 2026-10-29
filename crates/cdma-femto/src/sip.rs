mod otasp;

use self::otasp::{AddsAckResponses, AddsAckTransaction, PendingOtaspAnswer, SIP_T1};
use crate::cell::FemtoCellState;
use crate::config::{FcsConfig, SipConfig};
use crate::fap::FapRegistry;
use crate::fap::{FapFx2, FapMobileIdentityCandidate, MediaPacketFormat, SmsAckMode};
use crate::fcs::{FcsMessage, FemtoConvergenceServer};
use crate::voice::{
    RtpRelayHandle, RtpRelayPool, build_sdp_offer, parse_sdp_audio, rewrite_sdp_transport,
    sdp_speech_payload_types,
};
use cdma_ios::femto_interface_msg::{CONTENT_TYPE_FEMTO, CONTENT_TYPE_SDP, MimePart, Multipart};
use cdma_ios::{
    A2pBearerSessionParams, AddsPageAckMessage, AddsPageMessage, AddsTransferMessage, AddsUserPart,
    AssignmentCompleteMessage, AssignmentRequestMessage, CellId, ChannelNumber,
    ClearCompleteMessage, ClearRequestMessage, ConnectMessage, EncodedA1Message, Message,
    MessageType, MobileIdentity,
};
use cdma_sip::message::Method;
use cdma_sip::{Registrar, SipRequest, SipResponse, challenge_response, response};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket as StdUdpSocket};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const SIP_DIGEST_NONCE: &str = "1xbts-ims-nonce";

const SC_OK: u16 = 200;
const SC_TRYING: u16 = 100;
const SC_RINGING: u16 = 180;
const SC_SESSION_PROGRESS: u16 = 183;
const SC_BAD_REQUEST: u16 = 400;
const SC_FORBIDDEN: u16 = 403;
const SC_NOT_IMPLEMENTED: u16 = 501;
const SC_SERVICE_UNAVAILABLE: u16 = 503;
const SIP_CREDENTIAL_STATE_FILE: &str = "sip-credentials.json";
const A1_CAUSE_NORMAL_CALL_CLEARING: u8 = 0x09;
const SMS_DATA_BURST_TYPE: u8 = 0x03;
const IMSI_S_DIGITS: usize = 10;
const FULL_IMSI_DIGITS: usize = 15;

const H_P_ASSOCIATED_URI: &str = "P-Associated-URI";
const H_SERVICE_ROUTE: &str = "Service-Route";
const H_PATH: &str = "Path";
const H_AUTHENTICATION_INFO: &str = "Authentication-Info";
const H_RSEQ: &str = "RSeq";
const H_RACK: &str = "RAck";
const H_P_ASSERTED_IDENTITY: &str = "P-Asserted-Identity";

const SIP_INVITE_CSEQ: u32 = 1;
const SIP_FIRST_PRACK_CSEQ: u32 = 2;
const SIP_CSEQ_MAX: u64 = i32::MAX as u64;
const SIP_INVITE_TIMEOUT: Duration = Duration::from_secs(32);
const SIP_DIALOG_TIMER_INTERVAL: Duration = Duration::from_secs(1);
const SIP_DEFAULT_PORT: u16 = 5060;

#[derive(Debug, thiserror::Error)]
enum SipVoiceError {
    #[error("{message} has no A1 call ID")]
    MissingCallId { message: &'static str },
    #[error("no MO SIP dialog for A1 call_id={call_id}")]
    MissingMoDialog { call_id: u64 },
    #[error("no MT SIP dialog for A1 call {call_id}")]
    MissingMtDialog { call_id: u64 },
    #[error("MT SIP dialog {call_id} has no reliable provisional response")]
    MissingReliableResponse { call_id: u64 },
    #[error("MT Paging Request is not addressed by IMSI")]
    PagingRequiresImsi,
    #[error("no registered FAP for MT INVITE")]
    NoRegisteredFap,
    #[error("FCS SIP listener has no IPv4 address for SDP")]
    NoIpv4SdpAddress,
    #[error("MT SDP offer has no speech payload")]
    MissingSpeechPayload,
    #[error("registered FAP has no SIP public identity")]
    MissingPublicIdentity,
    #[error(transparent)]
    A1(#[from] cdma_ios::Error),
    #[error(transparent)]
    Transport(#[from] cdma_ios::A1TransportError),
    #[error(transparent)]
    Femto(#[from] cdma_ios::femto_interface_msg::FemtoError),
    #[error(transparent)]
    Fcs(#[from] crate::fcs::FcsError),
    #[error(transparent)]
    Voice(#[from] crate::voice::VoiceError),
}

fn is_sms_service_option_page(encoded: &EncodedA1Message) -> bool {
    encoded
        .decode()
        .ok()
        .and_then(|message| cdma_ios::PagingRequestMessage::decode(&message.payload).ok())
        .and_then(|paging| paging.service_option)
        == Some(cdma_ios::ServiceOption::SMS)
}

fn sip_transaction_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn sip_credential_state_path() -> Option<PathBuf> {
    std::env::var_os("FEMTO_STATE_DIR")
        .map(PathBuf::from)
        .map(|directory| directory.join(SIP_CREDENTIAL_STATE_FILE))
}

fn load_learned_sip_passwords() -> HashMap<String, String> {
    let Some(path) = sip_credential_state_path() else {
        return HashMap::new();
    };
    load_learned_sip_passwords_from(&path)
}

fn load_learned_sip_passwords_from(path: &std::path::Path) -> HashMap<String, String> {
    match std::fs::read(path) {
        Ok(contents) => match serde_json::from_slice(&contents) {
            Ok(passwords) => passwords,
            Err(error) => {
                log::warn!("cannot parse persisted femtocell SIP credentials: {error}");
                HashMap::new()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(error) => {
            log::warn!("cannot read persisted femtocell SIP credentials: {error}");
            HashMap::new()
        }
    }
}

pub(crate) fn persist_learned_sip_passwords(passwords: &HashMap<String, String>) {
    let Some(path) = sip_credential_state_path() else {
        return;
    };
    persist_learned_sip_passwords_to(passwords, &path);
}

fn persist_learned_sip_passwords_to(passwords: &HashMap<String, String>, path: &std::path::Path) {
    let Some(parent) = path.parent() else {
        return;
    };
    let result = (|| -> Result<(), std::io::Error> {
        std::fs::create_dir_all(parent)?;
        let contents = serde_json::to_vec(passwords).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary)?;
        #[cfg(unix)]
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        std::io::Write::write_all(&mut file, &contents)?;
        file.sync_all()?;
        std::fs::rename(temporary, path)?;
        Ok(())
    })();
    if let Err(error) = result {
        log::warn!("cannot persist femtocell SIP credentials: {error}");
    }
}

fn sip_message_cseq(transaction: u64) -> u32 {
    ((transaction % SIP_CSEQ_MAX) + 1) as u32
}

fn msc_rtp_bind_ip(config: &FcsConfig) -> Ipv4Addr {
    if let Some(bind_ip) = config.rtp_msc_bind_ip {
        return bind_ip;
    }
    let Some(remote) = config.msc_a1_addr.as_deref() else {
        return config.rtp_bind_ip;
    };
    let Some(remote) = remote
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.find(|address| address.is_ipv4()))
    else {
        return config.rtp_bind_ip;
    };
    let Ok(socket) = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return config.rtp_bind_ip;
    };
    if socket.connect(remote).is_err() {
        return config.rtp_bind_ip;
    }
    match socket.local_addr().map(|address| address.ip()) {
        Ok(std::net::IpAddr::V4(local)) => local,
        _ => config.rtp_bind_ip,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImsUser {
    impu: String,
    impi: String,
    password: String,
    feid: String,
}

/// The SIP registrar's IMS user table, shared so `ReloadConfig` can swap it without a restart.
pub(crate) type SharedSipUsers = std::sync::Arc<std::sync::RwLock<Vec<ImsUser>>>;

/// Build the IMS user table from SIP config.
pub(crate) fn build_ims_users(config: &SipConfig) -> Vec<ImsUser> {
    config
        .users
        .iter()
        .map(|u| ImsUser {
            impu: u.impu.clone(),
            impi: u.impi.clone(),
            password: u.password.clone(),
            feid: u
                .feid
                .clone()
                .unwrap_or_else(|| sip_uri_user(&u.impu).unwrap_or(&u.impi).to_string()),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredFap {
    fap_id: String,
    public_identity: String,
    peer: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FapRouteHint {
    pub serial_number: String,
    pub peer: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistrationSession {
    mobile_identity: String,
    response_uri: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredMobile {
    imsi: MobileIdentity,
    esn: Option<MobileIdentity>,
    meid: Option<MobileIdentity>,
    fap_id: String,
}

#[derive(Debug, Clone)]
struct PendingDownlink {
    message_type: MessageType,
    a1_call_id: Option<u64>,
    adds_page_ack: Option<(String, AddsPageAckMessage)>,
}

#[derive(Clone)]
struct MtVoiceDialog {
    fap_id: String,
    sip_call_id: String,
    invite_branch: String,
    request_uri: String,
    from: String,
    to: String,
    remote_to: Option<String>,
    peer: SocketAddr,
    rseq: Option<u32>,
    last_pracked_rseq: Option<u32>,
    next_prack_cseq: u32,
    relay: RtpRelayHandle,
    offered_speech_payload_types: Vec<u8>,
    service_option: u16,
    answered: bool,
    created_at: Instant,
    cancel_sent: bool,
}

#[derive(Clone)]
struct MoVoiceDialog {
    fap_id: String,
    invite: SipRequest,
    peer: SocketAddr,
    local: Option<SocketAddr>,
    relay: RtpRelayHandle,
    service_option: u16,
    rseq: u32,
    assignment_sent: bool,
    answered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VoiceAssignmentDialog {
    MobileTerminated,
    MobileOriginated,
}

fn select_voice_assignment_dialog(
    has_mobile_terminated: bool,
    has_mobile_originated: bool,
) -> Option<VoiceAssignmentDialog> {
    if has_mobile_terminated {
        Some(VoiceAssignmentDialog::MobileTerminated)
    } else if has_mobile_originated {
        Some(VoiceAssignmentDialog::MobileOriginated)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSmsCauseCode {
    mobile_identity: String,
    body: Vec<u8>,
}

pub struct ImsCore {
    realm: String,
    allow_unauthenticated: bool,
    users: SharedSipUsers,
    learned_passwords: std::sync::Arc<std::sync::RwLock<HashMap<String, String>>>,
    registrar: Mutex<Registrar>,
    fcs: FemtoConvergenceServer,
    fcs_by_fap: HashMap<String, (FemtoConvergenceServer, CellId)>,
    msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
    msc_a1_rx: Mutex<Option<mpsc::Receiver<EncodedA1Message>>>,
    cell_identifier: CellId,
    access_sessions: Mutex<HashMap<String, u64>>,
    call_mobile_identities: Mutex<HashMap<u64, String>>,
    call_fap: Mutex<HashMap<u64, String>>,
    registration_sessions: Mutex<HashMap<u64, RegistrationSession>>,
    registered_mobiles: Mutex<HashMap<String, RegisteredMobile>>,
    paging_sessions: Mutex<HashMap<String, u64>>,
    registered_faps: Mutex<HashMap<String, RegisteredFap>>,
    fap_registry: std::sync::Arc<FapRegistry>,
    fap_route_hint_tx: mpsc::UnboundedSender<FapRouteHint>,
    fap_route_hint_rx: Mutex<Option<mpsc::UnboundedReceiver<FapRouteHint>>>,
    next_call_id: AtomicU64,
    next_sip_transaction: AtomicU64,
    pending_downlink: Mutex<HashMap<String, PendingDownlink>>,
    pending_adds_page_acks: Mutex<HashMap<String, AddsPageAckMessage>>,
    adds_delivery_calls: Mutex<HashMap<(String, u32), u64>>,
    pending_otasp_answer: Mutex<HashMap<u64, PendingOtaspAnswer>>,
    adds_ack_responses: Mutex<AddsAckResponses>,
    sms_cause_code_tx: mpsc::UnboundedSender<PendingSmsCauseCode>,
    sms_cause_code_rx: Mutex<Option<mpsc::UnboundedReceiver<PendingSmsCauseCode>>>,
    cell_state: std::sync::Arc<FemtoCellState>,
    rtp_relays: RtpRelayPool,
    mt_voice_by_call: Mutex<HashMap<u64, MtVoiceDialog>>,
    mt_voice_by_sip: Mutex<HashMap<String, u64>>,
    mo_voice_by_call: Mutex<HashMap<u64, MoVoiceDialog>>,
    mo_voice_by_sip: Mutex<HashMap<String, u64>>,
    event_publisher: Option<cdma_events::EventPublisher>,
}

enum UplinkObservation<'a> {
    Fx2(&'a cdma_ios::femto_interface_msg::FemtoInterfaceMsg),
    A1Wire,
    AlreadyPublished,
}

impl ImsCore {
    pub fn new(config: &SipConfig) -> Self {
        Self::with_msc_a1(
            config,
            CellId {
                cell: 0x193,
                sector: 0,
            },
            None,
        )
    }

    pub fn with_msc_a1(
        config: &SipConfig,
        cell_identifier: CellId,
        msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
    ) -> Self {
        Self::with_msc_a1_and_cell_state(
            config,
            cell_identifier,
            msc_a1_tx,
            FemtoCellState::untracked(),
        )
    }

    pub fn with_msc_a1_and_cell_state(
        config: &SipConfig,
        cell_identifier: CellId,
        msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
        cell_state: std::sync::Arc<FemtoCellState>,
    ) -> Self {
        let fcs_config = FcsConfig::default();
        Self::with_runtime_config(config, &fcs_config, cell_identifier, msc_a1_tx, cell_state)
    }

    pub fn with_msc_a1_cell_state_and_faps(
        config: &SipConfig,
        fcs_config: &FcsConfig,
        cell_identifier: CellId,
        msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
        cell_state: std::sync::Arc<FemtoCellState>,
        fap_registry: std::sync::Arc<FapRegistry>,
    ) -> Self {
        Self::with_runtime_config_and_faps(
            config,
            fcs_config,
            cell_identifier,
            msc_a1_tx,
            cell_state,
            fap_registry,
        )
    }

    fn with_runtime_config(
        config: &SipConfig,
        fcs_config: &FcsConfig,
        cell_identifier: CellId,
        msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
        cell_state: std::sync::Arc<FemtoCellState>,
    ) -> Self {
        let fap_registry = std::sync::Arc::new(FapRegistry::compatibility(
            config.users.iter().map(|user| {
                user.feid
                    .clone()
                    .unwrap_or_else(|| sip_uri_user(&user.impu).unwrap_or(&user.impi).to_string())
            }),
            crate::config::FapImplementationKind::StandardXs0059,
        ));
        Self::with_runtime_config_and_faps(
            config,
            fcs_config,
            cell_identifier,
            msc_a1_tx,
            cell_state,
            fap_registry,
        )
    }

    fn with_runtime_config_and_faps(
        config: &SipConfig,
        fcs_config: &FcsConfig,
        cell_identifier: CellId,
        msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
        cell_state: std::sync::Arc<FemtoCellState>,
        fap_registry: std::sync::Arc<FapRegistry>,
    ) -> Self {
        let (sms_cause_code_tx, sms_cause_code_rx) = mpsc::unbounded_channel();
        let (fap_route_hint_tx, fap_route_hint_rx) = mpsc::unbounded_channel();
        let msc_rtp_bind_ip = msc_rtp_bind_ip(fcs_config);
        log::info!(
            "FCS RTP relay: FAP side {} MSC side {}",
            fcs_config.rtp_bind_ip,
            msc_rtp_bind_ip
        );
        let fcs_by_fap = config
            .users
            .iter()
            .filter_map(|user| {
                let feid = user
                    .feid
                    .clone()
                    .unwrap_or_else(|| sip_uri_user(&user.impu).unwrap_or(&user.impi).to_string());
                let definition = fap_registry.get(&feid)?;
                let fcs = FemtoConvergenceServer::with_imsi_prefix(
                    &definition.site.mcc,
                    &definition.site.imsi_11_12,
                )
                .expect("validated FAP site identity policy");
                Some((
                    feid,
                    (
                        fcs,
                        CellId {
                            cell: definition.site.cell_id,
                            sector: definition.site.sector,
                        },
                    ),
                ))
            })
            .collect();
        Self {
            realm: config.realm.clone(),
            allow_unauthenticated: config.allow_unauthenticated,
            users: std::sync::Arc::new(std::sync::RwLock::new(build_ims_users(config))),
            learned_passwords: std::sync::Arc::new(std::sync::RwLock::new(
                load_learned_sip_passwords(),
            )),
            registrar: Mutex::new(Registrar::new()),
            fcs: FemtoConvergenceServer::with_imsi_prefix(&fcs_config.mcc, &fcs_config.imsi_11_12)
                .expect("valid FCS IMSI prefix"),
            fcs_by_fap,
            msc_a1_tx,
            msc_a1_rx: Mutex::new(None),
            cell_identifier,
            access_sessions: Mutex::new(HashMap::new()),
            call_mobile_identities: Mutex::new(HashMap::new()),
            call_fap: Mutex::new(HashMap::new()),
            registration_sessions: Mutex::new(HashMap::new()),
            registered_mobiles: Mutex::new(HashMap::new()),
            paging_sessions: Mutex::new(HashMap::new()),
            registered_faps: Mutex::new(HashMap::new()),
            fap_registry,
            fap_route_hint_tx,
            fap_route_hint_rx: Mutex::new(Some(fap_route_hint_rx)),
            next_call_id: AtomicU64::new(1),
            next_sip_transaction: AtomicU64::new(sip_transaction_seed()),
            pending_downlink: Mutex::new(HashMap::new()),
            pending_adds_page_acks: Mutex::new(HashMap::new()),
            adds_delivery_calls: Mutex::new(HashMap::new()),
            pending_otasp_answer: Mutex::new(HashMap::new()),
            adds_ack_responses: Mutex::new(AddsAckResponses::default()),
            sms_cause_code_tx,
            sms_cause_code_rx: Mutex::new(Some(sms_cause_code_rx)),
            cell_state,
            rtp_relays: RtpRelayPool::new(
                fcs_config.rtp_bind_ip,
                msc_rtp_bind_ip,
                fcs_config.rtp_port_start,
                fcs_config.rtp_port_end,
            )
            .expect("valid FCS RTP relay range"),
            mt_voice_by_call: Mutex::new(HashMap::new()),
            mt_voice_by_sip: Mutex::new(HashMap::new()),
            mo_voice_by_call: Mutex::new(HashMap::new()),
            mo_voice_by_sip: Mutex::new(HashMap::new()),
            event_publisher: None,
        }
    }

    pub fn fap_route_hint_sender(&self) -> mpsc::UnboundedSender<FapRouteHint> {
        self.fap_route_hint_tx.clone()
    }

    pub fn learned_passwords(&self) -> std::sync::Arc<std::sync::RwLock<HashMap<String, String>>> {
        self.learned_passwords.clone()
    }

    pub(crate) fn sip_users_handle(&self) -> SharedSipUsers {
        self.users.clone()
    }

    pub fn with_msc_bridge(
        config: &SipConfig,
        cell_identifier: CellId,
        bridge: crate::fcs::MscA1Bridge,
    ) -> Self {
        let mut core = Self::with_msc_a1(config, cell_identifier, Some(bridge.uplink));
        *core.msc_a1_rx.get_mut().expect("MSC A1 receiver lock") = Some(bridge.downlink);
        core
    }

    pub fn with_msc_bridge_and_cell_state(
        config: &SipConfig,
        fcs_config: &FcsConfig,
        cell_identifier: CellId,
        bridge: crate::fcs::MscA1Bridge,
        cell_state: std::sync::Arc<FemtoCellState>,
    ) -> Self {
        let mut core = Self::with_runtime_config(
            config,
            fcs_config,
            cell_identifier,
            Some(bridge.uplink),
            cell_state,
        );
        *core.msc_a1_rx.get_mut().expect("MSC A1 receiver lock") = Some(bridge.downlink);
        core
    }

    pub fn with_msc_bridge_cell_state_and_faps(
        config: &SipConfig,
        fcs_config: &FcsConfig,
        cell_identifier: CellId,
        bridge: crate::fcs::MscA1Bridge,
        cell_state: std::sync::Arc<FemtoCellState>,
        fap_registry: std::sync::Arc<FapRegistry>,
    ) -> Self {
        let mut core = Self::with_runtime_config_and_faps(
            config,
            fcs_config,
            cell_identifier,
            Some(bridge.uplink),
            cell_state,
            fap_registry,
        );
        *core.msc_a1_rx.get_mut().expect("MSC A1 receiver lock") = Some(bridge.downlink);
        core
    }

    pub fn with_event_publisher(mut self, publisher: cdma_events::EventPublisher) -> Self {
        self.event_publisher = Some(publisher);
        self
    }

    fn full_imsi(fcs: &crate::fcs::FemtoConvergenceServer, identity: String) -> String {
        fcs.canonical_imsi(&identity).unwrap_or(identity)
    }

    /// Reading registration sessions non-destructively matters: the Location
    /// Updating Accept path removes the session, and does so after the event for
    /// that message has already been published.
    fn correlated_imsi_for_call(
        &self,
        fcs: &crate::fcs::FemtoConvergenceServer,
        call_id: u64,
    ) -> Option<String> {
        // Taken in two steps so no two of these locks are ever held at once.
        let from_calls = {
            let calls = self
                .call_mobile_identities
                .lock()
                .expect("call mobile identities lock");
            calls.get(&call_id).cloned()
        };
        let identity = match from_calls {
            Some(identity) => identity,
            None => {
                let sessions = self
                    .registration_sessions
                    .lock()
                    .expect("registration sessions lock");
                sessions.get(&call_id)?.mobile_identity.clone()
            }
        };
        Some(Self::full_imsi(fcs, identity))
    }

    // Typed A1 conversion drops unmodeled information elements.
    fn publish_a1(
        &self,
        fap_id: Option<&str>,
        message: &cdma_ios::femto_interface_msg::FemtoInterfaceMsg,
        context: crate::event::A1Context,
    ) {
        let Some(publisher) = self.event_publisher.as_ref() else {
            return;
        };
        publisher.publish(crate::event::a1_event(
            fap_id.unwrap_or_default(),
            message,
            &context,
        ));
    }

    fn a1_context(
        &self,
        direction: cdma_events::proto::A1Direction,
        fap_id: Option<&str>,
        call_id: Option<u64>,
        request: Option<&SipRequest>,
        peer: Option<SocketAddr>,
    ) -> crate::event::A1Context {
        let (fcs, cell_identifier) = self.fcs_for_fap(fap_id);
        crate::event::A1Context {
            direction,
            cell: Some(cell_identifier),
            sip_call_id: request
                .and_then(|request| request.headers.call_id())
                .map(str::to_string),
            content_type: request
                .and_then(|request| request.headers.content_type())
                .map(str::to_string),
            peer: peer.map(|peer| peer.to_string()),
            a1_call_id: call_id,
            correlated_imsi: call_id
                .and_then(|call_id| self.correlated_imsi_for_call(fcs, call_id)),
            peer_id: None,
        }
    }

    fn forward_to_msc(
        &self,
        what: &str,
        fap_id: Option<&str>,
        encoded: EncodedA1Message,
        observation: UplinkObservation<'_>,
        context: crate::event::A1Context,
    ) -> bool {
        let mut context = context;
        context.direction = cdma_events::proto::A1Direction::Uplink;
        if context.a1_call_id.is_none() {
            context.a1_call_id = encoded.call_id();
        }
        match observation {
            UplinkObservation::Fx2(body) => self.publish_a1(fap_id, body, context),
            UplinkObservation::A1Wire => self.publish_a1_wire(fap_id, &encoded, context),
            UplinkObservation::AlreadyPublished => {}
        }
        let Some(sender) = self.msc_a1_tx.as_ref() else {
            log::warn!("FCS: {what}: MSC A1 forwarding is not configured");
            return false;
        };
        if let Err(error) = sender.try_send(encoded) {
            log::warn!("FCS: {what}: MSC A1 forwarding queue unavailable: {error}");
            return false;
        }
        true
    }

    // Do not await between receiving and publishing or cancellation can lose the event.
    async fn next_downlink(
        &self,
        receiver: &mut Option<mpsc::Receiver<EncodedA1Message>>,
    ) -> Option<EncodedA1Message> {
        let encoded = receive_downlink(receiver).await?;
        self.publish_downlink(&encoded);
        Some(encoded)
    }

    fn publish_downlink(&self, encoded: &EncodedA1Message) {
        if self.event_publisher.is_none() {
            return;
        }
        let mobile_identity = self.downlink_mobile_identity(encoded).ok().flatten();
        let route = encoded
            .call_id()
            .and_then(|call_id| self.route_for_call(call_id))
            .or_else(|| {
                mobile_identity
                    .as_deref()
                    .and_then(|identity| self.route_for_mobile(identity))
            });
        let fap_id = route.as_ref().map(|route| route.fap_id.clone());
        let (fcs, _) = self.fcs_for_fap(fap_id.as_deref());
        let mut context = self.a1_context(
            cdma_events::proto::A1Direction::Downlink,
            fap_id.as_deref(),
            encoded.call_id(),
            None,
            route.as_ref().map(|route| route.peer),
        );
        if let Some(identity) = mobile_identity {
            context.correlated_imsi = Some(Self::full_imsi(fcs, identity));
        }
        match fcs.from_msc_a1(encoded) {
            Ok(femto) => self.publish_a1(fap_id.as_deref(), &femto, context),
            Err(_) => self.publish_a1_wire(fap_id.as_deref(), encoded, context),
        }
    }

    fn publish_a1_wire(
        &self,
        fap_id: Option<&str>,
        encoded: &EncodedA1Message,
        context: crate::event::A1Context,
    ) {
        let Some(publisher) = self.event_publisher.as_ref() else {
            return;
        };
        let Some(event) =
            crate::event::a1_event_from_wire(fap_id.unwrap_or_default(), encoded, &context)
        else {
            log::debug!(
                "FCS: A1 {:?} has no readable message framing to publish",
                encoded.message_type()
            );
            return;
        };
        publisher.publish(event);
    }

    fn user_for_impi(&self, impi: &str) -> Option<ImsUser> {
        let users = self.users.read().expect("sip users lock");
        users
            .iter()
            .find(|user| user.impi == impi)
            .or_else(|| {
                let (local_part, domain) = impi.rsplit_once('@')?;
                domain
                    .eq_ignore_ascii_case(&self.realm)
                    .then(|| users.iter().find(|user| user.impi == local_part))?
            })
            .or_else(|| {
                (!impi.contains('@')).then(|| {
                    users.iter().find(|user| {
                        user.impi
                            .rsplit_once('@')
                            .is_some_and(|(local_part, domain)| {
                                local_part == impi && domain.eq_ignore_ascii_case(&self.realm)
                            })
                    })
                })?
            })
            .cloned()
    }

    pub fn handle_request(&self, req: &SipRequest) -> SipResponse {
        self.handle_request_from(
            req,
            SocketAddr::from((Ipv4Addr::LOCALHOST, SIP_DEFAULT_PORT)),
        )
    }

    fn handle_request_from(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        match req.method {
            Method::Register => self.handle_register(req),
            Method::Invite => self.handle_mo_invite(req, peer),
            Method::Message => self.handle_message(req, peer),
            Method::Info => self.handle_otasp_info(req, peer),
            Method::Prack => self.handle_mo_prack(req, peer),
            Method::Bye => self.handle_bye(req, peer),
            Method::Cancel => self.handle_cancel(req, peer),
            Method::Update => response(req, SC_OK, "OK"),
            Method::Options => response(req, SC_OK, "OK"),
            _ => response(req, SC_NOT_IMPLEMENTED, "Not Implemented"),
        }
    }

    fn handle_bye(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        self.handle_mobile_clear(req, peer, "BYE")
    }

    fn handle_cancel(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        self.handle_mobile_clear(req, peer, "CANCEL")
    }

    fn handle_mobile_clear(&self, req: &SipRequest, peer: SocketAddr, method: &str) -> SipResponse {
        let Some(sip_call_id) = req.headers.call_id() else {
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let scoped_call_id = self.scoped_sip_call_id(peer, sip_call_id);
        let call_id = self
            .mo_voice_by_sip
            .lock()
            .expect("MO voice SIP dialog lock")
            .get(&scoped_call_id)
            .copied()
            .or_else(|| {
                self.mt_voice_by_sip
                    .lock()
                    .expect("MT voice SIP dialog lock")
                    .get(sip_call_id)
                    .copied()
            });
        let Some(call_id) = call_id else {
            return response(req, 481, "Call/Transaction Does Not Exist");
        };
        let clear = ClearRequestMessage {
            cause: cdma_ios::Cause(A1_CAUSE_NORMAL_CALL_CLEARING),
            cause_layer3: None,
        };
        let payload = match clear.encode() {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!("FCS: encoding Clear Request for call_id={call_id}: {error}");
                return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
            }
        };
        let fap_id = self.fap_id_for_peer(peer);
        if !self.forward_to_msc(
            &format!("forwarding SIP {method} as A1 Clear Request"),
            fap_id.as_deref(),
            EncodedA1Message::from_message_for_call(
                &Message::new(MessageType::ClearRequest, payload),
                Some(call_id),
            ),
            UplinkObservation::A1Wire,
            self.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                fap_id.as_deref(),
                Some(call_id),
                Some(req),
                Some(peer),
            ),
        ) {
            return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
        }
        log::info!("FCS: SIP {method} cleared A1 call_id={call_id}");
        response(req, SC_OK, "OK")
    }

    fn handle_mo_invite(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        let Some(sip_call_id) = req.headers.call_id().map(str::to_string) else {
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let scoped_sip_call_id = self.scoped_sip_call_id(peer, &sip_call_id);
        let existing_call = self
            .mo_voice_by_sip
            .lock()
            .expect("MO voice SIP dialog lock")
            .get(&scoped_sip_call_id)
            .copied();
        if let Some(call_id) = existing_call
            && let Some(answer) = self
                .pending_otasp_answer
                .lock()
                .expect("OTASP answer lock")
                .get(&call_id)
            && req.headers.cseq() == answer.response.headers.cseq()
        {
            return answer.response.clone();
        }
        if self
            .mo_voice_by_sip
            .lock()
            .expect("MO voice SIP dialog lock")
            .contains_key(&scoped_sip_call_id)
        {
            return response(req, SC_TRYING, "Trying");
        }
        let fap_id = self.source_fap(req, Some(peer));
        let fx2 = fap_id
            .as_deref()
            .and_then(|fap_id| self.fap_registry.get(fap_id))
            .map(|fap| fap.implementation.fx2());
        let femto = match request_femto_message_with_fx2(req, fx2) {
            Some(message)
                if message.message_type
                    == cdma_ios::femto_interface_msg::MSG_TYPE_CM_SERVICE_REQUEST =>
            {
                message
            }
            _ => {
                log::warn!("FCS: MO INVITE {sip_call_id} has no CM Service Request body");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let service_request = match femto.to_cm_service_request() {
            Ok(request) => request,
            Err(error) => {
                log::warn!("FCS: decoding MO INVITE {sip_call_id}: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let Some(fap_endpoint) = request_sdp_endpoint(req) else {
            log::warn!("FCS: MO INVITE {sip_call_id} has no usable SDP audio endpoint");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let call_id = match self.call_id_for_message(req, &femto, Some(peer)) {
            Ok(Some(call_id)) => call_id,
            Ok(None) => {
                log::warn!("FCS: MO INVITE {sip_call_id} has no correlatable mobile identity");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
            Err(error) => {
                log::warn!("FCS: correlating MO INVITE {sip_call_id}: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let (fcs, cell_identifier) = self.fcs_for_fap(fap_id.as_deref());
        self.publish_a1(
            fap_id.as_deref(),
            &femto,
            self.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                fap_id.as_deref(),
                Some(call_id),
                Some(req),
                Some(peer),
            ),
        );
        let a1 = match fcs.to_msc_a1(&femto, cell_identifier, Some(call_id)) {
            Ok(message) => message,
            Err(error) => {
                log::warn!("FCS: translating MO INVITE {sip_call_id} to A1: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let service_option = service_request
            .service_option
            .unwrap_or(cdma_ios::ServiceOption::EVRC_A)
            .0;
        let packet_format = self.media_packet_format(fap_id.as_deref());
        let relay = match self
            .rtp_relays
            .open_with_format(service_option, packet_format)
        {
            Ok(relay) => relay,
            Err(error) => {
                log::warn!("FCS: opening MO voice RTP relay: {error}");
                return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
            }
        };
        relay.set_fap_remote(SocketAddr::new(
            std::net::IpAddr::V4(fap_endpoint.address),
            fap_endpoint.port,
        ));
        if !self.forward_to_msc(
            "forwarding MO INVITE A1 to MSC",
            fap_id.as_deref(),
            a1,
            UplinkObservation::AlreadyPublished,
            crate::event::A1Context::default(),
        ) {
            return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
        }
        self.mo_voice_by_sip
            .lock()
            .expect("MO voice SIP dialog lock")
            .insert(scoped_sip_call_id, call_id);
        self.mo_voice_by_call
            .lock()
            .expect("MO voice A1 dialog lock")
            .insert(
                call_id,
                MoVoiceDialog {
                    fap_id: fap_id.unwrap_or_default(),
                    invite: req.clone(),
                    peer,
                    local: None,
                    relay,
                    service_option,
                    rseq: 1,
                    assignment_sent: false,
                    answered: false,
                },
            );
        let mobile = match service_request.mobile_identity_imsi {
            MobileIdentity::Imsi(identity) => identity,
            _ => "non-IMSI".to_string(),
        };
        log::info!(
            "FCS: accepted MO INVITE sip_call_id={sip_call_id} call_id={call_id} mobile={mobile} service_option={service_option} called_uri={}",
            req.uri
        );
        response(req, SC_TRYING, "Trying")
    }

    fn handle_mo_prack(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        let Some(sip_call_id) = req.headers.call_id() else {
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let scoped_call_id = self.scoped_sip_call_id(peer, sip_call_id);
        let call_id = self
            .mo_voice_by_sip
            .lock()
            .expect("MO voice SIP dialog lock")
            .get(&scoped_call_id)
            .copied();
        let Some(call_id) = call_id else {
            return response(req, 481, "Call/Transaction Does Not Exist");
        };
        let dialogs = self
            .mo_voice_by_call
            .lock()
            .expect("MO voice A1 dialog lock");
        let Some(dialog) = dialogs.get(&call_id) else {
            return response(req, 481, "Call/Transaction Does Not Exist");
        };
        if let Some(endpoint) = request_sdp_endpoint(req) {
            dialog.relay.set_fap_remote(SocketAddr::new(
                std::net::IpAddr::V4(endpoint.address),
                endpoint.port,
            ));
        }
        let fx2 = self
            .fap_registry
            .get(&dialog.fap_id)
            .map(|fap| fap.implementation.fx2());
        let Some(femto) = request_femto_message_with_fx2(req, fx2) else {
            log::warn!("FCS: MO PRACK call_id={call_id} has no Assignment Complete body");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let mut complete = match femto.to_assignment_complete() {
            Ok(complete) => complete,
            Err(error) => {
                log::warn!("FCS: decoding MO PRACK Assignment Complete: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        self.publish_a1(
            Some(&dialog.fap_id),
            &observed_body(&femto),
            self.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                Some(&dialog.fap_id),
                Some(call_id),
                Some(req),
                Some(peer),
            ),
        );
        let assigned_service_option = cdma_ios::ServiceOption(dialog.service_option);
        if complete.service_option != Some(assigned_service_option) {
            log::warn!(
                "FCS: MO call_id={call_id} Assignment Complete service option {:?} does not match assigned service option {}; preserving the assigned voice bearer",
                complete.service_option.map(|option| option.0),
                dialog.service_option
            );
        }
        complete.service_option = Some(assigned_service_option);
        complete.a2p_bearer_session_params = Some(A2pBearerSessionParams {
            ip_address: match dialog.relay.msc_local().ip() {
                std::net::IpAddr::V4(address) => address,
                _ => std::net::Ipv4Addr::UNSPECIFIED,
            },
            udp_port: dialog.relay.msc_local().port(),
        });
        let payload = match complete.encode() {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!("FCS: encoding MO Assignment Complete: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::AssignmentComplete, payload),
            Some(call_id),
        );
        if !self.forward_to_msc(
            "forwarding MO Assignment Complete to MSC",
            Some(&dialog.fap_id),
            encoded,
            UplinkObservation::AlreadyPublished,
            crate::event::A1Context::default(),
        ) {
            return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
        }
        log::info!("FCS: MO call_id={call_id} traffic assignment completed via PRACK");
        response(req, SC_OK, "OK")
    }

    fn mo_assignment_progress(
        &self,
        encoded: &EncodedA1Message,
        local: SocketAddr,
    ) -> Result<(SipResponse, SocketAddr), SipVoiceError> {
        let call_id = encoded.call_id().ok_or(SipVoiceError::MissingCallId {
            message: "MO Assignment Request",
        })?;
        let message = encoded.decode()?;
        let assignment = AssignmentRequestMessage::decode(&message.payload)?;
        let mut dialogs = self
            .mo_voice_by_call
            .lock()
            .expect("MO voice A1 dialog lock");
        let dialog = dialogs
            .get_mut(&call_id)
            .ok_or(SipVoiceError::MissingMoDialog { call_id })?;
        if let Some(parameters) = assignment.a2p_bearer_session_params {
            dialog.relay.set_msc_remote(SocketAddr::new(
                std::net::IpAddr::V4(parameters.ip_address),
                parameters.udp_port,
            ));
        }
        dialog.service_option = assignment
            .service_option
            .map(|option| option.0)
            .unwrap_or(dialog.service_option);
        let femto_assignment = AssignmentRequestMessage {
            circuit_identity_code: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
            ..assignment
        };
        let femto = cdma_ios::femto_interface_msg::FemtoInterfaceMsg::from_assignment_request(
            &femto_assignment,
        )?;
        let boundary = format!("1xbts-fx2-mo-{call_id}");
        let multipart = Multipart::new(
            &boundary,
            vec![
                MimePart::text(
                    self.fap_registry
                        .get(&dialog.fap_id)
                        .map(|fap| fap.implementation.fx2().preferred_femto_content_type())
                        .unwrap_or(CONTENT_TYPE_FEMTO),
                    femto.encode(),
                ),
                MimePart::text(
                    CONTENT_TYPE_SDP,
                    build_sdp_offer(
                        match dialog.relay.fap_local().ip() {
                            std::net::IpAddr::V4(address) => address,
                            _ => std::net::Ipv4Addr::UNSPECIFIED,
                        },
                        dialog.relay.fap_local().port(),
                        dialog.service_option,
                    ),
                ),
            ],
        );
        let mut progress = response(&dialog.invite, SC_SESSION_PROGRESS, "Session Progress");
        progress.headers.append("Require", "100rel");
        progress.headers.append(H_RSEQ, dialog.rseq.to_string());
        progress
            .headers
            .append("Contact", format!("<sip:fcs@{local}>"));
        progress.headers.append(
            "Content-Type",
            format!("multipart/mixed; boundary={boundary}"),
        );
        progress.body = multipart.encode();
        dialog.local = Some(local);
        dialog.assignment_sent = true;
        log::info!(
            "FCS: MO call_id={call_id} sending 183 with FAP RTP {} and service_option={}",
            dialog.relay.fap_local(),
            dialog.service_option
        );
        Ok((progress, dialog.peer))
    }

    fn mo_dialog_response(
        &self,
        encoded: &EncodedA1Message,
        status: u16,
        reason: &str,
    ) -> Option<(SipResponse, SocketAddr)> {
        let call_id = encoded.call_id()?;
        let mut dialogs = self
            .mo_voice_by_call
            .lock()
            .expect("MO voice A1 dialog lock");
        let dialog = dialogs.get_mut(&call_id)?;
        if status == SC_OK {
            if dialog.answered {
                return None;
            }
            dialog.answered = true;
        }
        let mut response = response(&dialog.invite, status, reason);
        if let Some(local) = dialog.local {
            response
                .headers
                .append("Contact", format!("<sip:fcs@{local}>"));
        }
        log::info!("FCS: MO call_id={call_id} sending SIP {status} {reason}");
        Some((response, dialog.peer))
    }

    fn handle_register(&self, req: &SipRequest) -> SipResponse {
        let mut authentication_info = None;
        if !self.allow_unauthenticated {
            let Some(auth) = req.headers.authorization() else {
                return challenge_response(req, &self.realm, SIP_DIGEST_NONCE);
            };
            let Some(username) = cdma_sip::auth::authorization_username(auth) else {
                return response(req, SC_BAD_REQUEST, "Bad Request");
            };
            let Some(user) = self.user_for_impi(&username) else {
                log::warn!("SIP REGISTER rejected: unknown IMPI {username}");
                return response(req, SC_FORBIDDEN, "Forbidden");
            };
            if self.fap_id_for_request(req).as_deref() != Some(user.feid.as_str()) {
                log::warn!(
                    "SIP REGISTER rejected: authenticated identity does not match target FAP"
                );
                return response(req, SC_FORBIDDEN, "Forbidden");
            }
            let nonce = cdma_sip::auth::auth_param(auth, "nonce").unwrap_or_default();
            let digest_response = cdma_sip::auth::auth_param(auth, "response").unwrap_or_default();
            if nonce.is_empty() || digest_response.is_empty() {
                return challenge_response(req, &self.realm, SIP_DIGEST_NONCE);
            }
            let learned_password = self
                .learned_passwords
                .read()
                .expect("learned SIP password lock")
                .get(&user.feid)
                .cloned();
            let password = learned_password.as_deref().unwrap_or(&user.password);
            if !cdma_sip::auth::verify(auth, "REGISTER", password, Some(&req.uri)) {
                log::warn!("SIP REGISTER rejected: invalid Digest for IMPI {username}");
                return response(req, SC_FORBIDDEN, "Forbidden");
            }
            authentication_info = cdma_sip::auth::authentication_info(auth, password);
        }
        let mut registrar = self.registrar.lock().expect("registrar lock");
        registrar.register(req);
        let mut response = registrar.ok_response(req);
        if let Some(route) = req.headers.route().first() {
            response.headers.append(H_SERVICE_ROUTE, *route);
        }
        for path in req.headers.get_all(H_PATH) {
            response.headers.append(H_PATH, path);
        }
        if let Some(public_identity) = req.headers.to() {
            response.headers.append(H_P_ASSOCIATED_URI, public_identity);
        }
        if let Some(authentication_info) = authentication_info {
            response
                .headers
                .append(H_AUTHENTICATION_INFO, authentication_info);
        }
        response
    }

    fn handle_message(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        let content_type = req.headers.content_type().unwrap_or("").to_string();
        let fx2 = self
            .fap_id_for_peer(peer)
            .and_then(|feid| self.fap_registry.get(&feid))
            .map(|fap| fap.implementation.fx2());
        if content_type.is_empty() && req.body.is_empty() {
            if self
                .fap_id_for_peer(peer)
                .and_then(|feid| self.fap_registry.get(&feid))
                .is_some_and(|fap| {
                    fap.implementation.fx2().sms_ack_mode() != SmsAckMode::CorrelatedEmptyMessage
                })
            {
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
            return self.handle_empty_message(req, peer);
        }
        let base_content_type = content_type.split(';').next().unwrap_or("").trim();
        if self
            .fap_id_for_peer(peer)
            .and_then(|feid| self.fap_registry.get(&feid))
            .is_some_and(|fap| {
                base_content_type
                    .to_ascii_lowercase()
                    .contains("femtointerfacemsg")
                    && !fap
                        .implementation
                        .fx2()
                        .accepts_femto_content_type(base_content_type)
            })
        {
            log::warn!("FCS: rejected Fx2 content type {base_content_type} for FAP at {peer}");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        }
        match self.fcs.decode_message(&content_type, &req.body) {
            Ok(FcsMessage::A1(message)) => {
                let ack_transaction = if message.message_type
                    == cdma_ios::femto_interface_msg::MSG_TYPE_ADDS_DELIVER_ACK
                    && message.discriminator
                        == Some(cdma_ios::femto_interface_msg::Discriminator::Dtap)
                {
                    let Some(key) = AddsAckTransaction::from_request(req, peer) else {
                        return response(req, SC_BAD_REQUEST, "Bad Request");
                    };
                    if let Some(cached) = self
                        .adds_ack_responses
                        .lock()
                        .expect("ADDS ACK response lock")
                        .response(&key, Instant::now())
                    {
                        return cached;
                    }
                    Some(key)
                } else {
                    None
                };
                if fx2.is_some() && request_femto_message_with_fx2(req, fx2).is_none() {
                    log::warn!("FCS: rejected profile-incompatible Fx2 body from {peer}");
                    return response(req, SC_BAD_REQUEST, "Bad Request");
                }
                let summary = format!(
                    "A1 message-type 0x{:02X}, {} IE(s)",
                    message.message_type,
                    message.ies.len()
                );
                let call_id = match self.call_id_for_message(req, &message, Some(peer)) {
                    Ok(call_id) => call_id,
                    Err(error) => {
                        log::warn!("FCS: could not correlate SIP MESSAGE to MSC A1: {error}");
                        return response(req, SC_BAD_REQUEST, "Bad Request");
                    }
                };
                let fap_id = self.fap_id_for_peer(peer);
                let (fcs, cell_identifier) = self.fcs_for_fap(fap_id.as_deref());
                self.publish_a1(
                    fap_id.as_deref(),
                    &message,
                    self.a1_context(
                        cdma_events::proto::A1Direction::Uplink,
                        fap_id.as_deref(),
                        call_id,
                        Some(req),
                        Some(peer),
                    ),
                );
                let a1 = match fcs
                    .to_msc_a1(&message, cell_identifier, call_id)
                    .and_then(|a1| self.otasp_uplink(a1, fap_id.as_deref(), cell_identifier))
                {
                    Ok(message) => message,
                    Err(error) => {
                        log::warn!("FCS: could not map SIP MESSAGE to MSC A1: {error}");
                        return response(req, SC_NOT_IMPLEMENTED, "Not Implemented");
                    }
                };
                if !self.forward_to_msc(
                    "forwarding SIP MESSAGE to MSC",
                    fap_id.as_deref(),
                    a1,
                    UplinkObservation::AlreadyPublished,
                    crate::event::A1Context::default(),
                ) {
                    return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
                }
                let reply = response(req, SC_OK, "OK");
                if let Some(key) = ack_transaction {
                    if let (Some(fap_id), Some(tag)) = (
                        fap_id.as_ref(),
                        message.to_adds_deliver_ack().ok().and_then(|ack| ack.tag),
                    ) {
                        self.adds_delivery_calls
                            .lock()
                            .expect("ADDS delivery calls lock")
                            .remove(&(fap_id.clone(), tag.0));
                    }
                    self.adds_ack_responses
                        .lock()
                        .expect("ADDS ACK response lock")
                        .remember(key, reply.clone(), Instant::now());
                }
                log::info!("FCS: forwarded SIP MESSAGE to MSC ({summary})");
                reply
            }
            Ok(FcsMessage::Sms(bytes)) => self.handle_mobile_originated_sms(req, peer, bytes),
            Err(err) => {
                log::warn!("FCS: could not translate SIP MESSAGE: {err}");
                response(req, SC_BAD_REQUEST, "Bad Request")
            }
        }
    }

    fn handle_mobile_originated_sms(
        &self,
        req: &SipRequest,
        peer: SocketAddr,
        bytes: Vec<u8>,
    ) -> SipResponse {
        let Some(alias) = req
            .headers
            .from()
            .and_then(sip_uri_from_name_addr)
            .and_then(|uri| sip_uri_user(&uri).map(str::to_string))
        else {
            log::warn!("FCS: MO SMS has no usable originating mobile identity");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let mobile = self
            .registered_mobiles
            .lock()
            .expect("registered mobiles lock")
            .get(&alias)
            .cloned();
        let mobile = match mobile {
            Some(mobile) => mobile,
            None if alias.bytes().all(|byte| byte.is_ascii_digit())
                && matches!(alias.len(), IMSI_S_DIGITS | FULL_IMSI_DIGITS) =>
            {
                log::info!(
                    "FCS: forwarding MO SMS with SIP originator identity {alias} before CM identity details arrive"
                );
                RegisteredMobile {
                    imsi: MobileIdentity::Imsi(alias.clone()),
                    esn: None,
                    meid: None,
                    fap_id: String::new(),
                }
            }
            None => {
                log::warn!("FCS: MO SMS originator {alias} is not a usable mobile identity");
                return response(req, SC_FORBIDDEN, "Forbidden");
            }
        };
        if let Err(error) = self.remember_mobile(req, Some(peer), mobile.clone()) {
            log::warn!("FCS: could not refresh MO SMS mobile binding: {error}");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        }
        let reply_seq = cdma_common::sms::decode_mo_sms(&bytes).and_then(|sms| sms.reply_seq);
        let transfer = AddsTransferMessage {
            mobile_identity_imsi: mobile.imsi,
            adds_user_part: AddsUserPart {
                burst_type: SMS_DATA_BURST_TYPE,
                data: bytes,
            },
            mobile_identity_esn: mobile.esn,
            mobile_identity_meid: mobile.meid.map(crate::cell::canonical_meid_identity),
            cell_identifier: Some(self.cell_identifier),
            tag: None,
            service_option: None,
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            authentication_event: None,
            authentication_data: None,
        };
        let payload = match transfer.encode() {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!("FCS: could not encode MO SMS as ADDS Transfer: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let encoded =
            EncodedA1Message::from_message(&Message::new(MessageType::AddsTransfer, payload));
        // The FAP sent a bare C.S0015 body, not an Fx2 one.
        let femto = cdma_ios::femto_interface_msg::FemtoInterfaceMsg::from_adds_transfer(&transfer)
            .inspect_err(|error| {
                log::debug!("FCS: MO SMS ADDS Transfer has no Fx2 view to log: {error}");
            })
            .ok();
        let fap_id = self.fap_id_for_peer(peer);
        if !self.forward_to_msc(
            "forwarding MO SMS as A1 ADDS Transfer",
            fap_id.as_deref(),
            encoded,
            femto
                .as_ref()
                .map_or(UplinkObservation::A1Wire, UplinkObservation::Fx2),
            self.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                fap_id.as_deref(),
                None,
                Some(req),
                Some(peer),
            ),
        ) {
            return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
        }
        if let Some(reply_seq) = reply_seq {
            let cause_code = PendingSmsCauseCode {
                mobile_identity: alias.clone(),
                body: cdma_common::sms::encode_sms_cause_code(reply_seq, 0),
            };
            if self.sms_cause_code_tx.send(cause_code).is_err() {
                log::warn!("FCS: SMS Cause Code downlink queue is unavailable");
                return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
            }
            log::info!("FCS: queued successful SMS Cause Code for {alias} reply_seq={reply_seq}");
        } else {
            log::debug!("FCS: MO SMS from {alias} did not request an SMS Cause Code");
        }
        log::info!(
            "FCS: forwarded MO SMS from {alias} to MSC as ADDS Transfer ({} bytes)",
            transfer.adds_user_part.data.len()
        );
        response(req, SC_OK, "OK")
    }

    fn sms_cause_code_request(
        &self,
        pending: PendingSmsCauseCode,
        local: SocketAddr,
    ) -> Result<(SipRequest, SocketAddr, String), crate::fcs::FcsError> {
        let route = self
            .route_for_mobile(&pending.mobile_identity)
            .ok_or(crate::fcs::FcsError::NoRegisteredFap)?;
        let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
        let call_id = format!("1xbts-sms-cause-{transaction}@{}", self.realm);
        let fcs_identity = format!("sip:fcs@{}", self.realm);
        let mut request = SipRequest::new(Method::Message, route.public_identity.clone());
        request.headers.append(
            "Via",
            format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbts{transaction}"),
        );
        request
            .headers
            .append("From", format!("<{fcs_identity}>;tag=1xbts{transaction}"));
        let to = sip_uri_with_user(&route.public_identity, &pending.mobile_identity)
            .unwrap_or_else(|| route.public_identity.clone());
        request.headers.append("To", format!("<{to}>"));
        request.headers.append("Call-ID", &call_id);
        request
            .headers
            .append("CSeq", format!("{} MESSAGE", sip_message_cseq(transaction)));
        request.headers.append("Max-Forwards", "70");
        request
            .headers
            .append("Contact", format!("<{fcs_identity}>"));
        request
            .headers
            .append("Content-Type", "application/vnd.3gpp2.sms");
        request
            .headers
            .append("Content-Transfer-Encoding", "binary");
        request.body = pending.body;
        Ok((request, route.peer, call_id))
    }

    fn handle_empty_message(&self, req: &SipRequest, peer: SocketAddr) -> SipResponse {
        let Some(mobile_identity) = req
            .headers
            .from()
            .and_then(sip_uri_from_name_addr)
            .and_then(|uri| sip_uri_user(&uri).map(str::to_string))
        else {
            log::warn!("FCS: empty SIP MESSAGE has no usable mobile identity");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        let Some(ack) = self
            .pending_adds_page_acks
            .lock()
            .expect("pending ADDS Page Acks lock")
            .remove(&mobile_identity)
        else {
            log::warn!("FCS: empty SIP MESSAGE from {mobile_identity} has no pending ADDS Page");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        };
        if let Err(error) = self.remember_mobile(
            req,
            Some(peer),
            RegisteredMobile {
                imsi: ack.mobile_identity.clone(),
                esn: None,
                meid: None,
                fap_id: String::new(),
            },
        ) {
            log::warn!("FCS: could not refresh SMS acknowledgment mobile binding: {error}");
            return response(req, SC_BAD_REQUEST, "Bad Request");
        }
        let payload = match ack.encode() {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!("FCS: could not encode correlated ADDS Page Ack: {error}");
                return response(req, SC_BAD_REQUEST, "Bad Request");
            }
        };
        let encoded =
            EncodedA1Message::from_message(&Message::new(MessageType::AddsPageAck, payload));
        let femto = cdma_ios::femto_interface_msg::FemtoInterfaceMsg::from_adds_page_ack(&ack)
            .inspect_err(|error| {
                log::debug!("FCS: ADDS Page Ack has no Fx2 view to log: {error}");
            })
            .ok();
        let fap_id = self.fap_id_for_peer(peer);
        if !self.forward_to_msc(
            "forwarding correlated ADDS Page Ack to MSC",
            fap_id.as_deref(),
            encoded,
            femto
                .as_ref()
                .map_or(UplinkObservation::A1Wire, UplinkObservation::Fx2),
            self.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                fap_id.as_deref(),
                None,
                Some(req),
                Some(peer),
            ),
        ) {
            return response(req, SC_SERVICE_UNAVAILABLE, "Service Unavailable");
        }
        log::info!(
            "FCS: correlated Airvana empty SIP MESSAGE from {mobile_identity} to ADDS Page Ack"
        );
        response(req, SC_OK, "OK")
    }

    fn call_id_for(&self, req: &SipRequest, peer: Option<SocketAddr>) -> u64 {
        let sip_call_id = req.headers.call_id().unwrap_or("missing-call-id");
        // Scope the SIP call-id to its source FAP (or peer address if none) so
        // identical call-ids from different FAPs never collide.
        let scope = self
            .source_fap(req, peer)
            .or_else(|| peer.map(|peer| peer.ip().to_string()))
            .unwrap_or_else(|| "unresolved-source".to_string());
        let scoped_call_id = format!("{scope}\0{sip_call_id}");
        let mut sessions = self.access_sessions.lock().expect("access sessions lock");
        *sessions
            .entry(scoped_call_id)
            .or_insert_with(|| self.next_call_id.fetch_add(1, Ordering::Relaxed))
    }

    fn call_id_for_message(
        &self,
        request: &SipRequest,
        message: &cdma_ios::femto_interface_msg::FemtoInterfaceMsg,
        peer: Option<SocketAddr>,
    ) -> Result<Option<u64>, crate::fcs::FcsError> {
        let call_id = self.classify_call_id_for_message(request, message, peer)?;
        // Bind the leg to its source FAP so later identity-less downlinks (the
        // Assignment Request after a Paging Response) route back to it.
        if let (Some(call_id), Some(fap_id)) = (call_id, self.source_fap(request, peer)) {
            self.call_fap
                .lock()
                .expect("call fap lock")
                .insert(call_id, fap_id);
        }
        Ok(call_id)
    }

    fn otasp_uplink(
        &self,
        encoded: EncodedA1Message,
        fap_id: Option<&str>,
        cell: CellId,
    ) -> Result<EncodedA1Message, crate::fcs::FcsError> {
        if encoded.message_type() != MessageType::AddsDeliver {
            return Ok(encoded);
        }
        let deliver = cdma_ios::AddsDeliverMessage::decode(&encoded.decode()?.payload)?;
        if deliver.adds_user_part.burst_type != cdma_common::consts::BURST_TYPE_OTASP {
            return Ok(encoded);
        }
        let identity = encoded
            .call_id()
            .and_then(|call_id| {
                self.call_mobile_identities
                    .lock()
                    .expect("call identities lock")
                    .get(&call_id)
                    .cloned()
            })
            .ok_or_else(|| {
                crate::fcs::FcsError::InvalidMobileIdentity(
                    "OTASP uplink has no active call identity".to_string(),
                )
            })?;
        let mobile = self
            .registered_mobiles
            .lock()
            .expect("registered mobiles lock")
            .get(&identity)
            .filter(|mobile| Some(mobile.fap_id.as_str()) == fap_id)
            .cloned()
            .ok_or_else(|| crate::fcs::FcsError::InvalidMobileIdentity(identity))?;
        let transfer = AddsTransferMessage {
            mobile_identity_imsi: mobile.imsi,
            mobile_identity_esn: mobile.esn,
            mobile_identity_meid: mobile.meid.map(crate::cell::canonical_meid_identity),
            adds_user_part: deliver.adds_user_part,
            cell_identifier: Some(cell),
            tag: deliver.tag,
            service_option: None,
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            authentication_event: None,
            authentication_data: None,
        };
        Ok(EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::AddsTransfer, transfer.encode()?),
            encoded.call_id(),
        ))
    }

    /// Airvana can use a new SIP Call-ID for each standalone traffic-channel MESSAGE.
    /// Resolve only a scoped dialog, a delivery tag, or a unique handset on that FAP.
    fn adds_call_id(
        &self,
        request: &SipRequest,
        message: &cdma_ios::femto_interface_msg::FemtoInterfaceMsg,
        peer: Option<SocketAddr>,
    ) -> Result<Option<u64>, crate::fcs::FcsError> {
        let fap_id = self
            .source_fap(request, peer)
            .ok_or(crate::fcs::FcsError::NoRegisteredFap)?;
        if message.message_type == cdma_ios::femto_interface_msg::MSG_TYPE_ADDS_DELIVER_ACK {
            let ack = message.to_adds_deliver_ack()?;
            if let Some(tag) = ack.tag {
                if let Some(call_id) = self
                    .adds_delivery_calls
                    .lock()
                    .expect("ADDS delivery calls lock")
                    .get(&(fap_id.clone(), tag.0))
                    .copied()
                {
                    return Ok(Some(call_id));
                }
            }
        }
        if message.message_type == cdma_ios::femto_interface_msg::MSG_TYPE_ADDS_DELIVER_ACK {
            return Err(crate::fcs::FcsError::InvalidMobileIdentity(
                "ADDS acknowledgment has no outstanding delivery tag".to_string(),
            ));
        }
        if let Some(sip_call_id) = request.headers.call_id() {
            let scoped = format!("{fap_id}\0{sip_call_id}");
            let existing = self
                .access_sessions
                .lock()
                .expect("access sessions lock")
                .get(&scoped)
                .copied();
            if let Some(call_id) = existing {
                if self.call_fap.lock().expect("call FAP lock").get(&call_id) == Some(&fap_id) {
                    return Ok(Some(call_id));
                }
            }
        }
        let from = request.headers.from().unwrap_or_default();
        let mut aliases = Vec::new();
        if let Some(alias) =
            sip_uri_from_name_addr(from).and_then(|uri| sip_uri_user(&uri).map(str::to_string))
        {
            if alias.bytes().all(|b| b.is_ascii_digit()) && matches!(alias.len(), 10 | 15) {
                aliases.push(alias);
            }
        }
        if let Some((display, _)) = from.split_once('<') {
            let display = display.trim().trim_matches('"');
            if display.bytes().all(|b| b.is_ascii_digit()) && matches!(display.len(), 10 | 15) {
                aliases.push(display.to_string());
            }
        }
        let mobiles = self
            .registered_mobiles
            .lock()
            .expect("registered mobiles lock");
        let imsis: Vec<_> = aliases
            .iter()
            .filter_map(|alias| mobiles.get(alias))
            .filter(|mobile| mobile.fap_id == fap_id)
            .map(|mobile| mobile.imsi.clone())
            .collect();
        let identities = self
            .call_mobile_identities
            .lock()
            .expect("call identities lock");
        let calls = self.call_fap.lock().expect("call FAP lock");
        let matches: Vec<_> = identities
            .iter()
            .filter_map(|(call_id, identity)| {
                let mobile = mobiles.get(identity)?;
                (calls.get(call_id) == Some(&fap_id) && imsis.contains(&mobile.imsi))
                    .then_some(*call_id)
            })
            .collect();
        if matches.len() == 1 {
            return Ok(Some(matches[0]));
        }
        Err(crate::fcs::FcsError::InvalidMobileIdentity(
            "traffic-channel ADDS message has no unique active handset".to_string(),
        ))
    }

    fn classify_call_id_for_message(
        &self,
        request: &SipRequest,
        message: &cdma_ios::femto_interface_msg::FemtoInterfaceMsg,
        peer: Option<SocketAddr>,
    ) -> Result<Option<u64>, crate::fcs::FcsError> {
        match message.message_type {
            cdma_ios::femto_interface_msg::MSG_TYPE_ADDS_DELIVER
            | cdma_ios::femto_interface_msg::MSG_TYPE_ADDS_DELIVER_ACK
                if message.discriminator
                    == Some(cdma_ios::femto_interface_msg::Discriminator::Dtap) =>
            {
                self.adds_call_id(request, message, peer)
            }
            cdma_ios::femto_interface_msg::MSG_TYPE_LOCATION_UPDATING_REQUEST => {
                let location_update = message.to_location_updating_request()?;
                let identity = match &location_update.mobile_identity_imsi {
                    MobileIdentity::Imsi(identity) => identity.clone(),
                    _ => return Ok(None),
                };
                let call_id = self.call_id_for(request, peer);
                let response_uri = request
                    .headers
                    .from()
                    .and_then(sip_uri_from_name_addr)
                    .filter(|uri| sip_uri_user(uri).is_some_and(|user| user == identity));
                self.remember_mobile(
                    request,
                    peer,
                    RegisteredMobile {
                        imsi: location_update.mobile_identity_imsi,
                        esn: location_update.mobile_identity_esn,
                        meid: location_update.mobile_identity_meid,
                        fap_id: String::new(),
                    },
                )?;
                self.registration_sessions
                    .lock()
                    .expect("registration sessions lock")
                    .insert(
                        call_id,
                        RegistrationSession {
                            mobile_identity: identity,
                            response_uri,
                        },
                    );
                Ok(Some(call_id))
            }
            cdma_ios::femto_interface_msg::MSG_TYPE_ADDS_PAGE_ACK => {
                let ack = AddsPageAckMessage::decode(&message.to_bsmap_wire()?)?;
                self.remember_mobile(
                    request,
                    peer,
                    RegisteredMobile {
                        imsi: ack.mobile_identity,
                        esn: ack.mobile_identity_esn,
                        meid: None,
                        fap_id: String::new(),
                    },
                )?;
                Ok(None)
            }
            cdma_ios::femto_interface_msg::MSG_TYPE_CM_SERVICE_REQUEST => {
                let service_request = message.to_cm_service_request()?;
                let mobile_identity = match &service_request.mobile_identity_imsi {
                    MobileIdentity::Imsi(identity) => Some(identity.clone()),
                    _ => None,
                };
                self.remember_mobile(
                    request,
                    peer,
                    RegisteredMobile {
                        imsi: service_request.mobile_identity_imsi,
                        esn: service_request.mobile_identity_esn,
                        meid: service_request.mobile_identity_meid,
                        fap_id: String::new(),
                    },
                )?;
                let call_id = self.call_id_for(request, peer);
                if let Some(mobile_identity) = mobile_identity {
                    self.call_mobile_identities
                        .lock()
                        .expect("call mobile identities lock")
                        .insert(call_id, mobile_identity);
                }
                Ok(Some(call_id))
            }
            cdma_ios::femto_interface_msg::MSG_TYPE_PAGING_RESPONSE => {
                let paging = message.to_paging_response()?;
                let identity = match &paging.mobile_identity_imsi {
                    cdma_ios::MobileIdentity::Imsi(identity) => identity.clone(),
                    _ => return Ok(None),
                };
                self.remember_mobile(
                    request,
                    peer,
                    RegisteredMobile {
                        imsi: paging.mobile_identity_imsi,
                        esn: paging.mobile_identity_esn,
                        meid: paging.mobile_identity_meid,
                        fap_id: String::new(),
                    },
                )?;
                Ok(self
                    .paging_sessions
                    .lock()
                    .expect("paging sessions lock")
                    .remove(&identity))
            }
            _ => Ok(Some(self.call_id_for(request, peer))),
        }
    }

    fn remember_mobile(
        &self,
        request: &SipRequest,
        peer: Option<SocketAddr>,
        mut mobile: RegisteredMobile,
    ) -> Result<(), crate::fcs::FcsError> {
        let MobileIdentity::Imsi(imsi) = &mut mobile.imsi else {
            return Ok(());
        };
        // Skip the binding if the source maps to no configured FAP, rather than
        // pinning the mobile to an invented FAP that would misroute it.
        let Some(fap_id) = self.source_fap(request, peer) else {
            log::warn!("FCS: not binding mobile {imsi:?}: source does not map to a configured FAP");
            return Ok(());
        };
        mobile.fap_id = fap_id;
        let (fcs, _) = self.fcs_for_fap(Some(&mobile.fap_id));
        let canonical_imsi = fcs.canonical_imsi(imsi)?;
        let femto_identity = fcs.femto_identity(&canonical_imsi)?;
        *imsi = canonical_imsi.clone();
        let mut aliases = vec![canonical_imsi.clone(), femto_identity];
        if let Some(implementation) = self.fap_registry.get(&mobile.fap_id) {
            for candidate in sip_mobile_identity_candidates(request) {
                if implementation
                    .implementation
                    .fx2()
                    .accepts_mobile_identity(&candidate)
                {
                    aliases.push(candidate.value().to_string());
                }
            }
        }
        let mut registered = self
            .registered_mobiles
            .lock()
            .expect("registered mobiles lock");
        if mobile.esn.is_none()
            && mobile.meid.is_none()
            && let Some(known) = registered
                .get(&canonical_imsi)
                .filter(|known| known.fap_id == mobile.fap_id)
        {
            // IMSI-only acknowledgments must preserve the hardware identity used by HLR.
            mobile.esn = known.esn.clone();
            mobile.meid = known.meid.clone();
        }
        for alias in aliases {
            registered.insert(alias, mobile.clone());
        }
        drop(registered);
        self.cell_state.observe_mobile(
            &mobile.fap_id,
            &canonical_imsi,
            mobile.esn.as_ref(),
            mobile.meid.as_ref(),
            "Registered",
        );
        log::info!("FCS: learned registered mobile identity {canonical_imsi}");
        Ok(())
    }

    fn fap_id_for_request(&self, request: &SipRequest) -> Option<String> {
        let headers = if request.method == Method::Register {
            [request.headers.to(), request.headers.from()]
        } else {
            [request.headers.from(), request.headers.to()]
        };
        let identities = headers
            .into_iter()
            .flatten()
            .filter_map(sip_uri_from_name_addr)
            .collect::<Vec<_>>();
        let users = self.users.read().expect("sip users lock");
        identities
            .iter()
            .find_map(|identity| {
                users.iter().find(|user| {
                    identity.eq_ignore_ascii_case(&user.impu)
                        || sip_uri_user(identity)
                            .is_some_and(|value| value.eq_ignore_ascii_case(&user.impi))
                })
            })
            .map(|user| user.feid.clone())
    }

    fn fap_id_for_peer(&self, peer: SocketAddr) -> Option<String> {
        self.registered_faps
            .lock()
            .expect("registered FAPs lock")
            .iter()
            .find_map(|(feid, route)| (route.peer.ip() == peer.ip()).then(|| feid.clone()))
            .or_else(|| self.cell_state.feid_for_inner_peer(peer.ip()))
    }

    fn scoped_sip_call_id(&self, peer: SocketAddr, call_id: &str) -> String {
        let fap_id = self
            .fap_id_for_peer(peer)
            .unwrap_or_else(|| peer.ip().to_string());
        format!("{fap_id}\0{call_id}")
    }

    fn fcs_for_fap(&self, fap_id: Option<&str>) -> (&FemtoConvergenceServer, CellId) {
        fap_id
            .and_then(|fap_id| self.fcs_by_fap.get(fap_id))
            .map(|(fcs, cell)| (fcs, *cell))
            .unwrap_or((&self.fcs, self.cell_identifier))
    }

    fn media_packet_format(&self, fap_id: Option<&str>) -> MediaPacketFormat {
        fap_id
            .and_then(|fap_id| self.fap_registry.get(fap_id))
            .map(|fap| fap.implementation.media().packet_format())
            .unwrap_or(MediaPacketFormat::StandardRtp)
    }

    fn route_for_mobile(&self, identity: &str) -> Option<RegisteredFap> {
        let fap_id = self
            .registered_mobiles
            .lock()
            .expect("registered mobiles lock")
            .get(identity)
            .map(|mobile| mobile.fap_id.clone())?;
        self.registered_faps
            .lock()
            .expect("registered FAPs lock")
            .get(&fap_id)
            .cloned()
    }

    fn route_for_call(&self, call_id: u64) -> Option<RegisteredFap> {
        let fap_id = self
            .call_fap
            .lock()
            .expect("call fap lock")
            .get(&call_id)
            .cloned()?;
        self.registered_faps
            .lock()
            .expect("registered FAPs lock")
            .get(&fap_id)
            .cloned()
    }

    /// The configured FAP a message came from, by transport peer then SIP
    /// identity. `None` means no configured FAP. Callers must not invent one.
    fn source_fap(&self, request: &SipRequest, peer: Option<SocketAddr>) -> Option<String> {
        peer.and_then(|peer| self.fap_id_for_peer(peer))
            .or_else(|| self.fap_id_for_request(request))
    }

    fn remember_registered_fap(&self, req: &SipRequest, peer: SocketAddr) {
        let Some(fap_id) = self.fap_id_for_request(req) else {
            log::warn!("SIP REGISTER does not match a configured FAP identity");
            return;
        };
        if req.headers.expires() == Some(0) {
            self.registered_faps
                .lock()
                .expect("registered FAPs lock")
                .remove(&fap_id);
            self.cell_state.deregister_fap(&fap_id);
            return;
        }
        let Some(public_identity) = req.headers.to().and_then(sip_uri_from_name_addr) else {
            log::warn!("SIP REGISTER has no usable public identity for Fx2 downlink");
            return;
        };
        let route = RegisteredFap {
            fap_id: fap_id.clone(),
            public_identity,
            peer,
        };
        let mut registered = self.registered_faps.lock().expect("registered FAPs lock");
        if registered.get(&fap_id) != Some(&route) {
            log::info!(
                "SIP registered Fx2 downlink route {} via {}",
                route.public_identity,
                route.peer
            );
        }
        self.cell_state
            .register_fap(&fap_id, route.public_identity.clone(), peer);
        registered.insert(fap_id, route);
    }

    fn downlink_request(
        &self,
        encoded: &EncodedA1Message,
        local: SocketAddr,
    ) -> Result<(SipRequest, SocketAddr, String, PendingDownlink), crate::fcs::FcsError> {
        let identity = self.downlink_mobile_identity(encoded)?;
        // Route by the call binding first, then by mobile identity for a
        // connectionless page. Do not fall back to the only registered FAP. It
        // misroutes once more than one FAP is registered.
        let route = encoded
            .call_id()
            .and_then(|call_id| self.route_for_call(call_id))
            .or_else(|| {
                identity
                    .as_deref()
                    .and_then(|identity| self.route_for_mobile(identity))
            })
            .ok_or(crate::fcs::FcsError::NoRegisteredFap)?;
        // Pin the leg so later identity-less downlinks route to the same FAP.
        // or_insert keeps an authoritative uplink binding if one exists.
        if let Some(call_id) = encoded.call_id() {
            self.call_fap
                .lock()
                .expect("call fap lock")
                .entry(call_id)
                .or_insert_with(|| route.fap_id.clone());
        }
        if encoded.message_type() == MessageType::AddsDeliver {
            let deliver = cdma_ios::AddsDeliverMessage::decode(&encoded.decode()?.payload)?;
            if let (Some(tag), Some(call_id)) = (deliver.tag, encoded.call_id()) {
                self.adds_delivery_calls
                    .lock()
                    .expect("ADDS delivery calls lock")
                    .insert((route.fap_id.clone(), tag.0), call_id);
            }
        }
        let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
        let multipart_boundary = format!("1xbts-fx2-{transaction}");
        let femto_content_type = self
            .fap_registry
            .get(&route.fap_id)
            .map(|fap| fap.implementation.fx2().preferred_femto_content_type())
            .unwrap_or(crate::fap::STANDARD_FEMTO_CONTENT_TYPE);
        let fap_id = Some(route.fap_id.as_str());
        let (fcs, _) = self.fcs_for_fap(fap_id);
        let mut downlink = fcs.downlink_from_msc_with_content_type(
            encoded,
            &multipart_boundary,
            femto_content_type,
        )?;
        // Not published here — `next_downlink` publishes every downlink before
        // routing, including the ones that never reach this function. A publish
        // here would be a duplicate row.
        let adds_page_ack = if encoded.message_type() == MessageType::AddsPage {
            let message = encoded.decode()?;
            let adds_page = AddsPageMessage::decode(&message.payload)?;
            downlink.mobile_identity.clone().map(|mobile_identity| {
                (
                    mobile_identity,
                    AddsPageAckMessage {
                        mobile_identity: adds_page.mobile_identity,
                        tag: adds_page.tag,
                        mobile_identity_esn: None,
                        cause: None,
                    },
                )
            })
        } else {
            None
        };
        let mut registration_response_uri = None;
        if encoded.message_type() == cdma_ios::MessageType::LocationUpdatingAccept {
            let registration = encoded.call_id().and_then(|call_id| {
                self.registration_sessions
                    .lock()
                    .expect("registration sessions lock")
                    .remove(&call_id)
            });
            downlink.mobile_identity = registration
                .as_ref()
                .map(|session| session.mobile_identity.clone());
            registration_response_uri = registration.and_then(|session| session.response_uri);
            if downlink.mobile_identity.is_none() {
                log::warn!(
                    "FCS: Location Updating Accept has no originating mobile identity correlation"
                );
            }
        }
        if let (Some(identity), Some(call_id)) =
            (downlink.mobile_identity.as_ref(), encoded.call_id())
        {
            self.paging_sessions
                .lock()
                .expect("paging sessions lock")
                .insert(identity.clone(), call_id);
        }
        if downlink.mobile_identity.is_none() {
            downlink.mobile_identity = encoded.call_id().and_then(|call_id| {
                self.call_mobile_identities
                    .lock()
                    .expect("call mobile identities lock")
                    .get(&call_id)
                    .cloned()
            });
        }
        let call_id = format!("1xbts-fcs-{transaction}@{}", self.realm);
        let fcs_identity = format!("sip:fcs@{}", self.realm);
        let to_uri = registration_response_uri.or_else(|| {
            downlink
                .mobile_identity
                .as_ref()
                .and_then(|identity| sip_uri_with_user(&route.public_identity, identity))
        });
        let request_uri = if encoded.message_type() == MessageType::AddsPage {
            to_uri
                .clone()
                .unwrap_or_else(|| route.public_identity.clone())
        } else {
            route.public_identity.clone()
        };
        let mut request = SipRequest::new(Method::Message, request_uri);
        request.headers.append(
            "Via",
            format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbts{transaction}"),
        );
        request
            .headers
            .append("From", format!("<{fcs_identity}>;tag=1xbts{transaction}"));
        let to = to_uri
            .map(|uri| format!("<{uri}>"))
            .unwrap_or_else(|| format!("<{}>", route.public_identity));
        request.headers.append("To", to);
        request.headers.append("Call-ID", &call_id);
        request
            .headers
            .append("CSeq", format!("{} MESSAGE", sip_message_cseq(transaction)));
        request.headers.append("Max-Forwards", "70");
        request
            .headers
            .append("Contact", format!("<{fcs_identity}>"));
        request
            .headers
            .append("Content-Type", downlink.content_type);
        request.body = downlink.body;
        let pending = PendingDownlink {
            message_type: encoded.message_type(),
            a1_call_id: encoded.call_id(),
            adds_page_ack,
        };
        Ok((request, route.peer, call_id, pending))
    }

    fn downlink_mobile_identity(
        &self,
        encoded: &EncodedA1Message,
    ) -> Result<Option<String>, crate::fcs::FcsError> {
        let identity = match encoded.message_type() {
            MessageType::AddsPage => {
                let message = encoded.decode()?;
                let page = AddsPageMessage::decode(&message.payload)?;
                match page.mobile_identity {
                    MobileIdentity::Imsi(identity) => Some(identity),
                    _ => None,
                }
            }
            MessageType::PagingRequest => {
                let message = encoded.decode()?;
                let page = cdma_ios::PagingRequestMessage::decode(&message.payload)?;
                match page.mobile_identity_imsi {
                    MobileIdentity::Imsi(identity) => Some(identity),
                    _ => None,
                }
            }
            MessageType::LocationUpdatingAccept => encoded.call_id().and_then(|call_id| {
                self.registration_sessions
                    .lock()
                    .ok()?
                    .get(&call_id)
                    .map(|session| session.mobile_identity.clone())
            }),
            _ => encoded.call_id().and_then(|call_id| {
                self.call_mobile_identities
                    .lock()
                    .ok()?
                    .get(&call_id)
                    .cloned()
            }),
        };
        Ok(identity)
    }

    fn mt_invite_request(
        &self,
        encoded: &EncodedA1Message,
        local: SocketAddr,
    ) -> Result<(SipRequest, SocketAddr, String), SipVoiceError> {
        let a1_call_id = encoded.call_id().ok_or(SipVoiceError::MissingCallId {
            message: "MT Paging Request",
        })?;
        let message = encoded.decode()?;
        let paging = cdma_ios::PagingRequestMessage::decode(&message.payload)?;
        let mobile_identity = match &paging.mobile_identity_imsi {
            MobileIdentity::Imsi(identity) => identity.clone(),
            _ => return Err(SipVoiceError::PagingRequiresImsi),
        };
        let route = self
            .route_for_mobile(&mobile_identity)
            .ok_or(SipVoiceError::NoRegisteredFap)?;
        let service_option = paging
            .service_option
            .map(|service_option| service_option.0)
            .unwrap_or(cdma_ios::ServiceOption::QCELP_13K.0);
        let fap_id = route.fap_id.clone();
        let relay = self
            .rtp_relays
            .open_with_format(service_option, self.media_packet_format(Some(&fap_id)))?;
        let relay_address = match relay.fap_local().ip() {
            std::net::IpAddr::V4(address) if !address.is_unspecified() => address,
            _ => match local.ip() {
                std::net::IpAddr::V4(address) => address,
                _ => return Err(SipVoiceError::NoIpv4SdpAddress),
            },
        };
        let originating_sdp = self
            .mo_voice_by_call
            .lock()
            .expect("MO voice A1 dialog lock")
            .get(&a1_call_id)
            .and_then(|dialog| request_sdp_body(&dialog.invite));
        let sdp = match originating_sdp {
            Some(originating_sdp) => {
                rewrite_sdp_transport(&originating_sdp, relay_address, relay.fap_local().port())?
            }
            None => build_sdp_offer(relay_address, relay.fap_local().port(), service_option),
        };
        let offered_speech_payload_types = sdp_speech_payload_types(&sdp);
        if offered_speech_payload_types.is_empty() {
            return Err(SipVoiceError::MissingSpeechPayload);
        }
        log::info!(
            "FCS: MT voice call_id={a1_call_id} outbound SDP:\n{}",
            String::from_utf8_lossy(&sdp)
        );
        let (fcs, _) = self.fcs_for_fap(Some(&fap_id));
        let femto = fcs.from_msc_a1(encoded)?;
        let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
        let boundary = format!("1xbts-fx2-voice-{transaction}");
        let multipart = Multipart::new(
            &boundary,
            vec![
                MimePart::text(
                    self.fap_registry
                        .get(&fap_id)
                        .map(|fap| fap.implementation.fx2().preferred_femto_content_type())
                        .unwrap_or(CONTENT_TYPE_FEMTO),
                    femto.encode(),
                ),
                MimePart::text(CONTENT_TYPE_SDP, sdp),
            ],
        );
        let sip_call_id = format!("1xbts-fcs-voice-{transaction}@{}", self.realm);
        let invite_branch = format!("z9hG4bK1xbtsvoice{transaction}");
        let from = format!("<sip:fcs@{}>;tag=1xbts{transaction}", self.realm);
        let femto_identity = fcs.femto_identity(&mobile_identity)?;
        let called_uri = sip_uri_with_user(&route.public_identity, &femto_identity)
            .ok_or(SipVoiceError::MissingPublicIdentity)?;
        let to = format!("<{called_uri}>");
        let mut request = SipRequest::new(Method::Invite, route.public_identity.clone());
        request
            .headers
            .append("Via", format!("SIP/2.0/UDP {local};branch={invite_branch}"));
        request.headers.append("From", &from);
        request.headers.append("To", &to);
        request.headers.append("Call-ID", &sip_call_id);
        request
            .headers
            .append("CSeq", format!("{SIP_INVITE_CSEQ} INVITE"));
        request.headers.append("Max-Forwards", "70");
        request
            .headers
            .append("Contact", format!("<sip:fcs@{local}>"));
        request.headers.append("Supported", "100rel");
        request.headers.append(
            "Allow",
            "INVITE,ACK,CANCEL,BYE,PRACK,UPDATE,INFO,OPTIONS,MESSAGE",
        );
        request
            .headers
            .append(H_P_ASSERTED_IDENTITY, format!("<sip:fcs@{}>", self.realm));
        request.headers.append(
            "Content-Type",
            format!("multipart/mixed; boundary={boundary}"),
        );
        request.body = multipart.encode();
        let dialog = MtVoiceDialog {
            fap_id,
            sip_call_id: sip_call_id.clone(),
            invite_branch,
            request_uri: route.public_identity,
            from,
            to,
            remote_to: None,
            peer: route.peer,
            rseq: None,
            last_pracked_rseq: None,
            next_prack_cseq: SIP_FIRST_PRACK_CSEQ,
            relay,
            offered_speech_payload_types,
            service_option,
            answered: false,
            created_at: Instant::now(),
            cancel_sent: false,
        };
        self.mt_voice_by_sip
            .lock()
            .expect("MT voice SIP dialog lock")
            .insert(sip_call_id.clone(), a1_call_id);
        self.mt_voice_by_call
            .lock()
            .expect("MT voice A1 dialog lock")
            .insert(a1_call_id, dialog);
        Ok((request, route.peer, sip_call_id))
    }

    fn expired_mt_cancels(&self, local: SocketAddr) -> Vec<(SipRequest, SocketAddr)> {
        let now = Instant::now();
        let mut dialogs = self
            .mt_voice_by_call
            .lock()
            .expect("MT voice A1 dialog lock");
        dialogs
            .iter_mut()
            .filter_map(|(call_id, dialog)| {
                if dialog.answered
                    || dialog.cancel_sent
                    || now.duration_since(dialog.created_at) < SIP_INVITE_TIMEOUT
                {
                    return None;
                }
                dialog.cancel_sent = true;
                let mut cancel = SipRequest::new(Method::Cancel, dialog.request_uri.clone());
                cancel.headers.append(
                    "Via",
                    format!("SIP/2.0/UDP {local};branch={}", dialog.invite_branch),
                );
                cancel.headers.append("From", &dialog.from);
                cancel.headers.append("To", &dialog.to);
                cancel.headers.append("Call-ID", &dialog.sip_call_id);
                cancel
                    .headers
                    .append("CSeq", format!("{SIP_INVITE_CSEQ} CANCEL"));
                cancel.headers.append("Max-Forwards", "70");
                log::warn!("FCS: cancelling unanswered MT voice call_id={call_id}");
                Some((cancel, dialog.peer))
            })
            .collect()
    }

    /// Clear a voice call per dialog state (RFC 3261 / X.S0059 Fx2). A confirmed
    /// dialog ends with a BYE. An early one must not: the caller's INVITE gets a
    /// final failure response and a ringing callee gets a CANCEL. A BYE on an
    /// early dialog leaves the phone stuck.
    fn clear_dialog_requests(
        &self,
        call_id: u64,
        local: SocketAddr,
    ) -> Vec<(cdma_sip::SipMessage, SocketAddr)> {
        let mut out: Vec<(cdma_sip::SipMessage, SocketAddr)> = Vec::new();
        if let Some(dialog) = self
            .mt_voice_by_call
            .lock()
            .expect("MT voice A1 dialog lock")
            .remove(&call_id)
        {
            self.mt_voice_by_sip
                .lock()
                .expect("MT voice SIP dialog lock")
                .remove(&dialog.sip_call_id);
            if dialog.answered {
                let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
                let mut bye = SipRequest::new(Method::Bye, dialog.request_uri);
                bye.headers.append(
                    "Via",
                    format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbtsbye{transaction}"),
                );
                bye.headers.append("From", &dialog.from);
                bye.headers
                    .append("To", dialog.remote_to.as_deref().unwrap_or(&dialog.to));
                bye.headers.append("Call-ID", &dialog.sip_call_id);
                bye.headers.append("CSeq", "4 BYE");
                bye.headers.append("Max-Forwards", "70");
                out.push((cdma_sip::SipMessage::Request(bye), dialog.peer));
            } else if !dialog.cancel_sent {
                let mut cancel = SipRequest::new(Method::Cancel, dialog.request_uri);
                cancel.headers.append(
                    "Via",
                    format!("SIP/2.0/UDP {local};branch={}", dialog.invite_branch),
                );
                cancel.headers.append("From", &dialog.from);
                cancel.headers.append("To", &dialog.to);
                cancel.headers.append("Call-ID", &dialog.sip_call_id);
                cancel
                    .headers
                    .append("CSeq", format!("{SIP_INVITE_CSEQ} CANCEL"));
                cancel.headers.append("Max-Forwards", "70");
                out.push((cdma_sip::SipMessage::Request(cancel), dialog.peer));
            }
        }
        if let Some(dialog) = self
            .mo_voice_by_call
            .lock()
            .expect("MO voice A1 dialog lock")
            .remove(&call_id)
        {
            let sip_call_id = dialog
                .invite
                .headers
                .call_id()
                .unwrap_or_default()
                .to_string();
            self.mo_voice_by_sip
                .lock()
                .expect("MO voice SIP dialog lock")
                .remove(&format!("{}\0{sip_call_id}", dialog.fap_id));
            if dialog.answered {
                let request_uri = dialog
                    .invite
                    .headers
                    .contact()
                    .and_then(sip_uri_from_name_addr)
                    .unwrap_or_else(|| format!("sip:fap@{}", dialog.peer));
                let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
                let mut bye = SipRequest::new(Method::Bye, request_uri);
                bye.headers.append(
                    "Via",
                    format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbtsbye{transaction}"),
                );
                let answered = response(&dialog.invite, SC_OK, "OK");
                if let Some(from) = answered.headers.to() {
                    bye.headers.append("From", from);
                }
                if let Some(to) = dialog.invite.headers.from() {
                    bye.headers.append("To", to);
                }
                bye.headers.append("Call-ID", &sip_call_id);
                bye.headers.append("CSeq", "3 BYE");
                bye.headers.append("Max-Forwards", "70");
                out.push((cdma_sip::SipMessage::Request(bye), dialog.peer));
            } else {
                // Early UAS dialog (never answered): a final failure response,
                // not a BYE. response() reuses the 180's To-tag so the caller
                // matches it and releases.
                let failure = response(&dialog.invite, 480, "Temporarily Unavailable");
                out.push((cdma_sip::SipMessage::Response(failure), dialog.peer));
            }
        }
        out
    }

    fn mt_prack_request(
        &self,
        encoded: &EncodedA1Message,
        local: SocketAddr,
    ) -> Result<(SipRequest, SocketAddr), SipVoiceError> {
        let a1_call_id = encoded.call_id().ok_or(SipVoiceError::MissingCallId {
            message: "Assignment Request",
        })?;
        let message = encoded.decode()?;
        let assignment = AssignmentRequestMessage::decode(&message.payload)?;
        let mut dialogs = self
            .mt_voice_by_call
            .lock()
            .expect("MT voice A1 dialog lock");
        let dialog = dialogs
            .get_mut(&a1_call_id)
            .ok_or(SipVoiceError::MissingMtDialog {
                call_id: a1_call_id,
            })?;
        let rseq = dialog.rseq.ok_or(SipVoiceError::MissingReliableResponse {
            call_id: a1_call_id,
        })?;
        if let Some(parameters) = assignment.a2p_bearer_session_params {
            dialog.relay.set_msc_remote(SocketAddr::new(
                std::net::IpAddr::V4(parameters.ip_address),
                parameters.udp_port,
            ));
        }
        if let Some(service_option) = assignment.service_option {
            dialog.relay.set_service_option(service_option.0);
            dialog.service_option = service_option.0;
        }
        let femto_assignment = AssignmentRequestMessage {
            circuit_identity_code: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
            ..assignment
        };
        let femto = cdma_ios::femto_interface_msg::FemtoInterfaceMsg::from_assignment_request(
            &femto_assignment,
        )?;
        let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
        let mut request = SipRequest::new(Method::Prack, dialog.request_uri.clone());
        request.headers.append(
            "Via",
            format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbtsprack{transaction}"),
        );
        request.headers.append("From", &dialog.from);
        request
            .headers
            .append("To", dialog.remote_to.as_deref().unwrap_or(&dialog.to));
        request.headers.append("Call-ID", &dialog.sip_call_id);
        let prack_cseq = dialog.next_prack_cseq;
        dialog.next_prack_cseq = dialog.next_prack_cseq.saturating_add(1);
        dialog.last_pracked_rseq = Some(rseq);
        request
            .headers
            .append("CSeq", format!("{prack_cseq} PRACK"));
        request
            .headers
            .append(H_RACK, format!("{rseq} {SIP_INVITE_CSEQ} INVITE"));
        request.headers.append("Max-Forwards", "70");
        request
            .headers
            .append("Contact", format!("<sip:fcs@{local}>"));
        request.headers.append(
            "Content-Type",
            self.fap_registry
                .get(&dialog.fap_id)
                .map(|fap| fap.implementation.fx2().preferred_femto_content_type())
                .unwrap_or(CONTENT_TYPE_FEMTO),
        );
        request.body = femto.encode();
        Ok((request, dialog.peer))
    }

    fn release_call_over_a1(&self, call_id: u64, fap_id: Option<&str>) {
        let clear = ClearRequestMessage {
            cause: cdma_ios::Cause(A1_CAUSE_NORMAL_CALL_CLEARING),
            cause_layer3: None,
        };
        let payload = match clear.encode() {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!("FCS: encoding Clear Request for call_id={call_id}: {error}");
                return;
            }
        };
        if self.forward_to_msc(
            "releasing call after terminating-leg failure via A1 Clear Request",
            fap_id,
            EncodedA1Message::from_message_for_call(
                &Message::new(MessageType::ClearRequest, payload),
                Some(call_id),
            ),
            UplinkObservation::A1Wire,
            self.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                fap_id,
                Some(call_id),
                None,
                None,
            ),
        ) {
            log::info!("FCS: released call_id={call_id} over A1 after terminating-leg failure");
        } else {
            log::warn!(
                "FCS: failed to release call_id={call_id} over A1 after terminating-leg failure"
            );
        }
    }

    fn handle_mt_voice_response(
        &self,
        response: &SipResponse,
        local: SocketAddr,
    ) -> Vec<(SipRequest, SocketAddr)> {
        let Some(sip_call_id) = response.headers.call_id() else {
            return Vec::new();
        };
        let a1_call_id = self
            .mt_voice_by_sip
            .lock()
            .expect("MT voice SIP dialog lock")
            .get(sip_call_id)
            .copied();
        let Some(a1_call_id) = a1_call_id else {
            return Vec::new();
        };
        let cseq_method = response.headers.cseq().map(|(_, method)| method);
        let mut dialogs = self
            .mt_voice_by_call
            .lock()
            .expect("MT voice A1 dialog lock");
        let Some(dialog) = dialogs.get_mut(&a1_call_id) else {
            return Vec::new();
        };
        if let Some(to) = response.headers.to() {
            dialog.remote_to = Some(to.to_string());
        }
        if (SC_RINGING..SC_OK).contains(&response.status) {
            let response_rseq = response
                .headers
                .get(H_RSEQ)
                .and_then(|value| value.trim().parse().ok());
            dialog.rseq = response_rseq.or(dialog.rseq).or(Some(1));
            if let Some(endpoint) = response_sdp_endpoint(response) {
                let answer_speech_payload_types = response_sdp_body(response)
                    .map(|body| sdp_speech_payload_types(&body))
                    .unwrap_or_default();
                let negotiated_speech =
                    answer_speech_payload_types
                        .iter()
                        .copied()
                        .find(|payload_type| {
                            dialog.offered_speech_payload_types.contains(payload_type)
                        });
                let Some(negotiated_speech) = negotiated_speech else {
                    log::warn!(
                        "FCS: rejecting MT voice call_id={a1_call_id} SDP answer without an offered speech payload (offered={:?}, answered={answer_speech_payload_types:?})",
                        dialog.offered_speech_payload_types
                    );
                    return Vec::new();
                };
                dialog.relay.set_fap_voice_payload_type(negotiated_speech);
                dialog.relay.set_fap_remote(SocketAddr::new(
                    std::net::IpAddr::V4(endpoint.address),
                    endpoint.port,
                ));
                log::info!(
                    "FCS: MT voice call_id={a1_call_id} negotiated FAP RTP {}:{} speech_payload={negotiated_speech} payloads={:?}",
                    endpoint.address,
                    endpoint.port,
                    endpoint.payload_types
                );
            }
            let fx2 = self
                .fap_registry
                .get(&dialog.fap_id)
                .map(|fap| fap.implementation.fx2());
            let femto = response.headers.content_type().and_then(|content_type| {
                sip_body_femto_message_with_fx2(content_type, &response.body, fx2)
            });
            if let Some(femto) = &femto {
                let (fcs, cell_identifier) = self.fcs_for_fap(Some(&dialog.fap_id));
                self.publish_a1(
                    Some(&dialog.fap_id),
                    &observed_body(femto),
                    crate::event::A1Context {
                        direction: cdma_events::proto::A1Direction::Uplink,
                        cell: Some(cell_identifier),
                        sip_call_id: response.headers.call_id().map(str::to_string),
                        content_type: response.headers.content_type().map(str::to_string),
                        peer: None,
                        a1_call_id: Some(a1_call_id),
                        correlated_imsi: self.correlated_imsi_for_call(fcs, a1_call_id),
                        peer_id: None,
                    },
                );
                match fcs.to_msc_a1(femto, cell_identifier, Some(a1_call_id)) {
                    Ok(message) if message.message_type() == MessageType::PagingResponse => {
                        self.forward_to_msc(
                            "forwarding MT Paging Response to MSC",
                            Some(&dialog.fap_id),
                            message,
                            UplinkObservation::AlreadyPublished,
                            crate::event::A1Context::default(),
                        );
                    }
                    Ok(message) => log::warn!(
                        "FCS: unexpected A1 {:?} in MT provisional response",
                        message.message_type()
                    ),
                    Err(_)
                        if femto.message_type
                            == cdma_ios::femto_interface_msg::MSG_TYPE_ASSIGNMENT_COMPLETE =>
                    {
                        log::debug!(
                            "FCS: MT voice call_id={a1_call_id} received Assignment Complete provisional body"
                        );
                    }
                    Err(error) => log::warn!("FCS: decoding MT provisional A1 body: {error}"),
                }
            }
            if let Some(rseq) = response_rseq
                && dialog.last_pracked_rseq.is_some()
                && dialog.last_pracked_rseq != Some(rseq)
            {
                let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
                let mut prack = SipRequest::new(Method::Prack, dialog.request_uri.clone());
                prack.headers.append(
                    "Via",
                    format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbtsprack{transaction}"),
                );
                prack.headers.append("From", &dialog.from);
                prack
                    .headers
                    .append("To", dialog.remote_to.as_deref().unwrap_or(&dialog.to));
                prack.headers.append("Call-ID", &dialog.sip_call_id);
                let prack_cseq = dialog.next_prack_cseq;
                dialog.next_prack_cseq = dialog.next_prack_cseq.saturating_add(1);
                dialog.last_pracked_rseq = Some(rseq);
                prack.headers.append("CSeq", format!("{prack_cseq} PRACK"));
                prack
                    .headers
                    .append(H_RACK, format!("{rseq} {SIP_INVITE_CSEQ} INVITE"));
                prack.headers.append("Max-Forwards", "70");
                prack
                    .headers
                    .append("Contact", format!("<sip:fcs@{local}>"));
                log::info!(
                    "FCS: acknowledging MT voice call_id={a1_call_id} reliable provisional RSeq {rseq}"
                );
                return vec![(prack, dialog.peer)];
            }
            return Vec::new();
        }
        if response.status == SC_OK
            && response.headers.cseq() == Some((SIP_FIRST_PRACK_CSEQ, "PRACK".to_string()))
        {
            let complete = AssignmentCompleteMessage {
                channel_number: ChannelNumber(0),
                encryption_information: None,
                service_option: Some(cdma_ios::ServiceOption(dialog.service_option)),
                a2p_bearer_session_params: Some(A2pBearerSessionParams {
                    ip_address: match dialog.relay.msc_local().ip() {
                        std::net::IpAddr::V4(address) => address,
                        _ => std::net::Ipv4Addr::UNSPECIFIED,
                    },
                    udp_port: dialog.relay.msc_local().port(),
                }),
                a2p_bearer_format_params: None,
            };
            match complete.encode() {
                Ok(payload) => {
                    let encoded = EncodedA1Message::from_message_for_call(
                        &Message::new(MessageType::AssignmentComplete, payload),
                        Some(a1_call_id),
                    );
                    self.forward_to_msc(
                        "forwarding MT Assignment Complete to MSC",
                        Some(&dialog.fap_id),
                        encoded,
                        UplinkObservation::A1Wire,
                        self.a1_context(
                            cdma_events::proto::A1Direction::Uplink,
                            Some(&dialog.fap_id),
                            Some(a1_call_id),
                            None,
                            Some(dialog.peer),
                        ),
                    );
                }
                Err(error) => log::warn!("FCS: encoding MT Assignment Complete: {error}"),
            }
            log::info!("FCS: MT voice call_id={a1_call_id} traffic assignment completed");
            return Vec::new();
        }
        if response.status == SC_OK && cseq_method.as_deref() == Some("INVITE") {
            dialog.answered = true;
            if let Ok(payload) = ConnectMessage.encode() {
                let encoded = EncodedA1Message::from_message_for_call(
                    &Message::new(MessageType::Connect, payload),
                    Some(a1_call_id),
                );
                self.forward_to_msc(
                    "forwarding MT Connect to MSC",
                    Some(&dialog.fap_id),
                    encoded,
                    UplinkObservation::A1Wire,
                    self.a1_context(
                        cdma_events::proto::A1Direction::Uplink,
                        Some(&dialog.fap_id),
                        Some(a1_call_id),
                        None,
                        Some(dialog.peer),
                    ),
                );
            }
            let transaction = self.next_sip_transaction.fetch_add(1, Ordering::Relaxed);
            let mut ack = SipRequest::new(Method::Ack, dialog.request_uri.clone());
            ack.headers.append(
                "Via",
                format!("SIP/2.0/UDP {local};branch=z9hG4bK1xbtsack{transaction}"),
            );
            ack.headers.append("From", &dialog.from);
            ack.headers
                .append("To", dialog.remote_to.as_deref().unwrap_or(&dialog.to));
            ack.headers.append("Call-ID", &dialog.sip_call_id);
            ack.headers.append("CSeq", format!("{SIP_INVITE_CSEQ} ACK"));
            ack.headers.append("Max-Forwards", "70");
            log::info!("FCS: MT voice call_id={a1_call_id} answered");
            return vec![(ack, dialog.peer)];
        }
        if response.status >= 300 {
            dialog.cancel_sent = true;
            log::warn!(
                "FCS: MT voice call_id={a1_call_id} failed with SIP {} {}",
                response.status,
                response.reason
            );
            let peer = dialog.peer;
            let fap_id = dialog.fap_id.clone();
            let ack = (cseq_method.as_deref() == Some("INVITE")).then(|| {
                let mut ack = SipRequest::new(Method::Ack, dialog.request_uri.clone());
                ack.headers.append(
                    "Via",
                    format!("SIP/2.0/UDP {local};branch={}", dialog.invite_branch),
                );
                ack.headers.append("From", &dialog.from);
                ack.headers
                    .append("To", dialog.remote_to.as_deref().unwrap_or(&dialog.to));
                ack.headers.append("Call-ID", &dialog.sip_call_id);
                ack.headers.append("CSeq", format!("{SIP_INVITE_CSEQ} ACK"));
                ack.headers.append("Max-Forwards", "70");
                ack
            });
            drop(dialogs);
            // The callee leg failed: release the whole call over A1 (both legs
            // share this call_id) so the caller stops ringing immediately.
            self.release_call_over_a1(a1_call_id, Some(fap_id.as_str()));
            return ack.map(|ack| vec![(ack, peer)]).unwrap_or_default();
        }
        Vec::new()
    }

    fn handle_downlink_response(&self, response: &SipResponse) {
        let Some(call_id) = response.headers.call_id() else {
            log::warn!("SIP response without Call-ID");
            return;
        };
        let mut pending = self.pending_downlink.lock().expect("pending downlink lock");
        let transaction = if response.status >= SC_OK {
            pending.remove(call_id)
        } else {
            pending.get(call_id).cloned()
        };
        if let Some(transaction) = transaction {
            log::info!(
                "FCS: Fx2 downlink {:?} call_id={:?} received SIP {} {}",
                transaction.message_type,
                transaction.a1_call_id,
                response.status,
                response.reason
            );
        } else {
            log::debug!(
                "SIP response {} {} call-id={call_id}",
                response.status,
                response.reason
            );
        }
    }

    fn stage_airave_adds_page_ack(&self, pending: &PendingDownlink) {
        let Some((mobile_identity, ack)) = &pending.adds_page_ack else {
            return;
        };
        self.pending_adds_page_acks
            .lock()
            .expect("pending ADDS Page Acks lock")
            .insert(mobile_identity.clone(), ack.clone());
    }

    fn recover_registered_fap_peer(&self, peer: SocketAddr) {
        let mut registered = self.registered_faps.lock().expect("registered FAPs lock");
        let users = self.users.read().expect("sip users lock");
        if let Some(route) = registered
            .values()
            .find(|route| route.peer == peer)
            .cloned()
        {
            self.cell_state
                .register_fap(&route.fap_id, route.public_identity.clone(), route.peer);
            return;
        }
        if let Some(feid) = self.cell_state.feid_for_inner_peer(peer.ip()) {
            if let Some(user) = users.iter().find(|user| user.feid == feid) {
                registered.insert(
                    user.feid.clone(),
                    RegisteredFap {
                        fap_id: user.feid.clone(),
                        public_identity: user.impu.clone(),
                        peer,
                    },
                );
                self.cell_state
                    .register_fap(&user.feid, user.impu.clone(), peer);
                log::info!(
                    "SIP recovered Fx2 downlink route {} via {} from persisted FAP peer",
                    user.impu,
                    peer
                );
                return;
            }
        }
        let candidates = users
            .iter()
            .filter(|user| !registered.contains_key(&user.feid))
            .collect::<Vec<_>>();
        let [user] = candidates.as_slice() else {
            log::warn!("SIP cannot infer a unique FAP identity for trusted peer {peer}");
            return;
        };
        registered.insert(
            user.feid.clone(),
            RegisteredFap {
                fap_id: user.feid.clone(),
                public_identity: user.impu.clone(),
                peer,
            },
        );
        self.cell_state
            .register_fap(&user.feid, user.impu.clone(), peer);
        log::info!(
            "SIP recovered Fx2 downlink route {} via {} from trusted FAP traffic",
            user.impu,
            peer
        );
    }

    fn recover_registered_fap_hint(&self, hint: FapRouteHint) {
        let users = self.users.read().expect("sip users lock");
        let Some(user) = users.iter().find(|user| {
            sip_uri_user(&user.impu)
                .is_some_and(|identity| identity.eq_ignore_ascii_case(&hint.serial_number))
                || user.impi.eq_ignore_ascii_case(&hint.serial_number)
        }) else {
            log::warn!(
                "ACS route hint for unconfigured FAP serial {} ignored",
                hint.serial_number
            );
            return;
        };
        let route = RegisteredFap {
            fap_id: user.feid.clone(),
            public_identity: user.impu.clone(),
            peer: hint.peer,
        };
        self.registered_faps
            .lock()
            .expect("registered FAPs lock")
            .insert(user.feid.clone(), route.clone());
        // Mirror the binding into cell state so an ACS Inform sets the Fx2 downlink
        // sip_peer like a REGISTER — otherwise the cell reports the FAP unregistered.
        self.cell_state
            .register_fap(&user.feid, user.impu.clone(), hint.peer);
        log::info!(
            "SIP recovered Fx2 downlink route {} via {} from authenticated ACS Inform",
            route.public_identity,
            route.peer
        );
    }

    async fn send_fx2_downlink(
        &self,
        encoded: &EncodedA1Message,
        local: SocketAddr,
        transport: &cdma_sip::UdpSipTransport,
    ) {
        let (request, peer, call_id, pending) = match self.downlink_request(encoded, local) {
            Ok(value) => value,
            Err(error) => {
                log::warn!("FCS: could not map MSC A1 to Fx2 downlink: {error}");
                return;
            }
        };
        self.stage_airave_adds_page_ack(&pending);
        self.pending_downlink
            .lock()
            .expect("pending downlink lock")
            .insert(call_id, pending);
        if let Err(error) = transport
            .send(&cdma_sip::SipMessage::Request(request), peer)
            .await
        {
            log::warn!("FCS: sending Fx2 downlink to {peer} failed: {error}");
        } else {
            log::info!(
                "FCS: sent Fx2 downlink {:?} call_id={:?} to {peer}",
                encoded.message_type(),
                encoded.call_id()
            );
        }
    }

    pub async fn serve(&self, listen: SocketAddr) -> Result<(), cdma_sip::TransportError> {
        let mut transport = cdma_sip::UdpSipTransport::bind(listen).await?;
        let local = transport.local_addr()?;
        let mut msc_a1_rx = self.msc_a1_rx.lock().expect("MSC A1 receiver lock").take();
        let mut sms_cause_code_rx = self
            .sms_cause_code_rx
            .lock()
            .expect("SMS Cause Code receiver lock")
            .take();
        let mut fap_route_hint_rx = self
            .fap_route_hint_rx
            .lock()
            .expect("FAP route hint receiver lock")
            .take();
        let mut otasp_timer = tokio::time::interval(SIP_T1);
        otasp_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut voice_dialog_timer = tokio::time::interval(SIP_DIALOG_TIMER_INTERVAL);
        voice_dialog_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        log::info!("IMS/SIP registrar listening on {local}");
        loop {
            tokio::select! {
                hint = receive_fap_route_hint(&mut fap_route_hint_rx) => {
                    let Some(hint) = hint else {
                        fap_route_hint_rx = None;
                        continue;
                    };
                    self.recover_registered_fap_hint(hint);
                }
                received = transport.recv() => {
                    let (message, peer) = match received {
                        Ok(value) => value,
                        Err(error) => {
                            log::warn!("SIP recv error: {error}");
                            continue;
                        }
                    };
                    match message {
                        cdma_sip::SipMessage::Request(request) => {
                            if request.method != Method::Register {
                                self.recover_registered_fap_peer(peer);
                            }
                            log::info!(
                                "SIP rx {} call-id={} cseq={:?} from {}",
                                request.method,
                                request.headers.call_id().unwrap_or("missing"),
                                request.headers.cseq(),
                                peer
                            );
                            // SIP ACK is never answered. For a locally served
                            // OTASP call it releases the queued data bursts.
                            if request.method == Method::Ack {
                                if let Some(bursts) = self.otasp_after_ack(&request, peer) {
                                    for encoded in bursts {
                                        self.send_fx2_downlink(&encoded, local, &transport).await;
                                    }
                                }
                                continue;
                            }
                            let response = if request.method == Method::Invite {
                                self.handle_mo_invite(&request, peer)
                            } else {
                                self.handle_request_from(&request, peer)
                            };
                            let registered = request.method == Method::Register
                                && response.status == SC_OK;
                            if let Err(error) = transport
                                .send(&cdma_sip::SipMessage::Response(response), peer)
                                .await
                            {
                                log::warn!("SIP send to {peer} failed: {error}");
                            } else if registered {
                                self.remember_registered_fap(&request, peer);
                            }
                        }
                        cdma_sip::SipMessage::Response(response) => {
                            let requests = self.handle_mt_voice_response(&response, local);
                            for (request, peer) in requests {
                                if let Err(error) = transport
                                    .send(&cdma_sip::SipMessage::Request(request), peer)
                                    .await
                                {
                                    log::warn!("FCS: sending in-dialog SIP request to {peer}: {error}");
                                }
                            }
                            self.handle_downlink_response(&response);
                        }
                    }
                }
                downlink = self.next_downlink(&mut msc_a1_rx) => {
                    let Some(encoded) = downlink else {
                        msc_a1_rx = None;
                        continue;
                    };
                    let message_type = encoded.message_type();
                    let a1_call_id = encoded.call_id();
                    if message_type == MessageType::PagingRequest
                        && !is_sms_service_option_page(&encoded)
                    {
                        match self.mt_invite_request(&encoded, local) {
                            Ok((request, peer, sip_call_id)) => {
                                if let Err(error) = transport
                                    .send(&cdma_sip::SipMessage::Request(request), peer)
                                    .await
                                {
                                    log::warn!("FCS: sending MT INVITE to {peer}: {error}");
                                } else {
                                    log::info!(
                                        "FCS: sent MT INVITE call_id={a1_call_id:?} sip_call_id={sip_call_id} to {peer}"
                                    );
                                }
                            }
                            Err(error) => log::warn!("FCS: building MT INVITE: {error}"),
                        }
                        continue;
                    }
                    if message_type == MessageType::ClearCommand
                        && let Some(call_id) = a1_call_id
                    {
                        self.pending_otasp_answer.lock().expect("OTASP answer lock").remove(&call_id);
                        self.adds_delivery_calls.lock().expect("ADDS delivery calls lock")
                            .retain(|_, active_call| *active_call != call_id);
                        self.call_fap
                            .lock()
                            .expect("call fap lock")
                            .remove(&call_id);
                        self.call_mobile_identities
                            .lock()
                            .expect("call mobile identities lock")
                            .remove(&call_id);
                        for (message, peer) in self.clear_dialog_requests(call_id, local) {
                            if let Err(error) = transport.send(&message, peer).await {
                                log::warn!("FCS: sending call-clearing SIP to {peer}: {error}");
                            }
                        }
                        let complete = ClearCompleteMessage {
                            power_down_indicator: false,
                        };
                        if let Ok(payload) = complete.encode() {
                            self.forward_to_msc(
                                "forwarding A1 Clear Complete to MSC",
                                None,
                                EncodedA1Message::from_message_for_call(
                                    &Message::new(MessageType::ClearComplete, payload),
                                    Some(call_id),
                                ),
                                UplinkObservation::A1Wire,
                                self.a1_context(
                                    cdma_events::proto::A1Direction::Uplink,
                                    None,
                                    Some(call_id),
                                    None,
                                    None,
                                ),
                            );
                        }
                        log::info!("FCS: cleared SIP dialogs for A1 call_id={call_id}");
                        continue;
                    }
                    let voice_assignment_dialog = if message_type
                        == MessageType::AssignmentRequest
                    {
                        a1_call_id.and_then(|call_id| {
                            select_voice_assignment_dialog(
                                self.mt_voice_by_call
                                    .lock()
                                    .expect("MT voice A1 dialog lock")
                                    .contains_key(&call_id),
                                self.mo_voice_by_call
                                    .lock()
                                    .expect("MO voice A1 dialog lock")
                                    .contains_key(&call_id),
                            )
                        })
                    } else {
                        None
                    };
                    if message_type == MessageType::AssignmentRequest
                        && voice_assignment_dialog
                            == Some(VoiceAssignmentDialog::MobileTerminated)
                    {
                        match self.mt_prack_request(&encoded, local) {
                            Ok((request, peer)) => {
                                if let Err(error) = transport
                                    .send(&cdma_sip::SipMessage::Request(request), peer)
                                    .await
                                {
                                    log::warn!("FCS: sending MT PRACK to {peer}: {error}");
                                } else {
                                    log::info!("FCS: sent MT PRACK call_id={a1_call_id:?} to {peer}");
                                }
                            }
                            Err(error) => log::warn!("FCS: building MT PRACK: {error}"),
                        }
                        continue;
                    }
                    if message_type == MessageType::AssignmentRequest
                        && voice_assignment_dialog
                            == Some(VoiceAssignmentDialog::MobileOriginated)
                    {
                        match self.mo_assignment_progress(&encoded, local) {
                            Ok((response, peer)) => {
                                if let Err(error) = transport
                                    .send(&cdma_sip::SipMessage::Response(response), peer)
                                    .await
                                {
                                    log::warn!("FCS: sending MO 183 to {peer}: {error}");
                                }
                            }
                            Err(error) => log::warn!("FCS: building MO 183: {error}"),
                        }
                        continue;
                    }
                    if message_type == MessageType::AlertWithInformation
                        && let Some((response, peer)) =
                            self.mo_dialog_response(&encoded, SC_RINGING, "Ringing")
                    {
                        if let Err(error) = transport
                            .send(&cdma_sip::SipMessage::Response(response), peer)
                            .await
                        {
                            log::warn!("FCS: sending MO 180 to {peer}: {error}");
                        }
                        continue;
                    }
                    if message_type == MessageType::Progress
                        && let Some((response, peer)) =
                            self.mo_dialog_response(&encoded, SC_OK, "OK")
                    {
                        if let Err(error) = transport
                            .send(&cdma_sip::SipMessage::Response(response), peer)
                            .await
                        {
                            log::warn!("FCS: sending MO 200 to {peer}: {error}");
                        }
                        continue;
                    }
                    if message_type == MessageType::AlertWithInformation
                        && a1_call_id.is_some_and(|call_id| {
                            self.mt_voice_by_call
                                .lock()
                                .expect("MT voice A1 dialog lock")
                                .contains_key(&call_id)
                        })
                    {
                        log::info!(
                            "FCS: MT voice call_id={a1_call_id:?} alerting is handled by the FAP after PRACK"
                        );
                        continue;
                    }
                    match self.stage_otasp_answer(&encoded) {
                        Ok(Some((answer, peer))) => {
                            if let Err(error) = transport.send(
                                &cdma_sip::SipMessage::Response(answer), peer).await {
                                log::warn!("FCS: sending OTASP SIP answer to {peer}: {error}");
                            }
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            log::warn!("FCS: preparing OTASP call answer: {error}");
                            continue;
                        }
                    }
                    self.send_fx2_downlink(&encoded, local, &transport).await;
                }
                _ = otasp_timer.tick() => {
                    for (message, peer) in self.otasp_answer_timer(Instant::now(), local) {
                        if let Err(error) = transport.send(&message, peer).await {
                            log::warn!("FCS: sending OTASP dialog response to {peer}: {error}");
                        }
                    }
                }
                _ = voice_dialog_timer.tick() => {
                    for (request, peer) in self.expired_mt_cancels(local) {
                        if let Err(error) = transport
                            .send(&cdma_sip::SipMessage::Request(request), peer)
                            .await
                        {
                            log::warn!("FCS: sending MT CANCEL to {peer}: {error}");
                        }
                    }
                }
                pending = receive_sms_cause_code(&mut sms_cause_code_rx) => {
                    let Some(pending) = pending else {
                        sms_cause_code_rx = None;
                        continue;
                    };
                    let mobile_identity = pending.mobile_identity.clone();
                    let (request, peer, call_id) = match self.sms_cause_code_request(pending, local) {
                        Ok(value) => value,
                        Err(error) => {
                            log::warn!("FCS: could not build SMS Cause Code downlink: {error}");
                            continue;
                        }
                    };
                    if let Err(error) = transport
                        .send(&cdma_sip::SipMessage::Request(request), peer)
                        .await
                    {
                        log::warn!("FCS: sending SMS Cause Code to {peer} failed: {error}");
                    } else {
                        log::info!(
                            "FCS: sent SMS Cause Code to {mobile_identity} via {peer} call-id={call_id}"
                        );
                    }
                }
            }
        }
    }
}

/// The body as it should be logged, recovering a message discriminator the FAP
/// omitted. The Airvana sends Assignment Complete without one, and 0x02 is
/// BSMAP Assignment Complete but DTAP Location Updating Accept. A successful
/// typed decode settles it. The forwarded bytes come from the original body.
fn observed_body(
    femto: &cdma_ios::femto_interface_msg::FemtoInterfaceMsg,
) -> cdma_ios::femto_interface_msg::FemtoInterfaceMsg {
    let mut observed = femto.clone();
    if observed.discriminator.is_none() && observed.to_assignment_complete().is_ok() {
        observed.discriminator = Some(cdma_ios::femto_interface_msg::Discriminator::Bsmap);
    }
    observed
}

#[cfg(test)]
fn response_femto_message(
    response: &SipResponse,
) -> Option<cdma_ios::femto_interface_msg::FemtoInterfaceMsg> {
    sip_body_femto_message(response.headers.content_type()?, &response.body)
}

#[cfg(test)]
fn request_femto_message(
    request: &SipRequest,
) -> Option<cdma_ios::femto_interface_msg::FemtoInterfaceMsg> {
    sip_body_femto_message(request.headers.content_type()?, &request.body)
}

fn request_femto_message_with_fx2(
    request: &SipRequest,
    fx2: Option<&dyn FapFx2>,
) -> Option<cdma_ios::femto_interface_msg::FemtoInterfaceMsg> {
    sip_body_femto_message_with_fx2(request.headers.content_type()?, &request.body, fx2)
}

#[cfg(test)]
fn sip_body_femto_message(
    content_type: &str,
    body: &[u8],
) -> Option<cdma_ios::femto_interface_msg::FemtoInterfaceMsg> {
    sip_body_femto_message_with_fx2(content_type, body, None)
}

fn sip_body_femto_message_with_fx2(
    content_type: &str,
    body: &[u8],
    fx2: Option<&dyn FapFx2>,
) -> Option<cdma_ios::femto_interface_msg::FemtoInterfaceMsg> {
    let base = content_type.split(';').next()?.trim();
    let accepts = |content_type: &str| {
        fx2.map(|fx2| fx2.accepts_femto_content_type(content_type))
            .unwrap_or_else(|| {
                content_type.eq_ignore_ascii_case(CONTENT_TYPE_FEMTO)
                    || content_type.eq_ignore_ascii_case("application/FemtoInterfaceMsg")
            })
    };
    if accepts(base) {
        return crate::fcs::decode_femto_body(base, body).ok();
    }
    if !base.to_ascii_lowercase().starts_with("multipart/") {
        return None;
    }
    let boundary = content_type_parameter(content_type, "boundary")?;
    let multipart = Multipart::parse(&boundary, body).ok()?;
    multipart.parts.iter().find_map(|part| {
        accepts(&part.content_type)
            .then(|| crate::fcs::decode_femto_body(&part.content_type, &part.body).ok())
            .flatten()
    })
}

fn response_sdp_endpoint(response: &SipResponse) -> Option<crate::voice::SdpAudioEndpoint> {
    sip_body_sdp_endpoint(response.headers.content_type()?, &response.body)
}

fn response_sdp_body(response: &SipResponse) -> Option<Vec<u8>> {
    sip_body_sdp(response.headers.content_type()?, &response.body)
}

fn request_sdp_endpoint(request: &SipRequest) -> Option<crate::voice::SdpAudioEndpoint> {
    sip_body_sdp_endpoint(request.headers.content_type()?, &request.body)
}

fn request_sdp_body(request: &SipRequest) -> Option<Vec<u8>> {
    sip_body_sdp(request.headers.content_type()?, &request.body)
}

fn sip_body_sdp_endpoint(
    content_type: &str,
    body: &[u8],
) -> Option<crate::voice::SdpAudioEndpoint> {
    parse_sdp_audio(&sip_body_sdp(content_type, body)?).ok()
}

fn sip_body_sdp(content_type: &str, body: &[u8]) -> Option<Vec<u8>> {
    let base = content_type.split(';').next()?.trim();
    if base.eq_ignore_ascii_case(CONTENT_TYPE_SDP) {
        return Some(body.to_vec());
    }
    if !base.to_ascii_lowercase().starts_with("multipart/") {
        return None;
    }
    let boundary = content_type_parameter(content_type, "boundary")?;
    let multipart = Multipart::parse(&boundary, body).ok()?;
    multipart
        .parts
        .iter()
        .find(|part| part.content_type.eq_ignore_ascii_case(CONTENT_TYPE_SDP))
        .map(|part| part.body.clone())
}

fn content_type_parameter(content_type: &str, name: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (key, value) = parameter.trim().split_once('=')?;
        key.eq_ignore_ascii_case(name)
            .then(|| value.trim().trim_matches('"').to_string())
    })
}

async fn receive_downlink(
    receiver: &mut Option<mpsc::Receiver<EncodedA1Message>>,
) -> Option<EncodedA1Message> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn receive_sms_cause_code(
    receiver: &mut Option<mpsc::UnboundedReceiver<PendingSmsCauseCode>>,
) -> Option<PendingSmsCauseCode> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn receive_fap_route_hint(
    receiver: &mut Option<mpsc::UnboundedReceiver<FapRouteHint>>,
) -> Option<FapRouteHint> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn sip_uri_from_name_addr(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if let Some(start) = trimmed.find('<') {
        let rest = &trimmed[start + 1..];
        return rest.find('>').map(|end| rest[..end].trim().to_string());
    }
    trimmed
        .split(';')
        .next()
        .map(str::trim)
        .filter(|uri| uri.starts_with("sip:"))
        .map(str::to_string)
}

fn sip_uri_with_user(uri: &str, user: &str) -> Option<String> {
    let (scheme, address) = uri.split_once(':')?;
    let (_, host) = address.split_once('@')?;
    Some(format!("{scheme}:{user}@{host}"))
}

fn sip_uri_user(uri: &str) -> Option<&str> {
    uri.strip_prefix("sip:")?
        .split_once('@')
        .map(|(user, _)| user)
}

fn sip_mobile_identity_candidates(request: &SipRequest) -> Vec<FapMobileIdentityCandidate> {
    [
        (
            request.headers.from(),
            FapMobileIdentityCandidate::SipFrom as fn(String) -> FapMobileIdentityCandidate,
        ),
        (
            request.headers.contact(),
            FapMobileIdentityCandidate::SipContact as fn(String) -> FapMobileIdentityCandidate,
        ),
    ]
    .into_iter()
    .filter_map(|(header, candidate)| {
        header
            .and_then(sip_uri_from_name_addr)
            .and_then(|uri| sip_uri_user(&uri).map(str::to_string))
            .map(candidate)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FapDefinition, FapImplementationKind, FapProfileConfig, FemtoSiteConfig};
    use cdma_ios::femto_interface_msg::{Discriminator, FemtoIe, FemtoInterfaceMsg};
    use cdma_ios::{
        AddsPageMessage, AddsUserPart, AssignmentRequestMessage, ChannelType,
        CompleteLayer3InformationMessage, MessageType, MobileIdentity, ServiceOption, Tag,
    };
    use cdma_sip::message::Method;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    pub(super) fn otasp_test_core() -> (ImsCore, mpsc::Receiver<EncodedA1Message>) {
        let (tx, rx) = mpsc::channel(8);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 403,
                sector: 0,
            },
            Some(tx),
        );
        core.registered_faps.lock().unwrap().insert(
            "fap-a".into(),
            RegisteredFap {
                fap_id: "fap-a".into(),
                public_identity: "sip:fap-a@example.test".into(),
                peer: "198.51.100.1:5060".parse().unwrap(),
            },
        );
        for (call_id, short, full, esn) in [
            (41, "5551239999", "310005551239999", 0x80E4D11C),
            (42, "5551239876", "310005551239876", 0x80B66BD7),
        ] {
            let mobile = RegisteredMobile {
                imsi: MobileIdentity::Imsi(full.into()),
                esn: Some(MobileIdentity::Esn(esn)),
                meid: None,
                fap_id: "fap-a".into(),
            };
            let mut mobiles = core.registered_mobiles.lock().unwrap();
            mobiles.insert(short.into(), mobile.clone());
            mobiles.insert(full.into(), mobile);
            drop(mobiles);
            core.call_mobile_identities
                .lock()
                .unwrap()
                .insert(call_id, short.into());
            core.call_fap
                .lock()
                .unwrap()
                .insert(call_id, "fap-a".into());
        }
        (core, rx)
    }

    pub(super) fn otasp_message(body: &FemtoInterfaceMsg) -> SipRequest {
        let mut req = SipRequest::new(Method::Message, "sip:fcs@example.test");
        req.headers
            .append("From", "5551239999<sip:fap-a@example.test>;tag=ota");
        req.headers.append("To", "<sip:fcs@example.test>");
        req.headers
            .append("Call-ID", "new-standalone-otasp-message");
        req.headers.append("CSeq", "1 MESSAGE");
        req.headers
            .append("Via", "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKmessage");
        req.headers
            .append("Content-Type", "application/FemtoInterfaceMsg");
        req.body = body.encode();
        req
    }

    #[test]
    fn otasp_standalone_uplink_resolves_correct_handset_and_hardware_identity() {
        let (core, mut rx) = otasp_test_core();
        let deliver = cdma_ios::AddsDeliverMessage {
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6, 1, 2],
            },
            tag: None,
        };
        let req = otasp_message(&FemtoInterfaceMsg::from_adds_deliver(&deliver).unwrap());
        assert_eq!(
            core.handle_message(&req, "198.51.100.1:5060".parse().unwrap())
                .status,
            SC_OK
        );
        let encoded = rx.try_recv().unwrap();
        assert_eq!(encoded.call_id(), Some(41));
        assert_eq!(encoded.message_type(), MessageType::AddsTransfer);
        let transfer = AddsTransferMessage::decode(&encoded.decode().unwrap().payload).unwrap();
        assert_eq!(
            transfer.mobile_identity_imsi,
            MobileIdentity::Imsi("310005551239999".into())
        );
        assert_eq!(
            transfer.mobile_identity_esn,
            Some(MobileIdentity::Esn(0x80E4D11C))
        );
        assert_eq!(transfer.adds_user_part, deliver.adds_user_part);
    }

    #[test]
    fn airvana_ota_reply_without_discriminator_reaches_correct_msc_call() {
        for header in ["53", "53 (=\"ADDS Deliver\")"] {
            let (core, mut rx) = otasp_test_core();
            let deliver = cdma_ios::AddsDeliverMessage {
                adds_user_part: AddsUserPart {
                    burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                    data: vec![6, 1, 2],
                },
                tag: None,
            };
            let mut req = otasp_message(&FemtoInterfaceMsg::from_adds_deliver(&deliver).unwrap());
            let body = String::from_utf8(req.body).unwrap();
            req.body = format!("{header}\r\n{}", body.split_once("\r\n").unwrap().1).into_bytes();
            assert!(FemtoInterfaceMsg::parse(&req.body).is_err());
            assert!(request_femto_message_with_fx2(&req, None).is_some());
            assert_eq!(
                core.handle_message(&req, "198.51.100.1:5060".parse().unwrap())
                    .status,
                SC_OK
            );
            let encoded = rx.try_recv().unwrap();
            assert_eq!(encoded.call_id(), Some(41));
            assert_eq!(encoded.message_type(), MessageType::AddsTransfer);
            let actual = AddsTransferMessage::decode(&encoded.decode().unwrap().payload).unwrap();
            assert_eq!(actual.adds_user_part, deliver.adds_user_part);
            assert_eq!(
                actual.mobile_identity_esn,
                Some(MobileIdentity::Esn(0x80E4D11C))
            );
        }
        for (content_type, body) in [
            (
                "application/vnd.3gpp2.FemtoInterfaceMsg",
                &b"53\r\n3D=PQIEBg==\r\n"[..],
            ),
            ("application/FemtoInterfaceMsg", &b"53\r\n"[..]),
            (
                "application/FemtoInterfaceMsg",
                &b"53\r\n3D=PQIDBg==\r\n"[..],
            ),
            (
                "application/FemtoInterfaceMsg",
                &b"53\r\n3D=PQkEBg==\r\n"[..],
            ),
        ] {
            assert!(crate::fcs::decode_femto_body(content_type, body).is_err());
        }
    }

    #[test]
    fn airvana_bare_info_ack_is_forwarded_only_for_matching_delivery_tag_and_fap() {
        let (core, mut rx) = otasp_test_core();
        core.adds_delivery_calls
            .lock()
            .unwrap()
            .insert(("fap-a".into(), 0x8000000c), 41);
        let mut req = SipRequest::new(Method::Info, "sip:fcs@example.test");
        req.headers
            .append("From", "5304023094<sip:fap-a@example.test>;tag=ota");
        req.headers.append("To", "<sip:fcs@example.test>");
        req.headers.append("Call-ID", "voice-dialog");
        req.headers.append("CSeq", "11 INFO");
        req.headers
            .append("Via", "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKinfo");
        req.headers
            .append("Content-Type", "application/FemtoInterfaceMsg");
        req.body = b"54\r\n33=M4AAAAw=\r\n".to_vec();
        assert!(FemtoInterfaceMsg::parse(&req.body).is_err());
        assert_ne!(
            core.handle_request_from(&req, "198.51.100.2:5060".parse().unwrap())
                .status,
            SC_OK
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(
            core.handle_request_from(&req, "198.51.100.1:5060".parse().unwrap())
                .status,
            SC_OK
        );
        let a1 = rx.try_recv().unwrap();
        assert_eq!(a1.call_id(), Some(41));
        assert_eq!(a1.message_type(), MessageType::AddsDeliverAck);
        assert_eq!(
            cdma_ios::AddsDeliverAckMessage::decode(&a1.decode().unwrap().payload)
                .unwrap()
                .tag,
            Some(cdma_ios::Tag(0x8000000c))
        );
        req.headers.set("CSeq", "12 INFO");
        req.body = b"54\r\n33=M4AAABI=\r\n".to_vec();
        assert_eq!(
            core.handle_request_from(&req, "198.51.100.1:5060".parse().unwrap())
                .status,
            SC_BAD_REQUEST
        );
        req.body = b"54\r\n".to_vec();
        assert_ne!(
            core.handle_request_from(&req, "198.51.100.1:5060".parse().unwrap())
                .status,
            SC_OK
        );
        req.body = b"53\r\n3D=PQIEBg==\r\n".to_vec();
        assert_ne!(
            core.handle_request_from(&req, "198.51.100.1:5060".parse().unwrap())
                .status,
            SC_OK
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn otasp_uplink_rejects_ambiguous_handset_and_wrong_fap() {
        let (core, mut rx) = otasp_test_core();
        let deliver = cdma_ios::AddsDeliverMessage {
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6],
            },
            tag: None,
        };
        let req = otasp_message(&FemtoInterfaceMsg::from_adds_deliver(&deliver).unwrap());
        assert_ne!(
            core.handle_message(&req, "198.51.100.2:5060".parse().unwrap())
                .status,
            SC_OK
        );
        core.call_mobile_identities
            .lock()
            .unwrap()
            .insert(43, "5551239999".into());
        core.call_fap.lock().unwrap().insert(43, "fap-a".into());
        assert_eq!(
            core.handle_message(&req, "198.51.100.1:5060".parse().unwrap())
                .status,
            SC_BAD_REQUEST
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn otasp_downlink_uses_femto_body_and_delivery_ack_retains_tag_and_cause() {
        let (core, mut rx) = otasp_test_core();
        let deliver = cdma_ios::AddsDeliverMessage {
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6],
            },
            tag: Some(Tag(0x80000001)),
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::AddsDeliver, deliver.encode().unwrap()),
            Some(41),
        );
        let (req, peer, _, _) = core
            .downlink_request(&encoded, "127.0.0.1:5060".parse().unwrap())
            .unwrap();
        assert_eq!(req.uri, "sip:fap-a@example.test");
        assert_eq!(peer, "198.51.100.1:5060".parse::<SocketAddr>().unwrap());
        let parsed = FemtoInterfaceMsg::parse(&req.body).unwrap();
        assert_eq!(parsed.to_adds_deliver().unwrap(), deliver);
        let ack = cdma_ios::AddsDeliverAckMessage {
            tag: deliver.tag,
            cause: Some(cdma_ios::Cause(0x20)),
        };
        let mut reply = otasp_message(&FemtoInterfaceMsg::from_adds_deliver_ack(&ack).unwrap());
        reply
            .headers
            .set("From", "<sip:fap-a@example.test>;tag=ack");
        assert_eq!(core.handle_message(&reply, peer).status, SC_OK);
        let returned = rx.try_recv().unwrap();
        assert_eq!(returned.call_id(), Some(41));
        assert_eq!(returned.message_type(), MessageType::AddsDeliverAck);
        assert_eq!(
            cdma_ios::AddsDeliverAckMessage::decode(&returned.decode().unwrap().payload).unwrap(),
            ack
        );
    }

    #[test]
    fn fap_sites_select_independent_identity_mappers() {
        let profiles = BTreeMap::from([(
            "standard".to_string(),
            FapProfileConfig {
                implementation: FapImplementationKind::StandardXs0059,
                connection_request: None,
                acs: None,
            },
        )]);
        let sites = BTreeMap::from([
            (
                "a".to_string(),
                FemtoSiteConfig {
                    mcc: "310".to_string(),
                    imsi_11_12: "00".to_string(),
                    cell_id: 403,
                    sector: 0,
                    ..FemtoSiteConfig::default()
                },
            ),
            (
                "b".to_string(),
                FemtoSiteConfig {
                    mcc: "311".to_string(),
                    imsi_11_12: "01".to_string(),
                    cell_id: 404,
                    sector: 1,
                    ..FemtoSiteConfig::default()
                },
            ),
        ]);
        let faps = BTreeMap::from([
            (
                "fap-a".to_string(),
                FapDefinition {
                    profile: "standard".to_string(),
                    site: "a".to_string(),
                    name: None,
                    cwmp_inform_response_style: crate::config::CwmpInformResponseStyle::Standard,
                },
            ),
            (
                "fap-b".to_string(),
                FapDefinition {
                    profile: "standard".to_string(),
                    site: "b".to_string(),
                    name: None,
                    cwmp_inform_response_style: crate::config::CwmpInformResponseStyle::Standard,
                },
            ),
        ]);
        let registry = Arc::new(FapRegistry::from_config(&profiles, &sites, &faps).unwrap());
        let mut sip_config = config(false);
        sip_config.users[0].feid = Some("fap-a".to_string());
        let mut second = sip_config.users[0].clone();
        second.impu = "sip:fap-b@femto.local.1xbts.org".to_string();
        second.impi = "fap-b".to_string();
        second.feid = Some("fap-b".to_string());
        sip_config.users.push(second);
        let core = ImsCore::with_runtime_config_and_faps(
            &sip_config,
            &FcsConfig::default(),
            CellId { cell: 1, sector: 0 },
            None,
            FemtoCellState::untracked(),
            registry,
        );

        let (first, first_cell) = core.fcs_for_fap(Some("fap-a"));
        let (second, second_cell) = core.fcs_for_fap(Some("fap-b"));
        assert_eq!(
            first.canonical_imsi("5551239999").unwrap(),
            "310005551239999"
        );
        assert_eq!(
            second.canonical_imsi("5551239999").unwrap(),
            "311015551239999"
        );
        assert_eq!(
            first_cell,
            CellId {
                cell: 403,
                sector: 0
            }
        );
        assert_eq!(
            second_cell,
            CellId {
                cell: 404,
                sector: 1
            }
        );
    }

    #[test]
    fn identical_sip_call_ids_are_scoped_to_the_serving_fap() {
        let core = ImsCore::new(&config(false));
        let first_peer = "10.55.100.1:5060".parse().unwrap();
        let second_peer = "10.55.100.2:5060".parse().unwrap();
        let route = |fap_id: &str, peer| RegisteredFap {
            fap_id: fap_id.to_string(),
            public_identity: "sip:fap@femto.local.1xbts.org".to_string(),
            peer,
        };
        core.registered_faps.lock().unwrap().extend([
            ("fap-a".to_string(), route("fap-a", first_peer)),
            ("fap-b".to_string(), route("fap-b", second_peer)),
        ]);
        let mut request = SipRequest::new(Method::Message, "sip:fcs@femto.local.1xbts.org");
        request.headers.append("Call-ID", "shared-call-id");

        let first = core.call_id_for(&request, Some(first_peer));
        let second = core.call_id_for(&request, Some(second_peer));

        assert_ne!(first, second);
        assert_eq!(core.call_id_for(&request, Some(first_peer)), first);
        assert_eq!(core.call_id_for(&request, Some(second_peer)), second);
    }

    #[test]
    fn standard_profile_rejects_airave_mime_alias() {
        let femto = FemtoInterfaceMsg {
            discriminator: Some(Discriminator::Dtap),
            message_type: 0x02,
            message_name: Some("Location Updating Accept".to_string()),
            ies: Vec::new(),
        };
        let mut request = SipRequest::new(Method::Message, "sip:fcs@example.test");
        request
            .headers
            .append("Content-Type", "application/FemtoInterfaceMsg");
        request.body = femto.encode();
        let standard = crate::fap::implementation(FapImplementationKind::StandardXs0059);
        let airave = crate::fap::implementation(FapImplementationKind::AirvanaAirave2);

        assert!(request_femto_message_with_fx2(&request, Some(standard.fx2())).is_none());
        assert!(request_femto_message_with_fx2(&request, Some(airave.fx2())).is_some());
    }

    #[test]
    fn sip_mobile_identity_candidates_retain_provenance() {
        let mut request = SipRequest::new(Method::Message, "sip:fcs@example.test");
        request
            .headers
            .append("From", "Mobile <sip:5551239999@example.test>;tag=1");
        request
            .headers
            .append("Contact", "<sip:0005B9AABBCC@10.55.100.1:5060>");

        assert_eq!(
            sip_mobile_identity_candidates(&request),
            vec![
                FapMobileIdentityCandidate::SipFrom("5551239999".to_string()),
                FapMobileIdentityCandidate::SipContact("0005B9AABBCC".to_string()),
            ]
        );
    }

    #[test]
    fn mt_sms_routes_by_destination_with_multiple_registered_faps() {
        let core = ImsCore::new(&config(false));
        let first_peer = "10.55.100.1:5060".parse().unwrap();
        let second_peer = "10.55.100.2:5060".parse().unwrap();
        core.registered_faps.lock().unwrap().extend([
            (
                "fap-a".to_string(),
                RegisteredFap {
                    fap_id: "fap-a".to_string(),
                    public_identity: "sip:fap-a@example.test".to_string(),
                    peer: first_peer,
                },
            ),
            (
                "fap-b".to_string(),
                RegisteredFap {
                    fap_id: "fap-b".to_string(),
                    public_identity: "sip:fap-b@example.test".to_string(),
                    peer: second_peer,
                },
            ),
        ]);
        core.registered_mobiles.lock().unwrap().insert(
            "310005551239999".to_string(),
            RegisteredMobile {
                imsi: MobileIdentity::Imsi("310005551239999".to_string()),
                esn: None,
                meid: None,
                fap_id: "fap-b".to_string(),
            },
        );
        let page = AddsPageMessage {
            mobile_identity: MobileIdentity::Imsi("310005551239999".to_string()),
            adds_user_part: AddsUserPart {
                burst_type: SMS_DATA_BURST_TYPE,
                data: vec![1, 2, 3],
            },
            tag: Some(Tag(1)),
            slot_cycle_index: None,
        };
        let encoded = EncodedA1Message::from_message(&Message::new(
            MessageType::AddsPage,
            page.encode().unwrap(),
        ));

        let (_, peer, _, _) = core
            .downlink_request(&encoded, "10.56.0.2:5060".parse().unwrap())
            .unwrap();

        assert_eq!(peer, second_peer);
    }

    #[test]
    fn mt_assignment_routes_to_paging_fap_with_multiple_registered_faps() {
        let core = ImsCore::new(&config(false));
        let first_peer = "10.55.100.1:5060".parse().unwrap();
        let second_peer = "10.55.100.2:5060".parse().unwrap();
        core.registered_faps.lock().unwrap().extend([
            (
                "fap-a".to_string(),
                RegisteredFap {
                    fap_id: "fap-a".to_string(),
                    public_identity: "sip:fap-a@example.test".to_string(),
                    peer: first_peer,
                },
            ),
            (
                "fap-b".to_string(),
                RegisteredFap {
                    fap_id: "fap-b".to_string(),
                    public_identity: "sip:fap-b@example.test".to_string(),
                    peer: second_peer,
                },
            ),
        ]);
        attach_mobile(&core, "310005551239999", "fap-b");
        let local: SocketAddr = "10.56.0.2:5060".parse().unwrap();

        let page = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            tag: Some(Tag(1)),
            cell_identifier_list: None,
            slot_cycle_index: None,
            service_option: Some(ServiceOption(6)),
            is2000_mobile_capabilities: None,
        };
        let page = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::PagingRequest, page.encode().unwrap()),
            Some(42),
        );
        let (_, page_peer, _, _) = core.downlink_request(&page, local).unwrap();
        assert_eq!(page_peer, second_peer);

        let assignment = AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 0x01,
                channel_rate_and_type: 0x08,
                coding: 0x05,
            },
            circuit_identity_code: None,
            encryption_information: None,
            service_option: Some(ServiceOption(6)),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let assignment_for = |call_id| {
            EncodedA1Message::from_message_for_call(
                &Message::new(MessageType::AssignmentRequest, assignment.encode().unwrap()),
                Some(call_id),
            )
        };
        let (_, assign_peer, _, _) = core.downlink_request(&assignment_for(42), local).unwrap();
        assert_eq!(assign_peer, second_peer);

        assert!(matches!(
            core.downlink_request(&assignment_for(99), local),
            Err(crate::fcs::FcsError::NoRegisteredFap)
        ));
    }

    #[test]
    fn deregistering_one_fap_preserves_other_routes() {
        let mut sip_config = config(false);
        sip_config.users[0].feid = Some("fap-a".to_string());
        let mut second = sip_config.users[0].clone();
        second.impu = "sip:fap-b@femto.local.1xbts.org".to_string();
        second.impi = "fap-b".to_string();
        second.feid = Some("fap-b".to_string());
        sip_config.users.push(second);
        let core = ImsCore::new(&sip_config);
        let first_peer = "10.55.100.1:5060".parse().unwrap();
        let second_peer = "10.55.100.2:5060".parse().unwrap();
        let mut first = register_request();
        first.headers.set("To", "<sip:fap@femto.local.1xbts.org>");
        let mut second = register_request();
        second
            .headers
            .set("To", "<sip:fap-b@femto.local.1xbts.org>");
        core.remember_registered_fap(&first, first_peer);
        core.remember_registered_fap(&second, second_peer);

        first.headers.set("Expires", "0");
        core.remember_registered_fap(&first, first_peer);

        let routes = core.registered_faps.lock().unwrap();
        assert!(!routes.contains_key("fap-a"));
        assert_eq!(
            routes.get("fap-b").map(|route| route.peer),
            Some(second_peer)
        );
    }

    #[test]
    fn sip_digest_identity_cannot_register_another_fap() {
        let mut sip_config = config(false);
        sip_config.users[0].feid = Some("fap-a".to_string());
        let mut second = sip_config.users[0].clone();
        second.impu = "sip:fap-b@femto.local.1xbts.org".to_string();
        second.impi = "fap-b".to_string();
        second.feid = Some("fap-b".to_string());
        sip_config.users.push(second);
        let core = ImsCore::new(&sip_config);
        let mut request = register_with_username("fap");
        request
            .headers
            .set("To", "<sip:fap-b@femto.local.1xbts.org>");

        assert_eq!(core.handle_request(&request).status, SC_FORBIDDEN);
    }

    #[test]
    fn secondary_assignment_prefers_mobile_terminated_dialog() {
        assert_eq!(
            select_voice_assignment_dialog(true, true),
            Some(VoiceAssignmentDialog::MobileTerminated)
        );
        assert_eq!(
            select_voice_assignment_dialog(false, true),
            Some(VoiceAssignmentDialog::MobileOriginated)
        );
    }

    const REFERENCE_FEMTO: &[u8] = b"08 (=\"0 Location Updating Request\")\r\n\
0D=DQaWMRWJCPA=\r\n\
12=EgzJAOYAAAAAAQMBAAY=\r\n\
1F=HwE=\r\n\
0D=DQUFKa4zmQ==\r\n\
35=NQE=\r\n\
42=QgQBATjd\r\n\
28=KP0=\r\n\
40=QAA=\r\n\
41=QQUBAAAA/Q==\r\n\
0D=DQjxAAAAAAAA8A==\r\n";

    pub(super) fn config(allow_unauth: bool) -> SipConfig {
        SipConfig {
            listen: "127.0.0.1:0".to_string(),
            realm: "femto.local.1xbts.org".to_string(),
            allow_unauthenticated: allow_unauth,
            users: vec![crate::config::SipUser {
                impu: "sip:fap@femto.local.1xbts.org".to_string(),
                impi: "fap@femto.local.1xbts.org".to_string(),
                password: "s3cret".to_string(),
                feid: None,
            }],
        }
    }

    #[test]
    fn reload_swaps_registrar_users_without_restart() {
        let core = ImsCore::new(&config(false));
        let new_impi = "0005B9163461@femto.local.1xbts.org";
        assert!(core.user_for_impi(new_impi).is_none());

        let mut cfg = config(false);
        cfg.users.push(crate::config::SipUser {
            impu: "sip:0005B9163461@femto.local.1xbts.org".to_string(),
            impi: new_impi.to_string(),
            password: "newsecret".to_string(),
            feid: Some("0005B9163461".to_string()),
        });
        *core.sip_users_handle().write().expect("sip users lock") = build_ims_users(&cfg);

        let user = core
            .user_for_impi(new_impi)
            .expect("new IMPI resolves after reload swap");
        assert_eq!(user.password, "newsecret");
        assert_eq!(user.feid, "0005B9163461");
        assert!(core.user_for_impi("fap@femto.local.1xbts.org").is_some());
    }

    fn config_with_impi(impi: &str) -> SipConfig {
        let mut config = config(false);
        config.users[0].impi = impi.to_string();
        config
    }

    fn airave_core(config: &SipConfig, cell_state: Arc<FemtoCellState>) -> ImsCore {
        airave_core_with_msc(config, cell_state, None)
    }

    fn airave_core_with_msc(
        config: &SipConfig,
        cell_state: Arc<FemtoCellState>,
        msc_a1_tx: Option<mpsc::Sender<EncodedA1Message>>,
    ) -> ImsCore {
        let feids = config.users.iter().map(|user| {
            user.feid
                .clone()
                .unwrap_or_else(|| sip_uri_user(&user.impu).unwrap_or(&user.impi).to_string())
        });
        ImsCore::with_runtime_config_and_faps(
            config,
            &FcsConfig::default(),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            msc_a1_tx,
            cell_state,
            Arc::new(FapRegistry::compatibility(
                feids,
                FapImplementationKind::AirvanaAirave2,
            )),
        )
    }

    fn register_with_username(username: &str) -> SipRequest {
        let req = register_request();
        let auth = cdma_sip::auth::build_authorization(
            username,
            "s3cret",
            "femto.local.1xbts.org",
            SIP_DIGEST_NONCE,
            "REGISTER",
            &req.uri,
            None,
            None,
            None,
        );
        let mut authed = register_request();
        authed.headers.append("Authorization", &auth);
        authed
    }

    fn attach_mobile(core: &ImsCore, imsi: &str, fap_id: &str) {
        core.registered_mobiles
            .lock()
            .expect("registered mobiles lock")
            .insert(
                imsi.to_string(),
                RegisteredMobile {
                    imsi: MobileIdentity::Imsi(imsi.to_string()),
                    esn: None,
                    meid: None,
                    fap_id: fap_id.to_string(),
                },
            );
    }

    fn register_request() -> SipRequest {
        let mut req = SipRequest::new(Method::Register, "sip:femto.local.1xbts.org");
        req.headers
            .append("Via", "SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bKabc");
        req.headers
            .append("From", "<sip:fap@femto.local.1xbts.org>;tag=1");
        req.headers.append("To", "<sip:fap@femto.local.1xbts.org>");
        req.headers.append("Call-ID", "callid-1");
        req.headers.append("CSeq", "1 REGISTER");
        req.headers.append("Contact", "<sip:fap@10.0.0.2:5060>");
        req.headers.append("Expires", "3600");
        req
    }

    #[test]
    fn correlated_imsi_reads_registration_sessions_and_returns_the_full_imsi() {
        let core = ImsCore::new(&config(false));
        core.registration_sessions
            .lock()
            .expect("registration sessions lock")
            .insert(
                42,
                RegistrationSession {
                    mobile_identity: "9135198800".to_string(),
                    response_uri: None,
                },
            );
        let (fcs, _) = core.fcs_for_fap(None);

        assert_eq!(
            core.correlated_imsi_for_call(fcs, 42).as_deref(),
            Some("310009135198800")
        );
        assert_eq!(core.correlated_imsi_for_call(fcs, 43), None);
    }

    #[test]
    fn correlated_imsi_lookup_does_not_consume_the_registration_session() {
        let core = ImsCore::new(&config(false));
        core.registration_sessions
            .lock()
            .expect("registration sessions lock")
            .insert(
                7,
                RegistrationSession {
                    mobile_identity: "9135198800".to_string(),
                    response_uri: None,
                },
            );
        let (fcs, _) = core.fcs_for_fap(None);

        assert!(core.correlated_imsi_for_call(fcs, 7).is_some());
        assert!(
            core.correlated_imsi_for_call(fcs, 7).is_some(),
            "lookup must be repeatable"
        );
        assert!(
            core.registration_sessions
                .lock()
                .expect("registration sessions lock")
                .contains_key(&7),
            "session must survive for the Accept handler to remove"
        );
    }

    #[test]
    fn call_mobile_identities_takes_precedence_and_is_also_expanded() {
        let core = ImsCore::new(&config(false));
        core.call_mobile_identities
            .lock()
            .expect("call mobile identities lock")
            .insert(9, "9135198800".to_string());
        let (fcs, _) = core.fcs_for_fap(None);

        assert_eq!(
            core.correlated_imsi_for_call(fcs, 9).as_deref(),
            Some("310009135198800")
        );
    }

    fn femto_message_request() -> SipRequest {
        let mut request = SipRequest::new(Method::Message, "sip:fcs@femto.local.1xbts.org");
        request
            .headers
            .append("Via", "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKmessage");
        request
            .headers
            .append("From", "<sip:fap@femto.local.1xbts.org>;tag=message");
        request
            .headers
            .append("To", "<sip:fcs@femto.local.1xbts.org>");
        request.headers.append("Call-ID", "access-session-1");
        request.headers.append("CSeq", "1 MESSAGE");
        request
            .headers
            .append("Content-Type", "application/FemtoInterfaceMsg");
        request.body = REFERENCE_FEMTO.to_vec();
        request
    }

    pub(super) fn mo_invite_request() -> SipRequest {
        let classmark = vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ];
        let femto = FemtoInterfaceMsg {
            message_type: cdma_ios::femto_interface_msg::MSG_TYPE_CM_SERVICE_REQUEST,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("CM Service Request".to_string()),
            ies: vec![
                FemtoIe {
                    iei: 0x90,
                    content: vec![0x91],
                },
                FemtoIe::tlv(0x12, &classmark).expect("classmark IE"),
                FemtoIe::tlv(
                    0x0d,
                    &MobileIdentity::Imsi("5551239999".to_string())
                        .encode()
                        .expect("IMSI"),
                )
                .expect("IMSI IE"),
                FemtoIe::tv(0x03, &ServiceOption::EVRC_A.encode()),
            ],
        };
        let multipart = Multipart::new(
            "airave-mo",
            vec![
                MimePart::text("application/FemtoInterfaceMsg", femto.encode()),
                MimePart::text(
                    CONTENT_TYPE_SDP,
                    build_sdp_offer("127.0.0.1".parse().expect("address"), 32000, 3),
                ),
            ],
        );
        let mut request = SipRequest::new(
            Method::Invite,
            "sip:5551239876@fap.airave2.ims.spcsdns.net;user=phone",
        );
        request.headers.append(
            "Via",
            "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKairave-mo",
        );
        request.headers.append(
            "From",
            "5551239999<sip:0005B9AABBCC@fap.airave2.ims.spcsdns.net>;tag=mo",
        );
        request.headers.append(
            "To",
            "<sip:5551239876@fap.airave2.ims.spcsdns.net;user=phone>",
        );
        request.headers.append("Call-ID", "airave-mo-call");
        request.headers.append("CSeq", "1 INVITE");
        request.headers.append("Supported", "100rel,precondition");
        request
            .headers
            .append("Content-Type", "multipart/mixed; boundary=airave-mo");
        request.body = multipart.encode();
        request
    }

    #[test]
    fn unauthenticated_register_is_challenged() {
        let core = ImsCore::new(&config(false));
        let resp = core.handle_request(&register_request());
        assert_eq!(resp.status, 401);
        assert!(resp.headers.get("WWW-Authenticate").is_some());
    }

    #[test]
    fn empty_digest_register_is_challenged() {
        let core = ImsCore::new(&config(false));
        let mut req = register_request();
        req.headers.append(
            "Authorization",
            "Digest username=\"fap@femto.local.1xbts.org\",realm=\"femto.local.1xbts.org\",nonce=\"\",uri=\"sip:femto.local.1xbts.org\",response=\"\",algorithm=MD5",
        );
        let resp = core.handle_request(&req);
        assert_eq!(resp.status, 401);
        assert!(resp.headers.get("WWW-Authenticate").is_some());
    }

    #[test]
    fn register_with_valid_digest_is_accepted_and_bound() {
        let core = ImsCore::new(&config(false));
        let resp = core.handle_request(&register_with_username("fap@femto.local.1xbts.org"));
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn access_mobile_uses_canonical_imsi_and_keeps_fap_aliases() {
        let cell_state = FemtoCellState::untracked();
        let mut sip_config = config(false);
        sip_config.users[0].feid = Some("test".to_string());
        let core = airave_core(&sip_config, cell_state.clone());
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        let request = mo_invite_request();
        let message = request_femto_message(&request).expect("CM Service Request");

        core.call_id_for_message(&request, &message, Some(peer))
            .expect("mobile identity")
            .expect("call ID");

        let mobiles = cell_state.mobiles();
        assert_eq!(mobiles.len(), 1);
        assert_eq!(mobiles[0].imsi, "310005551239999");
        let registered = core
            .registered_mobiles
            .lock()
            .expect("registered mobiles lock");
        assert_eq!(
            registered.get("5551239999").map(|mobile| &mobile.imsi),
            Some(&MobileIdentity::Imsi("310005551239999".to_string()))
        );
        assert!(registered.contains_key("310005551239999"));
        assert!(registered.contains_key("0005B9AABBCC"));
        drop(registered);

        let adds_page = AddsPageMessage {
            mobile_identity: MobileIdentity::Imsi("310005551239999".to_string()),
            adds_user_part: AddsUserPart {
                burst_type: SMS_DATA_BURST_TYPE,
                data: vec![0x01],
            },
            tag: Some(Tag(1)),
            slot_cycle_index: None,
        };
        let encoded = EncodedA1Message::from_message(&Message::new(
            MessageType::AddsPage,
            adds_page.encode().expect("ADDS Page"),
        ));
        let (downlink, destination, _, _) = core
            .downlink_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
            .expect("Fx2 downlink");

        assert_eq!(destination, peer);
        assert_eq!(downlink.uri, "sip:5551239999@femto.local.1xbts.org");
        assert_eq!(
            downlink.headers.to(),
            Some("<sip:5551239999@femto.local.1xbts.org>")
        );
    }

    #[test]
    fn paging_response_refreshes_canonical_mobile_binding() {
        let cell_state = FemtoCellState::untracked();
        let mut sip_config = config(false);
        sip_config.users[0].feid = Some("test".to_string());
        let core = airave_core(&sip_config, cell_state.clone());
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        let request = mo_invite_request();
        let classmark = vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ];
        let response = FemtoInterfaceMsg {
            message_type: cdma_ios::femto_interface_msg::MSG_TYPE_PAGING_RESPONSE,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("Paging Response".to_string()),
            ies: vec![
                FemtoIe::tlv(0x12, &classmark).expect("classmark IE"),
                FemtoIe::tlv(
                    0x0d,
                    &MobileIdentity::Imsi("5551239999".to_string())
                        .encode()
                        .expect("IMSI"),
                )
                .expect("IMSI IE"),
            ],
        };

        assert_eq!(
            core.call_id_for_message(&request, &response, Some(peer))
                .expect("paging response"),
            None
        );

        let mobiles = cell_state.mobiles();
        assert_eq!(mobiles.len(), 1);
        assert_eq!(mobiles[0].imsi, "310005551239999");
    }

    #[test]
    fn successful_register_returns_ims_routing_headers() {
        let core = ImsCore::new(&config(false));
        let mut request = register_request();
        let authorization = cdma_sip::auth::build_authorization(
            "fap@femto.local.1xbts.org",
            "s3cret",
            "femto.local.1xbts.org",
            SIP_DIGEST_NONCE,
            "REGISTER",
            &request.uri,
            Some(cdma_sip::auth::QOP_AUTH),
            Some("00000a12ca556a9d185b"),
            Some("00000001"),
        );
        request.headers.append("Authorization", authorization);
        request.headers.append("Route", "<sip:10.4.58.140:5060;lr>");
        request.headers.append(H_PATH, "<sip:fap@10.0.0.2:5060;lr>");
        request
            .headers
            .set("Contact", "fap<sip:fap@10.0.0.2:5060>;vendor=airvana");
        request
            .headers
            .set("To", "fap<sip:fap@femto.local.1xbts.org>");

        let response = core.handle_request(&request);

        assert_eq!(response.status, SC_OK);
        assert_eq!(
            response.headers.get(H_SERVICE_ROUTE),
            Some("<sip:10.4.58.140:5060;lr>")
        );
        assert_eq!(
            response.headers.get(H_PATH),
            Some("<sip:fap@10.0.0.2:5060;lr>")
        );
        assert_eq!(
            response.headers.get(H_P_ASSOCIATED_URI),
            request.headers.to()
        );
        assert_eq!(
            response.headers.contact(),
            Some("fap<sip:fap@10.0.0.2:5060>;vendor=airvana;expires=3600")
        );
        let authentication_info = response
            .headers
            .get(H_AUTHENTICATION_INFO)
            .expect("Digest server proof");
        assert!(authentication_info.contains("qop=auth"));
        assert!(authentication_info.contains("rspauth=\""));
        assert!(authentication_info.contains("nc=00000001"));
    }

    #[tokio::test]
    async fn mobile_originated_invite_completes_reliable_assignment_exchange() {
        let (msc_tx, mut msc_rx) = mpsc::channel(4);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        );
        let invite = mo_invite_request();

        let trying = core.handle_mo_invite(&invite, "198.51.100.1:5060".parse().expect("FAP peer"));

        assert_eq!(trying.status, SC_TRYING);
        let cli3 = msc_rx.try_recv().expect("MO Complete Layer 3 Information");
        assert_eq!(cli3.message_type(), MessageType::CompleteLayer3Information);
        let call_id = cli3.call_id().expect("A1 call ID");
        let assignment = AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 1,
                channel_rate_and_type: 8,
                coding: 5,
            },
            circuit_identity_code: None,
            encryption_information: None,
            service_option: Some(ServiceOption::EVRC_A),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: Some(A2pBearerSessionParams {
                ip_address: "127.0.0.1".parse().expect("MSC address"),
                udp_port: 33000,
            }),
            a2p_bearer_format_params: None,
        };
        let encoded_assignment = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::AssignmentRequest,
                assignment.encode().expect("Assignment Request"),
            ),
            Some(call_id),
        );

        let local = "10.4.58.140:5060".parse().expect("FCS endpoint");
        let (progress, peer) = core
            .mo_assignment_progress(&encoded_assignment, local)
            .expect("183 Session Progress");

        assert_eq!(peer, "198.51.100.1:5060".parse().expect("FAP peer"));
        assert_eq!(progress.status, SC_SESSION_PROGRESS);
        assert_eq!(progress.headers.get("Require"), Some("100rel"));
        assert_eq!(progress.headers.get(H_RSEQ), Some("1"));
        assert_eq!(
            progress.headers.get("Contact"),
            Some("<sip:fcs@10.4.58.140:5060>")
        );
        let femto_assignment = response_femto_message(&progress).expect("femto assignment");
        let assignment = AssignmentRequestMessage::decode(
            &femto_assignment.to_bsmap_wire().expect("BSMAP assignment"),
        )
        .expect("typed assignment");
        assert!(assignment.circuit_identity_code.is_none());
        assert!(assignment.a2p_bearer_session_params.is_none());
        assert!(response_sdp_endpoint(&progress).is_some());

        let complete = AssignmentCompleteMessage {
            channel_number: ChannelNumber(50),
            encryption_information: None,
            service_option: Some(ServiceOption::HIGH_RATE_PACKET_DATA),
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let femto_complete = FemtoInterfaceMsg::from_assignment_complete(&complete)
            .expect("femto Assignment Complete");
        let mut prack = SipRequest::new(Method::Prack, invite.uri.clone());
        prack.headers.append(
            "Via",
            "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKairave-prack",
        );
        prack.headers.append("From", invite.headers.from().unwrap());
        prack.headers.append("To", invite.headers.to().unwrap());
        prack.headers.append("Call-ID", "airave-mo-call");
        prack.headers.append("CSeq", "2 PRACK");
        prack.headers.append("RAck", "1 1 INVITE");
        prack
            .headers
            .append("Content-Type", "application/FemtoInterfaceMsg");
        prack.body = femto_complete.encode();

        let ok = core.handle_mo_prack(&prack, peer);

        assert_eq!(ok.status, SC_OK);
        let forwarded = msc_rx.try_recv().expect("A1 Assignment Complete");
        assert_eq!(forwarded.message_type(), MessageType::AssignmentComplete);
        assert_eq!(forwarded.call_id(), Some(call_id));
        let decoded = forwarded.decode().expect("A1 envelope");
        let complete =
            AssignmentCompleteMessage::decode(&decoded.payload).expect("typed Assignment Complete");
        assert!(complete.a2p_bearer_session_params.is_some());
        assert_eq!(complete.service_option, Some(ServiceOption::EVRC_A));

        let deliver = cdma_ios::AddsDeliverMessage {
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6],
            },
            tag: Some(Tag(0x80000001)),
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::AddsDeliver, deliver.encode().unwrap()),
            Some(call_id),
        );
        let (answer, destination) = core.stage_otasp_answer(&encoded).unwrap().unwrap();
        assert_eq!(answer.status, SC_OK);
        assert_eq!(answer.headers.cseq(), Some((1, "INVITE".to_string())));
        assert_eq!(destination, peer);
        assert!(
            core.pending_otasp_answer
                .lock()
                .unwrap()
                .contains_key(&call_id)
        );
        let mut ack = invite.clone();
        ack.method = Method::Ack;
        ack.headers.set("CSeq", "1 ACK");
        ack.headers.set("To", answer.headers.to().unwrap());
        assert!(
            core.otasp_after_ack(&ack, "198.51.100.2:5060".parse().unwrap())
                .is_none()
        );
        assert!(
            core.pending_otasp_answer
                .lock()
                .unwrap()
                .contains_key(&call_id)
        );
        let released = core.otasp_after_ack(&ack, peer).unwrap();
        let released = &released[0];
        assert_eq!(released.call_id(), Some(call_id));
        assert_eq!(
            cdma_ios::AddsDeliverMessage::decode(&released.decode().unwrap().payload).unwrap(),
            deliver
        );
        assert!(core.otasp_after_ack(&ack, peer).is_none());
        assert!(core.stage_otasp_answer(&encoded).unwrap().is_none());

        let mut bye = invite.clone();
        bye.method = Method::Bye;
        bye.headers.set("CSeq", "3 BYE");
        let bye_response = core.handle_request_from(&bye, peer);
        assert_eq!(bye_response.status, SC_OK);
        let clear = msc_rx.try_recv().expect("A1 Clear Request");
        assert_eq!(clear.message_type(), MessageType::ClearRequest);
        assert_eq!(clear.call_id(), Some(call_id));
        let decoded = clear.decode().expect("A1 envelope");
        let clear = ClearRequestMessage::decode(&decoded.payload).expect("typed Clear Request");
        assert_eq!(clear.cause, cdma_ios::Cause(A1_CAUSE_NORMAL_CALL_CLEARING));

        let mut cancel = invite.clone();
        cancel.method = Method::Cancel;
        cancel.headers.set("CSeq", "4 CANCEL");
        let cancel_response = core.handle_request_from(&cancel, peer);
        assert_eq!(cancel_response.status, SC_OK);
        let clear = msc_rx.try_recv().expect("A1 Clear Request for CANCEL");
        assert_eq!(clear.message_type(), MessageType::ClearRequest);
        assert_eq!(clear.call_id(), Some(call_id));
    }

    #[tokio::test]
    async fn mt_leg_failure_releases_the_caller_over_a1() {
        let (msc_tx, mut msc_rx) = mpsc::channel(8);
        let core = airave_core_with_msc(&config(false), FemtoCellState::untracked(), Some(msc_tx));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        let local: SocketAddr = "10.4.58.140:5060".parse().expect("local");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "310005551239999", "fap");
        let paging = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            tag: Some(Tag(101)),
            cell_identifier_list: None,
            slot_cycle_index: Some(cdma_ios::SlotCycleIndex(2)),
            service_option: Some(ServiceOption::EVRC_A),
            is2000_mobile_capabilities: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::PagingRequest,
                paging.encode().expect("Paging Request"),
            ),
            Some(101),
        );
        let (invite, _, _) = core.mt_invite_request(&encoded, local).expect("MT INVITE");

        let decline = response(&invite, 480, "Temporarily Unavailable");
        let follow_up = core.handle_mt_voice_response(&decline, local);

        assert!(follow_up.iter().any(|(req, _)| req.method == Method::Ack));
        let clear = msc_rx
            .try_recv()
            .expect("A1 Clear Request releasing the caller");
        assert_eq!(clear.message_type(), MessageType::ClearRequest);
        assert_eq!(clear.call_id(), Some(101));
        let decoded = clear.decode().expect("A1 envelope");
        let clear = ClearRequestMessage::decode(&decoded.payload).expect("typed Clear Request");
        assert_eq!(clear.cause, cdma_ios::Cause(A1_CAUSE_NORMAL_CALL_CLEARING));
    }

    #[tokio::test]
    async fn unanswered_caller_cleared_with_final_response_not_bye() {
        let (msc_tx, mut msc_rx) = mpsc::channel(4);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        );
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        let local: SocketAddr = "10.4.58.140:5060".parse().expect("local");
        let invite = mo_invite_request();
        assert_eq!(core.handle_mo_invite(&invite, peer).status, SC_TRYING);
        let call_id = msc_rx
            .try_recv()
            .expect("MO Complete Layer 3 Information")
            .call_id()
            .expect("A1 call ID");

        let cleared = core.clear_dialog_requests(call_id, local);
        assert_eq!(cleared.len(), 1, "one message to the caller's leg");
        match &cleared[0].0 {
            cdma_sip::SipMessage::Response(resp) => {
                assert_eq!(resp.status, 480, "caller gets a final failure response");
            }
            cdma_sip::SipMessage::Request(req) => {
                panic!("expected a final response, got a {:?} request", req.method)
            }
        }
        assert_eq!(cleared[0].1, peer);
    }

    fn published_ladder(publisher: &cdma_events::EventPublisher) -> Vec<(String, i32)> {
        publisher
            .drain_published()
            .into_iter()
            .filter_map(|event| match event.body {
                Some(cdma_events::proto::network_event::Body::A1(a1)) => {
                    Some((a1.message_type_name.clone(), a1.direction))
                }
                _ => None,
            })
            .collect()
    }

    fn uplink() -> i32 {
        cdma_events::proto::A1Direction::Uplink as i32
    }

    fn downlink() -> i32 {
        cdma_events::proto::A1Direction::Downlink as i32
    }

    #[tokio::test]
    async fn mobile_originated_call_publishes_every_a1_message() {
        let (msc_tx, mut msc_rx) = mpsc::channel(8);
        let publisher = cdma_events::EventPublisher::in_memory(64);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        )
        .with_event_publisher(publisher.clone());
        let invite = mo_invite_request();
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("FAP peer");
        let local: SocketAddr = "10.4.58.140:5060".parse().expect("FCS endpoint");

        assert_eq!(core.handle_mo_invite(&invite, peer).status, SC_TRYING);
        let call_id = msc_rx
            .try_recv()
            .expect("MO Complete Layer 3 Information")
            .call_id()
            .expect("A1 call ID");

        let assignment = AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 1,
                channel_rate_and_type: 8,
                coding: 5,
            },
            circuit_identity_code: None,
            encryption_information: None,
            service_option: Some(ServiceOption::EVRC_A),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: Some(A2pBearerSessionParams {
                ip_address: "127.0.0.1".parse().expect("MSC address"),
                udp_port: 33000,
            }),
            a2p_bearer_format_params: None,
        };
        let (msc_downlink_tx, msc_downlink_rx) = mpsc::channel(8);
        msc_downlink_tx
            .try_send(EncodedA1Message::from_message_for_call(
                &Message::new(
                    MessageType::AssignmentRequest,
                    assignment.encode().expect("Assignment Request"),
                ),
                Some(call_id),
            ))
            .expect("queue downlink");
        let mut downlink_rx = Some(msc_downlink_rx);
        let encoded_assignment = core
            .next_downlink(&mut downlink_rx)
            .await
            .expect("downlink Assignment Request");
        core.mo_assignment_progress(&encoded_assignment, local)
            .expect("183 Session Progress");

        let complete = AssignmentCompleteMessage {
            channel_number: ChannelNumber(50),
            encryption_information: None,
            service_option: Some(ServiceOption::EVRC_A),
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let femto_complete = FemtoInterfaceMsg::from_assignment_complete(&complete)
            .expect("femto Assignment Complete");
        let mut prack = SipRequest::new(Method::Prack, invite.uri.clone());
        prack.headers.append(
            "Via",
            "SIP/2.0/UDP 198.51.100.1:5060;branch=z9hG4bKairave-prack",
        );
        prack.headers.append("From", invite.headers.from().unwrap());
        prack.headers.append("To", invite.headers.to().unwrap());
        prack.headers.append("Call-ID", "airave-mo-call");
        prack.headers.append("CSeq", "2 PRACK");
        prack.headers.append("RAck", "1 1 INVITE");
        prack
            .headers
            .append("Content-Type", "application/FemtoInterfaceMsg");
        prack.body = femto_complete.encode();
        assert_eq!(core.handle_mo_prack(&prack, peer).status, SC_OK);

        let mut bye = invite.clone();
        bye.method = Method::Bye;
        bye.headers.set("CSeq", "3 BYE");
        assert_eq!(core.handle_request_from(&bye, peer).status, SC_OK);

        assert_eq!(
            published_ladder(&publisher),
            vec![
                ("CM Service Request".to_string(), uplink()),
                ("Assignment Request".to_string(), downlink()),
                ("Assignment Complete".to_string(), uplink()),
                ("Clear Request".to_string(), uplink()),
            ],
            "every A1 message of a mobile-originated call reaches the log"
        );
    }

    #[tokio::test]
    async fn msc_commanded_clear_publishes_both_halves() {
        let (msc_tx, mut msc_rx) = mpsc::channel(8);
        let publisher = cdma_events::EventPublisher::in_memory(64);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        )
        .with_event_publisher(publisher.clone());

        let command = cdma_ios::ClearCommandMessage {
            cause: cdma_ios::Cause(A1_CAUSE_NORMAL_CALL_CLEARING),
            cause_layer3: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::ClearCommand,
                command.encode().expect("Clear Command"),
            ),
            Some(77),
        );
        core.publish_downlink(&encoded);
        let complete = ClearCompleteMessage {
            power_down_indicator: false,
        };
        core.forward_to_msc(
            "test Clear Complete",
            None,
            EncodedA1Message::from_message_for_call(
                &Message::new(
                    MessageType::ClearComplete,
                    complete.encode().expect("Clear Complete"),
                ),
                Some(77),
            ),
            UplinkObservation::A1Wire,
            core.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                None,
                Some(77),
                None,
                None,
            ),
        );

        assert_eq!(
            published_ladder(&publisher),
            vec![
                ("Clear Command".to_string(), downlink()),
                ("Clear Complete".to_string(), uplink()),
            ],
            "a clear the FCS did not start is still fully logged"
        );
        assert_eq!(
            msc_rx.try_recv().expect("A1 Clear Complete").message_type(),
            MessageType::ClearComplete
        );
    }

    /// The Airvana sends Assignment Complete with no discriminator character,
    /// and 0x02 alone is ambiguous: BSMAP Assignment Complete or DTAP Location
    /// Updating Accept.
    #[test]
    fn legacy_assignment_complete_is_named_rather_than_logged_as_a_code_point() {
        let femto = FemtoInterfaceMsg::parse(b"02\r\n23=IwAA\r\n03=AwAD\r\n")
            .expect("legacy Assignment Complete parses");
        assert_eq!(femto.discriminator, None, "the FAP omits the discriminator");

        let observed = observed_body(&femto);

        assert_eq!(observed.discriminator, Some(Discriminator::Bsmap));
        let event =
            crate::event::a1_signaling_event(&observed, &crate::event::A1Context::default());
        assert_eq!(event.message_type_name, "Assignment Complete");
        assert_eq!(
            event.message_type,
            cdma_events::proto::A1MessageType::AssignmentComplete as i32
        );
        assert_eq!(observed.message_type, femto.message_type);
        assert_eq!(observed.ies, femto.ies);
    }

    #[tokio::test]
    async fn a1_only_messages_publish_their_full_layer3_sdu() {
        let publisher = cdma_events::EventPublisher::in_memory(8);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            None,
        )
        .with_event_publisher(publisher.clone());

        let payload = cdma_ios::ConnectMessage.encode().expect("Connect");
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::Connect, payload.clone()),
            Some(5),
        );
        core.publish_a1_wire(
            None,
            &encoded,
            core.a1_context(
                cdma_events::proto::A1Direction::Uplink,
                None,
                Some(5),
                None,
                None,
            ),
        );

        let published = publisher.drain_published();
        let a1 = match published.first().and_then(|event| event.body.as_ref()) {
            Some(cdma_events::proto::network_event::Body::A1(a1)) => a1,
            other => panic!("expected an A1 body, got {other:?}"),
        };
        assert_eq!(a1.message_type_name, "Connect");
        // A.S0001 Table 3.1.2-1: Connect is DTAP 0x07.
        assert_eq!(a1.message_type_value, 0x07);
        assert_eq!(
            a1.discriminator,
            cdma_events::proto::A1MessageDiscriminator::Dtap as i32
        );
        // The A1 link header is transport framing. The SDU is everything from
        // the protocol discriminator on, exactly as the encoder wrote it.
        assert_eq!(a1.sdu, payload[3..]);
        assert!(
            a1.information_elements.is_empty(),
            "no element split is claimed for a message read off the wire"
        );
    }

    #[tokio::test]
    async fn mobile_to_mobile_invite_preserves_originating_codec_offer() {
        let (msc_tx, mut msc_rx) = mpsc::channel(4);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        );
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "310005551239876", "fap");
        let mut mo_invite = mo_invite_request();
        let femto = request_femto_message(&mo_invite).expect("MO FemtoInterfaceMsg");
        let originating_sdp = b"v=0\r\no=HubBub 256 2 IN IP4 198.51.100.1\r\ns=Airave\r\nc=IN IP4 198.51.100.1\r\nt=0 0\r\nm=audio 5102 RTP/AVP 100 97\r\na=rtpmap:100 EVRC0/800\r\na=fmtp:100 maxinterleave=0\r\na=ptime:20\r\na=rtpmap:97 telephone-event/8000\r\n";
        mo_invite.body = Multipart::new(
            "airave-complete-offer",
            vec![
                MimePart::text(CONTENT_TYPE_FEMTO, femto.encode()),
                MimePart::text(CONTENT_TYPE_SDP, originating_sdp.to_vec()),
            ],
        )
        .encode();
        mo_invite.headers.set(
            "Content-Type",
            "multipart/mixed; boundary=airave-complete-offer",
        );

        let trying = core.handle_mo_invite(&mo_invite, peer);
        assert_eq!(trying.status, SC_TRYING);
        let call_id = msc_rx
            .try_recv()
            .expect("MO Complete Layer 3 Information")
            .call_id()
            .expect("call ID");
        let paging = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239876".to_string()),
            tag: Some(Tag(101)),
            cell_identifier_list: None,
            slot_cycle_index: Some(cdma_ios::SlotCycleIndex(2)),
            service_option: Some(ServiceOption::EVRC_A),
            is2000_mobile_capabilities: None,
        };
        let paging = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::PagingRequest,
                paging.encode().expect("Paging Request"),
            ),
            Some(call_id),
        );

        let (mt_invite, destination, _) = core
            .mt_invite_request(&paging, "10.4.58.140:5060".parse().expect("local"))
            .expect("MT INVITE");
        let mt_sdp = request_sdp_body(&mt_invite).expect("MT SDP");
        let mt_sdp = String::from_utf8(mt_sdp).expect("UTF-8 SDP");

        assert_eq!(destination, peer);
        let media = mt_sdp
            .lines()
            .find(|line| line.starts_with("m=audio "))
            .expect("audio media line");
        let mut media_fields = media.split_ascii_whitespace();
        assert_eq!(media_fields.next(), Some("m=audio"));
        let media_port = media_fields
            .next()
            .expect("audio media port")
            .parse::<u16>()
            .expect("numeric audio media port");
        assert!((18_000..=18_199).contains(&media_port));
        assert_eq!(media_fields.collect::<Vec<_>>(), ["RTP/AVP", "100", "97"]);
        assert!(mt_sdp.contains("a=rtpmap:100 EVRC0/800\r\n"));
        assert!(mt_sdp.contains("a=fmtp:100 maxinterleave=0\r\n"));
        assert!(mt_sdp.contains("a=ptime:20\r\n"));
        assert!(mt_sdp.contains("a=rtpmap:97 telephone-event/8000\r\n"));
    }

    #[tokio::test]
    async fn mobile_terminated_invite_addresses_called_mobile_in_to_uri() {
        let core = ImsCore::new(&config(false));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "310005551239999", "fap");
        let paging = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            tag: Some(Tag(101)),
            cell_identifier_list: None,
            slot_cycle_index: Some(cdma_ios::SlotCycleIndex(2)),
            service_option: Some(ServiceOption::EVRC_A),
            is2000_mobile_capabilities: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::PagingRequest,
                paging.encode().expect("Paging Request"),
            ),
            Some(101),
        );

        let (request, destination, _) = core
            .mt_invite_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
            .expect("MT INVITE");

        assert_eq!(destination, peer);
        assert_eq!(request.uri, "sip:fap@femto.local.1xbts.org");
        assert_eq!(
            request.headers.to(),
            Some("<sip:5551239999@femto.local.1xbts.org>")
        );
    }

    #[tokio::test]
    async fn mobile_terminated_dialog_pracks_each_reliable_provisional_response_once() {
        let core = airave_core(&config(false), FemtoCellState::untracked());
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        let local: SocketAddr = "10.4.58.140:5060".parse().expect("local");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "310005551239999", "fap");
        let paging = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            tag: Some(Tag(101)),
            cell_identifier_list: None,
            slot_cycle_index: Some(cdma_ios::SlotCycleIndex(2)),
            service_option: Some(ServiceOption::EVRC_A),
            is2000_mobile_capabilities: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::PagingRequest,
                paging.encode().expect("Paging Request"),
            ),
            Some(101),
        );
        let (invite, _, _) = core.mt_invite_request(&encoded, local).expect("MT INVITE");
        let mut progress = response(&invite, SC_SESSION_PROGRESS, "Session Progress");
        progress.headers.append(H_RSEQ, "2");
        assert!(core.handle_mt_voice_response(&progress, local).is_empty());

        let assignment = AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 1,
                channel_rate_and_type: 8,
                coding: 5,
            },
            circuit_identity_code: None,
            encryption_information: None,
            service_option: Some(ServiceOption::EVRC_A),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let assignment = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::AssignmentRequest,
                assignment.encode().expect("Assignment Request"),
            ),
            Some(101),
        );
        let (first_prack, _) = core
            .mt_prack_request(&assignment, local)
            .expect("first PRACK");
        assert_eq!(first_prack.headers.cseq(), Some((2, "PRACK".to_string())));
        assert_eq!(first_prack.headers.get(H_RACK), Some("2 1 INVITE"));
        assert_eq!(
            first_prack.headers.content_type(),
            Some(crate::fap::AIRVANA_FEMTO_CONTENT_TYPE)
        );

        let mut ringing = response(&invite, SC_RINGING, "Ringing");
        ringing.headers.append(H_RSEQ, "3");
        let follow_up = core.handle_mt_voice_response(&ringing, local);
        assert_eq!(follow_up.len(), 1);
        assert_eq!(follow_up[0].1, peer);
        assert_eq!(
            follow_up[0].0.headers.cseq(),
            Some((3, "PRACK".to_string()))
        );
        assert_eq!(follow_up[0].0.headers.get(H_RACK), Some("3 1 INVITE"));
        assert!(follow_up[0].0.body.is_empty());

        assert!(core.handle_mt_voice_response(&ringing, local).is_empty());
    }

    #[tokio::test]
    async fn mobile_terminated_call_publishes_every_a1_message() {
        let (msc_tx, mut msc_rx) = mpsc::channel(8);
        let publisher = cdma_events::EventPublisher::in_memory(64);
        let core = airave_core_with_msc(&config(false), FemtoCellState::untracked(), Some(msc_tx))
            .with_event_publisher(publisher.clone());
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        let local: SocketAddr = "10.4.58.140:5060".parse().expect("local");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "310005551239999", "fap");

        let paging = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            tag: Some(Tag(101)),
            cell_identifier_list: None,
            slot_cycle_index: Some(cdma_ios::SlotCycleIndex(2)),
            service_option: Some(ServiceOption::EVRC_A),
            is2000_mobile_capabilities: None,
        };
        let assignment = AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 1,
                channel_rate_and_type: 8,
                coding: 5,
            },
            circuit_identity_code: None,
            encryption_information: None,
            service_option: Some(ServiceOption::EVRC_A),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let (downlink_tx, downlink_rx) = mpsc::channel(8);
        for message in [
            Message::new(
                MessageType::PagingRequest,
                paging.encode().expect("Paging Request"),
            ),
            Message::new(
                MessageType::AssignmentRequest,
                assignment.encode().expect("Assignment Request"),
            ),
        ] {
            downlink_tx
                .try_send(EncodedA1Message::from_message_for_call(&message, Some(101)))
                .expect("queue downlink");
        }
        let mut downlink_rx = Some(downlink_rx);

        let encoded_paging = core.next_downlink(&mut downlink_rx).await.expect("page");
        let (invite, _, _) = core
            .mt_invite_request(&encoded_paging, local)
            .expect("MT INVITE");
        let mut progress = response(&invite, SC_SESSION_PROGRESS, "Session Progress");
        progress.headers.append(H_RSEQ, "2");
        assert!(core.handle_mt_voice_response(&progress, local).is_empty());

        let encoded_assignment = core
            .next_downlink(&mut downlink_rx)
            .await
            .expect("assignment");
        core.mt_prack_request(&encoded_assignment, local)
            .expect("MT PRACK");

        let mut prack_ok = response(&invite, SC_OK, "OK");
        prack_ok
            .headers
            .set("CSeq", format!("{SIP_FIRST_PRACK_CSEQ} PRACK"));
        core.handle_mt_voice_response(&prack_ok, local);
        let mut answer = response(&invite, SC_OK, "OK");
        answer
            .headers
            .set("CSeq", format!("{SIP_INVITE_CSEQ} INVITE"));
        core.handle_mt_voice_response(&answer, local);

        assert_eq!(
            published_ladder(&publisher),
            vec![
                ("Paging Request".to_string(), downlink()),
                ("Assignment Request".to_string(), downlink()),
                ("Assignment Complete".to_string(), uplink()),
                ("Connect".to_string(), uplink()),
            ],
            "every A1 message of a mobile-terminated call reaches the log"
        );
        assert_eq!(
            msc_rx
                .try_recv()
                .expect("A1 Assignment Complete")
                .message_type(),
            MessageType::AssignmentComplete
        );
        assert_eq!(
            msc_rx.try_recv().expect("A1 Connect").message_type(),
            MessageType::Connect
        );
    }

    #[test]
    fn trusted_fap_traffic_recovers_downlink_route_after_restart() {
        let core = ImsCore::new(&config(false));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");

        core.recover_registered_fap_peer(peer);

        assert_eq!(
            core.registered_faps
                .lock()
                .expect("registered FAPs lock")
                .get("fap")
                .cloned(),
            Some(RegisteredFap {
                fap_id: "fap".to_string(),
                public_identity: "sip:fap@femto.local.1xbts.org".to_string(),
                peer,
            })
        );
    }

    #[test]
    fn persisted_peer_attributes_traffic_before_registration_with_multiple_faps() {
        let cell_state = FemtoCellState::untracked();
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        cell_state.observe_inner_peer("test", peer.ip());
        let mut sip_config = config(false);
        sip_config.users[0].feid = Some("test".to_string());
        sip_config.users.push(crate::config::SipUser {
            impu: "sip:second@femto.local.1xbts.org".to_string(),
            impi: "second@femto.local.1xbts.org".to_string(),
            password: "other-secret".to_string(),
            feid: Some("second".to_string()),
        });
        let core = airave_core(&sip_config, cell_state);

        core.recover_registered_fap_peer(peer);

        assert_eq!(core.fap_id_for_peer(peer).as_deref(), Some("test"));
        assert_eq!(
            core.registered_faps
                .lock()
                .expect("registered FAPs lock")
                .get("test")
                .map(|route| route.peer),
            Some(peer)
        );
        assert_eq!(core.cell_state.cells()[0].sip_peer, Some(peer));
    }

    #[test]
    fn authenticated_acs_hint_recovers_downlink_route_after_restart() {
        let core = ImsCore::new(&config(false));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");

        core.recover_registered_fap_hint(FapRouteHint {
            serial_number: "fap".to_string(),
            peer,
        });

        assert_eq!(
            core.registered_faps
                .lock()
                .expect("registered FAPs lock")
                .get("fap")
                .cloned(),
            Some(RegisteredFap {
                fap_id: "fap".to_string(),
                public_identity: "sip:fap@femto.local.1xbts.org".to_string(),
                peer,
            })
        );
    }

    #[test]
    fn qualified_digest_impi_matches_configured_bare_impi() {
        let core = ImsCore::new(&config_with_impi("fap"));
        let resp = core.handle_request(&register_with_username("fap@femto.local.1xbts.org"));
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn bare_digest_impi_matches_configured_qualified_impi() {
        let core = ImsCore::new(&config(false));
        let resp = core.handle_request(&register_with_username("fap"));
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn qualified_digest_impi_from_another_realm_is_forbidden() {
        let core = ImsCore::new(&config_with_impi("fap"));
        let resp = core.handle_request(&register_with_username("fap@another.example"));
        assert_eq!(resp.status, 403);
    }

    #[test]
    fn register_with_wrong_password_is_forbidden() {
        let core = ImsCore::new(&config(false));
        let req = register_request();
        let auth = cdma_sip::auth::build_authorization(
            "fap@femto.local.1xbts.org",
            "wrong",
            "femto.local.1xbts.org",
            SIP_DIGEST_NONCE,
            "REGISTER",
            &req.uri,
            None,
            None,
            None,
        );
        let mut authed = register_request();
        authed.headers.append("Authorization", &auth);
        assert_eq!(core.handle_request(&authed).status, 403);
    }

    #[test]
    fn learned_fap_password_overrides_static_fallback() {
        let core = ImsCore::new(&config(false));
        core.learned_passwords
            .write()
            .expect("learned SIP password lock")
            .insert("fap".to_string(), "device-secret".to_string());
        let req = register_request();
        let auth = cdma_sip::auth::build_authorization(
            "fap@femto.local.1xbts.org",
            "device-secret",
            "femto.local.1xbts.org",
            SIP_DIGEST_NONCE,
            "REGISTER",
            &req.uri,
            None,
            None,
            None,
        );
        let mut authed = register_request();
        authed.headers.append("Authorization", &auth);
        assert_eq!(core.handle_request(&authed).status, 200);
    }

    #[test]
    fn femto_a1_message_is_forwarded_as_typed_complete_layer_three() {
        let (msc_tx, mut msc_rx) = mpsc::channel(1);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        );

        let response = core.handle_request(&femto_message_request());

        assert_eq!(response.status, SC_OK);
        let forwarded = msc_rx.try_recv().expect("forwarded A1 message");
        assert_eq!(
            forwarded.message_type(),
            MessageType::CompleteLayer3Information
        );
        assert_eq!(forwarded.call_id(), Some(1));
        let decoded = forwarded.decode().expect("A1 envelope");
        let complete_layer3 =
            CompleteLayer3InformationMessage::decode(&decoded.payload).expect("typed CLI3");
        assert_eq!(complete_layer3.cell_identifier.cell, 0x193);
        assert_eq!(
            complete_layer3
                .layer3_information
                .decode_location_updating_request()
                .expect("location update")
                .mobile_identity_imsi,
            cdma_ios::MobileIdentity::Imsi("310009135198800".to_string())
        );
    }

    #[test]
    fn femto_a1_message_is_rejected_when_msc_forwarding_is_unavailable() {
        let core = ImsCore::new(&config(false));

        let response = core.handle_request(&femto_message_request());

        assert_eq!(response.status, SC_SERVICE_UNAVAILABLE);
    }

    #[test]
    fn generic_core_builds_standard_fx2_downlink() {
        let core = ImsCore::new(&config(false));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "9135198800", "fap");
        core.registration_sessions
            .lock()
            .expect("registration sessions lock")
            .insert(
                7,
                RegistrationSession {
                    mobile_identity: "9135198800".to_string(),
                    response_uri: None,
                },
            );
        let encoded = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                MessageType::LocationUpdatingAccept,
                cdma_ios::LocationUpdatingAcceptMessage {
                    location_area_identification: None,
                }
                .encode()
                .expect("Location Updating Accept"),
            ),
            Some(7),
        );

        let (request, _, _, _) = core
            .downlink_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
            .expect("Fx2 downlink");

        assert_eq!(
            request.headers.content_type(),
            Some(crate::fap::STANDARD_FEMTO_CONTENT_TYPE)
        );
        assert_eq!(
            core.media_packet_format(None),
            MediaPacketFormat::StandardRtp
        );
    }

    #[test]
    fn builds_airvana_fx2_downlink_for_registered_fap() {
        let core = airave_core(&config(false), FemtoCellState::untracked());
        core.next_sip_transaction.store(u64::MAX, Ordering::Relaxed);
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "9135198800", "fap");
        core.registration_sessions
            .lock()
            .expect("registration sessions lock")
            .insert(
                7,
                RegistrationSession {
                    mobile_identity: "9135198800".to_string(),
                    response_uri: None,
                },
            );
        let accept = cdma_ios::LocationUpdatingAcceptMessage {
            location_area_identification: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                MessageType::LocationUpdatingAccept,
                accept.encode().expect("Location Updating Accept"),
            ),
            Some(7),
        );

        let (request, destination, _, _) = core
            .downlink_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
            .expect("Fx2 downlink");

        assert_eq!(destination, peer);
        assert_eq!(request.method, Method::Message);
        assert_eq!(request.uri, "sip:fap@femto.local.1xbts.org");
        assert!(
            request.headers.cseq().is_some_and(
                |(sequence, method)| sequence <= i32::MAX as u32 && method == "MESSAGE"
            )
        );
        assert_eq!(
            request.headers.content_type(),
            Some("application/FemtoInterfaceMsg")
        );
        assert!(
            request.headers.via().is_some_and(|via| {
                via.starts_with("SIP/2.0/UDP 10.4.58.140:5060;branch=z9hG4bK")
            })
        );
        assert_eq!(
            request.headers.contact(),
            Some("<sip:fcs@femto.local.1xbts.org>")
        );
        assert_eq!(
            request.headers.to(),
            Some("<sip:9135198800@femto.local.1xbts.org>")
        );
    }

    #[test]
    fn assignment_request_targets_originating_mobile_on_new_dialog() {
        let core = ImsCore::new(&config(false));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "5551239999", "fap");
        let mut uplink = femto_message_request();
        uplink.headers.set(
            "From",
            "<sip:5551239999@femto.local.1xbts.org>;tag=airave-call",
        );
        uplink.headers.set("To", "<sip:fcs@femto.local.1xbts.org>");
        uplink.headers.set("Call-ID", "airave-access-call-1");
        let a1_call_id = core.call_id_for(&uplink, None);
        core.call_mobile_identities
            .lock()
            .expect("call mobile identities lock")
            .insert(a1_call_id, "5551239999".to_string());
        let assignment = AssignmentRequestMessage {
            channel_type: ChannelType {
                speech_or_data_indicator: 0x01,
                channel_rate_and_type: 0x08,
                coding: 0x05,
            },
            circuit_identity_code: None,
            encryption_information: None,
            service_option: Some(ServiceOption(6)),
            signals: Vec::new(),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::AssignmentRequest,
                assignment.encode().expect("Assignment Request"),
            ),
            Some(a1_call_id),
        );

        let (request, destination, call_id, _) = core
            .downlink_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
            .expect("Fx2 downlink");

        assert_eq!(destination, peer);
        assert_ne!(call_id, "airave-access-call-1");
        assert_eq!(request.headers.call_id(), Some(call_id.as_str()));
        assert!(
            request.headers.from().is_some_and(|from| {
                from.starts_with("<sip:fcs@femto.local.1xbts.org>;tag=1xbts")
            })
        );
        assert_eq!(
            request.headers.to(),
            Some("<sip:5551239999@femto.local.1xbts.org>")
        );
    }

    #[test]
    fn location_update_accept_targets_originating_mobile_uri_when_available() {
        let core = ImsCore::new(&config(false));
        let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
        core.remember_registered_fap(&register_request(), peer);
        attach_mobile(&core, "9135198800", "fap");
        core.registration_sessions
            .lock()
            .expect("registration sessions lock")
            .insert(
                7,
                RegistrationSession {
                    mobile_identity: "9135198800".to_string(),
                    response_uri: Some("sip:9135198800@femto.local.1xbts.org".to_string()),
                },
            );
        let accept = cdma_ios::LocationUpdatingAcceptMessage {
            location_area_identification: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                MessageType::LocationUpdatingAccept,
                accept.encode().expect("Location Updating Accept"),
            ),
            Some(7),
        );

        let (request, _, _, _) = core
            .downlink_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
            .expect("Fx2 downlink");

        assert_eq!(
            request.headers.to(),
            Some("<sip:9135198800@femto.local.1xbts.org>")
        );
    }

    #[test]
    fn airvana_empty_message_acknowledges_adds_page_before_sip_response() {
        for known_fap in ["fap", "different-fap"] {
            let (msc_tx, mut msc_rx) = mpsc::channel(1);
            let core = ImsCore::with_msc_a1(
                &config(false),
                CellId {
                    cell: 0x193,
                    sector: 0,
                },
                Some(msc_tx),
            );
            let peer: SocketAddr = "198.51.100.1:5060".parse().expect("peer");
            core.remember_registered_fap(&register_request(), peer);
            attach_mobile(&core, "310005551239876", "fap");
            let hardware_esn = MobileIdentity::Esn(0x80b6_6bd7);
            let hardware_meid = MobileIdentity::Meid([0xa0, 0x00, 0x00, 0x2c, 0xc8, 0x1a, 0x35]);
            {
                let mut mobiles = core.registered_mobiles.lock().unwrap();
                let mobile = mobiles.get_mut("310005551239876").unwrap();
                mobile.esn = Some(hardware_esn.clone());
                mobile.meid = Some(hardware_meid.clone());
            }
            let adds_page = AddsPageMessage {
                mobile_identity: MobileIdentity::Imsi("310005551239876".to_string()),
                adds_user_part: AddsUserPart {
                    burst_type: 0x03,
                    data: vec![0x01, 0x02, 0x03],
                },
                tag: Some(Tag(48)),
                slot_cycle_index: None,
            };
            let encoded = EncodedA1Message::from_message(&Message::new(
                MessageType::AddsPage,
                adds_page.encode().expect("ADDS Page"),
            ));
            let (request, _, call_id, pending) = core
                .downlink_request(&encoded, "10.4.58.140:5060".parse().expect("local"))
                .expect("Fx2 downlink");
            assert_eq!(request.uri, "sip:5551239876@femto.local.1xbts.org");
            assert_eq!(
                request.headers.to(),
                Some("<sip:5551239876@femto.local.1xbts.org>")
            );
            core.stage_airave_adds_page_ack(&pending);
            core.pending_downlink
                .lock()
                .expect("pending downlink lock")
                .insert(call_id, pending);

            let mut empty = SipRequest::new(Method::Message, "sip:fap@femto.local.1xbts.org");
            empty
                .headers
                .append("From", "5551239876<sip:5551239876@femto.local.1xbts.org>");
            empty
                .headers
                .append("To", "<sip:fap@femto.local.1xbts.org>");
            empty.headers.append("Call-ID", "airvana-generated-call-id");
            empty.headers.append("CSeq", "1 MESSAGE");

            core.registered_mobiles
                .lock()
                .unwrap()
                .get_mut("310005551239876")
                .unwrap()
                .fap_id = known_fap.to_string();
            let result = core.handle_request(&empty);

            assert_eq!(result.status, SC_OK);
            let forwarded = msc_rx.try_recv().expect("forwarded ADDS Page Ack");
            assert_eq!(forwarded.message_type(), MessageType::AddsPageAck);
            let decoded = forwarded.decode().expect("A1 envelope");
            let ack = AddsPageAckMessage::decode(&decoded.payload).expect("ADDS Page Ack");
            assert_eq!(ack.mobile_identity, adds_page.mobile_identity);
            assert_eq!(ack.tag, adds_page.tag);
            assert_eq!(ack.cause, None);
            let mobiles = core.registered_mobiles.lock().unwrap();
            for alias in ["310005551239876", "5551239876"] {
                let mobile = mobiles.get(alias).unwrap();
                assert_eq!(
                    mobile.esn,
                    (known_fap == "fap").then(|| hardware_esn.clone())
                );
                assert_eq!(
                    mobile.meid,
                    (known_fap == "fap").then(|| hardware_meid.clone())
                );
            }
        }
    }

    #[test]
    fn binary_mobile_originated_sms_is_forwarded_as_adds_transfer() {
        let (msc_tx, mut msc_rx) = mpsc::channel(1);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        );
        core.registered_mobiles
            .lock()
            .expect("registered mobiles lock")
            .insert(
                "5551239876".to_string(),
                RegisteredMobile {
                    imsi: MobileIdentity::Imsi("310005551239876".to_string()),
                    esn: Some(MobileIdentity::Esn(0x80B66BD7)),
                    meid: None,
                    fap_id: "fap".to_string(),
                },
            );
        const REPLY_SEQ: u8 = 17;
        let sms_payload = vec![
            0x00,
            0x00,
            0x02,
            0x10,
            0x02,
            0x06,
            0x01,
            REPLY_SEQ << 2,
            0x08,
            0x00,
        ];
        let mut request = SipRequest::new(Method::Message, "sip:fcs@femto.local.1xbts.org");
        request
            .headers
            .append("From", "5551239876<sip:5551239876@femto.local.1xbts.org>");
        request
            .headers
            .append("To", "<sip:fcs@femto.local.1xbts.org>");
        request.headers.append("Call-ID", "mo-sms-1");
        request.headers.append("CSeq", "1 MESSAGE");
        request
            .headers
            .append("Content-Type", "application/vnd.3gpp2.sms");
        request.body = sms_payload.clone();

        let result = core.handle_request(&request);

        assert_eq!(result.status, SC_OK);
        let forwarded = msc_rx.try_recv().expect("forwarded ADDS Transfer");
        assert_eq!(forwarded.message_type(), MessageType::AddsTransfer);
        let decoded = forwarded.decode().expect("A1 envelope");
        let transfer = AddsTransferMessage::decode(&decoded.payload).expect("ADDS Transfer");
        assert_eq!(
            transfer.mobile_identity_imsi,
            MobileIdentity::Imsi("310005551239876".to_string())
        );
        assert_eq!(
            transfer.mobile_identity_esn,
            Some(MobileIdentity::Esn(0x80B66BD7))
        );
        assert_eq!(transfer.adds_user_part.burst_type, SMS_DATA_BURST_TYPE);
        assert_eq!(transfer.adds_user_part.data, sms_payload);
        assert_eq!(transfer.cell_identifier, Some(core.cell_identifier));
        let cause_code = core
            .sms_cause_code_rx
            .lock()
            .expect("SMS Cause Code receiver lock")
            .as_mut()
            .expect("SMS Cause Code receiver")
            .try_recv()
            .expect("queued SMS Cause Code");
        assert_eq!(cause_code.mobile_identity, "5551239876");
        let decoded =
            cdma_common::sms::decode_mt_sms(&cause_code.body).expect("decoded SMS Cause Code");
        assert_eq!(decoded.tl_msg_type, 0x02);
        assert_eq!(decoded.reply_seq, Some(REPLY_SEQ));
        assert_eq!(decoded.error_class, Some(0));
    }

    #[test]
    fn sms_cause_code_downlink_targets_mobile_with_binary_content() {
        let core = ImsCore::new(&config(false));
        core.registered_faps
            .lock()
            .expect("registered FAPs lock")
            .insert(
                "fap".to_string(),
                RegisteredFap {
                    fap_id: "fap".to_string(),
                    public_identity: "sip:fap@femto.local.1xbts.org".to_string(),
                    peer: "198.51.100.1:5060".parse().expect("FAP peer"),
                },
            );
        core.registered_mobiles
            .lock()
            .expect("registered mobiles lock")
            .insert(
                "5551239876".to_string(),
                RegisteredMobile {
                    imsi: MobileIdentity::Imsi("310005551239876".to_string()),
                    esn: None,
                    meid: None,
                    fap_id: "fap".to_string(),
                },
            );
        let pending = PendingSmsCauseCode {
            mobile_identity: "5551239876".to_string(),
            body: cdma_common::sms::encode_sms_cause_code(9, 0),
        };

        let (request, peer, _) = core
            .sms_cause_code_request(pending.clone(), "10.4.58.140:5060".parse().expect("local"))
            .expect("SMS Cause Code request");

        assert_eq!(peer, "198.51.100.1:5060".parse().expect("FAP peer"));
        assert_eq!(
            request.headers.to(),
            Some("<sip:5551239876@femto.local.1xbts.org>")
        );
        assert_eq!(
            request.headers.content_type(),
            Some("application/vnd.3gpp2.sms")
        );
        assert_eq!(request.body, pending.body);
    }

    #[test]
    fn binary_mobile_originated_sms_uses_imsi_s_before_cm_identity_arrives() {
        let (msc_tx, mut msc_rx) = mpsc::channel(1);
        let core = ImsCore::with_msc_a1(
            &config(false),
            CellId {
                cell: 0x193,
                sector: 0,
            },
            Some(msc_tx),
        );
        let mut request = SipRequest::new(Method::Message, "sip:fcs@femto.local.1xbts.org");
        request
            .headers
            .append("From", "5551239876<sip:5551239876@femto.local.1xbts.org>");
        request
            .headers
            .append("To", "<sip:fcs@femto.local.1xbts.org>");
        request.headers.append("Call-ID", "mo-sms-before-cm-1");
        request.headers.append("CSeq", "1 MESSAGE");
        request
            .headers
            .append("Content-Type", "application/vnd.3gpp2.sms");
        request.body = vec![0x00, 0x02, 0x10, 0x02, 0x00];

        let result = core.handle_request(&request);

        assert_eq!(result.status, SC_OK);
        let forwarded = msc_rx.try_recv().expect("forwarded ADDS Transfer");
        let decoded = forwarded.decode().expect("A1 envelope");
        let transfer = AddsTransferMessage::decode(&decoded.payload).expect("ADDS Transfer");
        assert_eq!(
            transfer.mobile_identity_imsi,
            MobileIdentity::Imsi("5551239876".to_string())
        );
        assert_eq!(transfer.mobile_identity_esn, None);
        assert_eq!(transfer.mobile_identity_meid, None);
    }

    #[test]
    fn learned_sip_credentials_survive_reload() {
        let directory = std::env::temp_dir().join(format!(
            "cdma-femto-sip-credentials-{}-{}",
            std::process::id(),
            sip_transaction_seed()
        ));
        let path = directory.join(SIP_CREDENTIAL_STATE_FILE);
        let passwords = HashMap::from([("fap-a".to_string(), "device-secret".to_string())]);

        persist_learned_sip_passwords_to(&passwords, &path);

        assert_eq!(load_learned_sip_passwords_from(&path), passwords);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
