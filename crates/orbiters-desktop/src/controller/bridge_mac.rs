//! The VRChat bridge's game on a Mac (`docs/vrchat-bridge.md`, "On a Mac"): VRChat under Obelisk's engine, started through
//! the start Home's Play shares (`obelisk_cli::Play`, `MacMachine`), and known by that one launch only.
//!
//! - **One verified game.** What the bridge reads is bound to its own launch: the launch id the shared start gave, the Game
//!   session (its UUID, from the program or the engine's record) and the engine's launcher process with its birth time.
//!   Its log is the one VRChat began after that launch whose own argument list names the instance made for it; its window
//!   is the engine window server's (the launcher's own child `wsd`). A VRChat started from Home, or found after a restart
//!   of the service, is the person's: seen as running, never read, looked at or closed.
//! - **Start and renderer evidence.** A join in the log is not a working game (2026-11-22 19:54: the graphics went 7 s in and
//!   VRChat still logged its join). Arrival needs the game's window on screen, and the engine's log is followed from the
//!   launch on: its graphics server dropping the game (`VTEST_CLIENT_ERROR_CONTEXT_LOST`) is `renderer_lost`.
//! - **Close is graceful and session-specific:** `steam quit <app> --session <uuid>` (obelisk-cli 0.1.3), never `--force`.
//! - Everything else (instances, worlds, labels, publishing, reconstructions) is the controller's, as on Windows
//!   ([`RealGame`]); input, OSC and 3D pictures are refused before they get here by the bridge's capabilities.

use super::bridge_game::RealGame;
use super::{Controller, Shared};
use crate::bridge::game::{CaptureSeen, Failure, Find, Game, GameWindow, Identity, LaunchPlan, OscArg, PictureFiles, Seen, Shot, WorldPage};
use crate::bridge::input::Input;
use crate::bridge::mac::{self as native, MacWindow};
use orbiters_core::obelisk;
use orbiters_core::settings::{Photo3dLength, PictureMode};
use orbiters_core::vrchat_log::LogWatcher;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant, SystemTime};

/// How much of the engine's log is read at one look, at most: the rest is read at the next looks, never skipped.
const ENGINE_LOG_LOOK: usize = 8 << 20;
/// How often the windows and the engine's log are looked at, at most: `seen` is asked many times a second while waiting.
const LOOK_EVERY: Duration = Duration::from_millis(700);
/// How long the program may take to say a quit was taken.
const QUIT_LIMIT: Duration = Duration::from_secs(20);
/// The game draws while the frames it presented or showed grew within this (the engine counts shown frames every 2 s; its
/// window server says its presents every 60 of them or so, about every 3 s at the game's usual rate).
const DRAWING_WITHIN: Duration = Duration::from_secs(15);

/// A process as macOS tells it: its parent, and when it was born (seconds, microseconds), as the engine records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Process {
    parent: u32,
    birth: (u64, u32),
}

/// Process `pid` while it runs.
fn process(pid: u32) -> Option<Process> {
    if pid <= 1 {
        return None;
    }
    // SAFETY: a plain query into a structure of ours, its size passed.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let got = libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size);
        // A zombie (SZOMB, 5) is gone.
        (got == size && info.pbi_status != 5).then_some(Process { parent: info.pbi_ppid, birth: (info.pbi_start_tvsec, info.pbi_start_tvusec as u32) })
    }
}

/// When process `pid` was born (seconds, microseconds) while it runs: what the engine records of its game.
pub(super) fn born(pid: u32) -> Option<(u64, u32)> {
    process(pid).map(|p| p.birth)
}

/// When process `pid` started (Unix seconds), while it runs.
fn birth(pid: u32) -> Option<u64> {
    process(pid).map(|p| p.birth.0)
}

/// The engine's window server the launcher `launcher` started (its child `wsd`), with its birth.
fn window_server(launcher: u32) -> Option<(u32, (u64, u32))> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
    let pid = system
        .processes()
        .values()
        .find(|p| p.parent() == Some(sysinfo::Pid::from_u32(launcher)) && p.name().to_string_lossy() == "wsd")
        .map(|p| p.pid().as_u32())?;
    let found = process(pid).filter(|p| p.parent == launcher)?;
    Some((pid, found.birth))
}

/// Which file a path is now (device, inode), to tell it from one put at its path since.
fn file_key(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (meta.dev(), meta.ino())
}

/// The log VRChat began after `since` whose own argument list holds `link`: this start's, and no other VRChat's.
pub(crate) fn log_of_launch(dir: &Path, since: SystemTime, link: &str) -> Option<(PathBuf, Vec<String>)> {
    let mut logs: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(modified, _)| *modified + Duration::from_secs(2) >= since)
        .collect();
    logs.sort();
    logs.into_iter().rev().find_map(|(_, path)| {
        let mut head = String::new();
        std::fs::File::open(&path).ok()?.take(256 * 1024).read_to_string(&mut head).ok();
        let args = crate::bridge::sessions::complete_args(&head)?;
        args.iter().any(|a| a == link).then_some((path, args))
    })
}

/// Whether the log's world state says the local player arrived in the destination of the launch link `link` (its world
/// and instance exactly): after the last join, never between it and the local player's arrival.
pub(crate) fn arrived_at(world: &orbiters_core::vrchat_log::WorldState, link: Option<&str>) -> bool {
    let (Some(link), Some(world_id), Some(instance)) = (link, world.world_id.as_deref(), world.instance.as_deref()) else { return false };
    world.in_world && world.arrived && crate::bridge::game::launch_link(world_id, instance) == link
}

/// Whether a Game session's record names the running launcher `pid` born at `birth` (seconds, microseconds) as its game:
/// the same process, not one that took its number since.
fn record_names(record: &obelisk::SessionRecord, pid: u32, birth: (u64, u32)) -> bool {
    !record.over() && record.game == Some(pid) && record.game_birth == Some(birth)
}

/// Whether the game's log at its path is still the file it was (`key`, `len` bytes at the last look), whole: the same file,
/// not shorter. Replaced, cut or gone, what it said is no longer this start's.
fn log_continues(key: (u64, u64), len: u64, now: ((u64, u64), u64)) -> bool {
    now.0 == key && now.1 >= len
}

