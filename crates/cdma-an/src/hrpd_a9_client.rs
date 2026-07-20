//! AN-side A9 client: establishes and releases the A8 bearer with the PCF by
//! exchanging SetupA8/ReleaseA8 transactions over A9 UDP signaling.
//!
//! This is the client counterpart to the PCF's A9 server. It owns the A9
//! signaling endpoint borrow, the client config, and the A9 sequence counter,
//! so the transaction methods no longer thread those through every call.

use std::net::SocketAddr;
use std::time::Duration;

use cdma_a8::HrpdA9ClientConfig;
use cdma_common::hrpd::air as hrpd_air;
use log::info;

use crate::hrpd_identity::HrpdA9MobileIdentity;

/// A9 ReleaseA8 cause for a normal BSC-initiated release.
const HRPD_A9_RELEASE_A8_CAUSE_BSC_NORMAL: u8 = 0x14;

/// Timeout awaiting a ConnectA8 / ReleaseA8Complete reply from the PCF.
const A9_REPLY_TIMEOUT: Duration = Duration::from_millis(1_500);

/// State needed to release an established A8 bearer, captured from the ConnectA8
/// reply. Opaque to callers other than [`HrpdAnA9Client::release_a8`].
#[derive(Clone, Debug)]
pub struct HrpdA9ReleaseContext {
    call_connection_reference: Option<cdma_a9::CallConnectionReference>,
    correlation_id: Option<cdma_a9::CorrelationId>,
    identity: HrpdA9MobileIdentity,
    con_ref: cdma_a9::ConRef,
    a8_traffic_id: cdma_a9::A8TrafficId,
    pdsn_ip_address: Option<cdma_a9::PdsnIpAddress>,
}

impl HrpdA9ReleaseContext {
    /// Build a release context from message fields, e.g. when replying to a
    /// PCF-initiated DisconnectA8 for which no active session is tracked.
    pub fn from_parts(
        call_connection_reference: Option<cdma_a9::CallConnectionReference>,
        correlation_id: Option<cdma_a9::CorrelationId>,
        identity: HrpdA9MobileIdentity,
        con_ref: cdma_a9::ConRef,
        a8_traffic_id: cdma_a9::A8TrafficId,
    ) -> Self {
        Self {
            call_connection_reference,
            correlation_id,
            identity,
            con_ref,
            a8_traffic_id,
            pdsn_ip_address: None,
        }
    }

    /// GRE key of the established A8 bearer.
    pub fn a8_key(&self) -> u32 {
        self.a8_traffic_id.key
    }

    /// Connection reference (assigned MAC index).
    pub fn con_ref(&self) -> cdma_a9::ConRef {
        self.con_ref
    }

    /// PDSN IP address reported in the ConnectA8 reply, if any.
    pub fn pdsn_ip_address(&self) -> Option<cdma_a9::PdsnIpAddress> {
        self.pdsn_ip_address
    }
}

/// AN-side A9 client bound to one PCF signaling endpoint.
pub struct HrpdAnA9Client<'a> {
    endpoint: &'a cdma_a9::UdpSignalingEndpoint,
    config: HrpdA9ClientConfig,
    sequence_no: u32,
}

impl<'a> HrpdAnA9Client<'a> {
    pub fn new(endpoint: &'a cdma_a9::UdpSignalingEndpoint, config: HrpdA9ClientConfig) -> Self {
        Self::with_sequence(endpoint, config, 0)
    }

    /// Construct with an existing A9 sequence counter, so a caller that spans
    /// several transient clients across a session keeps the counter monotonic.
    pub fn with_sequence(
        endpoint: &'a cdma_a9::UdpSignalingEndpoint,
        config: HrpdA9ClientConfig,
        sequence_no: u32,
    ) -> Self {
        Self {
            endpoint,
            config,
            sequence_no,
        }
    }

    /// Current A9 sequence counter, to carry across transient clients.
    pub fn sequence_no(&self) -> u32 {
        self.sequence_no
    }

