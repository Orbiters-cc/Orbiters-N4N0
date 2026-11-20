//! How Home's "Play in VR" / "Play in Desktop" start VRChat: the launch options (REQUEST10's performance set, each one
//! visible and editable in settings) and the command line, as pure functions. Starting the process is the desktop's
//! (`controller/launcher.rs`): through Steam (`steam.exe -applaunch 438100 …`), as a person does, so that Easy Anti-Cheat
//! starts as ever.
//!
//! VRChat's documented options (docs.vrchat.com/docs/launch-options): `--no-vr`, `--fps=N`, `--process-priority=N` and
//! `--main-thread-priority=N` (-2 idle … 2 high; VRChat warns to use them with care), `--enable-hw-video-decoding`.
//! `-screen-width`, `-screen-height`, `-screen-fullscreen` are Unity's own player options (the desktop window's size).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::launcher::LaunchMode;

/// Whether this computer can play in VR. A Mac has no SteamVR: VRChat there (through Obelisk, [`crate::obelisk`]) is
/// the desktop game, and the Play buttons are one.
pub const VR_HERE: bool = !cfg!(target_os = "macos");

/// Steam's id of VRChat.
pub const STEAM_APP: &str = "438100";

/// The profiles map with its numbers read from text keys. JSON writes a map's keys as text, and a message that is read
/// in two steps (a tagged one, as the service's are for the window and the overlay) hands those keys over as text:
/// read plainly as numbers they are refused ("invalid type: string \"0\", expected u32"), which ended the overlay and
/// the window at every start once a profile had been seen (0.3.6). A key that is not a profile's number is dropped.
mod profile_keys {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(map: &BTreeMap<u32, String>, to: S) -> Result<S::Ok, S::Error> {
        map.iter().map(|(profile, account)| (profile.to_string(), account)).collect::<BTreeMap<_, _>>().serialize(to)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<BTreeMap<u32, String>, D::Error> {
        let read = BTreeMap::<String, String>::deserialize(from)?;
        Ok(read.into_iter().filter_map(|(profile, account)| Some((profile.trim().parse().ok()?, account))).collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchSettings {
    /// The one switch: "Performance launch options" (on by default). Off: VRChat starts with none of the options below.
    pub performance: bool,
    /// `--process-priority=N`: 1 is "above normal". 2 (high) can starve SteamVR and Virtual Desktop and make the
    /// headset's picture stutter, so it is not offered above 1 by default.
    pub process_priority: i8,
    /// `--main-thread-priority=N`: VRChat's Unity main thread, the same scale.
    pub main_thread_priority: i8,
    /// `--fps=N`: the frame cap. In VR the headset's refresh paces the frames; the cap counts on the desktop.
    pub fps: u32,
    /// `--enable-hw-video-decoding`: videos decoded by the graphics card instead of the processor.
    pub hw_video_decoding: bool,
    /// In VR only: a tiny desktop window (`-screen-width 100 -screen-height 100 -screen-fullscreen 0`), so the mirror on
    /// the monitor costs almost nothing. Never in Desktop mode, where that window is the game.
    pub small_window_in_vr: bool,
    /// That window's size.
    pub window_width: u32,
    pub window_height: u32,
    /// The mode last played (what the page lights first).
    pub last_mode: LaunchMode,
    /// VRChat's own saved desktop window, as it was before the last Play in VR left the tiny one as VRChat's saved size:
    /// what Play in Desktop asks for then (not a setting; written by Play).
    pub desktop_window: Option<SavedWindow>,
    /// VRChat's profiles used on this computer and the account last seen signed in on each (from VRChat's logs), so
    /// Home's profile choice says whose each one is (REQUEST15 7a; not a setting, written as they are seen).
    #[serde(with = "profile_keys")]
    pub profiles: std::collections::BTreeMap<u32, String>,
}

impl Default for LaunchSettings {
    fn default() -> Self {
        Self {
            performance: true,
            process_priority: 1,
            main_thread_priority: 1,
            fps: 90,
            hw_video_decoding: true,
            small_window_in_vr: true,
            window_width: 100,
            window_height: 100,
            last_mode: LaunchMode::Vr,
            desktop_window: None,
            profiles: std::collections::BTreeMap::new(),
        }
    }
}

impl LaunchSettings {
    /// The options as VRChat takes them, in this mode.
    pub fn options(&self, mode: LaunchMode) -> Vec<String> {
        if !self.performance {
            return Vec::new();
        }
        let mut out = vec![
            format!("--process-priority={}", self.process_priority.clamp(-2, 2)),
            format!("--main-thread-priority={}", self.main_thread_priority.clamp(-2, 2)),
        ];
        if self.fps > 0 {
            out.push(format!("--fps={}", self.fps.clamp(10, 1000)));
        }
        if self.hw_video_decoding {
            out.push("--enable-hw-video-decoding".into());
        }
        if mode == LaunchMode::Vr && self.small_window_in_vr {
            for (k, v) in [("-screen-width", self.window_width.clamp(64, 8192)), ("-screen-height", self.window_height.clamp(64, 8192))] {
                out.push(k.into());
                out.push(v.to_string());
            }
            out.push("-screen-fullscreen".into());
            out.push("0".into());
        }
        out
    }
}

/// Unity's saved window: its size and `Fullscreen mode` (0 exclusive, 1 full-screen window, 2 maximised, 3 windowed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub width: u32,
    pub height: u32,
    pub mode: u32,
}

impl SavedWindow {
    /// A window someone plays in, not the tiny one Play in VR asks for.
    pub fn normal(&self) -> bool {
        self.width >= 640 && self.height >= 480
    }
}

/// What a Desktop start adds so that it never opens in the tiny window a VR start left as VRChat's saved size: nothing
/// while the saved window is a normal one; else the window kept before the VR start, or 1280 x 720 in a window.
pub fn desktop_window(saved: Option<SavedWindow>, kept: Option<SavedWindow>) -> Vec<String> {
    if saved.is_none_or(|w| w.normal()) {
        return Vec::new();
    }
    let w = kept.filter(SavedWindow::normal).unwrap_or(SavedWindow { width: 1280, height: 720, mode: 3 });
    // Unity's modes: 0 exclusive and 1 full-screen window are full screen; 2 maximised and 3 windowed are windows (a
    // maximised one comes back as a window of its size, never as full screen).
    let fullscreen = if matches!(w.mode, 2 | 3) { "0" } else { "1" };
    ["-screen-width".to_string(), w.width.to_string(), "-screen-height".into(), w.height.to_string(), "-screen-fullscreen".into(), fullscreen.into()].into()
}

/// An instance id as VRChat writes it after the world's id: a name of letters and digits, then `~tag` or `~tag(value)`
/// parts (`77505~group(grp_…)~groupAccessType(public)~region(eu)`, `1~private(usr_…)~canRequestInvite~nonce(…)`). Nothing
/// else: no space, quote, `&`, `%`, `:` or `?` can reach VRChat's link pipe, its launch link or `launch.exe`'s command line.
pub fn is_instance_id(instance: &str) -> bool {
    if instance.is_empty() || instance.len() > 512 {
        return false;
    }
    let mut parts = instance.split('~');
    let name = parts.next().unwrap_or_default();
    let tag = |t: &str| !t.is_empty() && t.len() <= 32 && t.bytes().all(|b| b.is_ascii_alphabetic());
    let value = |v: &str| !v.is_empty() && v.len() <= 128 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b));
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric())
        && parts.all(|p| match p.split_once('(') {
            None => tag(p),
            Some((t, rest)) => tag(t) && rest.strip_suffix(')').is_some_and(value),
        })
}

