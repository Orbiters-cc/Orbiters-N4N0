//! The device's music and volume, for N4N0's menu and N4N0's assistant (contracts `Docs/orb.md`, Menu): what plays (the
//! app that played last: Spotify, a browser, any app that tells the system), its art, and the speakers' volume. A
//! process reads them on a thread of its own, about once a second, only while something shows them
//! ([`Watcher::want`]), through its platform's [`MediaBackend`] (Windows' media controls on the desktop; on a phone,
//! Android's media sessions).
//!
//! A command changes what [`Watcher::reading`] returns at once (play becomes pause, the volume moves), then runs on the
//! thread; the next reading says what the app really did.

use orbiters_orb::menu::{Media, MediaCommand, Volume};
use orbiters_orb::picture::{self, Rgb};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a [`Watcher::want`] keeps the readings coming.
const WANT_FOR: Duration = Duration::from_secs(3);
/// Readings while wanted.
const EVERY: Duration = Duration::from_secs(1);
/// The art as both displays show it: whole, on the menu's card colour, 16:9.
pub const ART_WIDTH: usize = 320;
pub const ART_HEIGHT: usize = 180;
const ART_BACKGROUND: [u8; 3] = [0x10, 0x10, 0x12];

/// The art of what plays, under the key the menus keep its picture by.
#[derive(Debug)]
pub struct Art {
    pub key: String,
    pub rgb: Rgb,
}

/// What the last reading said.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    /// What plays (or played last): `None` when no app tells the system.
    pub media: Option<Media>,
    /// The speakers' volume, when there are speakers.
    pub volume: Option<Volume>,
    pub art: Option<Arc<Art>>,
}

/// How a platform reads and drives what plays. Made on the watcher's own thread, and used only there (Windows' media
/// controls want a COM apartment of their own).
pub trait MediaBackend {
    /// Asks the playing app or the speakers for `command`.
    fn run(&mut self, command: MediaCommand) -> Result<(), String>;
    /// What plays now, the volume and the art (`previous`: the last reading, whose art the same song keeps).
    fn read(&mut self, previous: &Reading) -> Reading;
}

/// No music to read (a device without, tests).
pub struct NoMedia;

impl MediaBackend for NoMedia {
    fn run(&mut self, _command: MediaCommand) -> Result<(), String> {
        Ok(())
    }

    fn read(&mut self, _previous: &Reading) -> Reading {
        Reading::default()
    }
}

/// Makes the backend, on the watcher's thread.
type MakeBackend = Box<dyn FnOnce() -> Box<dyn MediaBackend> + Send>;

#[derive(Default)]
struct State {
    reading: Reading,
    /// Bumped whenever the reading changes.
    version: u64,
    /// Readings made so far (changed or not).
    reads: u64,
    wanted_until: Option<Instant>,
    commands: VecDeque<MediaCommand>,
    started: bool,
}

/// A process's media watcher.
pub struct Watcher {
    state: Arc<(Mutex<State>, Condvar)>,
    /// Told after every reading.
    read: Arc<Condvar>,
    backend: Mutex<Option<MakeBackend>>,
}

impl Watcher {
    /// A watcher reading through the backend `make` makes (its thread starts at the first [`Watcher::want`] or command).
    pub fn new(make: impl FnOnce() -> Box<dyn MediaBackend> + Send + 'static) -> Self {
        Self { state: Arc::new((Mutex::new(State::default()), Condvar::new())), read: Arc::new(Condvar::new()), backend: Mutex::new(Some(Box::new(make))) }
    }

