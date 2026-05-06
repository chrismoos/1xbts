use cdma_a9::{
    A8TrafficId, A9Indicators, AccessLinkPhase, AlConnectedAckMessage, AlConnectedMessage,
    AlDisconnectedAckMessage, AlDisconnectedMessage, BsServicePhase, BsServiceRequestMessage,
    BsServiceRequestState, BsServiceResponseMessage, BscId, CallConnectionReference, CauseValue,
    ConRef, ConnectA8Message, CorrelationId, DataCount, DisconnectA8Message, Meid,
    PendingRequestIdentity, ProcedureEngine, ProcedureEvent, ProcedureMessage, ProcedureRole,
    ReleaseA8CompleteMessage, ReleaseA8Message, ServiceOptionValue, SessionPhase,
    SessionUpdatePhase, SetupA8Message, ShortDataAckMessage, ShortDataDeliveryMessage,
    ShortDataPhase, SoftwareVersion, UpdateA8AckMessage, UpdateA8Message, VersionInfoAckMessage,
    VersionInfoMessage, VersionInfoPhase,
};

fn setup_message() -> SetupA8Message {
    SetupA8Message {
        call_connection_reference: Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708)),
        correlation_id: Some(CorrelationId([9, 10, 11, 12])),
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        con_ref: ConRef(0x10),
        quality_of_service_parameters: None,
        bsc_id: BscId(vec![0xaa, 0xbb]),
        a8_traffic_id: A8TrafficId::gre_ppp(0x01020304, [192, 0, 2, 10]),
        service_option: ServiceOptionValue(0x0021),
        a9_indicators: A9Indicators {
            packet_boundary_supported: false,
            gre_segmentation_supported: false,
            sdb_supported: false,
            ccpd_mode: false,
            data_ready: true,
            handoff: false,
        },
        user_zone_id: None,
    }
}

fn connect_response(setup: &SetupA8Message) -> ConnectA8Message {
    ConnectA8Message {
        call_connection_reference: setup.call_connection_reference,
        correlation_id: setup.correlation_id,
        imsi: setup.imsi.clone(),
        esn: setup.esn,
        meid: None,
        con_ref: setup.con_ref,
        a8_traffic_id: setup.a8_traffic_id.clone(),
        cause: CauseValue(0x13),
        pdsn_ip_address: None,
    }
}

fn software_version(label: &str) -> SoftwareVersion {
    SoftwareVersion {
        ios_major_revision_level: 1,
        ios_minor_revision_level: 0,
        ios_point_release_level: 7,
        manufacturer_carrier_software_information: label.into(),
    }
}

