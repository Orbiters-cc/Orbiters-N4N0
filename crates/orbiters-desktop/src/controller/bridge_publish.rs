//! What the VRChat bridge's tour tools (bridge/tour.rs) ask of the tray service: the Orbiters environment (its Settings'
//! Development environment switch, the very switch a person turns), a session's 3D picture published as Photos & captures'
//! Share publicly does it (controller/shares.rs `share`, its upload records, the same files), and a picture's files freed as
//! the library's "Keep only the 3D view" and "Delete" do it (orbiters_core::cleanup).
//!
//! Publishing runs here, not through `Controller::capture_share`: that one tells the person of every share with a notice,
//! and a tour publishes hundreds while nobody watches. The library and the Map explorer are refreshed after each all the same.

use super::{Controller, Shared};
use crate::bridge::game::{DiskSeen, EnvironmentSeen, Failure, PublishSeen};
use orbiters_core::account::{self, Server};
use orbiters_core::shares::{self, Kind};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How a queued picture is published: Photos & captures' share for real ([`super::shares::share_as`], as whom it was asked
/// as), a stand-in in tests.
type ShareFn = Arc<dyn Fn(Option<&super::shares::Whose>, super::shares::Entry, shares::Share, super::shares::Report) -> anyhow::Result<shares::Shared> + Send + Sync>;

/// A picture waiting to be published.
struct Job {
    dir: PathBuf,
    server: Server,
    entry: super::shares::Entry,
    public: bool,
    /// The account signed in there as it was asked (none was): the one it is published as, however long it waits.
    whose: Option<super::shares::Whose>,
}

/// Publishing, one picture at a time and in the order asked, pacing itself on Orbiters' answer: a 429 holds the queue until
/// its `Retry-After` has passed (nothing is sent meanwhile: each refused request counts in Orbiters' window), then the
/// same picture goes again. What Orbiters accepted of it before (its upload, by the upload records) is not sent again.
///
/// Why: on the world tour (2026-11-15 05:55 to 05:58Z) twelve publishes went at once, the thirteenth and every retry got
/// "Too many attempts": production's `POST /maps/worlds/:worldId/previews` allows 12 an hour (`maps-preview-upload`, a
/// fixed window from the first upload).
pub(super) struct Pacer {
    /// By session folder: the environment it goes to, and where it stands.
    jobs: Mutex<HashMap<PathBuf, (Server, PublishSeen)>>,
    queue: Mutex<VecDeque<Job>>,
    /// Orbiters asked to wait until then, per environment.
    hold: Mutex<Vec<(Server, Instant)>>,
    working: Mutex<bool>,
    share: ShareFn,
    /// After each picture (refresh the library and the Map explorer).
    after: Arc<dyn Fn() + Send + Sync>,
}

impl Pacer {
    fn new(share: ShareFn, after: Arc<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new(Self { jobs: Mutex::default(), queue: Mutex::default(), hold: Mutex::default(), working: Mutex::new(false), share, after })
    }

    /// Seconds Orbiters still asked to wait, in that environment.
    fn wait_s(&self, server: Server) -> Option<u64> {
        let until = self.hold.lock().unwrap().iter().find(|(s, _)| *s == server)?.1;
        let left = until.saturating_duration_since(Instant::now());
        (!left.is_zero()).then(|| left.as_secs().max(1))
    }

    /// Queued: published in its turn. A picture already queued or under way is not queued twice.
    fn add(self: &Arc<Self>, job: Job) {
        {
            let mut jobs = self.jobs.lock().unwrap();
            if matches!(jobs.get(&job.dir), Some((_, PublishSeen::Running { .. } | PublishSeen::RateLimited { .. }))) {
                return;
            }
            jobs.insert(job.dir.clone(), (job.server, stage(super::shares::Stage::Waiting)));
        }
        self.queue.lock().unwrap().push_back(job);
        let mut working = self.working.lock().unwrap();
        if !*working {
            *working = true;
            let me = self.clone();
            if std::thread::Builder::new().name("bridge-publish".into()).spawn(move || me.work()).is_err() {
                *working = false;
            }
        }
    }

