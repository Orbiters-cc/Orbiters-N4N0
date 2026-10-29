//! N4N0's menu sharing 3D captures (contracts `Docs/orb.md`, Menu, from screen version 12), in the tray service: the
//! computer's captures listed for History, the last one the desktop made, and the shares menus asked, handed to N4N0
//! ([`orbiters_n4n0::N4n0::set_captures`]) whenever where they stand changes (looked at once a second). A share is a
//! Photos & captures job ([`super::shares`]: the same upload records, preparation and server calls); one asked before
//! the capture's 3D view is made waits here, and its reconstruction is queued when nothing makes it.

use super::shares::{self, Entry, Local, Operation, Outcome, Request};
use super::Shared;
use crate::state::{CaptureState, Snapshot};
use orbiters_core::account::Server;
use orbiters_orb::captures::{CapturePhase, CaptureRow, CapturesView, Friend, ShareAsk, Sharing, HISTORY_MAX, RECIPIENTS_MAX};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A share waiting for its capture's 3D view gives up once nothing has made nor queued it for this long.
const WAIT_UNQUEUED: Duration = Duration::from_secs(15);
/// A job just started shows as on its way until the snapshots have it.
const JOB_SHOWS: Duration = Duration::from_secs(10);

/// What a menu asked for a capture.
#[derive(Clone, Debug)]
enum Asked {
    /// Its 3D view first, since `since`.
    Waiting { entry: Box<Entry>, ask: ShareAsk, since: Instant },
    /// A Photos & captures job (its id), started at `since`.
    Job { id: String, ask: ShareAsk, since: Instant },
    /// It could not go: why.
    Failed(String),
}

#[derive(Default)]
pub(super) struct MenuShares {
    /// History's captures as last listed (newest first), whether they were, and why there are none.
    listed: Vec<Entry>,
    listed_once: bool,
    note: String,
    /// The last capture made: its folder, and its row as its manifest and content say (read again while they are not
    /// there yet).
    latest: Option<(PathBuf, CaptureRow)>,
    /// What menus asked, by the capture's folder name.
    asked: HashMap<String, Asked>,
    /// Captures whose 3D view was made since they were listed, by folder name, and the reconstruction running at the
    /// last look.
    made: HashSet<String>,
    running: Option<PathBuf>,
    /// What N4N0 was handed last.
    sent: Option<CapturesView>,
}

