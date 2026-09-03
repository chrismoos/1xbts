//! Base stations: the A1 peers this MSC serves.
//!
//! A base station is any element that terminates A1 toward the MSC. The MSC
//! configures each node's management endpoint, pulls its enrollment over
//! gRPC, and dials A1 to the address the enrollment names. Identity does not
//! belong on A1: the MSC knows which node every A1 connection is because it
//! dialed it. A node is retried forever with backoff, probed over its
//! management plane while A1 is quiet, and torn down when either link goes.
//!
//! A1 call ids are unique only within one node. The registry maps each
//! node's wire call id to an MSC-internal id that is unique across nodes, so
//! the runtime keys its state by [`CallId`] alone and every send finds the
//! node it belongs to.

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdma_ios::transport::A1TransportEvent;
use cdma_ios::transport::A1TransportSender;
use cdma_ios::{A1TransportError, EncodedA1Message};
use log::{info, warn};
use tokio::sync::mpsc;

use crate::call_control::CallId;
use crate::config::BaseStationConfig;
use crate::grpc::base_station::v1::EnrollRequest;
use crate::grpc::base_station::v1::base_station_service_client::BaseStationServiceClient;

/// First delay after a failed or dropped attach.
const RETRY_MIN: Duration = Duration::from_secs(1);
/// Ceiling the retry delay backs off to.
const RETRY_MAX: Duration = Duration::from_secs(30);
/// How often an attached node is probed over its management plane. A1
/// carries no traffic on an idle node, so this is what notices one that
/// disappeared without closing its connection.
const LIVENESS_POLL: Duration = Duration::from_secs(15);
/// Idle time before the management channel sends a keepalive probe.
const OAM_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// An unanswered keepalive probe closes the management channel after this.
const OAM_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Ceiling on any single management request.
const OAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Inbound events buffered ahead of the runtime.
const EVENT_QUEUE_CAPACITY: usize = 1024;
/// First internal call id handed out when a node's wire id is already taken
/// by another node. Sits above the MSC's own allocator and above the range
/// the BSC seeds mobile-originated ids from, so the three never meet.
const REMAPPED_CALL_ID_BASE: u64 = 1 << 45;

/// Stable identifier of a base station, as it enrolls and as it stamps
/// registration bindings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BaseStationId(pub String);

impl fmt::Display for BaseStationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One cell a base station serves, with the identity it broadcasts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedCell {
    pub cell: cdma_ios::CellId,
    pub sid: u16,
    pub nid: u16,
    pub mcc_digits: String,
    pub imsi_11_12_digits: String,
    pub in_service: bool,
}

/// What the MSC knows about one configured base station.
#[derive(Clone, Debug)]
pub struct BaseStationSummary {
    pub management_endpoint: String,
    /// Absent until the node has enrolled at least once.
    pub node_id: Option<BaseStationId>,
    pub a1_addr: Option<SocketAddr>,
    /// Whether the MSC currently holds the node's A1 connection.
    pub attached: bool,
    pub cells: Vec<ServedCell>,
    /// Why the node is not attached, when it is not.
    pub status_detail: Option<String>,
}

/// What the A1 side hands the runtime.
#[derive(Debug)]
pub enum A1Event {
    /// A message from `node`, with its call id already translated.
    Message {
        node: BaseStationId,
        message: EncodedA1Message,
    },
    /// The MSC now holds `node`'s A1 connection.
    Attached(BaseStationId),
    /// `node`'s A1 connection is gone. `calls` are the ids it held, already
    /// unmapped, for the runtime to clean up.
    Detached {
        node: BaseStationId,
        calls: Vec<CallId>,
    },
}

/// The runtime's view of A1, independent of how peers are reached.
#[async_trait::async_trait]
pub trait MscA1Endpoint: Send + Sync {
    /// Waits for the next inbound message or node transition.
    async fn recv(&self) -> Option<A1Event>;

    /// Sends a call-scoped message to the node its call id belongs to.
    async fn send(&self, message: EncodedA1Message) -> Result<(), A1TransportError>;

    /// Sends a connectionless message to a named node.
    async fn send_to_node(
        &self,
        node: &BaseStationId,
        message: EncodedA1Message,
    ) -> Result<(), A1TransportError>;

    /// Records that an MSC-allocated call id belongs to `node`, so later
    /// sends for it find the node.
    fn bind_call(&self, call_id: CallId, node: &BaseStationId);

    /// Forgets a call id once the call is gone.
    fn release_call(&self, call_id: CallId);

