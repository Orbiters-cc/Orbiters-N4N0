//! The banner studio's model (REQUEST22 1, m1, m2): the three targets, our own drawing of the zones of VRChat's two
//! nameplate templates, one crop per target over one source picture, and the lossless export.
//!
//! The zones were measured on 2026-11-22 from VRChat's ORIGINAL template pictures linked in the 19 November 2026
//! developer update (Anchored 2048x385, Centered 2048x472): each zone's left and right edge on every row, simplified to
//! within 1.2 px, as fractions of the canvas. VRChat's pictures are not shipped; these contours are our own data. Where
//! zones overlap, the later one in each list is on top. They are guides: the real name-length breakpoints are not known.
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

/// The target's picture from `src`: every pixel sampled (bilinear) where the crop puts it.
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
    let p = |a: u32, b: u32| src.get_pixel(a, b).0.map(|c| c as f32);
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        out[i] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    image::Rgba(out)
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

/// Writes `bytes` to `dir/<stem>.png`, or `<stem> (2).png`… : never over a file that is there (m2, M12).
pub fn save_new(dir: &Path, stem: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    for n in 1..1000 {
        let name = if n == 1 { format!("{stem}.png") } else { format!("{stem} ({n}).png") };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files of that name")) // i18n: not shown (io error, never displayed raw)
}

/// Where exports go: `Pictures/Orbiters/Group banners` (the test folders in tests and the screenshot harness).
pub fn export_dir() -> PathBuf {
    orbiters_core::paths::pictures_dir().unwrap_or_else(orbiters_core::paths::documents_root).join("Orbiters").join("Group banners")
}

/// The largest picture read, either side: past this it is scaled down on reading.
const SOURCE_MAX: u32 = 8192;

/// A picture file as it should be seen: its orientation applied (JPEG's EXIF), at most [`SOURCE_MAX`] a side.
pub fn load_source(path: &Path) -> anyhow::Result<image::RgbaImage> {
    let bytes = std::fs::read(path)?;
    decode_source(&bytes)
}

pub fn decode_source(bytes: &[u8]) -> anyhow::Result<image::RgbaImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    if img.width() > SOURCE_MAX || img.height() > SOURCE_MAX {
        img = img.resize(SOURCE_MAX, SOURCE_MAX, image::imageops::FilterType::Triangle);
    }
    Ok(img.to_rgba8())
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
}
