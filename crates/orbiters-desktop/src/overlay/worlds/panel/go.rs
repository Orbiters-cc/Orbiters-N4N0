//! Go there (Enzo's VR feedback F5, L-go-direct): the instances of a world one can go to, with who may join, where, how
//! full, and new ones to make; a press takes the person there directly (the link to their own VRChat, or VRChat started
//! in it: controller/travel.rs), with an invite to accept in VRChat only where that cannot be, and the page says why.
//! Only with the member's own VRChat session in Orbiters (the button is hidden otherwise). Opening it reads the list and
//! nothing else: only the instance pressed is gone to, a new instance made only for the kind pressed.

use super::*;
use crate::controller::travel::{Detour, Went};
use orbiters_core::vrchat_api::{CreateAccess, InstanceAccess, InstanceRegion, WorldInstance, WorldInstances};
use orbiters_i18n::{tr, tr_n};

/// The instances of the world whose page shows, as the panel draws them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GoView {
    /// The world they are of (empty: none asked for yet).
    pub world: String,
    /// The list is being read.
    pub loading: bool,
    /// The last list read.
    pub list: Option<WorldInstances>,
    /// Why the list could not be read, or the last invite or new instance did not happen.
    pub error: Option<String>,
    /// A new instance or an invite on its way.
    pub busy: Option<GoBusy>,
    /// The person is on the way to this instance, and how.
    pub went: Option<GoneTo>,
    /// The region of the instance the player is in: new instances are made there unless another is picked.
    pub region: Option<InstanceRegion>,
    /// The next batch of the list is being read (VRChat's list comes a batch at a time).
    pub more_loading: bool,
}

/// Pages of the list: those read, and one more while VRChat has more to give.
pub fn go_pages(go: &GoView) -> usize {
    go.list.as_ref().map_or(1, |l| (l.items.len() + usize::from(!l.remaining.is_empty())).div_ceil(GO_ROWS).max(1))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoBusy {
    /// A new instance is made, then gone to.
    Creating,
    /// On the way to one of the list.
    Going,
    /// The invite the person asked for instead.
    Inviting,
}

/// Where Go there took the person, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct GoneTo {
    pub instance: WorldInstance,
    pub how: Went,
    /// VRChat's log says they are in it now.
    pub arrived: bool,
}

/// What the card says happened: its title and what to do now.
pub fn went_words(gone: &GoneTo) -> (&'static str, String) {
    if gone.arrived {
        return (tr("You are there"), tr("VRChat took you to this instance.").to_string());
    }
    let invite = tr("In VRChat, open the Quick Menu's notifications and accept the invite. It takes you there.");
    match gone.how {
        Went::Direct => (tr("Taking you there"), tr("Your VRChat has the instance and takes you there now. If it shows the world's page instead, press Join on it.").to_string()),
        Went::Started => (tr("Starting VRChat"), tr("VRChat is starting, right in this instance.").to_string()),
        Went::Invited(Detour::Asked) => (tr("Invite sent"), invite.to_string()),
        Went::Invited(why) => {
            let because = match why {
                Detour::OtherVrchat => tr("Another VRChat on this computer, one of Orbiters' AI bridge, receives VRChat's links: going there directly would move it, not yours."),
                Detour::LinkRefused => tr("Your VRChat did not take the link to go there directly."),
                Detour::Remote => tr("Your VRChat runs on another device: Orbiters cannot take it there directly."),
                Detour::Unidentified => tr("The VRChat on this computer is not known to be yours: its log cannot be read."),
                Detour::OtherAccount => tr("This place is private, and the VRChat on this computer is not known to be signed in with your account."),
                Detour::Asked => "",
            };
            (tr("Invite sent"), format!("{because} {invite}"))
        }
    }
}

/// The kinds of instance one can make, in the order offered.
pub const NEW_KINDS: [CreateAccess; 5] = [CreateAccess::Public, CreateAccess::FriendsPlus, CreateAccess::Friends, CreateAccess::InvitePlus, CreateAccess::Invite];
/// The regions a new instance can be made in, in the order a press goes through them.
pub const REGIONS: [InstanceRegion; 4] = [InstanceRegion::Eu, InstanceRegion::Us, InstanceRegion::Use, InstanceRegion::Jp];
/// Instances on one page of the list.
pub const GO_ROWS: usize = 5;

