//! Scoped data requests, picture updates and deliberate instance creation/self-invites for the signed-in account.
//! Only workers call this module; session credentials never cross IPC.
use super::{Controller, NoticeKind, Shared};
use crate::state::Snapshot;
use orbiters_i18n::tr;
use orbiters_core::{
    account::Server,
    vrchat_api::{Kind, Step, User},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    development: bool,
    member: Option<String>,
    /// Whom the VRChat account is verified for (verify3-r21 B6: a name can be another member's).
    verified_for: Option<orbiters_core::launcher::VerifiedFor>,
    allowed: bool,
    user: Option<String>,
    revision: u64,
}
impl Scope {
    pub fn of(s: &Snapshot) -> Self {
        Self {
            development: s.settings.development,
            member: s.account.state.username().map(str::to_owned),
            verified_for: s.vrchat_login.verified_for.clone(),
            allowed: s.vrchat_login.allowed,
            user: s.vrchat_login.user.as_ref().map(|u| u.id.clone()),
            revision: s.vrchat_login.revision,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    WorldDetails { world: String },
    WorldInstances { world: String },
    /// Continue the exact remaining IDs from WorldInstances, at most 16 lookups per call.
    WorldInstancesNext { world: String, instances: Vec<String> },
    CreateInstance { world: String, access: orbiters_core::vrchat_api::CreateAccess, region: orbiters_core::vrchat_api::InstanceRegion },
    Avatars {
        ids: Vec<String>,
    },
    /// Worlds' thumbnails (at most 8), answered as [`Response::Avatars`].
    Worlds {
        ids: Vec<String>,
    },
    Friends,
    Instance {
        world: String,
        instance: String,
    },
    /// Whether it is open, as VRChat said at most `max_age_ms` ago, and how long ago that was: what Play goes by
    /// (verify3-r21 M2).
    InstanceSeen {
        world: String,
        instance: String,
        max_age_ms: u64,
    },
    Image {
        url: String,
    },
    /// Which of these avatars and worlds (at most [`OWNED_BATCH`]) the signed-in VRChat account uploaded, answered as
    /// [`Response::Owned`]: none without a personal session.
    Owned {
        ids: Vec<String>,
    },
    /// A page of a world list (the overlay's world search), answered as [`Response::Worlds`]: VRChat's through the
    /// personal session, else Orbiters' public Map explorer.
    WorldList {
        list: orbiters_core::maps::WorldList,
        page: usize,
    },
    /// Invite to an exact destination listed or created in this scope within five minutes.
    /// A fresh lookup checks capacity/closure before sending. Answer: Invited or WorldError.
    InviteSelf {
        world: String,
        instance: String,
    },
    /// Takes the person to an exact destination listed or created in this scope within five minutes, directly
    /// (controller/travel.rs): the link to their own VRChat, or VRChat started there; an invite only where neither can be.
    /// A fresh lookup checks capacity/closure first. Answer: Went or WorldError.
    GoThere {
        world: String,
        instance: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    WorldDetails(orbiters_core::vrchat_api::World),
    WorldInstances(orbiters_core::vrchat_api::WorldInstances),
    CreatedInstance(orbiters_core::vrchat_api::WorldInstance),
    WorldError(WorldError),
    Avatars { pictures: Vec<(String, super::vrchat_login::Picture)>, errors: Vec<String> },
    Friends(Vec<User>),
    Instance(Option<bool>),
    InstanceSeen { open: Option<bool>, age_ms: u64 },
    /// The file's bytes, as base64 on the link (CTL2-1).
    Image(#[serde(with = "super::vrchat_login::bytes_b64")] Vec<u8>),
    Owned(Vec<String>),
    Worlds(orbiters_core::maps::FoundWorlds),
    Invited,
    Went(super::travel::Went),
}

/// Structured failures for WorldDetails, WorldInstances, CreateInstance and InviteSelf. No UI wording here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldError {
    AccountChanged, SignInRequired, InvalidWorld, NotListed, Full, Closed, NotJoinable,
    /// Only Orbiters' AI bridge's own VRChat runs, with the person's account: none of the person's to take there.
    BridgeBusy,
    /// VRChat could not be started in the instance, in its launcher's words.
    NotStarted(String),
    Api(orbiters_core::vrchat_api::Error),
}

/// The instances a person may be sent to: those the last listing of a world showed joinable, and those made for their
/// press. One per controller (it was one for the process: tests ran into each other's, BUILD-7).
#[derive(Default)]
pub(crate) struct InstancePermits {
    entries: Vec<(Scope, String, String, std::time::Instant)>,
    /// Made for the person's press: a new listing of the world keeps them (VRChat may not list a room nobody entered
    /// yet, and the page refreshes its list between Make and Go).
    made: Vec<(Scope, String, String, std::time::Instant)>,
    pending: Vec<(Scope, String, Vec<String>, std::time::Instant)>,
}
impl InstancePermits {
    fn allow(&mut self, scope: &Scope, world: &str, items: &[orbiters_core::vrchat_api::WorldInstance]) {
        self.entries.retain(|(s, w, _, at)| s == scope && w != world && at.elapsed().as_secs() < 300);
        self.made.retain(|(s, _, _, at)| s == scope && at.elapsed().as_secs() < 300);
        self.entries.extend(items.iter().filter(|i| i.can_join).map(|i| (scope.clone(), world.into(), i.id.clone(), std::time::Instant::now())));
    }
    /// An instance made for the person's press: it may be gone to, beside those listed.
    fn made(&mut self, scope: &Scope, world: &str, id: &str) {
        self.entries.retain(|(s, _, _, at)| s == scope && at.elapsed().as_secs() < 300);
        self.made.retain(|(s, _, _, at)| s == scope && at.elapsed().as_secs() < 300);
        self.made.push((scope.clone(), world.into(), id.into(), std::time::Instant::now()));
    }
    fn contains(&self, scope: &Scope, world: &str, id: &str) -> bool {
        self.entries.iter().chain(&self.made).any(|(s, w, i, at)| s == scope && w == world && i == id && at.elapsed().as_secs() < 300)
    }
}
/// Whether a world request goes there directly or only invites.
#[derive(Clone, Copy)]
enum Way { Direct, Invite }

fn scoped_world_work(
    scope: &Scope,
    current: impl Fn() -> Scope,
    work: impl FnOnce() -> std::result::Result<Response, WorldError>,
) -> Response {
    if *scope != current() { return Response::WorldError(WorldError::AccountChanged); }
    let result = work();
    if *scope != current() { return Response::WorldError(WorldError::AccountChanged); }
    result.unwrap_or_else(Response::WorldError)
}
/// IDs one [`Request::Owned`] may ask about (VRChat's answers are kept a day, so a page asks once).
pub const OWNED_BATCH: usize = 16;
pub type Result = std::result::Result<Response, String>;

// Cache refreshes and upload publication share the same lock, including their temporary files.
static PICTURE_CACHE: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Controller {
    pub fn vrchat_data(&self, scope: Scope, request: Request) -> Result {
        self.vrchat_data_until(scope, request, std::time::Instant::now() + std::time::Duration::from_secs(60))
    }
    pub(crate) fn vrchat_data_until(&self, scope: Scope, request: Request, deadline: std::time::Instant) -> Result {
        if matches!(&request, Request::WorldDetails { .. } | Request::WorldInstances { .. } | Request::WorldInstancesNext { .. } | Request::CreateInstance { .. } | Request::InviteSelf { .. } | Request::GoThere { .. }) {
            return Ok(self.vrchat_world_data(scope, request, deadline));
        }
        if scope != Scope::of(&self.view()) {
            return Err(tr("The account changed. Please try again.").into());
        }
        let _pictures = matches!(&request, Request::Avatars { .. } | Request::Worlds { .. }).then(|| PICTURE_CACHE.lock().unwrap_or_else(|e| e.into_inner()));
        let source = self.vrchat_source();
        let result = (|| -> anyhow::Result<Response> {
            Ok(match request {
                Request::Avatars { ids } => {
                    anyhow::ensure!(ids.len() <= 8, "{}", tr("Too many avatar thumbnails requested."));
                    let mut pictures = Vec::new();
                    let mut missing = Vec::new();
                    let mut errors = Vec::new();
                    for id in ids {
                        if let Some(bytes) = orbiters_projects::avatars::local_thumbnail(&id).and_then(|p| std::fs::read(p).ok()) {
                            if let Some(p) = avatar_picture(&bytes) {
                                pictures.push((id, p));
                                continue;
                            }
                        }
                        missing.push(id);
                    }
                    if source.direct_available() {
                        for id in missing {
                            let file = orbiters_projects::data_dir()
                                .join("avatar-thumbnails/direct")
                                .join(format!("{}.image", orbiters_projects::avatars::filename(&id)));
                            let result = cached_avatar_picture(&file, || {
                                let Some(a) = source.avatar(&id)? else { return Ok(None) };
                                let url = if a.thumbnail_image_url.is_empty() { &a.image_url } else { &a.thumbnail_image_url };
                                if url.is_empty() {
                                    return Ok(None);
                                }
                                Ok(Some(source.image(url)?))
                            });
                            match result {
                                Ok(Some(p)) => pictures.push((id, p)),
                                Ok(None) => {}
                                Err(e) => errors.push(format!("{id}: {e}")),
                            }
                        }
                    } else {
                        let server = if scope.development { Server::Development } else { Server::Production };
                        match orbiters_projects::network::thumbnails(server, missing) {
                            Ok(paths) => {
                                for (id, t) in paths {
                                    if let Some(p) = std::fs::read(t.path).ok().and_then(|b| avatar_picture(&b)) {
                                        pictures.push((id, p));
                                    }
                                }
                            }
                            Err(e) => errors.push(e.to_string()),
                        }
                    }
                    Response::Avatars { pictures, errors }
                }
                Request::Worlds { ids } => {
                    anyhow::ensure!(ids.len() <= 8, "{}", tr("Too many world thumbnails requested."));
                    // Kept a day: a world's thumbnail changes with its uploads.
                    let cache = orbiters_projects::data_dir().join("world-thumbnails");
                    let mut pictures = Vec::new();
                    let mut errors = Vec::new();
                    for id in ids {
                        let file = cache.join(format!("{}.image", orbiters_projects::avatars::filename(&id)));
                        let result = cached_avatar_picture(&file, || {
                            let Some(url) = source.world_thumbnail(&id)? else { return Ok(None) };
                            if url.is_empty() {
                                return Ok(None);
                            }
                            Ok(Some(source.image(&url)?))
                        });
                        match result {
                            Ok(Some(p)) => pictures.push((id, p)),
                            Ok(None) => {}
                            Err(e) => errors.push(format!("{id}: {e}")),
                        }
                    }
                    Response::Avatars { pictures, errors }
                }
                Request::Friends => Response::Friends(source.friends()?),
                Request::Instance { world, instance } => Response::Instance(source.instance_alive(&world, &instance)?),
                Request::InstanceSeen { world, instance, max_age_ms } => {
                    let (open, age) = source.instance_seen(&world, &instance, std::time::Duration::from_millis(max_age_ms))?;
                    Response::InstanceSeen { open, age_ms: age.as_millis() as u64 }
                }
                Request::Image { url } => Response::Image(source.image(&url)?),
                Request::Owned { ids } => {
                    anyhow::ensure!(ids.len() <= OWNED_BATCH, "{}", tr("Too many avatars and worlds asked about."));
                    Response::Owned(source.owned(&ids)?)
                }
                Request::WorldList { list, page } => {
                    anyhow::ensure!(page <= orbiters_core::maps::FOUND_PAGES, "{}", tr("That page of worlds is too far."));
                    if let orbiters_core::maps::WorldList::Search(q) = &list {
                        anyhow::ensure!(q.trim().chars().count() >= 2, "{}", tr("Type at least two characters."));
                    }
                    Response::Worlds(source.worlds(&list, page)?)
                }
                Request::WorldDetails { .. } | Request::WorldInstances { .. } | Request::WorldInstancesNext { .. } | Request::CreateInstance { .. } | Request::InviteSelf { .. } | Request::GoThere { .. } => unreachable!(),
            })
        })()
        .map_err(|e| e.to_string());
        if scope != Scope::of(&self.view()) {
            return Err(tr("The account changed. Please try again.").into());
        }
        result
    }

    fn vrchat_world_data(&self, scope: Scope, request: Request, deadline: std::time::Instant) -> Response {
        scoped_world_work(&scope, || Scope::of(&self.view()), || {
            if !scope.allowed { return Err(WorldError::SignInRequired); }
            let user = scope.user.as_deref().ok_or(WorldError::SignInRequired)?;
            let world = match &request {
                Request::WorldDetails { world } | Request::WorldInstances { world } | Request::WorldInstancesNext { world, .. } | Request::CreateInstance { world, .. } | Request::InviteSelf { world, .. } | Request::GoThere { world, .. } => world,
                _ => unreachable!(),
            };
            if !orbiters_core::vrchat_log::is_world_id(world) { return Err(WorldError::InvalidWorld); }
            let source = self.vrchat_source();
            let continuation = match &request { Request::WorldInstancesNext { instances, .. } => Some(instances.clone()), _ => None };
            let way = if matches!(request, Request::GoThere { .. }) { Way::Direct } else { Way::Invite };
            match request {
                Request::WorldDetails { world } => source.personal_until(user, || scope == Scope::of(&self.view()), deadline, |c| c.world(&world)).map(Response::WorldDetails).map_err(WorldError::Api),
                Request::WorldInstances { world } | Request::WorldInstancesNext { world, .. } => {
                    let ids = continuation;
                    if ids.is_none() { let mut permits = self.instance_permits.lock().unwrap(); permits.allow(&scope, &world, &[]); permits.pending.retain(|(_, w, _, _)| *w != world); }
                    let ids = match ids { Some(ids) => {
                        if !self.instance_permits.lock().unwrap().pending.iter().any(|(s, w, pending, at)| *s == scope && *w == world && *pending == ids && at.elapsed().as_secs() < 300) { return Err(WorldError::NotListed); }
                        ids
                    }, None => source.personal_until(user, || scope == Scope::of(&self.view()), deadline, |c| c.world_instance_ids(&world)).map_err(WorldError::Api)? };
                    let remaining = ids.iter().skip(16).cloned().collect();
                    let mut list = orbiters_core::vrchat_api::WorldInstances { remaining, ..Default::default() };
                    for id in ids.into_iter().take(16) {
                        // Release the session lock between bounded units, so logout and other requests can proceed.
                        match source.personal_until(user, || scope == Scope::of(&self.view()), deadline, |c| c.world_instance(&world, &id)) {
                            Ok(Some(i)) => list.items.push(i),
                            Ok(None) | Err(orbiters_core::vrchat_api::Error::NotFound) => list.unavailable += 1,
                            Err(e) => return Err(WorldError::Api(e)),
                        }
                    }
                    if scope != Scope::of(&self.view()) { return Err(WorldError::AccountChanged); }
                    let mut permits = self.instance_permits.lock().unwrap();
                    permits.pending.retain(|(s, w, _, at)| *s == scope && *w != world && at.elapsed().as_secs() < 300);
                    if !list.remaining.is_empty() { permits.pending.push((scope.clone(), world.clone(), list.remaining.clone(), std::time::Instant::now())); }
                    permits.entries.extend(list.items.iter().filter(|i| i.can_join).map(|i| (scope.clone(), world.clone(), i.id.clone(), std::time::Instant::now())));
                    Ok(Response::WorldInstances(list))
                }
                Request::CreateInstance { world, access, region } => {
                    let instance = source.personal_until(user, || scope == Scope::of(&self.view()), deadline, |c| c.create_instance(&world, access, region)).map_err(WorldError::Api)?;
                    if scope != Scope::of(&self.view()) { return Err(WorldError::AccountChanged); }
                    // Made for this press: it may be gone to, whatever VRChat says of a room nobody entered yet, and the
                    // instances listed before stay as they were (allow() would forget them).
                    self.instance_permits.lock().unwrap().made(&scope, &world, &instance.id);
                    Ok(Response::CreatedInstance(instance))
                }
                Request::InviteSelf { world, instance } | Request::GoThere { world, instance } => {
                    let direct = matches!(way, Way::Direct);
                    if !self.instance_permits.lock().unwrap().contains(&scope, &world, &instance) { return Err(WorldError::NotListed); }
                    let fresh = source.personal_until(user, || scope == Scope::of(&self.view()), deadline, |c| c.world_instance(&world, &instance)).map_err(WorldError::Api)?
                        .ok_or(WorldError::NotJoinable)?;
                    if fresh.full { return Err(WorldError::Full); }
                    if fresh.closed { return Err(WorldError::Closed); }
                    if !fresh.can_join { return Err(WorldError::NotJoinable); }
                    if scope != Scope::of(&self.view()) { return Err(WorldError::AccountChanged); }
                    let detour = if direct {
                        let travel: std::sync::Arc<dyn super::travel::Travel> = self.travel.lock().unwrap().clone().unwrap_or_else(|| std::sync::Arc::new(super::travel::Real));
                        let bridge = super::travel::Bridge { main: self.sides.main_pid(), sides: self.sides.side_pids() };
                        // The person's VRChat is started under Home's one start lock: never two at once, and while one
                        // is starting the invite to oneself goes instead (that VRChat shows it once up).
                        let remote = self.view().vrchat.remote;
                        let mut started = self.launcher_started.lock().unwrap();
                        // The route decided once, from one look at the processes, is the one taken.
                        let processes = travel.running();
                        let chosen = super::travel::route(remote, &processes, &bridge, |p| travel.own_osc_ports(p));
                        // A start claims profile 0 for itself, as Home's do (REQUEST15 fix B4). Go there says nothing of a
                        // start that never showed (it sends an invite instead): its claim never arms Home's second press.
                        let start = chosen == super::travel::Route::Start;
                        let claim = if start && !started.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60)) { Some(self.launcher_claim_unattended(0, self.view().settings.launch.last_mode)) } else { None };
                        let went = match claim {
                            Some(Err(_)) => Err(super::travel::GoFailure::Invite(super::travel::Detour::Asked)),
                            None if start => Err(super::travel::GoFailure::Invite(super::travel::Detour::Asked)),
                            _ => super::travel::go_on(travel.as_ref(), chosen, &processes, &bridge, &world, &instance),
                        };
                        if went == Ok(super::travel::Went::Started) {
                            *started = Some(std::time::Instant::now());
                        } else if let Some(Ok(id)) = claim {
                            self.launcher_unclaim(id);
                        }
                        drop(started);
                        match went {
                            Ok(went) => return Ok(Response::Went(went)),
                            Err(super::travel::GoFailure::BridgeBusy) => return Err(WorldError::BridgeBusy),
                            Err(super::travel::GoFailure::Start(words)) => return Err(WorldError::NotStarted(words)),
                            Err(super::travel::GoFailure::BadDestination) => return Err(WorldError::InvalidWorld),
                            Err(super::travel::GoFailure::Invite(why)) => why,
                        }
                    } else {
                        super::travel::Detour::Asked
                    };
                    source.personal_until(user, || scope == Scope::of(&self.view()), deadline, |c| c.invite_self(&world, &instance)).map_err(WorldError::Api)?;
                    Ok(if direct { Response::Went(super::travel::Went::Invited(detour)) } else { Response::Invited })
                }
                _ => unreachable!(),
            }
        })
    }
}

/// A public instance as an invite to oneself names it: a number of at most five digits, and VRChat's region.
pub fn public_instance(instance: &str) -> bool {
    let (number, region) = instance.split_once('~').unwrap_or((instance, ""));
    let region_ok = region.is_empty() || ["us", "use", "eu", "jp"].iter().any(|r| region == format!("region({r})"));
    (1..=5).contains(&number.len()) && number.bytes().all(|c| c.is_ascii_digit()) && region_ok
}

/// The largest picture file a page may pick.
const PICTURE_FILE_LIMIT: u64 = 50 * 1024 * 1024;
/// VRChat's picture size for avatars and worlds (its SDK's camera takes them at 1200 × 900).
pub const PICTURE_SIZE: (u32, u32) = (1200, 900);
/// The shortest side a picked picture may have.
const PICTURE_MIN: u32 = 128;

impl Controller {
    /// Changes the picture of `id`, an avatar or a world of the signed-in VRChat account, to the picture file at `path`.
    /// Answers at once: the file is read, checked and made VRChat's size, then uploaded, on a thread of its own; its
    /// step and outcome are in [`Snapshot::vrchat_login`]'s `upload`, the outcome a notice too. One picture at a time.
    pub fn vrchat_picture(self: &Shared, scope: Scope, id: String, path: PathBuf) {
        use super::vrchat_login::PictureUpload;
        // Never repeats, even after an account change empties the snapshot's record: a page takes each outcome once.
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        if scope != Scope::of(&self.view()) {
            return self.notify(NoticeKind::Error, tr("The account changed. Please try again."));
        }
        if Kind::of(&id).is_none() {
            return self.notify(NoticeKind::Error, tr("Only avatars' and worlds' pictures can be changed."));
        }
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        let mut refused = Some(tr("The account changed. Please try again."));
        let Some(stamp) = self.vrchat_now() else { return self.notify(NoticeKind::Error, tr("Sign in to VRChat in Settings to change pictures.")) };
        self.publish_login(&stamp, |login, _| {
            refused = None;
            if !login.allowed || login.user.is_none() {
                refused = Some(tr("Sign in to VRChat in Settings to change pictures."));
            } else if login.upload.as_ref().is_some_and(PictureUpload::running) {
                refused = Some(tr("A picture is already uploading to VRChat. Try again once it is done."));
            } else {
                login.upload = Some(PictureUpload { serial, id: id.clone(), step: Some(Step::Checking), outcome: None });
            }
        });
        if let Some(message) = refused {
            return self.notify(NoticeKind::Error, message);
        }
        let ctl = self.clone();
        let theirs = stamp.clone();
        let finish = move |ctl: &Controller, result: std::result::Result<(), String>| {
            let mut ours = false;
            ctl.publish_login(&stamp, |login, _| {
                if let Some(u) = login.upload.as_mut().filter(|u| u.serial == serial) {
                    u.step = None;
                    u.outcome = Some(result.clone());
                    ours = true;
                }
            });
            // An account change meanwhile discarded it with the rest of the session's state.
            if ours {
                match result {
                    Ok(()) => ctl.notify(NoticeKind::Success, tr("Picture changed on VRChat")),
                    Err(e) => ctl.notify(NoticeKind::Error, tr!("The picture could not be changed: {error}", error = e)),
                }
            }
        };
        let spawned = std::thread::Builder::new().name("vrchat-picture".into()).spawn({
            let finish = finish.clone();
            move || {
                let result = ctl.replace_picture(&scope, &theirs, serial, &id, &path).map_err(|e| format!("{e:#}"));
                finish(&ctl, result);
            }
        });
        if spawned.is_err() {
            finish(self, Err(tr("The upload could not start.").into()));
        }
    }
    fn replace_picture(&self, scope: &Scope, stamp: &super::vrchat_scope::Stamp, serial: u64, id: &str, path: &Path) -> anyhow::Result<()> {
        let png = read_picture(path)?;
        anyhow::ensure!(*scope == Scope::of(&self.view()), "{}", tr("The account changed. Please try again."));
        self.vrchat_source().replace_image(id, &png, |step| {
            self.publish_login(stamp, |login, _| {
                if let Some(u) = login.upload.as_mut().filter(|u| u.serial == serial) {
                    u.step = Some(step);
                }
            });
        })?;
        keep_picture(&orbiters_projects::data_dir(), id, &png);
        Ok(())
    }
}

/// Reads a picked picture file (at most 50 MB) and makes it VRChat's ([`prepare_picture`]).
fn read_picture(path: &Path) -> anyhow::Result<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|_| anyhow::anyhow!("{}", tr("The picture file could not be found.")))?;
    anyhow::ensure!(meta.is_file(), "{}", tr("Pick a picture file."));
    anyhow::ensure!(meta.len() > 0, "{}", tr("That file is empty."));
    anyhow::ensure!(meta.len() <= PICTURE_FILE_LIMIT, "{}", tr("That picture is over 50 MB. Pick a smaller one."));
    prepare_picture(&std::fs::read(path).map_err(|_| anyhow::anyhow!("{}", tr("The picture file could not be read.")))?)
}

