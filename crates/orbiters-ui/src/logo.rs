//! The Orbiters mark (paths of the official logo), rasterised with tiny-skia at the exact pixel size so it stays
//! crisp. It can draw itself in (strokes tracing the outline, then the fill) while its cyan and violet echoes
//! slide apart and back like the website's signal reveal, and breathe a violet glow while recording.

use crate::anim::{self, ease};
use crate::theme;
use egui::{pos2, vec2, Color32, ColorImage, Context, Id, Painter, Rect, TextureHandle, TextureOptions, Vec2};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};

const PATHS: [&str; 3] = [
    "M981.949 343.795L584.95 422.295L858.45 467.295C866.283 486.295 877.25 578.595 858.45 671.795C839.65 764.995 802.116 836.629 777.449 883.296C835.949 839.796 943.397 712.253 968.449 631.795C1003.01 520.795 1004.28 415.962 981.949 343.795Z",
    "M424.448 422.295L27.4482 343.795C5.11482 415.962 6.38698 520.795 40.9487 631.795C66.0004 712.253 173.448 839.795 231.948 883.295C207.282 836.629 169.748 764.995 150.948 671.795C132.148 578.595 143.114 486.295 150.948 467.295L424.448 422.295Z",
    "M150.948 244.404C178.038 163.552 279.673 11.001 501.532 12.5111C723.391 14.0213 831.919 165.077 858.45 244.404C781.649 209.571 674.868 91.4406 501.532 91.4406C328.196 91.4406 228.307 209.571 150.948 244.404Z",
];

/// The mark's view box.
pub const VIEW: Vec2 = Vec2::new(1010.0, 900.0);

#[derive(Clone, Copy)]
enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Cubic([f32; 6]),
    Close,
}

fn parse(d: &str) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut nums: Vec<f32> = Vec::new();
    let mut cmd = ' ';
    let flush = |cmd: char, nums: &mut Vec<f32>, out: &mut Vec<Seg>| {
        match cmd {
            'M' => out.push(Seg::Move(nums[0], nums[1])),
            'L' => out.push(Seg::Line(nums[0], nums[1])),
            'C' => {
                for c in nums.as_chunks::<6>().0 {
                    out.push(Seg::Cubic(*c));
                }
            }
            'Z' => out.push(Seg::Close),
            _ => {}
        }
        nums.clear();
    };
    let mut token = String::new();
    for ch in d.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphabetic() || ch == ' ' || ch == ',' {
            if !token.is_empty() {
                nums.push(token.parse().unwrap_or(0.0));
                token.clear();
            }
            if ch.is_ascii_alphabetic() {
                if cmd != ' ' {
                    flush(cmd, &mut nums, &mut out);
                }
                cmd = ch;
            }
        } else if ch == '-' && !token.is_empty() {
            nums.push(token.parse().unwrap_or(0.0));
            token = "-".into();
        } else {
            token.push(ch);
        }
    }
    if cmd != ' ' {
        flush(cmd, &mut nums, &mut out);
    }
    out
}

fn build(segs: &[Seg]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for s in segs {
        match *s {
            Seg::Move(x, y) => pb.move_to(x, y),
            Seg::Line(x, y) => pb.line_to(x, y),
            Seg::Cubic([a, b, c, d, e, f]) => pb.cubic_to(a, b, c, d, e, f),
            Seg::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Outline length (view box units), flattening the curves.
fn length(segs: &[Seg]) -> f32 {
    let (mut x, mut y, mut sx, mut sy, mut len) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for s in segs {
        match *s {
            Seg::Move(nx, ny) => {
                (x, y, sx, sy) = (nx, ny, nx, ny);
            }
            Seg::Line(nx, ny) => {
                len += (nx - x).hypot(ny - y);
                (x, y) = (nx, ny);
            }
            Seg::Cubic([a, b, c, d, e, f]) => {
                let (mut px, mut py) = (x, y);
                for i in 1..=24 {
                    let t = i as f32 / 24.0;
                    let u = 1.0 - t;
                    let qx = u * u * u * x + 3.0 * u * u * t * a + 3.0 * u * t * t * c + t * t * t * e;
                    let qy = u * u * u * y + 3.0 * u * u * t * b + 3.0 * u * t * t * d + t * t * t * f;
                    len += (qx - px).hypot(qy - py);
                    (px, py) = (qx, qy);
                }
                (x, y) = (e, f);
            }
            Seg::Close => {
                len += (sx - x).hypot(sy - y);
                (x, y) = (sx, sy);
            }
        }
    }
    len
}

struct Mark {
    paths: Vec<tiny_skia::Path>,
    lengths: Vec<f32>,
}

fn mark() -> &'static Mark {
    static MARK: std::sync::OnceLock<Mark> = std::sync::OnceLock::new();
    MARK.get_or_init(|| {
        let segs: Vec<Vec<Seg>> = PATHS.iter().map(|d| parse(d)).collect();
        Mark { paths: segs.iter().filter_map(|s| build(s)).collect(), lengths: segs.iter().map(|s| length(s)).collect() }
    })
}

/// Draw-in progress of each path at `t` seconds into the intro: (stroke 0..1, fill 0..1).
fn intro_progress(t: f32, i: usize) -> (f32, f32) {
    let stroke = ease::segment(t, i as f32 * 0.08, 1.1, ease::out_expo);
    let fill = ease::segment(t, 0.65 + i as f32 * 0.08, 0.45, ease::out_cubic);
    (stroke, fill)
}

/// Length of the intro in seconds.
pub const INTRO: f32 = 1.3;

/// The mark in `color`; `outline` adds the 22-unit stroke the interface's mark carries (the echoes are fill only).
fn rasterize(w: u32, h: u32, color: Color32, outline: bool, intro_t: Option<f32>) -> ColorImage {
    let mut pixmap = Pixmap::new(w.max(1), h.max(1)).expect("pixmap");
    let scale = w as f32 / VIEW.x;
    let tr = Transform::from_scale(scale, h as f32 / VIEW.y);
    let m = mark();
    for (i, path) in m.paths.iter().enumerate() {
        let (stroke_k, fill_k) = intro_t.map(|t| intro_progress(t, i)).unwrap_or((1.0, 1.0));
        if fill_k > 0.0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color.r(), color.g(), color.b(), (fill_k * 255.0) as u8);
            paint.anti_alias = true;
            pixmap.fill_path(path, &paint, FillRule::Winding, tr, None);
        }
        if outline && stroke_k > 0.0 {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color.r(), color.g(), color.b(), 255);
            paint.anti_alias = true;
            let len = m.lengths[i];
            let dash = if stroke_k < 1.0 { StrokeDash::new(vec![len * stroke_k, len], 0.0) } else { None };
            let stroke = Stroke { width: 22.0, line_join: LineJoin::Round, line_cap: LineCap::Round, dash, ..Default::default() };
            pixmap.stroke_path(path, &paint, &stroke, tr, None);
        }
    }
    ColorImage::from_rgba_premultiplied([pixmap.width() as usize, pixmap.height() as usize], pixmap.data())
}

