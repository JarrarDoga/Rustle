//! The login screen. Owned by the auth worker in batch 2.

use egui::{RichText, Ui, vec2};

use crate::state::State;
use crate::theme;

use super::Out;

/// Shown while `state.session` is not `LoggedIn`. Sends `Command::Login` with
/// the pasted text; the backend validates it and updates `state.session`.
pub fn login(ui: &mut Ui, _state: &State, cookies: &mut String, _out: &mut Out) {
    ui.add_space((ui.available_height() * 0.18).max(20.0));
    ui.vertical_centered(|ui| {
        ui.set_max_width(520.0);
        ui.label(
            RichText::new("Rustle")
                .size(36.0)
                .strong()
                .color(theme::TEXT),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Paste the Cookie header or a cookies.txt export from music.youtube.com.",
            )
            .color(theme::TEXT_DIM),
        );
        ui.add_space(14.0);
        ui.add(
            egui::TextEdit::multiline(cookies)
                .hint_text("SAPISID=…; __Secure-3PSID=…")
                .desired_rows(6)
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 8.0)),
        );
        ui.add_space(12.0);
        ui.add_enabled(false, egui::Button::new("Connect"))
            .on_disabled_hover_text("Login is not wired up yet");
        ui.add_space(8.0);
        ui.label(
            RichText::new("Login is not wired up yet. Run `rustle --demo` to try the app.")
                .size(12.5)
                .color(theme::TEXT_FAINT),
        );
    });
}
