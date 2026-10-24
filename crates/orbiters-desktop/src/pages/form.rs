//! What the Settings and N4N0 pages share to edit the settings. Switches, sliders and choices apply as they change;
//! text fields wait for their Save button (or Enter) and keep what is typed while the settings change elsewhere (the
//! avatar's Walk boost radial, the SteamVR tab, N4N0). A change is saved over the settings as they are when it is
//! saved, so it carries only itself: never a text typed and not saved, nor an old copy of what changed meanwhile.

use crate::backend::Ctl;
use orbiters_core::settings::Settings;
use orbiters_ui::widgets::{self, Button, Size};
use std::collections::HashMap;

/// Text typed in a page's fields and not saved yet, by field.
#[derive(Default)]
pub struct Texts {
    typed: HashMap<&'static str, String>,
}

impl Texts {
    /// What a field shows: what was typed there, or else the saved value.
    pub fn shown(&self, key: &'static str, saved: &str) -> String {
        self.typed.get(key).cloned().unwrap_or_else(|| saved.to_string())
    }

    /// Keeps what the field holds after this frame's typing; typed back to the saved value, nothing waits.
    pub fn keep(&mut self, key: &'static str, text: String, saved: &str) {
        if text == saved {
            self.typed.remove(key);
        } else {
            self.typed.insert(key, text);
        }
    }

    /// Whether the field holds something its Save button would save.
    pub fn changed(&self, key: &'static str) -> bool {
        self.typed.contains_key(key)
    }

    /// What was typed, now being saved.
    pub fn take(&mut self, key: &'static str) -> Option<String> {
        self.typed.remove(key)
    }

    /// Forgets what was typed: the value was picked another way (a source chip).
    pub fn forget(&mut self, key: &'static str) {
        self.typed.remove(key);
    }
}

/// A text field and its Save button, in a [`widgets::field`]'s control area (laid out right to left). Save takes
/// clicks only while what is typed differs from what is saved. Returns the text to save when Save is clicked or
/// Enter pressed in the field.
pub fn text_row(ui: &mut egui::Ui, texts: &mut Texts, key: &'static str, saved: &str, hint: &str, width: f32, mono: bool) -> Option<String> {
    let mut text = texts.shown(key, saved);
    let save = Button::new("Save").size(Size::Sm).enabled(texts.changed(key)).show(ui).clicked();
    ui.add_space(8.0);
    let field = widgets::text_field(ui, key, &mut text, hint, width, mono);
    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    texts.keep(key, text, saved);
    if (save || enter) && texts.changed(key) {
        return texts.take(key);
    }
    None
}

/// Saves what a page changed in `draft` against `base` (the settings it drew this frame from): only those fields go,
/// merged by the service into the settings as they are then ([`orbiters_core::settings::Settings::patched`]). Where
/// something else changed the same field meanwhile, the page wins: it is what the person just did.
pub fn save(ctl: &Ctl, base: &Settings, draft: &Settings) {
    ctl.save_changes(base, draft);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_waits_for_save_and_outlives_changes_elsewhere() {
        let mut texts = Texts::default();
        assert_eq!(texts.shown("spout", "VRChat"), "VRChat");
        assert!(!texts.changed("spout"));
        texts.keep("spout", "OBS".into(), "VRChat");
        assert!(texts.changed("spout"));
        // The saved value changes elsewhere: what was typed stays.
        assert_eq!(texts.shown("spout", "Something else"), "OBS");
        // Typed back to what is saved: nothing waits, Save has nothing to do.
        texts.keep("spout", "VRChat".into(), "VRChat");
        assert!(!texts.changed("spout"));
        texts.keep("spout", "OBS".into(), "VRChat");
        assert_eq!(texts.take("spout").as_deref(), Some("OBS"));
        assert!(!texts.changed("spout"));
        texts.keep("afk", "Getting food".into(), "");
        texts.forget("afk");
        assert_eq!(texts.shown("afk", "Away"), "Away");
    }
}
