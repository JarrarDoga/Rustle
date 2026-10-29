//! Keyboard shortcuts, the control socket, and the `rustle play|next|...`
//! command line.
//!
//! - [`shortcuts`]: in-window keys, read once per frame by `app.rs`.
//! - [`Control`]: the socket server, shaped like `MediaKeys`. Commands from
//!   the socket go through the UI thread (`poll`), never straight into the
//!   backend, so the UI's optimistic replay stays correct. `publish` keeps
//!   the `status` reply current.
//! - [`run_cli`]: runs in `main` before tokio or eframe start, so
//!   `rustle next` never opens a window.
//!
//! Socket path: `$RUSTLE_CONTROL_SOCKET` if set, else
//! `$XDG_RUNTIME_DIR/rustle/control.sock`, or `control-demo.sock` with
//! `--demo`, so test runs never touch the user's real instance.
//!
//! Protocol: one request per line (`play`, `pause`, `toggle`, `next`,
//! `previous`, `status`, `raise`), one reply line per request: `ok`,
//! `error <message>`, or for `status` one JSON object:
//! `{"title":..,"artist":..,"position":<s>,"duration":<s or null>,"playing":<bool>,"loading":<bool>}`.
//! While the current track loads, `playing` is false, `loading` is true and
//! `position` holds still until the audio starts.

