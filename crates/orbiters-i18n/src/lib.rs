//! What Orbiters - N4N0 says, in the languages of the Orbiters website (`backend/src/i18n/locales.js`). The English text is the key: `tr("Save")`
//! looks it up in the language now, and anything without a translation stays English. Text with values in it names them:
//! `tr!("Starting in {seconds}", seconds = n)`; a translation has the same `{names}` and may put them anywhere.
//!
//! The translations are `catalog/<area>/<language>.json`, each a flat `{ "English": "translation" }`, embedded when this crate is built. An
//! area is one folder, so areas translated side by side touch separate files. `tests/coverage.rs` fails on text a person sees that is
//! not translated, in the files an area says it covers. See `reports/i18n/BATCH-BRIEF.md`.
//!
//! The language is set from outside (`orbiters-desktop`'s `language` module follows the account and this computer). It is read for every text
//! drawn, so nothing needs a restart to change language: what is drawn next is in the new one.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// A language the website supports. Its tag is the one the website stores and sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Language {
    En,
    Ja,
    Fr,
    Es,
    PtBr,
}

impl Language {
    pub const ALL: [Language; 5] = [Language::En, Language::Ja, Language::Fr, Language::Es, Language::PtBr];
    /// Those with translations: English is the text itself.
    pub const TRANSLATED: [Language; 4] = [Language::Ja, Language::Fr, Language::Es, Language::PtBr];

    /// Its tag, as the website has it (`pt-BR`).
    pub const fn tag(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ja => "ja",
            Language::Fr => "fr",
            Language::Es => "es",
            Language::PtBr => "pt-BR",
        }
    }

    /// Its name in itself, whatever the language now: who cannot read the menu can still find their own.
    pub const fn native_name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::Ja => "日本語",
            Language::Fr => "Français",
            Language::Es => "Español",
            Language::PtBr => "Português (Brasil)",
        }
    }

    fn from_index(i: u8) -> Language {
        Language::ALL.get(i as usize).copied().unwrap_or(Language::En)
    }
}

/// The supported language for a tag in any common spelling (`ja-JP`, `pt_BR`, `es-419`), as the website normalizes it: a regional
/// variant shares its language's translation, and European Portuguese readers get the Brazilian one. `None` when it is not supported.
pub fn normalize(tag: &str) -> Option<Language> {
    let tag = tag.trim().replace('_', "-").to_ascii_lowercase();
    let tag = tag.split(['.', '@']).next().unwrap_or_default();
    let shaped = !tag.is_empty() && tag.len() <= 35 && {
        let mut parts = tag.split('-');
        let first = parts.next().unwrap_or_default();
        (2..=3).contains(&first.len()) && first.bytes().all(|b| b.is_ascii_lowercase()) && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
    };
    if !shaped {
        return None;
    }
    match tag.split('-').next().unwrap_or_default() {
        "en" => Some(Language::En),
        "ja" => Some(Language::Ja),
        "fr" => Some(Language::Fr),
        "es" => Some(Language::Es),
        "pt" => Some(Language::PtBr),
        _ => None,
    }
}

static NOW: AtomicU8 = AtomicU8::new(Language::En as u8);
/// A language that stays whatever is chosen: a render in that language.
static PINNED: AtomicU8 = AtomicU8::new(NOT_PINNED);
const NOT_PINNED: u8 = u8::MAX;

thread_local! {
    static HERE: Cell<Option<Language>> = const { Cell::new(None) };
}

/// The language things are in now.
pub fn language() -> Language {
    if let Some(here) = HERE.with(Cell::get) {
        return here;
    }
    Language::from_index(NOW.load(Ordering::Relaxed))
}

/// Takes `language` from now on, on every thread. Whether that is a change.
pub fn set_language(language: Language) -> bool {
    let want = match PINNED.load(Ordering::Relaxed) {
        NOT_PINNED => language as u8,
        pinned => pinned,
    };
    NOW.swap(want, Ordering::Relaxed) != want
}

/// Stays in `language` whatever is set later (`--screenshot --language fr`).
pub fn pin_language(language: Language) {
    PINNED.store(language as u8, Ordering::Relaxed);
    NOW.store(language as u8, Ordering::Relaxed);
}

/// Runs `f` in `language`, on this thread alone: tests running side by side do not see each other's.
pub fn in_language<R>(language: Language, f: impl FnOnce() -> R) -> R {
    let was = HERE.with(|h| h.replace(Some(language)));
    let out = f();
    HERE.with(|h| h.set(was));
    out
}