    /// Something shows the music: keep reading it for a few seconds.
    pub fn want(&self) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        let first = state.wanted_until.is_none_or(|t| t < Instant::now());
        state.wanted_until = Some(Instant::now() + WANT_FOR);
        self.start(&mut state);
        if first {
            wake.notify_one();
        }
    }

    /// A reading made after this call (the commands asked so far done), or the latest one after `timeout`.
    pub fn fresh(&self, timeout: Duration) -> Reading {
        self.want();
        let lock = &self.state.0;
        let until = Instant::now() + timeout;
        let mut state = lock.lock().unwrap();
        let after = state.reads;
        while state.reads == after {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self.read.wait_timeout(state, left).unwrap().0;
        }
        state.reading.clone()
    }

    /// A made-up reading in place of the device's (the screenshot review), before anything wanted one: this process
    /// never reads the device's then.
    pub fn stand_in(&self, reading: Reading) {
        let mut state = self.state.0.lock().unwrap();
        state.started = true;
        state.reading = reading;
        state.version += 1;
    }

    /// The latest reading and its version (it changes with the reading).
    pub fn reading(&self) -> (u64, Reading) {
        let state = self.state.0.lock().unwrap();
        (state.version, state.reading.clone())
    }

    /// Asks the playing app or the speakers for `command`; the reading shows its effect at once.
    pub fn command(&self, command: MediaCommand) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        let reading = &mut state.reading;
        match command {
            MediaCommand::PlayPause | MediaCommand::Play | MediaCommand::Pause => {
                if let Some(media) = reading.media.as_mut() {
                    media.playing = match command {
                        MediaCommand::Play => true,
                        MediaCommand::Pause => false,
                        _ => !media.playing,
                    };
                }
            }
            MediaCommand::Volume(percent) => reading.volume = Some(Volume { percent: percent.min(100), muted: false }),
            MediaCommand::Mute(muted) => {
                if let Some(volume) = reading.volume.as_mut() {
                    volume.muted = muted;
                }
            }
            MediaCommand::Previous | MediaCommand::Next => {}
        }
        state.version += 1;
        state.commands.push_back(command);
        state.wanted_until = Some(Instant::now() + WANT_FOR);
        self.start(&mut state);
        wake.notify_one();
    }

    fn start(&self, state: &mut State) {
        if std::mem::replace(&mut state.started, true) {
            return;
        }
        let Some(make) = self.backend.lock().unwrap().take() else { return };
        let (shared, read) = (self.state.clone(), self.read.clone());
        if let Err(e) = std::thread::Builder::new().name("orbiters-media".into()).spawn(move || run(shared, read, make)) {
            eprintln!("media: could not start its thread: {e}");
        }
    }
}

/// A reading after `command` (or none), until nothing wants readings: then it sleeps until something does.
fn run(shared: Arc<(Mutex<State>, Condvar)>, read: Arc<Condvar>, make: MakeBackend) {
    let (lock, wake) = &*shared;
    let mut backend = make();
    loop {
        let commands: Vec<MediaCommand> = {
            let mut state = lock.lock().unwrap();
            while state.commands.is_empty() && state.wanted_until.is_none_or(|t| t < Instant::now()) {
                state = wake.wait(state).unwrap();
            }
            state.commands.drain(..).collect()
        };
        for command in commands {
            if let Err(e) = backend.run(command) {
                eprintln!("media: {command:?}: {e}");
            }
        }
        let previous = lock.lock().unwrap().reading.clone();
        let reading = backend.read(&previous);
        {
            let mut state = lock.lock().unwrap();
            // A command that came meanwhile shows its own effect until its reading.
            if state.commands.is_empty() && !same(&state.reading, &reading) {
                state.reading = reading;
                state.version += 1;
            }
            state.reads += 1;
            read.notify_all();
        }
        let state = lock.lock().unwrap();
        if state.commands.is_empty() {
            let _ = wake.wait_timeout(state, EVERY).unwrap();
        }
    }
}

fn same(a: &Reading, b: &Reading) -> bool {
    a.media == b.media && a.volume == b.volume && a.art.as_ref().map(|a| &a.key) == b.art.as_ref().map(|b| &b.key)
}

