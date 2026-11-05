//! UICC and CSIM commands over a card transport.

use crate::{CSIM_AID_PREFIX, CSIM_INSTANCE_AID, Fid, hex_spaced};

/// Sends one APDU and returns the response data followed by SW1 SW2.
pub trait Transport {
    fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, CardError>;
    fn atr(&self) -> Vec<u8>;
    fn name(&self) -> String;
}

#[derive(Debug, thiserror::Error)]
pub enum CardError {
    #[error("{op}: SW={sw:04X}")]
    Status { op: String, sw: u16 },
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Invalid(String),
}

impl CardError {
    /// The status word that refused the command, when there was one.
    pub fn sw(&self) -> Option<u16> {
        match self {
            CardError::Status { sw, .. } => Some(*sw),
            _ => None,
        }
    }
}

fn status(op: impl Into<String>, sw: u16) -> CardError {
    CardError::Status { op: op.into(), sw }
}

// CLA (TS 102 221 §10.1.1, C.S0065-B §9.4).
pub const CLA_UICC: u8 = 0x00;
pub const CLA_CSIM: u8 = 0x80;

// Interindustry INS (TS 102 221 §10.1.2).
pub const INS_SELECT: u8 = 0xA4;
pub const INS_STATUS: u8 = 0xF2;
pub const INS_READ_BINARY: u8 = 0xB0;
pub const INS_UPDATE_BINARY: u8 = 0xD6;
pub const INS_READ_RECORD: u8 = 0xB2;
pub const INS_UPDATE_RECORD: u8 = 0xDC;
pub const INS_VERIFY: u8 = 0x20;
pub const INS_CHANGE_PIN: u8 = 0x24;
pub const INS_DISABLE_PIN: u8 = 0x26;
pub const INS_ENABLE_PIN: u8 = 0x28;
pub const INS_UNBLOCK_PIN: u8 = 0x2C;
pub const INS_GET_RESPONSE: u8 = 0xC0;

// The applet's own CLA 8X commands (Const.java).
pub const INS_LOAD_IPSEC: u8 = 0xF0;
pub const INS_PREPERSO_UPDATE: u8 = 0xF4;
pub const INS_LOAD_CAVE: u8 = 0xF6;
pub const INS_AKEY_CHECKSUM: u8 = 0xF8;
pub const INS_INIT_META: u8 = 0xFA;
pub const INS_DEBUG_DUMP_META: u8 = 0xFB;
pub const INS_DEBUG_DUMP_APDULOG: u8 = 0xFC;
pub const INS_DEBUG_CLEAR_APDULOG: u8 = 0xFD;
pub const INS_DEBUG_PLATFORM_INFO: u8 = 0xFE;
pub const INS_LOAD_AKA_KEY: u8 = 0xEA;
pub const INS_SET_PIN: u8 = 0xEC;
const SET_PIN_P1_PIN: u8 = 0x00;
const SET_PIN_P1_PUK: u8 = 0x01;

const CAVE_LOAD_AKEY: u8 = 0x00;
const CAVE_LOAD_SSD: u8 = 0x01;
const A_KEY_LEN: usize = 8;
const SSD_LEN: usize = 16;
const AKA_KEY_LEN: usize = 16;

// SELECT P1 and P2 (TS 102 221 §11.1.1.2). P2 00 is the ISO 7816-4
// "return FCI" value, which some cards accept in place of 04.
const SELECT_P1_FID: u8 = 0x00;
const SELECT_P1_AID: u8 = 0x04;
const SELECT_P2_FCI: u8 = 0x00;
const SELECT_P2_FCP: u8 = 0x04;
const SELECT_P2_NO_DATA: u8 = 0x0C;

