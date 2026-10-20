//! Top-down map of a camera path (Unity X to the right, Z up the screen). The view springs towards the path's
//! bounds so new points glide in; the current position carries a heading cone and, while recording, a pulse.

use crate::anim::{self, Spring};
use crate::theme::{self, Txt};
use crate::widgets;
use egui::epaint::{PathShape, PathStroke};
use egui::{pos2, vec2, Align2, Color32, Id, Mesh, Pos2, Rect, Shape, Stroke, Ui};

pub struct Trail<'a> {
    /// Camera positions, Unity world metres (x, y, z).
    pub points: &'a [[f32; 3]],
    /// Heading in degrees (0 = +Z, clockwise towards +X), for the cone at the last point.
    pub heading: Option<f32>,
    /// Recording right now (pulse on the last point).
    pub live: bool,
    pub empty_text: &'a str,
}

pub fn show(ui: &Ui, rect: Rect, id: Id, trail: &Trail<'_>) {
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    crate::widgets::frame(&p, rect, theme::TILE_RADIUS, theme::surface_soft());
    let aspect = rect.width() / rect.height().max(1.0);

    // Target view: the path's bounds with a margin, at the box's aspect ratio.
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for q in trail.points {
        min_x = min_x.min(q[0]);
        max_x = max_x.max(q[0]);
        min_z = min_z.min(q[2]);
        max_z = max_z.max(q[2]);
    }
    let target = if trail.points.is_empty() {
        [0.0, 0.0, 10.0]
    } else {
        let span = 6.0f32.max((max_x - min_x) * 1.25).max((max_z - min_z) * 1.25 * aspect);
        [(min_x + max_x) * 0.5, -(min_z + max_z) * 0.5, span]
    };
    let [cx, cy, span] = anim::spring_eps(ui.ctx(), id.with("view"), target, target, Spring::SOFT, (target[2] * 0.0005).max(0.001));
    let scale = rect.width() / span.max(0.001);
    let to_screen = |x: f32, z: f32| pos2(rect.center().x + (x - cx) * scale, rect.center().y + (-z - cy) * scale);

    // Grid.
    let w = span;
    let h = span / aspect;
    let grid = if w > 60.0 {
        10.0
    } else if w > 20.0 {
        5.0
    } else {
        1.0
    };
    let stroke = Stroke::new(1.0, theme::white(0.045));
    let x0 = cx - w * 0.5;
    let y0 = cy - h * 0.5;
    let mut gx = (x0 / grid).floor() * grid;
    while gx < x0 + w {
        let sx = rect.center().x + (gx - cx) * scale;
        p.vline(sx, rect.y_range(), stroke);
        gx += grid;
    }
    let mut gy = (y0 / grid).floor() * grid;
    while gy < y0 + h {
        let sy = rect.center().y + (gy - cy) * scale;
        p.hline(rect.x_range(), sy, stroke);
        gy += grid;
    }

    // Path: a crisp line, grey where it is old, green where it is new.
    if trail.points.len() > 1 {
        let pts: Vec<Pos2> = trail.points.iter().map(|q| to_screen(q[0], q[2])).collect();
        let (lo, hi) = (to_screen(min_x, 0.0).x, to_screen(max_x, 0.0).x);
        let colour = move |x: f32, a: f32| {
            let t = if hi - lo > 1.0 { (x - lo) / (hi - lo) } else { 0.5 };
            // Older parts of the path fade to grey, the latest is Orbiters green.
            theme::alpha(theme::mix(theme::FG_4, theme::ACCENT, t), a)
        };
        p.add(PathShape::line(pts, PathStroke::new_uv(2.4, move |_r: Rect, pt: Pos2| colour(pt.x, 1.0))));
    }

    if let Some(last) = trail.points.last() {
        let at = to_screen(last[0], last[2]);
        if let Some(heading) = trail.heading {
            cone(&p, at, heading.to_radians(), 46.0);
        }
        if trail.live && anim::is_live(ui.ctx()) {
            let t = anim::clock(ui.ctx(), 30.0);
            let k = ((t % 1.6) / 1.6) as f32;
            let e = anim::ease::out_cubic(k);
            p.circle_stroke(at, 4.0 + 11.0 * e, Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.7 * (1.0 - e))));
        }
        p.circle_filled(at, 4.4, Color32::WHITE);
        p.circle_stroke(at, 4.4, Stroke::new(1.6, theme::ACCENT));
    }

    if trail.points.is_empty() {
        widgets::text(&p, rect.center(), Align2::CENTER_CENTER, &Txt::new(trail.empty_text, 12.5).color(theme::FG_4));
    }

    // Scale legend.
    let ly = rect.bottom() - 14.0;
    p.hline((rect.left() + 12.0)..=(rect.left() + 12.0 + 32.0), ly, Stroke::new(1.0, theme::FG_4));
    let unit = if grid >= 1.0 { format!("{grid:.0} m") } else { format!("{grid} m") };
    widgets::text(&p, pos2(rect.left() + 52.0, ly), Align2::LEFT_CENTER, &Txt::new(unit, 10.5).color(theme::FG_4));
}

/// A heading cone fading out from the camera: a fan of triangles with a radial alpha falloff.
fn cone(p: &egui::Painter, at: Pos2, heading: f32, length: f32) {
    let half = 0.42;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(at, theme::alpha(theme::ACCENT, 0.4));
    let n = 12;
    // Direction: 0 = up the screen (+Z), clockwise.
    for i in 0..=n {
        let a = heading - half + 2.0 * half * i as f32 / n as f32;
        let dir = vec2(a.sin(), -a.cos());
        mesh.colored_vertex(at + dir * length * 0.5, theme::alpha(theme::ACCENT, 0.15));
        mesh.colored_vertex(at + dir * length, Color32::TRANSPARENT);
    }
    for i in 0..n as u32 {
        let (a_mid, a_far, b_mid, b_far) = (1 + 2 * i, 2 + 2 * i, 3 + 2 * i, 4 + 2 * i);
        mesh.add_triangle(0, a_mid, b_mid);
        mesh.add_triangle(a_mid, a_far, b_far);
        mesh.add_triangle(a_mid, b_far, b_mid);
    }
    p.add(Shape::mesh(mesh));
}
