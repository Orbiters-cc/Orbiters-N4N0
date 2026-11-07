//! The language, on the Settings page. The same choice as the overlay's sheet of settings: saved here at once, and with the account in the background.

use super::{section, Cx};
use egui::{vec2, Sense, Ui};
use orbiters_core::settings::Settings;
use orbiters_i18n::tr;
use orbiters_ui::popup::{self, Item};
use orbiters_ui::{icons, widgets};

/// The card. Whether the settings changed.
pub fn card(ui: &mut Ui, cx: &Cx, index: usize, draft: &mut Settings) -> bool {
    let mut changed = false;
    section(ui, cx, index, icons::GLOBE, tr("Language"), Some(tr("Of Orbiters - N4N0 and your Orbiters account")), |ui| {
        widgets::field(ui, tr("Language"), Some(tr("Saved to your Orbiters account, and used on the website too.")), 260.0, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(260.0, 30.0), Sense::hover());
            let choices = crate::language::choices();
            let names: Vec<String> = choices.iter().map(|c| crate::language::name_of(*c)).collect();
            let items: Vec<Item> = names.iter().map(|n| Item::new(n)).collect();
            let chosen = crate::language::chosen(draft);
            let at = choices.iter().position(|c| *c == chosen);
            if let Some(picked) = popup::dropdown(ui, "language", rect, &crate::language::name_of(chosen), &items, at, true) {
                if choices[picked] != chosen {
                    draft.language = choices[picked].map(|l| l.tag().to_string());
                    draft.language_unsent = true;
                    changed = true;
                }
            }
        });
    });
    changed
}
