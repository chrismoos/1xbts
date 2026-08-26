use std::sync::OnceLock;

#[derive(Clone, Copy, Debug)]
struct PowerControlVerboseConfig {
    enabled: bool,
    walsh_filter: Option<u8>,
    summary_every: u64,
}

#[derive(Clone, Copy, Debug)]
struct Rc3LowerRateDiagConfig {
    enabled: bool,
    walsh_filter: Option<u8>,
    limit: usize,
}

/// Returns `true` if the env var is set to a truthy value, or `default` if unset.
fn env_bool_or(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes" || normalized == "on"
        }
        Err(_) => default,
    }
}

fn parse_walsh_filter(name: &str) -> Option<u8> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u8>().ok())
}

fn parse_limit(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_summary_every() -> u64 {
    std::env::var("CDMA_POWER_CONTROL_VERBOSE_EVERY")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        // 800 PCGs = 1 second at 1.2288 MHz / 1536 chips-per-PCG
        .unwrap_or(800)
}

fn power_control_verbose_config() -> PowerControlVerboseConfig {
    static CONFIG: OnceLock<PowerControlVerboseConfig> = OnceLock::new();
    *CONFIG.get_or_init(|| PowerControlVerboseConfig {
        enabled: env_bool_or("CDMA_POWER_CONTROL_VERBOSE", false),
        walsh_filter: parse_walsh_filter("CDMA_POWER_CONTROL_VERBOSE_WALSH"),
        summary_every: parse_summary_every(),
    })
}

fn rc3_lower_rate_diag_config() -> Rc3LowerRateDiagConfig {
    static CONFIG: OnceLock<Rc3LowerRateDiagConfig> = OnceLock::new();
    *CONFIG.get_or_init(|| Rc3LowerRateDiagConfig {
        enabled: env_bool_or("CDMA_RC3_LOWER_RATE_DIAG", false),
        walsh_filter: parse_walsh_filter("CDMA_RC3_LOWER_RATE_DIAG_WALSH"),
        limit: parse_limit("CDMA_RC3_LOWER_RATE_DIAG_LIMIT", 64),
    })
}

pub fn power_control_verbose_enabled_for_walsh(walsh_code: u8) -> bool {
    let config = power_control_verbose_config();
    config.enabled
        && config
            .walsh_filter
            .map(|filter| filter == walsh_code)
            .unwrap_or(true)
}

pub fn power_control_verbose_summary_every() -> u64 {
    power_control_verbose_config().summary_every
}

/// Per-PCG verbose logging in the BTS reverse power-control loop.
/// Set `CDMA_POWER_CONTROL_VERBOSE_PER_PCG=1` to log every tick. RC3 retains
/// the values in a compact frame-batched record to keep formatting and I/O
/// off its 800 Hz receive/control hot path.
pub fn power_control_verbose_per_pcg() -> bool {
    static CONFIG: OnceLock<bool> = OnceLock::new();
    *CONFIG.get_or_init(|| env_bool_or("CDMA_POWER_CONTROL_VERBOSE_PER_PCG", false))
}

pub fn power_control_verbose_per_pcg_enabled_for_walsh(walsh_code: u8) -> bool {
    power_control_verbose_per_pcg()
        && power_control_verbose_config()
            .walsh_filter
            .map(|filter| filter == walsh_code)
            .unwrap_or(true)
}

pub fn rc3_lower_rate_diag_enabled_for_walsh(walsh_code: u8) -> bool {
    let config = rc3_lower_rate_diag_config();
    config.enabled
        && config
            .walsh_filter
            .map(|filter| filter == walsh_code)
            .unwrap_or(true)
}

pub fn rc3_lower_rate_diag_limit() -> usize {
    rc3_lower_rate_diag_config().limit
}

/// Detailed per-window/per-packet HRPD reverse power-control logs.
/// Set `CDMA_HRPD_RPC_CONTROL_VERBOSE=1` to enable the detailed `rpc_control`
/// line in addition to the lower-rate aggregate summary.
pub fn hrpd_rpc_control_verbose() -> bool {
    static CONFIG: OnceLock<bool> = OnceLock::new();
    *CONFIG.get_or_init(|| env_bool_or("CDMA_HRPD_RPC_CONTROL_VERBOSE", false))
}

/// Detailed per-packet HRPD H-ARQ ACK issue logs.
/// Set `CDMA_HRPD_HARQ_VERBOSE=1` to show individual packet ACK misses/NAKs;
/// the default live path emits aggregate H-ARQ issue summaries instead.
pub fn hrpd_harq_verbose() -> bool {
    static CONFIG: OnceLock<bool> = OnceLock::new();
    *CONFIG.get_or_init(|| env_bool_or("CDMA_HRPD_HARQ_VERBOSE", false))
}
