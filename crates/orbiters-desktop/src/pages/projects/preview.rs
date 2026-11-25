//! The 3D preview of a Unity project (contracts' Docs/project-preview.md, docs/project-preview.md): the scene its
//! owner last saved in Unity, from the small file Orbiters Toolkit keeps in the project. The selected project's hero
//! card shows it in place of its avatar pictures as soon as it loads (well under a second): the pictures give way to
//! a dark studio, a ring runs out over the ground and the scene rises out of it behind a green edge while the camera
//! swings round to the front. Dragging the card turns it (with inertia), a click grows it into a window over the page
//! (the card's shared geometry) where it also zooms and pans; Escape or the × shrinks it back into the card. The
//! project's UserSettings folder is watched, so a save in Unity shows here as soon as Toolkit wrote it. A project
//! without one says how to get it, and offers to add or update Orbiters Toolkit when it lacks one that writes them.
//! A preview that recorded its scene's lights is drawn in them when they show it (else in the studio's light, so an
//! avatar never shows black: `preview_gpu::light_mode`); the window's switch shows it in the studio's light
//! instead (remembered for every preview).
//!
//! Every scene of the project is listed (`orbiters_projects::scene::list`: the one Unity had open last first), each
//! with its preview: Toolkit's when it is at least as new as the scene (exact), else one built from the project's own
//! files on the background worker ([`super::builds`]), kept in the app's cache until a file it read changes. So a
//! project never opened with Toolkit, or never opened at all, has its 3D preview too; the window's chips switch
//! between the scenes, building the one picked if needed; a scene that cannot be built says why ("No preview: …").

use orbiters_i18n::{mark, tr, tr_n};
use super::builds::{self, Builder, Done};
use super::preview_gpu::{self, LightMode, Pipelines, Scene, View};
use super::state::State;
use crate::gpu::Handles;
use crate::pages::{Cx, Job};
use egui::{pos2, vec2, Align2, Color32, Id, Order, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::discovery::Project;
use orbiters_projects::preview::{self as file, Header, Kind};
use orbiters_projects::scene::{self as native, Slot, Source};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::viewer::{self, Morph};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, icons, paint};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// The rise out of the ground, and the camera's swing round to the front (seconds).
const REVEAL_S: f64 = 1.25;
const SWING_S: f32 = 1.6;
/// The avatar pictures giving way to the studio.
const FADE_S: f64 = 0.28;
/// How often the project's previews are looked at again besides the folder's watch (a save in Unity writes a new one;
/// the watch sees it at once).
const RELIST_S: f64 = 15.0;
/// A scene's build is not asked for again sooner than this after it was asked.
const REBUILD_S: f64 = 20.0;

/// A project's scenes as listed, the same as another listing when they show the same: the same scenes, each with the
/// same preview file (by its path, time and size; its header, every draw of the scene, is not compared) or the same
/// failure.
#[derive(Clone)]
struct Listing(Vec<Slot>);

impl PartialEq for Listing {
    fn eq(&self, other: &Self) -> bool {
        let same = |a: &file::Entry, b: &file::Entry| a.path == b.path && a.modified == b.modified && a.size == b.size;
        self.0.len() == other.0.len()
            && self.0.iter().zip(&other.0).all(|(a, b)| {
                a.scene == b.scene
                    && match (&a.source, &b.source) {
                        (Source::Toolkit(a), Source::Toolkit(b)) | (Source::Built(a), Source::Built(b)) | (Source::Stale(a), Source::Stale(b)) => same(a, b),
                        (Source::Failed(a), Source::Failed(b)) => a == b,
                        (Source::None, Source::None) => true,
                        _ => false,
                    }
            })
    }
}
/// The first Orbiters Toolkit that writes previews.
pub(super) const TOOLKIT_PREVIEWS: &str = "0.4.10";
const TOOLKIT: &str = "orbiters.toolkit";
/// The hint under the window's view goes after this long, or once the view was turned.
const HINT_S: f64 = 5.0;

#[derive(Default)]
pub(super) struct Previews {
    pipelines: Option<Arc<Pipelines>>,
    current: Option<Current>,
    /// The project shown before (switching back shows it at once).
    previous: Option<Current>,
    window: Option<Morph>,
    /// When the window opened (its hint).
    opened_at: f64,
    /// Builds from the projects' files and the cards' stills, in the background.
    builds: Builder,
    /// The gallery cards' stills, by project, and when each arrived (it fades in).
    stills: HashMap<PathBuf, (egui::TextureHandle, f64)>,
    /// Projects that get no still (no scene that builds).
    no_still: HashSet<PathBuf>,
    /// Screenshots: the project folder whose previews every project shows; read on the frame (no jobs). A real project
    /// (`ORBITERS_PREVIEW_PROJECT`) is listed and built on the frame too.
    fixture: Option<PathBuf>,
    /// Screenshots: builds wait (the building state is what is shown).
    hold_builds: bool,
    /// Screenshots: the cards' stills drawn on the frame.
    fixture_stills: bool,
    /// Where the window's scene chips were drawn (their GUIDs), for screenshots to press.
    scene_rects: Vec<(String, Rect)>,
    /// Whether previews that recorded their scene's lights are drawn in them (else the studio's): read from
    /// [`PREFS_FILE`] the first time it is needed.
    scene_light: Option<bool>,
    /// Where the window's light switch drew its two halves (scene, studio), for screenshots to press.
    light_switch: Option<[Rect; 2]>,
}

/// The person's preview choices, in the config folder.
const PREFS_FILE: &str = "project-preview.json";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prefs {
    /// Previews that recorded their scene's lights are drawn in them.
    #[serde(default = "yes")]
    scene_light: bool,
}

fn yes() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self { scene_light: true }
    }
}

impl Prefs {
    fn read(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        orbiters_core::write_atomic(path, &serde_json::to_vec_pretty(self).unwrap_or_default())
    }
}

impl Previews {
    /// Whether previews that recorded their scene's lights are drawn in them (the default), else in the studio's.
    fn prefer_scene_light(&mut self) -> bool {
        let fixture = self.fixture.is_some();
        *self.scene_light.get_or_insert_with(|| fixture || Prefs::read(&orbiters_core::paths::config_dir().join(PREFS_FILE)).scene_light)
    }

    /// The switch's choice, remembered (not for screenshots).
    fn set_scene_light(&mut self, on: bool) {
        self.scene_light = Some(on);
        if self.fixture.is_none() {
            let path = orbiters_core::paths::config_dir().join(PREFS_FILE);
            if let Err(e) = (Prefs { scene_light: on }).write(&path) {
                eprintln!("project preview: could not remember the light chosen in {}: {e}", path.display());
            }
        }
    }
}

struct Current {
    project: PathBuf,
    /// The project's scenes and their previews (`orbiters_projects::scene::list`).
    slots: Vec<Slot>,
    /// The scenes listed again on a thread while the preview shows, every [`RELIST_S`] and when the project's
    /// UserSettings changed: the panel is drawn again only when the list differs.
    listing: crate::pages::Watch<PathBuf, Listing>,
    listed_at: f64,
    /// The scene the person picked in the window (its GUID; else the one the project opens on).
    chosen: Option<String>,
    /// Builds asked for (scene GUID, when): not asked again while they run or just ran.
    requested: HashMap<String, f64>,
    /// The file shown or loading, and when it was written.
    file: Option<(PathBuf, SystemTime)>,
    /// A file loading, and the scene (GUID) it shows.
    loading: Job<Result<(Scene, String), String>>,
    scene: Option<Scene>,
    /// The scene (GUID) `scene` shows.
    shown_guid: Option<String>,
    error: Option<String>,
    /// When the scene arrived (its reveal), and whether the camera swings in (a first showing; a re-saved scene
    /// rises again where the camera is).
    shown_at: f64,
    camera: Option<Camera>,
    /// The person turned the view.
    touched: bool,
    /// The project's UserSettings watched: `changed` is set when something there changed.
    _watch: Option<file::Watch>,
    changed: Arc<std::sync::atomic::AtomicBool>,
}

impl Current {
    fn new(project: &Path) -> Self {
        Self {
            project: project.to_path_buf(),
            slots: vec![],
            listing: Default::default(),
            listed_at: f64::NEG_INFINITY,
            chosen: None,
            requested: HashMap::new(),
            file: None,
            loading: Job::default(),
            scene: None,
            shown_guid: None,
            error: None,
            shown_at: 0.0,
            camera: None,
            touched: false,
            _watch: None,
            changed: Arc::default(),
        }
    }

    /// The scene to show: the one picked, else the one the project opens on.
    fn slot(&self) -> Option<&Slot> {
        self.chosen.as_ref().and_then(|g| self.slots.iter().find(|s| &s.scene.guid == g)).or_else(|| builds::default_slot(&self.slots))
    }

    /// The scene shown now.
    fn shown_slot(&self) -> Option<&Slot> {
        let g = self.shown_guid.as_ref()?;
        self.slots.iter().find(|s| &s.scene.guid == g)
    }

    fn listed(&self) -> bool {
        self.listed_at > f64::NEG_INFINITY
    }
}

/// Where the scene to show stands.
#[derive(Clone, Debug, PartialEq)]
enum Status {
    /// Its preview is shown (or loads).
    Ready,
    /// Being built from the project's files (or waiting to be).
    Building,
    /// It cannot be built: why.
    Failed(native::Failure),
    /// The project has no scene.
    NoScene,
    /// Not listed yet.
    Listing,
}

/// An orbit around `pivot`: `yaw` about Unity's up axis (0 looks along +Z), `pitch` up from level.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Orbit {
    pivot: [f32; 3],
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov: f32,
}

impl Orbit {
    fn forward(&self) -> [f32; 3] {
        [self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), self.yaw.cos() * self.pitch.cos()]
    }

    fn eye(&self) -> [f32; 3] {
        let f = self.forward();
        [0, 1, 2].map(|k| self.pivot[k] - f[k] * self.distance)
    }

    fn lerp(&self, to: &Orbit, k: f32) -> Orbit {
        let l = |a: f32, b: f32| a + (b - a) * k;
        Orbit {
            pivot: [0, 1, 2].map(|i| l(self.pivot[i], to.pivot[i])),
            yaw: l(self.yaw, to.yaw),
            pitch: l(self.pitch, to.pitch),
            // Zoom eases in log space, so near and far feel alike.
            distance: (l(self.distance.ln(), to.distance.ln())).exp(),
            fov: l(self.fov, to.fov),
        }
    }

    fn near(&self, o: &Orbit, scale: f32) -> bool {
        let p = (0..3).map(|i| (self.pivot[i] - o.pivot[i]).abs()).fold(0.0, f32::max);
        (self.yaw - o.yaw).abs() < 1e-4 && (self.pitch - o.pitch).abs() < 1e-4 && (self.distance / o.distance - 1.0).abs() < 1e-4 && p < scale * 1e-4
    }
}

/// The camera: where it is, where it is going (drags and the wheel move the goal, the view follows it smoothly),
/// the spin left by a flick, and the framing a double click returns to.
struct Camera {
    now: Orbit,
    goal: Orbit,
    home: Orbit,
    /// Yaw and pitch per second, after a flick.
    spin: [f32; 2],
    /// How fast the view catches its goal (per second): slow while it swings in.
    rate: f32,
    /// The ground an avatar stands on, never seen from below.
    ground: Option<f32>,
    /// The size of the scene (zoom limits, panning).
    radius: f32,
    /// The last frame a drag moved the view (a release after resting there leaves no spin).
    dragged_at: f64,
}

