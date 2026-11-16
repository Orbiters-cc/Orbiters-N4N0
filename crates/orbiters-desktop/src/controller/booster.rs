//! The FPS booster in the tray service: it owns other programs' priorities and what Orbiters itself holds back, and it puts
//! back whatever an earlier run left. It looks at SteamVR's frames through the overlay and never opens a connection of its
//! own: its connections disturbed the overlay's input ("asks SteamVR nothing itself", `overlay/mod.rs`).
//!
//! Every second it compares what the overlay wants with what is so. The overlay gone means nothing is wanted: priorities come
//! back then. A service that itself is killed leaves programs in efficiency mode until its next start or until they exit, and that gap
//! is real and is said in `docs/fps-booster.md`.

use super::Shared;
use crate::booster::{Command, OverlayReport};
use orbiters_booster::engine::{Request, Wants};
use orbiters_booster::guided::{Item, Known, ToPutBack};
use orbiters_booster::journal::{Store, Target};
use orbiters_booster::programs::{self, RealOs, Seen};
use orbiters_booster::{system, vrchat_read, Level};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How old the overlay's last report may be for it to count as there.
const REPORT_GONE_AFTER: Duration = Duration::from_secs(5);
/// How often other programs are looked at while VRChat runs here with SteamVR.
const SCAN_EVERY: Duration = Duration::from_secs(5);
/// How often a SteamVR value still owed is tried again through its file while SteamVR stays closed.
const OFFLINE_EVERY: Duration = Duration::from_secs(60);
/// How often step 1 looks again: for programs that started since, for one that started playing sound (put back), and for a
/// choice someone made by hand (kept).
const RELOWER_EVERY: Duration = Duration::from_secs(5);

/// What the service keeps of the booster.
#[derive(Default)]
pub(crate) struct Hub {
    /// 3D pictures wait for VRChat: held beside `bridge_holds_reconstructions`, never written to a setting, so nothing is left
    /// to restore.
    holds_reconstructions: AtomicBool,
    /// Preparing shares waits while VRChat has the scene.
    holds_preparing: AtomicBool,
    inner: Mutex<Inner>,
    generation: AtomicU64,
    worker: Mutex<Option<std::thread::Thread>>,
}

#[derive(Default)]
struct Inner {
    report: OverlayReport,
    report_at: Option<Instant>,
    next_request: u64,
    pending: Option<(u64, Request)>,
    to_put_back: Vec<ToPutBack>,
    test_load: Option<TestLoad>,
    test_report: Option<std::path::PathBuf>,
}

impl Hub {
    pub(crate) fn cancel_work(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(worker) = self.worker.lock().unwrap().as_ref() { worker.unpark(); }
    }

    pub(crate) fn holds_reconstructions(&self) -> bool {
        self.holds_reconstructions.load(Ordering::Relaxed)
    }

    pub(crate) fn holds_preparing(&self) -> bool {
        self.holds_preparing.load(Ordering::Relaxed)
    }
}

/// The journal, with `RunOnce` running this program at the next sign-in.
pub(crate) fn store() -> Option<Store> {
    let exe = std::env::current_exe().ok()?;
    Some(system::store_for_user(&exe))
}

/// `orbiters-desktop --booster-restore`, from `RunOnce` at sign-in after something died mid-change.
pub fn restore_at_sign_in() -> anyhow::Result<()> {
    if let Some(store) = store() {
        system::run_restorer(&store);
    }
    Ok(())
}

/// Off is authoritative even while the overlay still publishes an older enabled-level report.
fn effective_wants(snap: &crate::state::Snapshot) -> Wants {
    let report = &snap.booster.overlay.report;
    let stopping = snap.booster.request.is_some_and(|(_, r)| r == orbiters_booster::engine::Request::StopTest);
    let explicit_off_test = !stopping && report.level == Level::Off && report.testing.is_some_and(|t| t.manual);
    if snap.booster.watching && !stopping && (snap.settings.booster.level != Level::Off || explicit_off_test) { report.wants } else { Wants::default() }
}

impl super::Controller {
    /// The app is quitting: other programs come back now, as this process will not be there a second from now.
    pub fn booster_quit(self: &Shared) {
        if let Some(store) = store() {
            let _ = programs::restore_all(&store, &mut RealOs);
        }
    }

