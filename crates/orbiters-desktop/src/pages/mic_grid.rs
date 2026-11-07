//! Settings: every microphone and how much it hears, side by side, and pressing one makes it Windows' default. Enzo: "see at a glance where our mic sound goes and select the
//! right one rather than testing successively all of them until we find the right one". The microphones are open only while this card is on screen
//! ([`crate::audio_devices::Meters`]), and nothing they hear is kept: only how loud.

use orbiters_i18n::{tr, tr_ctx};
use super::{section, Cx, Job};
use crate::audio_devices::{self, MicLevels, ROLES};
use egui::{pos2, vec2, Align2, Rect, Sense, Stroke, StrokeKind, Ui};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};
use std::collections::HashMap;

const TILE_HIGH: f32 = 66.0;
const TILE_WIDE: f32 = 230.0;
const GAP: f32 = 10.0;

#[derive(Default)]
pub struct State {
    meters: Option<audio_devices::Meters>,
    /// What each bar shows now: it rises at once and falls slowly, to be read.
    shown: HashMap<String, f32>,
    /// The press that is being carried out, and what it asked for until the next reading says so.
    setting: Option<Job<Result<(), String>>>,
    asked: Option<String>,
    /// Why the last press did not take.
    error: Option<String>,
}

impl State {
    /// Leaving the page: the microphones close.
    pub fn stop(&mut self) {
        self.meters = None;
    }
}

/// How full a bar is for a peak (0 to 1): by loudness, not amplitude, as a voice peaks far below full scale. -60 dB and under is nothing.
pub fn fill_of(peak: f32) -> f32 {
    if peak <= 0.001 {
        return 0.0;
    }
    (1.0 + 20.0 * peak.log10() / 60.0).clamp(0.0, 1.0)
}

/// What a bar shows next, from what it showed and the peak now, `seconds` later.
pub fn settled(shown: f32, fill: f32, seconds: f32) -> f32 {
    if fill >= shown {
        fill
    } else {
        (shown - seconds * 1.4).max(fill)
    }
}

/// How many tiles to a row in `width`.
fn across(width: f32) -> usize {
    (((width + GAP) / (TILE_WIDE + GAP)).floor() as usize).max(1)
}

