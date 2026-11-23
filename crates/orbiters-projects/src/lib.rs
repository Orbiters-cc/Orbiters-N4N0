//! Unity discovery and VPM operations. Call on workers, never from an egui frame.
pub mod avatars;
pub mod blender;
mod builtin_templates;
pub mod coherent;
pub mod discovery;
pub mod editor;
pub mod git;
pub mod hidden;
pub mod icons;
#[cfg(target_os = "macos")]
mod mac;
pub mod missing;
pub mod network;
pub mod owned;
pub mod preview;
pub mod scene;
pub mod setup;
pub mod size;
pub mod storage;
pub mod suggestions;
pub mod templates;
pub mod updates;
mod vcc_store;
pub mod vpm;
mod vpm_http;

use orbiters_i18n::tr;
use std::path::{Path, PathBuf};

/// Explicit app roots also own the projects environment. They take precedence over machine defaults and the
/// standalone VCC/Hub overrides, so every startup worker uses the same isolated tree as its host application.
pub fn data_dir() -> PathBuf {
    if let Some(roots) = orbiters_core::paths::roots() {
        return roots.local_data.clone();
    }
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Orbiters")
}

/// The shared VPM environment VCC and ALCOM keep (settings, project list, repositories, templates). Without Creator
/// Companion it starts empty and VPM creates what it needs, as ALCOM does. `ORBITERS_VCC_DIR` points it elsewhere
/// (tests of a computer without VCC; never the person's real folder).
pub fn vcc_dir() -> PathBuf {
    if let Some(roots) = orbiters_core::paths::roots() {
        return roots.local_data.join("VCC");
    }
    match std::env::var_os("ORBITERS_VCC_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => vcc_parent().join("VRChatCreatorCompanion"),
    }
}

#[cfg(not(target_os = "macos"))]
fn vcc_parent() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir)
}

/// Where ALCOM and vrc-get keep their environment on the Mac, so that they list the same projects: vrc-get-vpm looks at
/// `XDG_DATA_HOME`, then `~/.local/share`, and not in Application Support.
#[cfg(target_os = "macos")]
fn vcc_parent() -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir().unwrap_or_else(std::env::temp_dir).join(".local/share"),
    }
}

/// Unity Hub's own files (its project and editor lists). `ORBITERS_UNITY_HUB_DIR` points them elsewhere, for stand-ins.
pub fn hub_data_dir() -> Option<PathBuf> {
    if let Some(roots) = orbiters_core::paths::roots() {
        return Some(roots.local_data.join("UnityHub"));
    }
    match std::env::var_os("ORBITERS_UNITY_HUB_DIR") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::config_dir().map(|d| d.join("UnityHub")),
    }
}

/// Where Unity writes the log of the editor last started.
pub fn editor_log() -> Option<PathBuf> {
    if let Some(roots) = orbiters_core::paths::roots() {
        return Some(roots.local_data.join("Unity/Editor/Editor.log"));
    }
    if cfg!(target_os = "macos") {
        dirs::home_dir().map(|p| p.join("Library/Logs/Unity/Editor.log"))
    } else if cfg!(windows) {
        dirs::data_local_dir().map(|p| p.join("Unity/Editor/Editor.log"))
    } else {
        dirs::config_dir().map(|p| p.join("unity3d/Editor.log"))
    }
}

/// [`editor_log`] as the person is told of it.
const EDITOR_LOG: &str = if cfg!(target_os = "macos") {
    "~/Library/Logs/Unity/Editor.log"
} else if cfg!(windows) {
    r"%LOCALAPPDATA%\Unity\Editor\Editor.log"
} else {
    "~/.config/unity3d/Editor.log"
};

/// Where Creator Companion itself is installed (its template covers), from its uninstall entry; none when it is not.
/// `ORBITERS_VCC_APP_DIR` replaces it (empty: not installed).
pub fn vcc_app_dir() -> Option<PathBuf> {
    if let Some(roots) = orbiters_core::paths::roots() {
        return Some(roots.local_data.join("VCC-app")).filter(|p| p.is_dir());
    }
    if let Some(dir) = std::env::var_os("ORBITERS_VCC_APP_DIR") {
        return (!dir.is_empty()).then(|| PathBuf::from(dir)).filter(|p| p.is_dir());
    }
    uninstall_entry(|key, name| key == "{A20FE4C3-FE52-495B-B0DA-92992240BFC0}_is1" || name.starts_with("VRChat Creator Companion"))
        .and_then(|(location, _)| location)
        .filter(|p| p.is_dir())
}

