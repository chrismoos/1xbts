use std::{
    fs,
    net::SocketAddr,
    sync::Arc,
    sync::mpsc::{Receiver as StdReceiver, channel},
};

use cdma_abis::udp_bearer::UdpBearerDatagram;
use cdma_common::error::Error;
use log::{info, warn};
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "soapy-backend")]
use crate::sdr::SoapySdrRadio;
use crate::{
    lac,
    mac::{self, Layer2MacRef},
    receiver::sync::SyncChannelMessage,
    sdr::{FileOutputRadio, NoopRadio, Radio},
};

use super::{
    Bts, BtsHandle, BtsNodeConfig, Config, OverheadParameters, PagingChannelSettings, RadioConfig,
    RealtimeSettings, ReverseRxTarget, RxSettings, TrafficResourceController,
    abis_agent::{AbisAgent, AbisAgentConfig, AbisAgentEvent},
    paging_supplier::{PagingRetryConfig, PagingSupplierState, build_bts_paging_supplier},
};

pub struct RadioBuildOptions {
    pub null_radio: bool,
    pub configure_rx: bool,
    pub tx_sample_rate_hz: usize,
    pub rx_sample_rate_hz: usize,
    pub rx_bandwidth_hz: usize,
    pub realtime: RealtimeSettings,
}

#[derive(Clone, Debug)]
pub struct ReverseRxPlan {
    pub configure_rx: bool,
    pub target: ReverseRxTarget,
    pub one_x_enabled: bool,
    pub center_frequency_hz: usize,
    pub sample_rate_hz: usize,
    pub bandwidth_hz: usize,
    pub one_x_reverse_frequency_hz: usize,
    pub one_x_rx_shift_hz: i64,
    pub hrpd_reverse_frequency_hz: Option<usize>,
    pub hrpd_rx_shift_hz: Option<i64>,
    pub required_bandwidth_hz: Option<usize>,
}

pub fn resolve_reverse_rx_plan(bts_config: &BtsNodeConfig) -> Result<ReverseRxPlan, Error> {
    let one_x_reverse_frequency_hz = bts_config.channel.uplink_hz() as usize;
    let sample_rate_hz = bts_config.rf.rx_sample_rate_hz;
    let bandwidth_hz = bts_config.rf.rx_bandwidth_hz;
    // The null radio is treated as RX-capable: it supplies a dummy RX half so
    // the reverse pipeline runs end to end against silence (no hardware).
    let radio_has_rx = matches!(
        bts_config.radio,
        RadioConfig::Soapy { .. }
            | RadioConfig::Uhd { .. }
            | RadioConfig::Lime { .. }
            | RadioConfig::BladeRf { .. }
            | RadioConfig::Noop
    );
    let resolved_target = if !bts_config.evdo.enabled {
        ReverseRxTarget::OneX
    } else {
        match bts_config.evdo.tx_mode() {
            super::evdo::EvdoTxMode::AdjacentComposite => ReverseRxTarget::Composite,
            super::evdo::EvdoTxMode::HrpdOnly => ReverseRxTarget::Hrpd,
        }
    };
    let one_x_enabled = resolved_target != ReverseRxTarget::Hrpd;
    let configure_rx = radio_has_rx;
    let mut plan = ReverseRxPlan {
        configure_rx,
        target: ReverseRxTarget::OneX,
        one_x_enabled,
        center_frequency_hz: one_x_reverse_frequency_hz,
        sample_rate_hz,
        bandwidth_hz,
        one_x_reverse_frequency_hz,
        one_x_rx_shift_hz: 0,
        hrpd_reverse_frequency_hz: None,
        hrpd_rx_shift_hz: None,
        required_bandwidth_hz: None,
    };
    if !configure_rx || !bts_config.evdo.enabled {
        return Ok(plan);
    }

    let Some(evdo) = super::evdo::resolve_evdo_config(
        &bts_config.evdo,
        bts_config.pilot_offset,
        bts_config.channel,
        bts_config.runtime.tx_sample_rate_hz,
        bts_config.runtime.tx_bandwidth_hz,
    )?
    else {
        return Ok(plan);
    };

    if resolved_target == ReverseRxTarget::Hrpd {
        let hrpd_reverse_frequency_hz = evdo.evdo_reverse_frequency_hz;
        plan.target = ReverseRxTarget::Hrpd;
        plan.center_frequency_hz = hrpd_reverse_frequency_hz;
        plan.one_x_rx_shift_hz =
            one_x_reverse_frequency_hz as i64 - hrpd_reverse_frequency_hz as i64;
        plan.hrpd_reverse_frequency_hz = Some(hrpd_reverse_frequency_hz);
        plan.hrpd_rx_shift_hz = Some(0);
        return Ok(plan);
    }

    if resolved_target != ReverseRxTarget::Composite {
        return Ok(plan);
    }

    let hrpd_reverse_frequency_hz = evdo.evdo_reverse_frequency_hz;
    let carrier_separation_hz = one_x_reverse_frequency_hz.abs_diff(hrpd_reverse_frequency_hz);
    let required_bandwidth_hz = carrier_separation_hz + super::evdo::SR1_OCCUPIED_BANDWIDTH_HZ;
    if sample_rate_hz < required_bandwidth_hz {
        return Err(format!(
            "rx: EV-DO reverse composite requires rx_sample_rate_hz >= {} to capture 1x reverse {} Hz and HRPD reverse {} Hz (separation={} Hz, occupied half-BW={} Hz); configured rx_sample_rate_hz={}",
            required_bandwidth_hz,
            one_x_reverse_frequency_hz,
            hrpd_reverse_frequency_hz,
            carrier_separation_hz,
            super::evdo::SR1_OCCUPIED_HALF_BW_HZ,
            sample_rate_hz,
        )
        .into());
    }
    if bandwidth_hz < required_bandwidth_hz {
        return Err(format!(
            "rx: EV-DO reverse composite requires rx_bandwidth_hz >= {} to capture 1x reverse {} Hz and HRPD reverse {} Hz; configured rx_bandwidth_hz={}",
            required_bandwidth_hz,
            one_x_reverse_frequency_hz,
            hrpd_reverse_frequency_hz,
            bandwidth_hz,
        )
        .into());
    }

    let center_frequency_hz = (one_x_reverse_frequency_hz + hrpd_reverse_frequency_hz) / 2;
    plan.target = ReverseRxTarget::Composite;
    plan.center_frequency_hz = center_frequency_hz;
    plan.one_x_rx_shift_hz = one_x_reverse_frequency_hz as i64 - center_frequency_hz as i64;
    plan.hrpd_reverse_frequency_hz = Some(hrpd_reverse_frequency_hz);
    plan.hrpd_rx_shift_hz = Some(hrpd_reverse_frequency_hz as i64 - center_frequency_hz as i64);
    plan.required_bandwidth_hz = Some(required_bandwidth_hz);
    Ok(plan)
}

