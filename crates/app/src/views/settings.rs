//! The settings popover, opened from the rail's settings button: Autoplay,
//! Accent from album art, and the theme picker (`views::theme_picker`).
//!
//! Integrator stub. The `settings` worker owns this file.

use egui::{Popup, Response, Ui};

use crate::state::{Command, SettingChange, State};
use crate::views::{Out, View, cmd, theme_picker};

/// Draws the popover under `anchor` (the rail button) while it is open.
pub fn show(ui: &mut Ui, anchor: &Response, state: &State, _view: &mut View, out: &mut Out) {
    Popup::menu(anchor).show(|ui| {
        let s = &state.settings;
        let mut autoplay = s.autoplay;
        if ui.checkbox(&mut autoplay, "Autoplay").changed() {
            cmd(out, Command::SetSetting(SettingChange::Autoplay(autoplay)));
        }
        let mut accent = s.accent_from_art;
        if ui.checkbox(&mut accent, "Accent from album art").changed() {
            cmd(
                out,
                Command::SetSetting(SettingChange::AccentFromArt(accent)),
            );
        }
        theme_picker::show(ui, state, out);
    });
    let _ = ui;
}