// Status words (TS 102 221 §10.2.1).
pub const SW_OK: u16 = 0x9000;
pub const SW_FILE_DEACTIVATED: u16 = 0x6283;
pub const SW_SECURITY_NOT_SATISFIED: u16 = 0x6982;
pub const SW_AUTH_METHOD_BLOCKED: u16 = 0x6983;
pub const SW_REF_DATA_INVALIDATED: u16 = 0x6984;
pub const SW_CONDITIONS_NOT_SATISFIED: u16 = 0x6985;
pub const SW_FILE_NOT_FOUND: u16 = 0x6A82;
pub const SW_WRONG_P1P2: u16 = 0x6B00;
pub const SW_NO_DIAGNOSIS: u16 = 0x6F00;
pub const SW_INS_NOT_SUPPORTED: u16 = 0x6D00;
const SW1_MASK: u16 = 0xFF00;
const SW1_MORE_DATA: u16 = 0x6100;
const SW1_WRONG_LE: u16 = 0x6C00;
const SW_VERIFY_MASK: u16 = 0xFFF0;
const SW_VERIFY_TRIES: u16 = 0x000F;
/// Logical channel bits of CLA (TS 102 221 §10.1.1).
const CLA_CHANNEL_MASK: u8 = 0x03;
pub const SW_VERIFY_FAILED: u16 = 0x63C0;

pub fn sw_is_selected(sw: u16) -> bool {
    sw == SW_OK || sw == SW_FILE_DEACTIVATED
}

pub fn sw_verify_tries(sw: u16) -> Option<u8> {
    ((sw & SW_VERIFY_MASK) == SW_VERIFY_FAILED).then_some((sw & SW_VERIFY_TRIES) as u8)
}

// Key references (TS 102 221 §9.5 Table 9.3).
pub const KEY_REF_PIN1: u8 = 0x01;
pub const KEY_REF_PIN2: u8 = 0x81;
pub const KEY_REF_ADM1: u8 = 0x0A;

/// UPDATE BINARY data per command, the short-APDU maximum.
const UPDATE_CHUNK: usize = 255;
/// READ BINARY Le per command. Le 00 asks for 256.
const READ_CHUNK: usize = 256;
/// INIT_META bytes per command, under the 128-byte APDU buffer some
/// platforms have.
const INIT_META_CHUNK: usize = 100;
const INIT_META_P1_LAST: u8 = 0x80;
const OFFSET_P1_MASK: usize = 0x7F;
const UNBLOCK_P1_NEW_PIN: u8 = 0x00;
const UNBLOCK_P1_KEEP_PIN: u8 = 0x01;
const RECORD_P2_ABSOLUTE: u8 = 0x04;

pub struct Response {
    pub data: Vec<u8>,
    pub sw: u16,
}

pub struct Card {
    transport: Box<dyn Transport>,
    trace: bool,
    /// Whether the current DF is the MF. SELECT by FID reaches only the
    /// children of the current DF (TS 102 221 §8.4.1), so a 2Fxx EF is
    /// selected from the MF and a 6Fxx EF from the ADF.
    at_mf: bool,
    /// The logical channel commands go to (TS 102 221 §8.6), 0 for the
    /// basic channel. Channels 1 to 3 are coded in b2..b1 of CLA.
    channel: u8,
}

pub const INS_MANAGE_CHANNEL: u8 = 0x70;
const MANAGE_CHANNEL_OPEN: u8 = 0x00;
const MANAGE_CHANNEL_CLOSE: u8 = 0x80;

impl Card {
    pub fn new(transport: Box<dyn Transport>, trace: bool) -> Card {
        Card {
            transport,
            trace,
            at_mf: false,
            channel: 0,
        }
    }

    pub fn atr(&self) -> Vec<u8> {
        self.transport.atr()
    }

    pub fn reader_name(&self) -> String {
        self.transport.name()
    }

    fn transmit_once(&mut self, apdu: &[u8]) -> Result<Response, CardError> {
        if self.trace {
            println!(">> {}", hex_spaced(apdu));
        }
        let mut raw = self.transport.transmit(apdu)?;
        if raw.len() < 2 {
            return Err(CardError::Transport(format!(
                "short response {}",
                hex_spaced(&raw)
            )));
        }
        let sw2 = raw.pop().unwrap_or(0);
        let sw1 = raw.pop().unwrap_or(0);
        let sw = u16::from_be_bytes([sw1, sw2]);
        if self.trace {
            let tail = if raw.is_empty() {
                String::new()
            } else {
                format!(" {}", hex_spaced(&raw))
            };
            println!("<< SW={sw:04X}{tail}");
        }
        Ok(Response { data: raw, sw })
    }

