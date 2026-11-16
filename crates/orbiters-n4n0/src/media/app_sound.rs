//! The playing app's own volume, the way Windows' volume mixer has one slider per app (Enzo, 2026-11-16: "make the
//! volume slider only change the volume of the app playing and not the whole computer").
//!
//! Windows' media controls name the app that plays by its app id (`Spotify.exe`, `Chrome`, `MSEdge`, Firefox's
//! `308046B0AF4A39CB`, a store app's `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify`); the mixer's sounds (audio
//! sessions) belong to processes, on every output device. The app's sounds are those of:
//! - the processes of its package, for a store app (its id has a `!`), whatever their program is called;
//! - else the processes whose program is the id's (`Chrome` → `chrome.exe`, a path's last part, a web app's browser,
//!   Firefox's id → `firefox.exe`);
//! - else, when no process has that program, those whose windows carry the id (the taskbar groups by it);
//! - and the processes these started, however deep (a browser plays from a child process; a player may use a helper),
//!   checked by start times so that a number Windows gave again to another process never makes it a child.
//!
//! Its sounds on every device count (a headset's, the speakers', whichever is the default): reading gives the loudest
//! of those playing (of all, when none plays) and muted when all are; setting sets them all. Nothing else is touched:
//! not the device's own volume, not another app's sound (VRChat's above all).

use orbiters_n4n0_core::menu::{AppSound, Volume};
use std::collections::{HashMap, HashSet};

/// A process as the system lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// The process that started it (it may have ended since, and its number been given again).
    pub parent: u32,
    /// Its program's file name (`chrome.exe`).
    pub exe: String,
    /// When it started (any steadily growing clock), when it could be read: a child starts after its parent.
    pub started: Option<u64>,
    /// Its app id when it belongs to a package (a store app): `Family_hash!App`.
    pub package: Option<String>,
}

/// One sound of the mixer: an app's audio session on one output device.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// Which output device it plays on.
    pub device: String,
    /// The process it belongs to (0: the system's own sounds).
    pub pid: u32,
    /// Its volume, 0 to 1, and whether it is muted.
    pub level: f32,
    pub muted: bool,
    /// It plays now (not only open).
    pub playing: bool,
}

/// The system's mixer: what [`AppVolume`] reads and sets (Windows' audio sessions; a stand-in in tests).
pub trait Mixer {
    /// The sounds on every output device that is on, as they are now (those that ended left out).
    fn sounds(&mut self) -> Vec<Sound>;
    /// The processes running now.
    fn processes(&mut self) -> Vec<Process>;
    /// The processes with a window that says it is app `id`'s (asked only when no program has its name).
    fn windows_of(&mut self, id: &str) -> Vec<u32>;
    /// Sets the `index`-th of the last [`Mixer::sounds`]: its level (0 to 1) and whether it is muted, each when given.
    fn set(&mut self, index: usize, level: Option<f32>, muted: Option<bool>) -> Result<(), String>;
}

/// The program `id` names, lower case and without `.exe`: `Chrome` → `chrome`, `C:\…\vlc.exe` → `vlc`, a web app
/// installed from Chrome (`Chrome._crx_…`) → `chrome`, Firefox's id → `firefox`. `None` for a store app's id.
pub fn program_of(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id.contains('!') {
        return None;
    }
    let id = id.split_once("._crx_").map_or(id, |(browser, _)| browser);
    let last = id.rsplit(['\\', '/']).next().unwrap_or(id).to_ascii_lowercase();
    let stem = last.strip_suffix(".exe").unwrap_or(&last);
    Some(
        match stem {
            // Firefox names itself by a hash of where it is installed; this is the usual one's.
            "308046b0af4a39cb" => "firefox",
            other => other,
        }
        .to_string(),
    )
}

/// A program's file name as [`program_of`] gives it.
fn stem(exe: &str) -> String {
    let exe = exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_ascii_lowercase();
    exe.strip_suffix(".exe").unwrap_or(&exe).to_string()
}

