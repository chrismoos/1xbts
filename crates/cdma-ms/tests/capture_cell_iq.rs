use std::path::{Path, PathBuf};

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use cdma_ms::iq_radio::read_iq_wav;
use cdma_ms::ms::MsCore;
use num_complex::Complex32;

const CELL_SID: u16 = 4107;
const CELL_PILOT_PN: u16 = 510;
const CHUNK_SAMPLES: usize = 65_536;

fn capture_path(name: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative = Path::new("test").join("capture").join(name);
    manifest_dir
        .ancestors()
        .map(|a| a.join(&relative))
        .find(|p| p.exists())
        .unwrap_or_else(|| manifest_dir.join(relative))
}

fn air_ms(samples: usize, rate: f64) -> f64 {
    samples as f64 * 1000.0 / rate
}

#[test]
#[ignore = "requires the local-only cell_iq.wav capture"]
fn capture_cell_iq_decodes_every_overhead_message_without_paging_crc_failures() {
    let _ = env_logger::builder().is_test(true).try_init();
    let path = capture_path("cell_iq.wav");
    let (iq, rate): (Vec<Complex32>, f64) = read_iq_wav(&path).expect("load cell capture");
    eprintln!(
        "capture: {} samples, {:.2} s at {:.4} Msps",
        iq.len(),
        iq.len() as f64 / rate,
        rate / 1e6
    );

    let mut rx = ForwardReceiver::new(
        ForwardRxConfig {
            ..Default::default()
        },
        rate,
    );
    let mut core = MsCore::new(cdma_ms::tx::MsAccessIdentity {
        esn: 0x4CDC_1D09,
        imsi_s: 1234567890,
        ..Default::default()
    });
    core.power_on();

    let mut fed = 0usize;
    let mut lock_at = None;
    let mut sync_at = None;
    let mut sync_params = None;
    let mut lock_losses = 0usize;
    let mut crc_valid = 0usize;
    let mut crc_failed = 0usize;
    let mut first_failure_at = None;
    for chunk in iq.chunks(CHUNK_SAMPLES) {
        fed += chunk.len();
        for ev in rx.push(chunk) {
            match ev {
                ForwardEvent::PilotLock => {
                    lock_at.get_or_insert(fed);
                }
                ForwardEvent::PilotLost => lock_losses += 1,
                ForwardEvent::Sync(params) => {
                    if sync_at.is_none() {
                        sync_at = Some(fed);
                        core.on_pilot_acquired(params.pilot_pn as u32);
                        core.on_sync_decoded(params.clone());
                        sync_params = Some(params);
                    }
                }
                ForwardEvent::Paging(msg) => {
                    crc_valid += 1;
                    core.ingest_paging_message(&msg, 0);
                }
                ForwardEvent::PagingCrcFail { .. } => {
                    crc_failed += 1;
                    first_failure_at.get_or_insert(fed);
                }
                ForwardEvent::PagingDecodeError { msg_type, error } => {
                    core.on_paging_decode_failed(msg_type, &error);
                }
                _ => {}
            }
        }
    }
    for ev in rx.flush() {
        if let ForwardEvent::Paging(msg) = ev {
            crc_valid += 1;
            core.ingest_paging_message(&msg, 0);
        }
    }

    let m = rx.measurements().snapshot();
    let overhead = core.overhead();
    let stats = core.paging_stats();
    eprintln!(
        "pilot: lock at {:?} ms, losses={}, Ec/Io {:.1} dB, rx {:.1} dBFS",
        lock_at.map(|s| air_ms(s, rate)),
        lock_losses,
        m.ec_io_db,
        m.rx_power_dbfs
    );
    eprintln!(
        "sync at {:?} ms: {:?}",
        sync_at.map(|s| air_ms(s, rate)),
        sync_params
    );
    eprintln!(
        "paging: crc valid={} failed={} first failure at {:?} ms, messages {:?}, undecodable {:?}, overhead {:?}",
        crc_valid,
        crc_failed,
        first_failure_at.map(|s| air_ms(s, rate)),
        stats.messages,
        stats.undecodable,
        overhead.received()
    );

    let lock_at = lock_at.expect("pilot tracker never locked on the capture");
    let sync = sync_params.expect("no sync message decoded from the capture");
    assert_eq!(sync.sid, CELL_SID);
    assert_eq!(sync.pilot_pn, CELL_PILOT_PN);
    assert!(
        air_ms(sync_at.unwrap() - lock_at, rate) <= 500.0,
        "sync took too long after lock"
    );
    assert_eq!(lock_losses, 0, "the tracker dropped a strong pilot");

    for name in ["SPM", "APM", "ESPM", "CCLM"] {
        assert!(
            overhead.received().contains(&name),
            "{name} never decoded (received {:?})",
            overhead.received()
        );
    }
    assert!(
        overhead.neighbor_list.is_some() || overhead.extended_neighbor_list.is_some(),
        "no neighbor list decoded (received {:?})",
        overhead.received()
    );
    assert_eq!(
        stats.decode_failed(),
        0,
        "CRC-valid messages the decoder rejected: {:?}",
        stats.undecodable
    );
    assert!(
        stats.messages.contains_key("GPM"),
        "no General Page Message decoded"
    );
    assert!(
        crc_valid >= 40,
        "only {crc_valid} CRC-valid paging messages in 5 s"
    );
    assert_eq!(
        crc_failed,
        0,
        "a strong pilot must not produce paging CRC failures ({crc_failed} of {})",
        crc_valid + crc_failed
    );
}
