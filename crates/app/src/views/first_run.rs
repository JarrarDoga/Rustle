//! First run (D-Empty): pick your main playlist; nothing playing yet.
//!
//! The `onboarding` worker owns this file.

use egui::{Align2, Id, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, vec2};
use model::PlaylistSummary;

use crate::state::{Command, Session, State};
use crate::theme;
use crate::views::{Out, cmd};
use crate::widgets::{self, ArtKind};

/// The picker column's width (D-Empty).
const COLUMN_W: f32 = 560.0;
/// Space above the title: D-Empty's 72 px minus the content inset.
const TOP: f32 = 72.0 - theme::SPACE_24;
const ROW_H: f32 = 56.0;
const ART: f32 = 40.0;

/// Whether the content region shows the first-run picker instead of the
/// Collection. `app.rs` asks every frame. Must not flash on a normal start
/// before the saved main playlist id arrives from the store.
///
/// True only for a logged-in user with no main playlist once the library
/// list is in. Guests have no library; a library still loading or failed
/// shows nothing to pick; a chosen or cached main playlist wins.
pub fn wanted(state: &State) -> bool {
    state.session == Session::LoggedIn && state.main.is_none() && state.library.ready().is_some()
}

pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    let p = theme::pal();
    let width = COLUMN_W.min(ui.available_width());
    let left = ui.max_rect().center().x - width / 2.0;
    let column = Rect::from_min_max(
        egui::pos2(left, ui.max_rect().top() + TOP),
        egui::pos2(left + width, ui.max_rect().bottom()),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.label(
            RichText::new("Pick your main playlist")
                .font(theme::page_title())
                .color(p.text_1),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Rustle opens straight into this playlist. You can switch it later from Library.",
            )
            .font(body())
            .color(p.text_2),
        );
        ui.add_space(theme::SPACE_20);

        let playlists = state.library.ready().map(|l| ranked(l)).unwrap_or_default();
        if !playlists.is_empty() {
            list(ui, &playlists, out);
            ui.add_space(theme::SPACE_20);
        }
        ui.label(
            RichText::new(
                "No playlists yet? Like a few songs on YouTube Music and your Liked music will \
                 show up here.",
            )
            .font(theme::secondary())
            .color(p.text_3),
        );
    });
}

/// The playlists, largest first: the recommended main playlist leads.
fn ranked(library: &[PlaylistSummary]) -> Vec<&PlaylistSummary> {
    let mut v: Vec<&PlaylistSummary> = library.iter().collect();
    v.sort_by_key(|pl| std::cmp::Reverse(pl.track_count.unwrap_or(0)));
    v
}

/// The bordered list. The hovered row, or else the selected one (the first
/// until the user clicks another), is highlighted and shows "Use this".
fn list(ui: &mut Ui, playlists: &[&PlaylistSummary], out: &mut Out) {
    let selected_id = Id::NULL.with("rustle-first-run-selected");
    let selected = ui
        .data(|d| d.get_temp::<usize>(selected_id))
        .unwrap_or(0)
        .min(playlists.len() - 1);
    // Leave room for the footer line under the list.
    let footer = theme::SPACE_20 + 2.0 * theme::SPACE_20;
    let max_h = (ui.available_height() - footer).max(ROW_H * 3.0);
    let height = (ROW_H * playlists.len() as f32).min(max_h);
    let width = ui.available_width();
    let (frame, _) = ui.allocate_exact_size(vec2(width, height + 2.0), Sense::hover());
    let inner = frame.shrink(theme::STROKE_W);

    // The row under the pointer is highlighted; with the pointer elsewhere,
    // the selected one is.
    let pointer_in_list = ui.rect_contains_pointer(inner);
    let mut clicked_row = None;
    let mut chosen = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.set_clip_rect(inner);
        ScrollArea::vertical()
            .id_salt("first-run-list")
            .auto_shrink([false, false])
            .show_rows(ui, ROW_H, playlists.len(), |ui, range| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for i in range {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(inner.width(), ROW_H), Sense::click());
                    if resp.clicked() {
                        clicked_row = Some(i);
                    }
                    let lit = if pointer_in_list {
                        resp.hovered()
                    } else {
                        i == selected
                    };
                    if row(ui, rect, i, playlists[i], lit) {
                        chosen = Some(i);
                    }
                }
            });
    });
    if let Some(i) = clicked_row {
        ui.data_mut(|d| d.insert_temp(selected_id, i));
    }
    ui.painter().rect_stroke(
        frame,
        theme::RADIUS_CONTROL,
        theme::hairline(),
        StrokeKind::Inside,
    );
    if let Some(i) = chosen {
        cmd(out, Command::SetMainPlaylist(playlists[i].id.clone()));
    }
}

