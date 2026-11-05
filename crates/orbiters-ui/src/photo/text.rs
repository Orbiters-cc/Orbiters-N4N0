//! Text layers, as the Toolkit photoshoot draws them (`PhotoshootText.cs`): the glyphs' coverage, then their outline,
//! shadow or glow and colours, the whole placed by its middle and sized by a share of the picture's height. Glyph
//! outlines come from the font file (skrifa) and are filled by tiny-skia, so the preview and the saved picture have
//! the same letters. Lines are centred under each other; letters a font lacks are left out.
//!
//! The fonts: the ones the Toolkit's photoshoot offers through My Avatar (Bonbon, Creepster, Diplomata, JetBrains
//! Mono, Matemasie, Monoton, Pirata One, Plaster) and Inter ExtraBold for the default, all under the SIL Open Font
//! License and built in, then the fonts installed on this computer ([`system`]), read from their files when used.

use super::{decode, lerp, Image, Text, TextStyle};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{FontRef, LocationRef, MetadataProvider, Size};
use skrifa::string::StringId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A font offered to text layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontInfo {
    /// What [`Text::font`] holds: a built-in font's name, or an installed font's file.
    pub key: String,
    /// What the list shows.
    pub name: String,
    pub bundled: bool,
}

/// The built-in fonts (SIL Open Font License, `assets/fonts/<family>/OFL.txt`): the default first.
const BUNDLED: [(&str, &[u8]); 9] = [
    ("Inter", include_bytes!("../../assets/fonts/Inter/Inter-ExtraBold.ttf")),
    ("Bonbon", include_bytes!("../../assets/fonts/Bonbon/Bonbon-Regular.ttf")),
    ("Creepster", include_bytes!("../../assets/fonts/Creepster/Creepster-Regular.ttf")),
    ("Diplomata", include_bytes!("../../assets/fonts/Diplomata/Diplomata-Regular.ttf")),
    ("JetBrains Mono", include_bytes!("../../assets/fonts/JetBrains_Mono/JetBrainsMono-Regular.ttf")),
    ("Matemasie", include_bytes!("../../assets/fonts/Matemasie/Matemasie-Regular.ttf")),
    ("Monoton", include_bytes!("../../assets/fonts/Monoton/Monoton-Regular.ttf")),
    ("Pirata One", include_bytes!("../../assets/fonts/Pirata_One/PirataOne-Regular.ttf")),
    ("Plaster", include_bytes!("../../assets/fonts/Plaster/Plaster-Regular.ttf")),
];

/// The built-in fonts, the default ("") first.
pub fn bundled() -> Vec<FontInfo> {
    BUNDLED.iter().enumerate().map(|(i, (name, _))| FontInfo { key: if i == 0 { String::new() } else { (*name).into() }, name: (*name).into(), bundled: true }).collect()
}

/// A font file's bytes: built in, or an installed file mapped (most of a font is never read).
type Data = Arc<dyn AsRef<[u8]> + Send + Sync>;

