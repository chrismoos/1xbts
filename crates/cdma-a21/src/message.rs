//! A21 messages (A.S0008-C §5.1.6) and elements (§5.2.4). Uninterpreted
//! elements re-encode unchanged.

use crate::error::{A21Error, Result};

/// A21 Message Type (A.S0008-C §5.2.4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum A21MessageType {
    AirInterfaceSignaling = 0x01,
    Ack = 0x02,
    OneXParameters = 0x03,
    EventNotification = 0x04,
    OneXParametersRequest = 0x05,
    ServiceRequest = 0x06,
    ServiceResponse = 0x07,
    RadioUpdateRequest = 0x08,
    RadioUpdateResponse = 0x09,
}

impl A21MessageType {
    pub fn from_u8(value: u8) -> Result<Self> {
        Ok(match value {
            0x01 => Self::AirInterfaceSignaling,
            0x02 => Self::Ack,
            0x03 => Self::OneXParameters,
            0x04 => Self::EventNotification,
            0x05 => Self::OneXParametersRequest,
            0x06 => Self::ServiceRequest,
            0x07 => Self::ServiceResponse,
            0x08 => Self::RadioUpdateRequest,
            0x09 => Self::RadioUpdateResponse,
            other => return Err(A21Error::UnknownMessageType(other)),
        })
    }
}

/// A21 information element identifiers (A.S0008-C §5.2.4.1).
pub mod iei {
    pub const LAC_ENCAPSULATED_PDU: u8 = 0x01;
    pub const ONE_X_PARAMETERS: u8 = 0x02;
    pub const PILOT_LIST: u8 = 0x03;
    pub const CORRELATION_ID: u8 = 0x04;
    pub const MOBILE_IDENTITY: u8 = 0x05;
    pub const AUTHENTICATION_CHALLENGE_PARAMETER: u8 = 0x06;
    pub const MESSAGE_TRANSMISSION_CONTROL: u8 = 0x07;
    pub const CAUSE: u8 = 0x08;
    pub const EVENT: u8 = 0x09;
    pub const SERVICE_OPTION: u8 = 0x0a;
    pub const MOBILE_SUBSCRIPTION_INFORMATION: u8 = 0x0b;
}

/// Mobile Identity (MN ID) (A.S0008-C §5.2.4.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIdentity {
    /// Decimal IMSI digits.
    Imsi(String),
    Esn(u32),
    /// Fourteen hexadecimal MEID digits.
    Meid(String),
}

const IDENTITY_TYPE_MEID: u8 = 0b001;
const IDENTITY_TYPE_ESN: u8 = 0b101;
const IDENTITY_TYPE_IMSI: u8 = 0b110;
const IDENTITY_TYPE_MASK: u8 = 0b111;
const IDENTITY_ODD_FLAG: u8 = 0b1000;
const IDENTITY_END_MARK: u8 = 0x0f;
const MEID_DIGITS: usize = 14;
const ESN_IDENTITY_LEN: usize = 5;

impl MobileIdentity {
    fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Esn(esn) => {
                let mut out = vec![IDENTITY_TYPE_ESN];
                out.extend_from_slice(&esn.to_be_bytes());
                Ok(out)
            }
            Self::Imsi(digits) => encode_digits(digits, 10, IDENTITY_TYPE_IMSI),
            Self::Meid(digits) => {
                if digits.len() != MEID_DIGITS {
                    return Err(A21Error::InvalidValue {
                        context: "MEID",
                        reason: "must be 14 hexadecimal digits",
                    });
                }
                let mut out = encode_digits(digits, 16, IDENTITY_TYPE_MEID)?;
                out[0] &= !IDENTITY_ODD_FLAG;
                Ok(out)
            }
        }
    }

    fn decode(value: &[u8]) -> Result<Self> {
        let Some(&first) = value.first() else {
            return Err(A21Error::Truncated {
                context: "Mobile Identity",
                needed: 1,
                actual: 0,
            });
        };
        match first & IDENTITY_TYPE_MASK {
            IDENTITY_TYPE_ESN => {
                if value.len() != ESN_IDENTITY_LEN {
                    return Err(A21Error::Truncated {
                        context: "ESN Mobile Identity",
                        needed: ESN_IDENTITY_LEN,
                        actual: value.len(),
                    });
                }
                Ok(Self::Esn(u32::from_be_bytes([
                    value[1], value[2], value[3], value[4],
                ])))
            }
            IDENTITY_TYPE_IMSI => Ok(Self::Imsi(decode_digits(value, 10)?)),
            IDENTITY_TYPE_MEID => Ok(Self::Meid(decode_digits(value, 16)?)),
            other => Err(A21Error::InvalidValue {
                context: "Mobile Identity type",
                reason: if other == 0 {
                    "no identity code"
                } else {
                    "reserved identity type"
                },
            }),
        }
    }
}

