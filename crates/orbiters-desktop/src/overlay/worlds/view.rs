//! The capture shown around the person: the cube's twelve SteamVR overlays (two cubes of six faces, the second taking
//! over when the head wanders off the first's centre), the render thread that draws their pictures, and what decides,
//! each turn of the overlay's loop, what to draw (stereo.rs explains why the faces stay right as the head turns).
//!
//! Cost: a new cube draws its six faces for both eyes once. After that a batch draws one wide view per eye (its field of
//! view and [`WIDE_MARGIN`] around it, at [`WIDE_DENSITY`] of the faces' density) and resamples it into the faces it
//! covers: two views of the splats a batch, whatever the faces seen. Motion admits work up to [`Quality::rate`],
//! with one batch in flight. The overlay-specific pacer slows after late scene frames overlapping its own work,
//! while keeping progress possible under persistent scene lateness. A wall-cost duty ceiling also bounds expensive
//! batches. Small movement is more responsive during motion; quiet tracking does not continuously redraw. Only
//! while still and without overload, a stale face out of view is drawn again whole for a later turn.

use super::catalog::Capture;
use super::pacing::{Kind, Pacer};
use super::render::FaceRenderer;
use super::stereo::TurnStick;
use super::stereo::{self, Cube, Placement, PlayCamera, V3};
use orbiters_openvr::{self as vr, compositor::Eye, overlay_flag, Pose, Universe, MAX_DEVICES};
use orbiters_i18n::{tr, tr_ctx};
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

    /// Its name as saved in pictures' names and files: English in every language, read back by tools.
    pub fn label(self) -> &'static str {
        match self {
            Quality::Performance => "Light", // i18n: not shown
            Quality::Balanced => "Balanced", // i18n: not shown (see label(): the name kept in files, shown_label() has the shown one)
            Quality::High => "Sharp", // i18n: not shown (see label(): the name kept in files, shown_label() has the shown one)
        }
    }

    /// Its name for a person, in the language now. "Light" here is how little it asks of the computer, not a lamp.
    pub fn shown_label(self) -> &'static str {
        match self {
            Quality::Performance => tr_ctx("quality", "Light"),
            Quality::Balanced => tr_ctx("quality", "Balanced"),
            Quality::High => tr_ctx("quality", "Sharp"),
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
    pub fn kind(&self) -> Kind {
        match self {
            Work::Wide { .. } => Kind::Wide,
            Work::Faces(faces) if faces.len() == 6 => Kind::Cube,
            _ => Kind::Face,
        }
    }

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
    /// Small with its middle moved: only what it reaches from there, with the capture to read that from.
    pub clip: Option<(V3, f32, Arc<Capture>)>,
    /// Splats drawn (the first ones: the reveal).
    pub count: usize,
    /// The shared fence value the GPU waits for first.
    pub wait: u64,
}

/// How long the device and shaders stay with nothing shown, for the next capture to need no getting ready.
const IDLE_KEEPS_GPU: Duration = Duration::from_secs(300);

/// VRChat's setting for how it turns: `VRC_INPUT_COMFORT_TURNING`, on for steps. It is read when a capture shows; if it cannot be read, smoothly, VRChat's default.
#[cfg(windows)]
fn vrchat_turns_in_steps() -> Option<bool> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let (mut value, mut size) = (0u32, 4u32);
    // SAFETY: the buffer and its size are valid for the call, and the type asked for is that size.
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from("Software\\VRChat\\VRChat"), &HSTRING::from("VRC_INPUT_COMFORT_TURNING_h2182102264"), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size)) };
    status.is_ok().then_some(value != 0)
}

#[cfg(not(windows))]
fn vrchat_turns_in_steps() -> Option<bool> {
    None
}

/// Draws a batch on `renderer`: into the faces' pictures, then copied to `targets` (face × 2 + slot).
pub fn draw(renderer: &mut FaceRenderer, job: &Job, targets: &[&eframe::wgpu::Texture]) -> anyhow::Result<bool> {
    renderer.clip(job.clip.as_ref());
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
    /// A capture, and whether nothing shows around it.
    Load(Arc<Capture>, bool),
    Width(u32),
    Render(Job),
}

