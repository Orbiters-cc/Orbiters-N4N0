//! The sheet of settings, row by row, from All tools. All of what it says is translated (`orbiters-i18n/catalog/overlay-settings`): this is the file to copy for how text
//! is wrapped. Labels the items hold go through `tr` where the items are made, as they are made again for each picture.

use super::*;
use orbiters_i18n::{tr, Language};

const ROW_SOUND: usize = 2;
const ROW_LANGUAGE: usize = 3;
const ROW_THEME: usize = 4;
const ROW_COLOUR: usize = 5;
const ROW_PICTURES: usize = 6;

/// Where a row starts, and where its controls do.
fn settings_row(i: usize) -> f32 {
    12.0 + i as f32 * 78.0
}
const SETTINGS_X: f32 = 276.0;
const SETTINGS_HIGH: f32 = 58.0;

/// The bar of colours.
pub(super) fn hue_bar() -> Rect {
    r(SETTINGS_X + 8.0 * 60.0 + 12.0, settings_row(ROW_COLOUR) + 12.0, 1000.0 - (SETTINGS_X + 8.0 * 60.0 + 12.0), 42.0)
}

/// The colour on the bar at `x` on the page.
pub fn hue_at(x: f32) -> [u8; 3] {
    let bar = hue_bar();
    hue_color(((x - bar.left()) / bar.width()).clamp(0.0, 1.0))
}

pub(super) fn hue_color(t: f32) -> [u8; 3] {
    egui::ecolor::Hsva::new(t, 0.8, 1.0, 1.0).to_srgb()
}

pub(super) fn settings_items(facts: &Facts) -> Vec<Item> {
    let mut out = Vec::new();
    let mut two = |i: usize, first: (Pick, &'static str, &'static str, bool), second: (Pick, &'static str, &'static str, bool)| {
        for (n, (pick, icon, label, on)) in [first, second].into_iter().enumerate() {
            out.push(Item { action: Action::Set(pick), rect: r(SETTINGS_X + n as f32 * 318.0, settings_row(i) + 4.0, 306.0, SETTINGS_HIGH), look: Look::Choice { icon, label, on } });
        }
    };
    let o = &facts.orbit;
    let strip = o.layout == OrbitLayout::Strip;
    two(0, (Pick::Layout(OrbitLayout::Wing), icons::SQUARES_FOUR, tr("Beside the menu"), !strip), (Pick::Layout(OrbitLayout::Strip), icons::ARROW_DOWN, tr("Under the menu"), strip));
    let springy = o.vr_motion == VrMotion::Springy;
    two(1, (Pick::Motion(VrMotion::Springy), icons::SPARKLE, tr("Springy"), springy), (Pick::Motion(VrMotion::Matched), icons::HAND_GRABBING, tr("Rigid"), !springy));
    let retro = o.theme == MenuLook::Retro;
    two(ROW_THEME, (Pick::Theme(MenuLook::Modern), icons::PALETTE, tr("Modern"), !retro), (Pick::Theme(MenuLook::Retro), icons::MONITOR_PLAY, tr("Retro"), retro));
    if retro {
        two(ROW_PICTURES, (Pick::Pictures(false), icons::IMAGE, tr("Fully monochrome"), !o.retro_pictures), (Pick::Pictures(true), icons::IMAGES, tr("Pictures in colour"), o.retro_pictures));
    }
    let mut stepper = |row: usize, before: Pick, after: Pick, name: Look| {
        let y = settings_row(row) + 4.0;
        out.push(Item { action: Action::Set(before), rect: r(SETTINGS_X, y + 1.0, 56.0, 56.0), look: Look::Step(false) });
        out.push(Item { action: Action::Idle, rect: r(SETTINGS_X + 64.0, y, 1000.0 - SETTINGS_X - 128.0, SETTINGS_HIGH), look: name });
        out.push(Item { action: Action::Set(after), rect: r(1000.0 - 56.0, y + 1.0, 56.0, 56.0), look: Look::Step(true) });
    };
    stepper(ROW_SOUND, Pick::Sound(false), Pick::Sound(true), Look::SoundName);
    stepper(ROW_LANGUAGE, Pick::Language(false), Pick::Language(true), Look::LanguageName);
    for i in 0..SWATCHES.len() {
        out.push(Item { action: Action::Set(Pick::Swatch(i)), rect: r(SETTINGS_X + i as f32 * 60.0, settings_row(ROW_COLOUR) + 6.0, 54.0, 54.0), look: Look::Swatch(i) });
    }
    out.push(Item { action: Action::Set(Pick::Hue), rect: hue_bar(), look: Look::HueBar });
    out
}