/// A store app's package family (`SpotifyAB.SpotifyMusic_zpdnekdrzrea0`), lower case.
fn family(id: &str) -> String {
    id.trim().split('!').next().unwrap_or_default().to_ascii_lowercase()
}

/// The processes whose sounds are app `id`'s (see the module's notes). `windows` is asked only when no process has
/// the app's package or program.
pub fn app_pids(id: &str, processes: &[Process], windows: impl FnOnce() -> Vec<u32>) -> HashSet<u32> {
    let id = id.trim();
    let mut found: HashSet<u32> = HashSet::new();
    if id.is_empty() {
        return found;
    }
    if id.contains('!') {
        let wanted = family(id);
        found.extend(processes.iter().filter(|p| p.package.as_deref().is_some_and(|k| family(k) == wanted)).map(|p| p.pid));
    } else if let Some(program) = program_of(id) {
        found.extend(processes.iter().filter(|p| stem(&p.exe) == program).map(|p| p.pid));
        // `VideoLAN.VLC`: its last dotted part, when the whole names no program.
        if found.is_empty() {
            if let Some((_, last)) = program.rsplit_once('.').filter(|(_, last)| !last.is_empty()) {
                found.extend(processes.iter().filter(|p| stem(&p.exe) == last).map(|p| p.pid));
            }
        }
    }
    if found.is_empty() {
        let running: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        found.extend(windows().into_iter().filter(|pid| running.contains(pid)));
    }
    found.remove(&0);
    if found.is_empty() {
        return found;
    }
    // The processes these started, however deep: each link only where the child started after its parent.
    let by_pid: HashMap<u32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
    let mut children = Vec::new();
    for process in processes {
        let mut at: &Process = process;
        let mut seen = 0;
        while !found.contains(&at.pid) && seen < 64 {
            let Some(&parent) = by_pid.get(&at.parent).filter(|parent| parent.pid != at.pid && parent.pid != 0) else { break };
            let (Some(child_start), Some(parent_start)) = (at.started, parent.started) else { break };
            if parent_start > child_start {
                break;
            }
            at = parent;
            seen += 1;
        }
        if found.contains(&at.pid) {
            children.push(process.pid);
        }
    }
    found.extend(children);
    found
}

/// The app's volume from its sounds: the loudest of those playing (of all, when none plays), muted when all are.
pub fn level_of<'a>(sounds: impl IntoIterator<Item = &'a Sound>) -> Option<Volume> {
    let sounds: Vec<&Sound> = sounds.into_iter().collect();
    let playing: Vec<&Sound> = sounds.iter().copied().filter(|s| s.playing).collect();
    let counted = if playing.is_empty() { sounds } else { playing };
    let loudest = counted.iter().map(|s| s.level).fold(None, |best: Option<f32>, l| Some(best.map_or(l, |b| b.max(l))))?;
    Some(Volume { percent: (loudest * 100.0).round().clamp(0.0, 100.0) as u8, muted: counted.iter().all(|s| s.muted) })
}

/// Reads and sets the playing app's own volume through a [`Mixer`].
pub struct AppVolume<M> {
    pub mixer: M,
}

impl<M: Mixer> AppVolume<M> {
    pub fn new(mixer: M) -> Self {
        Self { mixer }
    }

    /// The sounds of app `id` now, with their places in the mixer's list.
    fn sounds_of(&mut self, id: &str) -> Vec<(usize, Sound)> {
        let sounds = self.mixer.sounds();
        if sounds.iter().all(|s| s.pid == 0) {
            return Vec::new();
        }
        let processes = self.mixer.processes();
        let mixer = &mut self.mixer;
        let pids = app_pids(id, &processes, || mixer.windows_of(id));
        sounds.into_iter().enumerate().filter(|(_, s)| s.pid != 0 && pids.contains(&s.pid)).collect()
    }

