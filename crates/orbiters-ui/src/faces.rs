//! The interface's font, as chosen in Settings, Appearance: the system's (the default, [`crate::fonts`]), one of a few the
//! app brings, or a font installed on this computer.
//!
//! Pages are laid out for Segoe UI's widths and line heights, so a chosen font is fitted to them instead of the pages to it:
//! it is scaled so that its lower case is as tall, and its words no wider, than the default's, and its line heights are
//! set to the default's in the copy that is drawn with ([`with_lines`]; the file itself is never changed). Letters it does
//! not have (Japanese, symbols, icons) come from the default font and then the system's, glyph by glyph, as ever: the
//! chosen font only ever stands first.

use egui::epaint::text::Blob;
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::types::Tag;
use skrifa::raw::TableProvider;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// A font the app brings: variable, its Latin letters (`THIRD_PARTY_NOTICES.md`, Fonts).
pub struct Bundled {
    /// What the settings hold: never changes.
    pub id: &'static str,
    pub name: &'static str,
    bytes: &'static [u8],
}

pub static BUNDLED: [Bundled; 6] = [
    Bundled { id: "inter", name: "Inter", bytes: include_bytes!("../assets/faces/Inter/Inter.ttf") },
    Bundled { id: "atkinson", name: "Atkinson Hyperlegible", bytes: include_bytes!("../assets/faces/AtkinsonHyperlegibleNext/AtkinsonHyperlegibleNext.ttf") },
    Bundled { id: "nunito", name: "Nunito", bytes: include_bytes!("../assets/faces/Nunito/Nunito.ttf") },
    Bundled { id: "montserrat", name: "Montserrat", bytes: include_bytes!("../assets/faces/Montserrat/Montserrat.ttf") },
    Bundled { id: "roboto-slab", name: "Roboto Slab", bytes: include_bytes!("../assets/faces/RobotoSlab/RobotoSlab.ttf") },
    Bundled { id: "jetbrains-mono", name: "JetBrains Mono", bytes: include_bytes!("../assets/faces/JetBrainsMono/JetBrainsMono.ttf") },
];

/// How an installed font is written in the settings, before its family's name.
pub const INSTALLED: &str = "installed:";

/// A font family installed on this computer, its files by weight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub family: String,
    files: Vec<File>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct File {
    path: PathBuf,
    index: u32,
    weight: u16,
    variable: bool,
}

/// A font ready to draw with: a file for every weight, or one for each of 400, 500, 600 and 700.
#[derive(Clone)]
pub(crate) enum Sans {
    Variable(Blob, u32),
    Static([(Blob, u32); 4]),
}

#[derive(Clone)]
pub(crate) struct Chosen {
    pub(crate) key: String,
    pub(crate) sans: Sans,
    pub(crate) scale: f32,
}

static CHOSEN: Mutex<Option<Arc<Chosen>>> = Mutex::new(None);
/// The last choice asked for: an installed font being read when another is chosen is dropped.
static ASKED: Mutex<String> = Mutex::new(String::new());
static GENERATION: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static HERE: RefCell<Option<Option<Arc<Chosen>>>> = const { RefCell::new(None) };
}

