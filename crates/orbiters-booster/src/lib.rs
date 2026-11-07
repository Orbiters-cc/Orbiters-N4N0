//! The FPS booster without its interface. It spends things in three steps (other programs' speed, sharpness, how other
//! people look) only where a measurement says that gained real frames, and it puts every change back exactly, after a
//! crash, a power loss or an uninstall.
//!
//! The rule that shapes all of it: **nothing in, on or around VRChat's process.** No priority or affinity change on it,
//! no reading its memory, no injection, no files of VRChat's written, nothing of its anti-cheat's touched
//! ([`programs::never_touch`]). What the booster changes is other programs' priority and SteamVR's own settings, through
//! SteamVR's public interface. The rest VRChat offers in its own menus, so the person changes those there, by hand.
//!
//! This crate knows no SteamVR and no window, so that the app, its overlay and the setup can share it and tests can run
//! it without either. It holds [`journal`], the record every change is written to before it happens, [`frames`] and
//! [`classify`], what a frame says and what limits this PC, [`trial`], the comparison that keeps or undoes a change,
//! [`engine`], which decides, and [`programs`], [`vrsettings`], [`vrchat_read`] and [`log`], what touches the system.

use serde::{Deserialize, Serialize};

pub mod classify;
pub mod engine;
pub mod frames;
pub mod guided;
pub mod journal;
pub mod log;
pub mod programs;
pub mod steamvr;
pub mod system;
pub mod trial;
pub mod vrchat_read;
pub mod vrsettings;

/// How far the booster may go. Each step includes the ones before it, and the names say what is being spent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Nothing changes. The frame logger still runs.
    #[default]
    Off,
    /// Other programs' speed.
    Background,
    /// Sharpness, and waiting for 3D pictures.
    Picture,
    /// How other people look to you: by hand, in VRChat's own menus.
    Avatars,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Off, Level::Background, Level::Picture, Level::Avatars];
}

/// `%LOCALAPPDATA%\Orbiters\booster`, where the journal is. Beside the app's data on purpose: not in
/// `%LOCALAPPDATA%\Orbiters\Desktop`, which an uninstall that removes data takes away, and the journal must outlive that
/// while it has entries.
pub fn data_dir() -> std::path::PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Orbiters").join("booster")
}

/// When this happened, for the journal and the logs: only ever read by people.
pub(crate) fn now_text() -> String {
    time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}
