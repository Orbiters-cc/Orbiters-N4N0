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
}

impl LaunchPlan {
    /// The arguments VRChat gets, each one checked against docs.vrchat.com/docs/launch-options and Unity's player command
    /// line: `--no-vr` for desktop mode, Unity's `-screen-*` for a window of a known size (so a screenshot's pixels are
    /// mouse coordinates), and the launch link of the instance. No `--osc`: VRChat's defaults are what the app's OSC finds
    /// through OSCQuery, and the option turns nothing on anyway.
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
        args.push(format!("vrchat://launch?ref=orbiters.cc&id={}:{}", self.world_id, self.instance));
        args
    }
}

/// VRChat as far as the tools care.
pub trait Game: Send + Sync {
    fn seen(&self) -> Seen;
    /// Whether VRChat's camera is open, `None` when VRChat does not say. Apart from [`Game::seen`] as it asks VRChat.
    fn camera_open(&self) -> Option<bool>;
    /// A new private instance of `world`, owned by the signed-in VRChat account. Only ever that kind: nothing here can
    /// name another instance, or make a public, friends or group one.
    fn create_private_instance(&self, world: &str, region: &str) -> Result<String, Failure>;
    fn find_worlds(&self, find: &Find, page: usize) -> Result<Vec<WorldCard>, Failure>;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_goes_to_desktop_mode_in_a_window_into_the_instance() {
        let plan = LaunchPlan { world_id: "wrld_1".into(), instance: "123~private(usr_1)~region(eu)".into(), width: 1280, height: 720, profile: None };
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
        let args = LaunchPlan { profile: Some(3), ..plan }.args();
        assert!(args.contains(&"--profile=3".to_string()));
    }
}
