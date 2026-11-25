//! The avatar gun's picker (contracts `Docs/avatar-gun.md`), in VR and in desktop mode: the public avatars of the VRChat
//! account's avatar favourite lists, each list a section (its name and how many) over a grid of its avatars, their
//! pictures with their names under them. The avatar loaded is ringed, and shown in the page's head with what the gun
//! can do now. A press loads the gun at once (the ring goes to the pressed avatar before the settings say it back) and the
//! page stays. Reached from All tools (only while the worn avatar has the gun) and by itself when the gun's Choose is
//! pressed (`AvatarGun/Choose`). The lists come group by group (`crate::avatar_gun_picker`): a list not read yet keeps a
//! row of grey tiles in its place, so what arrives grows below what is looked at.

use super::*;
use crate::avatar_gun_picker::{self as picker, Stage, Store};
use crate::controller::vrchat_data::Scope;
use crate::state::Snapshot;

/// The page's head (the avatar loaded), where the lists start, their margins, their columns.
const HEAD_H: f32 = 64.0;
pub const TOP: f32 = 76.0;
const X: f32 = 24.0;
const WIDE: f32 = 1024.0 - 2.0 * X - 12.0;
const COLUMNS: usize = 6;
const GAP: f32 = 12.0;
/// A section's name over its grid, the line a list with nothing says, the room after a list.
const SECTION_H: f32 = 44.0;
const NOTE_H: f32 = 40.0;
const AFTER: f32 = 14.0;
/// Under each picture (4:3, VRChat's thumbnails), its name.
const NAME_H: f32 = 30.0;
/// Try again, in the head; Connect VRChat in the middle of the page.
const RETRY: Rect = Rect { min: Pos2 { x: 1024.0 - 24.0 - 176.0, y: 14.0 }, max: Pos2 { x: 1024.0 - 24.0, y: 54.0 } };
const CONNECT: Rect = Rect { min: Pos2 { x: 512.0 - 140.0, y: 352.0 }, max: Pos2 { x: 512.0 + 140.0, y: 404.0 } };

pub const TITLE: &str = mark("Avatar gun");
pub const OPEN_WHY: &str = mark("Load your gun with a public avatar from your favorites.");
/// All tools' tile, and the caption's way to use the page.
pub const TILE_LINE: &str = mark("Choose what your gun turns people into");
pub const HOW: &str = mark("Press an avatar to load your gun with it.");

fn tile_w() -> f32 {
    (WIDE - GAP * (COLUMNS - 1) as f32) / COLUMNS as f32
}

fn picture_h() -> f32 {
    tile_w() * 3.0 / 4.0
}

fn tile_h() -> f32 {
    picture_h() + NAME_H
}

/// An avatar as the page shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GunTile {
    pub id: String,
    pub name: String,
    pub url: String,
    pub picture: Option<TextureId>,
    pub loaded: bool,
}

/// A list as the page shows it: its name, its avatars (their place in [`GunFacts::tiles`]), or none yet, or why not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GunSection {
    pub name: String,
    pub tiles: Option<std::ops::Range<usize>>,
    pub error: Option<String>,
}

/// What the page draws, made from the picker's lists and the snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GunFacts {
    pub stage: Stage,
    pub sections: Vec<GunSection>,
    pub tiles: Vec<GunTile>,
    /// The avatar loaded: its name and picture.
    pub loaded: Option<(String, Option<TextureId>)>,
    /// What the gun can do now, in the head.
    pub status: String,
    pub ready: bool,
    /// A list failed: Try again shows.
    pub retry: bool,
    /// Whose session the pictures are read with.
    pub scope: Option<Scope>,
}

