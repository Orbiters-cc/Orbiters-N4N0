//! N4N0's saved avatar states (`orbiters_n4n0_core::states`; contracts `Docs/n4n0.md`, States): saving how the avatar worn is
//! (every parameter OSC may write, as VRChat's OSCQuery tree has it, and its eye height), putting it back over OSC a
//! step at a time, and the default state each avatar may have, put back after each world join and each switch to it,
//! once VRChat has applied the avatar's own values.
//!
//! The file is read once and written whole after each change, outside the N4N0's lock (the screen's steps never wait for
//! the disk), one write at a time and the newest last.

use crate::runtime::{N4n0, N4n0Runtime, Shared};
use crate::status::{AvatarView, NoticeKind, StateAction, StateView, StatesNote, StatesView};
use orbiters_core::osc::{OscValue, TreeNode, CAPTURE_PARAMETERS, PARAMETERS_PREFIX};
use orbiters_core::vrchat_log::LogEvent;
use orbiters_n4n0_core::menu::StateRow;
use orbiters_n4n0_core::states::{self, Current, Loaded, Refused, Store};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// An avatar's default state goes this long after its last `/avatar/change`: VRChat has applied the avatar's own defaults
/// and saved values by then, and N4N0 has read its parameters (1.5 s after the change).
const DEFAULT_SETTLE: Duration = Duration::from_millis(if cfg!(test) { 1800 } else { 3000 });
/// It waits at most this long for the avatar's parameters to be read (a heavy avatar loading).
const DEFAULT_WAIT: Duration = Duration::from_secs(30);
/// A world joined (VRChat's log): VRChat loads the avatar again there, and its `/avatar/change` puts the default back
/// once settled. Without one by then, the default goes anyway.
const JOIN_WAIT: Duration = Duration::from_secs(20);
/// A default put back this recently is not put back again for a join without a reload.
const DEFAULT_AGAIN: Duration = Duration::from_secs(30);
/// An apply's steps go this far apart (two of VRChat's frames at 60 fps and more), [`states::STEP_MESSAGES`] each.
const APPLY_STEP: Duration = Duration::from_millis(30);
/// How long VRChat gets to say the avatar's parameters.
const TREE_TIMEOUT: Duration = Duration::from_secs(2);
const EYE_HEIGHT: &str = states::EYE_HEIGHT;
const SCALING_ALLOWED: &str = states::EYE_HEIGHT_SCALING_ALLOWED;

/// One write of the file at a time, the newest content last.
static WRITING: Mutex<()> = Mutex::new(());

/// What a state saved now puts back besides the avatar's parameters, and whether it becomes the avatar's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SaveOptions {
    pub size: bool,
    pub pose: bool,
    pub default: bool,
}

/// A default state to put back: for which avatar, from when, until when its parameters may be waited for.
struct Due {
    avatar: String,
    at: Instant,
    until: Instant,
}

pub(crate) struct StatesRuntime {
    path: PathBuf,
    store: Option<Store>,
    /// The file could not be read: nothing is written over it until it can be.
    unreadable: Option<String>,
    /// Bumped on every change (the screen's menu follows it).
    pub(crate) version: u64,
    note: Option<StatesNote>,
    notes: u64,
    saving: bool,
    due: Option<Due>,
    pub(crate) joined: Option<Instant>,
    applied: Option<(String, Instant)>,
}

/// Without a file of its own (the runtime gives it the app's, [`crate::Files::states`]); a test's own runtime gets one
/// of its own, which goes with it.
impl Default for StatesRuntime {
    fn default() -> Self {
        Self::at(if cfg!(test) { test_file() } else { PathBuf::new() })
    }
}

/// A test's file goes with its runtime.
#[cfg(test)]
impl Drop for StatesRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A file of its own for each runtime a test makes: the person's states stay out of tests.
fn test_file() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("orbiters-states-test-{}-{}.json", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)))
}

impl StatesRuntime {
    pub fn at(path: PathBuf) -> Self {
        Self { path, store: None, unreadable: None, version: 0, note: None, notes: 0, saving: false, due: None, joined: None, applied: None }
    }

