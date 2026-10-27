//! The album page (D-Album): header with "Add N missing to Everything",
//! tracks on the shared table with the "In Everything" column.
//!
//! Integrator stub. The `pages` worker owns this file.

use egui::{RichText, Ui};

use crate::state::State;
use crate::theme;
use crate::views::{Out, View, loading_or_error};
use crate::widgets::track_table;

pub fn show(ui: &mut Ui, state: &State, view: &mut View, id: &str, out: &mut Out) {
    let Some((_, page)) = state.album.as_ref().filter(|(open, _)| open == id) else {
        return loading_or_error::<()>(ui, &crate::state::Loadable::Loading, "this album");
    };
    let Some(album) = page.ready() else {
        return loading_or_error(ui, page, "this album");
    };
    ui.label(
        RichText::new(&album.summary.name)
            .font(theme::page_title())
            .color(theme::pal().text_1),
    );
    let tracks = std::sync::Arc::new(
        album
            .tracks
            .iter()
            .cloned()
            .map(std::sync::Arc::new)
            .collect::<Vec<_>>(),
    );
    track_table::show(
        ui,
        state,
        view,
        track_table::Table {
            salt: "album",
            tracks: &tracks,
            rows: None,
            columns: track_table::ALBUM,
            scroll_to: None,
        },
        out,
    );
}
