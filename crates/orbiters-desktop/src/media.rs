//! The computer's music and volume, for N4N0's menu and N4N0's assistant: Windows' media controls (the app that played
//! last: Spotify, a browser, any app that tells Windows), its art, and the default speakers' volume, read by this
//! process's watcher (`orbiters_n4n0::media::Watcher`) on a thread of its own, only while something shows them. The
//! SteamVR overlay, the window and the tray service each have theirs.
//!
//! macOS tells other programs nothing about what plays (its media controls are closed to them), so a Mac reads the two
//! players that answer Apple events, Music and Spotify, when they are already running. What else plays is not shown.

pub use orbiters_n4n0::media::{Art, Reading, Watcher, ART_HEIGHT, ART_WIDTH};
use std::sync::{Arc, OnceLock};

/// Art larger than this is not read.
#[cfg(any(windows, target_os = "macos"))]
const ART_MAX_BYTES: u64 = 4 << 20;

/// The process's watcher, reading this system's music (its thread starts at the first want or command).
pub fn watcher() -> &'static Arc<Watcher> {
    static WATCHER: OnceLock<Arc<Watcher>> = OnceLock::new();
    WATCHER.get_or_init(|| Arc::new(Watcher::new(|| Box::new(platform::Media::new()))))
}

#[cfg(windows)]
mod platform {
    use super::{Reading, ART_MAX_BYTES};
    use orbiters_n4n0::media::{art, MediaBackend};
    use orbiters_n4n0_core::menu::{Media as Playing, MediaCommand, Volume};
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

