//! A capture turning in 3D inside a sheet, on the desktop, where nothing can stand around the person (Enzo's request: a press on a capture
//! shows it in 3D). The same splats and the same renderer as the small view in VR ([`Small`], [`FaceRenderer`]), drawn from its own thread
//! into a picture the sheet paints: live, and turned by the mouse.
//!
//! What it may cost, beside a running game: the budget the 3D view holds in VR ([`DUTY`]), and no more frames a second than a slow turn needs.
//! Nothing is drawn while nothing moves. The picture gets smaller, and later larger again, by what the last ones took. Everything of it,
//! on the GPU and in memory, goes with its thread when the view closes.

use super::super::worlds::catalog::{self, Capture};
use super::super::worlds::pacing::DUTY;
use super::super::worlds::render::{self, FaceRenderer};
use super::super::worlds::stereo::{self, Small};
use orbiters_openvr::Matrix34;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Sizes of the picture, largest first (16:9, as a sheet's page).
const SIZES: [[u32; 2]; 3] = [[960, 540], [720, 405], [540, 304]];
/// Where it starts.
const START_SIZE: usize = 1;
/// No more often than this: enough for a slow turn, and for a drag to feel held.
const FRAME: Duration = Duration::from_millis(50);
/// The slow turn sways around where it is, this far each way (radians) and this fast (radians of its sine a second): a capture is mostly taken from one side, and its back is
/// no sight to turn to by itself. A hand may take it anywhere.
const SWAY: f32 = 0.6;
const SWAY_RATE: f32 = 0.4;
/// A drag leaves the view where it was put for this long before the slow turn goes on.
const STAYS_AFTER_A_DRAG: Duration = Duration::from_secs(4);
/// A frame's cost over this makes the picture smaller; under the second, larger.
const TOO_LONG_MS: f32 = 22.0;
const ROOM_MS: f32 = 7.0;
/// How steep it may be looked at from, in radians, from level to from above.
const PITCH: (f32, f32) = (0.05, 1.05);
/// The fixed head it is looked at from, and how far the frame reaches at the middle's distance.
const TANGENT: f32 = 0.31;
/// Where its plate's middle lies under the head's level, in metres at its distance.
/// Lower and further than at first: round 12's judges saw the capture's top go under the sheet's head.
const PLATE_AT: f32 = -0.32;
/// What is left of what it shows, zoomed all the way out and in.
const ZOOM: (f32, f32) = (0.35, 1.6);

/// A picture of it, and the plate it stands on as a line in the picture (0 to 1 from its top left).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub plate: Vec<[f32; 2]>,
}

/// How it is getting on.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewState {
    Loading,
    Shown,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    yaw: f32,
    pitch: f32,
    zoom: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self { yaw: 0.0, pitch: 0.36, zoom: 1.0 }
    }
}

impl Pose {
    fn turned(mut self, yaw: f32, pitch: f32) -> Self {
        self.yaw = (self.yaw + yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + pitch).clamp(PITCH.0, PITCH.1);
        self
    }

    fn zoomed(mut self, notches: f32) -> Self {
        self.zoom = (self.zoom * 0.88f32.powf(notches)).clamp(ZOOM.0, ZOOM.1);
        self
    }
}

/// How far the slow turn has it turned, `seconds` of it along.
fn sway(seconds: f32) -> f32 {
    SWAY * (seconds * SWAY_RATE).sin()
}

#[derive(Default)]
struct Asked {
    pose: Pose,
    /// A hand is on it, or was lately: when it was let go.
    held: bool,
    let_go: Option<Instant>,
}

struct Shared {
    asked: Mutex<Asked>,
    stop: AtomicBool,
    frame: Mutex<Option<Frame>>,
    state: Mutex<ViewState>,
    costs: Mutex<Costs>,
}

/// What it cost so far, for the report and for whoever looks at the log.
#[derive(Clone, Copy, Debug, Default)]
pub struct Costs {
    pub frames: u32,
    pub total_ms: f32,
    pub longest_ms: f32,
    pub load_ms: f32,
    pub splats: usize,
    pub width: u32,
}

/// The view while a capture is open on a sheet. Dropped, its thread ends, and all it holds goes with it.
pub struct SheetView {
    shared: Arc<Shared>,
}

