//! Network-in-a-box launcher.
//!
//! Builds every component and runs each as a child process, relaying their
//! output to one terminal. Every component retries its peers, so they start
//! without ordering constraints and reach service as their dependencies
//! appear.
//!
//! A child is this same binary re-executed with `--run-component`, so one
//! `cargo run -p cdma-nib` produces the whole network, radio backends
//! included. Each child calls the same `run_node` a deployment runs, which
//! keeps one wiring path rather than two.

use std::{path::PathBuf, process::Stdio, time::Duration};

use clap::Parser;
use log::{error, info};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    signal,
    sync::mpsc,
};

/// How long a child gets to exit on its own after SIGTERM before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// How often the launcher checks whether a component has exited.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Components, in the order they are started. Ordering is presentational only
/// — each component retries until its peers answer.
const COMPONENTS: &[&str] = &[
    "cdma-hlr",
    "cdma-smsc",
    "cdma-events",
    "cdma-pdsn",
    "cdma-pcf",
    "cdma-msc",
    "cdma-bts",
    "cdma-bsc",
];

/// Component whose configuration lives in `bts.json` and which therefore
/// accepts the radio and profile overrides.
const RADIO_COMPONENT: &str = "cdma-bts";
/// Component that accepts the A1 and management address overrides.
const MSC_COMPONENT: &str = "cdma-msc";

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "1xBTS network-in-a-box: runs every component as a child process."
)]
struct Cli {
    /// Directory containing per-node config files.
    #[arg(long, value_name = "DIR")]
    config_dir: Option<PathBuf>,

    /// Path to a radio-only config JSON. Overrides the radio config referenced by `bts.json`.
    #[arg(long, value_name = "CONFIG")]
    radio_config: Option<PathBuf>,

    /// Path to a BTS config JSON. Overrides `<config-dir>/bts.json`.
    #[arg(long, value_name = "CONFIG")]
    bts_config: Option<PathBuf>,

    /// Named BTS profile applied after the base config and its local override.
    #[arg(long, value_name = "PROFILE")]
    bts_profile: Option<String>,

    /// Use a null radio that drops all TX samples and provides no RX.
    #[arg(long)]
    null_radio: bool,

    /// MSC management gRPC listen address.
    #[arg(long, value_name = "ADDR")]
    msc_mgmt_addr: Option<String>,

    /// Run only these components, repeatable. Defaults to all of them.
    #[arg(long = "component", value_name = "NAME")]
    components: Vec<String>,

    /// Run one component in this process instead of supervising. Set by the
    /// launcher on the children it starts.
    #[arg(long, value_name = "NAME", hide = true)]
    run_component: Option<String>,
}

/// A running component and the name it is known by.
struct Running {
    name: &'static str,
    child: Child,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    cdma_common::startup::init_logging();

    if let Some(component) = cli.run_component.clone() {
        return tokio::runtime::Runtime::new()?
            .block_on(run_component(&component, &cli))
            .map_err(|error| -> Box<dyn std::error::Error> { error.to_string().into() });
    }

    let selected: Vec<&'static str> = if cli.components.is_empty() {
        COMPONENTS.to_vec()
    } else {
        let mut chosen = Vec::new();
        for requested in &cli.components {
            match COMPONENTS.iter().find(|name| *name == requested) {
                Some(name) => chosen.push(*name),
                None => {
                    return Err(format!(
                        "unknown component {requested:?}; known components are {}",
                        COMPONENTS.join(", ")
                    )
                    .into());
                }
            }
        }
        chosen
    };

    tokio::runtime::Runtime::new()?.block_on(supervise(cli, selected))
}

async fn supervise(
    cli: Cli,
    selected: Vec<&'static str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Children write their own formatted lines, module path included, so they
    // are relayed verbatim rather than re-emitted through this process's
    // subscriber, which would stamp every line a second time.
    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<String>();

    let mut running = Vec::new();
    for name in &selected {
        let mut child = spawn_component(name, &cli)?;
        forward_output(&mut child, line_tx.clone());
        info!("starting {name}");
        running.push(Running { name, child });
    }
    drop(line_tx);

    let mut shutting_down = false;
    let mut failed: Option<String> = None;
    // One signal stream for the whole run. Re-creating it per iteration
    // drops a signal that lands between the drop and the re-subscribe.
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            Some(line) = line_rx.recv() => println!("{line}"),
            // Always completes, so the branch stays live and keeps polling. A
            // matching pattern here would disable it until some other branch
            // fired, which never happens when the components are quiet.
            exited = next_exit(&mut running), if !running.is_empty() => {
                if let Some((name, status)) = exited {
                    error!("{name} exited: {status}");
                    if !shutting_down {
                        error!("a component exited, stopping the rest");
                        failed = Some(format!("{name} exited: {status}"));
                        shutting_down = true;
                        terminate(&mut running).await;
                    }
                }
            }
            _ = &mut shutdown, if !shutting_down => {
                info!("shutting down");
                shutting_down = true;
                terminate(&mut running).await;
            }
            else => break,
        }
        if shutting_down && running.is_empty() {
            // Relay whatever the children wrote before they exited.
            while let Ok(line) = line_rx.try_recv() {
                println!("{line}");
            }
            break;
        }
    }

    match failed {
        Some(reason) => Err(reason.into()),
        None => Ok(()),
    }
}

