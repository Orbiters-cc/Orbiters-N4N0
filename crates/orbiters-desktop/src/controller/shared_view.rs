//! The shared 3D view in the tray service (contracts `Docs/n4n0-shared-view.md`, stage 1): looking at the same capture
//! together with a friend in the same VRChat instance, each copy in front of its own viewer.
//!
//! While N4N0 is on, signed in, and VRChat is in an instance, a shares presence goes to Orbiters every 30 s (and at
//! once when the people here change). It names only the friends this computer's VRChat log lists in this instance, as
//! hashes. It needs neither Calls nor the N4N0 avatar asset. Orbiters answers the friends who name this player too:
//! those the small view can be shown to.
//!
//! The events come on the calls' event stream, which is kept open for this even with Calls off. The rules are
//! [`SharedRuntime`]'s: its methods change the state and return what to do ([`Acts`]), which
//! [`Controller::shared_act`] does. Nothing a share's peer sends does anything but change that share's view. An
//! offer opens nothing before its Show. A state applies only to the share accepted, only from a newer revision and
//! term ([`orbiters_core::shared_view`]). The capture is downloaded signed in, never from a public address.

use super::rest::Part;
use super::{Controller, NoticeKind, Shared};
use crate::ipc::SharedViewCommand;
use crate::state::{CallContact, ShareOffer, ShareOut, ShareOutPhase, ShareStart, ShareWatching, SharedViewStatus};
use orbiters_core::account::{Api, ApiError};
use orbiters_core::shared_view::{self as sv, Applied, CaptureRef, Offer, Sending, ViewState, Watching};
use orbiters_i18n::tr;
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{mpsc, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The presence goes again after this, or at once when the friends here change.
const PRESENCE_EVERY: Duration = Duration::from_secs(30);
/// After a refusal (not linked, the server without the routes), the presence waits this long.
const PRESENCE_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// The contract's most friends here.
const HERE_MOST: usize = 80;
/// Offers waiting at once: more are declined.
const OFFERS_MOST: usize = 3;
/// How long an offer to show is up here at most (Orbiters ends it at 30 s).
const OFFER_FOR_MS: u64 = 30_000;
/// The largest capture file taken (the 3D worlds' own limit).
const FILE_MOST: u64 = 72 << 20;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A request to Orbiters, sent in order by one thread.
#[derive(Debug, PartialEq)]
enum Request {
    Presence { vrchat_id: String, here: Vec<String> },
    Withdraw,
    Offer { number: u64, contact: String, capture: CaptureRef, state: ViewState },
    Answer { number: u64, share: String, accept: bool },
    State { share: String, state: ViewState },
    End { share: String },
    /// The capture of the view `number` looks at: its start view and file, signed in.
    Fetch { number: u64, capture: CaptureRef },
}

/// What a change asks for.
#[derive(Debug, Default)]
struct Acts {
    requests: Vec<Request>,
    notice: Option<(NoticeKind, String)>,
    publish: bool,
}

#[derive(Debug)]
struct Out {
    number: u64,
    share: Option<String>,
    name: String,
    capture: CaptureRef,
    phase: ShareOutPhase,
    /// Ended here before Orbiters gave its id: the share ends as soon as it does.
    stopped: bool,
}

#[derive(Debug)]
struct In {
    number: u64,
    offer: Offer,
    until_ms: u64,
    /// Show was pressed: the answer is on its way.
    answering: bool,
}

#[derive(Debug)]
struct Watch {
    number: u64,
    rules: Watching,
    view: Option<(f32, [f32; 2])>,
    file: Option<PathBuf>,
    start: Option<ShareStart>,
    failed: Option<String>,
}

#[derive(Default)]
pub struct SharedRuntime {
    targets: Vec<CallContact>,
    out: Option<Out>,
    offers: Vec<In>,
    watching: Option<Watch>,
    sending: Sending,
    numbered: u64,
    /// The friends here last told Orbiters, and when; a refusal's pause.
    here_sent: Option<(Vec<String>, Instant)>,
    backoff_until: Option<Instant>,
    /// The presence is up on Orbiters (a withdrawal is owed when it stops).
    present: bool,
    /// The credential it is for: another one forgets everything of the last.
    identity: Option<String>,
    /// Tests: the identity whose presence was last withdrawn on its way out.
    #[cfg(test)]
    left: Option<String>,
    /// The requests' thread: each request goes with the identity (`identity` above) it was made under.
    requests: Option<mpsc::Sender<(String, Request)>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What a refused offer says to the sharer, by the server's code.
fn refusal(code: Option<&str>, name: &str) -> String {
    match code {
        Some("no_access") => tr!("{name} can't open this 3D picture. Share it with them in the gallery first.", name = name),
        Some("not_shareable") => tr("This 3D picture can't be shown to friends.").to_string(),
        Some("stale") => tr("This 3D picture was replaced. Open it again to show it.").to_string(),
        Some("busy") => tr!("{name} has other 3D pictures waiting. Try again in a moment.", name = name),
        Some("version") => tr!("{name} needs a newer Orbiters - N4N0 to look at it with you.", name = name),
        _ => tr!("{name} can't be shown it right now.", name = name),
    }
}

impl SharedRuntime {
    /// What the window and the overlay show.
    fn status(&self) -> SharedViewStatus {
        SharedViewStatus {
            targets: self.targets.clone(),
            outgoing: self.out.as_ref().filter(|o| !o.stopped).map(|o| ShareOut { to: o.name.clone(), capture: o.capture.id, phase: o.phase }),
            offer: self.offers.iter().find(|o| !o.answering).map(|o| ShareOffer {
                id: o.number,
                from: o.offer.from.name.clone(),
                title: o.offer.capture.title.clone(),
                until_ms: o.until_ms,
                for_ms: (o.offer.expires_in * 1000).min(OFFER_FOR_MS),
            }),
            watching: self.watching.as_ref().map(|w| ShareWatching {
                id: w.number,
                from: w.rules.from.name.clone(),
                title: w.rules.capture.title.clone(),
                capture: w.rules.capture.id,
                revision: w.rules.capture.revision,
                file: w.file.clone(),
                start: w.start,
                view: w.view,
                failed: w.failed.clone(),
            }),
        }
    }

    fn next_number(&mut self) -> u64 {
        self.numbered += 1;
        self.numbered
    }

    /// Show the small view to `contact`: one share at a time, to a friend who can be reached now.
    fn share(&mut self, contact: &str, capture: CaptureRef, reach: f32, moved: [f32; 2], now: u64) -> Acts {
        let Some(friend) = self.targets.iter().find(|t| t.id == contact).cloned() else {
            return Acts { notice: Some((NoticeKind::Warning, tr("This friend can't be shown it right now.").into())), ..Default::default() };
        };
        let mut acts = self.stop();
        let number = self.next_number();
        self.sending = Sending::default();
        let state = self.sending.first(reach, moved, now);
        acts.requests.push(Request::Offer { number, contact: friend.id, capture: capture.clone(), state });
        self.out = Some(Out { number, share: None, name: friend.name, capture, phase: ShareOutPhase::Offering, stopped: false });
        acts.publish = true;
        acts
    }

    /// Orbiters gave the offer `number` its share, or refused it (its code).
    fn offered(&mut self, number: u64, answer: Result<String, Option<String>>) -> Acts {
        let Some(out) = self.out.as_mut().filter(|o| o.number == number) else {
            // Ended here meanwhile, or another share since: it ends on Orbiters too.
            return match answer {
                Ok(share) => Acts { requests: vec![Request::End { share }], ..Default::default() },
                Err(_) => Acts::default(),
            };
        };
        match answer {
            Ok(share) if out.stopped => {
                self.out = None;
                Acts { requests: vec![Request::End { share }], publish: true, ..Default::default() }
            }
            Ok(share) => {
                out.share = Some(share);
                out.phase = ShareOutPhase::Asked;
                Acts { publish: true, ..Default::default() }
            }
            Err(code) => {
                let name = out.name.clone();
                self.out = None;
                Acts { notice: Some((NoticeKind::Warning, refusal(code.as_deref(), &name))), publish: true, ..Default::default() }
            }
        }
    }

    /// The small view shown is now so: its state goes when it changed (paced).
    fn view(&mut self, reach: f32, moved: [f32; 2], now: u64) -> Acts {
        let Some(share) = self.out.as_ref().filter(|o| !o.stopped).and_then(|o| o.share.clone()) else { return Acts::default() };
        match self.sending.next(reach, moved, now) {
            Some(state) => Acts { requests: vec![Request::State { share, state }], ..Default::default() },
            None => Acts::default(),
        }
    }

    /// This side stops showing its view.
    fn stop(&mut self) -> Acts {
        let Some(out) = self.out.as_mut() else { return Acts::default() };
        match out.share.clone() {
            Some(share) => {
                self.out = None;
                Acts { requests: vec![Request::End { share }], publish: true, ..Default::default() }
            }
            None => {
                out.stopped = true;
                Acts { publish: true, ..Default::default() }
            }
        }
    }

    /// The offer `id` answered here: Show or Not now. Nothing is fetched before Show.
    fn answer(&mut self, id: u64, accept: bool) -> Acts {
        let Some(at) = self.offers.iter().position(|o| o.number == id && !o.answering) else { return Acts::default() };
        let share = self.offers[at].offer.share.clone();
        if !accept {
            self.offers.remove(at);
            return Acts { requests: vec![Request::Answer { number: id, share, accept: false }], publish: true, ..Default::default() };
        }
        self.offers[at].answering = true;
        Acts { requests: vec![Request::Answer { number: id, share, accept: true }], publish: true, ..Default::default() }
    }

    /// Orbiters took the Show of the offer `number` (with the sharer's latest state), or refused it.
    fn accepted(&mut self, number: u64, answer: Result<Option<ViewState>, ()>) -> Acts {
        let Some(at) = self.offers.iter().position(|o| o.number == number) else { return Acts::default() };
        let offer = self.offers.remove(at).offer;
        let Ok(state) = answer else {
            return Acts {
                notice: Some((NoticeKind::Warning, tr!("{name}'s 3D picture isn't shown any more.", name = offer.from.name))),
                publish: true,
                ..Default::default()
            };
        };
        let mut acts = self.leave();
        let mut rules = Watching::new(&offer);
        let view = state.and_then(|s| match rules.take(&offer.share, &s) {
            Applied::Apply(s) => Some((s.reach, s.moved)),
            _ => None,
        });
        // One view looked at: other offers wait no longer.
        for other in std::mem::take(&mut self.offers) {
            acts.requests.push(Request::Answer { number: other.number, share: other.offer.share, accept: false });
        }
        let number = self.next_number();
        acts.requests.push(Request::Fetch { number, capture: offer.capture.clone() });
        self.watching = Some(Watch { number, rules, view, file: None, start: None, failed: None });
        acts.publish = true;
        acts
    }

    /// The capture of the view `number` came (its file and start view), or could not.
    fn fetched(&mut self, number: u64, answer: Result<(PathBuf, ShareStart), String>) -> Acts {
        let Some(w) = self.watching.as_mut().filter(|w| w.number == number) else { return Acts::default() };
        match answer {
            Ok((file, start)) => {
                w.file = Some(file);
                w.start = Some(start);
                Acts { publish: true, ..Default::default() }
            }
            Err(message) => {
                w.failed = Some(message.clone());
                let share = w.rules.share.clone();
                let name = w.rules.from.name.clone();
                self.watching = None;
                Acts {
                    requests: vec![Request::End { share }],
                    notice: Some((NoticeKind::Warning, tr!("{name}'s 3D picture could not be opened: {message}", name = name, message = message))),
                    publish: true,
                }
            }
        }
    }

    /// This side stops looking at the friend's view.
    fn leave(&mut self) -> Acts {
        match self.watching.take() {
            Some(w) => Acts { requests: vec![Request::End { share: w.rules.share }], publish: true, ..Default::default() },
            None => Acts::default(),
        }
    }

    /// An event of the stream about shares. Anything not about this side's own share, offer or view does nothing.
    fn event(&mut self, event: &str, v: &Value, now: u64) -> Acts {
        if v["v"] != json!(sv::VERSION) {
            return Acts::default();
        }
        let share = v["share"].as_str().unwrap_or_default();
        if !sv::share_id(share) {
            return Acts::default();
        }
        match event {
            "share_offer" => {
                let Some(offer) = Offer::read(v) else { return Acts::default() };
                let known = self.offers.iter().any(|o| o.offer.share == offer.share) || self.watching.as_ref().is_some_and(|w| w.rules.share == offer.share);
                if known {
                    // The same offer again (a stream that opened again).
                    return Acts::default();
                }
                if self.offers.len() >= OFFERS_MOST {
                    return Acts { requests: vec![Request::Answer { number: 0, share: offer.share, accept: false }], ..Default::default() };
                }
                let number = self.next_number();
                let until_ms = now + (offer.expires_in * 1000).min(OFFER_FOR_MS);
                self.offers.push(In { number, offer, until_ms, answering: false });
                Acts { publish: true, ..Default::default() }
            }
            "share_answer" => {
                let Some(out) = self.out.as_mut().filter(|o| o.share.as_deref() == Some(share) && o.phase == ShareOutPhase::Asked) else { return Acts::default() };
                if v["accepted"] == json!(true) {
                    out.phase = ShareOutPhase::Watching;
                    let name = out.name.clone();
                    return Acts { notice: Some((NoticeKind::Info, tr!("{name} is looking at it with you.", name = name))), publish: true, ..Default::default() };
                }
                let name = out.name.clone();
                self.out = None;
                Acts { notice: Some((NoticeKind::Info, tr!("{name} said not now.", name = name))), publish: true, ..Default::default() }
            }
            "share_state" => {
                let Some(w) = self.watching.as_mut() else { return Acts::default() };
                let Ok(state) = serde_json::from_value::<ViewState>(v["state"].clone()) else { return Acts::default() };
                match w.rules.take(share, &state) {
                    Applied::Apply(s) => {
                        w.view = Some((s.reach, s.moved));
                        Acts { publish: true, ..Default::default() }
                    }
                    _ => Acts::default(),
                }
            }
            "share_ended" => {
                let by_viewer = v["by"] == json!("viewer");
                if let Some(out) = self.out.as_ref().filter(|o| o.share.as_deref() == Some(share)) {
                    let name = out.name.clone();
                    self.out = None;
                    let said = if by_viewer { tr!("{name} stopped looking.", name = name) } else { tr!("{name} can't see it any more.", name = name) };
                    return Acts { notice: Some((NoticeKind::Info, said)), publish: true, ..Default::default() };
                }
                if let Some(w) = self.watching.as_ref().filter(|w| w.rules.share == share) {
                    let name = w.rules.from.name.clone();
                    self.watching = None;
                    return Acts { notice: Some((NoticeKind::Info, tr!("{name} stopped showing their 3D picture.", name = name))), publish: true, ..Default::default() };
                }
                let before = self.offers.len();
                self.offers.retain(|o| o.offer.share != share);
                Acts { publish: self.offers.len() != before, ..Default::default() }
            }
            _ => Acts::default(),
        }
    }

    /// Offers whose time is over go.
    fn expire(&mut self, now: u64) -> bool {
        let before = self.offers.len();
        self.offers.retain(|o| o.answering || o.until_ms > now);
        self.offers.len() != before
    }

    /// The friends here changed or it is time: the presence to send (`None`: nothing to send). `here`: the friends'
    /// hashes, sorted.
    fn presence(&mut self, vrchat_id: Option<&str>, here: Vec<String>, now: Instant) -> Option<Request> {
        let Some(vrchat_id) = vrchat_id else { return self.withdraw() };
        if self.backoff_until.is_some_and(|t| now < t) {
            return None;
        }
        let due = self.here_sent.as_ref().is_none_or(|(sent, at)| *sent != here || now.duration_since(*at) >= PRESENCE_EVERY);
        if !due {
            return None;
        }
        self.here_sent = Some((here.clone(), now));
        self.present = true;
        Some(Request::Presence { vrchat_id: vrchat_id.to_string(), here })
    }

    /// Not in an instance, signed out or N4N0 off: the presence goes (once), and with it every share.
    fn withdraw(&mut self) -> Option<Request> {
        self.here_sent = None;
        if !std::mem::take(&mut self.present) {
            return None;
        }
        Some(Request::Withdraw)
    }

    /// Everything of a share here ends without a word to Orbiters (it ended them with the presence, or the account changed).
    fn forget(&mut self) -> bool {
        let had = self.out.is_some() || self.watching.is_some() || !self.offers.is_empty() || !self.targets.is_empty();
        self.out = None;
        self.watching = None;
        self.offers.clear();
        self.targets.clear();
        had
    }
}

/// The shares' presence withdrawn with the credential of `stamp` (`SharedRuntime::identity`: the API's address, a line
/// break, the credential), on a thread of its own: the identity that leaves is not the one the queue sends with.
fn shared_withdraw_as(stamp: &str) {
    let Some((base, token)) = credential_of(stamp) else { return };
    let _ = std::thread::Builder::new().name("shared-view-leave".into()).spawn(move || {
        let api = Api::at(&base);
        let agent = orbiters_core::http_agent(Some(Duration::from_secs(3)), Some(Duration::from_secs(5)));
        let _ = agent.delete(&api.url("n4n0/shares/presence")).header("Authorization", &format!("Bearer {token}")).call();
    });
}

/// The API's address and the credential a stamp (`SharedRuntime::identity`) names.
fn credential_of(stamp: &str) -> Option<(String, String)> {
    stamp.split_once('\n').map(|(b, t)| (b.to_string(), t.to_string()))
}

/// The capture's start view from its item (`GET /captures/capture/:id`).
fn start_of(item: &Value) -> Option<ShareStart> {
    let view = &item["capture"]["view"];
    let point = |v: &Value| -> Option<[f32; 3]> {
        let a = v.as_array().filter(|a| a.len() == 3)?;
        let p = [a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32];
        p.iter().all(|x| x.is_finite()).then_some(p)
    };
    let height = |v: &Value| v.as_f64().map(|x| x as f32).filter(|x| x.is_finite());
    Some(ShareStart { position: point(&view["position"])?, target: point(&view["target"])?, floor: height(&view["floor"]), eye_height: height(&view["eyeHeight"]) })
}

fn post(api: &Api, token: &str, path: &str, body: &Value) -> anyhow::Result<Value> {
    let response = api.agent().post(&api.url(path)).config().timeout_global(Some(REQUEST_TIMEOUT)).build().header("Authorization", &format!("Bearer {token}")).send_json(body)?;
    Api::json::<Value>(response)
}

fn code_of(e: &anyhow::Error) -> Option<String> {
    e.downcast_ref::<ApiError>().and_then(|a| a.code.clone())
}

/// Downloads the capture `capture`, signed in (its item for the start view, then its file), into the cache's
/// `shared-views` folder. Never through a public address: the file route answers only what this account may read.
fn fetch(api: &Api, token: &str, capture: &CaptureRef) -> Result<(PathBuf, ShareStart), String> {
    let auth = format!("Bearer {token}"); // i18n: not shown (an HTTP header)
    let item = api
        .agent()
        .get(&api.url(&format!("captures/capture/{}", capture.id)))
        .config()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .header("Authorization", &auth)
        .call()
        .map_err(anyhow::Error::from)
        .and_then(Api::json::<Value>)
        .map_err(|e| format!("{e:#}"))?;
    let start = start_of(&item).ok_or_else(|| tr("it has no start view").to_string())?;
    let dir = orbiters_core::paths::cache_dir().join("shared-views");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}-{}.splat", capture.id, capture.revision));
    if file.metadata().is_ok_and(|m| m.len() > 0) {
        return Ok((file, start));
    }
    let response = api.get_transfer(&api.url(&format!("captures/capture/{}/file", capture.id))).header("Authorization", &auth).call().map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(tr!("Orbiters answered {status}", status = status));
    }
    let mut bytes = Vec::new();
    response.into_body().into_with_config().limit(FILE_MOST).reader().read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let part = file.with_extension("part");
    std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &file)).map_err(|e| e.to_string())?;
    Ok((file, start))
}

