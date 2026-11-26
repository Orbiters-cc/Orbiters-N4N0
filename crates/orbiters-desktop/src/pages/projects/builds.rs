//! Previews built from the projects' own files (`orbiters_projects::scene`, docs/project-preview.md) and the gallery
//! cards' stills, on one background worker, so that a gallery of dozens of projects never holds the window.
//!
//! - **Builds** of the scene the panel shows (or the window switches to) come first, one at a time: reading a scene's
//!   prefabs, models and textures takes seconds (a minute the first time on a hard disk), then the result is kept in
//!   the app's cache and every later look at that scene is instant until a file it read changes.
//! - **Stills** come after: a project whose card has no avatar picture gets one of its last-open scene (built first
//!   when needed), drawn once by the same renderer as the 3D view into a picture of its own on this worker, read back
//!   and kept as a JPEG next to the cached preview (`cover.jpg`, with what it was drawn from in `cover.json`). A still
//!   kept from before shows at once; it is drawn again only when its preview changed. They are asked for in the
//!   gallery's order, whatever is on screen, and only change a card's picture, never its place or size.
//! - **Nothing waits behind a still.** A build the panel or the window asks for stops the still (or the other build)
//!   running, which goes back in its queue and starts over later: a gallery of large projects drawing its stills never
//!   keeps the selected project's preview waiting for minutes. A stopped build keeps nothing ([`scene::STOPPED`]).
//! - **A job never takes the worker down.** A panic in a build (a file no reader foresaw) is caught: that scene says
//!   it could not be read, kept as a failure until its files change, and the worker goes on with the next job.
//! - **The worker stops with the app**: dropping the [`Builder`] stops the job running at its next check.

use super::preview_gpu::{self, Pipelines};
use eframe::egui_wgpu::wgpu;
use egui::ColorImage;
use orbiters_projects::preview::Entry;
use orbiters_projects::scene::{self, SceneInfo, Slot, Source};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long one build may take before it stops with a reason.
const BUILD_LIMIT: Duration = Duration::from_secs(120);
/// A gallery card's still (16:9, sharp on a 1.5× display).
const STILL: (u32, u32) = (640, 360);

/// What the worker finished.
pub(super) enum Done {
    /// A scene's preview built (its file), or why not.
    Built { project: PathBuf, guid: String, result: Result<Entry, scene::Failure> },
    /// A card's still (one kept from before first, then a new one if its preview changed).
    Still { project: PathBuf, image: ColorImage },
    /// A project gets no still (no scene, or none that builds).
    NoStill { project: PathBuf },
}

#[derive(Default)]
struct Queue {
    builds: VecDeque<(PathBuf, SceneInfo)>,
    stills: VecDeque<PathBuf>,
    /// What runs now.
    running: Option<Running>,
    scene_light: bool,
}

/// The job running: its project, the scene being built (none while drawing a still), and what stops it.
struct Running {
    project: PathBuf,
    guid: Option<String>,
    cancel: Arc<AtomicBool>,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    /// The app is closing: the worker ends.
    stop: AtomicBool,
}

impl Shared {
    /// The queue, also after a panic while it was held (a job's panic is caught outside it; this is for the rest).
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The worker and what it was asked.
pub(super) struct Builder {
    shared: Arc<Shared>,
    done: Option<mpsc::Receiver<Done>>,
    /// Projects whose still was asked for this session.
    stills_asked: HashSet<PathBuf>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue { scene_light: true, ..Default::default() }),
                wake: Condvar::new(),
                stop: AtomicBool::new(false),
            }),
            done: None,
            stills_asked: HashSet::new(),
        }
    }
}

impl Builder {
    /// Starts the worker on the window's device (once).
    pub fn start(&mut self, ctx: &egui::Context, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.done.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.done = Some(rx);
        let shared = self.shared.clone();
        let (ctx, device, queue) = (ctx.clone(), device.clone(), queue.clone());
        let _ = std::thread::Builder::new().name("project-preview-builds".into()).spawn(move || work(&shared, &tx, &ctx, &device, &queue));
    }

    /// Builds `scene` of `project` before anything else waiting (the panel or the window wants it now): the still or
    /// other build running stops and goes back in its queue.
    pub fn build(&self, project: &Path, scene: &SceneInfo) {
        let mut q = self.shared.queue();
        if q.running.as_ref().is_some_and(|r| r.project == project && r.guid.as_deref() == Some(scene.guid.as_str())) {
            return;
        }
        q.builds.retain(|(p, s)| !(p == project && s.guid == scene.guid));
        q.builds.push_front((project.to_path_buf(), scene.clone()));
        if let Some(r) = &q.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.shared.wake.notify_one();
    }

