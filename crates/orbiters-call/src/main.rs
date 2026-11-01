//! N4N0's calls: the call worker of Orbiters - N4N0 (protocol: `orbiters_core::call`, the call itself:
//! `contracts/Docs/n4n0-calls.md`). One WebRTC peer connection at a time with one two-way Opus track (48 kHz mono, about
//! 32 kbit/s, inband FEC); candidates trickle both ways as JSON lines. The microphone and the speaker open once the
//! call connects and close when it ends.
//!
//! Every command names its call (the service's number) and every event says which call it is about: a command for a
//! call that is over does nothing, and the service drops events of calls it ended.
//!
//! TURN goes over UDP only (webrtc-rs 0.21 allocates no other way): TURN over TCP or TLS is left out of the servers
//! the service gives, and a network that blocks UDP makes a call end as `unreachable`.
//!
//! `orbiters-call loopback` connects two calls inside this process over 127.0.0.1, sends a tone each way and checks
//! what arrives (no sound devices needed). `orbiters-call --no-sound` is the worker without sound devices (tests of the
//! protocol): its calls send silence and play nothing.

mod call;
mod loopback;
mod opus;
mod protocol;

use call::{Call, PeerEvent};
use orbiters_core::call::{CallCommand, CallEvent, EndReason, IceCandidate, IceServer};
use orbiters_sound::Duplex;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

/// Where calls bind when the default route's interface cannot be found: every interface (loopback and link-local
/// left out).
const ANY_ADDR: &str = "0.0.0.0:0";
/// A call that has not connected by then is given up (it rings for 30 s at most).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// A connected call that hears nothing for this long is over: the other side always sends (muted, or with its
/// microphone gone, a packet of a byte or two every 20 ms), so this is a call whose path died without a word.
const SILENT_CALL: Duration = Duration::from_secs(15);
/// How often levels are sent while connected.
const LEVELS_EVERY: Duration = Duration::from_millis(200);
/// The longest command line: a description is a few kB.
const MAX_LINE: usize = 1 << 20;
/// Candidates kept for a call that has not started yet.
const EARLY_MOST: usize = 64;

fn emit(event: &CallEvent) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", event.to_line());
    let _ = out.flush();
}

fn error(call: u64, message: impl Into<String>) {
    emit(&CallEvent::Error { call, message: message.into() });
}

/// The local address of the interface the default route goes through, as a bind address: the only one whose
/// address the other side may learn. Nothing is sent (a UDP "connect" only picks the route).
fn default_route_addr() -> String {
    let probe = || -> std::io::Result<std::net::IpAddr> {
        let socket = std::net::UdpSocket::bind(ANY_ADDR)?;
        // A documentation address (RFC 5737): routed like any public one, never reached.
        socket.connect("192.0.2.1:9")?;
        Ok(socket.local_addr()?.ip())
    };
    match probe() {
        Ok(ip) if !ip.is_unspecified() && !ip.is_loopback() => format!("{ip}:0"),
        _ => ANY_ADDR.to_string(),
    }
}

/// How a call starts: as the caller, or answering an offer.
enum Role {
    Caller,
    Callee(String),
}

struct Worker {
    events: UnboundedSender<(u64, PeerEvent)>,
    call: Option<Call>,
    /// The highest call number seen: a command for an earlier call is late.
    latest: u64,
    connected: Option<Instant>,
    mic_device: Option<String>,
    audio: Option<Duplex>,
    /// The sound devices' last trouble said, so it is said once.
    audio_error: Option<String>,
    /// Candidates for a call whose offer or answer has not come yet.
    early: Option<(u64, Vec<IceCandidate>)>,
    /// Calls open no sound device (`--no-sound`).
    no_sound: bool,
}

