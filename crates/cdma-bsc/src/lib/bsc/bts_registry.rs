//! Registry of the BTSs this BSC serves, keyed by cell.
//!
//! Radio parameters are BTS-owned: each peer supplies them over the OAM gRPC
//! channel at attach and again on every re-attach, and the BSC stores them
//! here rather than reading a BTS config file. Call control resolves a
//! mobile's serving cell to an entry and reads its parameters and Abis
//! control client from it.

use std::collections::BTreeMap;
use std::sync::Arc;

use log::{info, warn};
use parking_lot::RwLock;

use cdma_common::band_class::ChannelPlan;
use cdma_common::overhead::OverheadParameters;
use cdma_common::timezone::TimezoneConfig;

use crate::abis_edge::BtsControlClient;
use crate::config::BtsPeerConfig;

pub use cdma_common::events::AccessCellId;

/// gRPC OAM client for one BTS peer.
pub type BtsOamClient =
    crate::grpc::bts_management_proto::bts_management_service_client::BtsManagementServiceClient<
        tonic::transport::Channel,
    >;

/// How far a cell has progressed toward carrying traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtsAttachState {
    /// Configured but not reachable.
    Disconnected,
    /// Parameters retrieved, Abis signaling not yet established.
    Enrolled,
    /// Enrolled with Abis signaling up.
    InService,
}

/// Radio parameters one BTS supplies for the cell it operates.
#[derive(Debug, Clone)]
pub struct BtsCellParams {
    pub cell: AccessCellId,
    pub overhead: OverheadParameters,
    /// Pilot PN offset in 64-chip units.
    pub pilot_offset: usize,
    /// Reference dBm for converting relative Rx level to absolute power.
    /// `None` leaves reported Rx levels relative.
    pub rx_reference_dbm: Option<f64>,
    pub channel: ChannelPlan,
    pub tx_center_frequency_hz: usize,
    pub rx_center_frequency_hz: usize,
    /// Where this cell's HRPD access network serves its session API, so an
    /// AN query can be aimed at the cell that owns the session.
    pub an_grpc_addr: Option<String>,
    /// Paging retransmission budget the cell applies. The BSC schedules
    /// against the same values so both agree how long a page is outstanding.
    pub paging_retry: crate::config::PagingRetryConfig,
    /// Resolved EV-DO carrier, when the cell runs one.
    pub evdo: Option<crate::grpc::proto::EvdoCarrierConfig>,
    pub timezone: TimezoneConfig,
    /// MCC broadcast in the Extended System Parameters Message. The paging
    /// path compares against it to pick the shortest general-page record
    /// format that still identifies the mobile (C.S0004-E 3.1.2.2.1.1.1.2).
    pub mcc: u16,
    /// IMSI_11_12 broadcast in the Extended System Parameters Message, used
    /// alongside `mcc` for the same choice.
    pub imsi_11_12: u8,
}

/// MCC wildcard broadcast when no home MCC is configured.
pub const MCC_UNSPECIFIED: u16 = 0x03ff;
/// IMSI_11_12 wildcard broadcast when no home value is configured.
pub const IMSI_11_12_UNSPECIFIED: u8 = 0x7f;

impl Default for BtsCellParams {
    fn default() -> Self {
        let overhead = OverheadParameters::default();
        Self {
            cell: cell_of(&overhead),
            overhead,
            pilot_offset: 0,
            rx_reference_dbm: None,
            channel: ChannelPlan::default(),
            tx_center_frequency_hz: 0,
            rx_center_frequency_hz: 0,
            an_grpc_addr: None,
            paging_retry: crate::config::PagingRetryConfig::default(),
            evdo: None,
            timezone: TimezoneConfig::default(),
            mcc: MCC_UNSPECIFIED,
            imsi_11_12: IMSI_11_12_UNSPECIFIED,
        }
    }
}

/// Sector for the placeholder parameters the registry answers with before any
/// cell has enrolled. A real cell's sector always comes from its enrollment.
const FALLBACK_SECTOR: u8 = 1;

