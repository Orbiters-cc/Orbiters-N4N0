//! The computer's music and volume, for Orb's menu and Orb's assistant (contracts `Docs/orb.md`, Menu): what Windows'
//! media controls say plays (the app that played last: Spotify, a browser, any app that tells Windows), its art, and the
//! default speakers' volume. A process reads them on a thread of its own, about once a second, only while something
//! shows them ([`Watcher::want`]); the SteamVR overlay and the tray service each have theirs.
//!
//! A command changes what [`Watcher::reading`] returns at once (play becomes pause, the volume moves), then runs on the
//! thread; the next reading says what the app really did.

use orbiters_orb::menu::{Media, MediaCommand, Volume};
use orbiters_orb::picture::{self, Rgb};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a [`Watcher::want`] keeps the readings coming.
const WANT_FOR: Duration = Duration::from_secs(3);
/// Readings while wanted.
const EVERY: Duration = Duration::from_secs(1);
/// The art as both displays show it: whole, on the menu's card colour, 16:9.
pub const ART_WIDTH: usize = 320;
pub const ART_HEIGHT: usize = 180;
const ART_BACKGROUND: [u8; 3] = [0x10, 0x10, 0x12];
/// Art larger than this is not read.
const ART_MAX_BYTES: u64 = 4 << 20;

/// The art of what plays, under the key the menus keep its picture by.
#[derive(Debug)]
pub struct Art {
    pub key: String,
    pub rgb: Rgb,
}

/// What the last reading said.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    /// What plays (or played last): `None` when no app tells Windows.
    pub media: Option<Media>,
    /// The default speakers' volume, when there are speakers.
    pub volume: Option<Volume>,
    pub art: Option<Arc<Art>>,
}

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

/// This process's media watcher.
pub struct Watcher {
    state: Arc<(Mutex<State>, Condvar)>,
    /// Told after every reading.
    read: Arc<Condvar>,
}

/// The process's watcher (its thread starts at the first [`Watcher::want`] or command).
pub fn watcher() -> &'static Watcher {
    static WATCHER: OnceLock<Watcher> = OnceLock::new();
    WATCHER.get_or_init(|| Watcher { state: Arc::new((Mutex::new(State::default()), Condvar::new())), read: Arc::new(Condvar::new()) })
}

impl Watcher {
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
        let (shared, read) = (self.state.clone(), self.read.clone());
        if let Err(e) = std::thread::Builder::new().name("orbiters-media".into()).spawn(move || run(shared, read)) {
            eprintln!("media: could not start its thread: {e}");
        }
    }
}

