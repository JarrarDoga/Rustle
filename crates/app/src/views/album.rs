//! The album page (D-Album): header with "Add N missing to Everything",
//! tracks on the shared table with the "In Everything" column.
//!
//! Also holds the page pieces the other pages (artist, playlist, library,
//! search) share: the D page header, header buttons, cover art, a list row
//! and a memo keyed on `Arc` identity.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use egui::epaint::RectShape;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Layout, Painter, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, TextFormat, Ui, UiBuilder, vec2,
};
use model::{Album, Thumbnail, Track};

use crate::state::{Command, Loadable, State};
use crate::views::{Out, View, cmd, loading_or_error};
use crate::widgets::{ArtKind, art, seed, track_table};
use crate::{icons, theme, thumbs};

pub fn show(ui: &mut Ui, state: &State, view: &mut View, id: &str, out: &mut Out) {
    let Some((_, page)) = state.album.as_ref().filter(|(open, _)| open == id) else {
        return loading_or_error::<()>(ui, &Loadable::Loading, "this album");
    };
    let album = match page {
        Loadable::Ready(a) => Arc::clone(a),
        other => return loading_or_error(ui, other, "this album"),
    };
    let main = main_tracks(state);
    let d = memo(
        ui,
        egui::Id::unique("album-page"),
        (Arc::clone(&album), main.clone()),
        |a, b| Arc::ptr_eq(&a.0, &b.0) && same(&a.1, &b.1),
        || derive(&album, main.as_deref().map(|m| &m[..])),
    );
    let s = &album.summary;
    let caption = s
        .year
        .map_or_else(|| "Album".to_owned(), |y| format!("Album · {y}"));
    let main_name = main_name(state);
    let status = main
        .as_ref()
        .map(|_| in_main_status(d.tracks.len() - d.missing.len(), d.tracks.len(), main_name));
    header(
        ui,
        Header {
            key: &s.id,
            art: Art::Square(&s.thumbnails),
            art_size: theme::ALBUM_CARD_W,
            caption: &caption,
            title: &s.name,
            detail: &d.detail,
            status: status.as_deref().filter(|t| !t.is_empty()),
        },
        |ui| {
            play_buttons(ui, &d.tracks, "Shuffle", out);
            if main.is_some() && !d.missing.is_empty() {
                let label = add_missing_label(d.missing.len(), main_name);
                if button(ui, icons::PLUS, 12.0, &label, false).clicked() {
                    cmd(out, Command::AddToMain(d.missing.to_vec()));
                }
            }
        },
    );
    track_table::show(
        ui,
        state,
        view,
        track_table::Table {
            salt: "album",
            tracks: &d.tracks,
            rows: None,
            columns: track_table::ALBUM,
            scroll_to: None,
        },
        out,
    );
}

/// What the album page shows, recomputed only when the album or the main
/// playlist changes.
#[derive(Clone)]
struct Derived {
    tracks: Arc<Vec<Arc<Track>>>,
    missing: Arc<Vec<Arc<Track>>>,
    detail: Arc<str>,
}

fn derive(album: &Album, main: Option<&[Arc<Track>]>) -> Derived {
    let tracks: Vec<Arc<Track>> = album.tracks.iter().cloned().map(Arc::new).collect();
    let missing = main.map_or_else(Vec::new, |m| missing(&tracks, m));
    Derived {
        tracks: Arc::new(tracks),
        missing: Arc::new(missing),
        detail: album_detail(album).into(),
    }
}

// ------------------------------------------------------------ pure helpers

/// `tracks` that are not in `main`, once each, in album order.
pub(crate) fn missing(tracks: &[Arc<Track>], main: &[Arc<Track>]) -> Vec<Arc<Track>> {
    let mut seen: HashSet<&str> = main.iter().map(|t| t.id.as_str()).collect();
    tracks
        .iter()
        .filter(|t| seen.insert(t.id.as_str()))
        .cloned()
        .collect()
}

/// "1 song", "12 songs".
pub(crate) fn songs(n: usize) -> String {
    if n == 1 {
        "1 song".to_owned()
    } else {
        format!("{n} songs")
    }
}