/// The bridge's own game, verified: its Game session, the engine's launcher with its birth as the session's record
/// names them, and the launcher's own window server with its birth. Any of them changing ends the association.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ident {
    session: String,
    launcher: u32,
    birth: (u64, u32),
    wsd: Option<(u32, (u64, u32))>,
    /// When the session's record last said it is this game: read again at most every [`LOOK_EVERY`].
    verified: Instant,
}

/// The engine's log of this start, held open from the moment it was found (a new file each start): read whole, in order,
/// whatever later happens at its path.
struct EngineLog {
    file: std::fs::File,
    read: u64,
    /// The end of the last read, kept so that a line cut there is still found.
    tail: Vec<u8>,
    /// The most presents its window server said so far (`neptune unixlib wsi (wsd): <n> presents`).
    presents: Option<u64>,
}

/// Whether the game draws: the frames its Game session's record says it has shown, as they grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawing {
    /// No count yet, or one that lost its continuity: nothing proven either way.
    Unknown,
    /// The count grew within [`DRAWING_WITHIN`].
    Yes,
    /// The count stopped growing.
    Stalled,
}

/// What the frames say at `now`: the shown frames of the Game session's record (`continuous` while counted whole, last grown
/// at `shown_grew`) and the presents of this start's window server (last grown at `presents_grew`). Either growing is a game
/// that draws. A covered window or a locked screen shows nothing while the game still presents, so only both stalled is a
/// frozen game, and only when the presents are known.
pub(crate) fn drawing(continuous: Option<bool>, shown_grew: Option<Instant>, presents_grew: Option<Instant>, now: Instant) -> Drawing {
    let recent = |at: Option<Instant>| at.is_some_and(|at| now.duration_since(at) <= DRAWING_WITHIN);
    if (continuous == Some(true) && recent(shown_grew)) || recent(presents_grew) {
        Drawing::Yes
    } else if presents_grew.is_some() {
        Drawing::Stalled
    } else {
        Drawing::Unknown
    }
}

#[derive(Default)]
struct Mine {
    /// The shared start's launch id of the bridge's start, when it was asked, and its launch link.
    launch: Option<u64>,
    at: Option<SystemTime>,
    link: Option<String>,
    /// The engine's log file at the launch (the previous start's): the one found after it is this start's.
    engine_before: Option<(u64, u64)>,
    engine: Option<EngineLog>,
    renderer_lost: bool,
    /// The verified game.
    ident: Option<Ident>,
    /// This start's own VRChat log (its arguments name the launch link), which file it is and how long it was at the last
    /// look, and its follower. Lost (replaced, cut, unreadable): all of it is forgotten, and with it the place.
    log: Option<PathBuf>,
    log_key: Option<(u64, u64)>,
    log_len: u64,
    args: Option<Vec<String>>,
    watcher: Option<LogWatcher>,
    /// The frames shown so far (the record's `shown`), when they last grew, and whether they are still counted whole.
    shown: Option<u64>,
    grew: Option<Instant>,
    continuous: Option<bool>,
    /// The presents its window server said, and when they last grew.
    presents: Option<u64>,
    presents_grew: Option<Instant>,
    /// The last look at the windows and the engine's log.
    looked: Option<Instant>,
    window: Option<MacWindow>,
    /// Each arrival authorized (this launch's held log, its own destination, the local player in it, drawing) is
    /// numbered: a screenshot taken under one is delivered only under the same.
    arrivals: u64,
    authorized: bool,
}

impl Mine {
    /// The game's own log can no longer be followed: what it said is forgotten (place, arguments).
    fn lose_log(&mut self) {
        self.authorized = false;
        self.log = None;
        self.log_key = None;
        self.log_len = 0;
        self.args = None;
        self.watcher = None;
    }

    /// Another game, or the association broken: nothing of the last one is kept.
    fn forget_game(&mut self) {
        self.authorized = false;
        self.ident = None;
        self.window = None;
        self.shown = None;
        self.grew = None;
        self.continuous = None;
        self.presents = None;
        self.presents_grew = None;
        self.looked = None;
    }
}

pub(super) struct MacGame {
    real: RealGame,
    ctl: Weak<Controller>,
    mine: Mutex<Mine>,
    /// The shared start reserved by [`Game::admit_start`] for the next launch, until it is made or given back.
    reserved: Mutex<Option<u64>>,
}

impl MacGame {
    pub(super) fn new(ctl: Weak<Controller>, exe: PathBuf) -> Self {
        Self { real: RealGame::new(ctl.clone(), exe), ctl, mine: Mutex::default(), reserved: Mutex::default() }
    }

    fn ctl(&self) -> Result<Shared, Failure> {
        self.real.ctl()
    }

    /// The Mac's machine (whose start Home's Play shares), and what it calls on.
    fn machine(&self) -> Option<std::sync::Arc<dyn super::launcher::Machine>> {
        Some(self.ctl.upgrade()?.launcher_machine_now())
    }

    /// The bridge's own launch as the shared start says it now.
    fn launch(&self) -> Option<super::obelisk_cli::Launch> {
        let id = self.mine.lock().unwrap().launch?;
        let machine = self.machine()?;
        machine.as_mac()?.play().launch(id)
    }

    /// The bridge's own game while it runs: its launcher, verified with its Game session's record (the launcher's process
    /// and birth as the engine recorded them); a launch whose session is not named is not proven.
    fn own_launcher(&self) -> Option<u32> {
        let launch = self.launch().filter(|l| l.running && !l.over);
        let mut mine = self.mine.lock().unwrap();
        let Some((session, pid)) = launch.and_then(|l| Some((l.session?, l.launcher?))) else {
            mine.forget_game();
            return None;
        };
        let Some(now) = process(pid) else {
            mine.forget_game();
            return None;
        };
        let cached = mine.ident.as_ref().is_some_and(|i| i.session == session && i.launcher == pid && i.birth == now.birth);
        if cached && mine.ident.as_ref().is_some_and(|i| i.verified.elapsed() < LOOK_EVERY) {
            return Some(pid);
        }
        // The engine's record is read again: a session that ended, or names another game now, owns nothing.
        let library = obelisk::library(&obelisk::home());
        let record = obelisk::session_record_of(&library, &session);
        let named = match &record {
            Ok(Some(record)) => record_names(record, pid, now.birth),
            // Not readable this moment (being rewritten whole): the association stands only as long as it was just proven.
            Err(_) => cached && mine.ident.as_ref().is_some_and(|i| i.verified.elapsed() < LOOK_EVERY * 4),
            Ok(None) => false,
        };
        if !named {
            mine.forget_game();
            return None;
        }
        if cached {
            if let (Some(ident), Ok(Some(_))) = (mine.ident.as_mut(), &record) {
                ident.verified = Instant::now();
            }
            return Some(pid);
        }
        mine.forget_game();
        mine.ident = Some(Ident { session, launcher: pid, birth: now.birth, wsd: None, verified: Instant::now() });
        Some(pid)
    }

