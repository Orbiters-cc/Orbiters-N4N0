//! The window's groups state machine driven for real: the window's `State` with its jobs, through a real link to a real
//! service and its handler, with only VRChat's side made up (`controller::groups::world`). A link proxy
//! (`link_between`) loses the service's reply where a test needs it lost. The matrix of every write and every failure is
//! at the end.
use super::*;
use crate::backend::{Ctl, Remote};
use crate::controller::groups::world::{decoys, route, write_of, writes, Fault, Hold, Shown, World, G, OTHERS_ID, OTHERS_WORDS};
use crate::controller::groups::{Request, Write};
use crate::controller::Controller;
use crate::ipc::Role;
use crate::link_between::Between;
use crate::service::tests::{linked_through, service};
use crate::service::Service;
use crate::state::Snapshot;
use orbiters_core::groups::{PostDraft, Visibility};
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_api::{Scripted, Sent};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A window's state over `ctl`, signed in as the service's controller is.
struct Window {
    state: State,
    ctx: egui::Context,
    ctl: Ctl,
    snap: Arc<Snapshot>,
    toasts: orbiters_ui::toast::Toasts,
    t: f64,
}

impl Window {
    fn new(ctl: Ctl, snap: Arc<Snapshot>) -> Self {
        let mut w = Self { state: State::default(), ctx: egui::Context::default(), ctl, snap, toasts: Default::default(), t: 0.0 };
        w.frame();
        w
    }
    /// The window's own frame: the account followed, the answers taken (its clock a tenth of a second on).
    fn frame(&mut self) {
        self.t += 0.1;
        let input = egui::RawInput { time: Some(self.t), ..Default::default() };
        let (snap, ctl, state, toasts) = (&self.snap, &self.ctl, &mut self.state, &mut self.toasts);
        let mut out = self.ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            let mut cx = Cx { ctl, snap, toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
            state.sync(&cx);
            state.take(&ctx, &mut cx);
            state.keep_drafts();
        });
        out.textures_delta.clear();
    }
    /// Frames until `done`, a few seconds at most.
    fn until(&mut self, done: impl Fn(&State) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            self.frame();
            if done(&self.state) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }
    /// Frames until nothing is asked of the service any more.
    fn settle(&mut self) {
        assert!(self.until(|s| s.jobs.running.is_empty() && !s.ledger_due && s.cancels.is_empty()), "the window settles");
    }
    fn prepare(&mut self, write: Write) {
        let ctl = self.ctl.clone();
        self.state.prepare(&self.ctx, &ctl, write, Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(80.0, 30.0)));
    }
    /// The sealed write its confirmation shows.
    fn shown(&self) -> Sealed {
        match &self.state.flow {
            Some(Flow::Confirming { sealed, .. }) => sealed.clone(),
            _ => panic!("no confirmation; said: {:?}", self.state.said),
        }
    }
    fn confirm(&mut self, write: Write) -> Sealed {
        self.prepare(write);
        assert!(self.until(|s| !matches!(s.flow, Some(Flow::Preparing { .. }))));
        self.shown()
    }
    /// The person's Send on the confirmation shown.
    fn send(&mut self) {
        let Some(Flow::Confirming { sealed, .. }) = self.state.flow.take() else { panic!("no confirmation") };
        let ctl = self.ctl.clone();
        self.state.commit(&self.ctx, &ctl, sealed);
    }
    fn check(&mut self, op: &str) {
        let ctl = self.ctl.clone();
        self.state.check(&self.ctx, &ctl, op);
        self.settle();
    }
    fn stands(&self, op: &str) -> Option<Stands> {
        self.state.ledger.iter().find(|e| e.op == op).map(|e| e.state)
    }
    /// Draws what shows over the page for a while (`events` on the last frame), as the window does.
    fn drawn(&mut self, frames: usize, events: Vec<egui::Event>, draw: impl Fn(&mut Ui, &mut Cx, &mut State)) -> egui::FullOutput {
        let mut last = None;
        for i in 0..frames {
            self.t += 1.0 / 60.0;
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), time: Some(self.t), ..Default::default() };
            if i + 1 == frames {
                input.events = events.clone();
            }
            let (snap, ctl, state, toasts) = (&self.snap, &self.ctl, &mut self.state, &mut self.toasts);
            let mut out = self.ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl, snap, toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                draw(ui, &mut cx, state);
            });
            out.textures_delta.clear();
            last = Some(out);
        }
        last.unwrap()
    }
    /// Whether the confirmation's button can be pressed.
    fn sendable(&mut self, action: &str) -> bool {
        orbiters_ui::widgets::watch_controls();
        self.drawn(40, vec![], confirmation);
        orbiters_ui::widgets::control(&self.ctx, action).expect("its button is drawn").1
    }
    fn ask(&self) -> impl Fn(Request) -> Reply + '_ {
        move |r| self.ctl.groups(Scope::of(&self.snap), r)
    }
}

fn words(out: &egui::FullOutput) -> String {
    fn walk(s: &egui::Shape, to: &mut String) {
        match s {
            egui::Shape::Text(t) => {
                to.push_str(t.galley.text());
                to.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, to)),
            _ => {}
        }
    }
    let mut to = String::new();
    out.shapes.iter().for_each(|s| walk(&s.shape, &mut to));
    to
}

/// A service over a made-up VRChat, and a window linked to it.
struct Rig {
    service: Arc<Service>,
    world: Shown,
    sent: Arc<Mutex<Vec<Sent>>>,
    between: Between,
    w: Window,
}

fn rig() -> Rig {
    let service = service();
    let world: Shown = Arc::new(Mutex::new(World::default()));
    let sent = service.ctl.test_vrchat_routed(route(world.clone()));
    let (client, between) = linked_through(&service, Role::Window);
    let w = Window::new(Arc::new(Remote(client)), service.ctl.view());
    let mut rig = Rig { service, world, sent, between, w };
    rig.w.settle();
    rig
}
impl Rig {
    /// Another window: a new link, a new state.
    fn another(&self) -> Window {
        let client = crate::ipc::Client::connect(&self.service.endpoint_for_tests(), Role::Window).unwrap();
        Window::new(Arc::new(Remote(client)), self.service.ctl.view())
    }
    fn write(&self, kind: Wrote) -> Write {
        write_of(kind, &self.w.ask())
    }
    fn later(&self) {
        self.service.ctl.groups_book_for_tests().ahead_ms.store(90_000, std::sync::atomic::Ordering::Relaxed);
    }
    fn entry(&self, op: &str) -> Entry {
        let book = self.service.ctl.groups_book_for_tests();
        book.get(&self.service.ctl.groups_key(&Scope::of(&self.service.ctl.view())).unwrap(), op).unwrap()
    }
    fn sent_writes(&self) -> usize {
        writes(&self.sent).len()
    }
}
fn local() -> (Ctl, Arc<Snapshot>) {
    let ctl: Ctl = Arc::new(crate::backend::Local::new(Controller::new(Settings::default())));
    let snap = ctl.view();
    (ctl, snap)
}
fn card() -> GroupCard {
    GroupCard { id: G.into(), name: "Made-up Makers".into(), permissions: vec!["*".into()], named: true, ..Default::default() }
}

/// Blocker 6, J1: a held Enter (key repeats) on the focused Send of a real confirmation dialog sends nothing; a fresh
/// press sends it.
#[test]
fn a_held_key_never_sends_a_confirmation() {
    let (ctl, snap) = local();
    let mut w = Window::new(ctl, snap);
    w.state.staged = true;
    w.state.scope = Some(Scope::of(&w.snap));
    stage::confirming(&mut w.state, Sealed::made_up(Write::DeleteRole { group: G.into(), role: "grol_a".into() }, true), Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(80.0, 30.0)), None);
    orbiters_ui::widgets::watch_controls();
    w.drawn(90, vec![], confirmation);
    let (send, _) = orbiters_ui::widgets::control(&w.ctx, "Delete the role").expect("the Send button is drawn");
    let key = |key, pressed: bool| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    // Enter goes down before Send has the focus (held on the press that opened the confirmation): what follows is repeats.
    w.drawn(1, vec![key(egui::Key::Enter, true)], confirmation);
    let mut focused = false;
    for _ in 0..12 {
        w.drawn(1, vec![key(egui::Key::Tab, true)], confirmation);
        if w.ctx.memory(|m| m.focused()).and_then(|id| w.ctx.read_response(id)).is_some_and(|r| r.rect == send) {
            focused = true;
            break;
        }
    }
    assert!(focused, "Send takes the focus");
    for events in [vec![key(egui::Key::Enter, true)], vec![key(egui::Key::Enter, true), key(egui::Key::Enter, true)], vec![key(egui::Key::Enter, false)]] {
        w.drawn(1, events, confirmation);
        assert!(matches!(w.state.flow, Some(Flow::Confirming { .. })), "a repeating key, and its release, send nothing");
    }
    w.drawn(1, vec![key(egui::Key::Enter, true)], confirmation);
    assert!(matches!(w.state.flow, Some(Flow::Sending { .. })), "a fresh press sends it");
}

