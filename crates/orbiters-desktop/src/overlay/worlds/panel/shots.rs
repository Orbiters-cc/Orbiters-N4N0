//! The panel's pages and the capture's bar with stand-in captures and VRChat worlds (a search, VRChat's lists, the
//! worlds visited, a world without a capture), for the offline review (`--world-view <out>
//! --panel-only`, `--screenshot <out> --only worlds-overlay`): `panel-<page>.png`, `bar-<state>.png`, and
//! `with-bar-<page>.png` (the panel with the bar under it, as they hang in the play space).

use super::*;
use crate::overlay::Canvas;
use orbiters_i18n::{tr};
use orbiters_n4n0_core::feedback::FeedbackView;

/// Stand-in captures: names, authors, splats, a sky, likes and labels for everyone.
fn stand_ins() -> anyhow::Result<Vec<Entry>> {
    let names = [
        ("VRChat Home", "vrchat", 1_026_271u32, false),
        ("VRC NYE 2026 - Normal Capacity", "VRChat", 1_034_068, true),
        ("Autumn fragrance / 秋の香り", "_Harim", 722_750, false),
        ("Find the Needle", "Roofy.", 487_568, false),
        ("Silent Dawn", "Mei", 471_419, true),
        ("The Black Cat", "spookyghostboo", 1_520_000, false),
        ("Japan Shrine", "Kuroneko", 990_000, false),
        ("VRChat Home", "vrchat", 640_112, false),
    ];
    const CAPACITY: [u32; 8] = [32, 80, 16, 24, 20, 40, 32, 32];
    const VISITS: [u64; 8] = [48_211_302, 1_200_431, 81_224, 402_118, 23_455, 690_120, 151_002, 48_211_302];
    const FAVORITES: [u64; 8] = [2_310_400, 41_200, 6_120, 30_800, 1_920, 52_300, 12_400, 2_310_400];
    const PICTURES: [u32; 8] = [12, 3, 0, 1, 6, 0, 2, 12];
    let world = |i: usize| if i == 7 { "wrld_stand_in_0".to_string() } else { format!("wrld_stand_in_{i}") };
    let json = serde_json::json!({
        "previews": names.iter().enumerate().map(|(i, (name, author, splats, sky))| serde_json::json!({
            "slot": i, "key": 100 + i, "id": 10 + i, "worldId": world(i), "name": name, "author": author,
            "uploader": if i == 7 { "Kana" } else { "blackorbit" },
            "description": if i == 0 { "Your home in VRChat: a quiet place to meet friends, try avatars and find worlds." } else { "" },
            "splats": { "pc": splats }, "view": { "position": [0.0, 1.6, 0.0], "target": [0.0, 1.6, -1.0], "floor": null, "eyeHeight": null },
            "sky": sky, "official": i == 1, "capturedAt": format!("2026-10-{:02}T20:00:00Z", 28 - i),
            "capacity": CAPACITY[i], "visits": VISITS[i], "favorites": FAVORITES[i], "pictures": PICTURES[i],
        })).collect::<Vec<_>>(),
        "feedback": {
            "wrld_stand_in_0": { "likes": 41, "labels": { "meetup": 12, "mirror": 9, "chill": 7, "avatars": 4, "bright": 2, "quest": 1 }, "comments": 7,
                "recent": [{ "by": "Kana", "text": "The mirror room is perfect for photos, and the lights at night are lovely.", "at": "2026-10-29T21:04:00Z" }] },
            "wrld_stand_in_1": { "likes": 18, "labels": { "music": 9, "meetup": 6 }, "comments": 2 },
            "wrld_stand_in_2": { "likes": 66, "labels": { "nature": 14, "photo-spot": 11, "chill": 5 }, "comments": 4 },
            "wrld_stand_in_4": { "likes": 9, "labels": { "chill": 3, "nature": 2, "sleep": 2 }, "comments": 1 },
            "wrld_stand_in_5": { "likes": 3, "labels": { "horror": 5, "dark": 4, "game": 1 }, "comments": 0 },
            "wrld_stand_in_6": { "likes": 27, "labels": { "photo-spot": 8, "nature": 3 }, "comments": 3 },
        }
    });
    super::super::catalog::parse(&json.to_string())
}

/// A real-looking world id for stand-in world `i` (Open on VRChat needs one).
fn world_id(i: u32) -> String {
    format!("wrld_{i:08x}-5a1e-4000-8000-{i:012x}")
}

/// VRChat's answer to a search or a list, as stand-ins: name, author, people now, capacity, visits, favorites, and
/// `Some(i)` for the stand-in capture's world it is (`wrld_stand_in_<i>`).
fn stand_in_worlds(names: &[(&str, &str, u32, Option<usize>)]) -> Vec<orbiters_core::maps::FoundWorld> {
    names
        .iter()
        .enumerate()
        .map(|(i, (name, author, occupants, capture))| orbiters_core::maps::FoundWorld {
            world: orbiters_core::maps::World {
                world_id: capture.map_or_else(|| world_id(i as u32 + 1), |c| format!("wrld_stand_in_{c}")),
                name: name.to_string(),
                author_name: Some(author.to_string()),
                capacity: Some(16 + (i as u32 % 4) * 16),
                visits: Some(12_000 + i as u64 * 48_211),
                favorites: Some(800 + i as u64 * 2_310),
                tags: vec!["author_tag_rooftop".into(), "author_tag_city_lights".into(), "system_approved".into()],
                thumbnail_url: Some(format!("https://api.vrchat.cloud/stand-in/{i}")),
                ..Default::default()
            },
            occupants: Some(*occupants),
            ..Default::default()
        })
        .collect()
}

