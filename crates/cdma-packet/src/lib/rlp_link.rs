//! RLP for 1x packet data whose PPP runs at the PDSN over A8 (A.S0017-D).

use cdma_common::sch::Rc3FschProfile;

use crate::engine::{RlpBackend, rlp_backend_for_service_option};
use crate::ppp::framing::HdlcDeframer;
use crate::ppp::lcp::LCP_PROTOCOL;
use crate::ppp::vj::{PPP_IP_PROTOCOL, PPP_VJ_COMPRESSED_TCP, PPP_VJ_UNCOMPRESSED_TCP};

/// PPP protocol number for IPv6 (RFC 5072).
const PPP_IPV6_PROTOCOL: u16 = 0x0057;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardFrames {
    pub fch: (Vec<u8>, u32),
    pub sch: Option<(Vec<u8>, u32)>,
}

pub struct RlpLink {
    backend: Box<dyn RlpBackend>,
    sch: Option<Rc3FschProfile>,
    downlink_ppp: HdlcDeframer,
    /// PPP negotiation stays on the fundamental channel until IP flows.
    sch_data_ready: bool,
}

impl RlpLink {
    /// RLP Type 3 for SO 33, Type 1 otherwise.
    pub fn for_service_option(service_option: u32) -> Self {
        Self {
            backend: rlp_backend_for_service_option(service_option),
            sch: None,
            downlink_ppp: HdlcDeframer::new(),
            sch_data_ready: false,
        }
    }

    /// `None` is an erasure.
    pub fn receive(&mut self, uplink: Option<(&[u8], u32)>) -> Option<Vec<u8>> {
        match uplink {
            Some((bits, rate_bps)) => self.backend.receive_frame_bits(bits, rate_bps),
            None => self.backend.receive_frame_bits(&[], 0),
        }
        .filter(|octets| !octets.is_empty())
    }

    pub fn enqueue(&mut self, octets: &[u8]) {
        for packet in self.downlink_ppp.feed(octets) {
            match packet.protocol {
                PPP_IP_PROTOCOL
                | PPP_VJ_COMPRESSED_TCP
                | PPP_VJ_UNCOMPRESSED_TCP
                | PPP_IPV6_PROTOCOL => self.sch_data_ready = true,
                LCP_PROTOCOL => self.sch_data_ready = false,
                _ => {}
            }
        }
        self.backend.enqueue_data(octets);
    }

    pub fn set_sch(&mut self, rate_bps: Option<u32>) -> Result<(), String> {
        self.sch = match rate_bps {
            Some(rate) => Some(
                Rc3FschProfile::from_rate_bps(rate)
                    .ok_or_else(|| format!("no RC3 F-SCH profile for {rate} bps"))?,
            ),
            None => None,
        };
        Ok(())
    }

    /// While the SCH has data, the FCH sends only RLP control and idle frames.
    pub fn next_frames(&mut self) -> ForwardFrames {
        let sch_profile = self.sch.filter(|_| self.sch_data_ready);
        let fch = self.backend.next_frame_bits(sch_profile.is_none());
        let sch = sch_profile.and_then(|profile| {
            self.backend
                .next_sch_frame_bits(profile.info_bits, profile.rate_bps)
        });
        ForwardFrames { fch, sch }
    }

    pub fn is_data_transfer(&self) -> bool {
        self.backend.is_data_transfer()
    }

    pub fn set_log_context(&mut self, context: String) {
        self.backend.set_log_context(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppp::framing::{self, PppPacket};

    fn hdlc(protocol: u16, payload: &[u8]) -> Vec<u8> {
        framing::frame(&PppPacket {
            protocol,
            payload: payload.to_vec(),
        })
    }

    #[test]
    fn supplemental_channel_waits_for_ip_and_rejects_unknown_rates() {
        let mut link = RlpLink::for_service_option(33);
        assert!(link.set_sch(Some(12_345)).is_err());
        link.set_sch(Some(19_200)).unwrap();
        link.enqueue(&hdlc(LCP_PROTOCOL, &[1, 1, 0, 4]));
        assert!(!link.sch_data_ready);
        link.enqueue(&hdlc(PPP_IP_PROTOCOL, &[0x45; 20]));
        assert!(link.sch_data_ready);
        link.enqueue(&hdlc(LCP_PROTOCOL, &[1, 2, 0, 4]));
        assert!(!link.sch_data_ready);
    }

    #[test]
    fn no_supplemental_frame_before_rlp_sync() {
        let mut link = RlpLink::for_service_option(33);
        link.set_sch(Some(19_200)).unwrap();
        link.enqueue(&hdlc(PPP_IP_PROTOCOL, &[0x45; 20]));
        let frames = link.next_frames();
        assert!(frames.sch.is_none());
        assert!(!frames.fch.0.is_empty());
    }

    fn sync_rlp3(link: &mut RlpLink) {
        link.receive(None);
        link.next_frames();
        let sync_ack = crate::rlp3_frames::Rlp3Frame::Control {
            seq: 0,
            control_type: crate::rlp3_frames::Rlp3ControlType::SyncAck,
            init_var: false,
            nak_param_incl: false,
        };
        let bits = sync_ack.encode(crate::rlp3_frames::MuxOption::Odd).unwrap();
        link.receive(Some((&bits, 9_600)));
        for _ in 0..6 {
            link.next_frames();
            link.receive(None);
        }
        assert!(link.is_data_transfer());
    }

    #[test]
    fn data_moves_to_the_supplemental_channel_once_ip_flows() {
        let mut link = RlpLink::for_service_option(33);
        sync_rlp3(&mut link);
        link.set_sch(Some(19_200)).unwrap();
        link.enqueue(&hdlc(PPP_IP_PROTOCOL, &[0x45; 200]));
        let frames = link.next_frames();
        let (sch_bits, sch_rate) = frames.sch.expect("an SCH frame once IP flows");
        assert_eq!(sch_rate, 19_200);
        assert!(!sch_bits.is_empty());

        link.set_sch(None).unwrap();
        assert!(link.next_frames().sch.is_none());
    }
}
