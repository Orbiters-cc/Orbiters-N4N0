//! N4N0 calls in the tray service (contracts `Docs/n4n0-calls.md`): presence and the friends this computer's VRCX and
//! VRCNext know (as hashes) go to Orbiters every minute while calls are on; the server's event stream brings incoming
//! calls and the other side's answer and candidates; the `orbiters-call` worker carries the audio peer to peer
//! (protocol `orbiters_core::call`). N4N0's panel lists the contacts while its Call menu is open and says who is
//! calling.
//!
//! Each call has a number of this side's own, which the worker's commands and events carry, and the server's events
//! are checked against the call's id: nothing a call that ended (or the worker that carried it) says acts on the next
//! one. The rules are [`CallsRuntime`]'s: its methods change the state and return what to do ([`Acts`]), which
//! [`Controller::calls_act`](super::Controller) does: the worker's commands, the requests to Orbiters (sent in order
//! by one thread, candidates batched) and what the panel says.
//!
//! This computer's addresses reach no one who only lets a call ring: the worker leaves the candidates out of the
//! descriptions, and each side's own wait until both took the call. Calls go through the relay only unless direct
//! calls are on (`N4n0Settings::direct_calls`), so the other side does not learn them then either.

use super::{NoticeKind, Shared};
use crate::state::{CallContact, CallLink, CallPhase, CallView};
use orbiters_core::account::{Api, ApiError};
use orbiters_core::call::{self, CallCommand, CallEvent, EndReason, IceCandidate, IceServer};
use orbiters_core::vrchat_accounts::{self, LocalVrchat};
use orbiters_i18n::{fill, tr, tr_latin};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PRESENCE_EVERY: Duration = Duration::from_secs(60);
const CONTACTS_EVERY: Duration = Duration::from_secs(30);
/// VRCX and VRCNext are read again after this.
const FRIENDS_EVERY: Duration = Duration::from_secs(5 * 60);
/// The most friends a presence report carries (the contract's limit).
const FRIENDS_MOST: usize = 5000;
/// Contacts shown on the panel at once.
pub const PAGE: usize = 4;
/// How long each phase may last before the call is given up: our call ringing at the friend's (Orbiters rings it
/// 30 s), a friend's ringing here, connecting once answered.
const CALLING_FOR: Duration = Duration::from_secs(40);
const RINGING_FOR: Duration = Duration::from_secs(35);
const CONNECTING_FOR: Duration = Duration::from_secs(30);
/// Candidates kept for a call while they cannot go yet.
const KEPT_MOST: usize = 64;
/// The event stream is opened again when nothing came for this long (Orbiters pings every 20 s): a connection that
/// died without a word (the computer slept, another network) would never ring again otherwise.
const STREAM_IDLE: Duration = Duration::from_secs(60);
/// A stream that failed is opened again after this, doubling up to [`STREAM_RETRY_MOST`].
const STREAM_RETRY: Duration = Duration::from_secs(2);
const STREAM_RETRY_MOST: Duration = Duration::from_secs(60);
/// The longest line of the event stream (a description is a few kB).
const EVENT_MOST: usize = 256 * 1024;
/// This side's candidates that come within this of each other go in one request.
const BATCH: Duration = Duration::from_millis(100);
/// Requests about a call give up after this; the relay servers' too (a caller waits on them).
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(15);
const ICE_TIMEOUT: Duration = Duration::from_secs(10);

struct Worker {
    child: Child,
    stdin: ChildStdin,
    /// Which worker this is: the end of an earlier one's output says nothing about the calls of this one.
    number: u64,
}

impl Worker {
    fn spawn(ctl: Shared, number: u64) -> anyhow::Result<Self> {
        let path = crate::worker::sibling_exe("orbiters-call");
        anyhow::ensure!(path.is_file(), "{}", tr!("The call worker was not found at {path}", path = path.display()));
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn()?;
        crate::worker::keep_with_service(&child);
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        std::thread::Builder::new().name("call-stdout".into()).spawn(move || {
            crate::worker::pipe_records(stdout, "call", |line| {
                match serde_json::from_str::<CallEvent>(&line) {
                    Ok(e) => ctl.on_call_worker(e),
                    // Not the line: it may hold a description (addresses).
                    Err(e) => eprintln!("call: unexpected output ({} bytes): {e}", line.len()),
                }
            });
            ctl.on_call_worker_exit(number);
        })?;
        std::thread::Builder::new().name("call-stderr".into()).spawn(move || {
            crate::worker::pipe_lines(stderr, |line| eprintln!("call: {}", line.strip_prefix("call: ").unwrap_or(&line)));
        })?;
        Ok(Self { child, stdin, number })
    }

    fn send(&mut self, command: &CallCommand) -> std::io::Result<()> {
        writeln!(self.stdin, "{}", command.to_line())?;
        self.stdin.flush()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.send(&CallCommand::Quit);
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

/// The call under way.
#[derive(Debug)]
struct Call {
    /// This side's number for it: the worker's commands and events carry it.
    number: u64,
    /// The server's id, once it gave one (a caller's offer goes before).
    id: Option<String>,
    contact: String,
    name: String,
    phase: CallPhase,
    incoming: bool,
    /// When the phase began (its deadline counts from there).
    since: Instant,
    /// An incoming call's offer and relay servers, until it is answered.
    offer: Option<(String, Vec<IceServer>)>,
    /// The worker that took the call's offer or answer command: mute goes to it from then on, and its end ends the
    /// call.
    worker: Option<u64>,
    /// Both sides took the call: this side's candidates go to the other one as they come. Until then they wait in
    /// `outbox` (a caller's until the friend answered, a callee's until it answered itself).
    open: bool,
    outbox: Vec<IceCandidate>,
    /// The caller's candidates that came while the call rang here, for the worker once it is answered.
    inbox: Vec<IceCandidate>,
    connected_at: Option<u64>,
    muted: bool,
    levels: (f32, f32),
}

impl Call {
    fn new(number: u64, contact: String, name: String, incoming: bool, now: Instant) -> Self {
        Self {
            number,
            id: None,
            contact,
            name,
            phase: if incoming { CallPhase::Ringing } else { CallPhase::Calling },
            incoming,
            since: now,
            offer: None,
            worker: None,
            open: false,
            outbox: Vec::new(),
            inbox: Vec::new(),
            connected_at: None,
            muted: false,
            levels: (0.0, 0.0),
        }
    }

    fn ringing_in(&self) -> bool {
        self.incoming && self.phase == CallPhase::Ringing
    }
}

/// How a call ended, for the panel (which everyone around reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// The friend declined our call.
    Declined,
    /// We declined theirs.
    DeclinedHere,
    /// Our call rang out.
    Unanswered,
    /// Theirs rang out, or they gave up before it was answered.
    Missed,
    Ended,
    /// The friend is in another call.
    Busy,
    Failed,
    /// Nothing got through this network.
    Unreachable,
}

impl Ending {
    fn text(self, name: &str, connected: bool) -> String {
        match self {
            Ending::Declined => fill(tr_latin("{name} can't talk right now."), &[("name", &name)]),
            Ending::DeclinedHere => fill(tr_latin("Call from {name} declined."), &[("name", &name)]),
            Ending::Unanswered => fill(tr_latin("{name} didn't answer."), &[("name", &name)]),
            Ending::Missed => fill(tr_latin("Missed call from {name}."), &[("name", &name)]),
            Ending::Ended => fill(tr_latin("Call with {name} ended."), &[("name", &name)]),
            Ending::Busy => fill(tr_latin("{name} is on another call."), &[("name", &name)]),
            Ending::Failed if connected => fill(tr_latin("The call with {name} dropped."), &[("name", &name)]),
            Ending::Failed => fill(tr_latin("The call with {name} could not connect."), &[("name", &name)]),
            Ending::Unreachable => fill(tr_latin("The call with {name} could not connect: this network seems to block calls."), &[("name", &name)]),
        }
    }
}

/// What asking for the relay servers gave.
#[derive(Debug)]
enum Relays {
    Got(Vec<IceServer>),
    /// Orbiters refused: the account has not N4N0 calls (`not_enabled`). Final: nothing of the call goes on.
    Denied,
    Failed(String),
}

impl super::Controller {
    /// This side's relay servers, with the credential of the identity signed in; a `not_enabled` refusal closes calls
    /// for that credential.
    fn relays(self: &Shared) -> Relays {
        let Some((api, token)) = self.calls_api() else { return Relays::Failed(tr("sign in to Orbiters to call.").to_string()) };
        match ice_servers(&api, &token) {
            Ok(servers) => Relays::Got(servers),
            Err(e) if super::calls_access::not_enabled(&e) => {
                self.calls_access_denied(&token);
                Relays::Denied
            }
            Err(e) => Relays::Failed(tr!("the call relay could not be reached: {error}", error = e)),
        }
    }

    /// Whether calls are still open to `whose`, the identity a press was made with.
    fn calls_still_open_to(self: &Shared, whose: &Option<super::calls_access::Whose>) -> bool {
        whose.is_some() && self.calls_identity().map(|(w, ..)| w) == *whose && self.calls_open()
    }
}

/// A request to Orbiters about a call, sent in order by one thread.
#[derive(Debug, PartialEq)]
enum Signal {
    /// `POST /n4n0/calls`: our offer; the answer gives the call its id.
    Create { number: u64, contact: String, offer: String },
    /// `POST /n4n0/calls/:id/answer`: the call is over when it fails.
    Answer { number: u64, id: String, sdp: String },
    /// `POST /n4n0/calls/:id/candidates`, with those that follow within [`BATCH`].
    Candidates { id: String, candidates: Vec<IceCandidate> },
    /// `POST /n4n0/calls/:id/decline` or `/end` (the call is over here: a failure is only logged).
    Close { id: String, decline: bool },
}

/// What a change of the calls asks for.
#[derive(Debug, Default)]
struct Acts {
    /// The worker's commands, in order.
    worker: Vec<CallCommand>,
    /// Requests to Orbiters, in order.
    signals: Vec<Signal>,
    /// What the panel says.
    say: Option<String>,
    notice: Option<(NoticeKind, String)>,
    /// What the window and the overlay show changed.
    publish: bool,
}

#[derive(Default)]
pub struct CallsRuntime {
    local: LocalVrchat,
    local_read: Option<Instant>,
    /// The friends' hashes for presence (made again only when VRCX and VRCNext are read again), and their digest.
    friend_hashes: Vec<String>,
    friends_digest: u64,
    /// The digest of the friend hashes Orbiters has (sent again when they change or the server lost them).
    friends_sent: Option<u64>,
    link: CallLink,
    contacts: Vec<CallContact>,
    contacts_read: Option<Instant>,
    presence_sent: Option<Instant>,
    available_sent: Option<bool>,
    call: Option<Call>,
    /// The last number given to a call.
    numbered: u64,
    worker: Option<Worker>,
    /// The last number given to a worker.
    workers: u64,
    /// The thread that sends the requests about calls.
    signals: Option<mpsc::Sender<Signal>>,
    /// One client for Orbiters while the server stays the same: its connections are kept between requests.
    client: Option<Api>,
    /// Direct calls are allowed (the N4N0 setting, read before each offer and answer). Off, calls go through the relay
    /// only: the other side sees the relay's address, never this computer's.
    direct: bool,
    /// The panel lists the contacts (N4N0's Call menu is open), and which one is chosen.
    pub(super) menu_open: bool,
    pub(super) picked: usize,
    /// The event stream open (or opening): its number and the credential it was opened with. A stream whose number
    /// is not this one any more ends at its next line (calls turned off, signed out, another account).
    streaming: Option<(u64, String)>,
    streams: u64,
    stream_failures: u32,
    stream_retry_at: Option<Instant>,
    stop: bool,
    /// The server (its API address) and the credential the state above is for: when either changes (an environment
    /// switch, another account), nothing of the other's is listed or callable any more.
    identity: Option<(String, String)>,
    /// Whether this Orbiters account has N4N0 calls (`controller/calls_access.rs`).
    pub(super) access: super::calls_access::CallsAccess,
}

impl CallsRuntime {
    fn current(&mut self, number: u64) -> Option<&mut Call> {
        self.call.as_mut().filter(|c| c.number == number)
    }

