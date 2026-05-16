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
    RxSettings, TrafficResourceController,
    abis_agent::{AbisAgent, AbisAgentConfig, AbisAgentEvent},
    paging_supplier::{PagingRetryConfig, PagingSupplierState, build_bts_paging_supplier},
};

pub struct RadioBuildOptions {
    pub null_radio: bool,
}

pub fn build_radio_from_config(
    radio_config: &RadioConfig,
    options: RadioBuildOptions,
) -> Result<Box<dyn Radio>, Error> {
    if options.null_radio {
        info!("Using null radio (TX dropped, no RX)");
        return Ok(Box::new(NoopRadio::new()));
    }
    match radio_config {
        RadioConfig::FileOutput { path } => {
            Ok(Box::new(FileOutputRadio::new(fs::File::create(path)?)?))
        }
        RadioConfig::Noop => Ok(Box::new(NoopRadio::new())),
        #[cfg(feature = "soapy-backend")]
        RadioConfig::Soapy {
            device,
            channel,
            antenna,
            rx_antenna,
            tx_gain_db,
            rx_gain_db,
            rx_freq_hz,
            rx_sample_rate_hz,
            rx_bandwidth_hz,
            ..
        } => {
            let mut radio = SoapySdrRadio::new(device, *channel, antenna, *tx_gain_db)?;

            let rx_ant = rx_antenna.as_deref().unwrap_or("LNAW");
            let freq_hz = *rx_freq_hz as f64;
            let sample_rate_hz = *rx_sample_rate_hz as f64;
            let bandwidth_hz = rx_bandwidth_hz.unwrap_or(*rx_sample_rate_hz) as f64;
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
            rx_freq_hz,
            rx_sample_rate_hz,
            rx_bandwidth_hz,
            ..
        } => {
            let mut radio = crate::sdr::UhdRadio::new(
                device,
                *channel,
                antenna,
                *tx_gain_db,
                Some(*master_clock_rate),
            )?;
            if let Some(src) = clock_source {
                radio.set_clock_source(src)?;
            }
            if let Some(src) = time_source {
                radio.set_time_source(src)?;
            }
            let rx_ant = rx_antenna.as_deref().unwrap_or("RX2");
            let freq_hz = *rx_freq_hz as f64;
            let sample_rate_hz = *rx_sample_rate_hz as f64;
            let bandwidth_hz = rx_bandwidth_hz.unwrap_or(*rx_sample_rate_hz) as f64;
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
            rx_freq_hz,
            rx_sample_rate_hz,
            rx_bandwidth_hz,
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
                *rx_sample_rate_hz,
                oversample.unwrap_or(0),
                *tx_fifo_size,
                *rx_fifo_size,
                *stream_throughput_vs_latency,
            )?;
            if let Some(offset) = tx_lo_offset_hz {
                radio.set_tx_lo_offset(*offset)?;
            }
            let rx_ant = rx_antenna.as_deref().unwrap_or("LNAW");
            radio.setup_rx(
                *channel,
                rx_ant,
                *rx_freq_hz as f64,
                *rx_sample_rate_hz as f64,
                rx_bandwidth_hz.unwrap_or(*rx_sample_rate_hz) as f64,
                rx_gain_db.map(|g| g as f64),
            )?;
            info!(
                "rx: configured LimeSDR RX antenna={} freq={} rate={} bw={} gain={:?}",
                rx_ant,
                rx_freq_hz,
                rx_sample_rate_hz,
                rx_bandwidth_hz.unwrap_or(*rx_sample_rate_hz),
                rx_gain_db
            );
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
            rx_freq_hz,
            rx_sample_rate_hz,
            rx_bandwidth_hz,
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
                *rx_sample_rate_hz as u32,
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
            let rx_ant = rx_antenna.as_deref().unwrap_or("");
            let freq_hz = *rx_freq_hz as f64;
            let sample_rate_hz = *rx_sample_rate_hz as f64;
            let bandwidth_hz = rx_bandwidth_hz.unwrap_or(*rx_sample_rate_hz) as f64;
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
) -> BtsLaunchParts {
    bts_config.runtime.overhead.auth_mode = bts_config.overhead.auth_mode;
    bts_config.runtime.overhead.p_rev_in_use = bts_config.overhead.p_rev;

    let rx_sample_rate = bts_config.radio.rx_sample_rate_hz();
    let has_rx = matches!(
        bts_config.radio,
        RadioConfig::Soapy { .. }
            | RadioConfig::Uhd { .. }
            | RadioConfig::Lime { .. }
            | RadioConfig::BladeRf { .. }
    );
    let (reverse_bearer_tx, reverse_bearer_rx) = channel();
    let (traffic_ack_seq_tx, traffic_ack_seq_rx) = mpsc::channel::<(u8, u8)>(256);
    let rx = if has_rx {
        Some(RxSettings {
            sample_rate_hz: rx_sample_rate,
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
            tick_rate: bts_config.radio.tick_rate(),
            access_event_tx: None,
            reverse_bearer_tx: Some(reverse_bearer_tx),
            rx_metrics_tx: None,
            reanchor_origin: true,
            traffic_rx_pool: None,
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
            traffic_ack_seq_tx: Some(traffic_ack_seq_tx),
            rx_measurements: None,
        })
    } else {
        None
    };

    let (mac_to_lac_tx, mac_to_lac_rx) = channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    if bts_config.overhead.cdma_freq == 0 {
        let freq = bts_config.runtime.tx_center_frequency_hz;
        if freq >= 870_000_000 {
            bts_config.overhead.cdma_freq = ((freq - 870_000_000) / 30_000) as u16;
        }
    }
    let cdma_freq = bts_config.overhead.cdma_freq;
    let paging_settings = bts_config.runtime.downlink.paging.clone();
    let mac_layer_for_bts = mac_layer.clone();
    let rx_power_adj = bts_config.radio.rx_power_adj();
    let (bts, handle) = Bts::new_with_settings(
        radio,
        Config {
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
                ext_cdma_freq: bts_config.overhead.ext_cdma_freq,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            }),
            timezone: bts_config.timezone.clone(),
            overhead: bts_config.overhead.clone(),
            rx,
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
        paging_state.clone(),
    );
    lac_layer.set_paging_supplier(paging_supplier);
    info!("BTS-local paging supplier installed on LAC layer");

    let resource_controller = Arc::new(TrafficResourceController::from_pools(
        handle.walsh_allocator.clone(),
        handle.traffic_channels.clone(),
        handle.traffic_rx_pool.clone(),
        handle.traffic_rx_removals.clone(),
    ));

    BtsLaunchParts {
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
    }
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
        );
        assert_eq!(parts.overhead.cdma_freq, 384);
        assert_eq!(parts.paging_settings.paging_channel_number, 1);
    }
}
