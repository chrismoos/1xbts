//! BTS runtime and PHY channel settings.
//!
//! In-memory settings the TX synth and RX path operate on: per-channel PHY
//! configuration (pilot/sync/paging/overhead, downlink/uplink), interleaver and
//! spreading parameters, and `RxSettings`. Distinct from the operator-facing
//! `BtsNodeConfig` in `super::config`, which is the `config/bts.json` node
//! configuration loaded and resolved into these runtime settings.

use std::{
    collections::HashMap, path::PathBuf, sync::Arc, sync::Mutex as StdMutex, sync::mpsc as std_mpsc,
};

use cdma_abis::udp_bearer::UdpBearerDatagram;
use cdma_common::error::Error;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use super::handle::{HrpdTrafficRxQueue, TrafficChannelPool};
use super::handle::{RxMetrics, TrafficRxPool, TrafficRxRemovals};
use super::{BtsPowerControlRegistry, TxRxAnchor};

use crate::{
    lac::paging_messages::{
        AccessParametersMessage, AlternativeHrpdNeighborRecord,
        AlternativeHrpdNeighborSubnetColorCode, AlternativeHrpdRadioInterface,
        AlternativeTechnologiesInformationMessage, AlternativeTechnologyRadioInterfaceRecord,
        CdmaChannelListMessage, ExtendedSystemParametersMessage, GeneralPageMessage,
        NeighborListMessage, OrderMessage, PagingChannelMessage, PagingMessageDefaults,
        PagingMessageKind, SystemParametersMessage,
    },
    phy::coding::block_interleaver::{InterleaverParams, SR1_PARAMS_128, SR1_PARAMS_384},
};

use super::evdo::Evdo1xAdvertisement;