use std::fmt::Write as _;
use std::fs::{self, DirBuilder, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use egui::{Event, InputState, Key, Modifiers, ViewportCommand};
use model::Track;
use tokio::runtime::Handle;

use crate::state::{Command, Playback, State};
use crate::views::{self, Action, Route, View};

/// A client that stops sending or reading for this long is dropped, so one
/// stuck client cannot hold the server.
const IO_TIMEOUT: Duration = Duration::from_secs(1);
/// Longest request line the server reads; real requests are under 10 bytes.
const MAX_LINE: u64 = 256;
/// The CLI waits this long for a reply. The server answers without waiting
/// for the UI, so this only trips on a hung instance.
const CLI_TIMEOUT: Duration = Duration::from_secs(2);
/// Volume steps per 1.0: Ctrl+Up and Ctrl+Down move 5%.
const VOLUME_STEPS: f32 = 20.0;
const USAGE: &str = "usage: rustle [--demo [--first-run]]
       rustle play | pause | toggle | next | previous | status
       rustle --help | --version";
const HELP: &str = "
With no arguments, brings the running Rustle to the front, or starts one.

Commands for the running Rustle:
  play, pause, toggle   control playback
  next, previous        skip tracks
  status                print what is playing as JSON

Options:
  --demo                run on recorded data, with no account
  --first-run           with --demo, start at the main-playlist picker
  -h, --help            print this help
  -V, --version         print the version";
const CLI_VERBS: [&str; 6] = ["play", "pause", "toggle", "next", "previous", "status"];

/// The control socket for this instance.
pub fn socket_path(demo: bool) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("RUSTLE_CONTROL_SOCKET") {
        return Some(PathBuf::from(p));
    }
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("rustle");
    Some(dir.join(if demo {
        "control-demo.sock"
    } else {
        "control.sock"
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Status,
    Raise,
}

impl Request {
    fn parse(line: &str) -> Option<Self> {
        Some(match line.trim() {
            "play" => Self::Play,
            "pause" => Self::Pause,
            "toggle" => Self::Toggle,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "status" => Self::Status,
            "raise" => Self::Raise,
            _ => return None,
        })
    }
}

/// What `status` reports: the playing track and where playback is.
#[derive(Clone, Default)]
struct Snapshot {
    track: Option<Arc<Track>>,
    playback: Playback,
}

impl Snapshot {
    fn of(state: &State) -> Self {
        Self {
            track: state.current_track().cloned(),
            playback: state.playback.clone(),
        }
    }

    /// Same track and playback; volume does not count.
    fn same(&self, other: &Self) -> bool {
        let track = match (&self.track, &other.track) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let (a, b) = (&self.playback, &other.playback);
        track
            && a.status == b.status
            && a.position == b.position
            && a.anchor == b.anchor
            && a.loading == b.loading
    }

    fn json(&self, now: Instant) -> String {
        let track = self.track.as_deref();
        let duration = track.and_then(|t| t.duration);
        let mut position = self.playback.position_at(now);
        if let Some(d) = duration {
            position = position.min(d);
        }
        let (title, artist) = match track {
            Some(t) => (json_str(&t.title), json_str(&views::artists(t))),
            None => ("null".to_owned(), "null".to_owned()),
        };
        let duration = duration.map_or("null".to_owned(), |d| format!("{:.1}", d.as_secs_f64()));
        format!(
            r#"{{"title":{title},"artist":{artist},"position":{:.1},"duration":{duration},"playing":{},"loading":{}}}"#,
            position.as_secs_f64(),
            self.playback.is_playing() && !self.playback.loading,
            self.playback.loading,
        )
    }
}

/// `s` as a JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The socket server. Dropping it stops serving and removes the socket.
pub struct Control {
    ctx: egui::Context,
    /// Requests from the server thread; `None` when not serving.
    requests: Option<Receiver<Request>>,
    /// Read by the server thread to answer `status`.
    status: Arc<Mutex<Snapshot>>,
    /// What `status` holds, kept here to compare without locking.
    last: Snapshot,
    server: Option<Server>,
}

/// The running server thread. Dropping it wakes the thread and removes the
/// socket file.
struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking `accept` so the thread sees `stop`; it must
        // happen before the file goes away.
        let _ = UnixStream::connect(&self.path);
        let _ = fs::remove_file(&self.path);
    }
}

impl Control {
    /// Starts serving on `rt`. Never blocks the caller; a socket that
    /// another live instance answers on is left alone (log once).
    ///
    /// The server is one small `rustle-control` std thread blocking on
    /// `accept`, not a tokio task: it needs no tokio `net` feature, uses no
    /// CPU while idle, answers each short request inline, and ends with the
    /// process, so it never keeps the app alive. `_rt` is unused.
    pub fn start(_rt: &Handle, ctx: egui::Context, demo: bool) -> Self {
        Self::start_at(socket_path(demo), ctx)
    }

    fn start_at(path: Option<PathBuf>, ctx: egui::Context) -> Self {
        let mut control = Self {
            ctx: ctx.clone(),
            requests: None,
            status: Arc::default(),
            last: Snapshot::default(),
            server: None,
        };
        let Some(path) = path else {
            eprintln!("rustle: control socket off: XDG_RUNTIME_DIR is not set");
            return control;
        };
        let listener = match bind(&path) {
            Ok(Some(listener)) => listener,
            Ok(None) => {
                eprintln!(
                    "rustle: another Rustle answers on {}; control socket off",
                    path.display()
                );
                return control;
            }
            Err(e) => {
                eprintln!("rustle: control socket off: {}: {e}", path.display());
                return control;
            }
        };
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::clone(&control.status);
        let thread_stop = Arc::clone(&stop);
        let spawned = thread::Builder::new()
            .name("rustle-control".into())
            .spawn(move || serve(&listener, &tx, &status, &ctx, &thread_stop));
        if let Err(e) = spawned {
            eprintln!("rustle: control socket off: {e}");
            let _ = fs::remove_file(&path);
            return control;
        }
        control.requests = Some(rx);
        control.server = Some(Server { path, stop });
        control
    }

    /// Commands received since the last call. `raise` is handled here
    /// (`ViewportCommand::Focus`) and not returned.
    pub fn poll(&mut self) -> Vec<Command> {
        let Some(requests) = &self.requests else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // `play` and `pause` toggle only when needed; several in one batch
        // see the effect of the ones before.
        let mut playing = self.last.playback.is_playing();
        while let Ok(request) = requests.try_recv() {
            match request {
                Request::Toggle => {
                    playing = !playing;
                    out.push(Command::TogglePlay);
                }
                Request::Play if !playing => {
                    playing = true;
                    out.push(Command::TogglePlay);
                }
                Request::Pause if playing => {
                    playing = false;
                    out.push(Command::TogglePlay);
                }
                Request::Next => out.push(Command::Next),
                Request::Previous => out.push(Command::Previous),
                Request::Raise => self.ctx.send_viewport_cmd(ViewportCommand::Focus),
                Request::Play | Request::Pause | Request::Status => {}
            }
        }
        out
    }

    /// Keeps the `status` reply current. Called every frame; cheap when
    /// nothing changed.
    pub fn publish(&mut self, state: &State) {
        if self.server.is_none() {
            return;
        }
        let now = Snapshot::of(state);
        if now.same(&self.last) {
            return;
        }
        *lock(&self.status) = now.clone();
        self.last = now;
    }
}

fn lock(status: &Mutex<Snapshot>) -> std::sync::MutexGuard<'_, Snapshot> {
    status.lock().unwrap_or_else(|e| e.into_inner())
}

/// Binds the socket at `path`. `Ok(None)`: another instance answers there.
fn bind(path: &Path) -> io::Result<Option<UnixListener>> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        // Only directories created here get 0700; an existing one (say, a
        // temp dir from RUSTLE_CONTROL_SOCKET) is left as it is.
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if UnixStream::connect(path).is_ok() {
                return Ok(None);
            }
            if !meta.file_type().is_socket() {
                return Err(io::Error::other("exists and is not a socket"));
            }
            // Nothing answers: left over from an instance that crashed.
            fs::remove_file(path)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        // Another instance bound it between the check and here.
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => return Ok(None),
        Err(e) => return Err(e),
    };
    fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(Some(listener))
}

