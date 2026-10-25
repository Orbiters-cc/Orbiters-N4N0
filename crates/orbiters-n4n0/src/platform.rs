//! What N4N0 asks of the app and of the device it runs on, as small traits: the desktop implements them with Windows'
//! own (its controller, WASAPI, the clipboard, the voice worker's process), a phone with Android's.
//!
//! None of these is called while N4N0 holds a lock of its own, and nothing here may wait for N4N0: the app calls N4N0
//! from its own threads (the OSC receiving one among them) while N4N0 calls it from N4N0's.

use crate::status::{CallsStatus, NoticeKind, OrbStatus};
use orbiters_core::account::Api;
use orbiters_core::osc::OscValue;
use orbiters_core::settings::Settings;
use orbiters_core::voice::{VoiceCommand, VoiceEvent};
use orbiters_core::vrchat_log::WorldState;
use orbiters_orb::menu::KeyboardLayout;
use orbiters_orb::text::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

/// VRChat and SteamVR as the app sees them now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scene {
    /// VRChat runs (on a phone: the Quest's VRChat answers over the network).
    pub vrchat: bool,
    /// VRChat's process on this computer (desktop mode's clicks are read only while it is in front).
    pub vrchat_pid: Option<u32>,
    /// SteamVR runs: until VRChat says its mode, it is taken to run in VR.
    pub steamvr: bool,
}

/// What N4N0's menu, its triggers and its commands ask of the app's calls.
#[derive(Clone, Debug, PartialEq)]
pub enum CallRequest {
    /// Calls a friend (a contact id of [`CallsStatus`]).
    Contact(String),
    /// Answers the ringing call.
    Answer,
    /// Declines the ringing call or hangs up.
    End,
    Mute(bool),
    /// The avatar's call menu (`N4N0/Call/Menu`): the panel lists the friends, or no longer.
    Menu,
    /// `N4N0/Call/Go`: calls the friend picked, or ends the call.
    Go,
    /// `N4N0/Call/Pick`, the radial's value: which friend is picked.
    Pick(f32),
}

/// The app N4N0 runs in: its settings, what it knows of VRChat, where N4N0's state goes, the Orbiters account, the
/// order of what OSC messages ask for, and the features that are the app's own (calls, 3D pictures, the walk boost),
/// which an app without them leaves as they are.
pub trait Host: Send + Sync + 'static {
    /// Reads the settings where they are.
    fn with_settings(&self, read: &mut dyn FnMut(&Settings));
    /// Changes the settings in place; the app publishes and saves them.
    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings));
    /// VRChat and SteamVR now.
    fn scene(&self) -> Scene;
    /// The world VRChat is in, as its log says (a phone has no log: nothing known).
    fn world(&self) -> WorldState {
        WorldState::default()
    }
    /// A recording uses VRChat's camera now (a picture would stop it).
    fn capturing(&self) -> bool {
        false
    }
    /// N4N0's state changed: what the interfaces show. The app adds what it keeps itself (its calls, the avatar's name).
    fn publish(&self, status: OrbStatus);
    /// Something to tell the person (a toast).
    fn notify(&self, kind: NoticeKind, message: String);
    /// The Orbiters account the assistant asks with and link cards are dropped with: its API and credential.
    fn account(&self) -> Option<(Api, String)>;
    /// Runs `action` after what OSC messages asked for before it, off the OSC receiving thread ([`Actions`]). False when
    /// it was dropped (too many wait already).
    fn queue(&self, action: Box<dyn FnOnce() + Send>) -> bool;
    /// The app's calls with friends, as they stand.
    fn calls(&self) -> CallsStatus {
        CallsStatus::default()
    }
    /// A friend's call rings here (touching N4N0 answers it).
    fn ringing_in(&self) -> bool {
        false
    }
    fn call(&self, _request: CallRequest) {}
    /// A 3D picture where VRChat's camera stands.
    fn photo_3d(&self) -> anyhow::Result<()> {
        anyhow::bail!("3D pictures are taken by Orbiters - N4N0 on the computer that runs VRChat.")
    }
    /// The avatar's Walk boost radial moved (`N4N0/WalkBoost`): the strength, 0 to 1.
    fn walk_boost(&self, _strength: f32) {}
    /// VRChat's `InStation`, `Seated` or `VelocityMagnitude` (on the OSC receiving thread); `None`: a new avatar, which
    /// has not said how it moves yet.
    fn motion(&self, _said: Option<(&str, &OscValue)>) {}
}

