//! The eframe app: holds the optimistic state, draws the shell, applies the
//! views' actions after drawing.

use std::time::{Duration, Instant};

use egui::{CentralPanel, Frame, Key, Margin, Panel, RichText};
use tokio::runtime::Handle;

use crate::backend::{self, Client};
use crate::playback::SimulatedPlayback;
use crate::state::{Command, State, apply};
use crate::theme;
use crate::views::{self, Action, Route, View};

pub struct RustleApp {
    /// `None` until the user is logged in; then the login screen shows.
    client: Option<Client>,
    /// The newest backend snapshot with pending commands re-applied on top.
    state: State,
    /// Commands sent but not yet included in a snapshot, with their seq.
    pending: Vec<(u64, Command)>,
    seq: u64,
    view: View,
    cookies: String,
    /// Print `rustle: first frame` once the first frame is on screen.
    perf: bool,
    first_frame_done: bool,
}

impl RustleApp {
    pub fn new(cc: &eframe::CreationContext<'_>, demo: Option<&Handle>, perf: bool) -> Self {
        theme::apply(&cc.egui_ctx);
        let client = demo.map(|rt| {
            let ctx = cc.egui_ctx.clone();
            backend::spawn(
                rt,
                ytm_core::FixtureBackend::new(),
                SimulatedPlayback::new(),
                Box::new(move || ctx.request_repaint()),
            )
        });
        let mut app = Self {
            client,
            state: State::default(),
            pending: Vec::new(),
            seq: 0,
            view: View::default(),
            cookies: String::new(),
            perf,
            first_frame_done: false,
        };
        if app.client.is_some() {
            app.send(Command::Refresh);
        }
        app
    }

    /// Applies `command` locally at once and sends it to the backend.
    fn send(&mut self, command: Command) {
        let Some(client) = &self.client else {
            return;
        };
        self.seq += 1;
        apply(&mut self.state, &command, Instant::now());
        client.send(self.seq, command.clone());
        self.pending.push((self.seq, command));
    }

    /// Takes the newest snapshot, if any, and replays what it has not seen.
    fn pull(&mut self) {
        let Some(snapshot) = self.client.as_mut().and_then(Client::latest) else {
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

    fn shell(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let mut out = Vec::new();
        let state = &self.state;
        let view = &mut self.view;

        if let Some(banner) = &state.banner {
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
        if self.client.is_some() {
            self.shell(ui);
        } else {
            CentralPanel::default()
                .frame(Frame::NONE.fill(theme::BG).inner_margin(Margin::same(24)))
                .show(ui, |ui| views::login(ui, &mut self.cookies));
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
