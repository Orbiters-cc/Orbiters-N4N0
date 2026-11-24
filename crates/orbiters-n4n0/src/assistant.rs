//! The assistant's intents in the runtime (REQUEST20 6, `orbiters_n4n0_core::intents`): each runs through the same path
//! the menus and pages use (N4N0's menu effects and commands here, the app's own controller through
//! [`crate::Host::assistant_intent`] for what is the app's), is said where the person looks (the Ask page's conversation
//! and actions, a toast, the press sound, the avatar's panel), logged, and undoable where it has an opposite.
//!
//! The offline path ([`orbiters_n4n0_core::intents::quick`]) runs here too: the most common short commands, from the
//! person's own sentence alone, at once and with no network.

use crate::fence::{Fence, Permit, Ticket};
use crate::runtime::Shared;
use crate::status::{N4n0Who, NoticeKind};
use orbiters_core::slice::Slice;
use orbiters_i18n::{tr, tr_latin};
use orbiters_n4n0_core::context::{self, NAME_CHARS};
use orbiters_n4n0_core::intents::{self, About, Facts, Gate, Intent, Limiter, LogEntry, Outcome, Page, Risk, Via, Which};
use orbiters_n4n0_core::menu::{Effect, PhotoMode};
use orbiters_n4n0_core::tools::Action;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;

/// Steps of undo kept.
const UNDO_KEPT: usize = 10;
/// Items of a list given to the model.
const LIST_ITEMS: usize = 12;

/// A page the assistant asked the interfaces to open (numbered: each interface opens each request once).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenAsk {
    pub id: u32,
    pub page: Page,
    /// For the Worlds sheet: the world to show (its id, and its name to show while it loads).
    #[serde(default)]
    pub world: Option<(String, String)>,
}

/// What the interfaces show of the assistant's actions (the Ask page).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantView {
    /// The action log, the newest last.
    pub log: Vec<LogEntry>,
    /// What is being done now, in the person's language.
    #[serde(default)]
    pub doing: Option<String>,
    #[serde(default)]
    pub open: Option<OpenAsk>,
    /// The last turn when it failed (r23): its Retry button, under its line.
    #[serde(default)]
    pub failure: Option<TurnFailure>,
}

/// A turn that failed, as the chat and the Ask page show it: the line it goes under, and its Retry button.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFailure {
    /// The failed turn's number: what Retry names ([`crate::Command::RetryTurn`]), a later one is refused.
    pub id: u64,
    /// The error line it goes with ([`crate::N4n0Line::at_ms`]).
    pub at_ms: u64,
    /// The message can be sent again (a busy or unreachable service; not a refusal of the request itself).
    pub retry: bool,
    /// Unix time in milliseconds before which it is too early to try again (the service said so); 0: now.
    pub retry_at_ms: u64,
    /// N4N0 sends it once by itself at that moment, unless something else is asked before.
    pub automatic: bool,
    /// What an administrator of Orbiters reads (the provider's failure), small; nobody else.
    #[serde(default)]
    pub admin_detail: Option<String>,
}

impl TurnFailure {
    /// Seconds until it may be sent again (0: now), at `now_ms` (Unix milliseconds).
    pub fn waits(&self, now_ms: u64) -> u64 {
        self.retry_at_ms.saturating_sub(now_ms).div_ceil(1000)
    }

    /// The Retry button's words at `now_ms`: the wait while it has one ("Retry in 25 s"), and that N4N0 sends it itself
    /// when it does.
    pub fn label(&self, now_ms: u64) -> String {
        let wait = match self.waits(now_ms) {
            0 => return tr("Retry").to_string(),
            s if s < 60 => tr!("{n} s", n = s),
            s if s < 3600 => tr!("{n} min", n = s.div_ceil(60)),
            s => tr!("{n} h", n = s.div_ceil(3600)),
        };
        if self.automatic {
            tr!("Trying again in {wait}", wait = wait)
        } else {
            tr!("Retry in {wait}", wait = wait)
        }
    }
}

/// A message whose turn failed, kept to be sent again ([`N4n0::retry_turn`]).
#[derive(Clone, Debug)]
pub(crate) struct Retrying {
    pub(crate) id: u64,
    pub(crate) text: String,
    pub(crate) via: &'static str,
    pub(crate) not_before: Instant,
    /// This message has had its one automatic retry.
    pub(crate) auto_used: bool,
}

/// What undoes an action.
#[derive(Clone, Debug)]
enum UndoStep {
    Action(Action),
    Effects(Vec<Effect>),
    /// The text the clipboard held.
    Clipboard(String),
}

/// What an intent is for, taken as the person is asked (or as it runs at once): done for that, or not at all.
#[derive(Clone, Debug, Default)]
pub(crate) struct Target {
    /// The item an index names.
    item: Option<Value>,
    video: Option<orbiters_n4n0_core::youtube::Video>,
    capture: Option<String>,
    recipients: Vec<orbiters_n4n0_core::captures::Friend>,
    /// The world the owner is in.
    world: Option<String>,
    /// The call under way, or ringing.
    call: Option<u64>,
    contact: Option<String>,
    state: Option<(String, String)>,
    /// The app's own ([`crate::Host::assistant_target`]).
    app: Value,
    /// What names it for the person, beside what is done: others' text, as data.
    pub(crate) says: Option<String>,
}

/// A share asked for the next capture ("take a 3D picture, then share it"): it goes once a newer capture is ready.
#[derive(Clone, Debug)]
struct ShareNext {
    /// The latest capture's key when it was asked (a newer one is the picture's).
    after: Option<String>,
    public: bool,
    recipients: Vec<orbiters_n4n0_core::captures::Friend>,
    asked: Instant,
    /// The identity it was asked for: nobody else's.
    epoch: u64,
}

/// The assistant's part of N4N0's state.
#[derive(Default)]
pub(crate) struct AssistantRuntime {
    pub(crate) limiter: Limiter,
    pub(crate) view: Slice<AssistantView>,
    /// What was done (English, for the model), and what undoes it.
    undo: Vec<(String, UndoStep)>,
    /// The last list of each kind a tool answered with, its items as given (`ref` names what an index acts on).
    pub(crate) lists: HashMap<&'static str, Vec<Value>>,
    youtube: Vec<orbiters_n4n0_core::youtube::Video>,
    /// The groups of the app's tools the last turn carried: the next one carries them too (r23).
    pub(crate) last_groups: std::collections::BTreeSet<orbiters_n4n0_core::intents::route::ToolGroup>,
    share_next: Option<ShareNext>,
    /// The last failed message, which Retry sends again once.
    pub(crate) retry: Option<Retrying>,
}

