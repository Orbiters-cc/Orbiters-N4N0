//! Painting helpers egui lacks: anti-aliased gradient fills of convex shapes, squircles, arcs, dashed circles and
//! shadows. Colours interpolate in gamma space like CSS, so the web design's gradients carry over.

use egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Shape, Vec2};
use std::f32::consts::TAU;

/// Colour at `t` (0..1) along gradient stops (positions ascending).
pub fn sample(stops: &[(f32, Color32)], t: f32) -> Color32 {
    let Some(first) = stops.first() else { return Color32::TRANSPARENT };
    if t <= first.0 {
        return first.1;
    }
    for w in stops.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            return ca.lerp_to_gamma(cb, (t - a) / (b - a).max(1e-6));
        }
    }
    stops[stops.len() - 1].1
}

/// A linear gradient between two points.
#[derive(Clone, Debug)]
pub struct Gradient {
    pub start: Pos2,
    pub end: Pos2,
    pub stops: Vec<(f32, Color32)>,
}

impl Gradient {
    pub fn new(start: Pos2, end: Pos2, stops: &[(f32, Color32)]) -> Self {
        Self { start, end, stops: stops.to_vec() }
    }

    pub fn horizontal(rect: Rect, stops: &[(f32, Color32)]) -> Self {
        Self::new(rect.left_center(), rect.right_center(), stops)
    }

    pub fn vertical(rect: Rect, stops: &[(f32, Color32)]) -> Self {
        Self::new(rect.center_top(), rect.center_bottom(), stops)
    }

    fn t(&self, p: Pos2) -> f32 {
        let d = self.end - self.start;
        (p - self.start).dot(d) / d.length_sq().max(1e-6)
    }

    pub fn color(&self, p: Pos2) -> Color32 {
        sample(&self.stops, self.t(p))
    }
}

fn feather(painter: &Painter) -> f32 {
    1.0 / painter.ctx().pixels_per_point()
}

/// Outline of a rounded rectangle, clockwise on screen, starting at the top-left corner's arc.
pub fn rounded_rect_points(rect: Rect, radius: f32) -> Vec<Pos2> {
    rounded_rect_points_corners(rect, [radius; 4])
}

