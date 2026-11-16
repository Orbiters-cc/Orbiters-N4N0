//! Paints egui frames on the CPU, for the VR overlay: no rendering pipeline to set up (SteamVR gets the pictures as
//! textures of a small D3D11 device, or as raw pixels). Triangles with per-vertex colours and textures, multiplied and
//! blended premultiplied in gamma space like egui's own renderers do on a gamma framebuffer. Paint callbacks (the
//! glow shader) are skipped.
//!
//! Only what changed is painted again: each frame is compared with the last one tile by tile (the triangles over a
//! tile, in painting order), the changed tiles are painted again, and [`Software::damage`] says where, so that only
//! that part needs to be handed on. A pulsing dot repaints a few tiles, not the whole tab.

use egui::epaint::{ClippedPrimitive, ImageData, Primitive, TextureId, Vertex};
use egui::{Color32, ColorImage, TextureFilter, TextureWrapMode, TexturesDelta};
use std::collections::HashMap;

/// The side of the square tiles frames are compared in (pixels).
const TILE: usize = 32;
/// More separate changed areas than this are painted as the one rectangle around them.
const MAX_AREAS: usize = 8;

struct Texture {
    width: usize,
    height: usize,
    texels: Vec<Color32>,
    /// The nearest texel rather than four blended, where texels are larger than pixels (magnified) and smaller.
    nearest_magnified: bool,
    nearest_minified: bool,
    wrap: TextureWrapMode,
    /// What it was before the picture look, while one is on and this is a picture.
    plain: Option<(usize, usize, Vec<Color32>)>,
}

/// How pictures are drawn instead of as they are.
pub type PictureLook = fn(&ColorImage) -> ColorImage;

impl Texture {
    /// Whether it is a picture: a whole opaque image of some size. Text and marks have transparency and stay.
    fn is_picture(id: TextureId, image: &ColorImage) -> bool {
        id != TextureId::default() && image.size[0].min(image.size[1]) >= 32 && image.pixels.iter().all(|c| c.a() >= 250)
    }

    /// In `look`, from what it was.
    fn look(&mut self, look: PictureLook) {
        let (width, height, texels) = self.plain.take().unwrap_or_else(|| (self.width, self.height, std::mem::take(&mut self.texels)));
        let plain = ColorImage { size: [width, height], source_size: egui::vec2(width as f32, height as f32), pixels: texels };
        let shown = look(&plain);
        (self.width, self.height, self.texels) = (shown.size[0], shown.size[1], shown.pixels);
        self.plain = Some((width, height, plain.pixels));
    }

    /// As it was.
    fn unlook(&mut self) {
        if let Some((width, height, texels)) = self.plain.take() {
            (self.width, self.height, self.texels) = (width, height, texels);
        }
    }
}

impl Texture {
    /// Premultiplied texel at normalised `uv`, as 0–255 floats: bilinear or nearest as the texture asks for
    /// `minified` (texels smaller than pixels there) or magnified texels, beyond the edges as its wrap mode says.
    /// Nothing from a texture without texels.
    fn sample(&self, u: f32, v: f32, minified: bool) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [0.0; 4];
        }
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let get = |x: i32, y: i32| {
            let c = self.texels[wrap(y, self.height, self.wrap) * self.width + wrap(x, self.width, self.wrap)];
            [c.r() as f32, c.g() as f32, c.b() as f32, c.a() as f32]
        };
        if if minified { self.nearest_minified } else { self.nearest_magnified } {
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
    /// How pictures are drawn, where not as they are.
    picture_look: Option<PictureLook>,
    /// What covered each tile of the last frame: a hash of the triangles over it, in painting order (row by row).
    tiles: Vec<u64>,
    /// The same for the frame being painted (kept to reuse its memory).
    next_tiles: Vec<u64>,
    /// The next frame is painted whole: nothing was painted yet, a texture or the backdrop changed, or asked for.
    stale: bool,
    /// The colour the last frame was cleared to.
    clear: Option<Color32>,
    /// What the last frame changed (x0, y0, x1, y1).
    damage: Option<[u32; 4]>,
    parallel: bool,
}

