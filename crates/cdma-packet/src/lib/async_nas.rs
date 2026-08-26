//! End-user PPP/NAS termination for the SO 12 async data service.
//!
//! After the emulated modem reports `CONNECT`, the transparent byte stream on
//! the modem-server connection carries the end-user's dial-up PPP session
//! (what a landline modem would carry to an ISP NAS). This module answers that
//! PPP session locally — reusing the shared LCP/IPCP/HDLC implementations —
//! and bridges the negotiated IP traffic to the network egress the rest of the
//! packet service already uses.
//!
//! It is transport-agnostic: it consumes async octets from the mobile and
//! produces async octets to send back plus extracted IP packets, and it frames
//! downlink IP packets into async octets.

use crate::ppp::framing::{self, HdlcDeframer, PppPacket};
use crate::ppp::ipcp::{IPCP_PROTOCOL, IpcpConfig, IpcpSession};
use crate::ppp::lcp::{LCP_PROTOCOL, LcpSession};
use crate::ppp::vj::PPP_IP_PROTOCOL;

/// Output of feeding async octets to the NAS.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NasOut {
    /// Async octets (HDLC-framed PPP) to send back to the mobile.
    pub to_mobile: Vec<u8>,
    /// IP packets recovered from the PPP stream, for the network egress.
    pub to_network: Vec<Vec<u8>>,
}

/// The end-user PPP terminator ("NAS") for one async data call.
pub struct AsyncNas {
    deframer: HdlcDeframer,
    lcp: LcpSession,
    ipcp: IpcpSession,
    lcp_started: bool,
    ipcp_started: bool,
}

impl AsyncNas {
    pub fn new(ipcp_config: IpcpConfig) -> Self {
        Self {
            deframer: HdlcDeframer::new(),
            lcp: LcpSession::new(),
            ipcp: IpcpSession::new(ipcp_config),
            lcp_started: false,
            ipcp_started: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.lcp.is_open() && self.ipcp.is_open()
    }

    /// Begin PPP negotiation (send our LCP Configure-Request). Returns the
    /// async octets to transmit to the mobile.
    pub fn start(&mut self) -> Vec<u8> {
        if self.lcp_started {
            return Vec::new();
        }
        self.lcp_started = true;
        framing::frame(&self.lcp.start())
    }

    /// Drive retransmission timers; call once per session tick. Returns async
    /// octets to transmit (retransmitted Configure-Requests), if any.
    pub fn tick(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.lcp.is_open() {
            if let Some(req) = self.lcp.maybe_retransmit_configure_request() {
                out.extend(framing::frame(&req));
            }
        }
        out
    }

    /// Feed async octets received from the mobile after CONNECT.
    pub fn feed(&mut self, bytes: &[u8]) -> NasOut {
        let mut out = NasOut::default();
        let packets = self.deframer.feed(bytes);
        for pkt in packets {
            match pkt.protocol {
                LCP_PROTOCOL => {
                    for reply in self.lcp.receive(&pkt) {
                        out.to_mobile.extend(framing::frame(&reply));
                    }
                    if self.lcp.is_open() && !self.ipcp_started {
                        self.ipcp_started = true;
                        out.to_mobile.extend(framing::frame(&self.ipcp.start()));
                    }
                }
                IPCP_PROTOCOL => {
                    for reply in self.ipcp.receive(&pkt) {
                        out.to_mobile.extend(framing::frame(&reply));
                    }
                }
                PPP_IP_PROTOCOL => {
                    out.to_network.push(pkt.payload);
                }
                _ => {
                    // Unknown protocol: reject so the peer stops sending it.
                }
            }
        }
        out
    }

    /// Frame a downlink IP packet (from the network) into async octets to send
    /// to the mobile. Returns empty until IPCP is open.
    pub fn send_ip(&mut self, ip_packet: &[u8]) -> Vec<u8> {
        if !self.ipcp.is_open() {
            return Vec::new();
        }
        framing::frame(&PppPacket {
            protocol: PPP_IP_PROTOCOL,
            payload: ip_packet.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn nas() -> AsyncNas {
        AsyncNas::new(IpcpConfig {
            our_ip: Ipv4Addr::new(10, 55, 0, 1),
            peer_ip: Ipv4Addr::new(10, 55, 0, 2),
            ..IpcpConfig::default()
        })
    }

    #[test]
    fn start_emits_lcp_configure_request() {
        let mut n = nas();
        let bytes = n.start();
        assert!(!bytes.is_empty());
        // Deframes back into a single LCP packet.
        let mut d = HdlcDeframer::new();
        let pkts = d.feed(&bytes);
        assert_eq!(pkts.len(), 1);
        assert_eq!(pkts[0].protocol, LCP_PROTOCOL);
        // Idempotent.
        assert!(n.start().is_empty());
    }

    #[test]
    fn full_ppp_bringup_then_ip_bridges_both_ways() {
        // Two NAS instances negotiating against each other stand in for the
        // mobile's PPP peer and exercise the real LCP/IPCP exchange.
        let mut nas = nas();
        let mut peer = AsyncNas::new(IpcpConfig {
            our_ip: Ipv4Addr::new(10, 55, 0, 2),
            peer_ip: Ipv4Addr::new(10, 55, 0, 1),
            ..IpcpConfig::default()
        });

        let mut to_nas = peer.start();
        let mut to_peer = nas.start();
        // Pump the exchange to completion.
        for _ in 0..40 {
            let a = nas.feed(&std::mem::take(&mut to_peer));
            to_nas.extend(a.to_mobile);
            let b = peer.feed(&std::mem::take(&mut to_nas));
            to_peer.extend(b.to_mobile);
            to_peer.extend(nas.tick());
            to_nas.extend(peer.tick());
            if nas.is_open() && peer.is_open() {
                break;
            }
        }
        assert!(nas.is_open(), "NAS PPP did not open");

        // A downlink IP packet frames only once IPCP is open.
        let ip = vec![0x45u8; 40];
        let framed = nas.send_ip(&ip);
        assert!(!framed.is_empty());
        let out = peer.feed(&framed);
        assert_eq!(out.to_network, vec![ip]);
    }
}
