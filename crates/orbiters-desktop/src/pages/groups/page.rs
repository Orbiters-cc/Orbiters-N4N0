//! A group's page (REQUEST22 1): Look (its pictures, the nameplate banner, its words), Members (search, roles), Roles
//! (create, change, delete) and Posts (write, change, delete). One scrolling region; what VRChat gives is shown as it
//! answered, and each control is offered only when `orbiters_core::groups::can` allows it on the data read (VRChat still
//! decides).
use super::{abilities_line, failure_text, short_code, why_text, Kind, State};
use crate::controller::groups::{Done, Failure, Outcome, Page, Prepared, Reply, Request, Write};
use crate::pages::Cx;
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense, Ui};
use orbiters_core::groups::{self as g, Action, Actor, ArtSlot, Catalogue, FileTag, GroupCard, InventoryFile, Member, Post, PostDraft, Role, RoleChange, RoleDraft, Verdict, Visibility};
use orbiters_core::vrchat_api::GROUP_PAGE as PAGE;
use orbiters_i18n::tr;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Look,
    Members,
    Roles,
    Posts,
}

#[derive(Default)]
pub(crate) struct MemberList {
    pub query: String,
    /// The query the items are of, and when the person last typed.
    asked: String,
    typed_at: f64,
    pub items: Vec<Member>,
    offset: usize,
    more: bool,
    loading: bool,
    failed: Option<Failure>,
    total: Option<u32>,
    started: bool,
}

#[derive(Default)]
pub(crate) struct PostList {
    pub items: Vec<Post>,
    offset: usize,
    more: bool,
    loading: bool,
    failed: Option<Failure>,
    started: bool,
}

pub(crate) struct RoleEditor {
    pub baseline: Option<Role>,
    pub draft: RoleDraft,
}

pub(crate) struct Composer {
    pub baseline: Option<Post>,
    pub draft: PostDraft,
}

/// The account's own pictures to choose from (gallery for banners and post pictures, icons for the icon).
pub(crate) struct Picker {
    pub purpose: Purpose,
    pub files: Vec<InventoryFile>,
    offset: usize,
    more: bool,
    loading: bool,
    failed: Option<Failure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    Art(ArtSlot),
}

impl Purpose {
    fn tag(self) -> FileTag {
        match self {
            Purpose::Art(slot) => slot.tag(),
        }
    }
}

/// What the page says after a write that did not end done.
pub(crate) struct Notice {
    pub text: String,
    /// It may have been done: Check before anything else (B2).
    pub unknown: bool,
}

pub(crate) struct Open {
    pub id: String,
    pub card: GroupCard,
    pub page: Option<Page>,
    failed: Option<Failure>,
    asked: bool,
    pub tab: Tab,
    pub members: MemberList,
    pub posts: PostList,
    /// The member whose roles are open under their row (their user id, captured at the press).
    pub member_roles: Option<String>,
    pub role_editor: Option<RoleEditor>,
    pub composer: Option<Composer>,
    pub picker: Option<Picker>,
    pub notice: Option<Notice>,
    /// A picture uploaded whose apply has not been done (M6): kept to apply later, never uploaded again.
    pub uploaded: Option<(ArtSlot, InventoryFile)>,
    /// The signed-in account's name (the "you" row).
    pub me_name: String,
}

impl Open {
    pub fn new(card: GroupCard) -> Self {
        Self {
            id: card.id.clone(),
            card,
            page: None,
            failed: None,
            asked: false,
            tab: Tab::Look,
            members: MemberList::default(),
            posts: PostList::default(),
            member_roles: None,
            role_editor: None,
            composer: None,
            picker: None,
            notice: None,
            uploaded: None,
            me_name: String::new(),
        }
    }

    pub(crate) fn answer(&mut self, kind: Kind, reply: Reply) {
        match (kind, reply) {
            (Kind::Open(id) | Kind::Check(id), Reply::Opened(p)) if id == self.id => {
                self.page = Some(*p);
                self.failed = None;
                if self.notice.as_ref().is_some_and(|n| n.unknown) {
                    self.notice = None;
                }
                // What was listed may have changed: read again.
                self.members = MemberList { query: std::mem::take(&mut self.members.query), ..Default::default() };
                self.posts = PostList::default();
            }
            (Kind::Open(id) | Kind::Check(id), Reply::Failed(f)) if id == self.id => self.failed = Some(f),
            (Kind::Members { group, query, offset }, reply) if group == self.id && query == self.members.asked && offset == self.members.offset => {
                self.members.loading = false;
                match reply {
                    Reply::Members(m) => {
                        for item in m.items {
                            if !self.members.items.iter().any(|x| x.user_id == item.user_id) {
                                self.members.items.push(item);
                            }
                        }
                        // Offsets advance by what VRChat gave, kept or not (M8).
                        self.members.offset += m.raw;
                        self.members.total = m.total;
                        self.members.more = m.raw >= PAGE && m.total.is_none_or(|t| (self.members.offset as u32) < t);
                        self.members.failed = None;
                    }
                    Reply::Failed(f) => self.members.failed = Some(f),
                    _ => {}
                }
            }
            (Kind::Posts { group, offset }, reply) if group == self.id && offset == self.posts.offset => {
                self.posts.loading = false;
                match reply {
                    Reply::Posts(p) => {
                        for item in p.items {
                            if !self.posts.items.iter().any(|x| x.id == item.id) {
                                self.posts.items.push(item);
                            }
                        }
                        self.posts.offset += p.raw;
                        self.posts.more = p.raw >= PAGE && p.total.is_none_or(|t| (self.posts.offset as u32) < t);
                        self.posts.failed = None;
                    }
                    Reply::Failed(f) => self.posts.failed = Some(f),
                    _ => {}
                }
            }
            (Kind::Inventory { tag, offset }, reply) => {
                if let Some(picker) = self.picker.as_mut().filter(|p| p.purpose.tag() == tag && p.offset == offset) {
                    picker.loading = false;
                    match reply {
                        Reply::Inventory(files) => {
                            picker.more = files.len() >= PAGE;
                            picker.offset += files.len().max(1);
                            for f in files {
                                if !picker.files.iter().any(|x| x.id == f.id) {
                                    picker.files.push(f);
                                }
                            }
                        }
                        Reply::Failed(f) => picker.failed = Some(f),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// A write's end: VRChat's answer is the state from now on (M3); a refusal or an unknown outcome stays said here,
    /// and what was being written is kept for another go.
    pub(crate) fn ended(&mut self, prepared: Option<&Prepared>, outcome: Outcome) {
        match outcome {
            Outcome::Done(done) => {
                self.notice = None;
                match done {
                    Done::MemberRoles { user, role_ids } => {
                        if let Some(m) = self.members.items.iter_mut().find(|m| m.user_id == user) {
                            m.role_ids = role_ids.clone();
                        }
                        if let Some(me) = self.page.as_mut().and_then(|p| p.group.my_member.as_mut()).filter(|me| me.user_id == user) {
                            me.role_ids = role_ids;
                        }
                    }
                    Done::Role(role) => {
                        if let Some(roles) = self.page.as_mut().and_then(|p| p.group.roles.as_mut()) {
                            roles.push(role);
                        }
                        self.role_editor = None;
                    }
                    Done::Roles(roles) => {
                        if let Some(p) = self.page.as_mut() {
                            p.group.roles = Some(roles);
                        }
                        self.role_editor = None;
                    }
                    Done::Post(post) => {
                        match self.posts.items.iter_mut().find(|p| p.id == post.id) {
                            Some(p) => *p = post,
                            None => self.posts.items.insert(0, post),
                        }
                        self.composer = None;
                    }
                    Done::PostDeleted { post } => self.posts.items.retain(|p| p.id != post),
                    Done::Uploaded(file) => {
                        if let Some(Write::Upload { tag, .. }) = prepared.map(|p| &p.write) {
                            let slot = if *tag == FileTag::Icon { ArtSlot::Icon } else { ArtSlot::Banner };
                            self.uploaded = Some((slot, file));
                        }
                    }
                    Done::Group(group) => {
                        if let Some(p) = self.page.as_mut() {
                            p.group.icon_id = group.icon_id.clone();
                            p.group.icon_url = group.icon_url.clone();
                            p.group.banner_id = group.banner_id.clone();
                            p.group.banner_url = group.banner_url.clone();
                        }
                        self.card.icon_url = group.icon_url.clone();
                        self.card.banner_url = group.banner_url.clone();
                        self.uploaded = None;
                        self.picker = None;
                    }
                }
            }
            Outcome::Refused(f) => {
                self.notice = Some(Notice { text: failure_text(f), unknown: false });
                // What it was made against moved: read again.
                if matches!(f, Failure::Changed | Failure::Withheld(_) | Failure::Api(orbiters_core::vrchat_api::GroupFailure::Forbidden)) {
                    self.asked = false;
                }
            }
            Outcome::Unknown => {
                self.notice = Some(Notice { text: tr("VRChat may have done it: its answer was lost. Nothing is sent again until you check.").into(), unknown: true });
            }
        }
    }

    fn me(&self) -> Option<&g::MyMember> {
        self.page.as_ref().and_then(|p| p.group.my_member.as_ref())
    }
    fn roles(&self) -> &[Role] {
        self.page.as_ref().and_then(|p| p.group.roles.as_deref()).unwrap_or(&[])
    }
    /// The rules on what was read.
    pub(crate) fn can(&self, action: Action) -> Verdict {
        let Some(page) = self.page.as_ref() else { return Verdict::Withheld(g::Why::MissingData) };
        let Some(me) = page.group.my_member.as_ref() else { return Verdict::Withheld(g::Why::NoPermission) };
        g::can(action, &Actor::of(me), &Catalogue { roles: page.group.roles.as_deref(), permissions: page.catalogue.as_deref() })
    }
}

/// Names to show in a confirmation: of roles, members and the group, from what the page read.
#[derive(Default)]
pub(crate) struct Names {
    group: String,
    roles: HashMap<String, String>,
    members: HashMap<String, String>,
    permissions: HashMap<String, String>,
}

impl Names {
    pub(crate) fn of(open: &Open) -> Self {
        Self {
            group: open.card.name.clone(),
            roles: open.roles().iter().map(|r| (r.id.clone(), r.name.clone())).collect(),
            members: open.members.items.iter().filter_map(|m| Some((m.user_id.clone(), m.display_name.clone()?))).chain(open.me().map(|m| (m.user_id.clone(), open.me_name.clone()))).collect(),
            permissions: open.page.as_ref().and_then(|p| p.catalogue.as_ref()).map(|c| c.iter().map(|p| (p.name.clone(), p.display_name.clone())).collect()).unwrap_or_default(),
        }
    }
    fn role(&self, id: &str) -> String {
        self.roles.get(id).cloned().unwrap_or_else(|| tr("A role").to_string())
    }
    fn permission(&self, name: &str) -> String {
        self.permissions.get(name).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| name.to_string())
    }
}

fn line(ui: &mut Ui, label: &str, value: &str) {
    let w = ui.available_width();
    let p = ui.painter().clone();
    let galley = p.layout_job(Txt::new(value, 13.5).color(theme::FG).line_height(19.0).job(w - 150.0));
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y.max(19.0) + 10.0), Sense::hover());
    widgets::text(&p, pos2(rect.left(), rect.top() + 9.5), Align2::LEFT_CENTER, &Txt::new(label, 12.5).color(theme::FG_3));
    p.galley(pos2(rect.left() + 150.0, rect.top()), galley, theme::FG);
}

fn note(ui: &mut Ui, text: &str, tone: Color32) {
    let w = ui.available_width();
    let p = ui.painter().clone();
    let galley = p.layout_job(Txt::new(text, 12.5).color(tone).line_height(18.0).job(w));
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 8.0), Sense::hover());
    p.galley(rect.min + vec2(0.0, 4.0), galley, tone);
}