fn cell_of(overhead: &OverheadParameters) -> AccessCellId {
    AccessCellId {
        cell: overhead.base_id,
        sector: FALLBACK_SECTOR,
    }
}

impl BtsCellParams {
    /// Parameters for a cell identified only by its overhead, with the
    /// remaining radio values left at their defaults.
    pub fn from_overhead(overhead: OverheadParameters) -> Self {
        Self {
            cell: cell_of(&overhead),
            overhead,
            ..Self::default()
        }
    }

    /// Color code the cell's HRPD sector advertises. It is the top byte of the
    /// on-air access terminal identifier (C.S0024-500 5.3.7.1.5.1), so a value
    /// that does not fit one byte cannot have been advertised and reads as
    /// absent rather than aliasing onto another cell's color code.
    pub fn evdo_color_code(&self) -> Option<u8> {
        u8::try_from(self.evdo.as_ref()?.color_code).ok()
    }
}

/// One enrolled cell: its BTS-supplied parameters, the clients that reach it,
/// and how far it has attached.
pub struct BtsEntry {
    cell: AccessCellId,
    inner: RwLock<BtsEntryInner>,
}

struct BtsEntryInner {
    params: Arc<BtsCellParams>,
    control: Option<Arc<dyn BtsControlClient>>,
    oam: Option<BtsOamClient>,
    state: BtsAttachState,
    status_detail: Option<String>,
    /// Operations endpoint of the peer this entry belongs to. Two peers
    /// reporting one cell would otherwise share an entry and overwrite each
    /// other's clients.
    peer: Option<String>,
}

impl BtsEntry {
    pub fn cell(&self) -> AccessCellId {
        self.cell
    }

    pub fn params(&self) -> Arc<BtsCellParams> {
        self.inner.read().params.clone()
    }

    /// Abis control client for this cell, present once its Abis link is up.
    pub fn control(&self) -> Option<Arc<dyn BtsControlClient>> {
        self.inner.read().control.clone()
    }

    /// OAM gRPC client for this cell, used to proxy BTS management requests.
    pub fn oam(&self) -> Option<BtsOamClient> {
        self.inner.read().oam.clone()
    }

    pub fn attach_state(&self) -> BtsAttachState {
        self.inner.read().state
    }

    pub fn status_detail(&self) -> Option<String> {
        self.inner.read().status_detail.clone()
    }

    pub fn is_in_service(&self) -> bool {
        self.attach_state() == BtsAttachState::InService
    }

    /// Put the cell in service without an Abis client. Reaching `InService`
    /// otherwise needs a full `BtsControlClient`.
    #[cfg(test)]
    pub(crate) fn mark_in_service_for_test(&self) {
        self.inner.write().state = BtsAttachState::InService;
    }

    /// Operations endpoint of the peer serving this cell.
    pub fn peer(&self) -> Option<String> {
        self.inner.read().peer.clone()
    }

    /// Replace the enrolled parameters and the OAM client that delivered
    /// them. The cell is Enrolled until its Abis link comes up.
    pub fn set_enrolled(
        &self,
        params: BtsCellParams,
        oam: Option<BtsOamClient>,
        peer: Option<&str>,
    ) {
        let mut inner = self.inner.write();
        inner.params = Arc::new(params);
        inner.oam = oam;
        inner.status_detail = None;
        if let Some(peer) = peer {
            inner.peer = Some(peer.to_string());
        }
        if inner.control.is_none() {
            inner.state = BtsAttachState::Enrolled;
        } else {
            inner.state = BtsAttachState::InService;
        }
    }

    /// Attach the Abis control client, putting the cell in service.
    pub fn set_control(&self, control: Arc<dyn BtsControlClient>) {
        let mut inner = self.inner.write();
        inner.control = Some(control);
        inner.state = BtsAttachState::InService;
        inner.status_detail = None;
    }