    /// The node a call id belongs to.
    fn node_for_call(&self, call_id: CallId) -> Option<BaseStationId>;

    /// Whether the MSC currently holds `node`'s A1 connection.
    fn is_attached(&self, node: &BaseStationId) -> bool;

    /// The overhead of one cell on one node, when the node enrolled it.
    fn served_cell(&self, node: &BaseStationId, cell: cdma_ios::CellId) -> Option<ServedCell>;

    /// The MCC and IMSI_11_12 digits `node` broadcasts, used to complete an
    /// abbreviated IMSI_S into a full IMSI. Taken from an enrolled cell.
    fn serving_imsi_prefix(&self, node: &BaseStationId) -> Option<(String, String)>;
}

/// Digit counts for the parts of a CDMA IMSI (C.S0005-E 2.3.1): MCC, the
/// IMSI_11_12 pair, and the 10-digit IMSI_S that a base station broadcasts a
/// prefix for.
const MCC_DIGITS: usize = 3;
const IMSI_11_12_DIGITS: usize = 2;
const IMSI_S_DIGITS: usize = 10;

/// Completes an abbreviated IMSI_S (the 10-digit MIN) into a full IMSI using a
/// serving cell's broadcast MCC and IMSI_11_12. A femto peer sends only the
/// IMSI_S and relies on the MSC to prefix it, whereas a BSC already sends the
/// full IMSI. An identity that is already full, or is not a bare IMSI_S, is
/// returned unchanged.
pub fn complete_imsi(mcc: &str, imsi_11_12: &str, identity: &str) -> String {
    let is_imsi_s = identity.len() == IMSI_S_DIGITS && identity.bytes().all(|b| b.is_ascii_digit());
    if is_imsi_s && mcc.len() == MCC_DIGITS && imsi_11_12.len() == IMSI_11_12_DIGITS {
        format!("{mcc}{imsi_11_12}{identity}")
    } else {
        identity.to_string()
    }
}

struct NodeEntry {
    management_endpoint: String,
    node_id: Option<BaseStationId>,
    a1_addr: Option<SocketAddr>,
    cells: Vec<ServedCell>,
    link: Option<A1TransportSender>,
    status_detail: Option<String>,
}

/// Wire-to-internal call id translation across every node.
#[derive(Default)]
struct CallMap {
    by_internal: HashMap<u64, (BaseStationId, u64)>,
    by_wire: HashMap<(BaseStationId, u64), u64>,
    next_remapped: u64,
}

impl CallMap {
    /// The internal id for a node's wire id, allocating one on first sight.
    /// A wire id nobody else holds is used unchanged, so single-node
    /// deployments keep the ids they always had.
    fn inbound(&mut self, node: &BaseStationId, wire: u64) -> u64 {
        if let Some(internal) = self.by_wire.get(&(node.clone(), wire)) {
            return *internal;
        }
        let internal = if self.by_internal.contains_key(&wire) {
            self.allocate_remapped()
        } else {
            wire
        };
        self.by_internal.insert(internal, (node.clone(), wire));
        self.by_wire.insert((node.clone(), wire), internal);
        internal
    }

