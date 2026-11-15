//! What the bridge's tools act on: VRChat as seen from this app. [`Game`] is the seam between the tools, which hold every
//! rule (private instances only, the person first, bounded input), and the world: the real one is the tray service's
//! controller and Windows (`controller/bridge_game.rs`), the other a fake VRChat for tests and the lab (`lab.rs`), so
//! that the whole loop runs without touching the real game or the desktop of the person playing on this computer.

use super::input::Input;
use orbiters_core::settings::{Photo3dLength, PictureMode};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Why a tool did not do what it was asked: a code a model can branch on, what happened, and what to do next.
#[derive(Clone, Debug, Serialize)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_s: Option<u64>,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>, next: impl Into<String>) -> Self {
        Self { code, message: message.into(), next: next.into(), retry_after_s: None }
    }

    pub fn retry_after(mut self, seconds: u64) -> Self {
        self.retry_after_s = Some(seconds);
        self
    }
}

/// The top-level window of the running game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameWindow {
    pub handle: isize,
    /// The client area, in pixels: what a screenshot shows and what mouse coordinates mean.
    pub width: u32,
    pub height: u32,
}

/// What a recording looks like from outside it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CaptureSeen {
    /// `idle`, `starting`, `recording`, `paused`, `stopping` or `importing`.
    pub state: String,
    pub session: Option<PathBuf>,
    pub frames: u64,
    pub keyframes: u64,
    /// Why frames come without the capture strip, in the app's own sentence.
    pub no_strip: Option<String>,
    pub error: Option<String>,
}

/// The reconstruction running now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReconstructionSeen {
    pub session: PathBuf,
    pub stage: String,
    pub progress: f32,
}

/// Everything the tools read about the game, at one moment.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Seen {
    pub running: bool,
    pub pid: Option<u32>,
    /// VRChat's log folder was found, so joins can be seen at all.
    pub log_found: bool,
    pub world_id: Option<String>,
    pub world_name: Option<String>,
    pub instance: Option<String>,
    pub in_world: bool,
    pub local_player: Option<String>,
    /// People in the instance other than the local player.
    pub others: usize,
    /// The VRChat account signed in in Orbiters - N4N0's Settings (the one instances are made with): id and name.
    pub vrchat_user: Option<(String, String)>,
    /// The avatar worn, as the game's log names it.
    pub avatar_name: Option<String>,
    /// Started without VR, as the game's own log says; `None` until it said.
    pub desktop_mode: Option<bool>,
    /// VRChat's OSC answers (its OSCQuery service was found).
    pub osc_found: bool,
    /// VRChat told where its camera is (a 3D picture starts from there).
    pub camera_pose_known: bool,
    /// How fast the avatar moves, metres a second, as its parameters say (a quarter metre a second at best).
    pub speed: Option<f32>,
    pub capture: CaptureSeen,
    pub reconstruction: Option<ReconstructionSeen>,
    /// Sessions waiting for their reconstruction, next first.
    pub queue: Vec<PathBuf>,
    /// When the ones waiting start, as the app says it ("once VRChat closes …").
    pub queue_waits_for: String,
    pub steamvr: bool,
    /// The account this VRChat itself is signed in with, as its log says (a side session's profile has its own): id, name.
    pub session_account: Option<(String, String)>,
    /// This VRChat waits at its sign-in page: only the person can sign in, on its window.
    pub needs_sign_in: bool,
}

/// A place to find worlds from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Find {
    Search(String),
    Visited,
    Favorites,
    Popular,
    Active,
    Trending,
    /// VRChat's "Spotlight - PC" row (`orbiters_core::maps::Spotlight`).
    SpotlightPc,
    /// VRChat's "Spotlight - Cross Platform" row.
    SpotlightCrossPlatform,
}

/// A page of worlds, and whether another follows.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WorldPage {
    pub worlds: Vec<WorldCard>,
    pub more: bool,
}

/// A world an agent can pick.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorldCard {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// People in it now, when VRChat said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occupants: Option<u32>,
    /// When this account was last there (Unix seconds), from the visited worlds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_visit: Option<u64>,
}

/// A window's picture, ready for an MCP client.
#[derive(Clone, Debug)]
pub struct Shot {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The picture came out (almost) black: VRChat's loading screen, or a capture that does not work on this window.
    pub all_black: bool,
}

