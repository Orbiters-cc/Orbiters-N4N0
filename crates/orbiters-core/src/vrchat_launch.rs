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

/// Steam's id of VRChat.
pub const STEAM_APP: &str = "438100";

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
        if crate::vrchat_log::is_world_id(world) && !instance.is_empty() && crate::vrchat_log::is_instance(instance) {
            out.push(link(world, instance));
        }
    }
    out
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
    fn old_settings_files_get_the_defaults() {
        let s: LaunchSettings = serde_json::from_str(r#"{"fps": 120}"#).unwrap();
        assert_eq!((s.fps, s.performance, s.process_priority), (120, true, 1));
    }
}
