//! The screens. Views read the state and push [`Action`]s; the app applies
//! them after drawing, so no view mutates app state while it is borrowed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Align2, CornerRadius, FontId, Key, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui,
    UiBuilder, vec2,
};
use model::Track;

use crate::queue::Repeat;
use crate::state::{Command, Loadable, State};
use crate::theme;
mod login;
pub use login::login;

use crate::widgets::{
    ArtKind, art, card, entity_row, fmt_time, icon_button, round_button, section, seed, text_line,
    title,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    #[default]
    Home,
    Search,
    Library,
    Liked,
    Playlist(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchTab {
    #[default]
    Songs,
    Albums,
    Artists,
    Playlists,
}

pub enum Action {
    Cmd(Command),
    Go(Route),
}

/// UI-only state: where the user is and what they are doing with the mouse.
pub struct View {
    pub route: Route,
    pub search_text: String,
    pub search_tab: SearchTab,
    pub focus_search: bool,
    pub show_queue: bool,
    /// Seek bar value while the user drags it, in seconds.
    pub seek_drag: Option<f32>,
    /// Volume to restore when unmuting.
    pub unmuted_volume: f32,
    /// Selected row: list id and index.
    pub selected: Option<(egui::Id, usize)>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            route: Route::Home,
            search_text: String::new(),
            search_tab: SearchTab::Songs,
            focus_search: false,
            show_queue: true,
            seek_drag: None,
            unmuted_volume: 0.8,
            selected: None,
        }
    }
}

pub type Out = Vec<Action>;

fn cmd(out: &mut Out, c: Command) {
    out.push(Action::Cmd(c));
}

fn open_playlist(out: &mut Out, id: &str) {
    cmd(out, Command::OpenPlaylist(id.to_owned()));
    out.push(Action::Go(Route::Playlist(id.to_owned())));
}

fn artists(track: &Track) -> String {
    let names: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
    names.join(", ")
}

// ---------------------------------------------------------------- player bar

pub fn player_bar(ui: &mut Ui, state: &State, view: &mut View, now: Instant, out: &mut Out) {
    let rect = ui.max_rect();
    let w = rect.width();
    let left = Rect::from_min_max(rect.min, Pos2::new(rect.left() + w * 0.3, rect.bottom()));
    let right = Rect::from_min_max(Pos2::new(rect.right() - w * 0.3, rect.top()), rect.max);
    let center = Rect::from_min_max(
        Pos2::new(left.right(), rect.top()),
        Pos2::new(right.left(), rect.bottom()),
    );
    now_playing(ui, left, state);
    controls(ui, center, state, view, now, out);
    extras(ui, right, state, view, out);
}

fn now_playing(ui: &mut Ui, rect: Rect, state: &State) {
    let painter = ui.painter_at(rect);
    let Some(track) = state.current_track() else {
        text_line(
            &painter,
            Pos2::new(rect.left(), rect.center().y),
            "Nothing playing",
            FontId::proportional(13.0),
            theme::TEXT_FAINT,
            rect.width(),
        );
        return;
    };
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - 28.0),
        vec2(56.0, 56.0),
    );
    art(
        &painter,
        art_rect,
        &track.id,
        &track.title,
        ArtKind::Square(&track.thumbnails),
    );
    let x = art_rect.right() + 12.0;
    let w = rect.right() - x - 12.0;
    text_line(
        &painter,
        Pos2::new(x, rect.center().y - 9.0),
        &track.title,
        FontId::proportional(14.5),
        theme::TEXT,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.center().y + 11.0),
        &artists(track),
        FontId::proportional(12.5),
        theme::TEXT_DIM,
        w,
    );
}

