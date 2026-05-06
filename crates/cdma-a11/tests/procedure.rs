use cdma_a11::{
    AuthenticationExtension, AuthenticationExtensionType, CapabilitiesInfo,
    CapabilitiesInfoAcknowledge, ClearReason, Direction, Extension, Message, Nvse,
    PcfEnabledFeatureNvse, PdsnEnabledFeatureNvse, ProcedureEvent, RegistrationAcknowledge,
    RegistrationReply, RegistrationRequest, RegistrationUpdate, SessionKey, SessionProcedureTable,
    SessionSpecificExtension, SessionState, SessionUpdate, SessionUpdateAcknowledge,
};

fn session() -> SessionSpecificExtension {
    SessionSpecificExtension {
        protocol_type: 0x8881,
        pcf_session_id: 0x0102_0304,
        session_id_version: 1,
        mn_session_reference_id: 1,
        mn_id_type: 0x0006,
        mn_id: vec![0x20, 0x43, 0x65, 0x87, 0x09, 0xf1],
    }
}

fn mobile_home_authentication_extension() -> AuthenticationExtension {
    AuthenticationExtension {
        extension_type: AuthenticationExtensionType::MobileHome,
        security_parameter_index: 0x1122_3344,
        authenticator: vec![0xaa; 16],
    }
}

fn update_authentication_extension() -> AuthenticationExtension {
    AuthenticationExtension {
        extension_type: AuthenticationExtensionType::RegistrationUpdate,
        security_parameter_index: 0x0102_0304,
        authenticator: vec![0xbb; 16],
    }
}

fn request(lifetime: u16, identification: u64) -> RegistrationRequest {
    RegistrationRequest {
        flags: 0x0a,
        lifetime,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        care_of_address: [192, 0, 2, 2],
        identification,
        session: session(),
        extensions: vec![Extension::Authentication(
            mobile_home_authentication_extension(),
        )],
    }
}

fn reply(lifetime: u16, identification: u64, code: u8) -> RegistrationReply {
    RegistrationReply {
        code,
        lifetime,
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        identification,
        session: session(),
        extensions: vec![Extension::Authentication(
            mobile_home_authentication_extension(),
        )],
    }
}

fn update(identification: u64) -> RegistrationUpdate {
    RegistrationUpdate {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        identification,
        session: session(),
        nvses: vec![Nvse::PdsnCode(0xcb)],
        authentication_extension: update_authentication_extension(),
    }
}

fn acknowledge_from_update(update: &RegistrationUpdate) -> RegistrationAcknowledge {
    RegistrationAcknowledge {
        reserved: [0, 0],
        status: 0x00,
        home_address: update.home_address,
        care_of_address: [192, 0, 2, 2],
        identification: update.identification,
        session: update.session.clone(),
        authentication_extension: update_authentication_extension(),
    }
}

fn session_update(identification: u64) -> SessionUpdate {
    SessionUpdate {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        identification,
        session: session(),
        nvses: vec![Nvse::SessionParameter(
            cdma_a11::SessionParameterNvse::RnPdit(25),
        )],
        authentication_extension: update_authentication_extension(),
    }
}

fn session_update_ack(update: &SessionUpdate, status: u8) -> SessionUpdateAcknowledge {
    SessionUpdateAcknowledge {
        reserved: [0, 0],
        status,
        home_address: update.home_address,
        care_of_address: [192, 0, 2, 2],
        identification: update.identification,
        session: update.session.clone(),
        authentication_extension: update_authentication_extension(),
    }
}

fn capabilities_info(identification: u64) -> CapabilitiesInfo {
    CapabilitiesInfo {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        home_agent: [192, 0, 2, 1],
        care_of_address: [192, 0, 2, 2],
        identification,
        nvses: vec![Nvse::PcfEnabledFeature(
            PcfEnabledFeatureNvse::GreSegmentationEnabled,
        )],
        authentication_extension: update_authentication_extension(),
    }
}

