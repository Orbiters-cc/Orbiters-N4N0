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

pub mod capture;
pub mod clients;
// It drives a real game window through Windows; the bridge does nothing where VRChat does not run.
#[cfg(all(test, windows))]
mod e2e;
pub mod game;
pub mod guard;
pub mod input;
pub mod keeper;
pub mod lab;
pub mod mcp;
pub mod person;
pub mod tools;
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
    pub game: Arc<dyn Game>,
    pub(crate) timing: Timing,
    pub(crate) presence: Mutex<Presence>,
    pub(crate) own_input: Arc<OwnInput>,
    pub(crate) held: Held,
    guard: Mutex<Option<Guard>>,
    guard_exe: Option<PathBuf>,
    /// One action at a time: a second one is told so rather than waiting behind it.
    pub(crate) action: Mutex<()>,
    pub(crate) run: Mutex<Run>,
    log: Mutex<VecDeque<LogLine>>,
    client: Mutex<Option<(String, Instant)>>,
    in_flight: AtomicUsize,
    /// Bumped by every take-over: what started before it ends.
    stops: AtomicU64,
}

impl Bridge {
    /// `guard_exe`: the program whose `--bridge-guard` releases input when this process dies (`None` in tests, whose
    /// program is the test harness).
    pub fn new(game: Arc<dyn Game>, guard_exe: Option<PathBuf>) -> Arc<Self> {
        Self::with_timing(game, guard_exe, Timing::default())
    }

    pub fn with_timing(game: Arc<dyn Game>, guard_exe: Option<PathBuf>, timing: Timing) -> Arc<Self> {
        Arc::new(Self {
            game,
            timing,
            presence: Mutex::default(),
            own_input: Arc::default(),
            held: Held::default(),
            guard: Mutex::new(None),
            guard_exe,
            action: Mutex::new(()),
            run: Mutex::default(),
            log: Mutex::new(VecDeque::new()),
            client: Mutex::new(None),
            in_flight: AtomicUsize::new(0),
            stops: AtomicU64::new(0),
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

    pub fn paused(&self) -> Option<Paused> {
        self.presence.lock().unwrap().paused(Instant::now())
    }

    /// What the person's listener heard.
    pub fn heard(&self, heard: Heard) {
        match heard {
            Heard::PersonInput => {
                self.presence.lock().unwrap().person_input(Instant::now());
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
        let (was_running, was_going) = {
            let mut run = self.run.lock().unwrap();
            (run.pid.take().is_some(), run.going.take().is_some())
        };
        if was_going {
            self.game.hold_reconstructions(false);
        }
        if was_running && matches!(self.game.seen().capture.state.as_str(), "starting" | "recording" | "paused") {
            self.game.stop_picture();
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

    /// Brings up everything that is down.
    pub fn release_all(&self) {
        for hold in self.held.take_all() {
            self.lift_now(&hold);
        }
    }

    fn lift_now(&self, hold: &Hold) {
        if let Some((address, rest)) = hold.osc_rest() {
            let _ = self.game.osc(address, rest);
        }
        if let Some(up) = hold.up() {
            self.own_input_sent();
            self.game.release(&[up]);
            self.own_input_sent();
        }
        self.tell_guard(format!("up {}", hold.guard_line()));
    }

    /// What the listener does a few times a second: nothing stays down past its time, and the game's comings and goings
    /// are followed.
    pub fn tick(&self) {
        if self.held.overdue(Instant::now()) {
            self.release_all();
            self.log(false, tr("Input was still down past its time: released."));
        }
        self.follow_game();
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
        call: &Call,
        holds: &[(Hold, Option<OscArg>, Option<Input>)],
        seconds: Duration,
        window: Option<&game::GameWindow>,
        still_ok: &dyn Fn() -> Result<(), Failure>,
    ) -> Result<Duration, Failure> {
        /// Everything comes up when this goes, a panic included.
        struct Lifted<'a> {
            bridge: &'a Bridge,
            holds: Vec<Hold>,
        }
        impl Drop for Lifted<'_> {
            fn drop(&mut self) {
                for hold in &self.holds {
                    if self.bridge.held.lift(hold) {
                        self.bridge.lift_now(hold);
                    }
                }
            }
        }
        self.tell_guard_target();
        let begin = Instant::now();
        let until = begin + seconds;
        let mut lifted = Lifted { bridge: self, holds: Vec::new() };
        for (hold, osc, key) in holds {
            self.held.press(hold.clone(), until);
            lifted.holds.push(hold.clone());
            self.tell_guard(format!("down {}", hold.guard_line())); // i18n: not shown (what the AI reads)
            if let Some(address) = match hold {
                Hold::OscButton(a) | Hold::OscAxis(a) => Some(*a),
                _ => None,
            } {
                self.game.osc(address, osc.unwrap_or(OscArg::Int(1)))?;
            }
            if let (Some(key), Some(window)) = (key, window) {
                self.send_own(window, &[*key])?;
            }
        }
        let mut looked = begin;
        while Instant::now() < until {
            if call.client_gone() {
                return Err(Failure::new("client_gone", "The client disconnected: input was released.", "Call the tool again if this still makes sense."));
            }
            if self.stopped_since(call) {
                return Err(Failure::new(
                    "stopped",
                    "The person took control back in Orbiters - N4N0.",
                    "Stop here: the person has to turn the bridge on again.",
                ));
            }
            if let Some(paused) = self.paused() {
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
    pub(crate) fn send_own(&self, window: &game::GameWindow, input: &[Input]) -> Result<(), Failure> {
        self.own_input_sent();
        let sent = self.game.inject(window, input);
        self.own_input_sent();
        sent
    }

    /// Our own input is going out, or just went (see [`person::OwnInput`]).
    pub(crate) fn own_input_sent(&self) {
        #[cfg(windows)]
        // SAFETY: a plain query.
        self.own_input.sent(unsafe { windows::Win32::System::SystemInformation::GetTickCount() });
    }

    /// Tells the guard where OSC goes.
    pub(crate) fn tell_guard_target(&self) {
        if let Some(target) = self.game.osc_target() {
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
