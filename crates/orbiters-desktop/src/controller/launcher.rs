//! N4N0 as a VRChat launcher (REQUEST10), the controller side: what Home's friends list, profile modal, invite card and
//! Play buttons read (`Snapshot::launcher`, an [`orbiters_core::launcher::LauncherState`]) and the [`Action`]s they send.
//!
//! - Signed in to VRChat in N4N0 (Settings > VRChat account): every friend (pages of 100, a second apart), the
//!   favourite friend groups, the invites, invite requests and boops waiting; then VRChat's pipeline (one websocket)
//!   keeps it all live. A full read again only when the pipeline comes back after a cut, or every 30 minutes.
//! - Otherwise: VRCNext's or VRCX's own copy (`orbiters_core::launcher_local`), read again every 2 minutes, with
//!   [`Possible`] saying that live status, invites and boops need the sign-in.
//! - Play: VRChat through Steam (`-applaunch 438100`) with the launch options of Settings; never a second VRChat: when
//!   one runs, a destination goes to it through its own link pipe (`travel`), else an invite to oneself.
//! - Play on a Mac ([`MacMachine`]): VRChat runs there through Obelisk's engine (`orbiters_core::obelisk`), one game at a
//!   time, on the profile chosen on Home; a destination goes with the start, or to a game that runs as an invite to oneself.
//! - A private destination, where this computer uses several VRChat profiles, goes in two steps
//!   (`orbiters_core::vrchat_launch::way`, the one rule of every start and delivery): VRChat starts without it, and it is
//!   delivered once that very VRChat's own log proves the account it is for ([`Pending`]).
use super::vrchat_scope::{By, MachineParts, Session, Stamp};
use super::{Controller, Shared};
use orbiters_core::launcher::{Action, EngineSetup, Game, Invite, JoinFailure, JoinStep, Joining, LaunchMode, LauncherState, LogTrouble, Notice, Place, Possible, Profile, SentKind, Source, Whereabouts};
use orbiters_core::launcher_feed::{self, Cooldowns, Decorations};
use orbiters_core::vrchat_api::{Client, Error};
use orbiters_core::vrchat_launch::{desktop_window, SavedWindow, To, Way};
use orbiters_i18n::tr;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LOCAL_EVERY: Duration = Duration::from_secs(120);
/// A world or decoration lookup that failed (not "not found": a time-out, VRChat unreachable) is asked again after this.
const LOOKUP_AGAIN: Duration = Duration::from_secs(10 * 60);
/// A profile's details, kept per person this long (at most this many people).
const PROFILE_FOR: Duration = Duration::from_secs(5 * 60);
const PROFILES_KEPT: usize = 64;
/// Boops put away, remembered this long (VRChat may list them again on every full read).
const DISMISSED_FOR: i64 = 30 * 24 * 3600;
/// A local copy caught busy is read again after this.
const LOCAL_RETRY: Duration = Duration::from_secs(10);
const FULL_EVERY: Duration = Duration::from_secs(30 * 60);
const GAME_EVERY: Duration = Duration::from_secs(5);
/// A start just asked for counts as "starting" this long (Steam, then Easy Anti-Cheat, then VRChat's process).
const STARTING_FOR: Duration = Duration::from_secs(60);
/// How long a start's claim on its VRChat profile holds when no VRChat with that profile shows up: a slow start (Steam,
/// Easy Anti-Cheat, an update) is still a start, so this is long; a VRChat seen on the profile ends the claim at once.
const PROFILE_HELD: Duration = Duration::from_secs(180);
/// A start past [`PROFILE_HELD`] whose VRChat was never seen is not forgotten: its profile takes no new start from a
/// person until they, told so, press again within [`CONFIRM_WITHIN`] (VRChat may still come: a long Steam update), however
/// long that is; a start nobody pressed (the bridge's) may take the profile after this long.
const PROFILE_LAPSED: Duration = Duration::from_secs(10 * 60);
const CONFIRM_WITHIN: Duration = Duration::from_secs(20);
/// How long a VRChat whose profile is not known holds every start before the person may override it.
const UNKNOWN_WAIT: Duration = Duration::from_secs(60);
/// The same on a Mac, where the person presses Play in Obelisk (its sign-in, an update check, Easy Anti-Cheat).
const MAC_STARTING_FOR: Duration = Duration::from_secs(180);
/// How long an action's outcome stays in the state (the page's "Sent", the error it took back).
const NOTICE_FOR: Duration = Duration::from_secs(3);
/// After a failed read of VRChat: the first try again, doubling up to the longest (with jitter), never every tick.
const RETRY_FIRST: Duration = Duration::from_secs(30);
const RETRY_LONGEST: Duration = Duration::from_secs(10 * 60);
/// A pipeline that came back asks for a full read only when the last one is older than this (else it waits until then).
const RESYNC_AFTER: Duration = Duration::from_secs(5 * 60);
/// An accepted invite is hidden at VRChat once the account is in its instance; if it never gets there in this time, the
/// card comes back (and VRChat still has the invite).
const ARRIVE_WITHIN: Duration = Duration::from_secs(10 * 60);
const PENDING_FOR: Duration = Duration::from_secs(10 * 60);
/// How long a VRChat that is up may take to say who is signed in (a person may have to sign in there) before a place that
/// waits for it is given up.
const PROOF_WITHIN: Duration = Duration::from_secs(5 * 60);
/// A VRChat that goes this soon after it came may be followed by the one that stays (VRChat's launcher, Easy Anti-Cheat).
const AGAIN_WITHIN: Duration = Duration::from_secs(60);
/// A VRChat that has run this long without a log to read is not going to have one (it makes it within seconds).
const LOG_WITHIN: Duration = Duration::from_secs(90);
/// A log nobody wrote in for this long is not that of a VRChat that runs.
const LOG_STILL: Duration = Duration::from_secs(10 * 60);
/// Why another profile does not start while VRChat runs on a Mac ([`orbiters_core::vrchat_launch::SEVERAL_HERE`]): Home says
/// it under its Play button, and a press that comes anyway is told it.
pub const ONE_GAME: &str = orbiters_i18n::mark("One VRChat runs at a time on a Mac: quit it to start on another profile.");
/// How long a Play asked for before VRChat was in Obelisk waits for it (its sign-in to Steam, then gigabytes to download).
const PLAY_WHEN_READY_FOR: Duration = Duration::from_secs(60 * 60);

/// The game on this computer, real or a test's.
pub trait Machine: Send + Sync {
    /// The VRChats running here: process id and whether it runs in Desktop mode (`--no-vr`).
    fn running(&self) -> Vec<(u32, bool)>;
    fn installed(&self) -> bool;
    /// Starts VRChat with these arguments (after `-applaunch 438100`).
    fn start(&self, args: &[String]) -> Result<(), String>;
    /// On a Mac, the render scale Home's next start asks for (25 to 100 %); nothing elsewhere.
    fn set_render_percent(&self, _percent: u32) {}
    /// Starts one more VRChat beside those running, with these arguments (`--profile=N` among them): through VRChat's own
    /// launcher, as the bridge does, since Steam starts a game once.
    fn start_beside(&self, args: &[String]) -> Result<(), String> {
        self.start(args)
    }
    /// The VRChats running here with their profile: process id, Desktop mode, `--profile` (0 without one) as its command
    /// line says it; `None` when its command line could not be read (the profile is not known).
    fn running_profiles(&self) -> Vec<(u32, bool, Option<u32>)> {
        self.running().into_iter().map(|(pid, desktop)| (pid, desktop, None)).collect()
    }
    /// The VRChats that run here as their own logs say them, where this machine is what knows (a Mac's); elsewhere `None`:
    /// the watcher's roster (`vrchat.runs`).
    fn roster(&self) -> Option<Vec<crate::state::VrchatRunShown>> {
        None
    }
    /// The same, looked at this moment: whose a VRChat is right before a private place is handed to it.
    fn roster_now(&self) -> Option<Vec<crate::state::VrchatRunShown>> {
        self.roster()
    }
    /// What a start needs that may take long (a program to download), done before anything is decided or held.
    fn make_ready(&self) -> Result<(), String> {
        Ok(())
    }
    /// The profile of the newest start made here, where starts are followed as this machine's own (a Mac's).
    fn newest_profile(&self) -> Option<u32> {
        None
    }
    /// The desktop window VRChat saved for its next start (Unity's `Screenmanager` values): width, height, fullscreen mode.
    fn saved_window(&self) -> Option<SavedWindow> {
        None
    }
    /// A Mac without what runs VRChat there (Obelisk): nothing to install the game with yet.
    fn engine_missing(&self) -> bool {
        false
    }
    /// Whether `start` only opens what the person presses Play in (Obelisk on a Mac): the options cannot be given,
    /// a destination waits for the game, and a running game takes a destination as an invite only.
    fn by_hand(&self) -> bool {
        false
    }
    /// Opens what runs VRChat here, or where it is got (`Action::OpenEngine`).
    fn open_engine(&self) -> Result<(), String> {
        Ok(())
    }
    /// Whether a by-hand machine starts the game itself after all (a Mac, through the engine without a window): `start`
    /// then takes the arguments, a destination goes with them, and nothing is opened for the person to press Play in.
    fn direct(&self) -> bool {
        false
    }
    /// A start is still under way (the engine or an update of the game may download before the game shows).
    fn starting(&self) -> bool {
        false
    }
    /// Why the last start failed after `start` returned, told once.
    fn start_failure(&self) -> Option<String> {
        None
    }
    /// Sets up what runs VRChat here (`Action::InstallEngine`), the game included; `progress` hears each step and
    /// `stop` ends it early.
    fn install_engine(&self, _progress: &dyn Fn(EngineSetup), _stop: &AtomicBool) -> Result<(), String> {
        Err(tr("VRChat could not be started: nothing to install on this computer.").into())
    }
    /// A Mac's machine, whose start the VRChat bridge shares (`None` elsewhere, and for a test's).
    fn as_mac(&self) -> Option<&MacMachine> {
        None
    }
    /// A record of what runs VRChat here that cannot be read, for Home (a Mac's engine).
    fn start_trouble(&self) -> Option<orbiters_core::launcher::StartTrouble> {
        None
    }
    /// Sets that record aside: where it went, or why it was left (in the person's words).
    fn set_aside(&self, _file: &str) -> Result<String, String> {
        Err(tr("There is no such record on this computer.").into())
    }
    /// The Game session of a VRChat this N4N0 started (Home) that still runs and lost its picture for good.
    fn picture_lost(&self) -> Option<String> {
        None
    }
    /// Starts as [`Machine::start`], for what `errand` asks for (kept with the start, to make it again).
    fn start_for(&self, args: &[String], _errand: &Errand) -> Result<(), String> {
        self.start(args)
    }
    /// The newest start made here, where starts are followed as this machine's own (a Mac's): its number, and the VRChat it
    /// became once it is up.
    fn newest_launch(&self) -> Option<(u64, Option<u32>)> {
        None
    }
    /// Why VRChat's log cannot be read here, asked while VRChat runs and none of its own is.
    fn log_problem(&self) -> Option<LogTrouble> {
        None
    }
    /// Quits that session (`session`, checked again: still Home's, still running, still lost) and starts VRChat again
    /// with what that very launch was started with, through the one start; blocks until done or refused (in the person's
    /// words). `may` says what that launch may be made with now (as it was; without its place, when that one goes after it), or
    /// why it may not be (the VRChat account it was for is not the one signed in): asked before the quit and again before the
    /// start.
    fn restart_lost(&self, _session: &str, _may: &dyn Fn(&HomeLaunch) -> Result<Vec<String>, String>) -> Result<(), String> {
        Err(tr("There is nothing to start again.").into())
    }
    /// The engine's files no VRChat starts on (a Mac's engine), and what ends it.
    fn engine_problem(&self) -> Option<orbiters_core::obelisk::EngineProblem> {
        None
    }
    /// Installs what the engine is made of again; `progress` hears how far it is. Blocks until done or not (in the
    /// person's words).
    fn repair_engine(&self, _progress: &dyn Fn(Option<u8>), _stop: &AtomicBool) -> Result<(), String> {
        Err(tr("There is nothing to repair on this computer.").into())
    }
}

/// How VRChat is started on this computer: through Steam only when Steam's library really holds it; else through the
/// launcher VRChat registers for its links (the Meta store's VRChat, or one installed outside Steam).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Steam,
    Launcher,
    NotInstalled,
}

pub fn route(steam_has_vrchat: bool, launcher_registered: bool) -> Route {
    if steam_has_vrchat {
        Route::Steam
    } else if launcher_registered {
        Route::Launcher
    } else {
        Route::NotInstalled
    }
}

#[derive(Default)]
pub struct RealMachine {
    /// Each VRChat's Desktop mode and profile from its command line, by process and start time.
    desktop: Mutex<CmdLines>,
    /// The route, kept ten minutes (it reads the registry and Steam's library file).
    route: Mutex<Option<(Instant, Route)>>,
    /// The VRChats that run and their logs, as looked at when a private place is about to be handed over.
    roster: Mutex<super::vrchats::Roster>,
    /// VRChat's folder (a test's; none: this computer's).
    folder: Option<std::path::PathBuf>,
}

const ROUTE_FOR: Duration = Duration::from_secs(10 * 60);

impl RealMachine {
    fn log_dir(&self) -> Option<std::path::PathBuf> {
        self.folder.clone().or_else(orbiters_core::paths::vrchat_log_dir)
    }

    fn route_now() -> Route {
        let steam_has_vrchat = crate::bridge::win::steam_exe().and_then(|exe| exe.parent().map(|d| d.to_path_buf())).and_then(|steam| orbiters_core::vrchat_launch::vrchat_dir(&steam)).is_some();
        route(steam_has_vrchat, crate::bridge::win::vrchat_launcher().is_some())
    }

    fn route(&self, fresh: bool) -> Route {
        let mut kept = self.route.lock().unwrap();
        match *kept {
            Some((at, r)) if !fresh && at.elapsed() < ROUTE_FOR => r,
            _ => {
                let r = Self::route_now();
                *kept = Some((Instant::now(), r));
                r
            }
        }
    }
}

/// VRChat on this Mac: started by N4N0 itself, through Obelisk's engine without its window (`obelisk_cli`).
pub struct MacMachine {
    /// Where N4N0 keeps the program it downloaded.
    tools: std::path::PathBuf,
    /// The program's library: VRChat, the engine, the records of what runs.
    library: std::path::PathBuf,
    play: Arc<super::obelisk_cli::Play>,
    /// Steam refused the saved sign-in at the last start: the set-up is offered again until it ran.
    signed_out: AtomicBool,
    /// Home's own starts not told yet: each failure is told by its launch id, never another start's (the bridge's), and a
    /// later start does not hide an earlier one's. Each with what it was asked with, to start that very one again.
    home_launch: std::sync::Mutex<Vec<HomeLaunch>>,
    /// The render scale Home's next Play asks for (Settings, `LaunchSettings::mac_render_percent`).
    render: std::sync::atomic::AtomicU32,
    /// The session being quit and started again: one at a time.
    restarting: std::sync::Mutex<Option<String>>,
    repairing: AtomicBool,
}

/// One of Home's own starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeLaunch {
    id: u64,
    /// What it was started with, its profile among them: what it is started again with.
    args: Vec<String>,
    /// The VRChat account its destination is of, when it is one's.
    account: Option<String>,
    /// Its destination is that account's alone, whatever its instance is.
    bound: bool,
    /// The destination it was started without, to be delivered after it.
    after: Option<Place>,
}

/// How many of Home's starts are kept.
const HOME_LAUNCHES: usize = 16;

impl MacMachine {
    pub fn new(home: &std::path::Path) -> Self {
        // A newer release of the program (the engine's fixes come with it) is got once per run, a little after the start.
        let tools = orbiters_core::obelisk::tools(&orbiters_core::paths::config_dir());
        let _ = std::thread::Builder::new().name("obelisk-update".into()).spawn(move || {
            if cfg!(test) {
                return;
            }
            std::thread::sleep(Duration::from_secs(15));
            super::obelisk_cli::update(&tools, &AtomicBool::new(false));
        });
        // Whatever VRChat the engine runs, its hangs and quits that do not finish are kept as they happen.
        #[cfg(target_os = "macos")]
        if !cfg!(test) {
            let (library, home) = (orbiters_core::obelisk::library(home), home.to_path_buf());
            let _ = std::thread::Builder::new().name("vrchat-incidents".into()).spawn(move || super::incident::follow(library, home, orbiters_core::paths::logs_dir()));
        }
        MacMachine {
            tools: orbiters_core::obelisk::tools(&orbiters_core::paths::config_dir()),
            library: orbiters_core::obelisk::library(home),
            play: Arc::new(super::obelisk_cli::Play::new(orbiters_core::paths::config_dir().join("obelisk-start-gate.json"))),
            signed_out: AtomicBool::new(false),
            home_launch: std::sync::Mutex::new(Vec::new()),
            render: std::sync::atomic::AtomicU32::new(orbiters_core::obelisk::DEFAULT_RENDER_PERCENT),
            restarting: std::sync::Mutex::new(None),
            repairing: AtomicBool::new(false),
        }
    }
}

impl MacMachine {
    /// Whether Game session `session` is one Home started here and it still runs (the program's word on that launch).
    fn home_session_running(&self, session: &str) -> bool {
        self.home_launch_of(session).is_some()
    }

    /// Home's own start whose Game session is `session`, while it runs: what it was asked with.
    fn home_launch_of(&self, session: &str) -> Option<HomeLaunch> {
        let launches = self.home_launch.lock().unwrap().clone();
        launches.into_iter().find(|home| self.play.launch(home.id).is_some_and(|l| l.session.as_deref() == Some(session) && l.running && !l.over))
    }

    /// The profile of the VRChat running as process `pid`: as its launcher's own command line says it (what the game was
    /// really given, whoever started it), else as the launch it is of was made. `None`: not known.
    fn profile_of(&self, pid: u32) -> Option<u32> {
        #[cfg(target_os = "macos")]
        let said = super::bridge_mac::born(pid).and_then(|birth| super::bridge_mac::started_with(pid, birth)).and_then(|(command, _)| super::incident::game_arguments(&command));
        #[cfg(not(target_os = "macos"))]
        let said: Option<Vec<String>> = None;
        match said {
            // (several, a saved flag beside the one asked for, name none)
            Some(args) => orbiters_core::vrchat_launch::profile_told(&args),
            None => {
                let launch = self.play.newest().filter(|l| !l.over)?;
                (launch.launcher == Some(pid) || (launch.launcher.is_none() && self.play.program() == Some(pid))).then_some(launch.profile)
            }
        }
    }

    /// Home's start with `args`, kept as one of its own.
    fn start_home(&self, launch: HomeLaunch, steam: Option<u64>) -> Result<(), super::obelisk_cli::Refused> {
        let id = self.start_launch(&launch.args, steam)?;
        let mut mine = self.home_launch.lock().unwrap();
        mine.push(HomeLaunch { id, ..launch });
        let drop_old = mine.len().saturating_sub(HOME_LAUNCHES);
        mine.drain(..drop_old);
        Ok(())
    }

    /// The newest release of the program there is, got when the one here is too old (a download: never while a start is held).
    fn newest_program(&self) {
        if !cfg!(test) && self.program().is_some_and(|program| orbiters_core::obelisk::too_old(&program)) {
            super::obelisk_cli::update(&self.tools, &AtomicBool::new(false));
        }
    }

    /// The program to start VRChat with.
    fn program_to_start(&self) -> Result<std::path::PathBuf, super::obelisk_cli::Refused> {
        let refused = |words: &str| super::obelisk_cli::Refused::Failed(words.to_string());
        self.program().ok_or_else(|| refused(tr("VRChat is not set up on this Mac yet.")))?;
        self.program()
            .filter(|program| !orbiters_core::obelisk::too_old(program))
            .ok_or_else(|| refused(tr("What runs VRChat on this Mac is too old and its new version could not be downloaded. Check the connection and try again.")))
    }

    /// The Steam account the program is signed in with now.
    fn steam_account(&self, program: &std::path::Path) -> Option<u64> {
        let mut account = None;
        super::obelisk_cli::run(program, &["status"], &AtomicBool::new(false), |event| {
            if let orbiters_core::obelisk::Event::Status { steam_id, .. } = event {
                account = steam_id.as_deref().and_then(|id| id.parse().ok());
            }
        })
        .ok()?;
        account
    }