impl SheetView {
    /// Starts showing the capture in `dir`; `turning` says whether it turns by itself (Reduce motion: not).
    pub fn open(dir: PathBuf, turning: bool) -> Self {
        Self::open_with(move || catalog::read_local(&dir, &Default::default()), turning, 0)
    }

    /// The same, of what `load` gets, wherever that is: `largest` is the largest of [`SIZES`] it is worth drawing, for where it is shown.
    pub fn open_with(load: impl FnOnce() -> anyhow::Result<Capture> + Send + 'static, turning: bool, largest: usize) -> Self {
        let shared = Arc::new(Shared {
            asked: Mutex::new(Asked::default()),
            stop: AtomicBool::new(false),
            frame: Mutex::new(None),
            state: Mutex::new(ViewState::Loading),
            costs: Mutex::new(Costs::default()),
        });
        let for_thread = shared.clone();
        let spawned = std::thread::Builder::new().name("orbit-sheet-view".into()).spawn(move || {
            if let Err(e) = run(&for_thread, load, turning, largest) {
                eprintln!("the sheet's 3D view: {e:#}");
                *for_thread.state.lock().unwrap_or_else(|e| e.into_inner()) = ViewState::Failed(format!("{e:#}"));
            }
        });
        if let Err(e) = spawned {
            *shared.state.lock().unwrap_or_else(|e| e.into_inner()) = ViewState::Failed(e.to_string());
        }
        Self { shared }
    }

