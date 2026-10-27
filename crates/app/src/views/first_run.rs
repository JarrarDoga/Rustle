//! First run (D-Empty): pick your main playlist; nothing playing yet.
//!
//! Integrator stub. The `onboarding` worker owns this file.

use egui::{RichText, Ui};

use crate::state::{Command, Session, State};
use crate::theme;
use crate::views::{Out, cmd};

/// Whether the content region shows the first-run picker instead of the
/// Collection. `app.rs` asks every frame. Must not flash on a normal start
/// before the saved main playlist id arrives from the store.
pub fn wanted(state: &State) -> bool {
    state.session == Session::LoggedIn && state.main.is_none() && state.library.ready().is_some()
}

pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    let p = theme::pal();
    ui.label(
        RichText::new("Pick your main playlist")
            .font(theme::page_title())
            .color(p.text_1),
    );
    let Some(playlists) = state.library.ready() else {
        return;
    };
    for pl in playlists.iter() {
        if ui.button(&pl.name).clicked() {
            cmd(out, Command::SetMainPlaylist(pl.id.clone()));
        }
    }
}
