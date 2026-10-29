//! The one song table every list uses (D-Tokens track rows, D-Collection,
//! D-Search, D-Album). Rows are virtualized: only the rows inside the
//! viewport are laid out and painted. Dragging a row sideways queues its
//! track (right) or takes one queued copy out (left); see [`swipe`].
//!
//! Geometry follows the boards' CSS: the table sits 12 px in from the left
//! and 8 px in from the right of the region it is given, rows have 8 px
//! padding and 12 px column gaps, and rows are exactly [`theme::ROW_H`]
//! apart so the spine's viewport box lines up.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::epaint::RectShape;
use egui::{
    Align2, Color32, CornerRadius, Key, Painter, Pos2, Rect, Response, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, pos2, vec2,
};
use model::{Track, TrackId};

use crate::icons;
use crate::state::{Command, State};
use crate::theme;
use crate::thumbs;
use crate::views::{Out, View, artists, cmd, open_album, open_artist};
use crate::widgets::{fmt_time, seed, text_line};

/// A table column. Screens pick a set; see the constants below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The row's number: playlist position or album track number. On hover
    /// it becomes a play button; on the playing row, a speaker.
    Index,
    /// Art (24 px) and title. The album set (no [`Column::Album`]) leaves
    /// the art out, since every row shares it.
    Title,
    Artist,
    Album,
    /// From `State::last_played` ("3 mo", "Now" while playing).
    LastPlayed,
    /// Check plus `#412` when the song is in the main playlist, else an Add
    /// button on hover that sends `Command::AddToMain`.
    InEverything,
    /// Like on hover (always shown when liked), then the duration.
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
    /// Scroll so the row showing `tracks[i]` is about a third from the top.
    /// Applied on the frame it is passed; pass it once.
    pub scroll_to: Option<usize>,
}

/// What the table reports after drawing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Shown {
    /// `tracks` indices of the first and last rows on screen (partly
    /// visible rows count).
    pub on_screen: Option<(usize, usize)>,
}

// ------------------------------------------------------------------ layout

/// Space between the given region and the table, left and right.
const INSET_L: f32 = 12.0;
const INSET_R: f32 = 8.0;
/// Row padding, left and right.
const PAD: f32 = 8.0;
const GAP: f32 = 12.0;
const INDEX_W: f32 = 44.0;
const LAST_PLAYED_W: f32 = 76.0;
const IN_EVERYTHING_W: f32 = 104.0;
/// The album page's In Everything column is wider (D-Album).
const IN_EVERYTHING_ALBUM_W: f32 = 120.0;
const LIKE_W: f32 = 24.0;
const TIME_W: f32 = 40.0;
/// Below this the Album column goes, then the Artist column.
const TITLE_MIN: f32 = 160.0;
/// Space between the art and the title.
const ART_GAP: f32 = 10.0;
const ICON: f32 = 14.0;
/// Space between the check and `#412`.
const CHECK_GAP: f32 = 6.0;
const ADD_H: f32 = 22.0;
const SKELETON_BAR_H: f32 = 8.0;

/// A cell of the row grid: a column, or the like button between the last
/// data column and Time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Col(Column),
    Like,
}

/// One cell's horizontal span, relative to the row's left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    slot: Slot,
    x: f32,
    w: f32,
}

impl Cell {
    fn left(&self, row: Rect) -> f32 {
        row.left() + self.x
    }

    fn right(&self, row: Rect) -> f32 {
        row.left() + self.x + self.w
    }

    fn rect(&self, row: Rect) -> Rect {
        Rect::from_x_y_ranges(self.left(row)..=self.right(row), row.y_range())
    }
}

/// The album page's set: no Album column, a wider title.
fn is_album_set(columns: &[Column]) -> bool {
    !columns.contains(&Album)
}

/// Fixed width, or `None` for a column that shares the free space.
fn fixed_width(slot: Slot, album_set: bool) -> Option<f32> {
    match slot {
        Slot::Col(Index) => Some(INDEX_W),
        Slot::Col(LastPlayed) => Some(LAST_PLAYED_W),
        Slot::Col(InEverything) if album_set => Some(IN_EVERYTHING_ALBUM_W),
        Slot::Col(InEverything) => Some(IN_EVERYTHING_W),
        Slot::Col(Time) => Some(TIME_W),
        Slot::Like => Some(LIKE_W),
        Slot::Col(Title | Artist | Album) => None,
    }
}

/// Share of the free space (CSS `fr`).
fn fraction(slot: Slot, album_set: bool) -> f32 {
    match slot {
        Slot::Col(Title) if album_set => 4.0,
        Slot::Col(Title) => 3.0,
        Slot::Col(Artist | Album) => 2.0,
        _ => 0.0,
    }
}

/// Lays `columns` out across a row `width` px wide (the row itself, not
/// the region around it). Album goes first when the title would get too
/// narrow, then Artist.
fn layout(columns: &[Column], width: f32) -> Vec<Cell> {
    let album_set = is_album_set(columns);
    let mut slots: Vec<Slot> = Vec::with_capacity(columns.len() + 1);
    for &c in columns {
        if c == Time {
            slots.push(Slot::Like);
        }
        slots.push(Slot::Col(c));
    }
    let free = |slots: &[Slot]| {
        let fixed: f32 = slots
            .iter()
            .filter_map(|&s| fixed_width(s, album_set))
            .sum();
        let gaps = GAP * slots.len().saturating_sub(1) as f32;
        (width - 2.0 * PAD - fixed - gaps).max(0.0)
    };
    let title_width = |slots: &[Slot]| {
        let total: f32 = slots.iter().map(|&s| fraction(s, album_set)).sum();
        if total > 0.0 {
            free(slots) * fraction(Slot::Col(Title), album_set) / total
        } else {
            0.0
        }
    };
    for drop in [Album, Artist] {
        if !slots.contains(&Slot::Col(Title)) || title_width(&slots) >= TITLE_MIN {
            break;
        }
        slots.retain(|&s| s != Slot::Col(drop));
    }
    let free = free(&slots);
    let total: f32 = slots.iter().map(|&s| fraction(s, album_set)).sum();
    let mut x = PAD;
    slots
        .into_iter()
        .map(|slot| {
            let w = fixed_width(slot, album_set).unwrap_or_else(|| {
                if total > 0.0 {
                    free * fraction(slot, album_set) / total
                } else {
                    0.0
                }
            });
            let cell = Cell { slot, x, w };
            x += w + GAP;
            cell
        })
        .collect()
}

