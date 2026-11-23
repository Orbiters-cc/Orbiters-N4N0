//! VRChat groups (REQUEST22 1) over made-up groups, members and art only (painted here): Home's Groups section in each of
//! its states, the group page's four tabs (an owner's and a moderator's), the banner studio for each target, each
//! confirmation and each write's outcome, in English, French and Japanese. Nothing is asked of the service.
use super::{Page, Scenario};
use crate::app::App;
use crate::controller::groups::{Done, Outcome, Page as GroupPage, Prepared, Write};
use crate::controller::vrchat_data::Scope;
use crate::controller::vrchat_login::Status;
use crate::pages::groups::{art::{Crop, Target}, stage};
use crate::state::{AccountState, Snapshot};
use egui::{pos2, vec2, Color32, ColorImage, Rect};
use orbiters_core::groups::*;
use orbiters_core::vrchat_api::User;

pub(super) const NAMES: [&str; 39] = [
    "groups-home-several",
    "groups-home-one",
    "groups-home-none",
    "groups-home-failed",
    "groups-home-connecting",
    "groups-look",
    "groups-look-moderator",
    "groups-look-uploaded",
    "groups-members",
    "groups-members-roles",
    "groups-members-search",
    "groups-members-moderator",
    "groups-roles",
    "groups-roles-edit",
    "groups-roles-moderator",
    "groups-posts",
    "groups-posts-compose",
    "groups-posts-moderator",
    "groups-studio-empty",
    "groups-studio-banner",
    "groups-studio-anchored",
    "groups-studio-centered",
    "groups-studio-exported",
    "groups-confirm-give-role",
    "groups-confirm-take-role",
    "groups-confirm-new-role",
    "groups-confirm-edit-role",
    "groups-confirm-delete-role",
    "groups-confirm-post",
    "groups-confirm-edit-post",
    "groups-confirm-delete-post",
    "groups-confirm-upload",
    "groups-confirm-apply",
    "groups-outcome-done",
    "groups-outcome-refused",
    "groups-outcome-unknown",
    "groups-home-several-fr",
    "groups-studio-anchored-ja",
    "groups-confirm-post-ja",
];
/// Also in French or Japanese (the name and a language).
const TRANSLATED: [(&str, &str, &str); 6] = [
    ("groups-look-fr", "groups-look", "fr"),
    ("groups-outcome-unknown-fr", "groups-outcome-unknown", "fr"),
    ("groups-roles-edit-fr", "groups-roles-edit", "fr"),
    ("groups-members-roles-ja", "groups-members-roles", "ja"),
    ("groups-posts-compose-ja", "groups-posts-compose", "ja"),
    ("groups-studio-centered-fr", "groups-studio-centered", "fr"),
];

const ME: &str = "usr_00000000-0000-4000-8000-00000000e001";
const G1: &str = "grp_00000000-0000-4000-8000-00000000a001";

fn base_name(name: &str) -> &str {
    TRANSLATED.iter().find(|t| t.0 == name).map(|t| t.1).unwrap_or(name.trim_end_matches("-fr").trim_end_matches("-ja"))
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    for name in NAMES.into_iter().chain(TRANSLATED.iter().map(|t| t.0)) {
        let mut snapshot = signed_in(&base);
        if name == "groups-home-connecting" {
            snapshot.vrchat_login.known = snapshot.vrchat_login.user.take();
            snapshot.vrchat_login.restoring = true;
        }
        let language = TRANSLATED.iter().find(|t| t.0 == name).map(|t| t.2).or_else(|| name.strip_suffix("-fr").map(|_| "fr")).or_else(|| name.strip_suffix("-ja").map(|_| "ja"));
        snapshot.settings.language = language.map(str::to_string);
        let home = base_name(name).starts_with("groups-home");
        scenarios.push(Scenario {
            name,
            page: Page::Home,
            snapshot,
            frames: 110,
            splash: false,
            open_detail: false,
            toasts: if base_name(name) == "groups-outcome-done" { vec![(orbiters_ui::toast::Kind::Success, "The member's roles are updated.")] } else { vec![] },
            pointer: None,
            scroll: if home { 6000.0 } else if base_name(name) == "groups-roles-edit" { 430.0 } else { 0.0 },
            world: None,
            home: None,
            library: None,
        });
    }
}