impl Controller {
    /// Starts the shares presence's thread (it does nothing while N4N0 is off, signed out or out of an instance).
    pub(super) fn start_shared_view(self: &Shared) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("shared-view".into()).spawn(move || {
            let rest = ctl.rest("shared-view", |s| (Part::of(&s.settings), Part::of(&s.account), s.vrchat.running, Part::of(&s.vrchat.world)));
            while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
                ctl.shared_tick();
                rest.wait(ctl.shared_rests());
            }
            // Quitting: Orbiters hears it at once (it would let the presence lapse in 2 minutes anyway).
            let owed = ctl.shared_view.lock().unwrap().withdraw();
            if let (Some(_), Some((api, token))) = (owed, ctl.account_api()) {
                let agent = orbiters_core::http_agent(Some(Duration::from_secs(3)), Some(Duration::from_secs(5)));
                let _ = agent.delete(&api.url("n4n0/shares/presence")).header("Authorization", &format!("Bearer {token}")).call();
            }
        });
    }

    /// Whether the calls' event stream is wanted for shares (open even with Calls off).
    pub(super) fn shared_view_wants_stream(&self) -> bool {
        self.shared_view.lock().unwrap().present
    }

    /// How long until the next look: two seconds in an instance and while anything is shared, else until that changes.
    fn shared_rests(&self) -> Option<Duration> {
        let view = self.view();
        let world = &view.vrchat.world;
        let in_world = view.settings.n4n0.enabled && view.vrchat.running && world.in_world && world.instance.is_some();
        let rt = self.shared_view.lock().unwrap();
        let shared = rt.present || rt.out.is_some() || rt.watching.is_some() || !rt.offers.is_empty() || !rt.targets.is_empty();
        (shared || in_world && rt.identity.is_some()).then_some(Duration::from_secs(2))
    }

    fn shared_tick(self: &Shared) {
        let settings = self.settings();
        let account = settings.n4n0.enabled.then(|| self.account_api()).flatten();
        let snap = self.view();
        let world = &snap.vrchat.world;
        // Another account (or none, N4N0 switched off): nothing of the last one's shares is kept, and Orbiters hears that
        // it left with its own credential.
        let identity = account.as_ref().map(|(api, token)| format!("{}\n{token}", api.url("")));
        self.shared_identity_to(identity);
        let in_world = snap.vrchat.running && world.in_world && world.instance.is_some();
        let request = {
            let account_id = in_world.then(|| account.as_ref().and_then(|_| self.local_vrchat_account(world))).flatten();
            let here = account_id.as_ref().map(|(_, friends)| {
                let mut here: Vec<String> =
                    world.players.keys().filter(|id| id.starts_with("usr_") && friends.contains(*id)).map(|id| orbiters_core::vrchat_accounts::friend_hash(id)).collect();
                here.sort();
                here.truncate(HERE_MOST);
                here
            });
            let mut rt = self.shared_view.lock().unwrap();
            let expired = rt.expire(now_ms());
            let request = rt.presence(account_id.as_ref().map(|(id, _)| id.as_str()), here.unwrap_or_default(), Instant::now());
            if request == Some(Request::Withdraw) {
                rt.forget();
            }
            (request, expired)
        };
        if let Some(r) = request.0 {
            let acts = Acts { requests: vec![r], publish: true, ..Default::default() };
            self.shared_act(self.shared_view.lock().unwrap(), acts);
        } else if request.1 {
            self.shared_publish();
        }
        // The calls' stream carries the offers: open while the presence is up.
        if let (true, Some((api, token))) = (self.shared_view_wants_stream(), account) {
            self.calls_stream(&api, &token);
        }
    }

    /// The VRChat account signed in on this computer and its friends (VRCX's or VRCNext's data, as calls read it).
    fn local_vrchat_account(&self, world: &orbiters_core::vrchat_log::WorldState) -> Option<(String, std::collections::BTreeSet<String>)> {
        let log_account = world.local_player.as_ref().and_then(|me| world.players.iter().find(|(_, n)| *n == me).map(|(id, _)| id.clone()));
        let local = orbiters_core::vrchat_accounts::read(log_account.as_deref());
        local.account.map(|(id, _)| (id, local.friends))
    }

    /// What a surface asks (`crate::ipc::SharedViewCommand`), while it showed the member of `epoch`: what begins something
    /// with a friend is theirs, or is not done.
    pub fn shared_view(self: &Shared, command: SharedViewCommand, epoch: u64) {
        let begins = matches!(command, SharedViewCommand::Share { .. } | SharedViewCommand::Answer { accept: true, .. });
        let acting = self.n4n0.acting(epoch);
        if begins && acting.is_none() {
            return self.n4n0.refuse_other_identity();
        }
        let mut rt = self.shared_view.lock().unwrap();
        let acts = match command {
            SharedViewCommand::Share { contact, capture, revision, reach, moved } => {
                rt.share(&contact, CaptureRef { kind: "capture".into(), id: capture, revision, title: String::new() }, reach, moved, now_ms())
            }
            SharedViewCommand::View { reach, moved } => rt.view(reach, moved, now_ms()),
            SharedViewCommand::Stop => rt.stop(),
            SharedViewCommand::Answer { id, accept } => rt.answer(id, accept),
            SharedViewCommand::Leave => rt.leave(),
        };
        self.shared_act(rt, acts);
    }

    /// An event of the calls' stream about shares.
    pub(super) fn shared_view_event(self: &Shared, event: &str, v: &Value) {
        let mut rt = self.shared_view.lock().unwrap();
        let acts = rt.event(event, v, now_ms());
        self.shared_act(rt, acts);
    }

    fn shared_act(self: &Shared, mut rt: MutexGuard<'_, SharedRuntime>, acts: Acts) {
        for request in acts.requests {
            // Stamped with the identity it is made under; nobody signed in: nothing to send (verify3).
            let Some(stamp) = rt.identity.clone() else { continue };
            let request = (stamp, request);
            let request = match &rt.requests {
                Some(tx) => match tx.send(request) {
                    Ok(()) => continue,
                    Err(mpsc::SendError(r)) => r,
                },
                None => request,
            };
            let (tx, rx) = mpsc::channel();
            let ctl = self.clone();
            if std::thread::Builder::new().name("shared-view-requests".into()).spawn(move || ctl.shared_requests(rx)).is_ok() {
                let _ = tx.send(request);
                rt.requests = Some(tx);
            }
        }
        drop(rt);
        if let Some((kind, message)) = acts.notice {
            self.notify(kind, message);
        }
        if acts.publish {
            self.shared_publish();
        }
    }

    /// The identity of the shares becomes `now` (`None`: nobody, or N4N0 off). When it changes, the one before leaves:
    /// its presence is withdrawn with its own credential (kept for that: the queue would drop it, as it sends a request
    /// only with the identity signed in now), and everything shown of its shares goes on the same turn. The withdrawal
    /// sent, the identity it was for.
    pub(super) fn shared_identity_to(&self, now: Option<String>) -> Option<String> {
        let (departed, forgot) = {
            let mut rt = self.shared_view.lock().unwrap();
            if rt.identity == now {
                return None;
            }
            let before = std::mem::replace(&mut rt.identity, now);
            let owed = rt.withdraw().is_some();
            let forgot = rt.forget();
            let departed = before.filter(|_| owed);
            // Behind whatever of that identity is already queued or on its way (a presence update): the requests' thread
            // sends in order, so Orbiters hears the withdrawal last and the presence stays withdrawn. Without a queue,
            // nothing of it can be on its way: sent at once.
            if let Some(stamp) = &departed {
                let queued = rt.requests.as_ref().is_some_and(|tx| tx.send((stamp.clone(), Request::Withdraw)).is_ok());
                if !queued {
                    shared_withdraw_as(stamp);
                }
                #[cfg(test)]
                {
                    rt.left = Some(stamp.clone());
                }
            }
            (departed, forgot)
        };
        if forgot || departed.is_some() {
            self.shared_publish();
        }
        departed
    }

    /// The account leaves (signed out, another account): the shares let go of it at once, not at their next turn.
    pub(super) fn shared_depart(&self) {
        self.shared_identity_to(None);
    }

    /// The account signed in changed: the shares follow it now (another account's shares are not shown a moment longer).
    pub(super) fn shared_follow_account(&self) {
        let now = self.shared_identity_now().map(|(now, ..)| now);
        let held = self.shared_view.lock().unwrap().identity.clone();
        if held.is_some() && held != now {
            self.shared_identity_to(None);
        }
    }

    /// The identity signed in now, as `SharedRuntime::identity` writes it (N4N0 on, signed in), with its API and credential.
    fn shared_identity_now(&self) -> Option<(String, Api, String)> {
        if !self.settings().n4n0.enabled {
            return None;
        }
        self.account_api().map(|(api, token)| (format!("{}\n{token}", api.url("")), api, token))
    }

    /// The shares' state, to take what came back for `stamp`: only while that is still the identity signed in, and the
    /// one the state is for (an account switched meanwhile: dropped).
    fn shared_for(&self, stamp: &str) -> Option<MutexGuard<'_, SharedRuntime>> {
        if self.shared_identity_now().is_none_or(|(now, ..)| now != stamp) {
            return None;
        }
        let rt = self.shared_view.lock().unwrap();
        (rt.identity.as_deref() == Some(stamp)).then_some(rt)
    }

    /// The requests, in order. A state is skipped when a newer one for the same share already waits. Each goes with the
    /// credential of the identity it was made under, or not at all; what comes back is taken only for that identity.
    fn shared_requests(self: &Shared, rx: mpsc::Receiver<(String, Request)>) {
        let mut carried: Option<(String, Request)> = None;
        loop {
            let (stamp, request) = match carried.take() {
                Some(r) => r,
                None => match rx.recv() {
                    Ok(r) => r,
                    Err(_) => return,
                },
            };
            let request = match request {
                Request::State { share, mut state } => {
                    while let Ok(next) = rx.try_recv() {
                        match next {
                            (s_stamp, Request::State { share: s, state: newer }) if s == share && s_stamp == stamp => state = newer,
                            other => {
                                carried = Some(other);
                                break;
                            }
                        }
                    }
                    Request::State { share, state }
                }
                other => other,
            };
            // A withdrawal goes with the credential of the identity it is for (one leaving, too); anything else only with
            // the identity signed in now.
            let credential = match &request {
                Request::Withdraw => credential_of(&stamp).map(|(base, token)| (Api::at(&base), token)),
                _ => self.shared_identity_now().filter(|(now, ..)| *now == stamp).map(|(_, api, token)| (api, token)),
            };
            let Some((api, token)) = credential else { continue };
            match request {
                Request::Presence { vrchat_id, here } => {
                    match api
                        .agent()
                        .put(&api.url("n4n0/shares/presence"))
                        .config()
                        .timeout_global(Some(REQUEST_TIMEOUT))
                        .build()
                        .header("Authorization", &format!("Bearer {token}"))
                        .send_json(&json!({ "vrchat_id": vrchat_id, "here": here }))
                        .map_err(anyhow::Error::from)
                        .and_then(Api::json::<Value>)
                    {
                        Ok(v) => {
                            let targets: Vec<CallContact> = v["targets"]
                                .as_array()
                                .map(|a| a.iter().filter_map(|c| Some(CallContact { id: c["id"].as_str().filter(|id| sv::contact_id(id))?.to_string(), name: c["name"].as_str()?.to_string() })).collect())
                                .unwrap_or_default();
                            let Some(mut rt) = self.shared_for(&stamp) else { continue };
                            rt.targets = targets;
                            rt.backoff_until = None;
                            drop(rt);
                            self.shared_publish();
                        }
                        Err(e) => {
                            eprintln!("shared view presence: {e:#}");
                            let Some(mut rt) = self.shared_for(&stamp) else { continue };
                            rt.backoff_until = Some(Instant::now() + PRESENCE_BACKOFF);
                            rt.here_sent = None;
                            rt.targets.clear();
                            drop(rt);
                            self.shared_publish();
                        }
                    }
                }
                Request::Withdraw => {
                    let agent = orbiters_core::http_agent(Some(Duration::from_secs(5)), Some(Duration::from_secs(10)));
                    let _ = agent.delete(&api.url("n4n0/shares/presence")).header("Authorization", &format!("Bearer {token}")).call();
                }
                Request::Offer { number, contact, capture, state } => {
                    let body = json!({ "v": sv::VERSION, "contact": contact, "capture": { "type": "capture", "id": capture.id, "revision": capture.revision }, "state": state });
                    let answer = match post(&api, &token, "n4n0/shares", &body) {
                        Ok(v) => v["share"].as_str().filter(|s| sv::share_id(s)).map(str::to_string).ok_or(None),
                        Err(e) => {
                            eprintln!("shared view offer: {e:#}");
                            Err(code_of(&e))
                        }
                    };
                    let Some(mut rt) = self.shared_for(&stamp) else { continue };
                    let acts = rt.offered(number, answer);
                    self.shared_act(rt, acts);
                }
                Request::Answer { number, share, accept } => {
                    let path = format!("n4n0/shares/{share}/{}", if accept { "accept" } else { "decline" });
                    let answer = post(&api, &token, &path, &json!({ "v": sv::VERSION }));
                    if !accept {
                        if let Err(e) = answer {
                            eprintln!("shared view decline: {e:#}");
                        }
                        continue;
                    }
                    let answer = answer.map(|v| serde_json::from_value::<ViewState>(v["state_now"].clone()).ok()).map_err(|e| eprintln!("shared view accept: {e:#}"));
                    let Some(mut rt) = self.shared_for(&stamp) else { continue };
                    let acts = rt.accepted(number, answer);
                    self.shared_act(rt, acts);
                }
                Request::State { share, state } => {
                    if let Err(e) = post(&api, &token, &format!("n4n0/shares/{share}/state"), &json!({ "v": sv::VERSION, "state": state })) {
                        eprintln!("shared view state: {e:#}");
                    }
                }
                Request::End { share } => {
                    if let Err(e) = post(&api, &token, &format!("n4n0/shares/{share}/end"), &json!({ "v": sv::VERSION })) {
                        eprintln!("shared view end: {e:#}");
                    }
                }
                Request::Fetch { number, capture } => {
                    let answer = fetch(&api, &token, &capture);
                    let Some(mut rt) = self.shared_for(&stamp) else { continue };
                    let acts = rt.fetched(number, answer);
                    self.shared_act(rt, acts);
                }
            }
        }
    }

    fn shared_publish(&self) {
        let status = self.shared_view.lock().unwrap().status();
        self.update_if(|s| s.shared_view.set(status));
    }
}

