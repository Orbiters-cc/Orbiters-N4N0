//! `--only settings-vr-audio`: Settings' "When SteamVR starts" with stand-in sound devices (never this computer's),
//! the headset's speakers and microphone chosen while SteamVR runs, and the last change in its subtitle.

use super::{Page, Scenario};
use crate::audio_devices::{self, AudioEndpoint};
use crate::controller::vr_audio::Event;
use orbiters_core::settings::VrAudioSettings;

fn ep(id: &str, name: &str) -> AudioEndpoint {
    AudioEndpoint { id: id.into(), name: name.into() }
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let index = ep("{0.0.0.00000000}.{index}", "Speakers (Valve Index)");
    let index_mic = ep("{0.0.1.00000000}.{index}", "Microphone (Valve Index)");
    audio_devices::stand_in(
        vec![ep("{0.0.0.00000000}.{realtek}", "Speakers (Realtek(R) Audio)"), ep("{0.0.0.00000000}.{vd}", "Speakers (Virtual Desktop Audio)"), index.clone()],
        vec![ep("{0.0.1.00000000}.{usb}", "Microphone (Yeti Stereo Microphone)"), ep("{0.0.1.00000000}.{vd}", "Microphone (Virtual Desktop Audio)"), index_mic.clone()],
    );
    let mut snapshot = scenarios[0].snapshot.clone();
    snapshot.settings.vr_audio = VrAudioSettings { output: Some(index), input: Some(index_mic), restore: true };
    snapshot.steamvr.running = true;
    snapshot.steamvr.pid = Some(4242);
    let minute_ago = crate::controller::unix_ms().saturating_sub(60_000);
    snapshot.vr_audio.last = Some(Event { utc_ms: minute_ago, text: "Set to Speakers (Valve Index) and Microphone (Valve Index)".into(), error: false });
    scenarios.push(Scenario {
        name: "settings-vr-audio",
        page: Page::Settings,
        snapshot,
        frames: 120,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll: 3020.0,
        world: None,
        home: None,
        library: None,
    });
}
