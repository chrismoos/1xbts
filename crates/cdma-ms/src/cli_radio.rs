use std::path::Path;
use std::sync::Arc;

use cdma_bts::bts::config::RadioConfig as SdrRadioConfig;
use cdma_common::error::Error;
use cdma_ms::config::{MsChannel, MsNodeConfig, MsOnlyRadioConfig, MsRadioConfig};
use cdma_ms::engine::ScanConfig;
use cdma_ms::iq_radio::IqSourceRadio;
use cdma_ms::ms::MsEvent;
use cdma_ms::prl_scan::PrlScanPlan;
use cdma_ms::radio::Radio;
use cdma_ms::radio::TxCalibration;
use cdma_ms::sdr_radio::SdrRadio;
#[cfg(feature = "sim")]
use cdma_ms::sim::SimHandle;
use num_complex::Complex32;

/// Sample rate every MS radio must deliver: four samples per chip.
pub const SAMPLE_RATE_HZ: f64 = 1_228_800.0 * 4.0;
const NOOP_SILENCE_SECONDS: f64 = 1.0;

pub struct BuiltRadio {
    pub radio: Box<dyn Radio>,
    pub kind: &'static str,
    #[cfg(feature = "sim")]
    pub sim: Option<SimHandle>,
}

pub fn select_radio(config: &MsNodeConfig, arg: Option<&str>) -> Result<MsRadioConfig, Error> {
    match arg {
        None => Ok(config.radio.clone()),
        Some("sim") => Ok(MsRadioConfig::Ms(MsOnlyRadioConfig::Sim)),
        Some("noop") => Ok(MsRadioConfig::Ms(MsOnlyRadioConfig::Noop)),
        Some(path) => MsRadioConfig::load(Path::new(path)),
    }
}

fn tx_calibration_for(sdr: &SdrRadioConfig, config: &MsNodeConfig) -> TxCalibration {
    let estimated_full_scale_dbm = sdr.tx_full_scale_power_estimate_dbm();
    TxCalibration {
        tx_reference_dbm: estimated_full_scale_dbm.unwrap_or(config.calibration.tx_reference_dbm),
        estimated_full_scale_dbm,
        tx_delay_samples: sdr
            .tx_sample_delay()
            .unwrap_or(config.calibration.tx_delay_samples),
        power_control: config.calibration.tx_power_control,
        peak_limit: config.calibration.tx_peak_limit,
        relative_access_power: config.calibration.relative_access_power,
        access_initial_backoff_db: config.calibration.access_initial_backoff_db,
    }
}

pub fn rx_reference_dbm_for(radio: &MsRadioConfig, config: &MsNodeConfig) -> f32 {
    match radio {
        MsRadioConfig::Sdr(sdr) => sdr
            .rx_reference_dbm()
            .map(|dbm| dbm as f32)
            .unwrap_or(config.calibration.rx_reference_dbm),
        MsRadioConfig::Ms(_) => config.calibration.rx_reference_dbm,
    }
}

