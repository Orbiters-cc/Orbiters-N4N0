//! `orbiters-desktop --publish-cost [--rounds <n>]` measures what one change of the published state costs the tray
//! service and each process that shows it (architecture review 2026-11-10, DRT-1), beside `--idle-frames`. No window,
//! no GPU, no network: a controller that is never started holds a made-up state the size of a busy evening (a world
//! with 40 people, N4N0's conversation and lists filled, uploads, shares, the FPS booster), and each case changes it
//! the way the service does, `n` times. Nothing of the person's is read or written.
//!
//! For each change, three things are timed, as the code does them:
//! - **publish**: the change itself and the service handing the new state out (`Controller::update`, or N4N0's host
//!   taking a state, which is what an open N4N0 screen does);
//! - **write**: one link putting it on the wire, as `service.rs` does for the window and for the overlay, and how many
//!   bytes that is;
//! - **read**: one client taking that line in, as `ipc::Client`'s link thread does.
//!
//! A link writes every state it is handed while it keeps up (the overlay's), so write and read are per change here; the
//! window's link is paced and skips states, never more than these. Times are the median of the rounds. After each case
//! the client's state is compared with the service's, whole.
//!
//! **What this is not.** These are times in memory, in one thread of one process: the line is written to a buffer and
//! read from it. No socket, no link thread waking, no client's lock or frame, and no subscriber of the controller (a
//! real service posts each state to every link's mailbox and its two watchers). "copy" is this harness copying its
//! made-up N4N0 state, as a stand-in for `N4n0Runtime::status`, which also works out the face, the countdown, the
//! states' view and more, and is not measured here. A recording's statistics with the camera moving go through the
//! controller's real path (`on_worker_event`): the path grows and is thinned as in a recording.

use super::n4n0::Link;
use super::{Controller, Shared};
use crate::ipc;
use crate::state::{N4n0Line, N4n0Status, N4n0Who, ProcessUsage, Snapshot, StateView, UploadKind, UploadState, UploadStatus};
use orbiters_core::osc::OscValue;
use orbiters_core::settings::Settings;
use orbiters_n4n0::Host;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The made-up state: what the service holds on a busy evening in VR.
pub(crate) fn busy_evening() -> Snapshot {
    let mut s = Snapshot::new(Settings::default());
    s.account.state = crate::state::AccountState::SignedIn { username: "Orbit".into() };
    s.vrchat.running = true;
    s.vrchat.pid = Some(4242);
    s.vrchat.log_found = true;
    s.vrchat.motion = Some(Default::default());
    let world = &mut s.vrchat.world;
    world.world_id = Some("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b".into());
    world.world_name = Some("The Black Cat".into());
    world.instance = Some("48213~hidden(usr_3f0d9d62-6a55-4f07-9a0e-5b1d2b7f6c11)~region(eu)".into());
    world.in_world = true;
    world.log_file = Some(PathBuf::from(r"C:\Users\Someone\AppData\LocalLow\VRChat\VRChat\output_log_2026-11-10_20-12-44.txt"));
    world.local_player = Some("Orbit".into());
    world.avatar_name = Some("Rexouium".into());
    for i in 0..40 {
        let name = format!("Visitor number {i:02}");
        world.players.insert(format!("usr_{i:08x}-6a55-4f07-9a0e-5b1d2b7f6c11"), name.clone());
        world.avatars.insert(name, format!("Some avatar {i:02} ✨"));
    }
    *s.sources = vec![orbiters_core::protocol::SpoutSourceInfo { name: "VRCSender1".into(), width: 1920, height: 1080, format: 87, description: String::new() }];
    s.mics = (0..4).map(|i| orbiters_core::protocol::AudioDeviceInfo { id: format!("{{0.0.1.00000000}}.{{device-{i}}}"), name: format!("Microphone {i} (USB audio)"), default: i == 0 }).collect();
    s.osc.enabled = true;
    s.osc.listening = true;
    s.osc.advertising = true;
    s.osc.port = Some(9001);
    s.osc.vrchat_found = true;
    s.osc.values = 214;
    s.osc.receiving = true;
    s.osc.last_event = Some("/avatar/parameters/VelocityMagnitude".into());
    s.steamvr.running = true;
    s.steamvr.pid = Some(777);
    s.steamvr.overlay = true;
    s.steamvr.headset_in_use = Some(true);
    s.storage.used_bytes = 412_000_000_000;
    s.storage.free_bytes = Some(36_000_000_000);
    s.storage.sessions = 180;
    s.worker.ready = true;
    s.reconstruct_queue = (0..3).map(|i| PathBuf::from(format!(r"D:\Orbiters\sessions\2026-11-10_21-0{i}-12_The-Black-Cat"))).collect();
    s.uploads = (0..5)
        .map(|i| UploadStatus {
            id: i,
            world_id: format!("wrld_{i:08x}-430c-4f8f-b53e-1f294eed230b"),
            world_name: format!("A world worth showing {i}"),
            kind: if i % 2 == 0 { UploadKind::Pictures } else { UploadKind::Preview },
            paths: (0..6).map(|p| PathBuf::from(format!(r"C:\Users\Someone\Pictures\VRChat\2026-11\VRChat_2026-11-10_21-0{p}-11.png"))).collect(),
            sent: 48_000_000,
            total: 96_000_000,
            state: if i == 0 { UploadState::Sending } else { UploadState::Done { added: 6, skipped: 0 } },
        })
        .collect();
    s.booster.watching = true;
    s.booster.lowered = (0..6).map(|i| (format!("background-program-{i}.exe"), 2)).collect();
    s.booster.top = (0..8).map(|i| (format!("program-{i}.exe"), 3.5 * i as f32)).collect();
    s.booster.cpu_total = 41.0;
    s.booster.listed_cores = 2.4;
    s.n4n0 = busy_n4n0();
    s
}

