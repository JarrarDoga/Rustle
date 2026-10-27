//! The settings popover, opened from the rail's settings button: Autoplay,
//! Accent from album art, and the theme picker (`views::theme_picker`).
//!
//! D look: a 280 px panel to the right of the button, bottom-aligned with
//! it so it sits above the player bar; bg fill, stroke-2 hairline, radius 4
//! and no shadow (the queue drawer has the only one). Each switch row has
//! its label in 13/400 text-1 and a hint line in 11/400 text-3 under it, so
//! it is 44 px tall (the D two-line row) instead of 32.

use egui::{
    Align2, CornerRadius, CursorIcon, Frame, Margin, Popup, PopupCloseBehavior, Pos2, Rect,
    RectAlign, Response, Role, Sense, Stroke, Ui, WidgetInfo, vec2,
};

use crate::state::{Command, SettingChange, State};
use crate::theme;
use crate::views::{Out, View, cmd, theme_picker};

const WIDTH: f32 = 280.0;
const PAD_X: f32 = theme::SPACE_12;
const SWITCH_ROW_H: f32 = 44.0;
const SWITCH_W: f32 = 28.0;
const SWITCH_H: f32 = 16.0;
const KNOB_R: f32 = 6.0;

/// Draws the popover beside `anchor` (the rail button) while it is open. A
/// click on the button toggles it; a click outside or Escape closes it.
pub fn show(ui: &mut Ui, anchor: &Response, state: &State, _view: &mut View, out: &mut Out) {
    let p = theme::pal();
    // Past the rail's right edge, not over it.
    let gap = (ui.max_rect().right() - anchor.rect.right()).max(0.0) + theme::SPACE_8;
    let frame = Frame::NONE
        .fill(p.bg)
        .stroke(Stroke::new(theme::STROKE_W, p.stroke_2))
        .corner_radius(CornerRadius::same(theme::RADIUS_CONTROL))
        .inner_margin(Margin::symmetric(0, theme::SPACE_4 as i8));
    Popup::menu(anchor)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .align(RectAlign::RIGHT_END)
        .align_alternatives(&[RectAlign::RIGHT_END, RectAlign::RIGHT_START])
        .gap(gap)
        .width(WIDTH)
        .frame(frame)
        .show(|ui| {
            ui.set_width(WIDTH - 2.0 * theme::STROKE_W);
            ui.spacing_mut().item_spacing.y = 0.0;
            let s = &state.settings;
            if switch_row(
                ui,
                "Autoplay",
                "Plays radio when the queue runs out",
                s.autoplay,
            ) {
                cmd(
                    out,
                    Command::SetSetting(SettingChange::Autoplay(!s.autoplay)),
                );
            }
            if switch_row(
                ui,
                "Accent from album art",
                "Off: the theme's plain accent",
                s.accent_from_art,
            ) {
                cmd(
                    out,
                    Command::SetSetting(SettingChange::AccentFromArt(!s.accent_from_art)),
                );
            }
            theme_section(ui, state, out);
        });
}

/// One setting with a switch; the whole row is the click target. Returns
/// true when clicked.
fn switch_row(ui: &mut Ui, label: &str, hint: &str, on: bool) -> bool {
    let p = theme::pal();
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), SWITCH_ROW_H), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, true, on, label));
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, p.surface_2);
    }
    let left = rect.left() + PAD_X;
    painter.text(
        Pos2::new(left, rect.top() + 7.0),
        Align2::LEFT_TOP,
        label,
        theme::body(),
        p.text_1,
    );
    painter.text(
        Pos2::new(left, rect.top() + 7.0 + 17.0),
        Align2::LEFT_TOP,
        hint,
        theme::small(),
        p.text_3,
    );
    let track = Rect::from_center_size(
        Pos2::new(rect.right() - PAD_X - SWITCH_W / 2.0, rect.center().y),
        vec2(SWITCH_W, SWITCH_H),
    );
    switch(ui, track, on);
    resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// The 28 x 16 switch, no animation: accent track with an on-accent knob on
/// the right when on; a surface-3 track (as in D-Queue) with a text-2 knob on the left when
/// off.
fn switch(ui: &Ui, rect: Rect, on: bool) {
    let p = theme::pal();
    let (track, knob, knob_x) = if on {
        (theme::accent(), p.on_accent, rect.right() - SWITCH_H / 2.0)
    } else {
        (p.surface_3, p.text_2, rect.left() + SWITCH_H / 2.0)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((SWITCH_H / 2.0) as u8), track);
    painter.circle_filled(Pos2::new(knob_x, rect.center().y), KNOB_R, knob);
}

/// A hairline, the "Theme" caption, then the picker.
fn theme_section(ui: &mut Ui, state: &State, out: &mut Out) {
    let p = theme::pal();
    let (rule, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::SPACE_8 + 1.0),
        Sense::hover(),
    );
    ui.painter().hline(
        rule.x_range(),
        rule.top() + theme::SPACE_4 + 0.5,
        theme::hairline(),
    );
    let (cap, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().text(
        Pos2::new(cap.left() + PAD_X, cap.center().y),
        Align2::LEFT_CENTER,
        "Theme",
        theme::caption(),
        p.text_3,
    );
    Frame::NONE
        .inner_margin(Margin {
            left: PAD_X as i8,
            right: PAD_X as i8,
            top: 0,
            bottom: theme::SPACE_8 as i8,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = theme::SPACE_4;
            theme_picker::show(ui, state, out);
        });
}
