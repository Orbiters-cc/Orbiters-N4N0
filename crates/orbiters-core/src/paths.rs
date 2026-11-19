//! Default locations. Everything the user produces lives under `Documents/Orbiters/` (sessions under the home folder's
//! `Orbiters/` when OneDrive syncs Documents, and all of it there on a Mac); settings under the platform config folder (`%APPDATA%/Orbiters/Desktop`
//! on Windows, `~/.config/orbiters-desktop` on Linux); logs and caches, which stay on this computer, under
//! `%LOCALAPPDATA%/Orbiters`. An app whose platform keeps its files elsewhere (Android: the app's own files folder)
//! gives them once at start ([`set_roots`]).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folders an app gives where the platform's own are not the ones to use: on Android, the app's files and cache
/// folders, which only its Java side knows (`Context.getFilesDir`, `getCacheDir`), and no Documents folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roots {
    /// The settings and what the person keeps with them ([`config_dir`]).
    pub config: PathBuf,
    /// What stays on this device ([`local_data_root`]: speech models, avatar manifests, logs).
    pub local_data: PathBuf,
    /// What can be made again ([`cache_dir`]).
    pub cache: PathBuf,
    /// What the person makes ([`documents_root`]).
    pub documents: PathBuf,
}

static ROOTS: OnceLock<Roots> = OnceLock::new();

/// Gives the app's own folders, once, before any path is asked for: every path here derives from them from then on.
pub fn set_roots(roots: Roots) -> anyhow::Result<()> {
    ROOTS.set(roots).map_err(|_| anyhow::anyhow!("the app's folders were given already")) // i18n: not shown (a mistake in the program's start-up; the screenshot tool, run by developers, is the only one that passes it on)
}

/// The folders the app gave, when it gave some. In a test build (this crate's tests, or a crate whose tests enable
/// `test-support`) there are always some: a temporary folder of the test process ([`test_roots`]), so that no test
/// reads or writes the person's own settings, logs, caches or captures (night6).
pub fn roots() -> Option<&'static Roots> {
    #[cfg(any(test, feature = "test-support"))]
    {
        Some(ROOTS.get().unwrap_or_else(test_roots))
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        ROOTS.get()
    }
}

