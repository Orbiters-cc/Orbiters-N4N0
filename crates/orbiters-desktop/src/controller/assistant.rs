//! N4N0's assistant asking the app for its own features (REQUEST20 6, `orbiters_n4n0_core::intents`): each intent here is
//! the controller call the window, the overlay or Home already make for the same press (named beside each), never a
//! second way of doing it. What reaches VRChat from the account goes as Home sends it: bound to the VRChat account on
//! show, checked again before it goes. The gate (risk classes, the owner's yes) ran before any of this.

use super::{Controller, Shared};
use orbiters_core::launcher::{Action as Home, LaunchMode, Place, Presence, ProfileEdit, Source, Whereabouts};
use orbiters_n4n0_core::context::{data, NAME_CHARS};
use orbiters_n4n0_core::intents::{Facts, Intent, Page, SettingKey};
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Opens a page of the window (the service's `ShowPage`: the window comes up on it, started when it is closed), set by
/// the service when it starts. Tests have none.
type PageOpener = Box<dyn Fn(String) + Send + Sync>;
static PAGE_OPENER: OnceLock<PageOpener> = OnceLock::new();

/// The service's way of opening a page of the window, as its `ShowPage` does.
pub fn set_page_opener(open: PageOpener) {
    let _ = PAGE_OPENER.set(open);
}

/// Opens `page` of the window (its name as the window knows it: `page:Home`).
fn show_page(page: &str) -> Result<(), String> {
    let open = PAGE_OPENER.get().ok_or("The Orbiters window cannot be opened from here.")?; // i18n: not shown
    open(page.to_string());
    Ok(())
}

/// The one friend of `friends` (id, name) the owner named; several alike is a question for the owner.
fn pick<'a>(said: &str, friends: impl Iterator<Item = (&'a str, &'a str)>) -> Result<String, String> {
    let fold = |t: &str| -> String {
        t.chars()
            .flat_map(|c| match orbiters_contracts::n4n0::unaccented(c) {
                Some(base) => base.to_lowercase().chars().collect::<Vec<_>>(),
                None => c.to_lowercase().collect(),
            })
            .filter(|c| c.is_alphanumeric())
            .collect()
    };
    let said_f = fold(said);
    if said_f.is_empty() {
        return Err("No name given: ask the owner who.".into()); // i18n: not shown
    }
    let scored: Vec<(u8, &str, &str)> = friends
        .map(|(id, name)| {
            let n = fold(name);
            (if n == said_f { 2 } else if n.contains(&said_f) { 1 } else { 0 }, id, name)
        })
        .filter(|s| s.0 > 0)
        .collect();
    let best = scored.iter().map(|s| s.0).max().ok_or_else(|| format!("No friend called {}: ask the owner who they meant.", orbiters_n4n0_core::context::quoted(said, NAME_CHARS)))?; // i18n: not shown
    let top: Vec<_> = scored.iter().filter(|s| s.0 == best).collect();
    if top.len() > 1 {
        let names: Vec<String> = top.iter().take(5).map(|s| orbiters_n4n0_core::context::quoted(s.2, NAME_CHARS)).collect();
        return Err(format!("Several friends match: {}. Ask the owner which one.", names.join(", "))); // i18n: not shown
    }
    Ok(top[0].1.to_string())
}

impl Controller {
    /// What the app can do now, for the intents' needs (N4N0 adds the prop's).
    pub(super) fn assistant_facts(&self) -> Facts {
        let s = self.view();
        Facts {
            vrchat: s.vrchat.running,
            signed_in: self.account_api().is_some(),
            vrchat_account: matches!(s.launcher.source, Source::SignedIn { .. }),
            calls: matches!(s.n4n0.calls.link, orbiters_n4n0::status::CallLink::Linked { .. }),
            n4n0_prop: false,
            can_drop: false,
            computer: true,
            actions: self.assistant_actions(),
        }
    }