/// The page's facts: the lists of `store`, the avatar `picked` here a moment ago (ringed before the settings say it).
pub fn facts(store: &Store, snap: &Snapshot, picked: Option<&str>) -> GunFacts {
    let chosen = picked.map(str::to_string).or_else(|| snap.settings.avatar_gun.loaded.as_ref().map(|l| l.id.clone()));
    let mut tiles = Vec::new();
    let sections = store
        .groups()
        .iter()
        .map(|g| {
            let range = g.avatars.as_ref().map(|avatars| {
                let first = tiles.len();
                tiles.extend(avatars.iter().map(|a| GunTile { id: a.id.clone(), name: a.name.clone(), url: a.thumbnail.clone(), picture: None, loaded: chosen.as_deref() == Some(a.id.as_str()) }));
                first..tiles.len()
            });
            GunSection { name: g.name.clone(), tiles: range, error: g.error.as_ref().filter(|_| g.avatars.is_none()).map(picker::failure) }
        })
        .collect();
    let loaded = match (picked.and_then(|id| store.find(id)), snap.settings.avatar_gun.loaded.as_ref()) {
        (Some(p), _) => Some((p.name.clone(), None)),
        (None, Some(l)) => Some((l.name.clone(), None)),
        (None, None) => None,
    };
    let gun = &snap.avatar_gun;
    let status = if loaded.is_none() {
        tr("Choose an avatar: it's what your gun loads.")
    } else if !gun.gun {
        tr("Your avatar has no avatar gun. Add it in Orbiters' My Avatar.")
    } else if snap.account.state.username().is_none() {
        tr("Sign in to Orbiters to fire it.")
    } else if gun.loaded {
        tr("Loaded. Point at someone and pull the trigger.")
    } else {
        tr("Loading… the gun shows it when it's ready.")
    };
    GunFacts {
        stage: store.stage().clone(),
        sections,
        tiles,
        loaded,
        status: status.to_string(),
        ready: gun.gun && gun.loaded,
        retry: store.failed().is_some(),
        scope: Some(Scope::of(snap)),
    }
}

/// What the page lays: each list's name, its avatars (or its row of grey tiles, or its line), from the top of the list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Laid {
    Section(usize, f32),
    Tile(usize, Rect),
    Grey(Rect),
    Note(usize, f32),
}

/// The lists laid from the list's top (unscrolled), and their height.
fn layout(f: &GunFacts) -> (Vec<Laid>, f32) {
    let mut out = Vec::new();
    let mut y = 0.0;
    let (w, h) = (tile_w(), tile_h());
    let grid = |out: &mut Vec<Laid>, y: f32, n: usize, tile: &dyn Fn(usize, Rect) -> Laid| -> f32 {
        for i in 0..n {
            let (c, row) = ((i % COLUMNS) as f32, (i / COLUMNS) as f32);
            out.push(tile(i, r(X + c * (w + GAP), y + row * (h + GAP), w, h)));
        }
        n.div_ceil(COLUMNS) as f32 * (h + GAP) - GAP
    };
    // Nothing known yet: two lists' places, grey.
    if f.stage == Stage::Loading && f.sections.is_empty() {
        for _ in 0..2 {
            y += SECTION_H;
            y += grid(&mut out, y, COLUMNS, &|_, rect| Laid::Grey(rect)) + AFTER;
        }
        return (out, y);
    }
    for (s, section) in f.sections.iter().enumerate() {
        out.push(Laid::Section(s, y));
        y += SECTION_H;
        y += match (&section.tiles, &section.error) {
            (Some(range), _) if !range.is_empty() => grid(&mut out, y, range.len(), &|i, rect| Laid::Tile(range.start + i, rect)),
            (Some(_), _) | (None, Some(_)) => {
                out.push(Laid::Note(s, y));
                NOTE_H
            }
            (None, None) => grid(&mut out, y, COLUMNS, &|_, rect| Laid::Grey(rect)),
        } + AFTER;
    }
    (out, y)
}

/// Whether the page has lists to show (else it says why in its middle).
fn listing(f: &GunFacts) -> bool {
    match f.stage {
        Stage::Loading => true,
        // Something to list, unless every list is read and has nothing.
        Stage::Ready => !f.sections.is_empty() && !f.sections.iter().all(|s| s.tiles.as_ref().is_some_and(|r| r.is_empty())),
        _ => false,
    }
}

/// How far the list scrolls.
pub fn max_scroll(f: &GunFacts) -> f32 {
    if !listing(f) {
        return 0.0;
    }
    (TOP + layout(f).1 + 8.0 - 576.0).max(0.0)
}

/// Where the list may come to rest: each list's name and each row's top.
pub fn row_tops(f: &GunFacts) -> Vec<f32> {
    layout(f)
        .0
        .into_iter()
        .filter_map(|l| match l {
            Laid::Section(_, y) => Some(y),
            Laid::Tile(i, rect) if i % COLUMNS == 0 => Some(rect.top() - SECTION_H),
            _ => None,
        })
        .collect()
}

