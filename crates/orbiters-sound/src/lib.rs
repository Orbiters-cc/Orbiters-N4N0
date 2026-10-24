//! Sound for Orbiters - N4N0: the interface sounds (computed, not shipped as files), a player on the default output
//! device and the microphone monitor of the settings' mic test.

mod device;
mod fft;
pub mod sounds;
pub mod synth;

pub use device::{Duplex, MicMonitor, Output, RATE};
pub use synth::{render, Sound};
