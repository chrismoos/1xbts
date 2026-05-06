use cdma_a10::{
    ApplySessionOutcome, BearerEndpoint, BearerProfile, BearerSession, BearerTable, Error,
    GrePacket, RebindMode, RebindOutcome, SessionSnapshot, SessionStats, SessionTransition,
};

#[test]
fn a10_reuses_gre_packet_codec() {
    let packet = GrePacket::octet_stream(0x11121314, Some(7), [1, 2, 3, 4]);
    let encoded = packet.encode().unwrap();
    assert_eq!(GrePacket::decode(&encoded).unwrap(), packet);
}

#[test]
fn a10_apply_session_reports_control_plane_outcomes() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 2]);
    let endpoint_b = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 99]);

    assert_eq!(
        table
            .apply_session(BearerSession::new(0x11121314, endpoint_a))
            .unwrap(),
        ApplySessionOutcome::Created
    );
    assert_eq!(
        table
            .apply_session(BearerSession::new(0x11121314, endpoint_a))
            .unwrap(),
        ApplySessionOutcome::Unchanged
    );
    assert_eq!(
        table
            .apply_session(BearerSession::new(0x11121314, endpoint_b))
            .unwrap(),
        ApplySessionOutcome::Rebound {
            previous_endpoint: endpoint_a,
            previous_inbound_session_key: 0x11121314,
            previous_outbound_session_key: 0x11121314,
            previous_profile: BearerProfile::standard_packet_data(),
        }
    );
}

#[test]
fn a10_apply_session_rejects_overlapping_transition() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([198, 51, 101, 1], [198, 51, 101, 2]);
    let endpoint_b = BearerEndpoint::new([198, 51, 101, 1], [198, 51, 101, 3]);
    let endpoint_c = BearerEndpoint::new([198, 51, 101, 1], [198, 51, 101, 4]);
    table
        .create_session(BearerSession::new(0x11111111, endpoint_a))
        .unwrap();
    table
        .rebind_session_with_mode(0x11111111, endpoint_b, RebindMode::Mobility)
        .unwrap();

    assert_eq!(
        table
            .apply_session(BearerSession::new(0x11111111, endpoint_c))
            .unwrap_err(),
        Error::TransitionInProgress {
            session_id: 0x11111111
        }
    );
}

#[test]
fn a10_bearer_table_roundtrip_and_snapshot() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([198, 51, 100, 1], [198, 51, 100, 2]);
    table
        .create_session(BearerSession::new(0x11121314, endpoint))
        .unwrap();

    let outbound = table
        .build_outbound_packet(0x11121314, [1, 2, 3, 4])
        .unwrap();
    assert_eq!(outbound.session_id, 0x11121314);
    assert_eq!(outbound.gre_key, 0x11121314);
    assert_eq!(outbound.tx_ordinal, 1);
    assert_eq!(outbound.gre_sequence, Some(0));
    assert_eq!(
        outbound.wire_bytes[..12],
        [0x30, 0x00, 0x88, 0x81, 0x11, 0x12, 0x13, 0x14, 0, 0, 0, 0]
    );

    let inbound = table
        .decode_for_session(endpoint, &outbound.wire_bytes)
        .unwrap();
    assert_eq!(inbound.session_id, 0x11121314);
    assert_eq!(inbound.gre_key, 0x11121314);
    assert_eq!(inbound.rx_ordinal, 1);
    assert_eq!(inbound.gre_sequence, Some(0));
    assert_eq!(
        table.session_snapshot(0x11121314).unwrap(),
        SessionSnapshot {
            session: BearerSession::new(0x11121314, endpoint),
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
fn a10_supports_directional_keys_for_session_id_version_one_style_bindings() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 18, 0, 1], [172, 18, 0, 2]);
    table
        .create_session(BearerSession::with_directional_keys(
            0x71727374,
            0x01020304,
            0x05060708,
            endpoint,
            BearerProfile::standard_packet_data(),
        ))
        .unwrap();

    let outbound = table.build_outbound_packet(0x71727374, [7, 8, 9]).unwrap();
    assert_eq!(outbound.gre_key, 0x05060708);
    assert_eq!(
        GrePacket::decode(&outbound.wire_bytes).unwrap().key,
        Some(0x05060708)
    );

    let inbound = table
        .decode_for_session(
            endpoint,
            &GrePacket::octet_stream(0x01020304, Some(0), [7, 8, 9])
                .encode()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(inbound.session_id, 0x71727374);
    assert_eq!(inbound.gre_key, 0x01020304);

    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::octet_stream(0x05060708, Some(1), [7, 8, 9])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::UnknownInboundSessionKey(0x05060708)
    );
}

