//! The launcher on Home (`pages/launcher`) over made-up people only (`orbiters_core::launcher::sample` and more of the
//! same, pictures painted here): the list, the database-only list, a row hovered with a boop and a request just sent,
//! a profile open, an invite, and (run with `--width 900`) the rail and its drawer.
use super::{Page, Scenario};
use crate::app::App;
use egui::{pos2, vec2, Color32, ColorImage, Rect, TextureOptions};
use orbiters_core::launcher::*;

const NOW: i64 = 1_795_000_000;

pub(super) const NAMES: [&str; 9] =
    ["launcher-list", "launcher-invite", "launcher-hover", "launcher-modal", "launcher-database", "launcher-starting", "launcher-small", "launcher-small-drawer", "launcher-small-modal"];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    // Settings' Play VRChat card (its launch options), scrolled to.
    for (name, scroll) in [("launcher-settings", 2600.0), ("launcher-settings-2", 3400.0)] {
        scenarios.push(Scenario {
            name,
            page: Page::Settings,
            snapshot: base.clone(),
            frames: 100,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll,
            world: None,
            home: None,
            library: None,
        });
    }
    for name in NAMES {
        scenarios.push(Scenario {
            name,
            page: Page::Home,
            snapshot: base.clone(),
            frames: 100,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: (name == "launcher-hover").then_some(pos2(1150.0, 377.0)),
            scroll: 0.0,
            world: None,
            home: None,
            library: None,
        });
    }
}

