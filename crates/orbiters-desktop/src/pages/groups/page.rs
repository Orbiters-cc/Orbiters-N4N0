//! A group's page (REQUEST22 1): its pictures, with what changes them on the pictures themselves (REQUEST28), then Look (the
//! nameplate banner, its words), Members (search, roles), Roles (create, change, delete), Posts (write, delete; changed on
//! vrchat.com) and Events (`super::events`, read only). One scrolling region; what VRChat gives is shown as it
//! answered, and each control is offered only when `orbiters_core::groups::can` allows it on the data read (VRChat still
//! decides).
use super::{abilities_line, authority_text, failure_text, short_code, tail as id_tail, warning_text, Kind, State};
use super::art::Drafts;
use crate::controller::groups::{Done, Failure, Outcome, Page, Reply, Request, Sealed, Write};
use crate::pages::Cx;
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense, Ui};
use orbiters_core::groups::{self as g, Action, Actor, ArtSlot, Authority, Catalogue, FileTag, GroupCard, InventoryFile, Member, Missing, Post, PostDraft, Role, RoleChange, RoleDraft, RoleEdit, Visibility};
use orbiters_core::vrchat_api::GROUP_PAGE as PAGE;
use orbiters_i18n::{mark, tr};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, popup, widgets};
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
    Events,
}

#[derive(Default)]
pub(crate) struct MemberList {
    pub query: String,
    /// The query the items are of, and when the person last typed.
    asked: String,
    typed_at: f64,
    pub items: Vec<Member>,
    /// The memberships loaded (a person who left and came back is another row, verify4 F1), and where each person's row is,
    /// to keep each once without a scan per row (Major 9).
    seen: HashSet<String>,
    people: HashSet<String>,
    offset: usize,
    more: bool,
    loading: bool,
    failed: Option<Failure>,
    total: Option<u32>,
    started: bool,
    /// What shows is of a read before: its first page takes its place when it comes (nothing blanks meanwhile).
    again: bool,
}

impl MemberList {
    /// Read again from the start, what is there shown until then.
    fn read_again(&mut self) {
        (self.again, self.started, self.loading, self.offset, self.more, self.failed) = (!self.items.is_empty(), false, false, 0, false, None);
    }
    fn take(&mut self, m: orbiters_core::vrchat_api::Members) {
        if std::mem::take(&mut self.again) {
            self.items.clear();
            self.seen.clear();
            self.people.clear();
        }
        for item in m.items {
            if self.items.len() >= MEMBERS_MAX || !self.seen.insert(row_of(&item)) {
                continue;
            }
            // Their row of another membership goes: what VRChat gave later is who they are now.
            if !self.people.insert(item.user_id.clone()) {
                self.items.retain(|m| m.user_id != item.user_id);
            }
            self.items.push(item);
        }
        // Offsets advance by what VRChat gave, kept or not (M8).
        self.offset += m.raw;
        self.total = m.total;
        self.more = m.raw >= PAGE && m.total.is_none_or(|t| (self.offset as u32) < t) && self.items.len() < MEMBERS_MAX;
        self.failed = None;
    }
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
    /// As [`MemberList`]'s.
    again: bool,
}

impl PostList {
    fn read_again(&mut self) {
        (self.again, self.started, self.loading, self.offset, self.more, self.failed) = (!self.items.is_empty(), false, false, 0, false, None);
    }
    fn take(&mut self, p: orbiters_core::vrchat_api::Posts) {
        if std::mem::take(&mut self.again) {
            self.items.clear();
            self.seen.clear();
        }
        for item in p.items {
            if self.items.len() < POSTS_MAX && self.seen.insert(item.id.clone()) {
                self.items.push(item);
            }
        }
        self.offset += p.raw;
        self.more = p.raw >= PAGE && p.total.is_none_or(|t| (self.offset as u32) < t) && self.items.len() < POSTS_MAX;
        self.failed = None;
    }
}

pub(crate) struct RoleEditor {
    number: u64,
    form: RoleForm,
}

/// A new role, or one that is there as it was read and its words and flags as they are edited.
enum RoleForm {
    New(RoleDraft),
    Of(Box<Role>, RoleEdit),
}

impl RoleEditor {
    pub fn new(draft: RoleDraft) -> Self {
        Self { number: editor_number(), form: RoleForm::New(draft) }
    }
    pub fn of(baseline: Role, edit: RoleEdit) -> Self {
        Self { number: editor_number(), form: RoleForm::Of(Box::new(baseline), edit) }
    }
    fn baseline(&self) -> Option<&Role> {
        match &self.form {
            RoleForm::New(_) => None,
            RoleForm::Of(b, _) => Some(b),
        }
    }
}

/// A new post being written. Its id is its own for as long as it is written, on this computer and in the record of its
/// send: only that one is cleared when it is out (verify3 Major 6).
pub(crate) struct Composer {
    pub id: String,
    pub draft: PostDraft,
    /// Brought back from disk as it was, and not touched since.
    pub restored: bool,
}

impl Composer {
    pub fn new(draft: PostDraft) -> Self {
        Self { id: uuid::Uuid::new_v4().to_string(), draft, restored: false }
    }
}

/// The account's own pictures to choose from (gallery for banners, icons for the icon).
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

