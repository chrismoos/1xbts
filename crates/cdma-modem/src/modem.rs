//! The IWF-side emulated modem: command/online state machine driving dial,
//! connect, and hang-up for the CDMA async data service (IS-707-A.3/4).
//!
//! This layer works in terms of logical replies and events, not wire bytes:
//! the [`crate::server::ModemServer`] wraps it with the TIA-617 control-channel
//! codec and the transport. AT command bytes are fed after de-framing; the
//! `+++` return-to-online-command escape arrives as an explicit
//! [`ModemIwf::enter_online_command`] call because the MT2 (not the IWF)
//! detects `+++` and signals it via a Cellular Escape construct.

use crate::at::{self, AtCommand};
use crate::result::ResultCode;

/// Number of S-registers tracked (S0..S255).
const S_REGISTER_COUNT: usize = 256;

/// Modem application state (TIA-602).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModemState {
    /// Processing AT commands; no call up.
    Command,
    /// `ATD`/`ATA` issued; waiting for the data path/carrier before `CONNECT`.
    Dialing,
    /// Call up; user data passes transparently to the data path.
    Online,
    /// `+++` escape while a call is up; AT commands processed, carrier held.
    OnlineCommand,
}

/// A logical reply the IWF returns to the mobile (wrapped in TIA-617 by the
/// server layer, or rendered raw for a bench transport).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// A modem result code (e.g. OK, CONNECT, NO CARRIER).
    Result(ResultCode),
    /// An information-text line (e.g. an S-register value, `+CRM: 0`).
    Info(String),
}

/// A lifecycle event the integration layer must act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModemEvent {
    /// A reply destined for the mobile.
    Reply(Reply),
    /// Mobile issued `ATD`; bring up the end-user data path to this number.
    Dial { digits: String, dial_string: String },
    /// Mobile issued `ATA` (answer an inbound call).
    Answer,
    /// Online user data to forward to the connected data path (NAS/PPP).
    UserData(Vec<u8>),
    /// Mobile hung up (`ATH`); tear down the data path.
    Hangup,
}

/// IWF modem emulator.
pub struct ModemIwf {
    state: ModemState,
    echo: bool,
    verbose: bool,
    quiet: bool,
    /// `AT+CFG` has been received; subsequent AT commands are reflected.
    config_done: bool,
    /// `AT+CRM` Rm-interface protocol select (0 = async/fax, default).
    crm: u8,
    /// Assigned MS IP (for `AT+CMIP?`), set by the integration layer.
    ms_ip: Option<String>,
    /// IWF/base IP (for `AT+CBIP?`), set by the integration layer.
    iwf_ip: Option<String>,
    s_registers: [u8; S_REGISTER_COUNT],
    /// Assembling an AT command line (command state).
    line_buf: Vec<u8>,
}

impl Default for ModemIwf {
    fn default() -> Self {
        Self::new()
    }
}

impl ModemIwf {
    pub fn new() -> Self {
        let mut m = Self {
            state: ModemState::Command,
            echo: true,
            verbose: true,
            quiet: false,
            config_done: false,
            crm: 0,
            ms_ip: None,
            iwf_ip: None,
            s_registers: [0u8; S_REGISTER_COUNT],
            line_buf: Vec::new(),
        };
        m.load_defaults();
        m
    }

    pub fn state(&self) -> ModemState {
        self.state
    }

    pub fn config_done(&self) -> bool {
        self.config_done
    }

    pub fn verbose(&self) -> bool {
        self.verbose
    }

    pub fn quiet(&self) -> bool {
        self.quiet
    }

    pub fn echo(&self) -> bool {
        self.echo
    }

    /// Provide the negotiated IP addresses so `+CMIP?`/`+CBIP?` can answer.
    pub fn set_addresses(&mut self, ms_ip: Option<String>, iwf_ip: Option<String>) {
        self.ms_ip = ms_ip;
        self.iwf_ip = iwf_ip;
    }

    fn load_defaults(&mut self) {
        self.echo = true;
        self.verbose = true;
        self.quiet = false;
        self.s_registers = [0u8; S_REGISTER_COUNT];
        // Hayes / IS-707-A.3 Table 7.1.2-1 defaults for observed registers.
        self.s_registers[3] = 13; // CR char
        self.s_registers[4] = 10; // LF char
        self.s_registers[5] = 8; // backspace
        self.s_registers[6] = 2; // pause before blind dial (s)
        self.s_registers[7] = 50; // wait for carrier (s)
        self.s_registers[8] = 2; // comma pause (s)
        self.s_registers[9] = 6; // carrier-detect threshold (0.1 s)
        self.s_registers[10] = 14; // carrier-loss disconnect (0.1 s)
        self.s_registers[11] = 95; // DTMF timing (ms)
    }

