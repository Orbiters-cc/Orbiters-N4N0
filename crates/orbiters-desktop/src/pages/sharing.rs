//! One sharing surface for Photos & captures and Visited worlds: the website's picture window (its gallery and
//! capture windows). It grows out of the card that opened it ([`orbiters_ui::viewer`]), the picture large and sized
//! to its shape, the details and compact actions under it (beside tall pictures): share publicly, share to friends
//! (a picker of chips), who it is shared with (chips with an ×), the explicit warning, and what a share is doing,
//! did or could not do. Panels slide in under the actions; the window follows their height on the same spring.
use super::{Cx, Job};
use crate::controller::shares::{self as controller, Entry, Operation as Action, Outcome, Request, Stage};
use egui::{pos2, vec2, Align2, Color32, Id, Order, Rect, Sense, Ui, UiBuilder};
use orbiters_core::{
    account::Server,
    shares::{self, Share, Shared, Unshare},
    vrchat_accounts::Friend,
};
use orbiters_ui::{
    anim::{self, Spring},
    feedback, icons, morph,
    theme::{self, weight, Txt},
    viewer::{self, Arrangement, Morph},
    widgets::{self, Button, Chip, Size, Tone, Variant},
};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

/// Use the shared press activation, including the normal keyboard/accessibility click fallback.
pub fn pressed(response: egui::Response) -> bool {
    response.enabled() && widgets::activated(&response)
}

/// Every opening gets springs of its own.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// Friends the picker shows at once; the search finds the others.
const FRIEND_ROWS: usize = 3;
/// Space around the window inside the page.
const GUTTER: f32 = 24.0;
const PAD: f32 = 24.0;

/// The card a window grew out of, drawn by its page's own code under the window while the window leaves it and lands
/// back on it ([`morph::ride`]): where the card sits, its picture in it, and the card's drawing (laid out at `home`,
/// taking no input).
pub struct Source {
    pub home: Rect,
    pub picture: Rect,
    pub paint: Box<dyn Fn(&mut Ui, &Cx)>,
}

pub struct Dialog {
    pub entry: Entry,
    /// The card it came out of, when its page draws it under the window (set every frame; None when it scrolled away).
    pub source: Option<Source>,
    /// Its card is gone (scrolled away, removed): it closes into the middle of the page, fading.
    lost: bool,
    /// Landed back on its card on the last frame drawn: the page shows its card again.
    finished: bool,
    pub extra: Vec<Entry>,
    pub picker: bool,
    pub warning: Option<String>,
    friends: Job<Result<Vec<Friend>, String>>,
    friend_scope: Option<crate::controller::vrchat_data::Scope>,
    friends_requested: bool,
    friends_list: Vec<Friend>,
    friends_error: Option<String>,
    query: String,
    selected: BTreeSet<String>,
    message: String,
    operation_id: Option<String>,
    backend: Option<crate::backend::Ctl>,
    pending: Option<Request>,
    last: Option<Action>,
    error: Option<String>,
    notice: Option<String>,
    fixture_busy: bool,
    fixture_active: bool,
    rollback: Option<Entry>,
    /// A share sent and not answered yet: the friends (and Everyone) shown at once, marked on their way.
    on_its_way: BTreeSet<String>,
    public_on_its_way: bool,
    /// Where the running share stands, as the tray last said.
    stage: Stage,
    /// The capture's upload files asked to be made while friends are chosen.
    prepared_ahead: bool,
    request_public: bool,
    /// Copy public link was pressed while the capture was not public (or had no picture on Orbiters yet): its link is
    /// copied once the public share is done.
    copy_after: bool,
    /// When its public link was copied: the button says so for a moment, a ring leaving it.
    copied_at: Option<f64>,
    /// The picture shown whole over the window, its details folded away (a click on the picture).
    full: bool,
    pub changed: bool,
    pictures: crate::worlds::Pictures,
    viewer: Option<super::splats::Viewer>,
    /// The photo editor on a copy of the picture ([`super::editor`]), over the page while it is open.
    editor: Option<super::editor::Editor>,
    /// Where it opens from (a card's picture) and that rect's corner radius; the middle of the page when not given.
    origin: Option<(Rect, f32)>,
    morph: Option<Morph>,
    /// Details' heights measured at a width, for the next frame's layout.
    measured: HashMap<u32, f32>,
    /// The panel under the actions and the one before it (they slide past each other).
    panel_key: (u8, u64),
    /// Going to the next or previous item in the window: how many times, and which way (-1 or 1).
    nav: (u64, f32),
    /// The picture the window showed last, kept to slide away as another comes in.
    last_picture: Option<egui::TextureHandle>,
    previous: Option<egui::TextureHandle>,
    /// Whether the window has items before and after this one (the gallery's arrows), and which one was asked.
    pub neighbours: (bool, bool),
    pub step: Option<i64>,
    /// A multi-layer picture's layers: which one the window is about, how they are shown ([`super::gallery_layers`]).
    pub layers: super::gallery_layers::LayerView,
    /// Its page drags pictures out of the window (Photos & captures): the picture senses drags
    /// ([`super::gallery_select::WindowDrag`]: a press that moves is a drag out, a click still shows it whole).
    pub drag_out: bool,
    /// The picture's press became a drag out this frame: the page hands its file to Windows.
    pub dragged: bool,
    /// The page carries the picture out of the window: drawn lifted until the drop.
    pub lifted: bool,
    drag: super::gallery_select::WindowDrag,
}

/// What sits under the actions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Picker,
    Warning,
}

impl Dialog {
    pub fn new(ctx: &egui::Context, entry: Entry, picker: bool) -> Self {
        Self {
            entry,
            source: None,
            lost: false,
            finished: false,
            extra: Vec::new(),
            picker,
            warning: None,
            friends: Job::default(),
            friend_scope: None,
            friends_requested: false,
            friends_list: Vec::new(),
            friends_error: None,
            query: String::new(),
            selected: BTreeSet::new(),
            message: String::new(),
            operation_id: None,
            backend: None,
            pending: None,
            last: None,
            error: None,
            notice: None,
            fixture_busy: false,
            fixture_active: false,
            rollback: None,
            on_its_way: BTreeSet::new(),
            public_on_its_way: false,
            stage: Stage::default(),
            prepared_ahead: false,
            request_public: false,
            copy_after: false,
            copied_at: None,
            full: false,
            changed: false,
            pictures: crate::worlds::Pictures::new(ctx),
            viewer: None,
            editor: None,
            origin: None,
            morph: None,
            measured: HashMap::new(),
            panel_key: (0, 0),
            nav: (0, 0.0),
            last_picture: None,
            previous: None,
            neighbours: (false, false),
            step: None,
            layers: Default::default(),
            drag_out: false,
            dragged: false,
            lifted: false,
            drag: Default::default(),
        }
    }

    /// Opens out of `origin` (a card's picture whose corners have `radius`), and closes back into it.
    pub fn from(mut self, origin: Rect, radius: f32) -> Self {
        self.origin = Some((origin, radius));
        self
    }

    /// The card it closes into, once it shows another item (or the grid moved).
    pub fn set_origin(&mut self, origin: Rect) {
        if let Some(m) = self.morph.as_mut() {
            m.set_origin(origin);
        }
        self.origin = self.origin.map(|(_, r)| (origin, r));
        self.lost = false;
    }

    /// Its card has nowhere to be seen: it closes into the middle of the page instead, fading out.
    pub fn lose_origin(&mut self) {
        self.lost = true;
        self.source = None;
    }

    /// Whether its card is under the window (the window draws it while it leaves and lands, [`Dialog::source`]): the
    /// page leaves it out until the window landed back on it.
    pub fn hides_source(&self) -> bool {
        self.morph.as_ref().is_some_and(|m| m.id() != Id::NULL) && !self.finished
    }

    /// Starts closing back into its card ([`Dialog::show`] says when it is gone).
    pub fn close(&mut self) {
        match self.morph.as_mut() {
            Some(m) => m.close(),
            None => self.morph = Some(Morph::new(Id::NULL, Rect::NOTHING, 0.0)),
        }
    }

    /// Shows `entry` in the open window instead, sliding in from the side it is on (`dir` 1: the next one).
    pub fn navigate(&mut self, entry: Entry, dir: f32) {
        self.previous = self.last_picture.take();
        self.entry = entry;
        self.picker = false;
        self.warning = None;
        self.notice = None;
        self.error = None;
        self.last = None;
        self.rollback = None;
        self.on_its_way.clear();
        self.public_on_its_way = false;
        self.prepared_ahead = false;
        self.request_public = false;
        self.selected.clear();
        self.message.clear();
        self.query.clear();
        self.nav = (self.nav.0 + 1, dir);
    }

    /// Shows one of a multi-layer picture's layers instead, or the picture again ([`super::gallery_layers::LayerView`]):
    /// the window is about that file now (its sharing, its folder), over the same picture; what was being chosen for
    /// the other one is let go.
    fn show_layer(&mut self, entry: Entry) {
        self.entry = entry;
        self.picker = false;
        self.warning = None;
        self.notice = None;
        self.error = None;
        self.last = None;
        self.rollback = None;
        self.on_its_way.clear();
        self.public_on_its_way = false;
        self.request_public = false;
        self.selected.clear();
    }

    /// Chooses one of a multi-layer picture's layers (None: the picture), as its strip does (screenshots).
    pub fn choose_layer(&mut self, choice: Option<usize>) {
        if let Some(entry) = self.layers.choose(&self.entry, choice) {
            self.show_layer(entry);
        }
    }

    /// Whether another item may take this one's place now (nothing running, nothing being typed or asked).
    pub fn can_navigate(&self) -> bool {
        self.operation_id.is_none() && self.warning.is_none() && !self.picker && self.morph.as_ref().is_some_and(|m| !m.is_closing())
    }

    /// Opens the photo editor on a copy of this picture (a multi-layer picture with a stack of effects for each of its
    /// layers, whichever layer the window shows).
    pub fn edit(&mut self, ctx: &egui::Context) {
        let entry = self.layers.layered(&self.entry);
        if let Some(source) = super::editor::source_of(entry) {
            self.editor = Some(super::editor::Editor::open(ctx, source, entry.world_name.clone()));
        }
    }

