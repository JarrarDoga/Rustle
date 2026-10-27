//! Direction D tokens (`docs/design/png/D-Tokens.png`): colors, type, sizes,
//! spacing and radii, plus the egui style built from them.
//!
//! Colors come from a [`Palette`], read with [`pal`]. Views never hard-code a
//! neutral: Batch 4 adds more dark and medium palettes, and switching one
//! swaps every neutral at once. A palette swaps neutrals only. Album art stays
//! the only source of hue: [`accent`] is the now-playing art's accent, and a
//! palette's `fallback_accent` is used only when nothing is playing or the
//! "accent from album art" setting is off. Accent means playing, active or
//! primary action; never use it for decoration.
//!
//! Sizes, spacing, radii and text styles do not change with the palette, so
//! they are plain constants.

// A token table: not every token has a user at every commit.
#![allow(dead_code)]

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use egui::epaint::Hsva;
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme,
    ThemePreference, Visuals,
};

use crate::art_color;

// ------------------------------------------------------------------ palette

/// The neutral colors of one theme. Names follow D-Tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Shown in the theme picker.
    pub name: &'static str,
    /// Every region, one fill.
    pub bg: Color32,
    /// Hover.
    pub surface_2: Color32,
    /// Selected, active, chips.
    pub surface_3: Color32,
    /// Region edges, table rules.
    pub stroke: Color32,
    /// Inputs, secondary button borders.
    pub stroke_2: Color32,
    /// Primary text.
    pub text_1: Color32,
    /// Secondary text, icons.
    pub text_2: Color32,
    /// Captions, index numbers.
    pub text_3: Color32,
    /// Text and icons drawn on an accent fill.
    pub on_accent: Color32,
    /// Errors, expired session.
    pub danger: Color32,
    /// The accent when no art accent applies. D uses `text_1`.
    pub fallback_accent: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Direction D: B neutrals, C adaptive accent. The default.
pub const GRAPHITE: Palette = Palette {
    name: "Graphite",
    bg: hex(0x111214),
    surface_2: hex(0x1A1B1E),
    surface_3: hex(0x232428),
    stroke: hex(0x26272B),
    stroke_2: hex(0x3A3B40),
    text_1: hex(0xECECEE),
    text_2: hex(0x9C9DA3),
    text_3: hex(0x82838A),
    on_accent: hex(0x111214),
    danger: hex(0xF07A6A),
    fallback_accent: hex(0xECECEE),
};

/// Every palette the user can pick, default first. Batch 4 adds the dark and
/// medium presets here.
pub const PALETTES: &[Palette] = &[GRAPHITE];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The palette in use.
pub fn pal() -> &'static Palette {
    PALETTES
        .get(CURRENT.load(Ordering::Relaxed))
        .unwrap_or(&PALETTES[0])
}

/// Switches the palette (an index into [`PALETTES`]) and restyles `ctx`.
#[allow(dead_code)] // b4-themes: the theme picker
pub fn set_palette(ctx: &egui::Context, index: usize) {
    CURRENT.store(index.min(PALETTES.len() - 1), Ordering::Relaxed);
    style(ctx);
    ctx.request_repaint();
}

// ------------------------------------------------------------------- accent

thread_local! {
    /// The now-playing art's accent (sRGB), set by the app once per frame.
    static ART_ACCENT: Cell<Option<[u8; 3]>> = const { Cell::new(None) };
}

/// Sets the accent for this frame: the playing track's art accent, or
/// `None` for the palette's fallback. Called by `app.rs` before drawing.
pub fn set_art_accent(rgb: Option<[u8; 3]>) {
    ART_ACCENT.with(|a| a.set(rgb));
}

fn art_accent() -> Option<[u8; 3]> {
    ART_ACCENT.with(Cell::get)
}

fn rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

fn color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The accent: OKLCH L 0.80 from the playing art, or the fallback.
pub fn accent() -> Color32 {
    art_accent().map_or(pal().fallback_accent, color)
}

/// Accent on hover (L 0.86).
pub fn accent_hover() -> Color32 {
    match art_accent() {
        Some(a) => color(art_color::with_lightness(a, 0.86)),
        None => pal().fallback_accent,
    }
}

/// Accent while pressed (L 0.72).
pub fn accent_pressed() -> Color32 {
    match art_accent() {
        Some(a) => color(art_color::with_lightness(a, 0.72)),
        None => pal().text_2,
    }
}