/// The catalogs built in: (area, language tag, its JSON).
pub const CATALOGS: &[(&str, &str, &str)] = include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// One language's texts, from every area. An area that is not valid JSON fails its test, and is left out here rather than taking the app down.
fn table(language: Language) -> Option<&'static HashMap<&'static str, &'static str>> {
    static TABLES: [OnceLock<HashMap<&'static str, &'static str>>; 5] = [const { OnceLock::new() }; 5];
    if language == Language::En {
        return None;
    }
    Some(TABLES[language as usize].get_or_init(|| {
        let mut table = HashMap::new();
        for (_, tag, json) in CATALOGS.iter().filter(|c| c.1 == language.tag()) {
            let Ok(serde_json::Value::Object(entries)) = serde_json::from_str::<serde_json::Value>(json) else {
                eprintln!("translations for {tag} could not be read");
                continue;
            };
            for (english, translated) in entries {
                if let serde_json::Value::String(translated) = translated {
                    if !translated.is_empty() {
                        table.insert(&*Box::leak(english.into_boxed_str()), &*Box::leak(translated.into_boxed_str()));
                    }
                }
            }
        }
        table
    }))
}

/// `english` in the language now, or itself where there is no translation. The answer lives as long as what it was given: a `&'static str`
/// stays one, so tables of labels can hold it.
pub fn tr<'a>(english: &'a str) -> &'a str {
    match table(language()) {
        Some(table) => table.get(english).copied().unwrap_or(english),
        None => english,
    }
}

/// The same for what reaches the avatar's own panel or live Screen text, in VRChat: a 256-glyph atlas with the accents of French, Spanish and Portuguese and no Japanese
/// (`contracts/Docs/n4n0.md`, Glyphs), so Japanese there would come out as nothing or `?`. English in Japanese, for that place alone.
pub fn tr_latin<'a>(english: &'a str) -> &'a str {
    if language() == Language::Ja {
        english
    } else {
        tr(english)
    }
}

/// The separator between a context and the text in a catalog's key (gettext's).
const CONTEXT: char = '\u{4}';

/// The same where one English word needs two translations: a key of `context` and `english` together, written `"context\u0004english"` in a
/// catalog. English needs nothing for it. Falls back on the text with no context, then English.
pub fn tr_ctx<'a>(context: &str, english: &'a str) -> &'a str {
    if let Some(table) = table(language()) {
        if let Some(found) = table.get(format!("{context}{CONTEXT}{english}").as_str()) {
            return found;
        }
    }
    tr(english)
}

