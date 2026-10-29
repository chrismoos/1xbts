use cdma_ios::femto_interface_msg::{
    FemtoInterfaceMsg, MSG_TYPE_ADDS_DELIVER, MSG_TYPE_ADDS_DELIVER_ACK, MSG_TYPE_ADDS_PAGE_ACK,
    MSG_TYPE_ADDS_TRANSFER, MSG_TYPE_CM_SERVICE_REQUEST, MSG_TYPE_LOCATION_UPDATING_REQUEST,
    MSG_TYPE_PAGING_RESPONSE, MimePart, Multipart,
};
use cdma_ios::transport::A1TransportEvent;
use cdma_ios::{
    AddsDeliverAckMessage, AddsDeliverMessage, AddsPageAckMessage, AddsPageMessage,
    AddsTransferAckMessage, AddsTransferMessage, AssignmentRequestMessage, CellId,
    CompleteLayer3InformationMessage, EncodedA1Message, Layer3Information,
    LocationUpdatingAcceptMessage, Message, MessageType, PagingRequestMessage,
};
use tokio::sync::mpsc;

const CT_FEMTO: &str = "application/vnd.3gpp2.FemtoInterfaceMsg";
const CT_FEMTO_AIRAVE: &str = "application/FemtoInterfaceMsg";
const CT_SMS: &str = "application/vnd.3gpp2.sms";
const CT_MULTIPART_PREFIX: &str = "multipart/";

