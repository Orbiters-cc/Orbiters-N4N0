//! The VRChat bridge: an experimental MCP service in the tray process that lets an agent start VRChat in desktop mode, go
//! from private instance to private instance, look at the screen, walk around, take 3D pictures and close the game.
//! `docs/vrchat-bridge.md` tells the whole of it; in short:
//!
//! - It exists for accounts the server says have the `n4n0-vrchat-bridge` feature, asked again every few minutes
//!   ([`keeper`]), and only while Settings has it on. Without both nothing listens and nothing shows.
//! - It listens on 127.0.0.1 only, wants a token, and refuses other `Host`s and `Origin`s ([`mcp`]).
//! - The rules live in the tools ([`tools`]), between the agent and [`game::Game`]: only a game the bridge itself started,
//!   only strict private instances of the signed-in account, keyboard and mouse only into the foreground game window, the
//!   person's own input pausing everything ([`person`]), and every press bounded and released ([`input`], [`guard`]).
//! - It reads the game's log and window, and sends startup arguments, OSC and ordinary window input. It opens no handle to
//!   the game's process, reads none of its memory, injects nothing, and does nothing around Easy Anti-Cheat.
//! - It also does what the Unity projects tab does ([`projects`]): projects, packages, repositories, Unity's editors, new
//!   projects and copies, the Library move, Orbiters' packages and world uploads, behind the same gate, address and token.

pub mod capture;
pub mod clients;
// It drives a real game window through Windows; the bridge does nothing where VRChat does not run.
#[cfg(all(test, windows))]
mod e2e;
pub mod game;
pub mod guard;
pub mod input;
pub mod journal;
pub mod keeper;
pub mod lab;
pub mod mcp;
pub mod person;
pub mod projects;
pub mod sandbox;
pub mod sessions;
pub mod tools;
pub mod tour;
pub mod win;

use game::{Failure, Game, OscArg};
use guard::Guard;
use input::{Held, Hold, Input};
use person::{Heard, OwnInput, Paused, Presence};
use serde::{Deserialize, Serialize};
use orbiters_i18n::tr;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A client is "connected" this long after its last request: the protocol has no connection to watch.
const CONNECTED_FOR: Duration = Duration::from_secs(60);
/// How many actions the log keeps.
const LOG_LINES: usize = 80;

/// Limits the lab shortens: a fake game starts in a moment.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Instances made at most this often: the API's rules ask for restraint, and VRChat itself kicks people who hop too fast.
    pub instance_every: Duration,
    /// How long a start may take to reach its world (a large world on a first visit downloads for a while).
    pub start_limit: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { instance_every: Duration::from_secs(30), start_limit: Duration::from_secs(240) }
    }
}

/// One line of the log of recent actions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// Unix seconds.
    pub at: u64,
    pub text: String,
    /// The action did what was asked.
    pub ok: bool,
}

/// The client last heard from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSeen {
    pub name: String,
    /// It made a request within the last minute, or one is running.
    pub connected: bool,
}

/// What Settings shows. The token is in it because Settings shows it to give to a client: only for an account the server
/// says has the feature, and only over the app's own link to its window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The server says this account has the feature (recently enough): Settings shows the Experimental section.
    pub allowed: bool,
    pub listening: bool,
    /// `http://127.0.0.1:<port>/mcp`.
    pub address: Option<String>,
    pub token: Option<String>,
    pub client: Option<ClientSeen>,
    pub paused: Option<Paused>,
    /// An action is running right now.
    pub acting: bool,
    pub log: Vec<LogLine>,
    /// Why it does not listen although it is on, or what does not work.
    pub problem: Option<String>,
    /// The hotkey was taken by another program: the person's input and Stop still work.
    pub hotkey_taken: bool,
    /// The agents it can be added to, and what each of them has.
    pub clients: Vec<clients::ClientStatus>,
}

