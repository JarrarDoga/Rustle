//! The Collection screen (D-Collection, D-Filter): the main playlist with
//! its header, filter row and track table.
//!
//! Integrator stub. The `collection` worker owns this file: the page header
//! with scope-aware Shuffle / Play in order, the filter field with
//! removable chips, "Not heard in 3 months", the sort menu, and local
//! filtering in [`update`].

use std::sync::Arc;

use egui::{RichText, Ui};

use crate::state::{Command, Loadable, NEEDS_LOGIN, Scope, State};
use crate::theme;
use crate::views::{Out, View, cmd, loading_or_error};
use crate::widgets::{seed, track_table};

/// Recomputes `view.collection.visible` and `.scope` from the filter,
/// chips, quick filter, range and sort. `app.rs` calls it every frame
/// before drawing, so the spine and player see this frame's values. Must
/// be cheap when nothing changed: memoize on the inputs.
pub fn update(state: &State, view: &mut View) {
    let c = &mut view.collection;
    let n = state.main_view().map_or(0, |v| v.tracks.len());
    if c.visible.len() != n {
        c.visible = Arc::new((0..n).collect());
        c.scope = Some(Scope::All);
    }
}

pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    let p = theme::pal();
    let Some((_, page)) = &state.main else {
        let text = if state.needs_login() {
            NEEDS_LOGIN
        } else {
            "Pick your main playlist in Library."
        };
        ui.label(RichText::new(text).color(p.text_3));
        return;
    };
    let main = match page {
        Loadable::Ready(v) => Arc::clone(v),
        other => return loading_or_error(ui, other, "your playlist"),
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&main.summary.name)
                .font(theme::page_title())
                .color(p.text_1),
        );
        if ui.button("Shuffle all").clicked() {
            let scope = view.collection.scope.clone().unwrap_or(Scope::All);
            cmd(
                out,
                Command::PlayScope {
                    scope,
                    shuffle: true,
                    seed: seed(),
                },
            );
        }
    });
    let visible = Arc::clone(&view.collection.visible);
    let scroll_to = view.collection.scroll_to.take();
    let shown = track_table::show(
        ui,
        state,
        view,
        track_table::Table {
            salt: "collection",
            tracks: &main.tracks,
            rows: Some(&visible),
            columns: track_table::COLLECTION,
            scroll_to,
        },
        out,
    );
    if shown.on_screen != view.collection.on_screen {
        view.collection.on_screen = shown.on_screen;
        ui.ctx().request_repaint();
    }
}
