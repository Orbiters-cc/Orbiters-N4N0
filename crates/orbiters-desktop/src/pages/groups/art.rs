//! The banner studio's model (REQUEST22 1, m1, m2): the three targets, our own drawing of the zones of VRChat's two
//! nameplate templates, one crop per target over one source picture, and the lossless export.
//!
//! The zones were measured on 2026-11-22 from VRChat's ORIGINAL template pictures linked in the 19 November 2026
//! developer update (Anchored 2048x385, Centered 2048x472): each zone's left and right edge on every row, simplified to
//! within 1.2 px, as fractions of the canvas. VRChat's pictures are not shipped; these contours are our own data. Where
//! zones overlap, the later one in each list is on top. They are guides: the real name-length breakpoints are not known.
use crate::controller::groups::Key;
use crate::controller::vrchat_data::Scope;
use orbiters_core::groups::PostDraft;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a picture is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    /// The group's banner (VRChat's menus and website). 16:9 at 1920x1080, inside the inventory's limits (over 64 px,
    /// under 2048 px); how VRChat crops it on each screen is not published.
    GroupBanner,
    /// The nameplate banner, Anchored mode (2048x385).
    Anchored,
    /// The nameplate banner, Centered mode (2048x472).
    Centered,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::GroupBanner, Target::Anchored, Target::Centered];
    pub fn size(self) -> (u32, u32) {
        match self {
            Target::GroupBanner => (1920, 1080),
            Target::Anchored => (2048, 385),
            Target::Centered => (2048, 472),
        }
    }
    pub fn aspect(self) -> f32 {
        let (w, h) = self.size();
        w as f32 / h as f32
    }
    /// Its part of an exported file's name (never translated: a file name).
    pub fn file_part(self) -> &'static str {
        match self {
            Target::GroupBanner => "group-banner", // i18n: not shown (file name)
            Target::Anchored => "nameplate-anchored-2048x385", // i18n: not shown (file name)
            Target::Centered => "nameplate-centered-2048x472", // i18n: not shown (file name)
        }
    }
    pub fn zones(self) -> &'static [(Zone, &'static [(f32, f32)])] {
        match self {
            Target::GroupBanner => &[],
            Target::Anchored => ANCHORED,
            Target::Centered => CENTERED,
        }
    }
}

/// What a part of a nameplate banner does, from VRChat's templates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    /// Shown on every nameplate.
    Always,
    /// Shown when the nameplate is expanded.
    Expanded,
    /// Anchored: may be cut off for a short name.
    ShortNames,
    /// Centered: may be cut off for a long name.
    LongNames,
    /// Under the nameplate's edge: the art runs into it; nothing important there.
    Bleed,
}

