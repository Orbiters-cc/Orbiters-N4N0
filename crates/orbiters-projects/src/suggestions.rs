//! The repositories Orbiters suggests, as the Package repositories dialog lists them: the two lists of the server's
//! catalog as one (a repository in both shows once), and the search over them.
//!
//! About 330 repositories with their packages are searched on every keystroke and the dialog draws at 60 frames a
//! second, so the folded text is made once when the catalog arrives and the results are kept until the query changes:
//! a frame only compares the query with the one it was last given.
//!
//! A query is words, all of which have to be found, each anywhere: in the repository's name, its owner, its description
//! or one of its packages (id or display name). Case does not count, and neither do the differences a Japanese keyboard
//! makes without the person choosing them (full-width Latin, hiragana against katakana).

use crate::network::{Catalog, Listing};

/// One repository, its text folded for searching.
#[derive(Clone, Debug)]
struct Card {
    listing: Listing,
    /// Its name and owner.
    title: String,
    /// What the card says under them: its description, or the address shown when it has none.
    text: String,
    packages: Vec<FoldedPackage>,
}

#[derive(Clone, Debug)]
struct FoldedPackage {
    id: String,
    display: String,
}

/// A repository the query found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Which repository: for [`Suggestions::listing`].
    pub card: usize,
    /// The packages it was found through, as they are to be named on the card, when nothing else on the card explains
    /// why it is there.
    pub because: Option<String>,
}

/// The suggestions, and what the current query finds among them.
#[derive(Clone, Debug, Default)]
pub struct Suggestions {
    cards: Vec<Card>,
    /// The query as last given, so that a frame with the same one costs a comparison.
    query: String,
    /// Its words.
    words: Vec<String>,
    hits: Vec<Hit>,
}

impl Suggestions {
    pub fn new(catalog: &Catalog) -> Self {
        let mut cards: Vec<Listing> = Vec::with_capacity(catalog.known.len() + catalog.public.len());
        let mut seen = std::collections::HashMap::new();
        for l in catalog.known.iter().chain(&catalog.public) {
            let key = l.url.trim_end_matches('/').to_string();
            match seen.get(&key) {
                // In both lists: what one knows and the other does not.
                Some(&i) => merge(&mut cards[i], l),
                None => {
                    seen.insert(key, cards.len());
                    cards.push(l.clone());
                }
            }
        }
        let cards: Vec<Card> = cards.into_iter().map(Card::new).collect();
        let hits = (0..cards.len()).map(|card| Hit { card, because: None }).collect();
        Self { cards, query: String::new(), words: vec![], hits }
    }

    /// How many repositories there are, whatever the query.
    pub fn total(&self) -> usize {
        self.cards.len()
    }

    /// What the query finds, the closest first ([`Card::find`]), or every repository in the server's order without one.
    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }

    pub fn listing(&self, hit: &Hit) -> &Listing {
        &self.cards[hit.card].listing
    }

    /// Whether the query asks for anything.
    pub fn searching(&self) -> bool {
        !self.words.is_empty()
    }

    /// Searches for `query`, unless it is the one already searched. True when what it finds may have changed (another
    /// space is not another query), for the list to go back to its top.
    pub fn set_query(&mut self, query: &str) -> bool {
        if self.query == query {
            return false;
        }
        self.query.clear();
        self.query.push_str(query);
        let words: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
        if words == self.words {
            return false;
        }
        self.words = words;
        self.hits = if self.words.is_empty() {
            (0..self.cards.len()).map(|card| Hit { card, because: None }).collect()
        } else {
            let mut found: Vec<(u8, Hit)> =
                self.cards.iter().enumerate().filter_map(|(card, c)| c.find(&self.words).map(|(rank, because)| (rank, Hit { card, because }))).collect();
            // Stable: the server's order within a rank.
            found.sort_by_key(|(rank, _)| *rank);
            found.into_iter().map(|(_, hit)| hit).collect()
        };
        true
    }
}