/// Made-up people beyond the sample's seven, so the list looks lived in.
fn more_people(state: &mut LauncherState) {
    const FIRST: [&str; 12] = ["Velvet", "Byte", "Sunny", "Neon", "Cosmo", "Pebble", "Static", "Juniper", "Echo", "Tofu", "Glimmer", "Marble"];
    const LAST: [&str; 9] = ["Comet", "Wolf", "Static", "Otter", "Lynx", "Moth", "Kitsune", "Drake", "Finch"];
    const WORLDS: [(&str, &str); 5] = [("Sample Lounge", "eu"), ("Midnight Rooftop", "us"), ("Quiet Café", "jp"), ("Lantern Garden", "use"), ("Movie Night Hall", "eu")];
    const TEXTS: [&str; 8] = ["working on avatars", "", "afk, back soon", "dm for commissions", "chilling", "", "learning Japanese", "new world tonight!"];
    let accesses = [InstanceAccess::Public, InstanceAccess::Friends, InstanceAccess::FriendsPlus, InstanceAccess::InvitePlus, InstanceAccess::GroupPublic];
    let statuses = [Status::Active, Status::JoinMe, Status::AskMe, Status::Busy, Status::Active];
    let ranks = [Rank::TrustedUser, Rank::KnownUser, Rank::User, Rank::TrustedUser, Rank::NewUser];
    for n in 0..34u32 {
        let name = format!("{} {}", FIRST[n as usize % FIRST.len()], LAST[(n as usize * 5 + 2) % LAST.len()]);
        let (presence, whereabouts, status) = match n % 5 {
            0 | 1 => {
                let (world, region) = WORLDS[n as usize % WORLDS.len()];
                let access = accesses[n as usize % accesses.len()];
                let place = Place {
                    world_id: format!("wrld_00000000-0000-4000-8000-{:012}", 100 + n % 5),
                    world_name: world.into(),
                    world_picture: format!("sample://world/{}", n % 5),
                    instance_id: format!("{}~region({region})", 1000 + n),
                    access,
                    region: region.into(),
                };
                (Presence::InGame, Whereabouts::World(place), statuses[n as usize % statuses.len()])
            }
            2 => (Presence::InGame, Whereabouts::Private, Status::AskMe),
            3 if n < 20 => (Presence::Website, Whereabouts::Website, Status::Active),
            _ => (Presence::Offline, Whereabouts::Offline, Status::Offline),
        };
        let id = format!("usr_00000000-0000-4000-8000-{:012}", 100 + n);
        let favourite = match n {
            0 | 5 | 11 => vec!["group_0".to_string()],
            3 | 9 => vec!["group_1".to_string()],
            _ => vec![],
        };
        state.friends.push(Friend {
            id: id.clone(),
            name,
            look: Look {
                // Nameplates on some, banners on others, a banner colour on the rest.
                banner: if n % 3 == 0 { format!("sample://banner/{}", n % 7) } else { String::new() },
                nameplate: if n % 3 == 1 { format!("sample://banner/{}", (n + 3) % 7) } else { String::new() },
                banner_colour: (n % 3 == 2).then_some([(40 + n * 23 % 160) as u8, 60, (180 - n * 17 % 120) as u8]),
                icon_frame: if n % 7 == 3 { format!("sample://frame/{}", n % 3) } else { String::new() },
                icon: format!("sample://icon/{n}"),
                rank: ranks[n as usize % ranks.len()],
                supporter: n % 4 == 0,
            },
            status,
            status_text: TEXTS[n as usize % TEXTS.len()].into(),
            presence,
            whereabouts,
            platform: if n % 4 == 1 { Platform::Android } else { Platform::Pc },
            last_seen: Some(NOW - i64::from(n) * 5_400),
            note: String::new(),
            favourite_groups: favourite.clone(),
            boop_ready_at: None,
            request_ready_at: None,
        });
        for g in &favourite {
            if let Some(group) = state.groups.iter_mut().find(|x| &x.id == g) {
                group.members.push(id.clone());
            }
        }
        let section = match presence {
            Presence::InGame => "in-game",
            Presence::Website => "website",
            Presence::Offline => "offline",
        };
        state.groups.iter_mut().find(|x| x.id == section).unwrap().members.push(id);
    }
    // The sample's own seven get pictures and worlds too.
    for (i, f) in state.friends.iter_mut().enumerate().take(7) {
        f.look.icon = format!("sample://icon/{}", 50 + i);
        if i != 2 {
            f.look.banner = format!("sample://banner/{}", i % 7);
        }
        if let Whereabouts::World(place) = &mut f.whereabouts {
            place.world_picture = format!("sample://world/{}", i % 5);
        }
    }
    if let Some(me) = state.me.as_mut() {
        me.look.icon = "sample://icon/60".into();
        me.look.banner = "sample://banner/3".into();
    }
    for invite in &mut state.invites {
        invite.from_icon = state.friends.iter().find(|f| f.id == invite.from_id).map(|f| f.look.icon.clone()).unwrap_or_default();
        if let Some(place) = invite.place.as_mut() {
            place.world_picture = "sample://world/2".into();
        }
    }
    for boop in &mut state.boops {
        boop.from_icon = state.friends.iter().find(|f| f.id == boop.from_id).map(|f| f.look.icon.clone()).unwrap_or_default();
    }
    if let Some(profile) = state.profile.as_mut() {
        profile.friend = state.friends[0].clone();
        profile.bio = "Made-up person for drawing the page. Likes quiet worlds, rooftop sunsets and long talks about shaders.\nAlways up for a boop.".into();
        profile.links = vec!["https://example.com/nova".into(), "https://example.org/gallery".into()];
        profile.languages = vec!["eng".into(), "fra".into(), "jpn".into()];
        profile.badges.push(Badge { id: "bdg_sample2".into(), name: "Early supporter".into(), description: "Another made-up badge".into(), picture: String::new() });
    }
}

