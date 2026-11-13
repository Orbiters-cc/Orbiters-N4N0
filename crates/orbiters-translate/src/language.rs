//! Which language a line read seems to be in, and whether it needs translating at all: by its letters' scripts first
//! (kana is Japanese, Hangul Korean, Cyrillic Russian or a neighbour), then, for Latin letters, by the common short words
//! of the languages people around VRChat write most. A line already in a language the person reads, or with no words
//! (numbers, a lone letter, a time), is left as it is. What stays unsure goes to the translator, which is told to give
//! back a line already in the target language unchanged: a translation equal to its source is then not drawn.

/// A guess at a line's language: a BCP 47 tag, or `None` when the letters do not say.
pub fn guess(text: &str) -> Option<&'static str> {
    let c = Counts::of(text);
    if c.letters == 0 {
        return None;
    }
    if c.kana > 0 {
        return Some("ja");
    }
    if c.hangul * 2 >= c.letters {
        return Some("ko");
    }
    if c.han * 2 >= c.letters {
        return Some("zh");
    }
    if c.cyrillic * 2 >= c.letters {
        return Some(if text.chars().any(|ch| "іїєґ".contains(ch.to_lowercase().next().unwrap_or(ch))) { "uk" } else { "ru" });
    }
    if c.latin * 2 >= c.letters {
        return latin(text);
    }
    None
}

/// The primary language of a tag ("pt-BR" is "pt").
pub fn primary(tag: &str) -> String {
    tag.split(['-', '_']).next().unwrap_or(tag).to_ascii_lowercase()
}

/// Whether a line read is worth sending to the translator for `target`, the person reading `reads` too (primary tags).
pub fn needs_translating(text: &str, target: &str, reads: &[String]) -> bool {
    let c = Counts::of(text);
    // Nothing to read: numbers, a time, symbols, one Latin or Cyrillic letter. One Chinese character or kana or Hangul
    // syllable is a word ("男", "出口"'s "口" alone on a door).
    if c.letters < 2 && c.han + c.kana + c.hangul == 0 {
        return false;
    }
    let target = primary(target);
    match guess(text) {
        Some(l) => {
            // Chinese characters alone are Japanese too: a Japanese reader reads them.
            let l = if l == "zh" && target == "ja" { "ja" } else { l };
            l != target && !reads.iter().any(|r| primary(r) == l)
        }
        // Plain unaccented Latin words of no known language are most often English around VRChat (its menus, names,
        // chat): left alone for someone who reads English.
        None if text.is_ascii() && (target == "en" || reads.iter().any(|r| primary(r) == "en")) => false,
        None => true,
    }
}

#[derive(Default)]
struct Counts {
    letters: usize,
    latin: usize,
    cyrillic: usize,
    han: usize,
    kana: usize,
    hangul: usize,
}

impl Counts {
    fn of(text: &str) -> Self {
        let mut c = Counts::default();
        for ch in text.chars() {
            let u = ch as u32;
            if !ch.is_alphabetic() {
                continue;
            }
            c.letters += 1;
            match u {
                0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => c.kana += 1,
                0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => c.hangul += 1,
                0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x20000..=0x2FFFF => c.han += 1,
                0x0400..=0x04FF | 0x0500..=0x052F => c.cyrillic += 1,
                _ if ch.is_ascii_alphabetic() || (0x00C0..=0x024F).contains(&u) => c.latin += 1,
                _ => {}
            }
        }
        c
    }
}

