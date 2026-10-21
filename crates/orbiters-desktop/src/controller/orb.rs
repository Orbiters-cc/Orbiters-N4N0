//! Orb in the tray service (contracts `Docs/orb.md`): drives the Orb prop on the avatar over OSC (its text panel and
//! its face), keeps the helpers that need no AI (the AFK message, held and carried parameters, saved videos), and runs
//! the assistant's turns: a question from the voice worker, the avatar's menu or the window goes to Orbiters, the
//! tools it asks for run here, and what Orb answers goes on the panel.
//!
//! The OSC receiving thread only records what it hears: messages to VRChat leave from the orb thread (stopping OSC
//! joins the receiving thread while it holds the OSC lock, so that thread must never wait for it).

use super::{Controller, Shared};
use crate::ipc::OrbCommand;
use crate::state::{OrbConfirmation, OrbFace, OrbLine, OrbStatus, OrbWho, ProcessUsage, VoiceStatus};
use orbiters_contracts::avatar::{self, AvatarManifest};
use orbiters_contracts::orb::{self as contract, params};
use orbiters_core::account::{Api, Server};
use orbiters_core::osc::{OscValue, AVATAR_CHANGE, PARAMETERS_PREFIX};
use orbiters_core::settings::{OrbSettings, Pin};
use orbiters_orb::assistant::{self, AssistantError, Display, Event, Input, Quota, Request, ToolCall, ToolResult};
use orbiters_orb::context::Moment;
use orbiters_orb::controls::{Controls, Setting};
use orbiters_orb::text::{Message, TextSender};
use orbiters_orb::tools::{self, Action, Mood};
use orbiters_orb::videos::{self, SavedVideo, Videos};
use rosc::OscType;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(50);
/// Lines of the conversation the window keeps.
const MAX_LINES: usize = 60;
/// Tool rounds of one turn (contracts `Docs/orb-assistant.md`).
const MAX_ROUNDS: usize = 8;
const BUTTON_RELEASE: Duration = Duration::from_millis(400);
/// A new avatar's parameters exist a moment after `/avatar/change`.
const AVATAR_SETTLE: Duration = Duration::from_millis(1500);
/// A held parameter something else changed is put back after this (its own echo comes first).
const PIN_DEBOUNCE: Duration = Duration::from_millis(400);
/// What VRChat's `/input/Voice` needs between press and release.
const VOICE_PRESS: Duration = Duration::from_millis(80);
/// How much of the conversation the assistant gets back each turn (the server trims too).
const MAX_CONVERSATION: usize = 40;
/// Characters of the avatar's controls in the assistant's context.
const CONTROLS_CHARS: usize = 3500;

/// Who is listening for a sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The avatar menu's Talk button, held.
    Menu,
    /// A hand on the orb: one sentence.
    Touch,
    /// The window's or the overlay's Talk button, held.
    App,
    /// The wake word heard alone: the next sentence.
    WakeWord,
}

#[derive(Default)]
pub struct OrbRuntime {
    sender: Option<TextSender>,
    avatar_id: Option<String>,
    /// The avatar's parameter names, as VRChat listed them when it settled.
    params: HashSet<String>,
    controls: Controls,
    manifest: bool,
    /// Messages for the orb thread to send, and when.
    outbox: Vec<(Instant, Message)>,
    /// When the held parameters are put back.
    pins_due: Option<Instant>,
    face_sent: Option<i32>,
    /// The face the assistant asked for, while its answer is up.
    mood: Option<OrbFace>,
    afk: bool,
    mute_self: bool,
    afk_showing: bool,
    pub(super) listening: Option<(Source, Instant)>,
    muted_by_orb: bool,
    thinking: bool,
    conversation: Vec<Value>,
    lines: Vec<OrbLine>,
    confirmations: Vec<OrbConfirmation>,
    quota: Option<Quota>,
    videos: Videos,
    pub(super) voice: VoiceStatus,
    resources: Vec<ProcessUsage>,
    last_avatar_check: Option<Instant>,
    voice_hint_shown: bool,
    enabled: bool,
    /// Testing (the Orb page): the state the orb is held in, when every state began to show in turn, the owner acted as
    /// away, the whole orb hidden.
    test_face: Option<OrbFace>,
    test_cycle: Option<Instant>,
    test_afk: bool,
    test_hidden: bool,
    /// The orb thread ends (the app quits).
    stop: bool,
    /// A stand-in Orbiters server and credential (tests).
    #[cfg(test)]
    test_server: Option<(String, String)>,
}

impl OrbRuntime {
    fn face(&self) -> OrbFace {
        if let Some(face) = self.test_face {
            face
        } else if self.listening.is_some() {
            OrbFace::Listening
        } else if self.thinking {
            OrbFace::Thinking
        } else if self.sender.as_ref().is_some_and(TextSender::busy) {
            if self.afk_showing {
                OrbFace::Sleeping
            } else {
                self.mood.unwrap_or(OrbFace::Talking)
            }
        } else if self.away() {
            OrbFace::Sleeping
        } else {
            OrbFace::Idle
        }
    }

    /// VRChat says the owner is away, or testing acts as if.
    fn away(&self) -> bool {
        self.afk || self.test_afk
    }

    fn status(&self) -> OrbStatus {
        OrbStatus {
            avatar_id: self.avatar_id.clone(),
            prop_sync: self.sender.as_ref().map(|s| s.sync() as u8),
            manifest: self.manifest,
            controls: self.controls.controls.len(),
            face: self.face(),
            panel: self.sender.as_ref().and_then(TextSender::current_text),
            listening: self.listening.is_some(),
            thinking: self.thinking,
            afk: self.afk,
            lines: self.lines.clone(),
            confirmations: self.confirmations.clone(),
            quota: self.quota.clone(),
            voice: self.voice.clone(),
            videos: self.videos.videos.clone(),
            resources: self.resources.clone(),
            test_face: self.test_face,
            test_cycle: self.test_cycle.is_some(),
            test_afk: self.test_afk,
            test_hidden: self.test_hidden,
        }
    }

    pub(super) fn line(&mut self, who: OrbWho, text: impl Into<String>) {
        self.lines.push(OrbLine { who, text: text.into(), at_ms: super::unix_ms() });
        let excess = self.lines.len().saturating_sub(MAX_LINES);
        self.lines.drain(..excess);
    }

    fn send_now(&mut self, messages: impl IntoIterator<Item = Message>) {
        let now = Instant::now();
        self.outbox.extend(messages.into_iter().map(|m| (now, m)));
    }