fn serve(
    listener: &UnixListener,
    tx: &Sender<Request>,
    status: &Mutex<Snapshot>,
    ctx: &egui::Context,
    stop: &AtomicBool,
) {
    for conn in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match conn {
            Ok(stream) => {
                // A client that hangs up or times out only ends its own
                // connection.
                let _ = handle(&stream, tx, status, ctx);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                // E.g. out of file descriptors. Rare; back off instead of
                // spinning on the error.
                eprintln!("rustle: control socket: {e}");
                thread::sleep(IO_TIMEOUT);
            }
        }
    }
}

/// Answers each line from one client until it hangs up.
fn handle(
    stream: &UnixStream,
    tx: &Sender<Request>,
    status: &Mutex<Snapshot>,
    ctx: &egui::Context,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
    let mut line = String::new();
    loop {
        line.clear();
        let n = (&mut reader).take(MAX_LINE).read_line(&mut line)?;
        // EOF before a request: e.g. another instance checking whether this
        // one is live. Nothing to answer.
        if n == 0 {
            return Ok(());
        }
        if !line.ends_with('\n') && n as u64 == MAX_LINE {
            writer.write_all(b"error request too long\n")?;
            return Ok(());
        }
        let mut reply = respond(&line, tx, status, ctx);
        reply.push('\n');
        writer.write_all(reply.as_bytes())?;
    }
}

/// The reply line for one request, without the newline.
fn respond(
    line: &str,
    tx: &Sender<Request>,
    status: &Mutex<Snapshot>,
    ctx: &egui::Context,
) -> String {
    match Request::parse(line) {
        Some(Request::Status) => {
            let snapshot = lock(status).clone();
            snapshot.json(Instant::now())
        }
        Some(request) => {
            if tx.send(request).is_err() {
                return "error Rustle is closing".to_owned();
            }
            ctx.request_repaint();
            "ok".to_owned()
        }
        None if line.trim().is_empty() => "error empty request".to_owned(),
        None => {
            let shown: String = line.trim().chars().filter(|c| !c.is_control()).collect();
            format!("error unknown command: {shown}")
        }
    }
}

/// In-window shortcuts for this frame. Keys are ignored while a widget has
/// keyboard focus (a text field, or a button egui fires on Space itself),
/// except Ctrl+Q.
pub fn shortcuts(ctx: &egui::Context, state: &State, view: &mut View) -> Vec<Action> {
    let mut out = Vec::new();
    // egui's own quit shortcut (`Options::quit_shortcuts`) is also Ctrl+Q;
    // consuming it here means one Close, from either place.
    if ctx.input_mut(|i| presses(i, Modifiers::COMMAND, Key::Q, false)) > 0 {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    if ctx.egui_wants_keyboard_input() {
        return out;
    }
    let cmd = Modifiers::COMMAND;
    let [toggle, next, previous, up, down, find, locate] = ctx.input_mut(|i| {
        [
            presses(i, Modifiers::NONE, Key::Space, false),
            presses(i, cmd, Key::ArrowRight, false),
            presses(i, cmd, Key::ArrowLeft, false),
            presses(i, cmd, Key::ArrowUp, true),
            presses(i, cmd, Key::ArrowDown, true),
            presses(i, cmd, Key::F, false),
            presses(i, cmd, Key::L, false),
        ]
    });
    let repeat = |n: usize, command: Command| (0..n).map(move |_| Action::Cmd(command.clone()));
    out.extend(repeat(toggle, Command::TogglePlay));
    out.extend(repeat(next, Command::Next));
    out.extend(repeat(previous, Command::Previous));
    if up != down {
        // Snap to the 5% grid so steps stay round after a slider drag.
        let steps = up as f32 - down as f32;
        let volume = ((state.playback.volume * VOLUME_STEPS).round() + steps) / VOLUME_STEPS;
        out.push(Action::Cmd(Command::SetVolume(volume.clamp(0.0, 1.0))));
    }
    if find > 0 {
        view.collection.focus_filter = true;
        out.push(Action::Go(Route::Collection));
    }
    if locate > 0 {
        views::locate(state, view, &mut out);
    }
    // Not consumed: the spine clears its range on the same press.
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        view.deselect();
    }
    // The route change and focus land on the next frame.
    if find > 0 || locate > 0 {
        ctx.request_repaint();
    }
    out
}

/// Consumes every press of `modifiers`+`key` this frame (extra Shift and
/// Alt allowed, like egui's `consume_key`) and counts them. Held-key
/// repeats are consumed too but count only when `repeats` is set.
fn presses(input: &mut InputState, modifiers: Modifiers, key: Key, repeats: bool) -> usize {
    let mut count = 0;
    input.events.retain(|event| match event {
        Event::Key {
            key: k,
            pressed: true,
            repeat,
            modifiers: m,
            ..
        } if *k == key && m.matches_logically(modifiers) => {
            count += usize::from(repeats || !*repeat);
            false
        }
        _ => true,
    });
    count
}

