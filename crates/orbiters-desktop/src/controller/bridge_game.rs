//! The real [`Game`] of the VRChat bridge (`crate::bridge`): what the controller knows of VRChat, the signed-in VRChat session
//! for instances (with its rate limits and backoff, `orbiters_core::vrchat_api`), the OSC service, the 3D picture of
//! `burst.rs`, and Windows.

use super::{Controller, Shared};
use crate::bridge::game::{CaptureSeen, Failure, Find, Game, GameWindow, LaunchPlan, OscArg, PictureFiles, ReconstructionSeen, Seen, Shot, WorldCard};
use crate::bridge::input::Input;
use crate::bridge::keeper::{Flag, Keeper};
use crate::bridge::{capture, win, Bridge, Command};
use orbiters_core::maps::WorldList;
use orbiters_core::settings::Photo3dLength;
use orbiters_core::vrchat_api::Error as VrchatError;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};

/// What `find_worlds` gives at most, a page.
const WORLDS_A_PAGE: usize = 20;
/// How far into VRChat's log its command line is, at most.
const LOG_HEAD: usize = 256 * 1024;

struct RealGame {
    ctl: Weak<Controller>,
    exe: PathBuf,
    /// The log last looked at for its command line, and what it said.
    desktop_mode: Mutex<(Option<PathBuf>, Option<bool>)>,
}

impl RealGame {
    fn ctl(&self) -> Result<Shared, Failure> {
        self.ctl.upgrade().ok_or_else(|| Failure::new("app_closing", "Orbiters - N4N0 is closing.", "Stop here."))
    }

    /// Whether the game runs without VR, from the arguments VRChat's log starts with (`Arg: --no-vr`, `VR Disabled`).
    fn desktop_mode(&self, log: Option<&PathBuf>) -> Option<bool> {
        let log = log?;
        let mut known = self.desktop_mode.lock().unwrap();
        if known.0.as_ref() == Some(log) && known.1.is_some() {
            return known.1;
        }
        let head = read_head(log, LOG_HEAD);
        let said = desktop_mode_in(&head);
        *known = (Some(log.clone()), said);
        said
    }
}

fn read_head(path: &Path, most: usize) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(most as u64).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What VRChat's log says of VR: `None` until it listed its arguments.
fn desktop_mode_in(head: &str) -> Option<bool> {
    if head.lines().any(|l| l.ends_with("Arg: --no-vr") || l.ends_with("VR Disabled")) {
        return Some(true);
    }
    head.lines()
        .any(|l| l.contains("Launching with args:"))
        .then_some(false)
        .filter(|_| head.lines().any(|l| l.contains("VR Enabled") || l.contains("Starting VR")))
}

/// An error of VRChat's API as the agent reads it.
fn api_failure(error: anyhow::Error, doing: &str) -> Failure {
    match error.downcast_ref::<VrchatError>() {
        Some(VrchatError::RateLimited(wait)) => {
            let seconds = wait.as_secs().max(1);
            Failure::new("rate_limited", format!("VRChat's API asks to wait: {doing} was not done."), format!("Try again in {seconds} s, and ask less often."))
                .retry_after(seconds)
        }
        Some(VrchatError::Unauthorized | VrchatError::Disabled) => Failure::new(
            "vrchat_not_signed_in",
            "VRChat's session here is not signed in, or it expired.",
            "Tell the person to sign in to VRChat in Orbiters - N4N0's Settings.",
        ),
        Some(VrchatError::NotFound) => Failure::new(
            "world_not_found",
            format!("VRChat does not know this: {doing} was not done."),
            "Check the id with find_worlds, or go on with the next world.",
        ),
        Some(VrchatError::Unavailable) => Failure::new("vrchat_unavailable", "VRChat's API could not be reached.", "Try again in a minute.").retry_after(60),
        Some(VrchatError::Refused(status, said)) => {
            Failure::new("vrchat_refused", format!("VRChat refused ({status}): {said}"), "Go on with the next world. If it happens for all, tell the person.")
        }
        _ => Failure::new("vrchat_api_error", format!("{doing} failed: {error}"), "Try again later, or go on with the next world."),
    }
}