enum Answer {
    #[cfg(windows)]
    Textures(super::interop::Handles),
    Loaded,
    Rendered {
        id: u64,
        cube_index: usize,
        work: Work,
        eyes: [V3; 2],
        count: usize,
        ms: f64,
        ready: Instant,
        redraw: bool,
    },
    /// Not drawn (asked for textures of another size).
    Skipped {
        id: u64,
        cube_index: usize,
    },
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
                Ask::Load(capture, transparent) => {
                    renderer.load(&capture)?;
                    if transparent {
                        renderer.make_transparent()?;
                    }
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
                        let _ = answers.send(Answer::Rendered {
                            id: job.id,
                            cube_index: job.cube_index,
                            work: job.work,
                            eyes: job.eyes,
                            count: job.count,
                            ms,
                            ready: Instant::now(),
                            redraw,
                        });
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
    /// The capture as asked to show, and whether small: then `capture` is it in order around its subject, on its way
    /// here while it is sorted, and what holds it.
    full: Option<Arc<Capture>>,
    small_on: bool,
    small: Option<stereo::Small>,
    small_coming: Option<mpsc::Receiver<Arc<Capture>>>,
    /// Where a panel has a place for it, and how large that is: the panel's turn to say, each frame.
    small_place: Option<(vr::Matrix34, f32)>,
    /// When the hands last moved it; where it stood for the wide views last drawn.
    small_read: Option<Instant>,
    wide_placement: Option<Placement>,
    quality: Quality,
    /// Chosen on the panel's bar: the FPS booster leaves it.
    person_chose_quality: bool,
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
    asked_at: Option<Instant>,
    pacer: Pacer,
    trace: super::diagnostics::Trace,
    timing_read: Option<Instant>,
    refresh_hz: f32,
    photons: f32,
    /// Current-pose velocities, not derivatives of predicted positions (tracking jitter must not trigger work).
    motion: Option<(Instant, V3, V3)>,
    steady_since: Option<Instant>,
    moving: bool,
    still: bool,
    settling: bool,
    wait: u64,
    alpha: f32,
    /// A capture has shown on this device, and since when nothing has.
    gpu_ready: bool,
    idle_since: Option<Instant>,
    /// VRChat's turning stick, for what shows around the person; and how VRChat turns.
    turning: TurnStick,
    turn_read: Option<Instant>,
    turned_smoothly: bool,
    steps_turn: bool,
    /// It lies across a menu, and how far it has gone for that.
    /// Faces handed to the compositor: `vr-test.log`.
    published: u64,
    pub see_through: bool,
    unseen_turn: usize,
    last_unseen: Option<Instant>,
    eyes_read: Option<Instant>,
    eye_to_head: [vr::Matrix34; 2],
    tangents: [[f32; 4]; 2],
    /// Back-offs so far (the panel's numbers).
    pub backoffs: u32,
    /// What went wrong last, for the panel.
    pub error: Option<String>,
    /// Batches drawn and render wall time per batch (smoothed), for the panel's numbers.
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
            full: None,
            small_on: false,
            small: None,
            small_coming: None,
            small_place: None,
            small_read: None,
            wide_placement: None,
            quality: Quality::default(),
            person_chose_quality: false,
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
            asked_at: None,
            pacer: Pacer::default(),
            trace: super::diagnostics::Trace::default(),
            timing_read: None,
            refresh_hz: 90.0,
            photons: 0.011,
            motion: None,
            steady_since: None,
            moving: false,
            still: true,
            settling: false,
            wait: 0,
            alpha: 0.0,
            gpu_ready: false,
            idle_since: None,
            turning: TurnStick::default(),
            turn_read: None,
            turned_smoothly: false,
            steps_turn: false,
            published: 0,
            see_through: false,
            unseen_turn: 0,
            last_unseen: None,
            eyes_read: None,
            eye_to_head: [vr::Matrix34::IDENTITY; 2],
            tangents: [[-1.2, 1.2, -1.2, 1.2]; 2],
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

    /// It is getting ready with no capture having shown on this device yet: its shaders are being made.
    pub fn first_time(&self) -> bool {
        !self.gpu_ready
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// The FPS booster's step 2 on a PC short of graphics card: Light, for now, never saved. A choice the person makes on the
    /// panel's bar wins from then on.
    pub fn set_light_for_booster(&mut self, light: bool) {
        if self.person_chose_quality {
            return;
        }
        let wanted = if light { Quality::Performance } else { Quality::default() };
        self.change_quality(wanted);
    }

    /// Starts the render thread early (its device and shaders take a moment), while the capture downloads.
    pub fn warm_up(&mut self, steamvr: &vr::Client) {
        if self.thread.is_some() {
            return;
        }
        self.trace.start();
        #[cfg(windows)]
        let found = steamvr.dxgi_adapter_index().and_then(super::interop::luid_of_index);
        // SteamVR runs on Windows only here: the overlay's textures are shared through Direct3D.
        #[cfg(not(windows))]
        let found: Option<u64> = {
            let _ = steamvr;
            None
        };
        let Some(luid) = found else {
            self.error = Some(tr("SteamVR did not say which graphics card the headset uses.").into());
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
        self.idle_since = None;
        self.turning = TurnStick::default();
        self.turn_read = None;
        self.steps_turn = vrchat_turns_in_steps().unwrap_or(false);
        self.error = None;
        self.pacer = Pacer::default();
        self.motion = None;
        self.steady_since = None;
        self.moving = false;
        self.still = true;
        self.settling = false;
        self.timing_read = None;
        self.trace.begin();
        self.warm_up(steamvr);
        if self.thread.is_none() {
            return;
        }
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
        self.full = Some(capture.clone());
        self.small = None;
        self.small_coming = None;
        if self.small_on {
            // Sorted anew around its subject, on the way to the GPU.
            let (tx, rx) = mpsc::channel();
            let full = capture;
            let _ = std::thread::Builder::new().name("3d-worlds-small".into()).spawn(move || {
                let _ = tx.send(Arc::new(full.around_subject()));
            });
            self.small_coming = Some(rx);
            self.capture = None;
            self.loaded = false;
            self.cubes = [None; 2];
            self.switching = None;
            self.forget_drawn();
            self.phase = Phase::Loading;
            self.alpha = 0.0;
            return;
        }
        self.placement = Some(Placement::at(&capture.view, &head_pose(steamvr, 0.0)));
        self.begin(capture, false);
    }

    /// `capture` on its way to the GPU, to show as it is placed.
    fn begin(&mut self, capture: Arc<Capture>, transparent: bool) {
        let Some(thread) = &self.thread else { return };
        let _ = thread.tx.send(Ask::Load(capture.clone(), transparent));
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

    /// Whether captures show small before the person and not around them.
    pub fn small_on(&self) -> bool {
        self.small_on
    }

    /// Whether what shows now can be taken in a hand.
    pub fn small(&self) -> bool {
        self.small.is_some()
    }

    /// Captures show small, or around the person: what shows now too.
    pub fn set_small(&mut self, on: bool, overlay: &vr::Overlay, steamvr: &vr::Client) {
        if self.small_on == on {
            return;
        }
        self.small_on = on;
        if self.active() {
            if let Some(full) = self.full.clone() {
                self.show(overlay, steamvr, full);
            }
        }
    }

    /// The hands, for what shows small: their poses, grips and sticks' up and down.
    pub fn small_input(&mut self, hands: [Option<vr::Matrix34>; 2], grips: [bool; 2], sticks: [f32; 2], pads: [[f32; 2]; 2]) {
        let now = Instant::now();
        let seconds = self.small_read.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
        self.small_read = Some(now);
        let (Some(small), Some(capture)) = (self.small.as_mut(), self.capture.as_ref()) else { return };
        let dock = self.small_place.map(|(at, radius)| stereo::SmallDock::behind(&capture.view, &at, radius));
        small.step(hands, grips, sticks, seconds, dock);
        small.move_middle(hands, pads, seconds, capture.farthest());
        self.placement = Some(small.placement(capture.view.eye()));
    }

    /// Whether VRChat turns in steps here.
    pub fn turns_in_steps(&self) -> bool {
        self.steps_turn
    }

    /// What this submits now, for `vr-test.log`: what a render cannot say of a headset.
    pub fn report(&self, overlay: &vr::Overlay) -> String {
        let (mut visible, mut total) = (0, 0);
        if let Some(handles) = &self.handles {
            for h in handles.iter().flatten() {
                total += 1;
                visible += overlay.is_visible(*h) as u32;
            }
        }
        let small = self.small.as_ref().map(|s| format!("docked {} taken {} held {:?} reach {:.2} radius {:.3} anchor {:?} middle moved {:?}", s.docked, s.taken(), s.held_by(), s.reach(), s.radius(), s.anchor, s.moved_by())); // i18n: not shown (a note in the log)
        let place = self.small_place.map(|(at, radius)| (stereo::position(&at), radius));
        format!(
            "view: phase {} alpha {:.2} faces visible {visible}/{total} faces submitted {} loaded {} in flight {} cube {:?} small [{}] hole's place {place:?} error {:?}", // i18n: not shown (a log's line)
            match self.phase { Phase::Off => "off", Phase::Loading => "loading", Phase::Shown { .. } => "shown", Phase::Leaving { .. } => "leaving" },
            self.alpha,
            self.published,
            self.loaded,
            self.in_flight.is_some(),
            self.cubes[self.active].map(|c| c.center),
            small.unwrap_or_default(),
            self.error,
        )
    }

    /// Where a panel has a place for what shows small, and how large it is, or that it has none.
    pub fn small_place(&mut self, place: Option<(vr::Matrix34, f32)>) {
        self.small_place = place;
    }

    /// What shows small, once it shows.
    pub fn small_now(&self) -> Option<&stereo::Small> {
        self.small.as_ref().filter(|_| matches!(self.phase, Phase::Shown { .. }))
    }

    /// VRChat's turning stick (-1 to 1), for what shows around the person: the capture turns about the head, as the world does in VRChat. Enzo: "so I can turn around and look around in
    /// the same way as I would in vrchat and not feel stuck". What shows small has its own hand.
    pub fn turn_stick(&mut self, stick: f32, steamvr: &vr::Client) -> f32 {
        let now = Instant::now();
        let seconds = self.turn_read.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
        self.turn_read = Some(now);
        if self.small.is_some() || !matches!(self.phase, Phase::Shown { .. }) {
            return 0.0;
        }
        let by = self.turning.step(stick, seconds, self.steps_turn);
        if by == 0.0 {
            // Smoothly, only what is looked at was drawn anew: now all of it, from where it stopped.
            if std::mem::take(&mut self.turned_smoothly) {
                self.renew();
            }
            return 0.0;
        }
        self.turned_smoothly = !self.steps_turn;
        if let Some(placement) = self.placement {
            self.placement = Some(stereo::turned_about(&placement, stereo::position(&head_pose(steamvr, 0.0)), by));
            if self.steps_turn {
                self.renew();
            }
        }
        by
    }

    /// What shows small goes back to its place in the panel.
    pub fn small_back(&mut self) {
        if let Some(small) = self.small.as_mut() {
            small.bring_back();
        }
    }

    fn forget_drawn(&mut self) {
        self.drawn = [Drawn::default(); 6];
        self.last_wide = None;
    }

    /// Fades the capture out, then lets go of it (and of the GPU's memory).
    pub fn hide(&mut self) {
        if self.active() {
            self.trace.flush(Instant::now(), &self.pacer, true);
            self.phase = Phase::Leaving { since: Instant::now() };
        }
    }

    /// The start view over where the head stands now, facing its way: a new cube draws it whole.
    pub fn recenter(&mut self, steamvr: &vr::Client) {
        if let Some(c) = &self.capture {
            if let Some(small) = self.small.as_mut() {
                let reach = small.reach();
                *small = stereo::Small::new(&c.view, &head_pose(steamvr, 0.0), reach);
                self.placement = Some(small.placement(c.view.eye()));
            } else {
                self.placement = Some(Placement::at(&c.view, &head_pose(steamvr, 0.0)));
            }
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
        self.person_chose_quality = true;
        self.change_quality(quality);
    }

    fn change_quality(&mut self, quality: Quality) {
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
                *h =
                    overlay.create(&format!("{}.{c}.{f}", self.key), "3D world") // i18n: not shown (the overlay's name in SteamVR)
                    .map_err(|code| tr!("a 3D world overlay: SteamVR overlay error {code}", code = code))?;
                let _ = overlay.set_flag(*h, overlay_flag::SIDE_BY_SIDE_PARALLEL, true);
                // A capture all around is opaque and this changes nothing; small, what is around it shows the room.
                let _ = overlay.set_flag(*h, overlay_flag::IS_PREMULTIPLIED, true);
                // Under N4N0's menu (10) and the pill, and over what lies under it: a panel's pane, its plate.
                let _ = overlay.set_sort_order(*h, 2);
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
        self.trace.flush(Instant::now(), &self.pacer, true);
        if let Some(handles) = self.handles.take() {
            for h in handles.iter().flatten() {
                let _ = overlay.destroy(*h);
            }
        }
        self.thread = None;
    }

    /// One turn of the overlay's loop.
    pub fn update(&mut self, overlay: &vr::Overlay, steamvr: &vr::Client) {
        if self.active() {
            self.observe(steamvr, Instant::now());
        }
        self.answers(overlay);
        if let Some(sorted) = self.small_coming.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.small_coming = None;
            let mut small = stereo::Small::new(&sorted.view, &head_pose(steamvr, 0.0), sorted.small_reach());
            if let Some((at, radius)) = self.small_place {
                small = small.at_dock(&stereo::SmallDock::behind(&sorted.view, &at, radius));
            }
            self.placement = Some(small.placement(sorted.view.eye()));
            self.small = Some(small);
            self.begin(sorted, true);
        }
        let now = Instant::now();
        if matches!(self.phase, Phase::Off) {
            if self.idle_since.is_some_and(|t| t.elapsed() >= IDLE_KEEPS_GPU) {
                self.idle_since = None;
                // The device, the splats and the textures go with the thread.
                self.thread = None;
                self.gpu_ready = false;
                #[cfg(windows)]
                {
                    self.d3d11 = None;
                }
                self.width = 0;
            }
            return;
        }
        match self.phase {
            Phase::Off => return,
            Phase::Leaving { since } => {
                let t = since.elapsed().as_secs_f32() / FADE_OUT.as_secs_f32();
                if t >= 1.0 {
                    self.hide_all(overlay);
                    self.phase = Phase::Off;
                    self.capture = None;
                    self.full = None;
                    self.small = None;
                    self.small_coming = None;
                    // The next one opens around the person, whatever this one was (Enzo: immersive is the default); Small here asks for itself.
                    self.small_on = false;
                    // The device and its shaders stay for the next capture, which then needs no getting ready (Enzo). They go after a while with nothing shown: see `IDLE_KEEPS_GPU`.
                    self.idle_since = Some(Instant::now());
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
        self.pacer.configure(now, self.quality.rate());
        if self.in_flight.is_some() || !self.pacer.due(now) {
            return;
        }
        if self.eyes_read.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.eyes_read = Some(now);
            for (i, eye) in [Eye::Left, Eye::Right].into_iter().enumerate() {
                self.eye_to_head[i] = steamvr.eye_to_head(eye);
                self.tangents[i] = steamvr.projection_raw(eye);
            }
            self.refresh_hz = steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| hz.is_finite() && *hz > 1.0).unwrap_or(90.0);
            self.photons = steamvr.float_property(0, vr::prop::SECONDS_FROM_VSYNC_TO_PHOTONS).unwrap_or(0.011);
            self.trace.headset = serde_json::json!({
                "model": steamvr.string_property(0, vr::prop::MODEL_NUMBER),
                "driver": steamvr.string_property(0, vr::prop::TRACKING_SYSTEM_NAME),
                "refresh_hz": self.refresh_hz, "vsync_to_photons_s": self.photons,
                "tangents": self.tangents, "quality": self.quality.label()
            });
        }
        let current = tracked_head(steamvr, 0.0);
        if !current.pose_is_valid {
            // Keep the last good world. A transient tracking loss must not construct a cube at a fictitious origin.
            return;
        }
        self.read_motion(now, &current);
        let current_eyes = self.eye_to_head.map(|eye| stereo::position(&stereo::compose(&current.device_to_absolute, &eye)));
        let kind = if self.cubes[self.active].is_none_or(|c| c.width() != self.width || current_eyes.iter().any(|e| !c.holds(*e, REACH_M))) {
            Kind::Cube
        } else {
            Kind::Wide
        };
        let steady = self.moving && self.steady_since.is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_millis(100));
        let mut horizon = self.pacer.horizon(kind, self.refresh_hz, steamvr.time_since_vsync().unwrap_or(0.0), self.photons, steady);
        let mut predicted = tracked_head(steamvr, horizon);
        if !predicted.pose_is_valid {
            return;
        }
        // Prediction can cross the cube's boundary before the unpredicted head does. Select the full-cube
        // horizon before planning mutates transforms/slots; that job takes much longer than a wide refresh.
        let predicted_eyes = self.eye_to_head.map(|eye| stereo::position(&stereo::compose(&predicted.device_to_absolute, &eye)));
        if kind == Kind::Wide && self.cubes[self.active].is_some_and(|c| predicted_eyes.iter().any(|e| !c.holds(*e, REACH_M))) {
            horizon = self.pacer.horizon(Kind::Cube, self.refresh_hz, steamvr.time_since_vsync().unwrap_or(0.0), self.photons, steady);
            predicted = tracked_head(steamvr, horizon);
            if !predicted.pose_is_valid {
                return;
            }
        }
        let head = predicted.device_to_absolute;
        let views = [stereo::compose(&head, &self.eye_to_head[0]), stereo::compose(&head, &self.eye_to_head[1])];
        if let Some(mut job) = self.plan(overlay, &head, &views, now) {
            let horizon = if job.work.kind() == Kind::Face {
                let h = self.pacer.horizon(Kind::Face, self.refresh_hz, steamvr.time_since_vsync().unwrap_or(0.0), self.photons, false);
                let pose = tracked_head(steamvr, h);
                if pose.pose_is_valid {
                    job.eyes = self.eye_to_head.map(|eye| stereo::position(&stereo::compose(&pose.device_to_absolute, &eye)));
                }
                h
            } else {
                horizon
            };
            self.in_flight = Some(job.id);
            self.pacer.admitted(now, job.id);
            self.trace.admitted(now, horizon);
            self.asked_at = Some(now);
            if let Some(thread) = &self.thread {
                let _ = thread.tx.send(Ask::Render(job));
            }
        }
    }

    /// Service slices publish ready textures on the existing overlay thread, without waiting for input/UI painting.
    pub fn publish(&mut self, overlay: &vr::Overlay) {
        self.answers(overlay);
    }

    pub fn input_turn(&mut self, now: Instant) {
        if self.active() {
            self.trace.turn(now);
        }
    }

    pub fn wake_after(&self, now: Instant) -> Option<Duration> {
        if !self.active() || !self.loaded {
            return None;
        }
        if self.in_flight.is_some() {
            Some(Duration::from_millis(1))
        } else {
            self.pacer.wake_after(now)
        }
    }

    fn observe(&mut self, steamvr: &vr::Client, now: Instant) {
        if self.timing_read.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(5)) {
            return;
        }
        self.timing_read = Some(now);
        let mut timings = [vr::compositor::FrameTiming::default(); 16];
        let (n, protect) = match steamvr.compositor() {
            Ok(c) => (c.frame_timings(&mut timings), c.scene_focus_process() != 0 && !c.is_scene_loading()),
            Err(_) => (0, false),
        };
        self.pacer.observe(now, &timings[..n], protect);
        self.backoffs = self.pacer.attributed.min(u32::MAX as u64) as u32;
        self.trace.flush(now, &self.pacer, false);
    }

    fn read_motion(&mut self, now: Instant, pose: &Pose) {
        let speed = stereo::length(pose.velocity);
        let angular = stereo::length(pose.angular_velocity);
        let stable = self.motion.is_some_and(|(at, linear, turn)| {
            let dt = now.saturating_duration_since(at).as_secs_f32().max(0.001);
            stereo::length(stereo::sub(linear, pose.velocity)) / dt < 1.0 && stereo::length(stereo::sub(turn, pose.angular_velocity)) / dt < 3.0
        });
        let still = speed < 0.001 && angular < 0.5f32.to_radians();
        self.settling |= !self.still && still;
        self.still = still;
        self.moving = speed > 0.02 || angular > 10f32.to_radians();
        if stable && self.moving {
            self.steady_since.get_or_insert(now);
        } else {
            self.steady_since = None;
        }
        self.motion = Some((now, pose.velocity, pose.angular_velocity));
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

    /// The next batch: a new cube's six faces when the eyes left the active one; else, once the eyes moved or the head
    /// turned, the wide views; else now and then a face out of view drawn whole.
    fn plan(&mut self, overlay: &vr::Overlay, head: &vr::Matrix34, views: &[vr::Matrix34; 2], now: Instant) -> Option<Job> {
        let capture = self.capture.clone()?;
        let placement = self.placement?;
        let eyes = [stereo::position(&views[0]), stereo::position(&views[1])];
        let count = match self.phase {
            Phase::Shown { since } if since.elapsed() < REVEAL => {
                capture.within(stereo::reveal_radius(since.elapsed().as_secs_f32(), REVEAL.as_secs_f32(), capture.farthest()))
            }
            Phase::Loading => capture.within(0.0),
            _ => capture.gaussians.len(),
        };
        // Small, only what it reaches is drawn.
        let count = self.small.as_ref().map_or(count, |small| count.min(capture.within(small.reach() + small.moved_by())));
        let clip = self.small.as_ref().filter(|small| small.moved_by() > 0.0).map(|small| (small.middle(capture.view.eye()), small.reach(), capture.clone()));
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
            return Some(Job { id, cube, cube_index: next, placement, eyes, work: Work::Faces(faces), count, clip: clip.clone(), wait: self.wait });
        }
        let cube = self.cubes[self.active]?;
        let looking = stereo::scale(stereo::axis(head, 2), -1.0);
        let due = match self.last_wide {
            None => true,
            Some((from, look, drawn)) => {
                drawn != count
                    || self.wide_placement != Some(placement)
                    || (0..2).any(|e| super::pacing::translation_due(stereo::length(stereo::sub(eyes[e], from[e])), self.moving, self.still, self.settling))
                    || stereo::dot(look, looking).clamp(-1.0, 1.0).acos() > TURNED
            }
        };
        if due {
            let per_tan = self.quality.wide_per_tan();
            let cameras = PlayCamera::wide_pair(views, self.tangents, WIDE_MARGIN, per_tan, WIDE_MAX);
            let faces: Vec<(usize, usize)> = (0..6).filter(|&f| cameras.iter().any(|c| cube.face_in(f, c))).map(|f| (f, 1 - self.shown_slot[f])).collect();
            if faces.is_empty() {
                return None;
            }
            self.last_wide = Some((eyes, looking, count));
            self.wide_placement = Some(placement);
            self.settling = false;
            self.next_id += 1;
            return Some(Job { id, cube, cube_index: self.active, placement, eyes, work: Work::Wide { cameras, faces }, count, clip: clip.clone(), wait: self.wait });
        }
        // Now and then a face out of view, drawn whole from where the eyes are now.
        if !self.still || self.pacer.level != 0 || self.last_unseen.is_some_and(|t| now.duration_since(t) < UNSEEN_EVERY) {
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
                return Some(Job {
                    clip: clip.clone(),
                    id,
                    cube,
                    cube_index: self.active,
                    placement,
                    eyes,
                    work: Work::Faces(vec![(f, 1 - self.shown_slot[f])]),
                    count,
                    wait: self.wait,
                });
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
                Answer::Loaded => {
                    self.loaded = true;
                    self.gpu_ready = true;
                }
                Answer::Rendered { id, cube_index, work, eyes, count, ms, ready, redraw } => {
                    if self.in_flight == Some(id) {
                        self.in_flight = None;
                    }
                    if let Some(asked) = self.asked_at.take() {
                        self.pacer.completed(id, work.kind(), ready.saturating_duration_since(asked));
                    }
                    if matches!(work, Work::Wide { .. }) {
                        self.batches += 1;
                        self.batch_ms = if self.batch_ms == 0.0 { ms as f32 } else { 0.8 * self.batch_ms + 0.2 * ms as f32 };
                    }
                    let submit = Instant::now();
                    self.handed(overlay, cube_index, work.faces(), eyes, count);
                    self.trace.rendered(work.kind(), ms, submit.saturating_duration_since(ready), submit.elapsed());
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
                        self.error = Some(tr!("SteamVR refused a 3D world picture (overlay error {code})", code = code));
                    }
                }
                self.shown_slot[f] = slot;
                self.drawn[f] = Drawn { from: Some(mid), count };
            }
            self.wait = side.handed_over();
            if self.switching == Some(cube_index) {
                for f in 0..6 {
                    self.published += 1;
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
    let pose = tracked_head(steamvr, seconds);
    if pose.pose_is_valid {
        pose.device_to_absolute
    } else {
        vr::Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]] }
    }
}

fn tracked_head(steamvr: &vr::Client, seconds: f32) -> Pose {
    let mut poses = [Pose::default(); MAX_DEVICES];
    steamvr.predicted_poses(Universe::Standing, seconds, &mut poses);
    poses[0]
}
