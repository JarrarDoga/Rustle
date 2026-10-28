//! The 52 px icon rail (D-Collection): the app mark, then Collection,
//! Search and Library.
//!
//! The space below the three buttons stays empty: Batch 4 adds a settings
//! button at the bottom of the rail.

use egui::{Align2, Pos2, Rect, Sense, Ui, vec2};

use crate::icons;
use crate::state::State;
use crate::theme;
use crate::views::{Action, Out, Route, View};

/// The Rustle mark (`widgets::logo`), 24 px, 16 px from the top. The rail
/// buttons start where they did with the board's 16 px mark.
const MARK: f32 = 24.0;
/// Rail buttons are 36 px with a 20 px glyph and 4 px between them
/// (D-Collection), larger than the 28 px icon button token.
const BUTTON: f32 = theme::ICON_BUTTON + theme::SPACE_8;
const GLYPH: f32 = 20.0;

pub fn show(ui: &mut Ui, state: &State, view: &View, out: &mut Out) {
    let p = theme::pal();
    let rect = ui.max_rect();
    let x = rect.center().x;
    let mut y = rect.top() + theme::SPACE_16;

    let mark = Rect::from_min_size(Pos2::new(x - MARK / 2.0, y), vec2(MARK, MARK));
    crate::widgets::logo::paint(ui.painter(), mark, p.bg);
    y += MARK + theme::SPACE_8 + theme::SPACE_4;

    let main = state
        .main_view()
        .map_or("Collection", |v| v.summary.name.as_str());
    for (glyph, tip, route) in [
        (icons::PLAYLIST, main, Route::Collection),
        (icons::MAGNIFYING_GLASS, "Search", Route::Search),
        (icons::BOOKS, "Library", Route::Library),
    ] {
        let rect = Rect::from_min_size(Pos2::new(x - BUTTON / 2.0, y), vec2(BUTTON, BUTTON));
        y += BUTTON + theme::SPACE_4;
        let resp = ui.allocate_rect(rect, Sense::click()).on_hover_text(tip);
        let active = lights(&view.route).as_ref() == Some(&route);
        let fill = if active {
            Some(p.surface_3)
        } else if resp.hovered() {
            Some(p.surface_2)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, theme::RADIUS_CONTROL, fill);
        }
        let color = if active || resp.hovered() {
            p.text_1
        } else {
            p.text_2
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            theme::icon(GLYPH),
            color,
        );
        if resp.clicked() && view.route != route {
            out.push(Action::Go(route));
        }
    }
}

/// The settings button at the bottom of the rail; opens the settings
/// popover (`views::settings`).
pub fn settings_button(ui: &mut Ui) -> egui::Response {
    let p = theme::pal();
    let rect = ui.max_rect();
    let center = Pos2::new(
        rect.center().x,
        rect.bottom() - theme::SPACE_16 - BUTTON / 2.0,
    );
    let r = Rect::from_center_size(center, vec2(BUTTON, BUTTON));
    let resp = ui
        .allocate_rect(r, Sense::click())
        .on_hover_text("Settings");
    if resp.hovered() {
        ui.painter()
            .rect_filled(r, theme::RADIUS_CONTROL, p.surface_2);
    }
    let color = if resp.hovered() { p.text_1 } else { p.text_2 };
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        icons::GEAR_SIX,
        theme::icon(GLYPH),
        color,
    );
    resp
}

/// The rail button a route lights: other playlists and liked songs live
/// under Library; album and artist pages light none.
fn lights(route: &Route) -> Option<Route> {
    match route {
        Route::Collection => Some(Route::Collection),
        Route::Search => Some(Route::Search),
        Route::Library | Route::Liked | Route::Playlist(_) => Some(Route::Library),
        Route::Album(_) | Route::Artist(_) => None,
    }
}