    /// The store, read from the file the first time. One that could not be read is read again (it may be free now).
    pub(crate) fn store(&mut self) -> &mut Store {
        if self.store.is_none() || self.unreadable.is_some() {
            let (store, loaded) = Store::read(&self.path);
            match loaded {
                Loaded::Whole => self.unreadable = None,
                Loaded::Partial(aside) => {
                    self.unreadable = None;
                    let kept = aside.map(|p| format!(": the file as it was is kept as {}", p.display())).unwrap_or_default();
                    self.say(format!("Some saved states could not be read{kept}."), true);
                }
                Loaded::Unreadable(e) => {
                    self.say(format!("The saved states could not be read ({e}): nothing is saved over them until they can be."), true);
                    self.unreadable = Some(e);
                }
            }
            if self.store.is_none() || self.unreadable.is_none() {
                self.store = Some(store);
            }
            self.version += 1;
        }
        self.store.as_mut().expect("just read")
    }

    /// Reads the file (the app starts).
    pub(crate) fn load(&mut self) {
        self.store();
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.notes += 1;
        self.note = Some(StatesNote { id: self.notes, text: text.into(), error });
        self.version += 1;
    }

    /// The menu's rows for `avatar`.
    pub(crate) fn rows(&self, avatar: Option<&str>) -> Vec<StateRow> {
        avatar.and_then(|a| self.store.as_ref()?.avatar(a)).map(states::rows).unwrap_or_default()
    }

    /// What the window and the menus show for `avatar`, the avatar worn (`name`: its name as VRChat's OSCQuery gives it):
    /// its states, and every avatar's for the window's cards.
    pub(crate) fn view(&self, avatar: Option<&str>, name: Option<&str>) -> StatesView {
        let views = |e: &states::AvatarStates| -> Vec<StateView> {
            e.states
                .iter()
                .map(|s| StateView {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    saved_utc: s.saved_utc.clone(),
                    default: e.default.as_deref() == Some(s.id.as_str()),
                    pose: s.pose,
                    size: s.size,
                    parameters: s.parameters.len(),
                    pose_parameters: s.pose_parameters(),
                    eye_height: s.eye_height,
                })
                .collect()
        };
        let entry = avatar.and_then(|a| self.store.as_ref()?.avatar(a));
        let states = entry.map(views).unwrap_or_default();
        let name = name.map(str::to_string).or_else(|| entry.and_then(|e| e.name.clone()));
        // Every avatar with states, the worn one first (with or without), then the one saved in last.
        let mut avatars: Vec<AvatarView> = self
            .store
            .as_ref()
            .map(|store| {
                store
                    .avatars
                    .iter()
                    .filter(|(id, e)| !e.states.is_empty() && Some(id.as_str()) != avatar)
                    .map(|(id, e)| AvatarView { id: id.clone(), name: e.name.clone(), worn: false, states: views(e) })
                    .collect()
            })
            .unwrap_or_default();
        let newest = |a: &AvatarView| a.states.iter().map(|s| s.saved_utc.clone()).max().unwrap_or_default();
        avatars.sort_by(|a, b| newest(b).cmp(&newest(a)).then_with(|| a.id.cmp(&b.id)));
        if let Some(worn) = avatar {
            avatars.insert(0, AvatarView { id: worn.to_string(), name: name.clone(), worn: true, states: states.clone() });
        }
        StatesView { avatar_id: avatar.map(str::to_string), avatar_name: name, states, note: self.note.clone(), saving: self.saving, avatars }
    }

    /// `avatar`'s name as set in its SDK (VRChat's OSCQuery `/avatar/name`): kept with its states, when it has some.
    /// Whether that changed the store (to be written).
    pub(crate) fn named(&mut self, avatar: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.unreadable.is_some() {
            return false;
        }
        let Some(entry) = self.store().avatars.get_mut(avatar) else { return false };
        if entry.name.as_deref() == Some(name) {
            return false;
        }
        entry.name = Some(name.to_string());
        self.version += 1;
        true
    }

    /// VRChat loaded `avatar` (a switch, or the same one again in a new world): its default goes once it settled.
    pub(crate) fn avatar_changed(&mut self, avatar: &str, now: Instant) {
        self.due = Some(Due { avatar: avatar.to_string(), at: now + DEFAULT_SETTLE, until: now + DEFAULT_WAIT });
        self.joined = None;
    }

    /// When the N4N0 thread should next look (a default due).
    pub(crate) fn next_due(&self) -> Option<Instant> {
        let due = self.due.as_ref().map(|d| d.at);
        let joined = self.joined.map(|t| t + JOIN_WAIT);
        due.into_iter().chain(joined).min()
    }
}

