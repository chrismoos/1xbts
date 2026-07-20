use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use cdma_a8::{
    ApplySessionOutcome, BearerEndpoint, BearerProfile, BearerSession, BearerTable,
    BearerTransportConfig, BearerTransportMode, Error, GrePacket, RebindMode, RebindOutcome,
    SessionSnapshot, SessionStats, UdpGreEndpoint,
};

#[test]
fn gre_octet_stream_roundtrip() {
    let packet = GrePacket::octet_stream(0x01020304, Some(9), [0xaa, 0xbb, 0xcc]);
    let encoded = packet.encode().unwrap();
    assert_eq!(
        encoded,
        vec![
            0x30, 0x00, 0x88, 0x81, 0x01, 0x02, 0x03, 0x04, 0x00, 0x00, 0x00, 0x09, 0xaa, 0xbb,
            0xcc,
        ]
    );
    assert_eq!(GrePacket::decode(&encoded).unwrap(), packet);
}

#[test]
fn gre_ppp_roundtrip() {
    let packet = GrePacket::ppp(0x01020304, [0xaa, 0xbb, 0xcc]);
    let encoded = packet.encode().unwrap();
    assert_eq!(
        encoded,
        vec![
            0x20, 0x00, 0x88, 0x0b, 0x01, 0x02, 0x03, 0x04, 0xaa, 0xbb, 0xcc
        ]
    );
    assert_eq!(GrePacket::decode(&encoded).unwrap(), packet);
}

#[test]
fn bearer_transport_config_models_raw_and_udp_exact_gre() {
    let raw = BearerTransportConfig::raw_gre();
    assert_eq!(raw.mode, BearerTransportMode::RawGre);
    raw.validate("a8").unwrap();

    let bind: SocketAddr = "127.0.0.1:17040".parse().unwrap();
    let peer: SocketAddr = "127.0.0.1:17041".parse().unwrap();
    let udp = BearerTransportConfig::udp_encapsulated_gre(bind, peer);
    assert_eq!(udp.mode, BearerTransportMode::UdpEncapsulatedGre);
    assert_eq!(udp.udp_bind_addr, Some(bind));
    assert_eq!(udp.udp_peer_addr, Some(peer));
    udp.validate("a8").unwrap();
}

#[test]
fn bearer_transport_config_rejects_mixed_outer_modes() {
    let mut raw = BearerTransportConfig::raw_gre();
    raw.udp_bind_addr = Some("127.0.0.1:17040".parse().unwrap());
    assert!(raw.validate("a8").unwrap_err().contains("raw_gre"));

    let mut udp = BearerTransportConfig {
        mode: BearerTransportMode::UdpEncapsulatedGre,
        ..BearerTransportConfig::raw_gre()
    };
    assert!(udp.validate("a8").unwrap_err().contains("udp_bind_addr"));
    udp.udp_bind_addr = Some("127.0.0.1:17040".parse().unwrap());
    assert!(udp.validate("a8").unwrap_err().contains("udp_peer_addr"));
}

#[test]
fn udp_gre_endpoint_carries_exact_encoded_gre_packet() {
    let left_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let right_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let left_addr = left_socket.local_addr().unwrap();
    let right_addr = right_socket.local_addr().unwrap();
    left_socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();
    right_socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();

    let left = UdpGreEndpoint::from_socket(left_socket, right_addr);
    let right = UdpGreEndpoint::from_socket(right_socket, left_addr);
    let packet = GrePacket::octet_stream(0x0102_0304, Some(17), [0xde, 0xad, 0xbe, 0xef]);
    let encoded = packet.encode().unwrap();

    assert_eq!(left.send_gre_packet(&packet).unwrap(), encoded.len());
    let mut buf = [0_u8; 128];
    let (decoded, from) = right.recv_gre_packet(&mut buf).unwrap();
    assert_eq!(from, left_addr);
    assert_eq!(decoded, packet);

    assert_eq!(right.send_wire_packet(&encoded).unwrap(), encoded.len());
    let (decoded, from) = left.recv_gre_packet(&mut buf).unwrap();
    assert_eq!(from, right_addr);
    assert_eq!(decoded, packet);
}

