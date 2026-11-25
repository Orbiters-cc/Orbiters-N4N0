//! Fonts. Windows 11 has Segoe UI Variable: every weight through its `wght` axis and optical sizes through
//! `opsz`, so headings get the Display cut and labels the Text cut. macOS has no Segoe UI, so the app brings its own:
//! Orbiters Sans, Selawik (Microsoft's open font with Segoe UI's letter widths) as a variable font with Segoe UI's
//! line heights ([`SANS`]), and the same Cascadia Mono as Windows 11, so a page lays out as it does on Windows.
//! Elsewhere the first common sans family found (static weights when there is no variable file). Cascadia Mono or
//! Consolas for IDs, and the Phosphor icons.
//! Letters none of them has (Japanese, Chinese, Korean, Thai, Arabic… in world and friend names) come from the
//! system's own font for their script, opened the first time such a letter shows up. Every file is opened once per
//! process, mapped rather than read, and shared by all its egui contexts (the VR overlay runs five).

use egui::epaint::text::{Blob, FontTweak, VariationCoords};
use egui::{FallbackRequest, FontData, FontDefinitions, FontFamily, FontInsert, FontPriority, InsertFontFamily};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Phosphor Icons (MIT), regular and fill.
pub const PHOSPHOR: &[u8] = include_bytes!("../assets/Phosphor.ttf");
pub const PHOSPHOR_FILL: &[u8] = include_bytes!("../assets/Phosphor-Fill.ttf");

/// The text font on macOS and Linux (a Linux computer's own differ from one to the next): `scripts/fonts/orbiters_sans.py` makes it from Selawik's sources (SIL Open Font License).
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SANS: &[u8] = include_bytes!("../assets/OrbitersSans-VF.ttf");
/// Cascadia Mono (SIL Open Font License), as Windows 11 ships it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MONO: &[u8] = include_bytes!("../assets/CascadiaMono.ttf");

const WEIGHTS: [u16; 4] = [400, 500, 600, 700];

/// Set when the text font is variable: exact weights and optical sizes then come from [`coords`].
static VARIABLE: AtomicBool = AtomicBool::new(false);

/// The text families, one per weight of [`WEIGHTS`]. Made once: every run of text names one, and a family is an
/// `Arc<str>` that was formatted and allocated each time.
fn families() -> &'static [FontFamily; 4] {
    static FAMILIES: OnceLock<[FontFamily; 4]> = OnceLock::new();
    FAMILIES.get_or_init(|| [FontFamily::Proportional, FontFamily::Name("w500".into()), FontFamily::Name("w600".into()), FontFamily::Name("w700".into())])
}

/// Family for a weight (400, 500, 600 or 700, whichever is nearest).
pub fn family(weight: f32) -> FontFamily {
    let i = (0..WEIGHTS.len()).min_by_key(|&i| (WEIGHTS[i] as f32 - weight).abs() as u32).unwrap_or(0);
    families()[i].clone()
}

pub fn icons() -> FontFamily {
    static ICONS: OnceLock<FontFamily> = OnceLock::new();
    ICONS.get_or_init(|| FontFamily::Name("icons".into())).clone()
}

pub fn icons_fill() -> FontFamily {
    static ICONS_FILL: OnceLock<FontFamily> = OnceLock::new();
    ICONS_FILL.get_or_init(|| FontFamily::Name("icons-fill".into())).clone()
}

/// Variation coordinates for a text run: the exact weight and an optical size matching the text size.
pub fn coords(size: f32, weight: f32) -> VariationCoords {
    if VARIABLE.load(Ordering::Relaxed) {
        VariationCoords::new([(b"wght", weight.clamp(300.0, 700.0)), (b"opsz", (size * 0.75).clamp(8.0, 36.0))])
    } else {
        VariationCoords::default()
    }
}

enum Sans {
    Variable(Blob),
    /// Files for 400, 500, 600, 700 (missing weights borrow the nearest heavier one, then lighter).
    Static([Option<Blob>; 4]),
}

/// The interface's fonts as this computer has them, looked for once per process.
struct Found {
    sans: Option<Sans>,
    mono: Option<Blob>,
}

fn found() -> &'static Found {
    static FOUND: OnceLock<Found> = OnceLock::new();
    FOUND.get_or_init(|| Found { sans: find_sans(), mono: find_mono() })
}

/// Loads the fonts into `ctx` (egui's own fonts stay as fallbacks for symbols the system font lacks), and the
/// system's fonts for other scripts as their letters appear.
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let fallback_text = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    defs.font_data.insert("phosphor".into(), FontData::from_static(PHOSPHOR).into());
    defs.font_data.insert("phosphor-fill".into(), FontData::from_static(PHOSPHOR_FILL).into());
    defs.families.insert(icons(), vec!["phosphor".into()]);
    defs.families.insert(icons_fill(), vec!["phosphor-fill".into()]);

    let found = found();
    VARIABLE.store(matches!(found.sans, Some(Sans::Variable(_))), Ordering::Relaxed);
    for (i, w) in WEIGHTS.iter().enumerate() {
        let name = format!("ui-{w}");
        let data = match &found.sans {
            Some(Sans::Variable(bytes)) => {
                Some(FontData::from_blob(bytes.clone(), 0).tweak(FontTweak { coords: VariationCoords::new([(b"wght", *w as f32)]), ..Default::default() }))
            }
            Some(Sans::Static(files)) => {
                let pick = files[i..].iter().chain(files[..i].iter().rev()).flatten().next();
                pick.map(|bytes| FontData::from_blob(bytes.clone(), 0))
            }
            None => None,
        };
        let mut list = Vec::new();
        if let Some(data) = data {
            defs.font_data.insert(name.clone(), data.into());
            list.push(name);
        }
        list.push("phosphor".into());
        list.extend(fallback_text.iter().cloned());
        defs.families.insert(families()[i].clone(), list);
    }

    if let Some(bytes) = &found.mono {
        defs.font_data.insert("mono".into(), FontData::from_blob(bytes.clone(), 0).into());
        let mut list = vec!["mono".to_string()];
        list.extend(fallback_mono);
        defs.families.insert(FontFamily::Monospace, list);
    }
    ctx.set_fonts(defs);

    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(LANGUAGE_FONTS), preferred()));
    // Once per context, whatever calls this again.
    let added = ctx.data_mut(|d| std::mem::replace(d.get_temp_mut_or_default::<bool>(egui::Id::unique("orbiters-fonts-provider")), true));
    if !added {
        ctx.add_font_provider(Arc::new(SystemFallback { cjk: cjk() }));
    }
}

