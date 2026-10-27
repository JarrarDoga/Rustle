//! The login screen (D-Login, D-LoginError): paste a `Cookie` header or a
//! cookies.txt export.
//!
//! The `onboarding` worker owns this file.

use egui::text::LayoutJob;
use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key, Margin, Modifiers, Rect,
    ScrollArea, Sense, Stroke, StrokeKind, TextFormat, Ui, vec2,
};

use crate::auth::normalize_cookie_input;
use crate::icons;
use crate::state::{COOKIES_REJECTED, Command, Session, State};
use crate::theme;

use super::{Action, Out};

/// Inner padding of the sign-in card.
const PAD: f32 = 32.0;
/// The cookie field's height, border included (D-Login).
const FIELD_BOX_H: f32 = 112.0;
/// The sign-in button is 4 px taller than a normal button (D-Login).
const SIGN_IN_H: f32 = theme::BUTTON_H + theme::SPACE_4;

/// Shown under the field when the pasted text has no `SAPISID` cookie, which
/// YouTube Music needs to sign requests. Checked here, so the cookie never
/// leaves the machine when it cannot work.
const MISSING_SAPISID: &str = "That cookie is missing SAPISID, so YouTube Music won't accept it. \
     Copy the whole Cookie header again from a signed-in tab.";
const REJECTED: &str = "YouTube Music rejected that cookie. Copy the whole Cookie header again \
     from a signed-in tab.";

const STEPS: [&[(&str, bool)]; 4] = [
    &[("Open music.youtube.com in your browser and sign in.", false)],
    &[(
        "Press F12, open the Network tab, then reload the page.",
        false,
    )],
    &[
        (
            "Click any request to music.youtube.com and copy the whole ",
            false,
        ),
        ("Cookie", true),
        (" request header.", false),
    ],
    &[("Paste it below.", false)],
];

/// Shown while `state.session` is not `LoggedIn`. Sends `Command::Login` with
/// the pasted text; the backend validates it and updates `state.session`.
pub fn login(ui: &mut Ui, state: &State, cookies: &mut String, out: &mut Out) {
    let logging_in = state.session == Session::LoggingIn;
    let card_id = ui.scope_id().with("sign-in-card");
    // Center the card vertically using last frame's height.
    let last_h: Option<f32> = ui.data(|d| d.get_temp(card_id));
    let top = last_h.map_or(0.0, |h| ((ui.available_height() - h) / 2.0).max(0.0));

    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(top);
            let width = theme::SIGN_IN_CARD_W.min(ui.available_width());
            let left = ui.max_rect().left() + ((ui.available_width() - width) / 2.0).max(0.0);
            let rect = Rect::from_min_size(egui::pos2(left, ui.cursor().top()), vec2(width, 0.0));
            let card = ui
                .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                    Frame::NONE
                        .stroke(theme::hairline())
                        .corner_radius(theme::RADIUS_CONTROL)
                        .inner_margin(Margin::same(PAD as i8))
                        .show(ui, |ui| {
                            ui.set_width(width - 2.0 * PAD);
                            card(ui, state, cookies, logging_in, out);
                        })
                        .response
                })
                .inner;
            let h = card.rect.height();
            if last_h.is_none_or(|old| (old - h).abs() > 0.5) {
                ui.data_mut(|d| d.insert_temp(card_id, h));
                // Re-center once the height is known (first frame, error shown).
                ui.ctx().request_repaint();
            }
        });
}

