//! The texts N4N0's menu pages draw in their unchanging part (titles, buttons, chips, captions: the page pictures the
//! avatar's screen shows, and the same places on the SteamVR overlay), by key, English being the base. A strings file
//! (contracts `Docs/orb.md`, Menu themes and translations) gives them in another language:
//! `{ "schema": 1, "language": "fr", "strings": { "home.fastWalk": "Marche rapide" } }`; a key it leaves out stays
//! English. The changing parts (what N4N0 heard, a tile's note, names, results) are not here: the running app writes
//! them into the screen's memory ([`super::layout::text`]).
//!
//! The base translations are files beside this crate (`menu-strings/en.json`, `fr`, `es`, `pt`, `ja`), the English one
//! with each key's `max` (`orbiters-desktop --orb-menu-strings` writes it: the characters that fit the key's place).

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

/// The strings files' schema this build reads and writes.
pub const SCHEMA: u64 = 1;

macro_rules! texts {
    ($($(#[$doc:meta])* $name:ident = $key:literal => $en:literal,)*) => {
        /// A text of the menu's pages (not one of their changing parts), by its key.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Text {
            $($(#[$doc])* $name,)*
        }

        impl Text {
            /// Every text, in the order of the strings files.
            pub const ALL: &'static [Text] = &[$(Text::$name,)*];

            /// Its key in strings files (`home.fastWalk`): never changes once released.
            pub const fn key(self) -> &'static str {
                match self {
                    $(Text::$name => $key,)*
                }
            }

            /// Its English text, the base.
            pub const fn en(self) -> &'static str {
                match self {
                    $(Text::$name => $en,)*
                }
            }
        }
    };
}

texts! {
    /// The back of the page: what the screen shows from behind.
    BackTitle = "back.title" => "N4N0 menu",
    BackHint = "back.hint" => "look at the other side",
    /// Home's tiles (every Home: four, six, seven and ten places).
    HomeFastWalk = "home.fastWalk" => "Fast walk",
    HomeYouTube = "home.youtube" => "YouTube",
    HomeCall = "home.call" => "Call",
    HomePhoto = "home.photo" => "3D picture",
    HomeMusic = "home.music" => "Music",
    HomeLight = "home.light" => "Light",
    HomeStates = "home.states" => "States",
    HomeTracking = "home.tracking" => "Tracking",
    HomeSuggest = "home.suggest" => "Suggest",
    /// Search: its button beside the field and the keyboard's green key.
    Search = "search.search" => "Search",
    /// The keyboard's keys that are not characters.
    KeyboardSymbols = "keyboard.symbols" => "?123",
    KeyboardLetters = "keyboard.letters" => "ABC",
    KeyboardSpace = "keyboard.space" => "space",
    /// The keyboard page's Done (its button and the keyboard's green key).
    TypeDone = "type.done" => "Done",
    /// A result's (and the music's video's) Copy link, and its Drop once copied.
    LinkCopy = "link.copy" => "Copy link",
    LinkDrop = "link.drop" => "Drop",
    /// Calls: a friend's Call, a ringing call's Answer and Decline, a call's Hang up.
    CallsCall = "calls.call" => "Call",
    CallsAnswer = "calls.answer" => "Answer",
    CallsDecline = "calls.decline" => "Decline",
    CallsHangUp = "calls.hangUp" => "Hang up",
    /// The 3D picture's page.
    PhotoTitle = "photo.title" => "3D picture",
    PhotoTimer = "photo.timer" => "Timer",
    PhotoFlight = "photo.flight" => "Camera flight",
    PhotoTake = "photo.take" => "Take 3D picture",
    /// The timer's choices ([`super::TIMERS`] in order).
    PhotoTimerOff = "photo.timerOff" => "Off",
    PhotoTimer3 = "photo.timer3" => "3 s",
    PhotoTimer5 = "photo.timer5" => "5 s",
    PhotoTimer10 = "photo.timer10" => "10 s",
    /// The camera flights ([`super::PhotoLength::ALL`] in order).
    PhotoFlight1 = "photo.flight1" => "1 s",
    PhotoFlight4 = "photo.flight4" => "4 s",
    PhotoFlight8 = "photo.flight8" => "8 s",
    /// The music page.
    MusicTitle = "music.title" => "Music",
    MusicWatch = "music.watch" => "Watch",
    MusicSave = "music.save" => "Save",
    /// The light page: its title, its switch, its captions.
    LightTitle = "light.title" => "Light",
    LightSwitch = "light.switch" => "Light",
    LightColour = "light.colour" => "Colour",
    LightBrightness = "light.brightness" => "Brightness",
    /// The light's colours (contracts `orb::LIGHT_COLORS` in order).
    LightWhite = "light.white" => "White",
    LightWarm = "light.warm" => "Warm",
    LightSunset = "light.sunset" => "Sunset",
    LightPink = "light.pink" => "Pink",
    LightViolet = "light.violet" => "Violet",
    LightBlue = "light.blue" => "Blue",
    LightCyan = "light.cyan" => "Cyan",
    LightGreen = "light.green" => "Green",
    /// Its brightness choices (contracts `orb::LIGHT_LEVELS` in order).
    LightSoft = "light.soft" => "Soft",
    LightMedium = "light.medium" => "Medium",
    LightBright = "light.bright" => "Bright",
    /// The States page.
    StatesTitle = "states.title" => "States",
    StatesNew = "states.new" => "New state",
    StatesApply = "states.apply" => "Apply",
    StatesDefault = "states.default" => "Default",
    /// A dropped link card.
    CardGrabToCopy = "card.grabToCopy" => "Grab to copy link",
    CardAnyoneCanTake = "card.anyoneCanTake" => "Anyone around with N4N0 can take it",
    /// Take back, on the results and the music page while the owner's card is out (from screen version 9).
    CardTakeBack = "card.takeBack" => "Take back card",
    /// New state.
    NewStateTitle = "newState.title" => "New state",
    NewStateName = "newState.name" => "Name",
    NewStateOptions = "newState.options" => "Options",
    NewStateSize = "newState.size" => "Size",
    NewStatePose = "newState.pose" => "Pose",
    NewStateDefault = "newState.default" => "Default",
    NewStateSave = "newState.save" => "Save state",
    /// Tracking: its title, its switch, the kinds of devices ([`super::TRACKING_KINDS`]) and the strengths
    /// ([`super::SMOOTHING_LABELS`]).
    TrackingTitle = "tracking.title" => "Tracking",
    TrackingSmoothing = "tracking.smoothing" => "Smoothing",
    TrackingHeadset = "tracking.headset" => "Headset",
    TrackingControllers = "tracking.controllers" => "Controllers",
    TrackingTrackers = "tracking.trackers" => "Trackers",
    TrackingOff = "tracking.off" => "Off",
    TrackingLow = "tracking.low" => "Low",
    TrackingMedium = "tracking.medium" => "Medium",
    TrackingHigh = "tracking.high" => "High",
    /// Suggest features.
    SuggestTitle = "suggest.title" => "Suggest a feature",
    SuggestType = "suggest.type" => "Type",
    SuggestTalk = "suggest.talk" => "Talk",
    SuggestSend = "suggest.send" => "Send",
    SuggestHint = "suggest.hint" => "What should N4N0 do next? The Orbiters team reads every idea.",
    /// The simple Home's large tiles (from screen version 10): 3D capture and More.
    HomeCapture = "home.capture" => "3D capture",
    HomeMore = "home.more" => "More",
    /// More: its title and its cards (light and suggest keep Home's).
    MoreTitle = "more.title" => "More",
    MoreInstances = "more.instances" => "Previous instances",
    MoreTracking = "more.tracking" => "Smooth tracking",
    /// Previous instances: its title and each row's Invite me.
    InstancesTitle = "instances.title" => "Instances",
    InstancesInvite = "instances.invite" => "Invite me",
    /// Add Orbiters as a friend: its title, why and how, its buttons.
    FriendTitle = "friend.title" => "Add Orbiters as a friend",
    FriendWhy = "friend.why" => "VRChat lets Orbiters invite only its friends.",
    FriendHow = "friend.how" => "In VRChat: Social, search the name, Add friend",
    FriendCopy = "friend.copy" => "Copy name",
    FriendOpen = "friend.open" => "Open link",
    FriendRetry = "friend.retry" => "Try again",
    /// More's QR code scanner (from screen version 11), and its page: its title, the caption over the last link, its
    /// buttons.
    MoreQr = "more.qr" => "QR code scanner",
    QrTitle = "qr.title" => "QR code scanner",
    QrLast = "qr.last" => "Last link opened",
    QrCopy = "qr.copy" => "Copy link",
    QrOpen = "qr.open" => "Open again",
    QrAgain = "qr.again" => "Scan again",
    /// Sharing 3D captures (from screen version 12): the 3D picture page's History and its last capture's caption, the
    /// shares (there and on History's rows), History's title, Share to …'s title and buttons, the explicit warning's
    /// title and buttons.
    HistoryOpen = "history.open" => "History",
    HistoryTitle = "history.title" => "3D captures",
    SharePublic = "share.public" => "Share publicly",
    ShareTo = "share.to" => "Share to …",
    ShareLast = "share.last" => "Last capture",
    FriendsTitle = "friends.title" => "Share with friends",
    FriendsFind = "friends.find" => "Find a friend",
    FriendsSend = "friends.send" => "Send",
    ExplicitTitle = "explicit.title" => "Share publicly?",
    ExplicitAnyway = "explicit.anyway" => "Share anyway",
    ExplicitCancel = "explicit.cancel" => "Cancel",
    /// The retro look's own words: the plate on the frame's top line (the screen's version follows it), the memory plate
    /// at the bottom right, the caption under a big icon, a picture's place before it came.
    RetroSystem = "retro.system" => "N4N0·OS",
    RetroMemory = "retro.memory" => "MEM 0744 · SYS OK",
    RetroModel = "retro.model" => "MODEL // 3D",
    RetroNoSignal = "retro.noSignal" => "NO SIGNAL",
}

/// The 3D picture's timer choices as its page writes them ([`super::TIMERS`] in order).
pub const TIMERS: [Text; 4] = [Text::PhotoTimerOff, Text::PhotoTimer3, Text::PhotoTimer5, Text::PhotoTimer10];
/// Its camera flights ([`super::PhotoLength::ALL`] in order).
pub const FLIGHTS: [Text; 3] = [Text::PhotoFlight1, Text::PhotoFlight4, Text::PhotoFlight8];
/// The light's colours (contracts `orb::LIGHT_COLORS` in order).
pub const LIGHT_COLORS: [Text; 8] =
    [Text::LightWhite, Text::LightWarm, Text::LightSunset, Text::LightPink, Text::LightViolet, Text::LightBlue, Text::LightCyan, Text::LightGreen];
/// Its brightness choices (contracts `orb::LIGHT_LEVELS` in order).
pub const LIGHT_LEVELS: [Text; 3] = [Text::LightSoft, Text::LightMedium, Text::LightBright];
/// The kinds of devices the Tracking page smooths ([`super::TRACKING_KINDS`]).
pub const TRACKING_KINDS: [Text; 3] = [Text::TrackingHeadset, Text::TrackingControllers, Text::TrackingTrackers];
/// The strengths it offers ([`super::SMOOTHING_LABELS`]).
pub const SMOOTHING: [Text; 4] = [Text::TrackingOff, Text::TrackingLow, Text::TrackingMedium, Text::TrackingHigh];

impl Text {
    /// The text with this key.
    pub fn by_key(key: &str) -> Option<Text> {
        Text::ALL.iter().copied().find(|t| t.key() == key)
    }
}

/// The menu's texts in one language: each key's text, English where it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strings {
    language: String,
    texts: Vec<Option<String>>,
}

