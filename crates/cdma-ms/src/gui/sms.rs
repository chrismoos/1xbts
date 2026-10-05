use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use cdma_common::error::Error;
use cdma_ms::ms::MsEvent;

pub(super) const NOTIFICATION_DURATION: Duration = Duration::from_secs(6);
pub(super) const MAX_TEXT_CHARS: usize = 160;
const SEND_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ADDRESS_CHARS: usize = 32;

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub(super) enum MessageStatus {
    Received,
    Queued,
    BaseStationReceived,
    Accepted,
    Failed(String),
    Unconfirmed,
}

impl MessageStatus {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Received => "Received".into(),
            Self::Queued | Self::BaseStationReceived => "Pending".into(),
            Self::Accepted => "Sent".into(),
            Self::Failed(reason) => format!("Failed: {reason}"),
            Self::Unconfirmed => "No network confirmation".into(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Message {
    pub(super) peer: String,
    pub(super) text: String,
    pub(super) is_outgoing: bool,
    pub(super) timestamp_secs: i64,
    pub(super) status: MessageStatus,
}

#[derive(Default)]
pub(super) struct Mailbox {
    pub(super) messages: Vec<Message>,
    pub(super) notification: Option<(String, Instant)>,
    pub(super) error: Option<String>,
    pub(super) stream_ready: bool,
    path: Option<PathBuf>,
    pending: Option<(usize, Instant)>,
}

pub(super) fn history_path(esn: u32) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".cdma-ms")
            .join(format!("messages-{esn:08x}.json"))
    })
}

pub(super) fn validate(destination: &str, text: &str) -> Result<(), Error> {
    let number = destination.strip_prefix('+').unwrap_or(destination);
    if number.is_empty()
        || destination.len() > MAX_ADDRESS_CHARS
        || !number
            .chars()
            .all(|c| c.is_ascii_digit() || c == '*' || c == '#')
    {
        return Err("Enter a phone number or short code.".into());
    }
    if text.trim().is_empty() {
        return Err("Enter a message.".into());
    }
    if !text.is_ascii() || text.len() > MAX_TEXT_CHARS {
        return Err(format!("Use up to {MAX_TEXT_CHARS} ASCII characters.").into());
    }
    Ok(())
}

impl Mailbox {
    pub(super) fn load(path: Option<PathBuf>) -> Self {
        let mut mailbox = Self::default();
        if let Some(path) = path {
            match std::fs::read(&path) {
                Ok(bytes) => match serde_json::from_slice::<Vec<Message>>(&bytes) {
                    Ok(mut messages) => {
                        for message in &mut messages {
                            if matches!(
                                message.status,
                                MessageStatus::Queued | MessageStatus::BaseStationReceived
                            ) {
                                message.status = MessageStatus::Unconfirmed;
                            }
                        }
                        mailbox.messages = messages;
                        mailbox.path = Some(path);
                    }
                    Err(error) => mailbox.error = Some(format!("Cannot read SMS history: {error}")),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    mailbox.path = Some(path)
                }
                Err(error) => mailbox.error = Some(format!("Cannot read SMS history: {error}")),
            }
        } else {
            mailbox.error = Some("SMS history is temporary because HOME is unavailable.".into());
        }
        mailbox
    }

    pub(super) fn is_sending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn queue(&mut self, peer: String, text: String) -> Result<usize, Error> {
        validate(&peer, &text)?;
        if self.is_sending() {
            return Err("Wait for the current SMS to finish.".into());
        }
        let index = self.messages.len();
        self.messages.push(Message {
            peer,
            text,
            is_outgoing: true,
            timestamp_secs: chrono::Utc::now().timestamp(),
            status: MessageStatus::Queued,
        });
        self.pending = Some((index, Instant::now()));
        self.save();
        Ok(index)
    }

    pub(super) fn request_failed(&mut self, index: usize, reason: String) {
        if self.pending.is_some_and(|(pending, _)| pending == index) {
            self.finish(MessageStatus::Failed(reason));
        }
    }

    fn finish(&mut self, status: MessageStatus) {
        if let Some((index, _)) = self.pending.take() {
            self.messages[index].status = status;
            self.save();
        }
    }

    pub(super) fn expire(&mut self, now: Instant) {
        if self
            .pending
            .is_some_and(|(_, started)| now.duration_since(started) >= SEND_TIMEOUT)
        {
            self.finish(MessageStatus::Unconfirmed);
        }
        if self
            .notification
            .as_ref()
            .is_some_and(|(_, started)| now.duration_since(*started) >= NOTIFICATION_DURATION)
        {
            self.notification = None;
        }
    }