    fn allocate_remapped(&mut self) -> u64 {
        if self.next_remapped < REMAPPED_CALL_ID_BASE {
            self.next_remapped = REMAPPED_CALL_ID_BASE;
        }
        loop {
            let candidate = self.next_remapped;
            self.next_remapped += 1;
            if !self.by_internal.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    fn outbound(&self, internal: u64) -> Option<(BaseStationId, u64)> {
        self.by_internal.get(&internal).cloned()
    }

    /// Binds an MSC-allocated id to `node`. The wire id is the internal id,
    /// which cannot collide with the node's own ids because the MSC and the
    /// nodes allocate from disjoint ranges.
    fn bind(&mut self, internal: u64, node: &BaseStationId) {
        if let Some((owner, _)) = self.by_internal.get(&internal)
            && owner != node
        {
            warn!(
                "MSC: call id {internal} rebound from base station {owner} to {node}, the earlier call is lost"
            );
        }
        self.by_internal.insert(internal, (node.clone(), internal));
        self.by_wire.insert((node.clone(), internal), internal);
    }

    fn release(&mut self, internal: u64) {
        if let Some((node, wire)) = self.by_internal.remove(&internal) {
            self.by_wire.remove(&(node, wire));
        }
    }

    /// Drops every mapping for `node`, returning the internal ids it held.
    fn drop_node(&mut self, node: &BaseStationId) -> Vec<u64> {
        let internals: Vec<u64> = self
            .by_internal
            .iter()
            .filter(|(_, (owner, _))| owner == node)
            .map(|(internal, _)| *internal)
            .collect();
        for internal in &internals {
            self.release(*internal);
        }
        internals
    }
}

/// Every base station this MSC serves.
pub struct BaseStations {
    entries: Mutex<Vec<NodeEntry>>,
    calls: Mutex<CallMap>,
    events_tx: mpsc::Sender<A1Event>,
    events_rx: tokio::sync::Mutex<mpsc::Receiver<A1Event>>,
}

impl BaseStations {
    /// A registry for the configured nodes. Nothing attaches until
    /// [`BaseStations::spawn_attach_tasks`] runs.
    pub fn new(configs: &[BaseStationConfig]) -> Arc<Self> {
        let (events_tx, events_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        Arc::new(Self {
            entries: Mutex::new(
                configs
                    .iter()
                    .map(|config| NodeEntry {
                        management_endpoint: config.management_endpoint.clone(),
                        node_id: None,
                        a1_addr: None,
                        cells: Vec::new(),
                        link: None,
                        status_detail: Some("not yet enrolled".to_string()),
                    })
                    .collect(),
            ),
            calls: Mutex::new(CallMap::default()),
            events_tx,
            events_rx: tokio::sync::Mutex::new(events_rx),
        })
    }

    /// Start one attach task per configured node. Each runs until the
    /// process exits.
    pub fn spawn_attach_tasks(self: &Arc<Self>) {
        let endpoints: Vec<String> = self
            .entries
            .lock()
            .expect("base station registry lock")
            .iter()
            .map(|entry| entry.management_endpoint.clone())
            .collect();
        for endpoint in endpoints {
            let nodes = self.clone();
            tokio::spawn(async move { attach_forever(endpoint, nodes).await });
        }
    }

    /// Every configured node as the MSC currently sees it.
    pub fn summaries(&self) -> Vec<BaseStationSummary> {
        self.entries
            .lock()
            .expect("base station registry lock")
            .iter()
            .map(|entry| BaseStationSummary {
                management_endpoint: entry.management_endpoint.clone(),
                node_id: entry.node_id.clone(),
                a1_addr: entry.a1_addr,
                attached: entry.link.is_some(),
                cells: entry.cells.clone(),
                status_detail: entry.status_detail.clone(),
            })
            .collect()
    }

    /// Records a fresh enrollment for the node at `endpoint`. A node id
    /// already attached through a different endpoint is refused so two
    /// misconfigured nodes cannot claim one identity.
    fn record_enrollment(
        &self,
        endpoint: &str,
        node_id: BaseStationId,
        a1_addr: SocketAddr,
        cells: Vec<ServedCell>,
    ) -> Result<(), AttachError> {
        let mut entries = self.entries.lock().expect("base station registry lock");
        if let Some(other) = entries.iter().find(|entry| {
            entry.management_endpoint != endpoint
                && entry.node_id.as_ref() == Some(&node_id)
                && entry.link.is_some()
        }) {
            return Err(format!(
                "node id {node_id} is already attached through {}",
                other.management_endpoint
            )
            .into());
        }
        let entry = entries
            .iter_mut()
            .find(|entry| entry.management_endpoint == endpoint)
            .ok_or_else(|| {
                AttachError::from(format!("no configured base station at {endpoint}"))
            })?;
        entry.node_id = Some(node_id);
        entry.a1_addr = Some(a1_addr);
        entry.cells = cells;
        Ok(())
    }

    fn set_link(&self, endpoint: &str, link: Option<A1TransportSender>, detail: Option<String>) {
        let mut entries = self.entries.lock().expect("base station registry lock");
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.management_endpoint == endpoint)
        {
            entry.link = link;
            entry.status_detail = detail;
        }
    }

    fn refresh_cells(&self, endpoint: &str, cells: Vec<ServedCell>) {
        let mut entries = self.entries.lock().expect("base station registry lock");
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.management_endpoint == endpoint)
        {
            entry.cells = cells;
        }
    }

    fn link_for(&self, node: &BaseStationId) -> Option<A1TransportSender> {
        self.entries
            .lock()
            .expect("base station registry lock")
            .iter()
            .find(|entry| entry.node_id.as_ref() == Some(node))
            .and_then(|entry| entry.link.clone())
    }

    /// Translates an inbound message's call id into the MSC's space.
    fn translate_inbound(
        &self,
        node: &BaseStationId,
        message: EncodedA1Message,
    ) -> EncodedA1Message {
        let Some(wire) = message.call_id() else {
            return message;
        };
        let internal = self
            .calls
            .lock()
            .expect("call map lock")
            .inbound(node, wire);
        message.with_call_id(Some(internal))
    }

    /// Internal ids of every call bound to `node`, unmapped.
    fn drop_calls_for(&self, node: &BaseStationId) -> Vec<CallId> {
        self.calls
            .lock()
            .expect("call map lock")
            .drop_node(node)
            .into_iter()
            .map(CallId)
            .collect()
    }
}

#[async_trait::async_trait]
impl MscA1Endpoint for BaseStations {
    async fn recv(&self) -> Option<A1Event> {
        self.events_rx.lock().await.recv().await
    }