/// What Settings asks of the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Adds the bridge to an agent, updates what it has, or takes it away.
    Client { client: clients::Client, change: clients::Change },
    /// Settings is shown: what the agents have is asked again.
    CheckClients,
    /// Takes control back at once: releases input, ends what runs, and turns the bridge off.
    Stop,
    /// Pauses the agent until resumed.
    Pause,
    Resume,
    /// A new token: clients have to be given it again.
    NewToken,
}

/// A 3D picture the agent took in this run.
#[derive(Clone, Debug)]
pub(crate) struct PictureRecord {
    pub session: PathBuf,
    pub world_id: Option<String>,
    pub world_name: Option<String>,
    pub length_s: u32,
}

/// A start of the game that has not arrived yet.
#[derive(Clone, Debug)]
pub(crate) struct Going {
    pub world_id: String,
    pub instance: String,
    pub since: Instant,
    /// Starting is the next step (the game before it is being closed).
    pub started: bool,
}

/// What the bridge knows of its own run.
#[derive(Default)]
pub(crate) struct Run {
    /// The VRChat the bridge started: only that one is ever acted on. A VRChat somebody else started is theirs.
    pub pid: Option<u32>,
    pub going: Option<Going>,
    /// The bridge itself is closing the game: its going away is no news.
    pub closing: bool,
    /// The last time an instance was made.
    pub last_instance: Option<Instant>,
    pub pictures: Vec<PictureRecord>,
    /// The last thing that happened by itself: the game gone, a start that did not arrive.
    pub event: Option<String>,
    /// The worlds this session arrived in (lowercase ids), oldest first: the only ones its agent may label, besides the
    /// one it is in.
    pub been: Vec<String>,
    /// The instance its VRChat arrived in last (`world:number`, [`tools::place_key`]): moved anywhere else not its own
    /// private, the bridge leaves.
    pub placed: Option<String>,
    /// Instances this session was in or asked for before the start under way: what a log still shows of them is the past,
    /// never where a new start landed.
    pub earlier: Vec<String>,
    /// The bridge closed its VRChat because it was somewhere it must not be: its going away is that, not a crash.
    pub leaving: bool,
    /// The last event is such a leaving (`landed_elsewhere`).
    pub left: bool,
    /// The window's client size when its VRChat arrived (what `-screen-width` and `-screen-height` asked): the size clicks'
    /// coordinates are for, put back when someone changed it.
    pub size: Option<(u32, u32)>,
    /// A side session's start the bridge gave up on: when, the launch's moment, and where it was going. A VRChat of it that
    /// comes up afterwards is this session's ([`Game::adopt`]), never the person's (2026-11-15 12:25).
    pub given_up: Option<(Instant, std::time::SystemTime, Option<Going>)>,
}

/// One VRChat the bridge drives (bridge/sessions.rs): `main`, the app's own, or a side session beside anything else.
pub struct Session {
    /// `main`, or `p<profile>-<random>`: what an agent passes as `session`.
    pub id: String,
    /// VRChat's `--profile` (`None`: main without one, the default).
    pub profile: Option<u32>,
    /// A side session's OSC ports, in and out (`None`: main, on VRChat's defaults).
    pub osc: Option<(u16, u16)>,
    pub game: Arc<dyn Game>,
    pub(crate) run: Mutex<Run>,
    /// One action at a time in this session: a second one is told so rather than waiting behind it.
    pub(crate) action: Mutex<()>,
    pub(crate) held: Held,
    /// The person's own input in this session's window (side sessions): it pauses this session only.
    pub(crate) presence: Mutex<Presence>,
    /// Who drives it, for side sessions.
    pub(crate) lease: Mutex<Option<sessions::Lease>>,
    pub(crate) opened: Instant,
}

