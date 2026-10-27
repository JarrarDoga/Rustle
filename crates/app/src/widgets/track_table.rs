//! The one song table every list uses (D-Tokens track rows, D-Collection,
//! D-Search, D-Album). Rows are virtualized with `show_rows`.
//!
//! Integrator stub: a working table with the final API. The `track-table`
//! worker owns this file and builds the D row states (default, hover,
//! selected, playing), the column set per screen, like on hover, and the
//! loading skeleton.

use std::sync::Arc;

use egui::{Align2, CornerRadius, Pos2, Rect, RichText, ScrollArea, Sense, Ui, vec2};
use model::Track;

use crate::state::{Command, State};
use crate::theme;
use crate::views::{Out, View, artists, cmd};
use crate::widgets::{ArtKind, art, fmt_time, seed, text_line};

/// A table column. Screens pick a set; see the constants below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The row's number: playlist position or album track number. On hover
    /// it becomes a play button; on the playing row, a speaker.
    Index,
    /// Art (24 px) and title.
    Title,
    Artist,
    Album,
    /// From `State::last_played` ("3 mo", "Now" while playing).
    LastPlayed,
    /// Check plus `#412` when the song is in the main playlist, else an Add
    /// button that sends `Command::AddToMain`.
    InEverything,
    /// Like on hover, then the duration.
    Time,
}

use Column::*;

/// D-Collection: the main playlist.
pub const COLLECTION: &[Column] = &[Index, Title, Artist, Album, LastPlayed, Time];
/// D-Search, D-Artist top songs, other playlists.
pub const OUTSIDE: &[Column] = &[Index, Title, Artist, Album, InEverything, Time];
/// D-Album.
pub const ALBUM: &[Column] = &[Index, Title, Artist, InEverything, Time];

/// One table to draw.
pub struct Table<'a> {
    /// Unique per list; selection and scroll state key off it.
    pub salt: &'a str,
    pub tracks: &'a Arc<Vec<Arc<Track>>>,
    /// Rows to show, as indices into `tracks`, in display order. `None`
    /// shows every track in order. The Index column shows `index + 1`.
    pub rows: Option<&'a [usize]>,
    pub columns: &'a [Column],
    /// Scroll so the row showing `tracks[i]` is in view.
    pub scroll_to: Option<usize>,
}

/// What the table reports after drawing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Shown {
    /// `tracks` indices of the first and last rows on screen.
    pub on_screen: Option<(usize, usize)>,
}

/// Draws the header and the rows. Click selects; double-click plays the
/// shown rows from that row; right-click opens the song menu.
pub fn show(ui: &mut Ui, state: &State, view: &mut View, table: Table<'_>, out: &mut Out) -> Shown {
    let Table {
        salt,
        tracks,
        rows,
        columns,
        scroll_to,
    } = table;
    let list_id = ui.scope_id().with(salt);
    let all: Vec<usize>;
    let rows = match rows {
        Some(r) => r,
        None => {
            all = (0..tracks.len()).collect();
            &all
        }
    };
    let full = ui.available_rect_before_wrap();
    let (head, _) =
        ui.allocate_exact_size(vec2(full.width(), theme::TABLE_HEADER_H), Sense::hover());
    let p = theme::pal();
    let painter = ui.painter();
    for (x, text, align) in [
        (head.left() + 28.0, "#", Align2::RIGHT_CENTER),
        (head.left() + 44.0, "Title", Align2::LEFT_CENTER),
        (head.right() - 16.0, "Time", Align2::RIGHT_CENTER),
    ] {
        painter.text(
            Pos2::new(x, head.center().y),
            align,
            text,
            theme::caption(),
            p.text_3,
        );
    }
    painter.hline(full.x_range(), head.bottom(), theme::hairline());

    if rows.is_empty() {
        ui.label(RichText::new("No songs here.").color(p.text_3));
        return Shown::default();
    }
    let current = state.current_track().map(|t| t.id.clone());
    let mut shown = Shown::default();
    let mut area = ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false]);
    if let Some(i) = scroll_to.and_then(|i| rows.iter().position(|&r| r == i)) {
        area = area.vertical_scroll_offset(
            (i as f32 * theme::ROW_H - ui.available_height() / 3.0).max(0.0),
        );
    }
    area.show_rows(ui, theme::ROW_H, rows.len(), |ui, range| {
        if let (Some(first), Some(last)) = (range.clone().next(), range.clone().last()) {
            shown.on_screen = Some((rows[first], rows[last]));
        }
        for k in range {
            let i = rows[k];
            let track = &tracks[i];
            let selected = view.selected == Some((list_id, k));
            let is_current = current.as_deref() == Some(track.id.as_str());
            let resp = row(ui, i, track, selected, is_current, columns);
            if resp.clicked() {
                view.selected = Some((list_id, k));
            }
            if resp.double_clicked() {
                let list: Vec<Arc<Track>> = rows.iter().map(|&r| Arc::clone(&tracks[r])).collect();
                cmd(
                    out,
                    Command::PlayTracks {
                        tracks: Arc::new(list),
                        start: k,
                        seed: seed(),
                    },
                );
            }
            resp.context_menu(|ui| menu(ui, state, track, out));
        }
    });
    shown
}

