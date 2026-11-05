//! Installing the driver for SteamVR: a driver folder of its own,
//! `%LOCALAPPDATA%\Orbiters\SteamVR\drivers\orbiters_smooth` with `driver.vrdrivermanifest` and
//! `bin\win64\driver_orbiters_smooth.dll`, registered in the `external_drivers` of
//! `%LOCALAPPDATA%\openvr\openvrpaths.vrpath` (the list `vrpathreg adddriver` edits).
//!
//! SteamVR reads that list and loads drivers only when it starts, so the app registers the driver once and for all
//! (at its first start, SteamVR running or not) and never takes it out: loaded at every SteamVR start, the driver
//! does nothing until the app turns smoothing on, and every change after that is live. Only a first registration
//! made while SteamVR already runs waits for SteamVR's next start ([`Installation::registered_since`] tells when it
//! was made). Uninstalling takes the driver out. After a crash SteamVR starts in Safe Mode, with add-on drivers like
//! this one turned off until the person turns them on again (Settings, Startup / Shutdown, Manage Add-ons):
//! [`Installation::add_on_state`] reads that.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The driver's name in its manifest (and its DLL's, `driver_<name>.dll`).
pub const DRIVER_NAME: &str = "orbiters_smooth";
/// The DLL SteamVR loads, in `bin\win64` under the driver folder; also its name in the app's install folder.
pub const DLL_NAME: &str = "driver_orbiters_smooth.dll";
/// In the driver folder: when the driver was registered in SteamVR's list (Unix seconds).
const REGISTERED_FILE: &str = "registered";

/// What SteamVR's own settings say of the driver (`steamvr.vrsettings`, `driver_orbiters_smooth`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddOnState {
    /// Nothing against it: SteamVR loads it at its start.
    On,
    /// Turned off in SteamVR's Manage Add-ons.
    TurnedOff,
    /// Turned off by SteamVR's Safe Mode after a crash, until turned on again in Manage Add-ons.
    BlockedBySafeMode,
}

/// Where the driver goes and where SteamVR's driver list is. [`Installation::for_user`] is the real one; tests make
/// their own with [`Installation::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installation {
    /// The driver folder (`...\drivers\orbiters_smooth`).
    pub driver_dir: PathBuf,
    /// `openvrpaths.vrpath`.
    pub vrpath: PathBuf,
}

/// Installs the driver for this Windows user from `driver_dll` (the copy in the app's install folder); returns the
/// driver folder. See [`Installation::install`].
pub fn install(driver_dll: &Path) -> Result<PathBuf> {
    Installation::for_user()?.install(driver_dll)
}

/// Removes the driver for this Windows user. See [`Installation::uninstall`].
pub fn uninstall() -> Result<()> {
    Installation::for_user()?.uninstall()
}

/// Whether the driver is installed for this Windows user (registered, and its files in place).
pub fn installed() -> bool {
    Installation::for_user().map(|i| i.installed()).unwrap_or(false)
}

impl Installation {
    pub fn new(driver_dir: impl Into<PathBuf>, vrpath: impl Into<PathBuf>) -> Self {
        Self { driver_dir: driver_dir.into(), vrpath: vrpath.into() }
    }

    /// `%LOCALAPPDATA%\Orbiters\SteamVR\drivers\orbiters_smooth` and `%LOCALAPPDATA%\openvr\openvrpaths.vrpath`.
    pub fn for_user() -> Result<Self> {
        let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(PathBuf::from).context("LOCALAPPDATA is not set")?;
        Ok(Self::new(local.join("Orbiters").join("SteamVR").join("drivers").join(DRIVER_NAME), local.join("openvr").join("openvrpaths.vrpath")))
    }

    /// The installed DLL.
    pub fn dll_path(&self) -> PathBuf {
        self.driver_dir.join("bin").join("win64").join(DLL_NAME)
    }

    /// The installed manifest.
    pub fn manifest_path(&self) -> PathBuf {
        self.driver_dir.join("driver.vrdrivermanifest")
    }

    /// Registered in SteamVR's list, with the manifest and the DLL in place.
    pub fn installed(&self) -> bool {
        self.dll_path().is_file() && self.manifest_path().is_file() && self.registered().unwrap_or(false)
    }

    /// Whether the driver folder is in `external_drivers`.
    pub fn registered(&self) -> Result<bool> {
        let vrpath = read_vrpath(&self.vrpath)?;
        Ok(external_drivers(&vrpath).iter().any(|d| same_path(d, &self.driver_dir)))
    }

