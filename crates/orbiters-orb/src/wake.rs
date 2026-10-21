//! The wake word: a sentence for Orb starts with its name ("Orb, put my hoodie on", "Hey Orb what time is it"). The
//! microphone's sentences are transcribed on this computer; only those that start with a wake word go further.

use crate::controls::normalize;

/// Greetings that may come before the wake word.
const LEADS: &[&str] = &["hey", "hi", "hello", "ok", "okay", "yo", "dis", "eh", "he", "salut", "bonjour", "alors", "oh"];

/// How speech recognition writes "Orb" (and "Dis Orb") when it mishears it, seen with French and English voices.
const ORB_HEARD_AS: &[&str] = &["orbe", "orbs", "o herb", "oh herb", "herb", "drb", "dorb", "d'orb", "or b"];

/// The token sequences that stand for a wake word: its own, and how "orb" gets misheard.
fn forms(word: &str) -> Vec<Vec<String>> {
    let split = |w: &str| -> Vec<String> { normalize(w).split([' ', '/']).filter(|t| !t.is_empty()).map(str::to_string).collect() };
    let mut forms = vec![split(word)];
    if normalize(word) == "orb" {
        forms.extend(ORB_HEARD_AS.iter().map(|w| split(w)));
    }
    forms.retain(|f| !f.is_empty());
    forms
}

/// What follows the wake word when `text` starts with one of `words` (after at most two greetings), as said;
/// `Some("")` when the sentence was only the wake word, `None` when it does not start with one.
pub fn command(text: &str, words: &[String]) -> Option<String> {
    let wake: Vec<Vec<String>> = words.iter().flat_map(|w| forms(w)).collect();
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
        for w in &wake {
            let end = leads + w.len();
            if end <= tokens.len() && tokens[leads..end].iter().map(|(t, _)| t).eq(w.iter()) {
                let rest = &text[tokens[end - 1].1..];
                return Some(
                    rest.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | '!' | '?' | ':' | ';' | '-')).trim_end().to_string(),
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> Vec<String> {
        vec!["orb".into(), "orbe".into()]
    }

    #[test]
    fn finds_the_command_after_the_wake_word() {
        assert_eq!(command("Orb, mets mon sweat à capuche.", &words()).as_deref(), Some("mets mon sweat à capuche."));
        assert_eq!(command("Hey Orb what's up?", &words()).as_deref(), Some("what's up?"));
        assert_eq!(command("ok, Orbe : prends une photo", &words()).as_deref(), Some("prends une photo"));
        assert_eq!(command("Orb!", &words()).as_deref(), Some(""));
        assert_eq!(command("Dis Orb.", &words()).as_deref(), Some(""));
    }

    #[test]
    fn ignores_other_sentences() {
        assert_eq!(command("I saw an orb yesterday", &words()), None);
        assert_eq!(command("Orbiters is cool", &words()), None);
        assert_eq!(command("", &words()), None);
        assert_eq!(command("hey there orb", &words()), None);
    }

    #[test]
    fn hears_orb_through_recognition_mistakes() {
        assert_eq!(command("O Herb, mets mon souhait à capuche s'il te plaît.", &words()).as_deref(), Some("mets mon souhait à capuche s'il te plaît."));
        assert_eq!(command("DRB, organise une soirée cinéma samedi soir.", &words()).as_deref(), Some("organise une soirée cinéma samedi soir."));
        assert_eq!(command("Orb. Save me this video please.", &words()).as_deref(), Some("Save me this video please."));
        assert_eq!(command("Herbs are nice", &words()), None);
    }

    #[test]
    fn several_word_wake_words() {
        assert_eq!(command("Okay Computer, play music", &["okay computer".into()]).as_deref(), Some("play music"));
    }
}