/// OSC with VRChat as the app runs it (`orbiters_core::osc::OscService`: on this computer's loopback for the desktop,
/// over the Wi-Fi to a Quest for a phone). The app keeps what VRChat sent ([`orbiters_core::osc::Latest`]) and passes
/// N4N0 the avatar's parameters and changes ([`crate::N4n0::on_parameter`], [`crate::N4n0::on_avatar`]).
pub trait Osc: Send + Sync + 'static {
    /// Sends each of `bundles` to VRChat in one datagram, applied together in its order (a lone message as itself).
    /// `None` while OSC is off; else how each went.
    fn send(&self, bundles: &[&[Message]]) -> Option<Vec<anyhow::Result<()>>>;
    /// VRChat's OSCQuery service: `None` while OSC is off, `Some(None)` until it was found.
    fn vrchat_query(&self) -> Option<Option<SocketAddr>>;
    /// The latest value VRChat sent for each of `addresses` it sent one for (its first argument).
    fn latest(&self, addresses: &mut dyn Iterator<Item = String>) -> HashMap<String, OscValue>;
    /// VRChat's current values (its OSCQuery tree: the avatar and its parameters, the camera), empty when it cannot
    /// tell. Never waits for the OSC service's lock.
    fn vrchat_values(&self) -> Vec<(String, OscValue)>;
}

/// The device's clipboard. Tests keep what they copy instead.
pub trait Clipboard: Send + Sync {
    fn copy(&self, text: &str) -> Result<(), String>;
}

/// Opens a web page in the person's browser (only pages of the Orbiters website reach it).
pub trait Browser: Send + Sync {
    fn open(&self, url: &str) -> anyhow::Result<()>;
}

/// The sounds N4N0 plays itself: the press sound as a copy's feedback (the person may be in VR, where no toast shows).
pub trait Sounds: Send + Sync {
    fn press(&self);
}

/// The person's language and keyboard, as the device says.
pub trait Locale: Send + Sync {
    /// The language and region (`fr-FR`), for the assistant.
    fn locale(&self) -> String;
    /// The ISO 639-1 language (`fr`) YouTube is asked in.
    fn language(&self) -> String {
        language_of(&self.locale())
    }
    /// The keyboard's letters for N4N0's menu when its setting is Auto.
    fn keyboard(&self) -> KeyboardLayout {
        KeyboardLayout::Qwerty
    }
}

/// The two letters of a locale (`fr-FR`, `fr_FR.UTF-8` → `fr`), English when it has none.
pub fn language_of(locale: &str) -> String {
    let code = locale.split(['-', '_', '.']).next().unwrap_or("").to_lowercase();
    if code.len() == 2 && code.bytes().all(|b| b.is_ascii_lowercase()) {
        code
    } else {
        "en".into()
    }
}

/// What a voice worker says (each [`VoiceEvent`] as it comes), and the call made once it ended (on its own or not).
pub type VoiceEvents = Box<dyn Fn(VoiceEvent) + Send + Sync>;
pub type VoiceEnded = Box<dyn FnOnce() + Send>;

/// N4N0's ears (protocol `orbiters_core::voice`): a worker that listens to the microphone and transcribes on the
/// device. The desktop runs `orbiters-voice` as a process below normal priority; a phone runs it as a thread.
pub trait Ears: Send + Sync {
    /// Starts a worker. It never calls `events` or `ended` before this returns.
    fn spawn(&self, events: VoiceEvents, ended: VoiceEnded) -> anyhow::Result<Box<dyn VoiceWorker>>;
}

/// A running voice worker. Dropping it stops it (a [`VoiceCommand::Quit`], then by force a moment later).
pub trait VoiceWorker: Send {
    fn send(&mut self, command: &VoiceCommand) -> anyhow::Result<()>;
    /// How it ended, once it did (waiting a moment for it): its exit code.
    fn exit_code(&mut self) -> Option<i32>;
    /// The last thing it said of itself (what it was doing when it stopped).
    fn last_line(&self) -> Option<String>;
}

/// The mouse at a reading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseState {
    /// The left button is down.
    pub down: bool,
    /// The window in front is VRChat's.
    pub front: bool,
    /// VRChat shows the cursor: its Action Menu or Quick Menu is open, the view frozen (in mouse-look it hides it). Read
    /// only with VRChat in front.
    pub cursor: bool,
}

/// The left mouse button, the window in front and the cursor, as VRChat's desktop mode's clicks read them on N4N0's
/// screen (a device without desktop mode has none: [`NoMouse`]).
pub trait Mouse: Send {
    /// The mouse now, VRChat being process `vrchat`.
    fn read(&mut self, vrchat: u32) -> MouseState;
}

/// No mouse: nothing is ever pressed.
pub struct NoMouse;

impl Mouse for NoMouse {
    fn read(&mut self, _vrchat: u32) -> MouseState {
        MouseState::default()
    }
}