    /// Whether the launch that is started again is still this person's to start: `may` it as it was, for its VRChat account, and
    /// its Steam account (`steam`, of the session that ran) the one it would run on.
    fn same_accounts(&self, launch: &HomeLaunch, steam: Option<u64>, program: &std::path::Path, may: &dyn Fn(&HomeLaunch) -> Result<Vec<String>, String>) -> Result<Vec<String>, String> {
        let args = may(launch)?;
        if steam.is_some() && self.steam_account(program) == steam {
            return Ok(args);
        }
        Err(tr("The account VRChat was started with is no longer the one signed in: press Play to start it on this one.").into())
    }

    fn restart(&self, session: &str, may: &dyn Fn(&HomeLaunch) -> Result<Vec<String>, String>) -> Result<(), String> {
        // Checked again on the click: the same session, still Home's, still running, its picture still lost.
        let launch = self
            .home_launch_of(session)
            .filter(|_| super::picture::lost().as_deref() == Some(session))
            .ok_or_else(|| tr("VRChat draws again, or is no longer the one Home started: nothing was done.").to_string())?;
        self.newest_program();
        let program = self.program().ok_or_else(|| tr("VRChat is not set up on this Mac yet.").to_string())?;
        let steam = orbiters_core::obelisk::session_record_of(&self.library, session).ok().flatten().and_then(|r| r.account);
        self.same_accounts(&launch, steam, &program, may)?;
        // Asked to quit as its own Quit does, that very session only, never forced.
        let quit = ["steam", "quit", orbiters_core::obelisk::STEAM_APP, "--session", session, "--wait", "60"];
        let mut refused = None;
        let ran = super::obelisk_cli::run_for(&program, &quit, Duration::from_secs(75), |line| {
            if let Some(code) = serde_json::from_str::<serde_json::Value>(line).ok().and_then(|v| (v["event"] == "error").then(|| v["code"].as_str().map(str::to_owned)).flatten()) {
                refused = Some(code);
            }
        });
        match refused.as_deref() {
            None | Some("not_running") => {}
            Some(code) => return Err(tr!("VRChat could not be asked to quit ({code}): quit it yourself, then press Play.", code = code).to_string()),
        }
        if let (None, Err(e)) = (&refused, ran) {
            return Err(tr!("VRChat could not be asked to quit: {reason}", reason = e).to_string());
        }
        // It ended (or ends): then the one start, which keeps its 80 s after an early end and one start at a time.
        let until = Instant::now() + Duration::from_secs(90);
        while self.home_session_running(session) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(500));
        }
        if self.home_session_running(session) {
            return Err(tr("VRChat did not quit in time: quit it yourself, then press Play.").into());
        }
        for _ in 0..2 {
            // Minutes may have passed: nothing is started for an account that is no longer the one signed in, nor over a
            // start made meanwhile (which the one start refuses).
            let args = self.same_accounts(&launch, steam, &program, may)?;
            // Said to the program too, which checks it against the session it starts with: no other account between the two.
            match self.start_home(HomeLaunch { args, ..launch.clone() }, steam) {
                Ok(()) => return Ok(()),
                Err(super::obelisk_cli::Refused::Wait(wait)) if wait <= Duration::from_secs(100) => std::thread::sleep(wait + Duration::from_secs(1)),
                Err(refused) => return Err(refused.words()),
            }
        }
        Err(tr("VRChat could not be started again yet: press Play in a moment.").into())
    }

    /// The start shared by Home's Play and the VRChat bridge (the bridge's `controller/bridge_mac.rs`).
    pub fn play(&self) -> &Arc<super::obelisk_cli::Play> {
        &self.play
    }

    /// The program's library (the engine's records).
    pub fn library(&self) -> &std::path::Path {
        &self.library
    }

    /// The program to start the game with, when it is there.
    pub fn program(&self) -> Option<std::path::PathBuf> {
        orbiters_core::obelisk::program(&self.tools)
    }

    /// Home's start of VRChat with `args` through the shared start, at the render scale chosen in Settings (`render`,
    /// 25 to 100 % of the display's drawing resolution, the panel's pixels at most): its launch id, or why not. The
    /// bridge's starts (`start_launch_reserved`) keep the engine's own size, which its coordinates are measured in.
    /// For the Steam account `steam` or not at all, when one is named.
    fn start_launch(&self, args: &[String], steam: Option<u64>) -> Result<u64, super::obelisk_cli::Refused> {
        let program = self.program_to_start()?;
        let render = self.render.load(Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        let sizes = crate::mac_display::sizes();
        #[cfg(not(target_os = "macos"))]
        let sizes: Option<((u32, u32), (u32, u32))> = None;
        let percent = sizes.map(|(backing, panel)| orbiters_core::obelisk::render_percent(render, backing.0, panel.0));
        self.play.start_rendered(&program, &self.library, args, percent, steam)
    }

    /// The same, with the reservation `token` the bridge took before it created the instance (`Play::reserve`).
    pub fn start_launch_reserved(&self, args: &[String], token: Option<u64>) -> Result<u64, super::obelisk_cli::Refused> {
        let program = self.program().ok_or_else(|| super::obelisk_cli::Refused::Failed(tr("VRChat is not set up on this Mac yet.").to_string()))?;
        self.play.start_reserved(&program, &self.library, args, token)
    }
}

impl Machine for MacMachine {
    fn running(&self) -> Vec<(u32, bool)> {
        // What the engine's own records name, while that very process lives (its birth as recorded: a process that took
        // the number of a game long gone is not it); else what this N4N0's start said. Records that could not be read are
        // no proof that nothing runs: then the program's word, as before a record exists.
        let recorded = orbiters_core::obelisk::session_games_born(&self.library);
        let recorded_none = recorded.as_ref().is_none_or(Vec::is_empty);
        let mut found: Vec<u32> = recorded.unwrap_or_default().into_iter().filter(|(pid, birth)| still_born(*pid, *birth)).map(|(pid, _)| pid).collect();
        // Before the record names the game's process, the program's word that it runs: under the process it said, else
        // under the program's own (alive for exactly as long as it follows the game), never a made-up 0.
        if found.is_empty() && recorded_none {
            if let Some(said) = self.play.running() {
                found.extend(said.or_else(|| self.play.program()));
            }
        }
        found.sort_unstable();
        found.dedup();
        // No headset on a Mac: the desktop game, always.
        found.into_iter().map(|pid| (pid, true)).collect()
    }

    fn running_profiles(&self) -> Vec<(u32, bool, Option<u32>)> {
        self.running().into_iter().map(|(pid, desktop)| (pid, desktop, self.profile_of(pid))).collect()
    }

    fn roster(&self) -> Option<Vec<crate::state::VrchatRunShown>> {
        use crate::state::{VrchatDriverShown, VrchatRunShown};
        // Only what its own log said of the very process that runs (its birth with its number).
        let Some(player) = super::incident::player().filter(|p| still_born(p.launcher, Some(p.birth))) else { return Some(Vec::new()) };
        // The one start is Home's and the bridge's: a running launch of it that is not Home's is the bridge's.
        let bridges = self.play.newest().is_some_and(|l| l.running && !l.over && l.launcher == Some(player.launcher) && !self.home_launch.lock().unwrap().iter().any(|home| home.id == l.id));
        Some(vec![VrchatRunShown {
            pid: player.launcher,
            profile: player.profile.unwrap_or(0),
            profile_known: player.profile.is_some(),
            account_id: player.proven().map(str::to_owned),
            account: player.account,
            signed_out: player.signed_out,
            logged: player.logged,
            desktop: Some(true),
            driver: if bridges { VrchatDriverShown::Agent { session: "main".into() } } else { VrchatDriverShown::Person }, // i18n: not shown (the session's name, as the bridge calls it)
            ..VrchatRunShown::default()
        }])
    }

    fn make_ready(&self) -> Result<(), String> {
        self.newest_program();
        Ok(())
    }

    fn newest_profile(&self) -> Option<u32> {
        self.play.newest().map(|l| l.profile)
    }

    fn newest_launch(&self) -> Option<(u64, Option<u32>)> {
        self.play.newest().map(|l| (l.id, l.launcher.filter(|_| !l.over)))
    }

    fn log_problem(&self) -> Option<LogTrouble> {
        log_trouble(&orbiters_core::obelisk::vrchat_log_dir(&self.library))
    }

    fn installed(&self) -> bool {
        !self.signed_out.load(Ordering::Relaxed) && orbiters_core::obelisk::engine(&self.tools, &self.library) == orbiters_core::obelisk::Engine::Ready
    }

    fn engine_missing(&self) -> bool {
        orbiters_core::obelisk::engine(&self.tools, &self.library) == orbiters_core::obelisk::Engine::Missing
    }

    fn by_hand(&self) -> bool {
        true
    }

    fn direct(&self) -> bool {
        true
    }

    fn set_render_percent(&self, percent: u32) {
        self.render.store(percent, Ordering::Relaxed);
    }

    fn start(&self, args: &[String]) -> Result<(), String> {
        self.start_home(HomeLaunch { args: args.to_vec(), ..HomeLaunch::default() }, None).map_err(|refused| refused.words())
    }

    fn start_for(&self, args: &[String], errand: &Errand) -> Result<(), String> {
        let with_it = orbiters_core::vrchat_launch::destination_of(args).is_some();
        let launch = HomeLaunch { id: 0, args: args.to_vec(), account: errand.owner().map(str::to_owned), bound: errand.bound, after: errand.destination.clone().filter(|_| !with_it) };
        self.start_home(launch, None).map_err(|refused| refused.words())
    }

    fn engine_problem(&self) -> Option<orbiters_core::obelisk::EngineProblem> {
        self.play.engine_problem()
    }

    fn repair_engine(&self, progress: &dyn Fn(Option<u8>), stop: &AtomicBool) -> Result<(), String> {
        if self.repairing.swap(true, Ordering::AcqRel) {
            return Err(tr("It is being repaired already.").into());
        }
        self.newest_program();
        let done = self.program_to_start().map_err(|refused| refused.words()).and_then(|program| {
            self.play.repair(&program, stop, progress).map_err(|failure| match failure.code.as_str() {
                "busy" => tr("A game runs on what runs VRChat: quit it, then press Repair.").to_string(),
                "voile_failed" => tr("What runs VRChat could not be downloaded again. Check the connection and the free space, and try again: nothing was changed.").to_string(),
                _ => failure.words,
            })
        });
        self.repairing.store(false, Ordering::Release);
        done
    }

    fn starting(&self) -> bool {
        self.play.starting()
    }

    fn start_failure(&self) -> Option<String> {
        let launches = self.home_launch.lock().unwrap().clone();
        let (id, failure) = launches.into_iter().find_map(|home| self.play.take_failure(home.id).map(|f| (home.id, f)))?;
        self.home_launch.lock().unwrap().retain(|home| home.id != id);
        if matches!(failure.code.as_str(), "not_signed_in" | "sign_in_expired" | "not_installed") {
            self.signed_out.store(true, Ordering::Relaxed);
        }
        Some(failure.words)
    }

    fn install_engine(&self, progress: &dyn Fn(EngineSetup), stop: &AtomicBool) -> Result<(), String> {
        super::obelisk_cli::set_up(&self.tools, progress, stop).map_err(|failure| failure.words)?;
        self.signed_out.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn as_mac(&self) -> Option<&MacMachine> {
        Some(self)
    }

    fn start_trouble(&self) -> Option<orbiters_core::launcher::StartTrouble> {
        let trouble = self.play.trouble(&self.library)?;
        Some(orbiters_core::launcher::StartTrouble {
            file: trouble.file.display().to_string(),
            folder: trouble.unlistable,
            blocks: trouble.blocks,
            set_aside_in: trouble.settled_in.map(|d| d.as_secs().max(1)),
        })
    }

    fn picture_lost(&self) -> Option<String> {
        let session = super::picture::lost()?;
        self.home_session_running(&session).then_some(session)
    }

    fn restart_lost(&self, session: &str, may: &dyn Fn(&HomeLaunch) -> Result<Vec<String>, String>) -> Result<(), String> {
        {
            let mut restarting = self.restarting.lock().unwrap();
            if restarting.is_some() {
                return Err(tr("VRChat is being started again already.").into());
            }
            *restarting = Some(session.to_owned());
        }
        let done = self.restart(session, may);
        *self.restarting.lock().unwrap() = None;
        done
    }

    fn set_aside(&self, file: &str) -> Result<String, String> {
        let aside = self.play.set_aside(&self.library, std::path::Path::new(file))?;
        let text = format!("Set aside VRChat's engine record {file}: it is now {}", aside.display()); // i18n: not shown (for the log)
        if !cfg!(test) {
            orbiters_core::notice_log::append(&orbiters_core::paths::logs_dir(), "detail", &text, time::OffsetDateTime::now_utc().unix_timestamp());
        }
        Ok(aside.display().to_string())
    }
}

/// Why the logs in VRChat's folder `dir` cannot be read while it runs; `None` where they can.
fn log_trouble(dir: &std::path::Path) -> Option<LogTrouble> {
    let denied = |path: &std::path::Path| Some(LogTrouble::Denied(path.display().to_string()));
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(LogTrouble::NoFolder),
        Err(_) => return denied(dir),
    };
    let logs = entries.flatten().filter(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.starts_with("output_log_") && name.ends_with(".txt")
    });
    let newest = logs.filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).max();
    match newest {
        None => Some(LogTrouble::NotWritten),
        Some((at, _)) if at.elapsed().is_ok_and(|still| still > LOG_STILL) => Some(LogTrouble::NotWritten),
        Some((_, log)) if std::fs::File::open(&log).is_err() => denied(&log),
        Some(_) => None,
    }
}

/// Whether process `pid` lives and, when its record says its birth, is that same process (a reused number is not).
fn still_born(pid: u32, birth: Option<(u64, u32)>) -> bool {
    #[cfg(target_os = "macos")]
    {
        let born = super::bridge_mac::born(pid);
        match birth {
            Some(birth) => born == Some(birth),
            None => born.is_some(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = birth;
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]), true, sysinfo::ProcessRefreshKind::nothing());
        system.process(sysinfo::Pid::from_u32(pid)).is_some()
    }
}

/// How often, and how many times, a VRChat's command line Windows would not give is asked again.
const CMD_RETRY: Duration = Duration::from_secs(10);
const CMD_TRIES: u32 = 6;

/// What each VRChat process's command line said (REQUEST15 second pass, S4): kept by process id and start time (a reused
/// id is read anew); an unreadable one is asked again every [`CMD_RETRY`], [`CMD_TRIES`] times, not cached for the life
/// of the process (its log may still say its profile through the roster).
#[derive(Default)]
pub struct CmdLines {
    seen: HashMap<u32, CmdLine>,
}

#[derive(Clone, Copy)]
struct CmdLine {
    start: u64,
    no_vr: bool,
    profile: Option<u32>,
    tries: u32,
    next: Instant,
}

impl CmdLines {
    /// Forgets the processes not in `alive` (id, start time); the ones whose command line is to be read now.
    pub fn due(&mut self, alive: &[(u32, u64)], now: Instant) -> Vec<u32> {
        self.seen.retain(|pid, line| alive.iter().any(|(p, s)| p == pid && *s == line.start));
        alive
            .iter()
            .filter(|(pid, _)| match self.seen.get(pid) {
                None => true,
                Some(line) => line.profile.is_none() && line.tries < CMD_TRIES && now >= line.next,
            })
            .map(|(pid, _)| *pid)
            .collect()
    }

    /// What one read of `pid`'s command line gave (empty: Windows would not say).
    pub fn record(&mut self, pid: u32, start: u64, args: &[String], now: Instant) {
        let tries = self.seen.get(&pid).filter(|l| l.start == start).map_or(0, |l| l.tries) + 1;
        let profile = (!args.is_empty()).then(|| orbiters_core::vrchat_launch::profile_of(args));
        self.seen.insert(pid, CmdLine { start, no_vr: args.iter().any(|a| a == "--no-vr"), profile, tries, next: now + CMD_RETRY });
    }

    /// Desktop mode and profile of `pid` as read (`None`: not known).
    pub fn get(&self, pid: u32) -> (bool, Option<u32>) {
        self.seen.get(&pid).map_or((false, None), |l| (l.no_vr, l.profile))
    }
}

/// The VRChats running (process, Desktop mode, profile): the profile as their own log said it once the roster read its
/// arguments, else as their command line says it, else unknown (`None`): a VRChat whose log has not said its arguments
/// yet is never taken for profile 0 (REQUEST15 fix B5).
fn runs_with_profiles(machine: &dyn Machine, roster: &[crate::state::VrchatRunShown]) -> Vec<(u32, bool, Option<u32>)> {
    machine
        .running_profiles()
        .into_iter()
        .map(|(pid, desktop, profile)| (pid, desktop, roster.iter().find(|r| r.pid == pid && r.profile_known).map(|r| r.profile).or(profile)))
        .collect()
}

