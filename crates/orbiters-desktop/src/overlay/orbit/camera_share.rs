//! The share buttons' side that lives with the panels (REQUEST13): the first time's question for "Share publicly", and in
//! a headset the friends' chooser for "Share to …". Its friends are the desktop share dialog's own (the same request of
//! the service, `vrchat_data::Request::Friends`, for the same account), the friends in this instance first, then by
//! name. A friend is chosen by who they are ([`share_sheet::person_key`]), never by a row's place, and what is sent
//! names them by their VRChat id.

use super::panels::{share_sheet, ChooserRow, ChooserView, ShareView};
use crate::backend::Ctl;
use crate::controller::explorer::Command;
use crate::controller::vrchat_data::{Request, Response, Scope};
use crate::state::Snapshot;
use std::collections::BTreeSet;
use std::sync::mpsc;

/// At most this many friends at once, as the desktop's dialog.
pub const CHOSEN_MAX: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct Friend {
    pub id: String,
    pub name: String,
}

pub struct Chooser {
    /// The picture it is for.
    pub offer: u64,
    friends: Vec<Friend>,
    coming: Option<mpsc::Receiver<Result<Vec<Friend>, String>>>,
    error: Option<String>,
    chosen: BTreeSet<String>,
    page: usize,
}

impl Chooser {
    /// Opened for picture `offer`: its friends asked of the service on a thread of their own.
    pub fn open(offer: u64, backend: &Ctl, snap: &Snapshot) -> Self {
        // Those Home's list has show at once; VRChat's answer takes their place.
        let friends = crate::controller::vrchat_data::friends_here(snap).into_iter().map(|f| Friend { id: f.id.clone(), name: f.name.clone() }).collect();
        let mut c = Self { offer, friends, coming: None, error: None, chosen: BTreeSet::new(), page: 0 };
        c.load(backend, snap);
        c
    }

    /// Friends already known (tests, screenshots).
    pub fn with(offer: u64, friends: Vec<Friend>) -> Self {
        Self { offer, friends, coming: None, error: None, chosen: BTreeSet::new(), page: 0 }
    }

    pub fn load(&mut self, backend: &Ctl, snap: &Snapshot) {
        let (tx, rx) = mpsc::channel();
        let backend = backend.clone();
        let scope = Scope::of(snap);
        std::thread::spawn(move || {
            let answer = match backend.vrchat_data(scope, Request::Friends) {
                Ok(Response::Friends(users)) => Ok(users.into_iter().map(|u| Friend { id: u.id, name: u.display_name }).collect()),
                Ok(_) => Err(orbiters_i18n::tr("Friends could not be read").to_string()),
                Err(e) => Err(e),
            };
            let _ = tx.send(answer);
        });
        self.coming = Some(rx);
        self.error = None;
    }

    /// The friends arrived (or why not): true when something changed.
    pub fn follow(&mut self) -> bool {
        let Some(rx) = self.coming.as_ref() else { return false };
        match rx.try_recv() {
            Ok(Ok(friends)) => {
                self.friends = friends;
                self.coming = None;
                true
            }
            Ok(Err(e)) => {
                self.error = Some(e);
                self.coming = None;
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.coming = None;
                self.error = Some(orbiters_i18n::tr("Friends could not be read").to_string());
                true
            }
        }
    }

    /// The friends in the order shown: those in this instance first, then by name.
    fn ordered(&self, snap: &Snapshot) -> Vec<(&Friend, bool)> {
        let here = |f: &Friend| snap.vrchat.world.players.contains_key(&f.id);
        let mut all: Vec<(&Friend, bool)> = self.friends.iter().map(|f| (f, here(f))).collect();
        all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase())));
        all
    }

    pub fn pages(&self) -> usize {
        self.friends.len().div_ceil(share_sheet::ROWS).max(1)
    }

    /// The friend whose identity a press named, toggled (at most [`CHOSEN_MAX`]); none when nobody has it any more.
    pub fn toggle(&mut self, key: u64) -> bool {
        let Some(f) = self.friends.iter().find(|f| share_sheet::person_key(&f.id) == key) else { return false };
        if !self.chosen.remove(&f.id) && self.chosen.len() < CHOSEN_MAX {
            self.chosen.insert(f.id.clone());
        }
        true
    }

    pub fn step(&mut self, next: bool) {
        self.page = if next { (self.page + 1).min(self.pages() - 1) } else { self.page.saturating_sub(1) };
    }

    /// What is sent: the friends chosen, by id and name.
    pub fn chosen(&self) -> Vec<(String, String)> {
        self.friends.iter().filter(|f| self.chosen.contains(&f.id)).map(|f| (f.id.clone(), f.name.clone())).collect()
    }

    pub fn view(&self, snap: &Snapshot) -> ChooserView {
        let ordered = self.ordered(snap);
        let page = self.page.min(self.pages() - 1);
        let rows = ordered
            .iter()
            .skip(page * share_sheet::ROWS)
            .take(share_sheet::ROWS)
            .map(|(f, here)| ChooserRow { key: share_sheet::person_key(&f.id), name: f.name.clone(), here: *here, chosen: self.chosen.contains(&f.id) })
            .collect();
        ChooserView { rows, page, pages: self.pages(), chosen: self.chosen.len(), loading: self.coming.is_some() && self.friends.is_empty(), error: self.error.clone() }
    }
}

