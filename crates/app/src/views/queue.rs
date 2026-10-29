//! The queue drawer (D-Queue): 360 px over the right of the content, the
//! one region with a drop shadow. A header with the scope and a close
//! button, the now-playing row, "Next in queue" (the tracks the user queued,
//! shown only when there are some), "Next up" (the rest of the playlist,
//! shuffle or radio order; "Next from <scope>" under queued tracks), and the
//! "Autoplay radio" section with its toggle and the radio entries.
//!
//! `app.rs` places the drawer and gives it the background, hairline and
//! shadow; [`show`] fills it. Every list uses `show_rows`.

use egui::{Align2, CornerRadius, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, UiBuilder, vec2};

use crate::queue::Entry;
use crate::state::{Command, State};
use crate::views::player_bar::{cover, credit};
use crate::views::{Out, View, artists, cmd};
use crate::widgets::{fmt_time, text_line};
use crate::{icons, theme};

const HEADER_H: f32 = 64.0;
const PAD_X: f32 = 16.0;
const NOW_ROW_H: f32 = 52.0;
/// "Now playing" caption: 12 px above, 6 below.
const NOW_CAPTION_H: f32 = 31.0;
/// "Next up" caption: 14 px above, 6 below.
const NEXT_CAPTION_H: f32 = 33.0;
/// The autoplay block: 8 px margin, the rule, 12 px, two lines, 6 px.
const AUTOPLAY_H: f32 = 58.0;
/// The share of the drawer radio may take when both lists overflow.
const RADIO_SHARE: f32 = 0.4;
/// The share of the drawer "Next in queue" may take when the lists overflow.
const QUEUED_SHARE: f32 = 0.4;

/// Drag payload for reordering the queue: the dragged entry's index.
struct QueueDrag(usize);

pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    // A child that does not allocate in the frame: allocating past the
    // frame's content would grow the drawer.
    let full = ui.max_rect();
    let mut child = ui.new_child(UiBuilder::new().max_rect(full));
    child.set_clip_rect(full);
    let ui = &mut child;
    let p = theme::pal();
    let q = &state.queue;
    let main_name = state
        .main_view()
        .map_or("your main playlist", |v| v.summary.name.as_str());

    // Header.
    let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
    let painter = ui.painter_at(full);
    painter.hline(header.x_range(), header.bottom() - 0.5, theme::hairline());
    let close = Rect::from_center_size(
        Pos2::new(
            header.right() - 12.0 - theme::ICON_BUTTON / 2.0,
            header.center().y,
        ),
        vec2(theme::ICON_BUTTON, theme::ICON_BUTTON),
    );
    let text_x = header.left() + PAD_X;
    let text_w = close.left() - 8.0 - text_x;
    text_line(
        &painter,
        Pos2::new(text_x, header.top() + 23.5),
        "Queue",
        theme::section(),
        p.text_1,
        text_w,
    );
    text_line(
        &painter,
        Pos2::new(text_x, header.top() + 42.5),
        &subtitle(q.playing_from(), q.shuffle(), q.len()),
        theme::secondary(),
        p.text_2,
        text_w,
    );
    if drawer_button(ui, close, "close", icons::X)
        .on_hover_text("Close queue")
        .clicked()
    {
        view.show_queue = false;
    }

    // Now playing.
    let mut y = header.bottom();
    caption(&painter, full, y + 12.0, "Now playing", None);
    y += NOW_CAPTION_H;
    let now_rect = Rect::from_min_size(Pos2::new(full.left(), y), vec2(full.width(), NOW_ROW_H));
    match q.current_index() {
        Some(i) => {
            ui.scope_builder(UiBuilder::new().max_rect(now_rect), |ui| {
                now_row(ui, state, i, out);
            });
        }
        None => text_line(
            &painter,
            Pos2::new(full.left() + PAD_X, now_rect.center().y),
            "Nothing playing",
            theme::secondary(),
            p.text_3,
            full.width() - 2.0 * PAD_X,
        ),
    }
    y = now_rect.bottom();

    // Next in queue, next up and autoplay radio.
    let Sections {
        queued,
        upcoming,
        radio,
    } = if q.is_empty() {
        Sections::default()
    } else {
        sections(
            q.entries().iter().map(|e| (e.queued, e.radio)),
            q.upcoming_start(),
        )
    };
    let from = q.playing_from().map(scope_name);
    // With nothing after the queued tracks, "Next up" and its hint go.
    let show_upcoming = queued.is_empty() || !upcoming.is_empty();
    let row_h = theme::QUEUE_ROW_H;
    let captions = (!queued.is_empty() as u8 + show_upcoming as u8) as f32 * NEXT_CAPTION_H;
    let (queued_h, up_h, radio_h) = split_heights(
        full.bottom() - y - captions - AUTOPLAY_H,
        queued.len(),
        if show_upcoming {
            upcoming.len().max(1)
        } else {
            0
        },
        radio.len(),
        row_h,
    );

    if !queued.is_empty() {
        let more = format!("{} more", thousands(queued.len()));
        caption(&painter, full, y + 14.0, "Next in queue", Some(&more));
        y += NEXT_CAPTION_H;
        let rect = Rect::from_min_size(Pos2::new(full.left(), y), vec2(full.width(), queued_h));
        list(ui, rect, "queue-queued", &queued, state, out);
        y = rect.bottom();
    }

    if show_upcoming {
        let next_title = match from {
            Some(n) if !queued.is_empty() => format!("Next from {n}"),
            Some(n) => format!("Next up · from {n}"),
            None => "Next up".to_owned(),
        };
        let more = (!upcoming.is_empty()).then(|| format!("{} more", thousands(upcoming.len())));
        caption(&painter, full, y + 14.0, &next_title, more.as_deref());
        y += NEXT_CAPTION_H;
    }
    let up_rect = Rect::from_min_size(Pos2::new(full.left(), y), vec2(full.width(), up_h));
    if show_upcoming && upcoming.is_empty() {
        text_line(
            &painter,
            Pos2::new(full.left() + PAD_X, up_rect.top() + row_h / 2.0),
            "Right-click a song and pick \"Add to queue\".",
            theme::secondary(),
            p.text_3,
            full.width() - 2.0 * PAD_X,
        );
    } else if show_upcoming {
        list(ui, up_rect, "queue-next", &upcoming, state, out);
    }
    y = up_rect.bottom();

    // Autoplay block: rule, caption, explanation, toggle.
    let rule_y = y + 8.0;
    painter.hline(full.x_range(), rule_y + 0.5, theme::hairline());
    let toggle = Rect::from_min_size(
        Pos2::new(full.right() - PAD_X - 28.0, rule_y + 1.0 + 12.0 + 7.0),
        vec2(28.0, 16.0),
    );
    let block_w = toggle.left() - 8.0 - (full.left() + PAD_X);
    caption(&painter, full, rule_y + 1.0 + 12.0, "Autoplay radio", None);
    text_line(
        &painter,
        Pos2::new(full.left() + PAD_X, rule_y + 1.0 + 12.0 + 13.0 + 2.0 + 7.5),
        &format!("Plays when the queue runs out. Not in {main_name}."),
        theme::secondary(),
        p.text_2,
        block_w,
    );
    let on = q.autoplay();
    if switch(ui, toggle, on)
        .on_hover_text("Autoplay radio")
        .clicked()
    {
        cmd(out, Command::SetAutoplay(!on));
    }
    y += AUTOPLAY_H;

    if !radio.is_empty() {
        let radio_rect =
            Rect::from_min_size(Pos2::new(full.left(), y), vec2(full.width(), radio_h));
        list(ui, radio_rect, "queue-radio", &radio, state, out);
    }
}

/// The header line under "Queue": "Shuffling Everything · 1,535 songs".
fn subtitle(playing_from: Option<&str>, shuffle: bool, len: usize) -> String {
    let verb = if shuffle { "Shuffling" } else { "Playing" };
    match playing_from {
        Some(label) => format!("{verb} {label} songs"),
        None if len == 0 => "Empty".to_owned(),
        None if len == 1 => "1 song".to_owned(),
        None => format!("{} songs", thousands(len)),
    }
}

/// The scope's name from its "Playing from" label: "Saint Juniper · 41"
/// gives "Saint Juniper".
fn scope_name(label: &str) -> &str {
    label.rsplit_once(" · ").map_or(label, |(name, _)| name)
}