/// Home's profile choice: every profile used before (with the account last seen on it), running or starting now, by
/// number; profile 0 always.
pub fn game_profiles(
    known: &BTreeMap<u32, String>,
    runs: &[(u32, bool, Option<u32>)],
    roster: &[crate::state::VrchatRunShown],
    starting: &[(u32, LaunchMode)],
) -> Vec<orbiters_core::launcher::GameProfile> {
    let mut numbers: Vec<u32> = std::iter::once(0).chain(known.keys().copied()).chain(runs.iter().filter_map(|r| r.2)).chain(starting.iter().map(|s| s.0)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
        .into_iter()
        .map(|profile| {
            let run = runs.iter().find(|r| r.2 == Some(profile));
            let shown = run.and_then(|r| roster.iter().find(|s| s.pid == r.0));
            orbiters_core::launcher::GameProfile {
                profile,
                account: shown.and_then(|s| s.account.clone()).or_else(|| known.get(&profile).cloned()),
                running: run.map(|r| if r.1 { LaunchMode::Desktop } else { LaunchMode::Vr }),
                starting: starting.iter().find(|s| s.0 == profile && run.is_none()).map(|s| s.1),
                assistant: shown.is_some_and(|s| s.driver != crate::state::VrchatDriverShown::Person),
            }
        })
        .collect()
}

/// What one press asks for, whole: carried as one thing through every hand-off (a start, a set-up that comes first, a game
/// that is still coming up), so that no part of it is ever used with another press's.
#[derive(Clone, Debug, PartialEq)]
pub struct Errand {
    mode: LaunchMode,
    /// The profile chosen; `None`: none was (the default one for a start, whichever runs for a game that does).
    profile: Option<u32>,
    destination: Option<Place>,
    /// The VRChat account whose list it was pressed on.
    account: Option<String>,
    /// The destination is that account's alone, whatever its instance is (an invite's, one made for it).
    bound: bool,
    /// Into the game that runs or starts only: none is ever started for it.
    running_only: bool,
    /// The invite it accepts.
    invite: Option<String>,
    /// Pressed again for a place that was not delivered: into the VRChat that runs on its profile, when one does.
    again: bool,
    /// The Orbiters member who asked for it, and when: what is said of it on its way is theirs alone.
    asked: Option<Stamp>,
}

impl Errand {
    /// VRChat's own start.
    fn plain(mode: LaunchMode, profile: Option<u32>) -> Self {
        Errand { mode, profile, destination: None, account: None, bound: false, running_only: false, invite: None, again: false, asked: None }
    }

    fn to(self, destination: Option<Place>, account: Option<&str>) -> Self {
        Errand { destination, account: account.map(str::to_owned), ..self }
    }

    /// A destination of a signed-in account's list: it goes only while that account is the one signed in and on show.
    fn scoped(&self) -> bool {
        self.destination.is_some() && self.account.is_some()
    }

    /// The account its destination is of, for a start that may be made again.
    fn owner(&self) -> Option<&str> {
        self.account.as_deref().filter(|_| self.destination.is_some())
    }

    /// Whose what is said of its destination on its way is: its account's, for the member who asked; nobody's (`Ok(None)`) for
    /// a public one asked for on no account's list. `Err`: an account's, and by whom is not known: it is said to nobody.
    fn by(&self) -> Result<Option<By>, ()> {
        if !self.private() && self.owner().is_none() {
            return Ok(None);
        }
        match (&self.asked, self.owner()) {
            (Some(stamp), Some(user)) => Ok(Some(By { stamp: stamp.clone(), user: user.to_owned() })),
            _ => Err(()),
        }
    }

    /// Whether its destination is not everyone's.
    fn private(&self) -> bool {
        self.destination.as_ref().is_some_and(|place| self.bound || !orbiters_core::vrchat_launch::is_public_instance(&place.instance_id))
    }
}

/// A destination asked for with a VRChat that is still coming up: it goes there once that one is up, or once that very one's
/// own log proves its account.
#[derive(Clone, Debug)]
struct Pending {
    errand: Errand,
    since: Instant,
    /// It goes only to a VRChat proven to be its account's ([`Way::AfterProof`]).
    proof: bool,
    /// The VRChats that ran before the start it waits for: none of them is it.
    before: Vec<u32>,
    /// That start, where starts are followed as this machine's own.
    launch: Option<u64>,
    /// That VRChat, once it was seen, and since when: no other one is it.
    seen: Option<(u32, Instant)>,
}

/// What a look at the VRChats that run says of a [`Pending`].
#[derive(Clone, Debug, PartialEq)]
enum Waited {
    Wait(JoinStep),
    Deliver,
    /// Forgotten: the game was not started.
    Over,
    Failed(JoinFailure),
}

impl Pending {
    /// `newest`: the machine's newest start ([`Machine::newest_launch`]); `starting`: it is still under way; `trouble`: why
    /// the log of a VRChat that runs, and has had the time to make one, cannot be read.
    fn look(&mut self, newest: Option<(u64, Option<u32>)>, runs: &[(u32, bool, Option<u32>)], roster: &[crate::state::VrchatRunShown], starting: bool, trouble: Option<&LogTrouble>) -> Waited {
        if !self.proof && self.since.elapsed() > PENDING_FOR {
            return Waited::Over;
        }
        if self.launch.is_some_and(|mine| newest.is_some_and(|(id, _)| id != mine)) {
            return Waited::Failed(JoinFailure::Elsewhere);
        }
        let process = newest.filter(|_| self.launch.is_some()).and_then(|(_, pid)| pid);
        let found = match self.seen {
            // (The start followed says it became another process than the one seen: that one is not it.)
            Some((pid, _)) if process.is_some_and(|said| said != pid) => return Waited::Failed(JoinFailure::Elsewhere),
            Some((pid, at)) => match runs.iter().find(|run| run.0 == pid) {
                Some(run) => Some(run),
                // One that went as soon as it came: a PC's start may have another come in its place, for a while.
                None if self.launch.is_none() && at.elapsed() < AGAIN_WITHIN => {
                    self.seen = None;
                    self.since = Instant::now().checked_sub(PENDING_FOR - AGAIN_WITHIN).unwrap_or(self.since);
                    return Waited::Wait(JoinStep::Starting);
                }
                None => return Waited::Failed(JoinFailure::Gone),
            },
            // The start followed is the one that became this process; elsewhere, the VRChat that came on its profile since.
            None => runs.iter().filter(|run| !self.before.contains(&run.0)).find(|run| match self.launch {
                Some(_) => process == Some(run.0),
                None => !self.proof || run.2 == self.errand.profile,
            }),
        };
        let Some(&(pid, _, profile)) = found else {
            // Up, and nothing of it can be read: not even its profile.
            if let Some(trouble) = trouble.filter(|_| self.proof && runs.iter().any(|run| run.2.is_none() && !self.before.contains(&run.0))) {
                return Waited::Failed(JoinFailure::Log(trouble.clone()));
            }
            return match self.proof && self.since.elapsed() > PENDING_FOR && !starting {
                true => Waited::Failed(JoinFailure::Gone),
                false => Waited::Wait(JoinStep::Starting),
            };
        };
        // Asked for with a start on another profile than the one that is up: not this game's.
        if self.errand.profile.is_some_and(|asked| profile.is_some_and(|on| on != asked)) {
            return Waited::Failed(JoinFailure::Elsewhere);
        }
        if !self.proof {
            return Waited::Deliver;
        }
        let since = self.seen.get_or_insert((pid, Instant::now())).1;
        let said = roster.iter().find(|r| r.pid == pid && r.profile_known && Some(r.profile) == self.errand.profile);
        match orbiters_core::vrchat_launch::way(true, self.errand.owner(), To::Running { several: true, proven: said.and_then(|r| r.account_id.as_deref()) }) {
            Way::Direct => Waited::Deliver,
            Way::OtherAccount => Waited::Failed(JoinFailure::OtherAccount),
            _ => match trouble {
                Some(trouble) if since.elapsed() > LOG_WITHIN => Waited::Failed(JoinFailure::Log(trouble.clone())),
                _ if since.elapsed() > PROOF_WITHIN => Waited::Failed(JoinFailure::NoProof),
                _ => Waited::Wait(JoinStep::SigningIn),
            },
        }
    }
}

/// Why a place was not handed over.
enum HandFailure {
    /// In the person's words.
    Said(Notice),
    Not(super::travel::GoFailure),
}

/// A private destination whose account is not known: nothing can prove a VRChat to be its own.
fn nobodys() -> Notice {
    Notice::Error(tr("This place is private: sign in to VRChat in Settings to go there.").into())
}

/// A place of one VRChat account, pressed as another's.
fn made_for_another() -> Notice {
    Notice::Error(tr("This place was made for another VRChat account: choose another one.").into())
}

/// The person pressed Join for a game that has gone since.
fn no_longer_running() -> Notice {
    Notice::Error(tr("VRChat is no longer running: choose a profile and press Play to start it there.").into())
}

/// A start on `profile` just asked for where one game runs at a time: shown at once, before the next look.
fn starting_on(machine: &mut MachineParts, profile: u32, mode: LaunchMode) {
    machine.game = Game::Starting(mode);
    machine.profiles.iter_mut().for_each(|p| p.starting = None);
    match machine.profiles.iter_mut().find(|p| p.profile == profile) {
        Some(p) => p.starting = Some(mode),
        None => {
            machine.profiles.push(orbiters_core::launcher::GameProfile { profile, starting: Some(mode), ..Default::default() });
            machine.profiles.sort_by_key(|p| p.profile);
        }
    }
}

/// This computer's.
fn real_machine() -> Arc<dyn Machine> {
    if cfg!(target_os = "macos") {
        // (A test's is nobody's home: nothing of this Mac's own is read.)
        let home = if cfg!(test) { std::env::temp_dir().join(format!("orbiters-no-home-{}", std::process::id())) } else { orbiters_core::obelisk::home() };
        Arc::new(MacMachine::new(&home))
    } else {
        Arc::new(RealMachine::default())
    }
}

impl Machine for RealMachine {
    fn running(&self) -> Vec<(u32, bool)> {
        self.running_profiles().into_iter().map(|(pid, desktop, _)| (pid, desktop)).collect()
    }

    fn running_profiles(&self) -> Vec<(u32, bool, Option<u32>)> {
        // Names and start times only (no process is opened for them); the command line is read for VRChat's processes
        // alone: once each, and again a few times while Windows would not give it.
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
        let alive: Vec<(u32, u64)> = system
            .processes()
            .values()
            .filter(|p| matches!(p.name().to_string_lossy().to_lowercase().as_str(), "vrchat.exe" | "vrchat"))
            .map(|p| (p.pid().as_u32(), p.start_time()))
            .collect();
        let mut lines = self.desktop.lock().unwrap();
        let now = Instant::now();
        let due: Vec<sysinfo::Pid> = lines.due(&alive, now).into_iter().map(sysinfo::Pid::from_u32).collect();
        if !due.is_empty() {
            system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&due), true, sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always));
            for pid in due {
                let args: Vec<String> = system.process(pid).map(|p| p.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect()).unwrap_or_default();
                let start = alive.iter().find(|(p, _)| *p == pid.as_u32()).map_or(0, |(_, s)| *s);
                lines.record(pid.as_u32(), start, &args, now);
            }
        }
        alive.iter().map(|(pid, _)| {
            let (no_vr, profile) = lines.get(*pid);
            (*pid, no_vr, profile)
        }).collect()
    }

    fn installed(&self) -> bool {
        self.route(false) != Route::NotInstalled
    }

    fn start(&self, args: &[String]) -> Result<(), String> {
        // Through Steam as a person does when Steam has VRChat; else VRChat's own launcher with the link first (as it
        // registers itself), the options after: never reordered, `-screen-fullscreen 0` stays a pair.
        match self.route(true) {
            Route::Steam => crate::bridge::win::start_vrchat(args).map_err(|f| f.message),
            Route::Launcher => crate::bridge::win::start_vrchat_beside(args).map_err(|f| f.message),
            Route::NotInstalled => Err(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into()),
        }
    }

    fn start_beside(&self, args: &[String]) -> Result<(), String> {
        // VRChat's own launch.exe with `--profile`: a process of its own every time (measured 2026-11-15, bridge).
        crate::bridge::win::start_vrchat_beside(args).map_err(|f| f.message)
    }

    fn saved_window(&self) -> Option<SavedWindow> {
        saved_window()
    }

    fn roster_now(&self) -> Option<Vec<crate::state::VrchatRunShown>> {
        let vrchats = super::bridge_side::vrchats_now();
        let runs = self.roster.lock().unwrap().scan_whole(&vrchats, self.log_dir().as_deref());
        Some(runs.iter().map(|run| super::vrchats::shown(run, None, None)).collect())
    }

    fn log_problem(&self) -> Option<LogTrouble> {
        match self.log_dir() {
            Some(dir) => log_trouble(&dir),
            None => Some(LogTrouble::NoFolder),
        }
    }
}

/// VRChat's saved window, read from its registry key (read only; only looked at while no VRChat runs).
#[cfg(windows)]
fn saved_window() -> Option<SavedWindow> {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let dword = |name: &str| {
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: a u32 and its size are passed; the names live for the call.
        let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(r"Software\VRChat\VRChat"), &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size)) };
        status.is_ok().then_some(value)
    };
    Some(SavedWindow {
        width: dword("Screenmanager Resolution Width_h182942802")?, // i18n: not shown (a registry value name)
        height: dword("Screenmanager Resolution Height_h2627697771")?, // i18n: not shown (a registry value name)
        mode: dword("Screenmanager Fullscreen mode_h3630240806").unwrap_or(3), // i18n: not shown (a registry value name)
    })
}

#[cfg(not(windows))]
fn saved_window() -> Option<SavedWindow> {
    None
}

/// What the launcher keeps besides the published state.
pub(super) struct Runtime {
    /// The account the state was read for (`None`: the local databases), and the Orbiters member.
    loaded_for: Option<Option<String>>,
    loaded_by: Option<Stamp>,
    loaded_at: Option<Instant>,
    /// Reads of VRChat that failed in a row, and when the next try is (none: the usual clock).
    failures: u32,
    retry_at: Option<Instant>,
    favourite_groups: Vec<(String, String)>,
    cooldowns: Cooldowns,
    /// Decoration template id → picture (`None` while unknown or failed).
    pictures: HashMap<String, Option<String>>,
    /// Which friend wears which decorations.
    worn: BTreeMap<String, Decorations>,
    /// World id → its name and picture (`None` once asked and not given).
    worlds: HashMap<String, Option<(String, String)>>,
    /// World and decoration ids whose lookup failed for a passing reason, and when: asked again after [`LOOKUP_AGAIN`].
    lookup_failed: HashMap<String, Instant>,
    /// Profiles' details read, by person: `/profile`, `/users`, mutuals.
    profiles: HashMap<String, (Instant, serde_json::Value, serde_json::Value, Option<(u32, u32)>)>,
    /// Boops put away here (id → when), kept in `launcher-dismissed.json`; read once.
    dismissed: Option<BTreeMap<String, i64>>,
    pipeline: Option<Arc<AtomicBool>>,
    /// Whose messages the pipeline's are.
    pipeline_by: Option<By>,
    /// The client (one sign-in, one grant) the pipeline was opened with.
    pipeline_client: Option<Arc<Mutex<Client>>>,
    /// Invites accepted here, waiting for the account to arrive in their instance before VRChat is told to hide them.
    accepted: Vec<Accepted>,
    /// Accepted invites whose hide is being sent now.
    hiding: Vec<String>,
    /// Bumped at each profile opened: an older request's answer writes nothing.
    profile_request: u64,
    /// Bumped at every change of the source on show (another account, a new sign-in, signed out): work started for an
    /// older one (a profile read) writes nothing.
    epoch: u64,
    /// The pipeline came back after a cut: read everything again (what came meanwhile was missed).
    resync: Arc<AtomicBool>,
    game_checked: Option<Instant>,
    machine: Arc<dyn Machine>,
    /// Where to go once the VRChat that is coming up is there, as it was asked for (it goes to that account only).
    pending: Option<Pending>,
    /// A place that was not delivered, as it was asked for: kept to try again.
    kept: Option<Errand>,
    /// The VRChats that run without a log of their own to read, since when; and why none can be read here.
    unlogged: HashMap<u32, Instant>,
    log_trouble: Option<LogTrouble>,
    /// On a Mac: Play was asked for while VRChat was not in Obelisk yet (its install, or Obelisk's own, was started
    /// from here); it is played once it is there, within [`PLAY_WHEN_READY_FOR`].
    play_when_ready: Option<Instant>,
    /// What is played then: the press last made, whole (none: VRChat's own start), and when it was made.
    queued: Option<(Errand, Instant)>,
    /// What the Mac's engine is made of is being installed again: how far, when known.
    repairing: Option<Option<u8>>,
    /// Starts under way, by profile (Home's Play and profile choice, Go there, the VRChat bridge): each claims its
    /// profile from before the start until a VRChat on it is seen, or [`PROFILE_HELD`] passed (REQUEST15 fix B4).
    starting: Vec<Claim>,
    claims_made: u64,
    /// VRChats whose profile is not known, since when (by process id), and a press on a profile told about them that a
    /// second press within [`CONFIRM_WITHIN`] confirms (REQUEST15 third pass, S4).
    unknown_since: HashMap<u32, Instant>,
    unknown_confirm: Option<(u32, Instant, Vec<u32>)>,
    /// The account's own profile changes no sooner than this ([`mine::CHANGE_EVERY`] apart).
    mine_next: Option<Instant>,
}

/// A start's claim on a VRChat profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Claim {
    pub profile: u32,
    pub mode: LaunchMode,
    pub at: Instant,
    pub id: u64,
    /// The person was told this start never showed: a press on its profile until then starts again.
    pub confirm_by: Option<Instant>,
}

impl Claim {
    fn holds(&self) -> bool {
        self.at.elapsed() < PROFILE_HELD
    }

    /// Still barring its profile to a start nobody pressed (the bridge's): holding, or lapsed for less than
    /// [`PROFILE_LAPSED`]. A person is asked about it however old it is (see [`Controller::launcher_claim`]).
    fn kept(&self) -> bool {
        self.at.elapsed() < PROFILE_HELD + PROFILE_LAPSED
    }
}

/// An invite accepted here, until VRChat is told to hide it (the account got there) or its card comes back.
#[derive(Clone)]
struct Accepted {
    invite: Invite,
    at: Instant,
    /// The account whose invite it is: only its own session hides it, only its own list gets the card back.
    owner: String,
    /// Where the account was at the press: being there already is no proof of having arrived.
    from: Option<(String, String)>,
    /// Arrived; the hide failed this many times (it is tried again a minute later, at most [`HIDE_TRIES`] times).
    hide_failures: u32,
    hide_again: Option<Instant>,
    /// The account was seen somewhere else than `from` since the press: coming back to `from` is an arrival then.
    moved: bool,
}

const HIDE_TRIES: u32 = 5;
const HIDE_AGAIN: Duration = Duration::from_secs(60);

