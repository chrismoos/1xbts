//! The AN side of A21 (A.S0008-C §2.8). The AN has no CSNA, so it acknowledges
//! 1x tunneling requests as unavailable.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use cdma_a21::{A21Cause, A21Endpoint, A21EventKind, A21Message, A21MessageType, MobileIdentity};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdA21Config {
    /// Local A21 address. Port 4597 between hosts. The single-host default is
    /// `127.0.0.1:17049`, since the BSC holds 4597.
    pub bind_addr: SocketAddr,
    /// The IWS's A21 address, normally port 4597.
    pub iws_addr: SocketAddr,
}

impl Default for HrpdA21Config {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], cdma_a21::SINGLE_HOST_AN_PORT)),
            iws_addr: SocketAddr::from(([127, 0, 0, 1], cdma_a21::A21_UDP_PORT)),
        }
    }
}

#[derive(Debug)]
enum Presence {
    Present(String),
    Gone(String),
}

static REPORTS: OnceLock<mpsc::UnboundedSender<Presence>> = OnceLock::new();

pub(crate) fn report_present(imsi: &str) {
    if let Some(reports) = REPORTS.get() {
        let _ = reports.send(Presence::Present(imsi.to_string()));
    }
}

pub(crate) fn report_gone(imsi: &str) {
    if let Some(reports) = REPORTS.get() {
        let _ = reports.send(Presence::Gone(imsi.to_string()));
    }
}

/// A process runs one AN, so only the first call takes effect.
pub(crate) async fn spawn_hrpd_a21(config: HrpdA21Config) {
    let endpoint = match A21Endpoint::bind(config.bind_addr).await {
        Ok(endpoint) => endpoint,
        Err(err) => {
            warn!(
                "HRPD AN A21: bind {} failed, A21 is disabled: {err}",
                config.bind_addr
            );
            return;
        }
    };
    let (reports, report_rx) = mpsc::unbounded_channel();
    if REPORTS.set(reports).is_err() {
        warn!("HRPD AN A21: already running, ignoring a second endpoint");
        return;
    }
    info!(
        "HRPD AN A21: bound on {} toward IWS {}",
        config.bind_addr, config.iws_addr
    );
    tokio::spawn(run(endpoint, config.iws_addr, report_rx));
}

async fn run(
    endpoint: A21Endpoint,
    iws_addr: SocketAddr,
    mut report_rx: mpsc::UnboundedReceiver<Presence>,
) {
    let correlation = AtomicU32::new(1);
    let mut present: HashSet<String> = HashSet::new();
    loop {
        tokio::select! {
            report = report_rx.recv() => {
                let Some(report) = report else { return };
                let (imsi, event) = match report {
                    Presence::Present(imsi) if present.insert(imsi.clone()) => {
                        (imsi, A21EventKind::PresentInHrpd)
                    }
                    Presence::Gone(imsi) if present.remove(&imsi) => {
                        (imsi, A21EventKind::NoLongerPresentInAn)
                    }
                    _ => continue,
                };
                // Relaxed: the counter only needs distinct values.
                let correlation_id = correlation.fetch_add(1, Ordering::Relaxed);
                let message = A21Message::event_notification(
                    correlation_id,
                    MobileIdentity::Imsi(imsi.clone()),
                    event,
                );
                match endpoint.send(iws_addr, &message).await {
                    Ok(()) => info!("HRPD AN A21: reported imsi={imsi} {event:?}"),
                    Err(err) => warn!("HRPD AN A21: event to {iws_addr} failed: {err}"),
                }
            }
            (peer, received) = endpoint.recv() => {
                let message = match received {
                    Ok(message) => message,
                    Err(err) => {
                        warn!("HRPD AN A21: undecodable datagram from {peer}: {err}");
                        continue;
                    }
                };
                match message.message_type {
                    A21MessageType::AirInterfaceSignaling => {
                        info!(
                            "HRPD AN A21: IWS message for {:?} not tunneled, the AN has no CSNA",
                            message.mobile_identity()
                        );
                        let ack = A21Message::ack(
                            message.correlation_id(),
                            message.mobile_identity().cloned(),
                            Some(A21Cause::TunnelingNotAvailable),
                        );
                        if let Err(err) = endpoint.send(peer, &ack).await {
                            warn!("HRPD AN A21: Ack to {peer} failed: {err}");
                        }
                    }
                    A21MessageType::Ack => debug!(
                        "HRPD AN A21: IWS acknowledged correlation_id={:?} cause={:?}",
                        message.correlation_id(),
                        message.cause()
                    ),
                    other => debug!("HRPD AN A21: ignoring {other:?} from {peer}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const RECV_TIMEOUT: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn an_reports_presence_once_and_declines_tunneling() {
        let iws = A21Endpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let an_addr = probe.local_addr().unwrap();
        drop(probe);
        spawn_hrpd_a21(HrpdA21Config {
            bind_addr: an_addr,
            iws_addr: iws.local_addr().unwrap(),
        })
        .await;
        let imsi = "310009176936269";
        report_present(imsi);
        report_present(imsi);
        let (from, event) = tokio::time::timeout(RECV_TIMEOUT, iws.recv())
            .await
            .unwrap();
        let event = event.unwrap();
        assert_eq!(from, an_addr);
        assert_eq!(event.event(), Some(A21EventKind::PresentInHrpd));
        assert_eq!(
            event.mobile_identity(),
            Some(&MobileIdentity::Imsi(imsi.into()))
        );

        let page = A21Message::air_interface_signaling(
            77,
            MobileIdentity::Imsi(imsi.into()),
            cdma_a21::MessageTransmissionControl::default(),
            vec![vec![0x01]],
        );
        iws.send(an_addr, &page).await.unwrap();
        let (_, ack) = tokio::time::timeout(RECV_TIMEOUT, iws.recv())
            .await
            .unwrap();
        let ack = ack.unwrap();
        assert_eq!(ack.message_type, A21MessageType::Ack);
        assert_eq!(ack.correlation_id(), Some(77));
        assert_eq!(ack.cause(), Some(A21Cause::TunnelingNotAvailable));

        report_gone(imsi);
        let (_, gone) = tokio::time::timeout(RECV_TIMEOUT, iws.recv())
            .await
            .unwrap();
        assert_eq!(
            gone.unwrap().event(),
            Some(A21EventKind::NoLongerPresentInAn),
            "the duplicate presence report was not sent"
        );
    }
}
