//! `--only orbit`: the panels around VRChat's menus (overlay/orbit) as they are painted, placed as in a window of each size around a stand-in of the wheel or the Quick
//! Menu: `orbit-wheel-*.png`, `orbit-qm-*.png`, one per state; `orbit-takeover*.png` N4N0's menu in front of the Quick Menu. What the real game looks like with them is in
//! reports/orbit-overlays (captured over the bridge).

use super::over_ground;
use crate::overlay::orbit::panels::{self, Facts, Recent};
use crate::overlay::floating::space::Quad;
use crate::overlay::orbit::zones::{self, Around, Placed, Zone};
use crate::overlay::Canvas;
use egui::{pos2, vec2, Color32, ColorImage, TextureOptions, Vec2};
use orbiters_ui::offscreen::save_png;
use std::path::{Path, PathBuf};

/// Four recent captures, with stand-in pictures (dusk colours) that `paint` makes textures of.
fn recent() -> panels::Gallery {
    [("Sky Dream", "3D picture, ready"), ("Rainy Skyline", "Reconstructing 42%"), ("Pacific Dreams (Night)", "Recording, waiting"), ("After The Rain", "3D picture, ready")]
        .iter()
        .map(|(title, state)| Recent { dir: PathBuf::from(title), title: title.to_string(), state: state.to_string(), progress: state.contains('%').then_some(0.42), picture: None, ..Default::default() })
        .collect()
}

/// The names of the real pictures `stand_in` gives, in its order (`ORBIT_PANEL_WORLDS`, files named "NN Name.jpg"); none without them.
fn stand_in_names() -> Vec<String> {
    let Ok(dir) = std::env::var("ORBIT_PANEL_WORLDS") else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| image::ImageFormat::from_path(p).is_ok()).collect();
    paths.sort();
    paths.iter().filter_map(|p| Some(p.file_stem()?.to_string_lossy().split_once(' ')?.1.to_string())).collect()
}

fn stand_in(seed: u8) -> ColorImage {
    // Real pictures where there are some to hand: dots and colours are judged on those.
    static REAL: std::sync::OnceLock<Vec<ColorImage>> = std::sync::OnceLock::new();
    let real = REAL.get_or_init(|| {
        let Ok(dir) = std::env::var("ORBIT_PANEL_WORLDS") else { return Vec::new() };
        let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut paths: Vec<std::path::PathBuf> = read.flatten().map(|e| e.path()).collect();
        paths.sort();
        paths
            .into_iter()
            .filter_map(|path| {
                let image = image::open(&path).ok()?.to_rgb8();
                Some(ColorImage::from_rgb([image.width() as usize, image.height() as usize], image.as_raw()))
            })
            .collect()
    });
    if !real.is_empty() {
        return real[seed as usize % real.len()].clone();
    }
    let (w, h) = (160usize, 90usize);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let t = y as f32 / h as f32;
            let glow = 1.0 - ((x as f32 / w as f32 - 0.3 - seed as f32 * 0.12).abs() * 2.0).min(1.0);
            pixels.push(Color32::from_rgb((40.0 + 180.0 * (1.0 - t) * glow) as u8, (30.0 + 80.0 * (1.0 - t)) as u8, (60.0 + 90.0 * t) as u8));
        }
    }
    ColorImage::new([w, h], pixels)
}

/// A panel's picture, as the platform gets it: painted twice (what loads on the first frame shows on the second), and cut where the wheel bites.
fn picture(placed: &Placed, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) -> (Vec<u8>, [u32; 2]) {
    let px = [placed.rect.width().round() as u32, placed.rect.height().round() as u32];
    let logical = placed.logical();
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / logical.x).in_orbit_look();
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    facts.recent_pictures = textures.iter().enumerate().map(|(n, texture)| (n, texture.id())).collect();
    // A page shown over the camera: a stand-in of what it drew.
    if let Some(web) = facts.camera.web.as_mut().filter(|w| w.state == panels::WebState::Shown) {
        web.picture = textures.first().map(|t| t.id());
    }
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint(p, placed.zone, logical, &facts, hover, pressed));
    }
    let from = canvas.pixels().to_vec();
    let mut out = from.clone();
    panels::finish_picture(&mut out, &from, px, px[0] as f32 / logical.x, [0, 0, px[0], px[1]]);
    if let Some(ring) = placed.zone.ring() {
        let mask = panels::cell_mask(ring, px, px[0] as f32 / logical.x);
        panels::apply_mask(&mut out, &mask, px, [0, 0, px[0], px[1]]);
    }
    (out, px)
}

fn blit(onto: &mut [u8], width: u32, pixels: &[u8], px: [u32; 2], at: egui::Pos2) {
    for y in 0..px[1] {
        for x in 0..px[0] {
            let (ox, oy) = (at.x as u32 + x, at.y as u32 + y);
            let i = ((oy * width + ox) * 4) as usize;
            if i + 3 >= onto.len() || ox >= width {
                continue;
            }
            let j = ((y * px[0] + x) * 4) as usize;
            let a = pixels[j + 3] as u32;
            for c in 0..4 {
                onto[i + c] = (pixels[j + c] as u32 + onto[i + c] as u32 * (255 - a) / 255).min(255) as u8;
            }
        }
    }
}

/// A stand-in of the menu the panels surround, to judge the gaps.
fn stand_in_menu(around: Around, client: Vec2, canvas: &mut Canvas) {
    let (center, unit) = zones::frame(around, client);
    canvas.paint(0.0, |p| match around {
        Around::Wheel => {
            p.circle_filled(center, 100.0 * unit, Color32::from_rgb(7, 55, 58));
            for t in 0..6 {
                let way = zones::polar(100.0 * unit, 30.0 + t as f32 * 60.0);
                p.line_segment([center, center + way], egui::Stroke::new(1.0, Color32::from_rgb(15, 87, 92)));
            }
            p.circle_filled(center, 48.0 * unit, Color32::from_rgb(22, 34, 43));
        }
        Around::QuickMenu | Around::Camera => {
            let r = |x0: f32, y0: f32, x1: f32, y1: f32| egui::Rect::from_min_max(center + vec2(x0, y0) * unit, center + vec2(x1, y1) * unit);
            p.rect_filled(r(-351.0, -150.0, 351.0, 186.0), 8.0, Color32::from_rgb(25, 34, 42));
            p.rect_filled(r(-188.0, -204.0, 188.0, 204.0), 8.0, Color32::from_rgb(22, 34, 43));
            for x in zones::QM_ICONS_X {
                p.circle_filled(center + vec2(x, zones::QM_ICONS_Y) * unit, 13.0 * unit, Color32::from_rgb(30, 90, 95));
            }
        }
    });
}

fn scene(name: &str, out: &Path, around: Around, client: Vec2, facts: &Facts, hover: Option<(Zone, usize, bool)>) -> anyhow::Result<()> {
    let (w, h) = (client.x as u32, client.y as u32);
    let mut ground = Canvas::new(w, h);
    stand_in_menu(around, client, &mut ground);
    let mut image = ground.pixels().to_vec();
    for placed in zones::layout(around, client) {
        let (hovered, pressed) = match hover {
            Some((zone, i, down)) if zone == placed.zone => (Some(i), down.then_some(i)),
            _ => (None, None),
        };
        let (pixels, px) = picture(&placed, facts, hovered, pressed);
        blit(&mut image, w, &pixels, px, placed.rect.min);
    }
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

/// What a pixel of an eye sees: one image of `size`, from `eyes` looking along -Z, at about a headset's width of view.
struct Eye {
    size: [u32; 2],
    pixels: Vec<u8>,
    eyes: [f32; 3],
    focal: f32,
}

impl Eye {
    /// The world behind everything: a real frame of the game, covering the view.
    fn new(size: [u32; 2], ground: &image::RgbImage, eyes: [f32; 3]) -> Self {
        Self::seeing(size, ground, eyes, VR_SHOT_HALF_FOV)
    }

    /// The same, `half_fov` degrees each side.
    fn seeing(size: [u32; 2], ground: &image::RgbImage, eyes: [f32; 3], half_fov: f32) -> Self {
        // The middle of the frame, past the desktop's HUD in its corners, covering the view without stretching.
        let (gw, gh) = (ground.width(), ground.height());
        // Seen closer in, a smaller part of it, so that it keeps its size behind what floats.
        let k = half_fov.to_radians().tan() / VR_SHOT_HALF_FOV.to_radians().tan();
        let cw = ((gw * 2 / 3) as f32 * k) as u32;
        let ch = cw * size[1] / size[0];
        let part = image::imageops::crop_imm(ground, (gw - cw) / 2, (gh.saturating_sub(ch)) / 2 + gh / 12, cw, ch.min(gh)).to_image();
        let scaled = image::imageops::resize(&part, size[0], size[1], image::imageops::FilterType::Triangle);
        let mut pixels = vec![0u8; size[0] as usize * size[1] as usize * 4];
        for (i, p) in scaled.pixels().enumerate() {
            pixels[i * 4..i * 4 + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
        }
        Self { size, pixels, eyes, focal: size[0] as f32 / 2.0 / half_fov.to_radians().tan() }
    }

    fn ray(&self, x: f32, y: f32) -> [f32; 3] {
        let d = [(x - self.size[0] as f32 / 2.0) / self.focal, -(y - self.size[1] as f32 / 2.0) / self.focal, -1.0];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / l, d[1] / l, d[2] / l]
    }

    /// A picture of the whole view in premultiplied RGBA over what is there.
    fn over(&mut self, picture: &[u8]) {
        for (to, from) in self.pixels.chunks_exact_mut(4).zip(picture.chunks_exact(4)) {
            let left = 255 - from[3] as u32;
            for c in 0..3 {
                to[c] = (from[c] as u32 + to[c] as u32 * left / 255).min(255) as u8;
            }
        }
    }

    /// `picture` (premultiplied RGBA of `px`) on `quad`, seen from the eyes, each pixel sampled from it with four rays' worth of softness.
    fn quad(&mut self, quad: &Quad, picture: &[u8], px: [u32; 2], alpha: f32) {
        let (w, h) = (self.size[0], self.size[1]);
        // Only where the quad's corners say it can be.
        let mut bounds = egui::Rect::NOTHING;
        for (sx, sy) in [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)] {
            let p = crate::overlay::floating::space::apply(&quad.transform, [sx * quad.width, sy * quad.height, 0.0]);
            let d = [p[0] - self.eyes[0], p[1] - self.eyes[1], p[2] - self.eyes[2]];
            bounds.extend_with(pos2(w as f32 / 2.0 + d[0] / -d[2] * self.focal, h as f32 / 2.0 - d[1] / -d[2] * self.focal));
        }
        let bounds = bounds.expand(2.0).intersect(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(w as f32, h as f32)));
        if !bounds.is_positive() {
            return;
        }
        for y in bounds.top() as u32..bounds.bottom().ceil() as u32 {
            for x in bounds.left() as u32..bounds.right().ceil() as u32 {
                let mut sum = [0.0f32; 4];
                let mut any = false;
                for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                    let Some(hit) = quad.hit(self.eyes, self.ray(x as f32 + ox, y as f32 + oy), 0.0) else { continue };
                    any = true;
                    let (u, v) = (hit.uv[0] * px[0] as f32 - 0.5, hit.uv[1] * px[1] as f32 - 0.5);
                    let (x0, y0) = (u.floor().clamp(0.0, px[0] as f32 - 1.0) as u32, v.floor().clamp(0.0, px[1] as f32 - 1.0) as u32);
                    let (x1, y1) = ((x0 + 1).min(px[0] - 1), (y0 + 1).min(px[1] - 1));
                    let (ax, ay) = ((u - x0 as f32).clamp(0.0, 1.0), (v - y0 as f32).clamp(0.0, 1.0));
                    let at = |xx: u32, yy: u32, c: usize| picture[((yy * px[0] + xx) as usize) * 4 + c] as f32;
                    for (c, slot) in sum.iter_mut().enumerate() {
                        *slot += ((at(x0, y0, c) * (1.0 - ax) + at(x1, y0, c) * ax) * (1.0 - ay) + (at(x0, y1, c) * (1.0 - ax) + at(x1, y1, c) * ax) * ay) / 4.0;
                    }
                }
                if !any {
                    continue;
                }
                let i = (y * w + x) as usize * 4;
                let a = sum[3] / 255.0 * alpha;
                for c in 0..3 {
                    self.pixels[i + c] = (sum[c] * alpha + self.pixels[i + c] as f32 * (1.0 - a)).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    /// The laser: from a hand to where it ends, drawn as a tapering line over the rest.
    fn laser(&mut self, from: [f32; 3], to: [f32; 3]) {
        let project = |p: [f32; 3], e: &Eye| {
            let d = [p[0] - e.eyes[0], p[1] - e.eyes[1], p[2] - e.eyes[2]];
            pos2(e.size[0] as f32 / 2.0 + d[0] / -d[2] * e.focal, e.size[1] as f32 / 2.0 - d[1] / -d[2] * e.focal)
        };
        let (a, b) = (project(from, self), project(to, self));
        let steps = (a - b).length() as u32 + 1;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let c = a + (b - a) * t;
            let r = 3.5 * (1.0 - t) + 1.2;
            for yy in (c.y - r - 1.0) as i32..=(c.y + r + 1.0) as i32 {
                for xx in (c.x - r - 1.0) as i32..=(c.x + r + 1.0) as i32 {
                    if xx < 0 || yy < 0 || xx >= self.size[0] as i32 || yy >= self.size[1] as i32 {
                        continue;
                    }
                    let d = (pos2(xx as f32, yy as f32) - c).length();
                    let a = (r + 0.5 - d).clamp(0.0, 1.0) * (0.85 - 0.5 * t);
                    if a <= 0.0 {
                        continue;
                    }
                    let k = (yy as u32 * self.size[0] + xx as u32) as usize * 4;
                    for (c, v) in [0x00u8, 0xda, 0x6d].iter().enumerate() {
                        self.pixels[k + c] = (*v as f32 * a + self.pixels[k + c] as f32 * (1.0 - a)) as u8;
                    }
                }
            }
        }
        // Its dot on the panel.
        for yy in (b.y - 9.0) as i32..=(b.y + 9.0) as i32 {
            for xx in (b.x - 9.0) as i32..=(b.x + 9.0) as i32 {
                if xx < 0 || yy < 0 || xx >= self.size[0] as i32 || yy >= self.size[1] as i32 {
                    continue;
                }
                let d = (pos2(xx as f32, yy as f32) - b).length();
                let a = (7.0 - d).clamp(0.0, 1.0);
                let k = (yy as u32 * self.size[0] + xx as u32) as usize * 4;
                for c in 0..3 {
                    self.pixels[k + c] = (255.0 * a + self.pixels[k + c] as f32 * (1.0 - a)) as u8;
                }
            }
        }
    }

    fn save(&self, out: &Path, name: &str) -> anyhow::Result<()> {
        let path = out.join(format!("{name}.png"));
        save_png(&path, &egui::ColorImage::from_rgba_premultiplied([self.size[0] as usize, self.size[1] as usize], &self.pixels))?;
        println!("{}", path.display());
        Ok(())
    }
}

/// A square of a real frame around a point, out to a shape: premultiplied RGBA of its size.
fn cut(frame: &image::RgbImage, rect: egui::Rect, inside: impl Fn(Vec2) -> f32) -> (Vec<u8>, [u32; 2]) {
    let (w, h) = (rect.width().round() as u32, rect.height().round() as u32);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let p = frame.get_pixel((rect.left() as u32 + x).min(frame.width() - 1), (rect.top() as u32 + y).min(frame.height() - 1));
            let a = inside(vec2(x as f32 + 0.5, y as f32 + 0.5) + rect.min.to_vec2()).clamp(0.0, 1.0);
            out.extend_from_slice(&[(p[0] as f32 * a) as u8, (p[1] as f32 * a) as u8, (p[2] as f32 * a) as u8, (a * 255.0) as u8]);
        }
    }
    (out, [w, h])
}

