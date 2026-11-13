//! Live translation of the text seen in VR (Orbiters - N4N0's orbit menu, Translation). The overlay process reads the
//! headset's eye pictures a few times a second on a low-priority thread; this crate does the rest without SteamVR, so
//! that every step can be tested on still pictures:
//!
//! - [`ocr`]: lines of text found and read (PaddleOCR's PP-OCR models through ONNX Runtime), [`models`] downloads them,
//! - [`geometry`] and [`place`]: rays from an eye's pixels into the room, a line's distance from both eyes, its plane,
//! - [`track`]: lines followed from look to look, so that a translation stays put,
//! - [`language`]: which lines need translating at all,
//! - [`provider`]: who translates (an AI server on this computer or the network, Orbiters), [`builtin`] the model
//!   built in for a computer without one,
//! - [`paint`]: a translation drawn as the world's own text,
//! - [`pipeline`] and [`core`]: one look from pictures to quads, and the whole of it with the translator's thread,
//! - [`picture`]: pictures, text drawn with the interface's fonts, stand-in eyes for the tests.

pub mod builtin;
pub mod core;
pub mod geometry;
pub mod language;
pub mod models;
pub mod ocr;
pub mod paint;
pub mod picture;
pub mod pipeline;
pub mod place;
pub mod provider;
pub mod track;