    /// Case 2 (header and Le) or case 4 (header, Lc, data and Le).
    fn has_le(apdu: &[u8]) -> bool {
        apdu.len() == 5 || (apdu.len() > 5 && apdu.len() == 6 + usize::from(apdu[4]))
    }

    /// One APDU with the T=0 response procedures of TS 102 221 §7.3.1: on
    /// 6CXX the command is resent with Le XX, on 61XX the response is
    /// fetched with GET RESPONSE.
    pub fn transmit(&mut self, apdu: &[u8]) -> Result<Response, CardError> {
        let mut apdu = apdu.to_vec();
        if let Some(cla) = apdu.first_mut() {
            *cla |= self.channel;
        }
        let mut r = self.transmit_once(&apdu)?;
        if (r.sw & SW1_MASK) == SW1_WRONG_LE && Card::has_le(&apdu) {
            *apdu.last_mut().unwrap_or(&mut 0) = r.sw as u8;
            r = self.transmit_once(&apdu)?;
        }
        let mut data = r.data;
        let mut sw = r.sw;
        while (sw & SW1_MASK) == SW1_MORE_DATA {
            let more = self.transmit_once(&[
                apdu[0] & CLA_CHANNEL_MASK,
                INS_GET_RESPONSE,
                0,
                0,
                sw as u8,
            ])?;
            data.extend(more.data);
            sw = more.sw;
        }
        Ok(Response { data, sw })
    }

    fn expect_ok(&mut self, op: impl Into<String>, apdu: &[u8]) -> Result<Vec<u8>, CardError> {
        let r = self.transmit(apdu)?;
        if r.sw != SW_OK {
            return Err(status(op, r.sw));
        }
        Ok(r.data)
    }

    fn note_selected(&mut self, fid: Fid, sw: u16) {
        if !sw_is_selected(sw) {
            return;
        }
        if fid == Fid::MF {
            self.at_mf = true;
        } else if fid == Fid::ADF || fid.is_df() {
            self.at_mf = false;
        }
    }

    fn enter_df_for(&mut self, fid: Fid) -> Result<(), CardError> {
        if fid == Fid::MF || fid == Fid::ADF {
            return Ok(());
        }
        if fid.is_mf_ef() && !self.at_mf {
            self.select_fid_plain(Fid::MF)?;
        } else if fid.is_adf_ef() && self.at_mf {
            self.select_fid_plain(Fid::ADF)?;
        }
        Ok(())
    }

    pub fn select_aid(&mut self, aid: &[u8]) -> Result<(), CardError> {
        let mut apdu = vec![
            CLA_UICC,
            INS_SELECT,
            SELECT_P1_AID,
            SELECT_P2_NO_DATA,
            aid.len() as u8,
        ];
        apdu.extend(aid);
        let r = self.transmit(&apdu)?;
        if !sw_is_selected(r.sw) {
            return Err(status("SELECT AID failed", r.sw));
        }
        self.at_mf = false;
        Ok(())
    }

    pub fn select_csim(&mut self) -> Result<(), CardError> {
        self.select_aid(&CSIM_INSTANCE_AID)
    }

    /// SELECT by partial DF name (TS 102 221 §11.1.1), returning the FCP.
    /// Le 00 makes it a case 4 APDU, which a T=1 card needs before it
    /// returns the FCP.
    pub fn select_aid_partial(&mut self, aid_prefix: &[u8]) -> Result<Vec<u8>, CardError> {
        let mut apdu = vec![
            CLA_UICC,
            INS_SELECT,
            SELECT_P1_AID,
            SELECT_P2_FCI,
            aid_prefix.len() as u8,
        ];
        apdu.extend(aid_prefix);
        apdu.push(0x00);
        let r = self.transmit(&apdu)?;
        if !sw_is_selected(r.sw) {
            return Err(status("SELECT partial AID failed", r.sw));
        }
        self.at_mf = false;
        Ok(r.data)
    }

    pub fn select_csim_partial(&mut self) -> Result<Vec<u8>, CardError> {
        self.select_aid_partial(&CSIM_AID_PREFIX)
    }

