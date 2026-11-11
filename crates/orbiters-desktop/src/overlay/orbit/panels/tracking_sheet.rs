//! The sheet of tracking smoothing: sliders in real milliseconds, in the headset. N4N0's menu page for it has the avatar
//! screen's fixed places, and no slider can be sent to an avatar's screen: there, Off, Low, Medium and High stay, and its
//! state line says the exact values. Here each slider is the time constant itself: the devices' in the SteamVR driver and
//! the tiles' around VRChat's menus, each independent of the frame rate.

use super::*;
use orbiters_core::settings::TrackingSettings;
use orbiters_n4n0_core::menu::strings::{self, Strings};

/// Which smoothing a slider sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Smoothed {
    /// A kind of device: headset, controllers, trackers.
    Device(usize),
    /// What is rigid on the hand around VRChat's Quick Menu.
    QuickMenu,
    /// The same around VRChat's Action Menu.
    Wheel,
}

impl Smoothed {
    /// The settings' patch for it at `ms`, with each device's milliseconds as they are (a slider on a device turns
    /// smoothing on, as a choice on the menu's page does).
    pub fn patch(self, ms: u16, devices: Option<[u16; 3]>) -> Option<serde_json::Value> {
        Some(match self {
            Smoothed::Device(kind) => {
                let mut all = devices?;
                *all.get_mut(kind)? = ms.min(TrackingSettings::MAX_MS);
                serde_json::json!({ "tracking": { "enabled": true, "ms": all } })
            }
            Smoothed::QuickMenu => serde_json::json!({ "orbit": { "vr_smoothing_quick_menu_ms": ms.min(MAX_MS) } }),
            Smoothed::Wheel => serde_json::json!({ "orbit": { "vr_smoothing_wheel_ms": ms.min(MAX_MS) } }),
        })
    }

    fn name(self) -> &'static str {
        match self {
            // In the words of N4N0's menu, which has these kinds in its own table.
            Smoothed::Device(kind) => Strings::builtin(orbiters_i18n::language()).get(strings::TRACKING_KINDS[kind.min(2)]),
            Smoothed::QuickMenu => tr("Tiles on the Quick Menu"),
            Smoothed::Wheel => tr("Tiles on the wheel"),
        }
    }

    fn about(self) -> &'static str {
        match self {
            Smoothed::Device(0) => tr("Keep it low: a smoothed headset lags behind your head"),
            Smoothed::Device(1) => tr("For your hands"),
            Smoothed::Device(_) => tr("Full-body trackers jitter most"),
            Smoothed::QuickMenu => tr("About 35 ms lines up with VRChat"),
            Smoothed::Wheel => tr("About 20 ms lines up with VRChat"),
        }
    }

    /// What it is now: held on the slider, or as saved.
    fn value(self, facts: &Facts) -> u16 {
        if let Some((_, ms)) = facts.smoothing_held.filter(|(which, _)| *which == self) {
            return ms;
        }
        match self {
            Smoothed::Device(kind) => facts.tracking.as_ref().and_then(|t| t.1.get(kind).copied()).unwrap_or(0),
            Smoothed::QuickMenu => facts.orbit.vr_smoothing_quick_menu_ms.round().clamp(0.0, MAX_MS as f32) as u16,
            Smoothed::Wheel => facts.orbit.vr_smoothing_wheel_ms.round().clamp(0.0, MAX_MS as f32) as u16,
        }
    }
}

/// The sliders' scale: the devices stop at their driver's longest, [`TrackingSettings::MAX_MS`], and the same scale for
/// the tiles lets their values be compared at a glance.
const MAX_MS: u16 = TrackingSettings::MAX_MS;
const X: f32 = 276.0;
/// Room right of a slider for what it says.
const VALUE_WIDTH: f32 = 96.0;
const BAR_RIGHT: f32 = 1000.0 - VALUE_WIDTH;

fn row(i: usize) -> f32 {
    12.0 + i as f32 * 78.0
}

const ROWS: [Smoothed; 5] = [Smoothed::Device(0), Smoothed::Device(1), Smoothed::Device(2), Smoothed::QuickMenu, Smoothed::Wheel];

