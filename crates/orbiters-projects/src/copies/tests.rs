use super::*;
use crate::unity6::trial::tests::{env, Env};

fn held(e: &Env, name: &str) -> Held {
    Held::begin(&e.app, &e.source, &e.root.join("Projects").join(name)).unwrap()
}

fn one(e: &Env) -> (Copy, Status) {
    let mut all = shown(&e.app);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

/// From before anything is made to its place on the list: shown where it stands, with why, and gone when it is listed.
#[test]
fn a_copy_is_recorded_until_it_is_listed() {
    let e = env();
    let h = held(&e, "Copy");
    assert_eq!(one(&e).1, Status::Running);
    assert!(list(&e.app, &e.vcc, &h.copy.id).is_err() && dismiss(&e.app, &h.copy.id).is_err(), "somebody works on it");
    // Nothing made: nothing to show.
    drop(h);
    assert!(shown(&e.app).is_empty());
    // Cut short (this run ended): never a project, whatever is in it.
    let at = e.root.join("Projects/Copy");
    crate::unity6::trial::tests::copy_tree(&e.source, &at);
    let mut h = held(&e, "Copy");
    h.made(owned::id_of(&at).unwrap()).unwrap();
    drop(h);
    let (c, s) = one(&e);
    assert_eq!(s, Status::Incomplete);
    assert!(list(&e.app, &e.vcc, &c.id).is_err() && folders(&e.app).is_empty());
    assert_eq!(dismiss(&e.app, &c.id).unwrap(), at);
    assert!(shown(&e.app).is_empty() && at.join("Assets/Avatar.unity").is_file(), "its folder stays");
    // Whole, and the list could not be written: in the gallery, with why; listed later.
    let mut h = held(&e, "Copy");
    h.made(owned::id_of(&at).unwrap()).unwrap();
    h.whole().unwrap();
    h.failed(&anyhow::anyhow!("no list").context("the copy is there"));
    let (c, s) = one(&e);
    assert_eq!((s, c.error.as_deref()), (Status::Unlisted, Some("the copy is there: no list")));
    assert_eq!(folders(&e.app), [at.clone()]);
    std::fs::write(&e.vcc, "x").unwrap();
    assert!(list(&e.app, &e.vcc, &c.id).is_err());
    assert_eq!(one(&e).1, Status::Unlisted, "still there, with the new reason");
    assert_ne!(one(&e).0.error, c.error);
    std::fs::remove_file(&e.vcc).unwrap();
    assert_eq!(list(&e.app, &e.vcc, &c.id).unwrap(), at);
    assert!(shown(&e.app).is_empty() && crate::unity6::trial::tests::listed(&e, &at));
    // Not there, another folder at its place, or a marker still in it.
    let mut h = held(&e, "Copy");
    h.made(owned::id_of(&at).unwrap()).unwrap();
    h.whole().unwrap();
    drop(h);
    let away = e.root.join("away");
    std::fs::rename(&at, &away).unwrap();
    assert_eq!(one(&e).1, Status::Missing);
    std::fs::create_dir_all(&at).unwrap();
    assert_eq!(one(&e).1, Status::Missing);
    std::fs::remove_dir(&at).unwrap();
    std::fs::rename(&away, &at).unwrap();
    assert_eq!(one(&e).1, Status::Unlisted);
    // Put on the list another way: no record of it is needed any more.
    listed(&e.app, &at);
    assert!(shown(&e.app).is_empty());
}

/// Each copy has its own record: one that cannot be read hides only itself, and is not deleted.
#[test]
fn a_record_that_cannot_be_read_hides_only_itself() {
    let e = env();
    let (a, b) = (e.root.join("Projects/A"), e.root.join("Projects/B"));
    for at in [&a, &b] {
        std::fs::create_dir_all(at).unwrap();
        let mut h = Held::begin(&e.app, &e.source, at).unwrap();
        h.made(owned::id_of(at).unwrap()).unwrap();
        drop(h);
    }
    let first = shown(&e.app)[0].0.clone();
    let at = e.app.join(file(&first.id));
    std::fs::write(&at, "{").unwrap();
    assert_eq!(shown(&e.app).len(), 1);
    assert!(at.is_file() && dismiss(&e.app, &first.id).is_err() && dismiss(&e.app, "../x").is_err());
}

/// Another window, or the gallery's scan, has a copy's lock for an instant when it only asks: that is not a copy being made.
#[test]
fn a_copy_being_looked_at_is_not_one_being_made() {
    let e = env();
    let at = e.root.join("Projects/Copy");
    crate::unity6::trial::tests::copy_tree(&e.source, &at);
    let mut h = held(&e, "Copy");
    h.made(owned::id_of(&at).unwrap()).unwrap();
    h.whole().unwrap();
    let id = h.copy.id.clone();
    drop(h);
    let other = AppRoot::existing(&e.app).unwrap().try_lock(&lock(&id)).unwrap().unwrap();
    let asked = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(15));
        drop(other);
    });
    assert_eq!(one(&e).1, Status::Unlisted);
    asked.join().unwrap();
    assert!(dismiss(&e.app, &id).is_ok());
}
