mod sms;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Args;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontData, FontFamily, FontId, Frame, Margin, Pos2,
    Rect, RichText, Sense, Stroke, Vec2, pos2, vec2,
};
use tokio_stream::StreamExt;

use cdma_ms::config::MsNodeConfig;
use cdma_ms::grpc::proto::ms_service_client::MsServiceClient;
use cdma_ms::grpc::proto::{
    Diagnostics, MsState, MsStatus, OriginateRequest, PowerOnRequest, SendDtmfBurstRequest,
    SendSmsRequest, VoicePcmFrame,
};
use cdma_voice::{
    SERVICE_OPTION_BASIC_VOICE, SERVICE_OPTION_EVRC_A, SERVICE_OPTION_EVRC_B,
    SERVICE_OPTION_EVRC_WB, SERVICE_OPTION_QCELP_13K,
};
use tonic::transport::Channel;

use crate::cli_client::Session;

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Args, Debug)]
pub struct GuiArgs {
    /// Address of a running `cdma-ms serve` (host:port). Without it the GUI
    /// embeds a daemon for the given radio.
    #[arg(long)]
    connect: Option<String>,
    /// Path to the MS node config (embedded daemon only).
    #[arg(long, default_value = "config/ms.json")]
    config: PathBuf,
    /// Radio to use: `sim`, `noop`, or a radio JSON file (embedded only).
    #[arg(long)]
    radio: Option<String>,
    /// PRL file used when the embedded station powers on.
    #[arg(long)]
    prl: Option<PathBuf>,
    /// Override the mobile ESN.
    #[arg(long, value_parser = crate::parse_u32_auto)]
    esn: Option<u32>,
    /// Override the provisioned 15-digit IMSI.
    #[arg(long, value_parser = crate::parse_imsi)]
    imsi: Option<String>,
}

pub struct GuiCompanion(Child);

impl Drop for GuiCompanion {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn spawn_companion(endpoint: &str) -> Result<GuiCompanion, Error> {
    Ok(GuiCompanion(
        Command::new(std::env::current_exe()?)
            .args(["gui", "--connect", endpoint])
            .spawn()?,
    ))
}

#[derive(Default)]
struct ViewState {
    status: MsStatus,
    diag: Diagnostics,
    audio: String,
    microphone_peak: f32,
    speaker_peak: f32,
    microphone_frames: u64,
    speaker_frames: u64,
    error: String,
    dtmf_result: Option<String>,
    sms: sms::Mailbox,
}

const PAGE_BG: Color32 = Color32::from_rgb(0x0c, 0x0e, 0x14);
const BODY: Color32 = Color32::from_rgb(0x3a, 0x42, 0x4f);
const BODY_EDGE: Color32 = Color32::from_rgb(0x10, 0x12, 0x18);
const SCREEN_FRAME: Color32 = Color32::from_rgb(0x10, 0x14, 0x1b);
const EL_BG: Color32 = Color32::from_rgb(0x7f, 0xe0, 0xb4);
const EL_GLOW: Color32 = Color32::from_rgb(0xc9, 0xf7, 0xdd);
const EL_INK: Color32 = Color32::from_rgb(0x0c, 0x3a, 0x28);
const EL_DIM: Color32 = Color32::from_rgb(0x2b, 0x6e, 0x50);
const EL_INPUT_BG: Color32 = Color32::from_rgb(0xd6, 0xf5, 0xe4);
const EL_SEL: Color32 = Color32::from_rgb(0x0c, 0x3a, 0x28);
const KEY_FACE: Color32 = Color32::from_rgb(0x33, 0x3b, 0x48);
const KEY_HILITE: Color32 = Color32::from_rgb(0x49, 0x52, 0x60);
const KEY_TXT: Color32 = Color32::from_rgb(0xee, 0xf2, 0xf8);
const KEY_SUB: Color32 = Color32::from_rgb(0x9a, 0xa7, 0xb6);
const SK_TXT: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xe2);
const SEND: Color32 = Color32::from_rgb(0x12, 0xb9, 0x81);
const SEND_TXT: Color32 = Color32::from_rgb(0x08, 0x21, 0x0f);
const END: Color32 = Color32::from_rgb(0xe2, 0x3b, 0x3b);
const END_TXT: Color32 = Color32::from_rgb(0x2a, 0x08, 0x08);
const EMERALD: Color32 = Color32::from_rgb(0x34, 0xd3, 0x99);

const EL_SCREEN_H: f32 = 210.0;

const CODECS: [(u16, &str); 5] = [
    (SERVICE_OPTION_BASIC_VOICE, "TIA-96 (SO1)"),
    (SERVICE_OPTION_EVRC_A, "EVRC-A (SO3)"),
    (SERVICE_OPTION_EVRC_B, "EVRC-B (SO68)"),
    (SERVICE_OPTION_EVRC_WB, "EVRC-WB (SO70)"),
    (SERVICE_OPTION_QCELP_13K, "QCELP-13K"),
];
const MENU: [&str; 5] = [
    "Messages",
    "Diagnostics",
    "Identity",
    "Voice codec",
    "Test speaker",
];

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Messages,
    Compose,
    Home,
    Menu,
    Diagnostics,
    Identity,
    Codec,
}

pub async fn run(args: GuiArgs) -> Result<(), Error> {
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
    if session.embedded {
        session.client.clone().power_on(PowerOnRequest {}).await?;
    }

    let shared = Arc::new(Mutex::new(ViewState {
        sms: sms::Mailbox::load(sms::history_path(config.identity.esn)),
        ..ViewState::default()
    }));
    spawn_sms_events(session.client.clone(), shared.clone());
    let ringing = Arc::new(AtomicBool::new(false));
    spawn_status_poll(session.client.clone(), shared.clone(), ringing.clone());
    let audio = match AudioIo::start(session.client.clone(), shared.clone(), ringing) {
        Ok(audio) => Some(audio),
        Err(error) => {
            shared.lock().unwrap().audio = format!("unavailable: {error}");
            None
        }
    };
    let codec_sel = CODECS
        .iter()
        .position(|(so, _)| *so == SERVICE_OPTION_EVRC_A)
        .unwrap_or(1);
    let app = HandsetApp {
        client: session.client.clone(),
        endpoint: session.addr.clone(),
        imsi: config.identity.imsi.clone(),
        esn: config.identity.esn,
        meid: config.identity.meid.clone(),
        sms_destination: String::new(),
        sms_text: String::new(),
        digits: String::new(),
        screen: Screen::Home,
        menu_sel: 0,
        codec_sel,
        voice_service_option: SERVICE_OPTION_EVRC_A,
        calling: false,
        call_since: None,
        ended: None,
        prev_call_state: "idle".to_string(),
        toast: None,
        fitted: false,
        scroll: 0.0,
        scroll_max: 0.0,
        prev_screen: Screen::Home,
        net_time: None,
        last_sys_time: 0,
        shared,
        _session: session,
        audio,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([332.0, 640.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "1xBTS Mobile Station",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            install_style(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| error.to_string().into())
}

fn install_fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        f.font_data
            .insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        "fredoka5",
        include_bytes!("../assets/fonts/Fredoka-500.ttf"),
    );
    add(
        "fredoka6",
        include_bytes!("../assets/fonts/Fredoka-600.ttf"),
    );
    add(
        "fredoka7",
        include_bytes!("../assets/fonts/Fredoka-700.ttf"),
    );
    add(
        "mono",
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    add(
        "monob",
        include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf"),
    );
    add("inter5", include_bytes!("../assets/fonts/Inter-Medium.ttf"));
    add(
        "inter6",
        include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    );
    add(
        "inter8",
        include_bytes!("../assets/fonts/Inter-ExtraBold.ttf"),
    );
    f.families.insert(
        FontFamily::Proportional,
        vec!["fredoka6".into(), "fredoka5".into()],
    );
    f.families
        .insert(FontFamily::Monospace, vec!["mono".into()]);
    f.families.insert(
        FontFamily::Name("f7".into()),
        vec!["fredoka7".into(), "fredoka6".into()],
    );
    f.families
        .insert(FontFamily::Name("f5".into()), vec!["fredoka5".into()]);
    f.families.insert(
        FontFamily::Name("monob".into()),
        vec!["monob".into(), "mono".into()],
    );
    f.families
        .insert(FontFamily::Name("inter5".into()), vec!["inter5".into()]);
    f.families
        .insert(FontFamily::Name("inter6".into()), vec!["inter6".into()]);
    f.families
        .insert(FontFamily::Name("inter8".into()), vec!["inter8".into()]);
    ctx.set_fonts(f);
}

fn install_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        let r = CornerRadius::same(11);
        for w in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.noninteractive,
        ] {
            w.corner_radius = r;
        }
        style.visuals.panel_fill = PAGE_BG;
        style.spacing.item_spacing = vec2(6.0, 6.0);
        style.spacing.button_padding = vec2(4.0, 2.0);
    });
}

