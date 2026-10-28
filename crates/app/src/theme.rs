//! Direction D tokens (`docs/design/png/D-Tokens.png`): colors, type, sizes,
//! spacing and radii, plus the egui style built from them.
//!
//! Colors come from a [`Palette`], read with [`pal`]. Views never hard-code a
//! neutral: [`PALETTES`] holds Graphite (the default) and well-known dark and
//! medium themes, and switching one swaps every neutral at once. A palette swaps neutrals only. Album art stays
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

/// How light a palette's `bg` is; the theme picker groups by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// `bg` below OKLab L 0.25.
    Dark,
    /// `bg` at OKLab L 0.25 to 0.35. There are no light palettes.
    Medium,
}

/// The neutral colors of one theme. Names follow D-Tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Shown in the theme picker.
    pub name: &'static str,
    /// The picker group.
    pub tone: Tone,
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
    /// The accent when no art accent applies: the theme's signature color.
    /// D uses `text_1`.
    pub fallback_accent: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Direction D: B neutrals, C adaptive accent. The default.
pub const GRAPHITE: Palette = Palette {
    name: "Graphite",
    tone: Tone::Dark,
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

// The presets below map each theme's published colors onto the D roles:
// bg = the editor background, surface_2 = its cursor line or hover,
// surface_3 = its selection, stroke = its panel edge, stroke_2 = its input
// border, text_1/2/3 = foreground, dimmed foreground, comment. A role the
// theme does not define is a blend of two of its colors, noted inline.
// The `tests` module checks every palette's contrast.
//
// Dropped after the checks: Catppuccin Frappé and Everforest Dark (text_1
// reaches only 6.0:1 on the player tint), One Dark (#ABB2BF on #282C34 is
// 6.6:1, comments 2.3:1), Solarized Dark (base1 on base03 is 5.6:1).

/// Catppuccin Mocha: <https://github.com/catppuccin/palette> (palette.json).
pub const CATPPUCCIN_MOCHA: Palette = Palette {
    name: "Catppuccin Mocha",
    tone: Tone::Dark,
    bg: hex(0x1E1E2E),              // base
    surface_2: hex(0x28293A),       // text at 6% over base (style guide: cursor line)
    surface_3: hex(0x313244),       // surface0
    stroke: hex(0x313244),          // surface0
    stroke_2: hex(0x45475A),        // surface1
    text_1: hex(0xCDD6F4),          // text
    text_2: hex(0xA6ADC8),          // subtext0
    text_3: hex(0x7F849C),          // overlay1
    on_accent: hex(0x11111B),       // crust
    danger: hex(0xF38BA8),          // red
    fallback_accent: hex(0xCBA6F7), // mauve
};

/// Tokyo Night (night): <https://github.com/folke/tokyonight.nvim>
/// (`lua/tokyonight/colors/night.lua`).
pub const TOKYO_NIGHT: Palette = Palette {
    name: "Tokyo Night",
    tone: Tone::Dark,
    bg: hex(0x1A1B26),              // bg
    surface_2: hex(0x292E42),       // bg_highlight
    surface_3: hex(0x3B4261),       // fg_gutter
    stroke: hex(0x292E42),          // bg_highlight
    stroke_2: hex(0x414868),        // terminal_black
    text_1: hex(0xC0CAF5),          // fg
    text_2: hex(0xA9B1D6),          // fg_dark
    text_3: hex(0x737AA2),          // dark5 (comment #565F89 is 2.9:1)
    on_accent: hex(0x16161E),       // bg_dark
    danger: hex(0xF7768E),          // red
    fallback_accent: hex(0x7AA2F7), // blue
};

/// Rosé Pine (main): <https://rosepinetheme.com/palette>.
pub const ROSE_PINE: Palette = Palette {
    name: "Rosé Pine",
    tone: Tone::Dark,
    bg: hex(0x191724),              // base
    surface_2: hex(0x21202E),       // highlight low
    surface_3: hex(0x403D52),       // highlight med
    stroke: hex(0x26233A),          // overlay
    stroke_2: hex(0x524F67),        // highlight high
    text_1: hex(0xE0DEF4),          // text
    text_2: hex(0x908CAA),          // subtle
    text_3: hex(0x6E6A86),          // muted
    on_accent: hex(0x191724),       // base
    danger: hex(0xEB6F92),          // love
    fallback_accent: hex(0xEBBCBA), // rose
};

/// Kanagawa Wave: <https://github.com/rebelot/kanagawa.nvim>
/// (`lua/kanagawa/colors.lua`). text_2 blends fujiWhite and fujiGray: the
/// theme's `fg_dim` (oldWhite #C8C093) reads as yellow and almost as strong
/// as text_1 in the track table.
pub const KANAGAWA_WAVE: Palette = Palette {
    name: "Kanagawa Wave",
    tone: Tone::Dark,
    bg: hex(0x1F1F28),              // sumiInk3
    surface_2: hex(0x2A2A37),       // sumiInk4
    surface_3: hex(0x363646),       // sumiInk5
    stroke: hex(0x2A2A37),          // sumiInk4
    stroke_2: hex(0x54546D),        // sumiInk6
    text_1: hex(0xDCD7BA),          // fujiWhite
    text_2: hex(0xA7A492),          // fujiWhite at 50% over fujiGray
    text_3: hex(0x727169),          // fujiGray
    on_accent: hex(0x16161D),       // sumiInk0
    danger: hex(0xE46876),          // waveRed
    fallback_accent: hex(0x7E9CD8), // crystalBlue
};

/// Kanagawa Dragon: <https://github.com/rebelot/kanagawa.nvim>
/// (`lua/kanagawa/colors.lua`).
pub const KANAGAWA_DRAGON: Palette = Palette {
    name: "Kanagawa Dragon",
    tone: Tone::Dark,
    bg: hex(0x181616),              // dragonBlack3
    surface_2: hex(0x282727),       // dragonBlack4
    surface_3: hex(0x393836),       // dragonBlack5
    stroke: hex(0x282727),          // dragonBlack4
    stroke_2: hex(0x625E5A),        // dragonBlack6
    text_1: hex(0xC5C9C5),          // dragonWhite
    text_2: hex(0xA6A69C),          // dragonGray
    text_3: hex(0x737C73),          // dragonAsh
    on_accent: hex(0x0D0C0C),       // dragonBlack0
    danger: hex(0xC4746E),          // dragonRed
    fallback_accent: hex(0x8BA4B0), // dragonBlue2
};

/// GitHub Dark: <https://github.com/primer/primitives> v7.17
/// (`data/colors/vars/global_dark.ts`).
pub const GITHUB_DARK: Palette = Palette {
    name: "GitHub Dark",
    tone: Tone::Dark,
    bg: hex(0x0D1117),              // canvas.default
    surface_2: hex(0x161B22),       // canvas.subtle
    surface_3: hex(0x343941),       // neutral.muted (gray.4 at 40% over canvas)
    stroke: hex(0x21262D),          // border.muted
    stroke_2: hex(0x30363D),        // border.default
    text_1: hex(0xE6EDF3),          // fg.default
    text_2: hex(0x848D97),          // fg.muted
    text_3: hex(0x6E7681),          // fg.subtle
    on_accent: hex(0x0D1117),       // canvas.default
    danger: hex(0xF85149),          // danger.fg
    fallback_accent: hex(0x58A6FF), // blue.3 (accent.fg #2F81F7 looks heavy as a fill)
};

/// Catppuccin Macchiato: <https://github.com/catppuccin/palette>.
pub const CATPPUCCIN_MACCHIATO: Palette = Palette {
    name: "Catppuccin Macchiato",
    tone: Tone::Medium,
    bg: hex(0x24273A),              // base
    surface_2: hex(0x2E3145),       // text at 6% over base (style guide: cursor line)
    surface_3: hex(0x363A4F),       // surface0
    stroke: hex(0x363A4F),          // surface0
    stroke_2: hex(0x494D64),        // surface1
    text_1: hex(0xCAD3F5),          // text
    text_2: hex(0xA5ADCB),          // subtext0
    text_3: hex(0x8087A2),          // overlay1
    on_accent: hex(0x181926),       // crust
    danger: hex(0xED8796),          // red
    fallback_accent: hex(0xC6A0F6), // mauve
};

/// Tokyo Night Storm: <https://github.com/folke/tokyonight.nvim>
/// (`lua/tokyonight/colors/storm.lua`). `bg` is Storm's `bg_dark` (its
/// sidebar fill), not `bg` #24283B: on #24283B text_1 drops to 6.7:1 on the
/// player tint.
pub const TOKYO_NIGHT_STORM: Palette = Palette {
    name: "Tokyo Night Storm",
    tone: Tone::Medium,
    bg: hex(0x1F2335),              // bg_dark
    surface_2: hex(0x292E42),       // bg_highlight
    surface_3: hex(0x3B4261),       // fg_gutter
    stroke: hex(0x3B4261),          // fg_gutter
    stroke_2: hex(0x545C7E),        // dark3
    text_1: hex(0xC0CAF5),          // fg
    text_2: hex(0xA9B1D6),          // fg_dark
    text_3: hex(0x737AA2),          // dark5
    on_accent: hex(0x1F2335),       // bg_dark
    danger: hex(0xF7768E),          // red
    fallback_accent: hex(0x7AA2F7), // blue
};

/// Nord: <https://www.nordtheme.com/docs/colors-and-palettes>. Nord has no
/// gray between nord3 and nord4, so text_2 and text_3 blend them.
pub const NORD: Palette = Palette {
    name: "Nord",
    tone: Tone::Medium,
    bg: hex(0x2E3440),              // nord0
    surface_2: hex(0x3B4252),       // nord1
    surface_3: hex(0x434C5E),       // nord2
    stroke: hex(0x3B4252),          // nord1
    stroke_2: hex(0x4C566A),        // nord3
    text_1: hex(0xECEFF4),          // nord6
    text_2: hex(0xA8AEBA),          // nord4 at 72% over nord0
    text_3: hex(0x868C98),          // nord4 at 52% over nord0
    on_accent: hex(0x2E3440),       // nord0
    danger: hex(0xBF616A),          // nord11
    fallback_accent: hex(0x88C0D0), // nord8
};

/// Dracula: <https://github.com/dracula/visual-studio-code>
/// (`src/dracula.yml`, base colors and UI variants).
pub const DRACULA: Palette = Palette {
    name: "Dracula",
    tone: Tone::Medium,
    bg: hex(0x282A36),              // Background
    surface_2: hex(0x343746),       // BGLight
    surface_3: hex(0x44475A),       // Selection
    stroke: hex(0x343746),          // BGLight
    stroke_2: hex(0x424450),        // BGLighter
    text_1: hex(0xF8F8F2),          // Foreground
    text_2: hex(0xA6AEC7),          // Foreground at 45% over Comment
    text_3: hex(0x6272A4),          // Comment
    on_accent: hex(0x191A21),       // BGDarker
    danger: hex(0xFF5555),          // Red
    fallback_accent: hex(0xBD93F9), // Purple
};

/// Gruvbox Dark (medium contrast): <https://github.com/morhetz/gruvbox>.
pub const GRUVBOX_DARK: Palette = Palette {
    name: "Gruvbox Dark",
    tone: Tone::Medium,
    bg: hex(0x282828),              // bg0
    surface_2: hex(0x32302F),       // bg0_s (dark0_soft)
    surface_3: hex(0x3C3836),       // bg1
    stroke: hex(0x3C3836),          // bg1
    stroke_2: hex(0x504945),        // bg2
    text_1: hex(0xEBDBB2),          // fg1
    text_2: hex(0xA89984),          // fg4
    text_3: hex(0x928374),          // gray
    on_accent: hex(0x1D2021),       // bg0_h
    danger: hex(0xFB4934),          // red
    fallback_accent: hex(0xFABD2F), // yellow
};

/// Rosé Pine Moon: <https://rosepinetheme.com/palette>.
pub const ROSE_PINE_MOON: Palette = Palette {
    name: "Rosé Pine Moon",
    tone: Tone::Medium,
    bg: hex(0x232136),              // base
    surface_2: hex(0x2A283E),       // highlight low
    surface_3: hex(0x44415A),       // highlight med
    stroke: hex(0x393552),          // overlay
    stroke_2: hex(0x56526E),        // highlight high
    text_1: hex(0xE0DEF4),          // text
    text_2: hex(0x908CAA),          // subtle
    text_3: hex(0x6E6A86),          // muted
    on_accent: hex(0x232136),       // base
    danger: hex(0xEB6F92),          // love
    fallback_accent: hex(0xEA9A97), // rose
};

/// Every palette the user can pick, default first, then the dark presets,
/// then the medium ones.
pub const PALETTES: &[Palette] = &[
    GRAPHITE,
    CATPPUCCIN_MOCHA,
    TOKYO_NIGHT,
    ROSE_PINE,
    KANAGAWA_WAVE,
    KANAGAWA_DRAGON,
    GITHUB_DARK,
    CATPPUCCIN_MACCHIATO,
    TOKYO_NIGHT_STORM,
    NORD,
    DRACULA,
    GRUVBOX_DARK,
    ROSE_PINE_MOON,
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// The palette in use.
pub fn pal() -> &'static Palette {
    PALETTES
        .get(CURRENT.load(Ordering::Relaxed))
        .unwrap_or(&PALETTES[0])
}

/// Switches the palette (an index into [`PALETTES`]) and restyles `ctx`.
pub fn set_palette(ctx: &egui::Context, index: usize) {
    CURRENT.store(index.min(PALETTES.len() - 1), Ordering::Relaxed);
    style(ctx);
    ctx.request_repaint();
}

/// Switches to the palette called `name`; an unknown name picks the
/// default. Does nothing when it is already in use.
pub fn set_palette_by_name(ctx: &egui::Context, name: &str) {
    let index = PALETTES.iter().position(|p| p.name == name).unwrap_or(0);
    if index != CURRENT.load(Ordering::Relaxed) {
        set_palette(ctx, index);
    }
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

/// 13 / 400: body text (lyrics, settings rows, notes).
pub fn body() -> FontId {
    FontId::new(13.0, FontFamily::Proportional)
}

/// 11 / 400: small regular text (sources, hints).
pub fn small() -> FontId {
    FontId::new(11.0, FontFamily::Proportional)
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

/// Horizontal inset of page content (D-Search, D-Album: text at x = 72).
pub const PAGE_INSET_X: f32 = SPACE_20;
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
/// The spine's segments stretch to fill the strip, each about this tall.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }

    /// WCAG contrast ratio.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// OKLab lightness (Ottosson's matrices, as in `art_color`).
    fn oklab_l(c: Color32) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
        let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
        let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
        let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s
    }

    /// A neutral art accent: gray art at OKLCH L 0.80.
    fn gray_accent() -> Color32 {
        color(art_color::accent([128, 128, 128]))
    }

    /// Art accents at L 0.80 (the D-Tokens board) plus the neutral one.
    fn art_accents() -> Vec<Color32> {
        let mut out: Vec<Color32> = [0x71CFDA, 0x8DC4F4, 0xFA9FB4, 0xE0B767]
            .into_iter()
            .map(hex)
            .collect();
        out.push(gray_accent());
        out
    }

    fn check(what: &str, p: &Palette, got: f64, min: f64) {
        assert!(got >= min, "{}: {what} is {got:.2}, want >= {min}", p.name);
    }

    #[test]
    fn oklab_matches_art_color() {
        assert!((oklab_l(gray_accent()) - 0.80).abs() < 0.005);
        assert!((oklab_l(hex(0xFFFFFF)) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn graphite_is_first_and_names_are_unique() {
        assert_eq!(PALETTES[0], GRAPHITE);
        assert_eq!(PALETTES[0].name, "Graphite");
        for (i, a) in PALETTES.iter().enumerate() {
            for b in &PALETTES[i + 1..] {
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn tone_matches_bg_lightness() {
        for p in PALETTES {
            let l = oklab_l(p.bg);
            let ok = match p.tone {
                Tone::Dark => l < 0.25,
                Tone::Medium => (0.25..=0.35).contains(&l),
            };
            assert!(ok, "{}: bg L {l:.3} does not fit {:?}", p.name, p.tone);
        }
    }

    #[test]
    fn text_reads_on_bg_and_selection() {
        for p in PALETTES {
            check("text_1 on bg", p, contrast(p.text_1, p.bg), 7.0);
            check("text_2 on bg", p, contrast(p.text_2, p.bg), 4.5);
            check("text_3 on bg", p, contrast(p.text_3, p.bg), 3.0);
            check(
                "text_1 on surface_3",
                p,
                contrast(p.text_1, p.surface_3),
                4.5,
            );
            check("danger on bg", p, contrast(p.danger, p.bg), 3.0);
        }
    }

    #[test]
    fn on_accent_reads_on_every_accent() {
        for p in PALETTES {
            check(
                "on_accent on fallback_accent",
                p,
                contrast(p.on_accent, p.fallback_accent),
                4.5,
            );
            for a in art_accents() {
                check("on_accent on art accent", p, contrast(p.on_accent, a), 4.5);
            }
        }
    }

    #[test]
    fn surfaces_step_up_from_bg() {
        for p in PALETTES {
            let bg = oklab_l(p.bg);
            check("stroke ΔL", p, (oklab_l(p.stroke) - bg).abs(), 0.02);
            check("stroke_2 ΔL", p, oklab_l(p.stroke_2) - bg, 0.02);
            check("surface_2 ΔL", p, oklab_l(p.surface_2) - bg, 0.02);
            check(
                "surface_3 over surface_2",
                p,
                oklab_l(p.surface_3) - oklab_l(p.surface_2),
                0.0,
            );
        }
    }

    #[test]
    fn player_tint_keeps_text_readable() {
        for p in PALETTES {
            for a in art_accents().into_iter().chain([p.fallback_accent]) {
                let tint = color(art_color::tint_over(rgb(a), rgb(p.bg)));
                check("text_1 on player tint", p, contrast(p.text_1, tint), 7.0);
            }
        }
    }
}
