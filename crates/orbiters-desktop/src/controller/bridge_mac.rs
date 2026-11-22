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

/// How much of the engine's log is read at one look, at most (it grows by megabytes a minute while the game runs).
const ENGINE_LOG_LOOK: u64 = 8 << 20;
/// How often the windows and the engine's log are looked at, at most: `seen` is asked many times a second while waiting.
const LOOK_EVERY: Duration = Duration::from_millis(700);
/// How long the program may take to say a quit was taken.
const QUIT_LIMIT: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Mine {
    /// The shared start's launch id of the bridge's start, when it was asked, and its launch link.
    launch: Option<u64>,
    at: Option<SystemTime>,
    link: Option<String>,
    /// The engine's log from where it was at the launch: only what came after is this start's.
    engine_from: u64,
    renderer_lost: bool,
    /// The launcher and its birth, once the program said it.
    launcher: Option<(u32, u64)>,
    /// The launcher's window server.
    wsd: Option<u32>,
    /// This start's own VRChat log (its arguments name the launch link), and its follower.
    log: Option<PathBuf>,
    args: Option<Vec<String>>,
    watcher: Option<LogWatcher>,
    /// The last look at the windows and the engine's log.
    looked: Option<Instant>,
    window: Option<MacWindow>,
}

pub(super) struct MacGame {
    real: RealGame,
    ctl: Weak<Controller>,
    mine: Mutex<Mine>,
}

/// When process `pid` started (Unix seconds), while it runs.
fn birth(pid: u32) -> Option<u64> {
    let mut system = sysinfo::System::new();
    let p = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[p]), true, sysinfo::ProcessRefreshKind::nothing());
    system.process(p).filter(|p| !matches!(p.status(), sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead)).map(|p| p.start_time())
}

