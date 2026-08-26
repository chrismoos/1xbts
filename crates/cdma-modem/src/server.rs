//! `ModemServer` — the TCP/380 "modem server" endpoint logic: composes the
//! TIA-617 control-channel codec with the [`ModemIwf`] state machine.
//!
//! It consumes the transparent octet stream of the modem-server TCP connection
//! and produces [`ServerEvent`]s: bytes to send back to the mobile (already
//! TIA-617 framed), the dial request, user data for the connected data path,
//! and hang-up. The integration layer owns the TCP connection and the data
//! path (NAS/PPP); this type owns the modem semantics.

use crate::modem::{ModemEvent, ModemIwf, ModemState, Reply};
use crate::tia617::{
    self, Decoder, EXTEND_IWF0, EXTEND_IWF1, EXTEND_MS1, TYPE_B, TYPE_C, TYPE_STATUS,
};

/// An action the integration layer performs after feeding the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// TIA-617-framed bytes to transmit to the mobile over TCP/380.
    ToMobile(Vec<u8>),
    /// The mobile dialed; bring up the end-user data path to this number.
    Dial { digits: String, dial_string: String },
    /// The mobile answered an inbound call.
    Answer,
    /// User data to forward to the connected data path (NAS/PPP).
    UserData(Vec<u8>),
    /// The mobile hung up; tear down the data path.
    Hangup,
}

/// IWF modem-server endpoint.
pub struct ModemServer {
    modem: ModemIwf,
    decoder: Decoder,
}

impl Default for ModemServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ModemServer {
    pub fn new() -> Self {
        Self {
            modem: ModemIwf::new(),
            decoder: Decoder::new(),
        }
    }

    pub fn state(&self) -> ModemState {
        self.modem.state()
    }

    /// Supply the negotiated IPs so `+CMIP?`/`+CBIP?` answer correctly.
    pub fn set_addresses(&mut self, ms_ip: Option<String>, iwf_ip: Option<String>) {
        self.modem.set_addresses(ms_ip, iwf_ip);
    }

    /// TCP/380 reached ESTABLISHED.
    pub fn on_tcp_established(&mut self) {
        self.modem.on_tcp_established();
        self.decoder = Decoder::new();
    }

    /// The end-user data path connected; emit `CONNECT` and go online.
    pub fn on_carrier_up(&mut self, rate_bps: u32) -> Vec<ServerEvent> {
        let events = self.modem.on_carrier_up(rate_bps);
        self.translate(events)
    }

    /// The data path dropped; emit `NO CARRIER` and return to command state.
    pub fn on_carrier_lost(&mut self) -> Vec<ServerEvent> {
        let events = self.modem.on_carrier_lost();
        self.translate(events)
    }