    /// What the overlay said.
    pub fn booster_report(self: &Shared, report: OverlayReport) {
        let mut inner = self.booster.inner.lock().unwrap();
        // Answered: the request leaves the snapshot, so a new overlay does not do it again.
        if inner.pending.is_some_and(|(n, _)| report.answered >= n) {
            inner.pending = None;
        }
        inner.report = report;
        inner.report_at = Some(Instant::now());
        drop(inner);
        self.booster_publish();
    }

    /// The overlay's link closed: nothing it wanted holds any more. The thread below acts on this within a second.
    pub fn booster_overlay_gone(self: &Shared) {
        let mut inner = self.booster.inner.lock().unwrap();
        inner.report = OverlayReport::default();
        inner.report_at = None;
        // What it was asked and did not answer goes with it: a simulated crash asked of the next one too would never end.
        inner.pending = None;
        drop(inner);
        self.booster_publish();
        self.booster.cancel_work();
    }

    /// What a surface asked.
    pub fn booster_command(self: &Shared, command: Command) {
        let cancelling = matches!(command, Command::Overlay(Request::StopTest) | Command::PutBack);
        match command {
            Command::Overlay(request) => {
                let mut inner = self.booster.inner.lock().unwrap();
                inner.next_request += 1;
                inner.pending = Some((inner.next_request, request));
            }
            Command::PutBack => {
                self.patch_settings(&serde_json::json!({ "booster": { "level": Level::Off } }));
                self.booster_command(Command::Overlay(orbiters_booster::engine::Request::StopTest));
            },
            Command::OpenLogs => open_folder(&orbiters_core::paths::logs_dir()),
            Command::GuidedDone(item) => {
                let was = current_of(item, &self.view());
                let mut inner = self.booster.inner.lock().unwrap();
                if !inner.to_put_back.iter().any(|t| t.item == item) {
                    inner.to_put_back.push(ToPutBack { item, was });
                    keep_to_put_back(&inner.to_put_back);
                }
                inner.next_request += 1;
                inner.pending = Some((inner.next_request, Request::GuidedDone));
            }
            Command::PutBackDone => {
                let mut inner = self.booster.inner.lock().unwrap();
                inner.to_put_back.clear();
                keep_to_put_back(&inner.to_put_back);
            }
            Command::TestLoad(on) => {
                let mut inner = self.booster.inner.lock().unwrap();
                inner.test_load = None;
                if on && dev_build() {
                    inner.test_load = TestLoad::start();
                }
            }
            Command::WriteTestReport => {
                let path = write_test_report(&self.view());
                self.booster.inner.lock().unwrap().test_report = path;
            }
        }
        self.booster_publish();
        if cancelling { self.booster.cancel_work(); }
    }

    fn booster_publish(self: &Shared) {
        let inner = self.booster.inner.lock().unwrap();
        let watching = inner.report_at.is_some_and(|at| at.elapsed() < REPORT_GONE_AFTER);
        let (overlay, request, to_put_back, test_load, test_report) = (
            if watching { inner.report.clone() } else { OverlayReport::default() },
            inner.pending,
            inner.to_put_back.clone(),
            inner.test_load.is_some(),
            inner.test_report.clone(),
        );
        drop(inner);
        self.update_if(|s| {
            let was = s.booster.clone();
            let b = &mut s.booster;
            b.overlay = overlay;
            b.watching = watching;
            b.request = request;
            b.to_put_back = to_put_back;
            b.test_load = test_load;
            b.test_report = test_report;
            b.test_tools = dev_build();
            *b != was
        });
    }

    /// Starts the thread, once what an earlier run left is back: nothing new is applied before that.
    pub(crate) fn start_booster(self: &Shared) {
        if !orbiters_core::osc::VRCHAT_RUNS_HERE {
            return;
        }
        let ctl = self.clone();
        let store = store();
        if let Some(store) = &store {
            let unreadable = system::recover_for_user(store).is_err();
            // RunOnce may have been used up at sign-in: armed again while anything is owed.
            let _ = store.rearm();
            self.update_if(|s| {
                let changed = s.booster.journal_unreadable != unreadable;
                s.booster.journal_unreadable = unreadable;
                changed
            });
        }
        let Some(store) = store else { return };
        self.booster.inner.lock().unwrap().to_put_back = read_to_put_back();
        let _ = std::thread::Builder::new().name("fps-booster".into()).spawn(move || ctl.booster_thread(store));
    }