    /// Drop every client for this cell and record why it went away. The last
    /// enrolled parameters are kept so a page or teardown in flight still
    /// resolves an identity for the cell.
    pub fn set_detached(&self, detail: impl Into<String>) {
        let mut inner = self.inner.write();
        inner.control = None;
        inner.oam = None;
        inner.state = BtsAttachState::Disconnected;
        inner.status_detail = Some(detail.into());
    }

    /// Drop only the Abis control client, leaving the cell enrolled.
    pub fn set_abis_down(&self, detail: impl Into<String>) {
        let mut inner = self.inner.write();
        inner.control = None;
        inner.state = BtsAttachState::Enrolled;
        inner.status_detail = Some(detail.into());
    }

    pub fn summary(&self) -> BtsCellSummary {
        let inner = self.inner.read();
        let params = &inner.params;
        BtsCellSummary {
            cell: self.cell,
            state: inner.state,
            pilot_pn: params.pilot_offset as u16,
            band_class: params.channel.band_class.as_str().to_string(),
            cdma_channel: params.channel.cdma_channel,
            sid: params.overhead.sid,
            nid: params.overhead.nid,
            evdo_enabled: params.evdo.is_some(),
            evdo_color_code: params.evdo_color_code(),
            an_grpc_addr: params.an_grpc_addr.clone(),
            status_detail: inner.status_detail.clone(),
        }
    }
}

/// One cell, summarized for enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtsCellSummary {
    pub cell: AccessCellId,
    pub state: BtsAttachState,
    /// Pilot PN offset in 64-chip units.
    pub pilot_pn: u16,
    pub band_class: String,
    pub cdma_channel: u16,
    pub sid: u16,
    pub nid: u16,
    pub evdo_enabled: bool,
    /// Color code the cell's HRPD sector advertises. It is the top byte of the
    /// on-air access terminal identifier (C.S0024-500 5.3.7.1.5.1), so it is
    /// what ties an HRPD session back to the cell that issued its UATI.
    pub evdo_color_code: Option<u8>,
    /// Where this cell's HRPD access network serves its session API, when it
    /// runs an EV-DO carrier.
    pub an_grpc_addr: Option<String>,
    /// Why the cell is not in service, when it is not.
    pub status_detail: Option<String>,
}

/// Why the registry refused an enrollment.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    #[error("color code {color_code} is already advertised by cell {cell}")]
    ColorCodeInUse { color_code: u32, cell: AccessCellId },
    #[error("sector id {sector_id} on channel {channel} is already advertised by cell {cell}")]
    SectorChannelInUse {
        sector_id: String,
        channel: u32,
        cell: AccessCellId,
    },
    #[error("cell {cell} is already served by {peer}")]
    ServedByOtherPeer { cell: AccessCellId, peer: String },
}

/// One configured peer as the management plane sees it: a stable opaque id,
/// its attach state, and the cell it has once in service. Every configured
/// peer has one, so the UI can show an unenrolled or dropped peer as offline.
struct PeerHandle {
    peer_id: String,
    oam_endpoint: String,
    inner: RwLock<PeerRuntime>,
}

struct PeerRuntime {
    state: BtsAttachState,
    status_detail: Option<String>,
    cell: Option<AccessCellId>,
}

/// A configured peer's management-plane state.
#[derive(Debug, Clone)]
pub struct PeerSummary {
    pub peer_id: String,
    pub oam_endpoint: String,
    pub state: BtsAttachState,
    pub status_detail: Option<String>,
    /// The cell this peer enrolled with, present only while it is in service.
    pub cell: Option<AccessCellId>,
}

/// Every BTS this BSC serves. Radio logic keys on the enrolled cells. The
/// management plane keys on the configured peers by their opaque id.
#[derive(Default)]
pub struct BtsRegistry {
    entries: RwLock<BTreeMap<AccessCellId, Arc<BtsEntry>>>,
    peers: RwLock<Vec<Arc<PeerHandle>>>,
    fallback: Arc<BtsCellParams>,
}