    /// App `id`'s volume as the mixer has it now: changes made elsewhere (the system's mixer, the app itself) show at
    /// the next reading.
    pub fn read(&mut self, id: &str) -> AppSound {
        match level_of(self.sounds_of(id).iter().map(|(_, s)| s)) {
            Some(volume) => AppSound::Level(volume),
            None => AppSound::NotFound,
        }
    }

    /// App `id`'s volume to `percent`, unmuted, on all its sounds. An app without sound is an error: nothing else is
    /// set in its place.
    pub fn set_level(&mut self, id: &str, percent: u8) -> Result<(), String> {
        self.set(id, Some(percent.min(100) as f32 / 100.0), Some(false))
    }

    /// App `id` muted or not, on all its sounds.
    pub fn set_muted(&mut self, id: &str, muted: bool) -> Result<(), String> {
        self.set(id, None, Some(muted))
    }

    fn set(&mut self, id: &str, level: Option<f32>, muted: Option<bool>) -> Result<(), String> {
        let sounds = self.sounds_of(id);
        if sounds.is_empty() {
            return Err(format!("no sound of {id} in the mixer")); // i18n: not shown (logged by the media thread)
        }
        let mut failed = None;
        for (index, sound) in sounds {
            // Unchanged ones are left alone (an unmute with nothing muted sets nothing).
            let level = level.filter(|l| (l - sound.level).abs() > 0.000_5);
            let muted = muted.filter(|m| *m != sound.muted);
            if level.is_some() || muted.is_some() {
                if let Err(e) = self.mixer.set(index, level, muted) {
                    failed = Some(e);
                }
            }
        }
        failed.map_or(Ok(()), Err)
    }
}

/// A made-up mixer: its sounds and processes as a test sets them, and what was set.
#[cfg(test)]
#[derive(Default)]
pub struct StandIn {
    pub sounds: Vec<Sound>,
    pub processes: Vec<Process>,
    pub windows: Vec<(String, u32)>,
    pub windows_asked: u32,
}

#[cfg(test)]
impl Mixer for StandIn {
    fn sounds(&mut self) -> Vec<Sound> {
        self.sounds.clone()
    }

    fn processes(&mut self) -> Vec<Process> {
        self.processes.clone()
    }

    fn windows_of(&mut self, id: &str) -> Vec<u32> {
        self.windows_asked += 1;
        self.windows.iter().filter(|(w, _)| w == id).map(|(_, pid)| *pid).collect()
    }

