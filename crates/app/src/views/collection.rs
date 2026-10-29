//! The Collection screen (D-Collection, D-Filter, D-Range): the main
//! playlist with its page header, filter row and track table.
//!
//! [`update`] turns the filter text, chips, "Not heard in 3 months", the
//! spine range and the sort into `visible` and the scope once per frame,
//! memoized so an unchanged frame costs O(1). [`show`] draws the header
//! (scope-aware Shuffle / Play in order), the filter row and the table.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::text::Galley;
use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Key, Margin, Popup, Pos2, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, vec2,
};
use model::{Track, TrackId};

use crate::icons;
use crate::state::{Command, Loadable, NEEDS_LOGIN, PlaylistView, Scope, State};
use crate::theme;
use crate::views::{Chip, CollectionView, Out, Sort, View, artists, cmd};
use crate::widgets::{ArtKind, art, seed, text_line, track_table};

/// The "Not heard in" window in months (the setting), set by `app.rs`
/// each frame. A month is 30 days, so 3 months is 90.
static NOT_HEARD_MONTHS: AtomicU8 = AtomicU8::new(3);

/// Sets the "Not heard in" window from the settings.
pub fn set_not_heard_months(months: u8) {
    NOT_HEARD_MONTHS.store(months.max(1), Ordering::Relaxed);
}

fn not_heard_months() -> u8 {
    NOT_HEARD_MONTHS.load(Ordering::Relaxed)
}

/// "Not heard in 3 months": never played here, or last played longer
/// ago than the window.
fn not_heard_secs(months: u8) -> i64 {
    i64::from(months) * 30 * 86_400
}

fn not_heard_label() -> String {
    match not_heard_months() {
        1 => "Not heard in a month".to_owned(),
        12 => "Not heard in a year".to_owned(),
        n => format!("Not heard in {n} months"),
    }
}

/// Narrowest the filter field gets before the quick filter and the sort
/// menu give up text.
const FIELD_MIN_W: f32 = 200.0;

const SORTS: [(Sort, &str); 6] = [
    (Sort::PlaylistOrder, "Playlist order"),
    (Sort::Title, "Title"),
    (Sort::Artist, "Artist"),
    (Sort::Album, "Album"),
    (Sort::LastPlayed, "Last played"),
    (Sort::Time, "Time"),
];

// ------------------------------------------------------------------ update

/// Recomputes `view.collection.visible` and `.scope` from the filter,
/// chips, quick filter, range and sort. `app.rs` calls it every frame
/// before drawing, so the spine and player see this frame's values.
///
/// A pending `scroll_to` (Locate, spine click) whose row the filters hide
/// moves to the nearest shown row: the first at or after that position,
/// else the last before it. Locate clears the filters itself first.
pub fn update(state: &State, view: &mut View) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    update_at(state, view, now);
}

/// [`update`] with "now" (unix seconds) passed in, for tests.
fn update_at(state: &State, view: &mut View, now: i64) {
    let c = &mut view.collection;
    let Some(main) = state.main_view() else {
        if !c.visible.is_empty() {
            c.visible = Arc::default();
        }
        c.scope = None;
        return;
    };
    // The 90-day cutoff moves once an hour, so idle frames stay memo hits.
    let hour = now.div_euclid(3600);
    let (visible, scope) = lookup(&main.tracks, &state.last_played, c, hour);
    if let Some(p) = c.scroll_to
        && !visible.contains(&p)
    {
        c.scroll_to = nearest(&visible, p);
    }
    if !Arc::ptr_eq(&c.visible, &visible) {
        c.visible = visible;
    }
    // Not compared first: `Scope::Filter` compares its ids by value.
    c.scope = Some(scope);
}

/// The shown position closest to `p`: the first at or after it, else the
/// last before it. O(n); runs only while a scroll is pending.
fn nearest(visible: &[usize], p: usize) -> Option<usize> {
    let after = visible.iter().copied().filter(|&v| v >= p).min();
    after.or_else(|| visible.iter().copied().filter(|&v| v < p).max())
}

type Tracks = Vec<Arc<Track>>;
type Played = HashMap<TrackId, i64>;

/// The last frame's inputs and results. `Weak` keys: they keep the old
/// allocations' addresses from being reused, and an in-place edit through
/// `Arc::make_mut` moves the data to a new address, so a pointer match
/// means the same data.
struct Memo {
    tracks: Weak<Tracks>,
    played: Weak<Played>,
    hour: i64,
    months: u8,
    stats: Stats,
    query: Option<Query>,
    visible: Arc<Vec<usize>>,
    scope: Scope,
}

/// Header numbers that depend on the data only, not the filter.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Stats {
    /// Tracks that pass "Not heard in 3 months" on its own.
    not_heard: usize,
    total: Duration,
}

/// The filter inputs the memo was computed for.
#[derive(Debug, Clone, PartialEq)]
struct Query {
    text: String,
    chips: Vec<Chip>,
    not_heard: bool,
    range: Option<(usize, usize)>,
    sort: Sort,
}

impl Query {
    fn of(c: &CollectionView) -> Self {
        Self {
            text: c.filter_text.clone(),
            chips: c.chips.clone(),
            not_heard: c.not_heard,
            range: c.range,
            sort: c.sort,
        }
    }

    /// Same inputs as `c`, without allocating.
    fn is(&self, c: &CollectionView) -> bool {
        self.text == c.filter_text
            && self.chips == c.chips
            && self.not_heard == c.not_heard
            && self.range == c.range
            && self.sort == c.sort
    }
}