impl BtsRegistry {
    /// An empty registry with no configured peers, for tests and callers that
    /// enroll directly.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A registry seeded with the configured peers, each offline until it
    /// enrolls.
    pub fn with_peers(peers: &[BtsPeerConfig]) -> Arc<Self> {
        let handles = peers
            .iter()
            .map(|peer| {
                Arc::new(PeerHandle {
                    peer_id: peer.peer_id().to_string(),
                    oam_endpoint: peer.oam_endpoint.clone(),
                    inner: RwLock::new(PeerRuntime {
                        state: BtsAttachState::Disconnected,
                        status_detail: Some("not yet enrolled".to_string()),
                        cell: None,
                    }),
                })
            })
            .collect();
        Arc::new(Self {
            peers: RwLock::new(handles),
            ..Self::default()
        })
    }

    /// Every configured peer as the management plane sees it, in config order.
    pub fn peer_summaries(&self) -> Vec<PeerSummary> {
        self.peers
            .read()
            .iter()
            .map(|peer| {
                let runtime = peer.inner.read();
                PeerSummary {
                    peer_id: peer.peer_id.clone(),
                    oam_endpoint: peer.oam_endpoint.clone(),
                    state: runtime.state,
                    status_detail: runtime.status_detail.clone(),
                    cell: runtime.cell,
                }
            })
            .collect()
    }

    /// The cell a peer currently serves, or `None` when the peer is unknown or
    /// offline. Callers resolve this to an entry for the OAM client.
    pub fn peer_cell(&self, peer_id: &str) -> Option<AccessCellId> {
        self.peers
            .read()
            .iter()
            .find(|peer| peer.peer_id == peer_id)
            .and_then(|peer| peer.inner.read().cell)
    }

    /// Records that the peer at `oam_endpoint` is in service on `cell`.
    pub fn set_peer_in_service(&self, oam_endpoint: &str, cell: AccessCellId) {
        if let Some(peer) = self
            .peers
            .read()
            .iter()
            .find(|peer| peer.oam_endpoint == oam_endpoint)
        {
            let mut runtime = peer.inner.write();
            runtime.state = BtsAttachState::InService;
            runtime.status_detail = None;
            runtime.cell = Some(cell);
        }
    }

    /// Records that the peer at `oam_endpoint` is offline, with why.
    pub fn set_peer_offline(&self, oam_endpoint: &str, detail: impl Into<String>) {
        if let Some(peer) = self
            .peers
            .read()
            .iter()
            .find(|peer| peer.oam_endpoint == oam_endpoint)
        {
            let mut runtime = peer.inner.write();
            runtime.state = BtsAttachState::Disconnected;
            runtime.status_detail = Some(detail.into());
            runtime.cell = None;
        }
    }

    /// Cell whose HRPD sector advertises `color_code`.
    ///
    /// The color code is the top byte of the on-air access terminal identifier
    /// (C.S0024-500 5.3.7.1.5.1), so this resolves an HRPD session back to the
    /// cell that issued its UATI. `enroll` keeps color codes distinct, so at
    /// most one cell matches.
    pub fn cell_for_color_code(&self, color_code: u8) -> Option<AccessCellId> {
        self.entries
            .read()
            .values()
            .find(|entry| entry.params().evdo_color_code() == Some(color_code))
            .map(|entry| entry.cell())
    }