/// A world and instance that may be handed to VRChat: both of VRChat's own grammar.
pub fn is_destination(world: &str, instance: &str) -> bool {
    crate::vrchat_log::is_world_id(world) && is_instance_id(instance)
}

/// VRChat's launch link of an instance, as its website writes it.
pub fn link(world: &str, instance: &str) -> String {
    format!("vrchat://launch?ref=orbiters.cc&id={world}:{instance}")
}

/// What follows `steam.exe -applaunch 438100`: the mode, the options, then the destination's link when there is one.
/// Desktop mode is `--no-vr`; VR mode names nothing (VRChat starts in VR with SteamVR).
pub fn arguments(mode: LaunchMode, settings: &LaunchSettings, destination: Option<(&str, &str)>) -> Vec<String> {
    let mut out = Vec::new();
    if mode == LaunchMode::Desktop {
        out.push("--no-vr".to_string());
    }
    out.extend(settings.options(mode));
    if let Some((world, instance)) = destination {
        // Only what an instance is made of (its number and tags): no space, quote or link character can become an
        // option of its own on the command line Steam builds again.
        if is_destination(world, instance) {
            out.push(link(world, instance));
        }
    }
    out
}

/// The highest profile number Home offers (VRChat takes any; a person keeps a few accounts).
pub const PROFILE_MAX: u32 = 15;

