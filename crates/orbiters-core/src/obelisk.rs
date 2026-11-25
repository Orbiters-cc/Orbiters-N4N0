//! VRChat on a Mac: what runs it there is Obelisk's engine (github.com/Zagrios/Obelisk, GPL-3.0: Wine for Linux in
//! native macOS processes, x86 through FEX, so that VRChat's Easy Anti-Cheat starts as on a Steam Deck, Direct3D 11
//! drawn with DXMT). N4N0 never shows Obelisk's window: it runs `obelisk-cli`, the engine without a window, from the
//! fork at github.com/Orbiters-cc/Obelisk (branch `cli`, GPL-3.0 as Obelisk is; `docs/CLI.md` there is the contract
//! this module reads). It is a separate program N4N0 downloads and starts, never a line of its code in N4N0.
//!
//! The program signs in to Steam (the person's own account owns VRChat), downloads VRChat and the engine, and starts
//! the game with the arguments N4N0 gives. Here: which release to download, where things are kept, what the program
//! says (its JSON Lines), whether VRChat is installed and running. All of it pure, over paths and texts, so that it is
//! tested on every computer; the desktop (`controller/obelisk_cli.rs`) downloads and runs.
use std::path::{Path, PathBuf};

/// VRChat at Steam.
pub use crate::vrchat_launch::STEAM_APP;

/// The fork's releases, as GitHub tells them (no account, nothing of the person in the request).
pub const RELEASES: &str = "https://api.github.com/repos/Orbiters-cc/Obelisk/releases?per_page=30";
/// Where a release's files are, and nowhere else is one taken from.
const DOWNLOADS: &str = "https://github.com/Orbiters-cc/Obelisk/releases/download/";
/// The largest archive taken (0.1.0's is 11 MB).
const LARGEST: u64 = 256 << 20;
/// The first part of a release's tag (`cli-v0.1.0`).
const TAG: &str = "cli-v";
/// What `codesign --verify -R` holds each downloaded program to: signed by Orbiters' own team (Enzo Dutra,
/// Z57XP83A9Z; an Apple Development or a Developer ID certificate of it). A program signed by anyone else is not run.
pub const REQUIREMENT: &str = r#"anchor apple generic and certificate leaf[subject.OU] = "Z57XP83A9Z""#; // i18n: not shown (a code requirement, in codesign's language)
/// The programs of a release that are checked against [`REQUIREMENT`], from its folder.
pub const PROGRAMS: [&str; 4] = ["obelisk-cli", "SteamNative/obelisk-steam-host", "SteamNative/obelisk-steam-sdk-x86_64", "SteamNative/libobelisk_steam_discovery_adapter.dylib"];

/// The archive of a release, to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    /// `0.1.0`.
    pub version: String,
    /// The archive's name, and without `.tar.gz` the one folder it holds.
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The address of the file that says its SHA-256, published beside it.
    pub checksum_url: String,
}

impl Download {
    /// The folder the archive holds (`obelisk-cli-0.1.0-arm64`).
    pub fn folder(&self) -> &str {
        self.name.trim_end_matches(".tar.gz")
    }
}

/// A version's numbers, to tell the newer of two (`0.10.0` after `0.9.3`); none for anything else than numbers.
fn numbers(version: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = version.split('.').map(|n| n.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 4)
}

/// The newest Apple silicon archive GitHub's list of releases names, with its checksum file. A list without one, or
/// naming files anywhere but the fork's releases, gives none.
pub fn latest_download(releases: &str) -> Option<Download> {
    let releases: serde_json::Value = serde_json::from_str(releases).ok()?;
    releases
        .as_array()?
        .iter()
        .filter(|r| r.get("draft").and_then(serde_json::Value::as_bool) != Some(true))
        .filter_map(|release| {
            let version = release.get("tag_name")?.as_str()?.strip_prefix(TAG)?;
            let order = numbers(version)?;
            let assets: Vec<(&str, &str, u64)> = release
                .get("assets")?
                .as_array()?
                .iter()
                .filter_map(|a| Some((a.get("name")?.as_str()?, a.get("browser_download_url")?.as_str()?, a.get("size")?.as_u64()?)))
                .filter(|(name, url, _)| *url == format!("{DOWNLOADS}{TAG}{version}/{name}"))
                .collect();
            let name = format!("obelisk-cli-{version}-arm64.tar.gz");
            let (_, url, size) = *assets.iter().find(|(n, _, size)| *n == name && (1..=LARGEST).contains(size))?;
            let (_, checksum_url, _) = *assets.iter().find(|(n, _, _)| *n == format!("{name}.sha256"))?;
            Some((order, Download { version: version.into(), name, url: url.into(), size, checksum_url: checksum_url.into() }))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, download)| download)
}

/// The SHA-256 a checksum file says (`<64 hex digits>  <file name>`), in lower case.
pub fn checksum(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    (first.len() == 64 && first.bytes().all(|b| b.is_ascii_hexdigit())).then(|| first.to_ascii_lowercase())
}

/// The home of the person at this Mac.
pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// The program's library of the person whose home is `home` (its own default): the games, the engine, the receipts,
/// the records of what runs.
pub fn library(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("cc.orbiters.obelisk-cli")
}

/// Where N4N0 keeps the releases it downloaded, in its settings' folder `config`.
pub fn tools(config: &Path) -> PathBuf {
    config.join("obelisk-cli")
}

/// The program of the newest release in `tools`, whole (its Steam helper beside it).
pub fn program(tools: &Path) -> Option<PathBuf> {
    std::fs::read_dir(tools)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let order = numbers(name.strip_prefix("obelisk-cli-")?.strip_suffix("-arm64")?)?;
            let folder = entry.path();
            (folder.join(PROGRAMS[0]).is_file() && folder.join(PROGRAMS[1]).is_file()).then(|| (order, folder.join(PROGRAMS[0])))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, program)| program)
}

/// The folders one receipt file (`steam-installations-<steam id>.json`: an array of installations, each with Steam's
/// `appId` and its `directory`) says VRChat is installed in. A text that is not such a file gives none.
pub fn vrchat_directories(receipts: &str) -> Vec<PathBuf> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(receipts) else { return Vec::new() };
    items
        .iter()
        .filter(|r| r.get("appId").and_then(serde_json::Value::as_u64).map(|id| id.to_string()).as_deref() == Some(crate::vrchat_launch::STEAM_APP))
        .filter_map(|r| r.get("directory").and_then(serde_json::Value::as_str))
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Whether `name` is a receipt file of installed Steam games (the `.incomplete.json` beside it lists downloads that did
/// not finish).
pub fn is_receipt_file(name: &str) -> bool {
    name.starts_with("steam-installations-") && name.ends_with(".json") && !name.ends_with(".incomplete.json")
}

/// VRChat's folder as the program installed it, from the receipts in `library`: the first one whose game is really there.
pub fn vrchat_install(library: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(library).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(is_receipt_file)).collect();
    files.sort();
    files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|text| vrchat_directories(&text))
        .find(|d| d.join("VRChat.exe").is_file() || d.join("start_protected_game.exe").is_file())
}

/// The process of the VRChat that one of the engine's session records (`SteamSessions/<id>.v2.json`, which the game's
/// supervisor keeps while it runs) says is running: Steam's VRChat, neither finished nor failed, with the game's
/// process named. The caller checks that the process still lives (a record outlives a crash).
pub fn session_game(record: &str) -> Option<u32> {
    let record: serde_json::Value = serde_json::from_str(record).ok()?;
    let app = record.get("store")?.get("app_id")?.as_u64()?;
    let over = matches!(record.get("phase").and_then(serde_json::Value::as_str), Some("Finished" | "Failed")); // i18n: not shown (values in the engine's file)
    if app.to_string() != crate::vrchat_launch::STEAM_APP || over {
        return None;
    }
    record.get("game")?.get("pid")?.as_u64().and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 1)
}

