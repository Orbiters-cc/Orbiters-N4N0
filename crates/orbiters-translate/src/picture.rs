//! Pictures in memory, text drawn into them with the interface's own fonts and CPU painter, and the stand-in eyes the
//! tests look through: flat signs of known text placed in the room, seen by an eye at a known place
//! ([`render_eye`]), as SteamVR's compositor would hand them over.

use crate::geometry::{self, EyeView, M34, V3};
use crate::ocr::Rgba;
use std::path::Path;

/// An RGBA picture, top row first, straight (not premultiplied) alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> Self {
        Self { width, height, rgba: fill.repeat((width * height) as usize) }
    }
    pub fn view(&self) -> Rgba<'_> {
        Rgba { width: self.width, height: self.height, pixels: &self.rgba }
    }
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
    pub fn load_png(path: &Path) -> anyhow::Result<Self> {
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| anyhow::anyhow!("picture too large"))?]; // i18n: not shown (test helper)
        let info = reader.next_frame(&mut buf)?;
        anyhow::ensure!(info.bit_depth == png::BitDepth::Eight, "not 8-bit"); // i18n: not shown (test helper)
        let (w, h) = (info.width, info.height);
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..(w * h * 4) as usize].to_vec(),
            png::ColorType::Rgb => buf[..(w * h * 3) as usize].chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::Grayscale => buf[..(w * h) as usize].iter().flat_map(|&g| [g, g, g, 255]).collect(),
            other => anyhow::bail!("colour type {other:?}"), // i18n: not shown (test helper)
        };
        Ok(Self { width: w, height: h, rgba })
    }
    pub fn save_png(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&self.rgba)?;
        Ok(())
    }
    /// A part of it (x, y, width, height).
    pub fn crop(&self, area: [u32; 4]) -> Picture {
        let [x0, y0, w, h] = area;
        let mut out = Picture::new(w, h, [0; 4]);
        for y in 0..h {
            let s = (((y0 + y) * self.width + x0) * 4) as usize;
            out.rgba[(y * w * 4) as usize..((y + 1) * w * 4) as usize].copy_from_slice(&self.rgba[s..s + (w * 4) as usize]);
        }
        out
    }
    /// Scaled to `width` x `height` with bilinear samples.
    pub fn resized(&self, width: u32, height: u32) -> Picture {
        let v = self.view();
        let mut out = Picture::new(width, height, [0, 0, 0, 255]);
        let (sx, sy) = (self.width as f32 / width as f32, self.height as f32 / height as f32);
        for y in 0..height {
            for x in 0..width {
                let s = v.sample((x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
                let i = ((y * width + x) * 4) as usize;
                out.rgba[i..i + 3].copy_from_slice(&s.map(|c| c.round().clamp(0.0, 255.0) as u8));
            }
        }
        out
    }
}

/// A piece of text to draw: where its top-left corner is (pixels), how tall its letters are, its colour.
#[derive(Clone, Debug)]
pub struct Text {
    pub text: String,
    pub at: [f32; 2],
    pub size: f32,
    pub colour: [u8; 3],
}

/// Text drawn on a plain background with the interface's fonts (the system's own for other scripts: Yu Gothic, Malgun
/// Gothic, Microsoft YaHei, Segoe UI).
pub fn draw_text(width: u32, height: u32, background: [u8; 3], texts: &[Text]) -> Picture {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let mut painter = orbiters_ui::software::Software::new(width, height);
    let bg = egui::Color32::from_rgb(background[0], background[1], background[2]);
    let mut pixels = Vec::new();
    // Fonts for other scripts are opened when their letters first show up: a second frame draws with them.
    for _ in 0..2 {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32))), ..Default::default() };
        let mut full = ctx.run_ui(input, |ui| {
            let p = ui.painter();
            p.rect_filled(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, height as f32)), 0.0, bg);
            for t in texts {
                p.text(egui::pos2(t.at[0], t.at[1]), egui::Align2::LEFT_TOP, &t.text, egui::FontId::proportional(t.size), egui::Color32::from_rgb(t.colour[0], t.colour[1], t.colour[2]));
            }
        });
        painter.set_textures(&mut full.textures_delta);
        let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        painter.invalidate();
        pixels = painter.paint(&primitives, full.pixels_per_point, egui::Color32::TRANSPARENT).to_vec();
        painter.free_textures(&mut full.textures_delta);
    }
    Picture { width, height, rgba: pixels }
}

/// A flat picture standing in the room: its centre and axes (+X along its width, +Y up it, +Z out of its face towards
/// whoever reads it), and its width in metres (its height follows its picture's shape).
#[derive(Clone, Debug)]
pub struct Sign {
    pub picture: Picture,
    pub pose: M34,
    pub width: f32,
}