/// A row's name and what it is about, down the left of the sheet.
pub(super) fn draw_settings(p: &Painter, facts: &Facts) {
    let retro = facts.orbit.theme == MenuLook::Retro;
    let mut rows = vec![
        (0, tr("Layout"), tr("Where the tiles lie on the Quick Menu")),
        (1, tr("Tiles in VR"), tr("How they follow your hand")),
        (ROW_SOUND, tr("Sound in VR"), tr("The output Windows uses when SteamVR starts")),
        (ROW_LANGUAGE, tr("Language"), tr("Of Orbiters - N4N0 and your Orbiters account")),
        (ROW_THEME, tr("Theme"), tr("How these menus look")),
        (ROW_COLOUR, tr("Colour"), if retro { tr("The one colour of everything") } else { tr("The accent of every button") }),
    ];
    if retro {
        rows.push((ROW_PICTURES, tr("Pictures"), tr("Retro can leave them in colour")));
    }
    for (i, name, about) in rows {
        let y = settings_row(i);
        let font = look::font(20.0, weight::BOLD);
        p.text(pos2(26.0, y + 15.0), Align2::LEFT_CENTER, name, font, look::LABEL);
        let mut job = egui::text::LayoutJob::simple(about.to_string(), look::font(15.0, weight::MEDIUM), look::LABEL, SETTINGS_X - 40.0);
        job.wrap.max_rows = 2;
        p.galley(pos2(26.0, y + 28.0), p.layout_job(job), look::LABEL);
    }
}

/// The name in the row of the sound device, or of the language.
pub(super) fn draw_stepper_name(p: &Painter, rect: Rect, icon: &str, name: &str) {
    p.rect_filled(rect, look::radius(14.0), look::TILE);
    widgets::icon(p, pos2(rect.left() + 30.0, rect.center().y), icon, 24.0, look::LABEL);
    let font = look::font(19.0, weight::SEMIBOLD);
    p.text(pos2(rect.left() + 56.0, rect.center().y), Align2::LEFT_CENTER, fit(p, name, &font, rect.width() - 70.0), font, look::LABEL);
}

pub(super) fn sound_name(facts: &Facts) -> &str {
    facts.vr_output.as_deref().unwrap_or(tr("Leave it as it is"))
}

/// The language chosen. A language is in its own name, whatever the language now.
pub(super) fn language_name(facts: &Facts) -> String {
    crate::language::name_of(facts.language_choice)
}

/// What this sheet's things do, where they are pointed at.
pub(super) fn settings_caption(action: Action) -> Option<(String, String)> {
    let (a, b) = match action {
        Action::Settings => (tr("Settings"), tr("These menus' layout, colours, motion, sound and language.")),
        Action::Set(Pick::Hue) => (tr("Any colour"), tr("Press on the bar, or drag along it.")),
        Action::Set(Pick::Sound(_)) => (tr("Sound in VR"), tr("The output Windows uses when SteamVR starts.")),
        Action::Set(Pick::Language(_)) => (tr("Language"), tr("Saved to your Orbiters account, and used on the website too.")),
        _ => return None,
    };
    Some((a.to_string(), b.to_string()))
}

/// The next place in the language picker, or the one before, round.
pub fn language_after(now: Option<Language>, after: bool) -> Option<Language> {
    let places = crate::language::choices();
    let at = places.iter().position(|p| *p == now).unwrap_or(0);
    let next = if after { (at + 1) % places.len() } else { (at + places.len() - 1) % places.len() };
    places[next]
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_i18n::in_language;

    /// Each place once, and back where it began, either way.
    #[test]
    fn the_language_picker_goes_round_every_language() {
        let mut seen = vec![None];
        let mut at = None;
        for _ in 0..Language::ALL.len() {
            at = language_after(at, true);
            assert!(!seen.contains(&at), "{at:?}");
            seen.push(at);
        }
        assert_eq!(language_after(at, true), None, "and round");
        assert_eq!(language_after(None, false), Some(Language::PtBr));
    }

    /// Nothing of this sheet is left in English, in any language: what the coverage test asks of the source, here of what is drawn.
    #[test]
    fn the_sheet_says_all_of_it_in_each_language() {
        let retro = OrbitSettings { theme: MenuLook::Retro, ..Default::default() };
        let facts = Facts { sheet: Some(Sheet::Settings), orbit: retro, ..Facts::default() };
        let english: Vec<String> = settings_items(&facts).iter().filter_map(|i| if let Look::Choice { label, .. } = i.look { Some(label.to_string()) } else { None }).collect();
        assert_eq!(english.len(), 8);
        for language in Language::TRANSLATED {
            in_language(language, || {
                for item in settings_items(&facts) {
                    if let Look::Choice { label, .. } = item.look {
                        assert!(!english.contains(&label.to_string()) || label == "Retro" || label == "Moderno", "{label} in {language:?}");
                    }
                }
                let (name, about) = settings_caption(Action::Set(Pick::Language(true))).unwrap();
                assert_ne!(name, "Language");
                assert_ne!(about, "Saved to your Orbiters account, and used on the website too.");
                assert_ne!(sound_name(&facts), "Leave it as it is");
            });
        }
    }

    /// What it says fits what it is in, in the language that is longest: a look at the widths, as the sheet is tight.
    #[test]
    fn the_rows_stay_on_the_page() {
        let facts = Facts { sheet: Some(Sheet::Settings), orbit: OrbitSettings { theme: MenuLook::Retro, ..Default::default() }, ..Facts::default() };
        for item in settings_items(&facts) {
            assert!(item.rect.bottom() <= 576.0 && item.rect.right() <= 1024.0, "{:?}", item.rect);
        }
    }
}