/// verify2 B-E: the confirmation shows the sealed bytes and nothing else, and cannot be sent before it does: not a picture
/// of other bytes, not the service's answer to another operation.
#[test]
fn a_picture_is_applied_only_once_its_sealed_bytes_show() {
    let mut r = rig();
    let write = r.write(Wrote::ApplyBanner);
    r.w.prepare(write);
    assert!(r.w.until(|s| matches!(&s.flow, Some(Flow::Confirming { art: Some(_), .. }))), "the sealed picture arrives");
    let op = r.w.shown().op;
    assert!(r.w.sendable("Change the banner"));
    let wrong = super::digest(b"other bytes");
    for (what, kind) in [("other bytes", Kind::Art { op: op.clone(), sha256: wrong }), ("another operation", Kind::Art { op: "op-other".into(), sha256: r.w.shown().art.unwrap().sha256 })] {
        if let Some(Flow::Confirming { art, .. }) = &mut r.w.state.flow {
            *art = None;
        }
        let (scope, ctl) = (Scope::of(&r.w.snap), r.w.ctl.clone());
        r.w.state.jobs.ask(&r.w.ctx, &ctl, &scope, kind, Request::SealedArt { op: op.clone() });
        r.w.settle();
        assert!(!r.w.sendable("Change the banner"), "{what}");
    }
}
/// The second verification's own assertion (verify2-groups-scratch/probe), in the new shapes: a texture that is not the
/// chosen file's cannot enable Apply. No texture of the window's can reach a confirmation any more: the only way in is the
/// sealed bytes by their hash, so another file's picture, however it arrives, does not.
#[test]
fn verify2_an_unrelated_preview_cannot_enable_apply() {
    let mut r = rig();
    let write = r.write(Wrote::ApplyBanner);
    // The sealed file's picture never comes; another file's does, in the page's own pictures.
    r.world.lock().unwrap().instead = vec![];
    let sealed = r.w.confirm(write);
    if let Some(Flow::Confirming { art, .. }) = &mut r.w.state.flow {
        *art = None;
    }
    let ctx = r.w.ctx.clone();
    for url in [format!("https://api.vrchat.cloud/api/1/file/{}/1/file", "file_up"), "https://api.vrchat.cloud/api/1/file/file_gal1/1/file".into()] {
        stage::picture(&ctx, &mut r.w.state, &url, egui::ColorImage::filled([8, 8], egui::Color32::BLUE));
    }
    r.w.frame();
    assert!(!matches!(&r.w.state.flow, Some(Flow::Confirming { art: Some(_), .. })) || sealed.art.is_none());
    assert!(!r.w.sendable("Change the banner"), "an unrelated texture does not establish the identity of the file being confirmed");
}

/// verify2 B-F: leaving the view a write was asked from ends it unsent, at every point before the send; what the next
/// view shows and sends is its own. The studio's Back, Escape's close, the page's Back, Home, another tab of the window.
#[test]
fn leaving_a_view_ends_what_it_had_not_sent() {
    let leaves: [(&str, fn(&mut State)); 4] = [("the studio's Back", |s| s.close_studio()), ("the page closed", |s| s.close()), ("Home left", |s| s.end_flow()), ("another page opened", |s| { s.end_flow(); s.open = None })];
    for (what, leave) in leaves {
        for while_preparing in [true, false] {
            let mut r = rig();
            let write = r.write(Wrote::ApplyBanner);
            r.w.state.studio = Some(studio::Studio::new(card(), "usr_test", None));
            r.w.prepare(write);
            if !while_preparing {
                assert!(r.w.until(|s| matches!(s.flow, Some(Flow::Confirming { .. }))));
            }
            leave(&mut r.w.state);
            // Another group's page is entered; the late answer comes.
            r.w.state.open = Some(page::Open::new(GroupCard { id: "grp_00000000-0000-4000-8000-0000000000ff".into(), name: "Another".into(), ..Default::default() }, &r.w.state.drafts));
            r.w.settle();
            assert!(r.w.state.flow.is_none(), "{what}: no confirmation is taken up by another view");
            assert!(r.service.ctl.groups_book_for_tests().of(&r.service.ctl.groups_key(&Scope::of(&r.w.snap)).unwrap()).iter().all(|e| e.state == Stands::Refused || what == "the page closed" && while_preparing), "{what}: over, unsent");
            assert_eq!(r.sent_writes(), 0);
        }
    }
    // A late Done updates no other group's page.
    let mut r = rig();
    let hold = Arc::new(Hold::default());
    let write = r.write(Wrote::DeleteRole);
    r.w.confirm(write);
    r.world.lock().unwrap().fault = Some(Fault::Held(hold.clone()));
    r.w.send();
    assert!(hold.until_reached());
    r.w.state.end_flow();
    let mut other = page::Open::new(GroupCard { id: "grp_00000000-0000-4000-8000-0000000000ff".into(), name: "Another".into(), ..Default::default() }, &r.w.state.drafts);
    other.page = Some(crate::controller::groups::Page { group: orbiters_core::groups::Group { roles: Some(vec![]), ..Default::default() }, catalogue: None });
    r.w.state.open = Some(other);
    hold.let_go();
    r.w.settle();
    assert_eq!(r.w.state.open.as_ref().unwrap().page.as_ref().unwrap().group.roles, Some(vec![]), "its own roles, not the other group's");
}

/// Design 5's generic test: every piece of the window's live state changed between the seal and the send; what goes over
/// the wire is the sealed payload, or nothing.
#[test]
fn whatever_the_window_becomes_the_wire_is_the_sealed_payload() {
    let changes: [(&str, fn(&mut Window)); 7] = [
        ("the studio replaced", |w| w.state.studio = Some(studio::Studio::new(GroupCard { id: "grp_other".into(), ..card() }, "usr_test", None))),
        ("another group's page", |w| w.state.open = Some(page::Open::new(GroupCard { id: "grp_other".into(), name: "Other".into(), ..Default::default() }, &w.state.drafts))),
        ("the composer rewritten", |w| {
            let mut o = page::Open::new(card(), &w.state.drafts);
            o.composer = Some(page::Composer::new(PostDraft { title: "Changed".into(), text: "after the seal".into(), visibility: Visibility::Public, ..Default::default() }));
            w.state.open = Some(o);
        }),
        ("the ledger forgotten", |w| w.state.ledger.clear()),
        ("the pictures replaced", |w| {
            let ctx = w.ctx.clone();
            stage::picture(&ctx, &mut w.state, "https://api.vrchat.cloud/api/1/file/file_gal1/1/file", egui::ColorImage::filled([8, 8], egui::Color32::BLUE));
        }),
        ("what it said and observed", |w| {
            w.state.said = Some("x".into());
            w.state.observed.clear();
        }),
        ("the numbers", |w| {
            w.state.token += 5;
            w.state.serial += 5;
        }),
    ];
    for kind in Wrote::ALL {
        let mut r = rig();
        let write = r.write(kind);
        let sealed = r.w.confirm(write);
        for (_, change) in &changes {
            change(&mut r.w);
        }
        let payload = r.entry(&sealed.op).payload;
        r.w.send();
        r.w.settle();
        assert_eq!(r.sent_writes(), 1, "{kind:?}");
        let e = r.entry(&sealed.op);
        assert_eq!((e.state, e.payload.as_str()), (Stands::Done, payload.as_str()), "{kind:?}");
        let sent = r.sent.lock().unwrap();
        let out = sent.iter().find(|s| s.method != "GET").unwrap();
        let wire = match &out.upload {
            Some((tag, bytes)) => orbiters_core::groups::digest(&["POST", &format!("file/image/{tag}"), &super::digest(bytes)]),
            None => orbiters_core::groups::digest(&[&out.method, out.url.trim_start_matches("https://api.vrchat.cloud/api/1/groups/"), out.body.as_deref().unwrap_or("")]),
        };
        assert_eq!(wire, payload, "{kind:?}");
    }
}