/// The avatars whose pictures to ask for: those in view, and a row each side.
pub fn shown(f: &GunFacts, scroll: f32) -> Vec<usize> {
    let margin = tile_h();
    layout(f)
        .0
        .into_iter()
        .filter_map(|l| match l {
            Laid::Tile(i, rect) if rect.bottom() + TOP - scroll > TOP - margin && rect.top() + TOP - scroll < 576.0 + margin => Some(i),
            _ => None,
        })
        .collect()
}

/// What can be pressed: each avatar in view, Try again, Connect VRChat.
pub fn items(facts: &Facts) -> Vec<Item> {
    let f = &facts.avatar_gun;
    let mut out = Vec::new();
    if listing(f) {
        let scroll = facts.gallery_scroll;
        for laid in layout(f).0 {
            if let Laid::Tile(i, rect) = laid {
                let rect = rect.translate(vec2(0.0, TOP - scroll));
                if rect.bottom() > TOP && rect.top() < 576.0 {
                    out.push(Item { action: Action::AvatarGunPick(i), rect, look: Look::AvatarGunTile(i) });
                }
            }
        }
    }
    if f.retry {
        out.push(Item { action: Action::AvatarGunRetry, rect: RETRY, look: Look::AvatarGunRetry });
    }
    if f.stage == (Stage::SignedOut { gated: false }) {
        out.push(Item { action: Action::ConnectVrchat, rect: CONNECT, look: Look::AvatarGunConnect });
    }
    out
}