impl Worker {
    /// Runs `command`; false when the worker should quit.
    async fn command(&mut self, command: CallCommand) -> bool {
        let current = self.call.as_ref().map(|c| c.id);
        match command {
            CallCommand::Offer { call, ice_servers, mic, muted, relay_only } => self.start(call, Role::Caller, &ice_servers, mic, muted, relay_only).await,
            CallCommand::Answer { call, offer, ice_servers, mic, muted, relay_only } => {
                self.start(call, Role::Callee(offer), &ice_servers, mic, muted, relay_only).await
            }
            CallCommand::RemoteAnswer { call, sdp } if current == Some(call) => {
                let c = self.call.as_mut().expect("current");
                if let Err(e) = c.remote_answer(sdp).await {
                    self.fail(format!("The call's answer could not be used: {e:#}")).await;
                }
            }
            CallCommand::Candidates { call, candidates } if current == Some(call) => {
                self.call.as_mut().expect("current").add_candidates(candidates).await;
            }
            CallCommand::Candidates { call, candidates } if call > self.latest => {
                // For a call whose offer or answer is on its way: kept for it.
                match &mut self.early {
                    Some((id, kept)) if *id == call => kept.extend(candidates),
                    _ => self.early = Some((call, candidates)),
                }
                if let Some((_, kept)) = &mut self.early {
                    kept.truncate(EARLY_MOST);
                }
            }
            CallCommand::Mute { call, on } if current == Some(call) => {
                self.call.as_ref().expect("current").muted.store(on, std::sync::atomic::Ordering::Relaxed);
            }
            CallCommand::Hangup { call } if current == Some(call) => self.end(EndReason::Hangup).await,
            CallCommand::Quit => {
                self.stop().await;
                return false;
            }
            // A command for a call that is over (or never started here): nothing to do.
            _ => {}
        }
        true
    }

    /// A new call (replacing any other, without a word): its offer, or its answer to `offer`.
    async fn start(&mut self, id: u64, role: Role, ice_servers: &[IceServer], mic: Option<String>, muted: bool, relay_only: bool) {
        if id <= self.latest {
            // Late: a newer call was started meanwhile.
            return;
        }
        self.latest = id;
        self.stop().await;
        self.mic_device = mic;
        let servers = protocol::servers(ice_servers);
        if servers.skipped > 0 {
            eprintln!("call: {} relay URL(s) left out (TURN over TCP or TLS, or unreadable)", servers.skipped);
        }
        if relay_only && !servers.relay {
            error(id, "No relay server this computer can use was given: the call cannot start without showing your address.");
            emit(&CallEvent::Ended { call: id, reason: EndReason::Failed });
            return;
        }
        let mut call = match Call::new(id, servers.servers, relay_only, &default_route_addr(), self.events.clone()).await {
            Ok(call) => call,
            Err(e) => {
                error(id, format!("The call could not start: {e:#}"));
                emit(&CallEvent::Ended { call: id, reason: EndReason::Failed });
                return;
            }
        };
        call.muted.store(muted, std::sync::atomic::Ordering::Relaxed);
        let description = match role {
            Role::Caller => call.offer().await.map(|sdp| CallEvent::Offer { call: id, sdp }),
            Role::Callee(offer) => call.answer(offer).await.map(|sdp| CallEvent::Answer { call: id, sdp }),
        };
        match self.early.take() {
            Some((early_id, early)) if early_id == id => call.add_candidates(early).await,
            // For a call still to come: kept.
            Some((early_id, early)) if early_id > id => self.early = Some((early_id, early)),
            _ => {}
        }
        self.call = Some(call);
        match description {
            Ok(event) => emit(&event),
            Err(e) => self.fail(format!("Could not set up the call: {e:#}")).await,
        }
    }

    async fn fail(&mut self, message: String) {
        let id = self.call.as_ref().map(|c| c.id).unwrap_or_default();
        error(id, message);
        self.end(EndReason::Failed).await;
    }

    /// Ends the call (if any) and says so.
    async fn end(&mut self, reason: EndReason) {
        if let Some(id) = self.call.as_ref().map(|c| c.id) {
            self.stop().await;
            emit(&CallEvent::Ended { call: id, reason });
        }
    }

    async fn stop(&mut self) {
        self.audio = None;
        self.audio_error = None;
        self.connected = None;
        if let Some(call) = self.call.take() {
            call.close().await;
        }
    }

    async fn call_event(&mut self, id: u64, event: PeerEvent) {
        if self.call.as_ref().is_none_or(|c| c.id != id) {
            // From a call already ended.
            return;
        }
        match event {
            PeerEvent::Candidate(candidate) => emit(&CallEvent::Candidate { call: id, candidate }),
            PeerEvent::Gathered => {
                let call = self.call.as_ref().expect("checked above");
                if call.relay_only && call.found.relay.load(std::sync::atomic::Ordering::Relaxed) == 0 && self.connected.is_none() {
                    // Waiting longer changes nothing: no relay answered.
                    error(id, "The call relay could not be reached: this network may block calls (UDP).");
                    self.end(EndReason::Unreachable).await;
                }
            }
            PeerEvent::Connected => {
                self.connected = Some(Instant::now());
                emit(&CallEvent::Connected { call: id });
                let call = self.call.as_mut().expect("checked above");
                if let Err(e) = call.start_sending().await {
                    error(id, format!("The call's audio could not be sent: {e:#}"));
                }
                if !self.no_sound {
                    self.audio = open_audio(call, self.mic_device.clone());
                }
            }
            PeerEvent::Ended(EndReason::Failed) => {
                let reason = self.call.as_ref().map(|c| if self.connected.is_some() { EndReason::Failed } else { c.failure() }).unwrap_or(EndReason::Failed);
                self.end(reason).await;
            }
            PeerEvent::Ended(reason) => self.end(reason).await,
        }
    }