/// The avatar's nodes of VRChat's tree, split: its parameters by name, its eye height, whether the world allows
/// scaling.
fn split(nodes: Vec<TreeNode>) -> (BTreeMap<String, Current>, Option<f32>, Option<bool>) {
    let (mut parameters, mut eye, mut scaling) = (BTreeMap::new(), None, None);
    for node in nodes {
        if let Some(name) = node.path.strip_prefix(PARAMETERS_PREFIX) {
            parameters.insert(name.to_string(), Current { value: node.value, writable: node.writable });
        } else if node.path == EYE_HEIGHT {
            eye = float(&node.value);
        } else if node.path == SCALING_ALLOWED {
            scaling = Some(node.value.truthy());
        }
    }
    (parameters, eye, scaling)
}

fn float(value: &OscValue) -> Option<f32> {
    match value {
        OscValue::Float(f) if f.is_finite() && *f > 0.0 => Some(*f),
        _ => None,
    }
}

/// At the N4N0 thread's step: the default state to put back now, as (avatar, state id). An avatar change's default once
/// it settled and its parameters were read; a world join's when no reload came (and none went lately).
pub(crate) fn tick(rt: &mut N4n0Runtime, now: Instant) -> Option<(String, String)> {
    let avatar = rt.avatar_id.clone();
    let known = !rt.params.is_empty() && !rt.tree_unknown;
    let s = &mut rt.states;
    let mut go: Option<String> = None;
    if s.due.as_ref().is_some_and(|d| now >= d.at) {
        let due = s.due.take().expect("due");
        if avatar.as_deref() != Some(due.avatar.as_str()) {
            // Another avatar came meanwhile: its own change says when.
        } else if known {
            go = Some(due.avatar);
        } else if now < due.until {
            s.due = Some(Due { at: now + Duration::from_secs(1), ..due });
        }
    }
    if s.joined.is_some_and(|t| now >= t + JOIN_WAIT) {
        s.joined = None;
        let recent = s.applied.as_ref().is_some_and(|(a, at)| Some(a.as_str()) == avatar.as_deref() && now.duration_since(*at) < DEFAULT_AGAIN);
        if go.is_none() && s.due.is_none() && known && !recent {
            go = avatar.clone();
        }
    }
    let avatar = go?;
    let id = s.store().default_of(&avatar)?.id.clone();
    s.applied = Some((avatar.clone(), now));
    Some((avatar, id))
}

impl N4n0 {
    /// The avatar's nodes of VRChat's OSCQuery tree (empty when VRChat or its tree is not found), read without the OSC
    /// lock.
    fn vrchat_nodes(&self) -> anyhow::Result<Vec<TreeNode>> {
        let query = self.osc.vrchat_query().flatten().ok_or_else(|| anyhow::anyhow!("VRChat's OSCQuery service was not found"))?;
        orbiters_core::osc::vrchat_nodes(query, TREE_TIMEOUT)
    }

    /// The latest value VRChat sent for `address` (the eye height, which it sends when it changes).
    fn osc_latest_value(&self, address: &str) -> Option<OscValue> {
        self.osc.latest(&mut std::iter::once(address.to_string())).remove(address)
    }

    /// Writes the states file, the newest content: refused while it could not be read (it would be replaced).
    fn states_write(&self) -> Result<(), String> {
        let _writing = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (store, path) = {
            let mut rt = self.n4n0.lock().unwrap();
            let s = &mut rt.states;
            s.store();
            if let Some(e) = &s.unreadable {
                return Err(format!("The saved states could not be read ({e}): nothing was saved, so they stay whole."));
            }
            (s.store.clone().unwrap_or_default(), s.path.clone())
        };
        store.save(&path).map_err(|e| format!("Could not save the states: {e}"))
    }

