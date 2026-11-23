//! The assistant's intents in the runtime (REQUEST20 6, `orbiters_n4n0_core::intents`): each runs through the same path
//! the menus and pages use (N4N0's menu effects and commands here, the app's own controller through
//! [`crate::Host::assistant_intent`] for what is the app's), is said where the person looks (the Ask page's conversation
//! and actions, a toast, the press sound, the avatar's panel), logged, and undoable where it has an opposite.
//!
//! The offline path ([`orbiters_n4n0_core::intents::quick`]) runs here too: the most common short commands, from the
//! person's own sentence alone, at once and with no network.

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
}

/// What undoes an action.
#[derive(Clone, Debug)]
enum UndoStep {
    Action(Action),
    Effects(Vec<Effect>),
}

/// A share asked for the next capture ("take a 3D picture, then share it"): it goes once a newer capture is ready.
#[derive(Clone, Debug)]
struct ShareNext {
    /// The latest capture's key when it was asked (a newer one is the picture's).
    after: Option<String>,
    public: bool,
    recipients: Vec<orbiters_n4n0_core::captures::Friend>,
    asked: Instant,
}

/// The assistant's part of N4N0's state.
#[derive(Default)]
pub(crate) struct AssistantRuntime {
    pub(crate) limiter: Limiter,
    pub(crate) view: Slice<AssistantView>,
    undo: Vec<(Intent, UndoStep)>,
    /// The last list of each kind a tool answered with, its items as given (`ref` names what an index acts on).
    lists: HashMap<&'static str, Vec<Value>>,
    youtube: Vec<orbiters_n4n0_core::youtube::Video>,
    share_next: Option<ShareNext>,
}