/// What `other` (the same repository in the other list) knows that `into` does not.
fn merge(into: &mut Listing, other: &Listing) {
    if into.description.is_empty() {
        into.description.clone_from(&other.description);
    }
    if into.owner.is_empty() {
        into.owner.clone_from(&other.owner);
    }
    if into.packages.is_empty() {
        into.packages.clone_from(&other.packages);
    }
    into.package_count = into.package_count.or(other.package_count);
    if into.logo.is_none() {
        into.logo.clone_from(&other.logo);
    }
    if into.logos.is_empty() {
        into.logos.clone_from(&other.logos);
    }
}

impl Card {
    fn new(mut listing: Listing) -> Self {
        // A description is shown on two lines at most: whatever line breaks it came with go.
        listing.description = listing.description.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = fold(&format!("{}\n{}", listing.name, listing.owner));
        let text = fold(if listing.description.is_empty() { &listing.url } else { &listing.description });
        let packages = listing.packages.iter().map(|p| FoldedPackage { id: fold(&p.name), display: fold(&p.display_name) }).collect();
        Self { listing, title, text, packages }
    }

    /// Whether every word is somewhere in this repository. A rank (the repository named so first, then one holding a
    /// package named so, then one only mentioning it), and the packages to name for the words the card would not explain
    /// otherwise.
    fn find(&self, words: &[String]) -> Option<(u8, Option<String>)> {
        let mut rank = 0;
        let mut unexplained: Vec<&str> = vec![];
        for word in words {
            let in_title = self.title.contains(word.as_str());
            let in_text = self.text.contains(word.as_str());
            let in_package = self.packages.iter().any(|p| p.matches(word));
            if !(in_title || in_text || in_package) {
                return None;
            }
            if !in_title {
                rank = rank.max(if in_package { 1 } else { 2 });
            }
            if !in_title && !in_text {
                unexplained.push(word);
            }
        }
        Some((rank, self.because(&unexplained)))
    }

    /// The packages to name for `words`: the one with the most of them, then so on for the rest.
    fn because(&self, words: &[&str]) -> Option<String> {
        let mut left: Vec<&str> = words.to_vec();
        let mut names: Vec<&str> = vec![];
        while !left.is_empty() {
            let best = self.packages.iter().enumerate().max_by_key(|(i, p)| (left.iter().filter(|w| p.matches(w)).count(), std::cmp::Reverse(*i)));
            let Some((i, package)) = best else { break };
            if !left.iter().any(|w| package.matches(w)) {
                break;
            }
            // The name the words are in: its display name, or its id if they were found there (a typed id).
            let listed = &self.listing.packages[i];
            let by_display = !listed.display_name.is_empty() && left.iter().any(|w| package.display.contains(w));
            names.push(if by_display { &listed.display_name } else { &listed.name });
            left.retain(|w| !package.matches(w));
        }
        (!names.is_empty()).then(|| names.join(", "))
    }
}

impl FoldedPackage {
    fn matches(&self, word: &str) -> bool {
        self.id.contains(word) || self.display.contains(word)
    }
}

