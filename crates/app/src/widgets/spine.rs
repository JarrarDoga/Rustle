//! The spine (D-Range, D-Tokens): a 20 px strip at the right edge of the
//! Collection that stands for the whole main playlist, one 4 px segment per
//! ~10 songs colored by album art.
//!
//! It shows the rows on screen (white box), the playing song (accent line),
//! filter matches (white ticks) and a range (accent box with handles). Click
//! jumps the table (`scroll_to`); drag selects a range (`range`); dragging a
//! handle resizes it. A click outside the range or Escape clears it.
//!
//! Nothing here scales with the playlist per frame: segment colors, filter
//! ticks and the playing position are memoized on the `Arc`s they come from.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use egui::{
    Color32, CornerRadius, CursorIcon, Id, Key, Label, Pos2, Rect, Sense, Stroke, TextWrapMode,
    Tooltip, Ui, vec2,
};
use model::{Track, TrackId};

use crate::state::State;
use crate::theme;
use crate::views::{Out, Sort, View};

/// Gap around the strip inside its column (D-Range: `padding: 8px`).
const PAD: f32 = theme::SPACE_8;
/// How far the viewport and range boxes stick out on each side.
const BOX_OUT: f32 = 4.0;
/// How far filter ticks stick out on each side.
const TICK_OUT: f32 = 3.0;
const TICK_H: f32 = 2.0;
/// The playing line sticks out 8 px left and 4 px right.
const PLAY_OUT_LEFT: f32 = 8.0;
const PLAY_OUT_RIGHT: f32 = 4.0;
const PLAY_H: f32 = 2.0;
/// Range handles: 14 px wide pills centered on the box edges.
const HANDLE_W: f32 = 14.0;
const HANDLE_H: f32 = 4.0;
/// A press this close to a range edge grabs its handle.
const HANDLE_HIT: f32 = theme::SPACE_4;

/// Segment opacities from the boards.
const ALPHA_DEFAULT: f32 = 0.85;
const ALPHA_IN_RANGE: f32 = 0.9;
const ALPHA_OUT_RANGE: f32 = 0.22;
const ALPHA_FILTER: f32 = 0.18;