/// What became of a 3D picture's session, from its files and the queue.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PictureFiles {
    pub frames: u64,
    pub keyframes: u64,
    /// A finished reconstruction's result is there.
    pub reconstructed: bool,
    /// Why its last reconstruction failed.
    pub failed: Option<String>,
}

/// An OSC value for VRChat: buttons are ints (1 pressed, 0 released), axes floats from -1 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OscArg {
    Int(i32),
    Float(f32),
}

/// How to start VRChat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    pub world_id: String,
    pub instance: String,
    pub width: u32,
    pub height: u32,
    /// VRChat's `--profile`, whose account this start uses; `None`: the default.
    pub profile: Option<u32>,
    /// A side session's OSC ports, in and out (`--osc=<in>:127.0.0.1:<out>`); `None`: VRChat's defaults, for the app.
    pub osc: Option<(u16, u16)>,
}

impl LaunchPlan {
    /// The arguments VRChat gets, each one checked against docs.vrchat.com/docs/launch-options and Unity's player command
    /// line: `--no-vr` for desktop mode, Unity's `-screen-*` for a window of a known size (so a screenshot's pixels are
    /// mouse coordinates), and the launch link of the instance (none: the account's home). The main session has no `--osc`:
    /// VRChat's defaults are what the app's OSC finds through OSCQuery. A side session's ports are its own, so that two
    /// VRChats never share 9000 and 9001 (sessions.rs).
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![
            "--no-vr".to_string(),
            "-screen-fullscreen".to_string(),
            "0".to_string(),
            "-screen-width".to_string(),
            self.width.to_string(),
            "-screen-height".to_string(),
            self.height.to_string(),
        ];
        if let Some(profile) = self.profile {
            args.push(format!("--profile={profile}"));
        }
        if let Some((osc_in, osc_out)) = self.osc {
            args.push(super::sessions::osc_arg(osc_in, osc_out));
        }
        if !self.world_id.is_empty() {
            args.push(format!("vrchat://launch?ref=orbiters.cc&id={}:{}", self.world_id, self.instance));
        }
        args
    }
}

/// What starts side sessions and knows this computer: the controller's for real (controller/bridge_side.rs), the lab's in
/// tests.
pub trait Opener: Send + Sync {
    /// A side session's game for `profile` on its own OSC ports: nothing started yet ([`Game::launch`] starts it).
    fn side(&self, session: &str, profile: u32, osc: (u16, u16)) -> Result<std::sync::Arc<dyn Game>, Failure>;
    /// The side session went: the app forgets it.
    fn forget(&self, session: &str);
    /// A session taken over by another agent gets a new id.
    fn rename(&self, from: &str, to: &str);
    /// The VRChats running now, by process id.
    fn vrchat_pids(&self) -> Vec<u32>;
    /// The profiles the VRChats the bridge did not start run with, from their logs (0 for one without `--profile`).
    fn others_profiles(&self) -> Vec<u32>;
    /// Easy Anti-Cheat now.
    fn eac(&self) -> super::sessions::Eac;
    /// Memory free now, bytes.
    fn memory_free(&self) -> Option<u64>;
    /// How many VRChats the bridge may run at once (Settings).
    fn limit(&self) -> usize;
    /// The process whose window is the foreground now.
    fn foreground_pid(&self) -> Option<u32>;
    /// The main session's VRChat, the bridge's own: a side session's picture may borrow the app from it.
    fn set_main(&self, _pid: Option<u32>) {}
}

/// The Orbiters account world labels go under: the one signed in in Orbiters - N4N0's Settings, on the server its
/// Development environment switch chose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedbackAccount {
    /// The API's address (`https://api.orbiters.cc/`, or the development server's `http://127.0.0.1:4100/`).
    pub server: String,
    pub development: bool,
    /// The account's name and numeric id, when the saved account names them.
    pub name: Option<String>,
    pub id: Option<String>,
}

/// N4N0's world feedback as the bridge's label tools use it: the real one is the tray service's (controller/world_feedback.rs,
/// the very queue and calls a person's press on the worlds page goes through), the other the lab's.
pub trait Feedback: Send + Sync {
    /// The account labels go under; `None` while none is signed in.
    fn account(&self) -> Option<FeedbackAccount>;
    /// Asks Orbiters for `world`'s feedback now, as opening it on the worlds page does.
    fn show(&self, world: &str);
    /// What is known of `world`'s feedback now (its `loading` true until Orbiters answered once).
    fn feedback(&self, world: &str) -> orbiters_n4n0_core::feedback::WorldFeedback;
    /// What a person's press asks: queued on disk, sent under the account, retried.
    fn ask(&self, ask: orbiters_n4n0_core::feedback::FeedbackAsk);
}