fn f7(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("f7".into()))
}
fn f6(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
fn f5(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("f5".into()))
}
fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}
fn monob(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("monob".into()))
}
fn inter8(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("inter8".into()))
}
fn si(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("inter5".into()))
}
fn sib(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("inter6".into()))
}

fn spawn_status_poll(
    mut client: MsServiceClient<Channel>,
    shared: Arc<Mutex<ViewState>>,
    ringing: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        loop {
            match client.get_status(()).await {
                Ok(reply) => {
                    let status = reply.into_inner();
                    ringing.store(status.call_state == "ringing", Ordering::Relaxed);
                    shared.lock().unwrap().status = status;
                }
                Err(error) => shared.lock().unwrap().error = error.message().to_string(),
            }
            if let Ok(reply) = client.get_diagnostics(()).await {
                shared.lock().unwrap().diag = reply.into_inner();
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}

fn spawn_sms_events(mut client: MsServiceClient<Channel>, shared: Arc<Mutex<ViewState>>) {
    const RETRY_DELAY: Duration = Duration::from_secs(1);
    tokio::spawn(async move {
        loop {
            match client.stream_events(()).await {
                Ok(reply) => {
                    shared.lock().unwrap().sms.stream_ready = true;
                    let mut stream = reply.into_inner();
                    while let Ok(Some(event)) = stream.message().await {
                        if let Ok(event) =
                            serde_json::from_str::<cdma_ms::ms::MsEvent>(&event.detail)
                        {
                            shared.lock().unwrap().sms.on_event(&event);
                        }
                    }
                }
                Err(error) => log::warn!("SMS event stream unavailable: {error}"),
            }
            shared.lock().unwrap().sms.stream_ready = false;
            tokio::time::sleep(RETRY_DELAY).await;
        }
    });
}

struct AudioIo {
    _input: Option<Stream>,
    _output: Stream,
    speaker: Arc<Mutex<VecDeque<i16>>>,
}

impl AudioIo {
    fn start(
        client: MsServiceClient<Channel>,
        shared: Arc<Mutex<ViewState>>,
        ringing: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        let host = cpal::default_host();
        let output = host.default_output_device().ok_or("no default speaker")?;
        let output_name = output
            .description()
            .map(|description| description.name().to_string())
            .unwrap_or_else(|_| "speaker".to_string());
        let output_config = output.default_output_config()?;
        let output_rate = output_config.sample_rate() as u32;
        let output_channels = output_config.channels() as usize;
        let output_stream_config: cpal::StreamConfig = output_config.clone().into();
        let speaker = Arc::new(Mutex::new(VecDeque::<i16>::with_capacity(3_200)));

        let speaker_for_stream = speaker.clone();
        let downlink_state = shared.clone();
        let mut downlink_client = client.clone();
        tokio::spawn(async move {
            let reply = match downlink_client.stream_voice_audio(()).await {
                Ok(reply) => reply,
                Err(error) => {
                    downlink_state.lock().unwrap().error =
                        format!("speaker stream: {}", error.message());
                    return;
                }
            };
            let mut stream = reply.into_inner();
            while let Some(item) = stream.next().await {
                let frame = match item {
                    Ok(frame) => frame,
                    Err(error) => {
                        downlink_state.lock().unwrap().error =
                            format!("speaker stream: {}", error.message());
                        return;
                    }
                };
                let peak = frame
                    .samples
                    .iter()
                    .map(|sample| sample.unsigned_abs())
                    .max()
                    .unwrap_or(0) as f32
                    / i16::MAX as f32;
                let mut queue = speaker_for_stream.lock().unwrap();
                for sample in frame.samples {
                    if queue.len() < 3_200 {
                        queue.push_back(sample.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
                    }
                }
                drop(queue);
                let mut state = downlink_state.lock().unwrap();
                state.speaker_frames += 1;
                state.speaker_peak = peak;
            }
        });

        let mut playout = PlayoutState::new();
        let output_stream = match output_config.sample_format() {
            SampleFormat::F32 => {
                let speaker = speaker.clone();
                output.build_output_stream(
                    output_stream_config.clone(),
                    move |data: &mut [f32], _| {
                        fill_output(
                            data,
                            output_channels,
                            output_rate,
                            &speaker,
                            &ringing,
                            &mut playout,
                            |v| v as f32 / i16::MAX as f32,
                        )
                    },
                    audio_error,
                    None,
                )?
            }
            SampleFormat::I16 => {
                let speaker = speaker.clone();
                output.build_output_stream(
                    output_stream_config.clone(),
                    move |data: &mut [i16], _| {
                        fill_output(
                            data,
                            output_channels,
                            output_rate,
                            &speaker,
                            &ringing,
                            &mut playout,
                            |v| v,
                        )
                    },
                    audio_error,
                    None,
                )?
            }
            SampleFormat::U16 => {
                let speaker = speaker.clone();
                output.build_output_stream(
                    output_stream_config,
                    move |data: &mut [u16], _| {
                        fill_output(
                            data,
                            output_channels,
                            output_rate,
                            &speaker,
                            &ringing,
                            &mut playout,
                            |v| (i32::from(v) + 32_768) as u16,
                        )
                    },
                    audio_error,
                    None,
                )?
            }
            format => return Err(format!("unsupported speaker format {format:?}").into()),
        };
        output_stream.play()?;
        let (input_stream, input_status) = match start_microphone(&host, client, shared.clone()) {
            Ok((stream, name, rate)) => match stream.play() {
                Ok(()) => (Some(stream), format!("{name} ({rate} Hz)")),
                Err(error) => (None, format!("microphone unavailable: {error}")),
            },
            Err(error) => (None, format!("microphone unavailable: {error}")),
        };
        shared.lock().unwrap().audio =
            format!("{} · {} ({} Hz)", input_status, output_name, output_rate);
        Ok(Self {
            _input: input_stream,
            _output: output_stream,
            speaker,
        })
    }

    fn test_speaker(&self) {
        const TEST_SECONDS: usize = 1;
        const VOICE_RATE: usize = 8_000;
        const TEST_FREQUENCY_HZ: f32 = 440.0;
        const TEST_LEVEL: f32 = 8_000.0;
        let mut speaker = self.speaker.lock().unwrap();
        speaker.clear();
        speaker.extend((0..VOICE_RATE * TEST_SECONDS).map(|sample| {
            let phase =
                2.0 * std::f32::consts::PI * TEST_FREQUENCY_HZ * sample as f32 / VOICE_RATE as f32;
            (phase.sin() * TEST_LEVEL) as i16
        }));
    }
}

fn start_microphone(
    host: &cpal::Host,
    client: MsServiceClient<Channel>,
    shared: Arc<Mutex<ViewState>>,
) -> Result<(Stream, String, u32), Error> {
    let input = host.default_input_device().ok_or("no default microphone")?;
    let input_name = input
        .description()
        .map(|description| description.name().to_string())
        .unwrap_or_else(|_| "microphone".to_string());
    let input_config = input.default_input_config()?;
    let input_rate = input_config.sample_rate() as u32;
    let input_channels = input_config.channels() as usize;
    let input_stream_config: cpal::StreamConfig = input_config.clone().into();
    let (mic_tx, mic_rx) = crossbeam_channel::bounded::<Vec<f32>>(8);
    let input_stream = match input_config.sample_format() {
        SampleFormat::F32 => input.build_input_stream(
            input_stream_config.clone(),
            move |data: &[f32], _| push_input(data, input_channels, &mic_tx, |v| v),
            audio_error,
            None,
        )?,
        SampleFormat::I16 => input.build_input_stream(
            input_stream_config.clone(),
            move |data: &[i16], _| {
                push_input(data, input_channels, &mic_tx, |v| {
                    v as f32 / i16::MAX as f32
                })
            },
            audio_error,
            None,
        )?,
        SampleFormat::U16 => input.build_input_stream(
            input_stream_config,
            move |data: &[u16], _| {
                push_input(data, input_channels, &mic_tx, |v| {
                    (v as f32 - 32_768.0) / 32_768.0
                })
            },
            audio_error,
            None,
        )?,
        format => return Err(format!("unsupported microphone format {format:?}").into()),
    };
    let runtime = tokio::runtime::Handle::current();
    std::thread::Builder::new()
        .name("cdma-ms-microphone".into())
        .spawn(move || microphone_worker(mic_rx, input_rate, client, runtime, shared))?;
    Ok((input_stream, input_name, input_rate))
}

fn push_input<T: Copy>(
    data: &[T],
    channels: usize,
    tx: &crossbeam_channel::Sender<Vec<f32>>,
    convert: impl Fn(T) -> f32,
) {
    let mono = data
        .chunks(channels)
        .map(|frame| frame.iter().copied().map(&convert).sum::<f32>() / channels as f32)
        .collect();
    let _ = tx.try_send(mono);
}

fn microphone_worker(
    rx: crossbeam_channel::Receiver<Vec<f32>>,
    source_rate: u32,
    mut client: MsServiceClient<Channel>,
    runtime: tokio::runtime::Handle,
    shared: Arc<Mutex<ViewState>>,
) {
    const VOICE_RATE: u32 = 8_000;
    let mut phase = 0u32;
    let mut frame = Vec::with_capacity(cdma_voice::SAMPLES_PER_FRAME);
    let mut sequence = 0u64;
    while let Ok(samples) = rx.recv() {
        for sample in samples {
            phase += VOICE_RATE;
            if phase < source_rate {
                continue;
            }
            phase -= source_rate;
            frame.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
            if frame.len() == cdma_voice::SAMPLES_PER_FRAME {
                let peak = frame
                    .iter()
                    .map(|sample| sample.unsigned_abs())
                    .max()
                    .unwrap_or(0) as f32
                    / i16::MAX as f32;
                let request = VoicePcmFrame {
                    samples: frame.drain(..).map(i32::from).collect(),
                    sequence,
                };
                if runtime.block_on(client.push_voice_audio(request)).is_ok() {
                    let mut state = shared.lock().unwrap();
                    state.microphone_frames += 1;
                    state.microphone_peak = peak;
                }
                sequence = sequence.wrapping_add(1);
            }
        }
    }
}

struct PlayoutState {
    prev: f32,
    cur: f32,
    frac: f32,
    primed: bool,
    gain: f32,
    ring_sample: u64,
}

impl PlayoutState {
    fn new() -> Self {
        Self {
            prev: 0.0,
            cur: 0.0,
            frac: 0.0,
            primed: false,
            gain: 0.0,
            ring_sample: 0,
        }
    }
}

const VOICE_RATE_HZ: f32 = 8_000.0;
const JITTER_PREBUFFER_SAMPLES: usize = 640;

fn fill_output<T: Copy>(
    data: &mut [T],
    channels: usize,
    output_rate: u32,
    speaker: &Arc<Mutex<VecDeque<i16>>>,
    ringing: &Arc<AtomicBool>,
    st: &mut PlayoutState,
    convert: impl Fn(i16) -> T,
) {
    let step = VOICE_RATE_HZ / output_rate as f32;
    let fade_step = 1.0 / (0.005 * output_rate as f32);
    let ringing = ringing.load(Ordering::Relaxed);
    let mut queue = speaker.lock().unwrap();
    for frame in data.chunks_mut(channels) {
        if !st.primed && queue.len() >= JITTER_PREBUFFER_SAMPLES {
            st.primed = true;
        }
        if st.primed {
            st.frac += step;
            while st.frac >= 1.0 {
                match queue.pop_front() {
                    Some(sample) => {
                        st.frac -= 1.0;
                        st.prev = st.cur;
                        st.cur = sample as f32;
                    }
                    None => {
                        st.frac = 0.0;
                        st.primed = false;
                        break;
                    }
                }
            }
        }
        let target = if st.primed { 1.0 } else { 0.0 };
        st.gain = if st.gain < target {
            (st.gain + fade_step).min(target)
        } else {
            (st.gain - fade_step).max(target)
        };
        let voice = (st.prev + (st.cur - st.prev) * st.frac) * st.gain;
        let ring = if ringing {
            ringtone_sample(st.ring_sample, output_rate) as f32
        } else {
            0.0
        };
        st.ring_sample = st.ring_sample.wrapping_add(1);
        let mixed = (voice + ring).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        frame.fill(convert(mixed));
    }
}

fn ringtone_sample(sample: u64, sample_rate: u32) -> i16 {
    const LEVEL: f32 = 5_000.0;
    const GAP_S: f64 = 0.9;
    const MOTIF: [(f32, f64); 7] = [
        (880.0, 0.16),
        (0.0, 0.04),
        (988.0, 0.16),
        (0.0, 0.04),
        (1175.0, 0.16),
        (0.0, 0.04),
        (988.0, 0.32),
    ];
    let motif_s: f64 = MOTIF.iter().map(|(_, d)| d).sum();
    let period_s = motif_s + GAP_S;
    let pos = (sample as f64 / f64::from(sample_rate)) % period_s;
    if pos >= motif_s {
        return 0;
    }
    let mut start = 0.0;
    for (freq, dur) in MOTIF {
        if pos < start + dur {
            if freq == 0.0 {
                return 0;
            }
            let t_in = (pos - start) as f32;
            const ATTACK_S: f32 = 0.006;
            const RELEASE_S: f32 = 0.040;
            let dur_s = dur as f32;
            let env = if t_in < ATTACK_S {
                t_in / ATTACK_S
            } else if t_in > dur_s - RELEASE_S {
                ((dur_s - t_in) / RELEASE_S).max(0.0)
            } else {
                1.0
            };
            let tone = (2.0 * std::f32::consts::PI * freq * t_in).sin();
            return (tone * env * LEVEL) as i16;
        }
        start += dur;
    }
    0
}

fn audio_error(error: cpal::Error) {
    log::warn!("cdma-ms audio stream: {error}");
}

struct HandsetApp {
    client: MsServiceClient<Channel>,
    endpoint: String,
    imsi: String,
    esn: u32,
    meid: Option<String>,
    sms_destination: String,
    sms_text: String,
    digits: String,
    screen: Screen,
    menu_sel: usize,
    codec_sel: usize,
    voice_service_option: u16,
    calling: bool,
    call_since: Option<Instant>,
    ended: Option<(Option<String>, Instant)>,
    prev_call_state: String,
    toast: Option<(String, Instant)>,
    fitted: bool,
    scroll: f32,
    scroll_max: f32,
    prev_screen: Screen,
    net_time: Option<NetClock>,
    last_sys_time: u64,
    shared: Arc<Mutex<ViewState>>,
    _session: Session,
    audio: Option<AudioIo>,
}

#[derive(Clone, Copy)]
struct NetClock {
    skew_secs: i64,
    tz_offset_secs: i32,
}

impl HandsetApp {
    fn call(&mut self) {
        if self.digits.is_empty() {
            return;
        }
        self.calling = true;
        let mut client = self.client.clone();
        let digits = self.digits.clone();
        let service_option = self.voice_service_option;
        tokio::spawn(async move {
            let _ = client
                .originate(OriginateRequest {
                    service_option: u32::from(service_option),
                    dialed_digits: digits,
                })
                .await;
        });
    }
    fn hang_up(&mut self) {
        self.calling = false;
        self.digits.clear();
        self.screen = Screen::Home;
        let mut client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.hang_up(()).await;
        });
    }
    fn answer(&self) {
        let mut client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.answer(()).await;
        });
    }
    fn test_speaker(&mut self) {
        if let Some(audio) = &self.audio {
            audio.test_speaker();
        }
        self.toast = Some(("♪ Playing test tone…".into(), Instant::now()));
    }
    fn menu_activate(&mut self) {
        match self.menu_sel {
            0 => self.screen = Screen::Messages,
            1 => self.screen = Screen::Diagnostics,
            2 => self.screen = Screen::Identity,
            3 => {
                self.codec_sel = CODECS
                    .iter()
                    .position(|(so, _)| *so == self.voice_service_option)
                    .unwrap_or(0);
                self.screen = Screen::Codec;
            }
            4 => self.test_speaker(),
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
enum Sig {
    Bars(u8),
    Searching,
    None,
}

fn sig_state(status: &MsStatus, diag: &Diagnostics) -> Sig {
    if diag.pilot.as_ref().map(|p| p.measured).unwrap_or(false) {
        let e = diag
            .pilot
            .as_ref()
            .map(|p| p.ec_io_db)
            .unwrap_or(status.pilot_ec_io_db);
        Sig::Bars(sig_level(e))
    } else {
        if matches!(
            status.state(),
            MsState::SystemDetermination | MsState::PilotAcquisition | MsState::SyncAcquisition
        ) {
            Sig::Searching
        } else {
            Sig::None
        }
    }
}

fn sig_level(ec_io_db: f64) -> u8 {
    if ec_io_db > -8.0 {
        4
    } else if ec_io_db > -12.0 {
        3
    } else if ec_io_db > -16.0 {
        2
    } else if ec_io_db > -20.0 {
        1
    } else {
        0
    }
}

fn paint_signal(ui: &mut egui::Ui, sig: Sig, ink: Color32) {
    const N: usize = 4;
    let (bw, gap, maxh) = (3.5_f32, 2.5_f32, 13.0_f32);
    let w = N as f32 * bw + (N as f32 - 1.0) * gap;
    let (rect, _) = ui.allocate_exact_size(vec2(w, maxh), Sense::hover());
    let p = ui.painter();
    let faint = Color32::from_rgba_unmultiplied(ink.r(), ink.g(), ink.b(), 55);
    let bar = |p: &egui::Painter, i: usize, color: Color32| {
        let h = maxh * (i as f32 + 1.0) / N as f32;
        let x = rect.left() + i as f32 * (bw + gap);
        let r = Rect::from_min_max(pos2(x, rect.bottom() - h), pos2(x + bw, rect.bottom()));
        p.rect_filled(r, CornerRadius::same(1), color);
    };
    let cross = |p: &egui::Painter, color: Color32| {
        let s = Stroke::new(1.8, color);
        let c = rect.center();
        let r = 5.5;
        p.line_segment([pos2(c.x - r, c.y - r), pos2(c.x + r, c.y + r)], s);
        p.line_segment([pos2(c.x - r, c.y + r), pos2(c.x + r, c.y - r)], s);
    };
    match sig {
        Sig::Bars(level) => {
            for i in 0..N {
                bar(p, i, if (i as u8) < level { ink } else { faint });
            }
        }
        Sig::Searching | Sig::None => cross(p, ink),
    }
}

fn carrier_label(sig: Sig) -> &'static str {
    match sig {
        Sig::Bars(_) => "1xBTS",
        Sig::Searching => "Searching",
        Sig::None => "No Service",
    }
}

fn codec_label(service_option: u16) -> &'static str {
    CODECS
        .iter()
        .find(|(so, _)| *so == service_option)
        .map(|(_, label)| *label)
        .unwrap_or("-")
}