const ROWS_TOP: f32 = 146.0;
const ROW_H: f32 = 60.0;
const ROW_GAP: f32 = 8.0;
const LIST_RIGHT: f32 = 640.0;
const NEW_LEFT: f32 = 664.0;
const REGION: Rect = Rect { min: Pos2 { x: NEW_LEFT, y: 146.0 }, max: Pos2 { x: 992.0, y: 192.0 } };
/// Reads the list again; on the invite's card, back to the list.
const GO_RETRY: Rect = Rect { min: Pos2 { x: 816.0, y: 506.0 }, max: Pos2 { x: 992.0, y: 554.0 } };
const AGAIN: Rect = Rect { min: Pos2 { x: 200.0, y: 410.0 }, max: Pos2 { x: 472.0, y: 456.0 } };
/// On the card of a direct trip: the way back on the left, an invite instead on the right.
const AGAIN_LEFT: Rect = Rect { min: Pos2 { x: 52.0, y: 410.0 }, max: Pos2 { x: 316.0, y: 456.0 } };
const INVITE_INSTEAD: Rect = Rect { min: Pos2 { x: 328.0, y: 410.0 }, max: Pos2 { x: 620.0, y: 456.0 } };
const LIST: Rect = Rect { min: Pos2 { x: 32.0, y: ROWS_TOP }, max: Pos2 { x: LIST_RIGHT, y: 478.0 } };

fn row_rect(slot: usize) -> Rect {
    Rect::from_min_size(pos2(32.0, ROWS_TOP + slot as f32 * (ROW_H + ROW_GAP)), vec2(LIST_RIGHT - 32.0, ROW_H))
}

fn new_rect(k: usize) -> Rect {
    Rect::from_min_size(pos2(NEW_LEFT, 202.0 + k as f32 * 54.0), vec2(992.0 - NEW_LEFT, 46.0))
}

/// VRChat's name of a region (`eu`), as the API and instance ids write it.
pub fn region_of(name: &str) -> Option<InstanceRegion> {
    Some(match name {
        "us" => InstanceRegion::Us,
        "use" => InstanceRegion::Use,
        "eu" => InstanceRegion::Eu,
        "jp" => InstanceRegion::Jp,
        _ => return None,
    })
}

pub fn region_name(region: InstanceRegion) -> &'static str {
    match region {
        InstanceRegion::Us => tr("US West"),
        InstanceRegion::Use => tr("US East"),
        InstanceRegion::Eu => tr("Europe"),
        InstanceRegion::Jp => tr("Japan"),
    }
}

/// An instance's region as VRChat wrote it, in words when it is one of the four, as written otherwise.
fn region_words(region: &str) -> String {
    region_of(region).map_or_else(|| region.to_uppercase(), |r| region_name(r).to_string())
}

/// Who may join an instance, as VRChat names its kinds.
pub fn access_name(access: InstanceAccess) -> &'static str {
    match access {
        InstanceAccess::Public => tr("Public"),
        InstanceAccess::FriendsPlus => tr("Friends+"),
        InstanceAccess::Friends => tr("Friends"),
        InstanceAccess::InvitePlus => tr("Invite+"),
        InstanceAccess::Invite => tr("Invite"),
        InstanceAccess::GroupPublic => tr("Group Public"),
        InstanceAccess::GroupPlus => tr("Group+"),
        InstanceAccess::Group => tr("Group"),
        InstanceAccess::Unknown => tr("Another kind"),
    }
}

fn new_kind_name(access: CreateAccess) -> &'static str {
    match access {
        CreateAccess::Public => tr("Public"),
        CreateAccess::FriendsPlus => tr("Friends+"),
        CreateAccess::Friends => tr("Friends"),
        CreateAccess::InvitePlus => tr("Invite+"),
        CreateAccess::Invite => tr("Invite"),
    }
}

/// Who can join a new instance of that kind, in a few words.
fn new_kind_says(access: CreateAccess) -> &'static str {
    match access {
        CreateAccess::Public => tr("anyone"),
        CreateAccess::FriendsPlus => tr("your friends and theirs"),
        CreateAccess::Friends => tr("your friends"),
        CreateAccess::InvitePlus => tr("those invited, who can ask"),
        CreateAccess::Invite => tr("only those you invite"),
    }
}

/// How many are in it: of how many it takes, when VRChat says.
fn people(i: &WorldInstance) -> String {
    match i.capacity {
        Some(capacity) => tr!("{people} of {capacity} people", people = i.occupants, capacity = capacity),
        None => tr_n("{count} person", "{count} people", i.occupants as u64),
    }
}

/// An instance in one line: where, who may join, how full.
pub fn instance_line(i: &WorldInstance) -> String {
    format!("{} · {} · {}", region_words(&i.region), access_name(i.access), people(i))
}