/// `n` with thousands separators: 1534 gives "1,534".
fn thousands(n: usize) -> String {
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

/// Queue indices of the entries after the current one, in order, by
/// drawer section.
#[derive(Debug, Default, PartialEq)]
struct Sections {
    /// "Next in queue": the up-next segment the user queued.
    queued: Vec<usize>,
    /// "Next up": the rest of the playlist, shuffle or radio order.
    upcoming: Vec<usize>,
    /// "Autoplay radio".
    radio: Vec<usize>,
}

/// Splits the entries after the current one by each entry's `(queued,
/// radio)` marks, in queue order. A queued mark wins over a radio one.
fn sections(marks: impl Iterator<Item = (bool, bool)>, start: usize) -> Sections {
    let mut out = Sections::default();
    for (i, (queued, radio)) in marks.enumerate().skip(start) {
        match (queued, radio) {
            (true, _) => out.queued.push(i),
            (false, true) => out.radio.push(i),
            (false, false) => out.upcoming.push(i),
        }
    }
    out
}

/// Heights of the "Next in queue", "Next up" and radio lists in `avail`
/// px. Each list gets what its rows need while all fit. When they don't,
/// "Next in queue" keeps up to [`QUEUED_SHARE`] of the room (more if the
/// others need less), then radio keeps up to [`RADIO_SHARE`] of what is left
/// (more if "Next up" needs less), and "Next up" scrolls in the rest.
fn split_heights(
    avail: f32,
    queued_rows: usize,
    up_rows: usize,
    radio_rows: usize,
    row_h: f32,
) -> (f32, f32, f32) {
    let avail = avail.max(0.0);
    let queued = queued_rows as f32 * row_h;
    let up = up_rows as f32 * row_h;
    let radio = radio_rows as f32 * row_h;
    let queued = if queued + up + radio <= avail {
        queued
    } else {
        queued.min((avail - up - radio).max(avail * QUEUED_SHARE))
    };
    let avail = avail - queued;
    if up + radio <= avail {
        return (queued, up, radio);
    }
    let radio = radio.min((avail - up).max(avail * RADIO_SHARE));
    (queued, avail - radio, radio)
}

/// A caption (11/500, text-3) at `top`, with optional right-aligned text.
fn caption(painter: &egui::Painter, full: Rect, top: f32, left: &str, right: Option<&str>) {
    let p = theme::pal();
    let cy = top + 6.5;
    let mut w = full.width() - 2.0 * PAD_X;
    if let Some(right) = right {
        let r = painter.text(
            Pos2::new(full.right() - PAD_X, cy),
            Align2::RIGHT_CENTER,
            right,
            theme::caption(),
            p.text_3,
        );
        w = r.left() - 8.0 - (full.left() + PAD_X);
    }
    text_line(
        painter,
        Pos2::new(full.left() + PAD_X, cy),
        left,
        theme::caption(),
        p.text_3,
        w,
    );
}

/// A virtualized list of queue entries (by queue index) in `rect`.
fn list(ui: &mut Ui, rect: Rect, salt: &str, indices: &[usize], state: &State, out: &mut Out) {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            .max_height(rect.height())
            .show_rows(ui, theme::QUEUE_ROW_H, indices.len(), |ui, range| {
                for k in range {
                    entry_row(ui, state, indices[k], out);
                }
            });
    });
}

/// The 52 px now-playing row: 40 px art, the title in the accent, the
/// speaker mark.
fn now_row(ui: &mut Ui, state: &State, index: usize, out: &mut Out) {
    let p = theme::pal();
    let entry = &state.queue.entries()[index];
    let rect = ui.max_rect();
    let resp = ui.interact(rect, ui.scope_id().with("now"), Sense::click());
    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, p.surface_2);
    }
    let art_rect = Rect::from_min_size(
        Pos2::new(
            rect.left() + PAD_X,
            rect.center().y - theme::PLAYER_ART / 2.0,
        ),
        vec2(theme::PLAYER_ART, theme::PLAYER_ART),
    );
    cover(&painter, art_rect, &entry.track, 1.0);
    let speaker_x = rect.right() - PAD_X - 8.0;
    painter.text(
        Pos2::new(speaker_x, rect.center().y),
        Align2::CENTER_CENTER,
        icons::SPEAKER_HIGH,
        theme::icon(16.0),
        theme::accent(),
    );
    let x = art_rect.right() + 10.0;
    let w = speaker_x - 8.0 - 10.0 - x;
    two_lines(
        &painter,
        rect,
        x,
        w,
        &entry.track.title,
        theme::row_strong(),
        theme::accent(),
        &credit(&artists(&entry.track), state.main_position(&entry.track.id)),
        p.text_2,
    );
    resp.context_menu(|ui| {
        if ui.button("Remove from queue").clicked() {
            cmd(out, Command::RemoveEntry(index));
            ui.close();
        }
    });
}

