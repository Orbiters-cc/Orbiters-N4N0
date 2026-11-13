//! `IVRApplications_007`: the application manifests SteamVR knows, and the ones it starts with itself (its settings,
//! Startup / Shutdown, "Choose startup overlay apps"). Layout from Valve's public `openvr_capi.h`; the entries used:
//! 0 AddApplicationManifest, 1 RemoveApplicationManifest, 2 IsApplicationInstalled, 17 SetApplicationAutoLaunch,
//! 18 GetApplicationAutoLaunch.
//!
//! SteamVR is asked only while it runs: [`starts_with_steamvr`] and [`set_start_with_steamvr`] connect as a background
//! application, which SteamVR refuses when it is not running instead of starting it.

use crate::{entry, AppType, Client, Error};
use std::ffi::{c_char, CString};
use std::path::Path;

pub const APPLICATIONS_VERSION: &str = "IVRApplications_007";

/// `EVRApplicationError_AppKeyAlreadyExists`: a manifest with the same key is registered already.
const APP_KEY_ALREADY_EXISTS: i32 = 100;

/// What SteamVR is told about an application it may start with itself: a built-in application (not one of Steam's)
/// shown among its dashboard overlays, which SteamVR offers to start with itself.
pub struct Manifest<'a> {
    /// The application's key (`cc.orbiters.n4n0`).
    pub key: &'a str,
    /// Its program, an absolute path.
    pub binary: &'a Path,
    pub arguments: &'a str,
    pub name: &'a str,
    pub description: &'a str,
}

/// The program the manifest file at `path` names, when it reads as one.
pub fn manifest_binary(path: &Path) -> Option<std::path::PathBuf> {
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    json["applications"][0]["binary_path_windows"].as_str().map(std::path::PathBuf::from)
}

impl Manifest<'_> {
    /// The manifest file's content.
    pub fn json(&self) -> String {
        let manifest = serde_json::json!({
            "source": "builtin",
            "applications": [{
                "app_key": self.key,
                "launch_type": "binary",
                "binary_path_windows": self.binary.display().to_string(),
                "arguments": self.arguments,
                "is_dashboard_overlay": true,
                "strings": { "en_us": { "name": self.name, "description": self.description } },
            }],
        });
        serde_json::to_string_pretty(&manifest).expect("a manifest is JSON")
    }
}

/// A key or a path as the runtime takes it (UTF-8, NUL-terminated).
fn c_text(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no NUL left")
}

impl Client {
    fn applications(&self) -> Result<*const usize, Error> {
        self.table(&self.applications_table, APPLICATIONS_VERSION)
    }

    /// Registers the manifest at `path` (absolute) for good: SteamVR keeps it in its settings. `Err` holds its
    /// `EVRApplicationError`.
    pub fn add_application_manifest(&self, path: &Path) -> Result<(), i32> {
        type F = unsafe extern "system" fn(*const c_char, bool) -> i32;
        let table = self.applications().map_err(|_| -1)?;
        let path = c_text(&path.display().to_string());
        // SAFETY: IVRApplications_007 entry 0 is AddApplicationManifest; the path is NUL-terminated.
        match unsafe { entry::<F>(table, 0)(path.as_ptr(), false) } {
            0 => Ok(()),
            code => Err(code),
        }
    }

    /// Unregisters the manifest at `path`.
    pub fn remove_application_manifest(&self, path: &Path) -> Result<(), i32> {
        type F = unsafe extern "system" fn(*const c_char) -> i32;
        let table = self.applications().map_err(|_| -1)?;
        let path = c_text(&path.display().to_string());
        // SAFETY: IVRApplications_007 entry 1 is RemoveApplicationManifest.
        match unsafe { entry::<F>(table, 1)(path.as_ptr()) } {
            0 => Ok(()),
            code => Err(code),
        }
    }

    /// Whether SteamVR knows an application by `key`.
    pub fn is_application_installed(&self, key: &str) -> bool {
        type F = unsafe extern "system" fn(*const c_char) -> bool;
        let Ok(table) = self.applications() else { return false };
        let key = c_text(key);
        // SAFETY: IVRApplications_007 entry 2 is IsApplicationInstalled.
        unsafe { entry::<F>(table, 2)(key.as_ptr()) }
    }