fn controls(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, now: Instant, out: &mut Out) {
    let cx = rect.center().x;
    let cy = rect.top() + 20.0;
    let at = |dx: f32| Rect::from_center_size(Pos2::new(cx + dx, cy), vec2(30.0, 30.0));
    let queue = &state.queue;

    if icon_button(
        ui,
        at(-92.0),
        "shuffle",
        "🔀",
        16.0,
        queue.shuffle(),
        "Shuffle",
    )
    .clicked()
    {
        let on = !queue.shuffle();
        cmd(out, Command::SetShuffle { on, seed: seed() });
    }
    if icon_button(ui, at(-46.0), "prev", "⏮", 18.0, false, "Previous").clicked() {
        cmd(out, Command::Previous);
    }
    let playing = state.playback.is_playing();
    let glyph = if playing { "⏸" } else { "⏵" };
    if round_button(ui, Pos2::new(cx, cy), 17.0, "play", glyph, theme::TEXT)
        .on_hover_text(if playing { "Pause" } else { "Play" })
        .clicked()
    {
        cmd(out, Command::TogglePlay);
    }
    if icon_button(ui, at(46.0), "next", "⏭", 18.0, false, "Next").clicked() {
        cmd(out, Command::Next);
    }
    let (rglyph, rtip) = match queue.repeat() {
        Repeat::Off => ("🔁", "Repeat"),
        Repeat::All => ("🔁", "Repeat all"),
        Repeat::One => ("🔂", "Repeat one"),
    };
    let ractive = queue.repeat() != Repeat::Off;
    if icon_button(ui, at(92.0), "repeat", rglyph, 16.0, ractive, rtip).clicked() {
        cmd(out, Command::CycleRepeat);
    }

    // Seek row.
    let y = rect.bottom() - 16.0;
    let half = (rect.width() * 0.5 - 56.0).clamp(60.0, 300.0);
    let bar = Rect::from_center_size(Pos2::new(cx, y), vec2(half * 2.0, 18.0));
    let duration = state.duration().unwrap_or_default();
    let shown = view.seek_drag.map_or_else(
        || state.playback.position_at(now).min(duration),
        Duration::from_secs_f32,
    );
    let font = FontId::proportional(11.5);
    let painter = ui.painter();
    painter.text(
        Pos2::new(bar.left() - 10.0, y),
        Align2::RIGHT_CENTER,
        fmt_time(shown),
        font.clone(),
        theme::TEXT_DIM,
    );
    painter.text(
        Pos2::new(bar.right() + 10.0, y),
        Align2::LEFT_CENTER,
        fmt_time(duration),
        font,
        theme::TEXT_DIM,
    );
    let max = duration.as_secs_f32().max(1.0);
    let mut value = shown.as_secs_f32();
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(bar), |ui| {
            ui.spacing_mut().slider_width = bar.width();
            ui.add_enabled(
                state.current_track().is_some(),
                egui::Slider::new(&mut value, 0.0..=max)
                    .show_value(false)
                    .trailing_fill(true)
                    .handle_shape(egui::style::HandleShape::Circle),
            )
        })
        .inner;
    if resp.dragged() {
        view.seek_drag = Some(value);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        view.seek_drag = None;
        cmd(out, Command::Seek(Duration::from_secs_f32(value)));
    }
}

fn extras(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, out: &mut Out) {
    let cy = rect.center().y;
    let slider = Rect::from_min_max(
        Pos2::new(rect.right() - 110.0, cy - 9.0),
        Pos2::new(rect.right() - 4.0, cy + 9.0),
    );
    let mut volume = state.playback.volume;
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(slider), |ui| {
            ui.spacing_mut().slider_width = slider.width();
            ui.add(
                egui::Slider::new(&mut volume, 0.0..=1.0)
                    .show_value(false)
                    .trailing_fill(true)
                    .handle_shape(egui::style::HandleShape::Circle),
            )
        })
        .inner;
    if resp.changed() {
        cmd(out, Command::SetVolume(volume));
    }

    let glyph = match state.playback.volume {
        v if v <= 0.0 => "🔇",
        v if v < 0.5 => "🔉",
        _ => "🔊",
    };
    let mute = Rect::from_center_size(Pos2::new(slider.left() - 18.0, cy), vec2(28.0, 28.0));
    if icon_button(ui, mute, "mute", glyph, 16.0, false, "Mute").clicked() {
        if state.playback.volume > 0.0 {
            view.unmuted_volume = state.playback.volume;
            cmd(out, Command::SetVolume(0.0));
        } else {
            cmd(out, Command::SetVolume(view.unmuted_volume.max(0.1)));
        }
    }
    let queue = Rect::from_center_size(Pos2::new(mute.left() - 22.0, cy), vec2(28.0, 28.0));
    if icon_button(ui, queue, "queue", "☰", 16.0, view.show_queue, "Queue").clicked() {
        view.show_queue = !view.show_queue;
    }
}

