//! `csim`: personalize, read, dump and compare CSIM cards, and manage the
//! 1xBTS CSIM applet on a sysmoISIM-SJA5.

mod benchmark;
mod commands;
mod install;
mod pcsc_transport;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

/// The applet's factory values.
pub const DEFAULT_PIN1: &str = "1234";
pub const DEFAULT_PIN2: &str = "2222";

#[derive(Parser)]
#[command(
    name = "csim",
    about = "Personalize, read, dump and compare CSIM cards"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// How to reach the card and authenticate.
#[derive(Args, Clone)]
pub struct CardArgs {
    /// Reader name substring, or `sim` for the applet in jcardsim.
    #[arg(long, env = "CSIM_READER")]
    pub reader: Option<String>,
    /// The applet's ADM1, 8 digits. `csim install` sets it to the platform
    /// ADM1. Ignored for `--reader sim`, which uses the factory ADM1.
    #[arg(long, env = "PLATFORM_ADM1", hide_env_values = true)]
    pub adm1: Option<String>,
    /// PIN1 as ASCII digits.
    #[arg(long, default_value = DEFAULT_PIN1)]
    pub pin1: String,
    /// PIN2 as ASCII digits.
    #[arg(long, default_value = DEFAULT_PIN2)]
    pub pin2: String,
    /// Print every APDU exchanged.
    #[arg(long)]
    pub trace: bool,
}