/// `--only orbit-vr <folder of real frames>`: how the panels hang in a headset, as renders: the real panels' pictures and VRChat's real wheel and Quick Menu, cut from desktop frames (`ref-wheel-<world>.jpg`,
/// `ref-qm-<world>.jpg`, `ref-world-<world>.jpg` at 1920 x 1080), placed in 3D where `overlay::orbit::vr` places them and seen from the eyes. VRChat's menus were not captured in a headset: their place and size there is an estimate.
/// Half the width the VR renders see, in degrees: wide enough that the wing and what plays at its foot are in the frame (round 13's judges saw them cut).
// About 13 pixels a degree at 1280 across: nearer what a headset shows (a Quest Pro, about 18) than the 11 of rounds 9 to 19, whose pictures showed every label a
// quarter smaller than a headset does. The README says so.
const VR_SHOT_HALF_FOV: f32 = 50.0;
/// The view in a hand, seen closer in: about a headset's pixels to a degree once the picture is brought to 1280 across (a Quest Pro has about 18), so that how
/// its hint reads is what a headset shows, not what a wide frame shrinks it to (round 14's judges found its lines too small). A part of what the eye sees.
const HAND_SHOT_HALF_FOV: f32 = 36.0;

/// Which side tiles a Quick Menu render shows: the wing on the side away from this hand (0 right, 1 left), or the strip under the menu.
#[derive(Clone, Copy)]
enum SideTiles {
    Wing(usize),
    Strip,
}