thread_local! {
    static MEMO: RefCell<Option<Memo>> = const { RefCell::new(None) };
    #[cfg(test)]
    static COMPUTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn same<T>(weak: &Weak<T>, live: &Arc<T>) -> bool {
    std::ptr::eq(weak.as_ptr(), Arc::as_ptr(live))
}

fn lookup(
    tracks: &Arc<Tracks>,
    played: &Arc<Played>,
    c: &CollectionView,
    hour: i64,
) -> (Arc<Vec<usize>>, Scope) {
    MEMO.with_borrow_mut(|memo| {
        let months = not_heard_months();
        let cutoff = hour * 3600 - not_heard_secs(months);
        let fresh = memo.as_ref().is_some_and(|m| {
            same(&m.tracks, tracks)
                && same(&m.played, played)
                && m.hour == hour
                && m.months == months
        });
        if !fresh {
            *memo = Some(Memo {
                tracks: Arc::downgrade(tracks),
                played: Arc::downgrade(played),
                hour,
                months,
                stats: stats(tracks, played, cutoff),
                query: None,
                visible: Arc::default(),
                scope: Scope::All,
            });
        }
        let Some(m) = memo.as_mut() else {
            unreachable!("set above");
        };
        if !m.query.as_ref().is_some_and(|q| q.is(c)) {
            let (visible, scope) = filter(tracks, played, c, cutoff);
            m.visible = Arc::new(visible);
            m.scope = scope;
            m.query = Some(Query::of(c));
            #[cfg(test)]
            COMPUTES.with(|n| n.set(n.get() + 1));
        }
        (Arc::clone(&m.visible), m.scope.clone())
    })
}

/// The memoized header numbers; zero before the first [`update`].
fn memo_stats() -> Stats {
    MEMO.with_borrow(|m| m.as_ref().map(|m| m.stats).unwrap_or_default())
}

fn stats(tracks: &[Arc<Track>], played: &Played, cutoff: i64) -> Stats {
    Stats {
        not_heard: tracks
            .iter()
            .filter(|t| not_heard(t, played, cutoff))
            .count(),
        total: tracks.iter().filter_map(|t| t.duration).sum(),
    }
}

fn not_heard(track: &Track, played: &Played, cutoff: i64) -> bool {
    played.get(&track.id).is_none_or(|&at| at < cutoff)
}

/// Chips of one kind are alternatives (shift-click several artists: songs
/// by any of them); an artist chip and an album chip must both match.
fn chips_match(chips: &[Chip], track: &Track) -> bool {
    let kind_ok = |artist: bool| {
        let mut of_kind = chips
            .iter()
            .filter(|c| matches!(c, Chip::Artist(_)) == artist)
            .peekable();
        of_kind.peek().is_none() || of_kind.any(|c| chip_matches(c, track))
    };
    kind_ok(true) && kind_ok(false)
}

fn chip_matches(chip: &Chip, track: &Track) -> bool {
    match chip {
        Chip::Artist(name) => track.artists.iter().any(|a| &a.name == name),
        Chip::Album(name) => track.album.as_ref().is_some_and(|a| &a.name == name),
    }
}

fn push_lower(buf: &mut String, s: &str) {
    for c in s.chars() {
        buf.extend(c.to_lowercase());
    }
}

/// The range clamped to `n` tracks, `from <= to`. `None` when unset or
/// there are no tracks.
fn clamp_range(range: Option<(usize, usize)>, n: usize) -> Option<(usize, usize)> {
    let (a, b) = range?;
    let last = n.checked_sub(1)?;
    let to = a.max(b).min(last);
    Some((a.min(b).min(to), to))
}

/// The positions to show, in display order, and the scope. O(n), plus a
/// sort when one is picked.
fn filter(
    tracks: &[Arc<Track>],
    played: &Played,
    c: &CollectionView,
    cutoff: i64,
) -> (Vec<usize>, Scope) {
    let range = clamp_range(c.range, tracks.len());
    let span = match range {
        Some((from, to)) => from..to + 1,
        None if c.range.is_some() => 0..0,
        None => 0..tracks.len(),
    };
    let text = c.filter_text.trim();
    let mut needle = String::new();
    push_lower(&mut needle, text);
    let mut hay = String::new();
    let mut visible: Vec<usize> = span
        .filter(|&i| {
            let t = &tracks[i];
            if c.not_heard && !not_heard(t, played, cutoff) {
                return false;
            }
            if !chips_match(&c.chips, t) {
                return false;
            }
            if needle.is_empty() {
                return true;
            }
            // One reused buffer; '\n' keeps a match from spanning fields.
            hay.clear();
            push_lower(&mut hay, &t.title);
            for a in &t.artists {
                hay.push('\n');
                push_lower(&mut hay, &a.name);
            }
            if let Some(album) = &t.album {
                hay.push('\n');
                push_lower(&mut hay, &album.name);
            }
            hay.contains(&needle)
        })
        .collect();
    sort(&mut visible, tracks, played, c.sort);

    let filtered = !text.is_empty() || !c.chips.is_empty() || c.not_heard;
    // The scope is always what the table shows: a range with a filter on
    // top is a filter scope over the shown rows.
    let range = range.or(c.range);
    let scope = match (range, filtered) {
        (Some((from, to)), false) => Scope::Range { from, to },
        (range, true) => {
            let name = label(text, &c.chips);
            Scope::Filter {
                label: match range {
                    Some((from, to)) => format!("#{} to #{}, {name}", from + 1, to + 1),
                    None => name,
                },
                ids: Arc::new(visible.iter().map(|&i| tracks[i].id.clone()).collect()),
            }
        }
        (None, false) => Scope::All,
    };
    (visible, scope)
}

/// Ascending, ties in playlist order.
fn sort(rows: &mut [usize], tracks: &[Arc<Track>], played: &Played, by: Sort) {
    let lower = |s: &str| {
        let mut b = String::with_capacity(s.len());
        push_lower(&mut b, s);
        b
    };
    match by {
        Sort::PlaylistOrder => {}
        Sort::Title => rows.sort_by_cached_key(|&i| (lower(&tracks[i].title), i)),
        Sort::Artist => rows.sort_by_cached_key(|&i| (lower(&artists(&tracks[i])), i)),
        Sort::Album => rows.sort_by_cached_key(|&i| {
            let album = tracks[i].album.as_ref().map_or("", |a| a.name.as_str());
            (lower(album), i)
        }),
        // Never played sorts first: it is the longest unheard.
        Sort::LastPlayed => rows.sort_by_key(|&i| (played.get(&tracks[i].id).copied(), i)),
        Sort::Time => rows.sort_by_key(|&i| (tracks[i].duration, i)),
    }
}

fn chip_name(chip: &Chip) -> &str {
    match chip {
        Chip::Artist(name) | Chip::Album(name) => name,
    }
}

/// The "Playing from" name of a filter scope: the single chip's name, else
/// the filter text in quotes, else the chips, else the quick filter.
fn label(text: &str, chips: &[Chip]) -> String {
    match chips {
        [one] => chip_name(one).to_owned(),
        _ if !text.is_empty() => format!("\u{201c}{text}\u{201d}"),
        [] => not_heard_label(),
        many => many.iter().map(chip_name).collect::<Vec<_>>().join(", "),
    }
}

// -------------------------------------------------------------------- text

/// `1535` as "1,535".
fn group(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn songs(n: usize) -> String {
    if n == 1 {
        "1 song".to_owned()
    } else {
        format!("{} songs", group(n))
    }
}

/// "98 hr 12 min", or "42 min" under an hour.
fn hours(total: Duration) -> String {
    let min = total.as_secs() / 60;
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} hr {m} min"),
    }
}

fn chip_text(chip: &Chip) -> String {
    match chip {
        Chip::Artist(name) => format!("Artist: {name}"),
        Chip::Album(name) => format!("Album: {name}"),
    }
}

// -------------------------------------------------------------------- show

pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    let main = match &state.main {
        Some((_, Loadable::Ready(v))) => Arc::clone(v),
        _ => return not_ready(ui, state, view),
    };
    header(ui, &main, &view.collection, out);
    filter_row(ui, &main, &mut view.collection);
    let visible = Arc::clone(&view.collection.visible);
    let scroll_to = view.collection.scroll_to.take();
    // The spine draws in the column to the right of the table.
    let rest = ui.available_rect_before_wrap();
    let table = Rect::from_min_max(
        rest.min,
        Pos2::new(
            (rest.right()
                - if state.settings.spine {
                    theme::SPINE_COLUMN_W
                } else {
                    0.0
                })
            .max(rest.left()),
            rest.bottom(),
        ),
    );
    let shown = ui
        .scope_builder(UiBuilder::new().max_rect(table), |ui| {
            track_table::show(
                ui,
                state,
                view,
                track_table::Table {
                    salt: "collection",
                    tracks: &main.tracks,
                    rows: Some(&visible),
                    columns: track_table::COLLECTION,
                    scroll_to,
                },
                out,
            )
        })
        .inner;
    // The spine's viewport box only means something in playlist order.
    let on = shown
        .on_screen
        .filter(|_| view.collection.sort == Sort::PlaylistOrder);
    if on != view.collection.on_screen {
        view.collection.on_screen = on;
        ui.ctx().request_repaint();
    }
}

/// Guest, no main playlist, loading or failed.
fn not_ready(ui: &mut Ui, state: &State, view: &mut View) {
    let p = theme::pal();
    view.collection.on_screen = None;
    let message = |ui: &mut Ui, text: &str| {
        ui.add_space(theme::SPACE_24);
        ui.horizontal(|ui| {
            ui.add_space(theme::SPACE_20);
            ui.label(RichText::new(text).font(theme::row()).color(p.text_2));
        });
    };
    let Some((id, page)) = &state.main else {
        let text = if state.needs_login() {
            NEEDS_LOGIN
        } else {
            "Pick your main playlist in Library."
        };
        return message(ui, text);
    };
    if state.needs_login() && !matches!(page, Loadable::Failed(_)) {
        return message(ui, NEEDS_LOGIN);
    }
    let name = state
        .library
        .ready()
        .and_then(|l| l.iter().find(|s| &s.id == id))
        .map_or("Your playlist", |s| s.name.as_str());

    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::PAGE_HEADER_H),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.hline(rect.x_range(), rect.bottom() - 0.5, theme::hairline());
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left() + theme::SPACE_20, rect.center().y - 20.0),
        vec2(40.0, 40.0),
    );
    painter.rect_filled(art_rect, CornerRadius::same(theme::RADIUS_ROW), p.surface_2);
    let x = art_rect.right() + theme::SPACE_12;
    let w = rect.right() - theme::SPACE_20 - x;
    text_line(
        painter,
        Pos2::new(x, rect.top() + 22.5),
        name,
        theme::page_title(),
        p.text_1,
        w,
    );
    let (sub, color) = match page {
        Loadable::Failed(e) => (format!("Could not load songs: {e}"), p.danger),
        _ => ("Loading songs".to_owned(), p.text_2),
    };
    text_line(
        painter,
        Pos2::new(x, rect.top() + 44.0),
        &sub,
        theme::secondary(),
        color,
        w,
    );
    if matches!(page, Loadable::Failed(_)) {
        return;
    }

    let (row, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::FILTER_ROW_H),
        Sense::hover(),
    );
    let painter = ui.painter();
    painter.hline(row.x_range(), row.bottom() - 0.5, theme::hairline());
    let field = Rect::from_min_size(
        Pos2::new(
            row.left() + theme::SPACE_20,
            row.center().y - theme::FIELD_H / 2.0,
        ),
        vec2(
            440.0f32.min(row.width() - 2.0 * theme::SPACE_20),
            theme::FIELD_H,
        ),
    );
    painter.rect_stroke(
        field,
        CornerRadius::same(theme::RADIUS_CONTROL),
        Stroke::new(theme::STROKE_W, p.stroke),
        StrokeKind::Inside,
    );
    text_line(
        painter,
        Pos2::new(field.left() + 10.0, field.center().y),
        "Filter is ready when loading finishes",
        theme::row(),
        p.text_3,
        field.width() - 20.0,
    );
    let rows = (ui.available_height() / theme::row_h())
        .ceil()
        .clamp(1.0, 40.0) as usize;
    track_table::skeleton(ui, rows);
}

// ------------------------------------------------------------------ header

fn header(ui: &mut Ui, main: &PlaylistView, c: &CollectionView, out: &mut Out) {
    let p = theme::pal();
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::PAGE_HEADER_H),
        Sense::hover(),
    );
    ui.painter()
        .hline(rect.x_range(), rect.bottom() - 0.5, theme::hairline());
    let cy = rect.center().y;
    let left = rect.left() + theme::SPACE_20;
    let right = rect.right() - theme::SPACE_20;

    let art_rect = Rect::from_min_size(Pos2::new(left, cy - 20.0), vec2(40.0, 40.0));
    playlist_art(ui, art_rect, main);

    let n = main.tracks.len();
    let scope = c.scope.clone().unwrap_or(Scope::All);
    // Every scope is exactly the shown rows.
    let count = c.visible.len();
    let (shuffle, in_order) = if scope == Scope::All {
        ("Shuffle all".to_owned(), "Play in order".to_owned())
    } else {
        (
            format!("Shuffle {}", group(count)),
            format!("Play {} in order", group(count)),
        )
    };
    let order = Button::new(ui, icons::PLAY, theme::icon_fill(12.0), &in_order, false);
    let order_rect = Rect::from_min_size(
        Pos2::new(right - order.width(), cy - theme::BUTTON_H / 2.0),
        vec2(order.width(), theme::BUTTON_H),
    );
    let shuf = Button::new(ui, icons::SHUFFLE, theme::icon(14.0), &shuffle, true);
    let shuf_rect = Rect::from_min_size(
        Pos2::new(
            order_rect.left() - theme::SPACE_12 - shuf.width(),
            order_rect.top(),
        ),
        vec2(shuf.width(), theme::BUTTON_H),
    );
    let enabled = count > 0;
    for (button, rect, shuffle) in [(shuf, shuf_rect, true), (order, order_rect, false)] {
        let salt = if shuffle { "shuffle" } else { "in-order" };
        if button.show(ui, rect, salt, enabled).clicked() {
            cmd(
                out,
                Command::PlayScope {
                    scope: scope.clone(),
                    shuffle,
                    seed: seed(),
                },
            );
        }
    }

    let x = art_rect.right() + theme::SPACE_12;
    let w = shuf_rect.left() - theme::SPACE_12 - x;
    let painter = ui.painter();
    text_line(
        painter,
        Pos2::new(x, rect.top() + 22.5),
        &main.summary.name,
        theme::page_title(),
        p.text_1,
        w,
    );
    let stats = memo_stats();
    text_line(
        painter,
        Pos2::new(x, rect.top() + 44.0),
        &format!("Playlist \u{b7} {} \u{b7} {}", songs(n), hours(stats.total)),
        theme::secondary(),
        p.text_2,
        w,
    );
}

