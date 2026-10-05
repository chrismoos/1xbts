use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use crossterm::cursor::{self, MoveTo, MoveToColumn};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{
    Color as CtColor, Print, ResetColor, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, ScrollUp, disable_raw_mode,
    enable_raw_mode, size as terminal_size,
};
use crossterm::{ExecutableCommand, queue};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout as RLayout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use cdma_ms::config::MsNodeConfig;
use cdma_ms::grpc::proto::{
    Diagnostics, MsState, OriginateRequest, PowerOnRequest, ScanChannelSpec, ScanMode,
    SendDtmfBurstRequest, SendSmsRequest, SetRxGainRequest, StartScanRequest,
    ms_service_client::MsServiceClient,
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

use crate::cli_client::{Session, decode_event};
use crate::cli_radio;

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Args, Debug)]
pub struct TuiArgs {
    /// Address of a running `cdma-ms serve` (host:port). Without it the TUI
    /// embeds a daemon for the given radio.
    #[arg(long)]
    connect: Option<String>,
    /// Path to the MS node config (embedded daemon only).
    #[arg(long, default_value = "config/ms.json")]
    config: PathBuf,
    /// Radio to use: `sim`, `noop`, or a radio JSON file (embedded only).
    #[arg(long)]
    radio: Option<String>,
    /// PRL file used by `scan`, `sweep` and the M-s hotkey.
    #[arg(long)]
    prl: Option<PathBuf>,
    /// Do not power on the station at startup (type `on` when ready).
    #[arg(long)]
    no_auto_power_on: bool,
    /// Override the mobile ESN (decimal or 0x-prefixed hex).
    #[arg(long, value_parser = crate::parse_u32_auto)]
    esn: Option<u32>,
    /// Override the provisioned 15-digit IMSI.
    #[arg(long, value_parser = crate::parse_imsi)]
    imsi: Option<String>,
    /// Launch the graphical handset alongside the TUI.
    #[cfg(feature = "gui")]
    #[arg(long)]
    gui: bool,
}

const BG: Color = Color::Rgb(0x28, 0x2a, 0x36);
const FG: Color = Color::Rgb(0xf8, 0xf8, 0xf2);
const COMMENT: Color = Color::Rgb(0x62, 0x72, 0xa4);
const CYAN: Color = Color::Rgb(0x8b, 0xe9, 0xfd);
const GREEN: Color = Color::Rgb(0x50, 0xfa, 0x7b);
const ORANGE: Color = Color::Rgb(0xff, 0xb8, 0x6c);
const PURPLE: Color = Color::Rgb(0xbd, 0x93, 0xf9);
const RED: Color = Color::Rgb(0xff, 0x55, 0x55);
const LINE: Color = Color::Rgb(0x44, 0x47, 0x5a);
const BAR_BG: Color = Color::Rgb(0x19, 0x1a, 0x21);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Shell,
    Stats,
    Logs,
}

struct LogWriter(mpsc::UnboundedSender<Vec<u8>>);
impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.0.send(buf.to_vec());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[derive(Clone)]
struct MakeLogWriter(mpsc::UnboundedSender<Vec<u8>>);
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for MakeLogWriter {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> LogWriter {
        LogWriter(self.0.clone())
    }
}

struct App {
    pending: Vec<Line<'static>>,
    logs: VecDeque<Line<'static>>,
    log_partial: String,
    log_scroll: u16,
    input: String,
    history: Vec<String>,
    history_index: Option<usize>,
    history_draft: String,
    diag: Option<Diagnostics>,
    mode: Mode,
    prl_path: Option<String>,
    esn: u32,
    imsi: String,
    quit_armed: bool,
    clear_screen: bool,
    should_quit: bool,
}

const MAX_LINES: usize = 4000;
const TUI_HELP: &str = "\
command                       usage
  on                          power on and scan the loaded PRL
  off                         power off the station
  status                      show serving system, radio, and protocol state
  register                    request an immediate power-up registration
  pilot                       show pilot lock, Ec/Io, and receive power
  sync                        show the decoded Sync Channel Message
  overhead [spm|apm|espm|cclm|nlm|enlm]
                              show received overhead or one message in detail
  paging                      show paging decode counters
  stats                       show sample, overflow, and uptime counters
  scan [camp|survey]          scan the loaded PRL and optionally camp
  sweep                       survey every channel without camping
  tune <bc> <channel>         acquire one channel, e.g. tune bc0 384
  prl <path>                  select the PRL used by later scans
  gain <db>                   set receive gain, e.g. gain 42
  mosms <destination> <text>  originate an SMS, e.g. mosms 555105 hello
  call <digits> [codec]       originate voice using tia96, evrc-a, evrc-b,
                              evrc-wb, or qcelp-13k (default evrc-a)
  answer                      answer a ringing mobile-terminated call
  dtmf <digits>               send a DTMF burst during a voice call (0–9, *, #)
  hangup                      release the active traffic channel
  clear                       clear shell scrollback
  help                        show this command reference
  quit                        exit the TUI

Use Up/Down to recall commands.";

impl App {
    fn record_command(&mut self, command: &str) {
        if self.history.last().is_none_or(|last| last != command) {
            self.history.push(command.to_string());
        }
        self.history_index = None;
        self.history_draft.clear();
    }

    fn previous_command(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_draft = self.input.clone();
                self.history.len() - 1
            }
        };
        self.history_index = Some(index);
        self.input.clone_from(&self.history[index]);
    }

    fn next_command(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            self.history_index = Some(index + 1);
            self.input.clone_from(&self.history[index + 1]);
        } else {
            self.history_index = None;
            self.input.clone_from(&self.history_draft);
        }
    }

    fn push(&mut self, line: Line<'static>) {
        self.pending.push(line);
        while self.pending.len() > MAX_LINES {
            self.pending.remove(0);
        }
    }
    fn push_log_bytes(&mut self, bytes: &[u8]) {
        self.log_partial.push_str(&String::from_utf8_lossy(bytes));
        while let Some(nl) = self.log_partial.find('\n') {
            let line: String = self.log_partial.drain(..=nl).collect();
            let line = line.trim_end().to_string();
            if line.is_empty() {
                continue;
            }
            let color = if line.contains("ERROR") {
                RED
            } else if line.contains(" WARN") {
                ORANGE
            } else if line.contains(" INFO") {
                FG
            } else {
                COMMENT
            };
            self.logs
                .push_back(Line::from(Span::styled(line, Style::default().fg(color))));
            while self.logs.len() > MAX_LINES {
                self.logs.pop_front();
            }
        }
    }
    fn note(&mut self, text: impl Into<String>, color: Color) {
        self.push(Line::from(Span::styled(
            format!("  {}", text.into()),
            Style::default().fg(color),
        )));
    }
    fn echo_cmd(&mut self, cmd: &str) {
        self.push(Line::from(vec![
            Span::styled("ms> ", Style::default().fg(GREEN)),
            Span::styled(cmd.to_string(), Style::default().fg(FG)),
        ]));
    }
}

