//! The 3D preview of a Unity project (contracts' Docs/project-preview.md, docs/project-preview.md): the scene its
//! owner last saved in Unity, from the small file Orbiters Toolkit keeps in the project. The selected project's hero
//! card shows it in place of its avatar pictures as soon as it loads (well under a second): the pictures give way to
//! a dark studio, a ring runs out over the ground and the scene rises out of it behind a green edge while the camera
//! swings round to the front. Dragging the card turns it (with inertia), a click grows it into a window over the page
//! (the card's shared geometry) where it also zooms and pans; Escape or the × shrinks it back into the card. The
//! project's UserSettings folder is watched, so a save in Unity shows here as soon as Toolkit wrote it. A project
//! without one says how to get it, and offers to add or update Orbiters Toolkit when it lacks one that writes them.

use super::preview_gpu::{self, Pipelines, Scene, View};
use super::state::{Outcome, State};
use crate::gpu::Handles;
use crate::pages::{Cx, Job};
use egui::{pos2, vec2, Align2, Color32, Id, Order, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::discovery::Project;
use orbiters_projects::preview::{self as file, Entry, Header, Kind};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::viewer::{self, Morph};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{feedback, format, icons, paint};
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
    /// Screenshots: the project folder whose previews every project shows; read on the frame (no jobs).
    fixture: Option<PathBuf>,
}

struct Current {
    project: PathBuf,
    entries: Vec<Entry>,
    listing: Job<Vec<Entry>>,
    listed_at: f64,
    /// The scene the person picked in the window (else the newest).
    chosen: Option<PathBuf>,
    /// The file shown or loading, and when it was written.
    file: Option<(PathBuf, SystemTime)>,
    loading: Job<Result<Scene, String>>,
    scene: Option<Scene>,
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
            entries: vec![],
            listing: Job::default(),
            listed_at: f64::NEG_INFINITY,
            chosen: None,
            file: None,
            loading: Job::default(),
            scene: None,
            error: None,
            shown_at: 0.0,
            camera: None,
            touched: false,
            _watch: None,
            changed: Arc::default(),
        }
    }

    fn entry(&self) -> Option<&Entry> {
        let path = &self.file.as_ref()?.0;
        self.entries.iter().find(|e| &e.path == path)
    }
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
    if h.kind == Kind::Avatars {
        h.focus
    } else {
        h.bounds
    }
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
    /// Keeps `project`'s previews listed and the one to show loaded.
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
        let c = self.current.as_mut().unwrap();
        let folder = fixture.clone().unwrap_or_else(|| project.to_path_buf());
        if fixture.is_some() {
            if c.listed_at == f64::NEG_INFINITY {
                c.entries = file::list(&folder);
                c.listed_at = now;
            }
        } else {
            if let Some(entries) = c.listing.take() {
                c.entries = entries;
                c.listed_at = now;
            }
            if c._watch.is_none() {
                let (changed, ctx) = (c.changed.clone(), ctx.clone());
                c._watch = Some(file::watch(project, move || {
                    changed.store(true, std::sync::atomic::Ordering::Relaxed);
                    ctx.request_repaint();
                }));
            }
            let changed = c.changed.swap(false, std::sync::atomic::Ordering::Relaxed);
            if !c.listing.running() && (changed || now - c.listed_at > RELIST_S) {
                c.listed_at = now;
                c.listing = Job::spawn(ctx, move || file::list(&folder));
            }
            if c.listed_at > now - RELIST_S {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(RELIST_S));
            }
        }
        // The scene to show: the one picked, else the newest written.
        let want = c.chosen.as_ref().and_then(|p| c.entries.iter().find(|e| &e.path == p)).or(c.entries.first()).map(|e| (e.path.clone(), e.modified));
        if let Some(want) = want.filter(|w| c.file.as_ref() != Some(w)) {
            if !c.loading.running() {
                c.file = Some(want.clone());
                let (device, queue, pipelines) = (gpu.device.clone(), gpu.queue.clone(), self.pipelines.clone());
                let load = move || preview_gpu::load(&device, &queue, pipelines, &want.0).map_err(|e| format!("{e:#}"));
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
                Ok(scene) => {
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
                    if first || c.scene.as_ref().is_some_and(|s| s.header.scene.guid != scene.header.scene.guid) {
                        c.camera = Some(Camera::new(&scene.header, true));
                    }
                    c.shown_at = now;
                    c.scene = Some(scene);
                    c.error = None;
                }
                Err(e) => {
                    eprintln!("project preview: {e}");
                    c.error = Some(e);
                }
            }
        }
    }
}