    /// The photo editor, open while it is there (screenshots stage it).
    pub fn editor_slot(&mut self) -> &mut Option<super::editor::Editor> {
        &mut self.editor
    }

    pub fn loading(&self) -> bool {
        self.pictures.loading()
    }

    pub fn fixture(&mut self, state: &str) {
        self.fixture_active = true;
        if state.starts_with("friends") {
            self.picker = true;
            self.friends = Job::default();
            self.friends_list = ["Alex", "Mika", "Robin", "Sora", "Taylor", "Kasumi", "Juniper", "Lyra Nightfall", "Bob the Builder"]
                .iter()
                .enumerate()
                .map(|(i, n)| Friend { id: format!("usr_{i}"), name: (*n).into(), image: None })
                .collect();
            self.selected.insert("usr_0".into());
            self.selected.insert("usr_2".into());
            self.message = "A little moment from last night ✨".into();
            if state == "friends-error" {
                self.friends_list.clear();
                self.selected.clear();
                self.message.clear();
                self.friends_error = Some("VRChat is busy. Please try again shortly.".into());
            }
        }
        if state == "explicit" {
            self.warning = Some(shares::EXPLICIT_WARNING.into());
            if self.entry.content.explicit_reasons.is_empty() {
                self.entry.content.explicit_reasons = vec!["osc-parameter:NSFW/Toggle".into(), "avatar-label:sexually_explicit".into()];
            }
        }
        if state == "progress" {
            self.fixture_busy = true;
            let action = Action::Share(Share { public: true, ..Default::default() });
            self.optimistic(&action);
            self.last = Some(action);
            self.stage = Stage::Preparing;
        }
        if state == "sending" {
            self.fixture_busy = true;
            let recipients = vec![shares::Recipient { vrchat_user_id: "usr_0".into(), display_name: "Alex".into() }];
            let action = Action::Share(Share { recipients, ..Default::default() });
            self.optimistic(&action);
            self.last = Some(action);
            self.stage = Stage::Sending { sent: 12_400_000, total: 31_600_000 };
        }
        if state == "picture" {
            self.full = true;
        }
        if state == "copy-link" {
            if let Some(item) = self.entry.remote.as_mut() {
                item.public = true;
            }
        }
        if state == "error" {
            self.error = Some("Orbiters could not be reached. Your capture is still on this computer.".into());
            self.last = Some(Action::Share(Share { public: true, ..Default::default() }));
        }
        if state == "done" {
            self.notice = Some("Shared publicly: it is on Orbiters now.".into());
            if let Some(item) = self.entry.remote.as_mut() {
                item.public = true;
            }
        }
    }

    pub fn public(&mut self, ctx: &egui::Context, server: Server) {
        if self.entry.content.explicit || self.extra.iter().any(|e| e.content.explicit) {
            self.warning = Some(shares::EXPLICIT_WARNING.into());
        } else {
            self.start(ctx, server, Action::Share(Share { public: true, ..Default::default() }));
        }
    }

    fn start(&mut self, _ctx: &egui::Context, server: Server, action: Action) {
        if self.operation_id.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.last = Some(action.clone());
        let entry = self.entry.clone();
        let extra = self.extra.clone();
        // Optimistic revoke: disable actions immediately; restore the saved entry on failure.
        if let Action::Unshare(r) = &action {
            self.rollback = Some(self.entry.clone());
            if let Some(i) = self.entry.remote.as_mut() {
                i.revoke(r);
            }
        }
        self.optimistic(&action);
        self.stage = Stage::default();
        let id = uuid::Uuid::new_v4().to_string();
        self.operation_id = Some(id.clone());
        let request = Request { id, server, entry, extra, operation: action };
        if let Some(backend) = &self.backend {
            backend.capture_share(request);
        } else {
            self.pending = Some(request);
        }
    }

    /// A share of the member's own item shows at once: the picker folds away, the friends and Everyone appear among who
    /// sees it, marked on their way until the tray answers (a failure puts the saved entry back).
    fn optimistic(&mut self, action: &Action) {
        let Action::Share(s) = action else { return };
        if !self.entry.mine() {
            return;
        }
        self.rollback = Some(self.entry.clone());
        self.picker = false;
        let kind = if self.entry.is_picture() { shares::Kind::Picture } else { shares::Kind::Capture };
        let item = self.entry.remote.get_or_insert_with(|| shares::Item { kind, mine: true, can_share: true, ..Default::default() });
        if s.public && !item.public {
            item.public = true;
            self.public_on_its_way = true;
        }
        for r in &s.recipients {
            if !item.grants.iter().any(|g| g.vrchat_user_id == r.vrchat_user_id) {
                item.grants.push(shares::Grant { vrchat_user_id: r.vrchat_user_id.clone(), display_name: Some(r.display_name.clone()), ..Default::default() });
                self.on_its_way.insert(r.vrchat_user_id.clone());
            }
        }
    }

    /// The picture the card showed (shown at once) and the full one (crossfading in once decoded).
    fn pictures_of(&self, development: bool) -> (Option<crate::worlds::Picture>, Option<crate::worlds::Picture>) {
        use crate::worlds::Picture;
        // A layer chosen: the picture under its stack stays the multi-layer picture's.
        let entry = self.layers.layered(&self.entry);
        let large = match (&entry.local, &entry.remote) {
            (Some(controller::Local::Picture(path)), _) => Some(Picture::Shot { path: path.clone(), size: 0, width: 2048 }),
            (None, Some(item)) if item.picture.is_some() && !self.fixture_active => Some(Picture::Capture { development, id: item.id, large: true }),
            _ => None,
        };
        (entry.picture.clone(), large)
    }

    fn panel(&self) -> Panel {
        if self.warning.is_some() {
            Panel::Warning
        } else if self.picker {
            Panel::Picker
        } else {
            Panel::None
        }
    }