#[derive(Args, Clone)]
pub struct ReaderArgs {
    /// Reader name substring, or `sim` for the applet in jcardsim.
    #[arg(long, env = "CSIM_READER")]
    pub reader: Option<String>,
    /// Print every APDU exchanged.
    #[arg(long)]
    pub trace: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Time reader exchanges and authentication operations using existing card secrets.
    Bench(benchmark::BenchArgs),
    /// Show the ATR, whether the CSIM is present and the EFs readable
    /// without a PIN.
    Info(ReaderArgs),
    /// Check a profile without a card.
    Validate { profile: PathBuf },
    /// Fill a profile template for one subscriber, with a new A-key and
    /// new IP secrets.
    Profile {
        template: PathBuf,
        /// The 15-digit IMSI.
        #[arg(long)]
        imsi: String,
        /// Default: the MIN, the last 10 digits of the IMSI.
        #[arg(long)]
        mdn: Option<String>,
        /// Default: card-<IMSI>.local.yaml next to the template.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Replace an existing output file.
        #[arg(long)]
        force: bool,
    },
    /// Load the EF table, write a profile and disable PIN1.
    Write {
        #[command(flatten)]
        card: CardArgs,
        profile: PathBuf,
        /// Check the profile without writing.
        #[arg(long)]
        validate_only: bool,
        /// Write even when the profile has problems.
        #[arg(long)]
        force: bool,
        /// Leave PIN1 enabled.
        #[arg(long)]
        keep_pin1: bool,
    },
    /// Write a profile, read it back and compare.
    Verify {
        #[command(flatten)]
        card: CardArgs,
        profile: PathBuf,
        #[arg(long)]
        force: bool,
        /// Also dump the card to this file and check every written byte.
        #[arg(long)]
        save_dump: Option<PathBuf>,
    },
    /// Read and decode the EFs a profile writes.
    Read {
        #[command(flatten)]
        card: CardArgs,
        /// Save what was read as a profile.
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Record everything a CSIM exposes, on any vendor's card.
    Dump {
        #[command(flatten)]
        reader: ReaderArgs,
        /// PIN1, without which PIN-protected EFs show read SW=6982.
        #[arg(long)]
        pin1: Option<String>,
        #[arg(long)]
        pin2: Option<String>,
        /// The applet's ADM1, 8 digits.
        #[arg(long, conflicts_with = "adm1_hex")]
        adm1: Option<String>,
        /// ADM1 as 16 hex digits, for another vendor's card.
        #[arg(long)]
        adm1_hex: Option<String>,
        /// Also try every FID 6F00-6FFF under the ADF and 2F00-2FFF at
        /// the MF.
        #[arg(long)]
        scan: bool,
        /// Load the EF table first, for a fresh applet or `--reader sim`.
        #[arg(long)]
        init_meta: bool,
        /// Print each EF as it is read.
        #[arg(long)]
        verbose: bool,
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Compare two dumps, a dump with the EF table, or a dump with the
    /// bytes a profile writes.
    Compare {
        a: PathBuf,
        b: Option<PathBuf>,
        /// Compare `a` with the EF table.
        #[arg(long)]
        table: bool,
        /// Also report EFs whose content differs (two dumps).
        #[arg(long)]
        content: bool,
        /// Check every byte this profile writes against the dump.
        #[arg(long)]
        profile: Option<PathBuf>,
    },
    /// Load the EF table into a freshly installed applet.
    InitMeta(CardArgs),
    /// Read the EF table back from the applet and compare.
    VerifyMeta(CardArgs),
    /// Register the CSIM in the card's EF_DIR.
    RegisterDir {
        #[command(flatten)]
        reader: ReaderArgs,
        /// The platform ADM1 from the key material, 8 digits.
        #[arg(long, env = "PLATFORM_ADM1", hide_env_values = true)]
        platform_adm1: String,
        /// Put the CSIM in record 1 and move the other applications down.
        #[arg(long)]
        first: bool,
    },
    /// Print the applet's record of the last 32 commands.
    Apdulog {
        #[command(flatten)]
        card: CardArgs,
        /// Empty the record instead.
        #[arg(long)]
        clear: bool,
    },
    /// Print the Java Card version, commit capacity, free memory and APDU
    /// buffer size the applet sees.
    PlatformInfo(CardArgs),
    /// Set PIN1, PIN2, PUK1 or PUK2 with ADM1, change ADM1, or enable or
    /// disable PIN1.
    Chpin {
        #[command(flatten)]
        card: CardArgs,
        #[arg(value_parser = ["pin1", "pin2", "puk1", "puk2", "adm1"])]
        which: String,
        /// The current ADM1, or PIN1 for --enable and --disable. Default:
        /// --adm1 or --pin1.
        #[arg(long)]
        current: Option<String>,
        #[arg(long, conflicts_with_all = ["disable", "enable"])]
        new: Option<String>,
        #[arg(long, conflicts_with = "enable")]
        disable: bool,
        #[arg(long)]
        enable: bool,
    },
    /// Reset a PIN's retry counter with its PUK.
    Unblock {
        #[command(flatten)]
        card: CardArgs,
        #[arg(value_parser = ["pin1", "pin2"], default_value = "pin1")]
        which: String,
        /// The PUK (factory values: PUK1 12345678, PUK2 87654321).
        #[arg(long)]
        puk: String,
        /// Also set a new PIN.
        #[arg(long)]
        new_pin: Option<String>,
    },
    /// Print the 6-digit checksum of the stored A-key over an ESN.
    AkeyChecksum {
        #[command(flatten)]
        card: CardArgs,
        /// The ESN as 8 hex digits.
        #[arg(long)]
        esn: String,
    },
    /// Install the applet with gp.jar, register it first in the card's
    /// EF_DIR, load the EF table and disable PIN1.
    Install(install::InstallArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Bench(args) => benchmark::run(args),
        Command::Info(r) => commands::info(&r),
        Command::Validate { profile } => commands::validate(&profile),
        Command::Profile {
            template,
            imsi,
            mdn,
            output,
            force,
        } => commands::profile(&template, &imsi, mdn.as_deref(), output.as_deref(), force),
        Command::Write {
            card,
            profile,
            validate_only,
            force,
            keep_pin1,
        } => commands::write(
            card,
            &profile,
            commands::WriteOptions {
                validate_only,
                force,
                keep_pin1,
            },
        ),
        Command::Verify {
            card,
            profile,
            force,
            save_dump,
        } => commands::verify(card, &profile, force, save_dump.as_deref()),
        Command::Read { card, save } => commands::read(&card, save.as_deref()),
        Command::Dump {
            reader,
            pin1,
            pin2,
            adm1,
            adm1_hex,
            scan,
            init_meta,
            verbose,
            save,
        } => {
            let creds = commands::DumpCredentials {
                pin1,
                pin2,
                adm1,
                adm1_hex,
            };
            commands::dump(&reader, creds, scan, init_meta, verbose, save.as_deref())
        }
        Command::Compare {
            a,
            b,
            table,
            content,
            profile,
        } => commands::compare(&a, b.as_deref(), table, content, profile.as_deref()),
        Command::InitMeta(card) => commands::init_meta(&card),
        Command::VerifyMeta(card) => commands::verify_meta(&card),
        Command::RegisterDir {
            reader,
            platform_adm1,
            first,
        } => commands::register_dir(&reader, &platform_adm1, first),
        Command::Apdulog { card, clear } => commands::apdulog(&card, clear),
        Command::PlatformInfo(card) => commands::platform_info(&card),
        Command::Chpin {
            card,
            which,
            current,
            new,
            disable,
            enable,
        } => commands::chpin(
            &card,
            &which,
            current.as_deref(),
            new.as_deref(),
            disable,
            enable,
        ),
        Command::Unblock {
            card,
            which,
            puk,
            new_pin,
        } => commands::unblock(&card, &which, &puk, new_pin.as_deref()),
        Command::AkeyChecksum { card, esn } => commands::akey_checksum(&card, &esn),
        Command::Install(args) => install::run(args),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