    /// Reject `params` when another cell already claims the same HRPD
    /// identity.
    ///
    /// Two sectors are distinguished on air by their (SectorID, CDMA channel)
    /// pair (C.S0024-400 SectorParameters), and an access terminal's address
    /// carries only the color code, so a duplicate of either leaves both the
    /// air interface and session attribution ambiguous.
    fn hrpd_identity_conflict(
        entries: &BTreeMap<AccessCellId, Arc<BtsEntry>>,
        params: &BtsCellParams,
    ) -> Option<EnrollError> {
        let evdo = params.evdo.as_ref()?;
        entries
            .values()
            .filter(|entry| entry.cell() != params.cell)
            .find_map(|entry| {
                let other = entry.params();
                let other_evdo = other.evdo.as_ref()?;
                if other_evdo.color_code == evdo.color_code {
                    return Some(EnrollError::ColorCodeInUse {
                        color_code: evdo.color_code,
                        cell: entry.cell(),
                    });
                }
                if other_evdo.sector_id == evdo.sector_id && other_evdo.channel == evdo.channel {
                    return Some(EnrollError::SectorChannelInUse {
                        sector_id: evdo.sector_id.clone(),
                        channel: evdo.channel,
                        cell: entry.cell(),
                    });
                }
                None
            })
    }

    /// Log when an enrolling cell broadcasts a different network identity from
    /// the cells already enrolled.
    ///
    /// Mobiles treat SID/NID as the system they are registered on, so a cell
    /// that disagrees puts handsets into roaming as they move onto it. The BSC
    /// cannot say which value is right, so it reports the disagreement rather
    /// than refusing the cell.
    fn warn_on_network_identity_drift(
        entries: &BTreeMap<AccessCellId, Arc<BtsEntry>>,
        params: &BtsCellParams,
    ) {
        for entry in entries.values() {
            if entry.cell() == params.cell {
                continue;
            }
            let other = entry.params();
            if other.overhead.sid != params.overhead.sid
                || other.overhead.nid != params.overhead.nid
                || other.mcc != params.mcc
            {
                warn!(
                    "BSC: cell {} enrolls with SID {}/NID {}/MCC {} but cell {} broadcasts SID {}/NID {}/MCC {} — mobiles moving between them will see a system change",
                    params.cell,
                    params.overhead.sid,
                    params.overhead.nid,
                    params.mcc,
                    entry.cell(),
                    other.overhead.sid,
                    other.overhead.nid,
                    other.mcc,
                );
                return;
            }
        }
    }

    /// Record `params` for their cell, creating the entry on first
    /// enrollment and replacing the stored parameters on re-enrollment.
    ///
    /// Fails when the cell's HRPD identity collides with one already enrolled,
    /// leaving the registry untouched.
    pub fn enroll(
        &self,
        params: BtsCellParams,
        oam: Option<BtsOamClient>,
        peer: Option<&str>,
    ) -> Result<Arc<BtsEntry>, EnrollError> {
        let cell = params.cell;
        let mut entries = self.entries.write();
        // A peer that renumbered keeps its old entry otherwise, and that ghost
        // holds the HRPD identity the peer is now re-enrolling with.
        if let Some(peer) = peer {
            entries.retain(|other_cell, entry| {
                let stale = *other_cell != cell && entry.peer().as_deref() == Some(peer);
                if stale {
                    info!("BSC: retiring cell {other_cell} — {peer} now serves {cell}");
                }
                !stale
            });
        }
        if let Some(conflict) = Self::hrpd_identity_conflict(&entries, &params) {
            return Err(conflict);
        }
        Self::warn_on_network_identity_drift(&entries, &params);
        if let Some(existing) = entries.get(&cell) {
            let owner = existing.peer();
            if let (Some(owner), Some(peer)) = (owner.as_deref(), peer)
                && owner != peer
                && existing.is_in_service()
            {
                return Err(EnrollError::ServedByOtherPeer {
                    cell,
                    peer: owner.to_string(),
                });
            }
        }
        let entry = entries.entry(cell).or_insert_with(|| {
            Arc::new(BtsEntry {
                cell,
                inner: RwLock::new(BtsEntryInner {
                    params: Arc::new(params.clone()),
                    control: None,
                    oam: None,
                    state: BtsAttachState::Disconnected,
                    status_detail: None,
                    peer: None,
                }),
            })
        });
        entry.set_enrolled(params, oam, peer);
        Ok(entry.clone())
    }