/// One 56 px row. Returns whether "Use this" was clicked.
fn row(ui: &mut Ui, rect: Rect, i: usize, pl: &PlaylistSummary, lit: bool) -> bool {
    let p = theme::pal();
    let painter = ui.painter();
    if lit {
        painter.rect_filled(rect, 0, p.surface_2);
    }
    if i > 0 {
        painter.hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(theme::STROKE_W, p.stroke),
        );
    }
    let pad = theme::SPACE_12;
    let art = Rect::from_min_size(
        egui::pos2(rect.left() + pad, rect.center().y - ART / 2.0),
        vec2(ART, ART),
    );
    widgets::art(
        painter,
        art,
        &pl.id,
        &pl.name,
        ArtKind::Square(&pl.thumbnails),
    );

    let mut used = false;
    let mut text_right = rect.right() - pad;
    if lit {
        let font = egui::FontId::new(12.0, theme::row_strong().family);
        let galley = painter.layout_no_wrap("Use this".to_owned(), font, p.on_accent);
        let size = vec2(galley.size().x + 2.0 * theme::SPACE_12, theme::ICON_BUTTON);
        let button = Rect::from_min_size(
            egui::pos2(rect.right() - pad - size.x, rect.center().y - size.y / 2.0),
            size,
        );
        let resp = ui
            .interact(button, ui.scope_id().with(("use-this", i)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let fill = if resp.is_pointer_button_down_on() {
            theme::accent_pressed()
        } else if resp.hovered() {
            theme::accent_hover()
        } else {
            theme::accent()
        };
        let painter = ui.painter();
        painter.rect_filled(button, theme::RADIUS_CONTROL, fill);
        let pos = Align2::CENTER_CENTER
            .align_size_within_rect(galley.size(), button)
            .min;
        painter.galley(pos, galley, p.on_accent);
        used = resp.clicked();
        text_right = button.left() - pad;
    }

    let x = art.right() + pad;
    let max = text_right - x;
    let painter = ui.painter();
    widgets::text_line(
        painter,
        egui::pos2(x, rect.center().y - 9.0),
        &pl.name,
        theme::row(),
        p.text_1,
        max,
    );
    widgets::text_line(
        painter,
        egui::pos2(x, rect.center().y + 9.0),
        &subtitle(pl),
        theme::secondary(),
        p.text_2,
        max,
    );
    used
}

/// "Playlist · 1,535 songs". Liked music is YouTube's auto playlist.
fn subtitle(pl: &PlaylistSummary) -> String {
    let kind = if pl.id == "LM" {
        "Auto playlist"
    } else {
        "Playlist"
    };
    match pl.track_count {
        Some(1) => format!("{kind} · 1 song"),
        Some(n) => format!("{kind} · {} songs", thousands(n)),
        None => kind.to_owned(),
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 13 / 400: the explanation line. The theme has 400 weight only at 12 px.
fn body() -> egui::FontId {
    egui::FontId::new(13.0, egui::FontFamily::Proportional)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use super::*;
    use crate::state::{Event, Loadable, OFFLINE, apply, on_event};

    fn summary(id: &str, count: Option<u64>) -> PlaylistSummary {
        PlaylistSummary {
            id: id.to_owned(),
            name: id.to_owned(),
            owner: None,
            track_count: count,
            thumbnails: Vec::new(),
        }
    }

    fn library() -> Loadable<Arc<Vec<PlaylistSummary>>> {
        Loadable::Ready(Arc::new(vec![summary("PLa", Some(3))]))
    }

    fn logged_in() -> State {
        State {
            session: Session::LoggedIn,
            ..State::default()
        }
    }

    #[test]
    fn fresh_profile_shows_the_picker_once_the_library_is_in() {
        let mut s = logged_in();
        assert!(!wanted(&s), "library not asked for yet");
        s.library = Loadable::Loading;
        assert!(!wanted(&s), "library loading");
        s.library = library();
        assert!(wanted(&s));
        // An empty library still shows the picker and its hint.
        s.library = Loadable::Ready(Arc::new(Vec::new()));
        assert!(wanted(&s));
    }

    #[test]
    fn failed_library_shows_no_picker() {
        let mut s = logged_in();
        s.library = Loadable::Failed("offline".into());
        assert!(!wanted(&s));
    }

    #[test]
    fn cached_main_playlist_wins() {
        let now = Instant::now();
        let mut s = logged_in();
        on_event(&mut s, Event::CachedMainPlaylist("PLmain".into()), now);
        s.library = library();
        assert!(!wanted(&s));
        // Offline start: cached library, cached main, offline banner.
        s.banner = Some(OFFLINE.to_owned());
        assert!(!wanted(&s));
    }

    #[test]
    fn picking_a_playlist_hides_the_picker() {
        let mut s = logged_in();
        s.library = library();
        assert!(wanted(&s));
        apply(
            &mut s,
            &Command::SetMainPlaylist("PLa".into()),
            Instant::now(),
        );
        assert!(!wanted(&s));
    }

    #[test]
    fn no_picker_without_a_session() {
        for session in [
            Session::Starting,
            Session::LoggedOut,
            Session::LoggingIn,
            Session::Failed("x".into()),
        ] {
            let s = State {
                session,
                library: library(),
                ..State::default()
            };
            assert!(!wanted(&s), "{:?}", s.session);
        }
    }

    #[test]
    fn largest_playlist_leads() {
        let lib = vec![
            summary("small", Some(45)),
            summary("unknown", None),
            summary("big", Some(1535)),
            summary("LM", Some(2212)),
        ];
        let ids: Vec<&str> = ranked(&lib).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["LM", "big", "small", "unknown"]);
    }

    #[test]
    fn subtitles() {
        assert_eq!(
            subtitle(&summary("PLx", Some(1535))),
            "Playlist · 1,535 songs"
        );
        assert_eq!(
            subtitle(&summary("LM", Some(2212))),
            "Auto playlist · 2,212 songs"
        );
        assert_eq!(subtitle(&summary("PLx", Some(1))), "Playlist · 1 song");
        assert_eq!(subtitle(&summary("PLx", None)), "Playlist");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }
}