// ------------------------------------------------------------------- sidebar

pub fn sidebar(ui: &mut Ui, state: &State, view: &View, out: &mut Out) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 6.0, theme::ACCENT);
        ui.label(
            RichText::new("Rustle")
                .size(20.0)
                .strong()
                .color(theme::TEXT),
        );
    });
    ui.add_space(14.0);
    for (glyph, label, route) in [
        ("🏠", "Home", Route::Home),
        ("🔍", "Search", Route::Search),
        ("📚", "Your Library", Route::Library),
    ] {
        if nav_item(ui, glyph, label, view.route == route).clicked() {
            out.push(Action::Go(route));
        }
    }
    ui.add_space(12.0);
    ui.separator();
    section(ui, "PLAYLISTS");
    if nav_item(ui, "♥", "Liked Songs", view.route == Route::Liked).clicked() {
        out.push(Action::Go(Route::Liked));
    }
    let playlists = state.library.ready().cloned().unwrap_or_default();
    ScrollArea::vertical()
        .id_salt("sidebar-playlists")
        .auto_shrink([false, false])
        .show_rows(ui, 32.0, playlists.len(), |ui, range| {
            for p in &playlists[range] {
                let open = view.route == Route::Playlist(p.id.clone());
                if nav_item(ui, "♫", &p.name, open).clicked() {
                    open_playlist(out, &p.id);
                }
            }
        });
}

fn nav_item(ui: &mut Ui, glyph: &str, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), theme::HOVER);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::SURFACE);
    }
    let color = if selected || resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    painter.text(
        Pos2::new(rect.left() + 18.0, rect.center().y),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(15.0),
        if selected { theme::ACCENT } else { color },
    );
    text_line(
        &painter,
        Pos2::new(rect.left() + 38.0, rect.center().y),
        label,
        FontId::proportional(14.0),
        color,
        rect.width() - 44.0,
    );
    resp
}

// --------------------------------------------------------------------- queue

/// Drag payload for reordering the queue: the dragged entry's index.
struct QueueDrag(usize);

pub fn queue_panel(ui: &mut Ui, state: &State, out: &mut Out) {
    ui.add_space(8.0);
    ui.label(
        RichText::new("Queue")
            .size(18.0)
            .strong()
            .color(theme::TEXT),
    );
    ui.add_space(6.0);
    let q = &state.queue;
    section(ui, "NOW PLAYING");
    match q.current_index() {
        Some(i) => queue_row(ui, state, i, true, out),
        None => {
            ui.label(RichText::new("Nothing playing").color(theme::TEXT_FAINT));
        }
    }
    ui.add_space(8.0);
    let start = q.upcoming_start();
    let upcoming = if q.is_empty() {
        0
    } else {
        q.len().saturating_sub(start)
    };
    section(ui, &format!("NEXT UP · {upcoming}"));
    if upcoming == 0 {
        ui.label(
            RichText::new("Right-click a song and pick \"Add to queue\".").color(theme::TEXT_FAINT),
        );
        return;
    }
    ScrollArea::vertical()
        .id_salt("queue")
        .auto_shrink([false, false])
        .show_rows(ui, QUEUE_ROW, upcoming, |ui, range| {
            for k in range {
                queue_row(ui, state, start + k, false, out);
            }
        });
}

const QUEUE_ROW: f32 = 50.0;

