//! The lyrics drawer: the queue drawer's size, border and shadow (D-Queue),
//! opened by `Command::ShowLyrics(true)`. `app.rs` places the drawer and
//! gives it the background, hairline and shadow; [`show`] fills it.
//!
//! A header like the queue's ("Lyrics", the song title, a close button),
//! the lines, and the source at the bottom. Synced lyrics mark the current
//! line (text-1, 600) and seek on click; the drawer scrolls, without
//! animation, only when the current line changes. While playing, the next
//! repaint is timed to the next line's start, never a fixed short interval.

use std::time::{Duration, Instant};

use egui::style::ScrollAnimation;
use egui::{
    Align, Align2, CornerRadius, Frame, Label, Margin, Pos2, Rect, RichText, ScrollArea, Sense, Ui,
    UiBuilder, vec2,
};
use model::{LyricLine, Lyrics};

use crate::state::{Command, Loadable, State};
use crate::views::{Out, cmd};
use crate::widgets::text_line;
use crate::{icons, theme};

const HEADER_H: f32 = 64.0;
const PAD_X: f32 = 16.0;
/// The source line: 11 px text with 10 px above and below.
const FOOTER_H: f32 = 34.0;
/// Space between lines.
const LINE_GAP: f32 = 10.0;

pub fn show(ui: &mut Ui, state: &State, now: Instant, out: &mut Out) {
    // A child that does not allocate in the frame: allocating past the
    // frame's content would grow the drawer.
    let full = ui.max_rect();
    let mut child = ui.new_child(UiBuilder::new().max_rect(full));
    child.set_clip_rect(full);
    let ui = &mut child;
    let p = theme::pal();
    let track = state.current_track();

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
    let title_y = if track.is_some() { 23.5 } else { 32.0 };
    text_line(
        &painter,
        Pos2::new(text_x, header.top() + title_y),
        "Lyrics",
        theme::section(),
        p.text_1,
        text_w,
    );
    if let Some(t) = track {
        text_line(
            &painter,
            Pos2::new(text_x, header.top() + 42.5),
            &t.title,
            theme::secondary(),
            p.text_3,
            text_w,
        );
    }
    if close_button(ui, close)
        .on_hover_text("Close lyrics")
        .clicked()
    {
        cmd(out, Command::ShowLyrics(false));
    }

    let body = Rect::from_min_max(Pos2::new(full.left(), header.bottom()), full.max);
    let lyrics = match (track, &state.lyrics) {
        (None, _) => Err("Nothing playing".to_owned()),
        (Some(t), Some((id, page))) if *id == t.id => match page {
            Loadable::Ready(Some(l)) => Ok(l),
            Loadable::Ready(None) => Err("No lyrics for this song".to_owned()),
            Loadable::Failed(e) if e.is_empty() => Err("Couldn't load lyrics".to_owned()),
            Loadable::Failed(e) => Err(format!("Couldn't load lyrics: {e}")),
            Loadable::Idle | Loadable::Loading => Err("Loading lyrics…".to_owned()),
        },
        // The fetch for this track has not been emitted yet.
        (Some(_), _) => Err("Loading lyrics…".to_owned()),
    };
    let lyrics = match lyrics {
        Ok(l) => l,
        Err(message) => {
            let inner = body.shrink2(vec2(PAD_X, theme::SPACE_16));
            ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                ui.add(Label::new(
                    RichText::new(message).font(theme::body()).color(p.text_3),
                ));
            });
            return;
        }
    };

    // Footer: the source.
    let lines_rect = match &lyrics.source {
        Some(source) => {
            let (lines_rect, footer) = body.split_top_bottom_at_y(body.bottom() - FOOTER_H);
            text_line(
                &painter,
                Pos2::new(footer.left() + PAD_X, footer.center().y),
                source,
                theme::small(),
                p.text_3,
                footer.width() - 2.0 * PAD_X,
            );
            lines_rect
        }
        None => body,
    };

    let playing = state.playback.is_playing();
    let pos = state.playback.position_at(now);
    let current = lyrics
        .synced
        .then(|| current_line(&lyrics.lines, pos))
        .flatten();
    if lyrics.synced
        && playing
        && let Some(wait) = next_change(&lyrics.lines, pos)
    {
        ui.ctx().request_repaint_after(wait);
    }

    // Scroll only when the current line (or the song) changes.
    let scroll_key = egui::Id::unique("lyrics-scrolled");
    let wanted = track.map(|t| (t.id.clone(), current));
    let scroll = current.is_some() && ui.data(|d| d.get_temp(scroll_key)) != Some(wanted.clone());
    ui.data_mut(|d| d.insert_temp(scroll_key, wanted));

    ui.scope_builder(UiBuilder::new().max_rect(lines_rect), |ui| {
        ScrollArea::vertical()
            .id_salt("lyrics-lines")
            .animated(false)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(PAD_X as i8, theme::SPACE_16 as i8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        lines(ui, lyrics, current, scroll, out);
                    });
            });
    });
}

