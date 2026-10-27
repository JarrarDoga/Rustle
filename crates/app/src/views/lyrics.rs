//! The lyrics drawer: the queue drawer's size, border and shadow (D-Queue),
//! opened by `Command::ShowLyrics(true)`. `app.rs` places it; [`show`]
//! fills it.
//!
//! Integrator stub. The `lyrics` worker owns this file: current line in
//! text-1 600, others text-3, the source at the bottom, scroll to keep the
//! current line in view, click a synced line to seek, and a repaint timed
//! to the next line (never a fixed short interval).

use std::time::Instant;

use egui::{RichText, Ui};

use crate::state::{Command, Loadable, State};
use crate::theme;
use crate::views::{Out, cmd};

pub fn show(ui: &mut Ui, state: &State, _now: Instant, out: &mut Out) {
    let p = theme::pal();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Lyrics")
                .font(theme::section())
                .color(p.text_1),
        );
        if ui.button("Close").clicked() {
            cmd(out, Command::ShowLyrics(false));
        }
    });
    let text = match state.lyrics.as_ref().map(|(_, l)| l) {
        None => "Nothing playing".to_owned(),
        Some(Loadable::Ready(Some(lyrics))) => lyrics
            .lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Loadable::Ready(None)) => "No lyrics for this song".to_owned(),
        Some(Loadable::Failed(e)) => e.clone(),
        Some(_) => "Loading…".to_owned(),
    };
    ui.label(RichText::new(text).font(theme::body()).color(p.text_2));
}
