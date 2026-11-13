//! Live translation in VR (the orbit menu's Translation, `Settings::translate`): the text the headset shows read a few
//! times a second and its translation laid over it in the room, as a phone's camera translator does.
//!
//! Two threads of the overlay process and this turn's share:
//! - "translate": reads both eyes' pictures (one persistent pair of SteamVR mirror streams, buffers kept), finds the head
//!   at that moment in the poses this turn records, and runs `orbiters_translate::core::Core` (the text reader, the
//!   placing, the following of lines, the translator's batches on a thread of its own, the painting). Below normal
//!   priority, its pace set by what a look costs; it waits while a menu or SteamVR's dashboard covers the view and while
//!   the head turns fast. Nothing of it exists while the setting is off.
//! - the overlay's turn ([`TranslateHost::update`]): records the head, takes the latest scene, and shows each translation
//!   on one of at most [`orbiters_translate::core::MOST_SHOWN`] pooled overlays, anchored in the room; never waits.
//!
//! Our own overlays are in the eyes' pictures: the translations are masked out of what is read (a translation must not be
//! read again), and reading pauses while a menu is up.

use super::Presenter;
use crate::backend::Ctl;
use crate::state::{Snapshot, TranslateState, TranslateStatus};
use orbiters_core::settings::{TranslateProvider, TranslateSettings};
use orbiters_openvr::{self as vr, overlay_flag, Matrix34};
use orbiters_translate::core::{Activity, Core, Reader, Scene, MOST_SHOWN};
use orbiters_translate::geometry::M34;
use orbiters_translate::paint::Painted;
use orbiters_translate::pipeline::Shown;
use orbiters_translate::provider::{self, Failure, Translator};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Below the menus (orbit's 15, N4N0's 10), above the 3D world's cubes (2).
const SORT_ORDER: u32 = 6;
/// How long a translation takes to appear and to go.
const FADE: Duration = Duration::from_millis(180);
/// How old the compositor's mirror picture is when read, against the head poses recorded (the camera tiles' measure).
const EYE_AGE: Duration = Duration::from_millis(15);
/// A look's area wider than this (pixels) is read again, in part, at its own scale.
const ZOOM_FROM: u32 = 1500;
const ZOOM_SIDE: u32 = 960;
/// The head turning faster than this (degrees a second) smears text and the head's pose at capture: no look then.
const FAST_TURN: f32 = 70.0;

/// What the overlay's turn and the reading thread share.
#[derive(Default)]
struct Shared {
    /// Head poses (standing space) by time, the last two seconds.
    heads: VecDeque<(Instant, M34)>,
    /// SteamVR's eye-to-head transforms and projection tangents (left, right).
    eyes: Option<([M34; 2], [[f32; 4]; 2])>,
    /// A menu or the dashboard covers the view.
    paused: bool,
    /// The translations shown now, to mask them out of what is read.
    shown: Vec<Shown>,
    settings: Option<(TranslateSettings, String)>,
    /// VRChat's frames over the last two seconds.
    load: FrameLoad,
    /// What the thread found: the latest scene, and what it is doing.
    scene: Option<Scene>,
    status: TranslateStatus,
}

fn m34(m: &Matrix34) -> M34 {
    m.m
}
fn matrix(m: &M34) -> Matrix34 {
    Matrix34 { m: *m }
}

struct Slot {
    handle: vr::OverlayHandle,
    track: Option<u64>,
    picture: Option<Arc<Painted>>,
    pose: M34,
    width: f32,
    alpha: f32,
    /// Fading towards: 1 shown, 0 gone.
    to: f32,
    visible: bool,
}

/// The translations' overlays and the reading thread, while Translation is on.
pub struct TranslateHost {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    slots: Vec<Slot>,
    last_turn: Option<Instant>,
    status_sent: Option<(Instant, TranslateStatus)>,
    /// VRChat's frames gathered for [`FrameLoad`], since when, and since the last line in translate.log.
    frames: Vec<orbiters_booster::frames::Frame>,
    frames_since: Option<Instant>,
    frames_logged: Vec<orbiters_booster::frames::Frame>,
    frames_logged_since: Option<Instant>,
}

impl Default for TranslateHost {
    fn default() -> Self {
        Self {
            shared: Arc::default(),
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            slots: Vec::new(),
            last_turn: None,
            status_sent: None,
            frames: Vec::new(),
            frames_since: None,
            frames_logged: Vec::new(),
            frames_logged_since: None,
        }
    }
}

/// What a turn gives the host.
pub struct Cx<'a, 'b> {
    pub overlay: &'a vr::Overlay<'b>,
    pub steamvr: &'a vr::Client,
    pub presenter: &'a mut Presenter,
    pub snap: &'a Arc<Snapshot>,
    pub backend: &'a Ctl,
    /// A menu of ours or SteamVR's dashboard is up.
    pub covered: bool,
    /// VRChat's frames since the last turn, as SteamVR's compositor timed them.
    pub frames: &'a [orbiters_booster::frames::Frame],
}