/// Rows `first..=last` that a viewport from `top` to `bottom` (content
/// coordinates) touches, for `n` rows `row_h` apart.
fn rows_on_screen(top: f32, bottom: f32, row_h: f32, n: usize) -> Option<(usize, usize)> {
    let top = top.max(0.0);
    let bottom = bottom.min(row_h * n as f32);
    if n == 0 || bottom <= top {
        return None;
    }
    let first = (top / row_h).floor() as usize;
    let last = ((bottom / row_h).ceil() as usize).saturating_sub(1);
    Some((first.min(n - 1), last.clamp(first.min(n - 1), n - 1)))
}

/// Scroll offset that puts row `k` about a third of the way down a
/// viewport `height` px tall.
fn scroll_offset(k: usize, height: f32, row_h: f32) -> f32 {
    (k as f32 * row_h - height / 3.0).max(0.0)
}

/// "5 d", "3 wk", "8 mo", "1 yr" for a play `secs` seconds ago.
fn relative_time(secs: i64) -> String {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    let s = secs.max(0);
    let days = s / DAY;
    if s < HOUR {
        format!("{} min", (s / MIN).max(1))
    } else if s < DAY {
        format!("{} h", s / HOUR)
    } else if days < 7 {
        format!("{days} d")
    } else if days < 30 {
        format!("{} wk", days / 7)
    } else if days < 365 {
        format!("{} mo", (days / 30).min(11))
    } else {
        format!("{} yr", days / 365)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// ----------------------------------------------------------------- lookups

/// Liked ids and main-playlist positions, rebuilt only when the lists they
/// come from change. Each holds its source `Arc`, so a new list can never
/// reuse the old one's address and hit a stale entry.
#[derive(Default)]
struct Lookups {
    liked: Option<(TrackList, Rc<HashSet<TrackId>>)>,
    main: Option<(TrackList, Rc<HashMap<TrackId, usize>>)>,
}

type TrackList = Arc<Vec<Arc<Track>>>;

thread_local! {
    static LOOKUPS: RefCell<Lookups> = RefCell::default();
}

fn liked_ids(state: &State) -> Rc<HashSet<TrackId>> {
    let Some(list) = state.liked.ready() else {
        return Rc::default();
    };
    LOOKUPS.with_borrow_mut(|l| match &l.liked {
        Some((src, set)) if Arc::ptr_eq(src, list) => Rc::clone(set),
        _ => {
            let set: Rc<HashSet<TrackId>> = Rc::new(list.iter().map(|t| t.id.clone()).collect());
            l.liked = Some((Arc::clone(list), Rc::clone(&set)));
            set
        }
    })
}

/// `None` when no main playlist is loaded.
fn main_positions(state: &State) -> Option<Rc<HashMap<TrackId, usize>>> {
    let list = &state.main_view()?.tracks;
    Some(LOOKUPS.with_borrow_mut(|l| match &l.main {
        Some((src, map)) if Arc::ptr_eq(src, list) => Rc::clone(map),
        _ => {
            let mut map = HashMap::with_capacity(list.len());
            for (p, t) in list.iter().enumerate() {
                // The first copy wins, like `State::main_position`.
                map.entry(t.id.clone()).or_insert(p);
            }
            let map = Rc::new(map);
            l.main = Some((Arc::clone(list), Rc::clone(&map)));
            map
        }
    }))
}

// -------------------------------------------------------------------- show

/// What every row needs to know, read once per [`show`].
struct RowCtx<'a> {
    state: &'a State,
    cells: &'a [Cell],
    art: bool,
    now: i64,
    current: Option<&'a str>,
    liked: &'a HashSet<TrackId>,
    /// Set when the table has an In Everything column and a main playlist
    /// is loaded.
    main: Option<&'a HashMap<TrackId, usize>>,
}

/// Draws the header and the rows. Click selects; double-click (or Enter on
/// the selected row) plays the shown rows from that row; right-click opens
/// the song menu.
pub fn show(ui: &mut Ui, state: &State, view: &mut View, table: Table<'_>, out: &mut Out) -> Shown {
    let Table {
        salt,
        tracks,
        rows,
        columns,
        scroll_to,
    } = table;
    let list_id = ui.scope_id().with(salt);
    let n = rows.map_or(tracks.len(), <[usize]>::len);
    let at = |k: usize| rows.map_or(k, |r| r[k]);
    let display_pos = |i: usize| match rows {
        Some(r) => r.iter().position(|&x| x == i),
        None => (i < n).then_some(i),
    };
    let play_from = |out: &mut Out, k: usize| {
        let list: Vec<Arc<Track>> = (0..n).map(|k| Arc::clone(&tracks[at(k)])).collect();
        cmd(
            out,
            Command::PlayTracks {
                tracks: Arc::new(list),
                start: k,
                seed: seed(),
            },
        );
    };

    let full = ui.available_rect_before_wrap();
    let row_w = (full.width() - INSET_L - INSET_R).max(0.0);
    let cells = layout(columns, row_w);
    let p = theme::pal();
    let mut shown = Shown::default();

    ui.scope_builder(UiBuilder::new().max_rect(full), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (head, _) =
            ui.allocate_exact_size(vec2(full.width(), theme::TABLE_HEADER_H), Sense::hover());
        let head = Rect::from_min_size(
            pos2(head.left() + INSET_L, head.top()),
            vec2(row_w, theme::TABLE_HEADER_H),
        );
        header(ui.painter(), head, &cells);
        ui.add_space(theme::SPACE_4);

        if n == 0 {
            ui.add_space(theme::SPACE_12);
            ui.horizontal(|ui| {
                ui.add_space(INSET_L + PAD);
                ui.label(egui::RichText::new("No songs here.").color(p.text_3));
            });
            return;
        }

        if let Some((id, i)) = view.selected
            && id == list_id
            && ui.input(|inp| inp.key_pressed(Key::Enter))
            && ui.memory(|m| m.focused().is_none())
            && let Some(k) = display_pos(i)
        {
            play_from(out, k);
        }

        let liked = liked_ids(state);
        let main = if columns.contains(&InEverything) {
            main_positions(state)
        } else {
            None
        };
        let current = state.current_track().map(|t| t.id.as_str());
        let ctx = RowCtx {
            state,
            cells: &cells,
            art: !is_album_set(columns),
            now: unix_now(),
            current,
            liked: &liked,
            main: main.as_deref(),
        };

        let mut area = ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false]);
        if let Some(k) = scroll_to.and_then(display_pos) {
            area =
                area.vertical_scroll_offset(scroll_offset(k, ui.available_height(), theme::ROW_H));
        }
        area.show_viewport(ui, |ui, viewport| {
            ui.set_height(theme::ROW_H * n as f32);
            let Some((first, last)) =
                rows_on_screen(viewport.min.y, viewport.max.y, theme::ROW_H, n)
            else {
                return;
            };
            shown.on_screen = Some((at(first), at(last)));
            let top = ui.max_rect().top();
            let left = ui.max_rect().left() + INSET_L;
            for k in first..=last {
                let i = at(k);
                let track = &tracks[i];
                let rect = Rect::from_min_size(
                    pos2(left, top + k as f32 * theme::ROW_H),
                    vec2(row_w, theme::ROW_H),
                );
                let selected = view.selected == Some((list_id, i));
                let resp = row(
                    ui,
                    &ctx,
                    list_id.with(("row", i)),
                    rect,
                    i,
                    track,
                    selected,
                    out,
                );
                if resp.clicked() || resp.secondary_clicked() {
                    view.selected = Some((list_id, i));
                }
                if resp.double_clicked() {
                    play_from(out, k);
                }
                resp.context_menu(|ui| menu(ui, state, track, out));
            }
        });
    });
    shown
}