/// Run one component in this process. Each arm calls the same `run_node` the
/// component's own binary calls, so the launcher and a deployment share one
/// startup path.
async fn run_component(
    name: &str,
    cli: &Cli,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_dir = cdma_common::startup::resolve_config_dir(cli.config_dir.clone());
    match name {
        "cdma-hlr" => cdma_hlr::run_node(&config_dir).await,
        "cdma-smsc" => cdma_smsc::run_node(&config_dir).await,
        "cdma-events" => cdma_events::run_node(&config_dir).await,
        "cdma-pdsn" => cdma_pdsn::run_node(&config_dir).await,
        "cdma-pcf" => cdma_pcf::run_node(&config_dir).await,
        "cdma-msc" => {
            let overrides = cdma_msc::MscCliOverrides {
                mgmt_addr: cli
                    .msc_mgmt_addr
                    .as_deref()
                    .map(str::parse)
                    .transpose()
                    .map_err(|e| format!("invalid --msc-mgmt-addr: {e}"))?,
            };
            cdma_msc::run_node(&config_dir, overrides).await
        }
        "cdma-bts" => {
            cdma_bts::run_node(
                &config_dir,
                cdma_bts::BtsCliOverrides {
                    radio_config: cli.radio_config.clone(),
                    bts_config: cli.bts_config.clone(),
                    bts_profile: cli.bts_profile.clone(),
                    null_radio: cli.null_radio,
                },
            )
            .await
        }
        "cdma-bsc" => cdma_bsc::run_node(&config_dir).await,
        other => Err(format!("unknown component {other:?}").into()),
    }
}

/// Start one component as a child of this process, by re-executing this binary
/// in component mode. Children inherit the environment, so `RUST_LOG` and
/// friends reach every component unchanged.
fn spawn_component(name: &str, cli: &Cli) -> Result<Child, std::io::Error> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("--run-component").arg(name);
    if let Some(dir) = &cli.config_dir {
        command.arg("--config-dir").arg(dir);
    }
    if name == RADIO_COMPONENT {
        if let Some(path) = &cli.radio_config {
            command.arg("--radio-config").arg(path);
        }
        if let Some(path) = &cli.bts_config {
            command.arg("--bts-config").arg(path);
        }
        if let Some(profile) = &cli.bts_profile {
            command.arg("--bts-profile").arg(profile);
        }
        if cli.null_radio {
            command.arg("--null-radio");
        }
    }
    if name == MSC_COMPONENT {
        if let Some(addr) = &cli.msc_mgmt_addr {
            command.arg("--msc-mgmt-addr").arg(addr);
        }
    }
    // Children must not share the terminal's process group, or a Ctrl-C
    // reaches them directly and races the launcher's own shutdown.
    #[cfg(unix)]
    command.process_group(0);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        // Covers a panic or an early return here. A SIGKILL of this process
        // still leaves children behind, since nothing runs to reap them.
        .kill_on_drop(true);
    command.spawn()
}

/// Relay a child's stdout and stderr into the shared line stream.
fn forward_output(child: &mut Child, tx: mpsc::UnboundedSender<String>) {
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(text)) = lines.next_line().await {
                if tx.send(text).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(text)) = lines.next_line().await {
                if tx.send(text).is_err() {
                    break;
                }
            }
        });
    }
}

/// Yield the first component found to have exited, removing it from the set.
/// Returns `None` after a short wait when everything is still up, which leaves
/// the caller's `select!` free to relay output in between.
async fn next_exit(running: &mut Vec<Running>) -> Option<(&'static str, std::process::ExitStatus)> {
    let mut exited = None;
    for (index, entry) in running.iter_mut().enumerate() {
        if let Ok(Some(status)) = entry.child.try_wait() {
            exited = Some((index, status));
            break;
        }
    }
    if let Some((index, status)) = exited {
        let entry = running.remove(index);
        return Some((entry.name, status));
    }
    tokio::time::sleep(EXIT_POLL_INTERVAL).await;
    None
}

/// Ask every component to stop, then kill whatever is still up after the grace
/// period. Components are stopped in reverse start order so the radio outlives
/// the things that talk to it.
async fn terminate(running: &mut Vec<Running>) {
    for entry in running.iter_mut().rev() {
        if let Some(pid) = entry.child.id() {
            // SIGTERM rather than kill() so each component runs its own shutdown.
            // SAFETY: kill(2) has no memory-safety preconditions. The pid came
            // from Child::id() on a child this process still owns.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
    }
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
    while !running.is_empty() && tokio::time::Instant::now() < deadline {
        running.retain_mut(|entry| !matches!(entry.child.try_wait(), Ok(Some(_))));
        tokio::time::sleep(EXIT_POLL_INTERVAL).await;
    }
    for entry in running.iter_mut() {
        let _ = entry.child.start_kill();
    }
    running.clear();
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal as unix_signal};
    let mut term = match unix_signal(SignalKind::terminate()) {
        Ok(stream) => stream,
        Err(_) => {
            let _ = signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = signal::ctrl_c().await;
}
