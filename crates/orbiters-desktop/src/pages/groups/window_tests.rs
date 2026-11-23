//! The window's groups state machine driven for real (verify-r22: "verified through the desktop window and service"):
//! the window's `State` with its jobs, through a real link to a real service and its handler, with only VRChat's
//! transport scripted. A link proxy (`link_between`) loses the service's reply where a test needs it lost.
use super::*;
use crate::backend::{Ctl, Remote};
use crate::controller::groups::{Request, Write};
use crate::ipc::Role;
use crate::service::tests::{linked_through, service};
use crate::state::Snapshot;
use orbiters_core::groups::PostDraft;
use orbiters_core::vrchat_api::Scripted;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const G: &str = "grp_00000000-0000-4000-8000-0000000000aa";

fn group() -> Value {
    json!({"id": G, "name": "Made-up Makers", "ownerId": "usr_test", "memberCount": 3,
        "myMember": {"id": "gmem_me", "userId": "usr_test", "roleIds": ["grol_owner"], "permissions": ["*"], "membershipStatus": "member"},
        "roles": [{"id": "grol_owner", "name": "Owner", "order": 0, "permissions": ["*"], "defaultRole": false, "isManagementRole": true, "requiresPurchase": false}]})
}
fn ok(v: Value) -> Scripted {
    Scripted::Answer(200, v)
}

/// A window's state over `ctl`, signed in as the service's controller is.
struct Window {
    state: State,
    ctx: egui::Context,
    ctl: Ctl,
    snap: Arc<Snapshot>,
    toasts: orbiters_ui::toast::Toasts,
}

