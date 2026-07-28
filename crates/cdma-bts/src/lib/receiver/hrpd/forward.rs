//! HRPD forward-link acquisition and Control Channel decode.
//!
//! Accepts complex samples at an integer multiple of the 1.2288 Mcps chip
//! rate, acquires the TDM pilot bursts, despreads the forward short code, and
//! decodes FCS-valid Forward Control Channel overhead capsules in both the
//! Rev 0 (subtype 0) and Rev A (subtype 2) physical-layer control formats.

use cdma_common::hrpd::messages::{
    DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE, HrpdOverheadMessage, SyncMessage,
};
use num::complex::Complex32;
use rustfft::FftPlanner;

use crate::{
    phy::{
        hrpd::{
            crc::physical_crc16,
            slot::{DATA_EDGE_CHIPS, HALF_SLOT_CHIPS, SLOT_CHIPS},
            turbo_decoder::HrpdTurboDecoder,
        },
        spread::HrpdForwardPnSequence,
    },
    receiver::pipelined::{
        PipelineProcessor, PipelineProcessorShared, SampleBlock, VecEmitter,
        generic_rake_receiver::{
            BaseFinger, Correlator, DefaultPrunePolicy, GenericRakeReceiver, RakeFinger,
        },
        run_sub_chain,
    },
    sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32},
};

pub const HRPD_CHIP_RATE_HZ: u32 = 1_228_800;

const PILOT_START_IN_HALF_SLOT: usize = 464;
const PILOT_END_IN_HALF_SLOT: usize = 560;
const FORWARD_CORRELATOR_MIN_CHIPS: usize = 32_768 * 4;
const FORWARD_CORRELATOR_RETAIN_CHIPS: usize = 32_768 * 3;
const FORWARD_FINGER_RETAIN_MARGIN_CHIPS: usize = 4;
const FORWARD_OVERHEAD_RETAIN_CHIPS: usize = CONTROL_CHANNEL_CYCLE_SLOTS * SLOT_CHIPS as usize * 2;
const CONTROL_CHANNEL_CYCLE_SLOTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipStreamVariant {
    Decimated,
    Boxcar,
    MatchedFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PnDespreadMode {
    MultiplyPn,
    MultiplyConjPn,
}

#[derive(Debug, Clone)]
pub struct PilotAcquisition {
    pub samples_per_chip: usize,
    pub timing_phase: usize,
    pub chip_variant: ChipStreamVariant,
    pub pn_phase_chips: usize,
    pub pn_offset: u16,
    pub slot_phase_chips: usize,
    pub despread_mode: PnDespreadMode,
    pub pilot_metric: f32,
    pub pilot_snr_db: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSyncMessage {
    pub maximum_revision: u8,
    pub minimum_revision: u8,
    pub pilot_pn: u16,
    pub system_time: u64,
}

#[derive(Debug, Clone)]
pub struct ForwardSyncDecode {
    pub acquisition: PilotAcquisition,
    pub slot_start_chip: usize,
    pub payload_bits: u32,
    pub messages: Vec<Vec<u8>>,
    pub overhead_messages: Vec<HrpdOverheadMessage>,
    pub sync: DecodedSyncMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardSignalingMessage {
    pub ati_type: u8,
    pub ati: Option<u32>,
    pub protocol_type: u8,
    pub message_id: Option<u8>,
    pub message_id_bits: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ForwardControlCapsuleDecode {
    pub acquisition: PilotAcquisition,
    pub slot_start_chip: usize,
    pub payload_bits: u32,
    pub signaling_messages: Vec<ForwardSignalingMessage>,
    pub overhead_messages: Vec<HrpdOverheadMessage>,
    pub sync: Option<DecodedSyncMessage>,
}

#[derive(Debug, Clone)]
pub struct HrpdForwardReceiver {
    sample_rate_hz: u32,
}

impl HrpdForwardReceiver {
    pub fn new(sample_rate_hz: u32) -> Self {
        Self { sample_rate_hz }
    }

    pub fn acquire_pilot(&self, samples: &[Complex32]) -> Option<PilotAcquisition> {
        let samples_per_chip = samples_per_chip(self.sample_rate_hz)?;
        acquire_pilot_candidates(samples, samples_per_chip)
            .into_iter()
            .next()
    }

    pub fn decode_sync_with_acquisition(
        &self,
        samples: &[Complex32],
        acq: &PilotAcquisition,
    ) -> Option<ForwardSyncDecode> {
        let chips = make_chip_stream(
            samples,
            acq.samples_per_chip,
            acq.timing_phase,
            acq.chip_variant,
        );
        let mut despread = despread_chips(&chips, acq.pn_phase_chips, acq.despread_mode);
        let cfo_rad_per_chip = estimate_pilot_cfo_rad_per_chip(&despread, acq.slot_phase_chips);
        correct_chip_cfo(&mut despread, cfo_rad_per_chip);
        decode_sync_from_despread(&despread, acq)
    }
}

fn acquire_pilot_candidates(
    samples: &[Complex32],
    samples_per_chip: usize,
) -> Vec<PilotAcquisition> {
    // Single pipeline: matched-PN FFT correlation. If it acquires, we are done.
    if let Some(acq) = acquire_pilot_fft(samples, samples_per_chip) {
        return vec![acq];
    }
    Vec::new()
}

pub type HrpdForwardRakeReceiver = GenericRakeReceiver<ForwardPilotCorrelator>;

pub fn hrpd_forward_rake_receiver(sample_rate_hz: u32) -> HrpdForwardRakeReceiver {
    GenericRakeReceiver::new(ForwardPilotCorrelator::new(sample_rate_hz))
        .with_max_fingers(1)
        .with_finger_pool_size(1)
        .with_prune_policy(Box::new(hrpd_forward_prune_policy()))
}

pub fn hrpd_forward_overhead_chain() -> Vec<PipelineProcessorShared> {
    vec![Box::new(HrpdForwardOverheadProcessor::new()) as PipelineProcessorShared]
}

pub fn hrpd_forward_prune_policy() -> DefaultPrunePolicy {
    DefaultPrunePolicy {
        // A forward Control cycle is 256 slots (~427 ms). Keep an acquired
        // pilot alive long enough to see several cycles before declaring the
        // downstream decode path stale.
        max_idle_chips: 8 * CONTROL_CHANNEL_CYCLE_SLOTS as u64 * SLOT_CHIPS as u64,
        max_validated_idle_chips: 16 * CONTROL_CHANNEL_CYCLE_SLOTS as u64 * SLOT_CHIPS as u64,
        max_crc_miss_count: 512,
        max_post_walsh_no_event_chips: u64::MAX,
        max_validated_post_walsh_no_event_chips: u64::MAX,
        max_post_walsh_no_event_ms: u64::MAX,
        max_validated_post_walsh_no_event_ms: u64::MAX,
        ..DefaultPrunePolicy::default()
    }
}

pub struct ForwardPilotCorrelator {
    sample_rate_hz: u32,
    samples_per_chip: usize,
    buffer: Vec<Complex32>,
    buffer_start_chip: Option<usize>,
    next_finger_id: u64,
    spawned: bool,
    chain_factory: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,
}

impl ForwardPilotCorrelator {
    pub fn new(sample_rate_hz: u32) -> Self {
        let samples_per_chip = samples_per_chip(sample_rate_hz).unwrap_or(1);
        Self {
            sample_rate_hz,
            samples_per_chip,
            buffer: Vec::new(),
            buffer_start_chip: None,
            next_finger_id: 1,
            spawned: false,
            chain_factory: Box::new(hrpd_forward_overhead_chain),
        }
    }

    fn retain_recent_search_samples(&mut self) {
        let retain_samples = FORWARD_CORRELATOR_RETAIN_CHIPS * self.samples_per_chip;
        if self.buffer.len() <= retain_samples {
            return;
        }
        let mut drop_samples = self.buffer.len() - retain_samples;
        drop_samples -= drop_samples % self.samples_per_chip.max(1);
        if drop_samples == 0 {
            return;
        }
        self.buffer.drain(..drop_samples);
        if let Some(start) = &mut self.buffer_start_chip {
            *start += drop_samples / self.samples_per_chip.max(1);
        }
    }
}

impl Correlator for ForwardPilotCorrelator {
    type Finger = ForwardPilotFinger;

    fn correlate(
        &mut self,
        block: &SampleBlock,
    ) -> Vec<(Self::Finger, Vec<PipelineProcessorShared>)> {
        if self.spawned {
            return Vec::new();
        }

        if self.sample_rate_hz == 0 && block.sample_rate_hz > 0.0 {
            self.sample_rate_hz = block.sample_rate_hz.round() as u32;
            self.samples_per_chip = samples_per_chip(self.sample_rate_hz).unwrap_or(1);
        }

        if self.buffer.is_empty() {
            self.buffer_start_chip = Some(block.chip_start);
        }
        let previous_buffer_len = self.buffer.len();
        self.buffer.extend_from_slice(&block.samples);
        let min_samples = FORWARD_CORRELATOR_MIN_CHIPS * self.samples_per_chip;
        if self.buffer.len() < min_samples {
            return Vec::new();
        }

        let search = self.buffer.clone();
        let Some(acquisition) = acquire_pilot_fft(&search, self.samples_per_chip) else {
            self.retain_recent_search_samples();
            return Vec::new();
        };
        let chain_start_chip = self.buffer_start_chip.unwrap_or(block.chip_start);
        let prebuffer = self.buffer[..previous_buffer_len].to_vec();
        let finger = ForwardPilotFinger::new(
            self.next_finger_id,
            self.sample_rate_hz,
            acquisition,
            chain_start_chip,
            prebuffer,
        );
        self.next_finger_id += 1;
        self.spawned = true;
        Vec::from([(finger, (self.chain_factory)())])
    }

    fn notify_finger_removed(&mut self, _finger_id: u64) {
        self.spawned = false;
    }

    fn search_suppressed(&self) -> bool {
        self.spawned
    }
}

pub struct ForwardPilotFinger {
    base: BaseFinger,
    acquisition: PilotAcquisition,
    buffer: Vec<Complex32>,
    buffer_start_chip: usize,
    emitted_chips: usize,
    sample_rate_hz: u32,
    chain_start_chip: usize,
}

impl ForwardPilotFinger {
    fn new(
        id: u64,
        sample_rate_hz: u32,
        acquisition: PilotAcquisition,
        chain_start_chip: usize,
        buffer: Vec<Complex32>,
    ) -> Self {
        Self {
            base: BaseFinger::new(id),
            acquisition,
            buffer,
            buffer_start_chip: chain_start_chip,
            emitted_chips: 0,
            sample_rate_hz,
            chain_start_chip,
        }
    }

    fn despread_available(&mut self) -> Vec<SampleBlock> {
        let dropped_chips = self.buffer_start_chip.saturating_sub(self.chain_start_chip);
        let pn_phase = (self.acquisition.pn_phase_chips + dropped_chips) % 32_768usize;
        let chips = make_chip_stream(
            &self.buffer,
            self.acquisition.samples_per_chip,
            self.acquisition.timing_phase,
            self.acquisition.chip_variant,
        );
        let local_emitted = self.emitted_chips.saturating_sub(dropped_chips);
        if local_emitted >= chips.len() {
            return Vec::new();
        };
        let despread = despread_chips(&chips, pn_phase, self.acquisition.despread_mode);
        let new_chips = despread[local_emitted..].to_vec();
        if new_chips.is_empty() {
            return Vec::new();
        }
        let chip_start = dropped_chips + local_emitted;
        self.emitted_chips = dropped_chips + despread.len();
        let mut block = SampleBlock::new(new_chips, self.chain_start_chip + chip_start)
            .with_sample_rate_hz(f64::from(HRPD_CHIP_RATE_HZ));
        block.tags.insert("finger_id", self.base.id as i64);
        block.tags.insert("hrpd_forward_despread", 1);
        block
            .tags
            .insert("hrpd_pilot_chain_start_chip", self.chain_start_chip as i64);
        block.tags.insert(
            "hrpd_pilot_pn_phase",
            self.acquisition.pn_phase_chips as i64,
        );
        block.tags.insert(
            "hrpd_pilot_pn_offset",
            i64::from(self.acquisition.pn_offset),
        );
        block.tags.insert(
            "hrpd_pilot_halfslot_phase",
            self.acquisition.slot_phase_chips as i64,
        );
        block.tags.insert(
            "hrpd_pilot_timing_phase",
            self.acquisition.timing_phase as i64,
        );
        block.tags.insert(
            "hrpd_pilot_samples_per_chip",
            self.acquisition.samples_per_chip as i64,
        );
        block.tags.insert(
            "hrpd_pilot_snr_mdb",
            (self.acquisition.pilot_snr_db * 1000.0).round() as i64,
        );
        self.prune_emitted_samples();
        Vec::from([block])
    }

    fn prune_emitted_samples(&mut self) {
        let keep_from_chip = self
            .chain_start_chip
            .saturating_add(self.emitted_chips)
            .saturating_sub(FORWARD_FINGER_RETAIN_MARGIN_CHIPS);
        let drop_chips = keep_from_chip.saturating_sub(self.buffer_start_chip);
        let drop_samples = drop_chips * self.acquisition.samples_per_chip;
        if drop_samples == 0 || drop_samples > self.buffer.len() {
            return;
        }
        self.buffer.drain(..drop_samples);
        self.buffer_start_chip += drop_chips;
    }
}

impl RakeFinger for ForwardPilotFinger {
    fn id(&self) -> u64 {
        self.base.id
    }

    fn spawn_chip_start(&self) -> Option<u64> {
        Some(self.chain_start_chip as u64)
    }

    fn process(
        &mut self,
        block: &SampleBlock,
        chain: &mut Vec<PipelineProcessorShared>,
    ) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_start_chip = block.chip_start;
        }
        self.buffer.extend_from_slice(&block.samples);
        let mut out = self.despread_available();
        if !chain.is_empty() {
            let mut chained = Vec::new();
            for input in out.iter().cloned() {
                let mut emitter = VecEmitter::new();
                chained.extend(run_sub_chain(chain, input, &mut emitter));
                chained.extend(emitter.blocks);
            }
            out.extend(chained);
        }
        self.base.tick_and_validate(
            &out,
            (block.samples.len() / self.acquisition.samples_per_chip) as u64,
        );
        out
    }

    fn flush(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        let mut out = self.despread_available();
        out.extend(BaseFinger::flush_chain(chain));
        out
    }

    fn is_hard_validated(&self) -> bool {
        self.base.is_hard_validated()
    }

    fn describe(&self) -> String {
        format!(
            "hrpd-forward pn={} phase={} halfslot={} timing={} fs={} snr={:.1}dB",
            self.acquisition.pn_offset,
            self.acquisition.pn_phase_chips,
            self.acquisition.slot_phase_chips,
            self.acquisition.timing_phase,
            self.sample_rate_hz,
            self.acquisition.pilot_snr_db
        )
    }

    fn idle_blocks(&self) -> u64 {
        self.base.idle_blocks()
    }

    fn idle_chips(&self) -> u64 {
        self.base.idle_chips()
    }

    fn crc_miss_count(&self) -> u64 {
        self.base.crc_miss_count()
    }

    fn post_walsh_no_event_chips(&self) -> u64 {
        self.base.post_walsh_no_event_chips()
    }

    fn post_walsh_miss_count(&self) -> u64 {
        self.base.post_walsh_miss_count()
    }

    fn post_walsh_no_event_ms(&self) -> u64 {
        self.base.post_walsh_no_event_ms()
    }
}

pub struct HrpdForwardOverheadProcessor {
    acquisition: Option<PilotAcquisition>,
    buffer: Vec<Complex32>,
    buffer_start_chip: Option<usize>,
    cycle_abs_phase: Option<usize>,
    emitted_slot_starts: Vec<usize>,
    decoded_capsules: usize,
    decoded_messages: usize,
}

impl HrpdForwardOverheadProcessor {
    pub fn new() -> Self {
        Self {
            acquisition: None,
            buffer: Vec::new(),
            buffer_start_chip: None,
            cycle_abs_phase: None,
            emitted_slot_starts: Vec::new(),
            decoded_capsules: 0,
            decoded_messages: 0,
        }
    }

    fn acquisition_from_tags(block: &SampleBlock) -> Option<PilotAcquisition> {
        if block.tags.get("hrpd_forward_despread").copied() != Some(1) {
            return None;
        }
        Some(PilotAcquisition {
            samples_per_chip: block.tags.get("hrpd_pilot_samples_per_chip").copied()? as usize,
            timing_phase: block.tags.get("hrpd_pilot_timing_phase").copied()? as usize,
            chip_variant: ChipStreamVariant::Decimated,
            pn_phase_chips: block.tags.get("hrpd_pilot_pn_phase").copied()? as usize,
            pn_offset: block.tags.get("hrpd_pilot_pn_offset").copied()? as u16,
            slot_phase_chips: block.tags.get("hrpd_pilot_halfslot_phase").copied()? as usize,
            despread_mode: PnDespreadMode::MultiplyPn,
            pilot_metric: 0.0,
            pilot_snr_db: block
                .tags
                .get("hrpd_pilot_snr_mdb")
                .copied()
                .map(|v| v as f32 / 1000.0)
                .unwrap_or(0.0),
        })
    }

    fn capsule_block(&self, decoded: &ForwardControlCapsuleDecode) -> SampleBlock {
        let chip_start = self.buffer_start_chip.unwrap_or(0) + decoded.slot_start_chip;
        let mut block = SampleBlock::new(Vec::new(), chip_start);
        block.tags.insert("finger_event", 1);
        block.tags.insert("finger_crc_valid", 1);
        block.tags.insert("hrpd_forward_overhead_decoded", 1);
        block.tags.insert("hrpd_control_capsule_decoded", 1);
        block
            .tags
            .insert("hrpd_control_payload_bits", i64::from(decoded.payload_bits));
        block.tags.insert(
            "hrpd_control_message_count",
            decoded.signaling_messages.len() as i64,
        );
        block.tags.insert(
            "hrpd_control_overhead_count",
            decoded.overhead_messages.len() as i64,
        );
        block
            .tags
            .insert("hrpd_control_slot_start_chip", chip_start as i64);
        block.tags.insert(
            "hrpd_control_pilot_pn_offset",
            i64::from(decoded.acquisition.pn_offset),
        );
        block.tags.insert(
            "hrpd_control_acquisition_pn_offset",
            i64::from(decoded.acquisition.pn_offset),
        );
        if let Some(message) = decoded.signaling_messages.first() {
            block.tags.insert(
                "hrpd_control_first_protocol",
                i64::from(message.protocol_type),
            );
            if let Some(message_id) = message.message_id {
                block
                    .tags
                    .insert("hrpd_control_first_message_id", i64::from(message_id));
            }
        }
        if let Some(sync) = decoded.sync.as_ref() {
            block.tags.insert("hrpd_sync_decoded", 1);
            block
                .tags
                .insert("hrpd_sync_pilot_pn", i64::from(sync.pilot_pn));
            block
                .tags
                .insert("hrpd_sync_max_revision", i64::from(sync.maximum_revision));
            block
                .tags
                .insert("hrpd_sync_min_revision", i64::from(sync.minimum_revision));
            block
                .tags
                .insert("hrpd_sync_system_time", sync.system_time as i64);
            block
                .tags
                .insert("hrpd_sync_slot_start_chip", chip_start as i64);
        }
        block
    }

    fn signaling_message_block(
        &self,
        decoded: &ForwardControlCapsuleDecode,
        message_index: usize,
        message: &ForwardSignalingMessage,
    ) -> SampleBlock {
        let chip_start = self.buffer_start_chip.unwrap_or(0) + decoded.slot_start_chip;
        let mut block = SampleBlock::new(message_payload_samples(&message.payload), chip_start);
        block.tags.insert("hrpd_forward_signaling_message", 1);
        block
            .tags
            .insert("hrpd_control_slot_start_chip", chip_start as i64);
        block
            .tags
            .insert("hrpd_control_message_index", message_index as i64);
        block.tags.insert(
            "hrpd_signaling_protocol_type",
            i64::from(message.protocol_type),
        );
        block
            .tags
            .insert("hrpd_signaling_ati_type", i64::from(message.ati_type));
        if let Some(ati) = message.ati {
            block.tags.insert("hrpd_signaling_ati", i64::from(ati));
        }
        block.tags.insert(
            "hrpd_signaling_message_id_bits",
            i64::from(message.message_id_bits),
        );
        block
            .tags
            .insert("hrpd_signaling_payload_len", message.payload.len() as i64);
        if let Some(message_id) = message.message_id {
            block
                .tags
                .insert("hrpd_signaling_message_id", i64::from(message_id));
        }
        if let Some(overhead) =
            HrpdOverheadMessage::decode_for_protocol(message.protocol_type, &message.payload)
        {
            block.tags.insert("hrpd_overhead_message_decoded", 1);
            block.tags.insert(
                "hrpd_overhead_message_type",
                overhead_message_type(&overhead),
            );
            if let HrpdOverheadMessage::Sync(sync) = overhead {
                block.tags.insert("hrpd_sync_decoded", 1);
                block
                    .tags
                    .insert("hrpd_sync_pilot_pn", i64::from(sync.pilot_pn));
                block
                    .tags
                    .insert("hrpd_sync_max_revision", i64::from(sync.maximum_revision));
                block
                    .tags
                    .insert("hrpd_sync_min_revision", i64::from(sync.minimum_revision));
                block
                    .tags
                    .insert("hrpd_sync_system_time", sync.system_time as i64);
                block
                    .tags
                    .insert("hrpd_sync_slot_start_chip", chip_start as i64);
            }
        }
        block
    }

    fn event_blocks(&self, decoded: &ForwardControlCapsuleDecode) -> Vec<SampleBlock> {
        let mut blocks = Vec::with_capacity(1 + decoded.signaling_messages.len());
        blocks.push(self.capsule_block(decoded));
        blocks.extend(
            decoded
                .signaling_messages
                .iter()
                .enumerate()
                .map(|(idx, message)| self.signaling_message_block(decoded, idx, message)),
        );
        blocks
    }

    fn trim_buffer(&mut self) {
        if self.buffer.len() <= FORWARD_OVERHEAD_RETAIN_CHIPS {
            return;
        }
        let mut drop_chips = self.buffer.len() - FORWARD_OVERHEAD_RETAIN_CHIPS;
        drop_chips -= drop_chips % SLOT_CHIPS as usize;
        if drop_chips == 0 {
            return;
        }
        self.buffer.drain(..drop_chips);
        if let Some(start) = &mut self.buffer_start_chip {
            *start += drop_chips;
            let min_recent = start.saturating_sub(FORWARD_OVERHEAD_RETAIN_CHIPS);
            self.emitted_slot_starts.retain(|chip| *chip >= min_recent);
        }
    }
}

impl Default for HrpdForwardOverheadProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineProcessor for HrpdForwardOverheadProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let Some(acquisition) = Self::acquisition_from_tags(&block) else {
            return Vec::new();
        };
        if self.acquisition.is_none() {
            self.acquisition = Some(acquisition);
            self.buffer_start_chip = Some(block.chip_start);
        } else if let Some(start) = self.buffer_start_chip {
            let expected = start + self.buffer.len();
            if block.chip_start != expected {
                self.buffer.clear();
                self.buffer_start_chip = Some(block.chip_start);
            }
        }
        self.buffer.extend_from_slice(&block.samples);

        let Some(acquisition) = self.acquisition.as_ref() else {
            return Vec::new();
        };

        let mut corrected = self.buffer.clone();
        let cfo_rad_per_chip =
            estimate_pilot_cfo_rad_per_chip(&corrected, acquisition.slot_phase_chips);
        correct_chip_cfo(&mut corrected, cfo_rad_per_chip);

        let buffer_start = self.buffer_start_chip.unwrap_or(0);
        if self.cycle_abs_phase.is_none() {
            if let Some(sync) = decode_sync_from_despread(&corrected, acquisition) {
                self.cycle_abs_phase =
                    Some((buffer_start + sync.slot_start_chip) % control_channel_cycle_chips());
            }
        }

        let Some(cycle_abs_phase) = self.cycle_abs_phase else {
            self.trim_buffer();
            return Vec::new();
        };
        let cycle_chips = control_channel_cycle_chips();
        let packet_start_phase =
            (cycle_abs_phase + cycle_chips - (buffer_start % cycle_chips)) % cycle_chips;
        let decoded = decode_control_capsules_at_cycle(&corrected, acquisition, packet_start_phase);
        let mut out = Vec::new();
        for capsule in decoded {
            let abs_slot_start = buffer_start + capsule.slot_start_chip;
            if self.emitted_slot_starts.contains(&abs_slot_start) {
                continue;
            }
            self.emitted_slot_starts.push(abs_slot_start);
            self.decoded_capsules += 1;
            self.decoded_messages += capsule.signaling_messages.len();
            out.extend(self.event_blocks(&capsule));
        }
        self.trim_buffer();
        out
    }

    fn name(&self) -> &'static str {
        "HrpdForwardOverheadProcessor"
    }

    fn metrics(&self) -> Vec<(&'static str, String)> {
        vec![
            ("buffered_chips", self.buffer.len().to_string()),
            ("decoded_capsules", self.decoded_capsules.to_string()),
            ("decoded_messages", self.decoded_messages.to_string()),
        ]
    }
}