/// The playlist's cover, else a 2x2 mosaic of its first four songs' art.
fn playlist_art(ui: &Ui, rect: Rect, main: &PlaylistView) {
    let painter = ui.painter();
    let summary = &main.summary;
    if !summary.thumbnails.is_empty() || main.tracks.len() < 4 {
        match main
            .tracks
            .first()
            .filter(|_| summary.thumbnails.is_empty())
        {
            Some(t) => art(
                painter,
                rect,
                &t.id,
                &t.title,
                ArtKind::Square(&t.thumbnails),
            ),
            None => art(
                painter,
                rect,
                &summary.id,
                &summary.name,
                ArtKind::Square(&summary.thumbnails),
            ),
        }
        return;
    }
    let half = rect.width() / 2.0;
    for (k, t) in main.tracks.iter().take(4).enumerate() {
        let min = rect.min + vec2((k % 2) as f32 * half, (k / 2) as f32 * half);
        let tile = Rect::from_min_size(min, vec2(half, half));
        art(
            painter,
            tile,
            &t.id,
            &t.title,
            ArtKind::Square(&t.thumbnails),
        );
    }
}

/// A 32 px header button: the primary one in the accent, the secondary
/// one outlined.
struct Button {
    icon: Arc<Galley>,
    text: Arc<Galley>,
    primary: bool,
}

impl Button {
    const PAD: f32 = 14.0;

    fn new(ui: &Ui, icon: &str, icon_font: FontId, text: &str, primary: bool) -> Self {
        let font = if primary {
            theme::row_strong()
        } else {
            theme::row()
        };
        let painter = ui.painter();
        Self {
            icon: painter.layout_no_wrap(icon.to_owned(), icon_font, Color32::PLACEHOLDER),
            text: painter.layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER),
            primary,
        }
    }

    fn width(&self) -> f32 {
        2.0 * Self::PAD + self.icon.size().x + theme::SPACE_8 + self.text.size().x
    }

    fn show(self, ui: &mut Ui, rect: Rect, salt: &str, enabled: bool) -> Response {
        let p = theme::pal();
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let resp = ui.interact(rect, ui.scope_id().with(salt), sense);
        let down = resp.is_pointer_button_down_on();
        let radius = CornerRadius::same(theme::RADIUS_CONTROL);
        let painter = ui.painter();
        let fg = if self.primary {
            let fill = match (enabled, down, resp.hovered()) {
                (false, ..) => p.surface_3,
                (_, true, _) => theme::accent_pressed(),
                (_, _, true) => theme::accent_hover(),
                _ => theme::accent(),
            };
            painter.rect_filled(rect, radius, fill);
            if enabled { p.on_accent } else { p.text_3 }
        } else {
            if enabled && (down || resp.hovered()) {
                let fill = if down { p.surface_3 } else { p.surface_2 };
                painter.rect_filled(rect, radius, fill);
            }
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(theme::STROKE_W, p.stroke_2),
                StrokeKind::Inside,
            );
            if enabled { p.text_1 } else { p.text_3 }
        };
        let x = rect.left() + Self::PAD;
        let cy = rect.center().y;
        let icon_pos = Pos2::new(x, cy - self.icon.size().y / 2.0);
        let text_pos = Pos2::new(
            x + self.icon.size().x + theme::SPACE_8,
            cy - self.text.size().y / 2.0,
        );
        painter.galley(icon_pos, self.icon, fg);
        painter.galley(text_pos, self.text, fg);
        resp
    }
}

// -------------------------------------------------------------- filter row

/// What a chip's × removes.
enum Remove {
    Range,
    Chip(usize),
}

fn filter_row(ui: &mut Ui, main: &PlaylistView, c: &mut CollectionView) {
    let (row, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::FILTER_ROW_H),
        Sense::hover(),
    );
    ui.painter()
        .hline(row.x_range(), row.bottom() - 0.5, theme::hairline());
    let cy = row.center().y;
    let left = row.left() + theme::SPACE_20;
    let right = row.right() - theme::SPACE_20;
    let n = main.tracks.len();
    let stats = memo_stats();

    // Right to left: sort menu, then the quick filter after the field.
    let count = group(stats.not_heard);
    let (sort, quick) = fit_controls(ui, c, &count, right - left);
    let sort_rect = sort.show(ui, right, cy, c);
    let field_w = 440.0f32
        .min(sort_rect.left() - theme::SPACE_8 - quick.width() - theme::SPACE_8 - left)
        .max(160.0);
    let field = Rect::from_min_size(
        Pos2::new(left, cy - theme::FIELD_H / 2.0),
        vec2(field_w, theme::FIELD_H),
    );
    let quick_rect = Rect::from_min_size(
        Pos2::new(
            field.right() + theme::SPACE_8,
            cy - theme::ICON_BUTTON / 2.0,
        ),
        vec2(quick.width(), theme::ICON_BUTTON),
    );
    if quick.show(ui, quick_rect, c.not_heard, &count).clicked() {
        c.not_heard = !c.not_heard;
    }

    filter_field(ui, field, n, c);
}

/// The sort menu and quick filter for a row `width` wide. When the field
/// would get narrower than `FIELD_MIN_W` (the 780 px minimum window with
/// the browse panel open), they give up text in this order: the quick
/// filter's label, "Sort", the quick filter's count. Each stays named on
/// hover.
fn fit_controls(ui: &Ui, c: &CollectionView, count: &str, width: f32) -> (SortMenu, Pill) {
    [
        (true, true, true),
        (false, true, true),
        (false, false, true),
    ]
    .into_iter()
    .map(|(label, sort_label, count_shown)| {
        (
            SortMenu::new(ui, c, sort_label),
            Pill::new(ui, label, count, count_shown),
        )
    })
    .find(|(sort, quick)| {
        width - sort.width() - quick.width() - 2.0 * theme::SPACE_8 >= FIELD_MIN_W
    })
    .unwrap_or_else(|| {
        (
            SortMenu::new(ui, c, false),
            Pill::new(ui, false, count, false),
        )
    })
}