fn reg_word(registered: &str) -> (&'static str, Color32) {
    match registered {
        "yes" => ("REG", EL_INK),
        "pending" => ("reg?", EL_DIM),
        _ => ("no reg", EL_DIM),
    }
}

impl eframe::App for HandsetApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
        let (status, diag, mic_peak, spk_peak) = {
            let mut s = self.shared.lock().unwrap();
            s.sms.expire(Instant::now());
            if let Some(message) = s.dtmf_result.take() {
                self.toast = Some((message, Instant::now()));
            }
            (
                s.status.clone(),
                s.diag.clone(),
                s.microphone_peak,
                s.speaker_peak,
            )
        };

        match status.call_state.as_str() {
            "connected" => {
                self.calling = false;
                if self.call_since.is_none() {
                    self.call_since = Some(Instant::now());
                }
            }
            "idle" if self.screen == Screen::Home => self.calling = false,
            _ => {}
        }
        if matches!(
            self.prev_call_state.as_str(),
            "connected" | "ringing" | "incoming"
        ) && status.call_state == "idle"
        {
            let duration = self.call_since.map(|t| mmss(t.elapsed()));
            self.ended = Some((duration, Instant::now()));
            self.call_since = None;
            self.digits.clear();
            self.screen = Screen::Home;
        }
        self.prev_call_state = status.call_state.clone();
        if let Some((_, at)) = &self.ended {
            if at.elapsed() > Duration::from_secs(5) {
                self.ended = None;
            }
        }
        if self.screen != self.prev_screen {
            self.scroll = 0.0;
            self.prev_screen = self.screen;
        }
        if let Some((_, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(2) {
                self.toast = None;
            }
        }

        self.anchor_network_clock(&diag);
        self.handle_keys(ui);

        let outer = Frame::default()
            .fill(PAGE_BG)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                self.body(ui, &status, &diag, mic_peak, spk_peak);
            });
        if !self.fitted {
            let size = outer.response.rect.size() + Vec2::new(2.0, 6.0);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            self.fitted = true;
        }
    }
}

