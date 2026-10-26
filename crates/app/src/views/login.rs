//! The login screen: paste a `Cookie` header or a cookies.txt export.

use egui::{Key, Modifiers, RichText, Ui, vec2};

use crate::auth::normalize_cookie_input;
use crate::state::{Command, Session, State};
use crate::theme;

use super::{Action, Out};

/// Shown while `state.session` is not `LoggedIn`. Sends `Command::Login` with
/// the pasted text; the backend validates it and updates `state.session`.
pub fn login(ui: &mut Ui, state: &State, cookies: &mut String, out: &mut Out) {
    let logging_in = state.session == Session::LoggingIn;
    let normalized = normalize_cookie_input(cookies);
    let can_submit = !normalized.is_empty() && !logging_in;

    ui.add_space((ui.available_height() * 0.12).max(20.0));
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
        ui.add_enabled(
            !logging_in,
            egui::TextEdit::multiline(cookies)
                .hint_text("SAPISID=…; __Secure-3PSID=…")
                .desired_rows(6)
                .desired_width(f32::INFINITY)
                .margin(vec2(10.0, 8.0)),
        );
        if let Session::Failed(message) = &state.session {
            ui.add_space(6.0);
            ui.label(RichText::new(message).color(theme::ACCENT));
        }
        ui.add_space(12.0);

        let clicked = ui
            .horizontal(|ui| {
                // Center the button (and the spinner next to it).
                let width = if logging_in { 120.0 } else { 80.0 };
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                let clicked = ui
                    .add_enabled(can_submit, egui::Button::new("Connect"))
                    .on_hover_text("Ctrl+Enter")
                    .clicked();
                if logging_in {
                    // Spinner repaints every frame, but only while the login
                    // request is in flight.
                    ui.add(egui::Spinner::new());
                }
                clicked
            })
            .inner;
        let ctrl_enter =
            can_submit && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        if clicked || ctrl_enter {
            out.push(Action::Cmd(Command::Login(normalized)));
        }

        ui.add_space(18.0);
        instructions(ui);
    });
}

fn instructions(ui: &mut Ui) {
    let step = |ui: &mut Ui, text: &str| {
        ui.label(RichText::new(text).size(12.5).color(theme::TEXT_FAINT));
    };
    step(
        ui,
        "In a browser that is logged in to music.youtube.com, open DevTools (F12) › Network, \
         click any request to music.youtube.com, and copy the value of its Cookie request header.",
    );
    ui.add_space(4.0);
    step(
        ui,
        "Or export cookies.txt for music.youtube.com with a browser extension and paste the file.",
    );
    ui.add_space(4.0);
    step(
        ui,
        "Rustle keeps the cookies in the system keyring. Run `rustle --demo` to try the app without an account.",
    );
}
