//! Photos & captures' layouts and VRChat's multi-layer pictures, on made-up pictures written once to the temporary
//! folder (this computer's own are not needed): pictures of several shapes beside this computer's captures, and one
//! picture with its Environment, Player and UI layers as VRChat names them.
//!
//! `gallery-masonry` (`-hover` with the pointer on a picture, `-scrolled` down to the items shared with you),
//! `gallery-rows` (the default), `gallery-grid`; `gallery-layers-window` (the multi-layer picture's
//! window, its strip of layers), `gallery-layer-chosen` (the Player layer alone over the checkerboard), `gallery-layers-stack`
//! (the Player and UI layers stacked, the world hidden).
use super::{Page, Scenario};
use crate::controller::shares::{remote_entry, Entry, Layer, Local};
use crate::worlds::Picture;
use orbiters_core::settings::GalleryLayout;
use std::path::{Path, PathBuf};

const NAMES: [&str; 8] = [
    "gallery-masonry",
    "gallery-masonry-hover",
    "gallery-masonry-scrolled",
    "gallery-rows",
    "gallery-grid",
    "gallery-layers-window",
    "gallery-layer-chosen",
    "gallery-layers-stack",
];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in NAMES {
        scenarios.push(Scenario {
            name,
            page: Page::Sessions,
            snapshot: base.clone(),
            frames: if name.starts_with("gallery-layer") { 170 } else { 140 },
            splash: false,
            open_detail: false,
            toasts: vec![],
            // Over the third column's first picture: its kind and time on it.
            pointer: (name == "gallery-masonry-hover").then(|| egui::pos2(880.0, 580.0)),
            // Down to the pictures shared with you (who shared them on them) and the end of the list.
            scroll: if name == "gallery-masonry-scrolled" { 700.0 } else { 0.0 },
            world: None,
            home: None,
            library: None,
        });
    }
}

/// What each scenario does on frame `f` (after the gallery's own staging).
pub(super) fn stage(ctx: &egui::Context, gallery: &mut crate::pages::gallery::State, name: &str, f: u32) {
    if !NAMES.contains(&name) {
        return;
    }
    let dir = std::env::temp_dir().join("orbiters-gallery-fixture");
    if f == 0 || f == 45 {
        gallery.stage_extra(entries(&dir));
        gallery.stage_layout(match name {
            "gallery-rows" => GalleryLayout::Rows,
            "gallery-grid" => GalleryLayout::Grid,
            _ => GalleryLayout::Masonry,
        });
    }
    if name.starts_with("gallery-layer") && f == 80 {
        gallery.open_key(ctx, &dir.join(format!("{SHOT}.png")).to_string_lossy());
    }
    if f == 130 {
        match name {
            "gallery-layer-chosen" => gallery.stage_layer(Some(1), &[]),
            // The Player layer chosen, the UI shown over it too.
            "gallery-layers-stack" => gallery.stage_layer(Some(1), &[2]),
            _ => {}
        }
    }
}

/// The multi-layer picture's name, as VRChat writes it.
const SHOT: &str = "VRChat_2026-10-28_21-12-04.318_1920x1080";

/// The made-up pictures: several shapes, and the multi-layer one with its three layers (written when missing).
fn entries(dir: &Path) -> Vec<Entry> {
    let _ = std::fs::create_dir_all(dir);
    let shapes: [(u32, u32); 7] = [(960, 540), (540, 960), (720, 720), (800, 600), (1050, 450), (900, 600), (600, 900)];
    let mut out = Vec::new();
    let base = crate::controller::shares::unix("2026-10-28T21:20:00Z");
    for (n, (w, h)) in shapes.iter().enumerate() {
        let path = dir.join(format!("shape-{n}-{w}x{h}.png"));
        if !path.is_file() {
            let _ = save(&path, *w, *h, &scene(*w, *h, n as u32));
        }
        out.push(entry(&path, *w, *h, base - 120 * n as u64, Vec::new()));
    }
    let (w, h) = (960, 540);
    let world = scene(w, h, 9);
    let people = player(w, h);
    let ui = interface(w, h);
    let files = [("", over(&over(&world, &people), &ui)), ("_Environment", world), ("_Player", people), ("_UI", ui)];
    for (suffix, rgba) in &files {
        let path = dir.join(format!("{SHOT}{suffix}.png"));
        if !path.is_file() {
            let _ = save(&path, w, h, rgba);
        }
    }
    let layers = ["Environment", "Player", "UI"]
        .iter()
        .map(|n| Layer { path: dir.join(format!("{SHOT}_{n}.png")), name: (*n).into(), width: w, height: h, shared: None })
        .collect();
    out.insert(2, entry(&dir.join(format!("{SHOT}.png")), w, h, base + 30, layers));
    out
}

