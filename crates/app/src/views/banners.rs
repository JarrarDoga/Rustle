//! Banners (D-States): offline, session expired, and one-line notices; the
//! guest bar. 40 px, at the top of the window.
//!
//! Integrator stub (the old banner). The `onboarding` worker owns this file.

use egui::{Frame, Margin, Panel, RichText, Ui};

use crate::state::{Command, Session, State};
use crate::theme;
use crate::views::{Action, Out, cmd};

/// Draws the banner and the guest bar as top panels of the window, when
/// there is one. Call before the other panels.
pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    let p = theme::pal();
    if let Some(banner) = &state.banner {
        Panel::top("banner")
            .exact_size(theme::BANNER_H)
            .frame(
                Frame::NONE
                    .fill(p.surface_3)
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(banner).font(theme::row()).color(p.text_1));
                    if ui.button("Dismiss").clicked() {
                        cmd(out, Command::DismissBanner);
                    }
                });
            });
    }
    if state.session != Session::LoggedIn {
        Panel::top("guest")
            .exact_size(theme::BANNER_H)
            .frame(
                Frame::NONE
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("Not logged in").color(p.text_2));
                    if ui.link("Log in").clicked() {
                        out.push(Action::Guest(false));
                    }
                });
            });
    }
}
