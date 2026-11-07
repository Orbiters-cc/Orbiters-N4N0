//! Windows' default sound devices while SteamVR runs (Settings, "When SteamVR starts"; `docs/vr-audio-devices.md`).
//!
//! When the service sees SteamVR start, the chosen output and input become Windows' defaults for all three roles,
//! and are set again for a while after ([`APPLY_FOR`]): Virtual Desktop and SteamVR put their own a few seconds after
//! the start, and the chosen device may only appear then. The devices Windows had before are kept in
//! `vr-audio.json` (so a restart or a crash in between loses nothing) and put back when SteamVR closes, if the
//! settings say so. "Before" is what held outside VR for a while ([`SETTLED`]), not what Virtual Desktop set just
//! before it started SteamVR.
//!
//! A device is found by its id, else by its name (a driver installed again gives its devices new ids). The logic is
//! [`Keeper`], over [`Devices`] (Windows' in [`crate::audio_devices`], a fake one in the tests); [`run`] is the
//! service's thread. SteamVR does not run on a Mac, so nothing here does either.

use super::{Controller, Shared};
use crate::audio_devices::{AudioEndpoint, Flow, Role, ROLES};
use orbiters_core::settings::VrAudioSettings;
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// After SteamVR starts (or the choice changes while it runs), the defaults are looked at and set again this long.
pub const APPLY_FOR: Duration = Duration::from_secs(40);
/// After SteamVR closes, the devices put back are looked at and put back again this long.
pub const RESTORE_FOR: Duration = Duration::from_secs(12);
/// A default outside VR that held this long is the one to put back.
pub const SETTLED: Duration = Duration::from_secs(45);
/// Times a flow's default is set in one of those windows at most: another program wanting its own is not fought on.
pub const MAX_SETS: u8 = 5;

const FLOWS: [Flow; 2] = [Flow::Output, Flow::Input];

/// Windows' sound devices, as the keeper needs them.
pub trait Devices {
    /// The devices of `flow` that are there.
    fn list(&self, flow: Flow) -> Result<Vec<AudioEndpoint>, String>;
    /// The default of each of [`ROLES`].
    fn defaults(&self, flow: Flow) -> [Option<AudioEndpoint>; 3];
    fn set_default(&self, id: &str, role: Role) -> Result<(), String>;
}

/// This computer's.
pub struct Windows;

impl Devices for Windows {
    fn list(&self, flow: Flow) -> Result<Vec<AudioEndpoint>, String> {
        crate::audio_devices::list(flow)
    }

    fn defaults(&self, flow: Flow) -> [Option<AudioEndpoint>; 3] {
        crate::audio_devices::defaults(flow)
    }

    fn set_default(&self, id: &str, role: Role) -> Result<(), String> {
        crate::audio_devices::set_default(id, role)
    }
}

/// What the Settings show: the last thing done, or that failed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub last: Option<Event>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    /// Unix time in milliseconds.
    pub utc_ms: u64,
    /// "Set to Speakers (Valve Index) and Microphone (Valve Index)".
    pub text: String,
    pub error: bool,
}

/// One flow's defaults, role by role ([`ROLES`]).
pub type Roles = [Option<AudioEndpoint>; 3];

/// What is kept across restarts, per flow (output, input).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// The defaults before they were set for VR, to put back.
    pub previous: [Option<Roles>; 2],
    /// What was set for VR (nothing to put back where nothing was set).
    pub set: [Option<AudioEndpoint>; 2],
    /// The defaults outside VR that held [`SETTLED`].
    pub outside: [Option<Roles>; 2],
    pub last: Option<Event>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Apply,
    Restore,
}

/// A while in which the defaults are looked at every turn and set (again) when they are not the wanted ones.
#[derive(Debug)]
struct Window {
    kind: Kind,
    until: Instant,
    /// Times each flow was set.
    sets: [u8; 2],
    /// Each flow's device was not there at the last look.
    missing: [bool; 2],
}

impl Window {
    fn new(kind: Kind, until: Instant) -> Self {
        Self { kind, until, sets: [0; 2], missing: [false; 2] }
    }
}

/// The choice of each flow, `None`: left as it is.
type Choice = [Option<AudioEndpoint>; 2];