/// Tests elsewhere (the account's paths): the shares signed in as `identity`, a friend's offer on screen.
#[cfg(test)]
impl Controller {
    pub(super) fn shared_offer_on_screen(&self, identity: &str) {
        {
            let mut rt = self.shared_view.lock().unwrap();
            rt.identity = Some(identity.to_string());
            let offer = json!({ "v": 1, "share": "s_BBBBBBBBBBBBBBBBBBBBBB", "from": { "id": "c_AAAAAAAAAAAAAAAAAAAAAA", "name": "Alice" },
                "capture": { "type": "capture", "id": 412, "revision": 77, "title": "The Black Cat" }, "expires_in": 30 });
            let _ = rt.event("share_offer", &offer, now_ms());
        }
        self.shared_publish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify4: the shares let go of an account on the same turn it leaves (a friend's offer on screen goes), and Orbiters
    /// hears the presence withdrawn with the credential of the identity that left, also when N4N0 is switched off in an
    /// instance (the identity becomes nobody before the withdrawal is made).
    #[test]
    fn the_shares_let_go_at_once_and_withdraw_with_the_departing_credential() {
        let mut settings = orbiters_core::settings::Settings::default();
        settings.n4n0.enabled = true;
        let ctl = crate::controller::Controller::new(settings);
        // A server address nothing listens on: the withdrawal goes nowhere.
        let a = "http://127.0.0.1:1/\norbit-a".to_string();
        let offer_up = |ctl: &crate::controller::Shared| {
            {
                let mut rt = ctl.shared_view.lock().unwrap();
                rt.identity = Some(a.clone());
                rt.present = true;
                let _ = rt.event("share_offer", &offer_event(SHARE), now_ms());
            }
            ctl.shared_publish();
            assert!(ctl.snapshot.lock().unwrap().shared_view.offer.is_some(), "A's friend's offer on screen");
        };
        // N4N0 switched off (what the shares' turn does): A's presence is withdrawn with A's credential.
        offer_up(&ctl);
        assert_eq!(ctl.shared_identity_to(None), Some(a.clone()), "withdrawn as A");
        assert!(ctl.snapshot.lock().unwrap().shared_view.offer.is_none(), "the offer went on the same turn");
        // Signed out: the same, at once, before the shares' next turn.
        offer_up(&ctl);
        {
            let mut rt = ctl.account_runtime.lock().unwrap();
            rt.read = Some((orbiters_core::account::Server::Production, None));
            rt.account = Some(orbiters_core::account::Account { token: "orbit-a".into(), user: None, username: None, avatar_url: None, features: None });
        }
        assert!(ctl.leave_unless_same(None));
        assert!(ctl.snapshot.lock().unwrap().shared_view.offer.is_none());
        assert!(ctl.shared_view.lock().unwrap().identity.is_none() && !ctl.shared_view.lock().unwrap().present);
        // Nothing to withdraw twice.
        assert_eq!(ctl.shared_identity_to(None), None);
    }

    /// Verify5 (3): a presence update already queued (or on its way) for the identity that leaves, then the withdrawal:
    /// both go through the one requests' thread, the withdrawal after it, so Orbiters' last word is the withdrawal. The
    /// withdrawal carries the leaving identity's own credential, as the thread sends it.
    #[test]
    fn the_withdrawal_goes_after_a_presence_update_on_its_way() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        let a = "http://127.0.0.1:1/\norbit-a".to_string();
        let (tx, rx) = mpsc::channel();
        {
            let mut rt = ctl.shared_view.lock().unwrap();
            rt.identity = Some(a.clone());
            rt.present = true;
            rt.requests = Some(tx);
        }
        ctl.shared_act(ctl.shared_view.lock().unwrap(), Acts { requests: vec![Request::Presence { vrchat_id: "usr_a".into(), here: vec![] }], ..Default::default() });
        ctl.shared_identity_to(None);
        let order: Vec<(String, Request)> = rx.try_iter().collect();
        assert_eq!(order, vec![(a.clone(), Request::Presence { vrchat_id: "usr_a".into(), here: vec![] }), (a.clone(), Request::Withdraw)]);
        assert_eq!(credential_of(&a), Some(("http://127.0.0.1:1/".to_string(), "orbit-a".to_string())));
    }

    /// Verify4 should-fix: N4N0 switched off while in an instance: the shares' own turn withdraws the presence as the
    /// identity that leaves (it used to empty its identity first, and the queue then dropped the withdrawal).
    #[test]
    fn switching_n4n0_off_withdraws_the_presence_on_the_shares_turn() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        let a = "http://127.0.0.1:1/\norbit-a".to_string();
        {
            let mut rt = ctl.shared_view.lock().unwrap();
            rt.identity = Some(a.clone());
            rt.present = true;
        }
        ctl.shared_tick();
        let rt = ctl.shared_view.lock().unwrap();
        assert_eq!(rt.left.as_deref(), Some(a.as_str()), "withdrawn with A's credential");
        assert!(rt.identity.is_none() && !rt.present);
    }