#[derive(Clone)]
struct Textures {
    size: [u32; 2],
    mark: TextureHandle,
    cyan: TextureHandle,
    violet: TextureHandle,
}

fn textures(ctx: &Context, size: [u32; 2]) -> Textures {
    let id = Id::unique(("orbiters-logo", size));
    if let Some(t) = ctx.data(|d| d.get_temp::<Textures>(id)) {
        return t;
    }
    let load = |name: &str, c: Color32| {
        ctx.load_texture(format!("orbiters-logo-{name}-{}x{}", size[0], size[1]), rasterize(size[0], size[1], c, name == "mark", None), TextureOptions::LINEAR)
    };
    let t = Textures { size, mark: load("mark", theme::MARK), cyan: load("cyan", theme::CYAN), violet: load("violet", theme::VIOLET) };
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// How the mark moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Look {
    /// Draw-in started at this time ([`anim::time`]); `None` shows the finished mark.
    pub intro_start: Option<f64>,
    /// Signal glitch started at this time, with a strength (1.2 for the intro, 0.8 on hover).
    pub glitch: Option<(f64, f32)>,
    /// Breathing violet glow (while recording).
    pub pulse: bool,
}

/// Size of the mark for a given width (keeps the 1010:900 aspect).
pub fn size_for_width(width: f32) -> Vec2 {
    vec2(width, width * VIEW.y / VIEW.x)
}

/// Paints the mark into `rect`.
pub fn paint(painter: &Painter, rect: Rect, look: Look) {
    let ctx = painter.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
    let tex = textures(&ctx, size);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

    if look.pulse {
        let t = anim::clock(&ctx, 30.0);
        let k = ease::keyframes(ease::mirror(t, 1.2), &[0.0, 1.0], ease::in_out);
        let a = 0.2 + 0.55 * k;
        let s = 0.9 + 0.28 * k;
        // No glow (flat look): the recording state shows in the REC chip and the record button.
        let _ = (s, a);
    }

    // Echoes: x offsets [0, -14s, 8s, 0] and opacity [0, .9, .55, 0] over 0.6 + 0.3s seconds.
    if let Some((start, strength)) = look.glitch {
        let duration = 0.6 + strength as f64 * 0.3;
        let t = anim::since(&ctx, start, duration);
        if t < duration {
            let k = (t / duration) as f32;
            let times = [0.0, 0.3, 0.65, 1.0];
            let dx = ease::keyframes_at(k, &times, &[0.0, -14.0 * strength, 8.0 * strength, 0.0], ease::out_cubic) * rect.width() / 34.0;
            let alpha = ease::keyframes_at(k, &times, &[0.0, 0.9, 0.55, 0.0], ease::out_cubic);
            painter.image(tex.cyan.id(), rect.translate(vec2(dx, 0.0)), uv, Color32::WHITE.gamma_multiply(alpha));
            painter.image(tex.violet.id(), rect.translate(vec2(-dx, 0.0)), uv, Color32::WHITE.gamma_multiply(alpha));
        }
    }

    match look.intro_start {
        Some(start) if (anim::time(&ctx) - start) < INTRO as f64 => {
            let t = anim::since(&ctx, start, INTRO as f64) as f32;
            // A texture per frame only during the intro.
            let image = rasterize(size[0], size[1], theme::MARK, true, Some(t));
            let id = Id::unique(("orbiters-logo-intro", size));
            let handle = ctx.data(|d| d.get_temp::<TextureHandle>(id));
            let handle = match handle {
                Some(mut h) => {
                    h.set(image, TextureOptions::LINEAR);
                    h
                }
                None => {
                    let h = ctx.load_texture("orbiters-logo-intro", image, TextureOptions::LINEAR);
                    ctx.data_mut(|d| d.insert_temp(id, h.clone()));
                    h
                }
            };
            painter.image(handle.id(), rect, uv, Color32::WHITE);
        }
        _ => {
            painter.image(tex.mark.id(), rect, uv, Color32::WHITE);
        }
    }
    let _ = tex.size;
}