struct Static(&'static [u8]);

impl AsRef<[u8]> for Static {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

/// The font of `key` ([`FontInfo::key`]); the default for an unknown or missing one.
pub fn data(key: &str) -> Data {
    if let Some((_, bytes)) = BUNDLED.iter().find(|(name, _)| *name == key) {
        return Arc::new(Static(bytes));
    }
    static OPEN: OnceLock<Mutex<HashMap<String, Option<Data>>>> = OnceLock::new();
    let opened = if key.is_empty() {
        None
    } else {
        let mut open = OPEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        open.entry(key.to_string()).or_insert_with(|| map(Path::new(key))).clone()
    };
    opened.unwrap_or_else(|| Arc::new(Static(BUNDLED[0].1)))
}

fn map(path: &Path) -> Option<Data> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: an installed font file is not changed while it is in use (Windows does not replace an open font file);
    // the bytes are only read.
    let data: Data = match unsafe { memmap2::Mmap::map(&file) } {
        Ok(m) => Arc::new(m),
        Err(_) => Arc::new(std::fs::read(path).ok()?),
    };
    FontRef::from_index(data.as_ref().as_ref(), 0).ok()?;
    Some(data)
}

/// The fonts installed on this computer, by name (a style other than regular after it), those that have Latin letters.
/// Reads every font file's names: call it off the interface thread.
pub fn system() -> Vec<FontInfo> {
    let mut files = Vec::new();
    for dir in font_dirs() {
        walk(&dir, 0, &mut files);
    }
    let mut out: Vec<FontInfo> = files
        .into_iter()
        .filter_map(|path| {
            let data = map(&path)?;
            let font = FontRef::from_index(data.as_ref().as_ref(), 0).ok()?;
            // Symbol and icon fonts have no "A".
            font.charmap().map('A').filter(|g| g.to_u32() != 0)?;
            let name = |id| font.localized_strings(id).english_or_first().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
            let family = name(StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| name(StringId::FAMILY_NAME))?;
            let style = name(StringId::TYPOGRAPHIC_SUBFAMILY_NAME).or_else(|| name(StringId::SUBFAMILY_NAME)).unwrap_or_default();
            let name = if style.is_empty() || style.eq_ignore_ascii_case("regular") { family } else { format!("{family} {style}") };
            Some(FontInfo { key: path.to_string_lossy().into_owned(), name, bundled: false })
        })
        .collect();
    out.sort_by_key(|f| f.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 3 {
                walk(&p, depth + 1, out);
            }
        } else if p.extension().is_some_and(|x| ["ttf", "otf", "ttc"].iter().any(|k| x.eq_ignore_ascii_case(k))) {
            out.push(p);
        }
    }
}

#[cfg(windows)]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts")];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
    }
    dirs
}

#[cfg(not(windows))]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs
}

