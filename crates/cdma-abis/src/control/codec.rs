//! Encoder and decoder for Abis control messages.

use super::{AbisMessage, InformationElement, MessageType};
use crate::{Error, Result};

/// Encodes a validated Abis control message.
pub fn encode(message: &AbisMessage) -> Result<Vec<u8>> {
    super::messages::validate_elements(message.message_type, &message.elements)?;
    super::typed::validate_message_semantics(message.message_type, &message.elements)?;
    let mut out = vec![message.message_type.value()];
    for element in &message.elements {
        element.encode(&mut out)?;
    }
    Ok(out)
}

/// Decodes an Abis control message from `message_type | information_elements`.
pub fn decode(input: &[u8]) -> Result<AbisMessage> {
    let Some((&message_type, rest)) = input.split_first() else {
        return Err(Error::EmptyMessage);
    };
    let message_type = MessageType::from_u8(message_type)?;
    let mut elements = Vec::new();
    let mut offset = 0usize;
    while offset < rest.len() {
        let (element, used) = decode_element_for_message(message_type, &elements, &rest[offset..])?;
        elements.push(element);
        offset += used;
    }
    AbisMessage::new(message_type, elements)
}

fn decode_element_for_message(
    message_type: MessageType,
    seen: &[InformationElement],
    input: &[u8],
) -> Result<(InformationElement, usize)> {
    if input.is_empty() {
        return Err(Error::Truncated {
            context: "Abis information element header",
            needed: 1,
            actual: input.len(),
        });
    }
    let id = super::ies::ElementId::classify_for_message(message_type, seen, input[0])?;
    match id.framing() {
        super::ies::ElementFraming::Fixed { payload_len } => {
            let end = 1 + payload_len;
            if input.len() < end {
                return Err(Error::Truncated {
                    context: "Abis information element value",
                    needed: end,
                    actual: input.len(),
                });
            }
            Ok((
                InformationElement {
                    id,
                    value: input[1..end].to_vec(),
                },
                end,
            ))
        }
        super::ies::ElementFraming::Tlv => {
            if input.len() < 2 {
                return Err(Error::Truncated {
                    context: "Abis information element header",
                    needed: 2,
                    actual: input.len(),
                });
            }
            let len = input[1] as usize;
            let end = 2 + len;
            if input.len() < end {
                return Err(Error::Truncated {
                    context: "Abis information element value",
                    needed: end,
                    actual: input.len(),
                });
            }
            Ok((
                InformationElement {
                    id,
                    value: input[2..end].to_vec(),
                },
                end,
            ))
        }
    }
}
