//! Wire-format A1 message type shared between BSC and MSC.

use std::fmt::{Display, Formatter};

use crate::{Message, MessageType};

/// Wire-format A1 payload crossing the BSC/MSC boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedA1Message {
    message_type: MessageType,
    bytes: Vec<u8>,
    call_id: Option<u64>,
}

impl EncodedA1Message {
    /// Builds an encoded A1 message from a typed envelope.
    pub fn from_message(message: &Message) -> Self {
        Self::from_message_for_call(message, None)
    }

    /// Builds an encoded A1 message from a typed envelope plus optional
    /// transport-level call correlation metadata.
    pub fn from_message_for_call(message: &Message, call_id: Option<u64>) -> Self {
        Self {
            message_type: message.message_type,
            bytes: crate::encode(message),
            call_id,
        }
    }

    /// Validates and wraps an already-encoded A1 payload.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, A1TransportError> {
        let message = crate::decode(&bytes).map_err(A1TransportError::Codec)?;
        Ok(Self {
            message_type: message.message_type,
            bytes,
            call_id: None,
        })
    }

    /// Validates and wraps an already-encoded A1 payload with call correlation.
    pub fn from_bytes_with_call_id(
        bytes: Vec<u8>,
        call_id: Option<u64>,
    ) -> Result<Self, A1TransportError> {
        let message = crate::decode(&bytes).map_err(A1TransportError::Codec)?;
        Ok(Self {
            message_type: message.message_type,
            bytes,
            call_id,
        })
    }

    /// Returns the decoded A1 message type.
    pub fn message_type(&self) -> MessageType {
        self.message_type
    }

    /// Returns the encoded wire bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the transport-level MSC call correlation identifier, if present.
    pub fn call_id(&self) -> Option<u64> {
        self.call_id
    }

    /// Decodes the wrapped wire bytes into an A1 envelope.
    pub fn decode(&self) -> Result<Message, A1TransportError> {
        crate::decode(&self.bytes).map_err(A1TransportError::Codec)
    }
}

/// Errors returned by A1 transport operations.
#[derive(Debug)]
pub enum A1TransportError {
    /// The A1 payload was malformed.
    Codec(crate::Error),
    /// The transport link was closed.
    Closed,
    /// An I/O error occurred on the transport.
    Io(std::io::Error),
}

impl Display for A1TransportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codec(e) => write!(f, "A1 codec error: {e}"),
            Self::Closed => f.write_str("A1 transport closed"),
            Self::Io(e) => write!(f, "A1 transport I/O error: {e}"),
        }
    }
}

impl std::error::Error for A1TransportError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectMessage, Message, MessageType};

    #[test]
    fn encoded_message_validates_wire_bytes() {
        let raw = crate::encode(&Message::new(
            MessageType::Connect,
            ConnectMessage.encode().unwrap(),
        ));
        let message = EncodedA1Message::from_bytes(raw).unwrap();
        assert_eq!(message.message_type(), MessageType::Connect);
        assert_eq!(message.call_id(), None);
    }

    #[test]
    fn encoded_message_preserves_call_correlation() {
        let message = EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::Connect, ConnectMessage.encode().unwrap()),
            Some(42),
        );
        assert_eq!(message.call_id(), Some(42));
    }
}
