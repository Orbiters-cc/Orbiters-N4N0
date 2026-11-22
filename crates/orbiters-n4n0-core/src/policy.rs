//! The policy between N4N0's assistant and what it asks Orbiters - N4N0 to do (contracts `Docs/n4n0-assistant.md`,
//! client tools). The assistant reads text other people chose (a world's name, a video's title, an avatar's labels), so
//! a tool call is a request, not an order: the desktop runs one only once this policy lets it.
//!
//! - Only the desktop's own tools exist ([`crate::tools::parse`] knows no other), each kind at most so many times in a
//!   turn, at most [`CALLS_PER_ROUND`] calls in one answer and [`CALLS_PER_TURN`] in the whole turn.
//! - What lasts, or shows to everyone beyond the moment, waits for the owner's yes: holding a control (a pin kept in the
//!   settings and put back at every avatar load), a new AFK message, the clipboard (its text shown), a control the
//!   avatar's menu tags nsfw. The desktop asks in its own words ([`question`]) and only the owner's next sentence, said
//!   or typed, answers it ([`reply`]): nothing the assistant writes can.
//! - N4N0's own controls, the system's and Orbiters - N4N0's parameters are never offered at all
//!   ([`crate::controls::Controls`]).

use crate::context::{quoted, NAME_CHARS};
use crate::tools::Action;
use std::collections::HashMap;

/// Calls one answer may make; the rest are refused (each still gets its result).
pub const CALLS_PER_ROUND: usize = 8;
/// Calls a whole turn may make.
pub const CALLS_PER_TURN: usize = 24;
/// Characters of a text the owner is asked about (the clipboard's, an AFK message).
const ASKED_CHARS: usize = 80;

/// The kinds of calls, each with its limit in a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Avatar,
    Find,
    Hold,
    Photo,
    SaveVideo,
    ListVideos,
    Clipboard,
    Afk,
    Mood,
    Media,
    Light,
    /// The app's features (`crate::intents`): their risk class gates each, this counts them.
    App,
}

fn kind(action: &Action) -> Kind {
    match action {
        Action::SetControl { .. } | Action::SetParameter { .. } => Kind::Avatar,
        Action::FindControls { .. } => Kind::Find,
        Action::Hold { .. } => Kind::Hold,
        Action::TakePhoto { .. } => Kind::Photo,
        Action::SaveVideo => Kind::SaveVideo,
        Action::ListSavedVideos => Kind::ListVideos,
        Action::CopyToClipboard { .. } => Kind::Clipboard,
        Action::SetAfkMessage { .. } => Kind::Afk,
        Action::Mood { .. } => Kind::Mood,
        Action::Media { .. } => Kind::Media,
        Action::Light { .. } => Kind::Light,
        Action::App(_) => Kind::App,
    }
}

/// How many calls of a kind one turn may make, and what they are called when refused.
fn limit(kind: Kind) -> (usize, &'static str) {
    match kind {
        Kind::Avatar => (12, "avatar changes"), // i18n: not shown
        Kind::Find => (6, "control searches"), // i18n: not shown
        Kind::Hold => (4, "held controls"), // i18n: not shown
        Kind::Photo => (1, "pictures"),
        Kind::SaveVideo => (2, "saved videos"), // i18n: not shown
        Kind::ListVideos => (2, "lists of saved videos"), // i18n: not shown
        Kind::Clipboard => (1, "clipboard copies"), // i18n: not shown
        Kind::Afk => (1, "AFK message changes"), // i18n: not shown
        Kind::Mood => (3, "moods"),
        Kind::Media => (6, "music controls"), // i18n: not shown
        Kind::Light => (4, "light changes"), // i18n: not shown
        Kind::App => (12, "Orbiters - N4N0 actions"), // i18n: not shown
    }
}

/// What the policy says about a call.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// It runs now.
    Run,
    /// It waits for the owner's yes; `what` says what will be done, in the desktop's words ("copy … to the clipboard").
    Confirm { what: String },
    /// It does not run: `why` goes back to the assistant as the call's result.
    Refuse { why: String },
}

/// What the desktop found about the control a call names.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// Its menu path ("Clothes / Hoodie"): the avatar maker's labels.
    pub path: String,
    /// The avatar's menu tags it nsfw.
    pub nsfw: bool,
}

/// The calls of one turn so far.
#[derive(Debug, Default)]
pub struct TurnPolicy {
    round: usize,
    turn: usize,
    kinds: HashMap<Kind, usize>,
}