/// Finds `wanted` among `list`: by its id, else by its name.
pub fn resolve<'a>(wanted: &AudioEndpoint, list: &'a [AudioEndpoint]) -> Option<&'a AudioEndpoint> {
    list.iter().find(|e| e.id == wanted.id).or_else(|| {
        let name = wanted.name.trim();
        (!name.is_empty()).then(|| list.iter().find(|e| e.name.trim().eq_ignore_ascii_case(name))).flatten()
    })
}

fn same(a: &Roles, b: &Roles) -> bool {
    a.iter().zip(b).all(|(a, b)| a.as_ref().map(|e| &e.id) == b.as_ref().map(|e| &e.id))
}

fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => tr!("{a} and {b}", a = a, b = b),
        [rest @ .., last] => tr!("{list} and {last}", list = rest.join(tr(", ")), last = last),
    }
}

/// Keeps Windows' defaults as the settings ask while SteamVR runs: [`Keeper::tick`] each turn.
pub struct Keeper<D: Devices> {
    devices: D,
    pub saved: Saved,
    /// The SteamVR being served (its `vrserver` process id, 0 unknown).
    session: Option<u32>,
    started: bool,
    window: Option<Window>,
    /// Each flow's defaults seen outside VR, and since when.
    seen: [Option<(Roles, Instant)>; 2],
    choice: Choice,
}

impl<D: Devices> Keeper<D> {
    pub fn new(devices: D, saved: Saved) -> Self {
        Self { devices, saved, session: None, started: false, window: None, seen: Default::default(), choice: Default::default() }
    }

    /// Whether it is watching the defaults closely (turns should come often).
    pub fn busy(&self) -> bool {
        self.window.is_some()
    }

    /// One turn: `steamvr` is the running SteamVR (its process id, 0 unknown), `utc_ms` the time events get. True when
    /// [`Keeper::saved`] changed (to be written, and its last event shown).
    pub fn tick(&mut self, now: Instant, utc_ms: u64, steamvr: Option<u32>, settings: &VrAudioSettings) -> bool {
        let before = self.saved.clone();
        let choice: Choice = [settings.output.clone(), settings.input.clone()];
        match (self.session, steamvr) {
            (None, Some(pid)) => self.start(now, pid),
            // Another SteamVR (one quit and started again between two turns): the devices before stay the ones to
            // put back.
            (Some(was), Some(pid)) if was != pid => self.start(now, pid),
            (Some(_), None) => self.end(now, settings.restore),
            (None, None) if !self.started => self.leftover(utc_ms, settings.restore),
            _ => {}
        }
        self.started = true;
        // Chosen otherwise while SteamVR runs: set now (and watched again a while).
        if self.session.is_some() && choice != self.choice {
            self.window = Some(Window::new(Kind::Apply, now + APPLY_FOR));
        }
        self.choice = choice.clone();
        match self.window.as_ref().map(|w| (w.kind, now >= w.until)) {
            Some((Kind::Apply, false)) => self.apply(utc_ms, &choice),
            Some((Kind::Apply, true)) => self.applied(utc_ms, &choice),
            Some((Kind::Restore, false)) => self.restore(utc_ms),
            Some((Kind::Restore, true)) => self.restored(utc_ms),
            None => {}
        }
        if self.session.is_none() && self.window.is_none() {
            self.watch_outside(now, &choice);
        }
        self.saved != before
    }

    fn note(&mut self, utc_ms: u64, text: String, error: bool) {
        self.saved.last = Some(Event { utc_ms, text, error });
    }

    fn start(&mut self, now: Instant, pid: u32) {
        self.session = Some(pid);
        self.window = Some(Window::new(Kind::Apply, now + APPLY_FOR));
    }