/// The Windows uninstall entry `matches` picks (by its key and display name): its install folder and display icon.
#[allow(unused_variables)]
fn uninstall_entry(matches: impl Fn(&str, &str) -> bool) -> Option<(Option<PathBuf>, Option<PathBuf>)> {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        const UNINSTALL: [&str; 2] =
            [r"Software\Microsoft\Windows\CurrentVersion\Uninstall", r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"];
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for path in UNINSTALL {
                let Ok(root) = winreg::RegKey::predef(hive).open_subkey(path) else { continue };
                for key in root.enum_keys().flatten() {
                    let Ok(entry) = root.open_subkey(&key) else { continue };
                    let name: String = entry.get_value("DisplayName").unwrap_or_default();
                    if matches(&key, &name) {
                        let value = |v: &str| entry.get_value::<String, _>(v).ok().map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty());
                        // DisplayIcon is "path,index".
                        let icon = value("DisplayIcon")
                            .map(|s| PathBuf::from(s.rsplit_once(',').filter(|(_, i)| i.parse::<i32>().is_ok()).map_or(s.as_str(), |(p, _)| p)));
                        return Some((value("InstallLocation").map(PathBuf::from), icon));
                    }
                }
            }
        }
    }
    None
}

/// Unity Hub's program: where VCC was told it is, else where its installer says, else its usual folder.
pub fn unity_hub() -> Option<PathBuf> {
    let settings: serde_json::Value = std::fs::read(vcc_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let configured = settings["pathToUnityHub"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from);
    #[cfg(target_os = "macos")]
    return unity_hub_on_mac(configured);
    #[cfg(not(target_os = "macos"))]
    {
        let installed = uninstall_entry(|key, name| key == "Unity Technologies - Hub" || name.starts_with("Unity Hub")).and_then(|(location, icon)| { // i18n: not shown (Windows registry key and macOS query)
            icon.filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))).or(location.map(|l| l.join("Unity Hub.exe")))
        });
        [configured, installed, Some(PathBuf::from(r"C:\Program Files\Unity Hub\Unity Hub.exe"))].into_iter().flatten().find(|p| p.is_file())
    }
}

/// On the Mac: where VCC or ALCOM were told it is (the bundle or its program), the Applications folders, then wherever
/// Spotlight knows it by its bundle identifier (it can be anywhere: a drag is all it takes to move an app).
#[cfg(target_os = "macos")]
fn unity_hub_on_mac(configured: Option<PathBuf>) -> Option<PathBuf> {
    let configured = configured.map(|p| if p.extension().is_some_and(|e| e == "app") { mac::bundle_program(&p) } else { p });
    let usual = [Some(PathBuf::from("/Applications")), dirs::home_dir().map(|h| h.join("Applications"))];
    let usual = usual.into_iter().flatten().map(|d| mac::bundle_program(&d.join("Unity Hub.app")));
    configured
        .into_iter()
        .chain(usual)
        .find(|p| p.is_file())
        .or_else(|| mac::spotlight("kMDItemCFBundleIdentifier == 'com.unity3d.unityhub'").iter().map(|app| mac::bundle_program(app)).find(|p| p.is_file())) // i18n: not shown (Windows registry key and macOS query)
}

pub fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?)
}

/// Atomic publication in the destination directory; no partially written JSON is visible.
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or_else(|| anyhow::anyhow!("Missing parent folder"))?)?;
    // A file of this write's own (PROJECTS-36): the window and the service write the same files, and a shared temporary
    // name let one publish, truncate or take the other's half-written bytes.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}-{n}.tmp", std::process::id()));
    let written = (|| -> anyhow::Result<()> {
        let mut f = std::fs::File::options().write(true).create_new(true).open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(value)?)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // Windows rename does not replace existing files. MoveFileEx provides replacement without a gap.
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
        let src: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let dst: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Another writer replacing the same file at this instant (or a reader holding it) refuses the move for a moment.
        let mut tries = 0;
        while let Err(e) = unsafe { MoveFileExW(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } {
            tries += 1;
            if tries >= 20 {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5 * tries));
        }
    }
    #[cfg(not(windows))]
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