    async fn send(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
        let Some(internal) = message.call_id() else {
            warn!(
                "MSC: {:?} carries no call id and names no base station, dropping",
                message.message_type()
            );
            return Err(A1TransportError::Closed);
        };
        let Some((node, wire)) = self.calls.lock().expect("call map lock").outbound(internal)
        else {
            warn!(
                "MSC: {:?} for call id {internal} belongs to no attached base station, dropping",
                message.message_type()
            );
            return Err(A1TransportError::Closed);
        };
        let Some(link) = self.link_for(&node) else {
            return Err(A1TransportError::Closed);
        };
        link.send(&message.with_call_id(Some(wire))).await
    }

    async fn send_to_node(
        &self,
        node: &BaseStationId,
        message: EncodedA1Message,
    ) -> Result<(), A1TransportError> {
        let Some(link) = self.link_for(node) else {
            return Err(A1TransportError::Closed);
        };
        link.send(&message).await
    }

    fn bind_call(&self, call_id: CallId, node: &BaseStationId) {
        self.calls
            .lock()
            .expect("call map lock")
            .bind(call_id.0, node);
    }

    fn release_call(&self, call_id: CallId) {
        self.calls.lock().expect("call map lock").release(call_id.0);
    }

    fn node_for_call(&self, call_id: CallId) -> Option<BaseStationId> {
        self.calls
            .lock()
            .expect("call map lock")
            .outbound(call_id.0)
            .map(|(node, _)| node)
    }

    fn is_attached(&self, node: &BaseStationId) -> bool {
        self.link_for(node).is_some()
    }

    fn served_cell(&self, node: &BaseStationId, cell: cdma_ios::CellId) -> Option<ServedCell> {
        self.entries
            .lock()
            .expect("base station registry lock")
            .iter()
            .find(|entry| entry.node_id.as_ref() == Some(node))
            .and_then(|entry| {
                entry
                    .cells
                    .iter()
                    .find(|served| served.cell == cell)
                    .cloned()
            })
    }