impl Session {
    pub(crate) fn new(id: &str, profile: Option<u32>, osc: Option<(u16, u16)>, game: Arc<dyn Game>) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_string(),
            profile,
            osc,
            game,
            run: Mutex::default(),
            action: Mutex::new(()),
            held: Held::default(),
            presence: Mutex::default(),
            lease: Mutex::new(None),
            opened: Instant::now(),
        })
    }

    pub fn is_main(&self) -> bool {
        self.osc.is_none()
    }

    /// The VRChat this session started, while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.run.lock().unwrap().pid
    }
}

/// Whose turn it is to have the keyboard and the mouse: only one VRChat window can be the foreground, so the steps that
/// need it (keys, clicks, bringing a window forward) take turns between sessions, the one that asked first next.
#[derive(Default)]
pub(crate) struct FocusTurn {
    /// The session holding the turn, and when it last used it.
    holder: Option<(String, Instant)>,
    /// Sessions waiting: when they first asked, and last asked.
    waiting: Vec<(String, Instant, Instant)>,
}

/// A turn not used for this long is free for the next session.
pub(crate) const FOCUS_HOLD: Duration = Duration::from_secs(8);
/// A waiter that stopped asking for this long loses its place.
const FOCUS_WAITER: Duration = Duration::from_secs(30);

impl FocusTurn {
    /// Asks for the turn for `session`: `Ok` with it, or the seconds to wait.
    pub(crate) fn ask(&mut self, session: &str, now: Instant) -> Result<(), u64> {
        self.waiting.retain(|w| now.duration_since(w.2) < FOCUS_WAITER);
        if let Some((holder, used)) = self.holder.clone() {
            if holder == session {
                self.holder = Some((session.to_string(), now));
                return Ok(());
            }
            if now.duration_since(used) < FOCUS_HOLD {
                self.wait(session, now);
                return Err((FOCUS_HOLD - now.duration_since(used)).as_secs() + 1);
            }
        }
        self.wait(session, now);
        // Free: the one that asked first gets it.
        let first = self.waiting.iter().min_by_key(|w| w.1).map(|w| w.0.clone());
        if first.as_deref() == Some(session) {
            self.waiting.retain(|w| w.0 != session);
            self.holder = Some((session.to_string(), now));
            Ok(())
        } else {
            Err(2)
        }
    }

    fn wait(&mut self, session: &str, now: Instant) {
        match self.waiting.iter_mut().find(|w| w.0 == session) {
            Some(w) => w.2 = now,
            None => self.waiting.push((session.to_string(), now, now)),
        }
    }

    /// `session` is done with it (closed, or gone).
    pub(crate) fn leave(&mut self, session: &str) {
        if self.holder.as_ref().is_some_and(|h| h.0 == session) {
            self.holder = None;
        }
        self.waiting.retain(|w| w.0 != session);
    }

    pub(crate) fn holder(&self) -> Option<&str> {
        self.holder.as_ref().map(|h| h.0.as_str())
    }
}

/// A tool call's way of knowing it should end: its client went away, or the service stopped it.
#[derive(Clone)]
pub struct Call {
    client_gone: Arc<AtomicBool>,
    stop_at_start: u64,
}

impl Call {
    pub fn client_gone(&self) -> bool {
        self.client_gone.load(Ordering::Relaxed)
    }
}