    /// A VRChat that is not the bridge's: Home's, the person's own, one left from before a restart of the service.
    fn others_game(&self, own: Option<u32>) -> Option<u32> {
        let machine = self.machine()?;
        machine.running().into_iter().map(|(pid, _)| pid).find(|pid| Some(*pid) != own)
    }

    /// Follows the bridge's own start: its log, its window, its frames, the engine's word on its graphics.
    fn look(&self, launcher: u32, library: &Path) {
        let mut mine = self.mine.lock().unwrap();
        // Its own VRChat log: found once, then held to be the same file, read in whole.
        if let Some(path) = mine.log.clone() {
            let now = std::fs::metadata(&path).ok().map(|m| (file_key(&m), m.len()));
            match (mine.log_key, now) {
                (Some(key), Some(now)) if log_continues(key, mine.log_len, now) => mine.log_len = now.1,
                _ => mine.lose_log(),
            }
        } else if let (Some(at), Some(link)) = (mine.at, mine.link.clone()) {
            if let Some((log, args)) = log_of_launch(&obelisk::vrchat_log_dir(library), at, &link) {
                if let Ok(meta) = std::fs::metadata(&log) {
                    mine.watcher = Some(LogWatcher::of_file(log.clone()));
                    mine.log_key = Some(file_key(&meta));
                    mine.log_len = meta.len();
                    mine.log = Some(log);
                    mine.args = Some(args);
                }
            }
        }
        if let Some(watcher) = mine.watcher.as_mut() {
            watcher.poll();
        }
        // The follower reads the very file whose arguments were verified, or nothing of it counts.
        let held = mine.watcher.as_ref().map(|w| (w.lost(), w.held()));
        if let Some((lost, held)) = held {
            if lost || held.is_some_and(|h| Some(h) != mine.log_key) {
                mine.lose_log();
            }
        }
        if mine.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        mine.looked = Some(Instant::now());
        // Its window server: the launcher's own child, the same process as when it was found.
        let wsd = mine.ident.as_ref().and_then(|i| i.wsd).filter(|(pid, born)| process(*pid).is_some_and(|p| p.parent == launcher && p.birth == *born)).or_else(|| window_server(launcher));
        if let Some(ident) = mine.ident.as_mut() {
            if ident.wsd != wsd {
                ident.wsd = wsd;
                mine.window = None;
            }
        }
        mine.window = wsd.and_then(|(pid, _)| native::game_window(&native::windows(), pid));
        // Its frames, from its own Game session's record.
        if let Some(session) = mine.ident.as_ref().map(|i| i.session.clone()) {
            // Not readable this time: the count is not known now, and nothing changes.
            if let Ok(Some(record)) = obelisk::session_record_of(library, &session) {
                match record.shown {
                    Some(shown) => {
                        if grown(mine.shown, shown.frames) {
                            mine.grew = Some(Instant::now());
                        }
                        mine.shown = Some(mine.shown.map_or(shown.frames, |before| before.max(shown.frames)));
                        // Once lost, continuity does not come back.
                        mine.continuous = Some(shown.continuous && mine.continuous != Some(false));
                    }
                    None => mine.continuous = mine.continuous.filter(|c| !c),
                }
            }
        }
        // The engine's log of this start: held open once found, read in order, never skipped.
        if mine.engine.is_none() {
            if let Ok(file) = std::fs::File::open(obelisk::engine_log(&obelisk::home())) {
                let key = file.metadata().ok().map(|m| file_key(&m));
                if key.is_some() && key != mine.engine_before {
                    mine.engine = Some(EngineLog { file, read: 0, tail: Vec::new(), presents: None });
                }
            }
        }
        let read = mine.engine.as_mut().map(|engine| (read_engine_log(engine), engine.presents));
        if let Some((lost, presents)) = read {
            mine.renderer_lost |= lost;
            if let Some(presents) = presents {
                // The first count read (a backlog, or the first line) is where it stands, not a growth.
                if grown(mine.presents, presents) {
                    mine.presents_grew = Some(Instant::now());
                }
                mine.presents = Some(mine.presents.map_or(presents, |before| before.max(presents)));
            }
        }
    }

    /// The bridge's own window, while its game runs: shown by its verified window server, as listed now.
    fn own_window(&self, pid: u32) -> Option<MacWindow> {
        if self.own_launcher() != Some(pid) {
            return None;
        }
        let mine = self.mine.lock().unwrap();
        let (wsd, _) = mine.ident.as_ref()?.wsd?;
        let window = mine.window?;
        (window.owner == wsd).then_some(window)
    }

    /// Whether the window `id` is shown by the bridge's own verified window server now (checked again right before a
    /// capture: a window id names whatever window has it).
    fn window_is_ours(&self, id: u32) -> bool {
        let Some(pid) = self.own_launcher() else { return false };
        let Some((wsd, born)) = self.mine.lock().unwrap().ident.as_ref().and_then(|i| i.wsd) else { return false };
        // The very window chosen as the game's (not any window of the window server: its splash, a crash box).
        process(wsd).is_some_and(|p| p.parent == pid && p.birth == born) && native::game_window(&native::windows(), wsd).is_some_and(|w| w.id == id)
    }