/// verify2 B-A, the second verification's own assertion in the new shapes: the window that sends is dropped (its process
/// gone) and the operation is still there, in the service's record, for the next.
#[test]
fn verify2_pending_send_survives_window_drop() {
    let mut r = rig();
    let hold = Arc::new(Hold::default());
    let write = r.write(Wrote::CreatePost);
    let op = r.w.confirm(write).op;
    r.world.lock().unwrap().fault = Some(Fault::Held(hold.clone()));
    r.w.send();
    assert!(hold.until_reached());
    let next = r.another();
    let gone = std::mem::replace(&mut r.w, next);
    drop(gone);
    r.w.settle();
    assert_eq!(r.w.stands(&op), Some(Stands::Dispatched), "listed by the next window while it is being sent");
    assert_eq!(r.w.state.open_writes(G).len(), 1);
    // And on the disk, whatever becomes of the service.
    let book = r.service.ctl.groups_book_for_tests();
    // (Its lock ends with it: its copy is what the next program finds.)
    let after = crate::controller::groups::Ledger::at(copied(&book.dir()));
    let key = r.service.ctl.groups_key(&Scope::of(&r.w.snap)).unwrap();
    assert_eq!(after.get(&key, &op).unwrap().state, Stands::Unknown, "a program that ended here comes back to 'unknown'");
    hold.let_go();
}

/// The folder of a record as it is now, for another program to find.
fn copied(from: &std::path::Path) -> std::path::PathBuf {
    let to = from.with_extension("after");
    std::fs::create_dir_all(&to).unwrap();
    for f in std::fs::read_dir(from).unwrap().flatten().filter(|f| f.path().extension().is_some_and(|x| x == "json")) {
        std::fs::copy(f.path(), to.join(f.file_name())).unwrap();
    }
    to
}

/// verify2 B-C, the second verification's own assertion in the new shapes, and the scan of every file: deleting someone
/// else's post, and everything else the feature does, writes nothing of theirs to the disk.
#[test]
fn verify2_editing_someone_elses_post_writes_no_private_baseline() {
    let mut r = rig();
    let Reply::Posts(page) = r.w.ask()(Request::Posts { group: G.into(), offset: 0 }) else { panic!() };
    let theirs = page.items.iter().find(|p| p.text.contains(OTHERS_WORDS) && p.author_id == OTHERS_ID).expect("the fixture is someone else's").clone();
    let mut o = page::Open::new(card(), &r.w.state.drafts);
    o.posts.items = page.items;
    r.w.state.open = Some(o);
    for _ in 0..3 {
        r.w.frame();
    }
    // Its deletion is sent and its answer lost; checked; a new post is written and kept; the page is closed.
    let op = r.w.confirm(Write::DeletePost { group: G.into(), post: theirs }).op;
    r.world.lock().unwrap().fault = Some(Fault::Drop { done: true });
    r.w.send();
    r.w.settle();
    r.later();
    r.w.check(&op);
    r.w.state.open.as_mut().unwrap().composer = Some(page::Composer::new(PostDraft { title: "My own".into(), text: "my own words".into(), ..Default::default() }));
    r.w.frame();
    r.w.state.close();
    let mut mine = false;
    let mut scanned = 0;
    // Everything the feature keeps, this test's and whatever else is there: nobody else's words or id in any of it.
    for path in files() {
        let bytes = std::fs::read(&path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        for theirs in [OTHERS_WORDS, OTHERS_ID] {
            assert!(!text.contains(theirs), "{theirs} in {}", path.display());
        }
        mine |= text.contains("my own words");
        scanned += 1;
    }
    assert!(scanned >= 2 && mine, "{scanned} files scanned, the person's own new post among them");
}
/// Every file under the feature's folders.
fn files() -> Vec<std::path::PathBuf> {
    let root = orbiters_core::paths::local_data_root();
    let mut out = Vec::new();
    let mut stack = vec![root.join("group-drafts"), root.join("group-ledger"), root.join("group-uploads"), root.join("group-banners")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() { stack.push(p) } else { out.push(p) }
        }
    }
    out
}

/// Major 8, J9, verify3 Major 6: the new post being written is the account's own: closed and opened again, and from another
/// window, it is back; two windows writing in one group keep both; another account's is not it. The very one the ledger
/// says is out is not offered again; a later one with the same words is.
#[test]
fn new_posts_being_written_come_back() {
    let mut r = rig();
    let scope = Scope::of(&r.w.snap);
    let write = |w: &mut Window, group: &str, title: &str| {
        let mut o = page::Open::new(GroupCard { id: group.into(), ..card() }, &Drafts::default());
        o.composer = Some(page::Composer::new(PostDraft { title: title.into(), text: "written".into(), send_notification: true, ..Default::default() }));
        w.state.open = Some(o);
        w.frame();
        w.state.close();
    };
    let mut other = r.another();
    write(&mut r.w, G, "Half");
    write(&mut other, "grp_00000000-0000-4000-8000-0000000000fe", "Elsewhere");
    let newest = |group: &str| Drafts::load(&scope).newest(group).map(|k| (k.draft.title.clone(), k.draft.send_notification));
    assert_eq!(newest(G), Some(("Half".into(), true)));
    assert_eq!(newest("grp_00000000-0000-4000-8000-0000000000fe").map(|d| d.0).as_deref(), Some("Elsewhere"));
    std::thread::sleep(Duration::from_millis(5));
    write(&mut other, G, "Other half");
    assert_eq!(newest(G).map(|d| d.0).as_deref(), Some("Other half"), "the newest is offered");
    // It is cancelled: the other one is still there.
    let mut o = page::Open::new(card(), &Drafts::load(&scope));
    o.end_composer();
    other.state.drafts = Drafts::load(&scope);
    other.state.open = Some(o);
    other.frame();
    other.state.close();
    assert_eq!(newest(G).map(|d| d.0).as_deref(), Some("Half"), "two windows keep each their own");
    let open = page::Open::new(card(), &Drafts::load(&scope));
    let c = open.composer.as_ref().unwrap();
    assert!(c.restored && c.draft.title == "Half");
    // Sent from there; the window ends before its answer; the next one finds the post out, and does not offer it again.
    let op = r.w.confirm(Write::CreatePost { group: G.into(), draft: c.draft.clone(), of: c.id.clone() }).op;
    r.w.send();
    assert!(r.w.until(|s| s.flow.is_none()));
    let mut next = r.another();
    next.settle();
    assert_eq!(next.stands(&op), Some(Stands::Done));
    let offered = |next: &mut Window, change: &dyn Fn(&mut page::Composer)| {
        let mut o = page::Open::new(card(), &Drafts::default());
        let mut c = page::Composer { id: c.id.clone(), draft: c.draft.clone(), restored: true };
        change(&mut c);
        o.composer = Some(c);
        next.state.open = Some(o);
        next.state.ledger_due = true;
        next.settle();
        next.state.open.as_ref().unwrap().composer.is_some()
    };
    assert!(offered(&mut next, &|c| c.id = uuid::Uuid::new_v4().to_string()), "a later announcement with the same words is another");
    assert!(offered(&mut next, &|c| c.draft.text = "written, and more".into()), "what was written since stays");
    assert!(!offered(&mut next, &|_| {}), "it is on VRChat");
    assert!(!art::drafts_dir(&scope).unwrap().join(format!("{}.json", c.id)).exists(), "and off the disk");
    assert_eq!(newest(G).map(|d| d.0).as_deref(), Some("Half"), "the later one with the same words is still there");
    let _ = std::fs::remove_dir_all(art::drafts_dir(&scope).unwrap());
}

/// Major 1: an upload of the largest size accepted crosses the real link as a small message (its PNG is staged on
/// disk), one byte more is refused before anything is sent, and the link stays up for the next request.
#[test]
fn an_upload_at_the_size_limits_keeps_the_link_up() {
    let r = rig();
    let max = orbiters_core::vrchat_api::GROUP_UPLOAD_MAX;
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(max, 7);
    let up = |png: &[u8]| Request::Prepare { write: Write::Upload { tag: FileTag::Gallery, group: G.into(), staged: stage_upload(png).unwrap(), art: None } };
    let ask = r.w.ask();
    assert!(matches!(ask(up(&png)), Reply::Prepared(_)), "the largest picture is prepared");
    let message = serde_json::to_vec(&crate::ipc::ToService::Groups { id: "x".into(), scope: Scope::of(&r.w.snap), request: up(&png) }).unwrap();
    assert!(message.len() < 4096, "the link carries a reference, never the picture: {} bytes", message.len());
    png.push(0);
    assert_eq!(ask(up(&png)), Reply::Failed(Failure::TooLarge));
    assert!(!matches!(ask(Request::Discover), Reply::Failed(Failure::Lost)), "the link is still up");
}

