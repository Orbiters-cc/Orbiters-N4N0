//! Linux's own parts of the app, beside `mac.rs` and Windows' code: each a file of the stream of the port that fills it
//! (`orbiters_core::here` says what works). Other files of this folder are taken by their module with `#[path]`.

pub mod open;
pub mod startup;
pub mod tray;
pub mod update;