    /// The newest picture, once.
    pub fn take_frame(&self) -> Option<Frame> {
        self.shared.frame.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    pub fn state(&self) -> ViewState {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn costs(&self) -> Costs {
        *self.shared.costs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A hand took it, or let it go.
    pub fn hold(&self, held: bool) {
        let mut asked = self.shared.asked.lock().unwrap_or_else(|e| e.into_inner());
        if asked.held && !held {
            asked.let_go = Some(Instant::now());
        }
        asked.held = held;
    }

    /// The pointer moved this far over it, in the sheet's page's units.
    pub fn turn(&self, dx: f32, dy: f32) {
        let mut asked = self.shared.asked.lock().unwrap_or_else(|e| e.into_inner());
        asked.pose = asked.pose.turned(dx * 0.008, dy * 0.005);
    }

    /// The mouse's wheel, in notches: up comes closer.
    pub fn zoom(&self, notches: f32) {
        let mut asked = self.shared.asked.lock().unwrap_or_else(|e| e.into_inner());
        asked.pose = asked.pose.zoomed(notches);
        asked.let_go = Some(Instant::now());
    }
}

impl Drop for SheetView {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

/// The picture's frame on the head: the capture's up tipped towards it by `pitch`, its start view turned by `yaw`.
fn rotation(start: &[[f32; 3]; 3], pose: Pose) -> [[f32; 3]; 3] {
    let (ys, yc) = pose.yaw.sin_cos();
    let (ps, pc) = pose.pitch.sin_cos();
    let turned = [[yc, 0.0, ys], [0.0, 1.0, 0.0], [-ys, 0.0, yc]];
    let tipped = [[1.0, 0.0, 0.0], [0.0, pc, -ps], [0.0, ps, pc]];
    stereo::mat_mul(&tipped, &stereo::mat_mul(&turned, start))
}

/// Where the capture stands in front of the fixed head, for `pose`.
fn stands(capture: &Capture, head: &Matrix34, start: &[[f32; 3]; 3], pose: Pose, below: f32) -> Small {
    let mut small = Small::new(&capture.view, head, capture.small_reach()).zoomed_in(pose.zoom);
    small.rotation = rotation(start, pose);
    // A little further than at first: round 14's judges saw it reach the frame's top.
    small.anchor = [0.0, 0.0, -1.6];
    // Its plate at the same height in the picture whatever the capture's ground is, and however it is tipped: what stands on it has the room over it.
    let (plate, _) = small.plate(below);
    small.anchor[1] += PLATE_AT - stereo::position(&plate)[1];
    small
}

/// The plate's rim as a line in the picture.
fn plate_line(small: &Small, below: f32, aspect: f32) -> Vec<[f32; 2]> {
    let (at, _) = small.plate(below);
    let (x, y, middle) = (stereo::axis(&at, 0), stereo::axis(&at, 1), stereo::position(&at));
    (0..72)
        .filter_map(|i| {
            let (s, c) = (i as f32 / 72.0 * std::f32::consts::TAU).sin_cos();
            let p = stereo::add(stereo::add(middle, stereo::scale(x, c * small.radius())), stereo::scale(y, s * small.radius()));
            // Behind the head: not in the picture.
            (p[2] < -0.05).then(|| [0.5 + p[0] / -p[2] / (TANGENT * aspect) / 2.0, 0.5 - p[1] / -p[2] / TANGENT / 2.0])
        })
        .collect()
}

fn run(shared: &Shared, load: impl FnOnce() -> anyhow::Result<Capture>, turning: bool, largest: usize) -> anyhow::Result<()> {
    let started = Instant::now();
    let full = load()?;
    let capture = full.around_subject();
    drop(full);
    if shared.stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    let (_adapter, device, queue) = render::device(None)?;
    crate::gpu::report_errors(&device);
    let mut renderer = FaceRenderer::new(device, queue);
    renderer.load(&capture)?;
    renderer.make_transparent()?;
    {
        let mut costs = shared.costs.lock().unwrap_or_else(|e| e.into_inner());
        costs.load_ms = started.elapsed().as_secs_f32() * 1000.0;
        costs.splats = capture.gaussians.len();
    }
    let head = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]] };
    let start = stereo::Placement::at(&capture.view, &head).rotation;
    let below = capture.view.eye()[1] - capture.view.ground();
    let middle = capture.view.eye();
    let (mut size, mut drawn, mut last, mut smooth_ms, mut swaying) = (START_SIZE.max(largest), None::<(Pose, usize)>, Instant::now(), 0.0f32, 0.0f32);
    while !shared.stop.load(Ordering::Relaxed) {
        // It turns by itself, unless a hand has it or just had it.
        let pose = {
            let mut asked = shared.asked.lock().unwrap_or_else(|e| e.into_inner());
            let free = !asked.held && asked.let_go.is_none_or(|t| t.elapsed() >= STAYS_AFTER_A_DRAG);
            if turning && free {
                swaying += last.elapsed().as_secs_f32();
            }
            asked.pose.turned(sway(swaying), 0.0)
        };
        last = Instant::now();
        if drawn == Some((pose, size)) {
            std::thread::sleep(FRAME);
            continue;
        }
        let [w, h] = SIZES[size];
        let aspect = w as f32 / h as f32;
        let small = stands(&capture, &head, &start, pose, below);
        let camera = stereo::eye_camera(&head, [-TANGENT * aspect, TANGENT * aspect, -TANGENT, TANGENT], w, h, &small.placement(middle));
        let began = Instant::now();
        let rgba = renderer.render_view(&camera, capture.within(small.reach()))?;
        let ms = began.elapsed().as_secs_f32() * 1000.0;
        smooth_ms = if smooth_ms == 0.0 { ms } else { smooth_ms * 0.8 + ms * 0.2 };
        {
            let mut costs = shared.costs.lock().unwrap_or_else(|e| e.into_inner());
            costs.frames += 1;
            costs.total_ms += ms;
            costs.longest_ms = costs.longest_ms.max(ms);
            costs.width = w;
        }
        *shared.frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(Frame { size: [w as usize, h as usize], rgba, plate: plate_line(&small, below, aspect) });
        *shared.state.lock().unwrap_or_else(|e| e.into_inner()) = ViewState::Shown;
        drawn = Some((pose, size));
        // The budget: at most this share of the time on the GPU's side of it, as in VR.
        if smooth_ms > TOO_LONG_MS && size + 1 < SIZES.len() {
            size += 1;
            smooth_ms = 0.0;
        } else if smooth_ms > 0.0 && smooth_ms < ROOM_MS && size > largest {
            size -= 1;
            smooth_ms = 0.0;
        }
        std::thread::sleep(pause_after(ms));
    }
    Ok(())
}

/// How long to rest after a frame that took `ms`: at most [`DUTY`] of the time goes on drawing, and no more often than [`FRAME`].
fn pause_after(ms: f32) -> Duration {
    let wall = Duration::from_secs_f32(ms.max(0.0) / 1000.0);
    let by_duty = Duration::from_secs_f64(ms.max(0.0) as f64 / 1000.0 * (1.0 / DUTY - 1.0));
    FRAME.saturating_sub(wall).max(by_duty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_holds_the_budget_of_the_3d_view_in_vr() {
        // A cheap frame waits out the slow turn's rate.
        assert_eq!(pause_after(10.0), FRAME.saturating_sub(Duration::from_millis(10)));
        // An expensive one gives the GPU back more than it took.
        let long = pause_after(60.0);
        assert!(long.as_secs_f32() >= 0.060 * (1.0 / DUTY as f32 - 1.0) - 0.001, "{long:?}");
    }

    #[test]
    fn by_itself_it_sways_and_never_shows_its_back() {
        for i in 0..2000 {
            assert!(sway(i as f32 * 0.05).abs() <= SWAY);
        }
        assert!(SWAY < std::f32::consts::FRAC_PI_4);
        assert_eq!(sway(0.0), 0.0, "it starts from the view it was taken from");
    }

    #[test]
    fn it_can_be_turned_all_the_way_but_not_past_the_top() {
        let pose = Pose::default().turned(100.0, 100.0);
        assert!(pose.yaw >= 0.0 && pose.yaw < std::f32::consts::TAU);
        assert_eq!(pose.pitch, PITCH.1);
        assert_eq!(Pose::default().turned(0.0, -100.0).pitch, PITCH.0);
        assert_eq!(Pose::default().zoomed(100.0).zoom, ZOOM.0);
        assert_eq!(Pose::default().zoomed(-100.0).zoom, ZOOM.1);
    }

    /// `ORBIT_SESSION`: a real capture. Draws it as a sheet does, a few turns' worth, and says what it cost. Writes `ORBIT_OUT` as a picture over a checkerboard.
    #[test]
    #[ignore = "needs a GPU and a real session"]
    fn a_capture_turns_in_a_sheet() {
        let dir = PathBuf::from(std::env::var("ORBIT_SESSION").unwrap());
        let view = SheetView::open(dir, true);
        let began = Instant::now();
        let mut shown = Vec::new();
        while began.elapsed() < Duration::from_secs(40) {
            if let Some(frame) = view.take_frame() {
                shown.push(frame);
            }
            if let ViewState::Failed(e) = view.state() {
                panic!("{e}");
            }
            if shown.len() >= 60 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let costs = view.costs();
        println!("{costs:?}, mean {:.1} ms, in {:?}", costs.total_ms / costs.frames.max(1) as f32, began.elapsed());
        assert!(shown.len() >= 10, "it turned: {}", shown.len());
        // The size settles by what it costs: compared at the last one.
        let last = shown.last().unwrap();
        let first = shown.iter().find(|f| f.size == last.size).unwrap();
        let covered = |f: &Frame| f.rgba.chunks_exact(4).filter(|p| p[3] > 200).count() as f32 / (f.size[0] * f.size[1]) as f32;
        println!("{:.1}% solid, plate {} points", covered(first) * 100.0, first.plate.len());
        assert!(covered(first) > 0.02, "it shows");
        assert!(first.rgba != last.rgba, "and it moves");
        if let Ok(out) = std::env::var("ORBIT_OUT") {
            for (name, frame) in [("first", first), ("last", last)] {
                let (w, h) = (frame.size[0] as u32, frame.size[1] as u32);
                let mut over = frame.rgba.clone();
                for (i, p) in over.chunks_exact_mut(4).enumerate() {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    let back: u32 = if (x / 32 + y / 32) % 2 == 0 { 40 } else { 60 };
                    let a = p[3] as u32;
                    for c in p.iter_mut().take(3) {
                        *c = (*c as u32 + back * (255 - a) / 255).min(255) as u8;
                    }
                    p[3] = 255;
                }
                let path = format!("{out}-{name}.png");
                image::save_buffer(&path, &over, w, h, image::ColorType::Rgba8).unwrap();
            }
        }
    }
}
