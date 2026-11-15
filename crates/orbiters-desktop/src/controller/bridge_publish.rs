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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};

pub(super) struct RealPublish {
    pub(super) ctl: Weak<Controller>,
    /// Publishing started here, by session folder: the environment it went to, and where it stands.
    jobs: std::sync::Arc<Mutex<HashMap<PathBuf, (Server, PublishSeen)>>>,
}

impl RealPublish {
    pub(super) fn new(ctl: Weak<Controller>) -> Self {
        Self { ctl, jobs: Default::default() }
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
    let text = path.to_string_lossy();
    match text.get(..2) {
        Some(d) if d.ends_with(':') => d.to_ascii_uppercase(),
        _ => text.into_owned(),
    }
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
        {
            let mut jobs = self.jobs.lock().unwrap();
            if matches!(jobs.get(session), Some((_, PublishSeen::Running { .. }))) {
                return Ok(());
            }
            jobs.insert(session.to_path_buf(), (server, stage(super::shares::Stage::Starting)));
        }
        let jobs = self.jobs.clone();
        let dir = session.to_path_buf();
        let explicit = public && entry.content.explicit;
        std::thread::Builder::new()
            .name("bridge-publish".into())
            .spawn(move || {
                let report = |s: super::shares::Stage| {
                    jobs.lock().unwrap().insert(dir.clone(), (server, stage(s)));
                };
                let request = shares::Share { public, recipients: Vec::new(), acknowledge_explicit: false, message: String::new() };
                let seen = match super::shares::share(server, entry, request, &report) {
                    Ok(shares::Shared::Item { item }) => done(server, &item),
                    Ok(_) => PublishSeen::Failed { message: "Orbiters answered with a share request, not the item.".into(), explicit: false }, // i18n: not shown (an AI client reads it)
                    Err(e) => PublishSeen::Failed { message: format!("{e:#}"), explicit },
                };
                jobs.lock().unwrap().insert(dir, (server, seen));
                ctl.bump_library();
                ctl.refresh_maps();
            })
            .map_err(|e| Failure::new("publish_failed", format!("{e}"), "Try again."))?;
        Ok(())
    }

    fn published(&self, session: &Path) -> PublishSeen {
        let server = self.server();
        if let Some((s, seen)) = self.jobs.lock().unwrap().get(session) {
            if *s == server && !matches!(seen, PublishSeen::Done { .. }) {
                return seen.clone();
            }
        }
        // The upload records of this computer: what was published before, by the tour or by the person.
        match super::shares::uploads(server).into_iter().find(|(path, item)| path == session && item.kind == Kind::Capture) {
            Some((_, item)) => done(server, &item),
            None => match self.jobs.lock().unwrap().get(session) {
                Some((s, seen)) if *s == server => seen.clone(),
                _ => PublishSeen::None,
            },
        }
    }

    fn delete(&self, session: &Path, everything: bool) -> Result<u64, Failure> {
        let ctl = self.ctl()?;
        crate::upload_prep::forget(session);
        let result = if everything {
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
