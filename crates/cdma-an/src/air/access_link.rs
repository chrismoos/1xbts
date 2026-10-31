//! AN-stream access authentication glue: negotiates a Default Packet
//! Application bound to the access network during session configuration,
//! sends and receives [`AccessAuthLink`] PPP octets on that stream, and reports
//! CHAP credentials and the final result in the traffic outcome.

use super::*;
use crate::access_auth::{AaaVerdict, AccessAuthLink, AccessAuthOutput};

/// C.S0024 Stream Protocol application subtypes.
const DEFAULT_PACKET_ACCESS_NETWORK_SUBTYPE: u16 = 0x0001;
const STREAM_APPLICATION_UNUSED: u16 = 0xffff;
const STREAM_CONFIGURATION_ATTRIBUTE_ID: u8 = 0x00;
const STREAM_CONFIGURATION_RECORD_OCTETS: usize = 9;
/// Length octet of a StreamConfiguration attribute holding one record.
const STREAM_CONFIGURATION_ATTRIBUTE_LENGTH: u8 = 10;
/// Length octet of a ConfigurationResponse record that accepts one value.
const ACCEPTED_VALUE_RECORD_LENGTH: u8 = 2;
const AN_STREAM_CONFIGURATION_VALUE_ID: u8 = 0x00;
const ACCESS_STREAM_PROPOSAL_TIMEOUT: Duration = Duration::from_secs(3);
const ACCESS_STREAM_MAX_RLP_OCTETS: usize = 64;
const STREAM_CONFIGURATION_REQUEST_LABEL: &str = "StreamConfigurationRequest";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum AccessStreamNegotiation {
    #[default]
    Undecided,
    Proposed {
        transaction_id: u8,
        stream_id: u8,
        sent_at: Instant,
    },
    Accepted {
        stream_id: u8,
    },
    Unavailable,
}

#[derive(Debug, Default)]
pub(super) struct AtAccessLink {
    negotiation: AccessStreamNegotiation,
    at_stream_record: Option<[u8; STREAM_CONFIGURATION_RECORD_OCTETS]>,
    proposal_transaction_id: u8,
    resume_config_complete_transaction: Option<u8>,
    pub(super) reverse_rlp: DefaultPacketRlpReceiver,
    link: AccessAuthLink,
    forward_sequence: u32,
    data_ready_transaction: u8,
    start_requested: bool,
    nai: Option<String>,
}

impl AtAccessLink {
    pub(super) fn stream_id(&self) -> Option<u8> {
        match self.negotiation {
            AccessStreamNegotiation::Accepted { stream_id } => Some(stream_id),
            _ => None,
        }
    }

    pub(super) fn state_name(&self) -> &'static str {
        self.link.state_name()
    }

    pub(super) fn result(&self) -> Option<&crate::access_auth::AccessAuthResult> {
        self.link.result()
    }

    pub(super) fn nai(&self) -> Option<&str> {
        self.nai.as_deref()
    }

    pub(super) fn protocol_type(&self) -> Option<u8> {
        self.stream_id()
            .and_then(default_packet_stream_protocol_type)
    }

    /// Returns true when this call schedules the procedure.
    pub(super) fn request_start(&mut self) -> bool {
        if self.stream_id().is_some() && !self.link.is_started() && !self.start_requested {
            self.start_requested = true;
            return true;
        }
        false
    }
}

fn stream_applications(record: &[u8; STREAM_CONFIGURATION_RECORD_OCTETS]) -> [u16; 4] {
    std::array::from_fn(|stream| subtype_to_u16(&record[1 + stream * 2..3 + stream * 2]))
}

impl HrpdAirController {
    pub(super) fn record_at_stream_configuration(
        at: &mut AtSession,
        request_attributes: &[u8],
        value_id: u8,
    ) {
        at.access.at_stream_record = stream_configuration_record(request_attributes, value_id)
            .and_then(|record| record.try_into().ok());
    }