    /// Handles what came back from the service; returns true when the window is gone.
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx) -> bool {
        let ctx = ui.ctx().clone();
        let development = cx.snap.settings.development;
        let server = if development { Server::Development } else { Server::Production };
        let scope = crate::controller::vrchat_data::Scope::of(cx.snap);
        if !self.fixture_active && self.friend_scope.as_ref() != Some(&scope) {
            self.friend_scope = Some(scope.clone());
            self.friends_requested = false;
            self.friends_list.clear();
            self.selected.clear();
            self.friends_error = None;
            self.friends = Job::default();
            self.pictures = crate::worlds::Pictures::with_backend(&ctx, Some(cx.ctl.clone()));
        }
        // Read as soon as the window opens on something the member can share, so that Share to … lists them at once
        // (VRChat's friends come a hundred a request, a second apart).
        let shareable = self.entry.ready && self.entry.remote.as_ref().is_none_or(|i| i.can_share && i.hidden.is_none());
        if !self.fixture_active && (self.picker || shareable) && !self.friends_requested {
            self.friends_requested = true;
            let backend = cx.ctl.clone();
            self.friends = Job::spawn(&ctx, move || {
                use crate::controller::vrchat_data::{Request, Response};
                match backend.vrchat_data(scope, Request::Friends)? {
                    Response::Friends(users) => Ok(users
                        .into_iter()
                        .map(|u| Friend { image: (!u.picture().is_empty()).then(|| u.picture().to_owned()), id: u.id, name: u.display_name })
                        .collect()),
                    _ => Err("Unexpected friends response.".into()),
                }
            });
        }
        if let Some(f) = self.friends.take() {
            match f {
                Ok(f) => self.friends_list = f,
                Err(e) => self.friends_error = Some(e),
            }
        }
        self.backend = Some(cx.ctl.clone());
        if let Some(request) = self.pending.take() {
            cx.ctl.capture_share(request);
        }
        // Share to … on a capture not uploaded yet: its upload files are made while friends are chosen.
        if !self.fixture_active && self.picker && !self.prepared_ahead && self.entry.ready && self.entry.remote.is_none() {
            if let Some(controller::Local::Session(dir)) = &self.entry.local {
                self.prepared_ahead = true;
                cx.ctl.prepare_share(dir.clone());
            }
        }
        let job = self.operation_id.as_ref().and_then(|id| cx.snap.capture_shares.iter().find(|j| &j.id == id));
        if let Some(j) = job {
            self.stage = j.stage;
        }
        let done = job.and_then(|j| j.outcome.clone());
        if let Some(done) = done {
            self.operation_id = None;
            self.rollback_or_settle(&ctx, cx, done);
        }

        // The photo editor on a copy of the picture, over the whole window under its title bar; the window waits under it.
        if let Some(e) = self.editor.as_mut() {
            let window = ctx.content_rect();
            let rect = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
            let mut open = true;
            egui::Area::new(Id::unique("gallery-editor")).order(Order::Foreground).fixed_pos(rect.min).show(&ctx, |ui| {
                ui.set_min_size(rect.size());
                open = e.show(ui, rect);
            });
            if !open {
                self.editor = None;
            }
            // Edit in 3D on a copy made there: its capture's 3D view where it was, with its layers.
            if let (Some((view, edit)), Some(gpu)) = (self.editor.as_mut().and_then(|e| e.take_3d()), cx.gpu) {
                self.editor = None;
                self.viewer = Some(super::splats::Viewer::open_edit(&ctx, gpu, view, edit, self.entry.world_name.clone(), super::splats::on_integrated(cx)));
            }
            return false;
        }
        // The 3D view of the capture, over everything; the window waits under it.
        if let Some(v) = self.viewer.as_mut() {
            let rect = ui.max_rect();
            let mut open = true;
            egui::Area::new(Id::unique("gallery-3d")).order(egui::Order::Tooltip).fixed_pos(rect.min).show(&ctx, |ui| {
                ui.set_min_size(rect.size());
                open = v.show(ui, rect, super::splats::on_integrated(cx));
            });
            if !open {
                self.viewer = None;
            }
            return false;
        }
        self.window(ui, cx, server)
    }

    fn rollback_or_settle(&mut self, ctx: &egui::Context, cx: &mut Cx, done: Outcome) {
        match done {
            Outcome::Shared(Shared::Item { item }) | Outcome::Unshared(item) => {
                let was_public = self.entry.remote.as_ref().is_some_and(|i| i.public);
                let notice = match &self.last {
                    Some(Action::Share(s)) if s.public && !s.recipients.is_empty() => "Shared publicly and with your friends".to_string(),
                    Some(Action::Share(s)) if s.public => "Shared publicly: it is on Orbiters now".to_string(),
                    Some(Action::Share(s)) => format!("Shared with {} friend{}", s.recipients.len(), if s.recipients.len() == 1 { "" } else { "s" }),
                    Some(Action::Unshare(u)) if u.all => "Not shared any more".to_string(),
                    Some(Action::Unshare(u)) if u.public && was_public => "Not public any more".to_string(),
                    Some(Action::Unshare(_)) => "Sharing updated".to_string(),
                    _ => "Sharing updated".to_string(),
                };
                self.entry.remote = Some(item);
                self.changed = true;
                self.picker = false;
                self.rollback = None;
                self.on_its_way.clear();
                self.public_on_its_way = false;
                self.notice = Some(notice);
                if std::mem::take(&mut self.copy_after) {
                    let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
                    self.copy_link(ctx, server);
                }
            }
            Outcome::Shared(Shared::Request { .. }) => {
                self.picker = false;
                self.on_its_way.clear();
                self.public_on_its_way = false;
                if let Some(entry) = self.rollback.take() {
                    self.entry = entry;
                }
                let from =
                    self.entry.remote.as_ref().and_then(|i| i.from.as_ref()).map(|p| p.name.clone()).unwrap_or_else(|| "the person who shared it".into());
                self.notice = Some(format!("Request sent: {from} decides"));
            }
            Outcome::Answered(request) => {
                self.notice = Some("Share request answered".into());
                if let Some(item) = request.item {
                    self.entry.remote = Some(item);
                    self.changed = true;
                }
            }
            Outcome::Downloaded(path) => {
                self.notice = Some(format!("Saved to {}", path.display()));
                if let Some(item) = self.entry.remote.as_ref().filter(|i| i.kind == shares::Kind::Capture) {
                    if let Some(gpu) = cx.gpu {
                        self.viewer = Some(super::splats::Viewer::open_shared(
                            ctx,
                            gpu,
                            &path,
                            self.entry.world_name.clone(),
                            &item.capture.as_ref().map(|c| c.view.clone()).unwrap_or_default(),
                            super::splats::on_integrated(cx),
                        ));
                    }
                } else {
                    let _ = crate::controller::open_path(&path);
                }
            }
            Outcome::Failed { message, explicit } => {
                // The explicit warning's Share anyway still copies the link.
                self.copy_after &= explicit;
                if explicit {
                    self.warning = Some(message);
                } else {
                    self.error = Some(message);
                }
                if let Some(entry) = self.rollback.take() {
                    self.entry = entry;
                }
                self.on_its_way.clear();
                self.public_on_its_way = false;
            }
        }
    }

    /// The window itself; true once it closed back into its card.
    fn window(&mut self, ui: &mut Ui, cx: &mut Cx, server: Server) -> bool {
        let ctx = ui.ctx().clone();
        let page = ui.max_rect();
        let area = Rect::from_min_max(pos2(page.left() + GUTTER, page.top() + cx.top + 12.0), pos2(page.right() - GUTTER, page.bottom() - GUTTER));
        let busy = self.operation_id.is_some() || self.fixture_busy;
        let (thumb, large) = self.pictures_of(cx.snap.settings.development);
        let thumb_tex = thumb.as_ref().and_then(|p| self.pictures.get(&ctx, p));
        let large_tex = large.as_ref().and_then(|p| self.pictures.get(&ctx, p));
        let aspect = large_tex
            .as_ref()
            .or(thumb_tex.as_ref())
            .map(|t| t.size_vec2().x / t.size_vec2().y.max(1.0))
            .or_else(|| self.entry.remote.as_ref().and_then(|i| i.picture.as_ref()).filter(|p| p.height > 0).map(|p| p.width as f32 / p.height as f32))
            .unwrap_or(16.0 / 9.0);
        let measured = &self.measured;
        let layout = viewer::picture_layout(area, aspect, |w| {
            measured.get(&(w.round() as u32)).copied().unwrap_or(if w <= viewer::SIDE_W + 1.0 { 260.0 } else { 118.0 })
        });
        let full_rect = viewer::contain(area, aspect);
        let full = anim::toggle(&ctx, Id::unique(("capture-window-full", &self.entry.key)), self.full, Spring::EXPAND);
        let (target, picture_target) = if self.full { (full_rect, full_rect) } else { (layout.window, layout.picture) };
        let origin = self.origin;
        let morph = self.morph.get_or_insert_with(|| {
            let serial = OPENED.fetch_add(1, Ordering::Relaxed);
            let (origin, radius) = origin.unwrap_or((Rect::from_center_size(target.center(), target.size() * 0.92), viewer::RADIUS));
            Morph::new(Id::unique(("capture-window", serial)), origin, radius)
        });
        if morph.id() == Id::NULL {
            // Closed before it ever showed.
            return true;
        }
        if self.lost {
            morph.set_origin(Rect::from_center_size(target.center(), target.size() * 0.92));
        }
        morph.riding(self.source.is_some());
        let f = morph.frame(&ctx, target);
        let (wid, closing) = (morph.id(), morph.is_closing());
        let picture = morph.part(&ctx, "picture", picture_target, &f);
        // Landed and settled: this frame still draws its card where it rests, the page draws it from the next one.
        self.finished = f.closed;
        // Another item coming in from the side it is on, the one before leaving the other way.
        let nav = if self.nav.0 == 0 { 1.0 } else { anim::spring_from(&ctx, wid.with(("nav", self.nav.0)), [0.0], [1.0], Spring::SOFT)[0] };
        let slide = 28.0 * self.nav.1;
        if nav >= 0.999 {
            self.previous = None;
        }
        let details_alpha = f.content * (1.0 - full).clamp(0.0, 1.0) * nav.clamp(0.0, 1.0);

        let window = ctx.content_rect();
        let catcher = Rect::from_min_max(pos2(window.left(), window.top() + crate::app::TITLE_H), window.max);
        let mut close = false;
        let mut reopen = false;
        let mut toggle_full = false;
        let mut act: Option<Act> = None;
        let source = self.source.take();
        // Opaque from its first frame (egui fades new areas in): it grows out of its card instead.
        egui::Area::new(wid.with("area")).order(Order::Foreground).fixed_pos(catcher.min).fade_in(false).show(&ctx, |ui| {
            ui.set_min_size(catcher.size());
            let backdrop = ui.interact(catcher, wid.with("backdrop"), Sense::click());
            viewer::paint_backdrop(&ui.painter().with_clip_rect(window), window, &f);
            // Also while a share runs: the tray carries it on, the card shows it going.
            if widgets::pressed(&backdrop) && !f.rect.contains(backdrop.interact_pointer_pos().unwrap_or_default()) {
                close = true;
            }
            // Its card, riding under it near the card's place: the window fades in over it as it leaves and out over
            // it as it lands, then the card bounces.
            if let Some(src) = source.as_ref().filter(|_| f.source > 0.001) {
                let ride = morph::ride(src.home, src.picture, picture, f.landing);
                let mut su = ui.new_child(UiBuilder::new().max_rect(src.home).id_salt(wid.with("source")));
                su.set_clip_rect(ride.inverse() * window);
                su.multiply_opacity(f.source);
                su.with_visual_transform(ride, |su| (src.paint)(su, cx));
            }
            // Pressed again on its way back: it opens again from where it is.
            if widgets::pressed(&ui.interact(f.rect, wid.with("surface"), Sense::click())) && closing {
                reopen = true;
            }
            let p = ui.painter().clone();
            viewer::paint_surface(&p, &f);
            ui.multiply_opacity(f.shape);
            let p = ui.painter().clone();

            // The picture: the card's at once, the full one fading in over it once decoded.
            let r = f.radius;
            let radii = match (self.full, layout.arrangement) {
                (true, _) => [r; 4],
                (false, Arrangement::Under) => [r, r, r * full, r * full],
                (false, Arrangement::Beside) => [r, r * full, r * full, r],
            };
            // Dragging it out of the window (its page allows it): a press that moves past the slop is a drag out, a
            // press released before is its click, which shows it whole on the release (not on the press, which may be
            // a drag's).
            let sense = match (closing, self.drag_out) {
                (true, _) => Sense::hover(),
                (false, true) => Sense::click_and_drag(),
                (false, false) => Sense::click(),
            };
            let pic_resp = ui.interact(picture, wid.with("picture"), sense);
            self.dragged = self.drag_out && !closing && self.drag.update(&pic_resp);
            let lift = anim::toggle(&ctx, wid.with("picture-lift"), self.lifted, Spring::BOUNCY);
            let hover = anim::toggle(&ctx, wid.with("picture-hover"), pic_resp.hovered() && self.entry.is_picture(), Spring::SOFT);
            let shown_large = feedback::loaded(&ctx, wid.with(("large", self.nav.0)), large_tex.is_some());
            let k = nav.clamp(0.0, 1.0);
            if let Some(prev) = self.previous.as_ref().filter(|_| k < 0.999) {
                feedback::cover(&p, picture, radii, prev, 1.0, 1.0);
            }
            // The new one slides in over the one before, inside the picture's frame; lifted by a drag out, a little
            // smaller and faded until the drop (as the grid's cards).
            let mut np = p.with_clip_rect(picture.intersect(p.clip_rect()));
            let at = picture.translate(vec2(slide * (1.0 - k), 0.0));
            let at = Rect::from_center_size(at.center(), at.size() * (1.0 - 0.04 * lift));
            if lift > 0.001 {
                feedback::fill_corners(&np, picture, radii, theme::alpha(theme::INK_750, lift.clamp(0.0, 1.0)));
                np.multiply_opacity(1.0 - 0.35 * lift.clamp(0.0, 1.0));
            }
            match (&thumb_tex, &large_tex) {
                (Some(t), _) if shown_large < 0.99 => feedback::cover(&np, at, radii, t, 1.0 + 0.015 * hover, k),
                (None, None) => {
                    feedback::fill_corners(&np, at, radii, theme::alpha(theme::INK_750, k));
                    let glyph = if self.entry.is_picture() { icons::IMAGES } else { icons::CUBE };
                    widgets::icon(&np, at.center(), glyph, 40.0, theme::alpha(theme::FG_4, k));
                }
                _ => {}
            }
            if let Some(l) = &large_tex {
                feedback::cover(&np, at, radii, l, 1.0 + 0.015 * hover, shown_large * k);
            }
            self.last_picture = large_tex.clone().filter(|_| shown_large > 0.99).or_else(|| thumb_tex.clone());
            // A multi-layer picture's layers, stacked over a checkerboard, when they are shown instead of it.
            self.layers.paint(&np, at, radii, k, &mut self.pictures, &self.entry);
            let cursor = if self.lifted || pic_resp.dragged() && self.drag_out { egui::CursorIcon::Grabbing } else { egui::CursorIcon::ZoomIn };
            let pic_resp = pic_resp.on_hover_cursor(cursor);
            let shown_whole = if self.drag_out { pic_resp.clicked() } else { pressed(pic_resp) };
            if self.entry.is_picture() && shown_whole {
                toggle_full = true;
            }
            let mut cu = ui.new_child(UiBuilder::new().max_rect(f.rect).id_salt(wid.with("chrome")));
            cu.set_clip_rect(f.rect.intersect(ui.clip_rect()));
            cu.multiply_opacity(f.content);
            if closing || f.content < 0.05 {
                cu.disable();
            }
            self.picture_overlays(&mut cu, cx, picture, busy, full, &mut act);
            if viewer::close_button(&mut cu, f.rect, "Close") {
                close = true;
            }
            // The items before and after it, over the picture's sides.
            let can = self.can_navigate() || self.fixture_active;
            for (dir, glyph, label, x, there) in [
                (-1i64, icons::CARET_LEFT, "Previous", picture.left() + 16.0 + 20.0, self.neighbours.0),
                (1, icons::CARET_RIGHT, "Next", picture.right() - 16.0 - 20.0, self.neighbours.1),
            ] {
                if !there || full > 0.5 {
                    continue;
                }
                let r = Rect::from_center_size(pos2(x, picture.center().y), vec2(40.0, 40.0));
                let mut au = cu.new_child(UiBuilder::new().max_rect(r).id_salt(("capture-nav-button", dir)));
                if !can {
                    au.disable();
                    au.multiply_opacity(0.4);
                }
                if pressed(widgets::IconButton::new(glyph, label).size(40.0, 18.0).backdrop(theme::black(0.5)).show_at(&mut au, r)) && can {
                    self.step = Some(dir);
                }
            }

            // Details, laid out at the window's final width, clipped by the window on its way.
            if details_alpha > 0.01 || !self.full {
                let d = match layout.arrangement {
                    Arrangement::Under => Rect::from_min_size(pos2(f.rect.left(), picture.bottom()), vec2(layout.details.width(), 4000.0)),
                    Arrangement::Beside => Rect::from_min_size(pos2(picture.right(), f.rect.top()), vec2(layout.details.width(), 4000.0)),
                };
                let d = d.translate(vec2(slide * 0.5 * (1.0 - nav.clamp(0.0, 1.0)), 0.0));
                let mut du = ui.new_child(
                    UiBuilder::new().max_rect(d.shrink2(vec2(PAD, 0.0))).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(wid.with("details")),
                );
                du.set_clip_rect(f.rect.intersect(ui.clip_rect()));
                du.multiply_opacity(details_alpha);
                if closing || details_alpha < 0.5 {
                    du.disable();
                }
                du.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let dy = 10.0 * (1.0 - details_alpha);
                du.add_space(if layout.arrangement == Arrangement::Beside { 64.0 } else { 20.0 } + dy);
                self.details(&mut du, cx, busy, layout.arrangement, &mut act);
                du.add_space(22.0);
                let h = du.min_rect().height() - dy;
                self.measured.insert(layout.details.width().round() as u32, h.ceil());
            }
        });

        if toggle_full {
            self.full = !self.full;
        }
        if let Some(a) = act {
            self.act(&ctx, cx, server, a);
        }
        if !closing && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.full {
                self.full = false;
            } else if self.warning.is_some() {
                self.warning = None;
            } else if self.picker {
                self.picker = false;
            } else {
                close = true;
            }
        }
        if close {
            if let Some(m) = self.morph.as_mut() {
                m.close();
                self.full = false;
            }
        }
        if reopen && !f.closed {
            if let Some(m) = self.morph.as_mut() {
                m.reopen();
            }
        }
        f.closed
    }

    /// Over the picture: what it is (top left), its 3D view or folder (bottom right), a share's progress (bottom edge).
    fn picture_overlays(&mut self, ui: &mut Ui, cx: &Cx, picture: Rect, busy: bool, full: f32, act: &mut Option<Act>) {
        let p = ui.painter().clone();
        let (glyph, label, color) = if self.entry.is_picture() { (icons::IMAGES, "Picture", theme::FG) } else { (icons::CUBE, "3D capture", theme::ACCENT) };
        // A multi-layer picture's chosen layer says which.
        let layer = self.layers.chosen_name().map(|n| format!("{n} layer"));
        let (glyph, label) = match &layer {
            Some(l) => (icons::STACK, l.as_str()),
            None => (glyph, label),
        };
        let t = Txt::new(label, 11.5).weight(weight::SEMIBOLD).color(theme::FG);
        let tw = widgets::text_size(ui, &t).x;
        let badge = Rect::from_min_size(picture.min + vec2(16.0, 16.0), vec2(tw + 38.0, 28.0));
        p.rect_filled(badge, theme::radius(14.0), theme::black(0.55));
        widgets::icon(&p, pos2(badge.left() + 17.0, badge.center().y), glyph, 14.0, color);
        widgets::text(&p, pos2(badge.left() + 29.0, badge.center().y), Align2::LEFT_CENTER, &t);

        // A share running: a light running along the picture's bottom edge, or how much is sent while sending.
        let running = anim::toggle(ui.ctx(), Id::unique(("capture-window-busy", &self.entry.key)), busy, Spring::SOFT).clamp(0.0, 1.0);
        if running > 0.01 {
            let bar = Rect::from_min_max(pos2(picture.left(), picture.bottom() - 3.0), picture.max);
            let bp = p.with_clip_rect(bar.intersect(p.clip_rect()));
            bp.rect_filled(bar, 0.0, theme::alpha(theme::ACCENT, 0.18 * running));
            let sent = sent_part(self.stage).filter(|_| busy);
            let shown = anim::spring(ui.ctx(), Id::unique(("capture-window-sent", &self.entry.key)), [sent.unwrap_or(0.0)], Spring::SOFT)[0].clamp(0.0, 1.0);
            if sent.is_some() {
                bp.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * shown, 3.0)), theme::radius(1.5), theme::alpha(theme::ACCENT, running));
            } else {
                let k = ((anim::time(ui.ctx()) * 0.8) % 1.0) as f32;
                let w = bar.width() * 0.35;
                let x = bar.left() - w + k * (bar.width() + w);
                bp.rect_filled(Rect::from_min_size(pos2(x, bar.top()), vec2(w, 3.0)), theme::radius(1.5), theme::alpha(theme::ACCENT, running));
            }
            if anim::is_live(ui.ctx()) {
                ui.ctx().request_repaint();
            }
        }

        if full > 0.5 {
            // Shown whole: the way back to its details.
            let r = Rect::from_min_size(picture.max - vec2(16.0 + 32.0, 16.0 + 32.0), vec2(32.0, 32.0));
            let mut fu = ui.new_child(UiBuilder::new().max_rect(r).id_salt("capture-window-unfull"));
            fu.multiply_opacity(((full - 0.5) * 2.0).clamp(0.0, 1.0));
            let resp = widgets::IconButton::new(icons::CORNERS_IN, "Back to its details").size(32.0, 15.0).backdrop(theme::black(0.5)).show_at(&mut fu, r);
            if pressed(resp) {
                *act = Some(Act::Full);
            }
            return;
        }
        // A multi-layer picture's layers, bottom left: the one chosen is what the window shares.
        if let Some(entry) = self.layers.strip(ui, picture, picture.width() - 32.0 - 96.0, &self.entry, &mut self.pictures, !busy) {
            self.show_layer(entry);
        }
        let mut right = picture.right() - 16.0;
        let bottom = picture.bottom() - 16.0;
        if !self.entry.is_picture() {
            let (label, enabled, hint) = match &self.entry.local {
                Some(controller::Local::Session(_)) if !self.entry.ready => {
                    ("View in 3D", false, Some("Reconstruct it first: the 3D view comes from its splats"))
                }
                Some(controller::Local::Session(_)) => ("View in 3D", cx.gpu.is_some(), None),
                _ if busy && matches!(self.last, Some(Action::Download)) => ("Downloading…", false, None),
                _ => ("View in 3D", !busy, None),
            };
            let mut b = Button::new(label).icon(icons::CUBE_FOCUS).variant(Variant::Glass).size(Size::Sm).enabled(enabled).id_salt("capture-window-3d");
            if let Some(h) = hint {
                b = b.hint(h);
            }
            let w = b.desired_width(ui);
            let r = Rect::from_min_size(pos2(right - w, bottom - 32.0), vec2(w, 32.0));
            if pressed(b.show_at(ui, r)) {
                *act = Some(Act::View3d);
            }
            right = r.left() - 8.0;
        }
        if let Some(local) = &self.entry.local {
            let r = Rect::from_min_size(pos2(right - 32.0, bottom - 32.0), vec2(32.0, 32.0));
            let label = if matches!(local, controller::Local::Picture(_)) { "Show in its folder" } else { "Open its folder" };
            let resp = widgets::IconButton::new(icons::FOLDER_OPEN, label).size(32.0, 15.0).backdrop(theme::black(0.5)).show_at(ui, r);
            if pressed(resp) {
                *act = Some(Act::Folder);
            }
            right = r.left() - 8.0;
        }
        if self.entry.is_picture() {
            let r = Rect::from_min_size(pos2(right - 32.0, bottom - 32.0), vec2(32.0, 32.0));
            let resp = widgets::IconButton::new(icons::CORNERS_OUT, "See it whole").size(32.0, 15.0).backdrop(theme::black(0.5)).show_at(ui, r);
            if pressed(resp) {
                *act = Some(Act::Full);
            }
        }
    }

    /// Title, when and who sees it, the actions, who it is shared with, then the panel and messages.
    fn details(&mut self, ui: &mut Ui, cx: &Cx, busy: bool, arrangement: Arrangement, act: &mut Option<Act>) {
        let ctx = ui.ctx().clone();
        let w = ui.available_width();
        let mine = self.entry.mine();
        let item = self.entry.remote.clone();
        let hidden = item.as_ref().is_some_and(|i| i.hidden.is_some());
        let public = item.as_ref().is_some_and(|i| i.public);
        let can_share = self.entry.ready && !hidden && self.warning.is_none();

        // The actions this item has.
        let mut actions: Vec<(Act, Button)> = Vec::new();
        if mine {
            if !self.entry.ready && matches!(self.entry.local, Some(controller::Local::Session(_))) {
                let queued = matches!(&self.entry.local, Some(controller::Local::Session(d)) if cx.snap.reconstruct_queue.contains(d) || cx.snap.reconstruction.as_ref().is_some_and(|j| &j.session_dir == d));
                actions.push((
                    Act::Reconstruct,
                    Button::new(if queued { "Reconstructing…" } else { "Reconstruct to share" })
                        .icon(icons::MAGIC_WAND)
                        .variant(Variant::Primary)
                        .enabled(!queued),
                ));
            } else {
                if !public {
                    let label = if busy && matches!(&self.last, Some(Action::Share(s)) if s.public) { "Sharing…" } else { "Share publicly" };
                    actions.push((Act::Public, Button::new(label).icon(icons::GLOBE).variant(Variant::Primary).enabled(can_share && !busy)));
                }
                actions.push((Act::ShareTo, Button::new("Share to …").icon(icons::USERS).variant(Variant::Secondary).enabled(can_share && !busy)));
                if !self.entry.is_picture() {
                    actions.push((Act::CopyLink, self.copy_button(ui, busy, can_share)));
                }
                if super::editor::source_of(self.layers.layered(&self.entry)).is_some() {
                    actions.push((Act::Edit, Button::new("Edit").icon(icons::SLIDERS_HORIZONTAL).variant(Variant::Secondary)));
                }
            }
        } else {
            let label = if busy && matches!(self.last, Some(Action::Share(_))) { "Asking…" } else { "Ask to share" };
            actions.push((Act::ShareTo, Button::new(label).icon(icons::PAPER_PLANE_TILT).variant(Variant::Primary).enabled(can_share && !busy)));
            if self.entry.is_picture() {
                actions.push((Act::Download, Button::new("Download").icon(icons::DOWNLOAD_SIMPLE).enabled(!busy)));
                // A copy of it edited (downloaded at its full size), saved among this computer's pictures.
                if super::editor::source_of(&self.entry).is_some() {
                    actions.push((Act::Edit, Button::new("Edit").icon(icons::SLIDERS_HORIZONTAL).variant(Variant::Secondary)));
                }
            } else if public && !hidden {
                actions.push((Act::CopyLink, self.copy_button(ui, busy, true)));
            }
        }
        let actions: Vec<(Act, Button)> = actions
            .into_iter()
            .map(|(a, b)| {
                let tag = a.tag();
                (a, b.size(Size::Sm).id_salt(("capture-window-act", tag)))
            })
            .collect();
        let widths: Vec<f32> = actions.iter().map(|(_, b)| b.desired_width(ui)).collect();
        let actions_w = widths.iter().sum::<f32>() + 8.0 * widths.len().saturating_sub(1) as f32;

        // Title and its line; the actions beside it when they fit.
        let title = Txt::new(&self.entry.world_name, 19.0).weight(weight::SEMIBOLD).tracking(-0.01);
        let title_w = widgets::text_size(ui, &title).x;
        let beside = arrangement == Arrangement::Under && title_w.min(320.0) + 24.0 + actions_w <= w;
        let (head, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
        let text_w = if beside { w - actions_w - 24.0 } else { w };
        let p = ui.painter().clone();
        widgets::text_truncated(&p, pos2(head.left(), head.top() + 12.0), Align2::LEFT_CENTER, &title, text_w);
        self.subline(ui, pos2(head.left(), head.top() + 37.0), text_w, mine, item.as_ref());
        let row = if beside {
            Rect::from_min_size(pos2(head.right() - actions_w, head.top() + 8.0), vec2(actions_w, 32.0))
        } else {
            ui.add_space(14.0);
            let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            Rect::from_min_size(r.min, vec2(actions_w.min(w), 32.0))
        };
        let mut x = row.left();
        let mut copy_rect = None;
        for ((a, b), bw) in actions.into_iter().zip(widths) {
            let r = Rect::from_min_size(pos2(x, row.top()), vec2(bw, 32.0));
            if matches!(a, Act::CopyLink) {
                copy_rect = Some(r);
            }
            if pressed(b.show_at(ui, r)) {
                *act = Some(a);
            }
            x += bw + 8.0;
        }
        // Copied: a ring of green leaves the button as it turns green.
        if let (Some(r), Some(at)) = (copy_rect, self.copied_at) {
            let k = ((anim::time(&ctx) - at) / 0.7) as f32;
            if k < 1.0 {
                let e = orbiters_ui::anim::ease::out_cubic(k.max(0.0));
                let ring = r.expand(1.0 + 9.0 * e);
                p.rect_stroke(
                    ring,
                    theme::radius(ring.height() * 0.5),
                    egui::Stroke::new(2.0 * (1.0 - e), theme::alpha(theme::ACCENT, 0.85 * (1.0 - e))),
                    egui::StrokeKind::Outside,
                );
                ctx.request_repaint();
            }
        }

        // Who sees it, each with an ×.
        if mine && item.as_ref().is_some_and(|i| i.public || !i.grants.is_empty()) {
            ui.add_space(16.0);
            self.shared_with(ui, cx, busy, act);
        }
        if hidden {
            ui.add_space(14.0);
            feedback::banner(ui, Id::unique(("capture-hidden", &self.entry.key)), Tone::Warn, "Hidden by a moderator: it can no longer be shared.", false);
        }
        if busy {
            ui.add_space(14.0);
            let what = match &self.last {
                Some(Action::Download) => "Downloading…".to_string(),
                Some(Action::Unshare(_)) => "Updating who sees it…".to_string(),
                Some(Action::Share(_)) if !mine => format!("Asking {}…", item.as_ref().and_then(|i| i.from.as_ref()).map_or("them", |p| p.name.as_str())),
                Some(Action::Share(s)) => match self.stage {
                    Stage::Waiting => "Waiting for its other change to finish…".to_string(),
                    Stage::Preparing => "Preparing the 3D view for sharing…".to_string(),
                    Stage::Sending { sent, total } if sent > 0 && total > 0 => format!("Sending {} of {}…", megabytes(sent), megabytes(total)),
                    Stage::Sending { total, .. } if total > 0 => format!("Sending {}…", megabytes(total)),
                    Stage::Sending { .. } => "Sending…".to_string(),
                    Stage::Starting | Stage::Sharing => format!("Sharing {}…", whom(s)),
                    Stage::Items { done, total } => format!("Sharing {} · {done} of {total} done…", whom(s)),
                },
                _ => "Working…".to_string(),
            };
            working_part(ui, &what, sent_part(self.stage));
            if matches!(self.last, Some(Action::Share(_))) && mine {
                ui.add_space(6.0);
                paragraph(ui, "You can close this window: it carries on.", 11.5, theme::FG_4, w);
            }
        }
        let panel = self.panel();
        let key = (panel as u8, self.panel_key.1);
        if key.0 != self.panel_key.0 {
            self.panel_key = (key.0, self.panel_key.1 + 1);
        }
        if panel != Panel::None {
            ui.add_space(16.0);
            let id = Id::unique(("capture-panel", &self.entry.key, self.panel_key.1));
            let k = anim::spring_from(&ctx, id, [0.0], [1.0], Spring::SOFT)[0];
            let start = ui.cursor().min;
            let mut pu = ui.new_child(
                UiBuilder::new()
                    .max_rect(Rect::from_min_size(start + vec2(16.0 * (1.0 - k), 0.0), vec2(w, 2000.0)))
                    .layout(egui::Layout::top_down(egui::Align::Min))
                    .id_salt(id),
            );
            pu.multiply_opacity(k.clamp(0.0, 1.0));
            pu.spacing_mut().item_spacing = vec2(0.0, 0.0);
            match panel {
                Panel::Warning => self.warning_panel(&mut pu, busy, act),
                Panel::Picker => self.picker_panel(&mut pu, cx, busy, act),
                Panel::None => {}
            }
            let h = pu.min_rect().height();
            ui.allocate_exact_size(vec2(w, h), Sense::hover());
        }
        if let Some(n) = self.notice.clone() {
            ui.add_space(14.0);
            if feedback::banner(ui, Id::unique(("capture-notice", &self.entry.key, &n)), Tone::Ok, &n, true) {
                self.notice = None;
            }
        }
        if let Some(e) = self.error.clone() {
            ui.add_space(14.0);
            feedback::banner(ui, Id::unique(("capture-error", &self.entry.key, &e)), Tone::Danger, &e, false);
            if self.last.is_some() {
                ui.add_space(8.0);
                let b = Button::new("Try again").icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!busy).id_salt("capture-retry");
                let bw = b.desired_width(ui);
                let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
                if pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                    *act = Some(Act::Retry);
                }
            }
        }
    }

    /// "3D capture · 28 Oct 2026, 22:00 · ⊕ Public", or who shared it.
    fn subline(&mut self, ui: &Ui, at: egui::Pos2, max_w: f32, mine: bool, item: Option<&shares::Item>) {
        let p = ui.painter().clone();
        let kind = if self.entry.is_picture() { "VRChat picture" } else { "3D capture" };
        let date = super::local_time(&controller::iso(self.entry.taken));
        let line = Txt::new(format!("{kind} · {date}"), 12.5).color(theme::FG_3);
        let r = widgets::text_truncated(&p, at, Align2::LEFT_CENTER, &line, max_w);
        let x = r.right() + 10.0;
        let (glyph, label, color) = match item {
            None => (icons::HARD_DRIVE, "Only on this computer".to_string(), theme::FG_3),
            Some(i) if !mine => {
                if let Some(from) = &i.from {
                    let ar = Rect::from_center_size(pos2(x + 9.0, at.y), vec2(18.0, 18.0));
                    let tex = from.avatar_url.as_ref().filter(|_| !self.fixture_active).and_then(|url| {
                        self.pictures.get(ui.ctx(), &crate::worlds::Picture::Remote { world_id: format!("person-{}", from.id), url: url.clone(), large: false })
                    });
                    widgets::avatar(&p, ar, tex.as_ref(), &from.name);
                    let t = Txt::new(format!("from {}", from.name), 12.5).weight(weight::MEDIUM).color(theme::FG_2);
                    widgets::text_truncated(&p, pos2(ar.right() + 6.0, at.y), Align2::LEFT_CENTER, &t, (at.x + max_w - ar.right() - 6.0).max(10.0));
                    return;
                }
                (icons::USERS, "Shared with you".to_string(), theme::FG_2)
            }
            // Who sees it is listed under the line (Shared with).
            Some(i) if i.public || !i.grants.is_empty() => return,
            Some(_) => (icons::LOCK_SIMPLE, "Only you".to_string(), theme::FG_3),
        };
        if x + 40.0 > at.x + max_w {
            return;
        }
        widgets::icon(&p, pos2(x + 7.0, at.y), glyph, 13.0, color);
        widgets::sliding_value(
            ui,
            Id::unique(("capture-visibility", &self.entry.key)),
            pos2(x + 18.0, at.y),
            Align2::LEFT_CENTER,
            &label,
            &Txt::new("", 12.5).weight(weight::MEDIUM).color(color),
            (at.x + max_w - x - 18.0).max(10.0),
        );
    }

    /// Public and each friend it is shared with, as chips whose × takes them back; then Unshare everything.
    fn shared_with(&mut self, ui: &mut Ui, cx: &Cx, busy: bool, act: &mut Option<Act>) {
        let w = ui.available_width();
        let (cap, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
        widgets::text(ui.painter(), pos2(cap.left(), cap.center().y), Align2::LEFT_CENTER, &Txt::caps("Shared with"));
        ui.add_space(8.0);
        let Some(item) = self.entry.remote.clone() else { return };
        enum C {
            Public,
            Grant(String),
        }
        let mut chips: Vec<(C, Chip)> = Vec::new();
        // On their way: no ×, a spinner beside them until the tray answers.
        let mut coming: Vec<bool> = Vec::new();
        if item.public {
            let on_way = self.public_on_its_way;
            chips.push((C::Public, Chip::new("Everyone on Orbiters").icon(icons::GLOBE).tone(theme::ACCENT).removable(!on_way).enabled(!busy)));
            coming.push(on_way);
        }
        let names: Vec<(String, String)> =
            item.grants.iter().map(|g| (g.vrchat_user_id.clone(), g.display_name.clone().unwrap_or_else(|| g.vrchat_user_id.clone()))).collect();
        let pictures: Vec<Option<egui::TextureHandle>> = names
            .iter()
            .map(|(id, _)| {
                let url = self.friends_list.iter().find(|f| &f.id == id).and_then(|f| f.image.clone())?;
                self.pictures.get(ui.ctx(), &crate::worlds::Picture::Profile { scope: crate::controller::vrchat_data::Scope::of(cx.snap), url })
            })
            .collect();
        for ((id, name), tex) in names.iter().zip(&pictures) {
            let on_way = self.on_its_way.contains(id);
            chips.push((C::Grant(id.clone()), Chip::new(name).person(tex.as_ref()).removable(!on_way).enabled(!busy).id_salt(("grant", id))));
            coming.push(on_way);
        }
        let all = Button::new("Unshare everything").icon(icons::EYE_SLASH).variant(Variant::Danger).size(Size::Sm).enabled(!busy).id_salt("unshare-all");
        const SPIN: f32 = 22.0;
        let mut widths: Vec<f32> = chips.iter().zip(&coming).map(|((_, c), on_way)| c.width(ui) + if *on_way { SPIN } else { 0.0 }).collect();
        // Nothing to take back while everyone listed is still on their way.
        let any_shared = coming.iter().any(|on_way| !on_way);
        if any_shared {
            widths.push(all.desired_width(ui));
        }
        let (at, h) = widgets::flow(&widths, w, widgets::CHIP_H, 8.0);
        let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        for (n, (what, chip)) in chips.into_iter().enumerate() {
            let mut r = Rect::from_min_size(area.min + at[n], vec2(widths[n].min(w), widgets::CHIP_H));
            if coming[n] {
                r.max.x -= SPIN;
                feedback::spinner(ui.painter(), pos2(r.right() + SPIN * 0.5 + 2.0, r.center().y), 5.5, 1.8, theme::ACCENT);
            }
            let resp = chip.show_at(ui, r);
            if resp.removed {
                *act = Some(match what {
                    C::Public => Act::Unshare(Unshare { public: true, ..Default::default() }),
                    C::Grant(id) => Act::Unshare(Unshare { recipients: vec![id], ..Default::default() }),
                });
            }
        }
        if any_shared {
            let n = widths.len() - 1;
            let r = Rect::from_min_size(area.min + at[n], vec2(widths[n].min(w), widgets::CHIP_H));
            if pressed(all.show_at(ui, r)) {
                *act = Some(Act::Unshare(Unshare { all: true, ..Default::default() }));
            }
        }
    }

    /// Going ahead with something flagged explicit, said plainly.
    fn warning_panel(&mut self, ui: &mut Ui, busy: bool, act: &mut Option<Act>) {
        let w = ui.available_width();
        let warning = self.warning.clone().unwrap_or_default();
        let reasons: Vec<String> = self.entry.content.explicit_reasons.iter().map(|r| orbiters_core::explicit::reason_label(r)).collect();
        let inner = w - 32.0 - 52.0;
        let title = Txt::new("Before you share publicly", 14.5).weight(weight::SEMIBOLD);
        let body = ui.painter().layout_job(Txt::new(&warning, 12.5).color(theme::FG_2).line_height(19.0).job(inner));
        let tags_w: Vec<f32> = reasons.iter().map(|r| widgets::text_size(ui, &Txt::new(r, 11.0).weight(weight::MEDIUM)).x + 16.0).collect();
        let (tag_at, tags_h) = widgets::flow(&tags_w, inner, 22.0, 6.0);
        let h = 18.0 + 20.0 + 6.0 + body.size().y + if tags_h > 0.0 { 12.0 + tags_h } else { 0.0 } + 16.0 + 32.0 + 18.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let p = ui.painter().clone();
        widgets::frame(&p, rect, 18.0, theme::alpha(theme::WARN, 0.08));
        let tile = Rect::from_min_size(rect.min + vec2(16.0, 18.0), vec2(36.0, 36.0));
        p.rect_filled(tile, theme::radius(11.0), theme::alpha(theme::WARN, 0.14));
        widgets::icon(&p, tile.center(), icons::SHIELD_WARNING, 18.0, theme::WARN);
        let x = tile.right() + 16.0;
        let mut y = rect.top() + 18.0;
        widgets::text(&p, pos2(x, y + 10.0), Align2::LEFT_CENTER, &title);
        y += 26.0;
        let body_h = body.size().y;
        p.galley(pos2(x, y), body, theme::FG_2);
        y += body_h;
        if tags_h > 0.0 {
            y += 12.0;
            for (r, (o, tw)) in reasons.iter().zip(tag_at.iter().zip(&tags_w)) {
                let tr = Rect::from_min_size(pos2(x, y) + *o, vec2(*tw, 22.0));
                p.rect_filled(tr, theme::radius(11.0), theme::white(0.06));
                widgets::text(&p, tr.center(), Align2::CENTER_CENTER, &Txt::new(r, 11.0).weight(weight::MEDIUM).color(theme::FG_2));
            }
        }
        let anyway = Button::new("Share anyway").icon(icons::GLOBE).variant(Variant::Warn).size(Size::Sm).enabled(!busy).id_salt("share-anyway");
        let cancel = Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).id_salt("warning-cancel");
        let (aw, cw) = (anyway.desired_width(ui), cancel.desired_width(ui));
        let by = rect.bottom() - 18.0 - 32.0;
        if pressed(anyway.show_at(ui, Rect::from_min_size(pos2(rect.right() - 16.0 - aw, by), vec2(aw, 32.0)))) {
            *act = Some(Act::Anyway);
        }
        if pressed(cancel.show_at(ui, Rect::from_min_size(pos2(rect.right() - 16.0 - aw - 8.0 - cw, by), vec2(cw, 32.0)))) {
            *act = Some(Act::CancelWarning);
        }
    }

    /// Friends to share with (or to ask the sharer for): search, chips that toggle, a message, then send.
    fn picker_panel(&mut self, ui: &mut Ui, cx: &Cx, busy: bool, act: &mut Option<Act>) {
        let ctx = ui.ctx().clone();
        let w = ui.available_width();
        let mine = self.entry.mine();
        let from = self.entry.remote.as_ref().and_then(|i| i.from.as_ref()).map(|p| p.name.clone()).unwrap_or_else(|| "the sharer".into());
        let (title, sub) = if mine {
            ("Share with friends".to_string(), "They find it in Photos & captures, under Shared with me.".to_string())
        } else {
            (format!("Ask {from} to share it"), format!("{from} decides who else may see it."))
        };
        let (head, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
        let p = ui.painter().clone();
        widgets::text(&p, pos2(head.left(), head.top() + 10.0), Align2::LEFT_CENTER, &Txt::new(title, 14.5).weight(weight::SEMIBOLD));
        widgets::text_truncated(&p, pos2(head.left(), head.top() + 30.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.0).color(theme::FG_3), w - 120.0);
        let count = Txt::new(format!("{} of 50", self.selected.len()), 11.5).color(theme::FG_4);
        if !self.selected.is_empty() {
            widgets::text(&p, pos2(head.right(), head.top() + 10.0), Align2::RIGHT_CENTER, &count);
        }
        ui.add_space(10.0);
        let search_w = w;
        let mut su = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(ui.cursor().min, vec2(search_w, 40.0))).id_salt("friend-search-row"));
        widgets::search_field(&mut su, "friend-search", &mut self.query, "Search your VRChat friends", search_w);
        ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
        ui.add_space(12.0);

        let q = self.query.trim().to_lowercase();
        let scope = crate::controller::vrchat_data::Scope::of(cx.snap);
        if self.friends.running() && self.friends_list.is_empty() {
            let (r, _) = ui.allocate_exact_size(vec2(w, widgets::CHIP_H * 2.0 + 8.0), Sense::hover());
            let mut x = r.left();
            for (i, cw) in [96.0, 120.0, 84.0, 110.0, 132.0, 92.0].iter().enumerate() {
                let row = (i / 3) as f32;
                if i % 3 == 0 {
                    x = r.left();
                }
                feedback::shimmer(ui.painter(), Rect::from_min_size(pos2(x, r.top() + row * (widgets::CHIP_H + 8.0)), vec2(*cw, widgets::CHIP_H)), 16.0);
                x += cw + 8.0;
            }
        } else if let Some(error) = self.friends_error.clone() {
            feedback::banner(ui, Id::unique(("friends-error", &self.entry.key)), Tone::Danger, &error, false);
            ui.add_space(8.0);
            let b = Button::new("Load friends again").icon(icons::ARROW_CLOCKWISE).size(Size::Sm).enabled(!busy).id_salt("friends-retry");
            let bw = b.desired_width(ui);
            let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
            if pressed(b.show_at(ui, Rect::from_min_size(r.min, vec2(bw, 32.0)))) {
                self.friends_requested = false;
                self.friends_error = None;
            }
        } else if self.friends_list.is_empty() {
            let text = if cx.snap.vrchat_login.allowed && cx.snap.vrchat_login.user.is_some() {
                "No VRChat friends to show yet."
            } else {
                "No VRChat friends known yet: sign in to VRChat in Settings, or let VRCX or VRCNext load your friend list."
            };
            paragraph(ui, text, 12.5, theme::FG_3, w);
        } else {
            let matches: Vec<Friend> = self.friends_list.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).cloned().collect();
            let pictures: Vec<Option<egui::TextureHandle>> = matches
                .iter()
                .map(|f| f.image.as_ref().and_then(|url| self.pictures.get(&ctx, &crate::worlds::Picture::Profile { scope: scope.clone(), url: url.clone() })))
                .collect();
            let chips: Vec<Chip> = matches
                .iter()
                .zip(&pictures)
                .map(|(f, tex)| {
                    let chosen = self.selected.contains(&f.id);
                    Chip::new(&f.name).person(tex.as_ref()).selected(chosen).enabled(!busy && (chosen || self.selected.len() < 50)).id_salt(("friend", &f.id))
                })
                .collect();
            let widths: Vec<f32> = chips.iter().map(|c| c.width(ui)).collect();
            let (at, _) = widgets::flow(&widths, w, widgets::CHIP_H, 8.0);
            let limit = (FRIEND_ROWS as f32 - 1.0) * (widgets::CHIP_H + 8.0);
            let shown = at.iter().take_while(|o| o.y <= limit + 0.5).count();
            let h = if shown == 0 { 0.0 } else { at[shown - 1].y + widgets::CHIP_H };
            let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            let mut toggled = None;
            for (n, chip) in chips.into_iter().enumerate().take(shown) {
                if chip.show_at(ui, Rect::from_min_size(area.min + at[n], vec2(widths[n].min(w), widgets::CHIP_H))).pressed {
                    toggled = Some(matches[n].id.clone());
                }
            }
            if let Some(id) = toggled {
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
            }
            if matches.is_empty() {
                paragraph(ui, &format!("No friend called \"{}\".", self.query.trim()), 12.5, theme::FG_3, w);
            } else if shown < matches.len() {
                ui.add_space(8.0);
                paragraph(ui, &format!("{} more: search to find them", matches.len() - shown), 11.5, theme::FG_4, w);
            }
        }
        ui.add_space(12.0);
        if !mine {
            let public = Chip::new("Everyone on Orbiters too")
                .icon(icons::GLOBE)
                .tone(theme::ACCENT)
                .selected(self.request_public)
                .enabled(!busy)
                .id_salt("request-public");
            let cw = public.width(ui);
            let (r, _) = ui.allocate_exact_size(vec2(w, widgets::CHIP_H), Sense::hover());
            if public.show_at(ui, Rect::from_min_size(r.min, vec2(cw, widgets::CHIP_H))).pressed {
                self.request_public = !self.request_public;
            }
            ui.add_space(12.0);
        }
        let mut mu = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(ui.cursor().min, vec2(w, 36.0))).id_salt("share-message-row"));
        widgets::text_field(&mut mu, "share-message", &mut self.message, "Add a message (optional)", w, false);
        if self.message.chars().count() > 280 {
            self.message = self.message.chars().take(280).collect();
        }
        ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
        ui.add_space(14.0);
        let n = self.selected.len();
        let label = if mine {
            match n {
                0 => "Share".to_string(),
                1 => "Share with 1 friend".to_string(),
                _ => format!("Share with {n} friends"),
            }
        } else if n == 0 && self.request_public {
            "Ask to share publicly".to_string()
        } else {
            "Send request".to_string()
        };
        let send = Button::new(&label)
            .icon(icons::PAPER_PLANE_TILT)
            .variant(Variant::Primary)
            .size(Size::Sm)
            .enabled(!busy && (n > 0 || (!mine && self.request_public)))
            .id_salt("share-send");
        let cancel = Button::new("Cancel").variant(Variant::Ghost).size(Size::Sm).enabled(!busy).id_salt("share-cancel");
        let (sw, cw) = (send.desired_width(ui), cancel.desired_width(ui));
        let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        if pressed(send.show_at(ui, Rect::from_min_size(pos2(r.right() - sw, r.top()), vec2(sw, 32.0)))) {
            *act = Some(Act::Send);
        }
        if pressed(cancel.show_at(ui, Rect::from_min_size(pos2(r.right() - sw - 8.0 - cw, r.top()), vec2(cw, 32.0)))) {
            *act = Some(Act::CancelPicker);
        }
    }

    /// Copy public link: what it says now (copied for a moment, making it public, or the offer) at one width for all three,
    /// so the row never jumps.
    fn copy_button(&self, ui: &Ui, busy: bool, enabled: bool) -> Button<'static> {
        let now = anim::time(ui.ctx());
        let copied = self.copied_at.is_some_and(|at| now - at < COPIED_FOR);
        if let Some(at) = self.copied_at.filter(|_| copied) {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((at + COPIED_FOR - now).max(0.0)));
        }
        let looks = [(icons::LINK_SIMPLE, "Copy public link"), (icons::LINK_SIMPLE, "Making it public…"), (icons::CHECK, "Link copied")];
        let w = looks.iter().map(|(glyph, label)| Button::new(label).icon(glyph).size(Size::Sm).desired_width(ui)).fold(0.0, f32::max);
        let (glyph, label) = looks[if copied {
            2
        } else if busy && self.copy_after {
            1
        } else {
            0
        }];
        let public = self.entry.remote.as_ref().is_some_and(|i| i.public);
        Button::new(label).icon(glyph).variant(if copied { Variant::Primary } else { Variant::Secondary }).enabled(copied || (enabled && !busy)).width(w).hint(
            if public {
                "Copies the address of its page on Orbiters: Discord and other apps show its picture with it"
            } else {
                "Shares it publicly on Orbiters, then copies the address of its page"
            },
        )
    }

    /// Its link was just copied (screenshots).
    pub fn stage_copied(&mut self, ctx: &egui::Context) {
        self.copied_at = Some(anim::time(ctx));
        self.notice = Some("Public link copied: paste it in Discord or anywhere, its picture shows with it".into());
    }

    /// The capture's public page (contracts `Docs/capture-sharing.md`, Public page), once it is public.
    fn public_link(&self, server: Server) -> Option<String> {
        let item = self.entry.remote.as_ref().filter(|i| i.public && i.kind == shares::Kind::Capture)?;
        let world = if item.world.id.is_empty() { &self.entry.world_id } else { &item.world.id };
        Some(public_link(server, world, item.id))
    }

    fn copy_link(&mut self, ctx: &egui::Context, server: Server) {
        if let Some(link) = self.public_link(server) {
            ctx.copy_text(link);
            self.copied_at = Some(anim::time(ctx));
            self.notice = Some("Public link copied: paste it in Discord or anywhere, its picture shows with it".into());
        }
    }

    fn act(&mut self, ctx: &egui::Context, cx: &mut Cx, server: Server, a: Act) {
        match a {
            Act::Public => self.public(ctx, server),
            Act::ShareTo => self.picker = !self.picker,
            Act::Download => self.start(ctx, server, Action::Download),
            Act::View3d => match &self.entry.local {
                Some(controller::Local::Session(path)) => {
                    if let Some(gpu) = cx.gpu {
                        self.viewer = Some(super::splats::Viewer::open(ctx, gpu, path, self.entry.world_name.clone(), super::splats::on_integrated(cx)));
                    }
                }
                _ => self.start(ctx, server, Action::Download),
            },
            Act::Reconstruct => {
                if let Some(controller::Local::Session(path)) = &self.entry.local {
                    cx.ctl.reconstruct(path.clone());
                    self.notice = Some("Reconstruction started: share it once its 3D view is ready".into());
                }
            }
            Act::Folder => {
                if let Some(local) = &self.entry.local {
                    let dir = match local {
                        controller::Local::Picture(p) => p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| p.clone()),
                        controller::Local::Session(d) => d.clone(),
                    };
                    let r = crate::controller::open_path(&dir);
                    cx.run(ctx, r);
                }
            }
            Act::Full => self.full = !self.full,
            Act::Edit => self.edit(ctx),
            Act::Unshare(u) => self.start(ctx, server, Action::Unshare(u)),
            Act::Send => {
                let recipients = self
                    .friends_list
                    .iter()
                    .filter(|f| self.selected.contains(&f.id))
                    .map(|f| shares::Recipient { vrchat_user_id: f.id.clone(), display_name: f.name.clone() })
                    .collect();
                self.start(ctx, server, Action::Share(Share { public: self.request_public, recipients, message: self.message.clone(), ..Default::default() }));
            }
            Act::CancelPicker => self.picker = false,
            Act::Anyway => {
                self.warning = None;
                let mut r = match &self.last {
                    Some(Action::Share(r)) => r.clone(),
                    _ => Share { public: true, ..Default::default() },
                };
                r.public = true;
                r.acknowledge_explicit = true;
                self.start(ctx, server, Action::Share(r));
            }
            Act::CancelWarning => {
                self.warning = None;
                self.copy_after = false;
            }
            Act::CopyLink => {
                // Copied at once when its page shows it with its picture; otherwise it is shared publicly (and given its
                // picture) first, by the same job as Share publicly, with the same explicit warning.
                let item = self.entry.remote.as_ref();
                let pictured = item.and_then(|i| i.capture.as_ref()).is_some_and(|c| c.picture_url.is_some())
                    || !self.entry.mine()
                    || !matches!(self.entry.local, Some(controller::Local::Session(_)));
                if item.is_some_and(|i| i.public) && pictured {
                    self.copy_link(ctx, server);
                } else {
                    self.copy_after = true;
                    if self.entry.mine() && item.is_some_and(|i| i.public) {
                        self.start(ctx, server, Action::Share(Share { public: true, ..Default::default() }));
                    } else {
                        self.public(ctx, server);
                    }
                }
            }
            Act::Retry => {
                if let Some(a) = self.last.clone() {
                    self.start(ctx, server, a);
                }
            }
        }
    }
}