    fn booster_thread(self: Shared, store: Store) {
        *self.booster.worker.lock().unwrap() = Some(std::thread::current());
        let mut os = RealOs;
        let mut last_seen: Option<(Instant, Vec<Seen>)> = None;
        let mut last_lower: Option<Instant> = None;
        let mut lowered_any = true; // The start may have found some: looked at once.
        let mut last_limits: Option<Instant> = None;
        let mut stopping_prep = false;
        let (mut steamvr_was_closed, mut last_offline) = (false, None);
        let mut holding = false;
        let cores = programs::core_count();
        let mut slower = programs::Slower::default();
        let mut level = Level::Off;
        loop {
            std::thread::park_timeout(Duration::from_secs(1));
            if self.quitting.load(Ordering::Relaxed) {
                let _ = programs::restore_all(&store, &mut os);
                return;
            }
            self.booster_publish();
            let snap = self.view();
            let generation = self.booster.generation.load(Ordering::SeqCst);
            let mut wants = effective_wants(&snap);
            let here = snap.vrchat.runs_here();
            if snap.settings.booster.level != level {
                // A program taken out of efficiency mode by hand is left so until the step changes.
                level = snap.settings.booster.level;
                slower.forget();
            }

            // Who is running, without opening any of them, only while it can matter.
            let due = here && snap.steamvr.running && last_seen.as_ref().is_none_or(|(at, _)| at.elapsed() >= SCAN_EVERY);
            if due {
                let seen = programs::scan();
                if let Some((at, before)) = &last_seen {
                    // What plays sound can't be slowed, so it isn't counted as competition a test could act on. Not knowing
                    // what plays counts none.
                    let left_out = match programs::audio_pids() {
                        Some(audible) => programs::audible_names(&seen, &audible),
                        None => programs::ALLOWLIST.iter().map(|n| n.to_lowercase()).collect(),
                    };
                    let used = programs::cpu_use(before, &seen, at.elapsed().as_secs_f32(), cores, &left_out);
                    self.update_if(|s| {
                        let b = &mut s.booster;
                        let changed = b.cpu_total != used.total || b.listed_cores != used.listed_cores || b.top != used.top;
                        (b.cpu_total, b.listed_cores, b.top) = (used.total, used.listed_cores, used.top.clone());
                        changed
                    });
                }
                last_seen = Some((Instant::now(), seen));
            } else if !here {
                last_seen = None;
            }

            // Efficiency mode: set as the overlay wants, looked at again for what started since; all back otherwise.
            if wants.lower_programs && here {
                if last_lower.is_none_or(|at| at.elapsed() >= RELOWER_EVERY) {
                    // It lists the system afresh itself, and opens nothing it did not find there.
                    let allowed = || self.booster.generation.load(Ordering::SeqCst) == generation
                        && !self.quitting.load(Ordering::Relaxed) && effective_wants(&self.view()).lower_programs;
                    let _ = slower.slow_while(&store, &mut os, &allowed);
                    last_lower = Some(Instant::now());
                    lowered_any = true;
                    self.booster_note_lowered(&store);
                }
            } else {
                last_lower = None;
                if lowered_any {
                    lowered_any = programs::restore_all(&store, &mut os).map_or(true, |r| !r.waiting.is_empty());
                    self.booster_note_lowered(&store);
                }
            }

            if self.booster.generation.load(Ordering::SeqCst) != generation || !effective_wants(&self.view()).lower_programs {
                if lowered_any {
                    lowered_any = programs::restore_all(&store, &mut os).map_or(true, |r| !r.waiting.is_empty());
                    self.booster_note_lowered(&store);
                }
                last_lower = None;
                wants = effective_wants(&self.view());
            }

            // Preparing shares: stopped once, started again when no longer held.
            let stop_prep = wants.stop_preparing && here;
            self.booster.holds_preparing.store(stop_prep, Ordering::Relaxed);
            if stop_prep && !stopping_prep {
                crate::upload_prep::pause();
            }
            if !stop_prep && stopping_prep {
                self.next_reconstruction();
            }
            stopping_prep = stop_prep;

            // 3D pictures wait, through the same pause a running job already has, and start again by themselves.
            let snap = self.view();
            let hold = snap.settings.booster.level >= Level::Picture && snap.booster.watching && here && !wants.release_hold;
            self.booster.holds_reconstructions.store(hold, Ordering::Relaxed);
            if hold && !holding {
                self.follow_while_playing();
            }
            if !hold && holding {
                self.next_reconstruction();
            }
            holding = hold;

            // A SteamVR value still owed, once SteamVR is gone: there is no overlay to restore it now.
            // Tried when SteamVR has just closed, then once a minute: a value that can't be put back must not have the journal
            // and the registry rewritten every second.
            let closed_now = !snap.steamvr.running;
            let due = closed_now && (closed_now != steamvr_was_closed || last_offline.is_none_or(|at: Instant| at.elapsed() >= OFFLINE_EVERY));
            steamvr_was_closed = closed_now;
            if due && store.entries().is_ok_and(|e| system::owed_to_steamvr(&e)) {
                last_offline = Some(Instant::now());
                let _ = system::recover_for_user(&store);
            }

            // What the limits on avatars are set to, read only, for the list: only while someone could be looking at it.
            let wanted_limits = snap.settings.booster.level == Level::Avatars || !snap.booster.to_put_back.is_empty();
            if wanted_limits && last_limits.is_none_or(|at| at.elapsed() >= Duration::from_secs(10)) {
                last_limits = Some(Instant::now());
                let account = vrchat_read::account_of(snap.vrchat.world.local_player.as_deref(), &snap.vrchat.world.players);
                let limits = vrchat_read::read(account.as_deref());
                // VRChat may only write these as it exits: with it closed, what reads as it was is back.
                if !snap.vrchat.running {
                    let mut inner = self.booster.inner.lock().unwrap();
                    let before = inner.to_put_back.len();
                    inner.to_put_back.retain(|t| !(t.was.is_some() && t.was == current_from(t.item, limits.max_shown, limits.hide_beyond)));
                    if inner.to_put_back.len() != before {
                        keep_to_put_back(&inner.to_put_back);
                    }
                }
                self.update_if(|s| {
                    let changed = s.booster.max_shown != limits.max_shown || s.booster.hide_beyond != limits.hide_beyond;
                    s.booster.max_shown = limits.max_shown;
                    s.booster.hide_beyond = limits.hide_beyond;
                    changed
                });
            }
        }
    }