#[test]
fn session_lifecycle_tracks_setup_connect_access_link_and_release() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);

    let created = engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    assert_eq!(
        created,
        ProcedureEvent::SessionCreated {
            con_ref: vec![0x10],
            phase: SessionPhase::SetupPending {
                initiated_by_local: true,
                pending: PendingRequestIdentity {
                    call_connection_reference: setup.call_connection_reference,
                    correlation_id: setup.correlation_id,
                },
            },
        }
    );
    assert_eq!(
        engine.session(&[0x10]).unwrap().phase(),
        SessionPhase::SetupPending {
            initiated_by_local: true,
            pending: PendingRequestIdentity {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        }
    );
    assert_eq!(
        engine.session(&[0x10]).unwrap().access_link_phase(),
        AccessLinkPhase::Disconnected
    );

    let connected = engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();
    assert_eq!(
        connected,
        ProcedureEvent::SessionUpdated {
            con_ref: vec![0x10],
            phase: SessionPhase::Connected,
            access_link_phase: AccessLinkPhase::Disconnected,
        }
    );

    engine
        .apply_outbound(ProcedureMessage::AlConnected(AlConnectedMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            pdsn_ip_address: None,
        }))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::AlConnectedAck(AlConnectedAckMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
        }))
        .unwrap();
    assert_eq!(
        engine
            .session_by_traffic_id(&setup.a8_traffic_id)
            .unwrap()
            .access_link_phase(),
        AccessLinkPhase::Connected
    );

    engine
        .apply_outbound(ProcedureMessage::AlDisconnected(AlDisconnectedMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            a8_traffic_id: setup.a8_traffic_id.clone(),
        }))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::AlDisconnectedAck(
            AlDisconnectedAckMessage {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        ))
        .unwrap();
    assert_eq!(
        engine
            .session_by_traffic_id(&setup.a8_traffic_id)
            .unwrap()
            .access_link_phase(),
        AccessLinkPhase::Disconnected
    );

    engine
        .apply_inbound(ProcedureMessage::DisconnectA8(DisconnectA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: setup.imsi.clone(),
            esn: setup.esn,
            meid: None,
            con_ref: setup.con_ref,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            cause: CauseValue(0x10),
        }))
        .unwrap();
    assert_eq!(
        engine.session(&[0x10]).unwrap().phase(),
        SessionPhase::DisconnectPending {
            initiated_by_local: false,
            pending: PendingRequestIdentity {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        }
    );

    engine
        .apply_outbound(ProcedureMessage::ReleaseA8(ReleaseA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: setup.con_ref,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            cause: CauseValue(0x14),
        }))
        .unwrap();
    let released = engine
        .apply_inbound(ProcedureMessage::ReleaseA8Complete(
            ReleaseA8CompleteMessage {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        ))
        .unwrap();
    assert_eq!(
        released,
        ProcedureEvent::SessionReleased {
            con_ref: vec![0x10],
        }
    );
    assert!(engine.session(&[0x10]).is_none());
}

#[test]
fn pcf_lifecycle_tracks_directional_mirror_flow() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Pcf);

    engine
        .apply_inbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::AlConnected(AlConnectedMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            pdsn_ip_address: None,
        }))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::AlConnectedAck(AlConnectedAckMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
        }))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::AlDisconnected(AlDisconnectedMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            a8_traffic_id: setup.a8_traffic_id.clone(),
        }))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::AlDisconnectedAck(
            AlDisconnectedAckMessage {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        ))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::DisconnectA8(DisconnectA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: setup.imsi.clone(),
            esn: setup.esn,
            meid: None,
            con_ref: setup.con_ref,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            cause: CauseValue(0x20),
        }))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ReleaseA8(ReleaseA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: setup.con_ref,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            cause: CauseValue(0x20),
        }))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::ReleaseA8Complete(
            ReleaseA8CompleteMessage {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        ))
        .unwrap();
    assert!(engine.session(&[0x10]).is_none());
}

#[test]
fn rejects_duplicate_setup_by_con_ref_and_traffic_id() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Pcf);
    engine
        .apply_inbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();

    let duplicate_con_ref = engine
        .apply_inbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap_err();
    assert_eq!(duplicate_con_ref, cdma_a9::Error::DuplicateSession);

    let mut traffic_collision = setup.clone();
    traffic_collision.con_ref = ConRef(0x33);
    let duplicate_traffic = engine
        .apply_inbound(ProcedureMessage::SetupA8(traffic_collision))
        .unwrap_err();
    assert_eq!(
        duplicate_traffic,
        cdma_a9::Error::DuplicateTrafficId(setup.a8_traffic_id.key)
    );
}

#[test]
fn rejects_connect_for_unknown_or_mismatched_session() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);

    let unknown = engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap_err();
    assert_eq!(unknown, cdma_a9::Error::UnknownSession);

    engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    let mismatched = engine
        .apply_inbound(ProcedureMessage::ConnectA8(ConnectA8Message {
            a8_traffic_id: A8TrafficId::gre_ppp(0x05060708, [192, 0, 2, 11]),
            ..connect_response(&setup)
        }))
        .unwrap_err();
    assert_eq!(
        mismatched,
        cdma_a9::Error::TrafficIdMismatch {
            expected: setup.a8_traffic_id.key,
            actual: 0x05060708,
        }
    );

    let missing_correlation = engine
        .apply_inbound(ProcedureMessage::ConnectA8(ConnectA8Message {
            correlation_id: None,
            ..connect_response(&setup)
        }))
        .unwrap();
    assert_eq!(
        missing_correlation,
        ProcedureEvent::SessionUpdated {
            con_ref: vec![0x10],
            phase: SessionPhase::Connected,
            access_link_phase: AccessLinkPhase::Disconnected,
        }
    );
}

