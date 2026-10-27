//! The 52 px icon rail (D-Collection): Collection, Search, Library. Batch 4
//! adds a settings button at the bottom.
//!
//! Integrator stub. The `browse` worker owns this file.

use egui::{Align2, Pos2, Rect, Sense, Ui, vec2};

use crate::icons;
use crate::state::State;
use crate::theme;
use crate::views::{Action, Out, Route, View};

pub fn show(ui: &mut Ui, _state: &State, view: &View, out: &mut Out) {
    let p = theme::pal();
    let top = ui.max_rect().top() + theme::SPACE_8;
    let x = ui.max_rect().center().x;
    for (i, (glyph, tip, route)) in [
        (icons::PLAYLIST, "Everything", Route::Collection),
        (icons::MAGNIFYING_GLASS, "Search", Route::Search),
        (icons::BOOKS, "Library", Route::Library),
    ]
    .into_iter()
    .enumerate()
    {
        let center = Pos2::new(x, top + 52.0 + i as f32 * 40.0);
        let rect = Rect::from_center_size(center, vec2(36.0, 36.0));
        let resp = ui.allocate_rect(rect, Sense::click()).on_hover_text(tip);
        let active = view.route == route;
        if active || resp.hovered() {
            ui.painter()
                .rect_filled(rect, theme::RADIUS_CONTROL, p.surface_2);
        }
        let color = if active { p.text_1 } else { p.text_2 };
        ui.painter().text(
            center,
            Align2::CENTER_CENTER,
            glyph,
            theme::icon(20.0),
            color,
        );
        if resp.clicked() {
            out.push(Action::Go(route));
        }
    }
}