    /// When the driver was registered in SteamVR's list (a SteamVR started before then has not loaded it): the time
    /// [`Installation::install`] added it, or, for a registration older than that record, its manifest's.
    pub fn registered_since(&self) -> Option<SystemTime> {
        if !self.registered().unwrap_or(false) {
            return None;
        }
        let recorded = std::fs::read_to_string(self.driver_dir.join(REGISTERED_FILE)).ok().and_then(|t| t.trim().parse::<u64>().ok());
        match recorded {
            Some(secs) => UNIX_EPOCH.checked_add(Duration::from_secs(secs)),
            None => std::fs::metadata(self.manifest_path()).and_then(|m| m.modified()).ok(),
        }
    }

    /// What SteamVR's settings say of the driver: turned off in Manage Add-ons, or by Safe Mode after a crash. A
    /// settings file that cannot be read says nothing against it.
    pub fn add_on_state(&self) -> AddOnState {
        let settings = read_vrpath(&self.vrpath).ok().and_then(|vrpath| {
            let config = vrpath.get("config")?.as_array()?.iter().find_map(Value::as_str)?.to_owned();
            let text = std::fs::read_to_string(Path::new(&config).join("steamvr.vrsettings")).ok()?;
            serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok()
        });
        add_on_state_in(settings.as_ref())
    }

    /// Writes the driver folder (the manifest, and the DLL copied from `driver_dll`) and registers it in
    /// `openvrpaths.vrpath`, keeping everything else there; again is harmless (nothing rewritten that is the same,
    /// never a second entry). A DLL in use by a running SteamVR is moved aside, so the new one loads at its next
    /// start. Fails, changing nothing, when SteamVR has never run here (no `openvrpaths.vrpath`).
    pub fn install(&self, driver_dll: &Path) -> Result<PathBuf> {
        let mut vrpath = read_vrpath(&self.vrpath)?;
        let dll = std::fs::read(driver_dll).with_context(|| format!("could not read the driver {}", driver_dll.display()))?;

        let bin = self.dll_path().parent().map(Path::to_path_buf).context("the driver folder has no parent")?;
        std::fs::create_dir_all(&bin).with_context(|| format!("could not create {}", bin.display()))?;
        write_if_changed(&self.manifest_path(), manifest().as_bytes())?;
        replace_dll(&self.dll_path(), &dll)?;

        let dir = self.driver_dir.to_string_lossy().into_owned();
        let drivers = external_drivers(&vrpath);
        let ours = drivers.iter().filter(|d| same_path(d, &self.driver_dir)).count();
        if ours != 1 {
            let mut kept: Vec<Value> = drivers.into_iter().filter(|d| !same_path(d, &self.driver_dir)).map(Value::String).collect();
            kept.push(Value::String(dir));
            set_external_drivers(&mut vrpath, kept)?;
            write_vrpath(&self.vrpath, &vrpath)?;
            if ours == 0 {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let _ = std::fs::write(self.driver_dir.join(REGISTERED_FILE), now.to_string());
            }
        }
        Ok(self.driver_dir.clone())
    }

    /// Unregisters the driver and removes its folder. When SteamVR runs with the driver loaded, its DLL cannot be
    /// deleted: it is moved into the temporary folder (Windows lets a loaded DLL be renamed), so the folder still
    /// goes; if even that fails the error says so, and SteamVR no longer loads the driver from its next start all the
    /// same.
    pub fn uninstall(&self) -> Result<()> {
        if self.vrpath.is_file() {
            let mut vrpath = read_vrpath(&self.vrpath)?;
            let drivers = external_drivers(&vrpath);
            if drivers.iter().any(|d| same_path(d, &self.driver_dir)) {
                let kept: Vec<Value> = drivers.into_iter().filter(|d| !same_path(d, &self.driver_dir)).map(Value::String).collect();
                set_external_drivers(&mut vrpath, kept)?;
                write_vrpath(&self.vrpath, &vrpath)?;
            }
        }
        if self.driver_dir.exists() && std::fs::remove_dir_all(&self.driver_dir).is_err() {
            move_out_files_in_use(&self.driver_dir);
            std::fs::remove_dir_all(&self.driver_dir).with_context(|| {
                format!(
                    "SteamVR no longer loads the smoothing driver, but its folder {} could not be removed (SteamVR may still be using it; close SteamVR and try again)",
                    self.driver_dir.display()
                )
            })?;
        }
        // The empty folders above it (`SteamVR\drivers`, `SteamVR`), nothing else.
        let mut parent = self.driver_dir.parent();
        for _ in 0..2 {
            match parent {
                Some(p) if std::fs::remove_dir(p).is_ok() => parent = p.parent(),
                _ => break,
            }
        }
        Ok(())
    }
}