/// The milliseconds at `x` on the page: whole ones, as a laser cannot do better and a value should be exact.
pub fn smoothing_at(x: f32) -> u16 {
    (((x - X - 12.0) / (BAR_RIGHT - X - 24.0)).clamp(0.0, 1.0) * MAX_MS as f32).round() as u16
}

pub(super) fn tracking_items(facts: &Facts) -> Vec<Item> {
    let mut out = Vec::new();
    let on = facts.tracking.as_ref().is_some_and(|t| t.0);
    // Without the driver here (the note says why) only the tiles' own smoothing is left to set.
    let choices = if facts.tracking.is_some() { vec![(true, icons::CHECK, tr("On")), (false, icons::X, tr("Off"))] } else { Vec::new() };
    for (n, (value, icon, label)) in choices.into_iter().enumerate() {
        out.push(Item {
            action: Action::Set(Pick::Smoothing(value)),
            rect: r(X + n as f32 * 318.0, row(0) + 4.0, 306.0, 58.0),
            look: Look::Choice { icon, label, on: on == value },
        });
    }
    for (i, which) in ROWS.into_iter().enumerate() {
        if facts.tracking.is_some() || !matches!(which, Smoothed::Device(_)) {
            out.push(Item { action: Action::Set(Pick::SmoothingBar(which)), rect: r(X, row(i + 1) + 4.0, 1000.0 - X, 58.0), look: Look::SmoothingBar(which) });
        }
    }
    out
}

/// The rows' names, and what the driver does now.
pub(super) fn draw_tracking(p: &Painter, facts: &Facts) {
    let mut named = |i: usize, name: &str, about: &str| {
        let y = row(i);
        // Whole before its slider, smaller where a language's name is long ("Mosaicos en el menú rápido" ran into it).
        let (name, font) = fitted(p, name, 20.0, weight::BOLD, X - 26.0 - 10.0, 15.0);
        p.text(pos2(26.0, y + 15.0), Align2::LEFT_CENTER, name, font, look::LABEL);
        let mut job = egui::text::LayoutJob::simple(about.to_string(), look::font(15.0, weight::MEDIUM), look::LABEL, X - 40.0);
        job.wrap.max_rows = 2;
        p.galley(pos2(26.0, y + 28.0), p.layout_job(job), look::LABEL);
    };
    if facts.tracking.is_some() {
        named(0, tr("Smooth tracking"), tr("SteamVR's devices, at once"));
    }
    for (i, which) in ROWS.into_iter().enumerate() {
        if facts.tracking.is_some() || !matches!(which, Smoothed::Device(_)) {
            named(i + 1, which.name(), which.about());
        }
    }
    let note = facts.tracking.as_ref().map_or(facts.tracking_note.as_str(), |t| t.2.as_str());
    if !note.is_empty() {
        let font = look::font(16.0, weight::MEDIUM);
        p.text(pos2(26.0, row(ROWS.len() + 1) + 12.0), Align2::LEFT_CENTER, fit(p, tr(note), &font, 1000.0 - 26.0), font, look::MUTED);
    }
}

/// A slider: its track, filled up to its value, and the value itself in milliseconds.
pub(super) fn draw_smoothing_bar(p: &Painter, rect: Rect, which: Smoothed, facts: &Facts, hovered: bool) {
    let ms = which.value(facts);
    let bar = Rect::from_min_max(pos2(rect.left(), rect.center().y - 15.0), pos2(BAR_RIGHT, rect.center().y + 15.0));
    let fill = if hovered { look::TILE_HOVER } else { look::TILE };
    p.rect_filled(bar, look::radius(15.0), fill);
    let x = bar.left() + 12.0 + (bar.width() - 24.0) * ms as f32 / MAX_MS as f32;
    if ms > 0 {
        p.rect_filled(Rect::from_min_max(bar.min, pos2(x + 12.0, bar.bottom())), look::radius(15.0), theme::mix(look::TILE, look::accent(), 0.55));
    }
    p.circle_filled(pos2(x, bar.center().y), 12.0, look::LABEL);
    let words = if ms == 0 { tr("Off").to_string() } else { orbiters_i18n::tr!("{ms} ms", ms = ms) };
    // In its column after the slider: "Desactivado" ran onto the slider's end.
    let (words, font) = fitted(p, &words, 20.0, weight::SEMIBOLD, VALUE_WIDTH - 12.0, 14.0);
    p.text(pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, words, font, look::LABEL);
}

