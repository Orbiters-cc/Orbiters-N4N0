//! Fonts. Windows 11 has Segoe UI Variable: every weight through its `wght` axis and optical sizes through
//! `opsz`, so headings get the Display cut and labels the Text cut. Elsewhere the first common sans family found
//! (static weights when there is no variable file). Cascadia Mono or Consolas for IDs, and the Phosphor icons.

use egui::epaint::text::{FontTweak, VariationCoords};
use egui::{FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Phosphor Icons (MIT), regular and fill.
pub const PHOSPHOR: &[u8] = include_bytes!("../assets/Phosphor.ttf");
pub const PHOSPHOR_FILL: &[u8] = include_bytes!("../assets/Phosphor-Fill.ttf");

const WEIGHTS: [u16; 4] = [400, 500, 600, 700];

/// Set when the text font is variable: exact weights and optical sizes then come from [`coords`].
static VARIABLE: AtomicBool = AtomicBool::new(false);

/// Family for a weight (400, 500, 600 or 700, whichever is nearest).
pub fn family(weight: f32) -> FontFamily {
    let w = WEIGHTS.iter().min_by_key(|w| (**w as f32 - weight).abs() as u32).copied().unwrap_or(400);
    if w == 400 {
        FontFamily::Proportional
    } else {
        FontFamily::Name(format!("w{w}").into())
    }
}

pub fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

pub fn icons_fill() -> FontFamily {
    FontFamily::Name("icons-fill".into())
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
    Variable(&'static [u8]),
    /// Files for 400, 500, 600, 700 (missing weights borrow the nearest heavier one, then lighter).
    Static([Option<&'static [u8]>; 4]),
}

/// Loads the fonts into `ctx` (egui's own fonts stay as fallbacks for symbols the system font lacks).
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let fallback_text = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fallback_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    defs.font_data.insert("phosphor".into(), FontData::from_static(PHOSPHOR).into());
    defs.font_data.insert("phosphor-fill".into(), FontData::from_static(PHOSPHOR_FILL).into());
    defs.families.insert(icons(), vec!["phosphor".into()]);
    defs.families.insert(icons_fill(), vec!["phosphor-fill".into()]);

    let sans = find_sans();
    VARIABLE.store(matches!(sans, Some(Sans::Variable(_))), Ordering::Relaxed);
    for (i, w) in WEIGHTS.iter().enumerate() {
        let name = format!("ui-{w}");
        let data = match &sans {
            Some(Sans::Variable(bytes)) => {
                Some(FontData::from_static(bytes).tweak(FontTweak { coords: VariationCoords::new([(b"wght", *w as f32)]), ..Default::default() }))
            }
            Some(Sans::Static(files)) => {
                let pick = files[i..].iter().chain(files[..i].iter().rev()).flatten().next();
                pick.map(|bytes| FontData::from_static(bytes))
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
        defs.families.insert(family(*w as f32), list);
    }

    if let Some(bytes) = find_mono() {
        defs.font_data.insert("mono".into(), FontData::from_static(bytes).into());
        let mut list = vec!["mono".to_string()];
        list.extend(fallback_mono);
        defs.families.insert(FontFamily::Monospace, list);
    }
    ctx.set_fonts(defs);
}

/// Reads a font file once for the life of the app.
fn load(path: &Path) -> Option<&'static [u8]> {
    std::fs::read(path).ok().map(|bytes| &*Box::leak(bytes.into_boxed_slice()))
}

#[cfg(windows)]
fn windows_fonts() -> PathBuf {
    std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts")
}

#[cfg(windows)]
fn find_sans() -> Option<Sans> {
    let dir = windows_fonts();
    if let Some(bytes) = load(&dir.join("SegUIVar.ttf")) {
        return Some(Sans::Variable(bytes));
    }
    // Windows 10: static Segoe UI, which has no medium cut.
    let regular = load(&dir.join("segoeui.ttf"))?;
    Some(Sans::Static([Some(regular), None, load(&dir.join("seguisb.ttf")), load(&dir.join("segoeuib.ttf"))]))
}

#[cfg(windows)]
fn find_mono() -> Option<&'static [u8]> {
    let dir = windows_fonts();
    load(&dir.join("CascadiaMono.ttf")).or_else(|| load(&dir.join("consola.ttf")))
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

/// First file (in `names` order) found under the font folders.
#[cfg(not(windows))]
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
    let mut files = Vec::new();
    for d in font_dirs() {
        walk(&d, 0, &mut files);
    }
    names.iter().find_map(|n| files.iter().find(|p| p.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n))).cloned())
}

#[cfg(not(windows))]
fn find_sans() -> Option<Sans> {
    let variable =
        ["InterVariable.ttf", "Inter[opsz,wght].ttf", "Inter-VariableFont_opsz,wght.ttf", "NotoSans[wdth,wght].ttf", "NotoSans-VariableFont_wdth,wght.ttf"];
    if let Some(bytes) = find_file(&variable).and_then(|p| load(&p)) {
        return Some(Sans::Variable(bytes));
    }
    let families: [[&str; 4]; 3] = [
        ["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf", "Inter-Bold.ttf"],
        ["NotoSans-Regular.ttf", "NotoSans-Medium.ttf", "NotoSans-SemiBold.ttf", "NotoSans-Bold.ttf"],
        ["DejaVuSans.ttf", "", "", "DejaVuSans-Bold.ttf"],
    ];
    for names in families {
        if let Some(regular) = find_file(&[names[0]]).and_then(|p| load(&p)) {
            let other = |n: &str| if n.is_empty() { None } else { find_file(&[n]).and_then(|p| load(&p)) };
            return Some(Sans::Static([Some(regular), other(names[1]), other(names[2]), other(names[3])]));
        }
    }
    None
}

#[cfg(not(windows))]
fn find_mono() -> Option<&'static [u8]> {
    find_file(&["CascadiaMono.ttf", "JetBrainsMono-Regular.ttf", "DejaVuSansMono.ttf", "NotoSansMono-Regular.ttf", "LiberationMono-Regular.ttf"])
        .and_then(|p| load(&p))
}