impl HandsetApp {
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        status: &MsStatus,
        diag: &Diagnostics,
        mic_peak: f32,
        spk_peak: f32,
    ) -> Rect {
        let phone = Frame::default()
            .fill(BODY)
            .stroke(Stroke::new(1.0, BODY_EDGE))
            .corner_radius(CornerRadius {
                nw: 28,
                ne: 28,
                sw: 44,
                se: 44,
            })
            .inner_margin(Margin::symmetric(16, 16));
        let phone = phone.show(ui, |ui| {
            ui.set_width(300.0);
            let (er, _) = ui.allocate_exact_size(vec2(300.0, 8.0), Sense::hover());
            ui.painter().rect_filled(
                Rect::from_center_size(er.center(), vec2(60.0, 6.0)),
                CornerRadius::same(3),
                Color32::from_rgb(0x14, 0x18, 0x20),
            );
            ui.add_space(4.0);
            ui.vertical_centered(|ui| paint_logo(ui, 26.0));
            ui.add_space(10.0);

            Frame::default()
                .fill(SCREEN_FRAME)
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    Frame::default()
                        .fill(EL_BG)
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(12, 12))
                        .show(ui, |ui| {
                            ui.set_width(264.0);
                            ui.set_min_height(EL_SCREEN_H);
                            ui.set_max_height(EL_SCREEN_H);
                            self.screen_contents(ui, status, diag, mic_peak, spk_peak);
                        });
                });

            ui.add_space(10.0);
            self.controls(ui, status);
            ui.add_space(9.0);
            self.keypad(ui);
        });
        phone.response.rect
    }

    fn screen_contents(
        &mut self,
        ui: &mut egui::Ui,
        status: &MsStatus,
        diag: &Diagnostics,
        mic_peak: f32,
        spk_peak: f32,
    ) {
        let sig = sig_state(status, diag);
        let (reg, reg_col) = reg_word(&status.registered);
        ui.horizontal(|ui| {
            paint_signal(ui, sig, EL_INK);
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(reg).font(si(11.0)).color(reg_col));
                ui.add_space(6.0);
                ui.label(
                    RichText::new(carrier_label(sig))
                        .font(si(12.0))
                        .color(EL_DIM),
                );
            });
        });
        ui.add_space(2.0);

        let notification = self.shared.lock().unwrap().sms.notification.clone();
        if let Some((message, _)) = notification {
            let clicked = Frame::default()
                .fill(EL_GLOW)
                .corner_radius(CornerRadius::same(5))
                .inner_margin(Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(message).font(sib(12.0)).color(EL_INK))
                            .sense(Sense::click()),
                    )
                    .clicked()
                })
                .inner;
            if clicked {
                self.screen = Screen::Messages;
                self.shared.lock().unwrap().sms.notification = None;
            }
        }
        let effective = self.effective_screen(status);
        let out = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .vertical_scroll_offset(self.scroll)
            .show(ui, |ui| {
                match effective {
                    View::Messages => self.view_messages(ui),
                    View::Compose => self.view_compose(ui, status),
                    View::Idle => self.view_idle(ui, status),
                    View::Dialing => self.view_dialing(ui, status),
                    View::Calling => self.view_calling(ui, status),
                    View::InCall => self.view_incall(ui, status, mic_peak, spk_peak),
                    View::Incoming => self.view_incoming(ui, status),
                    View::Menu => self.view_menu(ui),
                    View::Diagnostics => self.view_diag(ui, status, diag),
                    View::Identity => self.view_identity(ui, status, diag),
                    View::Codec => self.view_codec(ui),
                    View::Ended => self.view_ended(ui),
                }
                if let Some((text, _)) = &self.toast {
                    ui.add_space(6.0);
                    Frame::default()
                        .fill(EL_SEL)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new(text).font(si(11.0)).color(EL_GLOW));
                            });
                        });
                }
            });
        self.scroll = out.state.offset.y;
        self.scroll_max = (out.content_size.y - out.inner_rect.height()).max(0.0);
    }

    fn effective_screen(&self, status: &MsStatus) -> View {
        match self.screen {
            Screen::Messages => View::Messages,
            Screen::Compose => View::Compose,
            Screen::Menu => View::Menu,
            Screen::Diagnostics => View::Diagnostics,
            Screen::Identity => View::Identity,
            Screen::Codec => View::Codec,
            Screen::Home => match status.call_state.as_str() {
                "connected" => View::InCall,
                "ringing" | "incoming" => View::Incoming,
                _ if self.calling => View::Calling,
                _ if self.ended.is_some() && self.digits.is_empty() => View::Ended,
                _ if self.digits.is_empty() => View::Idle,
                _ => View::Dialing,
            },
        }
    }

    fn anchor_network_clock(&mut self, diag: &Diagnostics) {
        let Some(sync) = diag.sync.as_ref() else {
            return;
        };
        if sync.sys_time == self.last_sys_time {
            return;
        }
        self.last_sys_time = sync.sys_time;
        let net_utc = network_utc_secs(sync.sys_time, sync.lp_sec);
        if net_utc < PLAUSIBLE_MIN_UNIX {
            self.net_time = None;
            return;
        }
        self.net_time = Some(NetClock {
            skew_secs: net_utc - host_unix_secs(),
            tz_offset_secs: net_tz_offset_secs(sync.ltm_off, sync.daylt),
        });
    }

    fn wall_clock(&self) -> (String, String) {
        let (local_epoch, offset_secs) = match self.net_time {
            Some(nt) => {
                let utc = host_unix_secs() + nt.skew_secs;
                (utc + i64::from(nt.tz_offset_secs), nt.tz_offset_secs)
            }
            None => {
                let offset = chrono::Local::now().offset().local_minus_utc();
                (host_unix_secs() + i64::from(offset), offset)
            }
        };
        format_clock(local_epoch, offset_secs)
    }

    fn view_idle(&self, ui: &mut egui::Ui, status: &MsStatus) {
        let (clock, dateline) = self.wall_clock();
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(clock).font(sib(30.0)).color(EL_INK));
            ui.add_space(3.0);
            ui.label(RichText::new(dateline).font(si(11.0)).color(EL_DIM));
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("SID {} · NID {}", status.sid, status.nid))
                    .font(si(11.0))
                    .color(EL_DIM),
            );
        });
        screen_footer(
            ui,
            &format!(
                "{} · {}",
                status.state().label(),
                reg_word(&status.registered).0
            ),
        );
    }

    fn view_ended(&self, ui: &mut egui::Ui) {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Call ended").font(sib(18.0)).color(EL_INK));
            if let Some((Some(duration), _)) = &self.ended {
                ui.add_space(6.0);
                ui.label(RichText::new(duration).font(si(13.0)).color(EL_DIM));
            }
        });
    }

    fn view_dialing(&self, ui: &mut egui::Ui, status: &MsStatus) {
        ui.add_space(14.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(&self.digits).font(sib(24.0)).color(EL_INK));
        });
        screen_footer(
            ui,
            &format!(
                "{} · SID {}",
                codec_label(self.voice_service_option),
                status.sid
            ),
        );
    }

    fn view_calling(&self, ui: &mut egui::Ui, _status: &MsStatus) {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Calling…").font(sib(15.0)).color(EL_INK));
            ui.add_space(4.0);
            ui.label(RichText::new(&self.digits).font(si(13.0)).color(EL_DIM));
        });
        screen_footer(
            ui,
            &format!(
                "{} · origination sent",
                codec_label(self.voice_service_option)
            ),
        );
    }

    fn view_incall(&self, ui: &mut egui::Ui, status: &MsStatus, mic_peak: f32, spk_peak: f32) {
        let who = if status.caller_number.is_empty() {
            self.digits.clone()
        } else {
            status.caller_number.clone()
        };
        let elapsed = self
            .call_since
            .map(|t| {
                let s = t.elapsed().as_secs();
                format!("{:02}:{:02}", s / 60, s % 60)
            })
            .unwrap_or_else(|| "00:00".into());
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(who).font(sib(15.0)).color(EL_INK));
            ui.add_space(3.0);
            ui.label(
                RichText::new(format!(
                    "{elapsed} · {}",
                    codec_label(status.voice_service_option as u16)
                ))
                .font(si(11.0))
                .color(EL_DIM),
            );
        });
        ui.add_space(4.0);
        meter_row(ui, "mic", mic_peak);
        meter_row(ui, "spk", spk_peak);
    }

    fn view_incoming(&self, ui: &mut egui::Ui, status: &MsStatus) {
        let who = if status.caller_number.is_empty() {
            "Unknown".to_string()
        } else {
            status.caller_number.clone()
        };
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Incoming call").font(sib(15.0)).color(EL_INK));
            ui.add_space(4.0);
            ui.label(RichText::new(who).font(sib(19.0)).color(EL_INK));
        });
        screen_footer(
            ui,
            &format!("ring · {}", codec_label(self.voice_service_option)),
        );
    }

    fn view_messages(&mut self, ui: &mut egui::Ui) {
        screen_title(ui, "Messages");
        if ui.button("Compose").clicked() {
            self.screen = Screen::Compose;
        }
        let state = self.shared.lock().unwrap();
        if !state.sms.stream_ready {
            ui.label("Reconnecting. Incoming messages may be missed.");
        }
        if let Some(error) = &state.sms.error {
            ui.label(error);
        }
        if state.sms.messages.is_empty() {
            ui.label(
                RichText::new("No messages yet")
                    .font(si(13.0))
                    .color(EL_DIM),
            );
        }
        for message in state.sms.messages.iter().rev() {
            ui.separator();
            let direction = if message.is_outgoing { "To" } else { "From" };
            ui.label(
                RichText::new(format!("{direction} {}", message.peer))
                    .font(sib(13.0))
                    .color(EL_INK),
            );
            let timestamp = chrono::DateTime::from_timestamp(message.timestamp_secs, 0)
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%b %d %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            ui.label(
                RichText::new(format!("{timestamp} · {}", message.status.label()))
                    .font(si(10.0))
                    .color(EL_DIM),
            );
            ui.label(RichText::new(&message.text).font(si(13.0)).color(EL_INK));
        }
    }

    fn view_compose(&mut self, ui: &mut egui::Ui, status: &MsStatus) {
        ui.visuals_mut().weak_text_color = Some(EL_DIM);
        screen_title(ui, "Compose SMS");
        ui.label(RichText::new("To").color(EL_INK));
        ui.add(
            egui::TextEdit::singleline(&mut self.sms_destination)
                .hint_text(RichText::new("Number or short code").color(EL_DIM))
                .background_color(EL_INPUT_BG)
                .text_color(EL_INK)
                .desired_width(f32::INFINITY),
        );
        ui.label(RichText::new("Message").color(EL_INK));
        ui.add(
            egui::TextEdit::multiline(&mut self.sms_text)
                .background_color(EL_INPUT_BG)
                .text_color(EL_INK)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.label(
            RichText::new(format!(
                "{}/{} characters · ASCII",
                self.sms_text.chars().count(),
                sms::MAX_TEXT_CHARS
            ))
            .color(EL_INK),
        );
        let state = self.shared.lock().unwrap();
        let ready = state.sms.stream_ready
            && !state.sms.is_sending()
            && status.state() == MsState::Idle
            && status.call_state == "idle";
        drop(state);
        let validation = sms::validate(self.sms_destination.trim(), &self.sms_text);
        if !ready {
            ui.label(RichText::new("Wait for the mobile to be idle and connected.").color(EL_INK));
        }
        if let Err(error) = &validation {
            ui.label(RichText::new(error.to_string()).color(EL_INK));
        }
        if ui
            .add_enabled(ready && validation.is_ok(), egui::Button::new("Send"))
            .clicked()
        {
            self.send_sms();
        }
    }

    fn send_sms(&mut self) {
        let destination = self.sms_destination.trim().to_string();
        let text = self.sms_text.clone();
        let mut state = self.shared.lock().unwrap();
        if !state.sms.stream_ready
            || state.status.state() != MsState::Idle
            || state.status.call_state != "idle"
        {
            self.toast = Some((
                "Wait for the mobile to be idle and connected.".into(),
                Instant::now(),
            ));
            return;
        }
        let index = match state.sms.queue(destination.clone(), text.clone()) {
            Ok(index) => index,
            Err(error) => {
                self.toast = Some((error.to_string(), Instant::now()));
                return;
            }
        };
        drop(state);
        self.sms_text.clear();
        self.screen = Screen::Messages;
        let mut client = self.client.clone();
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let error = match client.send_sms(SendSmsRequest { destination, text }).await {
                Ok(reply) if reply.get_ref().success => None,
                Ok(reply) => Some(reply.into_inner().message),
                Err(error) => Some(error.message().to_string()),
            };
            if let Some(error) = error {
                shared.lock().unwrap().sms.request_failed(index, error);
            }
        });
    }

    fn view_menu(&self, ui: &mut egui::Ui) {
        screen_title(ui, "Menu");
        for (i, item) in MENU.iter().enumerate() {
            list_row(ui, item, i == self.menu_sel);
        }
    }

    fn view_diag(&self, ui: &mut egui::Ui, status: &MsStatus, diag: &Diagnostics) {
        screen_title(ui, "Diagnostics");
        let p = diag.pilot.as_ref();
        let sy = diag.sync.as_ref();
        let pg = diag.paging.as_ref();
        let rd = diag.radio.as_ref();
        let g = |v: Option<String>| v.unwrap_or_else(|| "—".into());
        egui::Grid::new("diag")
            .num_columns(2)
            .spacing(vec2(10.0, 2.0))
            .show(ui, |ui| {
                kv(ui, "Ec/Io", &g(p.map(|p| format!("{:.1} dB", p.ec_io_db))));
                kv(
                    ui,
                    "RX power",
                    &g(p.map(|p| format!("{:.1} dBFS", p.rx_power_dbfs))),
                );
                kv(
                    ui,
                    "Pilot",
                    &g(p.map(|p| {
                        if p.locked {
                            "locked".into()
                        } else {
                            "searching".into()
                        }
                    })),
                );
                kv(
                    ui,
                    "PN / Base",
                    &format!(
                        "{} / {}",
                        g(sy.map(|s| s.pilot_pn.to_string())),
                        status.base_id
                    ),
                );
                kv(ui, "SID / NID", &format!("{} / {}", status.sid, status.nid));
                kv(
                    ui,
                    "P_REV · ch",
                    &format!(
                        "{} · {}",
                        g(sy.map(|s| s.p_rev.to_string())),
                        g(sy.map(|s| s.cdma_freq.to_string()))
                    ),
                );
                kv(
                    ui,
                    "Paging",
                    &g(pg.map(|p| format!("{} ok · {} bad", p.crc_valid, p.crc_failed))),
                );
                kv(
                    ui,
                    "Uptime",
                    &g(rd.map(|r| format!("{:.0} s", r.uptime_secs))),
                );
            });
    }

    fn view_identity(&self, ui: &mut egui::Ui, _status: &MsStatus, diag: &Diagnostics) {
        screen_title(ui, "Identity");
        let imsi = &self.imsi;
        let min = if imsi.len() == 15 {
            &imsi[5..15]
        } else {
            imsi.as_str()
        };
        let ch = diag
            .sync
            .as_ref()
            .map(|s| s.cdma_freq.to_string())
            .unwrap_or_else(|| "—".into());
        egui::Grid::new("ident")
            .num_columns(2)
            .spacing(vec2(10.0, 2.0))
            .show(ui, |ui| {
                kv(ui, "IMSI", imsi);
                kv(ui, "MIN", min);
                kv(ui, "ESN", &format!("0x{:08X}", self.esn));
                kv(ui, "ESN dec", &self.esn.to_string());
                kv(ui, "MEID", self.meid.as_deref().unwrap_or("—"));
                kv(ui, "Channel", &ch);
                kv(ui, "Daemon", &self.endpoint);
            });
    }

    fn view_codec(&self, ui: &mut egui::Ui) {
        screen_title(ui, "Voice codec");
        for (i, (_, label)) in CODECS.iter().enumerate() {
            list_row(ui, label, i == self.codec_sel);
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, status: &MsStatus) {
        let view = self.effective_screen(status);
        let (sl, sr) = soft_labels(view);
        ui.horizontal(|ui| {
            ui.set_width(300.0);
            if !sl.is_empty() && soft_key(ui, sl).clicked() {
                self.soft_left(view);
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if !sr.is_empty() && soft_key(ui, sr).clicked() {
                    self.soft_right(view);
                }
            });
        });
        ui.add_space(6.0);

        let send_hot = matches!(view, View::Dialing | View::Incoming);
        let end_hot = matches!(view, View::InCall | View::Calling);
        ui.horizontal(|ui| {
            ui.set_width(300.0);
            ui.add_space(50.0);
            ui.vertical(|ui| {
                if call_key(
                    ui,
                    if view == View::Incoming {
                        "Answer"
                    } else {
                        "Call"
                    },
                    SEND,
                    SEND_TXT,
                    send_hot,
                )
                .clicked()
                {
                    self.send_pressed(view);
                }
                if call_key(ui, "End", END, END_TXT, end_hot).clicked() {
                    self.hang_up();
                    self.screen = Screen::Home;
                }
            });
            ui.add_space(10.0);
            self.navpad(ui, view);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                if call_key(ui, "C", KEY_FACE, KEY_TXT, false).clicked() {
                    self.clear_pressed(view);
                }
                if menu_key(ui).clicked() {
                    self.screen = Screen::Menu;
                    self.menu_sel = 0;
                }
            });
        });
    }

    fn navpad(&mut self, ui: &mut egui::Ui, view: View) {
        let (rect, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
        let p = ui.painter();
        p.circle_filled(rect.center(), 36.0, KEY_HILITE);
        p.circle_stroke(rect.center(), 36.0, Stroke::new(1.0, BODY_EDGE));
        p.circle_filled(rect.center(), 15.0, KEY_FACE);
        let c = rect.center();
        let tri = |pts: [Pos2; 3]| egui::Shape::convex_polygon(pts.to_vec(), SK_TXT, Stroke::NONE);
        p.add(tri([
            c + vec2(0.0, -27.0),
            c + vec2(-5.0, -19.0),
            c + vec2(5.0, -19.0),
        ]));
        p.add(tri([
            c + vec2(0.0, 27.0),
            c + vec2(-5.0, 19.0),
            c + vec2(5.0, 19.0),
        ]));
        p.add(tri([
            c + vec2(-27.0, 0.0),
            c + vec2(-19.0, -5.0),
            c + vec2(-19.0, 5.0),
        ]));
        p.add(tri([
            c + vec2(27.0, 0.0),
            c + vec2(19.0, -5.0),
            c + vec2(19.0, 5.0),
        ]));
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "OK",
            f7(10.0),
            KEY_TXT,
        );
        let up = ui.interact(
            Rect::from_min_size(rect.min, vec2(72.0, 24.0)),
            ui.id().with("nav_up"),
            Sense::click(),
        );
        let dn = ui.interact(
            Rect::from_min_size(pos2(rect.min.x, rect.max.y - 24.0), vec2(72.0, 24.0)),
            ui.id().with("nav_dn"),
            Sense::click(),
        );
        let ok = ui.interact(
            Rect::from_center_size(rect.center(), vec2(30.0, 30.0)),
            ui.id().with("nav_ok"),
            Sense::click(),
        );
        if up.clicked() {
            self.nav(view, -1);
        }
        if dn.clicked() {
            self.nav(view, 1);
        }
        if ok.clicked() {
            self.ok_pressed(view);
        }
    }

    fn keypad(&mut self, ui: &mut egui::Ui) {
        let rows = [
            [("1", ""), ("2", "ABC"), ("3", "DEF")],
            [("4", "GHI"), ("5", "JKL"), ("6", "MNO")],
            [("7", "PQRS"), ("8", "TUV"), ("9", "WXYZ")],
            [("*", ""), ("0", "+"), ("#", "")],
        ];
        for row in rows {
            ui.horizontal(|ui| {
                ui.set_width(300.0);
                ui.add_space(1.0);
                for (digit, letters) in row {
                    if key_cap(ui, digit, letters).clicked() {
                        self.press_digit(digit);
                    }
                }
            });
        }
    }

    fn press_digit(&mut self, d: &str) {
        if !matches!(self.screen, Screen::Home) {
            return;
        }
        let connected = self.shared.lock().unwrap().status.call_state == "connected";
        if connected {
            let mut client = self.client.clone();
            let shared = self.shared.clone();
            let digits = d.to_string();
            tokio::spawn(async move {
                let message = match client
                    .send_dtmf_burst(SendDtmfBurstRequest { digits })
                    .await
                {
                    Ok(_) => "DTMF burst queued".to_string(),
                    Err(error) => format!("DTMF failed: {}", error.message()),
                };
                shared.lock().unwrap().dtmf_result = Some(message);
            });
        } else {
            self.digits.push_str(d);
        }
    }
    fn send_pressed(&mut self, view: View) {
        match view {
            View::Compose => self.send_sms(),
            View::Messages => self.screen = Screen::Compose,
            View::Incoming => self.answer(),
            _ => self.call(),
        }
    }
    fn clear_pressed(&mut self, view: View) {
        match view {
            View::Dialing | View::Idle => {
                self.digits.pop();
            }
            _ => {
                self.screen = Screen::Home;
                self.toast = None;
            }
        }
    }
    fn ok_pressed(&mut self, view: View) {
        match view {
            View::Messages => self.screen = Screen::Compose,
            View::Compose => self.send_sms(),
            View::Menu => self.menu_activate(),
            View::Codec => {
                self.voice_service_option = CODECS[self.codec_sel].0;
                self.screen = Screen::Menu;
            }
            View::Idle | View::Dialing => self.call(),
            View::Incoming => self.answer(),
            _ => {}
        }
    }
    fn nav(&mut self, view: View, delta: i32) {
        let step = |sel: usize, len: usize| -> usize {
            let n = len as i32;
            (((sel as i32 + delta) % n + n) % n) as usize
        };
        match view {
            View::Menu => self.menu_sel = step(self.menu_sel, MENU.len()),
            View::Codec => self.codec_sel = step(self.codec_sel, CODECS.len()),
            View::Diagnostics | View::Identity | View::Messages => {
                const SCROLL_STEP: f32 = 28.0;
                self.scroll =
                    (self.scroll + delta as f32 * SCROLL_STEP).clamp(0.0, self.scroll_max);
            }
            _ => {}
        }
    }
    fn soft_left(&mut self, view: View) {
        match view {
            View::Messages => self.screen = Screen::Compose,
            View::Compose => self.send_sms(),
            View::Idle | View::Dialing => {
                self.screen = Screen::Menu;
                self.menu_sel = 0;
            }
            View::Menu => self.menu_activate(),
            View::Codec => {
                self.voice_service_option = CODECS[self.codec_sel].0;
                self.screen = Screen::Menu;
            }
            View::InCall => {}
            _ => {}
        }
    }
    fn soft_right(&mut self, view: View) {
        match view {
            View::Compose => self.screen = Screen::Messages,
            View::Messages => self.screen = Screen::Menu,
            View::Idle | View::Dialing => {
                self.digits.pop();
            }
            View::Incoming => self.hang_up(),
            View::Ended => self.ended = None,
            View::Diagnostics | View::Identity | View::Codec => {
                self.screen = Screen::Menu;
                self.toast = None;
            }
            _ => {
                self.screen = Screen::Home;
                self.toast = None;
            }
        }
    }

    fn handle_keys(&mut self, ui: &mut egui::Ui) {
        let view = {
            let s = self.shared.lock().unwrap();
            self.effective_screen(&s.status)
        };
        if matches!(view, View::Compose) {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.screen = Screen::Messages;
            }
            return;
        }
        ui.input(|i| {
            for ev in &i.events {
                if let egui::Event::Text(t) = ev {
                    if matches!(view, View::Idle | View::Dialing | View::InCall) {
                        for c in t.chars() {
                            if c.is_ascii_digit()
                                || c == '*'
                                || c == '#'
                                || (c == '+' && !matches!(view, View::InCall))
                            {
                                self.press_digit(&c.to_string());
                            }
                        }
                    }
                }
            }
            if i.key_pressed(egui::Key::Enter) {
                self.ok_pressed(view);
            }
            if i.key_pressed(egui::Key::Backspace) {
                self.clear_pressed(view);
            }
            if i.key_pressed(egui::Key::Escape) {
                self.hang_up();
                self.screen = Screen::Home;
            }
            if i.key_pressed(egui::Key::ArrowUp) {
                self.nav(view, -1);
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                self.nav(view, 1);
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Messages,
    Compose,
    Idle,
    Dialing,
    Calling,
    InCall,
    Incoming,
    Menu,
    Diagnostics,
    Identity,
    Codec,
    Ended,
}

fn soft_labels(view: View) -> (&'static str, &'static str) {
    match view {
        View::Messages => ("Compose", "Back"),
        View::Compose => ("Send", "Back"),
        View::Idle | View::Dialing => ("Menu", "Clear"),
        View::Menu | View::Codec => ("Select", "Back"),
        View::Incoming => ("Answer", "Reject"),
        View::InCall => ("Mute", "Spkr"),
        View::Calling => ("", "End"),
        View::Diagnostics | View::Identity => ("", "Back"),
        View::Ended => ("", "Close"),
    }
}

fn screen_title(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).font(sib(14.0)).color(EL_INK));
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    ui.painter().hline(
        r.x_range(),
        r.top() + 1.0,
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(12, 58, 40, 60)),
    );
}