    /// Sets each chosen device that is there and not yet every role's default.
    fn apply(&mut self, utc_ms: u64, choice: &Choice) {
        let mut set = Vec::new();
        let mut again = false;
        let mut failed = Vec::new();
        for (i, flow) in FLOWS.into_iter().enumerate() {
            let Some(wanted) = &choice[i] else { continue };
            let Ok(list) = self.devices.list(flow) else { continue };
            let window = self.window.as_mut().expect("an apply window");
            let Some(target) = resolve(wanted, &list).cloned() else {
                window.missing[i] = true;
                continue;
            };
            window.missing[i] = false;
            let current = self.devices.defaults(flow);
            if current.iter().all(|c| c.as_ref().is_some_and(|c| c.id == target.id)) || window.sets[i] >= MAX_SETS {
                continue;
            }
            window.sets[i] += 1;
            // Set before (this window, or before a restart) and changed since; not a new choice.
            again |= window.sets[i] > 1 || self.saved.set[i].as_ref() == Some(&target);
            if self.saved.previous[i].is_none() {
                self.saved.previous[i] = Some(self.saved.outside[i].clone().unwrap_or(current));
            }
            let errors: Vec<String> = ROLES.iter().filter_map(|r| self.devices.set_default(&target.id, *r).err()).collect();
            if errors.len() == ROLES.len() {
                failed.push(format!("{}: {}", target.name, errors[0]));
                continue;
            }
            set.push(target.name.clone());
            self.saved.set[i] = Some(target);
        }
        if !failed.is_empty() {
            self.note(utc_ms, tr!("Windows did not take {devices}", devices = failed.join("; ")), true);
        } else if !set.is_empty() {
            let text = if again { tr!("Set again to {devices} (another app had changed it)", devices = names(&set)) } else { tr!("Set to {devices}", devices = names(&set)) };
            self.note(utc_ms, text, false);
        }
    }

    /// The apply window ended: a chosen device that never came is said.
    fn applied(&mut self, utc_ms: u64, choice: &Choice) {
        let Some(window) = self.window.take() else { return };
        let missing: Vec<String> = (0..2)
            .filter(|&i| window.missing[i] && choice[i].is_some())
            .map(|i| {
                let device = choice[i].as_ref().map(|e| e.name.as_str()).unwrap_or_default();
                match self.devices.defaults(FLOWS[i])[0].as_ref() {
                    Some(kept) => tr!("{device} was not there, Windows kept {kept}", device = device, kept = kept.name),
                    None => tr!("{device} was not there", device = device),
                }
            })
            .collect();
        if !missing.is_empty() {
            self.note(utc_ms, missing.join("; "), true);
        }
    }

    /// SteamVR closed: the devices before are put back over [`RESTORE_FOR`] (this turn first).
    fn end(&mut self, now: Instant, restore: bool) {
        self.session = None;
        self.window = None;
        let something = (0..2).any(|i| self.saved.set[i].is_some() && self.saved.previous[i].is_some());
        if !restore || !something {
            self.forget();
            return;
        }
        self.window = Some(Window::new(Kind::Restore, now + RESTORE_FOR));
    }

    fn forget(&mut self) {
        self.saved.previous = Default::default();
        self.saved.set = Default::default();
    }

    /// Puts back, role by role, the devices before that are there and not yet back.
    fn restore(&mut self, utc_ms: u64) {
        let mut back = Vec::new();
        let mut again = false;
        for (i, flow) in FLOWS.into_iter().enumerate() {
            let (Some(previous), Some(_)) = (self.saved.previous[i].clone(), &self.saved.set[i]) else { continue };
            let Ok(list) = self.devices.list(flow) else { continue };
            let current = self.devices.defaults(flow);
            let window = self.window.as_mut().expect("a restore window");
            if window.sets[i] >= MAX_SETS {
                continue;
            }
            let mut changed = false;
            let mut missing = false;
            for (k, role) in ROLES.into_iter().enumerate() {
                let Some(before) = &previous[k] else { continue };
                let Some(target) = resolve(before, &list) else {
                    missing = true;
                    continue;
                };
                if current[k].as_ref().is_some_and(|c| c.id == target.id) {
                    continue;
                }
                changed |= self.devices.set_default(&target.id, role).is_ok();
            }
            window.missing[i] = missing;
            if changed {
                window.sets[i] += 1;
                again |= window.sets[i] > 1;
                if let Some(name) = previous.iter().flatten().next().map(|e| e.name.clone()) {
                    back.push(name);
                }
            }
        }
        if !back.is_empty() {
            let text = if again { tr!("Put back {devices} again (another app had changed it)", devices = names(&back)) } else { tr!("Put back {devices}", devices = names(&back)) };
            self.note(utc_ms, text, false);
        }
    }