    /// Sends the AN-initiated Stream ConfigurationRequest that adds the access
    /// stream, holding the AT's ConfigurationComplete until it is answered.
    pub(super) fn maybe_propose_access_stream(
        at: &mut AtSession,
        uati: u32,
        at_config_complete_transaction: u8,
        drc_index: u8,
        ack_sequence_number: Option<u8>,
    ) -> Option<HrpdForwardTrafficPacket> {
        if at.access.negotiation != AccessStreamNegotiation::Undecided {
            return None;
        }
        let Some(record) = at.access.at_stream_record else {
            log::info!(
                "HRPD AN: access stream unavailable UATI=0x{uati:08x}; AT sent no StreamConfiguration"
            );
            at.access.negotiation = AccessStreamNegotiation::Unavailable;
            return None;
        };
        let applications = stream_applications(&record);
        if let Some(stream_id) = (1..=3u8).find(|&stream| {
            applications[usize::from(stream)] == DEFAULT_PACKET_ACCESS_NETWORK_SUBTYPE
        }) {
            log::info!(
                "HRPD AN: AT configured the access stream itself UATI=0x{uati:08x} stream={stream_id}"
            );
            at.access.negotiation = AccessStreamNegotiation::Accepted { stream_id };
            return None;
        }
        let Some(stream_id) = (1..=3u8)
            .find(|&stream| applications[usize::from(stream)] == STREAM_APPLICATION_UNUSED)
        else {
            log::info!(
                "HRPD AN: access stream unavailable UATI=0x{uati:08x}; AT uses every stream"
            );
            at.access.negotiation = AccessStreamNegotiation::Unavailable;
            return None;
        };
        let mut attributes = vec![
            STREAM_CONFIGURATION_ATTRIBUTE_LENGTH,
            STREAM_CONFIGURATION_ATTRIBUTE_ID,
            AN_STREAM_CONFIGURATION_VALUE_ID,
        ];
        for (stream, application) in applications.into_iter().enumerate() {
            let application = if stream == usize::from(stream_id) {
                DEFAULT_PACKET_ACCESS_NETWORK_SUBTYPE
            } else {
                application
            };
            attributes.extend_from_slice(&application.to_be_bytes());
        }
        at.access.proposal_transaction_id = at.access.proposal_transaction_id.wrapping_add(1);
        let transaction_id = at.access.proposal_transaction_id;
        let mut payload = vec![SESSION_CONFIGURATION_REQUEST, transaction_id];
        payload.extend_from_slice(&attributes);
        let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
        let pending = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati && pending.active)?;
        let packet = Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            SESSION_PROTOCOL_STREAM,
            &payload,
            Some(sequence_number),
            true,
            ack_sequence_number,
            STREAM_CONFIGURATION_REQUEST_LABEL,
        )?;
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            SESSION_PROTOCOL_STREAM,
            &payload,
            true,
            ack_sequence_number,
            STREAM_CONFIGURATION_REQUEST_LABEL,
        );
        at.slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
        at.access.negotiation = AccessStreamNegotiation::Proposed {
            transaction_id,
            stream_id,
            sent_at: Instant::now(),
        };
        at.access.resume_config_complete_transaction = Some(at_config_complete_transaction);
        log::info!(
            "HRPD AN: proposing access stream UATI=0x{uati:08x} stream={stream_id} transaction=0x{transaction_id:02x} attrs={}",
            bytes_to_hex(&attributes)
        );
        Some(packet)
    }

    /// Returns true when a forward packet acknowledging `ack_sequence_number`
    /// was queued.
    pub(super) fn handle_access_stream_configuration_response(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        payload: &[u8],
        ack_sequence_number: Option<u8>,
        outcome: &mut HrpdTrafficOutcome,
    ) -> bool {
        let AccessStreamNegotiation::Proposed {
            transaction_id,
            stream_id,
            ..
        } = at.access.negotiation
        else {
            return false;
        };
        if payload.get(1) != Some(&transaction_id) {
            log::info!(
                "HRPD AN: ignoring Stream ConfigurationResponse UATI=0x{uati:08x} transaction={:?} expected=0x{transaction_id:02x}",
                payload.get(1)
            );
            return false;
        }
        let attributes = payload.get(2..).unwrap_or_default();
        let accepted = attributes
            == [
                ACCEPTED_VALUE_RECORD_LENGTH,
                STREAM_CONFIGURATION_ATTRIBUTE_ID,
                AN_STREAM_CONFIGURATION_VALUE_ID,
            ];
        at.access.negotiation = if accepted {
            AccessStreamNegotiation::Accepted { stream_id }
        } else {
            AccessStreamNegotiation::Unavailable
        };
        log::info!(
            "HRPD AN: AT {} the access stream UATI=0x{uati:08x} stream={stream_id} response_attrs={}",
            if accepted { "accepted" } else { "declined" },
            bytes_to_hex(attributes)
        );
        self.resume_session_configuration_complete(at, uati, ack_sequence_number, outcome)
    }

    fn resume_session_configuration_complete(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        ack_sequence_number: Option<u8>,
        outcome: &mut HrpdTrafficOutcome,
    ) -> bool {
        let Some(transaction_id) = at.access.resume_config_complete_transaction.take() else {
            return false;
        };
        let Some((packet, _, queue_soft_commit_close)) = self.handle_session_configuration_message(
            at,
            uati,
            &[SESSION_CONFIGURATION_COMPLETE, transaction_id],
            ack_sequence_number,
        ) else {
            return false;
        };
        outcome.forward_traffic.push(packet);
        if queue_soft_commit_close {
            self.queue_session_config_commit_connection_close(at, uati, outcome);
        } else {
            Self::mark_soft_configuration_no_commit_sent(at, uati);
        }
        ack_sequence_number.is_some()
    }

    pub(super) fn maybe_expire_access_stream_proposal(
        &mut self,
        at: &mut AtSession,
        now: Instant,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let AccessStreamNegotiation::Proposed {
            sent_at, stream_id, ..
        } = at.access.negotiation
        else {
            return;
        };
        if now.saturating_duration_since(sent_at) < ACCESS_STREAM_PROPOSAL_TIMEOUT {
            return;
        }
        let Some(uati) = Self::active_traffic_uati(at) else {
            at.access.negotiation = AccessStreamNegotiation::Unavailable;
            at.access.resume_config_complete_transaction = None;
            return;
        };
        log::info!(
            "HRPD AN: no answer to the access stream proposal UATI=0x{uati:08x} stream={stream_id}; continuing without it"
        );
        at.access.negotiation = AccessStreamNegotiation::Unavailable;
        self.resume_session_configuration_complete(at, uati, None, outcome);
    }

    fn active_traffic_uati(at: &AtSession) -> Option<u32> {
        at.pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.active)
            .map(|pending| pending.traffic_uati)
    }

    pub(super) fn access_stream_proposal_pending(at: &AtSession) -> bool {
        matches!(
            at.access.negotiation,
            AccessStreamNegotiation::Proposed { .. }
        )
    }

    pub(super) fn is_access_stream_protocol(at: &AtSession, protocol_type: Option<u8>) -> bool {
        protocol_type.is_some() && at.access.protocol_type() == protocol_type
    }

    pub(super) fn handle_access_stream_octets(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        sequence: u32,
        payload: &[u8],
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let now = Instant::now();
        let rlp = at.access.reverse_rlp.ingest(sequence, payload, now);
        if !rlp.nak_requests.is_empty()
            && let Some(protocol_type) = at.access.protocol_type()
        {
            let nak_payload = default_packet_rlp_nak_payload(&rlp.nak_requests);
            if let Some(packet) = self.build_default_packet_flow_control_packet_with_protocol(
                at,
                uati,
                protocol_type,
                &nak_payload,
                "AccessRlpNak",
            ) {
                outcome.forward_traffic.push(packet);
            }
        }
        if rlp.delivered.is_empty() {
            return;
        }
        let outputs = at.access.link.on_stream_octets(&rlp.delivered, now);
        self.apply_access_auth_outputs(at, uati, outputs, outcome);
    }

    pub(super) fn handle_access_stream_flow_message(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        message: &HrpdAccessMessage,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let Some(protocol_type) = at.access.protocol_type() else {
            return;
        };
        let reply: Option<(&[u8], &str)> = match message {
            HrpdAccessMessage::DefaultPacketXonRequest => {
                Some((&[DEFAULT_PACKET_XON_RESPONSE], "AccessXonResponse"))
            }
            HrpdAccessMessage::DefaultPacketXoffRequest => {
                Some((&[DEFAULT_PACKET_XOFF_RESPONSE], "AccessXoffResponse"))
            }
            HrpdAccessMessage::DefaultPacketRlpReset(_) => {
                at.access.reverse_rlp.reset();
                Some((&[DEFAULT_PACKET_RLP_RESET_ACK], "AccessRlpResetAck"))
            }
            _ => None,
        };
        log::info!(
            "HRPD AN: access stream {message:?} UATI=0x{uati:08x} protocol=0x{protocol_type:02x}"
        );
        if let Some((payload, label)) = reply
            && let Some(packet) = self.build_default_packet_flow_control_packet_with_protocol(
                at,
                uati,
                protocol_type,
                payload,
                label,
            )
        {
            outcome.forward_traffic.push(packet);
        }
    }

    /// Starts a requested access authentication and runs its timers.
    pub(super) fn drive_access_auth(
        &mut self,
        at: &mut AtSession,
        now: Instant,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let ready_uati = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.active && pending.stream0_slp_reset_acked)
            .map(|pending| pending.traffic_uati);
        let Some(uati) = ready_uati else {
            if Self::active_traffic_uati(at).is_some() {
                return;
            }
            if at.access.link.is_started() && at.access.link.result().is_none() {
                let outputs = at.access.link.abandon();
                if let Some(session_uati) = at.session.session().map(|s| s.uati.as_u32()) {
                    self.apply_access_auth_outputs(at, session_uati, outputs, outcome);
                }
            }
            return;
        };
        if at.access.start_requested && !at.access.link.is_started() {
            at.access.start_requested = false;
            let Some(protocol_type) = at.access.protocol_type() else {
                return;
            };
            let session_uati = at
                .session
                .session()
                .map(|s| s.uati.as_u32())
                .unwrap_or(uati);
            log::info!(
                "HRPD AN: starting access authentication UATI=0x{uati:08x} session_uati=0x{session_uati:08x} stream={:?}",
                at.access.stream_id()
            );
            outcome.access_auth_started.push(HrpdAccessAuthStarted {
                session_uati,
                traffic_uati: uati,
            });
            at.access.data_ready_transaction = at.access.data_ready_transaction.wrapping_add(1);
            let data_ready = [DEFAULT_PACKET_DATA_READY, at.access.data_ready_transaction];
            if let Some(packet) = self.build_default_packet_flow_control_packet_with_protocol(
                at,
                uati,
                protocol_type,
                &data_ready,
                "AccessDataReady",
            ) {
                outcome.forward_traffic.push(packet);
            }
            let outputs = at.access.link.start(now);
            self.apply_access_auth_outputs(at, uati, outputs, outcome);
        } else if at.access.link.is_started() && at.access.link.result().is_none() {
            let outputs = at.access.link.on_timer(now);
            self.apply_access_auth_outputs(at, uati, outputs, outcome);
        }
    }

    pub fn handle_access_auth_verdict(
        &mut self,
        session_uati: u32,
        verdict: AaaVerdict,
    ) -> HrpdTrafficOutcome {
        let mut outcome = HrpdTrafficOutcome::default();
        let Some(mut at) = self.ats.remove(&session_uati) else {
            log::info!(
                "HRPD AN: A12 verdict {verdict:?} for unknown session_uati=0x{session_uati:08x}"
            );
            return outcome;
        };
        let now = Instant::now();
        let outputs = at.access.link.on_aaa_verdict(verdict, now);
        let uati = Self::active_traffic_uati(&at).unwrap_or(session_uati);
        self.apply_access_auth_outputs(&mut at, uati, outputs, &mut outcome);
        self.ats.insert(session_uati, at);
        outcome
    }

    fn apply_access_auth_outputs(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        outputs: Vec<AccessAuthOutput>,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let session_uati = at
            .session
            .session()
            .map(|s| s.uati.as_u32())
            .unwrap_or(uati);
        for output in outputs {
            match output {
                AccessAuthOutput::Send(octets) => {
                    let packets = self.build_access_stream_packets(at, uati, &octets);
                    outcome.forward_traffic.extend(packets);
                }
                AccessAuthOutput::AaaRequest(credentials) => {
                    at.access.nai = Some(credentials.nai.clone());
                    let hardware_id = self
                        .hardware_ids_by_uati
                        .get(&session_uati)
                        .or_else(|| self.hardware_ids_by_uati.get(&uati))
                        .cloned();
                    outcome.access_auth_requests.push(HrpdAccessAuthRequest {
                        session_uati,
                        traffic_uati: uati,
                        credentials,
                        hardware_id,
                    });
                }
                AccessAuthOutput::Settled(result) => {
                    outcome
                        .access_auth_settlements
                        .push(HrpdAccessAuthSettlement {
                            session_uati,
                            traffic_uati: uati,
                            result,
                        });
                }
            }
        }
    }

    fn build_access_stream_packets(
        &self,
        at: &mut AtSession,
        uati: u32,
        octets: &[u8],
    ) -> Vec<HrpdForwardTrafficPacket> {
        let Some(stream_id) = at.access.stream_id() else {
            return Vec::new();
        };
        let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati && pending.active)
        else {
            log::info!(
                "HRPD AN: dropping access stream octets UATI=0x{uati:08x}; no active traffic channel"
            );
            return Vec::new();
        };
        let Some(mac_index) = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)
        else {
            return Vec::new();
        };
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        let physical_bits = implemented_forward_traffic_payload_bits_for_drc(drc_index)
            .unwrap_or(HRPD_DEFAULT_FTC_PHYSICAL_BITS);
        let physical_layer_subtype = pending.in_use_physical_layer_subtype;
        let forward_traffic_mac_subtype = pending.in_use_forward_traffic_mac_subtype;
        let mut packets = Vec::new();
        for chunk in octets.chunks(ACCESS_STREAM_MAX_RLP_OCTETS) {
            let sequence = at.access.forward_sequence;
            at.access.forward_sequence =
                sequence.wrapping_add(chunk.len() as u32) & crate::rlp::SEQUENCE_MASK;
            match default_packet_ftc_payload_bits_many_for_mac_subtype(
                stream_id,
                &[(sequence, chunk)],
                physical_bits,
                forward_traffic_mac_subtype,
            ) {
                Ok(payload_bits) => packets.push(HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype,
                    forward_traffic_mac_subtype,
                    payload_bits,
                }),
                Err(err) => log::warn!(
                    "HRPD AN: failed to build access stream packet UATI=0x{uati:08x} seq={sequence} octets={}: {err:?}",
                    chunk.len()
                ),
            }
        }
        packets
    }
}