fn screen_footer(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(text).font(si(11.0)).color(EL_DIM));
    });
}

fn list_row(ui: &mut egui::Ui, text: &str, selected: bool) {
    if selected {
        Frame::default()
            .fill(EL_SEL)
            .corner_radius(CornerRadius::same(3))
            .inner_margin(Margin::symmetric(5, 1))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("> {text}"))
                        .font(sib(12.5))
                        .color(EL_GLOW),
                );
            });
    } else {
        ui.label(
            RichText::new(format!("   {text}"))
                .font(si(12.5))
                .color(EL_INK),
        );
    }
}

fn kv(ui: &mut egui::Ui, k: &str, v: &str) {
    ui.label(RichText::new(k).font(mono(10.5)).color(EL_DIM));
    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
        ui.label(RichText::new(v).font(monob(10.5)).color(EL_INK));
    });
    ui.end_row();
}

fn meter_row(ui: &mut egui::Ui, label: &str, level: f32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).font(mono(10.0)).color(EL_DIM));
        let (r, _) =
            ui.allocate_exact_size(vec2(ui.available_width().min(180.0), 8.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(
            r,
            CornerRadius::same(4),
            Color32::from_rgba_unmultiplied(12, 58, 40, 40),
        );
        let w = (r.width() * level.clamp(0.0, 1.0)).max(2.0);
        p.rect_filled(
            Rect::from_min_size(r.min, vec2(w, r.height())),
            CornerRadius::same(4),
            EL_INK,
        );
    });
}

