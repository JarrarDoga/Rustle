//! The 56 px player bar (D-Collection, D-Queue, D-States buffering, D-Empty):
//! now playing with Locate and "#412", transport, seek, the "Playing from"
//! scope chip, queue and volume. `app.rs` fills it with `theme::tint()`.
//!
//! Layout follows the boards' grid: a 320 px left column, a 320 px right
//! column, and the transport and seek track between them, 24 px apart.

use std::time::{Duration, Instant};

use egui::epaint::RectShape;
use egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Response, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, vec2,
};
use model::Track;
use player::PlaybackStatus;

use crate::queue::Repeat;
use crate::state::{Command, State};
use crate::views::{Out, View, artists, cmd};
use crate::widgets::{fmt_time, seed, text_line};
use crate::{icons, theme, thumbs};

/// Left and right column width, and the gap between the three columns.
const SIDE_W: f32 = 320.0;
const COLUMN_GAP: f32 = 24.0;
/// The boards pad the bar 8 px left and 12 px right; `app.rs` gives 8 on
/// both sides.
const RIGHT_INSET: f32 = 4.0;
const ICON: f32 = 16.0;
const PLAY_BUTTON: f32 = 32.0;
/// Seek and volume tracks react to the pointer this far above and below
/// the 3 px line.
const TRACK_HIT_H: f32 = 16.0;
const HANDLE_R: f32 = 5.0;
const VOLUME_W: f32 = 72.0;
const CHIP_H: f32 = 26.0;
/// The center column dims to this while nothing plays (D-Empty).
const DISABLED_OPACITY: f32 = 0.4;

pub fn show(ui: &mut Ui, state: &State, view: &mut View, now: Instant, out: &mut Out) {
    let rect = ui.max_rect();
    let rect = Rect::from_min_max(rect.min, Pos2::new(rect.right() - RIGHT_INSET, rect.max.y));
    let side = SIDE_W.min((rect.width() - 2.0 * COLUMN_GAP) * 0.3);
    let left = Rect::from_min_max(rect.min, Pos2::new(rect.left() + side, rect.bottom()));
    let right = Rect::from_min_max(Pos2::new(rect.right() - side, rect.top()), rect.max);
    let center = Rect::from_min_max(
        Pos2::new(left.right() + COLUMN_GAP, rect.top()),
        Pos2::new(right.left() - COLUMN_GAP, rect.bottom()),
    );
    now_playing(ui, left, state, view, out);
    ui.scope_builder(UiBuilder::new().max_rect(center), |ui| {
        controls(ui, center, state, view, now, out);
    });
    extras(ui, right, state, view, out);
}

// ------------------------------------------------------------- now playing

fn now_playing(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, out: &mut Out) {
    let p = theme::pal();
    let painter = ui.painter_at(rect);
    let cy = rect.center().y;
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left(), cy - theme::PLAYER_ART / 2.0),
        vec2(theme::PLAYER_ART, theme::PLAYER_ART),
    );
    let x = art_rect.right() + 10.0;
    let Some(track) = state.current_track() else {
        // D-Empty: a blank tile with a note, and the label in text-2.
        painter.rect_filled(art_rect, CornerRadius::same(theme::RADIUS_ROW), p.surface_2);
        painter.text(
            art_rect.center(),
            Align2::CENTER_CENTER,
            icons::MUSIC_NOTES,
            theme::icon(ICON),
            p.text_2,
        );
        text_line(
            &painter,
            Pos2::new(x, cy),
            "Nothing playing",
            theme::row(),
            p.text_2,
            rect.right() - x,
        );
        return;
    };
    cover(&painter, art_rect, track, 1.0);

    let position = state.main_position(&track.id);
    let locate = position.map(|_| {
        Rect::from_center_size(
            Pos2::new(rect.right() - theme::ICON_BUTTON / 2.0, cy),
            vec2(theme::ICON_BUTTON, theme::ICON_BUTTON),
        )
    });
    let text_right = locate.map_or(rect.right(), |r| r.left() - 10.0);
    let w = text_right - x;
    text_line(
        &painter,
        Pos2::new(x, cy - 8.0),
        &track.title,
        theme::row(),
        p.text_1,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, cy + 9.0),
        &credit(&artists(track), position),
        theme::secondary(),
        p.text_2,
        w,
    );
    if let (Some(_), Some(r)) = (position, locate) {
        let resp = icon_button(ui, r, "locate", icons::CROSSHAIR, p.text_2, true);
        if resp.on_hover_text("Show playing song in list").clicked() {
            crate::views::locate(state, view, out);
        }
    }
}