#[tokio::test]
async fn tokio_udp_gre_endpoint_waits_for_readiness_and_drains_nonblocking() {
    let left_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let right_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let left_addr = left_socket.local_addr().unwrap();
    let right_addr = right_socket.local_addr().unwrap();
    let left = UdpGreEndpoint::from_socket(left_socket, right_addr)
        .into_tokio()
        .unwrap();
    let right = UdpGreEndpoint::from_socket(right_socket, left_addr)
        .into_tokio()
        .unwrap();
    let packet = GrePacket::octet_stream(0x0102_0304, Some(23), [0xca, 0xfe]);

    let mut buf = [0_u8; 128];
    assert!(matches!(
        right.try_recv_gre_packet(&mut buf),
        Err(Error::UdpTransport(err)) if err.to_ascii_lowercase().contains("temporarily unavailable")
            || err.to_ascii_lowercase().contains("would block")
    ));
    left.send_gre_packet(&packet).await.unwrap();
    tokio::time::timeout(Duration::from_millis(250), right.readable())
        .await
        .unwrap()
        .unwrap();
    let (decoded, from) = right.try_recv_gre_packet(&mut buf).unwrap();
    assert_eq!(from, left_addr);
    assert_eq!(decoded, packet);

    right.send_gre_packet(&packet).await.unwrap();
    let (decoded, from) =
        tokio::time::timeout(Duration::from_millis(250), left.recv_gre_packet(&mut buf))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from, right_addr);
    assert_eq!(decoded, packet);
}

#[test]
fn gre_packet_encode_rejects_missing_required_fields() {
    let missing_key = GrePacket {
        key_present: true,
        sequence_present: false,
        protocol_type: 0x8881,
        key: None,
        sequence_number: None,
        payload: vec![],
    };
    assert_eq!(missing_key.encode(), Err(Error::MissingSessionKey));

    let missing_sequence = GrePacket {
        key_present: true,
        sequence_present: true,
        protocol_type: 0x8881,
        key: Some(0x01020304),
        sequence_number: None,
        payload: vec![],
    };
    assert_eq!(missing_sequence.encode(), Err(Error::MissingSequenceNumber));
}

#[test]
fn apply_session_reports_create_unchanged_and_rebind() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let endpoint_b = BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 9]);

    assert_eq!(
        table
            .apply_session(BearerSession::new(0x01020304, endpoint_a))
            .unwrap(),
        ApplySessionOutcome::Created
    );
    assert_eq!(
        table
            .apply_session(BearerSession::new(0x01020304, endpoint_a))
            .unwrap(),
        ApplySessionOutcome::Unchanged
    );
    assert_eq!(
        table
            .apply_session(BearerSession::new(0x01020304, endpoint_b))
            .unwrap(),
        ApplySessionOutcome::Rebound {
            previous_endpoint: endpoint_a,
            previous_inbound_session_key: 0x01020304,
            previous_outbound_session_key: 0x01020304,
            previous_profile: BearerProfile::standard_packet_data(),
        }
    );
    assert_eq!(table.session(0x01020304).unwrap().endpoint, endpoint_b);
}

#[test]
fn apply_session_rejects_overlapping_transition() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([10, 0, 1, 1], [10, 0, 1, 2]);
    let endpoint_b = BearerEndpoint::new([10, 0, 1, 1], [10, 0, 1, 3]);
    let endpoint_c = BearerEndpoint::new([10, 0, 1, 1], [10, 0, 1, 4]);
    table
        .create_session(BearerSession::new(0x51515151, endpoint_a))
        .unwrap();
    table
        .rebind_session_with_mode(0x51515151, endpoint_b, RebindMode::Mobility)
        .unwrap();

    assert_eq!(
        table
            .apply_session(BearerSession::new(0x51515151, endpoint_c))
            .unwrap_err(),
        Error::TransitionInProgress {
            session_id: 0x51515151
        }
    );
}