/// How VRChat's frames went lately: what slows the reading down when the game is short of processor time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameLoad {
    pub frames: usize,
    pub interval_ms: f32,
    pub app_cpu_ms: f32,
    /// Frames whose processor time took most of the frame's budget, and frames shown late or dropped.
    pub cpu_bound: f32,
    pub late: f32,
}

impl FrameLoad {
    pub fn of(frames: &[orbiters_booster::frames::Frame]) -> Self {
        let n = frames.len();
        if n == 0 {
            return Self::default();
        }
        let mut intervals: Vec<f32> = frames.iter().map(|f| f.interval_ms).filter(|i| *i > 0.0).collect();
        intervals.sort_by(|a, b| a.total_cmp(b));
        // The frame's budget: the most common interval (the headset's refresh, or the rate SteamVR holds the game to).
        let budget = intervals.get(intervals.len() / 4).copied().unwrap_or(11.1).max(1.0);
        Self {
            frames: n,
            interval_ms: frames.iter().map(|f| f.interval_ms).sum::<f32>() / n as f32,
            app_cpu_ms: frames.iter().map(|f| f.app_cpu_ms()).sum::<f32>() / n as f32,
            cpu_bound: frames.iter().filter(|f| f.app_cpu_ms() > 0.9 * budget).count() as f32 / n as f32,
            late: frames.iter().filter(|f| f.mis_presented > 0 || f.dropped > 0).count() as f32 / n as f32,
        }
    }

    /// The game is short of processor time: reading slows down.
    pub fn busy(&self) -> bool {
        self.frames >= 30 && (self.cpu_bound > 0.1 || self.late > 0.05)
    }
}

/// The language translations are made into: the one chosen, else the app's.
pub fn target(settings: &orbiters_core::settings::Settings) -> String {
    settings.translate.target.clone().unwrap_or_else(|| orbiters_i18n::language().tag().to_string())
}