impl Camera {
    fn new(h: &Header, swing: bool) -> Self {
        let home = home(h);
        let radius = focus_of(h).radius().max(0.05);
        let start = if swing { Orbit { yaw: home.yaw - 0.95, pitch: home.pitch - 0.22, distance: home.distance * 1.45, ..home } } else { home };
        let ground = h.ground.filter(|_| h.kind == Kind::Avatars);
        Self { now: start, goal: home, home, spin: [0.0; 2], rate: if swing { 5.0 / SWING_S } else { 14.0 }, ground, radius, dragged_at: 0.0 }
    }

    /// One frame: the flick's spin, the view towards the goal. True while it moves.
    fn step(&mut self, dt: f32, dragging: bool) -> bool {
        if !dragging && (self.spin[0] != 0.0 || self.spin[1] != 0.0) {
            self.goal.yaw += self.spin[0] * dt;
            self.goal.pitch += self.spin[1] * dt;
            let decay = (-dt * 3.2).exp();
            self.spin = self.spin.map(|s| s * decay);
            if self.spin[0].abs() < 0.015 && self.spin[1].abs() < 0.015 {
                self.spin = [0.0; 2];
            }
        }
        // Looking up at most as far as keeps the eye above the ground.
        let top = self.ground.map_or(1.45, |g| ((self.goal.pivot[1] - g - 0.02) / self.goal.distance).clamp(-1.0, 1.0).asin());
        self.goal.pitch = self.goal.pitch.clamp(-1.45, top.max(-1.45));
        let k = 1.0 - (-dt * self.rate).exp();
        self.now = self.now.lerp(&self.goal, k);
        // The swing-in eases to the usual follow rate.
        self.rate = (self.rate + dt * 9.0).min(if dragging { 30.0 } else { 14.0 });
        let moving = self.spin != [0.0; 2] || !self.now.near(&self.goal, self.radius);
        if !moving {
            self.now = self.goal;
        }
        moving
    }

    /// Back to the framing it opened with, the short way round.
    fn reset(&mut self) {
        let turns = ((self.now.yaw - self.home.yaw) / std::f32::consts::TAU).round();
        self.goal = Orbit { yaw: self.home.yaw + turns * std::f32::consts::TAU, ..self.home };
        self.spin = [0.0; 2];
        self.rate = 7.0;
    }
}

/// What a preview frames first: the avatars, else everything.
fn focus_of(h: &Header) -> file::Bounds {
    h.focus
}

/// The framing a preview opens with: avatars from the front and a little to their left, slightly from above, whole
/// with some room; worlds and scenes from their Scene view's camera, else over a corner of their bounds.
fn home(h: &Header) -> Orbit {
    let b = focus_of(h);
    let size = b.size();
    let fov = 30f32.to_radians();
    if h.kind == Kind::Avatars {
        let ground = h.ground.unwrap_or(b.min[1]);
        let height = (b.max[1] - ground).max(0.2);
        let f = h.avatars.first().map(|a| a.forward).filter(|f| f[0].abs() + f[2].abs() > 1e-3).unwrap_or([0.0, 0.0, 1.0]);
        let yaw = (-f[0]).atan2(-f[2]) + 0.42;
        let pitch = -0.1f32;
        let c = b.center();
        let pivot = [c[0], ground + height * 0.5, c[2]];
        let wide = size[0].max(size[2]);
        // The height in seven tenths of the view (the width in a 4:3 one), room for the card's title below.
        let fit = (height * 1.4).max(wide * 1.25 * 0.75) * 0.5 / (fov * 0.5).tan();
        return Orbit { pivot, yaw, pitch, distance: fit + wide * 0.5, fov };
    }
    if let Some(cam) = h.camera {
        let [x, y, z, w] = cam.rotation;
        // The camera's +Z.
        let f = [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)];
        let f = preview_gpu::normalize(f);
        let d = (0..3).map(|k| (cam.pivot[k] - cam.position[k]).powi(2)).sum::<f32>().sqrt().max(0.1);
        return Orbit {
            pivot: [0, 1, 2].map(|k| cam.position[k] + f[k] * d),
            yaw: f[0].atan2(f[2]),
            pitch: f[1].clamp(-1.0, 1.0).asin(),
            distance: d,
            fov: cam.fov_y.clamp(20.0, 100.0).to_radians(),
        };
    }
    let r = b.radius().max(0.5);
    Orbit { pivot: b.center(), yaw: 0.7, pitch: -0.45, distance: r * 1.15 / (fov * 0.5).tan(), fov }
}

impl Previews {
    /// Keeps `project`'s scenes listed, the one to show built (on the worker) and loaded.
    fn follow(&mut self, ctx: &egui::Context, gpu: &Handles, project: &Path) {
        let now = ctx.input(|i| i.time);
        if self.current.as_ref().is_none_or(|c| c.project != project) {
            let before = self.current.take();
            self.current = Some(match self.previous.take() {
                Some(mut p) if p.project == project => {
                    // Back to a project shown a moment ago: there at once, a short rise.
                    p.shown_at = now - REVEAL_S * 0.45;
                    p
                }
                _ => Current::new(project),
            });
            self.previous = before;
            self.window = None;
        }
        let fixture = self.fixture.clone();
        if fixture.is_none() {
            self.builds.start(ctx, &gpu.device, &gpu.queue);
        }
        self.receive(ctx);
        let scene_light = self.prefer_scene_light();
        self.builds.set_scene_light(scene_light);
        let c = self.current.as_mut().unwrap();
        let folder = fixture.clone().unwrap_or_else(|| project.to_path_buf());
        if fixture.is_some() {
            if !c.listed() {
                c.slots = native::list(&folder);
                c.listed_at = now;
            }
        } else {
            if c._watch.is_none() {
                let (changed, ctx) = (c.changed.clone(), ctx.clone());
                c._watch = Some(file::watch(project, move || {
                    changed.store(true, std::sync::atomic::Ordering::Relaxed);
                    ctx.request_repaint();
                }));
            }
            // Something changed in the project's UserSettings: listed now, not at the next period.
            if c.changed.swap(false, std::sync::atomic::Ordering::Relaxed) {
                c.listing.look_now();
            }
            let listed = c.listing.keep(ctx, &folder, std::time::Duration::from_secs_f64(RELIST_S), || {
                let folder = folder.clone();
                move || Listing(native::list(&folder))
            });
            if let Some(Listing(slots)) = listed {
                c.slots = slots;
                c.listed_at = now;
            }
        }
        // The scene to show: built first when it has no preview yet (or an outdated one, shown meanwhile).
        // Only the scene is copied (this runs every frame; the slot's preview header holds every draw of the scene).
        if let Some(scene) = c.slot().filter(|s| s.source.wants_build()).map(|s| s.scene.clone()) {
            let guid = scene.guid.clone();
            if fixture.is_some() {
                // Screenshots of a real project: built on the frame (unless the building state is the picture).
                if !self.hold_builds {
                    let result = native::build_scene(&folder, &scene, &native::Limits::default());
                    apply_build(c, &guid, result);
                }
            } else if !self.builds.building(project, &guid) {
                match c.requested.get(&guid).map(|t| REBUILD_S - (now - t)).filter(|left| *left > 0.0) {
                    // Just asked, and not building (it failed for want of time): asked again once that long passed, on
                    // a frame asked for here.
                    Some(left) => ctx.request_repaint_after(std::time::Duration::from_secs_f64(left)),
                    None => {
                        self.builds.build(project, &scene);
                        c.requested.insert(guid, now);
                    }
                }
            }
        }
        let want = c.slot().and_then(|s| {
            let built = matches!(s.source, Source::Built(_) | Source::Stale(_));
            s.source.entry().map(|e| (e.path.clone(), e.modified, s.scene.guid.clone(), built))
        });
        if let Some((path, modified, guid, built)) = want.filter(|w| c.file.as_ref() != Some(&(w.0.clone(), w.1))) {
            if !c.loading.running() {
                c.file = Some((path.clone(), modified));
                c.error = None;
                let (device, queue, pipelines) = (gpu.device.clone(), gpu.queue.clone(), self.pipelines.clone());
                let owner = folder.clone();
                let load = move || {
                    // Shown: the cache keeps it among the ones used last.
                    if built {
                        native::touch(&owner, &guid);
                    }
                    preview_gpu::load(&device, &queue, pipelines, &path).map(|s| (s, guid)).map_err(|e| format!("{e:#}"))
                };
                if fixture.is_some() {
                    // Screenshots wait for it.
                    let result = load();
                    c.loading = Job::spawn(ctx, move || result);
                    while c.loading.poll().is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                } else {
                    c.loading = Job::spawn(ctx, load);
                }
            }
        }
        if let Some(result) = c.loading.take() {
            match result {
                Ok((scene, guid)) => {
                    let t = scene.timings;
                    eprintln!(
                        "project preview {}: {:.0} ms (read {:.0}, pictures {:.0}, upload {:.0}, pipelines {:.0}), {:.1} MB",
                        scene.header.scene.name,
                        t.total_ms,
                        t.read_ms,
                        t.decode_ms,
                        t.upload_ms,
                        t.pipelines_ms,
                        t.bytes as f64 / 1e6
                    );
                    self.pipelines.get_or_insert_with(|| scene.pipelines());
                    let first = c.camera.is_none();
                    if first || c.shown_guid.as_ref() != Some(&guid) {
                        c.camera = Some(Camera::new(&scene.header, true));
                    }
                    c.shown_at = now;
                    c.scene = Some(scene);
                    c.shown_guid = Some(guid);
                    c.error = None;
                }
                Err(e) => {
                    eprintln!("project preview: {e}");
                    c.error = Some(e);
                }
            }
        }
    }