impl Window {
    fn new(ctl: Ctl, snap: Arc<Snapshot>) -> Self {
        let mut state = State::default();
        state.scope = Some(Scope::of(&snap));
        Self { state, ctx: egui::Context::default(), ctl, snap, toasts: Default::default() }
    }
    /// Takes the answers that came back, as a frame does.
    fn frame(&mut self) {
        let snap = self.snap.clone();
        let mut cx = Cx { ctl: &self.ctl, snap: &snap, toasts: &mut self.toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
        self.state.take(&self.ctx, &mut cx);
    }
    /// Frames until `done`, a few seconds at most.
    fn until(&mut self, done: impl Fn(&State) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            self.frame();
            if done(&self.state) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
    fn prepare(&mut self, write: Write) {
        let ctl = self.ctl.clone();
        self.state.prepare(&self.ctx, &ctl, write, Rect::NOTHING);
    }
    /// The person's Send on the confirmation shown.
    fn send(&mut self) {
        let Some(Flow::Confirming { prepared, .. }) = self.state.flow.take() else { panic!("no confirmation") };
        let ctl = self.ctl.clone();
        self.state.commit(&self.ctx, &ctl, prepared);
    }
}

fn post() -> Write {
    Write::CreatePost { group: G.into(), draft: PostDraft { title: "Meet-up".into(), text: "Saturday".into(), ..Default::default() } }
}

/// Blocker 1: the service publishes the post, its reply is lost on the link and the link drops. The window says
/// "unknown" (never "refused, try again"), refuses to prepare the same post again, and a check from a new link finds
/// the service's record: done. VRChat got one post.
#[test]
fn a_lost_reply_is_unknown_and_recovered_through_the_link() {
    let service = service();
    let sent = service.ctl.test_vrchat_scripted(vec![
        ok(group()), // prepare
        ok(group()), // commit's rules
        ok(json!({"id": "gpos_1", "groupId": G, "authorId": "usr_test", "title": "Meet-up", "text": "Saturday", "visibility": "group"})),
    ]);
    let snap = service.ctl.view();
    let (client, between) = linked_through(&service, Role::Window);
    let mut w = Window::new(Arc::new(Remote(client)), snap.clone());
    w.prepare(post());
    assert!(w.until(|s| matches!(s.flow, Some(Flow::Confirming { .. }))), "the confirmation shows");
    let op = match &w.state.flow {
        Some(Flow::Confirming { prepared, .. }) => prepared.op.clone(),
        _ => unreachable!(),
    };
    between.lose(r#"{"type":"groups""#);
    w.send();
    // The service sends the post; its answer goes missing; then the link drops.
    let start = Instant::now();
    while between.lost() == 0 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(between.lost(), 1, "the reply was lost on the link");
    between.hang_up();
    assert!(w.until(|s| s.flow.is_none()), "the window takes the end of the link");
    assert_eq!(w.state.drafts.uncertain.iter().map(|u| u.op.clone()).collect::<Vec<_>>(), [op.clone()], "the post is uncertain, not refused");
    assert_ne!(w.state.said, Some(failure_text(Failure::Api(GroupFailure::Unavailable))));
    // The same post is not prepared again before its check.
    w.prepare(post());
    assert!(w.state.flow.is_none(), "nothing prepared");
    assert_eq!(w.state.said, Some(failure_text(Failure::CheckFirst)));
    // A new link (the window reconnected) checks: the service kept the outcome.
    let client = crate::ipc::Client::connect(&service.endpoint_for_tests(), Role::Window).unwrap();
    w.ctl = Arc::new(Remote(client));
    let ctl = w.ctl.clone();
    w.state.check(&w.ctx, &ctl, &op);
    assert!(w.until(|s| s.drafts.uncertain.is_empty()), "the check settles it");
    let posts = sent.lock().unwrap().iter().filter(|s| s.method == "POST").count();
    assert_eq!(posts, 1, "VRChat got one post");
}

/// Blocker 6: a held Enter (key repeats) on the focused Send of a real confirmation dialog sends nothing; a fresh
/// press sends it.
#[test]
fn a_held_key_never_sends_a_confirmation() {
    use crate::controller::groups::{Prepared, Write};
    let ctl: Ctl = Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
    let snap = ctl.view();
    let mut w = Window::new(ctl.clone(), snap.clone());
    let prepared = Prepared { op: "op-1".into(), write: Write::DeleteRole { group: G.into(), role: "grol_a".into() }, confirm: true, role: None, post: None, member: None };
    stage::confirming(&mut w.state, prepared, Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(80.0, 30.0)));
    orbiters_ui::widgets::watch_controls();
    let mut toasts = orbiters_ui::toast::Toasts::default();
    let mut frame = |w: &mut Window, events: Vec<egui::Event>, t: f64| {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))), time: Some(t), ..Default::default() };
        input.events = events;
        let state = &mut w.state;
        let ctl = ctl.clone();
        let snap = snap.clone();
        let toasts = &mut toasts;
        let mut out = w.ctx.run_ui(input, move |ui| {
            let mut cx = Cx { ctl: &ctl, snap: &snap, toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
            confirmation(ui, &mut cx, state);
        });
        out.textures_delta.clear();
    };
    let mut t = 0.0;
    for _ in 0..90 {
        frame(&mut w, vec![], t);
        t += 1.0 / 60.0;
    }
    let (send, _) = orbiters_ui::widgets::control(&w.ctx, "Delete the role").expect("the Send button is drawn");
    // Enter goes down before Send has the focus (held on the press that opened the confirmation): egui counts what
    // follows as its repeats.
    let enter = |pressed: bool| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    frame(&mut w, vec![enter(true)], t);
    t += 1.0 / 60.0;
    assert!(matches!(w.state.flow, Some(Flow::Confirming { .. })));
    // Tab until Send has the keyboard focus.
    let tab = egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
    let mut focused = false;
    for _ in 0..12 {
        frame(&mut w, vec![tab.clone()], t);
        t += 1.0 / 60.0;
        let on_send = w.ctx.memory(|m| m.focused()).and_then(|id| w.ctx.read_response(id)).is_some_and(|r| r.rect == send);
        if on_send {
            focused = true;
            break;
        }
    }
    assert!(focused, "Send takes the focus");
    frame(&mut w, vec![enter(true)], t);
    frame(&mut w, vec![enter(true), enter(true)], t + 0.02);
    assert!(matches!(w.state.flow, Some(Flow::Confirming { .. })), "a repeating key sends nothing");
    frame(&mut w, vec![enter(false)], t + 0.04);
    assert!(matches!(w.state.flow, Some(Flow::Confirming { .. })), "nor its release");
    frame(&mut w, vec![enter(true)], t + 0.06);
    assert!(matches!(w.state.flow, Some(Flow::Sending { .. })), "a fresh press sends it");
}

