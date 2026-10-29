//! Incoming share requests are answered by the tray, using the same explicit acknowledgement as public sharing.
use super::{sharing, Cx, Job};
use crate::controller::shares::{self as controller, Operation, Outcome, Request};
use orbiters_core::{
    account::Server,
    shares::{self, Item, RequestStatus, ShareRequest},
};
use orbiters_ui::{
    icons, theme,
    widgets::{self, Button, IconButton, SegOption, Variant},
};

#[derive(Default)]
pub struct State {
    pub open: bool,
    incoming: Vec<ShareRequest>,
    outgoing: Vec<ShareRequest>,
    outgoing_box: bool,
    loaded: Option<(bool, String, u64)>,
    list: Job<anyhow::Result<(shares::Requests, shares::Requests)>>,
    refreshed: f64,
    error: Option<String>,
    row_error: Option<(u64, String)>,
    active: Option<(String, ShareRequest)>,
    last: Option<(ShareRequest, bool, bool)>,
    warning: Option<(ShareRequest, String)>,
    fixture: bool,
    pictures: Option<crate::worlds::Pictures>,
}
impl State {
    pub fn pending(&self) -> usize {
        self.incoming.iter().filter(|r| r.status == RequestStatus::Pending).count()
    }
    pub fn busy(&self) -> bool {
        self.list.running()
    }
    pub fn stage(&mut self, name: &str) {
        self.fixture = true;
        self.list = Job::default();
        let fixture: shares::Requests = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/capture-requests.json")).unwrap();
        self.incoming = fixture.requests;
        if name.starts_with("gallery-requests") {
            self.open = true;
        }
        if name == "gallery-requests-explicit" {
            let r = &mut self.incoming[1];
            r.item.as_mut().unwrap().explicit = true;
            self.warning = Some((r.clone(), shares::EXPLICIT_WARNING.into()));
        }
        if name == "gallery-requests-outgoing" {
            self.outgoing_box = true;
            self.outgoing = self.incoming.clone();
            for request in &mut self.outgoing {
                std::mem::swap(&mut request.requester, &mut request.approver);
            }
        }
        if name == "gallery-requests-denied" {
            self.incoming[0].status = RequestStatus::Denied;
            self.incoming[1].status = RequestStatus::Cancelled;
            self.incoming[1].item = None;
        }
        if name == "gallery-requests-approved" {
            self.incoming[0].status = RequestStatus::Approved;
            self.incoming[0].answered_at = Some("2026-10-29T01:00:00Z".into());
        }
        if name == "gallery-requests-error" {
            let r = self.incoming[0].clone();
            self.row_error = Some((r.id, "Orbiters could not be reached. This request is still pending.".into()));
            self.last = Some((r, true, false));
        }
    }
    fn refresh(&mut self, ctx: &egui::Context, server: Server) {
        self.refreshed = orbiters_ui::anim::time(ctx);
        self.error = None;
        self.list = Job::spawn(ctx, move || {
            let (api, token) = controller::api(server)?;
            Ok((api.capture_requests(&token, true)?, api.capture_requests(&token, false)?))
        });
    }
    pub fn poll(&mut self, ctx: &egui::Context, cx: &Cx) {
        let key = (cx.snap.settings.development, format!("{:?}", cx.snap.account.state), cx.visit);
        if self.loaded.as_ref().is_some_and(|old| old.0 != key.0 || old.1 != key.1) {
            self.list = Job::default();
            self.loaded = None;
            self.pictures = None;
            self.incoming.clear();
            self.outgoing.clear();
            self.open = false;
            self.warning = None;
            self.row_error = None;
            self.active = None;
        }
        let server = if key.0 { Server::Development } else { Server::Production };
        let now = orbiters_ui::anim::time(ctx);
        if !self.fixture && self.active.is_none() && !self.list.running() && (self.loaded.as_ref() != Some(&key) || now - self.refreshed >= 60.0) {
            self.loaded = Some(key);
            self.refresh(ctx, server);
        }
        if let Some(result) = self.list.take() {
            match result {
                Ok((incoming, outgoing)) => {
                    self.incoming = incoming.requests;
                    self.outgoing = outgoing.requests;
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        let result = self.active.as_ref().and_then(|(id, _)| cx.snap.capture_shares.iter().find(|j| &j.id == id)).and_then(|j| j.outcome.clone());
        if let Some(outcome) = result {
            let (_, before) = self.active.take().unwrap();
            match outcome {
                Outcome::Answered(request) => {
                    for r in self.incoming.iter_mut().chain(self.outgoing.iter_mut()) {
                        if r.id == request.id {
                            *r = request.clone();
                        }
                    }
                    self.row_error = None;
                }
                Outcome::Failed { message, explicit } => {
                    for r in &mut self.incoming {
                        if r.id == before.id {
                            *r = before.clone();
                        }
                    }
                    if explicit {
                        self.warning = Some((before, message));
                    } else {
                        self.row_error = Some((before.id, message));
                    }
                }
                _ => self.row_error = Some((before.id, "Unexpected answer from the sharing service.".into())),
            }
        }
    }
    fn answer(&mut self, cx: &Cx, request: ShareRequest, approve: bool, acknowledge_explicit: bool) {
        if self.active.is_some() {
            return;
        }
        if approve && request.public && request.item.as_ref().is_some_and(|i| i.explicit) && !acknowledge_explicit {
            self.warning = Some((request, shares::EXPLICIT_WARNING.into()));
            return;
        }
        self.list = Job::default();
        self.row_error = None;
        self.last = Some((request.clone(), approve, acknowledge_explicit));
        let id = uuid::Uuid::new_v4().to_string();
        // Null item means access was lost. Denying still addresses the request id; no item metadata is inferred.
        let entry = controller::remote_entry(request.item.clone().unwrap_or(Item { kind: request.kind, id: request.item_id, ..Default::default() }));
        cx.ctl.capture_share(Request {
            id: id.clone(),
            server: if cx.snap.settings.development { Server::Development } else { Server::Production },
            entry,
            extra: Vec::new(),
            operation: Operation::Answer { id: request.id, approve, acknowledge_explicit },
        });
        for r in &mut self.incoming {
            if r.id == request.id {
                r.status = if approve { RequestStatus::Approved } else { RequestStatus::Denied };
            }
        }
        self.active = Some((id, request));
    }
    pub fn show(&mut self, ui: &mut egui::Ui, cx: &Cx) -> Option<Item> {
        if !self.open {
            return None;
        }
        let ctx = ui.ctx().clone();
        let mut picked = None;
        let mut close = false;
        if self.pictures.is_none() {
            self.pictures = Some(crate::worlds::Pictures::new(&ctx));
        }
        let modal = egui::Modal::new(egui::Id::unique("sharing-requests"))
            .frame(egui::Frame::NONE.fill(theme::INK_900).corner_radius(24).inner_margin(24))
            .show(&ctx, |ui| {
                ui.set_width(760.0f32.min(ctx.content_rect().width() - 100.0));
                let panel_height = (ctx.content_rect().height() - 150.0).clamp(320.0, 650.0);
                ui.set_height(panel_height);
                ui.horizontal(|ui| {
                    ui.heading("Sharing requests");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if IconButton::new(icons::X, "Close requests").show(ui).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(if self.outgoing_box {
                        "Requests you sent to people who shared these items with you."
                    } else {
                        "Choose whether people can share a moment you shared with them."
                    })
                    .color(theme::FG_3),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    widgets::segmented(ui, "request-box", &mut self.outgoing_box, &[SegOption::new(false, "Incoming"), SegOption::new(true, "Outgoing")], true);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sharing::pressed(Button::new("Refresh").variant(Variant::Ghost).enabled(!self.list.running() && self.active.is_none()).show(ui)) {
                            self.refresh(&ctx, if cx.snap.settings.development { Server::Development } else { Server::Production });
                        }
                    });
                });
                ui.add_space(16.0);
                let entries = if self.outgoing_box { self.outgoing.clone() } else { self.incoming.clone() };
                let busy = self.active.is_some();
                egui::ScrollArea::vertical()
                    .id_salt("sharing-request-list")
                    .auto_shrink([false, false])
                    .min_scrolled_height((panel_height - 155.0).max(180.0))
                    .max_height((panel_height - 155.0).max(180.0))
                    .show(ui, |ui| {
                        if self.list.running() {
                            ui.label("Loading requests...");
                        }
                        if entries.is_empty() && !self.list.running() {
                            ui.add_space(30.0);
                            ui.label("No sharing requests here yet");
                            ui.add_space(30.0);
                        }
                        for request in entries {
                            ui.push_id(request.id, |ui| {
                                egui::Frame::NONE.fill(theme::surface()).corner_radius(16).inner_margin(16).show(ui, |ui| {
                                    ui.set_min_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        let (r, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                                        let person = if self.outgoing_box { &request.approver } else { &request.requester };
                                        let tex = person.avatar_url.as_ref().and_then(|url| {
                                            self.pictures.as_mut()?.get(
                                                &ctx,
                                                &crate::worlds::Picture::Remote {
                                                    world_id: format!("request-person-{}", person.id),
                                                    url: url.clone(),
                                                    large: false,
                                                },
                                            )
                                        });
                                        sharing::avatar(ui, r, tex.as_ref(), &person.name);
                                        ui.vertical(|ui| {
                                            ui.label(egui::RichText::new(&person.name).strong());
                                            ui.label(
                                                egui::RichText::new(request.item.as_ref().map(|i| i.world.name.as_str()).unwrap_or("Item no longer available"))
                                                    .color(theme::FG_3),
                                            );
                                        });
                                    });
                                    ui.add_space(10.0);
                                    let recipients = request.recipients.iter().map(|r| r.display_name.as_str()).collect::<Vec<_>>().join(", ");
                                    ui.label(if request.public {
                                        if recipients.is_empty() {
                                            "Requests public sharing".into()
                                        } else {
                                            format!("Requests public sharing and sharing with {recipients}")
                                        }
                                    } else {
                                        format!("Requests sharing with {recipients}")
                                    });
                                    if !request.message.is_empty() {
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(&request.message).color(theme::FG_2));
                                    }
                                    ui.add_space(12.0);
                                    ui.horizontal(|ui| {
                                        if self.active.as_ref().is_some_and(|(_, r)| r.id == request.id) {
                                            ui.spinner();
                                            ui.label("Updating...");
                                        } else if request.status == RequestStatus::Pending && !self.outgoing_box {
                                            if sharing::pressed(
                                                Button::new("Approve").variant(Variant::Primary).enabled(!busy && request.item.is_some()).show(ui),
                                            ) {
                                                self.answer(cx, request.clone(), true, false);
                                            }
                                            if sharing::pressed(Button::new("Deny").enabled(!busy).show(ui)) {
                                                self.answer(cx, request.clone(), false, false);
                                            }
                                        } else {
                                            ui.label(
                                                egui::RichText::new(match request.status {
                                                    RequestStatus::Pending => "Pending",
                                                    RequestStatus::Approved => "Approved",
                                                    RequestStatus::Denied => "Denied",
                                                    RequestStatus::Cancelled => "Cancelled",
                                                })
                                                .color(theme::FG_2),
                                            );
                                        }
                                        if let Some(item) = &request.item {
                                            if sharing::pressed(Button::new("Open item").variant(Variant::Ghost).show(ui)) {
                                                picked = Some(item.clone());
                                                close = true;
                                            }
                                        }
                                    });
                                    if let Some((id, message)) = &self.row_error {
                                        if *id == request.id {
                                            ui.add_space(8.0);
                                            ui.label(egui::RichText::new(message).color(theme::DANGER));
                                            if let Some((r, approve, ack)) = self.last.clone() {
                                                if sharing::pressed(Button::new("Retry").enabled(!busy).show(ui)) {
                                                    self.answer(cx, r, approve, ack);
                                                }
                                            }
                                        }
                                    }
                                });
                                ui.add_space(12.0);
                            });
                        }
                        if let Some(error) = &self.error {
                            ui.label(egui::RichText::new(error).color(theme::DANGER));
                        }
                    });
            });
        if close || modal.should_close() {
            self.open = false;
        }
        if let Some((request, message)) = self.warning.clone() {
            let modal = egui::Modal::new(egui::Id::unique("request-public-warning"))
                .frame(egui::Frame::NONE.fill(theme::INK_900).corner_radius(20).inner_margin(24))
                .show(&ctx, |ui| {
                    ui.set_width(520.0f32.min(ctx.content_rect().width() - 100.0));
                    ui.heading("Before you approve public sharing");
                    ui.add_space(12.0);
                    ui.label(message);
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if sharing::pressed(Button::new("Approve anyway").variant(Variant::Primary).show(ui)) {
                            self.warning = None;
                            self.answer(cx, request, true, true);
                        }
                        if sharing::pressed(Button::new("Cancel").show(ui)) {
                            self.warning = None;
                        }
                    });
                });
            if modal.should_close() {
                self.warning = None;
            }
        }
        picked
    }
}
