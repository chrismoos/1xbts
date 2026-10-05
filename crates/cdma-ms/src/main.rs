mod cli_client;
mod cli_radio;
mod console;
#[cfg(feature = "gui")]
mod gui;
mod tui;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};

use cdma_ms::grpc::proto::ScanMode;

#[derive(Parser, Debug)]
#[command(name = "cdma-ms", about = "CDMA2000 1x Mobile Station")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the mobile station daemon with its management gRPC server.
    Serve(ServeArgs),
    /// Drive a station interactively. Every command is a gRPC call.
    Console(console::ConsoleArgs),
    /// Scan the channels a PRL lists, acquire a pilot, decode sync, camp on
    /// the first permitted system, and report what it found.
    Scan(ScanArgs),
    /// Visit every channel a PRL lists and report each pilot, without
    /// camping.
    Sweep(ScanArgs),
    /// Interactive Dracula-themed terminal UI: command line plus live signal,
    /// serving-cell and paging panels.
    Tui(tui::TuiArgs),
    /// Open the feature-phone GUI with microphone and speaker audio.
    #[cfg(feature = "gui")]
    Gui(gui::GuiArgs),
}

#[derive(Args, Debug)]
struct ServeArgs {
    /// Path to the MS node config.
    #[arg(long, default_value = "config/ms.json")]
    config: PathBuf,

    /// Radio to use: `sim`, `noop`, or a radio JSON file. Overrides the
    /// config's radio.
    #[arg(long)]
    radio: Option<String>,

    /// PRL file. When given, power-on scans its channels instead of assuming
    /// the radio is already on a channel.
    #[arg(long)]
    prl: Option<PathBuf>,

    /// Override the gRPC listen address from the config.
    #[arg(long)]
    grpc_listen: Option<String>,

    /// Override the mobile ESN (decimal or 0x-prefixed hex).
    #[arg(long, value_parser = parse_u32_auto)]
    esn: Option<u32>,

    /// Override the provisioned 15-digit IMSI.
    #[arg(long, value_parser = parse_imsi)]
    imsi: Option<String>,

    /// Do not power on the MS at startup (wait for a PowerOn RPC).
    #[arg(long)]
    no_auto_power_on: bool,
}

#[derive(Args, Debug)]
struct ScanArgs {
    /// Address of a running `cdma-ms serve` (host:port). Without it the
    /// scan runs the daemon itself. The PRL path is read on the daemon's host.
    #[arg(long)]
    connect: Option<String>,

    /// Path to the MS node config.
    #[arg(long, default_value = "config/ms.json")]
    config: PathBuf,

    /// PRL file whose acquisition table drives the scan.
    #[arg(long)]
    prl: PathBuf,

    /// Radio to use: `sim`, `noop`, or a radio JSON file. Overrides the
    /// config's radio.
    #[arg(long)]
    radio: Option<String>,

    /// Scan only these band classes (bc0, bc1, ...). Repeatable.
    #[arg(long = "band-class")]
    band_class: Vec<String>,

    /// Time on a channel with no pilot before moving on, in ms.
    #[arg(long)]
    dwell_ms: Option<u32>,

    /// Keep listening this long after camping, to show the paging channel.
    #[arg(long, default_value_t = 5)]
    linger_secs: u64,

    /// Print every event, including per-second measurements and each paging
    /// message.
    #[arg(long)]
    verbose: bool,

    /// Override the mobile ESN (decimal or 0x-prefixed hex).
    #[arg(long, value_parser = parse_u32_auto)]
    esn: Option<u32>,

    /// Override the provisioned 15-digit IMSI.
    #[arg(long, value_parser = parse_imsi)]
    imsi: Option<String>,
}

pub(crate) fn parse_u32_auto(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let parsed = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
    } else {
        s.parse::<u32>()
    };
    parsed.map_err(|e| format!("invalid ESN {s:?}: {e}"))
}

pub(crate) fn parse_imsi(value: &str) -> Result<String, String> {
    if value.len() == 15 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(value.to_string())
    } else {
        Err("IMSI must contain exactly 15 decimal digits".to_string())
    }
}

const SCAN_DEADLINE: Duration = Duration::from_secs(20 * 60);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let command = Cli::parse().command;
    if !matches!(command, Command::Tui(_)) {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
    }

    match command {
        Command::Serve(args) => serve(args).await,
        Command::Console(args) => console::run(args).await,
        Command::Scan(args) => scan(args, ScanMode::Camp).await,
        Command::Sweep(args) => scan(args, ScanMode::Survey).await,
        Command::Tui(args) => tui::run(args).await,
        #[cfg(feature = "gui")]
        Command::Gui(args) => gui::run(args).await,
    }
}