fn queue_row(ui: &mut Ui, state: &State, index: usize, is_current: bool, out: &mut Out) {
    let q = &state.queue;
    let entry = &q.entries()[index];
    let sense = if is_current {
        Sense::click()
    } else {
        Sense::click_and_drag()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), QUEUE_ROW), sense);
    let painter = ui.painter_at(rect);
    if resp.hovered() || resp.dragged() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::HOVER);
    }
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 6.0, rect.center().y - 19.0),
        vec2(38.0, 38.0),
    );
    art(
        &painter,
        art_rect,
        &entry.track.id,
        &entry.track.title,
        ArtKind::Square(&entry.track.thumbnails),
    );
    let x = art_rect.right() + 10.0;
    let w = rect.right() - x - 34.0;
    let color = if is_current {
        theme::ACCENT
    } else {
        theme::TEXT
    };
    text_line(
        &painter,
        Pos2::new(x, rect.center().y - 8.0),
        &entry.track.title,
        FontId::proportional(13.5),
        color,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.center().y + 10.0),
        &artists(&entry.track),
        FontId::proportional(12.0),
        theme::TEXT_DIM,
        w,
    );

    if !is_current {
        resp.dnd_set_drag_payload(QueueDrag(index));
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let pointer_y = ui.ctx().pointer_interact_pos().map_or(0.0, |p| p.y);
        let above = pointer_y < rect.center().y;
        if resp.dnd_hover_payload::<QueueDrag>().is_some() {
            let y = if above { rect.top() } else { rect.bottom() };
            ui.painter()
                .hline(rect.x_range(), y, Stroke::new(2.0, theme::ACCENT));
        }
        if let Some(drag) = resp.dnd_release_payload::<QueueDrag>() {
            let slot = if above { index } else { index + 1 };
            let from = drag.0;
            let to = if from < slot { slot - 1 } else { slot };
            if from != to {
                cmd(out, Command::MoveEntry { from, to });
            }
        }
        let remove_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 16.0, rect.center().y),
            vec2(24.0, 24.0),
        );
        if (resp.hovered() || ui.rect_contains_pointer(remove_rect))
            && icon_button(
                ui,
                remove_rect,
                &format!("rm{index}"),
                "✕",
                13.0,
                false,
                "Remove",
            )
            .clicked()
        {
            cmd(out, Command::RemoveEntry(index));
        }
    }
    if resp.double_clicked() {
        cmd(out, Command::JumpTo(index));
    }
    let start = q.upcoming_start();
    let last = q.len().saturating_sub(1);
    resp.context_menu(|ui| {
        if ui.button("Play now").clicked() {
            cmd(out, Command::JumpTo(index));
            ui.close();
        }
        if !is_current {
            if index > start && ui.button("Move up").clicked() {
                cmd(
                    out,
                    Command::MoveEntry {
                        from: index,
                        to: index - 1,
                    },
                );
                ui.close();
            }
            if index < last && ui.button("Move down").clicked() {
                cmd(
                    out,
                    Command::MoveEntry {
                        from: index,
                        to: index + 1,
                    },
                );
                ui.close();
            }
            if index > start && ui.button("Move to top").clicked() {
                cmd(
                    out,
                    Command::MoveEntry {
                        from: index,
                        to: start,
                    },
                );
                ui.close();
            }
        }
        if ui.button("Remove from queue").clicked() {
            cmd(out, Command::RemoveEntry(index));
            ui.close();
        }
    });
}

// ----------------------------------------------------------------- track list

const TRACK_ROW: f32 = 50.0;
const HOME_SHELF: usize = 8;