/// `s` as searches compare it: lower case, full-width Latin and the ideographic space as their plain forms (a Japanese
/// keyboard types them unasked), katakana as hiragana (the same sounds: アバター finds あばたー).
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
            c => c,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ListingPackage;

    fn listing(name: &str, owner: &str, description: &str, packages: &[(&str, &str)]) -> Listing {
        Listing {
            name: name.into(),
            url: format!("https://{}.example.com/index.json", name.to_lowercase().replace(' ', "-")),
            owner: owner.into(),
            description: description.into(),
            packages: packages.iter().map(|(id, display)| ListingPackage { name: (*id).into(), display_name: (*display).into() }).collect(),
            ..Default::default()
        }
    }

    fn catalog() -> Suggestions {
        Suggestions::new(&Catalog {
            known: vec![
                listing("bd_", "bdunderscore", "Non-destructive avatar tools", &[("nadena.dev.modular-avatar", "Modular Avatar"), ("nadena.dev.ndmf", "NDMF")]),
                listing("Poiyomi", "Poiyomi", "Toon and Pro shaders", &[("com.poiyomi.toon", "Poiyomi Toon")]),
                listing("Modular Tools", "someone", "Handy editor helpers", &[]),
                listing("すずりょぐのアバターツール", "suzuryg", "表情メニューを簡単に作れるツール", &[("jp.suzuryg.face-emo", "FaceEmo")]),
                listing("Mentions", "other", "Works well with modular setups", &[]),
            ],
            public: vec![],
            errors: vec![],
        })
    }

    fn names(s: &Suggestions) -> Vec<&str> {
        s.hits().iter().map(|h| s.listing(h).name.as_str()).collect()
    }

    #[test]
    fn without_a_query_every_repository_shows_in_the_servers_order() {
        let s = catalog();
        assert_eq!(names(&s), ["bd_", "Poiyomi", "Modular Tools", "すずりょぐのアバターツール", "Mentions"]);
        assert!(s.hits().iter().all(|h| h.because.is_none()));
        assert!(!s.searching());
    }

    #[test]
    fn a_package_finds_its_repository_and_is_named_on_the_card() {
        let mut s = catalog();
        s.set_query("modular avatar");
        assert_eq!(names(&s), ["bd_"], "avatar is in its description, modular only in a package");
        assert_eq!(s.hits()[0].because.as_deref(), Some("Modular Avatar"));
        // Found by the package's id, it is named by its id: what the person typed.
        s.set_query("nadena.dev.ndmf");
        assert_eq!(s.hits()[0].because.as_deref(), Some("nadena.dev.ndmf"));
        // By its display name: the name people know.
        s.set_query("ndmf");
        assert_eq!(s.hits()[0].because.as_deref(), Some("NDMF"));
    }

    #[test]
    fn a_card_that_explains_itself_names_no_package() {
        let mut s = catalog();
        s.set_query("poiyomi");
        assert_eq!(names(&s), ["Poiyomi"]);
        assert_eq!(s.hits()[0].because, None, "its name says so, though a package is called that too");
    }

    #[test]
    fn the_owner_finds_a_repository() {
        let mut s = catalog();
        s.set_query("BDUNDERSCORE");
        assert_eq!(names(&s), ["bd_"]);
        assert_eq!(s.hits()[0].because, None);
    }

    #[test]
    fn the_description_finds_a_repository() {
        let mut s = catalog();
        s.set_query("shaders");
        assert_eq!(names(&s), ["Poiyomi"]);
    }

    #[test]
    fn every_word_has_to_be_found_each_anywhere() {
        let mut s = catalog();
        // Its owner, its package, its description.
        s.set_query("bdunderscore  ndmf tools");
        assert_eq!(names(&s), ["bd_"]);
        assert_eq!(s.hits()[0].because.as_deref(), Some("NDMF"));
        s.set_query("poiyomi tools");
        assert!(s.hits().is_empty(), "each word is somewhere, not both in one repository");
    }

    #[test]
    fn words_in_two_packages_name_both() {
        let mut s = Suggestions::new(&Catalog {
            known: vec![listing("Shelf", "", "", &[("a.first", "First Thing"), ("a.second", "Second Thing")])],
            ..Default::default()
        });
        s.set_query("first second");
        assert_eq!(s.hits()[0].because.as_deref(), Some("First Thing, Second Thing"));
    }

    #[test]
    fn japanese_is_found_in_every_field() {
        let mut s = catalog();
        s.set_query("アバター");
        assert_eq!(names(&s), ["すずりょぐのアバターツール"]);
        s.set_query("表情");
        assert_eq!(names(&s), ["すずりょぐのアバターツール"]);
        // A package in Japanese.
        let mut t = Suggestions::new(&Catalog { known: vec![listing("Shelf", "", "", &[("jp.a.cloth", "衣装ツール")])], ..Default::default() });
        t.set_query("衣装");
        assert_eq!(t.hits()[0].because.as_deref(), Some("衣装ツール"));
    }

    #[test]
    fn a_japanese_keyboards_forms_find_the_same_thing() {
        let mut s = catalog();
        // Hiragana for katakana, an ideographic space between the words, full-width Latin.
        s.set_query("あばたー\u{3000}つーる");
        assert_eq!(names(&s), ["すずりょぐのアバターツール"]);
        s.set_query("ＰＯＩＹＯＭＩ");
        assert_eq!(names(&s), ["Poiyomi"]);
    }

    #[test]
    fn nothing_found_is_an_empty_list_and_clearing_brings_all_back() {
        let mut s = catalog();
        assert!(s.set_query("zzzz"));
        assert!(s.hits().is_empty());
        assert_eq!(s.total(), 5);
        assert!(s.searching());
        assert!(s.set_query(""));
        assert_eq!(s.hits().len(), 5);
    }

    #[test]
    fn the_same_words_are_not_searched_again() {
        let mut s = catalog();
        assert!(s.set_query("modular"));
        assert!(!s.set_query("modular"), "a frame with the same query");
        assert!(!s.set_query("modular "), "a space typed before the next word");
        assert!(s.set_query("modular t"));
    }

    #[test]
    fn repositories_named_so_come_before_those_holding_or_mentioning_it() {
        let mut s = catalog();
        s.set_query("modular");
        assert_eq!(names(&s), ["Modular Tools", "bd_", "Mentions"]);
    }

    #[test]
    fn a_repository_in_both_lists_shows_once_with_what_each_knows() {
        let mut bare = listing("Orbiters", "", "", &[]);
        bare.url = "https://orbiters.cc/vpm/orbiters/index.json/".into();
        let mut full = listing("Orbiters", "Orbiters", "My Avatar and Toolkit", &[("orbiters.toolkit", "Toolkit")]);
        full.url = "https://orbiters.cc/vpm/orbiters/index.json".into();
        full.logo = Some("/logo.png".into());
        let s = Suggestions::new(&Catalog { known: vec![bare], public: vec![full], errors: vec![] });
        assert_eq!(s.total(), 1);
        let l = s.listing(&s.hits()[0]);
        assert_eq!(
            (l.owner.as_str(), l.description.as_str(), l.packages.len(), l.logo.as_deref()),
            ("Orbiters", "My Avatar and Toolkit", 1, Some("/logo.png"))
        );
    }

    /// Until the server sends more, a card shows its name and address: what it shows can be searched.
    #[test]
    fn a_card_without_a_description_is_found_by_the_address_it_shows() {
        let mut s = Suggestions::new(&Catalog { known: vec![listing("Bare", "", "", &[]), listing("Said", "", "Has its words", &[])], ..Default::default() });
        s.set_query("bare.example");
        assert_eq!(names(&s), ["Bare"]);
        s.set_query("said.example");
        assert!(s.hits().is_empty(), "its address is not on its card: every github.io repository would match github");
    }

    #[test]
    fn a_description_loses_its_line_breaks() {
        let s = Suggestions::new(&Catalog { known: vec![listing("A", "", "two\n  lines", &[])], ..Default::default() });
        assert_eq!(s.listing(&s.hits()[0]).description, "two lines");
    }

    /// Search stays instant: 330 repositories of 30 packages each, far more text than the server has. Debug builds of this
    /// crate are unoptimised, so a keystroke here costs several times what it does in the app, and the limit leaves room
    /// for a computer busy with other builds.
    #[test]
    fn a_keystroke_over_the_whole_catalog_stays_instant() {
        let packages: Vec<(String, String)> = (0..30).map(|i| (format!("com.example.package-number-{i}"), format!("Package Number {i}"))).collect();
        let packages: Vec<(&str, &str)> = packages.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let known: Vec<Listing> =
            (0..330).map(|i| listing(&format!("Repository {i}"), "Some owner", "A description of about this length", &packages)).collect();
        let mut s = Suggestions::new(&Catalog { known, ..Default::default() });
        let mut worst = std::time::Duration::ZERO;
        for q in ["p", "pa", "pac", "package nu", "package number 29", "zzz", ""] {
            let t = std::time::Instant::now();
            s.set_query(q);
            worst = worst.max(t.elapsed());
        }
        eprintln!("worst keystroke over 330 repositories of 30 packages: {worst:?}");
        assert!(worst < std::time::Duration::from_millis(50), "{worst:?}");
    }
}