    fn work(self: Arc<Self>) {
        loop {
            let job = {
                let mut queue = self.queue.lock().unwrap();
                match queue.pop_front() {
                    Some(job) => job,
                    None => {
                        *self.working.lock().unwrap() = false;
                        return;
                    }
                }
            };
            // Orbiters asked to wait: nothing goes until then.
            if let Some(wait) = self.wait_s(job.server) {
                self.queue.lock().unwrap().push_front(job);
                std::thread::sleep(Duration::from_secs(wait.min(5)));
                continue;
            }
            let (jobs, dir, server) = (&self.jobs, job.dir.clone(), job.server);
            let report = |s: super::shares::Stage| {
                jobs.lock().unwrap().insert(dir.clone(), (server, stage(s)));
            };
            let request = shares::Share { public: job.public, recipients: Vec::new(), acknowledge_explicit: false, message: String::new() };
            let explicit = job.public && job.entry.content.explicit;
            match (self.share)(job.whose.as_ref(), job.entry.clone(), request, &report) {
                Ok(shares::Shared::Item { item, .. }) => {
                    self.jobs.lock().unwrap().insert(dir, (server, done(server, &item)));
                }
                Ok(_) => {
                    self.jobs.lock().unwrap().insert(dir, (server, PublishSeen::Failed { message: "Orbiters answered with a share request, not the item.".into(), explicit: false })); // i18n: not shown (an AI client reads it)
                }
                Err(e) => match e.downcast_ref::<shares::RateLimited>() {
                    Some(limited) => {
                        let mut hold = self.hold.lock().unwrap();
                        hold.retain(|(s, _)| *s != server);
                        hold.push((server, Instant::now() + Duration::from_secs(limited.retry_after_s)));
                        drop(hold);
                        self.jobs.lock().unwrap().insert(dir, (server, PublishSeen::RateLimited { retry_after_s: limited.retry_after_s, ahead: 0 }));
                        self.queue.lock().unwrap().push_front(job);
                    }
                    None => {
                        self.jobs.lock().unwrap().insert(dir, (server, PublishSeen::Failed { message: format!("{e:#}"), explicit }));
                    }
                },
            }
            (self.after)();
        }
    }

    /// Where a picture publishing from here stands, a wait said with the seconds left now and its place in the queue.
    fn seen(&self, dir: &Path) -> Option<(Server, PublishSeen)> {
        let (server, seen) = self.jobs.lock().unwrap().get(dir).cloned()?;
        let queued = self.queue.lock().unwrap().iter().filter(|j| j.server == server).position(|j| j.dir == dir);
        match (self.wait_s(server), queued, seen) {
            (Some(wait), Some(ahead), PublishSeen::Running { .. } | PublishSeen::RateLimited { .. }) => Some((server, PublishSeen::RateLimited { retry_after_s: wait, ahead })),
            (None, Some(_), PublishSeen::RateLimited { .. }) => Some((server, stage(super::shares::Stage::Waiting))),
            (_, _, seen) => Some((server, seen)),
        }
    }
}

pub(super) struct RealPublish {
    pub(super) ctl: Weak<Controller>,
    pacer: Arc<Pacer>,
}

impl RealPublish {
    pub(super) fn new(ctl: Weak<Controller>) -> Self {
        let weak = ctl.clone();
        let after: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(ctl) = weak.upgrade() {
                ctl.bump_library();
                ctl.refresh_maps();
            }
        });
        Self { ctl, pacer: Pacer::new(Arc::new(|whose, entry, request, report| super::shares::share_as(whose.ok_or_else(super::shares::signed_out)?, entry, request, report)), after) }
    }

    fn ctl(&self) -> Result<Shared, Failure> {
        self.ctl.upgrade().ok_or_else(|| Failure::new("closing", "Orbiters - N4N0 is closing.", "Try again once it runs."))
    }

    fn server(&self) -> Server {
        match self.ctl.upgrade() {
            Some(ctl) if ctl.settings().development => Server::Development,
            Some(_) => Server::Production,
            None => Server::current(),
        }
    }
}

/// A published item's page: a public capture's own page, else the account's gallery (only its owner sees a private one).
fn link(server: Server, item: &shares::Item) -> String {
    if item.public {
        format!("{}maps/{}/captures/{}", server.website(), item.world.id, item.id)
    } else {
        format!("{}gallery", server.website())
    }
}

fn done(server: Server, item: &shares::Item) -> PublishSeen {
    PublishSeen::Done { id: item.id, public: item.public, link: link(server, item), world_id: item.world.id.clone() }
}

fn stage(stage: super::shares::Stage) -> PublishSeen {
    use super::shares::Stage;
    let (name, sent, total) = match stage {
        Stage::Starting => ("starting", 0, 0),
        Stage::Waiting => ("waiting", 0, 0),
        Stage::Preparing => ("preparing", 0, 0),
        Stage::Sending { sent, total } => ("sending", sent, total),
        Stage::Replacing => ("replacing", 0, 0),
        Stage::Sharing => ("sharing", 0, 0),
        Stage::Items { .. } => ("sharing", 0, 0),
    };
    PublishSeen::Running { stage: name.into(), sent, total }
}

/// The drive a path is on (`C:`), or the path itself where there are no drive letters.
fn drive(path: &Path) -> String {
    #[cfg(unix)]
    if let Some(volume) = volume_of(path) {
        return volume.to_string_lossy().into_owned();
    }
    let text = path.to_string_lossy();
    match text.get(..2) {
        Some(d) if d.ends_with(':') => d.to_ascii_uppercase(),
        _ => text.into_owned(),
    }
}

