//! The eframe app: holds the optimistic state, draws the shell, applies the
//! views' actions after drawing.

use std::time::{Duration, Instant};

use egui::{CentralPanel, Frame, Key, Margin, Panel, RichText};
use tokio::runtime::Handle;

use crate::backend::{self, Client, Keyring, Setup};
use crate::media_keys::MediaKeys;
use crate::playback::{self, SimulatedPlayback};
use crate::state::{Command, Session, State, apply};
use crate::theme;
use crate::views::{self, Action, Route, View};

pub struct RustleApp {
    client: Client,
    /// The newest backend snapshot with pending commands re-applied on top.
    state: State,
    /// Commands sent but not yet included in a snapshot, with their seq.
    pending: Vec<(u64, Command)>,
    seq: u64,
    view: View,
    cookies: String,
    /// The user chose to browse without logging in.
    guest: bool,
    /// Print `rustle: first frame` once the first frame is on screen.
    perf: bool,
    first_frame_done: bool,
    media_keys: MediaKeys,
}

impl RustleApp {
    /// Starts the backend task and returns at once. Building the real backend
    /// reads files and runs a helper binary, so it happens on `rt`.
    pub fn new(cc: &eframe::CreationContext<'_>, rt: &Handle, demo: bool, perf: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let notify: backend::Notify = Box::new(move || ctx.request_repaint());
        let mut initial = State::default();
        let client = if demo {
            initial.session = Session::LoggedIn;
            let setup = Setup {
                initial: initial.clone(),
                make: || Ok((ytm_core::FixtureBackend::new(), SimulatedPlayback::new())),
                store: Box::new(|| store::Store::open_in_memory().map_err(|e| e.to_string())),
                keyring: None,
            };
            backend::spawn(rt, setup, notify)
        } else {
            let setup = Setup {
                initial: initial.clone(),
                make: || {
                    let music = ytm_core::RustyPipeBackend::new()
                        .map_err(|e| format!("Could not start YouTube Music access: {e}"))?;
                    let audio = playback::system_playback()
                        .map_err(|e| format!("Could not open the audio output: {e}"))?;
                    Ok((music, audio))
                },
                store: Box::new(backend::open_library_db),
                keyring: Some(Keyring::SYSTEM),
            };
            backend::spawn(rt, setup, notify)
        };
        Self {
            client,
            state: initial,
            pending: Vec::new(),
            seq: 0,
            view: View::default(),
            cookies: String::new(),
            guest: false,
            perf,
            first_frame_done: false,
            media_keys: MediaKeys::new(cc.egui_ctx.clone()),
        }
    }

    /// Applies `command` locally at once and sends it to the backend.
    fn send(&mut self, command: Command) {
        if matches!(command, Command::Logout) {
            self.guest = false;
        }
        self.seq += 1;
        apply(&mut self.state, &command, Instant::now());
        self.client.send(self.seq, command.clone());
        self.pending.push((self.seq, command));
    }

    /// Takes the newest snapshot, if any, and replays what it has not seen.
    fn pull(&mut self) {
        let Some(snapshot) = self.client.latest() else {
            return;
        };
        self.pending.retain(|(seq, _)| *seq > snapshot.acked);
        self.state = (*snapshot).clone();
        let now = Instant::now();
        for (_, command) in &self.pending {
            apply(&mut self.state, command, now);
        }
    }

    fn act(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Cmd(command) => self.send(command),
                Action::Go(route) => {
                    if route == Route::Search && self.view.route != Route::Search {
                        self.view.focus_search = true;
                    }
                    self.view.route = route;
                }
            }
        }
    }

    /// The one-line message at the top of the window, if any.
    fn banner(&self, ui: &mut egui::Ui, out: &mut Vec<Action>) {
        let Some(banner) = &self.state.banner else {
            return;
        };
        Panel::top("banner")
            .frame(
                Frame::NONE
                    .fill(theme::ACCENT)
                    .inner_margin(Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(banner).color(egui::Color32::BLACK).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Dismiss").clicked() {
                            out.push(Action::Cmd(Command::DismissBanner));
                        }
                    });
                });
            });
    }

    /// Shown while the backend starts and checks the keyring.
    fn starting(ui: &mut egui::Ui) {
        CentralPanel::default()
            .frame(Frame::NONE.fill(theme::BG))
            .show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Loading…").color(theme::TEXT_DIM));
                });
            });
    }

    fn login(&mut self, ui: &mut egui::Ui) {
        let mut out = Vec::new();
        self.banner(ui, &mut out);
        let mut skip = false;
        CentralPanel::default()
            .frame(Frame::NONE.fill(theme::BG).inner_margin(Margin::same(24)))
            .show(ui, |ui| {
                views::login(ui, &self.state, &mut self.cookies, &mut out);
                ui.add_space(16.0);
                ui.vertical_centered(|ui| {
                    // Search and public playlists work without an account.
                    skip = ui
                        .link(RichText::new("Continue without logging in").color(theme::TEXT_DIM))
                        .clicked();
                });
            });
        if skip {
            self.guest = true;
        }
        self.act(out);
    }

    fn shell(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let mut out = Vec::new();
        self.banner(ui, &mut out);
        if self.state.session != Session::LoggedIn {
            let mut log_in = false;
            Panel::top("guest")
                .frame(
                    Frame::NONE
                        .fill(theme::SIDEBAR)
                        .inner_margin(Margin::symmetric(16, 6)),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Not logged in").color(theme::TEXT_DIM));
                        log_in = ui.link("Log in").clicked();
                    });
                });
            if log_in {
                self.guest = false;
            }
        }
        let state = &self.state;
        let view = &mut self.view;
        Panel::bottom("player")
            .exact_size(88.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(theme::BAR)
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| views::player_bar(ui, state, view, now, &mut out));
        Panel::left("nav")
            .exact_size(232.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(theme::SIDEBAR)
                    .inner_margin(Margin::symmetric(12, 12)),
            )
            .show(ui, |ui| views::sidebar(ui, state, view, &mut out));
        if view.show_queue {
            Panel::right("queue")
                .exact_size(300.0)
                .resizable(false)
                .show_separator_line(false)
                .frame(
                    Frame::NONE
                        .fill(theme::SIDEBAR)
                        .inner_margin(Margin::symmetric(12, 12)),
                )
                .show(ui, |ui| views::queue_panel(ui, state, &mut out));
        }
        CentralPanel::default()
            .frame(
                Frame::NONE
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(28, 22)),
            )
            .show(ui, |ui| views::page(ui, state, view, &mut out));

        let space = ui.input(|i| i.key_pressed(Key::Space));
        if space && !ui.ctx().egui_wants_keyboard_input() {
            out.push(Action::Cmd(Command::TogglePlay));
        }
        self.act(out);
    }
}

impl eframe::App for RustleApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.pull();
        let from_keys = self.media_keys.poll();
        for command in from_keys {
            self.send(command);
        }
        self.media_keys.sync(&self.state);
        match self.state.session {
            Session::LoggedIn => self.shell(ui),
            _ if self.guest => self.shell(ui),
            Session::Starting => Self::starting(ui),
            _ => self.login(ui),
        }

        // Repaint only on input or new state, plus once a second for the
        // progress bar while playing.
        if self.state.playback.is_playing() {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }

        if self.perf && !self.first_frame_done {
            // `cpu_usage` is `None` until a frame has been drawn.
            if frame.info().cpu_usage.is_some() {
                eprintln!("rustle: first frame");
                self.first_frame_done = true;
            } else {
                ui.ctx().request_repaint();
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }
}