    /// The programs lowered now, from the journal, by name.
    fn booster_note_lowered(self: &Shared, store: &Store) {
        let mut by_name: std::collections::BTreeMap<String, u32> = Default::default();
        if let Ok(entries) = store.entries() {
            for entry in entries {
                if let Target::Process { name, .. } = entry.target {
                    *by_name.entry(name).or_default() += 1;
                }
            }
        }
        let lowered: Vec<(String, u32)> = by_name.into_iter().collect();
        self.update_if(|s| {
            let changed = s.booster.lowered != lowered;
            s.booster.lowered = lowered.clone();
            changed
        });
    }
}

fn current_of(item: Item, snap: &crate::state::Snapshot) -> Option<Known> {
    current_from(item, snap.booster.max_shown, snap.booster.hide_beyond)
}

fn current_from(item: Item, max_shown: Option<Known>, hide_beyond: Option<Known>) -> Option<Known> {
    match item {
        Item::MaxShown => max_shown,
        Item::HideBeyond => hide_beyond,
        Item::BlockVeryPoor | Item::Msaa2x | Item::MsaaOff => None,
    }
}

fn to_put_back_file() -> std::path::PathBuf {
    orbiters_booster::data_dir().join("to-put-back.json")
}

/// Kept on disk, as the person may leave step 3 and come back days later: no setting of anyone's, only a reminder.
fn keep_to_put_back(list: &[ToPutBack]) {
    let path = to_put_back_file();
    if list.is_empty() {
        let _ = std::fs::remove_file(path);
    } else if let (Some(dir), Ok(bytes)) = (path.parent(), serde_json::to_vec_pretty(list)) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(path, bytes);
    }
}

