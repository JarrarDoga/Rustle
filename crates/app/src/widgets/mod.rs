//! Small painted building blocks shared by the views.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::epaint::RectShape;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, TextFormat};
use model::Thumbnail;

use crate::{theme, thumbs};

pub mod logo;
pub mod spine;
pub mod track_table;

/// `m:ss`, or `h:mm:ss` from an hour up.
pub fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A shuffle seed. Shuffles only need to differ from run to run.
pub fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
}

/// Paints one line of text left-aligned at `pos` (its vertical center),
/// cut with an ellipsis at `max_width`.
pub fn text_line(
    painter: &Painter,
    pos: Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) {
    if max_width <= 4.0 {
        return;
    }
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(max_width);
    let galley = painter.layout_job(job);
    let top = pos.y - galley.size().y / 2.0;
    painter.galley(Pos2::new(pos.x, top), galley, color);
}

/// Which art to draw, with the thumbnails to load for it.
#[allow(dead_code)] // Square: the D 2 px art for any view that needs it
pub enum ArtKind<'a> {
    Square(&'a [Thumbnail]),
    Round(&'a [Thumbnail]),
}

/// Cover art: the thumbnail once it has loaded, a letter tile until then.
pub fn art(painter: &Painter, rect: Rect, key: &str, title: &str, kind: ArtKind<'_>) {
    let radius = match kind {
        ArtKind::Round(_) => CornerRadius::same((rect.height() / 2.0).min(255.0) as u8),
        _ => CornerRadius::same(theme::RADIUS_ROW),
    };
    let thumbnails = match kind {
        ArtKind::Square(t) | ArtKind::Round(t) => t,
    };
    if let Some(tex) = thumbs::get(painter.ctx(), thumbnails, rect.height()) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, uv));
        return;
    }
    let fill = theme::art_color(key);
    let glyph = title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or("♪".to_owned(), |c| c.to_uppercase().collect());
    painter.rect_filled(rect, radius, fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(rect.height() * 0.42),
        Color32::from_white_alpha(170),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_times() {
        assert_eq!(fmt_time(Duration::from_secs(0)), "0:00");
        assert_eq!(fmt_time(Duration::from_secs(214)), "3:34");
        assert_eq!(fmt_time(Duration::from_secs(3725)), "1:02:05");
    }
}