    /// Our call to `contact` starts (its relay servers come next): its number. None while another call is on.
    fn dial(&mut self, contact: &str, name: &str, now: Instant) -> Option<u64> {
        if self.call.is_some() {
            return None;
        }
        self.numbered += 1;
        self.call = Some(Call::new(self.numbered, contact.to_string(), name.to_string(), false, now));
        Some(self.numbered)
    }

    /// The worker's command that starts our call `number`, now that its relay servers came (none when it ended
    /// meanwhile).
    fn offer(&mut self, number: u64, ice_servers: Vec<IceServer>, mic: Option<String>) -> Option<CallCommand> {
        let c = self.current(number).filter(|c| !c.incoming && c.phase == CallPhase::Calling && c.worker.is_none())?;
        Some(CallCommand::Offer { call: number, ice_servers, mic, muted: c.muted, relay_only: !self.direct })
    }

    /// Our call `number` once its relay servers were asked for (`open`: calls are still open to the identity it was
    /// dialled with; `signed_in`: a direct call can still be signalled). Refused (`not_enabled`) or closed meanwhile, it
    /// ends: no worker starts, even for a direct call. Without relays, only a direct call goes on (without servers).
    fn dial_with_relays(&mut self, number: u64, relays: Relays, open: bool, signed_in: bool, mic: Option<String>) -> Acts {
        let fail = |rt: &mut Self, notice: Option<(NoticeKind, String)>| {
            let mut acts = if rt.current(number).is_some() { rt.finish(Ending::Failed, true) } else { Acts::default() };
            acts.notice = notice;
            acts
        };
        match relays {
            Relays::Denied => fail(self, Some((NoticeKind::Info, tr(super::calls_access::CALLS_SOON_TOLD).to_string()))),
            _ if !open => fail(self, None),
            Relays::Got(servers) => Acts { worker: self.offer(number, servers, mic).into_iter().collect(), ..Default::default() },
            Relays::Failed(e) if self.direct && signed_in => {
                eprintln!("calls ice-servers: {e}");
                Acts { worker: self.offer(number, Vec::new(), mic).into_iter().collect(), ..Default::default() }
            }
            Relays::Failed(e) => fail(self, Some((NoticeKind::Warning, tr!("Call: {message}", message = e)))),
        }
    }

    /// The press on Answer for ringing call `number`, once this side's relay servers were asked for. Only that call, still
    /// ringing, is answered, and only while calls are still open to the identity it was pressed with (`open`); a refusal
    /// declines it. Without relays it is answered as it came.
    fn answer_with_relays(&mut self, number: u64, relays: Relays, open: bool, mic: Option<String>, now: Instant) -> Acts {
        if self.call.as_ref().filter(|c| c.ringing_in()).map(|c| c.number) != Some(number) {
            return Acts::default();
        }
        match relays {
            Relays::Denied => return self.hang_up(),
            _ if !open => return self.hang_up(),
            Relays::Got(servers) => {
                if let Some((_, offered)) = self.call.as_mut().and_then(|c| c.offer.as_mut()) {
                    *offered = servers;
                }
            }
            Relays::Failed(e) => eprintln!("calls ice-servers: {e}"),
        }
        self.answer(mic, now)
    }

    /// The ringing call is answered here: the worker takes it, with the caller's candidates that came meanwhile.
    fn answer(&mut self, mic: Option<String>, now: Instant) -> Acts {
        let Some(c) = self.call.as_mut().filter(|c| c.ringing_in()) else { return Acts::default() };
        let Some((offer, ice_servers)) = c.offer.take() else { return Acts::default() };
        c.phase = CallPhase::Connecting;
        c.since = now;
        let mut acts = Acts { publish: true, ..Default::default() };
        acts.worker.push(CallCommand::Answer { call: c.number, offer, ice_servers, mic, muted: c.muted, relay_only: !self.direct });
        if !c.inbox.is_empty() {
            acts.worker.push(CallCommand::Candidates { call: c.number, candidates: std::mem::take(&mut c.inbox) });
        }
        acts
    }

    /// This side declines the ringing call, or hangs up.
    fn hang_up(&mut self) -> Acts {
        let Some(c) = &self.call else { return Acts::default() };
        let ending = if c.ringing_in() { Ending::DeclinedHere } else { Ending::Ended };
        self.finish(ending, true)
    }

    fn mute(&mut self, on: bool) -> Acts {
        let Some(c) = self.call.as_mut() else { return Acts::default() };
        c.muted = on;
        let mut acts = Acts { publish: true, ..Default::default() };
        // Before the worker has the call, the muting goes with its start command.
        if c.worker.is_some() {
            acts.worker.push(CallCommand::Mute { call: c.number, on });
        }
        acts
    }

    /// The call is over: the panel says how, the worker hangs up and, when this side ends it (`tell`), Orbiters
    /// hears it (declined when it rang here).
    fn finish(&mut self, ending: Ending, tell: bool) -> Acts {
        let mut acts = Acts { publish: true, ..Default::default() };
        let Some(c) = self.call.take() else { return acts };
        if c.worker.is_some() {
            acts.worker.push(CallCommand::Hangup { call: c.number });
        }
        if let (true, Some(id)) = (tell, c.id.clone()) {
            acts.signals.push(Signal::Close { id, decline: c.ringing_in() });
        }
        acts.say = Some(ending.text(&c.name, c.connected_at.is_some()));
        acts
    }

    /// One event of the server's stream. Those about another call than this one do nothing (an incoming one is
    /// declined: one call at a time).
    fn server_event(&mut self, event: &str, v: &Value, now: Instant) -> Acts {
        let id = v["call"].as_str().unwrap_or_default();
        if id.is_empty() {
            return Acts::default();
        }
        if event == "incoming" {
            return self.incoming(id, v, now);
        }
        let Some(c) = self.call.as_mut().filter(|c| c.id.as_deref() == Some(id)) else { return Acts::default() };
        let ending = match event {
            "answered" if !c.incoming && c.phase == CallPhase::Calling => {
                c.phase = CallPhase::Connecting;
                c.since = now;
                c.open = true;
                let mut acts = Acts { publish: true, ..Default::default() };
                acts.worker.push(CallCommand::RemoteAnswer { call: c.number, sdp: v["answer"].as_str().unwrap_or_default().to_string() });
                // Our candidates waited for this: the friend took the call.
                if !c.outbox.is_empty() {
                    acts.signals.push(Signal::Candidates { id: id.to_string(), candidates: std::mem::take(&mut c.outbox) });
                }
                return acts;
            }
            "candidates" => {
                let list: Vec<IceCandidate> = v["candidates"].as_array().map(|a| a.iter().filter_map(IceCandidate::from_value).collect()).unwrap_or_default();
                if list.is_empty() {
                    return Acts::default();
                }
                if c.worker.is_some() && !c.ringing_in() {
                    return Acts { worker: vec![CallCommand::Candidates { call: c.number, candidates: list }], ..Default::default() };
                }
                // It rings here: the worker gets them with the answer.
                c.inbox.extend(list);
                c.inbox.truncate(KEPT_MOST);
                return Acts::default();
            }
            "declined" => Ending::Declined,
            "ended" if c.ringing_in() => Ending::Missed,
            "ended" if v["reason"] == "timeout" => {
                if c.incoming {
                    Ending::Missed
                } else {
                    Ending::Unanswered
                }
            }
            "ended" => Ending::Ended,
            _ => return Acts::default(),
        };
        self.finish(ending, false)
    }