/// The device's own, for N4N0.
pub struct Platform {
    pub clipboard: Box<dyn Clipboard>,
    pub browser: Box<dyn Browser>,
    pub sounds: Box<dyn Sounds>,
    pub locale: Box<dyn Locale>,
    pub ears: Box<dyn Ears>,
    /// The music playing and the volume ([`crate::media::Watcher`], with the device's backend).
    pub media: Arc<crate::media::Watcher>,
    /// Desktop mode's mouse (`None`: none, as on a phone).
    pub mouse: Option<Box<dyn Mouse>>,
}

/// Where N4N0 keeps its files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    /// The saved videos (`orb/videos.json` in the config folder).
    pub videos: PathBuf,
    /// The avatars' saved states (`avatar-states.json` beside the settings).
    pub states: PathBuf,
    /// The avatar manifests Orbiters Toolkit writes.
    pub avatars: PathBuf,
    /// The speech models.
    pub models: PathBuf,
}

impl Files {
    /// The app's own, in the folders `orbiters_core::paths` gives (the ones a phone app installed with
    /// `orbiters_core::paths::set_roots`, else the platform's).
    pub fn app() -> Self {
        Self {
            videos: orbiters_orb::videos::file(),
            states: orbiters_orb::states::file(),
            avatars: orbiters_core::paths::avatars_dir(),
            models: orbiters_core::paths::models_dir(),
        }
    }

    /// All of them in `dir` (tests).
    pub fn in_dir(dir: &Path) -> Self {
        Self { videos: dir.join("videos.json"), states: dir.join("avatar-states.json"), avatars: dir.join("Avatars"), models: dir.join("Models") }
    }
}

/// Runs what OSC messages ask for one after the other, on a thread of its own started on first use, so the OSC
/// receiving thread keeps reading (face tracking sends hundreds of values a second into a small socket buffer) while
/// what a message starts takes a while (a capture, N4N0's ears). Past [`Actions::MOST`] waiting, more are dropped
/// rather than kept without end (a flood of messages).
#[derive(Default)]
pub struct Actions {
    sender: Mutex<Option<mpsc::SyncSender<Action>>>,
}

/// What an OSC message asks for.
type Action = Box<dyn FnOnce() + Send>;

impl Actions {
    pub const MOST: usize = 64;

    /// Runs `action` after the ones before it; false when it was dropped.
    pub fn run(&self, action: Action) -> bool {
        let mut sender = self.sender.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Action>(Self::MOST);
            let spawned = std::thread::Builder::new().name("osc-actions".into()).spawn(move || {
                for action in rx {
                    action();
                }
            });
            match spawned {
                Ok(_) => *sender = Some(tx),
                Err(e) => {
                    eprintln!("OSC actions: {e}");
                    drop(sender);
                    action();
                    return true;
                }
            }
        }
        match sender.as_ref().map(|tx| tx.try_send(action)) {
            Some(Ok(())) => true,
            Some(Err(mpsc::TrySendError::Full(_))) => {
                eprintln!("OSC actions: {} are waiting already, one more is dropped", Self::MOST);
                false
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_are_two_letters() {
        assert_eq!(language_of("fr-FR"), "fr");
        assert_eq!(language_of("fr_FR.UTF-8"), "fr");
        assert_eq!(language_of("pt"), "pt");
        assert_eq!(language_of(""), "en");
        assert_eq!(language_of("C.UTF-8"), "en");
        assert_eq!(language_of("123-x"), "en");
    }

    #[test]
    fn actions_run_in_order_and_a_flood_is_cut() {
        let actions = Actions::default();
        let (done_tx, done_rx) = mpsc::channel();
        // The first holds the thread until the test lets it go: the others wait behind it, in order.
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (started_tx, started_rx) = mpsc::channel::<()>();
        assert!(actions.run(Box::new(move || {
            started_tx.send(()).unwrap();
            let _ = go_rx.recv();
        })));
        started_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        for i in 0..Actions::MOST {
            let tx = done_tx.clone();
            assert!(actions.run(Box::new(move || tx.send(i).unwrap())), "{i}");
        }
        assert!(!actions.run(Box::new(|| {})), "one more than may wait is dropped");
        go_tx.send(()).unwrap();
        let order: Vec<usize> = (0..Actions::MOST).map(|_| done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).collect();
        assert_eq!(order, (0..Actions::MOST).collect::<Vec<_>>());
    }

    #[test]
    fn the_app_s_files_and_a_test_s() {
        let dir = Path::new("x");
        let files = Files::in_dir(dir);
        assert_eq!(files.states, dir.join("avatar-states.json"));
        let app = Files::app();
        assert!(app.states.ends_with("avatar-states.json") && app.videos.ends_with(Path::new("orb").join("videos.json")));
        assert!(app.avatars.is_absolute() && app.models.is_absolute());
    }
}