/// The processes the session records in `library` name as a running VRChat. `None` when the records could not be read
/// at all: that is not "none runs" (a folder that does not exist yet is: no game was ever started there).
pub fn session_games(library: &Path) -> Option<Vec<u32>> {
    let files = match std::fs::read_dir(library.join("SteamSessions")) {
        Ok(files) => files,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    Some(files.flatten().filter(|f| f.file_name().to_string_lossy().ends_with(".v2.json")).filter_map(|f| std::fs::read_to_string(f.path()).ok()).filter_map(|text| session_game(&text)).collect())
}

/// A running game's process and, when its record says it, its birth (seconds, microseconds).
pub type GameBorn = (u32, Option<(u64, u32)>);

/// The VRChats the session records in `library` name as running, with the birth of each one's process when the record
/// says it (obelisk-cli 0.1.5): `None` when the records could not be read at all (that is not "none runs").
pub fn session_games_born(library: &Path) -> Option<Vec<GameBorn>> {
    let files = match std::fs::read_dir(library.join("SteamSessions")) {
        Ok(files) => files,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    Some(
        files
            .flatten()
            .filter(|f| f.file_name().to_string_lossy().ends_with(".v2.json"))
            .filter_map(|f| std::fs::read_to_string(f.path()).ok())
            .filter_map(|text| session_record(&text))
            .filter(|r| !r.over())
            .filter_map(|r| Some((r.game?, r.game_birth)))
            .collect(),
    )
}

/// One of the engine's Game session records (`SteamSessions/<id>.v2.json`) as far as N4N0 reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    /// The Game session's UUID (the record's name).
    pub id: String,
    /// `Preparing`, `Running`, `Finalizing`, `Finished`, `Failed`.
    pub phase: String,
    /// The engine's launcher of the game while it runs (the `running` event's pid).
    pub game: Option<u32>,
    /// How long the game ran, and how many frames it presented, once it ended (`None` for a count the engine could not
    /// read: `played.frames.measured` false, obelisk-cli 0.1.5).
    pub seconds: Option<u64>,
    pub frames: Option<u64>,
    /// When the game's process (the engine's launcher) was born, as the supervisor recorded it: seconds, microseconds.
    pub game_birth: Option<(u64, u32)>,
    /// While it runs, the frames shown so far (obelisk-cli 0.1.5 and later).
    pub shown: Option<Shown>,
    /// When the session's supervisor accepted a Quit of its game, on the boot clock (obelisk-cli 0.1.8): the boot's
    /// session UUID and the milliseconds since it.
    pub quit_accepted: Option<(String, u64)>,
    /// The Steam account it runs for.
    pub account: Option<u64>,
    /// The process of the session's supervisor and its birth, which writes the game's log and this record.
    pub supervisor: Option<(u32, (u64, u32))>,
    /// Its game's log does not hold what the game wrote (obelisk-cli 0.1.10: why, in the record).
    pub log_failed: bool,
}

/// The frames a running game has shown so far, as its record says them (`shown`, obelisk-cli 0.1.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub frames: u64,
    /// When the count was taken, seconds since 1970.
    pub at: u64,
    /// Counted from the game's own log without a gap: false once it is not, and then the count says nothing more.
    pub continuous: bool,
}

impl SessionRecord {
    pub fn over(&self) -> bool {
        matches!(self.phase.as_str(), "Finished" | "Failed") // i18n: not shown (values in the engine's file)
    }

    /// The game ended before it was played: within [`EARLY_END`] of its start, or without a frame shown (a start whose
    /// graphics never came up runs on 100 s and more, measured 2026-11-22 19:54).
    pub fn ended_early(&self) -> bool {
        self.over() && early(self.seconds, self.frames)
    }
}

/// Whether a game that ran `seconds` and showed `frames` (each `None` when not known) ended before it was played: within
/// [`EARLY_END`], or without a frame shown within [`NO_FRAMES_FOR`] (a start whose graphics never came up runs on 100 s and
/// more; a long game whose log reported no frame is no early end, whatever its count says: its log carried no timing).
pub fn early(seconds: Option<u64>, frames: Option<u64>) -> bool {
    seconds.is_some_and(|s| s < EARLY_END.as_secs()) || (frames == Some(0) && seconds.is_none_or(|s| s < NO_FRAMES_FOR.as_secs()))
}

/// How long a game without a frame shown still counts as one that did not start.
pub const NO_FRAMES_FOR: std::time::Duration = std::time::Duration::from_secs(600);

/// A game that ends this soon after it started did not start for the person (the engine, or VRChat in it, crashed).
pub const EARLY_END: std::time::Duration = std::time::Duration::from_secs(150);
/// How long VRChat is not started again after a start that ended early: one within about 80 s of an early end is what makes
/// Easy Anti-Cheat fail (AGENTS.md, the VR sandbox's rules), whoever starts it (Home's Play or the bridge).
pub const AFTER_EARLY_END: std::time::Duration = std::time::Duration::from_secs(80);

/// Reads a session record; anything else gives none.
pub fn session_record(text: &str) -> Option<SessionRecord> {
    let record: serde_json::Value = serde_json::from_str(text).ok()?;
    let app = record.get("store")?.get("app_id")?.as_u64()?;
    if app.to_string() != crate::vrchat_launch::STEAM_APP {
        return None;
    }
    let played = record.get("played").filter(|p| p.is_object());
    let frames = played.and_then(|p| p.get("frames"));
    // Absent in records before obelisk-cli 0.1.5, which read the log the same way: measured.
    let measured = frames.and_then(|f| f.get("measured")).and_then(serde_json::Value::as_bool).unwrap_or(true);
    let game = record.get("game").filter(|g| g.is_object());
    let birth = |g: &serde_json::Value| Some((g.get("birth_seconds")?.as_u64()?, u32::try_from(g.get("birth_microseconds")?.as_u64()?).ok()?));
    let shown = record.get("shown").filter(|s| s.is_object());
    Some(SessionRecord {
        id: record.get("id")?.as_str()?.to_string(),
        phase: record.get("phase")?.as_str()?.to_string(),
        game: game.and_then(|g| g.get("pid")).and_then(serde_json::Value::as_u64).and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 1),
        seconds: played.and_then(|p| p.get("seconds")).and_then(serde_json::Value::as_u64),
        frames: frames.and_then(|f| f.get("count")).and_then(serde_json::Value::as_u64).filter(|_| measured),
        game_birth: game.and_then(birth),
        shown: shown.and_then(|s| {
            Some(Shown { frames: s.get("frames")?.as_u64()?, at: s.get("at")?.as_u64()?, continuous: s.get("continuous")?.as_bool()? })
        }),
        quit_accepted: record.get("quit_accepted").and_then(boot_instant).map(|at| (at.boot, (at.seconds * 1000.0) as u64)),
        account: record.get("account_id").and_then(serde_json::Value::as_u64),
        supervisor: record.get("supervisor").and_then(|s| Some((u32::try_from(s.get("pid")?.as_u64()?).ok()?, birth(s)?))),
        log_failed: record.get("log_failure").is_some_and(|f| !f.is_null()),
    })
}

/// Every VRChat session record in `library`, with when it was last written.
pub fn session_records(library: &Path) -> Vec<(SessionRecord, std::time::SystemTime)> {
    let Ok(files) = std::fs::read_dir(library.join("SteamSessions")) else { return Vec::new() };
    files
        .flatten()
        .filter(|f| f.file_name().to_string_lossy().ends_with(".v2.json"))
        .filter_map(|f| {
            let written = f.metadata().and_then(|m| m.modified()).ok()?;
            Some((session_record(&std::fs::read_to_string(f.path()).ok()?)?, written))
        })
        .collect()
}

/// The engine's records could not be read now (not "there are none"): asked again later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// The record of the Game session `id` in `library`: `Ok(None)` when there is none (yet, or no more), `Err` when it could
/// not be read or is not whole (a supervisor rewriting it: ask again). Only a UUID names a record.
pub fn session_record_of(library: &Path, id: &str) -> Result<Option<SessionRecord>, Unreadable> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Ok(None);
    }
    match std::fs::read_to_string(library.join("SteamSessions").join(format!("{id}.v2.json"))) {
        Ok(text) => session_record(&text).filter(|r| r.id == id).map(Some).ok_or(Unreadable),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Unreadable),
    }
}

/// A moment on the clock since this Mac booted: the boot's own session UUID (`kern.bootsessionuuid`, which a wall clock
/// set forward or back never changes) and the seconds since that boot, sleep included (`CLOCK_MONOTONIC` on macOS).
#[derive(Clone, Debug, PartialEq)]
pub struct BootInstant {
    pub boot: String,
    pub seconds: f64,
}

/// What identifies one file's content as it is now: device, inode, length, last change in nanoseconds since 1970. Any
/// write, a replacement or a rename onto it gives another one.
pub type FileIdentity = (u64, u64, u64, i128);

/// One of the engine's record files that cannot be read as a record: unreadable, not JSON, not whole, not the engine's
/// own (another owner, open to others: the engine refuses those too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Damaged {
    pub path: PathBuf,
    /// `None` when not even its metadata could be read.
    pub identity: Option<FileIdentity>,
    /// The pointer to the current session (`session.v2.json`) or the record it names: the engine itself refuses every
    /// start past it (its supervisor reads it first), so it is no history to look past.
    pub current: bool,
}

/// What the engine's records say for the start gate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordScan {
    /// Every VRChat start that ended early: when its record was last written (wall clock), and the boot-clock moment it
    /// ended when the record says it (obelisk-cli 0.1.6 and later).
    pub early_ends: Vec<(std::time::SystemTime, Option<BootInstant>)>,
    pub damaged: Vec<Damaged>,
}

/// The folder of the engine's records could not be listed: nothing can be told of what ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unlistable(pub PathBuf);

/// The engine's records folder in `library`.
pub fn sessions_dir(library: &Path) -> PathBuf {
    library.join("SteamSessions")
}

/// The name of the pointer to the current session among the records.
pub const POINTER: &str = "session.v2.json";

#[cfg(unix)]
fn identity_of(meta: &std::fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino(), meta.len(), meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128)
}

#[cfg(not(unix))]
fn identity_of(meta: &std::fs::Metadata) -> FileIdentity {
    let modified = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i128);
    (0, 0, meta.len(), modified)
}

/// The identity of the file at `path` now, `None` when it cannot be told (gone, unreadable).
pub fn file_identity(path: &Path) -> Option<FileIdentity> {
    std::fs::symlink_metadata(path).ok().map(|m| identity_of(&m))
}