/// The page under its items: its head (the avatar loaded and what the gun can do), the lists' names, their grey tiles and
/// lines, or why there is nothing.
pub fn draw_page(p: &Painter, facts: &Facts) {
    let f = &facts.avatar_gun;
    // The head: the loaded avatar's picture, ringed as on its tile, its name and what the gun can do.
    let pic = r(X, 8.0, (HEAD_H - 16.0) * 4.0 / 3.0, HEAD_H - 16.0);
    let radii = [theme::squircle(10.0); 4];
    match f.loaded.as_ref().and_then(|(_, t)| *t) {
        Some(id) => squircle_picture(p, pic, radii, id, cover_uv_of(pic.size(), 4.0 / 3.0), Color32::WHITE),
        None => {
            orbiters_ui::paint::fill_convex(p, &orbiters_ui::paint::squircle_points_corners(pic, radii), |_| look::TILE);
            widgets::icon(p, pic.center(), icons::CROSSHAIR, 24.0, look::muted());
        }
    }
    if f.loaded.is_some() {
        p.rect_stroke(pic.expand(2.5), look::radius(12.0), Stroke::new(2.5, if f.ready { look::accent() } else { look::muted() }), egui::StrokeKind::Middle);
    }
    let x = pic.right() + 16.0;
    let right = if f.retry { RETRY.left() - 16.0 } else { 1000.0 };
    let title = match &f.loaded {
        Some((name, _)) => tr!("Loaded: {name}", name = name),
        None => tr(TITLE).to_string(),
    };
    let font = look::font(21.0, weight::BOLD);
    p.text(pos2(x, 22.0), Align2::LEFT_CENTER, fit(p, &title, &font, right - x), font, look::label());
    let font = look::font(15.0, weight::MEDIUM);
    p.text(pos2(x, 46.0), Align2::LEFT_CENTER, fit(p, &f.status, &font, right - x), font, if f.ready { theme::mix(look::accent(), Color32::WHITE, 0.3) } else { look::muted() });
    p.line_segment([pos2(X, HEAD_H + 2.0), pos2(1024.0 - X, HEAD_H + 2.0)], Stroke::new(1.0, Color32::from_white_alpha(18)));
    if !listing(f) {
        let c = pos2(512.0, 270.0);
        let (icon, first, second): (&str, String, String) = match &f.stage {
            Stage::SignedOut { gated: true } => (icons::LOCK_SIMPLE, tr("VRChat sign-in needed").into(), tr(picker::GATED).into()),
            Stage::SignedOut { gated: false } => (icons::SIGN_IN, tr("Sign in to VRChat").into(), tr(picker::SIGN_IN).into()),
            Stage::Failed(e) => (icons::WARNING_CIRCLE, tr("Your favorites could not be read").into(), picker::failure(e)),
            _ => (icons::STAR, tr("No public avatar yet").into(), tr(picker::EMPTY).into()),
        };
        widgets::icon(p, c - vec2(0.0, 40.0), icon, 54.0, look::muted());
        text(p, c + vec2(0.0, 16.0), Align2::CENTER_CENTER, &first, 22.0, weight::BOLD, look::label());
        let mut job = egui::text::LayoutJob::simple(second, look::font(16.0, weight::MEDIUM), look::muted(), 640.0);
        job.halign = egui::Align::Center;
        let galley = p.layout_job(job);
        p.galley(pos2(c.x, c.y + 38.0), galley, look::muted());
        return;
    }
    let list = p.with_clip_rect(Rect::from_min_max(pos2(0.0, TOP - 6.0), pos2(1024.0, 576.0)));
    let scroll = facts.gallery_scroll;
    let at = |y: f32| y + TOP - scroll;
    for laid in layout(f).0 {
        match laid {
            Laid::Section(s, y) => {
                let Some(section) = f.sections.get(s) else { continue };
                let y = at(y) + SECTION_H / 2.0 - 2.0;
                let font = look::font(18.0, weight::BOLD);
                let name = fit(&list, &section.name, &font, 640.0);
                let wide = list.layout_no_wrap(name.clone(), font.clone(), look::label()).size().x;
                list.text(pos2(X, y), Align2::LEFT_CENTER, name, font, look::label());
                let count = match &section.tiles {
                    Some(range) => orbiters_i18n::fill(&orbiters_i18n::tr_n("{count} avatar", "{count} avatars", range.len() as u64), &[("count", &range.len())]),
                    None if section.error.is_some() => String::new(),
                    None => tr("Loading…").to_string(),
                };
                list.text(pos2(X + wide + 12.0, y + 1.0), Align2::LEFT_CENTER, count, look::font(15.0, weight::MEDIUM), look::muted());
            }
            Laid::Grey(rect) => {
                let rect = rect.translate(vec2(0.0, TOP - scroll));
                let pic = Rect::from_min_size(rect.min, vec2(rect.width(), picture_h()));
                orbiters_ui::paint::fill_convex(&list, &orbiters_ui::paint::squircle_points_corners(pic, [theme::squircle(14.0); 4]), |_| look::TILE);
                list.rect_filled(Rect::from_min_size(pos2(rect.left() + 2.0, pic.bottom() + 11.0), vec2(rect.width() * 0.6, 10.0)), look::radius(5.0), look::TILE);
            }
            Laid::Note(s, y) => {
                let Some(section) = f.sections.get(s) else { continue };
                let words = section.error.clone().unwrap_or_else(|| tr(picker::EMPTY_GROUP).to_string());
                let font = look::font(15.5, weight::MEDIUM);
                list.text(pos2(X, at(y) + NOTE_H / 2.0 - 6.0), Align2::LEFT_CENTER, fit(&list, &words, &font, 940.0), font, look::muted());
            }
            Laid::Tile(..) => {}
        }
    }
}

