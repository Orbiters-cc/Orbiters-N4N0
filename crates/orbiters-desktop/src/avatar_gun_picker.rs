//! The avatar gun's picker (contracts `Docs/avatar-gun.md`, What the shooter's Orbiters - N4N0 does): the public avatars
//! of the signed-in VRChat account's avatar favourite groups, group by group in VRChat's order, as the window's page and
//! the menus' sheet show them. Asked through the service (`vrchat_data::Request::AvatarFavoriteGroups`, then
//! `AvatarFavorites` for each group in turn, on a thread): each group shows as it arrives, the others keep their place
//! as loading. VRChat's cookies never leave the service.

use crate::backend::Ctl;
use crate::controller::vrchat_data::{ListError, Request, Response, Scope};
use crate::state::Snapshot;
use orbiters_core::settings::LoadedAvatar;
use orbiters_i18n::{mark, tr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

/// An avatar the gun may be loaded with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub id: String,
    pub name: String,
    pub thumbnail: String,
}

impl Pick {
    pub fn loaded(&self) -> LoadedAvatar {
        LoadedAvatar { id: self.id.clone(), name: self.name.clone(), thumbnail: self.thumbnail.clone() }
    }
}

/// A favourite group: its tag, its name, its public avatars once read (or why not).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub tag: String,
    pub name: String,
    pub avatars: Option<Vec<Pick>>,
    pub error: Option<ListError>,
}

/// Where the picker stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// No VRChat sign-in to read the favourites with; `gated`: the server does not offer the sign-in to this Orbiters
    /// account now (its `n4n0-vrchat-login`, open to every member; asked again as always).
    SignedOut { gated: bool },
    /// The groups are being read.
    #[default]
    Loading,
    /// The groups are known (each its avatars or not yet).
    Ready,
    /// The groups could not be read.
    Failed(ListError),
}

/// The words of the picker's states (both pages say them).
pub const SIGN_IN: &str = mark("Sign in to VRChat in Orbiters - N4N0 to list the avatars of your favorites.");
pub const GATED: &str = mark("VRChat sign-in isn't available for your account right now. The picker needs it to read your favorites.");
pub const EMPTY: &str = mark("No public avatar in your favorites yet. Favorite public avatars in VRChat and they show up here.");
pub const EMPTY_GROUP: &str = mark("No public avatar in this list");
pub const RETRY: &str = mark("Try again");

/// What a failure says.
pub fn failure(e: &ListError) -> String {
    match e {
        ListError::SignInRequired => tr(SIGN_IN).to_string(),
        ListError::RateLimited { seconds } => orbiters_i18n::fill(tr(mark("VRChat asks to slow down. Try again in {seconds} s.")), &[("seconds", &seconds)]),
        ListError::Failed(why) => orbiters_i18n::fill(tr(mark("Your favorites could not be read: {why}")), &[("why", why)]),
    }
}

enum Came {
    Groups(Result<Vec<(String, String)>, ListError>),
    Group(String, Result<Vec<orbiters_core::vrchat_api::Avatar>, ListError>),
}

/// The picker's lists, read through the service for one VRChat account.
#[derive(Default)]
pub struct Store {
    scope: Option<Scope>,
    stage: Stage,
    groups: Vec<Group>,
    reading: Option<(mpsc::Receiver<Came>, Arc<AtomicBool>)>,
    /// Made-up lists (tests, screenshots): nothing is asked.
    stand_in: bool,
}