pub fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c
}

pub fn unity_open(path: &Path) -> bool {
    let lock = path.join("Temp/UnityLockfile");
    #[cfg(not(unix))]
    return lock.exists() && std::fs::OpenOptions::new().write(true).open(lock).is_err();
    // Unity's lock file is no exclusive open here: anyone can open it, and it stays behind after a crash. What tells is
    // a lock held on it, or an editor whose command line names the project.
    #[cfg(unix)]
    return lock_held(&lock) || editor::of(&editor::running(), path, false).is_some();
}

/// Whether a process holds a lock on `file`. `F_GETLK` takes none itself, so asking cannot get in the way of Unity
/// starting at that moment. On the Mac it answers for `flock` locks as well (measured on macOS 27, in both directions).
#[cfg(unix)]
fn lock_held(file: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::File::open(file) else { return false };
    // SAFETY: an all-zero flock is a valid value of it, and the descriptor is open for the call.
    let mut probe: libc::flock = unsafe { std::mem::zeroed() };
    probe.l_type = libc::F_WRLCK as _;
    probe.l_whence = libc::SEEK_SET as _;
    let answered = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut probe) } == 0;
    if answered && probe.l_type as i32 != libc::F_UNLCK as i32 {
        return true;
    }
    // Elsewhere the two kinds of lock do not see each other. A shared lock of a moment asks about the other kind.
    #[cfg(not(target_os = "macos"))]
    if matches!(file.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock)) {
        return true;
    }
    false
}

/// What Open in Unity did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Opened {
    /// Unity had this project open already: its window was brought to the front (if it has one yet) rather than a second
    /// one started.
    pub already_open: bool,
    /// A Library that barely existed was moved to a disk with room first ([`storage::preflight`]).
    pub moved: Option<PathBuf>,
}

/// How long Open in Unity may take before it says so. Everything it does is a moment's work on a disk that answers; a
/// disk that does not (a USB disk resetting) must not leave "Opening Unity…" spinning with every other action greyed out.
const OPEN_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a started editor is watched: Unity that cannot run at all ends within this. Longer would keep the toast up
/// for nothing, since the panel sees the editor by itself from then on.
const START_WATCH: std::time::Duration = std::time::Duration::from_millis(1500);

/// Runs `f` on its own thread and gives up waiting after `limit` (`None`; the thread then ends by itself, if ever). For
/// calls that cannot be interrupted and may block on a disk or a locked database.
pub(crate) fn within<T: Send + 'static>(limit: std::time::Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok()
}

/// Opens `project` in its Unity version and returns once the editor has started (or was already there). Always prompt:
/// never a long operation, which [`storage::repair`] is, asked for separately. Call on a worker.
pub fn open_project(project: &discovery::Project) -> anyhow::Result<Opened> {
    let name = project.name.clone();
    let p = project.clone();
    within(OPEN_LIMIT, move || open_now(&p)).unwrap_or_else(|| {
        anyhow::bail!("{}", tr!("Opening {name} is taking too long: its disk or Unity's list of editors is not answering. Unity may still open by itself.", name = name))
    })
}

fn open_now(project: &discovery::Project) -> anyhow::Result<Opened> {
    anyhow::ensure!(
        discovery::is_project(&project.path),
        "{}",
        tr!("{name} is not at {path} any more. If it is on a disk that is not connected, connect it and try again.", name = project.name, path = project.path.display())
    );
    let locked = unity_open(&project.path);
    let editors = editor::running();
    if let Some(open) = editor::of(&editors, &project.path, locked) {
        // Unity still starting has no window to bring forward yet; it is open all the same.
        let _ = editor::show(open.pid, &project.path);
        return Ok(Opened { already_open: true, moved: None });
    }
    // Unity would only say the same, in a dialog of its own, and quit.
    anyhow::ensure!(
        !locked,
        "{}",
        tr!("Something else holds {name}'s lock (its Temp/UnityLockfile): Unity still quitting, or Orbiters repairing its storage. Try again in a moment.", name = project.name)
    );
    anyhow::ensure!(
        !project.unity.is_empty(),
        "{}",
        tr!("{name} does not say which Unity version it uses (ProjectSettings/ProjectVersion.txt).", name = project.name)
    );
    let editor = discovery::find_editor(&project.unity)
        .ok_or_else(|| anyhow::anyhow!("{}", tr!("Unity {version} is not installed. Open Unity Hub to install this version.", version = project.unity)))?;
    // Only ever instant. A problem with it must not stop Unity opening: the panel's warning says what to do about the disk.
    let moved = storage::preflight(&project.path).unwrap_or_else(|e| {
        eprintln!("the storage check of {} did not finish: {e:#}", project.path.display());
        None
    });
    #[cfg(not(target_os = "macos"))]
    let started = {
        let mut command = hidden_command(&editor);
        command.arg("-projectPath").arg(&project.path);
        start_watched(command, START_WATCH, Starting { unity: &project.unity, editor: &editor })
    };
    #[cfg(target_os = "macos")]
    let started = start_on_mac(&editor, &project.path, START_WATCH, &project.unity);
    started?;
    Ok(Opened { already_open: false, moved })
}

