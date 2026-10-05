use super::*;

#[test]
fn manually_tuned_bursts_include_measured_carrier_offset() {
    let engine = Engine::new(EngineConfig::default());
    engine.forward.measurements().set_carrier_offset_hz(375.0);
    let burst = engine.burst_at(0, Vec::new(), 0.0, false, "traffic");
    assert_eq!(burst.forward_carrier_offset_hz, 375.0);
}
use crate::ms::ChannelAssignment;

#[test]
fn receive_gap_invalidates_access_timing_and_queued_transmissions() {
    let mut engine = Engine::new(EngineConfig::default());
    engine.power_on();
    engine.core.on_sync_decoded(SyncParameters::default());
    engine.originate(SERVICE_OPTION_SMS, "5551234".into());
    engine.time_base = Some(TimeBase {
        sync_chip: CHIPS_PER_FRAME,
        at_sample: 0,
        oversample: engine.oversample,
    });
    engine.lc_anchor = Some((CHIPS_PER_FRAME, 1));
    engine.pending_tx.push_back(engine.burst_at(
        CHIPS_PER_FRAME,
        vec![Complex32::new(1.0, 0.0)],
        0.0,
        true,
        "access",
    ));
    engine.on_forward_discontinuity();
    assert_eq!(*engine.state(), MsProtocolState::PilotAcquisition);
    assert!(engine.time_base.is_none());
    assert!(engine.lc_anchor.is_none());
    assert!(engine.core.pending_access().is_none());
    let stop = engine.poll_transmit().expect("mute after losing timing");
    assert!(stop.end_of_burst && stop.samples.is_empty());
    assert!(engine.poll_transmit().is_none());
    engine.apply_forward(ForwardEvent::PilotLock);
    engine.apply_forward(ForwardEvent::Sync(SyncParameters::default()));
    assert_eq!(*engine.state(), MsProtocolState::Idle);
    assert!(!engine.core.overhead_ready_for_access());
}

#[test]
fn power_off_mutes_after_the_traffic_session_is_gone() {
    let mut engine = Engine::new(EngineConfig::default());
    engine.pending_tx.push_back(engine.burst_at(
        CHIPS_PER_FRAME,
        vec![Complex32::new(1.0, 0.0)],
        0.0,
        false,
        "traffic",
    ));
    for _ in 0..2 {
        engine.power_off();
        let stop = engine
            .poll_transmit()
            .expect("power off must mute hardware");
        assert!(stop.end_of_burst && stop.samples.is_empty());
        assert!(engine.poll_transmit().is_none());
    }
}

#[test]
fn sync_acquisition_times_out_and_reacquires_when_camped() {
    let mut engine = Engine::new(EngineConfig::default());
    engine.power_on();
    engine.apply_forward(ForwardEvent::PilotLock);
    assert_eq!(*engine.state(), MsProtocolState::SyncAcquisition);

    engine.samples_fed += engine.samples_for_ms(SYNC_ACQUISITION_RECOVERY_MS) as u64 + 1;
    engine.check_decode_progress();

    assert_eq!(*engine.state(), MsProtocolState::PilotAcquisition);
    assert!(engine.pilot_locked_at.is_none());
}

#[test]
fn declining_an_incoming_call_before_the_channel_flags_a_release() {
    let mut engine = Engine::new(EngineConfig::default());
    engine.power_on();
    engine.apply_forward(ForwardEvent::PilotLock);
    engine.apply_forward(ForwardEvent::Sync(SyncParameters::default()));
    assert_eq!(*engine.state(), MsProtocolState::Idle);

    engine
        .core
        .on_page_received(engine.esn, cdma_voice::SERVICE_OPTION_EVRC_A, 0);
    assert!(engine.core.incoming_call());
    assert!(engine.traffic.is_none());

    engine.hang_up();
    assert!(
        engine.decline_incoming,
        "rejecting before the traffic channel should flag a Release Order"
    );
}

#[test]
fn rc3_commands_adjust_power_only_during_traffic() {
    let mut engine = Engine::new(EngineConfig::default());
    engine.apply_forward(ForwardEvent::TrafficPowerControl { up: 16, down: 0 });
    assert_eq!(engine.traffic_power_delta_db, 0.0);

    engine.traffic = Some(TrafficSession::new(
        0x1234_5678,
        ChannelAssignment {
            walsh_code: 10,
            frame_offset: 0,
            for_rc: 3,
            rev_rc: 3,
            pilot_pn: 0,
        },
        3,
        None,
        false,
        None,
        0,
        CHIPS_PER_FRAME,
    ));
    engine.apply_forward(ForwardEvent::TrafficPowerControl { up: 16, down: 0 });
    assert_eq!(engine.traffic_power_delta_db, 0.0);
    engine.traffic_tx_start_chip = Some(engine.system_time_chips() + CHIPS_PER_FRAME);
    engine.apply_forward(ForwardEvent::TrafficPowerControl { up: 16, down: 0 });
    assert_eq!(engine.traffic_power_delta_db, 0.0);
    engine.traffic_tx_start_chip = Some(0);
    engine.apply_forward(ForwardEvent::TrafficPowerControl { up: 10, down: 6 });
    assert_eq!(engine.traffic_power_delta_db, 0.25);
    engine.apply_forward(ForwardEvent::TrafficPowerControl { up: 0, down: 16 });
    assert_eq!(engine.traffic_power_delta_db, 0.25);
    engine.time_base = Some(TimeBase {
        sync_chip: 0,
        at_sample: 0,
        oversample: engine.oversample,
    });
    for _ in 0..40 {
        engine.samples_fed += TRAFFIC_POWER_UPDATE_CHIPS * engine.oversample;
        engine.apply_forward(ForwardEvent::TrafficPowerControl { up: 0, down: 16 });
    }
    assert_eq!(
        engine.traffic_power_delta_db,
        -DEFAULT_TRAFFIC_POWER_STEP_DB
    );
    engine.apply_forward(ForwardEvent::PilotLost);
    assert!(engine.traffic.is_none());
    let stop = engine.poll_transmit().expect("mute the active transmitter");
    assert!(stop.samples.is_empty());
    assert!(stop.end_of_burst);
}
