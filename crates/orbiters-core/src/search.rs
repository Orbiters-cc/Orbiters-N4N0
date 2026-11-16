//! Finding something by its name however the name is written: the one matching key every search by name uses.
//!
//! VRChat names are decorated: full-width letters (`ｎｅｏｎｉｘ`), mathematical and enclosed alphabets (`𝐍𝐞𝐨`, `Ⓝⓔⓞ`),
//! small capitals (`ɴᴇᴏ`), accents, zero-width characters and invisible fillers, emoji and symbols around the name,
//! letters spaced or dotted apart (`n e o n i x`, `n.e.o`), and letters of other scripts chosen for looking Latin
//! (`ηєσηιχ`). A person types the plain letters they hear in their head; [`Query`] finds the name all the same:
//!
//! 1. [`fold`]: Unicode compatibility decomposition (NFKD, which turns the presentation alphabets back into letters),
//!    lower case, the decorating accents dropped, zero-width and invisible characters dropped, the Latin letters NFKD
//!    leaves alone mapped (small capitals, stroked and IPA letters, regional indicator and negative enclosed letters),
//!    katakana read as hiragana; then every run of other characters (spaces, punctuation, symbols, emoji) is one
//!    space, and letters standing alone one after another are joined (`n e o n i x` is `neonix`).
//! 2. The compact form: the folded letters and digits only, so separators inside a name never matter (`neo/nix`).
//! 3. The look-alike form, added on top: a word made only of Latin letters and letters that look Latin (Cyrillic and
//!    Greek ones, as decorated names use them) is also read as the Latin word it imitates. A name keeps its own form
//!    beside it, so a Russian, Greek or Japanese name is still found by its own letters.
//!
//! [`Query::rank`] says how well a name matches: exact, start, word start, inside, separators ignored, then the same on
//! the look-alike forms. Keys are cached per thread, so filtering thousands of names on each keystroke stays cheap.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A name's matching forms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    /// Folded: lower-case letters and digits in words separated by single spaces.
    pub plain: String,
    /// The folded letters and digits alone.
    pub compact: String,
    /// The look-alike reading (plain, compact), when it differs.
    pub latin: Option<(String, String)>,
}

impl Key {
    pub fn new(text: &str) -> Self {
        let plain = fold(text);
        let compact = compact_of(&plain);
        let latin = look_alike(&plain).map(|l| {
            let c = compact_of(&l);
            (l, c)
        });
        Key { plain, compact, latin }
    }
    fn latin_plain(&self) -> &str {
        self.latin.as_ref().map_or(&self.plain, |l| &l.0)
    }
    fn latin_compact(&self) -> &str {
        self.latin.as_ref().map_or(&self.compact, |l| &l.1)
    }
}

/// What was typed, folded once; ranks names against it.
#[derive(Clone, Debug, Default)]
pub struct Query {
    key: Key,
}

impl Query {
    pub fn new(typed: &str) -> Self {
        Query { key: Key::new(typed) }
    }

    /// Nothing to look for (also when only spaces, symbols or invisible characters were typed): everything matches.
    pub fn is_empty(&self) -> bool {
        self.key.compact.is_empty()
    }

    /// How well `name` matches, lower is better, `None` when it does not: 0 the same name, 1 its start, 2 the start of
    /// one of its words, 3 inside it, 4 inside it once separators are ignored; 5 to 9 the same on the look-alike forms.
    /// An empty query matches everything at 0.
    pub fn rank(&self, name: &str) -> Option<u8> {
        if self.is_empty() {
            return Some(0);
        }
        with_key(name, |k| self.rank_key(k))
    }

    /// The best rank among several names of one thing (a display name and a user name, a title and an author).
    pub fn rank_any<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Option<u8> {
        names.into_iter().filter_map(|n| self.rank(n)).min()
    }

    /// Whether `name` matches at all.
    pub fn matches(&self, name: &str) -> bool {
        self.rank(name).is_some()
    }

    /// Whether any of `names` matches.
    pub fn matches_any<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> bool {
        self.rank_any(names).is_some()
    }

