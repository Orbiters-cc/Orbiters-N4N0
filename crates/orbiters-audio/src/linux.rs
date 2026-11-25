//! Linux: microphones and VRChat's own sound. Nothing yet (the port's sound stream, L3: PulseAudio's protocol, which PipeWire serves).

use super::{AudioSource, Device, Ended, Format, Options, Packet};
use std::sync::atomic::AtomicBool;

pub fn list_mics() -> Vec<Device> {
    Vec::new()
}

pub fn default_mic_id() -> Option<String> {
    None
}

pub fn capture_packets(
    _source: &AudioSource,
    _options: &Options,
    _stop: &AtomicBool,
    _ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
    _packet: impl FnMut(&Packet<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<(String, Ended)> {
    anyhow::bail!("{}", orbiters_i18n::tr("Sound is not recorded on Linux yet."))
}
