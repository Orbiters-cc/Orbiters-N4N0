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
fn recent() -> Vec<Recent> {
    [("Sky Dream", "3D picture, ready"), ("Rainy Skyline", "Reconstructing 42%"), ("Pacific Dreams (Night)", "Recording, waiting"), ("After The Rain", "3D picture, ready")]
        .iter()
        .map(|(title, state)| Recent { dir: PathBuf::from(title), title: title.to_string(), state: state.to_string(), progress: state.contains('%').then_some(0.42), picture: None })
        .collect()
}

fn stand_in(seed: u8) -> ColorImage {
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
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / logical.x);
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    for (recent, texture) in facts.recent.iter_mut().zip(&textures) {
        recent.picture = Some(texture.id());
    }
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint(p, placed.zone, logical, &facts, hover, pressed));
    }
    let mut out = canvas.pixels().to_vec();
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
        Around::QuickMenu => {
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
        // The middle of the frame, past the desktop's HUD in its corners, covering the view without stretching.
        let (gw, gh) = (ground.width(), ground.height());
        let (cw, ch) = (gw * 2 / 3, gw * 2 / 3 * size[1] / size[0]);
        let part = image::imageops::crop_imm(ground, (gw - cw) / 2, (gh.saturating_sub(ch)) / 2 + gh / 12, cw, ch.min(gh)).to_image();
        let scaled = image::imageops::resize(&part, size[0], size[1], image::imageops::FilterType::Triangle);
        let mut pixels = vec![0u8; size[0] as usize * size[1] as usize * 4];
        for (i, p) in scaled.pixels().enumerate() {
            pixels[i * 4..i * 4 + 4].copy_from_slice(&[p[0], p[1], p[2], 255]);
        }
        Self { size, pixels, eyes, focal: size[0] as f32 / 2.0 / 50f32.to_radians().tan() }
    }

    fn ray(&self, x: f32, y: f32) -> [f32; 3] {
        let d = [(x - self.size[0] as f32 / 2.0) / self.focal, -(y - self.size[1] as f32 / 2.0) / self.focal, -1.0];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / l, d[1] / l, d[2] / l]
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
                let placed = zones::in_units(zone);
                let hovered = hover == Some(zone);
                let (pixels, px) = sharp_picture(&placed, facts, hovered.then_some(0), 2.0);
                let quad = plane.quad(&placed, 0.0, 0.004);
                eye.quad(&quad, &pixels, px, 1.0);
                if hovered {
                    let at = zone.ring().map_or(placed.rect.center().to_vec2(), |r| r.anchor());
                    end = Some(space::apply(&plane.frame, [at.x * unit_m, -at.y * unit_m, 0.01]));
                }
            }
            if let Some(end) = end {
                eye.laser([-0.30, 1.12, -0.25], end);
            }
            eye.save(out, name)
        };
        let pointing = |a| Facts { pointed: Some(a), ..facts.clone() };
        let bloom = zones::Group::Pictures;
        wheel_scene(&facts, None, None, &format!("vr-wheel-{world}"))?;
        wheel_scene(&pointing(Action::Worlds), Some(Zone::WheelWorlds), None, &format!("vr-wheel-{world}-point-worlds"))?;
        wheel_scene(&pointing(Action::Pictures), Some(Zone::WheelPictures), Some(bloom), &format!("vr-wheel-{world}-point-pictures"))?;
        wheel_scene(&pointing(Action::Picture(PhotoMode::World)), Some(Zone::BloomWorld), Some(bloom), &format!("vr-wheel-{world}-point-picture"))?;
        wheel_scene(&pointing(Action::PlayPause), Some(Zone::BloomNext), Some(zones::Group::Playing), &format!("vr-wheel-{world}-point-playing"))?;
        let counting = Facts { countdown: Some(2), picture_mode: Some(PhotoMode::People), pointed: Some(Action::Picture(PhotoMode::People)), ..facts.clone() };
        wheel_scene(&counting, None, Some(bloom), &format!("vr-wheel-{world}-countdown"))?;
        // The Quick Menu where it opens before the eyes, the wing beside it.
        let held = space::before_eyes(&head, 0.5, 0.08);
        let plane = vr::quick_menu_plane(&vr::grip_holding(&held, &tune.quick_menu(1)), 1, &tune, 1.0);
        let qm_scene = |facts: &Facts, hover: Option<usize>, sheet: Option<Sheet>, name: &str| -> anyhow::Result<()> {
            let mut eye = Eye::new(size, &ground, eyes);
            let center_units = zones::Placed { zone: Zone::SheetBack, rect: qm_units, center: pos2(0.0, 0.0), unit: 1.0 };
            eye.quad(&plane.quad(&center_units, 0.0, 0.0), &qm, qm_px, 1.0);
            let mut end = None;
            for zone in Zone::QUICK_MENU {
                let placed = zones::in_units(zone);
                let hovered = hover.filter(|_| zone == Zone::WingWorlds).map(|_| 0);
                let (pixels, px) = sharp_picture(&placed, facts, hovered, 2.0);
                eye.quad(&plane.quad(&placed, 0.0, 0.004), &pixels, px, 1.0);
                if let Some(i) = hovered {
                    let item = panels::items(zone, placed.logical(), facts)[i].rect.center().to_vec2() + placed.rect.min.to_vec2();
                    end = Some(space::apply(&plane.frame, [item.x * plane.unit, -item.y * plane.unit, 0.01]));
                }
            }
            if let Some(sheet) = sheet {
                let back = zones::in_units(Zone::SheetBack);
                let (pixels, px) = sharp_picture(&back, facts, None, 2.0);
                eye.quad(&plane.quad(&back, 0.0, 0.016), &pixels, px, 1.0);
                let page = zones::in_units(Zone::Takeover);
                let px = [1408u32, 792u32];
                let pixels = sheet_picture(sheet, facts, px);
                eye.quad(&plane.quad(&page, 0.0, 0.02), &pixels, px, 1.0);
            }
            if let Some(end) = end {
                eye.laser([0.26, 1.05, -0.25], end);
            }
            eye.save(out, name)
        };
        qm_scene(&facts, None, None, &format!("vr-qm-{world}"))?;
        qm_scene(&pointing(Action::Worlds), Some(0), None, &format!("vr-qm-{world}-point-worlds"))?;
        qm_scene(&Facts { sheet: Some(Sheet::All), sheet_title: "All tools".into(), ..facts.clone() }, None, Some(Sheet::All), &format!("vr-sheet-{world}-all"))?;
        qm_scene(&Facts { sheet: Some(Sheet::Pictures), sheet_title: "3D pictures".into(), ..facts.clone() }, None, Some(Sheet::Pictures), &format!("vr-sheet-{world}-pictures"))?;
        let _ = Target::Close;
    }
    Ok(())
}