    /// AN-side A8 bearer endpoint (for constructing the local bearer session).
    pub fn an_a8_endpoint(&self) -> cdma_a8::BearerEndpoint {
        self.config.an_a8_endpoint
    }

    /// Establish an A8 bearer for `request`: send SetupA8, await and validate
    /// the ConnectA8 reply, and return the release context.
    pub async fn setup_a8(
        &mut self,
        request: &hrpd_air::HrpdTrafficAssignmentRequest,
        identity: Option<&HrpdA9MobileIdentity>,
    ) -> Result<HrpdA9ReleaseContext, String> {
        let setup = build_setup_a8(self.config, request, identity);
        let payload = setup
            .encode()
            .map_err(|err| format!("HRPD AN A9: encode SetupA8: {err}"))?;
        self.send_payload(request.uati, payload).await?;

        let connect = self
            .recv_reply(cdma_a9::MessageType::ConnectA8, "ConnectA8", |payload| {
                cdma_a9::ConnectA8Message::decode(payload)
                    .map_err(|err| format!("HRPD AN A9: decode ConnectA8: {err}"))
            })
            .await?;
        validate_connect_a8(request, &connect)?;
        Ok(HrpdA9ReleaseContext {
            call_connection_reference: connect.call_connection_reference,
            correlation_id: connect.correlation_id,
            identity: identity.cloned().unwrap_or(HrpdA9MobileIdentity {
                imsi: None,
                esn: None,
                meid: None,
            }),
            con_ref: connect.con_ref,
            a8_traffic_id: connect.a8_traffic_id,
            pdsn_ip_address: connect.pdsn_ip_address,
        })
    }

    /// Release the A8 bearer with the default BSC-normal cause.
    pub async fn release_a8(
        &mut self,
        uati: u32,
        context: &HrpdA9ReleaseContext,
        reason: &str,
    ) -> Result<(), String> {
        self.release_a8_with_cause(
            uati,
            context,
            reason,
            cdma_a9::CauseValue(HRPD_A9_RELEASE_A8_CAUSE_BSC_NORMAL),
        )
        .await
    }

    /// Release the A8 bearer with an explicit cause.
    pub async fn release_a8_with_cause(
        &mut self,
        uati: u32,
        context: &HrpdA9ReleaseContext,
        reason: &str,
        cause: cdma_a9::CauseValue,
    ) -> Result<(), String> {
        let release = cdma_a9::ReleaseA8Message {
            call_connection_reference: context.call_connection_reference,
            correlation_id: context.correlation_id,
            imsi: context.identity.imsi.clone(),
            esn: context.identity.esn,
            meid: context.identity.meid,
            con_ref: context.con_ref,
            a8_traffic_id: context.a8_traffic_id.clone(),
            cause,
        };
        let payload = release
            .encode()
            .map_err(|err| format!("HRPD AN A9: encode ReleaseA8: {err}"))?;
        self.send_payload(uati, payload).await?;

        let complete = self
            .recv_reply(
                cdma_a9::MessageType::ReleaseA8Complete,
                "ReleaseA8Complete",
                |payload| {
                    cdma_a9::ReleaseA8CompleteMessage::decode(payload)
                        .map_err(|err| format!("HRPD AN A9: decode ReleaseA8Complete: {err}"))
                },
            )
            .await?;
        if complete.call_connection_reference != context.call_connection_reference {
            return Err("HRPD AN A9: ReleaseA8Complete call reference mismatch".to_string());
        }
        if complete.correlation_id != context.correlation_id {
            return Err("HRPD AN A9: ReleaseA8Complete correlation id mismatch".to_string());
        }
        info!(
            "HRPD AN A9: ReleaseA8 complete UATI=0x{uati:08x} A8Key=0x{:08x} reason={reason}",
            context.a8_traffic_id.key
        );
        Ok(())
    }

