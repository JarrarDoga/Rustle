//! Colors and the egui style: a flat dark theme with one accent color.

use egui::epaint::Hsva;
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, ThemePreference, Visuals,
};

/// YouTube Music red, softened.
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x4e, 0x45);
pub const BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x07, 0x07, 0x07);
pub const BAR: Color32 = Color32::from_rgb(0x0b, 0x0b, 0x0b);
pub const SURFACE: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);
pub const HOVER: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
pub const SELECTED: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
pub const TEXT: Color32 = Color32::from_rgb(0xf5, 0xf5, 0xf5);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xa8, 0xa8, 0xa8);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6e, 0x6e, 0x6e);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(ThemePreference::Dark);
    ctx.style_mut_of(Theme::Dark, |style| {
        let v: &mut Visuals = &mut style.visuals;
        *v = Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = Color32::from_rgb(0x24, 0x24, 0x24);
        v.faint_bg_color = SURFACE;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.slider_trailing_fill = true;
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(6);
        v.window_stroke = Stroke::new(1.0, HOVER);

        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x22, 0x22, 0x22));
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        for (state, fill, fg) in [
            (&mut w.inactive, Color32::from_rgb(0x2a, 0x2a, 0x2a), TEXT),
            (&mut w.hovered, Color32::from_rgb(0x36, 0x36, 0x36), TEXT),
            (&mut w.active, Color32::from_rgb(0x44, 0x44, 0x44), TEXT),
            (&mut w.open, Color32::from_rgb(0x2a, 0x2a, 0x2a), TEXT),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.fg_stroke = Stroke::new(1.5, fg);
            state.corner_radius = CornerRadius::same(6);
            state.expansion = 0.0;
        }
        // Slider rails use the inactive fill; keep them subtle.
        w.inactive.bg_fill = Color32::from_rgb(0x4a, 0x4a, 0x4a);

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(8.0, 6.0);
        s.button_padding = egui::vec2(12.0, 6.0);
        s.interact_size.y = 28.0;

        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(11.5, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(26.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

/// A stable muted color for a placeholder cover, picked from `key`.
pub fn art_color(key: &str) -> Color32 {
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    let hue = (hash % 360) as f32 / 360.0;
    Hsva::new(hue, 0.45, 0.42, 1.0).into()
}
