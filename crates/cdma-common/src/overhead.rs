use serde::{Deserialize, Serialize};

/// Cell-level overhead parameters broadcast on the sync and paging channels.
///
/// These configure the overhead message train (SPM, APM, NLM, CCLM, ESPM)
/// and sync channel content. In the target architecture the BTS owns these
/// values and generates the overhead train locally from JSON config.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OverheadParameters {
    pub sid: u16,
    pub nid: u16,
    pub base_id: u16,
    pub reg_zone: u16,
    pub total_zones: u8,
    pub zone_timer: u8,
    pub max_slot_cycle_index: u8,
    pub page_chan: u8,
    pub config_seq: u8,
    pub acc_config_seq: u8,
    pub power_up_reg: bool,
    pub parameter_reg: bool,
    pub auth_mode: u8,
    pub t1b_ms: u64,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub lp_sec: u8,
    pub ltm_off: i8,
    pub daylt: u8,
    pub prat: u8,
    pub cdma_freq: u16,
    pub ext_cdma_freq: u16,
}

impl Default for OverheadParameters {
    fn default() -> Self {
        Self {
            sid: 1,
            nid: 1,
            base_id: 1,
            reg_zone: 0,
            total_zones: 1,
            zone_timer: 0,
            max_slot_cycle_index: 0,
            page_chan: 1,
            config_seq: 23,
            acc_config_seq: 1,
            power_up_reg: true,
            parameter_reg: false,
            auth_mode: 0,
            t1b_ms: 1280,
            p_rev: 6,
            min_p_rev: 6,
            lp_sec: 0,
            ltm_off: 0,
            daylt: 0,
            prat: 0,
            cdma_freq: 384,
            ext_cdma_freq: 0,
        }
    }
}