    /// Verify3 blocker 2: the shared 3D view's requests go with the identity they were made under, and what comes back
    /// is taken only while that identity is the one signed in and the one the state is for: a presence answer (friends'
    /// names to show a view to) of account A never reaches B, and a queued request of A is never sent with B's credential.
    #[test]
    fn shared_requests_and_replies_are_bound_to_their_identity() {
        let mut settings = orbiters_core::settings::Settings::default();
        settings.n4n0.enabled = true;
        let ctl = crate::controller::Controller::new(settings);
        let server = orbiters_core::account::Server::current();
        let sign_in = |token: &str| {
            let mut rt = ctl.account_runtime.lock().unwrap();
            rt.read = Some((server, None));
            rt.account = Some(orbiters_core::account::Account { token: token.into(), user: None, username: None, avatar_url: None, features: None });
        };
        sign_in("orbit-a");
        let a = format!("{}\n{}", Api::new(server).url(""), "orbit-a");
        let (tx, rx) = mpsc::channel();
        {
            let mut rt = ctl.shared_view.lock().unwrap();
            rt.identity = Some(a.clone());
            rt.requests = Some(tx);
        }
        ctl.shared_act(ctl.shared_view.lock().unwrap(), Acts { requests: vec![Request::Withdraw], ..Default::default() });
        assert_eq!(rx.try_recv().unwrap(), (a.clone(), Request::Withdraw), "stamped with A when made");
        assert!(ctl.shared_for(&a).is_some(), "A's answer, A signed in: taken");
        // B signs in: A's queued request has no credential to go with, A's answers are dropped.
        sign_in("orbit-b");
        assert!(ctl.shared_identity_now().is_some_and(|(now, ..)| now != a));
        assert!(ctl.shared_for(&a).is_none(), "A's answer never lands in B's state");
        // Signed out entirely: nothing is stamped, nothing goes.
        ctl.shared_view.lock().unwrap().identity = None;
        ctl.shared_act(ctl.shared_view.lock().unwrap(), Acts { requests: vec![Request::Withdraw], ..Default::default() });
        assert!(rx.try_recv().is_err());
    }