/// The running bridge's state and rules. Lives for as long as the service; [`mcp::Server`] comes and goes around it.
pub struct Bridge {
    /// The main session's game: the app's own VRChat.
    pub game: Arc<dyn Game>,
    pub(crate) timing: Timing,
    /// The person: their input anywhere on this computer, the hotkey and Settings' Pause.
    pub(crate) presence: Mutex<Presence>,
    pub(crate) own_input: Arc<OwnInput>,
    guard: Mutex<Option<Guard>>,
    guard_exe: Option<PathBuf>,
    /// The session of before, which tools without `session` act on.
    pub(crate) main: Arc<Session>,
    /// Side sessions, in the order they were opened.
    pub(crate) sides: Mutex<Vec<Arc<Session>>>,
    /// What starts side sessions and knows the computer (`None`: none can be opened).
    pub(crate) opener: Mutex<Option<Arc<dyn game::Opener>>>,
    /// N4N0's world feedback, for the label tools (`None`: they say it is not there).
    pub(crate) feedback: Mutex<Option<Arc<dyn game::Feedback>>>,
    /// Publishing pictures, the app's environment and the pictures' files (`None`: those tools say it is not there).
    pub(crate) publish: Mutex<Option<Arc<dyn game::Publish>>>,
    /// The worlds arrived in and the pictures taken, kept across restarts (journal.rs).
    pub(crate) journal: Mutex<journal::Journal>,
    /// One launch of VRChat at a time on this computer: a new VRChat is told apart from the others by appearing after it.
    pub(crate) launching: Mutex<()>,
    pub(crate) focus: Mutex<FocusTurn>,
    log: Mutex<VecDeque<LogLine>>,
    client: Mutex<Option<(String, Instant)>>,
    in_flight: AtomicUsize,
    /// Bumped by every take-over: what started before it ends.
    stops: AtomicU64,
    /// The Unity projects tools and their jobs.
    pub projects: projects::Projects,
}

impl Bridge {
    /// `guard_exe`: the program whose `--bridge-guard` releases input when this process dies (`None` in tests, whose
    /// program is the test harness).
    pub fn new(game: Arc<dyn Game>, guard_exe: Option<PathBuf>) -> Arc<Self> {
        Self::with_timing(game, guard_exe, Timing::default())
    }

    pub fn with_timing(game: Arc<dyn Game>, guard_exe: Option<PathBuf>, timing: Timing) -> Arc<Self> {
        Arc::new(Self {
            main: Session::new(sessions::MAIN, None, None, game.clone()),
            game,
            timing,
            presence: Mutex::default(),
            own_input: Arc::default(),
            guard: Mutex::new(None),
            guard_exe,
            sides: Mutex::new(Vec::new()),
            opener: Mutex::new(None),
            feedback: Mutex::new(None),
            publish: Mutex::new(None),
            journal: Mutex::default(),
            launching: Mutex::new(()),
            focus: Mutex::default(),
            log: Mutex::new(VecDeque::new()),
            client: Mutex::new(None),
            in_flight: AtomicUsize::new(0),
            stops: AtomicU64::new(0),
            projects: projects::Projects::default(),
        })
    }

    /// Fills in what the bridge itself knows.
    pub fn status_into(&self, status: &mut Status) {
        status.paused = self.paused_for_status();
        status.client = self.client_seen();
        status.acting = self.in_flight.load(Ordering::Relaxed) > 0;
        status.log = self.log.lock().unwrap().iter().cloned().collect();
    }

    /// The pause as Settings shows it: which kind, not its countdown, or the state would change every second.
    fn paused_for_status(&self) -> Option<Paused> {
        self.paused().map(|p| match p {
            Paused::Person { .. } => Paused::Person { resumes_in_s: 0 },
            Paused::Held => Paused::Held,
        })
    }

    fn client_seen(&self) -> Option<ClientSeen> {
        let client = self.client.lock().unwrap();
        client
            .as_ref()
            .map(|(name, at)| ClientSeen { name: name.clone(), connected: at.elapsed() < CONNECTED_FOR || self.in_flight.load(Ordering::Relaxed) > 0 })
    }

    /// A client made a request. `name` is what it calls itself, when it said.
    pub(crate) fn client_heard(&self, name: Option<&str>) {
        let mut client = self.client.lock().unwrap();
        // What a client calls itself is shown in Settings: a name, not a page.
        let name = name
            .map(|n| n.chars().filter(|c| !c.is_control()).take(64).collect::<String>())
            .or_else(|| client.as_ref().map(|c| c.0.clone()))
            .unwrap_or_else(|| tr("An MCP client").into());
        *client = Some((name, Instant::now()));
    }

