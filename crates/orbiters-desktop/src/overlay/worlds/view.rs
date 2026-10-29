//! The capture shown around the person: the cube's twelve SteamVR overlays (two cubes of six faces, the second taking
//! over when the head wanders off the first's centre), the render thread that draws their pictures, and what decides,
//! each turn of the overlay's loop, what to draw (stereo.rs explains why the faces stay right as the head turns).
//!
//! Cost: a new cube draws its six faces for both eyes once. After that a batch draws one wide view per eye (its field of
//! view and [`WIDE_MARGIN`] around it, at [`WIDE_DENSITY`] of the faces' density) and resamples it into the faces it
//! covers: two views of the splats a batch, whatever the faces seen. A batch goes only once an eye moved [`MOVED_M`] or
//! the head turned [`TURNED`] since the last one, at most [`Quality::rate`] a second, one at a time on the GPU: a still
//! head costs nothing. When SteamVR says VRChat's last frames came late for the GPU (dropped, or reprojected), batches
//! stop for a while, 50 ms then twice as long each time up to 2 s (`orbiters_reconstruct::governor`, as training
//! beside VRChat does): parallax lags, VRChat keeps its frames. Now and then a face out of view is drawn again whole,
//! should the head turn to it faster than the batches follow.

use super::catalog::Capture;
use super::render::FaceRenderer;
use super::stereo::{self, Cube, Placement, PlayCamera, V3};
use orbiters_openvr::{self as vr, compositor::Eye, overlay_flag, Pose, Universe, MAX_DEVICES};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Metres from the cube's centre to its faces.
pub const HALF_M: f32 = 1.5;
/// How far (each axis) an eye may go from the cube's centre before a new cube takes over.
pub const REACH_M: f32 = 0.45;
/// Texels past each side of a face (the overlap that hides seams).
pub const MARGIN: u32 = 4;
/// A batch goes once an eye moved this far, or the head turned this far (radians) since the last one.
pub const MOVED_M: f32 = 0.0025;
pub const TURNED: f32 = 0.07;
/// How much wider than the eye's field of view (radians, every side) a wide view is: the head may turn that far before
/// the next batch without reaching pictures drawn from an older place.
pub const WIDE_MARGIN: f32 = 0.105;
/// A wide view's pixels per unit of tangent, against the faces' at their centres (its edges are far denser: a
/// pinhole view's density grows towards its sides).
pub const WIDE_DENSITY: f32 = 0.8;
/// A wide view's pixels across at most.
const WIDE_MAX: u32 = 3072;
/// The faces' margin for being counted in view (radians), for the faces drawn again whole.
pub const TURN_MARGIN: f32 = 0.45;
/// A face out of view is drawn again whole at most this often, once an eye moved [`STALE_M`] since it was.
const UNSEEN_EVERY: Duration = Duration::from_millis(300);
const STALE_M: f32 = 0.03;
/// The capture appears from its start view outward over this long (the world viewer's revealSeconds), fading in over
/// [`FADE_IN`]; it fades out over [`FADE_OUT`] when hidden.
pub const REVEAL: Duration = Duration::from_millis(900);
const FADE_IN: Duration = Duration::from_millis(450);
const FADE_OUT: Duration = Duration::from_millis(350);
/// The overlays' alpha with "See VRChat" on.
pub const SEE_THROUGH_ALPHA: f32 = 0.55;

/// How sharp the faces are: pixels per eye across a face.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Quality {
    Performance,
    #[default]
    Balanced,
    High,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::Performance, Quality::Balanced, Quality::High];

    pub fn pixels(self) -> u32 {
        match self {
            Quality::Performance => 768,
            Quality::Balanced => 1024,
            Quality::High => 1440,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Performance => "Light",
            Quality::Balanced => "Balanced",
            Quality::High => "Sharp",
        }
    }

    /// Batches a second at most: how often parallax follows the eyes.
    pub fn rate(self) -> f32 {
        match self {
            Quality::Performance => 20.0,
            Quality::Balanced => 30.0,
            Quality::High => 45.0,
        }
    }

    /// The wide views' pixels per unit of tangent.
    pub fn wide_per_tan(self) -> f32 {
        self.pixels() as f32 / 2.0 * WIDE_DENSITY
    }
}