impl Default for Runtime {
    fn default() -> Self {
        Self {
            loaded_for: None,
            loaded_by: None,
            pipeline_by: None,
            loaded_at: None,
            failures: 0,
            retry_at: None,
            favourite_groups: Vec::new(),
            cooldowns: Cooldowns::default(),
            pictures: HashMap::new(),
            worn: BTreeMap::new(),
            worlds: HashMap::new(),
            lookup_failed: HashMap::new(),
            profiles: HashMap::new(),
            dismissed: None,
            pipeline: None,
            pipeline_client: None,
            accepted: Vec::new(),
            hiding: Vec::new(),
            profile_request: 0,
            epoch: 0,
            resync: Arc::new(AtomicBool::new(false)),
            game_checked: None,
            machine: real_machine(),
            pending: None,
            kept: None,
            unlogged: HashMap::new(),
            log_trouble: None,
            play_when_ready: None,
            queued: None,
            repairing: None,
            starting: Vec::new(),
            claims_made: 0,
            mine_next: None,
            unknown_since: HashMap::new(),
            unknown_confirm: None,
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Everything VRChat says, read once: friends (online pages then offline pages), the account, favourite groups and who
/// is in them, invites and boops waiting. Each call is paced by the client (one a second per kind).
pub fn read_signed_in(client: &Mutex<Client>) -> Result<(LauncherState, Vec<(String, String)>, BTreeMap<String, Decorations>), Error> {
    let mut friends = BTreeMap::new();
    let mut worn = BTreeMap::new();
    let mut fields = PictureFields::default();
    for offline in [false, true] {
        for page in 0..50 {
            let items = client.lock().unwrap().friends_page(offline, page * 100)?;
            let n = items.len();
            for v in &items {
                if let Some(f) = launcher_feed::friend(v, !offline) {
                    if !friends.contains_key(&f.id) {
                        fields.count(&f.id, v);
                    }
                    worn.insert(f.id.clone(), launcher_feed::decorations(v));
                    friends.entry(f.id.clone()).or_insert(f);
                }
            }
            if n < 100 {
                break;
            }
        }
    }
    crate::worlds::profile_pictures::note(&fields.line());
    let me_value = client.lock().unwrap().current_user_value()?;
    let mut me_value = me_value;
    me_value["state"] = serde_json::Value::String("online".into());
    let me = launcher_feed::friend(&me_value, false);
    let groups = launcher_feed::favourite_groups(&client.lock().unwrap().favourite_groups_value()?);
    let members = launcher_feed::favourites(&client.lock().unwrap().favourite_friends_value()?);
    // Notifications are a nicety: their failure leaves the list.
    let invites: Vec<_> = client.lock().unwrap().notifications().unwrap_or_default().iter().filter_map(launcher_feed::invite).collect();
    let boops: Vec<_> = client.lock().unwrap().notifications_v2().unwrap_or_default().iter().filter_map(launcher_feed::boop).collect();
    let mut state = LauncherState {
        source: Source::SignedIn { user_id: me.as_ref().map(|m| m.id.clone()).unwrap_or_default(), name: me.as_ref().map(|m| m.name.clone()).unwrap_or_default() },
        possible: Possible { live_status: true, invites: true, request_invite: true, boop: true, play: true },
        me,
        friends: friends.into_values().collect(),
        invites,
        boops,
        boop_emojis: launcher_feed::boop_emojis(),
        ..LauncherState::default()
    };
    for f in &mut state.friends {
        f.favourite_groups = members.get(&f.id).cloned().unwrap_or_default();
    }
    let icons: HashMap<String, String> = state.friends.iter().map(|f| (f.id.clone(), f.look.icon.clone())).collect();
    for i in &mut state.invites {
        i.from_icon = icons.get(&i.from_id).cloned().unwrap_or_default();
    }
    for b in &mut state.boops {
        if b.from_icon.is_empty() {
            b.from_icon = icons.get(&b.from_id).cloned().unwrap_or_default();
        }
    }
    state.invites.sort_by_key(|i| std::cmp::Reverse(i.received_at));
    state.boops.sort_by_key(|b| std::cmp::Reverse(b.received_at));
    launcher_feed::regroup(&mut state, &groups);
    Ok((state, groups, worn))
}

/// Which field each person's picture came from in VRChat's friends list ([`launcher_feed::ICON_FIELDS`]), for the
/// pictures' log: counts, and digests of the ids of those with none (never a name or an address).
#[derive(Default)]
struct PictureFields {
    from: BTreeMap<&'static str, usize>,
    none: Vec<String>,
}

impl PictureFields {
    fn count(&mut self, id: &str, v: &serde_json::Value) {
        match launcher_feed::icon_field(v) {
            Some(field) => *self.from.entry(field).or_default() += 1,
            None => self.none.push(crate::worlds::profile_pictures::who(Some(id))),
        }
    }

    fn line(&self) -> String {
        let from: Vec<String> = self.from.iter().map(|(field, n)| format!("{field} {n}")).collect();
        let none: Vec<&str> = self.none.iter().take(40).map(String::as_str).collect();
        format!("friends list pictures from: {}; none {} {}", from.join(", "), self.none.len(), none.join(" ")) // i18n: not shown (a log line)
    }
}

/// How long after the `failures`-th failed read in a row the next one is: 30 s doubling to 10 minutes, spread by
/// `random` over three quarters to five quarters of that, and never before VRChat's own "try again in".
fn retry_wait(failures: u32, e: &Error, random: u16) -> Duration {
    let base = RETRY_FIRST.saturating_mul(1 << failures.saturating_sub(1).min(10)).min(RETRY_LONGEST);
    let wait = base.mul_f64(0.75 + f64::from(random) / 131072.0);
    match e {
        Error::RateLimited(d) => wait.max(*d),
        _ => wait,
    }
}

fn random() -> u16 {
    let b = *uuid::Uuid::new_v4().as_bytes();
    u16::from_be_bytes([b[0], b[1]])
}

/// A token for one pipeline connection: only a session that is over stops the pipeline; anything else (no network, a
/// time-out, a 429, a 5xx) is tried again after the pipeline's wait.
fn pipeline_token(client: &Mutex<Client>) -> Result<String, orbiters_core::pipeline::NoToken> {
    token_outcome(client.lock().unwrap().pipeline_token().map(|s| s.0))
}

fn token_outcome(token: Result<String, Error>) -> Result<String, orbiters_core::pipeline::NoToken> {
    use orbiters_core::pipeline::NoToken;
    token.map_err(|e| match e {
        Error::Unauthorized | Error::Disabled => NoToken::Refused,
        _ => NoToken::Later,
    })
}

fn notice_or_changed(notice: Option<Notice>) -> Notice {
    notice.unwrap_or_else(changed_account)
}

fn changed_account() -> Notice {
    Notice::Error(tr("Your VRChat account changed: try again.").into())
}

/// Runs `send` on `client` only while it is still `account`'s session, under the one lock the send itself takes: a
/// sign-in that changed between the press and now never sends.
fn send_as<T>(client: &Mutex<Client>, account: &str, send: impl FnOnce(&mut Client) -> Result<T, Error>) -> Result<T, Notice> {
    let mut c = client.lock().unwrap();
    if c.signed_in_user_id() != Some(account) {
        return Err(changed_account());
    }
    send(&mut c).map_err(|e| Notice::Error(error_text(&e)))
}

/// The words for an action that could not be done (in the page's language).
pub fn error_text(e: &Error) -> String {
    e.to_string()
}

impl Controller {
    /// The signed-in VRChat session, when the feature and the session allow it, and whose it is.
    fn launcher_client(&self) -> Option<Session> {
        self.vrchat_session()
    }

    /// The client, only when the list on show is this very account's: a press on a row read for another account (the
    /// sign-in changed, its new list is still being read) never sends from the new one.
    /// `expected` is the account the press was made on ([`Action::As`]); a press that names none sends nothing.
    pub(super) fn launcher_sender(&self, expected: Option<&str>) -> Result<Session, Notice> {
        let Some(session) = self.launcher_client() else {
            if self.view().vrchat_login.waiting {
                return Err(Notice::Error(tr("Your VRChat session waits for Orbiters to answer. Try again in a moment.").into()));
            }
            return Err(Notice::Error(tr("Sign in to VRChat in Settings to boop and ask for invites.").into()));
        };
        match (&self.view().launcher.source, expected) {
            (Source::SignedIn { user_id, .. }, Some(expected)) if *user_id == session.by.user && session.by.user == expected => Ok(session),
            _ => Err(changed_account()),
        }
    }

    pub(super) fn start_launcher(self: &Shared) {
        let weak = Arc::downgrade(self);
        // A VRChat account just signed in rings it: its list is read at once, not at the next look (r21).
        let bell = self.vrchat_login.lock().unwrap().signed_bell.clone();
        let _ = std::thread::Builder::new().name("launcher".into()).spawn(move || {
            let mut seen = bell.rung();
            loop {
                let Some(ctl) = weak.upgrade() else { break };
                ctl.launcher_tick();
                drop(ctl);
                seen = bell.wait(seen, Duration::from_secs(2));
            }
        });
    }

    /// One look: is the source still the one read, is a read due, is the game running.
    fn launcher_tick(self: &Shared) {
        // The VRChat session waits for Orbiters (the environment or account just changed, Orbiters out of reach): the list
        // on show stays as it is, neither cleared nor replaced by the local copy, and nothing is read until it is back (r20).
        let session = self.launcher_client();
        let told = self.view().vrchat_login.user.as_ref().map(|u| u.id.clone());
        // Signed in, but the window is not told yet (the sign-in's own look comes first): its list would be nobody's.
        let untold = session.as_ref().is_some_and(|s| told.as_deref() != Some(s.by.user.as_str()));
        if self.view().vrchat_login.waiting || untold {
            let (machine, check_game) = {
                let mut rt = self.launcher.lock().unwrap();
                let check_game = rt.game_checked.is_none_or(|t| t.elapsed() > GAME_EVERY);
                if check_game {
                    rt.game_checked = Some(Instant::now());
                }
                (rt.machine.clone(), check_game)
            };
            if check_game {
                self.launcher_game(machine.as_ref());
            }
            return;
        }
        let account = session.as_ref().map(|s| s.by.user.clone());
        let by = session.as_ref().map(|s| s.by.clone());
        let client = session.map(|s| s.client);
        let reopen;
        let source_changed;
        let mut dropped = false;
        let mut forgotten = false;
        let (due, machine, check_game) = {
            let mut rt = self.launcher.lock().unwrap();
            // Another account of VRChat's, or the same one for another Orbiters member (or at another time of theirs).
            let another = matches!((&rt.loaded_by, &by), (Some(a), Some(b)) if !a.same(&b.stamp));
            let changed = rt.loaded_for.as_ref() != Some(&account) || another;
            // The same account signed in again (a new grant, a new client): its old connection stops, a new one opens.
            let regranted = !changed && rt.pipeline.is_some() && !matches!((&rt.pipeline_client, &client), (Some(a), Some(b)) if Arc::ptr_eq(a, b));
            if changed || regranted {
                if let Some(stop) = rt.pipeline.take() {
                    stop.store(true, Ordering::Release);
                }
                rt.pipeline_client = None;
                rt.pipeline_by = None;
            }
            // Its live updates come back with a new grant, also once they stopped on the old one's revocation (r20).
            let stopped = !changed && rt.pipeline.is_none() && rt.loaded_at.is_some() && account.is_some();
            reopen = (regranted || stopped) && client.is_some();
            if changed {
                rt.pictures.clear();
                rt.worn.clear();
            }
            if changed || regranted {
                rt.epoch += 1;
            }
            source_changed = changed && rt.loaded_for.is_some();
            if changed {
                rt.failures = 0;
                rt.retry_at = None;
                // What one account was shown (or refused) is not another's: its caches, its accepted invites, its
                // destination waiting for a Mac's game.
                rt.worlds.clear();
                rt.lookup_failed.clear();
                rt.profiles.clear();
                rt.accepted.clear();
                dropped = rt.pending.take().is_some();
                forgotten = rt.kept.take().is_some() || dropped;
                // A cool-down taken by one account's press says nothing of another's.
                rt.cooldowns = Cooldowns::default();
            }
            let every = if account.is_some() { FULL_EVERY } else { LOCAL_EVERY };
            // The pipeline came back: a full read, but not more often than every few minutes (kept pending meanwhile).
            let resync = rt.resync.load(Ordering::Acquire) && rt.loaded_at.is_none_or(|t| t.elapsed() > RESYNC_AFTER);
            let due = changed
                || match rt.retry_at {
                    // After a failure only its own clock counts.
                    Some(at) => Instant::now() >= at,
                    None => resync || rt.loaded_at.is_none_or(|t| t.elapsed() > every),
                };
            if due {
                rt.resync.store(false, Ordering::Release);
            }
            let check_game = rt.game_checked.is_none_or(|t| t.elapsed() > GAME_EVERY);
            if check_game {
                rt.game_checked = Some(Instant::now());
            }
            (due, rt.machine.clone(), check_game)
        };
        if forgotten {
            self.joining_goes(None);
        }
        if dropped {
            self.launcher_say(changed_account());
        }
        if check_game {
            self.launcher_game(machine.as_ref());
        }
        if let (true, Some(client), Some(by)) = (reopen, &client, &by) {
            self.launcher_pipeline(client.clone(), by.clone());
        }
        // Another source: the previous account's list (friends, invites, notes, the open profile) is not shown to it,
        // not even while its own list is read or failing to be read.
        if source_changed {
            if !self.blank_launcher(|l| matches!(&l.source, Source::SignedIn { user_id, .. } if Some(user_id) != account.as_ref())) {
                self.forget_launcher(|l| l.profile = None);
            }
        }
        if due {
            if self.view().launcher.friends.is_empty() {
                self.forget_launcher(|l| l.loading = true);
            }
            match (&client, &by) {
                (Some(client), Some(by)) => self.launcher_read_signed_in(client, by),
                _ => self.launcher_read_local(),
            }
        }
        self.launcher_settle_accepted(client.as_ref(), by.as_ref());
        if let (Some(client), Some(by)) = (&client, &by) {
            self.launcher_resolve_pictures(client, by);
            self.launcher_resolve_worlds(client, by);
        }
    }

    /// The names and pictures of the worlds friends are in (VRChat's friend list names the instance only): two a look,
    /// each kept for the session (the client keeps worlds a day too).
    fn launcher_resolve_worlds(&self, client: &Arc<Mutex<Client>>, by: &By) {
        let wanted: Vec<String> = {
            let rt = self.launcher.lock().unwrap();
            let mut ids = unnamed_worlds(&self.view().launcher);
            ids.retain(|id| !rt.worlds.contains_key(id) && rt.lookup_failed.get(id).is_none_or(|t| t.elapsed() > LOOKUP_AGAIN));
            ids.truncate(2);
            ids
        };
        let mut found = Vec::new();
        let mut failed = Vec::new();
        for id in wanted {
            match client.lock().unwrap().world(&id) {
                Ok(w) => found.push((id, Some((w.name, if w.thumbnail_image_url.is_empty() { w.image_url } else { w.thumbnail_image_url })))),
                // Not shown to this account (private, deleted): asked no more.
                Err(Error::NotFound) => found.push((id, None)),
                Err(Error::RateLimited(_)) => break,
                // Passing (a time-out, VRChat unreachable): again later, not never.
                Err(_) => failed.push(id),
            }
        }
        let worlds = {
            let mut rt = self.launcher.lock().unwrap();
            rt.worlds.extend(found);
            for id in failed {
                rt.lookup_failed.insert(id, Instant::now());
            }
            // Nothing known to write in (every unnamed world still unknown or not given): no copy, no publish. A world
            // already known is still written into a friend who just moved there.
            if !unnamed_worlds(&self.view().launcher).iter().any(|id| matches!(rt.worlds.get(id), Some(Some(_)))) {
                return;
            }
            rt.worlds.clone()
        };
        self.publish_launcher(by, |state| fill_worlds(state, &worlds));
    }

    fn launcher_game(self: &Shared, machine: &dyn Machine) {
        let view = self.view();
        // A Mac runs its own VRChat through Obelisk whatever it follows on the network; where Obelisk is not, a Mac
        // that follows a VRChat elsewhere is not told to get it.
        let remote = view.vrchat.remote;
        let here = if machine.by_hand() { !(remote && machine.engine_missing()) } else { orbiters_core::osc::VRCHAT_RUNS_HERE && !remote };
        // The watchers list every VRChat every 3 s already: the processes are looked at here only when they see one, or
        // while a start is under way (a Mac's game is looked for by its own scan).
        // The claims as they are before this look at the processes: only these may be ended by what it sees.
        let observed: Vec<u64> = self.launcher.lock().unwrap().starting.iter().map(|c| c.id).collect();
        let claimed = !observed.is_empty();
        let look = here && (machine.by_hand() || !view.vrchat.runs.is_empty() || view.vrchat.running || claimed || matches!(view.launcher.game, Game::Starting(_) | Game::Running(_)));
        let roster = machine.roster().unwrap_or_else(|| view.vrchat.runs.clone());
        let runs = if look { runs_with_profiles(machine, &roster) } else { Vec::new() };
        let claims = self.launcher_retire_claims(&observed, &runs);
        {
            let mut rt = self.launcher.lock().unwrap();
            let unknown: Vec<u32> = runs.iter().filter(|r| r.2.is_none()).map(|r| r.0).collect();
            rt.unknown_since.retain(|pid, _| unknown.contains(pid));
            for pid in unknown {
                rt.unknown_since.entry(pid).or_insert_with(Instant::now);
            }
        }
        // The accounts seen on each profile are remembered, so the choice says whose each one is.
        let learned: Vec<(u32, String)> = roster
            .iter()
            // Only a profile its log confirmed: until then the roster's 0 means "not known".
            .filter(|r| r.profile_known)
            .filter_map(|r| r.account.clone().filter(|a| !a.is_empty()).map(|a| (r.profile, a)))
            .filter(|(p, a)| view.settings.launch.profiles.get(p) != Some(a) && *p <= orbiters_core::vrchat_launch::PROFILE_MAX)
            .collect();
        if !learned.is_empty() {
            self.update_settings(|s| s.launch.profiles.extend(learned));
        }
        let running: Vec<(u32, bool)> = runs.iter().map(|r| (r.0, r.1)).collect();
        let trouble = if here { machine.start_trouble() } else { None };
        let picture_lost = if here { machine.picture_lost() } else { None };
        let repairing = self.launcher.lock().unwrap().repairing;
        let engine_trouble = if here { machine.engine_problem() } else { None }.map(|p| orbiters_core::launcher::EngineTrouble { paths: p.paths, action: p.action, repairing: repairing.is_some(), percent: repairing.flatten() });
        // A start that failed after it was asked for (the game's own start, far from the press): said, and over.
        let failed = machine.start_failure();
        if let Some(words) = &failed {
            *self.launcher_started.lock().unwrap() = None;
            let pending = self.launcher.lock().unwrap().pending.take();
            if let Some(pending) = pending.filter(|pending| pending.proof) {
                self.launcher_join_failed(pending.errand, JoinFailure::Gone);
            }
            let notice = Notice::Error(tr!("VRChat could not be started: {reason}", reason = words));
            let asked = self.vrchat_now();
            self.launcher_notice(asked.as_ref(), notice.clone());
            let ctl = self.clone();
            let _ = std::thread::Builder::new().name("launcher-start-failed".into()).spawn(move || {
                std::thread::sleep(NOTICE_FOR);
                ctl.launcher_notice_over(asked.as_ref(), &notice);
            });
        }
        let game = match running.first() {
            Some((_, desktop)) => Game::Running(if *desktop { LaunchMode::Desktop } else { LaunchMode::Vr }),
            None if !here => Game::NotRunning,
            None => match view.launcher.game {
                // Still starting for as long as the machine says so (the engine may download first).
                Game::Starting(mode) if failed.is_none() && machine.starting() => Game::Starting(mode),
                // A start just asked for is "starting" until VRChat shows up (or a minute passes).
                Game::Starting(mode) if failed.is_none() && self.launcher_started_recently() => Game::Starting(mode),
                _ if machine.engine_missing() => Game::NoEngine,
                _ if !machine.installed() => Game::NotInstalled,
                _ => Game::NotRunning,
            },
        };
        // The start asked for on a Mac came up: quit soon after (to play on another profile), it is not one still under way.
        // (Held by a press being decided right now, it is looked at again at the next look.)
        if matches!(game, Game::Running(_)) && machine.direct() {
            if let Ok(mut started) = self.launcher_started.try_lock() {
                *started = None;
            }
        }
        // A Mac's one start is followed by its machine, not by the claims of the starts beside one another.
        let starting = match game {
            Game::Starting(mode) if machine.by_hand() => vec![(machine.newest_profile().unwrap_or(0), mode)],
            _ => claims,
        };
        // Where the game is started by hand in another app, no profile can be given to it.
        let profiles = if here && (!machine.by_hand() || machine.direct()) { game_profiles(&self.view().settings.launch.profiles, &runs, &roster, &starting) } else { Vec::new() };
        let several = self.launcher_several(machine, None);
        if here {
            self.launcher_look_for_logs(machine, &runs, &roster, several);
            self.launcher_go_pending(machine, &runs, &roster);
        }
        let log_trouble = self.launcher.lock().unwrap().log_trouble.clone();
        // VRChat is in Obelisk now, and Play was asked for before it was: played at once (Obelisk comes to the front).
        let waited = match game {
            Game::NoEngine | Game::NotInstalled => None,
            _ => self.launcher.lock().unwrap().play_when_ready.take(),
        };
        if let Some(since) = waited {
            // Used once, whole, whatever comes of it.
            let queued = self.launcher.lock().unwrap().queued.take();
            let fresh = |at: &Instant| at.elapsed() < PLAY_WHEN_READY_FOR;
            if machine.by_hand() && matches!(game, Game::NotRunning) && fresh(&since) {
                let errand = match queued {
                    Some((errand, at)) if fresh(&at) => errand,
                    old => Errand::plain(LaunchMode::Desktop, old.and_then(|(errand, _)| errand.profile)),
                };
                let ctl = self.clone();
                let _ = std::thread::Builder::new().name("launcher-queued".into()).spawn(move || {
                    let notice = ctl.launcher_run(errand);
                    ctl.launcher_say(notice);
                });
            } else if let Some((errand, _)) = queued.filter(|(errand, _)| errand.destination.is_some()) {
                // Its game did not come, or another is there: said, and nothing is started for it.
                self.launcher_join_failed(errand, JoinFailure::Elsewhere);
            }
        }
        // Published only when something changed (this runs every few seconds).
        self.publish_machine(|m| {
            let unresolved = runs.iter().filter(|r| r.2.is_none()).count() as u32;
            // A set-up that ended (VRChat is there to play) is no longer on show.
            if matches!(m.engine_setup, Some(EngineSetup::Checking | EngineSetup::Game { .. })) && !matches!(game, Game::NotInstalled | Game::NoEngine) {
                m.engine_setup = None;
            }
            m.several_profiles = several;
            m.log_trouble = log_trouble;
            m.picture_lost = picture_lost;
            m.engine_trouble = engine_trouble;
            m.unresolved = unresolved;
            m.start_trouble = trouble;
            m.profiles = profiles;
            m.game = game;
            m.play = here;
        });
    }

    fn launcher_started_recently(&self) -> bool {
        // A game the person starts by hand in Obelisk takes its sign-in, an update check and Easy Anti-Cheat first.
        let patience = if self.launcher.lock().unwrap().machine.by_hand() { MAC_STARTING_FOR } else { STARTING_FOR };
        self.launcher_started.lock().unwrap().is_some_and(|t| t.elapsed() < patience)
    }

    /// Whether the logs of the VRChats that run are read, and why not where they have had the time (with one VRChat profile
    /// nothing goes by a log, and none is looked for).
    fn launcher_look_for_logs(&self, machine: &dyn Machine, runs: &[(u32, bool, Option<u32>)], roster: &[crate::state::VrchatRunShown], several: bool) {
        if !several {
            let mut rt = self.launcher.lock().unwrap();
            rt.unlogged.clear();
            rt.log_trouble = None;
            return;
        }
        let unread: Vec<u32> = runs.iter().map(|run| run.0).filter(|pid| !roster.iter().any(|r| r.pid == *pid && r.logged)).collect();
        let late = {
            let mut rt = self.launcher.lock().unwrap();
            rt.unlogged.retain(|pid, _| unread.contains(pid));
            for pid in &unread {
                rt.unlogged.entry(*pid).or_insert_with(Instant::now);
            }
            if runs.len() > unread.len() {
                rt.log_trouble = None;
            }
            runs.len() == unread.len() && rt.unlogged.values().any(|since| since.elapsed() > LOG_WITHIN)
        };
        if late {
            let trouble = machine.log_problem();
            self.launcher.lock().unwrap().log_trouble = trouble;
        }
    }

    /// The destination that waits for a VRChat coming up: delivered once that one is up (as an invite to oneself, which VRChat
    /// shows once the person is in), or, for a private one where there are several profiles, once that very VRChat's own log
    /// proves its account; else Home says why not, and it is kept.
    fn launcher_go_pending(self: &Shared, machine: &dyn Machine, runs: &[(u32, bool, Option<u32>)], roster: &[crate::state::VrchatRunShown]) {
        let newest = machine.newest_launch();
        let starting = machine.starting();
        let (waited, pending) = {
            let mut rt = self.launcher.lock().unwrap();
            let trouble = rt.log_trouble.clone().filter(|_| rt.unlogged.values().any(|since| since.elapsed() > LOG_WITHIN));
            let Some(pending) = rt.pending.as_mut() else { return };
            let waited = pending.look(newest, runs, roster, starting, trouble.as_ref());
            let pending = pending.clone();
            if !matches!(waited, Waited::Wait(_)) {
                rt.pending = None;
            }
            (waited, pending)
        };
        // The VRChat it was proven for, or that came up for it: no other one takes it.
        let Pending { errand, proof, seen, .. } = pending;
        match waited {
            Waited::Wait(step) if proof => self.joining_goes(Some(step)),
            Waited::Wait(_) | Waited::Over => {}
            Waited::Failed(why) => self.launcher_join_failed(errand, why),
            Waited::Deliver => {
                let ctl = self.clone();
                let asked = self.vrchat_now();
                let _ = std::thread::Builder::new().name("launcher-pending".into()).spawn(move || {
                    // To the account it was asked for only (the pending one is dropped at an account change, and checked here).
                    let notice = ctl.launcher_deliver(&errand, seen.map(|(pid, _)| pid));
                    match &notice {
                        Notice::Error(words) if proof => ctl.launcher_join_failed(errand, JoinFailure::NotSent(words.clone())),
                        _ if proof => ctl.joining_goes(None),
                        _ => {}
                    }
                    ctl.launcher_notice(asked.as_ref(), notice.clone());
                    std::thread::sleep(NOTICE_FOR);
                    ctl.launcher_notice_over(asked.as_ref(), &notice);
                });
            }
        }
    }

    /// VRChat was started without the private place of `errand`: it waits for that VRChat to prove its account. `before`: the
    /// VRChats that ran before that start.
    fn launcher_join_after(&self, machine: &dyn Machine, errand: Errand, before: Vec<u32>) {
        let world = errand.destination.as_ref().map(|place| place.world_name.clone()).unwrap_or_default();
        let by = errand.by();
        {
            let mut rt = self.launcher.lock().unwrap();
            let profile = errand.profile.or(Some(0));
            rt.kept = None;
            rt.pending = Some(Pending { errand: Errand { profile, ..errand }, since: Instant::now(), proof: true, before, launch: machine.newest_launch().map(|(id, _)| id), seen: None });
        }
        // (Nothing to say to anyone else signed in by now, nor of the place before.)
        if !by.is_ok_and(|by| self.publish_joining(by.as_ref(), Joining { world, step: JoinStep::Starting })) {
            self.joining_goes(None);
        }
    }

    /// A place that was not delivered: Home says why, it is kept, and the card of the invite it is of comes back.
    fn launcher_join_failed(&self, errand: Errand, why: JoinFailure) {
        let world = errand.destination.as_ref().map(|place| place.world_name.clone()).unwrap_or_default();
        let by = errand.by();
        {
            let mut rt = self.launcher.lock().unwrap();
            if let Some(accepted) = errand.invite.as_ref().and_then(|id| rt.accepted.iter_mut().find(|a| a.invite.id == *id)) {
                // (Given back as one that never got there: at the next look.)
                accepted.at = Instant::now().checked_sub(ARRIVE_WITHIN + Duration::from_secs(1)).unwrap_or(accepted.at);
            }
            rt.kept = Some(errand);
        }
        // (Nothing to say to anyone else signed in by now.)
        if !by.is_ok_and(|by| self.publish_joining(by.as_ref(), Joining { world, step: JoinStep::Failed(why) })) {
            self.joining_goes(None);
        }
    }

    /// Go there's start for a private place that does not go with one, of the VRChat account `account`: on the default profile,
    /// the place after it.
    pub(super) fn launcher_start_there(self: &Shared, world: &str, instance: &str, bound: bool, account: &str) -> Result<(), String> {
        let Some(place) = launcher_feed::place(&format!("{world}:{instance}"), "") else { return Err(tr("VRChat does not know this world.").into()) };
        let mode = self.view().settings.launch.last_mode;
        match self.launcher_play_on(Errand { bound, asked: self.vrchat_now(), ..Errand::plain(mode, Some(0)).to(Some(place), Some(account)) }) {
            Notice::Error(words) => Err(words),
            _ => Ok(()),
        }
    }

    /// The place kept is given up.
    fn launcher_forget_joining(&self) {
        if self.launcher.lock().unwrap().kept.take().is_some() {
            self.joining_goes(None);
        }
    }

    /// A press again for the place kept, whose VRChat is up (its profile's) or coming up: it waits for that one's proof; where it
    /// cannot be told whose that one is, the account's own invite to itself goes, which is nobody else's.
    fn launcher_join_running(&self, machine: &dyn Machine, errand: &Errand) -> Option<Notice> {
        let view = self.view();
        let roster = machine.roster().unwrap_or_else(|| view.vrchat.runs.clone());
        let runs = runs_with_profiles(machine, &roster);
        let on = errand.profile.unwrap_or(0);
        let unproven = matches!(view.launcher.joining.as_ref().map(|j| &j.step), Some(JoinStep::Failed(why)) if why.unproven());
        if unproven && runs.iter().any(|run| run.2.is_none_or(|profile| profile == on)) {
            let notice = errand.destination.as_ref().map(|place| self.launcher_invite_self(place, errand.account.as_deref()))?;
            if !matches!(notice, Notice::Error(_)) {
                self.launcher_forget_joining();
            }
            return Some(notice);
        }
        let run = runs.into_iter().find(|run| run.2 == Some(on))?;
        self.launcher_join_after(machine, errand.clone(), Vec::new());
        {
            let mut rt = self.launcher.lock().unwrap();
            if let Some(pending) = rt.pending.as_mut() {
                // That very one, whoever started it: its own log says whose it is.
                pending.launch = None;
                pending.seen = Some((run.0, Instant::now()));
            }
        }
        self.joining_goes(Some(JoinStep::SigningIn));
        Some(Notice::Sent { to: errand.destination.as_ref().map(|p| p.world_id.clone()).unwrap_or_default(), action: SentKind::Join })
    }

    /// The place of `errand`, to the VRChat that runs: as an invite to oneself on a Mac, by its link elsewhere (or such an invite
    /// where the link cannot be given): to the process `to` alone, where one is named.
    fn launcher_deliver(&self, errand: &Errand, to: Option<u32>) -> Notice {
        let Some(place) = &errand.destination else { return no_longer_running() };
        let machine = self.launcher.lock().unwrap().machine.clone();
        if machine.by_hand() {
            return self.launcher_invite_self(place, errand.account.as_deref());
        }
        let travel: Arc<dyn super::travel::Travel> = self.travel.lock().unwrap().clone().unwrap_or_else(|| Arc::new(super::travel::Real));
        let bridge = self.launcher_bridges(machine.as_ref());
        let processes = travel.running();
        let chosen = super::travel::route(false, &processes, &bridge, |p| travel.whose(p));
        if chosen == super::travel::Route::Start {
            return no_longer_running();
        }
        let went = self.launcher_hand(machine.as_ref(), travel.as_ref(), chosen, &processes, &bridge, errand, to);
        self.launcher_handed(errand, went)
    }

    /// The VRChats the bridge runs, and whether this computer uses several profiles, for [`super::travel`].
    pub(super) fn launcher_bridges(&self, machine: &dyn Machine) -> super::travel::Bridge {
        super::travel::Bridge { main: self.sides.main_pid(), sides: self.sides.side_pids(), several: self.launcher_several(machine, None) }
    }

    /// The place of `errand` along the route `chosen`: how it went. Whoever takes the link is asked whose it is as it does, and
    /// whether it is the one named (`to`): never a private link of one account's to another's game.
    #[allow(clippy::too_many_arguments)]
    fn launcher_hand(&self, machine: &dyn Machine, travel: &dyn super::travel::Travel, chosen: super::travel::Route, processes: &[u32], bridge: &super::travel::Bridge, errand: &Errand, to: Option<u32>) -> Result<super::travel::Went, HandFailure> {
        let Some(place) = errand.destination.as_ref() else { return Err(HandFailure::Said(no_longer_running())) };
        let account = errand.account.as_deref();
        if errand.scoped() && !self.launcher_live(account) {
            return Err(HandFailure::Said(changed_account()));
        }
        let may = |pid: Option<u32>| (to.is_none() || pid == to) && self.launcher_may_hand(machine, pid, &place.instance_id, errand.bound, account);
        super::travel::go_on(travel, chosen, processes, bridge, &place.world_id, &place.instance_id, &may).map_err(HandFailure::Not)
    }

    /// What is said of how it went; the account's own invite to itself is sent where that is the way (no start is held by then: it
    /// waits for VRChat).
    fn launcher_handed(&self, errand: &Errand, went: Result<super::travel::Went, HandFailure>) -> Notice {
        use super::travel::{GoFailure, Went};
        let Some(place) = errand.destination.clone() else { return no_longer_running() };
        let account = errand.account.as_deref();
        match went.map_err(|failure| match failure {
            HandFailure::Said(notice) => Err(notice),
            HandFailure::Not(failure) => Ok(failure),
        }) {
            Ok(Went::Direct | Went::Started | Went::Invited(_)) => Notice::Sent { to: place.world_id, action: SentKind::Join },
            Err(Err(notice)) => notice,
            Err(Ok(failure)) => match failure {
                GoFailure::Invite(why) => match (self.launcher_sender(account), account) {
                    (Ok(session), Some(account)) => match send_as(&session.client, account, |c| c.invite_self(&place.world_id, &place.instance_id)) {
                        Ok(()) => Notice::Sent { to: place.world_id, action: SentKind::Join },
                        Err(notice) => notice,
                    },
                    _ if self.launcher_client().is_some() => changed_account(),
                    // What its log's trouble is, where that is why.
                    _ if why == super::travel::Detour::Unidentified => match self.launcher_machine_now().log_problem() {
                        Some(trouble) => Notice::Error(crate::pages::launcher::destinations::log_words(&trouble)),
                        None => Notice::Error(tr("Sign in to VRChat in Settings so an invite to this instance can be sent to you.").into()),
                    },
                    _ => Notice::Error(tr("VRChat did not take the link. Sign in to VRChat in Settings so an invite can be sent instead.").into()),
                },
                GoFailure::BridgeBusy => Notice::Error(tr("Only the assistant's VRChat is running. Close it first.").into()),
                GoFailure::Start(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
                GoFailure::BadDestination => Notice::Error(tr("VRChat does not know this world.").into()),
            },
        }
    }

    /// An outcome nobody waits for, shown as an action's is.
    fn launcher_say(self: &Shared, notice: Notice) {
        let asked = self.vrchat_now();
        self.launcher_notice(asked.as_ref(), notice.clone());
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("launcher-said".into()).spawn(move || {
            std::thread::sleep(NOTICE_FOR);
            ctl.launcher_notice_over(asked.as_ref(), &notice);
        });
    }

    /// Whether this computer uses several VRChat profiles: one other than the default one is on Home's choice, or runs or starts now
    /// (Home's or the bridge's), the one a start is about to be made on (`starting`) among them. Nothing else says so.
    pub(super) fn launcher_several(&self, machine: &dyn Machine, starting: Option<u32>) -> bool {
        let view = self.view();
        let roster = machine.roster().unwrap_or_else(|| view.vrchat.runs.clone());
        let runs = runs_with_profiles(machine, &roster);
        let mut now: Vec<u32> = runs.iter().filter_map(|run| run.2).chain(roster.iter().filter(|r| r.profile_known).map(|r| r.profile)).chain(starting).collect();
        {
            let rt = self.launcher.lock().unwrap();
            now.extend(rt.starting.iter().filter(|claim| claim.holds()).map(|claim| claim.profile));
            now.extend(rt.pending.iter().filter_map(|pending| pending.errand.profile));
        }
        now.extend(self.sides.marks().into_iter().map(|mark| mark.profile));
        now.extend(machine.newest_profile().filter(|_| machine.starting()));
        now.extend(view.launcher.profiles.iter().filter(|p| p.starting.is_some()).map(|p| p.profile));
        orbiters_core::vrchat_launch::several(view.settings.launch.profiles.keys().copied(), now)
    }

    /// Takes `profile`, which does not run, off Home's choice: what is remembered of it goes (VRChat's own sign-in on it is
    /// VRChat's).
    fn launcher_forget_profile(self: &Shared, profile: u32) -> Option<Notice> {
        let machine = self.launcher.lock().unwrap().machine.clone();
        let shown = self.view().launcher.profiles.iter().any(|p| p.profile == profile && (p.running.is_some() || p.starting.is_some()));
        let roster = machine.roster().unwrap_or_else(|| self.view().vrchat.runs.clone());
        if shown || self.launcher_claimed(profile) || runs_with_profiles(machine.as_ref(), &roster).iter().any(|run| run.2 == Some(profile)) {
            return Some(Notice::Error(tr("VRChat is already running.").into()));
        }
        self.update_settings(|s| drop(s.launch.profiles.remove(&profile)));
        // Home's choice as it is now.
        self.launcher_game(machine.as_ref());
        None
    }

    /// How the destination of `errand` goes to a VRChat about to be started on `profile`: the one rule.
    fn launcher_start_way(&self, machine: &dyn Machine, errand: &Errand, profile: u32) -> Way {
        orbiters_core::vrchat_launch::way(errand.private(), errand.owner(), To::Start { several: self.launcher_several(machine, Some(profile)) })
    }

    /// The account of the VRChat that runs as process `pid`: what its own log says at this moment, its command line and its log
    /// agreeing on its profile.
    fn launcher_account_of(&self, machine: &dyn Machine, pid: u32) -> Option<String> {
        let roster = machine.roster_now().unwrap_or_else(|| self.view().vrchat.runs.clone());
        let said = roster.iter().find(|r| r.pid == pid && r.profile_known)?;
        let (_, _, profile) = machine.running_profiles().into_iter().find(|run| run.0 == pid)?;
        said.account_id.clone().filter(|_| profile.is_none_or(|profile| profile == said.profile))
    }

    /// Whether the VRChat of process `pid` (`None`: one started on the default profile) may be handed the link of `instance`:
    /// the one rule, for a destination of the account `owner` (`bound`: whatever its instance is).
    pub(super) fn launcher_may_hand(&self, machine: &dyn Machine, pid: Option<u32>, instance: &str, bound: bool, owner: Option<&str>) -> bool {
        use orbiters_core::vrchat_launch::{is_public_instance, way};
        let private = bound || !is_public_instance(instance);
        let several = private && self.launcher_several(machine, pid.is_none().then_some(0));
        let game = pid.filter(|_| several).and_then(|pid| self.launcher_account_of(machine, pid));
        let to = match pid {
            Some(_) => To::Running { several, proven: game.as_deref() },
            None => To::Start { several },
        };
        way(private, owner, to) == Way::Direct
    }

    /// Whether `place` is an instance made here for a VRChat account: that one's alone, whatever its kind. `Err`: made for
    /// another than the one `account` pressed it as, where there are several profiles.
    /// (`starting`: the profile chosen for the start.)
    fn launcher_made(&self, place: &Place, account: Option<&str>, starting: Option<u32>) -> Result<bool, Notice> {
        match self.made_for(&place.world_id, &place.instance_id) {
            None => Ok(false),
            Some(owner) if account == Some(owner.as_str()) => Ok(true),
            Some(_) if self.launcher_several(self.launcher_machine_now().as_ref(), starting) => Err(made_for_another()),
            Some(_) => Ok(false),
        }
    }

    fn launcher_invite_self(&self, place: &Place, account: Option<&str>) -> Notice {
        // Only for the account the press was made on, while its list is on show (AN-2).
        match (self.launcher_sender(account), account) {
            (Ok(session), Some(account)) => match send_as(&session.client, account, |c| c.invite_self(&place.world_id, &place.instance_id)) {
                Ok(()) => Notice::Sent { to: place.world_id.clone(), action: SentKind::Join },
                Err(notice) => notice,
            },
            _ if self.launcher_client().is_some() => changed_account(),
            _ => Notice::Error(tr("Sign in to VRChat in Settings so an invite to this instance can be sent to you.").into()),
        }
    }

    fn launcher_read_local(&self) {
        let Ok(mut state) = orbiters_core::launcher_local::read_now() else {
            // Caught while VRCX / VRCNext write their copy: the list on show stays if it is a local copy; again in a few
            // seconds. A list read with a VRChat sign-in that is over goes at once (its private details with it).
            {
                let mut rt = self.launcher.lock().unwrap();
                rt.loaded_for = Some(None);
                rt.retry_at = Some(Instant::now() + LOCAL_RETRY);
            }
            if self.blank_launcher(|l| matches!(l.source, Source::SignedIn { .. })) {
                self.forget_launcher(|l| l.loading = false);
            }
            return;
        };
        let groups: Vec<(String, String)> = state.groups.iter().filter(|g| g.kind == orbiters_core::launcher::GroupKind::Favourite).map(|g| (g.id.clone(), g.name.clone())).collect();
        {
            let mut rt = self.launcher.lock().unwrap();
            rt.loaded_for = Some(None);
            rt.loaded_at = Some(Instant::now());
            rt.retry_at = None;
            rt.favourite_groups = groups;
        }
        self.publish_local_list(|shown| {
            // The account's own profile is read on its own (`launcher_mine`): kept for the same account only.
            if shown.source == state.source {
                state.mine = shown.mine.clone();
            }
            // An open profile stays only when the source is the same copy (never from a VRChat sign-in into a local one).
            let same_source = matches!((&shown.source, &state.source), (Source::Database { app: a, .. }, Source::Database { app: b, .. }) if a == b);
            state.profile = shown.profile.clone().filter(|p| same_source && state.friends.iter().any(|f| f.id == p.friend.id));
            state.notice = shown.notice.clone();
            *shown = state;
        });
    }

    fn launcher_read_signed_in(self: &Shared, client: &Arc<Mutex<Client>>, by: &By) {
        let read = read_signed_in(client);
        // The sign-in (or the Orbiters member) changed while the pages were read: this read belongs to no one shown; the
        // next look starts again.
        if !self.launcher_client().is_some_and(|s| s.by.user == by.user && s.by.stamp.same(&by.stamp)) {
            return;
        }
        let account = Some(by.user.clone());
        match read {
            Ok((mut state, groups, worn)) => {
                let pipeline = {
                    let mut rt = self.launcher.lock().unwrap();
                    rt.loaded_for = Some(account);
                    rt.loaded_by = Some(by.stamp.clone());
                    rt.loaded_at = Some(Instant::now());
                    rt.failures = 0;
                    rt.retry_at = None;
                    rt.favourite_groups = groups;
                    rt.worn = worn;
                    rt.cooldowns.mark(&mut state.friends, now());
                    // Accepted here and on the way: VRChat still lists them until the account arrives.
                    state.invites.retain(|i| !rt.accepted.iter().any(|a| a.invite.id == i.id));
                    let dismissed = rt.dismissed.get_or_insert_with(load_dismissed);
                    state.boops.retain(|b| !dismissed.contains_key(&b.id));
                    for f in &mut state.friends {
                        if let Some(d) = rt.worn.get(&f.id) {
                            apply_pictures(&rt.pictures, d, &mut f.look);
                        }
                    }
                    rt.pipeline.is_none()
                };
                self.launcher_commit_read(by, state);
                if pipeline {
                    self.launcher_pipeline(client.clone(), by.clone());
                }
            }
            Err(e) => {
                {
                    // Tried for this account: the next try after a growing wait, never every two seconds.
                    let mut rt = self.launcher.lock().unwrap();
                    rt.loaded_for = Some(account);
                    rt.loaded_by = Some(by.stamp.clone());
                    rt.failures += 1;
                    rt.retry_at = Some(Instant::now() + retry_wait(rt.failures, &e, random()));
                }
                let text = error_text(&e);
                // Said once per run of failures (an expired session, VRChat unreachable), not at every try again.
                if self.launcher.lock().unwrap().failures == 1 && self.vrchat_now().is_some_and(|n| n.same(&by.stamp)) {
                    self.notify(super::NoticeKind::Error, text.clone());
                }
                if self.vrchat_now().is_some_and(|n| n.same(&by.stamp)) {
                    self.forget_launcher(|l| l.loading = false);
                }
                self.publish_notice(Some(&by.stamp), |_| Some(Some(Notice::Error(text))));
            }
        }
    }

    /// A full read's list into the state: published only when it differs from what is on show.
    pub(super) fn launcher_commit_read(self: &Shared, by: &By, mut state: LauncherState) {
        #[cfg(test)]
        self.test_point("commit");
        self.publish_launcher(by, |shown| {
            // The own profile, card lists and all, stays for the same account (it is read on its own, `launcher_mine`): a
            // scheduled read never erases them.
            let same_account = shown.source == state.source;
            if same_account {
                state.mine = shown.mine.clone();
            }
            // An open profile stays only when the list on show was this same account's, and the person is in it.
            state.profile = shown.profile.clone().filter(|p| same_account && (state.friends.iter().any(|f| f.id == p.friend.id) || state.me.as_ref().is_some_and(|m| m.id == p.friend.id)));
            state.notice = shown.notice.clone();
            state.loading = false;
            *shown = state;
        });
    }

    /// The one pipeline connection, until the account changes (or the session ends).
    fn launcher_pipeline(self: &Shared, client: Arc<Mutex<Client>>, by: By) {
        let stop = Arc::new(AtomicBool::new(false));
        let resync = {
            let mut rt = self.launcher.lock().unwrap();
            rt.pipeline = Some(stop.clone());
            rt.pipeline_client = Some(client.clone());
            rt.pipeline_by = Some(by);
            rt.resync.clone()
        };
        // Tests never open a websocket to VRChat.
        if cfg!(test) {
            return;
        }
        let weak = Arc::downgrade(self);
        let mine = stop.clone();
        let spawned = std::thread::Builder::new().name("vrchat-pipeline".into()).spawn(move || {
            let mut first = true;
            let ended = orbiters_core::pipeline::run(
                &stop,
                || pipeline_token(&client),
                |message| {
                    // A message of an account (or a sign-in) that is no longer the one shown is not applied.
                    if let Some(ctl) = weak.upgrade() {
                        ctl.launcher_message_of(&stop, message);
                    }
                },
                |up| {
                    if up && !std::mem::take(&mut first) {
                        resync.store(true, Ordering::Release);
                    }
                },
            );
            if ended == orbiters_core::pipeline::Ended::Refused {
                if let Some(ctl) = weak.upgrade() {
                    let mut rt = ctl.launcher.lock().unwrap();
                    if rt.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, &stop)) {
                        rt.pipeline = None;
                        rt.pipeline_client = None;
                    }
                }
            }
        });
        if spawned.is_err() {
            let mut rt = self.launcher.lock().unwrap();
            if rt.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, &mine)) {
                rt.pipeline = None;
                rt.pipeline_client = None;
            }
        }
    }

