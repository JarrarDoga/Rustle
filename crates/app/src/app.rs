//! The eframe app: holds the optimistic state, draws the shell, applies the
//! views' actions after drawing.

use std::time::{Duration, Instant};

use egui::{Area, CentralPanel, Frame, Key, Margin, Order, Panel, Pos2, Rect, RichText, UiBuilder};
use tokio::runtime::Handle;

use crate::backend::{self, Client, Keyring, Setup};
use crate::media_keys::MediaKeys;
use crate::playback::{self, SimulatedPlayback};
use crate::state::{Command, Session, State, apply};
use crate::views::{self, Action, Route, View};
use crate::{theme, widgets};

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
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        rt: &Handle,
        demo: bool,
        first_run: bool,
        perf: bool,
    ) -> Self {
        theme::apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let notify: backend::Notify = Box::new(move || ctx.request_repaint());
        let mut initial = State::default();
        let client = if demo {
            initial.session = Session::LoggedIn;
            let setup = Setup {
                initial: initial.clone(),
                make: || Ok((ytm_core::FixtureBackend::new(), SimulatedPlayback::new())),
                store: Box::new(move || {
                    let s = store::Store::open_in_memory().map_err(|e| e.to_string())?;
                    // `--first-run` shows the main-playlist picker instead.
                    if !first_run {
                        s.set_main_playlist(ytm_core::DEMO_MAIN)
                            .map_err(|e| e.to_string())?;
                    }
                    Ok(s)
                }),
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
        // The login screen is gone once logged in; don't keep the pasted
        // cookies in memory after that.
        if self.state.session == Session::LoggedIn && !self.cookies.is_empty() {
            self.cookies.clear();
            self.cookies.shrink_to_fit();
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
                Action::Guest(guest) => self.guest = guest,
            }
        }
    }

    /// Shown while the backend starts and checks the keyring.
    fn starting(ui: &mut egui::Ui) {
        CentralPanel::default()
            .frame(Frame::NONE.fill(theme::pal().bg))
            .show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Loading…").color(theme::pal().text_2));
                });
            });
    }

    fn login(&mut self, ui: &mut egui::Ui) {
        let mut out = Vec::new();
        views::banners::show(ui, &self.state, &mut out);
        CentralPanel::default()
            .frame(
                Frame::NONE
                    .fill(theme::pal().bg)
                    .inner_margin(Margin::same(24)),
            )
            .show(ui, |ui| {
                views::login::login(ui, &self.state, &mut self.cookies, &mut out);
            });
        self.act(out);
    }

    /// Direction D shell: rail | browse panel (Collection only) | content |
    /// spine (Collection only), the player bar along the bottom, and the
    /// queue drawer over the right of the content.
    fn shell(&mut self, ui: &mut egui::Ui) {
        let now = Instant::now();
        let mut out = Vec::new();
        let p = *theme::pal();
        views::banners::show(ui, &self.state, &mut out);
        let state = &self.state;
        let view = &mut self.view;
        views::collection::update(state, view);
        let first_run = views::first_run::wanted(state);
        let collection = view.route == Route::Collection && !first_run;
        let region = |fill| Frame::NONE.fill(fill);

        Panel::bottom("player")
            .exact_size(theme::PLAYER_H)
            .resizable(false)
            .show_separator_line(true)
            .frame(region(theme::tint()).inner_margin(Margin::symmetric(8, 0)))
            .show(ui, |ui| {
                views::player_bar::show(ui, state, view, now, &mut out)
            });
        Panel::left("rail")
            .exact_size(theme::RAIL_W)
            .resizable(false)
            .show_separator_line(true)
            .frame(region(p.bg))
            .show(ui, |ui| views::rail::show(ui, state, view, &mut out));
        if collection {
            Panel::left("browse")
                .exact_size(theme::FACET_PANEL_W)
                .resizable(false)
                .show_separator_line(true)
                .frame(region(p.bg))
                .show(ui, |ui| views::browse::show(ui, state, view, &mut out));
        }
        let content = CentralPanel::default()
            .frame(region(p.bg))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if first_run {
                    let inner = rect.shrink(theme::SPACE_24);
                    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                        views::first_run::show(ui, state, &mut out);
                    });
                } else if collection {
                    // The collection gets the full width: its header and filter
                    // row span the spine column; its table leaves the right
                    // `SPINE_COLUMN_W` free for the spine.
                    let (_, spine) =
                        rect.split_left_right_at_x(rect.right() - theme::SPINE_COLUMN_W);
                    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                        views::collection::show(ui, state, view, &mut out);
                    });
                    ui.scope_builder(UiBuilder::new().max_rect(spine), |ui| {
                        widgets::spine::show(ui, state, view, &mut out);
                    });
                } else {
                    let inner = rect.shrink2(egui::vec2(theme::PAGE_INSET_X, theme::SPACE_16));
                    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                        views::page(ui, state, view, &mut out);
                    });
                }
                rect
            })
            .inner;

        if view.show_queue {
            let drawer = Rect::from_min_max(
                Pos2::new(content.right() - theme::QUEUE_W, content.top()),
                content.max,
            );
            Area::new(egui::Id::unique("queue-drawer"))
                .fixed_pos(drawer.min)
                .order(Order::Middle)
                .show(ui.ctx(), |ui| {
                    Frame::NONE
                        .fill(p.bg)
                        .stroke(theme::hairline())
                        .shadow(ui.visuals().window_shadow)
                        .show(ui, |ui| {
                            ui.set_min_size(drawer.size());
                            ui.set_max_size(drawer.size());
                            views::queue::show(ui, state, view, &mut out);
                        });
                });
        }

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
        let accent = self
            .state
            .current_track()
            .and_then(|t| self.state.art_colors.get(&t.id).copied());
        theme::set_art_accent(accent);
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
        theme::pal().bg.to_normalized_gamma_f32()
    }
}