    /// Opens a logical channel from the basic channel and sends the
    /// following commands to it. On the new channel the MF is the current
    /// DF (TS 102 221 §8.6), whatever application the basic channel has
    /// selected, such as the card manager gp.jar leaves or this applet,
    /// which answers MF commands itself.
    pub fn open_channel(&mut self) -> Result<(), CardError> {
        self.channel = 0;
        let r = self.transmit(&[
            CLA_UICC,
            INS_MANAGE_CHANNEL,
            MANAGE_CHANNEL_OPEN,
            0x00,
            0x01,
        ])?;
        let number = match (r.sw, r.data.first()) {
            (SW_OK, Some(&n)) if (1..=u8::from(CLA_CHANNEL_MASK)).contains(&n) => n,
            (SW_OK, n) => {
                return Err(CardError::Invalid(format!(
                    "MANAGE CHANNEL assigned channel {n:?}"
                )));
            }
            (sw, _) => return Err(status("MANAGE CHANNEL open", sw)),
        };
        self.channel = number;
        self.at_mf = true;
        Ok(())
    }

    /// Closes the logical channel and sends the following commands to the
    /// basic channel again.
    pub fn close_channel(&mut self) -> Result<(), CardError> {
        let number = std::mem::replace(&mut self.channel, 0);
        self.at_mf = false;
        if number == 0 {
            return Ok(());
        }
        self.expect_ok(
            "MANAGE CHANNEL close",
            &[CLA_UICC, INS_MANAGE_CHANNEL, MANAGE_CHANNEL_CLOSE, number],
        )
        .map(drop)
    }

    fn select_fid_plain(&mut self, fid: Fid) -> Result<(), CardError> {
        let mut sw = 0;
        for p2 in [SELECT_P2_NO_DATA, SELECT_P2_FCI, SELECT_P2_FCP] {
            sw = self
                .transmit(&[
                    CLA_UICC,
                    INS_SELECT,
                    SELECT_P1_FID,
                    p2,
                    0x02,
                    fid.hi(),
                    fid.lo(),
                ])?
                .sw;
            if sw_is_selected(sw) {
                self.note_selected(fid, SW_OK);
                return Ok(());
            }
            if sw != SW_WRONG_P1P2 {
                break;
            }
        }
        Err(status(format!("SELECT {fid} failed"), sw))
    }

    /// SELECT by FID, first moving to the MF or the ADF when the FID is a
    /// child of the other one.
    pub fn select_fid(&mut self, fid: Fid) -> Result<(), CardError> {
        self.enter_df_for(fid)?;
        self.select_fid_plain(fid)
    }

    /// SELECT by FID asking for the FCP, trying P2 00, 04 then 0C while
    /// the card answers 6B00. A file that is not there is not an error:
    /// the status word comes back with the data.
    pub fn select_fid_fcp(&mut self, fid: Fid) -> Result<Response, CardError> {
        self.enter_df_for(fid)?;
        let mut last = Response {
            data: Vec::new(),
            sw: 0,
        };
        for p2 in [SELECT_P2_FCI, SELECT_P2_FCP, SELECT_P2_NO_DATA] {
            let mut apdu = vec![
                CLA_UICC,
                INS_SELECT,
                SELECT_P1_FID,
                p2,
                0x02,
                fid.hi(),
                fid.lo(),
            ];
            if p2 != SELECT_P2_NO_DATA {
                apdu.push(0x00);
            }
            last = self.transmit(&apdu)?;
            if last.sw != SW_WRONG_P1P2 {
                break;
            }
        }
        self.note_selected(fid, last.sw);
        Ok(last)
    }