    /// One message of the pipeline `from`: applied only while it is still the launcher's pipeline (an old connection
    /// of an account, or of a sign-in, that was replaced never writes into the list on show).
    fn launcher_message_of(&self, from: &Arc<AtomicBool>, message: &str) {
        self.launcher_apply(Some(from), message);
    }

    /// One pipeline message applied to the list.
    #[cfg(test)]
    pub(super) fn launcher_message(&self, message: &str) {
        self.launcher_apply(None, message);
    }

    /// Applies `message`; with `from`, only while that is still the launcher's pipeline. The check and the write happen
    /// under the launcher's lock held throughout, so a pipeline replaced meanwhile (another account, a new sign-in)
    /// cannot write into the new list between the two.
    fn launcher_apply(&self, from: Option<&Arc<AtomicBool>>, message: &str) {
        let mut rt = self.launcher.lock().unwrap();
        if let Some(from) = from {
            if !rt.pipeline.as_ref().is_some_and(|p| Arc::ptr_eq(p, from)) || from.load(Ordering::Acquire) {
                return;
            }
        }
        // Whose connection it came on.
        let by = match (from, &rt.pipeline_by) {
            (Some(_), Some(by)) => by.clone(),
            // Tests hand a message in for the session there is.
            (None, _) => match self.vrchat_session() {
                Some(session) => session.by,
                None => return,
            },
            _ => return,
        };
        let dismissed: Vec<String> = rt.dismissed.get_or_insert_with(load_dismissed).keys().cloned().collect();
        // A clear or hide from VRChat (another device) also ends an accepted invite waiting here: its card never comes
        // back. Seen by applying the message to the accepted ones alone.
        if !rt.accepted.is_empty() {
            let mut probe = LauncherState { invites: rt.accepted.iter().map(|a| a.invite.clone()).collect(), ..LauncherState::default() };
            launcher_feed::apply(&mut probe, &[], message, now());
            let left: Vec<String> = probe.invites.iter().map(|i| i.id.clone()).collect();
            rt.accepted.retain(|a| left.contains(&a.invite.id));
        }
        let accepted: Vec<String> = rt.accepted.iter().map(|a| a.invite.id.clone()).chain(rt.hiding.iter().cloned()).collect();
        let groups = rt.favourite_groups.clone();
        let mut applied = None;
        self.publish_launcher(&by, |state| {
            let a = launcher_feed::apply(state, &groups, message, now());
            state.boops.retain(|b| !dismissed.contains(&b.id));
            // An invite accepted here and on the way does not come back with a repeated notification.
            state.invites.retain(|i| !accepted.contains(&i.id));
            applied = Some(a);
        });
        for (id, d) in applied.unwrap_or_default().decorations {
            rt.worn.insert(id, d);
        }
    }