/// Major 9: the service takes in a few groups requests at once, before starting a thread for any; the rest are told to
/// wait at once. The admitted ones (checks, which never wait behind reads inside the controller) are held inside
/// VRChat's (scripted) transport until the others have their answer.
#[test]
fn the_service_admits_a_few_requests_at_once() {
    use std::sync::{mpsc, Condvar};
    let mut r = rig();
    let write = r.write(Wrote::DeleteRole);
    let op = r.w.confirm(write).op;
    r.world.lock().unwrap().fault = Some(Fault::Drop { done: false });
    r.w.send();
    r.w.settle();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = (0..12).map(|_| {
        let gate = gate.clone();
        Scripted::During(
            Box::new(move || {
                let (open, cv) = &*gate;
                let _ = cv.wait_timeout_while(open.lock().unwrap(), Duration::from_secs(10), |open| !*open);
            }),
            200,
            json!({}),
        )
    });
    let _sent = r.service.ctl.test_vrchat_scripted(held.collect());
    let before = r.service.ctl.groups_entered();
    let (ctl, snap) = (r.w.ctl.clone(), r.service.ctl.view());
    let (tx, rx) = mpsc::channel();
    let mut replies = Vec::new();
    let max = crate::controller::groups::ADMIT_MAX;
    let (entered, busy) = std::thread::scope(|s| {
        for _ in 0..12 {
            let (ctl, scope, tx, op) = (ctl.clone(), Scope::of(&snap), tx.clone(), op.clone());
            s.spawn(move || tx.send(ctl.groups(scope, Request::Check { op })).unwrap());
        }
        let start = Instant::now();
        while replies.len() < 12 - max && start.elapsed() < Duration::from_secs(5) {
            if let Ok(r) = rx.recv_timeout(Duration::from_millis(50)) {
                replies.push(r);
            }
        }
        let counted = (r.service.ctl.groups_entered() - before, replies.iter().filter(|r| **r == Reply::Failed(Failure::Busy)).count());
        let (open, cv) = &*gate;
        *open.lock().unwrap() = true;
        cv.notify_all();
        counted
    });
    assert!(entered <= max as u64, "{entered} reached the controller: admission comes first ({replies:?})");
    assert_eq!(busy, 12 - max, "told to wait while the others run: {replies:?}");
}

/// J4: a picture on its way when the page closes, and one that failed, are asked again when it is back; J8: a card that
/// could not be read waits before it is asked again, its siblings are named.
#[test]
fn what_was_not_read_is_asked_again_but_not_at_once() {
    let mut r = rig();
    let url = "https://api.vrchat.cloud/api/1/file/file_gal1/1/file";
    let (ctx, ctl, scope) = (r.w.ctx.clone(), r.w.ctl.clone(), Scope::of(&r.w.snap));
    assert!(r.w.state.picture(&ctx, &ctl, &scope, url).is_none(), "asked");
    r.w.state.close();
    assert!(r.w.state.picture(&ctx, &ctl, &scope, url).is_none(), "asked anew");
    assert!(r.w.until(|s| matches!(s.pictures.map.get(url), Some(Pic::Ready(_)))), "and arrives");
    r.w.state.pictures.map.insert("https://x".into(), Pic::Failed);
    r.w.state.close();
    assert!(!r.w.state.pictures.map.contains_key("https://x"));

    r.w.state.staged = false;
    let ids = [G.to_string(), "grp_00000000-0000-4000-8000-0000000000f1".into()];
    r.w.state.discovery = Discovery::Ready(ids.iter().map(|id| GroupCard { id: id.clone(), ..Default::default() }).collect(), 1.0e9);
    r.w.state.jobs.ask(&ctx, &ctl, &scope, Kind::Hydrate, Request::Hydrate { ids: ids.to_vec() });
    r.w.settle();
    let Discovery::Ready(cards, _) = &r.w.state.discovery else { panic!() };
    assert_eq!((cards[0].named, cards[1].named), (true, false), "one that fails leaves the others");
    assert!(r.w.state.hydrate_again.get(&ids[1]).is_some_and(|at| *at > r.w.t + 30.0));
    let before = calls_of(&r);
    r.w.drawn(5, vec![], home_section);
    r.w.settle();
    assert_eq!(calls_of(&r), before, "not asked again at each frame");
}
fn calls_of(r: &Rig) -> usize {
    crate::controller::groups::world::calls(&r.sent).iter().filter(|c| c.contains("f1?")).count()
}

/// J5: while a confirmation, the studio, a chooser or an editor of a group is up, Home says a modal is, so the chat's
/// shortcut does not take the keys.
#[test]
fn groups_own_the_keyboard_while_they_are_up() {
    let (ctl, snap) = local();
    let mut w = Window::new(ctl, snap);
    assert!(!w.state.holds());
    w.state.studio = Some(studio::Studio::new(card(), "usr_test", None));
    assert!(w.state.holds(), "the studio");
    w.state.studio = None;
    w.state.open = Some(page::Open::new(card(), &w.state.drafts));
    assert!(!w.state.holds(), "a page to read");
    w.state.open.as_mut().unwrap().composer = Some(page::Composer::new(PostDraft::default()));
    assert!(w.state.holds(), "the composer");
    w.state.open = None;
    stage::confirming(&mut w.state, Sealed::made_up(Write::DeleteRole { group: G.into(), role: "grol_a".into() }, true), Rect::NOTHING, None);
    assert!(w.state.holds(), "a confirmation");
    let mut home = crate::pages::home::State::default();
    assert!(!home.modal_up());
    home.groups_mut().studio = Some(studio::Studio::new(card(), "usr_test", None));
    assert!(home.modal_up(), "Home says so to the chat");
    // The same through the one place every layer says it: the studio, the page's editors, the confirmation.
    let front = |w: &mut Window| {
        w.drawn(3, vec![], show_page);
        orbiters_ui::front::held(&w.ctx)
    };
    let (ctl, snap) = local();
    let mut w = Window::new(ctl, snap);
    w.state.staged = true;
    w.state.scope = Some(Scope::of(&w.snap));
    w.state.open = Some(page::Open::new(card(), &w.state.drafts));
    assert!(!front(&mut w), "a page to read holds nothing");
    w.state.open.as_mut().unwrap().composer = Some(page::Composer::new(PostDraft::default()));
    assert!(front(&mut w), "the composer");
    w.state.open.as_mut().unwrap().composer = None;
    assert!(!front(&mut w), "closed");
    w.state.open.as_mut().unwrap().role_editor = Some(page::RoleEditor::new(None, Default::default()));
    assert!(front(&mut w), "the role editor");
    w.state.open.as_mut().unwrap().role_editor = None;
    w.state.studio = Some(studio::Studio::new(card(), "usr_test", None));
    assert!(front(&mut w), "the studio");
    w.state.studio = None;
    stage::confirming(&mut w.state, Sealed::made_up(Write::DeleteRole { group: G.into(), role: "grol_a".into() }, true), Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(80.0, 30.0)), None);
    assert!(front(&mut w), "a confirmation");
    assert!(orbiters_ui::front::missing(&w.ctx).is_empty(), "no layer here that did not say so");
}