pub fn build_radio_from_config(
    radio_config: &RadioConfig,
    rx_freq_hz: usize,
    options: RadioBuildOptions,
) -> Result<Box<dyn Radio>, Error> {
    // Native radio libraries may create streaming workers during device and
    // streamer setup. Let UHD, libbladeRF, and LimeSuite inherit the requested
    // scheduling class, then restore the launcher thread before returning.
    let _driver_priority =
        super::realtime::DriverPriorityGuard::enter("radio-driver-init", &options.realtime);
    if options.null_radio {
        let mut radio = NoopRadio::new();
        if options.configure_rx {
            let rate = options.rx_sample_rate_hz;
            let bandwidth = options.rx_bandwidth_hz;
            radio.setup_rx(
                0,
                "",
                rx_freq_hz as f64,
                rate as f64,
                bandwidth as f64,
                None,
            )?;
            info!("Using null radio (TX dropped, dummy RX paced at {rate} Hz feeding silence)");
        } else {
            info!("Using null radio (TX dropped, no RX)");
        }
        return Ok(Box::new(radio));
    }
    match radio_config {
        RadioConfig::FileOutput { path } => Ok(Box::new(FileOutputRadio::new(
            fs::File::create(path)?,
            options.tx_sample_rate_hz,
        )?)),
        RadioConfig::Noop => {
            let mut radio = NoopRadio::new();
            if options.configure_rx {
                let rate = options.rx_sample_rate_hz;
                let bandwidth = options.rx_bandwidth_hz;
                radio.setup_rx(
                    0,
                    "",
                    rx_freq_hz as f64,
                    rate as f64,
                    bandwidth as f64,
                    None,
                )?;
                info!("Using noop radio (TX dropped, dummy RX paced at {rate} Hz feeding silence)");
            }
            Ok(Box::new(radio))
        }
        #[cfg(feature = "soapy-backend")]
        RadioConfig::Soapy {
            device,
            channel,
            antenna,
            rx_antenna,
            tx_gain_db,
            rx_gain_db,
            ..
        } => {
            let mut radio = SoapySdrRadio::new(device, *channel, antenna, *tx_gain_db)?;

            if options.configure_rx {
                let rx_ant = rx_antenna.as_deref().unwrap_or("LNAW");
                let freq_hz = rx_freq_hz as f64;
                let sample_rate_hz = options.rx_sample_rate_hz as f64;
                let bandwidth_hz = options.rx_bandwidth_hz as f64;
                radio.setup_rx(
                    *channel,
                    rx_ant,
                    freq_hz,
                    sample_rate_hz,
                    bandwidth_hz,
                    *rx_gain_db,
                )?;
                info!(
                    "rx: configured on shared device antenna={} freq={} rate={} bw={} gain={:?}",
                    rx_ant, freq_hz, sample_rate_hz, bandwidth_hz, rx_gain_db
                );
            } else {
                info!("rx: 1x reverse RX disabled by HRPD-only configuration");
            }

            Ok(Box::new(radio))
        }
        #[cfg(not(feature = "soapy-backend"))]
        RadioConfig::Soapy { .. } => {
            Err("SoapySDR backend not compiled in (enable 'soapy-backend' feature)".into())
        }
        #[cfg(feature = "uhd-backend")]
        RadioConfig::Uhd {
            device,
            channel,
            antenna,
            tx_gain_db,
            master_clock_rate,
            clock_source,
            time_source,
            rx_antenna,
            rx_gain_db,
            ..
        } => {
            let mut radio = crate::sdr::UhdRadio::new(
                device,
                *channel,
                antenna,
                *tx_gain_db,
                Some(*master_clock_rate),
                options.tx_sample_rate_hz,
            )?;
            if let Some(src) = clock_source {
                radio.set_clock_source(src)?;
            }
            if let Some(src) = time_source {
                radio.set_time_source(src)?;
            }
            if options.configure_rx {
                let rx_ant = rx_antenna.as_deref().unwrap_or("RX2");
                let freq_hz = rx_freq_hz as f64;
                let sample_rate_hz = options.rx_sample_rate_hz as f64;
                let bandwidth_hz = options.rx_bandwidth_hz as f64;
                radio.setup_rx(
                    *channel,
                    rx_ant,
                    freq_hz,
                    sample_rate_hz,
                    bandwidth_hz,
                    *rx_gain_db,
                )?;
                info!(
                    "rx: configured UHD RX antenna={} freq={} rate={} bw={} gain={:?}",
                    rx_ant, freq_hz, sample_rate_hz, bandwidth_hz, rx_gain_db
                );
            } else {
                info!("rx: 1x reverse RX disabled by HRPD-only configuration");
            }
            Ok(Box::new(radio))
        }
        #[cfg(not(feature = "uhd-backend"))]
        RadioConfig::Uhd { .. } => {
            Err("UHD radio backend not compiled in (enable 'uhd-backend' feature)".into())
        }
        #[cfg(feature = "lime-backend")]
        RadioConfig::Lime {
            device,
            channel,
            tx_antenna,
            tx_gain_db,
            rx_antenna,
            rx_gain_db,
            oversample,
            tx_lo_offset_hz,
            tx_fifo_size,
            rx_fifo_size,
            stream_throughput_vs_latency,
            ..
        } => {
            let mut radio = crate::sdr::LimeRadio::with_stream_config(
                device,
                *channel,
                tx_antenna,
                *tx_gain_db,
                options.tx_sample_rate_hz,
                oversample.unwrap_or(0),
                *tx_fifo_size,
                *rx_fifo_size,
                *stream_throughput_vs_latency,
            )?;
            if let Some(offset) = tx_lo_offset_hz {
                radio.set_tx_lo_offset(*offset)?;
            }
            if options.configure_rx {
                let rx_ant = rx_antenna.as_deref().unwrap_or("LNAW");
                radio.setup_rx(
                    *channel,
                    rx_ant,
                    rx_freq_hz as f64,
                    options.rx_sample_rate_hz as f64,
                    options.rx_bandwidth_hz as f64,
                    rx_gain_db.map(|g| g as f64),
                )?;
                info!(
                    "rx: configured LimeSDR RX antenna={} freq={} rate={} bw={} gain={:?}",
                    rx_ant,
                    rx_freq_hz,
                    options.rx_sample_rate_hz,
                    options.rx_bandwidth_hz,
                    rx_gain_db
                );
            } else {
                info!("rx: 1x reverse RX disabled by HRPD-only configuration");
            }
            Ok(Box::new(radio))
        }
        #[cfg(not(feature = "lime-backend"))]
        RadioConfig::Lime { .. } => {
            Err("LimeSDR backend not compiled in (enable 'lime-backend' feature)".into())
        }
        #[cfg(feature = "bladerf-backend")]
        RadioConfig::BladeRf {
            device,
            channel,
            fpga_path,
            tx_antenna,
            rx_antenna,
            tx_gain_db,
            rx_gain_db,
            tx_lo_offset_hz,
            num_buffers,
            buffer_size,
            num_transfers,
            stream_timeout_ms,
            ..
        } => {
            let mut radio = crate::sdr::BladeRfRadio::with_stream_config(
                device,
                *channel,
                *tx_gain_db,
                options.tx_sample_rate_hz as u32,
                fpga_path.as_deref(),
                tx_antenna.as_deref(),
                *num_buffers,
                *buffer_size,
                *num_transfers,
                *stream_timeout_ms,
            )?;
            if let Some(offset) = tx_lo_offset_hz {
                radio.set_tx_lo_offset(*offset)?;
            }
            if options.configure_rx {
                let rx_ant = rx_antenna.as_deref().unwrap_or("");
                let freq_hz = rx_freq_hz as f64;
                let sample_rate_hz = options.rx_sample_rate_hz as f64;
                let bandwidth_hz = options.rx_bandwidth_hz as f64;
                radio.setup_rx(
                    *channel as usize,
                    rx_ant,
                    freq_hz,
                    sample_rate_hz,
                    bandwidth_hz,
                    rx_gain_db.map(|g| g as f64),
                )?;
                info!(
                    "rx: configured bladeRF RX antenna='{}' freq={} rate={} bw={} gain={:?}",
                    rx_ant, freq_hz, sample_rate_hz, bandwidth_hz, rx_gain_db
                );
            } else {
                info!("rx: 1x reverse RX disabled by HRPD-only configuration");
            }
            Ok(Box::new(radio))
        }
        #[cfg(not(feature = "bladerf-backend"))]
        RadioConfig::BladeRf { .. } => {
            Err("bladeRF backend not compiled in (enable 'bladerf-backend' feature)".into())
        }
    }
}