fn capabilities_info_ack(identification: u64) -> CapabilitiesInfoAcknowledge {
    CapabilitiesInfoAcknowledge {
        reserved: [0, 0, 0],
        home_address: [0, 0, 0, 0],
        care_of_address: [192, 0, 2, 2],
        identification,
        nvses: vec![Nvse::PdsnEnabledFeature(
            PdsnEnabledFeatureNvse::PacketBoundaryEnabled,
        )],
        authentication_extension: update_authentication_extension(),
    }
}

fn establish_session(table: &mut SessionProcedureTable, identification: u64) -> SessionKey {
    let key = SessionKey::from_session(&session());
    table
        .apply(
            10,
            Direction::Outbound,
            &Message::RegistrationRequest(request(30, identification)),
        )
        .unwrap();
    table
        .apply(
            10,
            Direction::Inbound,
            &Message::RegistrationReply(reply(30, identification, 0)),
        )
        .unwrap();
    key
}

#[test]
fn registration_and_refresh_flow() {
    let mut table = SessionProcedureTable::new();
    let key = SessionKey::from_session(&session());

    let event = table
        .apply(
            100,
            Direction::Outbound,
            &Message::RegistrationRequest(request(30, 0x1111)),
        )
        .unwrap();
    assert_eq!(event, ProcedureEvent::PendingRegistration { key });
    assert_eq!(
        table.session(key).unwrap().state,
        SessionState::PendingRegistration
    );

    let event = table
        .apply(
            100,
            Direction::Inbound,
            &Message::RegistrationReply(reply(30, 0x1111, 0)),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::Registered {
            key,
            expires_at: 130
        }
    );

    let event = table
        .apply(
            120,
            Direction::Outbound,
            &Message::RegistrationRequest(request(45, 0x2222)),
        )
        .unwrap();
    assert_eq!(event, ProcedureEvent::PendingRefresh { key });

    let event = table
        .apply(
            120,
            Direction::Inbound,
            &Message::RegistrationReply(reply(45, 0x2222, 0)),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::Refreshed {
            key,
            expires_at: 165
        }
    );
    assert_eq!(table.session(key).unwrap().state, SessionState::Active);
}

#[test]
fn remote_registration_update_and_acknowledge_flow() {
    let mut table = SessionProcedureTable::new();
    let key = establish_session(&mut table, 0x1111);

    let update = update(0x1111);
    let event = table
        .apply(
            20,
            Direction::Inbound,
            &Message::RegistrationUpdate(update.clone()),
        )
        .unwrap();
    assert_eq!(event, ProcedureEvent::PendingTeardown { key });
    assert_eq!(
        table.session(key).unwrap().state,
        SessionState::PendingTeardown
    );

    let event = table
        .apply(
            20,
            Direction::Outbound,
            &Message::RegistrationAcknowledge(acknowledge_from_update(&update)),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::Cleared {
            key,
            reason: ClearReason::RemoteTeardown
        }
    );
    assert!(table.session(key).is_none());
}

#[test]
fn session_update_outbound_and_ack_flow() {
    let mut table = SessionProcedureTable::new();
    let key = establish_session(&mut table, 0x3333);

    let update = session_update(0x4444);
    let event = table
        .apply(
            20,
            Direction::Outbound,
            &Message::SessionUpdate(update.clone()),
        )
        .unwrap();
    assert_eq!(event, ProcedureEvent::PendingSessionUpdate { key });
    assert_eq!(
        table.session(key).unwrap().state,
        SessionState::PendingSessionUpdate
    );

    let event = table
        .apply(
            25,
            Direction::Inbound,
            &Message::SessionUpdateAcknowledge(session_update_ack(&update, 0x00)),
        )
        .unwrap();
    assert_eq!(event, ProcedureEvent::SessionParametersUpdated { key });
    assert_eq!(table.session(key).unwrap().state, SessionState::Active);
}