    /// TCP/380 reached ESTABLISHED: reset to defaults, command state.
    pub fn on_tcp_established(&mut self) {
        self.load_defaults();
        self.config_done = false;
        self.state = ModemState::Command;
        self.line_buf.clear();
    }

    /// The data path/carrier is up; report `CONNECT <rate>` and go online.
    pub fn on_carrier_up(&mut self, rate_bps: u32) -> Vec<ModemEvent> {
        self.state = ModemState::Online;
        vec![ModemEvent::Reply(Reply::Result(ResultCode::Connect(Some(
            rate_bps,
        ))))]
    }

    /// The carrier/data path dropped; report `NO CARRIER` and return to command.
    pub fn on_carrier_lost(&mut self) -> Vec<ModemEvent> {
        self.state = ModemState::Command;
        self.line_buf.clear();
        vec![ModemEvent::Reply(Reply::Result(ResultCode::NoCarrier))]
    }

    /// The mobile signalled the `+++` return-to-online-command escape (via a
    /// TIA-617 Cellular Escape construct). Enter online-command state.
    pub fn enter_online_command(&mut self) -> Vec<ModemEvent> {
        if self.state == ModemState::Online {
            self.state = ModemState::OnlineCommand;
            self.line_buf.clear();
            vec![ModemEvent::Reply(Reply::Result(ResultCode::Ok))]
        } else {
            Vec::new()
        }
    }