    fn send_at(&mut self, at: Instant, messages: impl IntoIterator<Item = Message>) {
        self.outbox.extend(messages.into_iter().map(|m| (at, m)));
    }
}

/// How long each state shows while testing shows them all.
const TEST_CYCLE_STEP: Duration = Duration::from_secs(2);

fn parameter(name: &str) -> String {
    format!("{PARAMETERS_PREFIX}{name}")
}

fn osc_arg(value: &OscValue) -> OscType {
    match value {
        OscValue::Bool(b) => OscType::Bool(*b),
        OscValue::Int(i) => OscType::Int(*i),
        OscValue::Float(f) => OscType::Float(*f),
        OscValue::Text(t) => OscType::String(t.clone()),
    }
}

fn osc_value(arg: &OscType) -> Option<OscValue> {
    Some(match arg {
        OscType::Bool(b) => OscValue::Bool(*b),
        OscType::Int(i) => OscValue::Int(*i),
        OscType::Float(f) => OscValue::Float(*f),
        _ => return None,
    })
}

fn face_value(face: OrbFace) -> i32 {
    (match face {
        OrbFace::Idle => contract::State::Idle,
        OrbFace::Listening => contract::State::Listening,
        OrbFace::Thinking => contract::State::Thinking,
        OrbFace::Talking => contract::State::Talking,
        OrbFace::Sleeping => contract::State::Sleeping,
        OrbFace::Happy => contract::State::Happy,
        OrbFace::Annoyed => contract::State::Annoyed,
        OrbFace::Surprised => contract::State::Surprised,
    }) as i32
}

/// Pin messages for the parameters `present` holds (all of them when it is empty: not known yet).
fn pin_messages(pins: &[Pin], present: &HashSet<String>) -> Vec<Message> {
    pins.iter().filter(|p| present.is_empty() || present.contains(&p.parameter)).map(|p| (parameter(&p.parameter), osc_arg(&p.value))).collect()
}

/// This computer's language (`fr-FR`), for the assistant.
fn locale() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide, as the call requires.
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 1 {
            return String::from_utf16_lossy(&buf[..n as usize - 1]);
        }
    }
    std::env::var("LANG").ok().and_then(|l| l.split('.').next().map(|s| s.replace('_', "-"))).unwrap_or_default()
}

fn local_time_iso() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    now.format(&time::format_description::well_known::Rfc3339).unwrap_or_default()
}

/// What a server tool is called in the conversation ("create_event_draft" → "create event draft").
fn tool_words(name: &str) -> String {
    name.replace('_', " ")
}

