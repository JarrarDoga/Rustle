//! The spine (D-Range, D-Tokens): a 20 px strip at the right edge of the
//! Collection that stands for the whole main playlist, one 4 px segment per
//! ~10 songs colored by album art.
//!
//! Integrator stub. The `spine` worker owns this file: segments colored
//! from `State::art_colors`, the viewport box from
//! `view.collection.on_screen`, the playing line, filter ticks from
//! `view.collection.visible`, click to jump (`scroll_to`), and drag to
//! select a range (`range`) with resize handles.

use egui::{Sense, Ui};

use crate::state::State;
use crate::theme;
use crate::views::{Out, View};

/// Draws the spine in `ui.max_rect()` (full height of the content region,
/// `theme::SPINE_W` wide).
pub fn show(ui: &mut Ui, state: &State, view: &mut View, _out: &mut Out) {
    let rect = ui.max_rect();
    let resp = ui.allocate_rect(rect, Sense::click());
    ui.painter().rect_filled(rect, 0, theme::pal().surface_2);
    let n = state.main_view().map_or(0, |v| v.tracks.len());
    if let Some(pos) = resp
        .interact_pointer_pos()
        .filter(|_| resp.clicked() && n > 0)
    {
        let t = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        view.collection.scroll_to = Some(((t * n as f32) as usize).min(n - 1));
    }
}