    /// Decorations' pictures, a few per look (each one a call, paced), then written into the friends that wear them.
    fn launcher_resolve_pictures(&self, client: &Arc<Mutex<Client>>, by: &By) {
        let wanted: Vec<String> = {
            let rt = self.launcher.lock().unwrap();
            let mut ids: Vec<String> = rt
                .worn
                .values()
                .flat_map(|d| [d.icon_frame.clone(), d.nameplate.clone()])
                .flatten()
                .filter(|id| !rt.pictures.contains_key(id) && rt.lookup_failed.get(id).is_none_or(|t| t.elapsed() > LOOKUP_AGAIN))
                .collect();
            ids.sort();
            ids.dedup();
            ids.truncate(3);
            ids
        };
        if wanted.is_empty() {
            return;
        }
        let mut found = Vec::new();
        let mut failed = Vec::new();
        for id in wanted {
            match client.lock().unwrap().inventory_template_picture(&id) {
                Ok(p) => found.push((id, Some(p))),
                Err(Error::NotFound | Error::InvalidResponse) => found.push((id, None)),
                Err(Error::RateLimited(_)) => break,
                Err(_) => failed.push(id),
            }
        }
        let (pictures, worn) = {
            let mut rt = self.launcher.lock().unwrap();
            rt.pictures.extend(found);
            for id in failed {
                rt.lookup_failed.insert(id, Instant::now());
            }
            (rt.pictures.clone(), rt.worn.clone())
        };
        self.publish_launcher(by, |state| {
            for f in &mut state.friends {
                if let Some(d) = worn.get(&f.id) {
                    apply_pictures(&pictures, d, &mut f.look);
                }
            }
        });
    }

    /// What Home sends. Runs on its own thread; the outcome shows in `launcher.notice` for a few seconds, and an action
    /// that could not be done is said in the app's toast too.
    pub fn launcher_action(self: &Shared, action: Action) {
        let ctl = self.clone();
        // Its outcome is said to the member who asked for it.
        let asked = self.vrchat_now();
        let _ = std::thread::Builder::new().name("launcher-action".into()).spawn(move || {
            let Some(notice) = ctl.launcher_do(action) else { return };
            ctl.launcher_notice(asked.as_ref(), notice.clone());
            std::thread::sleep(NOTICE_FOR);
            ctl.launcher_notice_over(asked.as_ref(), &notice);
        });
    }