/// Exactly what a prepared write sends, in words (M7: the audience described, never counted).
pub(crate) fn confirm_body(ui: &mut Ui, prepared: &Prepared, names: &Names) {
    ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
    match &prepared.write {
        Write::MemberRole { user, add, .. } => {
            line(ui, tr("Member"), names.members.get(user).map(String::as_str).unwrap_or(tr("This member")));
            if let Some(role) = &prepared.role {
                line(ui, tr("Role"), &role.name);
                let rights: Vec<String> = role.permissions.iter().map(|p| names.permission(p)).collect();
                if !rights.is_empty() {
                    line(ui, tr("Its rights"), &rights.join(", "));
                }
                if role.management {
                    note(ui, tr("This is a management role: it lets them manage this group."), theme::WARN);
                }
            }
            note(ui, if *add { tr("They get it at once. Taking it back later is a new change of its own.") } else { tr("They lose what this role lets them do at once.") }, theme::FG_3);
        }
        Write::CreateRole { draft, .. } => {
            line(ui, tr("Name"), &draft.name);
            if !draft.description.is_empty() {
                line(ui, tr("Description"), &draft.description);
            }
            role_flags(ui, draft, names);
        }
        Write::EditRole { baseline, edit, .. } => {
            let change = RoleChange::between(baseline, edit);
            line(ui, tr("Role"), &baseline.name);
            if let Some(n) = &change.name {
                line(ui, tr("New name"), n);
            }
            if change.description.is_some() {
                line(ui, tr("Description"), &edit.description);
            }
            if let Some(rights) = &change.permissions {
                let added: Vec<String> = rights.iter().filter(|p| !baseline.permissions.contains(p)).map(|p| names.permission(p)).collect();
                let removed: Vec<String> = baseline.permissions.iter().filter(|p| !rights.contains(p)).map(|p| names.permission(p)).collect();
                if !added.is_empty() {
                    line(ui, tr("Rights given"), &added.join(", "));
                }
                if !removed.is_empty() {
                    line(ui, tr("Rights taken"), &removed.join(", "));
                }
            }
            for (on, label) in [(change.self_assignable, tr("Members can take it themselves")), (change.added_on_join, tr("Given to everyone who joins")), (change.requires_two_factor, tr("Needs two-factor authentication"))] {
                if let Some(on) = on {
                    line(ui, label, if on { tr("Yes") } else { tr("No") });
                }
            }
            if change.consequential() {
                note(ui, tr("Everyone who has this role, or gets it later, is concerned."), theme::WARN);
            }
        }
        Write::DeleteRole { role, .. } => {
            line(ui, tr("Role"), &prepared.role.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| names.role(role)));
            note(ui, tr("Every member who has it loses it. This cannot be undone."), theme::DANGER);
        }
        Write::CreatePost { draft, .. } | Write::EditPost { edit: draft, .. } => {
            line(ui, tr("Group"), &names.group);
            line(ui, tr("Title"), &draft.title);
            line(ui, tr("Text"), &draft.text);
            line(ui, tr("Who sees it"), if draft.visibility == Visibility::Public { tr("Anyone: it is public on VRChat's website") } else { tr("Members of the group") });
            if draft.visibility == Visibility::Group {
                let roles = if draft.role_ids.is_empty() { tr("Every member").to_string() } else { draft.role_ids.iter().map(|r| names.role(r)).collect::<Vec<_>>().join(", ") };
                line(ui, tr("Restricted to"), &roles);
            }
            if draft.image_id.is_some() {
                line(ui, tr("Picture"), tr("One of your VRChat gallery"));
            }
            let notify = if matches!(prepared.write, Write::EditPost { .. }) {
                tr("None: an edit notifies nobody again")
            } else if draft.send_notification {
                tr("Yes: members who follow the group's announcements are notified")
            } else {
                tr("No")
            };
            line(ui, tr("Notification"), notify);
        }
        Write::DeletePost { post, .. } => {
            line(ui, tr("Title"), &post.title);
            note(ui, tr("It disappears for everyone. This cannot be undone."), theme::DANGER);
        }
        Write::Upload { tag, png } => {
            line(ui, tr("Goes to"), if *tag == FileTag::Icon { tr("Your VRChat icons") } else { tr("Your VRChat gallery") });
            line(ui, tr("File"), &orbiters_i18n::tr!("PNG, {size}", size = orbiters_ui::format::bytes(png.len() as f64)));
            note(ui, tr("Nothing about the group changes yet: applying it is the next step."), theme::FG_3);
        }
        Write::ApplyArt { slot, .. } => {
            line(ui, tr("Group"), &names.group);
            line(ui, tr("Picture"), if *slot == ArtSlot::Icon { tr("The group's icon") } else { tr("The group's banner") });
            note(ui, tr("Everyone who looks at the group sees it."), theme::FG_3);
        }
    }
}

fn role_flags(ui: &mut Ui, draft: &RoleDraft, names: &Names) {
    if !draft.permissions.is_empty() {
        line(ui, tr("Its rights"), &draft.permissions.iter().map(|p| names.permission(p)).collect::<Vec<_>>().join(", "));
    }
    for (on, label) in [(draft.self_assignable, tr("Members can take it themselves")), (draft.added_on_join, tr("Given to everyone who joins")), (draft.requires_two_factor, tr("Needs two-factor authentication"))] {
        if on {
            line(ui, label, tr("Yes"));
        }
    }
}