    /// What the worker finished: a build lands in the project's scenes at once (and they are listed again), a still
    /// on its card.
    fn receive(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for done in self.builds.poll() {
            match done {
                Done::Built { project, guid, result } => {
                    for c in [self.current.as_mut(), self.previous.as_mut()].into_iter().flatten().filter(|c| c.project == project) {
                        apply_build(c, &guid, result.clone());
                        c.changed.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Done::Still { project, image } => {
                    let tex = ctx.load_texture(format!("project-still-{}", project.display()), image, egui::TextureOptions::LINEAR);
                    let at = self.stills.get(&project).map_or(now, |s| s.1);
                    self.stills.insert(project, (tex, at));
                }
                Done::NoStill { project } => {
                    self.no_still.insert(project);
                }
            }
        }
    }

    /// The status of the scene `c` is to show.
    fn status(&self, c: &Current) -> Status {
        if !c.listed() {
            return Status::Listing;
        }
        let Some(slot) = c.slot() else { return Status::NoScene };
        match &slot.source {
            Source::Failed(why) => Status::Failed(why.clone()),
            Source::None => Status::Building,
            _ => Status::Ready,
        }
    }

    /// A gallery card's still (a project without avatar pictures), once drawn: its texture and how far it faded in.
    pub(super) fn still(&self, ctx: &egui::Context, project: &Path) -> Option<(&egui::TextureHandle, f32)> {
        let (tex, at) = self.stills.get(project)?;
        let k = (anim::since(ctx, *at, FADE_S) / FADE_S).clamp(0.0, 1.0) as f32;
        if k < 1.0 {
            ctx.request_repaint();
        }
        Some((tex, k))
    }

    /// Starts the background worker (the gallery's stills need it before any project is selected) and takes what it
    /// finished.
    pub(super) fn start(&mut self, ctx: &egui::Context, gpu: &Handles) {
        if self.fixture.is_none() && !self.fixture_stills {
            self.builds.start(ctx, &gpu.device, &gpu.queue);
        }
        self.receive(ctx);
    }

    /// Asks for `project`'s card still (once a session; the worker draws it after the builds). Screenshots draw it on
    /// the frame.
    pub(super) fn want_still(&mut self, ctx: &egui::Context, gpu: Option<&Handles>, project: &Path) {
        if self.no_still.contains(project) || self.stills.contains_key(project) {
            return;
        }
        if self.fixture_stills {
            let Some(gpu) = gpu else { return };
            match builds::still_now(project, &gpu.device, &gpu.queue) {
                Some(image) => {
                    let tex = ctx.load_texture(format!("project-still-{}", project.display()), image, egui::TextureOptions::LINEAR);
                    self.stills.insert(project.to_path_buf(), (tex, f64::NEG_INFINITY));
                }
                None => {
                    self.no_still.insert(project.to_path_buf());
                }
            }
            return;
        }
        if self.fixture.is_none() {
            self.builds.still(project);
        }
    }
}

/// A finished build in `c`'s scenes: its preview, or why there is none.
fn apply_build(c: &mut Current, guid: &str, result: Result<file::Entry, native::Failure>) {
    if let Some(slot) = c.slots.iter_mut().find(|s| s.scene.guid == guid) {
        slot.source = match result {
            Ok(e) => Source::Built(e),
            Err(why) => Source::Failed(why),
        };
    }
}

/// The view a preview opens on, fully risen (a gallery card's still: the 3D view's own framing, avatars a little
/// closer, the card being smaller and its title below the picture rather than over it).
pub(super) fn home_view(h: &Header, light: LightMode) -> View {
    let mut o = home(h);
    if h.kind == Kind::Avatars {
        o.distance *= 0.82;
    }
    View { eye: o.eye(), forward: o.forward(), fov_y: o.fov, shift: 0.0, reveal: 1.0, light }
}

/// Moves `camera` with the pointer over `resp`: a drag turns it (a flick keeps it spinning a moment), with `free` the
/// wheel or a pinch zooms, a right or middle drag pans; a double click frames the scene again.
fn steer(ctx: &egui::Context, resp: &egui::Response, camera: &mut Camera, rect: Rect, free: bool) -> bool {
    if free {
        // Its middle drag pans: the wheel button's scrolling (`autoscroll`) does not start here.
        orbiters_ui::autoscroll::keep_middle(ctx, resp.rect);
    }
    let (dt, now) = ctx.input(|i| (i.stable_dt.clamp(1.0 / 240.0, 0.05), i.time));
    let mut touched = false;
    let d = resp.drag_delta();
    let turning = resp.dragged_by(egui::PointerButton::Primary);
    if turning && d != egui::Vec2::ZERO {
        let k = 0.0085;
        camera.goal.yaw += d.x * k;
        camera.goal.pitch -= d.y * k;
        let v = [d.x * k / dt, -d.y * k / dt];
        camera.spin = [0, 1].map(|i| camera.spin[i] * 0.55 + v[i] * 0.45);
        camera.dragged_at = now;
        touched = true;
    }
    if turning && d == egui::Vec2::ZERO && now - camera.dragged_at > 0.08 {
        camera.spin = [0.0; 2];
    }
    if free {
        let o = camera.goal;
        let per_point = o.distance * 2.0 * (0.5 * o.fov).tan() / rect.height().max(1.0);
        if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
            let f = o.forward();
            let right = preview_gpu::normalize([f[2], 0.0, -f[0]]);
            let up = [-f[0] * f[1], 1.0 - f[1] * f[1], -f[2] * f[1]];
            let up = preview_gpu::normalize(up);
            for k in 0..3 {
                camera.goal.pivot[k] += -right[k] * d.x * per_point + up[k] * d.y * per_point;
            }
            touched = true;
        }
        if resp.hovered() {
            let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            if scroll != 0.0 || pinch != 1.0 {
                let r = camera.radius;
                camera.goal.distance = (camera.goal.distance * (-scroll * 0.0018).exp() / pinch).clamp(r * 0.01, r * 40.0);
                touched = true;
            }
        }
    }
    if resp.double_clicked() {
        camera.reset();
        touched = true;
    }
    if touched {
        camera.rate = camera.rate.max(14.0);
    }
    touched
}

/// The view `c`'s camera shows now (after this frame's step), and whether it still moves.
fn frame_view(ctx: &egui::Context, c: &mut Current, dragging: bool, shift: f32, prefer_scene_light: bool) -> (View, bool) {
    let light = c.scene.as_ref().map_or(LightMode::Studio, |s| preview_gpu::light_mode(prefer_scene_light, &s.header));
    let dt = ctx.input(|i| i.stable_dt.clamp(1.0 / 240.0, 0.05));
    let cam = c.camera.as_mut().unwrap();
    let moving = cam.step(dt, dragging);
    let reveal = (anim::since(ctx, c.shown_at, REVEAL_S) / REVEAL_S).clamp(0.0, 1.0) as f32;
    (View { eye: cam.now.eye(), forward: cam.now.forward(), fov_y: cam.now.fov, shift, reveal, light }, moving)
}

fn pixels(ctx: &egui::Context, rect: Rect) -> (u32, u32) {
    let ppp = ctx.pixels_per_point();
    (((rect.width() * ppp).round() as u32).max(16), ((rect.height() * ppp).round() as u32).max(16))
}

/// The hero card's cover (`rect`, squircle corners `radii`): the project's 3D preview over its avatar pictures once it
/// loads, turning under a drag; a press opens it in a window. Without a preview, a quiet note on how to get one.
pub(super) fn cover(ui: &mut Ui, cx: &Cx, state: &mut State, project: &Project, rect: Rect, radii: [f32; 4], sheet: bool) {
    let Some(gpu) = cx.gpu else { return };
    let ctx = ui.ctx().clone();
    state.anchors.insert("preview-cover".into(), rect);
    state.preview.follow(&ctx, gpu, &project.path);
    // The project's Orbiters Toolkit (none, too old to write previews, or one that does), once its packages were read,
    // and the newest release the repositories offer.
    let read = !state.view_job.running() && !state.view.packages.is_empty();
    let toolkit = read.then(|| state.view.packages.iter().find(|p| p.name == TOOLKIT).cloned()).map(|p| {
        let installed = p.as_ref().and_then(|p| p.installed.clone());
        let newest = p.as_ref().and_then(|p| p.versions.iter().filter(|v| !v.contains('-')).max_by_key(|v| version(v)).cloned());
        (installed, newest.filter(|v| version(v) >= version(TOOLKIT_PREVIEWS)))
    });
    let updating = state.busy_package.as_deref() == Some(TOOLKIT) && state.running();
    let can_act = project.registered && !state.running();
    let mut act: Option<String> = None;
    let window_open = state.preview.window.is_some();
    let pv = &mut state.preview;
    let scene_light = pv.prefer_scene_light();
    let status = pv.current.as_ref().map_or(Status::Listing, |c| pv.status(c));
    let c = pv.current.as_mut().unwrap();
    let corner = rect.right() - 12.0 - if sheet { 40.0 } else { 0.0 };
    let id = Id::unique(("project-preview-cover", &project.path));
    if c.scene.is_none() {
        let scene_name = c.slot().map(|s| s.scene.name.clone()).unwrap_or_default();
        // The notes on the cover, right to left: why there is no preview, and an offer of Orbiters Toolkit.
        let mut notes: Vec<Note> = Vec::new();
        let busy = c.loading.running() || status == Status::Listing;
        if c.loading.running() {
        } else if let Some(e) = &c.error {
            notes.push(Note::new(icons::WARNING, tr("3D preview unreadable").into(), tr!("Its file could not be read: {error}", error = e)));
        } else {
            match &status {
                Status::NoScene => {
                    notes.push(Note::new(icons::CUBE, tr("No scene to preview").into(), tr("This project has no scene (.unity file) under Assets yet.").into()))
                }
                Status::Building => notes.push(Note {
                    spinner: true,
                    ..Note::new(
                        icons::CUBE,
                        tr("Building 3D preview…").into(),
                        tr!(
                            "Reading {scene} from the project's files: its prefabs, models, materials and textures. Kept once built, until one of them changes; the first time on a hard disk can take a minute.",
                            scene = scene_name
                        ),
                    )
                }),
                Status::Failed(why) => {
                    let offer = match &toolkit {
                        Some((None, newest)) => Some((
                            icons::PLUS,
                            if updating { tr("Adding Toolkit…") } else { tr("Add Toolkit for 3D") },
                            if newest.is_some() {
                                tr("Orbiters Toolkit draws the scene you save in Unity here, exactly. Press to add it, then save the scene once.").to_string()
                            } else {
                                tr("Orbiters Toolkit draws the scene you save in Unity here, exactly. Add the Orbiters repository, then Orbiters Toolkit below.").to_string()
                            },
                            newest.clone(),
                        )),
                        Some((Some(v), newest)) if version(v) < version(TOOLKIT_PREVIEWS) => Some((
                            super::art::glyph::ARROW_UP,
                            if updating { tr("Updating Toolkit…") } else { tr("Update Toolkit for 3D") },
                            match newest {
                                Some(n) => tr!(
                                    "Orbiters Toolkit {version} in this project does not draw 3D previews yet ({previews} does). Press to update it to {newest}: with the project open in Unity, its scene shows here once the editor reloaded.",
                                    version = v,
                                    previews = TOOLKIT_PREVIEWS,
                                    newest = n
                                ),
                                None => tr!(
                                    "Orbiters Toolkit {version} in this project does not draw 3D previews yet ({previews} does). Refresh the Orbiters repository to get the newer one.",
                                    version = v,
                                    previews = TOOLKIT_PREVIEWS
                                ),
                            },
                            newest.clone(),
                        )),
                        _ => None,
                    };
                    let hint = if why.out_of_time {
                        // `scene::RETRY_AFTER`: what the first try read is kept, so the next gets further.
                        tr("It is tried again in half an hour (the first look at a project on a hard disk is the slowest), or as soon as the scene is saved.")
                    } else if offer.is_some() {
                        tr("Orbiters Toolkit draws it exactly each time the scene is saved in Unity.")
                    } else {
                        if cfg!(target_os = "macos") {
                            tr("Save the scene in Unity (Cmd+S): Orbiters Toolkit draws it exactly and it shows here a moment later.")
                        } else {
                            tr("Save the scene in Unity (Ctrl+S): Orbiters Toolkit draws it exactly and it shows here a moment later.")
                        }
                    };
                    notes.push(Note {
                        short: Some(tr("No 3D preview")),
                        ..Note::new(icons::WARNING, tr!("No preview: {why}", why = why), tr!("{scene}: no 3D preview, {why}.\n\n{hint}", scene = scene_name, why = why, hint = hint))
                    });
                    if let Some((glyph, label, tip, offer)) = offer {
                        notes.push(Note { offer, spinner: updating, ..Note::new(glyph, label.into(), tip) });
                    }
                }
                Status::Ready | Status::Listing => {}
            }
        }
        let k = anim::spring_from(&ctx, id.with("note"), [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
        let mut right = corner;
        // Clear of the kind's badge in the top left corner (the hero's).
        let badge = widgets::text_size(ui, &Txt::new(project.kind.label(), 10.5).weight(weight::SEMIBOLD).tracking(0.02)).x + 36.0;
        let left = rect.left() + 12.0 + badge + 8.0;
        // The offer is the rightmost; the reason fills what is left, or says only that there is none (why on hover).
        for (i, note) in notes.iter().enumerate().rev() {
            let room = (right - left).max(80.0);
            let full = Txt::new(&note.label, 11.5).weight(weight::MEDIUM);
            let fits = widgets::text_size(ui, &full).x + 36.0 <= room;
            let label = match note.short {
                Some(short) if !fits => short,
                _ => note.label.as_str(),
            };
            let txt = Txt::new(label, 11.5).weight(weight::MEDIUM);
            let w = (widgets::text_size(ui, &txt).x + 36.0).min(room);
            let r = Rect::from_min_size(pos2(right - w, rect.top() + 12.0), vec2(w, 24.0));
            right = r.left() - 6.0;
            let actionable = note.offer.is_some() && can_act;
            let nid = id.with(("note", i));
            let resp = ui.interact(r, nid, if actionable { Sense::click() } else { Sense::hover() });
            let hover = anim::toggle(&ctx, nid.with("hover"), actionable && resp.hovered(), Spring::SOFT);
            let press = anim::toggle(&ctx, nid.with("press"), actionable && resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let lit = if note.offer.is_some() { 1.0 } else { 0.0 };
            let r = r.shrink(1.5 * press);
            let p = ui.painter();
            p.rect_filled(r, theme::radius(12.0), theme::mix(theme::black(0.55 * k), theme::alpha(theme::ink_900(), 0.9 * k), lit));
            if lit > 0.0 {
                p.rect_filled(r, theme::radius(12.0), theme::alpha(theme::accent(), (0.14 + 0.1 * hover) * k));
            }
            let fg = theme::mix(theme::mix(theme::fg_2(), theme::accent(), lit), theme::fg(), hover * 0.4);
            let warn = note.glyph == icons::WARNING;
            if note.spinner {
                feedback::spinner(p, pos2(r.left() + 15.0, r.center().y), 5.0, 1.6, theme::alpha(if lit > 0.0 { theme::accent() } else { theme::fg_2() }, k));
                ctx.request_repaint();
            } else {
                let tone = if warn { theme::WARN } else { theme::mix(theme::fg_3(), theme::accent(), lit) };
                widgets::icon(p, pos2(r.left() + 15.0, r.center().y - 1.5 * hover * lit), note.glyph, 12.0, theme::alpha(tone, k));
            }
            widgets::text_truncated(p, pos2(r.left() + 25.0, r.center().y), Align2::LEFT_CENTER, &txt.color(theme::alpha(fg, k)), r.width() - 25.0 - 10.0);
            let resp = if actionable { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
            resp.clone().on_hover_text(&note.tip);
            if actionable && widgets::activated(&resp) {
                act = note.offer.clone();
            }
        }
        if notes.is_empty() && busy {
            let r = Rect::from_center_size(pos2(corner - 14.0, rect.top() + 24.0), vec2(28.0, 28.0));
            ui.painter().circle_filled(r.center(), 14.0, theme::black(0.5));
            feedback::spinner(ui.painter(), r.center(), 7.0, 2.0, theme::fg_2());
        }
        if let Some(v) = act {
            let installed = toolkit.as_ref().and_then(|t| t.0.clone());
            let label = if installed.is_some() { tr!("Updating Orbiters Toolkit to {version}…", version = v) } else { tr!("Adding Orbiters Toolkit {version}…", version = v) };
            let change = orbiters_projects::vpm::Change::Install { names: vec![(TOOLKIT.into(), Some(v))], prerelease: false };
            super::panel::package_action(state, &ctx, project, change, &label, TOOLKIT);
        }
        return;
    }
    if window_open {
        // The window grew out of here: its place stays dimmed until it is back.
        feedback::fill_corners(ui.painter(), rect, radii, theme::ink_950());
        return;
    }
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let camera = c.camera.as_mut().unwrap();
    c.touched |= steer(&ctx, &resp, camera, rect, false);
    let dragging = resp.dragged();
    // Above the title and its shade.
    let (view, moving) = frame_view(&ctx, c, dragging, 0.1, scene_light);
    if moving {
        ctx.request_repaint();
    }
    let scene = c.scene.as_mut().unwrap();
    let (tex, uv) = scene.render(gpu, pixels(&ctx, rect), view);
    let k = (anim::since(&ctx, c.shown_at, FADE_S) / FADE_S).clamp(0.0, 1.0) as f32;
    paint::squircle_texture(ui.painter(), rect, radii.map(theme::squircle), tex, uv, Color32::WHITE.gamma_multiply(k));
    let resp = resp.on_hover_cursor(if dragging { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    // A press without a drag, or the corner button, opens the window.
    let button = Rect::from_center_size(pos2(corner - 16.0, rect.top() + 28.0), vec2(32.0, 32.0));
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() || resp.dragged(), Spring::SOFT);
    let mut bu = ui.new_child(UiBuilder::new().max_rect(button).id_salt(id.with("open")));
    bu.multiply_opacity(k * (0.75 + 0.25 * hover));
    let open =
        widgets::activated(&IconButton::new(icons::CORNERS_OUT, mark("Look around in 3D")).size(32.0, 15.0).backdrop(theme::black(0.55)).show_at(&mut bu, button));
    if (open || resp.clicked()) && k > 0.5 {
        pv.open_window(&ctx, rect, radii[0]);
    }
}

/// A note on the cover where the preview would be.
struct Note {
    glyph: &'static str,
    label: String,
    tip: String,
    /// What pressing it installs (Orbiters Toolkit's version).
    offer: Option<String>,
    spinner: bool,
    /// What the label says when it does not fit.
    short: Option<&'static str>,
}

impl Note {
    fn new(glyph: &'static str, label: String, tip: String) -> Self {
        Self { glyph, label, tip, offer: None, spinner: false, short: None }
    }
}

impl Previews {
    fn open_window(&mut self, ctx: &egui::Context, origin: Rect, radius: f32) {
        static OPENED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.window = Some(Morph::new(Id::unique(("project-preview-window", n)), origin, radius));
        self.opened_at = ctx.input(|i| i.time);
    }
}

/// The window the preview grows into, over the page: the view filling it, its title, the scenes to switch between,
/// Open in Unity, and how to move around.
pub(super) fn window(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let Some(gpu) = cx.gpu else { return };
    let ctx = ui.ctx().clone();
    let origin = state.anchors.get("preview-cover").copied();
    let Some(project) = state.scan.projects.iter().find(|p| Some(&p.path) == state.selected.as_ref()).cloned() else {
        state.preview.window = None;
        return;
    };
    let busy = state.running();
    let pv = &mut state.preview;
    let scene_light = pv.prefer_scene_light();
    let mut light_switch = None;
    let scenes = pv.current.as_ref().map(|c| scene_chips(c, &pv.builds)).unwrap_or_default();
    let mut rects_out: Vec<(String, Rect)> = Vec::new();
    let Some(morph) = pv.window.as_mut() else { return };
    let Some(c) = pv.current.as_mut().filter(|c| c.scene.is_some()) else {
        pv.window = None;
        return;
    };
    if let Some(o) = origin {
        morph.set_origin(o);
    }
    let screen = ctx.content_rect();
    let area = Rect::from_min_max(pos2(screen.left(), screen.top() + crate::app::TITLE_H), screen.max);
    let target = area.shrink2(vec2((area.width() * 0.04).clamp(16.0, 56.0), (area.height() * 0.04).clamp(14.0, 40.0)));
    let f = morph.frame(&ctx, target);
    if f.closed {
        pv.window = None;
        return;
    }
    let (wid, closing) = (morph.id(), morph.is_closing());
    let mut close = false;
    let mut act: Option<WindowAct> = None;
    orbiters_ui::front::area(&ctx, wid.with("area"), Order::Foreground, !closing).fixed_pos(area.min).fade_in(false).show(&ctx, |ui| {
        ui.set_min_size(area.size());
        let backdrop = ui.interact(area, wid.with("backdrop"), Sense::click());
        viewer::paint_backdrop(&ui.painter().with_clip_rect(screen), screen, &f);
        if widgets::pressed(&backdrop) && !f.rect.contains(backdrop.interact_pointer_pos().unwrap_or_default()) {
            close = true;
        }
        let k = f.t.clamp(0.0, 1.0);
        paint::shadow(ui.painter(), f.rect, f.radius, vec2(0.0, 24.0 * k), 48.0, -8.0, theme::black(0.5 * k));
        let resp = ui.interact(f.rect, wid.with("view"), if closing { Sense::hover() } else { Sense::click_and_drag() });
        let camera = c.camera.as_mut().unwrap();
        if !closing && f.content > 0.5 {
            c.touched |= steer(&ctx, &resp, camera, f.rect, true);
        }
        // Clear of the facts and buttons along the bottom.
        let (view, moving) = frame_view(&ctx, c, resp.dragged(), 0.07, scene_light);
        if moving {
            ctx.request_repaint();
        }
        let scene = c.scene.as_mut().unwrap();
        let (tex, uv) = scene.render(gpu, pixels(&ctx, f.rect), view);
        paint::squircle_texture(ui.painter(), f.rect, [theme::squircle(f.radius); 4], tex, uv, Color32::WHITE);
        let _ = resp.on_hover_cursor(if c.touched && ctx.input(|i| i.pointer.any_down()) { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });

        // A scene picked that is not shown yet: building, loading, or why it has none (the one before stays, dimmed).
        let pending = scenes.chips.iter().find(|ch| Some(&ch.guid) == scenes.selected.as_ref() && Some(&ch.guid) != c.shown_guid.as_ref());
        let pk = anim::toggle(&ctx, wid.with("pending"), pending.is_some() && !closing, Spring::SOFT) * f.content;
        if let Some(ch) = pending.filter(|_| pk > 0.01) {
            ui.painter().rect_filled(f.rect, theme::radius(f.radius), theme::black(0.45 * pk));
            let (glyph, text, spin) = match &ch.state {
                ChipState::Failed(why) => (icons::WARNING, tr!("No preview of {name}: {why}", name = ch.name, why = why), false),
                ChipState::Ready(_) => (icons::CUBE, tr!("Opening {name}…", name = ch.name), true),
                _ => (icons::CUBE, tr!("Building {name} from the project's files…", name = ch.name), true),
            };
            let txt = Txt::new(&text, 13.0).weight(weight::MEDIUM).color(theme::alpha(theme::fg(), pk));
            let w = (widgets::text_size(ui, &txt).x + 56.0).min(f.rect.width() - 80.0);
            let r = Rect::from_center_size(f.rect.center(), vec2(w, 40.0));
            paint::fill_squircle(ui.painter(), r, theme::squircle(20.0), theme::black(0.65 * pk));
            if spin {
                feedback::spinner(ui.painter(), pos2(r.left() + 22.0, r.center().y), 6.0, 1.8, theme::alpha(theme::accent(), pk));
                ctx.request_repaint();
            } else {
                widgets::icon(ui.painter(), pos2(r.left() + 22.0, r.center().y), glyph, 14.0, theme::alpha(theme::WARN, pk));
            }
            widgets::text_truncated(ui.painter(), pos2(r.left() + 38.0, r.center().y), Align2::LEFT_CENTER, &txt, r.width() - 52.0);
        }
        let mut cu = ui.new_child(UiBuilder::new().max_rect(f.rect).id_salt(wid.with("chrome")));
        cu.set_clip_rect(f.rect.intersect(ui.clip_rect()));
        cu.multiply_opacity(f.content);
        if closing || f.content < 0.5 {
            cu.disable();
        }
        let header = &c.scene.as_ref().unwrap().header;
        // When the scene was saved (a preview built here is newer than its scene).
        let shown = c.shown_slot();
        let written = shown.map(|s| s.scene.modified).or(c.file.as_ref().map(|f| f.1));
        let source = shown.map(|s| &s.source);
        // The light switch, when the file recorded its scene's lights (Toolkit's, or the directional lights and gradient
        // or flat ambient light a built one read); its Scene light only when they show the scene.
        let light = header.lighting.as_ref().map(|_| LightSwitch { prefer_scene: scene_light, scene_shows: preview_gpu::scene_light_shows_header(&header) });
        let mut scene_rects = Vec::new();
        chrome(&mut cu, cx, &project, header, written, source, &scenes, &mut scene_rects, busy, f.rect, light, &mut light_switch, &mut act);
        rects_out = scene_rects;
        if viewer::close_button(&mut cu, f.rect, tr("Close")) {
            close = true;
        }
        // How to move around, until the view was turned.
        let shown = !c.touched && ctx.input(|i| i.time) - pv.opened_at < HINT_S;
        let hk = anim::toggle(&ctx, wid.with("hint"), shown, Spring::SOFT) * f.content;
        if hk > 0.01 {
            let txt = Txt::new(tr("Drag to turn  ·  right-drag to move  ·  scroll to zoom  ·  double-click to frame"), 12.0).color(theme::alpha(theme::fg_2(), hk));
            let w = widgets::text_size(ui, &txt).x + 32.0;
            let y = f.rect.bottom() - 26.0 - if scenes.chips.len() > 1 { 52.0 } else { 0.0 } + 8.0 * (1.0 - hk);
            let r = Rect::from_center_size(pos2(f.rect.center().x, y), vec2(w, 30.0));
            if w < f.rect.width() - 360.0 {
                cu.painter().rect_filled(r, theme::radius(15.0), theme::black(0.5 * hk));
                widgets::text(cu.painter(), r.center(), Align2::CENTER_CENTER, &txt);
            }
            if shown {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(0.5));
            }
        }
    });
    if !closing && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !orbiters_ui::popup::any_open(&ctx) {
        close = true;
    }
    pv.light_switch = light_switch;
    pv.scene_rects = rects_out;
    match act {
        Some(WindowAct::SceneLight(on)) => {
            state.preview.set_scene_light(on);
            ctx.request_repaint();
        }
        Some(WindowAct::Scene(guid)) => {
            if let Some(c) = state.preview.current.as_mut() {
                c.chosen = Some(guid);
            }
            ctx.request_repaint();
        }
        Some(WindowAct::Reset) => {
            if let Some(cam) = c.camera.as_mut() {
                cam.reset();
            }
        }
        Some(WindowAct::Open) => {
            state.open_unity(&ctx, &project, false);
        }
        None => {}
    }
    if close {
        if let Some(m) = state.preview.window.as_mut() {
            m.close();
        }
    }
}

/// The window's light switch: whether the person prefers the scene's light, and whether this scene's shows it
/// (`preview_gpu::scene_light_shows`; else the studio's is drawn and the Scene light half is not offered).
#[derive(Clone, Copy)]
struct LightSwitch {
    prefer_scene: bool,
    scene_shows: bool,
}

enum WindowAct {
    /// Show the scene (its GUID).
    Scene(String),
    /// The scene's light (true) or the studio's.
    SceneLight(bool),
    Reset,
    Open,
}

/// The window's title (the scene, its project, when it was saved), what it holds, its scenes and its actions.
#[allow(clippy::too_many_arguments)]
fn chrome(
    ui: &mut Ui,
    _cx: &mut Cx,
    project: &Project,
    h: &Header,
    written: Option<SystemTime>,
    source: Option<&Source>,
    scenes: &SceneChips,
    scene_rects: &mut Vec<(String, Rect)>,
    busy: bool,
    rect: Rect,
    light: Option<LightSwitch>,
    light_switch: &mut Option<[Rect; 2]>,
    act: &mut Option<WindowAct>,
) {
    let p = ui.painter().clone();
    // The light it is drawn in, top right beside the close button: the scene's own, or the studio's (also when the scene's
    // light would show nothing: its half then stays dim and says why).
    if let Some(LightSwitch { prefer_scene, scene_shows }) = light {
        let scene_light = prefer_scene && scene_shows;
        let labels = [(true, icons::LIGHTBULB, tr("Scene light")), (false, icons::SUN, tr("Studio light"))];
        let txts = labels.map(|(on, _, l)| Txt::new(l, 12.5).weight(if on == scene_light { weight::SEMIBOLD } else { weight::MEDIUM }));
        let widths = txts.each_ref().map(|t| widgets::text_size(ui, t).x + 50.0);
        let close = viewer::close_rect(rect);
        let row =
            Rect::from_min_size(pos2(close.left() - 10.0 - widths[0] - widths[1] - 8.0, close.center().y - 19.0), vec2(widths[0] + widths[1] + 8.0, 38.0));
        paint::fill_squircle(&p, row, theme::squircle(19.0), theme::black(0.5));
        let mut sx = row.left() + 4.0;
        let mut rects = [Rect::NOTHING; 2];
        for (i, ((on, glyph, label), txt)) in labels.into_iter().zip(txts).enumerate() {
            let r = Rect::from_min_size(pos2(sx, row.top() + 4.0), vec2(widths[i], 30.0));
            rects[i] = r;
            let chosen = on == scene_light;
            let offered = !on || scene_shows;
            let resp = ui.interact(r, Id::unique(("preview-light", i)), if offered { Sense::click() } else { Sense::hover() });
            let k = anim::toggle(ui.ctx(), Id::unique(("preview-light-on", i)), chosen, Spring::SNAPPY);
            let hk = anim::toggle(ui.ctx(), Id::unique(("preview-light-hover", i)), offered && resp.hovered(), Spring::SNAPPY);
            if k > 0.01 || hk > 0.01 {
                paint::fill_nested_squircle(&p, row, theme::squircle(19.0), r, theme::white(0.13 * k + 0.05 * hk * (1.0 - k)));
            }
            let fg = if offered { theme::mix(theme::fg_2(), theme::fg(), k) } else { theme::fg_3() };
            let glyph_tint = if offered { theme::mix(theme::fg_3(), theme::accent(), k) } else { theme::alpha(theme::fg_3(), 0.6) };
            widgets::icon(&p, pos2(r.left() + 19.0, r.center().y), glyph, 13.0, glyph_tint);
            widgets::text(&p, pos2(r.left() + 31.0, r.center().y), Align2::LEFT_CENTER, &txt.color(fg));
            let tip = match (on, offered) {
                (true, true) => tr("The lights the scene has in Unity: its directional lights and ambient light"),
                (true, false) => {
                    tr("This scene has too little light of its own to show it (no sun or a very dim one, and an ambient light that is black or that Unity had not computed yet), so it is shown in the studio light")
                }
                _ => tr("The same studio light for every preview, as bright as a new Unity scene"),
            };
            let resp = if offered { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }.on_hover_text(tip);
            if widgets::activated(&resp) && !chosen {
                *act = Some(WindowAct::SceneLight(on));
            }
            sx = r.right();
        }
        *light_switch = Some(rects);
    }
    // Title, over a dark chip so it reads over any scene.
    let scene_name = if h.scene.name.is_empty() { project.name.clone() } else { h.scene.name.clone() };
    let saved = written
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| tr!("Saved {when}", when = crate::pages::ago(ui.ctx(), d.as_secs(), orbiters_projects::discovery::now())))
        .unwrap_or_default();
    let sub = [project.name.as_str(), saved.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("  ·  ");
    let title = Txt::new(&scene_name, 16.0).weight(weight::SEMIBOLD).tracking(-0.01);
    let subt = Txt::new(&sub, 12.0).color(theme::fg_3());
    let max_w = (rect.width() * 0.5).max(200.0);
    let w = (widgets::text_size(ui, &title).x.max(widgets::text_size(ui, &subt).x) + 72.0).min(max_w);
    let chip = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(w, 54.0));
    paint::fill_squircle(&p, chip, theme::squircle(16.0), theme::black(0.5));
    let (tint, glyph) = match h.kind {
        Kind::Avatars => (theme::BLUE, icons::PERSON_ARMS_SPREAD),
        Kind::World => (theme::VIOLET, icons::GLOBE),
        Kind::Scene => (theme::CYAN, icons::CUBE),
    };
    let tile = Rect::from_min_size(chip.min + vec2(10.0, 10.0), vec2(34.0, 34.0));
    let from = match source {
        Some(Source::Toolkit(_)) => tr("Drawn by Orbiters Toolkit when the scene was saved in Unity: exactly as Unity shows it."),
        Some(Source::Stale(_)) => tr("Built from the project's files; something changed since, so it is built again and updates here."),
        Some(_) => {
            tr("Built by Orbiters - N4N0 from the project's files (scene, prefabs, models, materials, textures), without Unity. Orbiters Toolkit draws it exactly each time the scene is saved in Unity.")
        }
        None => "",
    };
    if !from.is_empty() {
        ui.interact(chip, Id::unique("project-preview-title"), Sense::hover()).on_hover_text(from);
    }
    p.rect_filled(tile, theme::radius(10.0), theme::alpha(tint, 0.16));
    widgets::icon(&p, tile.center(), glyph, 16.0, tint);
    widgets::text_truncated(&p, pos2(tile.right() + 12.0, chip.top() + 19.0), Align2::LEFT_CENTER, &title, chip.right() - tile.right() - 24.0);
    widgets::text_truncated(&p, pos2(tile.right() + 12.0, chip.top() + 37.0), Align2::LEFT_CENTER, &subt, chip.right() - tile.right() - 24.0);

    // What it holds.
    let s = &h.stats;
    let mut facts = vec![];
    if !h.avatars.is_empty() {
        facts.push(tr_n("{count} avatar", "{count} avatars", h.avatars.len() as u64));
    }
    facts.push(tr!("{count} triangles", count = compact(s.triangles)));
    facts.push(tr_n("{count} material", "{count} materials", h.materials.len() as u64));
    // Left out for the budget or unreadable (utility shaders and empty meshes are not worth a word).
    let skipped: u32 = s.skipped.iter().filter(|(k, _)| matches!(k.as_str(), "budget" | "missing")).map(|(_, n)| n).sum();
    let mut x = rect.left() + 14.0;
    let y = rect.bottom() - 14.0 - 15.0;
    for fact in &facts {
        let txt = Txt::new(fact, 12.0).color(theme::fg_2());
        let fw = widgets::text_size(ui, &txt).x + 24.0;
        let r = Rect::from_min_size(pos2(x, y - 15.0), vec2(fw, 30.0));
        p.rect_filled(r, theme::radius(15.0), theme::black(0.5));
        widgets::text(&p, r.center(), Align2::CENTER_CENTER, &txt);
        x = r.right() + 6.0;
    }
    // Where the "Built from files" note goes once the scenes' chips took their room (drawn last, below).
    let note_x = x + if skipped > 0 { 36.0 } else { 0.0 };
    if skipped > 0 {
        let r = Rect::from_min_size(pos2(x, y - 15.0), vec2(30.0, 30.0));
        p.circle_filled(r.center(), 15.0, theme::black(0.5));
        widgets::icon(&p, r.center(), icons::INFO, 14.0, theme::fg_3());
        let n = |k: &str| s.skipped.get(k).copied().unwrap_or(0);
        let mut reasons = Vec::new();
        if n("budget") > 0 {
            reasons.push(tr!("{count} beyond the preview's 2 million triangles", count = n("budget")));
        }
        if n("missing") > 0 {
            reasons.push(tr!("{count} could not be read", count = n("missing")));
        }
        let why = reasons.join(", ");
        let text = orbiters_i18n::fill(&tr_n("{count} mesh is not in this preview: {why}.", "{count} meshes are not in this preview: {why}.", skipped as u64), &[("why", &why)]);
        ui.interact(r, Id::unique("project-preview-skipped"), Sense::hover()).on_hover_text(text);
    }

    // Open in Unity and framing again, bottom right.
    let b = Button::new(tr("Open in Unity"))
        .icon(icons::PLAY)
        .variant(Variant::Primary)
        .size(Size::Sm)
        .enabled(project.registered && !project.missing && !busy)
        .id_salt("preview-open");
    let bw = b.desired_width(ui);
    let br = Rect::from_min_size(pos2(rect.right() - 14.0 - bw, rect.bottom() - 14.0 - 32.0), vec2(bw, 32.0));
    if project.registered && widgets::activated(&b.show_at(ui, br)) {
        *act = Some(WindowAct::Open);
    }
    let rr = Rect::from_min_size(pos2(br.left() - 8.0 - 32.0, br.top()), vec2(32.0, 32.0));
    // The right end of the room the note may take: the chips' row when there is one, else the buttons.
    let mut note_end = rr.left() - 8.0;
    if widgets::activated(
        &IconButton::new(icons::ARROW_COUNTER_CLOCKWISE, mark("Frame it again"))
            .size(32.0, 15.0)
            .backdrop(theme::black(0.5))
            .id_salt("preview-reset")
            .show_at(ui, rr),
    ) {
        *act = Some(WindowAct::Reset);
    }

    // The project's scenes, to switch between: as many as fit between the facts and the buttons (the one shown always
    // among them), the others in a list from the last chip.
    let n = scenes.chips.len();
    if n > 1 {
        let selected = scenes.selected.as_deref();
        let txt_of = |ch: &SceneChip| {
            let on = Some(ch.guid.as_str()) == selected;
            Txt::new(&ch.name, 12.5).weight(if on { weight::SEMIBOLD } else { weight::MEDIUM }).color(if on { theme::fg() } else { theme::fg_2() })
        };
        let width_of = |ch: &SceneChip| widgets::text_size(ui, &txt_of(ch)).x + 28.0 + if ch.state.marked() { 18.0 } else { 0.0 };
        let more_label = tr!("All scenes · {n}", n = n);
        let more_txt = Txt::new(&more_label, 12.5).weight(weight::MEDIUM).color(theme::fg_2());
        let more_w = widgets::text_size(ui, &more_txt).x + 44.0;
        let side = (bw + 60.0).max(x - rect.left() + 12.0);
        let room = rect.width() - 2.0 * side - 8.0;
        let widths: Vec<f32> = scenes.chips.iter().map(&width_of).collect();
        let mut picked: Vec<usize> = Vec::new();
        let mut used = 0.0;
        for (i, w) in widths.iter().enumerate() {
            let reserve = if i + 1 < n { more_w } else { 0.0 };
            if used + w + reserve > room {
                break;
            }
            picked.push(i);
            used += w;
        }
        if let Some(si) = scenes.chips.iter().position(|ch| Some(ch.guid.as_str()) == selected) {
            if !picked.contains(&si) {
                while !picked.is_empty() && used + widths[si] + more_w > room {
                    used -= widths[picked.pop().unwrap()];
                }
                picked.push(si);
                used += widths[si];
            }
        }
        let overflow = picked.len() < n;
        let total = used + if overflow { more_w } else { 0.0 } + 8.0;
        if total <= room + 8.0 && !picked.is_empty() || overflow && more_w + 8.0 <= room {
            let row = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 30.0), vec2(total, 38.0));
            note_end = row.left() - 8.0;
            paint::fill_squircle(&p, row, theme::squircle(19.0), theme::black(0.5));
            let mut sx = row.left() + 4.0;
            for &i in &picked {
                let ch = &scenes.chips[i];
                let r = Rect::from_min_size(pos2(sx, row.top() + 4.0), vec2(widths[i], 30.0));
                scene_rects.push((ch.guid.clone(), r));
                let on = Some(ch.guid.as_str()) == selected;
                let resp = ui.interact(r, Id::unique(("preview-scene", &ch.guid)), Sense::click());
                let k = anim::toggle(ui.ctx(), Id::unique(("preview-scene-on", &ch.guid)), on, Spring::SNAPPY);
                let hk = anim::toggle(ui.ctx(), Id::unique(("preview-scene-hover", &ch.guid)), resp.hovered(), Spring::SNAPPY);
                if k > 0.01 || hk > 0.01 {
                    paint::fill_nested_squircle(&p, row, theme::squircle(19.0), r, theme::white(0.13 * k + 0.05 * hk * (1.0 - k)));
                }
                let mut tx = r.left() + 14.0;
                match &ch.state {
                    ChipState::Building => {
                        feedback::spinner(&p, pos2(tx + 5.0, r.center().y), 5.0, 1.6, theme::fg_3());
                        ui.ctx().request_repaint();
                        tx += 18.0;
                    }
                    ChipState::Failed(_) => {
                        widgets::icon(&p, pos2(tx + 5.0, r.center().y), icons::WARNING, 12.0, theme::WARN);
                        tx += 18.0;
                    }
                    _ => {}
                }
                widgets::text(&p, pos2(tx, r.center().y), Align2::LEFT_CENTER, &txt_of(ch));
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(ch.tip());
                if widgets::activated(&resp) && !on {
                    *act = Some(WindowAct::Scene(ch.guid.clone()));
                }
                sx = r.right();
            }
            if overflow {
                let r = Rect::from_min_size(pos2(sx, row.top() + 4.0), vec2(more_w, 30.0));
                scene_rects.push(("all".into(), r));
                let mid = Id::unique("preview-scenes-all");
                let resp = ui.interact(r, mid, Sense::click());
                let open = orbiters_ui::popup::is_open(ui.ctx(), mid.with("list"));
                let hk = anim::toggle(ui.ctx(), mid.with("hover"), resp.hovered() || open, Spring::SNAPPY);
                if hk > 0.01 {
                    p.rect_filled(r, theme::radius(15.0), theme::white(0.07 * hk));
                }
                widgets::text(&p, pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, &more_txt);
                widgets::icon(&p, pos2(r.right() - 16.0, r.center().y), super::art::glyph::CARET_UP, 11.0, theme::fg_3());
                let pressed = widgets::activated(&resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr("Every scene of the project")));
                let details: Vec<String> = scenes.chips.iter().map(|ch| ch.state.detail().to_string()).collect();
                let items: Vec<orbiters_ui::popup::Item> = scenes
                    .chips
                    .iter()
                    .zip(&details)
                    .map(|(ch, d)| {
                        let item = orbiters_ui::popup::Item::new(&ch.name);
                        if d.is_empty() {
                            item
                        } else {
                            item.detail(d)
                        }
                    })
                    .collect();
                let current = scenes.chips.iter().position(|ch| Some(ch.guid.as_str()) == selected);
                // As wide as the longest name needs (within reason), the list's own padding and detail included.
                let list_w = scenes
                    .chips
                    .iter()
                    .zip(&details)
                    .map(|(ch, d)| widgets::text_size(ui, &Txt::new(&ch.name, 13.0)).x + widgets::text_size(ui, &Txt::new(d, 11.5)).x + 72.0)
                    .fold(260.0, f32::max)
                    .min(440.0);
                if let Some(k) = orbiters_ui::popup::list_from(ui.ctx(), mid.with("list"), r, pressed, &items, current, list_w) {
                    if Some(k) != current {
                        *act = Some(WindowAct::Scene(scenes.chips[k].guid.clone()));
                    }
                }
            }
        }
    }

    // Where it comes from, when the desktop built it (Toolkit's is the exact one, and needs no word): quieter than the
    // facts, after them, only in the room the scenes' chips leave (it never takes one of their places), the title's
    // tooltip saying the same when it does not fit.
    if matches!(source, Some(Source::Built(_) | Source::Stale(_))) {
        let txt = Txt::new(tr("Built from files"), 12.0).color(theme::fg_3());
        let fw = widgets::text_size(ui, &txt).x + 24.0 + 18.0;
        if note_x + fw <= note_end {
            let r = Rect::from_min_size(pos2(note_x, y - 15.0), vec2(fw, 30.0));
            p.rect_filled(r, theme::radius(15.0), theme::black(0.35));
            widgets::icon(&p, pos2(r.left() + 19.0, r.center().y), icons::CUBE, 12.0, theme::fg_3());
            widgets::text(&p, pos2(r.left() + 31.0, r.center().y), Align2::LEFT_CENTER, &txt);
            ui.interact(r, Id::unique("project-preview-built"), Sense::hover()).on_hover_text(
                tr("Built by Orbiters - N4N0 from the project's files, without Unity: close to what Unity shows, not exact (no normal maps, animators or what tools do at upload). Orbiters Toolkit draws the exact one each time the scene is saved in Unity."),
            );
        }
    }
}

/// The project's scenes as the window's chips show them, and the one picked.
#[derive(Default)]
struct SceneChips {
    chips: Vec<SceneChip>,
    selected: Option<String>,
}

struct SceneChip {
    guid: String,
    name: String,
    rel: String,
    state: ChipState,
}

enum ChipState {
    /// Its preview is there: from Toolkit (true) or built here.
    Ready(bool),
    /// Built here, outdated: built again when shown.
    Outdated,
    /// No preview yet: built when picked.
    Unbuilt,
    Building,
    Failed(String),
}

impl ChipState {
    /// Marked on its chip (a spinner, a warning).
    fn marked(&self) -> bool {
        matches!(self, ChipState::Building | ChipState::Failed(_))
    }