/// The standard song table: number, title and artist, album, duration.
/// Double-click plays the list from that row.
fn track_table(
    ui: &mut Ui,
    salt: &str,
    tracks: &Arc<Vec<Arc<Track>>>,
    state: &State,
    view: &mut View,
    out: &mut Out,
) {
    let list_id = ui.scope_id().with(salt);
    let full = ui.available_rect_before_wrap();
    let cols = Cols::new(full.left(), full.width());
    let (head, _) = ui.allocate_exact_size(vec2(full.width(), 26.0), Sense::hover());
    let painter = ui.painter();
    let font = FontId::proportional(12.0);
    for (x, text, align) in [
        (cols.index_center, "#", Align2::CENTER_CENTER),
        (cols.title, "Title", Align2::LEFT_CENTER),
    ] {
        painter.text(
            Pos2::new(x, head.center().y),
            align,
            text,
            font.clone(),
            theme::TEXT_FAINT,
        );
    }
    if let Some(album) = cols.album {
        painter.text(
            Pos2::new(album, head.center().y),
            Align2::LEFT_CENTER,
            "Album",
            font.clone(),
            theme::TEXT_FAINT,
        );
    }
    painter.text(
        Pos2::new(cols.duration_right, head.center().y),
        Align2::RIGHT_CENTER,
        "Time",
        font,
        theme::TEXT_FAINT,
    );
    painter.hline(
        full.x_range(),
        head.bottom(),
        Stroke::new(1.0, theme::HOVER),
    );
    ui.add_space(4.0);

    if tracks.is_empty() {
        ui.label(RichText::new("No songs here yet.").color(theme::TEXT_FAINT));
        return;
    }
    let current = state.current_track().map(|t| t.id.clone());
    let playing = state.playback.is_playing();
    ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show_rows(ui, TRACK_ROW, tracks.len(), |ui, range| {
            for i in range {
                let track = &tracks[i];
                let selected = view.selected == Some((list_id, i));
                let is_current = current.as_deref() == Some(track.id.as_str());
                let resp = track_row(ui, &cols, i, track, selected, is_current, playing);
                if resp.clicked() {
                    view.selected = Some((list_id, i));
                }
                if resp.double_clicked() {
                    cmd(
                        out,
                        Command::PlayTracks {
                            tracks: Arc::clone(tracks),
                            start: i,
                            seed: seed(),
                        },
                    );
                }
                resp.context_menu(|ui| track_menu(ui, track, out));
            }
        });
}

/// Column x positions for the song table.
struct Cols {
    index_center: f32,
    art: f32,
    title: f32,
    title_width: f32,
    album: Option<f32>,
    album_width: f32,
    duration_right: f32,
}

impl Cols {
    fn new(left: f32, width: f32) -> Self {
        let duration_right = left + width - 16.0;
        let show_album = width > 560.0;
        let album_width = if show_album {
            (width * 0.3).min(320.0)
        } else {
            0.0
        };
        let album_left = duration_right - 64.0 - album_width;
        let title = left + 96.0;
        Self {
            index_center: left + 22.0,
            art: left + 46.0,
            title,
            title_width: if show_album {
                album_left - title - 16.0
            } else {
                duration_right - 64.0 - title
            },
            album: show_album.then_some(album_left),
            album_width: album_width - 16.0,
            duration_right,
        }
    }
}

fn track_row(
    ui: &mut Ui,
    cols: &Cols,
    index: usize,
    track: &Track,
    selected: bool,
    is_current: bool,
    playing: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), TRACK_ROW), Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), theme::SELECTED);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::HOVER);
    }
    let cy = rect.center().y;
    let (marker, color) = if is_current {
        (
            if playing {
                "🔊".to_owned()
            } else {
                "⏸".to_owned()
            },
            theme::ACCENT,
        )
    } else if resp.hovered() {
        ("⏵".to_owned(), theme::TEXT)
    } else {
        ((index + 1).to_string(), theme::TEXT_DIM)
    };
    painter.text(
        Pos2::new(cols.index_center, cy),
        Align2::CENTER_CENTER,
        marker,
        FontId::proportional(13.0),
        color,
    );
    let art_rect = Rect::from_min_size(Pos2::new(cols.art, cy - 19.0), vec2(38.0, 38.0));
    art(
        &painter,
        art_rect,
        &track.id,
        &track.title,
        ArtKind::Square(&track.thumbnails),
    );
    let title_color = if is_current {
        theme::ACCENT
    } else {
        theme::TEXT
    };
    text_line(
        &painter,
        Pos2::new(cols.title, cy - 9.0),
        &track.title,
        FontId::proportional(14.5),
        title_color,
        cols.title_width,
    );
    text_line(
        &painter,
        Pos2::new(cols.title, cy + 10.0),
        &artists(track),
        FontId::proportional(12.5),
        theme::TEXT_DIM,
        cols.title_width,
    );
    if let (Some(x), Some(album)) = (cols.album, &track.album) {
        text_line(
            &painter,
            Pos2::new(x, cy),
            &album.name,
            FontId::proportional(13.0),
            theme::TEXT_DIM,
            cols.album_width,
        );
    }
    if let Some(d) = track.duration {
        painter.text(
            Pos2::new(cols.duration_right, cy),
            Align2::RIGHT_CENTER,
            fmt_time(d),
            FontId::proportional(13.0),
            theme::TEXT_DIM,
        );
    }
    resp
}