/// What `steamvr.vrsettings` (parsed, if it could be) says of the driver.
fn add_on_state_in(settings: Option<&Value>) -> AddOnState {
    let Some(section) = settings.and_then(|s| s.get(format!("driver_{DRIVER_NAME}"))) else { return AddOnState::On };
    if section.get("blocked_by_safe_mode").and_then(Value::as_bool) == Some(true) {
        AddOnState::BlockedBySafeMode
    } else if section.get("enable").and_then(Value::as_bool) == Some(false) {
        AddOnState::TurnedOff
    } else {
        AddOnState::On
    }
}

/// Moves the files left in `dir` (a DLL a running SteamVR holds) into the temporary folder, each under a name of its
/// own, so that the folder can go. Best effort: a file on another drive than the temporary folder stays.
fn move_out_files_in_use(dir: &Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::remove_file(&path).is_err() {
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = std::fs::rename(&path, std::env::temp_dir().join(format!("orbiters-smooth-{stamp}-{name}.old")));
            }
        }
    }
}

/// The manifest, like OpenVR-SpaceCalibrator's: loaded whatever headset is in use, never only for resources.
fn manifest() -> String {
    let value = serde_json::json!({
        "alwaysActivate": true,
        "name": DRIVER_NAME,
        "directory": "",
        "resourceOnly": false,
        "hmd_presence": [],
    });
    to_text(&value)
}

fn read_vrpath(path: &Path) -> Result<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("SteamVR is not set up for this Windows user ({} is missing): start SteamVR once, then try again", path.display())
        }
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("{} is not valid JSON", path.display()))?;
    if !value.is_object() {
        bail!("{} is not a JSON object", path.display());
    }
    Ok(value)
}

