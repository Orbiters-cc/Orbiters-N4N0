//! Review 1124's findings of N4N0 (N4N.md), driven through the real commands and turns with only the model faked.

use super::*;

// --- N4N-1: a conversation that cannot be read now ---

/// The app started again with `n`'s files.
fn restart(t: &Arc<TestHost>, n: &Shared) -> Shared {
    let platform = Platform {
        clipboard: Box::new(FakeClipboard),
        browser: Box::new(Device::default().browser.clone()),
        sounds: Box::new(Device::default().sounds.clone()),
        locale: Box::new(English),
        ears: Box::new(Device::default().ears.clone()),
        media: Arc::new(Watcher::new(|| Box::new(NoMedia))),
        mouse: None,
    };
    N4n0::new(t.clone(), t.clone(), platform, n.files.clone())
}

/// A member who said something about the sea, which is kept: the file, as it is.
fn said() -> (Arc<TestHost>, Shared, Vec<u8>) {
    let (t, n, _device) = ready();
    let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "the sea" }, { "role": "assistant", "content": "Blue." }]), json!([]))]);
    t.sign_in(base, "orbit-a");
    n.asked("say something about the sea".into(), "typed");
    turn_ended(&n, &bodies, 1);
    until("the conversation is kept", || n.files.conversation.exists());
    let kept = std::fs::read(&n.files.conversation).unwrap();
    (t, n, kept)
}

fn about_the_sea(n: &Shared) -> bool {
    let rt = n.n4n0.lock().unwrap();
    rt.conversation.len() == rt.conversation_others.len() && rt.conversation.first().is_some_and(|m| m["content"] == "the sea") && rt.lines.iter().any(|l| l.text.contains("the sea"))
}

const DAMAGED: &[u8] = b"\0held by another program, or sealed with a key that cannot be had now";

/// What cannot be read now is neither removed nor taken for another member's: it is there at the next start, or at the next answer;
/// written over, it is kept aside; New conversation ends all of it.
#[test]
fn a_conversation_that_cannot_be_read_now_is_not_lost() {
    let (t, n, kept) = said();
    let (file, epoch) = (n.files.conversation.clone(), shown_epoch(&t));
    std::fs::write(&file, DAMAGED).unwrap();
    let again = restart(&t, &n);
    again.pause_effects().resume_account(epoch);
    assert_eq!(std::fs::read(&file).ok().as_deref(), Some(DAMAGED), "still there");
    assert!(!about_the_sea(&again));
    assert!(again.n4n0.lock().unwrap().lines.iter().any(|l| l.text.contains("could not be read")), "and said");
    // The next start reads it.
    std::fs::write(&file, &kept).unwrap();
    let later = restart(&t, &n);
    later.pause_effects().resume_account(epoch);
    assert!(about_the_sea(&later));
    // Or the next answer does: before what was said since.
    std::fs::write(&file, DAMAGED).unwrap();
    let late = restart(&t, &n);
    late.pause_effects().resume_account(epoch);
    late.n4n0.lock().unwrap().conversation.push(json!({ "role": "user", "content": "since" }));
    late.n4n0.lock().unwrap().conversation_others.push(Default::default());
    std::fs::write(&file, &kept).unwrap();
    late.acts().keep_conversation();
    assert!(about_the_sea(&late) && late.n4n0.lock().unwrap().conversation.last().unwrap()["content"] == "since");
    assert!(orbiters_core::sealed::asides(&file).is_empty());
    // Never readable: kept aside by what is said next, until New conversation.
    std::fs::write(&file, DAMAGED).unwrap();
    let never = restart(&t, &n);
    never.publish();
    never.pause_effects().resume_account(epoch);
    never.acts().keep_conversation();
    let aside: Vec<_> = orbiters_core::sealed::asides(&file).iter().map(|f| std::fs::read(f).unwrap()).collect();
    assert_eq!(aside, [DAMAGED]);
    assert_ne!(std::fs::read(&file).unwrap(), DAMAGED);
    never.command(Command::NewConversation { conversation: Some(t.view().conversation), stamp: never.stamped() });
    assert!(!file.exists() && orbiters_core::sealed::asides(&file).is_empty());
    t.shutdown(&n);
}

/// The same where who is signed in is not known for a while: the same member's comes back, another's is gone.
#[test]
fn a_conversation_not_read_when_its_member_was_not_known_is_theirs_again() {
    for (readable, member) in [(true, "production/1"), (false, "production/1"), (true, "production/2")] {
        let (t, n, kept) = said();
        let (file, epoch) = (n.files.conversation.clone(), shown_epoch(&t));
        let again = restart(&t, &n);
        again.n4n0.lock().unwrap().account_epoch = epoch;
        std::fs::write(&file, DAMAGED).unwrap();
        again.pause_effects().park_account(epoch + 1, Some("production/1"));
        assert!(file.exists() && !about_the_sea(&again));
        if readable {
            std::fs::write(&file, &kept).unwrap();
        }
        again.pause_effects().establish_account(epoch + 2, member);
        let same = member == "production/1";
        assert_eq!(about_the_sea(&again), readable && same);
        let aside = orbiters_core::sealed::asides(&file);
        match (readable, same) {
            // Kept under their epoch now.
            (true, true) => assert!(aside.is_empty() && file.exists()),
            (false, _) => assert!(aside.len() == 1 && std::fs::read(&aside[0]).unwrap() == DAMAGED, "never read: not another member's"),
            _ => assert!(aside.is_empty() && !file.exists(), "another member's sign-in ends it"),
        }
        t.shutdown(&n);
    }
}