fn row(
    ui: &mut Ui,
    index: usize,
    track: &Track,
    selected: bool,
    is_current: bool,
    columns: &[Column],
) -> egui::Response {
    let p = theme::pal();
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same(theme::RADIUS_ROW);
    if selected {
        painter.rect_filled(rect, radius, p.surface_3);
    } else if resp.hovered() {
        painter.rect_filled(rect, radius, p.surface_2);
    }
    let cy = rect.center().y;
    let (marker, font, color) = if is_current {
        (
            crate::icons::SPEAKER_HIGH.to_owned(),
            theme::icon(14.0),
            theme::accent(),
        )
    } else if resp.hovered() {
        (
            crate::icons::PLAY.to_owned(),
            theme::icon_fill(12.0),
            p.text_1,
        )
    } else {
        ((index + 1).to_string(), theme::secondary(), p.text_3)
    };
    painter.text(
        Pos2::new(rect.left() + 28.0, cy),
        Align2::RIGHT_CENTER,
        marker,
        font,
        color,
    );
    let art_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 56.0, cy),
        vec2(theme::ROW_ART, theme::ROW_ART),
    );
    art(
        &painter,
        art_rect,
        &track.id,
        &track.title,
        ArtKind::Square(&track.thumbnails),
    );
    let x = art_rect.right() + 10.0;
    let col = (rect.right() - x - 64.0) / 3.0;
    let (title_font, title_color) = if is_current {
        (theme::row_strong(), theme::accent())
    } else {
        (theme::row(), p.text_1)
    };
    text_line(
        &painter,
        Pos2::new(x, cy),
        &track.title,
        title_font,
        title_color,
        col - 12.0,
    );
    text_line(
        &painter,
        Pos2::new(x + col, cy),
        &artists(track),
        theme::secondary(),
        p.text_2,
        col - 12.0,
    );
    if let Some(album) = track.album.as_ref().filter(|_| columns.contains(&Album)) {
        text_line(
            &painter,
            Pos2::new(x + 2.0 * col, cy),
            &album.name,
            theme::secondary(),
            p.text_2,
            col - 12.0,
        );
    }
    if let Some(d) = track.duration {
        painter.text(
            Pos2::new(rect.right() - 16.0, cy),
            Align2::RIGHT_CENTER,
            fmt_time(d),
            theme::secondary(),
            p.text_2,
        );
    }
    resp
}

/// The right-click song menu.
pub fn menu(ui: &mut Ui, state: &State, track: &Arc<Track>, out: &mut Out) {
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
    let liked = state.is_liked(&track.id);
    if ui.button(if liked { "Unlike" } else { "Like" }).clicked() {
        let id = track.id.clone();
        cmd(
            out,
            if liked {
                Command::Unlike(id)
            } else {
                Command::Like(id)
            },
        );
        ui.close();
    }
}

/// Static skeleton rows for a list that is loading (D-States). No
/// animation.
#[allow(dead_code)] // ui-batch: the collection calls it while the main playlist loads
pub fn skeleton(ui: &mut Ui, rows: usize) {
    let p = theme::pal();
    for _ in 0..rows {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), theme::ROW_H), Sense::hover());
        let bar = Rect::from_min_size(
            Pos2::new(rect.left() + 44.0, rect.center().y - 5.0),
            vec2(rect.width() * 0.4, 10.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(theme::RADIUS_ROW), p.surface_2);
    }
}