impl TranslateHost {
    pub fn update(&mut self, cx: Cx) {
        let settings = &cx.snap.settings.translate;
        // A development build in the VR sandbox may be told to translate by the environment (`ORBITERS_TRANSLATE_ON=1`),
        // its settings untouched (the sandbox restores the person's own settings afterwards).
        // A development build in the VR sandbox may be told to translate by the environment, its settings untouched (the
        // sandbox restores the person's own afterwards): `ORBITERS_TRANSLATE_ON=1`, or `ORBITERS_TRANSLATE_ON_FILE` naming
        // a file whose presence turns it on (to measure the same scene with it on and off).
        let measuring = cfg!(debug_assertions) && std::env::var_os("ORBITERS_TRANSLATE_ON_FILE").is_some();
        let forced = cfg!(debug_assertions)
            && (std::env::var_os("ORBITERS_TRANSLATE_ON").is_some() || std::env::var_os("ORBITERS_TRANSLATE_ON_FILE").is_some_and(|f| std::path::Path::new(&f).exists()));
        let on = (settings.enabled || forced) && orbiters_core::osc::VRCHAT_RUNS_HERE;
        self.watch_frames(cx.frames, on, on || measuring);
        if !on {
            if self.thread.is_some() {
                self.stop_thread();
            }
            self.fade_all(cx.overlay, 0.0);
            self.report(cx.backend, TranslateStatus::default());
            return;
        }
        if self.thread.is_none() {
            self.start(cx.steamvr);
        }
        let now = Instant::now();
        let dt = self.last_turn.map_or(0.0, |t| (now - t).as_secs_f32());
        self.last_turn = Some(now);
        // The head now, for the thread to find it at the moment of a look.
        let mut poses = [vr::Pose::default(); vr::MAX_DEVICES];
        cx.steamvr.poses(vr::Universe::Standing, &mut poses);
        let (scene, status) = {
            let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if poses[0].pose_is_valid {
                s.heads.push_back((now, m34(&poses[0].device_to_absolute)));
                while s.heads.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(2)) {
                    s.heads.pop_front();
                }
            }
            if s.eyes.is_none() {
                let eyes = [vr::compositor::Eye::Left, vr::compositor::Eye::Right];
                s.eyes = Some((eyes.map(|e| m34(&cx.steamvr.eye_to_head(e))), eyes.map(|e| cx.steamvr.projection_raw(e))));
            }
            s.paused = cx.covered;
            s.settings = Some((settings.clone(), target(&cx.snap.settings)));
            (s.scene.take(), s.status.clone())
        };
        if let Some(scene) = scene {
            self.place(cx.overlay, cx.presenter, &scene);
        }
        self.animate(cx.overlay, dt);
        let shown: Vec<Shown> =
            self.slots.iter().filter(|s| s.visible && s.track.is_some()).map(|s| Shown { pose: s.pose, width: s.width, height: s.picture.as_ref().map_or(0.0, |p| p.quad_height), track: s.track }).collect();
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).shown = shown;
        let shown_now = self.slots.iter().filter(|s| s.visible && s.to > 0.5).count() as u32;
        // What the turn shows, for the reading thread's log line too.
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).status.shown = shown_now;
        let status = TranslateStatus { shown: shown_now, ..status };
        self.report(cx.backend, status);
    }

    /// VRChat's frames: their load over two seconds for the reading thread, and a line every ten in translate.log while
    /// translating (or measuring, with translation off).
    fn watch_frames(&mut self, fresh: &[orbiters_booster::frames::Frame], on: bool, log: bool) {
        let now = Instant::now();
        self.frames.extend_from_slice(fresh);
        if self.frames_since.is_none_or(|t| now - t >= Duration::from_secs(2)) {
            let load = FrameLoad::of(&self.frames);
            self.shared.lock().unwrap_or_else(|e| e.into_inner()).load = load;
            self.frames.clear();
            self.frames_since = Some(now);
        }
        if !log {
            self.frames_logged.clear();
            return;
        }
        self.frames_logged.extend_from_slice(fresh);
        if self.frames_logged_since.is_none_or(|t| now - t >= Duration::from_secs(10)) {
            if !self.frames_logged.is_empty() {
                let l = FrameLoad::of(&self.frames_logged);
                let mut ms: Vec<f32> = self.frames_logged.iter().map(|f| f.interval_ms).collect();
                ms.sort_by(|a, b| a.total_cmp(b));
                let p95 = ms[(ms.len() * 95 / 100).min(ms.len() - 1)];
                note(&format!(
                    "VRChat frames (translation {}): {} frames, interval mean {:.2} ms, p95 {p95:.2} ms, app processor {:.2} ms, processor-bound {:.0} %, late or dropped {:.1} %", // i18n: not shown (a log's line)
                    if on { "on" } else { "off" }, // i18n: not shown (a log's line)
                    l.frames,
                    l.interval_ms,
                    l.app_cpu_ms,
                    l.cpu_bound * 100.0,
                    l.late * 100.0
                ));
            }
            self.frames_logged.clear();
            self.frames_logged_since = Some(now);
        }
    }

    fn start(&mut self, steamvr: &vr::Client) {
        self.stop = Arc::new(AtomicBool::new(false));
        let (shared, stop) = (self.shared.clone(), self.stop.clone());
        {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            s.status = TranslateStatus { state: TranslateState::Starting, ..Default::default() };
            s.scene = None;
        }
        #[cfg(windows)]
        let from = (steamvr.compositor().ok().map(|c| c.mirror_source()), steamvr.dxgi_adapter_index());
        #[cfg(not(windows))]
        let from = {
            let _ = steamvr;
        };
        self.thread = std::thread::Builder::new().name("translate".into()).spawn(move || run(&shared, &stop, from)).ok(); // i18n: not shown (a thread's name)
    }

    fn stop_thread(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        stop_downloads();
        // Never waits on the turn: a thread in the middle of a look ends by itself; `finish` joins at teardown.
        if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        } else if let Some(t) = self.thread.take() {
            std::thread::spawn(move || {
                let _ = t.join();
            });
        }
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.scene = None;
        s.shown.clear();
    }

    /// The scene's quads onto the pooled overlays: a line keeps its overlay while it shows.
    fn place(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, scene: &Scene) {
        let wanted: Vec<_> = scene.quads.iter().take(MOST_SHOWN).collect();
        // Lines no longer wanted fade out.
        for slot in &mut self.slots {
            if slot.track.is_some_and(|t| !wanted.iter().any(|q| q.track == t)) {
                slot.to = 0.0;
            }
        }
        for q in wanted {
            let at = self.slots.iter().position(|s| s.track == Some(q.track)).or_else(|| self.slots.iter().position(|s| s.to == 0.0 && s.alpha <= 0.0)).or_else(|| {
                (self.slots.len() < MOST_SHOWN).then(|| {
                    let i = self.slots.len();
                    let handle = overlay.create(&format!("{}.translate.{i}", super::KEY), "Orbiters translation").unwrap_or(0); // i18n: not shown (a name for SteamVR)
                    let _ = overlay.set_flag(handle, overlay_flag::IS_PREMULTIPLIED, true);
                    let _ = overlay.set_sort_order(handle, SORT_ORDER);
                    self.slots.push(Slot { handle, track: None, picture: None, pose: q.pose, width: q.width, alpha: 0.0, to: 0.0, visible: false });
                    i
                })
            });
            let Some(i) = at else { continue };
            let slot = &mut self.slots[i];
            if slot.handle == 0 {
                continue;
            }
            let new_line = slot.track != Some(q.track);
            if new_line || !slot.picture.as_ref().is_some_and(|p| Arc::ptr_eq(p, &q.picture)) {
                let p = &q.picture;
                presenter.show_part(overlay, slot.handle, &p.rgba, p.width, p.height, [0, 0, p.width, p.height]);
                slot.picture = Some(p.clone());
            }
            if new_line {
                slot.alpha = 0.0;
            }
            slot.track = Some(q.track);
            slot.to = 1.0;
            if new_line || slot.pose != q.pose || slot.width != q.width {
                slot.pose = q.pose;
                slot.width = q.width;
                let _ = overlay.set_width_in_meters(slot.handle, q.width);
                let _ = overlay.set_transform_absolute(slot.handle, &matrix(&q.pose));
            }
        }
    }

    fn animate(&mut self, overlay: &vr::Overlay, dt: f32) {
        let step = dt / FADE.as_secs_f32();
        for slot in &mut self.slots {
            if slot.alpha == slot.to && slot.visible == (slot.to > 0.0) {
                continue;
            }
            slot.alpha = if slot.to > slot.alpha { (slot.alpha + step).min(slot.to) } else { (slot.alpha - step).max(slot.to) };
            let _ = overlay.set_alpha(slot.handle, slot.alpha);
            if slot.alpha > 0.0 && !slot.visible {
                let _ = overlay.show(slot.handle);
                slot.visible = true;
            } else if slot.alpha <= 0.0 && slot.visible {
                let _ = overlay.hide(slot.handle);
                slot.visible = false;
                slot.track = None;
            }
        }
    }

    fn fade_all(&mut self, overlay: &vr::Overlay, dt: f32) {
        for slot in &mut self.slots {
            slot.to = 0.0;
        }
        self.animate(overlay, dt.max(FADE.as_secs_f32()));
    }

    fn report(&mut self, backend: &Ctl, status: TranslateStatus) {
        let due = self.status_sent.as_ref().is_none_or(|(at, last)| *last != status && at.elapsed() >= Duration::from_millis(500));
        if due {
            backend.translate_status(status.clone());
            self.status_sent = Some((Instant::now(), status));
        }
    }

    /// At teardown, before SteamVR's client goes: the thread ends (its mirror streams released) and the overlays go.
    pub fn destroy(&mut self, overlay: &vr::Overlay) {
        self.stop.store(true, Ordering::Relaxed);
        stop_downloads();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        for slot in self.slots.drain(..) {
            let _ = overlay.destroy(slot.handle);
        }
    }

    /// What the stall log says it was doing.
    pub fn doing(&self) -> String {
        format!("translate: {} overlays", self.slots.iter().filter(|s| s.visible).count()) // i18n: not shown (a log's line)
    }
}