/// Packs identity digits: digit 1 in the high nibble of the first octet, then
/// pairs low nibble first, with an end mark after an even digit count.
fn encode_digits(digits: &str, radix: u32, identity_type: u8) -> Result<Vec<u8>> {
    let nibbles = digits
        .chars()
        .map(|digit| digit.to_digit(radix).map(|value| value as u8))
        .collect::<Option<Vec<u8>>>()
        .ok_or(A21Error::InvalidValue {
            context: "Mobile Identity digits",
            reason: "digit outside the identity's coding",
        })?;
    let Some((&first, rest)) = nibbles.split_first() else {
        return Err(A21Error::InvalidValue {
            context: "Mobile Identity digits",
            reason: "no digits",
        });
    };
    let odd = nibbles.len() % 2 == 1;
    let mut out = vec![(first << 4) | if odd { IDENTITY_ODD_FLAG } else { 0 } | identity_type];
    for pair in rest.chunks(2) {
        let high = pair.get(1).copied().unwrap_or(IDENTITY_END_MARK);
        out.push((high << 4) | pair[0]);
    }
    Ok(out)
}

fn decode_digits(value: &[u8], radix: u32) -> Result<String> {
    let render = |nibble: u8| {
        char::from_digit(u32::from(nibble), radix)
            .map(|digit| digit.to_ascii_uppercase())
            .ok_or(A21Error::InvalidValue {
                context: "Mobile Identity digits",
                reason: "digit outside the identity's coding",
            })
    };
    let mut digits = String::new();
    digits.push(render(value[0] >> 4)?);
    for octet in &value[1..] {
        digits.push(render(octet & 0x0f)?);
        let high = octet >> 4;
        if high == IDENTITY_END_MARK {
            break;
        }
        digits.push(render(high)?);
    }
    Ok(digits)
}

/// A21 1x Message Transmission Control (A.S0008-C §5.2.4.10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessageTransmissionControl {
    pub paging_message: bool,
    pub simul_xmit_with_next: bool,
    pub ack_required: bool,
    /// 3G1XLogicalChannel: `false` for the 1x f-csch / r-csch, `true` for the
    /// dedicated channel.
    pub logical_channel_dedicated: bool,
    pub protocol_revision: u8,
}

const TX_CONTROL_PAGING_MESSAGE: u8 = 0b1000;
const TX_CONTROL_SIMUL_XMIT: u8 = 0b0100;
const TX_CONTROL_ACK_REQUIRED: u8 = 0b0010;
const TX_CONTROL_LOGICAL_CHANNEL: u8 = 0b0001;
const TX_CONTROL_LEN: usize = 2;

/// A21 Cause value (A.S0008-C §5.2.4.11). Unlisted values keep their code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A21Cause {
    UnknownMobile,
    UnknownCellIdentifier,
    TunnelingNotAvailable,
    ResourcesNotAvailable,
    ContextMayBeReleased,
    AirlinkLost,
    AbortHandoff,
    Unspecified,
    Rejection,
    AlreadyPaging,
    Other(u8),
}