    pub fn get(&self, cell: AccessCellId) -> Option<Arc<BtsEntry>> {
        self.entries.read().get(&cell).cloned()
    }

    pub fn entries(&self) -> Vec<Arc<BtsEntry>> {
        self.entries.read().values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// The only enrolled cell, when there is exactly one. Requests that omit
    /// a cell identifier resolve through this, which keeps a single-cell
    /// deployment addressable without naming its cell.
    pub fn sole_entry(&self) -> Option<Arc<BtsEntry>> {
        let entries = self.entries.read();
        (entries.len() == 1).then(|| entries.values().next().cloned())?
    }

    /// Entry for `cell`, or the sole enrolled cell when `cell` is `None`.
    ///
    /// A named cell that is not enrolled resolves to nothing rather than to
    /// the sole entry, so a request aimed at the wrong cell is refused
    /// instead of being answered by another cell's radio.
    pub fn resolve(&self, cell: Option<AccessCellId>) -> Option<Arc<BtsEntry>> {
        match cell {
            Some(cell) => self.get(cell),
            None => self.sole_entry(),
        }
    }

    /// Cells currently carrying traffic. Broadcast pages go to these.
    pub fn in_service(&self) -> Vec<Arc<BtsEntry>> {
        self.entries
            .read()
            .values()
            .filter(|entry| entry.is_in_service())
            .cloned()
            .collect()
    }

    /// Parameters for `cell`, or the sole enrolled cell when `cell` is `None`.
    ///
    /// Falls back to built-in defaults when neither resolves. Every caller
    /// passes a cell that came off the Abis Cell Identifier IE or a mobile's
    /// binding, so a fallback with cells enrolled means an event arrived
    /// without one and is worth a warning rather than silently answering with
    /// parameters no cell broadcasts.
    pub fn params(&self, cell: Option<AccessCellId>) -> Arc<BtsCellParams> {
        if let Some(entry) = self.resolve(cell) {
            return entry.params();
        }
        if !self.is_empty() {
            warn!(
                "BSC: no cell resolved for {cell:?}; answering with default parameters that no cell broadcasts"
            );
        }
        self.fallback.clone()
    }

    /// Abis control client for `cell`, falling back to the sole enrolled cell.
    pub fn control(&self, cell: Option<AccessCellId>) -> Option<Arc<dyn BtsControlClient>> {
        self.resolve(cell).and_then(|entry| entry.control())
    }

    pub fn summaries(&self) -> Vec<BtsCellSummary> {
        self.entries
            .read()
            .values()
            .map(|entry| entry.summary())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_id(cell: u16) -> AccessCellId {
        AccessCellId {
            cell,
            sector: FALLBACK_SECTOR,
        }
    }

    fn params(cell: u16) -> BtsCellParams {
        let mut overhead = OverheadParameters::default();
        overhead.base_id = cell;
        BtsCellParams::from_overhead(overhead)
    }

    #[test]
    fn a_lone_cell_answers_a_request_that_names_no_cell() {
        let registry = BtsRegistry::new();
        registry.enroll(params(7), None, None).expect("no conflict");
        assert_eq!(registry.resolve(None).unwrap().cell().cell, 7);
    }

    #[test]
    fn two_cells_leave_an_unaddressed_request_unresolved() {
        let registry = BtsRegistry::new();
        registry.enroll(params(7), None, None).expect("no conflict");
        registry.enroll(params(8), None, None).expect("no conflict");
        assert!(registry.resolve(None).is_none());
        assert_eq!(registry.params(None).overhead.base_id, 1);
    }

    #[test]
    fn re_enrollment_replaces_parameters_and_keeps_the_entry() {
        let registry = BtsRegistry::new();
        let first = registry.enroll(params(7), None, None).expect("no conflict");
        let mut updated = params(7);
        updated.pilot_offset = 168;
        let second = registry.enroll(updated, None, None).expect("no conflict");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.params().pilot_offset, 168);
    }

    fn evdo_params(cell: u16, color_code: u32, sector_id: &str) -> BtsCellParams {
        let mut params = params(cell);
        params.evdo = Some(crate::grpc::proto::EvdoCarrierConfig {
            color_code,
            sector_id: sector_id.to_string(),
            channel: 630,
            ..Default::default()
        });
        params
    }

    #[test]
    fn a_second_cell_cannot_claim_an_advertised_color_code() {
        let registry = BtsRegistry::new();
        registry
            .enroll(evdo_params(7, 26, "aa"), None, None)
            .expect("no conflict");
        let Err(conflict) = registry.enroll(evdo_params(8, 26, "bb"), None, None) else {
            panic!("a duplicate color code is ambiguous on air");
        };
        assert!(conflict.to_string().contains("color code 26"), "{conflict}");
    }

    #[test]
    fn a_second_cell_cannot_claim_a_sector_id_on_the_same_channel() {
        let registry = BtsRegistry::new();
        registry
            .enroll(evdo_params(7, 26, "aa"), None, None)
            .expect("no conflict");
        let Err(conflict) = registry.enroll(evdo_params(8, 27, "aa"), None, None) else {
            panic!("a duplicate sector id is ambiguous on air");
        };
        assert!(conflict.to_string().contains("sector id aa"), "{conflict}");
    }

    #[test]
    fn distinct_hrpd_identities_resolve_a_session_back_to_its_cell() {
        let registry = BtsRegistry::new();
        registry
            .enroll(evdo_params(7, 26, "aa"), None, None)
            .expect("no conflict");
        registry
            .enroll(evdo_params(8, 27, "bb"), None, None)
            .expect("no conflict");
        assert_eq!(registry.cell_for_color_code(26).unwrap().cell, 7);
        assert_eq!(registry.cell_for_color_code(27).unwrap().cell, 8);
        assert!(registry.cell_for_color_code(99).is_none());
    }

    #[test]
    fn re_enrolling_a_cell_does_not_conflict_with_itself() {
        let registry = BtsRegistry::new();
        registry
            .enroll(evdo_params(7, 26, "aa"), None, None)
            .expect("no conflict");
        registry
            .enroll(evdo_params(7, 26, "aa"), None, None)
            .expect("a cell may re-enroll with its own identity");
    }

    #[test]
    fn a_peer_that_renumbers_releases_its_previous_cell() {
        let registry = BtsRegistry::new();
        registry
            .enroll(evdo_params(7, 26, "aa"), None, Some("http://bts-1"))
            .expect("no conflict");
        // Re-enrolling the same peer with a new cell id keeps its HRPD
        // identity and replaces the earlier entry for that peer.
        registry
            .enroll(evdo_params(8, 26, "aa"), None, Some("http://bts-1"))
            .expect("a renumbered peer is not blocked by its own ghost");
        assert!(registry.get(cell_id(7)).is_none());
        assert_eq!(registry.cell_for_color_code(26).unwrap().cell, 8);
    }

    #[test]
    fn a_second_peer_cannot_take_over_a_live_cell() {
        let registry = BtsRegistry::new();
        let entry = registry
            .enroll(params(7), None, Some("http://bts-1"))
            .expect("no conflict");
        entry.mark_in_service_for_test();
        let Err(refusal) = registry.enroll(params(7), None, Some("http://bts-2")) else {
            panic!("a live cell must not accept a different peer");
        };
        assert!(
            refusal.to_string().contains("already served by"),
            "{refusal}"
        );
    }

    #[test]
    fn a_cell_carries_traffic_only_once_abis_is_up() {
        let registry = BtsRegistry::new();
        let entry = registry.enroll(params(7), None, None).expect("no conflict");
        assert_eq!(entry.attach_state(), BtsAttachState::Enrolled);
        assert!(registry.in_service().is_empty());
        entry.set_abis_down("connection refused");
        assert_eq!(entry.status_detail().as_deref(), Some("connection refused"));
    }
}