/// A line in `translate.log` beside the app's other logs: how reading goes (never what was read).
fn note(line: &str) {
    use std::io::Write;
    let path = orbiters_core::paths::logs_dir().join("translate.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, path.with_extension("old.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {line}", orbiters_core::now_utc_iso());
    }
}

fn set_status(shared: &Mutex<Shared>, state: TranslateState) {
    shared.lock().unwrap_or_else(|e| e.into_inner()).status.state = state;
}

/// Lowers this thread's priority: reading text must never take time from VRChat.
fn lower_priority() {
    #[cfg(windows)]
    // SAFETY: the calling thread's own pseudo handle.
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// The translator the settings ask for, with what Settings says of it.
fn translator(settings: &TranslateSettings, backend_lan: &mut LanFound) -> Result<Box<dyn Translator>, TranslateState> {
    match settings.provider {
        TranslateProvider::Orbiters => {
            if !settings.orbiters_agreed {
                return Err(TranslateState::NeedsAgreement);
            }
            let remote = REMOTE.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or(TranslateState::Problem { key: orbiters_i18n::mark("Orbiters - N4N0's service is not reachable.").into(), detail: None })?;
            Ok(Box::new(provider::Remote(move |batch: &provider::Batch| remote(batch))))
        }
        TranslateProvider::Address => {
            if settings.server.trim().is_empty() {
                return Err(TranslateState::NeedsAddress);
            }
            let base = provider::normalise_base(&settings.server);
            let models = provider::list_models(&base).map_err(|f| failure_state(&f))?;
            backend_lan.found = Some((base.clone(), models.clone()));
            let model = if settings.model.is_empty() || !models.contains(&settings.model) { provider::pick_model(&models) } else { Some(settings.model.clone()) };
            let model = model.ok_or(TranslateState::Problem { key: orbiters_i18n::mark("The AI server has no model loaded.").into(), detail: None })?;
            backend_lan.model = Some(model.clone());
            Ok(Box::new(provider::LanModel::new(&base, &model)))
        }
        TranslateProvider::ThisComputer => match provider::find_server() {
            Some((base, models)) => {
                backend_lan.found = Some((base.clone(), models.clone()));
                let model = if settings.model.is_empty() || !models.contains(&settings.model) { provider::pick_model(&models) } else { Some(settings.model.clone()) };
                backend_lan.model = model.clone();
                Ok(Box::new(provider::LanModel::new(&base, &model.unwrap_or_default())))
            }
            None => {
                backend_lan.found = None;
                backend_lan.model = None;
                // No AI app here: the translator built in, once downloaded (`run` downloads it meanwhile).
                if !orbiters_translate::builtin::Builtin::installed() {
                    return Err(TranslateState::NoLocalTranslator);
                }
                orbiters_translate::builtin::Builtin::load(2).map(|b| Box::new(b) as Box<dyn Translator>).map_err(|e| TranslateState::Problem {
                    key: orbiters_i18n::mark("The built-in translator could not start.").into(),
                    detail: Some(e.to_string()),
                })
            }
        },
    }
}

#[derive(Default)]
struct LanFound {
    found: Option<(String, Vec<String>)>,
    /// The model asked.
    model: Option<String>,
}

/// How the overlay reaches Orbiters' translation service (through the desktop service, which holds the account's
/// credential): set once the overlay connected.
pub type RemoteFn = Arc<dyn Fn(&provider::Batch) -> Result<Vec<String>, Failure> + Send + Sync>;
pub static REMOTE: Mutex<Option<RemoteFn>> = Mutex::new(None);

fn failure_state(f: &Failure) -> TranslateState {
    use orbiters_i18n::mark;
    let (key, detail) = match f {
        Failure::Unreachable(d) => (mark("The AI server does not answer."), Some(d.clone())),
        Failure::BadAnswer(d) => (mark("The translator gave an answer that could not be used."), Some(d.clone())),
        Failure::Quota => (mark("Today's Orbiters translations are used up. They come back tomorrow."), None),
        Failure::Busy => (mark("Orbiters is busy: translations wait a moment."), None),
        Failure::SignedOut => (mark("Sign in to Orbiters to translate with Orbiters."), None),
        Failure::NotInstalled => (mark("The built-in translator is not downloaded yet."), None),
    };
    TranslateState::Problem { key: key.into(), detail }
}

/// The reading thread.
#[cfg(windows)]
fn run(shared: &Mutex<Shared>, stop: &AtomicBool, from: (Option<vr::compositor::MirrorSource>, Option<u32>)) {
    use orbiters_openvr::mirror::{EyePicture, EyeReader};
    use orbiters_translate::geometry::{compose, EyeView, Mirror};
    use orbiters_translate::pipeline::Look;
    use orbiters_translate::place::Pair;
    lower_priority();
    // The models first (a download the first time).
    if !orbiters_translate::models::missing().is_empty() {
        set_status(shared, TranslateState::Downloading { done: 0, total: orbiters_translate::models::missing_bytes() });
        let result = orbiters_translate::models::install(stop, &mut |done, total| set_status(shared, TranslateState::Downloading { done, total }));
        if let Err(e) = result {
            if !stop.load(Ordering::Relaxed) {
                set_status(shared, TranslateState::Problem { key: orbiters_i18n::mark("The text reader could not be downloaded.").into(), detail: Some(e.to_string()) });
            }
            return;
        }
    }
    set_status(shared, TranslateState::Starting);
    let engine = match orbiters_translate::pipeline::Engine::load(2) {
        Ok(e) => e,
        Err(e) => {
            set_status(shared, TranslateState::Problem { key: orbiters_i18n::mark("The text reader could not start.").into(), detail: Some(e.to_string()) });
            return;
        }
    };
    let Some(source) = from.0 else {
        set_status(shared, TranslateState::Problem { key: orbiters_i18n::mark("SteamVR does not show the headset's view.").into(), detail: None });
        return;
    };
    let reader = match EyeReader::new(from.1) {
        Ok(r) => r,
        Err(e) => {
            set_status(shared, TranslateState::Problem { key: orbiters_i18n::mark("SteamVR does not show the headset's view.").into(), detail: Some(e) });
            return;
        }
    };
    let (settings, target) = loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(s) = shared.lock().unwrap_or_else(|e| e.into_inner()).settings.clone() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let reader_of = |s: &TranslateSettings, target: &str| Reader { target: target.to_string(), reads: if s.keep_english { vec!["en".to_string()] } else { vec![] } };
    let mut lan = LanFound::default();
    let (mut core, mut translator_state) = match translator(&settings, &mut lan) {
        Ok(t) => (Core::new(Some(engine), t, reader_of(&settings, &target)), None),
        Err(state) => (Core::new(Some(engine), Box::new(Nobody), reader_of(&settings, &target)), Some(state)),
    };
    core.log_text = cfg!(debug_assertions) && std::env::var_os("ORBITERS_TRANSLATE_LOG_TEXT").is_some();
    let mut in_use = settings.clone();
    let mut translator_checked = Instant::now();
    // The built-in translator's download, on a thread of its own while reading goes on: bytes so far, in all, and
    // whether it ended (and how).
    let fetching: Arc<Mutex<Option<(u64, u64, Option<Result<(), String>>)>>> = Arc::default();
    let mut pictures: [EyePicture; 2] = Default::default();
    let started = Instant::now();
    let mut pace = Duration::from_millis(400);
    let mut last_look: Option<Instant> = None;
    let mut costs: VecDeque<f32> = VecDeque::new();
    // What the last ten seconds were spent on, for translate.log.
    let (mut looks, mut paused_turns, mut turning_turns, mut reported) = (0u32, 0u32, 0u32, Instant::now());
    note(&format!("reading started; translator: {}", if translator_state.is_none() { "ready" } else { "none yet" })); // i18n: not shown (a log's line)
    // The eyes' size, known after a first look (Quest Pro through Virtual Desktop: 3168 x 3456); until then, whole.
    let mut whole_size = [u32::MAX / 2; 2];
    let mut game_busy = false;
    let mut last_gaze = None;
    while !stop.load(Ordering::Relaxed) {
        let (heads, eyes, paused, shown, now_settings) = {
            let s = shared.lock().unwrap_or_else(|e| e.into_inner());
            (s.heads.clone(), s.eyes, s.paused, s.shown.clone(), s.settings.clone())
        };
        // Settings changed: another translator, another language.
        if let Some((s, target)) = now_settings {
            let translator_changed = s.provider != in_use.provider || s.server != in_use.server || s.model != in_use.model || s.orbiters_agreed != in_use.orbiters_agreed;
            // No AI app on this computer: the built-in translator is downloaded, once.
            if translator_state == Some(TranslateState::NoLocalTranslator) && s.provider == TranslateProvider::ThisComputer {
                let mut f = fetching.lock().unwrap_or_else(|e| e.into_inner());
                if f.is_none() {
                    *f = Some((0, 0, None));
                    let (fetching, stop) = (fetching.clone(), stop_flag(stop));
                    let _ = std::thread::Builder::new().name("translate download".into()).spawn(move || { // i18n: not shown (a thread's name)
                        lower_priority();
                        let result = orbiters_translate::models::install_files(&orbiters_translate::builtin::FILES, &stop, &mut |done, total| {
                            if let Some(f) = fetching.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                                (f.0, f.1) = (done, total);
                            }
                        });
                        if let Some(f) = fetching.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                            f.2 = Some(result.map_err(|e| e.to_string()));
                        }
                    });
                }
            }
            let downloaded = matches!(*fetching.lock().unwrap_or_else(|e| e.into_inner()), Some((_, _, Some(Ok(())))));
            // A translator not found is looked for again now and then (LM Studio started later), and at once when the
            // built-in one arrived.
            let retry = translator_state.is_some() && (translator_checked.elapsed() > Duration::from_secs(15) || (downloaded && translator_state == Some(TranslateState::NoLocalTranslator)));
            if translator_changed || retry {
                translator_checked = Instant::now();
                match translator(&s, &mut lan) {
                    Ok(t) => {
                        core.set_translator(t);
                        translator_state = None;
                    }
                    Err(state) => {
                        core.set_translator(Box::new(Nobody));
                        translator_state = Some(state);
                    }
                }
            }
            core.set_reader(reader_of(&s, &target));
            in_use = s;
        }
        let turning = turn_speed(&heads) > FAST_TURN;
        if reported.elapsed() >= Duration::from_secs(10) {
            let s = shared.lock().unwrap_or_else(|e| e.into_inner()).status.clone();
            let mean = if costs.is_empty() { 0.0 } else { costs.iter().sum::<f32>() / costs.len() as f32 };
            note(&format!("{looks} looks in {:.0} s (mean {mean:.0} ms, pace {} ms), paused {paused_turns}, turning {turning_turns}; {} lines, {} shown; {:?}", reported.elapsed().as_secs_f32(), pace.as_millis(), s.lines, s.shown, s.state)); // i18n: not shown (a log's line)
            (looks, paused_turns, turning_turns, reported) = (0, 0, 0, Instant::now());
        }
        paused_turns += paused as u32;
        turning_turns += (turning && !paused) as u32;
        // VRChat short of processor time: looks three times rarer, at most every four seconds.
        let load = shared.lock().unwrap_or_else(|e| e.into_inner()).load;
        let pace_now = if load.busy() { (pace * 3).clamp(Duration::from_millis(1500), Duration::from_secs(4)) } else { pace };
        if load.busy() != game_busy {
            game_busy = load.busy();
            note(&format!("VRChat {}: looks every {} ms", if game_busy { "short of processor time" } else { "at ease again" }, pace_now.as_millis())); // i18n: not shown (a log's line)
        }
        if paused || turning || last_look.is_some_and(|t| t.elapsed() < pace_now) {
            if paused {
                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                if !matches!(s.status.state, TranslateState::Problem { .. }) {
                    s.status.state = TranslateState::Paused;
                }
            }
            // A translation that arrives meanwhile shows now.
            if let Some(gaze) = last_gaze {
                if let Some(scene) = core.poll(started.elapsed().as_secs_f64(), gaze) {
                    shared.lock().unwrap_or_else(|e| e.into_inner()).scene = Some(scene);
                }
                for line in core.log.drain(..) {
                    note(&line);
                }
            }
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }
        let Some((eye_to_head, raw)) = eyes else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        last_look = Some(Instant::now());
        looks += 1;
        // The middle of each eye, where things are looked at: 70 % of it each way, in the left eye that is read; the
        // right eye's part is wider, as what is near shows further to the left there.
        let began = Instant::now();
        let read = reader.read_pair_into(source, [Some(pct(whole_size, [15, 15, 70, 70])), Some(pct(whole_size, [3, 15, 82, 70]))], &mut pictures);
        if let Err(e) = read {
            set_status(shared, TranslateState::Problem { key: orbiters_i18n::mark("SteamVR does not show the headset's view.").into(), detail: Some(e) });
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let Some(head) = head_at(&heads, began.checked_sub(EYE_AGE).unwrap_or(began)) else { continue };
        let whole = [pictures[0].whole[0] as f32, pictures[0].whole[1] as f32];
        let views = [0, 1].map(|k| EyeView { eye: compose(&head, &eye_to_head[k]), mirror: Mirror::whole_eye(raw[k]), size: whole });
        whole_size = pictures[0].whole;
        let area = pct(whole_size, [15, 15, 70, 70]);
        let pair = Pair {
            pictures: [0, 1].map(|k| orbiters_translate::ocr::Rgba { width: pictures[k].width, height: pictures[k].height, pixels: &pictures[k].rgba }),
            from: [pictures[0].from, pictures[1].from],
            views,
        };
        let gaze = (orbiters_translate::geometry::origin(&head), orbiters_translate::geometry::scale(orbiters_translate::geometry::axis(&head, 2), -1.0));
        // A large eye's area is scaled down for the detector (a Quest Pro's middle 2218 px to 960): every other look also
        // reads 960 px about where the head looks at their own scale, for small text there (tests/bench.rs,
        // smallest_readable_letters).
        let zoom = (area[2] > ZOOM_FROM && looks % 2 == 0)
            .then(|| views[0].project(orbiters_translate::geometry::add(gaze.0, orbiters_translate::geometry::scale(gaze.1, 2.0))))
            .flatten()
            .map(|c| {
                let side = ZOOM_SIDE.min(area[2]).min(area[3]);
                let x = (c[0] as i64 - side as i64 / 2).clamp(area[0] as i64, (area[0] + area[2] - side) as i64) as u32;
                let y = (c[1] as i64 - side as i64 / 2).clamp(area[1] as i64, (area[1] + area[3] - side) as i64) as u32;
                [x, y, side, side]
            });
        let look = Look { pair, area, shown: &shown, zoom };
        last_gaze = Some(gaze);
        let step = core.step(started.elapsed().as_secs_f64(), &look, gaze);
        for line in core.log.drain(..) {
            note(&line);
        }
        let cost = began.elapsed();
        costs.push_back(cost.as_secs_f32() * 1000.0);
        if costs.len() > 20 {
            costs.pop_front();
        }
        // At most a third of one core's time on average, and not more often than three times a second.
        pace = (cost * 2).clamp(Duration::from_millis(330), Duration::from_secs(3));
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        match step {
            Ok(scene) => {
                let fetch = fetching.lock().unwrap_or_else(|e| e.into_inner()).clone();
                s.status.state = match (&translator_state, &scene.activity) {
                    (Some(TranslateState::NoLocalTranslator), _) if matches!(fetch, Some((_, _, None))) => {
                        let (done, total, _) = fetch.unwrap_or_default();
                        TranslateState::DownloadingTranslator { done, total }
                    }
                    (Some(TranslateState::NoLocalTranslator), _) if matches!(fetch, Some((_, _, Some(Err(_))))) => TranslateState::Problem {
                        key: orbiters_i18n::mark("The built-in translator could not be downloaded.").into(),
                        detail: fetch.and_then(|f| f.2).and_then(|r| r.err()),
                    },
                    (Some(state), _) => state.clone(),
                    (None, Activity::Failed(f)) => failure_state(f),
                    (None, Activity::Translating(n)) => TranslateState::Translating(*n as u32),
                    (None, Activity::Waking) => TranslateState::Waking,
                    (None, Activity::Looking) => TranslateState::Looking,
                };
                s.status.lines = scene.lines as u32;
                s.status.look_ms = Some((costs.iter().sum::<f32>() / costs.len() as f32).round() as u32);
                s.status.server = lan.found.as_ref().map(|f| f.0.clone());
                s.status.models = lan.found.as_ref().map(|f| f.1.clone()).unwrap_or_default();
                s.status.waited_s = scene.waiting_s.map(|w| w as u32);
                s.status.small_model = lan.model.clone().filter(|m| provider::too_small(m));
                s.scene = Some(scene);
            }
            Err(e) => s.status.state = TranslateState::Problem { key: orbiters_i18n::mark("The text reader could not start.").into(), detail: Some(e.to_string()) },
        }
    }
}