/// A list's total length: "44 min", "1 hr 5 min".
pub(crate) fn fmt_length(total: Duration) -> String {
    let min = (total.as_secs() + 30) / 60;
    if min >= 60 {
        format!("{} hr {} min", min / 60, min % 60)
    } else {
        format!("{min} min")
    }
}

/// "12 songs · 44 min", with the length left out when no track has one.
pub(crate) fn count_and_length<'a>(tracks: impl IntoIterator<Item = &'a Track>) -> String {
    let (n, total) = tracks.into_iter().fold((0, Duration::ZERO), |(n, d), t| {
        (n + 1, d + t.duration.unwrap_or_default())
    });
    if total.is_zero() {
        songs(n)
    } else {
        format!("{} · {}", songs(n), fmt_length(total))
    }
}

/// "Saint Juniper · 12 songs · 44 min".
pub(crate) fn album_detail(album: &Album) -> String {
    let by: Vec<&str> = album
        .summary
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    let counts = count_and_length(&album.tracks);
    if by.is_empty() {
        counts
    } else {
        format!("{} · {counts}", by.join(", "))
    }
}

/// "8 of 12 already in Everything", "All 12 in Everything".
pub(crate) fn in_main_status(in_main: usize, total: usize, main: &str) -> String {
    match total {
        0 => String::new(),
        _ if in_main >= total => format!("All {total} in {main}"),
        _ => format!("{in_main} of {total} already in {main}"),
    }
}

/// "Add 4 missing to Everything".
pub(crate) fn add_missing_label(n: usize, main: &str) -> String {
    format!("Add {n} missing to {main}")
}

// ---------------------------------------------------- shared page pieces

/// The main playlist's tracks, if it is loaded.
pub(crate) fn main_tracks(state: &State) -> Option<Arc<Vec<Arc<Track>>>> {
    state.main_view().map(|v| Arc::clone(&v.tracks))
}

/// The main playlist's name ("Everything" until it loads).
pub(crate) fn main_name(state: &State) -> &str {
    state
        .main_view()
        .map_or("Everything", |v| v.summary.name.as_str())
}

/// Both `None`, or the same allocation.
pub(crate) fn same<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The value cached under `id` while `same(cached key, key)` holds;
/// otherwise `compute`s and caches it. Keys hold `Arc`s, so a freed
/// allocation cannot come back under the same pointer while cached.
pub(crate) fn memo<K, V>(
    ui: &Ui,
    id: egui::Id,
    key: K,
    same: impl Fn(&K, &K) -> bool,
    compute: impl FnOnce() -> V,
) -> V
where
    K: Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    if let Some((k, v)) = ui.data(|d| d.get_temp::<(K, V)>(id))
        && same(&k, &key)
    {
        return v;
    }
    let v = compute();
    ui.data_mut(|d| d.insert_temp(id, (key, v.clone())));
    v
}

/// Shuffle (primary) and Play in order for a list of tracks.
pub(crate) fn play_buttons(
    ui: &mut Ui,
    tracks: &Arc<Vec<Arc<Track>>>,
    shuffle_label: &str,
    out: &mut Out,
) {
    if tracks.is_empty() {
        return;
    }
    if button(ui, icons::SHUFFLE, 14.0, shuffle_label, true).clicked() {
        play(out, tracks, true);
    }
    if button(ui, icons::PLAY, 12.0, "Play in order", false).clicked() {
        play(out, tracks, false);
    }
}

/// Replaces the queue with `tracks`, shuffled or in order.
pub(crate) fn play(out: &mut Out, tracks: &Arc<Vec<Arc<Track>>>, shuffle: bool) {
    if tracks.is_empty() {
        return;
    }
    let s = seed();
    let start = if shuffle {
        (s % tracks.len() as u64) as usize
    } else {
        0
    };
    cmd(
        out,
        Command::SetShuffle {
            on: shuffle,
            seed: s,
        },
    );
    cmd(
        out,
        Command::PlayTracks {
            tracks: Arc::clone(tracks),
            start,
            seed: s,
        },
    );
}

/// A D header button (D-Tokens buttons): primary is the accent fill,
/// secondary a `stroke_2` outline.
pub(crate) fn button(
    ui: &mut Ui,
    icon: &str,
    icon_size: f32,
    label: &str,
    primary: bool,
) -> Response {
    button_with(ui, icon, icon_size, label, primary, None)
}