/// The page itself.
pub(crate) fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(scope) = state.scope.clone() else { return };
    let staged = state.staged;
    if let Some(open) = state.open.as_mut() {
        open.me_name = cx.snap.vrchat_login.user.as_ref().map(|u| u.display_name.clone()).unwrap_or_default();
        if !open.asked && !staged {
            open.asked = true;
            state.jobs.ask(&ctx, cx.ctl, &scope, Kind::Open(open.id.clone()), Request::Open { group: open.id.clone() });
        }
    }
    let full = ui.max_rect();
    let from = orbiters_ui::blur::next_shape(ui);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full).id_salt("group-page"));
    let mut acts: Vec<Act> = Vec::new();
    let tab = state.open.as_ref().map(|o| o.tab).unwrap_or(Tab::Look);
    egui::ScrollArea::vertical().id_salt(("group-page", tab as u8)).auto_shrink([false, false]).scroll_bar_rect(super::super::below_bar(&child, cx)).show(&mut child, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 40 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 16.0);
            ui.set_max_width(ui.available_width().min(980.0));
            head(ui, cx, state, &scope, &mut acts);
            let Some(open) = state.open.as_ref() else { return };
            if let Some(n) = &open.notice {
                notice(ui, n, &mut acts);
            }
            if open.page.is_none() {
                match open.failed {
                    Some(f) => {
                        note(ui, &failure_text(f), theme::WARN);
                        if widgets::Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).id_salt("group-page-retry").show(ui).clicked() {
                            acts.push(Act::Reload);
                        }
                    }
                    None => {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 180.0), Sense::hover());
                        orbiters_ui::feedback::shimmer(ui.painter(), r, theme::CARD_RADIUS);
                    }
                }
                return;
            }
            match tab {
                Tab::Look => look(ui, cx, state, &scope, &mut acts),
                Tab::Members => members(ui, cx, state, &scope, &mut acts),
                Tab::Roles => roles(ui, cx, state, &mut acts),
                Tab::Posts => posts(ui, cx, state, &scope, &mut acts),
            }
        });
    });
    super::super::under_top(ui, cx, "group-page", from, full.top() + cx.top + 8.0);
    for act in acts {
        do_act(&ctx, cx, state, &scope, act);
    }
}

/// What a press asks, done after drawing.
pub(crate) enum Act {
    Back,
    Tab(Tab),
    Reload,
    Check,
    Studio(Option<super::art::Target>),
    Web,
    Write(Write, Rect),
    MoreMembers,
    MorePosts,
    Picker(Purpose),
    MorePictures,
    UploadIcon,
}

fn do_act(ctx: &egui::Context, cx: &mut Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, act: Act) {
    match act {
        Act::Back => state.close(),
        Act::Tab(t) => {
            if let Some(o) = state.open.as_mut() {
                o.tab = t;
                o.member_roles = None;
            }
        }
        Act::Reload => {
            if let Some(o) = state.open.as_mut() {
                o.asked = false;
                o.failed = None;
            }
        }
        Act::Check => {
            if let Some(o) = state.open.as_ref() {
                state.jobs.ask(ctx, cx.ctl, scope, Kind::Check(o.id.clone()), Request::Check { group: o.id.clone() });
            }
        }
        Act::Studio(target) => {
            if let Some(o) = state.open.as_ref() {
                let mut studio = super::studio::Studio::new(o.card.clone(), scope.vrchat_user().unwrap_or_default(), None);
                if let Some(t) = target {
                    studio.target = t;
                }
                state.studio = Some(studio);
            }
        }
        Act::Web => {
            if let Some(url) = state.open.as_ref().and_then(|o| g::web_page(&o.id)) {
                let r = crate::controller::open_url(&url);
                cx.run(ctx, r);
            }
        }
        Act::Write(write, origin) => state.prepare(ctx, cx.ctl, write, origin),
        Act::MoreMembers => {
            if let Some(o) = state.open.as_mut() {
                ask_members(ctx, cx, &mut state.jobs, scope, o);
            }
        }
        Act::MorePosts => {
            if let Some(o) = state.open.as_mut() {
                o.posts.loading = true;
                let (group, offset) = (o.id.clone(), o.posts.offset);
                state.jobs.ask(ctx, cx.ctl, scope, Kind::Posts { group: group.clone(), offset }, Request::Posts { group, offset });
            }
        }
        Act::Picker(purpose) => {
            if let Some(o) = state.open.as_mut() {
                if o.picker.as_ref().is_some_and(|p| p.purpose == purpose) {
                    o.picker = None;
                } else {
                    o.picker = Some(Picker { purpose, files: Vec::new(), offset: 0, more: false, loading: true, failed: None });
                    state.jobs.ask(ctx, cx.ctl, scope, Kind::Inventory { tag: purpose.tag(), offset: 0 }, Request::Inventory { tag: purpose.tag(), offset: 0 });
                }
            }
        }
        Act::MorePictures => {
            if let Some(p) = state.open.as_mut().and_then(|o| o.picker.as_mut()) {
                p.loading = true;
                let (tag, offset) = (p.purpose.tag(), p.offset);
                state.jobs.ask(ctx, cx.ctl, scope, Kind::Inventory { tag, offset }, Request::Inventory { tag, offset });
            }
        }
        Act::UploadIcon => {
            let Some(path) = rfd::FileDialog::new().set_title(tr("Choose a picture for the icon")).add_filter(tr("Pictures"), &["png", "jpg", "jpeg", "webp"]).pick_file() else { return };
            match icon_png(&path) {
                Ok(png) => state.prepare(ctx, cx.ctl, Write::Upload { tag: FileTag::Icon, png }, state.origin),
                Err(_) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Error, tr("That picture could not be read.")),
            }
        }
    }
}

/// A picture made into an icon: its middle square, at most 1024 px, as a lossless PNG.
fn icon_png(path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    let img = super::art::load_source(path)?;
    let side = img.width().min(img.height());
    let square = image::imageops::crop_imm(&img, (img.width() - side) / 2, (img.height() - side) / 2, side, side).to_image();
    let square = if side > 1024 { image::imageops::resize(&square, 1024, 1024, image::imageops::FilterType::Lanczos3) } else { square };
    super::art::encode_png(&square)
}

fn ask_members(ctx: &egui::Context, cx: &Cx, jobs: &mut super::Jobs, scope: &crate::controller::vrchat_data::Scope, o: &mut Open) {
    let query = if o.members.query.trim().chars().count() >= 3 { o.members.query.trim().to_string() } else { String::new() };
    if query != o.members.asked {
        o.members = MemberList { query: std::mem::take(&mut o.members.query), asked: query.clone(), typed_at: o.members.typed_at, started: true, ..Default::default() };
    }
    o.members.loading = true;
    o.members.started = true;
    let (group, offset) = (o.id.clone(), o.members.offset);
    jobs.ask(ctx, cx.ctl, scope, Kind::Members { group: group.clone(), query: query.clone(), offset }, Request::Members { group, query, offset });
}

