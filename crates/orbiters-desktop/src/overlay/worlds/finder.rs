//! VRChat's worlds in general for the panel, not only those members captured: a search, VRChat's popular and active
//! worlds and the member's favorites (VRChat through the member's own session in the tray service when signed in, else
//! Orbiters' public Map explorer: its search and featured worlds), and the worlds visited on this computer
//! (orbiters-core `visited.rs`). Each list is merged with Orbiters' 3D captures ([`merge`]): a world with captures is
//! listed as its captures (the 3D flow), the others as worlds whose page has Join and Open on VRChat.
//!
//! Pages are asked one at a time on a thread of their own and kept five minutes ([`Finder`]); the panel's typing is
//! debounced by its host before a search is asked.

use orbiters_core::locks::*;
use super::catalog::{Aggregate, Entry};
use orbiters_core::maps::{FoundBy, FoundWorld, FoundWorlds, World, WorldList};
use orbiters_i18n::tr;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the panel browses while nothing is typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Section {
    /// The worlds members captured in 3D (Orbiters' listing, as before): what the panel opens on, as it is what it is for.
    #[default]
    Captures,
    /// VRChat's popular worlds (Orbiters' featured worlds without a VRChat session).
    Popular,
    /// The worlds with the most people now (VRChat only).
    Active,
    /// The member's favorite worlds on VRChat (VRChat only).
    Favorites,
    /// The worlds visited on this computer, the latest first.
    Visited,
}

impl Section {
    pub const ALL: [Section; 5] = [Section::Captures, Section::Popular, Section::Active, Section::Favorites, Section::Visited];

    /// Its name (`vrchat`: signed in to VRChat; Orbiters' featured worlds stand in for VRChat's popular ones otherwise).
    pub fn label(self, vrchat: bool) -> &'static str {
        match self {
            Section::Captures => tr("3D captures"),
            Section::Popular if vrchat => tr("Popular"),
            Section::Popular => tr("Featured"),
            Section::Active => tr("Active now"),
            Section::Favorites => tr("Favorites"),
            Section::Visited => tr("Visited"),
        }
    }

    /// The remote list it shows, if it is one.
    pub fn list(self) -> Option<WorldList> {
        match self {
            Section::Popular => Some(WorldList::Popular),
            Section::Active => Some(WorldList::Active),
            Section::Favorites => Some(WorldList::Favorites),
            Section::Captures | Section::Visited => None,
        }
    }

    /// Only VRChat has it: without a VRChat session the panel says to sign in instead of asking.
    pub fn needs_vrchat(self) -> bool {
        matches!(self, Section::Active | Section::Favorites)
    }
}

/// A list's answers are asked again after this long (an error's sooner).
const KEEP: Duration = Duration::from_secs(300);
const KEEP_ERROR: Duration = Duration::from_secs(20);
/// Answers kept at most (pages of searches typed lately).
const ANSWERS: usize = 96;

type Key = (WorldList, usize);
type Fetch = Box<dyn FnOnce() -> Result<FoundWorlds, String> + Send>;

struct Answer {
    result: Result<FoundWorlds, String>,
    at: Instant,
}

impl Answer {
    fn fresh(&self) -> bool {
        self.at.elapsed() < if self.result.is_ok() { KEEP } else { KEEP_ERROR }
    }
}

#[derive(Default)]
struct State {
    answers: HashMap<Key, Answer>,
    /// The page being read.
    asking: Option<Key>,
    /// The page asked for meanwhile (the latest only: typing on replaces it).
    wanted: Option<(Key, Fetch)>,
    /// Bumped with each answer.
    version: u64,
    /// Bumped when the answers are let go (the VRChat session or the account changed): late answers are dropped.
    generation: u64,
}

/// A list as far as it was read: its pages' worlds in order, whether more follow, a page on its way, why one failed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    pub items: Vec<FoundWorld>,
    pub more: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub source: Option<FoundBy>,
    /// Pages read.
    pub pages: usize,
}

/// The pages of world lists read lately, read one at a time on a thread of their own.
#[derive(Clone, Default)]
pub struct Finder {
    state: Arc<Mutex<State>>,
}

