//! The performance streams as both interfaces show them (the window's Capture page and the SteamVR tab): what
//! each one records right now, its level, and its switch (which applies to the next recording).

use crate::state::Snapshot;
use egui::{vec2, Id, Rect, Ui};
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::widgets::Tone;
use orbiters_ui::{format, icons, theme};

pub struct Stream {
    pub glyph: &'static str,
    pub name: &'static str,
    pub status: String,
    pub tone: Tone,
    /// Peak level while recording (sound).
    pub level: Option<f32>,
    /// The switch's state, when it has one.
    pub switch: Option<bool>,
}

/// Face and avatar parameters, voice, VRChat's sound, body tracking.
pub fn streams(snap: &Snapshot) -> [Stream; 4] {
    let s = &snap.settings;
    let p = &s.performance;
    let stats = snap.capture.stats.as_ref().filter(|_| snap.is_capturing());
    // Frames carrying the capture kit's skeleton band: the pose VRChat gave the avatar (read whatever the switch).
    let kit = stats.is_some_and(|st| st.skeleton_frames > 0);
    let mic_name = snap.mics.iter().find(|m| if p.mic_device.is_empty() { m.default } else { m.id == p.mic_device }).map(|m| m.name.clone());
    // Named here, as the scan reads a text after a label as a bare one.
    let (face, voice, sound, body) = (tr("Face & avatar"), tr("Your voice"), tr("VRChat sound"), tr("Body tracking"));
    [
        Stream {
            glyph: icons::SMILEY,
            name: face,
            status: match stats {
                Some(st) if s.osc.record_parameters => tr!("{count} parameter changes", count = format::integer(st.parameter_changes as f64)),
                _ if !s.osc.enabled => tr("OSC is off").into(),
                _ if snap.osc.vrchat_found => tr!("{count} values from VRChat", count = snap.osc.values),
                _ if snap.osc.values > 0 => tr!("{count} values heard", count = snap.osc.values),
                _ => tr("Waiting for VRChat's OSC").into(),
            },
            tone: if !s.osc.enabled || !s.osc.record_parameters {
                Tone::Off
            } else if snap.osc.vrchat_found || snap.osc.values > 0 {
                Tone::Ok
            } else {
                Tone::Warn
            },
            level: None,
            switch: s.osc.enabled.then_some(s.osc.record_parameters),
        },
        Stream {
            glyph: icons::MICROPHONE,
            name: voice,
            status: if p.record_mic { mic_name.unwrap_or_else(|| tr("Default microphone").into()) } else { tr("Off").into() },
            tone: if p.record_mic { Tone::Ok } else { Tone::Off },
            level: stats.and_then(|st| st.mic_level),
            switch: Some(p.record_mic),
        },
        Stream {
            glyph: icons::SPEAKER_HIGH,
            name: sound,
            status: match (p.record_game_audio, snap.vrchat.running) {
                (false, _) => tr("Off").into(),
                (true, true) => tr("World and other players").into(),
                (true, false) => tr("When VRChat runs").into(),
            },
            tone: match (p.record_game_audio, snap.vrchat.running) {
                (false, _) => Tone::Off,
                (true, true) => Tone::Ok,
                (true, false) => Tone::Warn,
            },
            level: stats.and_then(|st| st.game_level),
            switch: Some(p.record_game_audio),
        },
        Stream {
            glyph: icons::PERSON_ARMS_SPREAD,
            name: body,
            status: match stats.and_then(|st| st.tracking_connected.map(|c| (c, st.tracking_devices))) {
                Some((true, n)) if kit => tr!("VRChat's pose, {n} devices", n = n),
                Some((true, n)) => tr!("{n} devices at {hz} Hz", n = n, hz = format!("{:.0}", p.tracking_rate_hz)),
                _ if kit => tr("VRChat's pose (capture kit)").into(),
                Some((false, _)) => tr("Waiting for SteamVR").into(),
                None if !p.record_tracking => tr("Off").into(),
                None if snap.steamvr.running => tr!("SteamVR devices at {hz} Hz", hz = format!("{:.0}", p.tracking_rate_hz)),
                None => tr("When SteamVR runs").into(),
            },
            tone: match (p.record_tracking, snap.steamvr.running) {
                _ if kit => Tone::Ok,
                (false, _) => Tone::Off,
                (true, true) => Tone::Ok,
                (true, false) => Tone::Warn,
            },
            level: None,
            switch: Some(p.record_tracking),
        },
    ]
}

/// The settings with stream `index`'s switch set.
pub fn with_switch(settings: &Settings, index: usize, on: bool) -> Settings {
    let mut s = settings.clone();
    match index {
        0 => s.osc.record_parameters = on,
        1 => s.performance.record_mic = on,
        2 => s.performance.record_game_audio = on,
        _ => s.performance.record_tracking = on,
    }
    s
}

/// A level meter that follows the peak with a spring.
pub fn meter(ui: &Ui, id: Id, rect: Rect, level: f32) {
    let k = anim::value(ui.ctx(), id, level.clamp(0.0, 1.0).sqrt(), Spring::SNAPPY).clamp(0.0, 1.0);
    let p = ui.painter();
    p.rect_filled(rect, theme::radius(rect.height() / 2.0), theme::white(0.08));
    if k > 0.01 {
        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * k, rect.height()));
        p.rect_filled(fill, theme::radius(rect.height() / 2.0), if k > 0.92 { theme::WARN } else { theme::OK });
    }
}
