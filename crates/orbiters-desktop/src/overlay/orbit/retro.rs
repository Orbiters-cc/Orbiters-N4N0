//! The retro look's drawing, for everything these parts paint, in one place: what a part draws in Orbiters' own look is drawn again
//! here, shape by shape, at the end of its frame, so no part has a second drawing to keep in step. It is what N4N0's retro menu
//! ([`menu_retro`](super::super::menu_retro)) does: capitals, cut corners and lines round what can be pressed, a dot grid on the ground,
//! a double frame, icons between brackets, and pictures in dots. What comes out is in greys, as [`in_ramp`](super::panels::in_ramp) puts
//! everything in the colour chosen afterwards.

use super::panels::look;
use egui::epaint::{ClippedShape, CircleShape, Mesh, PathShape, PathStroke, RectShape, TextShape, Vertex, WHITE_UV};
use egui::layers::ShapeIdx;
use egui::{pos2, vec2, Color32, ColorImage, Context, CornerRadius, FontFamily, LayerId, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};

/// N4N0's retro colours as greys: through the ramp they come out at about its own line, dim line and grid.
pub const LINE: Color32 = Color32::from_gray(141);
pub const DIM: Color32 = Color32::from_gray(82);
/// The dim step of a picture's dots: with its gaps it would read darker than the line it is named after.
const DOT_DIM: Color32 = Color32::from_gray(100);
pub const GRID: Color32 = Color32::from_gray(44);
const GROUND: Color32 = Color32::from_gray(8);
const HOT: Color32 = Color32::from_gray(232);

/// The dot grid's step and its dots, as the retro menu has them.
const GRID_STEP: f32 = 16.0;
const GRID_DOT: f32 = 2.0;
/// How far the frame's two lines lie inside a page, as the retro menu has them.
const FRAME_OUTER: f32 = 4.0;
const FRAME_INNER: f32 = 10.0;

/// What a canvas's frames go through.
pub type Pass = fn(&Context, LayerId, Vec2);

/// The look of what `layer` holds so far, on a canvas of `size`. Nothing at all with the modern look.
pub fn pass(ctx: &Context, layer: LayerId, size: Vec2) {
    if !look::retro() {
        return;
    }
    // Taken out rather than changed where they lie: text is laid out again with the context's fonts, which the layers' lock would hold.
    let shapes: Vec<ClippedShape> = ctx.graphics(|g| g.get(layer).map_or_else(Vec::new, |list| list.all_entries().cloned().collect()));
    let mut out = Vec::with_capacity(shapes.len());
    for clipped in shapes {
        let mut shapes = Vec::with_capacity(1);
        restyle(ctx, clipped.shape, size, &mut shapes);
        out.push((clipped.clip_rect, Shape::Vec(shapes)));
    }
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        for (i, (clip, shape)) in out.into_iter().enumerate() {
            list.set(ShapeIdx(i), clip, shape);
        }
    });
}

fn restyle(ctx: &Context, shape: Shape, size: Vec2, out: &mut Vec<Shape>) {
    match shape {
        Shape::Vec(shapes) => {
            for shape in shapes {
                restyle(ctx, shape, size, out);
            }
        }
        Shape::Text(text) => {
            let icon = icon_box(&text);
            out.push(Shape::Text(capitals(ctx, text)));
            if let Some(icon) = icon {
                brackets(out, icon, (icon.width() * 0.22).max(6.0), Stroke::new(1.5, LINE));
            }
        }
        Shape::Rect(rect) => rect_shape(rect, size, out),
        Shape::Circle(circle) => circle_shape(circle, out),
        other => out.push(other),
    }
}

/// Where an icon's brackets go: large icons only, and not one on something lit, which it is cut out of.
fn icon_box(text: &TextShape) -> Option<Rect> {
    let job = &text.galley.job;
    let mut chars = job.text.chars();
    let (glyph, None) = (chars.next()?, chars.next()) else { return None };
    let section = job.sections.first()?;
    let size = section.format.font_id.size;
    let private = ('\u{E000}'..='\u{F8FF}').contains(&glyph);
    (private && size >= 26.0 && luma(section.format.color) > 70.0).then(|| Rect::from_center_size(text.pos + text.galley.size() / 2.0, Vec2::splat(size * 1.12)))
}