    /// What the interfaces and the menus ask of the saved states: the avatar worn's, or `avatar`'s (the window's States
    /// page, another avatar's card: its default, names, options and deletes; applying and saving are the worn one's).
    pub(crate) fn states_action(self: &Shared, action: StateAction, avatar: Option<String>) {
        let worn = self.n4n0.lock().unwrap().avatar_id.clone();
        let other = avatar.filter(|a| Some(a) != worn.as_ref());
        match action {
            StateAction::Save { .. } | StateAction::Apply { .. } if other.is_some() => {
                let e = "Wear this avatar in VRChat to save or apply its states.";
                self.n4n0.lock().unwrap().states.say(e, true);
                self.notify(NoticeKind::Info, e);
                self.publish();
            }
            StateAction::Save { name, size, pose, default } => self.states_save(name, SaveOptions { size, pose, default }),
            StateAction::Apply { id } => self.states_apply(id, false),
            StateAction::Default { id } => self.states_change(other, |store, avatar| store.set_default(avatar, id.as_deref())),
            StateAction::Rename { id, name } => self.states_change(other, |store, avatar| store.rename(avatar, &id, &name)),
            StateAction::Delete { id } => self.states_change(other, |store, avatar| store.delete(avatar, &id)),
            StateAction::Options { id, pose, size } => self.states_change(other, |store, avatar| store.set_options(avatar, &id, pose, size)),
        }
    }

    /// `avatar`'s name as set in its SDK (VRChat's OSCQuery `/avatar/name`), kept with its states when it has some (the
    /// file written when that changed it).
    pub(crate) fn states_remember_name(self: &Shared, avatar: &str, name: &str) {
        if !self.n4n0.lock().unwrap().states.named(avatar, name) {
            return;
        }
        if let Err(e) = self.states_write() {
            eprintln!("states: the avatar's name: {e}");
        }
    }

    /// A change to the states of `avatar` (`None`: the avatar worn), kept at once.
    fn states_change(self: &Shared, avatar: Option<String>, change: impl FnOnce(&mut Store, &str) -> Result<(), Refused>) {
        let refused = {
            let mut rt = self.n4n0.lock().unwrap();
            let Some(avatar) = avatar.or_else(|| rt.avatar_id.clone()) else {
                rt.states.say("VRChat's avatar is not known yet.", true);
                drop(rt);
                self.publish();
                return;
            };
            let s = &mut rt.states;
            s.store();
            if let Some(e) = s.unreadable.clone() {
                s.say(format!("The saved states could not be read ({e}): nothing was changed, so they stay whole."), true);
                drop(rt);
                self.publish();
                return;
            }
            let done = change(s.store(), &avatar);
            s.version += 1;
            done.err().map(|e| e.to_string())
        };
        let failed = match refused {
            Some(e) => Some(e),
            None => self.states_write().err(),
        };
        if let Some(e) = failed {
            self.n4n0.lock().unwrap().states.say(e.clone(), true);
            self.notify(NoticeKind::Warning, e);
        }
        self.publish();
    }

    /// Saves how the avatar worn is now as a state named `name` ("State n" without one), on a thread of its own: VRChat's
    /// tree takes a moment to read.
    pub(crate) fn states_save(self: &Shared, name: Option<String>, options: SaveOptions) {
        self.n4n0.lock().unwrap().states.saving = true;
        self.publish();
        let n = self.clone();
        let _ = std::thread::Builder::new().name("n4n0-state-save".into()).spawn(move || {
            let said = n.states_save_now(name, options);
            {
                let mut rt = n.n4n0.lock().unwrap();
                rt.states.saving = false;
                match &said {
                    Ok(text) => rt.states.say(text.clone(), false),
                    Err(e) => rt.states.say(e.clone(), true),
                }
                // The screen's States page says what went wrong, in place of "Saved as…".
                if let (Err(e), orbiters_n4n0_core::menu::Page::States) = (&said, rt.screen.menu.page) {
                    rt.screen.menu.notice = Some(e.clone());
                }
            }
            if let Err(e) = said {
                n.notify(NoticeKind::Warning, e);
            }
            n.publish();
        });
    }