    /// A still for `project`'s card, after the builds (once a session).
    pub fn still(&mut self, project: &Path) {
        if self.stills_asked.contains(project) {
            return;
        }
        self.stills_asked.insert(project.to_path_buf());
        let mut q = self.shared.queue();
        q.stills.push_back(project.to_path_buf());
        self.shared.wake.notify_one();
    }

    /// The light stills are drawn in (the 3D view's choice).
    pub fn set_scene_light(&self, on: bool) {
        self.shared.queue().scene_light = on;
    }

    /// Whether `scene` of `project` is being built or waits to be.
    pub fn building(&self, project: &Path, guid: &str) -> bool {
        let q = self.shared.queue();
        q.running.as_ref().is_some_and(|r| r.project == project && r.guid.as_deref() == Some(guid))
            || q.builds.iter().any(|(p, s)| p == project && s.guid == guid)
    }

    /// What finished since the last frame.
    pub fn poll(&self) -> Vec<Done> {
        self.done.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default()
    }
}

impl Drop for Builder {
    /// The app closing: the job running stops at its next check and the worker ends.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(r) = &self.shared.queue().running {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.shared.wake.notify_all();
    }
}

/// What a panic in a job says on its scene's chip and card (kept until its files change).
const CRASHED: &str = orbiters_i18n::mark("an error in Orbiters - N4N0 while reading it");

/// The next job, marked running, with the light and what stops it; none once the app is closing.
fn next(shared: &Shared) -> Option<(Job, bool, Arc<AtomicBool>)> {
    let mut q = shared.queue();
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return None;
        }
        let job = match q.builds.pop_front() {
            Some((p, s)) => Some(Job::Build(p, s)),
            None => q.stills.pop_front().map(Job::Still),
        };
        if let Some(job) = job {
            let cancel = Arc::new(AtomicBool::new(false));
            let (project, guid) = match &job {
                Job::Build(p, s) => (p.clone(), Some(s.guid.clone())),
                Job::Still(p) => (p.clone(), None),
            };
            q.running = Some(Running { project, guid, cancel: cancel.clone() });
            return Some((job, q.scene_light, cancel));
        }
        q = shared.wake.wait(q).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

fn work(shared: &Shared, tx: &mpsc::Sender<Done>, ctx: &egui::Context, device: &wgpu::Device, queue: &wgpu::Queue) {
    scene::Limits { background: true, ..Default::default() }.lower_priority();
    let mut pipelines: Option<Arc<Pipelines>> = None;
    while let Some((job, scene_light, cancel)) = next(shared) {
        let send = |d: Done| {
            let _ = tx.send(d);
            ctx.request_repaint();
        };
        let run = std::panic::AssertUnwindSafe(|| match &job {
            Job::Build(project, scene) => match build(project, scene, &cancel) {
                Err(why) if why.why == scene::STOPPED => Outcome::Stopped,
                result => {
                    send(Done::Built { project: project.clone(), guid: scene.guid.clone(), result });
                    Outcome::Done
                }
            },
            Job::Still(project) => match still(project, scene_light, device, queue, &mut pipelines, &send, &cancel) {
                Still::New(image) => {
                    send(Done::Still { project: project.clone(), image });
                    Outcome::Done
                }
                Still::Kept => Outcome::Done,
                Still::None => {
                    send(Done::NoStill { project: project.clone() });
                    Outcome::Done
                }
                Still::Stopped => Outcome::Stopped,
            },
        });
        let outcome = std::panic::catch_unwind(run).unwrap_or_else(|panic| {
            let what = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
            match &job {
                Job::Build(project, scene) => {
                    eprintln!("project preview of {}: panicked ({what})", scene.rel);
                    scene::keep_failure(project, scene, CRASHED);
                    send(Done::Built { project: project.clone(), guid: scene.guid.clone(), result: Err(CRASHED.into()) });
                }
                Job::Still(project) => {
                    eprintln!("project still of {}: panicked ({what})", project.display());
                    send(Done::NoStill { project: project.clone() });
                }
            }
            Outcome::Done
        });
        let mut q = shared.queue();
        q.running = None;
        if matches!(outcome, Outcome::Stopped) && !shared.stop.load(Ordering::Relaxed) {
            // Stopped for a build wanted first: back in its queue, after what stopped it.
            match job {
                Job::Build(p, s) => {
                    if !q.builds.iter().any(|(bp, bs)| *bp == p && bs.guid == s.guid) {
                        let at = q.builds.len().min(1);
                        q.builds.insert(at, (p, s));
                    }
                }
                Job::Still(p) => q.stills.push_front(p),
            }
        }
    }
}