    /// The restore window ended: what was set is forgotten; a device before that was not there is said.
    fn restored(&mut self, utc_ms: u64) {
        let Some(window) = self.window.take() else { return };
        let missing: Vec<String> = (0..2)
            .filter(|&i| window.missing[i])
            .filter_map(|i| self.saved.previous[i].as_ref()?.iter().flatten().next().map(|e| e.name.clone()))
            .collect();
        if !missing.is_empty() {
            self.note(utc_ms, tr!("{devices} was not there to put back", devices = names(&missing)), true);
        }
        self.forget();
    }

    /// The app starts with SteamVR closed while devices set for VR were not put back (it quit, or the computer went
    /// off, during VR): put back where Windows still has what was set, else left as the person has them since.
    fn leftover(&mut self, utc_ms: u64, restore: bool) {
        if !restore || self.saved.set.iter().all(Option::is_none) {
            self.forget();
            return;
        }
        let mut back = Vec::new();
        for (i, flow) in FLOWS.into_iter().enumerate() {
            let (Some(previous), Some(set)) = (self.saved.previous[i].clone(), self.saved.set[i].clone()) else { continue };
            let current = self.devices.defaults(flow);
            if current[0].as_ref().is_none_or(|c| c.id != set.id) {
                continue;
            }
            let Ok(list) = self.devices.list(flow) else { continue };
            let mut changed = false;
            for (k, role) in ROLES.into_iter().enumerate() {
                if let Some(target) = previous[k].as_ref().and_then(|p| resolve(p, &list)) {
                    changed |= current[k].as_ref().is_none_or(|c| c.id != target.id) && self.devices.set_default(&target.id, role).is_ok();
                }
            }
            if changed {
                back.extend(previous.iter().flatten().next().map(|e| e.name.clone()));
            }
        }
        if !back.is_empty() {
            self.note(utc_ms, tr!("Put back {devices} (SteamVR had closed while Orbiters - N4N0 was not running)", devices = names(&back)), false);
        }
        self.forget();
    }

    /// Outside VR: notes the defaults that held [`SETTLED`], the ones to put back after VR. Only while a device is
    /// chosen (else Windows is not asked at all).
    fn watch_outside(&mut self, now: Instant, choice: &Choice) {
        for (i, flow) in FLOWS.into_iter().enumerate() {
            if choice[i].is_none() {
                self.seen[i] = None;
                continue;
            }
            let current = self.devices.defaults(flow);
            if current.iter().all(Option::is_none) {
                continue;
            }
            match &self.seen[i] {
                Some((roles, since)) if same(roles, &current) => {
                    if now.duration_since(*since) >= SETTLED && self.saved.outside[i].as_ref() != Some(&current) {
                        self.saved.outside[i] = Some(current);
                    }
                }
                _ => self.seen[i] = Some((current, now)),
            }
        }
    }
}

fn saved_file() -> PathBuf {
    orbiters_core::paths::config_dir().join("vr-audio.json")
}

fn load(path: &Path) -> Saved {
    match orbiters_core::read_text(path) {
        Ok(Some(text)) => orbiters_core::json_lenient::<Saved>(&text).0,
        _ => Saved::default(),
    }
}

fn save(path: &Path, saved: &Saved) {
    let written = serde_json::to_vec_pretty(saved).map_err(std::io::Error::other).and_then(|bytes| orbiters_core::write_atomic(path, &bytes));
    if let Err(e) = written {
        eprintln!("vr audio: {}: {e}", path.display());
    }
}

impl Controller {
    fn set_vr_audio(&self, status: Status) {
        self.update_if(|s| {
            let changed = s.vr_audio != status;
            s.vr_audio = status;
            changed
        });
    }
}

