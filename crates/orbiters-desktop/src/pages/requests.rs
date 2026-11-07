//! Sharing requests: people asking to share further what you shared with them (approve or deny, the explicit
//! acknowledgement asked inline as for public sharing), and what you asked of others. The window grows out of the
//! Requests button ([`orbiters_ui::dialog`]); answers run in the tray service.
use super::{sharing, Cx, Job};
use orbiters_i18n::{tr, tr_ctx};
use crate::controller::shares::{self as controller, Operation, Outcome, Request};
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_core::{
    account::Server,
    shares::{self, Item, RequestStatus, ShareRequest},
};
use orbiters_ui::{
    anim::{self, Spring},
    dialog, feedback, format, icons,
    theme::{self, weight, Txt},
    widgets::{self, Button, IconButton, SegOption, Size, Tone, Variant},
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
    /// The window's serial (a new one each opening) and whether it shows.
    window: Option<u64>,
    serial: u64,
}
impl State {
    pub fn pending(&self) -> usize {
        self.incoming.iter().filter(|r| r.status == RequestStatus::Pending).count()
    }
    pub fn busy(&self) -> bool {
        self.list.running()
    }
    /// Whether the window is out of its button (until it closed back into it): the button is away meanwhile.
    pub fn is_out(&self) -> bool {
        self.window.is_some()
    }