#[test]
fn session_update_inbound_and_ack_flow() {
    let mut table = SessionProcedureTable::new();
    let key = establish_session(&mut table, 0x5555);

    let update = session_update(0x6666);
    let event = table
        .apply(
            20,
            Direction::Inbound,
            &Message::SessionUpdate(update.clone()),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::PendingSessionUpdateAcknowledge { key }
    );
    assert_eq!(
        table.session(key).unwrap().state,
        SessionState::PendingSessionUpdateAcknowledge
    );

    let event = table
        .apply(
            21,
            Direction::Outbound,
            &Message::SessionUpdateAcknowledge(session_update_ack(&update, 0xc9)),
        )
        .unwrap();
    assert_eq!(event, ProcedureEvent::Rejected { key, code: 0xc9 });
    assert_eq!(table.session(key).unwrap().state, SessionState::Active);
}

#[test]
fn capabilities_info_bidirectional_flow_and_unsolicited_ack() {
    let mut table = SessionProcedureTable::new();

    let outbound = capabilities_info(0x7777);
    let event = table
        .apply(
            50,
            Direction::Outbound,
            &Message::CapabilitiesInfo(outbound),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::PendingCapabilitiesInfo {
            identification: 0x7777
        }
    );

    let event = table
        .apply(
            51,
            Direction::Inbound,
            &Message::CapabilitiesInfoAcknowledge(capabilities_info_ack(0x7777)),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::CapabilitiesInfoCompleted {
            identification: 0x7777
        }
    );

    let inbound = CapabilitiesInfo {
        nvses: vec![Nvse::PdsnEnabledFeature(
            PdsnEnabledFeatureNvse::FlowControlEnabled,
        )],
        ..capabilities_info(0x8888)
    };
    let event = table
        .apply(60, Direction::Inbound, &Message::CapabilitiesInfo(inbound))
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::PendingCapabilitiesInfoAcknowledge {
            identification: 0x8888
        }
    );

    let outbound_ack = CapabilitiesInfoAcknowledge {
        nvses: vec![Nvse::PcfEnabledFeature(
            PcfEnabledFeatureNvse::ShortDataIndicationSupported,
        )],
        ..capabilities_info_ack(0x8888)
    };
    let event = table
        .apply(
            61,
            Direction::Outbound,
            &Message::CapabilitiesInfoAcknowledge(outbound_ack),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::CapabilitiesInfoCompleted {
            identification: 0x8888
        }
    );

    let event = table
        .apply(
            70,
            Direction::Inbound,
            &Message::CapabilitiesInfoAcknowledge(capabilities_info_ack(0x9999)),
        )
        .unwrap();
    assert_eq!(
        event,
        ProcedureEvent::IgnoredCapabilitiesInfoAcknowledge {
            identification: 0x9999
        }
    );
}

#[test]
fn protocol_timer_expiry_covers_session_update_and_capabilities_info() {
    let mut table = SessionProcedureTable::new();
    let key = establish_session(&mut table, 0xaaaa);

    let update = session_update(0xbbbb);
    table
        .apply(100, Direction::Outbound, &Message::SessionUpdate(update))
        .unwrap();
    table
        .apply(
            101,
            Direction::Outbound,
            &Message::CapabilitiesInfo(capabilities_info(0xcccc)),
        )
        .unwrap();

    let events = table.expire_protocol_timers(112, 10, 10);
    assert_eq!(
        events,
        vec![
            ProcedureEvent::SessionUpdateExpired { key },
            ProcedureEvent::CapabilitiesInfoExpired {
                identification: 0xcccc
            }
        ]
    );
    assert_eq!(table.session(key).unwrap().state, SessionState::Active);
}

#[test]
fn expires_active_sessions_and_can_clear_administratively() {
    let mut table = SessionProcedureTable::new();
    let key = establish_session(&mut table, 0xdddd);

    let expired = table.expire_sessions(50);
    assert_eq!(expired, vec![ProcedureEvent::Expired { key }]);
    assert!(table.session(key).is_none());

    let key = establish_session(&mut table, 0xeeee);
    let event = table.clear_session(key).unwrap();
    assert_eq!(
        event,
        ProcedureEvent::Cleared {
            key,
            reason: ClearReason::Administrative
        }
    );
}