    fn incoming(&mut self, id: &str, v: &Value, now: Instant) -> Acts {
        let offer = v["offer"].as_str().unwrap_or_default();
        if let Some(c) = &self.call {
            if c.id.as_deref() == Some(id) {
                // The same call again (a stream that opened again).
                return Acts::default();
            }
        }
        if self.call.is_some() || offer.is_empty() {
            // One call at a time (and a call without an offer cannot be answered): the caller hears this side is busy.
            return Acts { signals: vec![Signal::Close { id: id.to_string(), decline: true }], ..Default::default() };
        }
        let name = v["from"]["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or_else(|| tr_latin("A friend")).to_string();
        self.numbered += 1;
        let mut c = Call::new(self.numbered, v["from"]["id"].as_str().unwrap_or_default().to_string(), name.clone(), true, now);
        c.id = Some(id.to_string());
        c.offer = Some((offer.to_string(), call::ice_servers(&v["ice_servers"])));
        self.call = Some(c);
        Acts {
            say: Some(fill(tr_latin("{name} is calling. Touch me or press Call to answer."), &[("name", &name)])),
            notice: Some((NoticeKind::Info, tr!("{name} is calling: answer on the N4N0 page, in the SteamVR tab or by touching N4N0.", name = name))),
            publish: true,
            ..Default::default()
        }
    }

    /// One event of the call worker. Those about another call than this one do nothing.
    fn worker_event(&mut self, e: CallEvent, now: Instant) -> Acts {
        match e {
            CallEvent::Ready => Acts::default(),
            CallEvent::Error { call, message } => {
                if call != 0 && self.current(call).is_none() {
                    return Acts::default();
                }
                Acts { notice: Some((NoticeKind::Warning, tr!("Call: {message}", message = message))), ..Default::default() }
            }
            CallEvent::Offer { call, sdp } => match self.current(call).filter(|c| !c.incoming) {
                Some(c) => Acts { signals: vec![Signal::Create { number: call, contact: c.contact.clone(), offer: sdp }], ..Default::default() },
                None => Acts::default(),
            },
            CallEvent::Answer { call, sdp } => {
                let Some(c) = self.current(call).filter(|c| c.incoming) else { return Acts::default() };
                let Some(id) = c.id.clone() else { return Acts::default() };
                c.open = true;
                let mut acts = Acts::default();
                acts.signals.push(Signal::Answer { number: call, id: id.clone(), sdp });
                if !c.outbox.is_empty() {
                    acts.signals.push(Signal::Candidates { id, candidates: std::mem::take(&mut c.outbox) });
                }
                acts
            }
            CallEvent::Candidate { call, candidate } => {
                let Some(c) = self.current(call) else { return Acts::default() };
                match (&c.id, c.open) {
                    (Some(id), true) => Acts { signals: vec![Signal::Candidates { id: id.clone(), candidates: vec![candidate] }], ..Default::default() },
                    _ => {
                        if c.outbox.len() < KEPT_MOST {
                            c.outbox.push(candidate);
                        }
                        Acts::default()
                    }
                }
            }
            CallEvent::Connected { call } => {
                let Some(c) = self.current(call) else { return Acts::default() };
                c.phase = CallPhase::Connected;
                c.connected_at = Some(now_ms());
                c.since = now;
                Acts { say: Some(fill(tr_latin("On a call with {name}."), &[("name", &c.name)])), publish: true, ..Default::default() }
            }
            CallEvent::Levels { call, mic, remote } => {
                let Some(c) = self.current(call) else { return Acts::default() };
                // Coarse steps: the meters move, and an unchanged level is not published again.
                let levels = (quantize(mic), quantize(remote));
                let changed = c.levels != levels;
                c.levels = levels;
                Acts { publish: changed, ..Default::default() }
            }
            CallEvent::Ended { call, reason } => {
                if self.current(call).is_none() {
                    return Acts::default();
                }
                let ending = match reason {
                    EndReason::Hangup | EndReason::Closed => Ending::Ended,
                    EndReason::Failed => Ending::Failed,
                    EndReason::Unreachable => Ending::Unreachable,
                };
                self.finish(ending, true)
            }
        }
    }

    /// Worker `number` ended: the call it carried is over.
    fn worker_exit(&mut self, number: u64) -> Acts {
        if !self.call.as_ref().is_some_and(|c| c.worker == Some(number)) {
            return Acts::default();
        }
        let mut acts = self.finish(Ending::Failed, true);
        acts.notice = Some((NoticeKind::Warning, tr("Call: the call worker stopped.").into()));
        acts
    }

    /// Orbiters gave our call `number` its id, or refused it.
    fn created(&mut self, number: u64, answer: Result<String, Ending>) -> Acts {
        match (answer, self.current(number).is_some()) {
            (Ok(id), true) => {
                if let Some(c) = self.current(number) {
                    c.id = Some(id);
                }
                Acts::default()
            }
            // Ended here before Orbiters answered: the friend's ringing ends too.
            (Ok(id), false) => Acts { signals: vec![Signal::Close { id, decline: false }], ..Default::default() },
            (Err(ending), true) => self.finish(ending, false),
            (Err(_), false) => Acts::default(),
        }
    }

    /// A request call `number` needed failed (its answer): the call is over.
    fn signal_failed(&mut self, number: u64) -> Acts {
        if self.current(number).is_none() {
            return Acts::default();
        }
        self.finish(Ending::Failed, true)
    }

    /// A call whose phase lasted too long ends (a ring whose end never came, a connection that never completes).
    fn deadlines(&mut self, now: Instant) -> Acts {
        let Some(c) = &self.call else { return Acts::default() };
        let waited = now.saturating_duration_since(c.since);
        let ending = match c.phase {
            CallPhase::Calling if waited > CALLING_FOR => Ending::Unanswered,
            CallPhase::Ringing if waited > RINGING_FOR => Ending::Missed,
            CallPhase::Connecting if waited > CONNECTING_FOR => Ending::Failed,
            _ => return Acts::default(),
        };
        self.finish(ending, true)
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What an account that calls leave tells its own server (`api`, with its credential), in the background: the call it
/// had ends there (declined when it was ringing here), and this computer leaves its callable list when it was on it.
fn old_server_leaves(api: Api, token: String, closing: Option<(String, bool)>, available: bool) {
    if closing.is_none() && !available {
        return;
    }
    let _ = std::thread::Builder::new().name("calls-leave-old".into()).spawn(move || {
        let agent = orbiters_core::http_agent(Some(Duration::from_secs(5)), Some(Duration::from_secs(10)));
        let auth = format!("Bearer {token}"); // i18n: not shown
        if let Some((id, decline)) = closing {
            let path = format!("n4n0/calls/{id}/{}", if decline { "decline" } else { "end" });
            let _ = agent.post(&api.url(&path)).header("Authorization", &auth).send_json(json!({}));
        }
        if available {
            let _ = agent.delete(&api.url("n4n0/calls/presence")).header("Authorization", &auth).call();
        }
    });
}

/// A level in twentieths.
fn quantize(level: f32) -> f32 {
    (level.clamp(0.0, 1.0) * 20.0).round() / 20.0
}

fn friends_digest(hashes: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

/// Orbiters refused: this VRChat account is not the member's verified link (403 `{"code": "not_linked"}` on calls; a
/// 403 without a code, as a server from before codes answered, counts too). Another 403 (a firewall page, a feature
/// refused) is not "link your VRChat account".
fn not_linked(e: &anyhow::Error) -> bool {
    refused_as(e, 403, "not_linked")
}

/// Whether `e` is Orbiters' answer `status` with the contract's `code` (or with no code at all).
fn refused_as(e: &anyhow::Error, status: u16, code: &str) -> bool {
    e.downcast_ref::<ApiError>().is_some_and(|a| a.status == status && a.code.as_deref().is_none_or(|c| c == code))
}

/// The panel's text for the call menu: four contacts around the chosen one, `>` before it.
pub fn menu_text(contacts: &[CallContact], picked: usize) -> String {
    if contacts.is_empty() {
        // Two blocks of the panel (eight cells each on most props): whole within a tenth of a second.
        return tr_latin("No friend online").into();
    }
    let picked = picked.min(contacts.len() - 1);
    let first = (picked / PAGE) * PAGE;
    contacts[first..(first + PAGE).min(contacts.len())]
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}{}", if first + i == picked { "> " } else { "  " }, c.name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which contact a radial at `value` (0 to 1) chooses among `count`.
pub fn pick(value: f32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    ((value.clamp(0.0, 1.0) * count as f32) as usize).min(count - 1)
}

/// Server-sent events, a line at a time.
#[derive(Default)]
struct Sse {
    event: String,
    data: String,
    too_big: bool,
}

impl Sse {
    /// The event a blank line completes: its name and data. Comments (`: ping`) and other fields give nothing; an
    /// event whose data passes [`EVENT_MOST`] is dropped.
    fn line(&mut self, line: &str) -> Option<(String, String)> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            let done = (!self.event.is_empty() && !self.too_big).then(|| (std::mem::take(&mut self.event), std::mem::take(&mut self.data)));
            self.event.clear();
            self.data.clear();
            self.too_big = false;
            return done;
        }
        if let Some(v) = line.strip_prefix("event:") {
            self.event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(v.strip_prefix(' ').unwrap_or(v));
            if self.data.len() > EVENT_MOST {
                self.data.clear();
                self.too_big = true;
            }
        }
        None
    }
}

/// One line of `input` into `line` (without its end), at most `most` bytes: a longer one is skipped whole (false).
/// `None` at the end.
fn read_line(input: &mut impl BufRead, line: &mut Vec<u8>, most: usize) -> std::io::Result<Option<bool>> {
    line.clear();
    let (mut any, mut skipped) = (false, false);
    loop {
        let buf = match input.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if buf.is_empty() {
            return Ok(any.then_some(!skipped));
        }
        any = true;
        let newline = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if !skipped && line.len() + chunk.len() <= most {
            line.extend_from_slice(chunk);
        } else {
            skipped = true;
            line.clear();
        }
        let used = newline.map_or(buf.len(), |i| i + 1);
        input.consume(used);
        if newline.is_some() {
            return Ok(Some(!skipped));
        }
    }
}

/// An HTTP client whose reads give up after `idle` without a byte: a connection that died without a word ends
/// instead of waiting forever. (ureq has only overall timeouts; this wraps its transport.)
fn idle_agent(connect: Duration, idle: Duration) -> ureq::Agent {
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout, Transport};

    #[derive(Debug)]
    struct Idle(Duration);

    #[derive(Debug)]
    struct IdleTransport<T> {
        inner: T,
        idle: Duration,
    }

    impl<In: Transport> Connector<In> for Idle {
        type Out = IdleTransport<In>;

        fn connect(&self, _: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
            Ok(chained.map(|inner| IdleTransport { inner, idle: self.0 }))
        }
    }

    impl<T: Transport> Transport for IdleTransport<T> {
        fn buffers(&mut self) -> &mut dyn Buffers {
            self.inner.buffers()
        }

        fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
            self.inner.transmit_output(amount, timeout)
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
            let idle = NextTimeout { after: self.idle.into(), reason: ureq::Timeout::RecvBody };
            self.inner.await_input(if idle.after < timeout.after { idle } else { timeout })
        }

        fn is_open(&mut self) -> bool {
            self.inner.is_open()
        }

        fn is_tls(&self) -> bool {
            self.inner.is_tls()
        }
    }

