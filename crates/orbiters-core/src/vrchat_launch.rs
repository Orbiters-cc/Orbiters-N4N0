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

/// Whether VRChats run side by side on this computer, each on its own profile. On a Mac the engine runs one game at a
/// time: the profile is chosen for that one, and the others wait until it was quit.
pub const SEVERAL_HERE: bool = !cfg!(target_os = "macos");

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
    /// The account (its VRChat user id) each profile's own log last said is signed in on it: a label, like its name above, and
    /// never what a private destination goes by (a VRChat that runs says whose it is, [`way`]). Gone as soon as its log says
    /// nobody is.
    #[serde(with = "profile_keys")]
    pub profile_ids: std::collections::BTreeMap<u32, String>,
    /// On a Mac: the share of the display's drawing resolution, per side, Home's Play has VRChat draw at (25 to 100, 100
    /// being the panel's real pixels; macOS scales the picture to the screen). Not used elsewhere.
    pub mac_render_percent: u32,
    /// The default that was when these were saved: a file that says none was saved while it was 58 %, and its 58 was that
    /// default as far as can be told (not a setting).
    pub mac_render_default: u32,
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
            profile_ids: std::collections::BTreeMap::new(),
            mac_render_percent: crate::obelisk::DEFAULT_RENDER_PERCENT,
            mac_render_default: crate::obelisk::DEFAULT_RENDER_PERCENT,
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

/// A start on a VRChat profile (Home's Play with its profile choice, REQUEST15 7a) and, with Home's Where (REQUEST20 2),
/// into a destination: the mode and the options as [`arguments`], then `window` (what a Desktop start adds, see
/// [`desktop_window`]), then `--profile=N` ([`profile_arg`]), and the destination's link last, as [`arguments`] puts it.
/// A destination that is not of VRChat's own grammar ([`is_destination`]) adds nothing.
pub fn arguments_on(mode: LaunchMode, settings: &LaunchSettings, window: Vec<String>, profile: u32, destination: Option<(&str, &str)>) -> Vec<String> {
    let mut out = arguments(mode, settings, None);
    out.extend(window);
    out.extend(profile_arg(profile));
    if let Some((world, instance)) = destination {
        if is_destination(world, instance) {
            out.push(link(world, instance));
        }
    }
    out
}

/// The highest profile VRChat's main start is made on, whoever makes it: Home's choice, its arguments and the bridge's main
/// session (a person keeps a few accounts). The bridge's side sessions have a range of their own (to 99: their OSC ports).
pub const PROFILE_MAX: u32 = 15;

/// `profile` as one the main start is made on: none other is turned into another one, whose account it is not.
pub fn main_profile(profile: u64) -> Option<u32> {
    u32::try_from(profile).ok().filter(|profile| *profile <= PROFILE_MAX)
}

/// What a start on `profile` adds: `--profile=N`, none for the default profile 0 (VRChat's own start without it).
pub fn profile_arg(profile: u32) -> Option<String> {
    (profile != 0).then(|| format!("--profile={}", profile.min(PROFILE_MAX)))
}

/// Whether `arg` is one [`profile_arg`] makes, of a profile other than the default: `--profile=1` to `--profile=15`.
pub fn is_profile_arg(arg: &str) -> bool {
    (1..=PROFILE_MAX).any(|profile| profile_arg(profile).as_deref() == Some(arg))
}

/// Whether anyone may be handed the link of `instance`: a public instance, a group's public one. Told by what it is made of
/// (a kind VRChat adds later is private until it is known here).
pub fn is_public_instance(instance: &str) -> bool {
    use crate::launcher::InstanceAccess;
    let known = |tag: &str| tag == "ageGate" || tag == "groupAccessType(public)" || ["region(", "group("].iter().any(|of| tag.starts_with(of));
    is_instance_id(instance)
        && instance.split('~').skip(1).all(known)
        && matches!(crate::launcher_feed::access(instance).0, InstanceAccess::Public | InstanceAccess::GroupPublic)
}

/// The instance the link in `args` goes to, as [`link`] writes it: its world and its instance.
pub fn destination_of(args: &[String]) -> Option<(&str, &str)> {
    args.iter().rev().find_map(|arg| arg.strip_prefix("vrchat://launch?ref=orbiters.cc&id=")?.split_once(':'))
}

/// Where a destination would go, for [`way`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To<'a> {
    /// A VRChat that is about to be started; whether this computer uses several VRChat profiles ([`several_profiles`]).
    Start { several: bool },
    /// A VRChat that runs, and the account (its user id) its own log proves is the one signed in on it now.
    Running { proven: Option<&'a str> },
}