impl AssistantRuntime {
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
    pub(crate) fn intent_run(self: &Shared, intent: Intent, via: Via) -> Result<Value, String> {
        let risk = intent.risk();
        let limited = self.n4n0.lock().unwrap().assistant.limiter.take(if risk == Risk::Confirm { Risk::Confirm } else { Risk::Act }, Instant::now());
        if let Err(why) = limited {
            self.intent_log(&intent, Outcome::Refused, Some(tr("Too many actions in a short time: wait a moment.").into()), via);
            self.publish();
            return Err(why);
        }
        self.intent_doing(Some(intent.says()));
        self.publish();
        let outcome = self.intent_do(&intent);
        self.intent_doing(None);
        match outcome {
            Ok((value, undo)) => {
                if let Some(step) = undo {
                    let mut rt = self.n4n0.lock().unwrap();
                    rt.assistant.undo.push((intent.clone(), step));
                    let excess = rt.assistant.undo.len().saturating_sub(UNDO_KEPT);
                    rt.assistant.undo.drain(..excess);
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
    fn intent_do(self: &Shared, intent: &Intent) -> Result<(Value, Option<UndoStep>), String> {
        let done = || json!({ "done": true });
        let settings = self.settings();
        let item = self.item(intent)?;
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
            Intent::ShareCapture { which, public, friends } => {
                let recipients = if friends.is_empty() { Vec::new() } else { self.recipients(friends)? };
                let view = (*self.n4n0.lock().unwrap().captures).clone();
                let key = match which {
                    Which::Next => {
                        self.n4n0.lock().unwrap().assistant.share_next =
                            Some(ShareNext { after: view.latest.as_ref().map(|l| l.key.clone()), public: *public, recipients, asked: Instant::now() });
                        return Ok((json!({ "done": true, "note": "It is shared once the next 3D capture is ready (its 3D view made)." }), None)); // i18n: not shown
                    }
                    Which::Latest => view.latest.as_ref().or(view.history.first()).map(|r| r.key.clone()).ok_or("No 3D capture yet.")?, // i18n: not shown
                    Which::Index(_) => item.as_ref().and_then(|i| field(i, "ref")).map(str::to_string).ok_or("No such capture in the last list.")?, // i18n: not shown
                };
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
            Intent::YoutubeWatch { index } | Intent::YoutubeCopy { index } | Intent::YoutubeDrop { index } => {
                let video = self.n4n0.lock().unwrap().assistant.youtube.get(index - 1).cloned().ok_or("No such result: search again.")?; // i18n: not shown
                let (link, title) = (video.link(), video.title.clone());
                let effect = match intent {
                    Intent::YoutubeWatch { .. } => Effect::WatchVideo { link },
                    Intent::YoutubeCopy { .. } => Effect::Copy { link, title },
                    _ => Effect::Drop { link, title, video: Some(video.id.clone()) },
                };
                self.screen_effects(vec![effect]);
                (done(), None)
            }
            Intent::HistoryCopy { .. } | Intent::HistoryDrop { .. } => {
                let item = item.ok_or("No such video in the last list.")?; // i18n: not shown
                let link = field(&item, "ref").ok_or("No link for that video.")?.to_string(); // i18n: not shown
                let title = field(&item, "title").unwrap_or_default().to_string();
                let video = orbiters_n4n0_core::videos::youtube_id(&link);
                let effect = if matches!(intent, Intent::HistoryCopy { .. }) { Effect::Copy { link, title } } else { Effect::Drop { link, title, video } };
                self.screen_effects(vec![effect]);
                (done(), None)
            }
            Intent::TakeBackCard => {
                self.screen_effects(vec![Effect::TakeBack]);
                (done(), None)
            }
            Intent::WhereAmI => {
                let world = self.host.world();
                let kind = world.instance.as_deref().map(|i| orbiters_n4n0_core::menu::InstanceRow { location: format!("w:{i}"), world: String::new(), at: 0 }.kind());
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
                self.screen_effects(vec![Effect::CopyText { text: name }]);
                (done(), None)
            }
            Intent::WorldLike { on } => {
                let world = self.here()?;
                self.feedback_ask(orbiters_n4n0_core::feedback::FeedbackAsk::Like { world, on: *on });
                (done(), None)
            }
            Intent::WorldComment { text } => {
                let world = self.here()?;
                self.feedback_ask(orbiters_n4n0_core::feedback::FeedbackAsk::Comment { world, text: text.clone() });
                (done(), None)
            }
            Intent::WorldLabel { label, on } => {
                let world = self.here()?;
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
                let world = self.here()?;
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
                self.invite_send(location, false);
                (json!({ "done": true, "note": "The invite's answer shows on Previous instances." }), None) // i18n: not shown
            }
            Intent::Call { friend } => {
                let calls = self.host.calls();
                let (id, _) = pick_person(friend, calls.contacts.iter().map(|c| (c.id.as_str(), c.name.as_str())))?;
                self.host.call(crate::CallRequest::Contact(id));
                (done(), None)
            }
            Intent::AnswerCall => {
                self.host.call(crate::CallRequest::Answer);
                (done(), None)
            }
            Intent::EndCall => {
                self.host.call(crate::CallRequest::End);
                (done(), None)
            }
            Intent::MuteCall { on } => {
                self.host.call(crate::CallRequest::Mute(*on));
                (done(), Some(UndoStep::Action(Action::App(Intent::MuteCall { on: !on }))))
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
            Intent::ApplyState { name } => {
                let state = {
                    let rt = self.n4n0.lock().unwrap();
                    let view = rt.states.view(rt.avatar_id.as_deref(), None);
                    let names: Vec<String> = view.states.iter().map(|s| s.name.clone()).collect();
                    pick_person(name, view.states.iter().map(|s| (s.id.as_str(), s.name.as_str())))
                        .map_err(|_| format!("No saved state called {} on this avatar; its states: {}.", context::quoted(name, NAME_CHARS), names.iter().map(|n| context::quoted(n, NAME_CHARS)).collect::<Vec<_>>().join(", ")))? // i18n: not shown
                };
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
                let n = self.clone();
                std::thread::spawn(move || n.screen_effects(vec![Effect::Assistant(false)]));
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
                    self.host.assistant_intent(intent, None)?;
                }
                (done(), None)
            }
            Intent::Restricted { what } => {
                // Never done: only its page opens, after the owner's yes.
                self.open_page(what.page(), None);
                if what.page().on_window() {
                    self.host.assistant_intent(&Intent::OpenPage { page: what.page() }, None)?;
                }
                (json!({ "opened": what.page().key(), "note": "Only the page opened: the owner does this there." }), None) // i18n: not shown
            }
            Intent::State { about } => (intents::untrusted(self.state_answer(about)), None),
            Intent::Undo => {
                let last = self.n4n0.lock().unwrap().assistant.undo.pop();
                let (undone, step) = last.ok_or("Nothing to undo.")?; // i18n: not shown
                match step {
                    UndoStep::Effects(effects) => self.screen_effects(effects),
                    UndoStep::Action(Action::App(i)) => {
                        self.intent_do(&i)?;
                    }
                    UndoStep::Action(action) => {
                        self.run_action_undo(action)?;
                    }
                }
                (json!({ "undone": undone.what() }), None)
            }
            // The app's own: through its controller, as its pages do. What it read is kept as the list of its kind.
            _ => {
                let undo = self.host_undo(intent, &settings);
                let value = self.host.assistant_intent(intent, item.as_ref())?;
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

    /// An undo of one of the older tools' actions (the music, the light).
    fn run_action_undo(self: &Shared, action: Action) -> Result<(), String> {
        self.run_action_for_assistant(action).map(|_| ())
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

    /// Asks the interfaces to open `page` (each opens a request once); the window's pages through the app as well.
    pub(crate) fn open_page(self: &Shared, page: Page, world: Option<(String, String)>) {
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

    /// A newer capture is ready: the share asked for "the next one" goes now (once).
    pub(crate) fn share_next_check(self: &Shared) {
        let ask = {
            let mut rt = self.n4n0.lock().unwrap();
            let Some(next) = rt.assistant.share_next.as_ref() else { return };
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
        self.share_capture(ask);
        self.add_line(N4n0Who::Note, tr("N4N0 shares the 3D capture you just took, as you said."));
        self.publish();
    }

    /// The offline path: the person's sentence `text` was a known command; `actions` run now, in order. Said in the
    /// conversation, on the avatar's panel, logged, and told to the model at its next turn.
    pub(crate) fn quick(self: &Shared, text: &str, actions: Vec<Action>) {
        self.add_line(N4n0Who::You, text.to_string());
        let facts = self.assistant_facts();
        let (mut said, mut said_en, mut refused, mut panel) = (Vec::new(), Vec::new(), Vec::new(), Vec::<String>::new());
        for action in actions {
            match action {
                Action::App(intent) => match intents::gate(&intent, &facts, false) {
                    Gate::Run => match self.intent_run(intent.clone(), Via::Quick) {
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
                    // Never on this path (it only knows class a), but never run either.
                    Gate::Confirm | Gate::Offer(_) => refused.push(intent.says()),
                },
                other => {
                    let words = crate::runtime::action_says(&other);
                    let english = orbiters_i18n::in_language(orbiters_i18n::Language::En, || crate::runtime::action_says(&other));
                    match self.run_action_for_assistant(other) {
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
        rt.notes.push(format!("The owner said {quoted} and Orbiters - N4N0 did it by itself, without you: {}.", if said_en.is_empty() { "nothing".to_string() } else { said_en.join(", ") })); // i18n: not shown
        drop(rt);
        self.publish();
    }
}

/// The start of an error, for a line.
fn short(e: &str) -> String {
    e.chars().take(120).collect()
}