    async fn tick(&mut self) {
        let Some(call) = &self.call else { return };
        let id = call.id;
        if let Some(connected) = self.connected {
            let (mic, remote) = call.levels.take();
            emit(&CallEvent::Levels { call: id, mic, remote });
            if let Some(trouble) = self.audio.as_ref().and_then(Duplex::error) {
                if self.audio_error.as_ref() != Some(&trouble) {
                    error(id, format!("Call sound: {trouble}"));
                    self.audio_error = Some(trouble);
                }
            }
            if call.quiet_for(connected) > SILENT_CALL {
                error(id, "Nothing came from the other side for 15 s: the call dropped.");
                self.end(EndReason::Failed).await;
            }
        } else if call.started.elapsed() > CONNECT_TIMEOUT {
            let reason = call.failure();
            error(id, "The call did not connect.");
            self.end(reason).await;
        }
    }
}

/// The microphone (`device`, else the default one) into the call and the call to the default output. The devices
/// open on a thread of their own, which takes the default microphone when the chosen one fails, follows a default
/// output that changes and reopens a device that goes away; with no sound at all the call goes on, unheard.
fn open_audio(call: &Call, device: Option<String>) -> Option<Duplex> {
    let mic = call.mic.clone();
    let speaker = call.speaker.clone();
    // Waits up to 3 s for the devices.
    tokio::task::block_in_place(|| Duplex::start(device, move |samples: &[f32]| mic.push(samples), speaker))
        .map_err(|e| error(call.id, format!("The call's sound could not start: {e:#}")))
        .ok()
}

/// WebRTC's warnings and errors (a relay URL it skips, a socket it could not bind) on stderr, which the service logs.
struct Log;

impl log::Log for Log {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("call: {} {}: {}", record.level(), record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

/// One line of `input` into `line`, at most [`MAX_LINE`] bytes: a longer one is skipped whole (`Err` with its
/// length). `None` at the end.
fn read_line(input: &mut impl BufRead, line: &mut Vec<u8>) -> Option<Result<(), usize>> {
    line.clear();
    let mut skipped = 0usize;
    let mut any = false;
    loop {
        let buf = match input.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if buf.is_empty() {
            return any.then_some(if skipped > 0 { Err(skipped) } else { Ok(()) });
        }
        any = true;
        let newline = buf.iter().position(|&b| b == b'\n');
        let chunk = &buf[..newline.unwrap_or(buf.len())];
        if skipped == 0 && line.len() + chunk.len() <= MAX_LINE {
            line.extend_from_slice(chunk);
        } else {
            skipped += line.len() + chunk.len();
            line.clear();
        }
        let used = newline.map_or(buf.len(), |i| i + 1);
        input.consume(used);
        if newline.is_some() {
            return Some(if skipped > 0 { Err(skipped) } else { Ok(()) });
        }
    }
}

enum Input {
    Command(CallCommand),
    StdinClosed,
}

async fn serve(no_sound: bool) {
    let (inputs, mut input_rx) = unbounded_channel();
    // A thread of its own: tokio's stdin would hold the runtime open at exit.
    let _ = std::thread::Builder::new().name("stdin".into()).spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut line = Vec::new();
        while let Some(read) = read_line(&mut stdin, &mut line) {
            if let Err(bytes) = read {
                eprintln!("call: a command of {bytes} bytes was too long, skipped");
                continue;
            }
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<CallCommand>(&text) {
                Ok(c) => {
                    if inputs.send(Input::Command(c)).is_err() {
                        return;
                    }
                }
                // Not the line: it may hold a description (addresses) or relay credentials.
                Err(e) => eprintln!("call: unreadable command ({} bytes): {e}", line.len()),
            }
        }
        let _ = inputs.send(Input::StdinClosed);
    });
    let (events, mut event_rx) = unbounded_channel();
    let mut worker = Worker { events, call: None, latest: 0, connected: None, mic_device: None, audio: None, audio_error: None, early: None, no_sound };
    let mut ticker = tokio::time::interval(LEVELS_EVERY);
    emit(&CallEvent::Ready);
    loop {
        tokio::select! {
            input = input_rx.recv() => match input {
                Some(Input::Command(c)) => {
                    if !worker.command(c).await {
                        break;
                    }
                }
                Some(Input::StdinClosed) | None => {
                    worker.stop().await;
                    break;
                }
            },
            Some((id, event)) = event_rx.recv() => worker.call_event(id, event).await,
            _ = ticker.tick() => worker.tick().await,
        }
    }
}