/// The worlds' pages on the Map explorer: frame rates, tags, pictures.
fn stand_in_facts(entries: &[Entry]) -> HashMap<String, Facts> {
    let perf = |fps: u32, low: u32, ratio: f32| WorldPerformance { fps, fps_low: low, ratio, minutes: 640, members: 9, by_crowd: Vec::new() };
    let mut out = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let performance = match i {
            0 => Some(perf(84, 61, 0.93)),
            1 => Some(perf(52, 31, 0.58)),
            2 => Some(perf(71, 48, 0.79)),
            4 => Some(perf(88, 70, 0.97)),
            6 => Some(perf(64, 40, 0.71)),
            _ => None,
        };
        let pictures = (0..e.pictures as u64)
            .map(|n| WorldPicture {
                id: 1000 + i as u64 * 100 + n,
                width: Some(1920),
                height: Some(1080),
                taken_at: Some(format!("2026-10-{:02}T18:30:00Z", 27 - n % 20)),
                by: Some(["Kana", "blackorbit", "Mei", "Roofy."][n as usize % 4].into()),
                ..Default::default()
            })
            .collect();
        let tags = if i == 0 { vec!["home".into(), "hangout".into(), "mirror".into(), "official".into()] } else { Vec::new() };
        out.entry(e.world_id.clone()).or_insert(Facts { thumbnail: None, performance, tags, pictures, picture_count: e.pictures as usize, ..Default::default() });
    }
    out
}

/// The member's own feedback on VRChat Home (liked, two labels, a comment of theirs), read from Orbiters.
fn stand_in_feedback(signed_in: bool) -> FeedbackView {
    let comments = vec![
        CommentRow { id: 381, by: "You".into(), text: "Came here for the mirror, stayed for the music.".into(), at: 1_793_300_000, mine: true }, // i18n: not shown (data: the page writes a mine comment's author with tr)
        CommentRow { id: 377, by: "Kana".into(), text: "The mirror room is perfect for photos, and the lights at night are lovely.".into(), at: 1_793_250_000, mine: false },
        CommentRow { id: 370, by: "Mei".into(), text: "Quiet in the morning, busy after 9 pm. Great to meet new people.".into(), at: 1_792_900_000, mine: false },
        CommentRow { id: 352, by: "Roofy.".into(), text: "Pens by the stage.".into(), at: 1_792_500_000, mine: false },
        CommentRow { id: 340, by: "spookyghostboo".into(), text: "Would love a darker room for sleeping.".into(), at: 1_792_000_000, mine: false },
    ];
    let labels = [("meetup", 12, true), ("mirror", 9, true), ("chill", 7, false), ("avatars", 4, false), ("bright", 2, false), ("quest", 1, false)];
    FeedbackView {
        signed_in,
        subjects: Vec::new(),
        worlds: vec![WorldFeedback {
            world: "wrld_stand_in_0".into(),
            loading: false,
            likes: 42,
            liked: true,
            labels: labels.iter().map(|(id, count, mine)| LabelCount { id: id.to_string(), count: *count, mine: *mine }).collect(),
            justification: None,
            comments,
            comment_count: 7,
            pending: 0,
            error: None,
        }],
    }
}

/// Soft gradients like the menu's history stand-ins.
/// The real worlds a scene is to show, by name and picture.
fn real_worlds() -> Vec<(String, egui::ColorImage)> {
    let Ok(dir) = std::env::var("ORBIT_PANEL_WORLDS") else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<std::path::PathBuf> = read.flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let image = image::open(&path).ok()?.to_rgba8();
            let stem = path.file_stem()?.to_string_lossy().to_string();
            let name = stem.split_once(' ').map_or(stem.as_str(), |(_, name)| name).to_string();
            Some((name, egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())))
        })
        .collect()
}

/// The real worlds shown get what the app itself read of them (`ORBIT_WORLD_PAGES`, its cache of world pages: who made each, what it says of itself, its numbers),
/// by name. A stand-in's own are never left under a real world's name: round 18's judges read VRChat Home's description under Fort Rosebay. Where a real world's
/// page is not there, it says nothing of itself.
fn as_the_real_worlds(entries: &mut [Entry], real: &[(String, egui::ColorImage)]) {
    let pages: Vec<serde_json::Value> = std::env::var("ORBIT_WORLD_PAGES")
        .ok()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .map(|read| read.flatten().filter_map(|f| serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()).collect())
        .unwrap_or_default();
    for (entry, (name, _)) in entries.iter_mut().zip(real) {
        entry.name = name.clone();
        let world = pages.iter().map(|p| &p["page"]["world"]).find(|w| w["name"].as_str() == Some(name.as_str()));
        entry.description = world.and_then(|w| w["description"].as_str()).unwrap_or_default().to_string();
        if let Some(world) = world {
            entry.author = world["authorName"].as_str().map(str::to_string);
            entry.capacity = world["capacity"].as_u64().map(|n| n as u32);
            entry.visits = world["visits"].as_u64();
            entry.favorites = world["favorites"].as_u64();
        }
    }
}