pub fn build_radio(radio: &MsRadioConfig, config: &MsNodeConfig) -> Result<BuiltRadio, Error> {
    match radio {
        #[cfg(feature = "sim")]
        MsRadioConfig::Ms(MsOnlyRadioConfig::Sim) => {
            let (sim_radio, sim) = cdma_ms::sim::boot_sim(usize::MAX);
            Ok(BuiltRadio {
                radio: Box::new(sim_radio),
                kind: "sim",
                sim: Some(sim),
            })
        }
        #[cfg(not(feature = "sim"))]
        MsRadioConfig::Ms(MsOnlyRadioConfig::Sim) => {
            Err("the sim radio requires the 'sim' feature".into())
        }
        MsRadioConfig::Ms(MsOnlyRadioConfig::Noop) => {
            let silence =
                vec![Complex32::new(0.0, 0.0); (SAMPLE_RATE_HZ * NOOP_SILENCE_SECONDS) as usize];
            Ok(BuiltRadio {
                radio: Box::new(IqSourceRadio::new(silence, SAMPLE_RATE_HZ, 0.0).with_pacing(true)),
                kind: "noop",
                #[cfg(feature = "sim")]
                sim: None,
            })
        }
        MsRadioConfig::Ms(MsOnlyRadioConfig::IqFile {
            path,
            band_class,
            band_subclass,
            cdma_channel,
            loop_playback,
            paced,
            carrier_offset_hz,
        }) => {
            let plan = MsChannel {
                band_class: *band_class,
                band_subclass: *band_subclass,
                cdma_channel: *cdma_channel,
            }
            .channel_plan()?;
            let center_hz = plan.downlink_hz() as f64;
            let source = IqSourceRadio::from_wav(Path::new(path), center_hz)?;
            if (source.sample_rate_hz() - SAMPLE_RATE_HZ).abs() > 1.0 {
                return Err(format!(
                    "{}: sample rate {} Hz, the mobile station needs {} Hz",
                    path,
                    source.sample_rate_hz(),
                    SAMPLE_RATE_HZ
                )
                .into());
            }
            log::info!(
                "iq_radio: {} presented on {} ch{} ({:.3} MHz) loop={} paced={} carrier_offset={:+.0}Hz",
                path,
                band_class.as_str(),
                cdma_channel,
                center_hz / 1e6,
                loop_playback,
                paced,
                carrier_offset_hz
            );
            Ok(BuiltRadio {
                radio: Box::new(
                    source
                        .with_loop(*loop_playback)
                        .with_pacing(*paced)
                        .with_carrier_offset_hz(*carrier_offset_hz),
                ),
                kind: "iq_file",
                #[cfg(feature = "sim")]
                sim: None,
            })
        }
        MsRadioConfig::Sdr(sdr) => {
            let channel = config.channel.channel_plan()?;
            let calibration = tx_calibration_for(sdr, config);
            let sdr_radio = SdrRadio::open(
                sdr,
                channel.downlink_hz() as f64,
                channel.uplink_hz() as f64,
                SAMPLE_RATE_HZ,
                calibration,
            )?;
            Ok(BuiltRadio {
                radio: Box::new(sdr_radio),
                kind: radio.kind(),
                #[cfg(feature = "sim")]
                sim: None,
            })
        }
    }
}

pub fn scan_config(
    config: &MsNodeConfig,
    plan: Arc<PrlScanPlan>,
    dwell_ms: Option<u32>,
) -> ScanConfig {
    ScanConfig::new(plan).with_dwell_ms(dwell_ms.unwrap_or_else(|| default_dwell_ms(config)))
}

pub fn default_dwell_ms(config: &MsNodeConfig) -> u32 {
    config
        .acquisition
        .scan_dwell_ms
        .unwrap_or(if config.radio.is_hardware_radio() {
            cdma_ms::config::DEFAULT_SDR_SCAN_DWELL_MS
        } else {
            cdma_ms::config::DEFAULT_SCAN_DWELL_MS
        })
}