enum Job {
    Build(PathBuf, SceneInfo),
    Still(PathBuf),
}

enum Outcome {
    Done,
    /// Stopped before it finished (a build wanted first, or the app closing).
    Stopped,
}

fn build(project: &Path, scene: &SceneInfo, cancel: &Arc<AtomicBool>) -> Result<Entry, scene::Failure> {
    // Built meanwhile (a card's still and the panel asked for the same scene).
    if let Some(e) = scene::fresh(project, &scene.guid) {
        return Ok(e);
    }
    let start = std::time::Instant::now();
    let limits = scene::Limits { background: true, cancel: Some(cancel.clone()), ..scene::Limits::new(BUILD_LIMIT) };
    let result = scene::build_scene(project, scene, &limits);
    match &result {
        Ok(e) => eprintln!("project preview built: {} in {:.1} s ({:.1} MB)", scene.rel, start.elapsed().as_secs_f64(), e.size as f64 / 1e6),
        Err(why) => eprintln!("project preview of {} not built in {:.1} s: {why}", scene.rel, start.elapsed().as_secs_f64()),
    }
    result
}

/// The scene a project opens on: the first listed (the one Unity had open last), unless it cannot be built and
/// another has a preview.
pub(super) fn default_slot(slots: &[Slot]) -> Option<&Slot> {
    let first = slots.first()?;
    if matches!(first.source, Source::Failed(_)) {
        if let Some(other) = slots.iter().find(|s| s.source.entry().is_some()) {
            return Some(other);
        }
    }
    Some(first)
}