#[test]
fn bearer_table_tracks_standard_packet_data_lifecycle() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 2]);
    table
        .create_session(BearerSession::new(0x01020304, endpoint))
        .unwrap();

    let outbound = table
        .build_outbound_packet(0x01020304, [0xaa, 0xbb, 0xcc, 0xdd])
        .unwrap();
    assert_eq!(outbound.endpoint, endpoint);
    assert_eq!(outbound.session_id, 0x01020304);
    assert_eq!(outbound.gre_key, 0x01020304);
    assert_eq!(outbound.tx_ordinal, 1);
    assert_eq!(outbound.gre_sequence, Some(0));
    assert_eq!(outbound.payload_len, 4);
    assert_eq!(
        outbound.wire_bytes[..12],
        [0x30, 0x00, 0x88, 0x81, 0x01, 0x02, 0x03, 0x04, 0, 0, 0, 0]
    );

    let inbound = table
        .decode_for_session(endpoint, &outbound.wire_bytes)
        .unwrap();
    assert_eq!(inbound.session_id, 0x01020304);
    assert_eq!(inbound.gre_key, 0x01020304);
    assert_eq!(inbound.rx_ordinal, 1);
    assert_eq!(inbound.gre_sequence, Some(0));
    assert_eq!(inbound.payload, vec![0xaa, 0xbb, 0xcc, 0xdd]);

    let snapshot = table.session_snapshot(0x01020304).unwrap();
    assert_eq!(
        snapshot,
        SessionSnapshot {
            session: BearerSession::new(0x01020304, endpoint),
            stats: SessionStats {
                tx_packets: 1,
                tx_bytes: 4,
                rx_packets: 1,
                rx_bytes: 4,
                endpoint_mismatch_packets: 0,
                dropped_packets: 0,
                transition_rx_packets: 0,
                last_tx_ordinal: 1,
                last_rx_ordinal: 1,
                last_tx_sequence: Some(0),
                last_rx_sequence: Some(0),
                duplicate_sequence_packets: 0,
                reordered_sequence_packets: 0,
                sequence_gap_events: 0,
            },
            transition: None,
        }
    );
}

#[test]
fn bearer_table_supports_directional_keys() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([10, 20, 0, 1], [10, 20, 0, 2]);
    table
        .create_session(BearerSession::with_directional_keys(
            0x11110000,
            0x11112222,
            0x33334444,
            endpoint,
            BearerProfile::standard_packet_data(),
        ))
        .unwrap();

    let outbound = table
        .build_outbound_packet(0x11110000, [1, 2, 3, 4])
        .unwrap();
    assert_eq!(outbound.session_id, 0x11110000);
    assert_eq!(outbound.gre_key, 0x33334444);
    assert_eq!(
        GrePacket::decode(&outbound.wire_bytes).unwrap().key,
        Some(0x33334444)
    );

    let inbound = table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x11112222, Some(0), [1, 2, 3, 4])
                .encode()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(inbound.session_id, 0x11110000);
    assert_eq!(inbound.gre_key, 0x11112222);
    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::octet_stream(0x33334444, Some(1), [1, 2, 3, 4])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::UnknownInboundSessionKey(0x33334444)
    );
}

#[test]
fn ppp_compatibility_profile_accepts_unsequenced_ppp_gre() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([10, 10, 0, 1], [10, 10, 0, 2]);
    let profile = BearerProfile::ppp_compatibility();
    table
        .create_session(BearerSession::with_profile(0x21222324, endpoint, profile))
        .unwrap();

    let outbound = table.build_outbound_packet(0x21222324, [1, 2, 3]).unwrap();
    assert_eq!(outbound.gre_sequence, None);
    assert_eq!(
        GrePacket::decode(&outbound.wire_bytes)
            .unwrap()
            .protocol_type,
        0x880b
    );
    let inbound = table
        .decode_for_session(
            endpoint,
            &GrePacket::ppp(0x21222324, [1, 2, 3]).encode().unwrap(),
        )
        .unwrap();
    assert_eq!(inbound.gre_sequence, None);
}

