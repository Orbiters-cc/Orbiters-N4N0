//! The photo editor's screenshots (`--only editor`, or one `editor-…` by name): the editor over Photos & captures on a
//! stand-in VRChat picture (`ORBITERS_EDITOR_PICTURE`, else the first of this computer's VRChat pictures, else one
//! drawn here), one scenario per effect with its controls, crop and rotate, the layer list with the add grid, text in
//! the Toolkit's fonts and styles, and a multi-layer picture (its world and its people, a cut-out of the stand-in)
//! with effects on each layer. `editor-plain` is the picture before any effect. `--only editor-3d` saves a view of
//! the first reconstructed capture of `--sessions` (into `target/tmp/editor-pictures`, never VRChat's folder) and opens
//! the editor on it.

use super::{Page, Scenario};
use crate::app::App;
use crate::pages::editor::{files, Editor, Selected, Source};
use orbiters_ui::photo::{self, Aspect, Blur, BlurKind, Color, Comic, Crop, Curves, Effect, Light, Mask, Text, TextStyle, Vignette};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const NAMES: [&str; 18] = [
    "editor-window",
    "editor-plain",
    "editor-open",
    "editor-light",
    "editor-color",
    "editor-curves",
    "editor-vignette",
    "editor-aberration",
    "editor-comic",
    "editor-blur-gaussian",
    "editor-blur-lens",
    "editor-blur-directional",
    "editor-text",
    "editor-text-glow",
    "editor-crop",
    "editor-cropped",
    "editor-layers",
    "editor-multilayer",
];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in NAMES {
        scenarios.push(Scenario {
            name,
            page: Page::Sessions,
            snapshot: base.clone(),
            frames: 90,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }
}

/// The stand-in picture.
fn stand_in() -> &'static PathBuf {
    static PICTURE: OnceLock<PathBuf> = OnceLock::new();
    PICTURE.get_or_init(|| {
        if let Some(p) = std::env::var_os("ORBITERS_EDITOR_PICTURE").map(PathBuf::from).filter(|p| p.is_file()) {
            return p;
        }
        for dir in orbiters_core::visited::screenshot_dirs() {
            if let Some(p) = first_picture(&dir, 0) {
                return p;
            }
        }
        drawn()
    })
}

fn first_picture(dir: &Path, depth: u32) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in &entries {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if p.is_file() && name.starts_with("VRChat_") && name.ends_with(".png") {
            return Some(p.clone());
        }
    }
    entries.iter().filter(|p| p.is_dir() && depth < 2).find_map(|d| first_picture(d, depth + 1))
}

/// A picture drawn here when this computer has none: a dusk sky, a skyline, lights.
fn drawn() -> PathBuf {
    let (w, h) = (1920u32, 1080u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let sky = [0.9 - 0.6 * v, 0.45 - 0.3 * v, 0.55 + 0.2 * v];
            let building = ((u * 23.0).floor() * 7.31).sin().abs() * 0.35 + 0.35;
            let lit = v > 1.0 - building && x % 28 < 12 && y % 36 < 16 && ((x / 28) * 7 + (y / 36) * 3) % 5 < 2;
            let c = if v > 1.0 - building { if lit { [1.0, 0.85, 0.5] } else { [0.08, 0.07, 0.12] } } else { sky };
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, 255]);
        }
    }
    let path = std::env::temp_dir().join("orbiters-editor-stand-in.png");
    let _ = files::write_png(&path, &photo::Rgba8::new(w, h, px), None);
    path
}

/// A multi-layer picture made of the stand-in: VRChat's composite, its world, and its people (an oval cut out of the
/// middle, transparent around it), named as VRChat names them.
fn multi_layer() -> &'static Source {
    static SOURCE: OnceLock<Source> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("orbiters-editor-layers-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let picture = files::load(stand_in()).map(|p| p.fit(1920)).unwrap_or_else(|_| photo::Rgba8::new(1, 1, vec![0, 0, 0, 255]));
        let (w, h) = (picture.width, picture.height);
        let base = format!("VRChat_2026-10-29_21-04-11.532_{w}x{h}");
        let mut people = picture.clone();
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = ((x as f32 / w as f32 - 0.5) / 0.17, (y as f32 / h as f32 - 0.62) / 0.36);
                let d = (dx * dx + dy * dy).sqrt();
                let a = (1.0 - ((d - 0.97) / 0.03).clamp(0.0, 1.0)) * 255.0;
                people.pixels[((y * w + x) * 4 + 3) as usize] = a as u8;
            }
        }
        let composite = dir.join(format!("{base}.png"));
        let world = dir.join(format!("{base}_Environment.png"));
        let player = dir.join(format!("{base}_Player.png"));
        let _ = files::write_png(&composite, &picture, None);
        let _ = files::write_png(&world, &picture, None);
        let _ = files::write_png(&player, &people, None);
        Source::with_layers(composite, [("Environment".to_string(), world), ("Player".to_string(), player)])
    })
}