impl A21Cause {
    pub fn value(self) -> u8 {
        match self {
            Self::UnknownMobile => 0x00,
            Self::UnknownCellIdentifier => 0x01,
            Self::TunnelingNotAvailable => 0x02,
            Self::ResourcesNotAvailable => 0x03,
            Self::ContextMayBeReleased => 0x04,
            Self::AirlinkLost => 0x05,
            Self::AbortHandoff => 0x06,
            Self::Unspecified => 0x07,
            Self::Rejection => 0x08,
            Self::AlreadyPaging => 0x09,
            Self::Other(value) => value,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0x00 => Self::UnknownMobile,
            0x01 => Self::UnknownCellIdentifier,
            0x02 => Self::TunnelingNotAvailable,
            0x03 => Self::ResourcesNotAvailable,
            0x04 => Self::ContextMayBeReleased,
            0x05 => Self::AirlinkLost,
            0x06 => Self::AbortHandoff,
            0x07 => Self::Unspecified,
            0x08 => Self::Rejection,
            0x09 => Self::AlreadyPaging,
            other => Self::Other(other),
        }
    }
}

/// A21 Event value (A.S0008-C §5.2.4.12). Unlisted values keep their code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A21EventKind {
    PresentIn1x,
    PresentInHrpd,
    OneXPowerDown,
    HrpdPowerDown,
    HandoffRejected,
    OneXRegistration,
    LacPduTransmissionDisabled,
    LacPduTransmissionEnabled,
    NoLongerPresentInAn,
    NoLongerPresentIn1xBs,
    NotAcquired,
    Redirection,
    Other(u8),
}