fn filter_field(ui: &mut Ui, field: Rect, n: usize, c: &mut CollectionView) {
    let p = theme::pal();
    let id = ui.scope_id().with("collection-filter");
    // Under the chips and the text: a click on the field's padding focuses
    // the text.
    let back = ui.interact(field, id.with("back"), Sense::click());

    let cy = field.center().y;
    let mut x = field.left() + 10.0;
    ui.painter().text(
        Pos2::new(x, cy),
        Align2::LEFT_CENTER,
        icons::FUNNEL,
        theme::icon(14.0),
        p.text_3,
    );
    x += 14.0 + theme::SPACE_8;

    let active =
        c.range.is_some() || !c.chips.is_empty() || c.not_heard || !c.filter_text.trim().is_empty();
    let count = active.then(|| format!("{} of {}", group(c.visible.len()), group(n)));
    let count_w = count.as_ref().map_or(0.0, |t| {
        ui.painter()
            .layout_no_wrap(t.clone(), theme::caption(), p.text_3)
            .size()
            .x
            + theme::SPACE_8
    });
    let end = field.right() - 10.0 - count_w;

    let range = clamp_range(c.range, n).or(c.range);
    let mut chips: Vec<(String, Remove)> = Vec::with_capacity(c.chips.len() + 1);
    if let Some((from, to)) = range {
        chips.push((format!("#{} to #{}", from + 1, to + 1), Remove::Range));
    }
    chips.extend(
        c.chips
            .iter()
            .enumerate()
            .map(|(i, chip)| (chip_text(chip), Remove::Chip(i))),
    );
    let mut remove = None;
    let mut hidden: Vec<String> = Vec::new();
    for (k, (text, what)) in chips.into_iter().enumerate() {
        let room = end - x - 80.0;
        if room < 48.0 || !hidden.is_empty() {
            hidden.push(text);
            continue;
        }
        let w = chip(ui, Pos2::new(x, cy), &text, room, id.with(("chip", k)));
        if w.1 {
            remove = Some(what);
        }
        x += w.0 + theme::SPACE_8;
    }
    // Chips that do not fit (several artists picked with shift): "+N", with
    // their names on hover.
    if !hidden.is_empty() {
        let galley =
            ui.painter()
                .layout_no_wrap(format!("+{}", hidden.len()), theme::secondary(), p.text_2);
        let rect = Rect::from_min_size(Pos2::new(x, cy - galley.size().y / 2.0), galley.size());
        ui.painter().galley(rect.min, galley, p.text_2);
        ui.interact(rect, id.with("more-chips"), Sense::hover())
            .on_hover_text(hidden.join("\n"));
        x = rect.right() + theme::SPACE_8;
    }

    let hint = if active {
        format!("Filter {}", songs(c.visible.len()))
    } else {
        format!("Filter {}", songs(n))
    };
    let was_empty = c.filter_text.is_empty();
    let edit_rect = Rect::from_min_max(Pos2::new(x, field.top()), Pos2::new(end, field.bottom()));
    let edit = ui.put(
        edit_rect,
        TextEdit::singleline(&mut c.filter_text)
            .id(id)
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .font(theme::row())
            .text_color(p.text_1)
            .hint_text(RichText::new(hint).font(theme::row()).color(p.text_3))
            .align(Align2::LEFT_CENTER)
            .min_size(edit_rect.size())
            .desired_width(edit_rect.width()),
    );
    if back.clicked() {
        edit.request_focus();
    }
    if c.focus_filter {
        edit.request_focus();
        c.focus_filter = false;
    }
    // Escape clears the text and keeps focus, so the same press does not
    // also reach the spine (it clears the range on Escape when nothing has
    // focus). Escape in an empty field lets go of focus.
    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) && !c.filter_text.is_empty() {
        c.filter_text.clear();
        edit.request_focus();
    }
    // Backspace in an empty field takes off the last chip.
    if edit.has_focus() && was_empty && ui.input(|i| i.key_pressed(Key::Backspace)) {
        remove = if c.chips.is_empty() {
            c.range.is_some().then_some(Remove::Range)
        } else {
            Some(Remove::Chip(c.chips.len() - 1))
        };
    }
    match remove {
        Some(Remove::Range) => c.range = None,
        Some(Remove::Chip(i)) if i < c.chips.len() => {
            c.chips.remove(i);
        }
        _ => {}
    }

    let painter = ui.painter();
    if let Some(count) = count {
        painter.text(
            Pos2::new(field.right() - 10.0, cy),
            Align2::RIGHT_CENTER,
            count,
            theme::caption(),
            p.text_3,
        );
    }
    let stroke = if edit.has_focus() {
        Stroke::new(theme::FOCUS_W, theme::accent())
    } else {
        Stroke::new(theme::STROKE_W, p.stroke_2)
    };
    painter.rect_stroke(
        field,
        CornerRadius::same(theme::RADIUS_CONTROL),
        stroke,
        StrokeKind::Inside,
    );
}

/// A removable chip in the filter field, left-centered at `at`. Returns
/// its width and whether its × was clicked.
fn chip(ui: &mut Ui, at: Pos2, text: &str, max_w: f32, id: egui::Id) -> (f32, bool) {
    const H: f32 = 22.0;
    const X: f32 = 18.0;
    let p = theme::pal();
    // D-Tokens: chips are 12 / 400.
    let font = theme::secondary();
    let text_w = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), p.text_1)
        .size()
        .x
        .min(max_w - 8.0 - X - 2.0);
    let w = 8.0 + text_w + 4.0 + X + 2.0;
    let rect = Rect::from_min_size(Pos2::new(at.x, at.y - H / 2.0), vec2(w, H));
    let x_rect = Rect::from_center_size(Pos2::new(rect.right() - 2.0 - X / 2.0, at.y), vec2(X, X));
    let resp = ui
        .interact(x_rect, id, Sense::click())
        .on_hover_text("Remove filter");
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_ROW), p.surface_3);
    text_line(
        painter,
        Pos2::new(rect.left() + 8.0, at.y),
        text,
        font,
        p.text_1,
        text_w + 1.0,
    );
    if resp.hovered() {
        painter.rect_filled(x_rect, CornerRadius::same(theme::RADIUS_ROW), p.stroke_2);
    }
    painter.text(
        x_rect.center(),
        Align2::CENTER_CENTER,
        icons::X,
        theme::icon(11.0),
        if resp.hovered() { p.text_1 } else { p.text_2 },
    );
    (w, resp.clicked())
}

/// The "Not heard in 3 months 412" toggle. In a narrow row it drops the
/// label, then the count, down to the icon.
struct Pill {
    label: Option<Arc<Galley>>,
    count: Option<Arc<Galley>>,
}

impl Pill {
    const PAD: f32 = 10.0;
    const GAP: f32 = 6.0;
    const ICON: f32 = 14.0;

    fn new(ui: &Ui, label: bool, count: &str, count_shown: bool) -> Self {
        let painter = ui.painter();
        let layout = |text: &str| {
            painter.layout_no_wrap(text.to_owned(), theme::secondary(), Color32::PLACEHOLDER)
        };
        Self {
            label: label.then(|| layout(&not_heard_label())),
            count: count_shown.then(|| layout(count)),
        }
    }

    fn width(&self) -> f32 {
        let text: f32 = [&self.label, &self.count]
            .into_iter()
            .flatten()
            .map(|g| Self::GAP + g.size().x)
            .sum();
        2.0 * Self::PAD + Self::ICON + text
    }

