//! The launcher over this computer's REAL data, by hand only: the scenarios exist only when `ORBITERS_LAUNCHER_REAL`
//! is set, and are run alone (`--only real-launcher,real-launcher-modal,real-launcher-database`). The pictures they
//! make show real people: they stay on this computer, never in the repository or anything published.
//!
//! `real-launcher*`: the VRChat session saved on this computer, read once per run (the controller's own read, the
//! worlds and decorations of the people in view, one profile, up to [`PICTURES`] pictures): reads only.
//! `real-launcher-database`: VRCNext's or VRCX's copy, as without the sign-in.
use super::{Page, Scenario};
use crate::app::App;
use crate::controller::launcher::{fill_worlds, read_signed_in, unnamed_worlds};
use egui::{pos2, vec2, ColorImage, Rect, TextureOptions};
use orbiters_core::launcher::*;
use orbiters_core::vrchat_api::{Client, Gate, Login};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const PICTURES: usize = 40;
const NAMES: [&str; 3] = ["real-launcher", "real-launcher-modal", "real-launcher-database"];

pub(super) fn append(scenarios: &mut Vec<Scenario>) {
    if std::env::var_os("ORBITERS_LAUNCHER_REAL").is_none() {
        return;
    }
    let base = scenarios[0].snapshot.clone();
    for name in NAMES {
        scenarios.push(Scenario {
            name,
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
    }
}

struct Real {
    signed: Option<(LauncherState, Vec<(String, ColorImage)>)>,
    local: LauncherState,
    log: Vec<String>,
}

fn decode(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.thumbnail(480, 480).to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
}

fn read() -> &'static Real {
    static REAL: OnceLock<Real> = OnceLock::new();
    REAL.get_or_init(|| {
        let mut log = Vec::new();
        let local = orbiters_core::launcher_local::read();
        let signed = (|| {
            let mut client = Client::new(Arc::new(Gate::for_duration(Duration::from_secs(900))));
            match client.resume() {
                Ok(Some(Login::SignedIn(_))) => {}
                other => {
                    log.push(format!("no session: {:?}", other.map(|l| l.is_some())));
                    return None;
                }
            }
            let client = Mutex::new(client);
            let (mut state, _, worn) = match read_signed_in(&client) {
                Ok(read) => read,
                Err(e) => {
                    log.push(format!("read: {e:?}"));
                    return None;
                }
            };
            // As the controller does over its first minute: the worlds and decorations of those in view.
            let mut worlds = HashMap::new();
            for id in unnamed_worlds(&state).into_iter().take(8) {
                let w = client.lock().unwrap().world(&id).ok().map(|w| (w.name, if w.thumbnail_image_url.is_empty() { w.image_url } else { w.thumbnail_image_url }));
                worlds.insert(id, w);
            }
            fill_worlds(&mut state, &worlds);
            let in_view: Vec<String> = state.groups.iter().flat_map(|g| g.members.iter().cloned()).take(24).collect();
            let mut templates: HashMap<String, Option<String>> = HashMap::new();
            for id in &in_view {
                let Some(d) = worn.get(id) else { continue };
                for t in [&d.icon_frame, &d.nameplate].into_iter().flatten() {
                    if templates.len() < 8 && !templates.contains_key(t) {
                        templates.insert(t.clone(), client.lock().unwrap().inventory_template_picture(t).ok());
                    }
                }
            }
            for f in &mut state.friends {
                if let Some(d) = worn.get(&f.id) {
                    let get = |t: &Option<String>| t.as_ref().and_then(|t| templates.get(t).cloned().flatten()).unwrap_or_default();
                    f.look.icon_frame = get(&d.icon_frame);
                    f.look.nameplate = get(&d.nameplate);
                }
            }
            // One profile: the first in a world shown, else the first in game.
            let pick = in_view
                .iter()
                .filter_map(|id| state.friend(id))
                .find(|f| matches!(f.whereabouts, Whereabouts::World(_)))
                .or_else(|| in_view.iter().filter_map(|id| state.friend(id)).find(|f| f.presence == Presence::InGame))
                .cloned();
            if let Some(f) = pick {
                let public = client.lock().unwrap().public_profile(&f.id).unwrap_or(serde_json::Value::Null);
                let user = client.lock().unwrap().user_value(&f.id).unwrap_or(serde_json::Value::Null);
                let mutuals = client.lock().unwrap().mutuals(&f.id).ok();
                state.profile = Some(orbiters_core::launcher_feed::profile(f, &public, &user, mutuals));
            }
            // The pictures of what is in view, in the list's order.
            let mut urls: Vec<String> = Vec::new();
            let add = |urls: &mut Vec<String>, f: &Friend| {
                urls.extend([f.look.icon.clone(), f.look.nameplate.clone(), f.look.banner.clone(), f.look.icon_frame.clone()]);
                if let Whereabouts::World(p) = &f.whereabouts {
                    urls.push(p.world_picture.clone());
                }
            };
            if let Some(p) = &state.profile {
                add(&mut urls, &p.friend);
                urls.push(p.picture.clone());
                urls.extend(p.badges.iter().map(|b| b.picture.clone()));
            }
            for f in state.me.iter().chain(in_view.iter().filter_map(|id| state.friend(id))) {
                add(&mut urls, f);
            }
            let mut seen = std::collections::HashSet::new();
            urls.retain(|u| u.starts_with("https://") && seen.insert(u.clone()));
            urls.truncate(PICTURES);
            let mut pictures = Vec::new();
            for url in urls {
                match client.lock().unwrap().image(&url) {
                    Ok(bytes) => match decode(&bytes) {
                        Some(img) => pictures.push((url, img)),
                        None => log.push("picture did not decode".into()),
                    },
                    Err(e) => log.push(format!("picture: {e:?}")),
                }
            }
            state.possible.play = true;
            Some((state, pictures))
        })();
        Real { signed, local, log }
    })
}

/// What happens on frame `f` of a real scenario.
pub(super) fn stage(ctx: &egui::Context, app: &mut App, name: &str, f: u32) {
    if !name.starts_with("real-launcher") || f != 0 {
        return;
    }
    let real = read();
    let out = std::env::var("ORBITERS_LAUNCHER_REAL").unwrap_or_default();
    if !out.is_empty() {
        let _ = std::fs::write(std::path::Path::new(&out).join("real-log.txt"), real.log.join("\n"));
    }
    let state = app.home_mut().launcher_mut();
    if name == "real-launcher-database" {
        let mut data = real.local.clone();
        data.profile = None;
        state.set_data(Arc::new(data));
        return;
    }
    let Some((data, pictures)) = &real.signed else { return };
    for (url, img) in pictures {
        state.provide_picture(url, ctx.load_texture(url.clone(), img.clone(), TextureOptions::LINEAR));
    }
    let mut data = data.clone();
    let profile = data.profile.take();
    if name == "real-launcher-modal" {
        if let Some(p) = profile {
            let id = p.friend.id.clone();
            data.profile = Some(p);
            state.set_data(Arc::new(data));
            state.open_profile(ctx, &id, Rect::from_min_size(pos2(1100.0, 300.0), vec2(300.0, 56.0)));
            return;
        }
    }
    state.set_data(Arc::new(data));
}
