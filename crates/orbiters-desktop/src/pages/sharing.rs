//! One sharing surface used by Photos & captures and Visited worlds.
use super::{Cx, Job};
use crate::controller::shares::{self as controller, Entry, Operation as Action, Outcome, Request};
use egui::{pos2, vec2, Align2, Id, Rect, Ui, UiBuilder};
use orbiters_core::{
    account::Server,
    shares::{self, Share, Shared, Unshare},
    vrchat_accounts::Friend,
};
use orbiters_ui::{
    anim::{self, Spring},
    icons,
    theme::{self, Txt},
    widgets::{self, Button, IconButton, Variant},
};
use std::collections::BTreeSet;

/// Use the shared press activation, including the normal keyboard/accessibility click fallback.
pub fn pressed(response: egui::Response) -> bool {
    response.enabled() && widgets::activated(&response)
}
pub struct Dialog {
    pub entry: Entry,
    pub extra: Vec<Entry>,
    pub picker: bool,
    pub warning: Option<String>,
    friends: Job<Vec<Friend>>,
    friends_list: Vec<Friend>,
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
    request_public: bool,
    large_picture: bool,
    pub changed: bool,
    pictures: crate::worlds::Pictures,
    viewer: Option<super::splats::Viewer>,
}
impl Dialog {
    pub fn new(ctx: &egui::Context, entry: Entry, picker: bool) -> Self {
        Self {
            entry,
            extra: Vec::new(),
            picker,
            warning: None,
            friends: Job::spawn(ctx, || orbiters_core::vrchat_accounts::friends(None)),
            friends_list: Vec::new(),
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
            request_public: false,
            large_picture: false,
            changed: false,
            pictures: crate::worlds::Pictures::new(ctx),
            viewer: None,
        }
    }
    pub fn loading(&self) -> bool {
        self.pictures.loading()
    }
    pub fn fixture(&mut self, state: &str) {
        self.fixture_active = true;
        if state == "friends" {
            self.picker = true;
            self.friends = Job::default();
            self.friends_list = ["Alex", "Mika", "Robin", "Sora", "Taylor"]
                .iter()
                .enumerate()
                .map(|(i, n)| Friend { id: format!("usr_{i}"), name: (*n).into(), image: None })
                .collect();
            self.selected.insert("usr_0".into());
            self.selected.insert("usr_2".into());
            self.message = "A little moment from last night ✨".into();
        }
        if state == "explicit" {
            self.warning = Some(shares::EXPLICIT_WARNING.into());
        }
        if state == "progress" {
            self.fixture_busy = true;
        }
        if state == "picture" {
            self.large_picture = true;
        }
        if state == "error" {
            self.error = Some("Orbiters could not be reached. Your capture is still on this computer.".into());
            self.last = Some(Action::Share(Share { public: true, ..Default::default() }));
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
        let id = uuid::Uuid::new_v4().to_string();
        self.operation_id = Some(id.clone());
        let request = Request { id, server, entry, extra, operation: action };
        if let Some(backend) = &self.backend {
            backend.capture_share(request);
        } else {
            self.pending = Some(request);
        }
    }
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx) -> bool {
        let ctx = ui.ctx().clone();
        if !self.fixture_active && self.entry.local.is_none() {
            if let Some(item) = self.entry.remote.as_ref().filter(|i| i.picture.is_some()) {
                self.entry.picture = Some(crate::worlds::Picture::Capture { development: cx.snap.settings.development, id: item.id, large: true });
            }
        }
        let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
        if let Some(f) = self.friends.take() {
            self.friends_list = f;
        }
        self.backend = Some(cx.ctl.clone());
        if let Some(request) = self.pending.take() {
            cx.ctl.capture_share(request);
        }
        let done = self.operation_id.as_ref().and_then(|id| cx.snap.capture_shares.iter().find(|j| &j.id == id)).and_then(|j| j.outcome.clone());
        if let Some(done) = done {
            self.operation_id = None;
            match done {
                Outcome::Shared(Shared::Item { item }) | Outcome::Unshared(item) => {
                    self.entry.remote = Some(item);
                    self.changed = true;
                    self.picker = false;
                    self.notice = Some("Sharing updated".into());
                }
                Outcome::Shared(Shared::Request { .. }) => {
                    self.picker = false;
                    self.notice = Some("Share request sent to the person who shared this with you".into());
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
                                &ctx,
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
                    if explicit {
                        self.warning = Some(message);
                    } else {
                        self.error = Some(message);
                    }
                    if let Some(entry) = self.rollback.take() {
                        self.entry = entry;
                    }
                }
            }
        }
        let mut close = false;
        let busy = self.operation_id.is_some() || self.fixture_busy;
        let modal = egui::Modal::new(Id::unique(("capture-sharing", &self.entry.key)))
            .frame(egui::Frame::NONE.fill(theme::INK_900).corner_radius(24).inner_margin(24))
            .show(&ctx, |ui| {
                ui.set_width(880.0f32.min(ctx.content_rect().width() - 100.0));
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&self.entry.world_name).size(23.0).strong());
                        let date = super::local_time(&controller::iso(self.entry.taken));
                        let sub = if self.extra.is_empty() { date } else { format!("{} pictures, {date}", self.extra.len() + 1) };
                        ui.label(egui::RichText::new(sub).size(12.0).color(theme::FG_3));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if IconButton::new(icons::X, "Close").show(ui).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(20.0);
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    let width = ui.available_width();
                    let (r, response) = ui.allocate_exact_size(vec2(width, width * 0.72), egui::Sense::click());
                    if self.entry.is_picture() && pressed(response) {
                        self.large_picture = true;
                    }
                    let picture = match &self.entry.local {
                        Some(controller::Local::Picture(path)) => Some(crate::worlds::Picture::Shot { path: path.clone(), size: 0, width: 2048 }),
                        _ => self.entry.picture.clone(),
                    };
                    let tex = picture.as_ref().and_then(|p| self.pictures.get(&ctx, p));
                    super::sessions::thumb_shape(ui.painter(), r, theme::radius(16.0), tex.as_ref(), 1.0);
                    if self.entry.is_picture() {
                        ui.add_space(10.0);
                        if pressed(Button::new("View full size").icon(icons::ARROW_SQUARE_OUT).width(width).show(ui)) {
                            self.large_picture = true;
                        }
                    }
                    if !self.entry.is_picture() {
                        ui.add_space(10.0);
                        if let Some(controller::Local::Session(path)) = &self.entry.local {
                            if pressed(Button::new("View in 3D").icon(icons::CUBE_FOCUS).width(width).enabled(self.entry.ready).show(ui)) {
                                if let Some(gpu) = cx.gpu {
                                    self.viewer =
                                        Some(super::splats::Viewer::open(&ctx, gpu, path, self.entry.world_name.clone(), super::splats::on_integrated(cx)));
                                }
                            }
                        }
                    }
                    if !self.entry.is_picture() && self.entry.local.is_none() {
                        ui.add_space(10.0);
                        if pressed(
                            Button::new(if busy && matches!(self.last, Some(Action::Download)) { "Downloading..." } else { "View in 3D" })
                                .icon(icons::CUBE_FOCUS)
                                .enabled(!busy)
                                .width(width)
                                .show(ui),
                        ) {
                            self.start(&ctx, server, Action::Download);
                        }
                    }
                    let ui = &mut cols[1];
                    if busy {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(14.0).color(theme::FG_2));
                            ui.label(
                                egui::RichText::new(match self.last {
                                    Some(Action::Download) => "Downloading...",
                                    Some(Action::Unshare(_)) => "Updating sharing...",
                                    _ => "Sharing...",
                                })
                                .color(theme::FG_2),
                            );
                        });
                        ui.add_space(8.0);
                    }
                    if self.picker {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(if self.entry.mine() { "Share with friends" } else { "Request permission to share" }).size(18.0).strong());
                        ui.add_space(10.0);
                        widgets::search_field(ui, "friend-search", &mut self.query, "Search VRChat friends", ui.available_width());
                        ui.add_space(8.0);
                        let friends = self.friends_list.clone();
                        let q = self.query.to_lowercase();
                        egui::ScrollArea::vertical().id_salt("friend-picker").auto_shrink([false, false]).max_height(160.0).show(ui, |ui| {
                            for f in friends.iter().filter(|f| f.name.to_lowercase().contains(&q)) {
                                ui.horizontal(|ui| {
                                    let mut chosen = self.selected.contains(&f.id);
                                    if ui.checkbox(&mut chosen, "").changed() {
                                        if chosen {
                                            if self.selected.len() < 50 {
                                                self.selected.insert(f.id.clone());
                                            }
                                        } else {
                                            self.selected.remove(&f.id);
                                        }
                                    }
                                    let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
                                    let tex = f.image.as_ref().and_then(|url| {
                                        self.pictures.get(&ctx, &crate::worlds::Picture::Remote { world_id: f.id.clone(), url: url.clone(), large: false })
                                    });
                                    avatar(ui, r, tex.as_ref(), &f.name);
                                    ui.label(&f.name);
                                });
                            }
                            if self.friends.running() {
                                ui.label("Reading friends from VRCX / VRCNext…");
                            } else if friends.is_empty() {
                                ui.label("No cached VRChat friends. Open VRCX or VRCNext and let its friend list load.");
                            }
                        });
                        ui.add_space(8.0);
                        if !self.entry.mine() {
                            ui.checkbox(&mut self.request_public, "Request public sharing");
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.message)
                                .hint_text("Add a message (optional)")
                                .char_limit(280)
                                .desired_width(ui.available_width()),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if pressed(
                                Button::new(&format!("{} · {} selected", if self.entry.mine() { "Share" } else { "Request share" }, self.selected.len()))
                                    .variant(Variant::Primary)
                                    .enabled(!busy && (!self.selected.is_empty() || self.request_public))
                                    .show(ui),
                            ) {
                                let recipients = friends
                                    .iter()
                                    .filter(|f| self.selected.contains(&f.id))
                                    .map(|f| shares::Recipient { vrchat_user_id: f.id.clone(), display_name: f.name.clone() })
                                    .collect();
                                self.start(
                                    &ctx,
                                    server,
                                    Action::Share(Share { public: self.request_public, recipients, message: self.message.clone(), ..Default::default() }),
                                );
                            }
                            if pressed(Button::new("Cancel").variant(Variant::Ghost).enabled(!busy).show(ui)) {
                                self.picker = false;
                            }
                        });
                    } else {
                        ui.add_space(4.0);
                        if let Some(from) = self.entry.remote.as_ref().and_then(|i| i.from.as_ref()) {
                            ui.label(egui::RichText::new(format!("From {}", from.name)).color(theme::FG_2));
                            ui.add_space(12.0);
                        }
                        let mine = self.entry.mine();
                        let hidden = self.entry.remote.as_ref().is_some_and(|i| i.hidden.is_some());
                        ui.label(egui::RichText::new(if mine { "Choose who can see this" } else { "Shared with you" }).size(17.0).strong());
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(if mine {
                                "Public captures appear on Orbiters. Sharing to friends keeps them private."
                            } else {
                                "You can open and download this item. Ask the person who shared it before sharing it further."
                            })
                            .size(12.5)
                            .color(theme::FG_3),
                        );
                        ui.add_space(16.0);
                        if hidden {
                            ui.label("Hidden by a moderator. Sharing is unavailable.");
                        }
                        let enabled = !busy && self.entry.ready && !hidden && self.warning.is_none();
                        if mine {
                            let public = self.entry.remote.as_ref().is_some_and(|i| i.public);
                            if pressed(
                                Button::new(if busy {
                                    "Updating…"
                                } else if public {
                                    "Unshare publicly"
                                } else {
                                    "Share publicly"
                                })
                                .icon(icons::GLOBE)
                                .variant(if public || self.warning.is_some() { Variant::Secondary } else { Variant::Primary })
                                .enabled(enabled)
                                .width(ui.available_width())
                                .show(ui),
                            ) {
                                if public {
                                    self.start(&ctx, server, Action::Unshare(Unshare { public: true, ..Default::default() }));
                                } else {
                                    self.public(&ctx, server);
                                }
                            }
                            ui.add_space(8.0);
                            if pressed(Button::new("Share to …").icon(icons::USERS).enabled(enabled).width(ui.available_width()).show(ui)) {
                                self.picker = true;
                            }
                        } else {
                            if pressed(
                                Button::new(if busy { "Sending request…" } else { "Request share" })
                                    .icon(icons::CLOUD_ARROW_UP)
                                    .variant(Variant::Primary)
                                    .enabled(enabled)
                                    .width(ui.available_width())
                                    .show(ui),
                            ) {
                                self.picker = true;
                            }
                            ui.add_space(8.0);
                            if pressed(Button::new("Download / open").icon(icons::DOWNLOAD_SIMPLE).enabled(!busy).width(ui.available_width()).show(ui)) {
                                self.start(&ctx, server, Action::Download);
                            }
                        }
                        let grants = self.entry.remote.as_ref().map(|i| i.grants.clone()).unwrap_or_default();
                        if !grants.is_empty() {
                            ui.add_space(18.0);
                            ui.label(egui::RichText::new("SHARED WITH").size(10.0).color(theme::FG_4));
                            egui::ScrollArea::vertical().id_salt("grants").max_height(120.0).show(ui, |ui| {
                                for g in grants {
                                    ui.horizontal(|ui| {
                                        ui.label(g.display_name.as_deref().unwrap_or(&g.vrchat_user_id));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if pressed(Button::new("Revoke").variant(Variant::Ghost).enabled(!busy).id_salt(g.id).show(ui)) {
                                                self.start(
                                                    &ctx,
                                                    server,
                                                    Action::Unshare(Unshare { recipients: vec![g.vrchat_user_id.clone()], ..Default::default() }),
                                                );
                                            }
                                        });
                                    });
                                }
                            });
                        }
                        if mine && self.entry.remote.as_ref().is_some_and(|i| i.public || !i.grants.is_empty()) {
                            ui.add_space(10.0);
                            if pressed(Button::new("Unshare everything").variant(Variant::Ghost).enabled(!busy).show(ui)) {
                                self.start(&ctx, server, Action::Unshare(Unshare { all: true, ..Default::default() }));
                            }
                        }
                    }
                    if let Some(n) = &self.notice {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(n).color(theme::FG_2));
                    }
                    if let Some(e) = &self.error {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new(e).color(egui::Color32::from_rgb(248, 150, 150)));
                        if let Some(a) = self.last.clone() {
                            if pressed(Button::new("Retry").enabled(!busy).show(ui)) {
                                self.start(&ctx, server, a);
                            }
                        }
                    }
                });

                if let Some(warning) = self.warning.clone() {
                    ui.add_space(20.0);
                    egui::Frame::NONE.fill(theme::surface()).corner_radius(14).inner_margin(16).show(ui, |ui| {
                        ui.label(egui::RichText::new("Before you share publicly").strong());
                        ui.add_space(8.0);
                        ui.label(&warning);
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if pressed(Button::new("Share anyway").variant(Variant::Primary).enabled(!busy).show(ui)) {
                                self.warning = None;
                                let mut r = match &self.last {
                                    Some(Action::Share(r)) => r.clone(),
                                    _ => Share { public: true, ..Default::default() },
                                };
                                r.public = true;
                                r.acknowledge_explicit = true;
                                self.start(&ctx, server, Action::Share(r));
                            }
                            if pressed(Button::new("Cancel").variant(Variant::Ghost).show(ui)) {
                                self.warning = None;
                            }
                        });
                    });
                }
            });
        if self.large_picture {
            let response = egui::Modal::new(Id::unique("capture-full-picture"))
                .frame(egui::Frame::NONE.fill(theme::INK_900).corner_radius(20).inner_margin(16))
                .show(&ctx, |ui| {
                    let size = vec2((ctx.content_rect().width() - 120.0).max(200.0), (ctx.content_rect().height() - 150.0).max(150.0));
                    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let picture = match &self.entry.local {
                        Some(controller::Local::Picture(path)) => Some(crate::worlds::Picture::Shot { path: path.clone(), size: 0, width: 2048 }),
                        _ => self.entry.picture.clone(),
                    };
                    let texture = picture.as_ref().and_then(|p| self.pictures.get(&ctx, p));
                    if let Some(t) = texture {
                        let scale = (r.width() / t.size_vec2().x).min(r.height() / t.size_vec2().y);
                        let image = Rect::from_center_size(r.center(), t.size_vec2() * scale);
                        ui.painter().image(t.id(), image, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                    }
                    if Button::new("Close picture").show(ui).clicked() {
                        self.large_picture = false;
                    }
                });
            if response.should_close() {
                self.large_picture = false;
            }
            return false;
        }
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
        close || (!busy && modal.should_close())
    }
}
pub fn avatar(ui: &Ui, r: Rect, tex: Option<&egui::TextureHandle>, name: &str) {
    ui.painter().rect_filled(r, theme::radius(r.width() * 0.5), theme::surface());
    if let Some(t) = tex {
        super::sessions::thumb_shape(ui.painter(), r, theme::radius(r.width() * 0.5), Some(t), 1.0);
    } else {
        widgets::text(ui.painter(), r.center(), Align2::CENTER_CENTER, &Txt::new(name.chars().next().unwrap_or('?').to_string(), 12.0).color(theme::FG_3));
    }
}