pub async fn run(args: TuiArgs) -> Result<(), Error> {
    let mut config = MsNodeConfig::load(&args.config)?;
    if let Some(esn) = args.esn {
        config.identity.esn = esn;
    }
    if let Some(imsi) = args.imsi {
        config.identity.imsi = imsi;
    }
    let (log_tx, mut log_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut stdio = capture_stdio(log_tx.clone())?;
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(true)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(MakeLogWriter(log_tx))
        .try_init();

    let session = Session::open(
        args.connect.as_deref(),
        &config,
        args.radio.as_deref(),
        args.prl.as_deref(),
    )
    .await?;
    #[cfg(feature = "gui")]
    let _gui = args
        .gui
        .then(|| crate::gui::spawn_companion(&session.addr))
        .transpose()?;
    let client = session.client.clone();

    let (line_tx, mut line_rx) = mpsc::unbounded_channel::<Line<'static>>();
    {
        let mut ev_client = session.client.clone();
        let tx = line_tx.clone();
        tokio::spawn(async move {
            let Ok(stream) = ev_client.stream_events(()).await else {
                let _ = tx.send(Line::from(Span::styled(
                    "  (event stream unavailable)",
                    Style::default().fg(RED),
                )));
                return;
            };
            let mut stream = stream.into_inner();
            while let Some(Ok(ev)) = stream.next().await {
                let line = match decode_event(&ev) {
                    Some(typed) => match transcript_event_line(&typed) {
                        Some(line) => line,
                        None => continue,
                    },
                    None => Line::from(Span::styled(
                        format!("  {} {}", ev.event_type, ev.detail),
                        Style::default().fg(COMMENT),
                    )),
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let (resize_tx, mut resize_rx) = mpsc::unbounded_channel::<()>();
    std::thread::spawn(move || {
        loop {
            if !event::poll(Duration::from_millis(200)).unwrap_or(false) {
                continue;
            }
            match event::read() {
                Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                    if key_tx.send(k).is_err() {
                        break;
                    }
                }
                Ok(Event::Resize(_, _)) => {
                    if resize_tx.send(()).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let mut app = App {
        pending: Vec::new(),
        logs: VecDeque::new(),
        log_partial: String::new(),
        log_scroll: 0,
        input: String::new(),
        history: Vec::new(),
        history_index: None,
        history_draft: String::new(),
        diag: None,
        mode: Mode::Shell,
        prl_path: args.prl.as_ref().map(|p| p.display().to_string()),
        esn: config.identity.esn,
        imsi: config.identity.imsi.clone(),
        quit_armed: false,
        clear_screen: false,
        should_quit: false,
    };
    app.note(
        format!(
            "cdma-ms tui — {} {}",
            if session.embedded {
                "embedded daemon at"
            } else {
                "connected to"
            },
            session.addr
        ),
        PURPLE,
    );
    app.note(
        "M-t stats · M-g logs · M-s scan · M-p power · type a command · ^C quit",
        COMMENT,
    );

    if session.embedded && !args.no_auto_power_on {
        match session.client.clone().power_on(PowerOnRequest {}).await {
            Ok(_) => app.note("powered on", COMMENT),
            Err(e) => app.note(format!("power on failed: {}", e.message()), RED),
        }
    }

    setup_terminal(&stdio)?;
    let mut panel: Option<Terminal<CrosstermBackend<std::fs::File>>> = None;
    let mut current = Mode::Shell;
    let mut live = LiveState::default();
    let mut last_sig: Option<String> = None;
    let mut poll = tokio::time::interval(Duration::from_millis(400));

    let mut client = client;
    let loop_result: Result<(), Error> = async {
        while !app.should_quit {
            if app.mode != current {
                let mut out = stdio.stdout.try_clone()?;
                match app.mode {
                    Mode::Shell => {
                        panel = None;
                        out.execute(LeaveAlternateScreen)?;
                        // The panel leaves a scrolling region behind. With one
                        // set, a newline on the last row scrolls only inside it,
                        // so the live region lands a row short of the bottom and
                        // the erase misses its top row.
                        out.execute(Print(RESET_SCROLL_REGION))?;
                    }
                    Mode::Stats | Mode::Logs => {
                        out.execute(EnterAlternateScreen)?;
                        panel = Some(Terminal::new(CrosstermBackend::new(
                            stdio.stdout.try_clone()?,
                        ))?);
                    }
                }
                current = app.mode;
            }

            match current {
                Mode::Shell => {
                    let (term_w, term_h) = stdio.size();
                    let term_w = term_w.max(1);
                    let term_h = term_h.max(1);
                    let mut out = stdio.stdout.try_clone()?;
                    if app.clear_screen {
                        queue!(
                            out,
                            Clear(ClearType::All),
                            Clear(ClearType::Purge),
                            MoveTo(0, term_h.saturating_sub(1))
                        )?;
                        live = LiveState::default();
                        last_sig = None;
                        app.clear_screen = false;
                    }
                    let lines = shell_lines(&app, term_w);
                    let sig = shell_sig(term_w, term_h, &lines);
                    if !app.pending.is_empty() || last_sig.as_deref() != Some(sig.as_str()) {
                        live = paint_shell(&mut out, &mut app, term_w, term_h, &live, &lines)?;
                        last_sig = Some(sig);
                    }
                }
                Mode::Stats => {
                    if let Some(p) = panel.as_mut() {
                        p.draw(|f| render_stats(f, &app))?;
                    }
                }
                Mode::Logs => {
                    if let Some(p) = panel.as_mut() {
                        p.draw(|f| render_logs(f, &app))?;
                    }
                }
            }

            tokio::select! {
                _ = poll.tick() => {
                    if let Ok(d) = client.get_diagnostics(()).await {
                        app.diag = Some(d.into_inner());
                    }
                }
                Some(line) = line_rx.recv() => {
                    app.push(line);
                    while let Ok(l) = line_rx.try_recv() { app.push(l); }
                }
                Some(bytes) = log_rx.recv() => {
                    app.push_log_bytes(&bytes);
                    while let Ok(b) = log_rx.try_recv() { app.push_log_bytes(&b); }
                }
                Some(()) = resize_rx.recv() => {
                    while resize_rx.try_recv().is_ok() {}
                }
                Some(k) = key_rx.recv() => {
                    handle_key(&mut app, &mut client, k).await;
                }
            }
        }
        Ok::<(), Error>(())
    }
    .await;

    if panel.is_some() {
        let _ = (&stdio.stdout).execute(LeaveAlternateScreen);
    }
    restore_terminal(&mut stdio)?;
    loop_result
}

#[derive(Clone, Default)]
struct LiveState {
    widths: Vec<u16>,
    lines: Vec<String>,
    top: u16,
    term_width: u16,
    term_height: u16,
}

fn trace_paint(
    term_w: u16,
    term_h: u16,
    top: u16,
    path: &str,
    committed: usize,
    scrolled: u16,
    line_text: &[String],
) {
    let Ok(path_env) = std::env::var("MS_TUI_TRACE") else {
        return;
    };
    use std::io::Write as _;
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path_env)
    else {
        return;
    };
    let _ = writeln!(
        f,
        "paint {path} term={term_w}x{term_h} top={top} committed={committed} scrolled={scrolled}"
    );
    for (i, t) in line_text.iter().enumerate() {
        let _ = writeln!(f, "  row {} <- {:?}", top + i as u16, t);
    }
}

fn paint_shell(
    out: &mut impl Write,
    app: &mut App,
    term_w: u16,
    term_h: u16,
    prev: &LiveState,
    lines: &[Line<'static>],
) -> Result<LiveState, Error> {
    let lh = lines.len() as u16;
    let top = term_h.saturating_sub(lh);
    let committed: Vec<Line<'static>> = app.pending.drain(..).collect();
    let line_text: Vec<String> = lines.iter().map(plain_line).collect();
    let update_in_place = committed.is_empty()
        && prev.term_width == term_w
        && prev.term_height == term_h
        && prev.top == top
        && prev.widths.len() == lines.len()
        && prev.lines.len() == lines.len();

    if update_in_place {
        let mut widths = prev.widths.clone();
        for (i, line) in lines.iter().enumerate() {
            if prev.lines[i] == line_text[i] {
                continue;
            }
            queue!(out, MoveTo(0, top + i as u16))?;
            widths[i] = emit_bar_line(out, line, term_w, [LINE, BG, BAR_BG, BAR_BG][i])?;
        }
        out.flush()?;
        trace_paint(term_w, term_h, top, "in_place", 0, 0, &line_text);
        return Ok(LiveState {
            widths,
            lines: line_text,
            top,
            term_width: term_w,
            term_height: term_h,
        });
    }

    // Erase only when the live region moves to avoid flicker. Explicit scrolling avoids dependence on DECSTBM.
    let scrolled: u16 = committed
        .iter()
        .map(|line| committed_rows(line, term_w))
        .sum::<u16>()
        .min(top);
    if scrolled > 0 {
        queue!(out, ScrollUp(scrolled))?;
    }
    let moved = prev.term_width != term_w || prev.term_height != term_h || prev.top != top;
    if !prev.widths.is_empty() && moved {
        let erase_from = prev.top.min(term_h.saturating_sub(1));
        queue!(out, MoveTo(0, erase_from), Clear(ClearType::FromCursorDown))?;
    }

    let mut row = top.saturating_sub(scrolled);
    for line in &committed {
        queue!(out, MoveTo(0, row))?;
        emit_committed_line(out, line)?;
        row = row.saturating_add(committed_rows(line, term_w));
    }

    let bgs = [LINE, BG, BAR_BG, BAR_BG];
    let mut widths = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        queue!(out, MoveTo(0, top + i as u16))?;
        widths.push(emit_bar_line(out, line, term_w, bgs[i])?);
    }
    out.flush()?;
    trace_paint(
        term_w,
        term_h,
        top,
        "full",
        committed.len(),
        scrolled,
        &line_text,
    );
    Ok(LiveState {
        widths,
        lines: line_text,
        top,
        term_width: term_w,
        term_height: term_h,
    })
}

fn plain_line(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn emit_bar_line(out: &mut impl Write, line: &Line, width: u16, bar_bg: Color) -> io::Result<u16> {
    use unicode_width::UnicodeWidthChar;
    let cap = width.saturating_sub(1);
    queue!(out, MoveToColumn(0))?;
    let mut col: u16 = 0;
    'spans: for span in &line.spans {
        let fg = ct_color(span.style.fg.unwrap_or(Color::Reset));
        let bg = ct_color(span.style.bg.unwrap_or(bar_bg));
        queue!(out, SetForegroundColor(fg), SetBackgroundColor(bg))?;
        for ch in span.content.chars() {
            // Count ambiguous glyphs as two columns to prevent a wrapped live row from surviving redraw.
            let w = UnicodeWidthChar::width_cjk(ch).unwrap_or(0) as u16;
            if col + w > cap {
                break 'spans;
            }
            queue!(out, Print(ch))?;
            col += w;
        }
    }
    queue!(
        out,
        SetForegroundColor(CtColor::Reset),
        SetBackgroundColor(ct_color(bar_bg)),
        Clear(ClearType::UntilNewLine),
        ResetColor
    )?;
    Ok(col)
}

fn emit_committed_line(out: &mut impl Write, line: &Line) -> io::Result<()> {
    queue!(out, MoveToColumn(0))?;
    for span in &line.spans {
        queue!(
            out,
            SetForegroundColor(ct_color(span.style.fg.unwrap_or(Color::Reset))),
            SetBackgroundColor(ct_color(span.style.bg.unwrap_or(Color::Reset))),
            Print(span.content.as_ref())
        )?;
    }
    queue!(out, ResetColor, Clear(ClearType::UntilNewLine))?;
    Ok(())
}

fn committed_rows(line: &Line, term_w: u16) -> u16 {
    use unicode_width::UnicodeWidthChar;
    let width: usize = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars())
        .map(|c| UnicodeWidthChar::width_cjk(c).unwrap_or(0))
        .sum();
    ((width as u16).max(1)).div_ceil(term_w.max(1)).max(1)
}

fn ct_color(c: Color) -> CtColor {
    match c {
        Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        _ => CtColor::Reset,
    }
}

fn shell_sig(term_w: u16, term_h: u16, lines: &[Line<'static>]) -> String {
    let mut s = format!("{term_w}x{term_h}");
    for line in lines {
        s.push('|');
        for span in &line.spans {
            s.push_str(&span.content);
        }
    }
    s
}

fn shell_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    vec![
        rule_line(),
        input_line(app),
        status_line(app, width),
        keybar_line(app, width),
    ]
}

fn rule_line() -> Line<'static> {
    Line::from(Vec::new())
}

fn input_line(app: &App) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "ms> ",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.input.clone(), Style::default().fg(FG)),
        Span::styled("█", Style::default().fg(FG)),
    ])
}

async fn handle_key(app: &mut App, client: &mut MsServiceClient<Channel>, k: KeyEvent) {
    use KeyModifiers as M;
    let quit_armed = app.quit_armed;
    app.quit_armed = false;

    if let (KeyCode::Char('c'), M::CONTROL) = (k.code, k.modifiers) {
        if !app.input.is_empty() {
            app.input.clear();
        } else if quit_armed {
            app.should_quit = true;
        } else {
            app.quit_armed = true;
        }
        return;
    }

    match (k.code, k.modifiers) {
        (KeyCode::Char('t'), M::ALT) => {
            app.mode = if app.mode == Mode::Stats {
                Mode::Shell
            } else {
                Mode::Stats
            };
            return;
        }
        (KeyCode::Char('g'), M::ALT) => {
            app.mode = if app.mode == Mode::Logs {
                Mode::Shell
            } else {
                Mode::Logs
            };
            app.log_scroll = 0;
            return;
        }
        (KeyCode::Esc, _) if app.mode != Mode::Shell => {
            app.mode = Mode::Shell;
            return;
        }
        _ => {}
    }

    if app.mode != Mode::Shell {
        match k.code {
            KeyCode::PageUp => app.log_scroll = app.log_scroll.saturating_add(6),
            KeyCode::PageDown => app.log_scroll = app.log_scroll.saturating_sub(6),
            _ => {}
        }
        return;
    }

    match (k.code, k.modifiers) {
        (KeyCode::Char('s'), M::ALT) => run_command(app, client, "scan camp").await,
        (KeyCode::Char('p'), M::ALT) => {
            let powered = app
                .diag
                .as_ref()
                .map(|d| d.state() != MsState::Off)
                .unwrap_or(false);
            run_command(app, client, if powered { "off" } else { "on" }).await;
        }

        (KeyCode::Char('l'), M::CONTROL) => app.clear_screen = true,
        (KeyCode::Char('u'), M::CONTROL) => app.input.clear(),
        (KeyCode::Char('w'), M::CONTROL) => {
            let cut = app
                .input
                .trim_end()
                .rfind(char::is_whitespace)
                .map(|i| i + 1)
                .unwrap_or(0);
            app.input.truncate(cut);
        }

        (KeyCode::Enter, _) => {
            let cmd = app.input.trim().to_string();
            app.input.clear();
            if !cmd.is_empty() {
                app.record_command(&cmd);
                run_command(app, client, &cmd).await;
            }
        }
        (KeyCode::Up, _) => app.previous_command(),
        (KeyCode::Down, _) => app.next_command(),
        (KeyCode::Backspace, _) => {
            app.input.pop();
        }
        (KeyCode::Char(c), m) if !m.contains(M::CONTROL) && !m.contains(M::ALT) => {
            app.input.push(c)
        }
        _ => {}
    }
}

async fn run_command(app: &mut App, client: &mut MsServiceClient<Channel>, cmd: &str) {
    app.echo_cmd(cmd);
    let mut words = cmd.split_whitespace();
    let head = words.next().unwrap_or("");
    let rest: Vec<&str> = words.collect();
    let result: Result<String, String> = match head {
        "on" => client
            .power_on(PowerOnRequest {})
            .await
            .map(|r| r.into_inner().message)
            .map_err(|e| e.message().to_string()),
        "off" => client
            .power_off(())
            .await
            .map(|_| "powered off".into())
            .map_err(|e| e.message().to_string()),
        "register" | "reg" => client
            .register(())
            .await
            .map(|_| "registration requested".into())
            .map_err(|e| e.message().to_string()),
        "clear" => {
            app.clear_screen = true;
            Ok(String::new())
        }
        "quit" | "exit" => {
            app.should_quit = true;
            Ok(String::new())
        }
        "help" => Ok(TUI_HELP.into()),
        "status" | "pilot" | "sync" | "overhead" | "paging" | "stats" => {
            match client.get_diagnostics(()).await {
                Ok(r) => {
                    let d = r.into_inner();
                    app.diag = Some(d.clone());
                    Ok(diag_text(&d, head, rest.first().copied()))
                }
                Err(e) => Err(e.message().to_string()),
            }
        }
        "scan" | "sweep" => {
            let mode = if head == "sweep" {
                "survey"
            } else {
                rest.first().copied().unwrap_or("camp")
            };
            match (&app.prl_path, ScanMode::from_label(mode)) {
                (_, None) => Err(format!("scan mode '{mode}' is not camp or survey")),
                (None, _) => Err("no PRL loaded (start with --prl or: prl <path>)".into()),
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
            _ => Err("usage: tune <bc0|bc1|...> <channel>".into()),
        },
        "prl" => match rest.as_slice() {
            [path] => {
                app.prl_path = Some(path.to_string());
                Ok(format!("PRL for the next scan: {path}"))
            }
            _ => Err("usage: prl <path>".into()),
        },
        "gain" => match rest.first().and_then(|g| g.parse::<f64>().ok()) {
            Some(gain_db) => client
                .set_rx_gain(SetRxGainRequest { gain_db })
                .await
                .map(|_| format!("RX gain {gain_db:.1} dB"))
                .map_err(|e| e.message().to_string()),
            None => Err("usage: gain <db>".into()),
        },
        "mosms" => {
            if rest.len() < 2 {
                Err("usage: mosms <dest> <text>".into())
            } else {
                let dest = rest[0].to_string();
                let text = rest[1..].join(" ");
                client
                    .send_sms(SendSmsRequest {
                        destination: dest.clone(),
                        text,
                    })
                    .await
                    .map(|r| r.into_inner().message)
                    .map_err(|e| e.message().to_string())
            }
        }
        "call" => match rest.as_slice() {
            [digits] | [digits, _] => {
                match rest
                    .get(1)
                    .map(|codec| crate::cli_radio::parse_voice_codec(codec))
                    .unwrap_or(Some(cdma_voice::SERVICE_OPTION_EVRC_A))
                {
                    Some(service_option) => client
                        .originate(OriginateRequest {
                            service_option: u32::from(service_option),
                            dialed_digits: (*digits).to_string(),
                        })
                        .await
                        .map(|r| r.into_inner().message)
                        .map_err(|e| e.message().to_string()),
                    None => Err(
                        "unknown codec (use tia96, evrc-a, evrc-b, evrc-wb, or qcelp-13k)"
                            .to_string(),
                    ),
                }
            }
            _ => Err("usage: call <digits> [codec]".into()),
        },
        "answer" => client
            .answer(())
            .await
            .map(|_| "answered incoming call".into())
            .map_err(|e| e.message().to_string()),
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
            .map(|_| "releasing traffic channel".into())
            .map_err(|e| e.message().to_string()),
        "" => Ok(String::new()),
        other => Err(format!("unknown command: {other} (try: help)")),
    };
    match result {
        Ok(s) if s.is_empty() => {}
        Ok(s) => {
            for l in s.lines() {
                app.note(l.to_string(), FG);
            }
        }
        Err(e) => app.note(e, RED),
    }
}

fn diag_text(d: &Diagnostics, cmd: &str, arg: Option<&str>) -> String {
    use cli_radio::show;
    match cmd {
        "status" => show::status(d),
        "pilot" => d.pilot.as_ref().map(show::pilot).unwrap_or_default(),
        "sync" => d
            .sync
            .as_ref()
            .map(show::sync)
            .unwrap_or_else(|| "sync: not decoded".into()),
        "overhead" => d
            .overhead
            .as_ref()
            .map(|o| show::overhead(o, arg))
            .unwrap_or_default(),
        "paging" => d.paging.as_ref().map(show::paging).unwrap_or_default(),
        "stats" => d.radio.as_ref().map(show::radio).unwrap_or_default(),
        _ => String::new(),
    }
}

fn base_id(d: Option<&Diagnostics>) -> Option<u64> {
    let o = d?.overhead.as_ref()?;
    let v: serde_json::Value = serde_json::from_str(&o.system_parameters).ok()?;
    v.get("base_id")?.as_u64()
}

fn spans_w(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

fn fit_bar(
    cap: usize,
    head: Vec<Span<'static>>,
    segs: Vec<(u8, Vec<Span<'static>>)>,
) -> Vec<Span<'static>> {
    let mut budget = cap.saturating_sub(spans_w(&head));
    let mut order: Vec<usize> = (0..segs.len()).collect();
    order.sort_by_key(|&i| segs[i].0);
    let mut keep = vec![false; segs.len()];
    for i in order {
        let w = spans_w(&segs[i].1);
        if w <= budget {
            keep[i] = true;
            budget -= w;
        }
    }
    let mut spans = head;
    for (i, (_, seg_spans)) in segs.into_iter().enumerate() {
        if keep[i] {
            spans.extend(seg_spans);
        }
    }
    spans
}

const STATE_WIDTH: usize = 8;

fn status_line(app: &App, width: u16) -> Line<'static> {
    let d = app.diag.as_ref();

    let state = d.map(|d| d.state());
    let pill_bg = match state {
        Some(MsState::TrafficChannelInit | MsState::TrafficChannel) => GREEN,
        Some(MsState::SystemAccess) => PURPLE,
        Some(MsState::PilotAcquisition | MsState::SyncAcquisition) => ORANGE,
        Some(MsState::Off) | None => COMMENT,
        Some(_) => CYAN,
    };
    let pilot = d.and_then(|d| d.pilot.as_ref()).filter(|p| p.measured);
    let sync = d.and_then(|d| d.sync.as_ref());

    let head = vec![
        Span::styled(
            format!(" {:^STATE_WIDTH$} ", state_label(state)),
            Style::default()
                .fg(BG)
                .bg(pill_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        signal_span(pilot.map(|p| p.ec_io_db).unwrap_or(-100.0)),
    ];

    let (reg_txt, reg_col) = match d.map(|d| d.registered) {
        Some(true) => ("REG ok", GREEN),
        Some(false) => ("REG no", RED),
        None => ("REG --", COMMENT),
    };

    let segs = vec![
        (
            8,
            vec![Span::styled(
                match pilot {
                    Some(p) => format!(" {:>4.0}dBFS", p.rx_power_dbfs),
                    None => "   --dBFS".into(),
                },
                Style::default().fg(FG),
            )],
        ),
        (
            4,
            vec![
                sep(),
                seg(
                    "Ec/Io",
                    &pilot
                        .map(|p| format!("{:.1}", p.ec_io_db))
                        .unwrap_or_else(|| "--".into()),
                    6,
                    pilot.map(|p| ecio_color(p.ec_io_db)).unwrap_or(COMMENT),
                ),
            ],
        ),
        (
            3,
            vec![
                sep(),
                seg(
                    "SID",
                    &sync
                        .map(|s| s.sid.to_string())
                        .unwrap_or_else(|| "--".into()),
                    5,
                    FG,
                ),
            ],
        ),
        (
            6,
            vec![
                sep(),
                seg(
                    "BASE",
                    &base_id(d)
                        .map(|b| b.to_string())
                        .unwrap_or_else(|| "--".into()),
                    5,
                    FG,
                ),
            ],
        ),
        (
            5,
            vec![
                sep(),
                seg(
                    "PN",
                    &sync
                        .map(|s| s.pilot_pn.to_string())
                        .unwrap_or_else(|| "--".into()),
                    3,
                    FG,
                ),
            ],
        ),
        (
            7,
            vec![
                sep(),
                seg(
                    "chan",
                    &sync
                        .map(|s| format!("ch{}", s.cdma_freq))
                        .unwrap_or_else(|| "ch--".into()),
                    6,
                    CYAN,
                ),
            ],
        ),
        (
            2,
            vec![
                sep(),
                Span::styled(
                    format!(" {reg_txt} "),
                    Style::default().fg(reg_col).add_modifier(Modifier::BOLD),
                ),
            ],
        ),
    ];

    Line::from(fit_bar(width.saturating_sub(1) as usize, head, segs))
}

fn keybar_line(app: &App, width: u16) -> Line<'static> {
    if app.quit_armed {
        return Line::from(Span::styled(
            " press Ctrl-C again to quit — any other key cancels ",
            Style::default().fg(BG).bg(RED).add_modifier(Modifier::BOLD),
        ));
    }
    let key = |k: &str, label: &str, hot: bool| {
        let kc = if hot { PURPLE } else { COMMENT };
        vec![
            Span::styled(
                format!(" {k} "),
                Style::default().fg(BG).bg(kc).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {label}  "), Style::default().fg(kc)),
        ]
    };
    let cap = width.saturating_sub(1) as usize;
    let segs = vec![
        (2, key("M-t", "stats", true)),
        (3, key("M-g", "logs", false)),
        (4, key("↑↓", "history", false)),
        (5, key("M-s", "scan", false)),
        (6, key("M-p", "power", false)),
        (4, key("^L", "clear", false)),
        (1, key("^C", "quit", false)),
    ];
    Line::from(fit_bar(cap, Vec::new(), segs))
}

fn render_stats(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let rows = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    panel_title(f, rows[0], "STATS", "Esc / M-t back");

    let cols = RLayout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    let d = app.diag.as_ref();

    let mut left: Vec<Line> = Vec::new();
    left.push(group("SIGNAL"));
    match d.and_then(|d| d.pilot.as_ref()).filter(|p| p.measured) {
        Some(p) => {
            left.push(kv_signal(p.ec_io_db));
            left.push(kv("rx level", &format!("{:.1} dBFS", p.rx_power_dbfs)));
        }
        None => {
            left.push(kv_style("pilot", "not measured", COMMENT));
            left.push(kv_style("acq SNR", "--", COMMENT));
            left.push(kv_style("carrier", "--", COMMENT));
            left.push(kv_style("rx level", "--", COMMENT));
        }
    }
    left.push(Line::raw(""));
    left.push(group("SERVING"));
    match d.and_then(|d| d.sync.as_ref()) {
        Some(s) => {
            left.push(kv_c("SID / NID", &format!("{} · {}", s.sid, s.nid)));
            left.push(kv(
                "BASE_ID",
                &base_id(d)
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "--".into()),
            ));
            left.push(kv("PILOT_PN", &s.pilot_pn.to_string()));
            left.push(kv("P_REV", &s.p_rev.to_string()));
            left.push(kv("CDMA_FREQ", &format!("ch{}", s.cdma_freq)));
        }
        None => {
            for k in ["SID / NID", "BASE_ID", "PILOT_PN", "P_REV", "CDMA_FREQ"] {
                left.push(kv_style(k, "--", COMMENT));
            }
        }
    }

    let mut right: Vec<Line> = Vec::new();
    right.push(group("STATE"));
    right.push(kv_style(
        "station",
        d.map(|d| d.state().label()).unwrap_or("offline"),
        GREEN,
    ));
    let (reg_txt, reg_col) = match d.map(|d| d.registered) {
        Some(true) => ("registered", GREEN),
        Some(false) => ("not registered", ORANGE),
        None => ("--", COMMENT),
    };
    right.push(kv_style("register", reg_txt, reg_col));
    match d.and_then(|d| d.paging.as_ref()) {
        Some(pg) => {
            right.push(kv_style(
                "paging",
                &format!("{} ok · {} bad", pg.crc_valid, pg.crc_failed),
                GREEN,
            ));
            right.push(kv("for me", &pg.pages_for_me.to_string()));
        }
        None => {
            right.push(kv_style("paging", "--", COMMENT));
            right.push(kv_style("for me", "--", COMMENT));
        }
    }
    right.push(Line::raw(""));
    right.push(group("RADIO"));
    match d.and_then(|d| d.radio.as_ref()) {
        Some(r) => {
            right.push(kv_style("overflows", &r.rx_overflows.to_string(), GREEN));
            right.push(kv("uptime", &format!("{:.0} s", r.uptime_secs)));
        }
        None => {
            right.push(kv_style("overflows", "--", COMMENT));
            right.push(kv_style("uptime", "--", COMMENT));
        }
    }
    right.push(Line::raw(""));
    right.push(group("IDENTITY"));
    right.push(kv("ESN", &format!("0x{:08X}", app.esn)));
    right.push(kv("IMSI", &app.imsi));
    if let Some(prl) = &app.prl_path {
        right.push(kv("PRL", prl));
    }

    let pad = |r: Rect| Rect {
        x: r.x + 2,
        width: r.width.saturating_sub(3),
        y: r.y + 1,
        height: r.height.saturating_sub(1),
    };
    f.render_widget(
        Paragraph::new(left).wrap(Wrap { trim: false }),
        pad(cols[0]),
    );
    f.render_widget(
        Paragraph::new(right).wrap(Wrap { trim: false }),
        pad(cols[1]),
    );
}

fn render_logs(f: &mut ratatui::Frame, app: &App) {
    let area = f.area();
    f.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let rows = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    panel_title(f, rows[0], "LOGS", "PgUp/PgDn scroll · Esc / M-g back");

    let body = Rect {
        x: rows[1].x + 1,
        width: rows[1].width.saturating_sub(1),
        ..rows[1]
    };
    let total = app.logs.len() as u16;
    let max_scroll = total.saturating_sub(body.height);
    let scroll = max_scroll.saturating_sub(app.log_scroll.min(max_scroll));
    let lines: Vec<Line> = if app.logs.is_empty() {
        vec![Line::from(Span::styled(
            "  (no logs captured yet)",
            Style::default().fg(COMMENT),
        ))]
    } else {
        app.logs.iter().cloned().collect()
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        body,
    );
}

fn panel_title(f: &mut ratatui::Frame, area: Rect, title: &str, hint: &str) {
    let used = title.len() + 2;
    let pad = (area.width as usize).saturating_sub(used + hint.len() + 1);
    let line = Line::from(vec![
        Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(BG)
                .bg(PURPLE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".repeat(pad)),
        Span::styled(format!("{hint} "), Style::default().fg(COMMENT)),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::default().bg(BAR_BG)),
        area,
    );
}

fn group(name: &str) -> Line<'static> {
    Line::from(Span::styled(
        name.to_string(),
        Style::default().fg(COMMENT).add_modifier(Modifier::BOLD),
    ))
}
fn kv(k: &str, v: &str) -> Line<'static> {
    kv_style(k, v, FG)
}
fn kv_c(k: &str, v: &str) -> Line<'static> {
    kv_style(k, v, CYAN)
}
fn kv_style(k: &str, v: &str, vc: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<11}"), Style::default().fg(COMMENT)),
        Span::styled(v.to_string(), Style::default().fg(vc)),
    ])
}
fn kv_signal(ec_io: f64) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<11}", "pilot"), Style::default().fg(COMMENT)),
        signal_span(ec_io),
        Span::styled(
            format!(" {ec_io:.1} dB"),
            Style::default().fg(ecio_color(ec_io)),
        ),
    ])
}
fn ecio_color(ec_io: f64) -> Color {
    if ec_io >= -9.0 {
        GREEN
    } else if ec_io >= -13.0 {
        ORANGE
    } else {
        RED
    }
}
fn signal_span(ec_io: f64) -> Span<'static> {
    let level = signal_level(ec_io);
    let bars: String = (0..4).map(|i| if i < level { '┃' } else { '│' }).collect();
    let col = if level >= 3 {
        GREEN
    } else if level >= 1 {
        ORANGE
    } else {
        RED
    };
    Span::styled(bars, Style::default().fg(col))
}