    const ALICE: &str = "c_AAAAAAAAAAAAAAAAAAAAAA";
    const SHARE: &str = "s_BBBBBBBBBBBBBBBBBBBBBB";

    fn capture() -> CaptureRef {
        CaptureRef { kind: "capture".into(), id: 412, revision: 77, title: String::new() }
    }

    fn with_target(rt: &mut SharedRuntime) {
        rt.targets = vec![CallContact { id: ALICE.into(), name: "Alice".into() }];
    }

    fn offer_event(share: &str) -> Value {
        json!({ "v": 1, "share": share, "from": { "id": ALICE, "name": "Alice" }, "capture": { "type": "capture", "id": 412, "revision": 77, "title": "The Black Cat" }, "expires_in": 30 })
    }

    fn state_event(share: &str, rev: u64, reach: f32) -> Value {
        json!({ "v": 1, "share": share, "state": ViewState::before_viewer(rev, reach, [0.5, 0.0]) })
    }

    /// The sharer's side against a fake server: an offer, its id, the answer, paced states, the end.
    #[test]
    fn the_sharer_offers_follows_the_answer_and_sends_its_view() {
        let mut rt = SharedRuntime::default();
        // Nobody reachable: nothing is sent.
        let acts = rt.share(ALICE, capture(), 10.0, [0.0, 0.0], 0);
        assert!(acts.requests.is_empty() && acts.notice.is_some());
        with_target(&mut rt);
        let acts = rt.share(ALICE, capture(), 10.0, [0.0, 0.0], 0);
        let Some(Request::Offer { number, contact, state, .. }) = acts.requests.first() else { panic!("{:?}", acts.requests) };
        assert_eq!((contact.as_str(), state.rev), (ALICE, 1));
        let number = *number;
        assert_eq!(rt.status().outgoing.unwrap().phase, ShareOutPhase::Offering);
        // No view state goes before Orbiters named the share.
        assert!(rt.view(5.0, [0.0, 0.0], 1_000).requests.is_empty());
        rt.offered(number, Ok(SHARE.into()));
        assert_eq!(rt.status().outgoing.unwrap().phase, ShareOutPhase::Asked);
        let acts = rt.view(5.0, [0.0, 0.0], 1_000);
        assert!(matches!(&acts.requests[..], [Request::State { share, state }] if share == SHARE && state.rev == 2 && state.reach == 5.0));
        assert!(rt.view(4.0, [0.0, 0.0], 1_050).requests.is_empty(), "paced");
        // A stranger's answer about another share changes nothing.
        rt.event("share_answer", &json!({ "v": 1, "share": "s_CCCCCCCCCCCCCCCCCCCCCC", "accepted": true }), 0);
        assert_eq!(rt.status().outgoing.unwrap().phase, ShareOutPhase::Asked);
        let acts = rt.event("share_answer", &json!({ "v": 1, "share": SHARE, "accepted": true }), 0);
        assert!(acts.notice.is_some());
        assert_eq!(rt.status().outgoing.unwrap().phase, ShareOutPhase::Watching);
        // The friend leaves: the sharer is told, the share is over.
        let acts = rt.event("share_ended", &json!({ "v": 1, "share": SHARE, "by": "viewer", "reason": "left" }), 0);
        assert!(acts.notice.unwrap().1.contains("Alice"));
        assert!(rt.status().outgoing.is_none());
        assert!(rt.view(3.0, [0.0, 0.0], 9_000).requests.is_empty());
    }