/// A panel's picture for the headset: `sharp` pixels a unit, cut to its shape, pictures of recent captures standing in.
fn sharp_picture(placed: &Placed, facts: &Facts, hover: Option<usize>, sharp: f32) -> (Vec<u8>, [u32; 2]) {
    let px = [(placed.rect.width() * sharp).round() as u32, (placed.rect.height() * sharp).round() as u32];
    let logical = placed.logical();
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / logical.x);
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    for (recent, texture) in facts.recent.iter_mut().zip(&textures) {
        recent.picture = Some(texture.id());
    }
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
    let idle = Facts { recent: recent(), timer: 3, n4n0: true, desktop: true, music: Some(("The Weeknd - Moth to a Flame".into(), true)), ..Default::default() };
    let hint = Some(("Hold Left Alt to use this ring", "Your mouse then points here instead of at VRChat's wheel."));
    let empty = Facts { recent: Vec::new(), music: None, ..idle.clone() };
    let busy = Facts { recording: true, recording_seconds: Some(83), capture_busy: true, reconstructing: Some(0.42), waiting: 2, ..idle.clone() };
    let counting = Facts { countdown: Some(2), picture_mode: Some(PhotoMode::People), pointed: Some(Action::Picture(PhotoMode::People)), ..idle.clone() };
    let pointing = |a| Facts { pointed: Some(a), ..idle.clone() };
    let bloom = zones::Group::Pictures;
    let playing = zones::Group::Playing;
    wheel_scene("orbit-wheel-idle", out, hd, &Facts { hint, ..idle.clone() }, None, None, false)?;
    wheel_scene("orbit-wheel-empty", out, hd, &Facts { hint, ..empty.clone() }, None, None, false)?;
    wheel_scene("orbit-wheel-busy", out, hd, &busy, None, Some(bloom), true)?;
    wheel_scene("orbit-wheel-countdown", out, hd, &counting, None, Some(bloom), true)?;
    wheel_scene("orbit-wheel-point-worlds", out, hd, &pointing(Action::Worlds), Some((Zone::WheelWorlds, 0)), None, true)?;
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
    qm_scene("orbit-sheet-pictures", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "3D pictures".into(), ..idle.clone() }, None, Some(Sheet::Pictures))?;
    qm_scene("orbit-sheet-countdown", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "3D pictures".into(), ..counting.clone() }, None, Some(Sheet::Pictures))?;
    qm_scene("orbit-sheet-pictures-busy", out, hd, &Facts { sheet: Some(Sheet::Pictures), sheet_title: "3D pictures".into(), ..busy.clone() }, None, Some(Sheet::Pictures))?;
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
    for placed in zones::layout(Around::QuickMenu, client) {
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

fn sheet_picture(sheet: panels::Sheet, facts: &Facts, px: [u32; 2]) -> Vec<u8> {
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / 1024.0);
    let textures: Vec<_> = (0..4).map(|i| canvas.ctx().load_texture("recent", stand_in(i), TextureOptions::LINEAR)).collect();
    let mut facts = facts.clone();
    for (recent, texture) in facts.recent.iter_mut().zip(&textures) {
        recent.picture = Some(texture.id());
    }
    for _ in 0..2 {
        canvas.paint(0.0, |p| panels::paint_sheet(p, sheet, &facts, None, None));
    }
    canvas.pixels().to_vec()
}