    fn set(&mut self, index: usize, level: Option<f32>, muted: Option<bool>) -> Result<(), String> {
        let sound = self.sounds.get_mut(index).ok_or("no such sound")?; // i18n: not shown
        if let Some(level) = level {
            sound.level = level;
        }
        if let Some(muted) = muted {
            sound.muted = muted;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn process(pid: u32, parent: u32, exe: &str, started: u64) -> Process {
    Process { pid, parent, exe: exe.into(), started: Some(started), package: None }
}

#[cfg(test)]
pub(crate) fn sound(device: &str, pid: u32, level: f32, playing: bool) -> Sound {
    Sound { device: device.into(), pid, level, muted: false, playing }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A computer with a browser playing from its audio child, VRChat, Discord and the system's sounds.
    fn computer() -> StandIn {
        StandIn {
            processes: vec![
                process(4, 0, "System", 1),
                process(100, 4, "explorer.exe", 10),
                process(200, 100, "chrome.exe", 20),
                // Chrome's audio service: a child, and the one with the sound.
                process(210, 200, "chrome.exe", 21),
                process(300, 100, "VRChat.exe", 30),
                process(400, 100, "Discord.exe", 40),
                process(500, 100, "Spotify.exe", 50),
                // Started by Spotify: a helper of another name.
                process(510, 500, "SpotifyHelper.exe", 51),
            ],
            sounds: vec![
                sound("Speakers", 0, 1.0, true),
                sound("Speakers", 210, 0.6, true),
                sound("Speakers", 300, 0.9, true),
                sound("Headset", 300, 0.8, true),
                sound("Speakers", 400, 0.7, true),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn app_ids_name_their_programs() {
        assert_eq!(program_of("Chrome").as_deref(), Some("chrome"));
        assert_eq!(program_of("MSEdge").as_deref(), Some("msedge"));
        assert_eq!(program_of("Spotify.exe").as_deref(), Some("spotify"));
        assert_eq!(program_of("308046B0AF4A39CB").as_deref(), Some("firefox"));
        assert_eq!(program_of("{6D809377-6AF0-444B-8957-A3773F02200E}\\VideoLAN\\VLC\\vlc.exe").as_deref(), Some("vlc"));
        assert_eq!(program_of("Chrome._crx_agimnkijcaahngcdmfeangaknmldooml").as_deref(), Some("chrome"));
        assert_eq!(program_of("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), None, "a store app goes by its package");
        assert_eq!(program_of("  "), None);
    }

    /// A browser plays from a child process: its volume is that child's sound, and setting it leaves VRChat, Discord,
    /// the system's sounds and the device alone.
    #[test]
    fn several_processes_of_one_app_are_one_volume() {
        let mut apps = AppVolume::new(computer());
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 60, muted: false }));
        apps.set_level("Chrome", 25).unwrap();
        let sounds = &apps.mixer.sounds;
        assert_eq!(sounds[1].level, 0.25);
        assert_eq!((sounds[0].level, sounds[2].level, sounds[3].level, sounds[4].level), (1.0, 0.9, 0.8, 0.7), "nothing else moved");
        // Its window and its audio child both playing, on two levels: the loudest shows, and both are set.
        apps.mixer.sounds.push(sound("Speakers", 200, 0.4, true));
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 40, muted: false }));
        apps.set_muted("Chrome", true).unwrap();
        assert!(apps.mixer.sounds[1].muted && apps.mixer.sounds[5].muted && !apps.mixer.sounds[2].muted && !apps.mixer.sounds[4].muted);
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 40, muted: true }));
        // Turning it up unmutes it.
        apps.set_level("Chrome", 50).unwrap();
        assert!(!apps.mixer.sounds[1].muted && !apps.mixer.sounds[5].muted);
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 50, muted: false }));
    }

    /// A helper the app started is the app's, whatever it is called; a process whose parent's number was given
    /// again to the app later is not (VRChat started by a launcher that ended).
    #[test]
    fn children_count_and_a_number_given_again_does_not() {
        let mut computer = computer();
        computer.sounds.push(sound("Speakers", 510, 0.3, true));
        // VRChat's launcher (pid 900) ended; its number then went to a Chrome process started after VRChat.
        computer.processes.push(process(900, 200, "chrome.exe", 60));
        computer.processes.push(process(910, 900, "VRChat.exe", 35));
        computer.sounds.push(sound("Headset", 910, 0.9, true));
        let pids = app_pids("Spotify.exe", &computer.processes, Vec::new);
        assert!(pids.contains(&510) && pids.contains(&500));
        let chrome = app_pids("Chrome", &computer.processes, Vec::new);
        assert!(chrome.contains(&900) && !chrome.contains(&910) && !chrome.contains(&300), "{chrome:?}");
        // A start time that cannot be read links nothing.
        let mut unknown = computer.processes.clone();
        unknown.push(Process { pid: 920, parent: 200, exe: "renderer.exe".into(), started: None, package: None });
        assert!(!app_pids("Chrome", &unknown, Vec::new).contains(&920));
    }

    /// The app plays on the headset, not on the default speakers: its sound there is found and set; so is its sound
    /// on another device, each to the same level.
    #[test]
    fn the_app_on_another_device_is_found() {
        let mut computer = computer();
        computer.sounds.push(sound("Quest (Virtual Desktop)", 500, 0.5, true));
        computer.sounds.push(sound("Speakers", 500, 0.2, false));
        let mut apps = AppVolume::new(computer);
        assert_eq!(apps.read("Spotify.exe"), AppSound::Level(Volume { percent: 50, muted: false }), "the one playing counts");
        apps.set_level("Spotify.exe", 70).unwrap();
        assert_eq!((apps.mixer.sounds[5].level, apps.mixer.sounds[6].level), (0.7, 0.7));
        assert_eq!(apps.mixer.sounds[3].level, 0.8, "VRChat's on the headset untouched");
    }

    /// A store app by its package, whatever its program is called.
    #[test]
    fn a_store_app_goes_by_its_package() {
        let mut computer = computer();
        computer.processes.push(Process {
            pid: 600,
            parent: 100,
            exe: "Microsoft.Media.Player.exe".into(),
            started: Some(70),
            package: Some("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic".into()),
        });
        computer.sounds.push(sound("Speakers", 600, 0.33, true));
        let mut apps = AppVolume::new(computer);
        assert_eq!(apps.read("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), AppSound::Level(Volume { percent: 33, muted: false }));
        assert_eq!(apps.read("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), AppSound::NotFound);
    }

    /// No sound of the app: not found, and setting sets nothing at all (never the computer's, never another app's).
    #[test]
    fn no_sound_found_sets_nothing() {
        let mut apps = AppVolume::new(computer());
        // Spotify runs but has no sound now; foobar2000 does not run.
        assert_eq!(apps.read("Spotify.exe"), AppSound::NotFound);
        assert_eq!(apps.read("C:\\Program Files\\foobar2000\\foobar2000.exe"), AppSound::NotFound);
        assert_eq!(apps.read(""), AppSound::NotFound);
        let before = apps.mixer.sounds.clone();
        assert!(apps.set_level("Spotify.exe", 10).is_err());
        assert!(apps.set_muted("foobar2000.exe", true).is_err());
        assert_eq!(apps.mixer.sounds, before);
        // Only the system's sounds: nothing to look up.
        let mut quiet = AppVolume::new(StandIn { sounds: vec![sound("Speakers", 0, 1.0, true)], ..computer() });
        assert_eq!(quiet.read("Chrome"), AppSound::NotFound);
    }

    /// Changed in Windows' mixer or in the app: the next reading says so.
    #[test]
    fn a_change_made_elsewhere_shows() {
        let mut apps = AppVolume::new(computer());
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 60, muted: false }));
        apps.mixer.sounds[1].level = 0.15;
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 15, muted: false }));
        apps.mixer.sounds[1].muted = true;
        assert_eq!(apps.read("Chrome"), AppSound::Level(Volume { percent: 15, muted: true }));
        // Its sound ends (a paused tab lets it go), then comes back.
        let kept = apps.mixer.sounds.remove(1);
        assert_eq!(apps.read("Chrome"), AppSound::NotFound);
        apps.mixer.sounds.insert(1, kept);
        assert!(matches!(apps.read("Chrome"), AppSound::Level(_)));
    }

    /// An app id no program has: the processes whose windows carry it, asked only then.
    #[test]
    fn windows_name_an_app_no_program_is_called() {
        let mut computer = computer();
        computer.processes.push(process(700, 100, "player64.exe", 80));
        computer.processes.push(process(710, 700, "audiohost.exe", 81));
        computer.sounds.push(sound("Speakers", 710, 0.45, true));
        computer.windows = vec![("Example.Player".into(), 700), ("Example.Player".into(), 99_999)];
        let mut apps = AppVolume::new(computer);
        assert_eq!(apps.read("Example.Player"), AppSound::Level(Volume { percent: 45, muted: false }));
        assert_eq!(apps.mixer.windows_asked, 1);
        apps.read("Chrome");
        assert_eq!(apps.mixer.windows_asked, 1, "a program's name needs no windows");
    }
}