fn header(painter: &Painter, head: Rect, cells: &[Cell]) {
    let p = theme::pal();
    let cy = head.center().y;
    for cell in cells {
        let (text, right) = match cell.slot {
            Slot::Col(Index) => ("#", true),
            Slot::Col(Title) => ("Title", false),
            Slot::Col(Artist) => ("Artist", false),
            Slot::Col(Album) => ("Album", false),
            Slot::Col(LastPlayed) => ("Last played", false),
            Slot::Col(InEverything) => ("In Everything", false),
            Slot::Col(Time) => ("Time", true),
            Slot::Like => continue,
        };
        let (x, align) = if right {
            (cell.right(head), Align2::RIGHT_CENTER)
        } else {
            (cell.left(head), Align2::LEFT_CENTER)
        };
        painter.text(pos2(x, cy), align, text, theme::caption(), p.text_3);
    }
    painter.hline(
        head.x_range(),
        head.bottom() - theme::STROKE_W / 2.0,
        theme::hairline(),
    );
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    ctx: &RowCtx<'_>,
    id: egui::Id,
    rect: Rect,
    index: usize,
    track: &Arc<Track>,
    selected: bool,
    out: &mut Out,
) -> Response {
    let p = theme::pal();
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    // Swipe to queue: while dragged sideways the row slides inside its slot.
    let slot = rect;
    let swipe = swipe(ui, &resp, id, ctx.state, track, out);
    let rect = swipe.map_or(rect, |s| rect.translate(vec2(s.offset, 0.0)));
    // Half-open: on the line between two rows only the lower one hovers.
    let hover = resp.contains_pointer()
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|p| p.y < rect.bottom());
    let hover = hover || swipe.is_some();
    let is_current = ctx.current == Some(track.id.as_str());
    let liked = ctx.liked.contains(&track.id);
    let painter = ui.painter().clone();
    let hint = swipe.map(|s| SwipeHint::new(&painter, slot, s));
    let painter = match &hint {
        Some(h) => painter.with_clip_rect(painter.clip_rect().intersect(h.row_clip(slot))),
        None => painter,
    };
    let cy = rect.center().y;

    // The heart and Add sit on top of the row, so they take their own
    // clicks. Interact first, paint after.
    let mut like = None;
    let mut add = None;
    for cell in ctx.cells {
        match cell.slot {
            Slot::Like if hover || liked => {
                let r = Rect::from_center_size(
                    pos2(cell.rect(rect).center().x, cy),
                    vec2(LIKE_W, LIKE_W),
                );
                like = Some(ui.interact(r, id.with("like"), Sense::click()));
            }
            Slot::Col(InEverything) if hover => {
                if let Some(main) = ctx.main
                    && !main.contains_key(&track.id)
                {
                    let text = painter.layout_no_wrap(
                        "Add".to_owned(),
                        theme::secondary(),
                        Color32::PLACEHOLDER,
                    );
                    let w = 6.0 + 12.0 + theme::SPACE_4 + text.size().x + theme::SPACE_8;
                    let r = Rect::from_min_size(
                        pos2(cell.left(rect), cy - ADD_H / 2.0),
                        vec2(w.min(cell.w), ADD_H),
                    );
                    add = Some((ui.interact(r, id.with("add"), Sense::click()), text));
                }
            }
            _ => {}
        }
    }

    let radius = CornerRadius::same(theme::RADIUS_ROW);
    if selected {
        painter.rect_filled(rect, radius, p.surface_3);
    } else if hover {
        painter.rect_filled(rect, radius, p.surface_2);
    }

    for cell in ctx.cells {
        let left = cell.left(rect);
        let right = cell.right(rect);
        match cell.slot {
            Slot::Col(Index) => {
                let (text, font, color) = if hover {
                    (icons::PLAY.to_owned(), theme::icon_fill(12.0), p.text_1)
                } else if is_current {
                    (
                        icons::SPEAKER_HIGH.to_owned(),
                        theme::icon(ICON),
                        theme::accent(),
                    )
                } else {
                    ((index + 1).to_string(), theme::secondary(), p.text_3)
                };
                painter.text(pos2(right, cy), Align2::RIGHT_CENTER, text, font, color);
            }
            Slot::Col(Title) => {
                let mut x = left;
                if ctx.art {
                    let art = Rect::from_min_size(
                        pos2(x, cy - theme::ROW_ART / 2.0),
                        vec2(theme::ROW_ART, theme::ROW_ART),
                    );
                    row_art(&painter, art, track);
                    x = art.right() + ART_GAP;
                }
                let (font, color) = if is_current {
                    (theme::row_strong(), theme::accent())
                } else {
                    (theme::row(), p.text_1)
                };
                text_line(&painter, pos2(x, cy), &track.title, font, color, right - x);
            }
            Slot::Col(Artist) => text_line(
                &painter,
                pos2(left, cy),
                &artists(track),
                theme::secondary(),
                p.text_2,
                cell.w,
            ),
            Slot::Col(Album) => {
                if let Some(album) = &track.album {
                    text_line(
                        &painter,
                        pos2(left, cy),
                        &album.name,
                        theme::secondary(),
                        p.text_2,
                        cell.w,
                    );
                }
            }
            Slot::Col(LastPlayed) => {
                let (text, color) = if is_current {
                    ("Now".to_owned(), theme::accent())
                } else if let Some(&t) = ctx.state.last_played.get(&track.id) {
                    (relative_time(ctx.now - t), p.text_3)
                } else {
                    (String::new(), p.text_3)
                };
                text_line(
                    &painter,
                    pos2(left, cy),
                    &text,
                    theme::secondary(),
                    color,
                    cell.w,
                );
            }
            Slot::Col(InEverything) => {
                if let Some(&pos) = ctx.main.and_then(|m| m.get(&track.id)) {
                    painter.text(
                        pos2(left, cy),
                        Align2::LEFT_CENTER,
                        icons::CHECK,
                        theme::icon(ICON),
                        theme::accent(),
                    );
                    let x = left + ICON + CHECK_GAP;
                    text_line(
                        &painter,
                        pos2(x, cy),
                        &format!("#{}", pos + 1),
                        theme::secondary(),
                        p.text_2,
                        right - x,
                    );
                } else if let Some((resp, text)) = &add {
                    let (fill, fg) = if resp.hovered() {
                        (p.surface_3, p.text_1)
                    } else {
                        (Color32::TRANSPARENT, p.text_2)
                    };
                    let r = resp.rect;
                    painter.add(RectShape::new(
                        r,
                        CornerRadius::same(theme::RADIUS_CONTROL),
                        fill,
                        Stroke::new(theme::STROKE_W, p.stroke_2),
                        StrokeKind::Inside,
                    ));
                    painter.text(
                        pos2(r.left() + 6.0, cy),
                        Align2::LEFT_CENTER,
                        icons::PLUS,
                        theme::icon(12.0),
                        fg,
                    );
                    painter.galley(
                        pos2(
                            r.left() + 6.0 + 12.0 + theme::SPACE_4,
                            cy - text.size().y / 2.0,
                        ),
                        Arc::clone(text),
                        fg,
                    );
                }
            }
            Slot::Like => {
                if let Some(resp) = &like {
                    let (font, color) = if liked {
                        (theme::icon_fill(ICON), theme::accent())
                    } else if resp.hovered() {
                        (theme::icon(ICON), p.text_1)
                    } else {
                        (theme::icon(ICON), p.text_2)
                    };
                    painter.text(
                        resp.rect.center(),
                        Align2::CENTER_CENTER,
                        icons::HEART,
                        font,
                        color,
                    );
                }
            }
            Slot::Col(Time) => {
                if let Some(d) = track.duration {
                    painter.text(
                        pos2(right, cy),
                        Align2::RIGHT_CENTER,
                        fmt_time(d),
                        theme::secondary(),
                        p.text_3,
                    );
                }
            }
        }
    }

    if like.as_ref().is_some_and(Response::clicked) {
        let id = track.id.clone();
        cmd(
            out,
            if liked {
                Command::Unlike(id)
            } else {
                Command::Like(id)
            },
        );
    }
    if add.as_ref().is_some_and(|(r, _)| r.clicked()) {
        cmd(out, Command::AddToMain(vec![Arc::clone(track)]));
    }
    if let Some(h) = hint {
        h.paint(ui.painter());
    }
    resp
}

