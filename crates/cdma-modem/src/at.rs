//! AT command-line parsing for the IWF modem emulator (TIA-602 / IS-707-A.3).
//!
//! An AT command line begins with the `AT` prefix and may chain several
//! commands. Only the subset relevant to CDMA async data service is decoded;
//! anything else is surfaced as [`AtCommand::Unknown`] so the caller can apply
//! the "unrecognized command" handling from IS-707-A.3 4.2.

/// A single parsed AT command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtCommand {
    /// `ATD<dial string>` — originate. Carries the raw dial string and the
    /// extracted dialable digits (modifiers removed).
    Dial { dial_string: String, digits: String },
    /// `ATA` — answer.
    Answer,
    /// `ATH` / `ATH0` — hang up.
    Hangup,
    /// `ATO` — return to online (data) state from online-command state.
    ReturnOnline,
    /// `ATE0` / `ATE1` — command echo off/on.
    Echo(bool),
    /// `ATV0` / `ATV1` — numeric/verbose result codes.
    Verbose(bool),
    /// `ATQ0` / `ATQ1` — result codes enabled/suppressed.
    Quiet(bool),
    /// `ATZ` — reset to stored profile.
    Reset,
    /// `AT&F` — restore factory defaults.
    FactoryReset,
    /// `ATS<n>=<v>` — set S-register.
    SetRegister { reg: u8, value: u8 },
    /// `ATS<n>?` — query S-register.
    QueryRegister { reg: u8 },
    /// Extended `AT+NAME=<value>` / `AT+NAME?` / `AT+NAME` command.
    Extended {
        name: String,
        value: Option<String>,
        query: bool,
    },
    /// A syntactically valid but unrecognized command body.
    Unknown(String),
}

/// Dial-string modifier characters that are not part of the dialable number
/// (IS-707-A.3 4.2.6): tone/pulse select, pause, wait-for-dialtone, etc.
const DIAL_MODIFIERS: &[char] = &[
    'T', 'P', 'W', '@', '!', '$', ',', ';', '(', ')', '-', ' ', 'R',
];

/// Parse a full AT command line (without trailing CR) into its commands.
///
/// Returns `None` if the line does not start with the `AT` prefix.
pub fn parse_line(line: &str) -> Option<Vec<AtCommand>> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let body = strip_at_prefix(trimmed)?;
    Some(parse_body(body))
}

fn strip_at_prefix(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    if bytes.len() >= 2
        && bytes[0].eq_ignore_ascii_case(&b'A')
        && bytes[1].eq_ignore_ascii_case(&b'T')
    {
        Some(&line[2..])
    } else {
        None
    }
}

fn parse_body(body: &str) -> Vec<AtCommand> {
    let mut cmds = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c.to_ascii_uppercase() {
            // Dial consumes the remainder of the line.
            'D' => {
                let dial_string: String = chars[i + 1..].iter().collect();
                let digits = extract_digits(&dial_string);
                cmds.push(AtCommand::Dial {
                    dial_string,
                    digits,
                });
                break;
            }
            'A' => {
                cmds.push(AtCommand::Answer);
                i += 1;
            }
            'H' => {
                i += 1;
                i += consume_number(&chars[i..]);
                cmds.push(AtCommand::Hangup);
            }
            'O' => {
                cmds.push(AtCommand::ReturnOnline);
                i += 1;
            }
            'E' => {
                i += 1;
                let (n, adv) = read_number(&chars[i..]);
                i += adv;
                cmds.push(AtCommand::Echo(n != 0));
            }
            'V' => {
                i += 1;
                let (n, adv) = read_number(&chars[i..]);
                i += adv;
                cmds.push(AtCommand::Verbose(n != 0));
            }
            'Q' => {
                i += 1;
                let (n, adv) = read_number(&chars[i..]);
                i += adv;
                cmds.push(AtCommand::Quiet(n != 0));
            }
            'Z' => {
                i += 1;
                i += consume_number(&chars[i..]);
                cmds.push(AtCommand::Reset);
            }
            '&' => {
                // Ampersand commands: only &F is decoded.
                i += 1;
                if i < chars.len() {
                    let next = chars[i].to_ascii_uppercase();
                    i += 1;
                    i += consume_number(&chars[i..]);
                    if next == 'F' {
                        cmds.push(AtCommand::FactoryReset);
                    } else {
                        cmds.push(AtCommand::Unknown(format!("&{next}")));
                    }
                }
            }
            'S' => {
                i += 1;
                let (reg, adv) = read_number(&chars[i..]);
                i += adv;
                if i < chars.len() && chars[i] == '=' {
                    i += 1;
                    let (value, adv2) = read_number(&chars[i..]);
                    i += adv2;
                    cmds.push(AtCommand::SetRegister {
                        reg: reg as u8,
                        value: value as u8,
                    });
                } else if i < chars.len() && chars[i] == '?' {
                    i += 1;
                    cmds.push(AtCommand::QueryRegister { reg: reg as u8 });
                } else {
                    cmds.push(AtCommand::QueryRegister { reg: reg as u8 });
                }
            }
            '+' => {
                // Extended command runs to the next command separator (';') or
                // end of line.
                let rest: String = chars[i + 1..].iter().collect();
                let end = rest.find(';').unwrap_or(rest.len());
                let token = &rest[..end];
                cmds.push(parse_extended(token));
                i += 1 + end;
            }
            ';' | ' ' => {
                i += 1;
            }
            other => {
                cmds.push(AtCommand::Unknown(other.to_string()));
                i += 1;
            }
        }
    }
    cmds
}