/// Moves `camera` with the pointer over `resp`: a drag turns it (a flick keeps it spinning a moment), with `free` the
/// wheel or a pinch zooms, a right or middle drag pans; a double click frames the scene again.
fn steer(ctx: &egui::Context, resp: &egui::Response, camera: &mut Camera, rect: Rect, free: bool) -> bool {
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
                camera.goal.distance = (camera.goal.distance * (-scroll * 0.0018).exp() / pinch).clamp(r * 0.12, r * 40.0);
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
fn frame_view(ctx: &egui::Context, c: &mut Current, dragging: bool, shift: f32) -> (View, bool) {
    let dt = ctx.input(|i| i.stable_dt.clamp(1.0 / 240.0, 0.05));
    let cam = c.camera.as_mut().unwrap();
    let moving = cam.step(dt, dragging);
    let reveal = (anim::since(ctx, c.shown_at, REVEAL_S) / REVEAL_S).clamp(0.0, 1.0) as f32;
    (View { eye: cam.now.eye(), forward: cam.now.forward(), fov_y: cam.now.fov, shift, reveal }, moving)
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
    let c = pv.current.as_mut().unwrap();
    let corner = rect.right() - 12.0 - if sheet { 40.0 } else { 0.0 };
    let id = Id::unique(("project-preview-cover", &project.path));
    if c.scene.is_none() {
        let note = if c.loading.running() {
            None
        } else if let Some(e) = &c.error {
            Some((icons::WARNING, "3D preview unreadable", format!("Its file could not be read: {e}"), None))
        } else if c.listed_at > f64::NEG_INFINITY && c.entries.is_empty() {
            Some(match &toolkit {
                Some((None, newest)) => (
                    icons::PLUS,
                    if updating { "Adding Toolkit…" } else { "Add Toolkit for 3D" },
                    format!(
                        "Orbiters Toolkit draws the scene you save in Unity here.{}",
                        if newest.is_some() {
                            " Press to add it, then save the scene once."
                        } else {
                            " Add the Orbiters repository, then Orbiters Toolkit below."
                        }
                    ),
                    newest.clone(),
                ),
                Some((Some(v), newest)) if version(v) < version(TOOLKIT_PREVIEWS) => (
                    super::art::glyph::ARROW_UP,
                    if updating { "Updating Toolkit…" } else { "Update Toolkit for 3D" },
                    format!(
                        "Orbiters Toolkit {v} in this project does not draw 3D previews yet ({TOOLKIT_PREVIEWS} does).{}",
                        match newest {
                            Some(n) => format!(" Press to update it to {n}: with the project open in Unity, its scene shows here once the editor reloaded."),
                            None => " Refresh the Orbiters repository to get the newer one.".into(),
                        }
                    ),
                    newest.clone(),
                ),
                _ => (
                    icons::CUBE,
                    "3D after a save",
                    "Save the scene in Unity (Ctrl+S): Orbiters Toolkit draws it and it shows here a moment later.".to_string(),
                    None,
                ),
            })
        } else {
            None
        };
        if let Some((glyph, label, tip, offer)) = note {
            let actionable = offer.is_some() && can_act;
            let k = anim::spring_from(&ctx, id.with("note"), [0.0], [1.0], Spring::SOFT)[0].clamp(0.0, 1.0);
            let txt = Txt::new(label, 11.5).weight(weight::MEDIUM);
            let w = widgets::text_size(ui, &txt).x + 36.0;
            let r = Rect::from_min_size(pos2(corner - w, rect.top() + 12.0), vec2(w, 24.0));
            let resp = ui.interact(r, id.with("note-tip"), if actionable { Sense::click() } else { Sense::hover() });
            let hover = anim::toggle(&ctx, id.with("note-hover"), actionable && resp.hovered(), Spring::SOFT);
            let press = anim::toggle(&ctx, id.with("note-press"), actionable && resp.is_pointer_button_down_on(), Spring::SNAPPY);
            let lit = if offer.is_some() { 1.0 } else { 0.0 };
            let r = r.shrink(1.5 * press);
            let p = ui.painter();
            p.rect_filled(r, theme::radius(12.0), theme::mix(theme::black(0.55 * k), theme::alpha(theme::INK_900, 0.9 * k), lit));
            if lit > 0.0 {
                p.rect_filled(r, theme::radius(12.0), theme::alpha(theme::ACCENT, (0.14 + 0.1 * hover) * k));
            }
            let fg = theme::mix(theme::mix(theme::FG_2, theme::ACCENT, lit), theme::FG, hover * 0.4);
            if updating && offer.is_some() {
                feedback::spinner(p, pos2(r.left() + 15.0, r.center().y), 5.0, 1.6, theme::alpha(theme::ACCENT, k));
            } else {
                widgets::icon(
                    p,
                    pos2(r.left() + 15.0, r.center().y - 1.5 * hover * lit),
                    glyph,
                    12.0,
                    theme::alpha(theme::mix(theme::FG_3, theme::ACCENT, lit), k),
                );
            }
            widgets::text(p, pos2(r.left() + 25.0, r.center().y), Align2::LEFT_CENTER, &txt.color(theme::alpha(fg, k)));
            let resp = if actionable { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
            resp.clone().on_hover_text(tip);
            if actionable && widgets::activated(&resp) {
                act = offer;
            }
        } else if c.loading.running() && c.scene.is_none() {
            let r = Rect::from_center_size(pos2(corner - 14.0, rect.top() + 24.0), vec2(28.0, 28.0));
            ui.painter().circle_filled(r.center(), 14.0, theme::black(0.5));
            feedback::spinner(ui.painter(), r.center(), 7.0, 2.0, theme::FG_2);
        }
        if let Some(v) = act {
            let installed = toolkit.as_ref().and_then(|t| t.0.clone());
            let label = if installed.is_some() { format!("Updating Orbiters Toolkit to {v}…") } else { format!("Adding Orbiters Toolkit {v}…") };
            let change = orbiters_projects::vpm::Change::Install { names: vec![(TOOLKIT.into(), Some(v))], prerelease: false };
            super::panel::package_action(state, &ctx, project, change, &label, TOOLKIT);
        }
        return;
    }
    if window_open {
        // The window grew out of here: its place stays dimmed until it is back.
        feedback::fill_corners(ui.painter(), rect, radii, theme::INK_950);
        return;
    }
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let camera = c.camera.as_mut().unwrap();
    c.touched |= steer(&ctx, &resp, camera, rect, false);
    let dragging = resp.dragged();
    // Above the title and its shade.
    let (view, moving) = frame_view(&ctx, c, dragging, 0.1);
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
        widgets::activated(&IconButton::new(icons::CORNERS_OUT, "Look around in 3D").size(32.0, 15.0).backdrop(theme::black(0.55)).show_at(&mut bu, button));
    if (open || resp.clicked()) && k > 0.5 {
        pv.open_window(&ctx, rect, radii[0]);
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
    egui::Area::new(wid.with("area")).order(Order::Foreground).fixed_pos(area.min).fade_in(false).show(&ctx, |ui| {
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
        let (view, moving) = frame_view(&ctx, c, resp.dragged(), 0.07);
        if moving {
            ctx.request_repaint();
        }
        let scene = c.scene.as_mut().unwrap();
        let (tex, uv) = scene.render(gpu, pixels(&ctx, f.rect), view);
        paint::squircle_texture(ui.painter(), f.rect, [theme::squircle(f.radius); 4], tex, uv, Color32::WHITE);
        let _ = resp.on_hover_cursor(if c.touched && ctx.input(|i| i.pointer.any_down()) { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });

        let mut cu = ui.new_child(UiBuilder::new().max_rect(f.rect).id_salt(wid.with("chrome")));
        cu.set_clip_rect(f.rect.intersect(ui.clip_rect()));
        cu.multiply_opacity(f.content);
        if closing || f.content < 0.5 {
            cu.disable();
        }
        let header = &c.scene.as_ref().unwrap().header;
        let written = c.entry().map(|e| e.modified);
        chrome(&mut cu, cx, &project, header, written, &c.entries, c.file.as_ref().map(|f| f.0.clone()), busy, f.rect, &mut act);
        if viewer::close_button(&mut cu, f.rect, "Close") {
            close = true;
        }
        // How to move around, until the view was turned.
        let shown = !c.touched && ctx.input(|i| i.time) - pv.opened_at < HINT_S;
        let hk = anim::toggle(&ctx, wid.with("hint"), shown, Spring::SOFT) * f.content;
        if hk > 0.01 {
            let txt = Txt::new("Drag to turn  ·  right-drag to move  ·  scroll to zoom  ·  double-click to frame", 12.0).color(theme::alpha(theme::FG_2, hk));
            let w = widgets::text_size(ui, &txt).x + 32.0;
            let y = f.rect.bottom() - 26.0 - if c.entries.len() > 1 { 52.0 } else { 0.0 } + 8.0 * (1.0 - hk);
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
    match act {
        Some(WindowAct::Scene(path)) => {
            c.chosen = Some(path);
        }
        Some(WindowAct::Reset) => {
            if let Some(cam) = c.camera.as_mut() {
                cam.reset();
            }
        }
        Some(WindowAct::Open) => {
            let p = project.clone();
            state.task(&ctx, "Opening Unity…", move || {
                orbiters_projects::open_project(&p)?;
                Ok(Outcome::Changed("Project sent to Unity".into()))
            });
        }
        None => {}
    }
    if close {
        if let Some(m) = state.preview.window.as_mut() {
            m.close();
        }
    }
}

enum WindowAct {
    Scene(PathBuf),
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
    entries: &[Entry],
    shown: Option<PathBuf>,
    busy: bool,
    rect: Rect,
    act: &mut Option<WindowAct>,
) {
    let p = ui.painter().clone();
    // Title, over a dark chip so it reads over any scene.
    let scene_name = if h.scene.name.is_empty() { project.name.clone() } else { h.scene.name.clone() };
    let saved = written
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format!("Saved {}", format::relative_time(d.as_secs(), orbiters_projects::discovery::now())))
        .unwrap_or_default();
    let sub = [project.name.as_str(), saved.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("  ·  ");
    let title = Txt::new(&scene_name, 16.0).weight(weight::SEMIBOLD).tracking(-0.01);
    let subt = Txt::new(&sub, 12.0).color(theme::FG_3);
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
    p.rect_filled(tile, theme::radius(10.0), theme::alpha(tint, 0.16));
    widgets::icon(&p, tile.center(), glyph, 16.0, tint);
    widgets::text_truncated(&p, pos2(tile.right() + 12.0, chip.top() + 19.0), Align2::LEFT_CENTER, &title, chip.right() - tile.right() - 24.0);
    widgets::text_truncated(&p, pos2(tile.right() + 12.0, chip.top() + 37.0), Align2::LEFT_CENTER, &subt, chip.right() - tile.right() - 24.0);

    // What it holds.
    let s = &h.stats;
    let mut facts = vec![];
    if !h.avatars.is_empty() {
        facts.push(if h.avatars.len() == 1 { "1 avatar".to_string() } else { format!("{} avatars", h.avatars.len()) });
    }
    facts.push(format!("{} triangles", compact(s.triangles)));
    facts.push(if h.materials.len() == 1 { "1 material".to_string() } else { format!("{} materials", h.materials.len()) });
    // Left out for the budget or unreadable (utility shaders and empty meshes are not worth a word).
    let skipped: u32 = s.skipped.iter().filter(|(k, _)| matches!(k.as_str(), "budget" | "missing")).map(|(_, n)| n).sum();
    let mut x = rect.left() + 14.0;
    let y = rect.bottom() - 14.0 - 15.0;
    for fact in &facts {
        let txt = Txt::new(fact, 12.0).color(theme::FG_2);
        let fw = widgets::text_size(ui, &txt).x + 24.0;
        let r = Rect::from_min_size(pos2(x, y - 15.0), vec2(fw, 30.0));
        p.rect_filled(r, theme::radius(15.0), theme::black(0.5));
        widgets::text(&p, r.center(), Align2::CENTER_CENTER, &txt);
        x = r.right() + 6.0;
    }
    if skipped > 0 {
        let r = Rect::from_min_size(pos2(x, y - 15.0), vec2(30.0, 30.0));
        p.circle_filled(r.center(), 15.0, theme::black(0.5));
        widgets::icon(&p, r.center(), icons::INFO, 14.0, theme::FG_3);
        let n = |k: &str| s.skipped.get(k).copied().unwrap_or(0);
        let why = match (n("budget"), n("missing")) {
            (0, m) => format!("{m} could not be read"),
            (b, 0) => format!("{b} beyond the preview's 2 million triangles"),
            (b, m) => format!("{b} beyond the preview's 2 million triangles, {m} could not be read"),
        };
        ui.interact(r, Id::unique("project-preview-skipped"), Sense::hover()).on_hover_text(format!("{skipped} meshes are not in this preview: {why}."));
    }

    // Open in Unity and framing again, bottom right.
    let b = Button::new("Open in Unity")
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
    if widgets::activated(
        &IconButton::new(icons::ARROW_COUNTER_CLOCKWISE, "Frame it again")
            .size(32.0, 15.0)
            .backdrop(theme::black(0.5))
            .id_salt("preview-reset")
            .show_at(ui, rr),
    ) {
        *act = Some(WindowAct::Reset);
    }

    // Other scenes of the project, to switch to.
    if entries.len() > 1 {
        let labels: Vec<(PathBuf, Txt)> = entries
            .iter()
            .map(|e| {
                let name = if e.header.scene.name.is_empty() { "Scene".to_string() } else { e.header.scene.name.clone() };
                let on = Some(&e.path) == shown.as_ref();
                (
                    e.path.clone(),
                    Txt::new(name, 12.5).weight(if on { weight::SEMIBOLD } else { weight::MEDIUM }).color(if on { theme::FG } else { theme::FG_2 }),
                )
            })
            .collect();
        let widths: Vec<f32> = labels.iter().map(|(_, t)| widgets::text_size(ui, t).x + 28.0).collect();
        let total = widths.iter().sum::<f32>() + 8.0;
        let row = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 30.0), vec2(total, 38.0));
        if row.width() < rect.width() - 2.0 * (bw + 60.0).max(x - rect.left()) {
            paint::fill_squircle(&p, row, theme::squircle(19.0), theme::black(0.5));
            let mut sx = row.left() + 4.0;
            for ((path, txt), w) in labels.iter().zip(&widths) {
                let r = Rect::from_min_size(pos2(sx, row.top() + 4.0), vec2(*w, 30.0));
                let on = Some(path) == shown.as_ref();
                let resp = ui.interact(r, Id::unique(("preview-scene", path)), Sense::click());
                let k = anim::toggle(ui.ctx(), Id::unique(("preview-scene-on", path)), on, Spring::SNAPPY);
                let hk = anim::toggle(ui.ctx(), Id::unique(("preview-scene-hover", path)), resp.hovered(), Spring::SNAPPY);
                if k > 0.01 || hk > 0.01 {
                    p.rect_filled(r, theme::radius(15.0), theme::white(0.13 * k + 0.05 * hk * (1.0 - k)));
                }
                widgets::text(&p, r.center(), Align2::CENTER_CENTER, txt);
                if widgets::activated(&resp) && !on {
                    *act = Some(WindowAct::Scene(path.clone()));
                }
                sx = r.right();
            }
        }
    }
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
/// selected project, and the presses that open its window.
impl State {
    pub fn stage_preview(&mut self, name: &str) {
        let dir = std::env::temp_dir().join("orbiters-preview-stand-in");
        let folder = dir.join(file::FOLDER);
        for old in ["stand-in.preview", "second.preview"] {
            let _ = std::fs::remove_file(folder.join(old));
        }
        if !matches!(name, "projects-preview-none" | "projects-preview-no-toolkit" | "projects-preview-old-toolkit") {
            std::fs::create_dir_all(&folder).expect("stand-in folder");
            let bytes = match std::env::var_os("ORBITERS_PREVIEW_FILE") {
                Some(p) => std::fs::read(p).expect("ORBITERS_PREVIEW_FILE"),
                None => stand_in(),
            };
            std::fs::write(folder.join("stand-in.preview"), bytes).expect("stand-in preview");
            if name == "projects-preview-scenes" {
                std::fs::write(folder.join("second.preview"), stand_in_scene("Club stage")).expect("stand-in preview");
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
        match (name, frame) {
            ("projects-preview-open" | "projects-preview-open-mid" | "projects-preview-scenes", 90) => at("preview-cover").map(Poke::Press),
            ("projects-preview-turned", 100) => at("preview-cover").map(Poke::Hold),
            ("projects-preview-turned", 101..=112) => at("preview-cover").map(|p| Poke::Hover(p + vec2(9.0 * (frame - 100) as f32, 0.0))),
            ("projects-preview-sheet", 30) => at("card-0").map(Poke::Press),
            _ => None,
        }
    }
}

/// A stand-in scene for screenshots: N4N0's black orb with white strokes floating over a plinth, a ring round it and
/// two cubes, as an avatar scene.
fn stand_in() -> Vec<u8> {
    stand_in_scene("Avatar")
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
        });
    };
    add_mesh(&mut w, sphere(0.34, 48, 32));
    add_mesh(&mut w, cylinder(0.5, 0.42, 0.12, 64));
    add_mesh(&mut w, torus(0.52, 0.018, 96, 12));
    add_mesh(&mut w, cuboid(0.16));
    // The orb's strokes: white curves on black.
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
    let texture = Texture { width: tw, height: th, format: ImageFormat::Png, data: w.push(&png_bytes) };
    let mat = |name: &str, color: [f32; 4], texture: i32| Material {
        name: name.into(),
        color,
        texture,
        tiling: [1.0, 1.0],
        offset: [0.0, 0.0],
        alpha: Alpha::Opaque,
        cutoff: 0.5,
        cull: Cull::Back,
    };
    let materials = vec![
        mat("Orb", [1.0, 1.0, 1.0, 1.0], 0),
        mat("Plinth", [0.36, 0.36, 0.40, 1.0], -1),
        mat("Ring", [0.0, 0.85, 0.43, 1.0], -1),
        mat("Cube", [0.86, 0.55, 0.33, 1.0], -1),
        mat("Cube 2", [0.42, 0.52, 0.92, 1.0], -1),
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
        .map(|(i, m, mat)| Draw { instance: i, mesh: m, materials: vec![mat], shadows: true, avatar: Some(0) })
        .to_vec();
    let bounds = Bounds { min: [-0.72, 0.0, -0.55], max: [0.78, 0.96, 0.55] };
    let header = Header {
        written_utc: "2026-10-29T21:40:00Z".into(),
        generator: "Orbiters - N4N0 (stand-in)".into(),
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

    #[test]
    fn compact_counts() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(4_210), "4.2k");
        assert_eq!(compact(312_400), "312k");
        assert_eq!(compact(1_840_000), "1.8M");
    }
}