    /// VERIFY. A disabled PIN1 answers 6985 or 6984 (TS 102 221
    /// §11.1.9.1.1), and its files are readable without it, so that
    /// counts as verified. With `required` false a refusal returns false
    /// instead of an error.
    pub fn verify(&mut self, key_ref: u8, value: &[u8], required: bool) -> Result<bool, CardError> {
        let mut apdu = vec![CLA_UICC, INS_VERIFY, 0x00, key_ref, value.len() as u8];
        apdu.extend(value);
        let sw = self.transmit(&apdu)?.sw;
        if sw == SW_OK {
            return Ok(true);
        }
        let disabled = sw == SW_CONDITIONS_NOT_SATISFIED || sw == SW_REF_DATA_INVALIDATED;
        if disabled && key_ref == KEY_REF_PIN1 {
            return Ok(true);
        }
        if !required {
            return Ok(false);
        }
        let op = match sw_verify_tries(sw) {
            Some(tries) => {
                format!("VERIFY ref={key_ref:02X} failed ({tries} tries remaining, wrong PIN/ADM)")
            }
            None => format!("VERIFY ref={key_ref:02X} failed"),
        };
        Err(status(op, sw))
    }

    /// VERIFY with no data: the retry counter, without an attempt
    /// (TS 102 221 §11.1.9).
    pub fn pin_status(&mut self, key_ref: u8) -> Result<u16, CardError> {
        Ok(self.transmit(&[CLA_UICC, INS_VERIFY, 0x00, key_ref])?.sw)
    }

    fn pin_command(
        &mut self,
        op: &str,
        ins: u8,
        p1: u8,
        key_ref: u8,
        data: &[u8],
    ) -> Result<(), CardError> {
        let mut apdu = vec![CLA_UICC, ins, p1, key_ref, data.len() as u8];
        apdu.extend(data);
        self.expect_ok(format!("{op} ref={key_ref:02X} failed"), &apdu)
            .map(drop)
    }

    pub fn change_pin(&mut self, key_ref: u8, old: &[u8], new: &[u8]) -> Result<(), CardError> {
        if old.len() != new.len() {
            return Err(CardError::Invalid(
                "old and new values must be the same length".into(),
            ));
        }
        self.pin_command(
            "CHANGE PIN",
            INS_CHANGE_PIN,
            0,
            key_ref,
            &[old, new].concat(),
        )
    }

    pub fn enable_pin(&mut self, key_ref: u8, pin: &[u8]) -> Result<(), CardError> {
        self.pin_command("ENABLE PIN", INS_ENABLE_PIN, 0, key_ref, pin)
    }

    pub fn disable_pin(&mut self, key_ref: u8, pin: &[u8]) -> Result<(), CardError> {
        self.pin_command("DISABLE PIN", INS_DISABLE_PIN, 0, key_ref, pin)
    }

    /// UNBLOCK PIN (TS 102 221 §11.1.13), with a new PIN or keeping the
    /// current one.
    pub fn unblock_pin(
        &mut self,
        key_ref: u8,
        puk: &[u8],
        new_pin: Option<&[u8]>,
    ) -> Result<(), CardError> {
        match new_pin {
            Some(pin) => self.pin_command(
                "UNBLOCK PIN",
                INS_UNBLOCK_PIN,
                UNBLOCK_P1_NEW_PIN,
                key_ref,
                &[puk, pin].concat(),
            ),
            None => self.pin_command(
                "UNBLOCK PIN",
                INS_UNBLOCK_PIN,
                UNBLOCK_P1_KEEP_PIN,
                key_ref,
                puk,
            ),
        }
    }

    pub fn update_binary(&mut self, data: &[u8], offset: usize) -> Result<(), CardError> {
        for (i, chunk) in data.chunks(UPDATE_CHUNK).enumerate() {
            let at = offset + i * UPDATE_CHUNK;
            let mut apdu = vec![
                CLA_UICC,
                INS_UPDATE_BINARY,
                ((at >> 8) & OFFSET_P1_MASK) as u8,
                at as u8,
                chunk.len() as u8,
            ];
            apdu.extend(chunk);
            self.expect_ok(format!("UPDATE BINARY @{at:04X}"), &apdu)?;
        }
        Ok(())
    }

    pub fn update_record(&mut self, record: u8, data: &[u8]) -> Result<(), CardError> {
        let mut apdu = vec![
            CLA_UICC,
            INS_UPDATE_RECORD,
            record,
            RECORD_P2_ABSOLUTE,
            data.len() as u8,
        ];
        apdu.extend(data);
        self.expect_ok(format!("UPDATE RECORD #{record}"), &apdu)
            .map(drop)
    }

