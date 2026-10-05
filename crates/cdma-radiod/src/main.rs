use std::path::PathBuf;

use cdma_bts::bts::{RadioConfig, load_radio_from_path};
use cdma_radiod::config::DEFAULT_CONFIG_PATH;
use cdma_radiod::{RadioCalibration, RadiodConfig, RxDefaults, serve};
use clap::Parser;
use log::info;

#[derive(Parser)]
#[command(about = "1xBTS radio server: serves a local SDR over the network")]
struct Args {
    /// Listener and sample-rate settings.
    #[arg(long, default_value = DEFAULT_CONFIG_PATH)]
    config: PathBuf,

    /// Standalone radio JSON shared with BTS and MS.
    #[arg(long, visible_alias = "radio")]
    radio_config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), cdma_common::error::Error> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let config = RadiodConfig::load_from_path(&args.config)?;
    let radio_config = load_radio_from_path(&args.radio_config)?;
    if matches!(radio_config, RadioConfig::Network { .. }) {
        return Err("radiod requires a local radio backend, not another network radio".into());
    }
    info!("radiod: serving {} radio", radio_config.kind_name());

    let radio = cdma_bts::bts::build_radio_from_config(
        &radio_config,
        0,
        cdma_bts::bts::RadioBuildOptions {
            null_radio: false,
            configure_rx: false,
            tx_sample_rate_hz: config.tx_sample_rate_hz,
            rx_sample_rate_hz: 0,
            rx_bandwidth_hz: 0,
            realtime: cdma_bts::bts::RealtimeSettings::default(),
        },
    )?;
    let rx_defaults = RxDefaults {
        antenna: radio_config.rx_antenna_or_default(),
        gain_db: radio_config.rx_gain_db_f64(),
    };
    serve(
        radio,
        radio_config.has_rx(),
        radio_config.kind_name().to_string(),
        rx_defaults,
        RadioCalibration {
            tx_sample_delay: radio_config.tx_sample_delay(),
            tx_full_scale_power_estimate_dbm: radio_config.tx_full_scale_power_estimate_dbm(),
            rx_reference_dbm: radio_config.rx_reference_dbm().map(|dbm| dbm as f32),
        },
        config.listen,
        config.data_listen,
        None,
        async {
            let _ = tokio::signal::ctrl_c().await;
            info!("radiod: shutting down");
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_default_listener_config_and_accepts_an_override() {
        let args = Args::try_parse_from([
            "cdma-radiod",
            "--radio-config",
            "config/radio_soapydr_example.json",
        ])
        .unwrap();
        assert_eq!(args.config, PathBuf::from(DEFAULT_CONFIG_PATH));
        assert_eq!(
            args.radio_config,
            PathBuf::from("config/radio_soapydr_example.json")
        );
        let args = Args::try_parse_from([
            "cdma-radiod",
            "--config",
            "custom.json",
            "--radio",
            "hardware.json",
        ])
        .unwrap();
        assert_eq!(args.config, PathBuf::from("custom.json"));
        assert_eq!(args.radio_config, PathBuf::from("hardware.json"));
        assert!(Args::try_parse_from(["cdma-radiod"]).is_err());
    }
}
