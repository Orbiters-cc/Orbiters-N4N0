//! N4N0's runtime (contracts `Docs/n4n0.md`), shared by Orbiters - N4N0 on Windows and the Android app: the companion on
//! the avatar (its text panel, its face, the AFK message, held and carried parameters, saved videos), its assistant's
//! turns, its menu on the avatar's projected screen and in the apps' interfaces ([`menu_host`]), the avatar's saved
//! states, link cards, its ears, and the music it shows.
//!
//! What each platform does its own way stays behind small traits ([`platform`]): the app itself ([`Host`]: settings,
//! what it knows of VRChat, where N4N0's state goes, its own features), OSC with VRChat ([`Osc`]), the clipboard, the
//! browser, sounds, the language, the ears ([`Ears`]), desktop mode's mouse, the music ([`media::MediaBackend`]) and
//! where N4N0 keeps its files ([`Files`]). The desktop implements them with Windows' own; TLS, the folders and the
//! account's credential are `orbiters_core`'s, per platform.

mod captures;
mod command;
mod feedback;
pub mod input;
pub mod media;
pub mod menu_host;
mod mirror;
mod pointer_log;
pub mod platform;
mod qr;
mod runtime;
mod screen;
mod states;
pub mod status;
mod voice;

#[cfg(test)]
mod tests;

pub use command::Command;
pub use platform::{
    language_of, Actions, Browser, CallRequest, Clipboard, Ears, Files, Host, Locale, Mouse, MouseState, NoMouse, Osc, Platform, QrEvents, QrScan, QrSeen,
    Scene, Sounds, VoiceEnded, VoiceEvents, VoiceWorker,
};
pub use runtime::{N4n0, Shared, Source};
pub use status::NoticeKind;
pub use voice::LISTEN_LIMIT;

/// N4N0's menu helpers the interfaces share: the settings' keyboard and 3D picture lengths as the menu takes them.
pub mod menu {
    pub use crate::screen::{keyboard, photo_length, photo_setting};
}

/// Unix time in milliseconds.
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