pub(super) fn vr_shots(out: &Path, refs: &Path) -> anyhow::Result<()> {
    use crate::overlay::floating::space;
    use crate::overlay::orbit::vr;
    use orbiters_n4n0_core::menu::{PhotoMode, Target};
    use panels::{Action, Sheet};
    let head = space::from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0]);
    let eyes = [0.0, 1.6, 0.0];
    let size = [1600, 1200];
    for world in ["forest", "dream"] {
        let read = |name: &str| -> anyhow::Result<image::RgbImage> { Ok(image::open(refs.join(format!("ref-{name}-{world}.jpg")))?.to_rgb8()) };
        let (ground, wheel_frame, qm_frame) = (read("world")?, read("wheel")?, read("qm")?);
        // Only the small view's, to try it on captures.
        let only_small = std::env::var_os("ORBIT_ONLY_SMALL").is_some();
        let client = vec2(1920.0, 1080.0);
        let (center, radius) = zones::wheel(client);
        let reach = radius * 1.03;
        let (disc, disc_px) = cut(&wheel_frame, egui::Rect::from_center_size(center, Vec2::splat(reach * 2.0)), |p| reach - (p.to_pos2() - center).length() + 0.5);
        let (qc, unit) = zones::quick_menu(client);
        let parts = [(-188.0, -204.0, 188.0, 204.0), (-351.0, -150.0, 351.0, 186.0), (-242.0, 204.0, 242.0, 242.0), (-188.0, -240.0, 188.0, -204.0)];
        let qm_units = egui::Rect::from_min_max(pos2(-352.0, -240.0), pos2(352.0, 243.0));
        let qm_rect = egui::Rect::from_min_max(qc + qm_units.min.to_vec2() * unit, qc + qm_units.max.to_vec2() * unit);
        let (qm, qm_px) = cut(&qm_frame, qm_rect, |p| {
            let u = (p.to_pos2() - qc) / unit;
            if parts.iter().any(|(a, b, c, d)| u.x >= *a && u.x <= *c && u.y >= *b && u.y <= *d) { 1.0 } else { 0.0 }
        });
        let facts = Facts { recent: recent(), timer: 3, n4n0: true, desktop: false, vr_close: true, music: Some(("The Weeknd - Moth to a Flame".into(), true)), can_call: true, can_states: true, can_light: true, ..Default::default() };
        // The wheel on a hand, where a person looks at it; the other hand's laser on its cells.
        let tune = vr::Tune::default();
        let held = space::facing([0.03, 1.5, -0.40], space::position(&head), [0.0, 1.0, 0.0]).expect("a frame");
        let plane = vr::wheel_plane(&vr::grip_holding(&held, &tune.wheel(1)), 1, &tune, 1.0);
        let wheel_scene = |facts: &Facts, hover: Option<Zone>, bloom: Option<zones::Group>, name: &str| -> anyhow::Result<()> {
            let mut eye = Eye::new(size, &ground, eyes);
            let unit_m = plane.unit;
            let disc_quad = Quad { transform: plane.frame, width: 2.06 * 100.0 * unit_m, height: 2.06 * 100.0 * unit_m };
            eye.quad(&disc_quad, &disc, disc_px, 1.0);
            let mut list: Vec<Zone> = Zone::WHEEL.to_vec();
            list.push(Zone::WheelHide);
            let mut end = None;
            for zone in list {
                if zone == Zone::WheelVeil || zone.group().is_some_and(|g| Some(g) != bloom) {
                    continue;
                }
                let placed = vr::wheel_place(zone);
                let hovered = hover == Some(zone);
                let (pixels, px) = sharp_picture(&placed, facts, hovered.then_some(0), 2.0);
                let quad = plane.quad(&vr::clear_of_sheet(placed, false), 0.0, 0.004);
                eye.quad(&quad, &pixels, px, 1.0);
                if hovered {
                    let at = zone.ring().map_or(placed.rect.center().to_vec2(), |r| r.anchor());
                    end = Some(space::apply(&plane.frame, [at.x * unit_m, -at.y * unit_m, 0.01]));
                }
            }
            if let Some(end) = end {
                // From the other hand, held low before the body: its beam crosses no cell's name on its way.
                eye.laser([-0.02, 1.02, -0.22], end);
            }
            eye.save(out, name)
        };
        let pointing = |a| Facts { pointed: Some(a), ..facts.clone() };
        let bloom = zones::Group::Pictures;
        if !only_small {
        wheel_scene(&facts, None, None, &format!("vr-wheel-{world}"))?;
        wheel_scene(&pointing(Action::Worlds), Some(Zone::WheelWorlds), None, &format!("vr-wheel-{world}-point-worlds"))?;
        wheel_scene(&pointing(Action::Pictures), Some(Zone::WheelPictures), Some(bloom), &format!("vr-wheel-{world}-point-pictures"))?;
        wheel_scene(&pointing(Action::Picture(PhotoMode::World)), Some(Zone::BloomWorld), Some(bloom), &format!("vr-wheel-{world}-point-picture"))?;
        wheel_scene(&pointing(Action::PlayPause), Some(Zone::BloomNext), Some(zones::Group::Playing), &format!("vr-wheel-{world}-point-playing"))?;
        let counting = Facts { countdown: Some(2), picture_mode: Some(PhotoMode::People), pointed: Some(Action::Picture(PhotoMode::People)), ..facts.clone() };
        wheel_scene(&counting, None, Some(bloom), &format!("vr-wheel-{world}-countdown"))?;
        // The Quick Menu where it opens before the eyes, the wing beside it.
        }
        // Held a little to the left of straight ahead, as the hand that holds it is: the menu, a sheet on it and the wing beside it are all in the picture.
        let mut held = space::before_eyes(&head, 0.5, 0.08);
        held.m[0][3] -= 0.12;
        let plane = vr::quick_menu_plane(&vr::grip_holding(&held, &tune.quick_menu(1)), 1, &tune, 1.0);
        // The Quick Menu's side tiles as the app lays them on this hand or in the strip: with a page open they stay, beside it (Enzo, repeatedly; latest 2026-11-13).
        let side_tiles = |side: SideTiles| -> Vec<zones::Placed> {
            match side {
                SideTiles::Wing(hand) => vr::quick_menu_places(hand),
                SideTiles::Strip => vr::strip_places(),
            }
        };
        let qm_scene_on = |side: SideTiles, facts: &Facts, hover: Option<usize>, sheet: Option<Sheet>, name: &str| -> anyhow::Result<()> {
            let mut eye = Eye::new(size, &ground, eyes);
            let center_units = zones::Placed { zone: Zone::SheetBack, rect: qm_units, center: pos2(0.0, 0.0), unit: 1.0 };
            eye.quad(&plane.quad(&center_units, 0.0, 0.0), &qm, qm_px, 1.0);
            let mut end = None;
            // Where the headset has them: their gap from My Profile, as the app lays them (the renders had none, and judges saw it touch); with a page up, stood off it
            // at the page's depth, as `vr::near` has them.
            let sheet_up = sheet.is_some();
            let parts = side_tiles(side);
            let shown: Vec<Zone> = parts.iter().map(|p| p.zone).filter(|z| *z != Zone::QmStop || facts.recording).collect();
            for zone in shown {
                let placed = parts.iter().copied().find(|p| p.zone == zone).unwrap_or_else(|| zones::in_units(zone));
                let hovered = hover.filter(|_| zone == Zone::WingWorlds).map(|_| 0);
                let (pixels, px) = sharp_picture(&placed, facts, hovered, 2.0);
                eye.quad(&plane.quad(&vr::clear_of_sheet(placed, sheet_up), 0.0, if sheet_up { 0.02 } else { 0.004 }), &pixels, px, 1.0);
                if let Some(i) = hovered {
                    let item = panels::items(zone, placed.logical(), facts)[i].rect.center().to_vec2() + placed.rect.min.to_vec2();
                    end = Some(space::apply(&plane.frame, [item.x * plane.unit, -item.y * plane.unit, 0.01]));
                }
            }
            match sheet {
                // VRChat's own menus reached: only the way back, where the head's button was.
                Some(_) if facts.peek => {
                    let back = vr::clear_of_sheet(vr::sheet_place(Zone::SheetBack), true);
                    let at = panels::peek_place(facts, back.logical().x);
                    let pill = zones::Placed { zone: Zone::SheetPeek, rect: egui::Rect::from_min_size(back.rect.min + at.min.to_vec2() * back.unit, at.size() * back.unit), center: back.center, unit: back.unit };
                    let (pixels, px) = sharp_picture(&pill, facts, None, 2.0);
                    eye.quad(&plane.quad(&pill, 0.0, 0.02), &pixels, px, 1.0);
                }
                Some(sheet) => {
                    let (quad, pixels, px) = vr_sheet(facts, sheet, &plane);
                    eye.quad(&quad, &pixels, px, 1.0);
                }
                None => {}
            }
            if let Some(end) = end {
                eye.laser([0.26, 1.05, -0.25], end);
            }
            eye.save(out, name)
        };
        let qm_scene = |facts: &Facts, hover: Option<usize>, sheet: Option<Sheet>, name: &str| qm_scene_on(SideTiles::Wing(0), facts, hover, sheet, name);
        if !only_small {
        qm_scene(&facts, None, None, &format!("vr-qm-{world}"))?;
        // The side tiles stay while a page is open (Enzo, repeatedly; latest 2026-11-13): on either hand's side, and in the strip under the menu.
        let youtube = Facts { sheet: Some(Sheet::Results), sheet_title: "YouTube".into(), ..facts.clone() };
        qm_scene_on(SideTiles::Wing(1), &youtube, None, Some(Sheet::Results), &format!("vr-sheet-{world}-youtube-wing-left"))?;
        qm_scene_on(SideTiles::Strip, &youtube, None, Some(Sheet::Results), &format!("vr-sheet-{world}-youtube-strip"))?;
        qm_scene(&youtube, None, Some(Sheet::Results), &format!("vr-sheet-{world}-youtube"))?;
        let tracking = Facts { sheet: Some(Sheet::Tracking), sheet_title: "Smooth tracking".into(), tracking: Some((true, [0, 35, 78], "Smoothing your devices now".to_string())), ..facts.clone() };
        qm_scene(&tracking, None, Some(Sheet::Tracking), &format!("vr-sheet-{world}-tracking"))?;
        qm_scene(&Facts { sheet: Some(Sheet::Talk), sheet_title: "Ask N4N0".into(), ..facts.clone() }, None, Some(Sheet::Talk), &format!("vr-sheet-{world}-talk"))?;
        qm_scene(&Facts { sheet: Some(Sheet::Worlds), sheet_title: "Worlds".into(), ..facts.clone() }, None, Some(Sheet::Worlds), &format!("vr-sheet-{world}-worlds"))?;
        qm_scene(&Facts { sheet: Some(Sheet::Instances), sheet_title: "Instances".into(), ..facts.clone() }, None, Some(Sheet::Instances), &format!("vr-sheet-{world}-instances"))?;
        qm_scene(&Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), recording: true, recording_seconds: Some(42), ..facts.clone() }, None, Some(Sheet::Pictures), &format!("vr-sheet-{world}-pictures-recording"))?;
        qm_scene(&pointing(Action::Worlds), Some(0), None, &format!("vr-qm-{world}-point-worlds"))?;
        qm_scene(&Facts { sheet: Some(Sheet::All), sheet_title: "All tools".into(), ..facts.clone() }, None, Some(Sheet::All), &format!("vr-sheet-{world}-all"))?;
        qm_scene(&Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), ..facts.clone() }, None, Some(Sheet::Pictures), &format!("vr-sheet-{world}-pictures"))?;
        // Enzo's IMG1 (2026-11-11): All tools with what plays, its timeline, its volume at 0 and its two links.
        let playing = Facts { sheet: Some(Sheet::All), sheet_title: "All tools".into(), music: Some(("DJ Brace - 2016 DMC Online Finals (Winning Routine)".into(), true)), music_times: Some((364, 364)), volume: Some((0, false)), can_copy_playing: true, can_drop_playing: true, ..facts.clone() };
        qm_scene(&playing, None, Some(Sheet::All), &format!("vr-sheet-{world}-all-playing"))?;
        // A capture around the person: Hide 3D and Small here in the head, the tabs as icons where their names do not fit.
        let around = Facts { sheet: Some(Sheet::Instances), sheet_title: "Instances".into(), view_showing: true, ..facts.clone() };
        qm_scene(&around, None, Some(Sheet::Instances), &format!("vr-sheet-{world}-instances-capture"))?;
        qm_scene(&Facts { view_small: true, ..around.clone() }, None, Some(Sheet::Instances), &format!("vr-sheet-{world}-instances-capture-small"))?;
        // A page of N4N0's own: its name after the tabs.
        qm_scene(&Facts { sheet: Some(Sheet::Settings), sheet_title: "Settings".into(), ..facts.clone() }, None, Some(Sheet::Settings), &format!("vr-sheet-{world}-settings"))?;
        // VRChat's own menus reached from a sheet: the way back alone.
        qm_scene(&Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), peek: true, ..facts.clone() }, None, Some(Sheet::Pictures), &format!("vr-sheet-{world}-peek"))?;
        }
        let _ = Target::Close;
        // A real capture standing small, drawn by the view's own renderer: in the hole of a world's side panel (the panel's picture
        // beside the frames, `panel-small-docked.png` of --world-view), then in a hand with what says how to zoom it.
        let sessions = std::env::var("ORBIT_SESSION").unwrap_or_default();
        for (which, session) in sessions.split(';').filter(|s| !s.is_empty() && world == "forest").enumerate() {
            use crate::overlay::worlds::{catalog, render, small_hint, stereo};
            let names = if which == 0 { String::new() } else { format!("-{which}") };
            let full = catalog::read_local(Path::new(session), &Default::default())?;
            let capture = full.around_subject();
            println!("{session}: reach {} of {} splats {} showing", capture.small_reach(), full.gaussians.len(), capture.within(capture.small_reach()));
            let (_adapter, device, queue) = render::device(None)?;
            let mut renderer = render::FaceRenderer::new(device, queue);
            renderer.load(&capture)?;
            renderer.make_transparent()?;
            let (w, h) = (size[0], size[1]);
            let tangents_of = |half: f32| {
                let tan = half.to_radians().tan();
                [-tan, tan, -tan * h as f32 / w as f32, tan * h as f32 / w as f32]
            };
            let mut half_fov = VR_SHOT_HALF_FOV;
            let shared = std::sync::Arc::new(full.around_subject());
            let mut model = |small: &stereo::Small, half: f32| -> anyhow::Result<Vec<u8>> {
                let camera = stereo::eye_camera(&head, tangents_of(half), w, h, &small.placement(capture.view.eye()));
                // As the app draws it: only what it reaches, its floor fading at its edge.
                let ceiling = if small.docked { capture.view.ground() + small.reach() * stereo::HOLE_CEILING_PER_REACH } else { f32::INFINITY };
                renderer.clip(Some(&(small.middle(capture.view.eye()), small.reach(), ceiling, shared.clone())));
                // As many as the app draws: what it reaches from where its middle is.
                renderer.render_view(&camera, capture.within(small.reach() + small.moved_by()))
            };
            let panel = image::open(refs.join("panel-small-docked.png"))?.to_rgba8();
            let panel_px = [panel.width(), panel.height()];
            let mut panel_pixels = panel.into_raw();
            for p in panel_pixels.chunks_exact_mut(4) {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = (*c as u32 * a / 255) as u8;
                }
            }
            let page = vr::sheet_place(Zone::Takeover);
            let page_quad = plane.quad(&vr::clear_of_sheet(page, true), 0.0, 0.02);
            let hole = crate::overlay::worlds::panel::VR_HERO;
            let (x, y) = ((hole.center().x / 1024.0 - 0.5) * page_quad.width, (0.5 - hole.center().y / 576.0) * page_quad.height);
            let there = orbiters_openvr::Matrix34 { m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, 0.0]] };
            let at = space::compose(&page_quad.transform, &there);
            let radius = hole.width() / 1024.0 * page_quad.width / 2.0 * crate::overlay::worlds::SMALL_FILLS_HOLE;
            let dock = stereo::SmallDock::behind(&capture.view, &at, radius, capture.hole_frame().1);
            let small = stereo::Small::new(&capture.view, &head, capture.small_reach()).at_dock(&dock).framed(capture.hole_frame());
            println!("in its hole: its middle {:?} from the start eye, reaching {} of {}", capture.hole_frame().0, small.reach(), capture.small_reach());
            // Docked.
            let worlds = Facts { sheet: Some(Sheet::Worlds), sheet_title: "Worlds in 3D".into(), view_showing: true, view_small: true, sheet_hole: Some(egui::Rect::from_min_max(pos2(hole.min.x * 0.6875, hole.min.y * 0.6875 + panels::SHEET_HEAD), pos2(hole.max.x * 0.6875, hole.max.y * 0.6875 + panels::SHEET_HEAD))), ..facts.clone() };
            let center_units = zones::Placed { zone: Zone::SheetBack, rect: qm_units, center: pos2(0.0, 0.0), unit: 1.0 };
            let mut eye = Eye::new(size, &ground, eyes);
            eye.quad(&plane.quad(&center_units, 0.0, 0.0), &qm, qm_px, 1.0);
            // Nothing behind it in the hole (Enzo, 2026-11-11): the capture, then the sheet, its back and its page one picture, the wing away.
            eye.over(&model(&small, half_fov)?);
            let back = vr::clear_of_sheet(vr::sheet_place(Zone::SheetBack), true);
            let (back_pixels, back_px) = sharp_picture(&back, &worlds, None, 2.0);
            let mut panel_pixels = panel_pixels;
            panels::cut_page_corners(&mut panel_pixels, panel_px, [0, 0, panel_px[0], panel_px[1]]);
            if panel_px[0] == back_px[0] {
                let whole = vr::compose_sheet(&back_pixels, back_px, &panel_pixels, panel_px, back_px[1] - panel_px[1] - 11);
                eye.quad(&plane.quad(&back, 0.0, 0.02), &whole, back_px, 1.0);
            } else {
                eye.quad(&plane.quad(&back, 0.0, 0.02), &back_pixels, back_px, 1.0);
                eye.quad(&page_quad, &panel_pixels, panel_px, 1.0);
            }
            eye.save(out, &format!("vr-small-in-the-panel{names}"))?;
            // In a hand, with what says how to zoom it, and its Hide.
            let mut held = stereo::Small::new(&capture.view, &head, capture.small_reach());
            held.anchor = [0.0, 1.45, -0.62];
            half_fov = HAND_SHOT_HALF_FOV;
            let mut eye = Eye::seeing(size, &ground, eyes, half_fov);
            eye.over(&model(&held, half_fov)?);
            let hint_px = small_hint::PX;
            let mut canvas = Canvas::new(hint_px[0], hint_px[1]);
            for _ in 0..2 {
                canvas.paint(0.0, |p| small_hint::paint(p, small_hint::Stage::Stick { hand: 1 }, 0.65));
            }
            let up = small_hint::hint_place(held.anchor, held.radius(), space::position(&head));
            let hint_frame = small_hint::facing(up, space::position(&head));
            let hint_w = small_hint::WIDTH_M;
            eye.quad(&Quad { transform: hint_frame, width: hint_w, height: hint_w * hint_px[1] as f32 / hint_px[0] as f32 }, canvas.pixels(), hint_px, 1.0);
            // Its stick, as the left eye sees it from where it is: what the headset gets is a picture for each eye.
            {
                use crate::overlay::worlds::stick3d;
                let middle = space::add(space::add(space::position(&hint_frame), space::scale(space::axis(&hint_frame, 0), small_hint::STICK_AT[0])), space::scale(space::axis(&hint_frame, 1), small_hint::STICK_AT[1]));
                let mut there = hint_frame;
                for (i, row) in there.m.iter_mut().enumerate() {
                    row[3] = middle[i];
                }
                let own = |eye: [f32; 3]| {
                    let d = space::sub(eye, middle);
                    [space::dot(d, space::axis(&there, 0)), space::dot(d, space::axis(&there, 1)), space::dot(d, space::axis(&there, 2))]
                };
                let left = space::position(&head);
                let pair = stick3d::picture([own(left), own(left)], 0.85, crate::overlay::orbit::panels::look::accent());
                let (w, h) = (stick3d::EYE_PX[0] as usize, stick3d::EYE_PX[1] as usize);
                let mut one = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    one.extend_from_slice(&pair[y * w * 2 * 4..y * w * 2 * 4 + w * 4]);
                }
                eprintln!("stick: eye {:?} solid {} middle {:?}", own(left), one.chunks_exact(4).filter(|p| p[3] > 20).count(), middle);
                eye.quad(&Quad { transform: there, width: stick3d::WIDTH_M, height: stick3d::HEIGHT_M }, &one, stick3d::EYE_PX, 1.0);
            }
            let mut canvas = Canvas::new(small_hint::CHIP_PX[0], small_hint::CHIP_PX[1]);
            for _ in 0..2 {
                canvas.paint(0.0, |p| small_hint::paint_chip(p, false));
            }
            let chip = small_hint::chip_place(held.anchor, held.radius(), space::position(&head), true);
            eye.quad(&Quad { transform: small_hint::chip_quad(&chip), width: small_hint::CHIP_WIDTH_M, height: small_hint::CHIP_WIDTH_M * small_hint::CHIP_PX[1] as f32 / small_hint::CHIP_PX[0] as f32 }, canvas.pixels(), small_hint::CHIP_PX, 1.0);
            eye.save(out, &format!("vr-small-in-a-hand{names}"))?;
            // G1 (Enzo, 2026-11-12): our backdrop between the capture and VRChat's menu behind the sheet, as each eye sees it, before the sheet and
            // from both sides, the Worlds page painted in the language now; and, for the comparison, the hole bare as dev.40 had it.
            {
                use crate::overlay::worlds::opening::{self, Look, Opening};
                let page_px = [(page.rect.width() * 2.0).round() as u32, (page.rect.height() * 2.0).round() as u32];
                let mut page_pixels = crate::overlay::worlds::panel::side_small(page_px)?;
                panels::cut_page_corners(&mut page_pixels, page_px, [0, 0, page_px[0], page_px[1]]);
                let back_quad = plane.quad(&back, 0.0, 0.02);
                let top = ((page.rect.top() - vr::sheet_place(Zone::SheetBack).rect.top()) * 2.0).round() as u32;
                let sheet_pixels = vr::compose_sheet(&back_pixels, back_px, &page_pixels, page_px, top);
                let half = [hole.width() / 1024.0 * page_quad.width / 2.0, hole.height() / 576.0 * page_quad.height / 2.0];
                let accent = panels::look::accent();
                let o = Opening::in_sheet(at, half, opening::rect_from(&at, &back_quad.transform, back_quad.width, back_quad.height), radius, Look::ours([accent.r(), accent.g(), accent.b()], panels::look::retro()));
                let placement = small.placement(capture.view.eye());
                let ceiling = capture.view.ground() + small.reach() * stereo::HOLE_CEILING_PER_REACH;
                renderer.clip(Some(&(small.middle(capture.view.eye()), small.reach(), ceiling, shared.clone())));
                let count = capture.within(small.reach() + small.moved_by());
                for (view_name, offset) in [("left-eye", [-0.032, 0.0, 0.0]), ("right-eye", [0.032, 0.0, 0.0]), ("from-the-left", [-0.34, 0.04, 0.02]), ("from-the-right", [0.30, -0.06, 0.02])] {
                    let here = space::add(eyes, offset);
                    let mut view = head;
                    for (i, row) in view.m.iter_mut().enumerate() {
                        row[3] = here[i];
                    }
                    let camera = stereo::eye_camera(&view, tangents_of(VR_SHOT_HALF_FOV), w, h, &placement);
                    let capture_pixels = renderer.render_view(&camera, count)?;
                    for (with, suffix) in [(true, ""), (false, "-bare")] {
                        let mut eye = Eye::seeing(size, &ground, here, VR_SHOT_HALF_FOV);
                        eye.quad(&plane.quad(&zones::Placed { zone: Zone::SheetBack, rect: qm_units, center: pos2(0.0, 0.0), unit: 1.0 }, 0.0, 0.0), &qm, qm_px, 1.0);
                        let mut shown = capture_pixels.clone();
                        if with {
                            for (i, p) in shown.chunks_exact_mut(4).enumerate() {
                                let (x, y) = (i as u32 % w, i as u32 / w);
                                let c = [p[0], p[1], p[2], p[3]].map(|v| v as f32 / 255.0);
                                let seen = opening::texel(c, Some(&o), here, eye.ray(x as f32 + 0.5, y as f32 + 0.5), [x, y]);
                                for k in 0..4 {
                                    p[k] = (seen[k] * 255.0).round().clamp(0.0, 255.0) as u8;
                                }
                            }
                        }
                        eye.over(&shown);
                        eye.quad(&back_quad, &sheet_pixels, back_px, 1.0);
                        eye.save(out, &format!("vr-small-window-{view_name}{suffix}{names}"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// A panel's picture for the headset: `sharp` pixels a unit, cut to its shape, pictures of recent captures standing in.
fn sharp_picture(placed: &Placed, facts: &Facts, hover: Option<usize>, sharp: f32) -> (Vec<u8>, [u32; 2]) {
    let px = [(placed.rect.width() * sharp).round() as u32, (placed.rect.height() * sharp).round() as u32];
    let logical = placed.logical();
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / logical.x).in_orbit_look();
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    facts.recent_pictures = textures.iter().enumerate().map(|(n, texture)| (n, texture.id())).collect();
    facts.hero_captures = textures.iter().take(3).map(|texture| texture.id()).collect();
    facts.hero_picture = textures.first().map(|texture| texture.id());
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint(p, placed.zone, logical, &facts, hover, None));
    }
    let mut out = canvas.pixels().to_vec();
    if let Some(ring) = placed.zone.ring() {
        let mask = panels::cell_mask(ring, px, px[0] as f32 / logical.x);
        panels::apply_mask(&mut out, &mask, px, [0, 0, px[0], px[1]]);
    }
    (out, px)
}

pub(super) fn shots(out: &Path) -> anyhow::Result<()> {
    use orbiters_n4n0_core::menu::{PhotoMode, Target};
    use panels::{Action, Sheet};
    let hd = vec2(1280.0, 720.0);
    // `ORBIT_ONLY_OVER_THE_GAME`: only the states painted over a real frame of the game, which is quick.
    if std::env::var_os("ORBIT_ONLY_OVER_THE_GAME").is_some() {
        let idle = Facts { recent: recent(), timer: 3, n4n0: true, desktop: true, music: Some(("The Weeknd - Moth to a Flame".into(), true)), ..Default::default() };
        return round_18_over_the_game(out, &idle);
    }
    let idle = Facts { recent: recent(), timer: 3, n4n0: true, desktop: true, music: Some(("The Weeknd - Moth to a Flame".into(), true)), ..Default::default() };
    let hint = Some(("Hold Left Alt to use this ring", "Your mouse then points here instead of at VRChat's wheel."));
    let empty = Facts { recent: Default::default(), music: None, ..idle.clone() };
    let busy = Facts { recording: true, recording_seconds: Some(83), capture_busy: true, reconstructing: Some(0.42), waiting: 2, ..idle.clone() };
    let counting = Facts { countdown: Some(2), picture_mode: Some(PhotoMode::People), pointed: Some(Action::Picture(PhotoMode::People)), ..idle.clone() };
    let pointing = |a| Facts { pointed: Some(a), ..idle.clone() };
    let bloom = zones::Group::Pictures;
    let playing = zones::Group::Playing;
    wheel_scene("orbit-wheel-idle", out, hd, &Facts { hint, ..idle.clone() }, None, None, false)?;
    wheel_scene("orbit-wheel-empty", out, hd, &Facts { hint, ..empty.clone() }, None, None, false)?;
    wheel_scene("orbit-wheel-busy", out, hd, &busy, None, Some(bloom), true)?;
    wheel_scene("orbit-wheel-countdown", out, hd, &counting, None, Some(bloom), true)?;
    wheel_scene("orbit-wheel-point-worlds", out, hd, &Facts { key_caps: true, ..pointing(Action::Worlds) }, Some((Zone::WheelWorlds, 0)), None, true)?;
    wheel_scene("orbit-wheel-point-pictures", out, hd, &pointing(Action::Pictures), Some((Zone::WheelPictures, 0)), Some(bloom), true)?;
    wheel_scene("orbit-wheel-point-people", out, hd, &pointing(Action::Picture(PhotoMode::People)), Some((Zone::BloomPeople, 0)), Some(bloom), true)?;
    wheel_scene("orbit-wheel-point-playing", out, hd, &pointing(Action::PlayPause), Some((Zone::WheelPlaying, 0)), Some(playing), true)?;
    wheel_scene("orbit-wheel-1080", out, vec2(1920.0, 1080.0), &Facts { hint, ..idle.clone() }, None, None, false)?;
    wheel_scene("orbit-wheel-540", out, vec2(960.0, 540.0), &Facts { hint, ..idle.clone() }, None, None, false)?;
    wheel_scene("orbit-wheel-narrow", out, vec2(800.0, 800.0), &Facts { hint, ..idle.clone() }, None, None, false)?;
    qm_scene("orbit-qm-idle", out, hd, &idle, None, None)?;
    qm_scene("orbit-qm-empty", out, hd, &empty, None, None)?;
    qm_scene("orbit-qm-busy", out, hd, &busy, None, None)?;
    qm_scene("orbit-qm-point", out, hd, &pointing(Action::Worlds), Some(0), None)?;
    qm_scene("orbit-qm-narrow", out, vec2(800.0, 800.0), &idle, None, None)?;
    qm_scene("orbit-sheet-all", out, hd, &Facts { sheet: Some(Sheet::All), sheet_title: "All tools".into(), ..idle.clone() }, None, Some(Sheet::All))?;
    qm_scene("orbit-sheet-all-vr", out, hd, &Facts { desktop: false, can_call: true, can_states: true, can_light: true, sheet: Some(Sheet::All), sheet_title: "All tools".into(), ..idle.clone() }, None, Some(Sheet::All))?;
    // What plays with its link known, then copied; the page of microphones, with made-up ones.
    qm_scene("orbit-sheet-all-link", out, hd, &Facts { desktop: false, can_states: true, can_copy_playing: true, can_drop_playing: true, playing_link_copied: true, sheet: Some(Sheet::All), sheet_title: "All tools".into(), ..idle.clone() }, None, Some(Sheet::All))?;
    // VR feedback F3: no link card on the avatar: Drop link greyed; pointed at, the head says why and what to do (no N4N0 at all, an N4N0 from before the card).
    let no_card = Facts { desktop: false, can_states: true, can_copy_playing: true, can_drop_playing: false, drop_why: panels::DropWhy::NoN4n0, sheet: Some(Sheet::All), sheet_title: "All tools".into(), ..idle.clone() };
    qm_scene("orbit-sheet-all-link-no-n4n0", out, hd, &Facts { pointed: Some(Action::PlayingLink(Target::DropPlaying)), ..no_card.clone() }, None, Some(Sheet::All))?;
    qm_scene("orbit-sheet-all-link-old-n4n0", out, hd, &Facts { drop_why: panels::DropWhy::NoCard, pointed: Some(Action::PlayingLink(Target::DropPlaying)), ..no_card.clone() }, None, Some(Sheet::All))?;
    // VR feedback F10: a previous instance's row opens its world's page; its mark says so beside its name.
    let names = ["The Black Cat", "Midnight Rooftop ~ a very long world name that goes on", "Japan Shrine", "Cozy Cabin", "Just B Club", "Murder 4"];
    let visits = names
        .iter()
        .enumerate()
        .map(|(i, name)| panels::WorldVisit {
            world_id: format!("wrld_{i}"),
            name: name.to_string(),
            line: format!("{} h · eu · Public", i * 3 + 1),
            location: format!("wrld_{i}:1~region(eu)"),
            here: i == 0,
            earlier: if i == 1 { vec![("wrld_1:2".into(), "Mon 21:04".into()), ("wrld_1:3".into(), "Sun 18:40".into())] } else { Vec::new() },
            picture: None,
        })
        .collect();
    qm_scene("orbit-sheet-instances", out, hd, &Facts { desktop: false, worlds_visited: visits, sheet: Some(Sheet::Instances), sheet_title: "Instances".into(), ..idle.clone() }, None, Some(Sheet::Instances))?;
    let mics = vec![
        panels::MicTile { id: "a".into(), name: "Quest Pro microphone".into(), level: 0.72, can_hear: true, default: true },
        panels::MicTile { id: "b".into(), name: "Headset (Virtual Desktop Audio)".into(), level: 0.08, can_hear: true, default: false },
        panels::MicTile { id: "c".into(), name: "Webcam C920".into(), level: 0.3, can_hear: true, default: false },
        panels::MicTile { id: "d".into(), name: "Line in".into(), level: 0.0, can_hear: false, default: false },
    ];
    qm_scene("orbit-sheet-mics", out, hd, &Facts { desktop: false, mics, voice_on: true, sheet: Some(Sheet::Mics), sheet_title: "Your microphones".into(), ..idle.clone() }, None, Some(Sheet::Mics))?;
    qm_scene("orbit-sheet-pictures", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), ..idle.clone() }, None, Some(Sheet::Pictures))?;
    let lines = vec![(true, "What's the weather like in Paris?".to_string()), (false, "It is 14 degrees and raining in Paris, with clear skies expected by this evening.".to_string()), (true, "Find a world with rain".to_string()), (false, "After The Rain has a 3D capture you can walk in. Open Worlds in 3D to see it.".to_string())];
    // YouTube's results, stand-in pictures and names: a search needs typing, which no test does in the game.
    let videos: Vec<panels::VideoTile> = [
        ("Rain on a forest cabin, 8 hours", "Quiet Hours", "8:00:12"),
        ("Lo-fi beats for late-night worlds", "Night Radio", "1:12:03"),
        ("Moth to a Flame (official video)", "The Weeknd", "3:54"),
        ("Why VR worlds feel so big", "Spatial Notes", "14:21"),
        ("Ambient synth for building things", "Slow Machines", "58:40"),
        ("A walk through a rainy city at night", "Wander", "22:09"),
    ]
    .iter()
    .map(|(title, channel, duration)| panels::VideoTile { id: String::new(), title: title.to_string(), channel: channel.to_string(), duration: Some(duration.to_string()), picture: None })
    .collect();
    qm_scene("orbit-sheet-results", out, hd, &Facts { sheet: Some(Sheet::Results), sheet_title: "YouTube".into(), videos, videos_for: "rain sounds".into(), beside_wing: true, ..idle.clone() }, None, Some(Sheet::Results))?;
    results_over_the_game(out, hd, &idle)?;
    round_18_over_the_game(out, &idle)?;
    // Round 18's pre-check: the cell whose second ring is out stays lit while a cell of that ring is pointed (here one this avatar cannot take, with no kit).
    let locked = Facts { no_kit: true, bloomed: Some(Zone::WheelPictures), ..pointing(Action::Picture(PhotoMode::People)) };
    wheel_scene("orbit-wheel-parent-lit", out, vec2(1920.0, 1080.0), &locked, Some((Zone::BloomPeople, 0)), Some(bloom), true)?;
    settings_scenes(out, hd, &idle)?;
    // An answer on its way: the words so far, and no second copy when it is whole.
    let mut arriving = lines.clone();
    arriving.push((false, "Thirteen people are in this world right now, and two of your".to_string()));
    qm_scene("orbit-sheet-talk-arriving", out, hd, &Facts { sheet: Some(Sheet::Talk), sheet_title: "Ask N4N0".into(), talk_lines: arriving, talk: crate::overlay::orbit::panels::TalkNow::Thinking, beside_wing: true, ..idle.clone() }, None, Some(Sheet::Talk))?;
    // The FPS booster's states. The figures are sample data for the picture, not measured.
    booster_scenes(out, hd, &idle)?;
    translate_scenes(out, hd, &idle)?;
    qm_scene("orbit-sheet-tracking", out, hd, &Facts { sheet: Some(Sheet::Tracking), sheet_title: orbiters_i18n::tr("Smooth tracking").into(), tracking: Some((true, [0, 35, 78], "Smoothing your devices now".to_string())), beside_wing: true, ..idle.clone() }, None, Some(Sheet::Tracking))?;
    qm_scene("orbit-sheet-talk", out, hd, &Facts { sheet: Some(Sheet::Talk), sheet_title: "Ask N4N0".into(), talk_lines: lines.clone(), beside_wing: true, ..idle.clone() }, None, Some(Sheet::Talk))?;
    qm_scene("orbit-sheet-talk-listening", out, hd, &Facts { sheet: Some(Sheet::Talk), sheet_title: "Ask N4N0".into(), talk_lines: lines.clone(), talk: crate::overlay::orbit::panels::TalkNow::Listening(0.6), beside_wing: true, ..idle.clone() }, None, Some(Sheet::Talk))?;
    // VR feedback F23/F24: Ask signed out (said before a word), hands-free waiting, hearing with the words so far, making them out, by the button, and the
    // microphone stopped.
    {
        use crate::overlay::orbit::panels::TalkNow;
        let ask = |name: &str, talk: TalkNow, auto: bool, heard: Option<&str>, said: Vec<(bool, String)>| {
            let facts = Facts { sheet: Some(Sheet::Talk), sheet_title: "Ask N4N0".into(), talk, talk_auto: auto, talk_heard: heard.map(String::from), talk_lines: said, beside_wing: true, ..idle.clone() };
            qm_scene(name, out, hd, &facts, None, Some(Sheet::Talk))
        };
        ask("orbit-ask-sign-in", TalkNow::SignIn, true, None, Vec::new())?;
        ask("orbit-ask-waiting", TalkNow::Waiting, true, None, Vec::new())?;
        ask("orbit-ask-hearing", TalkNow::Listening(0.55), true, Some("What's the weather like in"), Vec::new())?;
        ask("orbit-ask-hearing-more", TalkNow::Listening(0.4), true, Some("And tomorrow"), lines.clone())?;
        ask("orbit-ask-finalizing", TalkNow::Finalizing, true, None, Vec::new())?;
        ask("orbit-ask-paused", TalkNow::Paused, true, None, lines.clone())?;
        ask("orbit-ask-manual", TalkNow::Ready, false, None, lines.clone())?;
        ask("orbit-ask-failed", TalkNow::Failed, true, None, Vec::new())?;
    }
    qm_scene("orbit-sheet-countdown", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), ..counting.clone() }, None, Some(Sheet::Pictures))?;
    qm_scene("orbit-sheet-pictures-busy", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), ..busy.clone() }, None, Some(Sheet::Pictures))?;
    let _ = Target::Close;
    Ok(())
}

