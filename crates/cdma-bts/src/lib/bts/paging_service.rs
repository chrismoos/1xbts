use std::sync::Arc;

use parking_lot::Mutex;

use super::paging_supplier::{
    PagingRetryConfig, PagingRetryEvent, PagingSupplierState, PchTransmitEvent, PendingPageRecord,
};
use crate::lac::paging_messages::{MsAddress, MsPageAddress};

/// A service wrapper around `PagingSupplierState` that provides clean,
/// lock-managing methods for paging operations.
#[derive(Clone)]
pub struct PagingService {
    state: Arc<Mutex<PagingSupplierState>>,
}

impl PagingService {
    /// Create a new paging service with default retry configuration.
    pub fn new(overhead_mcc: u16, overhead_imsi_11_12: u8) -> Self {
        Self {
            state: Arc::new(Mutex::new(PagingSupplierState::new(
                overhead_mcc,
                overhead_imsi_11_12,
            ))),
        }
    }

    /// Create a new paging service with explicit retry configuration.
    pub fn new_with_retry_config(
        config: PagingRetryConfig,
        overhead_mcc: u16,
        overhead_imsi_11_12: u8,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(PagingSupplierState::new_with_retry_config(
                config,
                overhead_mcc,
                overhead_imsi_11_12,
            ))),
        }
    }

    /// Get the underlying state for compatibility with AbisAgent and
    /// `build_bts_paging_supplier`.
    pub fn state(&self) -> &Arc<Mutex<PagingSupplierState>> {
        &self.state
    }

    /// Enqueue page records for inclusion in the next GPM.
    pub fn enqueue_page_records(&self, records: Vec<PendingPageRecord>) {
        let mut guard = self.state.lock();
        guard.pending_page_records.extend(records);
    }

    /// Cancel all pending page records for the given address.
    /// Returns the number of records removed.
    pub fn cancel_pages_for_address(&self, addr: &MsPageAddress) -> usize {
        let mut guard = self.state.lock();
        guard.cancel_pages_for_address(addr)
    }

    /// Queue a directed SDU from a PchMessageTransfer for air-interface
    /// transmission. Converts MobileIdentity to MsAddress, assigns ARQ
    /// fields, and builds the DataRequest with proper addressing.
    pub fn queue_directed_pch(
        &self,
        identity: &cdma_abis::control::typed::MobileIdentity,
        aim: &cdma_abis::control::typed::AirInterfaceMessagePayload,
        ack_req: bool,
        correlation_id: Option<u32>,
        ack_notify: bool,
    ) {
        let mut guard = self.state.lock();
        guard.queue_directed_pch(identity, aim, ack_req, correlation_id, ack_notify);
    }

    /// Check if an MS ACK matches a pending ack-notify request.
    /// Returns the correlation_id if found and removes the entry.
    pub fn check_ack_notify(&self, addr: &MsAddress, ack_seq: u8) -> Option<u32> {
        let mut guard = self.state.lock();
        guard.check_ack_notify(addr, ack_seq)
    }

    /// Drain buffered retry events (failures from slot-aligned retransmission).
    pub fn drain_retry_events(&self) -> Vec<PagingRetryEvent> {
        let mut guard = self.state.lock();
        guard.drain_retry_events()
    }

    /// Install a broadcast sender for PCH transmit events.
    pub fn set_pch_transmit_tx(&self, tx: tokio::sync::broadcast::Sender<PchTransmitEvent>) {
        let mut guard = self.state.lock();
        guard.set_pch_transmit_tx(tx);
    }

    /// Returns true if there are any pending page records or directed SDUs.
    pub fn has_pending_pages(&self) -> bool {
        let guard = self.state.lock();
        !guard.pending_page_records.is_empty() || !guard.pending_directed_sdus.is_empty()
    }

    /// Returns the number of pending page records.
    pub fn pending_page_count(&self) -> usize {
        let guard = self.state.lock();
        guard.pending_page_records.len()
    }

    /// Returns the number of pending directed SDUs.
    pub fn pending_directed_count(&self) -> usize {
        let guard = self.state.lock();
        guard.pending_directed_sdus.len()
    }
}