impl AssistantRuntime {
    /// Nothing is left to retry (something else was asked, or the conversation was started over).
    pub(crate) fn clear_failure(&mut self) {
        self.retry = None;
        self.view.failure = None;
    }

    /// The assistant is off: nothing it was to do later is done.
    pub(crate) fn disarm(&mut self) {
        self.share_next = None;
    }

    /// Forgets everything of the Orbiters account before (verify-r21 3): the log, what is being done, the page asked for,
    /// undo steps, the lists "the second one" points into, a share waiting for the next capture. The rate limits stay.
    /// The last page request's number stays (each interface opens a numbered request once: a new one must count on),
    /// without the world it named.
    pub(crate) fn forget(&mut self) {
        let limiter = std::mem::take(&mut self.limiter);
        let open = self.view.open.take().map(|o| OpenAsk { world: None, ..o });
        *self = Self { limiter, ..Self::default() };
        self.view.open = open;
    }
}

/// Which list an intent's results are, or its index counts in.
fn list_of(intent: &Intent) -> Option<&'static str> {
    Some(match intent {
        Intent::YoutubeSearch { .. } | Intent::YoutubeWatch { .. } | Intent::YoutubeCopy { .. } | Intent::YoutubeDrop { .. } => "youtube",
        Intent::VideoHistory | Intent::HistoryCopy { .. } | Intent::HistoryDrop { .. } => "video_history",
        Intent::WorldSearch { .. } | Intent::WorldPage { index: Some(_) } | Intent::GoToWorld { .. } | Intent::CopyWorldName { index: Some(_) } => "worlds",
        Intent::Places | Intent::Play { index: Some(_), .. } => "places",
        Intent::Instances | Intent::InviteMe { .. } | Intent::GoToInstance { .. } => "instances",
        Intent::Invites | Intent::AcceptInvite { .. } => "invites",
        Intent::Captures | Intent::ShareCapture { which: Which::Index(_), .. } => "captures",
        _ => return None,
    })
}

fn index_of(intent: &Intent) -> Option<usize> {
    match intent {
        Intent::YoutubeWatch { index }
        | Intent::YoutubeCopy { index }
        | Intent::YoutubeDrop { index }
        | Intent::HistoryCopy { index }
        | Intent::HistoryDrop { index }
        | Intent::InviteMe { index }
        | Intent::GoToInstance { index }
        | Intent::AcceptInvite { index }
        | Intent::WorldPage { index: Some(index) }
        | Intent::GoToWorld { index }
        | Intent::CopyWorldName { index: Some(index) }
        | Intent::Play { index: Some(index), .. }
        | Intent::ShareCapture { which: Which::Index(index), .. } => Some(*index),
        _ => None,
    }
}

/// Text of `item`'s `key`, if any.
fn field<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get(key).and_then(Value::as_str)
}

/// Names that say the same person, as the owner said them: case and accents aside, the whole name or its start.
fn name_matches(said: &str, name: &str) -> u8 {
    let fold = |t: &str| -> String {
        t.chars()
            .flat_map(|c| match orbiters_contracts::n4n0::unaccented(c) {
                Some(base) => base.to_lowercase().chars().collect::<Vec<_>>(),
                None => c.to_lowercase().collect(),
            })
            .filter(|c| c.is_alphanumeric())
            .collect()
    };
    let (said, name) = (fold(said), fold(name));
    if said.is_empty() {
        0
    } else if said == name {
        2
    } else if name.starts_with(&said) || name.contains(&said) {
        1
    } else {
        0
    }
}

/// The one of `candidates` (id, name) the owner named: exact first; several alike is a question for the owner.
pub(crate) fn pick_person<'a>(said: &str, candidates: impl Iterator<Item = (&'a str, &'a str)>) -> Result<(String, String), String> {
    let scored: Vec<(u8, &str, &str)> = candidates.map(|(id, name)| (name_matches(said, name), id, name)).filter(|(s, ..)| *s > 0).collect();
    let best = scored.iter().map(|s| s.0).max().ok_or_else(|| format!("Nobody called {} among the owner's friends here: ask who they meant.", context::quoted(said, NAME_CHARS)))?; // i18n: not shown
    let top: Vec<_> = scored.iter().filter(|s| s.0 == best).collect();
    if top.len() > 1 {
        let names: Vec<String> = top.iter().take(5).map(|s| context::quoted(s.2, NAME_CHARS)).collect();
        return Err(format!("Several friends match {}: {}. Ask the owner which one.", context::quoted(said, NAME_CHARS), names.join(", "))); // i18n: not shown
    }
    Ok((top[0].1.to_string(), top[0].2.to_string()))
}

impl crate::N4n0 {
    /// What the app can do now, for the intents' needs: the app's own ([`crate::Host::assistant_facts`]) and N4N0's.
    pub(crate) fn assistant_facts(&self) -> Facts {
        let mut facts = self.host.assistant_facts();
        let rt = self.n4n0.lock().unwrap();
        facts.n4n0_prop |= rt.sender.is_some() || rt.screen_present;
        facts.can_drop |= rt.screen.card_ready();
        facts
    }

    /// A newer capture is ready: the share asked for "the next one" goes now (once), while the member who asked it is
    /// signed in.
    pub(crate) fn share_next_check(self: &Shared) {
        let Some(epoch) = self.n4n0.lock().unwrap().assistant.share_next.as_ref().map(|n| n.epoch) else { return };
        let Some(fence) = self.fence(&Ticket::input(epoch)) else { return };
        // The switch, and the assistant, as they are now (verify4-r21 P11): off, it never goes.
        let share = Action::App(Intent::ShareCapture { which: Which::Next, public: true, friends: Vec::new() });
        if fence.permit_yes(&share).is_none() || !fence.with_settings(|s| s.n4n0.enabled && s.n4n0.assistant) {
            fence.cancel_share_next();
            return;
        }
        let ask = {
            let mut rt = fence.n4n0.lock().unwrap();
            let Some(next) = rt.assistant.share_next.as_ref().filter(|n| n.epoch == epoch) else { return };
            if next.asked.elapsed() > std::time::Duration::from_secs(30 * 60) {
                rt.assistant.share_next = None;
                return;
            }
            let latest = rt.captures.latest.clone();
            match latest {
                Some(row) if Some(&row.key) != next.after.as_ref() && matches!(row.phase, orbiters_n4n0_core::captures::CapturePhase::Ready) => {
                    let next = rt.assistant.share_next.take().unwrap();
                    orbiters_n4n0_core::captures::ShareAsk { key: row.key, public: next.public, recipients: next.recipients, acknowledge_explicit: false }
                }
                _ => return,
            }
        };
        fence.share_capture(ask);
        fence.add_line(N4n0Who::Note, tr("N4N0 shares the 3D capture you just took, as you said."));
        fence.publish();
    }