const ANCHORED_EXPANDED: &[(f32, f32)] = &[(0.0229, 0.0), (0.0146, 0.0104), (0.0078, 0.0338), (0.0029, 0.0675), (0.0005, 0.1039), (0.0005, 0.6), (0.6777, 0.6), (0.6772, 0.1039), (0.6743, 0.0623), (0.6699, 0.0338), (0.6631, 0.0104), (0.6548, 0.0)];
const ANCHORED_SHORT: &[(f32, f32)] = &[(0.6548, 0.0), (0.6548, 0.6), (1.0, 0.6), (1.0, 0.0)];
const ANCHORED_BLEED: &[(f32, f32)] = &[(0.0, 0.6), (0.0, 1.0), (1.0, 1.0), (1.0, 0.6)];
const ANCHORED_ALWAYS: &[(f32, f32)] = &[
    (0.0703, 0.2208), (0.061, 0.2286), (0.0498, 0.2468), (0.0332, 0.2935), (0.0269, 0.3195), (0.0166, 0.3766), (0.0103, 0.426), (0.0059, 0.4727),
    (0.0024, 0.5247), (0.0005, 0.574), (0.0005, 0.9688), (0.001, 0.9688), (0.0024, 0.9169), (0.0073, 0.8442), (0.0112, 0.8052), (0.0186, 0.7506),
    (0.0278, 0.7013), (0.0381, 0.6623), (0.0513, 0.6286), (0.063, 0.6104), (0.0752, 0.6), (0.3359, 0.5974), (0.3335, 0.5247), (0.3301, 0.4727),
    (0.3257, 0.426), (0.3174, 0.3636), (0.3091, 0.3195), (0.2974, 0.2753), (0.2886, 0.2519), (0.2788, 0.2338), (0.2656, 0.2208),
];
const CENTERED_BLEED: &[(f32, f32)] = &[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
const CENTERED_EXPANDED: &[(f32, f32)] = &[(0.1841, 0.0), (0.1738, 0.0127), (0.168, 0.0318), (0.1636, 0.0572), (0.1611, 0.0869), (0.1606, 0.4873), (0.8384, 0.4873), (0.8379, 0.0869), (0.8354, 0.0572), (0.8311, 0.0318), (0.8218, 0.0064), (0.8149, 0.0)];
const CENTERED_LONG: &[(f32, f32)] = &[
    (0.3511, 0.0), (0.3335, 0.0106), (0.3237, 0.0212), (0.3081, 0.0466), (0.2886, 0.0953), (0.2754, 0.1419), (0.2656, 0.1864), (0.2588, 0.2246),
    (0.2476, 0.3093), (0.2412, 0.3835), (0.2378, 0.4513), (0.2373, 0.4873), (0.7622, 0.4873), (0.7617, 0.4513), (0.7583, 0.3835), (0.752, 0.3093),
    (0.7407, 0.2246), (0.7266, 0.1525), (0.7109, 0.0953), (0.6914, 0.0466), (0.6738, 0.0191), (0.6582, 0.0042), (0.6494, 0.0),
];
const CENTERED_ALWAYS: &[(f32, f32)] = &[(0.2676, 0.178), (0.2559, 0.2436), (0.2461, 0.3242), (0.2393, 0.4174), (0.2373, 0.4873), (0.7622, 0.4873), (0.7603, 0.4174), (0.7534, 0.3242), (0.7437, 0.2436), (0.7319, 0.178)];

// The short-name zone runs on behind the expanded zone's rounded corner: under it.
const ANCHORED: &[(Zone, &[(f32, f32)])] = &[(Zone::ShortNames, ANCHORED_SHORT), (Zone::Expanded, ANCHORED_EXPANDED), (Zone::Bleed, ANCHORED_BLEED), (Zone::Always, ANCHORED_ALWAYS)];
const CENTERED: &[(Zone, &[(f32, f32)])] = &[(Zone::Bleed, CENTERED_BLEED), (Zone::Expanded, CENTERED_EXPANDED), (Zone::LongNames, CENTERED_LONG), (Zone::Always, CENTERED_ALWAYS)];

fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut odd = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// The zone at a point of the canvas (fractions), the topmost one; `None` outside every zone (the corners of Anchored's
/// rounded top).
pub fn zone_at(target: Target, x: f32, y: f32) -> Option<Zone> {
    target.zones().iter().rev().find(|(_, poly)| inside(poly, x, y)).map(|(z, _)| *z)
}

/// What a nameplate shows of the art, for the previews.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    pub expanded: bool,
    pub long_name: bool,
}

/// How much of a zone a nameplate shows (1 whole, 0 hidden): a guide, since VRChat publishes no exact rule.
pub fn shown(zone: Option<Zone>, look: Look) -> f32 {
    match zone {
        Some(Zone::Always) => 1.0,
        Some(Zone::Expanded) => if look.expanded { 1.0 } else { 0.0 },
        Some(Zone::ShortNames) => if look.expanded && look.long_name { 1.0 } else { 0.0 },
        Some(Zone::LongNames) => if look.long_name { 0.0 } else { 1.0 },
        // Under the plate's edge: some of it shows through.
        Some(Zone::Bleed) => 0.35,
        None => 0.0,
    }
}

/// One target's crop of the source: how far in (1 = the picture covers the target exactly), where its middle is (in
/// the turned picture, fractions), and how many quarter turns clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub zoom: f32,
    pub cx: f32,
    pub cy: f32,
    pub turns: u8,
}

impl Default for Crop {
    fn default() -> Self {
        Self { zoom: 1.0, cx: 0.5, cy: 0.5, turns: 0 }
    }
}

pub const ZOOM_MAX: f32 = 4.0;