/// The service's thread: a turn every second while it watches the defaults closely, every two otherwise.
pub fn run(ctl: Shared) {
    if cfg!(target_os = "macos") {
        return;
    }
    let path = saved_file();
    let mut keeper = Keeper::new(Windows, load(&path));
    ctl.set_vr_audio(Status { last: keeper.saved.last.clone() });
    loop {
        let snap = ctl.view();
        let steamvr = snap.steamvr.running.then(|| snap.steamvr.pid.unwrap_or(0));
        if keeper.tick(Instant::now(), super::unix_ms(), steamvr, &snap.settings.vr_audio) {
            save(&path, &keeper.saved);
            ctl.set_vr_audio(Status { last: keeper.saved.last.clone() });
        }
        drop(snap);
        std::thread::sleep(if keeper.busy() { Duration::from_secs(1) } else { Duration::from_secs(2) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn ep(id: &str, name: &str) -> AudioEndpoint {
        AudioEndpoint { id: id.into(), name: name.into() }
    }

    /// Windows as a test sees it: the devices there, each role's default, and every default set.
    #[derive(Default)]
    struct Fake {
        outputs: RefCell<Vec<AudioEndpoint>>,
        inputs: RefCell<Vec<AudioEndpoint>>,
        defaults: RefCell<[[Option<String>; 3]; 2]>,
        calls: RefCell<Vec<(String, Role)>>,
        refuse: RefCell<bool>,
    }

    impl Fake {
        fn index(flow: Flow) -> usize {
            (flow == Flow::Input) as usize
        }

        fn devices(&self, flow: Flow) -> std::cell::RefMut<'_, Vec<AudioEndpoint>> {
            if flow == Flow::Output { self.outputs.borrow_mut() } else { self.inputs.borrow_mut() }
        }

        /// Another program (Virtual Desktop, SteamVR) makes `id` the default of every role.
        fn other_app_sets(&self, flow: Flow, id: &str) {
            self.defaults.borrow_mut()[Self::index(flow)] = [Some(id.into()), Some(id.into()), Some(id.into())];
        }

        fn default_of(&self, flow: Flow) -> [Option<String>; 3] {
            self.defaults.borrow()[Self::index(flow)].clone()
        }

        fn sets(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl Devices for &Fake {
        fn list(&self, flow: Flow) -> Result<Vec<AudioEndpoint>, String> {
            Ok(self.devices(flow).clone())
        }

        fn defaults(&self, flow: Flow) -> [Option<AudioEndpoint>; 3] {
            let list = self.devices(flow).clone();
            self.default_of(flow).map(|id| id.and_then(|id| list.iter().find(|e| e.id == id).cloned()))
        }

        fn set_default(&self, id: &str, role: Role) -> Result<(), String> {
            if *self.refuse.borrow() {
                return Err("access denied".into());
            }
            let flow = if self.outputs.borrow().iter().any(|e| e.id == id) { Flow::Output } else { Flow::Input };
            let k = ROLES.iter().position(|r| *r == role).unwrap();
            self.defaults.borrow_mut()[Fake::index(flow)][k] = Some(id.into());
            self.calls.borrow_mut().push((id.into(), role));
            Ok(())
        }
    }

    const SPEAKERS: (&str, &str) = ("{0.0.0}.{speakers}", "Speakers (Realtek)");
    const VD: (&str, &str) = ("{0.0.0}.{vd}", "Speakers (Virtual Desktop Audio)");
    const INDEX: (&str, &str) = ("{0.0.0}.{index}", "Speakers (Valve Index)");
    const MIC: (&str, &str) = ("{0.0.1}.{usb}", "Microphone (USB)");
    const INDEX_MIC: (&str, &str) = ("{0.0.1}.{index}", "Microphone (Valve Index)");

    fn e((id, name): (&str, &str)) -> AudioEndpoint {
        ep(id, name)
    }

    fn computer() -> Fake {
        let fake = Fake::default();
        *fake.outputs.borrow_mut() = vec![e(SPEAKERS), e(VD), e(INDEX)];
        *fake.inputs.borrow_mut() = vec![e(MIC), e(INDEX_MIC)];
        fake.other_app_sets(Flow::Output, SPEAKERS.0);
        fake.other_app_sets(Flow::Input, MIC.0);
        fake
    }

    fn chosen(output: Option<(&str, &str)>, input: Option<(&str, &str)>) -> VrAudioSettings {
        VrAudioSettings { output: output.map(e), input: input.map(e), restore: true }
    }

    fn all(id: &str) -> [Option<String>; 3] {
        [Some(id.into()), Some(id.into()), Some(id.into())]
    }

    #[test]
    fn a_device_is_found_by_its_id_else_by_its_name() {
        let list = vec![ep("new-id", "Speakers (Valve Index)"), ep("other", "Speakers (Realtek)")];
        assert_eq!(resolve(&ep("other", "renamed"), &list).unwrap().id, "other");
        // The driver installed again: another id, the same name (letter case aside).
        assert_eq!(resolve(&ep("old-id", "speakers (valve index) "), &list).unwrap().id, "new-id");
        assert!(resolve(&ep("gone", "Headphones"), &list).is_none());
        assert!(resolve(&ep("gone", ""), &[ep("x", "")]).is_none(), "no name matches no name");
    }

    #[test]
    fn steamvr_starting_sets_every_role_and_closing_puts_back_the_ones_before() {
        let fake = computer();
        let settings = chosen(Some(INDEX), Some(INDEX_MIC));
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        assert!(!keeper.tick(t0, 1, None, &settings));
        assert!(keeper.tick(t0 + Duration::from_secs(2), 2, Some(7), &settings));
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
        assert_eq!(fake.default_of(Flow::Input), all(INDEX_MIC.0));
        assert_eq!(fake.sets(), 6, "three roles each");
        let last = keeper.saved.last.clone().unwrap();
        assert_eq!((last.text.as_str(), last.utc_ms, last.error), ("Set to Speakers (Valve Index) and Microphone (Valve Index)", 2, false));
        assert_eq!(keeper.saved.previous[0].as_ref().unwrap()[0].as_ref().unwrap().id, SPEAKERS.0);
        // Nothing to do while they stay.
        keeper.tick(t0 + Duration::from_secs(3), 3, Some(7), &settings);
        assert_eq!(fake.sets(), 6);
        // SteamVR closes: the ones before, role by role, and then forgotten.
        keeper.tick(t0 + Duration::from_secs(60), 60, None, &settings);
        assert_eq!(fake.default_of(Flow::Output), all(SPEAKERS.0));
        assert_eq!(fake.default_of(Flow::Input), all(MIC.0));
        assert_eq!(keeper.saved.last.as_ref().unwrap().text, "Put back Speakers (Realtek) and Microphone (USB)");
        keeper.tick(t0 + Duration::from_secs(60) + RESTORE_FOR, 80, None, &settings);
        assert!(!keeper.busy());
        assert_eq!(keeper.saved.previous, [None, None]);
        assert_eq!(keeper.saved.set, [None, None]);
    }

    #[test]
    fn a_default_another_app_sets_after_the_start_is_set_again_for_a_while_only() {
        let fake = computer();
        let settings = chosen(Some(INDEX), None);
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        keeper.tick(t0, 1, Some(7), &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
        // Virtual Desktop puts its own a few seconds later.
        fake.other_app_sets(Flow::Output, VD.0);
        keeper.tick(t0 + Duration::from_secs(5), 5, Some(7), &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
        assert!(keeper.saved.last.as_ref().unwrap().text.starts_with("Set again to Speakers (Valve Index)"));
        // A program that keeps wanting its own is not fought on.
        for s in 6..6 + 2 * MAX_SETS as u64 {
            fake.other_app_sets(Flow::Output, VD.0);
            keeper.tick(t0 + Duration::from_secs(s), s, Some(7), &settings);
        }
        assert_eq!(fake.default_of(Flow::Output), all(VD.0));
        assert_eq!(fake.sets(), 3 * MAX_SETS as usize);
        // Past the window: the person's own changes stay.
        let mut keeper = Keeper::new(&fake, Saved::default());
        fake.other_app_sets(Flow::Output, SPEAKERS.0);
        keeper.tick(t0, 1, Some(8), &settings);
        keeper.tick(t0 + APPLY_FOR, 2, Some(8), &settings);
        let before = fake.sets();
        fake.other_app_sets(Flow::Output, SPEAKERS.0);
        keeper.tick(t0 + APPLY_FOR + Duration::from_secs(1), 3, Some(8), &settings);
        assert_eq!(fake.sets(), before);
        assert!(!keeper.busy());
    }

    #[test]
    fn a_device_that_comes_late_is_set_when_it_comes_and_one_that_never_does_is_said() {
        let fake = computer();
        fake.outputs.borrow_mut().retain(|d| d.id != INDEX.0);
        let settings = chosen(Some(INDEX), None);
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        keeper.tick(t0, 1, Some(7), &settings);
        assert_eq!(fake.sets(), 0);
        // The headset's speakers appear once SteamVR has it.
        fake.outputs.borrow_mut().push(e(INDEX));
        keeper.tick(t0 + Duration::from_secs(4), 4, Some(7), &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));

        let fake = computer();
        fake.outputs.borrow_mut().retain(|d| d.id != INDEX.0);
        let mut keeper = Keeper::new(&fake, Saved::default());
        keeper.tick(t0, 1, Some(7), &settings);
        keeper.tick(t0 + APPLY_FOR, 41, Some(7), &settings);
        let last = keeper.saved.last.clone().unwrap();
        assert!(last.error);
        assert_eq!(last.text, "Speakers (Valve Index) was not there, Windows kept Speakers (Realtek)");
        // Nothing was set, so nothing is put back.
        keeper.tick(t0 + APPLY_FOR * 2, 90, None, &settings);
        assert_eq!(fake.sets(), 0);
    }

    #[test]
    fn a_device_with_a_new_id_is_found_by_its_name() {
        let fake = computer();
        let settings = chosen(Some(("{0.0.0}.{index-before-reinstall}", INDEX.1)), None);
        let mut keeper = Keeper::new(&fake, Saved::default());
        keeper.tick(Instant::now(), 1, Some(7), &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
    }

    #[test]
    fn the_devices_put_back_are_the_ones_that_held_outside_vr_not_what_virtual_desktop_set_just_before() {
        let fake = computer();
        let settings = chosen(Some(INDEX), None);
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        keeper.tick(t0, 1, None, &settings);
        keeper.tick(t0 + SETTLED, 2, None, &settings);
        assert_eq!(keeper.saved.outside[0].as_ref().unwrap()[0].as_ref().unwrap().id, SPEAKERS.0);
        // The headset connects: Virtual Desktop takes the default, then starts SteamVR.
        fake.other_app_sets(Flow::Output, VD.0);
        keeper.tick(t0 + SETTLED + Duration::from_secs(2), 3, None, &settings);
        keeper.tick(t0 + SETTLED + Duration::from_secs(4), 4, Some(7), &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
        keeper.tick(t0 + SETTLED * 3, 5, None, &settings);
        assert_eq!(fake.default_of(Flow::Output), all(SPEAKERS.0));
    }

    #[test]
    fn each_role_gets_back_its_own_device() {
        let fake = computer();
        fake.defaults.borrow_mut()[0][2] = Some(VD.0.into()); // calls on another device
        let settings = chosen(Some(INDEX), None);
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        keeper.tick(t0, 1, Some(7), &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
        keeper.tick(t0 + Duration::from_secs(9), 9, None, &settings);
        assert_eq!(fake.default_of(Flow::Output), [Some(SPEAKERS.0.into()), Some(SPEAKERS.0.into()), Some(VD.0.into())]);
    }

    #[test]
    fn without_put_back_the_vr_devices_stay() {
        let fake = computer();
        let settings = VrAudioSettings { restore: false, ..chosen(Some(INDEX), None) };
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        keeper.tick(t0, 1, Some(7), &settings);
        keeper.tick(t0 + Duration::from_secs(9), 9, None, &settings);
        assert_eq!(fake.default_of(Flow::Output), all(INDEX.0));
        assert_eq!((keeper.saved.previous.clone(), keeper.saved.set.clone()), (Default::default(), Default::default()));
    }

    #[test]
    fn dont_change_leaves_windows_alone() {
        let fake = computer();
        let settings = chosen(None, None);
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        for s in 0..5 {
            keeper.tick(t0 + Duration::from_secs(s * 10), s, (s % 2 == 1).then_some(7), &settings);
        }
        assert_eq!(fake.sets(), 0);
        assert_eq!(keeper.saved, Saved::default());
    }

    #[test]
    fn a_choice_made_while_steamvr_runs_is_set_at_once() {
        let fake = computer();
        let mut keeper = Keeper::new(&fake, Saved::default());
        let t0 = Instant::now();
        keeper.tick(t0, 1, Some(7), &chosen(None, None));
        keeper.tick(t0 + APPLY_FOR * 2, 2, Some(7), &chosen(None, None));
        assert!(!keeper.busy());
        keeper.tick(t0 + APPLY_FOR * 2 + Duration::from_secs(1), 3, Some(7), &chosen(None, Some(INDEX_MIC)));
        assert_eq!(fake.default_of(Flow::Input), all(INDEX_MIC.0));
        assert_eq!(fake.default_of(Flow::Output), all(SPEAKERS.0));
        assert_eq!(keeper.saved.last.as_ref().unwrap().text, "Set to Microphone (Valve Index)");
        // Another device chosen: a new choice, not another app's doing; the device before stays the one from outside VR.
        keeper.tick(t0 + APPLY_FOR * 2 + Duration::from_secs(2), 4, Some(7), &chosen(None, Some(MIC)));
        assert_eq!(fake.default_of(Flow::Input), all(MIC.0));
        assert_eq!(keeper.saved.last.as_ref().unwrap().text, "Set to Microphone (USB)");
        assert_eq!(keeper.saved.previous[1].as_ref().unwrap()[0].as_ref().unwrap().id, MIC.0);
    }

    #[test]
    fn a_restart_during_vr_keeps_the_devices_to_put_back() {
        let fake = computer();
        let settings = chosen(Some(INDEX), None);
        let t0 = Instant::now();
        let mut keeper = Keeper::new(&fake, Saved::default());
        keeper.tick(t0, 1, Some(7), &settings);
        let saved = keeper.saved.clone();
        // The app starts again while SteamVR runs: the defaults now are its own, not the ones before.
        let mut keeper = Keeper::new(&fake, saved.clone());
        keeper.tick(t0 + Duration::from_secs(5), 5, Some(7), &settings);
        assert_eq!(keeper.saved.previous, saved.previous);
        keeper.tick(t0 + Duration::from_secs(9), 9, None, &settings);
        assert_eq!(fake.default_of(Flow::Output), all(SPEAKERS.0));
        // SteamVR quit and started again between two turns: the devices before stay.
        let mut keeper = Keeper::new(&fake, Saved::default());
        keeper.tick(t0, 1, Some(7), &settings);
        keeper.tick(t0 + Duration::from_secs(2), 2, Some(8), &settings);
        assert_eq!(keeper.saved.previous[0].as_ref().unwrap()[0].as_ref().unwrap().id, SPEAKERS.0);
    }

    #[test]
    fn devices_left_set_when_the_app_was_not_running_are_put_back_unless_changed_since() {
        let t0 = Instant::now();
        let settings = chosen(Some(INDEX), None);
        let left = || {
            let fake = computer();
            let mut keeper = Keeper::new(&fake, Saved::default());
            keeper.tick(t0, 1, Some(7), &settings);
            let saved = keeper.saved.clone();
            drop(keeper);
            (fake, saved)
        };
        // Still the VR device: put back.
        let (fake, saved) = left();
        let mut keeper = Keeper::new(&fake, saved);
        assert!(keeper.tick(t0, 100, None, &settings));
        assert_eq!(fake.default_of(Flow::Output), all(SPEAKERS.0));
        assert!(keeper.saved.last.as_ref().unwrap().text.contains("SteamVR had closed while Orbiters - N4N0 was not running"));
        assert_eq!(keeper.saved.set, [None, None]);
        // The person chose another since: left alone.
        let (fake, saved) = left();
        fake.other_app_sets(Flow::Output, VD.0);
        let mut keeper = Keeper::new(&fake, saved);
        let before = fake.sets();
        keeper.tick(t0, 100, None, &settings);
        assert_eq!((fake.sets(), fake.default_of(Flow::Output)), (before, all(VD.0)));
        assert_eq!(keeper.saved.set, [None, None]);
    }

    #[test]
    fn windows_refusing_is_said() {
        let fake = computer();
        *fake.refuse.borrow_mut() = true;
        let mut keeper = Keeper::new(&fake, Saved::default());
        keeper.tick(Instant::now(), 1, Some(7), &chosen(Some(INDEX), None));
        let last = keeper.saved.last.clone().unwrap();
        assert!(last.error);
        assert_eq!(last.text, "Windows did not take Speakers (Valve Index): access denied");
        assert_eq!(keeper.saved.set, [None, None]);
    }

    #[test]
    fn what_is_kept_reads_back() {
        let saved = Saved {
            previous: [Some([Some(e(SPEAKERS)), None, Some(e(VD))]), None],
            set: [Some(e(INDEX)), None],
            outside: [None, Some([Some(e(MIC)), Some(e(MIC)), Some(e(MIC))])],
            last: Some(Event { utc_ms: 5, text: "Set to Speakers (Valve Index)".into(), error: false }),
        };
        let text = serde_json::to_string(&saved).unwrap();
        assert_eq!(orbiters_core::json_lenient::<Saved>(&text).0, saved);
        assert_eq!(orbiters_core::json_lenient::<Saved>("{}").0, Saved::default());
    }
}
