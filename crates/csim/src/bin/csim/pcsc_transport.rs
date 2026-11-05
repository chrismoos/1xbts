//! PC/SC readers as a card transport.

use std::ffi::CString;
use std::thread::sleep;
use std::time::Duration;

use csim::card::{CardError, Transport};
use pcsc::{Context, Disposition, Error, MAX_BUFFER_SIZE_EXTENDED, Protocols, Scope, ShareMode};

/// Connect attempts on an unresponsive card, each after a slot power
/// cycle.
const CONNECT_ATTEMPTS: usize = 4;
const POWER_CYCLE_SETTLE: Duration = Duration::from_secs(1);

/// What happens to the card when the connection closes, from
/// CSIM_DISPOSITION. "leave" keeps it powered and untouched, as gp.jar
/// does.
fn disposition() -> Result<Disposition, CardError> {
    match std::env::var("CSIM_DISPOSITION")
        .unwrap_or_else(|_| "leave".into())
        .to_lowercase()
        .as_str()
    {
        "leave" => Ok(Disposition::LeaveCard),
        "reset" => Ok(Disposition::ResetCard),
        "unpower" => Ok(Disposition::UnpowerCard),
        other => Err(CardError::Invalid(format!(
            "CSIM_DISPOSITION must be leave, reset or unpower, got {other}"
        ))),
    }
}

fn pcsc_error(op: &str, e: Error) -> CardError {
    CardError::Transport(format!("{op}: {e}"))
}

/// A YubiKey is a CCID reader too. When it is the only other reader, the
/// card reader is the one meant.
const TOKEN_READER_WORDS: [&str; 3] = ["yubikey", "yubico", "fido"];

/// The reader whose name contains `wanted`, or the only card reader.
fn pick_reader(ctx: &Context, wanted: Option<&str>) -> Result<CString, CardError> {
    let readers = ctx
        .list_readers_owned()
        .map_err(|e| pcsc_error("listing readers", e))?;
    let names: Vec<String> = readers
        .iter()
        .map(|r| r.to_string_lossy().into_owned())
        .collect();
    if readers.is_empty() {
        return Err(CardError::Invalid("no PC/SC readers found".into()));
    }
    if let Some(wanted) = wanted {
        let wanted = wanted.to_lowercase();
        return readers
            .iter()
            .zip(&names)
            .find(|(_, n)| n.to_lowercase().contains(&wanted))
            .map(|(r, _)| r.clone())
            .ok_or_else(|| {
                CardError::Invalid(format!(
                    "no reader matches {wanted:?}, available: {names:?}"
                ))
            });
    }
    if readers.len() == 1 {
        return Ok(readers[0].clone());
    }
    let cards: Vec<&CString> = readers
        .iter()
        .zip(&names)
        .filter(|(_, n)| {
            !TOKEN_READER_WORDS
                .iter()
                .any(|w| n.to_lowercase().contains(w))
        })
        .map(|(r, _)| r)
        .collect();
    match cards[..] {
        [one] => Ok(one.clone()),
        _ => Err(CardError::Invalid(format!(
            "multiple readers attached, use --reader or CSIM_READER, available: {names:?}"
        ))),
    }
}

/// The full name of the reader `csim` would use for `wanted`, without
/// connecting to the card.
pub fn reader_name(wanted: Option<&str>) -> Result<String, CardError> {
    let ctx = Context::establish(Scope::User).map_err(|e| pcsc_error("PC/SC context", e))?;
    Ok(pick_reader(&ctx, wanted)?.to_string_lossy().into_owned())
}

pub struct PcscTransport {
    ctx: Context,
    reader: CString,
    card: Option<pcsc::Card>,
    atr: Vec<u8>,
    disposition: Disposition,
    /// Whether a command has been answered on this connection. Until one
    /// has, there is no session state a power cycle could lose.
    answered: bool,
}

impl PcscTransport {
    pub fn open(wanted: Option<&str>) -> Result<PcscTransport, CardError> {
        let ctx = Context::establish(Scope::User).map_err(|e| pcsc_error("PC/SC context", e))?;
        let reader = pick_reader(&ctx, wanted)?;
        let mut t = PcscTransport {
            ctx,
            reader,
            card: None,
            atr: Vec::new(),
            disposition: disposition()?,
            answered: false,
        };
        t.connect()?;
        Ok(t)
    }