/// The font to draw with now: none, the default.
pub(crate) fn chosen() -> Option<Arc<Chosen>> {
    HERE.with(|h| h.borrow().clone()).unwrap_or_else(|| CHOSEN.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

/// The key of the font drawn with (empty: the default).
pub fn key() -> String {
    chosen().map(|c| c.key.clone()).unwrap_or_default()
}

/// What [`key`] may have changed with: the process's generation, and whether this thread has a font of its own.
pub(crate) fn mark() -> u32 {
    generation() * 2 + u32::from(HERE.with(|h| h.borrow().is_some()))
}

/// Counts the changes of font.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// The process draws in the font `key` from now on: empty (or one that is not there) the default's, a bundled font's id, or
/// [`INSTALLED`] and a family. An installed one is found and read on a thread of its own; `changed` is called once it is
/// what is drawn with (or at once).
pub fn choose(key: &str, changed: impl FnOnce() + Send + 'static) {
    {
        let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
        if *asked == key {
            return;
        }
        *asked = key.to_string();
    }
    if let Some(family) = key.strip_prefix(INSTALLED) {
        let (key, family) = (key.to_string(), family.to_string());
        let read = move || {
            let font = installed().iter().find(|f| f.family == family).and_then(|f| f.load(&key));
            if *ASKED.lock().unwrap_or_else(|e| e.into_inner()) == key {
                set(font);
                changed();
            }
        };
        let _ = std::thread::Builder::new().name("interface font".into()).spawn(read); // i18n: not shown (a thread's name)
    } else {
        set(BUNDLED.iter().find(|b| b.id == key).and_then(Bundled::load));
        changed();
    }
}

fn set(font: Option<Chosen>) {
    *CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) = font.map(Arc::new);
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Runs `f` with the bundled font `id` (empty: the default) chosen on this thread alone: contexts made in it draw with it.
pub fn in_face<R>(id: &str, f: impl FnOnce() -> R) -> R {
    in_chosen(BUNDLED.iter().find(|b| b.id == id).and_then(Bundled::load), f)
}

fn in_chosen<R>(font: Option<Chosen>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Option<Arc<Chosen>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            HERE.with(|h| *h.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(HERE.with(|h| h.borrow_mut().replace(font.map(Arc::new))));
    f()
}

impl Bundled {
    fn load(&self) -> Option<Chosen> {
        let scale = fit(self.bytes, 0);
        Some(Chosen { key: self.id.into(), sans: Sans::Variable(with_lines(self.bytes, 0, scale)?, 0), scale })
    }

    /// Its file, for a specimen of it.
    pub fn bytes(&self) -> &'static [u8] {
        self.bytes
    }
}

impl Installed {
    /// The file of its regular weight (and its place in a collection), for a specimen of it.
    pub fn regular(&self) -> Option<(&Path, u32)> {
        self.nearest(400).map(|f| (f.path.as_path(), f.index))
    }

    /// The file for a weight: a variable one, the weight's own, else the nearest heavier, then lighter.
    fn nearest(&self, weight: u16) -> Option<&File> {
        let file = |f: &&File| if f.variable { 0 } else if f.weight >= weight { f.weight - weight } else { 1000 + (weight - f.weight) };
        self.files.iter().min_by_key(file)
    }

    fn load(&self, key: &str) -> Option<Chosen> {
        let regular = self.nearest(400)?;
        let bytes = read(&regular.path)?;
        let scale = fit(bytes.as_ref().as_ref(), regular.index);
        let lined = |f: &File| Some((with_lines(read(&f.path)?.as_ref().as_ref(), f.index, scale)?, f.index));
        let sans = if regular.variable {
            let (blob, index) = lined(regular)?;
            Sans::Variable(blob, index)
        } else {
            Sans::Static([lined(regular)?, lined(self.nearest(500)?)?, lined(self.nearest(600)?)?, lined(self.nearest(700)?)?])
        };
        Some(Chosen { key: key.into(), sans, scale })
    }
}

fn read(path: &Path) -> Option<Blob> {
    let bytes = std::fs::read(path).ok()?;
    Some(Arc::new(bytes))
}

/// The font families installed on this computer that have Latin letters, by name: read once, from every font file's
/// tables. Call it off the interface thread.
pub fn installed() -> &'static [Installed] {
    static FOUND: OnceLock<Vec<Installed>> = OnceLock::new();
    FOUND.get_or_init(|| installed_in(&crate::photo::text::font_dirs()))
}

/// The families of the font files under `dirs`.
pub fn installed_in(dirs: &[PathBuf]) -> Vec<Installed> {
    let mut files = Vec::new();
    for dir in dirs {
        crate::photo::text::walk(dir, 0, &mut files);
    }
    let mut families: BTreeMap<String, Installed> = BTreeMap::new();
    for path in files {
        let Some((family, files)) = family_of(&path) else { continue };
        families.entry(family.to_lowercase()).or_insert_with(|| Installed { family, files: Vec::new() }).files.extend(files);
    }
    families.into_values().filter(|f| f.files.iter().any(|f| f.variable || (300..=600).contains(&f.weight))).collect()
}

/// A font file's family and its upright faces of it, when it has Latin letters.
fn family_of(path: &Path) -> Option<(String, Vec<File>)> {
    // A font being installed or a broken file is no reason to look at it for long.
    let len = std::fs::metadata(path).ok()?.len();
    if !(1024..=64 << 20).contains(&len) {
        return None;
    }
    let data = crate::photo::text::map(path)?;
    let bytes = data.as_ref().as_ref();
    let mut family: Option<String> = None;
    let mut files = Vec::new();
    // A collection's faces are few; only the first family's are kept.
    for index in 0..8u32 {
        let Ok(font) = FontRef::from_index(bytes, index) else { break };
        // Symbol fonts have no letters; some map them to pictures, under the symbol bit.
        if !"AaEeNnOoTt019".chars().all(|c| font.charmap().map(c).is_some_and(|g| g.to_u32() != 0)) || font.charmap().is_symbol() {
            continue;
        }
        let attributes = font.attributes();
        if attributes.style != skrifa::attribute::Style::Normal || attributes.stretch.ratio() < 0.95 || attributes.stretch.ratio() > 1.05 {
            continue;
        }
        let name = |id| font.localized_strings(id).english_or_first().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
        let Some(this) = name(StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| name(StringId::FAMILY_NAME)) else { continue };
        let this = this.trim().to_string();
        // Windows' vertical faces, and names a list could not show.
        if this.starts_with('@') || this.chars().any(char::is_control) || this.chars().count() > 64 {
            continue;
        }
        if family.get_or_insert_with(|| this.clone()) != &this {
            continue;
        }
        let variable = font.axes().iter().any(|a| a.tag() == Tag::new(b"wght") && a.max_value() >= 600.0);
        files.push(File { path: path.to_path_buf(), index, weight: attributes.weight.value() as u16, variable });
    }
    family.filter(|_| !files.is_empty()).map(|f| (f, files))
}

/// How tall the lower case and how wide the letters of a font are, as shares of its em.
fn size_of(bytes: &[u8], index: u32) -> Option<(f32, f32)> {
    let font = FontRef::from_index(bytes, index).ok()?;
    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    let em = f32::from(metrics.units_per_em).max(1.0);
    let widths = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    // The letters as often as words of the interface's languages have them.
    const SAMPLE: &str = "etaoinsrhldcumfpgwybvkxjqzEACS e a i o"; // i18n: not shown (letters to measure)
    let wide: f32 = SAMPLE.chars().filter_map(|c| font.charmap().map(c)).filter_map(|g| widths.advance_width(g)).sum();
    let x = metrics.x_height.filter(|x| *x > 0.0).unwrap_or(em * 0.5) / em;
    Some((x, wide / em / SAMPLE.chars().count() as f32))
}

/// The scale that brings a font to the default's sizes: as tall, unless its words would be wider.
fn fit(bytes: &[u8], index: u32) -> f32 {
    let Some((x, wide)) = size_of(bytes, index) else { return 1.0 };
    let (want_x, want_wide) = crate::fonts::default_sizes(size_of).unwrap_or((0.5, 0.5));
    (want_x / x.max(0.2)).min(want_wide * 1.02 / wide.max(0.2)).clamp(0.8, 1.15)
}

/// The default's line: Segoe UI's ascent and descent, as shares of the em.
const LINE: (f32, f32) = (2210.0 / 2048.0, 514.0 / 2048.0);

/// A copy of a font with the default's line heights once drawn at `scale`: its horizontal header and its OS/2 metrics
/// rewritten, nothing else.
fn with_lines(bytes: &[u8], index: u32, scale: f32) -> Option<Blob> {
    let font = FontRef::from_index(bytes, index).ok()?;
    let em = f32::from(font.head().ok()?.units_per_em());
    let up = (LINE.0 * em / scale).round().clamp(1.0, 32000.0) as i16;
    let down = (LINE.1 * em / scale).round().clamp(1.0, 32000.0) as i16;
    let at = |tag: &[u8; 4]| {
        let record = font.table_directory().table_records().iter().find(|r| r.tag() == Tag::new(tag))?;
        Some((record.offset() as usize, record.length() as usize))
    };
    let (hhea, hhea_len) = at(b"hhea")?;
    let mut copy = bytes.to_vec();
    let mut put = |at: usize, v: i16| {
        if let Some(to) = copy.get_mut(at..at + 2) {
            to.copy_from_slice(&v.to_be_bytes());
        }
    };
    if hhea_len >= 10 {
        put(hhea + 4, up);
        put(hhea + 6, -down);
        put(hhea + 8, 0);
    }
    if let Some((os2, len)) = at(b"OS/2").filter(|(_, len)| *len >= 78) {
        let _ = len;
        put(os2 + 68, up);
        put(os2 + 70, -down);
        put(os2 + 72, 0);
        put(os2 + 74, up);
        put(os2 + 76, down);
    }
    Some(Arc::new(copy))
}

/// `text` in a font, white on nothing, `px` pixels to the em and no taller than a line of it: what a list of fonts shows of one.
/// `None` when it draws nothing. A variable font at its regular weight.
pub fn specimen(bytes: &[u8], index: u32, text: &str, px: f32) -> Option<egui::ColorImage> {
    use skrifa::outline::DrawSettings;
    let font = FontRef::from_index(bytes, index).ok()?;
    let location = font.axes().location([("wght", 400.0)]);
    let size = Size::new(px);
    let (outlines, widths, map) = (font.outline_glyphs(), font.glyph_metrics(size, &location), font.charmap());
    let mut pen = Pen { path: tiny_skia::PathBuilder::new(), x: 2.0, y: px * 1.05 };
    for c in text.chars() {
        let glyph = map.map(c)?;
        if let Some(outline) = outlines.get(glyph) {
            outline.draw(DrawSettings::unhinted(size, &location), &mut pen).ok()?;
        }
        pen.x += widths.advance_width(glyph).unwrap_or(px * 0.5);
    }
    let (w, h) = ((pen.x + 2.0).ceil().clamp(1.0, 2048.0) as usize, (px * 1.5).ceil().clamp(1.0, 512.0) as usize);
    let path = pen.path.finish()?;
    let mut mask = tiny_skia::Mask::new(w as u32, h as u32)?;
    mask.fill_path(&path, tiny_skia::FillRule::Winding, true, tiny_skia::Transform::identity());
    if mask.data().iter().all(|v| *v < 8) {
        return None;
    }
    let pixels = mask.data().iter().map(|v| egui::Color32::from_white_alpha(*v)).collect();
    Some(egui::ColorImage::new([w, h], pixels))
}

struct Pen {
    path: tiny_skia::PathBuilder,
    x: f32,
    y: f32,
}

impl skrifa::outline::OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x, self.y - y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x, self.y - y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(self.x + cx, self.y - cy, self.x + x, self.y - y);
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        self.path.cubic_to(self.x + ax, self.y - ay, self.x + bx, self.y - by, self.x + x, self.y - y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_font_reads_is_variable_and_has_the_interface_s_letters() {
        for b in &BUNDLED {
            let font = FontRef::from_index(b.bytes, 0).unwrap_or_else(|e| panic!("{}: {e}", b.name));
            let axis = font.axes().iter().find(|a| a.tag() == Tag::new(b"wght")).unwrap_or_else(|| panic!("{} has weights", b.name));
            assert!(axis.min_value() <= 400.0 && axis.max_value() >= 700.0, "{}", b.name);
            // English, French, Spanish, Brazilian Portuguese, and what the texts use besides.
            for c in "ABCXYZabcxyz0123456789ÀÂÆÇÉÈÊËÎÏÔŒÙÛÜŸàâæçéèêëîïôœùûüÿÁÍÑÓÚáíñóú¿¡ÃÕãõ«»“”‘’–—…·€%&@#×".chars() {
                assert!(font.charmap().map(c).is_some_and(|g| g.to_u32() != 0), "{} has no {c}", b.name);
            }
            // None has Japanese: it comes from the system's font, letter by letter.
            assert!(font.charmap().map('あ').is_none(), "{}", b.name);
        }
        assert_eq!(BUNDLED.iter().map(|b| b.id).collect::<std::collections::HashSet<_>>().len(), BUNDLED.len());
    }

    #[test]
    fn a_chosen_font_has_the_default_s_lines_and_is_no_wider() {
        for b in &BUNDLED {
            let chosen = b.load().expect(b.name);
            assert!(specimen(b.bytes, 0, "Your world, in your words.", 40.0).is_some_and(|i| i.size[0] > 200), "{}", b.name);
            assert!((0.8..=1.15).contains(&chosen.scale), "{} {}", b.name, chosen.scale);
            let Sans::Variable(blob, 0) = &chosen.sans else { panic!("{} is variable", b.name) };
            let font = FontRef::from_index(blob.as_ref().as_ref(), 0).expect(b.name);
            let m = font.metrics(Size::new(100.0 * chosen.scale), LocationRef::default());
            assert!((m.ascent - LINE.0 * 100.0).abs() < 0.2 && (m.descent + LINE.1 * 100.0).abs() < 0.2 && m.leading == 0.0, "{}: {m:?}", b.name);
            let wide = size_of(b.bytes, 0).expect(b.name).1 * chosen.scale;
            if let Some((_, default)) = crate::fonts::default_sizes(size_of) {
                // A fixed-width font cannot be brought all the way.
                let most = if b.id == "jetbrains-mono" { 1.12 } else { 1.03 };
                assert!(wide <= default * most, "{}: {wide} against {default}", b.name);
            }
        }
    }

    /// A row of text is as tall in every font, and letters a font lacks are still drawn.
    #[test]
    fn rows_keep_their_height_and_no_letter_becomes_a_box() {
        let rows = |id: &str| {
            in_face(id, || {
                let ctx = egui::Context::default();
                crate::install(&ctx);
                let mut heights = Vec::new();
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    for (size, weight) in [(11.5, 400.0), (13.5, 500.0), (20.0, 600.0)] {
                        let g = ui.painter().layout_job(crate::theme::Txt::new("Café déjà vu ¿Qué? São", size).weight(weight).job(f32::INFINITY));
                        heights.push(g.size().y);
                    }
                    let has = ui.ctx().fonts_mut(|f| f.has_glyphs(&crate::theme::font(13.5, 400.0), "Orbiters 設定 外観 フォント ★"));
                    // On a computer without a Japanese font nothing can draw it; here it is drawn in every font or in none.
                    heights.push(if has { 1.0 } else { 0.0 });
                });
                out.textures_delta.clear();
                heights
            })
        };
        let default = rows("");
        for b in &BUNDLED {
            let h = rows(b.id);
            for (a, z) in h.iter().zip(&default) {
                assert!((a - z).abs() <= 1.0, "{}: {h:?} against {default:?}", b.name);
            }
            assert_eq!(h.last(), default.last(), "{}: letters it lacks", b.name);
        }
    }

    #[test]
    fn installed_fonts_are_families_of_upright_latin_faces() {
        // The bundled files stand in for a computer's fonts: a real folder is never read here.
        let dir = std::env::temp_dir().join(format!("orbiters-faces-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for b in &BUNDLED[..2] {
            std::fs::write(dir.join(format!("{}.ttf", b.id)), b.bytes).unwrap();
        }
        std::fs::write(dir.join("broken.ttf"), vec![7u8; 4096]).unwrap();
        std::fs::write(dir.join("icons.ttf"), crate::fonts::PHOSPHOR).unwrap();
        std::fs::write(dir.join("notes.txt"), "no font").unwrap();
        let found = installed_in(std::slice::from_ref(&dir));
        assert_eq!(found.len(), 2, "{:?}", found.iter().map(|f| &f.family).collect::<Vec<_>>());
        let font = found[1].load("installed:x").expect("it loads");
        assert!(matches!(font.sans, Sans::Variable(..)) && font.key == "installed:x");
        std::fs::remove_dir_all(&dir).ok();
        assert!(installed_in(&[dir]).is_empty(), "a folder that is not there");
    }

    #[test]
    fn a_weight_takes_its_own_file_or_the_nearest_heavier() {
        let f = |weight| File { path: format!("{weight}.ttf").into(), index: 0, weight, variable: false };
        let font = Installed { family: "Test".into(), files: vec![f(300), f(400), f(600), f(900)] };
        assert_eq!([400, 500, 600, 700].map(|w| font.nearest(w).unwrap().weight), [400, 600, 600, 900]);
        let light = Installed { family: "Test".into(), files: vec![f(300)] };
        assert_eq!(light.nearest(700).unwrap().weight, 300);
    }
}