impl Crop {
    /// The picture's size once turned.
    fn turned(self, src: (u32, u32)) -> (f32, f32) {
        if self.turns % 2 == 1 { (src.1 as f32, src.0 as f32) } else { (src.0 as f32, src.1 as f32) }
    }
    /// The part of the turned picture in view, as fractions of its width and height.
    pub fn view(self, src: (u32, u32), target: Target) -> (f32, f32) {
        let (w, h) = self.turned(src);
        let a = target.aspect();
        let zoom = self.zoom.clamp(1.0, ZOOM_MAX);
        if w / h > a {
            // Wider than the target: its full height at zoom 1.
            let vh = 1.0 / zoom;
            (vh * h * a / w, vh)
        } else {
            let vw = 1.0 / zoom;
            (vw, vw * w / a / h)
        }
    }
    /// The same crop kept inside the picture (never an empty edge).
    pub fn clamped(self, src: (u32, u32), target: Target) -> Self {
        let zoom = if self.zoom.is_finite() { self.zoom.clamp(1.0, ZOOM_MAX) } else { 1.0 };
        let c = Crop { zoom, turns: self.turns % 4, ..self };
        let (vw, vh) = c.view(src, target);
        let fix = |v: f32, half: f32| if v.is_finite() { v.clamp(half, (1.0 - half).max(half)) } else { 0.5 };
        Crop { cx: fix(self.cx, vw / 2.0), cy: fix(self.cy, vh / 2.0), ..c }
    }
    /// The point of the source picture (fractions) under the target's point (`u`, `v`).
    pub fn source_point(self, src: (u32, u32), target: Target, u: f32, v: f32) -> (f32, f32) {
        let (vw, vh) = self.view(src, target);
        let (x, y) = (self.cx + (u - 0.5) * vw, self.cy + (v - 0.5) * vh);
        match self.turns % 4 {
            1 => (y, 1.0 - x),
            2 => (1.0 - x, 1.0 - y),
            3 => (1.0 - y, x),
            _ => (x, y),
        }
    }
    /// Moved by a drag of `dx`, `dy` (fractions of the target).
    pub fn panned(self, src: (u32, u32), target: Target, dx: f32, dy: f32) -> Self {
        let (vw, vh) = self.view(src, target);
        Crop { cx: self.cx - dx * vw, cy: self.cy - dy * vh, ..self }.clamped(src, target)
    }
    /// Zoomed by `factor` keeping the target's point (`u`, `v`) where it is.
    pub fn zoomed(self, src: (u32, u32), target: Target, factor: f32, u: f32, v: f32) -> Self {
        let (vw, vh) = self.view(src, target);
        let next = Crop { zoom: (self.zoom * factor).clamp(1.0, ZOOM_MAX), ..self };
        let (nw, nh) = next.view(src, target);
        Crop { cx: self.cx + (u - 0.5) * (vw - nw), cy: self.cy + (v - 0.5) * (vh - nh), ..next }.clamped(src, target)
    }
    pub fn turned_once(self, src: (u32, u32), target: Target) -> Self {
        Crop { turns: (self.turns + 1) % 4, cx: 1.0 - self.cy, cy: self.cx, ..self }.clamped(src, target)
    }
}

/// The target's picture from `src`: every pixel sampled (bilinear, in premultiplied alpha: a transparent pixel lends no
/// colour to its neighbours, Major 10) where the crop puts it.
pub fn render(src: &image::RgbaImage, target: Target, crop: Crop) -> image::RgbaImage {
    let (tw, th) = target.size();
    let size = (src.width(), src.height());
    let crop = crop.clamped(size, target);
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    image::RgbaImage::from_fn(tw, th, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / tw as f32, (y as f32 + 0.5) / th as f32);
        let (sx, sy) = crop.source_point(size, target, u, v);
        sample(src, sx * sw - 0.5, sy * sh - 0.5, sw, sh)
    })
}

fn sample(src: &image::RgbaImage, x: f32, y: f32, sw: f32, sh: f32) -> image::Rgba<u8> {
    let x = x.clamp(0.0, sw - 1.0);
    let y = y.clamp(0.0, sh - 1.0);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(sw as u32 - 1), (y0 + 1).min(sh as u32 - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    // An exact pixel stays exact (a crop of the same size as the picture loses nothing).
    if fx < 1e-4 && fy < 1e-4 {
        return *src.get_pixel(x0, y0);
    }
    // Premultiplied: colour weighted by its alpha, then divided back.
    let p = |a: u32, b: u32| {
        let [r, g, b2, al] = src.get_pixel(a, b).0.map(|c| c as f32);
        [r * al / 255.0, g * al / 255.0, b2 * al / 255.0, al]
    };
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut mixed = [0f32; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        mixed[i] = top + (bottom - top) * fy;
    }
    let alpha = mixed[3];
    let colour = |v: f32| if alpha <= 0.0 { 0 } else { (v * 255.0 / alpha).round().clamp(0.0, 255.0) as u8 };
    image::Rgba([colour(mixed[0]), colour(mixed[1]), colour(mixed[2]), alpha.round().clamp(0.0, 255.0) as u8])
}

/// A lossless PNG of `img`: no metadata, alpha kept (m2).
pub fn encode_png(img: &image::RgbaImage) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive);
    encoder.write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)?;
    Ok(out)
}