fn entry(path: &Path, w: u32, h: u32, taken: u64, layers: Vec<Layer>) -> Entry {
    let mut e = remote_entry(orbiters_core::shares::Item::default());
    e.key = path.to_string_lossy().into();
    e.remote = None;
    e.source_key = None;
    e.taken = taken;
    e.local = Some(Local::Picture(path.to_path_buf()));
    e.picture = Some(Picture::Shot { path: PathBuf::from(path), size: std::fs::metadata(path).map_or(0, |m| m.len()), width: 560 });
    e.size = Some([w, h]);
    e.layers = layers;
    e
}

fn save(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    image::save_buffer(path, rgba, w, h, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

/// A dusk over hills: a sky in two colours (by `seed`), a sun, two rows of hills.
fn scene(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let skies = [
        ([38.0, 52.0, 110.0], [236.0, 140.0, 120.0]),
        ([20.0, 30.0, 60.0], [120.0, 90.0, 200.0]),
        ([60.0, 120.0, 190.0], [250.0, 210.0, 150.0]),
        ([24.0, 70.0, 90.0], [170.0, 230.0, 200.0]),
    ];
    let (top, bottom) = skies[seed as usize % skies.len()];
    let sun = (w as f32 * (0.25 + 0.15 * (seed % 4) as f32), h as f32 * 0.42, w.min(h) as f32 * 0.09);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let t = y as f32 / h as f32;
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let mut c = [0.0; 3];
            for k in 0..3 {
                c[k] = top[k] + (bottom[k] - top[k]) * t;
            }
            let d = ((x as f32 - sun.0).powi(2) + (y as f32 - sun.1).powi(2)).sqrt();
            let glow = (1.0 - (d / (sun.2 * 3.0))).clamp(0.0, 1.0).powi(2) * 0.5;
            let disc = (sun.2 - d).clamp(0.0, 1.0);
            for k in 0..3 {
                c[k] += (255.0 - c[k]) * (glow + disc * 0.9);
            }
            let far = 0.62 + 0.05 * (fx * 9.0 + seed as f32).sin() + 0.03 * (fx * 23.0).sin();
            let near = 0.76 + 0.06 * (fx * 5.0 + seed as f32 * 2.0).cos() + 0.02 * (fx * 31.0).sin();
            if t > near {
                c = [18.0 + 10.0 * t, 34.0 + 20.0 * t, 30.0 + 10.0 * t];
            } else if t > far {
                c = [c[0] * 0.45, c[1] * 0.5, c[2] * 0.6];
            }
            out.extend([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
    }
    out
}

/// Two people, transparent around them (VRChat's Player layer).
fn player(w: u32, h: u32) -> Vec<u8> {
    let s = h as f32;
    let people = [(w as f32 * 0.42, [240.0, 120.0, 160.0]), (w as f32 * 0.6, [120.0, 200.0, 250.0])];
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32, y as f32);
            for (cx, color) in &people {
                let head = ((fx - cx).powi(2) + (fy - s * 0.52).powi(2)).sqrt() - s * 0.055;
                let bx = (fx - cx).abs() - s * 0.07;
                let by = (fy - s * 0.78).abs() - s * 0.2;
                let body = bx.max(by).min(0.0) + (bx.max(0.0).powi(2) + by.max(0.0).powi(2)).sqrt() - s * 0.03;
                let a = (0.5 - head.min(body)).clamp(0.0, 1.0);
                if a > 0.0 {
                    let i = ((y * w + x) * 4) as usize;
                    let shade = if head < body { 1.0 } else { 0.8 };
                    out[i..i + 4].copy_from_slice(&[(color[0] * shade) as u8, (color[1] * shade) as u8, (color[2] * shade) as u8, (a * 255.0) as u8]);
                }
            }
        }
    }
    out
}

/// Name plates over the people (VRChat's UI layer), transparent elsewhere.
fn interface(w: u32, h: u32) -> Vec<u8> {
    let s = h as f32;
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (cx, len) in [(w as f32 * 0.42, 0.13), (w as f32 * 0.6, 0.17)] {
        let (pw, ph, top) = (s * len, s * 0.045, s * 0.39);
        for y in (top as u32)..((top + ph) as u32) {
            for x in ((cx - pw * 0.5) as u32)..((cx + pw * 0.5) as u32) {
                let i = ((y * w + x) * 4) as usize;
                let text = (y as f32 - top - ph * 0.5).abs() < ph * 0.12 && (x as f32 - cx).abs() < pw * 0.36;
                out[i..i + 4].copy_from_slice(if text { &[255, 255, 255, 255] } else { &[20, 22, 28, 200] });
            }
        }
    }
    out
}

/// `top` over `bottom` (straight alpha).
fn over(bottom: &[u8], top: &[u8]) -> Vec<u8> {
    bottom
        .chunks_exact(4)
        .zip(top.chunks_exact(4))
        .flat_map(|(b, t)| {
            let a = t[3] as f32 / 255.0;
            [0, 1, 2].map(|k| (t[k] as f32 * a + b[k] as f32 * (1.0 - a)) as u8).into_iter().chain([255])
        })
        .collect()
}