pub fn event_line(ev: &MsEvent) -> String {
    match ev {
        MsEvent::StateChange { from, to } => format!("state {from} -> {to}"),
        MsEvent::ScanStarted { prl_id, channels } => {
            format!("scan started: PRL {prl_id}, {channels} channels")
        }
        MsEvent::ChannelTuned {
            band_class,
            channel,
            frequency_hz,
            index,
            total,
        } => format!(
            "[{:>2}/{}] tuned {} ch{} ({:.3} MHz), searching",
            index + 1,
            total,
            band_class.to_ascii_lowercase(),
            channel,
            *frequency_hz as f64 / 1e6
        ),
        MsEvent::PilotSearch {
            band_class,
            channel,
            found: true,
            ec_io_db,
            rx_power_dbfs,
        } => format!(
            "        {} ch{} pilot locked: Ec/Io {}, rx {:.1} dBFS",
            band_class.to_ascii_lowercase(),
            channel,
            db_or_dash(*ec_io_db),
            rx_power_dbfs
        ),
        MsEvent::PilotSearch {
            band_class,
            channel,
            found: false,
            rx_power_dbfs,
            ..
        } => format!(
            "        {} ch{} no pilot (rx {:.1} dBFS)",
            band_class.to_ascii_lowercase(),
            channel,
            rx_power_dbfs
        ),
        MsEvent::PilotAcquired { pn } => format!("        pilot PN offset {pn}"),
        MsEvent::SyncDecoded(sync) => format!(
            "        sync decoded: SID {} NID {} pilot_pn {} p_rev {} sys_time {} ({})",
            sync.sid,
            sync.nid,
            sync.pilot_pn,
            sync.p_rev,
            sync.sys_time,
            sync.system_time()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        ),
        MsEvent::PilotMeasurement {
            ec_io_db,
            rx_power_dbfs,
        } => format!(
            "        pilot: Ec/Io {}, rx {rx_power_dbfs:.1} dBFS",
            db_or_dash(*ec_io_db)
        ),
        MsEvent::PagingMessageDecoded {
            name,
            config_msg_seq,
        } => format!(
            "        paging: {name}{}",
            config_msg_seq
                .map(|s| format!(" (seq {s})"))
                .unwrap_or_default()
        ),
        MsEvent::DumpFinished { path, samples } => {
            format!("dump finished: {samples} samples in {path}")
        }
        MsEvent::PrlVerdict {
            sid,
            nid,
            permitted,
            reason,
            record,
            roaming_indicator,
        } => format!(
            "        PRL verdict for SID {sid} NID {nid}: {} ({reason}{}{})",
            if *permitted {
                "permitted"
            } else {
                "not permitted"
            },
            record.map(|r| format!(", record {r}")).unwrap_or_default(),
            roaming_indicator
                .map(|r| format!(", roam_ind {r}"))
                .unwrap_or_default()
        ),
        MsEvent::ScanFinished {
            result,
            elapsed_ms,
            channels,
            pilots,
            syncs,
        } => format!(
            "scan finished: {result} after {:.1} s of air time ({channels} channels, {pilots} pilots, {syncs} syncs)",
            *elapsed_ms as f64 / 1000.0
        ),
        MsEvent::OverheadUpdated { sid, nid, base_id } => {
            format!("overhead: SID {sid} NID {nid} BASE_ID {base_id}")
        }
        MsEvent::AccessParametersUpdated => "access parameters received".to_string(),
        MsEvent::RegistrationNeeded { reg_type } => {
            format!("registration needed (type {reg_type})")
        }
        MsEvent::PageReceived {
            esn,
            service_option,
        } => format!("page received for ESN 0x{esn:08X} (SO {service_option})"),
        MsEvent::AccessStarted { reason } => format!("access started: {reason}"),
        MsEvent::AccessProbe {
            reason,
            msg_seq,
            ack_req,
            ack_seq,
            sequence,
            probe,
            power_dbm,
            ..
        } => format!(
            "access tx {reason} msg_seq {msg_seq}{}{} probe_seq {sequence} probe {probe} at {power_dbm:.1} dBm",
            ack_seq
                .map(|seq| format!(", ack_seq {seq}"))
                .unwrap_or_default(),
            if *ack_req { ", ack requested," } else { "," },
        ),
        MsEvent::AccessComplete { reason } => format!("access complete: {reason}"),
        MsEvent::AccessFailed { reason, cause } => {
            format!("access failed: {reason} ({cause})")
        }
        MsEvent::RegistrationAccepted => "registration accepted".to_string(),
        MsEvent::RegistrationRejected { ordq } => {
            format!("registration rejected (ordq {ordq})")
        }
        MsEvent::DirectedMessage {
            name,
            order,
            msg_seq,
            ack_req,
            ack_seq,
        } => format!(
            "directed {name}{} msg_seq {msg_seq}{}{}",
            order
                .as_ref()
                .map(|o| format!(" ({o})"))
                .unwrap_or_default(),
            ack_seq
                .map(|seq| format!(", acks reverse msg_seq {seq}"))
                .unwrap_or_default(),
            if *ack_req { ", ack requested" } else { "" }
        ),
        MsEvent::SmsReceived {
            originating_number,
            text,
        } => format!("SMS from {originating_number}: {text:?}"),
        MsEvent::ChannelAssignment {
            walsh_code,
            frame_offset,
            for_rc,
            rev_rc,
        } => format!(
            "channel assignment: walsh {walsh_code} frame_offset {frame_offset} for_rc {for_rc} rev_rc {rev_rc}"
        ),
        MsEvent::TrafficChannelUp { walsh_code } => {
            format!("traffic channel up on walsh {walsh_code}")
        }
        MsEvent::TrafficMessage {
            name,
            order,
            msg_seq,
            ack_seq,
            ack_req,
        } => format!(
            "traffic rx {name}{} msg_seq {msg_seq}, ack_seq {ack_seq}{}",
            order
                .as_ref()
                .map(|o| format!(" ({o})"))
                .unwrap_or_default(),
            if *ack_req { ", ack requested" } else { "" }
        ),
        MsEvent::TrafficTransmit {
            name,
            msg_seq,
            ack_seq,
            ack_req,
            retransmission,
        } => format!(
            "traffic tx {name} msg_seq {msg_seq}, ack_seq {ack_seq}{}{}",
            if *ack_req { ", ack requested" } else { "" },
            if *retransmission {
                ", retransmission"
            } else {
                ""
            }
        ),
        MsEvent::ServiceConnected { service_option } => {
            format!("service connected (SO {service_option})")
        }
        MsEvent::CallRinging {
            caller_number,
            service_option,
        } => format!(
            "incoming call from {} (SO {service_option})",
            caller_number.as_deref().unwrap_or("unknown caller")
        ),
        MsEvent::CallAnswered => "call answered".to_string(),
        MsEvent::StatusRequested { record_types } => {
            let types: Vec<String> = record_types.iter().map(|t| format!("0x{t:02x}")).collect();
            format!("status request for records {}", types.join(", "))
        }
        MsEvent::SmsCauseCode { error_class } => {
            format!("SMS cause code: error class {error_class}")
        }
        MsEvent::TrafficReleased { by } => format!("traffic channel released by {by}"),
    }
}