fn gradient(ctx: &egui::Context, name: &str, hue: f32, w: usize, h: usize) -> egui::TextureHandle {
    let (a, b) = (hsv(hue, 0.55, 0.62), hsv((hue + 0.08).fract(), 0.6, 0.22));
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let t = ((x as f32 / w as f32) * 0.6 + (y as f32 / h as f32) * 0.4).clamp(0.0, 1.0);
            let horizon = (y as f32 / h as f32 - 0.62).abs() < 0.004;
            px.push(if horizon { theme::white(0.5) } else { theme::mix(a, b, t) });
        }
    }
    ctx.load_texture(name, egui::ColorImage { size: [w, h], source_size: vec2(w as f32, h as f32), pixels: px }, egui::TextureOptions::LINEAR)
}

/// A world's side panel on a screen: with `live`, its capture live in 3D in its picture (the frame the sheet's 3D view drew), its full-screen button in that
/// picture's corner, and whether the view is on the whole page; without, its picture and See it in 3D. As the sheet shows it, `px` pixels, premultiplied. The list is the stand-in captures', with the
/// real worlds' names and pictures where `ORBIT_PANEL_WORLDS` gives them.
pub fn side_live(px: [u32; 2], live: Option<(&egui::ColorImage, bool)>) -> anyhow::Result<Vec<u8>> {
    let mut entries = stand_ins()?;
    let real = real_worlds();
    as_the_real_worlds(&mut entries, &real);
    let facts = stand_in_facts(&entries);
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / WIDTH).in_orbit_look();
    let thumbs: Vec<egui::TextureHandle> = (0..entries.len())
        .map(|i| match real.get(i) {
            Some((_, image)) => canvas.ctx().load_texture(format!("real-{i}"), image.clone(), egui::TextureOptions::LINEAR),
            None => gradient(canvas.ctx(), &format!("stand-in-{i}"), (i.min(6) as f32 * 0.137 + 0.55).fract(), 320, 180),
        })
        .collect();
    let live = live.map(|(frame, full)| (canvas.ctx().load_texture("live", frame.clone(), egui::TextureOptions::LINEAR), full));
    let thumb = |e: &Entry| -> Option<TextureId> { entries.iter().position(|x| x.id == e.id).map(|i| thumbs[i].id()) };
    let picture = |_: u64, _: bool| -> Option<TextureId> { None };
    let feedback = stand_in_feedback(true);
    // Signed in to VRChat, as the live captures are: the same lists (Popular) and Invite me.
    let mut status = Status::default();
    status.list.vrchat = true;
    let panel = Panel { on_sheet: true, section: Section::Captures, page: Page::Detail, selected: Some(10), live: live.as_ref().map(|(texture, full)| Live { id: 10, picture: Some(texture.id()), full: *full, ..Default::default() }), ..Default::default() };
    let d = Data { entries: &entries, picked: None, status: &status, feedback: &feedback, facts: &facts, thumb: &thumb, picture: &picture, now: 1_793_310_000 };
    for _ in 0..2 {
        canvas.paint(5.0, |p| panel.draw(p, &d, 5.0));
    }
    let from = canvas.pixels().to_vec();
    let mut out = from.clone();
    crate::overlay::orbit::panels::finish_picture(&mut out, &from, px, px[0] as f32 / WIDTH, [0, 0, px[0], px[1]]);
    Ok(out)
}

/// A world's side panel in a headset with its capture standing small in its hole (G1), in the language now: `px` pixels, premultiplied.
pub fn side_small(px: [u32; 2]) -> anyhow::Result<Vec<u8>> {
    let mut entries = stand_ins()?;
    let real = real_worlds();
    as_the_real_worlds(&mut entries, &real);
    let facts = stand_in_facts(&entries);
    let mut canvas = Canvas::zoomed(px[0], px[1], px[0] as f32 / WIDTH).in_orbit_look();
    let thumbs: Vec<egui::TextureHandle> = (0..entries.len())
        .map(|i| match real.get(i) {
            Some((_, image)) => canvas.ctx().load_texture(format!("real-{i}"), image.clone(), egui::TextureOptions::LINEAR),
            None => gradient(canvas.ctx(), &format!("stand-in-{i}"), (i.min(6) as f32 * 0.137 + 0.55).fract(), 320, 180),
        })
        .collect();
    let thumb = |e: &Entry| -> Option<TextureId> { entries.iter().position(|x| x.id == e.id).map(|i| thumbs[i].id()) };
    let picture = |_: u64, _: bool| -> Option<TextureId> { None };
    let feedback = stand_in_feedback(true);
    let mut status = Status { current: Some(14), shown: true, small: true, small_docked: true, headset: true, ..Default::default() };
    status.list.vrchat = true;
    let panel = Panel { on_sheet: true, section: Section::Captures, page: Page::Detail, selected: Some(14), ..Default::default() };
    let d = Data { entries: &entries, picked: None, status: &status, feedback: &feedback, facts: &facts, thumb: &thumb, picture: &picture, now: 1_793_310_000 };
    for _ in 0..2 {
        canvas.paint(5.0, |p| panel.draw(p, &d, 5.0));
    }
    let from = canvas.pixels().to_vec();
    let mut out = from.clone();
    crate::overlay::orbit::panels::finish_picture(&mut out, &from, px, px[0] as f32 / WIDTH, [0, 0, px[0], px[1]]);
    Ok(out)
}