impl Game for RealGame {
    fn seen(&self) -> Seen {
        let Ok(ctl) = self.ctl() else { return Seen::default() };
        let snap = ctl.view();
        let world = &snap.vrchat.world;
        let stats = snap.capture.stats.as_ref();
        Seen {
            running: snap.vrchat.running,
            pid: snap.vrchat.pid,
            log_found: snap.vrchat.log_found,
            world_id: world.world_id.clone(),
            world_name: world.world_name.clone(),
            instance: world.instance.clone(),
            in_world: world.in_world,
            local_player: world.local_player.clone(),
            others: world.others(),
            vrchat_user: snap.vrchat_login.user.as_ref().map(|u| (u.id.clone(), u.display_name.clone())),
            desktop_mode: if snap.vrchat.running { self.desktop_mode(world.log_file.as_ref()) } else { None },
            osc_found: snap.osc.vrchat_found,
            camera_pose_known: ctl.camera_pose_known(),
            speed: snap.vrchat.motion.map(|m| m.speed),
            capture: CaptureSeen {
                state: serde_json::to_value(snap.capture.state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
                session: snap.capture.session_dir.clone(),
                frames: stats.map_or(0, |s| s.frames),
                keyframes: stats.map_or(0, |s| s.keyframes),
                no_strip: snap.capture.no_strip.map(|n| n.message().to_string()),
                error: snap.capture.error.clone(),
            },
            reconstruction: snap.reconstruction.as_ref().map(|job| ReconstructionSeen {
                session: job.session_dir.clone(),
                stage: format!("{:?}", job.stage).to_lowercase(),
                progress: job.progress,
            }),
            queue: snap.reconstruct_queue.clone(),
            queue_waits_for: ctl.reconstruction_waits_for().to_string(),
            steamvr: snap.steamvr.running,
        }
    }

    fn camera_open(&self) -> Option<bool> {
        self.ctl().ok()?.camera_open()
    }

    fn create_private_instance(&self, world: &str, region: &str) -> Result<String, Failure> {
        self.ctl()?.vrchat_source().create_private_instance(world, region).map_err(|e| api_failure(e, "making the instance"))
    }

    fn find_worlds(&self, find: &Find, page: usize) -> Result<Vec<WorldCard>, Failure> {
        let ctl = self.ctl()?;
        let list = match find {
            Find::Visited => {
                let history = orbiters_core::visited::History::load(&orbiters_core::visited::history_file());
                let mut worlds: Vec<(&String, &orbiters_core::visited::WorldVisits)> = history.worlds.iter().collect();
                worlds.sort_by_key(|(_, w)| std::cmp::Reverse(w.last()));
                return Ok(worlds
                    .into_iter()
                    .skip(page * WORLDS_A_PAGE)
                    .take(WORLDS_A_PAGE)
                    .map(|(id, w)| WorldCard { id: id.clone(), name: w.name.clone().unwrap_or_default(), author: None, occupants: None, last_visit: w.last() })
                    .collect());
            }
            Find::Search(query) => WorldList::Search(query.clone()),
            Find::Favorites => WorldList::Favorites,
            Find::Popular => WorldList::Popular,
            Find::Active => WorldList::Active,
        };
        let found = ctl.vrchat_source().worlds(&list, page).map_err(|e| api_failure(e, "finding worlds"))?;
        Ok(found
            .items
            .into_iter()
            .map(|f| WorldCard {
                id: f.world.world_id.clone(),
                name: f.world.name.clone(),
                author: f.world.author_name.clone(),
                occupants: f.occupants,
                last_visit: None,
            })
            .collect())
    }

    fn launch(&self, plan: &LaunchPlan) -> Result<(), Failure> {
        win::start_vrchat(&plan.args())
    }

    fn window(&self, pid: u32) -> Option<GameWindow> {
        win::window_of(pid, None)
    }

    fn is_foreground(&self, window: &GameWindow) -> bool {
        win::is_foreground(window.handle)
    }

    fn bring_forward(&self, window: &GameWindow) {
        win::bring_forward(window.handle);
    }

    fn close(&self, window: &GameWindow) -> Result<(), Failure> {
        win::close_window(window.handle)
    }

    fn capture(&self, window: &GameWindow, max_width: u32) -> Result<Shot, Failure> {
        capture::shot_in_child(&self.exe, window.handle, max_width)
    }

    fn osc(&self, address: &str, value: OscArg) -> Result<(), Failure> {
        let ctl = self.ctl()?;
        let arg = match value {
            OscArg::Int(i) => rosc::OscType::Int(i),
            OscArg::Float(f) => rosc::OscType::Float(f),
        };
        match ctl.with_osc(|o| o.send(address, vec![arg])) {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(Failure::new(
                "osc_failed",
                format!("OSC to VRChat failed: {e}"),
                "Check get_state's osc_answers: OSC must be on in VRChat's Action Menu (r), Options, OSC.",
            )),
            None => Err(Failure::new("osc_off", "OSC is off in Orbiters - N4N0.", "Tell the person to turn it on in its Settings.")),
        }
    }

    fn osc_target(&self) -> Option<std::net::SocketAddr> {
        self.ctl().ok()?.with_osc(|o| o.vrchat_target()).flatten()
    }

    fn inject(&self, window: &GameWindow, input: &[Input]) -> Result<(), Failure> {
        win::inject(window, input)
    }

    fn release(&self, input: &[Input]) {
        win::release(input);
    }

    fn take_picture(&self, length: Photo3dLength) -> Result<(), Failure> {
        self.ctl()?.take_3d_picture_of(length).map_err(|e| {
            let said = e.to_string();
            if said.contains(orbiters_n4n0::platform::CAMERA_CLOSED) {
                Failure::new("camera_closed", "VRChat's camera is not open.", "press esc, screenshot, click the camera's icon with mouse, press esc.")
            } else {
                Failure::new("picture_did_not_start", said, "Call get_state and try again; if it stays, tell the person.")
            }
        })
    }

    fn stop_picture(&self) {
        if let Ok(ctl) = self.ctl() {
            if let Err(e) = ctl.stop_capture() {
                eprintln!("the bridge stopped a recording: {e:#}");
            }
        }
    }

    fn picture_files(&self, session: &Path) -> PictureFiles {
        let stats = orbiters_core::bundle::read_manifest(session).map(|m| m.stats).ok();
        PictureFiles {
            frames: stats.as_ref().map_or(0, |s| s.frames),
            keyframes: stats.as_ref().map_or(0, |s| s.keyframes),
            reconstructed: orbiters_core::reconstruction::read_report(session).is_some(),
        }
    }

    fn hold_reconstructions(&self, hold: bool) {
        if let Ok(ctl) = self.ctl() {
            ctl.bridge_holds_reconstructions.store(hold, Ordering::Relaxed);
            if !hold {
                ctl.next_reconstruction();
            }
        }
    }
}

/// What the keeper asks of the controller.
struct RealFlag {
    ctl: Weak<Controller>,
}

impl Flag for RealFlag {
    fn identity(&self) -> Option<String> {
        let ctl = self.ctl.upgrade()?;
        ctl.vrchat_identity().map(|(api, token)| format!("{}|{token}", api.url("")))
    }