pub fn db_or_dash(v: Option<f32>) -> String {
    match v {
        Some(db) => format!("{db:.1} dB"),
        None => "-".to_string(),
    }
}

pub fn parse_voice_codec(value: &str) -> Option<u16> {
    match value.to_ascii_lowercase().as_str() {
        "tia96" | "tia-96" | "so1" => Some(cdma_voice::SERVICE_OPTION_BASIC_VOICE),
        "evrc" | "evrc-a" | "so3" => Some(cdma_voice::SERVICE_OPTION_EVRC_A),
        "evrc-b" | "so68" => Some(cdma_voice::SERVICE_OPTION_EVRC_B),
        "evrc-wb" | "so70" => Some(cdma_voice::SERVICE_OPTION_EVRC_WB),
        "qcelp" | "qcelp-13k" | "so32768" => Some(cdma_voice::SERVICE_OPTION_QCELP_13K),
        _ => None,
    }
}

pub fn is_chatty(ev: &MsEvent) -> bool {
    matches!(
        ev,
        MsEvent::PilotMeasurement { .. } | MsEvent::PagingMessageDecoded { .. }
    )
}

pub mod show {

    use cdma_ms::grpc::proto::{
        Diagnostics, Overhead, PagingStats, PilotStatus, RadioStats, ScanReport, SyncParameters,
    };