    fn serving_imsi_prefix(&self, node: &BaseStationId) -> Option<(String, String)> {
        self.entries
            .lock()
            .expect("base station registry lock")
            .iter()
            .find(|entry| entry.node_id.as_ref() == Some(node))
            .and_then(|entry| {
                entry.cells.iter().find_map(|served| {
                    (!served.mcc_digits.is_empty() && !served.imsi_11_12_digits.is_empty())
                        .then(|| (served.mcc_digits.clone(), served.imsi_11_12_digits.clone()))
                })
            })
    }
}

async fn attach_forever(endpoint: String, nodes: Arc<BaseStations>) {
    let mut retry = RETRY_MIN;
    loop {
        match attach_once(&endpoint, &nodes).await {
            Ok(Detached { node, reason }) => {
                retry = RETRY_MIN;
                warn!("MSC: base station {node} at {endpoint} detached ({reason}), re-enrolling");
            }
            Err(error) => {
                warn!("MSC: attach to base station at {endpoint} failed: {error}");
                nodes.set_link(&endpoint, None, Some(error.to_string()));
                retry = (retry * 2).min(RETRY_MAX);
            }
        }
        tokio::time::sleep(retry).await;
    }
}

struct Detached {
    node: BaseStationId,
    reason: &'static str,
}

type AttachError = Box<dyn std::error::Error + Send + Sync>;

async fn attach_once(endpoint: &str, nodes: &Arc<BaseStations>) -> Result<Detached, AttachError> {
    let channel = tonic::transport::Endpoint::from_shared(endpoint.to_string())?
        .tcp_keepalive(Some(OAM_KEEPALIVE_INTERVAL))
        .http2_keep_alive_interval(OAM_KEEPALIVE_INTERVAL)
        .keep_alive_timeout(OAM_KEEPALIVE_TIMEOUT)
        .keep_alive_while_idle(true)
        .timeout(OAM_REQUEST_TIMEOUT)
        .connect()
        .await?;
    let mut client = BaseStationServiceClient::new(channel);
    let enrolled = enroll(&mut client).await?;
    nodes.record_enrollment(
        endpoint,
        enrolled.node_id.clone(),
        enrolled.a1_addr,
        enrolled.cells,
    )?;
    let node = enrolled.node_id;

    let (sender, mut events_rx) = cdma_ios::transport::connect(enrolled.a1_addr)
        .await
        .map_err(|e| format!("A1 connect to {}: {e}", enrolled.a1_addr))?;
    nodes.set_link(endpoint, Some(sender), None);
    info!(
        "MSC: base station {node} attached (management={endpoint} a1={})",
        enrolled.a1_addr
    );
    let _ = nodes.events_tx.send(A1Event::Attached(node.clone())).await;

    let reason = loop {
        tokio::select! {
            event = events_rx.recv() => match event {
                Some(A1TransportEvent::Message(message)) => {
                    let message = nodes.translate_inbound(&node, message);
                    if nodes
                        .events_tx
                        .send(A1Event::Message { node: node.clone(), message })
                        .await
                        .is_err()
                    {
                        break "runtime stopped";
                    }
                }
                Some(A1TransportEvent::Disconnected(error)) => {
                    warn!("MSC: base station {node} A1 link closed: {error}");
                    break "A1 link closed";
                }
                None => break "A1 link closed",
            },
            _ = tokio::time::sleep(LIVENESS_POLL) => {
                match tokio::time::timeout(OAM_REQUEST_TIMEOUT, enroll(&mut client)).await {
                    Ok(Ok(refreshed)) => nodes.refresh_cells(endpoint, refreshed.cells),
                    Ok(Err(error)) => {
                        warn!("MSC: base station {node} stopped answering its management plane: {error}");
                        break "management plane unreachable";
                    }
                    Err(_) => {
                        warn!("MSC: base station {node} management-plane probe timed out");
                        break "management plane unreachable";
                    }
                }
            }
        }
    };

    nodes.set_link(endpoint, None, Some(reason.to_string()));
    let dropped = nodes.drop_calls_for(&node);
    if !dropped.is_empty() {
        warn!(
            "MSC: base station {node} detached with {} call(s) in flight",
            dropped.len()
        );
    }
    let _ = nodes
        .events_tx
        .send(A1Event::Detached {
            node: node.clone(),
            calls: dropped,
        })
        .await;
    Ok(Detached { node, reason })
}

struct Enrolled {
    node_id: BaseStationId,
    a1_addr: SocketAddr,
    cells: Vec<ServedCell>,
}

async fn enroll(
    client: &mut BaseStationServiceClient<tonic::transport::Channel>,
) -> Result<Enrolled, AttachError> {
    let response = client.enroll(EnrollRequest {}).await?.into_inner();
    if response.node_id.is_empty() {
        return Err("enrollment carried no node id".into());
    }
    let a1_addr: SocketAddr = response.a1_addr.parse().map_err(|e| {
        format!(
            "enrollment carried an unusable A1 address {:?}: {e}",
            response.a1_addr
        )
    })?;
    let cells = response
        .cells
        .into_iter()
        .filter_map(|served| {
            let cell = served.cell?;
            let cell = cdma_ios::CellId {
                cell: u16::try_from(cell.cell).ok()?,
                sector: u8::try_from(cell.sector).ok()?,
            };
            Some(ServedCell {
                cell,
                sid: u16::try_from(served.sid).ok()?,
                nid: u16::try_from(served.nid).ok()?,
                mcc_digits: served.mcc_digits,
                imsi_11_12_digits: served.imsi_11_12_digits,
                in_service: served.in_service,
            })
        })
        .collect();
    Ok(Enrolled {
        node_id: BaseStationId(response.node_id),
        a1_addr,
        cells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str) -> BaseStationId {
        BaseStationId(name.to_string())
    }

    #[test]
    fn an_abbreviated_imsi_s_gets_the_serving_cells_prefix() {
        // IMSI_S only (10 digits) becomes MCC + IMSI_11_12 + IMSI_S.
        assert_eq!(complete_imsi("310", "00", "6054426442"), "310006054426442");
        // An already-full 15-digit IMSI is left alone.
        assert_eq!(
            complete_imsi("310", "00", "310006054426442"),
            "310006054426442"
        );
        // Anything that is not a bare 10-digit IMSI_S is returned unchanged.
        assert_eq!(complete_imsi("310", "00", "12345678901"), "12345678901");
        // A missing prefix leaves the identity as-is.
        assert_eq!(complete_imsi("", "", "6054426442"), "6054426442");
    }

    #[test]
    fn a_wire_id_nobody_holds_passes_through_unchanged() {
        let mut map = CallMap::default();
        assert_eq!(map.inbound(&node("bsc-1"), 0x1_0000_0001), 0x1_0000_0001);
        assert_eq!(map.inbound(&node("bsc-1"), 0x1_0000_0001), 0x1_0000_0001);
    }

    #[test]
    fn the_same_wire_id_on_two_nodes_gets_two_internal_ids() {
        let mut map = CallMap::default();
        let first = map.inbound(&node("bsc-1"), 0x1_0000_0001);
        let second = map.inbound(&node("bsc-2"), 0x1_0000_0001);
        assert_ne!(first, second);
        assert!(second >= REMAPPED_CALL_ID_BASE);
        assert_eq!(map.outbound(second), Some((node("bsc-2"), 0x1_0000_0001)));
        assert_eq!(map.outbound(first), Some((node("bsc-1"), 0x1_0000_0001)));
    }

    #[test]
    fn a_bound_msc_id_routes_to_its_node_until_released() {
        let mut map = CallMap::default();
        map.bind(7, &node("bsc-2"));
        assert_eq!(map.outbound(7), Some((node("bsc-2"), 7)));
        map.release(7);
        assert_eq!(map.outbound(7), None);
    }

    #[test]
    fn dropping_a_node_forgets_only_its_calls() {
        let mut map = CallMap::default();
        let a = map.inbound(&node("bsc-1"), 0x1_0000_0001);
        let b = map.inbound(&node("bsc-2"), 0x1_0000_0001);
        map.bind(9, &node("bsc-2"));
        let mut dropped = map.drop_node(&node("bsc-2"));
        dropped.sort();
        assert_eq!(dropped, vec![9, b]);
        assert_eq!(map.outbound(a), Some((node("bsc-1"), 0x1_0000_0001)));
        assert_eq!(map.outbound(b), None);
        // The wire id is still bsc-1's internal id, so bsc-2 is remapped again.
        let again = map.inbound(&node("bsc-2"), 0x1_0000_0001);
        assert!(again >= REMAPPED_CALL_ID_BASE);
        assert_ne!(again, a);
    }

    #[tokio::test]
    async fn sends_translate_back_to_the_wire_id_of_the_owning_node() {
        let nodes = BaseStations::new(&[BaseStationConfig {
            management_endpoint: "http://127.0.0.1:1".to_string(),
            id: None,
        }]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bsc =
            tokio::spawn(async move { cdma_ios::transport::accept(&listener).await.unwrap() });
        let (sender, _msc_rx) = cdma_ios::transport::connect(addr).await.unwrap();
        let (_bsc_sender, mut bsc_rx) = bsc.await.unwrap();
        nodes
            .record_enrollment("http://127.0.0.1:1", node("bsc-9"), addr, Vec::new())
            .unwrap();
        nodes.set_link("http://127.0.0.1:1", Some(sender), None);

        // A wire id already held by another node maps to a fresh internal id.
        nodes
            .calls
            .lock()
            .unwrap()
            .by_internal
            .insert(0x1_0000_0001, (node("other"), 0x1_0000_0001));
        let inbound = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(cdma_ios::MessageType::ClearRequest, vec![]),
            Some(0x1_0000_0001),
        );
        let translated = nodes.translate_inbound(&node("bsc-9"), inbound);
        let internal = translated.call_id().unwrap();
        assert!(internal >= REMAPPED_CALL_ID_BASE);

        let reply = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(cdma_ios::MessageType::ClearCommand, vec![]),
            Some(internal),
        );
        nodes.send(reply).await.unwrap();
        match bsc_rx.recv().await {
            Some(A1TransportEvent::Message(message)) => {
                assert_eq!(message.call_id(), Some(0x1_0000_0001));
                assert_eq!(message.message_type(), cdma_ios::MessageType::ClearCommand);
            }
            other => panic!("expected the translated message, got {other:?}"),
        }
    }
}
