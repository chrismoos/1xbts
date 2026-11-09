use std::net::SocketAddr;
use std::time::{Duration, Instant};

use cdma_abis::bearer::{
    ChannelFamily, Direction, ForwardFchDcchFrame, ForwardSchFrame, FrameContent, TrafficFrame,
};
use cdma_abis::bearer_transport::{BearerRole, BearerTransport, BearerTransportConfig};
use cdma_abis::udp_bearer::UdpBearerDatagram;

const WAIT: Duration = Duration::from_secs(2);

fn forward_fch(frame_content: FrameContent, first_octet: u8) -> ForwardFchDcchFrame {
    let mut information = vec![0u8; 22];
    information[0] = first_octet;
    ForwardFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        fpc_slc: 1,
        fsn: 2,
        fpc_gr: 0,
        rpc_olt: 0,
        frame_content,
        forward_link_information: information,
        message_crc: 0,
    }
}

fn wait_for(transport: &BearerTransport) -> UdpBearerDatagram {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(datagram) = transport.try_recv() {
            return datagram;
        }
        assert!(Instant::now() < deadline, "no bearer frame arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn each_traffic_connection_has_its_own_bts_port() {
    let bsc_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let bsc = BearerTransport::new(&BearerTransportConfig {
        role: BearerRole::Bsc,
        bind_addr: bsc_addr,
        remote_addr: "127.0.0.1:0".parse().unwrap(),
    })
    .unwrap();
    let bts = BearerTransport::new(&BearerTransportConfig {
        role: BearerRole::Bts,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        remote_addr: bsc.local_addr((ChannelFamily::Fch, 0)).unwrap(),
    })
    .unwrap();

    // FCH 12 and SCH 12 are different connections.
    let sch_10 = bts.open_bearer(ChannelFamily::Sch, 10).unwrap();
    let fch_12 = bts.open_bearer(ChannelFamily::Fch, 12).unwrap();
    let sch_12 = bts.open_bearer(ChannelFamily::Sch, 12).unwrap();
    assert_ne!(fch_12, sch_12);
    assert_eq!(bts.open_bearer(ChannelFamily::Fch, 12).unwrap(), fch_12);
    bsc.set_bearer_port(ChannelFamily::Sch, 10, sch_10);
    bsc.set_bearer_port(ChannelFamily::Fch, 12, fch_12);
    bsc.set_bearer_port(ChannelFamily::Sch, 12, sch_12);

    let forward = TrafficFrame::ForwardFchDcch(forward_fch(FrameContent::FchRc3_9600, 0));
    let payload = forward.encode().unwrap();
    bsc.send(&UdpBearerDatagram::from_payload(12, payload.clone()).unwrap())
        .unwrap();
    let received = wait_for(&bts);
    assert_eq!(received.bearer_id, 12);
    assert_eq!(received.payload, payload);
    assert_eq!(
        TrafficFrame::decode_message(&received.payload).unwrap(),
        forward
    );

    let reverse = cdma_abis::bearer::ReverseSchFrame {
        soft_handoff_leg: 0,
        fsn: 1,
        fqi: true,
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content: FrameContent::Sch20msRc3_9600,
        reverse_link_information: vec![1, 2, 3],
        message_crc: 0,
    };
    let payload = TrafficFrame::ReverseSch(reverse).encode().unwrap();
    bts.send(&UdpBearerDatagram::from_payload(10, payload).unwrap())
        .unwrap();
    let received = wait_for(&bsc);
    assert_eq!(received.bearer_id, 10, "attributed by the sending port");
    assert_eq!(received.channel_family, ChannelFamily::Sch);
    assert_eq!(received.direction, Direction::Reverse);

    bts.close_bearer(ChannelFamily::Sch, 10);
    assert!(bts.send(&received).is_err());
}

#[test]
fn forward_sch_frame_starts_with_its_message_type() {
    let frame = ForwardSchFrame {
        fpc_slc: 1,
        fsn: 3,
        fpc_gr: 0x10,
        frame_content: FrameContent::Sch20msRc3_19200,
        forward_link_information: vec![0xaa],
        message_crc: 0x1234,
    };
    let encoded = frame.encode().unwrap();
    assert_eq!(encoded, [0x10, 0x13, 0x10, 0x35, 0xaa, 0x12, 0x34]);
    assert_eq!(
        TrafficFrame::decode_message(&encoded).unwrap(),
        TrafficFrame::ForwardSch(frame)
    );
}

#[test]
fn blank_and_burst_frames_are_signaling() {
    // Rate Set 1: MM=1, TT=0, TM=11.
    assert!(forward_fch(FrameContent::FchRc3_9600, 0b1011_1000).is_blank_and_burst());
    assert!(forward_fch(FrameContent::FchRc1_9600, 0b1011_0000).is_blank_and_burst());
    // Rate Set 2: MM=1, Frame Mode 0011.
    assert!(forward_fch(FrameContent::FchRc2_14400, 0b1001_1100).is_blank_and_burst());

    // Voice, dim-and-burst and secondary traffic are not.
    assert!(!forward_fch(FrameContent::FchRc3_9600, 0b0111_1111).is_blank_and_burst());
    assert!(!forward_fch(FrameContent::FchRc3_9600, 0b1000_0000).is_blank_and_burst());
    assert!(!forward_fch(FrameContent::FchRc3_9600, 0b1111_0000).is_blank_and_burst());
    assert!(!forward_fch(FrameContent::FchRc2_14400, 0b1000_1000).is_blank_and_burst());
    // A lower-rate frame is never blank-and-burst.
    assert!(!forward_fch(FrameContent::FchRc3_4800, 0b1011_0000).is_blank_and_burst());
}