fn key_cap(ui: &mut egui::Ui, digit: &str, letters: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(94.0, 34.0), Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() { KEY_HILITE } else { KEY_FACE };
    p.rect_filled(rect, CornerRadius::same(11), fill);
    let cy = if letters.is_empty() {
        rect.center().y
    } else {
        rect.center().y - 4.0
    };
    p.text(
        pos2(rect.center().x, cy),
        Align2::CENTER_CENTER,
        digit,
        f6(17.0),
        KEY_TXT,
    );
    if !letters.is_empty() {
        p.text(
            pos2(rect.center().x, rect.center().y + 9.0),
            Align2::CENTER_CENTER,
            letters,
            f5(8.0),
            KEY_SUB,
        );
    }
    resp
}

fn soft_key(ui: &mut egui::Ui, text: &str) -> egui::Response {
    soft_key_sized(ui, text, vec2(70.0, 26.0))
}

fn menu_key(ui: &mut egui::Ui) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(46.0, 29.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(11), KEY_FACE);
    let cx = r.center().x;
    let cy = r.center().y;
    for dy in [-4.5, 0.0, 4.5] {
        p.hline((cx - 8.0)..=(cx + 8.0), cy + dy, Stroke::new(1.8, SK_TXT));
    }
    resp
}

fn soft_key_sized(ui: &mut egui::Ui, text: &str, size: Vec2) -> egui::Response {
    ui.add_sized(
        size,
        egui::Button::new(RichText::new(text).font(f6(11.0)).color(SK_TXT))
            .fill(KEY_FACE)
            .corner_radius(CornerRadius::same(11))
            .stroke(Stroke::new(1.0, BODY_EDGE)),
    )
}