/// Outline of a rectangle with its own radius per corner (top-left, top-right, bottom-right, bottom-left).
pub fn rounded_rect_points_corners(rect: Rect, radii: [f32; 4]) -> Vec<Pos2> {
    let limit = (rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let r = radii.map(|r| r.clamp(0.0, limit));
    let mut pts = Vec::with_capacity(4 * 12);
    let corners = [
        (pos2(rect.left() + r[0], rect.top() + r[0]), 180.0f32, r[0]),
        (pos2(rect.right() - r[1], rect.top() + r[1]), 270.0, r[1]),
        (pos2(rect.right() - r[2], rect.bottom() - r[2]), 0.0, r[2]),
        (pos2(rect.left() + r[3], rect.bottom() - r[3]), 90.0, r[3]),
    ];
    for (c, start, r) in corners {
        if r < 0.5 {
            pts.push(c);
            continue;
        }
        let steps = ((r * 0.6).ceil() as usize).clamp(3, 16);
        for i in 0..=steps {
            let a = (start + 90.0 * i as f32 / steps as f32).to_radians();
            pts.push(c + vec2(a.cos(), a.sin()) * r);
        }
    }
    without_repeats(pts)
}

/// Corners that meet (a radius of half the side) end where the next begins: a point repeated (or a hundredth of a point
/// away, from rounding) makes a stroke's join shoot hundreds of points out of the shape.
fn without_repeats(mut pts: Vec<Pos2>) -> Vec<Pos2> {
    const CLOSE: f32 = 0.25 * 0.25;
    pts.dedup_by(|a, b| (*a - *b).length_sq() < CLOSE);
    if pts.len() > 1 && (pts[0] - pts[pts.len() - 1]).length_sq() < CLOSE {
        pts.pop();
    }
    pts
}

/// Outline of a rectangle with squircle corners, as the website's `corner-shape: squircle` (a superellipse of
/// exponent 4): corner `r` is where the curve leaves each edge, like `border-radius` with that shape. A squircle
/// looks about half as round as a circular corner of the same radius (see [`crate::theme::squircle`]).
pub fn squircle_points_corners(rect: Rect, radii: [f32; 4]) -> Vec<Pos2> {
    let limit = (rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let r = radii.map(|r| r.clamp(0.0, limit));
    let mut pts = Vec::with_capacity(4 * 25);
    let corners = [
        (pos2(rect.left() + r[0], rect.top() + r[0]), 180.0f32, r[0]),
        (pos2(rect.right() - r[1], rect.top() + r[1]), 270.0, r[1]),
        (pos2(rect.right() - r[2], rect.bottom() - r[2]), 0.0, r[2]),
        (pos2(rect.left() + r[3], rect.bottom() - r[3]), 90.0, r[3]),
    ];
    // |x|^4 + |y|^4 = 1: (sign(cos)·√|cos|, sign(sin)·√|sin|).
    let root = |v: f32| v.signum() * v.abs().sqrt();
    for (c, start, r) in corners {
        if r < 0.5 {
            pts.push(c);
            continue;
        }
        let steps = ((r * 0.5).ceil() as usize).clamp(8, 24);
        for i in 0..=steps {
            let a = (start + 90.0 * i as f32 / steps as f32).to_radians();
            pts.push(c + vec2(root(a.cos()), root(a.sin())) * r);
        }
    }
    without_repeats(pts)
}

pub fn squircle_points(rect: Rect, radius: f32) -> Vec<Pos2> {
    squircle_points_corners(rect, [radius; 4])
}

/// A rectangle with squircle corners, filled.
pub fn fill_squircle(painter: &Painter, rect: Rect, radius: f32, color: Color32) {
    painter.add(Shape::mesh(squircle_mesh(rect, radius, color, feather(painter))));
}

/// The corner radius for a shape `inner` inside a squircle frame, for the two outlines to run side by side: the corner
/// that leaves the same centre as the frame's. `frame_radius` as given to [`fill_squircle`] (it is limited there to half
/// the frame's shorter side, and so here); the gap is the one above and below `inner`, its sides being free to sit anywhere
/// along a row (a segmented control's middle choice).
pub fn nested_radius(frame: Rect, frame_radius: f32, inner: Rect) -> f32 {
    let limit = (frame.width() * 0.5).min(frame.height() * 0.5).max(0.0);
    let gap = (inner.top() - frame.top()).min(frame.bottom() - inner.bottom()).max(0.0);
    (frame_radius.min(limit) - gap).max(0.0)
}

/// A highlight inside a squircle frame (a segmented control's chosen half): the same corner shape as the frame's and
/// concentric with it. A circular corner inside a squircle one never follows it, whatever its radius.
pub fn fill_nested_squircle(painter: &Painter, frame: Rect, frame_radius: f32, inner: Rect, color: Color32) {
    fill_squircle(painter, inner, nested_radius(frame, frame_radius, inner), color);
}

/// The fill of [`fill_squircle`] as a mesh (for shapes set later with `Painter::set`); `feather` in points.
pub fn squircle_mesh(rect: Rect, radius: f32, color: Color32, feather: f32) -> Mesh {
    convex_mesh(&squircle_points(rect, radius), |_| color, feather * 0.5)
}

/// A squircle outline drawn inside `rect`.
pub fn stroke_squircle(painter: &Painter, rect: Rect, radius: f32, stroke: egui::Stroke) {
    let inner = rect.shrink(stroke.width * 0.5);
    painter.add(Shape::closed_line(squircle_points(inner, (radius - stroke.width * 0.5).max(0.0)), stroke));
}

/// A texture (its `uv` part) in a rectangle with squircle corners, anti-aliased at the edge.
pub fn squircle_texture(painter: &Painter, rect: Rect, radii: [f32; 4], texture: egui::TextureId, uv: Rect, tint: Color32) {
    texture_in(painter, &squircle_points_corners(rect, radii), rect, texture, uv, tint);
}

/// A texture (its `uv` part) in the circle inscribed in `rect` (people's pictures), anti-aliased at the edge.
pub fn circle_texture(painter: &Painter, rect: Rect, texture: egui::TextureId, uv: Rect, tint: Color32) {
    let (c, r) = (rect.center(), rect.width().min(rect.height()) * 0.5);
    let steps = ((r * 1.2).ceil() as usize).clamp(16, 64);
    let points: Vec<Pos2> = (0..steps).map(|i| c + Vec2::angled(std::f32::consts::TAU * i as f32 / steps as f32) * r).collect();
    texture_in(painter, &points, rect, texture, uv, tint);
}

/// A texture (its `uv` part, spread over `rect`) inside a convex outline that runs clockwise on screen.
fn texture_in(painter: &Painter, points: &[Pos2], rect: Rect, texture: egui::TextureId, uv: Rect, tint: Color32) {
    let n = points.len();
    if n < 3 {
        return;
    }
    let f = feather(painter) * 0.5;
    let uv_at = |p: Pos2| {
        let t = (p - rect.min) / rect.size().max(Vec2::splat(1e-3));
        pos2(uv.min.x + t.x * uv.width(), uv.min.y + t.y * uv.height())
    };
    let mut mesh = Mesh::with_texture(texture);
    let push = |mesh: &mut Mesh, p: Pos2, c: Color32| mesh.vertices.push(egui::epaint::Vertex { pos: p, uv: uv_at(p), color: c });
    let centroid = (points.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / n as f32).to_pos2();
    push(&mut mesh, centroid, tint);
    for i in 0..n {
        let (prev, p, next) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        let (e1, e2) = ((p - prev).normalized(), (next - p).normalized());
        // Clockwise on screen (y down): (-e.y, e.x) points inside.
        let inward = (vec2(-e1.y, e1.x) + vec2(-e2.y, e2.x)).normalized();
        push(&mut mesh, p + inward * f, tint);
        push(&mut mesh, p - inward * f, Color32::TRANSPARENT);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        let (ii, io, ji, jo) = (1 + 2 * i, 2 + 2 * i, 1 + 2 * j, 2 + 2 * j);
        mesh.add_triangle(0, ii, ji);
        mesh.add_triangle(ii, io, jo);
        mesh.add_triangle(ii, jo, ji);
    }
    painter.add(Shape::mesh(mesh));
}

/// Fills a convex polygon, colouring each vertex with `color_at` and feathering the edge by one pixel.
pub fn fill_convex(painter: &Painter, points: &[Pos2], color_at: impl Fn(Pos2) -> Color32) {
    if points.len() < 3 {
        return;
    }
    painter.add(Shape::mesh(convex_mesh(points, color_at, feather(painter) * 0.5)));
}

/// The anti-aliased fill of a convex polygon, `f` points of feather on each side of the edge.
pub fn convex_mesh(points: &[Pos2], color_at: impl Fn(Pos2) -> Color32, f: f32) -> Mesh {
    let n = points.len();
    if n < 3 {
        return Mesh::default();
    }
    // Orientation decides which side of each edge is outside.
    let area: f32 = (0..n).map(|i| points[i].to_vec2().x * points[(i + 1) % n].y - points[(i + 1) % n].x * points[i].y).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut mesh = Mesh::default();
    let centroid = points.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / n as f32;
    mesh.colored_vertex(centroid.to_pos2(), color_at(centroid.to_pos2()));
    for i in 0..n {
        let prev = points[(i + n - 1) % n];
        let next = points[(i + 1) % n];
        let p = points[i];
        let e1 = (p - prev).normalized();
        let e2 = (next - p).normalized();
        let n1 = vec2(e1.y, -e1.x) * sign;
        let n2 = vec2(e2.y, -e2.x) * sign;
        let normal = (n1 + n2).normalized();
        let c = color_at(p);
        mesh.colored_vertex(p - normal * f, c);
        mesh.colored_vertex(p + normal * f, Color32::TRANSPARENT);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        let (ii, io, ji, jo) = (1 + 2 * i, 2 + 2 * i, 1 + 2 * j, 2 + 2 * j);
        mesh.add_triangle(0, ii, ji);
        mesh.add_triangle(ii, io, jo);
        mesh.add_triangle(ii, jo, ji);
    }
    mesh
}

/// Splits a convex polygon where the gradient crosses its stops, so every piece is exactly affine in colour.
fn clip_half(points: &[Pos2], keep: impl Fn(Pos2) -> f32) -> Vec<Pos2> {
    let mut out = Vec::with_capacity(points.len() + 2);
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let (da, db) = (keep(a), keep(b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

/// Fills a convex polygon with a multi-stop linear gradient (exact between stops), anti-aliased.
pub fn fill_gradient(painter: &Painter, points: &[Pos2], g: &Gradient) {
    let inner: Vec<f32> = g.stops.iter().map(|s| s.0).filter(|t| *t > 0.0 && *t < 1.0).collect();
    if inner.is_empty() {
        fill_convex(painter, points, |p| g.color(p));
        return;
    }
    // Interior pieces without feathering, then one feathered outline on top.
    let mut bounds = vec![f32::NEG_INFINITY];
    bounds.extend(inner);
    bounds.push(f32::INFINITY);
    let d = g.end - g.start;
    let len2 = d.length_sq().max(1e-6);
    let t_of = |p: Pos2| (p - g.start).dot(d) / len2;
    let mut mesh = Mesh::default();
    for w in bounds.windows(2) {
        let (lo, hi) = (w[0], w[1]);
        let piece = clip_half(&clip_half(points, |p| t_of(p) - lo), |p| hi - t_of(p));
        if piece.len() < 3 {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        for p in &piece {
            mesh.colored_vertex(*p, g.color(*p));
        }
        for k in 1..piece.len() as u32 - 1 {
            mesh.add_triangle(base, base + k, base + k + 1);
        }
    }
    painter.add(Shape::mesh(mesh));
    outline_feather(painter, points, |p| g.color(p));
}

/// A one-pixel anti-aliasing band around a convex polygon (for meshes painted without feathering).
fn outline_feather(painter: &Painter, points: &[Pos2], color_at: impl Fn(Pos2) -> Color32) {
    let f = feather(painter);
    let n = points.len();
    let area: f32 = (0..n).map(|i| points[i].x * points[(i + 1) % n].y - points[(i + 1) % n].x * points[i].y).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut mesh = Mesh::default();
    for i in 0..n {
        let prev = points[(i + n - 1) % n];
        let next = points[(i + 1) % n];
        let p = points[i];
        let e1 = (p - prev).normalized();
        let e2 = (next - p).normalized();
        let normal = (vec2(e1.y, -e1.x) * sign + vec2(e2.y, -e2.x) * sign).normalized();
        mesh.colored_vertex(p, color_at(p));
        mesh.colored_vertex(p + normal * f, Color32::TRANSPARENT);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        mesh.add_triangle(2 * i, 2 * i + 1, 2 * j + 1);
        mesh.add_triangle(2 * i, 2 * j + 1, 2 * j);
    }
    painter.add(Shape::mesh(mesh));
}

/// CSS-like box shadow of a rounded rectangle: a gaussian fall-off reaching `blur` points past its edge, made of rings
/// of the shape grown and shrunk. egui's blurred rectangles crease on shapes thinner than their blur (a hard line along
/// the middle, streaks under the corners: a tooltip's shadow); here a thin shape's shadow is the blurred box across its
/// narrower side, lighter in the middle and smooth through it.
pub fn shadow(painter: &Painter, rect: Rect, radius: f32, offset: Vec2, blur: f32, spread: f32, color: Color32) {
    let r = rect.translate(offset).expand(spread);
    if !r.is_finite() || r.width() <= 0.0 || r.height() <= 0.0 || color.a() == 0 {
        return;
    }
    painter.add(Shape::mesh(shadow_mesh(r, radius + spread.max(0.0), blur, color)));
}

/// Rings of [`shadow`], from inside the shape (or its middle line) to `blur` outside it.
const SHADOW_RINGS: usize = 12;
/// Points on each ring's quarter circles.
const SHADOW_ARC: usize = 7;

fn shadow_mesh(r: Rect, radius: f32, blur: f32, color: Color32) -> Mesh {
    let blur = blur.max(0.5);
    let half = 0.5 * r.width().min(r.height());
    // The blurred box across the narrower side at a signed distance `d` from the edge (outside positive), the kernel a
    // gaussian of sigma blur / 2 cut at ±blur.
    let edge = normal_cdf(-2.0);
    let kernel = |u: f32| ((normal_cdf(2.0 * u / blur) - edge) / (1.0 - 2.0 * edge)).clamp(0.0, 1.0);
    let value = |d: f32| (kernel(-d) - kernel(-2.0 * half - d)).clamp(0.0, 1.0);
    let inner = (-blur).max(-half);
    let n = 4 * (SHADOW_ARC + 1);
    let mut mesh = Mesh::default();
    for k in 0..SHADOW_RINGS {
        let d = inner + (blur - inner) * k as f32 / (SHADOW_RINGS - 1) as f32;
        let ring = r.expand(d);
        let cr = (radius + d).clamp(0.0, 0.5 * ring.width().min(ring.height()));
        let c = color.gamma_multiply(value(d));
        // Corners clockwise from the bottom right, each a quarter circle (one point when sharp).
        let centres = [
            pos2(ring.right() - cr, ring.bottom() - cr),
            pos2(ring.left() + cr, ring.bottom() - cr),
            pos2(ring.left() + cr, ring.top() + cr),
            pos2(ring.right() - cr, ring.top() + cr),
        ];
        for (q, centre) in centres.iter().enumerate() {
            for j in 0..=SHADOW_ARC {
                let a = (q as f32 + j as f32 / SHADOW_ARC as f32) * TAU / 4.0;
                mesh.colored_vertex(*centre + Vec2::angled(a) * cr, c);
            }
        }
    }
    for k in 0..SHADOW_RINGS as u32 - 1 {
        let (a, b) = (k * n as u32, (k + 1) * n as u32);
        for i in 0..n as u32 {
            let j = (i + 1) % n as u32;
            mesh.add_triangle(a + i, b + i, b + j);
            mesh.add_triangle(a + i, b + j, a + j);
        }
    }
    // The innermost ring filled (a line, no area, on shapes thinner than twice the blur).
    let centre = mesh.vertices.len() as u32;
    mesh.colored_vertex(r.center(), color.gamma_multiply(value(inner)));
    for i in 0..n as u32 {
        mesh.add_triangle(centre, i, (i + 1) % n as u32);
    }
    mesh
}

/// The standard normal distribution's cumulative function (Abramowitz and Stegun 7.1.26, within 1.5e-7).
fn normal_cdf(x: f32) -> f32 {
    let z = x.abs() / std::f32::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4))));
    let erf = 1.0 - poly * (-z * z).exp();
    0.5 * (1.0 + if x >= 0.0 { erf } else { -erf })
}

/// A stroked arc with round caps. Angles in radians, 0 = 3 o'clock, positive = clockwise on screen.
pub fn arc(painter: &Painter, center: Pos2, radius: f32, start: f32, sweep: f32, width: f32, color: Color32) {
    arc_colored(painter, center, radius, start, sweep, width, |_| color);
}

/// A stroked arc whose colour comes from its position (for gradient strokes), round caps, anti-aliased.
pub fn arc_colored(painter: &Painter, center: Pos2, radius: f32, start: f32, sweep: f32, width: f32, color_at: impl Fn(Pos2) -> Color32) {
    if sweep.abs() < 1e-4 || width <= 0.0 {
        return;
    }
    let f = feather(painter);
    let half = width * 0.5;
    let segs = ((sweep.abs() * radius / 3.0).ceil() as usize).clamp(4, 256);
    let mut mesh = Mesh::default();
    for i in 0..=segs {
        let a = start + sweep * i as f32 / segs as f32;
        let dir = Vec2::angled(a);
        let p = center + dir * radius;
        let c = color_at(p);
        // outer feather, outer, inner, inner feather
        mesh.colored_vertex(center + dir * (radius + half + f * 0.5), Color32::TRANSPARENT);
        mesh.colored_vertex(center + dir * (radius + half - f * 0.5), c);
        mesh.colored_vertex(center + dir * (radius - half + f * 0.5).max(0.0), c);
        mesh.colored_vertex(center + dir * (radius - half - f * 0.5).max(0.0), Color32::TRANSPARENT);
    }
    for i in 0..segs as u32 {
        let a = 4 * i;
        let b = a + 4;
        for k in 0..3 {
            mesh.add_triangle(a + k, b + k, b + k + 1);
            mesh.add_triangle(a + k, b + k + 1, a + k + 1);
        }
    }
    painter.add(Shape::mesh(mesh));
    // Round caps.
    for a in [start, start + sweep] {
        let p = center + Vec2::angled(a) * radius;
        painter.circle_filled(p, half, color_at(p));
    }
}

/// A dashed circle (`pattern` = dash and gap lengths in points), turned by `rotation` radians.
pub fn dashed_circle(painter: &Painter, center: Pos2, radius: f32, pattern: [f32; 2], width: f32, color: Color32, rotation: f32) {
    let [dash, gap] = pattern;
    let circumference = TAU * radius;
    // No pattern (or no circle) would ask for endless dashes: a plain ring then.
    let period = dash + gap;
    if period.is_nan() || period <= 0.0 || !circumference.is_finite() {
        ring(painter, center, radius, width, color);
        return;
    }
    let count = ((circumference / period).floor().max(1.0) as usize).min(4096);
    let step = TAU / count as f32;
    let dash_angle = step * dash / (dash + gap);
    for i in 0..count {
        let a = rotation + step * i as f32;
        let p0 = center + Vec2::angled(a) * radius;
        let p1 = center + Vec2::angled(a + dash_angle) * radius;
        painter.line_segment([p0, p1], egui::Stroke::new(width, color));
    }
}

/// A thin circle outline, anti-aliased by egui.
pub fn ring(painter: &Painter, center: Pos2, radius: f32, width: f32, color: Color32) {
    painter.circle_stroke(center, radius, egui::Stroke::new(width, color));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How far the highlight's outline runs from the frame's, around their top-left corners.
    fn gaps(frame: Rect, frame_r: f32, inner: Rect, inner_points: &[Pos2]) -> (f32, f32) {
        let outer = squircle_points(frame, frame_r);
        let corner = |p: &&Pos2| p.x < inner.left() + nested_radius(frame, frame_r, inner) && p.y < inner.top() + nested_radius(frame, frame_r, inner);
        let distances: Vec<f32> = inner_points.iter().filter(corner).map(|p| outer.iter().map(|q| p.distance(*q)).fold(f32::MAX, f32::min)).collect();
        assert!(distances.len() > 4);
        (distances.iter().copied().fold(f32::MAX, f32::min), distances.iter().copied().fold(0.0, f32::max))
    }

    #[test]
    fn a_nested_highlight_follows_its_frames_corner() {
        // The preview window's light switch: frame 38 high, highlight inset by 4.
        let frame = Rect::from_min_size(pos2(100.0, 50.0), vec2(240.0, 38.0));
        let r = crate::theme::squircle(19.0);
        let inner = Rect::from_min_size(pos2(104.0, 54.0), vec2(116.0, 30.0));
        assert_eq!(nested_radius(frame, r, inner), 15.0);
        let (near, far) = gaps(frame, r, inner, &squircle_points(inner, nested_radius(frame, r, inner)));
        assert!(near >= 3.5 && far <= 5.5, "the gap stays about 4 all the way round the corner: {near}..{far}");
        // What it was: a circular corner, which pinches the gap at the corner's start and opens it at the diagonal.
        let (old_near, old_far) = gaps(frame, r, inner, &rounded_rect_points(inner, 15.0));
        assert!(old_far - old_near > far - near + 1.0, "circle {old_near}..{old_far}, squircle {near}..{far}");
        // A frame's radius past half its height counts for what is drawn; a square frame leaves nothing to follow.
        assert_eq!(nested_radius(frame, 500.0, inner), 15.0);
        assert_eq!(nested_radius(frame, 2.0, inner), 0.0);
    }

    #[test]
    fn shadows_fall_off_smoothly_even_on_shapes_thinner_than_their_blur() {
        let black = Color32::from_black_alpha(200);
        for (rect, radius, blur) in [
            (Rect::from_min_size(pos2(10.0, 10.0), vec2(120.0, 28.0)), 14.0, 40.0),
            (Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0)), 24.0, 48.0),
            (Rect::from_min_size(pos2(5.0, 5.0), vec2(3.0, 3.0)), 0.0, 20.0),
        ] {
            let mesh = shadow_mesh(rect, radius, blur, black);
            assert!(mesh.vertices.iter().all(|v| v.pos.is_finite() && rect.expand(blur + 0.01).contains(v.pos)));
            assert!(mesh.vertices.iter().all(|v| v.color.a() <= 200));
            assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.vertices.len()));
            // Nothing at its outer edge; the middle darkest.
            let outer = &mesh.vertices[(SHADOW_RINGS - 1) * 4 * (SHADOW_ARC + 1)..SHADOW_RINGS * 4 * (SHADOW_ARC + 1)];
            assert!(outer.iter().all(|v| v.color.a() == 0));
            let centre = mesh.vertices.last().unwrap().color.a();
            assert!(mesh.vertices.iter().all(|v| v.color.a() <= centre));
        }
        // A large shape is fully dark inside, a thin one lighter (its blurred box).
        assert_eq!(shadow_mesh(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0)), 24.0, 48.0, black).vertices.last().unwrap().color.a(), 200);
        let thin = shadow_mesh(Rect::from_min_size(pos2(0.0, 0.0), vec2(120.0, 28.0)), 14.0, 40.0, black).vertices.last().unwrap().color.a();
        assert!(thin > 60 && thin < 180, "{thin}");
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-6 && (normal_cdf(1.96) - 0.975).abs() < 1e-3 && (normal_cdf(-1.96) - 0.025).abs() < 1e-3);
    }

    #[test]
    fn outlines_with_meeting_corners_repeat_no_point() {
        // A 64-point high bar with corners of half its height (the Visited worlds selection bar).
        let rect = Rect::from_min_size(pos2(437.0, 732.0), vec2(640.0, 64.0));
        for pts in [squircle_points(rect.shrink(0.5), 43.5), rounded_rect_points(rect, 40.0)] {
            for (i, p) in pts.iter().enumerate() {
                let next = pts[(i + 1) % pts.len()];
                assert!((*p - next).length() >= 0.25, "point {i} repeats");
                assert!(rect.expand(0.01).contains(*p));
            }
            // Its stroke stays on the shape.
            let mut mesh = Mesh::default();
            let mut tess = egui::epaint::Tessellator::new(1.0, egui::epaint::TessellationOptions::default(), [1, 1], vec![]);
            tess.tessellate_shape(Shape::closed_line(pts, egui::Stroke::new(1.0, Color32::WHITE)), &mut mesh);
            assert!(mesh.vertices.iter().all(|v| rect.expand(2.0).contains(v.pos)));
        }
    }
}
