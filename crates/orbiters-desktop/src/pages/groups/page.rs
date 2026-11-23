//! A group's page (REQUEST22 1): Look (its pictures, the nameplate banner, its words), Members (search, roles), Roles
//! (create, change, delete) and Posts (write, change, delete). One scrolling region; what VRChat gives is shown as it
//! answered, and each control is offered only when `orbiters_core::groups::can` allows it on the data read (VRChat still
//! decides).
use super::{abilities_line, authority_text, failure_text, short_code, warning_text, Kind, State};
use super::art::Drafts;
use crate::controller::groups::{Done, Failure, Outcome, Page, Reply, Request, Sealed, Write};
use crate::pages::Cx;
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense, Ui};
use orbiters_core::groups::{self as g, Action, Actor, ArtSlot, Authority, Catalogue, FileTag, GroupCard, InventoryFile, Member, Missing, Post, PostDraft, Role, RoleChange, RoleDraft, Visibility};
use orbiters_core::vrchat_api::GROUP_PAGE as PAGE;
use orbiters_i18n::tr;
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};
use std::collections::{HashMap, HashSet};

/// Members and posts kept loaded at most: past this, a search narrows (Major 9).
const MEMBERS_MAX: usize = 2000;
const POSTS_MAX: usize = 1000;

/// A number for each editor opened: a write that ends clears the very editor it came from, and no other (M12).
fn editor_number() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

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
    /// The user ids loaded, to keep each once without a scan per row (Major 9).
    seen: HashSet<String>,
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
    /// The ids loaded, to keep each once without a scan per post.
    seen: HashSet<String>,
    offset: usize,
    more: bool,
    loading: bool,
    failed: Option<Failure>,
    started: bool,
}

pub(crate) struct RoleEditor {
    number: u64,
    pub baseline: Option<Role>,
    pub draft: RoleDraft,
}

impl RoleEditor {
    pub fn new(baseline: Option<Role>, draft: RoleDraft) -> Self {
        Self { number: editor_number(), baseline, draft }
    }
}

pub(crate) struct Composer {
    number: u64,
    pub baseline: Option<Post>,
    pub draft: PostDraft,
    /// Brought back from disk as it was, and not touched since.
    pub restored: bool,
}

impl Composer {
    pub fn new(baseline: Option<Post>, draft: PostDraft) -> Self {
        Self { number: editor_number(), baseline, draft, restored: false }
    }
    /// The post's picture is to be removed by this edit (the draft has none, the post had one; Minor 6).
    pub fn removes_picture(&self) -> bool {
        self.draft.image_id.is_none() && self.baseline.as_ref().is_some_and(|b| b.image_id.is_some())
    }
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

pub(crate) struct Open {
    pub id: String,
    pub card: GroupCard,
    pub page: Option<Page>,
    failed: Option<Failure>,
    asked: bool,
    pub tab: Tab,
    pub members: MemberList,
    pub posts: PostList,
    /// Heights of post cards measured when drawn: the others take their place unmeasured (Major 9).
    post_heights: HashMap<String, f32>,
    /// The member whose roles are open under their row (their user id, captured at the press).
    pub member_roles: Option<String>,
    pub role_editor: Option<RoleEditor>,
    pub composer: Option<Composer>,
    /// The editor the write on its way came from.
    sent_editor: Option<u64>,
    pub picker: Option<Picker>,
    /// The signed-in account's id: what the rules are asked for.
    pub me_id: String,
    /// The signed-in account's name (the "you" row).
    pub me_name: String,
    /// Roster rows whose member was copied and worked out, ever (the tests' measure of Major 9).
    #[cfg(test)]
    rows_built: usize,
}

impl Open {
    /// The page of `card`, with the new post the person was writing for it brought back (Major 8).
    pub fn new(card: GroupCard, drafts: &Drafts) -> Self {
        let composer = drafts.composer(&card.id).map(|d| Composer { restored: true, ..Composer::new(None, d.clone()) });
        Self {
            id: card.id.clone(),
            card,
            page: None,
            failed: None,
            asked: false,
            tab: if composer.is_some() { Tab::Posts } else { Tab::Look },
            members: MemberList::default(),
            posts: PostList::default(),
            post_heights: HashMap::new(),
            member_roles: None,
            role_editor: None,
            composer,
            sent_editor: None,
            picker: None,
            me_id: String::new(),
            me_name: String::new(),
            #[cfg(test)]
            rows_built: 0,
        }
    }

    /// Everything read again (after a check, or a refusal that says the page is out of date).
    pub fn reload(&mut self) {
        self.asked = false;
    }

