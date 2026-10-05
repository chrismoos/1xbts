use std::path::Path;
use std::time::Duration;

use cdma_bts::bts::config::RadioConfig as SdrRadioConfig;
use cdma_common::band_class::{BandClass, ChannelPlan};
use cdma_common::config_load::load_json_with_local_override;
use cdma_common::error::Error;
use serde::{Deserialize, Serialize};

use crate::tx::MsAccessIdentity;

pub const DEFAULT_SCAN_DWELL_MS: u32 = 300;
pub const DEFAULT_SDR_SCAN_DWELL_MS: u32 = 500;

fn default_grpc_listen() -> String {
    "[::1]:50052".to_string()
}

fn default_true() -> bool {
    true
}

/// The MS's provisioned mobile identity (C.S0005-E §2.3.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MsIdentity {
    pub esn: u32,
    pub imsi: String,
    /// Mobile Equipment Identifier, 14 hexadecimal digits. The mobile claims
    /// MEID support in its station class mark, so a base station may ask for
    /// this by name. Unset leaves it out of what the mobile reports rather
    /// than reporting a filled-in placeholder.
    #[serde(default)]
    pub meid: Option<String>,
}

impl Default for MsIdentity {
    fn default() -> Self {
        MsIdentity {
            esn: 0,
            imsi: "310000000000000".to_string(),
            meid: None,
        }
    }
}

