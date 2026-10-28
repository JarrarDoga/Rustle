//! The Rustle mark: three leaves (gold, amber, rust), drawn from the same
//! geometry as `packaging/logo/svg/rustle-glyph.svg`, so the app needs no
//! SVG renderer.
//!
//! The leaf colors are the brand's own and do not follow the palette, the
//! one exception to "colors only via `theme`". Below [`SMALL_BELOW`] px the
//! small-size variant is used: bigger leaves and no rib on the large one,
//! as in `rustle-icon-small.svg`.

use egui::epaint::{CubicBezierShape, PathShape, QuadraticBezierShape};
use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2};

const GOLD: Color32 = Color32::from_rgb(0xE8, 0xC2, 0x6A);
const AMBER: Color32 = Color32::from_rgb(0xE0, 0xA0, 0x4A);
const RUST: Color32 = Color32::from_rgb(0xC8, 0x64, 0x3B);

/// Under this size (px) the small variant reads better.
pub const SMALL_BELOW: f32 = 40.0;

/// `translate(tx ty) scale(s) rotate(deg 52 48)` from the SVG.
#[derive(Clone, Copy)]
struct Place {
    tx: f32,
    ty: f32,
    s: f32,
    deg: f32,
}

impl Place {
    fn apply(self, p: Pos2) -> Pos2 {
        let (sin, cos) = self.deg.to_radians().sin_cos();
        let (dx, dy) = (p.x - 52.0, p.y - 48.0);
        let r = pos2(52.0 + dx * cos - dy * sin, 48.0 + dx * sin + dy * cos);
        pos2(self.tx + r.x * self.s, self.ty + r.y * self.s)
    }
}

const FULL: [(Place, Color32); 3] = [
    (place(48.96, 45.04, 0.52, -30.0), GOLD),
    (place(73.60, 76.40, 0.70, 75.0), AMBER),
    (place(109.04, 122.96, 0.98, -15.0), RUST),
];
const SMALL: [(Place, Color32); 3] = [
    (place(42.64, 38.40, 0.5616, -30.0), GOLD),
    (place(69.25, 72.27, 0.7560, 75.0), AMBER),
    (place(107.52, 122.56, 1.0584, -15.0), RUST),
];

const fn place(tx: f32, ty: f32, s: f32, deg: f32) -> Place {
    Place { tx, ty, s, deg }
}

/// `M22 82 C 6 46, 36 10, 90 10 C 90 50, 62 88, 22 82 Z`, flattened.
fn leaf(at: Place) -> Vec<Pos2> {
    let curve = |pts: [Pos2; 4]| {
        CubicBezierShape::from_points_stroke(
            pts.map(|p| at.apply(p)),
            false,
            Color32::TRANSPARENT,
            Stroke::NONE,
        )
        .flatten(Some(0.25))
    };
    let mut points = curve([
        pos2(22.0, 82.0),
        pos2(6.0, 46.0),
        pos2(36.0, 10.0),
        pos2(90.0, 10.0),
    ]);
    points.pop();
    points.extend(curve([
        pos2(90.0, 10.0),
        pos2(90.0, 50.0),
        pos2(62.0, 88.0),
        pos2(22.0, 82.0),
    ]));
    points.pop();
    points
}

/// Paints the mark fitted into `rect`, centered. `bg` is what is behind it:
/// the rib on the large leaf is cut out in that color.
pub fn paint(painter: &Painter, rect: Rect, bg: Color32) {
    let small = rect.width().min(rect.height()) < SMALL_BELOW;
    let set = if small { SMALL } else { FULL };
    let leaves: Vec<(Vec<Pos2>, Color32)> = set.iter().map(|&(at, c)| (leaf(at), c)).collect();
    // Fit the glyph's own bounds (in SVG units) into `rect`.
    let bounds = leaves
        .iter()
        .flat_map(|(pts, _)| pts.iter())
        .fold(Rect::NOTHING, |r, p| r.union(Rect::from_min_max(*p, *p)));
    let scale = (rect.width() / bounds.width()).min(rect.height() / bounds.height());
    let offset = rect.center() - bounds.center() * scale;
    let to_screen = |p: Pos2| pos2(p.x * scale, p.y * scale) + offset;

    for (pts, color) in leaves {
        let pts = pts.into_iter().map(to_screen).collect();
        painter.add(PathShape::convex_polygon(pts, color, Stroke::NONE));
    }
    if !small {
        // The rib: `M28 76 Q 54 50, 82 18`, stroke 5, in the last leaf's place.
        let at = FULL[2].0;
        let ends =
            [pos2(28.0, 76.0), pos2(54.0, 50.0), pos2(82.0, 18.0)].map(|p| to_screen(at.apply(p)));
        let width = 5.0 * at.s * scale;
        painter.add(QuadraticBezierShape::from_points_stroke(
            ends,
            false,
            Color32::TRANSPARENT,
            Stroke::new(width, bg),
        ));
        for end in [ends[0], ends[2]] {
            painter.circle_filled(end, width / 2.0, bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn leaves_are_closed_and_inside_the_viewbox() {
        for (at, _) in FULL.iter().chain(SMALL.iter()) {
            let pts = leaf(*at);
            assert!(pts.len() > 8);
            let view = Rect::from_min_size(Pos2::ZERO, vec2(256.0, 256.0));
            assert!(pts.iter().all(|p| view.contains(*p)), "{pts:?}");
        }
    }
}