impl Sign {
    pub fn height(&self) -> f32 {
        self.width * self.picture.height as f32 / self.picture.width as f32
    }
    /// A point of its picture (pixels) in the room.
    pub fn at(&self, px: [f32; 2]) -> V3 {
        let (w, h) = (self.width, self.height());
        let u = px[0] / self.picture.width as f32 - 0.5;
        let v = 0.5 - px[1] / self.picture.height as f32;
        geometry::apply(&self.pose, [u * w, v * h, 0.0])
    }
}

/// What an eye sees of `signs` before `background` (a colour for each direction), `size` pixels, two by two samples a
/// pixel (as a game's antialiasing would smooth edges).
pub fn render_eye(view: &EyeView, size: [u32; 2], background: &dyn Fn(V3) -> [u8; 3], signs: &[Sign]) -> Picture {
    let mut out = Picture::new(size[0], size[1], [0, 0, 0, 255]);
    let planes: Vec<(M34, f32, f32)> = signs.iter().map(|s| (geometry::inverse(&s.pose), s.width, s.height())).collect();
    let o = geometry::origin(&view.eye);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let mut acc = [0f32; 3];
            for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (_, d) = view.ray([x as f32 + dx, y as f32 + dy]);
                let mut best: Option<(f32, [f32; 3])> = None;
                for (sign, (inv, w, h)) in signs.iter().zip(&planes) {
                    let (lo, ld) = (geometry::apply(inv, o), geometry::rotate(inv, d));
                    if ld[2].abs() < 1e-6 {
                        continue;
                    }
                    let t = -lo[2] / ld[2];
                    if t <= 0.0 || best.as_ref().is_some_and(|(bt, _)| *bt < t) {
                        continue;
                    }
                    let (u, v) = (lo[0] + ld[0] * t, lo[1] + ld[1] * t);
                    if u.abs() > w / 2.0 || v.abs() > h / 2.0 {
                        continue;
                    }
                    let px = [(u / w + 0.5) * sign.picture.width as f32, (0.5 - v / h) * sign.picture.height as f32];
                    best = Some((t, sign.picture.view().sample(px[0], px[1])));
                }
                let c = best.map(|(_, c)| c).unwrap_or_else(|| background(d).map(|c| c as f32));
                for k in 0..3 {
                    acc[k] += c[k];
                }
            }
            let i = ((y * size[0] + x) * 4) as usize;
            out.rgba[i..i + 3].copy_from_slice(&acc.map(|c| (c / 4.0).round().clamp(0.0, 255.0) as u8));
        }
    }
    out
}

/// A dim sky-and-floor background, a little textured, as most worlds have behind their signs.
pub fn plain_room(d: V3) -> [u8; 3] {
    let grain = ((d[0] * 91.0).sin() * (d[1] * 57.0).cos() * 6.0) as i32;
    let base = if d[1] > 0.0 { [38, 44, 70] } else { [52, 46, 40] };
    base.map(|c| (c + grain).clamp(0, 255) as u8)
}

/// A premultiplied RGBA picture laid over an eye's picture as SteamVR composites an overlay: a rectangle of `width` x
/// `height` metres at `pose` (+Z facing the viewer), sampled for each pixel it covers.
#[allow(clippy::too_many_arguments)]
pub fn composite(eye: &mut Picture, view: &EyeView, pose: &M34, width: f32, height: f32, rgba: &[u8], w: u32, h: u32) {
    let inv = geometry::inverse(pose);
    let o = geometry::origin(&view.eye);
    let lo = geometry::apply(&inv, o);
    let px = |x: f32, y: f32| -> [f32; 4] {
        let (x, y) = ((x - 0.5).clamp(0.0, w as f32 - 1.0), (y - 0.5).clamp(0.0, h as f32 - 1.0));
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (ax, ay) = (x - x0 as f32, y - y0 as f32);
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [rgba[i] as f32, rgba[i + 1] as f32, rgba[i + 2] as f32, rgba[i + 3] as f32]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|k| (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay)
    };
    for y in 0..eye.height {
        for x in 0..eye.width {
            let (_, d) = view.ray([x as f32 + 0.5, y as f32 + 0.5]);
            let ld = geometry::rotate(&inv, d);
            if ld[2].abs() < 1e-6 {
                continue;
            }
            let t = -lo[2] / ld[2];
            if t <= 0.0 {
                continue;
            }
            let (u, v) = (lo[0] + ld[0] * t, lo[1] + ld[1] * t);
            if u.abs() > width / 2.0 || v.abs() > height / 2.0 {
                continue;
            }
            let s = px((u / width + 0.5) * w as f32, (0.5 - v / height) * h as f32);
            let a = s[3] / 255.0;
            let i = ((y * eye.width + x) * 4) as usize;
            for k in 0..3 {
                eye.rgba[i + k] = (s[k] + eye.rgba[i + k] as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}
