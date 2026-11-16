//! The device's music and volume, for N4N0's menu and N4N0's assistant (contracts `Docs/n4n0.md`, Menu): what plays (the
//! app that played last: Spotify, a browser, any app that tells the system), its art, and the speakers' volume. A
//! process reads them on a thread of its own, about once a second, only while something shows them
//! ([`Watcher::want`]), through its platform's [`MediaBackend`] (Windows' media controls on the desktop; on a phone,
//! Android's media sessions).
//!
//! A command changes what [`Watcher::reading`] returns at once (play becomes pause, the volume moves), then runs on the
//! thread; the next reading says what the app really did.
//!
//! What a browser or YouTube's app plays is looked up on YouTube ([`Links`]): the media controls give no link, and the
//! menus offer the video's link to copy and drop as they do a search result's.

use orbiters_n4n0_core::menu::{AppSound, Media, MediaCommand, Volume};
use orbiters_n4n0_core::picture::{self, Rgb};
use orbiters_n4n0_core::youtube::{self, Video};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub mod app_sound;

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

/// How a playing video is searched for: its title and channel (the app's artist) to YouTube's results.
pub type Search = Arc<dyn Fn(&str, &str) -> anyhow::Result<Vec<Video>> + Send + Sync>;

/// A search that failed is tried again after this.
const SEARCH_RETRY: Duration = Duration::from_secs(30);
/// Searches kept (the songs of an evening); past that, the oldest are forgotten.
const SEARCHES_KEPT: usize = 64;

enum Lookup {
    Asked,
    Found(Vec<Video>),
    Failed(Instant),
}

/// A playing video: its title and channel.
type Playing = (String, String);

/// The searches made, and their order (the oldest first, forgotten first).
#[derive(Default)]
struct Searches {
    lookups: HashMap<Playing, Lookup>,
    order: VecDeque<Playing>,
}

/// Finds the YouTube video a browser or YouTube's app plays ([`youtube::worth_looking_up`]): Windows' media controls
/// (and Android's) give its title, its channel as the artist and its length, never its link. YouTube's search for the
/// title and channel, once per video on a thread of its own, then the result with the same title, channel and length
/// ([`youtube::find_playing`]). The results stay, so a length that changes (an ad ends, the video starts) is matched
/// again without another search.
#[derive(Clone)]
pub struct Links {
    search: Search,
    found: Arc<Mutex<Searches>>,
}

impl Links {
    /// Links found with `search`.
    pub fn new(search: Search) -> Self {
        Self { search, found: Arc::default() }
    }

    /// Links found on YouTube itself, without an account ([`youtube::search_playing`]).
    pub fn youtube() -> Self {
        Self::new(Arc::new(youtube::search_playing))
    }

    /// Sets `media`'s video (its id) when it is found, or asks for it.
    pub fn fill(&self, media: &mut Media) {
        media.video = None;
        if !youtube::worth_looking_up(&media.app, &media.title, &media.artist, &media.album) {
            return;
        }
        let key = (media.title.clone(), media.artist.clone());
        let mut found = self.found.lock().unwrap();
        match found.lookups.get(&key) {
            Some(Lookup::Found(results)) => {
                media.video = youtube::find_playing(results, &media.title, &media.artist, media.duration).map(|v| v.id.clone());
                return;
            }
            Some(Lookup::Asked) => return,
            Some(Lookup::Failed(at)) if at.elapsed() < SEARCH_RETRY => return,
            Some(Lookup::Failed(_)) => {}
            None => {
                found.order.push_back(key.clone());
                while found.order.len() > SEARCHES_KEPT {
                    if let Some(old) = found.order.pop_front() {
                        found.lookups.remove(&old);
                    }
                }
            }
        }
        found.lookups.insert(key.clone(), Lookup::Asked);
        drop(found);
        let (search, kept) = (self.search.clone(), self.found.clone());
        let spawned = std::thread::Builder::new().name("orbiters-media-link".into()).spawn(move || {
            let lookup = match search(&key.0, &key.1) {
                Ok(results) => Lookup::Found(results),
                Err(e) => {
                    eprintln!("media: YouTube's search for what plays: {e:#}");
                    Lookup::Failed(Instant::now())
                }
            };
            let mut found = kept.lock().unwrap();
            // Kept only while still asked for (not forgotten meanwhile).
            if let Some(slot) = found.lookups.get_mut(&key) {
                *slot = lookup;
            }
        });
        if spawned.is_err() {
            self.found.lock().unwrap().lookups.insert((media.title.clone(), media.artist.clone()), Lookup::Failed(Instant::now()));
        }
    }
}