/// Draws the spine in `ui.max_rect()` (full height of the content region,
/// `theme::SPINE_W` wide or wider).
pub fn show(ui: &mut Ui, state: &State, view: &mut View, _out: &mut Out) {
    let rect = ui.max_rect();
    let p = *theme::pal();
    let strip = strip_rect(rect);
    let column = Rect::from_x_y_ranges(rect.x_range(), strip.top() - PAD..=rect.bottom());
    let id = Id::unique("spine");
    let resp = ui.interact(column, id, Sense::click_and_drag());
    if rect.width() >= theme::SPINE_W + 2.0 * PAD {
        // A column wider than the strip gets the board's left edge.
        ui.painter()
            .vline(rect.left(), column.y_range(), theme::hairline());
    }

    let Some(main) = state.main_view() else {
        return;
    };
    let tracks = &main.tracks;
    let layout = Layout::new(tracks.len(), strip.top(), strip.height());
    if layout.n == 0 {
        return;
    }
    let cv = &view.collection;
    let cache = ui.data_mut(|d| {
        let c = d.get_temp_mut_or_default::<Cache>(id);
        c.refresh(state, tracks, &cv.visible, layout.songs_per);
        c.clone()
    });

    // ---- input: click, drag, handles, Escape
    // A drag keeps its anchor (the end that stays put) and the range so far.
    let drag_id = id.with("drag");
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let anchor = match cv
            .range
            .and_then(|r| Some((r, hit_handle(&layout, r, origin.y)?)))
        {
            Some(((_, to), End::From)) => to,
            Some(((from, _), End::To)) => from,
            None => layout.pos_at(origin.y),
        };
        ui.data_mut(|d| d.insert_temp(drag_id, (anchor, (anchor, anchor))));
    }
    let mut drag = ui.data(|d| d.get_temp::<(usize, (usize, usize))>(drag_id));
    if let Some((anchor, range)) = &mut drag
        && resp.dragged()
        && let Some(at) = pointer
    {
        *range = range_from_drag(*anchor, layout.pos_at(at.y));
        let d = (*anchor, *range);
        ui.data_mut(|m| m.insert_temp(drag_id, d));
    }
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    if drag.is_some() && (escape || !resp.dragged()) {
        ui.data_mut(|d| d.remove::<(usize, (usize, usize))>(drag_id));
        if !escape && resp.drag_stopped() {
            view.collection.range = drag.map(|(_, r)| r);
        }
        drag = None;
    } else if escape && view.collection.range.is_some() && ui.memory(|m| m.focused().is_none()) {
        view.collection.range = None;
    }
    let dragging = drag.map(|(_, r)| r);
    if resp.clicked()
        && let Some(at) = pointer
    {
        let pos = layout.pos_at(at.y);
        // A click outside the range clears it; a click on a handle does not.
        if view.collection.range.is_some_and(|(from, to)| {
            !(from..=to).contains(&pos) && hit_handle(&layout, (from, to), at.y).is_none()
        }) {
            view.collection.range = None;
        }
        view.collection.scroll_to = Some(pos);
    }
    let cv = &view.collection;
    let range = dragging.or(cv.range);
    let on_handle = resp
        .hover_pos()
        .zip(cv.range)
        .is_some_and(|(at, r)| hit_handle(&layout, r, at.y).is_some());
    if dragging.is_some() || on_handle {
        ui.set_cursor_icon(CursorIcon::ResizeVertical);
    }

    // ---- segments
    let filtered = cv.not_heard
        || !cv.chips.is_empty()
        || !cv.filter_text.trim().is_empty()
        || (cv.range.is_none() && cv.visible.len() < layout.n);
    let alpha = |seg: usize| {
        if filtered {
            ALPHA_FILTER
        } else if let Some((from, to)) = range {
            let (first, last) = (from / layout.songs_per, to / layout.songs_per);
            if (first..=last).contains(&seg) {
                ALPHA_IN_RANGE
            } else {
                ALPHA_OUT_RANGE
            }
        } else {
            ALPHA_DEFAULT
        }
    };
    let painter = ui.painter();
    let color_of = |seg: usize| {
        let base = cache.colors[seg].map_or(p.surface_3, |[r, g, b]| Color32::from_rgb(r, g, b));
        over(base, p.bg, alpha(seg))
    };
    // Runs of equal color are one rect.
    let mut seg = 0;
    while seg < layout.segments {
        let color = color_of(seg);
        let mut end = seg + 1;
        while end < layout.segments && color_of(end) == color {
            end += 1;
        }
        let y0 = layout.seg_top(seg);
        let y1 = layout.seg_top(end);
        // The strip has 2 px corners.
        let round = |edge: bool| if edge { theme::RADIUS_ROW } else { 0 };
        let (top, bottom) = (round(seg == 0), round(end == layout.segments));
        let radius = CornerRadius {
            nw: top,
            ne: top,
            sw: bottom,
            se: bottom,
        };
        let r = Rect::from_x_y_ranges(strip.x_range(), y0..=y1);
        painter.rect_filled(r, radius, color);
        seg = end;
    }

    // ---- filter ticks, one per segment with a match
    if filtered {
        for (seg, _) in cache.ticks.iter().enumerate().filter(|(_, m)| **m) {
            let y = layout.seg_top(seg) + (layout.seg_h - TICK_H) / 2.0;
            let r = Rect::from_min_size(
                Pos2::new(strip.left() - TICK_OUT, y),
                vec2(strip.width() + 2.0 * TICK_OUT, TICK_H),
            );
            painter.rect_filled(r, 0, p.text_1);
        }
    }

    // ---- range box and handles
    let accent = theme::accent();
    if let Some((from, to)) = range {
        let (y0, y1) = (layout.y_of(from as f32), layout.y_of((to + 1) as f32));
        let r = Rect::from_x_y_ranges(strip.left() - BOX_OUT..=strip.right() + BOX_OUT, y0..=y1);
        painter.rect_stroke(
            r,
            theme::RADIUS_ROW,
            Stroke::new(theme::STROKE_W, accent),
            egui::StrokeKind::Inside,
        );
        for y in [y0, y1] {
            let h =
                Rect::from_center_size(Pos2::new(strip.center().x, y), vec2(HANDLE_W, HANDLE_H));
            painter.rect_filled(h, theme::RADIUS_ROW, accent);
        }
    }

    // ---- viewport box
    if cv.sort == Sort::PlaylistOrder
        && let Some((first, last)) = cv.on_screen
    {
        let (y0, y1) = viewport_span(&layout, first, last);
        let r = Rect::from_x_y_ranges(strip.left() - BOX_OUT..=strip.right() + BOX_OUT, y0..=y1);
        painter.rect_stroke(
            r,
            theme::RADIUS_ROW,
            Stroke::new(theme::STROKE_W, p.text_1),
            egui::StrokeKind::Inside,
        );
    }

    // ---- playing line
    if let Some(pos) = cache.playing {
        let y = layout.y_of(pos as f32 + 0.5);
        let r = Rect::from_x_y_ranges(
            strip.left() - PLAY_OUT_LEFT..=strip.right() + PLAY_OUT_RIGHT,
            y - PLAY_H / 2.0..=y + PLAY_H / 2.0,
        );
        painter.rect_filled(r, 0, accent);
    }

    if let Some(r) = dragging {
        Tooltip::for_widget(&resp)
            .at_pointer()
            .gap(12.0)
            .show(|ui| ui.add(Label::new(range_label(r)).wrap_mode(TextWrapMode::Extend)));
    }
}

