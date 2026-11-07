//! SteamVR's sharpness, changed and put back through the journal. The only two keys the booster ever touches are
//! `steamvr/supersampleManualOverride` and `steamvr/supersampleScale` ([`owned`]): any other key, whatever a journal says, is
//! neither read nor written.
//!
//! **A failed call is not a key that is not set.** SteamVR answers a key that is not set with its default, or, for one with no
//! default, error 5; any other error (1, its server gone) means nothing is known. Such a read is [`Read::Unreachable`]: the
//! entry stays in the journal, and so does `RunOnce`, until a restore is read back as done. Whether a key is there at all comes
//! from SteamVR's file, which it keeps.
//!
//! What a crash of SteamVR's leaves: SteamVR saved the booster's values in its file as they were set, so they come back with the
//! next SteamVR. An overlay that ends because SteamVR is gone does not restore through a SteamVR that is no more
//! ([`Sharpness::teardown`]); the entries stay, and either the service puts the file right while SteamVR is closed, or the next
//! overlay restores live as the first thing it does ([`Sharpness::recover`]), finding the booster's values there and putting
//! back the person's.

use crate::engine::{LeftAlone, ValueWatch};
use crate::journal::{Access, Applied, Change, Entry, Read, Store, Target, Value};
use crate::vrsettings;
use std::path::PathBuf;

pub const SECTION: &str = "steamvr";
pub const MANUAL_OVERRIDE: &str = "supersampleManualOverride";
pub const SCALE: &str = "supersampleScale";
const AUTO_SECTION: &str = "GpuSpeed";
const AUTO_SCALE: &str = "gpuSpeedRenderTargetScale";
/// `EVRSettingsError_UnsetSettingHasNoDefault`: the only error that says something of the key itself.
pub const UNSET_HAS_NO_DEFAULT: i32 = 5;
/// Lower than this, whatever it was, nothing goes; nor below this share of what it was.
pub const FLOOR: f32 = 0.7;
pub const FLOOR_OF: f32 = 0.7;

/// The keys the booster may change. Nothing else of SteamVR's, from any journal.
pub fn owned(section: &str, key: &str) -> bool {
    section == SECTION && (key == MANUAL_OVERRIDE || key == SCALE)
}

/// SteamVR's settings calls, each with SteamVR's error code. The overlay's is `orbiters_openvr::SettingsHandle`; tests give
/// their own, which can crash.
pub trait SettingsApi {
    fn get_bool(&self, section: &str, key: &str) -> Result<bool, i32>;
    fn get_float(&self, section: &str, key: &str) -> Result<f32, i32>;
    fn set_bool(&self, section: &str, key: &str, value: bool) -> Result<(), i32>;
    fn set_float(&self, section: &str, key: &str, value: f32) -> Result<(), i32>;
    fn remove(&self, section: &str, key: &str) -> Result<(), i32>;
}

/// [`Access`] to SteamVR's live settings.
pub struct Live<'a> {
    pub api: &'a dyn SettingsApi,
    pub file: Option<PathBuf>,
    /// Keys taken away this session, which the file may not say yet.
    pub removed: &'a mut Vec<String>,
}

impl Live<'_> {
    /// Whether the key is in SteamVR's file: `None` when the file can't be read or isn't what SteamVR writes.
    fn in_file(&self, section: &str, key: &str) -> Option<bool> {
        let text = std::fs::read_to_string(self.file.as_ref()?).ok()?;
        vrsettings::read_value(&text, section, key).map(|v| v != Value::Absent)
    }

    fn value(&self, section: &str, key: &str) -> Read {
        let got = if key == MANUAL_OVERRIDE { self.api.get_bool(section, key).map(Value::Bool) } else { self.api.get_float(section, key).map(Value::Float) };
        match got {
            Ok(value) => Read::Value(value),
            Err(UNSET_HAS_NO_DEFAULT) => Read::Value(Value::Absent),
            // IPC failed, SteamVR gone: nothing is known, and nothing may be concluded.
            Err(_) => Read::Unreachable,
        }
    }
}