    /// Its line in the list of every scene.
    fn detail(&self) -> &str {
        match self {
            ChipState::Ready(true) => tr("Toolkit"),
            ChipState::Ready(false) | ChipState::Outdated => "",
            ChipState::Unbuilt => tr("Not built yet"),
            ChipState::Building => tr("Building…"),
            ChipState::Failed(_) => tr("No preview"),
        }
    }
}

impl SceneChip {
    fn tip(&self) -> String {
        let what = match &self.state {
            ChipState::Ready(true) => tr("Drawn by Orbiters Toolkit when it was saved in Unity").to_string(),
            ChipState::Ready(false) => tr("Built from the project's files").to_string(),
            ChipState::Outdated => tr("Built from the project's files; changed since, built again when shown").to_string(),
            ChipState::Unbuilt => tr("Press to build its preview from the project's files").to_string(),
            ChipState::Building => tr("Building its preview from the project's files…").to_string(),
            ChipState::Failed(why) => tr!("No preview: {why}", why = why),
        };
        format!("{}\n{what}", self.rel)
    }
}

/// `c`'s scenes and where each stands. Scenes of the same name (`Main` in two folders) carry their folder's name too,
/// so that their chips and lines in the list tell them apart.
fn scene_chips(c: &Current, builds: &Builder) -> SceneChips {
    let named = |n: &str| c.slots.iter().filter(|s| s.scene.name.eq_ignore_ascii_case(n)).count();
    let chips = c
        .slots
        .iter()
        .map(|s| {
            let building = builds.building(&c.project, &s.scene.guid);
            let state = match &s.source {
                Source::Toolkit(_) => ChipState::Ready(true),
                Source::Built(_) => ChipState::Ready(false),
                Source::Stale(_) if building => ChipState::Building,
                Source::Stale(_) => ChipState::Outdated,
                Source::None if building => ChipState::Building,
                Source::None => ChipState::Unbuilt,
                Source::Failed(why) => ChipState::Failed(why.to_string()),
            };
            let name = match s.scene.rel.rsplit('/').nth(1) {
                Some(folder) if named(&s.scene.name) > 1 => format!("{} · {folder}", s.scene.name),
                _ => s.scene.name.clone(),
            };
            SceneChip { guid: s.scene.guid.clone(), name, rel: s.scene.rel.clone(), state }
        })
        .collect();
    SceneChips { chips, selected: c.slot().map(|s| s.scene.guid.clone()) }
}

/// 312400 → "312k", 1840000 → "1.8M".
/// A version's numbers for comparing ("0.4.10" after "0.4.9"); a pre-release's suffix is left out.
pub(super) fn version(v: &str) -> [u32; 3] {
    let mut out = [0; 3];
    for (o, part) in out.iter_mut().zip(v.split(['-', '+']).next().unwrap_or_default().split('.')) {
        *o = part.parse().unwrap_or(0);
    }
    out
}

fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1e3),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1e3).round()),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// Screenshots: a stand-in preview (or the file `ORBITERS_PREVIEW_FILE` names, a real one from Toolkit) shown for the
/// selected project, and the presses that open its window. `projects-preview-native*` show a real project
/// (`ORBITERS_PREVIEW_PROJECT`), its scenes listed and built from its files on the frame (read only; previews go to
/// the cache, `ORBITERS_PREVIEW_CACHE` when set); `projects-stills` shows the gallery with real projects
/// (`ORBITERS_PREVIEW_PROJECTS`, separated by `|`) without avatar pictures, their stills drawn on the frame.
impl State {
    pub fn stage_preview(&mut self, name: &str) {
        let dir = std::env::temp_dir().join("orbiters-preview-stand-in");
        let folder = dir.join(file::FOLDER);
        let _ = std::fs::remove_dir_all(dir.join("Assets"));
        for old in ["stand-in.preview", "second.preview"] {
            let _ = std::fs::remove_file(folder.join(old));
        }
        let _ = std::fs::remove_dir_all(native::cache_dir(&dir));
        // Screenshots start from the default (the scene's light), whatever this machine remembers.
        self.preview.scene_light = Some(true);
        if name.starts_with("projects-preview-native") {
            let real = PathBuf::from(std::env::var_os("ORBITERS_PREVIEW_PROJECT").expect("ORBITERS_PREVIEW_PROJECT: a Unity project's folder"));
            if let Some(p) = self.scan.projects.first_mut() {
                p.name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                p.path = real.clone();
            }
            self.preview.fixture = Some(real);
            self.preview.hold_builds = name == "projects-preview-native-building";
            return;
        }
        if name == "projects-stills" {
            let reals: Vec<PathBuf> =
                std::env::var("ORBITERS_PREVIEW_PROJECTS").unwrap_or_default().split('|').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
            for (p, real) in self.scan.projects.iter_mut().zip(&reals) {
                p.name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                p.path = real.clone();
                p.avatars.clear();
                p.worlds.clear();
            }
            self.preview.fixture_stills = true;
            return;
        }
        let scene = matches!(name, "projects-preview-none" | "projects-preview-no-toolkit" | "projects-preview-old-toolkit" | "projects-preview-building");
        if scene {
            // A scene that has nothing a preview draws (its build fails with the reason), or that is being built.
            let assets = dir.join("Assets");
            std::fs::create_dir_all(&assets).expect("stand-in folder");
            std::fs::write(assets.join("Avatar.unity"), "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n").expect("stand-in scene");
            std::fs::write(assets.join("Avatar.unity.meta"), format!("fileFormatVersion: 2\nguid: {}\n", "5".repeat(32))).expect("stand-in scene"); // i18n: not shown (a Unity meta file written for a screenshot scene)
            self.preview.hold_builds = name == "projects-preview-building";
        } else if name != "projects-preview-no-scene" {
            std::fs::create_dir_all(&folder).expect("stand-in folder");
            let bytes = match std::env::var_os("ORBITERS_PREVIEW_FILE") {
                Some(p) => std::fs::read(p).expect("ORBITERS_PREVIEW_FILE"),
                None => stand_in(),
            };
            std::fs::write(folder.join("stand-in.preview"), bytes).expect("stand-in preview");
            if name == "projects-preview-scenes" {
                std::fs::write(folder.join("second.preview"), stand_in_scene("Club stage")).expect("stand-in preview"); // i18n: not shown (sample data of a screenshot scene)
            }
        }
        self.preview.fixture = Some(dir);
        if name == "projects-preview-old-toolkit" {
            // A world project's Toolkit before previews (Enzo's had 0.4.8), the repositories offering a newer one.
            for p in self.view.packages.iter_mut().filter(|p| p.name == "orbiters.toolkit") {
                p.installed = Some("0.4.8".into());
                p.versions = vec!["0.4.10".into(), "0.4.9".into(), "0.4.8".into()];
            }
        }
        if name == "projects-preview-no-toolkit" {
            for p in self.view.packages.iter_mut().filter(|p| p.name == "orbiters.toolkit") {
                p.installed = None;
            }
        }
    }