/// The strip inside the spine column: below the page header and filter row,
/// `PAD` from the top and bottom. A column wider than the strip gets the
/// board's 1 px edge and 8 px gap on the left.
fn strip_rect(rect: Rect) -> Rect {
    let top = (rect.top() + theme::PAGE_HEADER_H + theme::FILTER_ROW_H + PAD).min(rect.bottom());
    let bottom = (rect.bottom() - PAD).max(top);
    let left = if rect.width() >= theme::SPINE_W + 2.0 * PAD {
        rect.left() + theme::STROKE_W + PAD
    } else {
        rect.left()
    };
    let width = theme::SPINE_W.min(rect.width());
    Rect::from_x_y_ranges(left..=left + width, top..=bottom)
}

/// "#380 to #520" for a 0-based range.
fn range_label((from, to): (usize, usize)) -> String {
    format!("#{} to #{}", from + 1, to + 1)
}

/// `color` at `alpha` over `bg`, opaque.
fn over(color: Color32, bg: Color32, alpha: f32) -> Color32 {
    let mix = |c: u8, b: u8| (f32::from(b) + (f32::from(c) - f32::from(b)) * alpha).round() as u8;
    Color32::from_rgb(
        mix(color.r(), bg.r()),
        mix(color.g(), bg.g()),
        mix(color.b(), bg.b()),
    )
}

// ------------------------------------------------------------------- layout

/// How the playlist maps onto the strip: `songs_per` songs per segment,
/// `segments` segments of `seg_h` px from `top`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    n: usize,
    top: f32,
    songs_per: usize,
    segments: usize,
    seg_h: f32,
}

impl Layout {
    /// `n` songs in a strip `height` px tall. Segments are nominally
    /// `SPINE_SEGMENT` px for `SPINE_SONGS_PER_SEGMENT` songs; a strip too
    /// short for that shrinks them, and below 1 px puts more songs in each.
    fn new(n: usize, top: f32, height: f32) -> Self {
        let rows = height.floor().max(1.0) as usize;
        let nominal = n.div_ceil(theme::SPINE_SONGS_PER_SEGMENT);
        let songs_per = if nominal <= rows {
            theme::SPINE_SONGS_PER_SEGMENT
        } else {
            n.div_ceil(rows)
        };
        let segments = n.div_ceil(songs_per);
        let seg_h = if segments == 0 {
            theme::SPINE_SEGMENT
        } else {
            theme::SPINE_SEGMENT.min(height / segments as f32)
        };
        Self {
            n,
            top,
            songs_per,
            segments,
            seg_h,
        }
    }

    fn px_per_song(&self) -> f32 {
        self.seg_h / self.songs_per as f32
    }

    /// The y of (fractional) position `pos`.
    fn y_of(&self, pos: f32) -> f32 {
        self.top + pos * self.px_per_song()
    }