    /// Connects, power cycling the slot and retrying when the card is
    /// unresponsive.
    fn connect(&mut self) -> Result<(), CardError> {
        for attempt in 1..=CONNECT_ATTEMPTS {
            match self
                .ctx
                .connect(&self.reader, ShareMode::Shared, Protocols::ANY)
            {
                Ok(card) => {
                    self.atr = card
                        .status2_owned()
                        .map(|s| s.atr().to_vec())
                        .unwrap_or_default();
                    self.card = Some(card);
                    return Ok(());
                }
                Err(e @ (Error::UnresponsiveCard | Error::ResetCard | Error::RemovedCard))
                    if attempt < CONNECT_ATTEMPTS =>
                {
                    eprintln!(
                        "card unresponsive ({e}), power cycling the slot (attempt {attempt} of {})",
                        CONNECT_ATTEMPTS - 1
                    );
                    self.power_cycle();
                    sleep(POWER_CYCLE_SETTLE);
                }
                Err(e) => return Err(pcsc_error("connecting to the card", e)),
            }
        }
        Err(CardError::Transport("card did not respond".into()))
    }

    /// Connects in direct mode and disconnects with the card unpowered,
    /// so the next connect powers it on from cold.
    fn power_cycle(&self) {
        let result = self
            .ctx
            .connect(&self.reader, ShareMode::Direct, Protocols::UNDEFINED)
            .and_then(|card| {
                card.disconnect(Disposition::UnpowerCard)
                    .map_err(|(_, e)| e)
            });
        if let Err(e) = result {
            eprintln!("slot power cycle failed: {e}");
        }
    }

    fn close(&mut self) {
        if let Some(card) = self.card.take() {
            // Dropping a pcsc::Card disconnects with a warm reset, which the
            // chosen disposition is there to avoid, so a card whose
            // disconnect failed is leaked instead.
            if let Err((card, _)) = card.disconnect(self.disposition) {
                std::mem::forget(card);
            }
        }
    }
}

impl Transport for PcscTransport {
    /// On macOS a transmit can fail with SCARD_E_NOT_TRANSACTED on a fresh
    /// connection, which reinserting the card clears. The command is
    /// retried after a reconnect that leaves the card untouched, then, on
    /// the first command of the connection only, after a slot power cycle.
    fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, CardError> {
        let first = self.transmit_raw(apdu);
        let Err(Error::NotTransacted) = first else {
            return self.answered(first);
        };
        eprintln!("PC/SC transaction lost, reconnecting");
        self.close();
        self.connect()?;
        let second = self.transmit_raw(apdu);
        if !matches!(second, Err(Error::NotTransacted)) || self.answered {
            return self.answered(second);
        }
        eprintln!("PC/SC transaction lost again, power cycling the slot");
        self.close();
        self.power_cycle();
        sleep(POWER_CYCLE_SETTLE);
        self.connect()?;
        let third = self.transmit_raw(apdu);
        self.answered(third)
    }

    fn atr(&self) -> Vec<u8> {
        self.atr.clone()
    }

    fn name(&self) -> String {
        self.reader.to_string_lossy().into_owned()
    }
}

impl PcscTransport {
    fn transmit_raw(&mut self, apdu: &[u8]) -> Result<Vec<u8>, Error> {
        let card = self.card.as_ref().ok_or(Error::InvalidHandle)?;
        let mut buf = vec![0u8; MAX_BUFFER_SIZE_EXTENDED];
        card.transmit(apdu, &mut buf).map(<[u8]>::to_vec)
    }

    fn answered(&mut self, result: Result<Vec<u8>, Error>) -> Result<Vec<u8>, CardError> {
        let rapdu = result.map_err(|e| match e {
            Error::NotTransacted => CardError::Transport(format!(
                "transmit: {e}. Remove the card from the reader, insert it again and rerun the command."
            )),
            e => pcsc_error("transmit", e),
        })?;
        self.answered = true;
        Ok(rapdu)
    }
}

impl Drop for PcscTransport {
    fn drop(&mut self) {
        self.close();
    }
}