    pub fn stage_preview_frame(&mut self, name: &str, frame: u32) -> Option<super::state::Poke> {
        use super::state::Poke;
        let at = |key: &str| self.anchors.get(key).map(|r| r.center());
        // The second scene's chip, and the list of every scene.
        let chip = |s: &Self, k: usize| s.preview.scene_rects.iter().filter(|(g, _)| g != "all").nth(k).map(|(_, r)| r.center());
        let all = |s: &Self| s.preview.scene_rects.iter().find(|(g, _)| g == "all").map(|(_, r)| r.center());
        match (name, frame) {
            (
                "projects-preview-open"
                | "projects-preview-open-mid"
                | "projects-preview-scenes"
                | "projects-preview-close"
                | "projects-preview-back"
                | "projects-preview-scene-light"
                | "projects-preview-studio-light"
                | "projects-preview-native-open"
                | "projects-preview-native-switch"
                | "projects-preview-native-switched"
                | "projects-preview-native-all",
                90,
            ) => at("preview-cover").map(Poke::Press),
            ("projects-preview-studio-light", 150) => self.preview.light_switch.map(|r| Poke::Press(r[1].center())),
            ("projects-preview-native-switch", 149) => {
                // The scene picked is built while the picture is taken.
                self.preview.hold_builds = true;
                None
            }
            ("projects-preview-native-switch" | "projects-preview-native-switched", 150) => chip(self, 1).map(Poke::Press),
            ("projects-preview-native-all", 150) => all(self).map(Poke::Press),
            ("projects-preview-close" | "projects-preview-back", 140) => {
                self.preview.frame_for_screenshot(name == "projects-preview-close");
                None
            }
            ("projects-preview-turned", 100) => at("preview-cover").map(Poke::Hold),
            ("projects-preview-turned", 101..=112) => at("preview-cover").map(|p| Poke::Hover(p + vec2(9.0 * (frame - 100) as f32, 0.0))),
            ("projects-preview-sheet", 30) => at("card-0").map(Poke::Press),
            _ => None,
        }
    }
}

