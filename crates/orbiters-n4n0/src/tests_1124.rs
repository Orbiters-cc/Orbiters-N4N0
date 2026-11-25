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