/// What a start on `profile` adds: `--profile=N`, none for the default profile 0 (VRChat's own start without it).
pub fn profile_arg(profile: u32) -> Option<String> {
    (profile != 0).then(|| format!("--profile={}", profile.min(PROFILE_MAX)))
}

/// VRChat's `--profile` in a command line (0 without one).
pub fn profile_of(args: &[String]) -> u32 {
    args.iter().find_map(|a| a.strip_prefix("--profile=").and_then(|p| p.trim().parse().ok())).unwrap_or(0)
}

/// The Steam library folders that hold VRChat, from Steam's `steamapps/libraryfolders.vdf`.
pub fn libraries_with_vrchat(vdf: &str) -> Vec<PathBuf> {
    // The file is nested `"key" "value"` pairs and `"key" { … }` blocks; a library is a block with a "path" and an
    // "apps" block whose keys are app ids.
    let tokens: Vec<String> = {
        let mut out = Vec::new();
        let mut chars = vdf.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let mut s = String::new();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                if let Some(n) = chars.next() {
                                    s.push(n);
                                }
                            }
                            '"' => break,
                            c => s.push(c),
                        }
                    }
                    out.push(s);
                }
                '{' => out.push("{".into()),
                '}' => out.push("}".into()),
                _ => {}
            }
        }
        out
    };
    let mut found = Vec::new();
    let (mut path, mut in_apps, mut depth, mut apps_depth) = (None::<String>, false, 0usize, 0usize);
    let mut has_vrchat = false;
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "{" => depth += 1,
            "}" => {
                if in_apps && depth == apps_depth {
                    in_apps = false;
                }
                if depth == 2 {
                    if let (Some(p), true) = (path.take(), has_vrchat) {
                        found.push(PathBuf::from(p));
                    }
                    has_vrchat = false;
                }
                depth = depth.saturating_sub(1);
            }
            key => {
                let next = tokens.get(i + 1).map(String::as_str);
                if next == Some("{") {
                    if key.eq_ignore_ascii_case("apps") {
                        in_apps = true;
                        apps_depth = depth + 1;
                    }
                } else if let Some(value) = next {
                    if in_apps && key == STEAM_APP {
                        has_vrchat = true;
                    } else if !in_apps && key.eq_ignore_ascii_case("path") {
                        path = Some(value.to_string());
                    }
                    i += 1;
                }
            }
        }
        i += 1;
    }
    found
}

