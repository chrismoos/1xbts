mod support;

use std::time::Duration;

use support::SmsRequest;

use cdma_ms::tx::{
    AccessArq, AccessChannelConfig, MsAccessIdentity, ServingSystem, build_page_response_capsule,
    build_registration_capsule, default_lc_state_at, modulate_access_probe, pulse_shape,
};

const OVERSAMPLE: u64 = 4;
const REG_CHIP: u64 = 24_576 * 100;
const PREAMBLE_FRAMES: usize = 8;
const TRAILING_FRAMES: usize = 4;
const SMS_SERVICE_OPTION: u16 = 6;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ms_answers_a_page_from_an_mt_sms() {
    let esn: u32 = 0x4CDC_1D09;
    let cfg = AccessChannelConfig::default();
    let id = MsAccessIdentity {
        esn,
        ..Default::default()
    };

    let node = support::boot();
    let sms_tx = node.sms_tx.clone();
    let mut running = support::start_running(node);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let reg =
        build_registration_capsule(&id, 1, &AccessArq::assured(0), &ServingSystem::at_p_rev(6));
    let reg_iq = pulse_shape(&modulate_access_probe(
        &cfg,
        reg.bits(),
        REG_CHIP,
        default_lc_state_at(REG_CHIP),
        PREAMBLE_FRAMES,
        TRAILING_FRAMES,
    ));
    let reg_chips = reg_iq.len() as u64 / OVERSAMPLE;
    running.inject_reverse(reg_iq, REG_CHIP);
    running
        .wait_for_mobile(esn, Duration::from_secs(10))
        .await
        .expect("MS should register");

    sms_tx
        .send(SmsRequest {
            originating_number: "5550001".to_string(),
            text: "ping".to_string(),
            target_address: Some(format!("ESN:0x{esn:08X}")),
            target_subscriber_id: None,
            timeout_ms: None,
            destination_number: None,
            sms_id: None,
            delivery_attempt_id: None,
            a1_tag: None,
            raw_payload: None,
        })
        .await
        .expect("sms send");
    running
        .wait_for_mobile_where(esn, Duration::from_secs(10), |m| m.state == "Paged")
        .await
        .expect("MT SMS should page the MS");

    let pr_chip = REG_CHIP + reg_chips;
    let pr = build_page_response_capsule(
        &id,
        SMS_SERVICE_OPTION,
        &AccessArq::assured(1),
        &ServingSystem::at_p_rev(6),
    );
    let pr_iq = pulse_shape(&modulate_access_probe(
        &cfg,
        pr.bits(),
        pr_chip,
        default_lc_state_at(pr_chip),
        PREAMBLE_FRAMES,
        TRAILING_FRAMES,
    ));
    running.inject_reverse(pr_iq, pr_chip);

    let mut seen: Vec<String> = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let s = running
                .mobiles
                .borrow()
                .iter()
                .find(|m| m.esn == Some(esn))
                .map(|m| m.state.clone());
            if let Some(s) = s {
                if seen.last() != Some(&s) {
                    eprintln!("mobile state: {s}");
                    seen.push(s.clone());
                    if s == "PageResponseReceived" {
                        return;
                    }
                }
            }
            if running.mobiles.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    running.shutdown().await;

    assert!(
        seen.iter().any(|s| s == "PageResponseReceived"),
        "MS page response should reach the BSC (states: {seen:?})"
    );
}