impl TurnPolicy {
    /// The calls of a new answer: one answer's count starts again.
    pub fn next_round(&mut self) {
        self.round = 0;
    }

    /// Whether `action` runs now, waits for the owner's yes or does not run. `control`: the control it names, as the
    /// desktop found it (none when it names none, or none was found: the action then fails by itself).
    pub fn review(&mut self, action: &Action, control: Option<&Control>) -> Verdict {
        self.round += 1;
        self.turn += 1;
        if self.round > CALLS_PER_ROUND {
            return Verdict::Refuse { why: format!("At most {CALLS_PER_ROUND} actions in one answer: do the rest in the next one.") }; // i18n: not shown
        }
        if self.turn > CALLS_PER_TURN {
            return Verdict::Refuse { why: "That is enough actions for one request: tell the owner what is left to do.".into() }; // i18n: not shown
        }
        let kind = kind(action);
        let used = self.kinds.entry(kind).or_default();
        *used += 1;
        let (most, words) = limit(kind);
        if *used > most {
            return Verdict::Refuse { why: format!("No more {words} in this request (at most {most}).") }; // i18n: not shown
        }
        let confirm = match action {
            Action::Hold { release, .. } => !release,
            Action::CopyToClipboard { .. } => true,
            Action::SetAfkMessage { text, .. } => text.is_some(),
            // A raw parameter write names a control too (its name, path or label): the same nsfw rule.
            Action::SetControl { .. } | Action::SetParameter { .. } => control.is_some_and(|c| c.nsfw),
            _ => false,
        };
        if confirm {
            Verdict::Confirm { what: what(action, control) }
        } else {
            Verdict::Run
        }
    }
}

/// What `action` does, as the desktop asks the owner about it ("copy … to your clipboard"). The texts it quotes (a
/// control's labels, what to copy) are cut short and quoted as JSON strings: the line stays the desktop's own.
pub fn what(action: &Action, control: Option<&Control>) -> String {
    let named = |fallback: &str| match control {
        Some(c) => quoted(&c.path, NAME_CHARS),
        None => quoted(fallback, NAME_CHARS),
    };
    let setting = |on: Option<bool>, value: Option<f32>| match (on, value) {
        (_, Some(v)) => format!(" at {v:.2}"),
        (Some(false), None) => " off".to_string(), // i18n: not shown (what the AI reads)
        _ => " on".to_string(),
    };
    let nsfw = if control.is_some_and(|c| c.nsfw) { ", which the avatar's menu tags nsfw" } else { "" }; // i18n: not shown
    match action {
        Action::SetControl { control: name, on, value } => {
            let verb = match (on, value) {
                (_, Some(v)) => return format!("set {} to {v:.2}{nsfw}", named(name)), // i18n: not shown
                (Some(false), None) => "turn off", // i18n: not shown
                _ => "turn on", // i18n: not shown
            };
            format!("{verb} {}{nsfw}", named(name))
        }
        Action::Hold { control: name, on, value, .. } => {
            format!("keep {}{} (put back after every reset and avatar change, until released){nsfw}", named(name), setting(*on, *value)) // i18n: not shown
        }
        Action::CopyToClipboard { text } => format!("copy to your clipboard: {}", quoted(text, ASKED_CHARS)), // i18n: not shown
        Action::SetAfkMessage { text, enabled } => {
            let text = text.as_deref().unwrap_or_default();
            let switch = match enabled {
                Some(true) => ", and turn it on", // i18n: not shown
                Some(false) => ", and keep it off", // i18n: not shown
                None => "",
            };
            format!("show {} to people while you are away{switch}", quoted(text, ASKED_CHARS)) // i18n: not shown
        }
        Action::SetParameter { name, value } => format!("set {} to {value:.2}{nsfw}", named(name)), // i18n: not shown
        Action::FindControls { .. } => "search the avatar's controls".into(), // i18n: not shown
        Action::TakePhoto { .. } => "take a picture".into(), // i18n: not shown
        Action::SaveVideo => "save the video".into(), // i18n: not shown
        Action::ListSavedVideos => "read your saved videos".into(), // i18n: not shown
        Action::Mood { .. } => "change N4N0's look".into(), // i18n: not shown
        Action::Media { .. } => "control the music".into(), // i18n: not shown
        Action::Light { .. } => "change N4N0's light".into(), // i18n: not shown
        Action::App(intent) => intent.what(),
    }
}