/// A stand-in of the wheel with the ring round it: `hover` on a zone, `bloom` open, the veil while `pointing`.
fn wheel_scene(name: &str, out: &Path, client: Vec2, facts: &Facts, hover: Option<(Zone, usize)>, bloom: Option<zones::Group>, pointing: bool) -> anyhow::Result<()> {
    let (w, h) = (client.x as u32, client.y as u32);
    let mut ground = Canvas::new(w, h);
    stand_in_menu(Around::Wheel, client, &mut ground);
    let mut image = ground.pixels().to_vec();
    for placed in zones::layout(Around::Wheel, client) {
        if placed.zone.group().is_some_and(|g| Some(g) != bloom) || (placed.zone == Zone::WheelVeil && !pointing) {
            continue;
        }
        let hovered = hover.filter(|(z, _)| *z == placed.zone).map(|(_, i)| i);
        let (pixels, px) = picture(&placed, facts, hovered, None);
        blit(&mut image, w, &pixels, px, placed.rect.min);
    }
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

/// A stand-in of the Quick Menu with the wing beside it, and a sheet in front when one is open.
fn qm_scene(name: &str, out: &Path, client: Vec2, facts: &Facts, hover: Option<usize>, sheet: Option<panels::Sheet>) -> anyhow::Result<()> {
    let (w, h) = (client.x as u32, client.y as u32);
    let mut ground = Canvas::new(w, h);
    stand_in_menu(Around::QuickMenu, client, &mut ground);
    let mut image = ground.pixels().to_vec();
    // With a sheet up the wing is away (Enzo, 2026-11-11): the sheet's head has its tools.
    for placed in zones::layout_as(Around::QuickMenu, client, facts.orbit.layout == orbiters_core::settings::OrbitLayout::Strip).into_iter().filter(|_| sheet.is_none()) {
        let hovered = hover.filter(|_| placed.zone == Zone::WingWorlds).map(|_| 0);
        let (pixels, px) = picture(&placed, facts, hovered, None);
        blit(&mut image, w, &pixels, px, placed.rect.min);
    }
    if let Some(sheet) = sheet {
        let back = zones::place(Zone::SheetBack, client).expect("it fits");
        let (pixels, px) = picture(&back, facts, None, None);
        blit(&mut image, w, &pixels, px, back.rect.min);
        let page = zones::place(Zone::Takeover, client).expect("it fits");
        let pixels = sheet_picture(sheet, facts, [page.rect.width() as u32, page.rect.height() as u32]);
        blit(&mut image, w, &pixels, [page.rect.width() as u32, page.rect.height() as u32], page.rect.min);
    }
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

/// The tiles around VRChat's camera, found by the tracker on Enzo's own picture of it, twice as large as a real window would show it: where they land, in pixels. The tracker reads the picture; nothing here is placed by hand.
fn camera_scene(out: &Path, facts: &Facts) -> anyhow::Result<()> {
    use crate::overlay::orbit::camera_track;
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera");
    for (name, shown) in [("camera-headset-1.png", "orbit-camera-tiles-headset-1"), ("camera-headset-2.png", "orbit-camera-tiles-headset-2"), ("camera-panel-752x417.jpg", "orbit-camera-tiles")] {
        let picture = image::open(folder.join(name))?.to_rgba8();
        let (w, h) = (picture.width(), picture.height());
        let mut image = picture.as_raw().clone();
        let found = camera_track::find(&image, w as usize, h as usize, w as usize * 4).ok_or_else(|| anyhow::anyhow!("no panel found on {name}"))?;
        // At the tracker's own place, turned as it reads the panel, on the side it chooses with this picture as the view.
        let side = camera_track::side_with_room(|u| found.at(u), [0.0, w as f32, 0.0, h as f32], camera_track::Side::Left);
        let unit = found.tall;
        let shift = [(-side.sign()) * 205.0 * (1.0 - found.across / found.tall).max(0.0), 0.0];
        let facts = Facts { pointed: Some(panels::Action::Picture(orbiters_n4n0_core::menu::PhotoMode::People)), ..facts.clone() };
        for zone in Zone::CAMERA {
            if zone.only_while_needed() && !panels::needed(zone, &facts) {
                continue;
            }
            let units = if side == camera_track::Side::Right { zones::in_units(zone).mirrored().rect } else { zone.units() };
            let size = units.size() * unit;
            let px = [size.x.round().max(1.0) * 2.0, size.y.round().max(1.0) * 2.0];
            let placed = Placed { zone, rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(px[0], px[1])), center: egui::pos2(0.0, 0.0), unit: unit * 2.0 };
            let (pixels, drawn) = picture_of(&placed, &facts);
            // Each of its pixels where the plane has it, sampled from where it is on the tile.
            let middle = units.center();
            let at = |u: f32, v: f32| {
                let p = [middle.x + shift[0] + u * units.width(), middle.y + shift[1] + v * units.height()];
                [found.middle[0] + (found.right[0] * p[0] + found.down[0] * p[1]) * unit, found.middle[1] + (found.right[1] * p[0] + found.down[1] * p[1]) * unit]
            };
            let corners = [at(-0.5, -0.5), at(0.5, -0.5), at(-0.5, 0.5), at(0.5, 0.5)];
            let (x0, x1) = (corners.iter().map(|c| c[0]).fold(f32::MAX, f32::min) as i32, corners.iter().map(|c| c[0]).fold(f32::MIN, f32::max) as i32);
            let (y0, y1) = (corners.iter().map(|c| c[1]).fold(f32::MAX, f32::min) as i32, corners.iter().map(|c| c[1]).fold(f32::MIN, f32::max) as i32);
            for y in y0.max(0)..(y1 + 1).min(h as i32) {
                for x in x0.max(0)..(x1 + 1).min(w as i32) {
                    let d = [x as f32 + 0.5 - found.middle[0], y as f32 + 0.5 - found.middle[1]];
                    let p = [(d[0] * found.right[0] + d[1] * found.right[1]) / unit - shift[0], (d[0] * found.down[0] + d[1] * found.down[1]) / unit - shift[1]];
                    let (u, v) = ((p[0] - units.min.x) / units.width(), (p[1] - units.min.y) / units.height());
                    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                        continue;
                    }
                    let (sx, sy) = ((u * drawn[0] as f32) as usize, (v * drawn[1] as f32) as usize);
                    let from = (sy * drawn[0] as usize + sx) * 4;
                    let to = (y as usize * w as usize + x as usize) * 4;
                    // Premultiplied, as a panel is drawn.
                    let a = pixels[from + 3] as f32 / 255.0;
                    for ch in 0..3 {
                        image[to + ch] = (pixels[from + ch] as f32 + image[to + ch] as f32 * (1.0 - a)) as u8;
                    }
                }
            }
        }
        let path = out.join(format!("{shown}.png"));
        save_png(&path, &over_ground(&image, w, h))?;
        println!("{}: {} side, the panel {} px wide {:?}", path.display(), if side == camera_track::Side::Left { "left" } else { "right" }, (found.across * 410.0) as u32, found.middle);
    }
    Ok(())
}

