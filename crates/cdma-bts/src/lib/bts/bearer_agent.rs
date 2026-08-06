//! BTS-side bearer agent.
//!
//! Uses a [`BearerTransport`] for UDP I/O. Spawns threads to:
//! - Receive forward bearer frames from the BSC and deliver them to
//!   traffic channels via the [`TrafficResourceService`].
//! - Forward reverse bearer datagrams (from the RX pipeline) to the BSC.

use std::sync::Arc;
use std::sync::mpsc::Receiver as StdReceiver;
use std::thread;

use log::{debug, warn};

use cdma_abis::bearer::{FrameContent, TrafficFrame};
use cdma_abis::bearer_transport::BearerTransport;
use cdma_abis::udp_bearer::UdpBearerDatagram;

use crate::channels::ftch::{TrafficFrame as Rc1TrafficFrame, TrafficRate};
use crate::channels::ftch_rc2::{Rc2Rate, TrafficFrameRc2};
use crate::channels::ftch_rc3::TrafficFrameRc3;

use super::{TrafficChannelWrapper, TrafficResourceService};

const FLAG_SIGNALING_QUEUE: u8 = 0x01;

/// Unpack packed bytes into individual bits (MSB first), padded/truncated to
/// exactly `target_bits`.
fn unpack_bytes_to_bits(packed: &[u8], target_bits: usize) -> Vec<u8> {
    let mut bits = Vec::with_capacity(target_bits);
    for &byte in packed {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits.truncate(target_bits);
    bits.resize(target_bits, 0);
    bits
}

/// Start the BTS-side bearer agent.
///
/// The caller provides a pre-constructed [`BearerTransport`] (bound to the
/// BTS bearer address, remote = BSC bearer address) and the channel that
/// the RX pipeline pushes reverse bearer datagrams into.
pub fn spawn_bts_bearer_agent(
    transport: Arc<BearerTransport>,
    controller: Arc<TrafficResourceService>,
    reverse_bearer_rx: StdReceiver<UdpBearerDatagram>,
) {
    let transport_rx = transport.clone();
    thread::spawn(move || {
        loop {
            let datagrams = transport_rx.drain();
            if datagrams.is_empty() {
                thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            log::trace!(
                "BTS bearer agent: received {} forward datagram(s)",
                datagrams.len()
            );
            for datagram in datagrams {
                let is_signaling = datagram.flags & FLAG_SIGNALING_QUEUE != 0;
                let walsh_code = datagram.bearer_id as u8;
                let frame = match TrafficFrame::decode(
                    datagram.channel_family,
                    datagram.direction,
                    &datagram.payload,
                ) {
                    Ok(f) => f,
                    Err(e) => {
                        warn!("BTS bearer agent: payload decode failed: {e}");
                        continue;
                    }
                };
                log::trace!(
                    "BTS bearer agent: delivering walsh={} signaling={} family={:?}",
                    walsh_code,
                    is_signaling,
                    datagram.channel_family
                );
                if let Err(e) = deliver_forward_frame(&controller, walsh_code, frame, is_signaling)
                {
                    warn!("BTS bearer agent: delivery failed: {e}");
                }
            }
        }
    });

    thread::spawn(move || {
        while let Ok(datagram) = reverse_bearer_rx.recv() {
            if let Err(e) = transport.send(&datagram) {
                warn!("BTS bearer agent: reverse TX failed: {e}");
            }
        }
    });
}

/// Deliver a forward bearer frame to the appropriate BTS traffic channel.
pub fn deliver_forward_frame(
    controller: &Arc<TrafficResourceService>,
    walsh_code: u8,
    frame: TrafficFrame,
    is_signaling: bool,
) -> Result<(), String> {
    let slot = controller
        .traffic_channels_pool()
        .lookup(walsh_code)
        .ok_or_else(|| format!("unknown bearer walsh={}", walsh_code))?;
    let channel = slot.channel.clone();

    match frame {
        TrafficFrame::ForwardFchDcch(payload) => {
            let info_len = payload.forward_link_information.len();
            match &channel {
                TrafficChannelWrapper::Rc1(ch) => {
                    let rate = decode_frame_content_rate(payload.frame_content)?;
                    if is_signaling {
                        let bits = unpack_bytes_to_bits(
                            &payload.forward_link_information,
                            TrafficRate::Full.info_bits(),
                        );
                        ch.channel.send_signaling_bits(bits);
                        log::trace!(
                            "BTS bearer: queued signaling frame walsh={} len={} queue_len={}",
                            walsh_code,
                            info_len,
                            ch.channel.queue_len()
                        );
                    } else {
                        ch.channel.send_frame(Rc1TrafficFrame {
                            data: payload.forward_link_information,
                            rate,
                        });
                    }
                }
                TrafficChannelWrapper::Rc3(ch) => {
                    let rate = decode_frame_content_rate(payload.frame_content)?;
                    if is_signaling {
                        let bits = unpack_bytes_to_bits(
                            &payload.forward_link_information,
                            TrafficRate::Full.info_bits(),
                        );
                        ch.channel.send_signaling_bits(bits);
                        log::trace!(
                            "BTS bearer: queued signaling frame walsh={} len={} queue_len={}",
                            walsh_code,
                            info_len,
                            ch.channel.queue_len()
                        );
                    } else {
                        ch.channel.send_frame(TrafficFrameRc3 {
                            data: payload.forward_link_information,
                            rate,
                        });
                    }
                }
                TrafficChannelWrapper::Rc2(ch) => {
                    let rc2_rate = decode_frame_content_rc2_rate(payload.frame_content)?;
                    if is_signaling {
                        let bits = unpack_bytes_to_bits(
                            &payload.forward_link_information,
                            Rc2Rate::Full.info_bits(),
                        );
                        ch.channel.send_signaling_bits(bits);
                        debug!(
                            "BTS bearer: queued RC2 signaling frame walsh={} len={} queue_len={}",
                            walsh_code,
                            info_len,
                            ch.channel.queue_len()
                        );
                    } else {
                        ch.channel.send_frame(TrafficFrameRc2 {
                            data: payload.forward_link_information,
                            rate: rc2_rate,
                        });
                    }
                }
                TrafficChannelWrapper::SchRc3(_) => {
                    return Err(format!("walsh={} is SCH, not FCH/DCCH", walsh_code));
                }
            }
        }
        TrafficFrame::ForwardSch(payload) => match &channel {
            TrafficChannelWrapper::SchRc3(ch) => {
                ch.channel.send_frame(payload.forward_link_information);
            }
            _ => return Err(format!("walsh={} is FCH, not SCH", walsh_code)),
        },
        TrafficFrame::ReverseFchDcch(_) | TrafficFrame::ReverseSch(_) => {
            return Err("cannot deliver reverse frames to BTS TX".into());
        }
    }

    Ok(())
}

fn decode_frame_content_rate(frame_content: FrameContent) -> Result<TrafficRate, String> {
    match frame_content {
        FrameContent::FchRc1_9600 | FrameContent::FchRc3_9600 => Ok(TrafficRate::Full),
        FrameContent::FchRc1_4800 | FrameContent::FchRc3_4800 => Ok(TrafficRate::Half),
        FrameContent::FchRc1_2400 | FrameContent::FchRc3_2700 => Ok(TrafficRate::Quarter),
        FrameContent::FchRc1_1200 | FrameContent::FchRc3_1500 => Ok(TrafficRate::Eighth),
        other => Err(format!(
            "unsupported bearer frame_content rate 0x{:02X}",
            other.value()
        )),
    }
}

/// Map a bearer frame tag to an RC2 rate.
fn decode_frame_content_rc2_rate(frame_content: FrameContent) -> Result<Rc2Rate, String> {
    match frame_content {
        FrameContent::FchRc2_14400 => Ok(Rc2Rate::Full),
        FrameContent::FchRc2_7200 => Ok(Rc2Rate::Half),
        FrameContent::FchRc2_3600 => Ok(Rc2Rate::Quarter),
        FrameContent::FchRc2_1800 => Ok(Rc2Rate::Eighth),
        other => Err(format!(
            "unsupported RC2 bearer frame_content rate 0x{:02X}",
            other.value()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bts::TrafficResourceService;
    use cdma_abis::bearer::{ChannelFamily, ForwardFchDcchFrame};
    use cdma_common::phy::long_code::LongCodeGenerator;

    fn rc2_traffic_frame(frame_content: FrameContent, info_bytes: Vec<u8>) -> TrafficFrame {
        TrafficFrame::ForwardFchDcch(ForwardFchDcchFrame {
            channel_family: ChannelFamily::Fch,
            fpc_slc: 1,
            fsn: 0,
            fpc_gr: 0,
            rpc_olt: 0,
            frame_content,
            forward_link_information: info_bytes,
            message_crc: 0,
        })
    }

    #[test]
    fn rc2_bearer_frames_enqueue_on_forward_traffic_channel() {
        let controller = Arc::new(TrafficResourceService::new());
        let (walsh_code, channel_ref) = controller
            .allocate_rc2_traffic(LongCodeGenerator::new_traffic_channel(0), 0, 0)
            .expect("rc2 allocation");

        let rates = [
            (FrameContent::FchRc2_14400, Rc2Rate::Full),
            (FrameContent::FchRc2_7200, Rc2Rate::Half),
            (FrameContent::FchRc2_3600, Rc2Rate::Quarter),
            (FrameContent::FchRc2_1800, Rc2Rate::Eighth),
        ];

        for (i, (fc, rate)) in rates.iter().enumerate() {
            let info_bits = rate.info_bits();
            let payload = vec![1; info_bits];
            let frame = rc2_traffic_frame(*fc, payload);
            deliver_forward_frame(&controller, walsh_code, frame, false)
                .expect("rc2 traffic dispatch");
            assert_eq!(channel_ref.channel.queue_len(), i + 1);
        }
    }

    #[test]
    fn rc2_bearer_signaling_routes_to_signaling_queue() {
        let controller = Arc::new(TrafficResourceService::new());
        let (walsh_code, channel_ref) = controller
            .allocate_rc2_traffic(LongCodeGenerator::new_traffic_channel(0), 0, 0)
            .expect("rc2 allocation");

        let frame = rc2_traffic_frame(FrameContent::FchRc2_14400, vec![0xFF_u8; 34]);
        deliver_forward_frame(&controller, walsh_code, frame, true)
            .expect("rc2 signaling dispatch");

        assert_eq!(channel_ref.channel.queue_len(), 1);
        assert!(channel_ref.channel.last_enqueue_at().is_some());
    }

    #[test]
    fn rc2_bearer_rejects_non_rc2_frame_content() {
        let controller = Arc::new(TrafficResourceService::new());
        let (walsh_code, _channel_ref) = controller
            .allocate_rc2_traffic(LongCodeGenerator::new_traffic_channel(0), 0, 0)
            .expect("rc2 allocation");

        let frame = rc2_traffic_frame(FrameContent::FchRc3_9600, vec![0u8; 22]);
        let err = deliver_forward_frame(&controller, walsh_code, frame, false)
            .expect_err("rc3 frame content on rc2 channel must fail");
        assert!(
            err.contains("unsupported RC2 bearer frame_content"),
            "{}",
            err
        );
    }
}
