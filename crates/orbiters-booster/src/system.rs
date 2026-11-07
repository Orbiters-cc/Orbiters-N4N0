//! The journal in real use: what makes recovery run after a power loss, and recovery itself, as the app at any start and the
//! restorer at sign-in run it.
//!
//! What it means for a computer that dies mid-change: priorities are gone with the processes, and SteamVR's settings, which
//! may have been kept in its file with the booster's values, come back through that file at the next sign-in, before SteamVR
//! is started again.

use crate::journal::{Access, Armed, Entry, Read, Restored, Store, Target, Unreadable, Value};
use crate::programs::{self, ProcessOs, RealOs};
use crate::vrsettings::{self, SettingsFile};
use std::path::{Path, PathBuf};

/// What `RunOnce`'s entry is called.
pub const RUN_ONCE_VALUE: &str = "OrbitersBoosterRestore";
#[cfg(not(test))]
const RUN_ONCE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\RunOnce";
/// What the app and the setup are started with to restore.
pub const RESTORE_ARGUMENT: &str = "--booster-restore";

/// A `RunOnce` entry that runs the restorer at the next sign-in. The key is given, so that tests use one of their own: the
/// real one is only named by [`RunOnce::for_user`].
pub struct RunOnce {
    key: String,
    value: String,
    command: String,
}

impl RunOnce {
    pub fn in_key(key: &str, value: &str, restorer: &Path) -> Self {
        Self { key: key.to_string(), value: value.to_string(), command: format!("\"{}\" {RESTORE_ARGUMENT}", restorer.display()) }
    }

    /// `HKCU\…\RunOnce`, running `restorer`.
    pub fn for_user(restorer: &Path) -> Self {
        #[cfg(test)]
        {
            let _ = restorer;
            panic!("tests never name the real RunOnce key");
        }
        #[cfg(not(test))]
        Self::in_key(RUN_ONCE_KEY, RUN_ONCE_VALUE, restorer)
    }
}

#[cfg(windows)]
impl Armed for RunOnce {
    fn arm(&self) -> bool {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
        };
        let wide = |t: &str| t.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (key_name, value_name, command) = (wide(&self.key), wide(&self.value), wide(&self.command));
        let mut key = HKEY::default();
        // SAFETY: NUL-terminated names and a valid out parameter.
        let made =
            unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None) };
        if made != ERROR_SUCCESS {
            return false;
        }
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(command.as_ptr().cast(), command.len() * 2) };
        // SAFETY: a NUL-terminated name, and the bytes of the wide text with its end.
        let set = unsafe { RegSetValueExW(key, PCWSTR(value_name.as_ptr()), None, REG_SZ, Some(bytes)) };
        // SAFETY: opened above, closed once.
        unsafe {
            let _ = RegCloseKey(key);
        }
        set == ERROR_SUCCESS
    }

    fn disarm(&self) {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::ERROR_SUCCESS;
        use windows::Win32::System::Registry::{RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE};
        let wide = |t: &str| t.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (key_name, value_name) = (wide(&self.key), wide(&self.value));
        let mut key = HKEY::default();
        // SAFETY: NUL-terminated names and a valid out parameter.
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()), None, KEY_SET_VALUE, &mut key) } == ERROR_SUCCESS {
            // SAFETY: an open key and a NUL-terminated name; closed once.
            unsafe {
                let _ = RegDeleteValueW(key, PCWSTR(value_name.as_ptr()));
                let _ = RegCloseKey(key);
            }
        }
    }
}

#[cfg(not(windows))]
impl Armed for RunOnce {
    fn arm(&self) -> bool {
        false
    }
    fn disarm(&self) {}
}

/// The real journal, with `RunOnce` running `restorer`.
pub fn store_for_user(restorer: &Path) -> Store {
    Store::new(&crate::data_dir(), Box::new(RunOnce::for_user(restorer)))
}

/// Everything that can be restored without SteamVR running: programs' efficiency mode and, while SteamVR is closed, its file.
/// Programs are checked against a list of the system taken when this is made, before anything is opened.
pub struct Offline<'a> {
    os: &'a mut dyn ProcessOs,
    list: Vec<programs::Seen>,
    settings: Option<SettingsFile<'a>>,
}

impl<'a> Offline<'a> {
    pub fn new(os: &'a mut dyn ProcessOs, settings_path: Option<PathBuf>, closed: &'a dyn Fn() -> bool) -> Self {
        let list = os.list();
        Self { os, list, settings: settings_path.map(|path| SettingsFile { path, closed }) }
    }
}