/// What a batch draws.
#[derive(Clone, Debug)]
pub enum Work {
    /// Faces drawn whole, for both eyes: a new cube's six, or one out of view now and then. (face, texture slot)
    Faces(Vec<(usize, usize)>),
    /// Each eye's wide view, resampled into the faces it covers. (face, texture slot)
    Wide { cameras: [PlayCamera; 2], faces: Vec<(usize, usize)> },
}

impl Work {
    pub fn faces(&self) -> &[(usize, usize)] {
        match self {
            Work::Faces(f) | Work::Wide { faces: f, .. } => f,
        }
    }
}

/// A batch of pictures to draw.
pub struct Job {
    pub id: u64,
    pub cube: Cube,
    /// Which of the two cubes it is for.
    pub cube_index: usize,
    pub placement: Placement,
    pub eyes: [V3; 2],
    pub work: Work,
    /// Splats drawn (the first ones: the reveal).
    pub count: usize,
    /// The shared fence value the GPU waits for first.
    pub wait: u64,
}

/// Draws a batch on `renderer`: into the faces' pictures, then copied to `targets` (face × 2 + slot).
pub fn draw(renderer: &mut FaceRenderer, job: &Job, targets: &[&eframe::wgpu::Texture]) -> anyhow::Result<bool> {
    let publish: Vec<(usize, &eframe::wgpu::Texture)> = job.work.faces().iter().map(|&(f, slot)| (f, targets[f * 2 + slot])).collect();
    match &job.work {
        Work::Faces(faces) => {
            for &(face, _) in faces {
                for eye in 0..2 {
                    renderer.render_master(&job.cube, &job.placement, job.eyes[eye], eye, face, job.count)?;
                }
            }
            renderer.resample(&job.cube, &[], &publish);
        }
        Work::Wide { cameras, faces } => {
            for (eye, camera) in cameras.iter().enumerate() {
                renderer.render_wide(eye, camera, &job.placement, job.count)?;
            }
            let jobs: Vec<(usize, usize, PlayCamera)> =
                faces.iter().flat_map(|&(f, _)| (0..2).filter(move |&e| job.cube.face_in(f, &cameras[e])).map(move |e| (f, e, cameras[e]))).collect();
            renderer.resample(&job.cube, &jobs, &publish);
        }
    }
    renderer.finish()
}

enum Ask {
    Load(Arc<Capture>),
    Width(u32),
    Render(Job),
}

enum Answer {
    #[cfg(windows)]
    Textures(super::interop::Handles),
    Loaded,
    Rendered { id: u64, cube_index: usize, work: Work, eyes: [V3; 2], count: usize, ms: f64, redraw: bool },
    /// Not drawn (asked for textures of another size).
    Skipped { id: u64, cube_index: usize },
    Failed(String),
}

/// The render thread: a device on the headset's GPU, the capture's splats, the shared face textures.
struct Thread {
    tx: mpsc::Sender<Ask>,
    rx: mpsc::Receiver<Answer>,
}

impl Thread {
    fn start(luid: u64) -> Self {
        let (tx, asks) = mpsc::channel::<Ask>();
        let (answers, rx) = mpsc::channel::<Answer>();
        let _ = std::thread::Builder::new().name("3d-worlds-render".into()).spawn(move || {
            if let Err(e) = Self::run(luid, asks, &answers) {
                let _ = answers.send(Answer::Failed(format!("{e:#}")));
            }
        });
        Self { tx, rx }
    }