    /// The top of segment `seg`.
    fn seg_top(&self, seg: usize) -> f32 {
        self.top + seg as f32 * self.seg_h
    }

    /// The bottom of the last segment.
    fn bottom(&self) -> f32 {
        self.seg_top(self.segments)
    }

    /// The position at `y`, clamped to the playlist. `n` must not be 0.
    fn pos_at(&self, y: f32) -> usize {
        let pos = ((y - self.top) / self.px_per_song()).floor();
        (pos.max(0.0) as usize).min(self.n.saturating_sub(1))
    }
}

/// The range a drag from `anchor` to `at` selects, `from <= to`.
fn range_from_drag(anchor: usize, at: usize) -> (usize, usize) {
    (anchor.min(at), anchor.max(at))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    From,
    To,
}

/// The range handle at `y`, if a press there grabs one. The nearer edge wins
/// when a short range puts both in reach.
fn hit_handle(layout: &Layout, (from, to): (usize, usize), y: f32) -> Option<End> {
    let top = (y - layout.y_of(from as f32)).abs();
    let bottom = (y - layout.y_of((to + 1) as f32)).abs();
    if top.min(bottom) > HANDLE_HIT {
        None
    } else if top <= bottom {
        Some(End::From)
    } else {
        Some(End::To)
    }
}

/// The viewport box's y span for positions `first..=last`: at least
/// `VIEWPORT_BRACKET_MIN` tall, grown about its center, kept on the
/// segments where it fits.
fn viewport_span(layout: &Layout, first: usize, last: usize) -> (f32, f32) {
    let bottom = layout.bottom();
    let (y0, y1) = (
        layout.y_of(first as f32),
        layout.y_of((last.max(first) + 1) as f32),
    );
    let h = (y1 - y0).max(theme::VIEWPORT_BRACKET_MIN);
    let top = ((y0 + y1) / 2.0 - h / 2.0).clamp(layout.top, (bottom - h).max(layout.top));
    (top, top + h)
}

/// One color per segment: the most common color among its songs (ties go
/// to the first), or `None` when none of them is colored yet.
fn group_colors(colors: &[Option<[u8; 3]>], songs_per: usize) -> Vec<Option<[u8; 3]>> {
    colors
        .chunks(songs_per.max(1))
        .map(representative)
        .collect()
}

fn representative(chunk: &[Option<[u8; 3]>]) -> Option<[u8; 3]> {
    let mut best: Option<([u8; 3], usize)> = None;
    for (i, c) in chunk.iter().enumerate() {
        let Some(c) = *c else { continue };
        if chunk[..i].contains(&Some(c)) {
            continue;
        }
        let count = chunk[i..].iter().filter(|x| **x == Some(c)).count();
        if best.is_none_or(|(_, b)| count > b) {
            best = Some((c, count));
        }
    }
    best.map(|(c, _)| c)
}

/// Which segments hold at least one of `visible`.
fn match_segments(visible: &[usize], segments: usize, songs_per: usize) -> Vec<bool> {
    let mut hit = vec![false; segments];
    for &pos in visible {
        if let Some(h) = hit.get_mut(pos / songs_per.max(1)) {
            *h = true;
        }
    }
    hit
}

// -------------------------------------------------------------------- memo

type Tracks = Vec<Arc<Track>>;
type ArtColors = HashMap<TrackId, [u8; 3]>;

/// Per-playlist work, redone only when its inputs change. Keys hold `Weak`
/// pointers, so a freed `Arc`'s address cannot be reused while it is a key
/// and `Arc::make_mut` in the reducer never has to clone for us.
#[derive(Clone, Default)]
struct Cache {
    colors_key: Option<(Weak<Tracks>, Weak<ArtColors>, usize)>,
    colors: Arc<Vec<Option<[u8; 3]>>>,
    ticks_key: Option<(Weak<Vec<usize>>, usize, usize)>,
    ticks: Arc<Vec<bool>>,
    playing_key: Option<(Weak<Tracks>, Option<TrackId>)>,
    playing: Option<usize>,
}

fn same<T>(w: &Weak<T>, a: &Arc<T>) -> bool {
    std::ptr::eq(w.as_ptr(), Arc::as_ptr(a))
}