    pub fn pilot(p: &PilotStatus) -> String {
        format!(
            "pilot: {}\n  Ec/Io      {}\n  rx power   {:.1} dBFS\n  symbols    {}",
            if p.locked { "locked" } else { "not locked" },
            if p.measured {
                format!("{:.1} dB", p.ec_io_db)
            } else {
                "-".to_string()
            },
            p.rx_power_dbfs,
            p.pilot_symbols
        )
    }

    pub fn sync(s: &SyncParameters) -> String {
        format!(
            "sync channel message:\n  P_REV      {}\n  MIN_P_REV  {}\n  SID        {}\n  NID        {}\n  PILOT_PN   {}\n  LC_STATE   0x{:010x}\n  SYS_TIME   {} ({})\n  LP_SEC     {}\n  LTM_OFF    {} ({:+.1} h)\n  DAYLT      {}\n  PRAT       {} ({} bps)\n  CDMA_FREQ  {}",
            s.p_rev,
            s.min_p_rev,
            s.sid,
            s.nid,
            s.pilot_pn,
            s.lc_state,
            s.sys_time,
            s.system_time,
            s.lp_sec,
            s.ltm_off,
            s.ltm_off as f64 * 0.5,
            s.daylt,
            s.prat,
            s.paging_rate_bps,
            s.cdma_freq
        )
    }

    fn fields(json: &str) -> String {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return "  (not received)".to_string();
        };
        let Some(map) = value.as_object() else {
            return format!("  {value}");
        };
        map.iter()
            .map(|(k, v)| format!("  {:<28} {}", k.to_ascii_uppercase(), v))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn overhead(o: &Overhead, which: Option<&str>) -> String {
        match which.map(|w| w.to_ascii_lowercase()).as_deref() {
            None => format!(
                "overhead received: {}\n  (overhead spm|apm|espm|cclm|nlm|enlm for the fields)",
                if o.received.is_empty() {
                    "none".to_string()
                } else {
                    o.received.join(", ")
                }
            ),
            Some("spm") => format!(
                "System Parameters Message:\n{}",
                fields(&o.system_parameters)
            ),
            Some("apm") => format!(
                "Access Parameters Message:\n{}",
                fields(&o.access_parameters)
            ),
            Some("espm") => format!(
                "Extended System Parameters Message:\n{}",
                fields(&o.extended_system_parameters)
            ),
            Some("cclm") => format!(
                "CDMA Channel List Message:\n{}",
                fields(&o.cdma_channel_list)
            ),
            Some("nlm") => format!("Neighbor List Message:\n{}", fields(&o.neighbor_list)),
            Some("enlm") => format!(
                "Extended Neighbor List Message:\n{}",
                fields(&o.extended_neighbor_list)
            ),
            Some("neighbors") => {
                if !o.extended_neighbor_list.is_empty() {
                    format!(
                        "Extended Neighbor List Message:\n{}",
                        fields(&o.extended_neighbor_list)
                    )
                } else {
                    format!("Neighbor List Message:\n{}", fields(&o.neighbor_list))
                }
            }
            Some(other) => {
                format!("unknown overhead message '{other}' (spm, apm, espm, cclm, nlm, enlm)")
            }
        }
    }

