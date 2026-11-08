//! A thumbstick as a real thing in the room, for what says how to zoom a capture standing small (Enzo: "a nice looking 3D not completely opaque element with a nice animation"). Traced for each
//! eye from where that eye is, so it stands at its depth and turns as a head moves round it: a flat picture cannot. Translucent glass over any world, with no frame behind it: its rim and its
//! highlights read on a dark world, its tinted body on a bright one.

use super::stereo::{self, V3};
use egui::Color32;

/// One eye's picture.
pub const EYE_PX: [u32; 2] = [240, 264];
/// How large it is in the room.
pub const WIDTH_M: f32 = 0.2;
pub const HEIGHT_M: f32 = WIDTH_M * EYE_PX[1] as f32 / EYE_PX[0] as f32;

/// One push up and pull down, in seconds.
pub const CYCLE_S: f32 = 2.8;

/// How far it is pushed `seconds` in, up positive: pushed with a little overshoot as a thumb does, held a moment, let back past the middle a little, and the other way.
pub fn push_at(seconds: f32) -> f32 {
    let t = (seconds / CYCLE_S).fract();
    // Towards `to` from `from` over a part of the cycle, on a spring that has settled by its end.
    let spring = |from: f32, to: f32, x: f32| {
        let x = x.clamp(0.0, 1.0);
        from + (to - from) * (1.0 - (-7.0 * x).exp() * (9.0 * x).cos())
    };
    match t {
        t if t < 0.42 => spring(0.0, 1.0, t / 0.2),
        t if t < 0.5 => spring(1.0, 0.0, (t - 0.42) / 0.16),
        t if t < 0.92 => spring(0.0, -1.0, (t - 0.5) / 0.2),
        t => spring(-1.0, 0.0, (t - 0.92) / 0.16),
    }
}

/// A part: a capsule between two points, of a radius, in a space squashed along up by `flat`, of a colour, how solid it is at its most and how much it shines of itself.
#[derive(Clone, Copy)]
struct Part {
    a: V3,
    b: V3,
    radius: f32,
    flat: f32,
    colour: [f32; 3],
    solid: f32,
    glow: f32,
}

/// How much the whole leans towards who looks, as a stick seen from a little above: at eye level its push would be lost.
const LEANS: f32 = 0.5;

fn leaned(p: V3) -> V3 {
    let (s, c) = LEANS.sin_cos();
    [p[0], p[1] * c - p[2] * s, p[1] * s + p[2] * c]
}

/// How much larger than a real stick it is shown, to be seen at arm's length.
const LARGER: f32 = 1.45;

fn parts(push: f32, accent: [f32; 3]) -> Vec<Part> {
    parts_of(push, accent)
        .into_iter()
        .map(|part| Part { a: stereo::scale(part.a, LARGER), b: stereo::scale(part.b, LARGER), radius: part.radius * LARGER, ..part })
        .collect()
}

fn parts_of(push: f32, accent: [f32; 3]) -> Vec<Part> {
    const BODY: [f32; 3] = [0.42, 0.48, 0.58];
    const LIGHT: [f32; 3] = [0.62, 0.68, 0.78];
    let tilt = push * 0.62;
    let (s, c) = tilt.sin_cos();
    const FOOT: f32 = -0.034;
    let up = |height: f32| leaned([0.0, FOOT + height * c, -height * s]);
    let base = leaned([0.0, FOOT, 0.0]);
    let cap = up(0.06);
    let pushed = push.abs().clamp(0.0, 1.0);
    let cap_colour: [f32; 3] = std::array::from_fn(|i| LIGHT[i] + (accent[i] - LIGHT[i]) * pushed * 0.8);
    let mut out = vec![
        // Its housing, then its boot, its stem and its cap.
        Part { a: base, b: base, radius: 0.036, flat: 0.16, colour: BODY, solid: 0.55, glow: 0.0 },
        Part { a: base, b: up(0.012), radius: 0.013, flat: 0.7, colour: BODY, solid: 0.5, glow: 0.0 },
        Part { a: base, b: cap, radius: 0.0042, flat: 1.0, colour: LIGHT, solid: 0.6, glow: 0.0 },
        Part { a: cap, b: cap, radius: 0.0165, flat: 0.5, colour: cap_colour, solid: 0.72, glow: 0.1 * pushed },
    ];
    // Which way it goes: a chevron beyond it and one before it, lit as it goes that way.
    for (at, point, lit) in [(-0.05f32, -0.01f32, ((push - 0.1) / 0.9).clamp(0.0, 1.0)), (0.05, 0.01, ((-push - 0.1) / 0.9).clamp(0.0, 1.0))] {
        let tip = leaned([0.0, FOOT + 0.002, at + point]);
        let colour: [f32; 3] = std::array::from_fn(|i| LIGHT[i] + (accent[i] - LIGHT[i]) * lit);
        for side in [-0.011f32, 0.011] {
            let end = leaned([side, FOOT + 0.002, at]);
            out.push(Part { a: tip, b: end, radius: 0.0024, flat: 1.0, colour, solid: 0.22 + 0.6 * lit, glow: 0.45 * lit });
        }
    }
    out
}

