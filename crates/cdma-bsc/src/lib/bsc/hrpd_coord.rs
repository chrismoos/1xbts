//! The BSC as IWS (A.S0008-C §2.8). A page for an HRPD-present IMSI also goes
//! to the AN, and the 1x page is sent regardless.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cdma_a21::{
    A21Cause, A21Endpoint, A21EventKind, A21Message, A21MessageType, CachedIdentity,
    HybridIdentityCache, MessageTransmissionControl, MobileIdentity,
};
use cdma_common::lac::paging_messages::MsPageAddress;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const IMSI_DIGITS: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct A21Config {
    /// Local A21 address, normally port 4597.
    pub bind_addr: SocketAddr,
    /// The HRPD AN's A21 address. Port 4597 between hosts, `127.0.0.1:17049`
    /// in the single-host default.
    pub an_addr: SocketAddr,
}

impl Default for A21Config {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], cdma_a21::A21_UDP_PORT)),
            an_addr: SocketAddr::from(([127, 0, 0, 1], cdma_a21::SINGLE_HOST_AN_PORT)),
        }
    }
}

#[derive(Debug)]
struct PageToAn {
    imsi: String,
    lac_pdu: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct HrpdCoord {
    pub cache: HybridIdentityCache,
    pages: mpsc::UnboundedSender<PageToAn>,
    local_addr: SocketAddr,
}

impl HrpdCoord {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn parse_imsi(imsi: &str) -> Option<u64> {
        let s = imsi.trim();
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse::<u64>().ok()
    }

    pub fn is_hrpd_attached(&self, imsi: &str) -> bool {
        Self::parse_imsi(imsi).is_some_and(|imsi| self.cache.is_hrpd_attached(imsi))
    }

    /// Only for an HRPD-present IMSI.
    pub fn offer_page(&self, page_addr: &MsPageAddress, lac_pdu: &[u8]) -> bool {
        let MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            ..
        } = page_addr
        else {
            return false;
        };
        let Some(imsi) = self.cache.snapshot().into_iter().find_map(|cached| {
            let imsi = format!("{:0width$}", cached.imsi, width = IMSI_DIGITS);
            (cdma_common::paging::imsi_s_from_imsi(&imsi) == Some((*imsi_m_s1, *imsi_m_s2)))
                .then_some(imsi)
        }) else {
            return false;
        };
        self.pages
            .send(PageToAn {
                imsi,
                lac_pdu: lac_pdu.to_vec(),
            })
            .is_ok()
    }
}

pub fn spawn_a21_iws(config: A21Config) -> std::io::Result<HrpdCoord> {
    let socket = std::net::UdpSocket::bind(config.bind_addr)?;
    socket.set_nonblocking(true)?;
    let local_addr = socket.local_addr()?;
    let endpoint = A21Endpoint::from_std(socket).map_err(std::io::Error::other)?;
    let cache = HybridIdentityCache::new();
    let (pages, page_rx) = mpsc::unbounded_channel();
    info!(
        "BSC A21: IWS bound on {local_addr} toward AN {}",
        config.an_addr
    );
    tokio::spawn(run_iws(endpoint, config.an_addr, cache.clone(), page_rx));
    Ok(HrpdCoord {
        cache,
        pages,
        local_addr,
    })
}

async fn run_iws(
    endpoint: A21Endpoint,
    an_addr: SocketAddr,
    cache: HybridIdentityCache,
    mut page_rx: mpsc::UnboundedReceiver<PageToAn>,
) {
    let correlation = Arc::new(AtomicU32::new(1));
    loop {
        tokio::select! {
            page = page_rx.recv() => {
                let Some(page) = page else { return };
                // Relaxed: the counter only needs distinct values.
                let correlation_id = correlation.fetch_add(1, Ordering::Relaxed);
                let message = A21Message::air_interface_signaling(
                    correlation_id,
                    MobileIdentity::Imsi(page.imsi.clone()),
                    MessageTransmissionControl {
                        paging_message: true,
                        ..MessageTransmissionControl::default()
                    },
                    vec![page.lac_pdu],
                );
                match endpoint.send(an_addr, &message).await {
                    Ok(()) => info!(
                        "BSC A21: sent page to AN imsi={} correlation_id={correlation_id}",
                        page.imsi
                    ),
                    Err(err) => warn!("BSC A21: page to {an_addr} failed imsi={}: {err}", page.imsi),
                }
            }
            (peer, received) = endpoint.recv() => {
                match received {
                    Ok(message) => handle_from_an(&endpoint, peer, message, &cache).await,
                    Err(err) => warn!("BSC A21: undecodable datagram from {peer}: {err}"),
                }
            }
        }
    }
}