fn head(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let Some(open) = state.open.as_ref() else { return };
    let card = open.card.clone();
    let tab = open.tab;
    let group = open.page.as_ref().map(|p| p.group.clone());
    let w = ui.available_width();
    // Back, then the banner with the icon over its edge, the name and what the person can do.
    let (row, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
    if widgets::Button::new(tr("Home")).icon(icons::ARROW_LEFT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("group-back").show_at(ui, Rect::from_min_size(row.min, vec2(96.0, 32.0))).clicked() {
        acts.push(Act::Back);
    }
    let web = Rect::from_min_size(pos2(row.right() - 170.0, row.top()), vec2(170.0, 32.0));
    if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("group-web").show_at(ui, web).clicked() {
        acts.push(Act::Web);
    }
    // Tall on Look (the pictures are its subject), a strip on the other tabs (their lists matter there).
    let banner_h = if tab == Tab::Look { (w / 4.2).clamp(120.0, 210.0) } else { 84.0 };
    let (hero, _) = ui.allocate_exact_size(vec2(w, banner_h + 64.0), Sense::hover());
    let banner = Rect::from_min_size(hero.min, vec2(w, banner_h));
    let p = ui.painter().clone();
    let radii = [theme::squircle(theme::CARD_RADIUS); 4];
    let banner_url = group.as_ref().map(|g| g.banner_url.clone()).unwrap_or(card.banner_url.clone());
    let icon_url = group.as_ref().map(|g| g.icon_url.clone()).unwrap_or(card.icon_url.clone());
    match state.picture(&ctx, cx.ctl, scope, &banner_url) {
        Some(t) => paint::squircle_texture(&p, banner, radii, t.id(), super::super::launcher::cover_uv(t.size_vec2(), banner.size()), Color32::WHITE),
        None => {
            let tint = widgets::name_color(&card.name);
            paint::fill_convex(&p, &paint::squircle_points(banner, theme::squircle(theme::CARD_RADIUS)), |at| theme::mix(theme::mix(tint, theme::INK_850, 0.5), theme::INK_850, ((at.x - banner.left()) / banner.width()).clamp(0.0, 1.0)))
        }
    }
    paint::stroke_squircle(&p, banner, theme::squircle(theme::CARD_RADIUS), theme::hairline());
    let icon = Rect::from_min_size(pos2(banner.left() + 24.0, banner.bottom() - 44.0), vec2(88.0, 88.0));
    p.circle_filled(icon.center(), 48.0, theme::INK_900);
    match state.picture(&ctx, cx.ctl, scope, &icon_url) {
        Some(t) => paint::circle_texture(&p, icon, t.id(), super::super::launcher::cover_uv(t.size_vec2(), icon.size()), Color32::WHITE),
        None => widgets::avatar(&p, icon, None, &card.name),
    }
    let tx = icon.right() + 18.0;
    let name = group.as_ref().map(|g| g.name.clone()).unwrap_or(card.name.clone());
    widgets::text_truncated(&p, pos2(tx, banner.bottom() + 20.0), Align2::LEFT_CENTER, &Txt::new(&name, 26.0).weight(weight::SEMIBOLD).tracking(-0.01), w - tx + hero.left());
    let code = short_code(&card.short_code, &card.discriminator);
    let members = orbiters_i18n::tr!("{n} members", n = group.as_ref().map(|g| g.member_count).unwrap_or(card.member_count));
    let mut sub = vec![members];
    if let Some(n) = group.as_ref().and_then(|g| g.online_count).or(card.online_count) {
        sub.push(orbiters_i18n::tr!("{n} online", n = n));
    }
    if !code.is_empty() {
        sub.insert(0, code);
    }
    sub.push(abilities_line(&card));
    widgets::text_truncated(&p, pos2(tx, banner.bottom() + 48.0), Align2::LEFT_CENTER, &Txt::new(sub.join(" · "), 13.0).color(theme::FG_3), w - tx + hero.left());
    // The tabs: the four are always there (reading is a member's); what each offers follows the rights.
    let options = [
        widgets::SegOption::new(Tab::Look, tr("Look")),
        widgets::SegOption::new(Tab::Members, tr("Members")),
        widgets::SegOption::new(Tab::Roles, tr("Roles")),
        widgets::SegOption::new(Tab::Posts, tr("Posts")),
    ];
    let mut chosen = tab;
    ui.horizontal(|ui| {
        if widgets::segmented(ui, "group-tabs", &mut chosen, &options, false) {
            acts.push(Act::Tab(chosen));
        }
    });
}

fn notice(ui: &mut Ui, n: &Notice, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let p = ui.painter().clone();
    let galley = p.layout_job(Txt::new(&n.text, 13.0).color(theme::FG).line_height(19.0).job(w - 210.0));
    let h = galley.size().y.max(20.0) + 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let tone = if n.unknown { theme::WARN } else { theme::DANGER };
    p.rect_filled(rect, theme::radius(14.0), theme::alpha(tone, 0.12));
    p.rect_stroke(rect, theme::radius(14.0), egui::Stroke::new(1.0, theme::alpha(tone, 0.45)), egui::StrokeKind::Inside);
    widgets::icon(&p, pos2(rect.left() + 22.0, rect.top() + 24.0), if n.unknown { icons::WARNING } else { icons::WARNING_CIRCLE }, 17.0, tone);
    p.galley(pos2(rect.left() + 42.0, rect.top() + 14.0), galley, theme::FG);
    if n.unknown {
        let b = Rect::from_min_size(pos2(rect.right() - 150.0, rect.top() + 10.0), vec2(136.0, 34.0));
        if widgets::Button::new(tr("Check VRChat")).icon(icons::ARROW_CLOCKWISE).variant(widgets::Variant::Warn).size(widgets::Size::Sm).id_salt("group-check").show_at(ui, b).clicked() {
            acts.push(Act::Check);
        }
    }
}

/// A card of the page: its frame, a caption, then its rows.
fn card(ui: &mut Ui, glyph: &str, title: &str, body: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    let top = ui.cursor().min;
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = Rect::from_min_size(top + vec2(22.0, 18.0), vec2(w - 44.0, 100_000.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("group-card", title)));
    child.spacing_mut().item_spacing = vec2(0.0, 10.0);
    widgets::section_header(&mut child, glyph, title, None);
    body(&mut child);
    let h = child.min_rect().height() + 18.0 + 20.0;
    let rect = Rect::from_min_size(top, vec2(w, h));
    ui.painter().set(bg, egui::Shape::mesh(paint::squircle_mesh(rect, theme::squircle(theme::CARD_RADIUS), theme::surface(), 1.0 / ui.ctx().pixels_per_point())));
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
}

fn button_row(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        f(ui)
    });
}

fn look(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let Some(open) = state.open.as_ref() else { return };
    let art = open.can(Action::ApplyArt).allowed();
    let group = open.page.as_ref().map(|p| p.group.clone()).unwrap_or_default();
    let uploaded = open.uploaded.clone();
    let picker = open.picker.as_ref().map(|p| (p.purpose, p.files.clone(), p.more, p.loading, p.failed));
    let busy = state.flow.is_some();
    // The banner.
    card(ui, icons::IMAGE, tr("Group banner"), |ui| {
        note(ui, tr("Shown on the group's page in VRChat's menus and on its website."), theme::FG_3);
        button_row(ui, |ui| {
            if widgets::Button::new(tr("Open the banner studio")).icon(icons::SPARKLE).variant(widgets::Variant::Primary).id_salt("look-studio").show(ui).clicked() {
                acts.push(Act::Studio(Some(super::art::Target::GroupBanner)));
            }
            let pick = widgets::Button::new(tr("Choose from your VRChat gallery")).icon(icons::IMAGES).enabled(art && !busy).id_salt("look-gallery");
            let pick = if art { pick } else { pick.hint(why_text(g::Why::NoPermission)) };
            if pick.show(ui).clicked() {
                acts.push(Act::Picker(Purpose::Art(ArtSlot::Banner)));
            }
        });
        if !art {
            note(ui, tr("Your role cannot change the group's pictures. The studio still makes and saves banners for whoever can."), theme::FG_3);
        }
    });
    if let Some((purpose @ Purpose::Art(slot), files, more, loading, failed)) = picker.clone() {
        pictures_grid(ui, cx, state, scope, purpose, &files, more, loading, failed, acts, |file, rect| Act::Write(Write::ApplyArt { group: group.id.clone(), slot, file: file.id.clone() }, rect));
    }
    if let Some((slot, file)) = uploaded {
        card(ui, icons::UPLOAD_SIMPLE, tr("Uploaded, not applied yet"), |ui| {
            note(ui, tr("The picture is in your VRChat inventory. The group is unchanged until you apply it."), theme::FG_2);
            let label = if slot == ArtSlot::Icon { tr("Apply as the icon") } else { tr("Apply as the banner") };
            let resp = widgets::Button::new(label).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(!busy).id_salt("look-apply-uploaded").show(ui);
            if resp.clicked() {
                acts.push(Act::Write(Write::ApplyArt { group: group.id.clone(), slot, file: file.id.clone() }, resp.rect));
            }
        });
    }
    // The icon.
    card(ui, icons::USER_CIRCLE, tr("Icon"), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
        let icon = Rect::from_min_size(r.min, vec2(72.0, 72.0));
        let p = ui.painter().clone();
        match state.picture(&ctx, cx.ctl, scope, &group.icon_url) {
            Some(t) => paint::circle_texture(&p, icon, t.id(), super::super::launcher::cover_uv(t.size_vec2(), icon.size()), Color32::WHITE),
            None => widgets::avatar(&p, icon, None, &group.name),
        }
        widgets::text_truncated(&p, pos2(icon.right() + 16.0, icon.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Square, from your VRChat icons or a picture of yours."), 13.0).color(theme::FG_3), r.width() - 100.0);
        button_row(ui, |ui| {
            if widgets::Button::new(tr("Choose from your icons")).icon(icons::IMAGES).enabled(art && !busy).id_salt("look-icons").show(ui).clicked() {
                acts.push(Act::Picker(Purpose::Art(ArtSlot::Icon)));
            }
            let up = widgets::Button::new(tr("Upload a new icon")).icon(icons::UPLOAD_SIMPLE).enabled(art && !busy).id_salt("look-upload-icon").show(ui);
            if up.clicked() {
                state.origin = up.rect;
                acts.push(Act::UploadIcon);
            }
        });
    });
    // The nameplate banner: exported, never applied by this version (B4).
    card(ui, icons::TAG, tr("Nameplate banner"), |ui| {
        note(ui, tr("VRChat's new nameplate banner comes in two modes: Anchored (2048 × 385) and Centered (2048 × 472). Make both in the studio from one picture, then upload them on vrchat.com."), theme::FG_2);
        note(ui, tr("Direct apply is not available in this version: VRChat has not opened this slot to tools yet."), theme::WARN);
        button_row(ui, |ui| {
            if widgets::Button::new(tr("Make nameplate banners")).icon(icons::SPARKLE).id_salt("look-nameplate").show(ui).clicked() {
                acts.push(Act::Studio(Some(super::art::Target::Anchored)));
            }
            if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).id_salt("look-web").show(ui).clicked() {
                acts.push(Act::Web);
            }
        });
    });
    // Its words: read here, changed on vrchat.com (not in this version).
    card(ui, icons::INFO, tr("About"), |ui| {
        if !group.description.is_empty() {
            line(ui, tr("Description"), &group.description);
        }
        if !group.rules.is_empty() {
            line(ui, tr("Rules"), &group.rules);
        }
        for l in &group.links {
            line(ui, tr("Link"), l);
        }
        if group.description.is_empty() && group.rules.is_empty() && group.links.is_empty() {
            note(ui, tr("No description, rules or links yet."), theme::FG_3);
        }
        note(ui, tr("The name, description, rules and links are changed on vrchat.com."), theme::FG_3);
    });
}

