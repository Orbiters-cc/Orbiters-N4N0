//! What was last read from VRChat, shown at the next start (REQUEST28), over made-up people and groups: the list and the
//! groups as they were before a close ("kept-before"), then, from the store written then and the pictures kept, in windows of
//! their own: the first frames of a start while the session is being restored ("kept-first-frame"), with no network at all
//! ("kept-offline"), signed in and not read yet ("kept-signed-in"), a group's page ("kept-group"), another Orbiters member
//! ("kept-other-member") and another VRChat account ("kept-other-account") than those it is of, and a store that cannot be
//! read ("kept-corrupted"). How many pictures are still to come after the first frame drawn is printed. Nothing is asked.
use super::{Page, Scenario};
use crate::app::App;
use crate::controller::groups::Page as GroupPage;
use crate::controller::vrchat_login::Status;
use crate::state::{AccountState, Snapshot};
use crate::worlds::profile_pictures::{Profiles, Size};
use orbiters_core::groups::GroupCard;
use orbiters_core::launcher::{LauncherState, Possible, Source, VerifiedFor, Whereabouts};
use orbiters_core::vrchat_api::{Members, Posts, User};
use orbiters_core::vrchat_store::{self as store, kind, Account, Change, Store, ONLY};

pub(super) const NAMES: [&str; 8] = ["kept-before", "kept-first-frame", "kept-offline", "kept-signed-in", "kept-group", "kept-other-member", "kept-other-account", "kept-corrupted"];
const MEMBER: &str = "00000000000000000000000000000e28";
const OTHER_MEMBER: &str = "00000000000000000000000000000b28";
const ME: &str = "usr_00000000-0000-4000-8000-00000000e001";
const OTHER: &str = "usr_00000000-0000-4000-8000-00000000e002";
/// When the list was last live (the page's clock is the computer's).
fn at() -> i64 {
    orbiters_core::visited::now() as i64 - 40 * 60
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in NAMES {
        scenarios.push(Scenario {
            name,
            page: Page::Home,
            snapshot: base.clone(),
            // Past the page's entrance: nothing is on its way by then, so what shows is what was there at once.
            frames: if name == "kept-before" { 100 } else { 60 },
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }
}

fn account(member: &str, user: &str) -> Account {
    Account::of(member, user).expect("a made-up account")
}

fn address(n: u64) -> String {
    format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-{n:012}/1/file")
}

/// The made-up people, every picture of theirs a VRChat address kept for the account (as a run before left it).
fn people(of: &Profiles) -> LauncherState {
    let mut s = super::launcher::data("launcher-list");
    s.source = Source::SignedIn { user_id: ME.into(), name: "Desktop Tester".into() };
    if let Some(me) = s.me.as_mut() {
        me.id = ME.into();
        me.name = "Desktop Tester".into();
    }
    s.profile = None;
    let mut known: Vec<String> = Vec::new();
    let mut keep = |sample: &mut String, user: Option<&str>| {
        if !sample.starts_with("sample://") {
            return;
        }
        let n = known.iter().position(|k| k == sample).unwrap_or_else(|| {
            known.push(sample.clone());
            known.len() - 1
        });
        let url = address(9000 + n as u64);
        let c = super::launcher::picture(sample);
        let rgba: Vec<u8> = c.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
        let image = image::RgbaImage::from_raw(c.size[0] as u32, c.size[1] as u32, rgba).expect("a made-up picture");
        let image = if image.width().max(image.height()) > 128 { image::DynamicImage::ImageRgba8(image).thumbnail(128, 128).to_rgba8() } else { image };
        of.keep(&url, user, &image);
        *sample = url;
    };
    for f in s.friends.iter_mut().chain(s.me.iter_mut()) {
        let id = f.id.clone();
        keep(&mut f.look.icon, Some(&id));
        for other in [&mut f.look.banner, &mut f.look.nameplate, &mut f.look.icon_frame] {
            keep(other, None);
        }
        if let Whereabouts::World(p) = &mut f.whereabouts {
            keep(&mut p.world_picture, None);
        }
    }
    for i in &mut s.invites {
        keep(&mut i.from_icon, None);
        if let Some(p) = i.place.as_mut() {
            keep(&mut p.world_picture, None);
        }
    }
    for b in &mut s.boops {
        keep(&mut b.from_icon, None);
    }
    if let Some(m) = s.mine.as_mut() {
        m.loaded = true;
        keep(&mut m.avatar_picture, None);
        keep(&mut m.icon, None);
        keep(&mut m.banner_custom, None);
    }
    s.possible = Possible { live_status: true, invites: true, request_invite: true, boop: true, play: true };
    s.notice = None;
    s
}

/// The groups, their pictures kept too.
fn groups(of: &Profiles) -> (Vec<GroupCard>, GroupPage, Members, Posts) {
    let mut cards = super::groups::cards();
    let keep = |url: &mut String, n: u64, image: image::RgbaImage| {
        *url = address(n);
        of.keep_sent(url, Size::Wide, &image, b"");
    };
    for (i, c) in cards.iter_mut().enumerate() {
        keep(&mut c.banner_url, 9500 + i as u64, super::groups::landscape(480, 270, i as u32));
        keep(&mut c.icon_url, 9600 + i as u64, super::groups::icon(i as u32));
    }
    let mut page = super::groups::page(true);
    (page.group.banner_url, page.group.icon_url) = (cards[0].banner_url.clone(), cards[0].icon_url.clone());
    page.group.id = cards[0].id.clone();
    let mut members = super::groups::members();
    for (i, m) in members.iter_mut().enumerate() {
        keep(&mut m.icon_url, 9700 + i as u64, super::groups::icon(i as u32 + 2));
    }
    let raw = members.len();
    let posts = super::groups::posts();
    let written = posts.len();
    (cards, page, Members { items: members, raw, total: None }, Posts { items: posts, raw: written, total: None })
}

/// The run before: what it showed, written as the service writes it. Once a run of the harness.
fn before() -> LauncherState {
    use std::sync::OnceLock;
    static BEFORE: OnceLock<LauncherState> = OnceLock::new();
    BEFORE
        .get_or_init(|| {
            let (root, a) = (store::root(), account(MEMBER, ME));
            store::begin(&root, &a);
            let pictures = Profiles::of(Some(&a));
            let list = people(&pictures);
            let (cards, page, members, posts) = groups(&pictures);
            let mut changes = store::launcher_changes(None, &list, at());
            let group = cards[0].id.clone();
            changes.extend([Change::put(kind::GROUPS, ONLY, at(), "", &cards), Change::put(kind::GROUP, &group, at(), "", &page), Change::put(kind::MEMBERS, &group, at(), "", &members), Change::put(kind::POSTS, &group, at(), "", &posts)].into_iter().flatten());
            let wrote = Store::open(&root, &a).is_some_and(|mut s| s.write(&changes));
            let from = std::time::Instant::now();
            let friends = Store::read(&root, &a).and_then(|s| s.launcher()).map_or(0, |l| l.friends.len());
            println!("kept: {friends} friends written ({wrote}) and read in {:?}; the folder takes {} KB", from.elapsed(), store::size(&root) / 1024);
            list
        })
        .clone()
}

/// The snapshot a window that connects then has: as the service makes it from the store.
pub(super) fn prepare(sc: &mut Scenario) {
    if !NAMES.contains(&sc.name) {
        return;
    }
    let live = before();
    let root = store::root();
    let (member, user) = match sc.name {
        "kept-other-member" => (OTHER_MEMBER, ME),
        "kept-other-account" => (MEMBER, OTHER),
        _ => (MEMBER, ME),
    };
    let verified = Some(VerifiedFor { epoch: 1, member: member.into(), user: Some(user.into()) });
    let me = User { id: user.into(), display_name: if user == ME { "Desktop Tester" } else { "Another Account" }.into(), ..Default::default() };
    let s: &mut Snapshot = &mut sc.snapshot;
    s.account.state = AccountState::SignedIn { username: "tester".into() };
    let mut broken = None;
    if sc.name == "kept-corrupted" {
        let dir = account(MEMBER, ME).dir(&root);
        let file = dir.join("state.sqlite3");
        broken = std::fs::read(&file).ok().map(|b| (file.clone(), b));
        for end in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("state.sqlite3{end}")));
        }
        let _ = std::fs::write(&file, vec![0x5Au8; 20_000]);
    }
    let restored = Store::read(&root, &account(member, user)).and_then(|s| s.launcher());
    if let Some((file, _)) = broken {
        // Made anew, as the service does; nothing of it is there.
        let _ = std::fs::remove_file(file);
    }
    s.vrchat_login = match sc.name {
        "kept-before" | "kept-signed-in" | "kept-group" | "kept-other-account" => Status { allowed: true, user: Some(me), verified_for: verified.clone(), ..Status::default() },
        _ => Status { restoring: true, waiting: sc.name == "kept-offline", known: Some(me), verified_for: verified.clone(), ..Status::default() },
    };
    let mut list = match sc.name {
        "kept-before" => live,
        // As Home's launcher is with nobody's list.
        _ => restored.unwrap_or_else(|| LauncherState { possible: Possible { play: true, ..Possible::default() }, loading: s.vrchat_login.user.is_some(), ..LauncherState::default() }),
    };
    if matches!(list.source, Source::SignedIn { .. }) {
        list.verified_for = verified;
    }
    if s.vrchat_login.user.is_none() {
        list.possible = Possible { live_status: true, play: true, ..Possible::default() };
    }
    list.possible.play = true;
    s.launcher = list.into();
}

/// Who is signed in to Orbiters on the scenario's computer.
pub(super) fn member(ctl: &crate::controller::Controller, sc: &Scenario) {
    if let Some(v) = NAMES.contains(&sc.name).then(|| sc.snapshot.vrchat_login.verified_for.clone()).flatten() {
        ctl.stand_in_vrchat_member(&v.member);
    }
}

/// On frame `f`, before it is drawn.
pub(super) fn stage(app: &mut App, name: &str, f: u32) {
    if !NAMES.contains(&name) {
        return;
    }
    if f == 0 {
        // A window of its own: what the one before holds in memory is not there.
        crate::worlds::at_once::start_again();
    }
    let home = app.home_mut();
    if f == 1 {
        let launcher = home.launcher_mut();
        let (waiting, unread) = (launcher.stand_in_pictures_loading(), launcher.wanted_pictures().len());
        println!("{name}: after the first frame, pictures still being read: {waiting}; not had: {unread}; groups: {}", home.groups_mut().kept_cards().map_or("none".into(), |n| n.to_string()));
    }
    if name == "kept-group" && f == 1 {
        home.groups_mut().open_kept(0);
    }
}
