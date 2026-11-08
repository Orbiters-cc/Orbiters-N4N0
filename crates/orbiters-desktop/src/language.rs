//! Which language this process speaks. The settings hold the last language the account gave or the person picked, and whether a pick has yet to reach
//! the account (`controller/language.rs` keeps them with the account, in the background). With none, this computer's. Followed wherever settings
//! arrive, so a change shows in what is drawn next, in every process, with no restart.

use crate::device::WindowsLocale;
use orbiters_core::settings::Settings;
use orbiters_i18n::{normalize, Language};
use orbiters_n4n0::Locale;

/// This computer's language, English where it is not one the website has.
pub fn computer() -> Language {
    normalize(&WindowsLocale.locale()).unwrap_or(Language::En)
}

/// The language `settings` ask for. `ORBITERS_LANGUAGE` (a tag) wins over it in this process and those it starts, and goes nowhere: for pictures taken in a
/// language other than the account's, without changing the account.
pub fn wanted(settings: &Settings, computer: Language) -> Language {
    if let Some(forced) = std::env::var("ORBITERS_LANGUAGE").ok().as_deref().and_then(normalize) {
        return forced;
    }
    settings.language.as_deref().and_then(normalize).unwrap_or(computer)
}

/// Takes the language `settings` ask for: whether it changed.
pub fn follow(settings: &Settings) -> bool {
    let language = wanted(settings, computer());
    let changed = orbiters_i18n::set_language(language);
    if changed {
        // Kanji in Japanese forms for Japanese text, whatever this computer's language.
        orbiters_ui::fonts::prefer_language(orbiters_i18n::language().tag());
    }
    changed
}

/// The picker's places: this computer's, then each language.
pub fn choices() -> Vec<Option<Language>> {
    std::iter::once(None).chain(Language::ALL.into_iter().map(Some)).collect()
}

/// What a place in the picker saves, as a settings patch.
pub fn patch(choice: Option<Language>) -> serde_json::Value {
    serde_json::json!({ "language": choice.map(Language::tag), "language_unsent": true })
}

/// The place in the picker `settings` are at.
pub fn chosen(settings: &Settings) -> Option<Language> {
    settings.language.as_deref().and_then(normalize)
}

/// A place in the picker, for people: a language in its own name, so whoever cannot read this menu can still find theirs.
pub fn name_of(choice: Option<Language>) -> String {
    match choice {
        Some(language) => language.native_name().to_string(),
        None => orbiters_i18n::tr!("Same as this computer ({language})", language = computer().native_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_or_a_pick_wins_and_this_computer_is_the_rest() {
        let settings = |language: Option<&str>| Settings { language: language.map(str::to_string), ..Settings::default() };
        assert_eq!(wanted(&settings(None), Language::Fr), Language::Fr, "nothing chosen");
        assert_eq!(wanted(&settings(Some("ja")), Language::Fr), Language::Ja);
        assert_eq!(wanted(&settings(Some("pt-BR")), Language::En), Language::PtBr);
        assert_eq!(wanted(&settings(Some("klingon")), Language::Es), Language::Es, "a value from a newer website is not a language here");
    }

    /// What the picker saves is taken by the settings, and a later read gives the same place back.
    #[test]
    fn every_place_in_the_picker_saves_and_reads_back() {
        for choice in choices() {
            let (saved, left_out) = Settings::default().patched(&patch(choice));
            assert!(left_out.is_empty(), "{left_out:?}");
            assert_eq!(chosen(&saved), choice);
            assert!(saved.language_unsent, "it has yet to reach the account");
        }
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!((old.language, old.language_unsent), (None, false), "settings saved before this follow the computer");
    }

    #[test]
    fn each_language_is_named_in_itself_in_any_language() {
        for language in Language::TRANSLATED {
            orbiters_i18n::in_language(language, || {
                assert_eq!(name_of(Some(Language::Ja)), "日本語");
                assert_eq!(name_of(Some(Language::Fr)), "Français");
                assert!(name_of(None).contains(computer().native_name()), "{}", name_of(None));
                assert_ne!(name_of(None), "Same as this computer (English)");
            });
        }
    }
}