fn signal_level(ec_io: f64) -> usize {
    if ec_io > -8.0 {
        4
    } else if ec_io > -12.0 {
        3
    } else if ec_io > -16.0 {
        2
    } else if ec_io > -20.0 {
        1
    } else {
        0
    }
}
fn seg(label: &str, v: &str, w: usize, vc: Color) -> Span<'static> {
    Span::styled(format!(" {label} {v:>w$} "), Style::default().fg(vc))
}

fn state_label(state: Option<MsState>) -> &'static str {
    match state {
        None => "OFFLINE",
        Some(MsState::Unspecified) => "UNKNOWN",
        Some(MsState::Off) => "OFF",
        Some(MsState::SystemDetermination) => "SYNC",
        Some(MsState::PilotAcquisition) => "PILOT",
        Some(MsState::SyncAcquisition) => "SYNC",
        Some(MsState::TimingChange) => "TIMING",
        Some(MsState::Idle) => "IDLE",
        Some(MsState::SystemAccess) => "ACCESS",
        Some(MsState::TrafficChannelInit) => "TCH-INIT",
        Some(MsState::TrafficChannel) => "TRAFFIC",
    }
}
fn sep() -> Span<'static> {
    Span::styled("│", Style::default().fg(LINE))
}

fn event_line(ev: &cdma_ms::ms::MsEvent) -> Line<'static> {
    let text = cli_radio::event_line(ev);
    let color = if text.contains("accepted") || text.contains("locked") || text.contains("camped") {
        GREEN
    } else if text.contains("failed") || text.contains("rejected") || text.contains("lost") {
        RED
    } else if text.contains("registration") || text.contains("access") {
        ORANGE
    } else {
        COMMENT
    };
    Line::from(Span::styled(
        format!("  {text}"),
        Style::default().fg(color),
    ))
}

