#[derive(Debug)]
pub struct ReceiveError {
    pub kind: ReceiveErrorKind,
    pub message: Option<String>,
}

impl ReceiveError {
    pub fn kind(&self) -> ReceiveErrorKind {
        self.kind.clone()
    }
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl std::error::Error for ReceiveError {}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ReceiveErrorKind {
    Timeout,
    LateCommand,
    BrokenChain,
    Overflow,
    OutOfSequence,
    Alignment,
    BadPacket,
    Other,
}
