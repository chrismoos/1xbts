mod support;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use cdma_ms::ms::{AccessReason, MsCore, MsProtocolState, REG_TYPE_POWER_UP};
use cdma_ms::tx::MsAccessIdentity;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ms_reads_overhead_and_decides_to_register() {
    let identity = MsAccessIdentity {
        esn: 0x1234_5678,
        imsi_s: 1234567890,
        ..Default::default()
    };
    let mut ms = MsCore::new(identity);
    ms.power_on();

    let node = support::boot();
    let iq = support::collect_forward_iq(node, 24_000).await;

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);
    let mut acquired = false;
    for ev in rx.decode_all(&iq) {
        match ev {
            ForwardEvent::Sync(decoded) if !acquired => {
                acquired = true;
                ms.on_pilot_acquired(decoded.pilot_pn as u32);
                ms.on_sync_decoded(decoded);
            }
            ForwardEvent::Paging(msg) => ms.ingest_paging_message(&msg, 0),
            _ => {}
        }
    }

    assert!(acquired, "MS never acquired sync from the booted node");
    assert_eq!(
        *ms.state(),
        MsProtocolState::SystemAccess {
            reason: AccessReason::Registration {
                reg_type: REG_TYPE_POWER_UP
            }
        },
        "MS should have decided to perform a power-up registration"
    );

    let sp = ms
        .system_params()
        .expect("MS should have cached System Parameters");
    assert!(sp.power_up_reg, "SPM should indicate power-up registration");
    assert_eq!(sp.sid, support::SID);

    let ap = ms
        .access_params()
        .expect("MS should have cached Access Parameters from the APM");
    eprintln!(
        "MS registration decision: reg=power_up sid={} nid={} acc_chan={} num_step={} reg_psist={}",
        sp.sid, sp.nid, ap.acc_chan, ap.num_step, ap.reg_psist
    );
}