    pub(super) fn on_event(&mut self, event: &MsEvent) {
        match event {
            MsEvent::SmsReceived {
                originating_number,
                text,
            } => {
                self.messages.push(Message {
                    peer: originating_number.clone(),
                    text: text.clone(),
                    is_outgoing: false,
                    timestamp_secs: chrono::Utc::now().timestamp(),
                    status: MessageStatus::Received,
                });
                self.notification =
                    Some((format!("New SMS from {originating_number}"), Instant::now()));
                self.save();
            }
            MsEvent::SmsCauseCode { error_class } => self.finish(if *error_class == 0 {
                MessageStatus::Accepted
            } else {
                MessageStatus::Failed(format!("network error class {error_class}"))
            }),
            MsEvent::TrafficReleased { .. } => self.finish(MessageStatus::Unconfirmed),
            MsEvent::AccessFailed { cause, .. } => {
                self.finish(MessageStatus::Failed(cause.clone()))
            }
            _ => {}
        }
    }

    fn save(&mut self) {
        if let Some(path) = &self.path {
            if let Err(error) = save_messages(path, &self.messages) {
                self.error = Some(format!("Cannot save SMS history: {error}"));
            }
        }
    }
}

fn save_messages(path: &Path, messages: &[Message]) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(messages)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn received_and_sent_messages_share_history_without_false_delivery() {
        let mut mailbox = Mailbox::default();
        mailbox.queue("22222".into(), "hello".into()).unwrap();
        mailbox.on_event(&MsEvent::SmsReceived {
            originating_number: "33333".into(),
            text: "reply".into(),
        });
        mailbox.on_event(&MsEvent::SmsCauseCode { error_class: 0 });
        assert_eq!(mailbox.messages.len(), 2);
        assert_eq!(mailbox.messages[0].status, MessageStatus::Accepted);
        assert_eq!(mailbox.messages[1].status, MessageStatus::Received);
        assert!(mailbox.notification.is_some());
        mailbox.expire(Instant::now() + NOTIFICATION_DURATION);
        assert!(mailbox.notification.is_none());
        mailbox.queue("22222".into(), "again".into()).unwrap();
        mailbox.on_event(&MsEvent::TrafficReleased {
            by: "mobile".into(),
        });
        assert_eq!(mailbox.messages[2].status, MessageStatus::Unconfirmed);
    }

    #[test]
    fn rejects_unsupported_text_and_tracks_failed_and_expired_sends() {
        assert!(validate("", "hello").is_err());
        assert!(validate("123", "é").is_err());
        assert!(validate("123", &"a".repeat(MAX_TEXT_CHARS + 1)).is_err());
        assert!(validate("+123", "hello").is_ok());
        let mut mailbox = Mailbox::default();
        let id = mailbox.queue("123".into(), "hello".into()).unwrap();
        assert!(mailbox.queue("123".into(), "second".into()).is_err());
        mailbox.request_failed(id, "offline".into());
        assert_eq!(
            mailbox.messages[0].status,
            MessageStatus::Failed("offline".into())
        );
        mailbox.queue("123".into(), "second".into()).unwrap();
        mailbox.expire(Instant::now() + SEND_TIMEOUT);
        assert_eq!(mailbox.messages[1].status, MessageStatus::Unconfirmed);
    }

    #[test]
    fn base_station_ack_keeps_sms_pending_until_cause() {
        let mut mailbox = Mailbox::default();
        mailbox.queue("22222".into(), "hello".into()).unwrap();
        assert_eq!(mailbox.messages[0].status.label(), "Pending");
        mailbox.on_event(&MsEvent::TrafficTransmit {
            name: "Data Burst (SMS)".into(),
            msg_seq: 1,
            ack_seq: 7,
            ack_req: true,
            retransmission: false,
        });
        mailbox.on_event(&MsEvent::TrafficMessage {
            name: "ORDM".into(),
            order: Some("Base Station Acknowledgment".into()),
            msg_seq: 0,
            ack_seq: 0,
            ack_req: false,
        });
        assert_eq!(mailbox.messages[0].status, MessageStatus::Queued);
        mailbox.on_event(&MsEvent::TrafficMessage {
            name: "ORDM".into(),
            order: Some("Base Station Acknowledgment".into()),
            msg_seq: 1,
            ack_seq: 1,
            ack_req: false,
        });
        assert_eq!(mailbox.messages[0].status, MessageStatus::Queued);
        assert!(mailbox.is_sending());
        mailbox.on_event(&MsEvent::SmsCauseCode { error_class: 0 });
        assert_eq!(mailbox.messages[0].status.label(), "Sent");
        mailbox.on_event(&MsEvent::TrafficReleased {
            by: "mobile".into(),
        });
        assert_eq!(mailbox.messages[0].status, MessageStatus::Accepted);
        assert!(!mailbox.is_sending());
    }

    #[test]
    fn history_round_trips_and_does_not_resume_a_stale_send() {
        let path = std::env::temp_dir().join(format!("cdma-ms-sms-{}.json", std::process::id()));
        let mut mailbox = Mailbox::load(Some(path.clone()));
        mailbox.queue("123".into(), "saved".into()).unwrap();
        let loaded = Mailbox::load(Some(path.clone()));
        assert_eq!(loaded.messages[0].text, "saved");
        assert_eq!(loaded.messages[0].status, MessageStatus::Unconfirmed);
        assert!(!loaded.is_sending());
        std::fs::remove_file(path).unwrap();
    }
}