/// A font file, opened once for the life of the process and shared by every context: mapped rather than read (a CJK
/// font is 12 to 20 MB, of which a few names touch a few pages), read when it cannot be mapped.
fn open(path: &Path) -> Option<Blob> {
    static OPEN: OnceLock<Mutex<HashMap<PathBuf, Option<Blob>>>> = OnceLock::new();
    let mut open = OPEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    open.entry(path.to_path_buf())
        .or_insert_with(|| {
            let file = std::fs::File::open(path).ok()?;
            // SAFETY: a font file is not changed while it is in use (Windows does not replace an open font file, and macOS's
            // sealed system volume holds its own), and egui only ever reads the bytes.
            match unsafe { memmap2::Mmap::map(&file) } {
                Ok(map) => Some(Arc::new(map) as Blob),
                Err(_) => std::fs::read(path).ok().map(|bytes| Arc::new(bytes) as Blob),
            }
        })
        .clone()
}

#[cfg(windows)]
fn windows_fonts() -> PathBuf {
    std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts")
}

#[cfg(windows)]
fn find_sans() -> Option<Sans> {
    let dir = windows_fonts();
    if let Some(bytes) = open(&dir.join("SegUIVar.ttf")) {
        return Some(Sans::Variable(bytes));
    }
    // Windows 10: static Segoe UI, which has no medium cut.
    let regular = open(&dir.join("segoeui.ttf"))?;
    Some(Sans::Static([Some(regular), None, open(&dir.join("seguisb.ttf")), open(&dir.join("segoeuib.ttf"))]))
}

#[cfg(windows)]
fn find_mono() -> Option<Blob> {
    let dir = windows_fonts();
    open(&dir.join("CascadiaMono.ttf")).or_else(|| open(&dir.join("consola.ttf")))
}

/// A system font file by its name in the fonts folder.
#[cfg(windows)]
fn system_file(name: &str) -> Option<Blob> {
    open(&windows_fonts().join(name))
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs
}

/// The system's fonts (with their supplement, and the folder PingFang lives in), then those the person added.
#[cfg(target_os = "macos")]
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/System/Library/Fonts",
        "/System/Library/PrivateFrameworks/FontServices.framework/Versions/A/Resources/Reserved",
        "/Library/Fonts",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join("Library/Fonts"));
    }
    dirs
}

