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
//! Integrator stub. The `control` worker owns this file.

use std::path::PathBuf;
use std::process::ExitCode;

use egui::Key;
use tokio::runtime::Handle;

use crate::state::{Command, State};
use crate::views::{Action, View};

/// The control socket for this instance.
#[allow(dead_code)] // b4-control
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

/// The socket server. Dropping it stops serving and removes the socket.
pub struct Control {}

impl Control {
    /// Starts serving on `rt`. Never blocks the caller; a socket that
    /// another live instance answers on is left alone (log once).
    pub fn start(_rt: &Handle, _ctx: egui::Context, _demo: bool) -> Self {
        // b4-control
        Self {}
    }

    /// Commands received since the last call. `raise` is handled here
    /// (`ViewportCommand::Focus`) and not returned.
    pub fn poll(&mut self) -> Vec<Command> {
        // b4-control
        Vec::new()
    }

    /// Keeps the `status` reply current. Called every frame; cheap when
    /// nothing changed.
    pub fn publish(&mut self, _state: &State) {
        // b4-control
    }
}

/// In-window shortcuts for this frame. Keys are ignored while a text field
/// has focus.
pub fn shortcuts(ctx: &egui::Context, _state: &State, _view: &mut View) -> Vec<Action> {
    // b4-control: the full set; Space is the existing play/pause.
    let mut out = Vec::new();
    if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Space)) {
        out.push(Action::Cmd(Command::TogglePlay));
    }
    out
}

/// Handles `rustle play|pause|toggle|next|previous|status` and the single
/// instance check. `None` means: start the app. Arguments starting with
/// `--` (`--demo`, `--first-run`) always start the app.
pub fn run_cli(_args: &[String]) -> Option<ExitCode> {
    // b4-control
    None
}
