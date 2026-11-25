//! What this computer's Orbiters - N4N0 can do, by name: the one place features, pages and settings ask, instead of "it is
//! not a Mac" (which on Linux claimed Windows' code for what nothing does yet). Linux says only what its code does; a
//! stream of the Linux port turns its own on in [`Here::LINUX`] when it lands, and nothing else asks about it.
//!
//! A capability is about this build on this system, never about this moment: whether SteamVR runs, VRChat is installed
//! or a device is there is asked elsewhere.

/// What the build for one system can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Here {
    /// VRChat runs on this computer beside the app: its process, its log and its OSC are looked for here. (On a Mac it is a
    /// VRChat on the network, [`crate::lan`], or Obelisk's engine's, [`crate::obelisk`].)
    pub vrchat_runs: bool,
    /// Home starts VRChat (Play, and everything that starts it to go somewhere).
    pub starts_vrchat: bool,
    /// VRChat can be played in VR on this computer: the Play buttons are two.
    pub play_vr: bool,
    /// VRChats run side by side, each on its own profile.
    pub several_games: bool,
    /// A running VRChat can be handed an instance's link (Windows' named pipe).
    pub link_pipe: bool,
    /// SteamVR is talked with (`orbiters_openvr::Client`): poses, its chaperone (fast walk through the play space), its timing.
    pub steam_vr: bool,
    /// N4N0's panels and the 3D worlds show in the headset.
    pub vr_overlay: bool,
    /// The SteamVR driver (tracking smoothing, fast walk through it) can be installed.
    pub driver: bool,
    /// What the headset's eyes see can be read (translation in VR, the camera tiles).
    pub eye_picture: bool,
    /// VRChat's stream camera can be read (Spout): 3D pictures, captures, the QR scanner.
    pub camera_frames: bool,
    /// N4N0's panels show over VRChat's window on the desktop.
    pub desktop_panels: bool,
    /// The text VRChat's window shows can be read (translation on the desktop).
    pub window_text: bool,
    /// N4N0 can move this computer's mouse, for VRChat's desktop mode.
    pub mouse: bool,
    /// The beta's menu mismatch buttons show (on the watch's wrist, in VRChat's window's corner).
    pub mismatch_buttons: bool,
    /// Web pages show inside the app (the camera's links).
    pub web_pages: bool,
    /// Pictures can be dragged out of the app into others.
    pub drag_out: bool,
    /// The app keeps an icon in the system's tray (or menu bar): closing the window is not quitting.
    pub tray: bool,
    /// The app can start with the session, and with SteamVR.
    pub autostart: bool,
    /// Microphones are listed and recorded, sounds played.
    pub sound: bool,
    /// What plays music on this computer is shown and steered.
    pub media: bool,
    /// The booster lowers other programs and says what they use.
    pub booster_programs: bool,
    /// What VRChat, the app and the computer use is measured (the performance report, its self-check).
    pub performance: bool,
    /// Updates are downloaded and put in place.
    pub updates: bool,
    /// The VRChat bridge (`orbiters-desktop/src/bridge`).
    pub bridge: bool,
}

impl Here {
    pub const WINDOWS: Self = Self {
        vrchat_runs: true,
        starts_vrchat: true,
        play_vr: true,
        several_games: true,
        link_pipe: true,
        steam_vr: true,
        vr_overlay: true,
        driver: true,
        eye_picture: true,
        camera_frames: true,
        desktop_panels: true,
        window_text: true,
        mouse: true,
        mismatch_buttons: true,
        web_pages: true,
        drag_out: true,
        tray: true,
        autostart: true,
        sound: true,
        media: true,
        booster_programs: true,
        performance: true,
        updates: true,
        bridge: true,
    };

    pub const MAC: Self = Self {
        vrchat_runs: false,
        starts_vrchat: true,
        play_vr: false,
        several_games: false,
        link_pipe: false,
        steam_vr: false,
        vr_overlay: false,
        driver: false,
        eye_picture: false,
        camera_frames: false,
        desktop_panels: true,
        window_text: false,
        mouse: false,
        mismatch_buttons: false,
        web_pages: false,
        drag_out: true,
        tray: true,
        autostart: true,
        sound: true,
        media: true,
        booster_programs: false,
        performance: false,
        updates: true,
        bridge: true,
    };

    /// VRChat runs here, under Proton ([`crate::proton`]); what talks with it over OSC and reads its log and files works.
    /// Everything else waits for its stream of the port (`plan-linux.md`, section 6), named beside it.
    pub const LINUX: Self = Self {
        vrchat_runs: true,
        starts_vrchat: false, // L1
        play_vr: true,
        several_games: false,    // L1, unknown under Proton
        link_pipe: false,        // never: a Wine pipe is not reachable from outside
        steam_vr: false,         // L4
        vr_overlay: false,       // L4, the worlds L5
        driver: false,           // L6
        eye_picture: false,      // L4, unknown
        camera_frames: false,    // L8
        desktop_panels: false,   // L7
        window_text: false,      // L7
        mouse: false,            // L7
        mismatch_buttons: false, // L4, L7
        web_pages: false,        // L13 (the link opens in the browser)
        drag_out: false,         // L2
        tray: false,             // L2
        autostart: false,        // L2
        sound: false,            // L3
        media: false,            // L3
        booster_programs: false, // never
        performance: false,      // L11
        updates: false,          // L12
        bridge: false,           // after its move
    };

    /// Whether panels can show around VRChat's menus: in the headset, or over its window.
    pub const fn around(self) -> bool {
        self.vr_overlay || self.desktop_panels
    }

    /// Whether what is seen can be read (translation): the headset's picture, or VRChat's window.
    pub const fn reads(self) -> bool {
        self.eye_picture || self.window_text
    }
}

/// This build's. (The phone's app asks none of these; it keeps what "not a Mac" said.)
pub const HERE: Here = if cfg!(target_os = "macos") {
    Here::MAC
} else if cfg!(target_os = "linux") {
    Here::LINUX
} else {
    Here::WINDOWS
};

/// Whether this build is Linux's (the phone's Android is not). For the words and looks that are a third kind beside Windows'
/// and the Mac's; what the app can do asks [`HERE`].
pub const LINUX: bool = cfg!(target_os = "linux");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_build_has_its_own_system_s_capabilities() {
        let expected = if cfg!(windows) {
            Here::WINDOWS
        } else if cfg!(target_os = "macos") {
            Here::MAC
        } else {
            Here::LINUX
        };
        assert_eq!(HERE, expected);
        assert_eq!(LINUX, cfg!(target_os = "linux"));
    }

    /// Windows has everything, as it always had: adding a capability there as `false` would take a feature away.
    #[test]
    fn windows_has_everything() {
        let w = format!("{:?}", Here::WINDOWS);
        assert!(!w.contains("false"), "{w}");
    }

    /// What Linux must never claim before its code exists: each is a feature that would look there and do nothing.
    #[test]
    fn linux_claims_only_what_its_code_does() {
        let l = Here::LINUX;
        assert!(l.vrchat_runs);
        assert!(!l.link_pipe, "a Wine pipe lives in wineserver");
        assert!(!l.bridge, "off on Linux (the owner's decision)");
        assert!(!l.vr_overlay || l.steam_vr, "the overlay talks with SteamVR");
        assert!(!l.eye_picture || l.steam_vr);
    }
}