impl Previews {
    /// Screenshots: the shown preview's camera on the upper half of the subject from close by (`close`), else on its
    /// back, at once.
    pub(super) fn frame_for_screenshot(&mut self, close: bool) {
        let Some(c) = self.current.as_mut() else { return };
        let Some(cam) = c.camera.as_mut() else { return };
        let mut o = cam.home;
        if close {
            o.distance *= 0.45;
            o.pivot[1] += cam.radius * 0.28;
            o.pitch = -0.08;
        } else {
            o.yaw += std::f32::consts::PI;
        }
        cam.goal = o;
        cam.now = o;
        cam.spin = [0.0; 2];
    }
}

/// A stand-in scene for screenshots: N4N0's black sphere with white strokes floating over a plinth, a ring round it and
/// two cubes, as an avatar scene.
fn stand_in() -> Vec<u8> {
    stand_in_scene("Avatar") // i18n: not shown (sample data of a screenshot scene)
}

fn stand_in_scene(scene: &str) -> Vec<u8> {
    use file::{Alpha, Bounds, Cull, Draw, ImageFormat, IndexFormat, Instances, Material, Mesh, Submesh, Texture, Writer};
    let mut w = Writer::default();
    let mut meshes = vec![];
    let mut add_mesh = |w: &mut Writer, (verts, idx): (Vec<([f32; 3], [f32; 3], [f32; 2])>, Vec<u32>)| {
        let mut data = Vec::with_capacity(verts.len() * 20);
        let mut b = Bounds { min: [f32::MAX; 3], max: [f32::MIN; 3] };
        for (p, n, uv) in &verts {
            data.extend_from_slice(&file::vertex(*p, *n, *uv));
            for k in 0..3 {
                b.min[k] = b.min[k].min(p[k]);
                b.max[k] = b.max[k].max(p[k]);
            }
        }
        let data = w.push(&data);
        let indices = w.push(&idx.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<_>>());
        meshes.push(Mesh {
            vertices: verts.len() as u32,
            data,
            index_format: IndexFormat::U32,
            indices,
            submeshes: vec![Submesh { start: 0, count: idx.len() as u32 }],
            bounds: b,
            lightmap_uv: None,
        });
    };
    add_mesh(&mut w, sphere(0.34, 48, 32));
    add_mesh(&mut w, cylinder(0.5, 0.42, 0.12, 64));
    add_mesh(&mut w, torus(0.52, 0.018, 96, 12));
    add_mesh(&mut w, cuboid(0.16));
    // The sphere's strokes: white curves on black.
    let (tw, th) = (512u32, 256u32);
    let mut rgb = vec![0u8; (tw * th * 3) as usize];
    for y in 0..th {
        for x in 0..tw {
            let (u, v) = (x as f32 / tw as f32, y as f32 / th as f32);
            let wave = |phase: f32, amp: f32, at: f32| ((v - at - amp * (u * std::f32::consts::TAU * 2.0 + phase).sin()).abs() < 0.012) as u8 as f32;
            let s = wave(0.0, 0.08, 0.42).max(wave(1.7, 0.05, 0.58)).max(wave(3.1, 0.11, 0.3));
            let c = 18.0 + 225.0 * s;
            let i = ((y * tw + x) * 3) as usize;
            rgb[i..i + 3].copy_from_slice(&[c as u8, c as u8, (c + 3.0).min(255.0) as u8]);
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png_bytes, tw, th);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgb).unwrap();
    }
    let texture = Texture { width: tw, height: th, format: ImageFormat::Png, linear: false, data: w.push(&png_bytes) };
    // Each look once: a glossy sphere, a matte plinth, a glowing ring, a toon cube and a metal one.
    let mat = |name: &str, color: [f32; 4], texture: i32| Material {
        name: name.into(),
        color,
        texture,
        cull: Cull::Back,
        alpha: Alpha::Opaque,
        ..Default::default()
    };
    let materials = vec![
        Material { smoothness: 0.75, ..mat("Sphere", [1.0, 1.0, 1.0, 1.0], 0) }, // i18n: not shown (sample data of a screenshot scene)
        Material { smoothness: 0.1, ..mat("Plinth", [0.36, 0.36, 0.40, 1.0], -1) }, // i18n: not shown (sample data of a screenshot scene)
        Material { shading: file::Shading::Unlit, emission: [0.0, 0.35, 0.08], ..mat("Ring", [0.0, 0.85, 0.43, 1.0], -1) }, // i18n: not shown (sample data of a screenshot scene)
        Material { shading: file::Shading::Toon, shade: [0.62, 0.55, 0.72], ..mat("Cube", [0.86, 0.55, 0.33, 1.0], -1) }, // i18n: not shown (sample data of a screenshot scene)
        Material { metallic: 1.0, smoothness: 0.7, ..mat("Cube 2", [0.42, 0.52, 0.92, 1.0], -1) }, // i18n: not shown (sample data of a screenshot scene)
    ];
    let trs = |t: [f32; 3], yaw: f32, tilt: f32| -> [f32; 16] {
        let (sy, cy) = yaw.sin_cos();
        let (st, ct) = tilt.sin_cos();
        // Yaw about Y, then tilt about X.
        [cy, 0.0, -sy, 0.0, sy * st, ct, cy * st, 0.0, sy * ct, -st, cy * ct, 0.0, t[0], t[1], t[2], 1.0]
    };
    let instances = [
        trs([0.0, 0.6, 0.0], 0.6, 0.0),
        trs([0.0, 0.0, 0.0], 0.0, 0.0),
        trs([0.0, 0.6, 0.0], 0.3, 0.42),
        trs([0.66, 0.0, 0.3], 0.5, 0.0),
        trs([-0.6, 0.0, -0.38], -0.3, 0.0),
    ];
    let matrices = w.push(&instances.iter().flatten().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    let draws = [(0, 0, 0), (1, 1, 1), (2, 2, 2), (3, 3, 3), (4, 3, 4)]
        .map(|(i, m, mat)| Draw { instance: i, mesh: m, materials: vec![mat], shadows: true, avatar: Some(0), lightmap: None })
        .to_vec();
    let bounds = Bounds { min: [-0.72, 0.0, -0.55], max: [0.78, 0.96, 0.55] };
    let header = Header {
        written_utc: "2026-10-29T21:40:00Z".into(),
        generator: "Orbiters - N4N0 (stand-in)".into(), // i18n: not shown (sample data of a screenshot scene)
        unity: "2022.3.22f1".into(),
        scene: file::Scene { name: scene.into(), path: format!("Assets/{scene}.unity"), guid: format!("{:032x}", scene.len()) },
        kind: Kind::Avatars,
        bounds,
        focus: bounds,
        ground: Some(0.0),
        camera: None,
        avatars: vec![file::AvatarPlace { name: "N4N0".into(), id: None, position: [0.0; 3], forward: [0.0, 0.0, 1.0] }],
        textures: vec![texture],
        materials,
        meshes,
        instances: Instances { count: instances.len() as u32, matrices },
        draws,
        // Its scene's light: a low warm sun from the right casting shadows, a cool light from behind, a dusky sky.
        lighting: Some(file::Lighting {
            lights: vec![
                file::SceneLight { direction: [-0.8, -0.45, 0.4], color: [1.25, 0.82, 0.52], intensity: 1.25, shadows: true, shadow_strength: 0.9, baked: false },
                file::SceneLight { direction: [0.2, -0.3, -0.93], color: [0.18, 0.3, 0.6], intensity: 0.6, shadows: false, shadow_strength: 1.0, baked: false },
            ],
            ambient: Some(file::Ambient {
                mode: file::AmbientMode::Trilight,
                sky: [0.16, 0.13, 0.24],
                equator: [0.12, 0.09, 0.13],
                ground: [0.05, 0.04, 0.04],
            }),
            reflection_intensity: 1.0,
            fog: None,
        }),
        lightmaps: Vec::new(),
        sky: None,
        stats: file::Stats { renderers: 5, triangles: 24_576, vertices: 14_000, skipped: Default::default() },
        cost: None,
    };
    w.finish(&header)
}