/// A project's still drawn on the calling thread (screenshots).
pub(super) fn still_now(project: &Path, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<ColorImage> {
    let mut pipelines = None;
    match still(project, true, device, queue, &mut pipelines, &|_| {}, &Arc::new(AtomicBool::new(false))) {
        Still::New(image) => Some(image),
        Still::Kept => read_jpeg(&scene::cache_dir(project).join("cover.jpg")),
        Still::None | Still::Stopped => None,
    }
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct StillRecord {
    preview: String,
    modified_ms: u64,
    scene_light: bool,
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn read_jpeg(path: &Path) -> Option<ColorImage> {
    let rgb = image::open(path).ok()?.to_rgb8();
    Some(ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw()))
}

enum Still {
    New(ColorImage),
    /// The one kept (already sent) is still right.
    Kept,
    None,
    /// Its scene's build was stopped (a build wanted first): to draw again later.
    Stopped,
}

/// A project's still: the one kept is sent at once; drawn again when its preview changed.
fn still(
    project: &Path,
    scene_light: bool,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &mut Option<Arc<Pipelines>>,
    send: &dyn Fn(Done),
    cancel: &Arc<AtomicBool>,
) -> Still {
    let mut stopped = false;
    match draw_still(project, scene_light, device, queue, pipelines, send, cancel, &mut stopped) {
        _ if stopped => Still::Stopped,
        Some(Some(image)) => Still::New(image),
        Some(None) => Still::Kept,
        None => Still::None,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_still(
    project: &Path,
    scene_light: bool,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &mut Option<Arc<Pipelines>>,
    send: &dyn Fn(Done),
    cancel: &Arc<AtomicBool>,
    stopped: &mut bool,
) -> Option<Option<ColorImage>> {
    let dir = scene::cache_dir(project);
    let jpg = dir.join("cover.jpg");
    let kept_record: Option<StillRecord> = std::fs::read(dir.join("cover.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    // What was kept shows while the scene is looked at again.
    if kept_record.is_some() {
        if let Some(img) = read_jpeg(&jpg) {
            send(Done::Still { project: project.to_path_buf(), image: img });
        }
    }
    let slots = scene::list(project);
    let slot = default_slot(&slots)?;
    let entry = match &slot.source {
        Source::Toolkit(e) => e.clone(),
        Source::Built(e) => {
            scene::touch(project, &slot.scene.guid);
            e.clone()
        }
        Source::Stale(_) | Source::None => {
            let result = build(project, &slot.scene, cancel);
            if result.as_ref().err().is_some_and(|why| why.why == scene::STOPPED) {
                *stopped = true;
                return None;
            }
            send(Done::Built { project: project.to_path_buf(), guid: slot.scene.guid.clone(), result: result.clone() });
            result.ok()?
        }
        Source::Failed(_) => return None,
    };
    let record = StillRecord { preview: entry.path.to_string_lossy().into_owned(), modified_ms: ms(entry.modified), scene_light };
    if kept_record.as_ref() == Some(&record) && jpg.is_file() {
        return Some(None);
    }
    let start = std::time::Instant::now();
    let mut s = preview_gpu::load(device, queue, pipelines.clone(), &entry.path).map_err(|e| eprintln!("project still: {e:#}")).ok()?;
    pipelines.get_or_insert_with(|| s.pipelines());
    let light = preview_gpu::light_mode(scene_light, &s.header);
    let view = super::preview::home_view(&s.header, light);
    let img = s.still(device, queue, STILL, view).map_err(|e| eprintln!("project still: {e:#}")).ok()?;
    let rgb: Vec<u8> = img.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut bytes = Vec::new();
    if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 88).encode(&rgb, STILL.0, STILL.1, image::ExtendedColorType::Rgb8).is_ok() {
        keep_still(&dir, &bytes, &serde_json::to_vec(&record).unwrap_or_default());
    }
    eprintln!("project still of {}: {:.0} ms", project.display(), start.elapsed().as_secs_f64() * 1000.0);
    Some(Some(ColorImage::from_rgb([STILL.0 as usize, STILL.1 as usize], &rgb)))
}

/// A still kept in `dir`, with what it was drawn from.
pub(super) fn keep_still(dir: &Path, jpeg: &[u8], record: &[u8]) -> bool {
    orbiters_projects::may_write(dir).is_ok() && orbiters_core::write_atomic(&dir.join("cover.jpg"), jpeg).is_ok() && orbiters_core::write_atomic(&dir.join("cover.json"), record).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &str, source: Source) -> Slot {
        Slot {
            scene: SceneInfo {
                path: PathBuf::from(format!("P/Assets/{name}.unity")),
                rel: format!("Assets/{name}.unity"),
                guid: name.repeat(32).chars().take(32).collect(),
                name: name.into(),
                modified: UNIX_EPOCH,
                last_open: false,
                in_build: false,
            },
            source,
        }
    }

    #[test]
    fn a_project_opens_on_its_last_scene_unless_that_one_cannot_be_built() {
        let entry = Entry { path: PathBuf::from("x.preview"), modified: UNIX_EPOCH, size: 1, header: Default::default() };
        let slots = vec![slot("a", Source::None), slot("b", Source::Built(entry.clone()))];
        assert_eq!(default_slot(&slots).unwrap().scene.name, "a", "not built yet: it will be");
        let slots = vec![slot("a", Source::Failed("why".into())), slot("b", Source::Toolkit(entry))];
        assert_eq!(default_slot(&slots).unwrap().scene.name, "b");
        let slots = vec![slot("a", Source::Failed("why".into())), slot("b", Source::None)];
        assert_eq!(default_slot(&slots).unwrap().scene.name, "a", "nothing better: its reason shows");
        assert!(default_slot(&[]).is_none());
    }

    #[test]
    fn the_worker_puts_the_panels_build_first() {
        let b = Builder::default();
        let s = |n: &str| slot(n, Source::None).scene;
        b.build(Path::new("P"), &s("a"));
        b.build(Path::new("P"), &s("b"));
        b.build(Path::new("P"), &s("a"));
        let q = b.shared.queue();
        let order: Vec<&str> = q.builds.iter().map(|(_, s)| s.name.as_str()).collect();
        assert_eq!(order, ["a", "b"], "asked again: first, once");
        drop(q);
        assert!(b.building(Path::new("P"), &s("b").guid));
        assert!(!b.building(Path::new("Q"), &s("b").guid));
    }

    #[test]
    fn a_build_asked_for_stops_the_still_running_and_the_app_closing_stops_everything() {
        let mut b = Builder::default();
        let s = |n: &str| slot(n, Source::None).scene;
        b.still(Path::new("Gallery"));
        let (job, _, still_cancel) = next(&b.shared).unwrap();
        assert!(matches!(job, Job::Still(_)));
        // The panel wants a scene now: the still stops (and is drawn later), the scene comes next.
        b.build(Path::new("P"), &s("a"));
        assert!(still_cancel.load(Ordering::Relaxed), "the still running was told to stop");
        b.shared.queue().running = None;
        let (job, _, cancel) = next(&b.shared).unwrap();
        assert!(matches!(&job, Job::Build(_, sc) if sc.name == "a"));
        // The same scene asked again while it runs: nothing stops.
        b.build(Path::new("P"), &s("a"));
        assert!(!cancel.load(Ordering::Relaxed));
        drop(b);
        assert!(cancel.load(Ordering::Relaxed), "closing the app stops the build running");
    }
}