    /// [`Query::rank`] on a key kept by the caller.
    pub fn rank_key(&self, k: &Key) -> Option<u8> {
        if self.is_empty() {
            return Some(0);
        }
        let q = &self.key;
        if let Some(r) = tier(&k.plain, &k.compact, &q.plain, &q.compact) {
            return Some(r);
        }
        if k.latin.is_none() && q.latin.is_none() {
            return None;
        }
        tier(k.latin_plain(), k.latin_compact(), q.latin_plain(), q.latin_compact()).map(|r| r + 5)
    }
}

/// Keeps `items` whose names match `query`, best first; items that match equally keep their order.
pub fn filter_ranked<T>(query: &Query, items: impl IntoIterator<Item = T>, names: impl Fn(&T) -> Vec<&str>) -> Vec<T> {
    let mut ranked: Vec<(u8, T)> = items.into_iter().filter_map(|t| query.rank_any(names(&t)).map(|r| (r, t))).collect();
    ranked.sort_by_key(|(r, _)| *r);
    ranked.into_iter().map(|(_, t)| t).collect()
}

fn tier(plain: &str, compact: &str, q_plain: &str, q_compact: &str) -> Option<u8> {
    if !q_plain.is_empty() {
        if plain == q_plain {
            return Some(0);
        }
        if plain.starts_with(q_plain) {
            return Some(1);
        }
        if plain.contains(q_plain) {
            let word_start = plain.match_indices(q_plain).any(|(i, _)| plain[..i].ends_with(' '));
            return Some(if word_start { 2 } else { 3 });
        }
    }
    (!q_compact.is_empty() && compact.contains(q_compact)).then_some(4)
}

const CACHE_MAX: usize = 1 << 16;

thread_local! {
    static KEYS: RefCell<HashMap<Box<str>, Rc<Key>>> = RefCell::new(HashMap::new());
}

/// The key of `name`, from this thread's cache (filled on first sight, emptied when it grows past 65,536 names).
fn with_key<R>(name: &str, f: impl FnOnce(&Key) -> R) -> R {
    let key = KEYS.with(|keys| {
        let mut keys = keys.borrow_mut();
        if let Some(k) = keys.get(name) {
            return k.clone();
        }
        if keys.len() >= CACHE_MAX {
            keys.clear();
        }
        let k = Rc::new(Key::new(name));
        keys.insert(name.into(), k.clone());
        k
    });
    f(&key)
}

/// The folded form of `text` (see the module's description, step 1).
pub fn fold(text: &str) -> String {
    let mut letters = String::with_capacity(text.len());
    for c in text.chars() {
        if let Some(l) = enclosed_letter(c) {
            letters.push(l);
            continue;
        }
        unicode_normalization::char::decompose_compatible(c, |d| {
            for l in d.to_lowercase() {
                if dropped(l) {
                    continue;
                }
                match latin_letter(l) {
                    Some(s) => letters.push_str(s),
                    None => letters.push(hiragana(l)),
                }
            }
        });
    }
    words(&letters)
}

/// Words of letters and digits separated by single spaces, letters standing alone in a row joined into one word.
fn words(letters: &str) -> String {
    let mut out = String::with_capacity(letters.len());
    let mut single_run = false;
    for word in letters.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let single = word.chars().nth(1).is_none();
        if !out.is_empty() && !(single && single_run) {
            out.push(' ');
        }
        out.push_str(word);
        single_run = single;
    }
    // "n e o n i x" joined, but "a club" stays two words: a single letter only joins the next one if that is single too.
    out
}

fn compact_of(plain: &str) -> String {
    plain.chars().filter(|c| *c != ' ').collect()
}

/// The look-alike reading of a folded name: each word made only of Latin letters, digits and letters that imitate
/// Latin ones (with at least one of those) is read as Latin; other words stay. `None` when nothing changes.
fn look_alike(plain: &str) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(plain.len());
    for (i, word) in plain.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let imitates = word.chars().any(|c| confusable(c).is_some()) && word.chars().all(|c| c.is_ascii_alphanumeric() || confusable(c).is_some());
        if imitates {
            changed = true;
            out.extend(word.chars().map(|c| confusable(c).unwrap_or(c)));
        } else {
            out.push_str(word);
        }
    }
    changed.then_some(out)
}