/// A process's media watcher.
pub struct Watcher {
    state: Arc<(Mutex<State>, Condvar)>,
    /// Told after every reading.
    read: Arc<Condvar>,
    backend: Mutex<Option<MakeBackend>>,
    links: Links,
}

impl Watcher {
    /// A watcher reading through the backend `make` makes (its thread starts at the first [`Watcher::want`] or command),
    /// looking up on YouTube what a browser or YouTube's app plays.
    pub fn new(make: impl FnOnce() -> Box<dyn MediaBackend> + Send + 'static) -> Self {
        Self::with_links(make, Links::youtube())
    }

    /// A watcher finding the videos played with `links`.
    pub fn with_links(make: impl FnOnce() -> Box<dyn MediaBackend> + Send + 'static, links: Links) -> Self {
        Self {
            state: Arc::new((Mutex::new(State::default()), Condvar::new())),
            read: Arc::new(Condvar::new()),
            backend: Mutex::new(Some(Box::new(make))),
            links,
        }
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
            // The app's own volume, where it has one (the next reading says what the mixer has).
            MediaCommand::AppVolume(percent) => {
                if let Some(media) = reading.media.as_mut().filter(|m| matches!(m.sound, AppSound::Level(_))) {
                    media.sound = AppSound::Level(Volume { percent: percent.min(100), muted: false });
                }
            }
            MediaCommand::AppMute(muted) => {
                if let Some(AppSound::Level(volume)) = reading.media.as_mut().map(|m| &mut m.sound) {
                    volume.muted = muted;
                }
            }
            // Shown where it goes at once (the next reading says where the app took it).
            MediaCommand::Seek(at) => {
                if let Some(media) = reading.media.as_mut() {
                    media.position = Some(media.duration.map_or(at, |d| at.min(d)));
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
        let (shared, read, links) = (self.state.clone(), self.read.clone(), self.links.clone());
        if let Err(e) = std::thread::Builder::new().name("orbiters-media".into()).spawn(move || run(shared, read, make, links)) {
            eprintln!("media: could not start its thread: {e}");
        }
    }
}

/// A reading after `command` (or none), until nothing wants readings: then it sleeps until something does.
fn run(shared: Arc<(Mutex<State>, Condvar)>, read: Arc<Condvar>, make: MakeBackend, links: Links) {
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
        let mut reading = backend.read(&previous);
        if let Some(media) = reading.media.as_mut() {
            links.fill(media);
        }
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
/// `msedge` → Edge; an installed web app by its name when known (`Chrome._crx_agimnkijcaahngcdmfeangaknmldooml` →
/// YouTube), else by its browser's.
pub fn app_name(id: &str) -> String {
    let id = id.rsplit('!').next().unwrap_or(id);
    let id = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let stem = id.strip_suffix(".exe").or_else(|| id.strip_suffix(".EXE")).unwrap_or(id);
    if let Some((browser, app)) = stem.split_once("._crx_") {
        // Chrome's and Edge's ids of the web apps installed from YouTube's and YouTube Music's sites.
        return match app.get(..32).unwrap_or(app) {
            "agimnkijcaahngcdmfeangaknmldooml" => "YouTube".into(),
            "cinhimbnkkaeohfgghhklpknlkffjgod" => "YouTube Music".into(),
            _ => app_name(browser),
        };
    }
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
        assert_eq!(app_name("Chrome._crx_agimnkijcaahngcdmfeangaknmldooml"), "YouTube");
        assert_eq!(app_name("MSEdge._crx_cinhimbnkkaeohfgghhklpknlkffjgod"), "YouTube Music");
        assert_eq!(app_name("MSEdge._crx_abcdefghijklmnopabcdefghijklmnop"), "Edge", "another web app: its browser");
    }

    /// Media as Chrome gives it on YouTube's page, or Spotify's.
    struct Playing(&'static str);

    impl MediaBackend for Playing {
        fn run(&mut self, _command: MediaCommand) -> Result<(), String> {
            Ok(())
        }

        fn read(&mut self, _previous: &Reading) -> Reading {
            let media =
                Media { title: "Never Gonna Give You Up".into(), artist: "Rick Astley".into(), app: self.0.into(), duration: Some(213), ..Default::default() };
            Reading { media: Some(media), volume: None, art: None }
        }
    }

    #[test]
    fn what_a_browser_plays_is_found_on_youtube_once() {
        let searched = Arc::new(Mutex::new(Vec::new()));
        let asked = searched.clone();
        let search: Search = Arc::new(move |title: &str, channel: &str| {
            asked.lock().unwrap().push(format!("{title} | {channel}"));
            let video = |id: &str, length: &str| Video {
                id: id.into(),
                title: "Never Gonna Give You Up".into(),
                channel: "Rick Astley".into(),
                duration: Some(length.into()),
                live: false,
            };
            Ok(vec![video("aaaaaaaaaaa", "4:40"), video("dQw4w9WgXcQ", "3:34")])
        });
        let watcher = Watcher::with_links(|| Box::new(Playing("Chrome")), Links::new(search.clone()));
        let mut video = None;
        for _ in 0..50 {
            video = watcher.fresh(Duration::from_secs(5)).media.and_then(|m| m.video);
            if video.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(video.as_deref(), Some("dQw4w9WgXcQ"), "the result of its length");
        watcher.fresh(Duration::from_secs(5));
        assert_eq!(searched.lock().unwrap().as_slice(), &["Never Gonna Give You Up | Rick Astley".to_string()], "searched once");
        // Spotify's songs are not looked for.
        let spotify = Watcher::with_links(|| Box::new(Playing("Spotify")), Links::new(search));
        assert_eq!(spotify.fresh(Duration::from_secs(5)).media.map(|m| m.video), Some(None));
        assert_eq!(searched.lock().unwrap().len(), 1);
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
        watcher.command(MediaCommand::Seek(42));
        let (after, reading) = watcher.reading();
        assert!(after > before);
        assert_eq!(reading.media.as_ref().unwrap().position, Some(42), "moved in its timeline at once");
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

    /// A mixer the test changes from outside while the watcher reads it.
    struct Shared(Arc<Mutex<app_sound::StandIn>>);

    impl app_sound::Mixer for Shared {
        fn sounds(&mut self) -> Vec<app_sound::Sound> {
            self.0.lock().unwrap().sounds()
        }
        fn processes(&mut self) -> Vec<app_sound::Process> {
            self.0.lock().unwrap().processes()
        }
        fn windows_of(&mut self, id: &str) -> Vec<u32> {
            self.0.lock().unwrap().windows_of(id)
        }
        fn set(&mut self, index: usize, level: Option<f32>, muted: Option<bool>) -> Result<(), String> {
            self.0.lock().unwrap().set(index, level, muted)
        }
    }

    /// A browser playing through a backend as Windows' is: the app's own volume read from the mixer, the computer's
    /// kept apart.
    struct Browser {
        apps: app_sound::AppVolume<Shared>,
        computer: Arc<Mutex<Volume>>,
    }

    impl MediaBackend for Browser {
        fn run(&mut self, command: MediaCommand) -> Result<(), String> {
            match command {
                MediaCommand::AppVolume(percent) => self.apps.set_level("Chrome", percent),
                MediaCommand::AppMute(muted) => self.apps.set_muted("Chrome", muted),
                MediaCommand::Volume(percent) => {
                    *self.computer.lock().unwrap() = Volume { percent, muted: false };
                    Ok(())
                }
                _ => Ok(()),
            }
        }

        fn read(&mut self, _previous: &Reading) -> Reading {
            let media = Media { title: "A video".into(), app: "Chrome".into(), playing: true, sound: self.apps.read("Chrome"), ..Default::default() };
            Reading { media: Some(media), volume: Some(*self.computer.lock().unwrap()), art: None }
        }
    }

    /// Enzo, 2026-11-16: the card's volume is the playing app's. Pressed in the menu, it reaches the app's sound in
    /// the mixer and not the computer's volume nor VRChat's sound; changed in the mixer, the next reading follows;
    /// gone from the mixer, it shows as not found and sets nothing.
    #[test]
    fn the_card_volume_sets_the_playing_app_and_follows_the_mixer() {
        use app_sound::{process, sound, StandIn};
        use orbiters_n4n0_core::menu::{AppSound, Effect, Menu, Target};
        let mixer = Arc::new(Mutex::new(StandIn {
            processes: vec![process(100, 0, "explorer.exe", 1), process(200, 100, "chrome.exe", 2), process(210, 200, "chrome.exe", 3), process(300, 100, "VRChat.exe", 4)],
            sounds: vec![sound("Headset", 210, 0.5, true), sound("Headset", 300, 0.9, true)],
            ..Default::default()
        }));
        let computer = Arc::new(Mutex::new(Volume { percent: 80, muted: false }));
        let (m, c) = (Shared(mixer.clone()), computer.clone());
        let watcher = Watcher::with_links(move || Box::new(Browser { apps: app_sound::AppVolume::new(m), computer: c }), Links::new(Arc::new(|_: &str, _: &str| Ok(Vec::new()))));
        let mut menu = Menu::default();
        menu.open();
        let reading = watcher.fresh(Duration::from_secs(5));
        menu.media = reading.media.clone();
        menu.volume = reading.volume;
        assert_eq!(menu.media.as_ref().unwrap().sound, AppSound::Level(Volume { percent: 50, muted: false }));
        assert_eq!(menu.press(Target::VolumeUp), vec![Effect::Media(MediaCommand::AppVolume(55))]);
        watcher.command(MediaCommand::AppVolume(55));
        let reading = watcher.fresh(Duration::from_secs(5));
        assert_eq!(reading.media.as_ref().unwrap().sound, AppSound::Level(Volume { percent: 55, muted: false }));
        assert_eq!(mixer.lock().unwrap().sounds[0].level, 0.55);
        assert_eq!(mixer.lock().unwrap().sounds[1].level, 0.9, "VRChat's sound untouched");
        assert_eq!(*computer.lock().unwrap(), Volume { percent: 80, muted: false }, "the computer's volume untouched");
        // Moved in Windows' mixer.
        mixer.lock().unwrap().sounds[0].level = 0.2;
        mixer.lock().unwrap().sounds[0].muted = true;
        assert_eq!(watcher.fresh(Duration::from_secs(5)).media.unwrap().sound, AppSound::Level(Volume { percent: 20, muted: true }));
        // The tab let its sound go.
        mixer.lock().unwrap().sounds.remove(0);
        menu.media = watcher.fresh(Duration::from_secs(5)).media;
        assert_eq!(menu.media.as_ref().unwrap().sound, AppSound::NotFound);
        assert!(menu.press(Target::VolumeUp).is_empty() && menu.press(Target::Mute).is_empty(), "nothing to set, and not the computer's");
        assert_eq!(*computer.lock().unwrap(), Volume { percent: 80, muted: false });
    }
}