/// How a destination goes to a VRChat, by whose each is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// With the start, or to the VRChat that runs.
    Direct,
    /// VRChat is started without it; it is delivered once that very VRChat's own log proves the account it is for.
    AfterProof,
    /// Not: the game's account is not known.
    UnknownAccount,
    /// Not: the game's account is not the one it is for.
    OtherAccount,
    /// Not: whom it is for is not known.
    NoOwner,
}

/// The one rule for giving VRChat a destination, on every computer and for every way there is (a start, a start made again, a
/// game that runs). A public one goes anywhere. A `private` one (an instance not everyone may enter, an invite, one made for
/// the account), of the account `owner`, goes with a start only where VRChat has a single profile, as it always did; on a
/// computer with several, never: VRChat starts without it and it is delivered once its account is proven. To a game that
/// runs it goes only while that game's account is proven to be its own.
pub fn way(private: bool, owner: Option<&str>, to: To) -> Way {
    let owner = owner.filter(|id| !id.is_empty());
    match (private, to) {
        (false, _) | (true, To::Start { several: false }) => Way::Direct,
        (true, To::Start { several: true }) => if owner.is_some() { Way::AfterProof } else { Way::NoOwner },
        (true, To::Running { proven }) => match (owner, proven.filter(|id| !id.is_empty())) {
            (None, _) => Way::NoOwner,
            (Some(_), None) => Way::UnknownAccount,
            (Some(owner), Some(game)) if owner == game => Way::Direct,
            _ => Way::OtherAccount,
        },
    }
}

/// Whether the VRChat profiles this computer has records of (known, remembered, running or starting) are more than the default
/// one alone.
pub fn several_profiles(recorded: impl IntoIterator<Item = u32>) -> bool {
    recorded.into_iter().any(|profile| profile != 0)
}