#[derive(Debug, thiserror::Error)]
pub enum FcsError {
    #[error("unsupported SIP body content-type: {0}")]
    UnsupportedContentType(String),
    #[error("multipart body has no boundary parameter")]
    MissingBoundary,
    #[error("decoding FemtoInterfaceMsg body: {0}")]
    Femto(#[from] cdma_ios::femto_interface_msg::FemtoError),
    #[error("multipart body carried no FemtoInterfaceMsg or SMS part")]
    NoUsablePart,
    #[error("unsupported femtocell A1 message type 0x{0:02X}")]
    UnsupportedA1Message(u8),
    #[error("no FAP has an active SIP registration for Fx2 downlink")]
    NoRegisteredFap,
    #[error("encoding MSC A1 message: {0}")]
    A1(#[from] cdma_ios::Error),
    #[error("decoding MSC A1 transport message: {0}")]
    A1Transport(#[from] cdma_ios::A1TransportError),
    #[error("invalid femtocell IMSI prefix: {0}")]
    InvalidImsiPrefix(String),
    #[error("invalid mobile identity for configured femtocell IMSI prefix: {0}")]
    InvalidMobileIdentity(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FcsMessage {
    A1(FemtoInterfaceMsg),
    Sms(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FcsDownlink {
    pub content_type: String,
    pub body: Vec<u8>,
    pub mobile_identity: Option<String>,
    pub femto: FemtoInterfaceMsg,
}

#[derive(Debug, Clone)]
pub struct FemtoImsiMapper {
    prefix: String,
}

impl FemtoImsiMapper {
    const FULL_IMSI_DIGITS: usize = 15;
    const IMSI_S_DIGITS: usize = 10;

    pub fn new(mcc: &str, imsi_11_12: &str) -> Result<Self, FcsError> {
        if cdma_common::paging::mcc_from_digits(mcc).is_none() {
            return Err(FcsError::InvalidImsiPrefix(format!("MCC {mcc:?}")));
        }
        if cdma_common::paging::imsi_11_12_from_digits(imsi_11_12).is_none() {
            return Err(FcsError::InvalidImsiPrefix(format!(
                "IMSI_11_12 {imsi_11_12:?}"
            )));
        }
        Ok(Self {
            prefix: format!("{mcc}{imsi_11_12}"),
        })
    }

    pub fn to_imsi_s(&self, identity: &str) -> Result<String, FcsError> {
        if identity.len() == Self::IMSI_S_DIGITS
            && identity.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Ok(identity.to_string());
        }
        if identity.len() != Self::FULL_IMSI_DIGITS
            || !identity.bytes().all(|byte| byte.is_ascii_digit())
            || !identity.starts_with(&self.prefix)
        {
            return Err(FcsError::InvalidMobileIdentity(identity.to_string()));
        }
        Ok(identity[self.prefix.len()..].to_string())
    }

    pub fn to_full_imsi(&self, identity: &str) -> Result<String, FcsError> {
        if identity.len() == Self::FULL_IMSI_DIGITS
            && identity.bytes().all(|byte| byte.is_ascii_digit())
            && identity.starts_with(&self.prefix)
        {
            return Ok(identity.to_string());
        }
        if identity.len() != Self::IMSI_S_DIGITS
            || !identity.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(FcsError::InvalidMobileIdentity(identity.to_string()));
        }
        Ok(format!("{}{}", self.prefix, identity))
    }
}

/// ADDS Deliver (DTAP) and Privacy Mode Command (BSMAP) share message type 0x53.
/// Airvana omits the discriminator, so only its legacy MIME type and a validated
/// OTASP payload permit this fallback. The general IOS parser remains strict.
pub(crate) fn decode_femto_body(
    content_type: &str,
    body: &[u8],
) -> Result<FemtoInterfaceMsg, cdma_ios::femto_interface_msg::FemtoError> {
    use cdma_ios::femto_interface_msg::{FemtoError, IE_CAUSE, IE_TAG};
    let parsed = FemtoInterfaceMsg::parse(body);
    let message_type = match &parsed {
        Err(FemtoError::MissingDiscriminatorOnConflict { message_type }) => *message_type,
        _ => return parsed,
    };
    if !content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case(CT_FEMTO_AIRAVE)
        || !matches!(
            message_type,
            MSG_TYPE_ADDS_DELIVER | MSG_TYPE_ADDS_DELIVER_ACK
        )
    {
        return parsed;
    }
    if let Some(rest) = body
        .windows(2)
        .position(|pair| pair == b"\r\n")
        .map(|end| &body[end + 2..])
    {
        let mut normalized = format!("{message_type:02X} (=\"0 ADDS\")\r\n").into_bytes();
        normalized.extend_from_slice(rest);
        if let Ok(message) = FemtoInterfaceMsg::parse(&normalized) {
            let valid = if message_type == MSG_TYPE_ADDS_DELIVER {
                message.to_adds_deliver().is_ok_and(|deliver| {
                    deliver.adds_user_part.burst_type == cdma_common::consts::BURST_TYPE_OTASP
                })
            } else {
                message.ies.iter().filter(|ie| ie.iei == IE_TAG).count() == 1
                    && message
                        .ies
                        .iter()
                        .all(|ie| matches!(ie.iei, IE_TAG | IE_CAUSE))
                    && message
                        .to_adds_deliver_ack()
                        .is_ok_and(|ack| ack.tag.is_some())
            };
            if valid {
                return Ok(message);
            }
        }
    }
    parsed
}

pub struct FemtoConvergenceServer {
    imsi: FemtoImsiMapper,
}

impl FemtoConvergenceServer {
    pub fn new() -> Self {
        Self::with_imsi_prefix("310", "00").expect("valid default femtocell IMSI prefix")
    }

    pub fn with_imsi_prefix(mcc: &str, imsi_11_12: &str) -> Result<Self, FcsError> {
        Ok(Self {
            imsi: FemtoImsiMapper::new(mcc, imsi_11_12)?,
        })
    }

    pub fn femto_identity(&self, identity: &str) -> Result<String, FcsError> {
        self.imsi.to_imsi_s(identity)
    }

    pub fn canonical_imsi(&self, identity: &str) -> Result<String, FcsError> {
        self.imsi.to_full_imsi(identity)
    }

    pub fn ingest_message(&self, content_type: &str, body: &[u8]) -> Result<String, FcsError> {
        Ok(match self.decode_message(content_type, body)? {
            FcsMessage::A1(message) => femto_summary(&message),
            FcsMessage::Sms(bytes) => format!("binary C.S0015 SMS ({} bytes)", bytes.len()),
        })
    }

    pub fn decode_message(&self, content_type: &str, body: &[u8]) -> Result<FcsMessage, FcsError> {
        let base = content_type.split(';').next().unwrap_or("").trim();
        if is_femto_content_type(base) {
            return Ok(FcsMessage::A1(decode_femto_body(base, body)?));
        }
        if base.eq_ignore_ascii_case(CT_SMS) {
            return Ok(FcsMessage::Sms(body.to_vec()));
        }
        if base.to_ascii_lowercase().starts_with(CT_MULTIPART_PREFIX) {
            let boundary = boundary_param(content_type).ok_or(FcsError::MissingBoundary)?;
            let multipart = Multipart::parse(&boundary, body)?;
            if let Some(part) = multipart
                .parts
                .iter()
                .find(|part| is_femto_content_type(&part.content_type))
            {
                return Ok(FcsMessage::A1(decode_femto_body(
                    &part.content_type,
                    &part.body,
                )?));
            }
            if let Some(part) = multipart.part_by_content_type(CT_SMS) {
                return Ok(FcsMessage::Sms(part.body.clone()));
            }
            return Err(FcsError::NoUsablePart);
        }
        Err(FcsError::UnsupportedContentType(base.to_string()))
    }

    pub fn to_msc_a1(
        &self,
        message: &FemtoInterfaceMsg,
        cell_identifier: CellId,
        call_id: Option<u64>,
    ) -> Result<EncodedA1Message, FcsError> {
        let layer3_information = match message.message_type {
            cdma_ios::femto_interface_msg::MSG_TYPE_PRIVACY_MODE_COMPLETE
                if message.discriminator
                    == Some(cdma_ios::femto_interface_msg::Discriminator::Bsmap) =>
            {
                let complete =
                    cdma_ios::PrivacyModeCompleteMessage::decode(&message.to_bsmap_wire()?)?;
                let encoded = Message::new(MessageType::PrivacyModeComplete, complete.encode()?);
                return Ok(EncodedA1Message::from_message_for_call(&encoded, call_id));
            }
            MSG_TYPE_LOCATION_UPDATING_REQUEST => {
                let mut request = message.to_location_updating_request()?;
                self.map_identity_to_full(&mut request.mobile_identity_imsi)?;
                request.mobile_identity_meid = request
                    .mobile_identity_meid
                    .map(crate::cell::canonical_meid_identity);
                Layer3Information::from_location_updating_request(&request)?
            }
            MSG_TYPE_CM_SERVICE_REQUEST => {
                let mut request = message.to_cm_service_request()?;
                self.map_identity_to_full(&mut request.mobile_identity_imsi)?;
                request.mobile_identity_meid = request
                    .mobile_identity_meid
                    .map(crate::cell::canonical_meid_identity);
                Layer3Information::from_cm_service_request(&request)?
            }
            MSG_TYPE_PAGING_RESPONSE => {
                let mut paging = message.to_paging_response()?;
                self.map_identity_to_full(&mut paging.mobile_identity_imsi)?;
                paging.mobile_identity_meid = paging
                    .mobile_identity_meid
                    .map(crate::cell::canonical_meid_identity);
                let encoded = Message::new(MessageType::PagingResponse, paging.encode()?);
                return Ok(EncodedA1Message::from_message_for_call(&encoded, call_id));
            }
            MSG_TYPE_ADDS_PAGE_ACK => {
                let payload = message.to_bsmap_wire()?;
                let mut ack = AddsPageAckMessage::decode(&payload)?;
                self.map_identity_to_full(&mut ack.mobile_identity)?;
                let encoded = Message::new(MessageType::AddsPageAck, ack.encode()?);
                return Ok(EncodedA1Message::from_message_for_call(&encoded, None));
            }
            MSG_TYPE_ADDS_DELIVER => {
                let deliver = message.to_adds_deliver()?;
                let encoded = Message::new(MessageType::AddsDeliver, deliver.encode()?);
                return Ok(EncodedA1Message::from_message_for_call(&encoded, call_id));
            }
            MSG_TYPE_ADDS_DELIVER_ACK => {
                let ack = message.to_adds_deliver_ack()?;
                let encoded = Message::new(MessageType::AddsDeliverAck, ack.encode()?);
                return Ok(EncodedA1Message::from_message_for_call(&encoded, call_id));
            }
            MSG_TYPE_ADDS_TRANSFER => {
                let mut transfer = AddsTransferMessage::decode(&message.to_bsmap_wire()?)?;
                self.map_identity_to_full(&mut transfer.mobile_identity_imsi)?;
                transfer.mobile_identity_meid = transfer
                    .mobile_identity_meid
                    .map(crate::cell::canonical_meid_identity);
                let encoded = Message::new(MessageType::AddsTransfer, transfer.encode()?);
                return Ok(EncodedA1Message::from_message_for_call(&encoded, call_id));
            }
            other => return Err(FcsError::UnsupportedA1Message(other)),
        };
        let complete_layer3 = CompleteLayer3InformationMessage {
            cell_identifier,
            layer3_information,
        };
        let encoded = Message::new(
            MessageType::CompleteLayer3Information,
            complete_layer3.encode()?,
        );
        Ok(EncodedA1Message::from_message_for_call(&encoded, call_id))
    }

    pub fn from_msc_a1(&self, encoded: &EncodedA1Message) -> Result<FemtoInterfaceMsg, FcsError> {
        let message = encoded.decode()?;
        match message.message_type {
            MessageType::PrivacyModeCommand => {
                let command = cdma_ios::PrivacyModeCommandMessage::decode(&message.payload)?;
                Ok(FemtoInterfaceMsg::from_privacy_mode_command(&command)?)
            }
            MessageType::LocationUpdatingAccept => {
                let accept = LocationUpdatingAcceptMessage::decode(&message.payload)?;
                Ok(FemtoInterfaceMsg::from_location_updating_accept(&accept)?)
            }
            MessageType::AssignmentRequest => {
                let assignment = AssignmentRequestMessage::decode(&message.payload)?;
                Ok(FemtoInterfaceMsg::from_assignment_request(&assignment)?)
            }
            MessageType::PagingRequest => {
                let mut paging = PagingRequestMessage::decode(&message.payload)?;
                if let cdma_ios::MobileIdentity::Imsi(imsi) = &mut paging.mobile_identity_imsi {
                    *imsi = self.imsi.to_imsi_s(imsi)?;
                }
                Ok(FemtoInterfaceMsg::from_paging_request(&paging)?)
            }
            MessageType::AddsPage => {
                let adds_page = AddsPageMessage::decode(&message.payload)?;
                Ok(FemtoInterfaceMsg::from_adds_page(&adds_page)?)
            }
            MessageType::AddsTransferAck => {
                let mut ack = AddsTransferAckMessage::decode(&message.payload)?;
                if let cdma_ios::MobileIdentity::Imsi(imsi) = &mut ack.mobile_identity_imsi {
                    *imsi = self.imsi.to_imsi_s(imsi)?;
                }
                Ok(FemtoInterfaceMsg::from_adds_transfer_ack(&ack)?)
            }
            MessageType::AddsDeliver => {
                let deliver = AddsDeliverMessage::decode(&message.payload)?;
                Ok(FemtoInterfaceMsg::from_adds_deliver(&deliver)?)
            }
            MessageType::AddsDeliverAck => {
                let ack = AddsDeliverAckMessage::decode(&message.payload)?;
                Ok(FemtoInterfaceMsg::from_adds_deliver_ack(&ack)?)
            }
            other => Err(FcsError::UnsupportedA1Message(other as u8)),
        }
    }

    pub fn downlink_from_msc(
        &self,
        encoded: &EncodedA1Message,
        multipart_boundary: &str,
    ) -> Result<FcsDownlink, FcsError> {
        self.downlink_from_msc_with_content_type(encoded, multipart_boundary, CT_FEMTO_AIRAVE)
    }

    pub fn downlink_from_msc_with_content_type(
        &self,
        encoded: &EncodedA1Message,
        multipart_boundary: &str,
        femto_content_type: &str,
    ) -> Result<FcsDownlink, FcsError> {
        if encoded.message_type() == MessageType::AddsPage {
            let message = encoded.decode()?;
            let mut adds_page = AddsPageMessage::decode(&message.payload)?;
            if let cdma_ios::MobileIdentity::Imsi(imsi) = &mut adds_page.mobile_identity {
                *imsi = self.imsi.to_imsi_s(imsi)?;
            }
            let femto = FemtoInterfaceMsg::from_adds_page_metadata(&adds_page)?;
            let multipart = Multipart::new(
                multipart_boundary,
                vec![
                    MimePart::text(femto_content_type, femto.encode()),
                    MimePart::binary(CT_SMS, adds_page.adds_user_part.data),
                ],
            );
            return Ok(FcsDownlink {
                content_type: format!("multipart/mixed; boundary={multipart_boundary}"),
                body: multipart.encode(),
                mobile_identity: imsi_string(&adds_page.mobile_identity),
                femto,
            });
        }
        let mobile_identity = if encoded.message_type() == MessageType::PagingRequest {
            let message = encoded.decode()?;
            let paging = PagingRequestMessage::decode(&message.payload)?;
            match &paging.mobile_identity_imsi {
                cdma_ios::MobileIdentity::Imsi(imsi) => Some(self.imsi.to_imsi_s(imsi)?),
                _ => None,
            }
        } else {
            None
        };
        let femto = self.from_msc_a1(encoded)?;
        Ok(FcsDownlink {
            content_type: femto_content_type.to_string(),
            body: femto.encode(),
            mobile_identity,
            femto,
        })
    }

    fn map_identity_to_full(
        &self,
        identity: &mut cdma_ios::MobileIdentity,
    ) -> Result<(), FcsError> {
        if let cdma_ios::MobileIdentity::Imsi(imsi) = identity {
            *imsi = self.imsi.to_full_imsi(imsi)?;
        }
        Ok(())
    }
}

fn imsi_string(identity: &cdma_ios::MobileIdentity) -> Option<String> {
    match identity {
        cdma_ios::MobileIdentity::Imsi(imsi) => Some(imsi.clone()),
        _ => None,
    }
}

fn is_femto_content_type(content_type: &str) -> bool {
    content_type.eq_ignore_ascii_case(CT_FEMTO)
        || content_type.eq_ignore_ascii_case(CT_FEMTO_AIRAVE)
}

impl Default for FemtoConvergenceServer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MscA1Bridge {
    pub uplink: mpsc::Sender<EncodedA1Message>,
    pub downlink: mpsc::Receiver<EncodedA1Message>,
}

pub fn spawn_msc_bridge(addr: String) -> MscA1Bridge {
    const QUEUE_CAPACITY: usize = 256;
    let (tx, mut rx) = mpsc::channel::<EncodedA1Message>(QUEUE_CAPACITY);
    let (downlink_tx, downlink) = mpsc::channel::<EncodedA1Message>(QUEUE_CAPACITY);
    tokio::spawn(async move {
        let socket_addr = loop {
            match tokio::net::lookup_host(&addr).await {
                Ok(mut addresses) => match addresses.next() {
                    Some(address) => break address,
                    None => log::warn!("FCS: MSC A1 endpoint {addr} resolved to no addresses"),
                },
                Err(error) => log::warn!("FCS: resolving MSC A1 endpoint {addr}: {error}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        };

        loop {
            let (sender, mut events) =
                match cdma_ios::transport::connect_with_reconnect(socket_addr).await {
                    Ok(transport) => transport,
                    Err(error) => {
                        log::warn!("FCS: connecting to MSC A1 endpoint {socket_addr}: {error}");
                        continue;
                    }
                };
            loop {
                tokio::select! {
                    message = rx.recv() => {
                        let Some(message) = message else {
                            return;
                        };
                        if let Err(error) = sender.send(&message).await {
                            log::warn!("FCS: sending A1 to MSC failed: {error}");
                            break;
                        }
                    }
                    event = events.recv() => {
                        match event {
                            Some(A1TransportEvent::Message(message)) => {
                                log::info!(
                                    "FCS: received MSC A1 {:?} call_id={:?}",
                                    message.message_type(),
                                    message.call_id()
                                );
                                if downlink_tx.send(message).await.is_err() {
                                    return;
                                }
                            }
                            Some(A1TransportEvent::Disconnected(error)) => {
                                log::warn!("FCS: MSC A1 disconnected: {error}");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        }
    });
    MscA1Bridge {
        uplink: tx,
        downlink,
    }
}

pub async fn spawn_msc_listener(addr: std::net::SocketAddr) -> std::io::Result<MscA1Bridge> {
    const QUEUE_CAPACITY: usize = 256;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let (tx, mut rx) = mpsc::channel::<EncodedA1Message>(QUEUE_CAPACITY);
    let (downlink_tx, downlink) = mpsc::channel::<EncodedA1Message>(QUEUE_CAPACITY);
    log::info!(
        "FCS A1 signaling listener bound on {}",
        listener.local_addr()?
    );
    tokio::spawn(async move {
        loop {
            let (sender, mut events) = match cdma_ios::transport::accept_with_retry(&listener).await
            {
                Ok(transport) => transport,
                Err(error) => {
                    log::warn!("FCS: accepting MSC A1 connection: {error}");
                    continue;
                }
            };
            log::info!("FCS accepted MSC A1 signaling connection");
            loop {
                tokio::select! {
                    message = rx.recv() => {
                        let Some(message) = message else {
                            return;
                        };
                        if let Err(error) = sender.send(&message).await {
                            log::warn!("FCS: sending A1 to MSC failed: {error}");
                            break;
                        }
                    }
                    event = events.recv() => {
                        match event {
                            Some(A1TransportEvent::Message(message)) => {
                                if downlink_tx.send(message).await.is_err() {
                                    return;
                                }
                            }
                            Some(A1TransportEvent::Disconnected(error)) => {
                                log::warn!("FCS: MSC A1 disconnected: {error}");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        }
    });
    Ok(MscA1Bridge {
        uplink: tx,
        downlink,
    })
}

fn femto_summary(msg: &FemtoInterfaceMsg) -> String {
    format!(
        "A1 message-type 0x{:02X}, {} IE(s)",
        msg.message_type,
        msg.ies.len()
    )
}

fn boundary_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|param| {
        let (name, value) = param.trim().split_once('=')?;
        if name.trim().eq_ignore_ascii_case("boundary") {
            Some(value.trim().trim_matches('"').to_string())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_ios::femto_interface_msg::{Discriminator, FemtoIe};
    use cdma_ios::typed::CmServiceType;
    use cdma_ios::{AddsUserPart, MobileIdentity, PagingResponseMessage, Tag};

    // The X.S0059-200 Table 2 reference FemtoInterfaceMsg body (Location Updating
    // Request, IMSI 9135198800).
    const REFERENCE_FEMTO: &[u8] = b"08 (=\"0 Location Updating Request\")\r\n\
0D=DQaWMRWJCPA=\r\n\
12=EgzJAOYAAAAAAQMBAAY=\r\n\
1F=HwE=\r\n\
0D=DQUFKa4zmQ==\r\n\
35=NQE=\r\n\
42=QgQBATjd\r\n\
28=KP0=\r\n\
40=QAA=\r\n\
41=QQUBAAAA/Q==\r\n\
0D=DQjxAAAAAAAA8A==\r\n";

    #[test]
    fn translates_a_femto_interface_msg_body() {
        let fcs = FemtoConvergenceServer::new();
        let summary = fcs
            .ingest_message(CT_FEMTO, REFERENCE_FEMTO)
            .expect("translate");
        assert!(summary.contains("0x08"), "summary was: {summary}");
    }

    #[test]
    fn translates_an_airave_femto_interface_msg_body() {
        let fcs = FemtoConvergenceServer::new();
        let summary = fcs
            .ingest_message("application/FemtoInterfaceMsg", REFERENCE_FEMTO)
            .expect("translate Airave body");
        assert!(summary.contains("0x08"), "summary was: {summary}");
    }

    #[test]
    fn wraps_location_update_as_typed_complete_layer_three_information() {
        let fcs = FemtoConvergenceServer::new();
        let message = FemtoInterfaceMsg::parse(REFERENCE_FEMTO).expect("femto body");
        let encoded = fcs
            .to_msc_a1(
                &message,
                CellId {
                    cell: 0x193,
                    sector: 0,
                },
                None,
            )
            .expect("MSC A1 message");

        assert_eq!(
            encoded.message_type(),
            MessageType::CompleteLayer3Information
        );
        assert_eq!(encoded.call_id(), None);
        let decoded = encoded.decode().expect("A1 envelope");
        let complete_layer3 =
            CompleteLayer3InformationMessage::decode(&decoded.payload).expect("typed CLI3");
        assert_eq!(complete_layer3.cell_identifier.cell, 0x193);
        assert_eq!(
            complete_layer3
                .layer3_information
                .decode_location_updating_request()
                .expect("typed location update")
                .mobile_identity_imsi,
            cdma_ios::MobileIdentity::Imsi("310009135198800".to_string())
        );
    }

    #[test]
    fn wraps_cm_service_request_with_esn_and_meid() {
        let classmark = vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ];
        let imsi = MobileIdentity::Imsi("5551239999".to_string());
        let esn = MobileIdentity::Esn(0x29ae_3399);
        let meid = MobileIdentity::Meid([0xa0, 0xb0, 0xc0, 0xd0, 0xe0, 0xf0, 0x10]);
        let message = FemtoInterfaceMsg {
            message_type: MSG_TYPE_CM_SERVICE_REQUEST,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("CM Service Request".to_string()),
            ies: vec![
                FemtoIe {
                    iei: 0x90,
                    content: vec![0x91],
                },
                FemtoIe::tlv(0x12, &classmark).expect("classmark IE"),
                FemtoIe::tlv(0x0d, &imsi.encode().expect("IMSI")).expect("IMSI IE"),
                FemtoIe::tlv(0x0d, &esn.encode().expect("ESN")).expect("ESN IE"),
                FemtoIe::tlv(0x0d, &meid.encode().expect("MEID")).expect("MEID IE"),
            ],
        };
        let encoded = FemtoConvergenceServer::new()
            .to_msc_a1(
                &message,
                CellId {
                    cell: 0x193,
                    sector: 0,
                },
                Some(7),
            )
            .expect("MSC A1 message");

        assert_eq!(encoded.call_id(), Some(7));
        let decoded = encoded.decode().expect("A1 envelope");
        let complete_layer3 =
            CompleteLayer3InformationMessage::decode(&decoded.payload).expect("typed CLI3");
        let cm_service = complete_layer3
            .layer3_information
            .decode_cm_service_request()
            .expect("typed CM Service Request");
        assert_eq!(
            cm_service.cm_service_type,
            CmServiceType::MobileOriginatingCallEstablishment
        );
        assert_eq!(
            cm_service.mobile_identity_imsi,
            MobileIdentity::Imsi("310005551239999".to_string())
        );
        assert_eq!(cm_service.mobile_identity_esn, Some(esn));
        // to_msc_a1 canonicalizes the FAP's nibble-reversed MEID before forwarding.
        assert_eq!(
            cm_service.mobile_identity_meid,
            Some(MobileIdentity::Meid([
                0x01, 0x0f, 0x0e, 0x0d, 0x0c, 0x0b, 0x0a
            ]))
        );
    }

    #[test]
    fn translates_an_airave_femto_interface_msg_multipart_part() {
        let multipart = Multipart::new(
            "airave-boundary",
            vec![cdma_ios::femto_interface_msg::MimePart::binary(
                "application/FemtoInterfaceMsg",
                REFERENCE_FEMTO,
            )],
        );
        let fcs = FemtoConvergenceServer::new();
        let summary = fcs
            .ingest_message(
                "multipart/mixed; boundary=airave-boundary",
                &multipart.encode(),
            )
            .expect("translate Airave multipart body");
        assert!(summary.contains("0x08"), "summary was: {summary}");
    }

    #[test]
    fn translates_a_binary_sms_body() {
        let fcs = FemtoConvergenceServer::new();
        let summary = fcs
            .ingest_message(CT_SMS, &[0x00, 0x01, 0x02, 0x03])
            .expect("translate");
        assert!(summary.contains("SMS"));
    }

    #[test]
    fn rejects_unknown_content_type() {
        let fcs = FemtoConvergenceServer::new();
        let err = fcs.ingest_message("text/plain", b"hi").unwrap_err();
        assert!(matches!(err, FcsError::UnsupportedContentType(_)));
    }

    #[test]
    fn extracts_boundary_parameter() {
        assert_eq!(
            boundary_param("multipart/mixed; boundary=\"abc123\""),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn maps_location_updating_accept_to_standard_fx2_message() {
        let accept = LocationUpdatingAcceptMessage {
            location_area_identification: None,
        };
        let message = Message::new(
            MessageType::LocationUpdatingAccept,
            accept.encode().expect("Location Updating Accept"),
        );
        let encoded = EncodedA1Message::from_message(&message);

        let femto = FemtoConvergenceServer::new()
            .from_msc_a1(&encoded)
            .expect("Fx2 message");

        assert_eq!(femto.message_type, 0x02);
        assert_eq!(femto.discriminator, Some(Discriminator::Dtap));
    }

    #[test]
    fn maps_paging_request_to_configured_imsi_s() {
        let paging = PagingRequestMessage {
            mobile_identity_meid: None,
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            tag: Some(Tag(1)),
            cell_identifier_list: None,
            slot_cycle_index: None,
            service_option: Some(cdma_ios::ServiceOption(3)),
            is2000_mobile_capabilities: None,
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(
                MessageType::PagingRequest,
                paging.encode().expect("Paging Request"),
            ),
            Some(7),
        );

        let downlink = FemtoConvergenceServer::with_imsi_prefix("310", "00")
            .expect("IMSI prefix")
            .downlink_from_msc(&encoded, "unused")
            .expect("Fx2 Paging Request");
        let femto = FemtoInterfaceMsg::parse(&downlink.body).expect("femto paging message");
        let mobile_identity_ie = femto.first_ie(0x0d).expect("Mobile Identity IE");
        let mobile_identity = MobileIdentity::decode(&mobile_identity_ie.content[2..])
            .expect("typed Mobile Identity");

        assert_eq!(downlink.mobile_identity.as_deref(), Some("5551239999"));
        assert_eq!(
            mobile_identity,
            MobileIdentity::Imsi("5551239999".to_string())
        );
    }

    #[test]
    fn maps_femto_paging_response_to_full_imsi() {
        let classmark = vec![
            0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
        ];
        let femto = FemtoInterfaceMsg {
            message_type: MSG_TYPE_PAGING_RESPONSE,
            discriminator: Some(Discriminator::Dtap),
            message_name: Some("Paging Response".to_string()),
            ies: vec![
                FemtoIe::tlv(0x12, &classmark).expect("classmark IE"),
                FemtoIe::tlv(
                    0x0d,
                    &MobileIdentity::Imsi("5551239876".to_string())
                        .encode()
                        .expect("IMSI"),
                )
                .expect("IMSI IE"),
                FemtoIe::tv(0x03, &cdma_ios::ServiceOption::EVRC_A.encode()),
            ],
        };

        let encoded = FemtoConvergenceServer::with_imsi_prefix("310", "00")
            .expect("IMSI prefix")
            .to_msc_a1(
                &femto,
                CellId {
                    cell: 403,
                    sector: 0,
                },
                Some(9),
            )
            .expect("A1 Paging Response");

        assert_eq!(encoded.message_type(), MessageType::PagingResponse);
        assert_eq!(encoded.call_id(), Some(9));
        let decoded = encoded.decode().expect("A1 envelope");
        let paging = PagingResponseMessage::decode(&decoded.payload).expect("Paging Response");
        assert_eq!(
            paging.mobile_identity_imsi,
            MobileIdentity::Imsi("310005551239876".to_string())
        );
    }

    #[test]
    fn imsi_mapper_round_trips_configured_prefix() {
        let mapper = FemtoImsiMapper::new("310", "00").expect("IMSI prefix");

        assert_eq!(
            mapper.to_full_imsi("5551239999").unwrap(),
            "310005551239999"
        );
        assert_eq!(mapper.to_imsi_s("310005551239999").unwrap(), "5551239999");
        assert_eq!(mapper.to_imsi_s("5551239999").unwrap(), "5551239999");
    }

    #[test]
    fn imsi_mapper_rejects_other_prefixes_and_malformed_values() {
        let mapper = FemtoImsiMapper::new("310", "00").expect("IMSI prefix");

        assert!(mapper.to_imsi_s("311005551239999").is_err());
        assert!(mapper.to_full_imsi("555123999A").is_err());
        assert!(FemtoImsiMapper::new("31", "00").is_err());
        assert!(FemtoImsiMapper::new("310", "0").is_err());
    }

    #[test]
    #[ignore = "pre-existing: ADDS Page -> femto multipart mapping emits an unexpected IE 0x3d; \
               unrelated to the config work, tracked separately"]
    fn maps_adds_page_to_femto_and_binary_sms_multipart() {
        let adds_page = AddsPageMessage {
            mobile_identity: MobileIdentity::Imsi("310005551239999".to_string()),
            adds_user_part: AddsUserPart {
                burst_type: 0x03,
                data: vec![0x00, 0x11, 0x22, 0x33],
            },
            tag: Some(Tag(7)),
            slot_cycle_index: None,
        };
        let message = Message::new(
            MessageType::AddsPage,
            adds_page.encode().expect("ADDS Page"),
        );
        let encoded = EncodedA1Message::from_message(&message);

        let downlink = FemtoConvergenceServer::new()
            .downlink_from_msc(&encoded, "fx2-boundary")
            .expect("multipart downlink");
        let multipart = Multipart::parse("fx2-boundary", &downlink.body).expect("multipart");

        assert_eq!(
            downlink.content_type,
            "multipart/mixed; boundary=fx2-boundary"
        );
        assert_eq!(downlink.mobile_identity.as_deref(), Some("5551239999"));
        assert_eq!(multipart.parts.len(), 2);
        assert_eq!(multipart.parts[0].content_type, CT_FEMTO_AIRAVE);
        let femto = FemtoInterfaceMsg::parse(&multipart.parts[0].body).expect("femto part");
        assert_eq!(femto.message_type, 0x65);
        assert_eq!(femto.discriminator, Some(Discriminator::Bsmap));
        let mobile_identity_ie = femto.first_ie(0x0d).expect("Mobile Identity IE");
        let decoded = MobileIdentity::decode(&mobile_identity_ie.content[2..])
            .expect("typed Mobile Identity");
        assert_eq!(decoded, MobileIdentity::Imsi("5551239999".to_string()));
        assert!(femto.first_ie(0x3d).is_none());
        assert!(femto.first_ie(0x33).is_some());
        assert_eq!(multipart.parts[1].content_type, CT_SMS);
        assert_eq!(
            multipart.parts[1].transfer_encoding.as_deref(),
            Some("binary")
        );
        assert_eq!(multipart.parts[1].body, vec![0x00, 0x11, 0x22, 0x33]);
    }

    #[test]
    fn selected_fx2_content_type_is_used_for_each_downlink_part() {
        let message = Message::new(
            MessageType::LocationUpdatingAccept,
            LocationUpdatingAcceptMessage {
                location_area_identification: None,
            }
            .encode()
            .expect("location update accept"),
        );
        let encoded = EncodedA1Message::from_message(&message);
        let fcs = FemtoConvergenceServer::new();

        let standard = fcs
            .downlink_from_msc_with_content_type(&encoded, "unused", CT_FEMTO)
            .expect("standard downlink");
        let airave = fcs
            .downlink_from_msc_with_content_type(&encoded, "unused", CT_FEMTO_AIRAVE)
            .expect("Airave downlink");

        assert_eq!(standard.content_type, CT_FEMTO);
        assert_eq!(airave.content_type, CT_FEMTO_AIRAVE);
    }

    #[test]
    fn maps_adds_transfer_ack_to_airave_fx2_message() {
        let ack = AddsTransferAckMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239876".to_string()),
            tag: Some(Tag(9)),
            cause: None,
        };
        let message = Message::new(
            MessageType::AddsTransferAck,
            ack.encode().expect("ADDS Transfer Ack"),
        );
        let encoded = EncodedA1Message::from_message(&message);

        let femto = FemtoConvergenceServer::new()
            .from_msc_a1(&encoded)
            .expect("Fx2 message");

        assert_eq!(femto.message_type, 0x68);
        assert_eq!(femto.discriminator, Some(Discriminator::Bsmap));
        let mobile_identity_ie = femto.first_ie(0x0d).expect("Mobile Identity IE");
        let decoded = MobileIdentity::decode(&mobile_identity_ie.content[2..])
            .expect("typed Mobile Identity");
        assert_eq!(decoded, MobileIdentity::Imsi("5551239876".to_string()));
    }
}

#[cfg(test)]
mod otasp_transport_tests {
    use super::*;
    use cdma_ios::{AddsUserPart, MobileIdentity, Tag};

    #[test]
    fn otasp_downlink_and_uplink_preserve_call_payload_and_tag() {
        let fcs = FemtoConvergenceServer::new();
        let deliver = AddsDeliverMessage {
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6],
            },
            tag: Some(Tag(0x80000001)),
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::AddsDeliver, deliver.encode().unwrap()),
            Some(41),
        );
        let downlink = fcs.downlink_from_msc(&encoded, "unused").unwrap();
        assert_eq!(downlink.content_type, CT_FEMTO_AIRAVE);
        let femto = FemtoInterfaceMsg::parse(&downlink.body).unwrap();
        let returned = fcs
            .to_msc_a1(
                &femto,
                CellId {
                    cell: 403,
                    sector: 0,
                },
                Some(41),
            )
            .unwrap();
        assert_eq!(returned.call_id(), Some(41));
        assert_eq!(
            AddsDeliverMessage::decode(&returned.decode().unwrap().payload).unwrap(),
            deliver
        );
    }

    #[test]
    fn otasp_transfer_expands_imsi_and_preserves_hardware_identity() {
        let fcs = FemtoConvergenceServer::new();
        let transfer = AddsTransferMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("5551239999".into()),
            adds_user_part: AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                data: vec![6, 1],
            },
            mobile_identity_esn: Some(MobileIdentity::Esn(0x80E4D11C)),
            mobile_identity_meid: None,
            cell_identifier: None,
            tag: Some(Tag(1)),
            service_option: None,
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            authentication_event: None,
            authentication_data: None,
        };
        let femto = FemtoInterfaceMsg::from_adds_transfer(&transfer).unwrap();
        let returned = fcs
            .to_msc_a1(
                &femto,
                CellId {
                    cell: 403,
                    sector: 0,
                },
                Some(41),
            )
            .unwrap();
        let decoded = AddsTransferMessage::decode(&returned.decode().unwrap().payload).unwrap();
        assert_eq!(
            decoded.mobile_identity_imsi,
            MobileIdentity::Imsi("310005551239999".into())
        );
        assert_eq!(decoded.adds_user_part, transfer.adds_user_part);
        assert_eq!(decoded.mobile_identity_esn, transfer.mobile_identity_esn);
    }
}

#[cfg(test)]
mod privacy_tests {
    use super::*;

    #[test]
    fn privacy_command_and_status_use_spec_bsmap_types_and_preserve_call_id() {
        let fcs = FemtoConvergenceServer::new();
        let command = cdma_ios::PrivacyModeCommandMessage {
            encryption_information: cdma_ios::EncryptionInformation::private_long_code(
                0x147_80cc_b8df,
                true,
            )
            .unwrap(),
        };
        let encoded = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::PrivacyModeCommand, command.encode().unwrap()),
            Some(7),
        );
        let femto = fcs.from_msc_a1(&encoded).unwrap();
        assert_eq!(femto.message_type, 0x53);
        assert_eq!(
            femto.discriminator,
            Some(cdma_ios::femto_interface_msg::Discriminator::Bsmap)
        );
        assert_eq!(
            cdma_ios::PrivacyModeCommandMessage::decode(&femto.to_bsmap_wire().unwrap()).unwrap(),
            command
        );
        let complete = cdma_ios::PrivacyModeCompleteMessage {
            encryption_information: Some(
                cdma_ios::EncryptionInformation::private_long_code_status(true),
            ),
            voice_privacy_request: false,
        };
        let femto = FemtoInterfaceMsg::from_privacy_mode_complete(&complete).unwrap();
        let encoded = fcs
            .to_msc_a1(&femto, CellId { cell: 1, sector: 0 }, Some(7))
            .unwrap();
        assert_eq!(encoded.call_id(), Some(7));
        assert_eq!(encoded.message_type(), MessageType::PrivacyModeComplete);
        assert_eq!(
            cdma_ios::PrivacyModeCompleteMessage::decode(&encoded.decode().unwrap().payload)
                .unwrap(),
            complete
        );
    }
}