impl Access for Offline<'_> {
    fn read(&mut self, target: &Target) -> Read {
        match target {
            Target::Process { pid, created, name } => programs::read_state(self.os, &self.list, *pid, *created, name),
            Target::SteamVr { .. } => self.settings.as_mut().map_or(Read::Unreachable, |s| s.read(target)),
        }
    }

    fn write(&mut self, target: &Target, value: &Value) -> bool {
        match (target, value) {
            (Target::Process { pid, created, name }, Value::Int(state)) => programs::set_state(self.os, &self.list, *pid, *created, name, *state, false),
            (Target::Process { .. }, _) => false,
            (Target::SteamVr { .. }, _) => self.settings.as_mut().is_some_and(|s| s.write(target, value)),
        }
    }
}

/// Runs recovery by what this computer offers now. A SteamVR value owed while SteamVR runs stays in the journal for the
/// overlay, which restores it live as the first thing it does.
pub fn recover_for_user(store: &Store) -> Result<Restored, Unreadable> {
    let closed = || !programs::steamvr_running(&programs::scan());
    let settings = vrsettings::settings_path_for_user();
    // What a write killed midway left beside SteamVR's file, once nothing can be writing it.
    if let Some(path) = settings.as_ref().filter(|_| closed()) {
        let mut temp = path.as_os_str().to_owned();
        temp.push(".orbiters-tmp");
        let _ = std::fs::remove_file(PathBuf::from(temp));
    }
    let mut os = RealOs;
    let mut access = Offline::new(&mut os, settings, &closed);
    store.restore(&mut access, &|_| true, &mut |_| {})
}

/// What is still owed that only a SteamVR closing can settle.
pub fn owed_to_steamvr(entries: &[Entry]) -> bool {
    entries.iter().any(|e| matches!(e.target, Target::SteamVr { .. }))
}

/// What the app or the restorer does at a sign-in: recovery, and arming the next try if something could not be done now.
/// Whether everything came back.
pub fn run_restorer(store: &Store) -> bool {
    let done = recover_for_user(store);
    // RunOnce was used up by running this.
    let _ = store.rearm();
    match done {
        Ok(restored) => restored.waiting.is_empty(),
        Err(Unreadable) => false,
    }
}

/// What an uninstall does about what is still owed, once the app has closed. Everything possible comes back at once. A SteamVR
/// value owed while SteamVR runs can only wait: `wait` is asked each moment whether to go on (the window says "Close SteamVR
/// to finish putting your settings back"), and a silent uninstall does not wait at all. Whether nothing is left owed. Never a
/// reason for the uninstall to fail: what is left stays in the journal, for the next sign-in.
pub fn settle_for_uninstall(
    store: &Store,
    os: &mut dyn ProcessOs,
    settings_path: Option<PathBuf>,
    closed: &dyn Fn() -> bool,
    silent: bool,
    wait: &mut dyn FnMut() -> bool,
) -> bool {
    let mut attempt = |store: &Store| {
        let mut access = Offline::new(&mut *os, settings_path.clone(), closed);
        let _ = store.restore(&mut access, &|_| true, &mut |_| {});
        store.entries().is_ok_and(|e| e.is_empty())
    };
    let mut done = attempt(store);
    if !done && !silent && !closed() && store.entries().is_ok_and(|e| owed_to_steamvr(&e)) {
        while !done && wait() {
            if closed() {
                done = attempt(store);
                break;
            }
        }
    }
    done
}

/// A folder for the copy of the restorer that outlives the app's install folder, when an uninstall leaves something owed.
pub fn restorer_copy_path() -> PathBuf {
    crate::data_dir().join("orbiters-booster-restore.exe")
}

/// Keeps a copy of `program` where the uninstall does not take it, and points `RunOnce` at it: what an uninstall that could
/// not finish restoring leaves behind. It never fails an uninstall, so it says only whether it worked.
pub fn leave_a_restorer(program: &Path) -> Option<PathBuf> {
    let to = restorer_copy_path();
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    std::fs::copy(program, &to).ok()?;
    RunOnce::for_user(&to).arm().then_some(to)
}