/// `external_drivers`' paths (absent or `null` is none).
fn external_drivers(vrpath: &Value) -> Vec<String> {
    vrpath.get("external_drivers").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// Sets `external_drivers`, as `vrpathreg` does: `null` when empty.
fn set_external_drivers(vrpath: &mut Value, drivers: Vec<Value>) -> Result<()> {
    let object = vrpath.as_object_mut().context("openvrpaths.vrpath is not a JSON object")?;
    object.insert("external_drivers".into(), if drivers.is_empty() { Value::Null } else { Value::Array(drivers) });
    Ok(())
}

/// Tab-indented, as SteamVR writes it. JSON strings escape their line breaks, so every line's leading spaces are
/// indentation (two per level in `to_string_pretty`).
fn to_text(value: &Value) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("a JSON value always serializes");
    let mut out = String::with_capacity(pretty.len() + 1);
    for line in pretty.lines() {
        let body = line.trim_start_matches(' ');
        for _ in 0..(line.len() - body.len()) / 2 {
            out.push('\t');
        }
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// Writes through a temporary file beside it, so SteamVR never reads half a file.
fn write_vrpath(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("vrpath.orbiters-tmp");
    std::fs::write(&tmp, to_text(value)).with_context(|| format!("could not write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    std::fs::write(path, bytes).with_context(|| format!("could not write {}", path.display()))
}

/// Puts `bytes` at `path`. A loaded DLL cannot be overwritten on Windows but can be renamed: the old one is moved
/// aside (`.old-<n>`, removed by a later install once SteamVR has let go of it).
fn replace_dll(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("the DLL has no folder")?;
    let name = path.file_name().context("the DLL has no name")?.to_string_lossy().into_owned();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&format!("{name}.old-")) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let new = dir.join(format!("{name}.new"));
    std::fs::write(&new, bytes).with_context(|| format!("could not write {}", new.display()))?;
    if std::fs::rename(&new, path).is_ok() {
        return Ok(());
    }
    let aside = (0..100).map(|n| dir.join(format!("{name}.old-{n}"))).find(|p| !p.exists()).context("too many old copies of the driver")?;
    let moved = std::fs::rename(path, &aside).and_then(|()| std::fs::rename(&new, path));
    moved.map_err(|e| {
        let _ = std::fs::remove_file(&new);
        anyhow::Error::new(e).context(format!("could not replace {}", path.display()))
    })
}

/// Whether two paths name the same folder, as Windows compares them: case and separators aside, no trailing slash.
fn same_path(a: &str, b: &Path) -> bool {
    let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Temp(PathBuf);

    impl Temp {
        fn new(what: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!("orbiters-smooth-test-{what}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const VRPATH: &str = r#"{
	"config" : [ "C:\\Program Files (x86)\\Steam\\config" ],
	"external_drivers" : null,
	"jsonid" : "vrpathreg",
	"log" : [ "C:\\Program Files (x86)\\Steam\\logs" ],
	"runtime" : [ "C:\\Program Files (x86)\\Steam\\steamapps\\common\\SteamVR" ],
	"some_future_key" : { "nested" : [ 1, 2.5, true ] },
	"version" : 1
}"#;

    fn setup(what: &str, vrpath: Option<&str>) -> (Temp, Installation, PathBuf) {
        let temp = Temp::new(what);
        let openvr = temp.0.join("openvr");
        std::fs::create_dir_all(&openvr).unwrap();
        if let Some(text) = vrpath {
            std::fs::write(openvr.join("openvrpaths.vrpath"), text).unwrap();
        }
        let app = temp.0.join("app");
        std::fs::create_dir_all(&app).unwrap();
        let dll = app.join(DLL_NAME);
        std::fs::write(&dll, b"MZ first build").unwrap();
        let install = Installation::new(temp.0.join("Orbiters").join("SteamVR").join("drivers").join(DRIVER_NAME), openvr.join("openvrpaths.vrpath"));
        (temp, install, dll)
    }

    fn vrpath_json(install: &Installation) -> Value {
        serde_json::from_str(&std::fs::read_to_string(&install.vrpath).unwrap()).unwrap()
    }

    #[test]
    fn install_registers_once_and_keeps_the_rest() {
        let (_temp, install, dll) = setup("install", Some(VRPATH));
        assert!(!install.installed());
        let dir = install.install(&dll).unwrap();
        assert_eq!(dir, install.driver_dir);
        assert!(install.installed());
        assert_eq!(std::fs::read(install.dll_path()).unwrap(), b"MZ first build");

        let manifest: Value = serde_json::from_str(&std::fs::read_to_string(install.manifest_path()).unwrap()).unwrap();
        assert_eq!(manifest["alwaysActivate"], true);
        assert_eq!(manifest["name"], DRIVER_NAME);
        assert_eq!(manifest["resourceOnly"], false);
        assert_eq!(manifest["hmd_presence"], serde_json::json!([]));

        let before: Value = serde_json::from_str(VRPATH).unwrap();
        let after = vrpath_json(&install);
        assert_eq!(after["external_drivers"], serde_json::json!([install.driver_dir.to_string_lossy()]));
        for key in ["config", "jsonid", "log", "runtime", "some_future_key", "version"] {
            assert_eq!(after[key], before[key], "{key} changed");
        }
        assert_eq!(after.as_object().unwrap().len(), before.as_object().unwrap().len());

        // Again: same file, same single entry.
        let text = std::fs::read_to_string(&install.vrpath).unwrap();
        install.install(&dll).unwrap();
        assert_eq!(std::fs::read_to_string(&install.vrpath).unwrap(), text);
    }

    #[test]
    fn existing_entries_in_another_spelling_are_not_doubled() {
        let (_temp, install, dll) = setup("spelling", Some(VRPATH));
        let shouting = install.driver_dir.to_string_lossy().to_uppercase().replace('\\', "/") + "/";
        let other = r"D:\Tools\SpaceCalibrator\01spacecalibrator";
        let mut vrpath: Value = serde_json::from_str(VRPATH).unwrap();
        vrpath["external_drivers"] = serde_json::json!([other, shouting, shouting]);
        std::fs::write(&install.vrpath, vrpath.to_string()).unwrap();
        assert!(install.registered().unwrap());

        install.install(&dll).unwrap();
        let drivers = vrpath_json(&install)["external_drivers"].clone();
        assert_eq!(drivers, serde_json::json!([other, install.driver_dir.to_string_lossy()]), "duplicates collapse to one, others stay");

        install.uninstall().unwrap();
        assert_eq!(vrpath_json(&install)["external_drivers"], serde_json::json!([other]));
        assert!(!install.driver_dir.exists());
        assert!(!install.installed());
    }

    #[test]
    fn uninstall_removes_the_entry_and_the_folders() {
        let (temp, install, dll) = setup("uninstall", Some(VRPATH));
        install.install(&dll).unwrap();
        install.uninstall().unwrap();
        assert_eq!(vrpath_json(&install)["external_drivers"], Value::Null);
        assert!(!install.driver_dir.exists());
        assert!(!temp.0.join("Orbiters").join("SteamVR").exists(), "the empty SteamVR folder goes too");
        assert!(temp.0.join("Orbiters").exists(), "but not the Orbiters folder");
        // Nothing installed: nothing to do.
        install.uninstall().unwrap();
    }

    #[test]
    fn a_new_build_replaces_the_dll() {
        let (_temp, install, dll) = setup("update", Some(VRPATH));
        install.install(&dll).unwrap();
        std::fs::write(&dll, b"MZ second build").unwrap();
        install.install(&dll).unwrap();
        assert_eq!(std::fs::read(install.dll_path()).unwrap(), b"MZ second build");
        let bin = install.dll_path().parent().unwrap().to_path_buf();
        let names: Vec<String> = std::fs::read_dir(bin).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![DLL_NAME.to_string()]);
    }

    #[cfg(windows)]
    #[test]
    fn a_dll_in_use_is_moved_aside() {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
            fn FreeLibrary(module: isize) -> i32;
        }
        const LOAD_LIBRARY_AS_IMAGE_RESOURCE: u32 = 0x20;

        // A real image, mapped the way SteamVR's loaded driver is (as an image section, none of its code run): this
        // test's own executable.
        let (_temp, install, dll) = setup("in-use", Some(VRPATH));
        std::fs::copy(std::env::current_exe().unwrap(), &dll).unwrap();
        install.install(&dll).unwrap();
        let held = install.dll_path();
        let wide: Vec<u16> = held.to_string_lossy().encode_utf16().chain(Some(0)).collect();
        // SAFETY: mapped as an image resource only, freed below.
        let module = unsafe { LoadLibraryExW(wide.as_ptr(), 0, LOAD_LIBRARY_AS_IMAGE_RESOURCE) };
        assert_ne!(module, 0, "could not map the stand-in DLL");
        assert!(std::fs::write(&held, b"overwritten").is_err(), "a mapped image cannot be overwritten");

        std::fs::write(&dll, b"MZ next build").unwrap();
        let result = install.install(&dll);
        let aside = held.with_file_name(format!("{DLL_NAME}.old-0"));
        let aside_existed = aside.exists();
        // SAFETY: the module mapped above (FreeLibrary takes the tagged handle of a resource mapping as it is).
        unsafe { FreeLibrary(module) };
        result.unwrap();
        assert_eq!(std::fs::read(&held).unwrap(), b"MZ next build");
        assert!(aside_existed, "the image in use was moved aside");

        // The next install tidies the old copy once nothing holds it.
        install.install(&dll).unwrap();
        assert!(!aside.exists());
    }

    #[test]
    fn the_registration_time_is_kept() {
        let (_temp, install, dll) = setup("registered-since", Some(VRPATH));
        assert_eq!(install.registered_since(), None, "not registered");
        let before = SystemTime::now() - Duration::from_secs(2);
        install.install(&dll).unwrap();
        let since = install.registered_since().expect("registered");
        assert!(since >= before && since <= SystemTime::now() + Duration::from_secs(2), "{since:?}");
        // Installing again (each start of the app) keeps the first registration's time: SteamVR started since loads it.
        std::fs::write(install.driver_dir.join(REGISTERED_FILE), "1000").unwrap();
        install.install(&dll).unwrap();
        assert_eq!(install.registered_since(), UNIX_EPOCH.checked_add(Duration::from_secs(1000)));
        // An older registration without the record: the manifest's time stands for it.
        std::fs::remove_file(install.driver_dir.join(REGISTERED_FILE)).unwrap();
        assert!(install.registered_since().is_some());
        install.uninstall().unwrap();
        assert_eq!(install.registered_since(), None);
    }

    #[test]
    fn steamvr_add_on_states() {
        let parse = |t: &str| serde_json::from_str::<Value>(t).unwrap();
        assert_eq!(add_on_state_in(None), AddOnState::On);
        assert_eq!(add_on_state_in(Some(&parse(r#"{ "steamvr": {} }"#))), AddOnState::On);
        assert_eq!(add_on_state_in(Some(&parse(r#"{ "driver_orbiters_smooth": { "enable": false } }"#))), AddOnState::TurnedOff);
        assert_eq!(add_on_state_in(Some(&parse(r#"{ "driver_orbiters_smooth": { "enable": true } }"#))), AddOnState::On);
        assert_eq!(add_on_state_in(Some(&parse(r#"{ "driver_orbiters_smooth": { "blocked_by_safe_mode": true } }"#))), AddOnState::BlockedBySafeMode);
        assert_eq!(add_on_state_in(Some(&parse(r#"{ "driver_standable": { "blocked_by_safe_mode": true } }"#))), AddOnState::On, "another driver's");

        // Read from SteamVR's config folder, which openvrpaths.vrpath names.
        let (temp, install, _dll) = setup("add-on", None);
        let config = temp.0.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let vrpath = serde_json::json!({ "config": [config.to_string_lossy()], "external_drivers": null, "version": 1 });
        std::fs::write(&install.vrpath, vrpath.to_string()).unwrap();
        assert_eq!(install.add_on_state(), AddOnState::On, "no settings file yet");
        std::fs::write(config.join("steamvr.vrsettings"), "\u{feff}{ \"driver_orbiters_smooth\" : { \"blocked_by_safe_mode\" : true } }").unwrap();
        assert_eq!(install.add_on_state(), AddOnState::BlockedBySafeMode);
    }

    #[cfg(windows)]
    #[test]
    fn uninstall_with_steamvr_holding_the_dll_still_removes_it() {
        #[link(name = "kernel32")]
        extern "system" {
            fn LoadLibraryExW(name: *const u16, file: isize, flags: u32) -> isize;
            fn FreeLibrary(module: isize) -> i32;
        }
        const LOAD_LIBRARY_AS_IMAGE_RESOURCE: u32 = 0x20;
        let (temp, install, dll) = setup("uninstall-in-use", Some(VRPATH));
        std::fs::copy(std::env::current_exe().unwrap(), &dll).unwrap();
        install.install(&dll).unwrap();
        let wide: Vec<u16> = install.dll_path().to_string_lossy().encode_utf16().chain(Some(0)).collect();
        // SAFETY: mapped as an image resource only (as SteamVR's loaded driver holds it), freed below.
        let module = unsafe { LoadLibraryExW(wide.as_ptr(), 0, LOAD_LIBRARY_AS_IMAGE_RESOURCE) };
        assert_ne!(module, 0, "could not map the stand-in DLL");
        let result = install.uninstall();
        let folder_gone = !install.driver_dir.exists();
        // SAFETY: the module mapped above.
        unsafe { FreeLibrary(module) };
        // The copy moved out of the way, left for the temporary folder's clean-up, goes now.
        for entry in std::fs::read_dir(std::env::temp_dir()).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("orbiters-smooth-") && name.ends_with(&format!("{DLL_NAME}.old")) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        result.unwrap();
        assert!(folder_gone, "the DLL in use was moved out and the folder removed");
        assert_eq!(vrpath_json(&install)["external_drivers"], Value::Null, "and SteamVR's list no longer has it");
        assert!(!temp.0.join("Orbiters").join("SteamVR").exists());
    }

    #[test]
    fn without_steamvr_nothing_is_written() {
        let (_temp, install, dll) = setup("no-steamvr", None);
        let error = install.install(&dll).unwrap_err().to_string();
        assert!(error.contains("start SteamVR once"), "{error}");
        assert!(!install.driver_dir.exists());
        assert!(!install.vrpath.exists());
        assert!(!install.installed());
        install.uninstall().unwrap();
    }

    #[test]
    fn a_broken_vrpath_is_left_alone() {
        let (_temp, install, dll) = setup("broken", Some("{ not json"));
        assert!(install.install(&dll).is_err());
        assert_eq!(std::fs::read_to_string(&install.vrpath).unwrap(), "{ not json");
        assert!(!install.driver_dir.exists());
    }

    #[test]
    fn a_bom_and_missing_key_are_fine() {
        let (_temp, install, dll) = setup("bom", Some("\u{feff}{ \"jsonid\" : \"vrpathreg\", \"version\" : 1 }"));
        install.install(&dll).unwrap();
        let after = vrpath_json(&install);
        assert_eq!(after["jsonid"], "vrpathreg");
        assert_eq!(after["external_drivers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn paths_compare_like_windows() {
        assert!(same_path(r"C:\Users\A\Drivers\X\", Path::new(r"c:/users/a/drivers/x")));
        assert!(!same_path(r"C:\Users\A\Drivers\X2", Path::new(r"C:\Users\A\Drivers\X")));
    }
}