/// The mount point of the volume `path` is on (its nearest existing folder's, walking up while the device stays the same):
/// two folders of one disk are one disk, as `C:` is on Windows.
#[cfg(unix)]
pub(crate) fn volume_of(path: &Path) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let mut at = path.ancestors().find(|p| p.exists())?.canonicalize().ok()?;
    let device = std::fs::metadata(&at).ok()?.dev();
    while let Some(parent) = at.parent() {
        if std::fs::metadata(parent).ok()?.dev() != device {
            break;
        }
        at = parent.to_path_buf();
    }
    Some(at)
}

impl crate::bridge::game::Publish for RealPublish {
    fn environment(&self) -> EnvironmentSeen {
        let server = self.server();
        let other = if server == Server::Development { Server::Production } else { Server::Development };
        let ctl = self.ctl.upgrade();
        // The account the app read for this environment (its account thread reads the other one within half a second of a switch).
        let account = ctl.as_ref().and_then(|c| c.feedback_account()).filter(|(s, _, _)| *s == server).map(|(_, name, id)| (name, id));
        EnvironmentSeen {
            development: server == Server::Development,
            server: server.api().to_string(),
            website: server.website().to_string(),
            account,
            other_signed_in: account::modified(other).is_some(),
            labels_pending: ctl.map_or(0, |c| c.feedback_pending()),
        }
    }

    fn set_environment(&self, development: bool) -> Result<(), Failure> {
        let ctl = self.ctl()?;
        ctl.update_settings(|s| s.development = development);
        Ok(())
    }

    fn sessions_dir(&self) -> PathBuf {
        self.ctl.upgrade().map(|c| c.with_settings(|s| s.sessions_dir.clone())).unwrap_or_default()
    }

    fn publish(&self, session: &Path, world_id: &str, world_name: &str, public: bool) -> Result<(), Failure> {
        let ctl = self.ctl()?;
        let server = self.server();
        let Some(mut entry) = super::shares::session_entry(session, server) else {
            return Err(Failure::new("not_a_picture", "Its folder is not a session this app can read.", "Leave it."));
        };
        if entry.world_id.is_empty() {
            entry.world_id = world_id.to_string();
            entry.world_name = world_name.to_string();
        }
        drop(ctl);
        self.pacer.add(Job { dir: session.to_path_buf(), server, entry, public, whose: super::shares::Whose::saved(server).ok() });
        Ok(())
    }

    fn published(&self, session: &Path) -> PublishSeen {
        let server = self.server();
        let mine = self.pacer.seen(session);
        if let Some((s, seen)) = &mine {
            if *s == server && !matches!(seen, PublishSeen::Done { .. }) {
                return seen.clone();
            }
        }
        // The upload records of this computer: what was published before, by the tour or by the person.
        match super::shares::uploads(server).into_iter().find(|(path, item)| path == session && item.kind == Kind::Capture) {
            Some((_, item)) => done(server, &item),
            None => match mine {
                Some((s, seen)) if s == server => seen,
                _ => PublishSeen::None,
            },
        }
    }

    fn delete(&self, session: &Path, everything: bool) -> Result<u64, Failure> {
        let ctl = self.ctl()?;
        crate::upload_prep::forget(session);
        let result = if everything {
            // Its gallery card on Orbiters shows the picture Orbiters keeps of it: never deleted before that is there.
            let problems = super::shares::keep_pictures_on_orbiters(session);
            if !problems.is_empty() {
                return Err(Failure::new(
                    "picture_not_on_orbiters",
                    format!("Its upload on Orbiters has no picture and none could be given from its folder: {}", problems.join("; ")),
                    "Keep its files (capture_files frees most of them) and tell the person.",
                ));
            }
            orbiters_core::cleanup::delete_session(session, &mut |_| {})
        } else {
            orbiters_core::cleanup::drop_recording(session, &mut |_| {})
        };
        ctl.bump_library();
        result.map_err(|e| Failure::new("not_freed", format!("{e:#}"), "Leave it and tell the person if it stays."))
    }

    fn size(&self, session: &Path) -> u64 {
        orbiters_core::bundle::folder_size(session)
    }