/// The flags the app reads at start.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    /// `--demo`: run on `FixtureBackend` with no account.
    pub demo: bool,
    /// `--first-run`, only with `--demo`: start with no main playlist.
    pub first_run: bool,
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Cli<'a> {
    /// Start the app. With no flags at all, first try to raise a running one.
    App(Flags),
    /// One of [`CLI_VERBS`], sent to the running instance.
    Verb(&'a str),
    Help,
    Version,
    /// A usage error, with the message to print above the usage.
    Usage(String),
}

/// Parses the arguments, left to right: the first bad one is the error, and
/// `--help` or `--version` before it wins. A verb must come alone, and
/// anything not listed in [`HELP`] is a usage error, so a typo never starts
/// a second window.
fn parse(args: &[String]) -> Cli<'_> {
    let mut flags = Flags::default();
    let mut verb = None;
    for (i, arg) in args.iter().enumerate() {
        match arg.as_str() {
            "-h" | "--help" => return Cli::Help,
            "-V" | "--version" => return Cli::Version,
            "--demo" if verb.is_none() => flags.demo = true,
            "--first-run" if verb.is_none() => flags.first_run = true,
            v if i == 0 && CLI_VERBS.contains(&v) => verb = Some(v),
            other => return Cli::Usage(format!("unexpected argument '{other}'")),
        }
    }
    if let Some(verb) = verb {
        return Cli::Verb(verb);
    }
    if flags.first_run && !flags.demo {
        return Cli::Usage("--first-run needs --demo".to_owned());
    }
    Cli::App(flags)
}