/// The editor being started, so that each way it can fail is told as one whole sentence (a sentence cut in pieces cannot be translated).
#[derive(Clone, Copy)]
struct Starting<'a> {
    unity: &'a str,
    editor: &'a Path,
}

impl Starting<'_> {
    fn not_started(&self, why: impl std::fmt::Display) -> anyhow::Error {
        let (unity, editor, why) = (self.unity, self.editor.display(), why.to_string());
        anyhow::anyhow!("{}", tr!("Unity {unity} ({editor}) could not be started: {why}", unity = unity, editor = editor, why = why))
    }

    fn not_watched(&self, why: impl std::fmt::Display) -> anyhow::Error {
        let (unity, editor, why) = (self.unity, self.editor.display(), why.to_string());
        anyhow::anyhow!("{}", tr!("Unity {unity} ({editor}) could not be watched: {why}", unity = unity, editor = editor, why = why))
    }

    /// It ended at once, with `status` when the system says one.
    fn quit(&self, status: Option<std::process::ExitStatus>) -> anyhow::Error {
        let (unity, editor) = (self.unity, self.editor.display());
        match status {
            Some(status) => anyhow::anyhow!(
                "{}",
                tr!("Unity {unity} ({editor}) quit right after starting ({status}). Its log is at {log}.", unity = unity, editor = editor, status = status, log = EDITOR_LOG)
            ),
            None => anyhow::anyhow!(
                "{}",
                tr!("Unity {unity} ({editor}) quit right after starting. Its log is at {log}.", unity = unity, editor = editor, log = EDITOR_LOG)
            ),
        }
    }
}