// ------------------------------------------------------------------- swipe

/// Horizontal travel at which a swipe commits on release.
const SWIPE_COMMIT: f32 = 40.0;
/// Vertical travel past which a drag stops being a swipe until release.
const SWIPE_SLOP_Y: f32 = 16.0;
/// How far the row slides at most.
const SWIPE_MAX: f32 = 96.0;

/// Right adds the track to the queue, left removes its first upcoming copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwipeDir {
    Add,
    Remove,
}

/// A swipe in progress, as the pointer has moved so far.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SwipeMove {
    dir: SwipeDir,
    /// How far the row slides, signed.
    offset: f32,
    /// Releasing now commits.
    armed: bool,
}

/// One row drag, kept in egui's temp memory between frames.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SwipeDrag {
    origin: Pos2,
    /// The drag went past [`SWIPE_SLOP_Y`]; it stays dead until release.
    vertical: bool,
}

impl SwipeDrag {
    fn new(origin: Pos2) -> Self {
        Self {
            origin,
            vertical: false,
        }
    }

    /// Feeds the pointer position. `None` while the drag is not a sideways
    /// swipe: not moved, more vertical than horizontal, or past the
    /// vertical slop at any point.
    fn update(&mut self, pos: Pos2) -> Option<SwipeMove> {
        let d = pos - self.origin;
        self.vertical |= d.y.abs() > SWIPE_SLOP_Y;
        if self.vertical || d.x.abs() <= d.y.abs() {
            return None;
        }
        Some(SwipeMove {
            dir: if d.x > 0.0 {
                SwipeDir::Add
            } else {
                SwipeDir::Remove
            },
            offset: d.x.clamp(-SWIPE_MAX, SWIPE_MAX),
            armed: d.x.abs() >= SWIPE_COMMIT,
        })
    }
}