/// Characters that never count: decorating accents (the combining diacritical blocks; the kana voicing marks and other
/// scripts' vowel signs are letters there and stay), zero-width and direction characters, variation selectors, tags,
/// and the blank fillers used to write an invisible or padded name.
fn dropped(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F // accents
        | 0x00AD | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x206F | 0x2800 | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// Letters written as pictures of letters that NFKD keeps as they are: regional indicators (🇦), negative circled (🅐)
/// and negative squared (🅰) capitals.
fn enclosed_letter(c: char) -> Option<char> {
    let n = c as u32;
    let base = match n {
        0x1F1E6..=0x1F1FF => 0x1F1E6,
        0x1F150..=0x1F169 => 0x1F150,
        0x1F170..=0x1F189 => 0x1F170,
        _ => return None,
    };
    char::from_u32('a' as u32 + n - base)
}

/// Latin letters NFKD leaves alone, read as the plain letters they decorate (lower case already).
fn latin_letter(c: char) -> Option<&'static str> {
    Some(match c {
        // Small capitals.
        'ᴀ' => "a",
        'ʙ' => "b",
        'ᴄ' => "c",
        'ᴅ' => "d",
        'ᴇ' => "e",
        'ꜰ' => "f",
        'ɢ' => "g",
        'ʜ' => "h",
        'ɪ' => "i",
        'ᴊ' => "j",
        'ᴋ' => "k",
        'ʟ' => "l",
        'ᴍ' => "m",
        'ɴ' => "n",
        'ᴏ' => "o",
        'ᴘ' => "p",
        'ꞯ' => "q",
        'ʀ' => "r",
        'ꜱ' => "s",
        'ᴛ' => "t",
        'ᴜ' => "u",
        'ᴠ' => "v",
        'ᴡ' => "w",
        'ʏ' => "y",
        'ᴢ' => "z",
        // Stroked, hooked and IPA letters, ligatures.
        'ø' => "o",
        'đ' | 'ð' | 'ɗ' | 'ɖ' | '∂' => "d",
        'ł' | 'ƚ' | 'ɫ' => "l",
        'ħ' => "h",
        'ŧ' | 'ƭ' | 'ʈ' => "t",
        'ƀ' | 'ɓ' => "b",
        'ɨ' | 'ı' => "i",
        'ȷ' | 'ɉ' => "j",
        'ɑ' => "a",
        'ɡ' | 'ɠ' => "g",
        'ŋ' | 'ɲ' | 'ɳ' | 'ƞ' => "n",
        'ɱ' => "m",
        'ɾ' | 'ɍ' | 'ɽ' => "r",
        'ƈ' | 'ȼ' => "c",
        'ƙ' => "k",
        'ʋ' => "v",
        'ɏ' | 'ƴ' => "y",
        'ȥ' | 'ƶ' | 'ɀ' => "z",
        'ƒ' => "f",
        'ʂ' => "s",
        'æ' => "ae",
        'œ' => "oe",
        'þ' => "th",
        'ß' => "ss",
        _ => return None,
    })
}

/// Katakana read as hiragana, so either kana finds the other.
fn hiragana(c: char) -> char {
    match c as u32 {
        n @ 0x30A1..=0x30F6 => char::from_u32(n - 0x60).unwrap_or(c),
        _ => c,
    }
}