/// The light it stands in, from above, to the left, towards who looks.
const LIGHT_FROM: V3 = [-0.42, 0.79, 0.45];

/// Where along a ray a part is hit, and how much of a pixel it covers, and its colour there: premultiplied, with how solid.
fn hit(part: &Part, from: V3, along: V3, pixel: f32) -> Option<(f32, [f32; 3], f32)> {
    let squash = |p: V3| [p[0], p[1] / part.flat, p[2]];
    let (o, a, b) = (squash(from), squash(part.a), squash(part.b));
    let stretched = squash(along);
    let scale = stereo::length(stretched);
    let d = stereo::scale(stretched, 1.0 / scale);
    // The nearest two points of the ray and the segment.
    let seg = stereo::sub(b, a);
    let w = stereo::sub(o, a);
    let (ss, sd, sw, dw) = (stereo::dot(seg, seg), stereo::dot(seg, d), stereo::dot(seg, w), stereo::dot(d, w));
    let denominator = ss - sd * sd;
    let on_seg = if ss < 1e-9 { 0.0 } else if denominator.abs() < 1e-9 { 0.0 } else { ((sw - sd * dw) / denominator).clamp(0.0, 1.0) };
    let q = stereo::add(a, stereo::scale(seg, on_seg));
    let t_near = stereo::dot(d, stereo::sub(q, o));
    let closest = stereo::add(o, stereo::scale(d, t_near));
    let apart = stereo::length(stereo::sub(closest, q));
    let cover = ((part.radius - apart) / pixel + 0.5).clamp(0.0, 1.0);
    if cover <= 0.0 {
        return None;
    }
    let back = (part.radius * part.radius - apart * apart).max(0.0).sqrt();
    let t = t_near - back;
    let at = stereo::add(o, stereo::scale(d, t));
    let out = stereo::sub(at, q);
    let n = [out[0], out[1] * part.flat, out[2]];
    let n = stereo::scale(n, 1.0 / stereo::length(n).max(1e-6));
    let view = stereo::scale(along, -1.0 / stereo::length(along));
    let light = stereo::scale(LIGHT_FROM, 1.0 / stereo::length(LIGHT_FROM));
    let halfway = stereo::add(light, view);
    let halfway = stereo::scale(halfway, 1.0 / stereo::length(halfway).max(1e-6));
    let facing = stereo::dot(n, view).abs();
    let diffuse = stereo::dot(n, light).max(0.0);
    let rim = (1.0 - facing).clamp(0.0, 1.0).powf(2.5);
    let shine = stereo::dot(n, halfway).max(0.0).powf(52.0);
    // Clearer through its middle, as glass, and solid towards its edge.
    let solid = (part.solid * (0.8 + 0.2 * rim) + 0.12).min(0.88) * cover;
    let lit = 0.34 + 0.66 * diffuse + part.glow;
    let colour: [f32; 3] = std::array::from_fn(|i| (part.colour[i] * lit + rim * 0.5) * solid + shine * 0.85 * cover);
    Some((t / scale, colour, (solid + shine * 0.3 * cover).min(0.92)))
}

