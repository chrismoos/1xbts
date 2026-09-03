//! Process bootstrap shared by the network-component binaries.
//!
//! Config-directory resolution and log-filter policy are identical whether a
//! component runs on its own or inside the all-in-one launcher, so every
//! entry point reads them from here.

use std::path::PathBuf;

use tracing_subscriber::{EnvFilter, prelude::*, util::SubscriberInitExt};

/// Directory the per-node config files are read from when neither
/// `--config-dir` nor `CDMA_CONFIG_DIR` names one.
pub const DEFAULT_CONFIG_DIR: &str = "config";
/// Environment variable that names the config directory.
pub const CONFIG_DIR_ENV: &str = "CDMA_CONFIG_DIR";

/// Log level applied to targets with no more specific directive.
pub const DEFAULT_LOG_FILTER: &str = "info";
/// Third-party targets clamped to `warn` unless the operator names them.
const DEFAULT_LOG_CLAMPS: &[&str] = &[
    "sqlx=warn",
    "h2=warn",
    "hyper=warn",
    "hyper_util=warn",
    "tower=warn",
    "tonic=warn",
];
/// Targets a bare `RUST_LOG=debug` expands to. A global `debug` floods the
/// per-chip receiver paths, so it selects the protocol and session targets an
/// operator actually wants instead.
const DEFAULT_GLOBAL_DEBUG_PROFILE: &[&str] = &[
    DEFAULT_LOG_FILTER,
    "cdma_packet=debug",
    "cdma_an=debug",
    "cdma_bsc::bsc::packet=debug",
    "cdma_bsc::bsc::traffic_forward=debug",
    "cdma_bsc::bsc::traffic_signaling=debug",
    "cdma_bsc::bsc::access=debug",
    "cdma_bts::bts::abis_agent=debug",
    "cdma_bts::bts::evdo=debug",
    "cdma_bts::bts::hrpd=debug",
    "cdma_bts::receiver::hrpd::reverse_traffic_rake=debug",
    "cdma_abis::transport=debug",
    "cdma_pcf=debug",
    "cdma_pdsn=debug",
];

/// Resolve the config directory: the explicit CLI value, else
/// `CDMA_CONFIG_DIR`, else `config`.
pub fn resolve_config_dir(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        return PathBuf::from(dir);
    }
    PathBuf::from(DEFAULT_CONFIG_DIR)
}

/// The `RUST_LOG` filter with the default profile and clamps applied.
pub fn effective_log_filter() -> String {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_string());
    apply_default_log_clamps(&filter)
}

/// Expand a bare `debug`/`trace` request into the targeted profile and clamp
/// noisy third-party targets the operator did not name.
pub fn apply_default_log_clamps(filter: &str) -> String {
    let requested_directives = filter
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let global_debug_requested = requested_directives
        .iter()
        .any(|directive| matches!(directive.as_str(), "debug" | "trace"));
    let mut directives = Vec::new();

    if global_debug_requested {
        directives.extend(
            DEFAULT_GLOBAL_DEBUG_PROFILE
                .iter()
                .map(|entry| entry.to_string()),
        );
    }

    directives.extend(
        requested_directives
            .into_iter()
            .filter(|directive| !matches!(directive.as_str(), "debug" | "trace")),
    );

    for clamp in DEFAULT_LOG_CLAMPS {
        let target = clamp
            .split_once('=')
            .map(|(target, _)| target)
            .unwrap_or(clamp);
        let target_already_configured = directives.iter().any(|directive| {
            directive
                .split_once('=')
                .map(|(configured_target, _)| configured_target.trim() == target)
                .unwrap_or(false)
        });
        if !target_already_configured {
            directives.push((*clamp).to_string());
        }
    }

    directives.join(",")
}

/// Install the `tracing` formatting layer for the effective filter. Its
/// `try_init` also installs the `tracing-log` bridge, so `log` records from
/// the node crates land in the same subscriber.
pub fn init_logging() {
    let filter = effective_log_filter();
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_filter(EnvFilter::builder().parse_lossy(&filter)),
        )
        .try_init();
}

/// Resolve on SIGINT or SIGTERM. Every node's run loop holds the process
/// open on this, so a launcher's SIGTERM and a terminal's Ctrl-C both shut
/// a component down the same way.
pub async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_debug_log_filter_uses_targeted_profile() {
        let filter = apply_default_log_clamps("debug");
        let directives = filter.split(',').collect::<Vec<_>>();

        assert!(directives.contains(&DEFAULT_LOG_FILTER));
        assert!(directives.contains(&"cdma_an=debug"));
        assert!(directives.contains(&"cdma_packet=debug"));
        assert!(directives.contains(&"cdma_bts::receiver::hrpd::reverse_traffic_rake=debug"));
        assert!(!directives.contains(&"debug"));
        assert!(directives.contains(&"tonic=warn"));
    }

    #[test]
    fn explicit_log_targets_survive_default_clamps() {
        let filter = apply_default_log_clamps("cdma_bts::receiver=trace,debug");

        assert!(filter.contains("cdma_bts::receiver=trace"));
        assert!(filter.contains("cdma_an=debug"));
        assert!(!filter.split(',').any(|directive| directive == "debug"));
    }
}