/// Whether the engine would take this file as its own (`journal.rs` of the fork: a plain file, of the folder's owner,
/// readable by nobody else, at most 64 KiB).
#[cfg(unix)]
fn engine_owned(meta: &std::fs::Metadata, folder: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    meta.file_type().is_file() && meta.uid() == folder.uid() && meta.permissions().mode() & 0o077 == 0 && meta.len() <= 64 * 1024
}

#[cfg(not(unix))]
fn engine_owned(meta: &std::fs::Metadata, _folder: &std::fs::Metadata) -> bool {
    meta.file_type().is_file() && meta.len() <= 64 * 1024
}

fn is_uuid(id: &str) -> bool {
    id.len() == 36 && id.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() })
}

/// Reads every record in `library` for the start gate. A record that cannot be read is no proof that nothing ended
/// early, and no reason to refuse for ever either: it is listed in [`RecordScan::damaged`] with what identifies it, so
/// that the caller tells live uncertainty (a file it has just seen) from historical damage (the same file, unchanged
/// past the wait after an early end). Another game's record and the pointer to the current session are no VRChat start;
/// a pointer that cannot be read, or that names a record that cannot, is damage the engine itself stops at.
pub fn scan_records(library: &Path) -> Result<RecordScan, Unlistable> {
    let folder = sessions_dir(library);
    let files = match std::fs::read_dir(&folder) {
        Ok(files) => files,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RecordScan::default()),
        Err(_) => return Err(Unlistable(folder)),
    };
    let folder_meta = std::fs::metadata(&folder).map_err(|_| Unlistable(folder.clone()))?;
    let mut scan = RecordScan::default();
    let mut current: Option<String> = None;
    for file in files {
        let Ok(file) = file else { return Err(Unlistable(folder)) };
        let name = file.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".v2.json") {
            continue;
        }
        let path = file.path();
        let meta = std::fs::symlink_metadata(&path);
        let identity = meta.as_ref().ok().map(identity_of);
        let damage = |scan: &mut RecordScan, current: bool| {
            scan.damaged.push(Damaged { path: path.clone(), identity, current });
        };
        let Ok(meta) = meta else {
            damage(&mut scan, name == POINTER);
            continue;
        };
        let text = if engine_owned(&meta, &folder_meta) { std::fs::read_to_string(&path).ok() } else { None };
        let Some(value) = text.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) else {
            damage(&mut scan, name == POINTER);
            continue;
        };
        if name == POINTER {
            match value.get("id").and_then(serde_json::Value::as_str).filter(|id| is_uuid(id)) {
                Some(id) => current = Some(id.to_string()),
                None => damage(&mut scan, true),
            }
            continue;
        }
        // Another game's record (its own store id, a number): no VRChat start. Without one it cannot be told whose.
        match value.get("store").and_then(|s| s.get("app_id")).and_then(serde_json::Value::as_u64) {
            Some(app) if app.to_string() != crate::vrchat_launch::STEAM_APP => continue,
            Some(_) => {}
            None => {
                damage(&mut scan, false);
                continue;
            }
        }
        let stem = name.trim_end_matches(".v2.json");
        match session_record(text.as_deref().unwrap_or_default()).filter(|r| r.id == stem) {
            Some(record) => {
                if record.ended_early() {
                    let written = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    scan.early_ends.push((written, ended_at(&value)));
                }
            }
            None => damage(&mut scan, false),
        }
    }
    if let Some(current) = current {
        let named = format!("{current}.v2.json");
        for d in &mut scan.damaged {
            if d.path.file_name().is_some_and(|n| n.to_string_lossy() == named) {
                d.current = true;
            }
        }
    }
    Ok(scan)
}

/// When a record says its game ended on the boot clock (`played.ended`, obelisk-cli 0.1.6).
fn ended_at(record: &serde_json::Value) -> Option<BootInstant> {
    boot_instant(record.get("played")?.get("ended")?)
}

/// A moment on the boot clock as the engine writes it (`{"boot": <UUID>, "seconds": <since boot>}`).
fn boot_instant(ended: &serde_json::Value) -> Option<BootInstant> {
    let boot = ended.get("boot")?.as_str().filter(|b| is_uuid(b))?;
    let seconds = ended.get("seconds")?.as_f64().filter(|s| s.is_finite() && *s >= 0.0)?;
    Some(BootInstant { boot: boot.to_ascii_uppercase(), seconds })
}

/// How long a start must still wait at `now`, after the last start that ended early at `ended` ([`AFTER_EARLY_END`]):
/// from the engine's own records, so that it holds across a restart of N4N0.
pub fn start_wait(ended: Option<std::time::SystemTime>, now: std::time::SystemTime) -> Option<std::time::Duration> {
    let since = now.duration_since(ended?).unwrap_or_default();
    AFTER_EARLY_END.checked_sub(since).filter(|wait| !wait.is_zero())
}

/// VRChat's own log folder in the engine's Wine prefix (its `crossover` user; the earlier lookup for Proton's `steamuser`
/// never finds it).
pub fn vrchat_log_dir(library: &Path) -> PathBuf {
    library.join("Prefixes/steam-438100-windows-linux/prefix/drive_c/users/crossover/AppData/LocalLow/VRChat/VRChat")
}

/// The engine's log of VRChat's runs (Wine, the window server, the graphics server), in the home `home`.
pub fn engine_log(home: &Path) -> PathBuf {
    home.join("Library/Logs/Obelisk/Steam").join(format!("wine-{}.log", crate::vrchat_launch::STEAM_APP))
}

/// Whether a stretch of the engine's log says the game's graphics went: its graphics server dropped the game's context
/// (measured 2026-11-22, three starts: 5 to 46 s after the world loaded, the game ran on without a frame).
pub fn renderer_lost(engine_log: &str) -> bool {
    engine_log.contains("VTEST_CLIENT_ERROR_CONTEXT_LOST") || engine_log.contains("npt: destroying Neptune context") // i18n: not shown (the engine's log lines)
}

/// What a line of the engine's log says of the game's graphics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsLine {
    /// The guest's driver presented this many frames on its current swapchain (each counts from its own 0).
    Presents(u64),
    /// The game made a swapchain: its counting begins.
    Swapchain,
    /// The graphics server dropped the game.
    Dropped,
}

/// What the lines of `text` say of the game's graphics, in order.
pub fn graphics_lines(text: &str) -> Vec<GraphicsLine> {
    text.lines()
        // what the engine's own log writer counts is not said again
        .filter(|line| !line.starts_with("[obelisk-log:")) // i18n: not shown (the engine's log lines)
        .filter_map(|line| match line {
            line if renderer_lost(line) => Some(GraphicsLine::Dropped),
            line if line.contains("guest swapchain: created") => Some(GraphicsLine::Swapchain), // i18n: not shown (the engine's log lines)
            line => presents_last(line).map(GraphicsLine::Presents),
        })
        .collect()
}

/// The most presents the guest driver's lines in `text` say (`neptune unixlib wsi (wsd): 6480 presents (4599 shown events)`,
/// `… alive: 6452 presents, …`): frames the game submitted, which grow behind a lock screen or a covered window too. Each
/// swapchain counts from its own 0.
pub fn presents_said(text: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.split_once("neptune unixlib wsi (wsd): ")) // i18n: not shown (the engine's log lines)
        .filter_map(|(_, rest)| rest.trim_start_matches("alive: ").split_once(" presents").and_then(|(n, _)| n.trim().parse().ok()))
        .max()
}

/// The size of the last swapchain the guest made in `text` (`guest swapchain: created 1705x1109 fmt=29 buffers=1`): the
/// pixels the game renders now, whatever percent the window server applied.
pub fn swapchain_last(text: &str) -> Option<(u32, u32)> {
    text.lines()
        .filter_map(|line| line.split_once("guest swapchain: created ")) // i18n: not shown (the engine's log lines)
        .filter_map(|(_, rest)| {
            let size = rest.split_whitespace().next()?;
            let (w, h) = size.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .rfind(|(w, h)| *w > 0 && *h > 0)
}

/// The last presents count the guest driver's lines in `text` say: where the current swapchain stands.
pub fn presents_last(text: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.split_once("neptune unixlib wsi (wsd): ")) // i18n: not shown (the engine's log lines)
        .filter_map(|(_, rest)| rest.trim_start_matches("alive: ").split_once(" presents").and_then(|(n, _)| n.trim().parse().ok()))
        .next_back()
}

/// The engine's socket of the window server of the game whose launcher is `launcher`, under the temporary folder `tmp`.
pub fn window_server_socket(tmp: &Path, launcher: u32) -> PathBuf {
    tmp.join(format!("obelisk-engine-{launcher}")).join("wsd.sock")
}

/// This person's temporary folder, whatever the environment says: where the engine makes its sessions' folders.
pub fn user_tmp() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let mut buffer = [0u8; 1024];
        // SAFETY: the buffer is this call's own and its size is said; what comes back is a C string within it.
        let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buffer.as_mut_ptr().cast(), buffer.len()) };
        if let Some(end) = buffer.iter().position(|b| *b == 0).filter(|end| n > 1 && *end > 0) {
            return PathBuf::from(String::from_utf8_lossy(&buffer[..end]).into_owned());
        }
    }
    std::env::temp_dir()
}

