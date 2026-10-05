use num_complex::Complex32;

use crate::access::{
    AccessAttempt, AccessFailure, AccessKind, AccessOutcome, ProbeParams,
    access_probe_correction_db, band_offset_power_dbm, open_loop_probe_power_dbm,
};
use crate::forward_rx::ForwardEvent;
use crate::ms::{AccessParameters, AccessReason, MsEvent, MsProtocolState, MsTimerExpiry};
use crate::traffic::TrafficSession;
use crate::tx::{
    AccessArq, AccessChannelConfig, SERVICE_OPTION_SMS, build_ms_ack_order_capsule,
    build_origination_capsule, build_page_response_capsule, build_registration_capsule,
    build_status_response_capsule, capsule_frames, modulate_access_probe, pulse_shape,
};

use super::{
    ACCESS_LEAD_CHIPS, DEFAULT_TRAFFIC_POWER_STEP_DB, Engine, MIN_PREAMBLE_FRAMES,
    REVERSE_LEAD_CHIPS, REVERSE_START_CHIPS, ReverseBurst, ReverseTransmitter,
    TRAFFIC_POWER_UPDATE_CHIPS, TRAILING_FRAMES,
};

impl ReverseTransmitter {
    fn capsule(
        &mut self,
        reason: &AccessReason,
        serving: &crate::tx::ServingSystem,
    ) -> (cdma_common::bits::Bitstream, u8) {
        let ack_req = !matches!(reason, AccessReason::AckOrder { .. });
        let msg_seq = self.seq.next(ack_req);
        let arq = match reason {
            AccessReason::PageResponse { ack_seq, .. } => AccessArq {
                ack_seq: *ack_seq,
                msg_seq,
                ack_req: true,
                valid_ack: true,
            },
            AccessReason::AckOrder { ack_seq } => AccessArq {
                ack_seq: *ack_seq,
                msg_seq,
                ack_req: false,
                valid_ack: true,
            },
            AccessReason::StatusResponse { ack_seq, .. } => AccessArq {
                ack_seq: *ack_seq,
                msg_seq,
                ack_req: true,
                valid_ack: true,
            },
            _ => AccessArq::assured(msg_seq),
        };
        let bits = match reason {
            AccessReason::Registration { reg_type } => {
                build_registration_capsule(&self.identity, *reg_type, &arq, serving)
            }
            AccessReason::PageResponse { service_option, .. } => {
                build_page_response_capsule(&self.identity, *service_option, &arq, serving)
            }
            AccessReason::Origination {
                service_option,
                digits,
            } => build_origination_capsule(&self.identity, *service_option, digits, &arq, serving),
            AccessReason::StatusResponse {
                qual_info_type,
                qual_info,
                record_types,
                ..
            } => build_status_response_capsule(
                &self.identity,
                *qual_info_type,
                qual_info,
                record_types,
                &arq,
                serving,
            ),
            AccessReason::AckOrder { .. } => {
                build_ms_ack_order_capsule(&self.identity, &arq, serving)
            }
        };
        (bits, msg_seq)
    }
}

fn default_probe_params() -> ProbeParams {
    ProbeParams {
        nom_pwr: 0,
        nom_pwr_ext: 0,
        init_pwr: 0,
        pwr_step: 1,
        num_step: 2,
        pam_sz: MIN_PREAMBLE_FRAMES.saturating_sub(1) as u8,
        max_cap_sz: 7,
        acc_chan: 0,
        probe_pn_ran: 9,
        acc_tmo: 3,
        probe_bkoff: 1,
        bkoff: 1,
        max_req_seq: 2,
        max_rsp_seq: 2,
        psist: 0,
        msg_psist: 0,
        reg_psist: 0,
    }
}

fn probe_params(access: Option<&AccessParameters>) -> ProbeParams {
    access
        .map(ProbeParams::from)
        .unwrap_or_else(default_probe_params)
}

fn access_kind(reason: &AccessReason) -> AccessKind {
    match reason {
        AccessReason::Registration { .. } => AccessKind::Registration,
        AccessReason::Origination { .. } => AccessKind::Origination,
        AccessReason::PageResponse { .. } => AccessKind::Response,
        AccessReason::AckOrder { .. } => AccessKind::Acknowledgment,
        AccessReason::StatusResponse { .. } => AccessKind::Response,
    }
}