/// The engine's window server the launcher `launcher` started (its child `wsd`).
fn window_server(launcher: u32) -> Option<u32> {
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
    system
        .processes()
        .values()
        .find(|p| p.parent() == Some(sysinfo::Pid::from_u32(launcher)) && p.name().to_string_lossy() == "wsd")
        .map(|p| p.pid().as_u32())
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

impl MacGame {
    pub(super) fn new(ctl: Weak<Controller>, exe: PathBuf) -> Self {
        Self { real: RealGame::new(ctl.clone(), exe), ctl, mine: Mutex::default() }
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

    /// The bridge's own game while it runs: its launcher, verified by its birth.
    fn own_launcher(&self) -> Option<u32> {
        let launch = self.launch().filter(|l| l.running && !l.over)?;
        let pid = launch.launcher?;
        let born = birth(pid)?;
        let mut mine = self.mine.lock().unwrap();
        match mine.launcher {
            Some((p, b)) if p == pid && b != born => return None,
            Some((p, _)) if p == pid => {}
            _ => {
                mine.launcher = Some((pid, born));
                mine.wsd = None;
                mine.window = None;
            }
        }
        Some(pid)
    }

    /// A VRChat that is not the bridge's: Home's, the person's own, one left from before a restart of the service.
    fn others_game(&self, own: Option<u32>) -> Option<u32> {
        let machine = self.machine()?;
        machine.running().into_iter().map(|(pid, _)| pid).find(|pid| Some(*pid) != own)
    }

    /// Follows the bridge's own start: its log, its window, the engine's word on its graphics.
    fn look(&self, launcher: u32, library: &Path) {
        let mut mine = self.mine.lock().unwrap();
        if mine.log.is_none() {
            if let (Some(at), Some(link)) = (mine.at, mine.link.clone()) {
                if let Some((log, args)) = log_of_launch(&obelisk::vrchat_log_dir(library), at, &link) {
                    mine.watcher = Some(LogWatcher::of_file(log.clone()));
                    mine.log = Some(log);
                    mine.args = Some(args);
                }
            }
        }
        if let Some(watcher) = mine.watcher.as_mut() {
            watcher.poll();
        }
        if mine.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        mine.looked = Some(Instant::now());
        if mine.wsd.is_none() {
            mine.wsd = window_server(launcher);
        }
        mine.window = mine.wsd.and_then(|wsd| native::game_window(&native::windows(), wsd));
        if !mine.renderer_lost {
            let file = obelisk::engine_log(&obelisk::home());
            if let Ok(mut f) = std::fs::File::open(&file) {
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                let from = mine.engine_from.max(len.saturating_sub(ENGINE_LOG_LOOK));
                if len > from && f.seek(SeekFrom::Start(from)).is_ok() {
                    let mut bytes = Vec::new();
                    let _ = f.take(len - from).read_to_end(&mut bytes);
                    mine.renderer_lost = obelisk::renderer_lost(&String::from_utf8_lossy(&bytes));
                    // A line cut at the end is read again next time.
                    mine.engine_from = len.saturating_sub(256);
                }
            }
        }
    }

    /// The bridge's own window, while its game runs.
    fn own_window(&self, pid: u32) -> Option<MacWindow> {
        (self.own_launcher() == Some(pid)).then(|| self.mine.lock().unwrap().window).flatten()
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
        let supports_session = program.parent().and_then(|f| f.file_name()).and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("obelisk-cli-")).and_then(|v| v.strip_suffix("-arm64")).is_some_and(version_has_session);
        let mut args = vec!["steam", "quit", obelisk::STEAM_APP];
        if supports_session {
            args.extend(["--session", session.as_str()]);
        } else {
            // An older program: the running session is checked first, by its own word, to be this start's.
            let mut running = None;
            let _ = super::obelisk_cli::run_for(&program, &["steam", "running"], QUIT_LIMIT, |line| {
                if let Some(id) = serde_json::from_str::<serde_json::Value>(line).ok().and_then(|v| v["session_id"].as_str().map(str::to_owned)) {
                    running = Some(id);
                }
            });
            if running.as_deref() != Some(session.as_str()) {
                return Err(Failure::new("not_started_by_bridge", "The VRChat running now is not the one this bridge started, so it was not asked to close.", "Leave it alone and tell the person.")); // i18n: not shown
            }
        }
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

/// Whether obelisk-cli `version` has `steam quit --session` (0.1.3 and later).
fn version_has_session(version: &str) -> bool {
    let parts: Vec<u32> = version.split('.').filter_map(|p| p.parse().ok()).collect();
    parts.len() >= 3 && (parts[0], parts[1], parts[2]) >= (0, 1, 3)
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
                seen.renderer_lost = mine.renderer_lost;
                seen.desktop_mode = mine.args.as_ref().map(|a| a.iter().any(|x| x == "--no-vr"));
                if let Some(watcher) = mine.watcher.as_ref().filter(|w| w.caught_up()) {
                    let world = &watcher.state;
                    seen.world_id = world.world_id.clone();
                    seen.world_name = world.world_name.clone();
                    seen.instance = world.instance.clone();
                    // Renderer evidence: in a world only while its window is on screen.
                    seen.in_world = world.in_world && mine.window.is_some();
                    seen.local_player = world.local_player.clone();
                    seen.others = world.others();
                    seen.avatar_name = world.avatar_name.clone().or_else(|| world.local_player.as_ref().and_then(|me| world.avatars.get(me).cloned()));
                }
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
        let link = crate::bridge::game::launch_link(&plan.world_id, &plan.instance);
        let engine_from = std::fs::metadata(obelisk::engine_log(&obelisk::home())).map(|m| m.len()).unwrap_or(0);
        let at = SystemTime::now();
        match mac.start_launch(&plan.args()) {
            Ok(id) => {
                *self.mine.lock().unwrap() = Mine { launch: Some(id), at: Some(at), link: Some(link), engine_from, ..Default::default() };
                Ok(())
            }
            Err(super::obelisk_cli::Refused::Wait(wait)) => Err(Failure::new(
                "start_wait",
                format!("VRChat ended early {} s ago on this Mac; a start this soon after one makes Easy Anti-Cheat fail.", (obelisk::AFTER_EARLY_END - wait).as_secs()), // i18n: not shown (an AI client reads it)
                format!("Wait {} s, then call go_to_world again.", wait.as_secs().max(1)), // i18n: not shown
            )
            .retry_after(wait.as_secs().max(1))),
            Err(super::obelisk_cli::Refused::Busy) => Err(Failure::new("vrchat_starting", "A VRChat is starting already on this Mac (from Orbiters' Home, or another start).", "Wait a minute, then call get_state.")), // i18n: not shown
            Err(super::obelisk_cli::Refused::Failed(why)) => Err(Failure::new("start_failed", format!("VRChat did not start: {why}"), "Ask the person to look at Orbiters - N4N0's Home.")), // i18n: not shown
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
        let (width, height) = native::content_pixels(&window, native::scale());
        Some(GameWindow { handle: window.id as isize, width, height })
    }

    fn is_foreground(&self, window: &GameWindow) -> bool {
        let wsd = self.mine.lock().unwrap().wsd;
        wsd.is_some() && native::front_owner() == wsd && self.mine.lock().unwrap().window.is_some_and(|w| w.id as isize == window.handle)
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
        let frame = self.mine.lock().unwrap().window;
        let png = native::capture_png(window.handle as u32).map_err(|e| Failure::new("screenshot_failed", format!("macOS did not give the window's picture: {e}"), "Call get_state; try again in a moment."))?; // i18n: not shown
        let image = image::load_from_memory(&png).map_err(|e| Failure::new("screenshot_failed", e.to_string(), "Try again in a moment."))?.to_rgba8(); // i18n: not shown
        // The title bar is the system's, not the game's: the picture is the content, as clicks and VRChat count it.
        let top = frame.map_or(0, |f| native::title_rows(image.height(), f.height)).min(image.height().saturating_sub(1));
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
        assert!(version_has_session("0.1.3") && version_has_session("0.2.0") && !version_has_session("0.1.1") && !version_has_session("x"));
    }
}