    /// Tells SteamVR this process is the application `key`, one whose manifest it knows. A background connection is
    /// otherwise known by its program's name (`system.generated.<program>`), with that name's own saved bindings and
    /// settings and none of the application's. `Err` holds its `EVRApplicationError`.
    pub fn identify_application(&self, key: &str) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u32, *const c_char) -> i32;
        let table = self.applications().map_err(|_| -1)?;
        let key = c_text(key);
        // SAFETY: IVRApplications_007 entry 11 is IdentifyApplication; the key is NUL-terminated.
        match unsafe { entry::<F>(table, 11)(std::process::id(), key.as_ptr()) } {
            0 => Ok(()),
            code => Err(code),
        }
    }

    /// The key SteamVR knows this process by, which is the section its saved bindings are under.
    pub fn own_application_key(&self) -> Option<String> {
        type F = unsafe extern "system" fn(u32, *mut c_char, u32) -> i32;
        let table = self.applications().ok()?;
        let mut text = [0 as c_char; 512];
        // SAFETY: IVRApplications_007 entry 5 is GetApplicationKeyByProcessId; the buffer's size is what is passed.
        if unsafe { entry::<F>(table, 5)(std::process::id(), text.as_mut_ptr(), text.len() as u32) } != 0 {
            return None;
        }
        // SAFETY: the buffer starts zeroed and its last byte is never written past by a call that succeeded.
        let key = unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }.to_string_lossy().into_owned();
        (!key.is_empty()).then_some(key)
    }

    /// Has SteamVR start the application `key` with itself, or not.
    pub fn set_application_auto_launch(&self, key: &str, on: bool) -> Result<(), i32> {
        type F = unsafe extern "system" fn(*const c_char, bool) -> i32;
        let table = self.applications().map_err(|_| -1)?;
        let key = c_text(key);
        // SAFETY: IVRApplications_007 entry 17 is SetApplicationAutoLaunch.
        match unsafe { entry::<F>(table, 17)(key.as_ptr(), on) } {
            0 => Ok(()),
            code => Err(code),
        }
    }

    /// Whether SteamVR starts the application `key` with itself.
    pub fn application_auto_launch(&self, key: &str) -> bool {
        type F = unsafe extern "system" fn(*const c_char) -> bool;
        let Ok(table) = self.applications() else { return false };
        let key = c_text(key);
        // SAFETY: IVRApplications_007 entry 18 is GetApplicationAutoLaunch.
        unsafe { entry::<F>(table, 18)(key.as_ptr()) }
    }

    /// Whether SteamVR starts the application `key` with itself (it knows it and starts it).
    pub fn starts_with_steamvr(&self, key: &str) -> bool {
        self.is_application_installed(key) && self.application_auto_launch(key)
    }

    /// SteamVR starts the application with itself (`on`: its manifest written at `path`, registered, set to start), or
    /// no longer does (it stays among SteamVR's startup overlay apps, switched off there too).
    pub fn set_start_with_steamvr(&self, manifest: &Manifest, path: &Path, on: bool) -> Result<(), String> {
        if !on {
            if self.is_application_installed(manifest.key) {
                self.set_application_auto_launch(manifest.key, false).map_err(|code| orbiters_i18n::tr!("SteamVR did not switch off its start (error {code})", code = code))?;
            }
            return Ok(());
        }
        let json = manifest.json();
        if std::fs::read_to_string(path).ok().as_deref() != Some(json.as_str()) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::write(path, &json).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        match self.add_application_manifest(path) {
            Ok(()) | Err(APP_KEY_ALREADY_EXISTS) => {}
            Err(code) => return Err(orbiters_i18n::tr!("SteamVR did not take the application's manifest (error {code})", code = code)),
        }
        self.set_application_auto_launch(manifest.key, true).map_err(|code| orbiters_i18n::tr!("SteamVR did not set its start (error {code})", code = code))
    }

    /// SteamVR knows the application by the manifest written at `path`, and starts it or not as it did. `Err` holds its
    /// `EVRApplicationError`, or -2 when the file could not be written.
    pub fn register_manifest(&self, manifest: &Manifest, path: &Path) -> Result<(), i32> {
        let json = manifest.json();
        if std::fs::read_to_string(path).ok().as_deref() != Some(json.as_str()) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| -2)?;
            }
            std::fs::write(path, &json).map_err(|_| -2)?;
        }
        match self.add_application_manifest(path) {
            Ok(()) | Err(APP_KEY_ALREADY_EXISTS) => Ok(()),
            Err(code) => Err(code),
        }
    }

    /// SteamVR forgets the application: not started any more, its manifest unregistered and removed (uninstalling).
    pub fn forget_application(&self, key: &str, path: &Path) {
        if self.is_application_installed(key) {
            let _ = self.set_application_auto_launch(key, false);
        }
        let _ = self.remove_application_manifest(path);
        let _ = std::fs::remove_file(path);
    }
}

/// Whether SteamVR starts the application `key` with itself; `Ok(None)` when SteamVR is not running.
pub fn starts_with_steamvr(key: &str) -> Result<Option<bool>, Error> {
    match Client::connect(AppType::Background) {
        Ok(client) => Ok(Some(client.starts_with_steamvr(key))),
        Err(e) if e.not_running() => Ok(None),
        Err(e) => Err(e),
    }
}

/// [`Client::set_start_with_steamvr`] over a background connection. `Ok(false)`: SteamVR is not running, nothing was
/// done (it is never started for this).
pub fn set_start_with_steamvr(manifest: &Manifest, path: &Path, on: bool) -> Result<bool, String> {
    match Client::connect(AppType::Background) {
        Ok(client) => client.set_start_with_steamvr(manifest, path, on).map(|_| true),
        Err(e) if e.not_running() => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// [`Client::forget_application`] over a background connection, when SteamVR runs; else only the manifest goes (SteamVR
/// drops a manifest whose file is gone).
pub fn forget_application(key: &str, path: &Path) {
    match Client::connect(AppType::Background) {
        Ok(client) => client.forget_application(key, path),
        Err(_) => {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_describes_a_dashboard_overlay_that_steamvr_can_start() {
        let binary = Path::new(r"C:\Users\Someone\AppData\Local\Programs\Orbiters - N4N0\orbiters-desktop.exe");
        let manifest = Manifest { key: "cc.orbiters.n4n0", binary, arguments: "--hidden", name: "Orbiters - N4N0", description: "N4N0's menu" };
        let json: serde_json::Value = serde_json::from_str(&manifest.json()).unwrap();
        assert_eq!(json["source"], "builtin");
        let app = &json["applications"][0];
        assert_eq!(app["app_key"], "cc.orbiters.n4n0");
        assert_eq!(app["launch_type"], "binary");
        assert_eq!(app["binary_path_windows"], binary.display().to_string());
        assert_eq!(app["arguments"], "--hidden");
        assert_eq!(app["is_dashboard_overlay"], true);
        assert_eq!(app["strings"]["en_us"]["name"], "Orbiters - N4N0");
    }
}