/// What the row draws while swiped.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SwipeShown {
    dir: SwipeDir,
    offset: f32,
    armed: bool,
    /// False for a left swipe on a track with no upcoming copy: it would do
    /// nothing, so the hint is dimmed.
    live: bool,
}

/// Reads a primary-button drag on a row as a swipe. Releasing an armed swipe
/// sends `AddToQueue` or `RemoveFromQueue`; releasing earlier cancels.
/// Returns what to draw while the drag lasts. Needs no repaint of its own:
/// pointer moves repaint (so repaints happen only while a drag is in
/// progress), and the release frame draws the row back in place.
fn swipe(
    ui: &Ui,
    resp: &Response,
    id: egui::Id,
    state: &State,
    track: &Arc<Track>,
    out: &mut Out,
) -> Option<SwipeShown> {
    let key = id.with("swipe");
    let primary = egui::PointerButton::Primary;
    if resp.drag_started_by(primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(key, SwipeDrag::new(origin)));
    }
    let live = |dir| dir == SwipeDir::Add || state.queue.upcoming_position(&track.id).is_some();
    if resp.dragged_by(primary) {
        let pos = ui.input(|i| i.pointer.latest_pos())?;
        let mut drag = ui.data(|d| d.get_temp::<SwipeDrag>(key))?;
        let m = drag.update(pos);
        ui.data_mut(|d| d.insert_temp(key, drag));
        // The row takes drags, so on a touch screen (where the scroll area
        // scrolls by dragging) a vertical one scrolls the list instead.
        if drag.vertical && ui.input(|i| i.has_touch_screen()) {
            let dy = ui.input(|i| i.pointer.delta().y);
            ui.scroll_with_delta_animation(vec2(0.0, dy), egui::style::ScrollAnimation::none());
        }
        let m = m?;
        return Some(SwipeShown {
            dir: m.dir,
            offset: m.offset,
            armed: m.armed,
            live: live(m.dir),
        });
    }
    if resp.drag_stopped()
        && let Some(mut drag) = ui.data_mut(|d| {
            let drag = d.get_temp::<SwipeDrag>(key);
            d.remove::<SwipeDrag>(key);
            drag
        })
        && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
        && let Some(m) = drag.update(pos)
        && m.armed
        && live(m.dir)
    {
        cmd(
            out,
            match m.dir {
                SwipeDir::Add => Command::AddToQueue(Arc::clone(track)),
                SwipeDir::Remove => Command::RemoveFromQueue(track.id.clone()),
            },
        );
    }
    None
}

/// "+ Queue" or "- Queue" at the edge the row is dragged toward: accent once
/// releasing would commit, dimmed when it would do nothing.
struct SwipeHint {
    shown: SwipeShown,
    pill: Rect,
    galley: Arc<egui::Galley>,
}

impl SwipeHint {
    fn new(painter: &Painter, slot: Rect, shown: SwipeShown) -> Self {
        let text = match shown.dir {
            SwipeDir::Add => "+ Queue",
            SwipeDir::Remove => "- Queue",
        };
        let galley =
            painter.layout_no_wrap(text.to_owned(), theme::secondary(), Color32::PLACEHOLDER);
        let size = vec2(galley.size().x + 2.0 * theme::SPACE_8, ADD_H);
        let x = match shown.dir {
            SwipeDir::Add => slot.right() - PAD - size.x,
            SwipeDir::Remove => slot.left() + PAD,
        };
        let pill = Rect::from_min_size(pos2(x, slot.center().y - size.y / 2.0), size);
        Self {
            shown,
            pill,
            galley,
        }
    }

    /// Where the sliding row paints: its slot, short of the pill, so the
    /// pill sits on a clear strip at the edge.
    fn row_clip(&self, slot: Rect) -> Rect {
        let mut clip = slot;
        match self.shown.dir {
            SwipeDir::Add => clip.max.x = self.pill.left() - theme::SPACE_4,
            SwipeDir::Remove => clip.min.x = self.pill.right() + theme::SPACE_4,
        }
        clip
    }

