//! A member's conversation with N4N0 across restarts of the app (verify3-r21 M5): what was said, in a private file sealed for this
//! user of this computer ([`orbiters_core::sealed`]), under the Orbiters identity's epoch, which belongs to one member and outlives the service. Only what was
//! said: what N4N0 asked, what waits to be sent again and what a turn was doing are never brought back.
//!
//! A file goes only once it was read and is another member's, or when the conversation ends (New conversation, a sign-out, another
//! member). One that cannot be read now (N4N-1) stays, is tried again, and is kept aside (`.bad-…`) by what is written in its place.

use orbiters_core::locks::*;
use crate::fence::Fence;
use crate::runtime::{N4n0, N4n0Runtime};
use crate::status::{N4n0Line, N4n0Who, SignedAs};
use orbiters_i18n::{mark, tr};
use orbiters_n4n0_core::context::Taint;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const LABEL: &[u8] = b"Orbiters N4N0 conversation";
/// What the person reads of a conversation that could not be read.
const NOT_READ: &str = mark("What you and N4N0 said before could not be read: it is kept, and tried again later.");

#[derive(Serialize, Deserialize)]
struct Kept {
    epoch: u64,
    conversation: Vec<Value>,
    /// Whose words were read where each was said (none told: an older file, others' for all of them; `true` or `false`:
    /// 0.3.9's, others' or none).
    #[serde(default)]
    others: Option<Vec<Mark>>,
    lines: Vec<N4n0Line>,
}

/// A message's mark in the file.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Mark {
    Known(Taint),
    Others(bool),
}

impl Mark {
    fn taint(self) -> Taint {
        match self {
            Mark::Known(taint) => taint,
            Mark::Others(others) => Taint::others(others),
        }
    }
}

impl Kept {
    /// As it stands, under `epoch`: not an error's line nor a note's, what was said.
    fn of(rt: &N4n0Runtime, epoch: u64) -> Self {
        let said = |l: &&N4n0Line| !l.streaming && matches!(l.who, N4n0Who::You | N4n0Who::N4n0);
        Kept { epoch, conversation: rt.conversation.clone(), others: Some(rt.conversation_others.iter().copied().map(Mark::Known).collect()), lines: rt.lines.iter().filter(said).cloned().collect() }
    }

    /// What was said, then what was said since.
    fn then(self, since: Kept) -> Self {
        let marks = [self.taints(), since.taints()].concat().into_iter().map(Mark::Known).collect();
        Kept { epoch: since.epoch, conversation: [self.conversation, since.conversation].concat(), others: Some(marks), lines: [self.lines, since.lines].concat() }
    }

    /// `None`: nothing is kept. An error: something is, and it could not be read now (said in the log).
    fn read(file: &std::path::Path) -> Result<Option<Self>, anyhow::Error> {
        let read = orbiters_core::sealed::read(file, LABEL);
        if let Err(e) = &read {
            eprintln!("N4N0's conversation could not be read: {e:#}"); // i18n: not shown
        }
        read
    }

    fn write(&self, file: &std::path::Path) {
        if let Err(e) = orbiters_core::sealed::write(file, self, LABEL) {
            eprintln!("N4N0's conversation could not be kept: {e:#}"); // i18n: not shown
        }
    }

    fn taints(&self) -> Vec<Taint> {
        self.others.as_ref().filter(|o| o.len() == self.conversation.len()).map_or_else(|| vec![Taint::Others; self.conversation.len()], |o| o.iter().map(|m| m.taint()).collect())
    }

    /// What was said goes on, before what was said since.
    fn restore(self, rt: &mut N4n0Runtime) {
        let mut marks = self.taints();
        marks.append(&mut rt.conversation_others);
        rt.conversation_others = marks;
        let since = std::mem::replace(&mut rt.conversation, self.conversation);
        rt.conversation.extend(since);
        let after = std::mem::take(&mut *rt.lines);
        *rt.lines = self.lines;
        rt.lines.extend(after);
    }
}

/// What a member had said when who is signed in was no longer known: in memory, theirs alone, until it is known again.
pub(crate) struct Parked {
    member: String,
    kept: Option<Kept>,
    /// The file could not be read then: it stays, theirs when it is of this epoch.
    unread: Option<u64>,
}

/// The conversation after a turn, with whose words were read where each message was said: `full` (and its `marks`) before it,
/// of which `sent` went with it (`some`: not all of them, those left out being the marked ones); `now` as the turn left it,
/// having read `taint`.
pub(crate) fn merged(full: Vec<Value>, marks: Vec<Taint>, sent: &[Value], some: bool, now: Vec<Value>, taint: Taint) -> (Vec<Value>, Vec<Taint>) {
    if now.len() >= sent.len() && now[..sent.len()] == *sent {
        let mut marks: Vec<Taint> = (0..full.len()).map(|n| marks.get(n).copied().unwrap_or_default()).collect();
        let mut all = full;
        all.extend(now.into_iter().skip(sent.len()));
        marks.resize(all.len(), taint);
        return (all, marks);
    }
    // Written again by Orbiters: whatever any of it was said with (of what went, the turn itself says).
    let mut all = taint;
    if !some {
        marks.into_iter().for_each(|m| all |= m);
    }
    let marks = vec![all; now.len()];
    (now, marks)
}

impl Fence<'_> {
    /// The conversation as it stands, kept for the next start.
    pub(crate) fn keep_conversation(&self) {
        // A member's only.
        if self.host.signed_as() != SignedAs::Member {
            return;
        }
        let file = &self.files.conversation;
        // What the last run kept could not be read: it may be by now, and goes before what was said since.
        let mut found = false;
        if orbiters_core::was_unreadable(file) {
            if self.n4n0.locked().thinking() {
                // Its end keeps it all.
                return;
            }
            if let Ok(Some(kept)) = Kept::read(file).map(|kept| kept.filter(|k| k.epoch == self.epoch())) {
                kept.restore(&mut self.n4n0.locked());
                found = true;
            }
        }
        Kept::of(&self.n4n0.locked(), self.epoch()).write(file);
        if found {
            self.publish();
        }
    }
}