/// "Artist · #412" when the song is in the main playlist (0-based
/// `position`, shown 1-based), else the artist alone.
pub(super) fn credit(artist: &str, position: Option<usize>) -> String {
    match position {
        Some(i) => format!("{artist} · #{}", i + 1),
        None => artist.to_owned(),
    }
}

/// Cover art with the D row radius (2): the thumbnail once loaded, a
/// letter tile until then. `opacity` dims it (autoplay radio rows).
pub(super) fn cover(painter: &Painter, rect: Rect, track: &Track, opacity: f32) {
    let radius = CornerRadius::same(theme::RADIUS_ROW);
    if let Some(tex) = thumbs::get(painter.ctx(), &track.thumbnails, rect.height()) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let tint = Color32::WHITE.gamma_multiply(opacity);
        painter.add(RectShape::filled(rect, radius, tint).with_texture(tex, uv));
        return;
    }
    let letter = track
        .title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or("♪".to_owned(), |c| c.to_uppercase().collect());
    painter.rect_filled(
        rect,
        radius,
        theme::art_color(&track.id).gamma_multiply(opacity),
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        letter,
        FontId::proportional(rect.height() * 0.42),
        theme::pal().text_1.gamma_multiply(0.67 * opacity),
    );
}

// -------------------------------------------------------------- transport