impl Finder {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.locked()
    }

    /// Asks for page `page` of `list` with `fetch` (run on a thread), unless it was answered lately or is being read.
    /// While another page is read, it waits its turn (only the latest asked waits).
    pub fn ask(&self, list: &WorldList, page: usize, fetch: impl FnOnce() -> Result<FoundWorlds, String> + Send + 'static) {
        let key = (list.clone(), page);
        let mut s = self.lock();
        if s.asking.as_ref() == Some(&key) || s.answers.get(&key).is_some_and(Answer::fresh) {
            return;
        }
        if s.asking.is_some() {
            s.wanted = Some((key, Box::new(fetch)));
            return;
        }
        s.asking = Some(key.clone());
        let generation = s.generation;
        drop(s);
        let state = self.state.clone();
        let started = std::thread::Builder::new().name("3d-worlds-find".into()).spawn(move || run(state, key, Box::new(fetch), generation));
        if started.is_err() {
            self.lock().asking = None;
        }
    }

    /// Lets every answer go (the VRChat session or the account changed: another source answers now).
    pub fn clear(&self) {
        let mut s = self.lock();
        s.answers.clear();
        s.asking = None;
        s.wanted = None;
        s.generation += 1;
        s.version += 1;
    }

    /// Bumped with each answer: the panel paints again.
    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// Lets the failed answers go: asked again at once (Try again).
    pub fn retry(&self) {
        let mut s = self.lock();
        s.answers.retain(|_, a| a.result.is_ok());
        s.version += 1;
    }

    /// `list` as far as it was read: its first pages answered in a row.
    pub fn list(&self, list: &WorldList) -> Listed {
        self.read(list, true)
    }

    /// [`Finder::list`] without its worlds (the panel asks it every turn).
    pub fn status(&self, list: &WorldList) -> Listed {
        self.read(list, false)
    }

    /// Worlds of `list` read so far.
    pub fn count(&self, list: &WorldList) -> usize {
        let s = self.lock();
        (0..)
            .map_while(|page| match s.answers.get(&(list.clone(), page)).map(|a| &a.result) {
                Some(Ok(found)) => Some((found.items.len(), found.more)),
                _ => None,
            })
            .scan(true, |go, (n, more)| {
                go.then(|| {
                    *go = more;
                    n
                })
            })
            .sum()
    }

    fn read(&self, list: &WorldList, items: bool) -> Listed {
        let s = self.lock();
        let mut out =
            Listed { loading: s.asking.as_ref().is_some_and(|k| k.0 == *list) || s.wanted.as_ref().is_some_and(|(k, _)| k.0 == *list), ..Default::default() };
        let mut seen = HashSet::new();
        for page in 0.. {
            match s.answers.get(&(list.clone(), page)).map(|a| &a.result) {
                Some(Ok(found)) => {
                    // A world twice across pages (VRChat's order moved meanwhile) is listed once.
                    if items {
                        out.items.extend(found.items.iter().filter(|w| seen.insert(w.world.world_id.to_ascii_lowercase())).cloned());
                    }
                    out.more = found.more;
                    out.source = Some(found.source);
                    out.pages = page + 1;
                    if !found.more {
                        break;
                    }
                }
                Some(Err(e)) => {
                    out.error = Some(e.clone());
                    out.more = false;
                    break;
                }
                None => break,
            }
        }
        out
    }
}

/// Reads `key`, then the page asked for meanwhile, and so on.
fn run(state: Arc<Mutex<State>>, mut key: Key, mut fetch: Fetch, generation: u64) {
    loop {
        let result = fetch();
        let mut s = state.locked();
        if s.generation != generation {
            return;
        }
        if s.answers.len() >= ANSWERS {
            s.answers.retain(|_, a| a.fresh());
            if s.answers.len() >= ANSWERS {
                s.answers.clear();
            }
        }
        s.answers.insert(key, Answer { result, at: Instant::now() });
        s.version += 1;
        s.asking = None;
        loop {
            let Some((next, next_fetch)) = s.wanted.take() else { return };
            if s.answers.get(&next).is_some_and(Answer::fresh) {
                continue;
            }
            s.asking = Some(next.clone());
            key = next;
            fetch = next_fetch;
            break;
        }
    }
}

/// A world visited on this computer: id, name, the latest visit (Unix seconds), how many visits.
pub type VisitedWorld = (String, String, u64, usize);

/// The worlds visited (VRChat's logs as the tray service keeps them), read on a thread at most every half minute.
#[derive(Clone, Default)]
pub struct Visited {
    state: Arc<Mutex<(Option<Instant>, Vec<VisitedWorld>, bool, u64)>>,
}

/// Visited worlds the panel lists at most.
const VISITED_MAX: usize = 240;

impl Visited {
    pub fn refresh(&self) {
        let mut s = self.state.locked();
        if s.2 || s.0.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        s.2 = true;
        let state = self.state.clone();
        let started = std::thread::Builder::new().name("3d-worlds-visited".into()).spawn(move || {
            let history = orbiters_core::visited::History::load(&orbiters_core::visited::history_file());
            let worlds = visited_worlds(&history);
            let mut s = state.locked();
            *s = (Some(Instant::now()), worlds, false, s.3 + 1);
        });
        if started.is_err() {
            s.2 = false;
        }
    }

