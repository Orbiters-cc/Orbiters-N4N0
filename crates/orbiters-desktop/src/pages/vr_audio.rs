//! Settings, "When SteamVR starts": the sound output and input Windows uses by default while SteamVR runs, a test of
//! each, and what was done last (the service does it: `controller/vr_audio.rs`; `docs/vr-audio-devices.md`).

use super::mic::{mic_test, MicTest};
use super::settings::source_chip;
use super::{section, Cx, Job};
use crate::audio_devices::{self, AudioEndpoint, Flow};
use egui::{pos2, vec2, Align2, Sense, Ui};
use orbiters_core::settings::VrAudioSettings;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::widgets::{self, Button, Size};

type Listed = (Result<Vec<AudioEndpoint>, String>, Result<Vec<AudioEndpoint>, String>);

#[derive(Default)]
pub struct State {
    /// Windows' outputs and inputs, as last listed.
    outputs: Vec<AudioEndpoint>,
    inputs: Vec<AudioEndpoint>,
    error: Option<String>,
    /// Listed on each visit and when SteamVR starts or stops (its devices come and go with it), off this thread.
    listed_for: Option<(u64, bool)>,
    listing: Option<Job<Listed>>,
    /// The output test playing, and how the last one went.
    test: Option<Job<Result<(), String>>>,
    test_error: Option<String>,
    mic: MicTest,
}

impl State {
    /// Leaving the page: the microphone test ends.
    pub fn stop(&mut self) {
        self.mic = MicTest::default();
    }
}

/// The section; true when `draft` changed.
pub fn show(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State, draft: &mut VrAudioSettings) -> bool {
    let ctx = ui.ctx().clone();
    let steamvr = cx.snap.steamvr.running;
    if let Some((outputs, inputs)) = state.listing.as_mut().and_then(Job::take) {
        state.listing = None;
        state.error = outputs.as_ref().err().or(inputs.as_ref().err()).cloned();
        state.outputs = outputs.unwrap_or_default();
        state.inputs = inputs.unwrap_or_default();
    }
    if state.listing.is_none() && state.listed_for != Some((cx.visit, steamvr)) {
        state.listed_for = Some((cx.visit, steamvr));
        state.listing = Some(Job::spawn(&ctx, || (audio_devices::list(Flow::Output), audio_devices::list(Flow::Input))));
    }
    if let Some(result) = state.test.as_mut().and_then(Job::take) {
        state.test = None;
        state.test_error = result.err();
    }

    let subtitle = match &cx.snap.vr_audio.last {
        Some(e) => format!("{} {}", e.text, when(e.utc_ms)),
        None => "Windows' sound devices while SteamVR runs".to_string(),
    };
    let mut apply = false;
    section(ui, cx, index, icons::SPEAKER_HIGH, "When SteamVR starts", Some(&subtitle), |ui| {
        let testing = state.test.is_some();
        let output_id = draft.output.as_ref().map(|e| present(e, &state.outputs));
        widgets::field(
            ui,
            "Output in VR",
            Some("The speakers or headphones Windows makes every app's default when SteamVR starts, set again over its first seconds as Virtual Desktop and SteamVR put their own."),
            96.0,
            |ui| {
                let label = if testing { "Playing…" } else { "Test" };
                let test = Button::new(label).icon(icons::SPEAKER_HIGH).size(Size::Sm).enabled(output_id.is_some() && !testing);
                if test.show(ui).clicked() {
                    if let Some(id) = output_id.clone() {
                        state.test_error = None;
                        state.test = Some(Job::spawn(&ctx, move || audio_devices::play_test(&id)));
                    }
                }
            },
        );
        apply |= choices(ui, &mut draft.output, &state.outputs);
        if let Some(e) = &state.test_error {
            caption(ui, &format!("The test did not play: {e}"), theme::DANGER);
        }
        widgets::separator(ui);
        widgets::field(ui, "Input in VR", Some("The microphone Windows makes every app's default when SteamVR starts."), 0.0, |_| {});
        apply |= choices(ui, &mut draft.input, &state.inputs);
        if let Some(input) = &draft.input {
            ui.add_space(4.0);
            mic_test(ui, &mut state.mic, &present(input, &state.inputs));
            ui.add_space(12.0);
        } else if state.mic.active() {
            state.mic = MicTest::default();
        }
        if let Some(e) = &state.error {
            caption(ui, &format!("Windows did not list its sound devices: {e}"), theme::DANGER);
        }
        widgets::separator(ui);
        widgets::field(
            ui,
            "Put back the devices before",
            Some("When SteamVR closes, the output and input Windows had before it started come back."),
            48.0,
            |ui| {
                apply |= widgets::toggle(ui, "vr-audio-restore", &mut draft.restore);
            },
        );
    });
    apply
}

/// "Don't change" and each device, the chosen one shown even when it is not there now.
fn choices(ui: &mut Ui, choice: &mut Option<AudioEndpoint>, devices: &[AudioEndpoint]) -> bool {
    let mut changed = false;
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if source_chip(ui, "Don't change", choice.is_none()).clicked() && choice.is_some() {
            *choice = None;
            changed = true;
        }
        let found = choice.as_ref().and_then(|c| crate::controller::vr_audio::resolve(c, devices)).map(|e| e.id.clone());
        if let Some(c) = choice.as_ref().filter(|_| found.is_none()) {
            source_chip(ui, &format!("{} (not connected)", c.name), true);
        }
        for d in devices {
            let active = found.as_deref() == Some(d.id.as_str());
            if source_chip(ui, &d.name, active).clicked() && choice.as_ref() != Some(d) {
                *choice = Some(d.clone());
                changed = true;
            }
        }
    });
    ui.add_space(8.0);
    changed
}

/// The id the chosen device has now (found by its name when its id changed).
fn present(chosen: &AudioEndpoint, devices: &[AudioEndpoint]) -> String {
    crate::controller::vr_audio::resolve(chosen, devices).unwrap_or(chosen).id.clone()
}

fn caption(ui: &mut Ui, text: &str, color: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    widgets::text_truncated(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 11.5).color(color), r.width());
    ui.add_space(4.0);
}

/// "at 21:03" today, else "on 30 Oct 2026, 21:03".
fn when(utc_ms: u64) -> String {
    let local = |secs: i64| {
        let t = time::OffsetDateTime::from_unix_timestamp(secs).ok()?;
        Some(time::UtcOffset::current_local_offset().map(|o| t.to_offset(o)).unwrap_or(t))
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    match (local((utc_ms / 1000) as i64), local(now)) {
        (Some(t), Some(today)) if t.date() == today.date() => format!("at {:02}:{:02}", t.hour(), t.minute()),
        _ => format!("on {}", super::local_time_unix(utc_ms / 1000)),
    }
}
