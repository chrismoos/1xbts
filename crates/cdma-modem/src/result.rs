//! Modem result codes (Hayes / TIA-602, IS-707-A.3 Table 4.2.6-1).
//!
//! The IWF returns these to the mobile after processing an AT command line.
//! Verbose form is `<CR><LF>TEXT<CR><LF>`; numeric form is `<digit><CR>`
//! (selected by `ATV0`/`ATV1`, default verbose). `ATQ1` suppresses result
//! codes entirely.

/// Carriage return / line feed used to frame verbose result codes and text.
pub const CR: u8 = b'\r';
pub const LF: u8 = b'\n';

/// A modem result code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultCode {
    Ok,
    /// CONNECT with an optional line rate (bps). `None` renders bare `CONNECT`.
    Connect(Option<u32>),
    Ring,
    NoCarrier,
    Error,
    NoDialtone,
    Busy,
    NoAnswer,
}

impl ResultCode {
    /// Numeric code (TIA-602 standard assignments).
    pub fn numeric(&self) -> u8 {
        match self {
            ResultCode::Ok => 0,
            ResultCode::Connect(_) => 1,
            ResultCode::Ring => 2,
            ResultCode::NoCarrier => 3,
            ResultCode::Error => 4,
            ResultCode::NoDialtone => 6,
            ResultCode::Busy => 7,
            ResultCode::NoAnswer => 8,
        }
    }

    /// Verbose text, without framing CR/LF.
    pub fn verbose_text(&self) -> String {
        match self {
            ResultCode::Ok => "OK".to_string(),
            ResultCode::Connect(None) => "CONNECT".to_string(),
            ResultCode::Connect(Some(rate)) => format!("CONNECT {rate}"),
            ResultCode::Ring => "RING".to_string(),
            ResultCode::NoCarrier => "NO CARRIER".to_string(),
            ResultCode::Error => "ERROR".to_string(),
            ResultCode::NoDialtone => "NO DIALTONE".to_string(),
            ResultCode::Busy => "BUSY".to_string(),
            ResultCode::NoAnswer => "NO ANSWER".to_string(),
        }
    }

    /// Render the result code for transmission given the current V (verbose)
    /// setting. Returns empty when quiet.
    pub fn render(&self, verbose: bool, quiet: bool) -> Vec<u8> {
        if quiet {
            return Vec::new();
        }
        if verbose {
            let mut out = vec![CR, LF];
            out.extend_from_slice(self.verbose_text().as_bytes());
            out.push(CR);
            out.push(LF);
            out
        } else {
            // Numeric form: for CONNECT with a rate, TIA-602 still uses a
            // single digit; the rate is only conveyed in verbose text.
            vec![self.numeric() + b'0', CR]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_framing() {
        assert_eq!(ResultCode::Ok.render(true, false), b"\r\nOK\r\n");
        assert_eq!(
            ResultCode::Connect(Some(14400)).render(true, false),
            b"\r\nCONNECT 14400\r\n"
        );
        assert_eq!(
            ResultCode::NoCarrier.render(true, false),
            b"\r\nNO CARRIER\r\n"
        );
    }

    #[test]
    fn numeric_framing() {
        assert_eq!(ResultCode::Ok.render(false, false), b"0\r");
        assert_eq!(ResultCode::Connect(None).render(false, false), b"1\r");
        assert_eq!(ResultCode::Busy.render(false, false), b"7\r");
    }

    #[test]
    fn quiet_suppresses() {
        assert!(ResultCode::Ok.render(true, true).is_empty());
    }
}
