//! The page of microphones, from the sheet of settings: every microphone and how much it hears, which is the one Windows gives out, your voice in recordings, and what N4N0 listens to. The same
//! settings and the same Windows default as the desktop's Settings and N4N0 pages (`pages::mic_grid`), so the two can never disagree: only their drawing is its own.

use super::*;
use orbiters_i18n::{tr, tr_ctx};

/// A microphone as it shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MicTile {
    pub id: String,
    pub name: String,
    /// What the bar shows, 0 to 1.
    pub level: f32,
    /// Its level can be read: another app may hold it for itself.
    pub can_hear: bool,
    pub default: bool,
}

const X: f32 = 24.0;
const NAME_W: f32 = 268.0;
const ROW_H: f32 = 58.0;
const ROWS_TOP: f32 = 12.0;
const ROW_STEP: f32 = 66.0;
const GRID_TOP: f32 = ROWS_TOP + ROW_STEP * 3.0 + 26.0;
const TILE_H: f32 = 84.0;
const COLUMNS: usize = 3;
const GAP: f32 = 10.0;

fn tile_rect(i: usize) -> Rect {
    let w = (1024.0 - X * 2.0 - GAP * (COLUMNS - 1) as f32) / COLUMNS as f32;
    r(X + (i % COLUMNS) as f32 * (w + GAP), GRID_TOP + (i / COLUMNS) as f32 * (TILE_H + GAP), w, TILE_H)
}

/// How many tiles fit the page.
fn fits() -> usize {
    ((576.0 - GRID_TOP) / (TILE_H + GAP)).floor() as usize * COLUMNS
}

pub(super) fn mic_items(facts: &Facts) -> Vec<Item> {
    let mut out = Vec::new();
    let controls = X + NAME_W;
    let choice = |pick, icon, label, on, n: f32| Item { action: Action::Set(pick), rect: r(controls + n * 348.0, ROWS_TOP, 340.0, ROW_H), look: Look::Choice { icon, label, on } };
    out.push(choice(Pick::VoiceOn(true), icons::MICROPHONE, tr_ctx("your voice", "Recorded"), facts.voice_on, 0.0));
    out.push(choice(Pick::VoiceOn(false), icons::X, tr_ctx("your voice", "Left out"), !facts.voice_on, 1.0));
    let mut stepper = |row: f32, before: Pick, after: Pick, name: Look| {
        let y = ROWS_TOP + row * ROW_STEP;
        out.push(Item { action: Action::Set(before), rect: r(controls, y + 1.0, 56.0, 56.0), look: Look::Step(false) });
        out.push(Item { action: Action::Idle, rect: r(controls + 64.0, y, 1000.0 - controls - 128.0, ROW_H), look: name });
        out.push(Item { action: Action::Set(after), rect: r(1000.0 - 56.0, y + 1.0, 56.0, 56.0), look: Look::Step(true) });
    };
    stepper(1.0, Pick::VoiceMic(false), Pick::VoiceMic(true), Look::MicName(true));
    stepper(2.0, Pick::N4n0Mic(false), Pick::N4n0Mic(true), Look::MicName(false));
    for i in 0..facts.mics.len().min(fits()) {
        out.push(Item { action: Action::MicDefault(i), rect: tile_rect(i), look: Look::MicTile(i) });
    }
    out
}

/// What each row is, and what stands over the tiles.
pub(super) fn draw_mics(p: &Painter, facts: &Facts) {
    let name = look::font(19.0, weight::BOLD);
    for (row, text) in [tr("Your voice in recordings"), tr("Which microphone"), tr("N4N0 listens to")].into_iter().enumerate() {
        let room = NAME_W - 14.0;
        p.text(pos2(X, ROWS_TOP + row as f32 * ROW_STEP + ROW_H / 2.0), Align2::LEFT_CENTER, fit(p, text, &name, room), name.clone(), look::label());
    }
    let font = look::font(14.5, weight::MEDIUM);
    p.text(pos2(X, GRID_TOP - 13.0), Align2::LEFT_CENTER, fit(p, tr("Speak: the one that moves is the one hearing you. Press one to make it Windows' default."), &font, 1024.0 - X * 2.0), font, look::muted());
    if facts.mics.is_empty() {
        let font = look::font(16.0, weight::MEDIUM);
        p.text(pos2(X, GRID_TOP + 24.0), Align2::LEFT_CENTER, tr("Looking for microphones…"), font, look::muted());
    }
}