/// The parts beside VRChat's camera in each of their states, round a plain stand-in of its panel (its picture, and the row of buttons VRChat draws under it), two
/// pixels a unit, the plane seen square on: the column beside it, what it says under it, the corners while it looks for a code, the page over its top edge. Then the
/// Stop on the wheel, beside the Quick Menu and on a sheet's head while a recording runs.
pub(super) fn camera_shots(out: &Path) -> anyhow::Result<()> {
    use orbiters_n4n0_core::menu::PhotoMode;
    use panels::{Action, CameraNow, CameraQr, DemoSwitch, Sheet, WebPage, WebState};
    let idle = Facts { recent: recent(), timer: 3, n4n0: true, desktop: false, vr_close: true, camera: CameraNow { kit: Some(true), ..Default::default() }, ..Default::default() };
    let camera = |change: &dyn Fn(&mut CameraNow)| {
        let mut camera = idle.camera.clone();
        change(&mut camera);
        Facts { camera, ..idle.clone() }
    };
    let no_kit = |demo: Option<DemoSwitch>| Facts { no_kit: true, camera: CameraNow { kit: Some(false), demo, ..Default::default() }, ..idle.clone() };
    let page = |state: WebState| Some(WebPage { url: "https://orbiters.cc/w/silent-dawn?from=qr".into(), title: String::new(), state, picture: None });
    let recording = Facts { recording: true, recording_seconds: Some(42), capture_busy: true, ..idle.clone() };
    let states: Vec<(&str, Facts, bool)> = vec![
        ("orbit-camera-idle", idle.clone(), false),
        ("orbit-camera-right", camera(&|c| c.on_the_right = true), true),
        ("orbit-camera-point-record", Facts { pointed: Some(Action::Record), ..idle.clone() }, false),
        ("orbit-camera-finding", camera(&|c| c.finding = true), false),
        ("orbit-camera-countdown", Facts { countdown: Some(3), picture_mode: Some(PhotoMode::People), capture_busy: true, ..idle.clone() }, false),
        ("orbit-camera-no-kit", no_kit(Some(DemoSwitch::Offered)), false),
        ("orbit-camera-no-kit-right", Facts { camera: CameraNow { on_the_right: true, ..no_kit(Some(DemoSwitch::Offered)).camera }, ..no_kit(Some(DemoSwitch::Offered)) }, true),
        ("orbit-camera-no-kit-signed-out", no_kit(None), false),
        ("orbit-camera-no-kit-pointed", Facts { pointed: Some(Action::Picture(PhotoMode::World)), ..no_kit(Some(DemoSwitch::Offered)) }, false),
        ("orbit-camera-demo-busy", no_kit(Some(DemoSwitch::Busy)), false),
        ("orbit-camera-demo-failed", no_kit(Some(DemoSwitch::Failed("VRChat did not answer: check your connection, then try again.".into()))), false),
        ("orbit-camera-demo-done", Facts { camera: CameraNow { demo: Some(DemoSwitch::Done), ..idle.camera.clone() }, ..idle.clone() }, false),
        ("orbit-camera-qr-looking", camera(&|c| c.qr = CameraQr::Looking), false),
        ("orbit-camera-qr-no-picture", camera(&|c| c.qr = CameraQr::NoPicture(orbiters_i18n::tr(&orbiters_n4n0_core::qr::no_camera_note(Some(true))).into())), false),
        ("orbit-camera-web-loading", camera(&|c| c.web = page(WebState::Loading)), false),
        ("orbit-camera-web-page", camera(&|c| c.web = Some(WebPage { title: "Silent Dawn · Orbiters".into(), ..page(WebState::Shown).expect("a page") })), false),
        ("orbit-camera-web-failed", camera(&|c| c.web = page(WebState::Failed("The site did not answer.".into()))), false),
        ("orbit-camera-recording", recording.clone(), false),
        ("orbit-camera-starting", Facts { rec_starting: true, capture_busy: true, ..idle.clone() }, false),
        ("orbit-camera-stopping", Facts { rec_stopping: true, capture_busy: true, ..idle.clone() }, false),
    ];
    for (name, facts, right) in &states {
        camera_state(name, out, facts, *right)?;
    }
    let hd = vec2(1280.0, 720.0);
    let desk = Facts { desktop: true, vr_close: false, ..recording.clone() };
    wheel_scene("orbit-wheel-recording", out, hd, &desk, None, None, false)?;
    qm_scene("orbit-qm-recording", out, hd, &desk, None, None)?;
    qm_scene("orbit-sheet-head-stop", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), beside_wing: true, ..desk.clone() }, None, Some(Sheet::Pictures))?;
    qm_scene("orbit-sheet-head-stop-view", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), view_showing: true, beside_wing: true, ..desk.clone() }, None, Some(Sheet::Pictures))?;
    // In a headset, with Hide 3D and Small here: its Stop is a round button beside them.
    qm_scene("orbit-sheet-head-stop-view-vr", out, hd, &Facts { desktop: false, vr_close: true, sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), view_showing: true, ..desk.clone() }, None, Some(Sheet::Pictures))?;
    qm_scene("orbit-sheet-head-stop-vr", out, hd, &Facts { desktop: false, vr_close: true, sheet: Some(Sheet::Instances), sheet_title: "Instances".into(), ..desk.clone() }, None, Some(Sheet::Instances))?;
    qm_scene("orbit-sheet-head-stopping", out, hd, &Facts { sheet: Some(Sheet::All), sheet_title: "All tools".into(), recording: false, rec_stopping: true, beside_wing: true, ..desk.clone() }, None, Some(Sheet::All))?;
    qm_scene("orbit-sheet-pictures-idle", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), beside_wing: true, ..Facts { desktop: true, ..idle.clone() } }, None, Some(Sheet::Pictures))?;
    Ok(())
}