/// How long Copy public link says the link was copied.
const COPIED_FOR: f64 = 2.4;

/// A public 3D capture's page on the website: its world's page opened on it, which crawlers preview with its picture.
pub fn public_link(server: Server, world_id: &str, id: u64) -> String {
    format!("{}maps/{world_id}/captures/{id}", server.website())
}

/// What the window's controls asked for this frame.
#[derive(Clone)]
enum Act {
    Public,
    ShareTo,
    Download,
    View3d,
    Reconstruct,
    Folder,
    Full,
    Unshare(Unshare),
    Send,
    CancelPicker,
    Anyway,
    CancelWarning,
    Retry,
    CopyLink,
    Edit,
}

impl Act {
    fn tag(&self) -> u8 {
        match self {
            Act::Public => 0,
            Act::ShareTo => 1,
            Act::Download => 2,
            Act::Reconstruct => 3,
            Act::CopyLink => 4,
            Act::Edit => 5,
            _ => 9,
        }
    }
}

/// A line of work going on: a spinner and what it does (the sharing window, the VRChat sign-in).
pub(super) fn working(ui: &mut Ui, text: &str) {
    working_part(ui, text, None);
}

/// [`working`], its frame filling up to `part` (0 to 1) of its width when how far it is is known.
fn working_part(ui: &mut Ui, text: &str, part: Option<f32>) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, r, 14.0, theme::alpha(theme::ACCENT, 0.07));
    if let Some(part) = part {
        let id = ui.scope_id().with("working-part");
        let k = anim::spring(ui.ctx(), id, [part], Spring::SOFT)[0].clamp(0.0, 1.0);
        let filled = Rect::from_min_size(r.min, vec2(r.width() * k, r.height()));
        p.with_clip_rect(filled.intersect(p.clip_rect())).rect_filled(r, theme::radius(14.0), theme::alpha(theme::ACCENT, 0.12));
    }
    feedback::spinner(p, pos2(r.left() + 22.0, r.center().y), 7.0, 2.0, theme::ACCENT);
    widgets::text_truncated(p, pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 12.5).weight(weight::MEDIUM), w - 56.0);
}