fn card(ui: &mut Ui, state: &State, cookies: &mut String, logging_in: bool, out: &mut Out) {
    let p = theme::pal();
    let gap = theme::SPACE_20;
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    // Logo mark and name.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SPACE_8;
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        logo(ui, rect);
        ui.label(
            egui::RichText::new("rustle")
                .font(theme::section())
                .color(p.text_1),
        );
    });
    ui.add_space(gap);

    ui.label(
        egui::RichText::new("Sign in")
            .font(theme::page_title())
            .color(p.text_1),
    );
    ui.add_space(6.0);
    wrapped(
        ui,
        "Rustle signs in with the cookie from a browser where you're already logged in to \
         YouTube Music.",
        body(),
        p.text_2,
    );
    ui.add_space(gap);

    for (i, step) in STEPS.iter().enumerate() {
        if i > 0 {
            ui.add_space(theme::SPACE_8);
        }
        step_row(ui, i + 1, step);
    }
    ui.add_space(gap);

    // Ctrl+Enter submits. Consumed before the field so it adds no newline.
    let ctrl_enter = !logging_in && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));

    ui.label(
        egui::RichText::new("Cookie")
            .font(label_font())
            .color(p.text_1),
    );
    ui.add_space(6.0);
    let field_id = Id::NULL.with("rustle-cookie-field");
    let local_error_id = field_id.with("local-error");
    let local_error = ui.data(|d| d.get_temp::<bool>(local_error_id).unwrap_or(false));
    let error = if local_error {
        Some(MISSING_SAPISID.to_owned())
    } else {
        match &state.session {
            Session::Failed(message) => Some(failure_text(message)),
            _ => None,
        }
    };
    if cookie_field(ui, field_id, cookies, error.is_some(), logging_in) {
        // A new paste replaces the old check.
        ui.data_mut(|d| d.remove::<bool>(local_error_id));
    }
    ui.add_space(6.0);
    match &error {
        Some(message) => error_row(ui, message),
        None => wrapped(
            ui,
            "Stored only on this computer. It stops working if you sign out of YouTube in that \
             browser.",
            regular(11.0),
            p.text_3,
        ),
    }
    ui.add_space(gap);

    let normalized = normalize_cookie_input(cookies);
    let can_submit = !normalized.is_empty() && !logging_in;
    let label = if logging_in {
        "Signing in…"
    } else {
        "Sign in"
    };
    let clicked = primary_button(ui, label, can_submit).clicked();
    if can_submit && (clicked || ctrl_enter) {
        if has_sapisid(&normalized) {
            ui.data_mut(|d| d.remove::<bool>(local_error_id));
            out.push(Action::Cmd(Command::Login(normalized)));
        } else {
            ui.data_mut(|d| d.insert_temp(local_error_id, true));
        }
    }

    // Search and public playlists work without an account.
    ui.add_space(theme::SPACE_12);
    ui.vertical_centered(|ui| {
        let text = egui::RichText::new("Continue without logging in").font(theme::secondary());
        let resp = ui.add(egui::Label::new(text.color(p.text_2)).sense(Sense::click()));
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() {
            let r = resp.rect;
            ui.painter().hline(
                r.x_range(),
                r.bottom(),
                Stroke::new(theme::STROKE_W, p.text_2),
            );
        }
        if resp.clicked() {
            out.push(Action::Guest(true));
        }
    });
}

/// The Rustle mark: a leaf, round on two opposite corners.
fn logo(ui: &Ui, rect: Rect) {
    let round = (rect.height() / 2.0) as u8;
    let radius = CornerRadius {
        nw: round,
        ne: 1,
        sw: 1,
        se: round,
    };
    ui.painter().rect_filled(rect, radius, theme::pal().text_1);
}

/// One numbered how-to step: a 20 px badge, then the wrapped text.
fn step_row(ui: &mut Ui, n: usize, parts: &[(&str, bool)]) {
    let p = theme::pal();
    let badge = 20.0;
    let gap = 10.0;
    let width = ui.available_width();
    let mut job = LayoutJob::default();
    for (text, strong) in parts {
        let color = if *strong { p.text_1 } else { p.text_2 };
        job.append(text, 0.0, TextFormat::simple(body(), color));
    }
    job.wrap.max_width = width - badge - gap;
    let galley = ui.painter().layout_job(job);
    let height = galley.size().y.max(badge);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let center = egui::pos2(rect.left() + badge / 2.0, rect.top() + badge / 2.0);
    ui.painter().circle_filled(center, badge / 2.0, p.surface_3);
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        n.to_string(),
        theme::caption(),
        p.text_1,
    );
    // The first line of text sits centered on the badge.
    let first_line = galley.rows.first().map_or(badge, |r| r.height());
    let y = rect.top() + ((badge - first_line) / 2.0).max(0.0);
    ui.painter()
        .galley(egui::pos2(rect.left() + badge + gap, y), galley, p.text_2);
}

/// The multiline cookie field: empty with placeholder text, stroke-2 border,
/// danger border on an error, 2 px accent ring while focused. Returns
/// whether the text changed.
fn cookie_field(ui: &mut Ui, id: Id, cookies: &mut String, error: bool, logging_in: bool) -> bool {
    let p = theme::pal();
    let focused = ui.memory(|m| m.has_focus(id));
    let stroke = if error {
        Stroke::new(theme::STROKE_W, p.danger)
    } else if focused {
        Stroke::new(theme::FOCUS_W, theme::accent())
    } else {
        Stroke::new(theme::STROKE_W, p.stroke_2)
    };
    // 10 px padding inside the 1 px border (D-Login). The border is painted
    // over the frame, so the 2 px ring does not move the layout.
    let margin = 11.0;
    let inner = FIELD_BOX_H - 2.0 * margin;
    // No blinking cursor: it would repaint twice a second while focused.
    ui.visuals_mut().text_cursor.blink = false;
    let framed = Frame::NONE
        .inner_margin(Margin::same(margin as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt("cookie-scroll")
                .min_scrolled_height(inner)
                .max_height(inner)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let resp = ui.add_enabled(
                        !logging_in,
                        egui::TextEdit::multiline(cookies)
                            .id(id)
                            .frame(Frame::NONE)
                            .font(theme::secondary())
                            .text_color(p.text_1)
                            .desired_width(f32::INFINITY)
                            // 1 px under the box, so the scrollbar shows
                            // only for text that overflows it.
                            .min_size(vec2(0.0, inner - 1.0)),
                    );
                    // Painted, not `hint_text`: egui puts the cursor after
                    // a hint that fills the width.
                    if cookies.is_empty() {
                        ui.painter().text(
                            resp.rect.min,
                            Align2::LEFT_TOP,
                            "SAPISID=…; __Secure-3PAPISID=…; HSID=…",
                            theme::secondary(),
                            p.text_3,
                        );
                    }
                    resp
                })
                .inner
        });
    ui.painter().rect_stroke(
        framed.response.rect,
        theme::RADIUS_CONTROL,
        stroke,
        StrokeKind::Inside,
    );
    let resp = framed.inner;
    resp.changed()
}