/// Monospace text in capitals, as all of the retro menu's is. A letter whose capital takes other bytes stays, as sections are counted in bytes.
fn capitals(ctx: &Context, mut text: TextShape) -> TextShape {
    let job = &text.galley.job;
    if !job.sections.iter().any(|s| s.format.font_id.family == FontFamily::Monospace) || !job.text.chars().any(char::is_lowercase) {
        return text;
    }
    let mut upper = String::with_capacity(job.text.len());
    for c in job.text.chars() {
        let mut big = c.to_uppercase();
        match (big.next(), big.next()) {
            (Some(one), None) if one.len_utf8() == c.len_utf8() => upper.push(one),
            _ => upper.push(c),
        }
    }
    let mut new = (**job).clone();
    new.text = upper;
    text.galley = ctx.fonts_mut(|f| f.layout_job(new));
    text
}

fn luma(c: Color32) -> f32 {
    let a = c.a().max(1) as f32 / 255.0;
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / a
}

/// A dark fill that is solid enough to read as a thing of its own, and not a veil over something.
fn solid_dark(c: Color32) -> bool {
    c.a() >= 170 && luma(c) < 100.0
}

/// A shape's outline: its corners cut where it had them round, at a size of its own.
pub fn cut_corners(rect: Rect, round: CornerRadius, cut: f32) -> Vec<Pos2> {
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let mut points = Vec::with_capacity(8);
    // A corner that stays square is its own point, in place of a pair.
    let mut add = |on: u8, square: Pos2, first: Pos2, second: Pos2| {
        if on > 0 {
            points.push(first);
            points.push(second);
        } else {
            points.push(square);
        }
    };
    add(round.nw, pos2(l, t), pos2(l, t + cut), pos2(l + cut, t));
    add(round.ne, pos2(r, t), pos2(r - cut, t), pos2(r, t + cut));
    add(round.se, pos2(r, b), pos2(r, b - cut), pos2(r - cut, b));
    add(round.sw, pos2(l, b), pos2(l + cut, b), pos2(l, b - cut));
    points
}

fn outline(points: Vec<Pos2>, fill: Color32, stroke: Stroke) -> Shape {
    Shape::Path(PathShape { points, closed: true, fill, stroke: PathStroke::from(stroke) })
}

fn rect_shape(mut shape: RectShape, size: Vec2, out: &mut Vec<Shape>) {
    let canvas = Rect::from_min_size(pos2(0.0, 0.0), size);
    // Shadows are not of this look, and a picture stays a plain rectangle, between its own lines.
    if shape.blur_width > 0.0 {
        return;
    }
    if shape.fill_texture_id() != egui::TextureId::default() {
        shape.corner_radius = CornerRadius::ZERO;
        out.push(Shape::Rect(shape));
        return;
    }
    let rect = shape.rect;
    let (w, h) = (rect.width(), rect.height());
    let small = w.min(h);
    let page = solid_dark(shape.fill) && small >= 200.0 && w >= size.x * 0.85 && h >= size.y * 0.6;
    if page {
        // The sheet's page has the frame: what lies behind it, only an edge round all of it.
        let inner = size.x >= 1000.0;
        page_ground(&shape, canvas, inner, out);
        return;
    }
    // A thing to press or to read has a line: a long row a dim one, as the retro menu's rows do.
    let mut stroke = shape.stroke;
    if stroke.is_empty() && solid_dark(shape.fill) && small >= 18.0 {
        stroke = Stroke::new(if h <= 60.0 && w >= h * 5.0 { 1.0 } else { 1.5 }, if h <= 60.0 && w >= h * 5.0 { DIM } else { LINE });
    }
    let round = shape.corner_radius;
    if round == CornerRadius::ZERO && stroke == shape.stroke {
        out.push(Shape::Rect(shape));
        return;
    }
    // A line right on the canvas's edge would be cut in half.
    let inset = if stroke.is_empty() { 0.0 } else { stroke.width / 2.0 };
    let rect = Rect::from_min_max(pos2(rect.left().max(inset), rect.top().max(inset)), pos2(rect.right().min(size.x - inset), rect.bottom().min(size.y - inset)));
    let cut = if round == CornerRadius::ZERO { 0.0 } else { (small * 0.2).clamp(2.0, 10.0) };
    let round = if cut > 0.0 { round } else { CornerRadius::ZERO };
    out.push(outline(cut_corners(rect, round, cut), shape.fill, stroke));
}