fn message_payload_samples(payload: &[u8]) -> Vec<Complex32> {
    payload
        .iter()
        .map(|&byte| Complex32::new(f32::from(byte), 0.0))
        .collect()
}

#[cfg(test)]
fn message_payload_bytes(samples: &[Complex32]) -> Vec<u8> {
    samples
        .iter()
        .map(|sample| sample.re.round().clamp(0.0, 255.0) as u8)
        .collect()
}

fn overhead_message_type(message: &HrpdOverheadMessage) -> i64 {
    match message {
        HrpdOverheadMessage::Sync(_) => 0,
        HrpdOverheadMessage::QuickConfig(_) => 1,
        HrpdOverheadMessage::SectorParameters(_) => 2,
        HrpdOverheadMessage::AccessParameters(_) => 3,
        HrpdOverheadMessage::BroadcastReverseRateLimit(_) => 4,
    }
}

fn acquire_pilot_fft(samples: &[Complex32], samples_per_chip: usize) -> Option<PilotAcquisition> {
    // TDM-gated matched PN reference over a full PN period. Gating to the
    // 96-chip pilot window per half-slot avoids folding non-pilot chips into
    // the noise sum (~10 dB over a continuous matched reference).
    //
    // Full-period (32768 chips = 27 ms) gives unambiguous PN-offset resolution
    // and ~35 dB processing gain (3072 pilot chips × γ). 27 ms is CFO-sensitive
    // (sinc(Δf · 0.027) nulls at 37 Hz), so we sweep a fine ±392 Hz grid.
    const COHERENT_CHIPS: usize = 32_768;
    // 10 dB peak/mean. Pure-noise expectation on 32768 chip-aligned bins is
    // ln(N) ≈ 10.4 dB so this is right at the noise floor; relies on
    // downstream CRC to reject false positives.
    const SNR_LINEAR_THRESHOLD: f32 = 10.0;

    let window_len = COHERENT_CHIPS * samples_per_chip;
    if samples.len() < window_len {
        return None;
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(window_len);
    let ifft = planner.plan_fft_inverse(window_len);
    let scratch_len = fft
        .get_inplace_scratch_len()
        .max(ifft.get_inplace_scratch_len());
    let mut scratch = vec![Complex32::new(0.0, 0.0); scratch_len];

    // Build TDM-gated matched PN reference: matched (pulse-shaped) PN over
    // the full window, then zero everything outside the 96-chip pilot window
    // of each 1024-chip half-slot.
    let base_ref_samples = build_hrpd_matched_pilot_reference(window_len, samples_per_chip, 1);
    let mut ref_fft_variants = Vec::new();
    for gate_offset in (0..HALF_SLOT_CHIPS as usize).step_by(64) {
        let mut gated = base_ref_samples.clone();
        for (idx, s) in gated.iter_mut().enumerate() {
            let chip = idx / samples_per_chip;
            let half_chip = (chip + gate_offset) % HALF_SLOT_CHIPS as usize;
            if !(PILOT_START_IN_HALF_SLOT..PILOT_END_IN_HALF_SLOT).contains(&half_chip) {
                *s = Complex32::new(0.0, 0.0);
            }
        }
        for (despread_mode, reference) in [
            (PnDespreadMode::MultiplyPn, gated.clone()),
            (
                PnDespreadMode::MultiplyConjPn,
                gated.iter().map(|sample| sample.conj()).collect::<Vec<_>>(),
            ),
        ] {
            let mut ref_fft = reference;
            fft.process_with_scratch(&mut ref_fft, &mut scratch);
            ref_fft_variants.push((
                despread_mode,
                gate_offset,
                ref_fft.iter().map(|r| r.conj()).collect::<Vec<_>>(),
            ));
        }
    }

    // Fine CFO grid: 9 hypotheses × ~98 Hz step covers ±392 Hz, keeping
    // sinc loss under 1 dB across the band.
    let cfo_step: f32 = 0.000125;
    let cfo_hyps: [f32; 9] = [
        -4.0 * cfo_step,
        -3.0 * cfo_step,
        -2.0 * cfo_step,
        -cfo_step,
        0.0,
        cfo_step,
        2.0 * cfo_step,
        3.0 * cfo_step,
        4.0 * cfo_step,
    ];
    // Scan every (window, CFO) tuple; keep the best peak/mean. With this
    // TDM-gated reference + full-PN-period coherent window the noise-floor
    // expectation is ln(N) ≈ 10 dB, so any single window's peak can hit
    // the threshold from pure noise — first-hit-above-threshold would
    // routinely lock on noise. Taking the max over the whole capture lets
    // a real sector's consistent peak dominate.
    let max_windows = samples.len() / window_len;
    let mut best_snr = 0.0f32;
    let mut peak = 0.0f32;
    let mut peak_lag = 0usize;
    let mut peak_window_start = 0usize;
    let mut peak_despread_mode = PnDespreadMode::MultiplyPn;
    let mut peak_gate_offset = 0usize;
    let mut derot = vec![Complex32::new(0.0, 0.0); window_len];
    for win_idx in 0..max_windows {
        let start = win_idx * window_len;
        let block = &samples[start..start + window_len];
        for &cfo in &cfo_hyps {
            if cfo == 0.0 {
                derot.copy_from_slice(block);
            } else {
                for (n, dst) in derot.iter_mut().enumerate() {
                    let a = -cfo * n as f32;
                    *dst = block[n] * Complex32::new(a.cos(), a.sin());
                }
            }
            let mut signal_fft = derot.clone();
            fft.process_with_scratch(&mut signal_fft, &mut scratch);
            for (despread_mode, gate_offset, ref_fft_conj) in &ref_fft_variants {
                let mut prod: Vec<Complex32> = signal_fft
                    .iter()
                    .zip(ref_fft_conj.iter())
                    .map(|(s, r)| *s * *r)
                    .collect();
                ifft.process_with_scratch(&mut prod, &mut scratch);
                let mut win_peak = 0.0f32;
                let mut win_peak_lag = 0usize;
                let mut sum = 0.0f64;
                let mut count = 0usize;
                let mut idx = 0usize;
                while idx < prod.len() {
                    let m = prod[idx].norm_sqr();
                    if m > win_peak {
                        win_peak = m;
                        win_peak_lag = idx;
                    }
                    sum += m as f64;
                    count += 1;
                    idx += samples_per_chip;
                }
                let mean = (sum / count.max(1) as f64) as f32;
                let snr = win_peak / mean.max(1e-30);
                if snr > best_snr {
                    best_snr = snr;
                    peak = win_peak;
                    peak_lag = win_peak_lag;
                    peak_window_start = start;
                    peak_despread_mode = *despread_mode;
                    peak_gate_offset = *gate_offset;
                }
            }
        }
    }
    if best_snr < SNR_LINEAR_THRESHOLD {
        return None;
    }

    let lag_chips = (peak_lag / samples_per_chip) % 32_768;
    let pn_phase_chips = (32_768 - lag_chips) % 32_768;
    let slot_phase_from_gate =
        (lag_chips + HALF_SLOT_CHIPS as usize - peak_gate_offset) % HALF_SLOT_CHIPS as usize;
    let chip_variant = ChipStreamVariant::MatchedFilter;
    let despread_mode = peak_despread_mode;
    // Refinement is O(chip_count) per call × 65 deltas. Cap input to ~32
    // half-slots = one PN period (~27 ms) so a 10 s WAV doesn't blow up.
    let refine_chips_needed = (256 + 2) * HALF_SLOT_CHIPS as usize;
    let refine_samples_needed = refine_chips_needed * samples_per_chip;
    let refine_end = (peak_window_start + refine_samples_needed).min(samples.len());
    let refine_slice = &samples[peak_window_start..refine_end];
    let timing_phase = peak_lag % samples_per_chip;
    let chips = make_chip_stream(refine_slice, samples_per_chip, timing_phase, chip_variant);
    let coarse = PilotAcquisition {
        samples_per_chip,
        timing_phase,
        chip_variant,
        pn_phase_chips,
        pn_offset: phase_to_offset(pn_phase_chips),
        slot_phase_chips: slot_phase_from_gate,
        despread_mode,
        pilot_metric: peak.sqrt(),
        pilot_snr_db: 10.0 * best_snr.max(1e-12).log10(),
    };
    let phase_refined = coarse;
    let mut refined = score_pilot_noncoherent_phase(
        &chips,
        samples_per_chip,
        timing_phase,
        chip_variant,
        phase_refined.pn_phase_chips,
        phase_refined.pn_offset,
        despread_mode,
        256,
    );
    refined.pilot_snr_db = phase_refined.pilot_snr_db;
    refined.pilot_metric = refined.pilot_metric.max(phase_refined.pilot_metric);
    correct_halfslot_phase_from_coherent_pilot(&chips, &mut refined);
    Some(refined)
}

fn score_pilot_noncoherent_phase(
    chips: &[Complex32],
    samples_per_chip: usize,
    timing_phase: usize,
    chip_variant: ChipStreamVariant,
    pn_phase_chips: usize,
    pn_offset: u16,
    despread_mode: PnDespreadMode,
    max_halfslots: usize,
) -> PilotAcquisition {
    let burst = PILOT_END_IN_HALF_SLOT - PILOT_START_IN_HALF_SLOT;
    let half = HALF_SLOT_CHIPS as usize;
    let mut pn = pn_sequence_at_phase(pn_phase_chips);
    let mut prefix = Vec::with_capacity(chips.len() + 1);
    prefix.push(Complex32::new(0.0, 0.0));
    for sample in chips {
        let p = pn.generate_iq();
        let z = match despread_mode {
            PnDespreadMode::MultiplyPn => *sample * p,
            PnDespreadMode::MultiplyConjPn => *sample * p.conj(),
        };
        prefix.push(*prefix.last().expect("prefix has initial zero") + z);
    }

    let usable_halfslots = max_halfslots.min(chips.len() / half).max(1);
    let mut metrics = vec![0.0f32; half];
    let mut counts = vec![0usize; half];
    for pilot_start_phase in 0..half {
        let mut metric = 0.0f32;
        let mut count = 0usize;
        for n in 0..usable_halfslots {
            let start = pilot_start_phase + n * half;
            let end = start + burst;
            if end > chips.len() {
                break;
            }
            let corr = prefix[end] - prefix[start];
            metric += corr.norm_sqr();
            count += 1;
        }
        metrics[pilot_start_phase] = metric;
        counts[pilot_start_phase] = count;
    }

    let min_count = usable_halfslots.min(4);
    let (best_pilot_start, best_metric) = metrics
        .iter()
        .copied()
        .enumerate()
        .filter(|(phase, _)| counts[*phase] >= min_count)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap_or((PILOT_START_IN_HALF_SLOT, 0.0));
    let mut noise_sum = 0.0f32;
    let mut noise_count = 0usize;
    for (phase, metric) in metrics.iter().copied().enumerate() {
        let d = phase.abs_diff(best_pilot_start);
        let dist = d.min(half - d);
        if counts[phase] >= min_count && dist > burst {
            noise_sum += metric;
            noise_count += 1;
        }
    }
    let noise = (noise_sum / noise_count.max(1) as f32).max(1e-12);
    let snr = (best_metric / noise).max(1e-12);
    let halfslot_phase = (best_pilot_start + half - PILOT_START_IN_HALF_SLOT) % half;

    PilotAcquisition {
        samples_per_chip,
        timing_phase,
        chip_variant,
        pn_phase_chips,
        pn_offset,
        slot_phase_chips: halfslot_phase,
        despread_mode,
        pilot_metric: best_metric.sqrt(),
        pilot_snr_db: 10.0 * snr.log10(),
    }
}

fn correct_halfslot_phase_from_coherent_pilot(chips: &[Complex32], acq: &mut PilotAcquisition) {
    let despread = despread_chips(chips, acq.pn_phase_chips, acq.despread_mode);
    let half = HALF_SLOT_CHIPS as usize;
    let (best_start, _) = coherent_pilot_alignment(&despread, acq.slot_phase_chips, 512);
    let phase_error = signed_circular_delta(best_start, PILOT_START_IN_HALF_SLOT, half);
    acq.slot_phase_chips =
        (acq.slot_phase_chips as isize + phase_error).rem_euclid(half as isize) as usize;
}

fn signed_circular_delta(value: usize, target: usize, period: usize) -> isize {
    let half = (period / 2) as isize;
    let mut delta = value as isize - target as isize;
    if delta > half {
        delta -= period as isize;
    } else if delta < -half {
        delta += period as isize;
    }
    delta
}

fn coherent_pilot_alignment(
    despread: &[Complex32],
    halfslot_phase: usize,
    max_halfslots: usize,
) -> (usize, f32) {
    let half = HALF_SLOT_CHIPS as usize;
    let burst = PILOT_END_IN_HALF_SLOT - PILOT_START_IN_HALF_SLOT;
    let max_start = half - burst;
    let mut coherent_fold = vec![Complex32::new(0.0, 0.0); half];
    let mut base = halfslot_phase;
    let mut halfslots = 0usize;
    while base + half <= despread.len() && halfslots < max_halfslots {
        let mut prefix = vec![Complex32::new(0.0, 0.0); half + 1];
        for idx in 0..half {
            prefix[idx + 1] = prefix[idx] + despread[base + idx];
        }
        let h = prefix[PILOT_END_IN_HALF_SLOT] - prefix[PILOT_START_IN_HALF_SLOT];
        let rot = if h.norm_sqr() > 1e-12 {
            h.conj() / h.norm_sqr().sqrt()
        } else {
            Complex32::new(1.0, 0.0)
        };
        for idx in 0..half {
            coherent_fold[idx] += despread[base + idx] * rot;
        }
        base += half;
        halfslots += 1;
    }

    if halfslots == 0 {
        return (PILOT_START_IN_HALF_SLOT, 0.0);
    }

    let mut prefix = vec![Complex32::new(0.0, 0.0); half + 1];
    for idx in 0..half {
        prefix[idx + 1] = prefix[idx] + coherent_fold[idx];
    }
    let mut best_start = 0usize;
    let mut best_metric = 0.0f32;
    for start in 0..=max_start {
        let metric = (prefix[start + burst] - prefix[start]).norm_sqr();
        if metric > best_metric {
            best_metric = metric;
            best_start = start;
        }
    }
    let pilot_metric =
        (prefix[PILOT_END_IN_HALF_SLOT] - prefix[PILOT_START_IN_HALF_SLOT]).norm_sqr();
    let mut outside_metric_sum = 0.0f32;
    let mut outside_count = 0usize;
    for start in 0..=max_start {
        if start.abs_diff(PILOT_START_IN_HALF_SLOT) > burst {
            outside_metric_sum += (prefix[start + burst] - prefix[start]).norm_sqr();
            outside_count += 1;
        }
    }
    let outside_mean = outside_metric_sum / outside_count.max(1) as f32;
    let margin_db = 10.0 * (pilot_metric / outside_mean.max(1e-12)).log10();
    (best_start, margin_db)
}

fn build_hrpd_fft_search_pn_samples(output_len: usize, oversample: usize) -> Vec<Complex32> {
    let mut pn = HrpdForwardPnSequence::new_repeat(0, 32_768, oversample.saturating_sub(1));
    (0..output_len).map(|_| pn.generate_iq()).collect()
}

fn build_hrpd_matched_pilot_reference(
    output_len: usize,
    oversample: usize,
    filter_passes: usize,
) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    let mut ref_matched = (0..filter_passes)
        .map(|_| ComplexFir32::new(&taps))
        .collect::<Vec<_>>();
    let pn = build_hrpd_fft_search_pn_samples(output_len, oversample);

    pn.into_iter()
        .map(|s| {
            let mut sample = Complex32::new(s.re, -s.im);
            for filter in &mut ref_matched {
                sample = filter.process_sample(sample);
            }
            sample
        })
        .collect()
}

fn samples_per_chip(sample_rate_hz: u32) -> Option<usize> {
    if sample_rate_hz % HRPD_CHIP_RATE_HZ != 0 {
        return None;
    }
    let n = (sample_rate_hz / HRPD_CHIP_RATE_HZ) as usize;
    (n > 0).then_some(n)
}

fn make_chip_stream(
    samples: &[Complex32],
    samples_per_chip: usize,
    timing_phase: usize,
    variant: ChipStreamVariant,
) -> Vec<Complex32> {
    let mut out = Vec::with_capacity(samples.len().saturating_sub(timing_phase) / samples_per_chip);
    let mut idx = timing_phase;
    while idx < samples.len() {
        let sample = match variant {
            ChipStreamVariant::Decimated => samples[idx],
            ChipStreamVariant::Boxcar => {
                let end = (idx + samples_per_chip).min(samples.len());
                let mut acc = Complex32::new(0.0, 0.0);
                for s in &samples[idx..end] {
                    acc += *s;
                }
                acc / ((end - idx).max(1) as f32)
            }
            ChipStreamVariant::MatchedFilter if samples_per_chip >= 4 => {
                if idx + HRPD_BASEBAND_FILTER.len() > samples.len() {
                    break;
                }
                let mut acc = Complex32::new(0.0, 0.0);
                for (tap, coeff) in HRPD_BASEBAND_FILTER.iter().enumerate() {
                    acc += samples[idx + tap] * *coeff;
                }
                acc
            }
            ChipStreamVariant::MatchedFilter => samples[idx],
        };
        out.push(sample);
        idx += samples_per_chip;
    }
    out
}

