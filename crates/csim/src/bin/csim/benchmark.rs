//! C.S0065-B §9.4.1 authentication timings using the card's existing SSD.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

mod authentication;
mod count;
use std::rc::Rc;
use std::time::Instant;

use clap::Args;

use csim::card::{Card, CardError, KEY_REF_PIN1, SW_OK, Transport, pin_bytes};
use csim::{Fid, parse_hex};

use crate::commands::Result;
use crate::pcsc_transport::PcscTransport;

const INS_AUTHENTICATE: u8 = 0x88;
const INS_GENERATE: u8 = 0x8E;
const RUN_CAVE_LEN: u8 = 17;
const SAVE_REGISTERS: u8 = 0x10;
const RAND_GLOBAL: u8 = 0;
const RAND_UNIQUE: u8 = 1;
const AUTH_CAVE: u8 = 0x80;
const CLA_CSIM: u8 = 0x80;
const CLA_UICC: u8 = 0;
const VPM_LAST: u8 = 0x40;
const NO_VPM: u8 = 0xFF;
const KEY_LEN: usize = 8;
const SIGNATURE_LEN: usize = 3;
const DEFAULT_ITERATIONS: u16 = 5;
const STATUS: &[u8] = &[0x80, 0xF2, 0, 0, 0];
const PIN_STATUS: &[u8] = &[0, 0x20, 0, KEY_REF_PIN1, 0];
const EF_READS: &[(u16, u8)] = &[(0x6F22, 10), (0x6F42, 1), (0x6F38, 8), (0x6F32, 5)];

#[derive(Args)]
pub(crate) struct BenchArgs {
    #[arg(long, env = "CSIM_READER")]
    reader: Option<String>,
    #[arg(long)]
    pin1: Option<String>,
    /// Handset ESN, eight hexadecimal digits.
    #[arg(long)]
    esn: String,
    #[arg(long, default_value_t = DEFAULT_ITERATIONS, value_parser = clap::value_parser!(u16).range(1..=100))]
    iterations: u16,
    /// Credentials for reference checks. No secrets are printed.
    #[arg(long)]
    profile: Option<PathBuf>,
    /// Commit a new test SSD. The network must resynchronize afterward.
    #[arg(long, requires = "profile")]
    commit_ssd: bool,
    /// Exercise COUNT writes and ring history, then restore all original records.
    #[arg(long)]
    check_count: bool,
}

#[derive(Default)]
struct Measurements {
    label: String,
    operations: BTreeMap<String, Vec<f64>>,
}

struct TimedTransport {
    inner: PcscTransport,
    measurements: Rc<RefCell<Measurements>>,
}

impl Transport for TimedTransport {
    fn transmit(&mut self, apdu: &[u8]) -> std::result::Result<Vec<u8>, CardError> {
        let start = Instant::now();
        let result = self.inner.transmit(apdu);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let sw = result
            .as_ref()
            .ok()
            .and_then(|r| r.get(r.len().checked_sub(2)?..))
            .map(|s| format!("{:02X}{:02X}", s[0], s[1]))
            .unwrap_or_else(|| "ERROR".into());
        println!(
            "exchange,{},{:02X},{elapsed_ms:.3},{sw}",
            self.measurements.borrow().label,
            apdu[1]
        );
        result
    }
    fn atr(&self) -> Vec<u8> {
        self.inner.atr()
    }
    fn name(&self) -> String {
        self.inner.name()
    }
}

struct Bench {
    card: Card,
    measurements: Rc<RefCell<Measurements>>,
}

impl Bench {
    fn measure<T>(
        &mut self,
        label: &str,
        operation: impl FnOnce(&mut Card) -> Result<T>,
    ) -> Result<T> {
        self.measurements.borrow_mut().label = label.into();
        let start = Instant::now();
        let result = operation(&mut self.card);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        println!(
            "operation,{label},,{elapsed_ms:.3},{}",
            if result.is_ok() { "OK" } else { "ERROR" }
        );
        if result.is_ok() {
            self.measurements
                .borrow_mut()
                .operations
                .entry(label.into())
                .or_default()
                .push(elapsed_ms);
        }
        result
    }

    fn command(&mut self, label: &str, apdu: &[u8], length: usize) -> Result<Vec<u8>> {
        self.measure(label, |card| {
            let response = card.transmit(apdu)?;
            if response.sw != SW_OK || response.data.len() != length {
                return Err(format!(
                    "{label}: SW={:04X}, response length={} expected={length}",
                    response.sw,
                    response.data.len()
                )
                .into());
            }
            Ok(response.data)
        })
    }

    fn auth(
        &mut self,
        label: &str,
        esn: [u8; 4],
        rand: u32,
        kind: u8,
        save: u8,
        digits: &[u8; 4],
    ) -> Result<()> {
        self.command(
            label,
            &auth_apdu(esn, rand, kind, save, digits),
            SIGNATURE_LEN,
        )?;
        Ok(())
    }

    fn generate(&mut self, label: &str, first: u8, last: u8) -> Result<Vec<u8>> {
        let length = KEY_LEN
            + if first == NO_VPM {
                0
            } else {
                usize::from(last - first + 1)
            };
        self.command(
            label,
            &[CLA_CSIM, INS_GENERATE, 0, 0, 2, first, last, 0],
            length,
        )
    }
}