    fn states_save_now(self: &Shared, name: Option<String>, options: SaveOptions) -> Result<String, String> {
        let avatar = self.n4n0.lock().unwrap().avatar_id.clone().ok_or("VRChat's avatar is not known yet: change avatar (or rejoin), then save.")?;
        let nodes = self.vrchat_nodes().map_err(|e| format!("VRChat did not say the avatar's parameters ({e:#}): is OSC on in VRChat?"))?;
        let (current, eye, _) = split(nodes);
        if current.is_empty() {
            return Err("VRChat said no parameter of the avatar yet: wait for it to load, then save again.".into());
        }
        let parameters = states::capture(&current, &CAPTURE_PARAMETERS);
        let eye = eye.or_else(|| self.osc_latest_value(EYE_HEIGHT).as_ref().and_then(float));
        // Its name as set in its SDK (VRChat's OSCQuery), else as VRChat's log calls it.
        let sdk_name = self.n4n0.lock().unwrap().avatar_name.clone();
        let avatar_name = sdk_name.or_else(|| self.host.world().avatar_name);
        let count = parameters.len();
        let saved = {
            let mut rt = self.n4n0.lock().unwrap();
            if rt.avatar_id.as_deref() != Some(avatar.as_str()) {
                return Err("The avatar changed while it was being saved: save again.".into());
            }
            let s = &mut rt.states;
            s.store();
            if let Some(e) = &s.unreadable {
                return Err(format!("The saved states could not be read ({e}): nothing was saved, so they stay whole."));
            }
            let saved = s.store().add(&avatar, avatar_name.as_deref(), name.as_deref(), parameters, eye, orbiters_core::now_utc_iso());
            let saved = saved.map(|s| (s.id.clone(), s.name.clone())).map_err(|e| e.to_string());
            let (id, saved) = saved?;
            // What applying it puts back besides the parameters, and whether it is the avatar's default.
            if options.size || options.pose {
                s.store().set_options(&avatar, &id, options.pose, options.size).map_err(|e| e.to_string())?;
            }
            if options.default {
                s.store().set_default(&avatar, Some(&id)).map_err(|e| e.to_string())?;
            }
            s.version += 1;
            saved
        };
        self.states_write()?;
        Ok(format!("Saved \"{saved}\": {count} parameter{}", if count == 1 { "" } else { "s" }))
    }

    /// Puts the avatar worn back as its state `id` holds it (`by_default`: its default, after a join or a switch), on a
    /// thread of its own: what VRChat holds now is read first, then the messages go in steps.
    pub(crate) fn states_apply(self: &Shared, id: String, by_default: bool) {
        let n = self.clone();
        let _ = std::thread::Builder::new().name("n4n0-state-apply".into()).spawn(move || {
            let said = n.states_apply_now(&id);
            let quiet = by_default && said.is_ok();
            {
                let mut rt = n.n4n0.lock().unwrap();
                match &said {
                    Ok(text) => rt.states.say(if by_default { format!("{text} (the default)") } else { text.clone() }, false),
                    Err(e) => rt.states.say(e.clone(), true),
                }
                // The screen's States page says how it went.
                if rt.screen.menu.page == orbiters_n4n0_core::menu::Page::States {
                    if let Err(e) = &said {
                        rt.screen.menu.notice = Some(e.clone());
                    }
                }
            }
            if let (Err(e), false) = (&said, quiet) {
                n.notify(NoticeKind::Warning, e.clone());
            }
            n.publish();
        });
    }

    fn states_apply_now(self: &Shared, id: &str) -> Result<String, String> {
        let (avatar, state) = {
            let mut rt = self.n4n0.lock().unwrap();
            let avatar = rt.avatar_id.clone().ok_or("VRChat's avatar is not known yet.")?;
            let state = rt.states.store().state(&avatar, id).cloned().ok_or("That state is not saved for this avatar.")?;
            (avatar, state)
        };
        // What the avatar holds now: its types, what it lacks, what is as saved already. Not read: as saved.
        let (current, _, scaling) = self.vrchat_nodes().map(split).unwrap_or_default();
        let scaling = scaling.or_else(|| self.osc_latest_value(SCALING_ALLOWED).map(|v| v.truthy()));
        let plan = states::plan(&state, &current, scaling);
        {
            let mut rt = self.n4n0.lock().unwrap();
            if rt.avatar_id.as_deref() != Some(avatar.as_str()) {
                return Err("The avatar changed: the state was not applied.".into());
            }
            let now = Instant::now();
            for (k, step) in states::paced(plan.messages.clone()).into_iter().enumerate() {
                rt.send_at(now + APPLY_STEP * k as u32, step);
            }
        }
        Ok(plan.summary(&state.name))
    }