#[allow(clippy::excessive_precision)]
const HRPD_BASEBAND_FILTER: [f32; 48] = [
    -0.025288315,
    -0.034167931,
    -0.035752323,
    -0.016733702,
    0.021602514,
    0.064938487,
    0.091002137,
    0.081894974,
    0.037071157,
    -0.021998074,
    -0.060716277,
    -0.051178658,
    0.007874526,
    0.084368728,
    0.126869306,
    0.094528345,
    -0.012839661,
    -0.143477028,
    -0.211829088,
    -0.140513128,
    0.094601918,
    0.441387140,
    0.785875640,
    1.0,
    1.0,
    0.785875640,
    0.441387140,
    0.094601918,
    -0.140513128,
    -0.211829088,
    -0.143477028,
    -0.012839661,
    0.094528345,
    0.126869306,
    0.084368728,
    0.007874526,
    -0.051178658,
    -0.060716277,
    -0.021998074,
    0.037071157,
    0.081894974,
    0.091002137,
    0.064938487,
    0.021602514,
    -0.016733702,
    -0.035752323,
    -0.034167931,
    -0.025288315,
];

fn phase_to_offset(phase: usize) -> u16 {
    (((32_768usize - (phase % 32_768usize)) / 64) & 0x01ff) as u16
}

fn pn_sequence_at_phase(phase_chips: usize) -> HrpdForwardPnSequence {
    let mut pn = HrpdForwardPnSequence::new(0, 32_768);
    pn.advance_chips(phase_chips as u64);
    pn
}

fn despread_chips(
    chips: &[Complex32],
    pn_phase_chips: usize,
    mode: PnDespreadMode,
) -> Vec<Complex32> {
    let mut pn = pn_sequence_at_phase(pn_phase_chips);
    chips
        .iter()
        .map(|sample| {
            let p = pn.generate_iq();
            despread_sample_to_spec(*sample, p, mode)
        })
        .collect()
}

fn despread_sample_to_spec(sample: Complex32, pn: Complex32, mode: PnDespreadMode) -> Complex32 {
    match mode {
        // The transmitter emits the conjugated analytic envelope of the spec
        // I/Q arm outputs. Multiplying by PN acquires those captures, then
        // conjugating here returns the modulation symbols to spec orientation.
        PnDespreadMode::MultiplyPn => (sample * pn).conj(),
        PnDespreadMode::MultiplyConjPn => sample * pn.conj(),
    }
}

fn estimate_pilot_cfo_rad_per_chip(despread: &[Complex32], halfslot_phase: usize) -> f32 {
    let half = HALF_SLOT_CHIPS as usize;
    let mut prev: Option<(usize, Complex32)> = None;
    let mut weighted_phase = 0.0f32;
    let mut weight_sum = 0.0f32;
    let mut base = halfslot_phase;
    while base + half <= despread.len() {
        let start = base + PILOT_START_IN_HALF_SLOT;
        let end = base + PILOT_END_IN_HALF_SLOT;
        if end <= despread.len() {
            let sum = despread[start..end]
                .iter()
                .copied()
                .fold(Complex32::new(0.0, 0.0), |acc, v| acc + v);
            if let Some((prev_center, prev_sum)) = prev {
                let center = (start + end) / 2;
                let delta_chips = center.saturating_sub(prev_center).max(1) as f32;
                let cross = sum * prev_sum.conj();
                let weight = cross.norm();
                weighted_phase += cross.arg() / delta_chips * weight;
                weight_sum += weight;
            }
            prev = Some(((start + end) / 2, sum));
        }
        base += half;
    }
    if weight_sum <= 1e-12 {
        0.0
    } else {
        weighted_phase / weight_sum
    }
}

fn correct_chip_cfo(chips: &mut [Complex32], rad_per_chip: f32) {
    if rad_per_chip.abs() <= 1e-9 {
        return;
    }
    for (idx, chip) in chips.iter_mut().enumerate() {
        let phase = -rad_per_chip * idx as f32;
        let rot = Complex32::new(phase.cos(), phase.sin());
        *chip *= rot;
    }
}

fn decode_sync_from_despread(
    despread: &[Complex32],
    acq: &PilotAcquisition,
) -> Option<ForwardSyncDecode> {
    // The FFT/matched-filter acquisition pins the pilot burst, but the FIR
    // timing center can leave the TDM chip boundary off by a couple of chips.
    // Refine only in that local chip neighborhood; acceptance remains gated by
    // physical FCS and a parsed Sync capsule.
    for slot_phase in ranked_pilot_aligned_slot_phase_candidates(despread, acq) {
        let equalized = pilot_equalize_half_slots(despread, slot_phase);
        // Sync starts at slot 0 of each Control Channel cycle (256 slots), but
        // the cycle anchor is unknown in a capture. Rank spec low-rate Control
        // formats only; broader rows remain diagnostics.
        for (_, slot_start, rate) in strongest_sync_preamble_candidates(&equalized, slot_phase, 64)
        {
            if let Some((messages, overhead_messages, sync)) =
                decode_low_rate_control_packet_with_expected_pilot(
                    &equalized, slot_start, rate, None,
                )
            {
                return Some(ForwardSyncDecode {
                    acquisition: acq.clone(),
                    slot_start_chip: slot_start,
                    payload_bits: rate.payload_bits,
                    messages,
                    overhead_messages,
                    sync,
                });
            }
        }
    }
    None
}

fn control_channel_cycle_chips() -> usize {
    CONTROL_CHANNEL_CYCLE_SLOTS * SLOT_CHIPS as usize
}

fn decode_control_capsules_at_cycle(
    despread: &[Complex32],
    acq: &PilotAcquisition,
    packet_start_phase: usize,
) -> Vec<ForwardControlCapsuleDecode> {
    let cycle_chips = control_channel_cycle_chips();
    let slot_phases = ranked_pilot_aligned_slot_phase_candidates(despread, acq);
    let equalized_by_phase = slot_phases
        .iter()
        .copied()
        .map(|slot_phase| (slot_phase, pilot_equalize_half_slots(despread, slot_phase)))
        .collect::<Vec<_>>();

    let mut decoded = Vec::new();
    for slot_start in (packet_start_phase..despread.len()).step_by(cycle_chips) {
        let mut cycle_decode = None;
        'phase: for (_, equalized) in &equalized_by_phase {
            for rate in LowRateControl::ALL {
                let Some(capsule) =
                    decode_low_rate_control_packet_capsule(equalized, slot_start, rate)
                else {
                    continue;
                };
                if capsule.signaling_messages.is_empty() {
                    continue;
                }
                cycle_decode = Some(ForwardControlCapsuleDecode {
                    acquisition: acq.clone(),
                    slot_start_chip: slot_start,
                    payload_bits: rate.payload_bits,
                    signaling_messages: capsule
                        .signaling_messages
                        .iter()
                        .map(forward_signaling_message)
                        .collect(),
                    overhead_messages: capsule.overhead_messages,
                    sync: capsule.sync,
                });
                break 'phase;
            }
        }
        if let Some(capsule) = cycle_decode {
            decoded.push(capsule);
        }
    }

    decoded
}

fn pilot_aligned_slot_phase_candidates(acq: &PilotAcquisition) -> Vec<usize> {
    let half = HALF_SLOT_CHIPS as usize;
    let base = acq.slot_phase_chips % half;
    [-2isize, -1, 0, 1, 2]
        .into_iter()
        .map(|delta| (base as isize + delta).rem_euclid(half as isize) as usize)
        .collect()
}