#[test]
fn rejects_directional_routing_violations_for_bsc_and_pcf_roles() {
    let setup = setup_message();

    let mut bsc = ProcedureEngine::new(ProcedureRole::Bsc);
    let bsc_outbound_error = bsc
        .apply_outbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap_err();
    assert_eq!(
        bsc_outbound_error,
        cdma_a9::Error::InvalidProcedureDirection {
            message_type: cdma_a9::MessageType::ConnectA8,
            state: "message direction is not valid for outbound routing on this role",
        }
    );

    let bsc_inbound_error = bsc
        .apply_inbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap_err();
    assert_eq!(
        bsc_inbound_error,
        cdma_a9::Error::InvalidProcedureDirection {
            message_type: cdma_a9::MessageType::SetupA8,
            state: "message direction is not valid for inbound routing on this role",
        }
    );

    let mut pcf = ProcedureEngine::new(ProcedureRole::Pcf);
    let pcf_outbound_error = pcf
        .apply_outbound(ProcedureMessage::ReleaseA8(ReleaseA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: setup.con_ref,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            cause: CauseValue(0x14),
        }))
        .unwrap_err();
    assert_eq!(
        pcf_outbound_error,
        cdma_a9::Error::InvalidProcedureDirection {
            message_type: cdma_a9::MessageType::ReleaseA8,
            state: "message direction is not valid for outbound routing on this role",
        }
    );
}

#[test]
fn rejects_invalid_access_link_ack_sequence_and_unknown_traffic_id() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();

    let ack_error = engine
        .apply_inbound(ProcedureMessage::AlConnectedAck(AlConnectedAckMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
        }))
        .unwrap_err();
    assert_eq!(
        ack_error,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::AlConnectedAck,
            state: "access link is not awaiting connect acknowledgement",
        }
    );

    let mut pcf = ProcedureEngine::new(ProcedureRole::Pcf);
    let traffic_id = A8TrafficId::gre_ppp(0x05060708, [192, 0, 2, 11]);
    let connect_error = pcf
        .apply_inbound(ProcedureMessage::AlConnected(AlConnectedMessage {
            call_connection_reference: None,
            correlation_id: None,
            a8_traffic_id: traffic_id.clone(),
            pdsn_ip_address: None,
        }))
        .unwrap_err();
    assert_eq!(connect_error, cdma_a9::Error::UnknownTrafficId(0x05060708));
}

#[test]
fn rejects_access_link_ack_in_same_direction_as_request() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::AlConnected(AlConnectedMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            pdsn_ip_address: None,
        }))
        .unwrap();

    let error = engine
        .apply_outbound(ProcedureMessage::AlConnectedAck(AlConnectedAckMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
        }))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidProcedureDirection {
            message_type: cdma_a9::MessageType::AlConnectedAck,
            state: "message direction is not valid for outbound routing on this role",
        }
    );
}