#[cfg(not(windows))]
fn run(shared: &Mutex<Shared>, _stop: &AtomicBool, _from: ()) {
    lower_priority();
    set_status(shared, TranslateState::Problem { key: orbiters_i18n::mark("Translation needs SteamVR on this computer.").into(), detail: None });
}

/// A flag that follows `stop` for a thread that outlives this borrow: set when `stop` is, checked by the download.
fn stop_flag(stop: &AtomicBool) -> Arc<AtomicBool> {
    // The reading thread's own flag is borrowed; the download keeps its own, set once the reading thread ends.
    let flag = Arc::new(AtomicBool::new(stop.load(Ordering::Relaxed)));
    STOPS.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&flag));
    flag
}

/// Downloads to stop when translation stops.
static STOPS: Mutex<Vec<std::sync::Weak<AtomicBool>>> = Mutex::new(Vec::new());

fn stop_downloads() {
    for flag in STOPS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        if let Some(f) = flag.upgrade() {
            f.store(true, Ordering::Relaxed);
        }
    }
}

/// No translator yet: every batch waits.
struct Nobody;
impl Translator for Nobody {
    fn translate(&mut self, _batch: &provider::Batch) -> Result<Vec<String>, Failure> {
        std::thread::sleep(Duration::from_millis(200));
        Err(Failure::Unreachable(String::new()))
    }
}