/// Major 1: an upload of the largest size accepted crosses the real link as a small message (its PNG is staged on
/// disk), one byte more is refused before anything is sent, and the link stays up for the next request.
#[test]
fn an_upload_at_the_size_limits_keeps_the_link_up() {
    let service = service();
    let _sent = service.ctl.test_vrchat_scripted(vec![ok(json!({"id": "usr_test", "tags": ["system_supporter"]})), ok(json!({}))]);
    let snap = service.ctl.view();
    let (client, _between) = linked_through(&service, Role::Window);
    let ctl: Ctl = Arc::new(Remote(client));
    let max = orbiters_core::vrchat_api::GROUP_UPLOAD_MAX;
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(max, 7);
    let staged = stage_upload(&png).unwrap();
    let message = serde_json::to_vec(&crate::ipc::ToService::Groups { id: "x".into(), scope: Scope::of(&snap), request: Request::Prepare { write: Write::Upload { tag: FileTag::Gallery, group: G.into(), staged: staged.clone() } } }).unwrap();
    assert!(message.len() < 4096, "the link carries a reference, never the picture: {} bytes", message.len());
    let reply = ctl.groups(Scope::of(&snap), Request::Prepare { write: Write::Upload { tag: FileTag::Gallery, group: G.into(), staged } });
    assert!(matches!(reply, Reply::Prepared(_)), "the largest picture is prepared: {reply:?}");
    let mut over = png.clone();
    over.push(0);
    let staged = stage_upload(&over).unwrap();
    assert_eq!(ctl.groups(Scope::of(&snap), Request::Prepare { write: Write::Upload { tag: FileTag::Gallery, group: G.into(), staged } }), Reply::Failed(Failure::TooLarge));
    let discovered = ctl.groups(Scope::of(&snap), Request::Discover);
    assert!(!matches!(discovered, Reply::Failed(Failure::Lost)), "the link is still up: {discovered:?}");
}

/// Major 9: the service takes in a few groups requests at once, before starting a thread for any; the rest are told to
/// wait at once. The admitted ones (checks, which never wait behind reads inside the controller) are held inside
/// VRChat's (scripted) transport until the others have their answer.
#[test]
fn the_service_admits_a_few_requests_at_once() {
    use std::sync::{mpsc, Condvar, Mutex};
    let service = service();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let held = (0..12).map(|_| {
        let gate = gate.clone();
        Scripted::During(
            Box::new(move || {
                let (open, cv) = &*gate;
                let _ = cv.wait_timeout_while(open.lock().unwrap(), Duration::from_secs(10), |open| !*open);
            }),
            200,
            json!([]),
        )
    });
    let _sent = service.ctl.test_vrchat_scripted(held.collect());
    let snap = service.ctl.view();
    let (client, _between) = linked_through(&service, Role::Window);
    let ctl: Ctl = Arc::new(Remote(client));
    let (tx, rx) = mpsc::channel();
    let threads: Vec<_> = (0..12)
        .map(|i| {
            let (ctl, scope, tx) = (ctl.clone(), Scope::of(&snap), tx.clone());
            std::thread::spawn(move || tx.send(ctl.groups(scope, Request::Check { op: format!("op-{i}"), write: post() })).unwrap())
        })
        .collect();
    let mut replies = Vec::new();
    let start = Instant::now();
    while replies.len() < 12 - crate::controller::groups::ADMIT_MAX && start.elapsed() < Duration::from_secs(5) {
        if let Ok(r) = rx.recv_timeout(Duration::from_millis(50)) {
            replies.push(r);
        }
    }
    let entered = service.ctl.groups_entered();
    let busy = replies.iter().filter(|r| **r == Reply::Failed(Failure::Busy)).count();
    let (open, cv) = &*gate;
    *open.lock().unwrap() = true;
    cv.notify_all();
    for t in threads {
        t.join().unwrap();
    }
    let rest: Vec<Reply> = rx.try_iter().collect();
    assert!(entered <= crate::controller::groups::ADMIT_MAX as u64, "{entered} reached the controller: admission comes first ({replies:?}, then {rest:?})");
    assert_eq!(busy, 12 - crate::controller::groups::ADMIT_MAX, "told to wait while the others run: {replies:?}");
}