/// The profile a command line names, when it names exactly one (0: none named). `None` where it names several, or one that is
/// no number: which of them the game takes is not known.
pub fn profile_told(args: &[String]) -> Option<u32> {
    let mut told = args.iter().filter(|a| a.as_str() == "--profile" || a.starts_with("--profile="));
    match (told.next(), told.next()) {
        (None, _) => Some(0),
        (Some(one), None) => one.strip_prefix("--profile=").and_then(|p| p.parse().ok()),
        _ => None,
    }
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

    /// REQUEST20 2: Home's Where starts the chosen profile right in the chosen instance, the link last after the profile.
    #[test]
    fn a_start_on_a_profile_puts_the_window_then_the_profile_then_the_link() {
        let w = "wrld_00000000-0000-4000-8000-0000000000aa";
        let off = LaunchSettings { performance: false, ..Default::default() };
        let window = vec!["-screen-width".to_string(), "1280".into(), "-screen-height".into(), "720".into(), "-screen-fullscreen".into(), "0".into()];
        let friends_plus = "48213~hidden(usr_00000000-0000-4000-8000-000000000001)~region(eu)~nonce(0a1b2c3d-0000-4000-8000-000000000000)";
        let args = arguments_on(LaunchMode::Desktop, &off, window.clone(), 2, Some((w, friends_plus)));
        let mut expected = vec!["--no-vr".to_string()];
        expected.extend(window.clone());
        expected.push("--profile=2".into());
        expected.push(format!("vrchat://launch?ref=orbiters.cc&id={w}:{friends_plus}"));
        assert_eq!(args, expected);
        // Profile 0 says no profile; no destination: what Play on a profile always did.
        assert_eq!(arguments_on(LaunchMode::Vr, &off, Vec::new(), 0, Some((w, "12345~region(us)"))), [format!("vrchat://launch?ref=orbiters.cc&id={w}:12345~region(us)")]);
        assert_eq!(arguments_on(LaunchMode::Desktop, &off, Vec::new(), 3, None), ["--no-vr", "--profile=3"]);
        // Still only VRChat's own instance ids: anything else adds nothing at all.
        for bad in ["1 --profile=0", "1\"&x", "", "1~region(eu)&id=wrld_x"] {
            assert_eq!(arguments_on(LaunchMode::Desktop, &off, Vec::new(), 1, Some((w, bad))), ["--no-vr", "--profile=1"], "{bad}");
        }
        assert_eq!(arguments_on(LaunchMode::Desktop, &off, Vec::new(), 1, Some(("wrld_bad", "1"))), ["--no-vr", "--profile=1"]);
        // With the performance options, they come before the window, as Play's always did.
        let on = arguments_on(LaunchMode::Vr, &LaunchSettings::default(), Vec::new(), 1, Some((w, "7~region(jp)")));
        assert_eq!(on.first().map(String::as_str), Some("--process-priority=1"));
        assert_eq!(&on[on.len() - 2..], ["--profile=1".to_string(), format!("vrchat://launch?ref=orbiters.cc&id={w}:7~region(jp)")]);
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
        assert!((1..=15).all(|profile| is_profile_arg(&profile_arg(profile).unwrap())));
        for not in ["--profile=0", "--profile=16", "--profile=02", "--profile=", "--profile=1 ", "--profile=1 --x", "--profile", "profile=1", "-profile=1"] {
            assert!(!is_profile_arg(not), "{not:?}");
        }
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(profile_of(&args(&["VRChat.exe", "--no-vr", "--profile=3"])), 3);
        assert_eq!(profile_of(&args(&["VRChat.exe", "--profile=x"])), 0);
        assert_eq!(profile_of(&args(&["VRChat.exe"])), 0);
    }

    /// A private destination never goes with a start where VRChat has several profiles, and to no game that runs but its
    /// account's own, known by its id; where it has one, it goes with the start as it always did; a public one goes anywhere.
    #[test]
    fn a_private_destination_waits_for_its_account_where_there_are_several_profiles() {
        let (a, b) = (Some("usr_a"), Some("usr_b"));
        for owner in [None, Some(""), a, b] {
            let owned = owner.is_some_and(|o| !o.is_empty());
            for several in [false, true] {
                assert_eq!(way(false, owner, To::Start { several }), Way::Direct);
            }
            assert_eq!(way(true, owner, To::Start { several: false }), Way::Direct, "one profile: as before there were several");
            assert_eq!(way(true, owner, To::Start { several: true }), if owned { Way::AfterProof } else { Way::NoOwner }, "{owner:?}");
            for proven in [None, Some(""), a, b] {
                assert_eq!(way(false, owner, To::Running { proven }), Way::Direct);
                let known = proven.is_some_and(|g| !g.is_empty());
                let want = match (owned, known, owner == proven) {
                    (false, ..) => Way::NoOwner,
                    (_, false, _) => Way::UnknownAccount,
                    (_, true, true) => Way::Direct,
                    (_, true, false) => Way::OtherAccount,
                };
                assert_eq!(way(true, owner, To::Running { proven }), want, "{owner:?} {proven:?}");
            }
        }
        assert!(!several_profiles([]) && !several_profiles([0, 0]));
        assert!(several_profiles([0, 1]) && several_profiles([7]));
        for public in ["12345", "1~region(eu)", "8~group(grp_x)~groupAccessType(public)~region(jp)"] {
            assert!(is_public_instance(public), "{public}");
        }
        for private in [
            "1~hidden(usr_a)~region(eu)",
            "1~friends(usr_a)",
            "1~private(usr_a)~region(eu)",
            "1~private(usr_a)~canRequestInvite",
            "1~group(grp_x)~groupAccessType(plus)",
            "1~group(grp_x)~groupAccessType(members)",
            "1~group(grp_x)",
            "1~region(eu)~nonce(abc)",
            "1~someNewKind(usr_a)~region(eu)",
            "",
        ] {
            assert!(!is_public_instance(private), "{private}");
        }
        // What is really launched says where it goes, and on which profile.
        let w = "wrld_00000000-0000-4000-8000-0000000000aa";
        let args = arguments_on(LaunchMode::Desktop, &LaunchSettings::default(), Vec::new(), 2, Some((w, "1~friends(usr_a)")));
        assert_eq!(destination_of(&args), Some((w, "1~friends(usr_a)")));
        assert_eq!(destination_of(&args[..args.len() - 1]), None);
    }

    /// Two profiles in one command line (a saved flag beside the one asked for) name none.
    #[test]
    fn a_command_line_with_several_profiles_names_none() {
        let told = |a: &[&str]| profile_told(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(told(&["--no-vr"]), Some(0));
        assert_eq!(told(&["--no-vr", "--profile=3"]), Some(3));
        for several in [&["--profile=3", "--profile=2"][..], &["--profile=2", "--profile=2"], &["--profile=x"], &["--profile", "2"]] {
            assert_eq!(told(several), None, "{several:?}");
        }
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
        launch.profile_ids = [(3, "usr_a".to_string())].into();
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