    pub fn paging(p: &PagingStats) -> String {
        let mut by_type: Vec<_> = p.messages.iter().collect();
        by_type.sort();
        let mut undecodable: Vec<_> = p.undecodable.iter().collect();
        undecodable.sort();
        format!(
            "paging channel:\n  CRC valid  {}\n  CRC failed {}\n  by message {}\n  undecodable {}\n  pages for me {}",
            p.crc_valid,
            p.crc_failed,
            if by_type.is_empty() {
                "-".to_string()
            } else {
                by_type
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
            if undecodable.is_empty() {
                "none".to_string()
            } else {
                undecodable
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
            p.pages_for_me
        )
    }

    pub fn radio(r: &RadioStats) -> String {
        format!(
            "radio:\n  samples fed   {}\n  RX overflows  {}\n  uptime        {:.1} s\n  real-time     {:.2}x",
            r.samples_fed, r.rx_overflows, r.uptime_secs, r.real_time_ratio
        )
    }

    pub fn scan_table(r: &ScanReport) -> String {
        let mut out = vec![format!(
            "scan ({}): {} of {} channels, {}",
            r.mode().label(),
            r.channels.len(),
            r.total_channels,
            if r.running {
                "running".to_string()
            } else if r.result.is_empty() {
                "not started".to_string()
            } else {
                r.result.clone()
            }
        )];
        out.push(format!(
            "{:<26} {:<9} {:>8} {:>8} {:<16} {}",
            "channel", "pilot", "ec/io", "rx dBFS", "sync", "verdict"
        ));
        for c in &r.channels {
            let m = c.measurement.clone().unwrap_or_default();
            let found = c.pilot == "found";
            out.push(format!(
                "{:<26} {:<9} {:>8} {:>8} {:<16} {}",
                format!(
                    "{} ch{} {:.3} MHz",
                    c.band_class,
                    c.channel,
                    c.frequency_hz / 1e6
                ),
                c.pilot,
                if found && m.measured {
                    format!("{:.1}", m.ec_io_db)
                } else {
                    "-".to_string()
                },
                format!("{:.1}", m.rx_power_dbfs),
                c.sync
                    .as_ref()
                    .map(|s| format!("SID {} NID {}", s.sid, s.nid))
                    .unwrap_or_else(|| "-".to_string()),
                c.verdict
                    .as_ref()
                    .map(|v| format!(
                        "{} ({})",
                        if v.permitted {
                            "permitted"
                        } else {
                            "not permitted"
                        },
                        v.reason().label()
                    ))
                    .unwrap_or_else(|| "-".to_string()),
            ));
        }
        out.join("\n")
    }

    pub fn status(d: &Diagnostics) -> String {
        let mut out = vec![format!(
            "state: {}  registered: {}",
            d.state().label(),
            if d.registered { "yes" } else { "no" }
        )];
        if let Some(p) = &d.pilot {
            out.push(pilot(p));
        }
        match &d.sync {
            Some(s) => out.push(format!(
                "sync: SID {} NID {} pilot_pn {} p_rev {} sys_time {} ({})",
                s.sid, s.nid, s.pilot_pn, s.p_rev, s.sys_time, s.system_time
            )),
            None => out.push("sync: not decoded".to_string()),
        }
        if let Some(o) = &d.overhead {
            out.push(overhead(o, None));
        }
        if let Some(p) = &d.paging {
            out.push(paging(p));
        }
        if let Some(s) = &d.scan {
            out.push(format!(
                "scan: mode {} {} ({} of {} channels)",
                s.mode().label(),
                if s.running {
                    "running"
                } else if s.result.is_empty() {
                    "not started"
                } else {
                    &s.result
                },
                s.channels.len(),
                s.total_channels
            ));
        }
        if let Some(r) = &d.radio {
            out.push(radio(r));
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directed_event_shows_acknowledged_reverse_sequence() {
        let event = MsEvent::DirectedMessage {
            name: "ORDM".to_string(),
            order: Some("Base Station Acknowledgment".to_string()),
            msg_seq: 7,
            ack_req: false,
            ack_seq: Some(2),
        };
        assert_eq!(
            event_line(&event),
            "directed ORDM (Base Station Acknowledgment) msg_seq 7, acks reverse msg_seq 2"
        );
    }

    #[test]
    fn access_probe_shows_every_layer_two_sequence_field() {
        let event = MsEvent::AccessProbe {
            reason: "page_response(so=6)".to_string(),
            msg_seq: 3,
            ack_req: true,
            ack_seq: Some(5),
            sequence: 0,
            probe: 1,
            power_dbm: 4.5,
            start_chip: 0,
        };
        assert_eq!(
            event_line(&event),
            "access tx page_response(so=6) msg_seq 3, ack_seq 5, ack requested, probe_seq 0 probe 1 at 4.5 dBm"
        );
    }

    #[test]
    fn selected_radio_supplies_tx_delay_and_power_estimate() {
        let mut config: MsNodeConfig = serde_json::from_str(
            r#"{
                "identity": {"esn": 305419896, "imsi": "310001234567890"},
                "channel": {"band_class": "bc1", "cdma_channel": 50}
            }"#,
        )
        .unwrap();
        config.calibration.tx_delay_samples = -68;

        let calibrated: SdrRadioConfig = serde_json::from_str(
            r#"{"kind":"uhd","device":"type=b200","channel":0,"antenna":"TX/RX","tx_gain_db":65.0,"tx_sample_delay":-70,"tx_max_gain_db":89.75,"tx_max_gain_power_estimate_dbm":18.75}"#,
        )
        .unwrap();
        assert_eq!(
            tx_calibration_for(&calibrated, &config).tx_delay_samples,
            -70
        );
        assert_eq!(
            tx_calibration_for(&calibrated, &config).estimated_full_scale_dbm,
            Some(-6.0)
        );
        assert_eq!(
            tx_calibration_for(&calibrated, &config).tx_reference_dbm,
            -6.0
        );
        let lower_gain: SdrRadioConfig = serde_json::from_str(
            r#"{"kind":"uhd","device":"type=b200","channel":0,"antenna":"TX/RX","tx_gain_db":55.0,"tx_max_gain_db":89.75,"tx_max_gain_power_estimate_dbm":18.75,"rx_reference_dbm":-18.0}"#,
        )
        .unwrap();
        assert_eq!(
            tx_calibration_for(&lower_gain, &config).estimated_full_scale_dbm,
            Some(-16.0)
        );
        assert_eq!(
            rx_reference_dbm_for(&MsRadioConfig::Sdr(Box::new(lower_gain)), &config),
            -18.0
        );
        let modeled_rx: MsRadioConfig = serde_json::from_str(
            r#"{"kind":"uhd","device":"type=b200","channel":0,"antenna":"TX/RX","rx_gain_db":50.0,"rx_max_gain_db":76.0,"rx_max_gain_reference_dbm":-65.0}"#,
        )
        .unwrap();
        assert_eq!(rx_reference_dbm_for(&modeled_rx, &config), -39.0);
        let lower_rx_gain: SdrRadioConfig = serde_json::from_str(
            r#"{"kind":"uhd","device":"type=b200","channel":0,"antenna":"TX/RX","rx_gain_db":40.0,"rx_max_gain_db":76.0,"rx_max_gain_reference_dbm":-65.0}"#,
        )
        .unwrap();
        assert_eq!(
            rx_reference_dbm_for(&MsRadioConfig::Sdr(Box::new(lower_rx_gain)), &config),
            -29.0
        );
        let uncalibrated: SdrRadioConfig = serde_json::from_str(
            r#"{"kind":"uhd","device":"type=b200","channel":0,"antenna":"TX/RX"}"#,
        )
        .unwrap();
        assert_eq!(
            tx_calibration_for(&uncalibrated, &config).tx_delay_samples,
            -68
        );
        assert_eq!(
            tx_calibration_for(&uncalibrated, &config).estimated_full_scale_dbm,
            None
        );
    }

    #[test]
    fn voice_codec_names_cover_every_supported_service_option() {
        assert_eq!(parse_voice_codec("tia96"), Some(1));
        assert_eq!(parse_voice_codec("evrc-a"), Some(3));
        assert_eq!(parse_voice_codec("evrc-b"), Some(68));
        assert_eq!(parse_voice_codec("evrc-wb"), Some(70));
        assert_eq!(parse_voice_codec("qcelp-13k"), Some(32768));
        assert_eq!(parse_voice_codec("bogus"), None);
    }
}