/// The player bar fill: the accent at 14% over `bg`. Player bar only.
pub fn tint() -> Color32 {
    color(art_color::tint_over(rgb(accent()), rgb(pal().bg)))
}

/// A track-colored mark on the tint: `text_1` at 16% over [`tint`].
pub fn on_tint_track() -> Color32 {
    let (t, f) = (rgb(tint()), rgb(pal().text_1));
    color(std::array::from_fn(|i| {
        let (a, b) = (f32::from(f[i]), f32::from(t[i]));
        (b + (a - b) * 0.16).round() as u8
    }))
}

// --------------------------------------------------------------------- type

/// Font family names. `Proportional` is Inter 400.
pub const INTER_500: &str = "inter-500";
pub const INTER_600: &str = "inter-600";
/// Phosphor regular and fill icons. Glyph constants are in `crate::icons`.
pub const PHOSPHOR: &str = "phosphor";
pub const PHOSPHOR_FILL: &str = "phosphor-fill";

fn family(name: &str) -> FontFamily {
    FontFamily::Name(Arc::from(name))
}

/// 22 / 600: page title.
pub fn page_title() -> FontId {
    FontId::new(22.0, family(INTER_600))
}

/// 16 / 600: section heading.
pub fn section() -> FontId {
    FontId::new(16.0, family(INTER_600))
}

/// 13 / 500: row title, nav, button.
pub fn row() -> FontId {
    FontId::new(13.0, family(INTER_500))
}

/// 13 / 600: the playing row's title.
pub fn row_strong() -> FontId {
    FontId::new(13.0, family(INTER_600))
}

/// 12 / 400: secondary text, chips.
pub fn secondary() -> FontId {
    FontId::new(12.0, FontFamily::Proportional)
}

/// 11 / 500: caption, table header.
pub fn caption() -> FontId {
    FontId::new(11.0, family(INTER_500))
}

/// A Phosphor regular icon at `size` px.
pub fn icon(size: f32) -> FontId {
    FontId::new(size, family(PHOSPHOR))
}

/// A Phosphor fill icon at `size` px (liked heart, filled play).
pub fn icon_fill(size: f32) -> FontId {
    FontId::new(size, family(PHOSPHOR_FILL))
}

// -------------------------------------------------------- sizes and spacing

pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_20: f32 = 20.0;
pub const SPACE_24: f32 = 24.0;

/// Regions have square corners.
pub const RADIUS_REGION: u8 = 0;
/// Rows and art.
pub const RADIUS_ROW: u8 = 2;
/// Buttons, fields, chips.
pub const RADIUS_CONTROL: u8 = 4;

pub const RAIL_W: f32 = 52.0;
pub const FACET_PANEL_W: f32 = 232.0;
pub const PAGE_HEADER_H: f32 = 64.0;
pub const FILTER_ROW_H: f32 = 48.0;
pub const FIELD_H: f32 = 32.0;
pub const TABLE_HEADER_H: f32 = 28.0;
pub const ROW_H: f32 = 32.0;
pub const ROW_ART: f32 = 24.0;
pub const FACET_ROW_H: f32 = 28.0;
pub const BUTTON_H: f32 = 32.0;
pub const ICON_BUTTON: f32 = 28.0;
pub const PLAYER_H: f32 = 56.0;
pub const PLAYER_ART: f32 = 40.0;
pub const SPINE_W: f32 = 20.0;
/// The column the spine sits in: 1 px edge, 8 px gap, the 20 px strip,
/// 9 px gap (D-Collection). Its top part, beside the page header and filter
/// row, belongs to the collection.
pub const SPINE_COLUMN_W: f32 = 37.0;
/// One spine segment stands for about 10 songs.
pub const SPINE_SEGMENT: f32 = 4.0;
pub const SPINE_SONGS_PER_SEGMENT: usize = 10;
pub const VIEWPORT_BRACKET_MIN: f32 = 12.0;
pub const STROKE_W: f32 = 1.0;
/// Focus ring, in the accent.
pub const FOCUS_W: f32 = 2.0;
/// Seek and volume track height.
pub const SLIDER_TRACK_H: f32 = 3.0;
pub const BANNER_H: f32 = 40.0;
pub const QUEUE_W: f32 = 360.0;
pub const QUEUE_ROW_H: f32 = 44.0;
pub const QUEUE_ART: f32 = 32.0;
pub const ARTIST_AVATAR: f32 = 112.0;
pub const ALBUM_CARD_W: f32 = 159.0;
pub const SIGN_IN_CARD_W: f32 = 480.0;