    pub fn read_binary(&mut self, length: usize, offset: usize) -> Result<Vec<u8>, CardError> {
        let mut out = Vec::new();
        let mut at = offset;
        let mut left = length;
        while left > 0 {
            let n = left.min(READ_CHUNK);
            let data = self.expect_ok(
                format!("READ BINARY @{at:04X}"),
                &[
                    CLA_UICC,
                    INS_READ_BINARY,
                    ((at >> 8) & OFFSET_P1_MASK) as u8,
                    at as u8,
                    n as u8,
                ],
            )?;
            if data.is_empty() {
                break;
            }
            at += data.len();
            left = left.saturating_sub(data.len());
            out.extend(data);
        }
        Ok(out)
    }

    pub fn read_record(&mut self, record: u8, length: u8) -> Result<Vec<u8>, CardError> {
        self.expect_ok(
            format!("READ RECORD #{record}"),
            &[
                CLA_UICC,
                INS_READ_RECORD,
                record,
                RECORD_P2_ABSOLUTE,
                length,
            ],
        )
    }

    /// STATUS (TS 102 221 §11.1.2) on the current DF.
    pub fn status(&mut self) -> Result<Response, CardError> {
        self.transmit(&[CLA_CSIM, INS_STATUS, 0x00, 0x00, 0x00])
    }

    fn csim_command(
        &mut self,
        op: impl Into<String>,
        ins: u8,
        p1: u8,
        p2: u8,
        data: &[u8],
    ) -> Result<Vec<u8>, CardError> {
        let mut apdu = vec![CLA_CSIM, ins, p1, p2, data.len() as u8];
        apdu.extend(data);
        self.expect_ok(op, &apdu)
    }

    /// Writes a transparent EF whatever its UPDATE condition. For the EFs
    /// the spec marks UPDATE NEVER, whose value is fixed before
    /// personalization. ADM1 required.
    pub fn preperso_update(&mut self, fid: Fid, data: &[u8]) -> Result<(), CardError> {
        self.csim_command(
            format!("PREPERSO UPDATE {fid}"),
            INS_PREPERSO_UPDATE,
            fid.hi(),
            fid.lo(),
            data,
        )
        .map(drop)
    }

    /// Loads the CAVE A-key and zeroes the stored SSD. ADM1 required.
    pub fn load_a_key(&mut self, key: u64) -> Result<(), CardError> {
        let key: [u8; A_KEY_LEN] = key.to_be_bytes();
        self.csim_command("LOAD A-KEY", INS_LOAD_CAVE, CAVE_LOAD_AKEY, 0, &key)
            .map(drop)
    }

    /// Loads a pre-shared SSD_A || SSD_B. ADM1 required.
    pub fn load_ssd(&mut self, ssd: &[u8]) -> Result<(), CardError> {
        if ssd.len() != SSD_LEN {
            return Err(CardError::Invalid(format!("SSD must be {SSD_LEN} bytes")));
        }
        self.csim_command("LOAD SSD", INS_LOAD_CAVE, CAVE_LOAD_SSD, 0, ssd)
            .map(drop)
    }

    /// Loads the AKA 3G root key (`p1` 0) or the WLAN root key (`p1` 1).
    pub fn load_aka_key(&mut self, p1: u8, key: &[u8]) -> Result<(), CardError> {
        if key.len() != AKA_KEY_LEN {
            return Err(CardError::Invalid(format!(
                "AKA key must be {AKA_KEY_LEN} bytes"
            )));
        }
        self.csim_command(
            format!("LOAD AKA KEY p1={p1}"),
            INS_LOAD_AKA_KEY,
            p1,
            0,
            key,
        )
        .map(drop)
    }

    /// Sets PIN1 (`KEY_REF_PIN1`) or PIN2 (`KEY_REF_PIN2`) to an 8-byte
    /// VERIFY value and resets its retry counter. ADM1 required.
    pub fn set_pin(&mut self, key_ref: u8, pin: &[u8]) -> Result<(), CardError> {
        self.csim_command(
            format!("SET PIN ref={key_ref:02X}"),
            INS_SET_PIN,
            SET_PIN_P1_PIN,
            key_ref,
            pin,
        )
        .map(drop)
    }