fn controls(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, now: Instant, out: &mut Out) {
    let p = theme::pal();
    let live = state.current_track().is_some();
    if !live {
        ui.set_opacity(DISABLED_OPACITY);
    }
    let cy = rect.center().y;
    let queue = &state.queue;
    let b = theme::ICON_BUTTON;
    let mut x = rect.left();
    let mut next_rect = |w: f32| {
        let r = Rect::from_min_size(Pos2::new(x, cy - w / 2.0), vec2(w, w));
        x += w + 4.0;
        r
    };
    let shuffle_r = next_rect(b);
    let prev_r = next_rect(b);
    let play_r = next_rect(PLAY_BUTTON);
    let next_r = next_rect(b);
    let repeat_r = next_rect(b);

    let toggle = |on: bool| if on { theme::accent() } else { p.text_2 };
    let shuffle_color = if live {
        toggle(queue.shuffle())
    } else {
        p.text_2
    };
    let step_color = if live { p.text_1 } else { p.text_2 };
    let resp = icon_button(
        ui,
        shuffle_r,
        "shuffle",
        icons::SHUFFLE,
        shuffle_color,
        live,
    );
    if resp.on_hover_text("Shuffle").clicked() {
        let on = !queue.shuffle();
        cmd(out, Command::SetShuffle { on, seed: seed() });
    }
    let resp = icon_button(ui, prev_r, "prev", icons::SKIP_BACK, step_color, live);
    if resp.on_hover_text("Previous").clicked() {
        cmd(out, Command::Previous);
    }
    let playing = state.playback.is_playing();
    if play_button(ui, play_r, playing, live).clicked() {
        cmd(out, Command::TogglePlay);
    }
    let resp = icon_button(ui, next_r, "next", icons::SKIP_FORWARD, step_color, live);
    if resp.on_hover_text("Next").clicked() {
        cmd(out, Command::Next);
    }
    let (glyph, tip) = match queue.repeat() {
        Repeat::Off => (icons::REPEAT, "Repeat"),
        Repeat::All => (icons::REPEAT, "Repeat all"),
        Repeat::One => (icons::REPEAT_ONCE, "Repeat one"),
    };
    let repeat_color = if live {
        toggle(queue.repeat() != Repeat::Off)
    } else {
        p.text_2
    };
    let resp = icon_button(ui, repeat_r, "repeat", glyph, repeat_color, live);
    if resp.on_hover_text(tip).clicked() {
        cmd(out, Command::CycleRepeat);
    }

    // Elapsed (or "Buffering"), the seek track, the duration; 16 px apart.
    let duration = state.duration().unwrap_or_default();
    let shown = view.seek_drag.map_or_else(
        || state.playback.position_at(now).min(duration),
        Duration::from_secs_f32,
    );
    let buffering = state.playback.status == PlaybackStatus::Buffering && view.seek_drag.is_none();
    let painter = ui.painter();
    let font = theme::caption();
    let (label, label_color) = if buffering {
        ("Buffering".to_owned(), p.text_1)
    } else {
        (fmt_time(shown), p.text_2)
    };
    let elapsed = painter.layout_no_wrap(label, font.clone(), label_color);
    let total = painter.layout_no_wrap(fmt_time(duration), font, p.text_2);
    let time_x = repeat_r.right() + 16.0;
    let end_x = rect.right() - total.size().x;
    let track_left = time_x + elapsed.size().x + 16.0;
    let track_right = end_x - 16.0;
    painter.galley(
        Pos2::new(time_x, cy - elapsed.size().y / 2.0),
        elapsed,
        label_color,
    );
    painter.galley(Pos2::new(end_x, cy - total.size().y / 2.0), total, p.text_2);
    if track_right - track_left < 8.0 {
        return;
    }
    let hit = Rect::from_min_max(
        Pos2::new(track_left, cy - TRACK_HIT_H / 2.0),
        Pos2::new(track_right, cy + TRACK_HIT_H / 2.0),
    );
    let seekable = live && !duration.is_zero();
    let sense = if seekable {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(hit, ui.scope_id().with("seek"), sense);
    let secs = duration.as_secs_f32();
    let pointer = resp.interact_pointer_pos();
    if let Some(pos) = pointer.filter(|_| resp.dragged()) {
        view.seek_drag = Some(fraction_at(pos.x, hit.left(), hit.width()) * secs);
    }
    if resp.drag_stopped() || resp.clicked() {
        let at = pointer
            .map(|pos| fraction_at(pos.x, hit.left(), hit.width()) * secs)
            .or(view.seek_drag);
        view.seek_drag = None;
        if let Some(at) = at {
            cmd(out, Command::Seek(Duration::from_secs_f32(at)));
        }
    }
    let fill = played_fraction(shown, duration);
    let handle = seekable && (resp.hovered() || resp.dragged());
    slider_track(ui.painter(), hit, fill, theme::accent(), handle);
    if handle {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
}

/// The 32 px play / pause button: accent-filled, the glyph in `on_accent`.
/// With nothing playing it is a `surface_3` square (D-Empty).
fn play_button(ui: &mut Ui, rect: Rect, playing: bool, live: bool) -> Response {
    let p = theme::pal();
    let sense = if live { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, ui.scope_id().with("play"), sense);
    let (fill, fg) = if !live {
        (p.surface_3, p.text_2)
    } else if resp.is_pointer_button_down_on() {
        (theme::accent_pressed(), p.on_accent)
    } else if resp.hovered() {
        (theme::accent_hover(), p.on_accent)
    } else {
        (theme::accent(), p.on_accent)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_CONTROL), fill);
    let glyph = if playing { icons::PAUSE } else { icons::PLAY };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon_fill(ICON),
        fg,
    );
    resp.on_hover_text(if playing { "Pause" } else { "Play" })
}

// ----------------------------------------------------------------- extras