    #[test]
    fn a_refused_offer_says_why_and_a_stop_before_the_id_ends_it_once_it_comes() {
        let mut rt = SharedRuntime::default();
        with_target(&mut rt);
        let acts = rt.share(ALICE, capture(), 10.0, [0.0, 0.0], 0);
        let Some(Request::Offer { number, .. }) = acts.requests.first() else { panic!() };
        let acts = rt.offered(*number, Err(Some("no_access".into())));
        assert!(acts.notice.unwrap().1.contains("Alice"), "says who cannot open it");
        assert!(rt.status().outgoing.is_none());
        // Stopped while the offer was on its way: once Orbiters names it, it ends there too.
        let acts = rt.share(ALICE, capture(), 10.0, [0.0, 0.0], 0);
        let Some(Request::Offer { number, .. }) = acts.requests.first() else { panic!() };
        let number = *number;
        assert!(rt.stop().requests.is_empty());
        assert!(rt.status().outgoing.is_none(), "it is gone for the person at once");
        let acts = rt.offered(number, Ok(SHARE.into()));
        assert_eq!(acts.requests, vec![Request::End { share: SHARE.into() }]);
    }

    /// The viewer's side against a fake peer: an offer opens nothing before Show; states apply in order; the end.
    #[test]
    fn the_viewer_is_asked_first_and_follows_only_its_share_in_order() {
        let mut rt = SharedRuntime::default();
        let acts = rt.event("share_offer", &offer_event(SHARE), 1_000);
        assert!(acts.requests.is_empty(), "nothing fetched, nothing answered before the person says");
        let offer = rt.status().offer.expect("asked");
        assert_eq!((offer.from.as_str(), offer.title.as_str(), offer.until_ms), ("Alice", "The Black Cat", 31_000));
        // States before an accept are not ours to apply.
        rt.event("share_state", &state_event(SHARE, 2, 9.0), 1_100);
        assert!(rt.status().watching.is_none());
        let acts = rt.answer(offer.id, true);
        assert!(matches!(&acts.requests[..], [Request::Answer { accept: true, .. }]));
        assert!(rt.status().offer.is_none(), "the band goes on Show");
        let acts = rt.accepted(offer.id, Ok(Some(ViewState::before_viewer(2, 9.0, [0.0, 0.0]))));
        assert!(acts.requests.iter().any(|r| matches!(r, Request::Fetch { capture, .. } if capture.id == 412)), "fetched once shown");
        let w = rt.status().watching.unwrap();
        assert_eq!(w.view, Some((9.0, [0.0, 0.0])));
        // In order: an older or repeated revision does nothing.
        rt.event("share_state", &state_event(SHARE, 3, 6.0), 0);
        rt.event("share_state", &state_event(SHARE, 2, 20.0), 0);
        rt.event("share_state", &state_event(SHARE, 3, 20.0), 0);
        assert_eq!(rt.status().watching.unwrap().view, Some((6.0, [0.5, 0.0])));
        // Another share's state and a stranger's do nothing.
        rt.event("share_state", &state_event("s_CCCCCCCCCCCCCCCCCCCCCC", 9, 1.0), 0);
        rt.event("share_state", &json!({ "v": 2, "share": SHARE, "state": ViewState::before_viewer(10, 1.0, [0.0, 0.0]) }), 0);
        assert_eq!(rt.status().watching.unwrap().view, Some((6.0, [0.5, 0.0])));
        // The file came.
        let number = w.id;
        rt.fetched(number, Ok((PathBuf::from("x.splat"), ShareStart::default())));
        assert!(rt.status().watching.unwrap().file.is_some());
        // Alice stops: the view goes.
        let acts = rt.event("share_ended", &json!({ "v": 1, "share": SHARE, "by": "sharer", "reason": "ended" }), 0);
        assert!(acts.notice.is_some());
        assert!(rt.status().watching.is_none());
    }