fn signed_in(base: &Snapshot) -> Snapshot {
    let mut s = base.clone();
    s.account.state = AccountState::SignedIn { username: "Desktop Tester".into() };
    s.vrchat_login = Status { allowed: true, user: Some(User { id: ME.into(), display_name: "Desktop Tester".into(), ..Default::default() }), ..Default::default() };
    s
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Made-up art: a soft landscape, a sun, a few shapes; `seed` changes its colours.
fn landscape(w: u32, h: u32, seed: u32) -> image::RgbaImage {
    let palettes = [([38, 24, 92], [255, 140, 110], [255, 214, 120]), ([10, 40, 70], [60, 190, 200], [210, 250, 220]), ([60, 20, 40], [230, 90, 140], [255, 200, 220]), ([20, 30, 30], [120, 200, 90], [240, 240, 160])];
    let (top, mid, sun) = palettes[(seed % 4) as usize];
    image::RgbaImage::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let mix = |a: [u8; 3], b: [u8; 3], t: f32| [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t.clamp(0.0, 1.0)) as u8);
        let mut c = mix(top, mid, v * 1.3);
        let (sx, sy) = (0.68 + 0.1 * (seed % 3) as f32, 0.38);
        let d = ((u - sx) * w as f32 / h as f32).hypot(v - sy);
        if d < 0.16 {
            c = mix(c, sun, 1.0 - d / 0.16 * 0.4);
        }
        let ridge = 0.62 + 0.08 * (u * 9.0 + seed as f32).sin() + 0.05 * (u * 23.0).sin();
        if v > ridge {
            c = mix(c, [12, 14, 28], 0.75 + (v - ridge));
        }
        let ridge2 = 0.78 + 0.05 * (u * 5.0 + 1.0 + seed as f32).cos();
        if v > ridge2 {
            c = mix(c, [5, 6, 14], 0.9);
        }
        // A round emblem on the left, as a logo would be.
        let e = ((u - 0.2) * w as f32 / h as f32).hypot(v - 0.42);
        if e < 0.14 && e > 0.1 {
            c = [250, 250, 255];
        }
        image::Rgba([c[0], c[1], c[2], 255])
    })
}