fn main() {
    orbiters_core::panic_log::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    static LOG: Log = Log;
    if log::set_logger(&LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("call: {e}");
            std::process::exit(1);
        }
    };
    if args.first().map(String::as_str) == Some("loopback") {
        match runtime.block_on(loopback::run(Duration::from_secs(3))) {
            Ok(report) => {
                println!("{report}");
                std::process::exit(if report.passed() { 0 } else { 1 });
            }
            Err(e) => {
                eprintln!("loopback: {e:#}");
                std::process::exit(1);
            }
        }
    }
    runtime.block_on(serve(args.iter().any(|a| a == "--no-sound")));
    // The stdin thread may still be waiting for a line.
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(input: &str) -> Vec<Result<String, usize>> {
        let mut reader = std::io::BufReader::with_capacity(16, input.as_bytes());
        let mut line = Vec::new();
        let mut out = Vec::new();
        while let Some(read) = read_line(&mut reader, &mut line) {
            out.push(read.map(|()| String::from_utf8(line.clone()).unwrap()));
        }
        out
    }

    #[test]
    fn lines_are_read_whole_and_bounded() {
        assert_eq!(lines("a\nbb\n\nccc"), vec![Ok("a".into()), Ok("bb".into()), Ok("".into()), Ok("ccc".into())]);
        let long = format!("{}\nnext\n", "x".repeat(MAX_LINE + 10));
        let read = lines(&long);
        assert!(read[0].is_err(), "a line past the limit is skipped");
        assert_eq!(read[1], Ok("next".into()));
    }

    fn worker() -> (Worker, tokio::sync::mpsc::UnboundedReceiver<(u64, PeerEvent)>) {
        let (events, rx) = unbounded_channel();
        (Worker { events, call: None, latest: 0, connected: None, mic_device: None, audio: None, audio_error: None, early: None, no_sound: true }, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn late_commands_do_nothing() {
        let (mut w, _rx) = worker();
        assert!(w.command(CallCommand::Offer { call: 2, ice_servers: vec![], mic: None, muted: true, relay_only: false }).await);
        assert_eq!(w.call.as_ref().map(|c| c.id), Some(2));
        assert!(w.call.as_ref().unwrap().muted.load(std::sync::atomic::Ordering::Relaxed), "muted from the start");
        // An earlier call's offer, answer, mute and hang-up change nothing.
        w.command(CallCommand::Offer { call: 1, ice_servers: vec![], mic: None, muted: false, relay_only: false }).await;
        w.command(CallCommand::Mute { call: 1, on: false }).await;
        w.command(CallCommand::Hangup { call: 1 }).await;
        assert_eq!(w.call.as_ref().map(|c| c.id), Some(2));
        assert!(w.call.as_ref().unwrap().muted.load(std::sync::atomic::Ordering::Relaxed));
        // Candidates for the next call wait for it.
        w.command(CallCommand::Candidates {
            call: 3,
            candidates: vec![IceCandidate { candidate: "candidate:1 1 udp 1 127.0.0.1 9 typ host".into(), ..Default::default() }],
        })
        .await;
        assert_eq!(w.early.as_ref().map(|(id, c)| (*id, c.len())), Some((3, 1)));
        w.command(CallCommand::Hangup { call: 2 }).await;
        assert!(w.call.is_none());
        assert!(!w.command(CallCommand::Quit).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_only_call_needs_a_relay() {
        let (mut w, _rx) = worker();
        let stun = vec![IceServer { urls: vec!["stun:stun.example:3478".into()], ..Default::default() }];
        w.command(CallCommand::Offer { call: 1, ice_servers: stun, mic: None, muted: false, relay_only: true }).await;
        assert!(w.call.is_none(), "no call without a relay: nothing would hide this computer's address");
        assert_eq!(w.latest, 1);
    }
}