/// verify3 Major 7: a confirmation that comes under a key already held down is not confirmed by it, through the page's own
/// flow: Enter saves a role, is held while its confirmation comes and takes the focus, repeats: nothing goes. Let go and
/// pressed again, it goes.
#[test]
fn a_key_held_from_the_opener_confirms_nothing() {
    let key = |k, pressed: bool, repeat: bool| egui::Event::Key { key: k, physical_key: None, pressed, repeat, modifiers: egui::Modifiers::NONE };
    for k in [egui::Key::Enter, egui::Key::Space] {
        let mut r = rig();
        let Write::EditRole { baseline, edit, .. } = r.write(Wrote::EditRole) else { panic!() };
        let Reply::Opened(page) = r.w.ask()(Request::Open { group: G.into() }) else { panic!() };
        let mut o = page::Open::new(card(), &Drafts::default());
        o.answer(Kind::Open(G.into()), Reply::Opened(page));
        o.tab = page::Tab::Roles;
        o.role_editor = Some(page::RoleEditor::new(Some(baseline), edit));
        r.w.state.open = Some(o);
        orbiters_ui::widgets::watch_controls();
        r.w.drawn(6, vec![], show_page);
        let focus = |w: &mut Window, label: &str| {
            for _ in 0..60 {
                let at = orbiters_ui::widgets::control(&w.ctx, label).map(|(r, _)| r);
                if at.is_some() && w.ctx.memory(|m| m.focused()).and_then(|id| w.ctx.read_response(id)).map(|r| r.rect) == at {
                    return true;
                }
                w.drawn(1, vec![key(egui::Key::Tab, true, false), key(egui::Key::Tab, false, false)], show_page);
            }
            false
        };
        assert!(focus(&mut r.w, "Save"), "{k:?}: Save takes the focus");
        // Down, and held from here on.
        r.w.drawn(1, vec![key(k, true, false)], show_page);
        let mut confirming = false;
        for _ in 0..400 {
            r.w.drawn(1, vec![key(k, true, true)], show_page);
            confirming = matches!(r.w.state.flow, Some(Flow::Confirming { .. }));
            if confirming {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(confirming, "{k:?}: the press opened its confirmation");
        assert!(focus(&mut r.w, "Save the role"), "{k:?}: its Send takes the focus");
        // Repeats, and what some keyboards and tools send for them: presses not marked as repeats.
        for events in [vec![key(k, true, true)], vec![key(k, true, false)], vec![key(k, true, true), key(k, true, true)], vec![key(k, true, false), key(k, true, false)]] {
            for _ in 0..40 {
                r.w.drawn(1, events.clone(), show_page);
                assert!(matches!(r.w.state.flow, Some(Flow::Confirming { .. })), "{k:?}: {events:?}");
            }
        }
        assert_eq!(r.sent_writes(), 0);
        r.w.drawn(1, vec![key(k, false, false)], show_page);
        r.w.drawn(2, vec![], show_page);
        assert!(matches!(r.w.state.flow, Some(Flow::Confirming { .. })), "{k:?}: its release sends nothing");
        r.w.drawn(1, vec![key(k, true, false)], show_page);
        assert!(!matches!(r.w.state.flow, Some(Flow::Confirming { .. })), "{k:?}: a press of its own sends");
        r.w.settle();
        assert_eq!(r.sent_writes(), 1, "{k:?}");
    }
}

/// The local guard by itself: armed only after the key was seen up, once per press, whatever the pointer does.
#[test]
fn a_press_is_one_controls_own() {
    let key = |pressed: bool| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    let ctx = egui::Context::default();
    let mut t = 0.0;
    let mut frame = |shown: &[&str], focus: &str, events: Vec<egui::Event>| {
        t += 0.05;
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), time: Some(t), ..Default::default() };
        input.events = events;
        let mut hit = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            for (i, name) in shown.iter().enumerate() {
                let r = Rect::from_min_size(egui::pos2(10.0, 10.0 + 50.0 * i as f32), egui::vec2(100.0, 40.0));
                let resp = ui.interact(r, Id::unique(("control", *name)), Sense::click());
                if *name == focus {
                    resp.request_focus();
                }
                if deliberate(&resp) {
                    hit.push(name.to_string());
                }
            }
        });
        out.textures_delta.clear();
        hit
    };
    for _ in 0..3 {
        assert!(frame(&["a"], "a", vec![]).is_empty());
    }
    assert_eq!(frame(&["a"], "a", vec![key(true)]), ["a"]);
    // "b" comes while the key is down, and takes the focus.
    for _ in 0..3 {
        assert!(frame(&["a", "b"], "b", vec![key(true)]).is_empty(), "never armed under a key already down");
    }
    assert!(frame(&["a", "b"], "b", vec![key(false)]).is_empty());
    assert!(frame(&["a", "b"], "b", vec![]).is_empty());
    assert_eq!(frame(&["a", "b"], "b", vec![key(true)]), ["b"], "its own press");
    assert!(frame(&["a", "b"], "b", vec![key(false)]).is_empty());
    // Gone and back while the key is down: to be armed anew.
    assert!(frame(&["a"], "a", vec![]).is_empty());
    assert_eq!(frame(&["a"], "a", vec![key(true)]), ["a"]);
    assert!(frame(&["a", "b"], "b", vec![key(true)]).is_empty());
}

/// The panel: which write it is, what the check saw now, and the person's way out, from the real service; nothing offered
/// to decide before a check, and something always after (verify3 Major 3).
#[test]
fn the_panel_says_which_it_is_and_always_has_a_way_out() {
    let ctl_of = |r: &Rig| r.w.ctl.clone();
    let text_of = |r: &mut Rig| {
        let ctl = ctl_of(r);
        words(&r.w.drawn(2, vec![], |ui, _, s| {
            let ctx = ui.ctx().clone();
            uncertain_panel(ui, &ctx, &ctl, s, G);
        }))
    };
    let lose = |r: &mut Rig, kind: Wrote, done: bool| {
        let write = r.write(kind);
        let op = r.w.confirm(write).op;
        r.world.lock().unwrap().fault = Some(Fault::Drop { done });
        r.w.send();
        r.w.settle();
        r.later();
        op
    };
    let decide = |r: &mut Rig, op: &str, choice: Choice| {
        let (ctx, ctl) = (r.w.ctx.clone(), r.w.ctl.clone());
        r.w.state.decide(&ctx, &ctl, op, choice);
        r.w.settle();
    };
    let decisions = [tr("None of these"), tr("This is it"), tr("Drop it"), tr("Do it again…"), tr("I looked: it is there"), tr("I looked: it is not there")];
    // A post: there, opened to everyone by someone else since: shown, for the person.
    let mut r = rig();
    let op = lose(&mut r, Wrote::CreatePost, true);
    let text = text_of(&mut r);
    assert!(text.contains(tr("Check VRChat")) && text.contains("may or may not have been published"), "{text}");
    assert!(decisions.iter().all(|d| !text.contains(d)), "nothing to decide before a check: {text}");
    r.world.lock().unwrap().posts[0]["roleIds"] = json!([]);
    r.w.check(&op);
    let text = text_of(&mut r);
    for want in [tr("This is it"), tr("None of these"), tr("Drop it"), tr("not exactly what was sent"), "Saturday", "…s_new1"] {
        assert!(text.contains(want), "{want}: {text}");
    }
    let made = match &r.w.state.observed[&op] {
        Observation::Candidates(l) => l[0].id.clone(),
        other => panic!("{other:?}"),
    };
    decide(&mut r, &op, Choice::Shows { made: Some(made) });
    assert!(r.entry(&op).closed.is_some() && r.w.state.open_writes(G).is_empty());
    // Two deletions lost: told apart by the post, its id, when and as whom.
    let mut r = rig();
    let Reply::Posts(page) = r.w.ask()(Request::Posts { group: G.into(), offset: 0 }) else { panic!() };
    for post in &page.items {
        r.w.confirm(Write::DeletePost { group: G.into(), post: post.clone() });
        r.world.lock().unwrap().fault = Some(Fault::Drop { done: true });
        r.w.send();
        r.w.settle();
    }
    let mut o = page::Open::new(card(), &Drafts::default());
    o.posts.items = page.items.clone();
    r.w.state.open = Some(o);
    let text = text_of(&mut r);
    for p in &page.items {
        assert!(text.contains(&p.title) && text.contains(&tail(&p.id)), "{}: {text}", p.id);
    }
    assert!(text.contains(&orbiters_i18n::tr!("as {name}", name = r.w.state.me_name)), "{text}");
    r.w.state.open = None;
    let text = text_of(&mut r);
    assert!(page.items.iter().all(|p| text.contains(&tail(&p.id))), "from its id alone, with no page: {text}");
    // A picture: by its file's id.
    let mut r = rig();
    let op = lose(&mut r, Wrote::UploadGallery, true);
    r.w.check(&op);
    let text = text_of(&mut r);
    assert!(text.contains(tr("A picture")) && text.contains("…le_up1") && text.contains(tr("exactly what was sent")), "{text}");
    // VRChat refuses the reads, or there is no answer at all: the person still has a way out, for every kind.
    for kind in Wrote::ALL {
        let mut r = rig();
        let op = lose(&mut r, kind, false);
        r.world.lock().unwrap().refuses = true;
        r.w.check(&op);
        let text = text_of(&mut r);
        for want in [tr("I looked: it is there"), tr("I looked: it is not there"), tr("Drop it")] {
            assert!(text.contains(want), "{kind:?}: {text}");
        }
        decide(&mut r, &op, Choice::ShowsNot);
        let text = text_of(&mut r);
        assert!(text.contains(tr("Do it again…")) && text.contains(tr("Drop it")), "{kind:?}: {text}");
        decide(&mut r, &op, Choice::Drop);
        assert!(r.w.state.open_writes(G).is_empty() && !text_of(&mut r).contains(tr("Check VRChat")), "{kind:?}");
        assert_eq!(r.entry(&op).state, Stands::Unknown, "{kind:?}: no one said it was or was not done");
        assert_eq!(r.sent_writes(), 1);
    }
}