/// Opens the editor of `name` over a picture window (frame 2), stages its layers, and keeps its preview made before
/// each frame from then on, so that the shot shows the edit as staged.
pub(super) fn stage(ctx: &egui::Context, app: &mut App, name: &str, f: u32) {
    let gallery = app.sessions_mut().gallery_mut();
    if f == 2 {
        let feed: orbiters_core::shares::Feed = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/captures.json")).expect("fixture");
        let Some(item) = feed.items.into_iter().next() else { return };
        let mut entry = crate::controller::shares::remote_entry(item);
        if name == "editor-window" {
            // The picture window of a picture on this computer: Edit beside its sharing.
            let path = stand_in().clone();
            entry.key = path.to_string_lossy().into_owned();
            entry.world_name = "Midnight Rooftop".into();
            entry.remote = None;
            entry.picture = Some(crate::worlds::Picture::Shot { path: path.clone(), size: 0, width: 560 });
            entry.local = Some(crate::controller::shares::Local::Picture(path));
        }
        let mut d = crate::pages::sharing::Dialog::new(ctx, entry, false);
        d.fixture("none");
        if name == "editor-window" {
            gallery.dialog = Some(d);
            return;
        }
        let source = if name == "editor-multilayer" { multi_layer().clone() } else { Source::picture(stand_in().clone()) };
        let mut e = Editor::open(ctx, source, "Midnight Rooftop");
        staged(&mut e, name);
        *d.editor_slot() = Some(e);
        gallery.dialog = Some(d);
        return;
    }
    if f > 2 {
        if let Some(e) = gallery.dialog.as_mut().and_then(|d| d.editor_slot().as_mut()) {
            e.wait(ctx, 30.0);
        }
    }
}

fn staged(e: &mut Editor, name: &str) {
    let mut select = None;
    {
        let doc = e.edit_mut();
        let add = |doc: &mut photo::Edit, effect: Effect| {
            let l = doc.layer(effect);
            let id = l.id;
            doc.layers.push(l);
            id
        };
        match name {
            "editor-light" => {
                select = Some(add(doc, Effect::Light(Light { exposure: 0.45, brightness: 0.1, contrast: 0.25, highlights: -0.3, shadows: 0.55, black_point: 0.04 })));
            }
            "editor-color" => {
                select = Some(add(doc, Effect::Color(Color { temperature: 0.45, tint: 0.1, saturation: 0.25, vibrance: 0.4, hue: 0.0 })));
            }
            "editor-curves" => {
                let c = Curves { rgb: vec![[0.0, 0.05], [0.25, 0.18], [0.75, 0.85], [1.0, 1.0]], blue: vec![[0.0, 0.12], [1.0, 0.92]], ..Default::default() };
                select = Some(add(doc, Effect::Curves(c)));
            }
            "editor-vignette" => {
                select = Some(add(doc, Effect::Vignette(Vignette { amount: 0.85, size: 0.85, feather: 0.55, center: [0.38, 0.46] })));
            }
            "editor-aberration" => {
                select = Some(add(doc, Effect::Aberration(photo::Aberration { amount: 2.6, center: [0.5, 0.5] })));
            }
            "editor-comic" => {
                select = Some(add(doc, Effect::Comic(Comic { tones: 4, dots: true, dot_size: 0.35, ink: 0.7 })));
            }
            "editor-blur-gaussian" => {
                select = Some(add(doc, Effect::Blur(Blur { kind: BlurKind::Gaussian, radius: 0.008, ..Default::default() })));
            }
            "editor-blur-lens" => {
                let mut l = doc.layer(Effect::Blur(Blur { kind: BlurKind::Lens, radius: 0.018, bokeh: 0.8, ..Default::default() }));
                l.mask = Mask::Radial { center: [0.5, 0.55], radius: 0.22, feather: 0.18, invert: true };
                select = Some(l.id);
                doc.layers.push(l);
            }
            "editor-blur-directional" => {
                let mut l = doc.layer(Effect::Blur(Blur { kind: BlurKind::Directional, radius: 0.05, angle: 12.0, ..Default::default() }));
                l.mask = Mask::Band { center: [0.5, 0.5], angle: 12.0, width: 0.25, feather: 0.2, invert: true };
                select = Some(l.id);
                doc.layers.push(l);
            }
            "editor-text" => {
                add(doc, Effect::Light(Light { exposure: -0.2, contrast: 0.2, ..Default::default() }));
                select = Some(add(doc, Effect::Text(Text { value: "Midnight Rooftop".into(), font: "Pirata One".into(), center: [0.5, 0.82], size: 0.13, color: [255, 230, 90], style: TextStyle::Outline })));
            }
            "editor-text-glow" => {
                add(doc, Effect::Text(Text { value: "N4N0".into(), font: "Monoton".into(), center: [0.5, 0.3], size: 0.2, color: [72, 230, 255], style: TextStyle::Glow }));
                select = Some(add(doc, Effect::Text(Text { value: "after hours\nwith the crew".into(), center: [0.5, 0.78], size: 0.07, style: TextStyle::Shadow, ..Default::default() })));
            }
            "editor-crop" | "editor-cropped" => {
                doc.geometry.straighten = 6.0;
                doc.geometry.aspect = Aspect::Portrait;
                // 4:5 of a 16:9 picture, a little right of its middle.
                let c = photo::geometry::with_ratio(Crop::FULL, 0.8, 16.0, 9.0);
                doc.geometry.crop = Crop { x: c.x + 0.06, ..c };
                add(doc, Effect::Vignette(Vignette::default()));
            }
            "editor-layers" => {
                add(doc, Effect::Light(Light { exposure: 0.2, shadows: 0.3, ..Default::default() }));
                add(doc, Effect::Color(Color { temperature: -0.3, saturation: 0.2, ..Default::default() }));
                let mut v = doc.layer(Effect::Vignette(Vignette::default()));
                v.opacity = 0.6;
                doc.layers.push(v);
                let mut hidden = doc.layer(Effect::Comic(Comic::default()));
                hidden.visible = false;
                doc.layers.push(hidden);
                select = Some(add(doc, Effect::Text(Text { value: "Saturday night".into(), font: "Creepster".into(), ..Default::default() })));
            }
            "editor-multilayer" => {
                let mut blur = doc.layer(Effect::Blur(Blur { kind: BlurKind::Lens, radius: 0.016, bokeh: 0.7, ..Default::default() }));
                blur.opacity = 1.0;
                let dim = doc.layer(Effect::Light(Light { exposure: -0.6, ..Default::default() }));
                doc.sources[0].layers = vec![blur, dim];
                let comic = doc.layer(Effect::Comic(Comic { tones: 4, ink: 0.8, ..Default::default() }));
                let id = comic.id;
                doc.sources[1].layers = vec![comic];
                let title = doc.layer(Effect::Text(Text { value: "Midnight Rooftop".into(), font: "Bonbon".into(), size: 0.1, center: [0.5, 0.14], ..Default::default() }));
                doc.layers.push(title);
                e.select(Selected::Layer(Some(1), id));
                return;
            }
            _ => {}
        }
    }
    match name {
        "editor-crop" => e.select(Selected::Crop),
        "editor-layers" => {
            if let Some(id) = select {
                e.select(Selected::Layer(None, id));
            }
            e.show_add();
        }
        "editor-cropped" | "editor-plain" | "editor-open" => {}
        _ => {
            if let Some(id) = select {
                e.select(Selected::Layer(None, id));
            }
        }
    }
}