impl Access for Live<'_> {
    fn read(&mut self, target: &Target) -> Read {
        match target {
            Target::SteamVr { section, key } if owned(section, key) => self.value(section, key),
            // Not one of the booster's: the entry goes without anything being read or written.
            Target::SteamVr { .. } => Read::Gone,
            Target::Process { .. } => Read::Unreachable,
        }
    }

    fn write(&mut self, target: &Target, value: &Value) -> bool {
        let Target::SteamVr { section, key } = target else { return false };
        if !owned(section, key) {
            return false;
        }
        let done = match value {
            Value::Absent => self.api.remove(section, key).is_ok(),
            Value::Bool(b) => self.api.set_bool(section, key, *b).is_ok(),
            Value::Float(f) => self.api.set_float(section, key, *f).is_ok(),
            Value::Int(_) => false,
        };
        if done && *value == Value::Absent {
            self.removed.push(key.clone());
        }
        done
    }

    fn absent_now(&mut self, target: &Target) -> bool {
        let Target::SteamVr { section, key } = target else { return false };
        self.removed.contains(key) || self.in_file(section, key) == Some(false)
    }

    fn original(&mut self, target: &Target) -> Read {
        let Target::SteamVr { section, key } = target else { return Read::Unreachable };
        match self.in_file(section, key) {
            // Not in the file: what was here is absent, not the default SteamVR answers with.
            Some(false) => Read::Value(Value::Absent),
            Some(true) => self.read(target),
            // The file can't say: the original can't be known, so nothing is changed.
            None if self.file.is_some() => Read::Unreachable,
            None => self.read(target),
        }
    }
}

fn is_steamvr(e: &Entry) -> bool {
    matches!(e.target, Target::SteamVr { .. })
}

fn floor_of(base: f32) -> f32 {
    (base * FLOOR_OF).max(FLOOR)
}

/// What the booster holds of SteamVR's sharpness.
pub struct Sharpness {
    store: Store,
    file: Option<PathBuf>,
    removed: Vec<String>,
    /// What it was before the booster (`None`: Auto), and the base the factor applies to.
    base: Option<(Option<f32>, f32)>,
    factor: f32,
    /// A restore that could not finish: tried again at each check, never given up.
    owed: bool,
    watch: ValueWatch,
}

impl Sharpness {
    pub fn new(store: Store, file: Option<PathBuf>) -> Self {
        Self { store, file, removed: Vec::new(), base: None, factor: 1.0, owed: false, watch: ValueWatch::default() }
    }

    /// While it is changed: what it was (`None`: Auto), and what it is now.
    pub fn shown(&self) -> Option<(Option<f32>, f32)> {
        self.base.filter(|_| self.factor < 1.0).map(|(was, base)| (was, (base * self.factor).max(floor_of(base))))
    }

