//! The 56 px player bar (D-Collection, D-Queue, D-States buffering): now
//! playing with Locate and "#412", transport, seek, the "Playing from"
//! scope chip, queue and volume. Filled with `theme::tint()`.
//!
//! Integrator stub (the old player bar). The `player` worker owns this file.

use std::time::{Duration, Instant};

use egui::{Align2, Pos2, Rect, Ui, UiBuilder, vec2};

use crate::queue::Repeat;
use crate::state::{Command, State};
use crate::theme;
use crate::views::{Out, View, artists, cmd};
use crate::widgets::{ArtKind, art, fmt_time, icon_button, round_button, seed, text_line};

pub fn show(ui: &mut Ui, state: &State, view: &mut View, now: Instant, out: &mut Out) {
    let rect = ui.max_rect();
    let w = rect.width();
    let left = Rect::from_min_max(rect.min, Pos2::new(rect.left() + w * 0.3, rect.bottom()));
    let right = Rect::from_min_max(Pos2::new(rect.right() - w * 0.3, rect.top()), rect.max);
    let center = Rect::from_min_max(
        Pos2::new(left.right(), rect.top()),
        Pos2::new(right.left(), rect.bottom()),
    );
    now_playing(ui, left, state);
    controls(ui, center, state, view, now, out);
    extras(ui, right, state, view, out);
}

fn now_playing(ui: &mut Ui, rect: Rect, state: &State) {
    let painter = ui.painter_at(rect);
    let Some(track) = state.current_track() else {
        text_line(
            &painter,
            Pos2::new(rect.left(), rect.center().y),
            "Nothing playing",
            theme::row(),
            theme::pal().text_3,
            rect.width(),
        );
        return;
    };
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left(), rect.center().y - theme::PLAYER_ART / 2.0),
        vec2(theme::PLAYER_ART, theme::PLAYER_ART),
    );
    art(
        &painter,
        art_rect,
        &track.id,
        &track.title,
        ArtKind::Square(&track.thumbnails),
    );
    let x = art_rect.right() + 12.0;
    let w = rect.right() - x - 12.0;
    text_line(
        &painter,
        Pos2::new(x, rect.center().y - 9.0),
        &track.title,
        theme::row(),
        theme::pal().text_1,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.center().y + 11.0),
        &artists(track),
        theme::secondary(),
        theme::pal().text_2,
        w,
    );
}