fn track_menu(ui: &mut Ui, track: &Arc<Track>, out: &mut Out) {
    if ui.button("Play now").clicked() {
        cmd(out, Command::PlayNow(Arc::clone(track)));
        ui.close();
    }
    if ui.button("Play next").clicked() {
        cmd(out, Command::PlayNext(Arc::clone(track)));
        ui.close();
    }
    if ui.button("Add to queue").clicked() {
        cmd(out, Command::AddToQueue(Arc::clone(track)));
        ui.close();
    }
}

// --------------------------------------------------------------------- pages

pub fn page(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    match view.route.clone() {
        Route::Home => home(ui, state, view, out),
        Route::Search => search(ui, state, view, out),
        Route::Library => library(ui, state, out),
        Route::Liked => liked(ui, state, view, out),
        Route::Playlist(id) => playlist(ui, state, view, &id, out),
    }
}

fn loading_or_error<T>(ui: &mut Ui, value: &Loadable<T>, what: &str) {
    let text = match value {
        Loadable::Failed(e) => format!("Could not load {what}: {e}"),
        _ => "Loading…".to_owned(),
    };
    ui.label(RichText::new(text).color(theme::TEXT_FAINT));
}

fn home(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    title(ui, "Home");
    ui.add_space(10.0);
    ui.label(
        RichText::new("Your playlists")
            .size(18.0)
            .strong()
            .color(theme::TEXT),
    );
    ui.add_space(6.0);
    let liked_count = state.liked.ready().map_or(0, |l| l.len());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
        if card(
            ui,
            "liked",
            "Liked Songs",
            &format!("{liked_count} songs"),
            ArtKind::Liked,
        )
        .clicked()
        {
            out.push(Action::Go(Route::Liked));
        }
        if let Some(playlists) = state.library.ready() {
            // A short shelf; the Library page lists everything with show_rows.
            for p in playlists.iter().take(HOME_SHELF) {
                let sub = p.owner.as_deref().unwrap_or("Playlist");
                if card(ui, &p.id, &p.name, sub, ArtKind::Square(&p.thumbnails)).clicked() {
                    open_playlist(out, &p.id);
                }
            }
        }
    });
    ui.add_space(18.0);
    ui.label(
        RichText::new("Liked songs")
            .size(18.0)
            .strong()
            .color(theme::TEXT),
    );
    ui.add_space(6.0);
    match &state.liked {
        Loadable::Ready(tracks) => track_table(ui, "home-liked", tracks, state, view, out),
        other => loading_or_error(ui, other, "liked songs"),
    }
}