/// A 1 px stroke in the palette's `stroke`.
pub fn hairline() -> Stroke {
    Stroke::new(STROKE_W, pal().stroke)
}

// -------------------------------------------------------------------- setup

/// Installs the fonts and the style. Call once at startup.
pub fn apply(ctx: &egui::Context) {
    fonts(ctx);
    style(ctx);
}

fn fonts(ctx: &egui::Context) {
    let font = |bytes: &'static [u8]| Arc::new(FontData::from_static(bytes));
    let mut defs = FontDefinitions::empty();
    for (name, bytes) in [
        (
            "inter-400",
            &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..],
        ),
        (
            INTER_500,
            &include_bytes!("../assets/fonts/Inter-Medium.ttf")[..],
        ),
        (
            INTER_600,
            &include_bytes!("../assets/fonts/Inter-SemiBold.ttf")[..],
        ),
        (
            PHOSPHOR,
            &include_bytes!("../assets/fonts/Phosphor.ttf")[..],
        ),
        (
            PHOSPHOR_FILL,
            &include_bytes!("../assets/fonts/Phosphor-Fill.ttf")[..],
        ),
    ] {
        defs.font_data.insert(name.to_owned(), font(bytes));
    }
    // Each text family falls back to Phosphor, so an icon glyph inside a
    // label renders without switching fonts.
    let chain = |first: &str| vec![first.to_owned(), PHOSPHOR.to_owned()];
    defs.families
        .insert(FontFamily::Proportional, chain("inter-400"));
    defs.families
        .insert(FontFamily::Monospace, chain("inter-400"));
    defs.families.insert(family(INTER_500), chain(INTER_500));
    defs.families.insert(family(INTER_600), chain(INTER_600));
    defs.families
        .insert(family(PHOSPHOR), vec![PHOSPHOR.to_owned()]);
    defs.families
        .insert(family(PHOSPHOR_FILL), vec![PHOSPHOR_FILL.to_owned()]);
    ctx.set_fonts(defs);
}

fn style(ctx: &egui::Context) {
    let p = *pal();
    ctx.set_theme(ThemePreference::Dark);
    ctx.style_mut_of(Theme::Dark, |style| {
        let v: &mut Visuals = &mut style.visuals;
        *v = Visuals::dark();
        v.panel_fill = p.bg;
        v.window_fill = p.bg;
        v.extreme_bg_color = p.bg;
        v.faint_bg_color = p.surface_2;
        v.code_bg_color = p.surface_2;
        v.hyperlink_color = p.text_1;
        v.error_fg_color = p.danger;
        v.warn_fg_color = p.danger;
        v.selection.bg_fill = p.surface_3;
        v.selection.stroke = Stroke::new(1.0, p.text_1);
        v.slider_trailing_fill = true;
        v.window_corner_radius = CornerRadius::same(RADIUS_CONTROL);
        v.menu_corner_radius = CornerRadius::same(RADIUS_CONTROL);
        v.window_stroke = Stroke::new(STROKE_W, p.stroke_2);
        v.window_shadow = egui::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(128),
        };
        v.popup_shadow = v.window_shadow;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.bg;
        w.noninteractive.weak_bg_fill = p.bg;
        w.noninteractive.bg_stroke = Stroke::new(STROKE_W, p.stroke);
        w.noninteractive.fg_stroke = Stroke::new(1.0, p.text_2);
        for (state, fill, fg) in [
            (&mut w.inactive, p.bg, p.text_1),
            (&mut w.hovered, p.surface_2, p.text_1),
            (&mut w.active, p.surface_3, p.text_1),
            (&mut w.open, p.surface_3, p.text_1),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(STROKE_W, p.stroke_2);
            state.fg_stroke = Stroke::new(1.0, fg);
            state.corner_radius = CornerRadius::same(RADIUS_CONTROL);
            state.expansion = 0.0;
        }
        // Slider rails use the inactive fill.
        w.inactive.bg_fill = p.stroke_2;

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(SPACE_8, SPACE_4);
        s.button_padding = egui::vec2(SPACE_12, SPACE_4);
        s.interact_size.y = ICON_BUTTON;

        style.text_styles = [
            (TextStyle::Small, caption()),
            (TextStyle::Body, secondary()),
            (TextStyle::Button, row()),
            (TextStyle::Heading, page_title()),
            (TextStyle::Monospace, secondary()),
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