    fn run(luid: u64, asks: mpsc::Receiver<Ask>, answers: &mpsc::Sender<Answer>) -> anyhow::Result<()> {
        let (_adapter, device, queue) = super::render::device(Some(luid))?;
        crate::gpu::report_errors(&device);
        let mut renderer = FaceRenderer::new(device.clone(), queue.clone());
        #[cfg(windows)]
        let mut side: Option<super::interop::RenderSide> = None;
        while let Ok(ask) = asks.recv() {
            match ask {
                Ask::Load(capture) => {
                    renderer.load(&capture)?;
                    let _ = answers.send(Answer::Loaded);
                }
                Ask::Width(width) => {
                    #[cfg(windows)]
                    {
                        // The old textures go before the new ones are made.
                        drop(side.take());
                        let (made, handles) = super::interop::RenderSide::new(&device, 12, width)?;
                        side = Some(made);
                        let _ = answers.send(Answer::Textures(handles));
                    }
                    #[cfg(not(windows))]
                    let _ = width;
                }
                Ask::Render(job) => {
                    #[cfg(windows)]
                    {
                        let fits = side.as_ref().is_some_and(|s| s.textures.first().is_some_and(|t| t.width() == job.cube.width() * 2));
                        let Some(side) = side.as_ref().filter(|_| fits) else {
                            // Asked before the textures of a new quality: the next batch draws it.
                            let _ = answers.send(Answer::Skipped { id: job.id, cube_index: job.cube_index });
                            continue;
                        };
                        let targets: Vec<&eframe::wgpu::Texture> = side.textures.iter().collect();
                        let (drawn, ms) = super::render::timed(|| -> anyhow::Result<bool> {
                            side.wait(&queue, job.wait)?;
                            draw(&mut renderer, &job, &targets)
                        });
                        let redraw = drawn?;
                        let _ = answers.send(Answer::Rendered { id: job.id, cube_index: job.cube_index, work: job.work, eyes: job.eyes, count: job.count, ms, redraw });
                    }
                    #[cfg(not(windows))]
                    let _ = answers.send(Answer::Skipped { id: job.id, cube_index: job.cube_index });
                }
            }
        }
        Ok(())
    }
}

/// What the cube is doing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Nothing shows.
    Off,
    /// A capture is on its way to the GPU.
    Loading,
    /// Shown since `since`, appearing first.
    Shown { since: Instant },
    /// Fading out since `since`; off after.
    Leaving { since: Instant },
}

/// Where each face of a cube was last drawn from (between the eyes), and how many splats it drew.
#[derive(Clone, Copy, Debug, Default)]
struct Drawn {
    from: Option<V3>,
    count: usize,
}

/// The capture around the person, in SteamVR.
pub struct View {
    key: String,
    handles: Option<[[vr::OverlayHandle; 6]; 2]>,
    thread: Option<Thread>,
    #[cfg(windows)]
    d3d11: Option<super::interop::OverlaySide>,
    capture: Option<Arc<Capture>>,
    placement: Option<Placement>,
    quality: Quality,
    /// The width the shared textures have (0: none yet).
    width: u32,
    phase: Phase,
    loaded: bool,
    cubes: [Option<Cube>; 2],
    active: usize,
    /// The cube that takes over once its six faces are drawn.
    switching: Option<usize>,
    /// The texture slot each face shows on the active cube.
    shown_slot: [usize; 6],
    drawn: [Drawn; 6],
    /// The last wide batch: its eyes, the way the head looked, its splat count (None: a batch is due).
    last_wide: Option<([V3; 2], V3, usize)>,
    in_flight: Option<u64>,
    next_id: u64,
    last_job: Option<Instant>,
    asked_at: Option<Instant>,
    /// Seconds a batch takes, from asking to its answer (smoothed): how far ahead the eyes are predicted.
    batch_s: f32,
    wait: u64,
    alpha: f32,
    pub see_through: bool,
    unseen_turn: usize,
    last_unseen: Option<Instant>,
    eyes_read: Option<Instant>,
    eye_to_head: [vr::Matrix34; 2],
    tangents: [[f32; 4]; 2],
    /// VRChat's frames came late: no batch before this, and the next back-off's length; the newest late frame answered.
    held_until: Option<Instant>,
    backoff: Duration,
    answered: u32,
    /// Back-offs so far (the panel's numbers).
    pub backoffs: u32,
    /// What went wrong last, for the panel.
    pub error: Option<String>,
    /// Batches drawn and the GPU's time a batch (smoothed), for the panel's numbers.
    pub batches: u64,
    pub batch_ms: f32,
}