use cdma_common::consts::SR1_CHIP_RATE_HZ;
const SR1_SHORT_CODE_LENGTH_CHIPS: usize = 32_768;
const SR1_WALSH_LENGTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadingRate {
    Sr1,
    Sr3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct InterleaverSettings {
    pub block_size: usize,
    pub m: usize,
    pub j: usize,
}

impl Default for InterleaverSettings {
    fn default() -> Self {
        let p = SR1_PARAMS_128;
        Self {
            block_size: p.block_size,
            m: p.m,
            j: p.j,
        }
    }
}

impl InterleaverSettings {
    pub(crate) fn as_params(&self) -> InterleaverParams {
        InterleaverParams {
            block_size: self.block_size,
            m: self.m,
            j: self.j,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PilotChannelSettings {
    pub walsh_code: usize,
    pub gain: f32,
}

impl Default for PilotChannelSettings {
    fn default() -> Self {
        Self {
            walsh_code: 0,
            // -7 dB => 10^(-7/20) ~= 0.4466836
            gain: 0.4466836,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncChannelSettings {
    pub walsh_code: usize,
    pub walsh_repetition: usize,
    pub data_rate_bps: usize,
    pub symbol_repeat: usize,
    pub interleaver: InterleaverSettings,
    pub gain: f32,
    pub availability_max_size_bits: usize,
}

impl Default for SyncChannelSettings {
    fn default() -> Self {
        let p = SR1_PARAMS_128;
        Self {
            walsh_code: 32,
            walsh_repetition: 4,
            data_rate_bps: 1200,
            symbol_repeat: 2,
            interleaver: InterleaverSettings {
                block_size: p.block_size,
                m: p.m,
                j: p.j,
            },
            // -13.3 dB => 10^(-13.3/20) ~= 0.21627898
            gain: 0.21627898,
            availability_max_size_bits: 32,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PagingChannelSettings {
    pub walsh_code: usize,
    pub paging_channel_number: u8,
    pub data_rate_bps: usize,
    pub interleaver: InterleaverSettings,
    pub gain: f32,
    pub availability_max_size_bits: usize,
    pub bypass_long_code: bool,
    pub force_zero_payload_bits: bool,
    pub message_defaults: PagingMessageDefaults,
}

impl Default for PagingChannelSettings {
    fn default() -> Self {
        let p = SR1_PARAMS_384;
        Self {
            walsh_code: 1,
            paging_channel_number: 1,
            data_rate_bps: 9600,
            interleaver: InterleaverSettings {
                block_size: p.block_size,
                m: p.m,
                j: p.j,
            },
            // -7.3 dB => 10^(-7.3/20) ~= 0.43151583
            gain: 0.43151583,
            availability_max_size_bits: 96,
            bypass_long_code: false,
            force_zero_payload_bits: false,
            message_defaults: PagingMessageDefaults::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DownlinkSettings {
    pub pilot: PilotChannelSettings,
    pub sync: SyncChannelSettings,
    pub paging: PagingChannelSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UplinkSettings {
    pub access_channels_per_paging_channel: usize,
    pub access_channel_numbers: Vec<u8>,
    pub access_channel_rate_bps: usize,
    pub access_frame_ms: usize,
    pub require_r_csch_f_csch_arq_ack: bool,
    pub arq_ack_timeout_ms: usize,
    /// Thread pool size for parallel reverse-access finger feeding.
    /// Falls back to `global_finger_pool_size` when not set (0).
    pub reverse_access_finger_pool_size: usize,
    /// Default thread pool size for finger feeding on all other channels
    /// (reverse traffic, etc.).
    pub global_finger_pool_size: usize,
}

impl Default for UplinkSettings {
    fn default() -> Self {
        Self {
            access_channels_per_paging_channel: 1,
            access_channel_numbers: vec![0],
            access_channel_rate_bps: 4800,
            access_frame_ms: 20,
            require_r_csch_f_csch_arq_ack: true,
            arq_ack_timeout_ms: 400,
            reverse_access_finger_pool_size: 8,
            global_finger_pool_size: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OverheadSettings {
    pub fragment_availability_interval_chips: usize,
    pub sync_superframe_interval_chips: usize,
    pub t1b_ms: usize,
    /// System AUTH_MODE from the serving overhead state.
    pub auth_mode: u8,
    /// Serving P_REV_IN_USE used to interpret conditionally-present reverse
    /// access-channel fields.
    pub p_rev_in_use: u8,
    pub require_spm: bool,
    pub require_apm: bool,
    pub require_cclm: bool,
    pub require_espm: bool,
}

impl Default for OverheadSettings {
    fn default() -> Self {
        Self {
            fragment_availability_interval_chips: 32_768,
            sync_superframe_interval_chips: 98_304,
            t1b_ms: 640,
            auth_mode: 0,
            p_rev_in_use: 11,
            require_spm: true,
            require_apm: true,
            require_cclm: true,
            require_espm: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RealtimeSettings {
    /// Request real-time scheduling for radio and baseband threads.
    pub enabled: bool,
    /// Linux SCHED_FIFO priority for the TX thread.
    pub tx_priority: i32,
    /// Linux SCHED_FIFO priority for the RX thread.
    pub rx_priority: i32,
    /// Linux SCHED_FIFO priority used while radio drivers create workers.
    pub driver_priority: i32,
    /// Optional Linux CPU index for the TX thread.
    pub tx_cpu: Option<usize>,
    /// Optional Linux CPU index for the RX thread.
    pub rx_cpu: Option<usize>,
    /// Optional Linux CPU index inherited by radio-driver workers.
    pub driver_cpu: Option<usize>,
}

impl Default for RealtimeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            tx_priority: 80,
            rx_priority: 75,
            driver_priority: 70,
            tx_cpu: None,
            rx_cpu: None,
            driver_cpu: None,
        }
    }
}

impl RealtimeSettings {
    fn validate(&self) -> Result<(), Error> {
        if self.enabled {
            for (name, priority) in [
                ("tx_priority", self.tx_priority),
                ("rx_priority", self.rx_priority),
                ("driver_priority", self.driver_priority),
            ] {
                if !(1..=99).contains(&priority) {
                    return Err(format!("runtime.realtime.{name} must be in 1..=99").into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BtsRuntimeSettings {
    pub spreading_rate: SpreadingRate,
    pub orthogonal_code_length: usize,
    pub chip_rate_hz: usize,
    #[serde(skip)]
    pub tx_sample_rate_hz: usize,
    #[serde(skip)]
    pub tx_bandwidth_hz: usize,
    /// TX-frequency override; `None` → derive from `BtsNodeConfig.channel`.
    #[serde(default)]
    pub tx_freq_hz_override: Option<usize>,
    /// Hardware TX LO offset in Hz. The SDR is tuned to
    /// `<resolved_tx_center_hz> + tx_lo_offset_hz`, and the baseband is
    /// digitally rotated by the negative offset so the on-air carrier
    /// remains centered at the resolved TX frequency.
    pub tx_lo_offset_hz: i64,
    pub tx_digital_backoff: f32,
    /// Logical synthesis step in chip-rate samples. The BTS evaluates paging,
    /// sync, and traffic timing at this granularity.
    pub block_size_chips: usize,
    /// Number of chip-rate samples to coalesce into one SDR write. Must be a
    /// positive multiple of `block_size_chips`.
    pub tx_batch_chips: usize,
    pub short_code_length_chips: usize,
    pub downlink: DownlinkSettings,
    pub uplink: UplinkSettings,
    pub overhead: OverheadSettings,
    /// Maximum TX lookahead in milliseconds.  The TX loop will sleep at
    /// paging-frame boundaries when it is more than this far ahead of the
    /// hardware playout clock.  This ensures FPch content decisions are
    /// made close to real-time so that BSC-enqueued PDUs are picked up
    /// promptly.  Set to 0 to disable the cap (old behaviour).
    pub max_tx_lookahead_ms: u32,
    pub realtime: RealtimeSettings,
}

impl Default for BtsRuntimeSettings {
    fn default() -> Self {
        Self {
            spreading_rate: SpreadingRate::Sr1,
            orthogonal_code_length: SR1_WALSH_LENGTH,
            chip_rate_hz: SR1_CHIP_RATE_HZ as usize,
            tx_sample_rate_hz: SR1_CHIP_RATE_HZ as usize * 4,
            tx_bandwidth_hz: 1_500_000,
            tx_freq_hz_override: None,
            tx_lo_offset_hz: 0,
            tx_digital_backoff: 0.15,
            block_size_chips: 64,
            tx_batch_chips: 3_072,
            short_code_length_chips: SR1_SHORT_CODE_LENGTH_CHIPS,
            downlink: DownlinkSettings::default(),
            uplink: UplinkSettings::default(),
            overhead: OverheadSettings::default(),
            max_tx_lookahead_ms: 5,
            realtime: RealtimeSettings::default(),
        }
    }
}

impl BtsRuntimeSettings {
    pub fn validate(&self) -> Result<(), Error> {
        self.realtime.validate()?;
        if self.spreading_rate != SpreadingRate::Sr1 {
            return Err("only spreading_rate=sr1 is currently implemented".into());
        }
        if self.orthogonal_code_length != SR1_WALSH_LENGTH {
            return Err("only orthogonal_code_length=64 is currently implemented".into());
        }
        if self.chip_rate_hz != SR1_CHIP_RATE_HZ as usize {
            return Err("only chip_rate_hz=1228800 is currently implemented".into());
        }
        if self.short_code_length_chips != SR1_SHORT_CODE_LENGTH_CHIPS {
            return Err("only short_code_length_chips=32768 is currently implemented".into());
        }
        if self.tx_lo_offset_hz.unsigned_abs() as usize >= self.tx_sample_rate_hz / 2 {
            return Err(
                "tx_lo_offset_hz magnitude must be less than half the TX sample rate".into(),
            );
        }
        if self.block_size_chips == 0 || self.chip_rate_hz % self.block_size_chips != 0 {
            return Err("block_size_chips must be > 0 and divide chip_rate_hz".into());
        }
        if self.tx_batch_chips == 0 || self.tx_batch_chips % self.block_size_chips != 0 {
            return Err(
                "tx_batch_chips must be > 0 and an integer multiple of block_size_chips".into(),
            );
        }
        if self.chip_rate_hz % 50 != 0 {
            return Err(
                "chip_rate_hz must support exact 20ms boundaries (chip_rate_hz % 50 == 0)".into(),
            );
        }
        if (self.chip_rate_hz / 50) % self.block_size_chips != 0 {
            return Err("20ms paging frame interval must be divisible by block_size_chips".into());
        }
        if self.overhead.fragment_availability_interval_chips == 0
            || self.overhead.fragment_availability_interval_chips % self.block_size_chips != 0
        {
            return Err(
                "overhead.fragment_availability_interval_chips must be > 0 and divisible by block_size_chips".into(),
            );
        }
        if self.overhead.auth_mode > 3 {
            return Err("overhead.auth_mode must be in 0..=3".into());
        }
        if self.overhead.sync_superframe_interval_chips
            != self.overhead.fragment_availability_interval_chips * 3
        {
            return Err(
                "overhead.sync_superframe_interval_chips must equal 3 * overhead.fragment_availability_interval_chips".into(),
            );
        }
        if self.downlink.sync.symbol_repeat == 0 {
            return Err("downlink.sync.symbol_repeat must be > 0".into());
        }
        if self.downlink.sync.walsh_repetition == 0 {
            return Err("downlink.sync.walsh_repetition must be > 0".into());
        }
        if self.downlink.sync.availability_max_size_bits == 0 {
            return Err("downlink.sync.availability_max_size_bits must be > 0".into());
        }
        if self.downlink.paging.availability_max_size_bits == 0 {
            return Err("downlink.paging.availability_max_size_bits must be > 0".into());
        }
        if self.downlink.paging.paging_channel_number == 0
            || self.downlink.paging.paging_channel_number > 7
        {
            return Err("downlink.paging.paging_channel_number must be in 1..=7".into());
        }
        if self.downlink.paging.data_rate_bps != 9600 {
            return Err("only downlink.paging.data_rate_bps=9600 is currently implemented".into());
        }
        if self.downlink.paging.availability_max_size_bits != 96 {
            return Err(
                "for downlink.paging.data_rate_bps=9600, availability_max_size_bits must be 96 (one half-frame with SCI)"
                    .into(),
            );
        }
        if self.downlink.pilot.walsh_code >= self.orthogonal_code_length {
            return Err("downlink.pilot.walsh_code is out of range".into());
        }
        if self.downlink.sync.walsh_code >= self.orthogonal_code_length {
            return Err("downlink.sync.walsh_code is out of range".into());
        }
        if self.downlink.paging.walsh_code >= self.orthogonal_code_length {
            return Err("downlink.paging.walsh_code is out of range".into());
        }
        if self.tx_digital_backoff <= 0.0 || self.tx_digital_backoff > 1.0 {
            return Err("tx_digital_backoff must be in (0, 1]".into());
        }
        if self.downlink.pilot.gain < 0.0
            || self.downlink.sync.gain < 0.0
            || self.downlink.paging.gain < 0.0
        {
            return Err("channel gains must be non-negative".into());
        }
        if (self.downlink.pilot.gain + self.downlink.sync.gain + self.downlink.paging.gain) <= 0.0 {
            return Err("sum of channel gains must be > 0".into());
        }
        self.downlink
            .paging
            .message_defaults
            .extended_system_parameters
            .validate()?;
        Ok(())
    }
}

pub use cdma_common::events::AccessChannelEvent;
use cdma_common::hrpd::air::{HrpdAccessIndication, HrpdTrafficEvent};

pub use cdma_common::metrics::{RxMeasurement, RxMeasurementKey};

/// Shared store of per-mobile access channel signal quality.
pub type RxMeasurementStore = Arc<StdMutex<HashMap<RxMeasurementKey, RxMeasurement>>>;

#[derive(Clone)]
pub struct RxSettings {
    pub sample_rate_hz: usize,
    pub rx_center_frequency_hz: Option<usize>,
    /// Internal mode-derived gate for all 1x reverse-link processing.
    pub one_x_enabled: bool,
    pub one_x_reverse_frequency_hz: Option<usize>,
    pub one_x_rx_shift_hz: i64,
    pub hrpd_reverse_frequency_hz: Option<usize>,
    pub hrpd_rx_shift_hz: Option<i64>,
    /// Serving AUTH_MODE for exact decode of reverse access-channel tails.
    pub auth_mode: u8,
    /// Serving P_REV_IN_USE for exact decode of reverse access-channel tails.
    pub p_rev_in_use: u8,
    pub capture_iq_wav: Option<PathBuf>,
    pub capture_seconds: Option<f64>,
    pub access_channel_number: u8,
    pub paging_channel_number: u8,
    pub base_id: u16,
    pub pilot_pn: u16,
    pub chip_rate_hz: usize,
    pub absolute_chip_start: u64,
    pub hardware_start_time_ns: u64,
    pub tick_rate: u64,
    /// Optional channel for surfacing decoded access events to the BSC.
    pub access_event_tx: Option<mpsc::UnboundedSender<AccessChannelEvent>>,
    /// Optional channel for sending decoded HRPD access events to the AN.
    pub hrpd_access_event_tx: Option<mpsc::UnboundedSender<HrpdAccessIndication>>,
    /// Optional channel for sending decoded HRPD traffic events to the AN.
    pub hrpd_traffic_event_tx: Option<mpsc::UnboundedSender<HrpdTrafficEvent>>,
    /// HRPD Access Channel cycle used to derive the reverse access long-code mask.
    pub hrpd_access_cycle_number: u8,
    /// Least-significant 24 bits of the HRPD SectorID used in the reverse access long-code mask.
    pub hrpd_access_sector_id_lsb: u32,
    /// HRPD ColorCode used in QuickConfig and the reverse access long-code mask.
    pub hrpd_access_color_code: u8,
    /// AccessParameters `PreambleLength` (in frames) the reverse-access RX
    /// finger despreads the capsule at. Sourced from the broadcast
    /// AccessParameters, defaulting to the spec value.
    pub hrpd_access_preamble_frames: usize,
    /// Enables 19.2/38.4 kbps HRPD access capsule decode hypotheses. Mirrors
    /// the broadcast AccessParameters: set when the sector advertises an
    /// enhanced `SectorAccessMaxRate` above 9.6 kbps. Default false (Rev 0).
    pub hrpd_access_enhanced_rates: bool,
    /// Optional datagram sender for reverse traffic frames carried on the Abis UDP bearer.
    pub reverse_bearer_tx: Option<std_mpsc::Sender<UdpBearerDatagram>>,
    /// Optional channel for publishing RX pipeline metrics to the BtsHandle.
    pub rx_metrics_tx: Option<Arc<watch::Sender<RxMetrics>>>,
    /// Re-anchor the correlator's absolute sample origin on every block
    /// using the hardware timestamp, correcting for SDR overflow drift.
    pub reanchor_origin: bool,
    /// Shared pool of active reverse traffic channel receivers.
    /// The BSC populates this dynamically; the RX loop feeds IQ to each receiver.
    pub traffic_rx_pool: Option<TrafficRxPool>,
    /// Lock-free HRPD reverse traffic worker lifecycle command queue, drained
    /// by the RX loop to spawn and stop per-UATI workers.
    pub hrpd_traffic_rx_queue: Option<HrpdTrafficRxQueue>,
    /// Shared HRPD H-ARQ event bus between the forward scheduler (on the
    /// BTS synth thread) and the per-MAC reverse traffic RX workers.
    /// Optional; when `None`, the RX workers fall back to gated-mask ACK
    /// decoding and the scheduler runs the no-feedback `unknown_retx`
    /// fallback exclusively.
    pub hrpd_harq_bus: Option<std::sync::Arc<crate::bts::hrpd::HarqBus>>,
    /// Assignment-scoped HRPD reverse-packet outer-loop power control.
    pub hrpd_power_control: Option<crate::bts::hrpd::HrpdPowerControlRegistry>,
    /// Shared pool of active forward traffic channels, used by BTS-local
    /// reverse power control to schedule PCBs on the TX timeline.
    pub traffic_channels: Option<TrafficChannelPool>,
    /// BTS-local reverse power-control registry.
    pub power_control: Option<BtsPowerControlRegistry>,
    /// Shared list of Walsh codes whose traffic RX receivers should be removed.
    pub traffic_rx_removals: Option<TrafficRxRemovals>,
    /// When true, reverse-traffic receiver threads locally conceal sample
    /// discontinuities after they have seen real traffic activity. This does
    /// not affect the shared access-channel RX path.
    pub traffic_rx_continuity: bool,
    /// Thread pool size for reverse-access finger feeding.
    pub reverse_access_finger_pool_size: usize,
    /// Default thread pool size for other channels (traffic, etc.).
    pub global_finger_pool_size: usize,
    /// Overhead MCC for IMSI class-0 forward address resolution.
    pub overhead_mcc: u16,
    /// Overhead IMSI_11_12 for IMSI class-0 forward address resolution.
    pub overhead_imsi_11_12: u8,
    /// Inherent RX pipeline delay in samples (at `sample_rate_hz`). Subtracted
    /// from the hardware-time → absolute-sample mapping so each received sample
    /// is labeled with the absolute sample number at which it was transmitted.
    /// Calibrated per-SDR via the `calibrate_rx_delay` bin.
    pub rx_sample_delay: i64,
    /// Number of PCGs (1536 chips) per RX read batch. Controls the
    /// granularity of sample delivery to the pipeline. Default: 2.
    pub rx_batch_pcgs: usize,
    /// Shared TX→RX timing anchor. When `Some`, the RX thread waits for TX to
    /// publish its (tick, chip) anchor before processing samples — until then it
    /// just drains to prevent overflow. `None` for the injected-RX test path.
    pub tx_rx_anchor: Option<Arc<TxRxAnchor>>,
    /// Optional channel for forwarding reverse-traffic ACK_SEQ to the BTS
    /// traffic LAC. Carries (walsh_code, ack_seq) pairs.
    pub traffic_ack_seq_tx: Option<mpsc::Sender<(u8, u8)>>,
    /// Shared store for access-channel signal quality measurements, read by BSC.
    pub rx_measurements: Option<RxMeasurementStore>,
}

pub use cdma_common::overhead::OverheadParameters;

fn build_evdo_atim(
    pilot_offset: usize,
    overhead: &OverheadParameters,
    evdo: Evdo1xAdvertisement,
) -> PagingChannelMessage {
    PagingChannelMessage::AlternativeTechnologiesInformation(
        AlternativeTechnologiesInformationMessage {
            pilot_pn: pilot_offset as u16,
            config_msg_seq: overhead.config_seq,
            radio_interfaces: vec![AlternativeTechnologyRadioInterfaceRecord::hrpd(
                &AlternativeHrpdRadioInterface {
                    subnet_color_code: Some(evdo.hrpd_color_code),
                    neighbors: vec![AlternativeHrpdNeighborRecord {
                        nghbr_pn: evdo.hrpd_pn,
                        freq_same_as_prev: false,
                        nghbr_band: Some(evdo.hrpd_band_class),
                        nghbr_freq: Some(evdo.hrpd_channel),
                        pn_association_ind: true,
                        data_association_ind: true,
                        subnet_color_code: AlternativeHrpdNeighborSubnetColorCode::SameAsCommon,
                    }],
                },
            )],
        },
    )
}

/// Build an overhead or GPM paging channel message from parameters.
pub fn build_scheduled_message(
    kind: PagingMessageKind,
    pilot_offset: usize,
    overhead: &OverheadParameters,
    paging: &PagingChannelSettings,
    evdo_advertisement: Option<Evdo1xAdvertisement>,
) -> PagingChannelMessage {
    match kind {
        PagingMessageKind::SystemParameters => {
            let defaults = &paging.message_defaults.system_parameters;
            let advertises_atim = evdo_advertisement.is_some();
            PagingChannelMessage::SystemParameters(SystemParametersMessage {
                pilot_pn: pilot_offset as u16,
                config_msg_seq: overhead.config_seq,
                sid: overhead.sid,
                nid: overhead.nid,
                reg_zone: overhead.reg_zone,
                total_zones: overhead.total_zones,
                zone_timer: overhead.zone_timer,
                mult_sids: defaults.mult_sids,
                mult_nids: defaults.mult_nids,
                base_id: overhead.base_id,
                base_class: defaults.base_class,
                page_chan: overhead.page_chan,
                max_slot_cycle_index: overhead.max_slot_cycle_index,
                home_reg: defaults.home_reg,
                for_sid_reg: defaults.for_sid_reg,
                for_nid_reg: defaults.for_nid_reg,
                power_up_reg: overhead.power_up_reg,
                power_down_reg: defaults.power_down_reg,
                parameter_reg: overhead.parameter_reg,
                reg_prd: defaults.reg_prd,
                base_lat: defaults.base_lat,
                base_long: defaults.base_long,
                reg_dist: defaults.reg_dist,
                srch_win_a: defaults.srch_win_a,
                srch_win_n: defaults.srch_win_n,
                srch_win_r: defaults.srch_win_r,
                nghbr_max_age: defaults.nghbr_max_age,
                pwr_rep_thresh: defaults.pwr_rep_thresh,
                pwr_rep_frames: defaults.pwr_rep_frames,
                pwr_thresh_enable: defaults.pwr_thresh_enable,
                pwr_period_enable: defaults.pwr_period_enable,
                pwr_rep_delay: defaults.pwr_rep_delay,
                rescan: defaults.rescan,
                t_add: defaults.t_add,
                t_drop: defaults.t_drop,
                t_comp: defaults.t_comp,
                t_tdrop: defaults.t_tdrop,
                ext_sys_parameter: defaults.ext_sys_parameter,
                ext_nghbr_lst: defaults.ext_nghbr_lst,
                gen_nghbr_lst: defaults.gen_nghbr_lst,
                global_redirect: false,
                pri_nghbr_lst: defaults.pri_nghbr_lst,
                user_zone_id: defaults.user_zone_id,
                ext_global_redirect: false,
                ext_chan_lst: false,
                // SPM tail per C.S0005-E §3.7.2.3.2.1, mandatory at P_REV >= 6.
                t_tdrop_range_incl: false,
                t_tdrop_range: 0,
                neg_slot_cycle_index_sup: false,
                crrm_msg_ind: false,
                num_opt_msg_bits: if advertises_atim { 6 } else { 0 },
                ap_pilot_info: false,
                ap_idt: false,
                ap_id_text: false,
                gen_ovhd_inf_ind: false,
                fd_chan_lst_ind: false,
                atim_ind: advertises_atim,
                appim_period_index: 0,
                gen_ovhd_cycle_index: 0,
                atim_cycle_index: 0,
                add_loc_info_incl: false,
            })
        }
        PagingMessageKind::AccessParameters => {
            let defaults = &paging.message_defaults.access_parameters;
            PagingChannelMessage::AccessParameters(AccessParametersMessage {
                pilot_pn: pilot_offset as u16,
                acc_msg_seq: overhead.acc_config_seq,
                acc_chan: defaults.acc_chan,
                nom_pwr: defaults.nom_pwr,
                init_pwr: defaults.init_pwr,
                pwr_step: defaults.pwr_step,
                num_step: defaults.num_step,
                max_cap_sz: defaults.max_cap_sz,
                pam_sz: defaults.pam_sz,
                psist_0_9: defaults.psist_0_9,
                psist_10: defaults.psist_10,
                psist_11: defaults.psist_11,
                psist_12: defaults.psist_12,
                psist_13: defaults.psist_13,
                psist_14: defaults.psist_14,
                psist_15: defaults.psist_15,
                msg_psist: defaults.msg_psist,
                reg_psist: defaults.reg_psist,
                probe_pn_ran: defaults.probe_pn_ran,
                acc_tmo: defaults.acc_tmo,
                probe_bkoff: defaults.probe_bkoff,
                bkoff: defaults.bkoff,
                max_req_seq: defaults.max_req_seq,
                max_rsp_seq: defaults.max_rsp_seq,
                auth: defaults.auth,
                rand: defaults.rand,
                nom_pwr_ext: defaults.nom_pwr_ext,
                psist_emg_incl: defaults.psist_emg_incl,
                psist_emg: defaults.psist_emg,
                acct_incl: defaults.acct_incl,
                acct_incl_emg: defaults.acct_incl_emg,
                acct_aoc_bitmap_incl: defaults.acct_aoc_bitmap_incl,
                acct_so_records: defaults.acct_so_records.clone(),
                acct_so_grp_records: defaults.acct_so_grp_records.clone(),
            })
        }
        PagingMessageKind::NeighborList => {
            let defaults = &paging.message_defaults.neighbor_list;
            PagingChannelMessage::NeighborList(NeighborListMessage {
                pilot_pn: pilot_offset as u16,
                config_msg_seq: overhead.config_seq,
                pilot_inc: defaults.pilot_inc,
                neighbors: defaults.neighbors.clone(),
            })
        }
        PagingMessageKind::CdmaChannelList => {
            let defaults = &paging.message_defaults.cdma_channel_list;
            // Default to the operating channel when no explicit list.
            let channels = if defaults.channels.is_empty() {
                vec![
                    overhead
                        .cdma_freq
                        .expect("cdma_freq resolved by BTS launcher"),
                ]
            } else {
                defaults.channels.clone()
            };
            PagingChannelMessage::CdmaChannelList(CdmaChannelListMessage {
                pilot_pn: pilot_offset as u16,
                config_msg_seq: overhead.config_seq,
                channels,
            })
        }
        PagingMessageKind::ExtendedSystemParameters => {
            let defaults = &paging.message_defaults.extended_system_parameters;
            PagingChannelMessage::ExtendedSystemParameters(ExtendedSystemParametersMessage {
                pilot_pn: pilot_offset as u16,
                config_msg_seq: overhead.config_seq,
                delete_for_tmsi: defaults.delete_for_tmsi,
                use_tmsi: defaults.use_tmsi,
                pref_msid_type: defaults.pref_msid_type,
                mcc: defaults.mcc,
                imsi_11_12: defaults.imsi_11_12,
                tmsi_zone: defaults.tmsi_zone.clone(),
                bcast_index: defaults.bcast_index,
                imsi_t_supported: defaults.imsi_t_supported,
                p_rev: overhead.p_rev,
                min_p_rev: overhead.min_p_rev,
                soft_slope: defaults.soft_slope,
                add_intercept: defaults.add_intercept,
                drop_intercept: defaults.drop_intercept,
                packet_zone_id: defaults.packet_zone_id,
                max_num_alt_so: defaults.max_num_alt_so,
                reselect_included: defaults.reselect_included,
                ec_thresh: defaults.ec_thresh,
                ec_io_thresh: defaults.ec_io_thresh,
                pilot_report: defaults.pilot_report,
                nghbr_set_entry_info: defaults.nghbr_set_entry_info,
                acc_ent_ho_order: defaults.acc_ent_ho_order,
                nghbr_set_access_info: defaults.nghbr_set_access_info,
                access_ho: defaults.access_ho,
                access_ho_msg_rsp: defaults.access_ho_msg_rsp,
                access_probe_ho: defaults.access_probe_ho,
                acc_ho_list_upd: defaults.acc_ho_list_upd,
                acc_probe_ho_other_msg: defaults.acc_probe_ho_other_msg,
                max_num_probe_ho: defaults.max_num_probe_ho,
                nghbr_set_size: defaults.nghbr_set_size,
                access_entry_ho: defaults.access_entry_ho.clone(),
                access_ho_allowed: defaults.access_ho_allowed.clone(),
                broadcast_gps_asst: defaults.broadcast_gps_asst,
                qpch_supported: defaults.qpch_supported,
                num_qpch: defaults.num_qpch,
                qpch_rate: defaults.qpch_rate,
                qpch_power_level_page: defaults.qpch_power_level_page,
                qpch_cci_supported: defaults.qpch_cci_supported,
                qpch_power_level_config: defaults.qpch_power_level_config,
                sdb_supported: defaults.sdb_supported,
                rlgain_traffic_pilot: defaults.rlgain_traffic_pilot,
                rev_pwr_cntl_delay_incl: defaults.rev_pwr_cntl_delay_incl,
                rev_pwr_cntl_delay: defaults.rev_pwr_cntl_delay,
                auto_msg_supported: defaults.auto_msg_supported,
                auto_msg_interval: defaults.auto_msg_interval,
                mob_qos: defaults.mob_qos,
                enc_supported: defaults.enc_supported,
                sig_encrypt_sup: defaults.sig_encrypt_sup,
                ui_encrypt_sup: defaults.ui_encrypt_sup,
                use_sync_id: defaults.use_sync_id,
                cs_supported: defaults.cs_supported,
                bcch_supported: defaults.bcch_supported,
                ms_init_pos_loc_sup_ind: defaults.ms_init_pos_loc_sup_ind,
                pilot_info_req_supported: defaults.pilot_info_req_supported,
                ext_pref_msid_type: defaults.ext_pref_msid_type,
                meid_reqd: defaults.meid_reqd,
            })
        }
        PagingMessageKind::GeneralPage => {
            let defaults = &paging.message_defaults.general_page;
            PagingChannelMessage::GeneralPage(GeneralPageMessage {
                config_msg_seq: overhead.config_seq,
                acc_msg_seq: overhead.acc_config_seq,
                class_0_done: defaults.class_0_done,
                class_1_done: defaults.class_1_done,
                tmsi_done: defaults.tmsi_done,
                ordered_tmsis: defaults.ordered_tmsis,
                broadcast_done: defaults.broadcast_done,
                reserved: defaults.reserved,
                add_pfield: defaults.add_pfield.clone(),
                page_records: defaults.page_records.clone(),
            })
        }
        PagingMessageKind::Order => {
            let defaults = &paging.message_defaults.order;
            PagingChannelMessage::Order(OrderMessage {
                order: defaults.order,
                ordq: defaults.ordq,
                order_specific_fields: Vec::new(),
            })
        }
        PagingMessageKind::AlternativeTechnologiesInformation => build_evdo_atim(
            pilot_offset,
            overhead,
            evdo_advertisement
                .expect("ATIM schedule entry requires a resolved EV-DO advertisement"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evdo_advertisement() -> Evdo1xAdvertisement {
        Evdo1xAdvertisement {
            hrpd_pn: 0,
            hrpd_band_class: 0,
            hrpd_channel: 425,
            hrpd_color_code: 26,
        }
    }

    #[test]
    fn realtime_priorities_must_be_valid_when_enabled() {
        let mut runtime = BtsRuntimeSettings::default();
        runtime.realtime.tx_priority = 0;
        assert!(runtime.validate().is_err());

        runtime.realtime.enabled = false;
        assert!(runtime.validate().is_ok());
    }

    #[test]
    fn system_parameters_sets_atim_indicator_when_evdo_is_advertised() {
        let mut overhead = OverheadParameters::default();
        overhead.config_seq = 5;
        let message = build_scheduled_message(
            PagingMessageKind::SystemParameters,
            0,
            &overhead,
            &PagingChannelSettings::default(),
            Some(evdo_advertisement()),
        );

        let PagingChannelMessage::SystemParameters(spm) = message else {
            panic!("expected SPM");
        };
        assert_eq!(spm.config_msg_seq, 5);
        assert_eq!(spm.num_opt_msg_bits, 6);
        assert!(!spm.ap_pilot_info);
        assert!(!spm.ap_idt);
        assert!(!spm.ap_id_text);
        assert!(!spm.gen_ovhd_inf_ind);
        assert!(!spm.fd_chan_lst_ind);
        assert!(spm.atim_ind);
        assert_eq!(spm.atim_cycle_index, 0);
    }

    #[test]
    fn evdo_atim_advertises_hrpd_frequency_pn_and_color_code() {
        let mut overhead = OverheadParameters::default();
        overhead.config_seq = 5;
        let message = build_scheduled_message(
            PagingMessageKind::AlternativeTechnologiesInformation,
            0,
            &overhead,
            &PagingChannelSettings::default(),
            Some(evdo_advertisement()),
        );

        let PagingChannelMessage::AlternativeTechnologiesInformation(atim) = message else {
            panic!("expected ATIM");
        };
        assert_eq!(atim.pilot_pn, 0);
        assert_eq!(atim.config_msg_seq, 5);
        assert_eq!(atim.radio_interfaces.len(), 1);
        let sdu = atim.to_sdu();
        assert_eq!(sdu.len(), 97);
        assert_eq!(
            sdu.to_packed_bytes(),
            vec![
                0x00, 0x0a, 0x24, 0x04, 0x0c, 0x68, 0x02, 0x30, 0x00, 0x06, 0xa7, 0x40, 0x00,
            ]
        );
        match &atim.radio_interfaces[0] {
            AlternativeTechnologyRadioInterfaceRecord::Hrpd { fields } => {
                assert_eq!(
                    fields,
                    &vec![0x18, 0xD0, 0x04, 0x60, 0x00, 0x0D, 0x4E, 0x80]
                );
            }
            _ => panic!("expected HRPD radio-interface record"),
        }

        let hrpd = atim.radio_interfaces[0]
            .hrpd_fields()
            .expect("ATIM HRPD radio-interface should decode")
            .expect("ATIM should contain HRPD fields");
        assert_eq!(hrpd.subnet_color_code, Some(26));
        assert_eq!(hrpd.neighbors.len(), 1);
        let neighbor = &hrpd.neighbors[0];
        assert_eq!(neighbor.nghbr_pn, 0);
        assert!(!neighbor.freq_same_as_prev);
        assert_eq!(neighbor.nghbr_band, Some(0));
        assert_eq!(neighbor.nghbr_freq, Some(425));
        assert!(neighbor.pn_association_ind);
        assert!(neighbor.data_association_ind);
        assert_eq!(
            neighbor.subnet_color_code,
            AlternativeHrpdNeighborSubnetColorCode::SameAsCommon
        );
    }
}