/// The folder of the session whose engine's launcher is `launcher` (its record's `game.pid`) for programs outside the engine (obelisk-cli 0.1.15): what the engine says of the
/// mouse, and the textures its game shares. The game does not see it.
pub fn host_folder(tmp: &Path, launcher: u32) -> PathBuf {
    tmp.join(format!("obelisk-engine-{launcher}.host"))
}

/// Where the engine says who has the mouse (voile-lkd's `wsd/wsd_pointer.h`), in a session's host folder.
pub fn pointer_channel(host: &Path) -> PathBuf {
    host.join("pointer.sock")
}

/// What `steam play` can ask of the engine for the programs outside it, for that launch alone (obelisk-cli 0.1.15): the mouse let go where VRChat does and said so on a
/// channel, and its stream camera's frames.
pub const FREE_POINTER: &str = "--free-pointer";
pub const SHARED_TEXTURES: &str = "--shared-textures";

/// Of those, what the program takes: those its `status` names among `steam play`'s options ([`Event::Status::play_options`]; one from before names none, and refuses a
/// whole start for an option it does not know).
pub fn host_options(play_options: &[String]) -> Vec<&'static str> {
    [FREE_POINTER, SHARED_TEXTURES].into_iter().filter(|o| play_options.iter().any(|p| p == o)).collect()
}

/// The first obelisk-cli N4N0 starts VRChat with (0.1.14: a paused thread resumes where it stopped, which crashed VRChat under heavy
/// loading; 0.1.13: `steam play --no-saved-arguments`, which every start is made with: the profile it names is the only one the game is given; 0.1.12: a lock file that cannot be written is held all the same, a release that
/// lost its Wine says so; 0.1.11: one lock for the whole library between every launch and every change
/// of its runtimes, a look at what runs that takes no failure for nothing, a repair compared with its archive byte for byte, and
/// every file a start needs looked at; 0.1.10: `steam play --steam-id`, the session named in `starting`).
pub const LEAST: (u32, u32, u32) = (0, 1, 14);

/// Whether the obelisk-cli at `program` is older than [`LEAST`], by its release folder's name; one whose folder names no
/// release is not known to be.
pub fn too_old(program: &Path) -> bool {
    let version = program.parent().and_then(|f| f.file_name()).and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("obelisk-cli-")?.strip_suffix("-arm64"));
    version.and_then(numbers).is_some_and(|v| v < vec![LEAST.0, LEAST.1, LEAST.2])
}

/// What a Mac has of what VRChat needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// The program is not downloaded yet.
    Missing,
    /// The program is there and has not installed VRChat (or not all of it yet).
    NoVrchat,
    Ready,
}

/// Looks at what N4N0 downloaded into `tools` and what the program installed in `library`.
pub fn engine(tools: &Path, library: &Path) -> Engine {
    match (program(tools), vrchat_install(library)) {
        (None, _) => Engine::Missing,
        (Some(_), None) => Engine::NoVrchat,
        (Some(_), Some(_)) => Engine::Ready,
    }
}

/// What a download of the game is at ([`Event::Progress`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Steam is asked what to install, or the files there are read.
    Checking,
    /// Valve's Steam components or the engine (Voile) download.
    Engine,
    /// The game's files download.
    Game,
}

/// One line of what the program says (its JSON Lines), as far as N4N0 acts on it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `status`: the Steam account signed in, if one is, and the options `steam play` takes (0.1.15 and later say them).
    Status { account: Option<String>, steam_id: Option<String>, play_options: Vec<String> },
    /// The address to show as a QR code, for the Steam app on the person's phone.
    Challenge { qr_url: String },
    SignedIn { account: String },
    Progress { phase: Phase, fraction: Option<f32> },
    Installed,
    /// The game's supervisor holds it, as this Game session (0.1.10 and later say it).
    Starting { session: Option<String> },
    /// The game runs: the engine's launcher of it, and its Game session (0.1.3 and later say it).
    Running { pid: Option<u32>, session: Option<String> },
    /// The game ended, after this many seconds and with this many frames shown, when the program knows; the Game session
    /// that held it (0.1.3 and later say it).
    Exited { code: Option<i64>, seconds: Option<u64>, frames: Option<u64>, session: Option<String> },
    /// The command failed: its stable code, a sentence in English for a log, and the engine files no game starts on when
    /// that is why (0.1.9 and later say it).
    Error { code: String, message: String, engine: Option<EngineProblem> },
    /// `engine repair`: the release downloads, or is being put in place; then it is there, whole.
    Repairing { fraction: Option<f32> },
    Repaired,
    /// Anything else the program tells.
    Other,
}

/// What ends an [`EngineProblem`], as the program says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineAction {
    /// Its release installed again (`engine repair`).
    Repair,
    /// The game that runs on them quit, then a new start.
    QuitGame,
    /// The library's folder cannot be written, or its disk takes no locks.
    CheckStorage,
    /// Anything a later program says: told as it is, with nothing to press.
    #[serde(other)]
    Other,
}

/// The engine files no game starts on (`error`'s `engine`, obelisk-cli 0.1.9; the fork's `docs/CLI.md`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineProblem {
    /// The files, by their path in the engine; none when they are not known.
    pub paths: Vec<String>,
    /// Why, in English, for a log.
    pub reason: String,
    pub action: EngineAction,
}

fn engine_problem(value: &serde_json::Value) -> Option<EngineProblem> {
    serde_json::from_value(value.clone()).ok()
}

/// Reads one line; a line that is not an event of the program gives none.
pub fn event(line: &str) -> Option<Event> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let text = |key: &str| value.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    Some(match value.get("event")?.as_str()? {
        "status" => {
            let account = |field: &str| value.get("account").and_then(|a| a.get(field)).and_then(serde_json::Value::as_str).map(str::to_owned);
            let options = value.get("play_options").and_then(serde_json::Value::as_array);
            let play_options = options.into_iter().flatten().filter_map(serde_json::Value::as_str).map(str::to_owned).collect();
            Event::Status { account: account("account_name"), steam_id: account("steam_id"), play_options }
        }
        "challenge" => Event::Challenge { qr_url: Some(text("qr_url")).filter(|url| url.starts_with("https://"))? },
        "signed_in" => Event::SignedIn { account: text("account_name") },
        "progress" => Event::Progress {
            phase: match value.get("phase").and_then(serde_json::Value::as_str) {
                Some("steam_components" | "voile") => Phase::Engine,
                Some("downloading") => Phase::Game,
                _ => Phase::Checking,
            },
            fraction: value.get("fraction").and_then(serde_json::Value::as_f64).map(|f| (f as f32).clamp(0.0, 1.0)),
        },
        "installed" => Event::Installed,
        "starting" => Event::Starting { session: value.get("session_id").and_then(serde_json::Value::as_str).map(str::to_owned) },
        "running" => Event::Running {
            pid: value.get("pid").and_then(serde_json::Value::as_u64).and_then(|pid| u32::try_from(pid).ok()),
            session: value.get("session_id").and_then(serde_json::Value::as_str).map(str::to_owned),
        },
        "exited" => Event::Exited {
            code: value.get("exit_code").and_then(serde_json::Value::as_i64),
            seconds: value.get("seconds").and_then(serde_json::Value::as_u64),
            frames: value.get("frames").and_then(serde_json::Value::as_u64),
            session: value.get("session_id").and_then(serde_json::Value::as_str).map(str::to_owned),
        },
        "error" => Event::Error { code: text("code"), message: text("message"), engine: value.get("engine").and_then(engine_problem) },
        "engine_repairing" => Event::Repairing { fraction: value.get("fraction").and_then(serde_json::Value::as_f64).map(|f| (f as f32).clamp(0.0, 1.0)) },
        "engine_repaired" => Event::Repaired,
        _ => Event::Other,
    })
}

/// VRChat's arguments for a start on a Mac, from those of a desktop start on a PC: the whole screen (Unity's own
/// options: without them the game opens in a small window, as it does at its first start anywhere), then the profile
/// (`--profile=N`, none for the default one) and the destination's link, in that order. `--no-vr` is left out (a Mac has no SteamVR to keep out of, and people who run VRChat this way
/// report starts that hang with it), and so are the PC's performance options (process priorities, video decoding by
/// the graphics card: tuned for Windows, never tried under the engine, and not offered in a Mac's settings).
pub fn game_arguments(desktop: Vec<String>) -> Vec<String> {
    let whole_screen = ["-screen-fullscreen", "1", "-window-mode", "borderless"].map(String::from);
    whole_screen.into_iter().chain(desktop.into_iter().filter(|a| a.starts_with("vrchat://") || crate::vrchat_launch::is_profile_arg(a))).collect()
}

/// The render scale of Home's Play on a Mac when the person chose none: 45 % per side, a fifth of the pixels the Mac
/// draws (the owner's choice, 2026-11-23: about 39 frames a second in VRChat Home on an M3 Air, 31 at 58 %, 19 at 100 %). The
/// bridge's own starts take none (`render_percent`).
pub const DEFAULT_RENDER_PERCENT: u32 = 45;
/// The default of before (0.3.9-dev.30 to dev.44), which the settings saved as any other value: one of those saved then is
/// [`DEFAULT_RENDER_PERCENT`], once (`settings::migrated`).
pub const EARLIER_RENDER_PERCENT: u32 = 58;
/// The range Settings offers.
pub const RENDER_PERCENTS: std::ops::RangeInclusive<u32> = 25..=100;