    pub(crate) fn answer(&mut self, kind: Kind, reply: Reply) {
        match (kind, reply) {
            (Kind::Open(id), Reply::Opened(p)) if id == self.id => {
                self.page = Some(*p);
                self.failed = None;
                // What was listed may have changed: read again.
                self.members = MemberList { query: std::mem::take(&mut self.members.query), ..Default::default() };
                self.posts = PostList::default();
            }
            (Kind::Open(id), Reply::Failed(f)) if id == self.id => self.failed = Some(f),
            (Kind::Members { group, query, offset }, reply) if group == self.id && query == self.members.asked && offset == self.members.offset => {
                self.members.loading = false;
                match reply {
                    Reply::Members(m) => {
                        for item in m.items {
                            if self.members.items.len() < MEMBERS_MAX && self.members.seen.insert(item.user_id.clone()) {
                                self.members.items.push(item);
                            }
                        }
                        // Offsets advance by what VRChat gave, kept or not (M8).
                        self.members.offset += m.raw;
                        self.members.total = m.total;
                        self.members.more = m.raw >= PAGE && m.total.is_none_or(|t| (self.members.offset as u32) < t) && self.members.items.len() < MEMBERS_MAX;
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
                            if self.posts.items.len() < POSTS_MAX && self.posts.seen.insert(item.id.clone()) {
                                self.posts.items.push(item);
                            }
                        }
                        self.posts.offset += p.raw;
                        self.posts.more = p.raw >= PAGE && p.total.is_none_or(|t| (self.posts.offset as u32) < t) && self.posts.items.len() < POSTS_MAX;
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
                        // Offsets and "more" by what VRChat gave, unusable entries included (Minor 5).
                        Reply::Inventory(page) => {
                            picker.more = page.raw >= PAGE;
                            picker.offset += page.raw.max(1);
                            for f in page.items {
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

    /// A write's end: VRChat's answer is the state from now on (M3). A refusal or an unknown outcome is said by the
    /// page's warnings (`super::uncertain_panel`); what was being written is kept, and only the submitted draft is
    /// cleared when done (a newer one stays, Major 12).
    pub(crate) fn ended(&mut self, sealed: Option<&Sealed>, outcome: Outcome) {
        let sent = sealed.and(self.sent_editor.take());
        match outcome {
            Outcome::Done(done) => match done {
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
                    self.clear_role_editor(sealed, sent);
                }
                Done::Roles(roles) => {
                    if let Some(p) = self.page.as_mut() {
                        p.group.roles = Some(roles);
                    }
                    self.clear_role_editor(sealed, sent);
                }
                Done::Post(post) => {
                    // The very editor it came from, still saying the very same.
                    let same = |c: &Composer| match sealed.map(|s| &s.write) {
                        Some(Write::CreatePost { draft, .. }) => c.baseline.is_none() && c.draft.content() == draft.content() && c.draft.send_notification == draft.send_notification,
                        Some(Write::EditPost { baseline, edit, .. }) => c.baseline.as_ref().is_some_and(|b| b.id == baseline.id) && c.draft.content() == edit.content(),
                        _ => false,
                    };
                    if self.composer.as_ref().is_some_and(|c| Some(c.number) == sent && same(c)) {
                        self.composer = None;
                    }
                    self.posts.seen.insert(post.id.clone());
                    match self.posts.items.iter_mut().find(|p| p.id == post.id) {
                        Some(p) => *p = post,
                        None => self.posts.items.insert(0, post),
                    }
                }
                Done::PostDeleted { post } => self.posts.items.retain(|p| p.id != post),
                Done::Uploaded(_) => {}
                Done::Group(group) => {
                    if let Some(p) = self.page.as_mut() {
                        p.group.icon_id = group.icon_id.clone();
                        p.group.icon_url = group.icon_url.clone();
                        p.group.banner_id = group.banner_id.clone();
                        p.group.banner_url = group.banner_url.clone();
                    }
                    self.card.icon_url = group.icon_url.clone();
                    self.card.banner_url = group.banner_url.clone();
                    self.picker = None;
                }
            },
            Outcome::Refused(f) => {
                // What it was made against moved: read again.
                if matches!(f, Failure::Changed | Failure::Withheld(_) | Failure::NotRead(_) | Failure::Api(orbiters_core::vrchat_api::GroupFailure::Forbidden)) {
                    self.asked = false;
                    self.posts = PostList::default();
                }
            }
            Outcome::Unknown => {}
        }
    }

    fn clear_role_editor(&mut self, sealed: Option<&Sealed>, sent: Option<u64>) {
        let same = |e: &RoleEditor| match sealed.map(|s| &s.write) {
            Some(Write::CreateRole { draft, .. }) => e.baseline.is_none() && e.draft.as_role().digest_of(&g::RoleField::ALL) == draft.as_role().digest_of(&g::RoleField::ALL),
            Some(Write::EditRole { baseline, edit, .. }) => e.baseline.as_ref().is_some_and(|b| b.id == baseline.id) && e.draft.as_role().digest_of(&g::RoleField::ALL) == edit.as_role().digest_of(&g::RoleField::ALL),
            _ => false,
        };
        if self.role_editor.as_ref().is_some_and(|e| Some(e.number) == sent && same(e)) {
            self.role_editor = None;
        }
    }

    fn me(&self) -> Option<&g::MyMember> {
        self.page.as_ref().and_then(|p| p.group.my_member.as_ref())
    }
    fn roles(&self) -> &[Role] {
        self.page.as_ref().and_then(|p| p.group.roles.as_deref()).unwrap_or(&[])
    }
    /// The rules on what was read, for the signed-in account.
    pub(crate) fn can(&self, action: Action) -> Authority {
        let Some(page) = self.page.as_ref() else { return Authority::Unknown(Missing::Membership) };
        g::can(action, Actor::of(&page.group, &self.me_id), &Catalogue { roles: page.group.roles.as_deref(), permissions: page.catalogue.as_deref() })
    }
}

/// A right by its display name, or its key through the text policy (Minor 1).
fn right_label(sealed: &Sealed, key: &str) -> String {
    sealed.label(key).map(|l| g::label(l, 64)).unwrap_or_else(|| g::label(key, 64))
}
fn role_label(sealed: &Sealed, id: &str) -> String {
    sealed.label(id).map(str::to_string).unwrap_or_else(|| format!("{} {}", tr("A role"), id_tail(id)))
}

/// A short, stable tail of an id, to tell two people of the same name apart (Major 3).
fn id_tail(id: &str) -> String {
    let tail: String = id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
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
pub(crate) fn confirm_body(ui: &mut Ui, sealed: &Sealed, art: Option<&egui::TextureHandle>) {
    ui.spacing_mut().item_spacing = vec2(0.0, 4.0);
    if let Some(w) = sealed.warning {
        note(ui, warning_text(w), theme::WARN);
    }
    if sealed.art.is_some() {
        art_preview(ui, art);
    }
    // The group it goes to, as sealed (B-F).
    line(ui, tr("Group"), &format!("{} {}", sealed.group_name, id_tail(&sealed.group)));
    let rights = |list: &[String]| list.iter().map(|p| right_label(sealed, p)).collect::<Vec<_>>().join(", ");
    match &sealed.write {
        Write::MemberRole { user, add, .. } => {
            let name = sealed.member.as_ref().and_then(|m| m.display_name.clone());
            line(ui, tr("Member"), name.as_deref().unwrap_or(tr("This member")));
            // Ids that tell two people of the same name apart (Major 3).
            let membership = sealed.member.as_ref().filter(|m| !m.membership_id.is_empty()).map(|m| id_tail(&m.membership_id)).unwrap_or_default();
            line(ui, tr("Account"), format!("{} {}", id_tail(user), membership).trim());
            if let Some(role) = &sealed.role {
                line(ui, tr("Role"), &g::label(&role.name, g::ROLE_NAME_MAX));
                if !role.permissions.is_empty() {
                    line(ui, tr("Its rights"), &rights(&role.permissions));
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
            if !draft.permissions.is_empty() {
                line(ui, tr("Its rights"), &rights(&draft.permissions));
            }
            for (on, label) in [(draft.self_assignable, tr("Members can take it themselves")), (draft.added_on_join, tr("Given to everyone who joins")), (draft.requires_two_factor, tr("Needs two-factor authentication"))] {
                if on {
                    line(ui, label, tr("Yes"));
                }
            }
        }
        Write::EditRole { baseline, edit, .. } => {
            let change = RoleChange::between(baseline, edit);
            line(ui, tr("Role"), &g::label(&baseline.name, g::ROLE_NAME_MAX));
            if let Some(n) = &change.name {
                line(ui, tr("New name"), n);
            }
            if change.description.is_some() {
                line(ui, tr("Description"), &edit.description);
            }
            if let Some(after) = &change.permissions {
                let added: Vec<String> = after.iter().filter(|p| !baseline.permissions.contains(p)).cloned().collect();
                let removed: Vec<String> = baseline.permissions.iter().filter(|p| !after.contains(p)).cloned().collect();
                if !added.is_empty() {
                    line(ui, tr("Rights given"), &rights(&added));
                }
                if !removed.is_empty() {
                    line(ui, tr("Rights taken"), &rights(&removed));
                }
            }
            for (on, label) in [(change.self_assignable, tr("Members can take it themselves")), (change.added_on_join, tr("Given to everyone who joins")), (change.requires_two_factor, tr("Needs two-factor authentication"))] {
                if let Some(on) = on {
                    line(ui, label, if on { tr("Yes") } else { tr("No") });
                }
            }
        }
        Write::DeleteRole { .. } => {
            if let Some(role) = &sealed.role {
                line(ui, tr("Role"), &g::label(&role.name, g::ROLE_NAME_MAX));
                if !role.description.is_empty() {
                    line(ui, tr("Description"), &g::text(&role.description, 300));
                }
                if !role.permissions.is_empty() {
                    line(ui, tr("Its rights"), &rights(&role.permissions));
                }
            }
            note(ui, tr("Members who have it lose it, and what it lets them do. This cannot be undone."), theme::DANGER);
        }
        Write::CreatePost { draft, .. } | Write::EditPost { edit: draft, .. } => {
            let baseline = match &sealed.write {
                Write::EditPost { baseline, .. } => Some(baseline),
                _ => None,
            };
            line(ui, tr("Title"), &draft.title);
            line(ui, tr("Text"), &draft.text);
            line(ui, tr("Who sees it"), if draft.visibility == Visibility::Public { tr("Anyone, on VRChat's website too") } else { tr("Members of the group") });
            if draft.visibility == Visibility::Group && !draft.role_ids.is_empty() {
                line(ui, tr("Restricted to"), &draft.role_ids.iter().map(|r| role_label(sealed, r)).collect::<Vec<_>>().join(", "));
            }
            match (&draft.image_id, baseline.and_then(|b| b.image_id.as_ref())) {
                (Some(id), _) => line(ui, tr("Picture"), &format!("{} {}", tr("The post's own"), id_tail(id))),
                (None, Some(_)) => line(ui, tr("Picture"), tr("Removed from the post")),
                (None, None) => {}
            }
            // What an edit changes against the post as VRChat has it (Blocker 3).
            if let Some(b) = baseline {
                let changed = b.changed_by(draft);
                if changed.contains(&g::PostField::Visibility) {
                    note(ui, tr("This changes who sees the post."), theme::WARN);
                }
                if changed.contains(&g::PostField::Roles) {
                    note(ui, tr("This changes which roles see the post."), theme::WARN);
                }
                note(ui, tr("The post is read again just before it is sent: if someone changed it, nothing goes. VRChat offers no way to close the instant between."), theme::FG_3);
            }
            let notify = if baseline.is_some() {
                tr("None: an edit notifies nobody again")
            } else if draft.send_notification {
                tr("Yes: members who follow the group's announcements are notified")
            } else {
                tr("No")
            };
            line(ui, tr("Notification"), notify);
        }
        Write::DeletePost { post, .. } => {
            // The post exactly as read now (Blocker 7).
            line(ui, tr("Title"), &g::label(&post.title, g::POST_TITLE_MAX));
            line(ui, tr("Text"), &g::text(&post.text, 300));
            note(ui, tr("It disappears for everyone. This cannot be undone."), theme::DANGER);
        }
        Write::Upload { tag, .. } => {
            line(ui, tr("Goes to"), if *tag == FileTag::Icon { tr("Your VRChat icons") } else { tr("Your VRChat gallery") });
            if let Some(a) = &sealed.art {
                line(ui, tr("File"), &orbiters_i18n::tr!("PNG, {size}", size = orbiters_ui::format::bytes(a.size as f64)));
            }
            note(ui, tr("Nothing about the group changes yet: applying it is the next step."), theme::FG_3);
        }
        Write::ApplyArt { slot, file, .. } => {
            line(ui, tr("Picture"), if *slot == ArtSlot::Icon { tr("The group's icon") } else { tr("The group's banner") });
            line(ui, tr("File"), &id_tail(file));
            note(ui, tr("Everyone who looks at the group sees it."), theme::FG_3);
        }
    }
}

/// What VRChat's answer left out, and reading it again.
fn unknown(ui: &mut Ui, a: &Authority, salt: &str, acts: &mut Vec<Act>) {
    if let (Authority::Unknown(_), Some(text)) = (a, authority_text(a)) {
        note(ui, text, theme::WARN);
        if widgets::Button::new(tr("Read again")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt(("group-read-again", salt)).show(ui).clicked() {
            acts.push(Act::Reload);
        }
    }
}

/// The art a picture write sends, as the confirmation shows it.
fn art_preview(ui: &mut Ui, art: Option<&egui::TextureHandle>) {
    let Some(t) = art else {
        note(ui, tr("Its picture is not here yet: it can go once it shows here."), theme::WARN);
        return;
    };
    let w = ui.available_width().min(360.0);
    let size = t.size_vec2();
    let h = (w * size.y / size.x.max(1.0)).min(220.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter().image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// The page itself.
pub(crate) fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(scope) = state.scope.clone() else { return };
    let staged = state.staged;
    if let Some(open) = state.open.as_mut() {
        open.me_id = scope.vrchat_user().unwrap_or_default().to_string();
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
            // What may have been done here, on every tab, with its check (Blocker 2).
            let group = state.open.as_ref().map(|o| o.id.clone()).unwrap_or_default();
            let ctx = ui.ctx().clone();
            super::uncertain_panel(ui, &ctx, cx.ctl, state, &group);
            let Some(open) = state.open.as_ref() else { return };
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
#[allow(clippy::large_enum_variant)]
pub(crate) enum Act {
    Back,
    Tab(Tab),
    Reload,
    Studio(Option<super::art::Target>),
    Web,
    Write(Write, Rect),
    MoreMembers,
    MorePosts,
    Picker(Purpose),
    MorePictures,
    UploadIcon,
    /// Apply a picture uploaded earlier, shown with it (Blocker 5).
    ApplyUploaded(ArtSlot, String, Rect),
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
        Act::Studio(target) => {
            if let Some(o) = state.open.as_ref() {
                let mut studio = super::studio::Studio::new(o.card.clone(), scope.vrchat_user().unwrap_or_default(), None);
                if let Some(t) = target {
                    studio.target = t;
                }
                state.end_flow();
                state.studio = Some(studio);
            }
        }
        Act::Web => {
            if let Some(url) = state.open.as_ref().and_then(|o| g::web_page(&o.id)) {
                let r = crate::controller::open_url(&url);
                cx.run(ctx, r);
            }
        }
        Act::Write(write, origin) => {
            if let Some(o) = state.open.as_mut() {
                o.sent_editor = match &write {
                    Write::CreatePost { .. } | Write::EditPost { .. } => o.composer.as_ref().map(|c| c.number),
                    Write::CreateRole { .. } | Write::EditRole { .. } => o.role_editor.as_ref().map(|e| e.number),
                    _ => None,
                };
            }
            state.prepare(ctx, cx.ctl, write, origin)
        }
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
        Act::ApplyUploaded(slot, file, origin) => {
            let Some(group) = state.open.as_ref().map(|o| o.id.clone()) else { return };
            state.prepare(ctx, cx.ctl, Write::ApplyArt { group, slot, file }, origin);
        }
        Act::UploadIcon => {
            let Some(path) = rfd::FileDialog::new().set_title(tr("Choose a picture for the icon")).add_filter(tr("Pictures"), &["png", "jpg", "jpeg", "webp"]).pick_file() else { return };
            let Some(group) = state.open.as_ref().map(|o| o.id.clone()) else { return };
            match icon_png(&path) {
                // The PNG waits in the staging folder; only its name and hash cross the link (Major 1).
                Ok(png) => match super::stage_upload(&png) {
                    Ok(staged) => state.prepare(ctx, cx.ctl, Write::Upload { tag: FileTag::Icon, group, staged, art: None }, state.origin),
                    Err(_) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Error, tr("The file could not be written.")),
                },
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
    let members = match group.as_ref().and_then(|g| g.member_count).or(card.member_count) {
        Some(n) => orbiters_i18n::tr!("{n} members", n = n),
        None => tr("Reading…").to_string(),
    };
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
    let can_art = open.can(Action::ApplyArt);
    let art = can_art.allowed();
    let group = open.page.as_ref().map(|p| p.group.clone()).unwrap_or_default();
    // Pictures uploaded for this group and not applied yet, kept with the account (Major 8).
    let uploaded = state.uploads(&open.id);
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
            let pick = match authority_text(&can_art) {
                Some(why) => pick.hint(why),
                None => pick,
            };
            if pick.show(ui).clicked() {
                acts.push(Act::Picker(Purpose::Art(ArtSlot::Banner)));
            }
        });
        unknown(ui, &can_art, "art", acts);
        if can_art.refused().is_some() {
            note(ui, tr("Your role cannot change the group's pictures. The studio still makes and saves banners for whoever can."), theme::FG_3);
        }
    });
    if let Some((purpose @ Purpose::Art(slot), files, more, loading, failed)) = picker.clone() {
        pictures_grid(ui, cx, state, scope, purpose, &files, more, loading, failed, acts, |file, rect| Act::Write(Write::ApplyArt { group: group.id.clone(), slot, file: file.id.clone() }, rect));
    }
    for up in uploaded {
        let (slot, file) = (up.slot, up.file.clone());
        let thumb = state.picture(&ctx, cx.ctl, scope, &up.url());
        card(ui, icons::UPLOAD_SIMPLE, tr("Uploaded, not applied yet"), |ui| {
            if let Some(t) = &thumb {
                let w = 220.0f32.min(ui.available_width());
                let (r, _) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::hover());
                paint::squircle_texture(ui.painter(), r, [theme::squircle(12.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
            }
            note(ui, tr("The picture is in your VRChat inventory. The group is unchanged until you apply it."), theme::FG_2);
            let label = if slot == ArtSlot::Icon { tr("Apply as the icon") } else { tr("Apply as the banner") };
            let resp = widgets::Button::new(label).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(!busy && art).id_salt(("look-apply-uploaded", file.as_str())).show(ui);
            if widgets::activated_fresh(&resp) {
                acts.push(Act::ApplyUploaded(slot, file.clone(), resp.rect));
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
        if !g::NameplateApply::current().can_apply() {
            note(ui, tr("Direct apply is unavailable in this version."), theme::WARN);
        }
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
        // The search: three characters ask VRChat (after a pause in typing); fewer filter what is loaded (M8). One read
        // of the list at a time: a newer query waits for the one on its way (Major 9).
        let w = ui.available_width();
        let before = o.members.query.clone();
        widgets::search_field_clearable(ui, "group-members-search", &mut o.members.query, tr("Search members by name (3 letters ask VRChat)"), w.min(420.0));
        if o.members.query != before {
            o.members.typed_at = now;
        }
        let wanted = if o.members.query.trim().chars().count() >= 3 { o.members.query.trim().to_string() } else { String::new() };
        let reading = jobs.busy_with(|k| matches!(k, Kind::Members { group, .. } if *group == o.id));
        if !staged && !reading && (!o.members.started || (wanted != o.members.asked && now - o.members.typed_at > 0.45)) {
            ask_members(&ctx, cx, jobs, scope, o);
        } else if wanted != o.members.asked {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    let Some(o) = state.open.as_ref() else { return };
    let filter = o.members.query.trim().to_lowercase();
    let short = !filter.is_empty() && filter.chars().count() < 3;
    let me = o.me().cloned();
    let open_for = o.member_roles.clone();
    let viewall = me.as_ref().is_some_and(|m| g::has(&m.permissions, g::MEMBERS_VIEWALL) || g::has(&m.permissions, g::MEMBERS_MANAGE));
    let failed = o.members.failed;
    let full = o.members.items.len() >= MEMBERS_MAX;
    // You first (the list VRChat gives never holds the account itself), then the members loaded: by index, nothing
    // cloned until its row is in view (Major 9).
    let mine_shown = me.is_some() && o.members.asked.is_empty() && (filter.is_empty() || o.me_name.to_lowercase().contains(&filter));
    let mut rows: Vec<Option<usize>> = Vec::new();
    if mine_shown {
        rows.push(None);
    }
    rows.extend(o.members.items.iter().enumerate().filter(|(_, m)| !short || m.display_name.as_deref().is_some_and(|n| n.to_lowercase().contains(&filter))).map(|(i, _)| Some(i)));
    if !viewall {
        note(ui, tr("VRChat shows your role only some members (friends among them); the full list needs the right to view all members."), theme::FG_3);
    }
    if let Some(f) = failed {
        note(ui, &failure_text(f), theme::WARN);
    }
    if short {
        note(ui, tr("Showing matches among the members loaded. Type three letters to search the whole group."), theme::FG_3);
    }
    if full {
        note(ui, tr("Many members are loaded: search by name to find someone."), theme::FG_3);
    }
    let w = ui.available_width();
    for row in rows {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
        let Some(o) = state.open.as_mut() else { return };
        let id = match row {
            None => me.as_ref().map(|me| me.user_id.as_str()),
            Some(i) => o.members.items.get(i).map(|m| m.user_id.as_str()),
        };
        let open_now = id.is_some() && open_for.as_deref() == id;
        if !ui.is_rect_visible(rect.expand(120.0)) && !open_now {
            continue;
        }
        #[cfg(test)]
        {
            o.rows_built += 1;
        }
        let member = match row {
            None => me.as_ref().map(|me| Member { user_id: me.user_id.clone(), display_name: Some(o.me_name.clone()), role_ids: me.role_ids.clone(), roles_known: true, ..Default::default() }),
            Some(i) => o.members.items.get(i).cloned(),
        };
        let Some(m) = member else { continue };
        let mine = row.is_none();
        let roles: Vec<Role> = o.roles().to_vec();
        // Roles can be given or taken back only on a whole record (M8, Blocker 4), when some role allows it.
        let assignable = (m.complete() && m.roles_known || mine) && roles.iter().any(|r| o.can(Action::AssignRole { role: &r.id, user: &m.user_id, add: !m.role_ids.contains(&r.id) }).allowed());
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
            let chip = Txt::new(g::label(&r.name, g::ROLE_NAME_MAX), 11.5).weight(weight::MEDIUM).color(if r.management || r.is_owner_role() { theme::ACCENT } else { theme::FG_2 });
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
        let name = g::label(&r.name, g::ROLE_NAME_MAX);
        let label = if m.role_ids.contains(&r.id) { format!("✓ {name}") } else { name }; // i18n: not shown (a check mark before a name)
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
        // Group, member, role and direction are the press's: a list read again under a press that would turn "give"
        // into "take back" makes it another control, which that press is not on (Major 3).
        let resp = ui.interact(cr, Id::unique(("assign", o.id.as_str(), m.user_id.as_str(), r.id.as_str(), has)), if verdict.allowed() && !busy { Sense::click() } else { Sense::hover() });
        let fill = if has { theme::alpha(theme::ACCENT, 0.2) } else { theme::INK_700 };
        p.rect_filled(cr, theme::radius(15.0), if resp.hovered() && verdict.allowed() { theme::mix(fill, theme::FG, 0.08) } else { fill });
        let color = if verdict.allowed() { if has { theme::ACCENT } else { theme::FG } } else { theme::FG_4 };
        widgets::text(&p, cr.center(), Align2::CENTER_CENTER, &Txt::new(label, 13.0).weight(weight::MEDIUM).color(color));
        if let Some(why) = authority_text(&verdict) {
            resp.clone().on_hover_text(why);
        }
        // A click released on this chip (a press dragged off it is no choice) or a fresh key: never on the press
        // alone (Major 3, Blocker 6).
        if verdict.allowed() && widgets::activated_fresh(&resp) {
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
        let b = match authority_text(&can_create) {
            Some(why) => b.hint(why),
            None => b,
        };
        if b.show(ui).clicked() {
            o.role_editor = Some(RoleEditor::new(None, RoleDraft::default()));
        }
        unknown(ui, &can_create, "roles", acts);
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
        let editable = r.whole && (o.can(Action::EditRole { role: &r.id, change: &rename }).allowed() || o.can(Action::EditRole { role: &r.id, change: &rights }).allowed());
        let deletable = o.can(Action::DeleteRole { role: &r.id });
        let w = ui.available_width();
        let p = ui.painter().clone();
        let desc = if r.description.is_empty() { None } else { Some(p.layout_job(Txt::new(g::text(&r.description, g::ROLE_DESCRIPTION_MAX), 12.5).color(theme::FG_3).line_height(18.0).job(w - 260.0))) };
        let h = 64.0 + desc.as_ref().map_or(0.0, |d| d.size().y + 4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        widgets::frame(&p, rect, 14.0, theme::surface());
        let glyph = if r.is_owner_role() { icons::SHIELD_CHECK } else if r.default_role { icons::USERS_THREE } else if r.management { icons::SHIELD_CHECK } else { icons::TAG };
        widgets::icon(&p, pos2(rect.left() + 24.0, rect.top() + 24.0), glyph, 16.0, if r.management || r.is_owner_role() { theme::ACCENT } else { theme::FG_3 });
        widgets::text_truncated(&p, pos2(rect.left() + 44.0, rect.top() + 24.0), Align2::LEFT_CENTER, &Txt::new(g::label(&r.name, g::ROLE_NAME_MAX), 15.0).weight(weight::SEMIBOLD), w - 280.0);
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
            o.role_editor = Some(RoleEditor::new(Some(r.clone()), RoleDraft::of(r)));
        }
        let del = Rect::from_min_size(pos2(rect.right() - 108.0, rect.top() + 14.0), vec2(96.0, 32.0));
        if deletable.allowed() {
            let resp = widgets::Button::new(tr("Delete")).icon(icons::TRASH).variant(widgets::Variant::Danger).size(widgets::Size::Sm).enabled(!busy).id_salt(("role-delete", r.id.as_str())).show_at(ui, del);
            if widgets::activated_fresh(&resp) {
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
        // Held still while its write is prepared, confirmed and sent: what goes is what shows (Major 12).
        if busy {
            ui.disable();
        }
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
        if !fixed && !owner_role {
            toggle_row(ui, tr("Needs two-factor authentication"), tr("Only members with two-factor authentication on can have it."), &mut ed.draft.requires_two_factor);
        }
        // Rights: from VRChat's catalogue; those this account cannot give are shown but locked; the wildcard never.
        match catalogue {
            Some(cat) if rights_ok => {
                ui.label(egui::RichText::new(tr("Rights")).color(theme::FG_3).size(12.5));
                for perm in cat.iter().filter(|p| p.name != g::ALL) {
                    let mut on = ed.draft.permissions.contains(&perm.name);
                    let before = on;
                    let label = if perm.display_name.is_empty() { g::label(&perm.name, 64) } else { perm.display_name.clone() };
                    ui.add_enabled_ui(perm.allowed_to_add, |ui| toggle_row(ui, &label, &perm.help, &mut on));
                    if on != before {
                        ed.draft.permissions = g::toggled(&ed.draft.permissions, &perm.name, on, cat);
                    }
                }
            }
            _ => note(ui, tr("Its rights cannot be changed from here by your role."), theme::FG_3),
        }
        match &verdict {
            Some(a @ Authority::Unknown(_)) => unknown(ui, a, "role-editor", acts),
            Some(a) => {
                if let Some(why) = authority_text(a) {
                    note(ui, why, theme::WARN);
                }
            }
            None => {}
        }
        button_row(ui, |ui| {
            let resp = widgets::Button::new(tr("Save")).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(verdict.as_ref().is_some_and(|v| v.allowed()) && !busy).id_salt("role-save").show(ui);
            if widgets::activated_fresh(&resp) {
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
    let kept = !state.drafts.failed;
    let Some(o) = state.open.as_mut() else { return };
    let can_post = o.can(Action::Post);
    if o.composer.is_none() {
        let b = widgets::Button::new(tr("New post")).icon(icons::PLUS).variant(widgets::Variant::Primary).enabled(can_post.allowed() && !busy).id_salt("post-new");
        let b = match authority_text(&can_post) {
            Some(why) => b.hint(why),
            None => b,
        };
        if b.show(ui).clicked() {
            o.composer = Some(Composer::new(None, PostDraft::default()));
        }
        unknown(ui, &can_post, "posts", acts);
        if can_post.refused().is_some() {
            note(ui, tr("Writing posts needs the right to manage the group's announcements."), theme::FG_3);
        }
    }
    if o.composer.as_ref().is_some_and(|c| c.baseline.is_none()) {
        composer(ui, o, busy, kept, acts);
    }
    if let Some(f) = o.posts.failed {
        note(ui, &failure_text(f), theme::WARN);
    }
    let role_names: HashMap<String, String> = o.roles().iter().map(|r| (r.id.clone(), g::label(&r.name, g::ROLE_NAME_MAX))).collect();
    let count = o.posts.items.len();
    for index in 0..count {
        let Some(id) = o.posts.items.get(index).map(|p| p.id.as_str()) else { break };
        if o.composer.as_ref().and_then(|c| c.baseline.as_ref()).is_some_and(|b| b.id == id) {
            composer(ui, o, busy, kept, acts);
            continue;
        }
        let w = ui.available_width();
        // Copied and laid out only in view; elsewhere its last measured height (or a guess) holds its place (Major 9).
        let h = o.post_heights.get(id).copied().unwrap_or(140.0);
        let (slot, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        if !ui.is_rect_visible(slot.expand(200.0)) {
            continue;
        }
        #[cfg(test)]
        {
            o.rows_built += 1;
        }
        let Some(post) = o.posts.items.get(index).cloned() else { break };
        let p = ui.painter().clone();
        let text = p.layout_job(Txt::new(g::text(&post.text, g::POST_TEXT_MAX), 13.5).color(theme::FG_2).line_height(20.0).job(w - 48.0));
        let measured = 96.0 + text.size().y;
        if (measured - h).abs() > 0.5 {
            o.post_heights.insert(post.id.clone(), measured);
            ui.ctx().request_repaint();
        }
        let rect = Rect::from_min_size(slot.min, vec2(w, measured.min(h.max(measured))));
        widgets::frame(&p, rect, theme::CARD_RADIUS, theme::surface());
        widgets::text_truncated(&p, pos2(rect.left() + 24.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(g::label(&post.title, g::POST_TITLE_MAX), 16.0).weight(weight::SEMIBOLD), w - 260.0);
        let who = if post.visibility == Visibility::Public {
            tr("Public").to_string()
        } else if post.role_ids.is_empty() {
            tr("Members").to_string()
        } else {
            post.role_ids.iter().map(|r| role_names.get(r).cloned().unwrap_or_else(|| tr("A role").into())).collect::<Vec<_>>().join(", ")
        };
        // A date VRChat gave in another form is not shown as it is (Minor 1).
        let when = post.created_at.as_deref().map(super::super::local_time).unwrap_or_else(|| "—".to_string());
        widgets::text_truncated(&p, pos2(rect.left() + 24.0, rect.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(format!("{when} · {who}"), 12.0).color(theme::FG_3), w - 260.0);
        p.galley(pos2(rect.left() + 24.0, rect.top() + 70.0), text, theme::FG_2);
        if can_post.allowed() {
            // A post this version cannot carry back whole is read-only here (Blocker 3).
            if post.complete {
                let edit = Rect::from_min_size(pos2(rect.right() - 212.0, rect.top() + 14.0), vec2(96.0, 32.0));
                if widgets::Button::new(tr("Edit")).icon(icons::PENCIL_SIMPLE).size(widgets::Size::Sm).enabled(!busy).id_salt(("post-edit", post.id.as_str())).show_at(ui, edit).clicked() {
                    let draft = PostDraft { title: post.title.clone(), text: post.text.clone(), visibility: post.visibility, role_ids: post.role_ids.clone(), image_id: post.image_id.clone(), send_notification: false };
                    o.composer = Some(Composer::new(Some(post.clone()), draft));
                }
            let del = Rect::from_min_size(pos2(rect.right() - 108.0, rect.top() + 14.0), vec2(96.0, 32.0));
            let resp = widgets::Button::new(tr("Delete")).icon(icons::TRASH).variant(widgets::Variant::Danger).size(widgets::Size::Sm).enabled(!busy).id_salt(("post-delete", post.id.as_str())).show_at(ui, del);
                if widgets::activated_fresh(&resp) {
                    acts.push(Act::Write(Write::DeletePost { group: o.id.clone(), post: post.clone() }, resp.rect));
                }
            }
        }
    }
    let items_empty = o.posts.items.is_empty();
    if o.posts.loading {
        note(ui, tr("Loading posts…"), theme::FG_3);
    } else if items_empty && o.posts.failed.is_none() && o.posts.started {
        note(ui, tr("No posts yet."), theme::FG_3);
    } else if o.posts.more && widgets::Button::new(tr("Load more posts")).id_salt("posts-more").show(ui).clicked() {
        acts.push(Act::MorePosts);
    }
}

/// Writing or changing a post: notification off unless turned on, and only for a new one (M7).
fn composer(ui: &mut Ui, o: &mut Open, busy: bool, kept: bool, acts: &mut Vec<Act>) {
    let group = o.id.clone();
    let roles: Vec<Role> = o.roles().iter().filter(|r| !r.default_role).cloned().collect();
    let Some(c) = o.composer.as_mut() else { return };
    let before = c.draft.clone();
    let editing = c.baseline.is_some();
    let mut send: Option<Rect> = None;
    let mut cancel = false;
    let had_picture = c.baseline.as_ref().is_some_and(|b| b.image_id.is_some());
    card(ui, icons::CHAT_CENTERED_TEXT, if editing { tr("Change the post") } else { tr("New post") }, |ui| {
        // Held still while its write is prepared, confirmed and sent: what goes is what shows (Major 12).
        if busy {
            ui.disable();
        }
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
                    let name = g::label(&r.name, g::ROLE_NAME_MAX);
                    let chip = widgets::Chip::new(&name).selected(on).id_salt(("post-role", r.id.as_str()));
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
        // The post's picture: kept, or removed on purpose (Minor 6). VRCX sends `imageId: null` for a post without one.
        if had_picture {
            let mut keep = c.draft.image_id.is_some();
            toggle_row(ui, tr("Keep the post's picture"), tr("Turn it off to remove the picture from the post."), &mut keep);
            c.draft.image_id = if keep { c.baseline.as_ref().and_then(|b| b.image_id.clone()) } else { None };
        }
        if !editing {
            toggle_row(ui, tr("Send a notification"), tr("Members who follow the group's announcements get one. Off unless you turn it on."), &mut c.draft.send_notification);
        } else {
            note(ui, tr("Changing a post notifies nobody again."), theme::FG_3);
            note(ui, tr("A change of a post is not kept if you leave this page: the post is not copied to this computer."), theme::FG_3);
        }
        if !editing && !kept {
            note(ui, tr("What you write could not be kept on this computer: it goes if you leave this page."), theme::WARN);
        }
        let ready = !c.draft.title.trim().is_empty() && !c.draft.text.trim().is_empty();
        button_row(ui, |ui| {
            let resp = widgets::Button::new(if editing { tr("Review the change") } else { tr("Review the post") }).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(ready && !busy).id_salt("post-review").show(ui);
            if widgets::activated_fresh(&resp) {
                send = Some(resp.rect);
            }
            if widgets::Button::new(tr("Cancel")).variant(widgets::Variant::Ghost).id_salt("post-cancel").show(ui).clicked() {
                cancel = true;
            }
        });
    });
    if c.draft != before {
        c.restored = false;
    }
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
        let mut o = Open::new(GroupCard { id: "grp_1".into(), name: "Made-up".into(), ..Default::default() }, &Drafts::default());
        o.me_id = "usr_me".into();
        let roles = vec![
            Role { id: "grol_owner".into(), order: Some(0), permissions: vec!["*".into()], known: true, whole: true, ..Default::default() },
            Role { id: "grol_mod".into(), order: Some(2), management: true, permissions: vec!["group-instance-moderate".into()], known: true, whole: true, ..Default::default() },
            Role { id: "grol_artist".into(), order: Some(3), known: true, whole: true, ..Default::default() },
        ];
        let group = g::Group {
            id: "grp_1".into(),
            my_member: Some(g::MyMember { user_id: "usr_me".into(), role_ids: my_roles.iter().map(|s| s.to_string()).collect(), permissions: rights.iter().map(|s| s.to_string()).collect(), status: "member".into(), whole: true, group_id: Some("grp_1".into()) }),
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

    /// B2: an unknown outcome keeps the draft (its check is the window's, `window_tests`).
    #[test]
    fn an_unknown_post_keeps_its_draft() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.composer = Some(Composer::new(None, PostDraft { title: "t".into(), text: "x".into(), ..Default::default() }));
        o.ended(None, Outcome::Unknown);
        assert!(o.composer.is_some(), "the draft is kept");
    }

    /// Major 12: a done write clears only the editor it came from, still saying exactly what was sent: another editor with
    /// the same words (a recovered one), other flags, another notification or another post stays.
    #[test]
    fn a_done_write_clears_only_the_submitted_draft() {
        let sent = PostDraft { title: "t".into(), text: "first".into(), ..Default::default() };
        let done = || Outcome::Done(Done::Post(Post { id: "gpos_1".into(), ..Default::default() }));
        let sealed = Sealed::made_up(Write::CreatePost { group: "grp_1".into(), draft: sent.clone() }, true);
        let after = |composer: Composer, from: Option<u64>| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            let number = composer.number;
            o.composer = Some(composer);
            o.sent_editor = from.or(Some(number));
            o.ended(Some(&sealed), done());
            o.composer.is_some()
        };
        assert!(!after(Composer::new(None, sent.clone()), None), "the one sent is cleared");
        assert!(after(Composer::new(None, sent.clone()), Some(1)), "another editor with the same words stays");
        assert!(after(Composer::new(None, PostDraft { text: "first, corrected".into(), ..sent.clone() }), None), "a newer draft stays");
        assert!(after(Composer::new(None, PostDraft { send_notification: true, ..sent.clone() }), None), "another notification stays");
        assert!(after(Composer::new(Some(Post { id: "gpos_9".into(), ..Default::default() }), sent.clone()), None), "an edit of a post stays");
        let role = RoleDraft { name: "n".into(), ..Default::default() };
        let sealed = Sealed::made_up(Write::CreateRole { group: "grp_1".into(), draft: role.clone() }, true);
        let after = |draft: RoleDraft| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            let e = RoleEditor::new(None, draft);
            o.sent_editor = Some(e.number);
            o.role_editor = Some(e);
            o.ended(Some(&sealed), Outcome::Done(Done::Role(Role::default())));
            o.role_editor.is_some()
        };
        assert!(!after(role.clone()));
        assert!(after(RoleDraft { self_assignable: true, ..role.clone() }) && after(RoleDraft { added_on_join: true, ..role.clone() }) && after(RoleDraft { requires_two_factor: true, ..role }), "another flag stays");
    }

    /// J1, through the pages themselves: a held Enter on a focused Save, Delete or Review asks for nothing; a fresh press
    /// does.
    #[test]
    fn a_held_key_asks_for_no_write() {
        let key = |key, pressed: bool| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        type Draw = fn(&mut Ui, &mut State, &mut Vec<Act>);
        let ctl: crate::backend::Ctl = std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let snap = ctl.view();
        let pages: [(&str, &str, Draw); 4] = [
            ("the role's Save", "Save", |ui, s, acts| role_editor(ui, s.open.as_mut().unwrap(), Some(&[]), false, acts)),
            ("the post's Review", "Review the post", |ui, s, acts| composer(ui, s.open.as_mut().unwrap(), false, true, acts)),
            ("a role's Delete", "Delete", |ui, s, acts| {
                let ctl: crate::backend::Ctl = std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
                let snap = ctl.view();
                let mut toasts = orbiters_ui::toast::Toasts::default();
                roles(ui, &mut Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None }, s, acts)
            }),
            ("a post's Delete", "Delete", |ui, s, acts| {
                let ctl: crate::backend::Ctl = std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
                let snap = ctl.view();
                let mut toasts = orbiters_ui::toast::Toasts::default();
                let scope = crate::controller::vrchat_data::Scope::of(&snap);
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                posts(ui, &cx, s, &scope, acts)
            }),
        ];
        let _ = (&ctl, &snap);
        for (what, button, draw) in pages {
            let mut state = State { staged: true, ..Default::default() };
            let mut o = open_as(&["*"], &["grol_owner"]);
            o.page.as_mut().unwrap().group.roles.as_mut().unwrap().truncate(3);
            match what {
                "the role's Save" => o.role_editor = Some(RoleEditor::new(None, RoleDraft { name: "n".into(), ..Default::default() })),
                "the post's Review" => o.composer = Some(Composer::new(None, PostDraft { title: "t".into(), text: "x".into(), ..Default::default() })),
                "a post's Delete" => o.posts = PostList { items: vec![Post { id: "gpos_1".into(), title: "t".into(), text: "x".into(), complete: true, ..Default::default() }], started: true, ..Default::default() },
                _ => {}
            }
            state.open = Some(o);
            orbiters_ui::widgets::watch_controls();
            let ctx = egui::Context::default();
            let mut t = 0.0;
            let mut frame = |state: &mut State, events: Vec<egui::Event>| {
                t += 0.05;
                let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 900.0))), time: Some(t), ..Default::default() };
                input.events = events;
                let mut acts = Vec::new();
                let mut out = ctx.run_ui(input, |ui| draw(ui, state, &mut acts));
                out.textures_delta.clear();
                acts.into_iter().filter(|a| matches!(a, Act::Write(..))).count()
            };
            for _ in 0..4 {
                frame(&mut state, vec![]);
            }
            // Enter down before the button has the focus: what follows is its repeats.
            assert_eq!(frame(&mut state, vec![key(egui::Key::Enter, true)]), 0);
            let mut focused = false;
            for _ in 0..40 {
                frame(&mut state, vec![key(egui::Key::Tab, true)]);
                let at = orbiters_ui::widgets::control(&ctx, button).map(|(r, _)| r);
                if at.is_some() && ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)).map(|r| r.rect) == at {
                    focused = true;
                    break;
                }
            }
            assert!(focused, "{what}: its button takes the focus");
            assert_eq!(frame(&mut state, vec![key(egui::Key::Enter, true)]), 0, "{what}: a repeat");
            assert_eq!(frame(&mut state, vec![key(egui::Key::Enter, true), key(egui::Key::Enter, true)]), 0, "{what}");
            assert_eq!(frame(&mut state, vec![key(egui::Key::Enter, false)]), 0, "{what}: its release");
            assert_eq!(frame(&mut state, vec![key(egui::Key::Enter, true)]), 1, "{what}: a fresh press");
        }
    }

    /// J6: the owner's role editor offers its words only; what it offers is what the rules allow.
    #[test]
    fn the_owners_role_offers_only_its_words() {
        let shown = |id: &str| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            let r = o.roles().iter().find(|r| r.id == id).unwrap().clone();
            o.role_editor = Some(RoleEditor::new(Some(r.clone()), RoleDraft::of(&r)));
            let ctx = egui::Context::default();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| role_editor(ui, &mut o, Some(&[]), false, &mut Vec::new()));
            out.textures_delta.clear();
            words(&out)
        };
        for flag in [tr("Needs two-factor authentication"), tr("Members can take it themselves"), tr("Given to everyone who joins")] {
            assert!(!shown("grol_owner").contains(flag) && shown("grol_artist").contains(flag), "{flag}");
        }
    }

    /// What VRChat left out is explained, with a way to read it again; what the rules refuse is said as such.
    #[test]
    fn what_is_unknown_is_explained_and_can_be_read_again() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.page.as_mut().unwrap().group.my_member.as_mut().unwrap().whole = false;
        assert_eq!(o.can(Action::Post), Authority::Unknown(Missing::Membership));
        let mut other = open_as(&["*"], &["grol_owner"]);
        other.me_id = "usr_someone".into();
        assert_eq!(other.can(Action::Post), Authority::Unknown(Missing::Identity));
        let ctx = egui::Context::default();
        let a = o.can(Action::Post);
        let mut acts = Vec::new();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| unknown(ui, &a, "t", &mut acts));
        out.textures_delta.clear();
        let said = words(&out);
        assert!(said.contains(super::super::missing_text(Missing::Membership)) && said.contains(tr("Read again")), "{said}");
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

    /// Major 3: a role is given only by a click released on its chip; a press dragged off the chip and released
    /// elsewhere gives nothing.
    #[test]
    fn a_role_chip_acts_only_on_a_release_on_it() {
        let o = open_as(&["*"], &["grol_owner"]);
        let m = Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), display_name: Some("Sam".into()), roles_known: true, ..Default::default() };
        let roles = o.roles().to_vec();
        let ctx = egui::Context::default();
        let mut chip = Rect::NOTHING;
        let run = |events: Vec<egui::Event>, t: f64, chip_out: &mut Rect| {
            let mut acts = Vec::new();
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), time: Some(t), ..Default::default() };
            input.events = events;
            let mut out = ctx.run_ui(input, |ui| {
                role_chooser(ui, &o, &m, &roles, false, &mut acts);
                // The first chip ("Moderators"): where it was drawn.
                *chip_out = Rect::from_min_size(ui.min_rect().min + vec2(16.0, 32.0), vec2(40.0, 30.0));
            });
            out.textures_delta.clear();
            acts.len()
        };
        run(vec![], 0.0, &mut chip);
        let at = chip.center();
        let away = at + vec2(0.0, 300.0);
        let press = |p: egui::Pos2, down: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE };
        assert_eq!(run(vec![egui::Event::PointerMoved(at), press(at, true)], 0.1, &mut chip), 0, "nothing on the press");
        assert_eq!(run(vec![egui::Event::PointerMoved(away)], 0.2, &mut chip), 0);
        assert_eq!(run(vec![press(away, false)], 0.3, &mut chip), 0, "released elsewhere: no choice");
        assert_eq!(run(vec![egui::Event::PointerMoved(at), press(at, true)], 1.0, &mut chip), 0);
        assert_eq!(run(vec![press(at, false)], 1.1, &mut chip), 1, "released on the chip: one choice");
    }

    /// Major 3: a role read as held while the press is down turns "give" into "take back": that press does neither.
    #[test]
    fn a_role_chip_never_turns_round_under_a_press() {
        let o = open_as(&["*"], &["grol_owner"]);
        let without = Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), display_name: Some("Sam".into()), roles_known: true, ..Default::default() };
        let first = o.roles().iter().find(|r| o.can(Action::AssignRole { role: &r.id, user: &without.user_id, add: true }).allowed()).unwrap().id.clone();
        let with = Member { role_ids: vec![first], ..without.clone() };
        let roles = o.roles().to_vec();
        let run = |ctx: &egui::Context, m: &Member, events: Vec<egui::Event>, t: f64, at: &mut egui::Pos2| {
            let mut acts = Vec::new();
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), time: Some(t), ..Default::default() };
            input.events = events;
            let mut out = ctx.run_ui(input, |ui| {
                role_chooser(ui, &o, m, &roles, false, &mut acts);
                *at = ui.min_rect().min + vec2(36.0, 47.0);
            });
            out.textures_delta.clear();
            acts.into_iter().filter_map(|a| if let Act::Write(Write::MemberRole { add, .. }, _) = a { Some(add) } else { None }).collect::<Vec<_>>()
        };
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        // (what the member has at the press, at the release) -> what is asked
        for (down, up, asked) in [(&without, &without, vec![true]), (&with, &with, vec![false]), (&without, &with, vec![]), (&with, &without, vec![])] {
            let (ctx, mut at) = (egui::Context::default(), egui::Pos2::ZERO);
            run(&ctx, down, vec![], 0.0, &mut at);
            run(&ctx, down, vec![egui::Event::PointerMoved(at)], 0.1, &mut at);
            assert!(run(&ctx, down, vec![press(at, true)], 0.2, &mut at).is_empty());
            assert!(run(&ctx, up, vec![], 0.3, &mut at).is_empty());
            assert_eq!(run(&ctx, up, vec![press(at, false)], 0.4, &mut at), asked);
        }
    }

    /// Minor 1: a right's key with no display name, or one holding controls, is shown through the text policy.
    #[test]
    fn fallback_names_go_through_the_text_policy() {
        let mut s = Sealed::made_up(Write::DeleteRole { group: "grp_1".into(), role: "grol_a".into() }, true);
        s.labels = vec![("group-x".to_string(), "Bad\u{202E}name".to_string())];
        assert_eq!(right_label(&s, "group-x"), "Badname");
        assert_eq!(right_label(&s, "group-unknown\u{202E}x"), "group-unknownx");
    }

    /// Every word painted, in order.
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
    fn confirmed(sealed: &Sealed) -> String {
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(700.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| confirm_body(ui, sealed, None));
        out.textures_delta.clear();
        words(&out)
    }
    fn sealed(write: Write) -> Sealed {
        let mut s = Sealed::made_up(write, true);
        s.group_name = "Made-up Makers".into();
        s
    }

    /// Major 3: two members of the same name are told apart in the confirmation.
    #[test]
    fn a_confirmation_tells_namesakes_apart() {
        let (a, b) = ("usr_00000000-0000-4000-8000-00000000aaaa", "usr_00000000-0000-4000-8000-00000000bbbb");
        let said = |user: &str| {
            let mut s = sealed(Write::MemberRole { group: "grp_1".into(), user: user.into(), role: "grol_mod".into(), add: true });
            s.member = Some(Member { user_id: user.into(), display_name: Some("Sam".into()), ..Default::default() });
            confirmed(&s)
        };
        assert!(said(a).contains("Sam") && said(a).contains("00aaaa"), "{}", said(a));
        assert_ne!(said(a), said(b));
    }

    /// verify2 B-F: every confirmation names the sealed group, by its name and id; an apply, its file; a warning is shown.
    #[test]
    fn a_confirmation_names_its_group_and_file() {
        let write = Write::ApplyArt { group: "grp_00000000-0000-4000-8000-0000000000aa".into(), slot: ArtSlot::Banner, file: "file_00000000-0000-4000-8000-00000000abcd".into() };
        let said = confirmed(&sealed(write.clone()));
        assert!(said.contains("Made-up Makers") && said.contains("0000aa") && said.contains("00abcd"), "{said}");
        let mut s = sealed(write);
        s.warning = Some(crate::controller::groups::Warning::AnotherMayHaveGone);
        let warned = confirmed(&s);
        assert!(warned.contains(warning_text(crate::controller::groups::Warning::AnotherMayHaveGone)), "{warned}");
    }

    /// Drives a composer: keys a person presses, one per frame.
    fn compose(o: &mut Open, busy: bool, frames: Vec<Vec<egui::Event>>) {
        let ctx = egui::Context::default();
        for (i, events) in frames.into_iter().enumerate() {
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))), time: Some(i as f64 * 0.05), ..Default::default() };
            input.events = events;
            let mut acts = Vec::new();
            let mut out = ctx.run_ui(input, |ui| composer(ui, o, busy, true, &mut acts));
            out.textures_delta.clear();
            if o.composer.is_none() {
                return;
            }
        }
    }
    fn press(key: egui::Key) -> Vec<Vec<egui::Event>> {
        let e = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        vec![vec![e(true)], vec![e(false)]]
    }

    /// Major 12: while its write is prepared, confirmed or sent, the composer takes no edit; otherwise it does.
    #[test]
    fn the_composer_is_held_still_while_its_write_goes() {
        let typed = |busy: bool| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            o.composer = Some(Composer::new(None, PostDraft { title: "Half".into(), text: "written".into(), ..Default::default() }));
            let mut frames = vec![vec![], vec![]];
            for _ in 0..4 {
                frames.extend(press(egui::Key::Tab));
                frames.push(vec![egui::Event::Text("X".into())]);
            }
            compose(&mut o, busy, frames);
            let c = o.composer.expect("still being written").draft;
            (c.title, c.text)
        };
        assert_eq!(typed(true), ("Half".to_string(), "written".to_string()), "held still");
        let (title, text) = typed(false);
        assert!(title.contains('X') || text.contains('X'), "the same keys do type when it is not: {title} / {text}");
    }

