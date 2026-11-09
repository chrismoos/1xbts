//! The csim subcommands.

use std::path::Path;
use std::process::ExitCode;

use csim::card::{
    Card, FACTORY_ADM1, KEY_REF_ADM1, KEY_REF_PIN1, KEY_REF_PIN2, SW_CONDITIONS_NOT_SATISFIED,
    SW_INS_NOT_SUPPORTED, Transport, adm1_bytes, new_pin_bytes, pin_bytes, puk_bytes,
};
use csim::dump::{Credentials, DumpOptions};
use csim::ef_dir::{self, Placement};
use csim::personalize::{self, Summary};
use csim::profile::{self, Profile};
use csim::read::{self, ReadEntry};
use csim::sim::SimTransport;
use csim::template;
use csim::{CSIM_INSTANCE_AID, Fid, compare, ef_table, hex_spaced, hex_upper, parse_hex};

use crate::pcsc_transport::PcscTransport;
use crate::{CardArgs, ReaderArgs};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn code(failed: bool) -> ExitCode {
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

const SIM_READER: &str = "sim";

/// `sim` selects the jcardsim transport, anything else a PC/SC reader.
pub fn open(reader: Option<&str>, trace: bool) -> Result<Card> {
    let transport: Box<dyn Transport> = match reader {
        Some(SIM_READER) => Box::new(SimTransport::start()?),
        other => Box::new(PcscTransport::open(other)?),
    };
    let card = Card::new(transport, trace);
    println!("reader: {}", card.reader_name());
    println!("ATR:    {}", hex_spaced(&card.atr()));
    Ok(card)
}

/// Opens the card, selects the CSIM and verifies the requested keys.
fn connect(args: &CardArgs, adm: bool, pin1: bool, pin2: bool) -> Result<Card> {
    let adm1 = adm.then(|| adm1(args)).transpose()?;
    let mut card = open(args.reader.as_deref(), args.trace)?;
    card.select_csim()?;
    if let Some(adm1) = adm1 {
        card.verify(KEY_REF_ADM1, &adm1, true)?;
    }
    verify_pins(&mut card, args, pin1, pin2)?;
    Ok(card)
}

/// The simulated card starts from a fresh applet for every command, so it
/// always has the factory ADM1.
fn adm1(args: &CardArgs) -> Result<Vec<u8>> {
    if args.reader.as_deref() == Some(SIM_READER) {
        return Ok(FACTORY_ADM1.to_vec());
    }
    let adm1 = args
        .adm1
        .as_deref()
        .ok_or("ADM1 needed: give --adm1 or set PLATFORM_ADM1 to the ADM1 in the key material")?;
    Ok(adm1_bytes(adm1)?)
}

fn verify_pins(card: &mut Card, args: &CardArgs, pin1: bool, pin2: bool) -> Result<()> {
    if pin1 {
        card.verify(KEY_REF_PIN1, &pin_bytes(&args.pin1)?, true)?;
    }
    if pin2 && !card.verify(KEY_REF_PIN2, &pin_bytes(&args.pin2)?, false)? {
        println!("PIN2: not verified, the PIN2-protected EFs are skipped");
    }
    Ok(())
}

/// Opens the card for a profile write. PIN1 and PIN2 given in the profile
/// are set with ADM1 before they are verified, so the card's current
/// values do not matter.
fn connect_for_write(cli: CardArgs, p: &Profile) -> Result<(CardArgs, Card)> {
    let mut card = connect(&cli, true, false, false)?;
    for (label, key_ref, pin) in [
        ("PIN1", KEY_REF_PIN1, &p.pin1),
        ("PIN2", KEY_REF_PIN2, &p.pin2),
    ] {
        if let Some(pin) = pin {
            card.set_pin(key_ref, &new_pin_bytes(&pin.text())?)?;
            println!("{label}: set from the profile");
        }
    }
    let args = credentials_from_profile(cli, p);
    verify_pins(&mut card, &args, true, true)?;
    Ok((args, card))
}

fn credentials_from_profile(mut args: CardArgs, p: &Profile) -> CardArgs {
    if let Some(pin) = &p.pin1 {
        args.pin1 = pin.text();
    }
    if let Some(pin) = &p.pin2 {
        args.pin2 = pin.text();
    }
    args
}

/// Loads and validates a profile. Problems end the command unless
/// `force`.
fn load_checked(path: &Path, force: bool) -> Result<(Profile, bool)> {
    let p = Profile::load(path)?;
    let problems = profile::validate(&p);
    if !problems.is_empty() {
        println!("validation errors:");
        for e in &problems {
            println!("  - {e}");
        }
        if !force {
            return Err("the profile has problems (--force writes anyway)".into());
        }
        println!("(--force given, proceeding anyway)");
    }
    Ok((p, problems.is_empty()))
}

pub fn info(args: &ReaderArgs) -> Result<ExitCode> {
    let mut card = open(args.reader.as_deref(), args.trace)?;
    if let Err(e) = card.select_csim() {
        println!(
            "CSIM AID: {} not found ({e})",
            hex_spaced(&CSIM_INSTANCE_AID)
        );
        return Ok(ExitCode::FAILURE);
    }
    println!("CSIM AID: {} (selectable)", hex_spaced(&CSIM_INSTANCE_AID));
    println!();
    println!("always-readable EFs (no PIN required):");
    const PROBE_LEN: usize = 64;
    for label in ["ICCID", "RUIMID", "ESN_ME", "LI", "AD", "ECC"] {
        let Some(entry) = ef_table::by_name(label) else {
            continue;
        };
        let len = usize::from(entry.size).min(PROBE_LEN);
        match card
            .select_fid(entry.fid)
            .and_then(|_| card.read_binary(len, 0))
        {
            Ok(data) => {
                println!("  {label:8} {}", hex_spaced(&data));
                let decoded = ReadEntry {
                    label: entry.name.as_str(),
                    content: Ok(data),
                }
                .decoded();
                for (k, v) in decoded {
                    println!("  {:8}   {k}: {v}", "");
                }
            }
            Err(e) => println!("  {label:8} ({e})"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn validate(path: &Path) -> Result<ExitCode> {
    let p = Profile::load(path)?;
    let problems = profile::validate(&p);
    if problems.is_empty() {
        println!("{}: OK", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    println!("{}: {} error(s):", path.display(), problems.len());
    for e in &problems {
        println!("  - {e}");
    }
    Ok(ExitCode::FAILURE)
}

/// Fills a profile template into a new profile and validates it.
pub fn profile(
    template_path: &Path,
    imsi: &str,
    mdn: Option<&str>,
    output: Option<&Path>,
    force: bool,
) -> Result<ExitCode> {
    let sub = template::Subscriber::new(imsi, mdn)?;
    let text = std::fs::read_to_string(template_path)
        .map_err(|e| format!("{}: {e}", template_path.display()))?;
    let template_dir = template_path.parent().unwrap_or(Path::new("."));
    let output = match output {
        Some(p) => p.to_owned(),
        None => template_dir.join(format!("card-{}.local.yaml", sub.imsi())),
    };
    if output.exists() && !force {
        return Err(format!(
            "{} exists. Give --force to replace it, which discards its A-key and secrets",
            output.display()
        )
        .into());
    }
    let mut urandom = std::fs::File::open("/dev/urandom")?;
    let mut random_err = None;
    let filled = template::fill(&text, &sub, |buf| {
        if let Err(e) = std::io::Read::read_exact(&mut urandom, buf) {
            random_err = Some(e);
        }
    })?;
    if let Some(e) = random_err {
        return Err(format!("/dev/urandom: {e}").into());
    }
    let output_dir = output.parent().unwrap_or(Path::new("."));
    let filled = rebase_prl_paths(&filled, template_dir, output_dir)?;
    let header = format!(
        "# Filled from {} for IMSI {}.\n\
         # Holds the A-key and IP secrets the network must also be given.\n\
         # Keep it out of version control.\n\n",
        template_path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        sub.imsi()
    );
    write_private(&output, &format!("{header}{filled}"))?;

    let p = Profile::load(&output)?;
    let problems = profile::validate(&p);
    println!("{}", output.display());
    println!("  IMSI   {}", sub.imsi());
    println!(
        "  MDN    {}",
        p.mdn.as_ref().map(|s| s.text()).unwrap_or_default()
    );
    println!(
        "  ICCID  {}",
        p.iccid.as_ref().map(|s| s.text()).unwrap_or_default()
    );
    if problems.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    println!("{} error(s):", problems.len());
    for e in &problems {
        println!("  - {e}");
    }
    Ok(ExitCode::FAILURE)
}

const PRL_PATH_KEYS: [&str; 2] = ["prl_file:", "eprl_file:"];

/// Rewrites relative `prl_file` and `eprl_file` paths, which are relative
/// to the profile, for a profile written to another directory.
fn rebase_prl_paths(text: &str, from: &Path, to: &Path) -> Result<String> {
    let abs = |p: &Path| -> Result<std::path::PathBuf> {
        let p = if p.as_os_str().is_empty() {
            Path::new(".")
        } else {
            p
        };
        Ok(std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))?)
    };
    let (from, to) = (abs(from)?, abs(to)?);
    if from == to {
        return Ok(text.to_owned());
    }
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let rebased = PRL_PATH_KEYS.iter().find_map(|key| {
            let value = line.strip_prefix(key)?;
            let (path, comment) = match value.find(" #") {
                Some(i) => (value[..i].trim(), &value[i..]),
                None => (value.trim(), ""),
            };
            let path = path.trim_matches('"');
            if Path::new(path).is_absolute() {
                return None;
            }
            Some(
                abs(&from.join(path))
                    .map(|file| format!("{key} {}{comment}", relative_to(&file, &to))),
            )
        });
        match rebased {
            Some(r) => out.push_str(&r?),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    Ok(out)
}

/// `path` relative to the directory `base`, or `path` itself when the two
/// share only the root. Both are canonical.
fn relative_to(path: &Path, base: &Path) -> String {
    use std::path::Component;
    let parts: Vec<Component> = path.components().collect();
    let base: Vec<Component> = base.components().collect();
    let common = parts.iter().zip(&base).take_while(|(a, b)| a == b).count();
    if common <= 1 {
        return path.display().to_string();
    }
    let path = parts;
    let mut rel = std::path::PathBuf::new();
    for _ in common..base.len() {
        rel.push("..");
    }
    for c in &path[common..] {
        rel.push(c);
    }
    rel.display().to_string()
}

/// Writes a file readable only by its owner.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const OWNER_READ_WRITE: u32 = 0o600;
        options.mode(OWNER_READ_WRITE);
    }
    let mut f = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

pub struct WriteOptions {
    pub validate_only: bool,
    pub force: bool,
    pub keep_pin1: bool,
}

/// Loads the EF table when the applet lacks it, reporting which.
pub fn ensure_ef_table(card: &mut Card) -> Result<()> {
    let table = ef_table::meta_bytes();
    if card.ensure_ef_table(&table)? {
        println!("EF table loaded ({} bytes)", table.len());
    } else {
        println!("EF table already loaded");
    }
    Ok(())
}

fn load_table_and_apply(card: &mut Card, p: &Profile) -> Result<Summary> {
    println!();
    ensure_ef_table(card)?;
    println!();
    Ok(personalize::apply_profile(card, p)?)
}

pub fn write(args: CardArgs, path: &Path, opts: WriteOptions) -> Result<ExitCode> {
    let (p, valid) = load_checked(path, opts.force)?;
    if opts.validate_only {
        println!(
            "{}",
            if valid {
                "validation OK"
            } else {
                "validation FAILED"
            }
        );
        return Ok(code(!valid));
    }
    let (args, mut card) = connect_for_write(args, &p)?;
    load_table_and_apply(&mut card, &p)?.print();

    if !opts.keep_pin1 {
        println!();
        disable_pin1(&mut card, &args.pin1);
    }
    Ok(ExitCode::SUCCESS)
}

/// Disables PIN1, so the handset does not ask for it at boot. A PIN1
/// that is already disabled answers 6985 (TS 102 221 §11.1.10).
pub fn disable_pin1(card: &mut Card, pin1: &str) {
    let result = pin_bytes(pin1).and_then(|pin| card.disable_pin(KEY_REF_PIN1, &pin));
    match result {
        Ok(()) => println!("PIN1: disabled (the handset does not prompt at boot)"),
        Err(e) if e.sw() == Some(SW_CONDITIONS_NOT_SATISFIED) => println!("PIN1: already disabled"),
        Err(e) => println!("PIN1: disable failed ({e})"),
    }
}

pub fn verify(
    args: CardArgs,
    path: &Path,
    force: bool,
    save_dump: Option<&Path>,
) -> Result<ExitCode> {
    let (p, _) = load_checked(path, force)?;
    let (args, mut card) = connect_for_write(args, &p)?;
    println!("=== writing ===");
    load_table_and_apply(&mut card, &p)?.print();
    println!();
    println!("=== reading back ===");
    let entries = read::read_all(&mut card);
    read::print_read_results(&entries);
    println!();
    println!("=== diff ===");
    let checks = read::verify_checks(&p, &entries);
    for c in &checks {
        let read = c
            .read
            .as_deref()
            .map_or("None".to_owned(), |r| format!("{r:?}"));
        println!(
            "  [{}] {}: wrote {:?}, read {read}",
            if c.ok() { "OK " } else { "FAIL" },
            c.label,
            c.wrote
        );
    }
    let mut failed = checks.iter().filter(|c| !c.ok()).count();
    println!();
    if failed > 0 {
        println!(
            "verify FAILED: {failed} mismatch(es), {} OK",
            checks.len() - failed
        );
    } else {
        println!("verify OK: {} checks passed", checks.len());
    }
    if let Some(save) = save_dump {
        println!();
        println!("=== dump ===");
        let creds = Credentials {
            pin1: Some(pin_bytes(&args.pin1)?),
            pin2: Some(pin_bytes(&args.pin2)?),
            adm1: Some(adm1(&args)?),
        };
        let dump = csim::dump::dump_card(
            &mut card,
            &DumpOptions {
                credentials: creds,
                scan: false,
                init_meta: false,
                progress: None,
            },
        )?;
        std::fs::write(save, serde_yaml::to_string(&dump)?)?;
        println!("saved dump to {}", save.display());
        println!();
        println!("=== every write against the dump ===");
        let (report, bad) =
            personalize::format_profile_comparison(&personalize::compare_profile(&dump, &p)?);
        println!("{report}");
        failed += bad;
    }
    Ok(code(failed > 0))
}

pub fn read(args: &CardArgs, save: Option<&Path>) -> Result<ExitCode> {
    let mut card = connect(args, true, true, false)?;
    println!();
    let entries = read::read_all(&mut card);
    read::print_read_results(&entries);
    if let Some(save) = save {
        std::fs::write(
            save,
            serde_yaml::to_string(&read::read_back_profile(&entries))?,
        )?;
        println!("\nsaved decoded profile to {}", save.display());
    }
    Ok(ExitCode::SUCCESS)
}

pub struct DumpCredentials {
    pub pin1: Option<String>,
    pub pin2: Option<String>,
    pub adm1: Option<String>,
    pub adm1_hex: Option<String>,
}

pub fn dump(
    reader: &ReaderArgs,
    creds: DumpCredentials,
    scan: bool,
    init_meta: bool,
    verbose: bool,
    save: Option<&Path>,
) -> Result<ExitCode> {
    let mut card = open(reader.reader.as_deref(), reader.trace)?;
    let credentials = Credentials {
        pin1: creds.pin1.as_deref().map(pin_bytes).transpose()?,
        pin2: creds.pin2.as_deref().map(pin_bytes).transpose()?,
        adm1: match (creds.adm1, creds.adm1_hex) {
            (Some(adm1), _) => Some(adm1_bytes(&adm1)?),
            (None, Some(hex)) => Some(parse_hex(&hex)?),
            (None, None) => None,
        },
    };
    let progress: Option<Box<dyn Fn(&str)>> =
        verbose.then(|| Box::new(|m: &str| eprintln!("  {m}")) as Box<dyn Fn(&str)>);
    let mut dump = csim::dump::dump_card(
        &mut card,
        &DumpOptions {
            credentials,
            scan,
            init_meta,
            progress,
        },
    )?;
    dump.insert("reader".into(), card.reader_name().into());
    csim::dump::print_summary(&dump);
    if let Some(save) = save {
        std::fs::write(save, serde_yaml::to_string(&dump)?)?;
        println!("saved dump to {}", save.display());
    }
    Ok(ExitCode::SUCCESS)
}

pub fn load_dump(path: &Path) -> Result<serde_yaml::Mapping> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?)
}

fn stem(path: &Path) -> String {
    path.file_stem().map_or_else(
        || path.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    )
}

/// The report text and whether anything differs.
pub fn compare_report(
    a: &Path,
    b: Option<&Path>,
    table: bool,
    content: bool,
    profile: Option<&Path>,
) -> Result<(String, bool)> {
    let da = load_dump(a)?;
    let left = stem(a);
    if let Some(profile) = profile {
        let p = Profile::load(profile)?;
        let name = profile
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        let (report, failed) =
            personalize::format_profile_comparison(&personalize::compare_profile(&da, &p)?);
        return Ok((
            format!("{left} against the writes of {name}:\n{report}"),
            failed > 0,
        ));
    }
    if let (Some(b), false) = (b, table) {
        let db = load_dump(b)?;
        let r = compare::compare_dumps(&da, &db, content);
        return Ok((
            compare::format_dump_report(&r, &left, &stem(b)),
            r.differs(),
        ));
    }
    let r = compare::compare_to_table(&da);
    Ok((compare::format_table_report(&r, &left), r.differs()))
}

pub fn compare(
    a: &Path,
    b: Option<&Path>,
    table: bool,
    content: bool,
    profile: Option<&Path>,
) -> Result<ExitCode> {
    let (report, differs) = compare_report(a, b, table, content, profile)?;
    println!("{report}");
    Ok(code(differs))
}

pub fn init_meta(args: &CardArgs) -> Result<ExitCode> {
    let mut card = connect(args, true, false, false)?;
    println!();
    ensure_ef_table(&mut card)?;
    Ok(ExitCode::SUCCESS)
}

/// A hardened applet answers its debug commands with 6D00.
fn debug_only(name: &str) -> impl Fn(csim::card::CardError) -> Box<dyn std::error::Error> + '_ {
    move |e| {
        if e.sw() == Some(SW_INS_NOT_SUPPORTED) {
            format!(
                "`{name}` needs the debug build of the applet. Build it with `./gradlew dist -Pdebug=true`"
            )
            .into()
        } else {
            e.into()
        }
    }
}

pub fn verify_meta(args: &CardArgs) -> Result<ExitCode> {
    const CHUNK: usize = 200;
    let mut card = connect(args, true, false, false)?;
    let expected = ef_table::meta_bytes();
    println!();
    println!("reading {} bytes in {CHUNK}-byte chunks...", expected.len());
    let mut actual = vec![0u8; expected.len()];
    for off in (0..expected.len()).step_by(CHUNK) {
        let n = CHUNK.min(expected.len() - off);
        let got = card
            .dump_meta(off, n as u8)
            .map_err(debug_only("verify-meta"))?;
        actual[off..off + got.len().min(n)].copy_from_slice(&got[..got.len().min(n)]);
        if got.len() < n {
            println!("  warn: chunk @{off:04X} got {} of {n}", got.len());
        }
    }
    println!();
    let mismatches: Vec<(usize, u8, u8)> = expected
        .iter()
        .zip(&actual)
        .enumerate()
        .filter(|(_, (e, a))| e != a)
        .map(|(i, (e, a))| (i, *e, *a))
        .collect();
    if mismatches.is_empty() {
        println!("OK: all {} bytes match", expected.len());
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "FAIL: {} of {} bytes differ",
        mismatches.len(),
        expected.len()
    );
    println!("first 20 mismatches (offset: expected -> actual):");
    for (i, e, a) in mismatches.iter().take(20) {
        println!(
            "  [{i:3} = entry {}, byte {}]: {e:02X} -> {a:02X}",
            i / ef_table::ENTRY_LEN,
            i % ef_table::ENTRY_LEN
        );
    }
    Ok(ExitCode::FAILURE)
}

pub fn register_dir(reader: &ReaderArgs, platform_adm1: &str, first: bool) -> Result<ExitCode> {
    let adm = adm1_bytes(platform_adm1)?;
    let mut card = open(reader.reader.as_deref(), reader.trace)?;
    let placement = if first {
        Placement::First
    } else {
        Placement::FirstFree
    };
    match ef_dir::register(&mut card, &adm, placement) {
        Ok(reg) => {
            let state = if reg.changed {
                "registered"
            } else {
                "already-registered"
            };
            println!("EF_DIR: CSIM {state} in record #{}", reg.record);
            for (i, (aid, label)) in reg.applications.iter().enumerate() {
                println!("  #{} {aid} {label:?}", i + 1);
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("EF_DIR registration failed: {e}");
            Ok(ExitCode::FAILURE)
        }
    }
}

const APDU_LOG_SIZE: usize = 352;
const APDU_LOG_ENTRY: usize = 11;
/// The SW the trace holds for a command still running.
const APDU_LOG_NO_SW: u16 = 0xFFFF;

fn sw_meaning(sw: u16) -> &'static str {
    match sw {
        0x9000 => "OK",
        0x6100 => "61 XX, more data",
        0x6200 => "warning, no info",
        0x6281 | 0x6282 => "EOF reached",
        0x6283 => "selected file deactivated",
        0x6700 => "wrong length",
        0x6981 => "command incompatible with file structure",
        0x6982 => "security status not satisfied",
        0x6983 => "auth blocked",
        0x6985 => "conditions of use not satisfied",
        0x6986 => "command not allowed (no current EF)",
        0x6A80 => "incorrect data parameters",
        0x6A81 => "function not supported",
        0x6A82 => "file not found",
        0x6A83 => "record not found",
        0x6A86 => "incorrect P1/P2",
        0x6A88 => "referenced data not found",
        0x6B00 => "wrong P1/P2",
        0x6D00 => "INS not supported",
        0x6E00 => "CLA not supported",
        0x6F00 => "no precise diagnosis (uncaught exception)",
        APDU_LOG_NO_SW => "(no SW recorded)",
        _ => "?",
    }
}

fn ins_name(ins: u8) -> &'static str {
    match ins {
        0xA4 => "SELECT",
        0xB0 => "READ BINARY",
        0xD6 => "UPDATE BINARY",
        0xB2 => "READ RECORD",
        0xDC => "UPDATE RECORD",
        0xF2 => "STATUS",
        0x20 => "VERIFY",
        0x24 => "CHANGE PIN",
        0x26 => "DISABLE PIN",
        0x28 => "ENABLE PIN",
        0x2C => "UNBLOCK PIN",
        0x70 => "MANAGE CHANNEL",
        0xC0 => "GET RESPONSE",
        0x32 => "INCREASE",
        0xA2 => "SEARCH RECORD",
        0x10 => "TERMINAL PROFILE",
        0xAA => "TERMINAL CAPABILITY",
        0x88 => "AUTHENTICATE",
        0x8A => "BASE STATION CHALLENGE",
        0x82 => "MANAGE SSD",
        0x8E => "GENERATE KEY/VPM",
        0x54 => "CONFIG REQUEST",
        0x56 => "DOWNLOAD REQUEST",
        0x50 => "KEY REQUEST",
        0xEE => "OTAPA REQUEST",
        0xDE => "STORE ESN_MEID_ME",
        0xFA => "INIT META",
        0xFB => "DUMP META",
        0xFC => "DUMP APDULOG",
        0xFD => "CLEAR APDULOG",
        _ => "?",
    }
}

const SELECT_BY_AID: u8 = 0x04;
const SELECT_BY_PATH: u8 = 0x08;

/// What a SELECT in the trace named, from its first four data bytes.
fn select_target(p1: u8, lc: u8, data: &[u8]) -> String {
    let shown = hex_upper(&data[..usize::from(lc).min(data.len())]);
    let label = |hi: u8, lo: u8| Fid(u16::from_be_bytes([hi, lo])).label().map(str::to_owned);
    match (p1, lc) {
        (SELECT_BY_AID, _) => format!("AID {shown}... "),
        (_, 2) => format!(
            "{} ",
            label(data[0], data[1]).unwrap_or_else(|| format!("FID {shown}"))
        ),
        (SELECT_BY_PATH, 4) => format!(
            "path {}/{} ",
            &shown[..4],
            label(data[2], data[3]).unwrap_or_else(|| shown[4..].to_owned())
        ),
        (SELECT_BY_PATH, _) => format!("path {shown}... "),
        _ => String::new(),
    }
}

pub fn apdulog(args: &CardArgs, clear: bool) -> Result<ExitCode> {
    let mut card = connect(args, false, false, false)?;
    if clear {
        card.clear_apdulog().map_err(debug_only("apdulog"))?;
        println!("APDU log cleared");
        return Ok(ExitCode::SUCCESS);
    }
    let raw = card
        .dump_apdulog(APDU_LOG_SIZE)
        .map_err(debug_only("apdulog"))?;
    println!(
        "{:>3}  {:>3} {:>3} {:>3} {:>3} {:>3}  {:8}  {:>4}  {:22}  meaning",
        "#", "CLA", "INS", "P1", "P2", "LC", "DATA", "SW", "INS name"
    );
    println!("{}", "-".repeat(96));
    let mut n = 0;
    for e in raw.chunks_exact(APDU_LOG_ENTRY) {
        let (cla, ins, p1, p2, lc) = (e[0], e[1], e[2], e[3], e[4]);
        let data = &e[5..9];
        let sw = u16::from_be_bytes([e[9], e[10]]);
        if cla == 0 && ins == 0 && sw == 0 {
            continue;
        }
        n += 1;
        let (data_s, target) = if ins == 0xA4 && lc > 0 {
            (
                hex_upper(&data[..usize::from(lc).min(4)]),
                select_target(p1, lc, data),
            )
        } else {
            (String::new(), String::new())
        };
        println!(
            "{n:>3}  {cla:02X}  {ins:02X}  {p1:02X}  {p2:02X}  {lc:02X}  {data_s:8}  {sw:04X}  {:22}  {target}{}",
            ins_name(ins),
            sw_meaning(sw)
        );
    }
    if n == 0 {
        println!("(log empty: the phone has not talked to the card, or the log was cleared)");
    } else {
        println!("\n{n} APDUs recorded");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn platform_info(args: &CardArgs) -> Result<ExitCode> {
    const MIN_LEN: usize = 26;
    const WITH_BUILD_STAGE: usize = 29;
    const WITH_CAVE_STEP: usize = 32;
    const WITH_IP_STEP: usize = 35;
    const NO_CHECKSUM: u16 = 0xFFFF;
    let mut card = connect(args, true, false, false)?;
    let d = card.platform_info()?;
    if d.len() < MIN_LEN {
        return Err(format!("PLATFORM_INFO returned {} bytes", d.len()).into());
    }
    let be = |i: usize| u16::from_be_bytes([d[i], d[i + 1]]);
    let ver = be(0);
    println!();
    println!("JC API version:            {}.{}", ver >> 8, ver & 0xFF);
    println!(
        "commit capacity:           {} bytes max, {} unused",
        be(2),
        be(4)
    );
    let more = if be(6) == 32767 { " (or more)" } else { "" };
    println!("available persistent:      {} bytes{more}", be(6));
    println!("available transient reset: {} bytes", be(8));
    println!("available transient desel: {} bytes", be(10));
    println!("APDU buffer:               {} bytes", be(12));
    println!(
        "object deletion:           {}",
        if d[14] != 0 { "yes" } else { "no" }
    );
    println!(
        "EF table loaded:           {}",
        if d[15] != 0 { "yes" } else { "no" }
    );
    for (label, a, b) in [("EF_META", 16, 18), ("data pool", 20, 22)] {
        let (direct, copy) = (be(a), be(b));
        let verdict = if direct == NO_CHECKSUM {
            "direct indexing THREW"
        } else if direct == copy {
            "direct indexing and arrayCopy agree"
        } else {
            "MISMATCH between direct indexing and arrayCopy"
        };
        println!(
            "{:27}{direct:04X} direct, {copy:04X} copy: {verdict}",
            format!("{label} checksum:")
        );
    }
    println!("data pool length:          {} bytes", be(24));
    if d.len() >= WITH_BUILD_STAGE {
        let stage = match d[26] {
            0 => None,
            1 => Some("Cave"),
            2 => Some("IpAuth"),
            3 => Some("Ss0055"),
            4 => Some("Aka"),
            5 => Some("OtaspDh"),
            6 => Some("OtaNam"),
            _ => Some("?"),
        };
        match stage {
            None => println!("subsystem build:           no failure"),
            Some(s) => println!(
                "subsystem build:           {s} constructor threw, reason {:04X}",
                be(27)
            ),
        }
    }
    if d.len() >= WITH_CAVE_STEP {
        let exception = match d[29] {
            0 => "none",
            1 => "SystemException",
            2 => "TransactionException",
            _ => "other CardRuntimeException",
        };
        let step = be(30);
        let steps = [
            "before constructor",
            "state RAM",
            "A-key",
            "SSD_A",
            "SSD_B",
            "SSD_A candidate",
            "SSD_B candidate",
            "256-byte lookup table",
            "lookup table low copy",
            "lookup table high copy",
            "LFSR RAM",
            "register RAM",
            "shuffle RAM",
            "ESN RAM",
            "offset RAM",
            "key RAM",
            "VPM RAM",
        ];
        let label = steps.get(usize::from(step)).unwrap_or(&"unknown");
        println!("CAVE build diagnostic:     {exception}, step {step} ({label})");
    }
    if d.len() >= WITH_IP_STEP {
        let exception = match d[32] {
            0 => "none",
            1 => "SystemException",
            2 => "TransactionException",
            _ => "other CardRuntimeException",
        };
        let step = be(33);
        let steps = [
            "before constructor",
            "secret pool",
            "secret lengths",
            "MD5 provider",
            "state RAM",
        ];
        let label = steps.get(usize::from(step)).unwrap_or(&"unknown");
        println!("IP build diagnostic:       {exception}, step {step} ({label})");
    }
    const WITH_RNG_DIAGNOSTIC: usize = 39;
    const RNG_STEP: usize = 35;
    const RNG_KIND: usize = 36;
    const RNG_REASON: usize = 37;
    if d.len() >= WITH_RNG_DIAGNOSTIC {
        let step = match d[RNG_STEP] {
            0 => "no failure",
            1 => "provider creation",
            2 => "generation",
            _ => "unknown",
        };
        let kind = match d[RNG_KIND] {
            0 => "none",
            1 => "SystemException",
            2 => "CryptoException",
            3 => "SecurityException",
            _ => "RuntimeException",
        };
        println!(
            "RNG diagnostic:           {step}, {kind}, reason {:04X}",
            be(RNG_REASON)
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn chpin(
    args: &CardArgs,
    which: &str,
    current: Option<&str>,
    new: Option<&str>,
    disable: bool,
    enable: bool,
) -> Result<ExitCode> {
    if disable || enable {
        if which != "pin1" {
            return Err("--disable and --enable apply to pin1 only".into());
        }
        let mut card = connect(args, false, false, false)?;
        let pin = pin_bytes(current.unwrap_or(&args.pin1))?;
        let want = if disable { "disabled" } else { "enabled" };
        let result = if disable {
            card.disable_pin(KEY_REF_PIN1, &pin)
        } else {
            card.enable_pin(KEY_REF_PIN1, &pin)
        };
        match result {
            // TS 102 221 §11.1.10 and §11.1.11: 6985 when the PIN is already
            // in the requested state.
            Err(e) if e.sw() == Some(SW_CONDITIONS_NOT_SATISFIED) => {
                println!("PIN1 was already {want}")
            }
            Err(e) => return Err(e.into()),
            Ok(()) => {
                let note = if disable {
                    "the handset does not prompt at boot"
                } else {
                    "the handset prompts at boot"
                };
                println!("PIN1 {want} ({note})");
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let new = new.ok_or("give --new, --disable or --enable")?;
    if which == "adm1" {
        let old = match current {
            Some(current) => adm1_bytes(current)?,
            None => adm1(args)?,
        };
        let new_value = adm1_bytes(new)?;
        let mut card = connect(args, false, false, false)?;
        card.change_pin(KEY_REF_ADM1, &old, &new_value)?;
        println!("ADM1 changed (do not lose the new value)");
        return Ok(ExitCode::SUCCESS);
    }
    let key_ref = if which.ends_with('1') {
        KEY_REF_PIN1
    } else {
        KEY_REF_PIN2
    };
    let is_puk = which.starts_with("puk");
    let value = if is_puk {
        puk_bytes(new)?
    } else {
        new_pin_bytes(new)?
    };
    let mut card = connect(args, true, false, false)?;
    if is_puk {
        card.set_puk(key_ref, &value)?;
    } else {
        card.set_pin(key_ref, &value)?;
    }
    println!("{} set to {new:?}", which.to_uppercase());
    Ok(ExitCode::SUCCESS)
}

pub fn unblock(args: &CardArgs, which: &str, puk: &str, new_pin: Option<&str>) -> Result<ExitCode> {
    let mut card = connect(args, false, false, false)?;
    let key_ref = if which == "pin2" {
        KEY_REF_PIN2
    } else {
        KEY_REF_PIN1
    };
    let new_pin_bytes = new_pin.map(pin_bytes).transpose()?;
    card.unblock_pin(key_ref, &pin_bytes(puk)?, new_pin_bytes.as_deref())?;
    match new_pin {
        Some(pin) => println!("{} unblocked and changed to {pin:?}", which.to_uppercase()),
        None => println!("{} unblocked (PIN value unchanged)", which.to_uppercase()),
    }
    Ok(ExitCode::SUCCESS)
}

/// The card computes the checksum over its stored A-key and the ESN. The
/// network computes the same digits from its copy, so a match confirms
/// both hold the same key.
pub fn akey_checksum(args: &CardArgs, esn: &str) -> Result<ExitCode> {
    let esn: [u8; 4] = parse_hex(esn)?
        .try_into()
        .map_err(|_| "--esn must be 8 hex digits")?;
    let mut card = connect(args, true, true, false)?;
    let digits = card.a_key_checksum(esn)?;
    println!(
        "A-key checksum (ESN {}): {}",
        hex_upper(&esn),
        String::from_utf8_lossy(&digits)
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::relative_to;
    use std::path::Path;

    #[test]
    fn relative_paths() {
        let file = Path::new("/r/crates/x/f.prl");
        assert_eq!(
            relative_to(file, Path::new("/r/sim/examples")),
            "../../crates/x/f.prl"
        );
        assert_eq!(relative_to(file, Path::new("/r/crates")), "x/f.prl");
        assert_eq!(
            relative_to(file, Path::new("/tmp/out")),
            "/r/crates/x/f.prl"
        );
    }
}
