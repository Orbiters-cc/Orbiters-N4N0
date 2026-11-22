//! `--only quiet-prompt` (Request20, "No 3D picture of this world yet" made discreet): the world notice as a person sees it,
//! over a made-up bright world and a made-up dark one (gradients and blocks, no real picture):
//! - `quiet-vr-<ground>-<state>.png`: one eye's view in a headset (100° across), the band where it is placed before the
//!   eyes, at its size there;
//! - `quiet-desk-<ground>-<state>.png`: VRChat's window in desktop mode (1920 x 1080), what of ours lies over the game.

use crate::overlay::Canvas;
use egui::{pos2, vec2, Color32, ColorImage, Rect};
use orbiters_ui::offscreen::save_png;
use std::path::Path;

/// One eye's picture and its field of view across.
const EYE_PX: u32 = 1440;
const EYE_FOV_DEG: f32 = 100.0;
const DESK: [u32; 2] = [1920, 1080];

#[derive(Clone, Copy)]
enum Ground {
    Bright,
    Dark,
}

impl Ground {
    fn name(self) -> &'static str {
        match self {
            Ground::Bright => "bright",
            Ground::Dark => "dark",
        }
    }
}

/// A made-up world: a sky, a floor, a few blocks and lights.
fn ground(kind: Ground, w: u32, h: u32) -> Vec<u8> {
    let mut canvas = Canvas::new(w, h);
    let (wf, hf) = (w as f32, h as f32);
    canvas.paint(0.0, |p| {
        let rows = 60;
        for i in 0..rows {
            let t = i as f32 / rows as f32;
            let c = match kind {
                Ground::Bright => Color32::from_rgb((222.0 + 22.0 * t) as u8, (233.0 + 10.0 * t) as u8, (245.0 - 15.0 * t) as u8),
                Ground::Dark => Color32::from_rgb((12.0 + 18.0 * t) as u8, (14.0 + 8.0 * t) as u8, (30.0 + 22.0 * t) as u8),
            };
            p.rect_filled(Rect::from_min_size(pos2(0.0, t * hf), vec2(wf, hf / rows as f32 + 1.0)), 0.0, c);
        }
        let floor = match kind {
            Ground::Bright => Color32::from_rgb(250, 248, 242),
            Ground::Dark => Color32::from_rgb(26, 24, 36),
        };
        p.rect_filled(Rect::from_min_max(pos2(0.0, hf * 0.62), pos2(wf, hf)), 0.0, floor);
        let blocks: [(f32, f32, f32, f32, [u8; 3], [u8; 3]); 5] = [
            (0.06, 0.30, 0.22, 0.66, [255, 255, 255], [60, 52, 90]),
            (0.30, 0.40, 0.42, 0.64, [214, 226, 236], [40, 70, 120]),
            (0.55, 0.22, 0.70, 0.70, [255, 252, 240], [90, 40, 110]),
            (0.74, 0.50, 0.98, 0.95, [255, 255, 255], [30, 90, 110]),
            (0.48, 0.74, 0.92, 1.00, [236, 240, 246], [255, 196, 90]),
        ];
        for (x0, y0, x1, y1, bright, dark) in blocks {
            let [r, g, b] = match kind {
                Ground::Bright => bright,
                Ground::Dark => dark,
            };
            p.rect_filled(Rect::from_min_max(pos2(x0 * wf, y0 * hf), pos2(x1 * wf, y1 * hf)), 6.0, Color32::from_rgb(r, g, b));
        }
        if let Ground::Dark = kind {
            for i in 0..14 {
                let x = (0.07 * i as f32 + 0.03) % 1.0;
                p.circle_filled(pos2(x * wf, hf * (0.12 + 0.05 * (i % 3) as f32)), 3.0 + (i % 4) as f32, Color32::from_rgb(255, 230, 170));
            }
        }
    });
    canvas.pixels().chunks_exact(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect()
}

/// Premultiplied `src` (`sw` x `sh`) resampled to `dw` x `dh` (bilinear).
fn scaled(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
            let fy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
            let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            for c in 0..4 {
                let at = |xx: u32, yy: u32| src[((yy * sw + xx) * 4 + c) as usize] as f32;
                let v = at(x0, y0) * (1.0 - tx) * (1.0 - ty) + at(x1, y0) * tx * (1.0 - ty) + at(x0, y1) * (1.0 - tx) * ty + at(x1, y1) * tx * ty;
                out[((y * dw + x) * 4 + c as u32) as usize] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Premultiplied `pixels` (`px`) laid over opaque `onto` (`width` wide) at `at`, at `alpha`.
fn lay(onto: &mut [u8], width: u32, pixels: &[u8], px: [u32; 2], at: [i32; 2], alpha: f32) {
    let height = onto.len() as u32 / 4 / width;
    for y in 0..px[1] as i32 {
        for x in 0..px[0] as i32 {
            let (ox, oy) = (at[0] + x, at[1] + y);
            if ox < 0 || oy < 0 || ox >= width as i32 || oy >= height as i32 {
                continue;
            }
            let i = ((oy as u32 * width + ox as u32) * 4) as usize;
            let j = ((y as u32 * px[0] + x as u32) * 4) as usize;
            let a = pixels[j + 3] as f32 * alpha / 255.0;
            for c in 0..3 {
                onto[i + c] = (pixels[j + c] as f32 * alpha + onto[i + c] as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn save(out: &Path, name: &str, rgba: &[u8], size: [u32; 2]) -> anyhow::Result<()> {
    let path = out.join(format!("{name}.png"));
    save_png(&path, &ColorImage::from_rgba_unmultiplied([size[0] as usize, size[1] as usize], rgba))?;
    println!("{}", path.display());
    Ok(())
}

/// One eye's view with a quad of `width_m` facing the eyes, its centre `right_m` right of, `below_m` under the eyes' line,
/// `distance_m` ahead, painted with `face` (premultiplied, `px`).
fn eye_view(kind: Ground, face: &[u8], px: [u32; 2], width_m: f32, right_m: f32, below_m: f32, distance_m: f32) -> Vec<u8> {
    let mut view = ground(kind, EYE_PX, EYE_PX);
    let f = (EYE_PX as f32 / 2.0) / (EYE_FOV_DEG.to_radians() / 2.0).tan();
    let w = (f * width_m / distance_m).round() as u32;
    let h = (w as f32 * px[1] as f32 / px[0] as f32).round() as u32;
    let c = [EYE_PX as f32 / 2.0 + f * right_m / distance_m, EYE_PX as f32 / 2.0 + f * below_m / distance_m];
    let small = scaled(face, px[0], px[1], w, h);
    lay(&mut view, EYE_PX, &small, [w, h], [(c[0] - w as f32 / 2.0) as i32, (c[1] - h as f32 / 2.0) as i32], 1.0);
    view
}

/// The band as it was before Request20 (0.3.9-dev.5, `world_notice::paint_vr`), kept here word for word to picture before
/// and after: 640 x 232 pixels, 0.27 m wide, 0.6 m before the eyes and 0.16 m under their line, straight ahead.
mod before {
    use crate::controller::explorer::{NOTICE_EARNS, NOTICE_TITLE, NOTICE_WHY, TAKE};
    use egui::{pos2, vec2, Align2, Painter, Rect};
    use orbiters_i18n::tr;
    use orbiters_ui::theme::{self, weight, Txt};
    use orbiters_ui::{icons, n4n0_logo, widgets};

    pub const VR_PX: [u32; 2] = [640, 232];
    pub const VR_WIDTH_M: f32 = 0.27;

    fn why_of(earns_one: bool) -> &'static str {
        if earns_one {
            tr(NOTICE_EARNS)
        } else {
            tr(NOTICE_WHY)
        }
    }

    pub fn band(p: &Painter, earns_one: bool, hover: bool, left: f32) {
        let all = Rect::from_min_size(pos2(0.0, 0.0), vec2(VR_PX[0] as f32, VR_PX[1] as f32));
        let band = all.shrink(6.0);
        let radius = 36.0;
        let fill = if hover { theme::mix(theme::INK_950, theme::FG, 0.14) } else { theme::alpha(theme::INK_950, 0.94) };
        p.rect_filled(band, radius, fill);
        p.rect_stroke(band, radius, egui::Stroke::new(3.0, if hover { theme::ACCENT } else { theme::white(0.16) }), egui::StrokeKind::Inside);
        let mark = Rect::from_center_size(pos2(band.left() + 30.0 + 24.0, band.top() + 60.0), n4n0_logo::size_for_width(48.0));
        n4n0_logo::paint(p, mark, theme::MARK);
        let words = mark.right() + 20.0;
        let room = band.right() - 30.0 - words;
        let title = tr(NOTICE_TITLE);
        let size = crate::mismatch::fitting(p, title, &[31.0, 28.0, 25.0, 23.0], room);
        widgets::text_truncated(p, pos2(words, band.top() + 42.0), Align2::LEFT_CENTER, &Txt::new(title, size).weight(weight::SEMIBOLD), room + 4.0);
        let why = why_of(earns_one);
        let size = crate::mismatch::fitting(p, why, &[22.0, 20.0, 18.0, 16.5], room);
        widgets::text_truncated(p, pos2(words, band.top() + 80.0), Align2::LEFT_CENTER, &Txt::new(why, size).color(theme::FG_3), room + 4.0);
        // The action, a pill across the band's foot.
        let pill = Rect::from_min_max(pos2(band.left() + 26.0, band.bottom() - 26.0 - 64.0), pos2(band.right() - 26.0, band.bottom() - 26.0));
        p.rect_filled(pill, 32.0, if hover { theme::mix(theme::ACCENT, theme::FG, 0.2) } else { theme::ACCENT });
        let take = tr(TAKE);
        let size = crate::mismatch::fitting(p, take, &[28.0, 26.0, 24.0, 22.0], pill.width() - 110.0);
        let wide = p.layout_no_wrap(take.to_string(), egui::FontId::proportional(size), theme::INK_950).size().x + 40.0;
        let from = pill.center().x - wide / 2.0;
        widgets::icon(p, pos2(from + 14.0, pill.center().y), icons::CUBE, 28.0, theme::INK_950);
        widgets::text_truncated(p, pos2(from + 40.0, pill.center().y), Align2::LEFT_CENTER, &Txt::new(take, size).weight(weight::SEMIBOLD).color(theme::INK_950), pill.width() - 100.0);
        let track = Rect::from_min_max(pos2(band.left() + radius, band.bottom() - 11.0), pos2(band.right() - radius, band.bottom() - 7.0));
        p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * left.clamp(0.0, 1.0), track.height())), 2.0, theme::white(0.35));
    }
}

pub(super) fn shots(out: &Path) -> anyhow::Result<()> {
    use crate::world_notice::Part;
    std::fs::create_dir_all(out)?;
    let lang = orbiters_i18n::language().tag();
    let spot = crate::world_notice::VR_SPOT;
    for kind in [Ground::Bright, Ground::Dark] {
        // Before: the band straight before the eyes, as it was.
        let [bw, bh] = before::VR_PX;
        let mut canvas = Canvas::new(bw, bh);
        for _ in 0..2 {
            canvas.paint(0.0, |p| before::band(p, true, false, 0.6));
        }
        let view = eye_view(kind, canvas.pixels(), [bw, bh], before::VR_WIDTH_M, 0.0, 0.16, 0.6);
        save(out, &format!("quiet-vr-{}-before-{lang}", kind.name()), &view, [EYE_PX, EYE_PX])?;
        // Before, desktop mode: nothing of it over the game (only N4N0's own window had its card).
        save(out, &format!("quiet-desk-{}-before", kind.name()), &ground(kind, DESK[0], DESK[1]), DESK)?;
        // After, in a headset: the card low and right of the gaze, at rest, a laser on it, a laser on its cross.
        for (state, hover) in [("rest", None), ("laser", Some(Part::Take)), ("laser-close", Some(Part::Close))] {
            let [bw, bh] = crate::world_notice::VR_PX;
            let mut canvas = Canvas::new(bw, bh);
            for _ in 0..2 {
                canvas.paint(0.0, |p| crate::world_notice::paint_vr(p, true, hover));
            }
            let view = eye_view(kind, canvas.pixels(), [bw, bh], crate::world_notice::VR_WIDTH_M, spot.right, spot.below, spot.distance);
            save(out, &format!("quiet-vr-{}-{state}-{lang}", kind.name()), &view, [EYE_PX, EYE_PX])?;
        }
        // After, desktop mode: the card in the bottom right corner of VRChat's window where the desktop puts it, at rest,
        // the pointer on its cross, and half faded as it goes.
        #[cfg(windows)]
        for (state, hover, alpha) in [("rest", None, 1.0), ("pointer-close", Some(Part::Close), 1.0), ("fading", None, 0.5)] {
            let client = vec2(DESK[0] as f32, DESK[1] as f32);
            let (rect, zoom) = crate::overlay::orbit::desk_notice::rect(client);
            let px = [rect.width().ceil() as u32, rect.height().ceil() as u32];
            let mut canvas = Canvas::new(px[0], px[1]);
            for _ in 0..2 {
                canvas.paint(0.0, |p| crate::world_notice::paint_card(p, egui::Pos2::ZERO, zoom, true, hover));
            }
            let mut desk = ground(kind, DESK[0], DESK[1]);
            lay(&mut desk, DESK[0], canvas.pixels(), px, [rect.min.x.round() as i32, rect.min.y.round() as i32], alpha);
            save(out, &format!("quiet-desk-{}-{state}-{lang}", kind.name()), &desk, DESK)?;
        }
    }
    Ok(())
}