    /// The app's own state for the assistant's questions, keyed as `intents::About` (others' text cut short as data).
    pub(super) fn assistant_state(&self) -> Value {
        let s = self.view();
        let (in_game, website) = s.launcher.friends.iter().fold((0, 0), |(g, w), f| match f.presence {
            Presence::InGame => (g + 1, w),
            Presence::Website => (g, w + 1),
            Presence::Offline => (g, w),
        });
        let recording = match s.capture.state {
            crate::state::CaptureState::Idle => "idle",
            crate::state::CaptureState::Starting => "starting",
            crate::state::CaptureState::Recording => "recording",
            crate::state::CaptureState::Paused => "paused",
            crate::state::CaptureState::Stopping => "stopping",
            crate::state::CaptureState::Importing => "importing",
        };
        json!({
            "explorer_points": s.explorer.points.as_ref().map(|e| json!({ "points": e.points, "title": e.title.as_ref().map(|t| t.name.clone()), "next": e.next.as_ref().map(|n| json!({ "title": n.name, "points_missing": n.remaining })) })),
            "friends": { "in_game": in_game, "on_website": website, "vrchat_account_connected": matches!(s.launcher.source, Source::SignedIn { .. }) },
            "recording": { "state": if s.capture.photo_3d && recording == "recording" { "3d_picture" } else { recording } },
            "translation_agreement": { "agreed": s.settings.translate.orbiters_agreed || s.settings.translate.orbiters_agreed_window },
            "vrchat": { "running": s.vrchat.running, "vr": s.steamvr.running && !s.n4n0.desktop_mode },
        })
    }

    /// Home's action, bound to the VRChat account on show as Home binds its presses (`pages::launcher::as_shown`).
    fn as_shown(&self, action: Home) -> Home {
        match &self.view().launcher.source {
            Source::SignedIn { user_id, .. } if !user_id.is_empty() => Home::As { account: user_id.clone(), action: Box::new(action) },
            _ => action,
        }
    }

    /// VR when SteamVR runs and VRChat is not in desktop mode, as Play's own choice reads it.
    fn launch_mode(&self) -> LaunchMode {
        let s = self.view();
        if s.steamvr.running && !s.n4n0.desktop_mode {
            LaunchMode::Vr
        } else {
            LaunchMode::Desktop
        }
    }