#[test]
fn capability_profile_is_retained_in_session_snapshot() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([10, 30, 0, 1], [10, 30, 0, 2]);
    let profile = BearerProfile {
        packet_boundary_supported: true,
        gre_segmentation_supported: true,
        short_data_indication_supported: true,
        flow_control_supported: true,
        ..BearerProfile::standard_packet_data()
    };
    table
        .create_session(BearerSession::with_profile(0x31323334, endpoint, profile))
        .unwrap();

    let snapshot = table.session_snapshot(0x31323334).unwrap();
    assert_eq!(snapshot.session.profile, profile);
    assert!(snapshot.session.profile.has_unresolved_wire_attributes());
}

#[test]
fn a8_accepts_ipv6_endpoints() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new_v6(
        [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
    );
    table
        .create_session(BearerSession::new(0x61626364, endpoint))
        .unwrap();

    let outbound = table
        .build_outbound_packet(0x61626364, [0xde, 0xad])
        .unwrap();
    let inbound = table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x61626364, Some(0), [0xde, 0xad])
                .encode()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(outbound.endpoint, endpoint);
    assert_eq!(inbound.endpoint, endpoint);
    assert_eq!(
        endpoint.local_ip,
        IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1))
    );
}

#[test]
fn mixed_address_families_are_rejected() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::from_ip(
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    );
    assert_eq!(
        table
            .create_session(BearerSession::new(0x41424344, endpoint))
            .unwrap_err(),
        Error::AddressFamilyMismatch {
            session_id: 0x41424344
        }
    );
}

#[test]
fn duplicate_inbound_keys_are_rejected() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([192, 0, 2, 1], [192, 0, 2, 2]);
    let endpoint_b = BearerEndpoint::new([192, 0, 2, 1], [192, 0, 2, 3]);
    table
        .create_session(BearerSession::with_directional_keys(
            0x11111111,
            0xabcdef01,
            0xabcdef02,
            endpoint_a,
            BearerProfile::standard_packet_data(),
        ))
        .unwrap();

    assert_eq!(
        table
            .create_session(BearerSession::with_directional_keys(
                0x22222222,
                0xabcdef01,
                0xabcdef03,
                endpoint_b,
                BearerProfile::standard_packet_data(),
            ))
            .unwrap_err(),
        Error::DuplicateInboundSessionKey(0xabcdef01)
    );
}

#[test]
fn bearer_table_supports_idempotent_remove_and_helper_methods() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([192, 0, 2, 1], [192, 0, 2, 2]);
    table
        .create_session(BearerSession::new(0x11121314, endpoint))
        .unwrap();

    assert_eq!(table.session_count(), 1);
    assert!(table.has_session(0x11121314));
    assert!(!table.finalize_rebind(0x11121314).unwrap());
    assert_eq!(
        table.remove_session_if_present(0x11121314),
        Some(BearerSession::new(0x11121314, endpoint))
    );
    assert_eq!(table.remove_session_if_present(0x11121314), None);
    assert_eq!(table.stats().sessions_removed, 1);
}

#[test]
fn apply_session_rekeys_and_reprofiles_existing_binding() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([192, 0, 2, 11], [192, 0, 2, 22]);
    let initial = BearerSession::with_directional_keys(
        0x50515253,
        0x11112222,
        0x33334444,
        endpoint,
        BearerProfile::standard_packet_data(),
    );
    let updated_profile = BearerProfile {
        packet_boundary_supported: true,
        gre_segmentation_supported: true,
        short_data_indication_supported: true,
        flow_control_supported: true,
        ..BearerProfile::ppp_compatibility()
    };
    let updated = BearerSession::with_directional_keys(
        0x50515253,
        0x55556666,
        0x77778888,
        endpoint,
        updated_profile,
    );

    table.create_session(initial).unwrap();
    assert_eq!(
        table.apply_session(updated).unwrap(),
        ApplySessionOutcome::Rebound {
            previous_endpoint: endpoint,
            previous_inbound_session_key: 0x11112222,
            previous_outbound_session_key: 0x33334444,
            previous_profile: BearerProfile::standard_packet_data(),
        }
    );

    let outbound = table
        .build_outbound_packet(0x50515253, [0xaa, 0xbb])
        .unwrap();
    assert_eq!(outbound.gre_key, 0x77778888);
    assert_eq!(outbound.gre_sequence, None);
    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::ppp(0x55556666, [0xaa, 0xbb]).encode().unwrap()
            )
            .unwrap()
            .gre_key,
        0x55556666
    );
    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::octet_stream(0x11112222, Some(0), [0xaa, 0xbb])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::UnknownInboundSessionKey(0x11112222)
    );

    let snapshot = table.session_snapshot(0x50515253).unwrap();
    assert_eq!(snapshot.session, updated);
    assert!(snapshot.session.profile.has_unresolved_wire_attributes());
}

