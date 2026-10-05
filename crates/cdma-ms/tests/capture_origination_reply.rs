use std::path::{Path, PathBuf};

use cdma_ms::forward_rx::directed::DirectedBody;
use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use cdma_ms::iq_radio::read_iq_wav;
use cdma_ms::tx::MsAccessIdentity;

const CHUNK_SAMPLES: usize = 65_536;

#[test]
#[ignore = "requires the local-only cdma_ms_aging_reply_audit.wav capture"]
fn capture_origination_contains_acknowledgments_and_channel_assignment() {
    let relative = Path::new("test/capture/cdma_ms_aging_reply_audit.wav");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|ancestor| ancestor.join(relative))
        .find(|candidate| candidate.exists())
        .expect("origination reply capture");
    let (iq, rate) = read_iq_wav(&path).unwrap();
    let identity = MsAccessIdentity {
        esn: 0x1234_5678,
        imsi_s: 1_234_567_890,
        ..Default::default()
    };
    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), rate).with_ms_identity(&identity);
    let mut paging = 0;
    let mut directed = 0;
    let mut assignments = 0;
    let mut acknowledgments = Vec::new();
    for (index, chunk) in iq.chunks(CHUNK_SAMPLES).enumerate() {
        for event in rx.push(chunk) {
            match event {
                ForwardEvent::Directed(message) => {
                    directed += 1;
                    println!(
                        "t={} directed={message:?}",
                        index as f64 * CHUNK_SAMPLES as f64 / rate
                    );
                    if message.valid_ack {
                        acknowledgments.push(message.ack_seq);
                    }
                    if matches!(message.body, DirectedBody::ChannelAssignment(_)) {
                        assignments += 1;
                    }
                }
                ForwardEvent::Paging(_) => paging += 1,
                _ => {}
            }
        }
    }
    println!("paging={paging} directed={directed} assignments={assignments}");
    assert_eq!((paging, directed, assignments), (185, 3, 1));
    assert_eq!(acknowledgments, [2, 2]);
}
