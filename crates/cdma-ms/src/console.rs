use std::path::PathBuf;

use clap::Args;

#[derive(Args, Debug)]
pub struct ConsoleArgs {
    /// Address of a running `cdma-ms serve` (host:port). Without it the
    /// console runs the daemon itself.
    #[arg(long)]
    connect: Option<String>,

    /// Path to the MS node config (embedded daemon only).
    #[arg(long, default_value = "config/ms.json")]
    config: PathBuf,

    /// Radio to use: `sim`, `noop`, or a radio JSON file (embedded daemon
    /// only). Overrides the config's radio.
    #[arg(long)]
    radio: Option<String>,

    /// PRL file. `on` then scans its channels, and `rescan`, `sweep` and
    /// `prl` use it.
    #[arg(long)]
    prl: Option<PathBuf>,

    /// Print every event, including per-second measurements and each paging
    /// message.
    #[arg(long)]
    verbose: bool,

    /// Do not power the station on at startup (type `on` when ready).
    #[arg(long)]
    no_auto_power_on: bool,

    /// Override the mobile ESN (decimal or 0x-prefixed hex).
    #[arg(long, value_parser = crate::parse_u32_auto)]
    esn: Option<u32>,

    /// Override the provisioned 15-digit IMSI.
    #[arg(long, value_parser = crate::parse_imsi)]
    imsi: Option<String>,

    /// Launch the graphical handset alongside the console.
    #[cfg(feature = "gui")]
    #[arg(long)]
    gui: bool,
}