fn auth_apdu(esn: [u8; 4], rand: u32, kind: u8, save: u8, digits: &[u8; 4]) -> Vec<u8> {
    let mut apdu = vec![CLA_UICC, INS_AUTHENTICATE, 0, AUTH_CAVE, RUN_CAVE_LEN, kind];
    apdu.extend(rand.to_be_bytes());
    apdu.extend(digits);
    apdu.push(save);
    apdu.extend(esn.into_iter().rev());
    apdu.extend([0, 0, 0, 0]);
    apdu
}

fn exercise(bench: &mut Bench, args: &BenchArgs, esn: [u8; 4]) -> Result<()> {
    let pin = pin_bytes(args.pin1.as_deref().unwrap_or(crate::DEFAULT_PIN1))?;
    bench.measure("select-csim", |c| Ok(c.select_csim()?))?;
    bench.measure("verify-pin1", |c| Ok(c.verify(KEY_REF_PIN1, &pin, true)?))?;
    for iteration in 0..args.iterations {
        println!("iteration,{}", iteration + 1);
        bench.measure("status", |c| {
            let response = c.transmit(STATUS)?;
            if response.sw != SW_OK {
                return Err(format!("STATUS: {:04X}", response.sw).into());
            }
            Ok(())
        })?;
        bench.measure("pin-status", |c| {
            let response = c.transmit(PIN_STATUS)?;
            if response.sw != SW_OK
                && csim::card::sw_verify_tries(response.sw).is_none()
                && response.sw != csim::card::SW_AUTH_METHOD_BLOCKED
            {
                return Err(format!("PIN status: {:04X}", response.sw).into());
            }
            Ok(())
        })?;
        for &(fid, length) in EF_READS {
            bench.measure(&format!("select-{fid:04X}"), |c| {
                Ok(c.select_fid_fcp(Fid(fid))?)
            })?;
            bench.measure(&format!("read-{fid:04X}"), |c| {
                Ok(c.read_binary(usize::from(length), 0)?)
            })?;
        }
        let rand = 0x1234_5678_u32.wrapping_add(u32::from(iteration));
        let registration = [0; 4];
        // C.S0023-D §4.4.4: bit length followed by packed DTMF digits (#777).
        let origination = [16, 0, 0xC7, 0x77];
        for (scenario, first, last) in [
            ("key-only", NO_VPM, NO_VPM),
            ("partial", 0, 5),
            ("full", 0, VPM_LAST),
            ("tail", 0x3C, VPM_LAST),
        ] {
            bench.auth(
                "run-cave-save-origination",
                esn,
                rand,
                RAND_GLOBAL,
                SAVE_REGISTERS,
                &origination,
            )?;
            let fresh = bench.generate(&format!("{scenario}-fresh"), first, last)?;
            let repeated = bench.generate(&format!("{scenario}-repeat"), first, last)?;
            if fresh != repeated {
                return Err(format!("{scenario}: repeated result differs").into());
            }
        }
        bench.auth(
            "run-cave-save-unique",
            esn,
            rand,
            RAND_UNIQUE,
            SAVE_REGISTERS,
            &registration,
        )?;
        bench.generate("unique-key-fresh", NO_VPM, NO_VPM)?;
        bench.generate("unique-full-after-key", 0, VPM_LAST)?;
        bench.auth(
            "run-cave-save-before-partial",
            esn,
            rand,
            RAND_GLOBAL,
            SAVE_REGISTERS,
            &origination,
        )?;
        let partial = bench.generate("sequence-partial", 0, 5)?;
        let expanded = bench.generate("sequence-full-after-partial", 0, VPM_LAST)?;
        if expanded[..partial.len()] != partial {
            return Err("partial key/VPM differs from full result".into());
        }
        bench.auth(
            "run-cave-save-registration",
            esn,
            rand,
            RAND_GLOBAL,
            SAVE_REGISTERS,
            &registration,
        )?;
        let key = bench.generate("sequence-key-only", NO_VPM, NO_VPM)?;
        let full = bench.generate("sequence-full-after-key", 0, VPM_LAST)?;
        if full[..KEY_LEN] != key {
            return Err("key changed when requesting VPM".into());
        }
        bench.auth(
            "run-cave-unique-no-save",
            esn,
            rand,
            RAND_UNIQUE,
            0,
            &registration,
        )?;
        if bench.generate("full-after-no-save", 0, VPM_LAST)? != full {
            return Err("unsaved authentication changed saved key/VPM".into());
        }
        bench.measure("reselect-mf", |c| Ok(c.select_fid(Fid::MF)?))?;
        bench.measure("reselect-csim", |c| Ok(c.select_csim()?))?;
        bench.measure("reverify-pin1", |c| {
            Ok(c.verify(KEY_REF_PIN1, &pin, true)?)
        })?;
        if bench.generate("full-after-reselect", 0, VPM_LAST)? != full {
            return Err("reselection changed saved key/VPM".into());
        }
    }
    Ok(())
}