    pub fn log(&self, ok: bool, text: impl Into<String>) {
        let mut log = self.log.lock().unwrap();
        log.push_back(LogLine { at: orbiters_core::visited::now(), text: text.into(), ok });
        while log.len() > LOG_LINES {
            log.pop_front();
        }
    }

    /// Paused on the whole computer: the hotkey, Settings, or the person's input anywhere in the last 30 s. The main session,
    /// and every step that needs the keyboard and the mouse, stop for it.
    pub fn paused(&self) -> Option<Paused> {
        self.presence.lock().unwrap().paused(Instant::now())
    }

    /// Paused for this session: the main session as [`Bridge::paused`] (as ever); a side session by the hotkey, Settings,
    /// or the person's own input in its window, not by their input elsewhere (OSC and screenshots need nothing of theirs).
    pub fn paused_in(&self, session: &Session) -> Option<Paused> {
        if session.is_main() {
            return self.paused();
        }
        let now = Instant::now();
        match self.presence.lock().unwrap().paused(now) {
            Some(Paused::Held) => Some(Paused::Held),
            // The person records in this session's VRChat (they chose it on the Capture page): its input waits for them.
            _ if session.game.persons_capture_here() => Some(Paused::Person { resumes_in_s: 30 }),
            _ => session.presence.lock().unwrap().paused(now),
        }
    }

    /// Every session, main first.
    pub fn sessions(&self) -> Vec<Arc<Session>> {
        std::iter::once(self.main.clone()).chain(self.sides.lock().unwrap().iter().cloned()).collect()
    }

    /// What starts side sessions and knows the computer.
    pub fn set_opener(&self, opener: Arc<dyn game::Opener>) {
        *self.opener.lock().unwrap() = Some(opener);
    }

    pub fn set_feedback(&self, feedback: Arc<dyn game::Feedback>) {
        *self.feedback.lock().unwrap() = Some(feedback);
    }

    pub fn set_publish(&self, publish: Arc<dyn game::Publish>) {
        *self.publish.lock().unwrap() = Some(publish);
    }

    /// Keeps the journal in `path` from now on (read from it now).
    pub fn keep_journal(&self, path: &std::path::Path) {
        *self.journal.lock().unwrap() = journal::Journal::at(path);
    }

    /// What the person's listener heard.
    pub fn heard(&self, heard: Heard) {
        match heard {
            Heard::PersonInput => {
                let now = Instant::now();
                self.presence.lock().unwrap().person_input(now);
                // Their input in a side session's window is theirs to that session: it waits for them.
                for side in self.sides.lock().unwrap().iter() {
                    if let Some(window) = side.pid().and_then(|pid| side.game.window(pid)) {
                        if side.game.is_foreground(&window) {
                            side.presence.lock().unwrap().person_input(now);
                        }
                    }
                }
                self.release_all();
            }
            Heard::Hotkey => {
                let held = {
                    let mut presence = self.presence.lock().unwrap();
                    let held = !presence.held();
                    if held {
                        presence.set_held(true);
                    } else {
                        presence.resume();
                    }
                    held
                };
                self.release_all();
                self.log(true, if held { tr("Paused by the hotkey.") } else { tr("Going on: resumed by the hotkey.") });
            }
        }
    }

    /// Settings' Pause and Resume.
    pub fn hold(&self, held: bool) {
        {
            let mut presence = self.presence.lock().unwrap();
            if held {
                presence.set_held(true);
            } else {
                presence.resume();
            }
        }
        self.release_all();
        self.log(true, if held { tr("Paused from Settings.") } else { tr("Going on: resumed from Settings.") });
    }