/// A file name of the group's name: letters, digits and a few signs; something when nothing is left.
pub fn file_stem(group: &str, target: Target) -> String {
    let name: String = group.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).take(40).collect();
    let name = name.trim();
    if name.is_empty() { target.file_part().to_string() } else { format!("{name} - {}", target.file_part()) }
}

/// Writes `bytes` to `dir/<stem>.png`, or `<stem> (2).png`… : never over a file that is there (m2, M12), and never a
/// part of one: the bytes go whole to a hidden temporary file first, which then takes the first free name by a hard link
/// (that fails rather than replace) and goes away (Minor 3). A failure leaves nothing behind.
pub fn save_new(dir: &Path, stem: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".{}.part", uuid::Uuid::new_v4()));
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    let mut result = Err(std::io::Error::other("too many files of that name")); // i18n: not shown (io error, never displayed raw)
    for n in 1..1000 {
        let name = if n == 1 { format!("{stem}.png") } else { format!("{stem} ({n}).png") };
        let path = dir.join(name);
        match std::fs::hard_link(&temp, &path) {
            Ok(()) => {
                result = Ok(path);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    let _ = std::fs::remove_file(&temp);
    result
}

/// Where exports go: `Pictures/Orbiters/Group banners` (the test folders in tests and the screenshot harness).
pub fn export_dir() -> PathBuf {
    orbiters_core::paths::pictures_dir().unwrap_or_else(orbiters_core::paths::documents_root).join("Orbiters").join("Group banners")
}

/// The largest picture read, either side: past this it is scaled down on reading.
const SOURCE_MAX: u32 = 8192;
/// Pixels decoded at most (Major 5): a studio's source or a recent picture, and a picture of the page.
pub const SOURCE_PIXELS: u64 = 40_000_000;
pub const PICTURE_PIXELS: u64 = 16_000_000;
/// A picture file read at most.
pub const SOURCE_BYTES: u64 = 64 * 1024 * 1024;
/// One large decode at a time in the window (sources, recent pictures, gallery pictures).
static DECODING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A picture file as it should be seen, within bounds: its size checked before reading.
pub fn load_source(path: &Path) -> anyhow::Result<image::RgbaImage> {
    let len = std::fs::metadata(path)?.len();
    anyhow::ensure!(len <= SOURCE_BYTES, "picture file too large"); // i18n: not shown (worded by the studio)
    let bytes = std::fs::read(path)?;
    decode_source(&bytes)
}

pub fn decode_source(bytes: &[u8]) -> anyhow::Result<image::RgbaImage> {
    decode_bounded(bytes, SOURCE_PIXELS)
}

/// Decodes a picture within bounds (Major 5): its size read from its header first and refused past `max_pixels`, the
/// decoder's own allocation limit set, one at a time; then its orientation applied (EXIF) and its colours turned into
/// sRGB from its ICC profile when it has one (Major 10); at most [`SOURCE_MAX`] a side.
pub fn decode_bounded(bytes: &[u8], max_pixels: u64) -> anyhow::Result<image::RgbaImage> {
    use image::ImageDecoder;
    anyhow::ensure!(bytes.len() as u64 <= SOURCE_BYTES, "picture too large"); // i18n: not shown (worded by the studio)
    let _one = DECODING.lock().unwrap_or_else(|e| e.into_inner());
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(max_pixels * 8 + 16 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (w, h) = decoder.dimensions();
    anyhow::ensure!(w as u64 * h as u64 <= max_pixels && w > 0 && h > 0, "picture has too many pixels"); // i18n: not shown (worded by the studio)
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let icc = decoder.icc_profile().ok().flatten();
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let mut rgba = img.to_rgba8();
    if let Some(icc) = icc {
        to_srgb(&mut rgba, &icc)?;
    }
    if rgba.width() > SOURCE_MAX || rgba.height() > SOURCE_MAX {
        let k = (SOURCE_MAX as f64 / rgba.width() as f64).min(SOURCE_MAX as f64 / rgba.height() as f64);
        let to = |v: u32| ((v as f64 * k).round() as u32).clamp(1, SOURCE_MAX);
        rgba = shrink(&rgba, to(rgba.width()), to(rgba.height()));
    }
    Ok(rgba)
}

/// The pixels turned from the colour space their ICC profile describes into sRGB, alpha untouched.
pub fn to_srgb(img: &mut image::RgbaImage, icc: &[u8]) -> anyhow::Result<()> {
    let source = moxcms::ColorProfile::new_from_slice(icc).map_err(|_| anyhow::anyhow!("unreadable colour profile"))?; // i18n: not shown (worded by the studio)
    let srgb = moxcms::ColorProfile::new_srgb();
    let transform = source
        .create_transform_8bit(moxcms::Layout::Rgba, &srgb, moxcms::Layout::Rgba, moxcms::TransformOptions::default())
        .map_err(|_| anyhow::anyhow!("unsupported colour profile"))?; // i18n: not shown (worded by the studio)
    let input = img.as_raw().clone();
    let mut output = vec![0u8; input.len()];
    transform.transform(&input, &mut output).map_err(|_| anyhow::anyhow!("colour conversion failed"))?; // i18n: not shown (worded by the studio)
    // The alpha as it was.
    for (o, i) in output.as_chunks_mut::<4>().0.iter_mut().zip(input.as_chunks::<4>().0) {
        o[3] = i[3];
    }
    *img = image::RgbaImage::from_raw(img.width(), img.height(), output).ok_or_else(|| anyhow::anyhow!("colour conversion failed"))?; // i18n: not shown (worded by the studio)
    Ok(())
}

/// Where the picture came from, remembered with the crops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source {
    File(PathBuf),
    /// One of the account's VRChat gallery pictures.
    Inventory { id: String, url: String },
}

/// One source and its three crops, per account and group (M12): the same picture feeds all three targets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub source: Option<Source>,
    pub group_banner: Crop,
    pub anchored: Crop,
    pub centered: Crop,
}

impl Saved {
    pub fn crop(&self, t: Target) -> Crop {
        match t {
            Target::GroupBanner => self.group_banner,
            Target::Anchored => self.anchored,
            Target::Centered => self.centered,
        }
    }
    pub fn crop_mut(&mut self, t: Target) -> &mut Crop {
        match t {
            Target::GroupBanner => &mut self.group_banner,
            Target::Anchored => &mut self.anchored,
            Target::Centered => &mut self.centered,
        }
    }
}

/// The file of one account's crops for one group, under the local data folder.
pub fn saved_file(account: &str, group: &str) -> Option<PathBuf> {
    (orbiters_core::vrchat_accounts::is_user_id(account) && orbiters_core::groups::is_group_id(group))
        .then(|| orbiters_core::paths::local_data_root().join("group-banners").join(account).join(format!("{group}.json")))
}

pub fn load_saved(account: &str, group: &str) -> Saved {
    saved_file(account, group).and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn store_saved(account: &str, group: &str, saved: &Saved) {
    if let Some(file) = saved_file(account, group) {
        if let (Some(dir), Ok(bytes)) = (file.parent(), serde_json::to_vec_pretty(saved)) {
            let _ = std::fs::create_dir_all(dir);
            let _ = orbiters_core::write_atomic_private(&file, &bytes);
        }
    }
}

/// The new posts the person is writing, one file per account (the environment and Orbiters account too) and group: their
/// own words, who they are for and the notification, nothing read from VRChat but the ids of the roles they chose. An
/// edit of an existing post is never here (verify2 B-C).
#[derive(Default)]
pub struct Drafts {
    posts: Vec<(String, PostDraft)>,
    /// The last write failed: what is being written is in memory only.
    pub failed: bool,
}

fn drafts_root() -> PathBuf {
    let root = orbiters_core::paths::local_data_root().join("group-drafts");
    // Each test's windows share theirs, and no other test's.
    #[cfg(test)]
    let root = {
        thread_local!(static OWN: String = uuid::Uuid::new_v4().to_string());
        OWN.with(|own| root.join(own))
    };
    root
}

/// The folder of one account's drafts.
pub fn drafts_dir(scope: &Scope) -> Option<PathBuf> {
    Key::of(scope).map(|k| drafts_root().join(k.folder()))
}

impl Drafts {
    pub fn load(scope: &Scope) -> Self {
        // What earlier builds kept here held the posts being edited, words and author and all: gone.
        if let Ok(files) = std::fs::read_dir(drafts_root()) {
            for f in files.flatten().filter(|f| f.path().is_file()) {
                let _ = std::fs::remove_file(f.path());
            }
        }
        let mut posts = Vec::new();
        if let Some(Ok(files)) = drafts_dir(scope).map(std::fs::read_dir) {
            for path in files.flatten().map(|f| f.path()) {
                let Some(group) = path.file_stem().and_then(|s| s.to_str()).filter(|s| orbiters_core::groups::is_group_id(s)).map(str::to_string) else { continue };
                if let Some(d) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<PostDraft>(&b).ok()) {
                    posts.push((group, PostDraft { image_id: None, ..d }));
                }
            }
        }
        Self { posts, failed: false }
    }
    pub fn composer(&self, group: &str) -> Option<&PostDraft> {
        self.posts.iter().find(|(g, _)| g == group).map(|(_, d)| d)
    }
    /// The new post being written in `group`, or none any more. One that could not be written is tried again at the next
    /// change or frame.
    pub fn set(&mut self, scope: &Scope, group: &str, draft: Option<PostDraft>) {
        let empty = |d: &PostDraft| d.title.trim().is_empty() && d.text.trim().is_empty();
        let draft = draft.filter(|d| !empty(d)).map(|d| PostDraft { image_id: None, ..d });
        if self.composer(group) == draft.as_ref() {
            return;
        }
        let Some(file) = drafts_dir(scope).filter(|_| orbiters_core::groups::is_group_id(group)).map(|d| d.join(format!("{group}.json"))) else { return };
        let written = match &draft {
            Some(d) => file.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok()) && serde_json::to_vec(d).is_ok_and(|b| orbiters_core::write_atomic_private(&file, &b).is_ok()),
            None => std::fs::remove_file(&file).is_ok() || !file.exists(),
        };
        self.failed = !written;
        if written {
            self.posts.retain(|(g, _)| g != group);
            self.posts.extend(draft.map(|d| (group.to_string(), d)));
        }
    }
}