fn call_key(
    ui: &mut egui::Ui,
    text: &str,
    fill: Color32,
    txt: Color32,
    hot: bool,
) -> egui::Response {
    let stroke = if hot {
        Stroke::new(2.0, Color32::WHITE)
    } else {
        Stroke::new(1.0, BODY_EDGE)
    };
    ui.add_sized(
        vec2(52.0, 30.0),
        egui::Button::new(RichText::new(text).font(f6(11.0)).color(txt))
            .fill(fill)
            .corner_radius(CornerRadius::same(14))
            .stroke(stroke),
    )
}

fn paint_logo(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(height * 4.3, height), Sense::hover());
    let s = height / 104.0;
    let o = rect.min;
    let at = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let p = ui.painter();
    let stroke = |w: f32| Stroke::new(w * s, EMERALD);
    for (radius, a0, a1) in [(24.0_f32, 205.0_f32, 335.0_f32), (14.0, 205.0, 335.0)] {
        let pts: Vec<Pos2> = (0..=10)
            .map(|k| {
                let t = a0 + (a1 - a0) * k as f32 / 10.0;
                let r = t.to_radians();
                at(52.0 + radius * r.cos(), 32.0 + radius * r.sin())
            })
            .collect();
        p.add(egui::Shape::line(pts, stroke(4.0)));
    }
    p.circle_filled(at(52.0, 30.0), 5.0 * s, EMERALD);
    p.line_segment([at(52.0, 36.0), at(52.0, 58.0)], stroke(6.0));
    p.line_segment([at(52.0, 58.0), at(38.0, 86.0)], stroke(5.0));
    p.line_segment([at(52.0, 58.0), at(66.0, 86.0)], stroke(5.0));
    p.line_segment([at(41.0, 70.0), at(63.0, 70.0)], stroke(3.0));
    let font = inter8(66.0 * s);
    let cy = at(120.0, 56.0).y;
    let mut x = at(120.0, 56.0).x;
    let track = -3.0 * s;
    for ch in "1xBTS".chars() {
        let g = p.layout_no_wrap(ch.to_string(), font.clone(), EMERALD);
        p.text(pos2(x, cy), Align2::LEFT_CENTER, ch, font.clone(), EMERALD);
        x += g.size().x + track;
    }
}

