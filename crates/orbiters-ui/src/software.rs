//! Paints egui frames on the CPU, for the VR overlay: no GPU device, so none of the ~100 MB a graphics driver
//! keeps per process. Triangles with per-vertex colours and textures, multiplied and blended premultiplied in gamma
//! space like egui's own renderers do on a gamma framebuffer. Paint callbacks (the glow shader) are skipped.

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId, Vertex};
use egui::{Color32, TextureFilter, TexturesDelta};
use std::collections::HashMap;

struct Texture {
    width: usize,
    height: usize,
    texels: Vec<Color32>,
    nearest: bool,
}

impl Texture {
    /// Premultiplied texel at normalised `uv` (bilinear unless the texture asks for nearest), as 0–255 floats.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let get = |x: i32, y: i32| {
            let c = self.texels[y.clamp(0, self.height as i32 - 1) as usize * self.width + x.clamp(0, self.width as i32 - 1) as usize];
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
        };
        if self.nearest {
            return get((x + 0.5).floor() as i32, (y + 0.5).floor() as i32);
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

/// A CPU framebuffer that egui frames are painted into.
pub struct Software {
    width: usize,
    height: usize,
    /// Premultiplied RGBA, top row first.
    pixels: Vec<[u8; 4]>,
    /// Painted under every frame instead of a plain colour (a backdrop that never changes is copied, not redrawn).
    backdrop: Option<Vec<[u8; 4]>>,
    textures: HashMap<TextureId, Texture>,
}

impl Software {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as usize,
            height: height as usize,
            pixels: vec![[0; 4]; width as usize * height as usize],
            backdrop: None,
            textures: HashMap::new(),
        }
    }

    /// Keeps the last painted frame as the backdrop of the following ones.
    pub fn keep_as_backdrop(&mut self) {
        self.backdrop = Some(self.pixels.clone());
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    /// Applies new and changed textures (before painting); takes them out of `delta`.
    pub fn set_textures(&mut self, delta: &mut TexturesDelta) {
        for (id, deltas) in delta.set.drain() {
            for d in deltas {
                let ImageData::Color(image) = &d.image;
                let nearest = d.options.magnification == TextureFilter::Nearest;
                match d.pos {
                    None => {
                        self.textures.insert(id, Texture { width: image.size[0], height: image.size[1], texels: image.pixels.clone(), nearest });
                    }
                    Some([px, py]) => {
                        let Some(t) = self.textures.get_mut(&id) else { continue };
                        for row in 0..image.size[1] {
                            let (dst_y, src) = (py + row, &image.pixels[row * image.size[0]..(row + 1) * image.size[0]]);
                            if dst_y >= t.height {
                                break;
                            }
                            let n = image.size[0].min(t.width.saturating_sub(px));
                            t.texels[dst_y * t.width + px..dst_y * t.width + px + n].copy_from_slice(&src[..n]);
                        }
                    }
                }
            }
        }
    }

    /// Frees textures egui no longer uses (after painting); takes them out of `delta`.
    pub fn free_textures(&mut self, delta: &mut TexturesDelta) {
        for id in delta.free.drain() {
            self.textures.remove(&id);
        }
    }

    /// Paints the frame over the backdrop (or `clear`) and returns RGBA bytes (premultiplied).
    pub fn paint(&mut self, primitives: &[ClippedPrimitive], pixels_per_point: f32, clear: Color32) -> &[u8] {
        match &self.backdrop {
            Some(b) => self.pixels.copy_from_slice(b),
            None => self.pixels.fill([clear.r(), clear.g(), clear.b(), clear.a()]),
        }
        for p in primitives {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let clip = [
                ((p.clip_rect.min.x * pixels_per_point).round().max(0.0) as usize).min(self.width),
                ((p.clip_rect.min.y * pixels_per_point).round().max(0.0) as usize).min(self.height),
                ((p.clip_rect.max.x * pixels_per_point).round().max(0.0) as usize).min(self.width),
                ((p.clip_rect.max.y * pixels_per_point).round().max(0.0) as usize).min(self.height),
            ];
            if clip[0] >= clip[2] || clip[1] >= clip[3] {
                continue;
            }
            let texture = self.textures.remove(&mesh.texture_id);
            for t in mesh.indices.as_chunks::<3>().0 {
                let (a, b, c) = (&mesh.vertices[t[0] as usize], &mesh.vertices[t[1] as usize], &mesh.vertices[t[2] as usize]);
                self.triangle(clip, [a, b, c], pixels_per_point, texture.as_ref());
            }
            if let Some(texture) = texture {
                self.textures.insert(mesh.texture_id, texture);
            }
        }
        // SAFETY: [u8; 4] has no padding; the slice covers exactly the pixel buffer.
        unsafe { std::slice::from_raw_parts(self.pixels.as_ptr().cast::<u8>(), self.pixels.len() * 4) }
    }

    fn triangle(&mut self, clip: [usize; 4], v: [&Vertex; 3], ppp: f32, texture: Option<&Texture>) {
        let mut p = v.map(|v| [v.pos.x * ppp, v.pos.y * ppp]);
        let mut v = v;
        let edge = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let mut area = edge(p[0], p[1], p[2]);
        if area.abs() < 1e-6 {
            return;
        }
        if area < 0.0 {
            p.swap(1, 2);
            v.swap(1, 2);
            area = -area;
        }
        let min_x = (p[0][0].min(p[1][0]).min(p[2][0]).floor().max(clip[0] as f32)) as usize;
        let min_y = (p[0][1].min(p[1][1]).min(p[2][1]).floor().max(clip[1] as f32)) as usize;
        let max_x = (p[0][0].max(p[1][0]).max(p[2][0]).ceil().min(clip[2] as f32)) as usize;
        let max_y = (p[0][1].max(p[1][1]).max(p[2][1]).ceil().min(clip[3] as f32)) as usize;
        if min_x >= max_x || min_y >= max_y {
            return;
        }
        // Edge functions w_i(x, y) = A_i x + B_i y + C_i, positive inside; w_0 is opposite vertex 0.
        let coeffs = |a: [f32; 2], b: [f32; 2]| (a[1] - b[1], b[0] - a[0], a[0] * b[1] - a[1] * b[0]);
        let e = [coeffs(p[1], p[2]), coeffs(p[2], p[0]), coeffs(p[0], p[1])];
        // Pixels exactly on a shared edge belong to one triangle only (top-left rule), so nothing blends twice.
        let top_left = |a: [f32; 2], b: [f32; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            dy < 0.0 || (dy == 0.0 && dx > 0.0)
        };
        let tl = [top_left(p[1], p[2]), top_left(p[2], p[0]), top_left(p[0], p[1])];
        let color = v.map(|v| [v.color.r() as f32, v.color.g() as f32, v.color.b() as f32, v.color.a() as f32]);
        let uv = v.map(|v| [v.uv.x, v.uv.y]);
        let flat_color = color[0] == color[1] && color[1] == color[2];
        let flat_uv = uv[0] == uv[1] && uv[1] == uv[2];
        // A plain shape (one colour, one texel: egui's white) needs no interpolation per pixel.
        let flat = match (flat_color, flat_uv, texture) {
            (true, true, Some(t)) => Some(modulate(color[0], t.sample(uv[0][0], uv[0][1]))),
            (true, _, None) => Some(color[0]),
            _ => None,
        };
        let inv_area = 1.0 / area;
        // Change of colour and uv per pixel to the right (the barycentric weights change by A_i / area).
        let dl = [e[0].0 * inv_area, e[1].0 * inv_area, e[2].0 * inv_area];
        let dc: [f32; 4] = std::array::from_fn(|i| color[0][i] * dl[0] + color[1][i] * dl[1] + color[2][i] * dl[2]);
        let duv = [0, 1].map(|j| uv[0][j] * dl[0] + uv[1][j] * dl[1] + uv[2][j] * dl[2]);
        let inside = |w: [f32; 3]| (0..3).all(|i| w[i] > 0.0 || (w[i] == 0.0 && tl[i]));
        for y in min_y..max_y {
            let py = y as f32 + 0.5;
            let k = e.map(|(_, b, c)| b * py + c);
            let w_at = |x: usize| {
                let px = x as f32 + 0.5;
                [e[0].0 * px + k[0], e[1].0 * px + k[1], e[2].0 * px + k[2]]
            };
            // Each edge bounds the row on one side; a pixel of slack absorbs rounding, the exact test decides.
            let (mut lo, mut hi) = (min_x as f32, max_x as f32);
            for i in 0..3 {
                let a = e[i].0;
                if a > 0.0 {
                    lo = lo.max(-k[i] / a - 1.5);
                } else if a < 0.0 {
                    hi = hi.min(-k[i] / a + 1.5);
                } else if k[i] < 0.0 || (k[i] == 0.0 && !tl[i]) {
                    hi = lo;
                }
            }
            if lo >= hi {
                continue;
            }
            let (x0, x1) = ((lo.floor().max(min_x as f32)) as usize, (hi.ceil() as usize).min(max_x));
            // A triangle covers one run of a row: find its ends.
            let Some(first) = (x0..x1).find(|&x| inside(w_at(x))) else { continue };
            let last = (first..x1).rev().find(|&x| inside(w_at(x))).unwrap_or(first);
            let row = &mut self.pixels[y * self.width + first..=y * self.width + last];
            match flat {
                Some(c) => {
                    let src = to_u32(c);
                    if src[3] >= 255 {
                        row.fill([src[0] as u8, src[1] as u8, src[2] as u8, 255]);
                    } else if src[3] > 0 || src[..3].iter().any(|v| *v > 0) {
                        let keep = 255 - src[3];
                        for px in row {
                            blend(px, src, keep);
                        }
                    }
                }
                None => {
                    // Colour and texture coordinates are linear across the triangle: step them along the run.
                    let l = w_at(first).map(|w| w * inv_area);
                    let mut c: [f32; 4] = std::array::from_fn(|i| color[0][i] * l[0] + color[1][i] * l[1] + color[2][i] * l[2]);
                    let mut t_uv = [0, 1].map(|j| uv[0][j] * l[0] + uv[1][j] * l[1] + uv[2][j] * l[2]);
                    match texture {
                        Some(t) if !flat_uv => {
                            for px in row {
                                let src = to_u32(modulate(c, t.sample(t_uv[0], t_uv[1])));
                                blend(px, src, 255 - src[3]);
                                for i in 0..4 {
                                    c[i] += dc[i];
                                }
                                t_uv[0] += duv[0];
                                t_uv[1] += duv[1];
                            }
                        }
                        _ => {
                            // One texel (egui's white for shapes): it only scales the colour.
                            let texel = texture.map(|t| t.sample(uv[0][0], uv[0][1])).unwrap_or([255.0; 4]);
                            let scale: [f32; 4] = std::array::from_fn(|i| texel[i] * (1.0 / 255.0));
                            for px in row {
                                let src = to_u32(std::array::from_fn(|i| c[i] * scale[i]));
                                blend(px, src, 255 - src[3]);
                                for i in 0..4 {
                                    c[i] += dc[i];
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Vertex colour × texel, both premultiplied 0–255.
fn modulate(c: [f32; 4], t: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| c[i] * t[i] * (1.0 / 255.0))
}

/// Float casts saturate at 0: only the top needs clamping.
#[inline]
fn to_u32(c: [f32; 4]) -> [u32; 4] {
    c.map(|v| ((v + 0.5) as u32).min(255))
}

/// x / 255 rounded, for x up to 255 × 255.
#[inline]
fn div255(x: u32) -> u32 {
    (x + 128 + ((x + 128) >> 8)) >> 8
}

/// Premultiplied "over": dst = src + dst × (255 − src alpha) / 255.
#[inline]
fn blend(dst: &mut [u8; 4], src: [u32; 4], keep: u32) {
    for i in 0..4 {
        dst[i] = (src[i] + div255(dst[i] as u32 * keep)).min(255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};

    fn run(ctx: &egui::Context, sw: &mut Software, ui: impl FnMut(&mut egui::Ui)) -> Vec<u8> {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 32.0))), ..Default::default() };
        let mut full = ctx.run_ui(input, ui);
        sw.set_textures(&mut full.textures_delta);
        let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
        let out = sw.paint(&prims, full.pixels_per_point, Color32::BLACK).to_vec();
        sw.free_textures(&mut full.textures_delta);
        out
    }

    #[test]
    fn paints_shapes_and_text() {
        let ctx = egui::Context::default();
        let mut sw = Software::new(64, 32);
        let px = |img: &[u8], x: usize, y: usize| [img[(y * 64 + x) * 4], img[(y * 64 + x) * 4 + 1], img[(y * 64 + x) * 4 + 2], img[(y * 64 + x) * 4 + 3]];
        // An opaque red rectangle over the left half, a half-transparent white one over the right top.
        let img = run(&ctx, &mut sw, |ui| {
            ui.painter().rect_filled(Rect::from_min_max(pos2(0.0, 0.0), pos2(32.0, 32.0)), 0.0, Color32::RED);
            ui.painter().rect_filled(Rect::from_min_max(pos2(32.0, 0.0), pos2(64.0, 16.0)), 0.0, Color32::from_white_alpha(128));
        });
        assert_eq!(px(&img, 10, 10), [255, 0, 0, 255]);
        assert_eq!(px(&img, 31, 31), [255, 0, 0, 255], "rect edge pixel fully covered once");
        assert_eq!(px(&img, 40, 4), [128, 128, 128, 255]);
        assert_eq!(px(&img, 40, 24), [0, 0, 0, 255]);
        // Text uses the font texture: some pixels lit, not all.
        let img = run(&ctx, &mut sw, |ui| {
            ui.painter().text(pos2(2.0, 2.0), egui::Align2::LEFT_TOP, "Hi", egui::FontId::proportional(20.0), Color32::WHITE);
        });
        let lit = img.as_chunks::<4>().0.iter().filter(|p| p[0] > 128).count();
        assert!(lit > 20 && lit < 64 * 32 / 2, "{lit} lit pixels");
    }
}
