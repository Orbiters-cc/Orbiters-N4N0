//! The offline path: the most common short commands (a switch on or off, the music and its volume, a page, a 3D picture
//! with its timer, undo) understood from the person's own sentence alone, in the five languages, with no model and no
//! network: they work at once, signed in or not. A sentence is matched whole (after the words that never change its
//! meaning are dropped: [`phrases::FILLERS`]); one that is not exactly a known command, or that joins one with anything
//! else, goes to the model as it is. Only harmless, reversible actions are taken at once; the VRChat status (an account
//! action, r23) is asked about first, as the model's would be.

use super::phrases::{self, Feature, Media};
use super::{Intent, Page};
use crate::menu::{MediaCommand, PhotoMode};
use crate::tools::Action;
use std::collections::HashMap;
use std::sync::OnceLock;

/// What a known sentence (or one part of it) says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Said {
    Switch(Feature, bool),
    /// A feature's name alone, after "and": it takes the switch before it ("turn on fast walk and translation").
    Bare(Feature),
    Media(Media),
    Undo,
    StopRecording,
    Picture(PhotoMode),
    Open(Page),
    /// The VRChat status: an account action, which still waits for the owner's yes (r23).
    Status(super::StatusKind),
}

/// `text` lower case, accents and punctuation gone, as words.
fn words(text: &str) -> Vec<String> {
    let folded: String = text
        .chars()
        .map(|c| match orbiters_contracts::n4n0::unaccented(c) {
            Some(base) => base.to_lowercase().to_string(),
            None => c.to_lowercase().collect(),
        })
        .collect();
    folded.split(|c: char| !(c.is_alphanumeric())).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// A sentence (or a pattern) as it is matched: its words without the fillers.
fn key(text: &str) -> String {
    words(text).into_iter().filter(|w| !phrases::FILLERS.contains(&w.as_str())).collect::<Vec<_>>().join(" ")
}

/// Japanese is matched without spaces, punctuation, the companion's name or a polite ending.
fn key_ja(text: &str) -> String {
    let mut t: String = text.chars().filter(|c| c.is_alphanumeric() || matches!(c, 'ー')).flat_map(char::to_lowercase).collect();
    for name in ["n4n0", "ナノ"] {
        if let Some(rest) = t.strip_prefix(name) {
            t = rest.to_string();
        }
    }
    for polite in phrases::JA_POLITE {
        if let Some(rest) = t.strip_suffix(polite) {
            t = rest.to_string();
        }
    }
    t
}

fn japanese(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}'))
}

const FEATURES: [Feature; 7] = [Feature::FastWalk, Feature::Translation, Feature::Light, Feature::N4n0, Feature::Smoothing, Feature::Assistant, Feature::CameraQr];
const MEDIA: [Media; 8] = [Media::Pause, Media::Play, Media::Next, Media::Previous, Media::Louder, Media::Quieter, Media::Mute, Media::Unmute];

/// Every known sentence, by its key (Japanese ones by theirs).
fn table() -> &'static HashMap<String, Said> {
    static TABLE: OnceLock<HashMap<String, Said>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out: HashMap<String, Said> = HashMap::new();
        let mut add = |lang: usize, pattern: String, said: Said| {
            let k = if lang == 4 { key_ja(&pattern) } else { key(&pattern) };
            if !k.is_empty() {
                // The first meaning given wins: a later language's same words never replace it.
                out.entry(k).or_insert(said);
            }
        };
        for lang in 0..5 {
            for feature in FEATURES {
                let (on, off) = if feature == Feature::N4n0 { (phrases::SHOW[lang], phrases::HIDE[lang]) } else { (phrases::ON[lang], phrases::OFF[lang]) };
                for name in phrases::names(feature)[lang] {
                    if feature != Feature::Assistant {
                        for p in on {
                            add(lang, p.replace("{x}", name), Said::Switch(feature, true));
                        }
                    }
                    for p in off {
                        add(lang, p.replace("{x}", name), Said::Switch(feature, false));
                    }
                }
            }
            for kind in MEDIA {
                for p in phrases::media(kind)[lang] {
                    add(lang, p.to_string(), Said::Media(kind));
                }
            }
            for p in phrases::UNDO[lang] {
                add(lang, p.to_string(), Said::Undo);
            }
            for p in phrases::STOP_RECORDING[lang] {
                add(lang, p.to_string(), Said::StopRecording);
            }
            for p in phrases::PICTURE_WORLD[lang] {
                add(lang, p.to_string(), Said::Picture(PhotoMode::World));
            }
            for p in phrases::PICTURE[lang] {
                add(lang, p.to_string(), Said::Picture(PhotoMode::People));
            }
            for status in [super::StatusKind::JoinMe, super::StatusKind::Online, super::StatusKind::AskMe, super::StatusKind::Busy] {
                for name in phrases::status_names(status) {
                    for p in phrases::STATUS[lang] {
                        add(lang, p.replace("{c}", name), Said::Status(status));
                    }
                }
            }
            for page in Page::ALL {
                for name in phrases::page_names(page)[lang] {
                    for p in phrases::OPEN[lang] {
                        add(lang, p.replace("{p}", name), Said::Open(page));
                    }
                }
            }
        }
        // A feature's name alone, for what follows "and" (never a sentence on its own: see [`understand`]).
        for lang in 0..4 {
            for feature in FEATURES {
                for name in phrases::names(feature)[lang] {
                    let k = format!("\u{1}{}", key(name));
                    out.entry(k).or_insert(Said::Bare(feature));
                }
            }
        }
        out
    })
}