fn read_to_put_back() -> Vec<ToPutBack> {
    std::fs::read(to_put_back_file()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Test builds only: what the 10-minute test needs.
pub(crate) fn dev_build() -> bool {
    orbiters_core::app_version().contains("-dev")
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// A test build's small program using four busy threads and no GPU, under an allowlisted name so that step 1 has something to
/// act on: a copy of the setup, which has this mode, in a folder of its own. It stops when asked, by its file going, or after
/// a few minutes whatever happens: nothing has to kill it.
struct TestLoad {
    stop_file: std::path::PathBuf,
}

impl TestLoad {
    fn start() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let setup = exe.parent()?.join(if cfg!(windows) { "orbiters-setup.exe" } else { "orbiters-setup" });
        let dir = std::env::temp_dir().join("orbiters-booster-test-load");
        std::fs::create_dir_all(&dir).ok()?;
        let copy = dir.join("blender.exe");
        std::fs::copy(&setup, &copy).ok()?;
        let stop_file = dir.join("running");
        std::fs::write(&stop_file, "").ok()?;
        std::process::Command::new(&copy).arg("--booster-test-load").arg(&stop_file).spawn().ok()?;
        Some(Self { stop_file })
    }
}

impl Drop for TestLoad {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.stop_file);
    }
}

/// What was measured, to the logs folder, in the language of the app: Enzo reads it after the test.
fn write_test_report(snap: &crate::state::Snapshot) -> Option<std::path::PathBuf> {
    let path = orbiters_core::paths::logs_dir().join("fps-test-report.txt");
    let _ = std::fs::create_dir_all(path.parent()?);
    let compositor = orbiters_booster::vrsettings::compositor_log_for_user()
        .and_then(|log| std::fs::read_to_string(log).ok())
        .and_then(|text| orbiters_booster::log::parse_compositor(&text).pop());
    std::fs::write(&path, crate::booster::test_report(snap, compositor.as_ref())).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::Controller;
    use orbiters_core::settings::Settings;

    fn level_of(ctl: &Shared) -> Level {
        ctl.view().settings.booster.level
    }

    /// One state: a level set from either surface is the settings' own, in the snapshot both draw from.
    #[test]
    fn off_rejects_stale_trial_wants_but_allows_a_new_manual_test() {
        use orbiters_booster::engine::{Request, Testing};
        use orbiters_booster::trial::{Kind, Side};
        let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        snap.booster.watching = true;
        snap.booster.overlay.report.level = Level::Background;
        snap.booster.overlay.report.wants.lower_programs = true;
        snap.booster.overlay.report.testing = Some(Testing { kind: Kind::Programs, seconds_left: 20.0, side: Side::With, manual: false });
        assert_eq!(effective_wants(&snap), Wants::default());
        snap.booster.overlay.report.level = Level::Off;
        snap.booster.overlay.report.testing.as_mut().unwrap().manual = true;
        assert!(effective_wants(&snap).lower_programs);
        snap.booster.request = Some((1, Request::StopTest));
        assert_eq!(effective_wants(&snap), Wants::default());
    }

    #[test]
    fn a_level_set_anywhere_is_in_the_one_snapshot() {
        let ctl = Controller::new(Settings::default());
        assert_eq!(level_of(&ctl), Level::Off);
        ctl.patch_settings(&serde_json::json!({ "booster": { "level": Level::Picture } }));
        assert_eq!(level_of(&ctl), Level::Picture);
        ctl.booster_command(Command::PutBack);
        assert_eq!(level_of(&ctl), Level::Off, "put everything back is Off");
    }

    /// A request reaches the overlay through the snapshot, once: answered, it leaves; and an overlay that dies with one takes
    /// it along, so that a simulated crash cannot crash the next overlay too.
    #[test]
    fn a_request_goes_once_and_dies_with_the_overlay() {
        let ctl = Controller::new(Settings::default());
        ctl.booster_command(Command::Overlay(Request::SimulateCrash));
        let (number, request) = ctl.view().booster.request.unwrap();
        assert_eq!(request, Request::SimulateCrash);
        ctl.booster_report(OverlayReport { answered: number, ..Default::default() });
        assert_eq!(ctl.view().booster.request, None);
        assert!(ctl.view().booster.watching);
        ctl.booster_command(Command::Overlay(Request::SimulateCrash));
        assert!(ctl.view().booster.request.is_some());
        ctl.booster_overlay_gone();
        let view = ctl.view();
        assert_eq!(view.booster.request, None);
        assert!(!view.booster.watching);
        assert_eq!(view.booster.overlay, OverlayReport::default(), "nothing it wanted holds any more");
    }

    #[test]
    fn nothing_is_held_until_the_overlay_says_so() {
        let ctl = Controller::new(Settings::default());
        assert!(!ctl.booster.holds_reconstructions() && !ctl.booster.holds_preparing());
    }
}