impl View {
    pub fn new(key: &str) -> Self {
        Self {
            key: format!("{key}.world"),
            handles: None,
            thread: None,
            #[cfg(windows)]
            d3d11: None,
            capture: None,
            placement: None,
            quality: Quality::default(),
            width: 0,
            phase: Phase::Off,
            loaded: false,
            cubes: [None; 2],
            active: 0,
            switching: None,
            shown_slot: [0; 6],
            drawn: [Drawn::default(); 6],
            last_wide: None,
            in_flight: None,
            next_id: 1,
            last_job: None,
            asked_at: None,
            batch_s: 0.012,
            wait: 0,
            alpha: 0.0,
            see_through: false,
            unseen_turn: 0,
            last_unseen: None,
            eyes_read: None,
            eye_to_head: [vr::Matrix34::IDENTITY; 2],
            tangents: [[-1.2, 1.2, -1.2, 1.2]; 2],
            held_until: None,
            backoff: Duration::ZERO,
            answered: 0,
            backoffs: 0,
            error: None,
            batches: 0,
            batch_ms: 0.0,
        }
    }

    /// Whether a capture shows (or appears, or is on its way).
    pub fn active(&self) -> bool {
        !matches!(self.phase, Phase::Off | Phase::Leaving { .. })
    }

    /// Whether the capture is still on its way to the GPU.
    pub fn preparing(&self) -> bool {
        matches!(self.phase, Phase::Loading)
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Starts the render thread early (its device and shaders take a moment), while the capture downloads.
    pub fn warm_up(&mut self, steamvr: &vr::Client) {
        if self.thread.is_some() {
            return;
        }
        let Some(luid) = steamvr.dxgi_adapter_index().and_then(super::interop::luid_of_index) else {
            self.error = Some("SteamVR did not say which graphics card the headset uses.".into());
            return;
        };
        #[cfg(windows)]
        match super::interop::OverlaySide::new(luid) {
            Ok(side) => self.d3d11 = Some(side),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        }
        self.thread = Some(Thread::start(luid));
        self.width = 0;
    }

    /// Shows `capture`, its start view over where the head stands now (play space).
    pub fn show(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client, capture: Arc<Capture>) {
        self.error = None;
        self.warm_up(steamvr);
        let Some(thread) = &self.thread else { return };
        if self.handles.is_none() {
            match self.create(overlay) {
                Ok(h) => self.handles = Some(h),
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        }
        self.hide_all(overlay);
        self.placement = Some(Placement::at(&capture.view, &head_pose(steamvr, 0.0)));
        let _ = thread.tx.send(Ask::Load(capture.clone()));
        if self.width != self.quality.pixels() + 2 * MARGIN {
            let _ = thread.tx.send(Ask::Width(self.quality.pixels() + 2 * MARGIN));
        }
        self.capture = Some(capture);
        self.loaded = false;
        self.cubes = [None; 2];
        self.switching = None;
        self.forget_drawn();
        self.phase = Phase::Loading;
        self.alpha = 0.0;
    }

    fn forget_drawn(&mut self) {
        self.drawn = [Drawn::default(); 6];
        self.last_wide = None;
    }

    /// Fades the capture out, then lets go of it (and of the GPU's memory).
    pub fn hide(&mut self) {
        if self.active() {
            self.phase = Phase::Leaving { since: Instant::now() };
        }
    }

    /// The start view over where the head stands now, facing its way: a new cube draws it whole.
    pub fn recenter(&mut self, steamvr: &vr::Client) {
        if let Some(c) = &self.capture {
            self.placement = Some(Placement::at(&c.view, &head_pose(steamvr, 0.0)));
            self.renew();
        }
    }

    /// Every face again, on a new cube (after a new placement or size).
    fn renew(&mut self) {
        if self.switching.is_none() {
            let other = 1 - self.active;
            self.cubes[other] = None;
        }
        self.forget_drawn();
        // The active cube no longer holds: the next batch makes a new one.
        if let Some(c) = self.cubes[self.active].as_mut() {
            c.pixels = 0;
        }
    }

    pub fn set_quality(&mut self, quality: Quality) {
        if quality == self.quality {
            return;
        }
        self.quality = quality;
        if let Some(thread) = &self.thread {
            let _ = thread.tx.send(Ask::Width(quality.pixels() + 2 * MARGIN));
            self.renew();
        }
    }

    fn create(&self, overlay: &vr::Overlay) -> Result<[[vr::OverlayHandle; 6]; 2], String> {
        let mut out = [[0; 6]; 2];
        for (c, cube) in out.iter_mut().enumerate() {
            for (f, h) in cube.iter_mut().enumerate() {
                *h = overlay.create(&format!("{}.{c}.{f}", self.key), "3D world").map_err(|code| format!("a 3D world overlay: SteamVR overlay error {code}"))?;
                let _ = overlay.set_flag(*h, overlay_flag::SIDE_BY_SIDE_PARALLEL, true);
                // Under N4N0's menu (10) and the pill.
                let _ = overlay.set_sort_order(*h, 0);
            }
        }
        Ok(out)
    }

    fn hide_all(&self, overlay: &vr::Overlay) {
        if let Some(handles) = &self.handles {
            for h in handles.iter().flatten() {
                let _ = overlay.hide(*h);
            }
        }
    }

    pub fn destroy(&mut self, overlay: &vr::Overlay) {
        if let Some(handles) = self.handles.take() {
            for h in handles.iter().flatten() {
                let _ = overlay.destroy(*h);
            }
        }
        self.thread = None;
    }

    /// One turn of the overlay's loop.
    pub fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client) {
        self.answers(overlay);
        let now = Instant::now();
        match self.phase {
            Phase::Off => return,
            Phase::Leaving { since } => {
                let t = since.elapsed().as_secs_f32() / FADE_OUT.as_secs_f32();
                if t >= 1.0 {
                    self.hide_all(overlay);
                    self.phase = Phase::Off;
                    self.capture = None;
                    // The device, the splats and the textures go with the thread.
                    self.thread = None;
                    #[cfg(windows)]
                    {
                        self.d3d11 = None;
                    }
                    self.width = 0;
                    self.cubes = [None; 2];
                    self.switching = None;
                    self.in_flight = None;
                    return;
                }
                let from = self.alpha;
                self.set_alpha(overlay, from.min((1.0 - t) * self.alpha_scale()));
                return;
            }
            Phase::Loading => {}
            Phase::Shown { since } => {
                let t = (since.elapsed().as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                self.set_alpha(overlay, eased * self.alpha_scale());
            }
        }
        if !self.loaded || self.width == 0 {
            return;
        }
        if self.in_flight.is_some() || self.last_job.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0 / self.quality.rate()) {
            return;
        }
        if !self.vrchat_on_time(steamvr, now) {
            return;
        }
        if self.eyes_read.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.eyes_read = Some(now);
            for (i, eye) in [Eye::Left, Eye::Right].into_iter().enumerate() {
                self.eye_to_head[i] = steamvr.eye_to_head(eye);
                self.tangents[i] = steamvr.projection_raw(eye);
            }
        }
        let head = head_pose(steamvr, self.ahead(steamvr));
        let views = [stereo::compose(&head, &self.eye_to_head[0]), stereo::compose(&head, &self.eye_to_head[1])];
        if let Some(job) = self.plan(overlay, &head, &views, now) {
            self.in_flight = Some(job.id);
            self.last_job = Some(now);
            self.asked_at = Some(now);
            if let Some(thread) = &self.thread {
                let _ = thread.tx.send(Ask::Render(job));
            }
        }
    }