/// One of the page's items.
pub fn draw_item(p: &Painter, item: &Item, facts: &Facts, hovered: bool, pressed: bool) {
    let f = &facts.avatar_gun;
    let rect = item.rect;
    match item.look {
        Look::AvatarGunTile(i) => {
            let Some(tile) = f.tiles.get(i) else { return };
            let list = p.with_clip_rect(Rect::from_min_max(pos2(0.0, TOP - 6.0), pos2(1024.0, 576.0)));
            let pic = Rect::from_min_size(rect.min, vec2(rect.width(), picture_h()));
            let radii = [theme::squircle(14.0); 4];
            let tint = if pressed { Color32::from_gray(200) } else { Color32::WHITE };
            match tile.picture {
                Some(id) => squircle_picture(&list, pic, radii, id, cover_uv_of(pic.size(), 4.0 / 3.0), tint),
                None => {
                    orbiters_ui::paint::fill_convex(&list, &orbiters_ui::paint::squircle_points_corners(pic, radii), |_| theme::ink_750());
                    widgets::icon(&list, pic.center(), icons::USER, 36.0, look::muted());
                }
            }
            if tile.loaded {
                // The one loaded: ringed, and its mark in its corner.
                list.rect_stroke(pic.expand(1.5), look::radius(15.5), Stroke::new(3.0, look::accent()), egui::StrokeKind::Middle);
                let badge = pos2(pic.right() - 18.0, pic.top() + 18.0);
                list.circle_filled(badge, 13.0, look::accent());
                widgets::icon(&list, badge, icons::CHECK, 15.0, look::on_accent());
            } else if hovered {
                list.rect_stroke(pic.shrink(1.0), look::radius(14.0), Stroke::new(2.0, look::accent()), egui::StrokeKind::Middle);
            }
            let font = look::font(15.5, weight::SEMIBOLD);
            let colour = if tile.loaded { theme::mix(look::accent(), Color32::WHITE, 0.35) } else { look::label() };
            list.text(pos2(rect.left() + 2.0, pic.bottom() + NAME_H / 2.0), Align2::LEFT_CENTER, fit(&list, &tile.name, &font, rect.width() - 4.0), font, colour);
        }
        Look::AvatarGunRetry | Look::AvatarGunConnect => {
            let (icon, words) = if item.look == Look::AvatarGunRetry { (icons::ARROW_CLOCKWISE, tr(picker::RETRY)) } else { (icons::SIGN_IN, tr("Connect VRChat")) };
            let radius = look::radius(rect.height() / 2.0);
            let fill = if pressed { theme::mix(look::accent(), Color32::BLACK, 0.25) } else if hovered { theme::mix(look::accent(), Color32::WHITE, 0.15) } else { look::accent() };
            p.rect_filled(rect, radius, fill);
            let (words, font) = fitted(p, words, 17.0, weight::BOLD, rect.width() - 64.0, 13.5);
            let wide = p.layout_no_wrap(words.clone(), font.clone(), look::on_accent()).size().x;
            let start = rect.center().x - (wide + 30.0) / 2.0;
            widgets::icon(p, pos2(start + 10.0, rect.center().y), icon, 20.0, look::on_accent());
            p.text(pos2(start + 30.0, rect.center().y), Align2::LEFT_CENTER, words, font, look::on_accent());
        }
        _ => {}
    }
}