/// What the panels hold for the share buttons.
#[derive(Default)]
pub struct CameraShare {
    /// "Share publicly" pressed the first time: it asks, for this picture.
    pub confirming: Option<u64>,
    pub chooser: Option<Chooser>,
}

impl CameraShare {
    /// The share row and chooser as they show now: none without a picture to share.
    pub fn view(&self, snap: &Snapshot) -> Option<ShareView> {
        let offer = snap.explorer.offer.clone()?;
        let chooser = self.chooser.as_ref().filter(|c| c.offer == offer.id).map(|c| c.view(snap));
        Some(ShareView { confirming: self.confirming == Some(offer.id), asks_first: !snap.settings.explorer.public_confirmed, chooser, offer })
    }

    /// Follows the service: another picture (or none) closes the question and the chooser; friends arriving. True when
    /// something shown changed.
    pub fn follow(&mut self, snap: &Snapshot) -> bool {
        let id = snap.explorer.offer.as_ref().map(|o| o.id);
        let mut changed = false;
        if self.confirming.is_some() && self.confirming != id {
            self.confirming = None;
            changed = true;
        }
        if self.chooser.as_ref().is_some_and(|c| Some(c.offer) != id) {
            self.chooser = None;
            changed = true;
        }
        if let Some(c) = self.chooser.as_mut() {
            changed |= c.follow();
        }
        changed
    }

    /// "Share publicly": asks the first time, else asks the service.
    pub fn public(&mut self, offer: u64, snap: &Snapshot, backend: &Ctl) {
        if snap.settings.explorer.public_confirmed {
            backend.explorer(Command::SharePublic { offer, confirmed: false });
        } else {
            self.confirming = Some(offer);
        }
    }

    pub fn confirm(&mut self, offer: u64, backend: &Ctl) {
        if self.confirming.take() == Some(offer) {
            backend.explorer(Command::SharePublic { offer, confirmed: true });
        }
    }

    /// Send on the chooser: the chosen friends to the service for picture `offer`; the chooser goes.
    pub fn send(&mut self, offer: u64, backend: &Ctl) {
        let Some(c) = self.chooser.take_if(|c| c.offer == offer) else { return };
        let recipients = c.chosen();
        if !recipients.is_empty() {
            backend.explorer(Command::ShareTo { offer, recipients });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friends(names: &[(&str, &str)]) -> Vec<Friend> {
        names.iter().map(|(id, name)| Friend { id: (*id).into(), name: (*name).into() }).collect()
    }

    /// Friends here first, then by name; a press toggles who it names even after the list moved under it.
    #[test]
    fn friends_here_come_first_and_a_press_chooses_who_it_names() {
        let mut snap = Snapshot::new(Default::default());
        snap.vrchat.world.players.insert("usr_c".into(), "Cleo".into());
        let mut c = Chooser::with(5, friends(&[("usr_a", "Ada"), ("usr_b", "bram"), ("usr_c", "Cleo")]));
        let v = c.view(&snap);
        assert_eq!(v.rows.iter().map(|r| (r.name.as_str(), r.here)).collect::<Vec<_>>(), [("Cleo", true), ("Ada", false), ("bram", false)]);
        // Bram's row is pressed; meanwhile someone arrives and the rows move: Bram is chosen all the same.
        let bram = v.rows[2].key;
        snap.vrchat.world.players.insert("usr_a".into(), "Ada".into());
        assert!(c.toggle(bram));
        assert_eq!(c.chosen(), [("usr_b".to_string(), "bram".to_string())]);
        // A friend gone from the list: the press does nothing.
        assert!(!c.toggle(share_sheet::person_key("usr_gone")));
        assert!(c.toggle(bram));
        assert!(c.chosen().is_empty());
    }

    #[test]
    fn pages_of_six_and_at_most_fifty() {
        let many: Vec<Friend> = (0..60).map(|i| Friend { id: format!("usr_{i:02}"), name: format!("F{i:02}") }).collect();
        let mut c = Chooser::with(1, many.clone());
        let snap = Snapshot::new(Default::default());
        assert_eq!(c.pages(), 10);
        c.step(true);
        assert_eq!(c.view(&snap).rows[0].name, "F06");
        c.step(false);
        c.step(false);
        assert_eq!(c.view(&snap).page, 0);
        for f in &many {
            c.toggle(share_sheet::person_key(&f.id));
        }
        assert_eq!(c.chosen().len(), CHOSEN_MAX);
    }

    /// Another picture, or none: the question and the chooser of the last one go.
    #[test]
    fn a_new_picture_closes_the_question_and_the_chooser() {
        let mut snap = Snapshot::new(Default::default());
        let offer = |id| crate::controller::explorer::Offer {
            id,
            key: String::new(),
            world_id: String::new(),
            would_earn: None,
            until_ms: u64::MAX,
            public: Default::default(),
            people: Default::default(),
        };
        snap.explorer.offer = Some(offer(1));
        let mut s = CameraShare { confirming: Some(1), chooser: Some(Chooser::with(1, Vec::new())) };
        assert!(!s.follow(&snap));
        assert!(s.view(&snap).is_some_and(|v| v.confirming && v.chooser.is_some()));
        snap.explorer.offer = Some(offer(2));
        assert!(s.follow(&snap));
        assert!(s.view(&snap).is_some_and(|v| !v.confirming && v.chooser.is_none()));
        snap.explorer.offer = None;
        assert!(s.view(&snap).is_none());
    }
}
