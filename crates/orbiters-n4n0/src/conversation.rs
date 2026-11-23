//! A member's conversation with N4N0 across restarts of the app (verify3-r21 M5): what was said, in a private file sealed for this
//! user of this computer (on Windows: DPAPI), under the Orbiters identity's epoch, which belongs to one member and outlives the service. Only what was
//! said: what N4N0 asked, what waits to be sent again and what a turn was doing are never brought back.

use crate::fence::Fence;
use crate::runtime::N4n0;
use crate::status::{N4n0Line, N4n0Who};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const LABEL: &[u8] = b"Orbiters N4N0 conversation";

#[derive(Serialize, Deserialize)]
struct Kept {
    epoch: u64,
    conversation: Vec<Value>,
    lines: Vec<N4n0Line>,
}

impl Fence<'_> {
    /// The conversation as it stands, kept for the next start.
    pub(crate) fn keep_conversation(&self) {
        let kept = {
            let rt = self.n4n0.lock().unwrap();
            // Not an error's line nor a note's: what was said.
            let said = |l: &&N4n0Line| !l.streaming && matches!(l.who, N4n0Who::You | N4n0Who::N4n0);
            Kept { epoch: self.epoch(), conversation: rt.conversation.clone(), lines: rt.lines.iter().filter(said).cloned().collect() }
        };
        let written = serde_json::to_vec(&kept).map_err(anyhow::Error::from).and_then(|plain| {
            let sealed = orbiters_core::seal_for_user(&plain, LABEL)?;
            if let Some(dir) = self.files.conversation.parent() {
                std::fs::create_dir_all(dir)?;
            }
            orbiters_core::write_atomic_private(&self.files.conversation, &sealed)
        });
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
        let Ok(sealed) = std::fs::read(&self.files.conversation) else { return };
        let kept = orbiters_core::open_for_user(&sealed, LABEL).ok().and_then(|plain| serde_json::from_slice::<Kept>(&plain).ok());
        let Some(kept) = kept.filter(|k| k.epoch == epoch) else {
            // Another member's (or not readable): not theirs to keep.
            return self.drop_kept_conversation();
        };
        {
            let mut rt = self.n4n0.lock().unwrap();
            rt.account_epoch = epoch;
            if !rt.conversation.is_empty() || rt.lines.iter().any(|l| l.who == N4n0Who::You) {
                return;
            }
            rt.conversation = kept.conversation;
            let after = std::mem::take(&mut *rt.lines);
            *rt.lines = kept.lines;
            rt.lines.extend(after);
        }
        self.publish();
    }
}