/// [`button`] with a trailing icon (the artist page's "→").
pub(crate) fn button_with(
    ui: &mut Ui,
    icon: &str,
    icon_size: f32,
    label: &str,
    primary: bool,
    trail: Option<&str>,
) -> Response {
    const PAD: f32 = 14.0;
    let p = theme::pal();
    let font = if primary {
        theme::row_strong()
    } else {
        theme::row()
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let trail_w = trail.map_or(0.0, |_| theme::SPACE_8 + icon_size);
    let w = PAD + icon_size + theme::SPACE_8 + galley.size().x + trail_w + PAD;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, theme::BUTTON_H), Sense::click());
    let down = resp.is_pointer_button_down_on();
    let hover = resp.hovered();
    let (fill, fg) = if primary {
        let fill = if down {
            theme::accent_pressed()
        } else if hover {
            theme::accent_hover()
        } else {
            theme::accent()
        };
        (fill, p.on_accent)
    } else {
        let fill = if down {
            p.surface_3
        } else if hover {
            p.surface_2
        } else {
            Color32::TRANSPARENT
        };
        (fill, p.text_1)
    };
    let radius = CornerRadius::same(theme::RADIUS_CONTROL);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    if !primary {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(theme::STROKE_W, p.stroke_2),
            StrokeKind::Inside,
        );
    }
    let cy = rect.center().y;
    let mut x = rect.left() + PAD;
    painter.text(
        Pos2::new(x + icon_size / 2.0, cy),
        Align2::CENTER_CENTER,
        icon,
        theme::icon(icon_size),
        fg,
    );
    x += icon_size + theme::SPACE_8;
    let text_w = galley.size().x;
    painter.galley(Pos2::new(x, cy - galley.size().y / 2.0), galley, fg);
    if let Some(trail) = trail {
        painter.text(
            Pos2::new(x + text_w + theme::SPACE_8 + icon_size / 2.0, cy),
            Align2::CENTER_CENTER,
            trail,
            theme::icon(icon_size),
            fg,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Which cover to draw.
#[derive(Clone, Copy)]
pub(crate) enum Art<'a> {
    Square(&'a [Thumbnail]),
    Round(&'a [Thumbnail]),
    Liked,
}

/// Cover art with D's 2 px radius (round for artists): the thumbnail once
/// loaded, a letter tile until then.
pub(crate) fn cover(painter: &Painter, rect: Rect, key: &str, title: &str, kind: Art<'_>) {
    let p = theme::pal();
    let radius = CornerRadius::same(theme::RADIUS_ROW);
    let thumbnails = match kind {
        Art::Round(t) => return art(painter, rect, key, title, ArtKind::Round(t)),
        Art::Liked => {
            painter.rect_filled(rect, radius, p.surface_3);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                icons::HEART,
                theme::icon_fill(rect.height() * 0.4),
                p.text_2,
            );
            return;
        }
        Art::Square(t) => t,
    };
    if let Some(tex) = thumbs::get(painter.ctx(), thumbnails, rect.height()) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, uv));
        return;
    }
    painter.rect_filled(rect, radius, theme::art_color(key));
    let letter = title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "♪".to_owned(), |c| c.to_uppercase().collect());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId {
            size: rect.height() * 0.42,
            ..theme::section()
        },
        p.text_1.gamma_multiply(0.6),
    );
}

/// Paints one line left-aligned at `pos` (its vertical center), cut with an
/// ellipsis at `max_width`. Returns the painted width.
pub(crate) fn line(
    painter: &Painter,
    pos: Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> f32 {
    if max_width <= 4.0 {
        return 0.0;
    }
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(max_width);
    let galley = painter.layout_job(job);
    let w = galley.size().x;
    painter.galley(
        Pos2::new(pos.x, pos.y - galley.size().y / 2.0),
        galley,
        color,
    );
    w
}

/// A full-bleed 1 px rule across the page, through the page margin.
pub(crate) fn rule(ui: &mut Ui) {
    let r = ui.max_rect();
    let y = ui.cursor().top() + 0.5;
    ui.painter().hline(
        (r.left() - theme::SPACE_24)..=(r.right() + theme::SPACE_24),
        y,
        theme::hairline(),
    );
    ui.add_space(theme::STROKE_W);
}

/// A D page header (D-Album, D-Artist).
pub(crate) struct Header<'a> {
    pub key: &'a str,
    pub art: Art<'a>,
    pub art_size: f32,
    /// "Album · 2025", "Artist", "Playlist".
    pub caption: &'a str,
    pub title: &'a str,
    /// "Saint Juniper · 12 songs · 44 min".
    pub detail: &'a str,
    /// A line after a check mark: "8 of 12 already in Everything".
    pub status: Option<&'a str>,
}