/// VRChat's folder in a Steam installation at `steam` (`…\Steam`), when Steam says it is installed and the game is there.
pub fn vrchat_dir(steam: &Path) -> Option<PathBuf> {
    let vdf = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).ok()?;
    libraries_with_vrchat(&vdf).into_iter().map(|l| l.join("steamapps").join("common").join("VRChat")).find(|d| d.join("launch.exe").is_file() || d.join("VRChat.exe").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_performance_options_are_the_requests_and_the_small_window_is_vr_only() {
        let s = LaunchSettings::default();
        assert_eq!(
            arguments(LaunchMode::Vr, &s, None),
            ["--process-priority=1", "--main-thread-priority=1", "--fps=90", "--enable-hw-video-decoding", "-screen-width", "100", "-screen-height", "100", "-screen-fullscreen", "0"]
        );
        assert_eq!(arguments(LaunchMode::Desktop, &s, None), ["--no-vr", "--process-priority=1", "--main-thread-priority=1", "--fps=90", "--enable-hw-video-decoding"]);
        let off = LaunchSettings { performance: false, ..s.clone() };
        assert_eq!(arguments(LaunchMode::Desktop, &off, None), ["--no-vr"]);
        assert!(arguments(LaunchMode::Vr, &off, None).is_empty());
        let edited = LaunchSettings { fps: 72, process_priority: 9, hw_video_decoding: false, small_window_in_vr: false, ..s };
        assert_eq!(arguments(LaunchMode::Vr, &edited, None), ["--process-priority=2", "--main-thread-priority=1", "--fps=72"]);
    }

    #[test]
    fn a_destination_goes_last_as_vrchats_link() {
        let w = "wrld_00000000-0000-4000-8000-0000000000aa";
        let args = arguments(LaunchMode::Desktop, &LaunchSettings { performance: false, ..Default::default() }, Some((w, "777~friends(usr_x)~region(jp)")));
        assert_eq!(args, ["--no-vr".to_string(), format!("vrchat://launch?ref=orbiters.cc&id={w}:777~friends(usr_x)~region(jp)")]);
        let bad = arguments(LaunchMode::Desktop, &LaunchSettings { performance: false, ..Default::default() }, Some((w, "1 --evil")));
        assert_eq!(bad, ["--no-vr"], "nothing that would split into another option");
        for bad in ["1\"--evil", "1&id=wrld_x", "1%20x", "1^x", "1?x", "1#x", ""] {
            assert_eq!(arguments(LaunchMode::Desktop, &LaunchSettings { performance: false, ..Default::default() }, Some((w, bad))), ["--no-vr"], "{bad}");
        }
        let group = "12345~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(plus)~canRequestInvite~region(use)~nonce(ab12)";
        assert_eq!(arguments(LaunchMode::Vr, &LaunchSettings { performance: false, ..Default::default() }, Some((w, group))).len(), 1);
        assert_eq!(arguments(LaunchMode::Vr, &LaunchSettings { performance: false, ..Default::default() }, Some(("wrld_bad", "1"))), Vec::<String>::new());
    }

    #[test]
    fn a_desktop_start_after_the_tiny_vr_window_gets_a_normal_window() {
        let tiny = SavedWindow { width: 100, height: 100, mode: 3 };
        let full = SavedWindow { width: 2560, height: 1440, mode: 1 };
        assert!(desktop_window(Some(full), None).is_empty(), "VRChat's own saved size is kept");
        assert!(desktop_window(None, None).is_empty(), "nothing known: nothing passed");
        assert_eq!(desktop_window(Some(tiny), Some(full)), ["-screen-width", "2560", "-screen-height", "1440", "-screen-fullscreen", "1"]);
        assert_eq!(desktop_window(Some(tiny), None), ["-screen-width", "1280", "-screen-height", "720", "-screen-fullscreen", "0"]);
        assert_eq!(desktop_window(Some(tiny), Some(tiny)), ["-screen-width", "1280", "-screen-height", "720", "-screen-fullscreen", "0"]);
        let maximised = SavedWindow { width: 2560, height: 1369, mode: 2 };
        assert_eq!(desktop_window(Some(tiny), Some(maximised)), ["-screen-width", "2560", "-screen-height", "1369", "-screen-fullscreen", "0"], "a window, not full screen");
    }

    #[test]
    fn only_vrchats_own_instance_ids_pass() {
        for good in ["1", "77505~region(eu)", "12345~private(usr_00000000-0000-4000-8000-000000000001)~canRequestInvite~region(use)~nonce(0a1b2c3d-0000-4000-8000-000000000000)", "8~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(plus)~region(jp)", "MyRoom~hidden(usr_x)"] {
            assert!(is_instance_id(good), "{good}");
        }
        for bad in ["", "1 --profile=3", "1&shortName=x", "1~region(eu)&id=wrld_x", "1~region(\"eu\")", "1~region(e u)", "1%20", "1:2", "1~", "~region(eu)", "1~region(eu", "1~region()", "1~re-gion(eu)", "1?x", "1
"] {
            assert!(!is_instance_id(bad), "{bad:?}");
        }
        assert!(!is_instance_id(&"1".repeat(65)));
        let w = "wrld_00000000-0000-4000-8000-0000000000aa";
        assert!(is_destination(w, "1~region(eu)") && !is_destination("wrld_bad", "1") && !is_destination(w, "1\"&"));
    }

    #[test]
    fn steams_library_file_says_where_vrchat_is() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"228980"		"0"
		}
	}
	"1"
	{
		"path"		"G:\\SteamLibrary"
		"apps"
		{
			"438100"		"123456"
			"250820"		"1"
		}
	}
}"#;
        assert_eq!(libraries_with_vrchat(vdf), [PathBuf::from(r"G:\SteamLibrary")]);
        assert!(libraries_with_vrchat("garbage {").is_empty());
    }

    #[test]
    fn a_profile_is_said_only_when_it_is_not_the_default() {
        assert_eq!(profile_arg(0), None);
        assert_eq!(profile_arg(2).as_deref(), Some("--profile=2"));
        assert_eq!(profile_arg(99).as_deref(), Some("--profile=15"), "never past what Home offers");
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(profile_of(&args(&["VRChat.exe", "--no-vr", "--profile=3"])), 3);
        assert_eq!(profile_of(&args(&["VRChat.exe", "--profile=x"])), 0);
        assert_eq!(profile_of(&args(&["VRChat.exe"])), 0);
    }

    /// The service's messages are tagged, so what they carry is read from kept values, where a map's keys are text.
    #[test]
    fn the_profiles_seen_survive_a_tagged_message() {
        #[derive(Serialize, Deserialize)]
        #[serde(tag = "event", rename_all = "snake_case")]
        enum Message {
            State { launch: LaunchSettings },
        }
        let mut launch = LaunchSettings::default();
        launch.profiles = [(0, "Sample Me".to_string()), (3, "Sample Alt".to_string())].into();
        let text = serde_json::to_string(&Message::State { launch: launch.clone() }).unwrap();
        assert!(text.contains(r#""profiles":{"0":"Sample Me","3":"Sample Alt"}"#), "the file's form is unchanged: {text}");
        let Message::State { launch: back } = serde_json::from_str(&text).unwrap();
        assert_eq!(back, launch);
        let odd: LaunchSettings = serde_json::from_str(r#"{"profiles": {"2": "Sample Alt", "x": "nobody"}}"#).unwrap();
        assert_eq!(odd.profiles, [(2, "Sample Alt".to_string())].into(), "a key that is no number is dropped, the rest kept");
    }

    #[test]
    fn old_settings_files_get_the_defaults() {
        let s: LaunchSettings = serde_json::from_str(r#"{"fps": 120}"#).unwrap();
        assert_eq!((s.fps, s.performance, s.process_priority), (120, true, 1));
    }
}