/// One eye's picture into `out` (RGBA, premultiplied, its stride given), the eye at `eye` in its own space: its middle at the origin, X to its right, Y up, Z towards who looks.
fn eye_picture(out: &mut [u8], stride: usize, eye: V3, parts: &[Part]) {
    let [w, h] = [EYE_PX[0] as usize, EYE_PX[1] as usize];
    let per_pixel = WIDTH_M / w as f32;
    let to_pixel = |p: V3| -> Option<[f32; 2]> {
        // Where the line from the eye to this meets the picture's plane.
        let d = stereo::sub(p, eye);
        (d[2] < -1e-4).then(|| {
            let t = -eye[2] / d[2];
            [(eye[0] + d[0] * t + WIDTH_M / 2.0) / per_pixel, (HEIGHT_M / 2.0 - (eye[1] + d[1] * t)) / per_pixel]
        })
    };
    let mut layers: Vec<Vec<(f32, [f32; 3], f32)>> = vec![Vec::new(); w * h];
    for part in parts {
        let (Some(a), Some(b)) = (to_pixel(part.a), to_pixel(part.b)) else { continue };
        // A part's own corner of the picture, with room for how large it is near the eye.
        let reach = part.radius / per_pixel * 1.7 + 2.0;
        let (x0, x1) = (((a[0].min(b[0]) - reach).max(0.0)) as usize, ((a[0].max(b[0]) + reach).max(0.0) as usize).min(w));
        let (y0, y1) = (((a[1].min(b[1]) - reach).max(0.0)) as usize, ((a[1].max(b[1]) + reach).max(0.0) as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                let on = [(x as f32 + 0.5) * per_pixel - WIDTH_M / 2.0, HEIGHT_M / 2.0 - (y as f32 + 0.5) * per_pixel, 0.0];
                if let Some(layer) = hit(part, eye, stereo::sub(on, eye), per_pixel) {
                    layers[y * w + x].push(layer);
                }
            }
        }
    }
    for y in 0..h {
        for x in 0..w {
            let here = &mut layers[y * w + x];
            if here.is_empty() {
                continue;
            }
            // Furthest first.
            here.sort_by(|p, q| q.0.total_cmp(&p.0));
            let (mut colour, mut solid) = ([0f32; 3], 0f32);
            for (_, c, a) in here.iter() {
                colour = std::array::from_fn(|i| c[i] + colour[i] * (1.0 - a));
                solid = a + solid * (1.0 - a);
            }
            let at = y * stride + x * 4;
            for i in 0..3 {
                out[at + i] = (colour[i].min(solid.max(colour[i] * 0.9)) * 255.0).clamp(0.0, 255.0) as u8;
            }
            out[at + 3] = (solid * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

/// Both eyes' pictures side by side (RGBA, premultiplied), for a stick pushed by `push`, as seen by `eyes` in its own space.
pub fn picture(eyes: [V3; 2], push: f32, accent: Color32) -> Vec<u8> {
    let tint = [accent.r() as f32 / 255.0, accent.g() as f32 / 255.0, accent.b() as f32 / 255.0];
    let parts = parts(push, tint);
    let stride = EYE_PX[0] as usize * 2 * 4;
    let mut out = vec![0u8; stride * EYE_PX[1] as usize];
    eye_picture(&mut out, stride, eyes[0], &parts);
    eye_picture(&mut out[EYE_PX[0] as usize * 4..], stride, eyes[1], &parts);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: Color32 = Color32::from_rgb(0, 218, 109);
    const EYES: [V3; 2] = [[-0.032, 0.05, 0.45], [0.032, 0.05, 0.45]];

    fn half(pixels: &[u8], eye: usize) -> Vec<&[u8]> {
        let (w, h) = (EYE_PX[0] as usize, EYE_PX[1] as usize);
        (0..h).map(|y| &pixels[y * w * 2 * 4 + eye * w * 4..y * w * 2 * 4 + (eye + 1) * w * 4]).collect()
    }

    fn solid(rows: &[&[u8]]) -> usize {
        rows.iter().map(|r| r.chunks_exact(4).filter(|p| p[3] > 20).count()).sum()
    }

    /// Enzo: "not completely opaque". And premultiplied: no channel past what covers it, but where it shines.
    #[test]
    fn it_shows_and_is_nowhere_opaque() {
        let pixels = picture(EYES, 0.0, GREEN);
        assert!(solid(&half(&pixels, 0)) > 2000, "{}", solid(&half(&pixels, 0)));
        let most = pixels.chunks_exact(4).map(|p| p[3]).max().unwrap();
        assert!(most < 250, "it is never quite opaque: {most}");
        assert!(pixels[3] == 0, "nothing behind it at its corner: no frame");
    }

    /// It stands at a depth: each eye sees it from where it is, unlike a flat picture.
    #[test]
    fn each_eye_sees_it_from_where_it_is() {
        let pixels = picture(EYES, 0.8, GREEN);
        let (left, right) = (half(&pixels, 0), half(&pixels, 1));
        let centre = |rows: &[&[u8]]| {
            let (mut sum, mut n) = (0f32, 0f32);
            for row in rows {
                for (x, p) in row.chunks_exact(4).enumerate() {
                    sum += x as f32 * p[3] as f32;
                    n += p[3] as f32;
                }
            }
            sum / n
        };
        assert_ne!(left, right);
        assert!((centre(&left) - centre(&right)).abs() > 0.2, "{} {}", centre(&left), centre(&right));
        // From well to the side, what shows moves by much more: it is not a picture turned.
        let side = picture([[0.25, 0.05, 0.35], [0.31, 0.05, 0.32]], 0.8, GREEN);
        assert!((centre(&half(&side, 0)) - centre(&left)).abs() > 1.0);
    }

    /// Pushed up the top goes away from who looks and up the picture; pulled down it comes towards them. The chevron that way lights.
    #[test]
    fn its_push_shows() {
        let (up, down) = (picture(EYES, 1.0, GREEN), picture(EYES, -1.0, GREEN));
        assert_ne!(up, down);
        let green = |pixels: &[u8]| pixels.chunks_exact(4).map(|p| p[1] as i32 - p[0] as i32).max().unwrap();
        assert!(green(&up) > green(&picture(EYES, 0.0, GREEN)));
    }

    /// Well made: it moves on a spring, goes past a little, rests at each end, and comes back to where it started so that its cycle has no jump.
    #[test]
    fn its_animation_overshoots_rests_and_has_no_jump() {
        let samples: Vec<f32> = (0..=280).map(|i| push_at(i as f32 / 100.0)).collect();
        assert!(samples.iter().copied().fold(f32::MIN, f32::max) > 1.0, "past the end a little");
        assert!(samples.iter().copied().fold(f32::MAX, f32::min) < -1.0);
        assert!((push_at(CYCLE_S * 0.35) - 1.0).abs() < 0.03, "held pushed");
        assert!((push_at(CYCLE_S * 0.85) + 1.0).abs() < 0.03, "held pulled");
        assert!(samples.windows(2).all(|w| (w[1] - w[0]).abs() < 0.35), "no jump");
    }
}