    /// How much of the Requests button shows: none while its window is out, all of it again as the window lands back
    /// on it.
    pub fn button_alpha(&self, ctx: &egui::Context) -> f32 {
        self.window.map_or(1.0, |s| dialog::source_alpha(ctx, Self::id(s)))
    }
    fn id(serial: u64) -> Id {
        Id::unique(("sharing-requests", serial))
    }
    pub fn stage(&mut self, name: &str) {
        self.fixture = true;
        self.list = Job::default();
        let fixture: shares::Requests = serde_json::from_str(include_str!("../../../orbiters-core/tests/fixtures/capture-requests.json")).unwrap();
        self.incoming = fixture.requests;
        if name.starts_with("gallery-requests") && name != "gallery-requests-opening" {
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
            self.row_error = Some((r.id, "Orbiters could not be reached. This request is still pending.".into())); // i18n: not shown (a staged scene; the real message comes from the service)
            self.last = Some((r, true, false));
        }
        if name == "gallery-requests-empty" {
            self.incoming.clear();
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
            self.window = None;
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
                _ => self.row_error = Some((before.id, tr("Unexpected answer from the sharing service.").into())),
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
        self.warning = None;
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

    /// The window, out of `button` (the Requests button); the item a request is about when Open was pressed, with
    /// the rect it opens from.
    pub fn show(&mut self, ui: &mut egui::Ui, cx: &Cx, button: Rect) -> Option<(Item, Option<Rect>)> {
        let ctx = ui.ctx().clone();
        if self.open && self.window.is_none() {
            self.serial += 1;
            self.window = Some(self.serial);
        }
        let serial = self.window?;
        let id = Self::id(serial);
        if !self.open && !dialog::is_closing(&ctx, id) {
            dialog::close(&ctx, id);
        }
        if self.pictures.is_none() {
            self.pictures = Some(crate::worlds::Pictures::new(&ctx));
        }
        let mut picked = None;
        let server = if cx.snap.settings.development { Server::Development } else { Server::Production };
        let sub = if self.outgoing_box { tr("What you asked of the people who shared with you") } else { tr("People asking to share further what you shared with them") };
        let busy = self.active.is_some();
        let response = dialog::Dialog::new(id, button, tr("Sharing requests")).glyph(icons::TRAY).subtitle(sub).width(640.0).show(&ctx, |ui, part| {
            if part != dialog::Part::Body {
                return;
            }
            let w = ui.available_width();
            // Incoming or sent, and reading them again.
            let (bar, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
            let mut bu = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("request-box-bar"));
            let incoming_label = match self.pending() {
                0 => tr_ctx("request", "Incoming").to_string(),
                n => tr!("Incoming · {count}", count = n),
            };
            widgets::segmented(&mut bu, "request-box", &mut self.outgoing_box, &[SegOption::new(false, &incoming_label), SegOption::new(true, tr_ctx("request", "Sent"))], true);
            let rr = Rect::from_center_size(pos2(bar.right() - 18.0, bar.center().y), vec2(36.0, 36.0));
            if self.list.running() {
                feedback::spinner(ui.painter(), rr.center(), 8.0, 2.0, theme::FG_2);
            } else if sharing::pressed(IconButton::new(icons::ARROW_CLOCKWISE, tr("Look for new requests")).id_salt("requests-refresh").show_at(ui, rr)) && !busy {
                self.refresh(&ctx, server);
            }
            if let Some(error) = self.error.clone() {
                feedback::banner(ui, Id::unique(("requests-error", &error)), Tone::Danger, &tr!("Requests could not be loaded: {error}", error = error), false);
            }
            let entries = if self.outgoing_box { self.outgoing.clone() } else { self.incoming.clone() };
            if entries.is_empty() {
                if self.list.running() {
                    for _ in 0..2 {
                        let (r, _) = ui.allocate_exact_size(vec2(w, 132.0), Sense::hover());
                        feedback::shimmer(ui.painter(), r, 18.0);
                    }
                } else {
                    empty(ui, self.outgoing_box);
                }
            }
            for request in entries {
                if let Some(action) = self.card(ui, &request) {
                    match action {
                        RowAction::Approve => self.answer(cx, request.clone(), true, false),
                        RowAction::ApproveAnyway => self.answer(cx, request.clone(), true, true),
                        RowAction::Deny => self.answer(cx, request.clone(), false, false),
                        RowAction::CancelWarning => self.warning = None,
                        RowAction::Retry => {
                            if let Some((r, approve, ack)) = self.last.clone() {
                                self.answer(cx, r, approve, ack);
                            }
                        }
                        RowAction::Open(origin) => {
                            if let Some(item) = &request.item {
                                picked = Some((item.clone(), Some(origin)));
                                dialog::close(&ctx, id);
                            }
                        }
                    }
                }
            }
        });
        if response.closing {
            self.open = false;
        }
        if response.closed {
            self.window = None;
        }
        picked
    }

    /// One request: who, about what, for whom, their message, then the answer (or how it was answered).
    fn card(&mut self, ui: &mut Ui, request: &ShareRequest) -> Option<RowAction> {
        let ctx = ui.ctx().clone();
        let w = ui.available_width();
        let outgoing = self.outgoing_box;
        let person = if outgoing { &request.approver } else { &request.requester };
        let world = request.item.as_ref().map(|i| i.world.name.clone());
        let inner = w - 32.0 - 48.0;
        let recipients: Vec<(Option<&str>, String)> = request
            .public
            .then(|| (Some(icons::GLOBE), tr("Everyone on Orbiters").to_string()))
            .into_iter()
            .chain(request.recipients.iter().map(|r| (None, r.display_name.clone())))
            .collect();
        let pill_w: Vec<f32> = recipients
            .iter()
            .map(|(g, n)| widgets::text_size(ui, &Txt::new(n, 12.0).weight(weight::MEDIUM)).x + if g.is_some() { 40.0 } else { 44.0 })
            .collect();
        let (pill_at, pills_h) = widgets::flow(&pill_w, inner, 28.0, 6.0);
        let message = (!request.message.trim().is_empty())
            .then(|| ui.painter().layout_job(Txt::new(request.message.trim(), 13.0).color(theme::FG).line_height(19.0).job(inner - 40.0)));
        let warning = self.warning.as_ref().filter(|(r, _)| r.id == request.id).map(|(_, m)| m.clone());
        let warning_g = warning.as_ref().map(|m| ui.painter().layout_job(Txt::new(m, 12.5).color(theme::FG_2).line_height(18.0).job(inner - 32.0)));
        let row_error = self.row_error.as_ref().filter(|(id, _)| *id == request.id).map(|(_, m)| m.clone());
        let mut h = 16.0 + 40.0 + 12.0 + pills_h;
        if let Some(g) = &message {
            h += 12.0 + g.size().y + 20.0;
        }
        if let Some(g) = &warning_g {
            h += 14.0 + 16.0 + 22.0 + g.size().y + 12.0 + 32.0 + 16.0;
        }
        if row_error.is_some() {
            h += 12.0 + 20.0;
        }
        h += 16.0 + 32.0 + 16.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        if !ui.is_rect_visible(rect) {
            return None;
        }
        let id = Id::unique(("request-card", request.id));
        let enter = anim::spring_from(&ctx, id.with("in"), [0.0], [1.0], Spring::SOFT)[0];
        let r = rect.translate(vec2(0.0, 8.0 * (1.0 - enter)));
        let mut p = ui.painter().clone();
        p.multiply_opacity(enter.clamp(0.0, 1.0));
        widgets::frame(&p, r, 18.0, theme::surface_soft());
        let mut action = None;

        // Who and about what; when, and the item's picture.
        let ar = Rect::from_min_size(r.min + vec2(16.0, 18.0), vec2(36.0, 36.0));
        let tex = person.avatar_url.as_ref().filter(|_| !self.fixture).and_then(|url| {
            self.pictures
                .as_mut()?
                .get(&ctx, &crate::worlds::Picture::Remote { world_id: format!("request-person-{}", person.id), url: url.clone(), large: false })
        });
        widgets::avatar(&p, ar, tex.as_ref(), &person.name);
        let x = ar.right() + 12.0;
        let when = format::relative_time(controller::unix(&request.created_at), now_secs());
        let when_t = Txt::new(&when, 11.5).color(theme::FG_4);
        let when_w = widgets::text_size(ui, &when_t).x;
        widgets::text(&p, pos2(r.right() - 16.0, ar.top() + 9.0), Align2::RIGHT_CENTER, &when_t);
        widgets::text_truncated(
            &p,
            pos2(x, ar.top() + 9.0),
            Align2::LEFT_CENTER,
            &Txt::new(&person.name, 14.0).weight(weight::SEMIBOLD),
            r.right() - 16.0 - when_w - 12.0 - x,
        );
        // A whole sentence for each, so a language can word the kind of item and the world its own way.
        let picture = request.kind == shares::Kind::Picture;
        let what = match (&world, outgoing, picture) {
            (Some(w), false, true) => tr!("wants to share your picture of {world}", world = w),
            (Some(w), false, false) => tr!("wants to share your 3D capture of {world}", world = w),
            (Some(w), true, true) => tr!("You asked to share their picture of {world}", world = w),
            (Some(w), true, false) => tr!("You asked to share their 3D capture of {world}", world = w),
            (None, _, _) => tr("The item is no longer available").to_string(),
        };
        widgets::text_truncated(&p, pos2(x, ar.top() + 29.0), Align2::LEFT_CENTER, &Txt::new(what, 12.5).color(theme::FG_3), r.right() - 16.0 - x);

        // For whom.
        let mut y = r.top() + 16.0 + 40.0 + 12.0;
        for ((glyph, name), (o, pw)) in recipients.iter().zip(pill_at.iter().zip(&pill_w)) {
            let pr = Rect::from_min_size(pos2(x, y) + *o, vec2(*pw, 28.0));
            p.rect_filled(pr, theme::radius(14.0), theme::control());
            match glyph {
                Some(g) => widgets::icon(&p, pos2(pr.left() + 15.0, pr.center().y), g, 13.0, theme::ACCENT),
                None => widgets::avatar(&p, Rect::from_center_size(pos2(pr.left() + 15.0, pr.center().y), vec2(20.0, 20.0)), None, name),
            }
            widgets::text(
                &p,
                pos2(pr.left() + if glyph.is_some() { 28.0 } else { 31.0 }, pr.center().y),
                Align2::LEFT_CENTER,
                &Txt::new(name, 12.0).weight(weight::MEDIUM).color(theme::FG_2),
            );
        }
        y += pills_h;

        // Their message, quoted.
        if let Some(g) = message {
            y += 12.0;
            let mr = Rect::from_min_size(pos2(x, y), vec2(inner, g.size().y + 20.0));
            p.rect_filled(mr, theme::radius(14.0), theme::white(0.04));
            widgets::icon(&p, pos2(mr.left() + 18.0, mr.top() + 19.0), icons::QUOTES, 14.0, theme::FG_4);
            let gh = g.size().y;
            p.galley(pos2(mr.left() + 34.0, mr.top() + 10.0), g, theme::FG);
            y += gh + 20.0;
        }

        // Approving something flagged explicit publicly: said plainly, here.
        if let (Some(g), Some(_)) = (warning_g, &warning) {
            y += 14.0;
            let wr = Rect::from_min_size(pos2(x, y), vec2(inner, 16.0 + 22.0 + g.size().y + 12.0 + 32.0 + 16.0));
            widgets::frame(&p, wr, 16.0, theme::alpha(theme::WARN, 0.09));
            widgets::icon(&p, pos2(wr.left() + 24.0, wr.top() + 27.0), icons::SHIELD_WARNING, 16.0, theme::WARN);
            widgets::text(&p, pos2(wr.left() + 42.0, wr.top() + 27.0), Align2::LEFT_CENTER, &Txt::new(tr("Flagged as explicit"), 13.5).weight(weight::SEMIBOLD));
            let gh = g.size().y;
            p.galley(pos2(wr.left() + 16.0, wr.top() + 16.0 + 22.0), g, theme::FG_2);
            let by = wr.top() + 16.0 + 22.0 + gh + 12.0;
            let anyway = Button::new(tr("Approve anyway"))
                .icon(icons::CHECK)
                .variant(Variant::Warn)
                .size(Size::Sm)
                .enabled(self.active.is_none())
                .id_salt(("approve-anyway", request.id));
            let cancel = Button::new(tr("Cancel")).variant(Variant::Ghost).size(Size::Sm).id_salt(("warning-cancel", request.id));
            let (aw, cw) = (anyway.desired_width(ui), cancel.desired_width(ui));
            if sharing::pressed(anyway.show_at(ui, Rect::from_min_size(pos2(wr.right() - 16.0 - aw, by), vec2(aw, 32.0)))) {
                action = Some(RowAction::ApproveAnyway);
            }
            if sharing::pressed(cancel.show_at(ui, Rect::from_min_size(pos2(wr.right() - 16.0 - aw - 8.0 - cw, by), vec2(cw, 32.0)))) {
                action = Some(RowAction::CancelWarning);
            }
            y = wr.bottom();
        }
        if let Some(err) = &row_error {
            y += 12.0;
            widgets::icon(&p, pos2(x + 7.0, y + 10.0), icons::WARNING, 13.0, theme::DANGER);
            widgets::text_truncated(&p, pos2(x + 20.0, y + 10.0), Align2::LEFT_CENTER, &Txt::new(err, 12.0).color(theme::DANGER), inner - 20.0);
            y += 20.0;
        }

        // The answer: buttons while it waits for yours, else how it went.
        y += 16.0;
        let row = Rect::from_min_size(pos2(x, y), vec2(inner, 32.0));
        let answering = self.active.as_ref().is_some_and(|(_, a)| a.id == request.id);
        let mut left = row.left();
        if answering {
            feedback::spinner(&p, pos2(left + 9.0, row.center().y), 7.0, 2.0, theme::ACCENT);
            widgets::text(&p, pos2(left + 24.0, row.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Answering…"), 12.5).color(theme::FG_2));
        } else if request.status == RequestStatus::Pending && !outgoing && warning.is_none() {
            let busy = self.active.is_some();
            let retry = row_error.is_some() && self.last.as_ref().is_some_and(|(r, _, _)| r.id == request.id);
            let approve = Button::new(if retry { tr("Try again") } else { tr("Approve") })
                .icon(if retry { icons::ARROW_CLOCKWISE } else { icons::CHECK })
                .variant(Variant::Primary)
                .size(Size::Sm)
                .enabled(!busy && (retry || request.item.is_some()))
                .id_salt(("approve", request.id));
            let deny = Button::new(tr("Deny")).icon(icons::X).size(Size::Sm).enabled(!busy).id_salt(("deny", request.id));
            let (aw, dw) = (approve.desired_width(ui), deny.desired_width(ui));
            if sharing::pressed(approve.show_at(ui, Rect::from_min_size(row.min, vec2(aw, 32.0)))) {
                action = Some(if retry { RowAction::Retry } else { RowAction::Approve });
            }
            if sharing::pressed(deny.show_at(ui, Rect::from_min_size(pos2(row.left() + aw + 8.0, row.top()), vec2(dw, 32.0)))) {
                action = Some(RowAction::Deny);
            }
            left += aw + 8.0 + dw;
        } else if warning.is_none() {
            let (glyph, label, tone) = match request.status {
                RequestStatus::Pending => (icons::CLOCK, tr!("Waiting for {name}", name = person.name), theme::WARN),
                RequestStatus::Approved => (icons::CHECK_CIRCLE, tr_ctx("request", "Approved").to_string(), theme::OK),
                RequestStatus::Denied => (icons::X_CIRCLE, tr_ctx("request", "Denied").to_string(), theme::DANGER),
                RequestStatus::Cancelled => (icons::PROHIBIT, tr_ctx("request", "Cancelled").to_string(), theme::FG_3),
            };
            // When it was answered, the status and the time as one sentence.
            let label = match (&request.answered_at, request.status) {
                (Some(at), RequestStatus::Approved) => tr!("Approved {time}", time = format::relative_time(controller::unix(at), now_secs())),
                (Some(at), RequestStatus::Denied) => tr!("Denied {time}", time = format::relative_time(controller::unix(at), now_secs())),
                _ => label,
            };
            let t = Txt::new(&label, 12.0).weight(weight::SEMIBOLD).color(tone);
            let tw = widgets::text_size(ui, &t).x;
            let pill = Rect::from_min_size(pos2(row.left(), row.center().y - 13.0), vec2(tw + 36.0, 26.0));
            let k = anim::spring_from(&ctx, id.with(("status", request.status as u8)), [0.85], [1.0], Spring::BOUNCY)[0];
            let pill = Rect::from_center_size(pill.center(), pill.size() * k);
            p.rect_filled(pill, theme::radius(13.0), theme::alpha(tone, 0.13));
            widgets::icon(&p, pos2(pill.left() + 15.0, pill.center().y), glyph, 13.0, tone);
            widgets::text(&p, pos2(pill.left() + 27.0, pill.center().y), Align2::LEFT_CENTER, &t);
            left = pill.right();
        }
        let _ = left;
        if request.item.is_some() {
            let open = Button::new(tr("Open")).icon(icons::ARROW_SQUARE_OUT).variant(Variant::Ghost).size(Size::Sm).id_salt(("open-item", request.id));
            let ow = open.desired_width(ui);
            let or = Rect::from_min_size(pos2(row.right() - ow, row.top()), vec2(ow, 32.0));
            if sharing::pressed(open.show_at(ui, or)) {
                action = Some(RowAction::Open(r));
            }
        }
        action
    }
}

enum RowAction {
    Approve,
    ApproveAnyway,
    Deny,
    CancelWarning,
    Retry,
    Open(Rect),
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// No requests in this box: what would come here.
fn empty(ui: &mut Ui, outgoing: bool) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 170.0), Sense::hover());
    let p = ui.painter();
    let tile = Rect::from_center_size(pos2(r.center().x, r.top() + 46.0), vec2(52.0, 52.0));
    p.rect_filled(tile, theme::radius(16.0), theme::white(0.05));
    widgets::icon(p, tile.center(), icons::TRAY, 24.0, theme::FG_3);
    let (title, text) = if outgoing {
        (tr("Nothing asked yet"), tr("Ask to share something a friend shared with you: their answer shows up here."))
    } else {
        (tr("No requests"), tr("When someone asks to share further what you shared with them, you answer here."))
    };
    widgets::text(p, pos2(r.center().x, tile.bottom() + 24.0), Align2::CENTER_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 12.5).color(theme::FG_3).line_height(19.0).job(380.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(r.center().x, tile.bottom() + 42.0), g, theme::FG_3);
}