/// One state of the camera's parts round the stand-in panel.
fn camera_state(name: &str, out: &Path, facts: &Facts, right: bool) -> anyhow::Result<()> {
    const PX: f32 = 2.0;
    let (x0, y0, x1, y1) = (-340.0f32, -660.0f32, 340.0f32, 240.0f32);
    let (w, h) = (((x1 - x0) * PX) as u32, ((y1 - y0) * PX) as u32);
    let at = move |x: f32, y: f32| pos2((x - x0) * PX, (y - y0) * PX);
    let mut ground = Canvas::new(w, h);
    ground.paint(0.0, |p| {
        p.rect_filled(egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(w as f32, h as f32)), 0.0, Color32::from_rgb(58, 62, 70));
        // The camera's picture in its rim, its close at the top left, its arrow at the foot; its buttons under it.
        let picture = egui::Rect::from_min_max(at(-205.0, -110.0), at(205.0, 110.0));
        p.rect_filled(picture.expand(5.0 * PX), 16.0 * PX, Color32::from_rgb(18, 20, 24));
        p.rect_filled(picture, 12.0 * PX, Color32::from_rgb(112, 124, 136));
        let x = at(-181.0, -83.0);
        let stroke = egui::Stroke::new(3.0 * PX, Color32::WHITE);
        p.line_segment([x + vec2(-8.0, -8.0) * PX, x + vec2(8.0, 8.0) * PX], stroke);
        p.line_segment([x + vec2(-8.0, 8.0) * PX, x + vec2(8.0, -8.0) * PX], stroke);
        let chevron = at(-8.0, 100.0);
        p.line_segment([chevron + vec2(-9.0, 4.0) * PX, chevron + vec2(0.0, -4.0) * PX], stroke);
        p.line_segment([chevron + vec2(0.0, -4.0) * PX, chevron + vec2(9.0, 4.0) * PX], stroke);
        for i in 0..6 {
            let left = -165.0 + i as f32 * 52.0;
            p.rect_filled(egui::Rect::from_min_max(at(left, 114.0), at(left + 44.0, 156.0)), 6.0 * PX, Color32::from_rgb(34, 40, 46));
        }
    });
    let mut image = ground.pixels().to_vec();
    for zone in Zone::CAMERA {
        if zone.only_while_needed() && !panels::needed(zone, facts) {
            continue;
        }
        let units = if right { zones::in_units(zone).mirrored().rect } else { zone.units() };
        let rect = egui::Rect::from_min_max(at(units.min.x, units.min.y), at(units.max.x, units.max.y));
        let placed = Placed { zone, rect, center: at(0.0, 0.0), unit: PX };
        let hovered = facts.pointed.and_then(|a| panels::items(zone, units.size(), facts).iter().position(|i| i.action == a));
        let (pixels, px) = picture(&placed, facts, hovered, None);
        blit(&mut image, w, &pixels, px, rect.min);
    }
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

fn picture_of(placed: &Placed, facts: &Facts) -> (Vec<u8>, [u32; 2]) {
    picture(placed, facts, None, None)
}

/// The sheet of settings, and what it chooses: another accent, the retro look of one colour and with its pictures in theirs, the strip.
fn settings_scenes(out: &Path, client: Vec2, idle: &Facts) -> anyhow::Result<()> {
    use orbiters_core::settings::{MenuLook, OrbitLayout, OrbitSettings};
    use panels::Sheet;
    let sheet = |orbit: OrbitSettings, which: Sheet, title: &str| Facts { orbit, sheet: Some(which), sheet_title: orbiters_i18n::tr(title).into(), beside_wing: true, vr_output: Some("Speakers (Realtek Audio)".into()), ..idle.clone() };
    let plain = OrbitSettings::default();
    camera_scene(out, idle)?;
    camera_shots(out)?;
    qm_scene("orbit-settings", out, client, &sheet(plain, Sheet::Settings, "Settings"), None, Some(Sheet::Settings))?;
    let pink = OrbitSettings { accent: panels::SWATCHES[4], ..plain };
    panels::set_style(&pink);
    qm_scene("orbit-settings-accent", out, client, &sheet(pink, Sheet::Settings, "Settings"), None, Some(Sheet::Settings))?;
    let mono = OrbitSettings { theme: MenuLook::Retro, ..plain };
    panels::set_style(&mono);
    qm_scene("orbit-settings-retro", out, client, &sheet(mono, Sheet::Settings, "Settings"), None, Some(Sheet::Settings))?;
    qm_scene("orbit-retro-mono", out, client, &sheet(mono, Sheet::Pictures, "Pictures"), None, Some(Sheet::Pictures))?;
    let colour = OrbitSettings { theme: MenuLook::Retro, retro_color: panels::SWATCHES[7], retro_pictures: true, ..plain };
    panels::set_style(&colour);
    qm_scene("orbit-retro-colour", out, client, &sheet(colour, Sheet::Pictures, "Pictures"), None, Some(Sheet::Pictures))?;
    panels::set_style(&mono);
    qm_scene("orbit-settings-retro-language", out, client, &Facts { language_choice: Some(orbiters_i18n::Language::Ja), ..sheet(mono, Sheet::Settings, "Settings") }, None, Some(Sheet::Settings))?;
    qm_scene("orbit-retro-all", out, client, &sheet(mono, Sheet::All, "All tools"), None, Some(Sheet::All))?;
    let kept = booster_snapshot(|snap| snap.booster.overlay.report.comparisons = vec![sample_comparison(orbiters_booster::trial::Verdict::Kept)]);
    qm_scene("orbit-retro-booster", out, client, &Facts { booster: panels::BoosterFacts::of(&kept, false, false, None), ..sheet(mono, Sheet::Booster, "FPS booster") }, None, Some(Sheet::Booster))?;
    let strip = OrbitSettings { layout: OrbitLayout::Strip, ..mono };
    qm_scene("orbit-retro-strip", out, client, &Facts { orbit: strip, strip_caption: true, vr_close: true, ..idle.clone() }, Some(0), None)?;
    wheel_scene("orbit-retro-wheel", out, client, &Facts { orbit: mono, ..idle.clone() }, None, None, false)?;
    page_scene("orbit-retro-page", out, &mono)?;
    let colour = OrbitSettings { theme: MenuLook::Retro, retro_color: panels::SWATCHES[7], retro_pictures: true, ..plain };
    panels::set_style(&colour);
    page_scene("orbit-retro-page-colour", out, &colour)?;
    panels::set_style(&pink);
    page_scene("orbit-accent-page", out, &pink)?;
    panels::set_style(&plain);
    panels::set_style(&plain);
    let strip = OrbitSettings { layout: OrbitLayout::Strip, ..plain };
    qm_scene("orbit-strip", out, client, &Facts { orbit: strip, strip_caption: true, hint: Some(("Your tools", "Worlds in 3D, videos, instances and 3D pictures.")), ..idle.clone() }, None, None)?;
    Ok(())
}