    /// The worlds visited, whether they are being read, and a number bumped with each read.
    pub fn worlds(&self) -> (Vec<VisitedWorld>, bool, u64) {
        let s = self.state.locked();
        (s.1.clone(), s.2 || s.0.is_none(), s.3)
    }

    /// [`Visited::worlds`] without the worlds (asked every turn).
    pub fn state(&self) -> (bool, u64) {
        let s = self.state.locked();
        (s.2 || s.0.is_none(), s.3)
    }
}

/// The history's worlds, the latest visit first.
fn visited_worlds(history: &orbiters_core::visited::History) -> Vec<VisitedWorld> {
    let mut worlds: Vec<VisitedWorld> = history
        .worlds
        .iter()
        .filter(|(id, _)| orbiters_core::vrchat_log::is_world_id(id))
        .map(|(id, v)| {
            (id.clone(), v.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| tr("Unnamed world").into()), v.last().unwrap_or(0), v.visits.len())
        })
        .collect();
    worlds.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    worlds.truncate(VISITED_MAX);
    worlds
}

/// The visited worlds as the lists' worlds (their pictures and numbers come with their pages on the Map explorer).
pub fn visited_found(worlds: &[VisitedWorld]) -> Vec<(FoundWorld, u64, usize)> {
    worlds
        .iter()
        .map(|(id, name, last, visits)| {
            (FoundWorld { world: World { world_id: id.clone(), name: name.clone(), ..Default::default() }, ..Default::default() }, *last, *visits)
        })
        .collect()
}

/// The worlds `found` (in their order) as the panel lists them, after `first` (the captures a search matched here): a
/// world Orbiters has captures of as all its captures, each a card as in the 3D captures list (the list's thumbnail and
/// people kept with them), the others as worlds with `feedback`, Orbiters' for everyone. A world is listed once.
/// `found_by`: the search that found them, so that they stay its results.
pub fn merge(captures: &[Entry], first: &[&Entry], found: &[FoundWorld], found_by: Option<&str>, feedback: &dyn Fn(&str) -> Aggregate) -> Vec<Entry> {
    let mut by_world: HashMap<String, Vec<&Entry>> = HashMap::new();
    for c in captures {
        by_world.entry(c.world_id.to_ascii_lowercase()).or_default().push(c);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for c in first {
        let key = c.world_id.to_ascii_lowercase();
        if seen.insert(key.clone()) {
            out.extend(by_world.get(&key).into_iter().flatten().map(|c| (*c).clone()));
        }
    }
    for f in found {
        let key = f.world.world_id.to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }
        match by_world.get(&key) {
            Some(theirs) => out.extend(theirs.iter().map(|c| {
                let mut c = (*c).clone();
                c.found_by = found_by.map(|q| q.trim().to_lowercase());
                c.thumbnail = c.thumbnail.or_else(|| f.world.thumbnail_url.clone());
                c.occupants = c.occupants.or(f.occupants);
                c.performance = c.performance.or_else(|| f.world.performance.clone());
                c
            })),
            None => out.push(Entry::world(f, found_by, feedback(&f.world.world_id))),
        }
    }
    out
}

/// VRChat's region of the instance `instance` (`48213~region(eu)`, as its log names it), `us` when it says none
/// (VRChat's own default).
pub fn region(instance: Option<&str>) -> &'static str {
    let Some(at) = instance.and_then(|i| i.find("~region(")) else { return "us" };
    let rest = &instance.unwrap_or_default()[at + "~region(".len()..];
    ["use", "us", "eu", "jp"].into_iter().find(|r| rest.starts_with(&format!("{r})"))).unwrap_or("us")
}

/// A new public instance of a world in `region`, named by `seed` as VRChat names them (five digits).
pub fn new_instance(region: &str, seed: u64) -> String {
    format!("{:05}~region({region})", 10_000 + seed % 90_000)
}

/// The link that launches VRChat in `world`'s instance `instance`, for an existing world id only.
pub fn launch_link(world_id: &str, instance: &str) -> Option<String> {
    let instance_ok = instance.bytes().all(|c| c.is_ascii_alphanumeric() || b"~()_-".contains(&c)) && !instance.is_empty();
    (orbiters_core::vrchat_log::is_world_id(world_id) && instance_ok).then(|| format!("vrchat://launch?ref=orbiters.cc&id={world_id}:{instance}"))
}

