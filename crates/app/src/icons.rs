//! Phosphor icon codepoints (https://phosphoricons.com), copied from
//! egui-phosphor 0.14.0 `src/variants/codepoints.rs` (MIT OR Apache-2.0;
//! the icons are MIT). The crate itself pins egui 0.36, so it is not a
//! dependency. Draw with `theme::icon(size)` or `theme::icon_fill(size)`.
//!
//! Only the icons the app draws are listed, and the fonts in
//! `assets/fonts/Phosphor*.ttf` hold only those glyphs (about 950 KB less
//! in the binary, about 0.6 MiB less RSS). To add one, copy its line from
//! `assets/fonts/phosphor-full/codepoints.rs` and run
//! `scripts/subset-icons.sh`; the test below fails until you do.

pub const ARROW_COUNTER_CLOCKWISE: &str = "\u{E038}";
pub const ARROW_RIGHT: &str = "\u{E06C}";
pub const BOOKS: &str = "\u{E758}";
pub const CARET_DOWN: &str = "\u{E136}";
pub const CHECK: &str = "\u{E182}";
pub const CLOCK_COUNTER_CLOCKWISE: &str = "\u{E1A0}";
pub const COPY: &str = "\u{E1CA}";
pub const CROSSHAIR: &str = "\u{E1D6}";
pub const DOWNLOAD_SIMPLE: &str = "\u{E20C}";
pub const FLOPPY_DISK: &str = "\u{E248}";
pub const FUNNEL: &str = "\u{E266}";
pub const GEAR_SIX: &str = "\u{E272}";
pub const HEART: &str = "\u{E2A8}";
pub const INFO: &str = "\u{E2CE}";
pub const KEYBOARD: &str = "\u{E2D8}";
pub const MAGNIFYING_GLASS: &str = "\u{E30C}";
pub const MICROPHONE_STAGE: &str = "\u{E75C}";
pub const MUSIC_NOTES: &str = "\u{E340}";
pub const PALETTE: &str = "\u{E6C8}";
pub const PAUSE: &str = "\u{E39E}";
pub const PLAY: &str = "\u{E3D0}";
pub const PLAYLIST: &str = "\u{E6AA}";
pub const PLUS: &str = "\u{E3D4}";
pub const PUSH_PIN: &str = "\u{E3E2}";
pub const QUEUE: &str = "\u{E6AC}";
pub const REPEAT: &str = "\u{E3F6}";
pub const REPEAT_ONCE: &str = "\u{E3F8}";
pub const SEAL_CHECK: &str = "\u{E606}";
pub const SHUFFLE: &str = "\u{E422}";
pub const SIGN_OUT: &str = "\u{E42A}";
pub const SKIP_BACK: &str = "\u{E5A4}";
pub const SKIP_FORWARD: &str = "\u{E5A6}";
pub const SPEAKER_HIGH: &str = "\u{E44A}";
pub const SPEAKER_LOW: &str = "\u{E44C}";
pub const SPEAKER_X: &str = "\u{E45C}";
pub const TEXT_AA: &str = "\u{E6EE}";
pub const USER_CIRCLE: &str = "\u{E4C4}";
pub const WARNING_CIRCLE: &str = "\u{E4E2}";
pub const WIFI_SLASH: &str = "\u{E4F2}";
pub const X: &str = "\u{E4F6}";

#[cfg(test)]
mod tests {
    use crate::theme;

    #[test]
    fn every_icon_is_in_both_subset_fonts() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        // Fonts load on the first pass.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let icons: Vec<(&str, char)> = include_str!("icons.rs")
            .lines()
            .filter_map(|l| {
                let rest = l.strip_prefix("pub const ")?;
                let name = rest.split(':').next()?;
                let hex = rest.split("\\u{").nth(1)?.split('}').next()?;
                Some((name, char::from_u32(u32::from_str_radix(hex, 16).ok()?)?))
            })
            .collect();
        assert!(icons.len() >= 20, "parsed {} icons", icons.len());
        ctx.fonts_mut(|f| {
            for (name, c) in icons {
                for font in [theme::icon(12.0), theme::icon_fill(12.0)] {
                    assert!(
                        f.has_glyph(&font, c),
                        "{name} is missing from {font:?}: run scripts/subset-icons.sh"
                    );
                }
            }
        });
    }
}