fn controls(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, now: Instant, out: &mut Out) {
    let cx = rect.center().x;
    let cy = rect.top() + 20.0;
    let at = |dx: f32| Rect::from_center_size(Pos2::new(cx + dx, cy), vec2(30.0, 30.0));
    let queue = &state.queue;

    if icon_button(
        ui,
        at(-92.0),
        "shuffle",
        crate::icons::SHUFFLE,
        16.0,
        queue.shuffle(),
        "Shuffle",
    )
    .clicked()
    {
        let on = !queue.shuffle();
        cmd(out, Command::SetShuffle { on, seed: seed() });
    }
    if icon_button(
        ui,
        at(-46.0),
        "prev",
        crate::icons::SKIP_BACK,
        18.0,
        false,
        "Previous",
    )
    .clicked()
    {
        cmd(out, Command::Previous);
    }
    let playing = state.playback.is_playing();
    let glyph = if playing {
        crate::icons::PAUSE
    } else {
        crate::icons::PLAY
    };
    if round_button(
        ui,
        Pos2::new(cx, cy),
        17.0,
        "play",
        glyph,
        theme::pal().text_1,
    )
    .on_hover_text(if playing { "Pause" } else { "Play" })
    .clicked()
    {
        cmd(out, Command::TogglePlay);
    }
    if icon_button(
        ui,
        at(46.0),
        "next",
        crate::icons::SKIP_FORWARD,
        18.0,
        false,
        "Next",
    )
    .clicked()
    {
        cmd(out, Command::Next);
    }
    let (rglyph, rtip) = match queue.repeat() {
        Repeat::Off => (crate::icons::REPEAT, "Repeat"),
        Repeat::All => (crate::icons::REPEAT, "Repeat all"),
        Repeat::One => (crate::icons::REPEAT_ONCE, "Repeat one"),
    };
    let ractive = queue.repeat() != Repeat::Off;
    if icon_button(ui, at(92.0), "repeat", rglyph, 16.0, ractive, rtip).clicked() {
        cmd(out, Command::CycleRepeat);
    }

    // Seek row.
    let y = rect.bottom() - 16.0;
    let half = (rect.width() * 0.5 - 56.0).clamp(60.0, 300.0);
    let bar = Rect::from_center_size(Pos2::new(cx, y), vec2(half * 2.0, 18.0));
    let duration = state.duration().unwrap_or_default();
    let shown = view.seek_drag.map_or_else(
        || state.playback.position_at(now).min(duration),
        Duration::from_secs_f32,
    );
    let font = theme::caption();
    let painter = ui.painter();
    painter.text(
        Pos2::new(bar.left() - 10.0, y),
        Align2::RIGHT_CENTER,
        fmt_time(shown),
        font.clone(),
        theme::pal().text_2,
    );
    painter.text(
        Pos2::new(bar.right() + 10.0, y),
        Align2::LEFT_CENTER,
        fmt_time(duration),
        font,
        theme::pal().text_2,
    );
    let max = duration.as_secs_f32().max(1.0);
    let mut value = shown.as_secs_f32();
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(bar), |ui| {
            ui.spacing_mut().slider_width = bar.width();
            ui.add_enabled(
                state.current_track().is_some(),
                egui::Slider::new(&mut value, 0.0..=max)
                    .show_value(false)
                    .trailing_fill(true)
                    .handle_shape(egui::style::HandleShape::Circle),
            )
        })
        .inner;
    if resp.dragged() {
        view.seek_drag = Some(value);
    }
    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
        view.seek_drag = None;
        cmd(out, Command::Seek(Duration::from_secs_f32(value)));
    }
}

fn extras(ui: &mut Ui, rect: Rect, state: &State, view: &mut View, out: &mut Out) {
    let cy = rect.center().y;
    let slider = Rect::from_min_max(
        Pos2::new(rect.right() - 110.0, cy - 9.0),
        Pos2::new(rect.right() - 4.0, cy + 9.0),
    );
    let mut volume = state.playback.volume;
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(slider), |ui| {
            ui.spacing_mut().slider_width = slider.width();
            ui.add(
                egui::Slider::new(&mut volume, 0.0..=1.0)
                    .show_value(false)
                    .trailing_fill(true)
                    .handle_shape(egui::style::HandleShape::Circle),
            )
        })
        .inner;
    if resp.changed() {
        cmd(out, Command::SetVolume(volume));
    }

    let glyph = match state.playback.volume {
        v if v <= 0.0 => crate::icons::SPEAKER_X,
        v if v < 0.5 => crate::icons::SPEAKER_LOW,
        _ => crate::icons::SPEAKER_HIGH,
    };
    let mute = Rect::from_center_size(Pos2::new(slider.left() - 18.0, cy), vec2(28.0, 28.0));
    if icon_button(ui, mute, "mute", glyph, 16.0, false, "Mute").clicked() {
        if state.playback.volume > 0.0 {
            view.unmuted_volume = state.playback.volume;
            cmd(out, Command::SetVolume(0.0));
        } else {
            cmd(out, Command::SetVolume(view.unmuted_volume.max(0.1)));
        }
    }
    let queue = Rect::from_center_size(Pos2::new(mute.left() - 22.0, cy), vec2(28.0, 28.0));
    if icon_button(
        ui,
        queue,
        "queue",
        crate::icons::QUEUE,
        16.0,
        view.show_queue,
        "Queue",
    )
    .clicked()
    {
        view.show_queue = !view.show_queue;
    }
}