/// Whether `action` changes something (the avatar, the settings, the clipboard, a picture taken), which the assistant
/// must hear of even when its turn fails afterwards.
pub fn changes(action: &Action) -> bool {
    if let Action::App(intent) = action {
        return intent.spec().changes;
    }
    !matches!(action, Action::FindControls { .. } | Action::ListSavedVideos | Action::Mood { .. } | Action::Media { command: None, volume_change: None })
}

/// What the owner is asked, in their language: one whole sentence for an action that waits for their yes (TXT-22). The
/// assistant keeps [`what`]'s English. The texts it quotes are cut short and quoted, as in [`what`].
pub fn ask(action: &Action, control: Option<&Control>) -> String {
    use orbiters_i18n::tr;
    let named = |fallback: &str| match control {
        Some(c) => quoted(&c.path, NAME_CHARS),
        None => quoted(fallback, NAME_CHARS),
    };
    let value = |v: f32| format!("{v:.2}");
    let asked = match action {
        Action::SetControl { control: name, on, value: v } | Action::Hold { control: name, on, value: v, release: false } => {
            let held = matches!(action, Action::Hold { .. });
            match (held, on, v) {
                (false, _, Some(v)) => tr!("N4N0 asks to set {control} to {value}.", control = named(name), value = value(*v)),
                (false, Some(false), None) => tr!("N4N0 asks to turn off {control}.", control = named(name)),
                (false, _, None) => tr!("N4N0 asks to turn on {control}.", control = named(name)),
                (true, _, Some(v)) => {
                    tr!("N4N0 asks to keep {control} at {value}, put back after every reset and avatar change until released.", control = named(name), value = value(*v))
                }
                (true, Some(false), None) => tr!("N4N0 asks to keep {control} off, put back after every reset and avatar change until released.", control = named(name)),
                (true, _, None) => tr!("N4N0 asks to keep {control} on, put back after every reset and avatar change until released.", control = named(name)),
            }
        }
        Action::SetParameter { name, value: v } => tr!("N4N0 asks to set {control} to {value}.", control = named(name), value = value(*v)),
        Action::CopyToClipboard { text } => tr!("N4N0 asks to copy to your clipboard: {text}", text = quoted(text, ASKED_CHARS)),
        Action::SetAfkMessage { text, enabled } => {
            let text = quoted(text.as_deref().unwrap_or_default(), ASKED_CHARS);
            match enabled {
                Some(true) => tr!("N4N0 asks to show {text} to people while you are away, and to turn it on.", text = text),
                Some(false) => tr!("N4N0 asks to show {text} to people while you are away, and to keep it off.", text = text),
                None => tr!("N4N0 asks to show {text} to people while you are away.", text = text),
            }
        }
        // The app's intents ask in their own words.
        Action::App(intent) => crate::intents::ask(intent),
        // Nothing else waits for a yes ([`TurnPolicy::review`]).
        other => what(other, control), // i18n: not shown
    };
    match control.filter(|c| c.nsfw) {
        Some(c) => format!("{asked} {}", tr!("The avatar's menu tags {control} as nsfw.", control = quoted(&c.path, NAME_CHARS))),
        None => asked,
    }
}

/// The owner's question: the sentences of [`ask`], then what to say, in their language.
pub fn asking(asks: &[String]) -> String {
    let close = match asks.len() {
        0 => return String::new(),
        1 => orbiters_i18n::tr("Say yes to do it."),
        _ => orbiters_i18n::tr("Say yes to do them."),
    };
    format!("{} {close}", asks.join(" "))
}

/// Yes and no in Japanese: a sentence of it has no spaces, so the whole text is matched, a word and what may follow it.
const YES_JA: &[&str] = &["はい", "うん", "ええ", "いいよ", "いいです", "どうぞ", "お願い", "おねがい", "オッケー", "わかった"]; // i18n: not shown
const NO_JA: &[&str] = &["いいえ", "いや", "だめ", "ダメ", "やめて", "やめる", "中止", "キャンセル"]; // i18n: not shown
/// What may follow a Japanese yes or no without changing it.
const WITH_JA: &[&str] = &["", "どうぞ", "します", "してください", "お願いします", "おねがいします", "ください", "です", "よ", "ね", "だよ"]; // i18n: not shown