#[allow(clippy::too_many_arguments)]
fn pictures_grid(
    ui: &mut Ui,
    cx: &Cx,
    state: &mut State,
    scope: &crate::controller::vrchat_data::Scope,
    purpose: Purpose,
    files: &[InventoryFile],
    more: bool,
    loading: bool,
    failed: Option<Failure>,
    acts: &mut Vec<Act>,
    choose: impl Fn(&InventoryFile, Rect) -> Act,
) {
    let ctx = ui.ctx().clone();
    let title = match purpose {
        Purpose::Art(ArtSlot::Icon) => tr("Your VRChat icons"),
        Purpose::Art(ArtSlot::Banner) => tr("Your VRChat gallery"),
    };
    card(ui, icons::IMAGES, title, |ui| {
        if let Some(f) = failed {
            note(ui, &failure_text(f), theme::WARN);
        }
        if files.is_empty() && !loading && failed.is_none() {
            note(ui, tr("Nothing here yet. VRChat+ lets you upload pictures to your gallery and icons."), theme::FG_3);
        }
        let w = ui.available_width();
        let square = matches!(purpose, Purpose::Art(ArtSlot::Icon));
        let tile_w = if square { 96.0 } else { 176.0 };
        let tile_h = if square { 96.0 } else { 99.0 };
        let cols = (((w + 10.0) / (tile_w + 10.0)).floor() as usize).max(1);
        let rows = files.len().div_ceil(cols);
        let (area, _) = ui.allocate_exact_size(vec2(w, (rows as f32 * (tile_h + 10.0) - 10.0).max(0.0)), Sense::hover());
        for (i, f) in files.iter().enumerate() {
            let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (tile_w + 10.0), (i / cols) as f32 * (tile_h + 10.0)), vec2(tile_w, tile_h));
            if !ui.is_rect_visible(r) {
                continue;
            }
            let resp = ui.interact(r, Id::unique(("group-pick", f.id.as_str())), Sense::click());
            let p = ui.painter().clone();
            widgets::frame(&p, r, 12.0, theme::INK_750);
            if let Some(t) = state.picture(&ctx, cx.ctl, scope, &f.url) {
                paint::squircle_texture(&p, r, [theme::squircle(12.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
            }
            if resp.hovered() {
                p.rect_stroke(r, theme::radius(12.0), egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
            }
            if widgets::activated(&resp) && state.flow.is_none() {
                acts.push(choose(f, r));
            }
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if loading {
            note(ui, tr("Loading…"), theme::FG_3);
        } else if more && widgets::Button::new(tr("Load more")).size(widgets::Size::Sm).id_salt("pick-more").show(ui).clicked() {
            acts.push(Act::MorePictures);
        }
    });
}

fn members(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let now = orbiters_ui::anim::time(&ctx);
    let staged = state.staged;
    let busy = state.flow.is_some();
    {
        let State { open, jobs, .. } = state;
        let Some(o) = open.as_mut() else { return };
        // The search: three characters ask VRChat (after a pause in typing); fewer filter what is loaded (M8).
        let w = ui.available_width();
        let before = o.members.query.clone();
        widgets::search_field_clearable(ui, "group-members-search", &mut o.members.query, tr("Search members by name (3 letters ask VRChat)"), w.min(420.0));
        if o.members.query != before {
            o.members.typed_at = now;
        }
        let wanted = if o.members.query.trim().chars().count() >= 3 { o.members.query.trim().to_string() } else { String::new() };
        if !staged && (!o.members.started || (wanted != o.members.asked && now - o.members.typed_at > 0.45)) {
            ask_members(&ctx, cx, jobs, scope, o);
        } else if wanted != o.members.asked {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    let Some(o) = state.open.as_ref() else { return };
    let filter = o.members.query.trim().to_lowercase();
    let short = !filter.is_empty() && filter.chars().count() < 3;
    let roles: Vec<Role> = o.roles().to_vec();
    let me = o.me().cloned();
    let open_for = o.member_roles.clone();
    let viewall = me.as_ref().is_some_and(|m| g::has(&m.permissions, g::MEMBERS_VIEWALL) || g::has(&m.permissions, g::MEMBERS_MANAGE));
    let failed = o.members.failed;
    // You first: the list VRChat gives never holds the account itself.
    let mut rows: Vec<(Member, bool)> = Vec::new();
    if let Some(me) = &me {
        if o.members.asked.is_empty() && (filter.is_empty() || o.me_name.to_lowercase().contains(&filter)) {
            rows.push((Member { user_id: me.user_id.clone(), display_name: Some(o.me_name.clone()), role_ids: me.role_ids.clone(), ..Default::default() }, true));
        }
    }
    rows.extend(o.members.items.iter().filter(|m| !short || m.display_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&filter))).cloned().map(|m| (m, false)));
    // Roles can be given or taken back only on a whole record (M8), when some role allows it.
    let rows: Vec<(Member, bool, bool)> = rows.into_iter().map(|(m, mine)| {
        let assignable = (m.complete() || mine) && roles.iter().any(|r| allowed_assign(o, r, &m.user_id) || o.can(Action::AssignRole { role: &r.id, user: &m.user_id, add: false }).allowed());
        (m, mine, assignable)
    }).collect();
    if !viewall {
        note(ui, tr("VRChat shows your role only some members (friends among them); the full list needs the right to view all members."), theme::FG_3);
    }
    if let Some(f) = failed {
        note(ui, &failure_text(f), theme::WARN);
    }
    if short {
        note(ui, tr("Showing matches among the members loaded. Type three letters to search the whole group."), theme::FG_3);
    }
    let w = ui.available_width();
    for (m, mine, assignable) in rows {
        let row_h = 60.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
        let open_now = open_for.as_deref() == Some(m.user_id.as_str());
        if ui.is_rect_visible(rect.expand(120.0)) {
            let p = ui.painter().clone();
            widgets::frame(&p, rect, 14.0, theme::surface());
            let face = Rect::from_min_size(pos2(rect.left() + 12.0, rect.center().y - 18.0), vec2(36.0, 36.0));
            let pic = state.picture(&ctx, cx.ctl, scope, &m.icon_url);
            let name = m.display_name.clone().unwrap_or_else(|| tr("Someone VRChat does not name").to_string());
            widgets::avatar(&p, face, pic.as_ref(), &name);
            let label = if mine { orbiters_i18n::tr!("{name} (you)", name = name) } else { name.clone() };
            widgets::text_truncated(&p, pos2(face.right() + 12.0, rect.top() + 20.0), Align2::LEFT_CENTER, &Txt::new(&label, 14.5).weight(weight::MEDIUM), w * 0.5);
            // Their roles as chips (Everyone's left out: everyone has it).
            let mut x = face.right() + 12.0;
            for r in roles.iter().filter(|r| m.role_ids.contains(&r.id) && !r.default_role) {
                let chip = Txt::new(&r.name, 11.5).weight(weight::MEDIUM).color(if r.management || r.is_owner_role() { theme::ACCENT } else { theme::FG_2 });
                let cw = widgets::text_size(ui, &chip).x + 16.0;
                if x + cw > rect.right() - 120.0 {
                    break;
                }
                let cr = Rect::from_min_size(pos2(x, rect.top() + 34.0), vec2(cw, 20.0));
                p.rect_filled(cr, theme::radius(10.0), theme::alpha(if r.management || r.is_owner_role() { theme::ACCENT } else { theme::FG_3 }, 0.14));
                widgets::text(&p, cr.center(), Align2::CENTER_CENTER, &chip);
                x += cw + 6.0;
            }
            if assignable {
                let b = Rect::from_min_size(pos2(rect.right() - 108.0, rect.center().y - 16.0), vec2(96.0, 32.0));
                if widgets::Button::new(tr("Roles")).icon(if open_now { icons::CARET_UP } else { icons::CARET_DOWN }).size(widgets::Size::Sm).id_salt(("member-roles", m.user_id.as_str())).show_at(ui, b).clicked() {
                    let user = m.user_id.clone();
                    if let Some(o) = state.open.as_mut() {
                        o.member_roles = if open_now { None } else { Some(user) };
                    }
                }
            }
        }
        if open_now {
            if let Some(o) = state.open.as_ref() {
                role_chooser(ui, o, &m, &roles, busy, acts);
            }
        }
    }
    let Some(o) = state.open.as_ref() else { return };
    if o.members.loading {
        note(ui, tr("Loading members…"), theme::FG_3);
    } else if o.members.more && widgets::Button::new(tr("Load more members")).id_salt("members-more").show(ui).clicked() {
        acts.push(Act::MoreMembers);
    } else if o.members.items.is_empty() && !o.members.asked.is_empty() && o.members.failed.is_none() {
        note(ui, tr("Nobody in this group has that name."), theme::FG_3);
    }
}

fn allowed_assign(o: &Open, r: &Role, user: &str) -> bool {
    o.can(Action::AssignRole { role: &r.id, user, add: true }).allowed()
}

/// The roles a member can be given or have taken back, each a press of its own: ids taken at the press (M3).
fn role_chooser(ui: &mut Ui, o: &Open, m: &Member, roles: &[Role], busy: bool, acts: &mut Vec<Act>) {
    let w = ui.available_width();
    let candidates: Vec<&Role> = roles.iter().filter(|r| !r.default_role && !r.is_owner_role() && !r.requires_purchase).collect();
    let chips: Vec<(String, f32)> = candidates.iter().map(|r| {
        let label = if m.role_ids.contains(&r.id) { format!("✓ {}", r.name) } else { r.name.clone() }; // i18n: not shown (a check mark before a name)
        let width = widgets::text_size(ui, &Txt::new(&label, 13.0).weight(weight::MEDIUM)).x + 28.0;
        (label, width)
    }).collect();
    let (spots, h) = widgets::flow(&chips.iter().map(|c| c.1).collect::<Vec<_>>(), w - 32.0, 32.0, 8.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h + 48.0), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, theme::radius(14.0), theme::INK_800);
    widgets::text(&p, pos2(rect.left() + 16.0, rect.top() + 16.0), Align2::LEFT_CENTER, &Txt::new(tr("Press a role to give it or take it back. Each press asks VRChat once."), 12.0).color(theme::FG_3));
    for ((r, (label, cw)), at) in candidates.iter().zip(chips.iter()).zip(spots) {
        let has = m.role_ids.contains(&r.id);
        let verdict = o.can(Action::AssignRole { role: &r.id, user: &m.user_id, add: !has });
        let cr = Rect::from_min_size(rect.min + vec2(16.0, 32.0) + at, vec2(*cw, 30.0));
        let resp = ui.interact(cr, Id::unique(("assign", m.user_id.as_str(), r.id.as_str())), if verdict.allowed() && !busy { Sense::click() } else { Sense::hover() });
        let fill = if has { theme::alpha(theme::ACCENT, 0.2) } else { theme::INK_700 };
        p.rect_filled(cr, theme::radius(15.0), if resp.hovered() && verdict.allowed() { theme::mix(fill, theme::FG, 0.08) } else { fill });
        let color = if verdict.allowed() { if has { theme::ACCENT } else { theme::FG } } else { theme::FG_4 };
        widgets::text(&p, cr.center(), Align2::CENTER_CENTER, &Txt::new(label, 13.0).weight(weight::MEDIUM).color(color));
        if let Verdict::Withheld(why) = verdict {
            resp.clone().on_hover_text(why_text(why));
        }
        if verdict.allowed() && widgets::activated(&resp) {
            acts.push(Act::Write(Write::MemberRole { group: o.id.clone(), user: m.user_id.clone(), role: r.id.clone(), add: !has }, cr));
        }
    }
}

fn roles(ui: &mut Ui, cx: &mut Cx, state: &mut State, acts: &mut Vec<Act>) {
    let _ = cx;
    let busy = state.flow.is_some();
    let Some(o) = state.open.as_mut() else { return };
    let mut roles: Vec<Role> = o.roles().to_vec();
    roles.sort_by_key(|r| r.order.unwrap_or(i64::MAX));
    let can_create = o.can(Action::CreateRole { draft: &RoleDraft::default() });
    let catalogue = o.page.as_ref().and_then(|p| p.catalogue.clone());
    if o.page.as_ref().is_some_and(|p| p.group.roles.is_none()) {
        note(ui, tr("VRChat did not give this group's roles: nothing about them can be changed from here."), theme::WARN);
    }
    if o.role_editor.as_ref().is_none_or(|e| e.baseline.is_some()) {
        let b = widgets::Button::new(tr("New role")).icon(icons::PLUS).variant(widgets::Variant::Primary).enabled(can_create.allowed() && !busy).id_salt("role-new");
        let b = match can_create {
            Verdict::Withheld(why) => b.hint(why_text(why)),
            _ => b,
        };
        if b.show(ui).clicked() {
            o.role_editor = Some(RoleEditor { baseline: None, draft: RoleDraft::default() });
        }
    }
    if o.role_editor.as_ref().is_some_and(|e| e.baseline.is_none()) {
        role_editor(ui, o, catalogue.as_deref(), busy, acts);
    }
    for r in &roles {
        if o.role_editor.as_ref().and_then(|e| e.baseline.as_ref()).is_some_and(|b| b.id == r.id) {
            role_editor(ui, o, catalogue.as_deref(), busy, acts);
            continue;
        }
        let rename = RoleChange { name: Some(String::new()), ..Default::default() };
        let rights = RoleChange { permissions: Some(r.permissions.clone()), ..Default::default() };
        let editable = o.can(Action::EditRole { role: &r.id, change: &rename }).allowed() || o.can(Action::EditRole { role: &r.id, change: &rights }).allowed();
        let deletable = o.can(Action::DeleteRole { role: &r.id });
        let w = ui.available_width();
        let p = ui.painter().clone();
        let desc = if r.description.is_empty() { None } else { Some(p.layout_job(Txt::new(&r.description, 12.5).color(theme::FG_3).line_height(18.0).job(w - 260.0))) };
        let h = 64.0 + desc.as_ref().map_or(0.0, |d| d.size().y + 4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        widgets::frame(&p, rect, 14.0, theme::surface());
        let glyph = if r.is_owner_role() { icons::SHIELD_CHECK } else if r.default_role { icons::USERS_THREE } else if r.management { icons::SHIELD_CHECK } else { icons::TAG };
        widgets::icon(&p, pos2(rect.left() + 24.0, rect.top() + 24.0), glyph, 16.0, if r.management || r.is_owner_role() { theme::ACCENT } else { theme::FG_3 });
        widgets::text_truncated(&p, pos2(rect.left() + 44.0, rect.top() + 24.0), Align2::LEFT_CENTER, &Txt::new(&r.name, 15.0).weight(weight::SEMIBOLD), w - 280.0);
        let mut tags: Vec<String> = Vec::new();
        if r.is_owner_role() {
            tags.push(tr("Owner").into());
        }
        if r.default_role {
            tags.push(tr("Everyone").into());
        }
        if r.management {
            tags.push(tr("Management").into());
        }
        if r.self_assignable {
            tags.push(tr("Members can take it").into());
        }
        if r.added_on_join {
            tags.push(tr("Given on joining").into());
        }
        if r.requires_two_factor {
            tags.push(tr("Two-factor").into());
        }
        if r.requires_purchase {
            tags.push(tr("Bought in the store").into());
        }
        let n = if r.is_owner_role() { tr("Every right").to_string() } else { orbiters_i18n::tr!("{n} rights", n = r.permissions.len()) };
        tags.push(n);
        widgets::text_truncated(&p, pos2(rect.left() + 44.0, rect.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(tags.join(" · "), 12.0).color(theme::FG_3), w - 280.0);
        if let Some(d) = desc {
            p.galley(pos2(rect.left() + 44.0, rect.top() + 58.0), d, theme::FG_3);
        }
        let edit = Rect::from_min_size(pos2(rect.right() - 212.0, rect.top() + 14.0), vec2(96.0, 32.0));
        if editable && widgets::Button::new(tr("Edit")).icon(icons::PENCIL_SIMPLE).size(widgets::Size::Sm).enabled(!busy).id_salt(("role-edit", r.id.as_str())).show_at(ui, edit).clicked() {
            o.role_editor = Some(RoleEditor { baseline: Some(r.clone()), draft: RoleDraft::of(r) });
        }
        let del = Rect::from_min_size(pos2(rect.right() - 108.0, rect.top() + 14.0), vec2(96.0, 32.0));
        if deletable.allowed() {
            let resp = widgets::Button::new(tr("Delete")).icon(icons::TRASH).variant(widgets::Variant::Danger).size(widgets::Size::Sm).enabled(!busy).id_salt(("role-delete", r.id.as_str())).show_at(ui, del);
            if resp.clicked() {
                acts.push(Act::Write(Write::DeleteRole { group: o.id.clone(), role: r.id.clone() }, resp.rect));
            }
        }
    }
}

fn role_editor(ui: &mut Ui, o: &mut Open, catalogue: Option<&[g::PermissionInfo]>, busy: bool, acts: &mut Vec<Act>) {
    let group = o.id.clone();
    let can_rights = |o: &Open, ed: &RoleEditor| match &ed.baseline {
        Some(b) => o.can(Action::EditRole { role: &b.id, change: &RoleChange { permissions: Some(b.permissions.clone()), ..Default::default() } }).allowed() && !b.is_owner_role(),
        None => true,
    };
    let Some(ed) = o.role_editor.as_ref() else { return };
    let title = if ed.baseline.is_some() { tr("Change the role") } else { tr("New role") };
    let rights_ok = can_rights(o, ed);
    let fixed = ed.baseline.as_ref().is_some_and(|b| b.default_role);
    let owner_role = ed.baseline.as_ref().is_some_and(|b| b.is_owner_role());
    let mut save: Option<Rect> = None;
    let mut cancel = false;
    let verdict = {
        let ed = o.role_editor.as_ref().unwrap();
        match &ed.baseline {
            Some(b) => {
                let change = RoleChange::between(b, &ed.draft);
                if change.is_empty() { None } else { Some(o.can(Action::EditRole { role: &b.id, change: &change })) }
            }
            None => (!ed.draft.name.trim().is_empty()).then(|| o.can(Action::CreateRole { draft: &ed.draft })),
        }
    };
    let ed = o.role_editor.as_mut().unwrap();
    card(ui, icons::PENCIL_SIMPLE, title, |ui| {
        if !fixed {
            ui.label(egui::RichText::new(tr("Name")).color(theme::FG_3).size(12.5));
            widgets::text_field(ui, "role-name", &mut ed.draft.name, tr("A short name"), ui.available_width().min(420.0), false);
            ed.draft.name = ed.draft.name.chars().take(g::ROLE_NAME_MAX).collect();
            ui.label(egui::RichText::new(tr("Description")).color(theme::FG_3).size(12.5));
            multiline(ui, "role-description", &mut ed.draft.description, tr("What this role is for"), g::ROLE_DESCRIPTION_MAX, 2);
        } else {
            note(ui, tr("Everyone's role keeps its name: only its rights change, with the right to manage the default role."), theme::FG_3);
        }
        if !fixed && !owner_role {
            for (value, label, help) in [
                (&mut ed.draft.self_assignable, tr("Members can take it themselves"), tr("Anyone in the group can give it to themselves.")),
                (&mut ed.draft.added_on_join, tr("Given to everyone who joins"), tr("New members get it as they join.")),
            ] {
                toggle_row(ui, label, help, value);
            }
        }
        if !fixed {
            toggle_row(ui, tr("Needs two-factor authentication"), tr("Only members with two-factor authentication on can have it."), &mut ed.draft.requires_two_factor);
        }
        // Rights: from VRChat's catalogue; those this account cannot give are shown but locked; the wildcard never.
        match catalogue {
            Some(cat) if rights_ok => {
                ui.label(egui::RichText::new(tr("Rights")).color(theme::FG_3).size(12.5));
                for perm in cat.iter().filter(|p| p.name != g::ALL) {
                    let mut on = ed.draft.permissions.contains(&perm.name);
                    let before = on;
                    let label = if perm.display_name.is_empty() { perm.name.clone() } else { perm.display_name.clone() };
                    ui.add_enabled_ui(perm.allowed_to_add, |ui| toggle_row(ui, &label, &perm.help, &mut on));
                    if on != before {
                        ed.draft.permissions = g::toggled(&ed.draft.permissions, &perm.name, on, cat);
                    }
                }
            }
            _ => note(ui, tr("Its rights cannot be changed from here by your role."), theme::FG_3),
        }
        if let Some(Verdict::Withheld(why)) = verdict {
            note(ui, why_text(why), theme::WARN);
        }
        button_row(ui, |ui| {
            let resp = widgets::Button::new(tr("Save")).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(verdict.is_some_and(|v| v.allowed()) && !busy).id_salt("role-save").show(ui);
            if resp.clicked() {
                save = Some(resp.rect);
            }
            if widgets::Button::new(tr("Cancel")).variant(widgets::Variant::Ghost).id_salt("role-cancel").show(ui).clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        o.role_editor = None;
    } else if let Some(rect) = save {
        let ed = o.role_editor.as_ref().unwrap();
        let write = match &ed.baseline {
            Some(b) => Write::EditRole { group, baseline: b.clone(), edit: ed.draft.clone() },
            None => Write::CreateRole { group, draft: ed.draft.clone() },
        };
        acts.push(Act::Write(write, rect));
    }
}

fn toggle_row(ui: &mut Ui, label: &str, help: &str, value: &mut bool) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.set_width(w);
        widgets::toggle(ui, ("toggle", label), value);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
            ui.label(egui::RichText::new(label).color(theme::FG).size(13.5));
            if !help.is_empty() {
                ui.label(egui::RichText::new(help).color(theme::FG_3).size(12.0));
            }
        });
    });
}

/// A field of several lines that grows with its text (no scrolling of its own: one scroll region per view).
fn multiline(ui: &mut Ui, salt: &str, text: &mut String, hint: &str, max: usize, rows: usize) {
    let font = theme::font(13.5, weight::REGULAR);
    egui::Frame::NONE.fill(theme::INK_750).corner_radius(theme::radius(12.0)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(text)
                .id(Id::unique(("group-field", salt)))
                .frame(egui::Frame::NONE)
                .char_limit(max)
                .font(font.clone())
                .text_color(theme::FG)
                .hint_text(egui::RichText::new(hint).color(theme::FG_4).font(font))
                .desired_width(ui.available_width())
                .desired_rows(rows),
        );
    });
}