    /// An action's outcome, for the member who `asked` for it: into the state, and when it failed, into the toast as well.
    pub(super) fn launcher_notice(&self, asked: Option<&Stamp>, notice: Notice) {
        let still = match (asked, self.vrchat_now()) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(&b),
            _ => false,
        };
        if let (true, Notice::Error(text) | Notice::Failed { text, .. }) = (still, &notice) {
            self.notify(super::NoticeKind::Error, text.clone());
        }
        self.publish_notice(asked, |_| Some(Some(notice)));
    }

    /// The outcome's time is over: gone from the state, unless another action's replaced it.
    pub(super) fn launcher_notice_over(&self, asked: Option<&Stamp>, notice: &Notice) {
        self.publish_notice(asked, |shown| (shown == Some(notice)).then_some(None));
    }

    pub(super) fn launcher_do(self: &Shared, action: Action) -> Option<Notice> {
        // The account the press was made on (the page names it); none, or one bound twice: nothing is sent for it.
        let (account, action) = match action {
            Action::As { account, action } if !matches!(*action, Action::As { .. }) => (Some(account), *action),
            Action::As { .. } => return Some(changed_account()),
            action => (None, action),
        };
        let account = account.as_deref();
        match action {
            Action::Play { mode, destination } => {
                let bound = match destination.as_ref().map(|place| self.launcher_made(place, account, None)).transpose() {
                    Ok(bound) => bound.unwrap_or(false),
                    Err(refused) => return Some(refused),
                };
                Some(self.launcher_run(Errand { bound, ..Errand::plain(mode, None).to(destination, account) }))
            }
            Action::PlayOn { mode, profile } => Some(self.launcher_run(Errand::plain(mode, Some(profile)))),
            // Home's Where (REQUEST20 2): the chosen profile, right into the chosen instance.
            Action::PlayAt { mode, profile, destination } => {
                let bound = match self.launcher_made(&destination, account, Some(profile)) {
                    Ok(bound) => bound,
                    Err(refused) => return Some(refused),
                };
                Some(self.launcher_run(Errand { bound, ..Errand::plain(mode, Some(profile)).to(Some(destination), account) }))
            }
            Action::JoinRunning { mode, destination } => {
                let bound = match self.launcher_made(&destination, account, None) {
                    Ok(bound) => bound,
                    Err(refused) => return Some(refused),
                };
                Some(self.launcher_run(Errand { bound, running_only: true, ..Errand::plain(mode, None).to(Some(destination), account) }))
            }
            // The account's own profile: REQUEST15 7b.
            Action::EditProfile(edit) => self.launcher_edit_mine(edit, account),
            Action::ListPictures(kind) => {
                self.launcher_list_pictures(kind, account);
                None
            }
            Action::ReadMine => {
                if let Ok(session) = self.launcher_sender(account) {
                    self.launcher_read_mine(&session);
                }
                None
            }
            Action::AcceptInvite { invite, mode } => Some(self.launcher_accept(&invite, Errand::plain(mode, None), account)),
            Action::AcceptInviteOn { invite, mode, profile } => Some(self.launcher_accept(&invite, Errand::plain(mode, Some(profile)), account)),
            Action::AcceptInviteRunning { invite, mode } => Some(self.launcher_accept(&invite, Errand { running_only: true, ..Errand::plain(mode, None) }, account)),
            Action::Dismiss { id } => {
                self.launcher_dismiss(&id, account);
                None
            }
            Action::RequestInvite { to } => Some(self.launcher_send(false, &to, None, account)),
            Action::Boop { to, emoji } => Some(self.launcher_send(true, &to, emoji.as_deref(), account)),
            Action::As { .. } => Some(changed_account()),
            Action::OpenProfile { id } => {
                self.launcher_profile(&id, account);
                None
            }
            Action::JoinAgain => {
                let kept = self.launcher.lock().unwrap().kept.clone()?;
                let errand = Errand { again: true, running_only: false, ..kept };
                // Its card is back: accepted as from there.
                let back = errand.invite.clone().filter(|id| self.view().launcher.invites.iter().any(|i| i.id == *id));
                Some(match (back, errand.account.clone()) {
                    (Some(invite), Some(account)) => self.launcher_accept(&invite, errand, Some(&account)),
                    _ => self.launcher_run(errand),
                })
            }
            Action::ForgetJoining => {
                {
                    let mut rt = self.launcher.lock().unwrap();
                    rt.kept = None;
                    rt.pending = rt.pending.take().filter(|pending| !pending.proof);
                }
                self.joining_goes(None);
                None
            }
            Action::ForgetProfile { profile } => (profile != 0).then(|| self.launcher_forget_profile(profile)).flatten(),
            Action::OpenEngine => {
                let machine = self.launcher.lock().unwrap().machine.clone();
                // Opened to download VRChat in it: the game is played once it is there.
                if machine.by_hand() && !machine.engine_missing() && !machine.installed() {
                    let mut rt = self.launcher.lock().unwrap();
                    rt.play_when_ready = Some(Instant::now());
                    rt.queued = None;
                }
                machine.open_engine().err().map(Notice::Error)
            }
            Action::InstallEngine { profile } => match orbiters_core::vrchat_launch::main_profile(u64::from(profile)) {
                Some(profile) => self.launcher_install_engine(profile),
                None => Some(Notice::Error(tr("VRChat does not know this profile.").into())),
            },
            Action::RestartLostGame { session } => {
                let machine = self.launcher.lock().unwrap().machine.clone();
                // The session the press was made on, never whichever lost its picture since.
                if machine.picture_lost().as_deref() != Some(session.as_str()) {
                    return Some(Notice::Error(tr("VRChat draws again, or is no longer the one Home started: nothing was done.").into()));
                }
                let ctl = self.clone();
                let asked = self.vrchat_now();
                let _ = std::thread::Builder::new().name("launcher-restart-lost".into()).spawn(move || {
                    // Its account is the one signed in, its destination still one that goes with a start (one that does not is
                    // delivered after it).
                    let after = Mutex::new(None);
                    let may = |launch: &HomeLaunch| {
                        if launch.account.as_deref().is_some_and(|account| !ctl.launcher_live(Some(account))) {
                            return Err(tr("The account VRChat was started with is no longer the one signed in: press Play to start it on this one.").to_string());
                        }
                        *after.lock().unwrap() = None;
                        let with_it = orbiters_core::vrchat_launch::destination_of(&launch.args).and_then(|(world, instance)| launcher_feed::place(&format!("{world}:{instance}"), ""));
                        let Some(place) = with_it.or_else(|| launch.after.clone()) else { return Ok(launch.args.clone()) };
                        let profile = orbiters_core::vrchat_launch::profile_of(&launch.args);
                        let errand = Errand { bound: launch.bound, asked: asked.clone(), ..Errand::plain(LaunchMode::Desktop, Some(profile)).to(Some(place), launch.account.as_deref()) };
                        match ctl.launcher_start_way(machine.as_ref(), &errand, profile) {
                            Way::Direct => Ok(launch.args.clone()),
                            Way::AfterProof => {
                                let without = launch.args.iter().filter(|arg| !arg.starts_with("vrchat://")).cloned().collect();
                                *after.lock().unwrap() = Some(errand);
                                Ok(without)
                            }
                            _ => Err(tr("This place is private: sign in to VRChat in Settings to go there.").to_string()),
                        }
                    };
                    match machine.restart_lost(&session, &may) {
                        Ok(()) => {
                            if let Some(errand) = after.lock().unwrap().take() {
                                ctl.launcher_join_after(machine.as_ref(), errand, Vec::new());
                            }
                            *ctl.launcher_started.lock().unwrap() = Some(Instant::now());
                            let profile = machine.newest_profile().unwrap_or(0);
                            ctl.publish_machine(|m| starting_on(m, profile, LaunchMode::Desktop));
                            ctl.notify(super::NoticeKind::Success, tr("VRChat is starting again.").to_string());
                        }
                        Err(why) => ctl.launcher_notice(asked.as_ref(), Notice::Error(why)),
                    }
                    ctl.launcher_game(machine.as_ref());
                });
                None
            }
            Action::RepairEngine => {
                let machine = self.launcher.lock().unwrap().machine.clone();
                {
                    let mut rt = self.launcher.lock().unwrap();
                    if rt.repairing.is_some() || machine.engine_problem().is_none() {
                        return None;
                    }
                    rt.repairing = Some(None);
                }
                self.launcher_game(machine.as_ref());
                let ctl = self.clone();
                let asked = self.vrchat_now();
                let _ = std::thread::Builder::new().name("launcher-repair-engine".into()).spawn(move || {
                    let step = |percent: Option<u8>| {
                        ctl.launcher.lock().unwrap().repairing = Some(percent);
                        ctl.publish_machine(|m| {
                            if let Some(t) = &mut m.engine_trouble {
                                t.percent = percent;
                            }
                        });
                    };
                    let done = machine.repair_engine(&step, &ctl.quitting);
                    ctl.launcher.lock().unwrap().repairing = None;
                    ctl.launcher_game(machine.as_ref());
                    match done {
                        Ok(()) => ctl.notify(super::NoticeKind::Success, tr("What runs VRChat was repaired: press Play.").to_string()),
                        Err(why) => ctl.launcher_notice(asked.as_ref(), Notice::Error(why)),
                    }
                });
                None
            }
            Action::SetAsideRecord { file } => {
                let machine = self.launcher.lock().unwrap().machine.clone();
                match machine.set_aside(&file) {
                    Ok(aside) => {
                        self.notify(super::NoticeKind::Success, tr!("The damaged record was set aside: {path}", path = aside));
                        // Home shows the records as they are now.
                        self.launcher_game(machine.as_ref());
                        None
                    }
                    Err(why) => Some(Notice::Error(why)),
                }
            }
            Action::CloseProfile => {
                self.forget_launcher(|l| l.profile = None);
                None
            }
            Action::Refresh => {
                let mut rt = self.launcher.lock().unwrap();
                rt.loaded_at = None;
                rt.retry_at = None;
                None
            }
        }
    }

    /// Accepts the invite `invite` of the list of `account` and plays it as `errand` says (its mode, its profile). One accept per
    /// invite, reserved before anything starts; the card goes at once and VRChat hides the invite once the account got there
    /// ([`Self::launcher_settle_accepted`]).
    fn launcher_accept(self: &Shared, invite: &str, errand: Errand, account: Option<&str>) -> Notice {
        // An invite is the account's whose list showed it: never accepted for another, nor with no sign-in.
        let owner = match (self.launcher_sender(account), account) {
            (Ok(_), Some(owner)) => owner.to_string(),
            (sender, _) => return notice_or_changed(sender.err()),
        };
        let found = self.view().launcher.invites.iter().find(|i| i.id == invite).cloned();
        let Some(found) = found.filter(|i| i.place.is_some()) else { return Notice::Error(tr("This invite is no longer here.").into()) };
        // One accept per invite: reserved before anything starts (two presses, two threads).
        {
            let from = match &self.view().launcher.me {
                Some(me) => match &me.whereabouts {
                    Whereabouts::World(p) => Some((p.world_id.clone(), p.instance_id.clone())),
                    _ => None,
                },
                None => None,
            };
            let mut rt = self.launcher.lock().unwrap();
            if rt.accepted.iter().any(|a| a.invite.id == invite) {
                return Notice::Error(tr("This invite is no longer here.").into());
            }
            rt.accepted.push(Accepted { invite: found.clone(), at: Instant::now(), owner: owner.clone(), from, hide_failures: 0, hide_again: None, moved: false });
        }
        let notice = self.launcher_run(Errand { bound: true, invite: Some(invite.to_string()), ..errand.to(found.place.clone(), Some(&owner)) });
        if matches!(notice, Notice::Error(_)) {
            // Not on the way: the reservation goes, the card stays.
            self.launcher.lock().unwrap().accepted.retain(|a| a.invite.id != invite);
        } else {
            // The card goes here at once; VRChat hides the invite only once the account got there (a start that
            // fails, or lands at home, leaves it in VRChat and brings the card back).
            self.forget_launcher(|l| l.invites.retain(|i| i.id != invite));
        }
        notice
    }

    /// One press, done: on the profile it names, or as Play does.
    fn launcher_run(self: &Shared, mut errand: Errand) -> Notice {
        if (errand.private() || errand.scoped()) && errand.asked.is_none() {
            errand.asked = self.vrchat_now();
        }
        if errand.again {
            let machine = self.launcher.lock().unwrap().machine.clone();
            if errand.scoped() && !self.launcher_live(errand.account.as_deref()) {
                return changed_account();
            }
            if let Some(notice) = self.launcher_join_running(machine.as_ref(), &errand) {
                return notice;
            }
        }
        let waiting = self.launcher.lock().unwrap().pending.as_ref().map(|pending| pending.since);
        let elsewhere = errand.destination.is_some();
        let notice = if errand.profile.is_some() { self.launcher_play_on(errand) } else { self.launcher_play(errand) };
        if !matches!(notice, Notice::Error(_)) {
            // Another start or place by now: the one kept is over, and so is the one that waited.
            self.launcher_forget_joining();
            let mut over = false;
            if elsewhere {
                let mut rt = self.launcher.lock().unwrap();
                over = rt.pending.as_ref().is_some_and(|pending| pending.proof && Some(pending.since) == waiting);
                if over {
                    rt.pending = None;
                }
            }
            if over {
                self.joining_goes(None);
            }
        }
        notice
    }

    /// Play: VRChat started, or the running one sent to the destination. With a profile (a Mac's start on a profile chosen,
    /// where one game runs at a time) it only starts: a game that runs, on that profile or another, refuses it.
    fn launcher_play(self: &Shared, errand: Errand) -> Notice {
        let (mode, profile, scoped) = (errand.mode, errand.profile, errand.scoped());
        let (destination, account) = (errand.destination.clone(), errand.account.clone());
        let account = account.as_deref();
        if !self.view().launcher.possible.play {
            return Notice::Error(tr("VRChat runs on another device: Play works on the computer that runs it.").into());
        }
        // Ids come from the network (a friend's location, an invite) or over IPC: only VRChat's own grammar goes on.
        if destination.as_ref().is_some_and(|p| !orbiters_core::vrchat_launch::is_destination(&p.world_id, &p.instance_id)) {
            return Notice::Error(tr("VRChat does not know this world.").into());
        }
        // A destination from a signed-in account's list (a friend's room, an invite) goes only while that account is
        // the one signed in and on show: checked here and again right before the hand-off.
        if scoped && !self.launcher_live(account) {
            return changed_account();
        }
        let machine = self.launcher.lock().unwrap().machine.clone();
        // What a start needs first (a program to download), before anything is held.
        if machine.direct() && machine.installed() && machine.running().is_empty() {
            let _ = machine.make_ready();
        }
        // One start at a time: held from the look at the running VRChats to the start, so that two presses (or a press
        // and an invite's Accept, on their own threads) never start two; and none while one is starting.
        let mut started = self.launcher_started.lock().unwrap();
        let running = machine.running();
        let settings = self.view().settings.launch.clone();
        if let (Some(profile), false) = (profile, running.is_empty()) {
            let roster = machine.roster().unwrap_or_default();
            let same = runs_with_profiles(machine.as_ref(), &roster).iter().any(|r| r.2 == Some(profile));
            return Notice::Error(if same { tr("VRChat is already running.").into() } else { tr(ONE_GAME).into() });
        }
        if !running.is_empty() {
            // Never a second VRChat: the running one goes there.
            let Some(place) = destination else { return Notice::Error(tr("VRChat is already running.").into()) };
            if machine.by_hand() {
                drop(started);
                return self.launcher_invite_self(&place, account);
            }
            let travel: Arc<dyn super::travel::Travel> = self.travel.lock().unwrap().clone().unwrap_or_else(|| Arc::new(super::travel::Real));
            let bridge = self.launcher_bridges(machine.as_ref());
            // Only the bridge's side sessions run: travel starts the person's own VRChat. That start is under the same
            // start lock (two Joins at once start one), and none while one is starting.
            // The route is decided once, from one look at the processes, and that route is the one taken.
            let processes = travel.running();
            let chosen = super::travel::route(false, &processes, &bridge, |p| travel.whose(p));
            if chosen == super::travel::Route::Start && errand.running_only {
                // (The person's is gone, or only the assistant's run.)
                return no_longer_running();
            }
            if chosen == super::travel::Route::Start && self.launcher_start_way(machine.as_ref(), &errand, 0) != Way::Direct {
                // Beside the assistant's, and private: the start on the default profile, as chosen on Home.
                drop(started);
                return self.launcher_play_on(Errand { profile: Some(0), ..errand });
            }
            if chosen == super::travel::Route::Start {
                if started.is_some_and(|t| t.elapsed() < STARTING_FOR) {
                    return Notice::Error(tr("Starting VRChat…").into());
                }
                // The claim's own words: a start that never showed asks for a second press, and that is said here
                // (REQUEST15 third pass: it was turned into "Starting VRChat…" while it armed the second press).
                if let Err(notice) = self.launcher_claim(0, mode) {
                    return notice;
                }
            }
            // To the game that runs: nothing is started, and nothing is held while VRChat answers.
            let held = (chosen == super::travel::Route::Start).then_some(started);
            let errand = Errand { destination: Some(place), ..errand };
            let went = self.launcher_hand(machine.as_ref(), travel.as_ref(), chosen, &processes, &bridge, &errand, None);
            if let Some(mut started) = held {
                if matches!(went, Ok(super::travel::Went::Started)) {
                    *started = Some(Instant::now());
                } else {
                    self.launcher_unclaim_last_if(true);
                }
            }
            return self.launcher_handed(&errand, went);
        }
        if machine.by_hand() {
            return self.launcher_play_by_hand(machine.as_ref(), errand, &mut started);
        }
        if started.is_some_and(|t| t.elapsed() < STARTING_FOR) {
            return Notice::Error(tr("Starting VRChat…").into());
        }
        if errand.running_only {
            return no_longer_running();
        }
        if !machine.installed() {
            return Notice::Error(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into());
        }
        let way = self.launcher_start_way(machine.as_ref(), &errand, 0);
        if !matches!(way, Way::Direct | Way::AfterProof) {
            return nobodys();
        }
        let with_it = destination.as_ref().filter(|_| way == Way::Direct);
        let mut args = orbiters_core::vrchat_launch::arguments(mode, &settings, with_it.map(|p| (p.world_id.as_str(), p.instance_id.as_str())));
        // The window VRChat saved (read while none runs): kept before a VR start shrinks it, given back to Desktop.
        let saved = machine.saved_window();
        let mut keep_window = None;
        match mode {
            LaunchMode::Desktop => {
                let extra = desktop_window(saved, settings.desktop_window);
                let link = args.iter().position(|a| a.starts_with("vrchat://")).unwrap_or(args.len());
                args.splice(link..link, extra);
            }
            LaunchMode::Vr if settings.performance && settings.small_window_in_vr => keep_window = saved.filter(SavedWindow::normal).filter(|w| settings.desktop_window != Some(*w)),
            LaunchMode::Vr => {}
        }
        if scoped && !self.launcher_live(account) {
            return changed_account();
        }
        // Claimed before the start, under the start lock: no other start can take profile 0 meanwhile. A refused claim
        // (another start under way, on any profile) starts nothing (REQUEST15 second pass, blocker 6).
        let claim = match self.launcher_claim(0, mode) {
            Ok(id) => id,
            Err(notice) => return notice,
        };
        match machine.start(&args) {
            Ok(()) => {
                if way == Way::AfterProof {
                    self.launcher_join_after(machine.as_ref(), errand, Vec::new());
                }
                *started = Some(Instant::now());
                drop(started);
                self.publish_machine(|m| m.game = Game::Starting(mode));
                if settings.last_mode != mode || keep_window.is_some() {
                    self.update_settings(|s| {
                        s.launch.last_mode = mode;
                        if keep_window.is_some() {
                            s.launch.desktop_window = keep_window;
                        }
                    });
                }
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => {
                self.launcher_unclaim(claim);
                Notice::Error(tr!("VRChat could not be started: {reason}", reason = words))
            }
        }
    }

    /// Claims `profile` for a start that is about to be made, called under the start lock (`launcher_started`): refused
    /// while another start holds any profile (one start at a time) or a VRChat bridge session holds this one. The claim
    /// keeps every other start of Orbiters off that profile until a VRChat on it is seen (REQUEST15 fix B4).
    /// This computer's machine now (a Mac's shares its start with the VRChat bridge: `bridge_mac.rs`).
    pub(super) fn launcher_machine_now(&self) -> Arc<dyn Machine> {
        self.launcher.lock().unwrap().machine.clone()
    }

    /// The lock Home's starts are made under, for the bridge's start on a Mac.
    pub(super) fn launcher_started_lock(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.launcher_started.lock().unwrap()
    }

    pub(super) fn launcher_claim(&self, profile: u32, mode: LaunchMode) -> Result<u64, Notice> {
        self.launcher_claim_as(profile, mode, true)
    }

    /// [`Self::launcher_claim`] for a start nobody pressed (the VRChat bridge's): a lapsed start on the profile is never
    /// overridden by it.
    pub(super) fn launcher_claim_unattended(&self, profile: u32, mode: LaunchMode) -> Result<u64, Notice> {
        self.launcher_claim_as(profile, mode, false)
    }

    fn launcher_claim_as(&self, profile: u32, mode: LaunchMode, person: bool) -> Result<u64, Notice> {
        if self.sides.marks().iter().any(|m| m.profile == profile) {
            return Err(Notice::Error(tr!("The assistant's VRChat uses profile {n}: choose another profile.", n = profile)));
        }
        let mut rt = self.launcher.lock().unwrap();
        if rt.starting.iter().any(Claim::holds) {
            return Err(Notice::Error(tr("Starting VRChat…").into()));
        }
        // A start on this profile that never showed (past its time): not started again behind its back. The person is
        // told once; a press again within a few seconds says it is not coming, and starts.
        let now = Instant::now();
        // Past its lapse, the bridge's start may take the profile; a person is always asked first.
        if !person {
            rt.starting.retain(|c| c.profile != profile || c.kept());
        }
        if let Some(lapsed) = rt.starting.iter_mut().find(|c| c.profile == profile) {
            match lapsed.confirm_by {
                Some(by) if person && now < by => {
                    let id = lapsed.id;
                    rt.starting.retain(|c| c.id != id);
                }
                _ => {
                    if person {
                        lapsed.confirm_by = Some(now + CONFIRM_WITHIN);
                    }
                    let minutes = (lapsed.at.elapsed().as_secs() / 60).max(1);
                    return Err(Notice::Error(tr!(
                        "VRChat was asked to start on profile {n} {m} min ago and has not shown up yet. If it is not coming, press Play again to start it anyway.",
                        n = profile,
                        m = minutes
                    )));
                }
            }
        }
        rt.claims_made += 1;
        let id = rt.claims_made;
        rt.starting.push(Claim { profile, mode, at: now, id, confirm_by: None });
        Ok(id)
    }

    /// Ends the claims of `observed` (those that existed before the look that read `runs`) whose profile `runs` shows
    /// running. A claim made after that look (a start while an old VRChat of the same profile was quitting) is never
    /// ended by it (REQUEST15 second pass, blocker 6). Returns the claims still holding, for the profile choice.
    pub(super) fn launcher_retire_claims(&self, observed: &[u64], runs: &[(u32, bool, Option<u32>)]) -> Vec<(u32, LaunchMode)> {
        let mut rt = self.launcher.lock().unwrap();
        rt.starting.retain(|c| !(observed.contains(&c.id) && runs.iter().any(|r| r.2 == Some(c.profile))));
        rt.starting.iter().filter(|c| c.holds()).map(|c| (c.profile, c.mode)).collect()
    }

    /// A start that did not happen gives its claim back.
    pub(super) fn launcher_unclaim(&self, id: u64) {
        self.launcher.lock().unwrap().starting.retain(|c| c.id != id);
    }

    /// [`Self::launcher_unclaim`] of the newest claim, when `made`.
    fn launcher_unclaim_last_if(&self, made: bool) {
        if made {
            let mut rt = self.launcher.lock().unwrap();
            let last = rt.claims_made;
            rt.starting.retain(|c| c.id != last);
        }
    }

    /// Whether a start holds `profile`, or one that never showed still bars it (lapsed, not yet confirmed by the person).
    pub(super) fn launcher_profile_barred(&self, profile: u32) -> bool {
        self.launcher.lock().unwrap().starting.iter().any(|c| c.kept() && c.profile == profile)
    }

    /// Whether a start holds `profile` now.
    pub(super) fn launcher_claimed(&self, profile: u32) -> bool {
        self.launcher.lock().unwrap().starting.iter().any(|c| c.holds() && c.profile == profile)
    }

    /// Home's Play with a profile chosen (REQUEST15 7a): a VRChat on `profile`, beside the ones running on other profiles
    /// (VRChat's own launcher with `--profile`), never a second one on the same profile. Through Steam when none runs.
    /// With Home's Where (REQUEST20 2), right into `destination`: its link goes last ([`vrchat_launch::arguments_on`]);
    /// one from the list of `account` (an invite, a friend's room) goes only while that account is signed in and on show.
    ///
    /// [`vrchat_launch::arguments_on`]: orbiters_core::vrchat_launch::arguments_on
    fn launcher_play_on(self: &Shared, errand: Errand) -> Notice {
        let (mode, profile, scoped) = (errand.mode, errand.profile.unwrap_or(0), errand.scoped());
        let (destination, account) = (errand.destination.clone(), errand.account.clone());
        let account = account.as_deref();
        if !self.view().launcher.possible.play {
            return Notice::Error(tr("VRChat runs on another device: Play works on the computer that runs it.").into());
        }
        if orbiters_core::vrchat_launch::main_profile(u64::from(profile)).is_none() {
            return Notice::Error(tr("VRChat does not know this profile.").into());
        }
        // Only VRChat's own grammar goes on, as Play's (a destination comes from the network or over IPC).
        if destination.as_ref().is_some_and(|p| !orbiters_core::vrchat_launch::is_destination(&p.world_id, &p.instance_id)) {
            return Notice::Error(tr("VRChat does not know this world.").into());
        }
        if scoped && !self.launcher_live(account) {
            return changed_account();
        }
        let machine = self.launcher.lock().unwrap().machine.clone();
        if machine.by_hand() {
            // A Mac: one VRChat at a time, on the profile chosen (a game the person starts by hand takes none).
            return if machine.direct() {
                self.launcher_play(errand)
            } else if profile == 0 {
                self.launcher_play(Errand { profile: None, ..errand })
            } else {
                Notice::Error(tr("On a Mac, VRChat runs one profile, through Obelisk.").into())
            };
        }
        // The same start lock as Play: from the look at the running VRChats to the start, two presses start one.
        let mut started = self.launcher_started.lock().unwrap();
        let view = self.view();
        let runs = runs_with_profiles(machine.as_ref(), &view.vrchat.runs);
        if runs.iter().any(|r| r.2 == Some(profile)) {
            return Notice::Error(tr!("VRChat already runs on profile {n}: choose another profile to start one more.", n = profile));
        }
        // A VRChat whose profile is not known yet could be on this one: wait until it says (a few seconds). One that has
        // still not said after a minute (its log never named it) does not hold Play for its whole life: the person is
        // told, and a second press starts anyway.
        let unknown: Vec<u32> = runs.iter().filter(|r| r.2.is_none()).map(|r| r.0).collect();
        if !unknown.is_empty() {
            let now = Instant::now();
            let mut rt = self.launcher.lock().unwrap();
            rt.unknown_since.retain(|pid, _| unknown.contains(pid));
            // Every unknown one must have had its minute: a VRChat starting now is not passed because an older one
            // never said its profile (REQUEST15 fourth pass).
            let newest = unknown.iter().map(|pid| *rt.unknown_since.entry(*pid).or_insert(now)).max().unwrap_or(now);
            if now.duration_since(newest) < UNKNOWN_WAIT {
                return Notice::Error(tr("A VRChat is starting and has not said its profile yet: try again in a moment.").into());
            }
            // The confirmation is for these very VRChats: another one appearing (or one going) asks again.
            let mut set = unknown.clone();
            set.sort_unstable();
            match &rt.unknown_confirm {
                Some((p, by, seen)) if *p == profile && now < *by && *seen == set => rt.unknown_confirm = None,
                _ => {
                    rt.unknown_confirm = Some((profile, now + CONFIRM_WITHIN, set));
                    let minutes = (now.duration_since(newest).as_secs() / 60).max(1);
                    return Notice::Error(tr!(
                        "A VRChat running for {m} min has not said its profile. If it is not on profile {n}, press Play again to start anyway.",
                        m = minutes,
                        n = profile
                    ));
                }
            }
        }
        if runs.is_empty() && started.is_some_and(|t| t.elapsed() < STARTING_FOR) {
            return Notice::Error(tr("Starting VRChat…").into());
        }
        if !machine.installed() {
            return Notice::Error(tr("VRChat was not found on this computer. Install it from Steam, or start it once.").into());
        }
        let settings = view.settings.launch.clone();
        // VRChat's saved window is one for all its profiles: the same care as Play's.
        let saved = machine.saved_window();
        let mut keep_window = None;
        let window = match mode {
            LaunchMode::Desktop => desktop_window(saved, settings.desktop_window),
            LaunchMode::Vr => {
                if runs.is_empty() && settings.performance && settings.small_window_in_vr {
                    keep_window = saved.filter(SavedWindow::normal).filter(|w| settings.desktop_window != Some(*w));
                }
                Vec::new()
            }
        };
        // A private destination goes with the start, or after it, by the one rule.
        let way = self.launcher_start_way(machine.as_ref(), &errand, profile);
        if !matches!(way, Way::Direct | Way::AfterProof) {
            return nobodys();
        }
        let with_it = destination.as_ref().filter(|_| way == Way::Direct);
        let args = orbiters_core::vrchat_launch::arguments_on(mode, &settings, window, profile, with_it.map(|p| (p.world_id.as_str(), p.instance_id.as_str())));
        // Checked again right before the hand-off: the account may have changed while VRChat's runs were looked at.
        if scoped && !self.launcher_live(account) {
            return changed_account();
        }
        // The claim is made before the start and while the start lock is held: a second press, Go there or the bridge
        // sees it from the moment this one decided (REQUEST15 fix B4).
        let claim = match self.launcher_claim(profile, mode) {
            Ok(id) => id,
            Err(notice) => return notice,
        };
        let result = if runs.is_empty() { machine.start(&args) } else { machine.start_beside(&args) };
        if result.is_err() {
            self.launcher_unclaim(claim);
        }
        match result {
            Ok(()) => {
                if way == Way::AfterProof {
                    self.launcher_join_after(machine.as_ref(), errand, runs.iter().map(|run| run.0).collect());
                }
                *started = Some(Instant::now());
                drop(started);
                self.publish_machine(|m| {
                    if runs.is_empty() {
                        m.game = Game::Starting(mode);
                    }
                    match m.profiles.iter_mut().find(|p| p.profile == profile) {
                        Some(p) => p.starting = Some(mode),
                        None => {
                            m.profiles.push(orbiters_core::launcher::GameProfile { profile, starting: Some(mode), ..Default::default() });
                            m.profiles.sort_by_key(|p| p.profile);
                        }
                    }
                });
                if settings.last_mode != mode || keep_window.is_some() {
                    self.update_settings(|s| {
                        s.launch.last_mode = mode;
                        if keep_window.is_some() {
                            s.launch.desktop_window = keep_window;
                        }
                    });
                }
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// Accepted invites: hidden at VRChat once their own account is in their instance (having moved there after the
    /// press); a hide that failed is tried again a minute later. After [`ARRIVE_WITHIN`] without arriving, the card comes
    /// back, in its own account's list only.
    fn launcher_settle_accepted(&self, client: Option<&Arc<Mutex<Client>>>, by: Option<&By>) {
        let view = self.view();
        let shown = match &view.launcher.source {
            Source::SignedIn { user_id, .. } => Some(user_id.clone()),
            _ => None,
        };
        let here = match &view.launcher.me {
            Some(me) => match &me.whereabouts {
                Whereabouts::World(p) => Some((p.world_id.clone(), p.instance_id.clone())),
                _ => None,
            },
            None => None,
        };
        let (hide, back) = {
            let mut rt = self.launcher.lock().unwrap();
            if rt.accepted.is_empty() {
                return;
            }
            let arrived = |a: &Accepted| a.invite.place.as_ref().is_some_and(|p| here.as_ref().is_some_and(|(w, i)| *w == p.world_id && *i == p.instance_id)) && (a.from != here || a.moved);
            let mut hide = Vec::new();
            let mut back = Vec::new();
            for mut a in std::mem::take(&mut rt.accepted) {
                if here.is_some() && a.from != here {
                    a.moved = true;
                }
                if shown.as_deref() != Some(a.owner.as_str()) {
                    // Another account's list is on show: nothing of this one's is hidden or shown with it (an account
                    // change drops them all; this covers a list still being replaced).
                    rt.accepted.push(a);
                } else if a.hide_failures > 0 || arrived(&a) {
                    if a.hide_again.is_none_or(|t| Instant::now() >= t) {
                        hide.push(a);
                    } else {
                        rt.accepted.push(a);
                    }
                } else if a.at.elapsed() > ARRIVE_WITHIN {
                    back.push(a);
                } else {
                    rt.accepted.push(a);
                }
            }
            // While their hide is on the way they still count as accepted (a repeated notification stays out).
            rt.hiding = hide.iter().map(|a: &Accepted| a.invite.id.clone()).collect();
            (hide, back)
        };
        for mut a in hide {
            // With the owner's own session only, under the lock the hide takes.
            let done = client.is_some_and(|client| {
                send_as(client, &a.owner, |c| match c.hide_notification(&a.invite.id) {
                    Err(Error::NotFound) => Ok(()),
                    other => other,
                })
                .is_ok()
            });
            if !done {
                a.hide_failures += 1;
                a.hide_again = Some(Instant::now() + HIDE_AGAIN);
                if a.hide_failures < HIDE_TRIES {
                    self.launcher.lock().unwrap().accepted.push(a);
                } else {
                    eprintln!("launcher: an accepted invite could not be hidden at VRChat after {HIDE_TRIES} tries"); // i18n: not shown (a log line)
                }
            }
        }
        self.launcher.lock().unwrap().hiding.clear();
        if let (false, Some(by)) = (back.is_empty(), by) {
            self.publish_launcher(by, |l| {
                for a in back {
                    // Its own account's list only (checked again: the list may have changed meanwhile).
                    if !matches!(&l.source, Source::SignedIn { user_id, .. } if *user_id == a.owner) {
                        continue;
                    }
                    if !l.invites.iter().any(|i| i.id == a.invite.id) {
                        l.invites.push(a.invite);
                    }
                }
                l.invites.sort_by_key(|i| std::cmp::Reverse(i.received_at));
            });
        }
    }

    /// A Mac where VRChat is not set up: what runs it is downloaded and checked, the person signs in to Steam, VRChat
    /// downloads, each step shown on Home; then it is played on `profile` (the one last asked for), without another press.
    /// One at a time.
    fn launcher_install_engine(self: &Shared, profile: u32) -> Option<Notice> {
        let machine = self.launcher.lock().unwrap().machine.clone();
        if !machine.by_hand() {
            return None;
        }
        {
            // The press last made; one that only goes on with the set-up of the same profile keeps where that one goes.
            let mut rt = self.launcher.lock().unwrap();
            if rt.queued.as_ref().is_none_or(|(queued, _)| queued.profile.unwrap_or(0) != profile) {
                rt.queued = Some((Errand::plain(LaunchMode::Desktop, Some(profile)), Instant::now()));
            }
        }
        if !machine.installed() {
            let mut first = false;
            self.publish_machine(|m| {
                first = matches!(m.engine_setup, None | Some(EngineSetup::Failed(_)));
                if first {
                    m.engine_setup = Some(EngineSetup::Downloading { percent: 0 });
                }
            });
            if !first {
                return None;
            }
            let step = |step: EngineSetup| {
                self.publish_machine(|m| m.engine_setup = Some(step));
            };
            if let Err(words) = machine.install_engine(&step, &self.quitting) {
                self.publish_machine(|m| m.engine_setup = Some(EngineSetup::Failed(words.clone())));
                return Some(Notice::Error(tr!("VRChat could not be set up: {reason}", reason = words)));
            }
        }
        self.launcher.lock().unwrap().play_when_ready = Some(Instant::now());
        self.publish_machine(|m| m.engine_setup = None);
        let opened = if machine.direct() { None } else { machine.open_engine().err().map(Notice::Error) };
        self.launcher_game(machine.as_ref());
        opened
    }

    /// A Mac: VRChat started on `profile` by the engine itself; or, where it cannot be, Obelisk opened for the person to
    /// press Play in (no profile then), the destination waiting for the game.
    /// `started` is the start's lock, held by [`Self::launcher_play`].
    fn launcher_play_by_hand(self: &Shared, machine: &dyn Machine, errand: Errand, started: &mut Option<Instant>) -> Notice {
        let direct = machine.direct();
        let profile = errand.profile.unwrap_or(0);
        let destination = errand.destination.clone();
        let account = errand.account.clone();
        let account = account.as_deref();
        // Started by N4N0 itself, straight into the destination, as on a PC (or without it, when that one goes after the start).
        let way = if direct { self.launcher_start_way(machine, &errand, profile) } else { Way::Direct };
        let given = matches!(way, Way::Direct | Way::AfterProof);
        let args = if direct {
            let settings = self.view().settings.launch.clone();
            let there = destination.as_ref().filter(|_| way == Way::Direct).map(|p| (p.world_id.as_str(), p.instance_id.as_str()));
            orbiters_core::obelisk::game_arguments(orbiters_core::vrchat_launch::arguments_on(LaunchMode::Desktop, &settings, Vec::new(), profile, there))
        } else {
            Vec::new()
        };
        let under_way = started.is_some_and(|t| t.elapsed() < MAC_STARTING_FOR) || machine.starting();
        if errand.running_only && !under_way {
            return no_longer_running();
        }
        if direct && !machine.installed() {
            // Not set up yet: the set-up starts (or goes on), and the game follows it, into where this press goes.
            if destination.is_some() {
                if !given {
                    return nobodys();
                }
                self.launcher.lock().unwrap().queued = Some((errand, Instant::now()));
            }
            self.launcher_action(Action::InstallEngine { profile });
            return Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch };
        }
        if machine.engine_missing() {
            let _ = machine.open_engine();
            return Notice::Error(tr("VRChat runs on a Mac through Obelisk, a free app. Get it, turn on Beta versions in its settings, then download VRChat in it.").into());
        }
        if !machine.installed() {
            let _ = machine.open_engine();
            return Notice::Error(tr("Download VRChat in Obelisk (with Beta versions on in its settings), then come back.").into());
        }
        // A second Play while the first start is under way starts nothing again, and keeps the destination the first
        // asked for (a plain Play asks for none); a new destination replaces it.
        if under_way {
            // Another profile's: one start at a time, and nothing of this one goes to the account of that one. (One for the
            // game that starts goes to its own account, as an invite to oneself, whichever profile that is.)
            let newest = machine.newest_profile().unwrap_or(0);
            if direct && !errand.running_only && newest != profile {
                return Notice::Error(tr("Starting VRChat…").into());
            }
            if destination.is_some() {
                let profile = direct.then_some(newest).or(errand.profile);
                let errand = Errand { profile, ..errand };
                if way == Way::AfterProof {
                    self.launcher_join_after(machine, errand, Vec::new());
                } else {
                    self.launcher.lock().unwrap().pending = Some(Pending { errand, since: Instant::now(), proof: false, before: Vec::new(), launch: None, seen: None });
                }
            }
            return Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch };
        }
        machine.set_render_percent(self.view().settings.launch.mac_render_percent);
        // Checked on what is launched: the account that is signed in, and the one of the profile it starts on.
        if errand.scoped() && !self.launcher_live(account) {
            return changed_account();
        }
        if !given {
            return nobodys();
        }
        // A destination is of the account it was asked on: a start of it again is too.
        match machine.start_for(&args, &errand) {
            Ok(()) => {
                // Bound to the account it was asked on.
                *started = Some(Instant::now());
                // A game the person starts by hand takes its destination as an invite once it is up.
                if way == Way::AfterProof {
                    self.launcher_join_after(machine, errand.clone(), Vec::new());
                } else {
                    self.launcher.lock().unwrap().pending = (!direct && destination.is_some()).then(|| Pending { errand: errand.clone(), since: Instant::now(), proof: false, before: Vec::new(), launch: None, seen: None });
                }
                self.publish_machine(|m| if direct { starting_on(m, profile, LaunchMode::Desktop) } else { m.game = Game::Starting(LaunchMode::Desktop) });
                Notice::Sent { to: destination.map(|p| p.world_id).unwrap_or_default(), action: SentKind::Launch }
            }
            Err(words) => Notice::Error(tr!("VRChat could not be started: {reason}", reason = words)),
        }
    }

    /// Hides an invite, a request or a boop here; an invite or a request is hidden at VRChat too (nobody is told).
    fn launcher_dismiss(&self, id: &str, account: Option<&str>) {
        // A press made on another account's list changes nothing of the list on show (nor the kept dismissals).
        let shown = match &self.view().launcher.source {
            Source::SignedIn { user_id, .. } => Some(user_id.clone()),
            _ => None,
        };
        if shown.is_some() && shown.as_deref() != account {
            return;
        }
        // A signed-in list's dismissal is kept (30 days) only while that account is still the one signed in: a list left on
        // show for a moment after a sign-out or a switch changes nothing (REQUEST15 third pass, blocker 2).
        if shown.is_some() && !self.launcher_live(account) {
            return;
        }
        let was_invite = self.view().launcher.invites.iter().any(|i| i.id == id);
        if self.view().launcher.boops.iter().any(|b| b.id == id) {
            // VRChat lists a boop again at each full read: remembered here, so that it does not come back.
            let mut rt = self.launcher.lock().unwrap();
            let at = now();
            let dismissed = rt.dismissed.get_or_insert_with(load_dismissed);
            dismissed.retain(|_, t| at - *t < DISMISSED_FOR);
            dismissed.insert(id.to_string(), at);
            save_dismissed(dismissed);
        }
        self.forget_launcher(|l| {
            l.invites.retain(|i| i.id != id);
            l.boops.retain(|b| b.id != id);
        });
        if was_invite {
            if let (Ok(session), Some(account)) = (self.launcher_sender(account), account) {
                let _ = send_as(&session.client, account, |c| c.hide_notification(id));
            }
        }
    }

    /// Whether `account` is the one signed in right now (the session, not the list on show).
    fn launcher_live(&self, account: Option<&str>) -> bool {
        let live = self.launcher_client().map(|s| s.by.user);
        account.is_some() && live.as_deref() == account
    }

    fn launcher_send(&self, boop: bool, to: &str, emoji: Option<&str>, account: Option<&str>) -> Notice {
        let kind = if boop { SentKind::Boop } else { SentKind::RequestInvite };
        // Refused before anything left: keyed, so that the page takes back its "Sent" for this person.
        let failed = |n: Notice| match n {
            Notice::Error(text) => Notice::Failed { to: to.into(), action: kind, text },
            other => other,
        };
        let (session, account) = match (self.launcher_sender(account), account) {
            (Ok(session), Some(account)) => (session, account),
            (Err(notice), _) => return failed(notice),
            (Ok(_), None) => return failed(changed_account()),
        };
        let at = now();
        let taken = self.launcher.lock().unwrap().cooldowns.take(boop, to, at);
        if let Err(until) = taken {
            return Notice::CoolingDown { to: to.into(), until };
        }
        // With the press's account only, checked under the lock the send takes.
        let sent = {
            let mut c = session.client.lock().unwrap();
            if c.signed_in_user_id() != Some(account) {
                drop(c);
                self.launcher.lock().unwrap().cooldowns.release(boop, to);
                return failed(changed_account());
            }
            if boop { c.boop(to, emoji) } else { c.request_invite(to) }
        };
        let notice = match sent {
            Ok(()) => Notice::Sent { to: to.into(), action: if boop { SentKind::Boop } else { SentKind::RequestInvite } },
            Err(e) => {
                // Not sent (refused or never left): the person may try again at once. A maybe-sent keeps its cool-down.
                if !matches!(e, Error::OutcomeUnknown) {
                    self.launcher.lock().unwrap().cooldowns.release(boop, to);
                }
                Notice::Failed { to: to.into(), action: if boop { SentKind::Boop } else { SentKind::RequestInvite }, text: error_text(&e) }
            }
        };
        let cooldowns = self.launcher.lock().unwrap().cooldowns.clone();
        self.publish_launcher(&session.by, |l| cooldowns.mark(&mut l.friends, now()));
        notice
    }

    fn launcher_profile(&self, id: &str, account: Option<&str>) {
        // The row, the source it came from and this request, taken together under the launcher's lock: a press made on
        // another account's list opens nothing, and a later request (or account) makes this one write nothing.
        // The client and whose session it is are read first (never a client's lock under the launcher's).
        let session = self.launcher_client();
        let client_account = session.as_ref().map(|s| s.by.user.clone());
        let (friend, epoch, request, session) = {
            let mut rt = self.launcher.lock().unwrap();
            let view = self.view();
            let shown = match &view.launcher.source {
                Source::SignedIn { user_id, .. } => Some(user_id.as_str()),
                _ => None,
            };
            if shown != account {
                return;
            }
            // A signed-in list's profile opens only while its account is still signed in: nothing of the row (its note)
            // is shown from a list whose session ended (REQUEST15 third pass, blocker 2).
            if shown.is_some() && client_account.as_deref() != account {
                return;
            }
            let Some(friend) = view.launcher.friend(id).cloned().or_else(|| view.launcher.me.clone().filter(|m| m.id == id)) else { return };
            let session = session.filter(|_| client_account.as_deref() == account);
            rt.profile_request += 1;
            let request = rt.profile_request;
            let open = Profile { friend: friend.clone(), loading: session.is_some(), ..Profile::default() };
            match &session {
                Some(session) => self.publish_launcher(&session.by, |l| l.profile = Some(open)),
                None => self.publish_local_list(|l| l.profile = Some(open)),
            };
            (friend, rt.epoch, request, session)
        };
        let Some(Session { client, by }) = session else { return };
        let kept = self.launcher.lock().unwrap().profiles.get(id).filter(|(at, ..)| at.elapsed() < PROFILE_FOR).map(|(_, p, u, m)| (p.clone(), u.clone(), *m));
        let (public, user, mutuals) = match kept {
            Some(kept) => kept,
            None => {
                // Each call only while this profile is still the one open (clicking down a list asks for the last one).
                let wanted = || {
                    let rt = self.launcher.lock().unwrap();
                    rt.epoch == epoch && rt.profile_request == request && self.view().launcher.profile.as_ref().is_some_and(|p| p.friend.id == id)
                };
                if !wanted() {
                    return;
                }
                let public = client.lock().unwrap().public_profile(id).unwrap_or(serde_json::Value::Null);
                if !wanted() {
                    return;
                }
                let user = client.lock().unwrap().user_value(id).unwrap_or(serde_json::Value::Null);
                if !wanted() {
                    return;
                }
                let mutuals = client.lock().unwrap().mutuals(id).ok();
                let mut rt = self.launcher.lock().unwrap();
                if rt.epoch != epoch {
                    return;
                }
                if rt.profiles.len() >= PROFILES_KEPT {
                    rt.profiles.retain(|_, (at, ..)| at.elapsed() < PROFILE_FOR);
                    if rt.profiles.len() >= PROFILES_KEPT {
                        rt.profiles.clear();
                    }
                }
                rt.profiles.insert(id.to_string(), (Instant::now(), public.clone(), user.clone(), mutuals));
                (public, user, mutuals)
            }
        };
        let profile = launcher_feed::profile(friend, &public, &user, mutuals);
        // Checked and written under the launcher's lock: an account change in between writes nothing.
        let rt = self.launcher.lock().unwrap();
        if rt.epoch != epoch || rt.profile_request != request {
            return;
        }
        self.publish_launcher(&by, |l| {
            if l.profile.as_ref().is_some_and(|p| p.friend.id == profile.friend.id) {
                l.profile = Some(profile);
            }
        });
        drop(rt);
    }

    #[cfg(test)]
    pub(super) fn launcher_machine(&self, machine: Arc<dyn Machine>) {
        self.launcher.lock().unwrap().machine = machine;
    }
}

fn dismissed_file() -> Option<std::path::PathBuf> {
    // Tests keep it in memory: never the person's own file.
    if cfg!(test) {
        return None;
    }
    orbiters_core::paths::roots().map(|r| r.config.join("launcher-dismissed.json"))
}

fn load_dismissed() -> BTreeMap<String, i64> {
    let at = now();
    let mut kept: BTreeMap<String, i64> = dismissed_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    kept.retain(|_, t| at - *t < DISMISSED_FOR);
    kept
}

fn save_dismissed(kept: &BTreeMap<String, i64>) {
    if let (Some(file), Ok(bytes)) = (dismissed_file(), serde_json::to_vec(kept)) {
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
}

/// The worlds named by a place whose name is not known yet, friends in the list's order first.
pub fn unnamed_worlds(state: &LauncherState) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let by_id: HashMap<&str, &orbiters_core::launcher::Friend> = state.friends.iter().map(|f| (f.id.as_str(), f)).collect();
    let places = state
        .invites
        .iter()
        .filter_map(|i| i.place.as_ref())
        .chain(state.groups.iter().flat_map(|g| g.members.iter()).filter_map(|id| by_id.get(id.as_str()).copied()).filter_map(|f| match &f.whereabouts {
            orbiters_core::launcher::Whereabouts::World(p) => Some(p),
            _ => None,
        }));
    for p in places {
        if p.world_name.is_empty() && !ids.contains(&p.world_id) {
            ids.push(p.world_id.clone());
        }
    }
    ids
}

/// Writes the known worlds' names and pictures into every place of the state.
pub fn fill_worlds(state: &mut LauncherState, worlds: &HashMap<String, Option<(String, String)>>) {
    let fill = |p: &mut Place| {
        if let Some(Some((name, picture))) = worlds.get(&p.world_id) {
            if p.world_name.is_empty() {
                p.world_name = name.clone();
            }
            if p.world_picture.is_empty() {
                p.world_picture = picture.clone();
            }
        }
    };
    let friends = state.friends.iter_mut().chain(state.me.iter_mut()).chain(state.profile.iter_mut().map(|p| &mut p.friend));
    for f in friends {
        if let orbiters_core::launcher::Whereabouts::World(p) = &mut f.whereabouts {
            fill(p);
        }
    }
    for i in &mut state.invites {
        if let Some(p) = &mut i.place {
            fill(p);
        }
    }
}

fn apply_pictures(pictures: &HashMap<String, Option<String>>, d: &Decorations, look: &mut orbiters_core::launcher::Look) {
    let get = |id: &Option<String>| id.as_ref().and_then(|id| pictures.get(id).cloned().flatten()).unwrap_or_default();
    look.icon_frame = get(&d.icon_frame);
    look.nameplate = get(&d.nameplate);
}

#[path = "launcher_mine.rs"]
mod mine;

#[cfg(test)]
#[path = "launcher_tests.rs"]
mod tests;

/// M13: the Mac's roster takes a recorded game only as the very process recorded (its birth), never one that reused its number.
#[cfg(all(test, target_os = "macos"))]
#[test]
fn a_recorded_game_is_running_only_as_the_process_recorded() {
    let me = std::process::id();
    let birth = super::bridge_mac::born(me).expect("this test's own process");
    assert!(still_born(me, Some(birth)));
    assert!(!still_born(me, Some((birth.0, birth.1.wrapping_add(1)))), "the same number, another birth");
    assert!(still_born(me, None), "an older record without a birth: by its number");
    assert!(!still_born(u32::MAX - 7, None));
}