    /// Asks the program to quit the bridge's own game session, as the game's own Quit does: never forced.
    fn quit(&self) -> Result<(), Failure> {
        let launch = self.launch().ok_or_else(|| Failure::new("vrchat_not_running", "The VRChat this bridge started is gone.", "Call get_state."))?; // i18n: not shown (an AI client reads it)
        let session = launch.session.ok_or_else(|| {
            Failure::new("close_failed", "The engine did not say which Game session runs this VRChat, so nothing was asked to close.", "Tell the person: they can quit VRChat themselves.") // i18n: not shown (an AI client reads it)
        })?;
        let machine = self.machine().ok_or_else(|| Failure::new("app_closing", "Orbiters - N4N0 is closing.", "Stop here."))?; // i18n: not shown
        let mac = machine.as_mac().ok_or_else(|| Failure::new("close_failed", "VRChat is not set up on this Mac.", "Tell the person."))?; // i18n: not shown
        let program = mac.program().ok_or_else(|| Failure::new("close_failed", "What runs VRChat on this Mac is not there.", "Tell the person."))?; // i18n: not shown
        let args = quit_command(program_version(&program).as_deref(), &session)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut code = None;
        let ran = super::obelisk_cli::run_for(&program, &args, QUIT_LIMIT, |line| {
            if let Some(c) = serde_json::from_str::<serde_json::Value>(line).ok().and_then(|v| (v["event"] == "error").then(|| v["code"].as_str().map(str::to_owned)).flatten()) {
                code = Some(c);
            }
        });
        match (ran, code.as_deref()) {
            (_, Some("not_running")) => Ok(()),
            (_, Some("other_session")) => Err(Failure::new("not_started_by_bridge", "The VRChat running now is not the one this bridge started, so it was not asked to close.", "Leave it alone and tell the person.")), // i18n: not shown
            (_, Some(other)) => Err(Failure::new("close_failed", format!("The engine did not take the request to quit: {other}."), "Call get_state; if VRChat still runs, tell the person.")), // i18n: not shown
            (Err(e), None) => Err(Failure::new("close_failed", format!("The engine could not be asked to quit VRChat: {e}"), "Call get_state; if VRChat still runs, tell the person.")), // i18n: not shown
            (Ok(()), None) => Ok(()),
        }
    }
}

/// Whether a count grew: from one already read of this launch, never from none (a first count, even 0, proves nothing).
pub(crate) fn grown(before: Option<u64>, now: u64) -> bool {
    before.is_some_and(|before| now > before)
}

/// The most presents the window server's lines in `text` say (`neptune unixlib wsi (wsd): 6480 presents (4599 shown events)`,
/// `… alive: 6452 presents, …`).
fn presents_said(text: &str) -> Option<u64> {
    text.lines()
        .filter_map(|line| line.split_once("neptune unixlib wsi (wsd): ")) // i18n: not shown (the engine's log lines)
        .filter_map(|(_, rest)| rest.trim_start_matches("alive: ").split_once(" presents").and_then(|(n, _)| n.trim().parse().ok()))
        .max()
}

/// Reads what the held engine log gained since the last read, up to [`ENGINE_LOG_LOOK`] at a time (the rest next time):
/// whether it says the game's graphics went.
fn read_engine_log(engine: &mut EngineLog) -> bool {
    let len = engine.file.metadata().map_or(0, |m| m.len());
    if len <= engine.read || engine.file.seek(SeekFrom::Start(engine.read)).is_err() {
        return false;
    }
    let mut bytes = std::mem::take(&mut engine.tail);
    let before = bytes.len();
    let _ = (&mut engine.file).take(ENGINE_LOG_LOOK as u64).read_to_end(&mut bytes);
    engine.read += (bytes.len() - before) as u64;
    let text = String::from_utf8_lossy(&bytes);
    let lost = obelisk::renderer_lost(&text);
    engine.presents = engine.presents.max(presents_said(&text));
    // A line cut at the end is read again with what follows.
    engine.tail = bytes[bytes.len().saturating_sub(256)..].to_vec();
    lost
}

/// The first obelisk-cli with `steam quit --session` (one step, checked by the program).
const CLI_SESSION_QUIT: (u32, u32, u32) = (0, 1, 3);
/// The first obelisk-cli whose Game session records say the frames shown so far (`shown`): what tells a game that draws
/// from a frozen window. The bridge starts no game with an older one.
const CLI_SHOWN_FRAMES: (u32, u32, u32) = (0, 1, 5);

/// What asks the program to quit the Game session `session`, with obelisk-cli `version`: only `steam quit --session`, which
/// the program checks against the running session itself in one step. An older program's unqualified quit could end the
/// person's game started between a check and the quit: refused, nothing is asked.
fn quit_command(version: Option<&str>, session: &str) -> Result<Vec<String>, Failure> {
    if !version.is_some_and(|v| version_at_least(v, CLI_SESSION_QUIT)) {
        return Err(Failure::new(
            "close_failed",
            "What runs VRChat on this Mac is too old to close only the bridge's own game, so nothing was asked to close.", // i18n: not shown (an AI client reads it)
            "Tell the person: they can quit VRChat themselves; Orbiters - N4N0 gets a newer version at its next start.", // i18n: not shown
        ));
    }
    Ok(["steam", "quit", obelisk::STEAM_APP, "--session", session].map(String::from).to_vec())
}

/// The version of the obelisk-cli at `program`, from its release folder's name (`obelisk-cli-<version>-arm64`).
pub(super) fn program_version(program: &Path) -> Option<String> {
    program.parent()?.file_name()?.to_str()?.strip_prefix("obelisk-cli-")?.strip_suffix("-arm64").map(str::to_owned)
}

/// Whether obelisk-cli `version` is `least` or later.
pub(super) fn version_at_least(version: &str, least: (u32, u32, u32)) -> bool {
    let parts: Vec<u32> = version.split('.').map(|p| p.parse().ok()).collect::<Option<_>>().unwrap_or_default();
    parts.len() >= 3 && (parts[0], parts[1], parts[2]) >= least
}