fn search(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    title(ui, "Search");
    ui.add_space(8.0);
    let resp = ui.add(
        egui::TextEdit::singleline(&mut view.search_text)
            .hint_text("What do you want to listen to?")
            .desired_width(420.0)
            .margin(vec2(12.0, 8.0))
            .font(FontId::proportional(15.0)),
    );
    if view.focus_search {
        resp.request_focus();
        view.focus_search = false;
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        cmd(out, Command::Search(view.search_text.clone()));
    }
    ui.add_space(10.0);

    let view_data = state.search.ready();
    let count = |tab: SearchTab| {
        view_data.map_or(0, |v| match tab {
            SearchTab::Songs => v.tracks.len(),
            SearchTab::Albums => v.results.albums.len(),
            SearchTab::Artists => v.results.artists.len(),
            SearchTab::Playlists => v.results.playlists.len(),
        })
    };
    ui.horizontal(|ui| {
        for (tab, label) in [
            (SearchTab::Songs, "Songs"),
            (SearchTab::Albums, "Albums"),
            (SearchTab::Artists, "Artists"),
            (SearchTab::Playlists, "Playlists"),
        ] {
            let text = if view_data.is_some() {
                format!("{label}  {}", count(tab))
            } else {
                label.to_owned()
            };
            ui.selectable_value(&mut view.search_tab, tab, text);
        }
    });
    ui.add_space(8.0);

    let data = match &state.search {
        Loadable::Idle => {
            ui.label(
                RichText::new("Search for songs, albums, artists and playlists, then press Enter.")
                    .color(theme::TEXT_FAINT),
            );
            return;
        }
        Loadable::Loading => {
            ui.label(RichText::new("Searching…").color(theme::TEXT_FAINT));
            return;
        }
        Loadable::Failed(e) => {
            ui.label(RichText::new(format!("Search failed: {e}")).color(theme::ACCENT));
            return;
        }
        Loadable::Ready(v) => Arc::clone(v),
    };
    if count(view.search_tab) == 0 {
        ui.label(
            RichText::new(format!("No results for \"{}\".", state.search_query))
                .color(theme::TEXT_FAINT),
        );
        return;
    }
    let r = &data.results;
    match view.search_tab {
        SearchTab::Songs => track_table(ui, "search-songs", &data.tracks, state, view, out),
        SearchTab::Albums => rows(ui, "search-albums", r.albums.len(), |ui, i| {
            let a = &r.albums[i];
            let by: Vec<&str> = a.artists.iter().map(|x| x.name.as_str()).collect();
            let year = a.year.map(|y| format!(" · {y}")).unwrap_or_default();
            let sub = format!("Album · {}{year}", by.join(", "));
            entity_row(
                ui,
                60.0,
                &a.id,
                &a.name,
                &sub,
                ArtKind::Square(&a.thumbnails),
                false,
            )
            .on_hover_text("Album pages are not available yet");
        }),
        SearchTab::Artists => rows(ui, "search-artists", r.artists.len(), |ui, i| {
            let a = &r.artists[i];
            entity_row(
                ui,
                60.0,
                &a.id,
                &a.name,
                "Artist",
                ArtKind::Round(&a.thumbnails),
                false,
            )
            .on_hover_text("Artist pages are not available yet");
        }),
        SearchTab::Playlists => rows(ui, "search-playlists", r.playlists.len(), |ui, i| {
            let p = &r.playlists[i];
            let sub = format!(
                "Playlist · {}",
                p.owner.as_deref().unwrap_or("YouTube Music")
            );
            if entity_row(
                ui,
                60.0,
                &p.id,
                &p.name,
                &sub,
                ArtKind::Square(&p.thumbnails),
                false,
            )
            .clicked()
            {
                open_playlist(out, &p.id);
            }
        }),
    }
}

fn rows(ui: &mut Ui, salt: &str, n: usize, mut row: impl FnMut(&mut Ui, usize)) {
    ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show_rows(ui, 60.0, n, |ui, range| {
            for i in range {
                row(ui, i);
            }
        });
}

fn library(ui: &mut Ui, state: &State, out: &mut Out) {
    title(ui, "Your Library");
    ui.add_space(10.0);
    let liked = state.liked.ready().map_or(0, |l| l.len());
    let playlists = match &state.library {
        Loadable::Ready(p) => Arc::clone(p),
        other => {
            loading_or_error(ui, other, "your library");
            return;
        }
    };
    rows(ui, "library", playlists.len() + 1, |ui, i| {
        if i == 0 {
            let sub = format!("Playlist · {liked} songs");
            if entity_row(
                ui,
                60.0,
                "liked",
                "Liked Songs",
                &sub,
                ArtKind::Liked,
                false,
            )
            .clicked()
            {
                out.push(Action::Go(Route::Liked));
            }
            return;
        }
        let p = &playlists[i - 1];
        let mut sub = format!("Playlist · {}", p.owner.as_deref().unwrap_or("You"));
        if let Some(n) = p.track_count {
            sub.push_str(&format!(" · {n} songs"));
        }
        if entity_row(
            ui,
            60.0,
            &p.id,
            &p.name,
            &sub,
            ArtKind::Square(&p.thumbnails),
            false,
        )
        .clicked()
        {
            open_playlist(out, &p.id);
        }
    });
}

