//! Small painted building blocks shared by the views.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use egui::epaint::RectShape;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense, TextFormat, Ui,
    vec2,
};
use model::Thumbnail;

use crate::{theme, thumbs};

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
pub enum ArtKind<'a> {
    Square(&'a [Thumbnail]),
    Round(&'a [Thumbnail]),
    Liked,
}

/// Cover art: the thumbnail once it has loaded, a letter tile until then.
pub fn art(painter: &Painter, rect: Rect, key: &str, title: &str, kind: ArtKind<'_>) {
    let radius = match kind {
        ArtKind::Round(_) => CornerRadius::same((rect.height() / 2.0).min(255.0) as u8),
        _ => CornerRadius::same(if rect.height() > 80.0 { 8 } else { 4 }),
    };
    let thumbnails = match kind {
        ArtKind::Square(t) | ArtKind::Round(t) => t,
        ArtKind::Liked => &[],
    };
    if let Some(tex) = thumbs::get(painter.ctx(), thumbnails, rect.height()) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, uv));
        return;
    }
    let (fill, glyph) = match kind {
        ArtKind::Liked => (Color32::from_rgb(0x5b, 0x3c, 0xc4), "♥".to_owned()),
        _ => (
            theme::art_color(key),
            title
                .chars()
                .find(|c| c.is_alphanumeric())
                .map_or("♪".to_owned(), |c| c.to_uppercase().collect()),
        ),
    };
    painter.rect_filled(rect, radius, fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(rect.height() * 0.42),
        Color32::from_white_alpha(170),
    );
}

/// A glyph button painted into `rect`. `active` draws it in the accent color.
pub fn icon_button(
    ui: &mut Ui,
    rect: Rect,
    name: &str,
    glyph: &str,
    size: f32,
    active: bool,
    tip: &str,
) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with(name), Sense::click());
    let color = if active {
        theme::accent()
    } else if resp.hovered() {
        theme::pal().text_1
    } else {
        theme::pal().text_2
    };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(size),
        color,
    );
    if active {
        ui.painter().circle_filled(
            Pos2::new(rect.center().x, rect.bottom() - 1.0),
            1.5,
            theme::accent(),
        );
    }
    resp.on_hover_text(tip)
}

/// A round filled button with a dark glyph, like the big play button.
pub fn round_button(
    ui: &mut Ui,
    center: Pos2,
    radius: f32,
    name: &str,
    glyph: &str,
    fill: Color32,
) -> Response {
    let rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
    let resp = ui.interact(rect, ui.scope_id().with(name), Sense::click());
    let r = if resp.hovered() { radius + 1.0 } else { radius };
    ui.painter().circle_filled(center, r, fill);
    // The play triangle looks centered when nudged right a little.
    let nudge = if glyph == "⏵" { radius * 0.08 } else { 0.0 };
    ui.painter().text(
        center + vec2(nudge, 0.0),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(radius * 1.05),
        Color32::from_rgb(0x10, 0x10, 0x10),
    );
    resp
}

/// A cover card for the home grid.
#[allow(dead_code)] // ui-batch: album cards on the artist page
pub fn card(ui: &mut Ui, key: &str, title: &str, subtitle: &str, kind: ArtKind<'_>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(172.0, 230.0), Sense::click());
    let painter = ui.painter_at(rect);
    let fill = if resp.hovered() {
        theme::pal().surface_3
    } else {
        theme::pal().surface_2
    };
    painter.rect_filled(rect, CornerRadius::same(8), fill);
    let art_rect = Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(148.0, 148.0));
    art(&painter, art_rect, key, title, kind);
    let x = rect.left() + 12.0;
    let w = rect.width() - 24.0;
    text_line(
        &painter,
        Pos2::new(x, art_rect.bottom() + 22.0),
        title,
        FontId::proportional(15.0),
        theme::pal().text_1,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, art_rect.bottom() + 44.0),
        subtitle,
        FontId::proportional(12.5),
        theme::pal().text_2,
        w,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A list row with art, a title and a subtitle, used for playlists,
/// albums and artists. Returns the row's click response.
pub fn entity_row(
    ui: &mut Ui,
    height: f32,
    key: &str,
    title: &str,
    subtitle: &str,
    kind: ArtKind<'_>,
    selected: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), theme::pal().surface_3);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::pal().surface_2);
    }
    let art_size = height - 14.0;
    let art_rect = Rect::from_min_size(
        Pos2::new(rect.left() + 8.0, rect.center().y - art_size / 2.0),
        vec2(art_size, art_size),
    );
    art(&painter, art_rect, key, title, kind);
    let x = art_rect.right() + 12.0;
    let w = rect.right() - x - 8.0;
    let title_color = if selected {
        theme::accent()
    } else {
        theme::pal().text_1
    };
    text_line(
        &painter,
        Pos2::new(x, rect.center().y - 9.0),
        title,
        FontId::proportional(14.5),
        title_color,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.center().y + 10.0),
        subtitle,
        FontId::proportional(12.5),
        theme::pal().text_2,
        w,
    );
    resp
}

/// A page title.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(28.0)
            .strong()
            .color(theme::pal().text_1),
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