const CDMA_EPOCH_UNIX: i64 = 315_964_800;
/// LTM_OFF is a signed count of 30-minute units.
const SECONDS_PER_HALF_HOUR: i32 = 1800;
const PLAUSIBLE_MIN_UNIX: i64 = 1_577_836_800;

fn mmss(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn host_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn utc_offset_label(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let abs = secs.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", abs / 3600, (abs % 3600) / 60)
}

/// UTC seconds from SYS_TIME (80 ms units since the CDMA epoch) and the
/// broadcast leap-second count.
fn network_utc_secs(sys_time: u64, lp_sec: u32) -> i64 {
    CDMA_EPOCH_UNIX + (sys_time as i64 * 80) / 1000 - i64::from(lp_sec)
}

/// Local offset in seconds from LTM_OFF (30-minute units) plus the DAYLT hour.
fn net_tz_offset_secs(ltm_off: i32, daylt: bool) -> i32 {
    ltm_off * SECONDS_PER_HALF_HOUR + if daylt { 3600 } else { 0 }
}

fn format_clock(local_epoch: i64, offset_secs: i32) -> (String, String) {
    let dt = chrono::DateTime::from_timestamp(local_epoch, 0).unwrap_or_default();
    (
        dt.format("%H:%M").to_string(),
        format!(
            "{} · {}",
            dt.format("%a %b %-d"),
            utc_offset_label(offset_secs)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ringtone_plays_a_motif_then_a_gap() {
        let rate = 48_000u32;
        let at = |ms: u64| ringtone_sample(ms * u64::from(rate) / 1000, rate);
        assert_ne!(at(20), 0, "early in the first note");
        assert_eq!(at(1_000), 0, "silent during the gap");
        assert_ne!(at(1_840), 0, "the motif repeats next period");
    }

    #[test]
    fn playout_resamples_ratio_and_fades_in() {
        let speaker = Arc::new(Mutex::new(VecDeque::<i16>::new()));
        speaker
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(10_000i16, 4_800));
        let ringing = Arc::new(AtomicBool::new(false));
        let mut st = PlayoutState::new();
        let mut out = vec![0i16; 4_800];
        fill_output(&mut out, 1, 48_000, &speaker, &ringing, &mut st, |v| v);

        let remaining = speaker.lock().unwrap().len();
        assert!(
            (3_900..=4_100).contains(&remaining),
            "expected ~800 input samples consumed, remaining={remaining}"
        );
        assert!(
            out[0].abs() < 1_000,
            "fade-in should start quiet, got {}",
            out[0]
        );
        assert!(
            *out.last().unwrap() > 9_000,
            "output should reach the input level, got {}",
            out.last().unwrap()
        );
    }

    #[test]
    fn playout_waits_for_the_prebuffer() {
        let speaker = Arc::new(Mutex::new(VecDeque::<i16>::new()));
        speaker
            .lock()
            .unwrap()
            .extend(std::iter::repeat_n(10_000i16, 100));
        let ringing = Arc::new(AtomicBool::new(false));
        let mut st = PlayoutState::new();
        let mut out = vec![0i16; 480];
        fill_output(&mut out, 1, 48_000, &speaker, &ringing, &mut st, |v| v);
        assert!(
            out.iter().all(|&s| s == 0),
            "unprimed playout should be silent"
        );
        assert_eq!(
            speaker.lock().unwrap().len(),
            100,
            "must not consume before priming"
        );
    }

    #[test]
    fn signal_level_tracks_ec_io() {
        assert_eq!(sig_level(-5.0), 4);
        assert_eq!(sig_level(-14.0), 2);
        assert_eq!(sig_level(-25.0), 0);
    }

    #[test]
    fn utc_offset_label_keeps_half_hours() {
        assert_eq!(utc_offset_label(-4 * 3600), "UTC-04:00");
        assert_eq!(utc_offset_label(5 * 3600 + 1800), "UTC+05:30");
        assert_eq!(utc_offset_label(0), "UTC+00:00");
    }

    #[test]
    fn network_time_applies_leap_and_local_offset() {
        assert_eq!(network_utc_secs(0, 0), CDMA_EPOCH_UNIX);
        assert_eq!(network_utc_secs(1000, 18), CDMA_EPOCH_UNIX + 80 - 18);
        assert_eq!(net_tz_offset_secs(-10, true), -4 * 3600);
        assert_eq!(net_tz_offset_secs(11, false), 5 * 3600 + 1800);
    }

    #[test]
    fn format_clock_renders_time_date_and_offset() {
        assert_eq!(
            format_clock(CDMA_EPOCH_UNIX, 0),
            ("00:00".to_string(), "Sun Jan 6 · UTC+00:00".to_string())
        );
        assert_eq!(
            format_clock(CDMA_EPOCH_UNIX + 14 * 3600 + 5 * 60, -4 * 3600),
            ("14:05".to_string(), "Sun Jan 6 · UTC-04:00".to_string())
        );
    }
}