impl Picker {
    /// Made up for the screenshot review: read.
    pub fn staged(slot: ArtSlot, files: Vec<InventoryFile>) -> Self {
        Self { purpose: Purpose::Art(slot), files, offset: 0, more: false, loading: false, failed: None }
    }
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
    /// The page shown is from a read before ([`Open::as_kept`]): its lists are read once it is read again.
    kept: bool,
    failed: Option<Failure>,
    asked: bool,
    pub tab: Tab,
    pub members: MemberList,
    pub posts: PostList,
    pub events: super::events::EventList,
    /// Heights of post cards measured when drawn: the others take their place unmeasured (Major 9).
    post_heights: HashMap<String, f32>,
    /// The row whose roles are open under it ([`row_of`]: the membership, captured at the press).
    pub member_roles: Option<String>,
    pub role_editor: Option<RoleEditor>,
    pub composer: Option<Composer>,
    /// New posts no longer written (posted, cancelled): off this computer's disk at the next frame.
    pub drafts_over: Vec<String>,
    /// The role editor the write on its way came from.
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
        let composer = drafts.newest(&card.id).map(|k| Composer { id: k.id.clone(), draft: k.draft.clone(), restored: true });
        Self {
            id: card.id.clone(),
            card,
            page: None,
            kept: false,
            failed: None,
            asked: false,
            tab: if composer.is_some() { Tab::Posts } else { Tab::Look },
            members: MemberList::default(),
            posts: PostList::default(),
            events: Default::default(),
            post_heights: HashMap::new(),
            member_roles: None,
            role_editor: None,
            composer,
            drafts_over: Vec::new(),
            sent_editor: None,
            picker: None,
            me_id: String::new(),
            me_name: String::new(),
            #[cfg(test)]
            rows_built: 0,
        }
    }

    /// The page, its first members and posts as they were last read on this computer: shown at once, read again behind.
    pub(crate) fn as_kept(&mut self, scope: &crate::controller::vrchat_data::Scope) {
        use orbiters_core::vrchat_store::kind;
        let Some(page) = super::kept::<Page>(scope, kind::GROUP, &self.id).filter(|p| p.group.id == self.id) else { return };
        self.page = Some(page);
        self.kept = true;
        if let Some(m) = super::kept::<orbiters_core::vrchat_api::Members>(scope, kind::MEMBERS, &self.id) {
            self.members.take(m);
            self.members.read_again();
        }
        if let Some(p) = super::kept::<orbiters_core::vrchat_api::Posts>(scope, kind::POSTS, &self.id) {
            self.posts.take(p);
            self.posts.read_again();
        }
        self.events.as_kept(super::kept(scope, kind::EVENTS, &self.id), super::kept(scope, kind::TOLD, &self.id));
    }
    /// The page shown is from a read before: nothing of it is asked until it is read again.
    pub(super) fn kept(&self) -> bool {
        self.kept
    }
    pub(super) fn group_name(&self) -> String {
        self.page.as_ref().map_or_else(|| self.card.name.clone(), |p| p.group.name.clone())
    }

    /// The new post being written is over: posted, or cancelled.
    pub(crate) fn end_composer(&mut self) {
        if let Some(c) = self.composer.take() {
            self.drafts_over.push(c.id);
        }
    }
    /// A role's or a member's name, where the page has it.
    pub(crate) fn role_name(&self, id: &str) -> Option<String> {
        self.roles().iter().find(|r| r.id == id).map(|r| g::label(&r.name, g::ROLE_NAME_MAX))
    }
    pub(crate) fn member_name(&self, user: &str) -> Option<String> {
        if user == self.me_id {
            return Some(self.me_name.clone());
        }
        self.members.items.iter().find(|m| m.user_id == user).and_then(|m| m.display_name.clone())
    }

    /// The membership of `user` as the page has it.
    pub(crate) fn membership_of(&self, user: &str) -> Option<g::Membership> {
        if user == self.me_id {
            return self.me().and_then(|m| m.membership());
        }
        self.members.items.iter().find(|m| m.user_id == user).and_then(Member::membership)
    }

    /// Everything read again (after a check, or a refusal that says the page is out of date).
    pub fn reload(&mut self) {
        self.asked = false;
    }

    pub(crate) fn answer(&mut self, kind: Kind, reply: Reply) {
        match (kind, reply) {
            (Kind::Open(id), Reply::Opened(p)) if id == self.id => {
                self.page = Some(*p);
                self.kept = false;
                self.failed = None;
                // What was listed may have changed: read again, what is there shown until then.
                if self.members.asked.is_empty() {
                    self.members.read_again();
                } else {
                    self.members = MemberList { query: std::mem::take(&mut self.members.query), ..Default::default() };
                }
                self.posts.read_again();
                self.events.again();
            }
            (Kind::Open(id), Reply::Failed(f)) if id == self.id => self.failed = Some(f),
            (Kind::Members { group, query, offset }, reply) if group == self.id && query == self.members.asked && offset == self.members.offset => {
                self.members.loading = false;
                match reply {
                    Reply::Members(m) => self.members.take(m),
                    Reply::Failed(f) => self.members.failed = Some(f),
                    _ => {}
                }
            }
            (Kind::Posts { group, offset }, reply) if group == self.id && offset == self.posts.offset => {
                self.posts.loading = false;
                match reply {
                    Reply::Posts(p) => self.posts.take(p),
                    Reply::Failed(f) => self.posts.failed = Some(f),
                    _ => {}
                }
            }
            (Kind::Events { group, month, offset }, reply) if group == self.id => self.events.answer(month, offset, reply),
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
                    // Of the very membership it was for: another row says nothing of it.
                    let of = match sealed.map(|s| &s.write) {
                        Some(Write::MemberRole { membership, .. }) => Some(membership.clone()),
                        _ => None,
                    };
                    let same = |m: Option<g::Membership>| of.is_none() || m == of;
                    if let Some(m) = self.members.items.iter_mut().find(|m| m.user_id == user && same(m.membership())) {
                        m.role_ids = role_ids.clone();
                    }
                    if let Some(me) = self.page.as_mut().and_then(|p| p.group.my_member.as_mut()).filter(|me| me.user_id == user && same(me.membership())) {
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
                    // The very one it came from, still saying the very same.
                    let same = |c: &Composer| matches!(sealed.map(|s| &s.write), Some(Write::CreatePost { draft, of, .. }) if *of == c.id && c.draft.content() == draft.content() && c.draft.send_notification == draft.send_notification);
                    if self.composer.as_ref().is_some_and(same) {
                        self.end_composer();
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
                if matches!(f, Failure::Changed | Failure::Rejoined | Failure::PostChanged(_) | Failure::RoleChanged(_) | Failure::Withheld(_) | Failure::NotRead(_) | Failure::Api(orbiters_core::vrchat_api::GroupFailure::Forbidden)) {
                    self.asked = false;
                    self.posts = PostList::default();
                }
            }
            Outcome::Unknown => {}
        }
    }

    fn clear_role_editor(&mut self, sealed: Option<&Sealed>, sent: Option<u64>) {
        let same = |e: &RoleEditor| match (&e.form, sealed.map(|s| &s.write)) {
            (RoleForm::New(d), Some(Write::CreateRole { draft, .. })) => d.as_role().digest_of(&g::RoleField::ALL) == draft.as_role().digest_of(&g::RoleField::ALL),
            (RoleForm::Of(b, e), Some(Write::EditRole { baseline, edit, .. })) => b.id == baseline.id && RoleChange::between(b, e) == RoleChange::between(baseline, edit),
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

/// A right as the role editor lists it: plain text, whatever VRChat gave.
fn listed_label(perm: &g::PermissionInfo) -> String {
    g::label(if perm.display_name.is_empty() { &perm.name } else { &perm.display_name }, 64)
}

/// A right by its display name, or its key through the text policy (Minor 1).
fn right_label(sealed: &Sealed, key: &str) -> String {
    sealed.label(key).map(|l| g::label(l, 64)).unwrap_or_else(|| g::label(key, 64))
}
fn role_label(sealed: &Sealed, id: &str) -> String {
    sealed.label(id).map(str::to_string).unwrap_or_else(|| format!("{} {}", tr("A role"), id_tail(id)))
}

pub(super) fn line(ui: &mut Ui, label: &str, value: &str) {
    let w = ui.available_width();
    let p = ui.painter().clone();
    let galley = p.layout_job(Txt::new(value, 13.5).color(theme::fg()).line_height(19.0).job(w - 150.0));
    // A long label (a flag's, in some languages) goes on as many lines as it takes: never over its value.
    let label = p.layout_job(Txt::new(label, 12.5).color(theme::fg_3()).line_height(19.0).job(140.0));
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y.max(label.size().y).max(19.0) + 10.0), Sense::hover());
    p.galley(rect.min, label, theme::fg_3());
    p.galley(pos2(rect.left() + 150.0, rect.top()), galley, theme::fg());
}

pub(super) fn note(ui: &mut Ui, text: &str, tone: Color32) {
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
            // The very membership it is for (verify3 Blocker 1).
            if let Some(at) = sealed.member.as_ref().and_then(|m| m.joined_at.as_deref()) {
                line(ui, tr("Joined"), &super::super::local_time(at));
            }
            if let Some(role) = &sealed.role {
                line(ui, tr("Role"), &g::label(&role.name, g::ROLE_NAME_MAX));
                if !role.permissions.is_empty() {
                    line(ui, tr("Its rights"), &rights(&role.permissions));
                }
                if role.management {
                    note(ui, tr("This is a management role: it lets them manage this group."), theme::WARN);
                }
                if !role.known {
                    note(ui, tr("VRChat does not say what this role lets them do."), theme::WARN);
                }
                if *add && role.requires_two_factor {
                    note(ui, two_factor(), theme::WARN);
                }
            }
            note(ui, if *add { tr("They get it at once. Taking it back later is a new change of its own.") } else { tr("They lose what this role lets them do at once.") }, theme::fg_3());
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
            for (on, label) in [(change.self_assignable, tr("Members can take it themselves")), (change.added_on_join, tr("Given to everyone who joins")), (change.requires_two_factor, tr("Needs two-factor authentication"))] {
                if let Some(on) = on {
                    line(ui, label, if on { tr("Yes") } else { tr("No") });
                }
            }
        }
        Write::DeleteRole { role: id, .. } => {
            if let Some(role) = &sealed.role {
                // The id with it: the very one, whatever others are called.
                line(ui, tr("Role"), &format!("{} {}", g::label(&role.name, g::ROLE_NAME_MAX), id_tail(id)));
                if !role.description.is_empty() {
                    line(ui, tr("Description"), &g::text(&role.description, 300));
                }
                if !role.permissions.is_empty() {
                    line(ui, tr("Its rights"), &rights(&role.permissions));
                }
            }
            note(ui, tr("Members who have it lose it, and what it lets them do. This cannot be undone."), theme::DANGER);
        }
        Write::CreatePost { draft, .. } => {
            line(ui, tr("Title"), &draft.title);
            line(ui, tr("Text"), &draft.text);
            line(ui, tr("Who sees it"), if draft.visibility == Visibility::Public { tr("Anyone, on VRChat's website too") } else { tr("Members of the group") });
            if draft.visibility == Visibility::Group && !draft.role_ids.is_empty() {
                line(ui, tr("Restricted to"), &draft.role_ids.iter().map(|r| role_label(sealed, r)).collect::<Vec<_>>().join(", "));
            }
            line(ui, tr("Notification"), if draft.send_notification { tr("Yes: members who follow the group's announcements are notified") } else { tr("No") });
            note(ui, tr("Another moderator, or another computer, can post the same meanwhile: Orbiters only sees what goes from this one."), theme::fg_3());
        }
        Write::DeletePost { post, .. } => {
            // The post exactly as read now (Blocker 7).
            line(ui, tr("Title"), &g::label(&post.title, g::POST_TITLE_MAX));
            line(ui, tr("Text"), &g::text(&post.text, 300));
            line(ui, tr("Who sees it"), if post.visibility == Visibility::Public { tr("Anyone, on VRChat's website too") } else { tr("Members of the group") });
            if post.visibility == Visibility::Group && !post.role_ids.is_empty() {
                line(ui, tr("Restricted to"), &post.role_ids.iter().map(|r| role_label(sealed, r)).collect::<Vec<_>>().join(", "));
            }
            note(ui, tr("It disappears for everyone. This cannot be undone."), theme::DANGER);
        }
        Write::Upload { tag, .. } => {
            line(ui, tr("Goes to"), if *tag == FileTag::Icon { tr("Your VRChat icons") } else { tr("Your VRChat gallery") });
            if let Some(a) = &sealed.art {
                line(ui, tr("File"), &orbiters_i18n::tr!("PNG, {size}", size = orbiters_ui::format::bytes(a.size as f64)));
            }
            note(ui, tr("Nothing about the group changes yet: applying it is the next step."), theme::fg_3());
            note(ui, tr("VRChat says only when it is sent whether your inventory has room for it."), theme::fg_3());
        }
        Write::ApplyArt { slot, file, back, .. } => {
            if back.is_some() {
                note(ui, tr("The picture that was there before, as it was read then. What else moves with it is looked at again after it."), theme::fg_2());
            }
            line(ui, tr("Picture"), if *slot == ArtSlot::Icon { tr("The group's icon") } else { tr("The group's banner") });
            line(ui, tr("File"), &id_tail(file));
            note(ui, tr("Everyone who looks at the group sees it."), theme::fg_3());
            if back.is_none() {
                note(ui, tr("VRChat takes the file, not this picture of it: it is read again just before, and not sent if it changed."), theme::fg_3());
            }
        }
    }
    note(ui, tr("It is all read again right before it goes. What someone else changes in the instant between is not seen: VRChat has no way to close it."), theme::fg_3());
}