pub fn shots(out: &std::path::Path) -> anyhow::Result<()> {
    let mut entries = stand_ins()?;
    // Real worlds' names and pictures, where given: `ORBIT_PANEL_WORLDS`, a folder of "NN Name.jpg".
    let real = real_worlds();
    as_the_real_worlds(&mut entries, &real);
    let facts = stand_in_facts(&entries);
    let mut canvas = Canvas::new(WIDTH as u32, HEIGHT as u32);
    let thumbs: Vec<egui::TextureHandle> = (0..entries.len())
        .map(|i| match real.get(i) {
            Some((_, image)) => canvas.ctx().load_texture(format!("real-{i}"), image.clone(), egui::TextureOptions::LINEAR),
            None => gradient(canvas.ctx(), &format!("stand-in-{i}"), (i.min(6) as f32 * 0.137 + 0.55).fract(), 320, 180),
        })
        .collect();
    let pictures: HashMap<u64, egui::TextureHandle> = facts
        .values()
        .flat_map(|f| f.pictures.iter())
        .map(|w| (w.id, gradient(canvas.ctx(), &format!("picture-{}", w.id), (w.id as f32 * 0.071).fract(), 192, 108)))
        .collect();
    let world_thumbs: Vec<egui::TextureHandle> = (0..8).map(|i| gradient(canvas.ctx(), &format!("stand-in-world-{i}"), (i as f32 * 0.191 + 0.05).fract(), 320, 180)).collect();
    let thumb = |e: &Entry| -> Option<TextureId> {
        match entries.iter().position(|x| x.id == e.id) {
            Some(i) => (i != 3 || !real.is_empty()).then(|| thumbs[i].id()),
            None => Some(world_thumbs[e.name.len() % world_thumbs.len()].id()),
        }
    };
    let picture = |id: u64, _large: bool| -> Option<TextureId> { pictures.get(&id).map(|t| t.id()) };
    let feedback = stand_in_feedback(true);
    let now = 1_793_310_000u64;
    let mut paint = |name: &str, entries: &[Entry], panel: &Panel, status: &Status, feedback: &FeedbackView, time: f64| -> anyhow::Result<Vec<u8>> {
        let d = Data { entries, picked: None, status, feedback, facts: &facts, thumb: &thumb, picture: &picture, now };
        // Twice: textures load on the first frame.
        for _ in 0..2 {
            canvas.paint(time, |p| panel.draw(p, &d, time));
        }
        let rgba = straight(canvas.pixels());
        super::super::shots::write_png(&out.join(format!("panel-{name}.png")), WIDTH as u32, HEIGHT as u32, &rgba)?;
        Ok(rgba)
    };
    let idle = Status::default();
    // The 3D captures, as the panel listed them before it searched every world.
    let base = Panel { section: Section::Captures, ..Default::default() };
    let mut panel = Panel { dirty: true, ..base.clone() };
    paint("browse", &entries, &panel, &idle, &feedback, 5.0)?;
    panel.hover = Some(Target::Card(1));
    paint("browse-hover", &entries, &panel, &idle, &feedback, 5.0)?;
    panel.hover = None;
    paint("browse-appearing", &entries, &panel, &idle, &feedback, 0.12)?;
    let liked = Panel { sort: Sort::Liked, labels: vec!["nature".into()], ..base.clone() };
    paint("browse-liked-nature", &entries, &liked, &idle, &feedback, 5.0)?;
    let smooth = Panel { sort: Sort::Smooth, ..base.clone() };
    paint("browse-smoothest", &entries, &smooth, &idle, &feedback, 5.0)?;
    let filter = Panel { page: Page::Filter, labels: vec!["nature".into(), "chill".into()], hover: Some(Target::FilterLabel(1)), ..base.clone() };
    paint("filter", &entries, &filter, &idle, &feedback, 5.0)?;
    let mut typing = Panel { page: Page::Typing, query: "ho".into(), ..base.clone() };
    typing.pressed = Some(Target::Key('m'));
    paint("typing", &entries, &typing, &idle, &feedback, 5.2)?;
    let none = Panel { page: Page::Typing, query: "zzz".into(), ..base.clone() };
    paint("typing-none", &entries, &none, &idle, &feedback, 5.2)?;
    let detail = Panel { page: Page::Detail, selected: Some(10), ..base.clone() };
    // What the real download writes: megabytes read and in all, with a decimal.
    let megabytes = ("13.8", "32.9");
    let downloading = Status { current: Some(10), download: Some(0.42), stage: Some(tr!("Downloading {read} of {total} MB", read = megabytes.0, total = megabytes.1)), ..Default::default() };
    paint("loading", &entries, &detail, &downloading, &feedback, 5.0)?;
    let batch = ("786k", "6.4");
    let shown = Status { current: Some(10), shown: true, stats: Some(tr!("{count} splats · {ms} ms a batch · {pixels} px", count = batch.0, ms = batch.1, pixels = 1024)), ..Default::default() };
    let detail_shown = paint("shown", &entries, &detail, &shown, &feedback, 5.0)?;
    let liking = Panel { hover: Some(Target::Like), ..detail.clone() };
    paint("shown-like-hover", &entries, &liking, &shown, &feedback, 5.0)?;
    // Not read yet: the listing's numbers, what the member gave unknown.
    paint("shown-reading", &entries, &detail, &shown, &FeedbackView { signed_in: true, ..Default::default() }, 5.0)?;
    let signed_out = Panel { note: Some((tr("Sign in to Orbiters - N4N0 to give feedback.").into(), 4.0)), ..detail.clone() };
    paint("shown-signed-out", &entries, &signed_out, &shown, &stand_in_feedback(false), 5.0)?;
    // The card growing into the page, a third of the way and nearly there.
    let mut morph = Panel { page: Page::Browse, ..base.clone() };
    {
        let d = Data { entries: &entries, picked: None, status: &idle, feedback: &feedback, facts: &facts, thumb: &thumb, picture: &picture, now };
        morph.press(Target::Card(1), &d, 10.0);
    }
    paint("morph-1", &entries, &morph, &downloading, &feedback, 10.0 + 0.07)?;
    let early = ("3.3", "33.1");
    paint("morph-2", &entries, &morph, &Status { current: Some(11), download: Some(0.1), stage: Some(tr!("Downloading {read} of {total} MB", read = early.0, total = early.1)), ..Default::default() }, &feedback, 10.0 + 0.2)?;
    // A capture standing small, seen through the side panel's hole; a hand near it; taken; a world's panel in a headset.
    let mut docked = Status { current: Some(10), shown: true, small: true, small_docked: true, headset: true, ..Default::default() };
    docked.list.vrchat = true;
    paint("small-docked", &entries, &detail, &docked, &feedback, 5.0)?;
    paint("small-near", &entries, &detail, &Status { small_near: true, ..docked.clone() }, &feedback, 5.0)?;
    paint("small-taken", &entries, &detail, &Status { small_docked: false, small_taken: true, ..docked.clone() }, &feedback, 5.0)?;
    // K-carousel and K-small-stick, in a headset: a capture of the middle of the list, its neighbours peeking out; the laser on the capture (the stick moves in
    // it), on the carousel (the stick flips), on the arrow; the first of the list; a flip on its way (the next one's picture coming in, then waiting for its 3D).
    let vr_detail = Panel { on_sheet: true, page: Page::Detail, selected: Some(12), ..base.clone() };
    let mut middle = Status { current: Some(12), shown: true, small: true, small_docked: true, headset: true, ..Default::default() };
    middle.list.vrchat = true;
    paint("carousel", &entries, &vr_detail, &middle, &feedback, 5.0)?;
    paint("carousel-pointing-capture", &entries, &Panel { pointer: Some(super::VR_HERO.center()), ..vr_detail.clone() }, &middle, &feedback, 5.0)?;
    paint("carousel-pointing-band", &entries, &Panel { pointer: Some(egui::pos2(985.0, 140.0)), hover: Some(Target::FlipNext), ..vr_detail.clone() }, &middle, &feedback, 5.0)?;
    paint("carousel-near", &entries, &vr_detail, &Status { small_near: true, ..middle.clone() }, &feedback, 5.0)?;
    let first = Panel { selected: Some(10), ..vr_detail.clone() };
    paint("carousel-first", &entries, &first, &Status { current: Some(10), ..middle.clone() }, &feedback, 5.0)?;
    let mut flipping = Panel { moves: true, ..vr_detail.clone() };
    {
        let d = Data { entries: &entries, picked: None, status: &middle, feedback: &feedback, facts: &facts, thumb: &thumb, picture: &picture, now };
        flipping.press(Target::FlipNext, &d, 20.0);
        flipping.pressed = None;
    }
    let opening = Status { current: Some(13), stage: Some(tr("Opening it").into()), shown: false, ..middle.clone() };
    paint("carousel-flip-1", &entries, &flipping, &opening, &feedback, 20.06)?;
    paint("carousel-flip-2", &entries, &flipping, &opening, &feedback, 20.5)?;
    paint("carousel-flip-3d", &entries, &flipping, &Status { current: Some(13), ..middle.clone() }, &feedback, 21.0)?;
    // The other layout, for the owner to compare: neighbours in gutters beside a narrower picture.
    super::set_carousel_gutters(true);
    paint("carousel-gutters", &entries, &vr_detail, &middle, &feedback, 5.0)?;
    paint("carousel-gutters-pointing-band", &entries, &Panel { pointer: Some(egui::pos2(985.0, 140.0)), hover: Some(Target::FlipNext), ..vr_detail.clone() }, &middle, &feedback, 5.0)?;
    paint("carousel-gutters-first", &entries, &first, &Status { current: Some(10), ..middle.clone() }, &feedback, 5.0)?;
    super::set_carousel_gutters(false);
    paint("review-signed-in", &entries, &detail, &docked, &feedback, 5.0)?;
    let mut signed_out_review = docked.clone(); signed_out_review.list.vrchat = false;
    paint("review-signed-out", &entries, &detail, &signed_out_review, &feedback, 5.0)?;
    paint("side-headset", &entries, &detail, &Status { headset: true, ..Default::default() }, &feedback, 5.0)?;
    // A headset's list with no side panel, on a sheet: endless, with no arrows and no Back of its own; and part way through the side panel coming out.
    let sheet = Panel { on_sheet: true, ..base.clone() };
    paint("browse-headset", &entries, &sheet, &Status { headset: true, ..Default::default() }, &feedback, 5.0)?;
    let coming = Panel { on_sheet: true, moves: true, page: Page::Detail, selected: detail.selected, ..base.clone() };
    paint("side-coming", &entries, &coming, &Status { headset: true, ..Default::default() }, &feedback, 0.05)?;
    let failed = Status { listing_error: Some("Orbiters could not be reached (timed out)".into()), ..Default::default() }; // i18n: not shown (a stand-in server error, English like the real ones)
    paint("offline", &[], &base, &failed, &FeedbackView::default(), 5.0)?;
    let not_current = Panel { page: Page::Detail, selected: Some(12), ..base.clone() };
    paint("detail", &entries, &not_current, &shown, &feedback, 5.0)?;
    let labels = Panel { page: Page::Labels, selected: Some(10), hover: Some(Target::Label(2)), ..base.clone() };
    paint("labels", &entries, &labels, &shown, &feedback, 5.0)?;
    let reason = Panel { page: Page::Compose(Compose::Reason), selected: Some(10), draft: "The thumbnail and the textures look generated".into(), ..base.clone() }; // i18n: not shown (a typed draft)
    paint("reason", &entries, &reason, &shown, &feedback, 5.2)?;
    let comments = Panel { page: Page::Comments, selected: Some(10), confirm: Some((381, 4.0)), ..base.clone() };
    paint("comments", &entries, &comments, &shown, &feedback, 5.0)?;
    let comment = Panel { page: Page::Compose(Compose::Comment), selected: Some(10), draft: "Lovely at sunset, ".into(), ..base.clone() }; // i18n: not shown (a typed draft)
    paint("comment", &entries, &comment, &shown, &feedback, 5.2)?;
    // VR feedback F6: signed out, Write a comment is the way to sign in, and what it did is said under it; a read Orbiters did not answer is said, with Try
    // again, and a comment can still be written.
    let browser = tr("Your browser opens on your computer: sign in to Orbiters there, then write your comment.");
    let comments_signed_out = Panel { page: Page::Comments, selected: Some(10), hover: Some(Target::WriteComment), note: Some((browser.into(), 4.5)), ..base.clone() };
    paint("comments-signed-out", &entries, &comments_signed_out, &shown, &stand_in_feedback(false), 5.0)?;
    let world_10 = entries.iter().find(|e| e.id == 10).map(|e| world_key(&e.world_id)).unwrap_or_default();
    let unanswered = tr("Orbiters did not answer for this world's feedback. It tries again by itself.");
    let unread = FeedbackView {
        signed_in: true,
        worlds: vec![orbiters_n4n0_core::feedback::WorldFeedback { world: world_10, loading: true, error: Some(unanswered.into()), ..Default::default() }],
        ..Default::default()
    };
    let comments_unread = Panel { page: Page::Comments, selected: Some(10), hover: Some(Target::RetryFeedback), ..base.clone() };
    paint("comments-unread", &entries, &comments_unread, &shown, &unread, 5.0)?;
    let comment_signed_out = Panel { note: Some((browser.into(), 4.9)), ..comment.clone() };
    paint("comment-signed-out", &entries, &comment_signed_out, &shown, &stand_in_feedback(false), 5.2)?;
    let pictures_page = Panel { page: Page::Pictures, selected: Some(10), hover: Some(Target::Picture(1)), ..base.clone() };
    paint("pictures", &entries, &pictures_page, &shown, &feedback, 5.0)?;
    let picture_open = Panel { page: Page::Pictures, selected: Some(10), picture: Some(2), ..base.clone() };
    paint("picture", &entries, &picture_open, &shown, &feedback, 5.0)?;
    let browse_shown = paint("browse-shown", &entries, &base, &shown, &feedback, 5.0)?;
    // Every world: a search (VRChat's answer, the captures it found badged 3D and first), VRChat's lists, the worlds
    // visited, a world without a capture and its page.
    let cat = stand_in_worlds(&[
        ("The Black Cat", "spookyghostboo", 214, Some(5)),
        ("Cat Café ~ Neko no Mori", "Hinata", 38, None),
        ("Black Cat Bar", "Velvet", 12, None),
        ("Catacombs of Ash", "Morrow", 6, None),
        ("Cat's Cradle", "Pom", 0, None),
        ("Neko Island", "Yuzu", 91, None),
    ]);
    let no_feedback = |_: &str| super::super::catalog::Aggregate::default();
    let first: Vec<&Entry> = entries.iter().filter(|e| e.matches("cat")).collect();
    let found = super::super::finder::merge(&entries, &first, &cat, Some("cat"), &no_feedback);
    let vrchat = |source| ListStatus { searching: true, vrchat: source == FoundBy::Vrchat, source: Some(source), more: true, ..Default::default() };
    let searched = Status { list: vrchat(FoundBy::Vrchat), ..Default::default() };
    let search = Panel { query: "cat".into(), ..Default::default() };
    paint("search", &found, &search, &searched, &feedback, 5.0)?;
    let only = Panel { has_3d: true, ..search.clone() };
    paint("search-3d-only", &found, &only, &searched, &feedback, 5.0)?;
    let public = Status { list: ListStatus { vrchat: false, ..vrchat(FoundBy::Orbiters) }, ..Default::default() };
    paint("search-orbiters", &found, &search, &public, &feedback, 5.0)?;
    let typing_search = Panel { page: Page::Typing, query: "cat".into(), ..Default::default() };
    paint("typing-search", &found, &typing_search, &searched, &feedback, 5.2)?;
    let asking = Status { list: ListStatus { loading: true, more: false, source: None, ..vrchat(FoundBy::Vrchat) }, ..Default::default() };
    paint("typing-searching", &[], &Panel { query: "midnight r".into(), ..typing_search.clone() }, &asking, &feedback, 5.2)?;
    let popular_worlds = stand_in_worlds(&[
        ("Murder 4", "Jar", 1802, None),
        ("Just B Club", "Rei", 690, None),
        ("VRChat Home", "vrchat", 412, Some(0)),
        ("Midnight Rooftop", "Kiyo", 312, None),
        ("Cozy Cabin", "Mei", 77, None),
        ("Japan Shrine", "Kuroneko", 64, Some(6)),
    ]);
    let popular = super::super::finder::merge(&entries, &[], &popular_worlds, None, &no_feedback);
    let listed = Status { list: ListStatus { vrchat: true, source: Some(FoundBy::Vrchat), more: true, ..Default::default() }, ..Default::default() };
    paint("browse-popular", &popular, &Panel::default(), &listed, &feedback, 5.0)?;
    paint("browse-active", &popular, &Panel { section: Section::Active, hover: Some(Target::Card(1)), ..Default::default() }, &listed, &feedback, 5.0)?;
    let signed_out = Status { list: ListStatus { vrchat: false, ..Default::default() }, ..Default::default() };
    paint("browse-favorites-signed-out", &[], &Panel { section: Section::Favorites, ..Default::default() }, &signed_out, &feedback, 5.0)?;
    let mut visited = super::super::finder::merge(&entries, &[], &popular_worlds[2..], None, &no_feedback);
    for (i, e) in visited.iter_mut().enumerate() {
        e.visited = Some((now - 3_600 * (i as u64 * 7 + 2), 1 + i * 3));
    }
    paint("browse-visited", &visited, &Panel { section: Section::Visited, ..Default::default() }, &listed, &feedback, 5.0)?;
    let failed_list = Status { list: ListStatus { vrchat: true, error: Some("VRChat asks to wait 20 s before searching again".into()), ..vrchat(FoundBy::Vrchat) }, ..Default::default() }; // i18n: not shown (VRChat's own English answer)
    paint("search-failed", &[], &search, &failed_list, &feedback, 5.0)?;
    // A world without a capture: its page, with Launch VRChat (signed out of VRChat) or Invite me.
    let rooftop = popular.iter().find(|e| e.name == "Midnight Rooftop").cloned().expect("stand-in");
    let mut rooftop = rooftop;
    rooftop.description = "A rooftop over a city that never sleeps: neon, rain, a jukebox and a view over the bay. Best at night with friends.".into();
    rooftop.feedback = super::super::catalog::Aggregate { likes: 23, labels: vec![("chill".into(), 6), ("music".into(), 4), ("photo-spot".into(), 2)], comments: 0, recent: Vec::new() };
    let rooftop_list = [rooftop.clone()];
    let page = Panel { page: Page::Detail, selected: Some(rooftop.id), ..Default::default() };
    paint("world", &rooftop_list, &page, &signed_out, &stand_in_feedback(true), 5.0)?;
    let pointed_go = Panel { hover: Some(Target::Join), ..page.clone() };
    paint("world-go", &rooftop_list, &pointed_go, &listed, &stand_in_feedback(true), 5.0)?;
    // VR feedback F5: Go there, signed in to VRChat in Orbiters: reading the list, the list (a full one greyed, some hidden), none, not read, a new one on
    // its way, the invite sent; and more than a page of them.
    {
        use super::go::{GoBusy, GoView};
        use orbiters_core::vrchat_api::{InstanceAccess, InstanceRegion, WorldInstance, WorldInstances};
        let world = rooftop.world_id.clone();
        let one = |id: &str, access: InstanceAccess, region: &str, occupants: u32, capacity: Option<u32>, friends: Option<u32>, full: bool| WorldInstance {
            world: world.clone(),
            id: id.into(),
            access,
            region: region.into(),
            occupants,
            capacity,
            friends,
            full,
            closed: false,
            can_join: !full,
        };
        let items = vec![
            one("1", InstanceAccess::Public, "eu", 14, Some(32), Some(2), false),
            one("2", InstanceAccess::FriendsPlus, "eu", 5, Some(32), Some(1), false),
            one("3", InstanceAccess::Public, "us", 32, Some(32), None, true),
            one("4", InstanceAccess::GroupPublic, "jp", 9, Some(40), None, false),
            one("5", InstanceAccess::InvitePlus, "use", 2, None, Some(0), false),
        ];
        let go = |view: GoView| Status { go: GoView { world: world.clone(), region: Some(InstanceRegion::Eu), ..view }, ..listed.clone() };
        let chooser = Panel { page: Page::Go, selected: Some(rooftop.id), ..Default::default() };
        let pointed = Panel { hover: Some(Target::GoInstance(0)), ..chooser.clone() };
        let list = |items: Vec<WorldInstance>, unavailable: usize| Some(WorldInstances { items, unavailable, remaining: Vec::new() });
        paint("go-loading", &rooftop_list, &chooser, &go(GoView { loading: true, ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        paint("go-list", &rooftop_list, &pointed, &go(GoView { list: list(items[..4].to_vec(), 2), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        paint("go-empty", &rooftop_list, &chooser, &go(GoView { list: list(Vec::new(), 0), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        paint("go-unread", &rooftop_list, &Panel { hover: Some(Target::GoRetry), ..chooser.clone() }, &go(GoView::default()), &stand_in_feedback(true), 5.0)?;
        paint("go-creating", &rooftop_list, &chooser, &go(GoView { list: list(items.clone(), 0), busy: Some(GoBusy::Creating), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        let full = tr("That instance is full now.");
        paint("go-refused", &rooftop_list, &chooser, &go(GoView { list: list(items.clone(), 0), error: Some(full.into()), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        // L-go-direct: on the way directly, there, VRChat starting, an invite for a reason, and the invite asked for.
        {
            use super::go::GoneTo;
            use crate::controller::travel::{Detour, Went};
            let gone = |how, arrived| go(GoView { list: list(items.clone(), 0), went: Some(GoneTo { instance: items[1].clone(), how, arrived }), ..Default::default() });
            paint("go-direct", &rooftop_list, &Panel { hover: Some(Target::GoInvite), ..chooser.clone() }, &gone(Went::Direct, false), &stand_in_feedback(true), 5.0)?;
            paint("go-arrived", &rooftop_list, &chooser, &gone(Went::Direct, true), &stand_in_feedback(true), 5.0)?;
            paint("go-started", &rooftop_list, &chooser, &gone(Went::Started, false), &stand_in_feedback(true), 5.0)?;
            paint("go-invited-bridge", &rooftop_list, &chooser, &gone(Went::Invited(Detour::OtherVrchat), false), &stand_in_feedback(true), 5.0)?;
            paint("go-invited-remote", &rooftop_list, &chooser, &gone(Went::Invited(Detour::Remote), false), &stand_in_feedback(true), 5.0)?;
            paint("go-invited", &rooftop_list, &chooser, &gone(Went::Invited(Detour::Asked), false), &stand_in_feedback(true), 5.0)?;
            paint("go-going", &rooftop_list, &chooser, &go(GoView { list: list(items.clone(), 0), busy: Some(GoBusy::Going), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
            let made_fail = tr!("The instance could not be made: {e}", e = tr("VRChat could not be reached. Try again shortly."));
            paint("go-make-failed", &rooftop_list, &chooser, &go(GoView { list: list(items.clone(), 0), error: Some(made_fail), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        }
        let many: Vec<WorldInstance> = (0..12).map(|i| one(&format!("m{i}"), InstanceAccess::Public, "eu", i, Some(32), None, false)).collect();
        let first_batch: Vec<WorldInstance> = (0..5).map(|i| one(&format!("b{i}"), InstanceAccess::Public, "eu", i + 3, Some(32), None, false)).collect();
        let partial = Some(WorldInstances { items: first_batch, unavailable: 0, remaining: vec!["rest".into()] });
        paint("go-more-coming", &rooftop_list, &Panel { go_page: 1, ..chooser.clone() }, &go(GoView { list: partial, more_loading: true, ..Default::default() }), &stand_in_feedback(true), 5.0)?;
        paint("go-pages", &rooftop_list, &Panel { go_page: 1, ..chooser.clone() }, &go(GoView { list: list(many, 0), ..Default::default() }), &stand_in_feedback(true), 5.0)?;
    }
    // The chip under N4N0's menu (no capture).
    let mut chip_canvas = Canvas::new(CHIP_PX[0], CHIP_PX[1]);
    for (name, hover) in [("chip", false), ("chip-hover", true)] {
        chip_canvas.paint(1.0, |p| draw_chip(p, &Status::default(), None, hover, false, 1.0));
        super::super::shots::write_png(&out.join(format!("panel-{name}.png")), CHIP_PX[0], CHIP_PX[1], &straight(chip_canvas.pixels()))?;
    }
    // The bar, whatever the panel shows: loading, shown, See VRChat on with Sharp hovered, an error.
    let mut bar_canvas = Canvas::new(BAR_PX[0], BAR_PX[1]);
    let see = Status { see_through: true, quality: Quality::High, ..shown.clone() };
    let answer = "Orbiters answered 503"; // i18n: not shown (a stand-in server answer, English like the real one)
    let broken = Status { current: Some(10), error: Some(tr!("Could not download it: {text}", text = answer)), ..Default::default() };
    let mut bars = HashMap::new();
    for (name, status, hover) in [
        ("loading", &downloading, None),
        ("shown", &shown, None),
        ("see-through", &see, Some(Target::Quality(Quality::Balanced))),
        ("hover-open", &shown, Some(Target::Open)),
        ("error", &broken, None),
    ] {
        bar_canvas.paint(1.0, |p| draw_bar(p, status, Some("VRChat Home"), hover, None, 1.0));
        let rgba = straight(bar_canvas.pixels());
        super::super::shots::write_png(&out.join(format!("bar-{name}.png")), BAR_PX[0], BAR_PX[1], &rgba)?;
        bars.insert(name, rgba);
    }
    // The panel with the bar under it, as in the play space (the bar 12 mm under the menu): the capture's page open,
    // and the results (its page closed).
    let gap = 20u32;
    for (name, panel_rgba) in [("detail", &detail_shown), ("browse", &browse_shown)] {
        let (w, h) = (WIDTH as u32, HEIGHT as u32 + gap + BAR_PX[1]);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        rgba[..panel_rgba.len()].copy_from_slice(panel_rgba);
        let at = ((HEIGHT as u32 + gap) * w * 4) as usize;
        rgba[at..at + bars["shown"].len()].copy_from_slice(&bars["shown"]);
        super::super::shots::write_png(&out.join(format!("with-bar-{name}.png")), w, h, &rgba)?;
    }
    Ok(())
}
