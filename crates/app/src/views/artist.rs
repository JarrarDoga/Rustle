//! The artist page (D-Artist): top songs on the shared table, then albums
//! and singles as cards.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    CornerRadius, CursorIcon, Pos2, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    vec2,
};
use model::{AlbumSummary, Artist, Track};

use crate::state::{Loadable, State};
use crate::views::album::{
    Art, Header, button, button_with, cover, header, line, main_name, main_tracks, memo, play, same,
};
use crate::views::{Action, Chip, Out, Route, View, loading_or_error, open_album};
use crate::widgets::track_table;
use crate::{icons, theme};

pub fn show(ui: &mut Ui, state: &State, view: &mut View, id: &str, out: &mut Out) {
    let Some((_, page)) = state.artist.as_ref().filter(|(open, _)| open == id) else {
        return loading_or_error::<()>(ui, &Loadable::Loading, "this artist");
    };
    let artist = match page {
        Loadable::Ready(a) => Arc::clone(a),
        other => return loading_or_error(ui, other, "this artist"),
    };
    let main = main_tracks(state);
    let d = memo(
        ui,
        egui::Id::unique("artist-page"),
        (Arc::clone(&artist), main.clone()),
        |a, b| Arc::ptr_eq(&a.0, &b.0) && same(&a.1, &b.1),
        || derive(&artist, main.as_deref().map(|m| &m[..])),
    );
    let s = &artist.summary;
    let main_name = main_name(state);
    let detail = artist_detail(&artist);
    let mut to_collection = false;
    header(
        ui,
        Header {
            key: &s.id,
            art: Art::Round(&s.thumbnails),
            art_size: theme::ARTIST_AVATAR,
            caption: "Artist",
            title: &s.name,
            detail: &detail,
            status: None,
        },
        |ui| {
            if !d.top.is_empty()
                && button(ui, icons::SHUFFLE, 14.0, "Shuffle artist", true).clicked()
            {
                play(out, &d.top, true);
            }
            if main.is_some() {
                let label = in_main_label(d.in_main, main_name);
                to_collection = button_with(
                    ui,
                    icons::CHECK,
                    14.0,
                    &label,
                    false,
                    Some(icons::ARROW_RIGHT),
                )
                .on_hover_text(format!("Show them in {main_name}"))
                .clicked();
            }
        },
    );
    if to_collection {
        // Filter the collection to this artist (D-Filter's chip).
        let chip = Chip::Artist(s.name.clone());
        let chips = &mut view.collection.chips;
        if !chips.contains(&chip) {
            chips.push(chip);
        }
        out.push(Action::Go(Route::Collection));
    }

    ScrollArea::vertical()
        .id_salt("artist-page")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !d.top.is_empty() {
                section(ui, "Top songs");
                let h = theme::TABLE_HEADER_H + d.top.len() as f32 * theme::ROW_H + theme::SPACE_8;
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    track_table::show(
                        ui,
                        state,
                        view,
                        track_table::Table {
                            salt: "artist-top",
                            tracks: &d.top,
                            rows: None,
                            columns: track_table::OUTSIDE,
                            scroll_to: None,
                        },
                        out,
                    );
                });
            }
            for (title, albums, counts) in [
                ("Albums", &artist.albums, &d.album_counts),
                ("Singles and EPs", &artist.singles, &d.single_counts),
            ] {
                if albums.is_empty() {
                    continue;
                }
                section(ui, title);
                cards(ui, albums, counts, main.is_some().then_some(main_name), out);
            }
            ui.add_space(theme::SPACE_16);
        });
}

/// What the artist page shows, recomputed only when the artist or the main
/// playlist changes.
#[derive(Clone)]
struct Derived {
    top: Arc<Vec<Arc<Track>>>,
    in_main: usize,
    album_counts: Arc<Vec<usize>>,
    single_counts: Arc<Vec<usize>>,
}

fn derive(artist: &Artist, main: Option<&[Arc<Track>]>) -> Derived {
    let main = main.unwrap_or_default();
    let per_album = album_counts(main);
    let counts = |albums: &[AlbumSummary]| {
        Arc::new(
            albums
                .iter()
                .map(|a| per_album.get(a.id.as_str()).copied().unwrap_or(0))
                .collect(),
        )
    };
    Derived {
        top: Arc::new(artist.top_songs.iter().cloned().map(Arc::new).collect()),
        in_main: songs_in_main(main, &artist.summary.id, &artist.summary.name),
        album_counts: counts(&artist.albums),
        single_counts: counts(&artist.singles),
    }
}

/// A 16/600 section heading with D-Artist's spacing.
fn section(ui: &mut Ui, text: &str) {
    ui.add_space(theme::SPACE_16);
    ui.label(
        egui::RichText::new(text)
            .font(theme::section())
            .color(theme::pal().text_1),
    );
    ui.add_space(theme::SPACE_4);
}

