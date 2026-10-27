//! Album-art accent color: the dominant color of a thumbnail, mapped to the
//! Direction D accent (OKLCH L 0.80, chroma 0.09 to 0.11) and the 14% player
//! tint.
//!
//! `dominant` finds the art's main hue, `accent` turns any color into the
//! accent, and `tint` gives the player bar background for an accent. Art
//! with no hue (grayscale, or chroma under `MIN_HUE_CHROMA`) gets a neutral
//! light gray accent instead of a random hue: the chrome stays neutral.

use egui::ColorImage;

/// Background the tint is blended over (`bg` in D-Tokens).
const BG: [u8; 3] = [0x11, 0x12, 0x14];
/// Accent lightness in OKLCH.
const ACCENT_L: f64 = 0.80;
const MIN_CHROMA: f64 = 0.09;
const MAX_CHROMA: f64 = 0.11;
/// Below this chroma a color has no reliable hue.
const MIN_HUE_CHROMA: f64 = 0.02;
/// Share of the accent in the player tint.
const TINT: f64 = 0.14;
/// `dominant` samples the art on a grid this many cells wide.
const GRID: usize = 16;
/// Hue histogram resolution.
const HUE_BINS: usize = 36;

/// The art's dominant color. Pixels vote for their hue weighted by chroma
/// squared, so a saturated motif beats a larger dull background (D-Tokens
/// example 3: pink circles on navy give a pink accent). Near-black and
/// near-white pixels do not vote unless the art is only that. Returns the
/// mean color of the winning hue window.
pub fn dominant(image: &ColorImage) -> [u8; 3] {
    let samples = grid_samples(image);
    if samples.is_empty() {
        return [0, 0, 0];
    }
    let labs: Vec<[f64; 3]> = samples.iter().map(|&c| srgb_to_oklab(c)).collect();

    let mut bins = [0.0f64; HUE_BINS];
    let mut voters = 0usize;
    for lab in &labs {
        if extreme(*lab) {
            continue;
        }
        voters += 1;
        let (c, h) = chroma_hue(*lab);
        if c >= MIN_HUE_CHROMA {
            bins[hue_bin(h)] += c * c;
        }
    }
    // Only near-black or near-white: the mean of everything.
    if voters == 0 {
        return oklab_to_srgb(mean(labs.iter()));
    }
    // No hue anywhere: the mean of the non-extreme pixels.
    let window =
        |i: usize| bins[(i + HUE_BINS - 1) % HUE_BINS] + bins[i] + bins[(i + 1) % HUE_BINS];
    let Some(best) = (0..HUE_BINS)
        .filter(|&i| window(i) > 0.0)
        .max_by(|&a, &b| window(a).total_cmp(&window(b)))
    else {
        return oklab_to_srgb(mean(labs.iter().filter(|l| !extreme(**l))));
    };
    let in_window = |lab: &&[f64; 3]| {
        let (c, h) = chroma_hue(**lab);
        if extreme(**lab) || c < MIN_HUE_CHROMA {
            return false;
        }
        let d = (hue_bin(h) + HUE_BINS - best) % HUE_BINS;
        d <= 1 || d == HUE_BINS - 1
    };
    // Weighted like the vote, so the window's saturated pixels set the hue.
    let mut sum = [0.0; 3];
    let mut total = 0.0;
    for lab in labs.iter().filter(in_window) {
        let (c, _) = chroma_hue(*lab);
        let w = c * c;
        for k in 0..3 {
            sum[k] += lab[k] * w;
        }
        total += w;
    }
    oklab_to_srgb(sum.map(|s| s / total))
}