    /// The person takes control back: everything held comes up, what runs ends, a recording stops, and the game (left as
    /// it is) is not the bridge's any more.
    pub fn take_over(&self, why: &str) {
        self.stops.fetch_add(1, Ordering::Relaxed);
        self.release_all();
        // A copy, a Library move, a scan or a size ends where it safely can; a package change finishes.
        self.projects.stop_all();
        for session in self.sessions() {
            let (was_running, was_going) = {
                let mut run = session.run.lock().unwrap();
                // A side session's VRChat stays the bridge's, to be closed by close_session: only what runs ends.
                let pid = if session.is_main() { run.pid.take() } else { run.pid };
                (pid.is_some(), run.going.take().is_some())
            };
            if was_going {
                session.game.hold_reconstructions(false);
            }
            if was_running && matches!(session.game.seen().capture.state.as_str(), "starting" | "recording" | "paused") {
                session.game.stop_picture();
            }
        }
        self.log(true, why.to_string());
    }

    /// Starts telling the guard; called when the service starts listening.
    pub(crate) fn start_guard(&self) -> Result<(), String> {
        let Some(exe) = &self.guard_exe else { return Ok(()) };
        let guard = Guard::spawn(exe).map_err(|e| tr!("The input guard did not start: {error}", error = e))?;
        *self.guard.lock().unwrap() = Some(guard);
        Ok(())
    }

    /// Ends the guard; called when the service stops listening, with everything released.
    pub(crate) fn stop_guard(&self) {
        *self.guard.lock().unwrap() = None;
    }

    fn tell_guard(&self, line: String) {
        if let Some(guard) = self.guard.lock().unwrap().as_mut() {
            guard.tell(&line);
        }
    }

    /// Brings up everything that is down, in every session.
    pub fn release_all(&self) {
        for session in self.sessions() {
            self.release_session(&session);
        }
    }

    /// Brings up what is down in one session.
    pub(crate) fn release_session(&self, session: &Session) {
        for hold in session.held.take_all() {
            self.lift_now(session, &hold);
        }
    }

    fn lift_now(&self, session: &Session, hold: &Hold) {
        if let Some((address, rest)) = hold.osc_rest() {
            let _ = session.game.osc(address, rest);
        }
        if let Some(up) = hold.up() {
            self.own_input_sent();
            session.game.release(&[up]);
            self.own_input_sent();
        }
        self.tell_guard(format!("up {}", Self::guard_line(session, hold)));
    }

    /// The guard's line for a hold: an OSC one names where it goes (each session has its own input port).
    fn guard_line(session: &Session, hold: &Hold) -> String {
        match (hold.osc_rest(), session.game.osc_target()) {
            (Some(_), Some(target)) => format!("{} @{target}", hold.guard_line()), // i18n: not shown (what the guard reads)
            _ => hold.guard_line(),
        }
    }

    /// What the listener does a few times a second: nothing stays down past its time, and the games' comings and goings
    /// are followed.
    pub fn tick(&self) {
        let now = Instant::now();
        if let Some(opener) = self.opener.lock().unwrap().clone() {
            opener.set_main(self.main.pid());
        }
        for session in self.sessions() {
            if session.held.overdue(now) {
                self.release_session(&session);
                self.log(false, tr("Input was still down past its time: released."));
            }
            session.game.tick();
            self.follow_game(&session);
        }
    }

    pub(crate) fn begin_call(&self, client_gone: Arc<AtomicBool>) -> Call {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        Call { client_gone, stop_at_start: self.stops.load(Ordering::Relaxed) }
    }