/// The header above a playlist: cover, name, details, play buttons.
fn page_header(
    ui: &mut Ui,
    key: &str,
    name: &str,
    detail: &str,
    kind: ArtKind<'_>,
    tracks: Option<&Arc<Vec<Arc<Track>>>>,
    out: &mut Out,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 168.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let art_rect = Rect::from_min_size(rect.min, vec2(160.0, 160.0));
    art(&painter, art_rect, key, name, kind);
    let x = art_rect.right() + 24.0;
    let w = rect.right() - x;
    text_line(
        &painter,
        Pos2::new(x, rect.top() + 56.0),
        "PLAYLIST",
        FontId::proportional(11.5),
        theme::TEXT_DIM,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.top() + 92.0),
        name,
        FontId::proportional(40.0),
        theme::TEXT,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.top() + 132.0),
        detail,
        FontId::proportional(13.0),
        theme::TEXT_DIM,
        w,
    );
    let Some(tracks) = tracks.filter(|t| !t.is_empty()) else {
        return;
    };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let play_center = Pos2::new(row.left() + 28.0, row.center().y);
    if round_button(ui, play_center, 26.0, "page-play", "⏵", theme::ACCENT)
        .on_hover_text("Play")
        .clicked()
    {
        cmd(
            out,
            Command::PlayTracks {
                tracks: Arc::clone(tracks),
                start: 0,
                seed: seed(),
            },
        );
    }
    let shuffle = Rect::from_center_size(
        Pos2::new(row.left() + 84.0, row.center().y),
        vec2(36.0, 36.0),
    );
    if icon_button(
        ui,
        shuffle,
        "page-shuffle",
        "🔀",
        22.0,
        false,
        "Shuffle play",
    )
    .clicked()
    {
        let s = seed();
        cmd(out, Command::SetShuffle { on: true, seed: s });
        cmd(
            out,
            Command::PlayTracks {
                tracks: Arc::clone(tracks),
                start: (s % tracks.len() as u64) as usize,
                seed: s,
            },
        );
    }
}

fn liked(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    let tracks = state.liked.ready();
    let detail = format!("You · {} songs", tracks.map_or(0, |t| t.len()));
    page_header(
        ui,
        "liked",
        "Liked Songs",
        &detail,
        ArtKind::Liked,
        tracks,
        out,
    );
    match &state.liked {
        Loadable::Ready(tracks) => track_table(ui, "liked", tracks, state, view, out),
        other => loading_or_error(ui, other, "liked songs"),
    }
}

fn playlist(ui: &mut Ui, state: &State, view: &mut View, id: &str, out: &mut Out) {
    let Some((open, page)) = state.playlist.as_ref().filter(|(open, _)| open == id) else {
        loading_or_error::<()>(ui, &Loadable::Loading, "this playlist");
        return;
    };
    let Loadable::Ready(p) = page else {
        // While loading, show what the library already knows.
        let name = state
            .library
            .ready()
            .and_then(|l| l.iter().find(|s| &s.id == open).map(|s| s.name.clone()))
            .unwrap_or_default();
        page_header(ui, open, &name, "", ArtKind::Square(&[]), None, out);
        loading_or_error(ui, page, "this playlist");
        return;
    };
    let mut detail = p
        .summary
        .owner
        .clone()
        .unwrap_or_else(|| "YouTube Music".into());
    detail.push_str(&format!(" · {} songs", p.tracks.len()));
    let total: Duration = p.tracks.iter().filter_map(|t| t.duration).sum();
    if !total.is_zero() {
        detail.push_str(&format!(" · {}", fmt_time(total)));
    }
    page_header(
        ui,
        &p.summary.id,
        &p.summary.name,
        &detail,
        ArtKind::Square(&p.summary.thumbnails),
        Some(&p.tracks),
        out,
    );
    track_table(ui, &format!("pl-{id}"), &p.tracks, state, view, out);
}