/// Album cards in rows: art, title, "2025 · 8 in Everything". Click opens
/// the album.
fn cards(
    ui: &mut Ui,
    albums: &[AlbumSummary],
    counts: &[usize],
    main: Option<&str>,
    out: &mut Out,
) {
    let p = theme::pal();
    let gap = theme::SPACE_12;
    let w = theme::ALBUM_CARD_W;
    let cols = (((ui.available_width() + gap) / (w + gap)).floor() as usize).max(1);
    let text_h = 6.0 + 16.0 + 1.0 + 16.0;
    for (r, chunk) in albums.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (c, a) in chunk.iter().enumerate() {
                let i = r * cols + c;
                let (rect, resp) = ui.allocate_exact_size(vec2(w, w + text_h), Sense::click());
                let painter = ui.painter_at(rect.expand(1.0));
                let art_rect = Rect::from_min_size(rect.min, vec2(w, w));
                cover(
                    &painter,
                    art_rect,
                    &a.id,
                    &a.name,
                    Art::Square(&a.thumbnails),
                );
                if resp.hovered() {
                    painter.rect_stroke(
                        art_rect,
                        CornerRadius::same(theme::RADIUS_ROW),
                        Stroke::new(theme::STROKE_W, p.stroke_2),
                        StrokeKind::Outside,
                    );
                }
                let y = art_rect.bottom() + 6.0;
                line(
                    &painter,
                    Pos2::new(rect.left(), y + 8.0),
                    &a.name,
                    theme::row(),
                    p.text_1,
                    w,
                );
                let sub = card_subtitle(a.year, counts.get(i).copied().unwrap_or(0), main);
                line(
                    &painter,
                    Pos2::new(rect.left(), y + 25.0),
                    &sub,
                    theme::secondary(),
                    p.text_2,
                    w,
                );
                if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    open_album(out, &a.id);
                }
            }
        });
        ui.add_space(gap);
    }
}

// ------------------------------------------------------------ pure helpers

/// Main-playlist songs credited to this artist: by channel id, or by name
/// for unlinked credits.
pub(crate) fn songs_in_main(main: &[Arc<Track>], id: &str, name: &str) -> usize {
    main.iter()
        .filter(|t| {
            t.artists.iter().any(|a| match &a.id {
                Some(a_id) => a_id == id,
                None => a.name == name,
            })
        })
        .count()
}

/// Main-playlist song counts per album id.
fn album_counts(main: &[Arc<Track>]) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for t in main {
        if let Some(a) = &t.album {
            *counts.entry(a.id.as_str()).or_insert(0) += 1;
        }
    }
    counts
}

/// "41 songs in Everything".
pub(crate) fn in_main_label(n: usize, main: &str) -> String {
    format!("{} in {main}", crate::views::album::songs(n))
}

/// "4 albums · 6 singles and EPs".
fn artist_detail(artist: &Artist) -> String {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if !artist.albums.is_empty() {
        parts.push(plural(artist.albums.len(), "album", "albums"));
    }
    if !artist.singles.is_empty() {
        parts.push(plural(
            artist.singles.len(),
            "single or EP",
            "singles and EPs",
        ));
    }
    parts.join(" · ")
}

/// "2025 · 8 in Everything", "2025", "8 in Everything".
pub(crate) fn card_subtitle(year: Option<u16>, in_main: usize, main: Option<&str>) -> String {
    let count = main
        .filter(|_| in_main > 0)
        .map(|m| format!("{in_main} in {m}"));
    match (year, count) {
        (Some(y), Some(c)) => format!("{y} · {c}"),
        (Some(y), None) => y.to_string(),
        (None, Some(c)) => c,
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{AlbumRef, ArtistRef};

    fn track(id: &str, artist: (Option<&str>, &str), album: Option<&str>) -> Arc<Track> {
        Arc::new(Track {
            id: id.into(),
            title: id.into(),
            artists: vec![ArtistRef {
                id: artist.0.map(Into::into),
                name: artist.1.into(),
            }],
            album: album.map(|a| AlbumRef {
                id: a.into(),
                name: a.into(),
            }),
            duration: None,
            thumbnails: Vec::new(),
        })
    }

    #[test]
    fn counts_songs_in_main_by_id_or_unlinked_name() {
        let main = [
            track("a", (Some("UCsj"), "Saint Juniper"), Some("A1")),
            track("b", (Some("UCother"), "Saint Juniper"), Some("A1")),
            track("c", (None, "Saint Juniper"), None),
            track("d", (Some("UCsj"), "Renamed"), Some("A2")),
        ];
        assert_eq!(songs_in_main(&main, "UCsj", "Saint Juniper"), 3);
        assert_eq!(in_main_label(41, "Everything"), "41 songs in Everything");
        assert_eq!(in_main_label(1, "Mix"), "1 song in Mix");
        let per = album_counts(&main);
        assert_eq!(per.get("A1"), Some(&2));
        assert_eq!(per.get("A2"), Some(&1));
    }

    #[test]
    fn card_subtitles() {
        assert_eq!(
            card_subtitle(Some(2025), 8, Some("Everything")),
            "2025 · 8 in Everything"
        );
        assert_eq!(card_subtitle(Some(2025), 0, Some("Everything")), "2025");
        assert_eq!(card_subtitle(None, 3, Some("Mix")), "3 in Mix");
        assert_eq!(card_subtitle(None, 3, None), "");
    }
}