    pub(crate) fn end_call(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    pub(crate) fn stopped_since(&self, call: &Call) -> bool {
        self.stops.load(Ordering::Relaxed) != call.stop_at_start
    }

    /// Presses `holds` for `seconds`, then brings them up, or as soon as the client goes away, the person takes over, or
    /// (`still_ok`, asked a few times a second) what allowed it stops being so. The end of this function has nothing down
    /// from it, however it ends.
    pub(crate) fn press_for(
        &self,
        session: &Session,
        call: &Call,
        holds: &[(Hold, Option<OscArg>, Option<Input>)],
        seconds: Duration,
        window: Option<&game::GameWindow>,
        still_ok: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<Duration, Failure> {
        /// Everything comes up when this goes, a panic included.
        struct Lifted<'a> {
            bridge: &'a Bridge,
            session: &'a Session,
            holds: Vec<Hold>,
        }
        impl Drop for Lifted<'_> {
            fn drop(&mut self) {
                for hold in &self.holds {
                    if self.session.held.lift(hold) {
                        self.bridge.lift_now(self.session, hold);
                    }
                }
            }
        }
        self.tell_guard_target();
        let begin = Instant::now();
        let until = begin + seconds;
        let mut lifted = Lifted { bridge: self, session, holds: Vec::new() };
        for (hold, osc, key) in holds {
            session.held.press(hold.clone(), until);
            lifted.holds.push(hold.clone());
            self.tell_guard(format!("down {}", Self::guard_line(session, hold))); // i18n: not shown (what the AI reads)
            if let Some(address) = match hold {
                Hold::OscButton(a) | Hold::OscAxis(a) => Some(*a),
                _ => None,
            } {
                session.game.osc(address, osc.unwrap_or(OscArg::Int(1)))?;
            }
            if let (Some(key), Some(window)) = (key, window) {
                self.send_own(session, window, &[*key])?;
            }
        }
        let mut looked = begin;
        while Instant::now() < until {
            if call.client_gone() {
                return Err(Failure::new("client_gone", "The client disconnected: input was released.", "Call the tool again if this still makes sense.")); // i18n: not shown (an AI client reads it in the tool's answer)
            }
            if self.stopped_since(call) {
                return Err(Failure::new(
                    "stopped",
                    "The person took control back in Orbiters - N4N0.", // i18n: not shown (an AI client reads it in the tool's answer)
                    "Stop here: the person has to turn the bridge on again.", // i18n: not shown (an AI client reads it in the tool's answer)
                ));
            }
            // Keys need the person away from the whole computer; OSC only from this session's window.
            if let Some(paused) = if window.is_some() { self.paused() } else { self.paused_in(session) } {
                return Err(tools::paused_failure(&paused));
            }
            if looked.elapsed() > Duration::from_millis(200) {
                looked = Instant::now();
                still_ok()?;
            }
            // A person's input is heard on another thread and released at once: this only ends the call.
            std::thread::sleep(Duration::from_millis(10).min(until.saturating_duration_since(Instant::now())));
        }
        Ok(begin.elapsed())
    }

    /// Sends input into the game as our own. Said before as well as after: the raw input of `SendInput` wakes the thread that
    /// hears the person at once, and told only afterwards, that thread took the bridge's own click for the person's, and paused it
    /// for 30 s after each click and key in the first run on the real game (2026-11-06).
    pub(crate) fn send_own(&self, session: &Session, window: &game::GameWindow, input: &[Input]) -> Result<(), Failure> {
        self.own_input_sent();
        let sent = session.game.inject(window, input);
        self.own_input_sent();
        sent
    }

    /// Our own input is going out, or just went (see [`person::OwnInput`]).
    pub(crate) fn own_input_sent(&self) {
        #[cfg(windows)]
        // SAFETY: a plain query.
        self.own_input.sent(unsafe { windows::Win32::System::SystemInformation::GetTickCount() });
    }

    /// Tells the guard where the main session's OSC goes (a side session's lines name their own).
    pub(crate) fn tell_guard_target(&self) {
        if let Some(target) = self.main.game.osc_target() {
            self.tell_guard(format!("target {target}")); // i18n: not shown (what the AI reads)
        }
    }
}

/// The person-hearing thread's callback.
pub fn hear_person(bridge: &Arc<Bridge>) -> Option<person::Listener> {
    let weak = Arc::downgrade(bridge);
    person::listen(bridge.own_input.clone(), move |heard| {
        if let Some(bridge) = weak.upgrade() {
            bridge.heard(heard);
        }
    })
}
