//! Settings: every microphone and how much it hears, side by side, and pressing one makes it Windows' default. Enzo: "see at a glance where our mic sound goes and select the
//! right one rather than testing successively all of them until we find the right one". The microphones are open only while this card is on screen
//! ([`crate::audio_devices::Meters`]), and nothing they hear is kept: only how loud. The card is drawn again when a bar would move (a level changed by
//! a step of its bar, a microphone came or went) and while a bar falls, not thirty times a second whatever the room.

use orbiters_i18n::{tr, tr_ctx};
use super::{section, Cx, Job};
use crate::audio_devices::{self, MicLevels, ROLES};
use egui::{pos2, vec2, Align2, Rect, Sense, Stroke, StrokeKind, Ui};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// How often the levels are looked at (as often as they are read, `audio_devices`), and a falling bar drawn.
const LOOK_EVERY: Duration = Duration::from_millis(33);
/// The steps of a bar told apart: about its points across.
const BAR_STEPS: f32 = 256.0;

/// The levels as the card shows them: the same as another reading when no bar would be drawn elsewhere and the same
/// microphones are there, the same one Windows' default.
#[derive(Clone)]
struct Shown(MicLevels);

impl PartialEq for Shown {
    fn eq(&self, other: &Self) -> bool {
        let step = |peak: Option<f32>| peak.map(|p| (fill_of(p) * BAR_STEPS).round() as i32);
        let (a, b) = (&self.0, &other.0);
        a.read == b.read
            && a.default_id == b.default_id
            && a.mics.len() == b.mics.len()
            && a.mics.iter().zip(&b.mics).all(|(a, b)| a.endpoint == b.endpoint && step(a.peak) == step(b.peak))
    }
}

const TILE_HIGH: f32 = 66.0;
const TILE_WIDE: f32 = 230.0;
const GAP: f32 = 10.0;

#[derive(Default)]
pub struct State {
    meters: Option<Arc<audio_devices::Meters>>,
    /// The levels looked at on a thread while the card shows: it keeps the microphones open between frames (they
    /// close when nothing looks) and asks for a frame when a bar would move.
    looks: super::Watch<(), Shown>,
    /// What each bar shows now: it rises at once and falls slowly, to be read.
    shown: HashMap<String, f32>,
    /// The press that is being carried out, and what it asked for until the next reading says so.
    setting: Option<Job<Result<(), String>>>,
    asked: Option<String>,
    /// Why the last press did not take.
    error: Option<String>,
    /// Where the card's body was drawn last (is it in view?).
    body: Option<Rect>,
}

impl State {
    /// Leaving the page: the microphones close.
    pub fn stop(&mut self) {
        self.looks.stop();
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
        let start = ui.cursor().min;
        // The card is laid out wherever it is on the page (Settings runs every card's body): scrolled out of view,
        // nothing looks at the microphones, so its watch ends and they close by themselves (they stayed open, and
        // Settings redrew up to 30 times a second, for as long as the page showed). Back in view, they open again.
        let in_view = state.body.is_none_or(|r| ui.is_rect_visible(r));
        (|ui: &mut Ui| {
        let meters = state.meters.get_or_insert_with(|| Arc::new(audio_devices::Meters::start()));
        // While something looks: off the screen or minimized, this is not drawn, its watch ends, and they close by
        // themselves.
        if in_view {
            meters.looked_at();
            state.looks.keep(&ctx, &(), LOOK_EVERY, || {
                let meters = meters.clone();
                move || {
                    meters.looked_at();
                    Shown(meters.levels())
                }
            });
        } else {
            state.looks.stop();
        }
        let levels: MicLevels = meters.levels();
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
            // Falling to where the level is: drawn until it is there.
            if bar > fill {
                ctx.request_repaint_after(LOOK_EVERY);
            }
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
        })(ui);
        state.body = Some(Rect::from_min_max(start, pos2(ui.max_rect().right(), ui.min_rect().bottom().max(start.y + 1.0))));
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

    /// The card drew itself thirty times a second while Settings showed, whatever the microphones heard: it is drawn
    /// again when a bar would move, or the microphones or the default changed.
    #[test]
    fn a_reading_is_new_only_when_a_bar_would_move() {
        let mic = |id: &str, peak| audio_devices::MicLevel { endpoint: orbiters_core::settings::AudioEndpoint { id: id.into(), name: id.into() }, peak };
        let levels = |peak| Shown(MicLevels { mics: vec![mic("a", peak), mic("b", Some(0.0))], default_id: Some("a".into()), read: true });
        // A room's hiss wandering under -60 dB, and a level moving by less than a step of the bar: nothing to draw.
        assert!(levels(Some(0.0002)) == levels(Some(0.0009)));
        assert!(levels(Some(0.1)) == levels(Some(0.1001)));
        // A voice, a microphone that can no longer be read, another default, one unplugged: drawn.
        assert!(levels(Some(0.0002)) != levels(Some(0.05)));
        assert!(levels(Some(0.1)) != levels(None));
        let mut other = levels(Some(0.1));
        other.0.default_id = Some("b".into());
        assert!(levels(Some(0.1)) != other);
        let mut fewer = levels(Some(0.1));
        fewer.0.mics.pop();
        assert!(levels(Some(0.1)) != fewer);
    }

    #[test]
    fn tiles_fit_the_cards_width() {
        assert_eq!(across(100.0), 1);
        assert_eq!(across(TILE_WIDE), 1);
        assert_eq!(across(TILE_WIDE * 2.0 + GAP), 2);
        assert_eq!(across(800.0), 3);
    }
}