/// A picture made VRChat's ([`fit_picture`]), as a PNG.
pub(crate) fn prepare_picture(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder as _;
    let rgb = fit_picture(bytes)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(&mut png, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
    Ok(png)
}

/// A picture made VRChat's: a PNG, JPEG, WebP or GIF (its first frame) of at least 128 pixels a side, turned the way
/// its camera was held, cropped about its centre to 4:3 and made 1200 × 900, opaque (any transparency over black).
/// Cropped rather than letterboxed: VRChat shows these pictures edge to edge at 4:3 (its menus, the website, the SDK's
/// own camera pictures), where bars would show as part of the picture.
pub(crate) fn fit_picture(bytes: &[u8]) -> anyhow::Result<image::RgbImage> {
    use image::ImageDecoder as _;
    let not_a_picture = || anyhow::anyhow!("{}", tr("That file is not a picture VRChat can show (PNG, JPEG, WebP or GIF)."));
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    if reader.format().is_none() {
        return Err(not_a_picture());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| not_a_picture())?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|_| not_a_picture())?;
    picture.apply_orientation(orientation);
    let (w, h) = (picture.width(), picture.height());
    anyhow::ensure!(w >= PICTURE_MIN && h >= PICTURE_MIN, "{}", tr!("That picture is {w} × {h} pixels: pick one of at least {min} on each side.", w = w, h = h, min = PICTURE_MIN));
    let (tw, th) = PICTURE_SIZE;
    let fitted = picture.resize_to_fill(tw, th, image::imageops::FilterType::Lanczos3).to_rgba8();
    Ok(image::RgbImage::from_fn(tw, th, |x, y| {
        let p = fitted.get_pixel(x, y).0;
        let a = p[3] as u32;
        image::Rgb([0, 1, 2].map(|i| ((p[i] as u32 * a + 127) / 255) as u8))
    }))
}