#[test]
fn a10_ppp_compatibility_profile_is_explicit_opt_in() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 18, 0, 1], [172, 18, 0, 2]);
    let profile = BearerProfile::ppp_compatibility();
    table
        .create_session(BearerSession::with_profile(0x71727374, endpoint, profile))
        .unwrap();

    let inbound = table
        .decode_for_session(
            endpoint,
            &GrePacket::ppp(0x71727374, [7, 8, 9]).encode().unwrap(),
        )
        .unwrap();
    assert_eq!(inbound.gre_sequence, None);
}

#[test]
fn a10_capability_profile_is_retained_in_session_snapshot() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 18, 0, 1], [172, 18, 0, 2]);
    let profile = BearerProfile {
        packet_boundary_supported: true,
        gre_segmentation_supported: true,
        short_data_indication_supported: true,
        flow_control_supported: true,
        ..BearerProfile::standard_packet_data()
    };
    table
        .create_session(BearerSession::with_profile(0x81828384, endpoint, profile))
        .unwrap();

    let snapshot = table.session_snapshot(0x81828384).unwrap();
    assert_eq!(snapshot.session.profile, profile);
    assert!(snapshot.session.profile.has_unresolved_wire_attributes());
}

#[test]
fn a10_apply_session_rekeys_and_reprofiles_existing_binding() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 18, 9, 1], [172, 18, 9, 2]);
    let initial = BearerSession::with_directional_keys(
        0x41424344,
        0x01010101,
        0x02020202,
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
        0x41424344,
        0x03030303,
        0x04040404,
        endpoint,
        updated_profile,
    );

    table.create_session(initial).unwrap();
    assert_eq!(
        table.apply_session(updated).unwrap(),
        ApplySessionOutcome::Rebound {
            previous_endpoint: endpoint,
            previous_inbound_session_key: 0x01010101,
            previous_outbound_session_key: 0x02020202,
            previous_profile: BearerProfile::standard_packet_data(),
        }
    );

    let outbound = table.build_outbound_packet(0x41424344, [1, 2]).unwrap();
    assert_eq!(outbound.gre_key, 0x04040404);
    assert_eq!(outbound.gre_sequence, None);
    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::ppp(0x03030303, [1, 2]).encode().unwrap()
            )
            .unwrap()
            .gre_key,
        0x03030303
    );
    assert_eq!(
        table
            .decode_for_session(
                endpoint,
                &GrePacket::octet_stream(0x01010101, Some(0), [1, 2])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::UnknownInboundSessionKey(0x01010101)
    );

    let snapshot = table.session_snapshot(0x41424344).unwrap();
    assert_eq!(snapshot.session, updated);
    assert!(snapshot.session.profile.has_unresolved_wire_attributes());
}

#[test]
fn a10_helper_methods_and_invalid_teardown_paths_work() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 18, 1, 1], [172, 18, 1, 2]);
    table
        .create_session(BearerSession::new(0x72737475, endpoint))
        .unwrap();

    assert_eq!(table.session_count(), 1);
    assert!(table.has_session(0x72737475));
    assert!(!table.finalize_rebind(0x72737475).unwrap());
    assert_eq!(
        table
            .create_session(BearerSession::new(0x72737475, endpoint))
            .unwrap_err(),
        Error::DuplicateSession(0x72737475)
    );
    assert_eq!(
        table.remove_session(0x02020202).unwrap_err(),
        Error::UnknownSession(0x02020202)
    );
    assert_eq!(
        table.finalize_rebind(0x02020202).unwrap_err(),
        Error::UnknownSession(0x02020202)
    );
}

#[test]
fn a10_missing_session_key_is_counted_as_malformed() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([172, 18, 10, 1], [172, 18, 10, 2]);
    table
        .create_session(BearerSession::new(0x72737476, endpoint))
        .unwrap();

    assert_eq!(
        table
            .decode_for_session(endpoint, &[0x00, 0x00, 0x88, 0x81, 0xca, 0xfe])
            .unwrap_err(),
        Error::MissingSessionKey
    );
    assert_eq!(table.stats().malformed_packets, 1);
}

#[test]
fn a10_rejects_duplicate_inbound_keys() {
    let mut table = BearerTable::new();
    let endpoint_a = BearerEndpoint::new([172, 18, 2, 1], [172, 18, 2, 2]);
    let endpoint_b = BearerEndpoint::new([172, 18, 2, 1], [172, 18, 2, 3]);
    table
        .create_session(BearerSession::with_directional_keys(
            0x01010101,
            0xaaaa0001,
            0xaaaa0002,
            endpoint_a,
            BearerProfile::standard_packet_data(),
        ))
        .unwrap();

    assert_eq!(
        table
            .create_session(BearerSession::with_directional_keys(
                0x02020202,
                0xaaaa0001,
                0xaaaa0003,
                endpoint_b,
                BearerProfile::standard_packet_data(),
            ))
            .unwrap_err(),
        Error::DuplicateInboundSessionKey(0xaaaa0001)
    );
}