fn extras(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, out: &mut Out) {
    let p = theme::pal();
    let cy = rect.center().y;
    let b = theme::ICON_BUTTON;

    // Right to left: volume track, mute, queue, chip; 6 px apart.
    let volume_r = Rect::from_min_max(
        Pos2::new(rect.right() - VOLUME_W, cy - TRACK_HIT_H / 2.0),
        Pos2::new(rect.right(), cy + TRACK_HIT_H / 2.0),
    );
    let mute_r = Rect::from_center_size(Pos2::new(volume_r.left() - 6.0 - b / 2.0, cy), vec2(b, b));
    let queue_r = Rect::from_center_size(Pos2::new(mute_r.left() - 6.0 - b / 2.0, cy), vec2(b, b));
    let lyrics_r =
        Rect::from_center_size(Pos2::new(queue_r.left() - 6.0 - b / 2.0, cy), vec2(b, b));

    let volume = state.playback.volume;
    let resp = ui.interact(
        volume_r,
        ui.scope_id().with("volume"),
        Sense::click_and_drag(),
    );
    if let Some(pos) = resp.interact_pointer_pos()
        && (resp.dragged() || resp.clicked())
    {
        let v = fraction_at(pos.x, volume_r.left(), volume_r.width());
        if v != volume {
            cmd(out, Command::SetVolume(v));
        }
    }
    let handle = resp.hovered() || resp.dragged();
    slider_track(ui.painter(), volume_r, volume, p.text_1, handle);
    if handle {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }

    let glyph = match volume {
        v if v <= 0.0 => icons::SPEAKER_X,
        v if v < 0.5 => icons::SPEAKER_LOW,
        _ => icons::SPEAKER_HIGH,
    };
    let tip = if volume > 0.0 { "Mute" } else { "Unmute" };
    let resp = icon_button(ui, mute_r, "mute", glyph, p.text_2, true);
    if resp.on_hover_text(tip).clicked() {
        if volume > 0.0 {
            view.unmuted_volume = volume;
            cmd(out, Command::SetVolume(0.0));
        } else {
            cmd(out, Command::SetVolume(view.unmuted_volume.max(0.1)));
        }
    }

    let color = if view.show_queue {
        theme::accent()
    } else {
        p.text_2
    };
    let resp = icon_button(ui, queue_r, "queue", icons::QUEUE, color, true);
    if resp.on_hover_text("Queue").clicked() {
        view.show_queue = !view.show_queue;
    }
    // Lyrics drawer: the reducer holds `lyrics_open`; app.rs keeps it and the
    // queue drawer mutually exclusive.
    let open = state.lyrics_open;
    let color = if open { theme::accent() } else { p.text_2 };
    let resp = icon_button(ui, lyrics_r, "lyrics", icons::MICROPHONE_STAGE, color, true);
    if resp.on_hover_text("Lyrics").clicked() {
        cmd(out, Command::ShowLyrics(!open));
    }

    if let Some(label) = state.queue.playing_from() {
        let room = lyrics_r.left() - 6.0 - rect.left();
        if scope_chip(
            ui,
            room,
            lyrics_r.left() - 6.0,
            cy,
            label,
            state.queue.shuffle(),
        )
        .clicked()
        {
            view.show_queue = !view.show_queue;
        }
    }
}