fn circle_shape(circle: CircleShape, out: &mut Vec<Shape>) {
    // Small ones are dots of the grid's kind; others cut as all corners are.
    let r = circle.radius;
    if r < 7.0 {
        out.push(Shape::Rect(RectShape::new(Rect::from_center_size(circle.center, Vec2::splat(r * 1.8)), 0.0, circle.fill, circle.stroke, StrokeKind::Middle)));
        return;
    }
    let stroke = if circle.stroke.is_empty() && solid_dark(circle.fill) && r >= 10.0 { Stroke::new(1.5, LINE) } else { circle.stroke };
    let rect = Rect::from_center_size(circle.center, Vec2::splat(r * 2.0));
    out.push(outline(cut_corners(rect, CornerRadius::same(1), r * 0.55), circle.fill, stroke));
}

/// The ground of a whole page: dark, a grid of dots, and (`framed`) a double frame with ticks along its foot.
fn page_ground(shape: &RectShape, canvas: Rect, framed: bool, out: &mut Vec<Shape>) {
    let rect = shape.rect.intersect(canvas);
    out.push(outline(cut_corners(rect, shape.corner_radius, 14.0), GROUND, Stroke::NONE));
    out.push(Shape::Mesh(std::sync::Arc::new(grid_mesh(rect.shrink(FRAME_INNER + 2.0)))));
    let any = CornerRadius::same(1);
    if !framed {
        out.push(outline(cut_corners(rect.shrink(1.0), any, 13.0), Color32::TRANSPARENT, Stroke::new(2.0, LINE)));
        return;
    }
    out.push(outline(cut_corners(rect.shrink(FRAME_OUTER), any, 12.0), Color32::TRANSPARENT, Stroke::new(2.0, LINE)));
    out.push(outline(cut_corners(rect.shrink(FRAME_INNER), any, 9.0), Color32::TRANSPARENT, Stroke::new(1.0, DIM)));
    let mut x = rect.left() + 32.0;
    let mut n = 0;
    while x < rect.right() - 24.0 {
        let long = if n % 4 == 0 { 5.0 } else { 3.0 };
        out.push(Shape::line_segment([pos2(x, rect.bottom() - FRAME_INNER - long - 1.0), pos2(x, rect.bottom() - FRAME_INNER - 1.0)], Stroke::new(1.0, DIM)));
        x += 32.0;
        n += 1;
    }
}

fn grid_mesh(rect: Rect) -> Mesh {
    let mut mesh = Mesh::default();
    let mut y = rect.top() + GRID_STEP / 2.0;
    while y < rect.bottom() {
        let mut x = rect.left() + GRID_STEP / 2.0;
        while x < rect.right() {
            let at = mesh.vertices.len() as u32;
            for p in [pos2(x, y), pos2(x + GRID_DOT, y), pos2(x + GRID_DOT, y + GRID_DOT), pos2(x, y + GRID_DOT)] {
                mesh.vertices.push(Vertex { pos: p, uv: WHITE_UV, color: GRID });
            }
            mesh.indices.extend_from_slice(&[at, at + 1, at + 2, at, at + 2, at + 3]);
            x += GRID_STEP;
        }
        y += GRID_STEP;
    }
    mesh
}

/// An L at each corner, as the retro menu frames its icons and pictures.
fn brackets(out: &mut Vec<Shape>, r: Rect, length: f32, stroke: Stroke) {
    for (corner, dx, dy) in [(r.left_top(), 1.0, 1.0), (r.right_top(), -1.0, 1.0), (r.right_bottom(), -1.0, -1.0), (r.left_bottom(), 1.0, -1.0)] {
        out.push(Shape::line(vec![corner + vec2(dx * length, 0.0), corner, corner + vec2(0.0, dy * length)], stroke));
    }
}