/// A restorer's copy, once nothing is owed.
pub fn remove_restorer_copy() {
    let _ = std::fs::remove_file(restorer_copy_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::tests::{setting, FileAccess, Scratch};
    use crate::journal::Change;

    /// A key under the test's own name in the current user's registry, gone at the end. Never `RunOnce`.
    #[cfg(windows)]
    struct ScratchKey(String);

    #[cfg(windows)]
    impl ScratchKey {
        fn new(name: &str) -> Self {
            Self(format!("Software\\OrbitersBoosterTest-{}-{name}", std::process::id()))
        }

        fn read(&self, value: &str) -> Option<String> {
            use windows::core::{PCWSTR, PWSTR};
            use windows::Win32::Foundation::ERROR_SUCCESS;
            use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
            let wide = |t: &str| t.encode_utf16().chain([0]).collect::<Vec<u16>>();
            let (key_name, value_name) = (wide(&self.0), wide(value));
            let mut key = HKEY::default();
            // SAFETY: NUL-terminated names and a valid out parameter.
            if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_name.as_ptr()), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
                return None;
            }
            let mut text = vec![0u16; 1024];
            let mut size = (text.len() * 2) as u32;
            // SAFETY: the buffer's size in bytes is what is passed.
            let status = unsafe { RegQueryValueExW(key, PCWSTR(value_name.as_ptr()), None, None, Some(PWSTR(text.as_mut_ptr()).0.cast()), Some(&mut size)) };
            // SAFETY: opened above, closed once.
            unsafe {
                let _ = RegCloseKey(key);
            }
            (status == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&text[..(size as usize / 2).saturating_sub(1)]))
        }
    }

    #[cfg(windows)]
    impl Drop for ScratchKey {
        fn drop(&mut self) {
            use windows::core::PCWSTR;
            use windows::Win32::System::Registry::{RegDeleteKeyW, HKEY_CURRENT_USER};
            let name: Vec<u16> = self.0.encode_utf16().chain([0]).collect();
            // SAFETY: a NUL-terminated name.
            unsafe {
                let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()));
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn run_once_is_set_while_the_journal_has_entries_and_gone_after() {
        let scratch_key = ScratchKey::new("armed");
        let dir = Scratch::new("run-once");
        let restorer = dir.0.join("restorer.exe");
        let store = Store::new(&dir.0, Box::new(RunOnce::in_key(&scratch_key.0, RUN_ONCE_VALUE, &restorer)));
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        assert_eq!(scratch_key.read(RUN_ONCE_VALUE), None);
        store.apply(&mut access, &[Change { target: setting("scale"), value: Value::Float(1.0), leave_with: None }], &mut |_| {}).unwrap();
        let command = scratch_key.read(RUN_ONCE_VALUE).expect("armed before the change");
        assert!(command.contains("restorer.exe") && command.ends_with(RESTORE_ARGUMENT), "{command}");
        store.restore(&mut access, &|_| true, &mut |_| {}).unwrap();
        assert_eq!(scratch_key.read(RUN_ONCE_VALUE), None, "gone once nothing is owed");
    }

    /// Used up by running, as Windows does, with something still owed: the restorer arms it again.
    #[cfg(windows)]
    #[test]
    fn what_could_not_be_restored_arms_the_next_try() {
        let scratch_key = ScratchKey::new("again");
        let dir = Scratch::new("run-once-again");
        let armed = RunOnce::in_key(&scratch_key.0, RUN_ONCE_VALUE, &dir.0.join("restorer.exe"));
        let store = Store::new(&dir.0, Box::new(RunOnce::in_key(&scratch_key.0, RUN_ONCE_VALUE, &dir.0.join("restorer.exe"))));
        let mut access = FileAccess::new(dir.0.join("settings.json"));
        store.apply(&mut access, &[Change { target: setting("scale"), value: Value::Float(1.0), leave_with: None }], &mut |_| {}).unwrap();
        armed.disarm();
        assert_eq!(store.rearm(), Ok(1));
        assert!(scratch_key.read(RUN_ONCE_VALUE).is_some());
    }

    /// The three ways an uninstall meets a value still owed.
    #[test]
    fn an_uninstall_with_steamvr_closed_open_or_silent() {
        struct Nothing;
        impl ProcessOs for Nothing {
            fn list(&mut self) -> Vec<programs::Seen> {
                Vec::new()
            }
            fn open(&mut self, _: u32) -> Option<programs::Opened> {
                None
            }
            fn set_state(&mut self, _: u32, _: u64, _: &str, _: i64) -> bool {
                false
            }
        }
        let owed = |dir: &Path| {
            let path = dir.join("steamvr.vrsettings");
            std::fs::write(&path, "{ \"steamvr\" : { \"supersampleScale\" : 1.4 } }").unwrap();
            let store = Store::new(dir, Box::new(crate::journal::NotArmed));
            let closed = || true;
            store
                .apply(
                    &mut Offline::new(&mut Nothing, Some(path.clone()), &closed),
                    &[Change { target: setting("supersampleScale"), value: Value::Float(1.19), leave_with: None }],
                    &mut |_| {},
                )
                .unwrap();
            (store, path)
        };
        let scale = |path: &Path| vrsettings::read_value(&std::fs::read_to_string(path).unwrap(), "steamvr", "supersampleScale").unwrap();
        // Closed: back at once, without waiting.
        let dir = Scratch::new("uninstall-closed");
        let (store, path) = owed(&dir.0);
        let closed = || true;
        assert!(settle_for_uninstall(&store, &mut Nothing, Some(path.clone()), &closed, false, &mut || panic!("no need to wait")));
        assert_eq!(scale(&path), Value::Float(1.4));
        assert!(store.remove_if_empty());
        // Open, and the person closes it while the window waits.
        let dir = Scratch::new("uninstall-open");
        let (store, path) = owed(&dir.0);
        let steamvr_closed = std::cell::Cell::new(false);
        let is_closed = || steamvr_closed.get();
        let mut asked = 0;
        assert!(settle_for_uninstall(&store, &mut Nothing, Some(path.clone()), &is_closed, false, &mut || {
            asked += 1;
            steamvr_closed.set(asked == 3);
            true
        }));
        assert_eq!(asked, 3);
        assert_eq!(scale(&path), Value::Float(1.4));
        // Open, and nobody closes it: the journal and the file stay as they are, and it does not fail.
        let dir = Scratch::new("uninstall-gave-up");
        let (store, path) = owed(&dir.0);
        let open = || false;
        assert!(!settle_for_uninstall(&store, &mut Nothing, Some(path.clone()), &open, false, &mut || false));
        assert!(store.has_entries());
        assert!(!store.remove_if_empty(), "an uninstall removing data still leaves what is owed");
        assert_eq!(scale(&path), Value::Float(1.19));
        // Silent, with SteamVR open: it does not wait at all.
        let dir = Scratch::new("uninstall-silent");
        let (store, path) = owed(&dir.0);
        assert!(!settle_for_uninstall(&store, &mut Nothing, Some(path), &open, true, &mut || panic!("a silent uninstall does not wait")));
        assert!(store.has_entries());
    }

    #[test]
    #[should_panic(expected = "tests never name")]
    fn a_test_cannot_name_the_real_key() {
        let _ = RunOnce::for_user(Path::new("x"));
    }

    /// A power loss, as far as a test can have one: the process and the program it lowered are gone, and only the files are
    /// left. The priority's entry goes, as its process is no more; the setting comes back through the file, only with SteamVR
    /// closed.
    #[test]
    fn after_a_power_loss_the_file_is_put_right_and_dead_processes_forgotten() {
        let dir = Scratch::new("power");
        let path = dir.0.join("steamvr.vrsettings");
        std::fs::write(&path, "{\n   \"steamvr\" : {\n      \"supersampleScale\" : 1.4,\n      \"disableAsync\" : true\n   }\n}\n").unwrap();
        // What the journal said when the power went: a value applied, and a process lowered.
        let store = Store::new(&dir.0, Box::new(crate::journal::NotArmed));
        struct Nothing;
        impl ProcessOs for Nothing {
            fn list(&mut self) -> Vec<programs::Seen> {
                Vec::new()
            }
            fn open(&mut self, _: u32) -> Option<programs::Opened> {
                None
            }
            fn set_state(&mut self, _: u32, _: u64, _: &str, _: i64) -> bool {
                false
            }
        }
        let closed = || true;
        let open = || false;
        store
            .apply(
                &mut Offline::new(&mut Nothing, Some(path.clone()), &closed),
                &[Change { target: setting("supersampleScale"), value: Value::Float(1.19), leave_with: None }],
                &mut |_| {},
            )
            .unwrap();
        let mut entries = store.entries().unwrap();
        entries.push(Entry {
            target: Target::Process { pid: 1234, created: 99, name: "chrome".into() },
            original: Value::Int(programs::SYSTEM_DECIDES),
            written: Value::Int(programs::EFFICIENCY),
            stage: crate::journal::Stage::Applied,
            at: String::new(),
            app_version: String::new(),
            leave_with: None,
        });
        std::fs::write(store.path(), serde_json::json!({ "version": 1, "entries": entries }).to_string()).unwrap();
        // SteamVR already running again at sign-in: the process's entry goes, the setting waits.
        let got = store.restore(&mut Offline::new(&mut Nothing, Some(path.clone()), &open), &|_| true, &mut |_| {}).unwrap();
        assert_eq!((got.gone.len(), got.waiting.len()), (1, 1));
        assert!(std::fs::read_to_string(&path).unwrap().contains("1.19"));
        // And closed.
        let got = store.restore(&mut Offline::new(&mut Nothing, Some(path.clone()), &closed), &|_| true, &mut |_| {}).unwrap();
        assert_eq!(got.back.len(), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"supersampleScale\" : 1.4,") && text.contains("disableAsync"), "{text}");
        assert!(store.entries().unwrap().is_empty());
    }
}