impl Software {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as usize,
            height: height as usize,
            pixels: Vec::new(),
            backdrop: None,
            textures: HashMap::new(),
            picture_look: None,
            tiles: Vec::new(),
            next_tiles: Vec::new(),
            stale: true,
            clear: None,
            damage: None,
            parallel: false,
        }
    }

    /// Keeps the last painted frame as the backdrop of the following ones.
    pub fn keep_as_backdrop(&mut self) {
        self.backdrop = Some(self.pixels.clone());
        self.stale = true;
    }

    /// Independent rows preserve triangle order and the exact sampling arithmetic.
    pub fn set_parallel(&mut self, on: bool) { self.parallel = on; }

    pub fn size(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    /// Of `width` x `height` pixels from now on, its textures kept: the interface that draws here sent them once, and sends only what changes after (a new framebuffer
    /// with the same interface drew every frame without them: nothing at all where text and shapes go). The next frame is painted whole.
    pub fn resize(&mut self, width: u32, height: u32) {
        (self.width, self.height) = (width as usize, height as usize);
        self.pixels = Vec::new();
        self.backdrop = None;
        self.tiles.clear();
        self.next_tiles.clear();
        self.damage = None;
        self.stale = true;
    }

    /// The next frame is painted whole, and its damage is all of it (the picture handed on was lost, say).
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Lets go of every texture, for an interface made anew whose textures are other ones under the same numbers: the next frame brings its own, and is painted whole.
    pub fn forget_textures(&mut self) {
        self.textures.clear();
        self.stale = true;
    }

    /// The last frame painted: RGBA bytes, premultiplied, top row first.
    pub fn pixels(&self) -> &[u8] {
        self.pixels.as_flattened()
    }

    /// What the last [`Software::paint`] changed (x0, y0, x1, y1 in pixels), `None` when the frame is the one before
    /// again: nothing needs to be handed on, or only that part.
    pub fn damage(&self) -> Option<[u32; 4]> {
        self.damage
    }

    /// Applies new and changed textures (before painting); takes them out of `delta`.
    /// Draws pictures through `look` from now on, those it has as well; `None`, as they are.
    pub fn set_picture_look(&mut self, look: Option<PictureLook>) {
        if self.picture_look.is_some() == look.is_some() {
            return;
        }
        self.picture_look = look;
        self.stale = true;
        for (id, texture) in &mut self.textures {
            match look {
                None => texture.unlook(),
                Some(look) => {
                    let plain = ColorImage { size: [texture.width, texture.height], source_size: egui::vec2(texture.width as f32, texture.height as f32), pixels: std::mem::take(&mut texture.texels) };
                    let picture = Texture::is_picture(*id, &plain);
                    texture.texels = plain.pixels;
                    if picture {
                        texture.look(look);
                    }
                }
            }
        }
    }

    pub fn set_textures(&mut self, delta: &mut TexturesDelta) {
        if !delta.set.is_empty() {
            // Triangles over a changed texture look different with the same vertices.
            self.stale = true;
        }
        for (id, deltas) in delta.set.drain() {
            for d in deltas {
                let ImageData::Color(image) = &d.image;
                match d.pos {
                    None => {
                        let mut texture = Texture {
                            width: image.size[0],
                            height: image.size[1],
                            texels: image.pixels.clone(),
                            nearest_magnified: d.options.magnification == TextureFilter::Nearest,
                            nearest_minified: d.options.minification == TextureFilter::Nearest,
                            wrap: d.options.wrap_mode,
                            plain: None,
                        };
                        if let Some(look) = self.picture_look.filter(|_| Texture::is_picture(id, image)) {
                            texture.look(look);
                        }
                        self.textures.insert(id, texture);
                    }
                    Some([px, py]) => {
                        let Some(t) = self.textures.get_mut(&id) else { continue };
                        if px >= t.width {
                            // Wholly beyond the right edge: nothing of it lands in the texture.
                            continue;
                        }
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

    /// Paints the frame over the backdrop (or `clear`) and returns RGBA bytes (premultiplied): only the tiles that
    /// changed since the last frame are painted again ([`Software::damage`] says where).
    pub fn paint(&mut self, primitives: &[ClippedPrimitive], pixels_per_point: f32, clear: Color32) -> &[u8] {
        // Source canvases for asynchronous overlays never paint. Allocate their large
        // pixel storage only on the thread that actually performs the first raster.
        self.pixels.resize(self.width * self.height, [0; 4]);
        let areas = self.changed_areas(primitives, pixels_per_point, clear);
        self.damage = areas.iter().copied().reduce(around).map(|a| a.map(|v| v as u32));
        let width = self.width;
        let height = self.height;
        let backdrop = self.backdrop.as_deref();
        let textures = &self.textures;
        let clips: Vec<_> = primitives.iter().map(|p| self.clip(p, pixels_per_point)).collect();
        let paint_rows = |pixels: &mut [[u8; 4]], from: usize| {
            let band = [0, from, width, from + pixels.len() / width];
            for area in &areas {
                let Some(area) = within(*area, band) else { continue };
                for y in area[1]..area[3] {
                    let row = &mut pixels[(y-from)*width+area[0]..(y-from)*width+area[2]];
                    match backdrop {
                        Some(b) => row.copy_from_slice(&b[y*width+area[0]..y*width+area[2]]),
                        None => row.fill([clear.r(), clear.g(), clear.b(), clear.a()]),
                    }
                }
                for (p, clip) in primitives.iter().zip(&clips) {
                    let Primitive::Mesh(mesh) = &p.primitive else { continue };
                    let Some(original_clip) = *clip else { continue };
                    let Some(clip) = within(original_clip, area) else { continue };
                    let Some(texture) = textures.get(&mesh.texture_id) else { continue };
                    for t in mesh.indices.as_chunks::<3>().0 {
                        let v = t.map(|i| &mesh.vertices[i as usize]);
                        Self::triangle_rows(pixels, width, from, clip, original_clip, v, pixels_per_point, texture);
                    }
                }
            }
        };
        // Small damage stays serial. The page worker alone opts in, keeping the input thread free of thread setup.
        let damaged: usize = areas.iter().map(|a| (a[2]-a[0])*(a[3]-a[1])).sum();
        if self.parallel && damaged > 128 * 1024 {
            // Retro's layered page remains raster-bound at four bands on the VR
            // CPU fixture. Six bounded row jobs retain identical per-row arithmetic.
            let rows = height.div_ceil(6);
            std::thread::scope(|scope| {
                for (n, pixels) in self.pixels.chunks_mut(rows * width).enumerate() {
                    let paint_rows = &paint_rows;
                    scope.spawn(move || paint_rows(pixels, n * rows));
                }
            });
        } else {
            paint_rows(&mut self.pixels, 0);
        }
        self.pixels.as_flattened()
    }

    /// A primitive's clip rectangle in pixels (x0, y0, x1, y1), `None` when it shows nothing.
    fn clip(&self, p: &ClippedPrimitive, ppp: f32) -> Option<[usize; 4]> {
        let clip = [
            ((p.clip_rect.min.x * ppp).round().max(0.0) as usize).min(self.width),
            ((p.clip_rect.min.y * ppp).round().max(0.0) as usize).min(self.height),
            ((p.clip_rect.max.x * ppp).round().max(0.0) as usize).min(self.width),
            ((p.clip_rect.max.y * ppp).round().max(0.0) as usize).min(self.height),
        ];
        (clip[0] < clip[2] && clip[1] < clip[3]).then_some(clip)
    }

    /// What to paint again (x0, y0, x1, y1 rectangles): the tiles whose triangles, in painting order, are not the
    /// last frame's (the same triangles over the same backdrop and textures paint the same pixels), merged into a
    /// few rectangles; the whole frame when it cannot be compared.
    fn changed_areas(&mut self, primitives: &[ClippedPrimitive], ppp: f32, clear: Color32) -> Vec<[usize; 4]> {
        let (cols, rows) = (self.width.div_ceil(TILE), self.height.div_ceil(TILE));
        let mut next = std::mem::take(&mut self.next_tiles);
        next.clear();
        next.resize(cols * rows, 0);
        let scale = mix(0, ppp.to_bits() as u64);
        for p in primitives {
            let Primitive::Mesh(mesh) = &p.primitive else { continue };
            let Some(clip) = self.clip(p, ppp) else { continue };
            let texture = match mesh.texture_id {
                TextureId::Managed(id) => id << 1,
                TextureId::User(id) => (id << 1) | 1,
            };
            let place = clip.iter().fold(0u64, |h, &c| (h << 16) | (c as u64 & 0xffff));
            let base = mix(mix(scale, texture), place);
            for t in mesh.indices.as_chunks::<3>().0 {
                let v = [&mesh.vertices[t[0] as usize], &mesh.vertices[t[1] as usize], &mesh.vertices[t[2] as usize]];
                let Some([x0, y0, x1, y1]) = bounds(v.map(|v| [v.pos.x * ppp, v.pos.y * ppp]), clip) else { continue };
                let h = v.iter().fold(base, |h, v| vertex_hash(h, v));
                for row in y0 / TILE..=(y1 - 1) / TILE {
                    for tile in &mut next[row * cols + x0 / TILE..=row * cols + (x1 - 1) / TILE] {
                        *tile = mix(*tile, h);
                    }
                }
            }
        }
        let whole = self.stale || self.clear != Some(clear) || self.tiles.len() != next.len();
        let changed: Vec<bool> = if whole { vec![true; next.len()] } else { self.tiles.iter().zip(&next).map(|(a, b)| a != b).collect() };
        self.next_tiles = std::mem::replace(&mut self.tiles, next);
        self.stale = false;
        self.clear = Some(clear);
        let mut areas: Vec<[usize; 4]> = tile_areas(&changed, cols, rows)
            .into_iter()
            .map(|[c0, r0, c1, r1]| [c0 * TILE, r0 * TILE, (c1 * TILE).min(self.width), (r1 * TILE).min(self.height)])
            .collect();
        // Many areas, or areas filling most of the rectangle around them: that rectangle in one pass.
        if let Some(all) = areas.iter().copied().reduce(around) {
            let size = |a: &[usize; 4]| (a[2] - a[0]) * (a[3] - a[1]);
            if areas.len() > MAX_AREAS || 2 * areas.iter().map(size).sum::<usize>() >= size(&all) {
                areas = vec![all];
            }
        }
        areas
    }

    fn triangle_rows(pixels: &mut [[u8; 4]], width: usize, from: usize, clip: [usize; 4], original_clip: [usize; 4], v: [&Vertex; 3], ppp: f32, texture: &Texture) {
        let mut p = v.map(|v| [v.pos.x * ppp, v.pos.y * ppp]);
        let mut v = v;
        let edge = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let mut area = edge(p[0], p[1], p[2]);
        // Flat, or a corner that is not a number (it would scan the whole clip finding nothing inside).
        if area.is_nan() || area.abs() < 1e-6 {
            return;
        }
        if area < 0.0 {
            p.swap(1, 2);
            v.swap(1, 2);
            area = -area;
        }
        let Some([min_x, min_y, max_x, max_y]) = bounds(p, clip) else { return };
        let Some([origin_min_x, _, origin_max_x, _]) = bounds(p, original_clip) else { return };
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
        let flat = (flat_color && flat_uv).then(|| modulate(color[0], texture.sample(uv[0][0], uv[0][1], false)));
        let inv_area = 1.0 / area;
        // Change of colour and uv per pixel to the right (the barycentric weights change by A_i / area).
        let dl = [e[0].0 * inv_area, e[1].0 * inv_area, e[2].0 * inv_area];
        let dc: [f32; 4] = std::array::from_fn(|i| color[0][i] * dl[0] + color[1][i] * dl[1] + color[2][i] * dl[2]);
        let duv = [0, 1].map(|j| uv[0][j] * dl[0] + uv[1][j] * dl[1] + uv[2][j] * dl[2]);
        // Texels per pixel, across and down: more than one, the texture is minified here.
        let down = [e[0].1 * inv_area, e[1].1 * inv_area, e[2].1 * inv_area];
        let duv_down = [0, 1].map(|j| uv[0][j] * down[0] + uv[1][j] * down[1] + uv[2][j] * down[2]);
        let texels = |d: [f32; 2]| ((d[0] * texture.width as f32).powi(2) + (d[1] * texture.height as f32).powi(2)).sqrt();
        let minified = texels(duv).max(texels(duv_down)) > 1.0;
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
            let row = &mut pixels[(y - from) * width + first..=(y - from) * width + last];
            match flat {
                Some(c) => {
                    let src = to_u32(c);
                    if src[3] >= 255 {
                        row.fill([src[0] as u8, src[1] as u8, src[2] as u8, 255]);
                    } else if src[3] > 0 || src[..3].iter().any(|v| *v > 0) {
                        // One colour over the run: the four channels blended at once.
                        let (lanes, keep) = (spread(src), 255 - src[3] as u64);
                        for px in row {
                            blend_lanes(px, lanes, keep);
                        }
                    }
                }
                None => {
                    // Colour and texture coordinates are linear across the triangle: step them along the run.
                    // Start at the full primitive's sample: a damage clip must not change f32 accumulation.
                    let origin = if min_x == origin_min_x { first } else {
                        let (mut left, mut right) = (origin_min_x as f32, origin_max_x as f32);
                        for i in 0..3 {
                            let a = e[i].0;
                            if a > 0.0 { left = left.max(-k[i] / a - 1.5); }
                            else if a < 0.0 { right = right.min(-k[i] / a + 1.5); }
                            else if k[i] < 0.0 || (k[i] == 0.0 && !tl[i]) { right = left; }
                        }
                        let x0 = left.floor().max(origin_min_x as f32) as usize;
                        let x1 = (right.ceil() as usize).min(origin_max_x);
                        (x0..x1).find(|&x| inside(w_at(x))).unwrap_or(first)
                    };
                    let l = w_at(origin).map(|w| w * inv_area);
                    let mut c: [f32; 4] = std::array::from_fn(|i| color[0][i] * l[0] + color[1][i] * l[1] + color[2][i] * l[2]);
                    let mut t_uv = [0, 1].map(|j| uv[0][j] * l[0] + uv[1][j] * l[1] + uv[2][j] * l[2]);
                    for _ in origin..first {
                        for i in 0..4 { c[i] += dc[i]; }
                        t_uv[0] += duv[0]; t_uv[1] += duv[1];
                    }
                    if flat_uv {
                        // One texel (egui's white for shapes): it only scales the colour.
                        let texel = texture.sample(uv[0][0], uv[0][1], false);
                        let scale: [f32; 4] = std::array::from_fn(|i| texel[i] * (1.0 / 255.0));
                        for px in row {
                            let src = to_u32(std::array::from_fn(|i| c[i] * scale[i]));
                            blend(px, src, 255 - src[3]);
                            for i in 0..4 {
                                c[i] += dc[i];
                            }
                        }
                    } else {
                        for px in row {
                            let src = to_u32(modulate(c, texture.sample(t_uv[0], t_uv[1], minified)));
                            blend(px, src, 255 - src[3]);
                            for i in 0..4 {
                                c[i] += dc[i];
                            }
                            t_uv[0] += duv[0];
                            t_uv[1] += duv[1];
                        }
                    }
                }
            }
        }
    }
}

/// The pixels a triangle (its corners in pixels) can touch inside `clip` (x0, y0, x1, y1), as
/// [`Software::triangle_rows`] scans them; `None` when none (a corner that is not a number touches none).
fn bounds(p: [[f32; 2]; 3], clip: [usize; 4]) -> Option<[usize; 4]> {
    if !p.as_flattened().iter().all(|v| v.is_finite()) {
        return None;
    }
    let min_x = (p[0][0].min(p[1][0]).min(p[2][0]).floor().max(clip[0] as f32)) as usize;
    let min_y = (p[0][1].min(p[1][1]).min(p[2][1]).floor().max(clip[1] as f32)) as usize;
    let max_x = (p[0][0].max(p[1][0]).max(p[2][0]).ceil().min(clip[2] as f32)) as usize;
    let max_y = (p[0][1].max(p[1][1]).max(p[2][1]).ceil().min(clip[3] as f32)) as usize;
    (min_x < max_x && min_y < max_y).then_some([min_x, min_y, max_x, max_y])
}

/// Texel `i` of a row or column of `n` (at least one), as `mode` reaches it from beyond the edges.
fn wrap(i: i32, n: usize, mode: TextureWrapMode) -> usize {
    let n = n as i32;
    match mode {
        TextureWrapMode::ClampToEdge => i.clamp(0, n - 1) as usize,
        TextureWrapMode::Repeat => i.rem_euclid(n) as usize,
        TextureWrapMode::MirroredRepeat => {
            let i = i.rem_euclid(2 * n);
            (if i < n { i } else { 2 * n - 1 - i }) as usize
        }
    }
}

/// The part of `a` inside `b`, if any.
fn within(a: [usize; 4], b: [usize; 4]) -> Option<[usize; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

/// The rectangle around both.
fn around<T: Ord + Copy>(a: [T; 4], b: [T; 4]) -> [T; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// One step of a fast hash (FxHash's): order matters, so a tile's hash changes when its triangles swap.
#[inline]
fn mix(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95)
}

#[inline]
fn vertex_hash(h: u64, v: &Vertex) -> u64 {
    let h = mix(h, v.pos.x.to_bits() as u64 | (v.pos.y.to_bits() as u64) << 32);
    let h = mix(h, v.uv.x.to_bits() as u64 | (v.uv.y.to_bits() as u64) << 32);
    mix(h, u32::from_le_bytes(v.color.to_array()) as u64)
}

/// The changed tiles (`cols` × `rows`, row by row) as rectangles of tiles (c0, r0, c1, r1): runs along a row,
/// grown down while the rows below have the same run.
fn tile_areas(changed: &[bool], cols: usize, rows: usize) -> Vec<[usize; 4]> {
    let mut done = Vec::new();
    let mut open: Vec<[usize; 4]> = Vec::new();
    for r in 0..rows {
        let row = &changed[r * cols..(r + 1) * cols];
        let mut grown = Vec::new();
        let mut c = 0;
        while c < cols {
            if !row[c] {
                c += 1;
                continue;
            }
            let start = c;
            while c < cols && row[c] {
                c += 1;
            }
            match open.iter().position(|o| o[0] == start && o[2] == c) {
                Some(i) => {
                    let mut o = open.swap_remove(i);
                    o[3] = r + 1;
                    grown.push(o);
                }
                None => grown.push([start, r, c, r + 1]),
            }
        }
        done.append(&mut open);
        open = grown;
    }
    done.append(&mut open);
    done
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

/// The four lanes of a `u64`, 16 bits each, that [`blend_lanes`] works in: one channel (0–255) per lane.
const LANES: u64 = 0x00ff_00ff_00ff_00ff;

/// Channels (0–255 each) into 16-bit lanes.
#[inline]
fn spread(c: [u32; 4]) -> u64 {
    c[0] as u64 | (c[1] as u64) << 16 | (c[2] as u64) << 32 | (c[3] as u64) << 48
}

/// [`blend`] on the four channels at once: each lane holds 255 × 255 without spilling into the next, so the
/// multiplication, the rounded division by 255 and the clamp are done for all four in a few operations, bit for bit
/// the same.
#[inline]
fn blend_lanes(dst: &mut [u8; 4], src: u64, keep: u64) {
    let d = u32::from_le_bytes(*dst) as u64;
    let d = (d | d << 16) & 0x0000_ffff_0000_ffff;
    let d = (d | d << 8) & LANES;
    let t = d * keep + 0x0080_0080_0080_0080;
    let kept = ((t + ((t >> 8) & LANES)) >> 8) & LANES;
    let sum = kept + src;
    let sum = (sum | (((sum >> 8) & 0x0001_0001_0001_0001) * 0xff)) & LANES;
    let packed = (sum | sum >> 8) & 0x0000_ffff_0000_ffff;
    *dst = (((packed | packed >> 16) & 0xffff_ffff) as u32).to_le_bytes();
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};

    #[test]
    fn large_pixels_are_allocated_by_the_first_raster_not_the_source_canvas() {
        let mut software = Software::new(1408, 792);
        assert!(software.pixels().is_empty(), "an unpainted source canvas allocates no page pixels");
        let pixels = software.paint(&[], 1.0, Color32::TRANSPARENT);
        assert_eq!(pixels.len(), 1408 * 792 * 4);
        assert!(pixels.iter().all(|b| *b == 0), "the first raster still starts transparent");
    }

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
    fn parallel_rows_are_pixel_identical_including_damage_and_textures() {
        let ctx = egui::Context::default();
        let mut serial = Software::new(512, 513);
        let mut parallel = Software::new(512, 513);
        parallel.set_parallel(true);
        let image = ColorImage::filled([64, 64], Color32::from_rgba_premultiplied(75, 30, 50, 100));
        let texture = ctx.load_texture("test-image", image, egui::TextureOptions::LINEAR);
        for offset in [0.0, 13.25, -4.75, 0.0] {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(512.0, 513.0))), ..Default::default() };
            let mut full = ctx.run_ui(input, |ui| {
                let p = ui.painter();
                p.rect_filled(Rect::from_min_max(pos2(0.0, 0.0), pos2(512.0, 513.0)), 0.0, Color32::DARK_BLUE);
                p.image(texture.id(), Rect::from_min_max(pos2(7.5, offset), pos2(510.0, 510.0+offset)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                p.text(pos2(20.25, 120.5+offset), egui::Align2::LEFT_TOP, "Pictures", egui::FontId::proportional(32.0), Color32::WHITE);
                p.circle_filled(pos2(250.0, 256.0+offset), 90.0, Color32::from_white_alpha(80));
            });
            serial.set_textures(&mut full.textures_delta.clone());
            parallel.set_textures(&mut full.textures_delta);
            let primitives = ctx.tessellate(full.shapes, full.pixels_per_point);
            assert_eq!(serial.paint(&primitives, full.pixels_per_point, Color32::TRANSPARENT), parallel.paint(&primitives, full.pixels_per_point, Color32::TRANSPARENT));
            assert_eq!(serial.damage(), parallel.damage());
        }
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

    /// Paints a `size` frame of `ui` over black: the pixels, and what changed.
    fn frame(ctx: &egui::Context, sw: &mut Software, size: [f32; 2], ui: impl FnMut(&mut egui::Ui)) -> (Vec<u8>, Option<[u32; 4]>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(size[0], size[1]))), ..Default::default() };
        let mut full = ctx.run_ui(input, ui);
        sw.set_textures(&mut full.textures_delta);
        let prims = ctx.tessellate(full.shapes, full.pixels_per_point);
        let out = sw.paint(&prims, full.pixels_per_point, Color32::BLACK).to_vec();
        sw.free_textures(&mut full.textures_delta);
        (out, sw.damage())
    }

    #[test]
    fn paints_again_only_what_changed() {
        let ctx = egui::Context::default();
        let mut sw = Software::new(256, 128);
        let scene = |dot: f32| {
            move |ui: &mut egui::Ui| {
                ui.painter().rect_filled(Rect::from_min_max(pos2(0.0, 0.0), pos2(256.0, 128.0)), 0.0, Color32::from_rgb(20, 30, 40));
                ui.painter().circle_filled(pos2(dot, 64.0), 6.0, Color32::from_rgb(220, 40, 40));
            }
        };
        let (first, damage) = frame(&ctx, &mut sw, [256.0, 128.0], scene(40.0));
        assert_eq!(damage, Some([0, 0, 256, 128]), "the first frame is painted whole");
        let (same, damage) = frame(&ctx, &mut sw, [256.0, 128.0], scene(40.0));
        assert_eq!(damage, None, "the same frame again changes nothing");
        assert_eq!(same, first);
        // The dot moves: the tiles it left and those it reached are painted again, in two areas.
        let (moved, damage) = frame(&ctx, &mut sw, [256.0, 128.0], scene(200.0));
        assert_eq!(damage, Some([32, 32, 224, 96]));
        let px = |img: &[u8], x: usize, y: usize| img[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4].to_vec();
        assert_eq!(px(&moved, 40, 64), vec![20, 30, 40, 255], "the old dot is gone");
        assert_eq!(px(&moved, 200, 64), vec![220, 40, 40, 255]);
        assert_eq!(px(&moved, 120, 64), px(&first, 120, 64), "between them nothing was painted");
        // Painted whole, the same picture (to the rounding of colours stepped along a row).
        sw.invalidate();
        let (whole, damage) = frame(&ctx, &mut sw, [256.0, 128.0], scene(200.0));
        assert_eq!(damage, Some([0, 0, 256, 128]));
        assert!(moved.iter().zip(&whole).all(|(a, b)| a.abs_diff(*b) <= 2));
        // New glyphs change the font texture: everything is painted again.
        let (_, damage) = frame(&ctx, &mut sw, [256.0, 128.0], |ui| {
            scene(200.0)(ui);
            ui.painter().text(pos2(4.0, 4.0), egui::Align2::LEFT_TOP, "Orbiters", egui::FontId::proportional(18.0), Color32::WHITE);
        });
        assert_eq!(damage, Some([0, 0, 256, 128]));
    }

    #[test]
    fn a_change_of_order_is_painted() {
        let ctx = egui::Context::default();
        let mut sw = Software::new(64, 64);
        let scene = |blue_on_top: bool| {
            move |ui: &mut egui::Ui| {
                let red = (Rect::from_min_max(pos2(4.0, 4.0), pos2(40.0, 40.0)), Color32::RED);
                let blue = (Rect::from_min_max(pos2(24.0, 24.0), pos2(60.0, 60.0)), Color32::BLUE);
                let order = if blue_on_top { [red, blue] } else { [blue, red] };
                for (rect, color) in order {
                    ui.painter().rect_filled(rect, 0.0, color);
                }
            }
        };
        frame(&ctx, &mut sw, [64.0, 64.0], scene(false));
        let (img, damage) = frame(&ctx, &mut sw, [64.0, 64.0], scene(true));
        assert!(damage.is_some());
        assert_eq!(img[(30 * 64 + 30) * 4..(30 * 64 + 30) * 4 + 4], [0, 0, 255, 255], "blue is on top where they overlap");
    }

    /// Paints one mesh over black on a 64 × 32 painter.
    fn mesh_frame(sw: &mut Software, mesh: egui::Mesh) -> Vec<u8> {
        let prim = ClippedPrimitive { clip_rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 32.0)), primitive: Primitive::Mesh(mesh) };
        sw.paint(&[prim], 1.0, Color32::BLACK).to_vec()
    }

    fn texture(sw: &mut Software, id: TextureId, delta: egui::epaint::ImageDelta) {
        let mut d = TexturesDelta::default();
        d.set.entry(id).or_default().push(delta);
        sw.set_textures(&mut d);
    }

    #[test]
    fn odd_meshes_and_textures_are_harmless() {
        let mut sw = Software::new(64, 32);
        let rect = Rect::from_min_max(pos2(4.0, 4.0), pos2(20.0, 20.0));
        // A texture that was never set: nothing is drawn (not a solid rectangle).
        let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let mut mesh = egui::Mesh::with_texture(TextureId::User(7));
        mesh.add_rect_with_uv(rect, whole, Color32::RED);
        assert!(mesh_frame(&mut sw, mesh).as_chunks::<4>().0.iter().all(|p| *p == [0, 0, 0, 255]));
        // A texture without texels, and an update past its right edge: no panic.
        let empty = TextureId::User(8);
        texture(&mut sw, empty, egui::epaint::ImageDelta::full(egui::ColorImage::new([0, 4], Vec::new()), egui::TextureOptions::LINEAR));
        let mut mesh = egui::Mesh::with_texture(empty);
        mesh.add_rect_with_uv(rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        mesh_frame(&mut sw, mesh);
        let small = TextureId::User(9);
        let opts = egui::TextureOptions::NEAREST;
        texture(&mut sw, small, egui::epaint::ImageDelta::full(egui::ColorImage::filled([4, 4], Color32::GREEN), opts));
        texture(&mut sw, small, egui::epaint::ImageDelta::partial([6, 3], egui::ColorImage::filled([2, 2], Color32::BLUE), opts));
        // A corner that is not a number: the triangle is skipped at once.
        let mut mesh = egui::Mesh::with_texture(small);
        mesh.add_rect_with_uv(rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        mesh.vertices[0].pos.x = f32::NAN;
        let started = std::time::Instant::now();
        let img = mesh_frame(&mut sw, mesh);
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        // The other triangle of the quad still shows the green texture.
        assert!(img.as_chunks::<4>().0.contains(&[0, 255, 0, 255]));
    }

    #[test]
    fn wrap_modes() {
        let at = |mode, i| wrap(i, 4, mode);
        assert_eq!([-1, 0, 3, 4, 9].map(|i| at(TextureWrapMode::ClampToEdge, i)), [0, 0, 3, 3, 3]);
        assert_eq!([-1, 0, 3, 4, 9].map(|i| at(TextureWrapMode::Repeat, i)), [3, 0, 3, 0, 1]);
        assert_eq!([-1, 0, 3, 4, 5, 8].map(|i| at(TextureWrapMode::MirroredRepeat, i)), [0, 0, 3, 3, 2, 0]);
    }

    #[test]
    fn blending_four_lanes_at_once_is_exact() {
        // Every source, kept fraction and destination, against the channel-by-channel blend.
        for keep in 0..=255u32 {
            for s in 0..=255u32 {
                let src = [s, 255 - s, s / 3, (s * 7) & 255];
                for d in 0..=255u32 {
                    let dst = [d as u8, (255 - d) as u8, ((d * 13) & 255) as u8, (d ^ s) as u8];
                    let (mut a, mut b) = (dst, dst);
                    blend(&mut a, src, keep);
                    blend_lanes(&mut b, spread(src), keep as u64);
                    assert_eq!(a, b, "src {src:?} keep {keep} dst {dst:?}");
                }
            }
        }
    }

    #[test]
    fn changed_tiles_merge_into_few_areas() {
        let t = true;
        let f = false;
        // Two blocks: a 2 × 2 one at the top left, a column at the right.
        #[rustfmt::skip]
        let changed = [
            t, t, f, t,
            t, t, f, t,
            f, f, f, t,
        ];
        let mut areas = tile_areas(&changed, 4, 3);
        areas.sort();
        assert_eq!(areas, vec![[0, 0, 2, 2], [3, 0, 4, 3]]);
        // A run that changes width ends one area and starts another.
        let areas = tile_areas(&[t, t, f, t, f, f], 3, 2);
        assert_eq!(areas.len(), 2);
        assert!(tile_areas(&[f; 6], 3, 2).is_empty());
    }
    #[test]
    fn review_triangle_redraw_is_independent_of_damage_clip() {
        let texture = Texture { width: 64, height: 64, texels: (0..4096).map(|n| Color32::from_rgb((n * 37 % 256) as u8, (n * 73 % 256) as u8, (n * 19 % 256) as u8)).collect(), nearest_magnified: false, nearest_minified: false, wrap: TextureWrapMode::ClampToEdge, plain: None };
        let vertices = [
            Vertex { pos: pos2(3.3, 2.2), uv: pos2(0.0013, 0.0024), color: Color32::WHITE },
            Vertex { pos: pos2(510.8, 8.1), uv: pos2(0.982, 0.2), color: Color32::WHITE },
            Vertex { pos: pos2(220.2, 127.9), uv: pos2(0.25, 0.875), color: Color32::WHITE },
        ];
        let mut full = vec![[0; 4]; 512 * 128];
        let mut split = full.clone();
        Software::triangle_rows(&mut full, 512, 0, [0, 0, 512, 128], [0, 0, 512, 128], vertices.each_ref(), 1.0, &texture);
        Software::triangle_rows(&mut split, 512, 0, [0, 0, 211, 128], [0, 0, 512, 128], vertices.each_ref(), 1.0, &texture);
        Software::triangle_rows(&mut split, 512, 0, [211, 0, 512, 128], [0, 0, 512, 128], vertices.each_ref(), 1.0, &texture);
        let different = full.iter().zip(&split).filter(|(a,b)| a != b).count();
        assert!(full == split, "damage clipping changed {different} pixels from the identical full redraw");
    }

}