/// Spring width, cross-faded labels and separating halves, matching LoginProviderButton's 340/25/0.8 spring.
pub fn morph(ui: &mut Ui, id: Id, right: egui::Pos2, expanded_width: f32, enabled: bool) -> Option<bool> {
    let mut expanded = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let k =
        anim::spring(ui.ctx(), id.with("split"), [if expanded { 1.0 } else { 0.0 }], Spring { stiffness: 340.0, damping: 25.0, mass: 0.8 })[0].clamp(0.0, 1.0);
    let width = 96.0 + (expanded_width - 96.0) * k;
    let r = Rect::from_min_size(pos2(right.x - width, right.y), vec2(width, 40.0));
    if expanded
        && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| !r.contains(p)))
    {
        expanded = false;
    }
    let mut result = None;
    // The surfaces remain visible while the labels cross-fade; fading whole buttons made the middle disappear.
    if k > 0.01 {
        let gap = 8.0 * k;
        let w = (width - gap) * 0.5;
        let a = Rect::from_min_size(r.min, vec2(w, 40.0));
        let b = Rect::from_min_size(pos2(a.right() + gap, r.top()), vec2(w, 40.0));
        ui.painter().rect_filled(a, theme::radius(20.0), theme::mix(theme::INK_750, theme::ACCENT, k));
        ui.painter().rect_filled(b, theme::radius(20.0), theme::INK_750);
    }
    if k < 0.98 {
        let mut child = ui.new_child(UiBuilder::new().max_rect(r).id_salt(id));
        child.multiply_opacity((1.0 - k * 3.0).clamp(0.0, 1.0));
        if pressed(
            Button::new("Share").icon(icons::CLOUD_ARROW_UP).enabled(enabled && !expanded).variant(Variant::Secondary).width(width).show_at(&mut child, r),
        ) {
            expanded = true;
        }
    }
    if k > 0.01 {
        let gap = 8.0 * k;
        let w = (width - gap) * 0.5;
        let mut child = ui.new_child(UiBuilder::new().max_rect(r).id_salt(id.with("halves")));
        child.multiply_opacity(((k - 0.55) / 0.45).clamp(0.0, 1.0));
        let a = Rect::from_min_size(r.min, vec2(w, 40.0));
        let b = Rect::from_min_size(pos2(a.right() + gap, r.top()), vec2(w, 40.0));
        if pressed(Button::new("Share publicly").variant(Variant::Primary).enabled(enabled && expanded).width(w).show_at(&mut child, a)) {
            result = Some(true);
        }
        if pressed(Button::new("Share to …").enabled(enabled && expanded).width(w).show_at(&mut child, b)) {
            result = Some(false);
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, expanded));
    result
}