/// What the engine is told for a render scale of `chosen` % per side of the display's drawing resolution (`backing`
/// pixels wide), "full" (100) being the panel's real pixels (`panel` wide) and never more: the engine's own percent,
/// 1 to 100, of the drawing resolution.
pub fn render_percent(chosen: u32, backing: u32, panel: u32) -> u32 {
    let chosen = chosen.clamp(*RENDER_PERCENTS.start(), *RENDER_PERCENTS.end());
    let full = if backing > 0 && panel > 0 && panel < backing { (100 * panel / backing).max(1) } else { 100 };
    chosen.min(full)
}

/// The size the game draws at for `chosen` % on a display drawing `backing` pixels (width, height) on a panel of
/// `panel` pixels, by the engine's own rules (voile-lkd `wsd.m`, `scale_at`): its percent of the drawing scale, never
/// below what keeps the desktop at 640 x 480 pixels and never above the display's scale; within a pixel.
pub fn render_size(chosen: u32, backing: (u32, u32), panel: (u32, u32)) -> (u32, u32) {
    let percent = render_percent(chosen, backing.0, panel.0) as f64 / 100.0;
    let least = if backing.0 > 0 && backing.1 > 0 { (640.0 / backing.0 as f64).max(480.0 / backing.1 as f64) } else { 0.0 };
    let share = percent.max(least).min(1.0);
    ((backing.0 as f64 * share).round() as u32, (backing.1 as f64 * share).round() as u32)
}

/// The percent the engine applies for `chosen` (what Settings shows beside the size): the panel's cap, then the 640 x 480
/// floor, as [`render_size`].
pub fn effective_percent(chosen: u32, backing: (u32, u32), panel: (u32, u32)) -> u32 {
    let (w, _) = render_size(chosen, backing, panel);
    if backing.0 == 0 { render_percent(chosen, backing.0, panel.0) } else { ((w as f64 * 100.0 / backing.0 as f64).round() as u32).clamp(1, 100) }
}