/// After a picture changed on VRChat, the copies this computer keeps (under `data`) show the new one at once: an
/// avatar's Toolkit picture and Orbiters' cached thumbnails of it, each only where one was kept, and a world's cached
/// thumbnail.
fn keep_picture(data: &Path, id: &str, png: &[u8]) {
    let _guard = PICTURE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let name = orbiters_projects::avatars::filename(id);
    let files: Vec<PathBuf> = match Kind::of(id) {
        Some(Kind::Avatar) => std::iter::once(data.join("Avatars").join(format!("{name}.png")))
            .chain(["production", "development", "direct"].map(|server| data.join("avatar-thumbnails").join(server).join(format!("{name}.image"))))
            .filter(|f| f.is_file() || f.parent().is_some_and(|p| p.ends_with("direct")))
            .collect(),
        Some(Kind::World) => vec![data.join("world-thumbnails").join(format!("{name}.image"))],
        None => vec![],
    };
    for file in files {
        let tmp = file.with_extension("tmp");
        let written =
            file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, png)).and_then(|()| std::fs::rename(&tmp, &file));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        } else {
            let _ = std::fs::remove_file(file.with_extension("missing"));
        }
    }
}

/// Share avatar and world fetches across callers, windows and sessions. Serialize the check/fetch/write
/// so concurrent IPC requests cannot both miss the same cache entry. A failed refresh retains the old picture.
fn cached_avatar_picture(file: &Path, fetch: impl FnOnce() -> anyhow::Result<Option<Vec<u8>>>) -> anyhow::Result<Option<super::vrchat_login::Picture>> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let kept = std::fs::read(file).ok().and_then(|b| avatar_picture(&b));
    let fresh = std::fs::metadata(file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 86400);
    if fresh && kept.is_some() {
        return Ok(kept);
    }
    // An avatar VRChat had no picture for (or refused) is not asked for again before a day has passed.
    let missing = file.with_extension("missing");
    let missed = std::fs::metadata(&missing).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 86400);
    if missed && kept.is_none() {
        return Ok(None);
    }
    let miss = || {
        if let Some(parent) = missing.parent() {
            let _ = std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&missing, b""));
        }
    };
    match fetch() {
        Ok(Some(bytes)) => {
            if let Some(picture) = avatar_picture(&bytes) {
                if let Some(parent) = file.parent() {
                    let tmp = file.with_extension("tmp");
                    let _ = std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&tmp, &bytes)).and_then(|()| std::fs::rename(tmp, file));
                }
                let _ = std::fs::remove_file(&missing);
                Ok(Some(picture))
            } else {
                Ok(kept)
            }
        }
        Ok(None) => {
            if kept.is_none() {
                miss();
            }
            Ok(kept)
        }
        Err(_) if kept.is_some() => Ok(kept),
        // A failure (network, rate limit) is not remembered: the window does not ask again before its next start.
        Err(e) => Err(e),
    }
}