impl Cache {
    fn refresh(
        &mut self,
        state: &State,
        tracks: &Arc<Tracks>,
        visible: &Arc<Vec<usize>>,
        songs_per: usize,
    ) {
        let art = &state.art_colors;
        if !matches!(&self.colors_key, Some((t, c, s))
            if same(t, tracks) && same(c, art) && *s == songs_per)
        {
            let colors: Vec<Option<[u8; 3]>> = tracks
                .iter()
                .map(|t| art.get(t.id.as_str()).copied())
                .collect();
            self.colors = Arc::new(group_colors(&colors, songs_per));
            self.colors_key = Some((Arc::downgrade(tracks), Arc::downgrade(art), songs_per));
        }
        let segments = tracks.len().div_ceil(songs_per.max(1));
        if !matches!(&self.ticks_key, Some((v, s, n))
            if same(v, visible) && *s == songs_per && *n == segments)
        {
            self.ticks = Arc::new(match_segments(visible, segments, songs_per));
            self.ticks_key = Some((Arc::downgrade(visible), songs_per, segments));
        }
        let current = state.current_track().map(|t| t.id.clone());
        if !matches!(&self.playing_key, Some((t, id)) if same(t, tracks) && *id == current) {
            self.playing = current
                .as_deref()
                .and_then(|id| tracks.iter().position(|t| t.id == id));
            self.playing_key = Some((Arc::downgrade(tracks), current));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_playlist_fits_600_px_at_nominal_grouping() {
        let l = Layout::new(1535, 100.0, 600.0);
        assert_eq!(l.songs_per, 10);
        assert_eq!(l.segments, 154);
        assert!(l.seg_h >= 1.0 && l.seg_h < theme::SPINE_SEGMENT);
        assert!(l.bottom() <= 700.0 + 1e-3);
        assert_eq!(l.pos_at(100.0), 0);
        assert_eq!(l.pos_at(699.9), 1534);
        assert_eq!(l.pos_at(10_000.0), 1534, "below the strip clamps");
        assert_eq!(l.pos_at(-50.0), 0, "above the strip clamps");
    }

    #[test]
    fn a_small_playlist_keeps_4_px_segments() {
        let l = Layout::new(40, 0.0, 600.0);
        assert_eq!((l.songs_per, l.segments), (10, 4));
        assert_eq!(l.seg_h, 4.0);
        assert_eq!(l.bottom(), 16.0);
        assert_eq!(l.pos_at(0.0), 0);
        assert_eq!(l.pos_at(3.9), 9);
        assert_eq!(l.pos_at(4.0), 10);
        assert_eq!(l.pos_at(300.0), 39);
    }

    #[test]
    fn a_short_strip_merges_songs_and_keeps_1_px() {
        let l = Layout::new(1535, 0.0, 100.0);
        assert!(l.segments <= 100);
        assert!(l.seg_h >= 1.0);
        assert!(l.songs_per > 10);
        assert!(l.songs_per * l.segments >= 1535);
        let tiny = Layout::new(1535, 0.0, 0.0);
        assert_eq!(tiny.segments, 1);
    }

    #[test]
    fn y_and_position_round_trip() {
        for (n, h) in [(1535, 600.0), (40, 600.0), (1535, 100.0), (7, 600.0)] {
            let l = Layout::new(n, 50.0, h);
            for pos in [0, n / 3, n / 2, n - 1] {
                let y = l.y_of(pos as f32 + 0.5);
                assert_eq!(l.pos_at(y), pos, "n={n} h={h} pos={pos}");
            }
        }
    }

    #[test]
    fn a_drag_selects_from_low_to_high() {
        assert_eq!(range_from_drag(519, 379), (379, 519));
        assert_eq!(range_from_drag(379, 519), (379, 519));
        assert_eq!(range_from_drag(5, 5), (5, 5));
        assert_eq!(range_label((379, 519)), "#380 to #520");
    }

    #[test]
    fn handles_are_hit_near_the_range_edges() {
        let l = Layout::new(1535, 0.0, 616.0);
        let r = (379, 519);
        let top = l.y_of(379.0);
        let bottom = l.y_of(520.0);
        assert_eq!(hit_handle(&l, r, top), Some(End::From));
        assert_eq!(hit_handle(&l, r, top - 3.0), Some(End::From));
        assert_eq!(hit_handle(&l, r, bottom + 3.0), Some(End::To));
        assert_eq!(hit_handle(&l, r, (top + bottom) / 2.0), None);
        assert_eq!(hit_handle(&l, r, bottom + 20.0), None);
        // A one-song range: the nearer edge wins.
        let one = (100, 100);
        let (t, b) = (l.y_of(100.0), l.y_of(101.0));
        assert_eq!(hit_handle(&l, one, t - 1.0), Some(End::From));
        assert_eq!(hit_handle(&l, one, b + 1.0), Some(End::To));
    }

    #[test]
    fn the_viewport_box_is_at_least_the_bracket_min() {
        let l = Layout::new(1535, 0.0, 616.0);
        let (y0, y1) = viewport_span(&l, 380, 397);
        assert!((y1 - y0 - theme::VIEWPORT_BRACKET_MIN).abs() < 1e-3);
        let mid = l.y_of((380.0 + 398.0) / 2.0);
        assert!(
            ((y0 + y1) / 2.0 - mid).abs() < 1e-3,
            "grown about its center"
        );
        // At the very top it stays inside the strip.
        let (y0, _) = viewport_span(&l, 0, 3);
        assert_eq!(y0, 0.0);
        // A tall span keeps its own height.
        let (y0, y1) = viewport_span(&l, 0, 199);
        assert!((y1 - y0 - l.y_of(200.0)).abs() < 1e-3);
    }

    #[test]
    fn segment_color_is_the_most_common_then_the_first() {
        let (a, b, c) = (Some([1, 1, 1]), Some([2, 2, 2]), Some([3, 3, 3]));
        assert_eq!(representative(&[None, a, b, b, a, b]), b);
        assert_eq!(representative(&[None, c, a, a, c]), c, "tie goes first");
        assert_eq!(representative(&[None, None]), None);
        let groups = group_colors(&[a, a, None, b, None, None, None], 3);
        assert_eq!(groups, vec![a, b, None]);
    }

    #[test]
    fn segment_colors_cover_every_song_of_a_big_playlist() {
        let colors: Vec<Option<[u8; 3]>> = (0..1535)
            .map(|i| (i % 7 != 0).then_some([(i / 10) as u8, 0, 0]))
            .collect();
        let l = Layout::new(1535, 0.0, 600.0);
        let groups = group_colors(&colors, l.songs_per);
        assert_eq!(groups.len(), l.segments);
        assert_eq!(groups[41], Some([41, 0, 0]));
    }

    #[test]
    fn ticks_mark_segments_with_matches() {
        let hit = match_segments(&[0, 9, 25, 1534, 99_999], 154, 10);
        assert_eq!(hit.len(), 154);
        assert!(hit[0] && hit[2] && hit[153]);
        assert_eq!(hit.iter().filter(|h| **h).count(), 3);
    }

    #[test]
    fn dimming_blends_toward_the_background() {
        let (c, bg) = (Color32::from_rgb(200, 100, 0), Color32::from_rgb(0, 0, 100));
        assert_eq!(over(c, bg, 1.0), c);
        assert_eq!(over(c, bg, 0.0), bg);
        assert_eq!(over(c, bg, 0.5), Color32::from_rgb(100, 50, 50));
    }

    #[test]
    fn the_strip_starts_below_the_header_and_filter_row() {
        let narrow = Rect::from_min_size(Pos2::new(1260.0, 0.0), vec2(20.0, 744.0));
        let s = strip_rect(narrow);
        assert_eq!(s.left(), 1260.0);
        assert_eq!(s.width(), theme::SPINE_W);
        assert_eq!(s.top(), theme::PAGE_HEADER_H + theme::FILTER_ROW_H + PAD);
        assert_eq!(s.bottom(), 744.0 - PAD);
        // The board's 37 px column: 1 px edge, 8 px gap, 20 px strip.
        let wide = Rect::from_min_size(Pos2::new(1243.0, 0.0), vec2(37.0, 744.0));
        assert_eq!(strip_rect(wide).left(), 1252.0);
        assert_eq!(strip_rect(wide).height(), 616.0);
    }
}