    fn show(self, ui: &mut Ui, rect: Rect, on: bool, count: &str) -> Response {
        let hint = format!(
            "Songs never played here, or last played over {} days ago",
            not_heard_secs(not_heard_months()) / 86_400
        );
        let p = theme::pal();
        let hover = if self.label.is_some() {
            hint
        } else {
            format!("{} \u{b7} {count}\n{hint}", not_heard_label())
        };
        let resp = ui
            .interact(rect, ui.scope_id().with("not-heard"), Sense::click())
            .on_hover_text(hover);
        let painter = ui.painter();
        let radius = CornerRadius::same(theme::RADIUS_CONTROL);
        let (fill, stroke) = match (on, resp.hovered()) {
            (true, _) => (Some(p.surface_3), p.stroke_2),
            (false, true) => (Some(p.surface_2), p.stroke),
            (false, false) => (None, p.stroke),
        };
        if let Some(fill) = fill {
            painter.rect_filled(rect, radius, fill);
        }
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(theme::STROKE_W, stroke),
            StrokeKind::Inside,
        );
        let cy = rect.center().y;
        let mut x = rect.left() + Self::PAD;
        painter.text(
            Pos2::new(x + Self::ICON / 2.0, cy),
            Align2::CENTER_CENTER,
            icons::CLOCK_COUNTER_CLOCKWISE,
            theme::icon(Self::ICON),
            if on { p.text_1 } else { p.text_2 },
        );
        x += Self::ICON + Self::GAP;
        for (galley, color) in [(self.label, p.text_1), (self.count, p.text_3)] {
            let Some(galley) = galley else { continue };
            let size = galley.size();
            painter.galley(Pos2::new(x, cy - size.y / 2.0), galley, color);
            x += size.x + Self::GAP;
        }
        resp
    }
}

/// "Sort  Playlist order ⌄"; in a narrow row, "Playlist order ⌄".
struct SortMenu {
    label: Option<Arc<Galley>>,
    value: Arc<Galley>,
    caret: Arc<Galley>,
}

impl SortMenu {
    const PAD: f32 = 8.0;
    const GAP: f32 = 6.0;

    fn new(ui: &Ui, c: &CollectionView, label: bool) -> Self {
        let p = theme::pal();
        let current = SORTS
            .iter()
            .find(|(s, _)| *s == c.sort)
            .map_or("Playlist order", |(_, name)| name);
        let painter = ui.painter();
        Self {
            label: label
                .then(|| painter.layout_no_wrap("Sort".to_owned(), theme::secondary(), p.text_2)),
            value: painter.layout_no_wrap(current.to_owned(), theme::secondary(), p.text_1),
            caret: painter.layout_no_wrap(
                icons::CARET_DOWN.to_owned(),
                theme::icon(12.0),
                p.text_2,
            ),
        }
    }

    fn width(&self) -> f32 {
        let label = self.label.as_ref().map_or(0.0, |g| g.size().x + Self::GAP);
        2.0 * Self::PAD + label + self.value.size().x + Self::GAP + self.caret.size().x
    }