fn ranked_pilot_aligned_slot_phase_candidates(
    despread: &[Complex32],
    acq: &PilotAcquisition,
) -> Vec<usize> {
    let mut ranked = pilot_aligned_slot_phase_candidates(acq)
        .into_iter()
        .map(|slot_phase| {
            let equalized = pilot_equalize_half_slots(despread, slot_phase);
            let metric = strongest_sync_preamble_candidates(&equalized, slot_phase, 1)
                .first()
                .map(|(metric, _, _)| *metric)
                .unwrap_or(0.0);
            (metric, slot_phase)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked
        .into_iter()
        .map(|(_, slot_phase)| slot_phase)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct LowRateControl {
    spec: ControlPhySpec,
    payload_bits: u32,
    slots: usize,
    preamble_chips: usize,
    data_chips: usize,
    mac_index: u8,
    rate_code: u8,
    b_code: u8,
    d4: u8,
    #[allow(dead_code)]
    preamble_cover_chips: usize,
}

impl LowRateControl {
    // Control Channel formats that can carry synchronous / quick synchronous
    // overhead capsules. Subtype 3+ uses the subtype 5 scrambler-state table
    // for the shared EV-DO Rev A/B CDMA Control formats.
    const ALL: [Self; 14] = [
        // 38.4 kbps Sync — C.S0024-0 §9.3.1.3.1.1, §9.3.1.3.2.4 (MACIndex 3).
        Self {
            spec: ControlPhySpec::Subtype0,
            payload_bits: 1024,
            slots: 16,
            preamble_chips: 1024,
            data_chips: 24_576,
            mac_index: 3,
            rate_code: 0b0001,
            b_code: 0,
            d4: 0,
            preamble_cover_chips: 32,
        },
        // 76.8 kbps Control — same spec, MACIndex 2.
        Self {
            spec: ControlPhySpec::Subtype0,
            payload_bits: 1024,
            slots: 8,
            preamble_chips: 512,
            data_chips: 12_288,
            mac_index: 2,
            rate_code: 0b0010,
            b_code: 0,
            d4: 0,
            preamble_cover_chips: 32,
        },
        // Subtype 2 physical layer: same low-rate Control formats, but the
        // scrambler seed is [111 b2..b0 r6..r0 d3..d0] with b=111 for 1024-bit
        // packets (C.S0024 §2.4.1.3.2.3.3-1).
        Self {
            spec: ControlPhySpec::Subtype2,
            payload_bits: 1024,
            slots: 16,
            preamble_chips: 1024,
            data_chips: 24_576,
            mac_index: 3,
            rate_code: 0b0001,
            b_code: 0b111,
            d4: 0,
            preamble_cover_chips: 64,
        },
        Self {
            spec: ControlPhySpec::Subtype2,
            payload_bits: 1024,
            slots: 8,
            preamble_chips: 512,
            data_chips: 12_288,
            mac_index: 2,
            rate_code: 0b0010,
            b_code: 0b111,
            d4: 0,
            preamble_cover_chips: 64,
        },
        // Subtype 2 short Control formats for asynchronous/sub-synchronous
        // capsules: C.S0024-A §13.3.1.3.2.4 permits (128,4,1024),
        // (256,4,1024), and (512,4,1024) using MACIndex 71.
        Self {
            spec: ControlPhySpec::Subtype2,
            payload_bits: 128,
            slots: 4,
            preamble_chips: 1024,
            data_chips: 5_376,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b000,
            d4: 0,
            preamble_cover_chips: 64,
        },
        Self {
            spec: ControlPhySpec::Subtype2,
            payload_bits: 256,
            slots: 4,
            preamble_chips: 1024,
            data_chips: 5_376,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b001,
            d4: 0,
            preamble_cover_chips: 64,
        },
        Self {
            spec: ControlPhySpec::Subtype2,
            payload_bits: 512,
            slots: 4,
            preamble_chips: 1024,
            data_chips: 5_376,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b010,
            d4: 0,
            preamble_cover_chips: 64,
        },
        // Subtype 3 and later: same b/d table rows for these two Control
        // formats, with seed [1 r7 d4 b2..b0 r6..r0 d3..d0]. The preamble
        // cover is a 128-chip bi-orthogonal sequence (C.S0024-200-C
        // §3.4.1.3.2.3.1 / §5.6.1.3.2.5.1).
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 1024,
            slots: 16,
            preamble_chips: 1024,
            data_chips: 24_576,
            mac_index: 3,
            rate_code: 0b0001,
            b_code: 0b111,
            d4: 0,
            preamble_cover_chips: 128,
        },
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 1024,
            slots: 8,
            preamble_chips: 512,
            data_chips: 12_288,
            mac_index: 2,
            rate_code: 0b0010,
            b_code: 0b111,
            d4: 0,
            preamble_cover_chips: 128,
        },
        // Subtype 5 quick synchronous capsule: C.S0024-200 §5.6.1.3.2.8 and
        // Table 5.6.1.3.2.4-4 specify MACIndex 71 with a 256-chip preamble.
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 128,
            slots: 4,
            preamble_chips: 256,
            data_chips: 6_144,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b000,
            d4: 0,
            preamble_cover_chips: 128,
        },
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 256,
            slots: 4,
            preamble_chips: 256,
            data_chips: 6_144,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b001,
            d4: 0,
            preamble_cover_chips: 128,
        },
        // Subtype 5 async/subsync short Control formats. These use the same
        // MACIndex 71 preamble cover but a 1024-chip preamble in the 4-slot
        // timing diagram.
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 128,
            slots: 4,
            preamble_chips: 1024,
            data_chips: 5_376,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b000,
            d4: 0,
            preamble_cover_chips: 128,
        },
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 256,
            slots: 4,
            preamble_chips: 1024,
            data_chips: 5_376,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b001,
            d4: 0,
            preamble_cover_chips: 128,
        },
        Self {
            spec: ControlPhySpec::Subtype3Plus,
            payload_bits: 512,
            slots: 4,
            preamble_chips: 1024,
            data_chips: 5_376,
            mac_index: 71,
            rate_code: 0b0011,
            b_code: 0b010,
            d4: 0,
            preamble_cover_chips: 128,
        },
    ];

    const SYNC: [Self; 6] = [
        Self::ALL[0],
        Self::ALL[1],
        Self::ALL[2],
        Self::ALL[3],
        Self::ALL[7],
        Self::ALL[8],
    ];

    fn scrambler(&self) -> crate::phy::hrpd::scrambler::HrpdForwardScrambler {
        crate::phy::hrpd::scrambler::HrpdForwardScrambler::with_initial_state(
            self.scrambler_initial_state(),
        )
    }

    fn slot_stride(&self) -> usize {
        physical_packet_slot_stride(self.slots)
    }

    fn packet_span_chips(&self) -> usize {
        if self.slots == 0 {
            return 0;
        }
        (1 + (self.slots - 1) * self.slot_stride()) * SLOT_CHIPS as usize
    }

    fn scrambler_initial_state(&self) -> u32 {
        match self.spec {
            ControlPhySpec::Subtype0 => {
                let leading = 0x7fu32 << 10;
                let r = (u32::from(self.mac_index) & 0x3f) << 4;
                let d = u32::from(self.rate_code) & 0x0f;
                leading | r | d
            }
            ControlPhySpec::Subtype2 => {
                let leading = 0b111u32 << 14;
                let b = (u32::from(self.b_code) & 0x07) << 11;
                let r = (u32::from(self.mac_index) & 0x7f) << 4;
                let d = u32::from(self.rate_code) & 0x0f;
                leading | b | r | d
            }
            ControlPhySpec::Subtype3Plus => {
                let r = u32::from(self.mac_index);
                let leading = 1u32 << 16;
                let r7 = ((r >> 7) & 1) << 15;
                let d4 = (u32::from(self.d4) & 1) << 14;
                let b = (u32::from(self.b_code) & 0x07) << 11;
                let r_low = (r & 0x7f) << 4;
                let d = u32::from(self.rate_code) & 0x0f;
                leading | r7 | d4 | b | r_low | d
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlPhySpec {
    Subtype0,
    Subtype2,
    Subtype3Plus,
}

#[cfg(test)]
fn decode_low_rate_control_packet(
    equalized: &[Complex32],
    slot_start: usize,
    rate: LowRateControl,
) -> Option<(Vec<Vec<u8>>, DecodedSyncMessage)> {
    decode_low_rate_control_packet_with_expected_pilot(equalized, slot_start, rate, None)
        .map(|(messages, _, sync)| (messages, sync))
}

fn decode_low_rate_control_packet_with_expected_pilot(
    equalized: &[Complex32],
    slot_start: usize,
    rate: LowRateControl,
    expected_pilot_pn: Option<u16>,
) -> Option<(Vec<Vec<u8>>, Vec<HrpdOverheadMessage>, DecodedSyncMessage)> {
    let capsule = decode_low_rate_control_packet_capsule(equalized, slot_start, rate)?;
    let sync = capsule.sync?;
    if expected_pilot_pn.is_some_and(|pn| sync.pilot_pn != (pn & 0x01ff)) {
        return None;
    }
    Some((capsule.messages, capsule.overhead_messages, sync))
}

fn decode_low_rate_control_packet_capsule(
    equalized: &[Complex32],
    slot_start: usize,
    rate: LowRateControl,
) -> Option<DecodedControlMacCapsule> {
    let tdm_data = collect_packet_data_region(equalized, slot_start, rate.slots)?;
    if tdm_data.len() < rate.preamble_chips + rate.data_chips {
        return None;
    }
    let data = &tdm_data[rate.preamble_chips..rate.preamble_chips + rate.data_chips];
    let repeated_symbols = walsh16_decover(data);
    if repeated_symbols.len() != rate.data_chips {
        return None;
    }

    // The chain is deterministic at this point: pilot is equalized to
    // +1 (per-half-slot conj-rotation), QPSK constellation is fixed by
    // spec Table 9.3.1.3.2.3.5.1-1. No sign / I-Q-swap brute force.
    let repeated_llrs = qpsk_llrs(&repeated_symbols, SymbolVariant::IDENTITY);
    let provided_symbols = rate.payload_bits as usize * 5 / 2;
    let mut llrs = vec![0.0f32; provided_symbols * 2];
    for (idx, pair) in repeated_llrs.chunks_exact(2).enumerate() {
        let dst = idx % provided_symbols;
        llrs[dst * 2] += pair[0];
        llrs[dst * 2 + 1] += pair[1];
    }

    let deinterleaved = rate_1_5_channel_deinterleave(rate, &llrs);
    let mut descrambled = deinterleaved;
    apply_scrambler_soft(&mut descrambled, rate);
    normalize_soft_llrs(&mut descrambled, 4.0);
    let decoder = HrpdTurboDecoder::new(rate.payload_bits)?.with_iterations(16);
    let decoded = decoder.decode(&descrambled);
    if !control_physical_fcs_ok(&decoded, rate.payload_bits as usize) {
        return None;
    }
    let mac_bits = control_mac_bits(&decoded, rate.payload_bits as usize)?;
    parse_control_mac_capsule_for_spec(mac_bits, rate.spec)
}

fn strongest_sync_preamble_candidates(
    equalized: &[Complex32],
    slot_phase: usize,
    keep: usize,
) -> Vec<(f32, usize, LowRateControl)> {
    let mut top = Vec::new();
    for rate in LowRateControl::SYNC {
        top.extend(strongest_control_preamble_candidates_for_rate(
            equalized, slot_phase, rate, keep,
        ));
    }
    top.sort_by(|a, b| b.0.total_cmp(&a.0));
    top.truncate(keep);
    top
}

fn strongest_control_preamble_candidates_for_rate(
    equalized: &[Complex32],
    slot_phase: usize,
    rate: LowRateControl,
    keep: usize,
) -> Vec<(f32, usize, LowRateControl)> {
    let mut top = Vec::new();
    let slot = SLOT_CHIPS as usize;
    let half = HALF_SLOT_CHIPS as usize;
    let packet_span = rate.packet_span_chips();
    for phase in [slot_phase % slot, (slot_phase + half) % slot] {
        let mut packet_start = phase;
        while packet_start + packet_span <= equalized.len() {
            if let Some(metric) = control_preamble_metric(equalized, packet_start, rate) {
                top.push((metric, packet_start, rate));
            }
            packet_start += slot;
        }
    }
    top.sort_by(|a, b| b.0.total_cmp(&a.0));
    top.truncate(keep);
    top
}

fn control_preamble_metric(
    equalized: &[Complex32],
    slot_start: usize,
    rate: LowRateControl,
) -> Option<f32> {
    let tdm_data = collect_packet_data_region(equalized, slot_start, rate.slots)?;
    if tdm_data.len() < rate.preamble_chips {
        return None;
    }
    let row = usize::from(rate.mac_index >> 1);
    let complement = (rate.mac_index & 1) != 0;
    let mut corr = Complex32::new(0.0, 0.0);
    let mut power = 0.0f32;
    for (idx, sample) in tdm_data[..rate.preamble_chips].iter().enumerate() {
        let mut sign = walsh_biorthogonal(row, idx % rate.preamble_cover_chips);
        if complement {
            sign = -sign;
        }
        corr += *sample * sign;
        power += sample.norm_sqr();
    }
    Some(corr.norm() / power.max(1e-12).sqrt())
}

fn collect_packet_data_region(
    equalized: &[Complex32],
    slot_start: usize,
    slots: usize,
) -> Option<Vec<Complex32>> {
    collect_packet_data_region_with_stride(
        equalized,
        slot_start,
        slots,
        physical_packet_slot_stride(slots),
    )
}

fn physical_packet_slot_stride(slots: usize) -> usize {
    if slots > 1 { 4 } else { 1 }
}

fn collect_packet_data_region_with_stride(
    equalized: &[Complex32],
    slot_start: usize,
    slots: usize,
    slot_stride: usize,
) -> Option<Vec<Complex32>> {
    let mut out = Vec::with_capacity(slots * 1_600);
    for s in 0..slots {
        let slot = slot_start + s * slot_stride * SLOT_CHIPS as usize;
        if slot + SLOT_CHIPS as usize > equalized.len() {
            return None;
        }
        for half_base in [0usize, HALF_SLOT_CHIPS as usize] {
            let first = slot + half_base;
            out.extend_from_slice(&equalized[first..first + DATA_EDGE_CHIPS as usize]);
            let second = first + 624;
            out.extend_from_slice(&equalized[second..second + DATA_EDGE_CHIPS as usize]);
        }
    }
    Some(out)
}

fn walsh16_decover(chips: &[Complex32]) -> Vec<Complex32> {
    let mut out = Vec::with_capacity(chips.len());
    for group in chips.chunks_exact(16) {
        for row in 0..16 {
            let mut acc = Complex32::new(0.0, 0.0);
            for (col, sample) in group.iter().enumerate() {
                acc += *sample * walsh16(row, col);
            }
            out.push(acc);
        }
    }
    out
}

fn walsh16(row: usize, col: usize) -> f32 {
    walsh_biorthogonal(row, col)
}

fn walsh_biorthogonal(row: usize, col: usize) -> f32 {
    if ((row & col).count_ones() & 1) == 0 {
        1.0
    } else {
        -1.0
    }
}

fn pilot_equalize_half_slots(despread: &[Complex32], slot_phase: usize) -> Vec<Complex32> {
    let mut out = despread.to_vec();
    let slot = SLOT_CHIPS as usize;
    let half = HALF_SLOT_CHIPS as usize;
    let first_slot_start = if slot_phase == 0 { 0 } else { slot_phase };
    let mut slot_start = first_slot_start;
    let mut pilots = Vec::new();
    while slot_start + slot <= out.len() {
        for half_base in [0usize, half] {
            let start = slot_start + half_base;
            let p0 = start + PILOT_START_IN_HALF_SLOT;
            let p1 = start + PILOT_END_IN_HALF_SLOT;
            if p1 > despread.len() {
                continue;
            }
            let mut h = Complex32::new(0.0, 0.0);
            for sample in &despread[p0..p1] {
                h += *sample;
            }
            if h.norm_sqr() <= 1e-12 {
                continue;
            }
            pilots.push((
                start + (PILOT_START_IN_HALF_SLOT + PILOT_END_IN_HALF_SLOT) / 2,
                h,
            ));
        }
        slot_start += slot;
    }
    if pilots.is_empty() {
        return out;
    }

    let mut phases = Vec::with_capacity(pilots.len());
    for (idx, (_, h)) in pilots.iter().enumerate() {
        let mut phase = h.arg();
        if idx > 0 {
            let prev = phases[idx - 1];
            while phase - prev > std::f32::consts::PI {
                phase -= std::f32::consts::TAU;
            }
            while phase - prev < -std::f32::consts::PI {
                phase += std::f32::consts::TAU;
            }
        }
        phases.push(phase);
    }

    for idx in 0..out.len() {
        let pos = idx;
        let (phase, mag) = if pos <= pilots[0].0 || pilots.len() == 1 {
            (phases[0], pilots[0].1.norm())
        } else if pos >= pilots[pilots.len() - 1].0 {
            let last = pilots.len() - 1;
            (phases[last], pilots[last].1.norm())
        } else {
            let next = pilots
                .partition_point(|(chip, _)| *chip <= pos)
                .min(pilots.len() - 1);
            let prev = next - 1;
            let span = (pilots[next].0 - pilots[prev].0).max(1) as f32;
            let frac = (pos - pilots[prev].0) as f32 / span;
            let phase = phases[prev] + (phases[next] - phases[prev]) * frac;
            let mag =
                pilots[prev].1.norm() + (pilots[next].1.norm() - pilots[prev].1.norm()) * frac;
            (phase, mag)
        };
        let rot = Complex32::from_polar(1.0, -phase) / mag.max(1e-6);
        out[idx] *= rot;
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct SymbolVariant {
    conj: bool,
    invert_i: bool,
    invert_q: bool,
    swap_iq: bool,
}

impl SymbolVariant {
    /// Deterministic identity mapping: no conjugation, no sign flips, no IQ
    /// swap. After matched-PN despread + per-half-slot pilot equalize, this is
    /// the only correct variant.
    pub const IDENTITY: Self = Self {
        conj: false,
        invert_i: false,
        invert_q: false,
        swap_iq: false,
    };

    #[allow(dead_code)]
    const ALL: [Self; 16] = {
        let mut out = [Self {
            conj: false,
            invert_i: false,
            invert_q: false,
            swap_iq: false,
        }; 16];
        let mut i = 0;
        while i < 16 {
            out[i] = Self {
                conj: (i & 1) != 0,
                invert_i: (i & 2) != 0,
                invert_q: (i & 4) != 0,
                swap_iq: (i & 8) != 0,
            };
            i += 1;
        }
        out
    };
}

fn qpsk_llrs(symbols: &[Complex32], variant: SymbolVariant) -> Vec<f32> {
    let mut out = Vec::with_capacity(symbols.len() * 2);
    for &s0 in symbols {
        let (i, q) = qpsk_llr_pair(s0, variant);
        out.push(i);
        out.push(q);
    }
    out
}

fn qpsk_llr_pair(s0: Complex32, variant: SymbolVariant) -> (f32, f32) {
    let mut s = if variant.conj { s0.conj() } else { s0 };
    if variant.swap_iq {
        s = Complex32::new(s.im, s.re);
    }
    let i = if variant.invert_i { -s.re } else { s.re };
    let q = if variant.invert_q { -s.im } else { s.im };
    (i, q)
}

fn apply_scrambler_soft(llrs: &mut [f32], rate: LowRateControl) {
    let mut scrambler = rate.scrambler();
    for llr in llrs {
        if scrambler.next_bit() {
            *llr = -*llr;
        }
    }
}

fn normalize_soft_llrs(llrs: &mut [f32], target_rms: f32) {
    let mean_square = llrs.iter().map(|v| v * v).sum::<f32>() / (llrs.len().max(1) as f32);
    let rms = mean_square.sqrt();
    if rms <= 1e-6 {
        return;
    }
    let scale = target_rms / rms;
    for llr in llrs {
        *llr *= scale;
    }
}

fn forward_rate_1_5_deinterleave(payload_bits: usize, interleaved: &[f32]) -> Vec<f32> {
    forward_rate_1_5_deinterleave_with_table_order(payload_bits, interleaved, false)
}

fn rate_1_5_channel_deinterleave(rate: LowRateControl, interleaved: &[f32]) -> Vec<f32> {
    match rate.spec {
        ControlPhySpec::Subtype0 | ControlPhySpec::Subtype2 => {
            forward_rate_1_5_deinterleave(rate.payload_bits as usize, interleaved)
        }
        ControlPhySpec::Subtype3Plus => {
            subtype5_rate_1_5_matrix_deinterleave(rate.payload_bits as usize, interleaved)
        }
    }
}

#[cfg(test)]
fn rate_1_5_channel_interleave(rate: LowRateControl, input: &[u8]) -> Vec<u8> {
    match rate.spec {
        ControlPhySpec::Subtype0 | ControlPhySpec::Subtype2 => {
            forward_rate_1_5_interleave(rate.payload_bits as usize, input)
        }
        ControlPhySpec::Subtype3Plus => {
            subtype5_rate_1_5_matrix_interleave(rate.payload_bits as usize, input)
        }
    }
}

fn forward_rate_1_5_deinterleave_with_table_order(
    payload_bits: usize,
    interleaved: &[f32],
    table_order: bool,
) -> Vec<f32> {
    let coded_bits = payload_bits * 5;
    if interleaved.len() != coded_bits {
        return interleaved.to_vec();
    }
    let u_len = payload_bits;
    let v_len = payload_bits * 2;
    let u_cols = payload_bits / 2;
    let v_cols = payload_bits;
    let u = forward_symbol_depermute(&interleaved[..u_len], 2, u_cols, ForwardInterleaverBlock::U);
    let v0_vp0 = forward_symbol_depermute(
        &interleaved[u_len..u_len + v_len],
        2,
        v_cols,
        ForwardInterleaverBlock::V,
    );
    let v1_vp1 = forward_symbol_depermute(
        &interleaved[u_len + v_len..coded_bits],
        2,
        v_cols,
        ForwardInterleaverBlock::V,
    );

    let mut out = vec![0.0f32; coded_bits];
    for k in 0..payload_bits {
        out[k * 5] = u[k];
        out[k * 5 + 1] = v0_vp0[k];
        if table_order {
            out[k * 5 + 2] = v0_vp0[payload_bits + k];
            out[k * 5 + 3] = v1_vp1[k];
        } else {
            out[k * 5 + 2] = v1_vp1[k];
            out[k * 5 + 3] = v0_vp0[payload_bits + k];
        }
        out[k * 5 + 4] = v1_vp1[payload_bits + k];
    }
    out
}

#[cfg(test)]
fn forward_rate_1_5_deinterleave_1024(interleaved: &[f32]) -> Vec<f32> {
    forward_rate_1_5_deinterleave(1024, interleaved)
}

#[cfg(test)]
fn forward_rate_1_5_interleave_1024(input: &[u8]) -> Vec<u8> {
    forward_rate_1_5_interleave(1024, input)
}

#[cfg(test)]
fn forward_rate_1_5_interleave(payload_bits: usize, input: &[u8]) -> Vec<u8> {
    assert_eq!(input.len(), payload_bits * 5);
    let mut u = vec![0u8; payload_bits];
    let mut v0_vp0 = vec![0u8; payload_bits * 2];
    let mut v1_vp1 = vec![0u8; payload_bits * 2];
    for k in 0..payload_bits {
        u[k] = input[k * 5];
        v0_vp0[k] = input[k * 5 + 1];
        v0_vp0[payload_bits + k] = input[k * 5 + 3];
        v1_vp1[k] = input[k * 5 + 2];
        v1_vp1[payload_bits + k] = input[k * 5 + 4];
    }

    let u = forward_symbol_permute(&u, 2, payload_bits / 2, ForwardInterleaverBlock::U);
    let v0_vp0 = forward_symbol_permute(&v0_vp0, 2, payload_bits, ForwardInterleaverBlock::V);
    let v1_vp1 = forward_symbol_permute(&v1_vp1, 2, payload_bits, ForwardInterleaverBlock::V);
    [u, v0_vp0, v1_vp1].concat()
}

fn subtype5_rate_1_5_matrix_deinterleave(payload_bits: usize, interleaved: &[f32]) -> Vec<f32> {
    let coded_bits = payload_bits * 5;
    if interleaved.len() != coded_bits {
        return interleaved.to_vec();
    }
    let Some(params) = subtype5_matrix_params(payload_bits) else {
        return interleaved.to_vec();
    };

    let u = subtype5_matrix_depermute_block(&interleaved[..payload_bits], params, false);
    let v0_vp0 =
        subtype5_matrix_depermute_block(&interleaved[payload_bits..payload_bits * 3], params, true);
    let v1_vp1 =
        subtype5_matrix_depermute_block(&interleaved[payload_bits * 3..coded_bits], params, true);

    let mut out = vec![0.0f32; coded_bits];
    for k in 0..payload_bits {
        out[k * 5] = u[k];
        out[k * 5 + 1] = v0_vp0[k];
        out[k * 5 + 2] = v0_vp0[payload_bits + k];
        out[k * 5 + 3] = v1_vp1[k];
        out[k * 5 + 4] = v1_vp1[payload_bits + k];
    }
    out
}

#[cfg(test)]
fn subtype5_rate_1_5_matrix_interleave(payload_bits: usize, input: &[u8]) -> Vec<u8> {
    assert_eq!(input.len(), payload_bits * 5);
    let params = subtype5_matrix_params(payload_bits)
        .expect("Subtype 5 matrix parameters should exist for generated Control packet size");
    let mut u = vec![0u8; payload_bits];
    let mut v0_vp0 = vec![0u8; payload_bits * 2];
    let mut v1_vp1 = vec![0u8; payload_bits * 2];
    for k in 0..payload_bits {
        u[k] = input[k * 5];
        v0_vp0[k] = input[k * 5 + 1];
        v0_vp0[payload_bits + k] = input[k * 5 + 2];
        v1_vp1[k] = input[k * 5 + 3];
        v1_vp1[payload_bits + k] = input[k * 5 + 4];
    }

    let u = subtype5_matrix_permute_block(&u, params, false);
    let v0_vp0 = subtype5_matrix_permute_block(&v0_vp0, params, true);
    let v1_vp1 = subtype5_matrix_permute_block(&v1_vp1, params, true);
    [u, v0_vp0, v1_vp1].concat()
}

#[derive(Debug, Clone, Copy)]
struct Subtype5MatrixParams {
    n: usize,
    k: usize,
    r: usize,
    m: u32,
    l: usize,
    d: usize,
}

fn subtype5_matrix_params(payload_bits: usize) -> Option<Subtype5MatrixParams> {
    let m = match payload_bits {
        128 => 6,
        256 => 7,
        512 => 8,
        1024 => 9,
        _ => return None,
    };
    Some(Subtype5MatrixParams {
        n: payload_bits,
        k: 1,
        r: 2,
        m,
        l: 0,
        d: 4,
    })
}

fn subtype5_matrix_depermute_block(
    interleaved: &[f32],
    params: Subtype5MatrixParams,
    v_block: bool,
) -> Vec<f32> {
    let input_len = if v_block {
        2 * (params.n + params.l)
    } else {
        params.n + params.l
    };
    let mut out = vec![0.0f32; input_len];
    for old_idx in 0..input_len {
        if let Some(new_idx) = subtype5_matrix_output_index(old_idx, params, v_block) {
            if new_idx < interleaved.len() {
                out[old_idx] = interleaved[new_idx];
            }
        }
    }
    out
}

#[cfg(test)]
fn subtype5_matrix_permute_block(
    input: &[u8],
    params: Subtype5MatrixParams,
    v_block: bool,
) -> Vec<u8> {
    let output_len = if v_block {
        2 * params.n - params.l
    } else {
        params.n + params.l
    };
    let mut out = vec![0u8; output_len];
    for (old_idx, value) in input.iter().copied().enumerate() {
        if let Some(new_idx) = subtype5_matrix_output_index(old_idx, params, v_block) {
            if new_idx < output_len {
                out[new_idx] = value;
            }
        }
    }
    out
}

fn subtype5_matrix_output_index(
    input_idx: usize,
    params: Subtype5MatrixParams,
    v_block: bool,
) -> Option<usize> {
    let c = if v_block {
        1usize << (params.m + 1)
    } else {
        1usize << params.m
    };
    let k = input_idx % params.k;
    let col_row = input_idx / params.k;
    let col = col_row % c;
    let row = col_row / c;
    if row >= params.r {
        return None;
    }

    let shift = if v_block {
        ((params.k * col + k) / params.d) % params.r
    } else {
        (params.k * col + k) % params.r
    };
    let shifted_row = (row + shift) % params.r;
    let final_col = bit_reverse(col as u32, c.ilog2()) as usize;
    let final_level = if params.k == 5 {
        match k {
            1 => params.k / 2,
            x if x == params.k / 2 => 1,
            _ => k,
        }
    } else {
        (79 * k) % params.k
    };

    Some((final_level * c + final_col) * params.r + shifted_row)
}

#[derive(Debug, Clone, Copy)]
enum ForwardInterleaverBlock {
    U,
    V,
}

#[cfg(test)]
fn forward_symbol_permute(
    input: &[u8],
    k_rows: usize,
    m_cols: usize,
    block: ForwardInterleaverBlock,
) -> Vec<u8> {
    debug_assert_eq!(input.len(), k_rows * m_cols);
    let mut out = vec![0u8; input.len()];
    let bits = m_cols.ilog2();
    for j in 0..m_cols {
        let final_col = bit_reverse(j as u32, bits) as usize;
        let shift = match block {
            ForwardInterleaverBlock::U => j % k_rows,
            ForwardInterleaverBlock::V => (j / 4) % k_rows,
        };
        for final_row in 0..k_rows {
            let input_row = (final_row + k_rows - shift) % k_rows;
            let input_idx = input_row * m_cols + j;
            let output_idx = final_col * k_rows + final_row;
            out[output_idx] = input[input_idx];
        }
    }
    out
}

fn forward_symbol_depermute(
    interleaved: &[f32],
    k_rows: usize,
    m_cols: usize,
    block: ForwardInterleaverBlock,
) -> Vec<f32> {
    debug_assert_eq!(interleaved.len(), k_rows * m_cols);
    let mut out = vec![0.0f32; interleaved.len()];
    let bits = m_cols.ilog2();
    for j in 0..m_cols {
        let final_col = bit_reverse(j as u32, bits) as usize;
        let shift = match block {
            ForwardInterleaverBlock::U => j % k_rows,
            ForwardInterleaverBlock::V => (j / 4) % k_rows,
        };
        for final_row in 0..k_rows {
            let input_row = (final_row + k_rows - shift) % k_rows;
            let input_idx = input_row * m_cols + j;
            let output_idx = final_col * k_rows + final_row;
            out[input_idx] = interleaved[output_idx];
        }
    }
    out
}

fn bit_reverse(mut value: u32, bits: u32) -> u32 {
    let mut out = 0;
    for _ in 0..bits {
        out = (out << 1) | (value & 1);
        value >>= 1;
    }
    out
}

#[derive(Debug, Clone)]
struct DecodedControlMacCapsule {
    messages: Vec<Vec<u8>>,
    #[allow(dead_code)]
    signaling_messages: Vec<DecodedDefaultSignalingMessage>,
    overhead_messages: Vec<HrpdOverheadMessage>,
    sync: Option<DecodedSyncMessage>,
}

#[derive(Debug, Clone)]
struct DecodedDefaultSignalingMessage {
    ati_type: u8,
    ati: Option<u32>,
    protocol_type: u8,
    message_id: Option<u8>,
    message_id_bits: u8,
    payload: Vec<u8>,
}

fn forward_signaling_message(message: &DecodedDefaultSignalingMessage) -> ForwardSignalingMessage {
    ForwardSignalingMessage {
        ati_type: message.ati_type,
        ati: message.ati,
        protocol_type: message.protocol_type,
        message_id: message.message_id,
        message_id_bits: message.message_id_bits,
        payload: message.payload.clone(),
    }
}

#[derive(Debug, Clone)]
struct ControlMacSecurityPacket {
    security_layer_format: bool,
    connection_layer_format: bool,
    ati_type: u8,
    ati: Option<u32>,
    security_payload_bits: Vec<u8>,
}

fn parse_control_mac_capsule_for_spec(
    bits: &[u8],
    spec: ControlPhySpec,
) -> Option<DecodedControlMacCapsule> {
    let mac_bits = if bits.len() >= 1024 {
        if !physical_fcs_ok(bits) {
            return None;
        }
        &bits[..1002]
    } else if bits.len() <= 1002 {
        bits
    } else {
        return None;
    };

    let packets = parse_control_mac_packets(mac_bits, control_header_has_synchronous_bit(spec))?;
    let mut messages = Vec::new();
    let mut signaling_messages = Vec::new();
    let mut overhead_messages = Vec::new();
    let mut sync = None;
    for packet in packets {
        if packet.security_layer_format {
            continue;
        }
        let session_packets = parse_connection_layer_packets(
            &packet.security_payload_bits,
            packet.connection_layer_format,
        )?;
        for session_packet in session_packets {
            for mut message in parse_default_signaling_messages(&session_packet) {
                message.ati_type = packet.ati_type;
                message.ati = packet.ati;
                if let Some(overhead) = HrpdOverheadMessage::decode_for_protocol(
                    message.protocol_type,
                    &message.payload,
                ) {
                    if sync.is_none() {
                        if let HrpdOverheadMessage::Sync(sync_message) = &overhead {
                            sync = Some(sync_from_common(sync_message));
                        }
                    }
                    overhead_messages.push(overhead);
                }
                if sync.is_none()
                    && message.protocol_type == DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE
                {
                    sync = parse_sync_message_bits_at(&bytes_to_bits(&message.payload), 0);
                }
                messages.push(message.payload.clone());
                signaling_messages.push(message);
            }
        }
    }

    Some(DecodedControlMacCapsule {
        messages,
        signaling_messages,
        overhead_messages,
        sync,
    })
}

fn sync_from_common(sync: &SyncMessage) -> DecodedSyncMessage {
    DecodedSyncMessage {
        maximum_revision: sync.maximum_revision,
        minimum_revision: sync.minimum_revision,
        pilot_pn: sync.pilot_pn,
        system_time: sync.system_time,
    }
}

fn control_header_has_synchronous_bit(_spec: ControlPhySpec) -> bool {
    // C.S0024-0 §8.2.6.2.2: the 8-bit Control Channel Header begins with
    // SynchronousCapsule(1) in Rev 0 (Subtype 0). Rev A (Subtype 2) and Rev B
    // (Subtype 3+) keep the same leading bit.
    true
}

fn parse_control_mac_packets(
    bits: &[u8],
    has_synchronous_capsule_bit: bool,
) -> Option<Vec<ControlMacSecurityPacket>> {
    // C.S0024 9.2.6.2 uses a 7-bit Default Control Channel header.
    // C.S0024 9.3.7.2 adds SynchronousCapsule ahead of FirstPacket for the
    // enhanced Control Channel MAC header.
    if !matches!(bits.len(), 98 | 226 | 482 | 1002)
        || bits[bits.len() - 2..].iter().any(|&bit| bit != 0)
    {
        return None;
    }
    let packet_bits_end = bits.len() - 2;
    let mut cursor = 0usize;
    if has_synchronous_capsule_bit {
        let _synchronous_capsule = read_bits(bits, &mut cursor, 1)?;
    }
    let first_packet = read_bits(bits, &mut cursor, 1)?;
    let last_packet = read_bits(bits, &mut cursor, 1)?;
    let _offset = read_bits(bits, &mut cursor, 2)?;
    let _sleep_state_capsule_done = read_bits(bits, &mut cursor, 1)?;
    // C.S0024 MAC Control Channel header Reserved bits are set to zero by
    // the AN, but the AT is required to ignore them.
    let _control_reserved = read_bits(bits, &mut cursor, 2)?;
    if (last_packet != 0 && last_packet != 1) || (first_packet != 0 && first_packet != 1) {
        return None;
    }

    let mut messages = Vec::new();
    while cursor + 16 <= packet_bits_end {
        if bits[cursor..packet_bits_end].iter().all(|&bit| bit == 0) {
            break;
        }

        let length_octets = read_bits(bits, &mut cursor, 8)? as usize;
        if length_octets == 0 {
            return None;
        }
        let packet_start = cursor;
        let packet_end = packet_start + length_octets * 8;
        if packet_end > packet_bits_end {
            return None;
        }

        let security_layer_format = read_bits(bits, &mut cursor, 1)? != 0;
        let connection_layer_format = read_bits(bits, &mut cursor, 1)? != 0;
        let _mac_reserved = read_bits(bits, &mut cursor, 4)?;
        let ati_type = read_bits(bits, &mut cursor, 2)? as u8;
        let ati = if ati_type != 0 {
            Some(read_bits(bits, &mut cursor, 32)? as u32)
        } else {
            None
        };
        if cursor > packet_end {
            return None;
        }
        let security_payload_bits = bits[cursor..packet_end].to_vec();
        messages.push(ControlMacSecurityPacket {
            security_layer_format,
            connection_layer_format,
            ati_type,
            ati,
            security_payload_bits,
        });
        cursor = packet_end;
    }

    Some(messages)
}

fn parse_connection_layer_packets(bits: &[u8], format_b: bool) -> Option<Vec<Vec<u8>>> {
    if !format_b {
        return Some(Vec::from([bits.to_vec()]));
    }

    let mut cursor = 0usize;
    let mut packets = Vec::new();
    while cursor + 8 <= bits.len() {
        if bits[cursor..].iter().all(|&bit| bit == 0) {
            break;
        }
        let length_octets = read_bits(bits, &mut cursor, 8)? as usize;
        if length_octets == 0 {
            return None;
        }
        let packet_end = cursor + length_octets * 8;
        if packet_end > bits.len() {
            return None;
        }
        packets.push(bits[cursor..packet_end].to_vec());
        cursor = packet_end;
    }
    Some(packets)
}

fn parse_default_signaling_messages(session_packet: &[u8]) -> Vec<DecodedDefaultSignalingMessage> {
    let mut out = Vec::new();
    if let Some(message) = parse_default_signaling_message_at(session_packet, 0) {
        out.push(message);
    }
    out
}

fn parse_default_signaling_message_at(
    bits: &[u8],
    start: usize,
) -> Option<DecodedDefaultSignalingMessage> {
    let mut cursor = start;
    let stream = read_bits(bits, &mut cursor, 2)?;
    if stream != 0 {
        return None;
    }

    let _slp_f_reserved = read_bits(bits, &mut cursor, 4)?;
    let fragmented = read_bits(bits, &mut cursor, 1)?;
    if fragmented != 0 {
        return None;
    }

    let full_slp_d_header = read_bits(bits, &mut cursor, 1)?;
    if full_slp_d_header != 0 {
        return None;
    }

    let _in_configuration = read_bits(bits, &mut cursor, 1)?;
    let protocol_type = read_bits(bits, &mut cursor, 7)?;

    if cursor > bits.len() || (bits.len() - cursor) % 8 != 0 {
        return None;
    }
    let payload = bits_to_bytes(&bits[cursor..]);
    let protocol_type = protocol_type as u8;
    let (message_id, message_id_bits) = signaling_message_id(protocol_type, &payload);
    Some(DecodedDefaultSignalingMessage {
        ati_type: 0,
        ati: None,
        protocol_type,
        message_id,
        message_id_bits,
        payload,
    })
}

fn signaling_message_id(protocol_type: u8, payload: &[u8]) -> (Option<u8>, u8) {
    if protocol_type == DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE {
        return (payload.first().map(|byte| byte >> 6), 2);
    }
    (payload.first().copied(), 8)
}

fn parse_sync_message_bits_at(bits: &[u8], start: usize) -> Option<DecodedSyncMessage> {
    if start + 64 > bits.len() {
        return None;
    }
    let mut cursor = start;
    let message_id = read_bits(bits, &mut cursor, 2)?;
    if message_id != 0 {
        return None;
    }
    let maximum_revision = read_bits(bits, &mut cursor, 8)? as u8;
    let minimum_revision = read_bits(bits, &mut cursor, 8)? as u8;
    let pilot_pn = read_bits(bits, &mut cursor, 9)? as u16;
    let system_time = read_bits(bits, &mut cursor, 37)?;
    if minimum_revision > maximum_revision || pilot_pn > 511 {
        return None;
    }
    Some(DecodedSyncMessage {
        maximum_revision,
        minimum_revision,
        pilot_pn,
        system_time,
    })
}

fn physical_fcs_ok(bits: &[u8]) -> bool {
    control_physical_fcs_ok(bits, 1024)
}

fn control_physical_fcs_ok(bits: &[u8], payload_bits: usize) -> bool {
    if payload_bits == 1024 {
        return control_mac_bits(bits, payload_bits).is_some()
            && physical_crc16(&bits[..1002]) == bits_to_u16(&bits[1002..1018]);
    }
    let Some(mac_bits) = control_mac_bits(bits, payload_bits) else {
        return false;
    };
    let fcs_start = mac_bits.len();
    let fcs_end = fcs_start + 24;
    physical_crc24(mac_bits) == bits_to_u24(&bits[fcs_start..fcs_end])
}

fn control_mac_bits(bits: &[u8], payload_bits: usize) -> Option<&[u8]> {
    if bits.len() < payload_bits {
        return None;
    }
    match payload_bits {
        1024 => Some(&bits[..1002]),
        128 | 256 | 512 => Some(&bits[..payload_bits - 30]),
        _ => None,
    }
}

fn read_bits(bits: &[u8], cursor: &mut usize, width: usize) -> Option<u64> {
    if *cursor + width > bits.len() {
        return None;
    }
    let mut out = 0u64;
    for _ in 0..width {
        out = (out << 1) | u64::from(bits[*cursor] & 1);
        *cursor += 1;
    }
    Some(out)
}

fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            let mut b = 0u8;
            for &bit in chunk {
                b = (b << 1) | (bit & 1);
            }
            b << (8 - chunk.len())
        })
        .collect()
}

fn bytes_to_bits(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits
}

fn bits_to_u16(bits: &[u8]) -> u16 {
    bits.iter()
        .fold(0u16, |acc, &bit| (acc << 1) | u16::from(bit & 1))
}

fn bits_to_u24(bits: &[u8]) -> u32 {
    bits.iter()
        .fold(0u32, |acc, &bit| (acc << 1) | u32::from(bit & 1))
}

fn physical_crc24(bits: &[u8]) -> u32 {
    let poly = 0x80_0063u32;
    let mut reg = 0u32;
    for &bit in bits {
        let feedback = ((reg >> 23) & 1) ^ u32::from(bit & 1);
        reg = (reg << 1) & 0xFF_FFFF;
        if feedback != 0 {
            reg ^= poly;
        }
    }
    reg & 0xFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bts::{
        config::BtsNodeConfig,
        evdo::{
            AdjacentCarrierComposer, HrpdForwardSlotModulator, ResolvedEvdoConfig,
            resolve_evdo_config,
        },
    };
    use crate::phy::spread::{PnSequence, Spreader};
    use crate::receiver::pipelined::PipelineProcessor;
    use crate::sdr::TxPulseShaper;
    use cdma_common::hrpd::{
        air::{
            AccessTerminalIdentifier, AccessTerminalIdentifierType,
            DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            HrpdForwardChannel, HrpdForwardSignalingRequest, HrpdTrafficChannelAssignment,
            HrpdUatiAssignment, HrpdUatiSubnetAssignment,
        },
        messages::{AccessParameters, ChannelRecord, HrpdOverheadMessage, SyncMessage},
    };
    use hound::WavReader;
    use std::path::PathBuf;

    const ADJACENT_CHANNEL_SELECT_TAPS: usize = 257;
    const ADJACENT_CHANNEL_SELECT_CUTOFF_HZ: f64 = 780_000.0;

    /// Loopback the Forward Traffic Channel TX path: build an RTCAck packet
    /// through the production scheduler at DRC 0x3 / MAC 5, then decode the
    /// emitted slot chips with this receiver's independent decode chain
    /// (Walsh-16 decover, QPSK LLR, repeat combining, channel deinterleave,
    /// per-MAC forward descramble, turbo decode). Catches any encode-order
    /// or per-MAC parameter bug the control channel cannot exercise.
    #[test]
    fn forward_traffic_rtc_ack_loopback_decodes() {
        use crate::bts::hrpd::scheduler::{ForwardTrafficPacket, HrpdForwardScheduler, SlotKind};
        use crate::phy::hrpd::scrambler::HrpdForwardScrambler;
        use cdma_common::hrpd::traffic::default_reverse_traffic_mac_rtc_ack_ftc_payload_bits;

        const MAC_INDEX: u8 = 5;
        const DRC_INDEX: u8 = 0x3;
        const PAYLOAD_BITS: usize = 1024;
        const SLOTS: usize = 4;
        const PREAMBLE_CHIPS: usize = 256;
        const DATA_CHIPS_PER_SLOT: usize = 1600;

        let payload = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(PAYLOAD_BITS, 0)
            .expect("rtc ack payload");
        let mut scheduler = HrpdForwardScheduler::new();
        let bus = std::sync::Arc::new(crate::bts::hrpd::HarqBus::new());
        bus.set_current_drc_at_slot(MAC_INDEX, 0, DRC_INDEX);
        scheduler.set_harq_bus(bus);
        scheduler.enqueue(ForwardTrafficPacket {
            mac_index: MAC_INDEX,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: payload.clone(),
        });

        // Drive slots; the packet's four slots come out 4-slot interlaced.
        let mut traffic_slots: Vec<(u64, Vec<Complex32>)> = Vec::new();
        for slot in 0..64u64 {
            let out = scheduler.next_slot(slot, false);
            if matches!(out.channel, SlotKind::Traffic { .. }) {
                traffic_slots.push((slot, out.data_chips));
            }
        }
        assert_eq!(traffic_slots.len(), SLOTS, "expected 4 transmit slots");
        let first_slot = traffic_slots[0].0;
        for (idx, (slot, chips)) in traffic_slots.iter().enumerate() {
            assert_eq!(
                *slot,
                first_slot + 4 * idx as u64,
                "packet slots must be 4-slot interlaced"
            );
            assert_eq!(chips.len(), DATA_CHIPS_PER_SLOT);
        }
        let packet_chips: Vec<Complex32> = traffic_slots
            .into_iter()
            .flat_map(|(_, chips)| chips)
            .collect();

        // Preamble: all-zero symbols covered by the 32-chip bi-orthogonal
        // sequence for MACIndex 5 (W_2^32 complemented), on I only.
        for (idx, chip) in packet_chips[..PREAMBLE_CHIPS].iter().enumerate() {
            let row = usize::from(MAC_INDEX >> 1);
            let mut expected = if ((row & (idx % 32)).count_ones() & 1) == 0 {
                1.0f32
            } else {
                -1.0
            };
            if MAC_INDEX & 1 != 0 {
                expected = -expected;
            }
            assert!(
                (chip.re - expected).abs() < 1e-3 && chip.im.abs() < 1e-3,
                "preamble chip {idx} mismatch: got {chip:?} expected {expected}"
            );
        }

        // Data region: decover, demap, combine sequence repetitions.
        let data = &packet_chips[PREAMBLE_CHIPS..];
        let symbols = walsh16_decover(data);
        let repeated_llrs = qpsk_llrs(&symbols, SymbolVariant::IDENTITY);
        let provided_symbols = PAYLOAD_BITS * 5 / 2;
        let mut llrs = vec![0.0f32; provided_symbols * 2];
        for (idx, pair) in repeated_llrs.chunks_exact(2).enumerate() {
            let dst = idx % provided_symbols;
            llrs[dst * 2] += pair[0];
            llrs[dst * 2 + 1] += pair[1];
        }

        let mut deinterleaved = forward_rate_1_5_deinterleave(PAYLOAD_BITS, &llrs);
        let mut scrambler = HrpdForwardScrambler::new_forward(MAC_INDEX, DRC_INDEX);
        for llr in &mut deinterleaved {
            if scrambler.next_bit() {
                *llr = -*llr;
            }
        }
        normalize_soft_llrs(&mut deinterleaved, 4.0);
        let decoder = HrpdTurboDecoder::new(PAYLOAD_BITS as u32)
            .expect("decoder")
            .with_iterations(16);
        let decoded = decoder.decode(&deinterleaved);
        assert_eq!(
            decoded[..PAYLOAD_BITS],
            payload[..],
            "forward traffic loopback did not recover the RTCAck payload"
        );
    }

    /// Same loopback through the full slot modulator with the default
    /// overhead/control schedule active, decoding from the spread chip
    /// stream. Exercises the TDM slot placement, PN spreading, and the
    /// interaction between traffic interlace slots and Control slots.
    #[test]
    fn forward_traffic_rtc_ack_modulator_loopback_decodes() {
        forward_traffic_modulator_loopback_for_profile(0x3, 256, 4, 5, 0, 0);
    }

    /// 8-slot DRC 0x2 variant: live ATs ACK our DRC 0x3 packets but NAK
    /// every DRC 0x2 packet, so the multi-interlace formats need their own
    /// loopback coverage.
    #[test]
    fn forward_traffic_rtc_ack_modulator_loopback_decodes_drc2() {
        forward_traffic_modulator_loopback_for_profile(0x2, 512, 8, 5, 0, 0);
    }

    /// Live negotiated profile: Physical Layer subtype 2, Enhanced Forward
    /// Traffic MAC subtype 1, MACIndex 6, DRC 0x2. This is the exact RTCAck
    /// profile used after SessionConfigurationComplete on the BladeRF trace.
    #[test]
    fn forward_traffic_rtc_ack_modulator_loopback_decodes_subtype2_enhanced_drc2() {
        forward_traffic_modulator_loopback_for_profile(0x2, 512, 8, 6, 2, 1);
    }

    /// Live UHD post-configuration setup profile: same negotiated Rev A
    /// traffic personality as above, but with the AT governing DRC at 0x3.
    #[test]
    fn forward_traffic_rtc_ack_modulator_loopback_decodes_subtype2_enhanced_drc3() {
        forward_traffic_modulator_loopback_for_profile(0x3, 256, 4, 6, 2, 1);
    }

    /// 16-slot DRC 0x1 variant.
    #[test]
    fn forward_traffic_rtc_ack_modulator_loopback_decodes_drc1() {
        forward_traffic_modulator_loopback_for_profile(0x1, 1024, 16, 5, 0, 0);
    }

    /// 2-slot DRC 0x4 variant. The decode chain here is fixed at turbo rate
    /// 1/5; this only passes if the encoder is also 1/5 for DRC 0x4
    /// (C.S0024-0 v4.0 Table 9.3.1.3.2.3.2-1), guarding against the 1/3
    /// regression that made every 307.2 kbps forward packet undecodable.
    #[test]
    fn forward_traffic_rtc_ack_modulator_loopback_decodes_drc4() {
        forward_traffic_modulator_loopback_for_profile(0x4, 128, 2, 5, 0, 0);
    }

    fn forward_traffic_modulator_loopback_for_profile(
        drc_index: u8,
        preamble_chips: usize,
        packet_slots: usize,
        mac_index: u8,
        physical_layer_subtype: u16,
        forward_traffic_mac_subtype: u16,
    ) {
        use crate::bts::hrpd::scheduler::ForwardTrafficPacket;
        use crate::phy::hrpd::scrambler::HrpdForwardScrambler;
        use crate::phy::hrpd::slot::{SLOT_CHIPS, SlotChannel, channel_for_chip};
        use crate::phy::spread::HrpdForwardPnSequence;
        use cdma_common::hrpd::traffic::default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype;

        let drc_index_const = drc_index;
        const PAYLOAD_BITS: usize = 1024;
        let preamble_chips_const = preamble_chips;
        const DATA_CHIPS_PER_SLOT: usize = 1600;
        const TOTAL_SLOTS: u64 = 160;

        let payload = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
            PAYLOAD_BITS,
            0,
            forward_traffic_mac_subtype,
        )
        .expect("rtc ack payload");
        let mut modulator = HrpdForwardSlotModulator::new(0, 32_768);
        let bus = std::sync::Arc::new(crate::bts::hrpd::HarqBus::new());
        bus.set_current_drc_at_slot(mac_index, 0, drc_index_const);
        modulator.set_harq_bus(bus);
        modulator.enqueue_traffic(ForwardTrafficPacket {
            mac_index,
            physical_layer_subtype,
            forward_traffic_mac_subtype,
            high_priority: false,
            payload: payload.clone(),
        });

        // Generate the spread chip stream and despread with the forward PN.
        let spread = modulator.next_block(0, (TOTAL_SLOTS * SLOT_CHIPS) as usize);
        let mut pn = HrpdForwardPnSequence::new(0, 32_768);
        let mut despread = Vec::with_capacity(spread.len());
        for chip in &spread {
            let reference = pn.generate_iq();
            // The spreader emits conj(s * pn) (conjugated complex envelope),
            // so s = conj(out) * conj(pn) / |pn|^2 with |pn|^2 = 2.
            despread.push(chip.conj() * reference.conj() * 0.5);
        }

        // Collect the Data-region chips of every slot.
        let mut slot_data: Vec<Vec<Complex32>> = vec![Vec::new(); TOTAL_SLOTS as usize];
        for (idx, chip) in despread.iter().enumerate() {
            let chip_index = idx as u64;
            if matches!(channel_for_chip(chip_index), SlotChannel::Data) {
                slot_data[(chip_index / SLOT_CHIPS) as usize].push(*chip);
            }
        }

        // Find the slots whose data region starts with the assigned MAC preamble.
        let preamble_metric = |chips: &[Complex32]| -> f32 {
            let cover_len = if physical_layer_subtype == 2 { 64 } else { 32 };
            let row = usize::from(mac_index >> 1);
            let mut acc = 0.0f32;
            for (idx, chip) in chips.iter().take(preamble_chips_const).enumerate() {
                let mut cover = if ((row & (idx % cover_len)).count_ones() & 1) == 0 {
                    1.0f32
                } else {
                    -1.0
                };
                if mac_index & 1 != 0 {
                    cover = -cover;
                }
                acc += chip.re * cover;
            }
            acc / preamble_chips_const as f32
        };
        let first_slot = slot_data
            .iter()
            .position(|chips| chips.len() == DATA_CHIPS_PER_SLOT && preamble_metric(chips) > 0.9)
            .expect("no slot carries the MAC-5 traffic preamble");

        // The packet's remaining slots must follow at exactly 4-slot
        // interlace and carry data (not be displaced by Control slots).
        let mut packet_chips = Vec::with_capacity(packet_slots * DATA_CHIPS_PER_SLOT);
        for k in 0..packet_slots {
            let slot = first_slot + 4 * k;
            let chips = &slot_data[slot];
            assert_eq!(
                chips.len(),
                DATA_CHIPS_PER_SLOT,
                "packet slot {k} (absolute {slot}) does not carry a full traffic data region"
            );
            packet_chips.extend_from_slice(chips);
        }

        let data = &packet_chips[preamble_chips_const..];
        let symbols = walsh16_decover(data);
        let repeated_llrs = qpsk_llrs(&symbols, SymbolVariant::IDENTITY);
        let provided_symbols = PAYLOAD_BITS * 5 / 2;
        let mut llrs = vec![0.0f32; provided_symbols * 2];
        for (idx, pair) in repeated_llrs.chunks_exact(2).enumerate() {
            let dst = idx % provided_symbols;
            llrs[dst * 2] += pair[0];
            llrs[dst * 2 + 1] += pair[1];
        }
        let mut deinterleaved = forward_rate_1_5_deinterleave(PAYLOAD_BITS, &llrs);
        // For MACIndex < 64 canonical formats the subtype-2 seed (r̄6
        // complemented) is bit-identical to the Rev 0 seed, so this match
        // mirrors the transmitter's forward_traffic_scrambler selection.
        let mut scrambler = match physical_layer_subtype {
            2 => HrpdForwardScrambler::new_forward_subtype2(mac_index, 0b111, drc_index_const),
            3.. => {
                HrpdForwardScrambler::new_forward_subtype3_plus(mac_index, 0b111, drc_index_const)
            }
            _ => HrpdForwardScrambler::new_forward(mac_index, drc_index_const),
        };
        for llr in &mut deinterleaved {
            if scrambler.next_bit() {
                *llr = -*llr;
            }
        }
        normalize_soft_llrs(&mut deinterleaved, 4.0);
        let decoder = HrpdTurboDecoder::new(PAYLOAD_BITS as u32)
            .expect("decoder")
            .with_iterations(16);
        let decoded = decoder.decode(&deinterleaved);
        assert_eq!(
            decoded[..PAYLOAD_BITS],
            payload[..],
            "modulator-level forward traffic loopback did not recover the RTCAck payload"
        );
    }

    fn synthetic_hrpd_pilot_capture(
        samples_per_chip: usize,
        n_slots: usize,
        zero_phase_chips: usize,
        pilot_pn: u16,
        noise_sigma: f32,
    ) -> Vec<Complex32> {
        let n_chips = n_slots * SLOT_CHIPS as usize;
        let half = HALF_SLOT_CHIPS as usize;
        let mut spreader = crate::phy::spread::Spreader::new(HrpdForwardPnSequence::new(
            pilot_pn as usize,
            32_768,
        ));
        spreader.align_to_chip(zero_phase_chips as u64);

        let mut chips = Vec::with_capacity(n_chips);
        for c in 0..n_chips {
            let half_chip = (zero_phase_chips + c) % half;
            let unspread =
                if (PILOT_START_IN_HALF_SLOT..PILOT_END_IN_HALF_SLOT).contains(&half_chip) {
                    Complex32::new(1.0, 0.0)
                } else {
                    Complex32::new(0.0, 0.0)
                };
            chips.push(spreader.spread(&unspread));
        }

        let mut samples = Vec::with_capacity(n_chips * samples_per_chip);
        for chip in &chips {
            for _ in 0..samples_per_chip {
                samples.push(*chip);
            }
        }

        let taps = cdma2000_baseband_filter_taps_f64();
        let mut filtered = ComplexFir32::new(&taps).process_block(&samples);

        if noise_sigma > 0.0 {
            let mut rng = 0x1234_5678u32;
            let mut uniform = || -> f32 {
                rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((rng >> 8) & 0xFFFFFF) as f32 / 16_777_216.0
            };
            let mut next_gauss = || -> f32 {
                let u1 = (uniform() + 1e-9).min(1.0 - 1e-9);
                let u2 = uniform();
                (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
            };
            for s in filtered.iter_mut() {
                s.re += next_gauss() * noise_sigma;
                s.im += next_gauss() * noise_sigma;
            }
        }

        filtered
    }

    fn pilot_sign_agreement(
        samples: &[Complex32],
        samples_per_chip: usize,
        acq: &PilotAcquisition,
        max_halfslots: usize,
    ) -> f32 {
        let half = HALF_SLOT_CHIPS as usize;
        let chip_stream = make_chip_stream(
            samples,
            samples_per_chip,
            acq.timing_phase,
            acq.chip_variant,
        );
        let despread = despread_chips(&chip_stream, acq.pn_phase_chips, acq.despread_mode);
        let mut total_agree = 0usize;
        let mut total_chips = 0usize;
        let mut halfslot_start = acq.slot_phase_chips;
        for _ in 0..max_halfslots {
            if halfslot_start + PILOT_END_IN_HALF_SLOT > despread.len() {
                break;
            }
            let burst = &despread[halfslot_start + PILOT_START_IN_HALF_SLOT
                ..halfslot_start + PILOT_END_IN_HALF_SLOT];
            let sum: Complex32 = burst.iter().copied().sum();
            let dir = if sum.norm_sqr() > 0.0 {
                sum / sum.norm_sqr().sqrt()
            } else {
                Complex32::new(1.0, 0.0)
            };
            total_agree += burst
                .iter()
                .filter(|s| s.re * dir.re + s.im * dir.im > 0.0)
                .count();
            total_chips += burst.len();
            halfslot_start += half;
        }
        100.0 * total_agree as f32 / total_chips.max(1) as f32
    }

    #[test]
    fn decodes_sync_from_generated_low_rate_control_packet() {
        let rate = LowRateControl::ALL[0];
        let pilot_pn = 333;
        let packet = generated_low_rate_control_packet(rate, pilot_pn);
        let (messages, sync) = decode_low_rate_control_packet(&packet, 0, rate)
            .expect("generated spec-shaped low-rate Control packet should decode through FCS");

        assert_eq!(messages.len(), 1);
        assert_eq!(sync.pilot_pn, pilot_pn);
        assert_eq!(sync.maximum_revision, 1);
        assert_eq!(sync.minimum_revision, 1);
        assert_eq!(sync.system_time, 0x12345);
    }

    #[test]
    fn decodes_sync_from_generated_mac71_quick_control_packet() {
        let rate = LowRateControl::ALL
            .iter()
            .copied()
            .find(|rate| {
                rate.mac_index == 71 && rate.payload_bits == 256 && rate.preamble_chips == 256
            })
            .expect("Subtype 5 MACIndex 71 quick-sync format should be in the Control table");
        let pilot_pn = 333;
        let packet = generated_low_rate_control_packet(rate, pilot_pn);
        let (messages, sync) = decode_low_rate_control_packet(&packet, 0, rate)
            .expect("generated MACIndex 71 quick-sync Control packet should decode");

        assert_eq!(messages.len(), 1);
        assert_eq!(sync.pilot_pn, pilot_pn);
        assert_eq!(sync.maximum_revision, 1);
        assert_eq!(sync.minimum_revision, 1);
        assert_eq!(sync.system_time, 0x12345);
    }

    #[test]
    fn decodes_sync_from_generated_subtype2_mac71_short_control_packet() {
        let rate = LowRateControl::ALL
            .iter()
            .copied()
            .find(|rate| {
                matches!(rate.spec, ControlPhySpec::Subtype2)
                    && rate.mac_index == 71
                    && rate.payload_bits == 256
                    && rate.preamble_chips == 1024
            })
            .expect("Subtype 2 MACIndex 71 short Control format should be in the table");
        let pilot_pn = 333;
        let packet = generated_low_rate_control_packet(rate, pilot_pn);
        let (messages, sync) = decode_low_rate_control_packet(&packet, 0, rate)
            .expect("generated Subtype 2 MACIndex 71 short Control packet should decode");

        assert_eq!(messages.len(), 1);
        assert_eq!(sync.pilot_pn, pilot_pn);
        assert_eq!(sync.maximum_revision, 1);
        assert_eq!(sync.minimum_revision, 1);
        assert_eq!(sync.system_time, 0x12345);
    }

    #[test]
    fn low_rate_control_frontend_round_trips_coded_symbols() {
        let rate = LowRateControl::ALL[0];
        let payload = generated_sync_physical_payload(333);
        let encoder = crate::phy::hrpd::turbo::HrpdTurboEncoder::new(1024).unwrap();
        let coded = encoder.encode(&payload, 1, 5);
        let mut scrambled = coded.clone();
        rate.scrambler().apply_bits(&mut scrambled);
        let interleaved = forward_rate_1_5_interleave_1024(&scrambled);
        let symbols = repeat_symbols(&map_qpsk_bits(&interleaved), rate.data_chips);
        let data_chips = walsh16_cover_symbols(&symbols);
        let repeated_symbols = walsh16_decover(&data_chips);
        let repeated_llrs = qpsk_llrs(&repeated_symbols, SymbolVariant::ALL[0]);
        let mut llrs = vec![0.0f32; 2_560 * 2];
        for (idx, pair) in repeated_llrs.chunks_exact(2).enumerate() {
            let dst = idx % 2_560;
            llrs[dst * 2] += pair[0];
            llrs[dst * 2 + 1] += pair[1];
        }
        let mut descrambled = forward_rate_1_5_deinterleave_1024(&llrs);
        apply_scrambler_soft(&mut descrambled, rate);
        let recovered: Vec<u8> = descrambled
            .iter()
            .map(|&llr| if llr >= 0.0 { 0 } else { 1 })
            .collect();
        let mismatches = recovered
            .iter()
            .zip(coded.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(mismatches, 0);
    }

    #[test]
    fn spawns_forward_rake_finger_and_despreads_generated_pilot() {
        let mut modulator = HrpdForwardSlotModulator::new(7, 32_768);
        modulator.set_overhead_quick_config(None);
        modulator.set_overhead_sector_params(None);
        modulator.set_overhead_access_params(None);
        modulator.set_overhead_reverse_rate(None);
        let mut sync = SyncMessage::defaults();
        sync.pilot_pn = 7;
        modulator.set_overhead_sync(Some(sync));

        let chips = modulator.next_block(0, FORWARD_CORRELATOR_MIN_CHIPS + SLOT_CHIPS as usize);
        let mut samples = Vec::with_capacity(chips.len() * 2);
        for chip in chips {
            samples.push(chip);
            samples.push(chip);
        }

        let mut rake = hrpd_forward_rake_receiver(HRPD_CHIP_RATE_HZ * 2);
        let block =
            SampleBlock::new(samples, 0).with_sample_rate_hz((HRPD_CHIP_RATE_HZ * 2) as f64);
        let out = rake.process_block(block);
        let despread = out
            .iter()
            .find(|b| b.tags.get("hrpd_forward_despread").copied() == Some(1))
            .expect("generated HRPD forward RAKE should spawn and emit despread chips");
        assert!(!despread.samples.is_empty());
        assert_eq!(
            despread.tags.get("hrpd_pilot_samples_per_chip").copied(),
            Some(2)
        );
        let halfslot_phase = despread
            .tags
            .get("hrpd_pilot_halfslot_phase")
            .copied()
            .expect("despread block should carry half-slot phase")
            as usize;
        let summary = pilot_halfslot_alignment_summary(&despread.samples, halfslot_phase);
        eprintln!(
            "generated forward rake despread: tags={:?} samples={} coherent_best=[{}, {}) coherent_pilot_to_outside_db={:.2}",
            despread.tags,
            despread.samples.len(),
            summary.coherent_best_start,
            summary.coherent_best_start + (PILOT_END_IN_HALF_SLOT - PILOT_START_IN_HALF_SLOT),
            summary.coherent_pilot_to_outside_db,
        );
        assert_eq!(summary.coherent_best_start, PILOT_START_IN_HALF_SLOT);
        assert!(summary.coherent_pilot_to_outside_db > 20.0);

        assert!(
            out.iter()
                .all(|b| b.tags.get("hrpd_sync_decoded").copied() != Some(1)),
            "legacy direct-QPSK generated control must not decode through the spec-bound Control path"
        );
    }

    #[test]
    fn forward_rake_decodes_generated_low_rate_control_packet() {
        let rate = LowRateControl::ALL[0];
        let pilot_pn = 333u16;
        let samples_per_chip = 4usize;
        let samples = generated_forward_control_capture(rate, pilot_pn, samples_per_chip);
        let mut rake = hrpd_forward_rake_receiver(HRPD_CHIP_RATE_HZ * samples_per_chip as u32);
        let block = SampleBlock::new(samples, 0)
            .with_sample_rate_hz((HRPD_CHIP_RATE_HZ * samples_per_chip as u32) as f64);

        let out = rake.process_block(block);
        let despread = out
            .iter()
            .find(|b| b.tags.get("hrpd_forward_despread").copied() == Some(1))
            .expect("generated forward Control capture should emit despread chips");
        assert!(!despread.samples.is_empty());

        let sync = out
            .iter()
            .find(|b| b.tags.get("hrpd_sync_decoded").copied() == Some(1))
            .expect("generated forward Control capture should decode Sync through RAKE chain");
        eprintln!(
            "generated forward rake Sync event: chip_start={} tags={:?}",
            sync.chip_start, sync.tags
        );
        assert_eq!(
            sync.tags.get("hrpd_sync_pilot_pn").copied(),
            Some(i64::from(pilot_pn))
        );
        assert_eq!(
            sync.tags.get("hrpd_sync_system_time").copied(),
            Some(0x12345)
        );
        assert_eq!(
            sync.tags.get("hrpd_control_payload_bits").copied(),
            Some(i64::from(rate.payload_bits))
        );
        assert!(rake.has_hard_validated_finger());
    }

    /// The production `ControlChannelModulator` parametrized for 38.4 kbps
    /// (16 slots, 1024-chip preamble, MACIndex 3 = complemented row-1 cover,
    /// rate code 0b0001) must produce a packet this receiver decodes. The
    /// 76.8 kbps path is AT-proven on air; this pins the 38.4 parameters
    /// to the same receiver conventions before a live `HRPD_CTRL_KBPS=38400`
    /// run.
    #[test]
    fn forward_decodes_production_control_modulator_38_4_packet() {
        use crate::bts::hrpd::control_channel::ControlChannelCapsule;
        use crate::bts::hrpd::control_modulator::ControlChannelModulator;
        use crate::bts::hrpd::scheduler::DATA_CHIPS_PER_SLOT;

        let rate = LowRateControl::ALL[0];
        assert_eq!(rate.mac_index, 3);
        assert_eq!(rate.slots, 16);

        let mut sync = cdma_common::hrpd::messages::SyncMessage::defaults();
        sync.pilot_pn = 333;
        sync.system_time = 0x12345;
        let capsule = ControlChannelCapsule::new(vec![sync.encode()], 38_400);
        let mut modulator = ControlChannelModulator::new();
        assert!(modulator.load_capsule(&capsule), "38.4 capsule should load");
        let mut tdm = Vec::new();
        while modulator.remaining() > 0 {
            tdm.extend(modulator.next_slot_chips());
        }
        assert_eq!(tdm.len(), rate.slots * DATA_CHIPS_PER_SLOT);

        let mut equalized = vec![Complex32::new(0.0, 0.0); rate.packet_span_chips()];
        scatter_packet_data_region_with_stride(
            &mut equalized,
            0,
            rate.slots,
            rate.slot_stride(),
            &tdm,
        );
        let (_, sync_out) = decode_low_rate_control_packet(&equalized, 0, rate)
            .expect("production 38.4 control packet should decode");
        assert_eq!(sync_out.pilot_pn, 333);
        assert_eq!(sync_out.system_time, 0x12345);
    }

    fn generated_low_rate_control_packet(rate: LowRateControl, pilot_pn: u16) -> Vec<Complex32> {
        let payload = generated_spec_sync_physical_payload_for(rate, pilot_pn);
        let encoder = crate::phy::hrpd::turbo::HrpdTurboEncoder::new(rate.payload_bits).unwrap();
        let coded = encoder.encode(&payload, 1, 5);
        let mut scrambled = coded;
        rate.scrambler().apply_bits(&mut scrambled);
        let interleaved = rate_1_5_channel_interleave(rate, &scrambled);
        let symbols = repeat_symbols(&map_qpsk_bits(&interleaved), rate.data_chips);
        let data_chips = walsh16_cover_symbols(&symbols);

        let packet_len = rate.packet_span_chips();
        let mut equalized = vec![Complex32::new(0.0, 0.0); packet_len];
        let row = usize::from(rate.mac_index >> 1);
        let complement = (rate.mac_index & 1) != 0;
        let mut tdm = (0..rate.preamble_chips)
            .map(|idx| {
                let mut sign = walsh_biorthogonal(row, idx % rate.preamble_cover_chips);
                if complement {
                    sign = -sign;
                }
                Complex32::new(sign, 0.0)
            })
            .collect::<Vec<_>>();
        tdm.extend_from_slice(&data_chips);
        scatter_packet_data_region_with_stride(
            &mut equalized,
            0,
            rate.slots,
            rate.slot_stride(),
            &tdm,
        );
        equalized
    }

    fn generated_forward_control_capture(
        rate: LowRateControl,
        pilot_pn: u16,
        samples_per_chip: usize,
    ) -> Vec<Complex32> {
        let packet = generated_low_rate_control_packet(rate, pilot_pn);
        let n_chips = FORWARD_CORRELATOR_MIN_CHIPS.max(packet.len() + SLOT_CHIPS as usize);
        let mut spec_chips = vec![Complex32::new(0.0, 0.0); n_chips];
        spec_chips[..packet.len()].copy_from_slice(&packet);

        let half = HALF_SLOT_CHIPS as usize;
        for half_start in (0..n_chips).step_by(half) {
            for offset in PILOT_START_IN_HALF_SLOT..PILOT_END_IN_HALF_SLOT {
                let chip = half_start + offset;
                if chip < spec_chips.len() {
                    spec_chips[chip] = Complex32::new(1.0, 0.0);
                }
            }
        }

        let mut spreader = crate::phy::spread::Spreader::new(HrpdForwardPnSequence::new(
            pilot_pn as usize,
            32_768,
        ));
        let chips = spec_chips
            .iter()
            .map(|chip| spreader.spread(chip))
            .collect::<Vec<_>>();

        let mut samples = Vec::with_capacity(chips.len() * samples_per_chip);
        for chip in &chips {
            for _ in 0..samples_per_chip {
                samples.push(*chip);
            }
        }

        let taps = cdma2000_baseband_filter_taps_f64();
        ComplexFir32::new(&taps).process_block(&samples)
    }

    #[derive(Debug)]
    struct RakeCapsuleSummary {
        slot_start_chip: usize,
        payload_bits: u32,
        messages: Vec<HrpdOverheadMessage>,
    }

    struct ChannelizedSamples {
        samples: Vec<Complex32>,
        group_delay_chips: usize,
    }

    fn bts_config_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/bts.json")
    }

    fn configured_bts_hrpd() -> (BtsNodeConfig, ResolvedEvdoConfig) {
        let bts_path = bts_config_path();
        // EV-DO ships disabled by default; these tests exercise the composite
        // decode path, so load it with EV-DO enabled and the RF re-derived.
        let config = BtsNodeConfig::load_evdo_enabled_for_test(&bts_path)
            .expect("checked-in BTS config should load and validate");
        let resolved = resolve_evdo_config(
            &config.evdo,
            config.pilot_offset,
            config.channel,
            config.runtime.tx_sample_rate_hz,
            config.runtime.tx_bandwidth_hz,
        )
        .expect("checked-in EVDO config should resolve")
        .expect("EVDO should be enabled in checked-in BTS config");
        (config, resolved)
    }

    fn bts_hrpd_modulator(
        config: &BtsNodeConfig,
        resolved: &ResolvedEvdoConfig,
    ) -> HrpdForwardSlotModulator {
        let mut modulator = HrpdForwardSlotModulator::new(
            config.pilot_offset,
            config.runtime.short_code_length_chips,
        );
        modulator.install_sector_overheads(
            resolved.pilot_pn,
            resolved.transmits_one_x().then_some((
                resolved.one_x_band_class,
                resolved.one_x_channel,
                resolved.pilot_pn,
            )),
            resolved.evdo_band_class,
            resolved.evdo_channel,
            resolved.overhead,
        );
        modulator
    }

    fn bts_hrpd_chips(
        config: &BtsNodeConfig,
        resolved: &ResolvedEvdoConfig,
        start_chip: u64,
        chips: usize,
    ) -> Vec<Complex32> {
        let mut modulator = bts_hrpd_modulator(config, resolved);
        modulator.next_block(start_chip, chips)
    }

    fn shape_bts_hrpd_only(
        config: &BtsNodeConfig,
        resolved: &ResolvedEvdoConfig,
        start_chip: u64,
        chips: usize,
    ) -> Vec<Complex32> {
        let chips = bts_hrpd_chips(config, resolved, start_chip, chips);
        let mut shaper = TxPulseShaper::new(config.runtime.tx_sample_rate_hz)
            .expect("checked-in TX sample rate should be chip aligned");
        shaper.shape(&chips)
    }

    fn one_x_pilot_chips(config: &BtsNodeConfig, start_chip: u64, chips: usize) -> Vec<Complex32> {
        let mut spreader = Spreader::new(PnSequence::new(
            config.pilot_offset,
            config.runtime.short_code_length_chips,
        ));
        spreader.align_to_chip(start_chip);
        (0..chips)
            .map(|_| spreader.spread(&Complex32::new(1.0, 0.0)))
            .collect()
    }

    fn bts_adjacent_composite_evdo_baseband(
        config: &BtsNodeConfig,
        resolved: &ResolvedEvdoConfig,
        start_chip: u64,
        chips: usize,
    ) -> ChannelizedSamples {
        let one_x = one_x_pilot_chips(config, start_chip, chips);
        let evdo = bts_hrpd_chips(config, resolved, start_chip, chips);
        let mut composer = AdjacentCarrierComposer::new(
            resolved,
            config.runtime.tx_sample_rate_hz,
            config.runtime.tx_digital_backoff,
        )
        .expect("checked-in TX sample rate should be chip aligned");
        let composite = composer.compose(&one_x, &evdo);
        let evdo_baseband = frequency_shift(
            &composite,
            -resolved.evdo_shift_hz,
            config.runtime.tx_sample_rate_hz,
        );
        channel_select_lowpass(&evdo_baseband, config.runtime.tx_sample_rate_hz)
    }

    fn frequency_shift(
        samples: &[Complex32],
        shift_hz: i64,
        sample_rate_hz: usize,
    ) -> Vec<Complex32> {
        let step = 2.0 * std::f64::consts::PI * shift_hz as f64 / sample_rate_hz as f64;
        let mut phase = 0.0f64;
        samples
            .iter()
            .map(|sample| {
                let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
                phase = (phase + step).rem_euclid(2.0 * std::f64::consts::PI);
                *sample * rot
            })
            .collect()
    }

    fn channel_select_lowpass(samples: &[Complex32], sample_rate_hz: usize) -> ChannelizedSamples {
        let m = (ADJACENT_CHANNEL_SELECT_TAPS - 1) as f64 / 2.0;
        let fc = ADJACENT_CHANNEL_SELECT_CUTOFF_HZ / sample_rate_hz as f64;
        let mut taps = (0..ADJACENT_CHANNEL_SELECT_TAPS)
            .map(|n| {
                let x = n as f64 - m;
                let sinc = if x.abs() < f64::EPSILON {
                    2.0 * fc
                } else {
                    (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
                };
                let window = 0.54
                    - 0.46
                        * (2.0 * std::f64::consts::PI * n as f64
                            / (ADJACENT_CHANNEL_SELECT_TAPS - 1) as f64)
                            .cos();
                sinc * window
            })
            .collect::<Vec<_>>();
        let sum: f64 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }

        let filtered = ComplexFir32::new(&taps).process_block(samples);

        let group_delay_samples = (ADJACENT_CHANNEL_SELECT_TAPS - 1) / 2;
        let spc = samples_per_chip(sample_rate_hz as u32)
            .expect("adjacent channelizer should use an integer chip-rate multiple");
        assert_eq!(
            group_delay_samples % spc,
            0,
            "adjacent channelizer group delay should be chip-aligned"
        );
        ChannelizedSamples {
            samples: filtered,
            group_delay_chips: group_delay_samples / spc,
        }
    }

    fn decode_rake_overhead(
        samples: Vec<Complex32>,
        sample_rate_hz: u32,
    ) -> Vec<RakeCapsuleSummary> {
        let mut rake = hrpd_forward_rake_receiver(sample_rate_hz);
        let block = SampleBlock::new(samples, 0).with_sample_rate_hz(f64::from(sample_rate_hz));
        let out = rake.process_block(block);
        if !rake.has_hard_validated_finger() {
            eprintln!(
                "BTS HRPD rake did not hard-validate: output_blocks={}",
                out.len()
            );
            for block in out.iter().filter(|block| !block.tags.is_empty()).take(24) {
                eprintln!(
                    "  event chip_start={} samples={} tags={:?}",
                    block.chip_start,
                    block.samples.len(),
                    block.tags
                );
            }
        }
        assert!(
            rake.has_hard_validated_finger(),
            "receiver should hard-validate a forward HRPD finger"
        );

        let capsule_events = out
            .iter()
            .filter(|block| block.tags.get("hrpd_control_capsule_decoded").copied() == Some(1))
            .collect::<Vec<_>>();
        let message_events = out
            .iter()
            .filter(|block| block.tags.get("hrpd_forward_signaling_message").copied() == Some(1))
            .collect::<Vec<_>>();

        let mut summaries = Vec::with_capacity(capsule_events.len());
        for capsule in capsule_events {
            let slot_start = capsule
                .tags
                .get("hrpd_control_slot_start_chip")
                .copied()
                .unwrap_or(capsule.chip_start as i64) as usize;
            let payload_bits = capsule
                .tags
                .get("hrpd_control_payload_bits")
                .copied()
                .expect("capsule event should tag payload bits")
                as u32;
            let message_count = capsule
                .tags
                .get("hrpd_control_message_count")
                .copied()
                .expect("capsule event should tag message count")
                as usize;
            let mut messages = Vec::new();
            for message_event in message_events.iter().filter(|event| {
                event
                    .tags
                    .get("hrpd_control_slot_start_chip")
                    .copied()
                    .map(|slot| slot as usize == slot_start)
                    .unwrap_or(false)
            }) {
                let protocol_type = message_event
                    .tags
                    .get("hrpd_signaling_protocol_type")
                    .copied()
                    .expect("message event should tag protocol type")
                    as u8;
                let payload = message_payload_bytes(&message_event.samples);
                if let Some(overhead) =
                    HrpdOverheadMessage::decode_for_protocol(protocol_type, &payload)
                {
                    messages.push(overhead);
                }
            }
            assert_eq!(
                messages.len(),
                message_count,
                "all decoded BTS messages should be covered by overhead decoders"
            );
            summaries.push(RakeCapsuleSummary {
                slot_start_chip: slot_start,
                payload_bits,
                messages,
            });
        }
        summaries
    }

    fn assert_bts_overhead_sequence(
        label: &str,
        resolved: &ResolvedEvdoConfig,
        start_chip: u64,
        expected_boundary_offset_chips: usize,
        capsules: &[RakeCapsuleSummary],
    ) {
        let cycle_chips = control_channel_cycle_chips();
        assert_eq!(
            capsules.len(),
            4,
            "{label}: four generated Control Channel cycles should decode"
        );
        for (idx, capsule) in capsules.iter().enumerate() {
            assert_eq!(
                capsule.slot_start_chip,
                idx * cycle_chips + expected_boundary_offset_chips,
                "{label}: capsule should land on the spec Control Channel cycle boundary"
            );
            assert_eq!(
                capsule.payload_bits, 1024,
                "{label}: default capsule should use low-rate 1024-bit physical payloads"
            );
        }

        let got_kinds = capsules
            .iter()
            .map(|capsule| {
                capsule
                    .messages
                    .iter()
                    .map(overhead_message_type)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            got_kinds,
            vec![vec![0, 1, 3], vec![1, 4], vec![1, 2], vec![0, 1, 3]],
            "{label}: generated overhead cadence should match the spec schedule (QuickConfig in every Sleep-State capsule)"
        );

        let quick = capsules[2]
            .messages
            .iter()
            .find_map(|message| match message {
                HrpdOverheadMessage::QuickConfig(m) => Some(m),
                _ => None,
            })
            .expect("third BTS overhead capsule should carry QuickConfig");
        assert_eq!(quick.color_code, resolved.overhead.color_code);
        assert_eq!(
            quick.sector_id24,
            resolved.overhead.sector_id24(),
            "{label}: QuickConfig SectorID24 should be the low 24 bits of configured SectorID"
        );

        let access = capsules[0]
            .messages
            .iter()
            .find_map(|message| match message {
                HrpdOverheadMessage::AccessParameters(m) => Some(m),
                _ => None,
            })
            .expect("first BTS overhead capsule should carry AccessParameters");
        let mut expected_access = AccessParameters::defaults();
        expected_access.access_signature = resolved.overhead.access_signature;
        assert_eq!(
            access, &expected_access,
            "{label}: first cycle should carry configured AccessParameters"
        );

        let reverse_rate = capsules[1]
            .messages
            .iter()
            .find_map(|message| match message {
                HrpdOverheadMessage::BroadcastReverseRateLimit(m) => Some(m),
                _ => None,
            })
            .expect("second BTS overhead capsule should carry BroadcastReverseRateLimit");
        assert_eq!(reverse_rate.rpc_count, 63);
        assert_eq!(reverse_rate.rate_limit, vec![5; 63]);

        let sector = capsules[2]
            .messages
            .iter()
            .find_map(|message| match message {
                HrpdOverheadMessage::SectorParameters(m) => Some(m),
                _ => None,
            })
            .expect("third BTS overhead capsule should carry SectorParameters");
        assert_eq!(
            quick.sector_signature, sector.sector_signature,
            "{label}: QuickConfig SectorSignature should match SectorParameters"
        );
        assert_eq!(
            quick.access_signature, access.access_signature,
            "{label}: QuickConfig AccessSignature should match AccessParameters"
        );
        assert_eq!(
            sector.sector_signature, resolved.overhead.sector_signature,
            "{label}: SectorParameters should use configured SectorSignature"
        );
        assert_eq!(
            access.access_signature, resolved.overhead.access_signature,
            "{label}: AccessParameters should use configured AccessSignature"
        );
        assert_eq!(
            sector.sector_id, resolved.overhead.sector_id,
            "{label}: SectorParameters should advertise configured SectorID"
        );
        assert_eq!(
            sector.subnet_mask, resolved.overhead.subnet_mask,
            "{label}: SectorParameters should advertise configured SubnetMask"
        );
        assert_eq!(
            sector.country_code, 310,
            "{label}: SectorParameters should advertise MCC 310 as decimal 310, not BCD/raw 0x310"
        );
        assert_eq!(
            sector.channels,
            vec![ChannelRecord {
                system_type: 0x00,
                band_class: resolved.evdo_band_class & 0x1F,
                channel_number: resolved.evdo_channel & 0x07FF,
            }]
        );
        assert_eq!(sector.neighbors.len(), 1);
        assert_eq!(sector.neighbors[0].pilot_pn, resolved.pilot_pn);
        assert_eq!(
            sector.neighbors[0].channel,
            Some(ChannelRecord {
                system_type: 0x01,
                band_class: resolved.one_x_band_class & 0x1F,
                channel_number: resolved.one_x_channel & 0x07FF,
            })
        );

        let expected_sync_times = [
            ((start_chip + 196_608) / 32_768) & ((1u64 << 37) - 1),
            ((start_chip + 3 * cycle_chips as u64 + 196_608) / 32_768) & ((1u64 << 37) - 1),
        ];
        let syncs = capsules
            .iter()
            .flat_map(|capsule| capsule.messages.iter())
            .filter_map(|message| match message {
                HrpdOverheadMessage::Sync(m) => Some(m),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            syncs.len(),
            2,
            "{label}: Sync should fire every third control cycle"
        );
        for (sync, expected_system_time) in syncs.iter().zip(expected_sync_times) {
            assert_eq!(sync.pilot_pn, resolved.pilot_pn);
            assert_eq!(sync.system_time, expected_system_time);
            assert_eq!(sync.maximum_revision, 1);
            assert_eq!(sync.minimum_revision, 1);
        }
    }

    fn print_bts_overhead_sequence(label: &str, capsules: &[RakeCapsuleSummary]) {
        fn channel_label(channel: &ChannelRecord) -> String {
            let system = match channel.system_type {
                0x00 => "HRPD",
                0x01 => "1x",
                _ => "system",
            };
            format!(
                "{} bc{} ch{}",
                system, channel.band_class, channel.channel_number
            )
        }

        let cycle_chips = control_channel_cycle_chips();
        eprintln!(
            "{label}: decoded {} HRPD Control Channel capsules; cycle={} chips (~426.667 ms)",
            capsules.len(),
            cycle_chips
        );
        for (idx, capsule) in capsules.iter().enumerate() {
            let delta = if idx == 0 {
                0
            } else {
                capsule
                    .slot_start_chip
                    .saturating_sub(capsules[idx - 1].slot_start_chip)
            };
            let messages = capsule
                .messages
                .iter()
                .map(|message| match message {
                    HrpdOverheadMessage::QuickConfig(m) => format!(
                        "QuickConfig(color={} sector24=0x{:06X} sigs={}/{})",
                        m.color_code, m.sector_id24, m.sector_signature, m.access_signature
                    ),
                    HrpdOverheadMessage::SectorParameters(m) => format!(
                        "SectorParameters(country={} sector={} subnet=/{} channels=[{}] neighbors=[{}])",
                        m.country_code,
                        m.sector_id
                            .iter()
                            .map(|b| format!("{b:02X}"))
                            .collect::<String>(),
                        m.subnet_mask,
                        m.channels
                            .iter()
                            .map(channel_label)
                            .collect::<Vec<_>>()
                            .join(";"),
                        m.neighbors
                            .iter()
                            .map(|n| format!(
                                "pn{} {}",
                                n.pilot_pn,
                                n.channel
                                    .as_ref()
                                    .map(channel_label)
                                    .unwrap_or_else(|| "no-channel".to_string())
                            ))
                            .collect::<Vec<_>>()
                            .join(";")
                    ),
                    HrpdOverheadMessage::AccessParameters(m) => {
                        format!("AccessParameters(signature={})", m.access_signature)
                    }
                    HrpdOverheadMessage::BroadcastReverseRateLimit(m) => format!(
                        "BroadcastReverseRateLimit(rpc_count={} rate_limit={:?})",
                        m.rpc_count, m.rate_limit
                    ),
                    HrpdOverheadMessage::Sync(m) => format!(
                        "Sync(pilot_pn={} system_time={} rev={}/{})",
                        m.pilot_pn, m.system_time, m.minimum_revision, m.maximum_revision
                    ),
                })
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "{label}: capsule#{idx} slot_start={} delta_chips={} payload_bits={} messages=[{}]",
                capsule.slot_start_chip, delta, capsule.payload_bits, messages
            );
        }
    }

    const TX_PULSE_SHAPER_GROUP_DELAY_CHIPS: usize = 0;

    #[test]
    fn rake_decodes_default_bts_hrpd_forward_overhead_e2e() {
        let (config, resolved) = configured_bts_hrpd();
        let cycle_chips = control_channel_cycle_chips();
        let start_chip = 0;
        let capture_chips = 4 * cycle_chips;
        let samples = shape_bts_hrpd_only(&config, &resolved, start_chip, capture_chips);
        assert_eq!(
            samples.len(),
            capture_chips * samples_per_chip(config.runtime.tx_sample_rate_hz as u32).unwrap()
        );

        let capsules = decode_rake_overhead(samples, config.runtime.tx_sample_rate_hz as u32);
        assert_bts_overhead_sequence(
            "default BTS HRPD-only",
            &resolved,
            start_chip,
            TX_PULSE_SHAPER_GROUP_DELAY_CHIPS,
            &capsules,
        );
        print_bts_overhead_sequence("default BTS HRPD-only", &capsules);
    }

    #[test]
    fn rake_decodes_default_bts_adjacent_composite_after_evdo_frequency_shift() {
        let (config, resolved) = configured_bts_hrpd();
        let cycle_chips = control_channel_cycle_chips();
        let start_chip = 0;
        let capture_chips = 4 * cycle_chips;
        let channelized =
            bts_adjacent_composite_evdo_baseband(&config, &resolved, start_chip, capture_chips);
        assert_eq!(
            channelized.samples.len(),
            capture_chips * samples_per_chip(config.runtime.tx_sample_rate_hz as u32).unwrap()
        );

        let expected_boundary_offset_chips =
            channelized.group_delay_chips + TX_PULSE_SHAPER_GROUP_DELAY_CHIPS;
        let capsules =
            decode_rake_overhead(channelized.samples, config.runtime.tx_sample_rate_hz as u32);
        assert_bts_overhead_sequence(
            "default BTS adjacent-composite",
            &resolved,
            start_chip,
            expected_boundary_offset_chips,
            &capsules,
        );
        print_bts_overhead_sequence("default BTS adjacent-composite", &capsules);
    }

    #[test]
    fn receiver_decodes_generated_directed_uati_assignment_control() {
        let mut modulator = HrpdForwardSlotModulator::new(0, 32_768);
        modulator.set_overhead_quick_config(None);
        modulator.set_overhead_sector_params(None);
        modulator.set_overhead_access_params(None);
        modulator.set_overhead_reverse_rate(None);
        modulator.set_overhead_sync(None);
        let uati104 = [
            0x00, 0x80, 0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let payload = HrpdUatiAssignment::from_uati032(1, 26, 0x8005_8001)
            .with_subnet(HrpdUatiSubnetAssignment {
                uati_subnet_mask: 26,
                uati104,
            })
            .encode();
        assert_eq!(
            payload,
            vec![
                0x01, 0x01, 0x01, 0x1a, 0x00, 0x80, 0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x1a, 0x05, 0x80, 0x01, 0x00,
            ]
        );
        modulator.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Rati,
                value: 0x5232_af53,
            },
            protocol_type: DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
            payload: payload.clone(),
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        let chips = modulator.next_block(0, (SLOT_CHIPS * 96) as usize);
        let mut decoded_message = None;
        for mode in [PnDespreadMode::MultiplyPn, PnDespreadMode::MultiplyConjPn] {
            let despread = despread_chips(&chips, 0, mode);
            let equalized = pilot_equalize_half_slots(&despread, 0);
            for slot_idx in (0..96usize).step_by(4) {
                let slot_start = slot_idx * SLOT_CHIPS as usize;
                for rate in LowRateControl::ALL {
                    let Some(capsule) =
                        decode_low_rate_control_packet_capsule(&equalized, slot_start, rate)
                    else {
                        continue;
                    };
                    if let Some(message) = capsule
                        .signaling_messages
                        .iter()
                        .map(forward_signaling_message)
                        .find(|message| {
                            message.protocol_type == DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
                                && message.message_id == Some(0x01)
                        })
                    {
                        decoded_message = Some(message);
                        break;
                    }
                }
                if decoded_message.is_some() {
                    break;
                }
            }
            if decoded_message.is_some() {
                break;
            }
        }
        let message = decoded_message.expect("expected decoded UATIAssignment signaling message");
        assert_eq!(message.ati_type, 0b11);
        assert_eq!(message.ati, Some(0x5232_af53));
        assert_eq!(message.payload, payload);
    }

    #[test]
    fn receiver_decodes_generated_directed_traffic_channel_assignment_control() {
        let mut modulator = HrpdForwardSlotModulator::new(0, 32_768);
        modulator.set_overhead_quick_config(None);
        modulator.set_overhead_sector_params(None);
        modulator.set_overhead_access_params(None);
        modulator.set_overhead_reverse_rate(None);
        modulator.set_overhead_sync(None);
        let assignment = HrpdTrafficChannelAssignment::single_pilot(0, None, 0, 5);
        let payload = assignment.encode();
        modulator.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            },
            protocol_type: DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            payload: payload.clone(),
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        let chips = modulator.next_block(0, (SLOT_CHIPS * 96) as usize);
        let mut decoded_message = None;
        for mode in [PnDespreadMode::MultiplyPn, PnDespreadMode::MultiplyConjPn] {
            let despread = despread_chips(&chips, 0, mode);
            let equalized = pilot_equalize_half_slots(&despread, 0);
            for slot_idx in (0..96usize).step_by(4) {
                let slot_start = slot_idx * SLOT_CHIPS as usize;
                for rate in LowRateControl::ALL {
                    let Some(capsule) =
                        decode_low_rate_control_packet_capsule(&equalized, slot_start, rate)
                    else {
                        continue;
                    };
                    if let Some(message) = capsule
                        .signaling_messages
                        .iter()
                        .map(forward_signaling_message)
                        .find(|message| {
                            message.protocol_type == DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
                                && message.message_id
                                    == Some(HrpdTrafficChannelAssignment::MESSAGE_ID)
                        })
                    {
                        decoded_message = Some(message);
                        break;
                    }
                }
                if decoded_message.is_some() {
                    break;
                }
            }
            if decoded_message.is_some() {
                break;
            }
        }
        let message =
            decoded_message.expect("expected decoded TrafficChannelAssignment signaling message");
        assert_eq!(message.ati_type, 0b10);
        assert_eq!(message.ati, Some(0x1a05_8001));
        assert_eq!(message.payload, payload);
    }

    #[test]
    fn receiver_decodes_generated_traffic_assignment_after_active_mac_overhead() {
        let mut modulator = HrpdForwardSlotModulator::new(0, 32_768);
        modulator.set_overhead_quick_config(None);
        modulator.set_overhead_sector_params(None);
        modulator.set_overhead_access_params(None);
        modulator.set_overhead_reverse_rate(None);
        modulator.set_overhead_sync(None);
        modulator.set_active_macs(vec![crate::bts::hrpd::mac_encoder::ActiveMac {
            mac_index: 5,
            rpc: true,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 0,
        }]);

        let assignment = HrpdTrafficChannelAssignment::single_pilot(0, None, 0, 5);
        let payload = assignment.encode();
        modulator.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            },
            protocol_type: DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            payload: payload.clone(),
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        let chips = modulator.next_block(0, (SLOT_CHIPS * 192) as usize);
        let mut decoded_message = None;
        for mode in [PnDespreadMode::MultiplyPn, PnDespreadMode::MultiplyConjPn] {
            let despread = despread_chips(&chips, 0, mode);
            let equalized = pilot_equalize_half_slots(&despread, 0);
            for slot_idx in (0..192usize).step_by(4) {
                let slot_start = slot_idx * SLOT_CHIPS as usize;
                for rate in LowRateControl::ALL {
                    let Some(capsule) =
                        decode_low_rate_control_packet_capsule(&equalized, slot_start, rate)
                    else {
                        continue;
                    };
                    if let Some(message) = capsule
                        .signaling_messages
                        .iter()
                        .map(forward_signaling_message)
                        .find(|message| {
                            message.protocol_type == DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
                                && message.message_id
                                    == Some(HrpdTrafficChannelAssignment::MESSAGE_ID)
                        })
                    {
                        decoded_message = Some(message);
                        break;
                    }
                }
                if decoded_message.is_some() {
                    break;
                }
            }
            if decoded_message.is_some() {
                break;
            }
        }

        let message =
            decoded_message.expect("expected decoded TrafficChannelAssignment after active MAC");
        assert_eq!(message.ati_type, 0b10);
        assert_eq!(message.ati, Some(0x1a05_8001));
        assert_eq!(message.payload, payload);
    }

    fn map_qpsk_bits(bits: &[u8]) -> Vec<Complex32> {
        let scale = 1.0_f32 / 2.0_f32.sqrt();
        bits.chunks_exact(2)
            .map(|pair| {
                let i = if pair[0] == 0 { scale } else { -scale };
                let q = if pair[1] == 0 { scale } else { -scale };
                Complex32::new(i, q)
            })
            .collect()
    }

    fn repeat_symbols(symbols: &[Complex32], len: usize) -> Vec<Complex32> {
        (0..len).map(|idx| symbols[idx % symbols.len()]).collect()
    }

    fn walsh16_cover_symbols(symbols: &[Complex32]) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(symbols.len());
        for group in symbols.chunks_exact(16) {
            for col in 0..16 {
                let mut chip = Complex32::new(0.0, 0.0);
                for (row, symbol) in group.iter().enumerate() {
                    chip += *symbol * walsh16(row, col) * 0.25;
                }
                out.push(chip);
            }
        }
        out
    }

    fn scatter_packet_data_region_with_stride(
        equalized: &mut [Complex32],
        slot_start: usize,
        slots: usize,
        slot_stride: usize,
        tdm: &[Complex32],
    ) {
        let mut cursor = 0usize;
        for s in 0..slots {
            let slot = slot_start + s * slot_stride * SLOT_CHIPS as usize;
            for half_base in [0usize, HALF_SLOT_CHIPS as usize] {
                let first = slot + half_base;
                equalized[first..first + DATA_EDGE_CHIPS as usize]
                    .copy_from_slice(&tdm[cursor..cursor + DATA_EDGE_CHIPS as usize]);
                cursor += DATA_EDGE_CHIPS as usize;
                let second = first + 624;
                equalized[second..second + DATA_EDGE_CHIPS as usize]
                    .copy_from_slice(&tdm[cursor..cursor + DATA_EDGE_CHIPS as usize]);
                cursor += DATA_EDGE_CHIPS as usize;
            }
        }
        assert_eq!(cursor, tdm.len());
    }

    fn generated_spec_sync_physical_payload(pilot_pn: u16) -> Vec<u8> {
        generated_spec_sync_physical_payload_for(LowRateControl::ALL[0], pilot_pn)
    }

    fn generated_spec_sync_physical_payload_for(rate: LowRateControl, pilot_pn: u16) -> Vec<u8> {
        let mut sync = SyncMessage::defaults();
        sync.pilot_pn = pilot_pn;
        sync.system_time = 0x12345;
        let body = generated_default_signaling_sync_packet(&sync.encode());
        let payload_bits = rate.payload_bits as usize;

        let mac_len = control_mac_bits(&vec![0; payload_bits], payload_bits)
            .expect("generated payload size should have a Control MAC field")
            .len();
        let mut mac = Vec::with_capacity(mac_len);
        if control_header_has_synchronous_bit(rate.spec) {
            push_bits_value(
                &mut mac,
                if generated_control_packet_uses_synchronous_capsule_bit(rate) {
                    1
                } else {
                    0
                },
                1,
            ); // SynchronousCapsule
        }
        push_bits_value(&mut mac, 1, 1); // FirstPacket
        push_bits_value(&mut mac, 1, 1); // LastPacket
        push_bits_value(&mut mac, 0, 2); // Offset
        push_bits_value(&mut mac, 1, 1); // SleepStateCapsuleDone
        push_bits_value(&mut mac, 0, 2); // Reserved
        push_bits_u8(&mut mac, (body.len() + 1) as u8);
        push_bits_u8(&mut mac, 0); // unsecured, connection format A, reserved, short ATI.
        mac.extend(bytes_to_bits(&body));
        mac.resize(mac_len, 0);

        let mut payload = mac;
        if payload_bits == 1024 {
            let crc = physical_crc16(&payload);
            push_bits_u16(&mut payload, crc);
        } else {
            let crc = physical_crc24(&payload);
            push_bits_u24(&mut payload, crc);
        }
        payload.resize(payload_bits, 0);
        assert!(control_physical_fcs_ok(&payload, payload_bits));
        payload
    }

    fn generated_control_packet_uses_synchronous_capsule_bit(rate: LowRateControl) -> bool {
        rate.mac_index == 2 || rate.mac_index == 3
    }

    fn generated_default_signaling_sync_packet(sync_body: &[u8]) -> Vec<u8> {
        let mut bits = Vec::new();
        push_bits_value(&mut bits, 0, 2); // Stream 0: default Signaling Application.
        push_bits_value(&mut bits, 0, 4); // SLP-F reserved.
        push_bits_value(&mut bits, 0, 1); // SLP-F unfragmented.
        push_bits_value(&mut bits, 0, 1); // SLP-D best-effort without full header.
        push_bits_value(&mut bits, 0, 1); // SNP InUse instance.
        push_bits_value(&mut bits, 0x0b, 7); // Initialization State Protocol.
        bits.extend(bytes_to_bits(sync_body));
        bits_to_bytes(&bits)
    }

    fn generated_sync_physical_payload(pilot_pn: u16) -> Vec<u8> {
        generated_spec_sync_physical_payload(pilot_pn)
    }

    fn push_bits_value(bits: &mut Vec<u8>, value: u64, width: usize) {
        for shift in (0..width).rev() {
            bits.push(((value >> shift) & 1) as u8);
        }
    }

    fn push_bits_u8(bits: &mut Vec<u8>, value: u8) {
        for shift in (0..8).rev() {
            bits.push((value >> shift) & 1);
        }
    }

    fn push_bits_u16(bits: &mut Vec<u8>, value: u16) {
        for shift in (0..16).rev() {
            bits.push(((value >> shift) & 1) as u8);
        }
    }

    fn push_bits_u24(bits: &mut Vec<u8>, value: u32) {
        for shift in (0..24).rev() {
            bits.push(((value >> shift) & 1) as u8);
        }
    }

    #[test]
    fn synthetic_hrpd_pilot_only_acquires_and_despreads() {
        // Generate a clean HRPD pilot-only signal using the same spreading
        // helper as the BTS transmitter. Per C.S0024-200 1.4.1.3.2.1 the
        // pilot is all-zero symbols on the I component with Walsh cover 0;
        // `Spreader` applies the forward short-code convention used on-air by
        // our TX path. If the pipeline is correct, acquire_pilot finds PN
        // offset 0 and despread shows coherent positive-real chips.
        let samples_per_chip = 4usize;
        let n_slots = 64usize;
        let n_chips = n_slots * SLOT_CHIPS as usize; // 64 * 2048 = 131072 chips
        let pilot_pn = 0u16;

        // Step 1: build chip stream. The unspread pilot value is V=(1,0)
        // during the 96-chip pilot window and zero elsewhere.
        let mut spreader = crate::phy::spread::Spreader::new(HrpdForwardPnSequence::new(
            pilot_pn as usize,
            32_768,
        ));
        let half = HALF_SLOT_CHIPS as usize;
        let pilot_lo = PILOT_START_IN_HALF_SLOT;
        let pilot_hi = PILOT_END_IN_HALF_SLOT;
        let mut chips = Vec::with_capacity(n_chips);
        for c in 0..n_chips {
            let half_chip = c % half;
            let unspread = if (pilot_lo..pilot_hi).contains(&half_chip) {
                Complex32::new(1.0, 0.0)
            } else {
                Complex32::new(0.0, 0.0)
            };
            let on_air = spreader.spread(&unspread);
            chips.push(on_air);
        }

        // Step 2: 4× oversample by boxcar hold.
        let mut samples: Vec<Complex32> = Vec::with_capacity(n_chips * samples_per_chip);
        for c in &chips {
            for _ in 0..samples_per_chip {
                samples.push(*c);
            }
        }

        // Step 3: baseband-filter (one pulse-shape pass).
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut filtered = ComplexFir32::new(&taps).process_block(&samples);

        // Step 4: add white complex Gaussian noise (Box-Muller). Choose
        // noise amplitude so chip-level pilot SNR (after pulse shaping) is
        // around the regime we see in real captures. Filter passband
        // gain on the pilot ≈ 1.0; pilot duty cycle is 9.4% so per-sample
        // pilot variance ≈ 0.094. Set noise σ = 0.3 per axis → noise
        // variance ≈ 0.09, so chip-level SNR per sample ≈ 1 (= 0 dB).
        // Real captures look more like -10 to -20 dB chip SNR.
        let mut rng = 0x1234_5678u32;
        let mut uniform = || -> f32 {
            rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((rng >> 8) & 0xFFFFFF) as f32 / 16_777_216.0
        };
        let mut next_gauss = || -> f32 {
            let u1 = (uniform() + 1e-9).min(1.0 - 1e-9);
            let u2 = uniform();
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
        };
        // Real-world fringe cellular has chip SNR around -15 to -20 dB.
        // σ=3 gives chip SNR ≈ -13 dB which is a healthy fringe capture.
        let noise_sigma = 3.0f32;
        for s in filtered.iter_mut() {
            s.re += next_gauss() * noise_sigma;
            s.im += next_gauss() * noise_sigma;
        }

        let receiver = HrpdForwardReceiver::new((1_228_800 * samples_per_chip as u32) as u32);
        let acq = receiver
            .acquire_pilot(&filtered)
            .expect("synthetic pilot should acquire");
        eprintln!(
            "synthetic acq: pn_phase={} pn_offset={} slot_phase={} timing={} mode={:?} snr_db={:.2}",
            acq.pn_phase_chips,
            acq.pn_offset,
            acq.slot_phase_chips,
            acq.timing_phase,
            acq.despread_mode,
            acq.pilot_snr_db
        );

        // Despread at acquired position and check sign coherence of pilot bursts.
        let chip_stream = make_chip_stream(
            &filtered,
            samples_per_chip,
            acq.timing_phase,
            acq.chip_variant,
        );
        let despread = despread_chips(&chip_stream, acq.pn_phase_chips, acq.despread_mode);
        let mut total_agree = 0usize;
        let mut total_chips = 0usize;
        let mut first_burst_sums = Vec::new();
        let mut slot_start = acq.slot_phase_chips;
        for hs in 0..32 {
            if slot_start + pilot_hi > despread.len() {
                break;
            }
            let burst = &despread[slot_start + pilot_lo..slot_start + pilot_hi];
            let sum: Complex32 = burst.iter().copied().sum();
            let dir = if sum.norm_sqr() > 0.0 {
                sum / sum.norm_sqr().sqrt()
            } else {
                Complex32::new(1.0, 0.0)
            };
            let agree = burst
                .iter()
                .filter(|s| s.re * dir.re + s.im * dir.im > 0.0)
                .count();
            total_agree += agree;
            total_chips += burst.len();
            if hs < 6 {
                first_burst_sums.push((hs, sum));
            }
            slot_start += half;
        }
        for (hs, sum) in &first_burst_sums {
            eprintln!(
                "  hs={:2} sum=({:+.3}, {:+.3}) |sum|={:.3} angle={:+6.1}°",
                hs,
                sum.re,
                sum.im,
                sum.norm_sqr().sqrt(),
                sum.arg().to_degrees()
            );
        }
        let agree_pct = 100.0 * total_agree as f32 / total_chips.max(1) as f32;
        eprintln!(
            "overall sign-agreement: {}/{} = {:.1}%",
            total_agree, total_chips, agree_pct
        );
        // Sanity gate: real pilot should give >85% sign agreement.
        assert!(
            agree_pct > 85.0,
            "synthetic pilot despread is incoherent ({:.1}%); pipeline bug",
            agree_pct
        );
        assert_eq!(acq.pn_offset, pilot_pn);
    }

    #[test]
    fn synthetic_hrpd_shifted_sector_pilot_acquires_and_despreads() {
        let samples_per_chip = 4usize;
        let pilot_pn = 333u16;
        let zero_phase_chips = 12_345usize;
        let samples =
            synthetic_hrpd_pilot_capture(samples_per_chip, 96, zero_phase_chips, pilot_pn, 0.3);

        let receiver = HrpdForwardReceiver::new(1_228_800 * samples_per_chip as u32);
        let acq = receiver
            .acquire_pilot(&samples)
            .expect("shifted sector pilot should acquire");
        let expected_pn_phase =
            (zero_phase_chips + 32_768 - (usize::from(pilot_pn) * 64 % 32_768)) % 32_768;
        let expected_slot_phase = (HALF_SLOT_CHIPS as usize
            - zero_phase_chips % HALF_SLOT_CHIPS as usize)
            % HALF_SLOT_CHIPS as usize;
        let phase_error = signed_circular_delta(acq.pn_phase_chips, expected_pn_phase, 32_768);
        let slot_error = signed_circular_delta(
            acq.slot_phase_chips,
            expected_slot_phase,
            HALF_SLOT_CHIPS as usize,
        );
        let agree_pct = pilot_sign_agreement(&samples, samples_per_chip, &acq, 64);
        eprintln!(
            "shifted synthetic acq: pn_phase={} expected={} phase_error={} pn_offset={} expected_pn={} slot_phase={} expected_slot={} slot_error={} timing={} mode={:?} snr_db={:.2} sign_agree={:.1}%",
            acq.pn_phase_chips,
            expected_pn_phase,
            phase_error,
            acq.pn_offset,
            pilot_pn,
            acq.slot_phase_chips,
            expected_slot_phase,
            slot_error,
            acq.timing_phase,
            acq.despread_mode,
            acq.pilot_snr_db,
            agree_pct,
        );
        assert!(
            phase_error.unsigned_abs() <= 1,
            "absolute PN phase is wrong"
        );
        assert!(slot_error.unsigned_abs() <= 1, "half-slot phase is wrong");
        assert!(
            agree_pct > 90.0,
            "shifted sector pilot despread is incoherent ({agree_pct:.1}%)"
        );
    }

    #[test]
    fn rake_decodes_884490_forward_overhead_chain() {
        let path = fixture_path("evdo_downlink_884490_4x_chip.wav");
        let (sample_rate, mut samples) = read_complex_wav(&path);
        let samples_per_chip = samples_per_chip(sample_rate).expect("integer chip-rate multiple");
        samples.truncate(2_200_000usize * samples_per_chip);

        let mut rake = hrpd_forward_rake_receiver(sample_rate);
        let block = SampleBlock::new(samples, 0).with_sample_rate_hz(sample_rate as f64);
        let out = rake.process_block(block);
        let overhead = out
            .iter()
            .filter(|b| {
                b.tags
                    .get("hrpd_forward_overhead_decoded")
                    .copied()
                    .unwrap_or(0)
                    != 0
            })
            .collect::<Vec<_>>();

        for block in &overhead {
            eprintln!(
                "884.490 rake overhead event: chip_start={} tags={:?}",
                block.chip_start, block.tags
            );
        }
        assert!(
            overhead.len() >= 3,
            "884.490 RAKE chain should decode multiple overhead capsules"
        );
        let sync = overhead
            .iter()
            .find(|b| b.tags.get("hrpd_sync_decoded").copied() == Some(1))
            .expect("884.490 RAKE chain should decode Sync");
        assert_eq!(sync.tags.get("hrpd_sync_pilot_pn").copied(), Some(4));
        assert!(rake.has_hard_validated_finger());
    }

    #[test]
    fn decodes_sync_from_884490_downlink_capture_wav() {
        let path = fixture_path("evdo_downlink_884490_4x_chip.wav");
        let (sample_rate, samples) = read_complex_wav(&path);
        let receiver = HrpdForwardReceiver::new(sample_rate);
        let acquisition = receiver
            .acquire_pilot(&samples)
            .expect("884.490 MHz capture should acquire pilot");
        eprintln!("884.490 MHz acquisition: {acquisition:?}");
        let decoded = receiver
            .decode_sync_with_acquisition(&samples, &acquisition)
            .expect("884.490 MHz capture should decode Sync");
        eprintln!("884.490 MHz sync decode: {decoded:?}");
        assert_eq!(decoded.payload_bits, 1024);
        assert_eq!(decoded.sync.maximum_revision, 1);
        assert_eq!(decoded.sync.minimum_revision, 1);
        assert_eq!(decoded.sync.pilot_pn, 4);
        assert_eq!(decoded.sync.system_time, 54_867_849_078);
        assert!(decoded.overhead_messages.iter().any(|message| matches!(
            message,
            HrpdOverheadMessage::Sync(sync)
                if sync.maximum_revision == 1
                    && sync.minimum_revision == 1
                    && sync.pilot_pn == 4
                    && sync.system_time == 54_867_849_078
        )));
    }

    fn physical_crc16(bits: &[u8]) -> u16 {
        super::physical_crc16(bits)
    }

    #[derive(Debug)]
    struct PilotHalfslotAlignmentSummary {
        coherent_best_start: usize,
        coherent_pilot_to_outside_db: f32,
    }

    fn pilot_halfslot_alignment_summary(
        despread: &[Complex32],
        halfslot_phase: usize,
    ) -> PilotHalfslotAlignmentSummary {
        let half = HALF_SLOT_CHIPS as usize;
        let burst = PILOT_END_IN_HALF_SLOT - PILOT_START_IN_HALF_SLOT;
        let max_start = half - burst;
        let mut coherent_fold = vec![Complex32::new(0.0, 0.0); half];

        let mut base = halfslot_phase;
        while base + half <= despread.len() {
            let mut prefix = vec![Complex32::new(0.0, 0.0); half + 1];
            for idx in 0..half {
                prefix[idx + 1] = prefix[idx] + despread[base + idx];
            }
            let h = prefix[PILOT_END_IN_HALF_SLOT] - prefix[PILOT_START_IN_HALF_SLOT];
            let rot = if h.norm_sqr() > 1e-12 {
                h.conj() / h.norm_sqr().sqrt()
            } else {
                Complex32::new(1.0, 0.0)
            };
            for idx in 0..half {
                coherent_fold[idx] += despread[base + idx] * rot;
            }
            base += half;
        }

        let mut aggregate_prefix = vec![Complex32::new(0.0, 0.0); half + 1];
        for idx in 0..half {
            aggregate_prefix[idx + 1] = aggregate_prefix[idx] + coherent_fold[idx];
        }
        let mut coherent_best_start = 0usize;
        let mut aggregate_best_metric = 0.0f32;
        let mut outside_metric_sum = 0.0f32;
        let mut outside_count = 0usize;
        for start in 0..=max_start {
            let metric = (aggregate_prefix[start + burst] - aggregate_prefix[start]).norm_sqr();
            if metric > aggregate_best_metric {
                aggregate_best_metric = metric;
                coherent_best_start = start;
            }
            if start.abs_diff(PILOT_START_IN_HALF_SLOT) > burst {
                outside_metric_sum += metric;
                outside_count += 1;
            }
        }

        let outside_mean = outside_metric_sum / outside_count.max(1) as f32;
        let pilot_metric = (aggregate_prefix[PILOT_END_IN_HALF_SLOT]
            - aggregate_prefix[PILOT_START_IN_HALF_SLOT])
            .norm_sqr();
        let coherent_pilot_to_outside_db = 10.0 * (pilot_metric / outside_mean.max(1e-12)).log10();

        PilotHalfslotAlignmentSummary {
            coherent_best_start,
            coherent_pilot_to_outside_db,
        }
    }

    fn walsh_biorthogonal(row: usize, col: usize) -> f32 {
        if ((row & col).count_ones() & 1) == 0 {
            1.0
        } else {
            -1.0
        }
    }

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/iq")
            .join(name)
    }

    fn read_complex_wav(path: &std::path::Path) -> (u32, Vec<Complex32>) {
        let mut reader = WavReader::open(path).expect("open complex WAV fixture");
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.bits_per_sample, 16);
        let samples = reader
            .samples::<i16>()
            .map(|s| s.expect("read PCM sample"))
            .collect::<Vec<_>>();
        let iq = samples
            .chunks_exact(2)
            .map(|iq| {
                Complex32::new(
                    iq[0] as f32 / i16::MAX as f32,
                    iq[1] as f32 / i16::MAX as f32,
                )
            })
            .collect();
        (spec.sample_rate, iq)
    }
}
