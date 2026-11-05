//! Orbiters - N4N0 on a phone, whichever the phone: N4N0's runtime (`orbiters_n4n0`) talking with VRChat on a Quest over
//! the Wi-Fi (`orbiters_core::osc`'s LAN mode), behind a [`app::Snapshot`] for each app's interface.
//!
//! What differs between phones goes through [`phone::Phone`] and [`app::Config`]; each app (Orbiters-N4N0-Android,
//! Orbiters-N4N0-iOS) brings those, its interface, and whatever keeps the core running there.

pub mod app;
pub mod connection;
pub mod format;
pub mod phone;
pub mod prefs;
pub mod signin;