/// The accent for a source color: its OKLCH hue at L 0.80 with chroma
/// clamped to 0.09..=0.11, reduced further if needed to stay in sRGB. A
/// source with no hue gives a neutral gray at L 0.80.
pub fn accent(rgb: [u8; 3]) -> [u8; 3] {
    let (c, h) = chroma_hue(srgb_to_oklab(rgb));
    if c < MIN_HUE_CHROMA {
        return oklab_to_srgb([ACCENT_L, 0.0, 0.0]);
    }
    let lab_at = |c: f64| [ACCENT_L, c * h.cos(), c * h.sin()];
    let mut c = c.clamp(MIN_CHROMA, MAX_CHROMA);
    if !in_gamut(oklab_to_linear(lab_at(c))) {
        // Bisect for the largest chroma that fits; chroma 0 always does.
        let (mut lo, mut hi) = (0.0, c);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if in_gamut(oklab_to_linear(lab_at(mid))) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        c = lo;
    }
    oklab_to_srgb(lab_at(c))
}

/// The player bar background: `accent` at 14% over `bg`, blended in sRGB
/// like the design board.
#[allow(dead_code)] // b3-ui: the player bar
pub fn tint(accent: [u8; 3]) -> [u8; 3] {
    std::array::from_fn(|i| {
        let (a, b) = (f64::from(accent[i]), f64::from(BG[i]));
        to_u8(b + (a - b) * TINT)
    })
}

/// Opaque pixels sampled as the means of a `GRID`×`GRID` grid of cells.
fn grid_samples(image: &ColorImage) -> Vec<[u8; 3]> {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let (gw, gh) = (w.min(GRID), h.min(GRID));
    let mut out = Vec::with_capacity(gw * gh);
    for gy in 0..gh {
        for gx in 0..gw {
            let (x0, x1) = (gx * w / gw, (gx + 1) * w / gw);
            let (y0, y1) = (gy * h / gh, (gy + 1) * h / gh);
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = image.pixels[y * w + x];
                    if p.a() < 128 {
                        continue;
                    }
                    sum[0] += u32::from(p.r());
                    sum[1] += u32::from(p.g());
                    sum[2] += u32::from(p.b());
                    n += 1;
                }
            }
            if n > 0 {
                out.push(sum.map(|s| ((s + n / 2) / n) as u8));
            }
        }
    }
    out
}

/// Near-black or near-white: too dark or too light to carry the art's hue.
fn extreme([l, a, b]: [f64; 3]) -> bool {
    let c = a.hypot(b);
    l < 0.18 || (l < 0.25 && c < 0.04) || l > 0.97 || (l > 0.93 && c < 0.03)
}

fn hue_bin(h: f64) -> usize {
    let turn = h.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
    ((turn * HUE_BINS as f64) as usize).min(HUE_BINS - 1)
}

fn mean<'a>(labs: impl Iterator<Item = &'a [f64; 3]>) -> [f64; 3] {
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    for lab in labs {
        for k in 0..3 {
            sum[k] += lab[k];
        }
        n += 1.0;
    }
    if n == 0.0 { sum } else { sum.map(|s| s / n) }
}

/// Chroma and hue (radians) of an OKLab color.
fn chroma_hue([_, a, b]: [f64; 3]) -> (f64, f64) {
    (a.hypot(b), b.atan2(a))
}

fn to_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn srgb_to_linear(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    to_u8(s * 255.0)
}

/// sRGB to OKLab (Björn Ottosson's matrices).
fn srgb_to_oklab(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
    let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
    let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

/// OKLab to linear sRGB, unclamped.
fn oklab_to_linear([ll, a, b]: [f64; 3]) -> [f64; 3] {
    let l = ll + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m = ll - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s = ll - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
    ]
}

fn oklab_to_srgb(lab: [f64; 3]) -> [u8; 3] {
    oklab_to_linear(lab).map(linear_to_srgb)
}