/// A reading after `command` (or none), until nothing wants readings: then it sleeps until something does.
fn run(shared: Arc<(Mutex<State>, Condvar)>, read: Arc<Condvar>) {
    let (lock, wake) = &*shared;
    let mut windows = platform::Media::new();
    loop {
        let commands: Vec<MediaCommand> = {
            let mut state = lock.lock().unwrap();
            while state.commands.is_empty() && state.wanted_until.is_none_or(|t| t < Instant::now()) {
                state = wake.wait(state).unwrap();
            }
            state.commands.drain(..).collect()
        };
        for command in commands {
            if let Err(e) = windows.run(command) {
                eprintln!("media: {command:?}: {e}");
            }
        }
        let previous = lock.lock().unwrap().reading.clone();
        let reading = windows.read(&previous);
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

/// Art as the menus show it: whole on a 16:9 card, keyed by what it is of.
fn art(media: &Media, bytes: &[u8]) -> Option<Arc<Art>> {
    let rgb = picture::rgb_any(bytes).ok()?;
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes.iter().step_by((bytes.len() / 4096).max(1)).chain(media.title.as_bytes()) {
        hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    Some(Arc::new(Art { key: format!("art:{hash:016x}"), rgb: picture::letterbox(&rgb, ART_WIDTH, ART_HEIGHT, ART_BACKGROUND) }))
}

#[cfg(windows)]
mod platform {
    use super::{art, MediaCommand, Reading, ART_MAX_BYTES};
    use orbiters_orb::menu::{Media as Playing, Volume};
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::{Buffer, DataReader, InputStreamOptions};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    /// 100 ns ticks between 1601 (Windows' epoch) and 1970.
    const EPOCH_GAP_TICKS: i64 = 11_644_473_600 * 10_000_000;

    pub struct Media {
        manager: Option<Manager>,
        /// What the last art was read for (the same song keeps its art), and how many times it was asked for (apps
        /// often give a new song's art a moment after its title).
        art_for: Option<(String, String, String)>,
        art_tries: u8,
    }

    /// Times a song's art is asked for before it goes without.
    const ART_TRIES: u8 = 3;

    impl Media {
        pub fn new() -> Self {
            // SAFETY: this thread's own COM apartment, multithreaded, for the media controls and the speakers.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            Self { manager: None, art_for: None, art_tries: 0 }
        }

        fn session(&mut self) -> Option<Session> {
            if self.manager.is_none() {
                self.manager = Manager::RequestAsync().and_then(|op| op.join()).ok();
            }
            self.manager.as_ref()?.GetCurrentSession().ok()
        }

        pub fn run(&mut self, command: MediaCommand) -> windows::core::Result<()> {
            match command {
                MediaCommand::Volume(percent) => {
                    let speakers = speakers()?;
                    // SAFETY: plain calls on the default speakers' volume; no event context.
                    unsafe {
                        speakers.SetMute(false, std::ptr::null())?;
                        speakers.SetMasterVolumeLevelScalar(percent.min(100) as f32 / 100.0, std::ptr::null())
                    }
                }
                // SAFETY: as above.
                MediaCommand::Mute(muted) => unsafe { speakers()?.SetMute(muted, std::ptr::null()) },
                _ => {
                    let Some(session) = self.session() else { return Ok(()) };
                    let done = match command {
                        MediaCommand::PlayPause => session.TryTogglePlayPauseAsync()?,
                        MediaCommand::Play => session.TryPlayAsync()?,
                        MediaCommand::Pause => session.TryPauseAsync()?,
                        MediaCommand::Next => session.TrySkipNextAsync()?,
                        _ => session.TrySkipPreviousAsync()?,
                    };
                    done.join().map(|_| ())
                }
            }
        }

        /// What plays now, the volume and the art (read again only for another song).
        pub fn read(&mut self, previous: &Reading) -> Reading {
            let volume = speakers().ok().and_then(|s| {
                // SAFETY: reads of the default speakers' volume.
                let (level, muted) = unsafe { (s.GetMasterVolumeLevelScalar().ok()?, s.GetMute().ok()?.as_bool()) };
                Some(Volume { percent: (level * 100.0).round().clamp(0.0, 100.0) as u8, muted })
            });
            let Some(session) = self.session() else {
                self.art_for = None;
                return Reading { media: None, volume, art: None };
            };
            let media = playing(&session);
            let key = media.as_ref().map(|m| (m.title.clone(), m.artist.clone(), m.album.clone()));
            let changed = key != self.art_for;
            if changed {
                self.art_for = key;
                self.art_tries = 0;
            }
            let art = match &media {
                Some(_) if !changed && previous.art.is_some() => previous.art.clone(),
                Some(m) if self.art_tries < ART_TRIES => {
                    self.art_tries += 1;
                    thumbnail(&session).and_then(|bytes| art(m, &bytes))
                }
                _ => None,
            };
            let media = media.map(|mut m| {
                m.art = art.as_ref().map(|a| a.key.clone());
                m
            });
            Reading { media, volume, art }
        }
    }

    fn speakers() -> windows::core::Result<IAudioEndpointVolume> {
        // SAFETY: COM calls on this thread's apartment; the enumerator and device are released when dropped.
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?.Activate(CLSCTX_ALL, None)
        }
    }

    fn playing(session: &Session) -> Option<Playing> {
        let properties = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let text = |value: windows::core::Result<windows::core::HSTRING>| value.map(|s| s.to_string_lossy().trim().to_string()).unwrap_or_default();
        let info = session.GetPlaybackInfo().ok()?;
        let controls = info.Controls().ok()?;
        let flag = |value: windows::core::Result<bool>| value.unwrap_or(false);
        let playing = info.PlaybackStatus().ok() == Some(Status::Playing);
        let (mut position, mut duration) = (None, None);
        if let Ok(timeline) = session.GetTimelineProperties() {
            let (start, end) = (timeline.StartTime().map(|t| t.Duration).unwrap_or(0), timeline.EndTime().map(|t| t.Duration).unwrap_or(0));
            if end > start {
                let mut at = timeline.Position().map(|t| t.Duration).unwrap_or(0) - start;
                // The app says where it was when it last told Windows: it played on since.
                if playing {
                    if let (Ok(updated), Ok(now)) = (timeline.LastUpdatedTime(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)) {
                        let now = now.as_nanos() as i64 / 100 + EPOCH_GAP_TICKS;
                        if updated.UniversalTime > 0 && now > updated.UniversalTime {
                            at += now - updated.UniversalTime;
                        }
                    }
                }
                let length = end - start;
                duration = Some((length / 10_000_000) as u32);
                position = Some((at.clamp(0, length) / 10_000_000) as u32);
            }
        }
        Some(Playing {
            title: text(properties.Title()),
            artist: text(properties.Artist()),
            album: text(properties.AlbumTitle()),
            app: session.SourceAppUserModelId().map(|id| super::app_name(&id.to_string_lossy())).unwrap_or_default(),
            playing,
            can_play_pause: flag(controls.IsPlayPauseToggleEnabled()) || flag(controls.IsPlayEnabled()) || flag(controls.IsPauseEnabled()),
            can_previous: flag(controls.IsPreviousEnabled()),
            can_next: flag(controls.IsNextEnabled()),
            position,
            duration,
            art: None,
        })
    }

    /// The art's bytes (JPEG or PNG), when the app gives some.
    fn thumbnail(session: &Session) -> Option<Vec<u8>> {
        let properties = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let stream = properties.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
        let size = stream.Size().ok()?;
        if size == 0 || size > ART_MAX_BYTES {
            return None;
        }
        let buffer = Buffer::Create(size as u32).ok()?;
        let read = stream.ReadAsync(&buffer, size as u32, InputStreamOptions::None).ok()?.join().ok()?;
        let reader = DataReader::FromBuffer(&read).ok()?;
        let mut bytes = vec![0u8; read.Length().ok()? as usize];
        reader.ReadBytes(&mut bytes).ok()?;
        Some(bytes)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{MediaCommand, Reading};

    pub struct Media;

    impl Media {
        pub fn new() -> Self {
            Self
        }

        pub fn run(&mut self, _command: MediaCommand) -> Result<(), String> {
            Ok(())
        }

        pub fn read(&mut self, _previous: &Reading) -> Reading {
            Reading::default()
        }
    }
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

    /// What this computer plays now (`cargo test -p orbiters-desktop media -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn reads_this_computer() {
        let reading = watcher().fresh(Duration::from_secs(5));
        println!("media: {:?}", reading.media);
        println!("volume: {:?}", reading.volume);
        println!("art: {:?}", reading.art.as_ref().map(|a| (&a.key, a.rgb.width, a.rgb.height)));
        assert!(reading.volume.is_some(), "this computer has speakers");
    }

    #[test]
    fn a_command_shows_at_once() {
        let watcher = Watcher { state: Arc::new((Mutex::new(State::default()), Condvar::new())), read: Arc::new(Condvar::new()) };
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
}