/// A picture in the retro menu's dots: its ordered dither, in four steps with a dark gap between dots, as squares whatever its shape. In greys.
/// A small one has fewer: a dot under a few pixels only makes a shimmer.
pub fn dots(image: &ColorImage) -> ColorImage {
    use orbiters_n4n0_core::menu::retro::{step, DOTS_X, LOW, SPAN};
    let [width, height] = image.size;
    let across = if width >= 640 { DOTS_X } else { DOTS_X * 2 / 3 };
    let down = (across * height / width.max(1)).clamp(1, 256);
    const SCALE: usize = 3;
    let steps = [GROUND, DOT_DIM, LINE, HOT];
    let mut out = ColorImage::filled([across * SCALE, down * SCALE], GROUND);
    let mut lights = Vec::with_capacity(across * down);
    for dy in 0..down {
        let (y0, y1) = (dy * height / down, ((dy + 1) * height / down).max(dy * height / down + 1).min(height));
        for dx in 0..across {
            let (x0, x1) = (dx * width / across, ((dx + 1) * width / across).max(dx * width / across + 1).min(width));
            let (mut sum, mut count) = (0.0, 0.0);
            for y in y0..y1 {
                for c in &image.pixels[y * width + x0..y * width + x1] {
                    sum += 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
                    count += 1.0;
                }
            }
            lights.push(sum / f32::max(count, 1.0) / 255.0);
        }
    }
    // Four steps have no light to waste: a dim picture is opened up to them, though not as far as to make a dark one bright.
    let mut sorted = lights.clone();
    sorted.sort_unstable_by(f32::total_cmp);
    let low = sorted[sorted.len() / 20].min(0.25);
    let high = sorted[sorted.len() * 97 / 100].max(0.6);
    for dy in 0..down {
        for dx in 0..across {
            let light = LOW + SPAN * (lights[dy * across + dx] - low) / (high - low);
            let shade = steps[step(light, dx, dy)];
            for y in 0..SCALE - 1 {
                let row = (dy * SCALE + y) * across * SCALE + dx * SCALE;
                out.pixels[row..row + SCALE - 1].fill(shade);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corner_left_square_stays_a_corner() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0));
        let head = cut_corners(rect, CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }, 6.0);
        assert_eq!(head, vec![pos2(0.0, 6.0), pos2(6.0, 0.0), pos2(94.0, 0.0), pos2(100.0, 6.0), pos2(100.0, 40.0), pos2(0.0, 40.0)]);
        assert_eq!(cut_corners(rect, CornerRadius::same(4), 6.0).len(), 8);
    }

    /// Light stays in order, a dark picture is mostly ground, and a tall picture's dots stay square.
    #[test]
    fn pictures_in_dots_keep_their_light_and_their_shape() {
        let flat = |shade: u8, size: [usize; 2]| ColorImage::filled(size, Color32::from_gray(shade));
        let lit = |image: &ColorImage| image.pixels.iter().filter(|c| **c != GROUND).count();
        let (dark, mid, bright) = (dots(&flat(10, [320, 180])), dots(&flat(120, [320, 180])), dots(&flat(245, [320, 180])));
        assert!(lit(&dark) < lit(&mid) && lit(&mid) <= lit(&bright));
        assert_eq!(dark.size, [64 * 3, 36 * 3]);
        assert_eq!(dots(&flat(120, [960, 540])).size, [96 * 3, 54 * 3], "as many as N4N0's own");
        assert_eq!(dots(&flat(120, [180, 320])).size, [64 * 3, 113 * 3]);
    }

    /// In capitals, wherever a capital has the same bytes; other letters are left, not broken.
    #[test]
    fn capitals_keep_every_byte_where_it_was() {
        let ctx = Context::default();
        let mut full = ctx.run_ui(Default::default(), |ui| {
            let font = egui::FontId::new(14.0, FontFamily::Monospace);
            let galley = ui.painter().layout_no_wrap("straße é ちま a".to_string(), font, Color32::WHITE);
            let text = TextShape::new(pos2(0.0, 0.0), galley, Color32::WHITE);
            assert_eq!(capitals(ui.ctx(), text).galley.job.text, "STRAßE É ちま A");
            let font = egui::FontId::new(14.0, FontFamily::Proportional);
            let galley = ui.painter().layout_no_wrap("kept".to_string(), font, Color32::WHITE);
            let text = TextShape::new(pos2(0.0, 0.0), galley, Color32::WHITE);
            assert_eq!(capitals(ui.ctx(), text).galley.job.text, "kept");
        });
        full.textures_delta.clear();
    }
}