/// First file (in `names` order) found under the font folders (walked once per process).
#[cfg(all(not(windows), not(target_os = "macos")))]
fn find_file(names: &[&str]) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 5 {
                    walk(&p, depth + 1, out);
                }
            } else {
                out.push(p);
            }
        }
    }
    static FILES: OnceLock<Vec<PathBuf>> = OnceLock::new();
    let files = FILES.get_or_init(|| {
        let mut files = Vec::new();
        for d in font_dirs() {
            walk(&d, 0, &mut files);
        }
        files
    });
    names.iter().find_map(|n| files.iter().find(|p| p.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n))).cloned())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn find_sans() -> Option<Sans> {
    Some(Sans::Variable(Arc::new(SANS) as Blob))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn find_mono() -> Option<Blob> {
    Some(Arc::new(MONO) as Blob)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn find_sans() -> Option<Sans> {
    let variable =
        ["InterVariable.ttf", "Inter[opsz,wght].ttf", "Inter-VariableFont_opsz,wght.ttf", "NotoSans[wdth,wght].ttf", "NotoSans-VariableFont_wdth,wght.ttf"];
    if let Some(bytes) = find_file(&variable).and_then(|p| open(&p)) {
        return Some(Sans::Variable(bytes));
    }
    let families: [[&str; 4]; 3] = [
        ["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf", "Inter-Bold.ttf"],
        ["NotoSans-Regular.ttf", "NotoSans-Medium.ttf", "NotoSans-SemiBold.ttf", "NotoSans-Bold.ttf"],
        ["DejaVuSans.ttf", "", "", "DejaVuSans-Bold.ttf"],
    ];
    for names in families {
        if let Some(regular) = find_file(&[names[0]]).and_then(|p| open(&p)) {
            let other = |n: &str| if n.is_empty() { None } else { find_file(&[n]).and_then(|p| open(&p)) };
            return Some(Sans::Static([Some(regular), other(names[1]), other(names[2]), other(names[3])]));
        }
    }
    None
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn find_mono() -> Option<Blob> {
    find_file(&["CascadiaMono.ttf", "JetBrainsMono-Regular.ttf", "DejaVuSansMono.ttf", "NotoSansMono-Regular.ttf", "LiberationMono-Regular.ttf"])
        .and_then(|p| open(&p))
}

/// A system font file by its file name. The file system finds it however its name is normalized (Hiragino's names
/// are decomposed on disk), which comparing the names listed in a folder would not.
#[cfg(target_os = "macos")]
fn system_file(name: &str) -> Option<Blob> {
    let mut dirs = font_dirs();
    dirs.insert(1, PathBuf::from("/System/Library/Fonts/Supplemental"));
    dirs.into_iter().map(|dir| dir.join(name)).find(|path| path.is_file()).and_then(|path| open(&path))
}

/// A system font file by its file name, wherever it is under the font folders.
#[cfg(all(not(windows), not(target_os = "macos")))]
fn system_file(name: &str) -> Option<Blob> {
    find_file(&[name]).and_then(|p| open(&p))
}

/// Which CJK font comes first for kana and Chinese characters: the one of this computer's language. Elsewhere the
/// Simplified Chinese font, which also has kana: a Japanese or Chinese name then reads in one font, not two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Cjk {
    Japanese,
    Simplified,
    Traditional,
    Korean,
}

fn cjk_for_locale(locale: &str) -> Cjk {
    let l = locale.to_ascii_lowercase().replace('_', "-");
    if l.starts_with("ja") {
        Cjk::Japanese
    } else if l.starts_with("ko") {
        Cjk::Korean
    } else if l.starts_with("zh") && ["hant", "-tw", "-hk", "-mo"].iter().any(|t| l.contains(t)) {
        Cjk::Traditional
    } else {
        Cjk::Simplified
    }
}

fn cjk() -> Cjk {
    static CJK: OnceLock<Cjk> = OnceLock::new();
    *CJK.get_or_init(|| cjk_for_locale(&user_locale()))
}

/// The CJK font a text's language asked for over the locale's ([`prefer_language`]): 0 none, else 1 + the [`Cjk`].
static PREFERRED: AtomicU8 = AtomicU8::new(0);

/// Texts in `language` (a BCP 47 tag: `ja`, `ko`, `zh-TW`) draw kana and Chinese characters in that language's font,
/// whatever this computer's locale: N4N0's menu in Japanese on a French computer gets Yu Gothic (or Meiryo), not
/// Microsoft YaHei, whose kanji are Chinese forms. Any other language goes back to the locale's choice. Letters a
/// context already drew keep the face it found for them; call this before drawing.
pub fn prefer_language(language: &str) {
    PREFERRED.store(preference(language), Ordering::Relaxed);
}

fn preference(language: &str) -> u8 {
    language_cjk(language).map_or(0, |c| 1 + c as u8)
}

thread_local! {
    /// What [`in_preferred_language`] asks for on this thread, over [`PREFERRED`].
    static PREFERRED_HERE: std::cell::Cell<Option<u8>> = const { std::cell::Cell::new(None) };
}

/// The preference that holds on this thread.
fn preferred() -> u8 {
    PREFERRED_HERE.with(std::cell::Cell::get).unwrap_or_else(|| PREFERRED.load(Ordering::Relaxed))
}

/// Runs `f` with texts drawn as [`prefer_language`] would have them, on this thread alone: tests side by side in one process do not see
/// each other's. One that set the whole process's to Japanese for a while had every context painting on another thread find its faces
/// anew halfway through what it drew, and the parts it compared differed (VR1115 flaky).
pub fn in_preferred_language<R>(language: &str, f: impl FnOnce() -> R) -> R {
    let was = PREFERRED_HERE.with(|h| h.replace(Some(preference(language))));
    struct Restore(Option<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PREFERRED_HERE.with(|h| h.set(self.0));
        }
    }
    let _restore = Restore(was);
    f()
}

/// Where a context notes which language its faces were found for.
const LANGUAGE_FONTS: &str = "orbiters-fonts-language";

/// To call each frame of a context that can live through a change of language. A face found once stays in a family's fallbacks and is what egui then uses for all the letters it has:
/// kanji already drawn on an English or French interface (a world's name) would come from Microsoft YaHei, in Chinese forms, for all the Japanese that follows. So when the
/// language wants another font the context starts over, with no faces found.
pub fn follow_language(ctx: &egui::Context) {
    let now = preferred();
    let was = ctx.data(|d| d.get_temp::<u8>(egui::Id::unique(LANGUAGE_FONTS)));
    if was.is_some_and(|was| was != now) {
        // The fonts are the same, so setting them again does nothing: new providers make egui build its fonts anew, and
        // forget the faces found so far.
        ctx.set_font_providers(vec![Arc::new(SystemFallback { cjk: cjk() })]);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(LANGUAGE_FONTS), now));
        ctx.request_repaint();
    }
}

/// The CJK font a language's texts want (none for other languages).
fn language_cjk(language: &str) -> Option<Cjk> {
    let l = language.to_ascii_lowercase();
    (l.starts_with("ja") || l.starts_with("ko") || l.starts_with("zh")).then(|| cjk_for_locale(&l))
}

/// The CJK font to try first now: the one a language asked for, else the locale's.
fn preferred_cjk(locale: Cjk) -> Cjk {
    match preferred() {
        1 => Cjk::Japanese,
        2 => Cjk::Simplified,
        3 => Cjk::Traditional,
        4 => Cjk::Korean,
        _ => locale,
    }
}

/// The user's locale name ("ja-JP").
#[cfg(windows)]
pub fn user_locale() -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, length: i32) -> i32;
    }
    let mut name = [0u16; 85];
    // SAFETY: the buffer holds LOCALE_NAME_MAX_LENGTH (85) UTF-16 units, the length the call is given.
    let n = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&name[..(n - 1) as usize])
}