// --- N4N-2: notes that did not fit ---

/// Every note reaches the model whole before it is dropped.
#[test]
fn a_note_the_model_was_not_told_stays_for_the_next_turn() {
    let (t, n, _device) = ready();
    let notes: Vec<String> = (0..5).map(|i| format!("NOTE-{i} {} END-{i}", "\u{1F600}".repeat(900))).collect();
    n.n4n0.lock().unwrap().notes.extend(notes.iter().map(|text| orbiters_n4n0_core::context::Note { text: text.clone(), taint: Default::default() }));
    let mut told = vec![0; notes.len()];
    for _ in 0..3 {
        let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), json!([]))]);
        t.sign_in(base, "orbit-a");
        n.asked("tell me something nice".into(), "typed");
        turn_ended(&n, &bodies, 1);
        let summary = bodies.lock().unwrap()[0]["context"]["summary"].as_str().unwrap().to_string();
        for (i, note) in notes.iter().enumerate() {
            assert_eq!(summary.contains(&format!("NOTE-{i} ")), summary.contains(note), "NOTE-{i} in part");
            told[i] += summary.contains(note) as usize;
        }
    }
    assert_eq!(told, [1; 5], "each told once, whole");
    assert!(n.n4n0.lock().unwrap().notes.is_empty());
    t.shutdown(&n);
}

// --- N4N-4: a handover and a server that takes its time ---

/// A stand-in Orbiters that starts reading a request's body once `read` is set, then never answers: how much of it came.
fn mute_orbiters(read: Arc<std::sync::atomic::AtomicBool>) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let (came, length) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (c, l) = (came.clone(), length.clone());
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                l.store(v.trim().parse().unwrap(), Ordering::SeqCst);
            }
            line.clear();
        }
        while !read.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut some = [0; 65536];
        while let Ok(n) = reader.read(&mut some) {
            if n == 0 {
                break;
            }
            c.fetch_add(n, Ordering::SeqCst);
        }
    });
    (base, came, length)
}