    /// Feed transparent octets received from the mobile. In command /
    /// online-command state these are AT command bytes; online they are user
    /// data forwarded to the data path.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ModemEvent> {
        if self.state == ModemState::Online {
            return vec![ModemEvent::UserData(bytes.to_vec())];
        }
        let mut events = Vec::new();
        for &b in bytes {
            if b == b'\r' {
                let line = String::from_utf8_lossy(&self.line_buf).to_string();
                self.line_buf.clear();
                self.process_line(&line, &mut events);
            } else if b != b'\n' {
                self.line_buf.push(b);
            }
        }
        events
    }

    fn process_line(&mut self, line: &str, events: &mut Vec<ModemEvent>) {
        let Some(cmds) = at::parse_line(line) else {
            if !line.trim().is_empty() {
                events.push(ModemEvent::Reply(Reply::Result(ResultCode::Error)));
            }
            return;
        };
        let mut ok = true;
        let mut suppress_final = false;
        for cmd in cmds {
            match cmd {
                AtCommand::Dial {
                    digits,
                    dial_string,
                } => {
                    self.state = ModemState::Dialing;
                    events.push(ModemEvent::Dial {
                        digits,
                        dial_string,
                    });
                    suppress_final = true;
                }
                AtCommand::Answer => {
                    self.state = ModemState::Dialing;
                    events.push(ModemEvent::Answer);
                    suppress_final = true;
                }
                AtCommand::Hangup => {
                    let was_up =
                        matches!(self.state, ModemState::Online | ModemState::OnlineCommand);
                    self.state = ModemState::Command;
                    if was_up {
                        events.push(ModemEvent::Hangup);
                    }
                }
                AtCommand::ReturnOnline => {
                    if self.state == ModemState::OnlineCommand {
                        self.state = ModemState::Online;
                        suppress_final = true;
                    } else {
                        ok = false;
                    }
                }
                AtCommand::Echo(v) => self.echo = v,
                AtCommand::Verbose(v) => self.verbose = v,
                AtCommand::Quiet(v) => self.quiet = v,
                AtCommand::Reset | AtCommand::FactoryReset => self.load_defaults(),
                AtCommand::SetRegister { reg, value } => {
                    self.s_registers[reg as usize] = value;
                }
                AtCommand::QueryRegister { reg } => {
                    let v = self.s_registers[reg as usize];
                    events.push(ModemEvent::Reply(Reply::Info(format!("{v:03}"))));
                }
                AtCommand::Extended { name, value, query } => {
                    ok &= self.process_extended(&name, value, query, events);
                }
                AtCommand::Unknown(_) => ok = false,
            }
        }
        if !suppress_final {
            let code = if ok {
                ResultCode::Ok
            } else {
                ResultCode::Error
            };
            events.push(ModemEvent::Reply(Reply::Result(code)));
        }
    }

    /// Process an `AT+NAME` extension. Returns false to signal ERROR.
    fn process_extended(
        &mut self,
        name: &str,
        value: Option<String>,
        query: bool,
        events: &mut Vec<ModemEvent>,
    ) -> bool {
        match name {
            "CFG" => {
                self.config_done = true;
                true
            }
            "CRM" => {
                if let Some(v) = value {
                    match v.trim().parse::<u8>() {
                        Ok(n) => {
                            self.crm = n;
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    if query {
                        events.push(ModemEvent::Reply(Reply::Info(format!(
                            "+CRM: {}",
                            self.crm
                        ))));
                    }
                    true
                }
            }
            "CMIP" => {
                if let Some(ip) = &self.ms_ip {
                    events.push(ModemEvent::Reply(Reply::Info(ip.clone())));
                }
                true
            }
            "CBIP" => {
                if let Some(ip) = &self.iwf_ip {
                    events.push(ModemEvent::Reply(Reply::Info(ip.clone())));
                }
                true
            }
            // Accept the remaining defined cellular extensions as no-ops.
            "CXT" | "CAD" | "CQD" | "CTA" | "CDR" | "CDS" | "CBC" | "CRC" | "CSS" | "CSQ" => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replies(events: &[ModemEvent]) -> Vec<Reply> {
        events
            .iter()
            .filter_map(|e| match e {
                ModemEvent::Reply(r) => Some(r.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn config_upload_then_dial_then_connect() {
        let mut m = ModemIwf::new();
        m.on_tcp_established();

        assert_eq!(
            replies(&m.feed(b"AT+CRM=0\r")),
            vec![Reply::Result(ResultCode::Ok)]
        );
        assert_eq!(m.crm, 0);
        assert_eq!(
            replies(&m.feed(b"AT+CFG\r")),
            vec![Reply::Result(ResultCode::Ok)]
        );
        assert!(m.config_done());

        let ev = m.feed(b"ATDT18005551212\r");
        assert!(ev.iter().any(|e| matches!(
            e,
            ModemEvent::Dial { digits, .. } if digits == "18005551212"
        )));
        assert!(replies(&ev).is_empty());
        assert_eq!(m.state(), ModemState::Dialing);

        assert_eq!(
            replies(&m.on_carrier_up(14400)),
            vec![Reply::Result(ResultCode::Connect(Some(14400)))]
        );
        assert_eq!(m.state(), ModemState::Online);
    }

    #[test]
    fn online_forwards_user_data() {
        let mut m = ModemIwf::new();
        m.on_tcp_established();
        m.feed(b"ATD1\r");
        m.on_carrier_up(14400);
        assert_eq!(
            m.feed(b"hello"),
            vec![ModemEvent::UserData(b"hello".to_vec())]
        );
    }

    #[test]
    fn escape_then_return_online_and_hangup() {
        let mut m = ModemIwf::new();
        m.on_tcp_established();
        m.feed(b"ATD1\r");
        m.on_carrier_up(9600);

        // Cellular Escape → online-command, OK.
        assert_eq!(
            replies(&m.enter_online_command()),
            vec![Reply::Result(ResultCode::Ok)]
        );
        assert_eq!(m.state(), ModemState::OnlineCommand);

        // ATO returns online, no final code.
        let ev = m.feed(b"ATO\r");
        assert!(replies(&ev).is_empty());
        assert_eq!(m.state(), ModemState::Online);

        // Escape again then ATH hangs up.
        m.enter_online_command();
        let ev = m.feed(b"ATH\r");
        assert!(ev.contains(&ModemEvent::Hangup));
        assert_eq!(m.state(), ModemState::Command);
    }

    #[test]
    fn cmip_reports_assigned_address() {
        let mut m = ModemIwf::new();
        m.on_tcp_established();
        m.set_addresses(Some("10.55.0.2".to_string()), Some("10.55.0.1".to_string()));
        let ev = m.feed(b"AT+CMIP?\r");
        assert_eq!(
            replies(&ev),
            vec![
                Reply::Info("10.55.0.2".to_string()),
                Reply::Result(ResultCode::Ok),
            ]
        );
    }

    #[test]
    fn s_register_roundtrip() {
        let mut m = ModemIwf::new();
        m.on_tcp_established();
        assert_eq!(
            replies(&m.feed(b"ATS7=60\r")),
            vec![Reply::Result(ResultCode::Ok)]
        );
        assert_eq!(
            replies(&m.feed(b"ATS7?\r")),
            vec![
                Reply::Info("060".to_string()),
                Reply::Result(ResultCode::Ok)
            ]
        );
    }
}