impl Panel {
    /// The region a new instance goes to: the one picked, else the player's own.
    pub(super) fn go_region_now(&self, go: &GoView) -> InstanceRegion {
        self.go_region.or(go.region).unwrap_or(InstanceRegion::Us)
    }

    pub(super) fn go_targets(&self, d: &Data, out: &mut Vec<(Target, Rect)>) {
        let go = &d.status.go;
        if let Some(gone) = &go.went {
            // VRChat may only show the world's page: the invite is the other way, on request.
            let invite = gone.how == Went::Direct && !gone.arrived && go.busy.is_none();
            out.push((Target::GoRetry, if invite { AGAIN_LEFT } else { AGAIN }));
            if invite {
                out.push((Target::GoInvite, INVITE_INSTEAD));
            }
            return;
        }
        if let Some(list) = &go.list {
            let pages = go_pages(go);
            let first = self.go_page.min(pages - 1) * GO_ROWS;
            // One that cannot be joined is shown, greyed, and cannot be pressed.
            for i in first..(first + GO_ROWS).min(list.items.len()) {
                if list.items[i].can_join {
                    out.push((Target::GoInstance(i), row_rect(i - first)));
                }
            }
            if pages > 1 {
                out.push((Target::Previous, Rect::from_center_size(pos2(440.0, 530.0), vec2(56.0, 56.0))));
                out.push((Target::Next, Rect::from_center_size(pos2(584.0, 530.0), vec2(56.0, 56.0))));
            }
        }
        if !go.loading && go.busy.is_none() {
            out.push((Target::GoRetry, GO_RETRY));
        }
        out.push((Target::GoRegion, REGION));
        for k in 0..NEW_KINDS.len() {
            out.push((Target::NewInstance(k as u8), new_rect(k)));
        }
    }