#[test]
fn create_and_teardown_reject_invalid_requests() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 16, 1, 1], [172, 16, 1, 2]);
    table
        .create_session(BearerSession::new(0x23232323, endpoint))
        .unwrap();

    assert_eq!(
        table
            .create_session(BearerSession::new(0x23232323, endpoint))
            .unwrap_err(),
        Error::DuplicateSession(0x23232323)
    );
    assert_eq!(
        table.remove_session(0x01010101).unwrap_err(),
        Error::UnknownSession(0x01010101)
    );
    assert_eq!(
        table.finalize_rebind(0x01010101).unwrap_err(),
        Error::UnknownSession(0x01010101)
    );
}

#[test]
fn dormant_rebind_cuts_over_immediately() {
    let mut table = BearerTable::new();
    let old_endpoint = BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 2]);
    let new_endpoint = BearerEndpoint::new([10, 0, 0, 1], [10, 0, 0, 3]);
    table
        .create_session(BearerSession::new(0x01020304, old_endpoint))
        .unwrap();

    assert_eq!(
        table
            .rebind_session_with_mode(0x01020304, new_endpoint, RebindMode::DormantResume)
            .unwrap(),
        RebindOutcome::Rebound {
            previous_endpoint: old_endpoint,
            mode: RebindMode::DormantResume,
        }
    );

    let packet = GrePacket::octet_stream(0x01020304, Some(0), [1, 2, 3])
        .encode()
        .unwrap();
    assert_eq!(
        table.decode_for_session(old_endpoint, &packet).unwrap_err(),
        Error::EndpointMismatch {
            session_id: 0x01020304
        }
    );
    assert!(
        table
            .session_snapshot(0x01020304)
            .unwrap()
            .transition
            .is_none()
    );
}

#[test]
fn mobility_rebind_accepts_previous_endpoint_until_finalized() {
    let mut table = BearerTable::new();
    let old_endpoint = BearerEndpoint::new([192, 0, 2, 1], [192, 0, 2, 2]);
    let new_endpoint = BearerEndpoint::new([192, 0, 2, 1], [192, 0, 2, 3]);
    table
        .create_session(BearerSession::new(0x11121314, old_endpoint))
        .unwrap();
    table
        .rebind_session_with_mode(0x11121314, new_endpoint, RebindMode::Mobility)
        .unwrap();

    let packet = GrePacket::octet_stream(0x11121314, Some(4), [9, 8, 7])
        .encode()
        .unwrap();
    let inbound = table.decode_for_session(old_endpoint, &packet).unwrap();
    assert_eq!(inbound.endpoint, old_endpoint);
    assert_eq!(inbound.gre_sequence, Some(4));
    assert_eq!(table.stats().transition_rx_packets, 1);
    assert!(table.finalize_rebind(0x11121314).unwrap());
    assert_eq!(table.stats().transitions_completed, 1);
    assert_eq!(
        table.decode_for_session(old_endpoint, &packet).unwrap_err(),
        Error::EndpointMismatch {
            session_id: 0x11121314
        }
    );
}