impl Controller {
    /// Loads Orb's saved videos and starts the orb thread: the panel's steps, the orb's face, AFK, held parameters.
    pub(super) fn start_orb(self: &Shared) {
        {
            let mut rt = self.orb.lock().unwrap();
            rt.videos = Videos::load(&videos::file());
            rt.voice.installed = super::voice::models_installed();
        }
        self.orb_publish();
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("orb".into()).spawn(move || {
            while !ctl.orb.lock().unwrap().stop {
                ctl.orb_tick(Instant::now());
                std::thread::sleep(TICK);
            }
        });
        // The slower checks run apart, so the panel's steps never wait for them.
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("orb-watch".into()).spawn(move || {
            let mut system = sysinfo::System::new();
            let mut beat = 0u64;
            while !ctl.orb.lock().unwrap().stop {
                ctl.voice_follow();
                if beat.is_multiple_of(3) {
                    ctl.orb_resources(&mut system);
                }
                if beat.is_multiple_of(60) && ctl.settings().world_performance {
                    // World performance summaries still waiting (VRChat closed before they were sent).
                    ctl.performance_send_if_due(false);
                }
                beat += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    fn orb_tick(self: &Shared, now: Instant) {
        let settings = self.settings().orb;
        let vrchat = self.snapshot.lock().unwrap().vrchat.running;
        let mut out: Vec<Message> = Vec::new();
        let mut check_avatar = false;
        let before;
        let after;
        {
            let mut rt = self.orb.lock().unwrap();
            before = (rt.face(), rt.sender.as_ref().and_then(TextSender::current_text), rt.listening.is_some());
            if rt.enabled && !settings.enabled {
                // Orb turned off: what it showed goes away.
                if let Some(sender) = rt.sender.as_mut() {
                    sender.hide();
                }
                rt.afk_showing = false;
            }
            rt.enabled = settings.enabled;
            if let Some(start) = rt.test_cycle {
                // Every state for two seconds, then back to what Orb is doing.
                let step = (now.duration_since(start).as_millis() / TEST_CYCLE_STEP.as_millis()) as usize;
                rt.test_face = OrbFace::ALL.get(step).copied();
                if rt.test_face.is_none() {
                    rt.test_cycle = None;
                }
            }
            self.orb_afk(&mut rt, &settings);
            let busy_before = rt.sender.as_ref().is_some_and(TextSender::busy);
            if let Some(sender) = rt.sender.as_mut() {
                sender.set_step(Duration::from_millis(settings.text_step_ms));
                out.extend(sender.tick(now));
            }
            // The answer's mood ends with it.
            if busy_before && !rt.sender.as_ref().is_some_and(TextSender::busy) {
                rt.mood = None;
                rt.afk_showing = false;
            }
            if rt.sender.is_some() && settings.enabled {
                let face = face_value(rt.face());
                if rt.face_sent != Some(face) {
                    rt.face_sent = Some(face);
                    out.push((parameter(params::STATE), OscType::Int(face)));
                }
            }
            let due: Vec<(Instant, Message)>;
            (due, rt.outbox) = std::mem::take(&mut rt.outbox).into_iter().partition(|(t, _)| *t <= now);
            out.extend(due.into_iter().map(|(_, m)| m));
            if rt.pins_due.is_some_and(|t| t <= now) {
                rt.pins_due = None;
                out.extend(pin_messages(&settings.pins, &rt.params));
            }
            if rt.listening.is_some_and(|(_, since)| now.duration_since(since) > super::voice::LISTEN_LIMIT) {
                rt.listening = None;
            }
            if vrchat && rt.avatar_id.is_none() && rt.last_avatar_check.is_none_or(|t| now.duration_since(t) > Duration::from_secs(10)) {
                rt.last_avatar_check = Some(now);
                check_avatar = true;
            }
            after = (rt.face(), rt.sender.as_ref().and_then(TextSender::current_text), rt.listening.is_some());
        }
        self.orb_send(&out);
        if check_avatar {
            // VRChat was running before Orb listened: ask it which avatar it wears.
            let ctl = self.clone();
            std::thread::spawn(move || ctl.orb_refresh_avatar(None, HashMap::new()));
        }
        if before != after {
            self.orb_publish();
        }
    }

    /// Shows the AFK message while VRChat says the person is away (and nothing else is up), takes it down after.
    fn orb_afk(&self, rt: &mut OrbRuntime, settings: &OrbSettings) {
        let wanted = settings.enabled && settings.afk_message_enabled && rt.away() && !settings.afk_message.trim().is_empty();
        let free = !rt.thinking && rt.listening.is_none();
        let Some(sender) = rt.sender.as_mut() else { return };
        if wanted && !rt.afk_showing && free && !sender.busy() {
            sender.show_sticky(&settings.afk_message);
            rt.afk_showing = true;
        } else if rt.afk_showing && !wanted {
            if sender.sticky() {
                sender.hide();
            }
            rt.afk_showing = false;
        }
    }

    /// Sends messages to VRChat (never from the OSC receiving thread).
    fn orb_send(&self, messages: &[Message]) {
        if messages.is_empty() {
            return;
        }
        let _ = self.with_osc(|o| {
            for (address, arg) in messages {
                if let Err(e) = o.send(address, vec![arg.clone()]) {
                    eprintln!("orb: {address}: {e}");
                }
            }
        });
    }

    pub(super) fn orb_publish(&self) {
        let status = self.orb.lock().unwrap().status();
        self.update_if(|s| {
            let changed = s.orb != status;
            s.orb = status;
            changed
        });
    }

    fn orb_line(&self, who: OrbWho, text: impl Into<String>) {
        self.orb.lock().unwrap().line(who, text);
        self.orb_publish();
    }

    /// An avatar parameter VRChat sent (on the OSC receiving thread: record, never send).
    pub(super) fn orb_on_parameter(self: &Shared, name: &str, value: &OscValue) {
        match name {
            params::TALK => self.orb_talk(value.truthy(), Source::Menu),
            params::TOUCH => {
                if value.truthy() {
                    self.orb_touch();
                }
            }
            "AFK" => self.orb.lock().unwrap().afk = value.truthy(),
            params::WALK_BOOST => {
                if let OscValue::Float(v) = value {
                    self.set_walk_boost(*v);
                }
            }
            "MuteSelf" => self.orb.lock().unwrap().mute_self = value.truthy(),
            _ => {
                let settings = self.settings().orb;
                if let Some(pin) = settings.pins.iter().find(|p| p.parameter == name) {
                    let differs = match (&pin.value, value) {
                        (OscValue::Float(a), OscValue::Float(b)) => (a - b).abs() > 0.005,
                        (a, b) => a != b,
                    };
                    if differs && settings.enabled {
                        self.orb.lock().unwrap().pins_due = Some(Instant::now() + PIN_DEBOUNCE);
                    }
                }
            }
        }
    }

    /// VRChat loaded an avatar (on the OSC receiving thread): what the carried parameters held on the avatar being
    /// left is kept for the new one, which is looked at once it settled.
    pub(super) fn orb_on_avatar(self: &Shared, id: String) {
        let settings = self.settings().orb;
        let carry: HashMap<String, OscValue> = {
            let left = self.orb.lock().unwrap().params.clone();
            let osc = self.osc_values.lock().unwrap();
            settings
                .carry_parameters
                .iter()
                .filter(|p| left.contains(*p))
                .filter_map(|p| osc.latest.get(&parameter(p)).and_then(|(v, _)| v.first().cloned()).map(|v| (p.clone(), v)))
                .collect()
        };
        {
            let mut rt = self.orb.lock().unwrap();
            rt.avatar_id = Some(id.clone());
            if let Some(sender) = rt.sender.as_mut() {
                sender.reset();
            }
            rt.face_sent = None;
            rt.afk_showing = false;
            rt.outbox.clear();
        }
        let ctl = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(AVATAR_SETTLE);
            ctl.orb_refresh_avatar(Some(id), carry);
        });
    }

    /// Looks at the avatar VRChat wears (its id from `/avatar/change`, else VRChat's OSCQuery tree): its parameters,
    /// the Orb prop, its manifest; then puts the carried and held parameters on it.
    pub(super) fn orb_refresh_avatar(self: &Shared, id: Option<String>, carry: HashMap<String, OscValue>) {
        let values = self.vrchat_values();
        let id = id.or_else(|| {
            values.iter().find(|(a, _)| a == AVATAR_CHANGE).and_then(|(_, v)| match v {
                OscValue::Text(t) if !t.is_empty() => Some(t.clone()),
                _ => None,
            })
        });
        let Some(id) = id else { return };
        let parameters: Vec<(&str, &OscValue)> = values.iter().filter(|(a, _)| a.starts_with(PARAMETERS_PREFIX)).map(|(a, v)| (a.as_str(), v)).collect();
        let names: HashSet<String> = parameters.iter().map(|(a, _)| a[PARAMETERS_PREFIX.len()..].to_string()).collect();
        let manifest = read_manifest(&id);
        let synced = names.iter().filter(|n| n.starts_with(params::TEXT_SYNC_PREFIX)).count();
        let sync = if contract::SYNC_COUNTS.contains(&synced) {
            Some(synced)
        } else if names.contains(params::TEXT_POINTER) || (names.is_empty() && manifest.as_ref().is_some_and(|m| m.orb.is_some())) {
            Some(manifest.as_ref().and_then(|m| m.orb).map(|o| o.text_sync as usize).filter(|n| contract::SYNC_COUNTS.contains(n)).unwrap_or(8))
        } else {
            None
        };
        let mut controls = match &manifest {
            Some(m) => Controls::from_manifest(m),
            None => Controls::from_parameters(parameters.iter().copied()),
        };
        controls.learn_types(parameters.iter().copied());
        let has_gogo = names.iter().any(|n| n.starts_with("Go/"));
        let settings = self.settings().orb;
        let mut out: Vec<Message> =
            carry.iter().filter(|(p, _)| names.contains(*p) && (p.as_str() != "VRCEmote" || has_gogo)).map(|(p, v)| (parameter(p), osc_arg(v))).collect();
        if settings.enabled {
            out.extend(pin_messages(&settings.pins, &names));
        }
        {
            let mut rt = self.orb.lock().unwrap();
            // A newer avatar change came meanwhile: its own look follows.
            if rt.avatar_id.as_ref().is_some_and(|current| *current != id) {
                return;
            }
            rt.avatar_id = Some(id);
            rt.params = names;
            rt.manifest = manifest.is_some();
            rt.controls = controls;
            match sync {
                Some(n) if rt.sender.as_ref().map(TextSender::sync) != Some(n) => {
                    rt.sender = Some(TextSender::new(n, Duration::from_millis(settings.text_step_ms)))
                }
                Some(_) => {}
                None => rt.sender = None,
            }
            rt.face_sent = None;
            rt.send_now(out);
        }
        self.orb_publish();
    }

    /// The avatar menu's Talk button (or the app's), pressed or let go.
    pub(super) fn orb_talk(self: &Shared, down: bool, source: Source) {
        let settings = self.settings().orb;
        if !settings.enabled || !settings.assistant {
            return;
        }
        if down {
            if !self.orb_can_listen() {
                return;
            }
            let mut rt = self.orb.lock().unwrap();
            rt.listening = Some((source, Instant::now()));
            // The sentence for Orb stays out of the world: mute VRChat's microphone while the menu's button is held.
            if source == Source::Menu && settings.mute_while_talking && !rt.mute_self {
                let now = Instant::now();
                rt.send_at(now, [("/input/Voice".to_string(), OscType::Int(1))]);
                rt.send_at(now + VOICE_PRESS, [("/input/Voice".to_string(), OscType::Int(0))]);
                rt.muted_by_orb = true;
            }
            drop(rt);
            self.voice_listen(source);
        } else {
            let mut rt = self.orb.lock().unwrap();
            if rt.listening.is_none_or(|(s, _)| s != source) {
                return;
            }
            rt.listening = None;
            // Unmute only what Orb muted (in push-to-talk mode the press never muted anything).
            if rt.muted_by_orb && rt.mute_self {
                let now = Instant::now();
                rt.send_at(now + Duration::from_millis(150), [("/input/Voice".to_string(), OscType::Int(1))]);
                rt.send_at(now + Duration::from_millis(150) + VOICE_PRESS, [("/input/Voice".to_string(), OscType::Int(0))]);
            }
            rt.muted_by_orb = false;
            drop(rt);
            self.voice_finish();
        }
        self.orb_publish();
    }

    /// A hand touched the orb: Orb listens to one sentence.
    fn orb_touch(self: &Shared) {
        let settings = self.settings().orb;
        if !settings.enabled || !settings.assistant || self.orb.lock().unwrap().listening.is_some() || !self.orb_can_listen() {
            return;
        }
        self.orb.lock().unwrap().listening = Some((Source::Touch, Instant::now()));
        self.voice_listen(Source::Touch);
        self.orb_publish();
    }

    /// Whether Orb can listen: the speech models are installed. When not, the panel says how to get them (once).
    fn orb_can_listen(self: &Shared) -> bool {
        let mut rt = self.orb.lock().unwrap();
        if rt.voice.installed {
            return true;
        }
        if !rt.voice_hint_shown {
            rt.voice_hint_shown = true;
            rt.line(OrbWho::Note, "Orb cannot hear yet: install its speech models in Orbiters desktop › Orb.");
            if let Some(sender) = rt.sender.as_mut() {
                sender.show("I can't hear you yet: install my ears in Orbiters desktop > Orb.");
            }
        }
        false
    }

    /// A sentence the voice worker heard: for Orb when it was listening, or when it starts with the wake word.
    pub(super) fn orb_heard(self: &Shared, text: String, wake_word_mode: bool) {
        let settings = self.settings().orb;
        if !settings.enabled || !settings.assistant {
            return;
        }
        let text = text.trim().to_string();
        let listening = self.orb.lock().unwrap().listening.take();
        if listening.is_some() {
            self.orb_publish();
            if !text.is_empty() {
                self.orb_ask(text, "voice");
            }
            return;
        }
        if wake_word_mode && settings.wake_word {
            match orbiters_orb::wake::command(&text, &settings.wake_words) {
                Some(command) if command.is_empty() => {
                    // Only the name: Orb listens to the next sentence.
                    self.orb.lock().unwrap().listening = Some((Source::WakeWord, Instant::now()));
                    self.voice_listen(Source::WakeWord);
                    self.orb_publish();
                }
                Some(command) => self.orb_ask(command, "voice"),
                None => {}
            }
        }
    }

    /// Sends one avatar parameter to VRChat from the orb thread (never from the OSC receiving thread).
    pub(super) fn orb_send_parameter(&self, name: &str, value: OscType) {
        self.orb.lock().unwrap().outbox.push((Instant::now(), (parameter(name), value)));
    }

    /// What the window and the overlay ask.
    pub fn orb_command(self: &Shared, command: OrbCommand) {
        match command {
            OrbCommand::Ask { text } => self.orb_ask(text, "typed"),
            OrbCommand::Talk { down } => self.orb_talk(down, Source::App),
            OrbCommand::Say { text } => {
                let shown = {
                    let mut rt = self.orb.lock().unwrap();
                    rt.afk_showing = false;
                    rt.sender.as_mut().map(|s| s.show(&text)).is_some()
                };
                if !shown {
                    self.notify(super::NoticeKind::Info, "This avatar has no Orb prop: add it from My Avatar's gallery.");
                }
            }
            OrbCommand::HidePanel => {
                let mut rt = self.orb.lock().unwrap();
                rt.afk_showing = false;
                if let Some(s) = rt.sender.as_mut() {
                    s.hide();
                }
            }
            OrbCommand::NewConversation => {
                let mut rt = self.orb.lock().unwrap();
                rt.conversation.clear();
                rt.lines.clear();
                rt.confirmations.clear();
            }
            OrbCommand::SaveVideo => {
                let ctl = self.clone();
                std::thread::spawn(move || match ctl.orb_save_video() {
                    Ok(video) => ctl.orb_line(OrbWho::Note, format!("Saved {}", video.title.as_deref().unwrap_or(&video.url))),
                    Err(e) => ctl.notify(super::NoticeKind::Warning, e),
                });
            }
            OrbCommand::RemoveVideo { url } => {
                let mut rt = self.orb.lock().unwrap();
                if rt.videos.remove(&url) {
                    let _ = rt.videos.save(&videos::file());
                }
            }
            OrbCommand::CopyText { text } => {
                if let Err(e) = copy_to_clipboard(&text) {
                    self.notify(super::NoticeKind::Error, e);
                } else {
                    self.notify(super::NoticeKind::Success, "Copied: paste it in VRChat.");
                }
            }
            OrbCommand::OpenLink { url } => {
                // Only the Orbiters website (an approval), never a link from elsewhere.
                if Server::current().is_website_page(&url) {
                    if let Err(e) = super::open_url(&url) {
                        self.notify(super::NoticeKind::Error, e.to_string());
                    }
                }
            }
            OrbCommand::Photo { delay_seconds } => {
                if let Err(e) = self.orb_photo(delay_seconds) {
                    self.notify(super::NoticeKind::Warning, e);
                }
            }
            OrbCommand::InstallVoice => self.voice_install(),
            OrbCommand::TestFace { face } => {
                let mut rt = self.orb.lock().unwrap();
                rt.test_cycle = None;
                rt.test_face = face;
            }
            OrbCommand::TestCycle => {
                let mut rt = self.orb.lock().unwrap();
                rt.test_cycle = Some(Instant::now());
                rt.test_face = Some(OrbFace::ALL[0]);
            }
            OrbCommand::TestAfk { on } => self.orb.lock().unwrap().test_afk = on,
            OrbCommand::TestTouch => self.orb_touch(),
            OrbCommand::TestVisible { on } => {
                let mut rt = self.orb.lock().unwrap();
                rt.test_hidden = !on;
                rt.outbox.push((Instant::now(), (parameter(params::VISIBLE), OscType::Bool(on))));
            }
        }
        self.orb_publish();
    }

    /// Puts a sentence to Orb's assistant on a thread of its own.
    pub(super) fn orb_ask(self: &Shared, text: String, via: &'static str) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let settings = self.settings().orb;
        if !settings.enabled || !settings.assistant {
            self.orb_line(OrbWho::Note, "Orb's assistant is off: turn it on in Orbiters desktop › Orb.");
            return;
        }
        let Some((api, token)) = self.orb_account() else {
            let mut rt = self.orb.lock().unwrap();
            rt.line(OrbWho::Error, "Sign in to Orbiters to talk to Orb.");
            if let Some(sender) = rt.sender.as_mut() {
                sender.show("Sign in to Orbiters desktop to talk to me.");
            }
            drop(rt);
            self.orb_publish();
            return;
        };
        {
            let mut rt = self.orb.lock().unwrap();
            if rt.thinking {
                rt.line(OrbWho::Note, "Orb is still answering the last one.");
                drop(rt);
                self.orb_publish();
                return;
            }
            rt.thinking = true;
            rt.mood = None;
            rt.line(OrbWho::You, text.clone());
        }
        self.orb_publish();
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("orb-turn".into()).spawn(move || ctl.orb_turn(api, token, text, via));
    }

    fn orb_turn(self: Shared, api: Api, token: String, text: String, via: &'static str) {
        let cancel = AtomicBool::new(false);
        let mut input = Some(Input { text: assistant::utf16_prefix(&text, assistant::INPUT_UNITS).to_string(), via });
        let mut results: Vec<ToolResult> = Vec::new();
        let mut shown = String::new();
        let mut rounds = 0;
        let outcome: anyhow::Result<()> = loop {
            rounds += 1;
            if rounds > MAX_ROUNDS {
                break Err(anyhow::anyhow!("Orb went round in circles: ask again."));
            }
            let (messages, tools, display) = {
                let rt = self.orb.lock().unwrap();
                let bare = !rt.manifest && !rt.controls.is_empty();
                (rt.conversation.clone(), tools::specs(bare), rt.sender.is_some())
            };
            let context = assistant::Context {
                summary: self.orb_moment().summary(),
                locale: locale(),
                time: local_time_iso(),
                display: display.then_some(Display { columns: contract::COLUMNS as u32, rows: contract::ROWS as u32 }),
            };
            let request =
                Request { client: "desktop", messages: &messages, input: input.take(), tool_results: std::mem::take(&mut results), tools: &tools, context };
            let mut answer = String::new();
            let done = assistant::send(&api, &token, &request, &cancel, |e| match e {
                Event::Text(t) => answer.push_str(&t),
                Event::Tool { name, phase } => {
                    if phase == "running" {
                        self.orb_line(OrbWho::Note, format!("Orb: {}…", tool_words(&name)));
                    }
                }
                Event::Confirmation(c) => {
                    let mut rt = self.orb.lock().unwrap();
                    rt.line(OrbWho::Note, format!("Waiting for your approval on Orbiters: {}", if c.title.is_empty() { &c.summary } else { &c.title }));
                    rt.confirmations.retain(|x| x.id != c.id);
                    rt.confirmations.push(OrbConfirmation { id: c.id, title: c.title, summary: c.summary, approve_url: c.approve_url });
                    drop(rt);
                    self.notify(super::NoticeKind::Info, "Orb prepared something that needs your approval: see Orbiters desktop › Orb.");
                }
            });
            match done {
                Err(e) => break Err(e),
                Ok(done) => {
                    {
                        let mut rt = self.orb.lock().unwrap();
                        let keep = done.messages.len().saturating_sub(MAX_CONVERSATION);
                        rt.conversation = done.messages.into_iter().skip(keep).collect();
                        if done.quota.is_some() {
                            rt.quota = done.quota;
                        }
                    }
                    if !answer.trim().is_empty() {
                        shown = answer.trim().to_string();
                    }
                    if done.tool_calls.is_empty() {
                        break Ok(());
                    }
                    for call in &done.tool_calls {
                        results.push(self.orb_run_tool(call));
                    }
                }
            }
        };
        let mut rt = self.orb.lock().unwrap();
        rt.thinking = false;
        match outcome {
            Ok(()) if !shown.is_empty() => {
                rt.line(OrbWho::Orb, shown.clone());
                rt.afk_showing = false;
                if let Some(sender) = rt.sender.as_mut() {
                    sender.show(&shown);
                }
            }
            Ok(()) => {}
            Err(e) => {
                let message = e.downcast_ref::<AssistantError>().map(|a| a.message.clone()).unwrap_or_else(|| format!("Orb could not answer: {e:#}"));
                rt.line(OrbWho::Error, message.clone());
                if let Some(sender) = rt.sender.as_mut() {
                    sender.show(&message);
                }
            }
        }
        drop(rt);
        self.orb_publish();
    }

    /// The Orbiters account the assistant is asked with.
    fn orb_account(&self) -> Option<(Api, String)> {
        #[cfg(test)]
        if let Some((base, token)) = self.orb.lock().unwrap().test_server.clone() {
            return Some((Api::at(&base), token));
        }
        self.account_api()
    }

    /// Ends the orb thread (the app quits).
    pub(super) fn orb_stop(&self) {
        self.orb.lock().unwrap().stop = true;
    }

    /// What the assistant is told about the moment.
    fn orb_moment(&self) -> Moment {
        let (running, world) = {
            let s = self.snapshot.lock().unwrap();
            (s.vrchat.running, s.vrchat.world.clone())
        };
        let latest: HashMap<String, OscValue> =
            self.osc_values.lock().unwrap().latest.iter().filter_map(|(a, (v, _))| v.first().map(|v| (a.clone(), v.clone()))).collect();
        let rt = self.orb.lock().unwrap();
        let last_video = world.last_video.as_ref().map(|v| {
            let title = rt.videos.videos.iter().find(|s| videos::canonical(&s.url) == videos::canonical(&v.url)).and_then(|s| s.title.clone());
            (v.url.clone(), title)
        });
        Moment {
            vrchat_running: running,
            world_name: world.in_world.then(|| world.world_name.clone()).flatten(),
            others: world.others(),
            avatar_name: world.avatar_name.clone(),
            orb_prop: rt.sender.is_some(),
            afk: rt.afk,
            last_video,
            controls: rt.controls.summary(&latest, CONTROLS_CHARS),
            controls_from_manifest: rt.manifest,
            saved_videos: rt.videos.videos.len(),
        }
    }

    /// Runs a tool the assistant called and says how it went.
    fn orb_run_tool(self: &Shared, call: &ToolCall) -> ToolResult {
        let outcome = tools::parse(&call.name, &call.input).and_then(|action| self.orb_action(action));
        match outcome {
            Ok(content) => {
                ToolResult { id: call.id.clone(), content: assistant::utf16_prefix(&content.to_string(), assistant::RESULT_UNITS).to_string(), is_error: false }
            }
            Err(message) => ToolResult { id: call.id.clone(), content: assistant::utf16_prefix(&message, assistant::RESULT_UNITS).to_string(), is_error: true },
        }
    }

    fn orb_action(self: &Shared, action: Action) -> Result<Value, String> {
        match action {
            Action::SetControl { control, on, value } => {
                let setting = match (on, value) {
                    (_, Some(v)) => Setting::Value(v),
                    (Some(true), None) => Setting::On,
                    (Some(false), None) => Setting::Off,
                    (None, None) => Setting::Press,
                };
                let mut rt = self.orb.lock().unwrap();
                let found = rt.controls.find(&control)?.clone();
                let plan = rt.controls.plan(&found, setting)?;
                rt.send_now(plan.now);
                rt.send_at(Instant::now() + BUTTON_RELEASE, plan.later);
                rt.line(OrbWho::Note, format!("Orb set {}", found.path_text()));
                Ok(json!({ "done": true, "control": found.path_text() }))
            }
            Action::SetParameter { name, value } => {
                let mut rt = self.orb.lock().unwrap();
                let found = rt.controls.find(&name)?.clone();
                let plan = rt.controls.plan(&found, Setting::Value(value))?;
                rt.send_now(plan.now);
                rt.line(OrbWho::Note, format!("Orb set {}", found.path_text()));
                Ok(json!({ "done": true, "parameter": found.path_text() }))
            }
            Action::FindControls { query } => {
                let latest: HashMap<String, OscValue> =
                    self.osc_values.lock().unwrap().latest.iter().filter_map(|(a, (v, _))| v.first().map(|v| (a.clone(), v.clone()))).collect();
                let rt = self.orb.lock().unwrap();
                let found: Vec<Value> = rt
                    .controls
                    .search(&query, 25)
                    .into_iter()
                    .map(|c| json!({ "control": c.path_text(), "kind": c.kind.word(), "state": rt.controls.state(c, &latest) }))
                    .collect();
                if found.is_empty() {
                    return Err(format!("No control of this avatar matches \"{query}\"."));
                }
                Ok(json!({ "controls": found }))
            }
            Action::Hold { control, on, value, release } => {
                let (target, plan) = {
                    let rt = self.orb.lock().unwrap();
                    let found = rt.controls.find(&control)?.clone();
                    let setting = match (on, value) {
                        (_, Some(v)) => Setting::Value(v),
                        (Some(false), None) => Setting::Off,
                        _ => Setting::On,
                    };
                    let plan = rt.controls.plan(&found, setting)?;
                    (found.target().unwrap_or_default().to_string(), plan)
                };
                let mut settings = self.settings();
                settings.orb.pins.retain(|p| p.parameter != target);
                if !release {
                    let value = plan.now.first().and_then(|(_, arg)| osc_value(arg)).ok_or("Nothing to hold.")?;
                    settings.orb.pins.push(Pin { parameter: target.clone(), value });
                    self.orb.lock().unwrap().send_now(plan.now);
                }
                self.save_settings(settings);
                self.orb_line(OrbWho::Note, if release { format!("Orb let go of {target}") } else { format!("Orb holds {target}") });
                Ok(json!({ "done": true, "held": !release, "parameter": target }))
            }
            Action::TakePhoto { delay_seconds } => self.orb_photo(delay_seconds),
            Action::SaveVideo => self.orb_save_video().map(|v| json!({ "saved": v.url, "title": v.title })),
            Action::ListSavedVideos => {
                let rt = self.orb.lock().unwrap();
                let list: Vec<Value> = rt.videos.videos.iter().take(10).map(|v| json!({ "title": v.title, "url": v.url, "world": v.world })).collect();
                Ok(json!({ "videos": list }))
            }
            Action::CopyToClipboard { text } => {
                copy_to_clipboard(&text)?;
                self.orb_line(OrbWho::Note, "Orb copied a text: paste it in VRChat.");
                Ok(json!({ "copied": true }))
            }
            Action::SetAfkMessage { text, enabled } => {
                let mut settings = self.settings();
                if let Some(text) = text {
                    settings.orb.afk_message = text;
                }
                if let Some(enabled) = enabled {
                    settings.orb.afk_message_enabled = enabled;
                }
                let (message, on) = (settings.orb.afk_message.clone(), settings.orb.afk_message_enabled);
                self.save_settings(settings);
                Ok(json!({ "afk_message": message, "enabled": on }))
            }
            Action::Mood { mood } => {
                self.orb.lock().unwrap().mood = Some(match mood {
                    Mood::Neutral => OrbFace::Talking,
                    Mood::Happy => OrbFace::Happy,
                    Mood::Annoyed => OrbFace::Annoyed,
                    Mood::Surprised => OrbFace::Surprised,
                    Mood::Sleepy => OrbFace::Sleeping,
                });
                Ok(json!({ "done": true }))
            }
        }
    }

    /// Takes a picture with VRChat's camera, opening it in photo mode first when it is closed (and closing it after).
    fn orb_photo(self: &Shared, delay_seconds: u32) -> Result<Value, String> {
        if self.is_capturing() {
            return Err("VRChat's camera is recording for Orbiters desktop right now.".into());
        }
        let now = Instant::now();
        let open = Duration::from_millis(800);
        let shot = now + open + Duration::from_secs(delay_seconds as u64);
        let mut rt = self.orb.lock().unwrap();
        rt.send_at(now, [("/usercamera/Mode".to_string(), OscType::Int(1))]);
        rt.send_at(shot, [("/usercamera/Capture".to_string(), OscType::Bool(true))]);
        rt.send_at(shot + Duration::from_millis(1500), [("/usercamera/Mode".to_string(), OscType::Int(0))]);
        rt.line(OrbWho::Note, if delay_seconds > 0 { format!("Orb takes a picture in {delay_seconds} s") } else { "Orb takes a picture".to_string() });
        Ok(json!({ "done": true, "in_seconds": delay_seconds }))
    }

    /// Saves the video the world played last (VRChat's log), with its title when YouTube gives it.
    fn orb_save_video(self: &Shared) -> Result<SavedVideo, String> {
        let world = self.snapshot.lock().unwrap().vrchat.world.clone();
        let seen = world.last_video.ok_or("No video played in this world yet.")?;
        let title = videos::youtube_title(&seen.url);
        let video = SavedVideo { url: seen.url, title, world: world.world_name, saved_utc: orbiters_core::now_utc_iso() };
        let mut rt = self.orb.lock().unwrap();
        if rt.videos.add(video.clone()) {
            rt.videos.save(&videos::file()).map_err(|e| format!("Could not save the video list: {e}"))?;
        }
        drop(rt);
        self.orb_publish();
        Ok(video)
    }

    /// CPU and memory of Orbiters' processes and VRChat's (the Orb page's footprint).
    fn orb_resources(&self, system: &mut sysinfo::System) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
        let groups: [(&str, &[&str]); 5] = [
            ("Orbiters desktop", &["orbiters-desktop.exe", "orbiters-desktop"]),
            ("Capture worker", &["orbiters-capture.exe", "orbiters-capture"]),
            ("Orb's ears", &["orbiters-voice.exe", "orbiters-voice"]),
            ("Reconstruction", &["orbiters-reconstruct.exe", "orbiters-reconstruct"]),
            ("VRChat", &["vrchat.exe", "vrchat"]),
        ];
        let mut usage = Vec::new();
        for (label, names) in groups {
            let (mut cpu, mut memory, mut any) = (0.0f32, 0u64, false);
            for p in system.processes().values() {
                let n = p.name().to_string_lossy().to_lowercase();
                if names.contains(&n.as_str()) {
                    any = true;
                    cpu += p.cpu_usage();
                    memory += p.memory();
                }
            }
            if any {
                usage.push(ProcessUsage { name: label.to_string(), cpu_percent: cpu, memory_mb: memory as f32 / (1024.0 * 1024.0) });
            }
        }
        let changed = {
            let mut rt = self.orb.lock().unwrap();
            let changed = rt.resources != usage;
            rt.resources = usage;
            changed
        };
        if changed {
            self.orb_publish();
        }
    }
}

/// The manifest Orbiters Toolkit wrote for `avatar_id`, when there is a readable one.
fn read_manifest(avatar_id: &str) -> Option<AvatarManifest> {
    let path = orbiters_core::paths::avatars_dir().join(avatar::file_name(avatar_id));
    let text = std::fs::read_to_string(path).ok()?;
    match AvatarManifest::parse(&text) {
        Ok(m) if m.avatar_id == avatar_id => Some(m),
        Ok(_) => None,
        Err(e) => {
            eprintln!("avatar manifest {avatar_id}: {e}");
            None
        }
    }
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).map_err(|e| format!("Could not copy: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::orb::{Page, TEXT_LENGTH};
    use orbiters_core::settings::Settings;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, UdpSocket};
    use std::sync::{Arc, Mutex};

    /// A stand-in VRChat: its OSC input port, and its OSCQuery service answering `tree` to every request.
    fn vrchat(tree: serde_json::Value) -> (UdpSocket, std::net::SocketAddr) {
        let input = UdpSocket::bind("127.0.0.1:0").unwrap();
        input.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let query = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = query.local_addr().unwrap();
        let body = tree.to_string();
        std::thread::spawn(move || {
            for stream in query.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(format!("{head}{body}").as_bytes());
            }
        });
        (input, addr)
    }