/// The world's page on vrchat.com.
pub fn web_link(world_id: &str) -> Option<String> {
    orbiters_core::vrchat_log::is_world_id(world_id).then(|| format!("https://vrchat.com/home/world/{world_id}"))
}

/// Opens a [`launch_link`] with the program Windows has for `vrchat://` links (VRChat's launcher).
pub fn open_launch(link: &str) -> anyhow::Result<()> {
    anyhow::ensure!(link.starts_with("vrchat://launch?ref=orbiters.cc&id=wrld_"), "{}", tr("Not a VRChat launch link"));
    #[cfg(windows)]
    {
        use windows::core::{w, HSTRING, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        // SAFETY: a plain call with valid, null-terminated strings.
        let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(link), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        // Values above 32 mean success.
        anyhow::ensure!(result.0 as usize > 32, "{}", tr!("VRChat's launcher did not open (error {code}): is VRChat installed?", code = result.0 as usize));
    }
    #[cfg(not(windows))]
    anyhow::bail!("{}", tr("Launching VRChat is for Windows"));
    #[cfg(windows)]
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_that_cannot_happen_says_so_in_the_language_now() {
        use orbiters_i18n::{in_language, Language};
        assert_eq!(open_launch("https://example.com").unwrap_err().to_string(), "Not a VRChat launch link");
        assert_eq!(in_language(Language::PtBr, || open_launch("https://example.com").unwrap_err().to_string()), "Não é um link de inicialização do VRChat");
    }

    const HOME: &str = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";

    fn world(id: &str, name: &str) -> FoundWorld {
        FoundWorld {
            world: World { world_id: id.into(), name: name.into(), thumbnail_url: Some(format!("https://api.vrchat.cloud/{name}")), ..Default::default() },
            occupants: Some(7),
            ..Default::default()
        }
    }

    fn id(i: u32) -> String {
        format!("wrld_{i:08x}-0000-4000-8000-000000000000")
    }

    #[test]
    fn a_list_merges_with_the_captures_of_its_worlds() {
        let captures = super::super::catalog::parse(&format!(
            r#"{{"previews":[
            {{"slot":0,"key":1,"id":1,"worldId":"{HOME}","name":"Home","author":"vrchat","uploader":"a","view":{{"position":[0,1,0],"target":[0,1,1]}}}},
            {{"slot":1,"key":2,"id":2,"worldId":"{}","name":"Dawn","author":"Mei","uploader":"b","view":{{"position":[0,1,0],"target":[0,1,1]}}}},
            {{"slot":2,"key":3,"id":3,"worldId":"{}","name":"Home","author":"vrchat","uploader":"Kana","view":{{"position":[0,1,0],"target":[0,1,1]}}}}
        ]}}"#,
            id(2),
            HOME.to_uppercase()
        ))
        .unwrap();
        let liked = |w: &str| if w == id(9) { Aggregate { likes: 4, ..Default::default() } } else { Aggregate::default() };
        // A search found Home (two captures), a world without one, Home again: Home is its captures, listed once.
        let found = [world(HOME, "VRChat Home"), world(&id(9), "Black Cat"), world(HOME, "VRChat Home")];
        let merged = merge(&captures, &[], &found, Some(" Cat "), &liked);
        assert_eq!(merged.iter().map(|e| (e.capture, e.name.as_str())).collect::<Vec<_>>(), [(true, "Home"), (true, "Home"), (false, "Black Cat")]);
        assert_eq!(merged[0].occupants, Some(7), "the list's people and picture kept with the capture");
        assert_eq!(merged[0].thumbnail.as_deref(), Some("https://api.vrchat.cloud/VRChat Home"));
        let cat = &merged[2];
        assert_eq!((cat.feedback.likes, cat.id & super::super::catalog::WORLD_BIT != 0), (4, true));
        assert_eq!(cat.id, super::super::catalog::world_entry_id(&id(9).to_uppercase()), "a world's id is stable, whatever its case");
        assert!(cat.matches("cat") && cat.matches("CAT ") && !cat.matches("dog"));
        assert!(merged[0].matches("cat"), "found by the search: still its result, whatever its name");
        // A search's own matches among the captures come first, the list after them without repeating them.
        let dawn: Vec<&Entry> = captures.iter().filter(|e| e.matches("dawn")).collect();
        let merged = merge(&captures, &dawn, &[world(&id(9), "Black Cat"), world(&id(2), "Dawn")], Some("dawn"), &liked);
        assert_eq!(merged.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Dawn", "Black Cat"]);
    }

    fn page(items: Vec<FoundWorld>, more: bool) -> FoundWorlds {
        FoundWorlds { items, more, source: FoundBy::Vrchat }
    }

    fn settle(finder: &Finder, version: u64) {
        for _ in 0..400 {
            if finder.version() >= version && finder.lock().asking.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("no answer");
    }

    #[test]
    fn pages_are_kept_and_read_in_a_row() {
        let finder = Finder::default();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let list = WorldList::Search("cat".into());
        let ask = |n: usize, items: Vec<FoundWorld>, more: bool| {
            let calls = calls.clone();
            finder.ask(&list, n, move || {
                calls.lock().unwrap().push(n);
                Ok(page(items, more))
            });
        };
        ask(0, vec![world(&id(1), "A"), world(&id(2), "B")], true);
        settle(&finder, 1);
        // Answered lately: not asked again.
        ask(0, vec![], false);
        settle(&finder, 1);
        assert_eq!(*calls.lock().unwrap(), [0]);
        let first = finder.list(&list);
        assert_eq!((first.items.len(), first.more, first.pages, first.source), (2, true, 1, Some(FoundBy::Vrchat)));
        // The next page, B again in it: listed once.
        ask(1, vec![world(&id(2), "B"), world(&id(3), "C")], false);
        settle(&finder, 2);
        let both = finder.list(&list);
        assert_eq!(both.items.iter().map(|w| w.world.name.as_str()).collect::<Vec<_>>(), ["A", "B", "C"]);
        assert!(!both.more && !both.loading);
        assert_eq!((finder.count(&list), finder.status(&list).pages, finder.status(&list).items.len()), (4, 2, 0));
        // An error is the list's end, said.
        let other = WorldList::Favorites;
        finder.ask(&other, 0, || Err("Sign in to VRChat".into()));
        settle(&finder, 3);
        assert_eq!(finder.list(&other).error.as_deref(), Some("Sign in to VRChat"));
        // Try again lets it go: asked at once.
        finder.retry();
        finder.ask(&other, 0, || Ok(page(vec![], false)));
        settle(&finder, 5);
        assert_eq!(finder.list(&other).error, None);
        // Let go: nothing is kept, a late answer is dropped.
        finder.clear();
        assert_eq!(finder.list(&list), Listed::default());
    }

    #[test]
    fn a_page_asked_while_another_is_read_waits_its_turn_the_latest_only() {
        let finder = Finder::default();
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let order = Arc::new(Mutex::new(Vec::new()));
        let o = order.clone();
        finder.ask(&WorldList::Search("a".into()), 0, move || {
            wait.recv().unwrap();
            o.lock().unwrap().push("a");
            Ok(page(vec![], false))
        });
        for q in ["ab", "abc"] {
            let o = order.clone();
            finder.ask(&WorldList::Search(q.into()), 0, move || {
                o.lock().unwrap().push(if q == "ab" { "ab" } else { "abc" });
                Ok(page(vec![], false))
            });
        }
        assert!(finder.list(&WorldList::Search("abc".into())).loading);
        go.send(()).unwrap();
        settle(&finder, 2);
        assert_eq!(*order.lock().unwrap(), ["a", "abc"], "typing on replaced what waited");
    }

    #[test]
    fn visited_worlds_come_latest_first_and_join_links_name_a_public_instance() {
        let mut history = orbiters_core::visited::History::default();
        for (world, name, visits) in [(id(1), Some("Old"), vec![10, 20]), (id(2), None, vec![50]), ("not a world".to_string(), Some("x"), vec![99])] {
            history.worlds.insert(world, orbiters_core::visited::WorldVisits { name: name.map(str::to_string), visits, ..Default::default() });
        }
        let worlds = visited_worlds(&history);
        assert_eq!(worlds, [(id(2), "Unnamed world".to_string(), 50, 1), (id(1), "Old".to_string(), 20, 2)]);
        assert_eq!(region(Some("12345~group(grp_x)~region(eu)")), "eu");
        assert_eq!(region(Some("12345~region(use)")), "use");
        assert_eq!(region(Some("12345")), "us");
        assert_eq!(region(None), "us");
        let instance = new_instance("eu", 123_456_789);
        assert!(orbiters_desktop_public(&instance), "{instance}");
        assert_eq!(launch_link(HOME, &instance).unwrap(), format!("vrchat://launch?ref=orbiters.cc&id={HOME}:{instance}"));
        assert_eq!(launch_link("wrld_../x", "1"), None);
        assert_eq!(launch_link(HOME, "1&ref=evil"), None);
        assert_eq!(web_link(HOME).unwrap(), format!("https://vrchat.com/home/world/{HOME}"));
        assert!(open_launch("https://example.com").is_err());
    }

    fn orbiters_desktop_public(instance: &str) -> bool {
        crate::controller::vrchat_data::public_instance(instance)
    }
}
