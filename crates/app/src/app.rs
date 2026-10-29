//! The eframe app: holds the optimistic state, draws the shell, applies the
//! views' actions after drawing.

use std::time::{Duration, Instant};

use egui::{Area, CentralPanel, Frame, Margin, Order, Panel, Pos2, Rect, RichText, UiBuilder};
use tokio::runtime::Handle;

use crate::backend::{self, Client, Keyring, Setup};
use crate::control::{self, Control};
use crate::media_keys::MediaKeys;
use crate::playback::{self, SimulatedPlayback};
use crate::state::{Command, Session, State, apply};
use crate::views::{self, Action, Route, View};
use crate::{theme, widgets};

/// The same 10 minutes as the store's staleness rule.
const FOCUS_REFRESH_EVERY: Duration = Duration::from_secs(10 * 60);

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
    /// Whether the window had focus last frame, to catch focus gains.
    focused: bool,
    /// When focus last asked for a refresh; quick window switches skip it.
    focus_refresh_at: Instant,
    /// Art colors known at the last `rustle: art colors` line (perf only).
    colors_reported: usize,
    media_keys: MediaKeys,
    control: Control,
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
            let large = demo_large();
            let setup = Setup {
                initial: initial.clone(),
                make: move || {
                    let music = match large {
                        Some((n, art)) => ytm_core::FixtureBackend::large(n, &art),
                        None => ytm_core::FixtureBackend::new(),
                    };
                    Ok((music, SimulatedPlayback::new()))
                },
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
            focused: true,
            focus_refresh_at: Instant::now(),
            colors_reported: 0,
            media_keys: MediaKeys::new(cc.egui_ctx.clone()),
            control: Control::start(rt, cc.egui_ctx.clone(), demo),
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
                // The lyrics and queue drawers share one slot.
                Action::Cmd(Command::ShowLyrics(true)) => {
                    self.view.show_queue = false;
                    self.send(Command::ShowLyrics(true));
                }
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
        views::banners::banner(ui, &self.state, &mut out);
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
        view.follow_playback(state);
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
            .show(ui, |ui| {
                views::rail::show(ui, state, view, &mut out);
                let anchor = views::rail::settings_button(ui);
                views::settings::show(ui, &anchor, state, view, &mut out);
            });
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

        // The queue drawer opened while lyrics were open: close the lyrics.
        if view.show_queue && state.lyrics_open {
            out.push(Action::Cmd(Command::ShowLyrics(false)));
        }
        if state.lyrics_open && !view.show_queue {
            let drawer = Rect::from_min_max(
                Pos2::new(content.right() - theme::QUEUE_W, content.top()),
                content.max,
            );
            Area::new(egui::Id::unique("lyrics-drawer"))
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
                            views::lyrics::show(ui, state, now, &mut out);
                        });
                });
        }
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

        out.extend(control::shortcuts(ui.ctx(), state, view));
        self.act(out);
    }
}

/// Largest texture side the app asks egui for; see `raw_input_hook`.
const MAX_TEXTURE_SIDE: usize = 2048;

/// `RUSTLE_DEMO_LARGE=<tracks>` with `RUSTLE_DEMO_ART=<url>`: `--demo` runs
/// on a generated library whose art comes from a local server. Set by
/// `scripts/perf-check.sh --large`.
fn demo_large() -> Option<(usize, String)> {
    let n = std::env::var("RUSTLE_DEMO_LARGE").ok()?.parse().ok()?;
    let art = std::env::var("RUSTLE_DEMO_ART").ok()?;
    Some((n, art))
}

impl eframe::App for RustleApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.pull();
        // Coming back to the window picks up changes made elsewhere (phone,
        // browser); the store's 10-minute staleness rule keeps it cheap.
        let focused = ui.ctx().input(|i| i.viewport().focused).unwrap_or(true);
        if focused
            && !self.focused
            && self.state.session == Session::LoggedIn
            && self.focus_refresh_at.elapsed() >= FOCUS_REFRESH_EVERY
        {
            self.focus_refresh_at = Instant::now();
            self.send(Command::RefreshIfStale);
        }
        self.focused = focused;
        let from_keys = self.media_keys.poll();
        for command in from_keys {
            self.send(command);
        }
        self.media_keys.sync(&self.state);
        for command in self.control.poll() {
            self.send(command);
        }
        self.control.publish(&self.state);
        theme::set_palette_by_name(ui.ctx(), &self.state.settings.theme);
        let accent = self
            .state
            .current_track()
            .filter(|_| self.state.settings.accent_from_art)
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
        // perf-check.sh --large waits for the color pass with this line.
        if self.perf && self.state.art_colors.len() != self.colors_reported {
            self.colors_reported = self.state.art_colors.len();
            eprintln!("rustle: art colors {}", self.colors_reported);
        }
    }

    /// egui makes the font atlas as wide as the largest texture the GPU
    /// takes (16,384 px here), so even its first 32 rows cost 2 MiB, held
    /// twice: the CPU image and the texture. Every texture the app draws is
    /// far smaller than `MAX_TEXTURE_SIDE`.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.max_texture_side = Some(
            raw_input
                .max_texture_side
                .map_or(MAX_TEXTURE_SIDE, |s| s.min(MAX_TEXTURE_SIDE)),
        );
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::pal().bg.to_normalized_gamma_f32()
    }
}