fn in_gamut(rgb: [f64; 3]) -> bool {
    const EPS: f64 = 1e-6;
    rgb.iter().all(|&c| (-EPS..=1.0 + EPS).contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    fn hex(s: &str) -> [u8; 3] {
        let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap();
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    fn assert_close(got: [u8; 3], want: &str, tol: u8) {
        let want = hex(want);
        let ok = (0..3).all(|i| got[i].abs_diff(want[i]) <= tol);
        assert!(ok, "got {got:02X?}, want {want:02X?} (±{tol})");
    }

    /// A shape on the art, in fractions of the side like the board's CSS.
    enum Shape {
        Rect,
        Ellipse,
    }

    /// Paints the art the way D-Tokens draws it: a background and up to two
    /// shapes (left, top, width, height as fractions), later ones on top.
    fn art(bg: [u8; 3], shapes: &[(Shape, [f64; 4], [u8; 3])]) -> ColorImage {
        const SIDE: usize = 96;
        let mut img = ColorImage::new(
            [SIDE, SIDE],
            vec![Color32::from_rgb(bg[0], bg[1], bg[2]); SIDE * SIDE],
        );
        for y in 0..SIDE {
            for x in 0..SIDE {
                let (px, py) = (
                    (x as f64 + 0.5) / SIDE as f64,
                    (y as f64 + 0.5) / SIDE as f64,
                );
                for (shape, [l, t, w, h], c) in shapes {
                    let inside = match shape {
                        Shape::Rect => px >= *l && px < l + w && py >= *t && py < t + h,
                        Shape::Ellipse => {
                            let dx = (px - (l + w / 2.0)) / (w / 2.0);
                            let dy = (py - (t + h / 2.0)) / (h / 2.0);
                            dx * dx + dy * dy <= 1.0
                        }
                    };
                    if inside {
                        img.pixels[y * SIDE + x] = Color32::from_rgb(c[0], c[1], c[2]);
                    }
                }
            }
        }
        img
    }

    /// The four art swatches of the D-Tokens accent table, with the accent
    /// and tint the board lists for each.
    fn board() -> Vec<(ColorImage, &'static str, &'static str)> {
        vec![
            (
                // Teal with a cream arch (drawn as a rect) and a dark bar.
                art(
                    [30, 92, 99],
                    &[
                        (Shape::Rect, [0.36, 0.20, 0.28, 0.50], [233, 223, 200]),
                        (Shape::Rect, [0.28, 0.70, 0.44, 0.04], [18, 63, 68]),
                    ],
                ),
                "#71CFDA",
                "#1E2C30",
            ),
            (
                // Pale gray with a gray-blue oval.
                art(
                    [207, 214, 220],
                    &[(Shape::Ellipse, [0.28, 0.10, 0.44, 0.80], [143, 163, 181])],
                ),
                "#8DC4F4",
                "#222B33",
            ),
            (
                // Navy with a pink and an orange circle.
                art(
                    [32, 28, 61],
                    &[
                        (Shape::Ellipse, [0.16, 0.16, 0.48, 0.48], [232, 73, 124]),
                        (Shape::Ellipse, [0.50, 0.50, 0.32, 0.32], [246, 160, 77]),
                    ],
                ),
                "#FA9FB4",
                "#32262A",
            ),
            (
                // Amber with two near-black bars.
                art(
                    [227, 174, 58],
                    &[
                        (Shape::Rect, [0.0, 0.28, 1.0, 0.10], [30, 30, 30]),
                        (Shape::Rect, [0.0, 0.50, 1.0, 0.20], [30, 30, 30]),
                    ],
                ),
                "#E0B767",
                "#2E2920",
            ),
        ]
    }

    #[test]
    fn oklab_round_trips() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(15) {
                for b in (0..=255).step_by(15) {
                    let c = [r as u8, g as u8, b as u8];
                    assert_eq!(oklab_to_srgb(srgb_to_oklab(c)), c);
                }
            }
        }
        // Reference values from Ottosson's OKLab post.
        let white = srgb_to_oklab([255, 255, 255]);
        assert!((white[0] - 1.0).abs() < 1e-4 && white[1].abs() < 1e-4 && white[2].abs() < 1e-4);
        let [l, c, h] = {
            let lab = srgb_to_oklab([255, 0, 0]);
            let (c, h) = chroma_hue(lab);
            [lab[0], c, h.to_degrees()]
        };
        assert!(
            (l - 0.62796).abs() < 1e-4 && (c - 0.25768).abs() < 1e-4 && (h - 29.234).abs() < 1e-2
        );
    }

    #[test]
    fn board_art_maps_to_board_accents_and_tints() {
        for (img, want_accent, want_tint) in board() {
            let a = accent(dominant(&img));
            assert_close(a, want_accent, 2);
            assert_close(tint(a), want_tint, 2);
        }
    }

    #[test]
    fn board_tints_are_exact_for_board_accents() {
        for (_, a, t) in board() {
            assert_close(tint(hex(a)), t, 0);
        }
    }

    #[test]
    fn accent_has_target_lightness_and_clamped_chroma() {
        for rgb in [
            [255, 0, 0],
            [0, 0, 255],
            [0, 255, 0],
            [40, 60, 50],
            [255, 255, 0],
        ] {
            let lab = srgb_to_oklab(accent(rgb));
            let (c, _) = chroma_hue(lab);
            assert!((lab[0] - ACCENT_L).abs() < 0.01, "{rgb:?}: L {}", lab[0]);
            assert!(c <= MAX_CHROMA + 0.005, "{rgb:?}: C {c}");
        }
        // Blue at L 0.80 fits chroma 0.09 inside sRGB.
        let (c, _) = chroma_hue(srgb_to_oklab(accent([0, 0, 255])));
        assert!(c >= MIN_CHROMA - 0.005, "C {c}");
    }

    #[test]
    fn gray_art_gets_a_neutral_accent() {
        let gray = accent([128, 128, 128]);
        assert_eq!(gray[0], gray[1]);
        assert_eq!(gray[1], gray[2]);
        assert_eq!(accent([0, 0, 0]), gray);
    }

    #[test]
    fn near_black_and_white_do_not_vote() {
        // A small red patch on black: red wins.
        let red = art(
            [4, 4, 4],
            &[(Shape::Rect, [0.0, 0.0, 0.25, 0.25], [200, 30, 40])],
        );
        let d = dominant(&red);
        assert!(d[0] > 150 && d[1] < 60, "{d:?}");
        // A small blue patch on white: blue wins.
        let blue = art(
            [252, 252, 252],
            &[(Shape::Rect, [0.5, 0.5, 0.25, 0.25], [30, 60, 200])],
        );
        let d = dominant(&blue);
        assert!(d[2] > 150 && d[0] < 60, "{d:?}");
    }

    #[test]
    fn art_that_is_only_black_or_white_keeps_its_color() {
        assert_eq!(dominant(&art([0, 0, 0], &[])), [0, 0, 0]);
        assert_eq!(dominant(&art([255, 255, 255], &[])), [255, 255, 255]);
        // Black with white text: a gray mean, so a neutral accent.
        let text = art(
            [0, 0, 0],
            &[(Shape::Rect, [0.2, 0.4, 0.6, 0.2], [255, 255, 255])],
        );
        let d = dominant(&text);
        assert!(d[0] == d[1] && d[1] == d[2], "{d:?}");
        assert_eq!(accent(d), accent([128, 128, 128]));
    }

    #[test]
    fn handles_tiny_and_transparent_images() {
        let one = ColorImage::new([1, 1], vec![Color32::from_rgb(200, 30, 40)]);
        assert_eq!(dominant(&one), [200, 30, 40]);
        let clear = ColorImage::new([2, 2], vec![Color32::TRANSPARENT; 4]);
        assert_eq!(dominant(&clear), [0, 0, 0]);
        let empty = ColorImage::new([0, 0], Vec::new());
        assert_eq!(dominant(&empty), [0, 0, 0]);
    }
}
