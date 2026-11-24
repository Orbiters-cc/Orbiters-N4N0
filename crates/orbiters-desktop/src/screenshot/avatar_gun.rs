//! `avatar-gun-*` scenes: the avatar gun on N4N0's page (its card, its picker grown out of Choose avatar, in each of the
//! picker's states) and the hit player's card in the window's corner. Made-up lists, avatars, pictures and names only.

use super::{Page, Scenario};
use crate::app::App;
use crate::avatar_gun_picker::{self as picker, Stage};
use crate::controller::avatar_gun::{Card, CardKind};

fn groups() -> Vec<picker::Group> {
    let mut groups = picker::sample(false);
    groups[1].avatars = Some(
        (0..9)
            .map(|n| picker::Pick { id: format!("avtr_00000000-0000-4000-8000-{:012}", 20 + n), name: ["Rubber Duck", "Walking Toaster", "Moon Rabbit", "Cloud Whale", "Pixel Knight", "Velvet Moth", "Cactus Pal", "Paper Crane", "Neon Jelly"][n].into(), thumbnail: String::new() }) // i18n: not shown (made-up avatars)
            .collect(),
    );
    groups
}

/// What a scene's picker shows, and whether it is open.
fn staged(name: &str) -> Option<(Stage, Vec<picker::Group>, bool)> {
    Some(match name {
        "avatar-gun-card" | "avatar-gun-ask" | "avatar-gun-changed" => (Stage::Ready, groups(), false),
        "avatar-gun-picker" => (Stage::Ready, groups(), true),
        "avatar-gun-picker-arriving" => (Stage::Ready, picker::sample(true), true),
        "avatar-gun-picker-loading" => (Stage::Loading, Vec::new(), true),
        "avatar-gun-picker-signed-out" => (Stage::SignedOut { gated: false }, Vec::new(), true),
        "avatar-gun-picker-gated" => (Stage::SignedOut { gated: true }, Vec::new(), true),
        "avatar-gun-picker-failed" => (Stage::Failed(crate::controller::vrchat_data::ListError::RateLimited { seconds: 12 }), Vec::new(), true),
        "avatar-gun-picker-empty" => (Stage::Ready, vec![picker::Group { tag: "avatars1".into(), name: "Favorites".into(), avatars: Some(Vec::new()), error: None }], true), // i18n: not shown (a made-up list)
        _ => return None,
    })
}

/// The scene's first frame: the made-up lists and pictures, the picker opened.
pub(super) fn stage(ctx: &egui::Context, app: &mut App, name: &str) {
    let Some((stage, groups, open)) = staged(name) else { return };
    let origin = open.then(|| egui::Rect::from_min_size(ctx.content_rect().right_top() + egui::vec2(-260.0, 360.0), egui::vec2(140.0, 32.0)));
    app.n4n0_mut().gun_mut().stage(ctx, stage, groups, &super::orbit::avatar_portrait, origin);
}

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    let mut snapshot = scenarios[0].snapshot.clone();
    snapshot.account.state = crate::state::AccountState::SignedIn { username: "SampleMember".into() };
    snapshot.vrchat_login.allowed = true;
    snapshot.vrchat_login.user = serde_json::from_value(serde_json::json!({"id": "usr_00000000-0000-4000-8000-000000000009", "displayName": "SampleMember"})).ok();
    snapshot.settings.avatar_gun.loaded = Some(orbiters_core::settings::LoadedAvatar { id: "avtr_00000000-0000-4000-8000-000000000001".into(), name: "Paper Robot".into(), thumbnail: String::new() });
    snapshot.avatar_gun.gun = true;
    snapshot.avatar_gun.receiver = true;
    snapshot.avatar_gun.loaded = true;
    snapshot.avatar_gun.stream = true;
    let scene = |name: &'static str, snapshot: crate::state::Snapshot, scroll: f32| Scenario {
        name,
        page: Page::N4n0,
        snapshot,
        frames: 90,
        splash: false,
        open_detail: false,
        toasts: vec![],
        pointer: None,
        scroll,
        world: None,
        home: None,
        library: None,
    };
    scenarios.push(scene("avatar-gun-card", snapshot.clone(), 420.0));
    for name in ["avatar-gun-picker", "avatar-gun-picker-arriving", "avatar-gun-picker-loading", "avatar-gun-picker-failed", "avatar-gun-picker-empty"] {
        scenarios.push(scene(name, snapshot.clone(), 0.0));
    }
    let mut signed_out = snapshot.clone();
    signed_out.vrchat_login.user = None;
    scenarios.push(scene("avatar-gun-picker-signed-out", signed_out.clone(), 0.0));
    let mut gated = signed_out;
    gated.vrchat_login.allowed = false;
    scenarios.push(scene("avatar-gun-picker-gated", gated, 0.0));
    // The hit player's card in the window's corner: asked, then changed (its Undo). Its time far ahead: it stays.
    let ten_minutes = 600_000;
    let until_ms = crate::controller::unix_ms() + ten_minutes;
    let card = |kind: CardKind| Card { id: 1, kind, from: "Kite".into(), avatar_id: "avtr_00000000-0000-4000-8000-000000000003".into(), avatar: "Starlight Witch".into(), thumbnail: String::new(), until_ms, for_ms: 2 * ten_minutes }; // i18n: not shown (made-up names)
    let mut ask = snapshot.clone();
    ask.avatar_gun.card = Some(card(CardKind::Ask));
    scenarios.push(scene("avatar-gun-ask", ask, 0.0));
    let mut changed = snapshot;
    changed.avatar_gun.card = Some(card(CardKind::Changed));
    scenarios.push(scene("avatar-gun-changed", changed, 0.0));
}
