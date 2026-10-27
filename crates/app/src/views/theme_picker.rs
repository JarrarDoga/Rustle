//! The theme picker inside the settings popover: the `theme::PALETTES`
//! entries grouped as Dark and Medium, one row each with a swatch strip of
//! the palette's own `bg`, `surface_3`, `text_1` and `fallback_accent`.
//! Picking one sends `Command::SetSetting(SettingChange::Theme(name))`;
//! `app.rs` applies the palette when `state.settings.theme` changes.

use egui::{Align2, Pos2, Rect, ScrollArea, Sense, Stroke, Ui, vec2};

use crate::icons;
use crate::state::{Command, SettingChange, State};
use crate::theme::{self, Palette, Tone};
use crate::views::{Out, cmd};

/// Taller lists scroll, so the popover stays on screen.
const MAX_H: f32 = 360.0;
/// Row width: swatches, the longest name and the check, inside the popover.
const ROW_W: f32 = 256.0;
const SWATCH: f32 = 12.0;
const SWATCH_GAP: f32 = 3.0;
const CHECK: f32 = 14.0;
const CAPTION_H: f32 = 24.0;

pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    ScrollArea::vertical()
        .id_salt("theme_picker")
        .max_height(MAX_H)
        .show(ui, |ui| {
            ui.set_width(ROW_W);
            ui.spacing_mut().item_spacing.y = 0.0;
            for (tone, label) in [(Tone::Dark, "Dark"), (Tone::Medium, "Medium")] {
                caption(ui, label);
                for p in theme::PALETTES.iter().filter(|p| p.tone == tone) {
                    let selected = state.settings.theme == p.name;
                    if row(ui, p, selected) && !selected {
                        cmd(
                            out,
                            Command::SetSetting(SettingChange::Theme(p.name.to_owned())),
                        );
                    }
                }
            }
        });
}

fn caption(ui: &mut Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CAPTION_H), Sense::hover());
    ui.painter().text(
        Pos2::new(rect.left() + theme::SPACE_8, rect.bottom() - theme::SPACE_4),
        Align2::LEFT_BOTTOM,
        label,
        theme::caption(),
        theme::pal().text_3,
    );
}

/// One palette row; true when clicked.
fn row(ui: &mut Ui, palette: &Palette, selected: bool) -> bool {
    let p = theme::pal();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::FACET_ROW_H),
        Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return resp.clicked();
    }
    let painter = ui.painter();
    let hovered = resp.hovered();
    let radius = theme::RADIUS_ROW;
    if selected {
        painter.rect_filled(rect, radius, p.surface_3);
    } else if hovered {
        painter.rect_filled(rect, radius, p.surface_2);
    }
    let y = rect.center().y;

    // The strip shows the palette's own colors; its stroke_2 outlines each
    // square so a swatch matching the current bg still shows.
    let mut x = rect.left() + theme::SPACE_8;
    for fill in [
        palette.bg,
        palette.surface_3,
        palette.text_1,
        palette.fallback_accent,
    ] {
        let square = Rect::from_min_size(Pos2::new(x, y - SWATCH / 2.0), vec2(SWATCH, SWATCH));
        painter.rect_filled(square, theme::RADIUS_ROW, fill);
        painter.rect_stroke(
            square,
            theme::RADIUS_ROW,
            Stroke::new(theme::STROKE_W, palette.stroke_2),
            egui::StrokeKind::Inside,
        );
        x += SWATCH + SWATCH_GAP;
    }

    let color = if selected || hovered {
        p.text_1
    } else {
        p.text_2
    };
    painter.text(
        Pos2::new(x - SWATCH_GAP + theme::SPACE_12, y),
        Align2::LEFT_CENTER,
        palette.name,
        theme::row(),
        color,
    );
    if selected {
        painter.text(
            Pos2::new(rect.right() - theme::SPACE_8, y),
            Align2::RIGHT_CENTER,
            icons::CHECK,
            theme::icon(CHECK),
            p.text_1,
        );
    }
    resp.clicked()
}