pub struct BtsLaunchOptions {
    pub paging_ack_timeout_ms: u64,
    pub paging_max_retries: u32,
}

pub struct BtsLaunchParts {
    pub bts: Bts,
    pub handle: BtsHandle,
    pub resource_controller: Arc<TrafficResourceController>,
    pub lac_layer: lac::Layer2LacRef,
    pub mac_layer: Layer2MacRef,
    pub reverse_bearer_rx: StdReceiver<UdpBearerDatagram>,
    pub traffic_ack_seq_rx: mpsc::Receiver<(u8, u8)>,
    pub paging_state: Arc<parking_lot::Mutex<PagingSupplierState>>,
    pub pch_transmit_tx: broadcast::Sender<super::paging_supplier::PchTransmitEvent>,
    pub overhead: OverheadParameters,
    pub paging_settings: PagingChannelSettings,
}

pub fn build_bts_launch_parts(
    mut bts_config: BtsNodeConfig,
    radio: Box<dyn Radio>,
    options: BtsLaunchOptions,
) -> Result<BtsLaunchParts, Error> {
    bts_config.runtime.overhead.auth_mode = bts_config.overhead.auth_mode;
    bts_config.runtime.overhead.p_rev_in_use = bts_config.overhead.p_rev;

    let reverse_rx_plan = resolve_reverse_rx_plan(&bts_config)?;
    let hrpd_rx_overhead = if bts_config.evdo.enabled {
        Some(bts_config.evdo.overhead.resolve()?)
    } else {
        None
    };
    let has_rx = reverse_rx_plan.configure_rx;
    let (reverse_bearer_tx, reverse_bearer_rx) = channel();
    let (traffic_ack_seq_tx, traffic_ack_seq_rx) = mpsc::channel::<(u8, u8)>(256);
    let rx = if has_rx {
        Some(RxSettings {
            sample_rate_hz: reverse_rx_plan.sample_rate_hz,
            rx_center_frequency_hz: Some(reverse_rx_plan.center_frequency_hz),
            one_x_enabled: reverse_rx_plan.one_x_enabled,
            one_x_reverse_frequency_hz: reverse_rx_plan
                .one_x_enabled
                .then_some(reverse_rx_plan.one_x_reverse_frequency_hz),
            one_x_rx_shift_hz: reverse_rx_plan.one_x_rx_shift_hz,
            hrpd_reverse_frequency_hz: reverse_rx_plan.hrpd_reverse_frequency_hz,
            hrpd_rx_shift_hz: reverse_rx_plan.hrpd_rx_shift_hz,
            auth_mode: bts_config.overhead.auth_mode,
            p_rev_in_use: bts_config.overhead.p_rev,
            capture_iq_wav: None,
            capture_seconds: None,
            access_channel_number: bts_config
                .runtime
                .uplink
                .access_channel_numbers
                .first()
                .copied()
                .unwrap_or(0),
            paging_channel_number: bts_config.runtime.downlink.paging.paging_channel_number,
            base_id: bts_config.overhead.base_id,
            pilot_pn: bts_config.pilot_offset as u16,
            chip_rate_hz: bts_config.runtime.chip_rate_hz,
            absolute_chip_start: 0,
            hardware_start_time_ns: 0,
            tick_rate: bts_config.radio.tick_rate(bts_config.rf.rx_sample_rate_hz),
            access_event_tx: None,
            hrpd_access_event_tx: None,
            hrpd_traffic_event_tx: None,
            hrpd_access_cycle_number: 0,
            hrpd_access_sector_id_lsb: hrpd_rx_overhead
                .as_ref()
                .map(|o| o.sector_id24())
                .unwrap_or(0),
            hrpd_access_color_code: hrpd_rx_overhead.as_ref().map(|o| o.color_code).unwrap_or(0),
            hrpd_access_preamble_frames: hrpd_rx_overhead
                .as_ref()
                .map(|o| o.access_preamble_frames())
                .unwrap_or(crate::receiver::hrpd::access::HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES),
            hrpd_access_enhanced_rates: hrpd_rx_overhead
                .as_ref()
                .map(|o| o.enhanced_access_rates())
                .unwrap_or(false),
            reverse_bearer_tx: reverse_rx_plan.one_x_enabled.then_some(reverse_bearer_tx),
            rx_metrics_tx: None,
            reanchor_origin: true,
            traffic_rx_pool: None,
            hrpd_traffic_rx_queue: None,
            hrpd_harq_bus: None,
            traffic_channels: None,
            power_control: None,
            traffic_rx_removals: None,
            traffic_rx_continuity: bts_config.radio.traffic_rx_continuity(),
            overhead_mcc: bts_config
                .runtime
                .downlink
                .paging
                .message_defaults
                .extended_system_parameters
                .mcc,
            overhead_imsi_11_12: bts_config
                .runtime
                .downlink
                .paging
                .message_defaults
                .extended_system_parameters
                .imsi_11_12,
            rx_sample_delay: bts_config.radio.rx_sample_delay(),
            rx_batch_pcgs: bts_config.radio.rx_batch_pcgs(),
            tx_rx_anchor: None,
            reverse_access_finger_pool_size: bts_config
                .runtime
                .uplink
                .reverse_access_finger_pool_size,
            global_finger_pool_size: bts_config.runtime.uplink.global_finger_pool_size,
            traffic_ack_seq_tx: reverse_rx_plan.one_x_enabled.then_some(traffic_ack_seq_tx),
            rx_measurements: None,
        })
    } else {
        None
    };

    let (mac_to_lac_tx, mac_to_lac_rx) = channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    let channel_plan = bts_config.channel;
    let tx_override = bts_config.runtime.tx_freq_hz_override;
    let tx_center_frequency_hz = tx_override.unwrap_or_else(|| channel_plan.downlink_hz() as usize);
    let rx_center_frequency_hz = reverse_rx_plan.center_frequency_hz;
    info!(
        "channel plan: band_class={} subclass={} cdma_channel={} tx={:.4} MHz ({} Hz) rx_center={:.4} MHz ({} Hz) tx_override={}",
        channel_plan.band_class.as_str(),
        channel_plan.band_subclass,
        channel_plan.cdma_channel,
        tx_center_frequency_hz as f64 / 1_000_000.0,
        tx_center_frequency_hz,
        rx_center_frequency_hz as f64 / 1_000_000.0,
        rx_center_frequency_hz,
        tx_override
            .map(|hz| format!("{hz}"))
            .unwrap_or_else(|| "none".into()),
    );
    if let Some(hrpd_reverse_frequency_hz) = reverse_rx_plan.hrpd_reverse_frequency_hz {
        match reverse_rx_plan.target {
            ReverseRxTarget::Hrpd => info!(
                "rx: EV-DO reverse direct HRPD center {:.4} MHz; HRPD reverse {:.4} MHz shift {:+.3} kHz; 1x RX disabled; rate {:.4} MHz bw {:.4} MHz",
                reverse_rx_plan.center_frequency_hz as f64 / 1_000_000.0,
                hrpd_reverse_frequency_hz as f64 / 1_000_000.0,
                reverse_rx_plan.hrpd_rx_shift_hz.unwrap_or(0) as f64 / 1_000.0,
                reverse_rx_plan.sample_rate_hz as f64 / 1_000_000.0,
                reverse_rx_plan.bandwidth_hz as f64 / 1_000_000.0,
            ),
            _ => info!(
                "rx: EV-DO reverse composite center {:.4} MHz; 1x reverse {:.4} MHz shift {:+.3} kHz; HRPD reverse {:.4} MHz shift {:+.3} kHz; rate {:.4} MHz bw {:.4} MHz required {:.4} MHz",
                reverse_rx_plan.center_frequency_hz as f64 / 1_000_000.0,
                reverse_rx_plan.one_x_reverse_frequency_hz as f64 / 1_000_000.0,
                reverse_rx_plan.one_x_rx_shift_hz as f64 / 1_000.0,
                hrpd_reverse_frequency_hz as f64 / 1_000_000.0,
                reverse_rx_plan.hrpd_rx_shift_hz.unwrap_or(0) as f64 / 1_000.0,
                reverse_rx_plan.sample_rate_hz as f64 / 1_000_000.0,
                reverse_rx_plan.bandwidth_hz as f64 / 1_000_000.0,
                reverse_rx_plan.required_bandwidth_hz.unwrap_or(0) as f64 / 1_000_000.0,
            ),
        }
    }
    let derived_cdma_freq = channel_plan.cdma_freq_field();
    let derived_band_class = channel_plan.band_class.field_value();
    if bts_config.overhead.cdma_freq.is_none() {
        bts_config.overhead.cdma_freq = Some(derived_cdma_freq);
    }
    if bts_config.overhead.ext_cdma_freq.is_none() {
        bts_config.overhead.ext_cdma_freq = Some(derived_cdma_freq);
    }
    if bts_config.overhead.band_class.is_none() {
        bts_config.overhead.band_class = Some(derived_band_class);
    }
    let cdma_freq = bts_config.overhead.cdma_freq.unwrap_or(derived_cdma_freq);
    let ext_cdma_freq = bts_config
        .overhead
        .ext_cdma_freq
        .unwrap_or(derived_cdma_freq);
    let evdo_config = super::evdo::resolve_evdo_config(
        &bts_config.evdo,
        bts_config.pilot_offset,
        bts_config.channel,
        bts_config.runtime.tx_sample_rate_hz,
        bts_config.runtime.tx_bandwidth_hz,
    )?;
    if let Some(evdo) = &evdo_config {
        match evdo.tx_mode {
            super::evdo::EvdoTxMode::AdjacentComposite => info!(
                "EV-DO TX mode: single-RF composite; composite center {:.04} MHz, 1x shift {:+.03} kHz, HRPD shift {:+.03} kHz",
                evdo.composite_center_frequency_hz as f64 / 1_000_000.0,
                evdo.one_x_shift_hz as f64 / 1_000.0,
                evdo.evdo_shift_hz as f64 / 1_000.0,
            ),
            super::evdo::EvdoTxMode::HrpdOnly => info!(
                "EV-DO TX mode: HRPD-only (unsupported/untested); HRPD bc{} ch{} {:.04} MHz, reverse {:.04} MHz; 1x TX/RX disabled",
                evdo.evdo_band_class,
                evdo.evdo_channel,
                evdo.evdo_frequency_hz as f64 / 1_000_000.0,
                evdo.evdo_reverse_frequency_hz as f64 / 1_000_000.0,
            ),
        }
    }
    let _ = cdma_freq; // legacy CDMA_FREQ field still used in overhead encoding
    let paging_settings = bts_config.runtime.downlink.paging.clone();
    let mac_layer_for_bts = mac_layer.clone();
    let rx_power_adj = bts_config.radio.rx_power_adj();
    let (bts, handle) = Bts::new_with_settings(
        radio,
        Config {
            tx_center_frequency_hz,
            pilot_offset: bts_config.pilot_offset,
            mac_layer: mac_layer_for_bts,
            start_system_time: None,
            sync_channel_template: Some(SyncChannelMessage {
                pd: 0,
                msg_type: 1,
                p_rev: bts_config.overhead.p_rev,
                min_p_rev: bts_config.overhead.min_p_rev,
                sid: bts_config.overhead.sid,
                nid: bts_config.overhead.nid,
                pilot_pn: bts_config.pilot_offset as u16,
                lc_state: 0,
                sys_time: 0,
                lp_sec: bts_config.overhead.lp_sec,
                ltm_off: bts_config.overhead.ltm_off,
                daylt: bts_config.overhead.daylt,
                prat: bts_config.overhead.prat,
                cdma_freq,
                ext_cdma_freq,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            }),
            timezone: bts_config.timezone.clone(),
            overhead: bts_config.overhead.clone(),
            rx,
            evdo: evdo_config.clone(),
        },
        bts_config.runtime.clone(),
    );
    handle.power_control.set_rx_power_adj_dbfs(rx_power_adj);
    info!(
        "rx: power-control dBFS threshold adjustment={:+.2} dB",
        rx_power_adj
    );

    let overhead = bts_config.overhead.clone();
    let (pch_transmit_tx, _pch_transmit_rx) =
        broadcast::channel::<super::paging_supplier::PchTransmitEvent>(256);
    let paging_state = {
        let mut state = PagingSupplierState::new_with_retry_config(
            PagingRetryConfig {
                ack_timeout_ms: options.paging_ack_timeout_ms,
                max_retries: options.paging_max_retries,
            },
            paging_settings
                .message_defaults
                .extended_system_parameters
                .mcc,
            paging_settings
                .message_defaults
                .extended_system_parameters
                .imsi_11_12,
        );
        state.set_pch_transmit_tx(pch_transmit_tx.clone());
        Arc::new(parking_lot::Mutex::new(state))
    };
    let paging_supplier = build_bts_paging_supplier(
        overhead.clone(),
        paging_settings.clone(),
        bts_config.pilot_offset,
        evdo_config.as_ref().and_then(|evdo| evdo.advertisement()),
        paging_state.clone(),
    );
    lac_layer.set_paging_supplier(paging_supplier);
    info!("BTS-local paging supplier installed on LAC layer");

    let resource_controller = Arc::new(TrafficResourceController::from_pools(
        handle.walsh_allocator.clone(),
        handle.traffic_channels.clone(),
        handle.traffic_rx_pool.clone(),
        handle.traffic_rx_removals.clone(),
        handle.power_control.clone(),
    ));

    Ok(BtsLaunchParts {
        bts,
        handle,
        resource_controller,
        lac_layer,
        mac_layer,
        reverse_bearer_rx,
        traffic_ack_seq_rx,
        paging_state,
        pch_transmit_tx,
        overhead,
        paging_settings,
    })
}