#[test]
fn bs_service_requires_request_response_ordering() {
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);

    let response_without_request = engine
        .apply_outbound(ProcedureMessage::BsServiceResponse(
            BsServiceResponseMessage {
                correlation_id: None,
                cause: Some(CauseValue(0x11)),
            },
        ))
        .unwrap_err();
    assert_eq!(
        response_without_request,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::BsServiceResponse,
            state: "no BS service request is pending",
        }
    );

    let updated = engine
        .apply_inbound(ProcedureMessage::BsServiceRequest(
            BsServiceRequestMessage {
                correlation_id: Some(CorrelationId([9, 10, 11, 12])),
                imsi: Some("123456789012345".into()),
                esn: None,
                meid: None,
                service_option: ServiceOptionValue(0x0021),
                data_count: DataCount(0x0010),
            },
        ))
        .unwrap();
    assert_eq!(
        updated,
        ProcedureEvent::BsServiceUpdated(BsServicePhase::RequestPending {
            request: BsServiceRequestState {
                correlation_id: Some(CorrelationId([9, 10, 11, 12])),
                imsi: Some("123456789012345".into()),
                esn: None,
                meid: None,
                service_option: ServiceOptionValue(0x0021),
                data_count: DataCount(0x0010),
            },
            initiated_by_local: false,
        })
    );

    let cleared = engine
        .apply_outbound(ProcedureMessage::BsServiceResponse(
            BsServiceResponseMessage {
                correlation_id: Some(CorrelationId([9, 10, 11, 12])),
                cause: Some(CauseValue(0x11)),
            },
        ))
        .unwrap();
    assert_eq!(
        cleared,
        ProcedureEvent::BsServiceUpdated(BsServicePhase::Idle)
    );
    assert_eq!(*engine.bs_service_phase(), BsServicePhase::Idle);
}

#[test]
fn bs_service_response_requires_opposite_direction_and_matching_correlation() {
    let mut engine = ProcedureEngine::new(ProcedureRole::Pcf);
    engine
        .apply_outbound(ProcedureMessage::BsServiceRequest(
            BsServiceRequestMessage {
                correlation_id: Some(CorrelationId([9, 10, 11, 12])),
                imsi: Some("123456789012345".into()),
                esn: None,
                meid: None,
                service_option: ServiceOptionValue(0x0021),
                data_count: DataCount(0x0010),
            },
        ))
        .unwrap();

    let mismatch = engine
        .apply_inbound(ProcedureMessage::BsServiceResponse(
            BsServiceResponseMessage {
                correlation_id: Some(CorrelationId([1, 2, 3, 4])),
                cause: None,
            },
        ))
        .unwrap_err();
    assert_eq!(
        mismatch,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::BsServiceResponse,
            state: "BS service response correlation does not match the pending request",
        }
    );

    engine
        .apply_inbound(ProcedureMessage::BsServiceResponse(
            BsServiceResponseMessage {
                correlation_id: Some(CorrelationId([9, 10, 11, 12])),
                cause: None,
            },
        ))
        .unwrap();
    assert_eq!(*engine.bs_service_phase(), BsServicePhase::Idle);
}

#[test]
fn bs_service_response_rejects_unexpected_correlation_when_request_has_none() {
    let mut engine = ProcedureEngine::new(ProcedureRole::Pcf);
    engine
        .apply_outbound(ProcedureMessage::BsServiceRequest(
            BsServiceRequestMessage {
                correlation_id: None,
                imsi: Some("123456789012345".into()),
                esn: None,
                meid: None,
                service_option: ServiceOptionValue(0x0021),
                data_count: DataCount(0x0010),
            },
        ))
        .unwrap();

    let error = engine
        .apply_inbound(ProcedureMessage::BsServiceResponse(
            BsServiceResponseMessage {
                correlation_id: Some(CorrelationId([1, 2, 3, 4])),
                cause: None,
            },
        ))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::BsServiceResponse,
            state: "BS service response correlation is present without a pending request correlation",
        }
    );
}

#[test]
fn release_complete_requires_release_pending() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();

    let error = engine
        .apply_inbound(ProcedureMessage::ReleaseA8Complete(
            ReleaseA8CompleteMessage {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
        ))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::ReleaseA8Complete,
            state: "session is not awaiting release complete",
        }
    );
}