/// verify3 Major 1: an answer lost, and a record that is not there, or cannot be read, is never "it was not done": it stays
/// said, with a way out. Records that cannot be read are said, and stop what they may be of until set aside.
#[test]
fn a_missing_record_is_no_leave_to_send_again() {
    let mut r = rig();
    let write = r.write(Wrote::CreatePost);
    let op = r.w.confirm(write.clone()).op;
    r.between.lose(r#"{"type":"groups""#);
    r.w.send();
    let start = Instant::now();
    while r.between.lost() == 0 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(2));
    }
    // The service ends; what it wrote of it is cut short; another comes up.
    let book = r.service.ctl.groups_book_for_tests();
    while book.active(&op) && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(2));
    }
    r.between.hang_up();
    assert!(r.w.until(|s| s.flow.is_none()));
    book.let_go();
    let path = book.dir().join(format!("{op}.json"));
    let whole = std::fs::read(&path).unwrap();
    std::fs::write(&path, &whole[..60]).unwrap();
    let after = service();
    after.ctl.groups_keep_book_for_tests(book.dir());
    let sent = after.ctl.test_vrchat_routed(route(r.world.clone()));
    let (client, between) = linked_through(&after, Role::Window);
    r.w.ctl = Arc::new(Remote(client));
    let before = r.sent_writes();
    let world = r.world.clone();
    let w = std::mem::replace(&mut r.w, Window::new(Arc::new(crate::backend::Local::new(Controller::new(Settings::default()))), after.ctl.view()));
    let mut r = Rig { service: after, world, sent, between, w };
    r.w.state.ledger_due = true;
    r.w.state.ledger_at = 0.0;
    r.w.settle();
    assert_eq!((before, r.w.state.lost.len(), r.w.state.unreadable), (1, 1, 1), "still said, and the record with it");
    let ctl = r.w.ctl.clone();
    let panel = |ui: &mut Ui, _: &mut Cx, s: &mut State| {
        let ctx = ui.ctx().clone();
        uncertain_panel(ui, &ctx, &ctl, s, G);
    };
    let text = words(&r.w.drawn(2, vec![], panel));
    for want in [tr("Drop it"), tr("I looked: go on"), tr("Its answer was lost, and its record here cannot be read: look on vrchat.com before doing it again.")] {
        assert!(text.contains(want), "{want}: {text}");
    }
    r.w.prepare(write.clone());
    assert!(r.w.until(|s| s.flow.is_none()));
    assert_eq!(r.w.state.said, Some(failure_text(Failure::RecordUnreadable)));
    assert_eq!(r.sent_writes(), 0, "the next program sends nothing by itself");
    // The person looked, and goes on: kept aside, warned of.
    let (ctx, scope) = (r.w.ctx.clone(), Scope::of(&r.w.snap));
    r.w.state.jobs.ask(&ctx, &ctl, &scope, Kind::SetAside, Request::SetAside);
    r.w.settle();
    assert_eq!(r.w.state.unreadable, 0);
    assert_eq!(r.w.confirm(write).warning, Some(Warning::RecordUnreadable));
    assert!(path.with_extension("aside").exists());
}

/// verify3 Major 2: another Orbiters keeps the record: the window says so, and nothing is sent from here.
#[test]
fn another_program_on_the_record_is_said() {
    let mut r = rig();
    let write = r.write(Wrote::CreatePost);
    let owner = crate::controller::groups::Ledger::at(r.service.ctl.groups_book_for_tests().dir());
    r.service.ctl.groups_book_for_tests().let_go();
    assert!(owner.owned());
    r.w.state.ledger_due = true;
    r.w.state.ledger_at = 0.0;
    assert!(r.w.until(|s| s.elsewhere));
    r.w.prepare(write);
    assert!(r.w.until(|s| s.flow.is_none()));
    assert_eq!(r.w.state.said, Some(failure_text(Failure::OtherOwner)));
    assert_eq!(r.sent_writes(), 0);
    owner.let_go();
    r.w.state.ledger_at = 0.0;
    assert!(r.w.until(|s| !s.elsewhere), "and when it is gone, so is that");
}

/// "Do it again" is a new operation: sealed anew, warned of, shown for the person's yes; nothing goes by the press itself.
#[test]
fn again_is_a_new_confirmed_operation() {
    for kind in Wrote::ALL {
        let mut r = rig();
        let write = r.write(kind);
        let op = r.w.confirm(write).op;
        r.world.lock().unwrap().fault = Some(Fault::Drop { done: false });
        r.w.send();
        r.w.settle();
        r.later();
        r.w.check(&op);
        let (ctx, ctl) = (r.w.ctx.clone(), r.w.ctl.clone());
        if !matches!(r.w.state.observed.get(&op), Some(Observation::ShowsNot)) {
            r.w.state.decide(&ctx, &ctl, &op, Choice::ShowsNot);
            r.w.settle();
        }
        r.w.state.decide(&ctx, &ctl, &op, Choice::Again);
        r.w.settle();
        assert_eq!(r.sent_writes(), 1, "{kind:?}: nothing goes by the press");
        assert_eq!(r.entry(&op).closed, Some(crate::controller::groups::Closing::Again));
        if kind.tag().is_some() {
            assert!(r.w.state.flow.is_none() && r.w.state.said.is_some(), "{kind:?}: its bytes went with it: done again where it was made");
            continue;
        }
        assert!(r.w.until(|s| matches!(s.flow, Some(Flow::Confirming { .. }))), "{kind:?}");
        let sealed = r.w.shown();
        assert_eq!((sealed.op == op, sealed.warning), (false, Some(Warning::SameBefore)), "{kind:?}");
        r.w.send();
        r.w.settle();
        assert_eq!((r.sent_writes(), r.w.stands(&sealed.op)), (2, Some(Stands::Done)), "{kind:?}");
        // After a restart of the window, from the record alone where its ids say it all.
        let next = r.another();
        let known = next.state.again(&r.entry(&op)).is_some();
        assert_eq!(known, matches!(kind, Wrote::AssignRole | Wrote::UnassignRole | Wrote::DeleteRole | Wrote::ApplyBanner | Wrote::ApplyIcon), "{kind:?}");
    }
}

// ------------------------------------------------------------------------------------------------------------ the matrix

/// Where it goes wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    BeforeDispatch,
    ServiceReplyLost,
    VrchatReplyLost,
    Timeout,
    Status500,
    Status429,
    DecodeFailure,
    ContradictoryAnswer,
    PageClosed,
    ScopeChange,
    WindowClosed,
    AppRestart,
}
const POINTS: [Point; 12] = [Point::BeforeDispatch, Point::ServiceReplyLost, Point::VrchatReplyLost, Point::Timeout, Point::Status500, Point::Status429, Point::DecodeFailure, Point::ContradictoryAnswer, Point::PageClosed, Point::ScopeChange, Point::WindowClosed, Point::AppRestart];

/// What a cell came to.
struct Cell {
    state: Stands,
    sent: usize,
    identical: String,
    /// The very same, sealed by another window before this one went, sent by it after.
    second: String,
    check: String,
    decoys: String,
    /// What the person can do of it in the end.
    way_out: String,
}