/// How far a share is: its files sent, or its items shared.
fn sent_part(stage: Stage) -> Option<f32> {
    match stage {
        Stage::Sending { sent, total } if sent > 0 && total > 0 => Some((sent as f64 / total as f64).clamp(0.0, 1.0) as f32),
        Stage::Items { done, total } if total > 0 => Some(done as f32 / total as f32),
        _ => None,
    }
}

/// "31.6 MB", "820 KB".
fn megabytes(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{} KB", bytes.div_ceil(1000))
    }
}

/// Who a share goes to: "with Alex", "with 3 friends", "publicly", "publicly and with 2 friends".
fn whom(s: &Share) -> String {
    let friends = match s.recipients.as_slice() {
        [] => String::new(),
        [one] => format!("with {}", one.display_name),
        many => format!("with {} friends", many.len()),
    };
    match (s.public, friends.is_empty()) {
        (true, true) => "publicly".into(),
        (true, false) => format!("publicly and {friends}"),
        _ => friends,
    }
}

/// Wrapped text across `w`.
fn paragraph(ui: &mut Ui, text: &str, size: f32, color: Color32, w: f32) {
    let g = ui.painter().layout_job(Txt::new(text, size).color(color).line_height(size * 1.5).job(w));
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, color);
}

/// The Share button that splits into Share publicly and Share to … (Visited worlds' cards and session rows). Its
/// surface stretches to the left on a bouncy spring and parts in two, the left half turning Orbiters green, the
/// labels crossfading; a press elsewhere or Escape folds it back. Returns `Some(true)` for Share publicly,
/// `Some(false)` for Share to ….
pub fn morph(ui: &mut Ui, id: Id, right: egui::Pos2, expanded_width: f32, enabled: bool) -> Option<bool> {
    let ctx = ui.ctx().clone();
    let mut expanded = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let k = anim::spring(&ctx, id.with("split"), [if expanded { 1.0 } else { 0.0 }], Spring { stiffness: 340.0, damping: 22.0, mass: 0.8 })[0];
    let kc = k.clamp(0.0, 1.0);
    let share = Button::new("Share").icon(icons::CLOUD_ARROW_UP).variant(Variant::Secondary).enabled(enabled && !expanded).id_salt(id.with("share"));
    let small_w = share.desired_width(ui);
    let small = Rect::from_min_size(pos2(right.x - small_w, right.y), vec2(small_w, 40.0));
    let full = Rect::from_min_size(pos2(right.x - expanded_width, right.y), vec2(expanded_width, 40.0));
    if expanded && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| !full.contains(p)))
    {
        expanded = false;
    }
    let gap = 8.0;
    let half = (expanded_width - gap) * 0.5;
    let a = viewer::lerp_rect(small, Rect::from_min_size(full.min, vec2(half, 40.0)), k);
    let b = viewer::lerp_rect(small, Rect::from_min_size(pos2(full.left() + half + gap, full.top()), vec2(half, 40.0)), k);
    let mut result = None;
    if kc < 0.01 {
        if pressed(share.show_at(ui, small)) {
            expanded = true;
        }
    } else {
        // The two halves: one surface at first, parting as they reach their places.
        let public_label = "Share publicly";
        let to_label = "Share to …";
        let with_icons = Button::new(public_label).icon(icons::GLOBE).desired_width(ui) <= half;
        let mut pa = Button::new(public_label).variant(Variant::Primary).enabled(enabled && expanded).width(half).id_salt(id.with("public"));
        let mut pb = Button::new(to_label).variant(Variant::Secondary).enabled(enabled && expanded).width(half).id_salt(id.with("to"));
        if with_icons {
            pa = pa.icon(icons::GLOBE);
            pb = pb.icon(icons::USERS);
        }
        if kc > 0.98 {
            if pressed(pa.show_at(ui, a)) {
                result = Some(true);
            }
            if pressed(pb.show_at(ui, b)) {
                result = Some(false);
            }
        } else {
            // While they part the halves overlap: the green one lies over the grey one, opaque (the grey of a
            // secondary button on a card, mixed rather than see-through), and each label stays inside its own half.
            let p = ui.painter();
            let grey = theme::mix(theme::INK_850, theme::INK_600, 0.55);
            let inn = ((kc - 0.45) / 0.55).clamp(0.0, 1.0);
            let label = |r: Rect, text: &str, glyph: &str, fg: Color32| {
                let mut lp = p.with_clip_rect(r.shrink(2.0).intersect(p.clip_rect()));
                lp.multiply_opacity(inn);
                let t = Txt::new(text, 13.5).weight(weight::MEDIUM).color(fg);
                let tw = widgets::text_size(ui, &t).x + if with_icons { 23.0 } else { 0.0 };
                let x = r.center().x - tw * 0.5 + 10.0 * (1.0 - inn);
                if with_icons {
                    widgets::icon(&lp, pos2(x + 7.5, r.center().y), glyph, 15.0, fg);
                }
                widgets::text(&lp, pos2(x + if with_icons { 23.0 } else { 0.0 }, r.center().y), Align2::LEFT_CENTER, &t);
            };
            p.rect_filled(b, theme::radius(20.0), theme::control());
            if inn > 0.0 {
                // Centred on what the green half leaves of it.
                label(Rect::from_min_max(pos2(b.left().max(a.right()), b.top()), b.max), to_label, icons::USERS, theme::FG);
            }
            p.rect_filled(a, theme::radius(20.0), theme::mix(grey, theme::ACCENT, kc));
            // "Share" leaves from where the button was, the halves' labels come in.
            let out = (1.0 - kc * 3.0).clamp(0.0, 1.0);
            if out > 0.0 {
                let mut lp = p.with_clip_rect(a.intersect(p.clip_rect()));
                lp.multiply_opacity(out);
                widgets::icon(&lp, pos2(small.left() + 18.0 + 7.5, small.center().y), icons::CLOUD_ARROW_UP, 15.0, theme::FG);
                widgets::text(&lp, pos2(small.left() + 18.0 + 23.0, small.center().y), Align2::LEFT_CENTER, &Txt::new("Share", 13.5).weight(weight::MEDIUM));
            }
            if inn > 0.0 {
                label(a, public_label, icons::GLOBE, theme::ON_ACCENT);
            }
            let _ = ui.interact(full, id.with("moving"), Sense::click());
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, expanded));
    result
}