#[test]
fn hard_handoff_rebind_auto_completes_on_new_endpoint_traffic() {
    let mut table = BearerTable::new();
    let old_endpoint = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 2]);
    let new_endpoint = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 3]);
    table
        .create_session(BearerSession::new(0x51525354, old_endpoint))
        .unwrap();
    table
        .rebind_session_with_mode(0x51525354, new_endpoint, RebindMode::HardHandoff)
        .unwrap();

    let packet = GrePacket::octet_stream(0x51525354, Some(7), [1, 2, 3])
        .encode()
        .unwrap();
    let inbound = table.decode_for_session(new_endpoint, &packet).unwrap();
    assert_eq!(inbound.endpoint, new_endpoint);
    assert!(
        table
            .session_snapshot(0x51525354)
            .unwrap()
            .transition
            .is_none()
    );
    assert_eq!(table.stats().transitions_completed, 1);
}

#[test]
fn hard_handoff_accepts_previous_endpoint_until_cutover_packet_arrives() {
    let mut table = BearerTable::new();
    let old_endpoint = BearerEndpoint::new([198, 51, 100, 11], [198, 51, 100, 12]);
    let new_endpoint = BearerEndpoint::new([198, 51, 100, 11], [198, 51, 100, 13]);
    table
        .create_session(BearerSession::new(0x61626365, old_endpoint))
        .unwrap();
    table
        .rebind_session_with_mode(0x61626365, new_endpoint, RebindMode::HardHandoff)
        .unwrap();

    let draining_packet = GrePacket::octet_stream(0x61626365, Some(0), [9, 9])
        .encode()
        .unwrap();
    let cutover_packet = GrePacket::octet_stream(0x61626365, Some(1), [8, 8])
        .encode()
        .unwrap();

    assert_eq!(
        table
            .decode_for_session(old_endpoint, &draining_packet)
            .unwrap()
            .endpoint,
        old_endpoint
    );
    assert_eq!(table.stats().transition_rx_packets, 1);
    assert_eq!(
        table
            .decode_for_session(new_endpoint, &cutover_packet)
            .unwrap()
            .endpoint,
        new_endpoint
    );
    assert!(
        table
            .session_snapshot(0x61626365)
            .unwrap()
            .transition
            .is_none()
    );
    assert_eq!(
        table
            .decode_for_session(old_endpoint, &draining_packet)
            .unwrap_err(),
        Error::EndpointMismatch {
            session_id: 0x61626365
        }
    );
}

#[test]
fn transition_rejects_second_rebind_until_completed() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([203, 0, 113, 1], [203, 0, 113, 2]);
    let endpoint_b = BearerEndpoint::new([203, 0, 113, 1], [203, 0, 113, 3]);
    let endpoint_c = BearerEndpoint::new([203, 0, 113, 1], [203, 0, 113, 4]);
    table
        .create_session(BearerSession::new(0x61626364, endpoint_a))
        .unwrap();
    table
        .rebind_session_with_mode(0x61626364, endpoint_b, RebindMode::Mobility)
        .unwrap();

    assert_eq!(
        table
            .rebind_session_with_mode(0x61626364, endpoint_c, RebindMode::HardHandoff)
            .unwrap_err(),
        Error::TransitionInProgress {
            session_id: 0x61626364
        }
    );
}

#[test]
fn missing_session_key_is_counted_as_malformed() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([203, 0, 113, 44], [203, 0, 113, 55]);
    table
        .create_session(BearerSession::new(0x41424345, endpoint))
        .unwrap();

    assert_eq!(
        table
            .decode_for_session(endpoint, &[0x00, 0x00, 0x88, 0x81, 0xde, 0xad])
            .unwrap_err(),
        Error::MissingSessionKey
    );
    assert_eq!(table.stats().malformed_packets, 1);
}