/// VRChat as far as the tools care.
pub trait Game: Send + Sync {
    fn seen(&self) -> Seen;
    /// Whether VRChat's camera is open, `None` when VRChat does not say. Apart from [`Game::seen`] as it asks VRChat.
    fn camera_open(&self) -> Option<bool>;
    /// Whether the avatar worn has N4N0's capture kit: `None` when VRChat does not say. Reads VRChat's tree: not for every tick.
    fn capture_kit(&self) -> Option<bool>;
    /// A new private instance of `world`, owned by the signed-in VRChat account. Only ever that kind: nothing here can
    /// name another instance, or make a public, friends or group one.
    fn create_private_instance(&self, world: &str, region: &str) -> Result<String, Failure>;
    fn find_worlds(&self, find: &Find, page: usize) -> Result<WorldPage, Failure>;
    fn launch(&self, plan: &LaunchPlan) -> Result<(), Failure>;
    /// The window of the process `pid`, while it has one.
    fn window(&self, pid: u32) -> Option<GameWindow>;
    /// Whether `window` is the foreground window at this moment.
    fn is_foreground(&self, window: &GameWindow) -> bool;
    /// Asks Windows to bring `window` forward. Whether it worked is [`Game::is_foreground`]'s to say.
    fn bring_forward(&self, window: &GameWindow);
    /// Asks the window to close, as its own close button does.
    fn close(&self, window: &GameWindow) -> Result<(), Failure>;
    /// The window's own picture (never the desktop), at most `max_width` wide.
    fn capture(&self, window: &GameWindow, max_width: u32) -> Result<Shot, Failure>;
    fn osc(&self, address: &str, value: OscArg) -> Result<(), Failure>;
    /// Asks VRChat to wear an avatar, over OSC.
    fn wear(&self, avatar_id: &str) -> Result<(), Failure>;
    /// Where OSC goes, for the guard that releases input when this process dies.
    fn osc_target(&self) -> Option<std::net::SocketAddr>;
    /// Sends keyboard and mouse input to `window`, refused unless it is the foreground window at this very moment.
    fn inject(&self, window: &GameWindow, input: &[Input]) -> Result<(), Failure>;
    /// Sends a key or button's release wherever the focus is: a key left down is stuck for everything on this computer,
    /// which is worse than one stray key-up.
    fn release(&self, input: &[Input]);
    fn take_picture(&self, length: Photo3dLength, mode: PictureMode) -> Result<(), Failure>;
    /// Stops a recording under way (the person took control back).
    fn stop_picture(&self);
    fn picture_files(&self, session: &Path) -> PictureFiles;
    /// Holds reconstructions back while the game restarts between two worlds (they wait for VRChat as ever: without this,
    /// the moments it is closed would start one, which would then fight the next world for the graphics card).
    fn hold_reconstructions(&self, hold: bool);
    /// A few times a second, from the bridge's listener: a side session follows its own VRChat here.
    fn tick(&self) {}
    /// The app is lent to a side session's VRChat for its 3D picture: what the main session reads of the app is that
    /// VRChat's for a few seconds, and nothing of the main session's may go there.
    fn lent_away(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_goes_to_desktop_mode_in_a_window_into_the_instance() {
        let plan = LaunchPlan { world_id: "wrld_1".into(), instance: "123~private(usr_1)~region(eu)".into(), width: 1280, height: 720, profile: None, osc: None };
        assert_eq!(
            plan.args(),
            [
                "--no-vr",
                "-screen-fullscreen",
                "0",
                "-screen-width",
                "1280",
                "-screen-height",
                "720",
                "vrchat://launch?ref=orbiters.cc&id=wrld_1:123~private(usr_1)~region(eu)"
            ]
        );
        let args = LaunchPlan { profile: Some(3), ..plan.clone() }.args();
        assert!(args.contains(&"--profile=3".to_string()));
        // A side session: its own OSC ports, and the account's home when it only opens.
        let side = LaunchPlan { world_id: String::new(), instance: String::new(), profile: Some(1), osc: Some((9010, 9011)), ..plan }.args();
        assert_eq!(&side[side.len() - 2..], ["--profile=1", "--osc=9010:127.0.0.1:9011"]);
        assert!(!side.iter().any(|a| a.starts_with("vrchat://")));
    }
}