/// What the caption says of the page's parts.
pub fn caption(action: Action, facts: &Facts) -> Option<(String, String)> {
    let f = &facts.avatar_gun;
    Some(match action {
        Action::AvatarGun => (tr(TITLE).into(), tr(OPEN_WHY).into()),
        Action::AvatarGunPick(i) => {
            let tile = f.tiles.get(i)?;
            let why = if tile.loaded { tr("Your gun is loaded with it.") } else { tr("Press to load your gun with it.") };
            (tile.name.clone(), why.into())
        }
        Action::AvatarGunRetry => (tr(picker::RETRY).into(), tr("Reads your favorites again.").into()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_of(stage: Stage, groups: Vec<picker::Group>) -> Facts {
        let mut store = Store::default();
        store.stand_in(stage, groups);
        let snap = Snapshot::new(Default::default());
        Facts { sheet: Some(Sheet::AvatarGun), avatar_gun: super::facts(&store, &snap, None), ..Facts::default() }
    }

    /// The lists as sections, each its avatars in a grid of six; only what is in view is laid; the loaded one ringed;
    /// the list scrolls to its last row.
    #[test]
    fn the_lists_are_sections_of_avatars() {
        let mut groups = picker::sample(false);
        groups[0].avatars = Some((0..40).map(|n| picker::Pick { id: format!("avtr_{n}"), name: format!("A{n}"), thumbnail: String::new() }).collect());
        let mut store = Store::default();
        store.stand_in(Stage::Ready, groups);
        let mut snap = Snapshot::new(Default::default());
        let mut settings = (*snap.settings).clone();
        settings.avatar_gun.loaded = Some(orbiters_core::settings::LoadedAvatar { id: "avtr_3".into(), name: "A3".into(), thumbnail: String::new() });
        snap.settings = settings.into();
        let f = Facts { sheet: Some(Sheet::AvatarGun), avatar_gun: super::facts(&store, &snap, None), ..Facts::default() };
        assert_eq!(f.avatar_gun.sections.len(), 3);
        assert_eq!(f.avatar_gun.tiles.len(), 43);
        assert!(f.avatar_gun.tiles[3].loaded && f.avatar_gun.tiles.iter().filter(|t| t.loaded).count() == 1);
        let laid = items(&f);
        assert!(laid.iter().all(|i| i.rect.top() < 576.0 && i.rect.bottom() > TOP), "in view only");
        assert!(laid.len() < 20, "{}", laid.len());
        let first = laid.iter().find(|i| i.action == Action::AvatarGunPick(0)).unwrap();
        let second = laid.iter().find(|i| i.action == Action::AvatarGunPick(1)).unwrap();
        assert!(first.rect.right() < second.rect.left() && (first.rect.top() - second.rect.top()).abs() < 0.1, "side by side");
        // A laser's size: wider than 140 of the page's 1024.
        assert!(first.rect.width() > 140.0);
        let end = max_scroll(&f.avatar_gun);
        assert!(end > 0.0);
        let at_end = Facts { gallery_scroll: end, ..f.clone() };
        let last = items(&at_end).into_iter().filter(|i| matches!(i.look, Look::AvatarGunTile(_))).last().unwrap();
        assert_eq!(last.look, Look::AvatarGunTile(42));
        assert!(last.rect.bottom() <= 576.0);
        assert!(row_tops(&f.avatar_gun).len() >= 3);
        // Picked here: ringed at once, before the settings say it.
        let picked = super::facts(&store, &snap, Some("avtr_7"));
        assert!(picked.tiles[7].loaded && !picked.tiles[3].loaded);
        assert_eq!(picked.loaded.as_ref().map(|l| l.0.as_str()), Some("A7"));
    }

    /// As lists arrive the page keeps its places: a list not read keeps a row of grey tiles, so the list above keeps its
    /// height and what arrives grows below it.
    #[test]
    fn lists_arriving_keep_what_is_above_in_place() {
        let loading = facts_of(Stage::Ready, picker::sample(true));
        let read = facts_of(Stage::Ready, picker::sample(false));
        let place = |f: &Facts| items(f).into_iter().filter(|i| i.action == Action::AvatarGunPick(6)).map(|i| i.rect).next();
        assert_eq!(place(&loading), place(&read), "the first list stays where it was");
        assert!(layout(&loading.avatar_gun).0.iter().any(|l| matches!(l, Laid::Grey(_))));
        assert!(!layout(&read.avatar_gun).0.iter().any(|l| matches!(l, Laid::Grey(_))));
        let nothing = facts_of(Stage::Loading, Vec::new());
        assert!(layout(&nothing.avatar_gun).0.iter().filter(|l| matches!(l, Laid::Grey(_))).count() >= COLUMNS * 2, "two lists' places before any is known");
    }

    /// Signed out: Connect VRChat (not when the sign-in is not open to the account); failed: Try again; nothing listed.
    #[test]
    fn every_state_offers_its_way_on() {
        let signed_out = facts_of(Stage::SignedOut { gated: false }, Vec::new());
        assert_eq!(items(&signed_out).iter().map(|i| i.action).collect::<Vec<_>>(), [Action::ConnectVrchat]);
        assert!(items(&facts_of(Stage::SignedOut { gated: true }, Vec::new())).is_empty());
        let failed = facts_of(Stage::Failed(crate::controller::vrchat_data::ListError::RateLimited { seconds: 9 }), Vec::new());
        assert_eq!(items(&failed).iter().map(|i| i.action).collect::<Vec<_>>(), [Action::AvatarGunRetry]);
        let mut one_failed = picker::sample(false);
        one_failed[1].avatars = None;
        one_failed[1].error = Some(crate::controller::vrchat_data::ListError::Failed("x".into()));
        let f = facts_of(Stage::Ready, one_failed);
        assert!(items(&f).iter().any(|i| i.action == Action::AvatarGunRetry));
        let empty = facts_of(Stage::Ready, vec![picker::Group { tag: "avatars1".into(), name: "A".into(), avatars: Some(Vec::new()), error: None }]);
        assert!(items(&empty).is_empty() && max_scroll(&empty.avatar_gun) == 0.0);
    }

    #[test]
    fn its_words_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            for words in [TITLE, OPEN_WHY, picker::SIGN_IN, picker::GATED, picker::EMPTY, picker::EMPTY_GROUP, picker::RETRY] {
                let said = orbiters_i18n::in_language(language, || tr(words).to_string());
                assert!(said != words, "{language:?}: {words}");
            }
        }
    }
}