fn posts(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let busy = state.flow.is_some();
    let staged = state.staged;
    {
        let State { open, jobs, .. } = state;
        let Some(o) = open.as_mut() else { return };
        if !o.posts.started && !staged {
            o.posts.started = true;
            o.posts.loading = true;
            jobs.ask(&ctx, cx.ctl, scope, Kind::Posts { group: o.id.clone(), offset: 0 }, Request::Posts { group: o.id.clone(), offset: 0 });
        }
    }
    let Some(o) = state.open.as_mut() else { return };
    let can_post = o.can(Action::Post);
    if o.composer.is_none() {
        let b = widgets::Button::new(tr("New post")).icon(icons::PLUS).variant(widgets::Variant::Primary).enabled(can_post.allowed() && !busy).id_salt("post-new");
        let b = match can_post {
            Verdict::Withheld(why) => b.hint(why_text(why)),
            _ => b,
        };
        if b.show(ui).clicked() {
            o.composer = Some(Composer { baseline: None, draft: PostDraft::default() });
        }
        if !can_post.allowed() {
            note(ui, tr("Writing posts needs the right to manage the group's announcements."), theme::FG_3);
        }
    }
    if o.composer.as_ref().is_some_and(|c| c.baseline.is_none()) {
        composer(ui, o, busy, acts);
    }
    if let Some(f) = o.posts.failed {
        note(ui, &failure_text(f), theme::WARN);
    }
    let items = o.posts.items.clone();
    let role_names: HashMap<String, String> = o.roles().iter().map(|r| (r.id.clone(), r.name.clone())).collect();
    for post in &items {
        if o.composer.as_ref().and_then(|c| c.baseline.as_ref()).is_some_and(|b| b.id == post.id) {
            composer(ui, o, busy, acts);
            continue;
        }
        let w = ui.available_width();
        let p = ui.painter().clone();
        let text = p.layout_job(Txt::new(&post.text, 13.5).color(theme::FG_2).line_height(20.0).job(w - 48.0));
        let h = 96.0 + text.size().y;
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        widgets::frame(&p, rect, theme::CARD_RADIUS, theme::surface());
        widgets::text_truncated(&p, pos2(rect.left() + 24.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(&post.title, 16.0).weight(weight::SEMIBOLD), w - 260.0);
        let who = if post.visibility == Visibility::Public {
            tr("Public").to_string()
        } else if post.role_ids.is_empty() {
            tr("Members").to_string()
        } else {
            post.role_ids.iter().map(|r| role_names.get(r).cloned().unwrap_or_else(|| tr("A role").into())).collect::<Vec<_>>().join(", ")
        };
        let when = super::super::local_time(&post.created_at);
        widgets::text_truncated(&p, pos2(rect.left() + 24.0, rect.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(format!("{when} · {who}"), 12.0).color(theme::FG_3), w - 260.0);
        p.galley(pos2(rect.left() + 24.0, rect.top() + 70.0), text, theme::FG_2);
        if can_post.allowed() {
            let edit = Rect::from_min_size(pos2(rect.right() - 212.0, rect.top() + 14.0), vec2(96.0, 32.0));
            if widgets::Button::new(tr("Edit")).icon(icons::PENCIL_SIMPLE).size(widgets::Size::Sm).enabled(!busy).id_salt(("post-edit", post.id.as_str())).show_at(ui, edit).clicked() {
                let draft = PostDraft { title: post.title.clone(), text: post.text.clone(), visibility: post.visibility, role_ids: post.role_ids.clone(), image_id: post.image_id.clone(), send_notification: false };
                o.composer = Some(Composer { baseline: Some(post.clone()), draft });
            }
            let del = Rect::from_min_size(pos2(rect.right() - 108.0, rect.top() + 14.0), vec2(96.0, 32.0));
            let resp = widgets::Button::new(tr("Delete")).icon(icons::TRASH).variant(widgets::Variant::Danger).size(widgets::Size::Sm).enabled(!busy).id_salt(("post-delete", post.id.as_str())).show_at(ui, del);
            if resp.clicked() {
                acts.push(Act::Write(Write::DeletePost { group: o.id.clone(), post: post.clone() }, resp.rect));
            }
        }
    }
    if o.posts.loading {
        note(ui, tr("Loading posts…"), theme::FG_3);
    } else if items.is_empty() && o.posts.failed.is_none() && o.posts.started {
        note(ui, tr("No posts yet."), theme::FG_3);
    } else if o.posts.more && widgets::Button::new(tr("Load more posts")).id_salt("posts-more").show(ui).clicked() {
        acts.push(Act::MorePosts);
    }
}

/// Writing or changing a post: notification off unless turned on, and only for a new one (M7).
fn composer(ui: &mut Ui, o: &mut Open, busy: bool, acts: &mut Vec<Act>) {
    let group = o.id.clone();
    let roles: Vec<Role> = o.roles().iter().filter(|r| !r.default_role).cloned().collect();
    let Some(c) = o.composer.as_mut() else { return };
    let editing = c.baseline.is_some();
    let mut send: Option<Rect> = None;
    let mut cancel = false;
    card(ui, icons::CHAT_CENTERED_TEXT, if editing { tr("Change the post") } else { tr("New post") }, |ui| {
        widgets::text_field(ui, "post-title", &mut c.draft.title, tr("Title"), ui.available_width(), false);
        c.draft.title = c.draft.title.chars().take(g::POST_TITLE_MAX).collect();
        multiline(ui, "post-text", &mut c.draft.text, tr("What do you want to tell the group?"), g::POST_TEXT_MAX, 5);
        ui.label(egui::RichText::new(tr("Who sees it")).color(theme::FG_3).size(12.5));
        ui.horizontal(|ui| {
            widgets::segmented(ui, "post-visibility", &mut c.draft.visibility, &[widgets::SegOption::new(Visibility::Group, tr("Members of the group")), widgets::SegOption::new(Visibility::Public, tr("Public"))], true);
        });
        if c.draft.visibility == Visibility::Group && !roles.is_empty() {
            note(ui, tr("Only for some roles? Choose them; none chosen reaches every member."), theme::FG_3);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for r in &roles {
                    let on = c.draft.role_ids.contains(&r.id);
                    let chip = widgets::Chip::new(&r.name).selected(on).id_salt(("post-role", r.id.as_str()));
                    let w = chip.width(ui);
                    let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
                    if chip.show_at(ui, rect).pressed {
                        if on {
                            c.draft.role_ids.retain(|x| *x != r.id);
                        } else {
                            c.draft.role_ids.push(r.id.clone());
                        }
                    }
                }
            });
        }
        if c.draft.visibility == Visibility::Public {
            note(ui, tr("A public post can be read by anyone on VRChat's website, members or not."), theme::WARN);
        }
        if !editing {
            toggle_row(ui, tr("Send a notification"), tr("Members who follow the group's announcements get one. Off unless you turn it on."), &mut c.draft.send_notification);
        } else {
            note(ui, tr("Changing a post notifies nobody again."), theme::FG_3);
        }
        let ready = !c.draft.title.trim().is_empty() && !c.draft.text.trim().is_empty();
        button_row(ui, |ui| {
            let resp = widgets::Button::new(if editing { tr("Review the change") } else { tr("Review the post") }).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(ready && !busy).id_salt("post-review").show(ui);
            if resp.clicked() {
                send = Some(resp.rect);
            }
            if widgets::Button::new(tr("Cancel")).variant(widgets::Variant::Ghost).id_salt("post-cancel").show(ui).clicked() {
                cancel = true;
            }
        });
    });
    if cancel {
        o.composer = None;
    } else if let Some(rect) = send {
        let c = o.composer.as_ref().unwrap();
        let write = match &c.baseline {
            Some(b) => Write::EditPost { group, baseline: b.clone(), edit: c.draft.clone() },
            None => Write::CreatePost { group, draft: c.draft.clone() },
        };
        acts.push(Act::Write(write, rect));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::groups::Page;

    fn open_as(rights: &[&str], my_roles: &[&str]) -> Open {
        let mut o = Open::new(GroupCard { id: "grp_1".into(), name: "Made-up".into(), ..Default::default() });
        let roles = vec![
            Role { id: "grol_owner".into(), order: Some(0), permissions: vec!["*".into()], ..Default::default() },
            Role { id: "grol_mod".into(), order: Some(2), management: true, permissions: vec!["group-instance-moderate".into()], ..Default::default() },
            Role { id: "grol_artist".into(), order: Some(3), ..Default::default() },
        ];
        let group = g::Group {
            id: "grp_1".into(),
            my_member: Some(g::MyMember { user_id: "usr_me".into(), role_ids: my_roles.iter().map(|s| s.to_string()).collect(), permissions: rights.iter().map(|s| s.to_string()).collect(), status: "member".into() }),
            roles: Some(roles),
            ..Default::default()
        };
        o.answer(Kind::Open("grp_1".into()), Reply::Opened(Box::new(Page { group, catalogue: Some(vec![]) })));
        o
    }

    /// M3: the member's roles after a write are VRChat's answer, not what the press assumed.
    #[test]
    fn a_done_write_takes_vrchats_answer() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.members.items = vec![Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), display_name: Some("Sam".into()), role_ids: vec![], ..Default::default() }];
        o.ended(None, Outcome::Done(Done::MemberRoles { user: "usr_00000000-0000-4000-8000-000000000001".into(), role_ids: vec!["grol_mod".into(), "grol_x".into()] }));
        assert_eq!(o.members.items[0].role_ids, ["grol_mod", "grol_x"]);
    }

    /// B2: an unknown outcome keeps the draft and offers Check; a refusal says why, without server text.
    #[test]
    fn an_unknown_post_keeps_its_draft() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.composer = Some(Composer { baseline: None, draft: PostDraft { title: "t".into(), text: "x".into(), ..Default::default() } });
        o.ended(None, Outcome::Unknown);
        assert!(o.composer.is_some(), "the draft is kept");
        assert!(o.notice.as_ref().unwrap().unknown);
        o.answer(Kind::Check("grp_1".into()), Reply::Opened(Box::new(o.page.clone().unwrap())));
        assert!(o.notice.is_none(), "checked");
    }

    /// M6: uploaded but not applied is a state of its own, kept to apply later.
    #[test]
    fn uploaded_then_apply_refused_keeps_the_file() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        let file = InventoryFile { id: "file_1".into(), url: "https://api.vrchat.cloud/api/1/file/file_1/1/file".into() };
        let upload = Prepared { op: "a".into(), write: Write::Upload { tag: FileTag::Icon, png: vec![] }, confirm: true, role: None };
        o.ended(Some(&upload), Outcome::Done(Done::Uploaded(file.clone())));
        o.ended(None, Outcome::Refused(Failure::Api(orbiters_core::vrchat_api::GroupFailure::Forbidden)));
        assert_eq!(o.uploaded, Some((ArtSlot::Icon, file)));
    }

    /// The page offers what the rules allow: a moderator sees no role controls, an owner does.
    #[test]
    fn controls_follow_the_rules() {
        let moderator = open_as(&["group-instance-moderate"], &["grol_mod"]);
        assert!(!moderator.can(Action::Post).allowed());
        assert!(!allowed_assign(&moderator, &moderator.roles()[2].clone(), "usr_00000000-0000-4000-8000-000000000001"));
        let owner = open_as(&["*"], &["grol_owner"]);
        assert!(owner.can(Action::Post).allowed());
        assert!(allowed_assign(&owner, &owner.roles()[2].clone(), "usr_00000000-0000-4000-8000-000000000001"));
    }

    /// M8: a search's answers are keyed by their query and offset: an obsolete one is dropped; duplicates once.
    #[test]
    fn member_pages_are_keyed_and_deduplicated() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.members.asked = "Sam".into();
        let m = |id: &str| Member { user_id: id.into(), display_name: Some("Sam".into()), ..Default::default() };
        let page = orbiters_core::vrchat_api::Members { items: vec![m("usr_a"), m("usr_a")], raw: 2, total: Some(2) };
        o.answer(Kind::Members { group: "grp_1".into(), query: "Old".into(), offset: 0 }, Reply::Members(page.clone()));
        assert!(o.members.items.is_empty(), "an answer to another query is dropped");
        o.answer(Kind::Members { group: "grp_1".into(), query: "Sam".into(), offset: 0 }, Reply::Members(page));
        assert_eq!(o.members.items.len(), 1);
        assert_eq!(o.members.offset, 2, "advanced by what VRChat gave");
        assert!(!o.members.more, "the total is reached");
    }
}