#[test]
fn bearer_table_rejects_unknown_session_and_endpoint_mismatch() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 20]);
    let wrong_endpoint = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 21]);

    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::octet_stream(0x01020304, Some(0), [5, 6, 7])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::UnknownInboundSessionKey(0x01020304)
    );
    assert_eq!(table.stats().unknown_session_packets, 1);

    table
        .create_session(BearerSession::new(0x01020304, endpoint))
        .unwrap();
    assert_eq!(
        table
            .decode_for_session(
                wrong_endpoint,
                &GrePacket::octet_stream(0x01020304, Some(0), [5, 6, 7])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::EndpointMismatch {
            session_id: 0x01020304
        }
    );
    assert_eq!(table.stats().endpoint_mismatch_packets, 1);
}

#[test]
fn bearer_table_counts_malformed_gre_inputs() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 20]);
    table
        .create_session(BearerSession::new(0x01020304, endpoint))
        .unwrap();

    assert_eq!(
        table
            .decode_for_session(endpoint, &[0x20, 0x00, 0x88])
            .unwrap_err(),
        Error::Truncated {
            needed: 4,
            actual: 3
        }
    );
    assert_eq!(
        table
            .decode_for_session(endpoint, &[0x80, 0x00, 0x88, 0x81])
            .unwrap_err(),
        Error::UnsupportedGreFlags(0x8000)
    );
    assert_eq!(table.stats().malformed_packets, 2);
}

#[test]
fn bearer_table_enforces_protocol_and_sequence_requirements() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 2]);
    table
        .create_session(BearerSession::new(0x01020304, endpoint))
        .unwrap();

    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::ppp(0x01020304, [1, 2]).encode().unwrap()
            )
            .unwrap_err(),
        Error::InvalidProtocolType(0x880b)
    );
    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::octet_stream(0x01020304, None, [1, 2])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::MissingSequenceNumber
    );
    assert_eq!(table.stats().malformed_packets, 2);
}

#[test]
fn packet_ordinals_and_sequence_counters_advance_monotonically() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 2]);
    table
        .create_session(BearerSession::new(0x11223344, endpoint))
        .unwrap();

    let first = table.build_outbound_packet(0x11223344, [0x01]).unwrap();
    let second = table.build_outbound_packet(0x11223344, [0x02]).unwrap();
    assert_eq!(first.tx_ordinal, 1);
    assert_eq!(first.gre_sequence, Some(0));
    assert_eq!(second.tx_ordinal, 2);
    assert_eq!(second.gre_sequence, Some(1));

    table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x11223344, Some(0), [0x01])
                .encode()
                .unwrap(),
        )
        .unwrap();
    table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x11223344, Some(2), [0x02])
                .encode()
                .unwrap(),
        )
        .unwrap();
    table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x11223344, Some(2), [0x03])
                .encode()
                .unwrap(),
        )
        .unwrap();
    table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x11223344, Some(1), [0x04])
                .encode()
                .unwrap(),
        )
        .unwrap();

    let stats = table.session_snapshot(0x11223344).unwrap().stats;
    assert_eq!(stats.last_tx_ordinal, 2);
    assert_eq!(stats.last_rx_ordinal, 4);
    assert_eq!(stats.last_tx_sequence, Some(1));
    assert_eq!(stats.last_rx_sequence, Some(1));
    assert_eq!(stats.sequence_gap_events, 1);
    assert_eq!(stats.duplicate_sequence_packets, 1);
    assert_eq!(stats.reordered_sequence_packets, 1);
}

#[test]
fn sequence_wrap_is_treated_as_forward_progress() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([198, 51, 100, 9], [198, 51, 100, 10]);
    table
        .create_session(BearerSession::new(0x91929394, endpoint))
        .unwrap();

    table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x91929394, Some(u32::MAX), [0xaa])
                .encode()
                .unwrap(),
        )
        .unwrap();
    table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x91929394, Some(0), [0xbb])
                .encode()
                .unwrap(),
        )
        .unwrap();

    let stats = table.session_snapshot(0x91929394).unwrap().stats;
    assert_eq!(stats.last_rx_sequence, Some(0));
    assert_eq!(stats.duplicate_sequence_packets, 0);
    assert_eq!(stats.reordered_sequence_packets, 0);
    assert_eq!(stats.sequence_gap_events, 0);
}
