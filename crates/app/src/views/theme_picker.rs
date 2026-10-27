//! The theme picker inside the settings popover: one swatch row per
//! `theme::PALETTES` entry; picking one sends
//! `Command::SetSetting(SettingChange::Theme(name))`. `app.rs` applies the
//! palette when `state.settings.theme` changes.
//!
//! Integrator stub. The `themes` worker owns this file and the palettes in
//! `theme.rs`.

use egui::Ui;

use crate::state::{Command, SettingChange, State};
use crate::theme;
use crate::views::{Out, cmd};

pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    for p in theme::PALETTES {
        if ui
            .selectable_label(state.settings.theme == p.name, p.name)
            .clicked()
        {
            cmd(
                out,
                Command::SetSetting(SettingChange::Theme(p.name.to_owned())),
            );
        }
    }
}