#[test]
fn a10_mobility_and_hard_handoff_rebinds_work() {
    let mut table = BearerTable::new();
    let old_endpoint = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 20]);
    let new_endpoint = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 21]);
    table
        .create_session(BearerSession::new(0x01020304, old_endpoint))
        .unwrap();

    assert_eq!(
        table
            .rebind_session_with_mode(0x01020304, new_endpoint, RebindMode::Mobility)
            .unwrap(),
        RebindOutcome::Rebound {
            previous_endpoint: old_endpoint,
            mode: RebindMode::Mobility,
        }
    );
    assert_eq!(
        table.session_snapshot(0x01020304).unwrap().transition,
        Some(SessionTransition {
            mode: RebindMode::Mobility,
            previous_endpoint: old_endpoint,
        })
    );
    table
        .decode_for_session(
            old_endpoint,
            &GrePacket::octet_stream(0x01020304, Some(9), [5, 6, 7])
                .encode()
                .unwrap(),
        )
        .unwrap();
    assert!(table.finalize_rebind(0x01020304).unwrap());

    let handoff_target = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 22]);
    table
        .rebind_session_with_mode(0x01020304, handoff_target, RebindMode::HardHandoff)
        .unwrap();
    table
        .decode_for_session(
            handoff_target,
            &GrePacket::octet_stream(0x01020304, Some(10), [8, 9, 10])
                .encode()
                .unwrap(),
        )
        .unwrap();
    assert!(
        table
            .session_snapshot(0x01020304)
            .unwrap()
            .transition
            .is_none()
    );
}

#[test]
fn a10_rejects_overlapping_rebinds_and_endpoint_mismatch() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([192, 0, 2, 10], [192, 0, 2, 20]);
    let next_endpoint = BearerEndpoint::new([192, 0, 2, 10], [192, 0, 2, 21]);
    let wrong_endpoint = BearerEndpoint::new([192, 0, 2, 10], [192, 0, 2, 22]);
    table
        .create_session(BearerSession::new(0x51525354, endpoint))
        .unwrap();
    table
        .rebind_session_with_mode(0x51525354, next_endpoint, RebindMode::Mobility)
        .unwrap();

    assert_eq!(
        table
            .rebind_session_with_mode(0x51525354, wrong_endpoint, RebindMode::HardHandoff)
            .unwrap_err(),
        Error::TransitionInProgress {
            session_id: 0x51525354
        }
    );
    assert_eq!(
        table
            .decode_for_session(
                wrong_endpoint,
                &GrePacket::octet_stream(0x51525354, Some(0), [1])
                    .encode()
                    .unwrap(),
            )
            .unwrap_err(),
        Error::EndpointMismatch {
            session_id: 0x51525354
        }
    );
}

#[test]
fn a10_hard_handoff_accepts_previous_endpoint_until_cutover_packet_arrives() {
    let mut table = BearerTable::new();
    let old_endpoint = BearerEndpoint::new([192, 0, 2, 30], [192, 0, 2, 40]);
    let new_endpoint = BearerEndpoint::new([192, 0, 2, 30], [192, 0, 2, 41]);
    table
        .create_session(BearerSession::new(0x91929395, old_endpoint))
        .unwrap();
    table
        .rebind_session_with_mode(0x91929395, new_endpoint, RebindMode::HardHandoff)
        .unwrap();

    let draining_packet = GrePacket::octet_stream(0x91929395, Some(0), [0xaa])
        .encode()
        .unwrap();
    let cutover_packet = GrePacket::octet_stream(0x91929395, Some(1), [0xbb])
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
            .session_snapshot(0x91929395)
            .unwrap()
            .transition
            .is_none()
    );
    assert_eq!(
        table
            .decode_for_session(old_endpoint, &draining_packet)
            .unwrap_err(),
        Error::EndpointMismatch {
            session_id: 0x91929395
        }
    );
}

#[test]
fn a10_rejects_unknown_session_and_malformed_inputs() {
    let mut table = BearerTable::new();
    let endpoint = BearerEndpoint::new([203, 0, 113, 10], [203, 0, 113, 20]);

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
fn a10_packet_ordinals_and_sequence_counters_advance() {
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