/// What VRChat's answer left out, reading it again, and where it is done whatever VRChat leaves out here.
fn unknown(ui: &mut Ui, a: &Authority, salt: &str, acts: &mut Vec<Act>) {
    if let (Authority::Unknown(_), Some(text)) = (a, authority_text(a)) {
        unread(ui, text, salt, acts);
    }
}
fn unread(ui: &mut Ui, text: &str, salt: &str, acts: &mut Vec<Act>) {
    note(ui, text, theme::WARN);
    button_row(ui, |ui| {
        if widgets::Button::new(tr("Read again")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt(("group-read-again", salt)).show(ui).clicked() {
            acts.push(Act::Reload);
        }
        if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("group-unread-web", salt)).show(ui).clicked() {
            acts.push(Act::Web);
        }
    });
}

/// What a role that needs two-factor authentication asks of who gets it.
fn two_factor() -> &'static str {
    tr("This role needs two-factor authentication: VRChat may refuse it to an account that has not turned it on.")
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
        if !open.asked && !staged && !state.held {
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
            picker(ui, cx, state, &scope, &mut acts);
            match tab {
                Tab::Look => look(ui, cx, state, &scope, &mut acts),
                Tab::Members => members(ui, cx, state, &scope, &mut acts),
                Tab::Roles => roles(ui, cx, state, &mut acts),
                Tab::Posts => posts(ui, cx, state, &scope, &mut acts),
                Tab::Events => super::events::show(ui, cx, state, &scope, &mut acts),
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
    /// A page of the web: a calendar event's on vrchat.com, an event's on Orbiters or in Discord.
    Url(String),
    Write(Write, Rect),
    MoreMembers,
    MorePosts,
    /// The calendar read on from where it failed; earlier events.
    EventsAgain,
    EarlierEvents,
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
                let mut studio = super::studio::Studio::new(o.card.clone(), super::owner_of(scope), None);
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
        Act::EventsAgain => state.events_again(),
        Act::EarlierEvents => state.earlier_events(),
        Act::Url(url) => {
            let r = crate::controller::open_url(&url);
            cx.run(ctx, r);
        }
        Act::Write(write, origin) => {
            if let Some(o) = state.open.as_mut() {
                o.sent_editor = match &write {
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
            state.prepare(ctx, cx.ctl, Write::ApplyArt { group, slot, file, back: None }, origin);
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
            paint::fill_convex(&p, &paint::squircle_points(banner, theme::squircle(theme::CARD_RADIUS)), |at| theme::mix(theme::mix(tint, theme::ink_850(), 0.5), theme::ink_850(), ((at.x - banner.left()) / banner.width()).clamp(0.0, 1.0)))
        }
    }
    paint::stroke_squircle(&p, banner, theme::squircle(theme::CARD_RADIUS), theme::hairline());
    // What changes the group's pictures sits on them, for who may (REQUEST28).
    let editable = state.open.as_ref().is_some_and(|o| o.page.is_some() && o.can(Action::ApplyArt).allowed());
    let busy = state.flow.is_some();
    if editable {
        let at = Rect::from_min_size(pos2(banner.right() - 48.0, banner.top() + 12.0), vec2(36.0, 36.0));
        let items = [popup::Item::new(tr("Open the banner studio")).icon(icons::SPARKLE), popup::Item::new(tr("Choose from your VRChat gallery")).icon(icons::IMAGES)];
        match edit_button(ui, mark("Change the banner"), "banner", at, &items, busy) {
            Some(0) => acts.push(Act::Studio(Some(super::art::Target::GroupBanner))),
            Some(_) => acts.push(Act::Picker(Purpose::Art(ArtSlot::Banner))),
            None => {}
        }
    }
    let icon = Rect::from_min_size(pos2(banner.left() + 24.0, banner.bottom() - 44.0), vec2(88.0, 88.0));
    p.circle_filled(icon.center(), 48.0, theme::ink_900());
    match state.picture(&ctx, cx.ctl, scope, &icon_url) {
        Some(t) => paint::circle_texture(&p, icon, t.id(), super::super::launcher::cover_uv(t.size_vec2(), icon.size()), Color32::WHITE),
        None => widgets::avatar(&p, icon, None, &card.name),
    }
    if editable {
        let at = Rect::from_center_size(icon.center() + vec2(31.0, -31.0), vec2(32.0, 32.0));
        let items = [popup::Item::new(tr("Choose from your icons")).icon(icons::IMAGES), popup::Item::new(tr("Upload a new icon")).icon(icons::UPLOAD_SIMPLE)];
        match edit_button(ui, mark("Change the icon"), "icon", at, &items, busy) {
            Some(0) => acts.push(Act::Picker(Purpose::Art(ArtSlot::Icon))),
            Some(_) => {
                state.origin = at;
                acts.push(Act::UploadIcon);
            }
            None => {}
        }
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
    widgets::text_truncated(&p, pos2(tx, banner.bottom() + 48.0), Align2::LEFT_CENTER, &Txt::new(sub.join(" · "), 13.0).color(theme::fg_3()), w - tx + hero.left());
    // The tabs are always there (reading is a member's); what each offers follows the rights.
    let options = [
        widgets::SegOption::new(Tab::Look, tr("Look")),
        widgets::SegOption::new(Tab::Members, tr("Members")),
        widgets::SegOption::new(Tab::Roles, tr("Roles")),
        widgets::SegOption::new(Tab::Posts, tr("Posts")),
        widgets::SegOption::new(Tab::Events, tr("Events")),
    ];
    let mut chosen = tab;
    ui.horizontal(|ui| {
        if widgets::segmented(ui, "group-tabs", &mut chosen, &options, false) {
            acts.push(Act::Tab(chosen));
        }
    });
}

/// The list a picture's edit button opens, by which picture.
pub(super) fn edit_list(which: &str) -> Id {
    Id::unique(("group-edit-picture", which))
}

/// A small round button on a picture of the group and the list it opens; the row pressed.
fn edit_button(ui: &mut Ui, label: &'static str, which: &'static str, at: Rect, items: &[popup::Item], busy: bool) -> Option<usize> {
    let p = ui.painter();
    p.circle_filled(at.center(), at.width() * 0.5 + 2.0, theme::alpha(theme::ink_900(), 0.9));
    let resp = widgets::IconButton::new(icons::PENCIL_SIMPLE, label).size(at.width(), 16.0).backdrop(theme::ink_700()).id_salt(("group-edit", which)).show_at(ui, at);
    let width = popup::list_width(ui, items);
    popup::list_from(ui.ctx(), edit_list(which), at, !busy && widgets::activated(&resp), items, None, width)
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

pub(super) fn button_row(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        f(ui)
    });
}

/// The pictures to choose a banner or an icon from, under the group's head while one is being chosen.
fn picker(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, acts: &mut Vec<Act>) {
    let Some(open) = state.open.as_ref() else { return };
    let Some(p) = open.picker.as_ref() else { return };
    let (group, purpose, files, more, loading, failed) = (open.id.clone(), p.purpose, p.files.clone(), p.more, p.loading, p.failed);
    let Purpose::Art(slot) = purpose;
    pictures_grid(ui, cx, state, scope, purpose, &files, more, loading, failed, acts, |file, rect| Act::Write(Write::ApplyArt { group: group.clone(), slot, file: file.id.clone(), back: None }, rect));
}

fn look(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &crate::controller::vrchat_data::Scope, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let Some(open) = state.open.as_ref() else { return };
    let can_art = open.can(Action::ApplyArt);
    let art = can_art.allowed();
    let group = open.page.as_ref().map(|p| p.group.clone()).unwrap_or_default();
    // Pictures uploaded for this group and not applied yet, kept with the account (Major 8).
    let uploaded = state.uploads(&open.id);
    let busy = state.flow.is_some();
    // Why the pictures have no edit button, when it is not the role.
    unknown(ui, &can_art, "art", acts);
    for up in uploaded {
        let (slot, file) = (up.slot, up.file.clone());
        let thumb = state.picture(&ctx, cx.ctl, scope, &up.url());
        card(ui, icons::UPLOAD_SIMPLE, tr("Uploaded, not applied yet"), |ui| {
            if let Some(t) = &thumb {
                let w = 220.0f32.min(ui.available_width());
                let (r, _) = ui.allocate_exact_size(vec2(w, w * 9.0 / 16.0), Sense::hover());
                paint::squircle_texture(ui.painter(), r, [theme::squircle(12.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
            }
            note(ui, tr("The picture is in your VRChat inventory. The group is unchanged until you apply it."), theme::fg_2());
            let label = if slot == ArtSlot::Icon { tr("Apply as the icon") } else { tr("Apply as the banner") };
            let resp = widgets::Button::new(label).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(!busy && art).id_salt(("look-apply-uploaded", file.as_str())).show(ui);
            if widgets::activated_fresh(&resp) {
                acts.push(Act::ApplyUploaded(slot, file.clone(), resp.rect));
            }
        });
    }
    // The nameplate banner: exported, never applied by this version (B4).
    card(ui, icons::TAG, tr("Nameplate banner"), |ui| {
        note(ui, tr("VRChat's new nameplate banner comes in two modes: Anchored (2048 × 385) and Centered (2048 × 472). Make both in the studio from one picture, then upload them on vrchat.com."), theme::fg_2());
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
            note(ui, tr("No description, rules or links yet."), theme::fg_3());
        }
        note(ui, tr("The name, description, rules and links are changed on vrchat.com."), theme::fg_3());
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
        let m = ui.max_rect();
        let x = Rect::from_min_size(pos2(m.right() - 32.0, m.top() - 2.0), vec2(32.0, 32.0));
        if widgets::activated(&widgets::IconButton::new(icons::X, mark("Close")).size(32.0, 15.0).id_salt("group-picker-close").show_at(ui, x)) {
            acts.push(Act::Picker(purpose));
        }
        if let Some(f) = failed {
            note(ui, &failure_text(f), theme::WARN);
        }
        if files.is_empty() && !loading && failed.is_none() {
            note(ui, tr("Nothing here yet. VRChat+ lets you upload pictures to your gallery and icons."), theme::fg_3());
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
            widgets::frame(&p, r, 12.0, theme::ink_750());
            if let Some(t) = state.picture(&ctx, cx.ctl, scope, &f.url) {
                paint::squircle_texture(&p, r, [theme::squircle(12.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
            }
            if resp.hovered() {
                p.rect_stroke(r, theme::radius(12.0), egui::Stroke::new(2.0, theme::accent()), egui::StrokeKind::Inside);
            }
            if widgets::activated(&resp) && state.flow.is_none() {
                acts.push(choose(f, r));
            }
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if loading {
            note(ui, tr("Loading…"), theme::fg_3());
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
        if !staged && !o.kept && !reading && (!o.members.started || (wanted != o.members.asked && now - o.members.typed_at > 0.45)) {
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
        note(ui, tr("VRChat shows your role only some members (friends among them); the full list needs the right to view all members."), theme::fg_3());
    }
    if let Some(f) = failed {
        note(ui, &failure_text(f), theme::WARN);
    }
    if short {
        note(ui, tr("Showing matches among the members loaded. Type three letters to search the whole group."), theme::fg_3());
    }
    if full {
        note(ui, tr("Many members are loaded: search by name to find someone."), theme::fg_3());
    }
    let w = ui.available_width();
    let mut withheld = false;
    for row in rows {
        let Some(o) = state.open.as_mut() else { return };
        let id = match row {
            None => me.as_ref().map(|me| me.membership_id.clone()),
            Some(i) => o.members.items.get(i).map(row_of),
        };
        let open_now = id.is_some() && open_for == id;
        if !open_now {
            let next = Rect::from_min_size(ui.cursor().min, vec2(w, 60.0));
            if !ui.is_rect_visible(next.expand(120.0)) {
                ui.allocate_exact_size(next.size(), Sense::hover());
                continue;
            }
        }
        #[cfg(test)]
        {
            o.rows_built += 1;
        }
        let member = match row {
            None => me.as_ref().map(|me| Member { user_id: me.user_id.clone(), membership_id: me.membership_id.clone(), joined_at: me.joined_at.clone(), display_name: Some(o.me_name.clone()), role_ids: me.role_ids.clone(), roles_known: me.roles_known && me.managed_known, ..Default::default() }),
            Some(i) => o.members.items.get(i).cloned(),
        };
        let Some(m) = member else { continue };
        let mine = row.is_none();
        let roles: Vec<Role> = o.roles().to_vec();
        // A member's roles open in their own card, which grows with them (REQUEST28).
        let chooser = (open_now && (m.complete() || mine) && m.roles_known && m.membership().is_some()).then(|| Chooser::of(ui, &m, &roles, w - 24.0));
        let (card, _) = ui.allocate_exact_size(vec2(w, 60.0 + chooser.as_ref().map_or(0.0, |c| c.height)), Sense::hover());
        let rect = Rect::from_min_size(card.min, vec2(w, 60.0));
        // Roles can be given or taken back only on a whole record that says which membership it is (its id and when it began:
        // M8, Blocker 4, verify3 Blocker 1, verify4 F1), when some role allows it.
        let membership = m.membership();
        let asked = membership.clone().unwrap_or_else(|| g::Membership { id: String::new(), joined_at: String::new() });
        let may = roles.iter().any(|r| o.can(Action::AssignRole { role: &r.id, user: &m.user_id, membership: &asked, add: !m.role_ids.contains(&r.id) }).allowed());
        let whole = (m.complete() || mine) && m.roles_known && membership.is_some();
        let assignable = whole && may;
        withheld |= may && !whole;
        let p = ui.painter().clone();
        widgets::frame(&p, card, 14.0, theme::surface());
        let face = Rect::from_min_size(pos2(rect.left() + 12.0, rect.center().y - 18.0), vec2(36.0, 36.0));
        let pic = state.picture(&ctx, cx.ctl, scope, &m.icon_url);
        let name = m.display_name.clone().unwrap_or_else(|| tr("Someone VRChat does not name").to_string());
        widgets::avatar(&p, face, pic.as_ref(), &name);
        let label = if mine { orbiters_i18n::tr!("{name} (you)", name = name) } else { name.clone() };
        widgets::text_truncated(&p, pos2(face.right() + 12.0, rect.top() + 20.0), Align2::LEFT_CENTER, &Txt::new(&label, 14.5).weight(weight::MEDIUM), w * 0.5);
        // Their roles as chips (Everyone's left out: everyone has it).
        let mut x = face.right() + 12.0;
        for r in roles.iter().filter(|r| m.role_ids.contains(&r.id) && !r.default_role) {
            let chip = Txt::new(g::label(&r.name, g::ROLE_NAME_MAX), 11.5).weight(weight::MEDIUM).color(if r.management || r.is_owner_role() { theme::accent() } else { theme::fg_2() });
            let cw = widgets::text_size(ui, &chip).x + 16.0;
            if x + cw > rect.right() - 120.0 {
                break;
            }
            let cr = Rect::from_min_size(pos2(x, rect.top() + 34.0), vec2(cw, 20.0));
            p.rect_filled(cr, theme::radius(10.0), theme::alpha(if r.management || r.is_owner_role() { theme::accent() } else { theme::fg_3() }, 0.14));
            widgets::text(&p, cr.center(), Align2::CENTER_CENTER, &chip);
            x += cw + 6.0;
        }
        if assignable {
            let b = Rect::from_min_size(pos2(rect.right() - 108.0, rect.center().y - 16.0), vec2(96.0, 32.0));
            if widgets::Button::new(tr("Roles")).icon(if open_now { icons::CARET_UP } else { icons::CARET_DOWN }).size(widgets::Size::Sm).id_salt(("member-roles", m.user_id.as_str())).show_at(ui, b).clicked() {
                if let Some(o) = state.open.as_mut() {
                    o.member_roles = if open_now { None } else { id };
                }
            }
        }
        if let (Some(chooser), Some(o)) = (chooser, state.open.as_ref()) {
            let under = Rect::from_min_max(pos2(card.left() + 12.0, rect.bottom()), card.right_bottom() - vec2(12.0, 0.0));
            chooser.show(ui, o, &m, &roles, under, busy, acts);
        }
    }
    // Members shown whose record VRChat gave in part: their roles are left alone here, and it says why.
    if withheld {
        unread(ui, super::missing_text(Missing::Member), "members", acts);
    }
    let Some(o) = state.open.as_ref() else { return };
    if o.members.loading && !o.members.again {
        note(ui, tr("Loading members…"), theme::fg_3());
    } else if o.members.more && widgets::Button::new(tr("Load more members")).id_salt("members-more").show(ui).clicked() {
        acts.push(Act::MoreMembers);
    } else if o.members.items.is_empty() && !o.members.asked.is_empty() && o.members.failed.is_none() {
        note(ui, tr("Nobody in this group has that name."), theme::fg_3());
    }
}

/// The row a member's record is: their membership (one who left and came back is another), or the record alone when it does
/// not say which.
fn row_of(m: &Member) -> String {
    if m.membership_id.is_empty() { m.user_id.clone() } else { m.membership_id.clone() }
}

#[cfg(test)]
fn allowed_assign(o: &Open, r: &Role, user: &str) -> bool {
    o.can(Action::AssignRole { role: &r.id, user, membership: &g::Membership { id: "gmem_1".into(), joined_at: "2026-01-01T00:00:00Z".into() }, add: true }).allowed()
}

/// The roles a member can be given or have taken back, laid out under their row in their card.
struct Chooser {
    /// The role (its place in the group's), its label, and its chip's width and place.
    chips: Vec<(usize, String, f32, egui::Vec2)>,
    height: f32,
}

impl Chooser {
    /// What the chips start under: the hairline and what a press does.
    const TOP: f32 = 36.0;

    fn of(ui: &Ui, m: &Member, roles: &[Role], w: f32) -> Self {
        let found: Vec<(usize, String, f32)> = roles
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.default_role && !r.is_owners_of(roles))
            .map(|(i, r)| {
                let name = g::label(&r.name, g::ROLE_NAME_MAX);
                let label = if m.role_ids.contains(&r.id) { format!("✓ {name}") } else { name }; // i18n: not shown (a check mark before a name)
                let width = widgets::text_size(ui, &Txt::new(&label, 13.0).weight(weight::MEDIUM)).x + 28.0;
                (i, label, width.min(w))
            })
            .collect();
        let (spots, h) = widgets::flow(&found.iter().map(|c| c.2).collect::<Vec<_>>(), w, 30.0, 8.0);
        Self { chips: found.into_iter().zip(spots).map(|((i, label, cw), at)| (i, label, cw, at)).collect(), height: Self::TOP + h + 14.0 }
    }

    /// Each chip a press of its own: ids taken at the press (M3).
    #[allow(clippy::too_many_arguments)]
    fn show(&self, ui: &mut Ui, o: &Open, m: &Member, roles: &[Role], rect: Rect, busy: bool, acts: &mut Vec<Act>) {
        let Some(membership) = m.membership() else { return };
        let p = ui.painter().clone();
        p.hline(rect.x_range(), rect.top(), theme::hairline());
        widgets::text_truncated(&p, pos2(rect.left(), rect.top() + 18.0), Align2::LEFT_CENTER, &Txt::new(tr("Press a role to give it or take it back. Each press asks VRChat once."), 12.0).color(theme::fg_3()), rect.width());
        for (i, label, cw, at) in &self.chips {
            let Some(r) = roles.get(*i) else { continue };
            let has = m.role_ids.contains(&r.id);
            let verdict = o.can(Action::AssignRole { role: &r.id, user: &m.user_id, membership: &membership, add: !has });
            let cr = Rect::from_min_size(rect.min + vec2(0.0, Self::TOP) + *at, vec2(*cw, 30.0));
            // Group, member, membership, role and direction are the press's: a list read again under a press that would turn
            // "give" into "take back", or make it someone's new membership, makes it another control, which that press is not
            // on (Major 3, verify4 F1).
            let resp = ui.interact(cr, Id::unique(("assign", o.id.as_str(), m.user_id.as_str(), &membership, r.id.as_str(), has)), if verdict.allowed() && !busy { Sense::click() } else { Sense::hover() });
            let fill = if has { theme::alpha(theme::accent(), 0.2) } else { theme::ink_700() };
            p.rect_filled(cr, theme::radius(15.0), if resp.hovered() && verdict.allowed() { theme::mix(fill, theme::fg(), 0.08) } else { fill });
            let color = if verdict.allowed() { if has { theme::accent() } else { theme::fg() } } else { theme::fg_4() };
            widgets::text(&p, cr.center(), Align2::CENTER_CENTER, &Txt::new(label, 13.0).weight(weight::MEDIUM).color(color));
            if let Some(why) = authority_text(&verdict) {
                resp.clone().on_hover_text(why);
            } else if !has && r.requires_two_factor {
                resp.clone().on_hover_text(two_factor());
            }
            // A click released on this chip (a press dragged off it is no choice) or a fresh key: never on the press
            // alone (Major 3, Blocker 6).
            if verdict.allowed() && widgets::activated_fresh(&resp) {
                acts.push(Act::Write(Write::MemberRole { group: o.id.clone(), user: m.user_id.clone(), membership: membership.clone(), role: r.id.clone(), add: !has }, cr));
            }
        }
    }
}

#[cfg(test)]
fn role_chooser(ui: &mut Ui, o: &Open, m: &Member, roles: &[Role], busy: bool, acts: &mut Vec<Act>) {
    let chooser = Chooser::of(ui, m, roles, ui.available_width());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), chooser.height), Sense::hover());
    chooser.show(ui, o, m, roles, rect, busy, acts);
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
    if o.role_editor.as_ref().is_none_or(|e| e.baseline().is_some()) {
        let b = widgets::Button::new(tr("New role")).icon(icons::PLUS).variant(widgets::Variant::Primary).enabled(can_create.allowed() && !busy).id_salt("role-new");
        let b = match authority_text(&can_create) {
            Some(why) => b.hint(why),
            None => b,
        };
        if b.show(ui).clicked() {
            o.role_editor = Some(RoleEditor::new(RoleDraft::default()));
        }
        unknown(ui, &can_create, "roles", acts);
    }
    // Rights are chosen when a role is made; those of a role that is there are only read here.
    if !roles.is_empty() && can_create.allowed() {
        rights_on_the_web(ui, "roles", acts);
    }
    if o.role_editor.as_ref().is_some_and(|e| e.baseline().is_none()) {
        role_editor(ui, o, catalogue.as_deref(), busy, acts);
    }
    for r in &roles {
        if o.role_editor.as_ref().and_then(|e| e.baseline()).is_some_and(|b| b.id == r.id) {
            role_editor(ui, o, catalogue.as_deref(), busy, acts);
            continue;
        }
        let rename = RoleChange { name: Some(String::new()), ..Default::default() };
        let editable = r.whole && o.can(Action::EditRole { role: &r.id, change: &rename }).allowed();
        let deletable = o.can(Action::DeleteRole { role: &r.id });
        let w = ui.available_width();
        let p = ui.painter().clone();
        let desc = if r.description.is_empty() { None } else { Some(p.layout_job(Txt::new(g::text(&r.description, g::ROLE_DESCRIPTION_MAX), 12.5).color(theme::fg_3()).line_height(18.0).job(w - 260.0))) };
        // Its rights by name, as VRChat has them: read, not changed, here.
        let named = (!r.is_owner_role() && !r.permissions.is_empty()).then(|| p.layout_job(Txt::new(rights_text(&r.permissions, catalogue.as_deref()), 12.0).color(theme::fg_2()).line_height(17.0).job(w - 64.0)));
        let dh = desc.as_ref().map_or(0.0, |d| d.size().y + 4.0);
        let h = 64.0 + dh + named.as_ref().map_or(0.0, |n| n.size().y + 6.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        widgets::frame(&p, rect, 14.0, theme::surface());
        let glyph = if r.is_owner_role() { icons::SHIELD_CHECK } else if r.default_role { icons::USERS_THREE } else if r.management { icons::SHIELD_CHECK } else { icons::TAG };
        widgets::icon(&p, pos2(rect.left() + 24.0, rect.top() + 24.0), glyph, 16.0, if r.management || r.is_owner_role() { theme::accent() } else { theme::fg_3() });
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
        widgets::text_truncated(&p, pos2(rect.left() + 44.0, rect.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(tags.join(" · "), 12.0).color(theme::fg_3()), w - 280.0);
        if let Some(d) = desc {
            p.galley(pos2(rect.left() + 44.0, rect.top() + 58.0), d, theme::fg_3());
        }
        if let Some(n) = named {
            p.galley(pos2(rect.left() + 44.0, rect.top() + 58.0 + dh), n, theme::fg_2());
        }
        let edit = Rect::from_min_size(pos2(rect.right() - 212.0, rect.top() + 14.0), vec2(96.0, 32.0));
        if editable && widgets::Button::new(tr("Edit")).icon(icons::PENCIL_SIMPLE).size(widgets::Size::Sm).enabled(!busy).id_salt(("role-edit", r.id.as_str())).show_at(ui, edit).clicked() {
            o.role_editor = Some(RoleEditor::of(r.clone(), RoleEdit::of(r)));
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

/// Rights by VRChat's names for them, or their keys.
fn rights_text(list: &[String], catalogue: Option<&[g::PermissionInfo]>) -> String {
    list.iter().map(|key| catalogue.unwrap_or_default().iter().find(|p| p.name == *key).map(listed_label).unwrap_or_else(|| g::label(key, 64))).collect::<Vec<_>>().join(", ")
}

/// Where the rights of a role that is there are changed, and why not here.
fn rights_on_the_web(ui: &mut Ui, salt: &str, acts: &mut Vec<Act>) {
    note(ui, tr("The rights of an existing role are changed on vrchat.com: VRChat takes the whole list at once, so a change made here could undo another manager's."), theme::fg_3());
    button_row(ui, |ui| {
        if widgets::Button::new(tr("Change rights on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("role-rights-web", salt)).show(ui).clicked() {
            acts.push(Act::Web);
        }
    });
}

fn role_editor(ui: &mut Ui, o: &mut Open, catalogue: Option<&[g::PermissionInfo]>, busy: bool, acts: &mut Vec<Act>) {
    let group = o.id.clone();
    let Some(ed) = o.role_editor.as_ref() else { return };
    let title = if ed.baseline().is_some() { tr("Change the role") } else { tr("New role") };
    let owner_role = ed.baseline().is_some_and(|b| b.is_owner_role());
    let mut save: Option<Rect> = None;
    let mut cancel = false;
    let verdict = match &ed.form {
        RoleForm::Of(b, edit) => {
            let change = RoleChange::between(b, edit);
            if change.is_empty() { None } else { Some(o.can(Action::EditRole { role: &b.id, change: &change })) }
        }
        RoleForm::New(draft) => (!draft.name.trim().is_empty()).then(|| o.can(Action::CreateRole { draft })),
    };
    let ed = o.role_editor.as_mut().unwrap();
    card(ui, icons::PENCIL_SIMPLE, title, |ui| {
        // Held still while its write is prepared, confirmed and sent: what goes is what shows (Major 12).
        if busy {
            ui.disable();
        }
        let as_it_was = ed.baseline().map(|b| b.name.clone());
        let (name, description, self_assignable, added_on_join, two_factor) = match &mut ed.form {
            RoleForm::New(d) => (&mut d.name, &mut d.description, &mut d.self_assignable, &mut d.added_on_join, &mut d.requires_two_factor),
            RoleForm::Of(_, e) => (&mut e.name, &mut e.description, &mut e.self_assignable, &mut e.added_on_join, &mut e.requires_two_factor),
        };
        ui.label(egui::RichText::new(tr("Name")).color(theme::fg_3()).size(12.5));
        widgets::text_field(ui, "role-name", name, tr("A short name"), ui.available_width().min(420.0), false);
        if as_it_was.as_ref() != Some(&*name) {
            *name = name.chars().take(g::ROLE_NAME_MAX).collect();
        }
        ui.label(egui::RichText::new(tr("Description")).color(theme::fg_3()).size(12.5));
        multiline(ui, "role-description", description, tr("What this role is for"), g::ROLE_DESCRIPTION_MAX, 2);
        if !owner_role {
            for (value, label, help) in [
                (self_assignable, tr("Members can take it themselves"), tr("Anyone in the group can give it to themselves.")),
                (added_on_join, tr("Given to everyone who joins"), tr("New members get it as they join.")),
                (two_factor, tr("Needs two-factor authentication"), tr("Only members with two-factor authentication on can have it.")),
            ] {
                toggle_row(ui, label, help, value);
            }
        }
        match &mut ed.form {
            // A new role's rights, from VRChat's catalogue: those this account cannot give are shown but locked; the
            // wildcard never.
            RoleForm::New(draft) => match catalogue {
                Some(cat) => {
                    ui.label(egui::RichText::new(tr("Rights")).color(theme::fg_3()).size(12.5));
                    for perm in cat.iter().filter(|p| p.name != g::ALL) {
                        let mut on = draft.permissions.contains(&perm.name);
                        let before = on;
                        let label = listed_label(perm);
                        ui.add_enabled_ui(perm.allowed_to_add, |ui| toggle_row(ui, &label, &perm.help, &mut on));
                        if on != before {
                            draft.permissions = g::toggled(&draft.permissions, &perm.name, on, cat);
                        }
                    }
                }
                None => note(ui, tr("VRChat did not give the list of rights: the role can be made without any."), theme::fg_3()),
            },
            RoleForm::Of(role, _) => {
                ui.label(egui::RichText::new(tr("Rights")).color(theme::fg_3()).size(12.5));
                if owner_role {
                    note(ui, tr("Every right"), theme::fg_2());
                } else {
                    if !role.permissions.is_empty() {
                        note(ui, &rights_text(&role.permissions, catalogue), theme::fg_2());
                    }
                    rights_on_the_web(ui, "role-editor", acts);
                }
            }
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
        let write = match &ed.form {
            RoleForm::Of(b, edit) => Write::EditRole { group, baseline: (**b).clone(), edit: edit.clone(), back: None },
            RoleForm::New(draft) => Write::CreateRole { group, draft: draft.clone() },
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
            ui.label(egui::RichText::new(label).color(theme::fg()).size(13.5));
            if !help.is_empty() {
                ui.label(egui::RichText::new(help).color(theme::fg_3()).size(12.0));
            }
        });
    });
}

/// A field of several lines that grows with its text (no scrolling of its own: one scroll region per view).
fn multiline(ui: &mut Ui, salt: &str, text: &mut String, hint: &str, max: usize, rows: usize) {
    let font = theme::font(13.5, weight::REGULAR);
    egui::Frame::NONE.fill(theme::ink_750()).corner_radius(theme::radius(12.0)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(text)
                .id(Id::unique(("group-field", salt)))
                .frame(egui::Frame::NONE)
                .char_limit(max)
                .font(font.clone())
                .text_color(theme::fg())
                .hint_text(egui::RichText::new(hint).color(theme::fg_4()).font(font))
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
        if !o.posts.started && !staged && !o.kept {
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
            o.composer = Some(Composer::new(PostDraft::default()));
        }
        unknown(ui, &can_post, "posts", acts);
        if can_post.refused().is_some() {
            note(ui, tr("Writing posts needs the right to manage the group's announcements."), theme::fg_3());
        }
    }
    if o.composer.is_some() {
        composer(ui, o, busy, kept, acts);
    }
    if let Some(f) = o.posts.failed {
        note(ui, &failure_text(f), theme::WARN);
    }
    let role_names: HashMap<String, String> = o.roles().iter().map(|r| (r.id.clone(), g::label(&r.name, g::ROLE_NAME_MAX))).collect();
    let count = o.posts.items.len();
    for index in 0..count {
        let Some(id) = o.posts.items.get(index).map(|p| p.id.as_str()) else { break };
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
        let text = p.layout_job(Txt::new(g::text(&post.text, g::POST_TEXT_MAX), 13.5).color(theme::fg_2()).line_height(20.0).job(w - 48.0));
        let measured = 96.0 + text.size().y;
        if (measured - h).abs() > 0.5 {
            o.post_heights.insert(post.id.clone(), measured);
            ui.ctx().request_repaint();
        }
        let rect = Rect::from_min_size(slot.min, vec2(w, measured.min(h.max(measured))));
        widgets::frame(&p, rect, theme::CARD_RADIUS, theme::surface());
        widgets::text_truncated(&p, pos2(rect.left() + 24.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::new(g::label(&post.title, g::POST_TITLE_MAX), 16.0).weight(weight::SEMIBOLD), w - 340.0);
        let who = if post.visibility == Visibility::Public {
            tr("Public").to_string()
        } else if post.role_ids.is_empty() {
            tr("Members").to_string()
        } else {
            post.role_ids.iter().map(|r| role_names.get(r).cloned().unwrap_or_else(|| tr("A role").into())).collect::<Vec<_>>().join(", ")
        };
        // A date VRChat gave in another form is not shown as it is (Minor 1).
        let when = post.created_at.as_deref().map(super::super::local_time).unwrap_or_else(|| "—".to_string());
        widgets::text_truncated(&p, pos2(rect.left() + 24.0, rect.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(format!("{when} · {who}"), 12.0).color(theme::fg_3()), w - 48.0);
        p.galley(pos2(rect.left() + 24.0, rect.top() + 70.0), text, theme::fg_2());
        if can_post.allowed() {
            // Changed on its group's page: VRChat's edit carries the audience whole and takes no condition, so one made here
            // could undo another moderator's restriction (verify3 Blocker 4).
            let edit_w = widgets::text_size(ui, &Txt::new(tr("Edit on vrchat.com"), 13.0).weight(weight::MEDIUM)).x + 52.0;
            let edit = Rect::from_min_size(pos2(rect.right() - 116.0 - edit_w, rect.top() + 14.0), vec2(edit_w, 32.0));
            if widgets::Button::new(tr("Edit on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("post-edit-web", post.id.as_str())).show_at(ui, edit).clicked() {
                acts.push(Act::Web);
            }
            // A post not read whole is not deleted from here (Blocker 3, verify3 Major 5).
            if post.complete {
                let del = Rect::from_min_size(pos2(rect.right() - 108.0, rect.top() + 14.0), vec2(96.0, 32.0));
                let resp = widgets::Button::new(tr("Delete")).icon(icons::TRASH).variant(widgets::Variant::Danger).size(widgets::Size::Sm).enabled(!busy).id_salt(("post-delete", post.id.as_str())).show_at(ui, del);
                if widgets::activated_fresh(&resp) {
                    acts.push(Act::Write(Write::DeletePost { group: o.id.clone(), post: post.clone() }, resp.rect));
                }
            }
        }
    }
    let items_empty = o.posts.items.is_empty();
    if o.posts.loading && !o.posts.again {
        note(ui, tr("Loading posts…"), theme::fg_3());
    } else if items_empty && o.posts.failed.is_none() && o.posts.started {
        note(ui, tr("No posts yet."), theme::fg_3());
    } else if o.posts.more && widgets::Button::new(tr("Load more posts")).id_salt("posts-more").show(ui).clicked() {
        acts.push(Act::MorePosts);
    }
}

/// Writing a post: notification off unless turned on (M7).
fn composer(ui: &mut Ui, o: &mut Open, busy: bool, kept: bool, acts: &mut Vec<Act>) {
    let group = o.id.clone();
    let roles: Vec<Role> = o.roles().iter().filter(|r| !r.default_role).cloned().collect();
    let Some(c) = o.composer.as_mut() else { return };
    let before = c.draft.clone();
    let mut send: Option<Rect> = None;
    let mut cancel = false;
    card(ui, icons::CHAT_CENTERED_TEXT, tr("New post"), |ui| {
        // Held still while its write is prepared, confirmed and sent: what goes is what shows (Major 12).
        if busy {
            ui.disable();
        }
        widgets::text_field(ui, "post-title", &mut c.draft.title, tr("Title"), ui.available_width(), false);
        c.draft.title = c.draft.title.chars().take(g::POST_TITLE_MAX).collect();
        multiline(ui, "post-text", &mut c.draft.text, tr("What do you want to tell the group?"), g::POST_TEXT_MAX, 5);
        ui.label(egui::RichText::new(tr("Who sees it")).color(theme::fg_3()).size(12.5));
        ui.horizontal(|ui| {
            widgets::segmented(ui, "post-visibility", &mut c.draft.visibility, &[widgets::SegOption::new(Visibility::Group, tr("Members of the group")), widgets::SegOption::new(Visibility::Public, tr("Public"))], true);
        });
        if c.draft.visibility == Visibility::Group && !roles.is_empty() {
            note(ui, tr("Only for some roles? Choose them; none chosen reaches every member."), theme::fg_3());
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
        toggle_row(ui, tr("Send a notification"), tr("Members who follow the group's announcements get one. Off unless you turn it on."), &mut c.draft.send_notification);
        if !kept {
            note(ui, tr("What you write could not be kept on this computer: it goes if you leave this page."), theme::WARN);
        }
        let ready = !c.draft.title.trim().is_empty() && !c.draft.text.trim().is_empty();
        button_row(ui, |ui| {
            let resp = widgets::Button::new(tr("Review the post")).icon(icons::CHECK).variant(widgets::Variant::Primary).enabled(ready && !busy).id_salt("post-review").show(ui);
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
        o.end_composer();
    } else if let Some(rect) = send {
        let c = o.composer.as_ref().unwrap();
        acts.push(Act::Write(Write::CreatePost { group, draft: c.draft.clone(), of: c.id.clone() }, rect));
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
            my_member: Some(g::MyMember { user_id: "usr_me".into(), membership_id: "gmem_me".into(), role_ids: my_roles.iter().map(|s| s.to_string()).collect(), permissions: rights.iter().map(|s| s.to_string()).collect(), status: "member".into(), rights_known: true, roles_known: true, managed_known: true, group_id: Some("grp_1".into()), ..Default::default() }),
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
        o.composer = Some(Composer::new(PostDraft { title: "t".into(), text: "x".into(), ..Default::default() }));
        o.ended(None, Outcome::Unknown);
        assert!(o.composer.is_some(), "the draft is kept");
    }

    /// Major 12, verify3 Major 6: a done write clears only the very one it came from, still saying exactly what was sent:
    /// another with the same words (a later one, a recovered one), other flags or another notification stays.
    #[test]
    fn a_done_write_clears_only_the_submitted_draft() {
        let sent = PostDraft { title: "t".into(), text: "first".into(), ..Default::default() };
        let done = || Outcome::Done(Done::Post(Post { id: "gpos_1".into(), ..Default::default() }));
        let after = |change: &dyn Fn(&mut Composer)| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            let mut c = Composer::new(sent.clone());
            let sealed = Sealed::made_up(Write::CreatePost { group: "grp_1".into(), draft: sent.clone(), of: c.id.clone() }, true);
            change(&mut c);
            let id = c.id.clone();
            o.composer = Some(c);
            o.ended(Some(&sealed), done());
            assert_eq!(o.composer.is_none(), o.drafts_over == [id], "off the disk when, and only when, it is out");
            o.composer.is_some()
        };
        assert!(!after(&|_| {}), "the one sent is cleared");
        assert!(after(&|c| c.id = "another".into()), "another with the same words stays");
        assert!(after(&|c| c.draft.text = "first, corrected".into()), "a newer draft stays");
        assert!(after(&|c| c.draft.send_notification = true), "another notification stays");
        let role = RoleDraft { name: "n".into(), ..Default::default() };
        let sealed = Sealed::made_up(Write::CreateRole { group: "grp_1".into(), draft: role.clone() }, true);
        let after = |draft: RoleDraft| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            let e = RoleEditor::new(draft);
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
                "the role's Save" => o.role_editor = Some(RoleEditor::new(RoleDraft { name: "n".into(), ..Default::default() })),
                "the post's Review" => o.composer = Some(Composer::new(PostDraft { title: "t".into(), text: "x".into(), ..Default::default() })),
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
            o.role_editor = Some(RoleEditor::of(r.clone(), RoleEdit::of(&r)));
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

    /// A role that is there: its rights are named and changed on vrchat.com, with no switch for one; everyone's has no
    /// Edit. A new one has its rights to choose.
    #[test]
    fn the_rights_of_a_role_that_is_there_are_read_only() {
        let cat = [g::PermissionInfo { name: "group-instance-moderate".into(), display_name: "Moderate instances".into(), allowed_to_add: true, ..Default::default() }];
        let shown = |editor: RoleEditor| {
            let mut o = open_as(&["*"], &["grol_owner"]);
            o.role_editor = Some(editor);
            let ctx = egui::Context::default();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| role_editor(ui, &mut o, Some(&cat), false, &mut Vec::new()));
            out.textures_delta.clear();
            words(&out)
        };
        let r = open_as(&["*"], &["grol_owner"]).roles().iter().find(|r| r.id == "grol_mod").unwrap().clone();
        let (edits, makes) = (shown(RoleEditor::of(r.clone(), RoleEdit::of(&r))), shown(RoleEditor::new(RoleDraft::default())));
        assert!(edits.contains("Moderate instances") && edits.contains(tr("Change rights on vrchat.com")), "{edits}");
        assert!(makes.contains("Moderate instances") && !makes.contains(tr("Change rights on vrchat.com")), "{makes}");
        let mut o = open_as(&["*"], &["grol_owner"]);
        let roles = o.page.as_mut().unwrap().group.roles.as_mut().unwrap();
        roles.push(Role { id: "grol_all".into(), name: "Everyone".into(), order: Some(9), default_role: true, known: true, whole: true, permissions: vec!["group-instance-moderate".into()], ..Default::default() });
        let change = RoleChange { name: Some("x".into()), ..Default::default() };
        assert!(!o.can(Action::EditRole { role: "grol_all", change: &change }).allowed());
    }

    /// A confirmation's labels are never drawn over their values, in any language.
    #[test]
    fn a_long_label_is_not_drawn_over_its_value() {
        for label in [tr("Needs two-factor authentication"), "Précisément une très longue étiquette de droit", "短い", "Yes"] {
            let ctx = egui::Context::default();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(520.0, 400.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| line(ui, label, "Yes"));
            out.textures_delta.clear();
            let at: Vec<Rect> = out.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Text(t) => Some(t.visual_bounding_rect()), _ => None }).collect();
            assert!(at.len() == 2 && !at[0].intersects(at[1]), "{label}: {at:?}");
        }
    }

    /// What VRChat left out is explained, with a way to read it again; what the rules refuse is said as such.
    #[test]
    fn what_is_unknown_is_explained_and_can_be_read_again() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.page.as_mut().unwrap().group.my_member.as_mut().unwrap().rights_known = false;
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
        assert!(said.contains(super::super::missing_text(Missing::Membership)) && said.contains(tr("Read again")) && said.contains(tr("Open on vrchat.com")), "{said}");
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

    /// verify4 F1: a row is a membership: its chips' presses carry it; a person who left and came back is another row, in the
    /// place of the one before; a row that does not say which membership it is, or since when, has no chips.
    #[test]
    fn a_row_is_a_membership() {
        let m = |id: &str, at: Option<&str>| Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), membership_id: id.into(), joined_at: at.map(str::to_string), display_name: Some("Sam".into()), roles_known: true, ..Default::default() };
        let mut o = open_as(&["*"], &["grol_owner"]);
        let page = |m: Member| orbiters_core::vrchat_api::Members { items: vec![m], raw: 50, total: None };
        let answer = |o: &mut Open, offset: usize, m: Member| o.answer(Kind::Members { group: "grp_1".into(), query: String::new(), offset }, Reply::Members(page(m)));
        answer(&mut o, 0, m("gmem_1", Some("2025-01-01T00:00:00Z")));
        answer(&mut o, 50, m("gmem_2", Some("2026-01-01T00:00:00Z")));
        assert_eq!(o.members.items.iter().map(|m| m.membership_id.as_str()).collect::<Vec<_>>(), ["gmem_2"]);
        assert_eq!(o.membership_of("usr_00000000-0000-4000-8000-000000000001").unwrap().id, "gmem_2");
        // What is done for one says nothing of the other.
        let sealed = Sealed::made_up(Write::MemberRole { group: "grp_1".into(), user: m("", None).user_id, membership: m("gmem_1", Some("2025-01-01T00:00:00Z")).membership().unwrap(), role: "grol_mod".into(), add: true }, true);
        o.ended(Some(&sealed), Outcome::Done(Done::MemberRoles { user: m("", None).user_id, role_ids: vec!["grol_mod".into()] }));
        assert!(o.members.items[0].role_ids.is_empty());
        let chips = |m: &Member| {
            let o = open_as(&["*"], &["grol_owner"]);
            let ctx = egui::Context::default();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| role_chooser(ui, &o, m, &o.roles().to_vec(), false, &mut Vec::new()));
            out.textures_delta.clear();
            !words(&out).is_empty()
        };
        assert!(chips(&m("gmem_1", Some("2025-01-01T00:00:00Z"))));
        for part in [m("", Some("2025-01-01T00:00:00Z")), m("gmem_1", None), m("gmem_1", Some("a while ago"))] {
            assert!(!chips(&part));
        }
    }

    /// verify4 F11: what cannot be closed is said where it is decided: another device's same post, the inventory's room, the
    /// instant between the last read and the send.
    #[test]
    fn the_limits_are_said_at_the_decision() {
        let said = |write: Write| {
            let s = Sealed::made_up(write, true);
            let ctx = egui::Context::default();
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(520.0, 900.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| confirm_body(ui, &s, None));
            out.textures_delta.clear();
            words(&out).replace('\n', " ")
        };
        let group = || "grp_1".to_string();
        let post = said(Write::CreatePost { group: group(), draft: PostDraft { title: "t".into(), text: "x".into(), ..Default::default() }, of: String::new() });
        assert!(post.contains(tr("Another moderator, or another computer, can post the same meanwhile: Orbiters only sees what goes from this one.")), "{post}");
        let up = said(Write::Upload { tag: FileTag::Gallery, group: group(), staged: crate::controller::groups::Staged { name: String::new(), size: 1, sha256: String::new() }, art: None });
        assert!(up.contains(tr("VRChat says only when it is sent whether your inventory has room for it.")), "{up}");
        let last = tr("It is all read again right before it goes. What someone else changes in the instant between is not seen: VRChat has no way to close it.");
        for s in [post, up, said(Write::ApplyArt { group: group(), slot: ArtSlot::Icon, file: "file_a".into(), back: None }), said(Write::DeleteRole { group: group(), role: "grol_a".into() })] {
            assert!(s.contains(last), "{s}");
        }
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
        let m = Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), display_name: Some("Sam".into()), membership_id: "gmem_1".into(), joined_at: Some("2026-01-01T00:00:00Z".into()), roles_known: true, ..Default::default() };
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
                *chip_out = Rect::from_min_size(ui.min_rect().min + vec2(0.0, Chooser::TOP), vec2(28.0, 30.0));
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
        let without = Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), display_name: Some("Sam".into()), membership_id: "gmem_1".into(), joined_at: Some("2026-01-01T00:00:00Z".into()), roles_known: true, ..Default::default() };
        let first = o.roles().iter().find(|r| o.can(Action::AssignRole { role: &r.id, user: &without.user_id, membership: &without.membership().unwrap(), add: true }).allowed()).unwrap().id.clone();
        let with = Member { role_ids: vec![first], ..without.clone() };
        let roles = o.roles().to_vec();
        let run = |ctx: &egui::Context, m: &Member, events: Vec<egui::Event>, t: f64, at: &mut egui::Pos2| {
            let mut acts = Vec::new();
            let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0))), time: Some(t), ..Default::default() };
            input.events = events;
            let mut out = ctx.run_ui(input, |ui| {
                role_chooser(ui, &o, m, &roles, false, &mut acts);
                *at = ui.min_rect().min + vec2(14.0, Chooser::TOP + 15.0);
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
        let perm = |name: &str, display: &str| g::PermissionInfo { name: name.into(), display_name: display.into(), ..Default::default() };
        assert_eq!(listed_label(&perm("group-\u{202E}x", "")), "group-x");
        assert_eq!(listed_label(&perm("group-x", "Bad\u{202E}name")), "Badname");
        let mut s = Sealed::made_up(Write::DeleteRole { group: "grp_1".into(), role: "grol_a".into() }, true);
        s.labels = vec![("group-x".to_string(), "Bad\u{202E}name".to_string())];
        assert_eq!(right_label(&s, "group-x"), "Badname");
        assert_eq!(right_label(&s, "group-unknown\u{202E}x"), "group-unknownx");
    }

    /// REQUEST28: the roles open in their member's own card, which grows with them; one member at a time; nothing else on the
    /// page scrolls.
    #[test]
    fn a_members_roles_open_in_their_own_card() {
        let mut o = open_as(&["*"], &["grol_owner"]);
        o.page.as_mut().unwrap().group.owner_id = "usr_me".into();
        // One whose rights VRChat left out, and one that is bought.
        let roles = o.page.as_mut().unwrap().group.roles.as_mut().unwrap();
        roles[1] = Role { name: "Staff".into(), known: false, permissions: vec![], ..roles[1].clone() };
        roles.push(Role { id: "grol_paid".into(), name: "Supporter".into(), order: Some(4), requires_purchase: true, known: true, whole: true, ..Default::default() });
        let m = Member { user_id: "usr_00000000-0000-4000-8000-000000000001".into(), membership_id: "gmem_1".into(), joined_at: Some("2026-01-01T00:00:00Z".into()), display_name: Some("Sam".into()), roles_known: true, ..Default::default() };
        let all = o.roles().to_vec();
        let gmem = m.membership().unwrap();
        let verdict = |role: &str| o.can(Action::AssignRole { role, user: &m.user_id, membership: &gmem, add: true });
        assert!(verdict("grol_mod").allowed() && verdict("grol_artist").allowed(), "the owner gives any role");
        assert_eq!(authority_text(&verdict("grol_paid")), Some(super::super::why_text(g::Why::PaidRole)), "a bought one says that");
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 900.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let c = Chooser::of(ui, &m, &all, 500.0);
            assert_eq!(c.chips.len(), 3, "the owner's role is none of them");
            assert!(c.height > Chooser::TOP + 30.0 && c.chips.iter().all(|(_, _, w, at)| at.x + w <= 500.5 && Chooser::TOP + at.y + 30.0 <= c.height));
            let narrow = Chooser::of(ui, &m, &all, 120.0);
            assert!(narrow.height > c.height, "a narrow card grows taller");
            role_chooser(ui, &o, &m, &all, false, &mut Vec::new());
        });
        out.textures_delta.clear();
        assert!(words(&out).contains("Staff") && words(&out).contains("Supporter"));
        // What it says when it cannot be given never sends the person to read again what reading cannot change.
        for m in [Missing::Roles, Missing::RoleOrder, Missing::RoleDefinition, Missing::RoleGone, Missing::Member] {
            assert!(!super::super::missing_text(m).contains("Read the group again"), "{m:?}");
        }
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
            let mut s = sealed(Write::MemberRole { group: "grp_1".into(), user: user.into(), membership: g::Membership { id: "gmem_1".into(), joined_at: "2026-01-01T00:00:00Z".into() }, role: "grol_mod".into(), add: true });
            s.member = Some(Member { user_id: user.into(), display_name: Some("Sam".into()), ..Default::default() });
            confirmed(&s)
        };
        assert!(said(a).contains("Sam") && said(a).contains("00aaaa"), "{}", said(a));
        assert_ne!(said(a), said(b));
    }

    /// verify2 B-F: every confirmation names the sealed group, by its name and id; an apply, its file; a warning is shown.
    #[test]
    fn a_confirmation_names_its_group_and_file() {
        let write = Write::ApplyArt { group: "grp_00000000-0000-4000-8000-0000000000aa".into(), slot: ArtSlot::Banner, file: "file_00000000-0000-4000-8000-00000000abcd".into(), back: None };
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
            o.composer = Some(Composer::new(PostDraft { title: "Half".into(), text: "written".into(), ..Default::default() }));
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
}