/// One of N4N0's own pages inside a sheet, as the sheet draws it with what is chosen: the accent reaching it, or its retro drawing under the one colour.
fn page_scene(name: &str, out: &Path, chosen: &orbiters_core::settings::OrbitSettings) -> anyhow::Result<()> {
    use crate::overlay::{menu_pages, menu_retro, menu_ui};
    use orbiters_core::settings::MenuLook;
    let px = [1024u32, 576];
    let mut canvas = Canvas::zoomed(px[0], px[1], 1.0).in_orbit_look();
    canvas.set_restyle_off(true);
    let retro = chosen.theme == MenuLook::Retro;
    let (_, menu, _, _) = menu_pages::samples().into_iter().find(|(page, ..)| format!("{page:?}").contains("Photo")).expect("a page with pictures");
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let skin = crate::overlay::orbit::page_skin(chosen.accent);
    let mut n = std::cell::Cell::new(0usize);
    let picture = |_: &str| {
        let i = n.get();
        n.set(i + 1);
        Some(textures[i % textures.len()].id())
    };
    let view = menu_ui::View { menu: &menu, hover: None, pressed: None, picture: &picture, time: 0.4, skin: &skin };
    for _ in 0..2 {
        canvas.paint(0.4, |p| if retro { menu_retro::draw(p, egui::pos2(0.0, 0.0), &view) } else { menu_ui::draw(p, egui::pos2(0.0, 0.0), &view) });
    }
    *n.get_mut() = 0;
    let from = canvas.pixels().to_vec();
    let mut image = from.clone();
    panels::finish_picture(&mut image, &from, px, 1.0, [0, 0, px[0], px[1]]);
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, px[0], px[1]))?;
    println!("{}", path.display());
    Ok(())
}

/// YouTube's results as they are: a real search, its real pictures, over a real frame of the game (`ORBIT_QM_FRAME`), a result's Drop in world pointed at. Typing is what starts a search
/// and a script cannot type there (the keyboard's hook leaves out what is not a real key): this is painted, and says so where it is shown.
fn results_over_the_game(out: &Path, client: Vec2, idle: &Facts) -> anyhow::Result<()> {
    use orbiters_n4n0_core::youtube;
    let Ok(frame) = std::env::var("ORBIT_QM_FRAME") else { return Ok(()) };
    let (w, h) = (client.x as u32, client.y as u32);
    let ground = image::imageops::resize(&image::open(&frame)?.to_rgba8(), w, h, image::imageops::FilterType::Triangle);
    let found = youtube::search("rain sounds", "en")?;
    let page = zones::place(Zone::Takeover, client).expect("it fits");
    let px = [page.rect.width() as u32, page.rect.height() as u32];
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / 1024.0).in_orbit_look();
    let videos: Vec<panels::VideoTile> = found
        .videos
        .iter()
        .take(9)
        .map(|v| panels::VideoTile {
            id: v.id.clone(),
            title: v.title.clone(),
            channel: v.channel.clone(),
            duration: v.duration.clone(),
            picture: youtube::thumbnail(&v.id).ok().and_then(|bytes| image::load_from_memory(&bytes).ok()).map(|picture| {
                let picture = picture.to_rgb8();
                canvas.ctx().load_texture(v.id.clone(), ColorImage::from_rgb([picture.width() as usize, picture.height() as usize], picture.as_raw()), TextureOptions::LINEAR).id()
            }),
        })
        .collect();
    let facts = Facts { sheet: Some(panels::Sheet::Results), sheet_title: "YouTube".into(), videos, videos_for: "rain sounds".into(), beside_wing: true, hint_band: true, pointed: Some(panels::Action::VideoDrop(1)), ..idle.clone() };
    let items = panels::sheet_items(panels::Sheet::Results, &facts);
    let hover = items.iter().position(|i| i.action == panels::Action::VideoDrop(1));
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint_sheet(p, panels::Sheet::Results, &facts, hover, None));
    }
    let mut image = ground.as_raw().clone();
    for placed in zones::layout(Around::QuickMenu, client).into_iter().chain(zones::place(Zone::SheetHint, client)) {
        let (pixels, px) = picture(&placed, &facts, None, None);
        blit(&mut image, w, &pixels, px, placed.rect.min);
    }
    let back = zones::place(Zone::SheetBack, client).expect("it fits");
    let (pixels, back_px) = picture(&back, &facts, None, None);
    blit(&mut image, w, &pixels, back_px, back.rect.min);
    blit(&mut image, w, canvas.pixels(), px, page.rect.min);
    let path = out.join("orbit-sheet-results-real.png");
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

/// A sheet over a real frame of the game: the wing beside it, the sheet's head, and `page` as the sheet's page (premultiplied, the page's pixels).
fn over_the_game(name: &str, out: &Path, client: Vec2, ground: &image::RgbaImage, facts: &Facts, page: impl FnOnce([u32; 2]) -> anyhow::Result<Vec<u8>>) -> anyhow::Result<()> {
    let (w, h) = (client.x as u32, client.y as u32);
    let ground = image::imageops::resize(ground, w, h, image::imageops::FilterType::Triangle);
    let mut image = ground.as_raw().clone();
    for placed in zones::layout(Around::QuickMenu, client).into_iter().chain(zones::place(Zone::SheetHint, client)) {
        let (pixels, px) = picture(&placed, facts, None, None);
        blit(&mut image, w, &pixels, px, placed.rect.min);
    }
    let back = zones::place(Zone::SheetBack, client).expect("it fits");
    let (pixels, back_px) = picture(&back, facts, None, None);
    blit(&mut image, w, &pixels, back_px, back.rect.min);
    let place = zones::place(Zone::Takeover, client).expect("it fits");
    let px = [place.rect.width() as u32, place.rect.height() as u32];
    blit(&mut image, w, &page(px)?, px, place.rect.min);
    let path = out.join(format!("{name}.png"));
    save_png(&path, &over_ground(&image, w, h))?;
    println!("{}", path.display());
    Ok(())
}

/// What the sheet's live 3D view draws of the capture in `dir`, at rest and after a drag of `drag` (the page's units): real frames, from the renderer the
/// app uses. It does not look around by itself here, so a frame is the last one once no other comes.
fn live_frames(dir: &Path, drags: &[(f32, f32)]) -> anyhow::Result<Vec<ColorImage>> {
    use crate::overlay::orbit::sheet_view::{SheetView, ViewState};
    use std::time::{Duration, Instant};
    let view = SheetView::open(dir.to_path_buf(), false);
    let settled = |view: &SheetView| -> anyhow::Result<ColorImage> {
        let (began, mut last, mut newest) = (Instant::now(), Instant::now(), None);
        loop {
            if let Some(frame) = view.take_frame() {
                newest = Some(frame);
                last = Instant::now();
            }
            if let ViewState::Failed(why) = view.state() {
                anyhow::bail!("the capture's 3D: {why}");
            }
            if (newest.is_some() && last.elapsed() > Duration::from_millis(1500)) || began.elapsed() > Duration::from_secs(90) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let frame = newest.ok_or_else(|| anyhow::anyhow!("the capture's 3D drew nothing"))?;
        Ok(ColorImage::from_rgba_premultiplied(frame.size, &frame.rgba))
    };
    let mut out = vec![settled(&view)?];
    for (dx, dy) in drags {
        view.hold(true);
        view.turn(*dx, *dy);
        out.push(settled(&view)?);
    }
    Ok(out)
}

/// Round 18's states that could not be photographed in the game (someone was at the computer), over a real frame of it (`ORBIT_QM_FRAME`), with real 3D
/// frames of real captures (`ORBIT_SESSION` on Pictures, `ORBIT_SESSION_WORLD` on a world's side panel): All tools, a capture in live 3D on Pictures in its
/// frame, dragged, and on the whole sheet, and a world's side panel with its capture live and on the whole page. Painted, and said so where they are shown.
fn round_18_over_the_game(out: &Path, idle: &Facts) -> anyhow::Result<()> {
    use panels::{OpenedView, RecentKind, Sheet, WorldVisit};
    let Ok(frame) = std::env::var("ORBIT_QM_FRAME") else { return Ok(()) };
    let ground = image::open(&frame)?.to_rgba8();
    let client = vec2(1920.0, 1080.0);
    // All tools: what plays with where it comes from on its own line, and the last places, a long name on two lines.
    let visit = |name: &str, line: &str, here: bool| WorldVisit { name: name.into(), line: line.into(), here, ..Default::default() };
    let all = Facts {
        sheet: Some(Sheet::All),
        sheet_title: "All tools".into(),
        beside_wing: true,
        music: Some(("Rain on a forest cabin, 8 hours".into(), true)),
        music_by: "Quiet Hours".into(),
        can_copy_playing: true,
        can_drop_playing: true,
        music_times: Some((754, 28_812)),
        volume: Some((62, false)),
        worlds_visited: vec![visit("VRChat Home", "Invite \u{b7} now", true), visit("everyone is asleep except you", "Fri 23:03 \u{b7} Friends", false), visit("\u{3086}\u{3063}\u{305f}\u{308a}House", "Fri 22:41 \u{b7} Invite", false)],
        ..idle.clone()
    };
    over_the_game("orbit-r18-all-tools", out, client, &ground, &all, |px| {
        // Its large tiles hold real pictures, as in the app: three captures on Worlds in 3D, the newest on Pictures, each place's own.
        let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / 1024.0).in_orbit_look();
        let textures: Vec<_> = (0..6).map(|i| canvas.ctx().load_texture("real", stand_in(i), TextureOptions::LINEAR)).collect();
        let mut facts = all.clone();
        facts.hero_captures = textures[..3].iter().map(|t| t.id()).collect();
        facts.hero_capture_names = stand_in_names().into_iter().take(3).collect();
        facts.hero_picture = Some(textures[1].id());
        for (visit, n) in facts.worlds_visited.iter_mut().zip([5, 3, 4]) {
            visit.picture = Some(textures[n].id());
        }
        for _ in 0..2 {
            canvas.paint(0.0, |p| panels::paint_sheet(p, Sheet::All, &facts, None, None));
        }
        let from = canvas.pixels().to_vec();
        let mut pixels = from.clone();
        panels::finish_picture(&mut pixels, &from, px, px[0] as f32 / 1024.0, [0, 0, px[0], px[1]]);
        Ok(pixels)
    })?;
    // A capture in live 3D on Pictures: seen from inside, its full-screen button in its frame's corner; dragged; and on the whole sheet.
    if let Ok(dir) = std::env::var("ORBIT_SESSION") {
        let dir = PathBuf::from(dir);
        let frames = live_frames(&dir, &[(-150.0, 20.0)])?;
        let title = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // "Palm Point - Night 2026-10-21 15-48-59": its name, without when.
        let name = title.rsplitn(3, ' ').nth(2).unwrap_or(&title).to_string();
        let capture = Recent { dir: dir.clone(), title: name, state: "Wed 15:48 \u{b7} 3D picture".into(), kind: RecentKind::Capture, count: 1, ..Default::default() };
        let others = [("Sky Dream", "Sun 08:21 \u{b7} 3D picture"), ("After The Rain", "Fri 23:03 \u{b7} 3D picture")];
        let recent: panels::Gallery = std::iter::once(capture).chain(others.iter().map(|(title, state)| Recent { dir: PathBuf::from(title), title: title.to_string(), state: state.to_string(), ..Default::default() })).collect();
        for (shot, frame, full) in [("orbit-r18-capture-3d", &frames[0], false), ("orbit-r18-capture-3d-dragged", &frames[1], false), ("orbit-r18-capture-3d-full", &frames[1], true)] {
            let facts = Facts { sheet: Some(Sheet::Pictures), sheet_title: "Pictures".into(), beside_wing: true, recent: recent.clone(), opened: Some(0), view_full: full, ..idle.clone() };
            over_the_game(shot, out, client, &ground, &facts, |px| {
                let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / 1024.0).in_orbit_look();
                let texture = canvas.ctx().load_texture("live", frame.clone(), TextureOptions::LINEAR);
                let facts = Facts { opened_view: Some(OpenedView { picture: Some(texture.id()), plate: Vec::new(), failed: None }), ..facts.clone() };
                let items = panels::sheet_items(Sheet::Pictures, &facts);
                // Full: the button that brings it back, pointed.
                let hover = full.then(|| items.iter().position(|i| i.action == panels::Action::FullView)).flatten();
                for _ in 0..2 {
                    canvas.paint(0.0, |p| panels::paint_sheet(p, Sheet::Pictures, &facts, hover, None));
                }
                let from = canvas.pixels().to_vec();
                let mut pixels = from.clone();
                panels::finish_picture(&mut pixels, &from, px, px[0] as f32 / 1024.0, [0, 0, px[0], px[1]]);
                Ok(pixels)
            })?;
        }
    }
    // A world's side panel on Worlds in 3D, the list in two columns beside it; then its capture live in its picture; and on the whole page.
    let worlds = Facts { sheet: Some(Sheet::Worlds), sheet_title: "Worlds in 3D".into(), beside_wing: true, ..idle.clone() };
    over_the_game("orbit-r18-world-side", out, client, &ground, &worlds, |px| crate::overlay::worlds::panel::side_live(px, None))?;
    if let Ok(dir) = std::env::var("ORBIT_SESSION_WORLD") {
        let frames = live_frames(Path::new(&dir), &[])?;
        for (shot, full) in [("orbit-r18-world-side-live", false), ("orbit-r18-world-side-live-full", true)] {
            over_the_game(shot, out, client, &ground, &worlds, |px| crate::overlay::worlds::panel::side_live(px, Some((&frames[0], full))))?;
        }
    }
    Ok(())
}