/// N4N0's part of it: on an avatar with its screen open, a whole conversation, the lists its menus show.
fn busy_n4n0() -> N4n0Status {
    use orbiters_n4n0_core::captures::{CaptureRow, CapturesView, Friend, Friends};
    use orbiters_n4n0_core::feedback::{CommentRow, FeedbackView, LabelCount, Subject, WorldFeedback};
    let mut o = N4n0Status {
        avatar_id: Some("avtr_7f3c2b1e-0d4a-4a7b-9c55-1f0e9a2b6c11".into()),
        prop_sync: Some(8),
        manifest: true,
        controls: 42,
        panel: Some("Done, hoodie on. Now go find\nyour friends instead of\ntalking to a floating ball.".into()),
        screen: true,
        screen_open: true,
        can_drop: true,
        versions: "N4N0 1.0.12 · App 0.3.5".into(),
        tracking_note: "Smoothing your headset, controllers and trackers".into(),
        input_params: 14,
        ..Default::default()
    };
    // The conversation as long as it gets (`MAX_LINES` in orbiters-n4n0's runtime).
    o.lines = (0..60)
        .map(|i| N4n0Line {
            who: [N4n0Who::You, N4n0Who::Note, N4n0Who::N4n0][i % 3],
            text: format!("Line {i} of the evening's conversation: drafted “Movie night” for Saturday 21:00 in Cozy Cinema, approve it on Orbiters and I'll announce it."),
            at_ms: 1_762_800_000_000 + i as u64 * 20_000,
            streaming: false,
        })
        .collect();
    o.videos = (0..40)
        .map(|i| orbiters_n4n0_core::videos::SavedVideo {
            url: format!("https://www.youtube.com/watch?v=36YnV9STB{i:02}"),
            title: Some(format!("Lo-fi beats to sit in a VRChat club to, part {i}")),
            world: Some("The Black Cat".into()),
            saved_utc: "2026-10-20T22:14:03Z".into(),
        })
        .collect();
    *o.resources = usage(0);
    o.voice = crate::state::VoiceStatus { installed: true, running: true, ..Default::default() };
    let state = |i: usize| StateView {
        id: format!("state-{i}"),
        name: format!("Cosy evening {i}"),
        saved_utc: "2026-10-23T21:05:00Z".into(),
        default: i == 1,
        pose: i.is_multiple_of(2),
        size: false,
        parameters: 46,
        pose_parameters: 12,
        eye_height: Some(1.62),
    };
    o.states.avatar_id = o.avatar_id.clone();
    o.states.avatar_name = Some("Rexouium".into());
    o.states.states = (0..6).map(state).collect();
    o.states.avatars = (0..8)
        .map(|a| crate::state::AvatarView { id: format!("avtr_{a:08x}-5f1a-4d90-8e3b-77a1c0d4e9f2"), name: Some(format!("Avatar {a} ✨")), worn: a == 0, states: (0..4).map(state).collect() })
        .collect();
    *o.captures = CapturesView {
        latest: None,
        history: (0..60).map(|i| CaptureRow { key: format!("2026-11-10_21-{i:02}-12_The-Black-Cat"), world: "The Black Cat".into(), taken: 1_762_800_000 + i, ..Default::default() }).collect(),
        listed: true,
        note: String::new(),
    };
    *o.friends = Friends::Ready { friends: (0..120).map(|i| Friend { id: format!("usr_{i:08x}-6a55-4f07-9a0e-5b1d2b7f6c11"), name: format!("A friend of mine {i}") }).collect() };
    o.feedback = FeedbackView {
        signed_in: true,
        subjects: (0..12).map(|i| Subject { world: format!("wrld_{i:08x}-430c-4f8f-b53e-1f294eed230b"), name: format!("A world visited {i}"), ..Default::default() }).collect(),
        worlds: (0..4)
            .map(|i| WorldFeedback {
                world: format!("wrld_{i:08x}-430c-4f8f-b53e-1f294eed230b"),
                likes: 120,
                labels: (0..8).map(|l| LabelCount { id: format!("label-{l}"), count: 12 + l, mine: l == 2 }).collect(),
                comments: (0..10).map(|c| CommentRow { id: c, by: format!("Someone {c}"), text: "A lovely place to sit with friends, and the lighting is gorgeous at night.".into(), at: 1_762_700_000 + c, mine: false }).collect(),
                comment_count: 31,
                ..Default::default()
            })
            .collect(),
    };
    o.calls.contacts = (0..12).map(|i| crate::state::CallContact { id: format!("usr_{i:08x}-6a55-4f07-9a0e-5b1d2b7f6c11"), name: format!("A friend who can be called {i}") }).collect();
    o
}