fn reply_ja(text: &str) -> Option<bool> {
    let t: String = text.chars().filter(|c| !c.is_whitespace() && !matches!(c, '、' | '。' | '！' | '？' | '!' | '?' | '.' | ',' | '〜' | 'ー')).collect();
    let t = t.strip_prefix("ナノ").or_else(|| t.strip_prefix("N4N0")).unwrap_or(&t);
    let said = |words: &[&str]| words.iter().any(|w| t.strip_prefix(w).is_some_and(|rest| WITH_JA.contains(&rest)));
    match (said(YES_JA), said(NO_JA)) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Words that say yes, in the languages the owner may speak (lowercase, without accents).
const YES: &str = concat!(
    "yes yeah yep yup ya yea sure ok okay okey kay confirm confirmed affirmative absolutely definitely course do go oui ouais ",
    "ouaip ouep accord dac vas allez fais confirme carrement evidemment bien si claro vale ja jawohl genau da да sim tak hai"
);
/// Words that say no.
const NO: &str = "no nope nah non cancel stop don dont not never annule annuler laisse pas nein nee нет nao nie"; // i18n: not shown
/// Words that may come with a yes or a no without changing it.
const WITH: &str = concat!(
    "please pls plz it ahead for of thanks thank you then now nano n4n0 merci stp svp s il te plait y le la l d sur tomber ",
    "peine mind t por favor bitte пожалуйста thx alright right"
);

/// Whether `word` is one of `words` (separated by spaces).
fn among(words: &str, word: &str) -> bool {
    words.split_whitespace().any(|w| w == word)
}

/// The owner's answer to a question: `Some(true)` for a yes ("yes", "oui vas-y", "ok, do it please"), `Some(false)` for
/// a no ("no thanks", "laisse tomber"), `None` for anything else (a new request, which leaves the question unanswered).
pub fn reply(text: &str) -> Option<bool> {
    if text.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}')) {
        return reply_ja(text);
    }
    let words: Vec<String> = text
        .chars()
        .map(|c| match orbiters_contracts::n4n0::unaccented(c) {
            Some(base) => base.to_lowercase(),
            None => c.to_lowercase().collect(),
        })
        .collect::<String>()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    if words.is_empty() || words.len() > 6 {
        return None;
    }
    let has = |list: &str| words.iter().any(|w| among(list, w));
    if !words.iter().all(|w| among(YES, w) || among(NO, w) || among(WITH, w)) {
        return None;
    }
    match (has(YES), has(NO)) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Mood;

    fn hoodie(on: bool) -> Action {
        Action::SetControl { control: "hoodie".into(), on: Some(on), value: None }
    }

    #[test]
    fn what_lasts_or_shows_waits_for_the_owner() {
        let mut policy = TurnPolicy::default();
        assert_eq!(policy.review(&hoodie(true), Some(&Control { path: "Clothes / Hoodie".into(), nsfw: false })), Verdict::Run);
        let nsfw = Control { path: "Outfits / Lingerie".into(), nsfw: true };
        let Verdict::Confirm { what } = policy.review(&hoodie(true), Some(&nsfw)) else { panic!("an nsfw control waits") };
        assert_eq!(what, "turn on \"Outfits / Lingerie\", which the avatar's menu tags nsfw");
        let raw = Action::SetParameter { name: "Lingerie".into(), value: 1.0 };
        let Verdict::Confirm { what } = TurnPolicy::default().review(&raw, Some(&nsfw)) else { panic!("a raw write to an nsfw control waits too") };
        assert_eq!(what, "set \"Outfits / Lingerie\" to 1.00, which the avatar's menu tags nsfw");
        let hold = Action::Hold { control: "hoodie".into(), on: Some(true), value: None, release: false };
        assert!(matches!(policy.review(&hold, None), Verdict::Confirm { .. }), "a pin lasts");
        let release = Action::Hold { control: "hoodie".into(), on: None, value: None, release: true };
        assert_eq!(policy.review(&release, None), Verdict::Run, "letting go of a pin does not");
        let copy = Action::CopyToClipboard { text: "https://youtu.be/36YnV9STBqc".into() };
        let Verdict::Confirm { what } = policy.review(&copy, None) else { panic!("the clipboard waits") };
        assert!(what.contains("https://youtu.be/36YnV9STBqc"), "the owner sees the text: {what}");
        let afk = Action::SetAfkMessage { text: Some("brb".into()), enabled: Some(true) };
        assert!(matches!(policy.review(&afk, None), Verdict::Confirm { .. }));
        assert_eq!(
            policy.review(&Action::SetAfkMessage { text: None, enabled: Some(false) }, None),
            Verdict::Refuse { why: "No more AFK message changes in this request (at most 1).".into() }
        );
        assert_eq!(policy.review(&Action::Mood { mood: Mood::Happy }, None), Verdict::Run);
        assert_eq!(asking(&[ask(&copy, None)]), "N4N0 asks to copy to your clipboard: \"https://youtu.be/36YnV9STBqc\" Say yes to do it.");
        assert_eq!(asking(&["A.".into(), "B.".into()]), "A. B. Say yes to do them.");
    }

    #[test]
    fn calls_are_capped_per_answer_per_kind_and_per_turn() {
        let mut policy = TurnPolicy::default();
        assert_eq!(policy.review(&Action::TakePhoto { delay_seconds: 0 }, None), Verdict::Run);
        assert!(matches!(policy.review(&Action::TakePhoto { delay_seconds: 0 }, None), Verdict::Refuse { .. }), "one picture a request");
        let mut policy = TurnPolicy::default();
        let verdicts: Vec<Verdict> = (0..10).map(|_| policy.review(&Action::FindControls { query: String::new() }, None)).collect();
        assert_eq!(verdicts.iter().filter(|v| **v == Verdict::Run).count(), 6, "six searches at most");
        let mut policy = TurnPolicy::default();
        let mut run = 0;
        for round in 0..5 {
            policy.next_round();
            for i in 0..10 {
                let action = if i % 2 == 0 { hoodie(true) } else { Action::Mood { mood: Mood::Neutral } };
                run += (policy.review(&action, None) == Verdict::Run) as usize;
            }
            assert!(run <= (round + 1) * CALLS_PER_ROUND);
        }
        assert!(run <= 12 + 3, "avatar changes and moods within their limits: {run}");
    }

    #[test]
    fn the_owner_is_asked_in_whole_sentences_of_their_language() {
        // TXT-22: the question was English pieces glued with commas in every language.
        let nsfw = Control { path: "Outfits / Lingerie".into(), nsfw: true };
        let hoodie = Action::SetControl { control: "lingerie".into(), on: Some(true), value: None };
        assert_eq!(ask(&hoodie, Some(&nsfw)), "N4N0 asks to turn on \"Outfits / Lingerie\". The avatar's menu tags \"Outfits / Lingerie\" as nsfw.");
        let hold = Action::Hold { control: "hoodie".into(), on: Some(true), value: None, release: false };
        assert!(ask(&hold, None).starts_with("N4N0 asks to keep \"hoodie\" on"));
        let afk = Action::SetAfkMessage { text: Some("brb".into()), enabled: Some(true) };
        for language in orbiters_i18n::Language::TRANSLATED {
            let (asked, close) = orbiters_i18n::in_language(language, || (ask(&afk, None), asking(&[ask(&hold, None)])));
            assert!(!asked.starts_with("N4N0 asks") && asked.contains("\"brb\""), "{language:?}: {asked}");
            assert!(!close.contains("Say yes"), "{language:?}: {close}");
        }
    }

    #[test]
    fn hears_yes_and_no_in_japanese_too() {
        for yes in ["はい", "はい、お願いします", "うん", "ええ、どうぞ", "はい！", "ナノ、はい"] {
            assert_eq!(reply(yes), Some(true), "{yes}");
        }
        for no in ["いいえ", "だめ", "やめて", "いいえ、やめてください" ] {
            assert!(reply(no) != Some(true), "{no}");
        }
        assert_eq!(reply("いいえ"), Some(false));
        assert_eq!(reply("はい、帽子もかぶせて"), None, "a new request");
        assert_eq!(reply("sí, por favor"), Some(true));
        assert_eq!(reply("não"), Some(false));
    }

    #[test]
    fn hears_yes_and_no_in_a_few_words() {
        for yes in ["Yes.", "yes please", "OK, fais-le stp", "oui vas-y", "D'accord !", "bien sûr", "Yeah, do it", "go ahead", "да", "Nano, yes"] {
            assert_eq!(reply(yes), Some(true), "{yes}");
        }
        for no in ["No.", "no thanks", "non merci", "laisse tomber", "don't", "нет", "cancel it"] {
            assert_eq!(reply(no), Some(false), "{no}");
        }
        for other in ["yes, and put my hat on too", "what time is it", "", "ok no", "yes yes yes yes yes yes yes"] {
            assert_eq!(reply(other), None, "{other}");
        }
    }
}
