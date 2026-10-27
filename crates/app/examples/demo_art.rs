//! Writes the art for `FixtureBackend::large`: `<dir>/<i>-<size>.jpg` for
//! each track `i` and each size in `ytm_core::LARGE_ART_SIZES`, then
//! `<dir>/done`. Used by `scripts/perf-check.sh --large`; no network.
//!
//! `cargo run --release -p rustle --example demo_art -- <dir> <tracks>`
//!
//! Each image is a two-color gradient with a few shapes and some grain, in a
//! hue that varies by track, so the art color pass and the spine get varied
//! input and the JPEGs are about the size YouTube serves (a few KB at 60 px,
//! tens of KB at 544 px).

use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{Rgb, RgbImage};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(n)) = (
        args.next(),
        args.next().and_then(|n| n.parse::<usize>().ok()),
    ) else {
        eprintln!("usage: demo_art <dir> <tracks>");
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("create the art directory");
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for t in 0..threads {
            let dir = &dir;
            s.spawn(move || {
                for i in (t..n).step_by(threads) {
                    write_track(dir, i);
                }
            });
        }
    });
    std::fs::write(dir.join("done"), n.to_string()).expect("write the done marker");
}

fn write_track(dir: &Path, i: usize) {
    let sizes = ytm_core::LARGE_ART_SIZES;
    let big = *sizes.last().expect("sizes");
    let full = draw(i as u64, big);
    for px in sizes {
        let img = if px == big {
            full.clone()
        } else {
            image::imageops::resize(&full, px, px, FilterType::Triangle)
        };
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&img)
            .expect("encode");
        std::fs::write(dir.join(format!("{i}-{px}.jpg")), out).expect("write art");
    }
}

/// splitmix64, so each track's art is fixed by its index.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// HSV (all 0..1) to RGB.
fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0]
}

fn draw(seed: u64, size: u32) -> RgbImage {
    let r = mix(seed);
    let unit = |k: u32| ((r >> (k * 8)) & 0xff) as f32 / 255.0;
    let hue = unit(0);
    // Some covers are near gray, as real ones are.
    let sat = if unit(1) < 0.12 {
        0.08
    } else {
        0.35 + 0.6 * unit(2)
    };
    let top = hsv(hue, sat, 0.25 + 0.7 * unit(3));
    let bottom = hsv(hue + 0.08, sat, 0.1 + 0.4 * unit(4));
    let accent = hsv(hue + 0.5, (sat + 0.3).min(1.0), 0.9);
    // Three circles in the accent color, positions fixed by the seed.
    let circles: Vec<(f32, f32, f32)> = (0..3)
        .map(|k| {
            let c = mix(seed ^ (k + 1));
            let f = |b: u32| ((c >> b) & 0xffff) as f32 / 65535.0;
            (f(0), f(16), 0.05 + 0.2 * f(32))
        })
        .collect();
    let s = size as f32;
    RgbImage::from_fn(size, size, |x, y| {
        let (fx, fy) = (x as f32 / s, y as f32 / s);
        let mut px = [0.0; 3];
        for k in 0..3 {
            px[k] = top[k] * (1.0 - fy) + bottom[k] * fy;
        }
        for &(cx, cy, rad) in &circles {
            if (fx - cx).powi(2) + (fy - cy).powi(2) < rad * rad {
                px = accent;
            }
        }
        let grain = (mix(seed ^ u64::from(x) << 20 ^ u64::from(y) << 40) & 15) as f32 - 8.0;
        Rgb(px.map(|v| (v + grain).clamp(0.0, 255.0) as u8))
    })
}