    #[test]
    fn the_viewer_can_say_not_now_or_leave_and_a_failed_download_ends_it_honestly() {
        let mut rt = SharedRuntime::default();
        rt.event("share_offer", &offer_event(SHARE), 0);
        let id = rt.status().offer.unwrap().id;
        let acts = rt.answer(id, false);
        assert!(matches!(&acts.requests[..], [Request::Answer { accept: false, .. }]));
        assert!(rt.status().offer.is_none());
        // Accepted, then the download fails: the share ends and the person is told why.
        rt.event("share_offer", &offer_event("s_DDDDDDDDDDDDDDDDDDDDDD"), 0);
        let id = rt.status().offer.unwrap().id;
        rt.answer(id, true);
        rt.accepted(id, Ok(None));
        let number = rt.status().watching.unwrap().id;
        let acts = rt.fetched(number, Err("Orbiters answered 404".into()));
        assert_eq!(acts.requests, vec![Request::End { share: "s_DDDDDDDDDDDDDDDDDDDDDD".into() }]);
        assert!(acts.notice.unwrap().1.contains("404"));
        assert!(rt.status().watching.is_none());
        // Accepted and left: Orbiters hears it.
        rt.event("share_offer", &offer_event("s_EEEEEEEEEEEEEEEEEEEEEE"), 0);
        let id = rt.status().offer.unwrap().id;
        rt.answer(id, true);
        rt.accepted(id, Ok(None));
        assert_eq!(rt.leave().requests, vec![Request::End { share: "s_EEEEEEEEEEEEEEEEEEEEEE".into() }]);
        // Accepted too late (the sharer stopped meanwhile): said, nothing opens.
        rt.event("share_offer", &offer_event("s_FFFFFFFFFFFFFFFFFFFFFF"), 0);
        let id = rt.status().offer.unwrap().id;
        rt.answer(id, true);
        let acts = rt.accepted(id, Err(()));
        assert!(acts.notice.is_some() && rt.status().watching.is_none());
    }