fn transcript_event_line(ev: &cdma_ms::ms::MsEvent) -> Option<Line<'static>> {
    (!cli_radio::is_chatty(ev)).then(|| event_line(ev))
}

struct StdioCapture {
    stdout: std::fs::File,
    stderr: std::fs::File,
    stdout_redirected: bool,
    stderr_redirected: bool,
}

impl StdioCapture {
    fn size(&self) -> (u16, u16) {
        if let Ok(size) = rustix::termios::tcgetwinsize(&self.stdout) {
            if size.ws_col > 0 && size.ws_row > 0 {
                return (size.ws_col, size.ws_row);
            }
        }
        terminal_size().unwrap_or((80, 24))
    }

    fn restore(&mut self) -> io::Result<()> {
        let stdout = if self.stdout_redirected {
            rustix::stdio::dup2_stdout(&self.stdout).map(|()| self.stdout_redirected = false)
        } else {
            Ok(())
        };
        let stderr = if self.stderr_redirected {
            rustix::stdio::dup2_stderr(&self.stderr).map(|()| self.stderr_redirected = false)
        } else {
            Ok(())
        };
        stdout.and(stderr).map_err(io::Error::from)
    }
}

impl Drop for StdioCapture {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            let _ = writeln!(&self.stderr, "could not restore terminal output: {error}");
        }
    }
}