/// A part of an eye of `size`, in percents of it (x, y, width, height).
fn pct(size: [u32; 2], p: [u32; 4]) -> [u32; 4] {
    if size[0] >= u32::MAX / 4 {
        return [0, 0, size[0], size[1]];
    }
    [size[0] * p[0] / 100, size[1] * p[1] / 100, size[0] * p[2] / 100, size[1] * p[3] / 100]
}

/// The head at `at`: the recorded pose nearest in time.
fn head_at(heads: &VecDeque<(Instant, M34)>, at: Instant) -> Option<M34> {
    heads.iter().min_by_key(|(t, _)| if *t > at { *t - at } else { at - *t }).map(|(_, m)| *m)
}

/// How fast the head turned over the last tenth of a second, degrees a second.
fn turn_speed(heads: &VecDeque<(Instant, M34)>) -> f32 {
    let Some((last_t, last)) = heads.back() else { return 0.0 };
    let Some((t, before)) = heads.iter().rev().find(|(t, _)| last_t.duration_since(*t) >= Duration::from_millis(100)) else { return 0.0 };
    let (a, b) = (orbiters_translate::geometry::axis(last, 2), orbiters_translate::geometry::axis(before, 2));
    let angle = orbiters_translate::geometry::dot(a, b).clamp(-1.0, 1.0).acos().to_degrees();
    angle / last_t.duration_since(*t).as_secs_f32().max(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_translate::geometry::{rotation, IDENTITY};

    #[test]
    fn the_head_at_a_look_is_the_nearest_recorded() {
        let t0 = Instant::now();
        let mut heads = VecDeque::new();
        for k in 0..10 {
            let mut m = IDENTITY;
            m[0][3] = k as f32;
            heads.push_back((t0 + Duration::from_millis(11 * k), m));
        }
        let head = head_at(&heads, t0 + Duration::from_millis(34)).unwrap();
        assert_eq!(head[0][3], 3.0);
    }

    #[test]
    fn a_fast_turn_is_seen() {
        let t0 = Instant::now();
        let mut heads = VecDeque::new();
        for k in 0..20u64 {
            // 90 degrees a second about Y.
            heads.push_back((t0 + Duration::from_millis(10 * k), rotation([0.0, 1.0, 0.0], (0.9 * k as f32).to_radians(), [0.0; 3])));
        }
        let speed = turn_speed(&heads);
        assert!((speed - 90.0).abs() < 5.0, "{speed}");
        let still: VecDeque<_> = (0..20u64).map(|k| (t0 + Duration::from_millis(10 * k), IDENTITY)).collect();
        assert!(turn_speed(&still) < 1.0);
    }

    #[test]
    fn the_target_follows_the_app_language_unless_chosen() {
        let mut s = orbiters_core::settings::Settings::default();
        s.translate.target = Some("ja".into());
        assert_eq!(target(&s), "ja");
    }
}