    fn ask(&self) -> anyhow::Result<bool> {
        let ctl = self.ctl.upgrade().ok_or_else(|| anyhow::anyhow!("the app is closing"))?;
        let (api, token) = ctl.vrchat_identity().ok_or_else(|| anyhow::anyhow!("signed out"))?;
        api.member_feature(&token, orbiters_core::features::VRCHAT_BRIDGE)
    }

    fn enabled(&self) -> bool {
        self.ctl.upgrade().is_some_and(|ctl| ctl.with_settings(|s| s.experimental.vrchat_bridge))
    }

    fn turn_off(&self) {
        if let Some(ctl) = self.ctl.upgrade() {
            ctl.update_settings(|s| s.experimental.vrchat_bridge = false);
        }
    }
}

impl Controller {
    /// Starts the bridge's keeper (the tray service: never the window's process, nor tests). Nothing listens until the
    /// server says yes for the account and Settings has it on.
    pub fn start_bridge(self: &Shared) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => {
                eprintln!("the VRChat bridge stays off: {e}");
                return;
            }
        };
        let game = Arc::new(RealGame { ctl: Arc::downgrade(self), exe: exe.clone(), desktop_mode: Mutex::new((None, None)) });
        let bridge = Bridge::new(game, Some(exe));
        let flag = Arc::new(RealFlag { ctl: Arc::downgrade(self) });
        let ctl = Arc::downgrade(self);
        let keeper = Keeper::for_service(
            bridge,
            flag,
            Box::new(move |status| {
                if let Some(ctl) = ctl.upgrade() {
                    ctl.update_if(|s| {
                        let changed = s.bridge != status;
                        if changed {
                            s.bridge = status;
                        }
                        changed
                    });
                }
            }),
        );
        keeper.start();
        *self.bridge.lock().unwrap() = Some(keeper);
    }

    /// What Settings asks of the bridge.
    pub fn bridge_command(&self, command: Command) {
        let keeper = self.bridge.lock().unwrap().clone();
        if let Some(keeper) = keeper {
            keeper.command(command);
        }
    }

    /// The app quits: input released and the port closed while OSC still works.
    pub fn stop_bridge(&self) {
        let keeper = self.bridge.lock().unwrap().take();
        if let Some(keeper) = keeper {
            keeper.shutdown();
        }
    }

    /// Whether VRChat told where its camera is: a 3D picture starts from there.
    fn camera_pose_known(&self) -> bool {
        let osc = self.osc_values.lock().unwrap();
        osc.latest.get(super::burst::POSE).is_some_and(|(values, _)| {
            let floats = super::burst::floats(values);
            floats.len() == 6 && floats.iter().any(|f| *f != 0.0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logs_own_arguments_say_whether_it_runs_without_vr() {
        // As in VRChat's log of 2026-11-04.
        let desktop = "2026.11.04 22:01:04 Debug      -  Launching with args: 4\n2026.11.04 22:01:04 Debug      -  Arg: C:\\Steam\\VRChat.exe\n2026.11.04 22:01:04 Debug      -  Arg: --no-vr\n2026.11.04 22:01:04 Debug      -  VR Disabled\n";
        assert_eq!(desktop_mode_in(desktop), Some(true));
        assert_eq!(desktop_mode_in("nothing yet"), None);
    }

    #[test]
    fn the_apis_errors_tell_the_agent_what_to_do() {
        let failure = api_failure(VrchatError::RateLimited(std::time::Duration::from_secs(61)).into(), "making the instance");
        assert_eq!((failure.code, failure.retry_after_s), ("rate_limited", Some(61)));
        assert_eq!(api_failure(VrchatError::Unauthorized.into(), "x").code, "vrchat_not_signed_in");
        assert_eq!(api_failure(VrchatError::NotFound.into(), "x").code, "world_not_found");
        assert_eq!(api_failure(anyhow::anyhow!("something else"), "x").code, "vrchat_api_error");
    }
}