impl Engine {
    pub(super) fn push_burst(&mut self, burst: ReverseBurst) {
        let chips = burst.samples.len() as u64 / self.oversample.max(1);
        self.reverse_end_chip = self.reverse_end_chip.max(burst.absolute_chip_start + chips);
        self.pending_tx.push_back(burst);
    }

    pub(super) fn burst_at(
        &self,
        start_chip: u64,
        samples: Vec<Complex32>,
        power_dbm: f32,
        end_of_burst: bool,
        label: &'static str,
    ) -> ReverseBurst {
        let slew = self.forward.pilot_slew();
        let stream_sample_start = self
            .time_base
            .map(|tb| tb.sample_at(start_chip, slew))
            .unwrap_or(0);
        ReverseBurst {
            samples,
            absolute_chip_start: start_chip,
            stream_sample_start,
            power_dbm,
            access_power_offset_db: None,
            traffic_gain_delta_db: 0.0,
            forward_carrier_offset_hz: self.forward.measurements().carrier_offset_hz(),
            end_of_burst,
            label,
        }
    }

    pub(super) fn probe_power_dbm(&self, params: &ProbeParams, pwr_lvl: u8) -> f32 {
        let m = self.forward.measurements().snapshot();
        let band_offset = self
            .current_channel()
            .map(|(_, ch)| band_offset_power_dbm(ch.band_class))
            .unwrap_or(-73.0);
        let rx_input_dbm = m.rx_power_dbfs + self.rx_reference_dbm;
        open_loop_probe_power_dbm(rx_input_dbm, m.ec_io_db, band_offset, params, pwr_lvl)
    }

    pub(super) fn finish_access(&mut self, outcome: AccessOutcome) {
        if let Some((attempt, _)) = &mut self.access {
            match outcome {
                AccessOutcome::Acknowledged => attempt.on_ack(),
                AccessOutcome::Failed(_) => attempt.cancel(),
            }
        }
        self.access = None;
        self.reverse.current = None;
    }