/// Whether the obelisk-cli at `program` takes `steam play --render-percent` (0.1.7 and later): an older one refuses
/// the whole start for an option it does not know. Told by its folder's name (`obelisk-cli-<version>-arm64`).
pub fn takes_render_percent(program: &Path) -> bool {
    program
        .parent()
        .and_then(|folder| folder.file_name())
        .and_then(|name| numbers(name.to_str()?.strip_prefix("obelisk-cli-")?.strip_suffix("-arm64")?))
        .is_some_and(|version| version >= vec![0, 1, 7])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This Mac (2026-11-23): a 2560 x 1664 panel drawn at 2940 x 1912. 58 % is a third of the pixels; "full" is the
    /// panel's 87 %, never the 100 % that draws more than the panel shows.
    #[test]
    fn the_render_scale_is_a_share_of_the_drawing_capped_at_the_panel() {
        let (backing, panel) = ((2940, 1912), (2560, 1664));
        assert_eq!(render_percent(58, backing.0, panel.0), 58);
        assert_eq!(render_size(58, backing, panel), (1705, 1109));
        let (w, h) = render_size(DEFAULT_RENDER_PERCENT, backing, panel);
        assert_eq!((DEFAULT_RENDER_PERCENT, render_percent(DEFAULT_RENDER_PERCENT, backing.0, panel.0), w, h), (45, 45, 1323, 860));
        assert_eq!((w * h * 100 + 50 * 2940 * 1912 / 100) / (2940 * 1912), 20, "a fifth of the pixels");
        assert_eq!(render_percent(100, backing.0, panel.0), 87);
        assert_eq!(render_size(100, backing, panel), (2558, 1663));
        assert_eq!(render_percent(90, backing.0, panel.0), 87, "above the panel is the panel");
        assert_eq!(render_percent(10, backing.0, panel.0), 25, "the lowest offered");
        assert_eq!(render_percent(100, 1920, 1920), 100, "a display drawn at its own pixels");
        assert_eq!(render_percent(100, 1920, 0), 100, "a panel that says nothing");
        // Fourth verification, finding 12: the label shows what the engine makes. 87 to 100 are one size on this panel
        // (and say 87 %); the engine keeps the desktop at 640 x 480 at least: 25 % of 1920 x 1080 is 853 x 480.
        for chosen in 87..=100 {
            assert_eq!(render_size(chosen, backing, panel), (2558, 1663), "{chosen}");
            assert_eq!(effective_percent(chosen, backing, panel), 87, "{chosen}");
        }
        assert_eq!(render_size(25, (1920, 1080), (1920, 1080)), (853, 480));
        assert_eq!(effective_percent(25, (1920, 1080), (1920, 1080)), 44);
        assert_eq!(render_size(25, backing, panel), (738, 480));
        assert_eq!(render_size(58, (1920, 1080), (1920, 1080)), (1114, 626), "above the floor: as asked");
    }

    /// The mouse and the camera are asked of an engine that says it takes it, each by itself: never by its version.
    #[test]
    fn what_a_start_asks_of_the_engine_is_what_its_status_names() {
        let options = |line: &str| match event(line) {
            Some(Event::Status { play_options, .. }) => host_options(&play_options),
            other => panic!("{other:?}"),
        };
        assert_eq!(options(r#"{"event":"status","version":"0.1.14","account":null,"keychain":"ok"}"#), [""; 0], "an older one says nothing of it");
        let said = r#"{"event":"status","version":"0.1.15","account":null,"play_options":["--steam-id","--render-percent","--no-saved-arguments","--free-pointer","--shared-textures"]}"#;
        assert_eq!(options(said), ["--free-pointer", "--shared-textures"]);
        assert_eq!(options(r#"{"event":"status","version":"9.9.9","account":null,"play_options":["--shared-textures",4,"--other"]}"#), ["--shared-textures"]);
        assert_eq!(options(r#"{"event":"status","version":"9.9.9","account":null,"play_options":"--free-pointer"}"#), [""; 0], "no list");
    }

    #[test]
    fn an_obelisk_cli_older_than_the_least_starts_no_vrchat() {
        let at = |folder: &str| PathBuf::from(format!("/T/obelisk-cli/{folder}/obelisk-cli"));
        for old in ["obelisk-cli-0.1.13-arm64", "obelisk-cli-0.1.12-arm64", "obelisk-cli-0.1.11-arm64", "obelisk-cli-0.1.10-arm64", "obelisk-cli-0.1.9-arm64", "obelisk-cli-0.1.8-arm64", "obelisk-cli-0.1.2-arm64", "obelisk-cli-0.0.99-arm64"] {
            assert!(too_old(&at(old)), "{old}");
        }
        for fit in ["obelisk-cli-0.1.14-arm64", "obelisk-cli-0.1.15-arm64", "obelisk-cli-0.2.0-arm64", "obelisk-cli-1.0.0-arm64", "built-here"] {
            assert!(!too_old(&at(fit)), "{fit}");
        }
    }

    #[test]
    fn only_an_obelisk_cli_from_0_1_7_is_given_a_render_percent() {
        let at = |v: &str| PathBuf::from(format!("/tools/obelisk-cli-{v}-arm64/obelisk-cli"));
        assert!(!takes_render_percent(&at("0.1.6")));
        assert!(takes_render_percent(&at("0.1.7")));
        assert!(takes_render_percent(&at("0.2.0")));
        assert!(!takes_render_percent(Path::new("/usr/local/bin/obelisk-cli")));
    }

    const RECEIPTS: &str = r#"[
        {"appId": 252950, "language": "english", "name": "Rocket League", "directory": "/Games/Rocket League", "platform": "windows", "totalBytes": 1},
        {"appId": 438100, "language": "english", "name": "VRChat", "directory": "/Games/VRChat", "platform": "windows", "totalBytes": 2}
    ]"#;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("orbiters-obelisk-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Temp(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn release(version: &str) -> String {
        let at = format!("https://github.com/Orbiters-cc/Obelisk/releases/download/cli-v{version}");
        format!(
            r#"{{"tag_name": "cli-v{version}", "prerelease": true, "assets": [
            {{"name": "obelisk-cli-{version}-arm64.tar.gz", "size": 11295267, "browser_download_url": "{at}/obelisk-cli-{version}-arm64.tar.gz"}},
            {{"name": "obelisk-cli-{version}-arm64.tar.gz.sha256", "size": 97, "browser_download_url": "{at}/obelisk-cli-{version}-arm64.tar.gz.sha256"}}
        ]}}"#
        )
    }

    #[test]
    fn the_newest_release_names_the_archive_to_download() {
        let list = format!(r#"[{}, {}, {{"tag_name": "v0.6.4", "assets": []}}, {}]"#, release("0.9.3"), release("0.10.0"), release("0.1.0"));
        let download = latest_download(&list).unwrap();
        assert_eq!(download.version, "0.10.0", "by its numbers, not as text");
        assert_eq!(download.name, "obelisk-cli-0.10.0-arm64.tar.gz");
        assert_eq!(download.folder(), "obelisk-cli-0.10.0-arm64");
        assert!(download.checksum_url.ends_with("/obelisk-cli-0.10.0-arm64.tar.gz.sha256"));
        assert_eq!(latest_download("not json"), None);
        assert_eq!(latest_download("[]"), None);
        let one = format!("[{}]", release("0.1.0"));
        assert!(latest_download(&one).is_some());
        assert_eq!(latest_download(&one.replace(".tar.gz.sha256", ".tar.gz.md5")), None, "no checksum, no download");
        assert_eq!(latest_download(&one.replace("https://github.com/Orbiters-cc/", "https://github.com/someone-else/")), None, "only from the fork's releases");
        assert_eq!(latest_download(&one.replace("11295267", "99999999999")), None, "not an archive of that size");
        assert_eq!(latest_download(&one.replace("\"prerelease\": true", "\"draft\": true")), None, "a draft is not out");
        assert_eq!(latest_download(&one.replace("cli-v0.1.0\"", "cli-v../x\"")), None);

        let sum = "1684f0cdea69a38770e70a934f75054454d9bd16f79571e0074292e11dec330c";
        assert_eq!(checksum(&format!("{sum}  obelisk-cli-0.1.0-arm64.tar.gz\n")).as_deref(), Some(sum));
        assert_eq!(checksum(&sum.to_uppercase()).as_deref(), Some(sum));
        assert_eq!(checksum("<html>Not Found</html>"), None);
        assert_eq!(checksum(""), None);
    }

    #[test]
    fn a_receipt_file_says_where_vrchat_is() {
        assert_eq!(vrchat_directories(RECEIPTS), [PathBuf::from("/Games/VRChat")]);
        assert!(vrchat_directories("not json").is_empty());
        assert!(vrchat_directories(r#"{"appId": 438100, "directory": "/x"}"#).is_empty(), "an array, or nothing");
        assert!(vrchat_directories(r#"[{"appId": 438100}, {"appId": "438100", "directory": "/x"}, {"appId": 438100, "directory": ""}]"#).is_empty());
        assert!(is_receipt_file("steam-installations-76561198000000000.json"));
        assert!(!is_receipt_file("steam-installations-76561198000000000.incomplete.json"), "a download that did not finish is no installation");
        assert!(!is_receipt_file("epic-installations-1.json"));
    }

    #[test]
    fn the_mac_has_the_program_and_vrchat_or_says_which_is_missing() {
        let temp = Temp::new("engine");
        let (tools, library) = (tools(&temp.0), temp.0.join("library"));
        assert_eq!(engine(&tools, &library), Engine::Missing);
        // An older release, whole, and a newer one whose download was cut short.
        for (version, whole) in [("0.9.0", true), ("0.10.0", false)] {
            let folder = tools.join(format!("obelisk-cli-{version}-arm64"));
            std::fs::create_dir_all(folder.join("SteamNative")).unwrap();
            std::fs::write(folder.join("obelisk-cli"), b"").unwrap();
            if whole {
                std::fs::write(folder.join("SteamNative").join("obelisk-steam-host"), b"").unwrap();
            }
        }
        assert_eq!(program(&tools), Some(tools.join("obelisk-cli-0.9.0-arm64").join("obelisk-cli")), "only a whole release is run");
        std::fs::write(tools.join("obelisk-cli-0.10.0-arm64").join("SteamNative").join("obelisk-steam-host"), b"").unwrap();
        assert_eq!(program(&tools), Some(tools.join("obelisk-cli-0.10.0-arm64").join("obelisk-cli")), "the newest by its numbers");
        assert_eq!(engine(&tools, &library), Engine::NoVrchat);

        std::fs::create_dir_all(&library).unwrap();
        let game = temp.0.join("Games").join("VRChat");
        let receipt = serde_json::json!([{"appId": 438100, "name": "VRChat", "directory": game, "platform": "windows", "totalBytes": 2}]).to_string();
        std::fs::write(library.join("steam-installations-42.json"), &receipt).unwrap();
        assert_eq!(engine(&tools, &library), Engine::NoVrchat, "a receipt whose folder is gone is no game to play");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("start_protected_game.exe"), b"").unwrap();
        assert_eq!(vrchat_install(&library), Some(game.clone()));
        assert_eq!(engine(&tools, &library), Engine::Ready);
        std::fs::remove_file(library.join("steam-installations-42.json")).unwrap();
        std::fs::write(library.join("steam-installations-42.incomplete.json"), &receipt).unwrap();
        assert_eq!(engine(&tools, &library), Engine::NoVrchat, "still downloading");
    }

    #[test]
    fn a_session_record_names_the_running_vrchat() {
        let record = |app: u32, phase: &str, game: &str| format!(r#"{{"version":2,"id":"f2766c7f","account_id":"1","store":{{"app_id":{app},"host":"running"}},"phase":"{phase}","supervisor":{{"pid":48097,"uid":501}},"game":{game},"starting":null,"socket":"/tmp/x/control"}}"#);
        assert_eq!(session_game(&record(438100, "Running", r#"{"pid":48200,"uid":501,"executable":"/x/wine"}"#)), Some(48200));
        assert_eq!(session_game(&record(438100, "Finished", r#"{"pid":48200,"uid":501}"#)), None, "as the engine leaves it once the game closed");
        assert_eq!(session_game(&record(438100, "Failed", r#"{"pid":48200,"uid":501}"#)), None);
        assert_eq!(session_game(&record(438100, "Running", "null")), None, "not started yet");
        assert_eq!(session_game(&record(252950, "Running", r#"{"pid":48200,"uid":501}"#)), None, "another game");
        assert_eq!(session_game(r#"{"version":2,"id":"f2766c7f"}"#), None, "the pointer to the last session");
        assert_eq!(session_game("not json"), None);
    }

    #[test]
    fn the_running_games_come_with_their_recorded_birth() {
        let temp = Temp::new("born");
        assert_eq!(session_games_born(&temp.0), Some(Vec::new()));
        let sessions = sessions_dir(&temp.0);
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("a.v2.json"), r#"{"id":"a","store":{"app_id":438100},"phase":"Running","game":{"pid":33466,"birth_seconds":1795387812,"birth_microseconds":431}}"#).unwrap();
        std::fs::write(sessions.join("b.v2.json"), r#"{"id":"b","store":{"app_id":438100},"phase":"Running","game":{"pid":4242}}"#).unwrap();
        std::fs::write(sessions.join("c.v2.json"), r#"{"id":"c","store":{"app_id":438100},"phase":"Finished","game":{"pid":5}}"#).unwrap();
        let mut games = session_games_born(&temp.0).unwrap();
        games.sort();
        assert_eq!(games, [(4242, None), (33466, Some((1795387812, 431)))]);
    }

    #[test]
    fn a_record_says_its_session_and_whether_it_ended_early() {
        // As the engine wrote them on 2026-11-22: a start whose graphics never came up (100 s, 0 frames), one played, one running.
        let crashed = r#"{"version":2,"id":"6da7c40d-9fd2-4f3b-8770-aadffa2d0088","store":{"app_id":438100,"host":"stopped"},"phase":"Finished","played":{"seconds":100,"frames":{"count":0}},"game":null}"#;
        let played = r#"{"version":2,"id":"f790b579","store":{"app_id":438100},"phase":"Finished","played":{"seconds":511,"frames":{"count":11500}},"game":null}"#;
        let running = r#"{"version":2,"id":"f2931045","store":{"app_id":438100},"phase":"Running","played":null,"game":{"pid":12777}}"#;
        let crashed = session_record(crashed).unwrap();
        assert_eq!((crashed.id.as_str(), crashed.frames, crashed.seconds), ("6da7c40d-9fd2-4f3b-8770-aadffa2d0088", Some(0), Some(100)));
        assert!(crashed.ended_early());
        assert!(!session_record(played).unwrap().ended_early());
        let running = session_record(running).unwrap();
        assert_eq!(running.game, Some(12777));
        assert!(!running.over() && !running.ended_early());
        assert_eq!(session_record(r#"{"version":2,"id":"x"}"#), None, "the pointer to the last session");
        assert_eq!(session_record(&running_other_app()), None);
    }

    #[test]
    fn a_record_says_its_game_s_birth_its_frames_so_far_and_an_unknown_count() {
        let running = r#"{"version":2,"id":"261267fa-b5be-4f91-87c2-e3df42fef46e","store":{"app_id":438100},"phase":"Running","game":{"pid":33466,"uid":501,"birth_seconds":1795387812,"birth_microseconds":431},"shown":{"frames":4594,"at":1795388231,"continuous":true}}"#;
        let r = session_record(running).unwrap();
        assert_eq!(r.game_birth, Some((1795387812, 431)));
        assert_eq!(r.shown, Some(Shown { frames: 4594, at: 1795388231, continuous: true }));
        // 0.1.5: a log that could not be read is an unknown count, not a start that showed nothing.
        let unread = r#"{"version":2,"id":"a","store":{"app_id":438100},"phase":"Finished","played":{"seconds":900,"frames":{"count":0,"measured":false}}}"#;
        let unread = session_record(unread).unwrap();
        assert_eq!(unread.frames, None);
        assert!(!unread.ended_early(), "900 s and an unknown count: played");
        let old = r#"{"version":2,"id":"a","store":{"app_id":438100},"phase":"Finished","played":{"seconds":100,"frames":{"count":0}}}"#;
        assert!(session_record(old).unwrap().ended_early(), "an older record's 0 is a count");
        // M8: an hour with a log that reported no frame is a game played, not an early end (no 80 s wait after it).
        let long = r#"{"version":2,"id":"a","store":{"app_id":438100},"phase":"Finished","played":{"seconds":3600,"frames":{"count":0,"measured":true}}}"#;
        assert!(!session_record(long).unwrap().ended_early());
        assert!(early(Some(599), Some(0)) && !early(Some(600), Some(0)) && early(Some(10), None) && !early(None, None));
    }

    #[test]
    fn one_record_is_read_by_its_uuid_and_an_unreadable_one_is_not_none() {
        let temp = Temp::new("record-of");
        let id = "261267fa-b5be-4f91-87c2-e3df42fef46e";
        assert_eq!(session_record_of(&temp.0, id), Ok(None), "no folder yet");
        let sessions = temp.0.join("SteamSessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join(format!("{id}.v2.json")), format!(r#"{{"id":"{id}","store":{{"app_id":438100}},"phase":"Running"}}"#)).unwrap();
        assert_eq!(session_record_of(&temp.0, id).unwrap().unwrap().phase, "Running");
        std::fs::write(sessions.join(format!("{id}.v2.json")), r#"{"id":"#).unwrap();
        assert_eq!(session_record_of(&temp.0, id), Err(Unreadable), "half written: ask again");
        assert_eq!(session_record_of(&temp.0, "../../etc/passwd"), Ok(None), "only a UUID names a record");
    }

    /// Writes a record as the engine does: readable by its owner only.
    fn record_file(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    }

    /// B3: every file that cannot be read as a record is listed with what identifies it, the pointer to the current
    /// session and the record it names marked as the engine's own stop; another game's record and a played game are
    /// nothing; a record parseable but not whole (`{}`, no id, another id than its name) is damage, not "nothing".
    #[test]
    fn records_that_cannot_be_read_are_listed_as_damage_never_as_nothing() {
        let temp = Temp::new("gate-unknown");
        assert_eq!(scan_records(&temp.0), Ok(RecordScan::default()), "no folder: nothing ever ran");
        let sessions = sessions_dir(&temp.0);
        std::fs::create_dir_all(&sessions).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let current = "c29c0b3a-9cd4-4ca7-aeeb-e69195577759";
        record_file(&sessions, POINTER, &format!(r#"{{"version":2,"id":"{current}"}}"#));
        record_file(&sessions, "f790b579-a9d4-4aa7-bb99-f3d545473689.v2.json", r#"{"id":"f790b579-a9d4-4aa7-bb99-f3d545473689","store":{"app_id":438100},"phase":"Finished","played":{"seconds":600,"frames":{"count":9000}}}"#);
        record_file(&sessions, "0e20b8b9-56f0-4736-8a72-602dbef7fbcd.v2.json", r#"{"id":"0e20b8b9-56f0-4736-8a72-602dbef7fbcd","store":{"app_id":252950},"phase":"Running"}"#);
        record_file(&sessions, "owner.lock", "");
        assert_eq!(scan_records(&temp.0), Ok(RecordScan::default()), "the pointer, a played game, another game's, a lock");
        // An early end, then on the boot clock (0.1.6).
        record_file(&sessions, "6da7c40d-9fd2-4f3b-8770-aadffa2d0088.v2.json", r#"{"id":"6da7c40d-9fd2-4f3b-8770-aadffa2d0088","store":{"app_id":438100},"phase":"Finished","played":{"seconds":40,"frames":{"count":96},"ended":{"boot":"11466e1d-bf8e-4060-a996-1c47be650634","seconds":5000.5}}}"#);
        let ends = scan_records(&temp.0).unwrap().early_ends;
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0].1, Some(BootInstant { boot: "11466E1D-BF8E-4060-A996-1C47BE650634".into(), seconds: 5000.5 }));
        // Damage of every kind.
        let broken = record_file(&sessions, "41596d78-6dc1-4715-b648-03836dc11ccc.v2.json", r#"{"id":"#);
        let empty = record_file(&sessions, "46210637-b4ba-4c9b-bd9b-97e1cfde3b37.v2.json", "{}");
        let renamed = record_file(&sessions, "47b08b35-0ae2-4bc5-8d01-6b2186eced87.v2.json", r#"{"id":"55731a97-1735-4171-ad06-0c5be2799b87","store":{"app_id":438100},"phase":"Finished"}"#);
        let named = record_file(&sessions, &format!("{current}.v2.json"), "not json");
        let scan = scan_records(&temp.0).unwrap();
        let mut found: Vec<(PathBuf, bool)> = scan.damaged.iter().map(|d| (d.path.clone(), d.current)).collect();
        found.sort();
        let mut want = vec![(broken.clone(), false), (empty, false), (renamed, false), (named.clone(), true)];
        want.sort();
        assert_eq!(found, want);
        assert!(scan.damaged.iter().all(|d| d.identity.is_some()));
        // Its identity changes with its content.
        let before = file_identity(&broken);
        std::thread::sleep(std::time::Duration::from_millis(5));
        record_file(&sessions, "41596d78-6dc1-4715-b648-03836dc11ccc.v2.json", r#"{"id":"x"#);
        assert_ne!(file_identity(&broken), before);
        // A pointer that is not one is the engine's stop too.
        record_file(&sessions, POINTER, "garbage");
        assert!(scan_records(&temp.0).unwrap().damaged.iter().any(|d| d.path.ends_with(POINTER) && d.current));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Open to others: the engine refuses it, so it is damage here too.
            std::fs::remove_file(&named).unwrap();
            let open = record_file(&sessions, "70c7a83d-c53a-4b76-8682-f36f83f4011e.v2.json", r#"{"id":"70c7a83d-c53a-4b76-8682-f36f83f4011e","store":{"app_id":438100},"phase":"Finished","played":{"seconds":40,"frames":{"count":96}}}"#);
            std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(scan_records(&temp.0).unwrap().damaged.iter().any(|d| d.path == open));
            std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o000)).unwrap();
            let unreadable = scan_records(&temp.0);
            std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(unreadable, Err(Unlistable(sessions.clone())), "a folder that cannot be listed");
        }
    }

    fn running_other_app() -> String {
        r#"{"version":2,"id":"y","store":{"app_id":252950},"phase":"Running","game":{"pid":5}}"#.into()
    }

    #[test]
    fn a_start_waits_80_s_after_an_early_end_whoever_starts_it() {
        use std::time::{Duration, SystemTime};
        let ended = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(start_wait(None, ended), None, "nothing ended early");
        assert_eq!(start_wait(Some(ended), ended + Duration::from_secs(10)), Some(Duration::from_secs(70)));
        assert_eq!(start_wait(Some(ended), ended + Duration::from_secs(80)), None);
        assert_eq!(start_wait(Some(ended), ended - Duration::from_secs(5)), Some(Duration::from_secs(80)), "a clock set back waits the whole time");
        // From the records themselves: it holds across a restart of N4N0.
        let temp = Temp::new("gate");
        let sessions = temp.0.join("SteamSessions");
        std::fs::create_dir_all(&sessions).unwrap();
        assert_eq!(session_games(&temp.0), Some(Vec::new()));
        record_file(&sessions, "6da7c40d-9fd2-4f3b-8770-aadffa2d0088.v2.json", r#"{"id":"6da7c40d-9fd2-4f3b-8770-aadffa2d0088","store":{"app_id":438100},"phase":"Finished","played":{"seconds":40,"frames":{"count":96}}}"#);
        let written = scan_records(&temp.0).unwrap().early_ends[0].0;
        let wait = start_wait(Some(written), SystemTime::now()).expect("an early end just now");
        assert!(wait > Duration::from_secs(70));
        assert_eq!(session_games(&temp.0.join("nothing-here")), Some(Vec::new()), "no folder: no game ever ran");
    }

    #[test]
    fn the_engine_s_log_is_read_in_the_order_it_says_things() {
        use GraphicsLine::{Dropped, Presents, Swapchain};
        let text = "neptune unixlib wsi (wsd): 60 presents (0 shown events)\nsomething else\nnpt: destroying Neptune context 1\n[obelisk-log: 3 lines like this: VTEST_CLIENT_ERROR_CONTEXT_LOST]\nneptune unixlib wsi (wsd): alive: 61 presents, x\nclient: VTEST_CLIENT_ERROR_CONTEXT_LOST\n";
        assert_eq!(graphics_lines(text), [Presents(60), Dropped, Presents(61), Dropped]);
        assert_eq!(graphics_lines("neptune: guest swapchain: created 1705x1109 fmt=28 buffers=2 hwnd=0000000000030070\nneptune unixlib wsi (wsd): 1 presents (0 shown events), last 1\n"), [Swapchain, Presents(1)]);
        assert_eq!(graphics_lines(""), []);
    }

    #[test]
    fn the_engine_s_log_says_when_the_graphics_went() {
        assert!(renderer_lost("neptune: ring fatal error\nclient: VTEST_CLIENT_ERROR_CONTEXT_LOST\n"));
        assert!(renderer_lost("Nov 22 20:53:33  virgl_render_server[19563] <Debug>: npt: destroying Neptune context 1"));
        assert!(!renderer_lost("wsd: window 1 overlay: frame held 100.0 ms"));
        assert_eq!(pointer_channel(&host_folder(Path::new("/var/folders/x/T"), 12777)), PathBuf::from("/var/folders/x/T/obelisk-engine-12777.host/pointer.sock"));
        assert_eq!(window_server_socket(Path::new("/var/folders/x/T"), 12777), PathBuf::from("/var/folders/x/T/obelisk-engine-12777/wsd.sock"));
        assert!(vrchat_log_dir(Path::new("/L")).ends_with("users/crossover/AppData/LocalLow/VRChat/VRChat"));
    }

    #[test]
    fn the_program_s_lines_are_read_as_what_n4n0_acts_on() {
        assert_eq!(event(r#"{"event":"status","version":"0.1.0","account":null,"keychain":"ok"}"#), Some(Event::Status { account: None, steam_id: None, play_options: Vec::new() }));
        assert_eq!(event(r#"{"event":"status","account":{"steam_id":"7656","account_name":"someone"}}"#), Some(Event::Status { account: Some("someone".into()), steam_id: Some("7656".into()), play_options: Vec::new() }));
        assert_eq!(event(r#"{"event":"challenge","qr_url":"https://s.team/q/1/2"}"#), Some(Event::Challenge { qr_url: "https://s.team/q/1/2".into() }));
        assert_eq!(event(r#"{"event":"challenge","qr_url":"javascript:alert(1)"}"#), None, "only a web address is ever shown as a code");
        assert_eq!(event(r#"{"event":"signed_in","steam_id":"7656","account_name":"someone"}"#), Some(Event::SignedIn { account: "someone".into() }));
        assert_eq!(event(r#"{"event":"progress","app_id":438100,"phase":"voile","bytes_done":null,"bytes_total":null,"fraction":0.25}"#), Some(Event::Progress { phase: Phase::Engine, fraction: Some(0.25) }));
        assert_eq!(event(r#"{"event":"progress","app_id":438100,"phase":"downloading","bytes_done":5,"bytes_total":0,"fraction":null}"#), Some(Event::Progress { phase: Phase::Game, fraction: None }));
        assert_eq!(event(r#"{"event":"progress","app_id":438100,"phase":"verifying","fraction":7.0}"#), Some(Event::Progress { phase: Phase::Checking, fraction: Some(1.0) }));
        assert_eq!(event(r#"{"event":"installed","app_id":438100}"#), Some(Event::Installed));
        assert_eq!(event(r#"{"event":"starting","app_id":438100}"#), Some(Event::Starting { session: None }));
        assert_eq!(event(r#"{"event":"starting","app_id":438100,"session_id":"f2931045-90d3-4302-a8a9-94021747c54f"}"#), Some(Event::Starting { session: Some("f2931045-90d3-4302-a8a9-94021747c54f".into()) }));
        assert_eq!(event(r#"{"event":"running","app_id":438100,"pid":4242}"#), Some(Event::Running { pid: Some(4242), session: None }));
        assert_eq!(event(r#"{"event":"running","app_id":438100,"pid":12777,"session_id":"f2931045-90d3-4302-a8a9-94021747c54f"}"#), Some(Event::Running { pid: Some(12777), session: Some("f2931045-90d3-4302-a8a9-94021747c54f".into()) }));
        assert_eq!(event(r#"{"event":"running","app_id":438100,"pid":null}"#), Some(Event::Running { pid: None, session: None }));
        assert_eq!(event(r#"{"event":"exited","app_id":438100,"exit_code":0,"exit_signal":null,"seconds":100}"#), Some(Event::Exited { code: Some(0), seconds: Some(100), frames: None, session: None }));
        assert_eq!(event(r#"{"event":"exited","app_id":438100,"session_id":"x","frames":0,"exit_code":0,"seconds":100}"#), Some(Event::Exited { code: Some(0), seconds: Some(100), frames: Some(0), session: Some("x".into()) }));
        assert_eq!(event(r#"{"event":"exited","app_id":438100,"session_id":"x","frames":null,"exit_code":0,"seconds":900}"#), Some(Event::Exited { code: Some(0), seconds: Some(900), frames: None, session: Some("x".into()) }), "0.1.5: an unknown count is null");
        assert_eq!(event(r#"{"event":"error","code":"not_signed_in","message":"No Steam account is signed in.","key":null}"#), Some(Event::Error { code: "not_signed_in".into(), message: "No Steam account is signed in.".into(), engine: None }));
        let refused = r#"{"event":"error","code":"engine_files","message":"m","key":"backend.runtime.engineFiles","engine":{"paths":["engine/wsd"],"reason":"why","action":"repair"}}"#;
        let problem = |action| EngineProblem { paths: vec!["engine/wsd".into()], reason: "why".into(), action };
        assert_eq!(event(refused), Some(Event::Error { code: "engine_files".into(), message: "m".into(), engine: Some(problem(EngineAction::Repair)) }));
        assert!(matches!(event(&refused.replace("repair\"", "quit_game\"")), Some(Event::Error { engine: Some(p), .. }) if p == problem(EngineAction::QuitGame)));
        assert!(matches!(event(&refused.replace("repair\"", "something_new\"")), Some(Event::Error { engine: Some(p), .. }) if p == problem(EngineAction::Other)));
        assert!(matches!(event(r#"{"event":"error","code":"engine_files","message":"m","engine":"half"}"#), Some(Event::Error { engine: None, .. })));
        assert_eq!(event(r#"{"event":"engine_repairing","bytes_done":1,"bytes_total":2,"fraction":0.5}"#), Some(Event::Repairing { fraction: Some(0.5) }));
        assert_eq!(event(r#"{"event":"engine_repaired","voile":"v1","keyboard_layout":"mac","engine_files":[]}"#), Some(Event::Repaired));
        assert_eq!(event(r#"{"event":"voile_update","version":"v1"}"#), Some(Event::Other));
        assert_eq!(event("warning: something on the wrong stream"), None);
        assert_eq!(event(r#"{"no":"event"}"#), None);
    }

    #[test]
    fn a_mac_s_start_is_given_the_whole_screen_and_the_destination() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let whole = ["-screen-fullscreen", "1", "-window-mode", "borderless"];
        assert_eq!(game_arguments(args(&["--no-vr", "--process-priority=1", "--fps=90", "vrchat://launch?ref=vrchat.com&id=wrld_1:2"])), args(&[whole.as_slice(), &["vrchat://launch?ref=vrchat.com&id=wrld_1:2"]].concat()));
        assert_eq!(game_arguments(args(&["--no-vr"])), args(&whole));
    }

    /// One profile at a time on a Mac: VRChat gets exactly `--profile=N` for one that is not the default, nothing for the
    /// default, before the link; none of the PC's options with it.
    #[test]
    fn a_mac_s_arguments_are_the_whole_screen_the_profile_and_the_link() {
        use crate::vrchat_launch::{arguments_on, LaunchMode, LaunchSettings, PROFILE_MAX};
        let (world, instance) = ("wrld_00000000-0000-4000-8000-0000000000aa", "7~friends(usr_x)");
        let screen = ["-screen-fullscreen", "1", "-window-mode", "borderless"].map(String::from).to_vec();
        for profile in 0..=PROFILE_MAX {
            let own: Vec<String> = (profile != 0).then(|| format!("--profile={profile}")).into_iter().collect();
            let plain = game_arguments(arguments_on(LaunchMode::Desktop, &LaunchSettings::default(), Vec::new(), profile, None));
            assert_eq!(plain, [screen.clone(), own.clone()].concat());
            let window = vec!["-screen-width".to_string(), "1280".into()];
            let there = game_arguments(arguments_on(LaunchMode::Desktop, &LaunchSettings::default(), window, profile, Some((world, instance))));
            assert_eq!(there, [screen.clone(), own, vec![crate::vrchat_launch::link(world, instance)]].concat());
        }
        // Whatever else is handed in is not passed on, however much it looks like a profile.
        let odd = ["--no-vr", "--profile=0", "--profile=16", "--profile=1 --osc=1:2:3", "--profile", "--fps=90"].map(String::from).to_vec();
        assert_eq!(game_arguments(odd), screen);
    }

    /// Finding 10 and R1: the size the game renders now is its last swapchain's; presents restart with each swapchain.
    #[test]
    fn the_last_swapchain_and_presents_are_read_from_the_engine_s_log() {
        let log = "guest swapchain: created 2940x1912 fmt=29 buffers=2\nneptune unixlib wsi (wsd): 9000 presents (0 shown events)\nguest swapchain: created 1705x1109 fmt=29 buffers=1\nneptune unixlib wsi (wsd): 60 presents (0 shown events)\n";
        assert_eq!(swapchain_last(log), Some((1705, 1109)));
        assert_eq!(swapchain_last("guest swapchain: created 0x0 fmt=1"), None);
        assert_eq!(swapchain_last("nothing"), None);
        assert_eq!(presents_last(log), Some(60));
        assert_eq!(presents_said(log), Some(9000));
    }

}