    fn live<'a>(&'a mut self, api: &'a dyn SettingsApi) -> (Live<'a>, &'a Store) {
        (Live { api, file: self.file.clone(), removed: &mut self.removed }, &self.store)
    }

    /// Restores what is owed in SteamVR, what was reachable. Whether all of it is done; what the person changed meanwhile.
    fn restore(&mut self, api: &dyn SettingsApi) -> (bool, bool) {
        let (mut live, store) = self.live(api);
        let got = store.restore(&mut live, &is_steamvr, &mut |_| {});
        let _ = store.rearm();
        match got {
            Ok(r) => (r.waiting.is_empty(), !r.kept.is_empty()),
            Err(_) => (false, false),
        }
    }

    /// The first thing a new overlay does: what the last one left comes back, if SteamVR answers.
    pub fn recover(&mut self, api: &dyn SettingsApi) {
        let (done, _) = self.restore(api);
        self.owed = !done;
    }

    /// What it is before the booster: Auto's own choice counts as the base when no manual value is set. `None`: SteamVR
    /// didn't answer.
    fn base_now(api: &dyn SettingsApi) -> Option<(Option<f32>, f32)> {
        let manual = match api.get_bool(SECTION, MANUAL_OVERRIDE) {
            Ok(manual) => manual,
            Err(UNSET_HAS_NO_DEFAULT) => false,
            Err(_) => return None,
        };
        if manual {
            let scale = api.get_float(SECTION, SCALE).ok()?;
            return Some((Some(scale), scale));
        }
        match api.get_float(AUTO_SECTION, AUTO_SCALE) {
            Ok(scale) => Some((None, scale)),
            Err(UNSET_HAS_NO_DEFAULT) => Some((None, 1.0)),
            Err(_) => None,
        }
    }

    /// Follows the factor the engine wants. What makes it leave sharpness alone, if anything.
    pub fn follow(&mut self, api: &dyn SettingsApi, factor: f32) -> Option<LeftAlone> {
        if (factor - self.factor).abs() < 0.001 {
            return None;
        }
        if factor >= 0.999 {
            self.factor = 1.0;
            self.base = None;
            self.watch.forget_last();
            let (done, kept) = self.restore(api);
            self.owed = !done;
            return kept.then_some(LeftAlone::HandChanged);
        }
        if self.base.is_none() {
            self.base = Self::base_now(api);
        }
        let Some((_, base)) = self.base else { return Some(LeftAlone::Refused) };
        let target = (base * factor).max(floor_of(base));
        if target >= base {
            return None;
        }
        let scale = Target::SteamVr { section: SECTION.into(), key: SCALE.into() };
        let manual = Target::SteamVr { section: SECTION.into(), key: MANUAL_OVERRIDE.into() };
        let changes = [
            Change { target: manual, value: Value::Bool(true), leave_with: Some(scale.clone()) },
            Change { target: scale, value: Value::Float(target), leave_with: None },
        ];
        let (mut live, store) = self.live(api);
        let got = store.apply(&mut live, &changes, &mut |_| {});
        match got {
            Ok(results) if results.contains(&Applied::ChangedSince) => {
                self.base = None;
                Some(LeftAlone::HandChanged)
            }
            Ok(results) if results.iter().all(|r| matches!(r, Applied::Done | Applied::AlreadySo)) => {
                self.factor = factor;
                self.watch.forget_last();
                None
            }
            // Half of it took, or none: what did take comes back now, not at the end of the session. Manual left on at
            // whatever scale was there would be a change nobody measured.
            _ => {
                self.base = None;
                self.factor = 1.0;
                let (done, _) = self.restore(api);
                self.owed = !done;
                Some(LeftAlone::Refused)
            }
        }
    }

    /// Every few seconds: a restore still owed is tried again; a value the booster owns that is no longer what it wrote is the
    /// person's. A read that fails is neither: it says nothing, and the entry stays.
    pub fn check(&mut self, api: &dyn SettingsApi, watching: bool) -> Option<LeftAlone> {
        if self.owed {
            let (done, kept) = self.restore(api);
            self.owed = !done;
            return kept.then_some(LeftAlone::HandChanged);
        }
        if self.factor < 1.0 {
            let (mut live, store) = self.live(api);
            let kept = store.release_changed(&mut live, &is_steamvr);
            if kept.is_ok_and(|k| !k.is_empty()) {
                self.factor = 1.0;
                self.base = None;
                // What it stood on stays, as the journal's `leave_with` says; anything else of the booster's comes back.
                let (done, _) = self.restore(api);
                self.owed = !done;
                return Some(LeftAlone::HandChanged);
            }
        } else if watching {
            if let Ok(value) = api.get_float(SECTION, SCALE) {
                if self.watch.see(value) {
                    return Some(LeftAlone::AnotherApp);
                }
            }
        }
        None
    }

    /// The session ends. With SteamVR still there (a quit, the app closing), everything comes back live, and SteamVR saves that.
    /// With SteamVR gone (a crash), nothing can be read or written: the entries and `RunOnce` stay for the file and the next
    /// overlay.
    pub fn teardown(&mut self, api: &dyn SettingsApi, steamvr_gone: bool) {
        if !steamvr_gone {
            let (done, _) = self.restore(api);
            self.owed = !done;
        }
        let _ = self.store.rearm();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::journal::tests::Scratch;
    use crate::journal::{Armed, Store};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A SteamVR: settings in memory, saved to its file as they are set (as SteamVR does), and a crash that makes every call
    /// fail with error 1 as the real one does once its server is gone.
    pub(crate) struct FakeSteamVr {
        pub values: RefCell<BTreeMap<String, Value>>,
        pub file: PathBuf,
        pub crashed: Cell<bool>,
        /// What Auto chose.
        pub auto: f32,
        /// Refuse every write of this key, as a SteamVR that won't take a change.
        pub refuse: Option<&'static str>,
    }

    impl FakeSteamVr {
        /// One started from the file, as SteamVR does.
        pub(crate) fn start(file: &Path) -> Self {
            let text = std::fs::read_to_string(file).unwrap_or_else(|_| "{}".into());
            let mut values = BTreeMap::new();
            for key in [MANUAL_OVERRIDE, SCALE] {
                if let Some(v) = vrsettings::read_value(&text, SECTION, key).filter(|v| *v != Value::Absent) {
                    values.insert(key.to_string(), v);
                }
            }
            Self { values: RefCell::new(values), file: file.to_path_buf(), crashed: Cell::new(false), auto: 1.5, refuse: None }
        }

        fn save(&self, key: &str, value: Value) {
            let text = std::fs::read_to_string(&self.file).unwrap_or_else(|_| "{}".into());
            std::fs::write(&self.file, vrsettings::edit_text(&text, SECTION, key, &value).unwrap()).unwrap();
        }

        fn set(&self, key: &str, value: Value) -> Result<(), i32> {
            if self.crashed.get() {
                return Err(1);
            }
            if self.refuse == Some(key) {
                return Err(2);
            }
            if value == Value::Absent {
                self.values.borrow_mut().remove(key);
            } else {
                self.values.borrow_mut().insert(key.to_string(), value);
            }
            self.save(key, value);
            Ok(())
        }
    }

    impl SettingsApi for FakeSteamVr {
        fn get_bool(&self, _: &str, key: &str) -> Result<bool, i32> {
            if self.crashed.get() {
                return Err(1);
            }
            Ok(matches!(self.values.borrow().get(key), Some(Value::Bool(true))))
        }
        fn get_float(&self, section: &str, key: &str) -> Result<f32, i32> {
            if self.crashed.get() {
                return Err(1);
            }
            if section == AUTO_SECTION {
                return Ok(self.auto);
            }
            Ok(match self.values.borrow().get(key) {
                Some(Value::Float(f)) => *f,
                _ => 1.0,
            })
        }
        fn set_bool(&self, _: &str, key: &str, value: bool) -> Result<(), i32> {
            self.set(key, Value::Bool(value))
        }
        fn set_float(&self, _: &str, key: &str, value: f32) -> Result<(), i32> {
            self.set(key, Value::Float(value))
        }
        fn remove(&self, _: &str, key: &str) -> Result<(), i32> {
            self.set(key, Value::Absent)
        }
    }

    /// Recovery at the next sign-in, as `RunOnce` would ask for it.
    #[derive(Clone, Default)]
    pub(crate) struct RunOnceFlag(pub Arc<AtomicBool>);

    impl Armed for RunOnceFlag {
        fn arm(&self) -> bool {
            self.0.store(true, Ordering::SeqCst);
            true
        }
        fn disarm(&self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    const START: &str = "{\n   \"steamvr\" : {\n      \"disableAsync\" : true\n   }\n}\n";

    fn session(dir: &Path) -> (Sharpness, FakeSteamVr, RunOnceFlag) {
        let file = dir.join("steamvr.vrsettings");
        if !file.exists() {
            std::fs::write(&file, START).unwrap();
        }
        let flag = RunOnceFlag::default();
        let sharpness = Sharpness::new(Store::new(dir, Box::new(flag.clone())), Some(file.clone()));
        (sharpness, FakeSteamVr::start(&file), flag)
    }

    fn file_value(dir: &Path, key: &str) -> Value {
        vrsettings::read_value(&std::fs::read_to_string(dir.join("steamvr.vrsettings")).unwrap(), SECTION, key).unwrap()
    }

    /// B1, a crash: on Auto, sharpness lowered, then SteamVR's server dies. The 5-second check and the overlay's end both meet
    /// failing calls: nothing is concluded from them, the journal keeps both entries and RunOnce stays armed. With SteamVR
    /// closed the service puts the file right: Auto again, every other key as it was.
    #[test]
    fn a_steamvr_crash_keeps_what_is_owed_and_the_file_comes_back_to_auto() {
        let dir = Scratch::new("crash");
        let (mut sharpness, steamvr, flag) = session(&dir.0);
        assert_eq!(sharpness.follow(&steamvr, 0.85), None);
        assert_eq!(file_value(&dir.0, MANUAL_OVERRIDE), Value::Bool(true), "SteamVR saved the booster's values");
        steamvr.crashed.set(true);
        assert_eq!(sharpness.check(&steamvr, true), None, "a failed read is not a hand change");
        sharpness.teardown(&steamvr, true);
        let store = Store::new(&dir.0, Box::new(flag.clone()));
        assert_eq!(store.entries().unwrap().len(), 2, "both still owed");
        assert!(flag.0.load(Ordering::SeqCst), "and recovery still armed");
        // SteamVR closed: the service's path, through its file.
        let closed = || true;
        let mut file = vrsettings::SettingsFile { path: dir.0.join("steamvr.vrsettings"), closed: &closed };
        let got = store.restore(&mut file, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(got.back.len(), 2);
        assert_eq!(std::fs::read_to_string(dir.0.join("steamvr.vrsettings")).unwrap(), START, "Auto, and the file as it was");
        assert!(!flag.0.load(Ordering::SeqCst));
    }

    /// B1, a restart before anyone touched the file: the new SteamVR loaded the booster's values, and the next overlay finds
    /// them and puts back what the person had.
    #[test]
    fn after_a_steamvr_restart_the_next_overlay_puts_back_the_persons_values() {
        let dir = Scratch::new("restart");
        // The person had a manual 1.4.
        std::fs::write(
            dir.0.join("steamvr.vrsettings"),
            "{\n   \"steamvr\" : {\n      \"supersampleManualOverride\" : true,\n      \"supersampleScale\" : 1.4\n   }\n}\n",
        )
        .unwrap();
        let (mut sharpness, steamvr, flag) = session(&dir.0);
        sharpness.follow(&steamvr, 0.85);
        assert!(matches!(file_value(&dir.0, SCALE), Value::Float(f) if (f - 1.19).abs() < 0.001));
        steamvr.crashed.set(true);
        sharpness.teardown(&steamvr, true);
        drop(sharpness);
        // SteamVR is back, from its file: the booster's 1.19.
        let (mut next, restarted, next_flag) = session(&dir.0);
        next_flag.0.store(true, Ordering::SeqCst);
        assert!(matches!(restarted.values.borrow().get(SCALE), Some(Value::Float(f)) if (f - 1.19).abs() < 0.001));
        next.recover(&restarted);
        assert!(matches!(restarted.values.borrow().get(SCALE), Some(Value::Float(f)) if (f - 1.4).abs() < 0.001), "the person's 1.4");
        assert_eq!(restarted.values.borrow().get(MANUAL_OVERRIDE), Some(&Value::Bool(true)));
        assert!(Store::new(&dir.0, Box::new(flag.clone())).entries().unwrap().is_empty());
        assert!(!next_flag.0.load(Ordering::SeqCst), "nothing owed: RunOnce gone");
    }

    /// B1, one failed read in the middle of a session, with SteamVR still there: the check lets go of nothing, and the next
    /// one, answering, still finds the booster's value as the booster's.
    #[test]
    fn one_failed_read_lets_go_of_nothing() {
        let dir = Scratch::new("blip");
        let (mut sharpness, steamvr, _) = session(&dir.0);
        sharpness.follow(&steamvr, 0.85);
        steamvr.crashed.set(true);
        assert_eq!(sharpness.check(&steamvr, true), None);
        steamvr.crashed.set(false);
        assert_eq!(sharpness.check(&steamvr, true), None);
        assert!(sharpness.shown().is_some(), "still in force and still owned");
        assert_eq!(sharpness.follow(&steamvr, 1.0), None);
        assert!(steamvr.values.borrow().is_empty(), "Auto again: both keys gone");
    }

    /// A clean end with SteamVR there restores live, as before.
    #[test]
    fn a_clean_end_restores_at_once() {
        let dir = Scratch::new("clean");
        let (mut sharpness, steamvr, flag) = session(&dir.0);
        sharpness.follow(&steamvr, 0.85);
        sharpness.teardown(&steamvr, false);
        assert!(steamvr.values.borrow().is_empty());
        assert!(!flag.0.load(Ordering::SeqCst));
    }

    /// F4: the override took and the scale was refused. The override comes back at once, not at the end of the session.
    #[test]
    fn half_a_change_comes_back_at_once() {
        let dir = Scratch::new("half");
        let (mut sharpness, mut steamvr, _) = session(&dir.0);
        steamvr.refuse = Some(SCALE);
        assert_eq!(sharpness.follow(&steamvr, 0.85), Some(LeftAlone::Refused));
        assert!(steamvr.values.borrow().is_empty(), "Manual is not left on");
        assert!(sharpness.shown().is_none());
        assert!(Store::new(&dir.0, Box::new(crate::journal::NotArmed)).entries().unwrap().is_empty());
    }

    /// A hand change while on: the person's scale and the override it needs stay, and the booster lets go.
    #[test]
    fn a_hand_change_while_on_stays() {
        let dir = Scratch::new("hand-live");
        let (mut sharpness, steamvr, _) = session(&dir.0);
        sharpness.follow(&steamvr, 0.85);
        steamvr.set_float(SECTION, SCALE, 0.9).unwrap();
        assert_eq!(sharpness.check(&steamvr, true), Some(LeftAlone::HandChanged));
        assert_eq!(steamvr.values.borrow().get(SCALE), Some(&Value::Float(0.9)));
        assert_eq!(steamvr.values.borrow().get(MANUAL_OVERRIDE), Some(&Value::Bool(true)));
    }

    /// F6: a journal naming another SteamVR key: nothing of it is read or written, and the entry goes.
    #[test]
    fn a_journal_cannot_reach_another_steamvr_key() {
        let dir = Scratch::new("other-key");
        std::fs::write(
            dir.0.join("journal.json"),
            r#"{"version":1,"entries":[{"target":{"kind":"steam_vr","section":"driver_lighthouse","key":"enable"},"original":{"type":"absent"},"written":{"type":"bool","value":true},"stage":"applied","at":"","app_version":""}]}"#,
        )
        .unwrap();
        let (mut sharpness, steamvr, _) = session(&dir.0);
        let writes = Cell::new(0);
        struct Counting<'a>(&'a FakeSteamVr, &'a Cell<u32>);
        impl SettingsApi for Counting<'_> {
            fn get_bool(&self, s: &str, k: &str) -> Result<bool, i32> {
                self.0.get_bool(s, k)
            }
            fn get_float(&self, s: &str, k: &str) -> Result<f32, i32> {
                self.0.get_float(s, k)
            }
            fn set_bool(&self, s: &str, k: &str, v: bool) -> Result<(), i32> {
                self.1.set(self.1.get() + 1);
                self.0.set_bool(s, k, v)
            }
            fn set_float(&self, s: &str, k: &str, v: f32) -> Result<(), i32> {
                self.1.set(self.1.get() + 1);
                self.0.set_float(s, k, v)
            }
            fn remove(&self, s: &str, k: &str) -> Result<(), i32> {
                self.1.set(self.1.get() + 1);
                self.0.remove(s, k)
            }
        }
        sharpness.recover(&Counting(&steamvr, &writes));
        assert_eq!(writes.get(), 0);
        assert!(Store::new(&dir.0, Box::new(crate::journal::NotArmed)).entries().unwrap().is_empty());
    }
}
