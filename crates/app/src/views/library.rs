//! Library: other playlists and liked songs. Rarely used in Direction D, so
//! it sits behind one rail icon. Also where the main playlist is changed.

use std::sync::Arc;

use egui::{Align2, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::state::{Command, Loadable, NEEDS_LOGIN, State};
use crate::views::album::{Art, LIST_INSET, ListRow, bleed, list_row, note, rule, songs};
use crate::views::{Action, Out, Route, cmd, loading_or_error, open_playlist, rows};
use crate::{icons, theme};

const MAKE_MAIN: &str = "Make this my main playlist";

pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    let p = theme::pal();
    let (head, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            theme::PAGE_HEADER_H - 2.0 * theme::SPACE_16,
        ),
        Sense::hover(),
    );
    // The page margin above plus this space below center the row in the
    // 64 px header.
    ui.add_space(theme::SPACE_16);
    ui.painter().text(
        Pos2::new(head.left(), head.center().y),
        Align2::LEFT_CENTER,
        "Library",
        theme::page_title(),
        p.text_1,
    );
    rule(ui);
    if state.needs_login() && state.library.ready().is_none() {
        return note(ui, NEEDS_LOGIN);
    }
    let playlists = match &state.library {
        Loadable::Ready(p) => Arc::clone(p),
        other => {
            ui.add_space(theme::SPACE_16);
            return loading_or_error(ui, other, "your library");
        }
    };
    let main_id = state.main.as_ref().map(|(id, _)| id.as_str());
    let liked = state.liked.ready().map_or_else(
        || "Playlist".to_owned(),
        |l| format!("Playlist · {}", songs(l.len())),
    );
    ui.add_space(theme::SPACE_8);
    bleed(ui, LIST_INSET.0, LIST_INSET.1, |ui| {
        rows(
            ui,
            "library",
            theme::QUEUE_ROW_H,
            playlists.len() + 1,
            |ui, i| {
                if i == 0 {
                    let resp = list_row(
                        ui,
                        ListRow {
                            key: "liked",
                            art: Art::Liked,
                            title: "Liked songs",
                            sub: &liked,
                            chip: None,
                            reserve: 0.0,
                        },
                    );
                    if resp.clicked() {
                        out.push(Action::Go(Route::Liked));
                    }
                    return;
                }
                let pl = &playlists[i - 1];
                let is_main = main_id == Some(pl.id.as_str());
                let sub = playlist_sub(pl.owner.as_deref(), pl.track_count);
                let resp = list_row(
                    ui,
                    ListRow {
                        key: &pl.id,
                        art: Art::Square(&pl.thumbnails),
                        title: &pl.name,
                        sub: &sub,
                        chip: is_main.then_some("Main"),
                        reserve: if is_main { 0.0 } else { 120.0 },
                    },
                );
                let mut make_main = false;
                if !is_main && resp.contains_pointer() {
                    make_main = row_action(ui, resp.rect, &pl.id);
                }
                if !is_main {
                    resp.context_menu(|ui| {
                        if ui.button(MAKE_MAIN).clicked() {
                            make_main = true;
                            ui.close();
                        }
                    });
                }
                if make_main {
                    cmd(out, Command::SetMainPlaylist(pl.id.clone()));
                    out.push(Action::Go(Route::Collection));
                } else if resp.clicked() {
                    if is_main {
                        out.push(Action::Go(Route::Collection));
                    } else {
                        open_playlist(out, &pl.id);
                    }
                }
            },
        )
    });
}

/// "Playlist · You · 120 songs".
pub(crate) fn playlist_sub(owner: Option<&str>, count: Option<u64>) -> String {
    let mut sub = format!("Playlist · {}", owner.unwrap_or("You"));
    if let Some(n) = count {
        sub.push_str(&format!(" · {}", songs(n as usize)));
    }
    sub
}

/// The hover action at the row's right edge: a small outlined "Make main"
/// button in the style of the table's Add button. Returns true on click.
fn row_action(ui: &mut Ui, row: Rect, id: &str) -> bool {
    let p = theme::pal();
    let label = "Make main";
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::secondary(), p.text_2);
    let icon = 12.0;
    let w = 6.0 + icon + theme::SPACE_4 + galley.size().x + theme::SPACE_8;
    let rect = Rect::from_min_size(
        Pos2::new(row.right() - theme::SPACE_8 - w, row.center().y - 11.0),
        vec2(w, 22.0),
    );
    let resp = ui
        .interact(rect, ui.scope_id().with(("make-main", id)), Sense::click())
        .on_hover_text(MAKE_MAIN)
        .on_hover_cursor(CursorIcon::PointingHand);
    let (fill, fg) = if resp.hovered() {
        (p.surface_3, p.text_1)
    } else {
        (p.surface_2, p.text_2)
    };
    let radius = CornerRadius::same(theme::RADIUS_CONTROL);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(theme::STROKE_W, p.stroke_2),
        StrokeKind::Inside,
    );
    painter.text(
        Pos2::new(rect.left() + 6.0 + icon / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        icons::PUSH_PIN,
        theme::icon(icon),
        fg,
    );
    painter.text(
        Pos2::new(rect.left() + 6.0 + icon + theme::SPACE_4, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::secondary(),
        fg,
    );
    resp.clicked()
}

// The old Library and Search pages were the last callers of these shared
// widgets. Keep them alive until the integrator removes them from
// `widgets/mod.rs` (this file may not edit it).
const _: fn(&mut Ui, &str) = crate::widgets::title;
const _: for<'a> fn(
    &mut Ui,
    f32,
    &str,
    &str,
    &str,
    crate::widgets::ArtKind<'a>,
    bool,
) -> egui::Response = crate::widgets::entity_row;
const _: crate::widgets::ArtKind<'static> = crate::widgets::ArtKind::Liked;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_subtitles() {
        assert_eq!(playlist_sub(None, Some(1)), "Playlist · You · 1 song");
        assert_eq!(
            playlist_sub(Some("Rustle"), Some(1535)),
            "Playlist · Rustle · 1535 songs"
        );
        assert_eq!(playlist_sub(Some("Rustle"), None), "Playlist · Rustle");
    }
}
