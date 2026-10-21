//! Orb's calls: the call worker of Orbiters desktop (protocol: [`protocol`], the call itself:
//! `contracts/Docs/orb-calls.md`). One WebRTC peer connection at a time with one two-way Opus track (48 kHz mono, about
//! 32 kbit/s, inband FEC); candidates trickle both ways as JSON lines. The microphone and the speaker open once the
//! call connects and close when it ends.
//!
//! `orbiters-call loopback` connects two calls inside this process over 127.0.0.1, sends a tone each way and checks
//! what arrives (no sound devices needed). `orbiters-call --no-sound` is the worker without sound devices (tests of the
//! protocol): its calls send nothing and play nothing.

mod call;
mod loopback;
mod opus;
mod protocol;

use call::{Call, CallEvent};
use orbiters_sound::Duplex;
use protocol::{Command, EndReason, Event};
use std::io::{BufRead, Write};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use webrtc::peer_connection::RTCIceServer;

/// Where calls bind: every interface (loopback and link-local left out).
const UDP_ADDR: &str = "0.0.0.0:0";
/// A call that has not connected by then is given up (it rings for 30 s at most).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How often levels are sent while connected.
const LEVELS_EVERY: Duration = Duration::from_millis(200);

fn emit(event: &Event) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", event.to_line());
    let _ = out.flush();
}

fn error(message: impl Into<String>) {
    emit(&Event::Error { message: message.into() });
}

struct Worker {
    events: UnboundedSender<(u64, CallEvent)>,
    next_id: u64,
    call: Option<Call>,
    connected: bool,
    mic_device: Option<String>,
    audio: Option<Duplex>,
    /// Calls open no sound device (`--no-sound`).
    no_sound: bool,
}

impl Worker {
    /// Runs `command`; false when the worker should quit.
    async fn command(&mut self, command: Command) -> bool {
        match command {
            Command::Offer { ice_servers, mic } => {
                if let Some(call) = self.start(ice_servers.into_iter().map(Into::into).collect(), mic).await {
                    match call.offer().await {
                        Ok(sdp) => emit(&Event::Offer { sdp }),
                        Err(e) => self.fail(format!("Could not make the call's offer: {e:#}")).await,
                    }
                }
            }
            Command::Answer { offer, ice_servers, mic } => {
                if let Some(call) = self.start(ice_servers.into_iter().map(Into::into).collect(), mic).await {
                    match call.answer(offer).await {
                        Ok(sdp) => emit(&Event::Answer { sdp }),
                        Err(e) => self.fail(format!("Could not answer the call: {e:#}")).await,
                    }
                }
            }
            Command::RemoteAnswer { sdp } => match &mut self.call {
                Some(call) => {
                    if let Err(e) = call.remote_answer(sdp).await {
                        self.fail(format!("The call's answer could not be used: {e:#}")).await;
                    }
                }
                None => error("There is no call to answer."),
            },
            Command::Candidates { candidates } => {
                if let Some(call) = &mut self.call {
                    call.add_candidates(candidates).await;
                }
            }
            Command::Mute { on } => {
                if let Some(call) = &self.call {
                    call.muted.store(on, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Command::Hangup => self.end(EndReason::Hangup).await,
            Command::Quit => {
                self.stop().await;
                return false;
            }
        }
        true
    }

    /// A new call (replacing any other, without a word).
    async fn start(&mut self, ice_servers: Vec<RTCIceServer>, mic: Option<String>) -> Option<&mut Call> {
        self.stop().await;
        self.next_id += 1;
        self.mic_device = mic;
        match Call::new(self.next_id, ice_servers, UDP_ADDR, self.events.clone()).await {
            Ok(call) => self.call = Some(call),
            Err(e) => {
                error(format!("The call could not start: {e:#}"));
                emit(&Event::Ended { reason: EndReason::Failed });
            }
        }
        self.call.as_mut()
    }

    async fn fail(&mut self, message: String) {
        error(message);
        self.end(EndReason::Failed).await;
    }

    /// Ends the call (if any) and says so.
    async fn end(&mut self, reason: EndReason) {
        if self.call.is_some() {
            self.stop().await;
            emit(&Event::Ended { reason });
        }
    }

    async fn stop(&mut self) {
        self.audio = None;
        self.connected = false;
        if let Some(call) = self.call.take() {
            call.close().await;
        }
    }

    async fn call_event(&mut self, id: u64, event: CallEvent) {
        if self.call.as_ref().is_none_or(|c| c.id != id) {
            // From a call already ended.
            return;
        }
        match event {
            CallEvent::Candidate(candidate) => emit(&Event::Candidate { candidate }),
            CallEvent::Connected => {
                self.connected = true;
                emit(&Event::Connected);
                let call = self.call.as_mut().expect("checked above");
                if let Err(e) = call.start_sending().await {
                    error(format!("The call's audio could not be sent: {e:#}"));
                }
                if !self.no_sound {
                    self.audio = open_audio(call, self.mic_device.clone());
                }
            }
            CallEvent::Ended(reason) => self.end(reason).await,
        }
    }

    async fn tick(&mut self) {
        let Some(call) = &self.call else { return };
        if self.connected {
            let (mic, remote) = call.levels.take();
            emit(&Event::Levels { mic, remote });
        } else if call.started.elapsed() > CONNECT_TIMEOUT {
            error("The call did not connect.");
            self.end(EndReason::Failed).await;
        }
    }
}

/// The microphone (`device`, else the default one) into the call and the call to the default output. A microphone that
/// fails is said and the default one tried; with no sound at all the call goes on, unheard.
fn open_audio(call: &Call, device: Option<String>) -> Option<Duplex> {
    let start = |device: Option<String>| {
        let mic = call.mic.clone();
        let speaker = call.speaker.clone();
        // Waits up to 3 s for the devices.
        tokio::task::block_in_place(|| {
            Duplex::start(
                device,
                move |samples: &[f32]| {
                    let _ = mic.send(samples.to_vec());
                },
                speaker,
            )
        })
    };
    let tried = device.is_some();
    match start(device) {
        Ok(audio) => return Some(audio),
        Err(e) if tried => error(format!("The microphone could not be opened ({e:#}); using the default one.")),
        Err(e) => {
            error(format!("The call's sound could not start: {e:#}"));
            return None;
        }
    }
    start(None).map_err(|e| error(format!("The call's sound could not start: {e:#}"))).ok()
}

enum Input {
    Command(Command),
    StdinClosed,
}

async fn serve(no_sound: bool) {
    let (inputs, mut input_rx) = unbounded_channel();
    // A thread of its own: tokio's stdin would hold the runtime open at exit.
    let _ = std::thread::Builder::new().name("stdin".into()).spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Command>(&line) {
                Ok(c) => {
                    if inputs.send(Input::Command(c)).is_err() {
                        return;
                    }
                }
                Err(e) => eprintln!("call: unreadable command ({e}): {line}"),
            }
        }
        let _ = inputs.send(Input::StdinClosed);
    });
    let (events, mut event_rx) = unbounded_channel();
    let mut worker = Worker { events, next_id: 0, call: None, connected: false, mic_device: None, audio: None, no_sound };
    let mut ticker = tokio::time::interval(LEVELS_EVERY);
    emit(&Event::Ready);
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
    let args: Vec<String> = std::env::args().skip(1).collect();
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