/// The page's data for a scenario.
fn data(name: &str) -> LauncherState {
    let mut s = sample();
    more_people(&mut s);
    if name == "launcher-starting" {
        s.game = Game::Starting(LaunchMode::Vr);
    }
    if name == "launcher-database" {
        // A VRCX copy: who and what was last known, nothing live.
        s.source = Source::Database { app: DatabaseApp::Vrcx, read_at: NOW - 420 };
        s.possible = Possible { play: true, ..Default::default() };
        s.invites.clear();
        s.boops.clear();
        s.profile = None;
        for f in &mut s.friends {
            f.presence = Presence::Offline;
            f.whereabouts = Whereabouts::Unknown;
            f.status = Status::Offline;
        }
        let all: Vec<String> = s.friends.iter().map(|f| f.id.clone()).collect();
        for g in &mut s.groups {
            match g.kind {
                GroupKind::Offline => g.members = all.clone(),
                GroupKind::InGame | GroupKind::Website => g.members.clear(),
                GroupKind::Favourite => {}
            }
        }
    }
    s
}

/// A made-up picture for a `sample://` URL: a round-headed creature on its colour, a banner of soft lights, a world.
fn picture(url: &str) -> ColorImage {
    let seed: u32 = url.rsplit('/').next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let hue = (seed as f32 * 0.137).fract();
    let rgb = |h: f32, s: f32, v: f32| -> [f32; 3] {
        let i = (h * 6.0).floor();
        let f = h * 6.0 - i;
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
        match i as i32 % 6 {
            0 => [v, t, p],
            1 => [q, v, p],
            2 => [p, v, t],
            3 => [p, q, v],
            4 => [t, p, v],
            _ => [v, p, q],
        }
    };
    let px = |c: [f32; 3]| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
    if url.contains("/frame/") {
        // A ring with little studs, transparent inside and out.
        let n = 128usize;
        let ring = rgb((hue + 0.1).fract(), 0.6, 0.95);
        let mut pixels = Vec::with_capacity(n * n);
        for y in 0..n {
            for x in 0..n {
                let (dx, dy) = (x as f32 / n as f32 - 0.5, y as f32 / n as f32 - 0.5);
                let d = (dx * dx + dy * dy).sqrt();
                let a = dy.atan2(dx);
                let stud = (a * 6.0 / std::f32::consts::PI).fract().abs() < 0.18 && (0.43..0.5).contains(&d);
                let band = (0.435..0.47).contains(&d);
                pixels.push(if band || stud { px([ring[0] * (0.8 + 0.4 * dy.abs()), ring[1], ring[2] * 0.7]) } else { Color32::TRANSPARENT });
            }
        }
        return ColorImage::new([n, n], pixels);
    }
    if url.contains("/icon/") {
        let n = 96usize;
        let bg = rgb(hue, 0.45, 0.55);
        let fur = rgb((hue + 0.5).fract(), 0.25, 0.92);
        let mut pixels = Vec::with_capacity(n * n);
        for y in 0..n {
            for x in 0..n {
                let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
                let head = (fx - 0.5).powi(2) + ((fy - 0.56) * 1.1).powi(2) < 0.09;
                let ear = |cx: f32| fy > 0.14 && fy < 0.42 && (fx - cx).abs() < (fy - 0.14) * 0.55;
                let eye = ((fx - 0.4).powi(2) + (fy - 0.55).powi(2) < 0.0012) || ((fx - 0.6).powi(2) + (fy - 0.55).powi(2) < 0.0012);
                let shade = 1.0 - 0.25 * fy;
                let c = if eye {
                    [0.08, 0.08, 0.1]
                } else if head || ear(0.32) || ear(0.68) {
                    [fur[0] * shade, fur[1] * shade, fur[2] * shade]
                } else {
                    [bg[0] * (1.1 - fy * 0.5), bg[1] * (1.1 - fy * 0.5), bg[2] * (1.1 - fy * 0.5)]
                };
                pixels.push(px(c));
            }
        }
        return ColorImage::new([n, n], pixels);
    }
    let (w, h) = if url.contains("/world/") { (320usize, 180usize) } else { (360usize, 120usize) };
    let a = rgb(hue, 0.7, 0.55);
    let b = rgb((hue + 0.18).fract(), 0.8, 0.35);
    let lights: Vec<(f32, f32, f32)> = (0..9).map(|k| {
        let k = k as f32 + seed as f32 * 3.1;
        ((k * 0.618).fract(), (k * 0.414 + 0.2).fract(), 0.05 + (k * 0.27).fract() * 0.1)
    }).collect();
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let t = (fx * 0.6 + fy * 0.4).clamp(0.0, 1.0);
            let mut c = [a[0] * (1.0 - t) + b[0] * t, a[1] * (1.0 - t) + b[1] * t, a[2] * (1.0 - t) + b[2] * t];
            if url.contains("/world/") {
                // A horizon and a few buildings.
                let ground = fy > 0.68 + 0.04 * (fx * 9.0 + seed as f32).sin();
                let tower = ((fx * 7.0 + seed as f32 * 0.3).fract() < 0.18) && fy > 0.42 + 0.2 * ((fx * 7.0).floor() * 1.7).sin().abs();
                if ground || tower {
                    c = [c[0] * 0.25, c[1] * 0.25, c[2] * 0.3];
                }
            }
            for (lx, ly, r) in &lights {
                let d = ((fx - lx) * (w as f32 / h as f32)).powi(2) + (fy - ly).powi(2);
                let g = (1.0 - d / (r * r)).max(0.0) * 0.5;
                c = [c[0] + g, c[1] + g * 0.9, c[2] + g * 0.8];
            }
            pixels.push(px([c[0].min(1.0), c[1].min(1.0), c[2].min(1.0)]));
        }
    }
    ColorImage::new([w, h], pixels)
}