/// A capture's folder name: what names it however its path is written (the worker's, the listing's).
fn folder(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn folder_of(key: &str) -> String {
    folder(Path::new(key))
}

/// The server Photos & captures shares with: the environment the settings name.
fn server(development: bool) -> Server {
    if development {
        Server::Development
    } else {
        Server::Production
    }
}

/// History's row for a capture as Photos & captures lists it (where its 3D view and shares stand comes after).
fn listed_row(entry: &Entry) -> CaptureRow {
    let remote = entry.remote.as_ref();
    let reasons = if entry.content.explicit_reasons.is_empty() {
        remote.map(|i| i.explicit_reasons.clone()).unwrap_or_default()
    } else {
        entry.content.explicit_reasons.clone()
    };
    CaptureRow {
        key: entry.key.clone(),
        world: entry.world_name.clone(),
        taken: entry.taken,
        phase: if entry.ready { CapturePhase::Ready } else { CapturePhase::NoView },
        explicit: entry.content.explicit || remote.is_some_and(|i| i.explicit),
        explicit_reasons: reasons,
        public: remote.is_some_and(|i| i.public),
        people: remote.map_or(0, |i| i.grants.len() as u32),
        sharing: Sharing::Idle,
    }
}

/// The last capture's row from its folder: its manifest's world and time (the world VRChat is in until the manifest
/// says), its content evidence, whether its 3D view is made.
fn latest_row(dir: &Path, key: String, snap: &Snapshot) -> CaptureRow {
    let manifest = orbiters_core::bundle::read_manifest(dir).ok();
    let world = manifest.as_ref().and_then(|m| m.world.as_ref()).and_then(|w| w.name.clone()).or_else(|| snap.vrchat.world.world_name.clone());
    let taken = manifest.as_ref().map(|m| shares::unix(&m.created_utc)).filter(|&t| t > 0).unwrap_or_else(|| orbiters_n4n0::unix_ms() / 1000);
    let content = orbiters_core::explicit::Content::read(dir);
    let ready = orbiters_core::reconstruction::read_report(dir).is_some();
    CaptureRow {
        key,
        world: world.unwrap_or_default(),
        taken,
        phase: if ready { CapturePhase::Ready } else { CapturePhase::NoView },
        explicit: content.explicit,
        explicit_reasons: content.explicit_reasons,
        ..Default::default()
    }
}

impl MenuShares {
    /// Where capture `row`'s 3D view and its shares stand now.
    fn live(&self, row: &mut CaptureRow, snap: &Snapshot) {
        let name = folder_of(&row.key);
        let is = |p: &Path| folder(p) == name;
        if snap.capture.session_dir.as_deref().is_some_and(is) {
            match snap.capture.state {
                CaptureState::Starting | CaptureState::Recording | CaptureState::Paused | CaptureState::Importing => row.phase = CapturePhase::Recording,
                CaptureState::Stopping => row.phase = CapturePhase::Saving,
                CaptureState::Idle => {}
            }
        }
        if !matches!(row.phase, CapturePhase::Recording | CapturePhase::Saving) {
            if let Some(job) = snap.reconstruction.as_ref().filter(|j| is(&j.session_dir)) {
                row.phase = CapturePhase::Making { percent: (crate::pages::sessions::overall_progress(job) * 100.0).clamp(0.0, 99.0) as u8 };
            } else if snap.reconstruct_queue.iter().any(|d| is(d)) {
                row.phase = CapturePhase::Waiting;
            } else if self.made.contains(&name) {
                row.phase = CapturePhase::Ready;
            }
        }
        row.sharing = match self.asked.get(&name) {
            None => Sharing::Idle,
            Some(Asked::Failed(message)) => Sharing::Failed { message: message.clone() },
            Some(Asked::Waiting { ask, .. }) => Sharing::Waiting { public: ask.public, people: ask.recipients.len() as u32 },
            Some(Asked::Job { id, ask, since }) => {
                let job = snap.capture_shares.iter().find(|j| &j.id == id);
                let people = ask.recipients.len() as u32;
                match job.map(|j| j.outcome.as_ref()) {
                    // Not in this snapshot yet.
                    None if since.elapsed() < JOB_SHOWS => Sharing::Busy { public: ask.public, people },
                    // Gone from the jobs kept: its row says what the listing knows.
                    None => Sharing::Idle,
                    Some(None) => Sharing::Busy { public: ask.public, people },
                    Some(Some(Outcome::Shared(orbiters_core::shares::Shared::Item { item }))) => {
                        Sharing::Done { public: item.public, people: item.grants.len() as u32 }
                    }
                    Some(Some(Outcome::Shared(orbiters_core::shares::Shared::Request { .. }))) => Sharing::Done { public: ask.public, people },
                    Some(Some(Outcome::Failed { explicit: true, .. })) => Sharing::Explicit,
                    Some(Some(Outcome::Failed { message, .. })) => Sharing::Failed { message: message.clone() },
                    Some(Some(_)) => Sharing::Idle,
                }
            }
        };
        // The server said it is explicit: sharing it publicly asks first from now on.
        if row.sharing == Sharing::Explicit {
            row.explicit = true;
        }
    }

    /// The captures as N4N0's menus show them now.
    fn view(&self, snap: &Snapshot) -> CapturesView {
        let latest = self.latest.as_ref().map(|(_, row)| {
            let mut row = row.clone();
            self.live(&mut row, snap);
            row
        });
        let history = self
            .listed
            .iter()
            .map(|entry| {
                let mut row = listed_row(entry);
                self.live(&mut row, snap);
                row
            })
            .collect();
        CapturesView { latest, history, listed: self.listed_once, note: self.note.clone() }
    }

    /// History's captures from Photos & captures' listing: the sessions, newest first.
    fn list(&mut self, entries: Vec<Entry>) {
        let mut sessions: Vec<Entry> = entries.into_iter().filter(|e| matches!(e.local, Some(Local::Session(_)))).collect();
        sessions.truncate(HISTORY_MAX);
        self.note = if sessions.is_empty() { "None on this computer yet: take a 3D picture".into() } else { String::new() };
        self.listed = sessions;
        self.listed_once = true;
    }
}

impl super::Controller {
    /// Once a second (N4N0's watch) and after a menu asked something: the last capture made, the 3D views made since,
    /// the waiting shares whose view is there (they go now) or that nothing makes; N4N0 is handed the captures when
    /// where they stand changed.
    pub(super) fn menu_captures_tick(self: &Shared) {
        let snap = self.view();
        let mut m = self.menu_shares.lock().unwrap();
        let m = &mut *m;
        let latest = snap.capture.session_dir.clone();
        let known = m.latest.as_ref().map(|(dir, row)| (dir.clone(), row.world.is_empty()));
        match (latest, known) {
            (Some(dir), Some((was, unnamed))) if dir == was && !unnamed => {}
            (Some(dir), _) => {
                // Named as History names it (the sessions folder and the session's name), so both rows are the same capture.
                let key = snap.settings.sessions_dir.join(folder(&dir)).to_string_lossy().into_owned();
                m.latest = Some((dir.clone(), latest_row(&dir, key, &snap)));
            }
            (None, _) => m.latest = None,
        }
        // A reconstruction that ended made its capture's 3D view (when it left its report).
        let running = snap.reconstruction.as_ref().map(|j| j.session_dir.clone());
        if m.running != running {
            if let Some(done) = m.running.take() {
                if orbiters_core::reconstruction::read_report(&done).is_some() {
                    m.made.insert(folder(&done));
                }
            }
            m.running = running;
        }
        // Shares waiting for a 3D view: they go once it is made, and give up when nothing makes it.
        let mut go = Vec::new();
        for (name, asked) in m.asked.iter_mut() {
            let Asked::Waiting { entry, ask, since } = asked else { continue };
            let made = m.made.contains(name) || entry.local.as_ref().is_some_and(|l| orbiters_core::reconstruction::read_report(l.path()).is_some());
            let busy =
                snap.reconstruction.as_ref().is_some_and(|j| folder(&j.session_dir) == *name) || snap.reconstruct_queue.iter().any(|d| folder(d) == *name);
            if made {
                let mut entry = (**entry).clone();
                entry.ready = true;
                go.push((name.clone(), entry, ask.clone()));
            } else if busy {
                *since = Instant::now();
            } else if since.elapsed() > WAIT_UNQUEUED {
                *asked = Asked::Failed("Its 3D view could not be made: reconstruct it in Photos & captures".into());
            }
        }
        for (name, entry, ask) in go {
            m.asked.insert(name, self.menu_share_job(entry, ask, snap.settings.development));
        }
        let view = m.view(&snap);
        if m.sent.as_ref() != Some(&view) {
            m.sent = Some(view.clone());
            // Handed over under the lock, so a newer view never goes before an older one.
            self.n4n0.set_captures(view);
        }
    }

    /// Lists the computer's captures again for History (a menu opened it), as Photos & captures lists them.
    pub(super) fn menu_captures_list(self: &Shared) {
        let settings = self.settings();
        let entries = shares::local_entries(&settings.sessions_dir, server(settings.development));
        self.menu_shares.lock().unwrap().list(entries);
        self.menu_captures_tick();
    }

    /// A share a menu asked: the capture as Photos & captures lists it (its upload, if any, reused), shared at once when
    /// its 3D view is made, else once it is (queued for reconstruction when nothing makes it).
    pub(super) fn menu_capture_share(self: &Shared, ask: ShareAsk) -> Result<(), String> {
        if ask.recipients.len() > RECIPIENTS_MAX {
            return Err(format!("A capture goes to at most {RECIPIENTS_MAX} friends at a time"));
        }
        if !ask.public && ask.recipients.is_empty() {
            return Err("Choose who to share it with".into());
        }
        let name = folder_of(&ask.key);
        let snap = self.view();
        if snap.capture.session_dir.as_deref().is_some_and(|d| folder(d) == name) && (snap.is_capturing() || snap.capture.state == CaptureState::Stopping) {
            return Err("It is still being recorded: share it once it is saved".into());
        }
        {
            let m = self.menu_shares.lock().unwrap();
            let on_its_way = match m.asked.get(&name) {
                Some(Asked::Waiting { .. }) => true,
                Some(Asked::Job { id, .. }) => snap.capture_shares.iter().any(|j| &j.id == id && j.outcome.is_none()),
                _ => false,
            };
            if on_its_way {
                return Ok(());
            }
        }
        let settings = self.settings();
        let entries = shares::local_entries(&settings.sessions_dir, server(settings.development));
        let entry = entries.iter().find(|e| matches!(e.local, Some(Local::Session(_))) && folder_of(&e.key) == name).cloned();
        let Some(mut entry) = entry else { return Err("This capture is no longer on the computer".into()) };
        let made = self.menu_shares.lock().unwrap().made.contains(&name);
        entry.ready |= made;
        let asked = if entry.ready {
            self.menu_share_job(entry, ask, settings.development)
        } else {
            let dir = entry.local.as_ref().map(|l| l.path().to_path_buf()).unwrap_or_default();
            let busy = snap.reconstruction.as_ref().is_some_and(|j| folder(&j.session_dir) == name) || snap.reconstruct_queue.iter().any(|d| folder(d) == name);
            if !busy {
                self.queue_reconstruction(dir).map_err(|e| format!("Its 3D view cannot be made: {e}"))?;
            }
            Asked::Waiting { entry: Box::new(entry), ask, since: Instant::now() }
        };
        {
            let mut m = self.menu_shares.lock().unwrap();
            m.asked.insert(name, asked);
            // The listing it just read is History's too.
            m.list(entries);
        }
        self.menu_captures_tick();
        Ok(())
    }

    /// Starts the Photos & captures job sharing `entry` as `ask` says.
    fn menu_share_job(self: &Shared, entry: Entry, ask: ShareAsk, development: bool) -> Asked {
        let id = format!("n4n0-menu-{}", uuid::Uuid::new_v4().simple());
        let recipients =
            ask.recipients.iter().map(|f| orbiters_core::shares::Recipient { vrchat_user_id: f.id.clone(), display_name: f.name.clone() }).collect();
        let share = orbiters_core::shares::Share { public: ask.public, recipients, acknowledge_explicit: ask.acknowledge_explicit, message: String::new() };
        let request = Request { id: id.clone(), server: server(development), entry, extra: Vec::new(), operation: Operation::Share(share) };
        // The tray owns it from here (its work runs on a thread of its own).
        self.capture_share(request);
        Asked::Job { id, ask, since: Instant::now() }
    }

    /// The VRChat friends a capture can go to: the member's VRChat session when there is one, else VRCX's or VRCNext's.
    pub(super) fn menu_friends(&self) -> Result<Vec<Friend>, String> {
        let users = self.vrchat_source().friends().map_err(|e| format!("Friends could not be read: {e}"))?;
        if users.is_empty() {
            return Err("No VRChat friends found: sign in to VRChat in Orbiters - N4N0, or use VRCX or VRCNext".into());
        }
        Ok(users.into_iter().map(|u| Friend { id: u.id, name: u.display_name }).collect())
    }
}

/// A capture's picture for N4N0's menus (its key: the session's folder), as the session's thumbnail.
pub(crate) fn capture_picture(key: &str) -> Option<orbiters_orb::picture::Rgb> {
    let (width, height, rgba) = orbiters_core::bundle::read_thumbnail(Path::new(key))?;
    let pixels = rgba.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect();
    Some(orbiters_orb::picture::Rgb { width: width as usize, height: height as usize, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::shares::ShareJob;
    use crate::state::ReconstructionJob;

    fn entry(dir: &Path, ready: bool) -> Entry {
        Entry {
            key: dir.to_string_lossy().into(),
            world_id: "wrld_a".into(),
            world_name: "Rooftop".into(),
            taken: 1,
            instance: None,
            source_key: Some(folder(dir)),
            people: Vec::new(),
            local: Some(Local::Session(dir.to_path_buf())),
            remote: None,
            content: Default::default(),
            ready,
            picture: None,
        }
    }

    /// A capture's row follows its recording, its 3D view and its share, by its folder's name whichever path names it.
    #[test]
    fn a_rows_view_and_share_follow_the_recording_the_reconstruction_and_the_job() {
        let mut m = MenuShares::default();
        let mut snap = Snapshot::new(Default::default());
        let dir = PathBuf::from("C:/Sessions/2026-10-29_10-00-00");
        m.list(vec![entry(&dir, false)]);
        let row = |m: &MenuShares, snap: &Snapshot| m.view(snap).history[0].clone();
        assert_eq!(row(&m, &snap).phase, CapturePhase::NoView);
        snap.capture.session_dir = Some(PathBuf::from(r"C:\Sessions\2026-10-29_10-00-00"));
        snap.capture.state = CaptureState::Recording;
        assert_eq!(row(&m, &snap).phase, CapturePhase::Recording);
        snap.capture.state = CaptureState::Idle;
        snap.reconstruct_queue.push(dir.clone());
        assert_eq!(row(&m, &snap).phase, CapturePhase::Waiting);
        snap.reconstruct_queue.clear();
        snap.reconstruction = Some(ReconstructionJob::new(dir.clone(), false));
        assert!(matches!(row(&m, &snap).phase, CapturePhase::Making { .. }));
        snap.reconstruction = None;
        m.made.insert(folder(&dir));
        assert_eq!(row(&m, &snap).phase, CapturePhase::Ready);
        // Its share: on its way, then how it went; the server's explicit answer flags it.
        let name = folder(&dir);
        let ask = ShareAsk { key: entry(&dir, true).key, public: true, ..Default::default() };
        m.asked.insert(name.clone(), Asked::Job { id: "j".into(), ask: ask.clone(), since: Instant::now() - JOB_SHOWS });
        assert_eq!(row(&m, &snap).sharing, Sharing::Idle, "a job no longer kept");
        snap.capture_shares.push(ShareJob { id: "j".into(), key: ask.key.clone(), stage: Default::default(), outcome: None });
        assert_eq!(row(&m, &snap).sharing, Sharing::Busy { public: true, people: 0 });
        snap.capture_shares[0].outcome = Some(Outcome::Failed { message: orbiters_core::shares::EXPLICIT_WARNING.into(), explicit: true });
        let flagged = row(&m, &snap);
        assert_eq!((flagged.sharing, flagged.explicit), (Sharing::Explicit, true));
        snap.capture_shares[0].outcome = Some(Outcome::Failed { message: "Sign in to Orbiters in Settings first.".into(), explicit: false });
        assert_eq!(row(&m, &snap).sharing, Sharing::Failed { message: "Sign in to Orbiters in Settings first.".into() });
        m.asked.insert(name, Asked::Waiting { entry: Box::new(entry(&dir, false)), ask, since: Instant::now() });
        assert_eq!(row(&m, &snap).sharing, Sharing::Waiting { public: true, people: 0 });
    }

    #[test]
    fn history_lists_the_newest_sessions_only() {
        let mut m = MenuShares::default();
        let mut entries: Vec<Entry> = (0..HISTORY_MAX + 5).map(|i| entry(&PathBuf::from(format!("C:/S/{i:03}")), true)).collect();
        let mut picture = entry(Path::new("C:/Pictures/a.png"), true);
        picture.local = Some(Local::Picture(PathBuf::from("C:/Pictures/a.png")));
        entries.insert(0, picture);
        m.list(entries);
        let view = m.view(&Snapshot::new(Default::default()));
        assert_eq!(view.history.len(), HISTORY_MAX);
        assert!(view.listed && view.history[0].key.ends_with("000"), "pictures are not 3D captures");
        m.list(Vec::new());
        assert!(m.view(&Snapshot::new(Default::default())).note.starts_with("None on this computer"));
    }
}
