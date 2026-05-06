//! Thin service wrapper around the BTS bearer agent spawn logic.

use std::sync::Arc;
use std::sync::mpsc::Receiver as StdReceiver;

use cdma_abis::bearer_transport::BearerTransport;
use cdma_abis::udp_bearer::UdpBearerDatagram;

use super::TrafficResourceService;

/// Service boundary for bearer-plane transport between the BTS and BSC.
///
/// Wraps [`super::bearer_agent::spawn_bts_bearer_agent`] behind a named
/// type so that callers can treat bearer transport as a discrete service
/// rather than invoking a bare free function.
pub struct BearerTransportService;

impl BearerTransportService {
    /// Create a new bearer transport service instance.
    pub fn new() -> Self {
        Self
    }

    /// Spawn the bearer agent threads.
    ///
    /// Starts two background threads: one that receives forward bearer
    /// frames from the BSC and delivers them to BTS traffic channels, and
    /// one that forwards reverse bearer datagrams to the BSC.
    ///
    /// Delegates to [`super::bearer_agent::spawn_bts_bearer_agent`].
    pub fn spawn(
        &self,
        transport: Arc<BearerTransport>,
        controller: Arc<TrafficResourceService>,
        reverse_bearer_rx: StdReceiver<UdpBearerDatagram>,
    ) {
        super::bearer_agent::spawn_bts_bearer_agent(transport, controller, reverse_bearer_rx)
    }
}
