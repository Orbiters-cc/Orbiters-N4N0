//! Linux: the sounds played, the microphone's monitor and a call's two ways. Nothing yet (the port's sound stream, L3).

use super::{Buffering, Command, Shared};
use std::collections::VecDeque;
use std::sync::{mpsc, Mutex};

pub fn output(rx: mpsc::Receiver<Command>) {
    while rx.recv().is_ok() {}
}

pub fn run(
    _device: Option<&str>,
    shared: &Shared,
    _on_mic: &mut dyn FnMut(&[f32]),
    _playback: &Mutex<VecDeque<f32>>,
    _buffering: Buffering,
    _lenient: bool,
) {
    shared.fail(orbiters_i18n::tr("Sound devices are not used on Linux yet.").into());
}