type Geometry = (Vec<([f32; 3], [f32; 3], [f32; 2])>, Vec<u32>);

/// Triangles wound as Unity's front faces (clockwise seen from the side the normal points to).
fn wind(verts: &[([f32; 3], [f32; 3], [f32; 2])], idx: &mut [u32]) {
    for t in idx.chunks_mut(3) {
        let (a, b, c) = (verts[t[0] as usize].0, verts[t[1] as usize].0, verts[t[2] as usize].0);
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let vn = verts[t[0] as usize].1;
        if n[0] * vn[0] + n[1] * vn[1] + n[2] * vn[2] < 0.0 {
            t.swap(1, 2);
        }
    }
}

fn grid(cols: u32, rows: u32, f: impl Fn(f32, f32) -> ([f32; 3], [f32; 3])) -> Geometry {
    let mut verts = vec![];
    for j in 0..=rows {
        for i in 0..=cols {
            let (u, v) = (i as f32 / cols as f32, j as f32 / rows as f32);
            let (p, n) = f(u, v);
            verts.push((p, n, [u, v]));
        }
    }
    let mut idx = vec![];
    for j in 0..rows {
        for i in 0..cols {
            let a = j * (cols + 1) + i;
            let (b, c, d) = (a + 1, a + cols + 1, a + cols + 2);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    wind(&verts, &mut idx);
    (verts, idx)
}

fn sphere(r: f32, cols: u32, rows: u32) -> Geometry {
    use std::f32::consts::{PI, TAU};
    grid(cols, rows, |u, v| {
        let (t, p) = (u * TAU, (v - 0.5) * PI);
        let n = [p.cos() * t.cos(), p.sin(), p.cos() * t.sin()];
        (n.map(|c| c * r), n)
    })
}

fn torus(r: f32, tube: f32, cols: u32, rows: u32) -> Geometry {
    use std::f32::consts::TAU;
    grid(cols, rows, |u, v| {
        let (a, b) = (u * TAU, v * TAU);
        let n = [b.cos() * a.cos(), b.sin(), b.cos() * a.sin()];
        ([(r + tube * b.cos()) * a.cos(), tube * b.sin(), (r + tube * b.cos()) * a.sin()], n)
    })
}

/// A plinth: a slightly tapered cylinder with a top.
fn cylinder(bottom: f32, top: f32, h: f32, cols: u32) -> Geometry {
    use std::f32::consts::TAU;
    let (mut verts, mut idx) = grid(cols, 1, |u, v| {
        let a = u * TAU;
        let r = bottom + (top - bottom) * v;
        let slope = (bottom - top) / h;
        let n = preview_gpu::normalize([a.cos(), slope, a.sin()]);
        ([r * a.cos(), h * v, r * a.sin()], n)
    });
    let base = verts.len() as u32;
    verts.push(([0.0, h, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5]));
    for i in 0..=cols {
        let a = i as f32 / cols as f32 * TAU;
        verts.push(([top * a.cos(), h, top * a.sin()], [0.0, 1.0, 0.0], [0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()]));
    }
    let mut cap = vec![];
    for i in 0..cols {
        cap.extend_from_slice(&[base, base + 1 + i, base + 2 + i]);
    }
    wind(&verts, &mut cap);
    idx.extend(cap);
    (verts, idx)
}

fn cuboid(s: f32) -> Geometry {
    let mut verts = vec![];
    let mut idx = vec![];
    for (n, u, v) in [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ] {
        let base = verts.len() as u32;
        for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            // Standing on its origin.
            let p: [f32; 3] = std::array::from_fn(|k| (n[k] + u[k] * a + v[k] * b) * s * 0.5 + if k == 1 { s * 0.5 } else { 0.0 });
            verts.push((p, n, [(a + 1.0) * 0.5, (b + 1.0) * 0.5]));
        }
        idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    }
    wind(&verts, &mut idx);
    (verts, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_chosen_is_remembered_and_the_scenes_is_the_default() {
        let dir = std::env::temp_dir().join(format!("orbiters-preview-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PREFS_FILE);
        assert!(Prefs::read(&path).scene_light, "nothing remembered yet: the scene's light");
        Prefs { scene_light: false }.write(&path).unwrap();
        assert!(!Prefs::read(&path).scene_light);
        std::fs::write(&path, b"{}").unwrap();
        assert!(Prefs::read(&path).scene_light, "a file without the choice");
        std::fs::write(&path, b"not json").unwrap();
        assert!(Prefs::read(&path).scene_light, "an unreadable file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stand_in_reads_back() {
        let p = file::parse(stand_in()).unwrap();
        assert_eq!(p.header.draws.len(), 5);
        assert_eq!(p.header.textures.len(), 1);
    }

    #[test]
    fn an_avatar_opens_facing_the_camera() {
        let p = file::parse(stand_in()).unwrap();
        let o = home(&p.header);
        let eye = o.eye();
        // In front of an avatar facing +Z, a little to one side, above its middle.
        assert!(eye[2] > 1.0 && eye[0].abs() < eye[2] && eye[1] > o.pivot[1], "{eye:?}");
    }

    #[test]
    fn the_camera_catches_its_goal_and_rests() {
        let p = file::parse(stand_in()).unwrap();
        let mut c = Camera::new(&p.header, true);
        let mut frames = 0;
        while c.step(1.0 / 60.0, false) {
            frames += 1;
            assert!(frames < 600, "never rests");
        }
        assert_eq!(c.now, c.home);
    }

    fn slot(name: &str, source: Source) -> Slot {
        Slot {
            scene: native::SceneInfo {
                path: PathBuf::from(format!("P/Assets/{name}.unity")),
                rel: format!("Assets/{name}.unity"),
                guid: name.repeat(32).chars().take(32).collect(),
                name: name.into(),
                modified: std::time::UNIX_EPOCH,
                last_open: false,
                in_build: false,
            },
            source,
        }
    }

    #[test]
    fn a_finished_build_lands_in_its_scene_and_the_states_follow() {
        let pv = Previews::default();
        let mut c = Current::new(Path::new("P"));
        assert_eq!(pv.status(&c), Status::Listing);
        c.listed_at = 1.0;
        assert_eq!(pv.status(&c), Status::NoScene);
        c.slots = vec![slot("a", Source::None), slot("b", Source::None)];
        assert_eq!(pv.status(&c), Status::Building, "the first scene, not built yet: it is built");
        let guid_a = c.slots[0].scene.guid.clone();
        apply_build(&mut c, &guid_a, Err("the scene has nothing to draw".into()));
        assert_eq!(pv.status(&c), Status::Failed("the scene has nothing to draw".into()));
        let entry = file::Entry { path: PathBuf::from("b.preview"), modified: std::time::UNIX_EPOCH, size: 1, header: Default::default() };
        let guid_b = c.slots[1].scene.guid.clone();
        apply_build(&mut c, &guid_b, Ok(entry));
        assert_eq!(pv.status(&c), Status::Ready, "the first cannot be built: the project opens on the one that has a preview");
        assert_eq!(c.slot().unwrap().scene.name, "b");
        c.chosen = Some(guid_a);
        assert!(matches!(pv.status(&c), Status::Failed(_)), "picked in the window: its reason");
        let chips = scene_chips(&c, &pv.builds);
        assert_eq!(chips.chips.len(), 2);
        assert!(matches!(chips.chips[0].state, ChipState::Failed(_)) && matches!(chips.chips[1].state, ChipState::Ready(false)));
        assert!(chips.chips[0].tip().contains("No preview: the scene has nothing to draw"));
    }

    #[test]
    fn compact_counts() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(4_210), "4.2k");
        assert_eq!(compact(312_400), "312k");
        assert_eq!(compact(1_840_000), "1.8M");
    }
}