/// Draws the header, its `buttons` row, and the rule under it. Text and
/// buttons sit on the art's bottom edge.
pub(crate) fn header(ui: &mut Ui, h: Header<'_>, buttons: impl FnOnce(&mut Ui)) {
    const CAPTION_H: f32 = 14.0;
    const TITLE_H: f32 = 26.0;
    const LINE_H: f32 = 16.0;
    let p = theme::pal();
    let gap = 6.0;
    let status_h = h.status.map_or(0.0, |_| gap + 2.0 + LINE_H);
    let block = CAPTION_H + gap + TITLE_H + gap + LINE_H + status_h + 10.0 + theme::BUTTON_H;
    let height = block.max(h.art_size);

    ui.add_space(theme::SPACE_8);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(rect);
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left(), rect.bottom() - h.art_size),
        vec2(h.art_size, h.art_size),
    );
    cover(&painter, art_rect, h.key, h.title, h.art);
    let x = art_rect.right() + theme::SPACE_20;
    let w = rect.right() - x;
    let mut y = rect.bottom() - block;
    line(
        &painter,
        Pos2::new(x, y + CAPTION_H / 2.0),
        h.caption,
        theme::caption(),
        p.text_3,
        w,
    );
    y += CAPTION_H + gap;
    line(
        &painter,
        Pos2::new(x, y + TITLE_H / 2.0),
        h.title,
        theme::page_title(),
        p.text_1,
        w,
    );
    y += TITLE_H + gap;
    line(
        &painter,
        Pos2::new(x, y + LINE_H / 2.0),
        h.detail,
        theme::secondary(),
        p.text_2,
        w,
    );
    y += LINE_H;
    if let Some(status) = h.status {
        y += gap + 2.0;
        let cy = y + LINE_H / 2.0;
        painter.text(
            Pos2::new(x + 7.0, cy),
            Align2::CENTER_CENTER,
            icons::CHECK,
            theme::icon(14.0),
            theme::accent(),
        );
        line(
            &painter,
            Pos2::new(x + 14.0 + gap, cy),
            status,
            theme::secondary(),
            p.text_2,
            w - 20.0,
        );
    }
    let row = Rect::from_min_max(Pos2::new(x, rect.bottom() - theme::BUTTON_H), rect.max);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = theme::SPACE_8;
            buttons(ui);
        },
    );
    ui.add_space(theme::SPACE_24);
    rule(ui);
}

/// A list row (D style): 32 px cover, title with an optional chip, and a
/// secondary line. `reserve` keeps that much of the right edge free for a
/// row action. Search results and the library use it.
pub(crate) struct ListRow<'a> {
    pub key: &'a str,
    pub art: Art<'a>,
    pub title: &'a str,
    pub sub: &'a str,
    pub chip: Option<&'a str>,
    pub reserve: f32,
}

