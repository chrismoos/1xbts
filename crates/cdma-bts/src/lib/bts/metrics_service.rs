use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use super::AccessChannelEvent;
use super::handle::{RxMetrics, TxMetrics};

/// Centralized service for publishing BTS telemetry and access-channel events.
///
/// `MetricsService` is `Clone` and cheap to pass into sub-tasks.  It owns the
/// sender sides of the TX-metrics, RX-metrics, and access-event channels that
/// the BTS handle exposes to higher layers (BSC / gRPC).
#[derive(Clone)]
pub struct MetricsService {
    tx_metrics_tx: Arc<watch::Sender<TxMetrics>>,
    rx_metrics_tx: Arc<watch::Sender<RxMetrics>>,
    access_event_tx: mpsc::UnboundedSender<AccessChannelEvent>,
}

impl MetricsService {
    /// Build a new `MetricsService` from the raw channel senders produced by
    /// [`super::handle::create_handle`].
    pub fn new(
        tx_metrics_tx: watch::Sender<TxMetrics>,
        rx_metrics_tx: watch::Sender<RxMetrics>,
        access_event_tx: mpsc::UnboundedSender<AccessChannelEvent>,
    ) -> Self {
        Self {
            tx_metrics_tx: Arc::new(tx_metrics_tx),
            rx_metrics_tx: Arc::new(rx_metrics_tx),
            access_event_tx,
        }
    }

    /// Publish a TX-metrics snapshot.  Receivers see the latest value via a
    /// `watch` channel, so intermediate updates are coalesced automatically.
    pub fn publish_tx_metrics(&self, metrics: TxMetrics) {
        let _ = self.tx_metrics_tx.send(metrics);
    }

    /// Publish an RX-metrics snapshot.
    pub fn publish_rx_metrics(&self, metrics: RxMetrics) {
        let _ = self.rx_metrics_tx.send(metrics);
    }

    /// Publish an access-channel event.  Returns `true` if the event was
    /// enqueued, `false` if the receiver has been dropped or the channel is
    /// full.
    pub fn publish_access_event(&self, event: AccessChannelEvent) -> bool {
        self.access_event_tx.send(event).is_ok()
    }

    /// Clone the `Arc`-wrapped RX-metrics watch sender.
    ///
    /// This is an escape hatch so that `RxSettings` can still receive the
    /// sender directly during the migration period.
    pub fn rx_metrics_sender(&self) -> Arc<watch::Sender<RxMetrics>> {
        Arc::clone(&self.rx_metrics_tx)
    }

    /// Clone the access-event mpsc sender.
    ///
    /// This is an escape hatch so that `RxSettings` can still receive the
    /// sender directly during the migration period.
    pub fn access_event_sender(&self) -> mpsc::UnboundedSender<AccessChannelEvent> {
        self.access_event_tx.clone()
    }
}