    let config = ureq::Agent::config_builder().tls_config(orbiters_core::http_tls()).timeout_connect(Some(connect)).build();
    ureq::Agent::with_parts(config, DefaultConnector::new().chain(Idle(idle)), DefaultResolver::default())
}

/// A request about a call, its answer as JSON (`null` when it has none).
fn post(api: &Api, token: &str, path: &str, body: &Value) -> anyhow::Result<Value> {
    let response = api
        .agent()
        .post(&api.url(path))
        .config()
        .timeout_global(Some(SIGNAL_TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(body)?;
    if response.status().as_u16() == 204 {
        return Ok(Value::Null);
    }
    Api::json::<Value>(response)
}

/// The relay servers a call is made with.
fn ice_servers(api: &Api, token: &str) -> anyhow::Result<Vec<IceServer>> {
    let response = api
        .agent()
        .get(&api.url("n4n0/calls/ice-servers"))
        .config()
        .timeout_global(Some(ICE_TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {token}"))
        .call()?;
    let v: Value = Api::json(response)?;
    Ok(call::ice_servers(&v["ice_servers"]))
}

impl super::Controller {
    /// Starts the presence thread (it does nothing while calls are off).
    pub(super) fn start_calls(self: &Shared) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("calls".into()).spawn(move || loop {
            if ctl.calls.lock().unwrap().stop {
                break;
            }
            ctl.calls_tick();
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    /// Signing out of Orbiters (PRIV-7): with the identity that leaves, while it is still there, the call under way is
    /// ended on Orbiters and this computer leaves the callable list; the next tick, signed out, ends the call here. Sent
    /// in the background (sign-out finishes offline too); nothing of the credential is kept for later.
    pub(super) fn calls_depart(&self) {
        let Some((api, token)) = self.account_api() else { return };
        let (was_on, call_id) = {
            let mut rt = self.calls.lock().unwrap();
            let call_id = rt.call.as_ref().and_then(|c| c.id.clone());
            let was_on = rt.available_sent.take().is_some();
            rt.streaming = None;
            rt.presence_sent = None;
            rt.identity = None;
            (was_on, call_id)
        };
        if !was_on && call_id.is_none() {
            return;
        }
        let _ = std::thread::Builder::new().name("calls-depart".into()).spawn(move || {
            let agent = orbiters_core::http_agent(Some(Duration::from_secs(2)), Some(Duration::from_secs(5)));
            let auth = format!("Bearer {token}"); // i18n: not shown
            if let Some(id) = call_id {
                let _ = agent.post(&api.url(&format!("n4n0/calls/{id}/end"))).header("Authorization", &auth).send_json(json!({}));
            }
            if was_on {
                let _ = agent.delete(&api.url("n4n0/calls/presence")).header("Authorization", &auth).call();
            }
        });
    }

    /// Calls stop (the app quits): the call worker ends, the call under way ends for the friend too, and this
    /// computer leaves the callable list.
    pub(super) fn calls_stop(self: &Shared) {
        let (api, was_on, worker, call_id) = {
            let mut rt = self.calls.lock().unwrap();
            rt.stop = true;
            rt.streaming = None;
            rt.signals = None;
            let call_id = rt.call.take().and_then(|c| c.id);
            (self.account_api(), rt.available_sent.is_some(), rt.worker.take(), call_id)
        };
        // Its end waits a moment for the process: not under the lock.
        drop(worker);
        let Some((api, token)) = api else { return };
        // Quitting never waits long on the network: Orbiters drops a presence that stopped coming anyway.
        let (done, wait) = mpsc::channel();
        let _ = std::thread::Builder::new().name("calls-leave".into()).spawn(move || {
            let agent = orbiters_core::http_agent(Some(Duration::from_secs(2)), Some(Duration::from_secs(3)));
            let auth = format!("Bearer {token}"); // i18n: not shown
            if let Some(id) = call_id {
                let _ = agent.post(&api.url(&format!("n4n0/calls/{id}/end"))).header("Authorization", &auth).send_json(json!({}));
            }
            if was_on {
                let _ = agent.delete(&api.url("n4n0/calls/presence")).header("Authorization", &auth).call();
            }
            let _ = done.send(());
        });
        let _ = wait.recv_timeout(Duration::from_secs(4));
    }

    /// Calls now go to `api` with `token`: when that is another server or account than the state was for, the other
    /// one is told this member is gone (its presence), a call under way ends, and its contacts, presence and stream are
    /// forgotten, so that no contact of one environment or account can be called in the other (audit F25).
    fn calls_follow_identity(self: &Shared, api: &Api, token: &str) {
        let now = (api.url(""), token.to_string());
        let old = {
            let mut rt = self.calls.lock().unwrap();
            match rt.identity.replace(now.clone()) {
                Some(old) if old != now => Some((old, rt.available_sent.is_some())),
                _ => None,
            }
        };
        let Some(((base, old_token), available)) = old else { return };
        // The call under way ends here at once, and on the server it was made on, with the credential it was made with:
        // never as a hang-up of the new account on the new server (verifier, PRIV-7/CORE2-3).
        let closing = {
            let mut rt = self.calls.lock().unwrap();
            let closing = rt.call.as_ref().and_then(|c| c.id.clone().map(|id| (id, c.ringing_in())));
            if let Some(ending) = rt.call.as_ref().map(|c| if c.ringing_in() { Ending::DeclinedHere } else { Ending::Ended }) {
                let acts = rt.finish(ending, false);
                self.calls_act(rt, acts);
            }
            closing
        };
        old_server_leaves(Api::at(&base), old_token, closing, available);
        self.calls_publish(|rt| {
            rt.contacts.clear();
            rt.contacts_read = None;
            rt.presence_sent = None;
            rt.available_sent = None;
            rt.friends_sent = None;
            rt.link = CallLink::default();
            rt.streaming = None;
            rt.stream_failures = 0;
            rt.stream_retry_at = None;
        });
    }

    /// The API and credential for calls (signed in), through the client kept for the server.
    fn calls_api(self: &Shared) -> Option<(Api, String)> {
        let (fresh, token) = self.account_api()?;
        let mut rt = self.calls.lock().unwrap();
        if !rt.client.as_ref().is_some_and(|c| c.url("") == fresh.url("")) {
            rt.client = Some(fresh);
        }
        rt.client.clone().map(|api| (api, token))
    }

    fn calls_tick(self: &Shared) {
        let settings = self.settings();
        // Whether this account has N4N0 calls, asked again when it is time to (whatever the setting: Call's tile shows it).
        self.calls_access_refresh();
        let open = self.calls_open();
        let on = open && settings.n4n0.enabled && settings.n4n0.calls;
        // A phase that lasted too long ends.
        {
            let mut rt = self.calls.lock().unwrap();
            let acts = rt.deadlines(Instant::now());
            self.calls_act(rt, acts);
        }
        let account = if on { self.calls_api() } else { None };
        if let Some((api, token)) = &account {
            self.calls_follow_identity(api, token);
        }
        let Some((api, token)) = account else {
            let was = {
                let mut rt = self.calls.lock().unwrap();
                // No calls any more: the stream ends at its next line (unless the shared 3D view keeps it,
                // `controller/shared_view.rs`), a call under way ends.
                if !self.shared_view_wants_stream() {
                    rt.streaming = None;
                }
                let was = rt.available_sent.take().is_some();
                if rt.call.is_some() {
                    let acts = rt.hang_up();
                    self.calls_act(rt, acts);
                }
                was
            };
            if was {
                if let Some((api, token)) = self.account_api() {
                    let agent = orbiters_core::http_agent(Some(Duration::from_secs(5)), Some(Duration::from_secs(10)));
                    let _ = agent.delete(&api.url("n4n0/calls/presence")).header("Authorization", &format!("Bearer {token}")).call();
                }
            }
            let link = if !open {
                CallLink::ComingSoon
            } else if !on {
                CallLink::Off
            } else {
                CallLink::SignedOut
            };
            self.calls_publish(|rt| {
                rt.link = link.clone();
                rt.contacts.clear();
            });
            return;
        };
        // This computer's VRChat account and friends.
        let read_due = self.calls.lock().unwrap().local_read.is_none_or(|t| t.elapsed() >= FRIENDS_EVERY);
        if read_due {
            let log_account = {
                let snap = self.snapshot.lock().unwrap();
                let w = &snap.vrchat.world;
                w.local_player.as_ref().and_then(|me| w.players.iter().find(|(_, n)| *n == me).map(|(id, _)| id.clone()))
            };
            let local = vrchat_accounts::read(log_account.as_deref());
            // Hashed once per read, not at every tick (up to 5000 of them).
            let hashes: Vec<String> = local.friends.iter().take(FRIENDS_MOST).map(|f| vrchat_accounts::friend_hash(f)).collect();
            let digest = friends_digest(&hashes);
            let mut rt = self.calls.lock().unwrap();
            rt.local = local;
            rt.local_read = Some(Instant::now());
            rt.friend_hashes = hashes;
            rt.friends_digest = digest;
        }
        let Some((account, source)) = self.calls.lock().unwrap().local.account.clone() else {
            self.calls_publish(|rt| {
                rt.link = CallLink::NoAccount;
                rt.streaming = None;
            });
            return;
        };
        let available = {
            let snap = self.snapshot.lock().unwrap();
            snap.vrchat.running && snap.n4n0.prop_sync.is_some()
        };
        let (due, friends) = {
            let rt = self.calls.lock().unwrap();
            let changed = rt.friends_sent != Some(rt.friends_digest);
            let due = rt.presence_sent.is_none_or(|t| t.elapsed() >= PRESENCE_EVERY) || rt.available_sent != Some(available) || changed;
            (due, changed.then(|| (rt.friend_hashes.clone(), rt.friends_digest)))
        };
        if due {
            let mut body = json!({ "available": available, "vrchat_id": account });
            if let Some((hashes, _)) = &friends {
                body["friends"] = json!(hashes);
                body["friends_changed"] = json!(true);
            }
            let answer = api
                .agent()
                .put(&api.url("n4n0/calls/presence"))
                .config()
                .timeout_global(Some(SIGNAL_TIMEOUT))
                .build()
                .header("Authorization", &format!("Bearer {token}"))
                .send_json(&body);
            match answer.map_err(anyhow::Error::from).and_then(Api::json::<Value>) {
                Ok(v) => {
                    let mut rt = self.calls.lock().unwrap();
                    rt.presence_sent = Some(Instant::now());
                    rt.available_sent = Some(available);
                    if let Some((_, digest)) = friends {
                        rt.friends_sent = Some(digest);
                    }
                    if v["friends_known"] == json!(false) {
                        rt.friends_sent = None;
                    }
                    rt.link = CallLink::Linked { account: account.clone() };
                }
                // The account has not N4N0 calls (switched off since): closed here at once, the next look says so.
                Err(e) if super::calls_access::not_enabled(&e) => {
                    self.calls.lock().unwrap().presence_sent = Some(Instant::now());
                    self.calls_access_denied(&token);
                    return;
                }
                Err(e) => {
                    let refused = not_linked(&e);
                    let mut rt = self.calls.lock().unwrap();
                    rt.presence_sent = Some(Instant::now());
                    if refused {
                        rt.link = CallLink::NotLinked { account: account.clone(), source: source.name().to_string() };
                    } else {
                        eprintln!("calls presence: {e:#}");
                    }
                }
            }
        }
        let linked = matches!(self.calls.lock().unwrap().link, CallLink::Linked { .. });
        if !linked {
            self.calls_publish(|rt| rt.streaming = None);
            return;
        }
        self.calls_stream(&api, &token);
        let contacts_due = self.calls.lock().unwrap().contacts_read.is_none_or(|t| t.elapsed() >= CONTACTS_EVERY);
        if contacts_due && available {
            let answer = api
                .agent()
                .get(&api.url("n4n0/calls/contacts"))
                .config()
                .timeout_global(Some(SIGNAL_TIMEOUT))
                .build()
                .header("Authorization", &format!("Bearer {token}"))
                .call();
            match answer.map_err(anyhow::Error::from).and_then(Api::json::<Value>) {
                Ok(v) => {
                    let contacts: Vec<CallContact> = v["contacts"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|c| Some(CallContact { id: c["id"].as_str()?.to_string(), name: c["name"].as_str()?.to_string() })).collect())
                        .unwrap_or_default();
                    let mut rt = self.calls.lock().unwrap();
                    rt.contacts = contacts;
                    rt.contacts_read = Some(Instant::now());
                }
                Err(e) => {
                    // Asked again after the usual wait, not at every tick (verify A, contacts refused).
                    self.calls.lock().unwrap().contacts_read = Some(Instant::now());
                    if super::calls_access::not_enabled(&e) {
                        self.calls_access_denied(&token);
                        return;
                    }
                    eprintln!("calls contacts: {e:#}");
                }
            }
        }
        self.calls_publish(|_| {});
    }

    /// Keeps the event stream open on a thread of its own: opened again (for this credential) when it ended,
    /// failed or went silent, after a pause that grows while it keeps failing.
    pub(super) fn calls_stream(self: &Shared, api: &Api, token: &str) {
        let number = {
            let mut rt = self.calls.lock().unwrap();
            if rt.streaming.as_ref().is_some_and(|(_, t)| t != token) {
                // Another account: that stream ends at its next line.
                rt.streaming = None;
            }
            if rt.streaming.is_some() || rt.stream_retry_at.is_some_and(|t| Instant::now() < t) {
                return;
            }
            rt.streams += 1;
            let number = rt.streams;
            rt.streaming = Some((number, token.to_string()));
            number
        };
        let ctl = self.clone();
        let (api, token) = (api.clone(), token.to_string());
        let _ = std::thread::Builder::new().name("calls-events".into()).spawn(move || {
            let result = ctl.calls_events(&api, &token, number);
            let mut rt = ctl.calls.lock().unwrap();
            if rt.streaming.as_ref().is_some_and(|(n, _)| *n == number) {
                rt.streaming = None;
            }
            match result {
                // It ran: opened again in a moment.
                Ok(true) => {
                    rt.stream_failures = 0;
                    rt.stream_retry_at = Some(Instant::now() + STREAM_RETRY);
                }
                Ok(false) | Err(_) => {
                    if let Err(e) = &result {
                        eprintln!("calls events: {e:#}");
                    }
                    rt.stream_failures += 1;
                    let pause = STREAM_RETRY.saturating_mul(1 << (rt.stream_failures - 1).min(5)).min(STREAM_RETRY_MOST);
                    rt.stream_retry_at = Some(Instant::now() + pause);
                }
            }
        });
    }

    /// Reads the server's event stream until it ends, fails, stays silent past [`STREAM_IDLE`] or is replaced. True
    /// when it delivered something.
    fn calls_events(self: &Shared, api: &Api, token: &str, number: u64) -> anyhow::Result<bool> {
        let agent = idle_agent(Duration::from_secs(15), STREAM_IDLE);
        let response =
            agent.get(&api.url("n4n0/calls/events")).header("Authorization", &format!("Bearer {token}")).header("Accept", "text/event-stream").call()?;
        let mut reader = BufReader::new(response.into_body().into_with_config().limit(u64::MAX).reader());
        let mut sse = Sse::default();
        let mut line = Vec::new();
        let mut delivered = false;
        while let Some(whole) = read_line(&mut reader, &mut line, EVENT_MOST)? {
            delivered = true;
            {
                let rt = self.calls.lock().unwrap();
                if rt.stop || rt.streaming.as_ref().is_none_or(|(n, _)| *n != number) {
                    break;
                }
            }
            if !whole {
                continue;
            }
            if let Some((event, data)) = sse.line(&String::from_utf8_lossy(&line)) {
                let value: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                self.on_call_event(&event, value, token);
            }
        }
        Ok(delivered)
    }

    /// One event of the server's stream.
    /// One event of the stream opened with credential `token`.
    fn on_call_event(self: &Shared, event: &str, v: Value, token: &str) {
        // Orbiters closed calls for this credential (the feature switched off): closed here at once.
        if event == "calls_closed" {
            self.calls_access_denied(token);
            return;
        }
        // The shared 3D view's events come on the same stream.
        if event.starts_with("share_") {
            self.shared_view_event(event, &v);
            return;
        }
        // Nothing of a call reaches an account without N4N0 calls (Orbiters sends it none either).
        if !self.calls_open() {
            return;
        }
        let mut rt = self.calls.lock().unwrap();
        let acts = rt.server_event(event, &v, Instant::now());
        self.calls_act(rt, acts);
    }

    /// One event of the call worker.
    fn on_call_worker(self: &Shared, e: CallEvent) {
        let mut rt = self.calls.lock().unwrap();
        let acts = rt.worker_event(e, Instant::now());
        self.calls_act(rt, acts);
    }

    /// Worker `number`'s output ended: it is gone, and so is the call it carried.
    fn on_call_worker_exit(self: &Shared, number: u64) {
        let mut rt = self.calls.lock().unwrap();
        let gone = if rt.worker.as_ref().is_some_and(|w| w.number == number) { rt.worker.take() } else { None };
        let acts = rt.worker_exit(number);
        self.calls_act(rt, acts);
        drop(gone);
    }

    /// Does what a change of the calls asks: the worker's commands and the requests to Orbiters in order, under the
    /// lock (so two threads' changes never cross), then what the panel says and the published status.
    fn calls_act(self: &Shared, mut rt: MutexGuard<'_, CallsRuntime>, mut acts: Acts) {
        let mut gone = Vec::new();
        let mut said = Vec::new();
        let mut notices = Vec::new();
        let mut publish = false;
        loop {
            let mut next = None;
            for command in std::mem::take(&mut acts.worker) {
                if let Err(e) = self.calls_send(&mut rt, &command, &mut gone) {
                    notices.push((NoticeKind::Error, e));
                    // Nothing carries the call it starts.
                    if command.call().is_some_and(|n| rt.current(n).is_some()) {
                        next = Some(rt.finish(Ending::Failed, true));
                    }
                    break;
                }
            }
            for signal in std::mem::take(&mut acts.signals) {
                self.calls_signal(&mut rt, signal);
            }
            said.extend(acts.say.take());
            notices.extend(acts.notice.take());
            publish |= acts.publish;
            match next {
                Some(more) => acts = more,
                None => break,
            }
        }
        drop(rt);
        drop(gone);
        for text in said {
            self.n4n0_say(&text);
        }
        for (kind, message) in notices {
            self.notify(kind, message);
        }
        if publish {
            self.calls_publish(|_| {});
        }
    }

    /// Sends `command` to the call worker. A command that starts a call starts the worker when there is none, and
    /// goes to a new one when the current one cannot be written to (it ended); another command is lost with the
    /// call it was for, whose end the old worker's exit reports. The workers that went are left in `gone`, to be
    /// dropped once the lock is let go.
    fn calls_send(self: &Shared, rt: &mut CallsRuntime, command: &CallCommand, gone: &mut Vec<Worker>) -> Result<(), String> {
        let starts = matches!(command, CallCommand::Offer { .. } | CallCommand::Answer { .. });
        for _ in 0..2 {
            if rt.worker.is_none() {
                if !starts {
                    return Ok(());
                }
                rt.workers += 1;
                match Worker::spawn(self.clone(), rt.workers) {
                    Ok(w) => rt.worker = Some(w),
                    Err(e) => return Err(format!("{e:#}")),
                }
            }
            let w = rt.worker.as_mut().expect("there is one");
            match w.send(command) {
                Ok(()) => {
                    let number = w.number;
                    if starts {
                        if let Some(c) = command.call().and_then(|n| rt.current(n)) {
                            c.worker = Some(number);
                        }
                    }
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("call worker: {e}");
                    gone.extend(rt.worker.take());
                    if !starts {
                        return Ok(());
                    }
                }
            }
        }
        Err(tr("The call worker could not be started.").into())
    }

    /// Hands a request to the thread that sends them (started when there is none; none once calls stopped).
    fn calls_signal(self: &Shared, rt: &mut CallsRuntime, signal: Signal) {
        if rt.stop {
            return;
        }
        let signal = match &rt.signals {
            Some(tx) => match tx.send(signal) {
                Ok(()) => return,
                Err(mpsc::SendError(s)) => s,
            },
            None => signal,
        };
        let (tx, rx) = mpsc::channel();
        let ctl = self.clone();
        if std::thread::Builder::new().name("call-signals".into()).spawn(move || ctl.calls_signals(rx)).is_ok() {
            let _ = tx.send(signal);
            rt.signals = Some(tx);
        }
    }

    /// The requests about calls, in order; a call's candidates that come within [`BATCH`] go in one request.
    fn calls_signals(self: &Shared, rx: mpsc::Receiver<Signal>) {
        let mut carried: Option<Signal> = None;
        loop {
            let signal = match carried.take() {
                Some(s) => s,
                None => match rx.recv() {
                    Ok(s) => s,
                    Err(_) => return,
                },
            };
            // Signed out: nothing can be sent (and the call cannot go on either).
            let Some((api, token)) = self.calls_api() else { continue };
            match signal {
                Signal::Candidates { id, mut candidates } => {
                    let until = Instant::now() + BATCH;
                    loop {
                        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                            Ok(Signal::Candidates { id: more_id, candidates: more }) if more_id == id => candidates.extend(more),
                            Ok(other) => {
                                carried = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    if let Err(e) = post(&api, &token, &format!("n4n0/calls/{id}/candidates"), &json!({ "candidates": candidates })) {
                        eprintln!("calls candidates: {e:#}");
                        if super::calls_access::not_enabled(&e) {
                            self.calls_access_denied(&token);
                        }
                    }
                }
                Signal::Create { number, contact, offer } => {
                    let answer = match post(&api, &token, "n4n0/calls", &json!({ "contact": contact, "offer": offer })) {
                        Ok(v) => v["call"].as_str().filter(|id| !id.is_empty()).map(str::to_string).ok_or(Ending::Failed),
                        Err(e) if refused_as(&e, 409, "busy") => Err(Ending::Busy),
                        Err(e) if super::calls_access::not_enabled(&e) => {
                            if self.calls_access_denied(&token) {
                                self.notify(NoticeKind::Info, tr(super::calls_access::CALLS_SOON_TOLD));
                            }
                            Err(Ending::Failed)
                        }
                        Err(e) => {
                            eprintln!("calls n4n0/calls: {e:#}");
                            self.notify(NoticeKind::Warning, tr!("Call: {message}", message = e));
                            Err(Ending::Failed)
                        }
                    };
                    let mut rt = self.calls.lock().unwrap();
                    let acts = rt.created(number, answer);
                    self.calls_act(rt, acts);
                }
                Signal::Answer { number, id, sdp } => {
                    if let Err(e) = post(&api, &token, &format!("n4n0/calls/{id}/answer"), &json!({ "answer": sdp })) {
                        eprintln!("calls answer: {e:#}");
                        if super::calls_access::not_enabled(&e) {
                            if self.calls_access_denied(&token) {
                                self.notify(NoticeKind::Info, tr(super::calls_access::CALLS_SOON_TOLD));
                            }
                        } else {
                            self.notify(NoticeKind::Warning, tr!("Call: {message}", message = e));
                        }
                        let mut rt = self.calls.lock().unwrap();
                        let acts = rt.signal_failed(number);
                        self.calls_act(rt, acts);
                    }
                }
                Signal::Close { id, decline } => {
                    let path = format!("n4n0/calls/{id}/{}", if decline { "decline" } else { "end" });
                    if let Err(e) = post(&api, &token, &path, &json!({})) {
                        eprintln!("calls {path}: {e:#}");
                    }
                }
            }
        }
    }

    /// Says why calls cannot be used here when they are not open to this account (`controller/calls_access.rs`): every
    /// way into a call asks this first. True when they are open.
    fn calls_open_or_say(self: &Shared) -> bool {
        if self.calls_open() {
            return true;
        }
        self.notify(NoticeKind::Info, tr(super::calls_access::CALLS_SOON_TOLD));
        false
    }

    /// Calls a contact (the window, the overlay or N4N0's menu).
    pub fn call_contact(self: &Shared, contact: String) {
        if !self.calls_open_or_say() {
            return;
        }
        let (number, name) = {
            let mut rt = self.calls.lock().unwrap();
            let Some(name) = rt.contacts.iter().find(|c| c.id == contact).map(|c| c.name.clone()) else {
                drop(rt);
                self.notify(NoticeKind::Warning, tr("This friend cannot be called right now."));
                return;
            };
            let Some(number) = rt.dial(&contact, &name, Instant::now()) else { return };
            rt.menu_open = false;
            (number, name)
        };
        self.calls_publish(|_| {});
        self.n4n0_say(&fill(tr_latin("Calling {name}…"), &[("name", &name)]));
        let whose = self.calls_identity().map(|(w, ..)| w);
        let ctl = self.clone();
        std::thread::spawn(move || {
            let relays = ctl.relays();
            // Checked again once the relays came: the same identity, calls still open to it.
            let open = ctl.calls_still_open_to(&whose);
            let signed_in = ctl.account_api().is_some();
            let (mic, direct) = ctl.with_settings(|s| (s.n4n0.mic_device.clone(), s.n4n0.direct_calls));
            let mic = (!mic.is_empty()).then_some(mic);
            let mut rt = ctl.calls.lock().unwrap();
            rt.direct = direct;
            let acts = rt.dial_with_relays(number, relays, open, signed_in, mic);
            ctl.calls_act(rt, acts);
        });
    }

    /// Answers the ringing call.
    pub fn call_answer(self: &Shared) {
        if !self.calls_open_or_say() {
            return;
        }
        let (mic, direct) = self.with_settings(|s| (s.n4n0.mic_device.clone(), s.n4n0.direct_calls));
        let mic = (!mic.is_empty()).then_some(mic);
        let mut rt = self.calls.lock().unwrap();
        rt.direct = direct;
        let without_relays = rt.call.as_ref().filter(|c| c.ringing_in()).and_then(|c| c.offer.as_ref()).is_some_and(|(_, servers)| servers.is_empty());
        if without_relays && !direct {
            // The call came without relay servers: this side asks for its own first (not on this thread, which may
            // be the one OSC arrives on). The answer is for this call, pressed by this identity.
            let number = rt.call.as_ref().map(|c| c.number).unwrap_or_default();
            drop(rt);
            let whose = self.calls_identity().map(|(w, ..)| w);
            let ctl = self.clone();
            std::thread::spawn(move || {
                let relays = ctl.relays();
                let open = ctl.calls_still_open_to(&whose);
                let mut rt = ctl.calls.lock().unwrap();
                let acts = rt.answer_with_relays(number, relays, open, mic, Instant::now());
                ctl.calls_act(rt, acts);
            });
            return;
        }
        let acts = rt.answer(mic, Instant::now());
        self.calls_act(rt, acts);
    }

    /// Declines a ringing call or hangs up.
    pub fn call_end(self: &Shared) {
        let mut rt = self.calls.lock().unwrap();
        let acts = rt.hang_up();
        self.calls_act(rt, acts);
    }

    pub fn call_mute(self: &Shared, on: bool) {
        let mut rt = self.calls.lock().unwrap();
        let acts = rt.mute(on);
        self.calls_act(rt, acts);
    }

    // ---- N4N0's Call menu (avatar parameters, contracts N4N0/Call/*) ----

    /// `N4N0/Call/Menu` pressed: the contacts on the panel, or off it.
    pub(super) fn calls_menu(self: &Shared) {
        // Not open to this account: N4N0's panel says so, and no list opens.
        if !self.calls_open() {
            self.calls.lock().unwrap().menu_open = false;
            // The panel's letters (no Japanese) and its width: the short words.
            self.n4n0_say_quick(tr_latin(orbiters_n4n0_core::menu::CALLS_SOON_WHY));
            return;
        }
        let (open, text) = {
            let mut rt = self.calls.lock().unwrap();
            rt.menu_open = !rt.menu_open;
            (rt.menu_open, menu_text(&rt.contacts, rt.picked))
        };
        if open {
            // The owner opened it to pick from: it shows at once (Docs/n4n0-calls.md, Call menu).
            self.n4n0_say_quick(&text);
        } else {
            self.n4n0_command(crate::ipc::N4n0Command::HidePanel);
        }
        self.calls_publish(|_| {});
    }

    /// `N4N0/Call/Pick` moved.
    pub(super) fn calls_pick(self: &Shared, value: f32) {
        if !self.calls_open() {
            return;
        }
        let text = {
            let mut rt = self.calls.lock().unwrap();
            let picked = pick(value, rt.contacts.len());
            if picked == rt.picked || !rt.menu_open {
                rt.picked = picked;
                None
            } else {
                rt.picked = picked;
                Some(menu_text(&rt.contacts, picked))
            }
        };
        if let Some(text) = text {
            self.n4n0_say_quick(&text);
        }
        self.calls_publish(|_| {});
    }

    /// `N4N0/Call/Go` (or a touch while it rings): answers a ringing call, or calls the chosen contact.
    pub(super) fn calls_go(self: &Shared) -> bool {
        // Not open to this account: the touch is N4N0's own (no call rings, no list is open).
        if !self.calls_open() {
            return false;
        }
        let (ringing, chosen) = {
            let rt = self.calls.lock().unwrap();
            let ringing = rt.call.as_ref().is_some_and(Call::ringing_in);
            let chosen = (rt.menu_open && rt.call.is_none()).then(|| rt.contacts.get(rt.picked).map(|c| c.id.clone())).flatten();
            (ringing, chosen)
        };
        if ringing {
            self.call_answer();
            return true;
        }
        if let Some(id) = chosen {
            self.call_contact(id);
            return true;
        }
        false
    }

    /// Whether a call rings for this member (touching N4N0 answers it).
    pub(super) fn call_ringing_in(&self) -> bool {
        self.calls.lock().unwrap().call.as_ref().is_some_and(Call::ringing_in)
    }

    /// Calls were closed for this account (`calls_access_denied`): a call under way ends (the other side is told), the
    /// friends to call go, and everything shows "Coming soon" now rather than at the next look; N4N0's call menu, when
    /// it was open on the panel, goes too.
    pub(super) fn calls_closed_here(self: &Shared, menu_was_open: bool) {
        {
            let mut rt = self.calls.lock().unwrap();
            if rt.call.is_some() {
                let acts = rt.hang_up();
                self.calls_act(rt, acts);
            }
        }
        if menu_was_open {
            self.n4n0_command(crate::ipc::N4n0Command::HidePanel);
        }
        self.calls_publish(|rt| {
            rt.link = CallLink::ComingSoon;
            rt.contacts.clear();
            rt.contacts_read = None;
            rt.available_sent = None;
            rt.presence_sent = None;
        });
    }

    fn calls_publish(self: &Shared, change: impl FnOnce(&mut CallsRuntime)) {
        let status = {
            let mut rt = self.calls.lock().unwrap();
            change(&mut rt);
            crate::state::CallsStatus {
                link: rt.link.clone(),
                contacts: rt.contacts.clone(),
                call: rt.call.as_ref().map(|c| CallView {
                    name: c.name.clone(),
                    phase: c.phase,
                    incoming: c.incoming,
                    connected_at_ms: c.connected_at,
                    muted: c.muted,
                    mic_level: c.levels.0,
                    remote_level: c.levels.1,
                }),
                menu_open: rt.menu_open,
                picked: rt.picked,
            }
        };
        self.update_if(|s| s.n4n0.calls.set(status));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifier (PRIV-7/CORE2-3): switching accounts during a call ends it here at once and sends nothing for it through
    /// the new account's signals (its hang-up goes to the old server, with the old credential).
    #[test]
    fn an_account_switch_ends_the_call_without_the_new_accounts_signals() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        let (tx, rx) = mpsc::channel();
        {
            let mut rt = ctl.calls.lock().unwrap();
            rt.signals = Some(tx);
            rt.identity = Some(("http://127.0.0.1:1/".into(), "old-credential".into()));
            let number = rt.dial("c_1", "Kite", Instant::now()).unwrap();
            let _ = rt.created(number, Ok("call-1".into()));
            while rx.try_recv().is_ok() {}
        }
        ctl.calls_follow_identity(&Api::at("http://127.0.0.1:2/"), "new-credential");
        assert!(ctl.calls.lock().unwrap().call.is_none(), "ended here");
        assert!(rx.try_recv().is_err(), "nothing for the old call goes through the new account's signals");
        assert_eq!(ctl.calls.lock().unwrap().identity.as_ref().map(|i| i.1.as_str()), Some("new-credential"));
    }

    /// CORE2-11: "link your VRChat account" and "busy" are Orbiters' own refusals (their code), not any 403 or 409 of
    /// something in front of it; a server from before codes still reads as before.
    #[test]
    fn refusals_are_read_by_their_code() {
        let e = |status: u16, code: Option<&str>| anyhow::Error::from(ApiError { status, message: String::new(), code: code.map(str::to_string) });
        assert!(not_linked(&e(403, Some("not_linked"))) && not_linked(&e(403, None)));
        assert!(!not_linked(&e(403, Some("feature_disabled"))), "another 403 is not about the link");
        assert!(!not_linked(&e(401, None)) && !not_linked(&anyhow::anyhow!("offline")));
        assert!(refused_as(&e(409, Some("busy")), 409, "busy") && refused_as(&e(409, None), 409, "busy"));
        assert!(!refused_as(&e(409, Some("conflict")), 409, "busy"));
    }

    /// A controller signed in as account 42 (in memory only: no account file is read and nothing goes to a server, as the
    /// account was never read for a server), whose session check said `allowed` of its N4N0 calls.
    fn signed_in_controller(allowed: bool) -> crate::controller::Shared {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        ctl.account_runtime.lock().unwrap().account =
            Some(orbiters_core::account::Account { token: "orbit-test".into(), user: Some("42".into()), username: Some("Nova".into()), avatar_url: None, features: None });
        ctl.update(|s| s.account.state = crate::state::AccountState::SignedIn { username: "Nova".into() });
        let features = std::collections::BTreeMap::from([(orbiters_core::features::N4N0_CALLS.to_string(), allowed)]);
        ctl.calls_access_checked(orbiters_core::account::Server::current(), Some("42"), "orbit-test", Some(&features));
        ctl
    }

    /// A friend's call rings, as the server's stream brings it (with a relay, so an answer goes to the worker at once).
    fn ring(ctl: &crate::controller::Shared, id: &str) {
        ctl.on_call_event("incoming", ringing(id), "orbit-test");
    }

    fn ringing(id: &str) -> Value {
        json!({ "call": id, "from": { "id": "c_9", "name": "Kite" }, "offer": "v=0", "ice_servers": [{ "urls": "turn:relay.example.invalid" }] })
    }

    /// Request15 10: every way into a call asks `Controller::calls_open` (`controller/calls_access.rs`). Without N4N0 calls none starts anything (calling
    /// a friend from the window, the overlay or a menu, being rung, answering, the avatar's Call menu, pick and go), and
    /// Call is published as "Coming soon"; with them each does what it did before.
    #[test]
    fn every_way_into_a_call_is_closed_without_the_feature_and_open_with_it() {
        for allowed in [false, true] {
            let ctl = signed_in_controller(allowed);
            assert_eq!(ctl.calls_open(), allowed);
            let friends = vec![crate::state::CallContact { id: "c_1".into(), name: "Ada".into() }, crate::state::CallContact { id: "c_2".into(), name: "Ben".into() }];
            ctl.calls.lock().unwrap().contacts = friends;
            // Calling a friend (the window's N4N0 page, the overlay's and the avatar's menus all end here).
            ctl.call_contact("c_1".into());
            assert_eq!(ctl.calls.lock().unwrap().numbered, u64::from(allowed), "a call was dialled: {allowed}");
            ctl.calls.lock().unwrap().call = None;
            // The avatar's Call menu, its pick and its go.
            ctl.calls_menu();
            assert_eq!(ctl.calls.lock().unwrap().menu_open, allowed, "the Call menu: {allowed}");
            ctl.calls_pick(1.0);
            assert_eq!(ctl.calls.lock().unwrap().picked, if allowed { 1 } else { 0 }, "pick: {allowed}");
            ctl.calls.lock().unwrap().menu_open = false;
            // Being rung.
            ring(&ctl, "k_1");
            assert_eq!(ctl.call_ringing_in(), allowed, "rung: {allowed}");
            // Answering by the avatar's Call/Go, then by Answer (a press, or touching N4N0, which sends Answer): each on a
            // call that rings (without the feature, one put there as if it had rung). The call worker is not beside the test program: it is asked for, and the call ends.
            let rung_anyway = |ctl: &crate::controller::Shared, id: &str| {
                if allowed {
                    ring(ctl, id);
                } else {
                    let mut rt = ctl.calls.lock().unwrap();
                    let _ = rt.server_event("incoming", &ringing(id), Instant::now());
                }
            };
            ctl.calls.lock().unwrap().call = None;
            rung_anyway(&ctl, "k_2");
            let workers = ctl.calls.lock().unwrap().workers;
            assert_eq!(ctl.calls_go(), allowed, "Call/Go answers: {allowed}");
            assert_eq!(ctl.calls.lock().unwrap().workers > workers, allowed, "Call/Go asked the worker to answer: {allowed}");
            ctl.calls.lock().unwrap().call = None;
            rung_anyway(&ctl, "k_3");
            let workers = ctl.calls.lock().unwrap().workers;
            ctl.call_answer();
            assert_eq!(ctl.calls.lock().unwrap().workers > workers, allowed, "Answer asked the worker to answer: {allowed}");
            // What every surface reads: "Coming soon" without the feature.
            ctl.calls.lock().unwrap().call = None;
            ctl.calls_tick();
            let link = ctl.snapshot.lock().unwrap().n4n0.calls.link.clone();
            assert_eq!(link == crate::state::CallLink::ComingSoon, !allowed, "{link:?}");
            assert!(ctl.snapshot.lock().unwrap().n4n0.calls.call.is_none() || allowed, "no call to show, so no \"Talk with N4N0?\" either");
        }
    }

    /// Verify A2: an answer or a refusal comes back for the credential it was asked with. After another account signed in
    /// (account switch while it was on its way), it never opens nor closes the new account's calls.
    #[test]
    fn answers_for_the_account_before_a_switch_never_touch_the_next_one() {
        let server = orbiters_core::account::Server::current();
        let yes = std::collections::BTreeMap::from([(orbiters_core::features::N4N0_CALLS.to_string(), true)]);
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        let sign_in = |user: &str, token: &str| {
            ctl.account_runtime.lock().unwrap().account =
                Some(orbiters_core::account::Account { token: token.into(), user: Some(user.into()), username: Some(user.into()), avatar_url: None, features: None });
        };
        ctl.update(|s| s.account.state = crate::state::AccountState::SignedIn { username: "B".into() });
        // B signed in, without calls; A's yes (asked before the switch) arrives: B stays closed.
        sign_in("2", "orbit-b");
        ctl.calls_access_checked(server, Some("1"), "orbit-a", Some(&yes));
        assert!(!ctl.calls_open(), "A's yes is not B's");
        // B has calls; A's refusal arrives late: B stays open.
        ctl.calls_access_checked(server, Some("2"), "orbit-b", Some(&yes));
        assert!(ctl.calls_open());
        assert!(!ctl.calls_access_denied("orbit-a"), "A's refusal is not B's");
        assert!(ctl.calls_open());
        // B's own refusal closes B, and clears what was shown.
        ctl.calls.lock().unwrap().contacts = vec![crate::state::CallContact { id: "c_1".into(), name: "Ada".into() }];
        assert!(ctl.calls_access_denied("orbit-b"));
        assert!(!ctl.calls_open());
        assert!(ctl.calls.lock().unwrap().contacts.is_empty());
        assert_eq!(ctl.snapshot.lock().unwrap().n4n0.calls.link, crate::state::CallLink::ComingSoon);
        // An account without an id: told apart by its credential, never sharing another's answer.
        ctl.account_runtime.lock().unwrap().account =
            Some(orbiters_core::account::Account { token: "orbit-c".into(), user: None, username: Some("".into()), avatar_url: None, features: None });
        assert!(!ctl.calls_open());
        ctl.calls_access_checked(server, None, "orbit-d", Some(&yes));
        assert!(!ctl.calls_open(), "another credential without an id is another identity");
    }

    /// Verify A, should-fix: Orbiters closing calls (`calls_closed` on the stream of this credential) closes them here at
    /// once, with the friends to call and N4N0's call menu; one for another credential does nothing.
    #[test]
    fn calls_closed_on_the_stream_closes_calls_for_that_credential_only() {
        let ctl = signed_in_controller(true);
        ctl.calls.lock().unwrap().contacts = contacts(2);
        ctl.calls.lock().unwrap().menu_open = true;
        ctl.on_call_event("calls_closed", json!({ "code": "not_enabled" }), "orbit-someone-else");
        assert!(ctl.calls_open(), "another credential's stream");
        ring(&ctl, "k_1");
        assert!(ctl.call_ringing_in());
        ctl.on_call_event("calls_closed", json!({ "code": "not_enabled" }), "orbit-test");
        assert!(!ctl.calls_open());
        let rt = ctl.calls.lock().unwrap();
        assert!(rt.contacts.is_empty() && !rt.menu_open && rt.call.is_none(), "nothing of calls is left");
    }

    /// Signed out, or the server's word not come yet: closed. A refusal of a call request closes calls at once.
    #[test]
    fn calls_close_when_signed_out_unknown_or_refused() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        assert!(!ctl.calls_open(), "nobody signed in");
        ctl.calls_tick();
        assert_eq!(ctl.snapshot.lock().unwrap().n4n0.calls.link, crate::state::CallLink::ComingSoon);
        let ctl = signed_in_controller(true);
        assert!(ctl.calls_access_denied("orbit-test"));
        assert!(!ctl.calls_open());
        // A session check from an older server says nothing of features: what was known stays.
        let ctl = signed_in_controller(true);
        ctl.calls_access_checked(orbiters_core::account::Server::current(), Some("42"), "orbit-test", None);
        assert!(ctl.calls_open());
    }

    /// Verify A3: a relay refusal is final, even for a direct call: no worker starts, the call ends, and it says why. A
    /// failure that is not a refusal still lets a direct call go on; calls closed meanwhile end it.
    #[test]
    fn a_relay_refusal_never_starts_a_worker() {
        let mut rt = CallsRuntime { direct: true, ..Default::default() };
        let number = rt.dial("c_1", "Ada", Instant::now()).unwrap();
        let acts = rt.dial_with_relays(number, Relays::Denied, true, true, None);
        assert!(acts.worker.is_empty(), "no offer: {:?}", acts.worker);
        assert!(rt.call.is_none());
        assert!(acts.notice.is_some_and(|(_, text)| text.contains("coming soon")));
        let number = rt.dial("c_1", "Ada", Instant::now()).unwrap();
        assert!(rt.dial_with_relays(number, Relays::Got(Vec::new()), false, true, None).worker.is_empty(), "closed meanwhile");
        let number = rt.dial("c_1", "Ada", Instant::now()).unwrap();
        let acts = rt.dial_with_relays(number, Relays::Failed("offline".into()), true, true, None);
        assert!(matches!(acts.worker.as_slice(), [CallCommand::Offer { .. }]), "a direct call without relays");
    }

    /// Verify A3: an answer that waited for the relays is for the call that was pressed. Another call ringing by then is
    /// not answered without its own press; a refusal declines the pressed one.
    #[test]
    fn a_delayed_answer_acts_only_on_the_call_pressed() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let _ = incoming(&mut rt, "k_1", now);
        let pressed = rt.call.as_ref().unwrap().number;
        // That call ended while the relays were asked for, and another one rings now.
        let _ = rt.finish(Ending::Missed, false);
        let _ = incoming(&mut rt, "k_2", now);
        let acts = rt.answer_with_relays(pressed, Relays::Got(Vec::new()), true, None, now);
        assert!(acts.worker.is_empty() && rt.call.as_ref().is_some_and(Call::ringing_in), "k_2 still rings, unanswered");
        // The pressed call itself: answered; refused, declined.
        let k2 = rt.call.as_ref().unwrap().number;
        let refused = rt.answer_with_relays(k2, Relays::Denied, true, None, now);
        assert!(refused.worker.is_empty() && rt.call.is_none());
        assert!(matches!(refused.signals.as_slice(), [Signal::Close { decline: true, .. }]));
        let _ = incoming(&mut rt, "k_3", now);
        let k3 = rt.call.as_ref().unwrap().number;
        assert!(rt.answer_with_relays(k3, Relays::Got(Vec::new()), false, None, now).worker.is_empty(), "closed meanwhile: declined");
        let _ = incoming(&mut rt, "k_4", now);
        let k4 = rt.call.as_ref().unwrap().number;
        assert!(matches!(rt.answer_with_relays(k4, Relays::Got(Vec::new()), true, None, now).worker.as_slice(), [CallCommand::Answer { .. }]));
    }

    fn contacts(n: usize) -> Vec<CallContact> {
        (0..n).map(|i| CallContact { id: format!("c_{i}"), name: format!("Friend {i}") }).collect()
    }

    #[test]
    fn the_menu_shows_four_contacts_around_the_chosen_one() {
        assert!(menu_text(&[], 0).starts_with("No friend"));
        let list = contacts(6);
        assert_eq!(menu_text(&list, 1), "  Friend 0\n> Friend 1\n  Friend 2\n  Friend 3");
        assert_eq!(menu_text(&list, 5), "  Friend 4\n> Friend 5");
        assert_eq!(menu_text(&list, 99), "  Friend 4\n> Friend 5", "past the end: the last");
    }

    #[test]
    fn the_radial_spreads_over_the_list() {
        assert_eq!(pick(0.0, 6), 0);
        assert_eq!(pick(0.5, 6), 3);
        assert_eq!(pick(1.0, 6), 5);
        assert_eq!(pick(0.7, 0), 0);
    }

    fn candidate(n: u32) -> IceCandidate {
        IceCandidate { candidate: format!("candidate:{n} 1 udp 1 203.0.113.{n} 9 typ relay raddr 0.0.0.0 rport 0"), ..Default::default() }
    }

    /// Our call to Friend, its worker command sent (worker 1).
    fn calling(rt: &mut CallsRuntime, now: Instant) -> u64 {
        let number = rt.dial("c_1", "Friend", now).unwrap();
        let offer = rt.offer(number, vec![], None).unwrap();
        assert!(matches!(offer, CallCommand::Offer { call, relay_only: true, .. } if call == number));
        rt.current(number).unwrap().worker = Some(1);
        number
    }

    fn incoming(rt: &mut CallsRuntime, id: &str, now: Instant) -> Acts {
        rt.server_event("incoming", &json!({ "call": id, "from": { "id": "c_9", "name": "Rexouium Fan" }, "offer": "v=0", "ice_servers": null }), now)
    }

    #[test]
    fn a_late_event_of_an_ended_call_never_acts_on_the_next() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let a = calling(&mut rt, now);
        // Hung up before Orbiters gave the call its id: nothing to tell it yet.
        let acts = rt.hang_up();
        assert_eq!(acts.worker, vec![CallCommand::Hangup { call: a }]);
        assert!(acts.signals.is_empty());
        let b = calling(&mut rt, now);
        assert_ne!(a, b);
        // A's offer, candidates, connection and failure change nothing for B.
        for e in [
            CallEvent::Offer { call: a, sdp: "v=0".into() },
            CallEvent::Candidate { call: a, candidate: candidate(1) },
            CallEvent::Connected { call: a },
            CallEvent::Ended { call: a, reason: EndReason::Failed },
            CallEvent::Error { call: a, message: "late".into() },
        ] {
            let acts = rt.worker_event(e, now);
            assert!(acts.worker.is_empty() && acts.signals.is_empty() && acts.say.is_none() && acts.notice.is_none());
        }
        let c = rt.call.as_ref().unwrap();
        assert_eq!((c.number, c.phase, c.outbox.len()), (b, CallPhase::Calling, 0));
        // Orbiters' answer for A comes now: A is ended there too, so it stops ringing at the friend's.
        let acts = rt.created(a, Ok("k_a".into()));
        assert_eq!(acts.signals, vec![Signal::Close { id: "k_a".into(), decline: false }]);
        assert_eq!(rt.call.as_ref().unwrap().id, None, "B keeps waiting for its own id");
        // A worker that carried an older call ends: B goes on.
        rt.current(b).unwrap().worker = Some(2);
        assert!(rt.worker_exit(1).say.is_none());
        assert!(rt.call.is_some());
        let acts = rt.worker_exit(2);
        assert_eq!(acts.say.as_deref(), Some("The call with Friend could not connect."));
        assert!(rt.call.is_none());
    }

    #[test]
    fn the_caller_holds_its_candidates_until_the_friend_answers() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let n = calling(&mut rt, now);
        let acts = rt.worker_event(CallEvent::Offer { call: n, sdp: "v=0".into() }, now);
        assert_eq!(acts.signals, vec![Signal::Create { number: n, contact: "c_1".into(), offer: "v=0".into() }]);
        for i in 1..=2 {
            assert!(rt.worker_event(CallEvent::Candidate { call: n, candidate: candidate(i) }, now).signals.is_empty());
        }
        rt.created(n, Ok("k_1".into()));
        // The call has its id but only rings: still nothing about this computer leaves.
        assert!(rt.worker_event(CallEvent::Candidate { call: n, candidate: candidate(3) }, now).signals.is_empty());
        let acts = rt.server_event("answered", &json!({ "call": "k_1", "answer": "v=0 answer" }), now);
        assert_eq!(acts.worker, vec![CallCommand::RemoteAnswer { call: n, sdp: "v=0 answer".into() }]);
        assert_eq!(acts.signals, vec![Signal::Candidates { id: "k_1".into(), candidates: vec![candidate(1), candidate(2), candidate(3)] }]);
        assert_eq!(rt.call.as_ref().unwrap().phase, CallPhase::Connecting);
        // From now on they go as they come, and the friend's go to the worker.
        assert_eq!(
            rt.worker_event(CallEvent::Candidate { call: n, candidate: candidate(4) }, now).signals,
            vec![Signal::Candidates { id: "k_1".into(), candidates: vec![candidate(4)] }]
        );
        let acts = rt.server_event("candidates", &json!({ "call": "k_1", "candidates": ["candidate:5 1 udp 1 198.51.100.5 9 typ relay"] }), now);
        assert!(matches!(&acts.worker[..], [CallCommand::Candidates { call, candidates }] if *call == n && candidates.len() == 1));
        let acts = rt.worker_event(CallEvent::Connected { call: n }, now);
        assert_eq!(acts.say.as_deref(), Some("On a call with Friend."));
        let acts = rt.server_event("ended", &json!({ "call": "k_1" }), now);
        assert_eq!(acts.say.as_deref(), Some("Call with Friend ended."));
        assert_eq!(acts.worker, vec![CallCommand::Hangup { call: n }]);
        assert!(acts.signals.is_empty(), "Orbiters told this side: nothing to tell it back");
    }

    #[test]
    fn the_callee_keeps_the_candidates_that_come_while_it_rings() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let acts = incoming(&mut rt, "k_7", now);
        assert!(acts.publish, "the Answer button shows at once");
        assert_eq!(acts.say.as_deref(), Some("Rexouium Fan is calling. Touch me or press Call to answer."));
        // The caller's candidates (an older desktop sends them at once) wait for the answer.
        let acts = rt.server_event(
            "candidates",
            &json!({ "call": "k_7", "candidates": [{ "candidate": "candidate:1 1 udp 1 203.0.113.1 9 typ srflx", "sdpMid": "0" }] }),
            now,
        );
        assert!(acts.worker.is_empty());
        rt.mute(true);
        let acts = rt.answer(Some("mic-1".into()), now);
        let number = rt.call.as_ref().unwrap().number;
        assert_eq!(acts.worker.len(), 2);
        assert!(matches!(&acts.worker[0], CallCommand::Answer { call, offer, ice_servers, mic, muted: true, relay_only: true }
            if *call == number && offer == "v=0" && ice_servers.is_empty() && mic.as_deref() == Some("mic-1")));
        assert!(matches!(&acts.worker[1], CallCommand::Candidates { candidates, .. } if candidates.len() == 1));
        rt.current(number).unwrap().worker = Some(1);
        // This side's candidates wait for its own answer to go first.
        assert!(rt.worker_event(CallEvent::Candidate { call: number, candidate: candidate(2) }, now).signals.is_empty());
        let acts = rt.worker_event(CallEvent::Answer { call: number, sdp: "v=0 answer".into() }, now);
        assert_eq!(
            acts.signals,
            vec![
                Signal::Answer { number, id: "k_7".into(), sdp: "v=0 answer".into() },
                Signal::Candidates { id: "k_7".into(), candidates: vec![candidate(2)] }
            ]
        );
    }

    #[test]
    fn events_about_another_call_are_ignored_and_a_second_call_is_declined() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        incoming(&mut rt, "k_1", now);
        for event in ["declined", "ended", "answered", "candidates"] {
            let acts = rt.server_event(event, &json!({ "call": "k_2", "candidates": ["candidate:1"], "answer": "x" }), now);
            assert!(acts.worker.is_empty() && acts.signals.is_empty() && acts.say.is_none(), "{event}");
        }
        assert!(rt.call.as_ref().is_some_and(Call::ringing_in));
        // Another friend calls meanwhile: declined for them; the same call again (the stream reopened): nothing.
        assert_eq!(incoming(&mut rt, "k_2", now).signals, vec![Signal::Close { id: "k_2".into(), decline: true }]);
        assert!(incoming(&mut rt, "k_1", now).signals.is_empty());
        // The caller gave up: a missed call, said as one.
        let acts = rt.server_event("ended", &json!({ "call": "k_1", "reason": "hangup" }), now);
        assert_eq!(acts.say.as_deref(), Some("Missed call from Rexouium Fan."));
    }

    #[test]
    fn mute_before_the_call_starts_goes_with_it() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let n = rt.dial("c_1", "Friend", now).unwrap();
        // Pressed while the relay servers are fetched: no worker has the call yet.
        assert!(rt.mute(true).worker.is_empty());
        assert!(matches!(rt.offer(n, vec![], None), Some(CallCommand::Offer { muted: true, .. })));
        rt.current(n).unwrap().worker = Some(1);
        assert_eq!(rt.mute(false).worker, vec![CallCommand::Mute { call: n, on: false }]);
        // Ended meanwhile: no offer goes.
        rt.hang_up();
        assert!(rt.offer(n, vec![], None).is_none());
    }

    #[test]
    fn phases_that_last_too_long_end() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let n = calling(&mut rt, now);
        rt.created(n, Ok("k_1".into()));
        assert!(rt.deadlines(now + CALLING_FOR / 2).say.is_none());
        let acts = rt.deadlines(now + CALLING_FOR + Duration::from_secs(1));
        assert_eq!(acts.say.as_deref(), Some("Friend didn't answer."));
        assert_eq!(acts.signals, vec![Signal::Close { id: "k_1".into(), decline: false }]);
        assert_eq!(acts.worker, vec![CallCommand::Hangup { call: n }]);
        // A ring whose end never came stops, declined for the caller.
        incoming(&mut rt, "k_2", now);
        let acts = rt.deadlines(now + RINGING_FOR + Duration::from_secs(1));
        assert_eq!(acts.say.as_deref(), Some("Missed call from Rexouium Fan."));
        assert_eq!(acts.signals, vec![Signal::Close { id: "k_2".into(), decline: true }]);
        // Answered, but never connected.
        incoming(&mut rt, "k_3", now);
        rt.answer(None, now);
        let acts = rt.deadlines(now + CONNECTING_FOR + Duration::from_secs(1));
        assert_eq!(acts.say.as_deref(), Some("The call with Rexouium Fan could not connect."));
        assert!(rt.call.is_none());
    }