/// The lyric lines, wrapped to the drawer's width.
fn lines(ui: &mut Ui, lyrics: &Lyrics, current: Option<usize>, scroll: bool, out: &mut Out) {
    let p = theme::pal();
    ui.spacing_mut().item_spacing.y = LINE_GAP;
    for (i, line) in lyrics.lines.iter().enumerate() {
        let is_current = current == Some(i);
        let text = if is_current {
            RichText::new(&line.text)
                .font(theme::row_strong())
                .color(p.text_1)
        } else {
            RichText::new(&line.text)
                .font(theme::body())
                .color(p.text_3)
        };
        let seek_to = line.start.filter(|_| lyrics.synced);
        let label = Label::new(text)
            .wrap()
            .selectable(false)
            .sense(if seek_to.is_some() {
                Sense::click()
            } else {
                Sense::hover()
            });
        let resp = ui.add(label);
        if is_current && scroll {
            resp.scroll_to_me_animation(Some(Align::Center), ScrollAnimation::none());
        }
        if let Some(start) = seek_to
            && resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            cmd(out, Command::Seek(start));
        }
    }
}

/// The line playing at `pos`: the last line that starts at or before it.
/// `None` before the first line. `lines` is sorted by start (the parser
/// sorts synced lyrics).
pub fn current_line(lines: &[LyricLine], pos: Duration) -> Option<usize> {
    lines
        .partition_point(|l| l.start.is_some_and(|s| s <= pos))
        .checked_sub(1)
}

/// Time from `pos` until the next line starts; `None` after the last line.
pub fn next_change(lines: &[LyricLine], pos: Duration) -> Option<Duration> {
    let next = lines.partition_point(|l| l.start.is_some_and(|s| s <= pos));
    lines.get(next)?.start.map(|s| s - pos)
}

/// The header's close button, drawn like the queue drawer's.
fn close_button(ui: &mut Ui, rect: Rect) -> egui::Response {
    let p = theme::pal();
    let resp = ui.interact(rect, ui.scope_id().with("close"), Sense::click());
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
        icons::X,
        theme::icon(16.0),
        color,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced(starts_ms: &[u64]) -> Vec<LyricLine> {
        starts_ms
            .iter()
            .map(|&ms| LyricLine {
                start: Some(Duration::from_millis(ms)),
                text: format!("{ms}"),
            })
            .collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn current_line_at_boundaries() {
        let lines = synced(&[1000, 2000, 2000, 5000]);
        // Before the first line.
        assert_eq!(current_line(&lines, ms(0)), None);
        assert_eq!(current_line(&lines, ms(999)), None);
        // Exactly at a start.
        assert_eq!(current_line(&lines, ms(1000)), Some(0));
        assert_eq!(current_line(&lines, ms(1999)), Some(0));
        // Two lines at one time: the later one.
        assert_eq!(current_line(&lines, ms(2000)), Some(2));
        assert_eq!(current_line(&lines, ms(5000)), Some(3));
        // After the last line.
        assert_eq!(current_line(&lines, ms(600_000)), Some(3));
        assert_eq!(current_line(&[], ms(10)), None);
    }

    #[test]
    fn next_repaint_is_the_next_line_start() {
        let lines = synced(&[1000, 2000, 5000]);
        assert_eq!(next_change(&lines, ms(0)), Some(ms(1000)));
        assert_eq!(next_change(&lines, ms(400)), Some(ms(600)));
        // At a start, the wait runs to the following line.
        assert_eq!(next_change(&lines, ms(1000)), Some(ms(1000)));
        assert_eq!(next_change(&lines, ms(4999)), Some(ms(1)));
        // After the last line: no timed repaint.
        assert_eq!(next_change(&lines, ms(5000)), None);
        assert_eq!(next_change(&[], ms(0)), None);
    }

    #[test]
    fn repaint_lands_on_the_line_change() {
        let lines = synced(&[1000, 2000, 5000]);
        let mut pos = ms(300);
        let mut seen = vec![current_line(&lines, pos)];
        // Waking exactly when asked shows each line once, in order.
        while let Some(wait) = next_change(&lines, pos) {
            pos += wait;
            seen.push(current_line(&lines, pos));
        }
        assert_eq!(seen, [None, Some(0), Some(1), Some(2)]);
    }
}