/// The seconds a timer says ("in ten seconds", "dans 5 s", "en 3 segundos", "10秒後に"), and the sentence without them.
fn timer(text: &str) -> (Option<u32>, String) {
    if japanese(text) {
        // "10秒後に…" or "…10秒後": the digits just before 秒 (not the 3 of 3D).
        let chars: Vec<char> = text.chars().collect();
        let at = chars.iter().position(|c| *c == '秒');
        let digits: String = at.map(|at| {
            let start = (0..at).rev().take_while(|i| chars[*i].is_ascii_digit()).last().unwrap_or(at);
            chars[start..at].iter().collect()
        }).unwrap_or_default();
        if let Ok(n) = digits.parse::<u32>() {
            let cut = text.replacen(&format!("{digits}秒後に"), "", 1).replacen(&format!("{digits}秒後"), "", 1).replacen(&format!("{digits}秒で"), "", 1);
            return (Some(n.min(30)), cut);
        }
        return (None, text.to_string());
    }
    let w = words(text);
    for lang in 0..4 {
        for i in 0..w.len() {
            if !phrases::IN[lang].contains(&w[i].as_str()) || i + 1 >= w.len() {
                continue;
            }
            let n = w[i + 1].parse::<u32>().ok().or_else(|| phrases::NUMBERS.iter().find(|(word, _)| *word == w[i + 1]).map(|(_, n)| *n));
            let unit = w.get(i + 2).is_some_and(|u| phrases::SECONDS[lang].contains(&u.as_str()));
            if let (Some(n), true) = (n, unit) {
                let rest: Vec<String> = w[..i].iter().chain(&w[i + 3..]).cloned().collect();
                return (Some(n.min(30)), rest.join(" "));
            }
        }
    }
    (None, text.to_string())
}

/// The parts of a sentence joined by "and" ("et", "y", "e", "then"…).
fn parts(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for w in words(text) {
        if phrases::AND.contains(&w.as_str()) {
            out.push(String::new());
        } else {
            let last = out.last_mut().unwrap();
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(&w);
        }
    }
    out.into_iter().filter(|p| !p.is_empty()).collect()
}

/// What the offline path makes of the person's sentence: the actions, in order, when the whole of it is known; `None`
/// otherwise (the model is asked).
pub fn understand(sentence: &str) -> Option<Vec<Action>> {
    let sentence = sentence.trim();
    if sentence.is_empty() || sentence.chars().count() > 120 {
        return None;
    }
    let (seconds, rest) = timer(sentence);
    let table = table();
    let mut said = Vec::new();
    if japanese(&rest) {
        let pieces: Vec<&str> = rest.split(['、', '，', ',']).filter(|p| !p.trim().is_empty()).collect();
        for piece in pieces.into_iter().map(key_ja).filter(|k| !k.is_empty()) {
            said.push(*table.get(&piece)?);
        }
    } else {
        let mut before: Option<bool> = None;
        for (i, part) in parts(&rest).into_iter().enumerate() {
            let k = key(&part);
            let found = match table.get(&k) {
                Some(s) => *s,
                // A bare name only after a switch.
                None if i > 0 => match (table.get(&format!("\u{1}{k}")), before) {
                    (Some(Said::Bare(f)), Some(on)) => Said::Switch(*f, on),
                    _ => return None,
                },
                None => return None,
            };
            if let Said::Switch(_, on) = found {
                before = Some(on);
            }
            said.push(found);
        }
    }
    if said.is_empty() || said.len() > 4 {
        return None;
    }
    // A timer only with a 3D picture.
    if seconds.is_some() && !said.iter().any(|s| matches!(s, Said::Picture(_))) {
        return None;
    }
    Some(said.into_iter().filter_map(|s| action(s, seconds.unwrap_or(0))).collect()).filter(|a: &Vec<Action>| !a.is_empty())
}

fn action(said: Said, seconds: u32) -> Option<Action> {
    Some(match said {
        Said::Switch(Feature::FastWalk, on) => Action::App(Intent::FastWalk { on }),
        Said::Switch(Feature::Translation, on) => Action::App(Intent::Translation { on }),
        Said::Switch(Feature::Light, on) => Action::Light { on: Some(on), color: None, level: None },
        Said::Switch(Feature::N4n0, on) => Action::App(Intent::ShowN4n0 { on }),
        Said::Switch(Feature::Smoothing, on) => Action::App(Intent::Smoothing { on: Some(on), strength: None }),
        Said::Switch(Feature::Assistant, false) => Action::App(Intent::AssistantOff),
        Said::Switch(Feature::Assistant, true) => return None,
        Said::Switch(Feature::CameraQr, on) => Action::App(Intent::CameraQr { on }),
        Said::Bare(_) => return None,
        Said::Media(m) => {
            let (command, volume_change) = match m {
                Media::Pause => (Some(MediaCommand::Pause), None),
                Media::Play => (Some(MediaCommand::Play), None),
                Media::Next => (Some(MediaCommand::Next), None),
                Media::Previous => (Some(MediaCommand::Previous), None),
                Media::Louder => (None, Some(10)),
                Media::Quieter => (None, Some(-10)),
                Media::Mute => (Some(MediaCommand::Mute(true)), None),
                Media::Unmute => (Some(MediaCommand::Mute(false)), None),
            };
            Action::Media { command, volume_change }
        }
        Said::Undo => Action::App(Intent::Undo),
        Said::StopRecording => Action::App(Intent::StopRecording),
        Said::Picture(of) => Action::App(Intent::Picture3d { of: Some(of), seconds }),
        Said::Open(page) => Action::App(Intent::OpenPage { page }),
        Said::Status(status) => Action::App(Intent::SetStatus { status }),
    })
}
