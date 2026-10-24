//! Default locations. Everything the user produces lives under `Documents/Orbiters/` (sessions under the home folder's
//! `Orbiters/` when OneDrive syncs Documents); settings under the platform config folder (`%APPDATA%/Orbiters/Desktop`
//! on Windows, `~/.config/orbiters-desktop` on Linux); logs and caches, which stay on this computer, under
//! `%LOCALAPPDATA%/Orbiters`.

use std::path::{Path, PathBuf};

/// A folder for when the system names none: never a relative path, which would land wherever the app was started
/// from (System32 for a program started at sign-in).
fn fallback_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// `Documents/Orbiters`.
pub fn documents_root() -> PathBuf {
    dirs::document_dir().unwrap_or_else(fallback_dir).join("Orbiters")
}

/// Whether OneDrive syncs `dir` (its Known Folder Move puts Documents inside the OneDrive folder).
fn synced_by_onedrive(dir: &Path) -> bool {
    let named = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|root| !root.is_empty() && dir.starts_with(Path::new(&root)));
    named || dir.components().any(|c| c.as_os_str().to_string_lossy().starts_with("OneDrive"))
}

/// Default folder for session bundles: `Documents/Orbiters/Sessions`, or `~/Orbiters/Sessions` when OneDrive syncs
/// Documents (gigabytes of captures, other players in them, would go to the cloud, and OneDrive holds files while it
/// uploads them, which fails the renames a recording makes).
pub fn default_sessions_dir() -> PathBuf {
    match (dirs::document_dir(), dirs::home_dir()) {
        (Some(documents), Some(home)) if synced_by_onedrive(&documents) && !synced_by_onedrive(&home) => home.join("Orbiters").join("Sessions"),
        _ => documents_root().join("Sessions"),
    }
}

/// Default folder where Mixer's capture simulator writes raw captures.
pub fn default_raw_captures_dir() -> PathBuf {
    documents_root().join("Raw captures")
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(fallback_dir);
    if cfg!(windows) {
        base.join("Orbiters").join("Desktop")
    } else {
        base.join("orbiters-desktop")
    }
}

/// Where Unity editors running Mixer take requests ("Send to Mixer"): `%LOCALAPPDATA%/Orbiters/Mixer/inbox`, one
/// folder per editor process.
pub fn mixer_inbox_dir() -> PathBuf {
    local_data_root().join("Mixer").join("inbox")
}

/// `%LOCALAPPDATA%/Orbiters` (`~/.local/share/Orbiters` on Linux): data shared by the Orbiters apps and tools on
/// this computer, too big or too local for the config folder.
pub fn local_data_root() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(fallback_dir).join("Orbiters")
}

/// What Orbiters - N4N0 can make again (what screenshots say, world pages, thumbnails): on this computer only, not
/// in the roaming config folder.
pub fn cache_dir() -> PathBuf {
    local_data_root().join("Desktop").join("cache")
}

/// Avatar manifests Orbiters Toolkit writes while the VRChat SDK builds an avatar (contracts `Docs/avatar-manifest.md`).
pub fn avatars_dir() -> PathBuf {
    local_data_root().join("Avatars")
}

/// Speech models N4N0 downloads when the person asks for them.
pub fn models_dir() -> PathBuf {
    local_data_root().join("Models")
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// `%LOCALAPPDATA%/Orbiters/logs`: what the processes of the app note when they panic ([`crate::panic_log`]).
pub fn logs_dir() -> PathBuf {
    local_data_root().join("logs")
}

/// `%USERPROFILE%/AppData/LocalLow` as Windows knows it (`FOLDERID_LocalAppDataLow`, which follows a moved profile),
/// read once.
#[cfg(windows)]
fn local_low() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::sync::OnceLock;
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    /// `FOLDERID_LocalAppDataLow`, {A520A1A4-1780-4FF6-BD18-167343C5AF16}.
    const LOCAL_APP_DATA_LOW: Guid = Guid { data1: 0xA520_A1A4, data2: 0x1780, data3: 0x4FF6, data4: [0xBD, 0x18, 0x16, 0x73, 0x43, 0xC5, 0xAF, 0x16] };
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: *mut core::ffi::c_void, path: *mut *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoTaskMemFree(memory: *const core::ffi::c_void);
    }
    static FOLDER: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let mut path: *mut u16 = std::ptr::null_mut();
            // SAFETY: the id is a static and the out-pointer ours; the string Windows allocates (also on failure, then
            // null) is read up to its terminating zero and freed once.
            unsafe {
                let found = SHGetKnownFolderPath(&LOCAL_APP_DATA_LOW, 0, std::ptr::null_mut(), &mut path) >= 0 && !path.is_null();
                let folder = found.then(|| {
                    let len = (0..).take_while(|&i| *path.add(i) != 0).count();
                    PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(path, len)))
                });
                CoTaskMemFree(path.cast());
                folder
            }
        })
        .clone()
        .or_else(|| dirs::home_dir().map(|home| home.join("AppData").join("LocalLow")))
}

/// VRChat's log folder on this machine, when it exists.
pub fn vrchat_log_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    candidates.extend(local_low().map(|low| low.join("VRChat").join("VRChat")));
    #[cfg(not(windows))]
    if let Some(home) = dirs::home_dir() {
        // Steam Proton prefix (default library and Flatpak Steam).
        for steam in [".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
            candidates.push(home.join(steam).join("steamapps/compatdata/438100/pfx/drive_c/users/steamuser/AppData/LocalLow/VRChat/VRChat"));
        }
    }
    candidates.into_iter().find(|p| p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_absolute_and_onedrive_is_known() {
        assert!(synced_by_onedrive(Path::new("C:/Users/x/OneDrive - Contoso/Documents")));
        assert!(synced_by_onedrive(Path::new("C:/Users/x/OneDrive/Documents")));
        assert!(!synced_by_onedrive(Path::new("C:/Users/nobody-here/Documents")));
        for dir in [config_dir(), documents_root(), default_sessions_dir(), local_data_root()] {
            assert!(dir.is_absolute(), "{}", dir.display());
        }
        assert!(logs_dir().starts_with(local_data_root()) && cache_dir().starts_with(local_data_root()));
        #[cfg(windows)]
        assert!(local_low().is_some_and(|low| low.ends_with("LocalLow")));
    }
}
