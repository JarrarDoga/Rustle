//! The artist page (D-Artist): top songs on the shared table, then albums
//! and singles as cards.
//!
//! Integrator stub. The `pages` worker owns this file.

use egui::{RichText, Ui};

use crate::state::State;
use crate::theme;
use crate::views::{Out, View, loading_or_error};

pub fn show(ui: &mut Ui, state: &State, _view: &mut View, id: &str, _out: &mut Out) {
    let Some((_, page)) = state.artist.as_ref().filter(|(open, _)| open == id) else {
        return loading_or_error::<()>(ui, &crate::state::Loadable::Loading, "this artist");
    };
    let Some(artist) = page.ready() else {
        return loading_or_error(ui, page, "this artist");
    };
    ui.label(
        RichText::new(&artist.summary.name)
            .font(theme::page_title())
            .color(theme::pal().text_1),
    );
    for t in &artist.top_songs {
        ui.label(RichText::new(&t.title).color(theme::pal().text_2));
    }
}