    async fn send_payload(&mut self, session_id: u32, payload: Vec<u8>) -> Result<(), String> {
        self.sequence_no = self.sequence_no.wrapping_add(1);
        let metadata = cdma_a9::TransportMetadata {
            flags: 0,
            session_id,
            sequence_no: self.sequence_no,
        };
        let datagram = cdma_a9::UdpSignalingDatagram::new(metadata, payload)
            .map_err(|err| format!("HRPD AN A9: encode A9 UDP datagram: {err}"))?;
        self.endpoint
            .send_datagram(self.config.pcf_addr, &datagram)
            .await
            .map_err(|err| {
                format!(
                    "HRPD AN A9: send A9 UDP datagram to {}: {err}",
                    self.config.pcf_addr
                )
            })?;
        Ok(())
    }

    async fn recv_reply<T>(
        &self,
        expected: cdma_a9::MessageType,
        label: &str,
        decode: impl FnOnce(&[u8]) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut buf = vec![0u8; 4096];
        let (datagram, peer) =
            tokio::time::timeout(A9_REPLY_TIMEOUT, self.endpoint.recv_datagram(&mut buf))
                .await
                .map_err(|_| {
                    format!(
                        "HRPD AN A9: timed out waiting for {label} from {}",
                        self.config.pcf_addr
                    )
                })?
                .map_err(|err| format!("HRPD AN A9: receive {label}: {err}"))?;
        self.check_peer(peer)?;
        if datagram.message_type != expected {
            return Err(format!(
                "HRPD AN A9: expected {label}, got {:?}",
                datagram.message_type
            ));
        }
        decode(&datagram.payload)
    }

    fn check_peer(&self, peer: SocketAddr) -> Result<(), String> {
        if peer != self.config.pcf_addr {
            return Err(format!(
                "HRPD AN A9: reply came from unexpected peer {peer}, expected {}",
                self.config.pcf_addr
            ));
        }
        Ok(())
    }
}

/// Build a SetupA8 message for `request`, addressing the mobile by `identity`.
pub fn build_setup_a8(
    config: HrpdA9ClientConfig,
    request: &hrpd_air::HrpdTrafficAssignmentRequest,
    identity: Option<&HrpdA9MobileIdentity>,
) -> cdma_a9::SetupA8Message {
    // A9 MobileIdentity ordering requires IMSI before ESN/MEID. If HLR has not
    // resolved an IMSI yet, omit hardware-only identity and let PCF keep A11
    // registration deferred while still establishing A8.
    let identity = identity.filter(|identity| identity.imsi.is_some());
    cdma_a9::SetupA8Message {
        call_connection_reference: None,
        correlation_id: Some(cdma_a9::CorrelationId(request.uati.to_be_bytes())),
        imsi: identity.and_then(|identity| identity.imsi.clone()),
        esn: identity.and_then(|identity| identity.esn),
        meid: identity.and_then(|identity| identity.meid),
        con_ref: cdma_a9::ConRef(request.mac_index),
        quality_of_service_parameters: None,
        bsc_id: cdma_a9::BscId(b"1XBTS".to_vec()),
        a8_traffic_id: cdma_a9::A8TrafficId::gre_ppp(request.uati, config.a8_peer_ipv4),
        service_option: cdma_a9::ServiceOptionValue::HIGH_RATE_PACKET_DATA,
        a9_indicators: cdma_a9::A9Indicators {
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

/// Validate a ConnectA8 reply against the assignment it answers.
pub fn validate_connect_a8(
    request: &hrpd_air::HrpdTrafficAssignmentRequest,
    connect: &cdma_a9::ConnectA8Message,
) -> Result<(), String> {
    if connect.con_ref.0 != request.mac_index {
        return Err(format!(
            "ConnectA8 con_ref={} does not match assigned MAC index {}",
            connect.con_ref.0, request.mac_index
        ));
    }
    if connect.a8_traffic_id.key != request.uati {
        return Err(format!(
            "ConnectA8 A8 key=0x{:08x} does not match UATI=0x{:08x}",
            connect.a8_traffic_id.key, request.uati
        ));
    }
    if connect.cause != cdma_a9::CauseValue::A8_CONNECTION_COMPLETE {
        return Err(format!(
            "ConnectA8 returned unsupported cause=0x{:02x}",
            connect.cause.0
        ));
    }
    Ok(())
}