    pub(super) fn drive_reverse(&mut self) {
        if self.time_base.is_none() {
            return;
        }
        if let Some(expiry) = self.core.maintain_idle_timers(self.system_time_chips()) {
            match expiry {
                MsTimerExpiry::CommonChannel => {
                    log::warn!("cdma-ms: no valid paging message within T30m, reacquiring")
                }
                MsTimerExpiry::AccessOverhead => {
                    log::warn!("cdma-ms: access overhead was not current within T41m, reacquiring")
                }
            }
            self.apply_forward(ForwardEvent::PilotLost);
            self.forward.reset();
            return;
        }
        if self.core.state().on_traffic() {
            return;
        }
        let received_through_chip = self
            .forward
            .paging_decoded_through_network_chip()
            .unwrap_or(0);
        let transmit_now_chip = self.reverse_now_chips();
        self.core.expire_access_response_wait(received_through_chip);

        // Access timing and the long-code mask require the overhead messages.
        let overhead_current = self.core.overhead_ready_for_access();
        if self.access.is_none() && overhead_current {
            let reason = self.core.pending_access().cloned().or_else(|| {
                self.core
                    .take_pending_ack()
                    .map(|ack_seq| AccessReason::AckOrder { ack_seq })
            });
            if let Some(reason) = reason {
                let params = probe_params(self.core.access_params());
                let mob_p_rev = self.reverse.identity.mob_p_rev;
                let serving = crate::tx::ServingSystem {
                    p_rev_in_use: crate::tx::p_rev_in_use(
                        self.core.base_station_p_rev().unwrap_or(mob_p_rev),
                        mob_p_rev,
                    ),
                    active_pilot_strength: crate::tx::pilot_strength(
                        self.forward.measurements().snapshot().ec_io_db,
                    ),
                    extended_scm: self.current_channel().is_some_and(|(_, ch)| {
                        matches!(
                            ch.band_class,
                            cdma_common::band_class::BandClass::Bc1
                                | cdma_common::band_class::BandClass::Bc4
                        )
                    }),
                    base_mcc: self
                        .core
                        .overhead()
                        .extended_system_parameters
                        .as_ref()
                        .map(|espm| espm.mcc),
                    base_imsi_11_12: self
                        .core
                        .overhead()
                        .extended_system_parameters
                        .as_ref()
                        .map(|espm| espm.imsi_11_12),
                };
                let p_rev_in_use = serving.p_rev_in_use;
                let (bits, msg_seq) = self.reverse.capsule(&reason, &serving);
                self.reverse.current = Some((reason.clone(), msg_seq));
                let kind = access_kind(&reason);
                let frames = capsule_frames(bits.len());
                let attempt = AccessAttempt::new(kind, params, frames, self.esn, transmit_now_chip);
                log::info!(
                    "cdma-ms: access attempt {} msg_seq={} ({} capsule frame{}, p_rev_in_use {}, pilot_slew {} samples)",
                    reason,
                    msg_seq,
                    frames,
                    if frames == 1 { "" } else { "s" },
                    p_rev_in_use,
                    self.forward.pilot_slew()
                );
                self.access = Some((attempt, reason));
                self.access_capsule = Some(bits);
            }
        }

        let Some((mut attempt, reason)) = self.access.take() else {
            return;
        };
        // Build before queuing a probe so modulation does not consume its transmit lead.
        if self.staged_probe.is_none() {
            if let Some(next) = attempt.staged_probe(transmit_now_chip, ACCESS_LEAD_CHIPS) {
                if let Some(samples) =
                    self.build_access_probe(next.code_start_chip, next.preamble_frames, next.acn)
                {
                    self.staged_probe = Some((next.code_start_chip, next.pwr_lvl, samples));
                }
            }
        }
        let params = probe_params(self.core.access_params());
        while let Some(plan) = attempt.poll(transmit_now_chip, ACCESS_LEAD_CHIPS) {
            let power = self.probe_power_dbm(&params, plan.pwr_lvl);
            let Some(bits) = self.access_capsule.clone() else {
                break;
            };
            let start_chip = plan.start_chip.max(self.reverse_end_chip);
            let code_start_chip = plan.code_start_chip + start_chip - plan.start_chip;
            let lc_state = self.lc_state_at(code_start_chip);
            log::info!(
                "cdma-ms: access probe seq={} pwr_lvl={} start_chip={} code_start_chip={} slot_frame={} lc_state=0x{:011x} acn={} pcn={} base_id={} pilot_pn={} preamble_frames={} capsule_bits={}",
                plan.sequence,
                plan.pwr_lvl,
                start_chip,
                code_start_chip,
                code_start_chip / crate::access::CHIPS_PER_FRAME,
                lc_state,
                plan.acn,
                self.reverse.access_cfg.paging_channel,
                self.core
                    .system_params()
                    .map_or(self.reverse.access_cfg.base_id, |sp| sp.base_id),
                self.core
                    .sync_info()
                    .map_or(self.reverse.access_cfg.pilot_pn, |sync| sync.pilot_pn),
                plan.preamble_frames,
                bits.bits().len()
            );
            let staged = self
                .staged_probe
                .take()
                .filter(|(chip, pwr_lvl, _)| *chip == code_start_chip && *pwr_lvl == plan.pwr_lvl)
                .map(|(_, _, samples)| samples);
            let Some(samples) = staged.or_else(|| {
                self.build_access_probe(code_start_chip, plan.preamble_frames, plan.acn)
            }) else {
                break;
            };
            let mut burst = self.burst_at(start_chip, samples, power, true, "access");
            burst.access_power_offset_db = Some(access_probe_correction_db(&params, plan.pwr_lvl));
            self.push_burst(burst);
            let msg_seq = self
                .reverse
                .current
                .as_ref()
                .map(|(_, msg_seq)| *msg_seq)
                .unwrap_or_default();
            let ack_seq = match &reason {
                AccessReason::PageResponse { ack_seq, .. }
                | AccessReason::AckOrder { ack_seq }
                | AccessReason::StatusResponse { ack_seq, .. } => Some(*ack_seq),
                _ => None,
            };
            self.core.emit(MsEvent::AccessProbe {
                reason: reason.to_string(),
                msg_seq,
                ack_req: !matches!(reason, AccessReason::AckOrder { .. }),
                ack_seq,
                sequence: plan.sequence,
                probe: plan.pwr_lvl as u32,
                power_dbm: power,
                start_chip,
            });
        }
        match attempt.outcome() {
            Some(AccessOutcome::Failed(cause)) => {
                self.access = None;
                self.reverse.current = None;
                self.access_capsule = None;
                let waiting = cause == AccessFailure::MaxProbes
                    && self.core.on_access_probes_exhausted(received_through_chip);
                if !waiting {
                    self.core
                        .on_access_failed(cause.label(), received_through_chip);
                }
            }
            Some(AccessOutcome::Acknowledged) => {
                self.access = None;
                self.reverse.current = None;
                self.access_capsule = None;
            }
            None => self.access = Some((attempt, reason)),
        }
    }

