//! Banners (D-States): offline, session expired, and one-line notices; the
//! guest bar. 40 px, at the top of the window.
//!
//! The `onboarding` worker owns this file.

use egui::{Align, Align2, Frame, Layout, Margin, Panel, Rect, RichText, Sense, Stroke, Ui, vec2};

use crate::icons;
use crate::state::{Command, OFFLINE, SESSION_EXPIRED, Session, State};
use crate::theme;
use crate::views::{Action, Out, cmd};

/// What a banner is, from the reducer's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// [`OFFLINE`]: neutral, wifi-slash icon.
    Offline,
    /// [`SESSION_EXPIRED`]: danger icon and one "Paste new cookie" action.
    Expired,
    /// Any other one-line notice: neutral, with Dismiss.
    Notice,
}

fn kind(text: &str) -> Kind {
    match text {
        OFFLINE => Kind::Offline,
        SESSION_EXPIRED => Kind::Expired,
        _ => Kind::Notice,
    }
}

/// Draws the banner and the guest bar as top panels of the window, when
/// there are any. Call before the other panels.
pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    banner(ui, state, out);
    guest_bar(ui, state, out);
}

/// The one banner from `state.banner`, if any.
pub fn banner(ui: &mut Ui, state: &State, out: &mut Out) {
    let Some(text) = &state.banner else {
        return;
    };
    let p = theme::pal();
    let kind = kind(text);
    bar(ui, "banner", p.surface_2, |ui| {
        let (glyph, color, message) = match kind {
            Kind::Offline => (
                icons::WIFI_SLASH,
                p.text_2,
                "You're offline. Rustle could not reach YouTube Music; songs already buffered \
                 keep playing.",
            ),
            Kind::Expired => (
                icons::WARNING_CIRCLE,
                p.danger,
                "Your YouTube Music session expired. Paste a new cookie to sign in again.",
            ),
            Kind::Notice => (icons::INFO, p.text_2, text.as_str()),
        };
        ui.label(RichText::new(glyph).font(theme::icon(16.0)).color(color));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            match kind {
                // Not `Command::Logout`: the reducer has already logged the
                // session out, and Logout would also drop the cached library
                // and main playlist and clear the keyring. Leaving guest mode
                // makes app.rs show the login screen with the cache intact.
                Kind::Expired => {
                    if primary_small(ui, "Paste new cookie").clicked() {
                        out.push(Action::Guest(false));
                    }
                }
                Kind::Offline | Kind::Notice => {
                    if quiet_button(ui, "Dismiss").clicked() {
                        cmd(out, Command::DismissBanner);
                    }
                }
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(message).font(body()).color(p.text_1))
                        .truncate(),
                );
            });
        });
    });
}

/// "Not logged in · Log in" while browsing as a guest.
pub fn guest_bar(ui: &mut Ui, state: &State, out: &mut Out) {
    if state.session == Session::LoggedIn {
        return;
    }
    let p = theme::pal();
    bar(ui, "guest", p.bg, |ui| {
        ui.label(
            RichText::new("Not logged in")
                .font(theme::secondary())
                .color(p.text_2),
        );
        ui.label(RichText::new("·").font(theme::secondary()).color(p.text_3));
        if link(ui, "Log in").clicked() {
            out.push(Action::Guest(false));
        }
    });
}

/// A 40 px top panel with a hairline under it; `add` lays out left to right.
fn bar(ui: &mut Ui, id: &str, fill: egui::Color32, add: impl FnOnce(&mut Ui)) {
    Panel::top(id)
        .exact_size(theme::BANNER_H)
        .resizable(false)
        .show_separator_line(true)
        .frame(
            Frame::NONE
                .fill(fill)
                .inner_margin(Margin::symmetric(theme::SPACE_20 as i8, 0)),
        )
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.horizontal_centered(add);
        });
}

/// A 28 px primary button: accent fill, on-accent text, 12 / 600.
fn primary_small(ui: &mut Ui, label: &str) -> egui::Response {
    let p = theme::pal();
    let font = egui::FontId::new(12.0, theme::row_strong().family);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, p.on_accent);
    let size = vec2(galley.size().x + 2.0 * theme::SPACE_12, theme::ICON_BUTTON);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        theme::accent_pressed()
    } else if resp.hovered() {
        theme::accent_hover()
    } else {
        theme::accent()
    };
    ui.painter().rect_filled(rect, theme::RADIUS_CONTROL, fill);
    paint_centered(ui, rect, galley, p.on_accent);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A 28 px text button: text-2, surface-2 behind it on hover (D-States
/// "Retry now").
fn quiet_button(ui: &mut Ui, label: &str) -> egui::Response {
    let p = theme::pal();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::secondary(), p.text_2);
    let size = vec2(galley.size().x + 2.0 * 10.0, theme::ICON_BUTTON);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let color = if resp.hovered() {
        ui.painter()
            .rect_filled(rect, theme::RADIUS_CONTROL, p.surface_3);
        p.text_1
    } else {
        p.text_2
    };
    paint_centered(ui, rect, galley, color);
    resp
}

fn link(ui: &mut Ui, label: &str) -> egui::Response {
    let p = theme::pal();
    let resp = ui
        .add(
            egui::Label::new(
                RichText::new(label)
                    .font(theme::secondary())
                    .color(p.text_1),
            )
            .sense(Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        let r = resp.rect;
        ui.painter().hline(
            r.x_range(),
            r.bottom(),
            Stroke::new(theme::STROKE_W, p.text_1),
        );
    }
    resp
}

fn paint_centered(ui: &Ui, rect: Rect, galley: std::sync::Arc<egui::Galley>, color: egui::Color32) {
    let pos = Align2::CENTER_CENTER
        .align_size_within_rect(galley.size(), rect)
        .min;
    ui.painter().galley(pos, galley, color);
}

/// 13 / 400: banner text. The theme has 400 weight only at 12 px.
fn body() -> egui::FontId {
    egui::FontId::new(13.0, egui::FontFamily::Proportional)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{COOKIES_REJECTED, NEEDS_LOGIN};

    #[test]
    fn classifies_banners_by_text() {
        assert_eq!(kind(OFFLINE), Kind::Offline);
        assert_eq!(kind(SESSION_EXPIRED), Kind::Expired);
        assert_eq!(kind("Could not play this track: 403"), Kind::Notice);
        assert_eq!(kind(COOKIES_REJECTED), Kind::Notice);
        assert_eq!(kind(NEEDS_LOGIN), Kind::Notice);
        // Only the exact texts count: a notice that quotes one is a notice.
        assert_eq!(kind(&format!("Library: {SESSION_EXPIRED}")), Kind::Notice);
    }
}