/// Cyrillic and Greek letters as decorated names use them in place of Latin ones (lower case, after folding).
fn confusable(c: char) -> Option<char> {
    Some(match c {
        // Cyrillic.
        'а' => 'a',
        'в' | 'ь' | 'ъ' => 'b',
        'с' => 'c',
        'ԁ' => 'd',
        'е' | 'є' => 'e',
        'ғ' => 'f',
        'һ' | 'н' => 'h',
        'і' => 'i',
        'ј' => 'j',
        'к' | 'ҡ' | 'ӄ' => 'k',
        'ӏ' => 'l',
        'м' => 'm',
        'и' | 'п' => 'n',
        'о' => 'o',
        'р' => 'p',
        'ԛ' => 'q',
        'г' | 'я' => 'r',
        'ѕ' => 's',
        'т' => 't',
        'ц' => 'u',
        'ѵ' => 'v',
        'ш' | 'щ' | 'ԝ' | 'ѡ' => 'w',
        'х' => 'x',
        'у' | 'ү' => 'y',
        // Greek.
        'α' => 'a',
        'β' => 'b',
        'δ' => 'd',
        'ε' => 'e',
        'η' | 'π' => 'n',
        'ι' => 'i',
        'κ' => 'k',
        'μ' | 'υ' => 'u',
        'ν' => 'v',
        'ο' | 'σ' => 'o',
        'ρ' => 'p',
        'τ' => 't',
        'χ' => 'x',
        'γ' => 'y',
        'ω' => 'w',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-style names: each is found by its plain form and by its own form.
    const NAMES: &[(&str, &str)] = &[
        ("ｎｅｏｎｉｘ", "neonix"),                 // full-width (Enzo's friend, as VRChat sends it)
        ("ｎ ｅ ｏ ｎ ｉ ｘ", "neonix"),            // full-width, spaced
        ("n e o n i x", "neonix"),                 // spaced
        ("n.e.o.n.i.x", "neonix"),                 // dotted
        ("neo/nix", "neonix"),                     // a separator inside
        ("neo_nix", "neonix"),
        ("𝐍𝐞𝐨𝐧𝐢𝐱", "neonix"),                     // mathematical bold
        ("𝓝𝓮𝓸𝓷𝓲𝔁", "neonix"),                     // mathematical bold script
        ("𝕟𝕖𝕠𝕟𝕚𝕩", "neonix"),                     // double-struck
        ("ⓝⓔⓞⓝⓘⓧ", "neonix"),                     // circled
        ("🅽🅴🅾🅽🅸🆇", "neonix"),                     // negative squared
        ("🇳🇪🇴🇳🇮🇽", "neonix"),                     // regional indicators
        ("ɴᴇᴏɴɪx", "neonix"),                      // small capitals
        ("ηєσηιχ", "neonix"),                      // Greek and Cyrillic dressed as Latin
        ("Nеonix", "neonix"),                      // one Cyrillic е hidden in a Latin name
        ("Né̂ønïx", "neonix"),                      // accents and a stroke
        ("ne\u{200D}on\u{200B}ix", "neonix"),      // zero-width joiner and space
        ("neonix\u{FE0F}", "neonix"),              // variation selector
        ("✧･ﾟ: *✧ neonix ✧*:･ﾟ✧", "neonix"),       // symbols around it
        ("🌸 Neonix 🌸", "neonix"),                 // emoji around it
        ("ᴺᵉᵒⁿⁱˣ", "neonix"),                      // superscript
        ("NEONIX", "neonix"),
        ("Straße", "strasse"),
        ("Zoë", "zoe"),
        ("ネオニクス", "ねおにくす"),               // katakana, found by hiragana
        ("ﾈｵﾆｸｽ", "ネオニクス"),                    // half-width katakana, found by full-width
        ("ねおにっくす★", "ねおにっくす"),
        ("Neo☆ネオ", "neo"),                        // kana beside Latin
        ("Neo☆ネオ", "ネオ"),
        ("Саша", "саша"),                           // Cyrillic, by its own letters
        ("Саша", "Саша"),
        ("Σοφία", "σοφια"),                         // Greek with accents, by its own letters
        ("東京タワー", "東京"),
        ("İlknur", "ilknur"),
        ("ǅemal", "dzemal"),
    ];

    #[test]
    fn real_names_are_found_by_their_plain_form_and_by_themselves() {
        for (name, plain) in NAMES {
            assert!(Query::new(plain).matches(name), "{name:?} not found by {plain:?} (key {:?})", Key::new(name));
            assert!(Query::new(name).matches(name), "{name:?} not found by itself");
            assert_eq!(Query::new(name).rank(name), Some(0), "{name:?} by itself is an exact match");
        }
    }

    #[test]
    fn enzos_friend_is_an_exact_match_for_neonix() {
        // His friend's display name, code point by code point, as his VRCX cache holds it.
        let name = "\u{FF4E}\u{FF45}\u{FF4F}\u{FF4E}\u{FF49}\u{FF58}";
        assert_eq!(fold(name), "neonix");
        assert_eq!(Query::new("neonix").rank(name), Some(0));
        assert_eq!(Query::new("neon").rank(name), Some(1));
        assert_eq!(Query::new("NeoNix").rank(name), Some(0));
        assert_eq!(Query::new("neo nix").rank(name), Some(4), "separators typed do not matter either");
    }

    #[test]
    fn different_names_do_not_match() {
        let apart: &[(&str, &str)] = &[
            ("neonix", "Neo"),
            ("neonix", "nomix"),
            ("neonix", "ｎｅｏｍｉｘ"),
            ("neonix", "Nyx"),
            ("Саша", "Сара"),
            ("саша", "Sasha"),
            ("ねこ", "ネオ"),
            ("東京", "京都"),
            ("zoe", "Zed"),
            ("ac", "abc club"),
            ("x", "neo"),
        ];
        for (typed, name) in apart {
            assert!(!Query::new(typed).matches(name), "{typed:?} must not find {name:?}");
        }
    }

    #[test]
    fn a_russian_name_is_not_read_as_latin_when_it_has_real_cyrillic_letters() {
        // Жанна has ж, which imitates no Latin letter: the word is Cyrillic and is not given a Latin reading.
        assert_eq!(Key::new("Жанна").latin, None, "ж has no Latin look-alike: the word stays Cyrillic");
        assert!(!Query::new("ah").matches("Жанна"));
        assert!(Query::new("жан").matches("Жанна"));
        assert!(Query::new("ЖАН").matches("Жанна"));
    }

    #[test]
    fn exact_folded_matches_rank_first() {
        let names = ["Neonix Fan Club", "xneonix", "ｎｅｏｎｉｘ", "the neonix", "n e o.n i x", "ηєσηιχ", "neo/nix"];
        let q = Query::new("neonix");
        let ranked = filter_ranked(&q, names.iter(), |n| vec![**n]);
        assert_eq!(ranked[..2], [&"ｎｅｏｎｉｘ", &"n e o.n i x"][..], "{ranked:?}");
        assert_eq!(ranked.len(), names.len());
        let ranks: Vec<_> = names.iter().map(|n| q.rank(n)).collect();
        assert_eq!(ranks, [Some(1), Some(3), Some(0), Some(2), Some(0), Some(5), Some(4)]);
    }

    #[test]
    fn an_empty_or_invisible_query_matches_everything() {
        for typed in ["", "   ", "\u{200B}", "★☆", "\u{3164}"] {
            let q = Query::new(typed);
            assert!(q.is_empty(), "{typed:?}");
            assert!(q.matches("anything"));
        }
    }

    #[test]
    fn a_display_name_or_a_user_name_finds_the_person() {
        let q = Query::new("neonix");
        assert_eq!(q.rank_any(["ｎｅｏｎｉｘ", "neo/nix"]), Some(0));
        assert_eq!(q.rank_any(["Someone", "neo/nix"]), Some(4));
        assert_eq!(q.rank_any(["Someone", "else"]), None);
    }

    /// The cost on a list of 5,000 friends: the first keystroke folds every name, later ones read the cache.
    /// Run with `--ignored --nocapture` to see the numbers; the bound is loose because tests run unoptimised.
    #[test]
    #[ignore]
    fn five_thousand_friends_cost() {
        let names: Vec<String> = (0..5000)
            .map(|i| match i % 5 {
                0 => format!("ｆｒｉｅｎｄ{i}"),
                1 => format!("✧ 𝓕𝓻𝓲𝓮𝓷𝓭 {i} ✧"),
                2 => format!("f r i e n d {i}"),
                3 => format!("Друг{i}"),
                _ => format!("フレンド{i}ちゃん"),
            })
            .collect();
        let time = |typed: &str| {
            let t = std::time::Instant::now();
            let q = Query::new(typed);
            let n = names.iter().filter(|n| q.matches(n)).count();
            (t.elapsed(), n)
        };
        let (first, _) = time("f");
        let mut later = Vec::new();
        for typed in ["fr", "fri", "frie", "frien", "friend", "friend4", "friend49"] {
            later.push(time(typed));
        }
        let worst = later.iter().map(|(d, _)| *d).max().unwrap();
        println!("5,000 names: first keystroke (folds all) {first:?}, later keystrokes at most {worst:?}; found {:?}", later.iter().map(|l| l.1).collect::<Vec<_>>());
        assert_eq!(later.last().unwrap().1, 66, "friend490..499 and friend4900..4999 of the three Latin kinds (i % 5 < 3)");
    }
}