    pub(super) fn build_access_probe(
        &self,
        code_start_chip: u64,
        preamble_frames: usize,
        acn: u8,
    ) -> Option<Vec<Complex32>> {
        let bits = self.access_capsule.as_ref()?;
        let base = &self.reverse.access_cfg;
        let cfg = AccessChannelConfig {
            access_channel_number: acn,
            base_id: self
                .core
                .system_params()
                .map_or(base.base_id, |sp| sp.base_id),
            pilot_pn: self
                .core
                .sync_info()
                .map_or(base.pilot_pn, |sync| sync.pilot_pn),
            ..base.clone()
        };
        let lc_state = self.lc_state_at(code_start_chip);
        Some(pulse_shape(&modulate_access_probe(
            &cfg,
            bits.bits(),
            code_start_chip,
            lc_state,
            preamble_frames,
            TRAILING_FRAMES,
        )))
    }

    /// Advance decoded LC_STATE from its validity chip. Before sync, use the initial state at chip zero.
    pub(super) fn lc_state_at(&self, chip: u64) -> u64 {
        match self.lc_anchor {
            Some((anchor_chip, state)) => crate::tx::lc_state_at(state, anchor_chip, chip),
            None => crate::tx::default_lc_state_at(chip),
        }
    }

    pub(super) fn drive_traffic(&mut self) {
        if self.time_base.is_none() {
            return;
        }
        if self.traffic.is_some() && self.forward.traffic_forward_lost() {
            log::warn!("cdma-ms: forward traffic fade timer T5m expired, reacquiring");
            self.apply_forward(ForwardEvent::PilotLost);
            self.forward.reset();
            return;
        }
        let now = self.reverse_now_chips();
        if self.traffic.is_none() {
            if let MsProtocolState::TrafficChannelInit { assignment } = self.core.state().clone() {
                let so = match self.core.pending_service_option() {
                    Some(so) => so,
                    None => SERVICE_OPTION_SMS,
                };
                // Start after any burst still on the air (the last access
                // probe), so the reverse traffic does not overlap it.
                let start_floor = self.reverse_end_chip.saturating_sub(REVERSE_START_CHIPS);
                let power_report = self
                    .core
                    .overhead()
                    .system_parameters
                    .as_ref()
                    .and_then(crate::traffic::PowerReportParameters::from_system_parameters);
                let mut session = TrafficSession::new(
                    self.esn,
                    assignment,
                    so,
                    self.pending_sms.take(),
                    self.core.incoming_call(),
                    power_report,
                    now.max(start_floor),
                    REVERSE_START_CHIPS,
                );
                log::info!("cdma-ms: traffic channel up, starting reverse RC3 transmit");
                if self.decline_incoming {
                    log::info!("cdma-ms: declining incoming call with a Release Order");
                    session.release();
                    self.decline_incoming = false;
                }
                self.traffic = Some(session);
                self.traffic_power_delta_db = 0.0;
                self.traffic_power_step_db = DEFAULT_TRAFFIC_POWER_STEP_DB;
                self.traffic_power_frames = 0;
                self.traffic_tx_start_chip = None;
                self.traffic_power_window = [0; 2];
                self.traffic_power_report = [0; 2];
                self.traffic_power_next_update_chip = 0;
                self.traffic_shaper = Some(cdma_bts::sdr::fir::ComplexFir32::new(
                    &cdma_bts::sdr::cdma2000_baseband_filter_taps_f64(),
                ));
            }
        }
        let Some(mut session) = self.traffic.take() else {
            return;
        };
        let power = self.traffic_power_dbm();
        let lc_anchor = self.lc_anchor;
        let lc = move |chip: u64| match lc_anchor {
            Some((anchor_chip, state)) => crate::tx::lc_state_at(state, anchor_chip, chip),
            None => crate::tx::default_lc_state_at(chip),
        };
        session.set_forward_confirmed(self.forward.traffic_forward_confirmed());
        let (measured_frames, measured_bad_frames) = self.forward.take_traffic_power_measurements();
        let active_pilot_strength =
            crate::tx::pilot_strength(self.forward.measurements().snapshot().ec_io_db);
        session.record_forward_measurements(
            measured_frames,
            measured_bad_frames,
            active_pilot_strength,
        );
        let chunks = session.poll(now, REVERSE_LEAD_CHIPS, &lc);
        for notice in session.drain_tx_notices() {
            self.core.emit(MsEvent::TrafficTransmit {
                name: notice.name.to_string(),
                msg_seq: notice.msg_seq,
                ack_seq: notice.ack_seq,
                ack_req: notice.ack_req,
                retransmission: notice.retransmission,
            });
        }
        for mut chunk in chunks {
            if self.traffic_tx_start_chip.is_none() {
                self.traffic_tx_start_chip = Some(chunk.chip);
                self.traffic_power_next_update_chip = chunk.chip + TRAFFIC_POWER_UPDATE_CHIPS;
            }
            // C.S0002-E §2.1.3.1.18.1 applies chip impulses to the baseband filter.
            for chip in chunk.samples.chunks_mut(self.oversample as usize) {
                chip[1..].fill(Complex32::new(0.0, 0.0));
            }
            let samples = match self.traffic_shaper.as_mut() {
                Some(fir) => fir.process_block(&chunk.samples),
                None => pulse_shape(&chunk.samples),
            };
            let mut burst =
                self.burst_at(chunk.chip, samples, power, chunk.end_of_burst, "traffic");
            burst.traffic_gain_delta_db = std::mem::take(&mut self.traffic_power_delta_db);
            self.push_burst(burst);
        }
        if session.up()
            && matches!(
                self.core.state(),
                MsProtocolState::TrafficChannelInit { .. }
            )
        {
            self.core.on_traffic_channel_up();
        }
        if session.ended() {
            self.traffic_shaper = None;
            self.forward.deactivate_traffic();
            let released_by = session.released_by().unwrap_or("mobile");
            if released_by == "release timeout" {
                log::warn!("cdma-ms: release was not acknowledged, reacquiring the system");
                self.reacquire_forward_link(released_by);
                self.forward.reset();
            } else {
                self.core.on_traffic_released(released_by);
                self.core.begin_idle_timers(now);
            }
        } else {
            self.traffic = Some(session);
        }
    }

    pub(super) fn mute_reverse(&mut self) {
        self.traffic_power_delta_db = 0.0;
        self.traffic_tx_start_chip = None;
        self.traffic_power_window = [0; 2];
        self.traffic_power_report = [0; 2];
        self.traffic_power_next_update_chip = 0;
        self.pending_tx.clear();
        self.pending_tx.push_back(self.burst_at(
            self.system_time_chips(),
            Vec::new(),
            0.0,
            true,
            "traffic",
        ));
    }

    pub(super) fn traffic_power_dbm(&self) -> f32 {
        let m = self.forward.measurements().snapshot();
        let band_offset = self
            .current_channel()
            .map(|(_, ch)| band_offset_power_dbm(ch.band_class))
            .unwrap_or(-73.0);
        let params = probe_params(self.core.access_params());
        let rx_input_dbm = m.rx_power_dbfs + self.rx_reference_dbm;
        open_loop_probe_power_dbm(rx_input_dbm, m.ec_io_db, band_offset, &params, 0)
    }
}