async fn handle_from_an(
    endpoint: &A21Endpoint,
    peer: SocketAddr,
    message: A21Message,
    cache: &HybridIdentityCache,
) {
    match message.message_type {
        A21MessageType::EventNotification => {
            let identity = message.mobile_identity().cloned();
            let imsi = match &identity {
                Some(MobileIdentity::Imsi(imsi)) => HrpdCoord::parse_imsi(imsi),
                _ => None,
            };
            let cause = match (message.event(), imsi) {
                (Some(A21EventKind::PresentInHrpd), Some(imsi)) => {
                    cache.bind(CachedIdentity { imsi });
                    info!("BSC A21: AN reports imsi={imsi} present in HRPD");
                    None
                }
                (
                    Some(A21EventKind::NoLongerPresentInAn | A21EventKind::HrpdPowerDown),
                    Some(imsi),
                ) => {
                    cache.release_by_imsi(imsi);
                    info!("BSC A21: AN reports imsi={imsi} gone from HRPD");
                    None
                }
                (Some(event), Some(_)) => {
                    debug!("BSC A21: event {event:?} from {peer} needs no action");
                    None
                }
                (_, None) => Some(A21Cause::UnknownMobile),
                (None, Some(_)) => Some(A21Cause::Unspecified),
            };
            let ack = A21Message::ack(message.correlation_id(), identity, cause);
            if let Err(err) = endpoint.send(peer, &ack).await {
                warn!("BSC A21: Ack to {peer} failed: {err}");
            }
        }
        A21MessageType::Ack => match message.cause() {
            None => debug!(
                "BSC A21: AN accepted message correlation_id={:?}",
                message.correlation_id()
            ),
            Some(cause) => info!(
                "BSC A21: AN declined message correlation_id={:?} cause={cause:?}; the 1x page still reaches the AT",
                message.correlation_id()
            ),
        },
        other => debug!("BSC A21: ignoring {other:?} from {peer}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const RECV_TIMEOUT: Duration = Duration::from_secs(2);

    #[test]
    fn parse_imsi_accepts_digit_strings_only() {
        assert_eq!(
            HrpdCoord::parse_imsi("001010123456789"),
            Some(1_010_123_456_789u64)
        );
        assert_eq!(HrpdCoord::parse_imsi(""), None);
        assert_eq!(HrpdCoord::parse_imsi("123-456"), None);
    }

    #[tokio::test]
    async fn event_notifications_drive_presence_and_pages_reach_the_an() {
        let an = A21Endpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let coord = spawn_a21_iws(A21Config {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            an_addr: an.local_addr().unwrap(),
        })
        .unwrap();
        let imsi = "310009176936269";
        let iws = coord.local_addr();
        assert!(!coord.offer_page(&page_address(imsi), &[0x11]));
        let present = A21Message::event_notification(
            5,
            MobileIdentity::Imsi(imsi.into()),
            A21EventKind::PresentInHrpd,
        );
        an.send(iws, &present).await.unwrap();
        let (_, ack) = tokio::time::timeout(RECV_TIMEOUT, an.recv()).await.unwrap();
        let ack = ack.unwrap();
        assert_eq!(ack.message_type, A21MessageType::Ack);
        assert_eq!((ack.correlation_id(), ack.cause()), (Some(5), None));
        assert!(coord.is_hrpd_attached(imsi));

        assert!(coord.offer_page(&page_address(imsi), &[0x11, 0x22]));
        let (_, page) = tokio::time::timeout(RECV_TIMEOUT, an.recv()).await.unwrap();
        let page = page.unwrap();
        assert_eq!(page.message_type, A21MessageType::AirInterfaceSignaling);
        assert_eq!(
            page.mobile_identity(),
            Some(&MobileIdentity::Imsi(imsi.into()))
        );
        assert!(page.transmission_control().unwrap().paging_message);
        assert_eq!(
            page.lac_pdus().collect::<Vec<_>>(),
            vec![&[0x11u8, 0x22][..]]
        );

        let gone = A21Message::event_notification(
            6,
            MobileIdentity::Imsi(imsi.into()),
            A21EventKind::NoLongerPresentInAn,
        );
        an.send(iws, &gone).await.unwrap();
        let _ = tokio::time::timeout(RECV_TIMEOUT, an.recv()).await.unwrap();
        assert!(!coord.is_hrpd_attached(imsi));
    }

    fn page_address(imsi: &str) -> MsPageAddress {
        let (imsi_m_s1, imsi_m_s2) = cdma_common::paging::imsi_s_from_imsi(imsi).unwrap();
        MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc: None,
            imsi_11_12: None,
        }
    }
}