    /// Sets PUK1 (`KEY_REF_PIN1`) or PUK2 (`KEY_REF_PIN2`) and resets its
    /// retry counter. ADM1 required.
    pub fn set_puk(&mut self, key_ref: u8, puk: &[u8]) -> Result<(), CardError> {
        self.csim_command(
            format!("SET PUK ref={key_ref:02X}"),
            INS_SET_PIN,
            SET_PIN_P1_PUK,
            key_ref,
            puk,
        )
        .map(drop)
    }

    pub fn load_ip_secret(&mut self, class: u8, index: u8, secret: &[u8]) -> Result<(), CardError> {
        self.csim_command(
            format!("LOAD IP SECRET class={class} idx={index}"),
            INS_LOAD_IPSEC,
            class,
            index,
            secret,
        )
        .map(drop)
    }

    /// The six A-key check digits over the stored A-key and an ESN, as
    /// ASCII.
    pub fn a_key_checksum(&mut self, esn: [u8; 4]) -> Result<Vec<u8>, CardError> {
        let mut apdu = vec![CLA_CSIM, INS_AKEY_CHECKSUM, 0, 0, esn.len() as u8];
        apdu.extend(esn);
        apdu.push(0x00);
        self.expect_ok("A-KEY CHECKSUM", &apdu)
    }

    pub fn platform_info(&mut self) -> Result<Vec<u8>, CardError> {
        self.expect_ok(
            "PLATFORM_INFO",
            &[CLA_CSIM, INS_DEBUG_PLATFORM_INFO, 0, 0, 0],
        )
    }

    /// Whether the applet has its EF table, from PLATFORM_INFO.
    pub fn ef_table_loaded(&mut self) -> Result<bool, CardError> {
        const META_LOADED_BYTE: usize = 15;
        Ok(self
            .platform_info()?
            .get(META_LOADED_BYTE)
            .is_some_and(|&b| b != 0))
    }

    /// Loads the EF table unless the applet already has it, and returns
    /// whether it was sent. Each complete load makes the applet allocate a
    /// new EF data pool, which erases every EF and leaves the old pool
    /// allocated, so a loaded table must not be sent again. ADM1 must be
    /// verified first.
    pub fn ensure_ef_table(&mut self, table: &[u8]) -> Result<bool, CardError> {
        if self.ef_table_loaded()? {
            return Ok(false);
        }
        self.init_meta(table)?;
        Ok(true)
    }

    /// Loads the EF table in chunks. P1 b8 marks the last chunk and P1
    /// b7..b1 with P2 is the offset. ADM1 must be verified first.
    pub fn init_meta(&mut self, table: &[u8]) -> Result<(), CardError> {
        for (i, chunk) in table.chunks(INIT_META_CHUNK).enumerate() {
            let at = i * INIT_META_CHUNK;
            let mut p1 = ((at >> 8) & OFFSET_P1_MASK) as u8;
            if at + chunk.len() >= table.len() {
                p1 |= INIT_META_P1_LAST;
            }
            self.csim_command(
                format!("INIT_META chunk @{at:04X}/{}", table.len()),
                INS_INIT_META,
                p1,
                at as u8,
                chunk,
            )?;
        }
        Ok(())
    }

    pub fn dump_meta(&mut self, offset: usize, length: u8) -> Result<Vec<u8>, CardError> {
        self.expect_ok(
            format!("DUMP_META @{offset:04X}"),
            &[
                CLA_CSIM,
                INS_DEBUG_DUMP_META,
                (offset >> 8) as u8,
                offset as u8,
                length,
            ],
        )
    }

