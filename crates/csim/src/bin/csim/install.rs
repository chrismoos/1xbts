//! First-time setup of a sysmoISIM-SJA5: install the applet and make the
//! card usable by a handset.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use clap::Args;
use csim::card::{FACTORY_ADM1, KEY_REF_ADM1, adm1_bytes};
use csim::ef_dir::{self, Placement};

use crate::DEFAULT_PIN1;
use crate::commands::{self, Result};

#[derive(Args)]
pub struct InstallArgs {
    /// The platform ADM1 from the key material, 8 digits. The applet's
    /// ADM1 is set to the same value.
    #[arg(long, env = "PLATFORM_ADM1", hide_env_values = true)]
    pub platform_adm1: String,
    /// Path to gp.jar.
    #[arg(long, env = "GP_JAR", default_value = "gp.jar")]
    pub gp_jar: PathBuf,
    /// GlobalPlatform ENC key from the key material (KIC1).
    #[arg(long, env = "KIC1")]
    pub kic1: String,
    /// GlobalPlatform MAC key from the key material (KID1).
    #[arg(long, env = "KID1")]
    pub kid1: String,
    /// GlobalPlatform DEK key from the key material (KIK1).
    #[arg(long, env = "KIK1")]
    pub kik1: String,
    /// GlobalPlatform keyset version. The sysmoISIM-SJA5 uses 0x70.
    #[arg(long, env = "GP_KEY_VER", default_value = "0x70")]
    pub key_ver: String,
    /// gp.jar reader (`-r`). Default: the reader csim picks from --reader.
    #[arg(long, env = "GP_READER")]
    pub gp_reader: Option<String>,
    /// The CAP to install. Default: the bundled sim/applet/dist/csim.cap.
    /// A local build is sim/applet/build/csim.cap.
    #[arg(long)]
    pub cap: Option<PathBuf>,
    /// Reader name substring.
    #[arg(long, env = "CSIM_READER")]
    pub reader: Option<String>,
    /// The applet's PIN1.
    #[arg(long, default_value = DEFAULT_PIN1)]
    pub pin1: String,
    /// Leave PIN1 enabled.
    #[arg(long)]
    pub keep_pin1: bool,
    /// Print every APDU exchanged.
    #[arg(long)]
    pub trace: bool,
}

const BUNDLED_CAP: &str = "dist/csim.cap";
/// The first bytes of a Git LFS pointer file, which a checkout without
/// git-lfs leaves in place of the CAP.
const LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs";

/// Fails before gp.jar runs when the CAP is missing or is an LFS pointer.
fn check_cap(cap: &std::path::Path) -> Result<()> {
    let head = std::fs::read(cap).map_err(|e| format!("{}: {e}", cap.display()))?;
    if head.starts_with(LFS_POINTER_PREFIX) {
        return Err(format!(
            "{} is a Git LFS pointer, not the CAP. Run `git lfs pull` in the checkout",
            cap.display()
        )
        .into());
    }
    Ok(())
}

fn banner(text: &str) {
    println!("\n==> {text}");
}

/// Loads and installs the CAP with gp.jar. --force deletes an installed
/// copy first, which erases the applet's EF table, profile and PIN
/// state, and is a no-op on a card that never had it.
fn gp_install(a: &InstallArgs) -> Result<()> {
    let cap = a
        .cap
        .clone()
        .unwrap_or_else(|| csim::sim::applet_dir().join(BUNDLED_CAP));
    check_cap(&cap)?;
    if !a.gp_jar.is_file() {
        return Err(format!(
            "{} not found. Download gp.jar, or give its path with --gp-jar or GP_JAR",
            a.gp_jar.display()
        )
        .into());
    }
    banner(&format!("installing {}", cap.display()));
    let mut gp = Command::new("java");
    gp.arg("-jar").arg(&a.gp_jar);
    // gp.jar on its own takes the first reader with a card, which can be a
    // security key instead of the card reader.
    let reader = match &a.gp_reader {
        Some(r) => r.clone(),
        None => crate::pcsc_transport::reader_name(a.reader.as_deref())?,
    };
    println!("reader: {reader}");
    gp.args(["-r", &reader]);
    gp.args(["--key-ver", &a.key_ver])
        .args(["--key-enc", &a.kic1])
        .args(["--key-mac", &a.kid1])
        .args(["--key-dek", &a.kik1])
        .arg("--force")
        .arg("--install")
        .arg(&cap)
        .arg("--list");
    let status = gp
        .status()
        .map_err(|e| format!("running java -jar {}: {e}", a.gp_jar.display()))?;
    if !status.success() {
        return Err("gp.jar install failed".into());
    }
    Ok(())
}

/// Installs the applet, registers the CSIM as the first application in
/// the card's EF_DIR, loads the EF table, sets the applet's ADM1 to the
/// platform ADM1 and disables PIN1. The EF_DIR step writes nothing when
/// the CSIM is already first.
pub fn run(a: InstallArgs) -> Result<ExitCode> {
    let platform_adm1 = adm1_bytes(&a.platform_adm1)?;
    gp_install(&a)?;

    let mut card = commands::open(a.reader.as_deref(), a.trace)?;
    banner("registering the CSIM in EF_DIR");
    let reg = ef_dir::register(&mut card, &platform_adm1, Placement::First)?;
    let state = if reg.changed {
        "registered"
    } else {
        "already registered"
    };
    println!("EF_DIR: CSIM {state} in record #{}", reg.record);
    for (i, (aid, label)) in reg.applications.iter().enumerate() {
        println!("  #{} {aid} {label:?}", i + 1);
    }

    banner("EF table");
    card.select_csim()?;
    // gp.jar --force reinstalls the applet, so it has the factory ADM1.
    card.verify(KEY_REF_ADM1, &FACTORY_ADM1, true)?;
    commands::ensure_ef_table(&mut card)?;

    banner("ADM1");
    card.change_pin(KEY_REF_ADM1, &FACTORY_ADM1, &platform_adm1)?;
    println!("ADM1: set to the platform ADM1");

    if !a.keep_pin1 {
        banner("PIN1");
        commands::disable_pin1(&mut card, &a.pin1);
    }
    println!("\ninstalled. Write a profile with `csim write <profile>`.");
    Ok(ExitCode::SUCCESS)
}