/// A 44 px row: 32 px art, title, "Artist · #415", time. Hover shows the
/// row action (remove, or "Add to" the main playlist for radio); drag
/// reorders; double-click plays; right-click opens the menu.
fn entry_row(ui: &mut Ui, state: &State, index: usize, out: &mut Out) {
    let p = theme::pal();
    let q = &state.queue;
    let entry: &Entry = &q.entries()[index];
    let track = &entry.track;
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::QUEUE_ROW_H),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let hovered = resp.hovered() || resp.dragged() || ui.rect_contains_pointer(rect);
    if hovered {
        painter.rect_filled(rect, CornerRadius::ZERO, p.surface_2);
    }
    let cy = rect.center().y;
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left() + PAD_X, cy - theme::QUEUE_ART / 2.0),
        vec2(theme::QUEUE_ART, theme::QUEUE_ART),
    );
    let radio = entry.radio;
    cover(&painter, art_rect, track, if radio { 0.7 } else { 1.0 });

    let time = track.duration.map(fmt_time).unwrap_or_default();
    let time_rect = painter.text(
        Pos2::new(rect.right() - PAD_X, cy),
        Align2::RIGHT_CENTER,
        time,
        theme::secondary(),
        p.text_3,
    );
    let action = Rect::from_center_size(
        Pos2::new(time_rect.left() - 10.0 - 12.0, cy),
        vec2(24.0, 24.0),
    );
    let position = state.main_position(&track.id);
    let x = art_rect.right() + 10.0;
    let w = action.left() - 10.0 - x;
    let (title_color, sub, sub_color) = if radio {
        (p.text_2, artists(track), p.text_3)
    } else {
        (p.text_1, credit(&artists(track), position), p.text_2)
    };
    two_lines(
        &painter,
        rect,
        x,
        w,
        &track.title,
        theme::row(),
        title_color,
        &sub,
        sub_color,
    );

    // Drag to reorder.
    resp.dnd_set_drag_payload(QueueDrag(index));
    if resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    let pointer_y = ui.ctx().pointer_interact_pos().map_or(0.0, |p| p.y);
    let above = pointer_y < cy;
    if resp.dnd_hover_payload::<QueueDrag>().is_some() {
        let y = if above { rect.top() } else { rect.bottom() };
        ui.painter()
            .hline(rect.x_range(), y, Stroke::new(2.0, theme::accent()));
    }
    if let Some(drag) = resp.dnd_release_payload::<QueueDrag>() {
        let slot = if above { index } else { index + 1 };
        let from = drag.0;
        let to = if from < slot { slot - 1 } else { slot };
        if from != to {
            cmd(out, Command::MoveEntry { from, to });
        }
    }

    // Hover action.
    let add = radio && position.is_none();
    if hovered {
        let (glyph, tip) = if add {
            (icons::PLUS, "Add to your main playlist")
        } else {
            (icons::X, "Remove from queue")
        };
        let name = format!("act{}", entry.uid);
        if drawer_button(ui, action, &name, glyph)
            .on_hover_text(tip)
            .clicked()
        {
            if add {
                cmd(out, Command::AddToMain(vec![track.clone()]));
            } else {
                cmd(out, Command::RemoveEntry(index));
            }
        }
    }

    if resp.double_clicked() {
        cmd(out, Command::JumpTo(index));
    }
    // Menu moves stay inside the entry's section: the queued tracks, or
    // everything after them. Dragging can cross between them.
    let context = q.context_start();
    let (start, last) = if entry.queued {
        (q.upcoming_start(), context.saturating_sub(1))
    } else {
        (context, q.len().saturating_sub(1))
    };
    resp.context_menu(|ui| {
        if ui.button("Play now").clicked() {
            cmd(out, Command::JumpTo(index));
            ui.close();
        }
        if add && ui.button("Add to your main playlist").clicked() {
            cmd(out, Command::AddToMain(vec![track.clone()]));
            ui.close();
        }
        let moves = [
            (index > start, "Move up", index.saturating_sub(1)),
            (index < last, "Move down", index + 1),
            (index > start, "Move to top", start),
        ];
        for (ok, label, to) in moves {
            if ok && ui.button(label).clicked() {
                cmd(out, Command::MoveEntry { from: index, to });
                ui.close();
            }
        }
        if ui.button("Remove from queue").clicked() {
            cmd(out, Command::RemoveEntry(index));
            ui.close();
        }
    });
}

/// Title and subtitle stacked around the row's middle.
#[allow(clippy::too_many_arguments)]
fn two_lines(
    painter: &egui::Painter,
    rect: Rect,
    x: f32,
    w: f32,
    title: &str,
    title_font: egui::FontId,
    title_color: egui::Color32,
    sub: &str,
    sub_color: egui::Color32,
) {
    let cy = rect.center().y;
    text_line(
        painter,
        Pos2::new(x, cy - 8.0),
        title,
        title_font,
        title_color,
        w,
    );
    text_line(
        painter,
        Pos2::new(x, cy + 9.0),
        sub,
        theme::secondary(),
        sub_color,
        w,
    );
}