    /// A new Friends+ instance of `world` made for the signed-in VRChat account, then VRChat started (or sent) into it:
    /// Home's Where's `make_and_go` (REQUEST20 2), the region the person played in last.
    fn new_instance_and_play(self: &Shared, world: &str, name: &str, vr: bool) -> Result<Value, String> {
        use super::vrchat_data::{Request, Response, Scope};
        use orbiters_core::vrchat_api::{CreateAccess, InstanceRegion};
        let s = self.view();
        let Source::SignedIn { user_id: me, .. } = &s.launcher.source else {
            return Err("A new Friends+ instance is made for the owner's VRChat account: none is connected (Settings).".into()); // i18n: not shown
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let history = orbiters_core::visited::History::load(&orbiters_core::visited::history_file());
        let region = crate::overlay::worlds::panel::go::region_of(crate::pages::launcher::destinations::HistoryView::of(&history, now).region()).unwrap_or(InstanceRegion::Us);
        let made = match self.vrchat_data(Scope::of(&s), Request::CreateInstance { world: world.into(), access: CreateAccess::FriendsPlus, region }) {
            Ok(Response::CreatedInstance(made)) => made,
            Ok(Response::WorldError(e)) => return Err(format!("VRChat did not make the instance: {e:?}")), // i18n: not shown
            Ok(_) => return Err("VRChat answered something unexpected.".into()), // i18n: not shown
            Err(e) => return Err(format!("VRChat did not make the instance: {e}")), // i18n: not shown
        };
        let place = Place { world_id: made.world.clone(), world_name: name.into(), world_picture: String::new(), instance_id: made.id.clone(), access: made.access, region: made.region.clone() };
        let mode = if vr { LaunchMode::Vr } else { LaunchMode::Desktop };
        self.launcher_action(Home::As { account: me.clone(), action: Box::new(Home::Play { mode, destination: Some(place) }) });
        Ok(json!({ "done": true, "instance": "new Friends+" }))
    }

    /// The friend the owner named, among the VRChat account's friends.
    fn friend_id(&self, said: &str) -> Result<String, String> {
        let s = self.view();
        pick(said, s.launcher.friends.iter().map(|f| (f.id.as_str(), f.name.as_str())))
    }

    /// Runs one of the app's intents, as its page would (the gate already ran). `item`: what an index named in the list
    /// this answered before (its `ref`).
    pub(super) fn assistant_intent(self: &Shared, intent: &Intent, item: Option<&Value>) -> Result<Value, String> {
        let done = || Ok(json!({ "done": true }));
        let reference = || item.and_then(|i| i.get("ref")).and_then(Value::as_str).map(str::to_string).ok_or_else(|| "No such item in the last list.".to_string()); // i18n: not shown
        match intent {
            // The overlay's walk boost slider (`ToService::WalkBoost`).
            Intent::WalkBoost { strength, step } => {
                let now = self.view().settings.walk_boost.strength;
                let to = strength.unwrap_or(now + 0.1 * *step as f32).clamp(0.0, 1.0);
                self.set_walk_boost(to, false);
                Ok(json!({ "done": true, "strength": to }))
            }
            // Stop beside the camera (`ToService::StopCapture`).
            Intent::StopRecording => {
                let s = self.view();
                if !matches!(s.capture.state, crate::state::CaptureState::Starting | crate::state::CaptureState::Recording | crate::state::CaptureState::Paused) {
                    return Err("Nothing is recording.".into()); // i18n: not shown
                }
                self.stop_capture(super::StopOf::Person).map_err(|e| e.to_string())?;
                done()
            }
            // The camera's QR code button (`ToService::CameraQr`).
            Intent::CameraQr { on } => {
                self.camera_qr(*on);
                done()
            }
            // The Videos page's list (the file the service keeps).
            Intent::VideoHistory => {
                let history = orbiters_core::video_history::History::load(&orbiters_core::video_history::history_file());
                let mut videos = history.videos.clone();
                videos.sort_by(|a, b| b.last.cmp(&a.last));
                let items: Vec<Value> = videos
                    .iter()
                    .take(12)
                    .map(|v| json!({ "ref": v.url, "title": v.title.as_deref().map(|t| data(t, NAME_CHARS)).unwrap_or_default(), "channel": v.channel.as_deref().map(|c| data(c, NAME_CHARS)), "plays": v.plays, "last_unix": v.last }))
                    .collect();
                Ok(json!({ "items": items }))
            }
            // The 3D worlds sheet's search (the Map explorer's, `Api::find_worlds`).
            Intent::WorldSearch { query } => {
                let api = orbiters_core::account::Api::new(orbiters_core::account::Server::current());
                let found = api.find_worlds(&orbiters_core::maps::WorldList::Search(query.clone()), 0).map_err(|e| format!("Worlds could not be searched: {e}"))?; // i18n: not shown
                let items: Vec<Value> = found
                    .items
                    .iter()
                    .map(|f| json!({ "ref": f.world.world_id, "name": data(&f.world.name, NAME_CHARS), "author": f.world.author_name.as_deref().map(|a| data(a, NAME_CHARS)), "people_now": f.occupants, "has_3d_preview": f.preview }))
                    .collect();
                Ok(json!({ "items": items }))
            }
            // Previous instances (N4N0's menu's list, `invites::read_rows`).
            Intent::Instances => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let items: Vec<Value> = orbiters_n4n0_core::invites::read_rows()
                    .iter()
                    .map(|r| json!({ "ref": r.location, "world": data(&r.world, NAME_CHARS), "type": r.kind(), "region": r.region(), "minutes_ago": now.saturating_sub(r.at) / 60 }))
                    .collect();
                Ok(json!({ "items": items }))
            }
            // Play with a destination, as Home's place chips and the Instances sheet's Go do.
            Intent::GoToInstance { .. } => {
                let location = reference()?;
                let (world_id, instance_id) = location.split_once(':').ok_or("Not a VRChat location.")?; // i18n: not shown
                let (access, region) = orbiters_core::launcher_feed::access(instance_id);
                let place = Place { world_id: world_id.into(), world_name: String::new(), world_picture: String::new(), instance_id: instance_id.into(), access, region };
                self.launcher_action(Home::Play { mode: self.launch_mode(), destination: Some(place) });
                done()
            }
            // Home's Where: the places VRChat can start at, as its cards offer them (REQUEST20 2).
            Intent::Places => {
                let s = self.view();
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let mut items = vec![json!({ "ref": "home", "go": "home", "name": "VRChat's home world" })]; // i18n: not shown
                for i in s.launcher.invites.iter().filter(|i| i.kind == orbiters_core::launcher::InviteKind::Invite && i.place.is_some()).take(3) {
                    items.push(json!({ "ref": i.id, "go": "invite", "name": i.place.as_ref().map(|p| data(&p.world_name, NAME_CHARS)), "from": data(&i.from_name, NAME_CHARS) }));
                }
                for r in orbiters_n4n0_core::invites::read_rows().iter().take(3) {
                    items.push(json!({ "ref": r.location, "go": "instance", "name": data(&r.world, NAME_CHARS), "type": r.kind() }));
                }
                let history = orbiters_core::visited::History::load(&orbiters_core::visited::history_file());
                for f in crate::pages::launcher::destinations::frequent(&history, now).iter().take(3) {
                    items.push(json!({ "ref": f.world_id, "go": "new", "name": data(&f.name, NAME_CHARS), "visits_lately": f.visits, "instance": "a new Friends+ one" })); // i18n: not shown
                }
                Ok(json!({ "items": items }))
            }
            // Home's Play, with or without a place of Where.
            Intent::Play { vr, index } => {
                let mode = if *vr { LaunchMode::Vr } else { LaunchMode::Desktop };
                let go = item.and_then(|i| i.get("go")).and_then(Value::as_str).unwrap_or("home");
                match (index, go) {
                    (None, _) | (Some(_), "home") => self.launcher_action(Home::Play { mode, destination: None }),
                    (Some(_), "invite") => self.launcher_action(self.as_shown(Home::AcceptInvite { invite: reference()?, mode })),
                    (Some(_), "instance") => {
                        let location = reference()?;
                        let (world_id, instance_id) = location.split_once(':').ok_or("Not a VRChat location.")?; // i18n: not shown
                        let (access, region) = orbiters_core::launcher_feed::access(instance_id);
                        let place = Place { world_id: world_id.into(), world_name: String::new(), world_picture: String::new(), instance_id: instance_id.into(), access, region };
                        self.launcher_action(Home::Play { mode, destination: Some(place) });
                    }
                    (Some(_), "new") => {
                        let name = item.and_then(|i| i.get("name")).and_then(Value::as_str).unwrap_or_default().to_string();
                        return self.new_instance_and_play(&reference()?, &name, *vr);
                    }
                    _ => return Err("Not a place of the last places list.".into()), // i18n: not shown
                }
                done()
            }
            // A searched world, as Where's new instance (REQUEST20 2).
            Intent::GoToWorld { .. } => {
                let name = item.and_then(|i| i.get("name")).and_then(Value::as_str).unwrap_or_default().to_string();
                let vr = self.launch_mode() == LaunchMode::Vr;
                self.new_instance_and_play(&reference()?, &name, vr)
            }
            // Home's friends list, by its sections (REQUEST20 3).
            Intent::FriendsOnline => {
                let s = self.view();
                let items: Vec<Value> = s
                    .launcher
                    .friends
                    .iter()
                    .filter(|f| f.presence != Presence::Offline)
                    .take(30)
                    .map(|f| {
                        let place = match &f.whereabouts {
                            Whereabouts::World(p) => data(&p.world_name, NAME_CHARS),
                            Whereabouts::Private => "a private instance".into(), // i18n: not shown
                            Whereabouts::Traveling => "between worlds".into(), // i18n: not shown
                            Whereabouts::Website => "on the website".into(), // i18n: not shown
                            _ => String::new(),
                        };
                        json!({ "name": data(&f.name, NAME_CHARS), "section": if f.presence == Presence::InGame { "in the game" } else { "on the website" }, "status": format!("{:?}", f.status), "status_text": data(&f.status_text, NAME_CHARS), "where": place }) // i18n: not shown
                    })
                    .collect();
                if items.is_empty() && !matches!(s.launcher.source, Source::SignedIn { .. } | Source::Database { .. }) {
                    return Err("No friends list here: the owner connects a VRChat account in Settings.".into()); // i18n: not shown
                }
                let offline = s.launcher.friends.iter().filter(|f| f.presence == Presence::Offline).count();
                Ok(json!({ "friends": items, "offline_count": offline }))
            }
            // Home's Boop (back), Request invite, profile.
            Intent::Boop { friend } => {
                let to = match friend {
                    Some(name) => self.friend_id(name)?,
                    None => self.view().launcher.boops.first().map(|b| b.from_id.clone()).ok_or("Nobody booped the owner lately.")?, // i18n: not shown
                };
                self.launcher_action(self.as_shown(Home::Boop { to, emoji: None }));
                done()
            }
            Intent::RequestInvite { friend } => {
                let to = self.friend_id(friend)?;
                self.launcher_action(self.as_shown(Home::RequestInvite { to }));
                done()
            }
            Intent::FriendProfile { friend } => {
                let id = self.friend_id(friend)?;
                self.launcher_action(self.as_shown(Home::OpenProfile { id }));
                show_page("page:Home")?;
                done()
            }
            Intent::Invites => {
                let s = self.view();
                let items: Vec<Value> = s
                    .launcher
                    .invites
                    .iter()
                    .take(12)
                    .map(|i| json!({ "ref": i.id, "from": data(&i.from_name, NAME_CHARS), "kind": format!("{:?}", i.kind), "world": i.place.as_ref().map(|p| data(&p.world_name, NAME_CHARS)), "message": data(&i.message, 120) }))
                    .collect();
                Ok(json!({ "items": items }))
            }
            Intent::AcceptInvite { .. } => {
                let invite = reference()?;
                self.launcher_action(self.as_shown(Home::AcceptInvite { invite, mode: self.launch_mode() }));
                done()
            }
            // Home's own profile card (REQUEST15 7b).
            Intent::SetStatus { status } => {
                self.launcher_action(self.as_shown(Home::EditProfile(ProfileEdit::Status(status.launcher()))));
                done()
            }
            Intent::SetStatusText { text } => {
                self.launcher_action(self.as_shown(Home::EditProfile(ProfileEdit::StatusText(text.chars().take(orbiters_core::launcher::STATUS_TEXT_MAX).collect()))));
                done()
            }
            Intent::SetBio { text } => {
                self.launcher_action(self.as_shown(Home::EditProfile(ProfileEdit::Bio(text.chars().take(orbiters_core::launcher::BIO_MAX).collect()))));
                done()
            }
            Intent::MyStatus => {
                let s = self.view();
                let me = s.launcher.me.as_ref();
                Ok(json!({
                    "status": me.map(|m| format!("{:?}", m.status)),
                    "status_text": me.map(|m| data(&m.status_text, NAME_CHARS)),
                    "bio": s.launcher.mine.as_ref().map(|m| data(&m.bio, 300)),
                }))
            }
            // Translation's sheet: On, and Into (`ToService::PatchSettings`, the same fields it patches).
            Intent::Translation { on } => {
                let s = self.view();
                let desktop = s.n4n0.desktop_mode;
                let t = &s.settings.translate;
                let agreed = if desktop { t.orbiters_agreed_window } else { t.orbiters_agreed };
                if *on && t.provider == orbiters_core::settings::TranslateProvider::Orbiters && !agreed {
                    return Err("Translation through Orbiters needs the owner's agreement first, on the Translation page (open_page translation): it is never given by voice.".into()); // i18n: not shown
                }
                let patch = if desktop { json!({ "translate": { "enabled": on, "window_on": on } }) } else { json!({ "translate": { "enabled": on } }) };
                self.patch_settings(&patch);
                done()
            }
            Intent::TranslateInto { language } => {
                self.patch_settings(&json!({ "translate": { "target": language } }));
                done()
            }
            // The menus' Settings sheet and the app's language picker.
            Intent::Setting { key, value } => {
                let patch = match key {
                    SettingKey::MenuTheme => json!({ "orbit": { "theme": value } }),
                    SettingKey::ReduceMotion => json!({ "reduce_motion": value == "on" }),
                    SettingKey::AppLanguage => crate::language::patch(orbiters_i18n::normalize(value)),
                };
                self.patch_settings(&patch);
                done()
            }
            // A page of the window, as the overlay's Open in app does (`ToService::ShowPage`).
            Intent::OpenPage { page } => {
                let name = page.window_name().ok_or("Not a page of the window.")?; // i18n: not shown
                show_page(&format!("page:{name}"))?;
                done()
            }
            other => Err(format!("Orbiters - N4N0 has no way to do {} here.", other.name())), // i18n: not shown
        }
    }
}

/// Whether `page` is one the window opens (for the service's hook).
pub fn window_page(page: &str) -> Option<&'static str> {
    page.strip_prefix("page:").and_then(|name| Page::ALL.into_iter().filter_map(Page::window_name).find(|n| *n == name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_friend_is_named_once_or_asked_about() {
        let friends = [("usr_1", "Sam"), ("usr_2", "Samuel"), ("usr_3", "Kira"), ("usr_4", "Kíra Two")];
        assert_eq!(pick("sam", friends.iter().copied()), Ok("usr_1".into()), "the exact name first");
        assert!(pick("kira", friends.iter().copied()) == Ok("usr_3".into()));
        assert!(pick("ki", friends.iter().copied()).unwrap_err().contains("Several"), "two alike: the owner is asked");
        assert!(pick("nobody", friends.iter().copied()).unwrap_err().contains("No friend"));
        assert!(pick("", friends.iter().copied()).is_err());
        assert_eq!(window_page("page:Home"), Some("Home"));
        assert_eq!(window_page("page:Nowhere"), None);
    }
}