impl Strings {
    /// English, the base: what the menu draws unless told otherwise.
    pub fn english() -> &'static Strings {
        static ENGLISH: OnceLock<Strings> = OnceLock::new();
        ENGLISH.get_or_init(|| Strings { language: "en".into(), texts: vec![None; Text::ALL.len()] })
    }

    /// Its language (BCP 47: `fr`, `pt-BR`, `ja`).
    pub fn language(&self) -> &str {
        &self.language
    }

    /// The text of `text` in this language, or English when it has none.
    pub fn get(&self, text: Text) -> &str {
        self.texts.get(text as usize).and_then(|t| t.as_deref()).unwrap_or(text.en())
    }

    /// The text of `text` if this language gives one.
    pub fn given(&self, text: Text) -> Option<&str> {
        self.texts.get(text as usize).and_then(|t| t.as_deref())
    }

    /// Reads a strings file's JSON. Keys it does not know, texts it leaves empty and fields it does not know are
    /// warnings (the texts stay English); anything malformed refuses the whole file.
    pub fn parse(json: &str) -> Result<Parsed, String> {
        let value: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
        let Value::Object(top) = value else { return Err("not a strings file: its JSON is not an object".into()) };
        match top.get("schema") {
            Some(Value::Number(n)) if n.as_u64() == Some(SCHEMA) => {}
            Some(Value::Number(n)) => return Err(format!("\"schema\" {n} is not one this Orbiters - N4N0 reads (it reads {SCHEMA})")),
            Some(_) => return Err(format!("\"schema\" must be the number {SCHEMA}")),
            None => return Err(format!("no \"schema\" (it must be {SCHEMA})")),
        }
        let language = match top.get("language") {
            Some(Value::String(l)) if is_language_tag(l) => l.clone(),
            Some(Value::String(l)) => return Err(format!("\"language\" {l:?} is not a language tag such as \"fr\" or \"pt-BR\"")),
            Some(_) => return Err("\"language\" must be a language tag such as \"fr\" or \"pt-BR\"".into()),
            None => return Err("no \"language\" (a language tag such as \"fr\" or \"pt-BR\")".into()),
        };
        let Some(Value::Object(given)) = top.get("strings") else {
            return Err("no \"strings\" object (each key's text: { \"home.fastWalk\": \"…\" })".into());
        };
        let mut warnings = Vec::new();
        for field in top.keys().filter(|k| !matches!(k.as_str(), "schema" | "language" | "strings" | "max")) {
            warnings.push(format!("unknown field {field:?} ignored"));
        }
        let mut texts = vec![None; Text::ALL.len()];
        for (key, value) in given {
            let Value::String(text) = value else { return Err(format!("\"strings\".{key:?} must be a text")) };
            if let Some(c) = text.chars().find(|c| c.is_control()) {
                return Err(format!("\"strings\".{key:?} has a control character (U+{:04X}): texts are one line", c as u32));
            }
            let Some(t) = Text::by_key(key) else {
                warnings.push(format!("unknown key {key:?} ignored"));
                continue;
            };
            if text.trim().is_empty() {
                warnings.push(format!("{key:?} is empty: it stays English"));
                continue;
            }
            texts[t as usize] = Some(text.trim().to_string());
        }
        if let Some(max) = top.get("max") {
            let Value::Object(max) = max else { return Err("\"max\" must be an object of numbers".into()) };
            if let Some((key, _)) = max.iter().find(|(_, v)| v.as_u64().is_none()) {
                return Err(format!("\"max\".{key:?} must be a whole number"));
            }
        }
        Ok(Parsed { strings: Strings { language, texts }, warnings })
    }

    /// Reads a strings file (see [`Strings::parse`]).
    pub fn load(path: &Path) -> Result<Parsed, String> {
        let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Strings::parse(json.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The keys this language leaves English.
    pub fn missing(&self) -> Vec<Text> {
        Text::ALL.iter().copied().filter(|&t| self.given(t).is_none()).collect()
    }

    /// The strings file of this language (keys in [`Text::ALL`]'s order), with `max` when given (the base).
    pub fn to_json(&self, max: Option<&dyn Fn(Text) -> usize>) -> String {
        let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
        let mut out = format!("{{\n  \"schema\": {SCHEMA},\n  \"language\": {},\n  \"strings\": {{\n", quote(&self.language));
        let given: Vec<Text> = Text::ALL.iter().copied().filter(|&t| self.language == "en" || self.given(t).is_some()).collect();
        for (i, &t) in given.iter().enumerate() {
            out += &format!("    {}: {}{}\n", quote(t.key()), quote(self.get(t)), if i + 1 < given.len() { "," } else { "" });
        }
        out += "  }";
        if let Some(max) = max {
            out += ",\n  \"max\": {\n";
            for (i, &t) in Text::ALL.iter().enumerate() {
                out += &format!("    {}: {}{}\n", quote(t.key()), max(t), if i + 1 < Text::ALL.len() { "," } else { "" });
            }
            out += "  }";
        }
        out += "\n}\n";
        out
    }
}

/// A strings file read, with what was left out of it.
#[derive(Clone, Debug)]
pub struct Parsed {
    pub strings: Strings,
    pub warnings: Vec<String>,
}

/// Whether `tag` reads as a BCP 47 language tag: a language of 2 or 3 letters, then subtags of 1 to 8 letters or digits
/// (`fr`, `pt-BR`, `zh-Hant-TW`).
pub fn is_language_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let first = parts.next().unwrap_or("");
    (2..=3).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Every key, for checks: the same key twice would make two texts one.
pub fn keys() -> BTreeSet<&'static str> {
    Text::ALL.iter().map(|t| t.key()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The base translations beside this crate, by file name.
    const BASES: [(&str, &str); 5] = [
        ("en", include_str!("../../menu-strings/en.json")),
        ("fr", include_str!("../../menu-strings/fr.json")),
        ("es", include_str!("../../menu-strings/es.json")),
        ("pt", include_str!("../../menu-strings/pt.json")),
        ("ja", include_str!("../../menu-strings/ja.json")),
    ];

    #[test]
    fn keys_are_unique_and_well_formed() {
        assert_eq!(keys().len(), Text::ALL.len(), "two texts share a key");
        for t in Text::ALL {
            let key = t.key();
            let (group, name) = key.split_once('.').unwrap_or_else(|| panic!("{key}: no group"));
            assert!(!group.is_empty() && !name.is_empty() && !name.contains('.'), "{key}");
            assert!(key.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'), "{key}");
            assert!(!t.en().is_empty() && t.en().trim() == t.en(), "{key}");
            assert_eq!(Text::ALL[*t as usize], *t, "ALL is in declaration order");
        }
    }

    #[test]
    fn lists_follow_what_they_name() {
        for (i, (name, _)) in orbiters_contracts::orb::LIGHT_COLORS.iter().enumerate() {
            assert_eq!(LIGHT_COLORS[i].en(), *name);
        }
        assert_eq!(LIGHT_COLORS.len(), orbiters_contracts::orb::LIGHT_COLORS.len());
        for (i, (name, _)) in orbiters_contracts::orb::LIGHT_LEVELS.iter().enumerate() {
            assert_eq!(LIGHT_LEVELS[i].en(), *name);
        }
        assert_eq!(TIMERS.len(), super::super::TIMERS.len());
        assert_eq!(FLIGHTS.len(), super::super::PhotoLength::ALL.len());
    }

    #[test]
    fn a_strings_file_reads_with_english_where_it_says_nothing() {
        let parsed = Strings::parse(
            r#"{ "schema": 1, "language": "fr", "strings": { "home.fastWalk": "Marche rapide", "home.nope": "x", "home.call": "  " }, "note": 1 }"#,
        )
        .unwrap();
        let s = parsed.strings;
        assert_eq!(s.language(), "fr");
        assert_eq!(s.get(Text::HomeFastWalk), "Marche rapide");
        assert_eq!(s.get(Text::HomeCall), "Call");
        assert_eq!(s.get(Text::HomeMusic), "Music");
        assert_eq!(parsed.warnings.len(), 3, "{:?}", parsed.warnings);
        assert!(parsed.warnings.iter().any(|w| w.contains("home.nope")));
        assert!(parsed.warnings.iter().any(|w| w.contains("\"note\"")));
    }

    #[test]
    fn malformed_files_are_refused_with_what_is_wrong() {
        for (json, says) in [
            ("nope", "not JSON"),
            ("[]", "not an object"),
            (r#"{ "language": "fr", "strings": {} }"#, "schema"),
            (r#"{ "schema": 2, "language": "fr", "strings": {} }"#, "schema"),
            (r#"{ "schema": 1, "strings": {} }"#, "language"),
            (r#"{ "schema": 1, "language": "french!", "strings": {} }"#, "language"),
            (r#"{ "schema": 1, "language": "fr" }"#, "strings"),
            (r#"{ "schema": 1, "language": "fr", "strings": [] }"#, "strings"),
            (r#"{ "schema": 1, "language": "fr", "strings": { "home.call": 3 } }"#, "home.call"),
            (r#"{ "schema": 1, "language": "fr", "strings": { "home.call": "a\nb" } }"#, "control"),
            (r#"{ "schema": 1, "language": "fr", "strings": {}, "max": { "home.call": "x" } }"#, "max"),
        ] {
            let error = Strings::parse(json).expect_err(json);
            assert!(error.contains(says), "{json}: {error}");
        }
    }

    #[test]
    fn language_tags() {
        for ok in ["en", "fr", "pt-BR", "zh-Hant-TW", "ja", "yue"] {
            assert!(is_language_tag(ok), "{ok}");
        }
        for bad in ["", "e", "english", "pt_BR", "pt-", "fr-123456789", "日本"] {
            assert!(!is_language_tag(bad), "{bad}");
        }
    }

    #[test]
    fn a_language_writes_and_reads_back() {
        let parsed = Strings::parse(BASES[1].1).unwrap();
        let again = Strings::parse(&parsed.strings.to_json(None)).unwrap();
        assert_eq!(again.strings, parsed.strings);
    }

    /// The base translations: every key given, nothing unknown, each in the language its file is named after.
    #[test]
    fn the_base_translations_are_whole() {
        for (name, json) in BASES {
            let parsed = Strings::parse(json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(parsed.warnings.is_empty(), "{name}: {:?}", parsed.warnings);
            assert_eq!(parsed.strings.language().split('-').next(), Some(name), "{name}");
            let missing: Vec<&str> = parsed.strings.missing().iter().map(|t| t.key()).collect();
            assert!(missing.is_empty(), "{name} leaves English: {missing:?}");
        }
        let en = Strings::parse(BASES[0].1).unwrap().strings;
        for t in Text::ALL {
            assert_eq!(en.get(*t), t.en(), "en.json says otherwise than the base for {}", t.key());
        }
    }
}