    fn paint(self, painter: &Painter) {
        let p = theme::pal();
        let s = self.shown;
        let (fill, fg) = if !s.live {
            (p.surface_2, p.text_3)
        } else if s.armed {
            (theme::accent(), p.on_accent)
        } else {
            (p.surface_3, p.text_1)
        };
        painter.rect_filled(self.pill, CornerRadius::same(theme::RADIUS_CONTROL), fill);
        let at = self.pill.center() - self.galley.size() / 2.0;
        painter.galley(at, self.galley, fg);
    }
}

/// Row art: the thumbnail once loaded, a flat tile until then.
fn row_art(painter: &Painter, rect: Rect, track: &Track) {
    let radius = CornerRadius::same(theme::RADIUS_ROW);
    match thumbs::get(painter.ctx(), &track.thumbnails, rect.height()) {
        Some(tex) => {
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            painter.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, uv));
        }
        None => {
            painter.rect_filled(rect, radius, theme::art_color(&track.id));
        }
    }
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
    ui.separator();
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
    if state.main_view().is_some() {
        if state.main_position(&track.id).is_some() {
            if ui.button("Remove from Everything").clicked() {
                cmd(out, Command::RemoveFromMain(vec![track.id.clone()]));
                ui.close();
            }
        } else if ui.button("Add to Everything").clicked() {
            cmd(out, Command::AddToMain(vec![Arc::clone(track)]));
            ui.close();
        }
    }
    let album = track.album.as_ref().filter(|a| !a.id.is_empty());
    let artist = track.artists.iter().find_map(|a| a.id.as_deref());
    if album.is_some() || artist.is_some() {
        ui.separator();
    }
    if let Some(album) = album
        && ui.button("Go to album").clicked()
    {
        open_album(out, &album.id);
        ui.close();
    }
    if let Some(artist) = artist
        && ui.button("Go to artist").clicked()
    {
        open_artist(out, artist);
        ui.close();
    }
}