fn error_row(ui: &mut Ui, message: &str) {
    let p = theme::pal();
    let icon = 14.0;
    let gap = theme::SPACE_8;
    let width = ui.available_width();
    let mut job = LayoutJob::single_section(
        message.to_owned(),
        TextFormat::simple(regular(12.0), p.danger),
    );
    job.wrap.max_width = width - icon - gap;
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
    let first_line = galley.rows.first().map_or(icon, |r| r.height());
    ui.painter().text(
        egui::pos2(rect.left() + icon / 2.0, rect.top() + first_line / 2.0),
        Align2::CENTER_CENTER,
        icons::WARNING_CIRCLE,
        theme::icon(icon),
        p.danger,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + icon + gap, rect.top()),
        galley,
        p.danger,
    );
}

/// Full-width primary button: accent fill and on-accent text when enabled,
/// surface-3 fill and text-3 when not.
fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let p = theme::pal();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(ui.available_width(), SIGN_IN_H), sense);
    if enabled {
        resp = resp
            .on_hover_text("Ctrl+Enter")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let (fill, text) = if !enabled {
        (p.surface_3, p.text_3)
    } else if resp.is_pointer_button_down_on() {
        (theme::accent_pressed(), p.on_accent)
    } else if resp.hovered() {
        (theme::accent_hover(), p.on_accent)
    } else {
        (theme::accent(), p.on_accent)
    };
    ui.painter().rect_filled(rect, theme::RADIUS_CONTROL, fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::row_strong(),
        text,
    );
    resp
}

fn wrapped(ui: &mut Ui, text: &str, font: FontId, color: Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).wrap());
}

/// What a failed login shows. The reducer's terse messages read as a
/// sentence here; others are shown as they are.
fn failure_text(message: &str) -> String {
    if message == COOKIES_REJECTED {
        return REJECTED.to_owned();
    }
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => REJECTED.to_owned(),
    }
}

/// Whether `normalized` (from [`normalize_cookie_input`]) has the `SAPISID`
/// cookie rustypipe signs requests with: a `SAPISID=` pair in a header, or a
/// `SAPISID` line in a cookies.txt export.
fn has_sapisid(normalized: &str) -> bool {
    let txt_line = |l: &str| l.split('\t').nth(5).is_some_and(|name| name == "SAPISID");
    normalized.split(';').any(|pair| {
        pair.trim()
            .strip_prefix("SAPISID=")
            .is_some_and(|v| !v.is_empty())
    }) || normalized.lines().any(txt_line)
}

/// Inter 400 at `size`. Body copy on this screen is 400 weight, which the
/// theme has only at 12 px ([`theme::secondary`]).
fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// 13 / 400: explanation and step text.
fn body() -> FontId {
    regular(13.0)
}

/// 12 / 500: the field label.
fn label_font() -> FontId {
    FontId::new(12.0, theme::row().family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_needs_sapisid() {
        assert!(!has_sapisid("x=1"));
        assert!(!has_sapisid("__Secure-3PAPISID=abc; HSID=def"));
        assert!(!has_sapisid("SAPISID=; HSID=def"));
        assert!(has_sapisid("HSID=def; SAPISID=abc/xyz"));
        assert!(has_sapisid(&normalize_cookie_input(
            "Cookie: SAPISID=abc;\nHSID=x"
        )));
    }

    #[test]
    fn cookies_txt_needs_sapisid() {
        let txt = "# Netscape HTTP Cookie File\n\
                   .youtube.com\tTRUE\t/\tTRUE\t0\tHSID\tdef\n\
                   .youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tabc\n";
        assert!(has_sapisid(&normalize_cookie_input(txt)));
        let without = "# Netscape HTTP Cookie File\n\
                       .youtube.com\tTRUE\t/\tTRUE\t0\tHSID\tdef\n";
        assert!(!has_sapisid(&normalize_cookie_input(without)));
    }

    #[test]
    fn failure_messages_read_as_sentences() {
        assert_eq!(failure_text(COOKIES_REJECTED), REJECTED);
        assert_eq!(
            failure_text("could not reach YouTube Music: timeout"),
            "Could not reach YouTube Music: timeout"
        );
    }
}