#[test]
fn release_complete_rejects_conflicting_identifier_resolution() {
    let setup = setup_message();
    let mut first = setup.clone();
    first.con_ref = ConRef(0x10);
    let mut second = setup.clone();
    second.con_ref = ConRef(0x11);
    second.a8_traffic_id = A8TrafficId::gre_ppp(0x01020305, [192, 0, 2, 11]);
    second.call_connection_reference =
        Some(CallConnectionReference::new(0x0807, 0x0605, 0x04030201));
    second.correlation_id = Some(CorrelationId([1, 2, 3, 4]));

    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    engine
        .apply_outbound(ProcedureMessage::SetupA8(first.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&first)))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::ReleaseA8(ReleaseA8Message {
            call_connection_reference: first.call_connection_reference,
            correlation_id: first.correlation_id,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: first.con_ref,
            a8_traffic_id: first.a8_traffic_id.clone(),
            cause: CauseValue(0x14),
        }))
        .unwrap();

    engine
        .apply_outbound(ProcedureMessage::SetupA8(second.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&second)))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::ReleaseA8(ReleaseA8Message {
            call_connection_reference: second.call_connection_reference,
            correlation_id: second.correlation_id,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: second.con_ref,
            a8_traffic_id: second.a8_traffic_id.clone(),
            cause: CauseValue(0x14),
        }))
        .unwrap();

    let error = engine
        .apply_inbound(ProcedureMessage::ReleaseA8Complete(
            ReleaseA8CompleteMessage {
                call_connection_reference: first.call_connection_reference,
                correlation_id: second.correlation_id,
            },
        ))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::ReleaseA8Complete,
            state: "response identifiers resolve to different sessions",
        }
    );
}

#[test]
fn version_info_tracks_tvers9_and_ignores_unsolicited_ack() {
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    let outbound = VersionInfoMessage {
        correlation_id: Some(CorrelationId([1, 2, 3, 4])),
        cause: Some(CauseValue(0x07)),
        software_version: Some(software_version("bsc-reset")),
    };

    let pending = engine
        .apply_outbound(ProcedureMessage::VersionInfo(outbound))
        .unwrap();
    assert_eq!(
        pending,
        ProcedureEvent::VersionInfoUpdated(VersionInfoPhase::RequestPending {
            request: cdma_a9::VersionInfoRequestState {
                correlation_id: Some(CorrelationId([1, 2, 3, 4])),
            },
            initiated_by_local: true,
        })
    );

    let cleared = engine
        .apply_inbound(ProcedureMessage::VersionInfoAck(VersionInfoAckMessage {
            correlation_id: Some(CorrelationId([1, 2, 3, 4])),
            software_version: Some(software_version("pcf-reset")),
        }))
        .unwrap();
    assert_eq!(
        cleared,
        ProcedureEvent::VersionInfoUpdated(VersionInfoPhase::Idle)
    );
    assert_eq!(*engine.version_info_phase(), VersionInfoPhase::Idle);

    let ignored = engine
        .apply_inbound(ProcedureMessage::VersionInfoAck(VersionInfoAckMessage {
            correlation_id: None,
            software_version: Some(software_version("pcf-idle")),
        }))
        .unwrap();
    assert_eq!(
        ignored,
        ProcedureEvent::Ignored {
            message_type: cdma_a9::MessageType::VersionInfoAck,
        }
    );
}

#[test]
fn update_a8_tracks_tupd9_against_the_session() {
    let setup = setup_message();
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();

    let pending = engine
        .apply_outbound(ProcedureMessage::UpdateA8(UpdateA8Message {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            imsi: setup.imsi.clone(),
            esn: setup.esn,
            meid: None,
            service_configuration_record: None,
            service_option: Some(setup.service_option),
            user_zone_id: None,
            quality_of_service_parameters: None,
            cause: Some(CauseValue(0x1b)),
            rn_pdit: None,
            sr_id: None,
            a9_indicators: Some(A9Indicators {
                sdb_supported: true,
                ..Default::default()
            }),
            pdsn_ip_address: None,
            anchor_pdsn_address: None,
            anchor_pp_address: None,
        }))
        .unwrap();
    assert_eq!(
        pending,
        ProcedureEvent::SessionUpdateUpdated {
            con_ref: vec![0x10],
            phase: SessionUpdatePhase::RequestPending {
                pending: PendingRequestIdentity {
                    call_connection_reference: setup.call_connection_reference,
                    correlation_id: setup.correlation_id,
                },
                initiated_by_local: true,
            },
        }
    );
    assert_eq!(
        engine.session(&[0x10]).unwrap().session_update_phase(),
        SessionUpdatePhase::RequestPending {
            pending: PendingRequestIdentity {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
            },
            initiated_by_local: true,
        }
    );

    let cleared = engine
        .apply_inbound(ProcedureMessage::UpdateA8Ack(UpdateA8AckMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            cause: Some(CauseValue(0x13)),
        }))
        .unwrap();
    assert_eq!(
        cleared,
        ProcedureEvent::SessionUpdateUpdated {
            con_ref: vec![0x10],
            phase: SessionUpdatePhase::Idle,
        }
    );
    assert_eq!(
        engine.session(&[0x10]).unwrap().session_update_phase(),
        SessionUpdatePhase::Idle
    );
}

