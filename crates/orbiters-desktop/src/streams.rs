//! The performance streams as both interfaces show them (the window's Capture page and the SteamVR tab): what
//! each one records right now, its level, and its switch (which applies to the next recording).

use crate::state::Snapshot;
use egui::{vec2, Id, Rect, Ui};
use orbiters_core::settings::Settings;
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
    let mic_name = snap.mics.iter().find(|m| if p.mic_device.is_empty() { m.default } else { m.id == p.mic_device }).map(|m| m.name.clone());
    [
        Stream {
            glyph: icons::SMILEY,
            name: "Face & avatar",
            status: match stats {
                Some(st) if s.osc.record_parameters => format!("{} parameter changes", format::integer(st.parameter_changes as f64)),
                _ if !s.osc.enabled => "OSC is off".into(),
                _ if snap.osc.vrchat_found => format!("{} values from VRChat", snap.osc.values),
                _ if snap.osc.values > 0 => format!("{} values heard", snap.osc.values),
                _ => "Waiting for VRChat's OSC".into(),
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
            name: "Your voice",
            status: if p.record_mic { mic_name.unwrap_or_else(|| "Default microphone".into()) } else { "Off".into() },
            tone: if p.record_mic { Tone::Ok } else { Tone::Off },
            level: stats.and_then(|st| st.mic_level),
            switch: Some(p.record_mic),
        },
        Stream {
            glyph: icons::SPEAKER_HIGH,
            name: "VRChat sound",
            status: match (p.record_game_audio, snap.vrchat.running) {
                (false, _) => "Off".into(),
                (true, true) => "World and other players".into(),
                (true, false) => "When VRChat runs".into(),
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
            name: "Body tracking",
            status: match stats.and_then(|st| st.tracking_connected.map(|c| (c, st.tracking_devices))) {
                Some((true, n)) => format!("{n} devices at {:.0} Hz", p.tracking_rate_hz),
                Some((false, _)) => "Waiting for SteamVR".into(),
                None if !p.record_tracking => "Off".into(),
                None if snap.steamvr.running => format!("SteamVR devices at {:.0} Hz", p.tracking_rate_hz),
                None => "When SteamVR runs".into(),
            },
            tone: match (p.record_tracking, snap.steamvr.running) {
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