/// The drawer's icon button (`.ib`): text-2, text-1 on a surface-2 square
/// on hover.
fn drawer_button(ui: &mut Ui, rect: Rect, name: &str, glyph: &str) -> egui::Response {
    let p = theme::pal();
    let resp = ui.interact(rect, ui.scope_id().with(name), Sense::click());
    let color = if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(theme::RADIUS_CONTROL), p.surface_2);
        p.text_1
    } else {
        p.text_2
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon(16.0),
        color,
    );
    resp
}

/// The 28 x 16 autoplay switch: accent track and an on-accent knob on the
/// right when on, a surface-3 track and a text-2 knob on the left when off.
fn switch(ui: &mut Ui, rect: Rect, on: bool) -> egui::Response {
    let p = theme::pal();
    let resp = ui.interact(rect, ui.scope_id().with("autoplay"), Sense::click());
    let (track, knob, knob_x) = if on {
        (theme::accent(), p.on_accent, rect.right() - 8.0)
    } else {
        (p.surface_3, p.text_2, rect.left() + 8.0)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), track);
    painter.circle_filled(Pos2::new(knob_x, rect.center().y), 6.0, knob);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_split_after_the_current_entry() {
        // Index 0 is history, 1 is current: upcoming starts at 2.
        let radio = [false, false, false, true, false, true];
        let marks = radio.map(|r| (false, r));
        let s = sections(marks.into_iter(), 2);
        assert!(s.queued.is_empty());
        assert_eq!(s.upcoming, vec![2, 4]);
        assert_eq!(s.radio, vec![3, 5]);
        assert_eq!(sections(marks.into_iter(), 6), Sections::default());
    }

    #[test]
    fn sections_put_queued_entries_first() {
        // Current at 0, two queued, the playlist, one radio entry.
        let marks = [
            (false, false),
            (true, false),
            (true, false),
            (false, false),
            (false, false),
            (false, true),
        ];
        let s = sections(marks.into_iter(), 1);
        assert_eq!(s.queued, vec![1, 2]);
        assert_eq!(s.upcoming, vec![3, 4]);
        assert_eq!(s.radio, vec![5]);
        // A queued radio entry shows as queued.
        let s = sections([(false, false), (true, true)].into_iter(), 1);
        assert_eq!((s.queued, s.radio), (vec![1], vec![]));
    }

    #[test]
    fn heights_fit_when_both_lists_fit() {
        assert_eq!(split_heights(500.0, 0, 3, 2, 44.0), (0.0, 132.0, 88.0));
        assert_eq!(split_heights(500.0, 2, 3, 2, 44.0), (88.0, 132.0, 88.0));
    }

    #[test]
    fn heights_cap_radio_when_both_overflow() {
        let (queued, up, radio) = split_heights(500.0, 0, 1534, 25, 44.0);
        assert_eq!(queued, 0.0);
        assert_eq!(radio, 200.0);
        assert_eq!(up, 300.0);
    }

    #[test]
    fn heights_cap_queued_when_all_overflow() {
        let (queued, up, radio) = split_heights(500.0, 40, 1534, 25, 44.0);
        assert_eq!(queued, 200.0);
        assert_eq!(radio, 120.0);
        assert_eq!(up, 180.0);
        // A short queue keeps its rows; the rest share what is left.
        let (queued, up, radio) = split_heights(500.0, 3, 1534, 0, 44.0);
        assert_eq!((queued, up, radio), (132.0, 368.0, 0.0));
    }

    #[test]
    fn radio_takes_what_next_up_leaves() {
        // Next up needs 88 px, so radio gets the other 412.
        let (_, up, radio) = split_heights(500.0, 0, 2, 25, 44.0);
        assert_eq!((up, radio), (88.0, 412.0));
        assert_eq!(split_heights(-10.0, 1, 2, 2, 44.0), (0.0, 0.0, 0.0));
    }

    #[test]
    fn labels() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1534), "1,534");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(scope_name("Saint Juniper · 41"), "Saint Juniper");
        assert_eq!(scope_name("#380 to #520 · 141"), "#380 to #520");
        assert_eq!(scope_name("Odd"), "Odd");
        assert_eq!(
            subtitle(Some("Everything · 1,535"), true, 1535),
            "Shuffling Everything · 1,535 songs"
        );
        assert_eq!(subtitle(None, false, 3), "3 songs");
        assert_eq!(subtitle(None, false, 0), "Empty");
    }
}
