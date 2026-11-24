//! A member's conversation with N4N0 across restarts of the app (verify3-r21 M5): what was said, in a private file sealed for this
//! user of this computer (on Windows: DPAPI), under the Orbiters identity's epoch, which belongs to one member and outlives the service. Only what was
//! said: what N4N0 asked, what waits to be sent again and what a turn was doing are never brought back.

use crate::fence::Fence;
use crate::runtime::{N4n0, N4n0Runtime};
use crate::status::{N4n0Line, N4n0Who, SignedAs};
use orbiters_n4n0_core::context::Taint;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const LABEL: &[u8] = b"Orbiters N4N0 conversation";

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

    fn read(file: &std::path::Path) -> Option<Self> {
        let sealed = std::fs::read(file).ok()?;
        serde_json::from_slice(&orbiters_core::open_for_user(&sealed, LABEL).ok()?).ok()
    }

    fn write(&self, file: &std::path::Path) -> anyhow::Result<()> {
        let sealed = orbiters_core::seal_for_user(&serde_json::to_vec(self)?, LABEL)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Ok(orbiters_core::write_atomic_private(file, &sealed)?)
    }

    /// What was said goes on, before what was said since.
    fn restore(self, rt: &mut N4n0Runtime) {
        rt.conversation_others = self.others.filter(|o| o.len() == self.conversation.len()).map_or_else(|| vec![Taint::Others; self.conversation.len()], |o| o.into_iter().map(Mark::taint).collect());
        rt.conversation = self.conversation;
        let after = std::mem::take(&mut *rt.lines);
        *rt.lines = self.lines;
        rt.lines.extend(after);
    }
}

/// What a member had said when who is signed in was no longer known: in memory, theirs alone, until it is known again.
pub(crate) struct Parked {
    member: String,
    kept: Kept,
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
        let kept = Kept::of(&self.n4n0.lock().unwrap(), self.epoch());
        let written = kept.write(&self.files.conversation);
        if let Err(e) = written {
            eprintln!("N4N0's conversation could not be kept: {e:#}"); // i18n: not shown
        }
    }
}

impl N4n0 {
    /// Nothing of a conversation is kept any more (New conversation, another member).
    pub(crate) fn drop_kept_conversation(&self) {
        match std::fs::remove_file(&self.files.conversation) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => eprintln!("N4N0's conversation could not be removed: {e}"), // i18n: not shown
            _ => {}
        }
    }

    /// The conversation kept for `epoch`, when there is one and nothing was said since the start.
    pub(crate) fn resume_account(&self, epoch: u64) {
        if !self.files.conversation.exists() {
            return;
        }
        let Some(kept) = Kept::read(&self.files.conversation).filter(|k| k.epoch == epoch) else {
            // Another member's (or not readable): not theirs to keep.
            return self.drop_kept_conversation();
        };
        {
            let mut rt = self.n4n0.lock().unwrap();
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
            let kept = {
                let rt = self.n4n0.lock().unwrap();
                let said = Kept::of(&rt, rt.account_epoch);
                // Nothing said since the start: what the last run kept of theirs.
                if said.conversation.is_empty() && said.lines.is_empty() {
                    Kept::read(&self.files.conversation).filter(|k| k.epoch == rt.account_epoch)
                } else {
                    Some(said)
                }
            };
            *self.parked.lock().unwrap() = kept.map(|kept| Parked { member: member.to_string(), kept });
        }
        self.forget_account(epoch);
    }

    /// [`crate::Paused::establish_account`].
    pub(crate) fn establish_account(&self, epoch: u64, member: &str) {
        self.forget_account(epoch);
        let Some(mut parked) = self.parked.lock().unwrap().take().filter(|p| p.member == member) else { return };
        parked.kept.epoch = epoch;
        if let Err(e) = parked.kept.write(&self.files.conversation) {
            eprintln!("N4N0's conversation could not be kept: {e:#}"); // i18n: not shown
        }
        parked.kept.restore(&mut self.n4n0.lock().unwrap());
        self.publish();
    }

    pub(crate) fn drop_parked(&self) {
        *self.parked.lock().unwrap() = None;
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