    /// Whether VRChat's last frames leave room for a batch: after a frame SteamVR dropped or reprojected for the GPU,
    /// none for a while (see the module).
    fn vrchat_on_time(&mut self, steamvr: &vr::Client, now: Instant) -> bool {
        if self.held_until.is_some_and(|t| now < t) {
            return false;
        }
        let Ok(compositor) = steamvr.compositor() else { return true };
        let mut timings = [vr::compositor::FrameTiming::default(); 4];
        let n = compositor.frame_timings(&mut timings);
        let frame_ms = steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).map_or(11.1, |hz| 1000.0 / hz);
        match orbiters_reconstruct::governor::verdict(&timings[..n], frame_ms, 0.0, self.answered) {
            orbiters_reconstruct::governor::Verdict::BackOff(newest) => {
                self.answered = newest;
                self.backoff = (self.backoff * 2).clamp(Duration::from_millis(50), Duration::from_secs(2));
                self.backoffs += 1;
                self.held_until = Some(now + self.backoff);
                false
            }
            _ => {
                self.backoff /= 2;
                true
            }
        }
    }

    fn alpha_scale(&self) -> f32 {
        if self.see_through {
            SEE_THROUGH_ALPHA
        } else {
            1.0
        }
    }

    fn set_alpha(&mut self, overlay: &vr::Overlay, alpha: f32) {
        if (alpha - self.alpha).abs() < 1e-3 {
            return;
        }
        self.alpha = alpha;
        if let Some(handles) = &self.handles {
            for h in handles.iter().flatten() {
                let _ = overlay.set_alpha(*h, alpha);
            }
        }
    }

    /// Seconds from now until a batch asked now lights the displays: the batch, then the next vsync and the headset's
    /// own delay.
    fn ahead(&self, steamvr: &vr::Client) -> f32 {
        let frame = steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).map_or(1.0 / 90.0, |hz| 1.0 / hz);
        let since = steamvr.time_since_vsync().unwrap_or(0.0).clamp(0.0, frame);
        let photons = steamvr.float_property(0, vr::prop::SECONDS_FROM_VSYNC_TO_PHOTONS).unwrap_or(0.011).clamp(0.0, 0.05);
        (self.batch_s + (frame - since) + photons).clamp(0.0, 0.1)
    }

    /// The next batch: a new cube's six faces when the eyes left the active one; else, once the eyes moved or the head
    /// turned, the wide views; else now and then a face out of view drawn whole.
    fn plan(&mut self, overlay: &vr::Overlay, head: &vr::Matrix34, views: &[vr::Matrix34; 2], now: Instant) -> Option<Job> {
        let capture = self.capture.clone()?;
        let placement = self.placement?;
        let eyes = [stereo::position(&views[0]), stereo::position(&views[1])];
        let count = match self.phase {
            Phase::Shown { since } if since.elapsed() < REVEAL => capture.within(stereo::reveal_radius(since.elapsed().as_secs_f32(), REVEAL.as_secs_f32(), capture.farthest())),
            Phase::Loading => capture.within(0.0),
            _ => capture.gaussians.len(),
        };
        let pixels = self.width - 2 * MARGIN;
        let id = self.next_id;
        let active_holds = self.cubes[self.active].is_some_and(|c| c.pixels == pixels && eyes.iter().all(|e| c.holds(*e, REACH_M)));
        if self.switching.is_some() {
            // Its six faces are on their way.
            return None;
        }
        if !active_holds {
            // A new cube around the head, its six faces drawn before it shows.
            let next = if self.cubes[self.active].is_none() { self.active } else { 1 - self.active };
            let cube = Cube { center: stereo::scale(stereo::add(eyes[0], eyes[1]), 0.5), half: HALF_M, pixels, margin: MARGIN };
            self.cubes[next] = Some(cube);
            self.switching = Some(next);
            if let Some(handles) = &self.handles {
                for f in 0..6 {
                    let h = handles[next][f];
                    let _ = overlay.set_width_in_meters(h, cube.quad_width());
                    let _ = overlay.set_transform_absolute(h, &cube.transform(f));
                }
            }
            self.next_id += 1;
            let faces = (0..6).map(|f| (f, 1 - self.shown_slot[f])).collect();
            return Some(Job { id, cube, cube_index: next, placement, eyes, work: Work::Faces(faces), count, wait: self.wait });
        }
        let cube = self.cubes[self.active]?;
        let looking = stereo::scale(stereo::axis(head, 2), -1.0);
        let due = match self.last_wide {
            None => true,
            Some((from, look, drawn)) => {
                drawn != count
                    || (0..2).any(|e| stereo::length(stereo::sub(eyes[e], from[e])) > MOVED_M)
                    || stereo::dot(look, looking).clamp(-1.0, 1.0).acos() > TURNED
            }
        };
        if due {
            let per_tan = self.quality.wide_per_tan();
            let cameras = [0, 1].map(|e| PlayCamera::wide(&views[e], self.tangents[e], WIDE_MARGIN, per_tan, WIDE_MAX));
            let faces: Vec<(usize, usize)> = (0..6).filter(|&f| cameras.iter().any(|c| cube.face_in(f, c))).map(|f| (f, 1 - self.shown_slot[f])).collect();
            if faces.is_empty() {
                return None;
            }
            self.last_wide = Some((eyes, looking, count));
            self.next_id += 1;
            return Some(Job { id, cube, cube_index: self.active, placement, eyes, work: Work::Wide { cameras, faces }, count, wait: self.wait });
        }
        // Now and then a face out of view, drawn whole from where the eyes are now.
        if self.last_unseen.is_some_and(|t| now.duration_since(t) < UNSEEN_EVERY) {
            return None;
        }
        let mut seen = [false; 6];
        for (view, tangents) in views.iter().zip(self.tangents) {
            for (s, v) in seen.iter_mut().zip(cube.faces_seen(view, tangents, TURN_MARGIN)) {
                *s |= v;
            }
        }
        let mid = stereo::scale(stereo::add(eyes[0], eyes[1]), 0.5);
        for k in 0..6 {
            let f = (self.unseen_turn + k) % 6;
            // Drawn from too far, or while the capture was appearing.
            let stale = self.drawn[f].from.is_none_or(|from| stereo::length(stereo::sub(mid, from)) > STALE_M) || self.drawn[f].count < count;
            if !seen[f] && stale {
                self.unseen_turn = f + 1;
                self.last_unseen = Some(now);
                self.next_id += 1;
                return Some(Job { id, cube, cube_index: self.active, placement, eyes, work: Work::Faces(vec![(f, 1 - self.shown_slot[f])]), count, wait: self.wait });
            }
        }
        self.last_unseen = Some(now);
        None
    }

    fn answers(&mut self, overlay: &vr::Overlay) {
        let mut answers = Vec::new();
        if let Some(thread) = &self.thread {
            while let Ok(a) = thread.rx.try_recv() {
                answers.push(a);
            }
        }
        for answer in answers {
            match answer {
                #[cfg(windows)]
                Answer::Textures(handles) => {
                    if let Some(side) = self.d3d11.as_mut() {
                        match side.open(&handles) {
                            Ok(()) => {
                                self.width = handles.width;
                                self.wait = 0;
                                self.shown_slot = [0; 6];
                                self.forget_drawn();
                            }
                            Err(e) => self.error = Some(format!("{e:#}")),
                        }
                    }
                }
                Answer::Loaded => self.loaded = true,
                Answer::Rendered { id, cube_index, work, eyes, count, ms, redraw } => {
                    if self.in_flight == Some(id) {
                        self.in_flight = None;
                    }
                    if let Some(asked) = self.asked_at.take() {
                        self.batch_s = 0.8 * self.batch_s + 0.2 * asked.elapsed().as_secs_f32().min(0.1);
                    }
                    if matches!(work, Work::Wide { .. }) {
                        self.batches += 1;
                        self.batch_ms = if self.batch_ms == 0.0 { ms as f32 } else { 0.8 * self.batch_ms + 0.2 * ms as f32 };
                    }
                    self.handed(overlay, cube_index, work.faces(), eyes, count);
                    if redraw {
                        // A picture left splats out: drawn again whole by the next batch.
                        self.last_wide = None;
                    }
                }
                Answer::Skipped { id, cube_index } => {
                    if self.in_flight == Some(id) {
                        self.in_flight = None;
                    }
                    self.abandon(cube_index);
                }
                Answer::Failed(e) => {
                    eprintln!("3D worlds: {e}");
                    self.error = Some(e);
                    self.in_flight = None;
                    self.phase = Phase::Leaving { since: Instant::now() };
                }
            }
        }
    }

    /// A cube whose faces were not drawn is planned again.
    fn abandon(&mut self, cube_index: usize) {
        if self.switching == Some(cube_index) {
            self.switching = None;
            self.cubes[cube_index] = None;
        }
    }

    /// A batch is drawn: its textures go to the faces of `cube_index`; a cube taking over shows, the other hides.
    fn handed(&mut self, overlay: &vr::Overlay, cube_index: usize, faces: &[(usize, usize)], eyes: [V3; 2], count: usize) {
        #[cfg(windows)]
        {
            let (Some(handles), Some(side)) = (self.handles, self.d3d11.as_mut()) else { return };
            if self.cubes[cube_index].is_none_or(|c| c.width() != side.width) {
                // Drawn for a size since replaced.
                self.abandon(cube_index);
                return;
            }
            let mid = stereo::scale(stereo::add(eyes[0], eyes[1]), 0.5);
            for &(f, slot) in faces {
                if let Some(raw) = side.raw(f * 2 + slot) {
                    if let Err(code) = overlay.set_texture_d3d11(handles[cube_index][f], raw) {
                        self.error = Some(format!("SteamVR refused a 3D world picture (overlay error {code})"));
                    }
                }
                self.shown_slot[f] = slot;
                self.drawn[f] = Drawn { from: Some(mid), count };
            }
            self.wait = side.handed_over();
            if self.switching == Some(cube_index) {
                for f in 0..6 {
                    let _ = overlay.set_alpha(handles[cube_index][f], self.alpha);
                    let _ = overlay.show(handles[cube_index][f]);
                }
                if cube_index != self.active {
                    for f in 0..6 {
                        let _ = overlay.hide(handles[self.active][f]);
                    }
                }
                self.active = cube_index;
                self.switching = None;
                if self.phase == Phase::Loading {
                    self.phase = Phase::Shown { since: Instant::now() };
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (overlay, cube_index, faces, eyes, count);
        }
    }
}

/// The headset's pose `seconds` ahead (standing play space), or a standing head at the origin when it is not tracked.
pub fn head_pose(steamvr: &vr::Client, seconds: f32) -> vr::Matrix34 {
    let mut poses = [Pose::default(); MAX_DEVICES];
    steamvr.predicted_poses(Universe::Standing, seconds, &mut poses);
    if poses[0].pose_is_valid {
        poses[0].device_to_absolute
    } else {
        vr::Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] }
    }
}