/// A picture's pixels as one number.
pub fn pixels(img: &image::RgbaImage) -> u64 {
    let mut h = 0xcbf29ce484222325u64 ^ ((img.width() as u64) << 32 | img.height() as u64);
    for c in img.as_raw().as_chunks::<8>().0 {
        h = (h ^ u64::from_le_bytes(*c)).wrapping_mul(0x100000001b3).rotate_left(23);
    }
    for b in img.as_raw().as_chunks::<8>().1 {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// The studio's art as one number: its very pixels and the group banner's crop (what an upload was made from, Blocker 5).
/// The same art gives the same number on every run and every visit; another file under the same name does not.
pub fn revision(pixels: u64, crop: Crop) -> u64 {
    let text = format!("{pixels:x}|{:.5}|{:.5}|{:.5}|{}", crop.zoom, crop.cx, crop.cy, crop.turns);
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// `img` made smaller, each pixel the average of those it covers, colour weighted by its alpha: no fringe beside
/// transparent ones (M10).
pub fn shrink(img: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    let (sw, sh) = img.dimensions();
    image::RgbaImage::from_fn(w, h, |x, y| {
        let span = |i: u32, n: u32, s: u32| ((i as u64 * s as u64 / n as u64) as u32, (((i as u64 + 1) * s as u64).div_ceil(n as u64) as u32).min(s));
        let ((x0, x1), (y0, y1)) = (span(x, w, sw), span(y, h, sh));
        let (mut c, mut a, mut n) = ([0u64; 3], 0u64, 0u64);
        for yy in y0..y1.max(y0 + 1) {
            for xx in x0..x1.max(x0 + 1) {
                let p = img.get_pixel(xx.min(sw - 1), yy.min(sh - 1)).0;
                for i in 0..3 {
                    c[i] += p[i] as u64 * p[3] as u64;
                }
                a += p[3] as u64;
                n += 1;
            }
        }
        let colour = |v: u64| if a == 0 { 0 } else { ((v + a / 2) / a).min(255) as u8 };
        image::Rgba([colour(c[0]), colour(c[1]), colour(c[2]), ((a + n / 2) / n.max(1)) as u8])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// m1: landmarks measured on VRChat's originals, checked without them.
    #[test]
    fn zone_landmarks() {
        let a = |x: f32, y: f32| zone_at(Target::Anchored, x / 2048.0, y / 385.0);
        // Row 120: the short-name zone starts near x 1388 (67.8 %).
        assert_eq!(a(1380.0, 120.0), Some(Zone::Expanded));
        assert_eq!(a(1396.0, 120.0), Some(Zone::ShortNames));
        assert_eq!(a(300.0, 160.0), Some(Zone::Always));
        assert_eq!(a(900.0, 160.0), Some(Zone::Expanded));
        assert_eq!(a(1000.0, 330.0), Some(Zone::Bleed));
        assert_eq!(a(8.0, 300.0), Some(Zone::Always), "the always-visible part reaches down the left edge");
        assert_eq!(a(40.0, 330.0), Some(Zone::Bleed));
        assert_eq!(a(2040.0, 3.0), Some(Zone::ShortNames));
        let c = |x: f32, y: f32| zone_at(Target::Centered, x / 2048.0, y / 472.0);
        // Rows from 230 down are bleed from edge to edge (48.7 %).
        for x in [5.0, 700.0, 1024.0, 1400.0, 2040.0] {
            assert_eq!(c(x, 232.0), Some(Zone::Bleed), "x {x}");
        }
        assert_eq!(c(1024.0, 150.0), Some(Zone::Always));
        assert_eq!(c(1024.0, 30.0), Some(Zone::LongNames));
        assert_eq!(c(400.0, 150.0), Some(Zone::Expanded));
        assert_eq!(c(1650.0, 150.0), Some(Zone::Expanded));
        assert_eq!(c(200.0, 100.0), Some(Zone::Bleed), "outside the plate's frame");
        assert_eq!(zone_at(Target::GroupBanner, 0.5, 0.5), None);
    }

    #[test]
    fn previews_show_what_each_look_shows() {
        let collapsed = Look { expanded: false, long_name: false };
        let expanded_long = Look { expanded: true, long_name: true };
        assert_eq!(shown(Some(Zone::Expanded), collapsed), 0.0);
        assert_eq!(shown(Some(Zone::ShortNames), Look { expanded: true, long_name: false }), 0.0);
        assert_eq!(shown(Some(Zone::ShortNames), expanded_long), 1.0);
        assert_eq!(shown(Some(Zone::LongNames), expanded_long), 0.0);
        assert_eq!(shown(Some(Zone::Always), collapsed), 1.0);
    }

    #[test]
    fn crops_stay_inside_the_picture_and_zoom_about_the_pointer() {
        let src = (4000, 3000);
        let t = Target::Anchored;
        let c = Crop::default().clamped(src, t);
        let (vw, vh) = c.view(src, t);
        assert!((vw - 1.0).abs() < 1e-4 && vh < 1.0, "a wide target takes the picture's full width");
        let far = Crop { cy: 5.0, cx: -3.0, ..c }.clamped(src, t);
        assert!((far.cx - 0.5).abs() < 1e-4 && (far.cy - (1.0 - vh / 2.0)).abs() < 1e-4);
        let z = c.zoomed(src, t, 2.0, 0.25, 0.5);
        let before = c.source_point(src, t, 0.25, 0.5);
        let after = z.source_point(src, t, 0.25, 0.5);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3, "{before:?} {after:?}");
        // A quarter turn: the source's top-left corner is the turned picture's top-right.
        let turned = Crop { turns: 1, ..Crop::default() }.clamped((1000, 1000), Target::GroupBanner);
        let (x, y) = turned.source_point((1000, 1000), Target::GroupBanner, 0.999, 0.0);
        assert!(x < 0.3 && y < 0.01, "{x} {y}");
    }

    /// m2: an export is lossless (same size, every pixel and its alpha), carries no metadata, and never overwrites.
    #[test]
    fn exports_are_lossless_and_never_overwrite() {
        let (w, h) = Target::Anchored.size();
        let src = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x * 7 + y) % 256) as u8, (255 - (x % 200)) as u8]));
        let out = render(&src, Target::Anchored, Crop::default());
        assert_eq!(out, src, "a crop of the target's own size changes nothing");
        let png = encode_png(&out).unwrap();
        let back = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(back, src, "lossless, alpha kept");
        assert!(!png.windows(4).any(|w| w == b"tEXt" || w == b"eXIf" || w == b"iTXt"), "no metadata");
        let dir = std::env::temp_dir().join(format!("orbiters-banner-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = save_new(&dir, "Makers - group-banner", b"one").unwrap();
        let second = save_new(&dir, "Makers - group-banner", b"two").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"one", "the first file untouched");
        assert!(second.to_string_lossy().ends_with("Makers - group-banner (2).png"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(file_stem("Made-up: Makers/..", Target::Centered), "Made-up Makers - nameplate-centered-2048x472");
    }

    #[test]
    fn crops_are_kept_per_account_and_group() {
        let me = "usr_00000000-0000-4000-8000-0000000000dd";
        let g = "grp_00000000-0000-4000-8000-0000000000ee";
        let file = saved_file(me, g).unwrap();
        assert!(file.starts_with(orbiters_core::paths::local_data_root()), "under the test folders");
        let mut saved = Saved { source: Some(Source::File("C:/made-up.png".into())), ..Default::default() };
        saved.crop_mut(Target::Centered).zoom = 2.0;
        store_saved(me, g, &saved);
        assert_eq!(load_saved(me, g), saved);
        assert_eq!(load_saved(me, "grp_00000000-0000-4000-8000-0000000000ef"), Saved::default(), "another group: its own");
        assert!(saved_file("usr_x/../y", g).is_none());
        let _ = std::fs::remove_file(file);
    }

    /// Major 5: a picture whose header announces more pixels than allowed is refused from its header, before any
    /// pixel is decoded; a picture file over the byte limit is refused before it is read.
    #[test]
    fn decoding_is_bounded_before_it_starts() {
        // A tiny PNG that says it is 30000 × 30000.
        let mut png = encode_png(&image::RgbaImage::new(1, 1)).unwrap();
        png[16..20].copy_from_slice(&30000u32.to_be_bytes());
        png[20..24].copy_from_slice(&30000u32.to_be_bytes());
        let started = std::time::Instant::now();
        assert!(decode_bounded(&png, SOURCE_PIXELS).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "refused from its header");
        let ok = encode_png(&image::RgbaImage::new(64, 64)).unwrap();
        assert!(decode_bounded(&ok, 64 * 64).is_ok());
        assert!(decode_bounded(&ok, 64 * 63).is_err(), "one pixel over the bound");
    }

    /// Major 10: an ICC-tagged picture is turned into sRGB (a Display P3 colour changes; an sRGB-tagged one does not),
    /// transparency is kept, and a transparent pixel lends no colour to an opaque neighbour when resampled.
    #[test]
    fn colours_are_managed_and_edges_are_clean() {
        use image::ImageEncoder;
        let tagged = |profile: moxcms::ColorProfile| {
            let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 180]));
            let mut out = Vec::new();
            let mut enc = image::codecs::png::PngEncoder::new(&mut out);
            enc.set_icc_profile(profile.encode().unwrap()).unwrap();
            enc.write_image(img.as_raw(), 4, 4, image::ExtendedColorType::Rgba8).unwrap();
            decode_source(&out).unwrap()
        };
        let p3 = tagged(moxcms::ColorProfile::new_display_p3());
        let px = p3.get_pixel(0, 0).0;
        assert_ne!(&px[..3], &[200, 100, 50], "a P3 colour is not the same numbers in sRGB");
        assert_eq!(px[3], 180, "alpha kept");
        let srgb = tagged(moxcms::ColorProfile::new_srgb());
        let px = srgb.get_pixel(0, 0).0;
        assert!(px[0].abs_diff(200) <= 2 && px[1].abs_diff(100) <= 2 && px[2].abs_diff(50) <= 2, "{px:?}");
        // Opaque red beside transparent blue: halfway, red at half alpha (not purple).
        let edge = image::RgbaImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 0]) });
        let mid = sample(&edge, 0.5, 0.0, 2.0, 1.0).0;
        assert_eq!((mid[0], mid[2]), (255, 0), "no colour from the transparent pixel: {mid:?}");
        assert!(mid[3].abs_diff(128) <= 1);
        // The same when a source over the working size is first reduced: columns of red beside transparent blue.
        let wide = image::RgbaImage::from_fn(8, 4, |x, _| if x % 2 == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 0]) });
        let small = shrink(&wide, 4, 2);
        assert_eq!((small.width(), small.height()), (4, 2));
        for p in small.pixels() {
            assert_eq!((p.0[0], p.0[1], p.0[2]), (255, 0, 0), "no fringe: {p:?}");
            assert!(p.0[3].abs_diff(128) <= 1);
        }
    }

    /// Major 10: an EXIF orientation in a real file is applied (a picture turned a quarter on its side comes upright).
    #[test]
    fn exif_orientation_is_applied() {
        use image::ImageEncoder;
        // 4 × 2, left half red, right half blue; orientation 6 (turn 90° clockwise to view).
        let img = image::RgbaImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) });
        let exif: Vec<u8> = vec![b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0];
        let mut out = Vec::new();
        let mut enc = image::codecs::png::PngEncoder::new(&mut out);
        enc.set_exif_metadata(exif).unwrap();
        enc.write_image(img.as_raw(), 4, 2, image::ExtendedColorType::Rgba8).unwrap();
        let seen = decode_source(&out).unwrap();
        assert_eq!((seen.width(), seen.height()), (2, 4), "turned upright");
        assert_eq!(seen.get_pixel(0, 0).0[0], 255, "red on top after a clockwise turn");
        assert_eq!(seen.get_pixel(0, 3).0[2], 255, "blue below");
    }

    /// Minor 3: a write that fails leaves no file behind, and a written file is whole (no `.part` left).
    #[test]
    fn an_export_is_whole_or_absent() {
        let dir = std::env::temp_dir().join(format!("orbiters-banner-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(save_new(&dir, "bad\u{0}name", b"bytes").is_err(), "a name the disk refuses");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert!(left.is_empty(), "nothing left behind: {left:?}");
        let path = save_new(&dir, "good", b"whole").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"whole");
        let parts = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count();
        assert_eq!(parts, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