/// The card.
pub fn card(ui: &mut Ui, cx: &Cx, index: usize, state: &mut State) {
    let ctx = ui.ctx().clone();
    if let Some(result) = state.setting.as_mut().and_then(Job::take) {
        state.setting = None;
        if let Err(e) = result {
            state.asked = None;
            state.error = Some(tr!("Windows did not take that microphone: {error}", error = e));
        }
    }
    section(ui, cx, index, icons::MICROPHONE, tr("Microphones"), Some(tr("Speak: the one that moves is the one hearing you. Press one to make it Windows' default.")), |ui| {
        let meters = state.meters.get_or_insert_with(audio_devices::Meters::start);
        // While something looks: off the screen or minimized, this is not drawn, and they close by themselves.
        meters.looked_at();
        let levels: MicLevels = meters.levels();
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
        if levels.mics.is_empty() {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TILE_HIGH), Sense::hover());
            let said = if levels.read { tr("No microphone is plugged in or turned on.") } else { tr("Looking for microphones…") };
            ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, said, theme::font(13.5, weight::MEDIUM), theme::FG_3);
            return;
        }
        // What Windows says, once it says what was asked.
        if state.asked.as_deref() == levels.default_id.as_deref() {
            state.asked = None;
        }
        let chosen = state.asked.clone().or_else(|| levels.default_id.clone());
        let width = ui.available_width();
        let columns = across(width);
        let tile_w = (width - GAP * (columns - 1) as f32) / columns as f32;
        let rows = levels.mics.len().div_ceil(columns);
        if let Some(error) = &state.error {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
            ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, error, theme::font(13.0, weight::MEDIUM), theme::DANGER);
        }
        let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * TILE_HIGH + (rows - 1) as f32 * GAP), Sense::hover());
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        for (i, mic) in levels.mics.iter().enumerate() {
            let (row, column) = (i / columns, i % columns);
            let rect = Rect::from_min_size(area.min + vec2(column as f32 * (tile_w + GAP), row as f32 * (TILE_HIGH + GAP)), vec2(tile_w, TILE_HIGH));
            let response = ui.interact(rect, ui.id().with(("mic", &mic.endpoint.id)), Sense::click());
            let is_default = chosen.as_deref() == Some(mic.endpoint.id.as_str());
            let fill = mic.peak.map_or(0.0, fill_of);
            let was = state.shown.get(&mic.endpoint.id).copied().unwrap_or(0.0);
            let bar = settled(was, fill, dt);
            state.shown.insert(mic.endpoint.id.clone(), bar);
            let hearing = bar > 0.45;
            let p = ui.painter();
            let back = if response.is_pointer_button_down_on() {
                theme::INK_700
            } else if response.hovered() {
                theme::INK_750
            } else {
                theme::INK_800
            };
            let radius = theme::radius(12.0);
            p.rect_filled(rect, radius, back);
            if is_default {
                p.rect_stroke(rect.shrink(0.75), radius, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
            }
            // The name gives way to "Default", as long as that is in the language.
            let default_wide = if is_default { ui.painter().layout_no_wrap(tr_ctx("microphone", "Default").to_string(), theme::font(12.0, weight::SEMIBOLD), theme::ACCENT).size().x + 12.0 } else { 0.0 };
            let room = rect.width() - 28.0 - default_wide.max(if is_default { 62.0 } else { 0.0 });
            let name = p.layout_job(Txt::new(mic.endpoint.name.clone(), 13.5).weight(weight::MEDIUM).color(theme::FG).job_truncated(room));
            p.galley(pos2(rect.left() + 14.0, rect.top() + 12.0), name, theme::FG);
            if is_default {
                p.text(pos2(rect.right() - 14.0, rect.top() + 21.0), Align2::RIGHT_CENTER, tr_ctx("microphone", "Default"), theme::font(12.0, weight::SEMIBOLD), theme::ACCENT);
            }
            let track = Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 21.0), pos2(rect.right() - 14.0, rect.bottom() - 13.0));
            match mic.peak {
                Some(_) => {
                    p.rect_filled(track, theme::radius(4.0), theme::white(0.08));
                    if bar > 0.0 {
                        let lit = Rect::from_min_size(track.min, vec2((track.width() * bar).max(track.height()), track.height()));
                        p.rect_filled(lit, theme::radius(4.0), if hearing { theme::ACCENT } else { theme::alpha(theme::ACCENT, 0.55) });
                    }
                }
                // Held by another app for itself, say: it can still be chosen.
                None => {
                    p.text(track.left_center(), Align2::LEFT_CENTER, tr("Its level cannot be read now"), theme::font(12.0, weight::MEDIUM), theme::FG_3);
                }
            }
            if widgets::activated(&response) && !is_default && state.setting.is_none() {
                let id = mic.endpoint.id.clone();
                state.asked = Some(id.clone());
                state.error = None;
                state.setting = Some(Job::spawn(&ctx, move || ROLES.iter().try_for_each(|role| audio_devices::set_default(&id, *role))));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_reads_loudness_and_falls_slowly() {
        assert_eq!(fill_of(0.0), 0.0);
        assert_eq!(fill_of(1.0), 1.0);
        assert_eq!(fill_of(2.0), 1.0, "past full scale, as a meter may say");
        // A voice at -30 dB is half the bar, a room's hiss at -60 hardly anything.
        assert!((fill_of(0.0316) - 0.5).abs() < 0.01);
        assert!(fill_of(0.0011) < 0.05);
        assert_eq!(settled(0.2, 0.8, 0.016), 0.8, "up at once");
        let down = settled(0.8, 0.0, 0.1);
        assert!(down < 0.8 && down > 0.5, "down in a while: {down}");
        assert_eq!(settled(0.05, 0.0, 1.0), 0.0);
    }

    #[test]
    fn tiles_fit_the_cards_width() {
        assert_eq!(across(100.0), 1);
        assert_eq!(across(TILE_WIDE), 1);
        assert_eq!(across(TILE_WIDE * 2.0 + GAP), 2);
        assert_eq!(across(800.0), 3);
    }
}