        fn run_command(&mut self, command: MediaCommand) -> windows::core::Result<()> {
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
                        // From the timeline's start (most apps' is 0), in 100 ns ticks; the app answers whether it moved.
                        MediaCommand::Seek(seconds) => {
                            let start = session.GetTimelineProperties().and_then(|t| t.StartTime()).map(|t| t.Duration).unwrap_or(0);
                            let moved = session.TryChangePlaybackPositionAsync(start + seconds as i64 * 10_000_000)?.join()?;
                            if !moved {
                                // E_FAIL: the app said no.
                                return Err(windows::core::Error::new(windows::core::HRESULT(0x8000_4005_u32 as i32), "the app did not move in its timeline"));
                            }
                            return Ok(());
                        }
                        _ => session.TrySkipPreviousAsync()?,
                    };
                    done.join().map(|_| ())
                }
            }
        }

        /// What plays now, the volume and the art (read again only for another song).
        fn read_now(&mut self, previous: &Reading) -> Reading {
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

    impl MediaBackend for Media {
        fn run(&mut self, command: MediaCommand) -> Result<(), String> {
            self.run_command(command).map_err(|e| e.to_string())
        }

        fn read(&mut self, previous: &Reading) -> Reading {
            self.read_now(previous)
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
            app: session.SourceAppUserModelId().map(|id| orbiters_n4n0::media::app_name(&id.to_string_lossy())).unwrap_or_default(),
            playing,
            can_play_pause: flag(controls.IsPlayPauseToggleEnabled()) || flag(controls.IsPlayEnabled()) || flag(controls.IsPauseEnabled()),
            can_previous: flag(controls.IsPreviousEnabled()),
            can_next: flag(controls.IsNextEnabled()),
            // The app takes a position (a browser's page that handles "seekto", Spotify, Windows' Media Player).
            can_seek: flag(controls.IsPlaybackPositionEnabled()),
            position,
            duration,
            art: None,
            // Looked up on YouTube by the watcher (`orbiters_n4n0::media::Links`).
            video: None,
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

/// Music and Spotify through `osascript`. It is started only for a player that is running, which is looked up in the
/// process list first (asking macOS for free, and never launching the player), and its scripts look once more. The
/// first Apple event to a player makes macOS ask the person whether this app may control it; refused, the player is
/// left alone, and looked at again now and then in case the person changed their mind in System Settings (macOS does
/// not ask a second time).
#[cfg(target_os = "macos")]
mod platform {
    use super::{Reading, ART_MAX_BYTES};
    use orbiters_n4n0::media::{art, MediaBackend};
    use orbiters_n4n0_core::menu::{Media as Playing, MediaCommand, Volume};
    use orbiters_sound::coreaudio as ca;
    use std::os::raw::{c_int, c_void};
    use std::process::{Command, Output, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A player that answers Apple events.
    pub(super) struct App {
        /// Its bundle identifier: what the scripts name, and what macOS's question is about.
        id: &'static str,
        /// Its name, as shown, and its program in its bundle.
        name: &'static str,
        /// The units its tracks' length comes in, to a second.
        length_per_second: u32,
        /// Tells a link to its art in its track.
        art_link: bool,
    }

    const APPS: [App; 2] = [
        App { id: "com.apple.Music", name: "Music", length_per_second: 1, art_link: false },
        App { id: "com.spotify.client", name: "Spotify", length_per_second: 1000, art_link: true },
    ];

    /// How long a reading waits for a player: longer, and it goes on without it (the volume still reads) and looks for
    /// the answer next time. The first Apple event waits for the person's answer to macOS's question.
    const ANSWER_WAIT: Duration = Duration::from_secs(1);
    /// A command to a player, or its art, waits this much.
    const COMMAND_WAIT: Duration = Duration::from_secs(5);
    /// A player that refused is looked at again after this, without a word. macOS does not ask again.
    const REFUSED_FOR: Duration = Duration::from_secs(60);
    /// While a player plays, the other one is read one reading in this many (a look that costs another program).
    const OTHER_EVERY: u32 = 5;
    /// Times a song's art is asked for before it goes without.
    const ART_TRIES: u8 = 3;
    /// Between the fields of a reading: never in a title.
    const SEPARATOR: char = '\u{1f}';

    /// The script that reads `app`: nothing when it stopped, else its state, title, artist, album, position and length
    /// in whole seconds (a real number written out would depend on the person's language), and a link to the art.
    pub(super) fn read_script(app: &App) -> String {
        let id = app.id;
        let per_second = app.length_per_second;
        let art = if app.art_link { "my field(artwork url of theTrack)" } else { "\"\"" };
        format!(
            r#"on field(v)
	if v is missing value then return ""
	return v as text
end field
if application id "{id}" is not running then return ""
tell application id "{id}"
	set theState to player state as text
	if theState is "stopped" then return ""
	set theTrack to current track
	set separator to character id 31
	set theLength to 0
	try
		set theLength to (duration of theTrack) div {per_second}
	end try
	set thePosition to 0
	try
		set thePosition to (player position) div 1
	end try
	return theState & separator & my field(name of theTrack) & separator & my field(artist of theTrack) & separator & my field(album of theTrack) & separator & thePosition & separator & theLength & separator & {art}
end tell"#
        )
    }

    /// The script that fetches the art of `app`'s track, for an app that keeps it itself.
    pub(super) fn art_script(app: &App) -> String {
        let id = app.id;
        format!(
            r#"if application id "{id}" is not running then return ""
tell application id "{id}"
	if (count of artworks of current track) is 0 then return ""
	return raw data of artwork 1 of current track
end tell"#
        )
    }

    /// The script that makes `app` do `what`.
    pub(super) fn command_script(app: &App, what: &str) -> String {
        let id = app.id;
        format!(r#"if application id "{id}" is running then tell application id "{id}" to {what}"#)
    }

    /// What `command` is in a player's words, for those a player takes.
    pub(super) fn player_words(command: MediaCommand) -> Option<String> {
        Some(match command {
            MediaCommand::PlayPause => "playpause".into(),
            MediaCommand::Play => "play".into(),
            MediaCommand::Pause => "pause".into(),
            MediaCommand::Next => "next track".into(),
            MediaCommand::Previous => "previous track".into(),
            MediaCommand::Seek(seconds) => format!("set player position to {seconds}"),
            MediaCommand::Volume(_) | MediaCommand::Mute(_) => return None,
        })
    }

    /// A reading of `app`, with the link to its art if it tells one.
    pub(super) fn parse(app: &App, output: &str) -> Option<(Playing, Option<String>)> {
        let mut fields = output.trim_end_matches(['\r', '\n']).split(SEPARATOR);
        let state = fields.next()?;
        let title = fields.next()?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        let artist = fields.next().unwrap_or_default().trim().to_string();
        let album = fields.next().unwrap_or_default().trim().to_string();
        let position = fields.next().and_then(|p| p.trim().parse::<u32>().ok());
        let duration = fields.next().and_then(|d| d.trim().parse::<u32>().ok()).filter(|d| *d > 0);
        let link = fields.next().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
        let media = Playing {
            title,
            artist,
            album,
            app: app.name.to_string(),
            playing: state == "playing",
            can_play_pause: true,
            can_previous: true,
            can_next: true,
            can_seek: duration.is_some(),
            position: duration.and(position).map(|p| p.min(duration.unwrap_or(u32::MAX))),
            duration,
            art: None,
            video: None,
        };
        Some((media, link))
    }

    /// The bytes of a picture as `osascript` writes binary data: `«data tdtaFFD8…»`.
    pub(super) fn parse_data(output: &str) -> Option<Vec<u8>> {
        let hex = output.trim().strip_prefix("«data ")?.strip_suffix('»')?;
        let hex = hex.get(4..)?.as_bytes();
        if !hex.len().is_multiple_of(2) || hex.len() as u64 / 2 > ART_MAX_BYTES {
            return None;
        }
        let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
        hex.chunks_exact(2).map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?)).collect()
    }

    /// Whether a player's program is running, by its name and where it is in its bundle. No program is started for this
    /// and nothing is asked of the person.
    fn running(app: &App) -> bool {
        extern "C" {
            fn proc_listallpids(buffer: *mut c_void, size: c_int) -> c_int;
            fn proc_name(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
            fn proc_pidpath(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
        }
        let mut pids = vec![0 as c_int; 4096];
        let size = (pids.len() * std::mem::size_of::<c_int>()) as c_int;
        // SAFETY: the buffers are as long as the sizes given, and the calls write within them and say how much.
        unsafe {
            let count = proc_listallpids(pids.as_mut_ptr().cast(), size);
            let mut name = [0u8; 64];
            let mut path = vec![0u8; 4096];
            let bundle_end = format!(".app/Contents/MacOS/{}", app.name);
            pids.iter().take(count.max(0) as usize).any(|pid| {
                let length = proc_name(*pid, name.as_mut_ptr().cast(), name.len() as u32);
                if length <= 0 || &name[..length as usize] != app.name.as_bytes() {
                    return false;
                }
                let length = proc_pidpath(*pid, path.as_mut_ptr().cast(), path.len() as u32);
                length > 0 && path[..length as usize].ends_with(bundle_end.as_bytes())
            })
        }
    }

    /// What `osascript` answered, on a thread of its own: a first Apple event may wait a while for the person, and
    /// `osascript` ends by itself after its two minutes.
    fn ask(script: String) -> mpsc::Receiver<std::io::Result<Output>> {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("media-script".into()).spawn(move || {
            let _ = tx.send(Command::new("/usr/bin/osascript").arg("-e").arg(script).stdin(Stdio::null()).output());
        });
        rx
    }

    /// What became of a question to a player.
    enum Answer {
        Text(String),
        /// macOS refuses this app the player, or the person did.
        Refused,
        /// It failed or has not answered yet: said nothing about the player.
        Nothing,
    }

    fn answer(app: &App, output: std::io::Result<Output>, said: &mut bool) -> Answer {
        let output = match output {
            Ok(output) => output,
            Err(e) => {
                if !std::mem::replace(said, true) {
                    eprintln!("media: {}: osascript: {e}", app.name);
                }
                return Answer::Nothing;
            }
        };
        if output.status.success() {
            return Answer::Text(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let error = String::from_utf8_lossy(&output.stderr);
        // errAEEventNotPermitted: the person's answer, quietly respected.
        if error.contains("-1743") {
            return Answer::Refused;
        }
        // The player quit meanwhile, or has no track (`-600`, `-1728`): nothing to say. Anything else once.
        if !error.contains("-600") && !error.contains("-1728") && !std::mem::replace(said, true) {
            eprintln!("media: {}: {}", app.name, error.trim());
        }
        Answer::Nothing
    }

    /// What is known of one player.
    #[derive(Default)]
    struct Player {
        /// A reading still on its way.
        waiting: Option<mpsc::Receiver<std::io::Result<Output>>>,
        refused_until: Option<Instant>,
        /// An unexpected error of this player was said already.
        said: bool,
    }

    pub struct Media {
        players: [Player; APPS.len()],
        /// The player shown last: commands go to it.
        current: Option<usize>,
        reads: u32,
        /// What the last art was read for (the same song keeps its art), and how many times it was asked for.
        art_for: Option<(String, String, String)>,
        art_tries: u8,
    }

    impl Media {
        pub fn new() -> Self {
            Self { players: Default::default(), current: None, reads: 0, art_for: None, art_tries: 0 }
        }

        /// What `index` plays, unless it is not running, refused, stopped or has not answered yet.
        fn read_player(&mut self, index: usize) -> Option<(Playing, Option<String>)> {
            let app = &APPS[index];
            let player = &mut self.players[index];
            if player.refused_until.is_some_and(|t| Instant::now() < t) {
                return None;
            }
            player.refused_until = None;
            if player.waiting.is_none() {
                if !running(app) {
                    return None;
                }
                player.waiting = Some(ask(read_script(app)));
            }
            let output = match player.waiting.as_ref()?.recv_timeout(ANSWER_WAIT) {
                Ok(output) => output,
                Err(mpsc::RecvTimeoutError::Timeout) => return None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    player.waiting = None;
                    return None;
                }
            };
            player.waiting = None;
            match answer(app, output, &mut player.said) {
                Answer::Text(text) => parse(app, &text),
                Answer::Refused => {
                    player.refused_until = Some(Instant::now() + REFUSED_FOR);
                    None
                }
                Answer::Nothing => None,
            }
        }

        /// The art's bytes (JPEG or PNG), when the player gives some.
        fn fetch_art(&mut self, index: usize, link: Option<&str>) -> Option<Vec<u8>> {
            if APPS[index].art_link {
                // The player's own address, kept to its own secure scheme.
                let link = link?.replacen("http://", "https://", 1);
                let agent = orbiters_core::http_agent(Some(Duration::from_secs(3)), Some(COMMAND_WAIT));
                let mut body = agent.get(&link).call().ok()?.into_body();
                return body.with_config().limit(ART_MAX_BYTES).read_to_vec().ok();
            }
            let output = ask(art_script(&APPS[index])).recv_timeout(COMMAND_WAIT).ok()?;
            match answer(&APPS[index], output, &mut self.players[index].said) {
                Answer::Text(text) => parse_data(&text),
                _ => None,
            }
        }

        /// What plays now, the volume and the art (read again only for another song). The player that plays comes
        /// first, then the one shown last.
        fn read_now(&mut self, previous: &Reading) -> Reading {
            let volume = ca::output_volume().map(|(level, muted)| Volume { percent: (level * 100.0).round().clamp(0.0, 100.0) as u8, muted });
            self.reads = self.reads.wrapping_add(1);
            let mut found: Option<(usize, Playing, Option<String>)> = None;
            let shown_first = self.current.into_iter().chain((0..APPS.len()).filter(|i| Some(*i) != self.current)).collect::<Vec<_>>();
            for index in shown_first {
                if found.as_ref().is_some_and(|(_, m, _)| m.playing) && !self.reads.is_multiple_of(OTHER_EVERY) {
                    break;
                }
                if let Some((media, link)) = self.read_player(index) {
                    if found.as_ref().is_none_or(|(_, m, _)| media.playing && !m.playing) {
                        found = Some((index, media, link));
                    }
                }
            }
            let Some((index, media, link)) = found else {
                // A player that has not answered yet keeps what it last said (the answer that comes will tell).
                if self.current.is_some_and(|i| self.players[i].waiting.is_some()) {
                    return Reading { media: previous.media.clone(), volume, art: previous.art.clone() };
                }
                self.current = None;
                self.art_for = None;
                return Reading { media: None, volume, art: None };
            };
            self.current = Some(index);
            let key = Some((media.title.clone(), media.artist.clone(), media.album.clone()));
            let changed = key != self.art_for;
            if changed {
                self.art_for = key;
                self.art_tries = 0;
            }
            let art_now = if !changed && previous.art.is_some() {
                previous.art.clone()
            } else if self.art_tries < ART_TRIES {
                self.art_tries += 1;
                self.fetch_art(index, link.as_deref()).and_then(|bytes| art(&media, &bytes))
            } else {
                None
            };
            let media = Playing { art: art_now.as_ref().map(|a| a.key.clone()), ..media };
            Reading { media: Some(media), volume, art: art_now }
        }
    }

    impl MediaBackend for Media {
        fn run(&mut self, command: MediaCommand) -> Result<(), String> {
            match command {
                MediaCommand::Volume(percent) => {
                    ca::set_output_mute(false)?;
                    ca::set_output_volume(percent.min(100) as f32 / 100.0)
                }
                MediaCommand::Mute(muted) => ca::set_output_mute(muted),
                _ => {
                    let (Some(index), Some(words)) = (self.current, player_words(command)) else { return Ok(()) };
                    let app = &APPS[index];
                    if self.players[index].refused_until.is_some() || !running(app) {
                        return Ok(());
                    }
                    let Ok(output) = ask(command_script(app, &words)).recv_timeout(COMMAND_WAIT) else {
                        return Err(format!("{} did not answer", app.name));
                    };
                    match answer(app, output, &mut self.players[index].said) {
                        Answer::Refused => self.players[index].refused_until = Some(Instant::now() + REFUSED_FOR),
                        Answer::Text(_) | Answer::Nothing => {}
                    }
                    Ok(())
                }
            }
        }

        fn read(&mut self, previous: &Reading) -> Reading {
            self.read_now(previous)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_reading_becomes_what_plays() {
            let (m, link) = parse(&APPS[0], "playing\u{1f}Song\u{1f}Singer\u{1f}Album\u{1f}12\u{1f}200\u{1f}\n").unwrap();
            assert_eq!((m.title.as_str(), m.artist.as_str(), m.album.as_str(), m.app.as_str()), ("Song", "Singer", "Album", "Music"));
            assert!(m.playing && m.can_seek && m.can_play_pause && m.can_next && m.can_previous);
            assert_eq!((m.position, m.duration, link), (Some(12), Some(200), None));
            // Paused, with a link to its art; a stream without a length cannot be moved in.
            let (m, link) = parse(&APPS[1], "paused\u{1f}Song\u{1f}\u{1f}\u{1f}3\u{1f}0\u{1f}https://i.scdn.co/image/ab\n").unwrap();
            assert!(!m.playing && !m.can_seek);
            assert_eq!((m.position, m.duration, m.artist.as_str()), (None, None, ""));
            assert_eq!(link.as_deref(), Some("https://i.scdn.co/image/ab"));
            // A position beyond the end stays at the end.
            assert_eq!(parse(&APPS[0], "playing\u{1f}Song\u{1f}\u{1f}\u{1f}300\u{1f}200\u{1f}").unwrap().0.position, Some(200));
        }

        #[test]
        fn a_stopped_player_or_no_title_shows_nothing() {
            assert!(parse(&APPS[0], "\n").is_none());
            assert!(parse(&APPS[0], "").is_none());
            assert!(parse(&APPS[0], "playing\u{1f} \u{1f}a\u{1f}b\u{1f}1\u{1f}2\u{1f}").is_none());
        }

        #[test]
        fn binary_data_reads_back() {
            assert_eq!(parse_data("«data tdtaFFD8FF00»\n"), Some(vec![0xff, 0xd8, 0xff, 0x00]));
            assert_eq!(parse_data("«data JPEGffd8»"), Some(vec![0xff, 0xd8]));
            assert_eq!(parse_data("«data tdtaFFD»"), None);
            assert_eq!(parse_data("«data tdtaZZ»"), None);
            assert_eq!(parse_data(""), None);
            assert_eq!(parse_data("not data"), None);
        }

        #[test]
        fn commands_say_what_players_understand() {
            assert_eq!(player_words(MediaCommand::Seek(42)).as_deref(), Some("set player position to 42"));
            assert_eq!(player_words(MediaCommand::Next).as_deref(), Some("next track"));
            assert_eq!(player_words(MediaCommand::Volume(3)), None);
            assert_eq!(
                command_script(&APPS[1], "pause"),
                r#"if application id "com.spotify.client" is running then tell application id "com.spotify.client" to pause"#
            );
        }

        #[test]
        fn a_refusal_is_quiet_and_other_failures_are_said_once() {
            use std::os::unix::process::ExitStatusExt;
            let failed = |error: &str| Ok(Output { status: std::process::ExitStatus::from_raw(1 << 8), stdout: Vec::new(), stderr: error.as_bytes().to_vec() });
            let mut said = false;
            assert!(matches!(answer(&APPS[0], failed("execution error: Not authorized to send Apple events to Music. (-1743)"), &mut said), Answer::Refused));
            assert!(!said);
            assert!(matches!(answer(&APPS[0], failed("execution error: Music got an error: Application isn't running. (-600)"), &mut said), Answer::Nothing));
            assert!(matches!(answer(&APPS[0], failed("Can't get current track. (-1728)"), &mut said), Answer::Nothing));
            assert!(!said);
            assert!(matches!(answer(&APPS[0], failed("something else"), &mut said), Answer::Nothing));
            assert!(said);
        }

        /// The scripts in the words of the player's own dictionary, read from its bundle: compiling starts nothing.
        #[test]
        fn the_scripts_compile_and_start_nothing() {
            let dir = std::env::temp_dir().join(format!("orbiters-media-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let installed = |app: &App| match app.name {
                "Music" => std::path::Path::new("/System/Applications/Music.app").is_dir(),
                _ => std::path::Path::new("/Applications/Spotify.app").is_dir(),
            };
            for app in APPS.iter().filter(|a| installed(a)) {
                let was_running = running(app);
                let mut scripts =
                    vec![read_script(app), command_script(app, "playpause"), command_script(app, "next track"), command_script(app, "previous track")];
                scripts.push(command_script(app, "set player position to 42"));
                if !app.art_link {
                    scripts.push(art_script(app));
                }
                for script in scripts {
                    let out = Command::new("/usr/bin/osacompile").arg("-o").arg(dir.join("s.scpt")).arg("-e").arg(&script).output().unwrap();
                    assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
                }
                assert_eq!(running(app), was_running, "{} was not started", app.name);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// What `osascript` itself writes reads back as these parsers expect: a reading's fields joined as the read script
        /// joins them (a real number cut to whole seconds, a missing value empty), and binary data. No player is asked.
        #[test]
        fn what_osascript_writes_is_what_is_parsed() {
            let run = |script: &str| {
                let out = Command::new("/usr/bin/osascript").arg("-e").arg(script).output().unwrap();
                assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
                String::from_utf8(out.stdout).unwrap()
            };
            let reading = run(r#"on field(v)
	if v is missing value then return ""
	return v as text
end field
set separator to character id 31
return "playing" & separator & my field("Sóng “x”") & separator & my field(missing value) & separator & my field("Album") & separator & (12.7 div 1) & separator & (215.6 div 1) & separator & """#);
            let (media, link) = parse(&APPS[0], &reading).unwrap();
            assert_eq!((media.title.as_str(), media.artist.as_str(), media.album.as_str()), ("Sóng “x”", "", "Album"));
            assert_eq!((media.playing, media.position, media.duration, link), (true, Some(12), Some(215), None));
            assert_eq!(parse_data(&run("return «data tdtaFFD8FF00»")), Some(vec![0xff, 0xd8, 0xff, 0x00]));
        }

        /// What a player that is not running gets: nothing is started for it.
        #[test]
        fn no_process_of_that_name_is_no_player() {
            let nobody = App { id: "cc.orbiters.no-such-player", name: "OrbitersNoSuchPlayer", length_per_second: 1, art_link: false };
            assert!(!running(&nobody));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    /// No media controls to read.
    pub struct Media;

    impl Media {
        pub fn new() -> orbiters_n4n0::media::NoMedia {
            orbiters_n4n0::media::NoMedia
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// What this computer plays now (`cargo test -p orbiters-desktop media -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn reads_this_computer() {
        let reading = watcher().fresh(Duration::from_secs(5));
        println!("media: {:?}", reading.media);
        println!("volume: {:?}", reading.volume);
        println!("art: {:?}", reading.art.as_ref().map(|a| (&a.key, a.rgb.width, a.rgb.height)));
        assert!(reading.volume.is_some(), "this computer has speakers");
        // A YouTube video in a browser: found on YouTube within a few readings.
        if reading.media.as_ref().is_some_and(|m| orbiters_n4n0_core::youtube::worth_looking_up(&m.app, &m.title, &m.artist, &m.album)) {
            let found = (0..10).find_map(|_| watcher().fresh(Duration::from_secs(5)).media.and_then(|m| m.link()));
            println!("YouTube link: {found:?}");
        }
    }
}