/// Handles `rustle play|pause|toggle|next|previous|status`, `--help`,
/// `--version`, usage errors, and the single instance check. `Ok` means:
/// start the app with these flags; `Err` means: exit with this code.
pub fn run_cli(args: &[String]) -> Result<Flags, ExitCode> {
    let path = socket_path(false);
    cli(
        args,
        path.as_deref(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
    .map_err(ExitCode::from)
}

/// [`run_cli`] with the socket path and output streams passed in.
fn cli(
    args: &[String],
    path: Option<&Path>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Flags, u8> {
    let verb = match parse(args) {
        Cli::App(flags) => {
            // Plain `rustle` while one is running: bring that one up
            // instead. Flags always start a new instance.
            let raised = args.is_empty() && path.is_some_and(|p| request(p, "raise").is_ok());
            return if raised { Err(0) } else { Ok(flags) };
        }
        Cli::Verb(verb) => verb,
        Cli::Help => {
            let _ = writeln!(out, "{USAGE}\n{HELP}");
            return Err(0);
        }
        Cli::Version => {
            let _ = writeln!(out, "rustle {}", env!("CARGO_PKG_VERSION"));
            return Err(0);
        }
        Cli::Usage(message) => {
            let _ = writeln!(err, "rustle: {message}\n{USAGE}");
            return Err(2);
        }
    };
    let result = match path {
        Some(path) => request(path, verb),
        None => Err(CliError::NotRunning),
    };
    Err(match result {
        Ok(reply) => match reply.strip_prefix("error ") {
            Some(message) => {
                let _ = writeln!(err, "rustle: {message}");
                1
            }
            None => {
                let _ = writeln!(out, "{reply}");
                0
            }
        },
        Err(CliError::NotRunning) => {
            let _ = writeln!(err, "Rustle is not running");
            1
        }
        Err(CliError::NoAnswer(e)) => {
            let _ = writeln!(err, "rustle: no answer from the running instance: {e}");
            1
        }
    })
}

enum CliError {
    NotRunning,
    NoAnswer(io::Error),
}

/// Sends one request line and reads the one-line reply.
fn request(path: &Path, line: &str) -> Result<String, CliError> {
    let stream = UnixStream::connect(path).map_err(|_| CliError::NotRunning)?;
    let talk = || -> io::Result<String> {
        stream.set_read_timeout(Some(CLI_TIMEOUT))?;
        stream.set_write_timeout(Some(CLI_TIMEOUT))?;
        (&stream).write_all(format!("{line}\n").as_bytes())?;
        let mut reply = String::new();
        BufReader::new((&stream).take(64 * 1024)).read_line(&mut reply)?;
        if reply.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(reply.trim_end().to_owned())
    };
    talk().map_err(CliError::NoAnswer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::track;
    use crate::state::apply;
    use crate::state::tests::started;
    use egui::RawInput;
    use player::PlaybackStatus;

    /// A fresh path under the temp dir, short enough for a socket.
    fn temp_sock(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rctl-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("c.sock")
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    fn playing(ids: &[&str]) -> State {
        let mut s = State::default();
        let tracks = Arc::new(ids.iter().map(|id| track(id)).collect());
        let play = Command::PlayTracks {
            tracks,
            start: 0,
            seed: 1,
        };
        let now = Instant::now();
        apply(&mut s, &play, now);
        started(&mut s, now);
        s
    }

    /// Sends `lines` on one connection and returns the replies.
    fn talk(path: &Path, lines: &[&str]) -> Vec<String> {
        let stream = UnixStream::connect(path).unwrap();
        stream.set_read_timeout(Some(CLI_TIMEOUT)).unwrap();
        let mut reader = BufReader::new(&stream);
        lines
            .iter()
            .map(|line| {
                (&stream).write_all(format!("{line}\n").as_bytes()).unwrap();
                let mut reply = String::new();
                reader.read_line(&mut reply).unwrap();
                reply.trim_end().to_owned()
            })
            .collect()
    }

    /// Polls until `n` commands arrive (the server thread queues them).
    fn poll_n(control: &mut Control, n: usize) -> Vec<Command> {
        let mut out = Vec::new();
        for _ in 0..200 {
            out.extend(control.poll());
            if out.len() >= n {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        out
    }

    #[test]
    fn parses_every_verb() {
        let verbs = [
            ("play", Request::Play),
            ("pause", Request::Pause),
            ("toggle", Request::Toggle),
            ("next", Request::Next),
            ("previous", Request::Previous),
            ("status", Request::Status),
            ("raise", Request::Raise),
        ];
        for (line, request) in verbs {
            assert_eq!(Request::parse(line), Some(request));
            assert_eq!(Request::parse(&format!(" {line}\r\n")), Some(request));
        }
        for bad in ["", "\n", "PLAY", "next now", "nex", "quit"] {
            assert_eq!(Request::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn replies_to_bad_requests() {
        let (tx, _rx) = mpsc::channel();
        let status = Mutex::default();
        let ctx = egui::Context::default();
        let reply = |line| respond(line, &tx, &status, &ctx);
        assert_eq!(reply("\n"), "error empty request");
        assert_eq!(reply("jump\n"), "error unknown command: jump");
        assert_eq!(reply("a\u{7}b\n"), "error unknown command: ab");
        assert_eq!(reply("next\n"), "ok");
    }

    #[test]
    fn closed_ui_is_an_error() {
        let (tx, rx) = mpsc::channel();
        drop(rx);
        let ctx = egui::Context::default();
        assert_eq!(
            respond("next", &tx, &Mutex::default(), &ctx),
            "error Rustle is closing"
        );
    }

    #[test]
    fn json_escapes_strings() {
        assert_eq!(json_str("plain"), r#""plain""#);
        assert_eq!(
            json_str("a \"b\" \\ c\nd\te\u{1}"),
            r#""a \"b\" \\ c\nd\te\u0001""#
        );
        assert_eq!(json_str("Björk – Jóga"), "\"Björk – Jóga\"");
    }

    #[test]
    fn status_json_with_and_without_a_track() {
        let now = Instant::now();
        assert_eq!(
            Snapshot::default().json(now),
            r#"{"title":null,"artist":null,"position":0.0,"duration":null,"playing":false,"loading":false}"#
        );
        let mut s = playing(&["a"]);
        s.playback.anchor = Some(now - Duration::from_millis(12_340));
        assert_eq!(
            Snapshot::of(&s).json(now),
            r#"{"title":"A","artist":"","position":12.3,"duration":100.0,"playing":true,"loading":false}"#
        );
        // Extrapolation never runs past the end of the track.
        s.playback.anchor = Some(now - Duration::from_secs(500));
        assert!(Snapshot::of(&s).json(now).contains(r#""position":100.0"#));
        s.playback.status = PlaybackStatus::Paused;
        s.playback.anchor = None;
        s.playback.position = Duration::from_secs(7);
        assert_eq!(
            Snapshot::of(&s).json(now),
            r#"{"title":"A","artist":"","position":7.0,"duration":100.0,"playing":false,"loading":false}"#
        );
    }

    #[test]
    fn snapshot_ignores_volume() {
        let s = playing(&["a"]);
        let a = Snapshot::of(&s);
        let mut louder = s.clone();
        louder.playback.volume = 0.1;
        assert!(a.same(&Snapshot::of(&louder)));
        let mut paused = s.clone();
        paused.playback.status = PlaybackStatus::Paused;
        assert!(!a.same(&Snapshot::of(&paused)));
        assert!(!a.same(&Snapshot::of(&playing(&["a"]))), "another Arc");
        assert!(!a.same(&Snapshot::default()));
    }

    #[test]
    fn serves_commands_status_and_cleans_up() {
        let path = temp_sock("serve");
        let mut control = Control::start_at(Some(path.clone()), egui::Context::default());
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);

        let s = playing(&["a"]);
        control.publish(&s);
        let replies = talk(&path, &["next", "previous", "status", "bogus"]);
        assert_eq!(replies[..2], ["ok", "ok"]);
        assert!(replies[2].starts_with(r#"{"title":"A","#), "{}", replies[2]);
        assert!(
            replies[2].ends_with(r#""playing":true,"loading":false}"#),
            "{}",
            replies[2]
        );
        assert_eq!(replies[3], "error unknown command: bogus");
        assert!(matches!(
            poll_n(&mut control, 2)[..],
            [Command::Next, Command::Previous]
        ));

        drop(control);
        assert!(!path.exists(), "drop removes the socket");
        cleanup(&path);
    }

    #[test]
    fn play_and_pause_toggle_only_when_needed() {
        let path = temp_sock("playpause");
        let mut control = Control::start_at(Some(path.clone()), egui::Context::default());
        control.publish(&playing(&["a"]));
        // Playing: `play` does nothing, the first `pause` toggles, the
        // second sees it paused, then `play` toggles back.
        let replies = talk(&path, &["play", "pause", "pause", "play", "raise"]);
        assert!(replies.iter().all(|r| r == "ok"), "{replies:?}");
        thread::sleep(Duration::from_millis(50));
        let commands = poll_n(&mut control, 2);
        assert!(
            matches!(commands[..], [Command::TogglePlay, Command::TogglePlay]),
            "{commands:?}"
        );
        drop(control);
        cleanup(&path);
    }

    #[test]
    fn removes_a_stale_socket() {
        let path = temp_sock("stale");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A socket file nobody listens on, as a crashed instance leaves it.
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let mut control = Control::start_at(Some(path.clone()), egui::Context::default());
        assert!(control.server.is_some(), "serves after removing it");
        assert_eq!(talk(&path, &["toggle"]), ["ok"]);
        assert_eq!(poll_n(&mut control, 1).len(), 1);
        drop(control);
        cleanup(&path);
    }

    #[test]
    fn leaves_a_live_socket_alone() {
        let path = temp_sock("live");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let other = UnixListener::bind(&path).unwrap();
        let mut control = Control::start_at(Some(path.clone()), egui::Context::default());
        assert!(control.server.is_none());
        assert!(control.poll().is_empty());
        drop(control);
        assert!(path.exists(), "the other instance's socket stays");
        // The check connected once and sent nothing.
        other.set_nonblocking(true).unwrap();
        let (probe, _) = other.accept().unwrap();
        let mut buf = Vec::new();
        (&probe).read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty());
        cleanup(&path);
    }

    #[test]
    fn leaves_a_regular_file_alone() {
        let path = temp_sock("file");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not a socket").unwrap();
        let control = Control::start_at(Some(path.clone()), egui::Context::default());
        assert!(control.server.is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not a socket");
        cleanup(&path);
    }

    fn run(args: &[&str], path: Option<&Path>) -> (Option<u8>, String, String) {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = cli(&args, path, &mut out, &mut err).err();
        let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
        (code, text(out), text(err))
    }

    /// A fake instance that answers each request line with `reply` and
    /// records what it got.
    fn fake_server(path: &Path, reply: &'static str) -> thread::JoinHandle<Vec<String>> {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(path).unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut got = Vec::new();
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            while reader.read_line(&mut line).unwrap() > 0 {
                got.push(line.trim_end().to_owned());
                line.clear();
                (&stream)
                    .write_all(format!("{reply}\n").as_bytes())
                    .unwrap();
            }
            got
        })
    }

    #[test]
    fn cli_prints_the_reply() {
        let path = temp_sock("cli-ok");
        let server = fake_server(&path, "ok");
        assert_eq!(
            run(&["next"], Some(&path)),
            (Some(0), "ok\n".into(), "".into())
        );
        assert_eq!(server.join().unwrap(), ["next"]);

        let json = r#"{"title":"A","artist":"","position":1.0,"duration":null,"playing":true}"#;
        fs::remove_file(&path).unwrap();
        let server = fake_server(&path, json);
        assert_eq!(
            run(&["status"], Some(&path)),
            (Some(0), format!("{json}\n"), "".into())
        );
        assert_eq!(server.join().unwrap(), ["status"]);
        cleanup(&path);
    }

    #[test]
    fn cli_error_reply_exits_1() {
        let path = temp_sock("cli-err");
        let server = fake_server(&path, "error Rustle is closing");
        assert_eq!(
            run(&["play"], Some(&path)),
            (Some(1), "".into(), "rustle: Rustle is closing\n".into())
        );
        server.join().unwrap();
        cleanup(&path);
    }

    #[test]
    fn cli_without_an_instance_exits_1() {
        let path = temp_sock("cli-none");
        let not_running = (Some(1), String::new(), "Rustle is not running\n".to_owned());
        assert_eq!(run(&["next"], Some(&path)), not_running);
        assert_eq!(run(&["status"], None), not_running);
        // A stale socket file is the same as none.
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        drop(UnixListener::bind(&path).unwrap());
        assert_eq!(run(&["pause"], Some(&path)), not_running);
        cleanup(&path);
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn parse_verbs() {
        for verb in CLI_VERBS {
            assert_eq!(parse(&strings(&[verb])), Cli::Verb(verb));
        }
    }

    #[test]
    fn parse_app_flags() {
        let app = |demo, first_run| Cli::App(Flags { demo, first_run });
        let cases: [(&[&str], _); 5] = [
            (&[], app(false, false)),
            (&["--demo"], app(true, false)),
            (&["--demo", "--first-run"], app(true, true)),
            (&["--first-run", "--demo"], app(true, true)),
            (&["--demo", "--demo"], app(true, false)),
        ];
        for (args, want) in cases {
            assert_eq!(parse(&strings(args)), want, "{args:?}");
        }
    }

    #[test]
    fn parse_help_and_version() {
        for args in [
            &["--help"][..],
            &["-h"],
            &["--demo", "--help"],
            &["next", "--help"],
            &["--help", "--bogus"],
        ] {
            assert_eq!(parse(&strings(args)), Cli::Help, "{args:?}");
        }
        for args in [&["--version"][..], &["-V"], &["--demo", "--version"]] {
            assert_eq!(parse(&strings(args)), Cli::Version, "{args:?}");
        }
    }

    #[test]
    fn parse_usage_errors() {
        let bad = |a: &str| Cli::Usage(format!("unexpected argument '{a}'"));
        let cases: [(&[&str], _); 12] = [
            (&["--demo2"], bad("--demo2")),
            (&["--bogus", "--help"], bad("--bogus")),
            (&["--"], bad("--")),
            (&["-d"], bad("-d")),
            (&["--DEMO"], bad("--DEMO")),
            (&["bogus"], bad("bogus")),
            (&["help"], bad("help")),
            (&["raise"], bad("raise")),
            (&["next", "now"], bad("now")),
            (&["next", "--demo"], bad("--demo")),
            (&["--demo", "next"], bad("next")),
            (
                &["--first-run"],
                Cli::Usage("--first-run needs --demo".into()),
            ),
        ];
        for (args, want) in cases {
            assert_eq!(parse(&strings(args)), want, "{args:?}");
        }
    }

    #[test]
    fn cli_usage_exits_2() {
        let path = temp_sock("cli-usage");
        for args in [
            &["jump"][..],
            &["next", "now"],
            &["help"],
            &["--demo2"],
            &["--first-run"],
        ] {
            let (code, out, err) = run(args, Some(&path));
            assert_eq!(code, Some(2), "{args:?}");
            assert!(out.is_empty());
            assert!(err.starts_with("rustle: "), "{err}");
            assert!(err.contains("\nusage: rustle"), "{err}");
        }
        assert!(!path.exists(), "usage never connects");
    }

    #[test]
    fn cli_help_and_version_exit_0() {
        // A live instance must not hear about either.
        let path = temp_sock("cli-help");
        let server = fake_server(&path, "ok");
        let (code, out, err) = run(&["--help"], Some(&path));
        assert_eq!((code, err.as_str()), (Some(0), ""));
        assert!(out.starts_with("usage: rustle"), "{out}");
        for word in CLI_VERBS
            .iter()
            .chain(&["--demo", "--first-run", "--version"])
        {
            assert!(out.contains(word), "{word} missing from {out}");
        }
        let version = format!("rustle {}\n", env!("CARGO_PKG_VERSION"));
        assert_eq!(run(&["-V"], Some(&path)), (Some(0), version, "".into()));
        // Wake the fake server with one real request; it saw only that.
        assert_eq!(run(&["next"], Some(&path)).0, Some(0));
        assert_eq!(server.join().unwrap(), ["next"]);
        cleanup(&path);
    }

    #[test]
    fn cli_plain_rustle_raises_a_running_instance() {
        let path = temp_sock("cli-raise");
        assert_eq!(run(&[], Some(&path)), (None, "".into(), "".into()));
        assert_eq!(run(&[], None).0, None);
        let server = fake_server(&path, "ok");
        assert_eq!(run(&[], Some(&path)), (Some(0), "".into(), "".into()));
        assert_eq!(server.join().unwrap(), ["raise"]);
        cleanup(&path);
    }

    #[test]
    fn app_flags_start_without_raising() {
        let path = temp_sock("cli-flags");
        let server = fake_server(&path, "ok");
        for args in [&["--demo"][..], &["--demo", "--first-run"]] {
            let (code, out, err) = run(args, Some(&path));
            assert_eq!((code, out.as_str(), err.as_str()), (None, "", ""));
        }
        let args = strings(&["--demo", "--first-run"]);
        let flags = cli(&args, Some(&path), &mut io::sink(), &mut io::sink());
        let want = Flags {
            demo: true,
            first_run: true,
        };
        assert_eq!(flags, Ok(want));
        assert_eq!(run(&["next"], Some(&path)).0, Some(0));
        assert_eq!(server.join().unwrap(), ["next"]);
        cleanup(&path);
    }

    fn key(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    /// Runs one frame with `events` and returns the shortcuts' actions.
    fn frame(
        ctx: &egui::Context,
        events: Vec<Event>,
        state: &State,
        view: &mut View,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        let input = RawInput {
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| out = shortcuts(ui.ctx(), state, view));
        output.textures_delta.clear();
        out
    }

    fn commands(actions: &[Action]) -> Vec<String> {
        actions
            .iter()
            .map(|a| match a {
                Action::Cmd(Command::SetVolume(v)) => format!("SetVolume({v:.2})"),
                Action::Cmd(c) => format!("{c:?}"),
                Action::Go(r) => format!("Go({r:?})"),
                Action::Guest(g) => format!("Guest({g})"),
            })
            .collect()
    }

    #[test]
    fn shortcut_mapping() {
        let ctx = egui::Context::default();
        let mut state = State::default();
        state.playback.volume = 0.8;
        let mut view = View::default();
        let ctrl = Modifiers::COMMAND;
        let events = vec![
            key(Key::Space, Modifiers::NONE, false),
            key(Key::ArrowRight, ctrl, false),
            key(Key::ArrowLeft, ctrl, false),
            key(Key::ArrowUp, ctrl, false),
            key(Key::ArrowUp, ctrl, true),
        ];
        let got = commands(&frame(&ctx, events, &state, &mut view));
        assert_eq!(got, ["TogglePlay", "Next", "Previous", "SetVolume(0.90)"]);

        // Held Space repeats do not toggle again; held Ctrl+Down keeps going.
        let events = vec![
            key(Key::Space, Modifiers::NONE, true),
            key(Key::ArrowDown, ctrl, false),
            key(Key::ArrowDown, ctrl, true),
        ];
        let got = commands(&frame(&ctx, events, &state, &mut view));
        assert_eq!(got, ["SetVolume(0.70)"]);

        // Clamped at the ends, snapped to 5% steps.
        state.playback.volume = 0.98;
        let got = frame(
            &ctx,
            vec![key(Key::ArrowUp, ctrl, false)],
            &state,
            &mut view,
        );
        assert_eq!(commands(&got), ["SetVolume(1.00)"]);
        state.playback.volume = 0.02;
        let got = frame(
            &ctx,
            vec![key(Key::ArrowDown, ctrl, false)],
            &state,
            &mut view,
        );
        assert_eq!(commands(&got), ["SetVolume(0.00)"]);

        // Plain arrows and Ctrl+Space are not shortcuts.
        let events = vec![
            key(Key::ArrowRight, Modifiers::NONE, false),
            key(Key::Space, ctrl, false),
        ];
        assert!(frame(&ctx, events, &state, &mut view).is_empty());
    }

    #[test]
    fn ctrl_f_focuses_the_filter() {
        let ctx = egui::Context::default();
        let state = State::default();
        let mut view = View {
            route: Route::Search,
            ..View::default()
        };
        let got = frame(
            &ctx,
            vec![key(Key::F, Modifiers::COMMAND, false)],
            &state,
            &mut view,
        );
        assert_eq!(commands(&got), ["Go(Collection)"]);
        assert!(view.collection.focus_filter);
    }

    #[test]
    fn ctrl_l_without_a_main_playlist_does_nothing() {
        let ctx = egui::Context::default();
        let state = playing(&["a"]);
        let mut view = View::default();
        let got = frame(
            &ctx,
            vec![key(Key::L, Modifiers::COMMAND, false)],
            &state,
            &mut view,
        );
        assert!(got.is_empty());
        assert_eq!(view.collection.scroll_to, None);
    }

    #[test]
    fn escape_clears_the_selection() {
        let ctx = egui::Context::default();
        let state = State::default();
        let mut view = View::default();
        view.select(egui::Id::NULL.with("table"), 3, "d");
        let got = frame(
            &ctx,
            vec![key(Key::Escape, Modifiers::NONE, false)],
            &state,
            &mut view,
        );
        assert!(got.is_empty());
        assert_eq!(view.selected, None);
        assert_eq!(view.selected_track, None);
    }

    #[test]
    fn text_focus_blocks_all_but_ctrl_q() {
        let ctx = egui::Context::default();
        let state = State::default();
        let mut view = View::default();
        let field = egui::Id::unique("field");
        let mut out = Vec::new();
        let input = RawInput {
            events: vec![
                key(Key::Space, Modifiers::NONE, false),
                key(Key::ArrowRight, Modifiers::COMMAND, false),
                key(Key::Q, Modifiers::COMMAND, false),
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            ui.memory_mut(|m| m.request_focus(field));
            ui.interact(
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0)),
                field,
                egui::Sense::click(),
            );
            out = shortcuts(ui.ctx(), &state, &mut view);
        });
        output.textures_delta.clear();
        assert!(out.is_empty());
        let close = output
            .viewport_output
            .values()
            .flat_map(|v| &v.commands)
            .filter(|c| matches!(c, ViewportCommand::Close))
            .count();
        assert_eq!(close, 1, "Ctrl+Q closes once");
    }
}