/// Test builds: this process's own folders under the system's temporary folder (`orbiters-tests-<pid>`), made at the
/// first ask. Folders of earlier test processes older than a day are removed then.
#[cfg(any(test, feature = "test-support"))]
pub fn test_roots() -> &'static Roots {
    static TEST: OnceLock<Roots> = OnceLock::new();
    TEST.get_or_init(|| {
        let temp = std::env::temp_dir();
        if let Ok(entries) = std::fs::read_dir(&temp) {
            let day = std::time::Duration::from_secs(24 * 3600);
            for e in entries.flatten() {
                let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
                if old && e.file_name().to_string_lossy().starts_with("orbiters-tests-") {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let root = temp.join(format!("orbiters-tests-{}", std::process::id()));
        let roots = Roots { config: root.join("config"), local_data: root.join("local"), cache: root.join("cache"), documents: root.join("documents") };
        for dir in [&roots.config, &roots.local_data, &roots.cache, &roots.documents] {
            let _ = std::fs::create_dir_all(dir);
        }
        roots
    })
}

static REAL_DATA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The screenshot harness's `--real-data`: with the app's folders given (a harness run), VRChat's own folders (its
/// pictures, its logs and settings) are still read in place. Off by default: they then come from the given folders.
pub fn allow_real_data() {
    REAL_DATA.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Where the folders of other programs this app reads (VRChat's logs and pictures) are looked for: the person's own,
/// or, when the app's folders were given (a harness, a test), under them (`local/LocalLow`, `documents/Pictures`),
/// unless [`allow_real_data`] said otherwise.
pub fn others_folders_in_roots() -> Option<&'static Roots> {
    roots().filter(|_| !REAL_DATA.load(std::sync::atomic::Ordering::Relaxed))
}

/// The person's pictures folder (`Pictures`), or `documents/Pictures` under the given folders
/// ([`others_folders_in_roots`]).
pub fn pictures_dir() -> Option<PathBuf> {
    match others_folders_in_roots() {
        Some(r) => Some(r.documents.join("Pictures")),
        None => dirs::picture_dir(),
    }
}

/// A folder for when the system names none: never a relative path, which would land wherever the app was started
/// from (System32 for a program started at sign-in).
fn fallback_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// `Documents/Orbiters`.
pub fn documents_root() -> PathBuf {
    documents_root_in(roots())
}

fn documents_root_in(roots: Option<&Roots>) -> PathBuf {
    match roots {
        Some(r) => r.documents.clone(),
        // Not in a Mac's Documents: gigabytes of captures would go to iCloud where it keeps that folder (as OneDrive does on
        // Windows, below), and the app would ask for permission before it could show anything.
        None if cfg!(target_os = "macos") => fallback_dir().join("Orbiters"),
        None => dirs::document_dir().unwrap_or_else(fallback_dir).join("Orbiters"),
    }
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
    if let Some(r) = roots() {
        return documents_root_in(Some(r)).join("Sessions");
    }
    if cfg!(target_os = "macos") {
        return documents_root().join("Sessions");
    }
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
    config_dir_in(roots())
}

fn config_dir_in(roots: Option<&Roots>) -> PathBuf {
    if let Some(r) = roots {
        return r.config.clone();
    }
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
    shared_with_unity_root().join("Mixer").join("inbox")
}

/// What Unity's Orbiters tools and this app both read and write (`Environment.SpecialFolder.LocalApplicationData` to them,
/// contracts `Docs/avatar-manifest.md`). It is [`local_data_root`] but on a Mac: Unity's Mono has no Mac branch for it and
/// answers `~/.local/share`, where `dirs` answers `~/Library/Application Support`, and both sides must find the same files.
pub fn shared_with_unity_root() -> PathBuf {
    #[cfg(target_os = "macos")]
    if roots().is_none() {
        let data = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")));
        if let Some(data) = data {
            return data.join("Orbiters");
        }
    }
    local_data_root()
}

/// `%LOCALAPPDATA%/Orbiters` (`~/.local/share/Orbiters` on Linux): data shared by the Orbiters apps and tools on
/// this computer, too big or too local for the config folder.
pub fn local_data_root() -> PathBuf {
    local_data_root_in(roots())
}

fn local_data_root_in(roots: Option<&Roots>) -> PathBuf {
    match roots {
        Some(r) => r.local_data.clone(),
        None => dirs::data_local_dir().unwrap_or_else(fallback_dir).join("Orbiters"),
    }
}

/// What Orbiters - N4N0 can make again (what screenshots say, world pages, thumbnails): on this computer only, not
/// in the roaming config folder.
pub fn cache_dir() -> PathBuf {
    cache_dir_in(roots())
}

fn cache_dir_in(roots: Option<&Roots>) -> PathBuf {
    match roots {
        Some(r) => r.cache.clone(),
        None => local_data_root_in(None).join("Desktop").join("cache"),
    }
}

/// Avatar manifests Orbiters Toolkit writes while the VRChat SDK builds an avatar (contracts `Docs/avatar-manifest.md`).
pub fn avatars_dir() -> PathBuf {
    shared_with_unity_root().join("Avatars")
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
    // A debug build started by the VR sandbox (`tools/vr-sandbox`) logs into the sandbox: a test build never writes the person's own logs.
    if cfg!(debug_assertions) {
        if let Some(dir) = std::env::var_os("ORBITERS_SANDBOX_LOGS").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir);
        }
    }
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
    // A harness or a test: never the person's own VRChat folder.
    if let Some(r) = others_folders_in_roots() {
        return Some(r.local_data.join("LocalLow").join("VRChat").join("VRChat")).filter(|p| p.is_dir());
    }
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

    #[test]
    fn an_app_s_own_folders_replace_the_platform_s() {
        // A phone app's: its files folder (as Java gives it) for the settings and data, its cache folder for the cache.
        let files = Path::new("/data/user/0/cc.orbiters.n4n0/files");
        let given = Roots { config: files.join("config"), local_data: files.join("data"), cache: files.join("cache"), documents: files.join("documents") };
        assert_eq!(config_dir_in(Some(&given)), files.join("config"));
        assert_eq!(local_data_root_in(Some(&given)), files.join("data"));
        assert_eq!(cache_dir_in(Some(&given)), files.join("cache"));
        assert_eq!(documents_root_in(Some(&given)), files.join("documents"));
        // None given (the desktop): the platform's, as before (computed, never touched here).
        assert_eq!(cache_dir_in(None), local_data_root_in(None).join("Desktop").join("cache"));
        assert!(config_dir_in(None).is_absolute());
    }

    /// Night6: in a test build every per-user folder is the test process's own: settings, logs, caches, captures,
    /// VRChat's pictures and logs. A path that resolves to the person's own folders fails this test.
    #[test]
    fn tests_never_resolve_the_persons_folders() {
        let root = test_roots().config.parent().unwrap().to_path_buf();
        assert!(root.starts_with(std::env::temp_dir()), "{}", root.display());
        let real: Vec<PathBuf> = [
            dirs::config_dir().map(|d| d.join("Orbiters")),
            dirs::data_local_dir().map(|d| d.join("Orbiters")),
            dirs::document_dir().map(|d| d.join("Orbiters")),
            dirs::picture_dir(),
            dirs::home_dir().map(|h| h.join("Orbiters")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut ours = vec![
            config_dir(),
            settings_file(),
            local_data_root(),
            logs_dir(),
            cache_dir(),
            models_dir(),
            avatars_dir(),
            shared_with_unity_root(),
            mixer_inbox_dir(),
            documents_root(),
            default_sessions_dir(),
            default_raw_captures_dir(),
            pictures_dir().expect("a pictures folder"),
            crate::settings::Settings::default().sessions_dir,
            crate::settings::Settings::default().raw_captures_dir,
        ];
        ours.extend(vrchat_log_dir());
        ours.extend(crate::visited::screenshot_dirs_with(&[]));
        for p in ours {
            assert!(p.starts_with(&root), "not the test's own: {}", p.display());
            assert!(!real.iter().any(|r| p.starts_with(r)), "the person's own: {}", p.display());
        }
    }
}
