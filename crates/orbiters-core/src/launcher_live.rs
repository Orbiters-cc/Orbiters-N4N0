//! A look at real data, by hand only (`--ignored`): counts, never names. LAUNCHER_LIVE_OUT names the JSON file written.
//! With LAUNCHER_LIVE_VRCHAT=1 it also reads with the VRChat session saved on this computer: about ten calls, all reads,
//! then 40 seconds of the pipeline (message types counted).
use crate::launcher::*;
use serde_json::json;

fn summary(state: &LauncherState) -> serde_json::Value {
    let sections: Vec<_> = state.groups.iter().map(|g| json!({"kind": format!("{:?}", g.kind), "named": !g.name.is_empty(), "members": g.members.len()})).collect();
    let count = |f: &dyn Fn(&Friend) -> bool| state.friends.iter().filter(|x| f(x)).count();
    json!({
        "source": match &state.source { Source::Nothing => "nothing", Source::SignedIn { .. } => "signed-in", Source::Database { app: DatabaseApp::Vrcx, .. } => "vrcx", Source::Database { app: DatabaseApp::VrcNext, .. } => "vrcnext" },
        "friends": state.friends.len(),
        "sections": sections,
        "with_icon": count(&|f| !f.look.icon.is_empty()),
        "with_banner": count(&|f| !f.look.banner.is_empty()),
        "with_banner_colour": count(&|f| f.look.banner_colour.is_some()),
        "with_nameplate": count(&|f| !f.look.nameplate.is_empty()),
        "with_status_text": count(&|f| !f.status_text.is_empty()),
        "in_a_world": count(&|f| matches!(f.whereabouts, Whereabouts::World(_))),
        "with_last_seen": count(&|f| f.last_seen.is_some()),
        "with_note": count(&|f| !f.note.is_empty()),
        "in_a_favourite_group": count(&|f| !f.favourite_groups.is_empty()),
        "invites": state.invites.len(),
        "boops": state.boops.len(),
        "me": state.me.is_some(),
    })
}

#[test]
#[ignore = "reads this computer's data; run by hand"]
fn live_counts() {
    let mut out = json!({});
    if let Some(dir) = crate::vrchat_accounts::vrcnext_dir() {
        out["vrcnext"] = crate::launcher_local::vrcnext(&dir).map_or(json!(null), |s| summary(&s));
    }
    if let Some(db) = crate::vrchat_accounts::vrcx_db() {
        out["vrcx"] = crate::launcher_local::vrcx(&db).map_or(json!(null), |s| summary(&s));
    }
    if std::env::var("LAUNCHER_LIVE_VRCHAT").as_deref() == Ok("1") {
        out["vrchat"] = signed_in();
    }
    let path = std::env::var("LAUNCHER_LIVE_OUT").expect("LAUNCHER_LIVE_OUT");
    std::fs::write(path, serde_json::to_string_pretty(&out).unwrap()).unwrap();
}

fn signed_in() -> serde_json::Value {
    use crate::vrchat_api::{Client, Gate, Login};
    use std::sync::Arc;
    use std::time::Duration;
    let mut client = Client::new(Arc::new(Gate::for_duration(Duration::from_secs(600))));
    match client.resume() {
        Ok(Some(Login::SignedIn(_))) => {}
        other => return json!({"resume": format!("{:?}", other.map(|l| l.is_some()))}),
    }
    let mut calls = 1;
    let mut state = LauncherState::default();
    let mut errors = Vec::new();
    for offline in [false, true] {
        for page in 0..10 {
            calls += 1;
            match client.friends_page(offline, page * 100) {
                Ok(items) => {
                    let n = items.len();
                    state.friends.extend(items.iter().filter_map(|v| crate::launcher_feed::friend(v, !offline)));
                    if n < 100 {
                        break;
                    }
                }
                Err(e) => {
                    errors.push(format!("friends: {e:?}"));
                    break;
                }
            }
        }
    }
    calls += 4;
    let groups = client.favourite_groups_value().map(|v| crate::launcher_feed::favourite_groups(&v)).unwrap_or_else(|e| {
        errors.push(format!("groups: {e:?}"));
        Vec::new()
    });
    let members = client.favourite_friends_value().map(|v| crate::launcher_feed::favourites(&v)).unwrap_or_default();
    for f in &mut state.friends {
        f.favourite_groups = members.get(&f.id).cloned().unwrap_or_default();
    }
    let notifications = client.notifications();
    let v2 = client.notifications_v2();
    let v1_types: Vec<String> = notifications.as_ref().map(|n| n.iter().filter_map(|x| x["type"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let v2_types: Vec<String> = v2.as_ref().map(|n| n.iter().filter_map(|x| x["type"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    state.invites = notifications.as_ref().map(|n| n.iter().filter_map(crate::launcher_feed::invite).collect()).unwrap_or_default();
    state.boops = v2.as_ref().map(|n| n.iter().filter_map(crate::launcher_feed::boop).collect()).unwrap_or_default();
    if let Err(e) = &v2 {
        errors.push(format!("notifications v2: {e:?}"));
    }
    crate::launcher_feed::regroup(&mut state, &groups);
    // One friend's profile (the first in a world), to see which fields come.
    let mut profile_fields = json!(null);
    if let Some(f) = state.friends.iter().find(|f| f.presence == Presence::InGame).cloned() {
        calls += 3;
        let public = client.public_profile(&f.id).unwrap_or(serde_json::Value::Null);
        let user = client.user_value(&f.id).unwrap_or(serde_json::Value::Null);
        let mutuals = client.mutuals(&f.id).ok();
        let p = crate::launcher_feed::profile(f, &public, &user, mutuals);
        profile_fields = json!({"public_profile_answered": !public.is_null(), "user_answered": !user.is_null(), "mutuals": mutuals.is_some(),
            "bio": !p.bio.is_empty(), "links": p.links.len(), "languages": p.languages.len(), "badges": p.badges.len(), "date_joined": !p.date_joined.is_empty(), "picture": !p.picture.is_empty(), "banner": !p.friend.look.banner.is_empty()});
    }
    calls += 1;
    let token = client.pipeline_token();
    let mut pipeline = json!({"token": token.is_ok()});
    if let Ok(token) = token {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(40));
            s2.store(true, std::sync::atomic::Ordering::Release);
        });
        let mut types: std::collections::BTreeMap<String, usize> = Default::default();
        let mut applied_changes = 0;
        let mut connected = 0;
        let mut once = Some(token.0);
        let ended = crate::pipeline::run(
            &stop,
            || once.take(),
            |m| {
                let t = serde_json::from_str::<serde_json::Value>(m).ok().and_then(|v| v["type"].as_str().map(str::to_string)).unwrap_or_default();
                *types.entry(t).or_default() += 1;
                if crate::launcher_feed::apply(&mut state, &groups, m, 0).changed {
                    applied_changes += 1;
                }
            },
            |up| connected += up as usize,
        );
        pipeline = json!({"token": true, "connected": connected, "ended": format!("{ended:?}"), "message_types": types, "changes_applied": applied_changes});
    }
    json!({"calls": calls, "errors": errors, "state": summary(&state), "notification_types_v1": v1_types, "notification_types_v2": v2_types, "profile": profile_fields, "pipeline": pipeline})
}