async fn serve(args: ServeArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use cdma_ms::config::MsNodeConfig;
    use cdma_ms::grpc::run_grpc_server;

    let mut config = MsNodeConfig::load(&args.config)?;
    if let Some(esn) = args.esn {
        config.identity.esn = esn;
    }
    if let Some(imsi) = args.imsi {
        config.identity.imsi = imsi;
    }
    if let Some(listen) = args.grpc_listen {
        config.grpc_listen = listen;
    }
    let addr = config.grpc_listen.parse()?;
    let service = cli_client::build_service(&config, args.radio.as_deref(), args.prl.as_deref())?;
    if !args.no_auto_power_on {
        service.power_on();
    }
    tracing::info!(%addr, "cdma-ms management gRPC listening");
    run_grpc_server(addr, service).await
}

async fn scan(
    args: ScanArgs,
    mode: ScanMode,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use cdma_ms::config::MsNodeConfig;
    use cdma_ms::grpc::proto::StartScanRequest;
    use cdma_ms::ms::MsEvent;
    use tokio_stream::StreamExt;

    use cli_client::{Session, decode_event};
    use cli_radio::show;

    let mut config = MsNodeConfig::load(&args.config)?;
    if let Some(esn) = args.esn {
        config.identity.esn = esn;
    }
    if let Some(imsi) = args.imsi {
        config.identity.imsi = imsi;
    }
    let session = Session::open(
        args.connect.as_deref(),
        &config,
        args.radio.as_deref(),
        None,
    )
    .await?;
    let mut client = session.client.clone();
    let mut events = client.stream_events(()).await?.into_inner();

    let started = client
        .start_scan(StartScanRequest {
            prl_path: args.prl.display().to_string(),
            mode: mode.into(),
            band_classes: args.band_class.clone(),
            dwell_ms: args.dwell_ms.unwrap_or(0),
            channels: Vec::new(),
        })
        .await?
        .into_inner();
    println!("{}", started.summary);

    let mut result: Option<String> = None;
    let deadline = Instant::now() + SCAN_DEADLINE;
    while result.is_none() && Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_secs(1), events.next()).await;
        let Ok(Some(Ok(ev))) = next else {
            if matches!(next, Ok(None)) {
                break;
            }
            continue;
        };
        let Some(typed) = decode_event(&ev) else {
            continue;
        };
        if cli_radio::is_chatty(&typed) && !args.verbose {
            continue;
        }
        println!("{}", cli_radio::event_line(&typed));
        if let MsEvent::ScanFinished { result: r, .. } = &typed {
            result = Some(r.clone());
        }
    }

    let camped = result.as_deref() == Some("camped");
    if camped && args.linger_secs > 0 {
        println!("camped, listening for {} s", args.linger_secs);
        let until = Instant::now() + Duration::from_secs(args.linger_secs);
        while Instant::now() < until {
            if let Ok(Some(Ok(ev))) =
                tokio::time::timeout(Duration::from_millis(500), events.next()).await
            {
                if let Some(typed) = decode_event(&ev) {
                    if !cli_radio::is_chatty(&typed) || args.verbose {
                        println!("{}", cli_radio::event_line(&typed));
                    }
                }
            }
        }
    }

    let d = client.get_diagnostics(()).await?.into_inner();
    println!();
    if let Some(scan) = &d.scan {
        println!("{}", show::scan_table(scan));
    }
    if let Some(sync) = &d.sync {
        println!();
        println!("{}", show::sync(sync));
    }
    if camped {
        if let Some(pilot) = &d.pilot {
            println!();
            println!("{}", show::pilot(pilot));
        }
        if let Some(overhead) = &d.overhead {
            println!();
            println!("{}", show::overhead(overhead, None));
            if !overhead.system_parameters.is_empty() {
                println!("{}", show::overhead(overhead, Some("spm")));
            }
            if !overhead.access_parameters.is_empty() {
                println!("{}", show::overhead(overhead, Some("apm")));
            }
        }
        if let Some(paging) = &d.paging {
            println!();
            println!("{}", show::paging(paging));
        }
    }
    if let Some(radio) = &d.radio {
        println!();
        println!("{}", show::radio(radio));
    }
    println!(
        "result: {}",
        result.as_deref().unwrap_or("no result before the deadline")
    );

    let ok = match mode {
        ScanMode::Survey => d
            .scan
            .as_ref()
            .map(|s| s.channels.iter().any(|c| c.pilot == "found"))
            .unwrap_or(false),
        _ => camped,
    };
    std::process::exit(if ok { 0 } else { 1 });
}