/// The footprint of Orbiters' processes and VRChat's, as the service reads it every three seconds.
fn usage(round: usize) -> Vec<ProcessUsage> {
    ["Orbiters - N4N0", "Capture worker", "N4N0's ears", "VRChat"]
        .iter()
        .enumerate()
        .map(|(i, name)| ProcessUsage { name: name.to_string(), cpu_percent: 1.5 * i as f32 + (round % 7) as f32 * 0.1, memory_mb: 148.0 + 300.0 * i as f32 })
        .collect()
}

#[derive(Default)]
struct Timed {
    /// N4N0 making the state it hands over (`N4n0Runtime::status`), where the change is N4N0's.
    state: Vec<Duration>,
    publish: Vec<Duration>,
    write: Vec<Duration>,
    read: Vec<Duration>,
    bytes: Vec<usize>,
    /// Changes that published nothing (the state was as before).
    silent: usize,
}

fn median<T: Copy + Ord + Default>(values: &mut [T]) -> T {
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or_default()
}

/// One link's writing and one client's reading of the states the controller hands out.
struct Wire {
    line: Vec<u8>,
    /// The state the link last wrote, and the one the client holds.
    sent: Option<Arc<Snapshot>>,
    client: Option<Arc<Snapshot>>,
}

impl Wire {
    /// Writes `snapshot` as a link does and reads it as a client does; the two times and the line's length.
    fn carry(&mut self, snapshot: &Arc<Snapshot>) -> anyhow::Result<(Duration, Duration, usize)> {
        self.line.clear();
        let started = Instant::now();
        ipc::write_state(&mut self.line, self.sent.as_ref(), snapshot)?;
        self.sent = Some(snapshot.clone());
        let wrote = started.elapsed();
        let text = std::str::from_utf8(&self.line)?.trim_end();
        let started = Instant::now();
        // As `ipc::Client`'s link thread: the changed parts into a copy of the state held, or the whole state.
        self.client = Some(match (ipc::changed_parts(text), &self.client) {
            (Some(parts), Some(held)) => {
                let mut next = (**held).clone();
                orbiters_core::slice::apply_changes(&mut next, parts)?;
                Arc::new(next)
            }
            (Some(_), None) => anyhow::bail!("changes before the whole state"), // i18n: not shown (a developer's command line, in English)
            (None, _) => match serde_json::from_str::<ipc::ToClient>(text)? {
                ipc::ToClient::Snapshot { snapshot } => Arc::new(*snapshot),
                other => anyhow::bail!("{other:?}"), // i18n: not shown (a developer's command line, in English)
            },
        });
        let read = started.elapsed();
        Ok((wrote, read, self.line.len()))
    }