/// A request that left does not hold a sign-out while Orbiters has not answered; one still leaving does, so that nothing of it goes
/// after the handover.
#[test]
fn a_handover_waits_for_a_request_to_leave_not_for_its_answer() {
    // It left: the handover comes although nothing is answered.
    let (t, n, _device) = ready();
    let (base, came, length) = mute_orbiters(Arc::new(true.into()));
    t.sign_in(base, "orbit-a");
    n.asked("say something about the sea".into(), "typed");
    until("the request", || length.load(Ordering::SeqCst) > 0 && came.load(Ordering::SeqCst) == length.load(Ordering::SeqCst));
    let start = Instant::now();
    t.switch_member(&n, None);
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    t.shutdown(&n);
    // Still leaving (the other side reads nothing yet): the handover waits for all of it.
    let (t, n, _device) = ready();
    let read = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (base, came, length) = mute_orbiters(read.clone());
    t.sign_in(base, "orbit-a");
    {
        let mut rt = n.n4n0.lock().unwrap();
        rt.conversation = vec![json!({ "role": "user", "content": "x".repeat(64 << 20) })];
        rt.conversation_others = vec![Default::default()];
    }
    n.asked("say something about the sea".into(), "typed");
    until("the request begins", || length.load(Ordering::SeqCst) > 0);
    let over = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (t2, n2, done) = (t.clone(), n.clone(), over.clone());
    let switching = std::thread::spawn(move || {
        t2.switch_member(&n2, None);
        done.store(true, Ordering::SeqCst);
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(!over.load(Ordering::SeqCst), "before the request left");
    read.store(true, Ordering::SeqCst);
    switching.join().unwrap();
    until("all of it", || came.load(Ordering::SeqCst) == length.load(Ordering::SeqCst));
    t.shutdown(&n);
}

// --- N4N-3: nothing runs while nothing happens ---

/// N4N0's threads run, the speech models on this device.
fn started(n: &Shared) {
    n.start();
    n.n4n0.lock().unwrap().voice.installed = true;
}

/// What N4N0 hands over while nobody does anything, its threads running: nothing, with the assistant off as with it on and its ears
/// there.
#[test]
fn nothing_is_handed_over_while_nothing_happens() {
    for assistant in [false, true] {
        let (t, n, device) = ready();
        t.settings.lock().unwrap().n4n0.assistant = assistant;
        t.sign_in("http://127.0.0.1:9/".into(), "orbit-a");
        started(&n);
        until("the ears follow", || device.ears.workers.lock().unwrap().len() == assistant as usize);
        std::thread::sleep(Duration::from_millis(800));
        let before = t.publishes.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(t.publishes.load(Ordering::SeqCst) - before, 0, "the assistant {assistant}");
        t.shutdown(&n);
    }
}

/// XCT-23: in an app that wakes it, N4N0's thread makes no step while nothing happens, and one at once for what does: a write
/// nobody published, what the app says changed. With VRChat there it looks again as before.
#[test]
fn it_sleeps_while_nothing_happens_and_wakes_for_what_does() {
    let (t, n, _device) = ready();
    t.wakes.store(true, Ordering::SeqCst);
    t.settings.lock().unwrap().n4n0.assistant = false;
    started(&n);
    n.look();
    std::thread::sleep(Duration::from_millis(800));
    let steps = n.steps();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(n.steps() - steps <= 1, "{} steps in a second and a half", n.steps() - steps);
    // Written to by another thread, which publishes nothing.
    n.n4n0.lock().unwrap().line(N4n0Who::Note, "unannounced");
    let wrote = Instant::now();
    until("a line", || t.view().lines.iter().any(|l| l.text == "unannounced"));
    assert!(wrote.elapsed() < Duration::from_secs(5));
    // What it looks at in the app.
    std::thread::sleep(Duration::from_millis(500));
    let steps = n.steps();
    n.look();
    until("a step", || n.steps() > steps);
    // VRChat runs: four looks a second.
    t.scene(|s| s.vrchat = true);
    n.look();
    std::thread::sleep(Duration::from_millis(500));
    let steps = n.steps();
    std::thread::sleep(Duration::from_secs(2));
    assert!(n.steps() - steps >= 4, "{}", n.steps() - steps);
    t.scene(|s| s.vrchat = false);
    n.look();
    std::thread::sleep(Duration::from_millis(800));
    let steps = n.steps();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(n.steps() - steps <= 1, "asleep again: {}", n.steps() - steps);
    t.shutdown(&n);
}

/// What nobody publishes by itself, a setting changed and VRChat closed are followed all the same.
#[test]
fn what_changes_unannounced_is_followed() {
    let (t, n, device) = ready();
    t.sign_in("http://127.0.0.1:9/".into(), "orbit-a");
    started(&n);
    let worker = |i: usize| device.ears.workers.lock().unwrap().get(i).cloned();
    until("the ears start", || worker(0).is_some());
    n.n4n0.lock().unwrap().line(N4n0Who::Note, "unannounced");
    until("a line", || t.view().lines.iter().any(|l| l.text == "unannounced"));
    t.settings.lock().unwrap().n4n0.mic_device = "another".into();
    until("the microphone", || worker(0).unwrap().commands().iter().any(|c| matches!(c, VoiceCommand::Mic { mic: Some(m) } if m == "another")));
    // Off and on in Settings.
    t.settings.lock().unwrap().n4n0.assistant = false;
    until("the ears stop", || *worker(0).unwrap().dropped.lock().unwrap() && t.view().voice.readiness == VoiceReadiness::AssistantOff);
    t.settings.lock().unwrap().n4n0.assistant = true;
    until("they start again", || worker(1).is_some());
    // VRChat closed and started, the member gone.
    t.scene(|s| s.vrchat = false);
    until("no ears without VRChat", || *worker(1).unwrap().dropped.lock().unwrap());
    t.scene(|s| s.vrchat = true);
    until("with it again", || worker(2).is_some());
    *t.server.lock().unwrap() = None;
    until("nobody is signed in", || *worker(2).unwrap().dropped.lock().unwrap() && t.view().voice.readiness == VoiceReadiness::SignIn);
    t.shutdown(&n);
}

/// The ears look again by themselves when a wait of theirs ends: a worker kept for a while without VRChat, one that failed to start.
#[test]
fn the_ears_say_when_to_look_again() {
    let (t, n, device) = ready();
    t.sign_in("http://127.0.0.1:9/".into(), "orbit-a");
    let in_a_while = |due: Option<Instant>, least: u64, most: u64| {
        let left = due.expect("a wait").saturating_duration_since(Instant::now());
        assert!(left >= Duration::from_secs(least) && left <= Duration::from_secs(most), "{left:?}");
    };
    assert!(n.voice_follow().is_none(), "VRChat runs: only a change matters");
    t.scene(|s| s.vrchat = false);
    assert!(n.voice_follow().is_none() && device.ears.workers.lock().unwrap().len() == 1);
    // A sentence asked for meanwhile keeps a worker for a while.
    n.app_talk(true, false, at(Surface::Window, None, shown_epoch(&t)), None);
    in_a_while(n.voice_follow(), 60, 155);
    // It stops on its own: again in a while.
    t.scene(|s| s.vrchat = true);
    device.ears.workers.lock().unwrap().last().unwrap().clone().crash(3, "gone");
    n.n4n0.lock().unwrap().voice.error = None;
    in_a_while(n.voice_follow(), 25, 30);
    t.shutdown(&n);
}

fn at(surface: Surface, question: Option<u64>, epoch: u64) -> Option<Stamp> {
    Some(Stamp { epoch, question, surface, seq: crate::next_input() })
}