/// What this sheet's things do, where they are pointed at.
pub(super) fn tracking_caption(action: Action) -> Option<(String, String)> {
    let Action::Set(pick) = action else { return None };
    let (a, b) = match pick {
        Pick::Smoothing(true) => (tr("Smooth tracking on"), tr("SteamVR's devices are eased as these sliders say.")),
        Pick::Smoothing(false) => (tr("Smooth tracking off"), tr("SteamVR tracks every device as it is.")),
        Pick::SmoothingBar(which) => (which.name(), tr("Press on the bar, or drag along it: a device in steady motion trails by this time.")),
        _ => return None,
    };
    Some((a.to_string(), b.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sliders_give_whole_milliseconds_from_end_to_end() {
        assert_eq!(smoothing_at(0.0), 0);
        assert_eq!(smoothing_at(X + 12.0), 0);
        assert_eq!(smoothing_at(BAR_RIGHT - 12.0), MAX_MS);
        assert_eq!(smoothing_at(1024.0), MAX_MS);
        // Each value can be reached: 35 ms, as well as 34 and 36.
        let per_ms = (BAR_RIGHT - X - 24.0) / MAX_MS as f32;
        assert!(per_ms > 4.0, "a laser can land on one: {per_ms} units a millisecond");
        assert_eq!(smoothing_at(X + 12.0 + 35.0 * per_ms), 35);
    }

    #[test]
    fn a_slider_patches_what_it_sets_and_leaves_the_rest() {
        assert_eq!(Smoothed::Device(1).patch(35, Some([0, 78, 42])), Some(serde_json::json!({ "tracking": { "enabled": true, "ms": [0, 35, 42] } })));
        assert_eq!(Smoothed::Device(1).patch(35, None), None, "the driver is not here");
        assert_eq!(Smoothed::Device(3).patch(35, Some([0, 0, 0])), None);
        assert_eq!(Smoothed::QuickMenu.patch(35, None), Some(serde_json::json!({ "orbit": { "vr_smoothing_quick_menu_ms": 35 } })));
        assert_eq!(Smoothed::Wheel.patch(20, None), Some(serde_json::json!({ "orbit": { "vr_smoothing_wheel_ms": 20 } })));
        // And the settings take them: whole milliseconds in a setting that holds any number, each saved as patches are.
        let settings = orbiters_core::settings::Settings::default();
        let (patched, left_out) = settings.patched(&Smoothed::QuickMenu.patch(33, None).unwrap());
        assert!(left_out.is_empty(), "{left_out:?}");
        assert_eq!(patched.orbit.vr_smoothing_quick_menu_ms, 33.0);
        let (patched, left_out) = settings.patched(&Smoothed::Device(1).patch(33, Some(settings.tracking.ms)).unwrap());
        assert!(left_out.is_empty(), "{left_out:?}");
        assert_eq!((patched.tracking.enabled, patched.tracking.ms[1]), (true, 33));
    }

    #[test]
    fn without_the_driver_only_the_tiles_are_left() {
        let items = tracking_items(&Facts::default());
        assert!(items.iter().all(|i| matches!(i.look, Look::SmoothingBar(Smoothed::QuickMenu | Smoothed::Wheel))), "{}", items.len());
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn it_shows_what_is_saved_and_what_is_held() {
        let facts = Facts { tracking: Some((true, [0, 78, 42], String::new())), ..Facts::default() };
        assert_eq!(Smoothed::Device(1).value(&facts), 78);
        let held = Facts { smoothing_held: Some((Smoothed::Device(1), 33)), ..facts };
        assert_eq!(Smoothed::Device(1).value(&held), 33);
        assert_eq!(Smoothed::Device(2).value(&held), 42, "only the one held");
        let items = tracking_items(&held);
        assert_eq!(items.iter().filter(|i| matches!(i.look, Look::SmoothingBar(_))).count(), 5);
        for item in &items {
            assert!(item.rect.right() <= 1024.0 && item.rect.bottom() <= 576.0, "{:?}", item.rect);
        }
    }
}