    /// What VRChat's log said (the app's watchers): a world joined as it happens (`live`) puts the default state back
    /// once the avatar loaded there.
    pub fn states_log(&self, events: &[LogEvent], live: bool) {
        if !live || !events.iter().any(|e| matches!(e, LogEvent::JoinedRoom { .. })) {
            return;
        }
        let mut rt = self.n4n0.lock().unwrap();
        // A reload under way says when, itself.
        if rt.states.due.is_none() {
            rt.states.joined = Some(Instant::now());
            rt.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> N4n0Runtime {
        let mut rt = N4n0Runtime::default();
        rt.avatar_id = Some("avtr_a".into());
        rt.params = ["Hoodie".to_string()].into_iter().collect();
        let id = rt.states.store().add("avtr_a", None, Some("Cosy"), BTreeMap::new(), None, "t".into()).unwrap().id.clone();
        rt.states.store().set_default("avtr_a", Some(&id)).unwrap();
        rt
    }

    #[test]
    fn the_default_goes_once_the_avatar_settled_and_its_parameters_are_known() {
        let mut rt = runtime();
        let t0 = Instant::now();
        rt.states.avatar_changed("avtr_a", t0);
        assert_eq!(tick(&mut rt, t0 + Duration::from_secs(1)), None, "VRChat applies its own first");
        assert_eq!(rt.states.next_due(), Some(t0 + DEFAULT_SETTLE));
        // Not read yet (a heavy avatar): it waits, a second at a time.
        rt.tree_unknown = true;
        assert_eq!(tick(&mut rt, t0 + DEFAULT_SETTLE), None);
        rt.tree_unknown = false;
        assert_eq!(tick(&mut rt, t0 + DEFAULT_SETTLE + Duration::from_secs(1)), Some(("avtr_a".into(), "s1".into())));
        assert_eq!(tick(&mut rt, t0 + Duration::from_secs(10)), None, "once");
        // A burst of changes counts from the newest; another avatar's change drops this one's.
        rt.states.avatar_changed("avtr_a", t0);
        rt.avatar_id = Some("avtr_b".into());
        assert_eq!(tick(&mut rt, t0 + DEFAULT_SETTLE), None, "avtr_b has no default, and avtr_a's change is gone");
        assert!(rt.states.next_due().is_none());
    }

    #[test]
    fn a_world_join_without_a_reload_puts_the_default_back_after_a_while_once() {
        let mut rt = runtime();
        let t0 = Instant::now();
        rt.states.joined = Some(t0);
        assert_eq!(tick(&mut rt, t0 + Duration::from_secs(5)), None, "VRChat reloads the avatar first, as a rule");
        assert_eq!(tick(&mut rt, t0 + JOIN_WAIT), Some(("avtr_a".into(), "s1".into())));
        // A reload came after all, then a late join line: the default is not put back twice.
        rt.states.avatar_changed("avtr_a", t0 + JOIN_WAIT);
        rt.states.joined = Some(t0 + JOIN_WAIT);
        assert_eq!(tick(&mut rt, t0 + JOIN_WAIT + DEFAULT_SETTLE), Some(("avtr_a".into(), "s1".into())), "the reload's");
        rt.states.joined = Some(t0 + JOIN_WAIT + Duration::from_secs(1));
        assert_eq!(tick(&mut rt, t0 + JOIN_WAIT * 2 + Duration::from_secs(2)), None, "put back moments ago");
        // No default: nothing goes.
        let mut plain = N4n0Runtime::default();
        plain.avatar_id = Some("avtr_none".into());
        plain.params = ["Hoodie".to_string()].into_iter().collect();
        plain.states.avatar_changed("avtr_none", t0);
        assert_eq!(tick(&mut plain, t0 + DEFAULT_SETTLE), None);
    }

    #[test]
    fn the_tree_splits_into_parameters_eye_height_and_scaling() {
        let node = |path: &str, value: OscValue, writable: bool| TreeNode { path: path.into(), value, writable };
        let (parameters, eye, scaling) = split(vec![
            node("/avatar/change", OscValue::Text("avtr_a".into()), true),
            node("/avatar/parameters/Hoodie", OscValue::Bool(true), true),
            node("/avatar/parameters/VRMode", OscValue::Int(1), false),
            node(EYE_HEIGHT, OscValue::Float(1.62), true),
            node(SCALING_ALLOWED, OscValue::Bool(false), false),
        ]);
        assert_eq!(parameters.keys().collect::<Vec<_>>(), vec!["Hoodie", "VRMode"]);
        assert!(!parameters["VRMode"].writable);
        assert_eq!((eye, scaling), (Some(1.62), Some(false)));
    }
}