#[test]
fn short_data_tracks_tsdd9_and_requires_matching_ack_identity() {
    let mut pcf = ProcedureEngine::new(ProcedureRole::Pcf);
    let delivery = ShortDataDeliveryMessage {
        correlation_id: Some(CorrelationId([5, 6, 7, 8])),
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: Some(Meid([0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc])),
        sr_id: None,
        data_count: Some(DataCount(0x0010)),
        adds_user_part: cdma_a9::AddsUserPart::short_data_burst([0xaa, 0xbb]),
        a9_indicators: Some(A9Indicators {
            ccpd_mode: true,
            sdb_supported: true,
            ..Default::default()
        }),
    };

    let pending = pcf
        .apply_outbound(ProcedureMessage::ShortDataDelivery(delivery.clone()))
        .unwrap();
    assert_eq!(
        pending,
        ProcedureEvent::ShortDataUpdated(ShortDataPhase::DeliveryPending {
            request: cdma_a9::ShortDataRequestState {
                correlation_id: delivery.correlation_id,
                imsi: delivery.imsi.clone(),
                esn: delivery.esn,
                meid: delivery.meid,
            },
            initiated_by_local: true,
        })
    );

    let mismatch = pcf
        .apply_inbound(ProcedureMessage::ShortDataAck(ShortDataAckMessage {
            correlation_id: delivery.correlation_id,
            imsi: delivery.imsi.clone(),
            esn: delivery.esn,
            meid: None,
            cause: CauseValue(0x16),
        }))
        .unwrap_err();
    assert_eq!(
        mismatch,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::ShortDataAck,
            state: "short data acknowledgement MEID does not match the pending request",
        }
    );

    let cleared = pcf
        .apply_inbound(ProcedureMessage::ShortDataAck(ShortDataAckMessage {
            correlation_id: delivery.correlation_id,
            imsi: delivery.imsi.clone(),
            esn: delivery.esn,
            meid: delivery.meid,
            cause: CauseValue(0x17),
        }))
        .unwrap();
    assert_eq!(
        cleared,
        ProcedureEvent::ShortDataUpdated(ShortDataPhase::Idle)
    );
    assert_eq!(*pcf.short_data_phase(), ShortDataPhase::Idle);
}

#[test]
fn access_link_ack_rejects_unexpected_pending_correlation() {
    let mut setup = setup_message();
    setup.correlation_id = None;
    let mut engine = ProcedureEngine::new(ProcedureRole::Bsc);
    engine
        .apply_outbound(ProcedureMessage::SetupA8(setup.clone()))
        .unwrap();
    engine
        .apply_inbound(ProcedureMessage::ConnectA8(connect_response(&setup)))
        .unwrap();
    engine
        .apply_outbound(ProcedureMessage::AlConnected(AlConnectedMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: setup.correlation_id,
            a8_traffic_id: setup.a8_traffic_id.clone(),
            pdsn_ip_address: None,
        }))
        .unwrap();

    let error = engine
        .apply_inbound(ProcedureMessage::AlConnectedAck(AlConnectedAckMessage {
            call_connection_reference: setup.call_connection_reference,
            correlation_id: Some(CorrelationId([9, 10, 11, 12])),
        }))
        .unwrap_err();
    assert_eq!(
        error,
        cdma_a9::Error::InvalidProcedureState {
            message_type: cdma_a9::MessageType::AlConnectedAck,
            state: "access link connect acknowledgement correlation is present without a pending request correlation",
        }
    );
}