fn sheet_picture(sheet: panels::Sheet, facts: &Facts, px: [u32; 2]) -> Vec<u8> {
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / 1024.0).in_orbit_look();
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    facts.recent_pictures = textures.iter().enumerate().map(|(n, texture)| (n, texture.id())).collect();
    for (n, video) in facts.videos.iter_mut().enumerate() {
        video.picture = Some(textures[n % textures.len()].id());
    }
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint_sheet(p, sheet, &facts, None, None));
    }
    let from = canvas.pixels().to_vec();
    let mut out = from.clone();
    panels::finish_picture(&mut out, &from, px, px[0] as f32 / 1024.0, [0, 0, px[0], px[1]]);
    panels::cut_page_corners(&mut out, px, [0, 0, px[0], px[1]]);
    out
}

/// The sheet as the headset is given it: its back and its page in one picture ([`vr::compose_sheet`]), and where it goes.
fn vr_sheet(facts: &Facts, sheet: panels::Sheet, plane: &crate::overlay::orbit::vr::Plane) -> (Quad, Vec<u8>, [u32; 2]) {
    use crate::overlay::orbit::vr;
    let back = vr::clear_of_sheet(vr::sheet_place(Zone::SheetBack), true);
    let (back_pixels, back_px) = sharp_picture(&back, facts, None, 2.0);
    let page = vr::sheet_place(Zone::Takeover);
    let page_px = [(page.rect.width() * 2.0).round() as u32, (page.rect.height() * 2.0).round() as u32];
    let page_pixels = sheet_picture(sheet, facts, page_px);
    let top = ((page.rect.top() - vr::sheet_place(Zone::SheetBack).rect.top()) * 2.0).round() as u32;
    let whole = vr::compose_sheet(&back_pixels, back_px, &page_pixels, page_px, top);
    (plane.quad(&back, 0.0, 0.02), whole, back_px)
}

/// Translation's sheet: reading with translations shown, Orbiters waiting for its notice to be accepted, the text
/// reader downloading, and no AI app on the computer.
fn translate_scenes(out: &std::path::Path, size: egui::Vec2, idle: &Facts) -> anyhow::Result<()> {
    use crate::state::{TranslateState, TranslateStatus};
    use orbiters_core::settings::TranslateProvider;
    use panels::Sheet;
    let at = |provider: TranslateProvider, agreed: bool, state: TranslateState, shown: u32| Facts {
        sheet: Some(Sheet::Translate),
        sheet_title: "Translation".into(),
        translate: panels::TranslateFacts { enabled: true, provider, target: orbiters_i18n::language().tag().into(), keep_english: true, agreed, status: TranslateStatus { state, shown, lines: shown + 1, look_ms: Some(214), ..Default::default() } },
        ..idle.clone()
    };
    qm_scene("orbit-sheet-translate", out, size, &at(TranslateProvider::ThisComputer, false, TranslateState::Looking, 3), None, Some(Sheet::Translate))?;
    qm_scene("orbit-translate-orbiters", out, size, &at(TranslateProvider::Orbiters, false, TranslateState::NeedsAgreement, 0), None, Some(Sheet::Translate))?;
    qm_scene("orbit-translate-downloading", out, size, &at(TranslateProvider::ThisComputer, false, TranslateState::Downloading { done: 12_000_000, total: 39_000_000 }, 0), None, Some(Sheet::Translate))?;
    qm_scene("orbit-translate-no-app", out, size, &at(TranslateProvider::ThisComputer, false, TranslateState::NoLocalTranslator, 0), None, Some(Sheet::Translate))?;
    Ok(())
}

/// A snapshot the FPS booster's sheet can be drawn from: VR running, the overlay watching, at Background unless changed.
fn booster_snapshot(change: impl FnOnce(&mut crate::state::Snapshot)) -> crate::state::Snapshot {
    use orbiters_booster::{engine, Level};
    let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
    snap.steamvr.running = true;
    snap.settings.booster.level = Level::Background;
    snap.booster.watching = true;
    snap.booster.overlay.streamer = true;
    let report = &mut snap.booster.overlay.report;
    report.line = engine::Line::Limit;
    report.limit = orbiters_booster::classify::Limit::Processor;
    report.now = Some(engine::Now { real_fps: 36.0, refresh_hz: 72.0, filled_share: 0.49, app_cpu_ms: 18.0, app_gpu_ms: 9.5, ..Default::default() });
    report.guide.items = orbiters_booster::guided::list_for(report.limit);
    report.guide.likely = true;
    change(&mut snap);
    snap
}

fn sample_comparison(verdict: orbiters_booster::trial::Verdict) -> orbiters_booster::trial::Comparison {
    orbiters_booster::trial::Comparison {
        kind: orbiters_booster::trial::Kind::Programs,
        verdict,
        before_fps: 36.0,
        after_fps: if verdict == orbiters_booster::trial::Verdict::Kept { 41.0 } else { 36.0 },
        before_filled: 0.49,
        after_filled: if verdict == orbiters_booster::trial::Verdict::Kept { 0.38 } else { 0.49 },
        before_over_2x: 0.0,
        after_over_2x: 0.0,
        before_late_cpu: 0.4,
        after_late_cpu: 0.3,
        before_late_gpu: 0.0,
        after_late_gpu: 0.0,
        stalls_at_switch: 0,
        kept: true,
    }
}

fn booster_scenes(out: &std::path::Path, size: egui::Vec2, idle: &Facts) -> anyhow::Result<()> {
    use orbiters_booster::engine::Line;
    use orbiters_booster::trial::Verdict;
    use panels::Sheet;
    use orbiters_booster::Level;
    let at = |snap: &crate::state::Snapshot, details: bool, asking: bool| Facts {
        sheet: Some(Sheet::Booster),
        sheet_title: "FPS booster".into(),
        booster: panels::BoosterFacts::of(snap, details, asking, None),
        ..idle.clone()
    };
    let quiet = booster_snapshot(|_| {});
    qm_scene("orbit-sheet-booster", out, size, &at(&quiet, false, false), None, Some(Sheet::Booster))?;
    let kept = booster_snapshot(|s| {
        s.booster.overlay.report.line = Line::Kept;
        s.booster.overlay.report.comparisons = vec![sample_comparison(Verdict::Kept)];
    });
    qm_scene("orbit-booster-kept", out, size, &at(&kept, false, false), None, Some(Sheet::Booster))?;
    let none = booster_snapshot(|s| {
        s.booster.overlay.report.line = Line::NoGain;
        s.booster.overlay.report.comparisons = vec![sample_comparison(Verdict::NoGain)];
    });
    qm_scene("orbit-booster-no-gain", out, size, &at(&none, false, false), None, Some(Sheet::Booster))?;
    let processor = booster_snapshot(|s| {
        s.settings.booster.level = Level::Picture;
        s.booster.overlay.report.line = Line::ProcessorStays;
    });
    qm_scene("orbit-booster-processor", out, size, &at(&processor, false, false), None, Some(Sheet::Booster))?;
    let avatars = booster_snapshot(|s| {
        s.settings.booster.level = Level::Avatars;
        s.booster.max_shown = Some(orbiters_booster::guided::Known::Count(30));
    });
    qm_scene("orbit-booster-avatars", out, size, &at(&avatars, false, false), None, Some(Sheet::Booster))?;
    qm_scene("orbit-booster-asking", out, size, &at(&quiet, false, true), None, Some(Sheet::Booster))?;
    let details = booster_snapshot(|s| {
        s.booster.lowered = vec![("chrome".into(), 24), ("cargo".into(), 1)];
        s.booster.overlay.report.wants.stop_preparing = true;
        s.booster.test_tools = true;
    });
    qm_scene("orbit-booster-details", out, size, &at(&details, true, false), None, Some(Sheet::Booster))?;
    // VR feedback F4: Off, measured and nothing changed; a test asked for at Off, with its Stop; measured, with no gain; step 3's advice.
    let off = booster_snapshot(|s| {
        s.settings.booster.level = Level::Off;
        s.booster.overlay.report.now = Some(orbiters_booster::engine::Now { real_fps: 35.9, refresh_hz: 72.0, half_rate_pacing: true, filled_share: 0.48, ..Default::default() });
    });
    qm_scene("orbit-booster-off", out, size, &at(&off, false, false), None, Some(Sheet::Booster))?;
    let testing = booster_snapshot(|s| {
        s.settings.booster.level = Level::Off;
        s.booster.lowered = vec![("chrome".into(), 22)];
        s.booster.overlay.report.now = Some(orbiters_booster::engine::Now { real_fps: 35.9, refresh_hz: 72.0, half_rate_pacing: true, filled_share: 0.48, ..Default::default() });
        s.booster.overlay.report.testing = Some(orbiters_booster::engine::Testing { kind: orbiters_booster::trial::Kind::Programs, seconds_left: 12.0, side: orbiters_booster::trial::Side::With, manual: true });
    });
    qm_scene("orbit-booster-testing", out, size, &at(&testing, false, false), None, Some(Sheet::Booster))?;
    let measured_none = booster_snapshot(|s| {
        s.booster.overlay.report.line = Line::NoGain;
        s.booster.overlay.report.now = Some(orbiters_booster::engine::Now { real_fps: 35.9, refresh_hz: 72.0, half_rate_pacing: true, filled_share: 0.48, ..Default::default() });
        s.booster.overlay.report.comparisons = vec![sample_comparison(Verdict::NoGain)];
        s.booster.overlay.report.wants.stop_preparing = true;
    });
    qm_scene("orbit-booster-measured-no-gain", out, size, &at(&measured_none, false, false), None, Some(Sheet::Booster))?;
    Ok(())
}

/// Behaviour regression pictures; event/ack ordering is verified separately in IPC and flow tests.
pub fn behaviour_shots(out: &Path) -> anyhow::Result<()> {
    use panels::{Sheet, Smoothed};
    let base = Facts { sheet: Some(Sheet::Tracking), sheet_title: orbiters_i18n::tr("Smooth tracking").into(), tracking: Some((true, [0, 35, 78], String::new())), ..Facts::default() };
    for (name, values, held) in [
        ("held", [0, 35, 78], Some((Smoothed::Device(1), 90))),
        ("released", [0, 90, 78], None),
        ("stale-driver", [0, 90, 78], None),
        ("two-sliders", [60, 90, 78], None),
        ("clamped", [60, 80, 78], None),
    ] {
        let mut facts = base.clone();
        facts.tracking = Some((true, values, String::new()));
        facts.smoothing_held = held;
        qm_scene(&format!("tracking-{name}"), out, vec2(1920.0, 1080.0), &facts, None, Some(Sheet::Tracking))?;
    }
    Ok(())
}