fn capture_stdio(log_tx: mpsc::UnboundedSender<Vec<u8>>) -> io::Result<StdioCapture> {
    use std::io::Read as _;

    let mut stdio = StdioCapture {
        stdout: rustix::io::dup(std::io::stdout())?.into(),
        stderr: rustix::io::dup(std::io::stderr())?.into(),
        stdout_redirected: false,
        stderr_redirected: false,
    };
    let (reader, writer) = rustix::pipe::pipe()?;
    rustix::stdio::dup2_stdout(&writer)?;
    stdio.stdout_redirected = true;
    rustix::stdio::dup2_stderr(&writer)?;
    stdio.stderr_redirected = true;

    let mut reader = std::fs::File::from(reader);
    std::thread::Builder::new()
        .name("ms-stdio".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || log_tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        })?;
    Ok(stdio)
}

/// DECSTBM without parameters restores whole-screen scrolling.
const RESET_SCROLL_REGION: &str = "\x1b[r";

fn setup_terminal(stdio: &StdioCapture) -> Result<(), Error> {
    enable_raw_mode()?;
    let mut out = stdio.stdout.try_clone()?;
    out.execute(cursor::Hide)?;
    out.execute(Print(RESET_SCROLL_REGION))?;
    out.execute(Clear(ClearType::All))?;
    let (_, rows) = stdio.size();
    out.execute(MoveTo(0, rows.saturating_sub(1)))?;
    out.flush()?;
    Ok(())
}
fn restore_terminal(stdio: &mut StdioCapture) -> Result<(), Error> {
    let raw_mode = disable_raw_mode();
    let restore = stdio.restore();
    let mut out = &stdio.stdout;
    out.execute(Print(RESET_SCROLL_REGION))?;
    out.execute(cursor::Show)?;
    let _ = writeln!(out);
    let _ = out.flush();
    raw_mode?;
    restore?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_ms::grpc::proto::{Overhead, PagingStats, PilotStatus, RadioStats, SyncParameters};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn stdio_capture_restores_stdout_and_stderr_on_explicit_restore_and_drop() {
        const CHILD_MODE: &str = "CDMA_MS_STDIO_CAPTURE_TEST_CHILD";
        const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
        if let Ok(mode) = std::env::var(CHILD_MODE) {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut capture = capture_stdio(tx).unwrap();
            writeln!(std::io::stdout(), "captured stdout").unwrap();
            writeln!(std::io::stderr(), "captured stderr").unwrap();
            writeln!(&capture.stdout, "terminal output").unwrap();
            if mode == "explicit" {
                capture.restore().unwrap();
                capture.restore().unwrap();
            }
            drop(capture);
            writeln!(std::io::stdout(), "restored stdout").unwrap();
            writeln!(std::io::stderr(), "restored stderr").unwrap();
            let deadline = std::time::Instant::now() + CAPTURE_TIMEOUT;
            let mut captured = Vec::new();
            while std::time::Instant::now() < deadline {
                match rx.try_recv() {
                    Ok(bytes) => captured.extend(bytes),
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                    Err(mpsc::error::TryRecvError::Empty) => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            assert_eq!(captured, b"captured stdout\ncaptured stderr\n");
            return;
        }
        // Redirecting stdio affects every thread, so isolate it from the test harness.
        for mode in ["explicit", "drop"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tui::tests::stdio_capture_restores_stdout_and_stderr_on_explicit_restore_and_drop",
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{mode}: {stdout} {stderr}");
            assert!(stdout.contains("terminal output\n"));
            assert!(stdout.contains("restored stdout\n"));
            assert!(!stdout.contains("restored stderr\n"));
            assert!(stderr.contains("restored stderr\n"));
            assert!(!stderr.contains("restored stdout\n"));
        }
    }

    fn sample_app() -> App {
        App {
            pending: Vec::new(),
            logs: VecDeque::from([Line::from(
                "  12:00:00 INFO cdma_bts: tx_stats ...".to_string(),
            )]),
            log_partial: String::new(),
            log_scroll: 0,
            input: "stat".into(),
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            diag: Some(Diagnostics {
                state: MsState::Idle.into(),
                registered: false,
                pilot: Some(PilotStatus {
                    locked: true,
                    measured: true,
                    ec_io_db: -2.1,
                    rx_power_dbfs: -43.7,
                    pilot_symbols: 187392,
                }),
                sync: Some(SyncParameters {
                    sid: 4107,
                    nid: 65535,
                    pilot_pn: 510,
                    p_rev: 6,
                    cdma_freq: 50,
                    ..Default::default()
                }),
                overhead: Some(Overhead {
                    system_parameters: r#"{"sid":4107,"nid":65535,"base_id":16040}"#.into(),
                    ..Default::default()
                }),
                paging: Some(PagingStats {
                    crc_valid: 216,
                    crc_failed: 0,
                    pages_for_me: 0,
                    ..Default::default()
                }),
                scan: None,
                radio: Some(RadioStats {
                    samples_fed: 68_000_000,
                    rx_overflows: 0,
                    uptime_secs: 30.0,
                    real_time_ratio: 1.0,
                }),
            }),
            mode: Mode::Shell,
            prl_path: None,
            esn: 0x1234_5678,
            imsi: "310001234567890".to_string(),
            quit_armed: false,
            clear_screen: false,
            should_quit: false,
        }
    }

    fn shell_text(app: &App) -> String {
        let mut s = String::new();
        for line in shell_lines(app, 110) {
            for span in &line.spans {
                s.push_str(&span.content);
            }
            s.push('\n');
        }
        s
    }

    struct Vt {
        w: u16,
        h: u16,
        rows: Vec<Vec<char>>,
        scrollback: Vec<String>,
        r: u16,
        c: u16,
        stop: u16,
        sbot: u16,
    }

    impl Vt {
        fn new(w: u16, h: u16) -> Self {
            Self {
                w,
                h,
                rows: vec![vec![' '; w as usize]; h as usize],
                scrollback: Vec::new(),
                r: 0,
                c: 0,
                stop: 0,
                sbot: h - 1,
            }
        }

        fn row_text(row: &[char]) -> String {
            row.iter().collect::<String>().trim_end().to_string()
        }

        fn visible(&self) -> Vec<String> {
            self.rows.iter().map(|r| Self::row_text(r)).collect()
        }

        fn scroll(&mut self) {
            let top = self.rows.remove(self.stop as usize);
            if self.stop == 0 {
                self.scrollback.push(Self::row_text(&top));
            }
            self.rows
                .insert(self.sbot as usize, vec![' '; self.w as usize]);
        }

        fn newline(&mut self) {
            if self.r == self.sbot {
                self.scroll();
            } else if self.r + 1 < self.h {
                self.r += 1;
            }
        }

        fn put(&mut self, ch: char) {
            let cw = unicode_width::UnicodeWidthChar::width_cjk(ch).unwrap_or(0) as u16;
            if cw == 0 {
                return;
            }
            if self.c + cw > self.w {
                self.c = 0;
                self.newline();
            }
            self.rows[self.r as usize][self.c as usize] = ch;
            self.c += cw;
        }

        fn clear_to_eol(&mut self) {
            for x in self.c..self.w {
                self.rows[self.r as usize][x as usize] = ' ';
            }
        }

        fn clear_from_cursor_down(&mut self) {
            self.clear_to_eol();
            for y in (self.r + 1)..self.h {
                self.rows[y as usize] = vec![' '; self.w as usize];
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            let s = String::from_utf8_lossy(bytes).to_string();
            let mut it = s.chars().peekable();
            while let Some(ch) = it.next() {
                if ch != '\u{1b}' {
                    match ch {
                        '\r' => self.c = 0,
                        '\n' => self.newline(),
                        _ => self.put(ch),
                    }
                    continue;
                }
                if it.peek() != Some(&'[') {
                    continue;
                }
                it.next();
                let mut params = String::new();
                let mut final_byte = ' ';
                for p in it.by_ref() {
                    if p.is_ascii_alphabetic() {
                        final_byte = p;
                        break;
                    }
                    params.push(p);
                }
                let nums: Vec<u16> = params
                    .split(';')
                    .map(|p| p.parse::<u16>().unwrap_or(0))
                    .collect();
                match final_byte {
                    'H' => {
                        self.r = nums
                            .first()
                            .copied()
                            .unwrap_or(1)
                            .saturating_sub(1)
                            .min(self.h - 1);
                        self.c = nums
                            .get(1)
                            .copied()
                            .unwrap_or(1)
                            .saturating_sub(1)
                            .min(self.w - 1);
                    }
                    'G' => {
                        self.c = nums
                            .first()
                            .copied()
                            .unwrap_or(1)
                            .saturating_sub(1)
                            .min(self.w - 1)
                    }
                    'r' => {
                        if params.is_empty() {
                            self.stop = 0;
                            self.sbot = self.h - 1;
                        } else {
                            self.stop = nums.first().copied().unwrap_or(1).saturating_sub(1);
                            self.sbot = nums
                                .get(1)
                                .copied()
                                .unwrap_or(self.h)
                                .saturating_sub(1)
                                .min(self.h - 1);
                        }
                        self.r = self.stop;
                        self.c = 0;
                    }
                    'S' => {
                        let n = nums.first().copied().unwrap_or(1).max(1);
                        for _ in 0..n {
                            self.scroll();
                        }
                    }
                    'J' => self.clear_from_cursor_down(),
                    'K' => self.clear_to_eol(),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn live_rows_never_leak_into_scrollback() {
        for w in 40u16..=200 {
            for h in [10u16, 24, 40] {
                leak_check(w, h);
            }
        }
    }

    fn leak_check(w: u16, h: u16) {
        let mut vt = Vt::new(w, h);
        vt.r = h - 1;
        let mut app = sample_app();
        let mut live = LiveState::default();

        for round in 0..12 {
            app.push(Line::raw(""));
            app.push(Line::from("  state idle -> pilot_acquisition".to_string()));
            app.push(Line::from(
                "  state pilot_acquisition -> sync_acquisition".to_string(),
            ));
            app.push(Line::from("          pilot PN offset 510".to_string()));
            app.push(Line::from(format!(
                "          sync decoded: SID 4107 NID 65535 pilot_pn 510 p_rev 5 sys_time 1843106844{round} (2026-09-26T19:17:55.360Z)"
            )));
            app.push(Line::from(
                "  state sync_acquisition -> timing_change".to_string(),
            ));
            app.push(Line::from("  state timing_change -> idle".to_string()));
            app.diag.as_mut().unwrap().pilot.as_mut().unwrap().ec_io_db = -3.0 - round as f64 * 0.1;

            let lines = shell_lines(&app, w);
            let mut buf = Vec::new();
            live = paint_shell(&mut buf, &mut app, w, h, &live, &lines).unwrap();
            vt.feed(&buf);

            app.diag
                .as_mut()
                .unwrap()
                .pilot
                .as_mut()
                .unwrap()
                .rx_power_dbfs = -18.0 - round as f64 * 0.2;
            let lines = shell_lines(&app, w);
            let mut buf = Vec::new();
            live = paint_shell(&mut buf, &mut app, w, h, &live, &lines).unwrap();
            vt.feed(&buf);
        }

        let leaked: Vec<&String> = vt
            .scrollback
            .iter()
            .filter(|l| l.contains("Ec/Io") || l.contains("M-t") || l.contains("ms>"))
            .collect();
        assert!(
            leaked.is_empty(),
            "w={w} h={h}: live rows leaked into scrollback: {leaked:#?}\nvisible tail: {:#?}",
            &vt.visible()[(h as usize - 5)..]
        );
    }

    fn run_bursts(vt: &mut Vt, app: &mut App, live: &mut LiveState, w: u16, h: u16, rounds: u16) {
        for round in 0..rounds {
            app.push(Line::raw(""));
            app.push(Line::from("  state idle -> pilot_acquisition".to_string()));
            app.push(Line::from(format!("          pilot PN offset {round}")));
            app.push(Line::from("  state timing_change -> idle".to_string()));
            let lines = shell_lines(app, w);
            let mut buf = Vec::new();
            *live = paint_shell(&mut buf, app, w, h, live, &lines).unwrap();
            vt.feed(&buf);
        }
    }

    fn leaked_rows(vt: &Vt) -> Vec<String> {
        vt.scrollback
            .iter()
            .filter(|l| l.contains("Ec/Io") || l.contains("M-t") || l.contains("ms>"))
            .cloned()
            .collect()
    }

    #[test]
    fn a_leftover_scroll_region_does_not_strand_live_rows() {
        let (w, h) = (100u16, 24u16);
        for reset in [false, true] {
            let mut vt = Vt::new(w, h);
            vt.r = h - 1;
            vt.feed(format!("\x1b[1;{}r", h - 1).as_bytes());
            if reset {
                vt.feed(RESET_SCROLL_REGION.as_bytes());
                vt.r = h - 1;
            }
            let mut app = sample_app();
            let mut live = LiveState::default();
            run_bursts(&mut vt, &mut app, &mut live, w, h, 6);
            assert!(
                leaked_rows(&vt).is_empty(),
                "reset={reset}: live rows leaked: {:#?}",
                leaked_rows(&vt)
            );
        }
    }

    #[test]
    fn committing_transcript_does_not_clear_the_live_region() {
        let (w, h) = (100u16, 24u16);
        let mut app = sample_app();
        let mut live = LiveState::default();

        let lines = shell_lines(&app, w);
        let mut buf = Vec::new();
        live = paint_shell(&mut buf, &mut app, w, h, &live, &lines).unwrap();

        app.push(Line::from("  state idle -> pilot_acquisition".to_string()));
        let lines = shell_lines(&app, w);
        let mut buf = Vec::new();
        paint_shell(&mut buf, &mut app, w, h, &live, &lines).unwrap();
        assert!(
            !buf.windows(3).any(|b| b == b"\x1b[J"),
            "a transcript flush wiped the live region, which makes the bars blink"
        );
    }

    #[test]
    fn only_one_live_block_is_visible() {
        for (w, h) in [(80u16, 24u16), (100, 24), (143, 40)] {
            for region in [false, true] {
                let mut vt = Vt::new(w, h);
                vt.r = h - 1;
                if region {
                    vt.feed(format!("\x1b[1;{}r", h - 1).as_bytes());
                }
                let mut app = sample_app();
                let mut live = LiveState::default();
                run_bursts(&mut vt, &mut app, &mut live, w, h, 8);

                let visible = vt.visible();
                let statuses = visible.iter().filter(|l| l.contains("Ec/Io")).count();
                let keybars = visible.iter().filter(|l| l.contains("M-t")).count();
                assert_eq!(
                    statuses,
                    1,
                    "w={w} h={h} region={region}: expected one status row, saw {statuses}: {:#?}",
                    &visible[(h as usize - 6)..]
                );
                assert_eq!(
                    keybars, 1,
                    "w={w} h={h} region={region}: expected one key row"
                );
                assert!(
                    visible[h as usize - 1].contains("M-t"),
                    "w={w} h={h} region={region}: the key row must sit on the last line: {:#?}",
                    &visible[(h as usize - 6)..]
                );
            }
        }
    }

    #[test]
    fn command_history_moves_back_forward_and_restores_draft() {
        let mut app = sample_app();
        app.input.clear();
        app.record_command("status");
        app.record_command("paging");
        app.input = "mosms 555105 ".into();

        app.previous_command();
        assert_eq!(app.input, "paging");
        app.previous_command();
        assert_eq!(app.input, "status");
        app.previous_command();
        assert_eq!(app.input, "status");
        app.next_command();
        assert_eq!(app.input, "paging");
        app.next_command();
        assert_eq!(app.input, "mosms 555105 ");
        app.next_command();
        assert_eq!(app.input, "mosms 555105 ");
    }

    #[test]
    fn command_history_ignores_consecutive_duplicates() {
        let mut app = sample_app();
        app.record_command("status");
        app.record_command("status");
        assert_eq!(app.history, ["status"]);
    }

    #[test]
    fn stable_live_region_updates_without_clearing_it() {
        let mut app = sample_app();
        let mut first = Vec::new();
        let lines = shell_lines(&app, 110);
        let live =
            paint_shell(&mut first, &mut app, 110, 24, &LiveState::default(), &lines).unwrap();

        app.diag
            .as_mut()
            .unwrap()
            .pilot
            .as_mut()
            .unwrap()
            .rx_power_dbfs = -44.2;
        let mut update = Vec::new();
        let lines = shell_lines(&app, 110);
        paint_shell(&mut update, &mut app, 110, 24, &live, &lines).unwrap();

        assert!(
            !update.windows(3).any(|bytes| bytes == b"\x1b[J"),
            "status update cleared the live region before repainting"
        );
    }

    #[test]
    fn signaling_events_remain_in_transcript() {
        let pilot = cdma_ms::ms::MsEvent::PilotMeasurement {
            ec_io_db: Some(-3.0),
            rx_power_dbfs: -40.0,
        };
        let paging = cdma_ms::ms::MsEvent::PagingMessageDecoded {
            name: "General Page Message".into(),
            config_msg_seq: None,
        };
        let state = cdma_ms::ms::MsEvent::StateChange {
            from: "idle".into(),
            to: "system_access".into(),
        };

        assert!(transcript_event_line(&pilot).is_none());
        assert!(transcript_event_line(&paging).is_none());
        assert!(transcript_event_line(&state).is_some());
        let tx = cdma_ms::ms::MsEvent::TrafficTransmit {
            name: "Send Burst DTMF".into(),
            msg_seq: 3,
            ack_seq: 2,
            ack_req: true,
            retransmission: false,
        };
        let rx = cdma_ms::ms::MsEvent::TrafficMessage {
            name: "ORDM".into(),
            order: Some("Base Station Acknowledgment".into()),
            msg_seq: 4,
            ack_seq: 3,
            ack_req: false,
        };
        for (event, text) in [(tx, "traffic tx Send Burst DTMF"), (rx, "traffic rx ORDM")] {
            let wire = serde_json::to_string(&event).unwrap();
            let decoded = serde_json::from_str(&wire).unwrap();
            let line = transcript_event_line(&decoded).unwrap();
            assert!(line.to_string().contains(text));
        }
    }

    fn render_panel_to_string(app: &App, stats: bool) -> String {
        let backend = TestBackend::new(96, 24);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            if stats {
                render_stats(f, app)
            } else {
                render_logs(f, app)
            }
        })
        .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn emit_html_snapshot() {
        let Ok(path) = std::env::var("TUI_SNAPSHOT") else {
            return;
        };

        fn css(c: Color, fallback: &str) -> String {
            match c {
                Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
                _ => fallback.to_string(),
            }
        }
        fn esc(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        }
        fn to_html(buf: &ratatui::buffer::Buffer) -> String {
            let area = buf.area;
            let mut out = String::from("<pre class=\"screen\">");
            for y in 0..area.height {
                for x in 0..area.width {
                    let cell = &buf[(x, y)];
                    let fg = css(cell.fg, "#f8f8f2");
                    let bg = css(cell.bg, "#282a36");
                    out.push_str(&format!(
                        "<span style=\"color:{fg};background:{bg}\">{}</span>",
                        esc(cell.symbol())
                    ));
                }
                out.push('\n');
            }
            out.push_str("</pre>");
            out
        }
        fn shell_html(app: &App, width: u16) -> String {
            let bgs = ["#44475a", "#282a36", "#191a21", "#191a21"];
            let cap = width.saturating_sub(1) as usize;
            let mut out = String::from("<pre class=\"screen\">");
            for (i, line) in shell_lines(app, width).iter().enumerate() {
                let bar_bg = bgs[i];
                let mut col = 0usize;
                'spans: for span in &line.spans {
                    let fg = css(span.style.fg.unwrap_or(Color::Reset), "#f8f8f2");
                    let bg = css(span.style.bg.unwrap_or(Color::Reset), bar_bg);
                    for ch in span.content.chars() {
                        if col >= cap {
                            break 'spans;
                        }
                        out.push_str(&format!(
                            "<span style=\"color:{fg};background:{bg}\">{}</span>",
                            esc(&ch.to_string())
                        ));
                        col += 1;
                    }
                }
                while col < cap {
                    out.push_str(&format!("<span style=\"background:{bar_bg}\"> </span>"));
                    col += 1;
                }
                out.push('\n');
            }
            out.push_str("</pre>");
            out
        }

        let app = sample_app();
        let mut disc = sample_app();
        disc.diag = None;

        let shell = shell_html(&app, 110);
        let shell_off = shell_html(&disc, 110);
        let stats = {
            let mut t = Terminal::new(TestBackend::new(96, 22)).unwrap();
            t.draw(|f| render_stats(f, &app)).unwrap();
            to_html(t.backend().buffer())
        };
        let mut logapp = sample_app();
        logapp.logs.clear();
        for l in [
            "2026-09-17T21:14:02Z  INFO cdma_ms::ms: power on, scanning PRL",
            "2026-09-17T21:14:03Z  INFO cdma_ms::forward_rx: pilot PN 510 Ec/Io -2.1 dB",
            "2026-09-17T21:14:03Z  INFO cdma_ms::ms: sync decoded SID 4107 P_REV 6",
            "2026-09-17T21:14:04Z  WARN cdma_ms::tx: no access ack, retrying probe 2/5",
            "2026-09-17T21:14:05Z  INFO cdma_ms::ms: camped on BC1 ch50",
        ] {
            logapp.push_log_bytes(format!("{l}\n").as_bytes());
        }
        let logs = {
            let mut t = Terminal::new(TestBackend::new(96, 22)).unwrap();
            t.draw(|f| render_logs(f, &logapp)).unwrap();
            to_html(t.backend().buffer())
        };

        let page = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>cdma-ms TUI \
             (rendered)</title><style>body{{background:#1b1c24;color:#f8f8f2;\
             font-family:'JetBrains Mono',Menlo,Consolas,monospace;margin:0;padding:24px}}\
             h1{{color:#bd93f9;font-size:16px}}h2{{color:#8be9fd;font-size:13px;margin-top:26px}}\
             p{{color:#6272a4;font-size:12.5px;max-width:900px}}\
             .screen{{font-size:12px;line-height:1.28;padding:10px;border:1px solid #44475a;\
             border-radius:6px;overflow-x:auto;margin:6px 0}}\
             </style></head><body><h1>cdma-ms TUI — rendered frames</h1>\
             <p>These are the real ratatui frames, not a mockup. The shell is an inline viewport \
             (transcript lives in the terminal's own scrollback above the rule); Stats and Logs \
             open full screen on a hotkey.</p>\
             <h2>Shell — status bar (connected + camped)</h2>\
             <p>Scrollback flows above the rule. Live region = rule, powerline status bar, input, key bar.</p>{shell}\
             <h2>Shell — status bar (disconnected)</h2>\
             <p>All fields stay put with unavailable markers instead of vanishing.</p>{shell_off}\
             <h2>Stats panel — M-t</h2>{stats}\
             <h2>Logs panel — M-g</h2>{logs}\
             </body></html>"
        );
        std::fs::write(&path, page).unwrap();
        eprintln!("wrote {path}");
    }

    #[test]
    fn status_columns_do_not_move_between_states() {
        let mut app = sample_app();
        let mut layouts: Vec<(&str, Vec<(char, usize)>)> = Vec::new();

        for (state, sid, pn, freq, ec, rx, reg) in [
            (
                MsState::Idle,
                4107u32,
                510u32,
                50u32,
                -16.2f64,
                -35.0f64,
                false,
            ),
            (MsState::SyncAcquisition, 1, 1, 1, -3.0, -8.0, false),
            (
                MsState::TrafficChannelInit,
                65535,
                511,
                1023,
                -9.55,
                -120.0,
                true,
            ),
            (MsState::SystemDetermination, 0, 0, 0, -100.0, -7.5, true),
        ] {
            {
                let d = app.diag.as_mut().unwrap();
                d.state = state.into();
                d.registered = reg;
                let s = d.sync.as_mut().unwrap();
                s.sid = sid;
                s.pilot_pn = pn;
                s.cdma_freq = freq;
                let p = d.pilot.as_mut().unwrap();
                p.ec_io_db = ec;
                p.rx_power_dbfs = rx;
            }
            let text: String = status_line(&app, 200)
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            let marks = ["Ec/Io", "SID", "BASE", "PN", "chan", "REG"]
                .iter()
                .map(|label| {
                    let byte = text.find(label).unwrap_or_else(|| {
                        panic!("state {state:?}: column {label} missing from {text:?}")
                    });
                    (label.chars().next().unwrap(), text[..byte].chars().count())
                })
                .collect::<Vec<_>>();
            layouts.push((state.label(), marks));
        }

        let (first_state, first) = &layouts[0];
        for (state, marks) in &layouts[1..] {
            assert_eq!(
                marks, first,
                "state {state} moved the columns relative to {first_state}"
            );
        }
    }

    #[test]
    fn status_bar_shows_all_fields_when_connected() {
        let s = shell_text(&sample_app());
        assert!(s.contains("ms>"), "missing prompt");
        assert!(s.contains("IDLE"), "missing state pill");
        assert!(s.contains("Ec/Io"), "missing Ec/Io");
        assert!(s.contains("SID"), "missing SID");
        assert!(s.contains("4107"), "missing SID value");
        assert!(s.contains("BASE"), "missing BASE");
        assert!(s.contains("16040"), "missing base_id value");
        assert!(s.contains("510"), "missing PN");
        assert!(s.contains("REG"), "missing reg segment");
        assert!(s.contains("M-t"), "missing keybar");
    }

    #[test]
    fn status_bar_shows_placeholders_when_disconnected() {
        let mut app = sample_app();
        app.diag = None;
        let s = shell_text(&app);
        assert!(s.contains("OFFLINE"), "missing offline pill");
        assert!(s.contains("SID"), "SID label dropped when disconnected");
        assert!(s.contains("BASE"), "BASE label dropped when disconnected");
        assert!(s.contains("--"), "missing unavailable markers");
        assert!(s.contains("REG"), "REG label dropped when disconnected");
    }

    #[test]
    fn stats_panel_renders_groups() {
        let s = render_panel_to_string(&sample_app(), true);
        assert!(s.contains("STATS"), "missing panel title");
        assert!(s.contains("SIGNAL"), "missing signal group");
        assert!(s.contains("SERVING"), "missing serving group");
        assert!(s.contains("IDENTITY"), "missing identity group");
        assert!(s.contains("16040"), "missing base_id");
        assert!(s.contains("4107"), "missing SID");
    }

    #[test]
    fn logs_panel_renders_captured_lines() {
        let mut app = sample_app();
        app.logs.clear();
        app.push_log_bytes(b"2026-09-18T02:45:26Z  INFO cdma_bts::bts: tx_stats batches=2000\n");
        assert_eq!(app.logs.len(), 1, "one full line captured");
        app.push_log_bytes(b"2026-09-18T02:45:27Z  WARN partial");
        assert_eq!(app.logs.len(), 1, "partial line not emitted yet");
        app.push_log_bytes(b" line\n");
        assert_eq!(app.logs.len(), 2, "completed line emitted");

        let s = render_panel_to_string(&app, false);
        assert!(s.contains("LOGS"), "missing log pane title");
        assert!(s.contains("tx_stats"), "missing captured log line");
    }

    #[tokio::test]
    async fn alt_toggles_switch_panels() {
        let endpoint = tonic::transport::Endpoint::from_static("http://127.0.0.1:1");
        let mut client = MsServiceClient::new(endpoint.connect_lazy());
        let mut app = sample_app();
        assert_eq!(app.mode, Mode::Shell);

        handle_key(
            &mut app,
            &mut client,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT),
        )
        .await;
        assert_eq!(app.mode, Mode::Stats, "M-t opens stats");
        handle_key(
            &mut app,
            &mut client,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::ALT),
        )
        .await;
        assert_eq!(app.mode, Mode::Logs, "M-g switches to logs");
        handle_key(
            &mut app,
            &mut client,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;
        assert_eq!(app.mode, Mode::Shell, "Esc returns to shell");
    }

    #[tokio::test]
    async fn ctrl_c_needs_two_presses_to_quit() {
        let endpoint = tonic::transport::Endpoint::from_static("http://127.0.0.1:1");
        let mut client = MsServiceClient::new(endpoint.connect_lazy());
        let mut app = sample_app();
        app.input.clear();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        handle_key(&mut app, &mut client, ctrl_c).await;
        assert!(app.quit_armed, "first Ctrl-C arms quit");
        assert!(!app.should_quit, "first Ctrl-C does not quit");

        handle_key(
            &mut app,
            &mut client,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .await;
        assert!(!app.quit_armed, "another key cancels the warning");
        assert!(!app.should_quit);

        handle_key(&mut app, &mut client, ctrl_c).await;
        handle_key(&mut app, &mut client, ctrl_c).await;
        assert!(app.should_quit, "two Ctrl-C in a row quit");
    }

    #[test]
    fn signal_bars_scale_with_ec_io() {
        assert_eq!(signal_level(-2.0), 4);
        assert_eq!(signal_level(-100.0), 0);
        assert_eq!(signal_span(-2.0).content.as_ref(), "┃┃┃┃");
        assert_eq!(signal_span(-100.0).content.as_ref(), "││││");
    }

    #[test]
    fn bar_line_never_overflows_the_terminal_width() {
        let line = Line::from(vec![Span::raw("┃┃┃┃ ✓ ✗ ↑↓ ││││ padding text here")]);
        for width in [8u16, 12, 20, 40] {
            let mut buf = Vec::new();
            let cols = emit_bar_line(&mut buf, &line, width, BG).unwrap();
            assert!(
                cols <= width.saturating_sub(1),
                "width {width}: emitted {cols} columns, must be <= {}",
                width - 1
            );
            assert!(
                cols < width,
                "width {width}: a clipped bar line must fit in one row"
            );
        }
    }
}
