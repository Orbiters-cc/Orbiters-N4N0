//! The wake word: a sentence for N4N0 starts with its name, said "Nano" ("Nano, put my hoodie on", "Hey Nano what time
//! is it"). The microphone's sentences are transcribed on this computer; only those that start with a wake word go
//! further.

use crate::controls::normalize;

/// Greetings that may come before the wake word ("et" is how speech recognition writes a French "Hé").
const LEADS: &[&str] = &["hey", "hi", "hello", "ok", "okay", "yo", "dis", "eh", "he", "et", "salut", "bonjour", "alors", "oh"];

/// How speech recognition writes "Nano" (and "Dis Nano", "Oh Nano") when it mishears it, seen with French and English
/// voices. Only as the whole sentence: "Nana is…" also starts ordinary sentences, which must not go further.
const NANO_HEARD_AS: &[&str] = &["nana", "dano", "onano"];

/// The token sequences that stand for a wake word: its own (`true`), and how "Nano" gets misheard (`false`: only when
/// nothing follows). N4N0 written as such is said "Nano".
fn forms(word: &str) -> Vec<(Vec<String>, bool)> {
    let split = |w: &str| -> Vec<String> { normalize(w).split([' ', '/']).filter(|t| !t.is_empty()).map(str::to_string).collect() };
    let mut forms = vec![(split(word), true)];
    if matches!(normalize(word).as_str(), "nano" | "n4n0") {
        forms.push((split("nano"), true));
        forms.extend(NANO_HEARD_AS.iter().map(|w| (split(w), false)));
    }
    forms.retain(|(f, _)| !f.is_empty());
    forms
}

/// What follows the wake word when `text` starts with one of `words` (after at most two greetings), as said;
/// `Some("")` when the sentence was only the wake word, `None` when it does not start with one.
pub fn command(text: &str, words: &[String]) -> Option<String> {
    let wake: Vec<(Vec<String>, bool)> = words.iter().flat_map(|w| forms(w)).collect();
    // Words of the sentence with where each one ends.
    let mut tokens: Vec<(String, usize)> = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        if c.is_whitespace() || c == ',' {
            if let Some(s) = start.take() {
                tokens.push((normalize(&text[s..i]), i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
        if tokens.len() > 6 {
            break;
        }
    }
    for leads in 0..=2usize {
        if leads > 0 && !tokens.get(leads - 1).is_some_and(|(t, _)| LEADS.contains(&t.as_str())) {
            break;
        }
        for (w, own) in &wake {
            let end = leads + w.len();
            if end <= tokens.len() && tokens[leads..end].iter().map(|(t, _)| t).eq(w.iter()) {
                let rest = &text[tokens[end - 1].1..];
                let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | '!' | '?' | ':' | ';' | '-')).trim_end();
                if !own && !rest.is_empty() {
                    continue;
                }
                return Some(rest.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> Vec<String> {
        vec!["nano".into()]
    }

    // The sentences below are how NVIDIA Parakeet wrote Windows' voices (David, Zira, Hazel, Hortense) saying them.
    #[test]
    fn finds_the_command_after_the_wake_word() {
        assert_eq!(command("Nano, mets mon souhait à capuche.", &words()).as_deref(), Some("mets mon souhait à capuche."));
        assert_eq!(command("Hey Nano, what time is it?", &words()).as_deref(), Some("what time is it?"));
        assert_eq!(command("Ok Nano, allume la lumière.", &words()).as_deref(), Some("allume la lumière."));
        assert_eq!(command("Et Nano, mets de la musique.", &words()).as_deref(), Some("mets de la musique."));
        assert_eq!(command("Nano", &words()).as_deref(), Some(""));
        assert_eq!(command("Dis Nano.", &words()).as_deref(), Some(""));
        assert_eq!(command("Salut Nano!", &words()).as_deref(), Some(""));
    }

    #[test]
    fn ignores_other_sentences() {
        assert_eq!(command("I saw a nano drone yesterday", &words()), None);
        assert_eq!(command("Nanotech is cool", &words()), None);
        assert_eq!(command("", &words()), None);
        assert_eq!(command("hey there nano", &words()), None);
    }

    #[test]
    fn hears_nano_through_recognition_mistakes_said_alone() {
        // Misheard, the name alone still calls N4N0 (it then listens to the next sentence)...
        assert_eq!(command("Nana", &words()).as_deref(), Some(""));
        assert_eq!(command("Hi Nana", &words()).as_deref(), Some(""));
        assert_eq!(command("Dano.", &words()).as_deref(), Some(""));
        assert_eq!(command("Onano !", &words()).as_deref(), Some(""));
        // ...but a sentence that only starts like it is ordinary speech: it goes nowhere.
        assert_eq!(command("Dano, prend une photo.", &words()), None);
        assert_eq!(command("Onano, prend une photo.", &words()), None);
        assert_eq!(command("Nana is coming tonight", &words()), None);
        // The wake word itself, however the sentence goes on.
        assert_eq!(command("Nano. Put my hoodie on.", &words()).as_deref(), Some("Put my hoodie on."));
    }

    #[test]
    fn n4n0_written_is_said_nano() {
        assert_eq!(command("Nano, save this video please.", &["N4N0".into()]).as_deref(), Some("save this video please."));
        assert_eq!(command("Nana", &["N4N0".into()]).as_deref(), Some(""));
    }

    #[test]
    fn several_word_wake_words() {
        assert_eq!(command("Okay Computer, play music", &["okay computer".into()]).as_deref(), Some("play music"));
    }
}