    #[test]
    fn offers_expire_repeat_once_and_never_pile_up() {
        let mut rt = SharedRuntime::default();
        rt.event("share_offer", &offer_event(SHARE), 0);
        rt.event("share_offer", &offer_event(SHARE), 0);
        assert_eq!(rt.offers.len(), 1, "the same offer again is one offer");
        for s in ["s_CCCCCCCCCCCCCCCCCCCCCC", "s_DDDDDDDDDDDDDDDDDDDDDD"] {
            rt.event("share_offer", &offer_event(s), 0);
        }
        let acts = rt.event("share_offer", &offer_event("s_EEEEEEEEEEEEEEEEEEEEEE"), 0);
        assert!(matches!(&acts.requests[..], [Request::Answer { accept: false, .. }]), "a fourth is declined");
        assert!(rt.expire(30_000));
        assert!(rt.offers.is_empty());
        // Malformed or of another version: nothing.
        rt.event("share_offer", &json!({ "v": 1, "share": "s_1", "from": { "id": ALICE, "name": "x" } }), 0);
        let mut v2 = offer_event(SHARE);
        v2["v"] = json!(2);
        rt.event("share_offer", &v2, 0);
        assert!(rt.offers.is_empty());
    }

    #[test]
    fn the_presence_goes_when_the_friends_here_change_and_withdraws_once() {
        let mut rt = SharedRuntime::default();
        let t = Instant::now();
        assert!(rt.presence(None, vec![], t).is_none(), "nothing to withdraw yet");
        assert!(matches!(rt.presence(Some("usr_me"), vec!["a".into()], t), Some(Request::Presence { .. })));
        assert!(rt.presence(Some("usr_me"), vec!["a".into()], t + Duration::from_secs(5)).is_none());
        assert!(rt.presence(Some("usr_me"), vec!["a".into(), "b".into()], t + Duration::from_secs(6)).is_some(), "someone came");
        assert!(rt.presence(Some("usr_me"), vec!["a".into(), "b".into()], t + Duration::from_secs(40)).is_some(), "its time");
        assert_eq!(rt.presence(None, vec![], t + Duration::from_secs(41)), Some(Request::Withdraw));
        assert!(rt.presence(None, vec![], t + Duration::from_secs(42)).is_none(), "once");
    }

    #[test]
    fn its_words_are_in_every_language() {
        for language in orbiters_i18n::Language::TRANSLATED {
            orbiters_i18n::in_language(language, || {
                for code in [Some("no_access"), Some("not_shareable"), Some("stale"), Some("busy"), Some("version"), None] {
                    assert!(!refusal(code, "Alice").is_empty());
                }
            });
        }
    }
}