/// `text` with each `{name}` replaced by its value. A name that is not in `values` stays as written, so a mistake shows and breaks nothing;
/// a value is put in as it is, never read for names itself.
pub fn fill(text: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(text.len() + values.len() * 8);
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}').map(|close| (close, &after[..close])) {
            Some((close, name)) if values.iter().any(|(n, _)| *n == name) => {
                let (_, value) = values.iter().find(|(n, _)| *n == name).expect("just looked");
                out.push_str(&value.to_string());
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// English text with values in it, in the language now: `tr!("Starting in {seconds}", seconds = n)`. Text without any is simply [`tr()`].
#[macro_export]
macro_rules! tr {
    ($text:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::fill($crate::tr($text), &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

/// Marks a text for translation where it is written, for a [`tr()`] of it where it is drawn: tables and constants. It does nothing at run time;
/// `tests/coverage.rs` reads it as a key.
pub const fn mark(english: &'static str) -> &'static str {
    english
}

/// A count of something: `tr_n("{count} picture", "{count} pictures", n)`. Both English forms are keys. Japanese has one form (the second's), and
/// French and Brazilian Portuguese take the singular for 0 too, as the website's own plural rules do.
pub fn tr_n(one: &str, other: &str, count: u64) -> String {
    let singular = match language() {
        Language::Ja => false,
        Language::Fr | Language::PtBr => count <= 1,
        Language::En | Language::Es => count == 1,
    };
    fill(tr(if singular { one } else { other }), &[("count", &count)])
}

/// The `{names}` in a text, sorted: what a translation must keep.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        let name = &after[..close];
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            names.push(name);
        }
        rest = &after[close + 1..];
    }
    names.sort_unstable();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_normalize_as_the_website_does() {
        for (tag, language) in [("en", Language::En), ("ja-JP", Language::Ja), ("JA", Language::Ja), ("fr_CA", Language::Fr), ("es-419", Language::Es), ("pt", Language::PtBr), ("pt-PT", Language::PtBr), ("pt_BR", Language::PtBr), ("pt-br", Language::PtBr), ("fr_FR.UTF-8", Language::Fr), ("en-US@rg=frzzzz", Language::En)] {
            assert_eq!(normalize(tag), Some(language), "{tag}");
        }
        for tag in ["", "de", "klingon", "zh-TW", "x", "e n", "C", "123"] {
            assert_eq!(normalize(tag), None, "{tag}");
        }
        for language in Language::ALL {
            assert_eq!(normalize(language.tag()), Some(language));
        }
    }

    #[test]
    fn what_has_no_translation_stays_english() {
        for language in Language::ALL {
            in_language(language, || assert_eq!(tr("A text nothing translates \u{1f47e}"), "A text nothing translates \u{1f47e}"));
        }
    }

    #[test]
    fn a_catalog_text_is_found_in_its_language_and_english_in_english() {
        in_language(Language::En, || assert_eq!(tr("Language"), "Language"));
        in_language(Language::Fr, || assert_eq!(tr("Language"), "Langue"));
        in_language(Language::Ja, || assert_eq!(tr("Language"), "言語"));
        in_language(Language::Es, || assert_eq!(tr("Language"), "Idioma"));
        in_language(Language::PtBr, || assert_eq!(tr("Language"), "Idioma"));
    }

    #[test]
    fn a_static_text_stays_static() {
        fn keep(label: &'static str) -> &'static str {
            tr(label)
        }
        assert_eq!(in_language(Language::Fr, || keep("Language")), "Langue");
    }

    /// `{name}` survives: filled wherever the translation puts it, in any number and order, and a value holding braces is not read again.
    #[test]
    fn values_land_where_a_translation_puts_them() {
        assert_eq!(fill("{b} and {a}", &[("a", &1), ("b", &"two")]), "two and 1");
        assert_eq!(fill("{a}{a}", &[("a", &"x")]), "xx");
        assert_eq!(fill("{a}", &[("a", &"{b}"), ("b", &"no")]), "{b}");
        assert_eq!(fill("{unknown} stays", &[("a", &1)]), "{unknown} stays");
        assert_eq!(fill("a { b", &[("b", &1)]), "a { b");
        assert_eq!(fill("ちま {name} 部屋", &[("name", &"é")]), "ちま é 部屋");
        assert_eq!(placeholders("{b} x {a_1} {} {not a name}"), vec!["a_1", "b"]);
        let n = 3;
        in_language(Language::Fr, || assert_eq!(tr!("Saved {count} of {total}", count = n, total = 5), "Saved 3 of 5"));
    }

    #[test]
    fn counts_take_each_languages_plural() {
        let form = |language, n| in_language(language, || tr_n("{count} thing", "{count} things", n));
        assert_eq!((form(Language::En, 0), form(Language::En, 1), form(Language::En, 2)), ("0 things".into(), "1 thing".into(), "2 things".into()));
        // No translation here: which English form it asked for shows the rule.
        assert_eq!((form(Language::Fr, 0), form(Language::Fr, 2)), ("0 thing".into(), "2 things".into()));
        assert_eq!((form(Language::PtBr, 0), form(Language::Es, 0)), ("0 thing".into(), "0 things".into()));
        assert_eq!(form(Language::Ja, 1), "1 things");
    }

    #[test]
    fn what_reaches_the_avatars_atlas_is_never_japanese() {
        in_language(Language::Ja, || assert_eq!(tr_latin("Language"), "Language"));
        in_language(Language::Fr, || assert_eq!(tr_latin("Language"), "Langue"));
    }

    #[test]
    fn a_context_tells_two_translations_apart_and_falls_back() {
        in_language(Language::Fr, || assert_eq!(tr_ctx("nothing has this context", "Language"), "Langue"));
        in_language(Language::En, || assert_eq!(tr_ctx("nothing has this context", "Language"), "Language"));
    }

    #[test]
    fn a_language_set_on_one_thread_does_not_cross_in_language() {
        in_language(Language::Ja, || {
            assert_eq!(language(), Language::Ja);
            in_language(Language::Fr, || assert_eq!(language(), Language::Fr));
            assert_eq!(language(), Language::Ja);
        });
    }
}