/// Starts `editor` on `project`, through the system when it is in an application bundle ([`mac::start_app`]). There is no
/// exit status to read then. What shows a failure is `open`'s own answer, the editor's process going again within `watch`
/// of being seen, or never being seen: a process exists as soon as `open` has answered, so one not there after
/// [`FIRST_SIGHT`] was gone before it could be looked at.
#[cfg(target_os = "macos")]
fn start_on_mac(editor: &Path, project: &Path, watch: std::time::Duration, unity: &str) -> anyhow::Result<()> {
    use std::process::Stdio;
    let who = Starting { unity, editor };
    let Some(app) = mac::enclosing_app(editor) else {
        let mut command = hidden_command(editor);
        command.arg("-projectPath").arg(project).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        return start_watched(command, watch, who);
    };
    let answer = mac::start_app(&app).arg("--args").arg("-projectPath").arg(project).output().map_err(|e| who.not_started(e))?;
    if !answer.status.success() {
        return Err(who.not_started(String::from_utf8_lossy(&answer.stderr).trim()));
    }
    let start = std::time::Instant::now();
    let mut seen: Option<std::time::Instant> = None;
    loop {
        let here = editor::of(&editor::running(), project, false).is_some();
        if !(here || (seen.is_none() && start.elapsed() < FIRST_SIGHT)) {
            return Err(who.quit(None));
        }
        if here && seen.get_or_insert_with(std::time::Instant::now).elapsed() >= watch {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// How long the system may take to show a process for an editor it was told to start.
#[cfg(target_os = "macos")]
const FIRST_SIGHT: std::time::Duration = std::time::Duration::from_secs(6);

/// Starts `command` and watches it for `watch`: a program that could not start, or that quit at once with a failure, is an
/// error rather than a success said too soon. One still running, or ended well (it handed over to another process), is
/// started.
fn start_watched(mut command: std::process::Command, watch: std::time::Duration, who: Starting) -> anyhow::Result<()> {
    let mut child = command.spawn().map_err(|e| who.not_started(e))?;
    let start = std::time::Instant::now();
    while start.elapsed() < watch {
        if let Some(status) = child.try_wait().map_err(|e| who.not_watched(e))? {
            if !status.success() {
                return Err(who.quit(Some(status)));
            }
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // A child that ended stays a zombie until it is waited for, and it goes on running by itself.
    #[cfg(unix)]
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn open_hub() -> anyhow::Result<()> {
    let path = unity_hub().ok_or_else(|| anyhow::anyhow!("{}", tr("Unity Hub is not installed. Download it from unity.com/download.")))?;
    #[cfg(target_os = "macos")]
    if let Some(app) = mac::enclosing_app(&path) {
        let answer = mac::open_command().arg(&app).output()?;
        anyhow::ensure!(
            answer.status.success(),
            "{}",
            tr!("Unity Hub could not be opened: {why}", why = String::from_utf8_lossy(&answer.stderr).trim())
        );
        return Ok(());
    }
    hidden_command(path).spawn()?;
    Ok(())
}

/// What shows `path` in the system's file manager: a folder opened; a file in its folder (selected in the Finder).
fn reveal_command(path: &Path) -> std::process::Command {
    #[cfg(windows)]
    {
        let mut c = hidden_command("explorer.exe");
        c.arg(path);
        c
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = mac::open_command();
        if !path.is_dir() {
            c.arg("-R");
        }
        c.arg(path);
        c
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut c = hidden_command("xdg-open");
        c.arg(if path.is_dir() { path } else { path.parent().unwrap_or(path) });
        c
    }
}

/// Shows `path` in the system's file manager: Explorer, the Finder.
pub fn reveal(path: &Path) -> anyhow::Result<()> {
    reveal_command(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod open_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn who() -> Starting<'static> {
        Starting { unity: "2022.3.22f1", editor: Path::new("Unity.exe") }
    }

    #[cfg(windows)]
    fn cmd(args: &[&str]) -> std::process::Command {
        let mut c = hidden_command("cmd");
        c.arg("/c").args(args);
        c
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_quits_at_once_with_a_failure_is_said_to_have_failed() {
        let error = start_watched(cmd(&["exit", "3"]), Duration::from_secs(10), who()).unwrap_err();
        assert!(format!("{error:#}").contains("quit right after starting"), "{error:#}");
    }

    #[cfg(windows)]
    #[test]
    fn a_program_that_ended_well_or_goes_on_running_is_started_without_waiting_for_it() {
        start_watched(cmd(&["exit", "0"]), Duration::from_secs(10), who()).unwrap();
        let start = Instant::now();
        start_watched(cmd(&["ping", "-n", "4", "127.0.0.1"]), Duration::from_millis(200), who()).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2), "the watch ends the wait, not the program");
    }

    #[cfg(unix)]
    fn sh(script: &str) -> std::process::Command {
        let mut c = hidden_command("/bin/sh");
        c.args(["-c", script]);
        c
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_quits_at_once_with_a_failure_says_where_its_log_is() {
        let error = format!("{:#}", start_watched(sh("exit 3"), Duration::from_secs(10), who()).unwrap_err());
        assert!(error.contains("quit right after starting") && error.contains(EDITOR_LOG), "{error}");
        start_watched(sh("exit 0"), Duration::from_secs(10), who()).unwrap();
        let start = Instant::now();
        start_watched(sh("sleep 3"), Duration::from_millis(200), who()).unwrap();
        assert!(start.elapsed() < Duration::from_secs(2), "the watch ends the wait, not the program");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_keeps_things_where_its_other_programs_look() {
        // Whatever is set in this process, vrc-get-vpm is what ALCOM uses: the same folder.
        if std::env::var_os("ORBITERS_VCC_DIR").is_none() {
            let upstream = vrc_get_vpm::io::DefaultEnvironmentIo::new_default();
            assert_eq!(vcc_dir(), upstream.resolve(Path::new("")).components().collect::<PathBuf>());
            assert!(!vcc_dir().starts_with(dirs::data_local_dir().unwrap()), "not in Application Support: ALCOM would list other projects");
        }
        assert!(editor_log().unwrap().ends_with("Library/Logs/Unity/Editor.log"));
        assert!(!EDITOR_LOG.contains("LOCALAPPDATA"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn revealing_selects_a_file_in_the_finder_and_opens_a_folder() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("scene.blend");
        std::fs::write(&file, "x").unwrap();
        let args = |c: std::process::Command| (c.get_program().to_os_string(), c.get_args().map(|a| a.to_os_string()).collect::<Vec<_>>());
        assert_eq!(args(reveal_command(temp.path())), ("/usr/bin/open".into(), vec![temp.path().as_os_str().to_os_string()]));
        assert_eq!(args(reveal_command(&file)), ("/usr/bin/open".into(), vec!["-R".into(), file.as_os_str().to_os_string()]));
    }

    #[cfg(unix)]
    #[test]
    fn a_held_lock_is_told_from_a_lock_file_left_behind() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("Project");
        std::fs::create_dir_all(project.join("Temp")).unwrap();
        assert!(!unity_open(&project), "no lock file");
        // What a crash leaves: the file, and nobody holding it.
        std::fs::write(project.join("Temp/UnityLockfile"), "").unwrap();
        assert!(!unity_open(&project));
        let held = std::fs::File::open(project.join("Temp/UnityLockfile")).unwrap();
        held.lock().unwrap();
        assert!(unity_open(&project), "held, whoever holds it");
        drop(held);
        assert!(!unity_open(&project), "let go");
    }

    #[test]
    fn a_start_failure_is_one_sentence_in_the_language_of_the_moment() {
        use orbiters_i18n::{in_language, Language};
        let said = |language| in_language(language, || who().quit(None).to_string());
        assert!(said(Language::En).contains("quit right after starting"));
        let french = said(Language::Fr);
        assert!(french.contains("2022.3.22f1") && french.contains(EDITOR_LOG) && !french.contains("quit right"), "{french}");
        let japanese = in_language(Language::Ja, || who().not_started("x").to_string());
        assert!(japanese.contains("起動できませんでした") && japanese.contains('x'), "{japanese}");
    }

    #[test]
    fn the_conflict_error_is_said_in_the_language_of_the_moment() {
        use orbiters_i18n::{in_language, Language};
        let error = coherent::Conflicted { plan: Default::default() };
        assert!(error.to_string().contains("conflict with installed packages"));
        let portuguese = in_language(Language::PtBr, || error.to_string());
        assert!(portuguese.contains("conflito") && !portuguese.contains("conflict with"), "{portuguese}");
    }

    #[test]
    fn a_program_that_cannot_start_says_so() {
        let error = start_watched(hidden_command("Z:/no/such/Unity.exe"), Duration::from_secs(1), who()).unwrap_err();
        assert!(format!("{error:#}").contains("could not be started"), "{error:#}");
    }

    #[test]
    fn a_call_that_blocks_is_given_up_on() {
        let start = Instant::now();
        assert_eq!(within(Duration::from_millis(50), || std::thread::sleep(Duration::from_secs(5))), None);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(within(Duration::from_secs(5), || 7), Some(7));
    }

    #[test]
    fn a_project_gone_from_its_folder_is_not_handed_to_unity() {
        // Unity would make a new, empty project there.
        let temp = tempfile::tempdir().unwrap();
        let project = discovery::Project { name: "Gone".into(), path: temp.path().join("gone"), unity: "2022.3.22f1".into(), ..Default::default() };
        let error = open_now(&project).unwrap_err();
        assert!(format!("{error:#}").contains("is not at"), "{error:#}");
        assert!(!temp.path().join("gone").exists());
    }
}

#[cfg(test)]
mod write_tests {
    /// PROJECTS-36: writers of the same file at the same time each publish whole bytes, and none fails on another's
    /// temporary file.
    #[test]
    fn writers_of_one_file_never_share_a_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unity-projects.json");
        let writers: Vec<_> = (0..4)
            .map(|w| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..40 {
                        super::write_json(&path, &serde_json::json!({ "writer": w, "i": i, "pad": "x".repeat(4096) })).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["i"], 39);
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(left.is_empty(), "{left:?}");
    }
}