fn median(values: &[f64]) -> f64 {
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

pub(crate) fn run(mut args: BenchArgs) -> Result<ExitCode> {
    let profile = args
        .profile
        .as_deref()
        .map(csim::profile::Profile::load)
        .transpose()
        .map_err(|_| "cannot load benchmark profile")?;
    if args.pin1.is_none() {
        if let Some(pin) = profile.as_ref().and_then(|p| p.pin1.as_ref()) {
            args.pin1 = Some(pin.text());
        }
    }
    let esn: [u8; 4] = parse_hex(&args.esn)?
        .try_into()
        .map_err(|_| "ESN must contain four bytes")?;
    let pin = pin_bytes(args.pin1.as_deref().unwrap_or(crate::DEFAULT_PIN1))?;
    drop(pin);
    let start = Instant::now();
    let transport = PcscTransport::open(args.reader.as_deref())?;
    println!("reader: {}", transport.name());
    println!("connect_ms: {:.3}", start.elapsed().as_secs_f64() * 1000.0);
    println!(
        "Timings include reader/PCSC latency, GET RESPONSE, and any reported transport recovery."
    );
    println!(
        "Fresh means after saved authentication, not a card power cycle. Response secrets are omitted."
    );
    let measurements = Rc::new(RefCell::new(Measurements::default()));
    let mut bench = Bench {
        card: Card::new(
            Box::new(TimedTransport {
                inner: transport,
                measurements: Rc::clone(&measurements),
            }),
            false,
        ),
        measurements,
    };
    let result = count::exercise(&mut bench, &args)
        .and_then(|()| exercise(&mut bench, &args, esn))
        .and_then(|()| authentication::exercise(&mut bench, &args, esn, profile.as_ref()));
    println!("summary,count,min_ms,median_ms,max_ms");
    for (label, values) in &mut bench.measurements.borrow_mut().operations {
        values.sort_by(f64::total_cmp);
        println!(
            "{label},{},{:.3},{:.3},{:.3}",
            values.len(),
            values[0],
            median(values),
            values[values.len() - 1]
        );
    }
    result?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCard {
        reject_pin: bool,
        calls: Rc<RefCell<Vec<u8>>>,
    }

    impl Transport for TestCard {
        fn transmit(&mut self, apdu: &[u8]) -> std::result::Result<Vec<u8>, CardError> {
            self.calls.borrow_mut().push(apdu[1]);
            if apdu[1] == INS_AUTHENTICATE {
                assert_eq!(apdu[0], CLA_UICC);
            }
            let mut data = match apdu[1] {
                0xA4 | 0xF2 => Vec::new(),
                0x20 if self.reject_pin => return Ok(vec![0x63, 0xC2]),
                0x20 => Vec::new(),
                0xB0 => vec![0; usize::from(apdu[4])],
                INS_AUTHENTICATE => vec![0; SIGNATURE_LEN],
                INS_GENERATE => {
                    let mut data = vec![0xAB; KEY_LEN];
                    if apdu[5] != NO_VPM {
                        data.extend(apdu[5]..=apdu[6]);
                    }
                    data
                }
                ins => panic!("unexpected instruction {ins:02X}"),
            };
            data.extend([0x90, 0]);
            Ok(data)
        }
        fn atr(&self) -> Vec<u8> {
            Vec::new()
        }
        fn name(&self) -> String {
            "test".into()
        }
    }

    #[test]
    fn sequence_completes_without_provisioning_and_stops_after_wrong_pin() {
        for reject_pin in [false, true] {
            let calls = Rc::new(RefCell::new(Vec::new()));
            let mut bench = Bench {
                card: Card::new(
                    Box::new(TestCard {
                        reject_pin,
                        calls: Rc::clone(&calls),
                    }),
                    false,
                ),
                measurements: Rc::new(RefCell::new(Measurements::default())),
            };
            let args = BenchArgs {
                reader: None,
                pin1: Some("1234".into()),
                esn: "8069398A".into(),
                iterations: 1,
                profile: None,
                commit_ssd: false,
                check_count: false,
            };
            let result = exercise(&mut bench, &args, [0x80, 0x69, 0x39, 0x8A]);
            assert_eq!(result.is_err(), reject_pin);
            if reject_pin {
                assert_eq!(*calls.borrow(), vec![0xA4, 0x20]);
            } else {
                assert!(
                    bench
                        .measurements
                        .borrow()
                        .operations
                        .contains_key("full-after-reselect")
                );
            }
        }
    }

    #[test]
    fn origination_encodes_save_digits_and_little_endian_esn() {
        assert_eq!(
            auth_apdu(
                [0x80, 0x69, 0x39, 0x8A],
                0x12345678,
                RAND_GLOBAL,
                SAVE_REGISTERS,
                &[16, 0, 0xC7, 0x77]
            ),
            vec![
                0x00, 0x88, 0, 0x80, 17, 0, 0x12, 0x34, 0x56, 0x78, 16, 0, 0xC7, 0x77, 0x10, 0x8A,
                0x39, 0x69, 0x80, 0, 0, 0, 0
            ]
        );
    }
}