fn parse_extended(token: &str) -> AtCommand {
    if let Some((name, value)) = token.split_once('=') {
        AtCommand::Extended {
            name: name.to_ascii_uppercase(),
            value: Some(value.to_string()),
            query: false,
        }
    } else if let Some(name) = token.strip_suffix('?') {
        AtCommand::Extended {
            name: name.to_ascii_uppercase(),
            value: None,
            query: true,
        }
    } else {
        AtCommand::Extended {
            name: token.to_ascii_uppercase(),
            value: None,
            query: false,
        }
    }
}

fn extract_digits(dial_string: &str) -> String {
    dial_string
        .chars()
        .filter(|c| !DIAL_MODIFIERS.contains(&c.to_ascii_uppercase()))
        .collect()
}

/// Read an optional decimal number; returns (value, chars consumed).
fn read_number(chars: &[char]) -> (u32, usize) {
    let mut val: u32 = 0;
    let mut n = 0;
    while n < chars.len() && chars[n].is_ascii_digit() {
        val = val * 10 + (chars[n] as u32 - '0' as u32);
        n += 1;
    }
    (val, n)
}

fn consume_number(chars: &[char]) -> usize {
    read_number(chars).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_at_line_rejected() {
        assert!(parse_line("hello").is_none());
    }

    #[test]
    fn dial_extracts_digits_and_drops_modifiers() {
        let cmds = parse_line("ATDT1-800-555-1212,,W9").unwrap();
        assert_eq!(
            cmds,
            vec![AtCommand::Dial {
                dial_string: "T1-800-555-1212,,W9".to_string(),
                digits: "180055512129".to_string(),
            }]
        );
    }

    #[test]
    fn chained_config_line() {
        let cmds = parse_line("ATE0V1Q0").unwrap();
        assert_eq!(
            cmds,
            vec![
                AtCommand::Echo(false),
                AtCommand::Verbose(true),
                AtCommand::Quiet(false),
            ]
        );
    }

    #[test]
    fn s_register_set_and_query() {
        assert_eq!(
            parse_line("ATS7=60").unwrap(),
            vec![AtCommand::SetRegister { reg: 7, value: 60 }]
        );
        assert_eq!(
            parse_line("ATS0?").unwrap(),
            vec![AtCommand::QueryRegister { reg: 0 }]
        );
    }

    #[test]
    fn extended_commands() {
        assert_eq!(
            parse_line("AT+CRM=0").unwrap(),
            vec![AtCommand::Extended {
                name: "CRM".to_string(),
                value: Some("0".to_string()),
                query: false,
            }]
        );
        assert_eq!(
            parse_line("AT+CFG").unwrap(),
            vec![AtCommand::Extended {
                name: "CFG".to_string(),
                value: None,
                query: false,
            }]
        );
        assert_eq!(
            parse_line("AT+CMIP?").unwrap(),
            vec![AtCommand::Extended {
                name: "CMIP".to_string(),
                value: None,
                query: true,
            }]
        );
    }

    #[test]
    fn hangup_and_answer() {
        assert_eq!(parse_line("ATH0").unwrap(), vec![AtCommand::Hangup]);
        assert_eq!(parse_line("ATA").unwrap(), vec![AtCommand::Answer]);
        assert_eq!(parse_line("ATO").unwrap(), vec![AtCommand::ReturnOnline]);
    }
}