impl Drop for Store {
    fn drop(&mut self) {
        if let Some((_, stop)) = &self.reading {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

impl Store {
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Made-up lists, for tests and the screenshot review.
    pub fn stand_in(&mut self, stage: Stage, groups: Vec<Group>) {
        self.stand_in = true;
        self.reading = None;
        (self.stage, self.groups) = (stage, groups);
    }

    /// The avatar `id`, wherever it is listed.
    pub fn find(&self, id: &str) -> Option<&Pick> {
        self.groups.iter().filter_map(|g| g.avatars.as_ref()).flatten().find(|p| p.id == id)
    }

    /// Reads them again (Try again).
    pub fn again(&mut self) {
        if !self.stand_in {
            self.scope = None;
        }
    }

    /// While a page shows them: asks for the lists of the VRChat account signed in now (again when another is), and takes
    /// what came. Whether what shows changed.
    pub fn follow(&mut self, backend: Option<&Ctl>, snap: &Snapshot) -> bool {
        if self.stand_in {
            return false;
        }
        let scope = Scope::of(snap);
        if !scope.signed_in() {
            let stage = Stage::SignedOut { gated: !snap.vrchat_login.allowed };
            let changed = self.stage != stage || !self.groups.is_empty();
            self.scope = None;
            self.reading = None;
            self.groups.clear();
            self.stage = stage;
            return changed;
        }
        let mut changed = false;
        if self.scope.as_ref() != Some(&scope) {
            let Some(backend) = backend else { return false };
            if let Some((_, stop)) = self.reading.take() {
                stop.store(true, Ordering::Relaxed);
            }
            // Another account's, or asked again: nothing of the last read shows until this one says.
            self.scope = Some(scope.clone());
            self.stage = Stage::Loading;
            self.groups.clear();
            changed = true;
            let (tx, rx) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            self.reading = Some((rx, stop.clone()));
            let backend = backend.clone();
            std::thread::Builder::new()
                .name("avatar-gun-picker".into())
                .spawn(move || read(&backend, scope, &tx, &stop))
                .ok();
        }
        let mut came = Vec::new();
        let mut ended = false;
        if let Some((rx, _)) = &self.reading {
            loop {
                match rx.try_recv() {
                    Ok(c) => came.push(c),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        ended = true;
                        break;
                    }
                }
            }
        }
        if ended {
            self.reading = None;
        }
        for c in came {
            self.take(c);
            changed = true;
        }
        changed
    }

    fn take(&mut self, came: Came) {
        match came {
            Came::Groups(Ok(groups)) => {
                self.stage = Stage::Ready;
                self.groups = groups.into_iter().map(|(tag, name)| Group { tag, name, avatars: None, error: None }).collect();
            }
            Came::Groups(Err(e)) => self.stage = Stage::Failed(e),
            Came::Group(tag, result) => {
                if let Some(g) = self.groups.iter_mut().find(|g| g.tag == tag) {
                    match result {
                        Ok(avatars) => {
                            g.avatars = Some(avatars.into_iter().map(|a| Pick { thumbnail: if a.thumbnail_image_url.is_empty() { a.image_url } else { a.thumbnail_image_url }, id: a.id, name: a.name }).collect());
                            g.error = None;
                        }
                        Err(e) => g.error = Some(e),
                    }
                }
            }
        }
    }

    /// Whether every group is read (or failed) and the groups are known.
    pub fn settled(&self) -> bool {
        match self.stage {
            Stage::Ready => self.groups.iter().all(|g| g.avatars.is_some() || g.error.is_some()),
            Stage::Loading => false,
            _ => true,
        }
    }

    /// Whether some group could not be read (a Try again shows).
    pub fn failed(&self) -> Option<&ListError> {
        match &self.stage {
            Stage::Failed(e) => Some(e),
            _ => self.groups.iter().find_map(|g| g.error.as_ref()),
        }
    }
}

/// On the picker's thread: the groups, then each group's avatars in turn (the client's pacing spaces them), until the
/// page lets go.
fn read(backend: &Ctl, scope: Scope, tx: &mpsc::Sender<Came>, stop: &AtomicBool) {
    let as_list = |r: crate::controller::vrchat_data::Result| match r {
        Ok(Response::ListError(e)) => Err(e),
        Err(e) => Err(ListError::Failed(e)),
        Ok(other) => Ok(other),
    };
    let groups = match as_list(backend.vrchat_data(scope.clone(), Request::AvatarFavoriteGroups)) {
        Ok(Response::AvatarGroups(groups)) => Ok(groups),
        Ok(_) => Err(ListError::Failed(tr("The app's service answered something else.").to_string())),
        Err(e) => Err(e),
    };
    let tags: Vec<String> = groups.as_ref().map(|g| g.iter().map(|(t, _)| t.clone()).collect()).unwrap_or_default();
    if tx.send(Came::Groups(groups)).is_err() {
        return;
    }
    for tag in tags {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let avatars = match as_list(backend.vrchat_data(scope.clone(), Request::AvatarFavorites { group: tag.clone() })) {
            Ok(Response::AvatarList(a)) => Ok(a),
            Ok(_) => Err(ListError::Failed(tr("The app's service answered something else.").to_string())),
            Err(e) => Err(e),
        };
        // A rate limit holds every group: the rest wait for Try again.
        let limited = matches!(avatars, Err(ListError::RateLimited { .. }));
        if tx.send(Came::Group(tag, avatars)).is_err() || limited {
            return;
        }
    }
}

/// Made-up lists for tests and the screenshot review: three groups, the second still loading when `loading`.
pub fn sample(loading: bool) -> Vec<Group> {
    let pick = |n: usize, name: &str| Pick { id: format!("avtr_00000000-0000-4000-8000-{n:012}"), name: name.into(), thumbnail: String::new() };
    vec![
        Group {
            tag: "avatars1".into(),
            name: "Party".into(), // i18n: not shown (made-up lists)
            avatars: Some(["Neon Fox", "Paper Robot", "Mossy Golem", "Starlight Witch", "Tiny Dragon", "Glass Cat", "Retro Astronaut"].iter().enumerate().map(|(i, n)| pick(i, n)).collect()), // i18n: not shown (made-up avatars)
            error: None,
        },
        Group {
            tag: "avatars2".into(),
            name: "Silly ones".into(), // i18n: not shown (made-up lists)
            avatars: (!loading).then(|| ["Rubber Duck", "Walking Toaster", "A Very Long Avatar Name That Goes On And On"].iter().enumerate().map(|(i, n)| pick(10 + i, n)).collect()), // i18n: not shown (made-up avatars)
            error: None,
        },
        Group { tag: "avatars3".into(), name: "avatars3".into(), avatars: (!loading).then(Vec::new), error: None },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed out of VRChat (or with the sign-in not open to the account), nothing is asked and the page says which.
    #[test]
    fn signed_out_asks_nothing_and_says_why() {
        let mut snap = Snapshot::new(Default::default());
        let mut store = Store::default();
        assert!(store.follow(None, &snap));
        assert_eq!(store.stage(), &Stage::SignedOut { gated: true });
        snap.vrchat_login.allowed = true;
        assert!(store.follow(None, &snap));
        assert_eq!(store.stage(), &Stage::SignedOut { gated: false });
        assert!(!store.follow(None, &snap), "nothing changed");
    }

    /// Groups come first, then each group's avatars: a group read shows while the next is still loading; a failed group says
    /// so and offers Try again; the avatar loaded is found wherever it is listed.
    #[test]
    fn groups_arrive_one_by_one() {
        let mut store = Store::default();
        store.take(Came::Groups(Ok(vec![("avatars1".into(), "Party".into()), ("avatars2".into(), "More".into())])));
        assert_eq!(store.stage(), &Stage::Ready);
        assert!(store.groups().iter().all(|g| g.avatars.is_none()) && !store.settled());
        let avatar = orbiters_core::vrchat_api::Avatar { id: "avtr_1".into(), name: "Robot".into(), thumbnail_image_url: "https://api.vrchat.cloud/t".into(), ..Default::default() };
        store.take(Came::Group("avatars1".into(), Ok(vec![avatar])));
        assert_eq!(store.groups()[0].avatars.as_ref().map(Vec::len), Some(1));
        assert!(store.groups()[1].avatars.is_none() && !store.settled(), "the second still loading");
        store.take(Came::Group("avatars2".into(), Err(ListError::RateLimited { seconds: 5 })));
        assert!(store.settled() && matches!(store.failed(), Some(ListError::RateLimited { seconds: 5 })));
        assert_eq!(store.find("avtr_1").map(|p| p.thumbnail.as_str()), Some("https://api.vrchat.cloud/t"));
        let mut failed = Store::default();
        failed.take(Came::Groups(Err(ListError::Failed("offline".into()))));
        assert!(failed.settled() && failed.failed().is_some());
        for e in [ListError::SignInRequired, ListError::RateLimited { seconds: 3 }, ListError::Failed("x".into())] {
            assert!(!failure(&e).is_empty());
        }
    }
}