impl Game for MacGame {
    fn seen(&self) -> Seen {
        let base = self.real.seen();
        let library = obelisk::library(&obelisk::home());
        let mut seen = Seen {
            log_found: obelisk::vrchat_log_dir(&library).is_dir(),
            vrchat_user: base.vrchat_user,
            reconstruction: base.reconstruction,
            queue: base.queue,
            queue_waits_for: base.queue_waits_for,
            // The app's capture follows the VRChat on the network on a Mac, never this one: nothing of it is this game's.
            capture: CaptureSeen { state: "idle".into(), ..Default::default() },
            ..Default::default()
        };
        let own = self.own_launcher();
        match own {
            Some(launcher) => {
                self.look(launcher, &library);
                let mine = self.mine.lock().unwrap();
                seen.running = true;
                seen.pid = Some(launcher);
                let drawn = drawing(mine.continuous, mine.grew, mine.presents_grew, Instant::now());
                // Lost only when the engine says so (or the game ends): frames that stop for a while (a long load, a
                // suspended compositor) are no proof of drawing, which keeps the arrival from being given, and no loss.
                seen.renderer_lost = mine.renderer_lost;
                seen.desktop_mode = mine.args.as_ref().map(|a| a.iter().any(|x| x == "--no-vr"));
                let mut mine = mine;
                let link = mine.link.clone();
                let window = mine.window.is_some();
                let mut authorized = false;
                if let Some(watcher) = mine.watcher.as_ref().filter(|w| w.caught_up() && !w.lost() && w.held().is_some()) {
                    let world = &watcher.state;
                    seen.world_id = world.world_id.clone();
                    seen.world_name = world.world_name.clone();
                    seen.instance = world.instance.clone();
                    // In a world only once the local player arrived in this launch's own destination, as its own held log
                    // says, while its window is on screen and its frames grow.
                    authorized = arrived_at(world, link.as_deref()) && window && drawn == Drawing::Yes;
                    seen.in_world = authorized;
                    seen.local_player = world.local_player.clone();
                    seen.others = world.others();
                    seen.avatar_name = world.avatar_name.clone().or_else(|| world.local_player.as_ref().and_then(|me| world.avatars.get(me).cloned()));
                }
                if authorized && !mine.authorized {
                    mine.arrivals += 1;
                }
                mine.authorized = authorized;
                seen.arrival = authorized.then_some(mine.arrivals);
            }
            None => {
                // The bridge's start that never ran, or ended: why, for the agent.
                if let Some(launch) = self.launch().filter(|l| !l.running) {
                    seen.start_failed = launch.failure.map(|f| f.words).or_else(|| {
                        launch.ended.map(|(seconds, frames)| format!("VRChat ended after {} s with {} frames shown.", seconds.map_or("?".into(), |s| s.to_string()), frames.map_or("?".into(), |f| f.to_string()))) // i18n: not shown (an AI client reads it)
                    });
                }
                if let Some(pid) = self.others_game(None) {
                    // The person's (or one this run of the service did not start): running, and nothing else read.
                    seen.running = true;
                    seen.pid = Some(pid);
                }
            }
        }
        seen
    }

    fn camera_open(&self) -> Option<bool> {
        None
    }

    fn capture_kit(&self) -> Option<bool> {
        None
    }

    fn create_private_instance(&self, world: &str, region: &str) -> Result<String, Failure> {
        self.real.create_private_instance(world, region)
    }

    fn find_worlds(&self, find: &Find, page: usize) -> Result<WorldPage, Failure> {
        self.real.find_worlds(find, page)
    }

    fn launch(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        let ctl = self.ctl()?;
        let machine = ctl.launcher_machine_now();
        let Some(mac) = machine.as_mac() else {
            return Err(Failure::new("vrchat_not_installed", "VRChat is not set up on this Mac.", "Ask the person to set it up from Orbiters - N4N0's Home.")); // i18n: not shown
        };
        // The same lock Home's Play starts under.
        let _started = ctl.launcher_started_lock();
        // What tells a drawing game from a frozen window is the program's (its records' `shown`): no start without it.
        cli_fit(mac)?;
        let link = crate::bridge::game::launch_link(&plan.world_id, &plan.instance);
        let engine_before = std::fs::metadata(obelisk::engine_log(&obelisk::home())).ok().map(|m| file_key(&m));
        let at = SystemTime::now();
        let token = self.reserved.lock().unwrap().take();
        match mac.start_launch_reserved(&plan.args(), token) {
            Ok(id) => {
                *self.mine.lock().unwrap() = Mine { launch: Some(id), at: Some(at), link: Some(link), engine_before, ..Default::default() };
                Ok(())
            }
            Err(super::obelisk_cli::Refused::Wait(wait)) => Err(start_wait_failure(wait)),
            Err(super::obelisk_cli::Refused::Records(trouble)) => Err(records_failure(&trouble)),
            Err(super::obelisk_cli::Refused::Busy) => Err(Failure::new("vrchat_starting", "A VRChat is starting already on this Mac (from Orbiters' Home, or another start).", "Wait a minute, then call get_state.")), // i18n: not shown
            Err(super::obelisk_cli::Refused::Failed(why)) => Err(Failure::new("start_failed", format!("VRChat did not start: {why}"), "Ask the person to look at Orbiters - N4N0's Home.")), // i18n: not shown
        }
    }

    fn admit_start(&self, own_running: bool) -> Result<(), Failure> {
        let ctl = self.ctl()?;
        let machine = ctl.launcher_machine_now();
        let Some(mac) = machine.as_mac() else { return Ok(()) };
        // The program must have what the bridge relies on before anything is made for a start.
        cli_fit(mac)?;
        self.release_start();
        let own = self.mine.lock().unwrap().launch.filter(|_| own_running);
        // Reserved, not only looked at: from here no other start (Home's) is made until this one is or is given back.
        match mac.play().reserve(mac.library(), own) {
            Ok(token) => {
                *self.reserved.lock().unwrap() = Some(token);
                Ok(())
            }
            Err(super::obelisk_cli::Refused::Busy) => Err(Failure::new("vrchat_starting", "A VRChat is starting already on this Mac (from Orbiters' Home, or another start), so no instance was made.", "Wait a minute, then call get_state.")), // i18n: not shown (an AI client reads it)
            Err(super::obelisk_cli::Refused::Wait(wait)) => Err(start_wait_failure(wait)),
            Err(super::obelisk_cli::Refused::Records(trouble)) => Err(records_failure(&trouble)),
            Err(other) => Err(Failure::new("start_failed", other.words(), "Call get_state.")), // i18n: not shown (an AI client reads it)
        }
    }

    fn release_start(&self) {
        let Some(token) = self.reserved.lock().unwrap().take() else { return };
        if let Some(mac) = self.machine() {
            if let Some(mac) = mac.as_mac() {
                mac.play().unreserve(token);
            }
        }
    }

    fn start_ready(&self) -> Option<Duration> {
        let machine = self.machine()?;
        let mac = machine.as_mac()?;
        // The bridge's own last game, closed just now: its end being finalized.
        if mac.play().attached().is_some() {
            return Some(Duration::from_millis(500));
        }
        match mac.play().gate(mac.library()) {
            Ok(wait) => wait,
            Err(_) => Some(Duration::from_secs(1)),
        }
    }