fn color_image(img: &image::RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

fn icon(seed: u32) -> image::RgbaImage {
    let colours = [[0, 218, 109], [116, 102, 255], [255, 107, 122], [72, 230, 255], [245, 183, 79]];
    let c = colours[(seed % 5) as usize];
    image::RgbaImage::from_fn(128, 128, |x, y| {
        let (u, v) = (x as f32 / 128.0 - 0.5, y as f32 / 128.0 - 0.5);
        let r = u.hypot(v);
        let ring = (r - 0.26).abs() < 0.06;
        let k = if ring { 1.0 } else { 0.35 + 0.4 * (1.0 - r) };
        image::Rgba([(c[0] as f32 * k) as u8, (c[1] as f32 * k) as u8, (c[2] as f32 * k) as u8, 255])
    })
}

fn card(i: u32, name: &str, code: &str, owner: bool, rights: &[&str], members: u32) -> GroupCard {
    GroupCard {
        id: format!("grp_00000000-0000-4000-8000-00000000a{i:03}"),
        name: name.into(),
        short_code: code.into(),
        discriminator: format!("{:04}", 11 * i),
        icon_url: format!("https://api.vrchat.cloud/api/1/file/file_icon{i}/1/file"),
        banner_url: format!("https://api.vrchat.cloud/api/1/file/file_banner{i}/1/file"),
        member_count: Some(members),
        named: true,
        online_count: Some(members / 9),
        owner,
        permissions: s(rights),
    }
}

fn cards() -> Vec<GroupCard> {
    vec![
        card(1, "Made-up Makers", "MAKE", true, &["*"], 1240),
        card(2, "Night Painters Club", "PAINT", false, &["group-roles-manage", "group-roles-assign", "group-members-manage", "group-announcement-manage"], 312),
        card(3, "Quiet Tea Lounge", "TEA", false, &["group-instance-moderate", "group-bans-manage"], 87),
        card(4, "Synth Wave Riders", "SYNTH", false, &["group-announcement-manage", "group-data-manage"], 2093),
        card(5, "Pixel Garden", "PIXEL", true, &["*"], 41),
    ]
}

fn roles() -> Vec<Role> {
    let r = |id: &str, name: &str, order: i64, perms: &[&str]| Role { id: id.into(), name: name.into(), order: Some(order), permissions: s(perms), known: true, ..Default::default() };
    vec![
        Role { description: "Founded the group.".into(), ..r("grol_owner", "Owner", 0, &["*"]) },
        Role { management: true, description: "Keeps the group running: roles, posts and members.".into(), ..r("grol_admin", "Admins", 1, &["group-roles-manage", "group-roles-assign", "group-members-manage", "group-announcement-manage"]) },
        Role { management: true, description: "Looks after our instances.".into(), ..r("grol_mod", "Moderators", 2, &["group-instance-moderate", "group-bans-manage"]) },
        Role { description: "Members who made the art of the month.".into(), ..r("grol_artist", "Artists", 3, &[]) },
        Role { self_assignable: true, ..r("grol_events", "Event goers", 4, &[]) },
        Role { requires_two_factor: true, ..r("grol_staff", "Event staff", 5, &["group-instance-join"]) },
        Role { default_role: true, ..r("grol_everyone", "Everyone", 9, &["group-instance-join"]) },
    ]
}

fn catalogue() -> Vec<PermissionInfo> {
    let p = |name: &str, display: &str, help: &str, add: bool, deps: &[&str]| PermissionInfo { name: name.into(), display_name: display.into(), help: help.into(), allowed_to_add: add, depends_on: s(deps), management: true };
    vec![
        p("group-announcement-manage", "Manage Group Announcement", "Lets the role write the group's posts.", true, &[]),
        p("group-members-manage", "Manage Group Member Data", "Lets the role manage members' data.", true, &[]),
        p("group-roles-assign", "Assign Group Roles", "Lets the role give and take back roles.", true, &["group-members-manage"]),
        p("group-roles-manage", "Manage Group Roles", "Lets the role create and change roles.", false, &[]),
        p("group-instance-moderate", "Moderate Group Instances", "Lets the role moderate the group's instances.", true, &[]),
        p("group-instance-join", "Join Group Instances", "Lets the role join the group's instances.", true, &[]),
    ]
}

fn page(owner: bool) -> GroupPage {
    let c = &cards()[0];
    let (rights, mine) = if owner { (s(&["*"]), s(&["grol_owner"])) } else { (s(&["group-instance-moderate", "group-bans-manage"]), s(&["grol_mod"])) };
    GroupPage {
        group: Group {
            id: G1.into(),
            name: c.name.clone(),
            short_code: c.short_code.clone(),
            discriminator: c.discriminator.clone(),
            description: "We make worlds, avatars and props together, and show them every Saturday.".into(),
            rules: "Be kind. Credit artists. No reposting without asking.".into(),
            links: s(&["https://example.org/made-up-makers"]),
            icon_url: c.icon_url.clone(),
            banner_url: c.banner_url.clone(),
            member_count: c.member_count.unwrap_or(0),
            online_count: c.online_count,
            owner_id: if owner { ME.into() } else { "usr_00000000-0000-4000-8000-00000000e0ff".into() },
            privacy: "default".into(),
            my_member: Some(MyMember { user_id: ME.into(), role_ids: mine, permissions: rights, status: "member".into() }),
            roles: Some(roles()),
            ..Default::default()
        },
        catalogue: owner.then(catalogue),
    }
}

fn members() -> Vec<Member> {
    let names = [("Nova Lumen", &["grol_admin"][..]), ("Kite", &["grol_mod", "grol_artist"]), ("Sam Juniper", &[]), ("Sam Juniper", &["grol_events"]), ("Orla Finch", &["grol_artist"]), ("Pip", &[]), ("Teodor Vance", &["grol_events", "grol_staff"])];
    names
        .iter()
        .enumerate()
        .map(|(i, (n, r))| Member {
            user_id: format!("usr_00000000-0000-4000-8000-00000000b{i:03}"),
            membership_id: format!("gmem_{i}"),
            display_name: Some(n.to_string()),
            icon_url: format!("https://api.vrchat.cloud/api/1/file/file_member{i}/1/file"),
            role_ids: s(r),
            joined_at: None,
            roles_known: true,
            group_id: None,
        })
        .collect()
}

fn posts() -> Vec<Post> {
    vec![
        Post { id: "gpos_1".into(), title: "Saturday showcase".into(), text: "Bring what you made this week: worlds, avatars, props. We start at 20:00 in the gallery world.".into(), visibility: Visibility::Group, created_at: Some("2026-11-21T19:00:00Z".into()), complete: true, ..Default::default() },
        Post { id: "gpos_2".into(), title: "New role for artists".into(), text: "Artists of the month now get their own role and colour.".into(), visibility: Visibility::Public, created_at: Some("2026-11-18T10:30:00Z".into()), complete: true, ..Default::default() },
        Post { id: "gpos_3".into(), title: "Staff briefing".into(), text: "Event staff: meet ten minutes early.".into(), visibility: Visibility::Group, role_ids: s(&["grol_staff"]), created_at: Some("2026-11-15T09:00:00Z".into()), complete: true, ..Default::default() },
    ]
}

fn prepared(write: Write, role: Option<Role>) -> Prepared {
    Prepared { op: "op".into(), write, confirm: true, role, post: None, member: Some(Member { user_id: members()[0].user_id.clone(), membership_id: "gmem_00000000-0000-4000-8000-00000000c001".into(), ..Default::default() }) }
}

pub(super) fn stage(ctx: &egui::Context, app: &mut App, name: &str, snapshot: &Snapshot, f: u32) {
    if !name.starts_with("groups-") || f != 0 {
        return;
    }
    let name = base_name(name);
    let state = app.home_mut().groups_mut();
    if name == "groups-home-connecting" {
        return;
    }
    stage::scope(state, Scope::of(snapshot));
    for (i, c) in cards().iter().enumerate() {
        stage::picture(ctx, state, &c.banner_url, color_image(&landscape(480, 270, i as u32)));
        stage::picture(ctx, state, &c.icon_url, color_image(&icon(i as u32)));
    }
    for (i, m) in members().iter().enumerate() {
        stage::picture(ctx, state, &m.icon_url, color_image(&icon(i as u32 + 2)));
    }
    let owner = !name.ends_with("-moderator");
    match name {
        "groups-home-several" => stage::groups(state, cards()),
        "groups-home-one" => stage::groups(state, vec![cards()[0].clone()]),
        "groups-home-none" => stage::groups(state, vec![]),
        "groups-home-failed" => stage::failed(state),
        n if n.starts_with("groups-studio") => {
            let studio = stage::studio(state, cards()[0].clone(), ME, landscape(2400, 1350, 0));
            studio.target = match n {
                "groups-studio-anchored" => Target::Anchored,
                "groups-studio-centered" => Target::Centered,
                _ => Target::GroupBanner,
            };
            studio.set_crop(Target::GroupBanner, Crop { zoom: 1.15, cx: 0.45, cy: 0.5, turns: 0 });
            studio.set_crop(Target::Anchored, Crop { zoom: 1.3, cx: 0.4, cy: 0.42, turns: 0 });
            studio.set_crop(Target::Centered, Crop { zoom: 1.2, cx: 0.5, cy: 0.4, turns: 0 });
            if n == "groups-studio-exported" {
                studio.exported = vec![(Target::GroupBanner, "Made-up Makers - group-banner.png".into(), 2_811_204), (Target::Anchored, "Made-up Makers - nameplate-anchored-2048x385.png".into(), 902_117), (Target::Centered, "Made-up Makers - nameplate-centered-2048x472.png".into(), 1_061_490)];
                let rev = studio.revision();
                studio.uploaded = Some((InventoryFile { id: "file_up".into(), url: String::new() }, Some(rev)));
            }
            if n == "groups-studio-empty" {
                stage::studio_empty(state, cards()[0].clone());
            }
        }
        n => {
            let open = stage::open(state, cards()[0].clone(), page(owner));
            open.me_name = "Desktop Tester".into();
            open.members.items = members();
            open.posts.items = posts();
            use crate::pages::groups::Tab as tab;
            open.tab = match n {
                n if n.starts_with("groups-members") => tab::Members,
                n if n.starts_with("groups-roles") => tab::Roles,
                n if n.starts_with("groups-posts") => tab::Posts,
                n if n.starts_with("groups-confirm") && (n.contains("role") && !n.contains("give") && !n.contains("take")) => tab::Roles,
                n if n.starts_with("groups-confirm") && n.contains("post") => tab::Posts,
                n if n.starts_with("groups-confirm") || n.starts_with("groups-outcome") => tab::Members,
                _ => tab::Look,
            };
            match n {
                "groups-members-roles" => open.member_roles = Some(members()[1].user_id.clone()),
                "groups-members-search" => {
                    open.members.query = "Sa".into();
                }
                "groups-roles-edit" => {
                    let r = roles()[3].clone();
                    let mut draft = RoleDraft::of(&r);
                    draft.permissions = s(&["group-instance-join"]);
                    open.role_editor = Some(crate::pages::groups::RoleEditor { baseline: Some(r), draft });
                }
                "groups-posts-compose" => {
                    open.composer = Some(crate::pages::groups::Composer { baseline: None, draft: PostDraft { title: "Saturday showcase".into(), text: "Bring what you made this week.\nWe start at 20:00.".into(), role_ids: s(&["grol_artist"]), ..Default::default() } });
                }
                "groups-look-uploaded" => {}
                "groups-outcome-refused" => {}
                "groups-outcome-unknown" => {}
                "groups-outcome-done" => open.ended(None, Outcome::Done(Done::MemberRoles { user: members()[2].user_id.clone(), role_ids: s(&["grol_artist"]) })),
                _ => {}
            }
            let origin = Rect::from_min_size(pos2(700.0, 500.0), vec2(160.0, 36.0));
            let r = roles();
            let confirm = match n {
                "groups-confirm-give-role" => Some(prepared(Write::MemberRole { group: G1.into(), user: members()[0].user_id.clone(), role: "grol_mod".into(), add: true }, Some(r[2].clone()))),
                "groups-confirm-take-role" => Some(prepared(Write::MemberRole { group: G1.into(), user: members()[1].user_id.clone(), role: "grol_artist".into(), add: false }, Some(r[3].clone()))),
                "groups-confirm-new-role" => Some(prepared(Write::CreateRole { group: G1.into(), draft: RoleDraft { name: "Greeters".into(), description: "Welcome new members.".into(), permissions: s(&["group-instance-join"]), self_assignable: true, ..Default::default() } }, None)),
                "groups-confirm-edit-role" => {
                    let mut edit = RoleDraft::of(&r[3]);
                    edit.name = "Featured artists".into();
                    edit.permissions = s(&["group-instance-moderate"]);
                    Some(prepared(Write::EditRole { group: G1.into(), baseline: r[3].clone(), edit }, Some(r[3].clone())))
                }
                "groups-confirm-delete-role" => Some(prepared(Write::DeleteRole { group: G1.into(), role: "grol_events".into() }, Some(r[4].clone()))),
                "groups-confirm-post" => Some(prepared(Write::CreatePost { group: G1.into(), draft: PostDraft { title: "Saturday showcase".into(), text: "Bring what you made this week. We start at 20:00 in the gallery world.".into(), role_ids: s(&["grol_artist", "grol_events"]), send_notification: true, ..Default::default() } }, None)),
                "groups-confirm-edit-post" => {
                    let p = posts()[1].clone();
                    let edit = PostDraft { title: p.title.clone(), text: "Artists of the month now get their own role, colour and a spot on the banner.".into(), visibility: p.visibility, ..Default::default() };
                    Some(prepared(Write::EditPost { group: G1.into(), baseline: p, edit }, None))
                }
                "groups-confirm-delete-post" => Some(prepared(Write::DeletePost { group: G1.into(), post: posts()[2].clone() }, None)),
                "groups-confirm-upload" => Some(prepared(Write::Upload { tag: FileTag::Gallery, group: G1.into(), staged: crate::controller::groups::Staged { name: "00000000-0000-4000-8000-000000000000.png".into(), size: 2_811_204, sha256: String::new() } }, None)),
                "groups-confirm-apply" => Some(prepared(Write::ApplyArt { group: G1.into(), slot: ArtSlot::Banner, file: "file_up".into() }, None)),
                _ => None,
            };
            // The page-level states that are the window's, not the page's (uncertain writes, refusals, uploads kept).
            match n {
                "groups-outcome-unknown" => stage::uncertain(state, "op-unknown", Write::CreatePost { group: G1.into(), draft: PostDraft { title: "Saturday showcase".into(), text: "x".into(), ..Default::default() } }, None),
                "groups-outcome-refused" => state.said = Some(crate::pages::groups::failure_text(crate::controller::groups::Failure::Api(orbiters_core::vrchat_api::GroupFailure::Forbidden))),
                "groups-confirm-upload" => stage::confirm_art(state, &cards()[1].banner_url),
                "groups-look-uploaded" | "groups-confirm-apply" => state.drafts.set_uploaded(crate::pages::groups::art::UploadedRef { group: G1.into(), slot: ArtSlot::Banner, file: InventoryFile { id: "file_up".into(), url: cards()[1].banner_url.clone() }, revision: None }),
                _ => {}
            }
            if let Some(p) = confirm {
                stage::confirming(state, p, origin);
            }
        }
    }
    let _ = Color32::WHITE;
}