pub(crate) fn list_row(ui: &mut Ui, r: ListRow<'_>) -> Response {
    let p = theme::pal();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::QUEUE_ROW_H),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    if resp.hovered() || resp.contains_pointer() {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_ROW), p.surface_2);
    }
    let art_rect = Rect::from_min_size(
        Pos2::new(
            rect.left() + theme::SPACE_8,
            rect.center().y - theme::QUEUE_ART / 2.0,
        ),
        vec2(theme::QUEUE_ART, theme::QUEUE_ART),
    );
    cover(&painter, art_rect, r.key, r.title, r.art);
    let x = art_rect.right() + theme::SPACE_12;
    let w = rect.right() - x - theme::SPACE_8 - r.reserve;
    let chip_w = r.chip.map_or(0.0, |c| {
        painter
            .layout_no_wrap(c.to_owned(), theme::caption(), p.text_1)
            .size()
            .x
            + 2.0 * theme::SPACE_8
    });
    let cy = rect.center().y;
    let title_w = line(
        &painter,
        Pos2::new(x, cy - 8.0),
        r.title,
        theme::row(),
        p.text_1,
        w - chip_w,
    );
    if let Some(chip) = r.chip {
        let chip_rect = Rect::from_min_size(
            Pos2::new(x + title_w + theme::SPACE_8, cy - 8.0 - 9.0),
            vec2(chip_w - theme::SPACE_8, 18.0),
        );
        painter.rect_filled(
            chip_rect,
            CornerRadius::same(theme::RADIUS_CONTROL),
            p.surface_3,
        );
        painter.text(
            chip_rect.center(),
            Align2::CENTER_CENTER,
            chip,
            theme::caption(),
            p.text_1,
        );
    }
    line(
        &painter,
        Pos2::new(x, cy + 9.0),
        r.sub,
        theme::secondary(),
        p.text_2,
        w,
    );
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// Shown instead of content: idle, loading, empty and error states.
pub(crate) fn note(ui: &mut Ui, text: &str) {
    ui.add_space(theme::SPACE_16);
    ui.label(
        egui::RichText::new(text)
            .font(theme::secondary())
            .color(theme::pal().text_3),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{AlbumSummary, ArtistRef};

    fn track(id: &str, secs: u64) -> Track {
        Track {
            id: id.into(),
            title: id.into(),
            artists: vec![ArtistRef {
                id: Some("UCa".into()),
                name: "Saint Juniper".into(),
            }],
            album: None,
            duration: Some(Duration::from_secs(secs)),
            thumbnails: Vec::new(),
        }
    }

    fn arcs(ids: &[&str]) -> Vec<Arc<Track>> {
        ids.iter().map(|id| Arc::new(track(id, 60))).collect()
    }

    #[test]
    fn missing_skips_main_and_duplicates() {
        let album = arcs(&["a", "b", "c", "c", "d"]);
        let main = arcs(&["x", "b", "d"]);
        let ids: Vec<_> = missing(&album, &main)
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(ids, ["a", "c"]);
        assert!(missing(&arcs(&["b"]), &main).is_empty());
    }

    #[test]
    fn lengths() {
        assert_eq!(fmt_length(Duration::from_secs(44 * 60 + 10)), "44 min");
        assert_eq!(fmt_length(Duration::from_secs(29)), "0 min");
        assert_eq!(fmt_length(Duration::from_secs(65 * 60)), "1 hr 5 min");
    }

    #[test]
    fn album_detail_line() {
        let album = Album {
            summary: AlbumSummary {
                id: "MPREb".into(),
                name: "Small Hours, Vol. II".into(),
                artists: vec![ArtistRef {
                    id: None,
                    name: "Saint Juniper".into(),
                }],
                year: Some(2025),
                thumbnails: Vec::new(),
            },
            tracks: vec![track("a", 1300), track("b", 1340)],
        };
        assert_eq!(album_detail(&album), "Saint Juniper · 2 songs · 44 min");
        let mut one = album.clone();
        one.summary.artists.clear();
        one.tracks.truncate(1);
        one.tracks[0].duration = None;
        assert_eq!(album_detail(&one), "1 song");
    }

    #[test]
    fn status_and_add_labels() {
        assert_eq!(
            in_main_status(8, 12, "Everything"),
            "8 of 12 already in Everything"
        );
        assert_eq!(in_main_status(12, 12, "Mix"), "All 12 in Mix");
        assert_eq!(in_main_status(0, 0, "Mix"), "");
        assert_eq!(
            add_missing_label(4, "Everything"),
            "Add 4 missing to Everything"
        );
    }

    #[test]
    fn same_compares_allocations() {
        let a = Arc::new(1);
        assert!(same(&Some(Arc::clone(&a)), &Some(Arc::clone(&a))));
        assert!(!same(&Some(a), &Some(Arc::new(1))));
        assert!(same::<i32>(&None, &None));
    }
}