pub struct LocalAbisEndpointConfig {
    pub bind_addr: SocketAddr,
    pub bearer_bind_addr: SocketAddr,
    pub bearer_remote_addr: SocketAddr,
    pub pilot_pn: u16,
    pub cell_id: cdma_abis::control::typed::CellId,
    pub mscid: u32,
}

pub async fn spawn_local_abis_endpoint(
    config: LocalAbisEndpointConfig,
    controller: Arc<TrafficResourceController>,
    reverse_bearer_rx: StdReceiver<UdpBearerDatagram>,
    mut traffic_ack_seq_rx: mpsc::Receiver<(u8, u8)>,
    paging_state: Arc<parking_lot::Mutex<PagingSupplierState>>,
    mut access_events: mpsc::UnboundedReceiver<super::AccessChannelEvent>,
) -> Result<SocketAddr, Error> {
    let bearer_config = cdma_abis::bearer_transport::BearerTransportConfig {
        bind_addr: config.bearer_bind_addr,
        remote_addr: config.bearer_remote_addr,
        bts_id: config.cell_id.cell as u32,
        cell_id: config.cell_id.sector as u32,
    };
    let bearer = Arc::new(
        cdma_abis::bearer_transport::BearerTransport::new(&bearer_config)
            .map_err(|e| Error::from(format!("failed to create BTS bearer transport: {e}")))?,
    );
    super::bearer_agent::spawn_bts_bearer_agent(bearer, controller.clone(), reverse_bearer_rx);

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .map_err(|e| {
            Error::from(format!(
                "failed to bind BTS Abis listener at {}: {e}",
                config.bind_addr
            ))
        })?;
    let bind_addr = listener
        .local_addr()
        .map_err(|e| Error::from(format!("failed to read BTS Abis listener address: {e}")))?;
    let agent_config = AbisAgentConfig {
        pilot_pn: config.pilot_pn,
        cell_id: config.cell_id,
        mscid: config.mscid,
    };
    let controller_for_agent = controller.clone();
    let controller_for_frames = controller.clone();
    let agent_cell_id = config.cell_id;
    tokio::spawn(async move {
        let (sender, mut events_rx) = match cdma_abis::transport::accept(&listener).await {
            Ok(pair) => pair,
            Err(e) => {
                warn!("BTS Abis accept failed: {e}");
                return;
            }
        };
        let mut agent = AbisAgent::new(agent_config, controller_for_agent);
        agent.set_paging_state(paging_state.clone());
        let mut tick_interval = tokio::time::interval(std::time::Duration::from_secs(1));
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let deliver_agent_events = |events: Vec<AbisAgentEvent>,
                                    ctrl: &Arc<TrafficResourceController>|
         -> Vec<cdma_abis::control::AbisMessage> {
            let mut abis_responses = Vec::new();
            for event in events {
                match event {
                    AbisAgentEvent::ForwardTrafficFrames { walsh_code, frames } => {
                        if let Some(slot) = ctrl.traffic_channels_pool().lookup(walsh_code) {
                            for frame in frames {
                                slot.channel.send_signaling_bits(frame.bits().to_vec());
                            }
                        } else {
                            warn!("BTS: ForwardTrafficFrames for unknown walsh={}", walsh_code);
                        }
                    }
                    AbisAgentEvent::TrafficConnected { ccr, walsh_code } => {
                        info!("BTS: TrafficConnected ccr={:?} walsh={}", ccr, walsh_code);
                    }
                    AbisAgentEvent::TrafficReleased { ccr, walsh_code } => {
                        info!("BTS: TrafficReleased ccr={:?} walsh={}", ccr, walsh_code);
                    }
                    AbisAgentEvent::BtsReleaseInitiated { ccr, walsh_code } => {
                        info!(
                            "BTS: BtsReleaseInitiated ccr={:?} walsh={}",
                            ccr, walsh_code
                        );
                    }
                    AbisAgentEvent::PagingRetryFailed { responses } => {
                        abis_responses.extend(responses);
                    }
                }
            }
            abis_responses
        };

        loop {
            tokio::select! {
                event = events_rx.recv() => {
                    match event {
                        Some(cdma_abis::transport::TransportEvent::Message(msg)) => {
                            let (responses, events) = agent.handle_message(&msg);
                            for response in responses {
                                if let Err(e) = sender.send(&response).await {
                                    warn!("BTS Abis send failed: {e}");
                                }
                            }
                            let abis_responses = deliver_agent_events(events, &controller_for_frames);
                            for resp in abis_responses {
                                if let Err(e) = sender.send(&resp).await {
                                    warn!("BTS Abis send failed: {e}");
                                }
                            }
                        }
                        Some(cdma_abis::transport::TransportEvent::Disconnected(e)) => {
                            warn!("BTS Abis disconnected: {e}");
                            break;
                        }
                        None => break,
                    }
                }
                ack = traffic_ack_seq_rx.recv() => {
                    let Some((walsh_code, ack_seq)) = ack else { break };
                    let events = agent.handle_reverse_ack_seq(walsh_code, ack_seq);
                    let abis_responses = deliver_agent_events(events, &controller_for_frames);
                    for resp in abis_responses {
                        if let Err(e) = sender.send(&resp).await {
                            warn!("BTS Abis send failed: {e}");
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    let events = agent.tick_all_sessions();
                    let abis_responses = deliver_agent_events(events, &controller_for_frames);
                    for resp in abis_responses {
                        if let Err(e) = sender.send(&resp).await {
                            warn!("BTS Abis send failed: {e}");
                        }
                    }
                    let paging_events = agent.tick_paging_retries();
                    let paging_abis_responses = deliver_agent_events(paging_events, &controller_for_frames);
                    for resp in paging_abis_responses {
                        if let Err(e) = sender.send(&resp).await {
                            warn!("BTS Abis send failed: {e}");
                        }
                    }
                }
                access = access_events.recv() => {
                    let Some(access_event) = access else { break };
                    agent.record_access_msg_seq(&access_event);
                    let l2_ack_responses = agent.check_access_ack_notify(&access_event);
                    for resp in l2_ack_responses {
                        if let Err(e) = sender.send(&resp).await {
                            warn!("BTS Abis L2 ack send failed: {e}");
                        }
                    }
                    let page_response_acks = agent.check_page_response_cancel(&access_event);
                    for resp in page_response_acks {
                        if let Err(e) = sender.send(&resp).await {
                            warn!("BTS Abis page-response ack send failed: {e}");
                        }
                    }
                    let raw_bits = match &access_event.raw_pdu_bits {
                        Some(bits) => bits.clone(),
                        None => continue,
                    };
                    let msg_type = access_event
                        .message_id
                        .wire_type(crate::lac::message_types::WireChannel::ReverseCommon)
                        .unwrap_or(0);
                    let octets: Vec<u8> = raw_bits
                        .chunks(8)
                        .map(|chunk| {
                            let mut byte = 0u8;
                            for (i, &bit) in chunk.iter().enumerate() {
                                byte |= (bit & 1) << (7 - i);
                            }
                            byte
                        })
                        .collect();
                    let mut mobile_ids = Vec::new();
                    if let Some(imsi) = access_event.imsi.as_ref() {
                        mobile_ids.push(cdma_abis::control::typed::MobileIdentity::Imsi(
                            imsi.clone(),
                        ));
                    }
                    if let Some(esn) = access_event.esn {
                        mobile_ids.push(cdma_abis::control::typed::MobileIdentity::Esn(esn));
                    }
                    let ach = cdma_abis::control::AchMessageTransferMessage {
                        correlation_id: None,
                        mobile_identities: mobile_ids,
                        cell_identifier: Some(agent_cell_id),
                        bts_l2_termination: None,
                        air_interface_message: Some(
                            cdma_abis::control::typed::AirInterfaceMessagePayload {
                                message_type: msg_type,
                                message: octets,
                            },
                        ),
                        cdma_serving_one_way_delay:
                            cdma_abis::control::typed::CdmaServingOneWayDelay {
                                cell: agent_cell_id,
                                delay_100ns: 0,
                            },
                        authentication_challenge_parameter: None,
                    };
                    match ach.encode() {
                        Ok(bytes) => match cdma_abis::control::decode(&bytes) {
                            Ok(abis_msg) => {
                                info!(
                                    "BTS→BSC Abis ACH Msg Transfer: {}",
                                    access_event.msg_type_name
                                );
                                if let Err(e) = sender.send(&abis_msg).await {
                                    warn!("BTS Abis ACH send failed: {e}");
                                }
                            }
                            Err(e) => {
                                warn!("BTS Abis ACH decode failed: {e}");
                            }
                        },
                        Err(e) => {
                            warn!("BTS Abis ACH encode failed: {e}");
                        }
                    }
                }
            }
        }
    });

    Ok(bind_addr)
}

pub async fn spawn_configured_local_abis_endpoint(
    bts_config: &BtsNodeConfig,
    resource_controller: Arc<TrafficResourceController>,
    reverse_bearer_rx: StdReceiver<UdpBearerDatagram>,
    traffic_ack_seq_rx: mpsc::Receiver<(u8, u8)>,
    paging_state: Arc<parking_lot::Mutex<PagingSupplierState>>,
    access_events: mpsc::UnboundedReceiver<cdma_common::events::AccessChannelEvent>,
) -> Result<SocketAddr, Error> {
    spawn_local_abis_endpoint(
        LocalAbisEndpointConfig {
            bind_addr: bts_config.abis.bind_addr,
            bearer_bind_addr: bts_config.bearer.bind_addr,
            bearer_remote_addr: bts_config.bearer.remote_addr,
            pilot_pn: bts_config.pilot_offset as u16,
            cell_id: cdma_abis::control::typed::CellId {
                cell: bts_config.overhead.base_id,
                sector: 0x01,
            },
            mscid: bts_config.overhead.sid as u32,
        },
        resource_controller,
        reverse_bearer_rx,
        traffic_ack_seq_rx,
        paging_state,
        access_events,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn bts_launch_parts_build_from_default_config() {
        let mut config = BtsNodeConfig::default();
        config.radio = RadioConfig::Noop;
        let parts = build_bts_launch_parts(
            config,
            Box::new(NoopRadio::new()),
            BtsLaunchOptions {
                paging_ack_timeout_ms: 100,
                paging_max_retries: 0,
            },
        )
        .expect("default launch parts should build");
        assert_eq!(parts.overhead.cdma_freq, Some(384));
        assert_eq!(parts.paging_settings.paging_channel_number, 1);
    }

    #[test]
    fn hrpd_only_mode_infers_direct_hrpd_uplink_capture() {
        let mut bts = BtsNodeConfig::default();
        bts.radio = RadioConfig::Noop;
        bts.channel.cdma_channel = 777;
        bts.evdo.enabled = true;
        bts.evdo.channel = Some(630);
        bts.evdo.mode = super::super::evdo::EvdoMode::HrpdOnly;
        bts.evdo.overhead.sector_id = Some(super::super::evdo::HrpdSectorId::new([0; 16]));
        bts.evdo.overhead.subnet_mask = Some(26);
        bts.evdo.overhead.color_code = Some(26);
        bts.rf = crate::bts::config::BtsRfProfile::derive(bts.channel, &bts.evdo)
            .expect("derive HRPD-only RF profile");
        bts.runtime.tx_sample_rate_hz = bts.rf.tx_sample_rate_hz;
        bts.runtime.tx_bandwidth_hz = bts.rf.tx_bandwidth_hz;

        let plan = resolve_reverse_rx_plan(&bts).expect("resolve reverse RX plan");
        assert!(plan.configure_rx);
        assert_eq!(plan.target, ReverseRxTarget::Hrpd);
        assert!(!plan.one_x_enabled);
        assert_eq!(plan.center_frequency_hz, 843_900_000);
        assert_eq!(plan.one_x_reverse_frequency_hz, 848_310_000);
        assert_eq!(plan.one_x_rx_shift_hz, 4_410_000);
        assert_eq!(plan.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert_eq!(plan.hrpd_rx_shift_hz, Some(0));
        assert_eq!(plan.sample_rate_hz, 4_915_200);
        assert_eq!(plan.bandwidth_hz, 1_500_000);
        assert_eq!(plan.required_bandwidth_hz, None);

        let parts = build_bts_launch_parts(
            bts,
            Box::new(NoopRadio::new()),
            BtsLaunchOptions {
                paging_ack_timeout_ms: 100,
                paging_max_retries: 0,
            },
        )
        .expect("build HRPD-only launch parts");
        let rx = parts.bts.config.rx.as_ref().expect("HRPD RX settings");
        assert_eq!(rx.one_x_reverse_frequency_hz, None);
        assert_eq!(rx.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert!(rx.reverse_bearer_tx.is_none());
        assert!(rx.traffic_ack_seq_tx.is_none());
    }

    #[test]
    fn reverse_rx_plan_centers_explicit_composite_uplink_capture() {
        // EV-DO ships disabled by default; this test covers the composite plan.
        let bts = BtsNodeConfig::load_evdo_enabled_for_test(&fixture_path("../../config/bts.json"))
            .expect("load BTS config");

        let plan = resolve_reverse_rx_plan(&bts).expect("resolve reverse RX plan");
        assert!(plan.configure_rx);
        assert_eq!(plan.target, ReverseRxTarget::Composite);
        assert_eq!(plan.center_frequency_hz, 844_815_000);
        assert_eq!(plan.one_x_reverse_frequency_hz, 845_730_000);
        assert_eq!(plan.one_x_rx_shift_hz, 915_000);
        assert_eq!(plan.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert_eq!(plan.hrpd_rx_shift_hz, Some(-915_000));
        assert_eq!(plan.sample_rate_hz, 4_915_200);
        assert_eq!(plan.bandwidth_hz, 3_310_000);
        assert_eq!(plan.required_bandwidth_hz, Some(3_310_000));
    }

    #[test]
    fn default_null_radio_resolves_composite_evdo_reverse_plan() {
        // With no radio configured, bts.json falls back to the null radio,
        // which is treated as RX-capable so the EV-DO reverse composite
        // pipeline runs against the dummy RX without hardware. EV-DO ships
        // disabled by default, so load it enabled for the composite plan.
        let bts = BtsNodeConfig::load_evdo_enabled_for_test(&fixture_path("../../config/bts.json"))
            .expect("load BTS config");
        assert!(matches!(bts.radio, RadioConfig::Noop));

        let plan = resolve_reverse_rx_plan(&bts).expect("resolve reverse RX plan");
        assert!(plan.configure_rx);
        assert_eq!(plan.target, ReverseRxTarget::Composite);
        assert_eq!(plan.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert_eq!(plan.sample_rate_hz, 4_915_200);
        assert_eq!(plan.bandwidth_hz, 3_310_000);
        assert_eq!(plan.required_bandwidth_hz, Some(3_310_000));
    }
}