/// `--only editor-3d`: the first reconstructed capture's 3D view, Save this view (the picture written into
/// `target/tmp/editor-pictures`), then Edit on it. Shots: `editor-3d-saved`, `editor-3d-edit`.
pub(super) fn three_d(gpu: &mut orbiters_ui::offscreen::Offscreen, out: &Path, settings: &orbiters_core::settings::Settings, scale: f32) -> anyhow::Result<()> {
    use super::{controller, idle_snapshot, Controller, Ctl, Local, Run};
    let pictures = out.join("editor-pictures");
    std::fs::create_dir_all(&pictures)?;
    std::env::set_var("ORBITERS_EDITOR_PICTURES", &pictures);
    let ctl = Controller::new(settings.clone());
    let sessions = controller::list_sessions(&settings.sessions_dir);
    ctl.set_snapshot(idle_snapshot(settings, &sessions));
    let backend: Ctl = std::sync::Arc::new(Local::new(ctl));
    let handles = crate::gpu::Handles::of_offscreen(gpu);
    let mut run = Run::new(gpu, backend, scale);
    run.app.set_gpu(handles.clone());
    let t = run.time();
    run.app.set_page(Page::Sessions, t);
    run.until("the session list", 30.0, |run| run.app.sessions_mut().loaded())?;
    let ctx = run.ctx.clone();
    run.app.sessions_mut().open_first(&ctx, egui::Rect::from_min_size(egui::pos2(264.0, 160.0), egui::vec2(300.0, 265.0)));
    anyhow::ensure!(run.app.sessions_mut().open_dir().is_some(), "no session in {}", settings.sessions_dir.display());
    run.wait(1.0)?;
    run.app.sessions_mut().open_viewer(&ctx, &handles, false);
    run.until("the 3D view's splats", 120.0, |run| run.app.sessions_mut().viewer_loaded())?;
    run.wait(0.6)?;
    run.click("Save this view", 20.0)?;
    run.until("the view saved", 120.0, |run| orbiters_ui::widgets::control(&run.ctx, "Edit").is_some_and(|(_, on)| on))?;
    run.wait(0.4)?;
    run.shoot(out, "editor-3d-saved")?;
    run.click("Edit", 10.0)?;
    run.wait(5.0)?;
    run.shoot(out, "editor-3d-edit")?;
    Ok(())
}