    /// Feed transparent octets received from the mobile over TCP/380.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ServerEvent> {
        let items = self.decoder.feed(bytes);
        let mut modem_events = Vec::new();
        for item in items {
            match item {
                tia617::Item::Raw(data) => {
                    modem_events.extend(self.modem.feed(&data));
                }
                tia617::Item::Construct {
                    extend,
                    type_byte,
                    string,
                } => {
                    // The only MS→IWF construct that changes IWF state is the
                    // Cellular Escape (return to online-command). Other MS→IWF
                    // constructs report the mobile's local command handling and
                    // need no IWF action for basic operation.
                    if extend == EXTEND_MS1 && type_byte == TYPE_B && string.is_empty() {
                        modem_events.extend(self.modem.enter_online_command());
                    }
                }
            }
        }
        self.translate(modem_events)
    }

    /// Convert modem events into server events, framing replies with TIA-617.
    fn translate(&self, events: Vec<ModemEvent>) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        for ev in events {
            match ev {
                ModemEvent::Reply(reply) => {
                    if let Some(bytes) = self.render_reply(&reply) {
                        out.push(ServerEvent::ToMobile(bytes));
                    }
                }
                ModemEvent::Dial {
                    digits,
                    dial_string,
                } => out.push(ServerEvent::Dial {
                    digits,
                    dial_string,
                }),
                ModemEvent::Answer => out.push(ServerEvent::Answer),
                ModemEvent::UserData(d) => out.push(ServerEvent::UserData(d)),
                ModemEvent::Hangup => out.push(ServerEvent::Hangup),
            }
        }
        out
    }

    /// Frame a reply as TIA-617 constructs (IWF→MS). Result codes use the
    /// STATUS-report construct; information text uses the response construct.
    /// Returns None when result codes are suppressed (`ATQ1`).
    fn render_reply(&self, reply: &Reply) -> Option<Vec<u8>> {
        match reply {
            Reply::Result(rc) => {
                if self.modem.quiet() {
                    return None;
                }
                let text = if self.modem.verbose() {
                    rc.verbose_text().into_bytes()
                } else {
                    vec![rc.numeric() + b'0']
                };
                Some(tia617::encode_message(EXTEND_IWF0, TYPE_STATUS, &text))
            }
            Reply::Info(text) => Some(tia617::encode_message(EXTEND_IWF1, TYPE_C, text.as_bytes())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::ResultCode;
    use crate::tia617::{Decoder as D, Item};

    /// Decode the 617 constructs the server sent to the mobile and return the
    /// (type_byte, string) pairs.
    fn decode_to_mobile(events: &[ServerEvent]) -> Vec<(u8, Vec<u8>)> {
        let mut d = D::new();
        let mut out = Vec::new();
        for e in events {
            if let ServerEvent::ToMobile(bytes) = e {
                for item in d.feed(bytes) {
                    if let Item::Construct {
                        type_byte, string, ..
                    } = item
                    {
                        out.push((type_byte, string));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn full_dial_flow_frames_replies_in_617() {
        let mut s = ModemServer::new();
        s.on_tcp_established();

        // Config commands come back as OK STATUS constructs.
        let ev = s.feed(b"AT+CRM=0\r");
        assert_eq!(decode_to_mobile(&ev), vec![(TYPE_STATUS, b"OK".to_vec())]);
        let ev = s.feed(b"AT+CFG\r");
        assert_eq!(decode_to_mobile(&ev), vec![(TYPE_STATUS, b"OK".to_vec())]);

        // Dial surfaces a Dial event and no immediate reply.
        let ev = s.feed(b"ATDT5551212\r");
        assert!(ev.iter().any(|e| matches!(
            e,
            ServerEvent::Dial { digits, .. } if digits == "5551212"
        )));
        assert!(decode_to_mobile(&ev).is_empty());
        assert_eq!(s.state(), ModemState::Dialing);

        // Data path up → CONNECT construct, online.
        let ev = s.on_carrier_up(14400);
        assert_eq!(
            decode_to_mobile(&ev),
            vec![(TYPE_STATUS, b"CONNECT 14400".to_vec())]
        );
        assert_eq!(s.state(), ModemState::Online);
    }

    #[test]
    fn online_user_data_is_forwarded() {
        let mut s = ModemServer::new();
        s.on_tcp_established();
        s.feed(b"ATD1\r");
        s.on_carrier_up(9600);
        let ev = s.feed(b"\x7e\xff\x03payload"); // PPP-ish bytes
        assert_eq!(
            ev,
            vec![ServerEvent::UserData(b"\x7e\xff\x03payload".to_vec())]
        );
    }

    #[test]
    fn cellular_escape_construct_enters_online_command() {
        let mut s = ModemServer::new();
        s.on_tcp_established();
        s.feed(b"ATD1\r");
        s.on_carrier_up(9600);
        // MS sends the Cellular Escape construct (0x19 0x41 0x20 0x42).
        let escape = tia617::encode_construct(EXTEND_MS1, TYPE_B, b"");
        let ev = s.feed(&escape);
        assert_eq!(decode_to_mobile(&ev), vec![(TYPE_STATUS, b"OK".to_vec())]);
        assert_eq!(s.state(), ModemState::OnlineCommand);
    }

    #[test]
    fn no_carrier_on_data_path_loss() {
        let mut s = ModemServer::new();
        s.on_tcp_established();
        s.feed(b"ATD1\r");
        s.on_carrier_up(9600);
        let ev = s.on_carrier_lost();
        assert_eq!(
            decode_to_mobile(&ev),
            vec![(TYPE_STATUS, b"NO CARRIER".to_vec())]
        );
        assert_eq!(s.state(), ModemState::Command);
    }

    #[test]
    fn numeric_result_codes_when_v0() {
        let mut s = ModemServer::new();
        s.on_tcp_established();
        let ev = s.feed(b"ATV0\r");
        // V0 itself replies with numeric OK = '0'.
        assert_eq!(decode_to_mobile(&ev), vec![(TYPE_STATUS, vec![b'0'])]);
        let _ = ResultCode::Ok; // keep import used
    }
}