/// Short words that mark a Latin-script language (articles, prepositions, pronouns: what signs and chat lines share).
const WORDS: &[(&str, &[&str])] = &[
    ("en", &["the", "and", "of", "to", "in", "is", "you", "for", "on", "with", "this", "that", "are", "it", "your", "welcome", "here", "please", "no", "not", "be", "at", "my", "we", "do", "go", "all", "can", "what", "how", "hi", "hello", "thanks", "lol", "yes", "a", "an", "or", "from", "by", "me", "so", "if", "out", "up", "just", "like", "now"]),
    ("fr", &["le", "la", "les", "des", "et", "est", "un", "une", "du", "de", "pour", "dans", "sur", "avec", "pas", "ne", "vous", "je", "tu", "il", "elle", "nous", "bienvenue", "ici", "merci", "oui", "non", "au", "aux", "ce", "cette", "mon", "ma", "salut", "bonjour", "qui", "que", "sont", "être", "très", "mais", "ou"]),
    ("es", &["el", "la", "los", "las", "y", "es", "un", "una", "del", "de", "para", "en", "con", "por", "que", "no", "sí", "si", "bienvenido", "bienvenidos", "aquí", "gracias", "hola", "mi", "tu", "yo", "su", "al", "lo", "muy", "pero", "como", "está", "son"]),
    ("pt", &["o", "a", "os", "as", "e", "é", "um", "uma", "do", "da", "dos", "das", "de", "para", "em", "no", "na", "com", "por", "que", "não", "sim", "bem-vindo", "bem-vindos", "aqui", "obrigado", "obrigada", "olá", "oi", "meu", "minha", "você", "eu", "muito", "mas", "como", "está", "são"]),
    ("de", &["der", "die", "das", "und", "ist", "ein", "eine", "nicht", "mit", "für", "auf", "zu", "den", "dem", "von", "ich", "du", "wir", "sie", "willkommen", "hier", "danke", "ja", "nein", "hallo", "bitte", "auch", "aber", "oder"]),
    ("it", &["il", "lo", "la", "gli", "le", "e", "è", "un", "una", "di", "del", "della", "per", "in", "con", "su", "non", "sì", "benvenuto", "benvenuti", "qui", "grazie", "ciao", "io", "tu", "noi", "che", "ma", "sono"]),
];

/// The Latin-script language whose short words a line uses most, when it uses enough of them to tell.
fn latin(text: &str) -> Option<&'static str> {
    let words: Vec<String> = text.split(|c: char| !(c.is_alphabetic() || c == '-' || c == '\'')).filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return None;
    }
    let mut best: Option<(&'static str, usize)> = None;
    let mut second = 0;
    for (lang, list) in WORDS {
        let n = words.iter().filter(|w| list.contains(&w.as_str())).count();
        match best {
            Some((_, b)) if n <= b => second = second.max(n),
            _ => {
                second = best.map_or(0, |(_, b)| b).max(second);
                best = Some((lang, n));
            }
        }
    }
    let (lang, n) = best?;
    // Letters only some languages use settle a tie.
    let accented = |set: &str| text.chars().any(|c| set.contains(c));
    if n == 0 || n == second {
        if accented("ãõç") && !accented("ñ") {
            return Some("pt");
        }
        if accented("ñ¿¡") {
            return Some("es");
        }
        if accented("ßäöü") {
            return Some("de");
        }
        if accented("èêëœàâîûù") {
            return Some("fr");
        }
        return None;
    }
    // One short word in a long line says little.
    (n * 4 >= words.len().min(8) || n >= 2).then_some(lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_say_the_language() {
        assert_eq!(guess("出口はこちらです"), Some("ja"));
        assert_eq!(guess("欢迎来到我们的世界"), Some("zh"));
        assert_eq!(guess("안녕하세요 친구들"), Some("ko"));
        assert_eq!(guess("Добро пожаловать"), Some("ru"));
        assert_eq!(guess("Welcome to the club"), Some("en"));
        assert_eq!(guess("Bienvenue dans le monde"), Some("fr"));
        assert_eq!(guess("12:30"), None);
    }

    #[test]
    fn what_the_person_reads_is_left_alone() {
        let none: Vec<String> = vec![];
        assert!(!needs_translating("Welcome to the club", "en", &none));
        assert!(needs_translating("Welcome to the club", "fr", &none));
        assert!(!needs_translating("Welcome to the club", "fr", &["en".to_string()]));
        assert!(needs_translating("出口はこちらです", "en", &none));
        assert!(!needs_translating("出口はこちらです", "ja-JP", &none));
        // Kanji alone, for a Japanese reader.
        assert!(!needs_translating("非常口", "ja", &none));
        assert!(!needs_translating("12:30 !", "en", &none));
        assert!(!needs_translating("x", "en", &none));
        assert!(needs_translating("男", "en", &none), "one Chinese character is a word");
        // Unsure: the translator decides (and gives it back unchanged when it is already in the target language).
        assert!(needs_translating("Mixer", "fr", &none));
        assert!(!needs_translating("Respawn", "en", &none));
    }
}