    fn disks(&self) -> Vec<DiskSeen> {
        let mut disks: Vec<DiskSeen> = Vec::new();
        let places = [(self.sessions_dir(), "pictures"), (orbiters_core::paths::config_dir(), "app")];
        for (path, holds) in places {
            if path.as_os_str().is_empty() {
                continue;
            }
            let name = drive(&path);
            if let Some(d) = disks.iter_mut().find(|d| d.drive == name) {
                d.holds.push(holds);
                continue;
            }
            let free = orbiters_projects::size::free_space(&path).unwrap_or(0);
            disks.push(DiskSeen { drive: name, free_gb: (free as f64 / 1e8).round() / 10.0, holds: vec![holds] });
        }
        disks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::shares::{Entry, Stage};

    fn item(id: u64, public: bool) -> shares::Item {
        shares::Item { kind: Kind::Capture, id, public, mine: true, ..Default::default() }
    }

    fn entry() -> Entry {
        crate::controller::shares::remote_entry(shares::Item::default())
    }

    /// The tour's case: Orbiters takes a few, then answers 429 with Retry-After. The queue goes one at a time, sends nothing
    /// while asked to wait (each refused request counts in Orbiters' window), says the wait with its seconds and each
    /// picture's place, then goes on by itself with the same picture.
    #[test]
    fn publishing_paces_itself_on_orbiters_retry_after_and_goes_on_by_itself() {
        let sent: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::default();
        let allowed = Arc::new(Mutex::new(2usize));
        let at_once = Arc::new(Mutex::new((0usize, 0usize)));
        let share: ShareFn = {
            let (sent, allowed, at_once) = (sent.clone(), allowed.clone(), at_once.clone());
            Arc::new(move |_whose: Option<&crate::controller::shares::Whose>, entry: Entry, request: shares::Share, report: crate::controller::shares::Report| {
                let dir = match entry.local.as_ref() {
                    Some(crate::controller::shares::Local::Session(d)) => d.clone(),
                    _ => PathBuf::new(),
                };
                {
                    let mut n = at_once.lock().unwrap();
                    n.0 += 1;
                    n.1 = n.1.max(n.0);
                }
                report(Stage::Sending { sent: 1, total: 2 });
                std::thread::sleep(Duration::from_millis(30));
                at_once.lock().unwrap().0 -= 1;
                let mut left = allowed.lock().unwrap();
                sent.lock().unwrap().push((dir.clone(), Instant::now()));
                if *left == 0 {
                    return Err(shares::RateLimited { retry_after_s: 2, message: "Too many attempts. Please wait before trying again.".into() }.into());
                }
                *left -= 1;
                let id = sent.lock().unwrap().len() as u64;
                Ok(shares::Shared::Item { explorer: None, item: item(id, request.public) })
            })
        };
        let pacer = Pacer::new(share, Arc::new(|| {}));
        let dirs: Vec<PathBuf> = (0..4).map(|n| PathBuf::from(format!("picture-{n}"))).collect();
        for dir in &dirs {
            let mut e = entry();
            e.local = Some(crate::controller::shares::Local::Session(dir.clone()));
            pacer.add(Job { dir: dir.clone(), server: Server::Production, entry: e, public: true, whose: None });
            // Asked twice: queued once.
            let mut e = entry();
            e.local = Some(crate::controller::shares::Local::Session(dir.clone()));
            pacer.add(Job { dir: dir.clone(), server: Server::Production, entry: e, public: true, whose: None });
        }
        // Two go, the third is refused: the queue holds, says so, and sends nothing for 2 s.
        let until = Instant::now() + Duration::from_secs(5);
        while !matches!(pacer.seen(&dirs[2]), Some((_, PublishSeen::RateLimited { .. }))) {
            assert!(Instant::now() < until, "never held");
            std::thread::sleep(Duration::from_millis(10));
        }
        let refused_at = Instant::now();
        assert!(matches!(pacer.seen(&dirs[0]), Some((_, PublishSeen::Done { id: 1, .. }))));
        assert!(matches!(pacer.seen(&dirs[2]), Some((_, PublishSeen::RateLimited { retry_after_s: 1..=2, ahead: 0 }))), "{:?}", pacer.seen(&dirs[2]));
        assert!(matches!(pacer.seen(&dirs[3]), Some((_, PublishSeen::RateLimited { ahead: 1, .. }))), "{:?}", pacer.seen(&dirs[3]));
        assert_eq!(sent.lock().unwrap().len(), 3);
        *allowed.lock().unwrap() = 10;
        let until = Instant::now() + Duration::from_secs(10);
        while !matches!(pacer.seen(&dirs[3]), Some((_, PublishSeen::Done { .. }))) {
            assert!(Instant::now() < until, "never went on");
            std::thread::sleep(Duration::from_millis(20));
        }
        let sent = sent.lock().unwrap().clone();
        assert_eq!(sent.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>(), [&dirs[0], &dirs[1], &dirs[2], &dirs[2], &dirs[3]].map(|d| d.clone()), "in order, each once, the refused one again");
        assert!(sent[3].1.duration_since(refused_at) >= Duration::from_millis(1500), "nothing sent before Retry-After passed");
        assert_eq!(at_once.lock().unwrap().1, 1, "one at a time, never a burst");
    }
}