    /// Minor 6, through the composer itself: its toggle removes the post's picture, and the confirmation says so.
    #[test]
    fn the_composers_toggle_removes_the_picture_and_the_confirmation_says_so() {
        let baseline = Post { id: "gpos_1".into(), title: "t".into(), text: "x".into(), image_id: Some("file_x".into()), complete: true, ..Default::default() };
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.composer = Some(Composer::new(Some(baseline.clone()), PostDraft { title: "t".into(), text: "x".into(), image_id: Some("file_x".into()), ..Default::default() }));
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let mut frame = |o: &mut Open, events: Vec<egui::Event>| {
            t += 0.1;
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))), time: Some(t), ..Default::default() };
            input.events = events;
            let mut out = ctx.run_ui(input, |ui| composer(ui, o, false, true, &mut Vec::new()));
            out.textures_delta.clear();
            out
        };
        frame(&mut o, vec![]);
        let out = frame(&mut o, vec![]);
        // The toggle is drawn left of its label (its switch is 48 x 28).
        let label = out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == tr("Keep the post's picture") => Some(t.pos),
            _ => None,
        });
        let label = label.expect("the composer of a post with a picture offers to remove it");
        let at = pos2(label.x - 34.0, label.y + 14.0);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        frame(&mut o, vec![egui::Event::PointerMoved(at)]);
        frame(&mut o, vec![press(true)]);
        frame(&mut o, vec![press(false)]);
        let c = o.composer.expect("still being written");
        assert!(c.removes_picture(), "the toggle removes the picture");
        let edit = c.draft;
        let said = confirmed(&sealed(Write::EditPost { group: "grp_1".into(), baseline, edit }));
        assert!(said.contains(tr("Removed from the post")), "{said}");
    }

    /// Major 9: a large roster costs its rows in view, not every member loaded, each frame.
    #[test]
    fn only_the_roster_rows_in_view_are_built() {
        let ctl: crate::backend::Ctl = std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let snap = ctl.view();
        let scope = crate::controller::vrchat_data::Scope::of(&snap);
        let mut state = State { staged: true, ..Default::default() };
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.members.started = true;
        o.members.items = (0..2000).map(|i| Member { user_id: format!("usr_00000000-0000-4000-8000-{i:012}"), display_name: Some(format!("Member {i}")), roles_known: true, ..Default::default() }).collect();
        state.open = Some(o);
        let ctx = egui::Context::default();
        let frames = 3;
        for t in 0..frames {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), time: Some(t as f64), ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut out = ctx.run_ui(input, |ui| {
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                egui::ScrollArea::vertical().max_height(600.0).show(ui, |ui| members(ui, &cx, &mut state, &scope, &mut Vec::new()));
            });
            out.textures_delta.clear();
        }
        let built = state.open.unwrap().rows_built / frames;
        assert!(built > 3 && built < 40, "{built} rows a frame of 2000");
    }

    /// Major 9: so does a long list of posts, and it is bounded.
    #[test]
    fn only_the_posts_in_view_are_built() {
        let ctl: crate::backend::Ctl = std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let snap = ctl.view();
        let scope = crate::controller::vrchat_data::Scope::of(&snap);
        let mut state = State { staged: true, ..Default::default() };
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.posts.started = true;
        for page in 0..30 {
            let items = (0..50).map(|i| Post { id: format!("gpos_{page}x{i}"), title: "t".into(), text: "x".into(), complete: true, ..Default::default() }).collect();
            o.answer(Kind::Posts { group: "grp_1".into(), offset: o.posts.offset }, Reply::Posts(orbiters_core::vrchat_api::Posts { items, raw: 50, total: None }));
        }
        assert_eq!((o.posts.items.len(), o.posts.more), (POSTS_MAX, false), "bounded");
        state.open = Some(o);
        let ctx = egui::Context::default();
        for t in 0..3 {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), time: Some(t as f64), ..Default::default() };
            let mut toasts = orbiters_ui::toast::Toasts::default();
            let mut out = ctx.run_ui(input, |ui| {
                let cx = Cx { ctl: &ctl, snap: &snap, toasts: &mut toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 0.0, go: None };
                egui::ScrollArea::vertical().max_height(600.0).show(ui, |ui| posts(ui, &cx, &mut state, &scope, &mut Vec::new()));
            });
            out.textures_delta.clear();
        }
        let built = state.open.unwrap().rows_built / 3;
        assert!(built > 1 && built < 20, "{built} posts a frame of {POSTS_MAX}");
    }

    /// Minor 6: an edit can remove the post's picture on purpose, and the confirmation says so.
    #[test]
    fn a_post_picture_can_be_removed_on_purpose() {
        let baseline = Post { id: "gpos_1".into(), image_id: Some("file_x".into()), complete: true, ..Default::default() };
        let c = Composer::new(Some(baseline.clone()), PostDraft { image_id: None, ..Default::default() });
        assert!(c.removes_picture());
        let kept = Composer::new(Some(baseline), PostDraft { image_id: Some("file_x".into()), ..Default::default() });
        assert!(!kept.removes_picture());
    }
}
