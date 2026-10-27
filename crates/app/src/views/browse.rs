//! The 232 px browse panel (D-Collection): every artist or album in the main
//! playlist with song counts; click to filter.
//!
//! Integrator stub. The `browse` worker owns this file: Artists / Albums
//! tabs, counts computed once per main-playlist change (memoize; never per
//! frame), the playing marker, and clicking a row to add a filter chip.

use egui::{RichText, Ui};

use crate::state::State;
use crate::theme;
use crate::views::{Out, View};

pub fn show(ui: &mut Ui, state: &State, _view: &mut View, _out: &mut Out) {
    let p = theme::pal();
    let name = state
        .main_view()
        .map_or("Everything", |v| v.summary.name.as_str());
    ui.label(
        RichText::new(format!("Browse {name}"))
            .font(theme::caption())
            .color(p.text_3),
    );
}