/// Static skeleton rows for a list that is loading (D-States): the
/// Collection header and `rows` placeholder rows. No animation.
#[allow(dead_code)] // ui-batch: the collection calls it while the main playlist loads
pub fn skeleton(ui: &mut Ui, rows: usize) {
    // Bar widths from D-States, as a share of the cell, cycled.
    const TITLE: [f32; 5] = [0.62, 0.48, 0.74, 0.56, 0.40];
    const ARTIST: [f32; 5] = [0.54, 0.66, 0.40, 0.60, 0.50];
    const ALBUM: [f32; 5] = [0.70, 0.44, 0.58, 0.36, 0.64];
    let p = theme::pal();
    let full = ui.available_rect_before_wrap();
    let row_w = (full.width() - INSET_L - INSET_R).max(0.0);
    let cells = layout(COLLECTION, row_w);
    let head = Rect::from_min_size(
        pos2(full.left() + INSET_L, full.top()),
        vec2(row_w, theme::TABLE_HEADER_H),
    );
    let painter = ui.painter_at(full);
    header(&painter, head, &cells);
    let radius = CornerRadius::same(theme::RADIUS_ROW);
    let bar = |left: f32, w: f32, cy: f32| {
        Rect::from_min_size(
            pos2(left, cy - SKELETON_BAR_H / 2.0),
            vec2(w.max(0.0), SKELETON_BAR_H),
        )
    };
    let top = head.bottom() + theme::SPACE_4;
    for r in 0..rows {
        let rect = Rect::from_min_size(
            pos2(head.left(), top + r as f32 * theme::ROW_H),
            vec2(row_w, theme::ROW_H),
        );
        if rect.top() > full.bottom() {
            break;
        }
        let cy = rect.center().y;
        let v = r % TITLE.len();
        for cell in &cells {
            let (l, rt) = (cell.left(rect), cell.right(rect));
            let shape = match cell.slot {
                Slot::Col(Index) => bar(rt - 18.0, 18.0, cy),
                Slot::Col(Title) => {
                    let art = Rect::from_min_size(
                        pos2(l, cy - theme::ROW_ART / 2.0),
                        vec2(theme::ROW_ART, theme::ROW_ART),
                    );
                    painter.rect_filled(art, radius, p.surface_2);
                    let x = art.right() + ART_GAP;
                    bar(x, (rt - x) * TITLE[v], cy)
                }
                Slot::Col(Artist) => bar(l, cell.w * ARTIST[v], cy),
                Slot::Col(Album) => bar(l, cell.w * ALBUM[v], cy),
                Slot::Col(LastPlayed) => bar(l, 32.0, cy),
                Slot::Col(Time) => bar(rt - 28.0, 28.0, cy),
                _ => continue,
            };
            painter.rect_filled(shape, radius, p.surface_2);
        }
    }
    let used = (top - full.top() + rows as f32 * theme::ROW_H).min(full.height());
    ui.allocate_rect(
        Rect::from_min_size(full.min, vec2(full.width(), used)),
        Sense::hover(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn relative_time_boundaries() {
        assert_eq!(relative_time(-5), "1 min");
        assert_eq!(relative_time(0), "1 min");
        assert_eq!(relative_time(59 * 60), "59 min");
        assert_eq!(relative_time(3600), "1 h");
        assert_eq!(relative_time(23 * 3600), "23 h");
        assert_eq!(relative_time(DAY), "1 d");
        assert_eq!(relative_time(6 * DAY), "6 d");
        assert_eq!(relative_time(7 * DAY), "1 wk");
        assert_eq!(relative_time(29 * DAY), "4 wk");
        assert_eq!(relative_time(30 * DAY), "1 mo");
        assert_eq!(relative_time(364 * DAY), "11 mo");
        assert_eq!(relative_time(365 * DAY), "1 yr");
        assert_eq!(relative_time(800 * DAY), "2 yr");
    }

    fn slots(cells: &[Cell]) -> Vec<Slot> {
        cells.iter().map(|c| c.slot).collect()
    }

    #[test]
    fn collection_layout_matches_board() {
        // D-Collection: the table region is 284..1240, so the row is 936 px.
        let cells = layout(COLLECTION, 936.0);
        assert_eq!(
            slots(&cells),
            [
                Slot::Col(Index),
                Slot::Col(Title),
                Slot::Col(Artist),
                Slot::Col(Album),
                Slot::Col(LastPlayed),
                Slot::Like,
                Slot::Col(Time),
            ]
        );
        assert_eq!((cells[0].x, cells[0].w), (8.0, 44.0));
        // 936 - 16 padding - 184 fixed - 72 gaps = 664 free, 3:2:2.
        let title = cells[1];
        assert_eq!(title.x, 64.0);
        assert!((title.w - 664.0 * 3.0 / 7.0).abs() < 0.01);
        let time = cells[6];
        assert!((time.x + time.w - (936.0 - 8.0)).abs() < 0.01);
    }

    #[test]
    fn narrow_tables_drop_album_then_artist() {
        let wide = layout(OUTSIDE, 900.0);
        assert!(slots(&wide).contains(&Slot::Col(Album)));
        let mid = layout(OUTSIDE, 560.0);
        assert!(!slots(&mid).contains(&Slot::Col(Album)));
        assert!(slots(&mid).contains(&Slot::Col(Artist)));
        let narrow = layout(OUTSIDE, 380.0);
        assert!(!slots(&narrow).contains(&Slot::Col(Artist)));
        assert!(slots(&narrow).contains(&Slot::Col(Title)));
        assert!(slots(&narrow).contains(&Slot::Col(InEverything)));
        // Nothing overlaps and nothing is negative.
        for cells in [wide, mid, narrow, layout(COLLECTION, 0.0)] {
            for pair in cells.windows(2) {
                assert!(pair[0].x + pair[0].w <= pair[1].x);
                assert!(pair[0].w >= 0.0);
            }
        }
    }

    #[test]
    fn album_layout_uses_wider_columns() {
        let cells = layout(ALBUM, 1200.0);
        let ie = cells
            .iter()
            .find(|c| c.slot == Slot::Col(InEverything))
            .unwrap();
        assert_eq!(ie.w, IN_EVERYTHING_ALBUM_W);
        let title = cells.iter().find(|c| c.slot == Slot::Col(Title)).unwrap();
        let artist = cells.iter().find(|c| c.slot == Slot::Col(Artist)).unwrap();
        assert!((title.w / artist.w - 2.0).abs() < 0.01);
    }

    #[test]
    fn on_screen_rows() {
        let h = 32.0;
        assert_eq!(rows_on_screen(0.0, 320.0, h, 100), Some((0, 9)));
        // A half-visible last row counts.
        assert_eq!(rows_on_screen(0.0, 330.0, h, 100), Some((0, 10)));
        assert_eq!(rows_on_screen(48.0, 368.0, h, 100), Some((1, 11)));
        // Fewer rows than fit.
        assert_eq!(rows_on_screen(0.0, 600.0, h, 5), Some((0, 4)));
        // Rubber-banding above the top.
        assert_eq!(rows_on_screen(-20.0, 300.0, h, 100), Some((0, 9)));
        assert_eq!(rows_on_screen(0.0, 320.0, h, 0), None);
    }

    fn drag_to(points: &[(f32, f32)]) -> Option<SwipeMove> {
        let mut drag = SwipeDrag::new(pos2(100.0, 100.0));
        let mut last = None;
        for &(x, y) in points {
            last = drag.update(pos2(100.0 + x, 100.0 + y));
        }
        last
    }

    #[test]
    fn swipe_arms_at_the_threshold() {
        let m = drag_to(&[(39.0, 2.0)]).unwrap();
        assert_eq!(m.dir, SwipeDir::Add);
        assert!(!m.armed);
        assert_eq!(m.offset, 39.0);
        let m = drag_to(&[(20.0, 0.0), (SWIPE_COMMIT, 3.0)]).unwrap();
        assert!(m.armed);
        let m = drag_to(&[(-45.0, -4.0)]).unwrap();
        assert_eq!(m.dir, SwipeDir::Remove);
        assert!(m.armed);
    }

    #[test]
    fn swipe_back_below_the_threshold_disarms() {
        let m = drag_to(&[(60.0, 0.0), (30.0, 0.0)]).unwrap();
        assert!(!m.armed);
        // Crossing over turns it into the other direction.
        let m = drag_to(&[(60.0, 0.0), (-50.0, 0.0)]).unwrap();
        assert_eq!(m.dir, SwipeDir::Remove);
        assert!(m.armed);
    }

    #[test]
    fn row_slides_at_most_swipe_max() {
        assert_eq!(drag_to(&[(400.0, 0.0)]).unwrap().offset, SWIPE_MAX);
        assert_eq!(drag_to(&[(-400.0, 0.0)]).unwrap().offset, -SWIPE_MAX);
    }

    #[test]
    fn vertical_drags_are_not_swipes() {
        assert_eq!(drag_to(&[(0.0, 0.0)]), None);
        // More vertical than horizontal.
        assert_eq!(drag_to(&[(8.0, 10.0)]), None);
        // Past the slop, even though the horizontal travel is large.
        assert_eq!(drag_to(&[(80.0, SWIPE_SLOP_Y + 1.0)]), None);
        // Once vertical, the rest of the drag stays dead, even back on the
        // row's line.
        assert_eq!(drag_to(&[(0.0, 30.0), (80.0, 0.0)]), None);
        assert_eq!(drag_to(&[(0.0, -30.0), (-80.0, 2.0)]), None);
    }

    /// The swipe through real egui input: drags on a headless table.
    mod swipe_gesture {
        use std::time::Instant;

        use egui::{Event, PointerButton, Vec2};

        use super::super::*;
        use crate::queue::tests::track;
        use crate::views::Action;

        struct Rig {
            ctx: egui::Context,
            view: View,
            out: Out,
            full: Rect,
            time: f64,
            tracks: Arc<Vec<Arc<Track>>>,
        }

        impl Rig {
            fn new() -> Self {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let tracks = Arc::new(["a", "b", "c"].iter().map(|id| track(id)).collect());
                Self {
                    ctx,
                    view: View::default(),
                    out: Out::new(),
                    full: Rect::NOTHING,
                    time: 0.0,
                    tracks,
                }
            }

            fn frame(&mut self, state: &State, events: Vec<Event>) {
                self.time += 0.05;
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
                    time: Some(self.time),
                    events,
                    ..Default::default()
                };
                let (view, out, full, tracks) =
                    (&mut self.view, &mut self.out, &mut self.full, &self.tracks);
                self.ctx
                    .run_ui(input, |ui| {
                        *full = ui.available_rect_before_wrap();
                        let table = Table {
                            salt: "t",
                            tracks,
                            rows: None,
                            columns: ALBUM,
                            scroll_to: None,
                        };
                        show(ui, state, view, table, out);
                    })
                    .textures_delta
                    .clear();
            }

            /// A point `x` px into row `k`, on its centre line.
            fn at(&self, k: usize, x: f32) -> Pos2 {
                let top = self.full.top() + theme::TABLE_HEADER_H + theme::SPACE_4;
                pos2(
                    self.full.left() + INSET_L + x,
                    top + (k as f32 + 0.5) * theme::ROW_H,
                )
            }

            fn button(&mut self, state: &State, pos: Pos2, pressed: bool) {
                let e = Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                self.frame(state, vec![Event::PointerMoved(pos), e]);
            }

            /// Presses on row `k`, moves through `path` (offsets from the
            /// press) one frame each, and releases at the last point.
            fn drag(&mut self, state: &State, k: usize, path: &[(f32, f32)]) {
                let from = self.at(k, 200.0);
                self.frame(state, vec![Event::PointerMoved(from)]);
                self.button(state, from, true);
                let mut pos = from;
                for &(x, y) in path {
                    pos = from + Vec2::new(x, y);
                    self.frame(state, vec![Event::PointerMoved(pos)]);
                }
                self.button(state, pos, false);
                self.frame(state, vec![]);
            }

            fn commands(&self) -> Vec<&Command> {
                self.out
                    .iter()
                    .filter_map(|a| match a {
                        Action::Cmd(c) => Some(c),
                        _ => None,
                    })
                    .collect()
            }
        }

        /// Playing "a" with "b" and "c" upcoming.
        fn playing() -> State {
            let mut state = State::default();
            let tracks = Arc::new(["a", "b", "c"].iter().map(|id| track(id)).collect());
            let play = Command::PlayTracks {
                tracks,
                start: 0,
                seed: 1,
            };
            crate::state::apply(&mut state, &play, Instant::now());
            state
        }

        #[test]
        fn a_drag_right_past_the_threshold_queues_the_row() {
            let state = State::default();
            let mut r = Rig::new();
            r.frame(&state, vec![]);
            r.drag(&state, 1, &[(10.0, 0.0), (30.0, 2.0), (60.0, 3.0)]);
            match r.commands()[..] {
                [Command::AddToQueue(t)] => assert_eq!(t.id, "b"),
                ref other => panic!("{other:?}"),
            }
            assert_eq!(r.view.selected, None);
        }

        #[test]
        fn short_or_vertical_drags_do_nothing() {
            let state = playing();
            let mut r = Rig::new();
            r.frame(&state, vec![]);
            r.drag(&state, 1, &[(10.0, 0.0), (30.0, 0.0)]);
            r.drag(&state, 1, &[(10.0, 0.0), (60.0, 0.0), (20.0, 0.0)]);
            r.drag(&state, 1, &[(0.0, 10.0), (10.0, 40.0)]);
            r.drag(&state, 1, &[(0.0, 30.0), (60.0, 0.0)]);
            assert!(r.commands().is_empty(), "{:?}", r.commands());
        }

        #[test]
        fn a_drag_left_removes_only_an_upcoming_copy() {
            let state = playing();
            let mut r = Rig::new();
            r.frame(&state, vec![]);
            // "a" is playing, not upcoming: nothing.
            r.drag(&state, 0, &[(-10.0, 0.0), (-60.0, 0.0)]);
            assert!(r.commands().is_empty());
            r.drag(&state, 2, &[(-10.0, 0.0), (-60.0, 0.0)]);
            match r.commands()[..] {
                [Command::RemoveFromQueue(id)] => assert_eq!(id, "c"),
                ref other => panic!("{other:?}"),
            }
        }

        #[test]
        fn click_still_selects_and_double_click_still_plays() {
            let state = State::default();
            let mut r = Rig::new();
            r.frame(&state, vec![]);
            let pos = r.at(2, 200.0);
            r.frame(&state, vec![Event::PointerMoved(pos)]);
            r.button(&state, pos, true);
            r.button(&state, pos, false);
            assert!(r.commands().is_empty());
            assert!(matches!(r.view.selected, Some((_, 2))));
            r.button(&state, pos, true);
            r.button(&state, pos, false);
            match r.commands()[..] {
                [Command::PlayTracks { start, .. }] => assert_eq!(*start, 2),
                ref other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn scroll_target_sits_a_third_down() {
        assert_eq!(scroll_offset(0, 600.0, 32.0), 0.0);
        assert_eq!(scroll_offset(100, 600.0, 32.0), 3200.0 - 200.0);
    }
}