const HELP: &str = "\
command                       usage
  on                          power on and scan the loaded PRL
  off                         power off the station
  status                      show serving system, radio, and protocol state
  register                    request an immediate power-up registration
  pilot                       show pilot lock, Ec/Io, and receive power
  sync                        show the decoded Sync Channel Message
  overhead [spm|apm|espm|cclm|nlm|enlm]
                              show received overhead or one message in detail
  neighbors                   show the decoded neighbor list
  paging                      show paging decode counters
  stats                       show sample, overflow, and real-time counters
  scan [camp|survey]          scan the loaded PRL and optionally camp
  sweep                       survey every channel without camping
  results                     show results from the latest scan
  tune <bc> <channel>         acquire one channel, e.g. tune bc0 384
  prl <path>                  select the PRL used by later scans
  prl <sid> [nid]             check whether the loaded PRL permits a system
  channels                    list channels in the loaded PRL
  gain <db>                   set receive gain, e.g. gain 42
  power                       show transmit calibration
  power tx <dbm>              set the full-scale transmit reference
  power delay <samples>       set receive-to-transmit timing compensation
  power control on|off        enable or disable calibrated power control
  dump <seconds> <file.wav>   record forward IQ on the daemon host
  sms [text]                  send this station an MT SMS (sim radio only)
  mosms <destination> <text>  originate an SMS, e.g. mosms 555105 hello
  call <digits> [codec]       originate voice using tia96, evrc-a, evrc-b,
                              evrc-wb, or qcelp-13k (default evrc-a)
  answer                      answer a ringing mobile-terminated call
  tone [seconds]              send a 440 Hz microphone test tone (default 2 s)
  dtmf <digits>               send a DTMF burst during a voice call (0–9, *, #)
  hangup                      release the active traffic channel
  mobiles                     list mobiles known by the simulated BSC
  verbose on|off              show or hide chatty measurements and paging events
  mute                        stop printing events
  unmute                      resume printing events
  help                        show this command reference
  quit                        exit the console

Use Up/Down to recall commands. Ctrl-R searches command history.";

pub async fn run(args: ConsoleArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use cdma_ms::config::MsNodeConfig;
    use cdma_ms::grpc::proto::{
        DumpForwardRequest, OriginateRequest, PageWithSmsRequest, PowerOnRequest, ScanChannelSpec,
        ScanMode, SendDtmfBurstRequest, SendSmsRequest, SetRxGainRequest, StartScanRequest,
        TxCalibrationTrim,
    };
    use rustyline::ExternalPrinter;
    use tokio_stream::StreamExt;

    use crate::cli_client::{Session, decode_event};
    use crate::cli_radio::{self, show};

    let mut config = MsNodeConfig::load(&args.config)?;
    if let Some(esn) = args.esn {
        config.identity.esn = esn;
    }
    if let Some(imsi) = args.imsi {
        config.identity.imsi = imsi;
    }
    let session = Session::open(
        args.connect.as_deref(),
        &config,
        args.radio.as_deref(),
        args.prl.as_deref(),
    )
    .await?;
    let mut client = session.client.clone();
    #[cfg(feature = "gui")]
    let _gui = args
        .gui
        .then(|| crate::gui::spawn_companion(&session.addr))
        .transpose()?;
    println!(
        "cdma-ms console: {} {}",
        if session.embedded {
            "embedded daemon at"
        } else {
            "connected to"
        },
        session.addr
    );
    let mut prl_path: Option<String> = args.prl.as_ref().map(|p| p.display().to_string());
    let mut editor = rustyline::DefaultEditor::new()?;
    let mut event_printer = editor.create_external_printer().ok();

    let muted = Arc::new(AtomicBool::new(false));
    let verbose = Arc::new(AtomicBool::new(args.verbose));
    let printer = {
        let mut events_client = session.client.clone();
        let muted = muted.clone();
        let verbose = verbose.clone();
        tokio::spawn(async move {
            let Ok(stream) = events_client.stream_events(()).await else {
                if let Some(printer) = &mut event_printer {
                    let _ = printer.print("  (event stream unavailable)".to_string());
                } else {
                    eprintln!("  (event stream unavailable)");
                }
                return;
            };
            let mut stream = stream.into_inner();
            while let Some(Ok(ev)) = stream.next().await {
                if muted.load(Ordering::Relaxed) {
                    continue;
                }
                match decode_event(&ev) {
                    Some(typed) => {
                        if cli_radio::is_chatty(&typed) && !verbose.load(Ordering::Relaxed) {
                            continue;
                        }
                        let line = format!("  [event] {}", cli_radio::event_line(&typed));
                        if let Some(printer) = &mut event_printer {
                            let _ = printer.print(line);
                        } else {
                            println!("{line}");
                        }
                    }
                    None => {
                        let line = format!("  [event] {} {}", ev.event_type, ev.detail);
                        if let Some(printer) = &mut event_printer {
                            let _ = printer.print(line);
                        } else {
                            println!("{line}");
                        }
                    }
                }
            }
        })
    };

    println!("{HELP}\n");
    if !args.no_auto_power_on && session.embedded {
        match client.power_on(PowerOnRequest {}).await {
            Ok(_) => println!("  powered on"),
            Err(e) => println!("  power on failed: {}", e.message()),
        }
    }
    loop {
        let line = match editor.readline("ms> ") {
            Ok(line) => line,
            Err(rustyline::error::ReadlineError::Interrupted) => continue,
            Err(rustyline::error::ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        };
        let line = line.trim();
        if !line.is_empty() {
            editor.add_history_entry(line)?;
        }
        let mut words = line.split_whitespace();
        let cmd = words.next().unwrap_or("");
        let rest: Vec<&str> = words.collect();
        let outcome: Result<String, String> = match cmd {
            "" => Ok(String::new()),
            "on" => client
                .power_on(PowerOnRequest {})
                .await
                .map(|r| r.into_inner().message)
                .map_err(|e| e.message().to_string()),
            "off" => client
                .power_off(())
                .await
                .map(|_| "powered off".to_string())
                .map_err(|e| e.message().to_string()),
            "register" | "reg" => client
                .register(())
                .await
                .map(|_| "registration requested".to_string())
                .map_err(|e| e.message().to_string()),
            "status" | "pilot" | "sync" | "overhead" | "neighbors" | "paging" | "stats"
            | "results" => match client.get_diagnostics(()).await {
                Err(e) => Err(e.message().to_string()),
                Ok(r) => {
                    let d = r.into_inner();
                    Ok(match cmd {
                        "status" => show::status(&d),
                        "pilot" => d.pilot.map(|p| show::pilot(&p)).unwrap_or_default(),
                        "sync" => d
                            .sync
                            .map(|s| show::sync(&s))
                            .unwrap_or_else(|| "sync: not decoded".to_string()),
                        "overhead" => d
                            .overhead
                            .map(|o| show::overhead(&o, rest.first().copied()))
                            .unwrap_or_default(),
                        "neighbors" => d
                            .overhead
                            .map(|o| show::overhead(&o, Some("neighbors")))
                            .unwrap_or_default(),
                        "paging" => d.paging.map(|p| show::paging(&p)).unwrap_or_default(),
                        "stats" => d.radio.map(|r| show::radio(&r)).unwrap_or_default(),
                        _ => d
                            .scan
                            .map(|s| show::scan_table(&s))
                            .unwrap_or_else(|| "no scan".to_string()),
                    })
                }
            },
            "scan" | "sweep" => {
                let mode = if cmd == "sweep" {
                    "survey"
                } else {
                    rest.first().copied().unwrap_or("camp")
                };
                let scan_mode = ScanMode::from_label(mode);
                match (&prl_path, scan_mode) {
                    (_, None) => Err(format!("scan mode '{mode}' is not camp or survey")),
                    (None, _) => {
                        Err("no PRL loaded (start with --prl or use: prl <path>)".to_string())
                    }
                    (Some(path), Some(scan_mode)) => client
                        .start_scan(StartScanRequest {
                            prl_path: path.clone(),
                            mode: scan_mode.into(),
                            ..Default::default()
                        })
                        .await
                        .map(|r| {
                            let r = r.into_inner();
                            format!("scanning {} channels ({mode}): {}", r.channels, r.summary)
                        })
                        .map_err(|e| e.message().to_string()),
                }
            }
            "tune" => match (
                rest.first(),
                rest.get(1).and_then(|c| c.parse::<u32>().ok()),
            ) {
                (Some(bc), Some(ch)) => client
                    .start_scan(StartScanRequest {
                        channels: vec![ScanChannelSpec {
                            band_class: bc.to_string(),
                            channel: ch,
                        }],
                        ..Default::default()
                    })
                    .await
                    .map(|_| format!("acquiring {bc} ch{ch}"))
                    .map_err(|e| e.message().to_string()),
                _ => Err("usage: tune <bc0|bc1|...> <channel>".to_string()),
            },
            "prl" => match rest.as_slice() {
                [sid] if sid.parse::<u32>().is_ok() => {
                    verdict(&mut client, sid.parse().unwrap(), 65535).await
                }
                [sid, nid] if sid.parse::<u32>().is_ok() && nid.parse::<u32>().is_ok() => {
                    verdict(&mut client, sid.parse().unwrap(), nid.parse().unwrap()).await
                }
                [path] => {
                    prl_path = Some(path.to_string());
                    Ok(format!("PRL for the next scan: {path}"))
                }
                _ => Err("usage: prl <path> | prl <sid> [nid]".to_string()),
            },
            "channels" => client
                .list_channels(())
                .await
                .map(|r| {
                    let r = r.into_inner();
                    let mut out = vec![r.summary];
                    for c in r.channels {
                        out.push(format!(
                            "  acq#{:<3} {} ch{:<5} {:.3} MHz",
                            c.acq_index,
                            c.band_class,
                            c.channel,
                            c.frequency_hz / 1e6
                        ));
                    }
                    out.join("\n")
                })
                .map_err(|e| e.message().to_string()),
            "gain" => match rest.first().and_then(|g| g.parse::<f64>().ok()) {
                Some(gain_db) => client
                    .set_rx_gain(SetRxGainRequest { gain_db })
                    .await
                    .map(|_| format!("RX gain {gain_db:.1} dB"))
                    .map_err(|e| e.message().to_string()),
                None => Err("usage: gain <db>".to_string()),
            },
            "power" => {
                let trim = match rest.as_slice() {
                    [] => Some(TxCalibrationTrim::default()),
                    ["tx", v] => v.parse::<f64>().ok().map(|dbm| TxCalibrationTrim {
                        tx_reference_dbm: Some(dbm),
                        tx_delay_samples: None,
                        power_control: None,
                    }),
                    ["delay", v] => v.parse::<i64>().ok().map(|n| TxCalibrationTrim {
                        tx_reference_dbm: None,
                        tx_delay_samples: Some(n),
                        power_control: None,
                    }),
                    ["control", v] => match *v {
                        "on" => Some(TxCalibrationTrim {
                            tx_reference_dbm: None,
                            tx_delay_samples: None,
                            power_control: Some(true),
                        }),
                        "off" => Some(TxCalibrationTrim {
                            tx_reference_dbm: None,
                            tx_delay_samples: None,
                            power_control: Some(false),
                        }),
                        _ => None,
                    },
                    _ => None,
                };
                match trim {
                    Some(trim) => client
                        .trim_tx_calibration(trim)
                        .await
                        .map(|r| {
                            let c = r.into_inner();
                            if c.supported {
                                format!(
                                    "tx calibration: reference {:.1} dBm, delay {} samples, power control {}",
                                    c.tx_reference_dbm,
                                    c.tx_delay_samples,
                                    if c.power_control { "on" } else { "off" }
                                )
                            } else {
                                "this radio has no transmit calibration".to_string()
                            }
                        })
                        .map_err(|e| e.message().to_string()),
                    None => Err("usage: power [tx <dbm> | delay <samples> | control on|off]".to_string()),
                }
            }
            "dump" => match (
                rest.first().and_then(|s| s.parse::<f64>().ok()),
                rest.get(1),
            ) {
                (Some(seconds), Some(path)) => client
                    .dump_forward(DumpForwardRequest {
                        path: path.to_string(),
                        seconds,
                    })
                    .await
                    .map(|_| format!("dumping {seconds} s to {path}"))
                    .map_err(|e| e.message().to_string()),
                _ => Err("usage: dump <seconds> <file.wav>".to_string()),
            },
            "sms" => client
                .page_with_sms(PageWithSmsRequest {
                    text: rest.join(" "),
                })
                .await
                .map(|r| r.into_inner().message)
                .map_err(|e| e.message().to_string()),
            "mosms" => match rest.split_first() {
                Some((dest, text)) if !text.is_empty() => client
                    .send_sms(SendSmsRequest {
                        destination: dest.to_string(),
                        text: text.join(" "),
                    })
                    .await
                    .map(|r| r.into_inner().message)
                    .map_err(|e| e.message().to_string()),
                _ => Err("usage: mosms <destination> <text>".to_string()),
            },
            "call" => match rest.as_slice() {
                [digits] | [digits, _] => {
                    let service_option = rest
                        .get(1)
                        .map(|codec| crate::cli_radio::parse_voice_codec(codec))
                        .unwrap_or(Some(cdma_voice::SERVICE_OPTION_EVRC_A))
                        .ok_or_else(|| {
                            "unknown codec (use tia96, evrc-a, evrc-b, evrc-wb, or qcelp-13k)"
                                .to_string()
                        })?;
                    client
                        .originate(OriginateRequest {
                            service_option: u32::from(service_option),
                            dialed_digits: (*digits).to_string(),
                        })
                        .await
                        .map(|r| r.into_inner().message)
                        .map_err(|e| e.message().to_string())
                }
                _ => Err("usage: call <digits> [codec]".to_string()),
            },
            "answer" => client
                .answer(())
                .await
                .map(|_| "answered incoming call".to_string())
                .map_err(|e| e.message().to_string()),
            "tone" => {
                let seconds = rest
                    .first()
                    .map(|value| value.parse::<u64>())
                    .transpose()
                    .map_err(|_| "usage: tone [seconds]".to_string())?
                    .unwrap_or(2);
                let frames = seconds.saturating_mul(50);
                for frame_index in 0..frames {
                    let samples = (0..cdma_voice::SAMPLES_PER_FRAME)
                        .map(|sample_index| {
                            let index = frame_index * cdma_voice::SAMPLES_PER_FRAME as u64
                                + sample_index as u64;
                            let phase = 2.0 * std::f64::consts::PI * 440.0 * index as f64 / 8_000.0;
                            (phase.sin() * 12_000.0) as i32
                        })
                        .collect();
                    client
                        .push_voice_audio(cdma_ms::grpc::proto::VoicePcmFrame {
                            samples,
                            sequence: frame_index,
                        })
                        .await
                        .map_err(|error| error.message().to_string())?;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Ok(format!("sent {seconds} s microphone test tone"))
            }
            "dtmf" => match rest.as_slice() {
                [digits] => client
                    .send_dtmf_burst(SendDtmfBurstRequest {
                        digits: (*digits).to_string(),
                    })
                    .await
                    .map(|_| "DTMF burst queued".into())
                    .map_err(|e| e.message().to_string()),
                _ => Err("usage: dtmf <digits>".into()),
            },
            "hangup" => client
                .hang_up(())
                .await
                .map(|_| "release queued".to_string())
                .map_err(|e| e.message().to_string()),
            "mobiles" => client
                .list_mobiles(())
                .await
                .map(|r| {
                    let m = r.into_inner().mobiles;
                    if m.is_empty() {
                        "(no mobiles registered)".to_string()
                    } else {
                        m.iter()
                            .map(|e| {
                                format!("esn={} imsi={} state={}", e.esn, e.imsi, e.state().label())
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                })
                .map_err(|e| e.message().to_string()),
            "verbose" => {
                let on = rest.first().copied() != Some("off");
                verbose.store(on, Ordering::Relaxed);
                Ok(format!("verbose {}", if on { "on" } else { "off" }))
            }
            "mute" => {
                muted.store(true, Ordering::Relaxed);
                Ok("events muted".to_string())
            }
            "unmute" => {
                muted.store(false, Ordering::Relaxed);
                Ok("events unmuted".to_string())
            }
            "help" => Ok(HELP.to_string()),
            "quit" | "exit" => break,
            other => Err(format!("unknown command '{other}' (try 'help')")),
        };
        match outcome {
            Ok(text) if text.is_empty() => {}
            Ok(text) => {
                for line in text.lines() {
                    println!("  {line}");
                }
            }
            Err(e) => println!("  error: {e}"),
        }
    }

    println!("\nshutting down...");
    printer.abort();
    std::process::exit(0);
}

async fn verdict(
    client: &mut cdma_ms::grpc::proto::ms_service_client::MsServiceClient<
        tonic::transport::Channel,
    >,
    sid: u32,
    nid: u32,
) -> Result<String, String> {
    use cdma_ms::grpc::proto::PrlVerdictRequest;
    client
        .prl_verdict(PrlVerdictRequest { sid, nid })
        .await
        .map(|r| {
            let v = r.into_inner().verdict.unwrap_or_default();
            format!(
                "SID {sid} NID {nid}: {} ({}{}{}{})",
                if v.permitted {
                    "permitted"
                } else {
                    "not permitted"
                },
                v.reason().label(),
                v.record
                    .map(|r| format!(", record {r}"))
                    .unwrap_or_default(),
                v.geo.map(|g| format!(", geo {g}")).unwrap_or_default(),
                v.roaming_indicator
                    .map(|r| format!(", roam_ind {r}"))
                    .unwrap_or_default()
            )
        })
        .map_err(|e| e.message().to_string())
}