    /// A share asked for the next capture is not made any more.
    pub(crate) fn cancel_share_next(&self) {
        if self.n4n0.lock().unwrap().assistant.share_next.take().is_some() {
            self.add_line(N4n0Who::Note, tr("The next 3D capture is not shared after all: what N4N0 does from your account is off."));
            self.publish();
        }
    }

    /// What N4N0 may do from the account changed ([`crate::Host::assistant_facts`]): off, what was asked or armed of it goes at
    /// once.
    pub fn account_actions_changed(self: &Shared) {
        if self.assistant_facts().actions {
            return;
        }
        self.cancel_share_next();
        let dropped = {
            let mut rt = self.n4n0.lock().unwrap();
            rt.drop_account_question()
        };
        if dropped {
            self.publish();
        }
    }
}

/// The assistant's actions: each is done for an input let in, and only a fence does one.
impl Fence<'_> {
    /// One line of the action log, published.
    pub(crate) fn intent_log(&self, intent: &Intent, outcome: Outcome, why: Option<String>, via: Via) {
        self.log_line(intent.name(), intent.says(), intent.risk(), outcome, why, via);
    }

    pub(crate) fn log_line(&self, name: &str, said: String, risk: Risk, outcome: Outcome, why: Option<String>, via: Via) {
        let mut rt = self.n4n0.lock().unwrap();
        let view = &mut *rt.assistant.view;
        view.log.push(LogEntry { at_ms: crate::unix_ms(), intent: name.to_string(), said, risk, outcome, why, via });
        let excess = view.log.len().saturating_sub(intents::LOG_KEPT);
        view.log.drain(..excess);
    }

    /// What is being done now (the Ask page's line under the conversation), or nothing.
    pub(crate) fn intent_doing(&self, doing: Option<String>) {
        let mut rt = self.n4n0.lock().unwrap();
        if rt.assistant.view.doing != doing {
            rt.assistant.view.doing = doing;
        }
    }

    /// Runs `intent` (its gate said so, or the owner said yes): counted against the rate limits, said, logged, its undo
    /// kept. The result is for the model.
    pub(crate) fn intent_run(&self, permit: &Permit, intent: Intent, via: Via) -> Result<Value, String> {
        let target = self.resolve(&intent)?;
        self.intent_run_for(permit, intent, via, &target)
    }

    /// [`Fence::intent_run`] for `target`, as it was asked about.
    pub(crate) fn intent_run_for(&self, permit: &Permit, intent: Intent, via: Via, target: &Target) -> Result<Value, String> {
        let risk = intent.risk();
        let limited = self.n4n0.lock().unwrap().assistant.limiter.take(if risk == Risk::Confirm { Risk::Confirm } else { Risk::Act }, Instant::now());
        if let Err(why) = limited {
            self.intent_log(&intent, Outcome::Refused, Some(tr("Too many actions in a short time: wait a moment.").into()), via);
            self.publish();
            return Err(why);
        }
        self.intent_doing(Some(intent.says()));
        self.publish();
        let outcome = self.intent_do(permit, &intent, target);
        self.intent_doing(None);
        match outcome {
            Ok((value, undo)) => {
                if let Some(step) = undo {
                    self.keep_undo(intent.what(), step);
                }
                self.intent_log(&intent, Outcome::Done, None, via);
                // Every change shows and sounds where the person is (a toast on the desktop, the press in VR).
                if intent.spec().changes {
                    self.device.sounds.press();
                    self.notify(NoticeKind::Success, tr!("N4N0: {what}", what = intent.says()));
                }
                self.publish();
                Ok(value)
            }
            Err(e) => {
                self.intent_log(&intent, Outcome::Failed, Some(e.clone()), via);
                self.publish();
                Err(e)
            }
        }
    }

    fn keep_undo(&self, what: String, step: UndoStep) {
        let mut rt = self.n4n0.lock().unwrap();
        rt.assistant.undo.push((what, step));
        let excess = rt.assistant.undo.len().saturating_sub(UNDO_KEPT);
        rt.assistant.undo.drain(..excess);
    }

    /// The clipboard's text as it is now is what Undo puts back after `what` (nothing readable: nothing to undo).
    pub(crate) fn keep_clipboard(&self, what: String) {
        if let Some(text) = self.device.clipboard.paste() {
            self.keep_undo(what, UndoStep::Clipboard(text));
        }
    }

    /// Who and what `intent` is for, now (verify4-r21, binding what a yes acts on): its list's item, the newest capture, the
    /// friends its names mean, the world, the call, the VRChat account. An error says what is missing, for the model.
    pub(crate) fn resolve(&self, intent: &Intent) -> Result<Target, String> {
        let mut t = Target { item: self.item(intent)?, ..Target::default() };
        let name = |text: &str| Some(context::quoted(text, NAME_CHARS)).filter(|_| !text.trim().is_empty());
        t.says = t.item.as_ref().and_then(|i| ["name", "title", "world", "from"].into_iter().find_map(|k| field(i, k)).and_then(name));
        let call = || self.host.calls().call.ok_or_else(|| "No call now.".to_string()); // i18n: not shown
        match intent {
            Intent::YoutubeWatch { index } | Intent::YoutubeCopy { index } | Intent::YoutubeDrop { index } => {
                let video = self.n4n0.lock().unwrap().assistant.youtube.get(index - 1).cloned().ok_or("No such result: search again.")?; // i18n: not shown
                t.says = name(&video.title);
                t.video = Some(video);
            }
            Intent::ShareCapture { which, friends, .. } => {
                t.recipients = if friends.is_empty() { Vec::new() } else { self.recipients(friends)? };
                let view = (*self.n4n0.lock().unwrap().captures).clone();
                match which {
                    Which::Next => {}
                    Which::Latest => {
                        let row = view.latest.as_ref().or(view.history.first()).ok_or("No 3D capture yet.")?; // i18n: not shown
                        (t.capture, t.says) = (Some(row.key.clone()), name(&row.world));
                    }
                    Which::Index(_) => {
                        t.capture = Some(t.item.as_ref().and_then(|i| field(i, "ref")).map(str::to_string).ok_or("No such capture in the last list.")?); // i18n: not shown
                    }
                }
                // The friends their names mean, in full.
                let to: Vec<String> = t.recipients.iter().filter_map(|f| name(&f.name)).collect();
                t.says = Some(t.says.into_iter().chain(to).collect::<Vec<_>>().join(", ")).filter(|s| !s.is_empty());
            }
            Intent::WorldLike { .. } | Intent::WorldComment { .. } | Intent::WorldLabel { .. } | Intent::WorldFeedback => {
                t.world = Some(self.here()?);
                t.says = self.host.world().world_name.as_deref().and_then(name);
            }
            // Sent from the VRChat account connected now, or not at all.
            Intent::InviteMe { .. } => t.app = json!({ "vrchat": self.host.invite_account() }),
            Intent::Call { friend } => {
                let calls = self.host.calls();
                let (id, full) = pick_person(friend, calls.contacts.iter().map(|c| (c.id.as_str(), c.name.as_str())))?;
                (t.contact, t.says) = (Some(id), name(&full));
            }
            Intent::AnswerCall | Intent::EndCall | Intent::MuteCall { .. } => {
                let call = call()?;
                (t.call, t.says) = (Some(call.number), name(&call.name));
            }
            Intent::ApplyState { name: said } => {
                let rt = self.n4n0.lock().unwrap();
                let view = rt.states.view(rt.avatar_id.as_deref(), None);
                let names: Vec<String> = view.states.iter().map(|s| s.name.clone()).collect();
                let state = pick_person(said, view.states.iter().map(|s| (s.id.as_str(), s.name.as_str())))
                    .map_err(|_| format!("No saved state called {} on this avatar; its states: {}.", context::quoted(said, NAME_CHARS), names.iter().map(|n| context::quoted(n, NAME_CHARS)).collect::<Vec<_>>().join(", ")))?; // i18n: not shown
                t.says = name(&state.1);
                t.state = Some(state);
            }
            _ => {
                t.app = self.host.assistant_target(intent, t.item.as_ref())?;
                if let Some(says) = t.app.get("says").and_then(Value::as_str) {
                    t.says = name(says);
                }
            }
        }
        Ok(t)
    }

    /// The item an index names in its list, as the last answer gave it.
    fn item(&self, intent: &Intent) -> Result<Option<Value>, String> {
        let (Some(kind), Some(i)) = (list_of(intent), index_of(intent)) else { return Ok(None) };
        let rt = self.n4n0.lock().unwrap();
        let list = rt.assistant.lists.get(kind).ok_or_else(|| format!("No {kind} list yet: read it first (its query intent), then use its numbers."))?; // i18n: not shown
        list.get(i - 1).cloned().map(Some).ok_or_else(|| format!("The last {kind} list has {} items: ask the owner which one.", list.len())) // i18n: not shown
    }

    /// Keeps `items` as the last list of `intent`'s kind, numbered for the model.
    fn keep_list(&self, intent: &Intent, items: Vec<Value>) -> Vec<Value> {
        let items: Vec<Value> = items.into_iter().take(LIST_ITEMS).enumerate().map(|(i, mut v)| {
            if let Value::Object(map) = &mut v {
                map.insert("number".into(), json!(i + 1));
            }
            v
        }).collect();
        if let Some(kind) = list_of(intent) {
            self.n4n0.lock().unwrap().assistant.lists.insert(kind, items.clone());
        }
        items
    }

    /// Does `intent`, by the same path its menu or page takes. Its result for the model, and what undoes it.
    fn intent_do(&self, permit: &Permit, intent: &Intent, target: &Target) -> Result<(Value, Option<UndoStep>), String> {
        let done = || json!({ "done": true });
        let settings = self.settings();
        let item = target.item.clone();
        Ok(match intent {
            Intent::FastWalk { on } => {
                let was = settings.walk_boost.enabled;
                self.screen_effects(vec![Effect::FastWalk(*on)]);
                let desktop = self.n4n0.lock().unwrap().screen.menu.desktop_mode;
                let note = (desktop && *on).then_some("VRChat runs in desktop mode: Fast walk carries the owner in VR only."); // i18n: not shown
                (json!({ "done": true, "fast_walk": on, "note": note }), (was != *on).then(|| UndoStep::Effects(vec![Effect::FastWalk(was)])))
            }
            Intent::PictureLength { length } => {
                let timer = settings.photo_3d_timer;
                let mode = crate::screen::photo_mode(settings.photo_3d_mode);
                let before = crate::screen::photo_length(settings.photo_3d);
                self.screen_effects(vec![Effect::PhotoChoice { timer, length: *length, mode }]);
                (json!({ "done": true, "seconds": length.seconds() }), Some(UndoStep::Effects(vec![Effect::PhotoChoice { timer, length: before, mode }])))
            }
            Intent::Picture3d { of, seconds } => {
                if self.host.capturing() {
                    return Err("VRChat's camera is recording right now: stop the recording first.".into()); // i18n: not shown
                }
                let mode = of.unwrap_or_else(|| crate::screen::photo_mode(settings.photo_3d_mode));
                let length = crate::screen::photo_length(settings.photo_3d);
                // As the menus beside VRChat's: with the camera closed, the picture waits for it to be opened.
                if self.host.camera_open() == Some(false) {
                    self.screen_effects(vec![Effect::PhotoChoice { timer: *seconds, length, mode }]);
                    self.photo_3d_when_camera(*seconds);
                    (json!({ "waiting_for_camera": true, "note": "VRChat's camera is closed: the 3D picture is taken once the owner opens it (OSC cannot open it)." }), None) // i18n: not shown
                } else {
                    self.screen_effects(vec![Effect::Photo3d { timer: *seconds, length, mode }]);
                    (json!({ "started": true, "in_seconds": seconds, "of": if mode == PhotoMode::World { "world" } else { "people" } }), None)
                }
            }
            Intent::Captures => {
                let view = (*self.n4n0.lock().unwrap().captures).clone();
                if !view.listed {
                    self.captures_list();
                }
                let mut rows: Vec<&orbiters_n4n0_core::captures::CaptureRow> = view.latest.iter().collect();
                rows.extend(view.history.iter().filter(|r| view.latest.as_ref().is_none_or(|l| l.key != r.key)));
                let items: Vec<Value> = rows
                    .iter()
                    .map(|r| json!({ "ref": r.key, "world": context::data(&r.world, NAME_CHARS), "taken_unix": r.taken, "ready": matches!(r.phase, orbiters_n4n0_core::captures::CapturePhase::Ready), "public": r.public }))
                    .collect();
                let items = self.keep_list(intent, items);
                (intents::untrusted(json!({ "captures": items, "listed": view.listed })), None)
            }
            Intent::ShareCapture { which, public, .. } => {
                let recipients = target.recipients.clone();
                if *which == Which::Next {
                    let mut rt = self.n4n0.lock().unwrap();
                    let after = rt.captures.latest.as_ref().map(|l| l.key.clone());
                    rt.assistant.share_next = Some(ShareNext { after, public: *public, recipients, asked: Instant::now(), epoch: self.epoch() });
                    return Ok((json!({ "done": true, "note": "It is shared once the next 3D capture is ready (its 3D view made)." }), None)); // i18n: not shown
                }
                let key = target.capture.clone().ok_or("No such capture in the last list.")?; // i18n: not shown
                self.share_capture(orbiters_n4n0_core::captures::ShareAsk { key, public: *public, recipients, acknowledge_explicit: false });
                (json!({ "done": true, "note": "Sharing: it may take a minute; Photos & captures shows how it goes." }), None) // i18n: not shown
            }
            Intent::YoutubeSearch { query } => {
                let language = self.device.locale.language();
                let results = orbiters_n4n0_core::youtube::search(query, &language).map_err(|e| format!("YouTube could not be searched: {e}"))?; // i18n: not shown
                let videos: Vec<_> = results.videos.into_iter().take(LIST_ITEMS).collect();
                let items: Vec<Value> = videos
                    .iter()
                    .map(|v| json!({ "title": context::data(&v.title, NAME_CHARS), "channel": context::data(&v.channel, NAME_CHARS), "duration": v.duration, "live": v.live }))
                    .collect();
                self.n4n0.lock().unwrap().assistant.youtube = videos;
                let items = self.keep_list(intent, items);
                (intents::untrusted(json!({ "results": items })), None)
            }
            Intent::YoutubeWatch { .. } | Intent::YoutubeCopy { .. } | Intent::YoutubeDrop { .. } => {
                let video = target.video.clone().ok_or("No such result: search again.")?; // i18n: not shown
                let (link, title) = (video.link(), video.title.clone());
                let effect = match intent {
                    Intent::YoutubeWatch { .. } => Effect::WatchVideo { link },
                    Intent::YoutubeCopy { .. } => Effect::Copy { link, title },
                    _ => Effect::Drop { link, title, video: Some(video.id.clone()) },
                };
                let undo = self.undo_copy(intent);
                self.screen_effects(vec![effect]);
                (done(), undo)
            }
            Intent::HistoryCopy { .. } | Intent::HistoryDrop { .. } => {
                let item = item.ok_or("No such video in the last list.")?; // i18n: not shown
                let link = field(&item, "ref").ok_or("No link for that video.")?.to_string(); // i18n: not shown
                let title = field(&item, "title").unwrap_or_default().to_string();
                let video = orbiters_n4n0_core::videos::youtube_id(&link);
                let effect = if matches!(intent, Intent::HistoryCopy { .. }) { Effect::Copy { link, title } } else { Effect::Drop { link, title, video } };
                let undo = self.undo_copy(intent);
                self.screen_effects(vec![effect]);
                (done(), undo)
            }
            Intent::TakeBackCard => {
                self.screen_effects(vec![Effect::TakeBack]);
                (done(), None)
            }
            Intent::WhereAmI => {
                let world = self.host.world();
                let kind = world.instance.as_deref().map(|i| orbiters_n4n0_core::menu::InstanceRow { location: format!("w:{i}"), world: String::new(), at: 0, account: None }.kind());
                (
                    intents::untrusted(json!({
                        "in_world": world.in_world,
                        "world": world.world_name.as_deref().map(|n| context::data(n, NAME_CHARS)),
                        "instance_type": kind,
                        "people_besides_owner": world.others(),
                    })),
                    None,
                )
            }
            Intent::WorldPage { index } => {
                let world = match index {
                    Some(_) => {
                        let item = item.ok_or("No such world in the last search.")?; // i18n: not shown
                        (field(&item, "ref").unwrap_or_default().to_string(), field(&item, "name").unwrap_or_default().to_string())
                    }
                    None => {
                        let w = self.host.world();
                        (w.world_id.ok_or("The owner is in no world now.")?, w.world_name.unwrap_or_default()) // i18n: not shown
                    }
                };
                self.open_page(Page::Worlds, Some(world));
                (done(), None)
            }
            // The worlds sheet's Copy name: the same clipboard as the menus' copies.
            Intent::CopyWorldName { index } => {
                let name = match index {
                    Some(_) => item.as_ref().and_then(|i| field(i, "name")).map(str::to_string),
                    None => self.host.world().world_name,
                };
                let name = name.filter(|n| !n.trim().is_empty()).ok_or("No world name to copy.")?; // i18n: not shown
                let undo = self.undo_copy(intent);
                self.screen_effects(vec![Effect::CopyText { text: name }]);
                (done(), undo)
            }
            Intent::WorldLike { on } => {
                let world = target.world.clone().ok_or("The owner is in no world now.")?; // i18n: not shown
                self.feedback_ask(orbiters_n4n0_core::feedback::FeedbackAsk::Like { world, on: *on });
                (done(), None)
            }
            Intent::WorldComment { text } => {
                let world = target.world.clone().ok_or("The owner is in no world now.")?; // i18n: not shown
                self.feedback_ask(orbiters_n4n0_core::feedback::FeedbackAsk::Comment { world, text: text.clone() });
                (done(), None)
            }
            Intent::WorldLabel { label, on } => {
                let world = target.world.clone().ok_or("The owner is in no world now.")?; // i18n: not shown
                let found = orbiters_contracts::world_feedback::labels()
                    .iter()
                    .find(|l| l.id.eq_ignore_ascii_case(label) || l.text.values().any(|t| name_matches(label, t) == 2))
                    .ok_or_else(|| {
                        let known: Vec<&str> = orbiters_contracts::world_feedback::labels().iter().map(|l| l.name("en")).collect();
                        format!("No label {}: the labels are {}.", context::quoted(label, NAME_CHARS), known.join(", ")) // i18n: not shown
                    })?;
                if *on && found.justify.is_some() {
                    return Err("That label needs a few words of why: the owner gives it on the world's feedback page (open_page worlds).".into()); // i18n: not shown
                }
                self.feedback_ask(orbiters_n4n0_core::feedback::FeedbackAsk::Label { world, label: found.id.clone(), on: *on, text: None });
                (done(), None)
            }
            Intent::WorldFeedback => {
                let world = target.world.clone().ok_or("The owner is in no world now.")?; // i18n: not shown
                self.feedback_ask(orbiters_n4n0_core::feedback::FeedbackAsk::Show { world: world.clone() });
                let rt = self.n4n0.lock().unwrap();
                let f = rt.feedback.world(&world);
                let value = match f {
                    Some(f) if !f.loading => json!({
                        "likes": f.likes,
                        "liked_by_owner": f.liked,
                        "labels": f.top().iter().map(|l| json!({ "label": orbiters_contracts::world_feedback::label(&l.id).map(|x| x.name("en")).unwrap_or(&l.id), "count": l.count, "mine": l.mine })).collect::<Vec<_>>(),
                        "comments": f.comments.iter().take(5).map(|c| json!({ "text": context::data(&c.text, 200), "mine": c.mine })).collect::<Vec<_>>(),
                        "comment_count": f.comment_count,
                    }),
                    _ => json!({ "loading": true, "note": "Being read from Orbiters: ask again in a moment." }), // i18n: not shown
                };
                (intents::untrusted(value), None)
            }
            Intent::InviteMe { .. } => {
                let item = item.ok_or("No such instance in the last list.")?; // i18n: not shown
                let location = field(&item, "ref").ok_or("No location for that instance.")?.to_string(); // i18n: not shown
                if target.app.get("vrchat").and_then(Value::as_str) != self.host.invite_account().as_deref() {
                    return Err(tr("Your VRChat account changed meanwhile: nothing was sent.").into());
                }
                self.invite_send(location, false);
                (json!({ "done": true, "note": "The invite's answer shows on Previous instances." }), None) // i18n: not shown
            }
            Intent::Call { .. } => {
                self.host.call(crate::CallRequest::Contact(target.contact.clone().ok_or("Nobody to call.")?)); // i18n: not shown
                (done(), None)
            }
            // The call it was asked about, or none. (Nothing undoes them: unmuting sends what is said next.)
            Intent::AnswerCall | Intent::EndCall | Intent::MuteCall { .. } => {
                let call = target.call.ok_or("No call now.")?; // i18n: not shown
                self.host.call(match intent {
                    Intent::AnswerCall => crate::CallRequest::Answer(call),
                    Intent::MuteCall { on } => crate::CallRequest::Mute { call, on: *on },
                    _ => crate::CallRequest::End(call),
                });
                (done(), None)
            }
            Intent::ShowN4n0 { on } => {
                let (was, avatar) = {
                    let rt = self.n4n0.lock().unwrap();
                    (rt.bot, rt.avatar_id.clone())
                };
                if was.is_none() {
                    return Err("This avatar's N4N0 has no Show N4N0 switch (or VRChat has not said yet).".into()); // i18n: not shown
                }
                self.command(crate::Command::ShowBot { on: *on, avatar });
                (done(), was.filter(|w| w != on).map(|w| UndoStep::Action(Action::App(Intent::ShowN4n0 { on: w }))))
            }
            Intent::N4n0Menu { open } => {
                let is_open = {
                    let rt = self.n4n0.lock().unwrap();
                    rt.screen.menu.open
                };
                // Opened as touching N4N0 opens it; closed as its own toggle closes it.
                if *open || is_open {
                    self.menu_open(!*open);
                }
                (done(), None)
            }
            Intent::SaveState { name } => {
                self.screen_effects(vec![Effect::SaveState { name: name.clone(), size: true, pose: false, default: false }]);
                (done(), None)
            }
            Intent::ApplyState { .. } => {
                let state = target.state.clone().ok_or("No such saved state.")?; // i18n: not shown
                self.screen_effects(vec![Effect::ApplyState { id: state.0, name: state.1 }]);
                (done(), None)
            }
            Intent::Smoothing { on, strength } => {
                let (was_on, was_ms) = (settings.tracking.enabled, settings.tracking.ms);
                let ms = match strength {
                    Some(s) => [(s * orbiters_core::settings::TrackingSettings::MAX_MS as f32).round() as u16; 3],
                    None => was_ms,
                };
                let on = on.unwrap_or(true);
                self.screen_effects(vec![Effect::Tracking { on, ms }]);
                (json!({ "done": true, "on": on, "ms": ms }), Some(UndoStep::Effects(vec![Effect::Tracking { on: was_on, ms: was_ms }])))
            }
            Intent::StayHere { on } => {
                let was = self.n4n0.lock().unwrap().screen.menu.pinned;
                self.screen_effects(vec![Effect::StayHere(*on)]);
                (done(), (was != *on).then(|| UndoStep::Effects(vec![Effect::StayHere(was)])))
            }
            Intent::AssistantOff => {
                // Off at once, whatever was under way, this answer too: only the owner turns it on again.
                let later = self.later();
                std::thread::spawn(move || {
                    if let Some(fence) = later.fence() {
                        fence.screen_effects(vec![Effect::Assistant(false)]);
                    }
                });
                (done(), None)
            }
            Intent::Suggest { text } => {
                self.suggestion_send(text.clone(), true, false);
                (done(), None)
            }
            Intent::OpenPage { page } => {
                self.open_page(*page, None);
                if page.on_window() {
                    // The window's pages through the app, which brings the window up on it.
                    self.host.assistant_intent(intent, None, &Value::Null)?;
                }
                (done(), None)
            }
            Intent::Restricted { what } => {
                // Never done: only its page opens, after the owner's yes.
                self.open_page(what.page(), None);
                if what.page().on_window() {
                    self.host.assistant_intent(&Intent::OpenPage { page: what.page() }, None, &Value::Null)?;
                }
                (json!({ "opened": what.page().key(), "note": "Only the page opened: the owner does this there." }), None) // i18n: not shown
            }
            Intent::State { about } => (intents::untrusted(self.state_answer(about)), None),
            Intent::Undo => {
                let last = self.n4n0.lock().unwrap().assistant.undo.pop();
                let (undone, step) = last.ok_or("Nothing to undo.")?; // i18n: not shown
                match step {
                    UndoStep::Effects(effects) => self.screen_effects(effects),
                    UndoStep::Clipboard(text) => self.device.clipboard.copy(&text)?,
                    // What undoes is judged as itself: one that waits for a yes is not done as an undo.
                    UndoStep::Action(action) if action.effect().asks || action.effect().from_account => {
                        return Err("That is not undone by voice: the owner does it on its page.".into()); // i18n: not shown
                    }
                    UndoStep::Action(Action::App(i)) => {
                        let target = self.resolve(&i)?;
                        self.intent_do(permit, &i, &target)?;
                    }
                    UndoStep::Action(action) => {
                        self.run_action_for_assistant(permit, action)?;
                    }
                }
                (json!({ "undone": undone }), None)
            }
            // The app's own: through its controller, as its pages do. What it read is kept as the list of its kind.
            _ => {
                let undo = self.host_undo(intent, &settings);
                let value = self.host.assistant_intent(intent, item.as_ref(), &target.app)?;
                let value = match value.get("items").and_then(Value::as_array) {
                    Some(items) => {
                        let items = self.keep_list(intent, items.clone());
                        let mut v = value.clone();
                        v["items"] = Value::Array(items);
                        v
                    }
                    None => value,
                };
                (if intent.spec().reads_others { intents::untrusted(value) } else { value }, undo)
            }
        })
    }

    /// What undoes a copy: the text the clipboard holds before it.
    fn undo_copy(&self, intent: &Intent) -> Option<UndoStep> {
        intent.copies().then(|| self.device.clipboard.paste()).flatten().map(UndoStep::Clipboard)
    }

    /// What undoes an app intent, from the settings before it.
    fn host_undo(&self, intent: &Intent, s: &orbiters_core::settings::Settings) -> Option<UndoStep> {
        let app = |i: Intent| Some(UndoStep::Action(Action::App(i)));
        match intent {
            Intent::WalkBoost { .. } => app(Intent::WalkBoost { strength: Some(s.walk_boost.strength), step: 0 }),
            Intent::Translation { on } if *on != s.translate.enabled => app(Intent::Translation { on: s.translate.enabled }),
            Intent::TranslateInto { .. } => s.translate.target.clone().and_then(|t| app(Intent::TranslateInto { language: t })),
            Intent::CameraQr { on } => app(Intent::CameraQr { on: !on }),
            Intent::Setting { key: intents::SettingKey::MenuTheme, .. } => {
                let was = if s.orbit.theme == orbiters_core::settings::MenuLook::Retro { "retro" } else { "modern" };
                app(Intent::Setting { key: intents::SettingKey::MenuTheme, value: was.into() })
            }
            Intent::Setting { key: intents::SettingKey::ReduceMotion, .. } => {
                app(Intent::Setting { key: intents::SettingKey::ReduceMotion, value: if s.reduce_motion { "on" } else { "off" }.into() })
            }
            _ => None,
        }
    }

    /// The world the owner is in, for its feedback.
    fn here(&self) -> Result<String, String> {
        let w = self.host.world();
        w.world_id.filter(|_| w.in_world).ok_or_else(|| "The owner is in no world now.".into()) // i18n: not shown
    }

    /// The VRChat friends the owner named, for a share.
    fn recipients(&self, said: &[String]) -> Result<Vec<orbiters_n4n0_core::captures::Friend>, String> {
        let friends = self.host.friends()?;
        said.iter()
            .map(|s| pick_person(s, friends.iter().map(|f| (f.id.as_str(), f.name.as_str()))).map(|(id, name)| orbiters_n4n0_core::captures::Friend { id, name }))
            .collect()
    }

    /// The page where the person does it themselves opens (the account's actions are off for them): an action like any
    /// other, with its leave, its line in the log and its count.
    pub(crate) fn open_instead(&self, page: Page, tainted: bool, via: Via) {
        let open = Intent::OpenPage { page };
        if let Some(permit) = self.permit(&Action::App(open.clone()), tainted) {
            let _ = self.intent_run(&permit, open, via);
        }
    }

    /// Asks the interfaces to open `page` (each opens a request once); the window's pages through the app as well.
    pub(crate) fn open_page(&self, page: Page, world: Option<(String, String)>) {
        let mut rt = self.n4n0.lock().unwrap();
        let id = rt.assistant.view.open.as_ref().map_or(1, |o| o.id.wrapping_add(1));
        rt.assistant.view.open = Some(OpenAsk { id, page, world });
        drop(rt);
        self.publish();
    }

    /// How things are, for a state question: N4N0's own and the app's ([`crate::Host::assistant_state`]).
    fn state_answer(&self, about: &[About]) -> Value {
        let all = about.contains(&About::Everything);
        let wants = |a: About| all || about.contains(&a);
        let app = self.host.assistant_state();
        let mut out = serde_json::Map::new();
        let s = self.settings();
        if wants(About::FastWalk) {
            out.insert("fast_walk".into(), json!({ "on": s.walk_boost.enabled, "strength": s.walk_boost.strength }));
        }
        if wants(About::Translation) {
            out.insert("translation".into(), json!({ "on": s.translate.enabled, "into": s.translate.target }));
        }
        if wants(About::Tracking) {
            out.insert("tracking_smoothing".into(), json!({ "on": s.tracking.enabled, "ms": s.tracking.ms }));
        }
        if wants(About::Pictures) {
            out.insert("pictures".into(), json!({ "timer_seconds": s.photo_3d_timer, "length_seconds": crate::screen::photo_length(s.photo_3d).seconds(), "of": if s.photo_3d_mode == orbiters_core::settings::PictureMode::World { "world" } else { "people" } }));
        }
        {
            let rt = self.n4n0.lock().unwrap();
            if wants(About::Light) {
                out.insert("light".into(), rt.light.map_or(json!({ "available": false }), |l| json!({ "on": l.on, "color": orbiters_contracts::n4n0::LIGHT_COLORS.get(l.color as usize).map(|c| c.0.to_lowercase()), "brightness": l.level })));
            }
            if wants(About::N4n0) {
                out.insert("n4n0".into(), json!({ "on_avatar": rt.sender.is_some() || rt.screen_present, "shown": rt.bot, "menu_open": rt.screen.menu.open, "afk": rt.afk }));
            }
            if wants(About::Assistant) {
                out.insert("assistant".into(), json!({ "quota_used": rt.quota.as_ref().map(|q| q.used), "quota_limit": rt.quota.as_ref().map(|q| q.limit), "own_key": rt.quota.as_ref().map(|q| q.own_key) }));
            }
        }
        if wants(About::Music) {
            let (_, reading) = self.device.media.reading();
            out.insert(
                "music".into(),
                json!({
                    "playing": reading.media.as_ref().filter(|m| !m.title.is_empty()).map(|m| json!({ "title": context::data(&m.title, NAME_CHARS), "artist": context::data(&m.artist, NAME_CHARS), "playing": m.playing })),
                    "volume_percent": reading.volume.map(|v| v.percent),
                }),
            );
        }
        if wants(About::World) {
            let w = self.host.world();
            out.insert("world".into(), json!({ "in_world": w.in_world, "name": w.world_name.as_deref().map(|n| context::data(n, NAME_CHARS)), "people_besides_owner": w.others() }));
        }
        if wants(About::Calls) {
            let calls = self.host.calls();
            out.insert("calls".into(), json!({ "in_call": calls.call.as_ref().map(|c| context::data(&c.name, NAME_CHARS)), "can_call": calls.contacts.len() }));
        }
        if wants(About::Version) {
            out.insert("version".into(), json!(self.host.app_version()));
        }
        // The app's own (explorer points, friends, recording, fast walk's carrying…), as it gives them.
        if let Value::Object(map) = app {
            for (k, v) in map {
                let a = About::ALL.into_iter().find(|a| a.key() == k);
                if a.is_none_or(&wants) {
                    out.insert(k, v);
                }
            }
        }
        Value::Object(out)
    }

    /// Whether a copy could be undone, when `intent` is one (the clipboard is read for nothing else).
    pub(crate) fn see_clipboard(&self, intent: &Intent, facts: &mut Facts) {
        facts.clipboard = intent.copies() && self.device.clipboard.paste().is_some();
    }

    /// The offline path: the person's sentence `text` was a known command; `actions` run now, in order. Said in the
    /// conversation, on the avatar's panel, logged, and told to the model at its next turn.
    pub(crate) fn quick(&self, text: &str, actions: Vec<Action>) {
        self.add_line(N4n0Who::You, text.to_string());
        let mut facts = self.assistant_facts();
        let (mut said, mut said_en, mut refused, mut panel) = (Vec::new(), Vec::new(), Vec::new(), Vec::<String>::new());
        let (mut asked, mut asked_en) = (Vec::new(), Vec::new());
        for action in actions {
            match action {
                Action::App(intent) => match {
                    self.see_clipboard(&intent, &mut facts);
                    intents::gate(&intent, &facts, false)
                } {
                    Gate::Run => match self.permit_said(&Action::App(intent.clone())).ok_or_else(String::new).and_then(|permit| self.intent_run(&permit, intent.clone(), Via::Quick)) {
                        Ok(_) => {
                            said.push(intent.says());
                            said_en.push(intent.what());
                        }
                        Err(e) => refused.push(format!("{} ({})", intent.says(), short(&e))),
                    },
                    Gate::Refuse(_) => {
                        let why = facts.lacks(&intent).map(|n| n.says().to_string()).unwrap_or_else(|| tr(intents::NOT_NOW).to_string());
                        self.intent_log(&intent, Outcome::Refused, Some(why.clone()), Via::Quick);
                        refused.push(format!("{} ({why})", intent.says()));
                    }
                    // The status and what else is the account's: asked about first, as the model's call would be (r23).
                    Gate::Confirm => {
                        self.intent_log(&intent, Outcome::Asked, None, Via::Quick);
                        asked_en.push(intent.what());
                        asked.push(Action::App(intent));
                    }
                    Gate::Offer(_) => refused.push(intent.says()),
                    Gate::Instead(page) => {
                        self.intent_log(&intent, Outcome::Refused, Some(tr("Not from your account yet: its page opened instead.").into()), Via::Quick);
                        self.open_instead(page, false, Via::Quick);
                        refused.push(format!("{} ({})", intent.says(), tr("not from your account yet")));
                    }
                },
                other => {
                    let words = crate::runtime::action_says(&other);
                    let english = orbiters_i18n::in_language(orbiters_i18n::Language::En, || crate::runtime::action_says(&other));
                    match self.permit_said(&other).ok_or_else(String::new).and_then(|permit| self.run_action_for_assistant(&permit, other)) {
                        Ok(_) => {
                            // The music and the light say what they did in the conversation themselves: the panel says it.
                            self.log_line("quick", words.clone(), Risk::Act, Outcome::Done, None, Via::Quick);
                            panel.push(words);
                            said_en.push(english);
                        }
                        Err(e) => {
                            self.log_line("quick", words.clone(), Risk::Act, Outcome::Failed, Some(short(&e)), Via::Quick);
                            refused.push(format!("{words} ({})", short(&e)));
                        }
                    }
                }
            }
        }
        let mut rt = self.n4n0.lock().unwrap();
        if !said.is_empty() {
            rt.line(N4n0Who::N4n0, said.join(" · "));
        }
        panel.extend(said.iter().cloned());
        if !panel.is_empty() {
            let words = panel.join(", ");
            rt.show(&orbiters_n4n0_core::text::latin(|| format!("{words}.")));
        }
        if !refused.is_empty() {
            rt.line(N4n0Who::Error, tr!("Not done: {what}", what = refused.join("; ")));
            if panel.is_empty() {
                rt.show(tr_latin("I can't do that now."));
            }
        }
        let quoted = context::quoted(text, 200);
        let mut note = format!("The owner said {quoted} and Orbiters - N4N0 did it by itself, without you: {}.", if said_en.is_empty() { "nothing".to_string() } else { said_en.join(", ") }); // i18n: not shown
        if !asked_en.is_empty() {
            note.push_str(&format!(" It asked the owner first about: {}.", asked_en.join(", "))); // i18n: not shown
        }
        rt.notes.push(context::Note { text: note, others: false });
        drop(rt);
        if !asked.is_empty() {
            self.ask_owner(asked);
        }
        self.publish();
        self.keep_conversation();
    }
}

/// The start of an error, for a line.
fn short(e: &str) -> String {
    e.chars().take(120).collect()
}