    /// VRChat's tree for an avatar with the Orb prop (8 sync values) and a Hoodie toggle.
    fn avatar_tree(id: &str) -> serde_json::Value {
        let mut parameters = serde_json::Map::new();
        let mut put = |name: &str, kind: &str, value: serde_json::Value| {
            parameters.insert(name.replace('/', "_"), json!({ "FULL_PATH": format!("{PARAMETERS_PREFIX}{name}"), "TYPE": kind, "VALUE": [value] }));
        };
        for i in 0..8 {
            put(&format!("Orb/Text/Sync{i}"), "f", json!(0.0));
        }
        put("Orb/Text/Pointer", "i", json!(0));
        put("Orb/Text/Visible", "T", json!(false));
        put("Orb/State", "i", json!(0));
        put("Hoodie", "T", json!(false));
        put("VelocityX", "f", json!(0.0));
        json!({ "FULL_PATH": "/avatar", "CONTENTS": {
            "change": { "FULL_PATH": "/avatar/change", "TYPE": "s", "VALUE": [id] },
            "parameters": { "FULL_PATH": "/avatar/parameters", "CONTENTS": parameters }
        } })
    }

    fn received(socket: &UdpSocket, wait: Duration) -> Vec<Message> {
        let mut out = Vec::new();
        let until = Instant::now() + wait;
        let mut buf = [0u8; 2048];
        while Instant::now() < until {
            if let Ok(n) = socket.recv(&mut buf) {
                if let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                    out.push((m.addr, m.args[0].clone()));
                }
            }
        }
        out
    }

    fn first_row(page: &Page) -> String {
        contract::page_text(page).lines().next().unwrap_or_default().trim_end().to_string()
    }

    fn orb_with_vrchat() -> (Shared, UdpSocket) {
        let (input, query) = vrchat(avatar_tree("avtr_orb_test"));
        let mut settings = Settings::default();
        settings.osc.vrchat_port = input.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.with_osc(|o| o.set_vrchat_query(Some(query)));
        ctl.start_orb();
        ctl.orb_refresh_avatar(None, HashMap::new());
        (ctl, input)
    }

    #[test]
    fn finds_the_prop_and_shows_text_on_its_panel() {
        let (ctl, vrchat) = orb_with_vrchat();
        let status = ctl.view().orb.clone();
        assert_eq!(status.avatar_id.as_deref(), Some("avtr_orb_test"));
        assert_eq!(status.prop_sync, Some(8));
        assert_eq!(status.controls, 1, "Hoodie: the system and Orb parameters are left out");
        ctl.orb_command(OrbCommand::Say { text: "Bonjour ! Ça va ?".into() });
        let sent = received(&vrchat, Duration::from_millis(2700));
        let mut cells: Page = [9; TEXT_LENGTH];
        let visible = orbiters_orb::text::apply(&mut cells, 8, &sent);
        assert_eq!(visible, Some(true), "{sent:?}");
        assert_eq!(first_row(&cells), "Bonjour ! Ça va ?");
        assert!(sent.contains(&(parameter(params::STATE), OscType::Int(3))), "the orb talks while it shows");
        ctl.orb_stop();
        ctl.shutdown();
    }

    #[test]
    fn testing_holds_a_state_then_plays_them_all() {
        let (ctl, vrchat) = orb_with_vrchat();
        ctl.orb_command(OrbCommand::TestFace { face: Some(OrbFace::Happy) });
        let sent = received(&vrchat, Duration::from_millis(400));
        assert!(sent.contains(&(parameter(params::STATE), OscType::Int(5))), "held happy: {sent:?}");
        assert_eq!(ctl.view().orb.test_face, Some(OrbFace::Happy));
        ctl.orb_command(OrbCommand::TestCycle);
        let sent = received(&vrchat, Duration::from_millis(2600));
        let states: Vec<i32> =
            sent.iter().filter(|(a, _)| *a == parameter(params::STATE)).filter_map(|(_, v)| if let OscType::Int(i) = v { Some(*i) } else { None }).collect();
        assert_eq!(states, vec![0, 1], "idle, then listening two seconds later");
        ctl.orb_command(OrbCommand::TestVisible { on: false });
        let sent = received(&vrchat, Duration::from_millis(300));
        assert!(sent.contains(&(parameter(params::VISIBLE), OscType::Bool(false))));
        ctl.orb_stop();
        ctl.shutdown();
    }

    /// A stand-in Orbiters server: answers each turn request with the next of `answers` (server-sent events) and
    /// keeps the requests' bodies.
    fn orbiters(answers: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let seen = bodies.clone();
        std::thread::spawn(move || {
            let mut answers = answers.into_iter();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0usize;
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                seen.lock().unwrap().push(serde_json::from_slice::<Value>(&body).unwrap());
                let events = answers.next().unwrap_or_default();
                let mut stream = stream;
                let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{events}").as_bytes());
            }
        });
        (base, bodies)
    }

    #[test]
    fn a_turn_runs_the_tools_it_asks_for_and_answers_on_the_panel() {
        let (ctl, vrchat) = orb_with_vrchat();
        let first = format!(
            "event: done\ndata: {}\n\n",
            json!({ "messages": [{ "role": "user", "content": "mets mon sweat" }],
                    "tool_calls": [{ "id": "call_1", "name": "set_avatar_control", "input": { "control": "hoodie", "on": true } }],
                    "quota": { "used": 1, "limit": 80 } })
        );
        let second = format!(
            "event: text\ndata: {}\n\nevent: text\ndata: {}\n\nevent: done\ndata: {}\n\n",
            json!({ "delta": "Voilà, ton sweat. " }),
            json!({ "delta": "Va voir tes potes." }),
            json!({ "messages": [], "tool_calls": [], "quota": { "used": 2, "limit": 80 } })
        );
        let (base, bodies) = orbiters(vec![first, second]);
        ctl.orb.lock().unwrap().test_server = Some((base, "orbit-test".into()));
        ctl.orb_ask("Orb, mets mon sweat".into(), "typed");
        let sent = received(&vrchat, Duration::from_millis(4000));
        assert!(sent.contains(&(parameter("Hoodie"), OscType::Bool(true))), "{sent:?}");
        let mut cells: Page = [0; TEXT_LENGTH];
        orbiters_orb::text::apply(&mut cells, 8, &sent);
        assert_eq!(first_row(&cells), "Voilà, ton sweat. Va voir tes");
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["input"]["text"], "Orb, mets mon sweat");
        assert!(bodies[0]["context"]["summary"].as_str().unwrap().contains("Hoodie (parameter"), "{}", bodies[0]["context"]["summary"]);
        assert_eq!(bodies[0]["context"]["display"]["columns"], 32);
        assert!(bodies[0]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "set_avatar_parameter"), "a bare avatar gets its parameters by name");
        assert_eq!(bodies[1]["tool_results"][0]["id"], "call_1");
        assert_eq!(bodies[1]["tool_results"][0]["is_error"], false);
        assert_eq!(bodies[1]["messages"][0]["content"], "mets mon sweat", "the conversation comes back as the server returned it");
        drop(bodies);
        let status = ctl.view().orb.clone();
        assert_eq!(status.quota.map(|q| q.used), Some(2));
        let whos: Vec<OrbWho> = status.lines.iter().map(|l| l.who).collect();
        assert_eq!(whos, vec![OrbWho::You, OrbWho::Note, OrbWho::Orb]);
        assert!(!status.thinking);
        ctl.orb_stop();
        ctl.shutdown();
    }

    #[test]
    fn the_wake_word_starts_a_turn_and_other_sentences_stay_here() {
        let mut settings = Settings::default();
        settings.orb.wake_word = true;
        let ctl = Controller::new(settings);
        let (base, bodies) = orbiters(vec![format!(
            "event: done
data: {}

",
            json!({ "messages": [], "tool_calls": [] })
        )]);
        ctl.orb.lock().unwrap().test_server = Some((base, "orbit-test".into()));
        // Heard while the wake word is on, but not for Orb: nothing goes anywhere.
        ctl.orb_heard("I saw a cool orb in that world".into(), true);
        // Misheard "Orb" still wakes it; the request carries what follows.
        ctl.orb_heard("O Herb, mets mon sweat à capuche.".into(), true);
        let started = Instant::now();
        while bodies.lock().unwrap().is_empty() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["input"]["text"], "mets mon sweat à capuche.");
        assert_eq!(bodies[0]["input"]["via"], "voice");
        assert!(bodies[0]["context"]["display"].is_null(), "no Orb prop: no panel limits");
    }
}