impl A21EventKind {
    pub fn value(self) -> u8 {
        match self {
            Self::PresentIn1x => 0x00,
            Self::PresentInHrpd => 0x01,
            Self::OneXPowerDown => 0x02,
            Self::HrpdPowerDown => 0x03,
            Self::HandoffRejected => 0x04,
            Self::OneXRegistration => 0x05,
            Self::LacPduTransmissionDisabled => 0x06,
            Self::LacPduTransmissionEnabled => 0x07,
            Self::NoLongerPresentInAn => 0x08,
            Self::NoLongerPresentIn1xBs => 0x09,
            Self::NotAcquired => 0x0a,
            Self::Redirection => 0x0b,
            Self::Other(value) => value,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0x00 => Self::PresentIn1x,
            0x01 => Self::PresentInHrpd,
            0x02 => Self::OneXPowerDown,
            0x03 => Self::HrpdPowerDown,
            0x04 => Self::HandoffRejected,
            0x05 => Self::OneXRegistration,
            0x06 => Self::LacPduTransmissionDisabled,
            0x07 => Self::LacPduTransmissionEnabled,
            0x08 => Self::NoLongerPresentInAn,
            0x09 => Self::NoLongerPresentIn1xBs,
            0x0a => Self::NotAcquired,
            0x0b => Self::Redirection,
            other => Self::Other(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A21Element {
    /// A 1x LAC Encapsulated PDU, passed through unchanged (§5.2.4.4).
    LacEncapsulatedPdu(Vec<u8>),
    CorrelationId(u32),
    MobileIdentity(MobileIdentity),
    MessageTransmissionControl(MessageTransmissionControl),
    Cause(A21Cause),
    Event {
        event: A21EventKind,
        additional_info: Vec<u8>,
    },
    ServiceOption(u16),
    Other {
        iei: u8,
        value: Vec<u8>,
    },
}

const CORRELATION_ID_LEN: usize = 4;
const SERVICE_OPTION_LEN: usize = 2;

impl A21Element {
    fn iei(&self) -> u8 {
        match self {
            Self::LacEncapsulatedPdu(_) => iei::LAC_ENCAPSULATED_PDU,
            Self::CorrelationId(_) => iei::CORRELATION_ID,
            Self::MobileIdentity(_) => iei::MOBILE_IDENTITY,
            Self::MessageTransmissionControl(_) => iei::MESSAGE_TRANSMISSION_CONTROL,
            Self::Cause(_) => iei::CAUSE,
            Self::Event { .. } => iei::EVENT,
            Self::ServiceOption(_) => iei::SERVICE_OPTION,
            Self::Other { iei, .. } => *iei,
        }
    }

    fn value(&self) -> Result<Vec<u8>> {
        Ok(match self {
            Self::LacEncapsulatedPdu(pdu) => pdu.clone(),
            Self::CorrelationId(value) => value.to_be_bytes().to_vec(),
            Self::MobileIdentity(identity) => identity.encode()?,
            Self::MessageTransmissionControl(control) => {
                let mut flags = 0;
                if control.paging_message {
                    flags |= TX_CONTROL_PAGING_MESSAGE;
                }
                if control.simul_xmit_with_next {
                    flags |= TX_CONTROL_SIMUL_XMIT;
                }
                if control.ack_required {
                    flags |= TX_CONTROL_ACK_REQUIRED;
                }
                if control.logical_channel_dedicated {
                    flags |= TX_CONTROL_LOGICAL_CHANNEL;
                }
                vec![flags, control.protocol_revision]
            }
            Self::Cause(cause) => vec![cause.value()],
            Self::Event {
                event,
                additional_info,
            } => {
                let mut out = vec![event.value()];
                out.extend_from_slice(additional_info);
                out
            }
            Self::ServiceOption(service_option) => service_option.to_be_bytes().to_vec(),
            Self::Other { value, .. } => value.clone(),
        })
    }

    fn decode(iei: u8, value: &[u8]) -> Result<Self> {
        let exact = |len: usize, context: &'static str| {
            if value.len() == len {
                Ok(())
            } else {
                Err(A21Error::InvalidLength {
                    context,
                    expected: len,
                    actual: value.len(),
                })
            }
        };
        Ok(match iei {
            iei::LAC_ENCAPSULATED_PDU => Self::LacEncapsulatedPdu(value.to_vec()),
            iei::CORRELATION_ID => {
                exact(CORRELATION_ID_LEN, "Correlation ID")?;
                Self::CorrelationId(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
            }
            iei::MOBILE_IDENTITY => Self::MobileIdentity(MobileIdentity::decode(value)?),
            iei::MESSAGE_TRANSMISSION_CONTROL => {
                exact(TX_CONTROL_LEN, "A21 1x Message Transmission Control")?;
                let flags = value[0];
                Self::MessageTransmissionControl(MessageTransmissionControl {
                    paging_message: flags & TX_CONTROL_PAGING_MESSAGE != 0,
                    simul_xmit_with_next: flags & TX_CONTROL_SIMUL_XMIT != 0,
                    ack_required: flags & TX_CONTROL_ACK_REQUIRED != 0,
                    logical_channel_dedicated: flags & TX_CONTROL_LOGICAL_CHANNEL != 0,
                    protocol_revision: value[1],
                })
            }
            iei::CAUSE => {
                exact(1, "A21 Cause")?;
                Self::Cause(A21Cause::from_u8(value[0]))
            }
            iei::EVENT => {
                let Some((&event, additional_info)) = value.split_first() else {
                    return Err(A21Error::Truncated {
                        context: "A21 Event",
                        needed: 1,
                        actual: 0,
                    });
                };
                Self::Event {
                    event: A21EventKind::from_u8(event),
                    additional_info: additional_info.to_vec(),
                }
            }
            iei::SERVICE_OPTION => {
                exact(SERVICE_OPTION_LEN, "Service Option")?;
                Self::ServiceOption(u16::from_be_bytes([value[0], value[1]]))
            }
            iei => Self::Other {
                iei,
                value: value.to_vec(),
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A21Message {
    pub message_type: A21MessageType,
    pub elements: Vec<A21Element>,
}

impl A21Message {
    pub fn new(message_type: A21MessageType, elements: Vec<A21Element>) -> Self {
        Self {
            message_type,
            elements,
        }
    }

    /// A.S0008-C §5.1.6.1.
    pub fn air_interface_signaling(
        correlation_id: u32,
        identity: MobileIdentity,
        control: MessageTransmissionControl,
        pdus: Vec<Vec<u8>>,
    ) -> Self {
        let mut elements = vec![
            A21Element::CorrelationId(correlation_id),
            A21Element::MobileIdentity(identity),
            A21Element::MessageTransmissionControl(control),
        ];
        elements.extend(pdus.into_iter().map(A21Element::LacEncapsulatedPdu));
        Self::new(A21MessageType::AirInterfaceSignaling, elements)
    }

    /// A21-Ack (§5.1.6.2). `cause` is omitted on success.
    pub fn ack(
        correlation_id: Option<u32>,
        identity: Option<MobileIdentity>,
        cause: Option<A21Cause>,
    ) -> Self {
        let mut elements = Vec::new();
        elements.extend(correlation_id.map(A21Element::CorrelationId));
        elements.extend(identity.map(A21Element::MobileIdentity));
        elements.extend(cause.map(A21Element::Cause));
        Self::new(A21MessageType::Ack, elements)
    }

    /// A21-Event Notification (§5.1.6.4).
    pub fn event_notification(
        correlation_id: u32,
        identity: MobileIdentity,
        event: A21EventKind,
    ) -> Self {
        Self::new(
            A21MessageType::EventNotification,
            vec![
                A21Element::CorrelationId(correlation_id),
                A21Element::MobileIdentity(identity),
                A21Element::Event {
                    event,
                    additional_info: Vec::new(),
                },
            ],
        )
    }

    pub fn correlation_id(&self) -> Option<u32> {
        self.elements.iter().find_map(|element| match element {
            A21Element::CorrelationId(value) => Some(*value),
            _ => None,
        })
    }

    pub fn mobile_identity(&self) -> Option<&MobileIdentity> {
        self.elements.iter().find_map(|element| match element {
            A21Element::MobileIdentity(identity) => Some(identity),
            _ => None,
        })
    }

    pub fn cause(&self) -> Option<A21Cause> {
        self.elements.iter().find_map(|element| match element {
            A21Element::Cause(cause) => Some(*cause),
            _ => None,
        })
    }

    pub fn event(&self) -> Option<A21EventKind> {
        self.elements.iter().find_map(|element| match element {
            A21Element::Event { event, .. } => Some(*event),
            _ => None,
        })
    }

    pub fn transmission_control(&self) -> Option<MessageTransmissionControl> {
        self.elements.iter().find_map(|element| match element {
            A21Element::MessageTransmissionControl(control) => Some(*control),
            _ => None,
        })
    }

    pub fn lac_pdus(&self) -> impl Iterator<Item = &[u8]> {
        self.elements.iter().filter_map(|element| match element {
            A21Element::LacEncapsulatedPdu(pdu) => Some(pdu.as_slice()),
            _ => None,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![self.message_type as u8];
        for element in &self.elements {
            let value = element.value()?;
            let length = u8::try_from(value.len()).map_err(|_| A21Error::InvalidLength {
                context: "A21 element",
                expected: usize::from(u8::MAX),
                actual: value.len(),
            })?;
            out.push(element.iei());
            out.push(length);
            out.extend_from_slice(&value);
        }
        Ok(out)
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&message_type, mut rest)) = input.split_first() else {
            return Err(A21Error::Truncated {
                context: "A21 message",
                needed: 1,
                actual: 0,
            });
        };
        let message_type = A21MessageType::from_u8(message_type)?;
        let mut elements = Vec::new();
        while !rest.is_empty() {
            if rest.len() < 2 {
                return Err(A21Error::Truncated {
                    context: "A21 element header",
                    needed: 2,
                    actual: rest.len(),
                });
            }
            let iei = rest[0];
            let length = usize::from(rest[1]);
            let Some(value) = rest.get(2..2 + length) else {
                return Err(A21Error::Truncated {
                    context: "A21 element value",
                    needed: 2 + length,
                    actual: rest.len(),
                });
            };
            elements.push(A21Element::decode(iei, value)?);
            rest = &rest[2 + length..];
        }
        Ok(Self {
            message_type,
            elements,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_interface_signaling_matches_the_a_s0008_bitmap() {
        let message = A21Message::air_interface_signaling(
            0x0102_0304,
            MobileIdentity::Imsi("310009176936269".into()),
            MessageTransmissionControl {
                paging_message: true,
                protocol_revision: 6,
                ..MessageTransmissionControl::default()
            },
            vec![vec![0x11, 0x22]],
        );
        let wire = message.encode().unwrap();
        assert_eq!(
            wire,
            [
                0x01, // A21-1x Air Interface Signaling
                0x04, 0x04, 0x01, 0x02, 0x03, 0x04, // Correlation ID
                0x05, 0x08, 0x3e, 0x01, 0x00, 0x19, 0x67, 0x39, 0x26, 0x96, // IMSI, odd
                0x07, 0x02, 0x08, 0x06, // Paging Message, ProtocolRevision 6
                0x01, 0x02, 0x11, 0x22, // 1x LAC Encapsulated PDU
            ]
        );
        assert_eq!(A21Message::decode(&wire).unwrap(), message);
    }

    #[test]
    fn even_digit_imsi_ends_with_the_end_mark() {
        let wire = MobileIdentity::Imsi("0010101234".into()).encode().unwrap();
        assert_eq!(wire, [0x06, 0x10, 0x10, 0x10, 0x32, 0xf4]);
        assert_eq!(
            MobileIdentity::decode(&wire).unwrap(),
            MobileIdentity::Imsi("0010101234".into())
        );
    }

    #[test]
    fn esn_and_meid_identities_round_trip() {
        for identity in [
            MobileIdentity::Esn(0x80857e58),
            MobileIdentity::Meid("A0000012345678".into()),
        ] {
            let wire = identity.encode().unwrap();
            assert_eq!(MobileIdentity::decode(&wire).unwrap(), identity);
        }
        assert_eq!(
            MobileIdentity::Esn(0x80857e58).encode().unwrap(),
            [0x05, 0x80, 0x85, 0x7e, 0x58]
        );
    }

    #[test]
    fn ack_and_event_notification_round_trip_with_typed_values() {
        let ack = A21Message::ack(
            Some(7),
            Some(MobileIdentity::Esn(1)),
            Some(A21Cause::TunnelingNotAvailable),
        );
        let wire = ack.encode().unwrap();
        assert_eq!(&wire[..1], &[0x02]);
        let decoded = A21Message::decode(&wire).unwrap();
        assert_eq!(decoded.cause(), Some(A21Cause::TunnelingNotAvailable));
        assert_eq!(decoded.correlation_id(), Some(7));

        let event = A21Message::event_notification(
            9,
            MobileIdentity::Imsi("001010123456789".into()),
            A21EventKind::PresentInHrpd,
        );
        let decoded = A21Message::decode(&event.encode().unwrap()).unwrap();
        assert_eq!(decoded.event(), Some(A21EventKind::PresentInHrpd));
        assert_eq!(decoded, event);
    }

    #[test]
    fn unknown_elements_are_kept_and_truncation_is_rejected() {
        let wire = [0x08, 0x03, 0x02, 0xaa, 0xbb];
        let message = A21Message::decode(&wire).unwrap();
        assert_eq!(message.message_type, A21MessageType::RadioUpdateRequest);
        assert_eq!(
            message.elements,
            vec![A21Element::Other {
                iei: iei::PILOT_LIST,
                value: vec![0xaa, 0xbb]
            }]
        );
        assert_eq!(message.encode().unwrap(), wire);
        assert!(A21Message::decode(&[0x02, 0x04, 0x04, 0x00]).is_err());
        assert!(matches!(
            A21Message::decode(&[0x0f]),
            Err(A21Error::UnknownMessageType(0x0f))
        ));
    }
}
