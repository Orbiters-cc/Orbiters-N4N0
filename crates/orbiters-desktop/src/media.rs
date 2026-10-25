//! The computer's music and volume, for N4N0's menu and N4N0's assistant: Windows' media controls (the app that played
//! last: Spotify, a browser, any app that tells Windows), its art, and the default speakers' volume, read by this
//! process's watcher (`orbiters_n4n0::media::Watcher`) on a thread of its own, only while something shows them. The
//! SteamVR overlay, the window and the tray service each have theirs.

pub use orbiters_n4n0::media::{Art, Reading, Watcher, ART_HEIGHT, ART_WIDTH};
use std::sync::{Arc, OnceLock};

/// Art larger than this is not read.
#[cfg(windows)]
const ART_MAX_BYTES: u64 = 4 << 20;

/// The process's watcher, reading Windows' media controls (its thread starts at the first want or command).
pub fn watcher() -> &'static Arc<Watcher> {
    static WATCHER: OnceLock<Arc<Watcher>> = OnceLock::new();
    WATCHER.get_or_init(|| Arc::new(Watcher::new(|| Box::new(platform::Media::new()))))
}

#[cfg(windows)]
mod platform {
    use super::{Reading, ART_MAX_BYTES};
    use orbiters_n4n0::media::{art, MediaBackend};
    use orbiters_orb::menu::{Media as Playing, MediaCommand, Volume};
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

#[cfg(not(windows))]
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
    }
}