    /// Whether the client holds what the service published (checked once a case, outside the timing).
    fn in_step(&self, published: &Snapshot) -> anyhow::Result<bool> {
        let held = self.client.as_ref().map(|s| serde_json::to_value(&**s)).transpose()?;
        Ok(held == Some(serde_json::to_value(published)?))
    }
}

/// Runs `change` `rounds` times on `ctl` (`change` returns how long N4N0 took to make its state, when it did), each
/// followed by one link's write and one client's read of what was published.
fn case(ctl: &Shared, wire: &mut Wire, rounds: usize, mut change: impl FnMut(usize) -> Option<Duration>) -> anyhow::Result<Timed> {
    let mut timed = Timed::default();
    // The first rounds warm the allocator and the caches up.
    for round in 0..rounds + rounds / 10 {
        let before = ctl.view();
        let started = Instant::now();
        let state = change(round);
        let published = started.elapsed().saturating_sub(state.unwrap_or_default());
        let after = ctl.view();
        if Arc::ptr_eq(&before, &after) {
            timed.silent += 1;
            continue;
        }
        let (wrote, read, bytes) = wire.carry(&after)?;
        if round >= rounds / 10 {
            timed.state.extend(state);
            timed.publish.push(published);
            timed.write.push(wrote);
            timed.read.push(read);
            timed.bytes.push(bytes);
        }
    }
    Ok(timed)
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let rounds = args.iter().position(|a| a == "--rounds").and_then(|i| args.get(i + 1)).and_then(|n| n.parse().ok()).unwrap_or(2000usize).max(20);
    let ctl = Controller::new(Settings::default());
    ctl.set_snapshot(busy_evening());
    let link = Link(Arc::downgrade(&ctl));
    let n4n0 = busy_n4n0();
    let mut wire = Wire { line: Vec::new(), sent: None, client: None };
    // A client that connects is handed everything: the median of fifty connections (each a snapshot of its own: a line
    // already made for a snapshot is not made again).
    let (mut wrote, mut read, mut bytes) = (Vec::new(), Vec::new(), 0);
    for _ in 0..50 {
        let mut connecting = Wire { line: Vec::new(), sent: None, client: None };
        let (w, r, b) = connecting.carry(&Arc::new((*ctl.view()).clone()))?;
        wrote.push(w);
        read.push(r);
        bytes = b;
    }
    println!("the whole state, as a client that connects gets it: {bytes} bytes, written in {:.1} µs, read in {:.1} µs", micros(median(&mut wrote)), micros(median(&mut read)));
    wire.carry(&ctl.view())?;
    println!();
    println!("{:<44} {:>9} {:>9} {:>9} {:>9} {:>10}   {:>9}", "one change (median of the rounds)", "copy µs", "publish µs", "write µs", "read µs", "bytes", "total µs");
    // What N4N0 hands over, stood in for by a copy of this harness's made-up state with what changed
    // (`N4n0Runtime::status` itself is not measured here).
    let handed = |link: &Link, change: &dyn Fn(&mut N4n0Status)| {
        let started = Instant::now();
        let mut status = n4n0.clone();
        change(&mut status);
        let made = started.elapsed();
        link.publish(status);
        Some(made)
    };
    type Change<'a> = Box<dyn FnMut(usize) -> Option<Duration> + 'a>;
    let cases: Vec<(&str, Change)> = vec![
        ("a publish with nothing changed", Box::new(|_| {
            ctl.update(|_| {});
            None
        })),
        ("N4N0's screen open: its panel's text", Box::new(|round| handed(&link, &|s| s.panel = Some(format!("Step {round} of what N4N0 is writing on its panel"))))),
        ("N4N0's screen open: a picture's countdown", Box::new(|round| handed(&link, &|s| s.photo_countdown = Some(round as u32 % 10)))),
        ("processes' footprint (every 3 s)", Box::new(|round| handed(&link, &|s| *s.resources = usage(round + 1)))),
        ("N4N0's answer arriving (a line replaced)", Box::new(|round| {
            handed(&link, &|s| {
                if let Some(last) = s.lines.last_mut() {
                    last.text = format!("N4N0's answer as far as it has arrived, round {round}");
                    last.streaming = true;
                }
            })
        })),
        ("the avatar's speed (VelocityMagnitude)", Box::new(|round| {
            link.motion(Some(("VelocityMagnitude", &OscValue::Float(1.0 + 0.25 * (round % 2) as f32))));
            None
        })),
        ("a recording's statistics (4 a second)", Box::new(|round| {
            ctl.update(|s| {
                let stats = s.capture.stats.get_or_insert_with(Default::default);
                stats.frames = round as u64;
                stats.bytes = round as u64 * 1_000_000;
            });
            None
        })),
        ("someone joins the world", Box::new(|round| {
            ctl.update(|s| {
                s.vrchat.world.players.insert("usr_newcomer".into(), format!("Newcomer {round}"));
            });
            None
        })),
        ("a setting", Box::new(|round| {
            ctl.update(|s| s.settings.walk_boost.strength = (round % 10) as f32 / 10.0);
            None
        })),
    ];
    for (name, mut change) in cases {
        let mut t = case(&ctl, &mut wire, rounds, &mut *change)?;
        anyhow::ensure!(wire.in_step(&ctl.view())?, "{name}: the client does not hold what the service published"); // i18n: not shown (a developer's command line, in English)
        if t.publish.is_empty() {
            println!("{name:<44} published nothing ({} rounds)", t.silent);
            continue;
        }
        let (state, publish, write, read) = (median(&mut t.state), median(&mut t.publish), median(&mut t.write), median(&mut t.read));
        let total = state + publish + write + read;
        let silent = if t.silent > 0 { format!("   ({} rounds published nothing)", t.silent) } else { String::new() };
        println!(
            "{name:<44} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>10}   {:>9.1}{silent}",
            micros(state),
            micros(publish),
            micros(write),
            micros(read),
            median(&mut t.bytes),
            micros(total)
        );
    }
    // What a recording adds: its camera path, as long as it gets, goes with each of its statistics.
    let mut recording = (*ctl.view()).clone();
    recording.capture.trail = (0..6000).map(|i| [i as f32 * 0.3, 1.6, (i as f32 * 0.05).sin() * 4.0]).collect();
    ctl.set_snapshot(recording);
    let mut t = case(&ctl, &mut wire, rounds / 4, |round| {
        ctl.update(|s| {
            let stats = s.capture.stats.get_or_insert_with(Default::default);
            stats.frames = round as u64;
        });
        None
    })?;
    anyhow::ensure!(wire.in_step(&ctl.view())?, "a long camera path: the client does not hold what the service published"); // i18n: not shown (a developer's command line, in English)
    let (publish, write, read) = (median(&mut t.publish), median(&mut t.write), median(&mut t.read));
    println!(
        "{:<44} {:>9} {:>9.1} {:>9.1} {:>9.1} {:>10}   {:>9.1}",
        "… with a long camera path recorded",
        "",
        micros(publish),
        micros(write),
        micros(read),
        median(&mut t.bytes),
        micros(publish + write + read)
    );
    // The recording's real path: the capture worker's statistics as the controller takes them, the camera moving, so
    // the path grows by a point each time and is thinned for the interface as it grows.
    ctl.set_snapshot(busy_evening());
    wire.carry(&ctl.view())?;
    let mut t = case(&ctl, &mut wire, rounds, |round| {
        let at = round as f32;
        ctl.on_worker_event(orbiters_core::protocol::WorkerEvent::Stats(orbiters_core::protocol::CaptureStats {
            frames: round as u64,
            position: Some([at * 0.3, 1.6, (at * 0.05).sin() * 4.0]),
            ..Default::default()
        }));
        None
    })?;
    anyhow::ensure!(wire.in_step(&ctl.view())?, "the camera moving: the client does not hold what the service published"); // i18n: not shown (a developer's command line, in English)
    let (publish, write, read) = (median(&mut t.publish), median(&mut t.write), median(&mut t.read));
    println!(
        "{:<44} {:>9} {:>9.1} {:>9.1} {:>9.1} {:>10}   {:>9.1}",
        "a recording's statistics, the camera moving",
        "",
        micros(publish),
        micros(write),
        micros(read),
        median(&mut t.bytes),
        micros(publish + write + read)
    );
    println!();
    println!("{} publishes in all", ctl.publishes());
    Ok(())
}

fn micros(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}