    pub(super) fn draw_go(&self, p: &Painter, d: &Data, time: f64, targets: &[(Target, Rect)], shown: f32) {
        let go = &d.status.go;
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        self.title(p, tr("Go there"), self.entry(d).map(|e| e.name.as_str()), 990.0, shown);
        text_cut(p, pos2(32.0, 86.0), 960.0, orbit_look::txt(tr("Pick an instance, or make one: Orbiters takes you there in VRChat."), 17.0).color(fade(theme::fg_3(), shown)));
        let head = |p: &Painter, x: f32, words: &str| text(p, pos2(x, 128.0), Align2::LEFT_CENTER, orbit_look::txt(words, 15.0).weight(weight::SEMIBOLD).color(fade(theme::fg_2(), shown)));
        // On the way: how, and what to do now if anything, large, where the list was (the end of this story is in VRChat).
        if let Some(gone) = &go.went {
            let sent = &gone.instance;
            let (title, says) = went_words(gone);
            let card = Rect::from_min_max(pos2(32.0, 124.0), pos2(LIST_RIGHT, 474.0));
            p.rect_filled(card, CornerRadius::same(24), fade(theme::alpha(orbit_look::accent(), 0.12), shown));
            p.rect_stroke(card, CornerRadius::same(24), Stroke::new(1.5, fade(orbit_look::accent(), shown)), StrokeKind::Inside);
            let glyph = match gone.how {
                _ if gone.arrived => icons::CHECK_CIRCLE,
                Went::Invited(_) => icons::ENVELOPE_SIMPLE,
                Went::Direct | Went::Started => icons::SIGN_IN,
            };
            widgets::icon(p, pos2(card.center().x, 176.0), glyph, 48.0, fade(orbit_look::accent(), shown));
            text(p, pos2(card.center().x, 228.0), Align2::CENTER_CENTER, orbit_look::txt(title, 28.0).weight(weight::BOLD).color(fade(theme::fg(), shown)));
            text(p, pos2(card.center().x, 272.0), Align2::CENTER_CENTER, orbit_look::txt(instance_line(sent), 18.0).color(fade(theme::fg_2(), shown)));
            let mut job = orbit_look::txt(says, 19.0).color(fade(theme::fg(), shown)).line_height(26.0).job(card.width() - 56.0);
            job.wrap.max_rows = 4;
            job.halign = egui::Align::Center;
            let galley = p.layout_job(job);
            p.galley(pos2(card.center().x, 300.0), galley, theme::fg());
            for (t, rect) in targets {
                if *t == Target::GoRetry {
                    let (h, pr) = state(*t);
                    pill_button(p, *rect, Some(icons::ARROW_U_UP_LEFT), tr("See the instances again"), RAISED2, theme::fg(), h, pr, shown);
                }
                if *t == Target::GoInvite {
                    let (h, pr) = state(*t);
                    pill_button(p, *rect, Some(icons::ENVELOPE_SIMPLE), tr("Send me an invite instead"), RAISED, theme::fg(), h, pr, shown);
                }
            }
        } else {
            head(p, 32.0, tr("Instances now"));
            match (&go.list, go.loading) {
                (None, true) => {
                    widgets::icon(p, pos2(LIST.center().x, LIST.center().y - 30.0), icons::GLOBE_HEMISPHERE_WEST, 40.0, fade(theme::fg_4(), shown));
                    text(p, pos2(LIST.center().x, LIST.center().y + 18.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Looking for instances…"), 20.0).color(fade(theme::fg_3(), shown)));
                }
                (None, false) => {
                    widgets::icon(p, pos2(LIST.center().x, LIST.center().y - 40.0), icons::WARNING, 40.0, fade(theme::WARN, shown));
                    let words = go.error.clone().unwrap_or_else(|| tr("The instances could not be read.").to_string());
                    let mut job = orbit_look::txt(words, 18.0).color(fade(theme::fg(), shown)).line_height(24.0).job(LIST.width() - 40.0);
                    job.wrap.max_rows = 3;
                    job.halign = egui::Align::Center;
                    p.galley(pos2(LIST.center().x, LIST.center().y), p.layout_job(job), theme::fg());
                }
                (Some(list), _) if list.items.is_empty() => {
                    widgets::icon(p, pos2(LIST.center().x, LIST.center().y - 40.0), icons::USERS_THREE, 40.0, fade(theme::fg_4(), shown));
                    text(p, pos2(LIST.center().x, LIST.center().y + 6.0), Align2::CENTER_CENTER, orbit_look::txt(tr("No instance you can join right now"), 21.0).weight(weight::SEMIBOLD).color(fade(theme::fg(), shown)));
                    text(p, pos2(LIST.center().x, LIST.center().y + 38.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Make a new one on the right."), 17.0).color(fade(theme::fg_3(), shown)));
                }
                (Some(list), _) => {
                    let pages = go_pages(go);
                    let first = self.go_page.min(pages - 1) * GO_ROWS;
                    // A page past what was read: the next batch is on its way.
                    if first >= list.items.len() {
                        text(p, pos2(LIST.center().x, LIST.center().y), Align2::CENTER_CENTER, orbit_look::txt(tr("Looking for more instances…"), 19.0).color(fade(theme::fg_3(), shown)));
                    }
                    for (slot, i) in (first..(first + GO_ROWS).min(list.items.len())).enumerate() {
                        let item = &list.items[i];
                        let (h, pr) = state(Target::GoInstance(i));
                        let appear = ease_out((time - self.page_at - slot as f64 * CARD_STAGGER) / PAGE_FADE) * shown;
                        let alpha = if item.can_join { appear } else { appear * 0.55 };
                        let rect = row_rect(slot);
                        let rect = if pr { rect.shrink(2.0) } else { rect };
                        p.rect_filled(rect, CornerRadius::same(18), fade(if h { RAISED2 } else { RAISED }, appear));
                        // Who may join, then where and how full; friends in it, when VRChat said.
                        text_cut(p, pos2(rect.min.x + 20.0, rect.min.y + 8.0), 360.0, orbit_look::txt(access_name(item.access), 19.0).weight(weight::SEMIBOLD).color(fade(theme::fg(), alpha)));
                        let mut under = format!("{} · {}", region_words(&item.region), people(item));
                        if let Some(friends) = item.friends.filter(|n| *n > 0) {
                            under = format!("{under} · {}", tr_n("{count} friend", "{count} friends", friends as u64));
                        }
                        text_cut(p, pos2(rect.min.x + 20.0, rect.min.y + 34.0), 400.0, orbit_look::txt(under, 15.5).color(fade(theme::fg_3(), alpha)));
                        let (word, tint) = if item.can_join {
                            (tr("Join"), orbit_look::accent())
                        } else if item.full {
                            (tr("Full"), theme::fg_3())
                        } else if item.closed {
                            (tr("Closed"), theme::fg_3())
                        } else {
                            (tr("Cannot join"), theme::fg_3())
                        };
                        text(p, pos2(rect.max.x - 20.0, rect.center().y), Align2::RIGHT_CENTER, orbit_look::txt(word, 17.0).weight(weight::SEMIBOLD).color(fade(tint, alpha)));
                    }
                    if list.unavailable > 0 && pages == 1 && list.remaining.is_empty() {
                        let line = tr_n("{count} more is hidden from you", "{count} more are hidden from you", list.unavailable as u64);
                        let y = row_rect((list.items.len() - first).min(GO_ROWS)).min.y + 14.0;
                        text(p, pos2(48.0, y), Align2::LEFT_CENTER, orbit_look::txt(line, 15.0).color(fade(theme::fg_4(), shown)));
                    }
                    if pages > 1 {
                        text(p, pos2(512.0, 530.0), Align2::CENTER_CENTER, orbit_look::txt(format!("{}/{}", self.go_page.min(pages - 1) + 1, pages), 17.0).color(fade(theme::fg_3(), shown)));
                    }
                }
            }
        }
        // Making one: where, then which kind. A press makes it and goes there. Not beside the card of a trip.
        if go.went.is_none() {
            head(p, NEW_LEFT, tr("Make a new one"));
        }
        let busy = go.busy.is_some();
        let region = self.go_region_now(go);
        for (t, rect) in targets {
            let (h, pr) = state(*t);
            match *t {
                Target::GoRegion => {
                    let rect = if pr { rect.shrink(2.0) } else { *rect };
                    p.rect_stroke(rect, CornerRadius::same(23), Stroke::new(1.5, fade(if h { theme::fg_2() } else { theme::ink_600() }, shown)), StrokeKind::Inside);
                    widgets::icon(p, pos2(rect.min.x + 24.0, rect.center().y), icons::GLOBE_HEMISPHERE_WEST, 20.0, fade(theme::fg_2(), shown));
                    text_cut(p, pos2(rect.min.x + 44.0, rect.center().y - 10.0), rect.width() - 80.0, orbit_look::txt(tr!("In {region}", region = region_name(region)), 17.0).weight(weight::SEMIBOLD).color(fade(theme::fg(), shown)));
                    widgets::icon(p, pos2(rect.max.x - 22.0, rect.center().y), icons::CARET_RIGHT, 18.0, fade(theme::fg_3(), shown));
                }
                Target::NewInstance(k) => {
                    let kind = NEW_KINDS[k as usize];
                    let rect = if pr { rect.shrink(2.0) } else { *rect };
                    let alpha = if busy { shown * 0.5 } else { shown };
                    p.rect_filled(rect, CornerRadius::same(23), fade(if h && !busy { RAISED2 } else { RAISED }, alpha));
                    widgets::icon(p, pos2(rect.min.x + 24.0, rect.center().y), icons::PLUS, 18.0, fade(orbit_look::accent(), alpha));
                    let name = orbit_look::txt(new_kind_name(kind), 17.0).weight(weight::SEMIBOLD).color(fade(theme::fg(), alpha));
                    let galley = p.layout_job(name.job_truncated(120.0));
                    let wide = galley.size().x;
                    p.galley(pos2(rect.min.x + 44.0, rect.center().y - galley.size().y / 2.0), galley, theme::fg());
                    text_cut(p, pos2(rect.min.x + 52.0 + wide, rect.center().y - 9.0), rect.max.x - 16.0 - (rect.min.x + 52.0 + wide), orbit_look::txt(new_kind_says(kind), 15.0).color(fade(theme::fg_3(), alpha)));
                }
                Target::Previous => round_button(p, rect.center(), icons::CARET_LEFT, h, pr, shown),
                Target::Next => round_button(p, rect.center(), icons::CARET_RIGHT, h, pr, shown),
                Target::GoRetry if go.went.is_none() => {
                    pill_button(p, *rect, Some(icons::ARROW_CLOCKWISE), if go.list.is_some() { tr("Read again") } else { tr("Try again") }, RAISED2, theme::fg(), h, pr, shown)
                }
                _ => {}
            }
        }
        // What is going on now, or what did not happen, at the foot.
        let (line, tint) = match (go.busy, &go.error) {
            (Some(GoBusy::Creating), _) => (tr("Making the instance, then taking you there…").to_string(), theme::fg_2()),
            (Some(GoBusy::Going), _) => (tr("Taking you there…").to_string(), theme::fg_2()),
            (Some(GoBusy::Inviting), _) => (tr("Sending you an invite…").to_string(), theme::fg_2()),
            (None, Some(error)) if go.list.is_some() || go.went.is_some() => (error.clone(), theme::WARN),
            (None, _) if go.loading && go.list.is_some() => (tr("Reading the instances again…").to_string(), theme::fg_3()),
            _ => (String::new(), theme::fg_3()),
        };
        if !line.is_empty() {
            let room = if go_pages(go) > 1 { 380.0 } else { 760.0 };
            text_cut(p, pos2(32.0, 520.0), room, orbit_look::txt(line, 16.0).color(fade(tint, shown)));
        }
    }
}