    /// The applet's APDU trace, oldest entry first. Each chunk starts with
    /// the 2-byte ring head.
    pub fn dump_apdulog(&mut self, size: usize) -> Result<Vec<u8>, CardError> {
        let mut raw = Vec::new();
        let mut head = 0usize;
        while raw.len() < size {
            let off = raw.len();
            let data = self.expect_ok(
                format!("DUMP_APDULOG @{off}"),
                &[
                    CLA_CSIM,
                    INS_DEBUG_DUMP_APDULOG,
                    (off >> 8) as u8,
                    off as u8,
                    0,
                ],
            )?;
            if data.len() <= 2 {
                break;
            }
            head = usize::from(u16::from_be_bytes([data[0], data[1]]));
            raw.extend(&data[2..]);
        }
        raw.truncate(size);
        let head = head.min(raw.len());
        raw.rotate_left(head);
        Ok(raw)
    }

    pub fn clear_apdulog(&mut self) -> Result<(), CardError> {
        self.expect_ok("CLEAR_APDULOG", &[CLA_CSIM, INS_DEBUG_CLEAR_APDULOG, 0, 0])
            .map(drop)
    }
}

const PIN_LEN: usize = 8;

/// A PIN as the 8-byte VERIFY value: ASCII digits padded with FF.
pub fn pin_bytes(pin: &str) -> Result<Vec<u8>, CardError> {
    if pin.len() > PIN_LEN || !pin.is_ascii() {
        return Err(CardError::Invalid(format!(
            "a PIN is at most {PIN_LEN} ASCII characters"
        )));
    }
    let mut out = pin.as_bytes().to_vec();
    out.resize(PIN_LEN, 0xFF);
    Ok(out)
}

/// A new PIN as the VERIFY value: 4 to 8 digits padded with FF.
pub fn new_pin_bytes(pin: &str) -> Result<Vec<u8>, CardError> {
    if !(PIN_MIN_LEN..=PIN_LEN).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CardError::Invalid(format!(
            "a PIN is {PIN_MIN_LEN} to {PIN_LEN} digits, got {pin:?}"
        )));
    }
    pin_bytes(pin)
}

/// A PUK: exactly 8 digits.
pub fn puk_bytes(puk: &str) -> Result<Vec<u8>, CardError> {
    if puk.len() != PIN_LEN || !puk.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CardError::Invalid(format!(
            "a PUK is {PIN_LEN} digits, got {puk:?}"
        )));
    }
    Ok(puk.as_bytes().to_vec())
}

const PIN_MIN_LEN: usize = 4;

/// The applet's ADM1 as installed. `csim install` changes it to the
/// platform ADM1.
pub const FACTORY_ADM1: [u8; ADM1_LEN] = [0; ADM1_LEN];
const ADM1_LEN: usize = 8;

/// An ADM1 as the VERIFY value: 8 decimal digits sent as ASCII, the form
/// given in the sysmocom key material.
pub fn adm1_bytes(adm1: &str) -> Result<Vec<u8>, CardError> {
    if adm1.len() != ADM1_LEN || !adm1.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CardError::Invalid(format!(
            "ADM1 must be {ADM1_LEN} digits, got {adm1:?}"
        )));
    }
    Ok(adm1.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adm1_digits() {
        assert_eq!(adm1_bytes("12345678").unwrap(), b"12345678");
        for bad in ["1234567", "123456789", "1234567a", "0000000000000000"] {
            assert!(adm1_bytes(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn new_pin_and_puk() {
        assert_eq!(new_pin_bytes("5678").unwrap(), b"5678\xFF\xFF\xFF\xFF");
        for bad in ["123", "123456789", "12a4"] {
            assert!(new_pin_bytes(bad).is_err(), "{bad}");
        }
        assert_eq!(puk_bytes("12345678").unwrap(), b"12345678");
        assert!(puk_bytes("1234567").is_err());
    }

    #[test]
    fn pin_padding() {
        assert_eq!(pin_bytes("1234").unwrap(), b"1234\xFF\xFF\xFF\xFF");
        assert!(pin_bytes("123456789").is_err());
    }

    #[test]
    fn le_detection() {
        assert!(Card::has_le(&[0, 0xB0, 0, 0, 10]));
        assert!(!Card::has_le(&[0, 0xA4, 0, 0x0C, 2, 0x6F, 0x22]));
        assert!(Card::has_le(&[0, 0xA4, 0, 0x00, 2, 0x6F, 0x22, 0]));
    }
}