/// What happens on frame `f` (of `frames`) of a launcher scenario.
pub(super) fn stage(ctx: &egui::Context, app: &mut App, name: &str, f: u32, frames: u32, t: f64) {
    if !name.starts_with("launcher-") {
        return;
    }
    let state = app.home_mut().launcher_mut();
    if f == 0 {
        let data = data(name);
        let mut urls: Vec<String> = Vec::new();
        for fr in data.friends.iter().chain(data.me.iter()) {
            urls.extend([fr.look.icon.clone(), fr.look.banner.clone(), fr.look.nameplate.clone(), fr.look.icon_frame.clone()]);
            if let Whereabouts::World(p) = &fr.whereabouts {
                urls.push(p.world_picture.clone());
            }
        }
        urls.push("sample://world/2".into());
        urls.sort();
        urls.dedup();
        for url in urls.into_iter().filter(|u| u.starts_with("sample://")) {
            let texture = ctx.load_texture(url.clone(), picture(&url), TextureOptions::LINEAR);
            state.provide_picture(&url, texture);
        }
        let invites: Vec<String> = data.invites.iter().filter(|i| i.kind == InviteKind::Invite).map(|i| i.id.clone()).collect();
        state.set_data(std::sync::Arc::new(data));
        state.stand_in(NOW, "");
        if name != "launcher-invite" {
            for id in invites {
                state.stand_in_dismiss(&id);
            }
        }
    }
    let nova = "usr_00000000-0000-4000-8000-000000000001";
    match name {
        "launcher-hover" => {
            // Pixel Fox was asked for an invite a second ago; Mochi Byte booped a third of a second ago.
            if f + 60 == frames {
                state.stand_in_request("usr_00000000-0000-4000-8000-000000000002", t);
            }
            if f + 20 == frames {
                state.stand_in_boop("usr_00000000-0000-4000-8000-000000000003", t, None);
            }
        }
        "launcher-modal" | "launcher-small-modal" if f == 30 => {
            state.open_profile(ctx, nova, Rect::from_min_size(pos2(954.0, 348.0), vec2(306.0, 54.0)));
        }
        "launcher-small-drawer" if f == 30 => state.open_drawer(ctx),
        _ => {}
    }
}