    #[test]
    fn how_calls_end_is_said() {
        let now = Instant::now();
        let mut rt = CallsRuntime::default();
        let n = calling(&mut rt, now);
        assert_eq!(rt.created(n, Err(Ending::Busy)).say.as_deref(), Some("Friend is on another call."));
        let n = calling(&mut rt, now);
        rt.created(n, Ok("k_1".into()));
        assert_eq!(rt.server_event("declined", &json!({ "call": "k_1" }), now).say.as_deref(), Some("Friend can't talk right now."));
        let n = calling(&mut rt, now);
        let acts = rt.worker_event(CallEvent::Ended { call: n, reason: EndReason::Unreachable }, now);
        assert_eq!(acts.say.as_deref(), Some("The call with Friend could not connect: this network seems to block calls."));
        incoming(&mut rt, "k_4", now);
        let acts = rt.hang_up();
        assert_eq!(acts.say.as_deref(), Some("Call from Rexouium Fan declined."));
        assert_eq!(acts.signals, vec![Signal::Close { id: "k_4".into(), decline: true }]);
        // Levels are published only when they move by a step.
        let n = calling(&mut rt, now);
        assert!(rt.worker_event(CallEvent::Levels { call: n, mic: 0.31, remote: 0.0 }, now).publish);
        assert!(!rt.worker_event(CallEvent::Levels { call: n, mic: 0.32, remote: 0.01 }, now).publish);
    }