/// One write, one failure point, done or not done at VRChat where the point allows either: the window's real state machine
/// and the service's real handler, over the real link.
fn cell(kind: Wrote, point: Point, done: bool) -> Cell {
    let mut r = rig();
    let write = r.write(kind);
    // The very same again, later: an upload's own bytes, staged anew.
    let bytes = match &write {
        Write::Upload { staged, .. } => Some(staged.read().unwrap()),
        _ => None,
    };
    let again = |w: &Write| match w {
        Write::Upload { tag, group, art, .. } => Write::Upload { tag: *tag, group: group.clone(), staged: stage_upload(bytes.as_ref().unwrap()).unwrap(), art: *art },
        other => other.clone(),
    };
    // Another window has the very same ready, unwarned, before this one goes.
    let mut other = r.another();
    let twin = other.confirm(again(&write));
    assert_eq!(twin.warning, None);
    let sealed = r.w.confirm(write.clone());
    let op = sealed.op.clone();
    r.w.settle();
    let hold = Arc::new(Hold::default());
    {
        let mut w = r.world.lock().unwrap();
        w.fault = match point {
            Point::BeforeDispatch | Point::ServiceReplyLost => None,
            Point::VrchatReplyLost => Some(Fault::Drop { done }),
            Point::Timeout | Point::AppRestart => Some(Fault::Late { done, hold: hold.clone() }),
            Point::Status500 => Some(Fault::Answer { done, status: 502, body: json!({}) }),
            Point::Status429 => Some(Fault::Answer { done, status: 429, body: json!({}) }),
            Point::DecodeFailure => Some(Fault::Garbage { done }),
            Point::ContradictoryAnswer => Some(Fault::Answer { done, status: 200, body: json!({"id": "gpos_other", "unrelated": true}) }),
            Point::PageClosed | Point::ScopeChange | Point::WindowClosed => Some(Fault::Held(hold.clone())),
        };
        if point == Point::BeforeDispatch {
            // The account loses what it needs between the seal and the send.
            w.rights = vec!["group-instance-moderate".into()];
            w.tags.clear();
        }
    }
    if point == Point::ServiceReplyLost {
        r.between.lose(r#"{"type":"groups""#);
    }
    if point == Point::PageClosed {
        r.w.state.open = Some(page::Open::new(card(), &r.w.state.drafts));
    }
    r.w.send();
    match point {
        Point::ServiceReplyLost => {
            let start = Instant::now();
            while r.between.lost() == 0 && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(2));
            }
            r.between.hang_up();
            assert!(r.w.until(|s| s.flow.is_none()));
            // What the window can say with no service: it may have been done.
            assert!(r.w.state.lost.iter().any(|l| l.op == op) && r.w.state.said.is_none(), "{kind:?}: lost, not refused");
            let client = crate::ipc::Client::connect(&r.service.endpoint_for_tests(), Role::Window).unwrap();
            r.w.ctl = Arc::new(Remote(client));
            r.w.settle();
        }
        Point::Timeout => {
            assert!(hold.until_reached());
            // No answer yet: it is being sent, to the ledger and to a check, and settles nothing.
            assert_eq!(r.entry(&op).state, Stands::Dispatched, "{kind:?}: on the disk as sent before its answer");
            let ctl = r.w.ctl.clone();
            r.w.state.check(&r.w.ctx, &ctl, &op);
            assert!(r.w.until(|s| s.observed.contains_key(&op)));
            assert_eq!(r.w.state.observed.get(&op), Some(&Observation::Running), "{kind:?}");
            hold.let_go();
        }
        Point::PageClosed => {
            assert!(hold.until_reached());
            r.w.state.close();
            assert!(r.w.until(|s| s.ledger.iter().any(|e| e.op == op && e.open())), "{kind:?}: still listed, being sent");
            hold.let_go();
        }
        Point::ScopeChange => {
            assert!(hold.until_reached());
            r.service.ctl.update(|s| s.vrchat_login.revision += 1);
            r.w.snap = r.service.ctl.view();
            r.w.frame();
            assert!(r.w.state.flow.is_none() && r.w.state.ledger.is_empty(), "{kind:?}: nothing of it for the next sign-in's view");
            hold.let_go();
        }
        Point::WindowClosed => {
            assert!(hold.until_reached());
            let next = r.another();
            let gone = std::mem::replace(&mut r.w, next);
            drop(gone);
            r.between.hang_up();
            r.w.settle();
            assert_eq!(r.w.stands(&op), Some(Stands::Dispatched), "{kind:?}: the next window lists it");
            hold.let_go();
        }
        Point::AppRestart => {
            assert!(hold.until_reached());
            // Both programs end here: what the disk holds now is all there is.
            let to = copied(&r.service.ctl.groups_book_for_tests().dir());
            hold.let_go();
            let after = service();
            after.ctl.groups_keep_book_for_tests(to);
            let sent = after.ctl.test_vrchat_routed(route(r.world.clone()));
            let (client, between) = linked_through(&after, Role::Window);
            let before = r.sent_writes();
            let w = Window::new(Arc::new(Remote(client)), after.ctl.view());
            // The old program's request was one; the new one sends none by itself.
            assert_eq!(before, 1);
            let world = r.world.clone();
            drop(std::mem::replace(&mut r, Rig { service: after, world, sent, between, w }));
            r.w.settle();
        }
        _ => {}
    }
    // Everything answers; and nothing is asked of VRChat by itself.
    r.w.settle();
    let start = Instant::now();
    while r.service.ctl.groups_book_for_tests().active(&op) && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(2));
    }
    r.w.state.ledger_due = true;
    r.w.settle();
    for _ in 0..20 {
        r.w.frame();
    }
    r.w.settle();
    let restarted = point == Point::AppRestart;
    let sent = r.sent_writes() + usize::from(restarted);
    let state = r.entry(&op).state;
    assert_eq!(r.w.stands(&op), Some(state), "{kind:?}, {point:?}: the window's view is the ledger");
    assert!(r.w.state.flow.is_none(), "{kind:?}, {point:?}");
    // The account has its rights again; VRChat's wait is over.
    {
        let mut w = r.world.lock().unwrap();
        w.rights = vec!["*".into()];
        w.tags = vec!["system_supporter".into()];
    }
    r.service.ctl.groups_forget_waits_for_tests();
    // The other window's Send on what it had ready.
    let second = if restarted {
        "gone with its service".to_string()
    } else {
        other.snap = r.service.ctl.view();
        other.frame();
        match &other.state.flow {
            Some(Flow::Confirming { .. }) => {
                let before = r.sent_writes();
                other.send();
                assert!(other.until(|s| s.flow.is_none()));
                let went = r.sent_writes() - before;
                match &other.state.said {
                    None => format!("went ({went})"),
                    Some(s) if *s == failure_text(Failure::Since) => format!("refused: look again ({went})"),
                    Some(s) if *s == failure_text(Failure::MaySent) => format!("refused: check first ({went})"),
                    Some(_) => format!("refused: it is so already, or changed ({went})"),
                }
            }
            _ => "ended with its sign-in".to_string(),
        }
    };
    let sent = sent + if second == "went (1)" { 1 } else { 0 };
    // The same again, by the window.
    r.w.prepare(again(&write));
    assert!(r.w.until(|s| !matches!(s.flow, Some(Flow::Preparing { .. }))));
    let identical = match &r.w.state.flow {
        Some(Flow::Confirming { sealed, .. }) => match sealed.warning {
            Some(w) => format!("confirmed, warned: {w:?}"),
            None => "confirmed".into(),
        },
        None if r.w.state.said == Some(failure_text(Failure::MaySent)) => "refused: check first".into(),
        None => "refused: it is so already, or changed".into(),
        _ => "?".into(),
    };
    r.w.state.end_flow();
    r.w.settle();
    assert_eq!(r.sent_writes() + usize::from(restarted), sent, "{kind:?}, {point:?}: nothing goes by itself");
    // What a check concludes, with the right evidence, and with every look-alike.
    let mut decoyed = String::from("—");
    if state == Stands::Unknown {
        r.later();
        let e = r.entry(&op).evidence;
        let list = decoys(kind, (e.md5, e.size));
        let n = list.len();
        let mut fooled = Vec::new();
        for (name, decoy) in list {
            let kept = {
                let w = r.world.lock().unwrap();
                (w.roles.clone(), w.members.clone(), w.posts.clone(), w.files.clone(), w.banner.clone(), w.icon.clone())
            };
            // On the state before the write, so that only the look-alike is seen.
            if done || restarted {
                let d = World::default();
                let mut w = r.world.lock().unwrap();
                (w.roles, w.members, w.posts, w.files, w.banner, w.icon) = (d.roles, d.members, d.posts, d.files, d.banner, d.icon);
            }
            decoy(&mut r.world.lock().unwrap());
            r.w.check(&op);
            let after = r.entry(&op);
            if !after.open() || after.answer == Some(Looked::Shows) {
                fooled.push(format!("{name} → {:?} {:?}", after.state, after.closed));
            }
            let mut w = r.world.lock().unwrap();
            (w.roles, w.members, w.posts, w.files, w.banner, w.icon) = kept;
            w.instead.clear();
        }
        decoyed = if fooled.is_empty() { format!("{n} of {n} close nothing") } else { format!("FOOLED: {}", fooled.join("; ")) };
    }
    r.w.check(&op);
    let check = match (r.entry(&op), r.w.state.observed.get(&op)) {
        (e, _) if e.state == Stands::Done => "answered: done".to_string(),
        (e, _) if e.state == Stands::Refused => "answered: refused, or not gone".to_string(),
        (e, _) if e.closed.is_some() => "VRChat shows it".to_string(),
        (_, Some(Observation::ShowsNot)) => "VRChat does not show it".to_string(),
        (_, Some(Observation::Candidates(l))) => format!("cannot tell ({} shown)", l.len()),
        (_, Some(Observation::CannotTell(u))) => format!("cannot tell ({u:?})"),
        (e, o) => format!("{:?} {o:?}", e.state),
    };
    // The way out, taken: what VRChat does not show is asked for again, as a new one; what it cannot tell is dropped.
    let (ctx, ctl) = (r.w.ctx.clone(), r.w.ctl.clone());
    let before = r.sent_writes();
    let way_out = match r.w.state.observed.get(&op) {
        _ if !r.entry(&op).open() => "—".to_string(),
        Some(Observation::ShowsNot) => {
            r.w.state.decide(&ctx, &ctl, &op, Choice::Again);
            r.w.settle();
            r.w.prepare(again(&write));
            assert!(r.w.until(|s| !matches!(s.flow, Some(Flow::Preparing { .. }))));
            match &r.w.state.flow {
                Some(Flow::Confirming { sealed, .. }) if sealed.op != op && sealed.confirm => format!("again, confirmed ({:?})", sealed.warning),
                _ => format!("again: {:?}", r.w.state.said),
            }
        }
        Some(_) => {
            r.w.state.decide(&ctx, &ctl, &op, Choice::Drop);
            r.w.settle();
            "dropped".to_string()
        }
        None => "none".to_string(),
    };
    r.w.state.end_flow();
    r.w.settle();
    assert!(!r.entry(&op).open(), "{kind:?}, {point:?}: it can be left");
    assert!(r.entry(&op).state == state, "{kind:?}, {point:?}: what became of it is said by no one");
    assert_eq!(r.sent_writes(), before, "{kind:?}, {point:?}: nothing goes by a decision");
    Cell { state, sent, identical, second, check, decoys: decoyed, way_out }
}