    fn launch_args(&self, pid: u32) -> Option<Vec<String>> {
        // Only this start's own log, read for its own launcher: never an intention, always what VRChat itself received.
        (self.own_launcher() == Some(pid)).then(|| self.mine.lock().unwrap().args.clone()).flatten()
    }

    fn identity(&self, pid: u32) -> Identity {
        match birth(pid) {
            Some(start) => Identity::Running(start),
            None => Identity::Gone,
        }
    }

    fn window(&self, pid: u32) -> Option<GameWindow> {
        let window = self.own_window(pid)?;
        // The bridge's own starts take no render percent: the game's pixels are the content's (R2).
        let (width, height) = native::guest_pixels(&window, native::scale_of(&window), native::title_points(&window, native::display_frame(&window)), 100);
        Some(GameWindow { handle: window.id as isize, width, height })
    }

    /// Its exact window in front of every other: the frontmost ordinary window is this one, shown by its window server.
    fn is_foreground(&self, window: &GameWindow) -> bool {
        u32::try_from(window.handle).is_ok_and(|id| native::front_window() == Some(id) && self.window_is_ours(id))
    }

    /// Nothing on a Mac: no input is sent to the game there, so its window is never brought in front of the person's.
    fn bring_forward(&self, _window: &GameWindow) {}

    /// The bridge's own Game session is asked to quit, whatever window was named (a game whose graphics went may have
    /// none): the session, not a window, is what is checked.
    fn close(&self, _window: &GameWindow) -> Result<(), Failure> {
        self.quit()
    }