/// The "Playing from" chip, right-aligned at `right`: a 1 px border on the
/// tint, the shuffle (or play) icon in the accent, and the scope label.
fn scope_chip(ui: &mut Ui, room: f32, right: f32, cy: f32, label: &str, shuffle: bool) -> Response {
    let p = theme::pal();
    let painter = ui.painter();
    let pad = 10.0;
    let icon_w = 12.0;
    let gap = 6.0;
    let max_text = room - 2.0 * pad - icon_w - gap;
    let galley = painter.layout_no_wrap(label.to_owned(), theme::secondary(), p.text_1);
    let text_w = galley.size().x.min(max_text.max(0.0));
    let w = 2.0 * pad + icon_w + gap + text_w;
    let rect = Rect::from_min_max(
        Pos2::new(right - w, cy - CHIP_H / 2.0),
        Pos2::new(right, cy + CHIP_H / 2.0),
    );
    let resp = ui.interact(rect, ui.scope_id().with("scope-chip"), Sense::click());
    let radius = CornerRadius::same(theme::RADIUS_CONTROL);
    if resp.hovered() {
        painter.rect_filled(rect, radius, theme::on_tint_track());
    }
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(theme::STROKE_W, theme::on_tint_track()),
        StrokeKind::Inside,
    );
    let glyph = if shuffle { icons::SHUFFLE } else { icons::PLAY };
    painter.text(
        Pos2::new(rect.left() + pad + icon_w / 2.0, cy),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon(icon_w),
        theme::accent(),
    );
    text_line(
        painter,
        Pos2::new(rect.left() + pad + icon_w + gap, cy),
        label,
        theme::secondary(),
        p.text_1,
        text_w + 1.0,
    );
    resp.on_hover_text("Playing from · open the queue")
}

// ---------------------------------------------------------------- helpers

/// A 28 px icon button with no background (the bar's `.ib`): `color` at
/// rest, `text_1` on hover. A disabled one ignores the pointer.
fn icon_button(
    ui: &mut Ui,
    rect: Rect,
    name: &str,
    glyph: &str,
    color: Color32,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with(name), sense);
    let color = if enabled && resp.hovered() && color == theme::pal().text_2 {
        theme::pal().text_1
    } else {
        color
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        theme::icon(ICON),
        color,
    );
    resp
}

/// A 3 px track centered in `rect`: `on_tint_track` rest, `fill` color up
/// to `fraction`, and a handle at the end when `handle` is set.
fn slider_track(painter: &Painter, rect: Rect, fraction: f32, fill: Color32, handle: bool) {
    let h = theme::SLIDER_TRACK_H;
    let cy = rect.center().y;
    let line = Rect::from_min_max(
        Pos2::new(rect.left(), cy - h / 2.0),
        Pos2::new(rect.right(), cy + h / 2.0),
    );
    let radius = CornerRadius::same(theme::RADIUS_ROW);
    painter.rect_filled(line, radius, theme::on_tint_track());
    let x = line.left() + line.width() * fraction.clamp(0.0, 1.0);
    if x > line.left() {
        let done = Rect::from_min_max(line.min, Pos2::new(x, line.bottom()));
        painter.rect_filled(done, radius, fill);
    }
    if handle {
        painter.circle_filled(Pos2::new(x, cy), HANDLE_R, theme::pal().text_1);
    }
}

/// Where `x` falls along a track from `left`, `width` wide: 0.0 to 1.0.
fn fraction_at(x: f32, left: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    ((x - left) / width).clamp(0.0, 1.0)
}

/// How much of the track is played: 0.0 to 1.0, 0.0 with no duration.
fn played_fraction(position: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        return 0.0;
    }
    (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_adds_the_one_based_position() {
        assert_eq!(
            credit("The Vellum Hours", Some(411)),
            "The Vellum Hours · #412"
        );
        assert_eq!(credit("Odette Vance", None), "Odette Vance");
        assert_eq!(credit("A", Some(0)), "A · #1");
    }

    #[test]
    fn fraction_at_clamps_to_the_track() {
        assert_eq!(fraction_at(50.0, 0.0, 200.0), 0.25);
        assert_eq!(fraction_at(-10.0, 0.0, 200.0), 0.0);
        assert_eq!(fraction_at(500.0, 0.0, 200.0), 1.0);
        assert_eq!(fraction_at(120.0, 100.0, 40.0), 0.5);
        assert_eq!(fraction_at(5.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn played_fraction_handles_ends() {
        let s = Duration::from_secs;
        assert_eq!(played_fraction(s(78), s(312)), 0.25);
        assert_eq!(played_fraction(s(0), s(0)), 0.0);
        assert_eq!(played_fraction(s(400), s(312)), 1.0);
    }
}
