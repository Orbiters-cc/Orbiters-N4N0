//! The launcher on Home (`pages/launcher`) over made-up people only (`orbiters_core::launcher::sample` and more of the
//! same, pictures painted here): the list, the database-only list, a row hovered with a boop and a request just sent,
//! a profile open, an invite, and (run with `--width 900`) the rail and its drawer.
use super::{Page, Scenario};
use crate::app::App;
use egui::{pos2, vec2, Color32, ColorImage, Rect, TextureOptions};
use crate::pages::launcher::mine::Editing;
use orbiters_core::launcher::*;

const NOW: i64 = 1_795_000_000;

pub(super) const NAMES: [&str; 32] = [
    "launcher-list",
    "launcher-invite",
    "launcher-hover",
    "launcher-modal",
    "launcher-database",
    "launcher-starting",
    "launcher-no-engine",
    "launcher-not-installed",
    "launcher-small",
    "launcher-small-drawer",
    "launcher-small-modal",
    // REQUEST15 7: the Play buttons with VRChat's profiles, the account's own profile changed where it shows.
    "launcher-running-one",
    "launcher-profiles",
    "launcher-profile-starting",
    "launcher-mine-status-text",
    "launcher-mine-bio",
    "launcher-mine-saved",
    "launcher-mine-failed",
    "launcher-mine-banner",
    "launcher-mine-icons",
    "launcher-mine-gallery",
    "launcher-small-profiles",
    "launcher-small-mine",
    // REQUEST15 third pass: signed out (or another account) with the old list still the data, a profile open before.
    "launcher-stale",
    "launcher-small-stale",
    // Request16 1: the status list open under its pill, each row's dot in its status's colour.
    "launcher-mine-status",
    // REQUEST18 1: every boop received is a row of the friends' one list, before the friends: six; thirty (and an invite
    // request), at the list's top ("launcher-boops-30-scrolled": the same list scrolled past them).
    "launcher-boops-6",
    "launcher-boops-30",
    // REQUEST20 3: the friends in VRCNext's sections (favourites in the game by group, in game, on the website, offline):
    // some folded; one favourite group in the game (one Favourites section); a search across the sections; the same list
    // scrolled down to the website and offline ones ("launcher-sections-scrolled" below). "launcher-list" is all open.
    "launcher-sections-folded",
    "launcher-sections-one-group",
    "launcher-sections-search",
    "launcher-sections-copy",
];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let base = scenarios[0].snapshot.clone();
    // Settings' Play VRChat card (its launch options), scrolled to.
    // REQUEST15 4: the FPS booster's slider in Settings (scrolled down to its card).
    for (name, scroll) in [("launcher-settings", 1500.0), ("launcher-settings-2", 2000.0), ("launcher-settings-booster", 820.0)] {
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
    // Request15 10: Home beside the launcher (its compact features) for an account without N4N0 calls: Call greyed out,
    // "Coming soon", a press told (run with --width 1040 --height 700 too).
    scenarios.push(Scenario {
        name: "launcher-calls-soon",
        page: Page::Home,
        snapshot: super::without_calls(&base),
        frames: 100,
        splash: false,
        open_detail: false,
        toasts: vec![(super::toast::Kind::Info, "Calls are coming soon to Orbiters - N4N0. They aren't open for your account yet.")],
        pointer: None,
        scroll: 0.0,
        world: None,
        home: None,
        library: None,
    });
    // Request16 3: scrollbars show only while scrolling. Home and the friends list half a second after a turn of the wheel over each (their bars up); run with
    // `--extend 90 --sequence 90` to see them fade once the wheel stops. "launcher-list" is the same page at rest, no bar.
    // REQUEST18 1: the friends' list with thirty boops at its top, scrolled past them by one turn of the wheel over it.
    for (name, pointer, scroll) in [
        ("launcher-scrolled", pos2(560.0, 520.0), 400.0),
        ("launcher-friends-scrolled", pos2(1100.0, 560.0), 400.0),
        ("launcher-boops-30-scrolled", pos2(1100.0, 560.0), 1720.0),
        ("launcher-sections-scrolled", pos2(1100.0, 560.0), 1300.0),
    ] {
        scenarios.push(Scenario {
            name,
            page: Page::Home,
            snapshot: base.clone(),
            frames: 100,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: Some(pointer),
            scroll,
            world: None,
            home: None,
            library: None,
        });
    }
    // REQUEST17: the list's pictures are VRChat addresses (made up) kept on this computer from an earlier run: read from
    // that cache by the loader, no network, on screen within frames of being asked (the frames are printed).
    scenarios.push(Scenario {
        name: "launcher-pictures-cached",
        page: Page::Home,
        snapshot: base.clone(),
        frames: 100,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll: 0.0,
        world: None,
        home: None,
        library: None,
    });
    // REQUEST19 1: the member's Explorer points on Home's profile card (made-up standings): a title on the way to the next,
    // none yet, the top title, Orbiters not said yet, and signed out of Orbiters (no chip). Run with --width 1040 and
    // --width 900 too, and --language fr / ja.
    let signed_out = super::with_account(&base, crate::state::AccountState::SignedOut, None);
    for (name, snapshot) in [
        ("launcher-points", super::with_explorer_points(&base, 12.5)),
        ("launcher-points-zero", super::with_explorer_points(&base, 0.0)),
        ("launcher-points-top", super::with_explorer_points(&base, 1200.0)),
        ("launcher-points-waiting", base.clone()),
        ("launcher-points-signed-out", signed_out),
        ("launcher-small-points", super::with_explorer_points(&base, 12.5)),
    ] {
        scenarios.push(Scenario {
            name,
            page: Page::Home,
            snapshot,
            frames: 100,
            splash: false,
            open_detail: false,
            toasts: vec![],
            pointer: None,
            scroll: 0.0,
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
    // Sorted as the app sorts a list it read (VRCNext's order).
    let groups: Vec<(String, String)> = state.groups.iter().filter(|g| g.kind == GroupKind::Favourite).map(|g| (g.id.clone(), g.name.clone())).collect();
    orbiters_core::launcher_feed::regroup(state, &groups);
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
    if let Some(n) = name.strip_prefix("launcher-boops-").and_then(|n| n.split('-').next()).and_then(|n| n.parse::<usize>().ok()) {
        // Made-up people of the list, newest first, a few minutes apart.
        s.boops = (0..n)
            .map(|i| {
                let f = &s.friends[(i * 7 + 3) % s.friends.len()];
                orbiters_core::launcher::Boop { id: format!("not_boop_{i:02}"), from_id: f.id.clone(), from_name: f.name.clone(), from_icon: f.look.icon.clone(), emoji: None, received_at: NOW - 40 - i as i64 * 170 }
            })
            .collect();
        if n < 10 {
            s.invites.retain(|i| i.kind != InviteKind::Request);
        }
    }
    if name == "launcher-starting" {
        // A Mac has the desktop game alone (its one button says that Obelisk is open).
        s.game = Game::Starting(if orbiters_core::vrchat_launch::VR_HERE { LaunchMode::Vr } else { LaunchMode::Desktop });
        s.invites.clear();
    }
    // What a Mac says while Obelisk, or VRChat in it, is missing (the line of a PC without VRChat elsewhere).
    if name == "launcher-no-engine" {
        s.game = Game::NoEngine;
    }
    if name == "launcher-not-installed" {
        s.game = Game::NotInstalled;
    }
    // VRChat's profiles (REQUEST15 7a): one running, or a second account beside it.
    let profile = |profile: u32, account: &str, running: Option<LaunchMode>, starting: Option<LaunchMode>| GameProfile { profile, account: Some(account.into()), running, starting, assistant: false };
    match name {
        "launcher-running-one" => {
            s.game = Game::Running(LaunchMode::Vr);
            s.profiles = vec![profile(0, "Sample Me", Some(LaunchMode::Vr), None)];
        }
        "launcher-profiles" | "launcher-small-profiles" => {
            s.game = Game::Running(LaunchMode::Vr);
            s.profiles = vec![profile(0, "Sample Me", Some(LaunchMode::Vr), None), profile(1, "Sample Alt", None, None), profile(2, "Sample Builder", None, None)];
        }
        "launcher-profile-starting" => {
            s.game = Game::Running(LaunchMode::Vr);
            s.profiles = vec![profile(0, "Sample Me", Some(LaunchMode::Vr), None), profile(1, "Sample Alt", None, Some(LaunchMode::Desktop))];
        }
        _ => {}
    }
    // The account's own profile (REQUEST15 7b): pictures to choose among, what VRChat answered.
    if let Some(mine) = s.mine.as_mut() {
        mine.avatar_picture = "sample://icon/60".into();
        mine.icons = Some((70..76).map(|n| OwnPicture { id: format!("file_{n}"), url: format!("sample://icon/{n}") }).collect());
        mine.gallery = Some((0..5).map(|n| OwnPicture { id: format!("file_g{n}"), url: format!("sample://world/{n}") }).collect());
        match name {
            "launcher-mine-saved" => {
                mine.saving = Some(Saving::Saved(ProfilePart::Bio));
                mine.answered = 1;
            }
            "launcher-mine-failed" => {
                mine.saving = Some(Saving::Failed(ProfilePart::StatusText, orbiters_i18n::tr!("VRChat is rate limiting requests. Try again in {seconds} seconds.", seconds = 30)));
                mine.answered = 1;
            }
            _ => {}
        }
    }
    if name.starts_with("launcher-mine") || name == "launcher-small-mine" {
        s.invites.clear();
    }
    if name == "launcher-sections-one-group" {
        // Nobody of the second group in the game now.
        for f in s.friends.iter_mut().filter(|f| f.favourite_groups.first().is_some_and(|g| g == "group_1") && !f.favourite_groups.contains(&"group_0".to_string())) {
            f.presence = Presence::Website;
            f.whereabouts = Whereabouts::Website;
        }
        let groups: Vec<(String, String)> = s.groups.iter().filter(|g| g.kind == GroupKind::Favourite).map(|g| (g.id.clone(), g.name.clone())).collect();
        orbiters_core::launcher_feed::regroup(&mut s, &groups);
    }
    if name == "launcher-database" || name == "launcher-sections-copy" {
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

/// A made-up VRChat address for a `sample://icon/N` picture.
fn vrchat_address(sample: &str) -> String {
    let n: u64 = sample.rsplit('/').next().and_then(|s| s.parse().ok()).unwrap_or(0);
    format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-{:012}/1/file", 7000 + n)
}

/// The people's pictures become VRChat addresses, each kept in this run's cache folder (as a run before would have left
/// it): the page reads them from there.
fn keep_pictures(data: &mut LauncherState) {
    let keep = |sample: &str, user: &str| -> String {
        let url = vrchat_address(sample);
        let c = picture(sample);
        let rgba: Vec<u8> = c.pixels.iter().flat_map(|p| p.to_array()).collect();
        let image = image::RgbaImage::from_raw(c.size[0] as u32, c.size[1] as u32, rgba).expect("a made-up picture");
        crate::worlds::profile_pictures::Profiles::shared().keep(&url, Some(user), &image);
        url
    };
    for f in data.friends.iter_mut().chain(data.me.iter_mut()) {
        if f.look.icon.starts_with("sample://icon/") {
            f.look.icon = keep(&f.look.icon, &f.id);
        }
    }
    for i in &mut data.invites {
        if i.from_icon.starts_with("sample://icon/") {
            i.from_icon = keep(&i.from_icon, &i.from_id);
        }
    }
    for b in &mut data.boops {
        if b.from_icon.starts_with("sample://icon/") {
            b.from_icon = keep(&b.from_icon, &b.from_id);
        }
    }
}

/// Says (once) from which frame the page asked for pictures and on which frame every one was on screen, and how long
/// that took in real time.
fn pictures_on_screen(state: &crate::pages::launcher::State, f: u32) {
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<(Option<(u32, std::time::Instant)>, bool)>> = OnceLock::new();
    let mut seen = SEEN.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    let loading = state.stand_in_pictures_loading();
    if loading && seen.0.is_none() {
        seen.0 = Some((f, std::time::Instant::now()));
    }
    if let (Some((asked, at)), false, false) = (seen.0, seen.1, loading) {
        seen.1 = true;
        println!("launcher-pictures-cached: pictures asked on frame {asked}, every one on screen on frame {f} ({} ms later)", at.elapsed().as_millis());
    }
}

/// What happens on frame `f` (of `frames`) of a launcher scenario.
pub(super) fn stage(ctx: &egui::Context, app: &mut App, name: &str, f: u32, frames: u32, t: f64) {
    if !name.starts_with("launcher-") {
        return;
    }
    let state = app.home_mut().launcher_mut();
    if name == "launcher-pictures-cached" && f > 0 {
        pictures_on_screen(state, f);
    }
    if f == 0 {
        let mut data = data(name);
        if name == "launcher-pictures-cached" {
            keep_pictures(&mut data);
        }
        let mut urls: Vec<String> = Vec::new();
        for fr in data.friends.iter().chain(data.me.iter()) {
            urls.extend([fr.look.icon.clone(), fr.look.banner.clone(), fr.look.nameplate.clone(), fr.look.icon_frame.clone()]);
            if let Whereabouts::World(p) = &fr.whereabouts {
                urls.push(p.world_picture.clone());
            }
        }
        if let Some(mine) = &data.mine {
            urls.push(mine.avatar_picture.clone());
            urls.extend(mine.icons.iter().flatten().chain(mine.gallery.iter().flatten()).map(|p| p.url.clone()));
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
        state.stand_in(NOW, if name == "launcher-sections-search" { "moth" } else { "" });
        match name {
            "launcher-sections-folded" => state.stand_in_folded(&["group_1", "in-game", "offline"]),
            // In game folded, Offline opened (it starts folded): the website's and the offline ones, by name.
            "launcher-sections-scrolled" => state.stand_in_folded(&["in-game"]),
            _ => {}
        }
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
        "launcher-profiles" | "launcher-small-profiles" | "launcher-profile-starting" if f == 1 => state.stand_in_profile(1),
        "launcher-stale" | "launcher-small-stale" if f == 10 => state.open_profile(ctx, nova, Rect::from_min_size(pos2(954.0, 348.0), vec2(306.0, 54.0))),
        "launcher-stale" | "launcher-small-stale" if f == 40 => state.stand_in_login(None),
        "launcher-mine-status-text" if f == 1 => state.mine.stand_in_editing(Editing::StatusText("building a new world tonight".into())),
        "launcher-mine-bio" | "launcher-small-mine" if f == 1 => state.mine.stand_in_editing(Editing::Bio("Made-up account for drawing the page.\nBuilds quiet worlds, mostly at night. Say hi!".into())),
        "launcher-mine-failed" if f == 1 => state.mine.stand_in_last(ProfileEdit::StatusText("afk".into())),
        "launcher-mine-banner" if f == 30 => state.mine.stand_in_picker(true, false, Rect::NOTHING, t),
        "launcher-mine-gallery" if f == 30 => state.mine.stand_in_picker(true, true, Rect::NOTHING, t),
        "launcher-mine-status" if f == 30 => state.mine.stand_in_status_list(ctx),
        "launcher-mine-icons" if f == 30 => state.mine.stand_in_picker(false, false, Rect::NOTHING, t),
        _ => {}
    }
}