/// Glyph outlines into a tiny-skia path, from the font's y-up units at `origin` to the picture's y-down pixels.
struct Pen<'a> {
    path: &'a mut tiny_skia::PathBuilder,
    x: f32,
    y: f32,
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x, self.y - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x, self.y - y);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.path.quad_to(self.x + cx0, self.y - cy0, self.x + x, self.y - y);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.cubic_to(self.x + cx0, self.y - cy0, self.x + cx1, self.y - cy1, self.x + x, self.y - y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// The font size in pixels of `text` on a picture `height` pixels high.
pub fn pixel_size(text: &Text, height: u32) -> f32 {
    (text.size.clamp(Text::MIN_SIZE, Text::MAX_SIZE) * height as f32).clamp(4.0, 2048.0)
}

/// The letters' outlines at `px` pixels, lines centred under each other, the first baseline at the font's ascent.
pub fn outline(text: &Text, px: f32) -> Option<tiny_skia::Path> {
    let data = data(&text.font);
    let font = FontRef::from_index(data.as_ref().as_ref(), 0).ok()?;
    let size = Size::new(px);
    let location = LocationRef::default();
    let metrics = font.metrics(size, location);
    let advances = font.glyph_metrics(size, location);
    let charmap = font.charmap();
    let glyphs = font.outline_glyphs();
    let line_h = (metrics.ascent - metrics.descent + metrics.leading).max(px * 0.8);
    // Each line's glyphs where they start, and its width.
    let lines: Vec<(Vec<(skrifa::GlyphId, f32)>, f32)> = text
        .value
        .lines()
        .map(|line| {
            let mut x = 0.0;
            let glyphs = line
                .chars()
                .filter(|c| !c.is_control())
                .filter_map(|c| {
                    let g = charmap.map(c)?;
                    let at = x;
                    x += advances.advance_width(g).unwrap_or(0.0);
                    Some((g, at))
                })
                .collect();
            (glyphs, x)
        })
        .collect();
    let widest = lines.iter().map(|l| l.1).fold(0f32, f32::max);
    let mut path = tiny_skia::PathBuilder::new();
    for (i, (line, width)) in lines.iter().enumerate() {
        let left = (widest - width) * 0.5;
        let baseline = metrics.ascent + i as f32 * line_h;
        for &(g, x) in line {
            if let Some(glyph) = glyphs.get(g) {
                let _ = glyph.draw(DrawSettings::unhinted(size, location), &mut Pen { path: &mut path, x: left + x, y: baseline });
            }
        }
    }
    path.finish()
}

/// Where the letters of `text` are on a `width`×`height` picture, in shares: left, top, right, bottom (the box a canvas
/// shows to drag it by). None for empty text.
pub fn bounds(text: &Text, width: u32, height: u32) -> Option<[f32; 4]> {
    let px = pixel_size(text, height);
    let b = outline(text, px)?.bounds();
    let (cx, cy) = (text.center[0] * width as f32, text.center[1] * height as f32);
    let (w, h) = (b.width(), b.height());
    Some([(cx - w * 0.5) / width as f32, (cy - h * 0.5) / height as f32, (cx + w * 0.5) / width as f32, (cy + h * 0.5) / height as f32])
}

/// How a style looks at a font size of `px` pixels (the Toolkit's `Look.Of`): the outline's and blur's widths, the
/// shadow's offset (down and right) and the colours of the fill, the edge and the shadow or glow (encoded, straight).
struct Look {
    style: TextStyle,
    outline: f32,
    blur: f32,
    shadow: [f32; 2],
    fill: [f32; 4],
    edge: [f32; 4],
    effect: [f32; 4],
}

impl Look {
    fn of(text: &Text, px: f32) -> Self {
        let fill = [text.color[0] as f32 / 255.0, text.color[1] as f32 / 255.0, text.color[2] as f32 / 255.0, 1.0];
        // The edge contrasts with the letters: dark around light text, light around dark text.
        let light = fill[0] * 0.299 + fill[1] * 0.587 + fill[2] * 0.114 > 0.5;
        let edge = if light { [0.05, 0.05, 0.07, 1.0] } else { [1.0; 4] };
        let mut look = Look { style: text.style, outline: 0.0, blur: 0.0, shadow: [0.0; 2], fill, edge, effect: [0.0; 4] };
        match text.style {
            TextStyle::Clean => {}
            TextStyle::Outline => look.outline = (px * 0.075).max(1.5),
            TextStyle::Shadow => {
                look.blur = (px * 0.07).max(1.0);
                look.shadow = [px * 0.04, px * 0.06];
                look.effect = [0.0, 0.0, 0.0, 0.75];
            }
            TextStyle::Glow => {
                // Neon: the letters' own colour around them, the letters themselves brighter.
                look.outline = (px * 0.05).max(1.0);
                look.blur = (px * 0.16).max(2.0);
                look.effect = fill;
                look.fill = [lerp(fill[0], 1.0, 0.55), lerp(fill[1], 1.0, 0.55), lerp(fill[2], 1.0, 0.55), 1.0];
            }
        }
        look
    }
}

/// A text drawn: linear premultiplied pixels, `width`×`height`, its middle at the text's centre.
pub struct Drawn {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

/// The letters' coverage (0 to 1) of `path` moved by `offset`, in a `w`×`h` box, with the outline of `outline` pixels
/// around them when it is more than none.
fn coverage(path: &tiny_skia::Path, offset: [f32; 2], w: u32, h: u32, outline: f32) -> Vec<f32> {
    let Some(mut pixmap) = tiny_skia::Pixmap::new(w, h) else { return vec![0.0; (w * h) as usize] };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let at = tiny_skia::Transform::from_translate(offset[0], offset[1]);
    pixmap.fill_path(path, &paint, tiny_skia::FillRule::Winding, at, None);
    if outline > 0.0 {
        let stroke = tiny_skia::Stroke { width: outline * 2.0, line_join: tiny_skia::LineJoin::Round, line_cap: tiny_skia::LineCap::Round, ..Default::default() };
        pixmap.stroke_path(path, &paint, &stroke, at, None);
    }
    pixmap.pixels().iter().map(|p| p.alpha() as f32 / 255.0).collect()
}

/// Softened like the Toolkit's three box blurs of this radius.
fn soften(c: &[f32], w: u32, h: u32, radius: f32) -> Vec<f32> {
    let mut img = Image { width: w, height: h, data: c.iter().map(|v| [*v, 0.0, 0.0, 0.0]).collect() };
    super::blur::gaussian(&mut img, radius / 1.7);
    img.data.iter().map(|p| p[0]).collect()
}

/// Straight-alpha "over": `color` at `coverage` on top of what is there (the Toolkit's `Over`).
fn over(pixels: &mut [[f32; 4]], coverage: &[f32], color: [f32; 4]) {
    for (d, c) in pixels.iter_mut().zip(coverage) {
        let sa = c * color[3];
        if sa <= 0.0 {
            continue;
        }
        let a = sa + d[3] * (1.0 - sa);
        let rgb = [0, 1, 2].map(|k| (color[k] * sa + d[k] * d[3] * (1.0 - sa)) / a);
        *d = [rgb[0], rgb[1], rgb[2], a];
    }
}

/// `text` drawn for a picture `height` pixels high: None for empty text.
pub fn draw(text: &Text, height: u32) -> Option<Drawn> {
    if text.value.trim().is_empty() {
        return None;
    }
    let px = pixel_size(text, height);
    let path = outline(text, px)?;
    let look = Look::of(text, px);
    let pad = (look.outline + look.blur * 2.0 + look.shadow[0].abs().max(look.shadow[1].abs())).ceil() + 2.0;
    let b = path.bounds();
    let (w, h) = (((b.width() + pad * 2.0).ceil() as u32).clamp(1, 8192), ((b.height() + pad * 2.0).ceil() as u32).clamp(1, 8192));
    let origin = [pad - b.left(), pad - b.top()];
    let fill = coverage(&path, origin, w, h, 0.0);
    let ring = (look.outline > 0.0).then(|| coverage(&path, origin, w, h, look.outline));
    let mut pixels = vec![[0f32; 4]; (w * h) as usize];
    match look.style {
        TextStyle::Shadow => {
            let shifted = coverage(&path, [origin[0] + look.shadow[0].round(), origin[1] + look.shadow[1].round()], w, h, 0.0);
            over(&mut pixels, &soften(&shifted, w, h, look.blur), look.effect);
        }
        TextStyle::Glow => {
            let glow: Vec<f32> = soften(ring.as_ref().unwrap_or(&fill), w, h, look.blur).iter().map(|v| (v * 1.6).min(1.0)).collect();
            over(&mut pixels, &glow, look.effect);
        }
        _ => {}
    }
    if let (Some(ring), true) = (&ring, look.style != TextStyle::Glow) {
        over(&mut pixels, ring, look.edge);
    }
    over(&mut pixels, &fill, look.fill);
    let pixels = pixels.into_iter().map(|p| [decode(p[0]) * p[3], decode(p[1]) * p[3], decode(p[2]) * p[3], p[3]]).collect();
    Some(Drawn { width: w, height: h, pixels })
}

/// Draws `text` over `img` at `opacity`.
pub fn composite(img: &mut Image, text: &Text, opacity: f32) {
    let Some(d) = draw(text, img.height) else { return };
    let left = (text.center[0] * img.width as f32 - d.width as f32 * 0.5).round() as i64;
    let top = (text.center[1] * img.height as f32 - d.height as f32 * 0.5).round() as i64;
    let k = opacity.clamp(0.0, 1.0);
    for y in 0..d.height as i64 {
        let iy = top + y;
        if iy < 0 || iy >= img.height as i64 {
            continue;
        }
        for x in 0..d.width as i64 {
            let ix = left + x;
            if ix < 0 || ix >= img.width as i64 {
                continue;
            }
            let s = d.pixels[(y * d.width as i64 + x) as usize];
            if s[3] <= 0.0 {
                continue;
            }
            let p = &mut img.data[(iy * img.width as i64 + ix) as usize];
            let a = s[3] * k;
            for c in 0..4 {
                p[c] = s[c] * k + p[c] * (1.0 - a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_font_draws_letters() {
        for f in bundled() {
            let text = Text { value: "Aa".into(), font: f.key.clone(), style: TextStyle::Clean, ..Default::default() };
            let d = draw(&text, 400).unwrap_or_else(|| panic!("{} draws nothing", f.name));
            assert!(d.pixels.iter().any(|p| p[3] > 0.9), "{}", f.name);
        }
        // An unknown font is the default one.
        let a = draw(&Text { value: "Hi".into(), font: "C:/nowhere.ttf".into(), ..Default::default() }, 300).unwrap();
        let b = draw(&Text { value: "Hi".into(), ..Default::default() }, 300).unwrap();
        assert_eq!((a.width, a.height), (b.width, b.height));
        assert!(draw(&Text { value: "  ".into(), ..Default::default() }, 300).is_none());
    }

    #[test]
    fn text_is_placed_by_its_middle_and_sized_by_the_height() {
        let text = Text { value: "Orbiters".into(), center: [0.25, 0.75], ..Default::default() };
        let b = bounds(&text, 1000, 500).unwrap();
        assert!((((b[0] + b[2]) * 0.5) - 0.25).abs() < 1e-4 && (((b[1] + b[3]) * 0.5) - 0.75).abs() < 1e-4, "{b:?}");
        // Twice the picture, twice the letters: the same share of it.
        let big = bounds(&text, 2000, 1000).unwrap();
        assert!(((b[2] - b[0]) - (big[2] - big[0])).abs() < 0.01, "{b:?} {big:?}");
        // Two lines are taller than one and centred under each other.
        let two = bounds(&Text { value: "Orbiters\nN4N0".into(), center: [0.5, 0.5], ..Default::default() }, 1000, 500).unwrap();
        assert!(two[3] - two[1] > (b[3] - b[1]) * 1.6);
        let mut img = Image::filled(200, 100, [0.0, 0.0, 0.0, 1.0]);
        composite(&mut img, &Text { value: "I".into(), center: [0.5, 0.5], style: TextStyle::Clean, size: 0.4, ..Default::default() }, 1.0);
        assert!(img.data[50 * 200 + 100][0] > 0.9, "white letters in the middle");
        assert!(img.data[50 * 200 + 20][0] < 0.01 && img.data[5 * 200 + 100][0] < 0.01);
    }

    #[test]
    fn styles_add_their_edges() {
        let base = Text { value: "O".into(), size: 0.3, color: [255, 255, 255], ..Default::default() };
        let clean = draw(&Text { style: TextStyle::Clean, ..base.clone() }, 400).unwrap();
        let outlined = draw(&Text { style: TextStyle::Outline, ..base.clone() }, 400).unwrap();
        let ink = |d: &Drawn| d.pixels.iter().filter(|p| p[3] > 0.5).count();
        assert!(ink(&outlined) > ink(&clean) * 5 / 4, "{} {}", ink(&outlined), ink(&clean));
        // White letters get a dark outline.
        assert!(outlined.pixels.iter().any(|p| p[3] > 0.9 && p[0] < 0.01));
        let shadow = draw(&Text { style: TextStyle::Shadow, ..base.clone() }, 400).unwrap();
        assert!(shadow.pixels.iter().any(|p| p[3] > 0.2 && p[3] < 0.8 && p[0] < 0.05), "a soft dark shadow");
        let glow = draw(&Text { style: TextStyle::Glow, color: [255, 0, 0], ..base }, 400).unwrap();
        assert!(glow.pixels.iter().any(|p| p[3] > 0.1 && p[3] < 0.9 && p[0] > p[1] * 4.0), "a red glow");
    }
}