    /// Right-aligned at `right`. Returns its rect.
    fn show(self, ui: &mut Ui, right: f32, cy: f32, c: &mut CollectionView) -> Rect {
        let p = theme::pal();
        let w = self.width();
        let rect = Rect::from_min_size(
            Pos2::new(right - w, cy - theme::ICON_BUTTON / 2.0),
            vec2(w, theme::ICON_BUTTON),
        );
        let mut resp = ui.interact(rect, ui.scope_id().with("sort"), Sense::click());
        if self.label.is_none() {
            resp = resp.on_hover_text("Sort");
        }
        let painter = ui.painter();
        if resp.hovered() || Popup::is_id_open(ui.ctx(), Popup::default_response_id(&resp)) {
            painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_CONTROL), p.surface_2);
        }
        let mut x = rect.left() + Self::PAD;
        for g in [self.label, Some(self.value), Some(self.caret)]
            .into_iter()
            .flatten()
        {
            let size = g.size();
            painter.galley(Pos2::new(x, cy - size.y / 2.0), g, p.text_1);
            x += size.x + Self::GAP;
        }
        Popup::menu(&resp).show(|ui| {
            for (sort, name) in SORTS {
                if ui.selectable_label(c.sort == sort, name).clicked() {
                    c.sort = sort;
                    ui.close();
                }
            }
        });
        rect
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use model::{AlbumRef, ArtistRef, PlaylistSummary};

    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    fn track(id: &str, title: &str, artist: &str, album: &str, secs: u64) -> Arc<Track> {
        Arc::new(Track {
            id: id.to_owned(),
            title: title.to_owned(),
            artists: vec![ArtistRef {
                id: None,
                name: artist.to_owned(),
            }],
            album: Some(AlbumRef {
                id: format!("al-{album}"),
                name: album.to_owned(),
            }),
            duration: Some(Duration::from_secs(secs)),
            thumbnails: Vec::new(),
        })
    }

    fn state(tracks: Vec<Arc<Track>>, played: &[(&str, i64)]) -> State {
        let entry_ids = Arc::new(vec![None; tracks.len()]);
        let view = PlaylistView {
            summary: PlaylistSummary {
                id: "main".to_owned(),
                name: "Everything".to_owned(),
                owner: None,
                track_count: Some(tracks.len() as u64),
                thumbnails: Vec::new(),
            },
            tracks: Arc::new(tracks),
            entry_ids,
        };
        State {
            main: Some(("main".to_owned(), Loadable::Ready(Arc::new(view)))),
            last_played: Arc::new(
                played
                    .iter()
                    .map(|(id, at)| ((*id).to_owned(), *at))
                    .collect(),
            ),
            ..State::default()
        }
    }

    /// 0 Lantern Season, Saint Juniper, Small Hours (played 10 days ago)
    /// 1 Halflight, Static Bloom, Parallax (played 200 days ago)
    /// 2 Wintering, Saint Juniper, Halogen (never)
    /// 3 Copper Wire, Wren & Ash, Halogen (played 1 day ago)
    /// 4 Juniper Smoke, Mara, Small Hours (never)
    fn demo() -> State {
        state(
            vec![
                track("a", "Lantern Season", "Saint Juniper", "Small Hours", 261),
                track("b", "Halflight", "Static Bloom", "Parallax", 408),
                track("c", "Wintering", "Saint Juniper", "Halogen", 228),
                track("d", "Copper Wire", "Wren & Ash", "Halogen", 161),
                track("e", "juniper smoke", "Mara", "Small Hours", 302),
            ],
            &[
                ("a", NOW - 10 * DAY),
                ("b", NOW - 200 * DAY),
                ("d", NOW - DAY),
            ],
        )
    }

    fn run(s: &State, v: &mut View) -> Vec<usize> {
        update_at(s, v, NOW);
        v.collection.visible.to_vec()
    }

    fn label_of(v: &View) -> String {
        match &v.collection.scope {
            Some(Scope::Filter { label, .. }) => label.clone(),
            other => panic!("not a filter scope: {other:?}"),
        }
    }

    #[test]
    fn no_filter_shows_all_in_playlist_order() {
        let s = demo();
        let mut v = View::default();
        assert_eq!(run(&s, &mut v), [0, 1, 2, 3, 4]);
        assert_eq!(v.collection.scope, Some(Scope::All));
    }

    #[test]
    fn text_matches_title_artist_album_case_insensitively() {
        let s = demo();
        let mut v = View::default();
        v.collection.filter_text = "  JUNIPER ".to_owned();
        assert_eq!(run(&s, &mut v), [0, 2, 4]);
        v.collection.filter_text = "halogen".to_owned();
        assert_eq!(run(&s, &mut v), [2, 3]);
        v.collection.filter_text = "copper".to_owned();
        assert_eq!(run(&s, &mut v), [3]);
        // A match may not span two fields.
        v.collection.filter_text = "season saint".to_owned();
        assert_eq!(run(&s, &mut v), Vec::<usize>::new());
    }

    #[test]
    fn chips_match_exact_names_and_and_with_text() {
        let s = demo();
        let mut v = View::default();
        v.collection.chips = vec![Chip::Artist("Saint Juniper".to_owned())];
        assert_eq!(run(&s, &mut v), [0, 2]);
        v.collection.chips = vec![Chip::Album("Halogen".to_owned())];
        assert_eq!(run(&s, &mut v), [2, 3]);
        v.collection.chips = vec![
            Chip::Artist("Saint Juniper".to_owned()),
            Chip::Album("Halogen".to_owned()),
        ];
        assert_eq!(run(&s, &mut v), [2]);
        v.collection.chips = vec![Chip::Artist("Saint Juniper".to_owned())];
        v.collection.filter_text = "lantern".to_owned();
        assert_eq!(run(&s, &mut v), [0]);
        // Exact name, not substring.
        v.collection.filter_text.clear();
        v.collection.chips = vec![Chip::Artist("Saint".to_owned())];
        assert_eq!(run(&s, &mut v), Vec::<usize>::new());
        // Several artists: songs by any of them.
        v.collection.chips = vec![
            Chip::Artist("Saint Juniper".to_owned()),
            Chip::Artist("Static Bloom".to_owned()),
        ];
        let any = run(&s, &mut v);
        v.collection.chips = vec![Chip::Artist("Static Bloom".to_owned())];
        let bloom = run(&s, &mut v);
        assert!(!bloom.is_empty());
        let mut want: Vec<usize> = [0, 2].into_iter().chain(bloom).collect();
        want.sort_unstable();
        want.dedup();
        assert_eq!(any, want);
    }

    #[test]
    fn not_heard_is_never_or_over_90_days() {
        let s = demo();
        let mut v = View::default();
        v.collection.not_heard = true;
        assert_eq!(run(&s, &mut v), [1, 2, 4]);
        assert_eq!(memo_stats().not_heard, 3);
        // The count is the quick filter alone, whatever else is on.
        v.collection.filter_text = "juniper".to_owned();
        assert_eq!(run(&s, &mut v), [2, 4]);
        assert_eq!(memo_stats().not_heard, 3);
        assert_eq!(memo_stats().total, Duration::from_secs(1360));
    }

    #[test]
    fn range_filters_and_wins_scope() {
        let s = demo();
        let mut v = View::default();
        v.collection.range = Some((1, 3));
        assert_eq!(run(&s, &mut v), [1, 2, 3]);
        assert_eq!(v.collection.scope, Some(Scope::Range { from: 1, to: 3 }));
        v.collection.filter_text = "juniper".to_owned();
        assert_eq!(run(&s, &mut v), [2]);
        // A range with a filter on top: the scope is what the table shows.
        assert_eq!(label_of(&v), "#2 to #4, \u{201c}juniper\u{201d}");
        let Some(Scope::Filter { ids, .. }) = &v.collection.scope else {
            unreachable!()
        };
        assert_eq!(ids.as_slice(), ["c"]);
        v.collection.filter_text.clear();
        v.collection.chips = vec![Chip::Album("Halogen".to_owned())];
        assert_eq!(run(&s, &mut v), [2, 3]);
        assert_eq!(label_of(&v), "#2 to #4, Halogen");
        v.collection.chips.clear();
        // Clamped to the playlist.
        v.collection.filter_text.clear();
        v.collection.range = Some((3, 99));
        assert_eq!(run(&s, &mut v), [3, 4]);
        assert_eq!(v.collection.scope, Some(Scope::Range { from: 3, to: 4 }));
    }

    #[test]
    fn scope_labels_and_ids() {
        let s = demo();
        let mut v = View::default();
        v.collection.chips = vec![Chip::Artist("Saint Juniper".to_owned())];
        v.collection.filter_text = "lantern".to_owned();
        run(&s, &mut v);
        assert_eq!(label_of(&v), "Saint Juniper");
        let Some(Scope::Filter { ids, .. }) = &v.collection.scope else {
            unreachable!()
        };
        assert_eq!(ids.as_slice(), ["a"]);

        v.collection.chips.clear();
        run(&s, &mut v);
        assert_eq!(label_of(&v), "\u{201c}lantern\u{201d}");

        v.collection.filter_text.clear();
        v.collection.not_heard = true;
        run(&s, &mut v);
        assert_eq!(label_of(&v), "Not heard in 3 months");

        v.collection.not_heard = false;
        v.collection.chips = vec![
            Chip::Artist("Saint Juniper".to_owned()),
            Chip::Album("Halogen".to_owned()),
        ];
        run(&s, &mut v);
        assert_eq!(label_of(&v), "Saint Juniper, Halogen");

        v.collection.chips.clear();
        v.collection.filter_text = "   ".to_owned();
        run(&s, &mut v);
        assert_eq!(v.collection.scope, Some(Scope::All));
    }

    #[test]
    fn sorts_ascending_with_ties_in_playlist_order() {
        let s = demo();
        let mut v = View::default();
        let mut by = |sort| {
            v.collection.sort = sort;
            run(&s, &mut v)
        };
        assert_eq!(by(Sort::Title), [3, 1, 4, 0, 2]);
        assert_eq!(by(Sort::Artist), [4, 0, 2, 1, 3]);
        assert_eq!(by(Sort::Album), [2, 3, 1, 0, 4]);
        assert_eq!(by(Sort::LastPlayed), [2, 4, 1, 0, 3]);
        assert_eq!(by(Sort::Time), [3, 2, 0, 4, 1]);
        assert_eq!(by(Sort::PlaylistOrder), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn unchanged_frame_is_a_memo_hit() {
        let s = demo();
        let mut v = View::default();
        let computes = || COMPUTES.with(std::cell::Cell::get);
        v.collection.filter_text = "juniper".to_owned();
        update_at(&s, &mut v, NOW);
        let first = Arc::clone(&v.collection.visible);
        let n = computes();
        for _ in 0..3 {
            update_at(&s, &mut v, NOW + 60);
        }
        assert_eq!(computes(), n);
        assert!(Arc::ptr_eq(&first, &v.collection.visible));

        v.collection.filter_text.push('x');
        update_at(&s, &mut v, NOW);
        assert_eq!(computes(), n + 1);

        // New data: a new tracks Arc recomputes even with the same inputs.
        let s2 = demo();
        update_at(&s2, &mut v, NOW);
        assert_eq!(computes(), n + 2);
        // So does the next hour (the 90-day cutoff moves).
        update_at(&s2, &mut v, NOW + 3600);
        assert_eq!(computes(), n + 3);
    }

    #[test]
    fn scroll_to_a_hidden_row_moves_to_the_nearest_shown_row() {
        let s = demo();
        let mut v = View::default();
        v.collection.chips = vec![Chip::Artist("Saint Juniper".to_owned())];
        v.collection.scroll_to = Some(2);
        assert_eq!(run(&s, &mut v), [0, 2]);
        assert_eq!(v.collection.scroll_to, Some(2), "shown: unchanged");

        // Hidden: the first shown at or after it, filters kept.
        v.collection.scroll_to = Some(1);
        run(&s, &mut v);
        assert_eq!(v.collection.scroll_to, Some(2));
        assert_eq!(v.collection.chips.len(), 1);
        // Nothing after it: the last shown before it.
        v.collection.scroll_to = Some(4);
        run(&s, &mut v);
        assert_eq!(v.collection.scroll_to, Some(2));
        // Sorted tables still pick by position.
        v.collection.sort = Sort::Title;
        v.collection.scroll_to = Some(1);
        run(&s, &mut v);
        assert_eq!(v.collection.scroll_to, Some(2));
        // Nothing shown: no scroll.
        v.collection.filter_text = "zzz".to_owned();
        v.collection.scroll_to = Some(1);
        assert_eq!(run(&s, &mut v), Vec::<usize>::new());
        assert_eq!(v.collection.scroll_to, None);
    }

    #[test]
    fn no_main_playlist_clears_visible() {
        let mut v = View::default();
        v.collection.visible = Arc::new(vec![1, 2]);
        update_at(&State::default(), &mut v, NOW);
        assert!(v.collection.visible.is_empty());
        assert_eq!(v.collection.scope, None);
    }

    #[test]
    fn text_helpers() {
        assert_eq!(group(1535), "1,535");
        assert_eq!(group(41), "41");
        assert_eq!(songs(1), "1 song");
        assert_eq!(songs(1535), "1,535 songs");
        assert_eq!(
            hours(Duration::from_secs(98 * 3600 + 12 * 60 + 5)),
            "98 hr 12 min"
        );
        assert_eq!(hours(Duration::from_secs(42 * 60)), "42 min");
        assert_eq!(clamp_range(Some((5, 2)), 10), Some((2, 5)));
        assert_eq!(clamp_range(Some((0, 3)), 0), None);
    }

    #[test]
    fn filtering_1535_tracks_is_fast() {
        let artists = [
            "Saint Juniper",
            "Static Bloom",
            "Wren & Ash",
            "Mara Lindqvist",
        ];
        let tracks: Vec<Arc<Track>> = (0..1535)
            .map(|i| {
                track(
                    &format!("t{i}"),
                    &format!("Song number {i} (Extended Club Mix)"),
                    artists[i % artists.len()],
                    &format!("Album {}", i / 12),
                    120 + (i as u64 * 7) % 300,
                )
            })
            .collect();
        let played: Vec<(String, i64)> = (0..1535)
            .step_by(3)
            .map(|i| (format!("t{i}"), NOW - (i as i64 % 200) * DAY))
            .collect();
        let played: Vec<(&str, i64)> = played.iter().map(|(id, at)| (id.as_str(), *at)).collect();
        let s = state(tracks, &played);
        let tracks = &s.main_view().unwrap().tracks;
        let mut c = CollectionView {
            filter_text: "number 1".to_owned(),
            not_heard: true,
            ..CollectionView::default()
        };
        let cutoff = NOW - not_heard_secs(3);
        // The fastest of a few runs: on a loaded machine (load average 35
        // seen) a single run can be descheduled past the bound.
        let fastest = |c: &CollectionView, with_stats: bool| {
            (0..5)
                .map(|_| {
                    let start = Instant::now();
                    let (visible, _) = filter(tracks, &s.last_played, c, cutoff);
                    if with_stats {
                        let _ = stats(tracks, &s.last_played, cutoff);
                    }
                    (start.elapsed(), visible.len())
                })
                .min()
                .unwrap()
        };
        let mut worst = Duration::ZERO;
        for sort in [Sort::PlaylistOrder, Sort::Title, Sort::LastPlayed] {
            c.sort = sort;
            let (took, visible) = fastest(&c, true);
            worst = worst.max(took);
            assert!(visible > 0);
        }
        c.filter_text.clear();
        c.not_heard = false;
        c.sort = Sort::Title;
        let (took, visible) = fastest(&c, false);
        worst = worst.max(took);
        assert_eq!(visible, 1535);
        eprintln!("collection filter, 1,535 tracks: worst {worst:?}");
        let limit = if cfg!(debug_assertions) { 40 } else { 2 };
        assert!(worst < Duration::from_millis(limit), "{worst:?}");
    }

    #[test]
    fn toolbar_fits_the_minimum_window() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        // Fonts load on the first pass.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        // The filter row at the 780 px minimum: the rail, the browse panel,
        // their separator lines and the row's side insets taken off.
        let narrow = 780.0 - theme::RAIL_W - theme::FACET_PANEL_W - 2.0 - 2.0 * theme::SPACE_20;
        let wide = 1200.0 - theme::RAIL_W - theme::FACET_PANEL_W - 2.0 - 2.0 * theme::SPACE_20;
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for (sort, _) in SORTS {
                let c = CollectionView {
                    sort,
                    ..CollectionView::default()
                };
                for count in ["33", "1,528", "15,000"] {
                    let (s, q) = fit_controls(ui, &c, count, narrow);
                    let field = narrow - s.width() - q.width() - 2.0 * theme::SPACE_8;
                    assert!(field >= FIELD_MIN_W, "{sort:?} {count}: field {field}");
                    // The count is the last thing to go.
                    assert!(q.count.is_some(), "{sort:?} {count}");
                    let (s, q) = fit_controls(ui, &c, count, wide);
                    assert!(s.label.is_some() && q.label.is_some(), "{sort:?} {count}");
                }
            }
        })
        .textures_delta
        .clear();
    }
}
