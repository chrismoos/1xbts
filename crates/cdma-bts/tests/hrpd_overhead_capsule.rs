//! Smoke test: confirm the `cdma-common` HRPD overhead messages snap
//! together with the `cdma-bts` overhead schedule and Control Channel capsule
//! framing via the trait adapter.

use cdma_bts::bts::hrpd::control_channel::{
    CTRL_CH_DEFAULT_KBPS, ControlChannelCapsule, ctrl_ch_crc16,
};
use cdma_bts::bts::hrpd::overhead::{OverheadMessage, OverheadSchedule, OverheadSources};
use cdma_common::hrpd::messages::{HrpdOverheadMessage, QuickConfig};

#[test]
fn quick_config_flows_through_schedule_and_capsule_framing() {
    // Build a QuickConfig with a recognisable color code / sector id.
    let mut qc = QuickConfig::defaults();
    qc.color_code = 0xA5;
    qc.sector_id24 = 0x12_3456;
    let quick = HrpdOverheadMessage::QuickConfig(qc);

    let sources = OverheadSources {
        quick_config: Some(&quick as &dyn OverheadMessage),
        sector_params: None,
        access_params: None,
        sync: None,
        reverse_rate: None,
    };
    let schedule = OverheadSchedule::defaults();

    // The default live-derived schedule fires QuickConfig at cycle 2.
    let msgs = schedule.messages_for_cycle(2, &sources);
    assert_eq!(msgs.len(), 1, "expected only QuickConfig at cycle 2");

    let bodies: Vec<Vec<u8>> = msgs.iter().map(|m| m.encode()).collect();
    // The adapter must forward to the cdma-common encoder verbatim.
    assert_eq!(bodies[0], quick.encode());
    let body_len = bodies[0].len();
    assert!(body_len > 0 && body_len <= 0xFF);

    let capsule = ControlChannelCapsule::new(bodies.clone(), CTRL_CH_DEFAULT_KBPS);
    let bits = capsule.frame();

    // Non-empty, tail is the trailing 6 zero bits.
    assert!(!bits.is_empty());
    let tail = &bits[bits.len() - 6..];
    assert!(tail.iter().all(|&b| b == 0), "tail must be six zero bits");

    // First 8 bits encode MessageLength (BE, MSB first) of the first body.
    let mut first_len: u8 = 0;
    for i in 0..8 {
        first_len = (first_len << 1) | (bits[i] & 1);
    }
    assert_eq!(
        first_len as usize, body_len,
        "embedded length octet mismatch"
    );

    // CRC self-consistency: the 16 bits before the tail should equal
    // ctrl_ch_crc16 applied to everything before them.
    let crc_end = bits.len() - 6;
    let crc_start = crc_end - 16;
    let pre_crc = &bits[..crc_start];
    let mut framed_crc: u16 = 0;
    for &b in &bits[crc_start..crc_end] {
        framed_crc = (framed_crc << 1) | (b as u16 & 1);
    }
    assert_eq!(
        framed_crc,
        ctrl_ch_crc16(pre_crc),
        "framed CRC must reproduce ctrl_ch_crc16 over the preceding bits"
    );
}