/// Blocker 1: leaving the page (or the account) while a write is being sent drops its answer with the jobs; the write is
/// listed as uncertain, to check, never forgotten.
#[test]
fn leaving_while_a_write_is_sent_keeps_it_to_check() {
    let ctl: Ctl = Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
    let mut snap = (*ctl.view()).clone();
    snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_00000000-0000-4000-8000-0000000000e2".into(), ..Default::default() });
    let mut w = Window::new(ctl, Arc::new(snap));
    let prepared = Prepared { op: "op-sent".into(), write: post(), confirm: true, role: None, post: None, member: None };
    w.state.flow = Some(Flow::Sending { prepared });
    w.state.close();
    assert!(w.state.flow.is_none());
    assert_eq!(w.state.uncertain_of(G).iter().map(|u| u.op.as_str()).collect::<Vec<_>>(), ["op-sent"], "still to check");
    let _ = std::fs::remove_file(art::drafts_file("usr_00000000-0000-4000-8000-0000000000e2").unwrap());
}

/// Major 8: an unknown post's uncertainty, the post being written and a picture uploaded and not applied are the
/// account's own: closed and opened again (another State, as after a restart), they are back.
#[test]
fn drafts_and_uncertain_writes_come_back() {
    let ctl: Ctl = Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
    let mut snap = (*ctl.view()).clone();
    snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_00000000-0000-4000-8000-0000000000e1".into(), ..Default::default() });
    let snap = Arc::new(snap);
    let mut w = Window::new(ctl.clone(), snap.clone());
    let card = GroupCard { id: G.into(), name: "Made-up Makers".into(), ..Default::default() };
    w.state.open = Some(page::Open::new(card.clone(), &w.state.drafts));
    w.state.open.as_mut().unwrap().composer = Some(page::Composer { baseline: None, draft: PostDraft { title: "Half".into(), text: "written".into(), ..Default::default() } });
    w.state.add_uncertain("op-9".into(), post());
    w.state.drafts.set_uploaded(art::UploadedRef { group: G.into(), slot: orbiters_core::groups::ArtSlot::Banner, file: orbiters_core::groups::InventoryFile { id: "file_up".into(), url: String::new() }, revision: None });
    w.state.close();
    let mut again = Window::new(ctl, snap);
    again.state.drafts = art::load_drafts("usr_00000000-0000-4000-8000-0000000000e1");
    assert_eq!(again.state.drafts.uncertain.len(), 1, "the uncertain post is still to check");
    assert!(again.state.drafts.uploaded(G, orbiters_core::groups::ArtSlot::Banner).is_some(), "the upload not applied is still there");
    let open = page::Open::new(card, &again.state.drafts);
    assert_eq!(open.composer.as_ref().map(|c| c.draft.title.as_str()), Some("Half"), "the post being written is back");
    let _ = std::fs::remove_file(art::drafts_file("usr_00000000-0000-4000-8000-0000000000e1").unwrap());
}