pub(super) fn draw_tile(p: &Painter, rect: Rect, mic: Option<&MicTile>, hovered: bool, pressed: bool) {
    let Some(mic) = mic else { return };
    let radius = look::radius(14.0);
    p.rect_filled(rect, radius, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
    if mic.default {
        p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(2.0, look::accent()), egui::StrokeKind::Inside);
    }
    let font = look::font(17.0, weight::SEMIBOLD);
    let word = tr_ctx("microphone", "Default");
    let small = look::font(13.0, weight::SEMIBOLD);
    let word_wide = if mic.default { p.layout_no_wrap(word.to_string(), small.clone(), look::accent()).size().x + 12.0 } else { 0.0 };
    p.text(pos2(rect.left() + 16.0, rect.top() + 26.0), Align2::LEFT_CENTER, fit(p, &mic.name, &font, rect.width() - 32.0 - word_wide), font, look::label());
    if mic.default {
        p.text(pos2(rect.right() - 16.0, rect.top() + 26.0), Align2::RIGHT_CENTER, word, small, look::accent());
    }
    let track = Rect::from_min_max(pos2(rect.left() + 16.0, rect.bottom() - 26.0), pos2(rect.right() - 16.0, rect.bottom() - 15.0));
    if mic.can_hear {
        p.rect_filled(track, look::radius(5.5), Color32::from_white_alpha(28));
        if mic.level > 0.0 {
            let lit = Rect::from_min_size(track.min, vec2((track.width() * mic.level).max(track.height()), track.height()));
            p.rect_filled(lit, look::radius(5.5), if mic.level > 0.45 { look::accent() } else { theme::alpha(look::accent(), 0.55) });
        }
    } else {
        let font = look::font(13.0, weight::MEDIUM);
        p.text(track.left_center(), Align2::LEFT_CENTER, tr("Its level cannot be read now"), font, look::muted());
    }
}

/// The name of what is chosen, by the desktop's list: for your voice, or for N4N0.
pub(super) fn chosen_name(facts: &Facts, voice: bool) -> &str {
    let id = if voice { &facts.voice_mic } else { &facts.n4n0_mic };
    if id.is_empty() {
        return crate::pages::settings::default_mic();
    }
    facts.mic_choices.iter().find(|(i, _)| i == id).map_or(id.as_str(), |(_, name)| name.as_str())
}

/// The id of the next in the list after `now`, or the one before: Windows' default, an empty id, first, as the desktop has it.
pub fn mic_after(choices: &[(String, String)], now: &str, after: bool) -> String {
    let count = choices.len() + 1;
    let place = choices.iter().position(|(id, _)| id == now).map_or(0, |i| i + 1);
    let next = if after { (place + 1) % count } else { (place + count - 1) % count };
    next.checked_sub(1).and_then(|i| choices.get(i)).map_or(String::new(), |(id, _)| id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same two settings as the desktop's, round through Windows' default.
    #[test]
    fn the_next_microphone_goes_round_through_the_default() {
        let list = vec![("a".to_string(), "A".to_string()), ("b".to_string(), "B".to_string())];
        assert_eq!(mic_after(&list, "", true), "a");
        assert_eq!(mic_after(&list, "a", true), "b");
        assert_eq!(mic_after(&list, "b", true), "");
        assert_eq!(mic_after(&list, "", false), "b");
        assert_eq!(mic_after(&[], "", true), "");
    }

    #[test]
    fn its_tiles_stay_on_the_page() {
        let facts = Facts { mics: vec![MicTile::default(); 40], ..Facts::default() };
        for item in mic_items(&facts) {
            assert!(item.rect.left() >= 0.0 && item.rect.right() <= 1024.0 && item.rect.bottom() <= 576.0, "{:?}", item.rect);
        }
        assert!(fits() >= 6);
    }
}