fn avatar_picture(bytes: &[u8]) -> Option<super::vrchat_login::Picture> {
    let rgb = orbiters_n4n0_core::picture::rgb_any(bytes).ok()?;
    let image = orbiters_n4n0_core::picture::letterbox(&rgb, 480, 360, [24, 24, 27]);
    let rgba = image.pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    Some(super::vrchat_login::Picture { width: 480, height: 360, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projects_direct_avatar_cache_avoids_fetch_and_refreshes_expired_entries() {
        let dir = std::env::temp_dir().join(format!("orbiters-direct-avatar-test-{}", std::process::id()));
        let file = dir.join("avtr_test.image");
        let input = image::RgbImage::from_pixel(4, 2, image::Rgb([200, 80, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(input).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let calls = std::cell::Cell::new(0);
        let fetch = || {
            calls.set(calls.get() + 1);
            Ok(Some(bytes.get_ref().clone()))
        };
        assert!(cached_avatar_picture(&file, fetch).unwrap().is_some());
        assert!(cached_avatar_picture(&file, fetch).unwrap().is_some());
        assert_eq!(calls.get(), 1);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(172800)).unwrap();
        assert!(cached_avatar_picture(&file, fetch).unwrap().is_some());
        assert_eq!(calls.get(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn projects_missing_picture_expires_and_errors_are_not_cached() {
        let dir = std::env::temp_dir().join(format!("orbiters-missing-{}", uuid::Uuid::new_v4()));
        let file = dir.join("avtr_a.image");
        assert!(cached_avatar_picture(&file, || Ok(None)).unwrap().is_none());
        assert!(file.with_extension("missing").is_file());
        assert!(!file.exists());
        assert!(!file.with_extension("tmp").exists());
        assert!(cached_avatar_picture(&file, || panic!("negative cache missed")).unwrap().is_none());
        std::fs::File::options()
            .write(true)
            .open(file.with_extension("missing"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(172800))
            .unwrap();
        assert!(cached_avatar_picture(&file, || anyhow::bail!("offline")).is_err());
        let png = encoded(image::RgbaImage::from_pixel(4, 3, image::Rgba([1, 2, 3, 255])));
        assert!(cached_avatar_picture(&file, || Ok(Some(png))).unwrap().is_some());
        assert!(!file.with_extension("missing").exists());
        let other = dir.join("avtr_b.image");
        assert!(cached_avatar_picture(&other, || anyhow::bail!("rate limited")).is_err());
        assert!(!other.with_extension("missing").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn projects_avatar_pictures_keep_the_source_aspect_ratio() {
        let input = image::RgbImage::from_pixel(4, 2, image::Rgb([200, 80, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(input).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let p = avatar_picture(bytes.get_ref()).unwrap();
        assert_eq!((p.width, p.height), (480, 360));
        assert_eq!(&p.rgba[..4], &[24, 24, 27, 255]);
        let middle = (180 * 480 + 240) * 4;
        assert_eq!(&p.rgba[middle..middle + 4], &[200, 80, 40, 255]);
        assert!(avatar_picture(b"broken image").is_none());
    }
    #[test]
    fn data_scope_tracks_account_environment_and_session_revocation() {
        let mut s = Snapshot::new(orbiters_core::settings::Settings::default());
        let base = Scope::of(&s);
        s.settings.development = !s.settings.development;
        assert_ne!(base, Scope::of(&s));
        s.settings.development = !s.settings.development;
        s.vrchat_login.allowed = true;
        s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() });
        let signed_in = Scope::of(&s);
        s.vrchat_login.user = None;
        assert_ne!(signed_in, Scope::of(&s));
        let c = Controller::new(s.settings);
        assert!(c.vrchat_data(signed_in, Request::Friends).unwrap_err().contains("account changed"));
    }
    #[test]
    fn unavailable_instance_keeps_unknown_without_using_a_session() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        assert!(matches!(
            c.vrchat_data(Scope::of(&c.view()), Request::Instance { world: "wrld_test".into(), instance: "123".into() }),
            Ok(Response::Instance(None))
        ));
    }
    #[test]
    fn ownership_is_none_without_a_personal_session_and_asked_in_batches() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let scope = Scope::of(&c.view());
        assert!(
            matches!(c.vrchat_data(scope.clone(), Request::Owned { ids: vec!["avtr_a".into(), "wrld_b".into()] }), Ok(Response::Owned(ids)) if ids.is_empty())
        );
        let many = (0..=OWNED_BATCH).map(|i| format!("avtr_{i}")).collect();
        assert!(c.vrchat_data(scope, Request::Owned { ids: many }).unwrap_err().contains("Too many"));
    }
    #[test]
    fn world_lists_and_invites_are_checked_before_any_request() {
        use orbiters_core::maps::WorldList;
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let scope = Scope::of(&c.view());
        let ask = |request| c.vrchat_data(scope.clone(), request).unwrap_err();
        assert!(ask(Request::WorldList { list: WorldList::Search(" a ".into()), page: 0 }).contains("two characters"));
        assert!(ask(Request::WorldList { list: WorldList::Popular, page: 21 }).contains("too far"));
        // Without a personal session: the lists only VRChat has say to sign in, and nothing is sent.
        assert!(ask(Request::WorldList { list: WorldList::Favorites, page: 0 }).contains("Sign in to VRChat"));
        let home = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        assert!(matches!(c.vrchat_data(scope.clone(), Request::InviteSelf { world: home.into(), instance: "48213~region(eu)".into() }),
            Ok(Response::WorldError(WorldError::SignInRequired))));
        for bad in ["48213~private(usr_x)", "123456", "", "1~region(mars)", "12a"] {
            assert!(!public_instance(bad), "{bad}");
            assert!(matches!(c.vrchat_data(scope.clone(), Request::InviteSelf { world: home.into(), instance: bad.into() }),
                Ok(Response::WorldError(WorldError::SignInRequired))));
        }
        assert!(public_instance("7") && public_instance("48213~region(use)"));
        c.update(|s| {
            s.account.state = crate::state::AccountState::SignedIn { username: "tester".into() };
            s.account.development = Server::current() == Server::Development;
            s.vrchat_login.allowed = true;
            s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() });
        });
        assert!(matches!(c.vrchat_data(Scope::of(&c.view()), Request::InviteSelf { world: "wrld_../x".into(), instance: "1".into() }),
            Ok(Response::WorldError(WorldError::InvalidWorld))));
    }
    fn encoded(picture: image::RgbaImage) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(picture).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
    fn near(a: image::Rgb<u8>, b: [u8; 3]) -> bool {
        a.0.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 6)
    }
    #[test]
    fn pictures_are_cropped_about_their_centre_to_vrchats_size() {
        use image::GenericImageView as _;
        let (red, green, blue) = (image::Rgba([220, 30, 30, 255]), image::Rgba([30, 200, 60, 255]), image::Rgba([30, 60, 220, 255]));
        // 1920 × 900 is 1200 × 900 with 360 pixels more on each side: bands of 300 at the edges fall off.
        let wide = image::RgbaImage::from_fn(1920, 900, |x, _| {
            if x < 300 {
                red
            } else if x >= 1620 {
                blue
            } else {
                green
            }
        });
        // 600 × 1200 scales to 1200 × 2400, of which the middle 900 stay: bands of 200 (400 scaled) fall off.
        let tall = image::RgbaImage::from_fn(600, 1200, |_, y| {
            if y < 200 {
                red
            } else if y >= 1000 {
                blue
            } else {
                green
            }
        });
        for source in [wide, tall] {
            let png = prepare_picture(&encoded(source)).unwrap();
            assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png);
            let out = image::load_from_memory(&png).unwrap();
            assert_eq!(out.dimensions(), PICTURE_SIZE);
            let out = out.to_rgb8();
            for (x, y) in [(0, 0), (1199, 0), (0, 899), (1199, 899), (600, 450)] {
                assert!(near(*out.get_pixel(x, y), [30, 200, 60]), "{:?} at {x}, {y}", out.get_pixel(x, y));
            }
        }
        // Opaque: transparency over black.
        let clear = prepare_picture(&encoded(image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 0])))).unwrap();
        assert_eq!(image::load_from_memory(&clear).unwrap().to_rgb8().get_pixel(10, 10).0, [0, 0, 0]);
        // Too small, or not a picture at all.
        assert!(prepare_picture(&encoded(image::RgbaImage::new(100, 400))).unwrap_err().to_string().contains("100 × 400"));
        assert!(prepare_picture(b"definitely not a picture").unwrap_err().to_string().contains("not a picture"));
    }
    #[test]
    fn picked_files_are_checked_before_anything_is_decoded() {
        let dir = std::env::temp_dir().join(format!("orbiters-picture-file-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_picture(&dir.join("missing.png")).unwrap_err().to_string().contains("could not be found"));
        assert!(read_picture(&dir).unwrap_err().to_string().contains("Pick a picture file"));
        std::fs::write(dir.join("empty.png"), b"").unwrap();
        assert!(read_picture(&dir.join("empty.png")).unwrap_err().to_string().contains("empty"));
        std::fs::write(dir.join("ok.png"), encoded(image::RgbaImage::from_pixel(320, 240, image::Rgba([1, 2, 3, 255])))).unwrap();
        assert!(read_picture(&dir.join("ok.png")).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn a_changed_picture_replaces_only_the_copies_kept_of_it() {
        let data = std::env::temp_dir().join(format!("orbiters-picture-test-{}", uuid::Uuid::new_v4()));
        let toolkit = data.join("Avatars").join("avtr_a.png");
        std::fs::create_dir_all(toolkit.parent().unwrap()).unwrap();
        std::fs::write(&toolkit, b"old").unwrap();
        keep_picture(&data, "avtr_a", b"new");
        assert_eq!(std::fs::read(&toolkit).unwrap(), b"new");
        // Direct fetches must see the upload even when no previous picture existed.
        assert_eq!(std::fs::read(data.join("avatar-thumbnails/direct/avtr_a.image")).unwrap(), b"new");
        keep_picture(&data, "avtr_other", b"new");
        assert!(!data.join("Avatars").join("avtr_other.png").exists());
        keep_picture(&data, "wrld_b", b"world");
        assert_eq!(std::fs::read(data.join("world-thumbnails").join("wrld_b.image")).unwrap(), b"world");
        assert_eq!(std::fs::read_dir(data.join("Avatars")).unwrap().count(), 1);
        std::fs::remove_dir_all(&data).unwrap();
    }
    #[test]
    fn pictures_need_a_vrchat_session_and_go_one_at_a_time() {
        use crate::state::AccountState;
        use orbiters_core::account::Server;
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.vrchat_picture(Scope::of(&c.view()), "avtr_a".into(), "picture.png".into());
        assert!(c.view().vrchat_login.upload.is_none());
        assert!(c.take_notices().iter().any(|n| format!("{n:?}").contains("Sign in to VRChat")));
        c.update(|s| {
            s.account.state = AccountState::SignedIn { username: "tester".into() };
            s.account.development = Server::current() == Server::Development;
            s.vrchat_login.allowed = true;
            s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() });
            s.vrchat_login.upload =
                Some(super::super::vrchat_login::PictureUpload { serial: 0, id: "avtr_b".into(), step: Some(Step::Uploading), outcome: None });
        });
        c.vrchat_picture(Scope::of(&c.view()), "avtr_a".into(), "picture.png".into());
        assert_eq!(c.view().vrchat_login.upload.as_ref().unwrap().id, "avtr_b");
        assert!(c.take_notices().iter().any(|n| format!("{n:?}").contains("already uploading")));
        c.vrchat_picture(Scope::of(&c.view()), "usr_test".into(), "picture.png".into());
        assert!(c.take_notices().iter().any(|n| format!("{n:?}").contains("Only avatars")));
    }
    #[test]
    fn chooser_scope_changes_before_and_during_work_discard_results() {
        use std::cell::Cell;
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let scope = Scope::of(&c.view());
        let mut changed = scope.clone(); changed.revision += 1;
        assert!(matches!(scoped_world_work(&scope, || changed.clone(), || panic!("stale work ran")), Response::WorldError(WorldError::AccountChanged)));
        let finished = Cell::new(false);
        let result = scoped_world_work(&scope, || if finished.get() { changed.clone() } else { scope.clone() }, || {
            finished.set(true); Ok(Response::Invited)
        });
        assert!(matches!(result, Response::WorldError(WorldError::AccountChanged)));
    }
    #[test]
    fn chooser_permits_only_exact_listed_world_account_and_revision() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let scope = Scope::of(&c.view());
        let item = orbiters_core::vrchat_api::WorldInstance {world:"wrld_test".into(),id:"1~hidden(usr_test)".into(),
            access:orbiters_core::vrchat_api::InstanceAccess::FriendsPlus,region:"eu".into(),occupants:1,capacity:Some(20),friends:None,full:false,closed:false,can_join:true};
        let mut permits = InstancePermits::default();
        assert!(!permits.contains(&scope,"wrld_test",&item.id));
        permits.allow(&scope,"wrld_test",std::slice::from_ref(&item));
        assert!(permits.contains(&scope,"wrld_test",&item.id));
        assert!(!permits.contains(&scope,"wrld_test","2"));
        assert!(!permits.contains(&scope,"wrld_other",&item.id));
        let mut changed=scope.clone();changed.revision+=1;
        assert!(!permits.contains(&changed,"wrld_test",&item.id));
        permits.allow(&scope,"wrld_test",&[]);
        assert!(!permits.contains(&scope,"wrld_test",&item.id));
    }
    #[test]
    fn chooser_service_refuses_unlisted_ids_before_any_http() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.update(|s| {
            s.account.state=crate::state::AccountState::SignedIn {username:"tester".into()};
            s.account.development=Server::current()==Server::Development;
            s.vrchat_login.allowed=true; s.vrchat_login.user=Some(User {id:"usr_test".into(),..Default::default()});
        });
        let result=c.vrchat_data(Scope::of(&c.view()),Request::InviteSelf {
            world:"wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b".into(),instance:"12345".into()
        }).unwrap();
        assert!(matches!(result,Response::WorldError(WorldError::NotListed)));
    }
    const REVIEW_WORLD: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
    const REVIEW_ID: &str = "123~private(usr_test)~region(eu)~nonce(opaque)";
    fn review_instance() -> serde_json::Value {
        serde_json::json!({"worldId":REVIEW_WORLD,"instanceId":REVIEW_ID,"accessType":"invite","ownerId":"usr_test","region":"eu","n_users":1,"capacity":20,"active":true})
    }
    #[test]
    fn review_service_list_create_fresh_lookup_and_invite() {
        for create in [false, true] {
            let ctl = Controller::new(orbiters_core::settings::Settings::default());
            let mut replies = if create { vec![(200,review_instance())] } else { vec![(200,serde_json::json!({"instances":[[REVIEW_ID,1,{}]]})),(200,review_instance())] };
            replies.extend([(200,review_instance()),(200,serde_json::json!({}))]);
            let (_, calls) = ctl.test_vrchat(replies);
            let scope = Scope::of(&ctl.view());
            let request = if create { Request::CreateInstance { world: REVIEW_WORLD.into(), access: orbiters_core::vrchat_api::CreateAccess::Invite, region: orbiters_core::vrchat_api::InstanceRegion::Eu } } else { Request::WorldInstances {world:REVIEW_WORLD.into()} };
            let result = ctl.vrchat_data(scope.clone(),request).unwrap();
            assert!(matches!(result,Response::CreatedInstance(_) | Response::WorldInstances(_)), "{result:?}");
            assert!(matches!(ctl.vrchat_data(scope,Request::InviteSelf {world:REVIEW_WORLD.into(),instance:REVIEW_ID.into()}).unwrap(),Response::Invited));
            let calls = calls.lock().unwrap();
            assert_eq!(calls.last().unwrap().0,"POST");
            assert!(calls.last().unwrap().1.contains("/invite/myself/to/"));
            assert!(calls.last().unwrap().1.ends_with(&format!("/invite/myself/to/{REVIEW_WORLD}:{REVIEW_ID}")));
        }
    }
    #[test]
    fn review_service_fresh_refusals_and_transport_errors() {
        for mode in ["full","closed","null","401","429"] {
            let ctl = Controller::new(orbiters_core::settings::Settings::default());
            let mut fresh = review_instance();
            if mode=="full" { fresh["full"]=true.into(); }
            if mode=="closed" { fresh["hardClose"]=true.into(); }
            if mode=="null" { fresh=serde_json::Value::Null; }
            let status = mode.parse().unwrap_or(200);
            let mut replies = vec![(200,review_instance()),(status,fresh)];
            // r20: a 401 is looked into once (who is signed in) before the session counts as ended.
            if status == 401 { replies.push((401,serde_json::json!({}))); }
            let (_, calls) = ctl.test_vrchat(replies);
            let scope=Scope::of(&ctl.view());
            ctl.vrchat_data(scope.clone(),Request::CreateInstance {world:REVIEW_WORLD.into(),access:orbiters_core::vrchat_api::CreateAccess::Invite,region:orbiters_core::vrchat_api::InstanceRegion::Eu}).unwrap();
            let result=ctl.vrchat_data(scope,Request::InviteSelf {world:REVIEW_WORLD.into(),instance:REVIEW_ID.into()}).unwrap();
            assert!(matches!(result,Response::WorldError(_)),"{mode}: {result:?}");
            assert!(!calls.lock().unwrap().iter().any(|(_,u)|u.contains("/invite/myself")));
        }
        for status in [401,429] {
            let ctl=Controller::new(orbiters_core::settings::Settings::default());
            ctl.test_vrchat(if status == 401 { vec![(401,serde_json::json!({})),(401,serde_json::json!({}))] } else { vec![(status,serde_json::json!({}))] });
            assert!(matches!(ctl.vrchat_data(Scope::of(&ctl.view()),Request::CreateInstance {world:REVIEW_WORLD.into(),access:orbiters_core::vrchat_api::CreateAccess::Invite,region:orbiters_core::vrchat_api::InstanceRegion::Eu}).unwrap(),Response::WorldError(WorldError::Api(_))));
        }
    }
    #[test]
    fn review_signout_acceptance_invalidates_queued_create_before_unlock() {
        let ctl=Controller::new(orbiters_core::settings::Settings::default());
        let (client,calls)=ctl.test_vrchat(vec![(200,serde_json::json!({}))]);
        let locked=client.lock().unwrap();
        let source=ctl.vrchat_source();
        let scope=Scope::of(&ctl.view());
        ctl.vrchat_command(super::super::vrchat_login::Command::SignOut);
        assert_ne!(scope,Scope::of(&ctl.view()),"sign-out must invalidate synchronously");
        assert!(ctl.view().vrchat_login.user.is_none(),"new requests must also be refused while logout waits");
        drop(locked);
        let result=source.personal("usr_test",|| true,|c|c.create_instance(REVIEW_WORLD,orbiters_core::vrchat_api::CreateAccess::Invite,orbiters_core::vrchat_api::InstanceRegion::Eu));
        assert!(result.is_err());
        assert!(!calls.lock().unwrap().iter().any(|(m,_)|m=="POST"));
    }
    #[test]
    fn review_expired_queued_create_never_posts_and_permits_expire() {
        let ctl=Controller::new(orbiters_core::settings::Settings::default());
        let (_,calls)=ctl.test_vrchat(vec![]);
        let result=ctl.vrchat_data_until(Scope::of(&ctl.view()),Request::CreateInstance {world:REVIEW_WORLD.into(),access:orbiters_core::vrchat_api::CreateAccess::Invite,region:orbiters_core::vrchat_api::InstanceRegion::Eu},std::time::Instant::now()-std::time::Duration::from_secs(1)).unwrap();
        assert!(matches!(result,Response::WorldError(_)));
        assert!(calls.lock().unwrap().is_empty());
        let scope=Scope::of(&ctl.view());
        let permits=InstancePermits {entries:vec![(scope.clone(),REVIEW_WORLD.into(),REVIEW_ID.into(),std::time::Instant::now()-std::time::Duration::from_secs(301))],..Default::default()};
        assert!(!permits.contains(&scope,REVIEW_WORLD,REVIEW_ID));
    }

    #[test]
    fn review_invite_post_errors_are_not_success_and_scope_changes_do_no_http() {
        for status in [401,429,500] {
            let ctl=Controller::new(orbiters_core::settings::Settings::default());
            let mut replies=vec![(200,review_instance()),(200,review_instance()),(status,serde_json::json!({}))];
            if status==401 { replies.push((401,serde_json::json!({}))); }
            let (_,calls)=ctl.test_vrchat(replies);
            let scope=Scope::of(&ctl.view());
            ctl.vrchat_data(scope.clone(),Request::CreateInstance {world:REVIEW_WORLD.into(),access:orbiters_core::vrchat_api::CreateAccess::Invite,region:orbiters_core::vrchat_api::InstanceRegion::Eu}).unwrap();
            let reply=ctl.vrchat_data(scope.clone(),Request::InviteSelf {world:REVIEW_WORLD.into(),instance:REVIEW_ID.into()}).unwrap();
            assert!(matches!(reply,Response::WorldError(WorldError::Api(_))));
            if status==500 { assert!(matches!(reply,Response::WorldError(WorldError::Api(orbiters_core::vrchat_api::Error::OutcomeUnknown)))); }
            let before=calls.lock().unwrap().len();
            ctl.update(|s|s.vrchat_login.revision+=1);
            assert!(matches!(ctl.vrchat_data(scope,Request::InviteSelf {world:REVIEW_WORLD.into(),instance:REVIEW_ID.into()}).unwrap(),Response::WorldError(WorldError::AccountChanged)));
            assert_eq!(calls.lock().unwrap().len(),before);
        }
    }
    #[test]
    fn review_continuation_uses_server_ids_and_releases_bounded_batches() {
        let ctl=Controller::new(orbiters_core::settings::Settings::default());
        let ids:Vec<_>=(0..122).map(|n|format!("{n}~region(eu)")).collect();
        let mut replies=vec![(200,serde_json::json!({"instances":ids.iter().map(|id|serde_json::json!([id,1,{}])).collect::<Vec<_>>()}))];
        replies.extend(ids.iter().take(32).map(|id|{let mut v=review_instance();v["instanceId"]=id.clone().into();v["accessType"]="public".into();(200,v)}));
        let (_,calls)=ctl.test_vrchat(replies);
        let scope=Scope::of(&ctl.view());
        let Response::WorldInstances(first)=ctl.vrchat_data(scope.clone(),Request::WorldInstances {world:REVIEW_WORLD.into()}).unwrap() else {panic!("no listing")};
        assert_eq!(first.items.len(),16);assert_eq!(first.remaining.len(),106);
        let bad=ctl.vrchat_data(scope.clone(),Request::WorldInstancesNext {world:REVIEW_WORLD.into(),instances:vec!["arbitrary".into()]}).unwrap();
        assert!(matches!(bad,Response::WorldError(WorldError::NotListed)));
        let Response::WorldInstances(next)=ctl.vrchat_data(scope,Request::WorldInstancesNext {world:REVIEW_WORLD.into(),instances:first.remaining}).unwrap() else {panic!("no continuation")};
        assert_eq!(next.items.len(),16);assert_eq!(next.remaining.len(),90);assert_eq!(calls.lock().unwrap().len(),33);
    }

    #[test]
    fn review_changed_account_environment_and_user_refuse_before_http() {
        let ctl=Controller::new(orbiters_core::settings::Settings::default());
        let (_,calls)=ctl.test_vrchat(vec![]);
        let scope=Scope::of(&ctl.view());
        for change in 0..3 {
            let mut stale=scope.clone();
            match change {0=>stale.member=Some("other".into()),1=>stale.user=Some("usr_other".into()),_=>stale.development=!stale.development}
            assert!(matches!(ctl.vrchat_data(stale,Request::CreateInstance {world:REVIEW_WORLD.into(),access:orbiters_core::vrchat_api::CreateAccess::Public,region:orbiters_core::vrchat_api::InstanceRegion::Eu}).unwrap(),Response::WorldError(WorldError::AccountChanged)));
        }
        assert!(calls.lock().unwrap().is_empty());
    }

    /// VRChat's answer to making a Friends+ instance (2026-11-15, the fields read): nobody in it yet, `active: false`.
    fn made_answer(world: &str) -> serde_json::Value {
        serde_json::json!({"worldId":world,"instanceId":MADE_ID,"accessType":"friendsPlus","ownerId":"usr_test","region":"eu",
            "n_users":0,"capacity":80,"full":false,"active":false,"users":[]})
    }
    const MADE_ID: &str = "46657~hidden(usr_test)~region(eu)";
    // Worlds of their own (permits are per controller since BUILD-7; kept apart all the same).
    const MADE_WORLD: &str = "wrld_266523e8-9161-40da-acd0-6bd82e075833";
    const KEEP_WORLD: &str = "wrld_266523e8-9161-40da-acd0-6bd82e075834";
    const DIRECT_WORLDS: [&str; 3] = ["wrld_266523e8-9161-40da-acd0-6bd82e075835", "wrld_266523e8-9161-40da-acd0-6bd82e075836", "wrld_266523e8-9161-40da-acd0-6bd82e075837"];
    fn fake_pc(ctl: &Controller, fake: crate::controller::travel::tests::Fake) -> std::sync::Arc<crate::controller::travel::tests::Fake> {
        let fake = std::sync::Arc::new(fake);
        *ctl.travel.lock().unwrap() = Some(fake.clone() as std::sync::Arc<dyn crate::controller::travel::Travel>);
        fake
    }

    /// BUILD-7: a listing of the world between Make and Go (the page refreshing) does not forget the room just made,
    /// which VRChat may not list yet; and another controller's listing (another test) never touches this one's.
    #[test]
    fn a_made_instance_survives_a_new_listing_of_its_world() {
        let other = Controller::new(orbiters_core::settings::Settings::default());
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = ctl.test_vrchat(vec![(200, made_answer(REVIEW_WORLD)), (200, serde_json::json!({"instances": []})), (200, made_answer(REVIEW_WORLD)), (200, serde_json::json!({}))]);
        let _ = other.test_vrchat(vec![(200, serde_json::json!({"instances": []}))]);
        let scope = Scope::of(&ctl.view());
        let Response::CreatedInstance(made) = ctl.vrchat_data(scope.clone(), Request::CreateInstance { world: REVIEW_WORLD.into(), access: orbiters_core::vrchat_api::CreateAccess::FriendsPlus, region: orbiters_core::vrchat_api::InstanceRegion::Eu }).unwrap() else { panic!("not made") };
        let Response::WorldInstances(listed) = ctl.vrchat_data(scope.clone(), Request::WorldInstances { world: REVIEW_WORLD.into() }).unwrap() else { panic!("no listing") };
        assert!(listed.items.is_empty(), "VRChat does not list it yet");
        let elsewhere = other.vrchat_data(Scope::of(&other.view()), Request::WorldInstances { world: REVIEW_WORLD.into() }).unwrap();
        assert!(matches!(elsewhere, Response::WorldInstances(_)), "{elsewhere:?}");
        let reply = ctl.vrchat_data(scope, Request::InviteSelf { world: REVIEW_WORLD.into(), instance: made.id }).unwrap();
        assert!(matches!(reply, Response::Invited), "{reply:?}");
        assert_eq!(calls.lock().unwrap().len(), 4);
    }

    /// Enzo's press (L-invite-error): Make a new one, Friends+, then the invite. A new instance said `active: false`, read
    /// as closed, never got its permit, and the invite was refused as "no longer on the list".
    #[test]
    fn a_new_instance_can_be_invited_to_at_once() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let (_, calls) = ctl.test_vrchat(vec![(200, made_answer(MADE_WORLD)), (200, made_answer(MADE_WORLD)), (200, serde_json::json!({}))]);
        let scope = Scope::of(&ctl.view());
        let Response::CreatedInstance(made) = ctl.vrchat_data(scope.clone(), Request::CreateInstance { world: MADE_WORLD.into(), access: orbiters_core::vrchat_api::CreateAccess::FriendsPlus, region: orbiters_core::vrchat_api::InstanceRegion::Eu }).unwrap() else { panic!("not made") };
        assert!(made.can_join);
        let reply = ctl.vrchat_data(scope, Request::InviteSelf { world: MADE_WORLD.into(), instance: made.id }).unwrap();
        assert!(matches!(reply, Response::Invited), "{reply:?}");
        assert!(calls.lock().unwrap().last().unwrap().1.ends_with(&format!("/invite/myself/to/{MADE_WORLD}:{MADE_ID}")));
    }

    #[test]
    fn making_one_keeps_the_listed_ones_open_to_go_to() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let kept = || { let mut v = review_instance(); v["worldId"] = KEEP_WORLD.into(); v };
        ctl.test_vrchat(vec![(200, serde_json::json!({"instances":[[REVIEW_ID,1,{}]]})), (200, kept()), (200, made_answer(KEEP_WORLD)), (200, kept())]);
        let pc = fake_pc(&ctl, crate::controller::travel::tests::Fake { running: vec![10], server: Some(10), ..Default::default() });
        let scope = Scope::of(&ctl.view());
        ctl.vrchat_data(scope.clone(), Request::WorldInstances { world: KEEP_WORLD.into() }).unwrap();
        ctl.vrchat_data(scope.clone(), Request::CreateInstance { world: KEEP_WORLD.into(), access: orbiters_core::vrchat_api::CreateAccess::FriendsPlus, region: orbiters_core::vrchat_api::InstanceRegion::Eu }).unwrap();
        let reply = ctl.vrchat_data(scope, Request::GoThere { world: KEEP_WORLD.into(), instance: REVIEW_ID.into() }).unwrap();
        assert!(matches!(reply, Response::Went(crate::controller::travel::Went::Direct)), "{reply:?}");
        assert_eq!(pc.written.lock().unwrap().len(), 1);
    }

    /// L-go-direct: a new instance, then straight there through the person's own VRChat; no invite is sent.
    #[test]
    fn go_there_takes_the_person_directly_and_invites_only_where_it_cannot() {
        use crate::controller::travel::{tests::Fake, Detour, Went};
        let make = |world: &str| Request::CreateInstance { world: world.into(), access: orbiters_core::vrchat_api::CreateAccess::FriendsPlus, region: orbiters_core::vrchat_api::InstanceRegion::Eu };
        let go = |world: &str| Request::GoThere { world: world.into(), instance: MADE_ID.into() };
        // The person's VRChat owns the link pipe.
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let world = DIRECT_WORLDS[0];
        let (_, calls) = ctl.test_vrchat(vec![(200, made_answer(DIRECT_WORLDS[0])), (200, made_answer(DIRECT_WORLDS[0]))]);
        let pc = fake_pc(&ctl, Fake { running: vec![10], server: Some(10), ..Default::default() });
        let scope = Scope::of(&ctl.view());
        ctl.vrchat_data(scope.clone(), make(world)).unwrap();
        assert!(matches!(ctl.vrchat_data(scope, go(world)).unwrap(), Response::Went(Went::Direct)));
        assert_eq!(*pc.written.lock().unwrap(), [format!("vrchat://launch?ref=orbiters.cc&id={world}:{MADE_ID}")]);
        assert!(!calls.lock().unwrap().iter().any(|(_, u)| u.contains("/invite/")), "no invite to accept");
        // A bridge side session owns the pipe: nothing written there, the invite goes, and says why.
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let world = DIRECT_WORLDS[1];
        let (_, calls) = ctl.test_vrchat(vec![(200, made_answer(DIRECT_WORLDS[1])), (200, made_answer(DIRECT_WORLDS[1])), (200, serde_json::json!({}))]);
        let pc = fake_pc(&ctl, Fake { running: vec![20, 10], server: Some(20), side_args: vec![20], ..Default::default() });
        let scope = Scope::of(&ctl.view());
        ctl.vrchat_data(scope.clone(), make(world)).unwrap();
        assert!(matches!(ctl.vrchat_data(scope, go(world)).unwrap(), Response::Went(Went::Invited(Detour::OtherVrchat))));
        assert!(pc.written.lock().unwrap().is_empty());
        assert!(calls.lock().unwrap().last().unwrap().1.contains("/invite/myself/to/"));
        // The bridge's main VRChat alone: nothing moved, nothing sent.
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let world = DIRECT_WORLDS[2];
        let (_, calls) = ctl.test_vrchat(vec![(200, made_answer(DIRECT_WORLDS[2])), (200, made_answer(DIRECT_WORLDS[2]))]);
        ctl.sides.set_main(Some(30));
        fake_pc(&ctl, Fake { running: vec![30], server: Some(30), ..Default::default() });
        let scope = Scope::of(&ctl.view());
        ctl.vrchat_data(scope.clone(), make(world)).unwrap();
        assert!(matches!(ctl.vrchat_data(scope, go(world)).unwrap(), Response::WorldError(WorldError::BridgeBusy)));
        assert!(!calls.lock().unwrap().iter().any(|(_, u)| u.contains("/invite/")));
    }

}