    fn capture(&self, window: &GameWindow, max_width: u32) -> Result<Shot, Failure> {
        if native::screen_recording() != Some(true) {
            return Err(Failure::new(
                "screen_recording_not_allowed",
                "macOS does not let Orbiters - N4N0 record the screen, so VRChat's window cannot be looked at.", // i18n: not shown (an AI client reads it)
                "Tell the person: in Orbiters - N4N0's Settings, Experimental, VRChat bridge, press Allow screen recording (or allow Orbiters - N4N0 in System Settings, Privacy & Security, Screen Recording), then quit and reopen Orbiters - N4N0.", // i18n: not shown
            ));
        }
        if native::screen_locked() {
            return Err(Failure::new(
                "screen_locked",
                "The Mac's screen is locked: macOS does not let VRChat's window be drawn meanwhile, so its picture would be black.", // i18n: not shown (an AI client reads it)
                "Try again in a minute (the person has to unlock the Mac first), or tell them.", // i18n: not shown
            )
            .retry_after(60));
        }
        // The window id is checked again right before the picture: it must still be shown by this game's window server.
        let id = u32::try_from(window.handle).ok().filter(|id| self.window_is_ours(*id)).ok_or_else(|| {
            Failure::new("not_started_by_bridge", "That window is no longer this bridge's VRChat's, so it was not looked at.", "Call get_state.") // i18n: not shown (an AI client reads it)
        })?;
        let frame = self.mine.lock().unwrap().window;
        let owner = self.mine.lock().unwrap().ident.as_ref().and_then(|i| i.wsd);
        let png = native::capture_png(id).map_err(|e| Failure::new("screenshot_failed", format!("macOS did not give the window's picture: {e}"), "Call get_state; try again in a moment."))?; // i18n: not shown
        // The window may have changed hands while it was captured: the picture is kept only if it is still this game's.
        let still = self.window_is_ours(id) && self.mine.lock().unwrap().ident.as_ref().and_then(|i| i.wsd) == owner;
        if !still {
            return Err(Failure::new("not_started_by_bridge", "That window stopped being this bridge's VRChat's while it was looked at, so the picture was not kept.", "Call get_state.")); // i18n: not shown (an AI client reads it)
        }
        let image = image::load_from_memory(&png).map_err(|e| Failure::new("screenshot_failed", e.to_string(), "Try again in a moment."))?.to_rgba8(); // i18n: not shown
        // The title bar is the system's, not the game's: the picture is the content, as clicks and VRChat count it.
        let top = frame.map_or(0, |f| native::title_rows(image.height(), f.height, native::title_points(&f, native::display_frame(&f)))).min(image.height().saturating_sub(1));
        let (width, height) = (image.width(), image.height() - top);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in top..image.height() {
            for x in 0..width {
                let p = image.get_pixel(x, row).0;
                pixels.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        crate::bridge::capture::encode(&crate::bridge::capture::Bgra { width, height, pixels }, max_width).map_err(|e| Failure::new("screenshot_failed", e, "Try again in a moment.")) // i18n: not shown
    }

    fn osc(&self, _address: &str, _value: OscArg) -> Result<(), Failure> {
        Err(not_here())
    }

    fn wear(&self, _avatar_id: &str) -> Result<(), Failure> {
        Err(not_here())
    }

    fn osc_target(&self) -> Option<std::net::SocketAddr> {
        None
    }

    fn inject(&self, _window: &GameWindow, _input: &[Input]) -> Result<(), Failure> {
        Err(not_here())
    }

    fn release(&self, _input: &[Input]) {}

    fn take_picture(&self, _length: Photo3dLength, _mode: PictureMode) -> Result<(), Failure> {
        Err(not_here())
    }

    fn stop_picture(&self) {}

    fn picture_files(&self, session: &Path) -> PictureFiles {
        self.real.picture_files(session)
    }

    fn hold_reconstructions(&self, hold: bool) {
        self.real.hold_reconstructions(hold);
    }

    fn favorite_groups(&self) -> Result<Vec<(String, String)>, Failure> {
        self.real.favorite_groups()
    }
}

/// What starts sessions and knows this computer, on a Mac (docs/vrchat-bridge.md's "Trait dispositions"): no side
/// session, one VRChat at a time, the engine's own roster (the verified launchers the shared start and the engine's records
/// name), and nothing of Windows' (no Easy Anti-Cheat service to probe, no launcher left to end, no executable names).
pub(super) struct MacOpener {
    ctl: Weak<Controller>,
}

impl MacOpener {
    pub(super) fn new(ctl: Weak<Controller>) -> Self {
        Self { ctl }
    }

    fn running(&self) -> Vec<u32> {
        self.ctl.upgrade().map(|c| c.launcher_machine_now().running().into_iter().map(|(pid, _)| pid).collect()).unwrap_or_default()
    }
}

impl crate::bridge::game::Opener for MacOpener {
    fn side(&self, _session: &str, _profile: u32, _osc: (u16, u16)) -> Result<std::sync::Arc<dyn Game>, Failure> {
        Err(not_here())
    }

    fn forget(&self, _session: &str) {}

    fn rename(&self, _from: &str, _to: &str) {}

    fn vrchat_pids(&self) -> Vec<u32> {
        self.running()
    }

    fn others_profiles(&self) -> Vec<u32> {
        // One engine library, one Steam game: every VRChat here runs on the default profile.
        self.running().iter().map(|_| 0).collect()
    }

    fn eac(&self) -> crate::bridge::sessions::Eac {
        crate::bridge::sessions::Eac { service: "ABSENT".into(), driver: "ABSENT".into(), vrchat_runs: !self.running().is_empty(), launcher_runs: false, delete_pending: false } // i18n: not shown (states as Windows' sc names them)
    }

    fn memory_free(&self) -> Option<u64> {
        let mut system = sysinfo::System::new();
        system.refresh_memory();
        Some(system.available_memory())
    }

    fn limit(&self) -> usize {
        1
    }

    /// The engine's launcher whose window server owns the frontmost ordinary window, when that is one of the VRChats here.
    fn foreground_pid(&self) -> Option<u32> {
        let front = native::front_window()?;
        let windows = native::windows();
        let owner = windows.iter().find(|w| w.id == front)?.owner;
        // Only the game's own window counts (a splash or a crash box of the same window server does not).
        if native::game_window(&windows, owner).is_none_or(|w| w.id != front) {
            return None;
        }
        let launcher = process(owner)?.parent;
        self.running().contains(&launcher).then_some(launcher)
    }

    fn vrchats(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.running()
                .into_iter()
                .map(|_| serde_json::json!({ "profile": 0, "account": null, "world_name": null, "desktop": true, "camera_open": null, "driven_by": null, "pictures_here": false }))
                .collect(),
        )
    }
}

/// A start held by the wait after an early end, `wait` still to go.
fn start_wait_failure(wait: Duration) -> Failure {
    Failure::new(
        "start_wait",
        format!("VRChat ended early {} s ago on this Mac; a start this soon after one makes Easy Anti-Cheat fail, so nothing was made or started.", (obelisk::AFTER_EARLY_END.saturating_sub(wait)).as_secs()), // i18n: not shown (an AI client reads it)
        format!("Wait {} s, then call go_to_world again.", wait.as_secs().max(1)), // i18n: not shown
    )
    .retry_after(wait.as_secs().max(1))
}

/// Whether the program has what the bridge relies on (`shown` frames, the session-qualified quit), refused before
/// anything is made otherwise.
fn cli_fit(mac: &super::launcher::MacMachine) -> Result<(), Failure> {
    if mac.program().as_deref().and_then(program_version).is_some_and(|v| version_at_least(&v, CLI_SHOWN_FRAMES)) {
        return Ok(());
    }
    Err(Failure::new(
        "start_failed",
        "What runs VRChat on this Mac is too old for the bridge (it cannot tell whether the game draws), so nothing was made or started.", // i18n: not shown (an AI client reads it)
        "Tell the person: Orbiters - N4N0 gets a newer version a little after it starts; reopen it, then call go_to_world again.", // i18n: not shown
    ))
}

/// A start held by a record of the engine's that cannot be read: a wait while it may still be a start's, else the person.
fn records_failure(trouble: &super::obelisk_cli::Trouble) -> Failure {
    match trouble.wait() {
        Some(wait) => Failure::new(
            "start_wait",
            format!("A record of an earlier VRChat start on this Mac cannot be read ({}), so whether it ended early is not known yet and nothing was made or started.", super::obelisk_cli::file_name(&trouble.file)), // i18n: not shown (an AI client reads it)
            format!("Wait {} s, then call go_to_world again.", wait.as_secs().max(1)), // i18n: not shown
        )
        .retry_after(wait.as_secs().max(1)),
        None => Failure::new(
            "start_blocked",
            format!("VRChat cannot be started on this Mac until a damaged record of its engine is set aside ({}).", trouble.file.display()), // i18n: not shown (an AI client reads it)
            "Tell the person: Orbiters - N4N0's Home shows the record and a Set aside button.", // i18n: not shown
        ),
    }
}

/// What a Mac's game answers for what its capabilities already refuse (a second line of defence).
fn not_here() -> Failure {
    crate::bridge::caps::Capabilities::MAC.refusal("walk", &serde_json::Value::Null).unwrap_or_else(|| Failure::new("not_on_this_computer", "Not on this computer.", "Tell the person.")) // i18n: not shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_s_own_log_is_the_one_that_names_its_link() {
        let dir = std::env::temp_dir().join(format!("orbiters-mac-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let since = SystemTime::now() - Duration::from_secs(1);
        let link = "vrchat://launch?ref=orbiters.cc&id=wrld_1:2~private(usr_1)~region(eu)~nonce(n)";
        let head = |args: &[&str]| {
            let mut text = format!("2026.11.22 20:45:07 Debug      -  Launching with args: {}\n", args.len());
            for a in args {
                text.push_str(&format!("2026.11.22 20:45:07 Debug      -  Arg: {a}\n"));
            }
            text
        };
        // As VRChat under the engine wrote it on 2026-11-22: the exe, Steam's --no-vr, then ours in order.
        std::fs::write(dir.join("output_log_2026-11-22_20-45-03.txt"), head(&["Z:\\opt\\game\\VRChat.exe", "--no-vr", "-screen-fullscreen", "0", link])).unwrap();
        std::fs::write(dir.join("output_log_2026-11-22_20-50-00.txt"), head(&["Z:\\opt\\game\\VRChat.exe", "--no-vr"])).unwrap();
        let (log, args) = log_of_launch(&dir, since, link).expect("its own log");
        assert!(log.ends_with("output_log_2026-11-22_20-45-03.txt"));
        assert_eq!(args.last().map(String::as_str), Some(link));
        assert_eq!(log_of_launch(&dir, since, "vrchat://launch?ref=orbiters.cc&id=wrld_1:3"), None, "a log naming another instance is not this start's");
        assert_eq!(log_of_launch(&dir, SystemTime::now() + Duration::from_secs(60), link), None, "a log from before the start is not this start's");
        let _ = std::fs::remove_dir_all(&dir);
        // An older program is never asked to quit without the session (it would quit whatever game runs).
        assert_eq!(quit_command(Some("0.1.5"), "6da7c40d").unwrap(), ["steam", "quit", "438100", "--session", "6da7c40d"]);
        assert_eq!(quit_command(Some("0.1.3"), "6da7c40d").unwrap().last().map(String::as_str), Some("6da7c40d"));
        assert_eq!(quit_command(Some("0.1.2"), "6da7c40d").unwrap_err().code, "close_failed");
        assert_eq!(quit_command(None, "6da7c40d").unwrap_err().code, "close_failed");
        assert!(version_at_least("0.1.3", CLI_SESSION_QUIT) && version_at_least("0.2.0", CLI_SESSION_QUIT));
        assert!(!version_at_least("0.1.1", CLI_SESSION_QUIT) && !version_at_least("x", CLI_SESSION_QUIT) && !version_at_least("0.1.x", CLI_SESSION_QUIT));
        assert!(version_at_least("0.1.5", CLI_SHOWN_FRAMES) && !version_at_least("0.1.4", CLI_SHOWN_FRAMES));
        assert_eq!(program_version(Path::new("/T/obelisk-cli/obelisk-cli-0.1.5-arm64/obelisk-cli")).as_deref(), Some("0.1.5"));
        assert_eq!(program_version(Path::new("/T/obelisk-cli/other/obelisk-cli")), None);
    }

    /// The verifier's cases: the launcher is the record's only with its recorded birth (a reused process number is not),
    /// and a log replaced or cut ends what its follower said.
    #[test]
    fn only_the_recorded_launcher_and_its_own_whole_log_are_the_bridge_s() {
        let record = |phase: &str| obelisk::SessionRecord { id: "s".into(), phase: phase.into(), game: Some(33466), seconds: None, frames: None, game_birth: Some((1795387812, 431)), shown: None };
        assert!(record_names(&record("Running"), 33466, (1795387812, 431)));
        assert!(!record_names(&record("Running"), 33466, (1795387812, 432)), "the same number, another process");
        assert!(!record_names(&record("Running"), 33467, (1795387812, 431)));
        assert!(!record_names(&record("Finished"), 33466, (1795387812, 431)), "a session that ended owns nothing");
        let no_birth = obelisk::SessionRecord { game_birth: None, ..record("Running") };
        assert!(!record_names(&no_birth, 33466, (1795387812, 431)), "a record that does not say its birth proves nothing");
        assert!(log_continues((1, 7), 100, ((1, 7), 150)));
        assert!(!log_continues((1, 7), 100, ((1, 8), 150)), "another file at its path");
        assert!(!log_continues((1, 7), 100, ((1, 7), 40)), "cut shorter");
    }

    /// B1: a screenshot's arrival is this launch's exact destination with its local player in it, never the join before
    /// it (loading, spawning), never another instance of the same world, never with no launch link.
    #[test]
    fn arrival_is_the_launch_s_own_destination_with_its_local_player() {
        use orbiters_core::vrchat_log::{LogEvent, WorldState};
        let world = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
        let ours = "11~private(usr_8784f94a-27ed-4a3b-9c1d-112233445566)~region(eu)~nonce(a)";
        let link = crate::bridge::game::launch_link(world, ours);
        let mut state = WorldState::default();
        state.apply(&LogEvent::Joining { world_id: world.into(), instance: ours.into(), at: None });
        assert!(!arrived_at(&state, Some(&link)), "Joining: loading");
        state.apply(&LogEvent::JoinedRoom { name: "W".into() });
        assert!(!arrived_at(&state, Some(&link)), "joined, spawning: not arrived");
        state.apply(&LogEvent::LocalPlayer { name: "Me".into() });
        assert!(arrived_at(&state, Some(&link)));
        assert!(!arrived_at(&state, None), "no launch of the bridge's");
        assert!(!arrived_at(&state, Some(&crate::bridge::game::launch_link(world, "12~private(usr_8784f94a-27ed-4a3b-9c1d-112233445566)~region(eu)~nonce(b)"))), "another instance");
        state.apply(&LogEvent::Joining { world_id: world.into(), instance: "12~private(usr_x)~region(eu)".into(), at: None });
        state.apply(&LogEvent::LocalPlayer { name: "Me".into() });
        assert!(!arrived_at(&state, Some(&link)), "moved on: the old destination is not where it is");
    }

    /// The game draws while its frames grow; a count that stopped is a frozen window, one without continuity is unknown.
    #[test]
    fn a_window_is_drawn_only_while_its_frames_grow() {
        let now = Instant::now();
        let (fresh, old) = (Some(now - Duration::from_secs(2)), Some(now - Duration::from_secs(40)));
        assert_eq!(drawing(Some(true), fresh, old, now), Drawing::Yes, "shown frames growing");
        assert_eq!(drawing(Some(true), old, fresh, now), Drawing::Yes, "covered, or the screen locked: presented, not shown");
        assert_eq!(drawing(Some(true), old, old, now), Drawing::Stalled, "neither: a frozen game");
        assert_eq!(drawing(Some(false), Some(now), None, now), Drawing::Unknown, "a log that lost its continuity, no presents known");
        assert_eq!(drawing(None, None, None, now), Drawing::Unknown, "no count yet");
        // M6: a first count (0, or a backlog's) is where it stands; only a later, larger one is drawing.
        assert!(!grown(None, 0) && !grown(None, 6452), "a first count proves nothing");
        assert!(!grown(Some(10), 10) && grown(Some(10), 11));
        // As this Mac's engine wrote them on 2026-11-22 23:56.
        let log = "neptune unixlib wsi (wsd): 6420 presents (4599 shown events), last 60: interval avg 55.840 ms\nneptune unixlib wsi (wsd): alive: 6452 presents, 4599 shown events, 7286 drains\nwsd: window 1 overlay: frame held 183.3 ms\n";
        assert_eq!(presents_said(log), Some(6452));
        assert_eq!(presents_said("nothing"), None);
    }
}