/// The app's name from its id: `Spotify.exe` → Spotify, `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify` → Spotify,
/// `msedge` → Edge.
pub fn app_name(id: &str) -> String {
    let id = id.rsplit('!').next().unwrap_or(id);
    let id = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let stem = id.strip_suffix(".exe").or_else(|| id.strip_suffix(".EXE")).unwrap_or(id);
    match stem.to_ascii_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "chrome" => "Chrome".into(),
        "firefox" | "308046b0af4a39cb" => "Firefox".into(),
        "brave" => "Brave".into(),
        "opera" => "Opera".into(),
        "vlc" => "VLC".into(),
        "zunemusic" | "microsoft.zunemusic_8wekyb3d8bbwe" => "Media Player".into(),
        _ => {
            // A package's name: its last dotted part ("Microsoft.ZuneMusic" → ZuneMusic).
            let name = stem.split('_').next().unwrap_or(stem);
            let name = name.rsplit('.').next().unwrap_or(name);
            let mut chars = name.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    }
}

/// Art as the menus show it: whole on a 16:9 card, keyed by what it is of (`bytes`: the JPEG or PNG the app gave).
pub fn art(media: &Media, bytes: &[u8]) -> Option<Arc<Art>> {
    let rgb = picture::rgb_any(bytes).ok()?;
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes.iter().step_by((bytes.len() / 4096).max(1)).chain(media.title.as_bytes()) {
        hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    Some(Arc::new(Art { key: format!("art:{hash:016x}"), rgb: picture::letterbox(&rgb, ART_WIDTH, ART_HEIGHT, ART_BACKGROUND) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_apps_as_people_know_them() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(app_name("msedge"), "Edge");
        assert_eq!(app_name("Chrome"), "Chrome");
        assert_eq!(app_name("308046B0AF4A39CB"), "Firefox");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name("C:\\Program Files\\foobar2000\\foobar2000.exe"), "Foobar2000");
    }

    #[test]
    fn a_command_shows_at_once() {
        let watcher = Watcher::new(|| Box::new(NoMedia));
        {
            let mut state = watcher.state.0.lock().unwrap();
            state.reading.media = Some(Media { title: "Song".into(), playing: true, ..Default::default() });
            state.reading.volume = Some(Volume { percent: 30, muted: true });
            // No thread: these commands only change the reading.
            state.started = true;
        }
        let (before, _) = watcher.reading();
        watcher.command(MediaCommand::PlayPause);
        watcher.command(MediaCommand::Volume(55));
        let (after, reading) = watcher.reading();
        assert!(after > before);
        assert!(!reading.media.unwrap().playing);
        assert_eq!(reading.volume, Some(Volume { percent: 55, muted: false }));
    }

    /// A stand-in backend: what it was asked, and the song it says plays.
    struct Fake(Arc<Mutex<Vec<MediaCommand>>>);

    impl MediaBackend for Fake {
        fn run(&mut self, command: MediaCommand) -> Result<(), String> {
            self.0.lock().unwrap().push(command);
            Ok(())
        }

        fn read(&mut self, _previous: &Reading) -> Reading {
            let playing = !self.0.lock().unwrap().contains(&MediaCommand::Pause);
            Reading { media: Some(Media { title: "Fake".into(), playing, ..Default::default() }), volume: None, art: None }
        }
    }

    #[test]
    fn the_backend_runs_commands_on_its_thread_and_is_read_after() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let kept = asked.clone();
        let watcher = Watcher::new(move || Box::new(Fake(kept)));
        let first = watcher.fresh(Duration::from_secs(5));
        assert!(first.media.as_ref().is_some_and(|m| m.title == "Fake" && m.playing), "{first:?}");
        watcher.command(MediaCommand::Pause);
        assert!(!watcher.reading().1.media.unwrap().playing, "at once");
        let read = watcher.fresh(Duration::from_secs(5));
        assert!(!read.media.unwrap().playing, "the backend's own reading says so too");
        assert_eq!(asked.lock().unwrap().as_slice(), &[MediaCommand::Pause]);
    }
}