    #[test]
    fn server_sent_events_are_read_line_by_line() {
        let mut sse = Sse::default();
        let mut events = Vec::new();
        for line in [": ping", "", "event: incoming", "data: {\"call\":", "data: \"k_1\"}", "", "event: ended\r", "data: {}\r", "\r"] {
            events.extend(sse.line(line));
        }
        assert_eq!(events, vec![("incoming".to_string(), "{\"call\":\n\"k_1\"}".to_string()), ("ended".to_string(), "{}".to_string())]);
        // An event too big to be one is dropped whole.
        let big = format!("data: {}", "x".repeat(EVENT_MOST + 1));
        for line in ["event: candidates", big.as_str()] {
            assert!(sse.line(line).is_none());
        }
        assert!(sse.line("").is_none());
        assert_eq!(sse.line("event: declined").or(sse.line("")), Some(("declined".to_string(), String::new())));
    }

    #[test]
    fn a_silent_event_stream_gives_up() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // The server stays silent until the test is done, so that only the idle timeout can end the read.
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: ping\n\n").unwrap();
            // Then nothing, as from a server behind a connection that died.
            let _ = wait.recv_timeout(Duration::from_secs(30));
        });
        let agent = idle_agent(Duration::from_secs(2), Duration::from_millis(300));
        let response = agent.get(&format!("http://127.0.0.1:{port}/n4n0/calls/events")).call().unwrap();
        let mut reader = BufReader::new(response.into_body().into_with_config().limit(u64::MAX).reader());
        let mut line = Vec::new();
        assert_eq!(read_line(&mut reader, &mut line, EVENT_MOST).unwrap(), Some(true));
        assert_eq!(line, b": ping");
        let started = Instant::now();
        let mut ended = false;
        for _ in 0..3 {
            match read_line(&mut reader, &mut line, EVENT_MOST) {
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => {
                    ended = true;
                    break;
                }
            }
        }
        // 300 ms of silence ends it; the bound leaves room for a busy computer.
        let waited = started.elapsed();
        let _ = done.send(());
        server.join().unwrap();
        assert!(ended && waited < Duration::from_secs(10), "the read gave up after {waited:?}");
    }

    #[test]
    fn long_lines_are_skipped_whole() {
        let text = format!("a\n{}\nb", "x".repeat(40));
        let mut reader = std::io::BufReader::with_capacity(8, text.as_bytes());
        let mut line = Vec::new();
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), Some(true));
        assert_eq!(line, b"a");
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), Some(false));
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), Some(true));
        assert_eq!(line, b"b");
        assert_eq!(read_line(&mut reader, &mut line, 16).unwrap(), None);
    }
}