/// The user's locale name, from the system (an app started from the Finder has no `LANG`).
#[cfg(target_os = "macos")]
pub fn user_locale() -> String {
    use std::ffi::{c_char, c_void, CStr};
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFLocaleCopyCurrent() -> *const c_void;
        fn CFLocaleGetIdentifier(locale: *const c_void) -> *const c_void;
        fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(object: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;
    let mut buffer = [0 as c_char; 128];
    // SAFETY: the locale is ours to release; its identifier is borrowed from it for the copy into a buffer of the size given.
    unsafe {
        let locale = CFLocaleCopyCurrent();
        if locale.is_null() {
            return String::new();
        }
        let copied = CFStringGetCString(CFLocaleGetIdentifier(locale), buffer.as_mut_ptr(), buffer.len() as isize, UTF8) != 0;
        let name = if copied { CStr::from_ptr(buffer.as_ptr()).to_string_lossy().replace('_', "-") } else { String::new() };
        CFRelease(locale);
        name
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub fn user_locale() -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default()
}

/// The scripts the system fallback knows a font for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Hangul,
    Kana,
    /// Chinese characters, CJK punctuation and full-width forms.
    Han,
    Thai,
    Lao,
    Khmer,
    Arabic,
    Hebrew,
    Armenian,
    Georgian,
    /// Devanagari to Sinhala.
    Indic,
    Ethiopic,
    Myanmar,
    Tibetan,
    Mongolian,
    Cherokee,
    Yi,
    /// Anything else the interface's fonts lack (rare Latin, Greek or Cyrillic letters, symbols).
    Other,
}

fn script(c: char) -> Script {
    match c as u32 {
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xFFA0..=0xFFDC => Script::Hangul,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF65..=0xFF9F | 0x1B000..=0x1B16F => Script::Kana,
        0x2E80..=0x2FDF
        | 0x3000..=0x303F
        | 0x3100..=0x312F
        | 0x31A0..=0x31EF
        | 0x3200..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF64
        | 0xFFE0..=0xFFEF
        | 0x20000..=0x3FFFF => Script::Han,
        0x0E00..=0x0E7F => Script::Thai,
        0x0E80..=0x0EFF => Script::Lao,
        0x1780..=0x17FF | 0x19E0..=0x19FF => Script::Khmer,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => Script::Hebrew,
        0x0530..=0x058F => Script::Armenian,
        0x10A0..=0x10FF | 0x1C90..=0x1CBF | 0x2D00..=0x2D2F => Script::Georgian,
        0x0900..=0x0DFF | 0xA8E0..=0xA8FF => Script::Indic,
        0x1200..=0x139F | 0x2D80..=0x2DDF | 0xAB00..=0xAB2F => Script::Ethiopic,
        0x1000..=0x109F | 0xA9E0..=0xA9FF | 0xAA60..=0xAA7F => Script::Myanmar,
        0x0F00..=0x0FFF => Script::Tibetan,
        0x1800..=0x18AF => Script::Mongolian,
        0x13A0..=0x13FF | 0xAB70..=0xABBF => Script::Cherokee,
        0xA000..=0xA4CF => Script::Yi,
        _ => Script::Other,
    }
}

/// A face of a system font file: its file name and its index in a collection.
type Face = (&'static str, u32);

/// A font family as its weights: regular (and medium), semibold, bold (the regular again when it has no other).
type Weights = [Face; 3];

#[cfg(windows)]
mod table {
    use super::Weights;
    // The UI cuts of the collections (Yu Gothic UI, Microsoft YaHei UI, Microsoft JhengHei UI), then older fonts.
    pub const JAPANESE: &[Weights] = &[
        [("YuGothM.ttc", 1), ("YuGothB.ttc", 2), ("YuGothB.ttc", 1)],
        // Meiryo UI, where Windows' optional Japanese fonts are installed.
        [("meiryo.ttc", 2), ("meiryob.ttc", 2), ("meiryob.ttc", 2)],
        [("msgothic.ttc", 1); 3],
    ];
    pub const SIMPLIFIED: &[Weights] = &[[("msyh.ttc", 1), ("msyhbd.ttc", 1), ("msyhbd.ttc", 1)], [("simsun.ttc", 0); 3]];
    pub const TRADITIONAL: &[Weights] = &[[("msjh.ttc", 1), ("msjhbd.ttc", 1), ("msjhbd.ttc", 1)], [("mingliu.ttc", 0); 3]];
    pub const KOREAN: &[Weights] = &[[("malgun.ttf", 0), ("malgunbd.ttf", 0), ("malgunbd.ttf", 0)], [("gulim.ttc", 0); 3]];
    pub const THAI: &[Weights] = &[
        [("LeelawUI.ttf", 0), ("LeelaUIb.ttf", 0), ("LeelaUIb.ttf", 0)],
        [("leelawad.ttf", 0), ("leelawdb.ttf", 0), ("leelawdb.ttf", 0)],
        [("tahoma.ttf", 0), ("tahomabd.ttf", 0), ("tahomabd.ttf", 0)],
    ];
    pub const LAO: &[Weights] = &[[("LeelawUI.ttf", 0), ("LeelaUIb.ttf", 0), ("LeelaUIb.ttf", 0)], [("LaoUI.ttf", 0), ("LaoUIb.ttf", 0), ("LaoUIb.ttf", 0)]];
    pub const KHMER: &[Weights] =
        &[[("LeelawUI.ttf", 0), ("LeelaUIb.ttf", 0), ("LeelaUIb.ttf", 0)], [("KhmerUI.ttf", 0), ("KhmerUIb.ttf", 0), ("KhmerUIb.ttf", 0)]];
    /// Static Segoe UI: Arabic, Hebrew, Armenian, Georgian, and every Latin, Greek and Cyrillic letter (Segoe UI
    /// Variable has fewer).
    pub const SEGOE: &[Weights] = &[
        [("segoeui.ttf", 0), ("seguisb.ttf", 0), ("segoeuib.ttf", 0)],
        [("tahoma.ttf", 0), ("tahomabd.ttf", 0), ("tahomabd.ttf", 0)],
        [("arial.ttf", 0), ("arialbd.ttf", 0), ("arialbd.ttf", 0)],
    ];
    pub const INDIC: &[Weights] = &[[("Nirmala.ttf", 0), ("NirmalaB.ttf", 0), ("NirmalaB.ttf", 0)], [("Nirmala.ttc", 0); 3]]; // i18n: not shown (system font file names)
    pub const ETHIOPIC: &[Weights] = &[[("ebrima.ttf", 0), ("ebrimabd.ttf", 0), ("ebrimabd.ttf", 0)]];
    pub const MYANMAR: &[Weights] = &[[("mmrtext.ttf", 0), ("mmrtextb.ttf", 0), ("mmrtextb.ttf", 0)]];
    pub const TIBETAN: &[Weights] = &[[("himalaya.ttf", 0); 3]];
    pub const MONGOLIAN: &[Weights] = &[[("monbaiti.ttf", 0); 3]];
    pub const CHEROKEE: &[Weights] = &[[("gadugi.ttf", 0), ("gadugib.ttf", 0), ("gadugib.ttf", 0)]];
    pub const YI: &[Weights] = &[[("msyi.ttf", 0); 3]];
    pub const SYMBOLS: &[Weights] = &[[("seguisym.ttf", 0); 3]];
}

#[cfg(target_os = "macos")]
mod table {
    use super::Weights;
    // Hiragino for Japanese as the system does; PingFang UI (its Text cuts) and Heiti for Chinese. PingFang's file holds
    // one weight per face that is not a static file on every macOS, so the bolder columns ask Heiti's Medium when it is missing.
    pub const JAPANESE: &[Weights] = &[[("ヒラギノ角ゴシック W4.ttc", 0), ("ヒラギノ角ゴシック W6.ttc", 0), ("ヒラギノ角ゴシック W7.ttc", 0)], [("PingFangUI.ttc", 4); 3]]; // i18n: not shown (system font file names)
    pub const SIMPLIFIED: &[Weights] =
        &[[("PingFangUI.ttc", 0); 3], [("Hiragino Sans GB.ttc", 0), ("Hiragino Sans GB.ttc", 2), ("Hiragino Sans GB.ttc", 2)], [("STHeiti Medium.ttc", 1); 3]]; // i18n: not shown (system font file names)
    pub const TRADITIONAL: &[Weights] = &[[("PingFangUI.ttc", 1); 3], [("STHeiti Light.ttc", 0), ("STHeiti Medium.ttc", 0), ("STHeiti Medium.ttc", 0)]]; // i18n: not shown (system font file names)
    pub const KOREAN: &[Weights] = &[[("AppleSDGothicNeo.ttc", 0), ("AppleSDGothicNeo.ttc", 4), ("AppleSDGothicNeo.ttc", 6)]];
    pub const THAI: &[Weights] = &[[("ThonburiUI.ttc", 0); 3], [("Thonburi.ttc", 0), ("Thonburi.ttc", 1), ("Thonburi.ttc", 1)]]; // i18n: not shown (system font file names)
    pub const LAO: &[Weights] = &[[("Lao MN.ttc", 0), ("Lao MN.ttc", 1), ("Lao MN.ttc", 1)], [("Lao Sangam MN.ttf", 0); 3]]; // i18n: not shown (system font file names)
    pub const KHMER: &[Weights] = &[[("Khmer MN.ttc", 0), ("Khmer MN.ttc", 1), ("Khmer MN.ttc", 1)], [("Khmer Sangam MN.ttf", 0); 3]]; // i18n: not shown (system font file names)
    /// San Francisco: every Latin, Greek and Cyrillic letter Orbiters Sans lacks, then Arabic, Hebrew, Armenian and
    /// Georgian in the system's own cuts, and Arial Unicode for the rest.
    pub const SEGOE: &[Weights] = &[
        [("SFNS.ttf", 0); 3],
        [("SFArabic.ttf", 0); 3],
        [("SFHebrew.ttf", 0); 3],
        [("SFArmenian.ttf", 0); 3],
        [("SFGeorgian.ttf", 0); 3],
        [("Arial Unicode.ttf", 0); 3], // i18n: not shown (system font file names)
    ];
    pub const INDIC: &[Weights] = &[
        [("Kohinoor.ttc", 0), ("Kohinoor.ttc", 2), ("Kohinoor.ttc", 3)], // i18n: not shown (system font file names)
        // Bangla, Devanagari, Gujarati, Gurmukhi, Kannada, Malayalam, Odia, Tamil, Telugu.
        [("SFIndia.ttc", 0); 3],
        [("SFIndia.ttc", 1); 3],
        [("SFIndia.ttc", 2); 3],
        [("SFIndia.ttc", 3); 3],
        [("SFIndia.ttc", 4); 3],
        [("SFIndia.ttc", 5); 3],
        [("SFIndia.ttc", 6); 3],
        [("SFIndia.ttc", 7); 3],
        [("SFIndia.ttc", 8); 3],
    ];
    pub const ETHIOPIC: &[Weights] = &[[("KefaIII.ttf", 0); 3], [("Kefa.ttc", 0); 3]]; // i18n: not shown (system font file names)
    pub const MYANMAR: &[Weights] = &[[("NotoSansMyanmar.ttc", 6), ("NotoSansMyanmar.ttc", 7), ("NotoSansMyanmar.ttc", 1)]];
    pub const TIBETAN: &[Weights] = &[[("Kailasa.ttc", 0), ("Kailasa.ttc", 1), ("Kailasa.ttc", 1)]]; // i18n: not shown (system font file names)
    pub const MONGOLIAN: &[Weights] = &[[("NotoSansMongolian-Regular.ttf", 0); 3]];
    pub const CHEROKEE: &[Weights] = &[[("PlantagenetCherokee.ttf", 0); 3]];
    pub const YI: &[Weights] = &[[("NotoSansYi-Regular.ttf", 0); 3]];
    pub const SYMBOLS: &[Weights] = &[[("Apple Symbols.ttf", 0); 3], [("Arial Unicode.ttf", 0); 3]]; // i18n: not shown (system font file names)
}

#[cfg(all(not(windows), not(target_os = "macos")))]
mod table {
    use super::Weights;
    // Noto's CJK collection holds Japanese, Korean, Simplified and Traditional Chinese in that order.
    pub const JAPANESE: &[Weights] = &[[("NotoSansCJK-Regular.ttc", 0), ("NotoSansCJK-Bold.ttc", 0), ("NotoSansCJK-Bold.ttc", 0)]];
    pub const KOREAN: &[Weights] = &[[("NotoSansCJK-Regular.ttc", 1), ("NotoSansCJK-Bold.ttc", 1), ("NotoSansCJK-Bold.ttc", 1)]];
    pub const SIMPLIFIED: &[Weights] =
        &[[("NotoSansCJK-Regular.ttc", 2), ("NotoSansCJK-Bold.ttc", 2), ("NotoSansCJK-Bold.ttc", 2)], [("DroidSansFallbackFull.ttf", 0); 3]];
    pub const TRADITIONAL: &[Weights] = &[[("NotoSansCJK-Regular.ttc", 3), ("NotoSansCJK-Bold.ttc", 3), ("NotoSansCJK-Bold.ttc", 3)]];
    pub const THAI: &[Weights] = &[[("NotoSansThai-Regular.ttf", 0), ("NotoSansThai-Bold.ttf", 0), ("NotoSansThai-Bold.ttf", 0)]];
    pub const LAO: &[Weights] = &[[("NotoSansLao-Regular.ttf", 0); 3]];
    pub const KHMER: &[Weights] = &[[("NotoSansKhmer-Regular.ttf", 0); 3]];
    pub const SEGOE: &[Weights] = &[
        [("NotoSansArabic-Regular.ttf", 0); 3],
        [("NotoSansHebrew-Regular.ttf", 0); 3],
        [("NotoSansArmenian-Regular.ttf", 0); 3],
        [("NotoSansGeorgian-Regular.ttf", 0); 3],
        [("DejaVuSans.ttf", 0), ("DejaVuSans-Bold.ttf", 0), ("DejaVuSans-Bold.ttf", 0)],
    ];
    pub const INDIC: &[Weights] = &[[("NotoSansDevanagari-Regular.ttf", 0); 3], [("NotoSansBengali-Regular.ttf", 0); 3], [("NotoSansTamil-Regular.ttf", 0); 3]];
    pub const ETHIOPIC: &[Weights] = &[[("NotoSansEthiopic-Regular.ttf", 0); 3]];
    pub const MYANMAR: &[Weights] = &[[("NotoSansMyanmar-Regular.ttf", 0); 3]];
    pub const TIBETAN: &[Weights] = &[[("NotoSerifTibetan-Regular.ttf", 0); 3]];
    pub const MONGOLIAN: &[Weights] = &[[("NotoSansMongolian-Regular.ttf", 0); 3]];
    pub const CHEROKEE: &[Weights] = &[[("NotoSansCherokee-Regular.ttf", 0); 3]];
    pub const YI: &[Weights] = &[[("NotoSansYi-Regular.ttf", 0); 3]];
    pub const SYMBOLS: &[Weights] = &[[("NotoSansSymbols2-Regular.ttf", 0); 3], [("DejaVuSans.ttf", 0); 3]];
}

/// The fonts to try for a script, in order.
fn candidates(script: Script, cjk: Cjk) -> Vec<&'static [Weights]> {
    use table::*;
    match script {
        Script::Hangul => vec![KOREAN],
        Script::Kana => match cjk {
            Cjk::Japanese => vec![JAPANESE, SIMPLIFIED, KOREAN],
            Cjk::Korean => vec![KOREAN, JAPANESE, SIMPLIFIED],
            Cjk::Simplified | Cjk::Traditional => vec![SIMPLIFIED, JAPANESE, TRADITIONAL, KOREAN],
        },
        Script::Han => match cjk {
            Cjk::Japanese => vec![JAPANESE, SIMPLIFIED, TRADITIONAL, KOREAN],
            Cjk::Traditional => vec![TRADITIONAL, SIMPLIFIED, JAPANESE, KOREAN],
            Cjk::Korean => vec![KOREAN, SIMPLIFIED, TRADITIONAL, JAPANESE],
            Cjk::Simplified => vec![SIMPLIFIED, TRADITIONAL, JAPANESE, KOREAN],
        },
        Script::Thai => vec![THAI],
        Script::Lao => vec![LAO],
        Script::Khmer => vec![KHMER],
        Script::Arabic | Script::Hebrew | Script::Armenian | Script::Georgian => vec![SEGOE],
        Script::Indic => vec![INDIC],
        Script::Ethiopic => vec![ETHIOPIC],
        Script::Myanmar => vec![MYANMAR],
        Script::Tibetan => vec![TIBETAN],
        Script::Mongolian => vec![MONGOLIAN],
        Script::Cherokee => vec![CHEROKEE],
        Script::Yi => vec![YI],
        Script::Other => vec![SEGOE, SYMBOLS],
    }
}

/// Whether a face draws `c` (its character map has it).
fn covers(data: &FontData, c: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(data.bytes(), data.index).is_ok_and(|font| font.charmap().map(c).is_some())
}

/// Letters the interface's fonts lack, from the system's font for their script at the nearest weight. A file is
/// opened the first time one of its letters shows up; egui then keeps the face in the family's fallbacks, and asks
/// again only for a letter that face lacks too.
struct SystemFallback {
    cjk: Cjk,
}

impl SystemFallback {
    /// The face for `c` in text of `weight`, and the name egui knows it by.
    fn face(&self, c: char, weight: u16) -> Option<(String, FontData)> {
        let column = match weight {
            600 => 1,
            700 => 2,
            _ => 0,
        };
        for family in candidates(script(c), preferred_cjk(self.cjk)) {
            for weights in family {
                // A missing bold file falls back to the regular one.
                let mut tries = vec![weights[column]];
                if weights[0] != weights[column] {
                    tries.push(weights[0]);
                }
                for (file, index) in tries {
                    let Some(blob) = system_file(file) else { continue };
                    let data = FontData::from_blob(blob, index);
                    if covers(&data, c) {
                        return Some((format!("system:{file}#{index}"), data));
                    }
                }
            }
        }
        None
    }
}

impl egui::FontProvider for SystemFallback {
    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        // The icon fonts have nothing to fall back to.
        let weight = match families().iter().position(|f| f == request.family) {
            Some(i) => WEIGHTS[i],
            None if *request.family == icons() || *request.family == icons_fill() => return None,
            None => 400,
        };
        let (name, data) = self.face(c, weight)?;
        Some(FontInsert::new(&name, data, vec![InsertFontFamily { family: request.family.clone(), priority: FontPriority::Lowest }]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every language of the website, in every family the interface writes with, with the fonts of this computer: each letter a real glyph of a real
    /// font, on what the windows, the overlay canvases, the VR parts and the software painter all share ([`install`]). A letter no font has
    /// would draw as a blank or a box, and silently: this compares it with what the fonts give for a character that does not exist.
    #[cfg(windows)]
    #[test]
    fn every_language_of_the_website_draws_in_every_family() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        const SAMPLES: [(&str, &str); 5] = [
            ("en", "AaZz09?!'\u{2026}"),
            ("fr", "\u{e0}\u{e2}\u{e4}\u{e6}\u{e7}\u{e9}\u{e8}\u{ea}\u{eb}\u{ee}\u{ef}\u{f4}\u{f6}\u{153}\u{f9}\u{fb}\u{fc}\u{ff}\u{c0}\u{c7}\u{c9}\u{c8}\u{152}\u{ab}\u{bb}\u{2019}"),
            ("es", "\u{e1}\u{e9}\u{ed}\u{f3}\u{fa}\u{fc}\u{f1}\u{c1}\u{c9}\u{cd}\u{d3}\u{da}\u{d1}\u{bf}\u{a1}"),
            ("pt-BR", "\u{e1}\u{e2}\u{e3}\u{e0}\u{e7}\u{e9}\u{ea}\u{ed}\u{f3}\u{f4}\u{f5}\u{fa}\u{c3}\u{d5}\u{c7}"),
            ("ja", "\u{3042}\u{3093}\u{30a2}\u{30f3}\u{30fc}\u{8a2d}\u{5b9a}\u{8a00}\u{8a9e}\u{5199}\u{771f}\u{4fdd}\u{5b58}\u{300c}\u{300d}\u{3001}\u{3002}\u{ff01}\u{ff1f}"),
        ];
        let ctx = egui::Context::default();
        install(&ctx);
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))), ..Default::default() };
        let draw = |text: &str, family: &FontFamily| {
            let mut out = Vec::new();
            let mut full = ctx.run_ui(raw.clone(), |ui| {
                let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::new(16.0, family.clone()), egui::Color32::WHITE);
                for row in &galley.rows {
                    for glyph in &row.glyphs {
                        out.push((glyph.chr, glyph.uv_rect));
                    }
                }
            });
            full.textures_delta.clear();
            out
        };
        let mut missing = Vec::new();
        for family in [family(400.0), family(500.0), family(600.0), family(700.0), FontFamily::Monospace] {
            // What a character no font has draws as: the same place in the atlas as this one is the same box.
            let nothing = draw("\u{10FFFD}", &family)[0].1;
            let other = draw("\u{10FFFC}", &family)[0].1;
            assert!(other.min == nothing.min && other.max == nothing.max, "this test can tell a missing letter");
            for (language, letters) in SAMPLES {
                // Twice: a system font comes in at the first letter it is asked for.
                let _ = draw(letters, &family);
                for (c, uv) in draw(letters, &family) {
                    if uv.is_nothing() || (uv.min == nothing.min && uv.max == nothing.max) {
                        missing.push(format!("{language} {c:?} in {family:?}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{}", missing.join("\n"));
    }

    /// Japanese has its fonts on every Windows this runs on, without the optional Japanese fonts: Yu Gothic comes first, and it ships with Windows 10 and 11.
    /// The tests that change which language's fonts are preferred, one at a time: it is one setting for the whole process, and a test that looked for Japanese while another
    /// went back to English found Microsoft YaHei.
    static PREFERRING: Mutex<()> = Mutex::new(());

    #[cfg(windows)]
    #[test]
    fn japanese_comes_from_a_font_windows_always_has() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(table::JAPANESE[0][0].0, "YuGothM.ttc");
        let dir = windows_fonts();
        assert!(dir.join("YuGothM.ttc").is_file() && dir.join("YuGothB.ttc").is_file());
        let face = SystemFallback { cjk: Cjk::Simplified };
        PREFERRED.store(1 + Cjk::Japanese as u8, Ordering::Relaxed);
        let found = face.face('\u{8a00}', 400).map(|f| f.0);
        let bold = face.face('\u{8a00}', 700).map(|f| f.0);
        PREFERRED.store(0, Ordering::Relaxed);
        assert_eq!(found.as_deref(), Some("system:YuGothM.ttc#1"), "Japanese text on a computer in any language");
        assert!(bold.as_deref().is_some_and(|b| b.starts_with("system:YuGothB")), "{bold:?}");
    }

    /// VR1115 flaky: a test drawing N4N0's pages in Japanese set the preference for the whole process, and contexts painting on other threads found their
    /// faces anew halfway through what they compared. One thread's preference is its own.
    #[test]
    fn a_preference_for_one_thread_leaves_the_process_and_the_others() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        let process = PREFERRED.load(Ordering::Relaxed);
        let ctx = egui::Context::default();
        install(&ctx);
        in_preferred_language("ja", || {
            assert_eq!(preferred(), 1 + Cjk::Japanese as u8);
            std::thread::scope(|s| {
                s.spawn(|| {
                    assert_eq!(preferred(), process, "another thread");
                    follow_language(&ctx);
                });
            });
        });
        assert_eq!((preferred(), PREFERRED.load(Ordering::Relaxed)), (process, process), "and after");
        assert_eq!(ctx.data(|d| d.get_temp::<u8>(egui::Id::unique(LANGUAGE_FONTS))), Some(process), "a context painting elsewhere kept its faces");
    }

    #[test]
    fn letters_find_their_script() {
        for (c, s) in [
            ('あ', Script::Kana),
            ('カ', Script::Kana),
            ('ｶ', Script::Kana),
            ('漢', Script::Han),
            ('。', Script::Han),
            ('Ａ', Script::Han),
            ('한', Script::Hangul),
            ('ㅎ', Script::Hangul),
            ('ไ', Script::Thai),
            ('ع', Script::Arabic),
            ('א', Script::Hebrew),
            ('ह', Script::Indic),
            ('ሰ', Script::Ethiopic),
            ('A', Script::Other),
            ('★', Script::Other),
        ] {
            assert_eq!(script(c), s, "{c}");
        }
    }

    #[test]
    fn the_locale_picks_the_cjk_font() {
        for (locale, cjk) in [
            ("ja-JP", Cjk::Japanese),
            ("ja_JP.UTF-8", Cjk::Japanese),
            ("ko-KR", Cjk::Korean),
            ("zh-CN", Cjk::Simplified),
            ("zh-TW", Cjk::Traditional),
            ("zh-Hant-HK", Cjk::Traditional),
            ("fr-FR", Cjk::Simplified),
            ("", Cjk::Simplified),
        ] {
            assert_eq!(cjk_for_locale(locale), cjk, "{locale}");
        }
    }

    /// A kanji drawn before the language became Japanese must not keep its Chinese font for everything after.
    #[cfg(windows)]
    #[test]
    fn a_change_to_japanese_leaves_no_chinese_face_behind() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        prefer_language("zh");
        install(&ctx);
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0))), ..Default::default() };
        let draw = |ctx: &egui::Context| {
            let mut full = ctx.run_ui(raw.clone(), |ui| {
                let _ = ui.painter().layout_no_wrap("\u{8a00}\u{8a9e}".to_string(), egui::FontId::new(16.0, family(400.0)), egui::Color32::WHITE);
            });
            full.textures_delta.clear();
            // The faces this context found for letters its own fonts lack.
            ctx.fonts(|f| f.discovered_fonts().iter().map(|i| i.name.clone()).filter(|n| n.starts_with("system:")).collect::<Vec<_>>())
        };
        draw(&ctx);
        let before = draw(&ctx);
        assert!(before.iter().any(|f| f.contains("msyh")), "a font for Chinese first: {before:?}");
        prefer_language("ja");
        follow_language(&ctx);
        draw(&ctx);
        let after = draw(&ctx);
        prefer_language("en");
        assert!(after.iter().any(|f| f.contains("YuGoth")) && !after.iter().any(|f| f.contains("msyh")), "{after:?}");
    }

    #[test]
    fn a_language_puts_its_cjk_font_first() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(language_cjk("ja"), Some(Cjk::Japanese));
        assert_eq!(language_cjk("zh-TW"), Some(Cjk::Traditional));
        assert_eq!(language_cjk("zh"), Some(Cjk::Simplified));
        assert_eq!(language_cjk("ko-KR"), Some(Cjk::Korean));
        assert_eq!(language_cjk("fr"), None);
        // Japanese only: the other tests here look for faces in Japanese or the locale's order.
        prefer_language("ja");
        assert_eq!(preferred_cjk(Cjk::Simplified), Cjk::Japanese);
        prefer_language("en");
        assert_eq!(preferred_cjk(Cjk::Simplified), Cjk::Simplified);
    }

    #[test]
    fn families_are_the_same_values_every_time() {
        assert_eq!(family(400.0), FontFamily::Proportional);
        assert_eq!(family(650.0), FontFamily::Name("w600".into()));
        assert_eq!(family(f32::NAN), FontFamily::Proportional);
        assert_eq!(icons(), FontFamily::Name("icons".into()));
    }

    /// The Mac's fonts draw what Orbiters Sans lacks, in every script names show up in.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_s_fonts_draw_other_scripts() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        let fallback = SystemFallback { cjk: Cjk::Japanese };
        for c in ['猫', 'ね', '한', 'ไ', 'ع', 'א', 'ह', 'ሰ', 'д', 'α', 'ệ', '✓', '→'] {
            let (name, data) = fallback.face(c, 400).unwrap_or_else(|| panic!("no face for {c}"));
            assert!(covers(&data, c), "{c} in {name}");
        }
        assert!(fallback.face('ね', 700).is_some_and(|(n, _)| n.contains("W7")), "bold Japanese comes from a bold cut");
        assert!(matches!(found().sans, Some(Sans::Variable(_))) && found().mono.is_some(), "the interface's own fonts are in the program");
    }

    /// The bundled text font carries the letters and punctuation the interface writes, with Segoe UI's line height.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn orbiters_sans_is_the_interface_s_font() {
        use skrifa::{instance::Size, MetadataProvider as _};
        let font = skrifa::FontRef::new(SANS).unwrap();
        for c in "AZaz09éèêàçñßœ·…—–’“”‹›•€™".chars() {
            assert!(font.charmap().map(c).is_some(), "{c}");
        }
        let m = font.metrics(Size::new(2048.0), skrifa::instance::LocationRef::default());
        assert_eq!((m.ascent, m.descent), (2210.0, -514.0));
        let axes: Vec<_> = font.axes().iter().map(|a| a.tag().to_string()).collect();
        assert_eq!(axes, ["wght"]);
    }

    /// With Windows' fonts at hand: Japanese, Korean and Thai letters get a face that draws them, bold in bold text,
    /// and the icon families are left alone.
    #[cfg(windows)]
    #[test]
    fn system_fonts_draw_other_scripts() {
        let _one_at_a_time = PREFERRING.lock().unwrap_or_else(|e| e.into_inner());
        if !windows_fonts().join("msyh.ttc").is_file() {
            return;
        }
        let fallback = SystemFallback { cjk: Cjk::Japanese };
        let (name, data) = fallback.face('猫', 400).expect("a face for kanji");
        assert!(name.starts_with("system:YuGoth") || name.starts_with("system:msyh"), "{name}");
        assert!(covers(&data, '猫'));
        if windows_fonts().join("YuGothB.ttc").is_file() {
            let (bold, _) = fallback.face('ね', 700).expect("a bold face for kana");
            assert_eq!(bold, "system:YuGothB.ttc#1");
        }
        if windows_fonts().join("malgun.ttf").is_file() {
            assert!(fallback.face('한', 400).is_some_and(|(n, _)| n.starts_with("system:malgun")));
        }
        if windows_fonts().join("LeelawUI.ttf").is_file() {
            assert!(fallback.face('ไ', 400).is_some_and(|(n, _)| n == "system:LeelawUI.ttf#0"));
        }
        let request = FallbackRequest { cluster: "猫", family: &icons() };
        assert!(egui::FontProvider::font_for(&fallback, &request).is_none());
        let request = FallbackRequest { cluster: "猫", family: &families()[3] };
        assert!(egui::FontProvider::font_for(&fallback, &request).is_some_and(|f| f.families[0].family == families()[3]));
    }
}