impl MsIdentity {
    pub fn validate(&self) -> Result<(), Error> {
        if self.imsi.len() != 15 || !self.imsi.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::from(format!(
                "invalid IMSI {:?}: expected exactly 15 decimal digits",
                self.imsi
            )));
        }
        Ok(())
    }

    pub fn access_identity(&self) -> Result<MsAccessIdentity, Error> {
        self.validate()?;
        let imsi_s = self.imsi[5..]
            .parse::<u64>()
            .map_err(|error| Error::from(format!("invalid IMSI {:?}: {error}", self.imsi)))?;
        Ok(MsAccessIdentity {
            esn: self.esn,
            imsi_s,
            mcc: self.imsi[..3].to_string(),
            imsi_11_12: self.imsi[3..5].to_string(),
            meid: self.meid.clone(),
            ..Default::default()
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MsRadioConfig {
    Ms(MsOnlyRadioConfig),
    Sdr(Box<SdrRadioConfig>),
}

impl Default for MsRadioConfig {
    fn default() -> Self {
        MsRadioConfig::Ms(MsOnlyRadioConfig::Sim)
    }
}

impl MsRadioConfig {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let value = load_json_with_local_override(path).map_err(|e| Error::from(e.to_string()))?;
        serde_json::from_value(value).map_err(|e| {
            Error::from(format!(
                "{}: not an MS radio config (kind sim, iq_file, noop) or an SDR radio config (kind uhd, bladerf, soapy, lime): {}",
                path.display(),
                e
            ))
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            MsRadioConfig::Ms(MsOnlyRadioConfig::Sim) => "sim",
            MsRadioConfig::Ms(MsOnlyRadioConfig::IqFile { .. }) => "iq_file",
            MsRadioConfig::Ms(MsOnlyRadioConfig::Noop) => "noop",
            MsRadioConfig::Sdr(sdr) => match sdr.as_ref() {
                SdrRadioConfig::Uhd { .. } => "uhd",
                SdrRadioConfig::BladeRf { .. } => "bladerf",
                SdrRadioConfig::Soapy { .. } => "soapy",
                SdrRadioConfig::Lime { .. } => "lime",
                SdrRadioConfig::FileOutput { .. } => "file_output",
                SdrRadioConfig::Network { .. } => "network",
                SdrRadioConfig::Noop => "noop",
            },
        }
    }

    pub fn is_hardware_radio(&self) -> bool {
        !matches!(self, MsRadioConfig::Ms(MsOnlyRadioConfig::Sim))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MsOnlyRadioConfig {
    Sim,
    IqFile {
        path: String,
        band_class: BandClass,
        #[serde(default)]
        band_subclass: u8,
        cdma_channel: u16,
        #[serde(default = "default_true")]
        loop_playback: bool,
        #[serde(default = "default_true")]
        paced: bool,
        #[serde(default)]
        carrier_offset_hz: f64,
    },
    Noop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsChannel {
    pub band_class: BandClass,
    #[serde(default)]
    pub band_subclass: u8,
    pub cdma_channel: u16,
}

impl MsChannel {
    pub fn channel_plan(&self) -> ChannelPlan {
        ChannelPlan::new(self.band_class, self.band_subclass, self.cdma_channel)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MsAcquisitionConfig {
    #[serde(default)]
    pub scan_dwell_ms: Option<u32>,
    /// T57m power-up registration delay (C.S0005-E Annex D). Zero disables it.
    #[serde(default)]
    pub t57m_power_up_delay_ms: Option<u32>,
}

impl Default for MsAcquisitionConfig {
    fn default() -> Self {
        MsAcquisitionConfig {
            scan_dwell_ms: None,
            t57m_power_up_delay_ms: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MsCalibrationConfig {
    /// Mean input power in dBm that reads as 0 dBFS on this receiver. The
    /// open-loop power estimate uses wideband received dBFS plus this reference.
    #[serde(default = "default_rx_reference_dbm")]
    pub rx_reference_dbm: f32,
    /// Mean output power in dBm of a transmit burst whose samples have unit
    /// RMS. The radio scales each burst to hit its commanded power from this.
    #[serde(default = "default_tx_reference_dbm")]
    pub tx_reference_dbm: f32,
    /// Fallback transmit timing correction when the selected radio has no
    /// `tx_sample_delay` calibration.
    #[serde(default)]
    pub tx_delay_samples: i64,
    #[serde(default)]
    pub tx_power_control: bool,
    #[serde(default = "default_tx_peak_limit")]
    pub tx_peak_limit: f32,
    #[serde(default)]
    pub relative_access_power: bool,
    /// Digital backoff from the TX peak before NOM_PWR and INIT_PWR are applied.
    #[serde(default)]
    pub access_initial_backoff_db: f32,
}

fn default_tx_peak_limit() -> f32 {
    1.0
}

fn default_rx_reference_dbm() -> f32 {
    -30.0
}

fn default_tx_reference_dbm() -> f32 {
    0.0
}

impl Default for MsCalibrationConfig {
    fn default() -> Self {
        MsCalibrationConfig {
            rx_reference_dbm: default_rx_reference_dbm(),
            tx_reference_dbm: default_tx_reference_dbm(),
            tx_delay_samples: 0,
            tx_power_control: false,
            tx_peak_limit: default_tx_peak_limit(),
            relative_access_power: false,
            access_initial_backoff_db: 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MsNodeConfig {
    pub identity: MsIdentity,
    pub channel: MsChannel,
    #[serde(default)]
    pub radio: MsRadioConfig,
    #[serde(default)]
    pub acquisition: MsAcquisitionConfig,
    #[serde(default)]
    pub calibration: MsCalibrationConfig,
    #[serde(default = "default_grpc_listen")]
    pub grpc_listen: String,
}

impl MsNodeConfig {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let value = load_json_with_local_override(path).map_err(|e| Error::from(e.to_string()))?;
        let config: Self = serde_json::from_value(value).map_err(|e| Error::from(e.to_string()))?;
        config.identity.validate()?;
        config.channel.channel_plan().validate()?;
        Ok(config)
    }

    pub fn power_up_delay(&self) -> Duration {
        match self.acquisition.t57m_power_up_delay_ms {
            Some(ms) => Duration::from_millis(u64::from(ms)),
            None if self.radio.is_hardware_radio() => crate::ms::T57M,
            None => Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_default_config() {
        let json = r#"{
            "identity": { "esn": 305419896, "imsi": "310001234567890" },
            "channel": { "band_class": "bc0", "cdma_channel": 384 }
        }"#;
        let cfg: MsNodeConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.identity.esn, 305419896);
        let identity = cfg.identity.access_identity().unwrap();
        assert_eq!(identity.mcc, "310");
        assert_eq!(identity.imsi_11_12, "00");
        assert_eq!(identity.imsi_s, 1_234_567_890);
        assert_eq!(cfg.channel.channel_plan().downlink_hz(), 881_520_000);
        assert_eq!(cfg.channel.channel_plan().uplink_hz(), 836_520_000);
        assert_eq!(cfg.grpc_listen, default_grpc_listen());
        assert_eq!(cfg.radio.kind(), "sim");
        assert!(!cfg.radio.is_hardware_radio());
        assert_eq!(cfg.acquisition.scan_dwell_ms, None);
        assert_eq!(cfg.power_up_delay(), Duration::ZERO);
    }

    #[test]
    fn power_up_delay_defaults_to_t57m_on_a_radio_and_honors_an_override() {
        let radio: MsNodeConfig = serde_json::from_str(
            r#"{
            "identity": { "esn": 305419896, "imsi": "310001234567890" },
            "channel": { "band_class": "bc0", "cdma_channel": 384 },
            "radio": { "kind": "noop" }
        }"#,
        )
        .unwrap();
        assert_eq!(radio.power_up_delay(), crate::ms::T57M);

        let overridden: MsNodeConfig = serde_json::from_str(
            r#"{
            "identity": { "esn": 305419896, "imsi": "310001234567890" },
            "channel": { "band_class": "bc0", "cdma_channel": 384 },
            "radio": { "kind": "noop" },
            "acquisition": { "t57m_power_up_delay_ms": 1500 }
        }"#,
        )
        .unwrap();
        assert_eq!(overridden.power_up_delay(), Duration::from_millis(1500));
    }

    #[test]
    fn rejects_an_imsi_that_is_not_fifteen_digits() {
        let identity = MsIdentity {
            esn: 1,
            imsi: "1234567890".to_string(),
            meid: None,
        };
        assert!(identity.validate().is_err());
    }

    #[test]
    fn example_channel_derives_paired_frequencies_and_rejects_rate_overrides() {
        let cfg: MsNodeConfig =
            serde_json::from_str(include_str!("../../../../config/ms.json")).unwrap();
        let channel = cfg.channel.channel_plan();
        channel.validate().unwrap();
        assert_eq!(channel.downlink_hz(), 1_932_500_000);
        assert_eq!(channel.uplink_hz(), 1_852_500_000);
        for field in ["sample_rate_hz", "oversample", "forward_hz", "reverse_hz"] {
            let mut value = serde_json::to_value(&cfg.channel).unwrap();
            value[field] = serde_json::json!(4);
            assert!(serde_json::from_value::<MsChannel>(value).is_err());
        }
    }

    #[test]
    fn accepts_a_bts_radio_config_as_the_sdr_backend() {
        let json = r#"{
            "kind": "uhd",
            "device": "type=b200",
            "channel": 0,
            "antenna": "TX/RX",
            "rx_antenna": "RX2",
            "rx_gain_db": 40
        }"#;
        let radio: MsRadioConfig = serde_json::from_str(json).unwrap();
        assert_eq!(radio.kind(), "uhd");
        assert!(radio.is_hardware_radio());
    }

    #[test]
    fn accepts_an_iq_file_radio() {
        let json = r#"{
            "kind": "iq_file",
            "path": "capture.wav",
            "band_class": "bc0",
            "cdma_channel": 384
        }"#;
        let radio: MsRadioConfig = serde_json::from_str(json).unwrap();
        assert_eq!(radio.kind(), "iq_file");
        match radio {
            MsRadioConfig::Ms(MsOnlyRadioConfig::IqFile {
                loop_playback,
                paced,
                cdma_channel,
                ..
            }) => {
                assert!(loop_playback);
                assert!(paced);
                assert_eq!(cdma_channel, 384);
            }
            other => panic!("unexpected radio {other:?}"),
        }
    }
}