/// Every write and every failure point, generated: the ledger's final state, one request at most, nothing by itself, the
/// same not sent again unseen, and what a check concludes from the right evidence and from every look-alike. Prints the
/// table (`--nocapture`).
#[test]
fn the_matrix_of_every_write_and_every_failure() {
    let cells: Vec<(Wrote, Point, bool)> = Wrote::ALL
        .into_iter()
        .flat_map(|k| POINTS.into_iter().flat_map(move |p| {
            // Where VRChat may have done it or not, both.
            let both = matches!(p, Point::VrchatReplyLost | Point::Timeout | Point::Status500 | Point::Status429 | Point::DecodeFailure | Point::ContradictoryAnswer | Point::AppRestart);
            let dones: &[bool] = if both { &[true, false] } else if p == Point::BeforeDispatch { &[false] } else { &[true] };
            dones.iter().map(move |d| (k, p, *d)).collect::<Vec<_>>()
        }))
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let rows: Mutex<Vec<(usize, String, Vec<String>)>> = Mutex::default();
    std::thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((kind, point, done)) = cells.get(i).copied() else { return };
                let c = std::panic::catch_unwind(|| cell(kind, point, done));
                let (row, wrong) = match c {
                    Err(e) => {
                        let why = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                        (format!("| {kind:?} | {point:?} | {done} | PANIC | | | {why} | |"), vec![why])
                    }
                    Ok(c) => {
                        let want_state = match point {
                            Point::BeforeDispatch => Stands::Refused,
                            Point::ServiceReplyLost | Point::PageClosed | Point::ScopeChange | Point::WindowClosed => Stands::Done,
                            _ => Stands::Unknown,
                        };
                        // Before the dispatch, it is the other window's that went.
                        let want_sent = 1;
                        let mut wrong = Vec::new();
                        if c.state != want_state {
                            wrong.push(format!("state {:?}, wanted {want_state:?}", c.state));
                        }
                        if c.sent != want_sent {
                            wrong.push(format!("{} requests, wanted {want_sent}", c.sent));
                        }
                        // What the other window had ready: it went only where this one never did.
                        let second_ok = match (point, want_state) {
                            (Point::AppRestart, _) => c.second == "gone with its service",
                            (Point::ScopeChange, _) => c.second == "ended with its sign-in",
                            (_, Stands::Refused) => c.second == "went (1)",
                            _ => c.second.starts_with("refused") && c.second.ends_with("(0)"),
                        };
                        if !second_ok {
                            wrong.push(format!("the other window: {}", c.second));
                        }
                        let same = match want_state {
                            Stands::Unknown => c.identical == "refused: check first" || (done && c.identical == "refused: it is so already, or changed"),
                            Stands::Done if kind.makes() => c.identical == "confirmed, warned: AlreadyDone",
                            Stands::Done => c.identical == "refused: it is so already, or changed",
                            // The other window's went: so is it here.
                            _ if kind.makes() => c.identical == "confirmed, warned: AlreadyDone",
                            _ => c.identical == "refused: it is so already, or changed",
                        };
                        // An apply of the picture that is there already is harmless; it is sealed and confirmed like any.
                        let reapply = kind.slot().is_some() && want_state != Stands::Unknown && c.identical == "confirmed";
                        if !same && !reapply {
                            wrong.push(format!("the same again: {}", c.identical));
                        }
                        let want_check = match (want_state, kind, done) {
                            (Stands::Done, ..) => "answered: done",
                            (Stands::Refused, ..) => "answered: refused, or not gone",
                            (_, Wrote::DeletePost, true) => "cannot tell (NotSeen)",
                            (_, k, true) if k.makes() => "cannot tell (1 shown)",
                            (_, k, false) if k.makes() => "cannot tell (0 shown)",
                            (_, _, true) => "VRChat shows it",
                            _ => "VRChat does not show it",
                        };
                        let want_out = match want_check {
                            "VRChat does not show it" => "again, confirmed (Some(SameBefore))",
                            w if w.starts_with("cannot tell") => "dropped",
                            _ => "—",
                        };
                        if c.way_out != want_out {
                            wrong.push(format!("the way out: {}, wanted {want_out}", c.way_out));
                        }
                        if c.check != want_check {
                            wrong.push(format!("check: {}, wanted {want_check}", c.check));
                        }
                        if c.decoys.contains("FOOLED") {
                            wrong.push(c.decoys.clone());
                        }
                        let by_vrchat = if point == Point::BeforeDispatch { "—" } else if done { "done" } else { "not done" };
                        (format!("| {kind:?} | {point:?} | {by_vrchat} | {:?} | {} | {} | {} | {} | {} | {} |", c.state, c.sent, c.second, c.identical, c.check, c.decoys, c.way_out), wrong)
                    }
                };
                rows.lock().unwrap().push((i, row, wrong));
            });
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|r| r.0);
    println!("MATRIX {} cells", rows.len());
    println!("| Write | Failure point | VRChat did it | Ledger | Requests | The same, ready in another window | The same again | Check | Check, look-alikes | The person's way out |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (_, row, wrong) in &rows {
        println!("{row}{}", if wrong.is_empty() { String::new() } else { format!(" **{}**", wrong.join("; ")) });
    }
    let failed: Vec<String> = rows.iter().filter(|r| !r.2.is_empty()).map(|r| format!("{} {}", r.1, r.2.join("; "))).collect();
    println!("MATRIX FAILURES {}", failed.len());
    assert_eq!(rows.len(), cells.len());
    assert!(failed.is_empty(), "{} of {} cells:\n{}", failed.len(), rows.len(), failed.join("\n"));
}