impl N4n0 {
    /// Nothing of a conversation is kept any more (New conversation, another member).
    pub(crate) fn drop_kept_conversation(&self) {
        Self::removed(orbiters_core::sealed::remove(&self.files.conversation));
    }

    /// The file alone goes: it was read.
    fn drop_conversation_file(&self) {
        Self::removed(std::fs::remove_file(&self.files.conversation));
    }

    fn removed(removed: std::io::Result<()>) {
        if let Err(e) = removed.or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) {
            eprintln!("N4N0's conversation could not be removed: {e}"); // i18n: not shown
        }
    }

    /// The conversation kept for `epoch`, when there is one and nothing was said since the start.
    pub(crate) fn resume_account(&self, epoch: u64) {
        let kept = match Kept::read(&self.files.conversation) {
            Ok(None) => return,
            // Another member's: not theirs to keep.
            Ok(Some(kept)) if kept.epoch != epoch => return self.drop_conversation_file(),
            Ok(Some(kept)) => kept,
            Err(_) => return self.add_line(N4n0Who::Note, tr(NOT_READ)),
        };
        {
            let mut rt = self.n4n0.locked();
            rt.account_epoch = epoch;
            if !rt.conversation.is_empty() || rt.lines.iter().any(|l| l.who == N4n0Who::You) {
                return;
            }
            kept.restore(&mut rt);
        }
        self.publish();
    }

    /// [`crate::Paused::park_account`].
    pub(crate) fn park_account(&self, epoch: u64, member: Option<&str>) {
        if let Some(member) = member {
            let said = {
                let rt = self.n4n0.locked();
                Kept::of(&rt, rt.account_epoch)
            };
            let theirs = said.epoch;
            let mut kept = Some(said).filter(|said| !said.conversation.is_empty() || !said.lines.is_empty());
            // Nothing said since the start: what the last run kept of theirs.
            if kept.is_none() {
                kept = Kept::read(&self.files.conversation).unwrap_or(None).filter(|k| k.epoch == theirs);
            }
            let unread = orbiters_core::was_unreadable(&self.files.conversation).then_some(theirs);
            *self.parked.locked() = Some(Parked { member: member.to_string(), kept, unread });
        }
        // What waits, in memory or unread, is not for the next one to see; it goes when it is not theirs.
        if self.parked.locked().as_ref().is_none_or(|p| p.unread.is_none()) {
            self.drop_conversation_file();
        }
        self.forget_said(epoch);
    }

    /// [`crate::Paused::establish_account`].
    pub(crate) fn establish_account(&self, epoch: u64, member: &str) {
        let Some(Parked { mut kept, unread, .. }) = self.parked.locked().take().filter(|p| p.member == member) else {
            return self.forget_account(epoch);
        };
        self.forget_said(epoch);
        let file = &self.files.conversation;
        if let Some(theirs) = unread {
            match Kept::read(file) {
                Ok(Some(earlier)) if earlier.epoch == theirs => {
                    kept = Some(match kept {
                        Some(since) => earlier.then(since),
                        None => earlier,
                    });
                }
                Ok(_) => self.drop_conversation_file(),
                // Their epoch is another one now: aside, it is not taken for another member's.
                Err(_) => {
                    Self::removed(orbiters_core::sealed::set_aside(file));
                    self.add_line(N4n0Who::Note, tr(NOT_READ));
                }
            }
        }
        if let Some(mut kept) = kept {
            kept.epoch = epoch;
            kept.write(file);
            kept.restore(&mut self.n4n0.locked());
        }
        self.publish();
    }

    pub(crate) fn drop_parked(&self) {
        *self.parked.locked() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verify4-r21 P4: a turn's messages keep what was read with them, and those left out of a request come back.
    #[test]
    fn messages_keep_whether_others_text_was_read_with_them() {
        let m = |t: &str| json!({ "role": "user", "content": t });
        let full = vec![m("a"), m("b"), m("c")];
        let (no, yes, labels) = (Taint::None, Taint::Others, Taint::Labels);
        let marks = vec![no, yes, no];
        // All of it was sent.
        let now = vec![m("a"), m("b"), m("c"), m("d")];
        assert_eq!(merged(full.clone(), marks.clone(), &full, false, now, labels), (vec![m("a"), m("b"), m("c"), m("d")], vec![no, yes, no, labels]));
        // The ones of no others' text only: the other is still there after it.
        let sent = vec![m("a"), m("c")];
        let now = vec![m("a"), m("c"), m("d")];
        assert_eq!(merged(full.clone(), marks.clone(), &sent, true, now, yes), (vec![m("a"), m("b"), m("c"), m("d")], vec![no, yes, no, yes]));
        // Written again by Orbiters.
        assert_eq!(merged(full.clone(), marks.clone(), &full, false, vec![m("z")], no), (vec![m("z")], vec![yes]));
        assert_eq!(merged(full, marks, &sent, true, vec![m("z")], no), (vec![m("z")], vec![no]));
        // As 0.3.9 wrote them, and as they are now.
        let read: Vec<Mark> = serde_json::from_str(r#"[true,false,"none","labels","others"]"#).unwrap();
        assert_eq!(read.into_iter().map(Mark::taint).collect::<Vec<_>>(), [yes, no, no, labels, yes]);
        assert_eq!(serde_json::to_string(&Mark::Known(labels)).unwrap(), r#""labels""#);
    }
}
