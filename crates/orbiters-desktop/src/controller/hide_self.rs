//! Pictures and recordings without the wearer (contracts' `Docs/rig-frames.md`), without the other players, and saying why
//! a recording has no strip.
//!
//! N4N0's capture kit is part of the avatar, so VRChat's camera mask "Local User" culls it with the avatar: in Enzo's
//! sessions of 2026-11-03 the same avatar gave a strip on 678 of 748 frames with the mask on and on 0 of 509 with it off.
//! No strip is no pose, no depth, no keyframe. The mask is also how a person says "leave me out", and a 3D picture of the
//! world leaves the avatars out whatever the mask says, so the app keeps that promise another way: it keeps Local User on
//! while it records, and has the avatar's own animator turn off every renderer but the kit's and the rig's, on the wearer's
//! client only. Nothing of the wearer is then in the colour, the depth or the rig's faces, so nothing has to be erased
//! afterwards.
//!
//! The order matters. The avatar goes first, and nothing is recorded (the worker gets no Start) and the mask is not turned on
//! until it is gone, so no frame ever has both a pose and the wearer. At the end the mask goes back first, then the avatar.
//!
//! **The avatar always comes back.** A stuck invisible avatar is the worst this can do, so the app holds it hidden rather
//! than switching it: the [`beat`](super::Controller::hide_beat) flips `N4N0/Capture/HideSelf` every [`BEAT`], and the
//! animator lets go by itself about 1.5 s after the last flip (Toolkit's `N4N0Installer.HideSelf`). A crash, a closed
//! OSC port or a frozen app all end the same way, with the avatar shown, and loading an avatar anew starts it shown.
//!
//! **Hiding is proved, not assumed.** Enzo's seven recordings of 2026-11-04 from 22:14 had Local User off: the app turned it on
//! as it should, and his avatar stayed in all seven. That upload, The 1MB Ultirex, was built on 2026-11-02 with Toolkit 0.4.11,
//! which has no such layer (0.4.18 added it), and VRChat's tree, its stored list of the avatar's parameters
//! (`OSC/usr_.../Avatars/avtr_....json`, 272 parameters, written 2026-11-03) and all 172 N4N0 parameters in those recordings lack
//! `N4N0/Capture/HideSelf`. The only word about it was one toast on the computer, during a flight in a headset. So now: VRChat
//! echoes every value it takes for a parameter the avatar has (`N4N0/Capture/Rig` and `Follow` within milliseconds, in all those
//! recordings) and nothing for one it has not (`N4N0/Capture/Record` on the kitless avatar of 2026-11-03: not one N4N0
//! parameter came back). The app flips, and waits for that echo up to [`PROOF_WAIT`]. Without it, the avatar is in the
//! picture, and the person reads why and what to do on the menus, the toast and the Capture page, at that moment
//! ([`cannot_hide_text`]).
//!
//! **What happens to the wearer then.** Nothing from outside can change an uploaded avatar, so the frames show them: the capture
//! goes on, and reconstruction takes them out afterwards (orbiters_reconstruct::wearer, told by `picture.json` or the recording's
//! camera mask). On Enzo's seven recordings that left the place as it was with nothing of the avatar but its shadow, so this is no
//! longer said as a failure; an upload with the current Toolkit stays the cleaner way, as nothing has to be guessed, and the
//! Capture page offers it in a press.
//!
//! **The others.** `/usercamera/RemotePlayer` is the mask's other half: read and set like Local User, and put back at the end
//! exactly as it was. No recording had it off with Local User on, so whether the kit survives it is unknown (it should: the kit
//! draws for its wearer only). If the strip goes away while it is off, it is turned back on and the person is told
//! ([`Controller::others_cost_the_strip`](super::Controller::others_cost_the_strip)). The rig's six cameras have no such mask
//! (Toolkit's `RigHiddenLayers` leaves the player layers in): with other players in the instance, a picture of the world
//! flies without the rig (burst.rs).
//!
//! What VRChat really does with the message: `/usercamera/LocalPlayer` is documented as "Get/Toggle". Enzo's sessions of
//! 2026-11-04 show a bool message sets it: initially off, then on as the app sent, three echoes, and off again at the next
//! recording's start. The app still reads it back after [`CONFIRM`] and, if VRChat kept the mask off, lets the avatar go
//! and says so.

use super::Shared;
use crate::controller::NoticeKind;
use crate::state::NoStrip;
use orbiters_contracts::avatar::AvatarManifest;
use orbiters_core::clock;
use orbiters_core::osc::{OscValue, AVATAR_CHANGE, HIDE_SELF_PARAMETER, PARAMETERS_PREFIX, RECORD_PARAMETER};
use orbiters_core::picture::Left;
use std::time::{Duration, Instant};

/// VRChat's camera mask "Local User" (read and set; VRChat 2025.3.3 and later).
pub const LOCAL_PLAYER: &str = "/usercamera/LocalPlayer";
/// VRChat's camera mask "Remote User", every other player.
pub const REMOTE_PLAYER: &str = "/usercamera/RemotePlayer";
/// How often the avatar is told again to stay hidden: six of these fit in the animator's 1.5 s, so a datagram lost or a
/// busy moment shows nothing.
const BEAT: Duration = Duration::from_millis(250);
/// From the first beat to the mask or the recording: past one flip (a beat) and a few animator frames at any frame rate.
const SETTLE: Duration = Duration::from_millis(350);
/// How long VRChat gets to echo a flip. Only a value that changes comes back, so the first flip may say nothing: this is past
/// the second and the third.
const PROOF_WAIT: Duration = Duration::from_millis(650);
/// How long VRChat gets to show it took the mask.
const CONFIRM: Duration = Duration::from_millis(1200);
/// A person turning the mask off again is answered this often at most.
const REASSERT: Duration = Duration::from_secs(1);
/// Frames without a strip for this long are told, with the reason.
pub const NO_STRIP_AFTER_S: f32 = 2.0;
/// Frames without a strip for this long with Remote User off put it back: a picture is seconds long, and it is worth more
/// than leaving the others out. The longest stretch without one in Enzo's 13 flown recordings of 2026-11-04 was 0.44 s, and
/// the first frame always had one.
const OTHERS_COST_S: f32 = 1.0;
/// The first Toolkit whose builds add the layer.
const HIDE_SELF_TOOLKIT: (u32, u32, u32) = (0, 4, 18);

const WITHOUT_YOU: &str = "Recording without your avatar, as Local User is off in VRChat's camera: it is hidden on your side while this records, and comes back after. Others still see you.";
const NOT_CONFIRMED: &str =
    "VRChat did not confirm your avatar hid itself. Check this capture: if your avatar shows, Reconstruct without my avatar takes it out, and an upload with the current Orbiters Toolkit keeps it out cleanly.";
const NOT_TAKEN: &str =
    "VRChat kept Local User off, so your avatar is shown again and this recording has no keyframes. Turn Local User on in the camera's Mask menu.";
const OTHERS_KEPT: &str = "VRChat kept Remote User on, so other players are in this picture.";
const OTHERS_UNKNOWN: &str = "VRChat's camera has no Remote User mask to set here (OSC, or a VRChat before 2025.3.3), so other players are in this picture.";
/// For the menus' short line.
const OTHERS_SHOW: &str = "Other players show in this picture";
const OTHERS_COST: &str = "With Remote User off, N4N0's capture kit stopped drawing, so it was turned back on: other players are in this picture.";

/// What a recording does about its wearer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The avatar stays as it is: asked for, or an avatar without the kit, or the camera is not the app's to change.
    Leave,
    /// The avatar hidden, then, with the mask off, Local User on.
    Hide { turn_mask_on: bool },
}

/// `mask`: Local User as VRChat says. `kit`: the worn avatar has the capture kit, `None` when VRChat does not say.
/// `always`: a picture of the world, which leaves the wearer out whatever the mask says.
pub fn plan(may_drive: bool, mask: Option<bool>, kit: Option<bool>, always: bool) -> Plan {
    if !may_drive || kit == Some(false) {
        return Plan::Leave;
    }
    match (mask, kit) {
        (Some(false), Some(true)) => Plan::Hide { turn_mask_on: true },
        // VRChat does not say whether this avatar has the kit: the mask stays, on no guess.
        (Some(false), None) if !always => Plan::Leave,
        (_, _) if always => Plan::Hide { turn_mask_on: false },
        _ => Plan::Leave,
    }
}

/// The Toolkit that wrote an avatar manifest ("Orbiters Toolkit 0.4.11").
fn toolkit_version(generator: &str) -> Option<(u32, u32, u32)> {
    let version = generator.strip_prefix("Orbiters Toolkit ")?;
    let mut parts = version.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// What the person reads when the avatar could not hide: why, from what the upload's manifest says, and what to do.
pub fn cannot_hide_text(name: Option<&str>, generator: Option<&str>) -> String {
    let avatar = name.unwrap_or("this avatar");
    let (a, b, c) = HIDE_SELF_TOOLKIT;
    const AFTER: &str = "It is in the frames, and comes out of the 3D view when this capture is reconstructed.";
    match generator.and_then(|g| toolkit_version(g).map(|v| (v, g))) {
        Some((version, generator)) if version >= HIDE_SELF_TOOLKIT => {
            let toolkit = generator.strip_prefix("Orbiters ").unwrap_or(generator);
            format!(
                "{avatar} could not hide itself this time: it was uploaded with {toolkit}, but VRChat keeps an old list of its parameters. {AFTER} For a cleaner result, with nothing guessed and no shadow left: in VRChat's Action Menu press Options, OSC, Reset Config, then change avatar and back."
            )
        }
        Some((_, generator)) => {
            let toolkit = generator.strip_prefix("Orbiters ").unwrap_or(generator);
            format!("{avatar} was uploaded with {toolkit}, which cannot hide it while the camera records. {AFTER} An upload with Orbiters Toolkit {a}.{b}.{c} or later gives a cleaner result, with nothing guessed and no shadow left.")
        }
        None => format!("{avatar} did not answer the message that hides it. {AFTER} An upload with Orbiters Toolkit {a}.{b}.{c} or later gives a cleaner result, with nothing guessed and no shadow left."),
    }
}

/// Whether the upload of `avatar_id` can hide itself, as far as its manifest tells: `None` when no manifest says.
pub fn upload_can_hide(avatar_id: &str) -> Option<bool> {
    let file = orbiters_core::paths::avatars_dir().join(orbiters_contracts::avatar::file_name(avatar_id));
    let manifest = AvatarManifest::parse(&std::fs::read_to_string(file).ok()?).ok()?;
    Some(toolkit_version(manifest.generator.as_deref()?)? >= HIDE_SELF_TOOLKIT)
}

/// The name of the avatar `avatar_id`, as its manifest says.
pub fn uploaded_avatar_name(avatar_id: &str) -> Option<String> {
    let file = orbiters_core::paths::avatars_dir().join(orbiters_contracts::avatar::file_name(avatar_id));
    AvatarManifest::parse(&std::fs::read_to_string(file).ok()?).ok()?.name
}

/// Why frames come without a strip, from what VRChat says. The mask first: it culls the kit whatever the avatar has.
pub fn why_no_strip(mask: Option<bool>, kit: Option<bool>) -> NoStrip {
    match (mask, kit) {
        (Some(false), Some(false)) | (None, Some(false)) | (Some(true), Some(false)) => NoStrip::NoKit,
        (Some(false), _) => NoStrip::MaskOff,
        (Some(true), Some(true)) => NoStrip::NotDrawn,
        _ => NoStrip::Unknown,
    }
}

/// The current recording's state of all this.
#[derive(Default)]
pub struct HideRuntime {
    /// The beat of the current recording: a new value ends the last one.
    generation: u64,
    /// A beat is running.
    hiding: bool,
    /// The app turned Local User on, and turns it off again at the end.
    turned_on: bool,
    /// The app turned Remote User off, and turns it on again at the end.
    others_off: bool,
    /// Remote User off cost the strip once: this run of the app no longer tries.
    others_cost_strip: bool,
    /// The camera is the app's to change in this recording.
    may_drive: bool,
    /// A picture of the world: hidden whatever the mask says.
    always: bool,
    /// The worn avatar has the kit, as VRChat said at the start or since it changed.
    kit: Option<bool>,
    /// Local User as VRChat said at the start (later changes are in the OSC values).
    mask: Option<bool>,
    /// Told about frames without a strip, for the stretch under way.
    told: bool,
    last_reassert: Option<Instant>,
}

fn value_of<'a>(vrchat: &'a [(String, OscValue)], path: &str) -> Option<&'a OscValue> {
    vrchat.iter().find(|(p, _)| p == path).map(|(_, v)| v)
}

fn parameter(name: &str) -> String {
    format!("{PARAMETERS_PREFIX}{name}")
}

/// What VRChat's tree says about the worn avatar: whether it has the kit (`None`: no tree, or no answer), and the layer.
fn avatar_support(vrchat: &[(String, OscValue)]) -> (Option<bool>, bool) {
    let says = vrchat.iter().any(|(p, _)| p.starts_with(PARAMETERS_PREFIX));
    let kit = says.then(|| value_of(vrchat, &parameter(RECORD_PARAMETER)).is_some());
    (kit, value_of(vrchat, &parameter(HIDE_SELF_PARAMETER)).is_some())
}

impl super::Controller {
    /// The mask as VRChat last said it: what it sent since, else what its tree said at the start.
    fn local_user(&self) -> Option<bool> {
        let heard = self.osc_values.lock().unwrap().latest.get(LOCAL_PLAYER).and_then(|(v, _)| v.first().map(OscValue::truthy));
        heard.or(self.hide.lock().unwrap().mask)
    }

    /// Whether this recording holds the wearer's avatar hidden.
    pub(super) fn hiding_self(&self) -> bool {
        self.hide.lock().unwrap().hiding
    }

    /// Whether Remote User is off for this recording's sake.
    pub(super) fn hiding_others(&self) -> bool {
        self.hide.lock().unwrap().others_off
    }

    /// A recording starts: `vrchat` is VRChat's tree as read for it. `world`: it is a picture of the world. With an avatar to
    /// hide, this returns when it is hidden or known not to hide, before anything is recorded: at most [`PROOF_WAIT`].
    pub(super) fn start_hide_self(self: &Shared, vrchat: &[(String, OscValue)], may_drive: bool, world: bool) {
        let (kit, listed) = avatar_support(vrchat);
        let mask = value_of(vrchat, LOCAL_PLAYER).map(OscValue::truthy).or_else(|| self.local_user());
        {
            let mut hide = self.hide.lock().unwrap();
            hide.may_drive = may_drive;
            hide.always = world;
            hide.kit = kit;
            hide.mask = mask;
            hide.told = false;
        }
        if world && may_drive {
            self.leave_others_out(vrchat);
        }
        self.apply_plan(plan(may_drive, mask, kit, world), listed);
    }

    fn apply_plan(self: &Shared, plan: Plan, listed: bool) {
        let Plan::Hide { turn_mask_on } = plan else { return };
        let world = self.hide.lock().unwrap().always;
        let hidden = self.hide_and_wait(listed);
        // Without the mask on the kit draws nothing, so it goes on either way: a capture with the avatar in it beats none.
        if turn_mask_on {
            self.turn_local_user_on(Duration::ZERO);
        }
        match hidden {
            Some(proved) => {
                self.update(|s| s.capture.left_out.wearer = Left::Out);
                if !proved {
                    self.tell_about_picture(NoticeKind::Warning, NOT_CONFIRMED, "Your avatar may show: check this picture");
                } else if !world {
                    self.notify(NoticeKind::Success, WITHOUT_YOU);
                }
            }
            None => self.say_cannot_hide(),
        }
    }

    /// Holds the avatar hidden and waits for VRChat's word: `Some(true)` when it echoed a flip, `Some(false)` with no echo from
    /// an avatar its tree says has the parameter (hidden, it seems), `None` when the avatar has nothing that hides it (the
    /// beat ended).
    fn hide_and_wait(self: &Shared, listed: bool) -> Option<bool> {
        let began = Instant::now();
        let began_ns = clock::now_ns();
        self.begin_beat();
        let address = parameter(HIDE_SELF_PARAMETER);
        let echoed = |c: &Shared| c.osc_values.lock().unwrap().latest.get(&address).is_some_and(|(_, at)| *at >= began_ns);
        let mut proved = false;
        while began.elapsed() < PROOF_WAIT {
            proved = echoed(self);
            if proved && began.elapsed() >= SETTLE {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if proved {
            return Some(true);
        }
        if listed {
            return Some(false);
        }
        self.end_beat();
        None
    }

    /// The avatar stays in the picture: why, and what to do, where the person looks.
    fn say_cannot_hide(self: &Shared) {
        let manifest = self.worn_manifest();
        let text = cannot_hide_text(manifest.as_ref().and_then(|m| m.name.as_deref()), manifest.as_ref().and_then(|m| m.generator.as_deref()));
        self.update(|s| s.capture.left_out.wearer = Left::InPicture);
        self.tell_about_picture(NoticeKind::Info, &text, "Your avatar comes out afterwards");
    }

    /// The manifest of the avatar worn, when its upload wrote one here.
    fn worn_manifest(&self) -> Option<AvatarManifest> {
        let heard = match self.osc_values.lock().unwrap().latest.first(AVATAR_CHANGE) {
            Some(OscValue::Text(id)) => Some(id),
            _ => None,
        };
        let id = heard.or_else(|| self.snapshot.lock().unwrap().n4n0.avatar_id.clone())?;
        let file = orbiters_core::paths::avatars_dir().join(orbiters_contracts::avatar::file_name(&id));
        AvatarManifest::parse(&std::fs::read_to_string(file).ok()?).ok().filter(|m| m.avatar_id == id)
    }

    /// Something the person must read about this capture: the toast, the Capture page and, as the flight may be seen from a
    /// headset, the 3D picture page of every menu.
    fn tell_about_picture(self: &Shared, kind: NoticeKind, text: &str, line: &str) {
        self.update(|s| s.capture.left_out.why = Some(text.to_string()));
        if self.burst.lock().unwrap().recording {
            // Its own toast comes with it.
            self.n4n0.photo_note(text.to_string(), line);
        } else {
            self.notify(kind, text);
        }
    }

    /// Everyone else out of a picture of the world, when anyone else is here. The mask goes back at the end.
    fn leave_others_out(self: &Shared, vrchat: &[(String, OscValue)]) {
        if self.others_here() == 0 {
            return;
        }
        let heard = self.osc_values.lock().unwrap().latest.get(REMOTE_PLAYER).and_then(|(v, _)| v.first().map(OscValue::truthy));
        let mask = value_of(vrchat, REMOTE_PLAYER).map(OscValue::truthy).or(heard);
        let cost = self.hide.lock().unwrap().others_cost_strip;
        match mask {
            // Already off: the person's own choice, nothing to put back.
            Some(false) => self.update(|s| s.capture.left_out.others = Left::Out),
            Some(true) if cost => {
                self.update(|s| s.capture.left_out.others = Left::InPicture);
                self.tell_about_picture(NoticeKind::Warning, OTHERS_COST, OTHERS_SHOW);
            }
            Some(true) => {
                self.hide.lock().unwrap().others_off = true;
                self.update(|s| s.capture.left_out.others = Left::Out);
                let _ = self.with_osc(|o| o.send(REMOTE_PLAYER, vec![rosc::OscType::Bool(false)]));
                self.check_others_went();
            }
            None => {
                self.update(|s| s.capture.left_out.others = Left::InPicture);
                self.tell_about_picture(NoticeKind::Warning, OTHERS_UNKNOWN, OTHERS_SHOW);
            }
        }
    }

    /// Other players in the instance, as VRChat's log says.
    pub(super) fn others_here(&self) -> usize {
        self.snapshot.lock().unwrap().vrchat.world.others()
    }

    /// Reads Remote User back after a while: VRChat keeping it on leaves nothing to put back.
    fn check_others_went(self: &Shared) {
        let ctl = self.clone();
        let spawned = std::thread::Builder::new().name("n4n0-remote-user".into()).spawn(move || {
            std::thread::sleep(CONFIRM);
            if !ctl.hide.lock().unwrap().others_off || ctl.mask_says(REMOTE_PLAYER) != Some(true) {
                return;
            }
            ctl.hide.lock().unwrap().others_off = false;
            ctl.update(|s| s.capture.left_out.others = Left::InPicture);
            ctl.tell_about_picture(NoticeKind::Warning, OTHERS_KEPT, OTHERS_SHOW);
        });
        if let Err(e) = spawned {
            eprintln!("reading Remote User back: {e}");
        }
    }

    /// The strip went away with Remote User off: it goes back on, for this picture and the next ones of this run, and `true`.
    /// A picture is worth more than leaving the others out, and the kit surviving this mask is the one thing no recording shows.
    pub(super) fn others_cost_the_strip(self: &Shared) -> bool {
        {
            let mut hide = self.hide.lock().unwrap();
            if !std::mem::take(&mut hide.others_off) {
                return false;
            }
            hide.others_cost_strip = true;
        }
        let _ = self.with_osc(|o| o.send(REMOTE_PLAYER, vec![rosc::OscType::Bool(true)]));
        self.update(|s| s.capture.left_out.others = Left::InPicture);
        self.tell_about_picture(NoticeKind::Warning, OTHERS_COST, OTHERS_SHOW);
        true
    }

    /// Starts holding the avatar hidden, unless it already is.
    fn begin_beat(self: &Shared) {
        let generation = {
            let mut hide = self.hide.lock().unwrap();
            if hide.hiding {
                return;
            }
            hide.hiding = true;
            hide.generation += 1;
            hide.generation
        };
        let ctl = self.clone();
        if let Err(e) = std::thread::Builder::new().name("n4n0-hide-beat".into()).spawn(move || ctl.hide_beat(generation)) {
            eprintln!("hiding the avatar: {e}");
            self.hide.lock().unwrap().hiding = false;
        }
    }

    /// Flips the parameter while this recording holds the avatar hidden, off first: the animator comes in on a change.
    fn hide_beat(self: &Shared, generation: u64) {
        let address = parameter(HIDE_SELF_PARAMETER);
        let mut value = false;
        while self.hide.lock().unwrap().generation == generation {
            let _ = self.with_osc(|o| o.send(&address, vec![rosc::OscType::Bool(value)]));
            value = !value;
            std::thread::sleep(BEAT);
        }
    }

    /// Local User on, `after` a while, unless the recording ended by then; then checks that VRChat took it.
    fn turn_local_user_on(self: &Shared, after: Duration) {
        self.hide.lock().unwrap().turned_on = true;
        let generation = self.hide.lock().unwrap().generation;
        let ctl = self.clone();
        let spawned = std::thread::Builder::new().name("n4n0-local-user".into()).spawn(move || {
            std::thread::sleep(after);
            let still = |c: &Shared| {
                let hide = c.hide.lock().unwrap();
                hide.turned_on && hide.generation == generation
            };
            if !still(&ctl) {
                return;
            }
            let _ = ctl.with_osc(|o| o.send(LOCAL_PLAYER, vec![rosc::OscType::Bool(true)]));
            std::thread::sleep(CONFIRM);
            if still(&ctl) && ctl.mask_says(LOCAL_PLAYER) == Some(false) {
                ctl.mask_not_taken();
            }
        });
        if let Err(e) = spawned {
            eprintln!("turning Local User on: {e}");
        }
    }

    /// A mask, as VRChat says it when asked anew: its tree first, else what it last sent.
    fn mask_says(&self, path: &str) -> Option<bool> {
        let query = self.with_osc(|o| o.vrchat_query()).flatten();
        let asked = query.map(|addr| orbiters_core::osc::vrchat_camera_values(addr, Duration::from_millis(500))).unwrap_or_default();
        value_of(&asked, path)
            .map(OscValue::truthy)
            .or_else(|| self.osc_values.lock().unwrap().latest.get(path).and_then(|(v, _)| v.first().map(OscValue::truthy)))
    }

    /// VRChat kept the mask off: no strip comes, so there is nothing to hide the avatar for.
    fn mask_not_taken(self: &Shared) {
        {
            let mut hide = self.hide.lock().unwrap();
            hide.turned_on = false;
            hide.mask = Some(false);
        }
        self.end_beat();
        self.update(|s| s.capture.left_out.wearer = Left::NotAsked);
        self.notify(NoticeKind::Error, NOT_TAKEN);
    }

    /// Stops holding the avatar hidden; the last value, off, keeps it from waiting for the clock.
    fn end_beat(&self) {
        let was = {
            let mut hide = self.hide.lock().unwrap();
            hide.generation += 1;
            std::mem::take(&mut hide.hiding)
        };
        if was {
            let _ = self.with_osc(|o| o.send(&parameter(HIDE_SELF_PARAMETER), vec![rosc::OscType::Bool(false)]));
        }
    }

    /// The recording is over: the masks as the person left them, then the avatar back.
    pub(super) fn restore_hide_self(&self) {
        let (turned_on, others_off) = {
            let mut hide = self.hide.lock().unwrap();
            (std::mem::take(&mut hide.turned_on), std::mem::take(&mut hide.others_off))
        };
        if others_off {
            let _ = self.with_osc(|o| o.send(REMOTE_PLAYER, vec![rosc::OscType::Bool(true)]));
        }
        if turned_on {
            let _ = self.with_osc(|o| o.send(LOCAL_PLAYER, vec![rosc::OscType::Bool(false)]));
        }
        self.end_beat();
    }

    /// VRChat said Local User is off while recording (the person changed it, or it answered late), on the OSC action
    /// thread. In a recording that holds the avatar hidden it is put back on, as the kit needs it; in any other, this is
    /// the start's choice made now.
    pub(super) fn local_user_went_off(self: &Shared) {
        if !self.is_capturing() {
            return;
        }
        let (turned_on, may_drive, always, due) = {
            let mut hide = self.hide.lock().unwrap();
            let due = hide.last_reassert.is_none_or(|t| t.elapsed() >= REASSERT);
            if due {
                hide.last_reassert = Some(Instant::now());
            }
            (hide.turned_on, hide.may_drive, hide.always, due)
        };
        if !may_drive || !due {
            return;
        }
        if turned_on {
            let _ = self.with_osc(|o| o.send(LOCAL_PLAYER, vec![rosc::OscType::Bool(true)]));
            return;
        }
        let vrchat = self.vrchat_values();
        let (kit, listed) = avatar_support(&vrchat);
        self.hide.lock().unwrap().kit = kit;
        if self.hiding_self() {
            // Hidden already (a picture of the world): only the mask, without which the kit draws nothing.
            if kit != Some(false) {
                self.turn_local_user_on(Duration::ZERO);
            }
            return;
        }
        self.apply_plan(plan(may_drive, Some(false), kit, always), listed);
    }

    /// Another avatar was worn while recording and settled: its upload may hide itself or not.
    pub(super) fn hide_self_avatar_changed(self: &Shared) {
        let vrchat = self.vrchat_values();
        let (kit, listed) = avatar_support(&vrchat);
        let (hiding, wanted) = {
            let mut hide = self.hide.lock().unwrap();
            hide.kit = kit;
            hide.told = false;
            (hide.hiding, hide.turned_on || hide.always)
        };
        if !wanted {
            return;
        }
        if listed && !hiding {
            self.begin_beat();
        } else if !listed && hiding {
            self.end_beat();
            self.say_cannot_hide();
        }
    }

    /// Stats came: frames without a strip for a while are told once a stretch, with the reason, and shown on the page.
    pub(super) fn check_strip(self: &Shared, missing_s: f32) {
        if missing_s >= OTHERS_COST_S && self.others_cost_the_strip() {
            return;
        }
        if missing_s < NO_STRIP_AFTER_S {
            let was = std::mem::take(&mut self.hide.lock().unwrap().told);
            if was {
                self.update(|s| s.capture.no_strip = None);
            }
            return;
        }
        if std::mem::replace(&mut self.hide.lock().unwrap().told, true) {
            return;
        }
        let why = self.why_no_strip_now();
        self.update(|s| s.capture.no_strip = Some(why));
        self.notify(NoticeKind::Warning, why.message());
    }

    pub(super) fn why_no_strip_now(&self) -> NoStrip {
        let mask = self.local_user();
        let kit = self.hide.lock().unwrap().kit;
        why_no_strip(mask, kit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::Controller;
    use crate::state::CaptureState;
    use orbiters_core::settings::Settings;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn the_wearer_hides_for_the_mask_or_a_picture_of_the_world() {
        let hide = |turn_mask_on| Plan::Hide { turn_mask_on };
        assert_eq!(plan(true, Some(false), Some(true), false), hide(true));
        // On: the avatar stays in the picture, as it was asked to, but for a picture of the world.
        assert_eq!(plan(true, Some(true), Some(true), false), Plan::Leave);
        assert_eq!(plan(true, Some(true), Some(true), true), hide(false));
        assert_eq!(plan(true, Some(false), Some(true), true), hide(true));
        // VRChat does not say: the mask is not changed on a guess. A picture of the world still tries: the echo tells.
        assert_eq!(plan(true, None, Some(true), false), Plan::Leave);
        assert_eq!(plan(true, Some(false), None, false), Plan::Leave);
        assert_eq!(plan(true, None, None, true), hide(false));
        assert_eq!(plan(true, Some(false), None, true), hide(false));
        // No kit: nothing would draw whatever is hidden, and the strip's own message says so.
        assert_eq!(plan(true, Some(false), Some(false), false), Plan::Leave);
        assert_eq!(plan(true, Some(true), Some(false), true), Plan::Leave);
        // The camera is not the app's to change (Settings).
        assert_eq!(plan(false, Some(false), Some(true), true), Plan::Leave);
    }

    #[test]
    fn the_reason_names_what_vrchat_says() {
        assert_eq!(why_no_strip(Some(false), Some(true)), NoStrip::MaskOff, "the sessions of 22:59 and 23:00");
        assert_eq!(why_no_strip(Some(false), None), NoStrip::MaskOff);
        assert_eq!(why_no_strip(Some(true), Some(false)), NoStrip::NoKit, "the sessions of 22:56 and 02:16, the N4N0 avatar");
        assert_eq!(why_no_strip(Some(false), Some(false)), NoStrip::NoKit, "turning the mask on would not help");
        assert_eq!(why_no_strip(Some(true), Some(true)), NoStrip::NotDrawn);
        assert_eq!(why_no_strip(None, None), NoStrip::Unknown);
        for reason in [NoStrip::MaskOff, NoStrip::NoKit, NoStrip::NotDrawn, NoStrip::Unknown] {
            assert!(reason.message().contains("keyframes"), "{reason:?}");
            assert!(!reason.short().is_empty());
        }
        assert!(NoStrip::MaskOff.message().contains("Local User"));
        assert!(NoStrip::NoKit.message().contains("My Avatar"));
    }

    #[test]
    fn what_the_avatar_supports_comes_from_vrchats_tree() {
        let record = (parameter(RECORD_PARAMETER), OscValue::Bool(false));
        let hide = (parameter(HIDE_SELF_PARAMETER), OscValue::Bool(false));
        let other = (parameter("Viseme"), OscValue::Int(0));
        assert_eq!(avatar_support(&[record.clone(), hide, other.clone()]), (Some(true), true));
        assert_eq!(avatar_support(&[record, other.clone()]), (Some(true), false), "an older upload");
        assert_eq!(avatar_support(&[other]), (Some(false), false), "an avatar without the kit");
        assert_eq!(avatar_support(&[("/usercamera/Mode".into(), OscValue::Int(2))]), (None, false), "no avatar in the tree: VRChat does not say");
    }

    /// Enzo's avatar as its manifest said on 2026-11-04, and what a later upload or a stale list would say.
    #[test]
    fn the_message_says_why_and_what_to_do() {
        let his = cannot_hide_text(Some("The 1MB Ultirex"), Some("Orbiters Toolkit 0.4.11"));
        assert!(
            his.contains("comes out of the 3D view")
                && his.contains("The 1MB Ultirex")
                && his.contains("Toolkit 0.4.11")
                && his.contains("0.4.18 or later")
                && his.contains("cleaner result")
                && !his.contains("IS in this capture"),
            "{his}"
        );
        let stale = cannot_hide_text(Some("The 1MB Ultirex"), Some("Orbiters Toolkit 0.4.18"));
        assert!(stale.contains("comes out of the 3D view") && stale.contains("Reset Config") && !stale.contains("An upload"), "{stale}");
        let later = cannot_hide_text(None, Some("Orbiters Toolkit 0.5.0-dev.3"));
        assert!(later.contains("Reset Config") && later.contains("this avatar"), "{later}");
        let unknown = cannot_hide_text(None, None);
        assert!(unknown.contains("comes out of the 3D view") && unknown.contains("0.4.18 or later"), "{unknown}");
        assert!(toolkit_version("Unity").is_none());
    }

    /// VRChat over UDP, as far as this goes: it hears everything, and echoes a flip of the hiding parameter only for an
    /// avatar that has it, as the real one did for every parameter the sessions of 2026-11-04 show.
    struct Stand {
        ctl: Shared,
        sent: Arc<Mutex<Vec<(Instant, String, rosc::OscType)>>>,
        done: Arc<AtomicBool>,
    }

    fn stand_in(has_layer: bool) -> Stand {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        let (sent, done) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (out, end, weak) = (sent.clone(), done.clone(), Arc::downgrade(&ctl));
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while !end.load(Ordering::Relaxed) {
                let Ok(n) = vrchat.recv(&mut buf) else { continue };
                let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) else { continue };
                if has_layer && m.addr == parameter(HIDE_SELF_PARAMETER) {
                    if let Some(ctl) = weak.upgrade() {
                        let value = matches!(m.args[0], rosc::OscType::Bool(true));
                        ctl.osc_values.lock().unwrap().latest.record(m.addr.clone(), vec![OscValue::Bool(value)], clock::now_ns());
                    }
                }
                out.lock().unwrap().push((Instant::now(), m.addr, m.args[0].clone()));
            }
        });
        Stand { ctl, sent, done }
    }

    impl Stand {
        /// What was sent since the last call, after waiting `wait`.
        fn heard(&self, wait: Duration) -> Vec<(Instant, String, rosc::OscType)> {
            std::thread::sleep(wait);
            std::mem::take(&mut *self.sent.lock().unwrap())
        }

        fn end(self) {
            self.ctl.update(|s| s.capture.state = CaptureState::Idle);
            self.ctl.shutdown();
            self.done.store(true, Ordering::Relaxed);
        }
    }

    fn tree(mask: bool, hide: bool) -> Vec<(String, OscValue)> {
        let mut v = vec![(LOCAL_PLAYER.to_string(), OscValue::Bool(mask)), (parameter(RECORD_PARAMETER), OscValue::Bool(false))];
        if hide {
            v.push((parameter(HIDE_SELF_PARAMETER), OscValue::Bool(false)));
        }
        v
    }

    fn says(ctl: &Shared, mask: bool) {
        ctl.osc_values.lock().unwrap().latest.record(LOCAL_PLAYER.to_string(), vec![OscValue::Bool(mask)], 1);
    }

    fn toast_with(ctl: &Shared, kind: NoticeKind, text: &str) -> bool {
        ctl.take_notices().iter().any(|n| matches!(n, crate::controller::Notice::Toast { kind: k, message } if *k == kind && message.contains(text)))
    }

    #[test]
    fn with_the_mask_off_the_avatar_goes_first_and_comes_back_last() {
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        let t0 = Instant::now();
        ctl.start_hide_self(&tree(false, true), true, false);
        let first = stand.heard(Duration::from_millis(150));
        let hide = parameter(HIDE_SELF_PARAMETER);
        // The beat first, off then on, then the mask, then the beat goes on.
        assert_eq!((first[0].1.as_str(), &first[0].2), (hide.as_str(), &rosc::OscType::Bool(false)), "{first:?}");
        assert_eq!((first[1].1.as_str(), &first[1].2), (hide.as_str(), &rosc::OscType::Bool(true)), "{first:?}");
        let mask = first.iter().position(|(_, a, _)| a == LOCAL_PLAYER).expect("the mask");
        assert!(mask >= 2, "a flip reached the animator before the mask: {first:?}");
        assert_eq!(first[mask].2, rosc::OscType::Bool(true));
        assert!(first[mask].0 - t0 >= SETTLE);
        assert_eq!(ctl.view().capture.left_out.wearer, Left::Out);
        // VRChat echoes it, and nothing is taken back.
        says(ctl, true);
        let beats = stand.heard(Duration::from_millis(1600));
        let values: Vec<_> = beats.iter().filter(|(_, a, _)| a == &hide).map(|(_, _, v)| v.clone()).collect();
        assert!(values.len() >= 5, "{beats:?}");
        assert!(values.windows(2).all(|w| w[0] != w[1]), "it flips: the animator's clock only restarts on a change: {values:?}");
        assert!(!beats.iter().any(|(_, a, _)| a == LOCAL_PLAYER), "{beats:?}");
        // The end: the mask as it was, then the avatar back with no more beats.
        ctl.restore_hide_self();
        let end = stand.heard(Duration::from_millis(700));
        let at = end.iter().position(|(_, a, _)| a == LOCAL_PLAYER).expect("the mask back");
        assert_eq!(end[at].2, rosc::OscType::Bool(false));
        assert_eq!(end.last().map(|(_, a, v)| (a.clone(), v.clone())), Some((hide.clone(), rosc::OscType::Bool(false))));
        assert!(end.iter().filter(|(_, a, _)| a == &hide).count() <= 2, "the beat ended: {end:?}");
        stand.end();
    }

    /// Enzo's seven recordings of 2026-11-04: Local User off, an upload without the layer. The mask goes on for the kit, the
    /// flips stop, and the person reads it.
    #[test]
    fn an_older_upload_gets_the_mask_on_no_more_beats_and_a_plain_warning() {
        let stand = stand_in(false);
        let ctl = &stand.ctl;
        ctl.start_hide_self(&tree(false, false), true, false);
        let sent = stand.heard(Duration::from_millis(600));
        let hide = parameter(HIDE_SELF_PARAMETER);
        assert_eq!(
            sent.iter().filter(|(_, a, _)| a == LOCAL_PLAYER).map(|(_, _, v)| v.clone()).collect::<Vec<_>>(),
            vec![rosc::OscType::Bool(true)],
            "{sent:?}"
        );
        assert_eq!(sent.last().map(|(_, a, v)| (a.clone(), v.clone())), Some((LOCAL_PLAYER.to_string(), rosc::OscType::Bool(true))), "{sent:?}");
        assert!(sent.iter().filter(|(_, a, _)| a == &hide).count() >= 3, "it asked: {sent:?}");
        says(ctl, true);
        assert_eq!(ctl.view().capture.left_out.wearer, Left::InPicture);
        assert!(ctl.view().capture.left_out.why.as_deref().is_some_and(|w| w.contains("comes out of the 3D view")));
        assert!(toast_with(ctl, NoticeKind::Info, "comes out of the 3D view"));
        ctl.restore_hide_self();
        let sent = stand.heard(Duration::from_millis(200));
        assert_eq!(sent.iter().map(|(_, a, v)| (a.clone(), v.clone())).collect::<Vec<_>>(), vec![(LOCAL_PLAYER.to_string(), rosc::OscType::Bool(false))]);
        stand.end();
    }

    #[test]
    fn with_the_mask_on_nothing_is_touched() {
        let stand = stand_in(true);
        stand.ctl.start_hide_self(&tree(true, true), true, false);
        assert!(stand.heard(Duration::from_millis(700)).is_empty());
        stand.ctl.restore_hide_self();
        assert!(stand.heard(Duration::from_millis(100)).is_empty());
        stand.end();
    }

    /// A picture of the world: the mask already on, as it usually is. Nothing is recorded before the start returns, so the avatar
    /// must be gone by then.
    #[test]
    fn a_picture_of_the_world_hides_the_wearer_with_the_mask_on_and_leaves_the_mask_alone() {
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        let t0 = Instant::now();
        ctl.start_hide_self(&tree(true, true), true, true);
        assert!(t0.elapsed() >= SETTLE, "it waits for the avatar to go");
        assert!(ctl.hiding_self());
        assert_eq!(ctl.view().capture.left_out.wearer, Left::Out);
        assert!(ctl.view().capture.left_out.why.is_none());
        let sent = stand.heard(Duration::from_millis(600));
        assert!(!sent.iter().any(|(_, a, _)| a == LOCAL_PLAYER), "{sent:?}");
        ctl.restore_hide_self();
        assert!(!ctl.hiding_self());
        let end = stand.heard(Duration::from_millis(100));
        assert!(!end.iter().any(|(_, a, _)| a == LOCAL_PLAYER), "{end:?}");
        stand.end();
    }

    /// The same picture with the upload Enzo wore: no silent picture with the avatar in it.
    #[test]
    fn a_picture_of_the_world_says_it_when_the_avatar_cannot_hide() {
        let stand = stand_in(false);
        let ctl = &stand.ctl;
        ctl.start_hide_self(&tree(true, false), true, true);
        assert!(!ctl.hiding_self());
        assert_eq!(ctl.view().capture.left_out.wearer, Left::InPicture);
        assert!(toast_with(ctl, NoticeKind::Info, "comes out of the 3D view"));
        let flips = stand.heard(Duration::from_millis(400));
        assert!(flips.iter().all(|(_, a, _)| a != LOCAL_PLAYER), "{flips:?}");
        assert!(stand.heard(Duration::from_millis(400)).is_empty(), "the flips ended");
        stand.end();
    }

    /// An avatar VRChat lists the parameter of, which does not echo: hidden as far as anyone can tell, and the person is told to look.
    #[test]
    fn no_echo_from_a_listed_parameter_is_said_too() {
        let stand = stand_in(false);
        let ctl = &stand.ctl;
        ctl.start_hide_self(&tree(true, true), true, true);
        assert!(ctl.hiding_self());
        assert!(toast_with(ctl, NoticeKind::Warning, "did not confirm"));
        ctl.restore_hide_self();
        stand.end();
    }

    fn with_others(ctl: &Shared, n: usize) {
        ctl.update(|s| {
            for i in 0..n {
                s.vrchat.world.players.insert(format!("usr_{i}"), format!("Player {i}"));
            }
        });
    }

    fn remote(mask: bool) -> (String, OscValue) {
        (REMOTE_PLAYER.to_string(), OscValue::Bool(mask))
    }

    #[test]
    fn the_other_players_leave_a_picture_of_the_world_and_their_mask_goes_back_as_it_was() {
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        with_others(ctl, 2);
        let mut vrchat = tree(true, true);
        vrchat.push(remote(true));
        ctl.start_hide_self(&vrchat, true, true);
        let sent = stand.heard(Duration::from_millis(100));
        assert_eq!(sent.iter().filter(|(_, a, _)| a == REMOTE_PLAYER).map(|(_, _, v)| v.clone()).collect::<Vec<_>>(), vec![rosc::OscType::Bool(false)]);
        assert!(ctl.hiding_others());
        assert_eq!(ctl.view().capture.left_out.others, Left::Out);
        ctl.restore_hide_self();
        let end = stand.heard(Duration::from_millis(100));
        assert_eq!(end.iter().filter(|(_, a, _)| a == REMOTE_PLAYER).map(|(_, _, v)| v.clone()).collect::<Vec<_>>(), vec![rosc::OscType::Bool(true)]);
        stand.end();

        // Already off is the person's own: it is neither set nor put back.
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        with_others(ctl, 2);
        let mut vrchat = tree(true, true);
        vrchat.push(remote(false));
        ctl.start_hide_self(&vrchat, true, true);
        ctl.restore_hide_self();
        assert!(!stand.heard(Duration::from_millis(100)).iter().any(|(_, a, _)| a == REMOTE_PLAYER));
        assert_eq!(ctl.view().capture.left_out.others, Left::Out);
        stand.end();

        // Alone: nothing is touched. In a picture of people, neither.
        for (others, world) in [(0, true), (2, false)] {
            let stand = stand_in(true);
            with_others(&stand.ctl, others);
            let mut vrchat = tree(true, true);
            vrchat.push(remote(true));
            stand.ctl.start_hide_self(&vrchat, true, world);
            stand.ctl.restore_hide_self();
            assert!(!stand.heard(Duration::from_millis(100)).iter().any(|(_, a, _)| a == REMOTE_PLAYER));
            assert!(!stand.ctl.hiding_others());
            stand.end();
        }
    }

    /// No recording ever had Remote User off with Local User on: if the kit goes with it, the mask comes back and it is said.
    #[test]
    fn a_strip_lost_to_the_other_players_mask_brings_them_back_and_says_so() {
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        with_others(ctl, 1);
        let mut vrchat = tree(true, true);
        vrchat.push(remote(true));
        ctl.start_hide_self(&vrchat, true, true);
        ctl.take_notices();
        stand.heard(Duration::ZERO);
        ctl.check_strip(0.5);
        assert!(ctl.hiding_others(), "a moment without a strip is nothing");
        ctl.check_strip(1.1);
        assert!(!ctl.hiding_others());
        assert_eq!(ctl.view().capture.left_out.others, Left::InPicture);
        assert!(toast_with(ctl, NoticeKind::Warning, "Remote User"));
        let sent = stand.heard(Duration::from_millis(100));
        assert_eq!(sent.iter().filter(|(_, a, _)| a == REMOTE_PLAYER).map(|(_, _, v)| v.clone()).collect::<Vec<_>>(), vec![rosc::OscType::Bool(true)]);
        // At the end it is not sent again, and the next picture of this run does not try.
        ctl.restore_hide_self();
        assert!(!stand.heard(Duration::from_millis(100)).iter().any(|(_, a, _)| a == REMOTE_PLAYER));
        ctl.start_hide_self(&vrchat, true, true);
        assert!(!stand.heard(Duration::from_millis(100)).iter().any(|(_, a, _)| a == REMOTE_PLAYER));
        assert_eq!(ctl.view().capture.left_out.others, Left::InPicture);
        ctl.restore_hide_self();
        stand.end();
    }

    #[test]
    fn vrchat_keeping_the_mask_off_lets_the_avatar_go_and_says_so() {
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        says(ctl, false);
        ctl.start_hide_self(&tree(false, true), true, false);
        // VRChat never echoes anything but off.
        let sent = stand.heard(CONFIRM + Duration::from_millis(900));
        let hide = parameter(HIDE_SELF_PARAMETER);
        let last = sent.iter().filter(|(_, a, _)| a == &hide).last().expect("beats");
        assert_eq!(last.2, rosc::OscType::Bool(false), "the last word lets it go");
        assert!(stand.heard(Duration::from_millis(500)).is_empty(), "the beat ended");
        assert!(toast_with(ctl, NoticeKind::Error, "kept Local User off"));
        // And it does not try to turn the mask off at the end: it never took it on.
        ctl.restore_hide_self();
        assert!(stand.heard(Duration::from_millis(100)).is_empty());
        stand.end();
    }

    #[test]
    fn turning_the_mask_off_during_a_hidden_recording_puts_it_back_on() {
        let stand = stand_in(true);
        let ctl = &stand.ctl;
        ctl.start_hide_self(&tree(false, true), true, false);
        says(ctl, true);
        stand.heard(Duration::from_millis(100));
        says(ctl, false);
        ctl.local_user_went_off();
        let sent = stand.heard(Duration::from_millis(200));
        assert!(sent.iter().any(|(_, a, v)| a == LOCAL_PLAYER && *v == rosc::OscType::Bool(true)), "{sent:?}");
        says(ctl, true);
        ctl.restore_hide_self();
        stand.end();
    }

    fn stopped(frames: u64, keyframes: u64, reason: &str) -> orbiters_core::protocol::WorkerEvent {
        let mut manifest = Box::<orbiters_contracts::manifest::SessionManifest>::default();
        manifest.stats.frames = frames;
        manifest.stats.keyframes = keyframes;
        orbiters_core::protocol::WorkerEvent::Stopped { session_dir: std::env::temp_dir().join("orbiters-no-such-session"), manifest, reason: reason.into() }
    }

    /// What ended silently before: 419 frames, none with a pose, and a green "Session saved".
    #[test]
    fn a_session_without_a_keyframe_says_so_with_the_reason_and_is_not_reconstructed() {
        let ctl = Controller::new(Settings::default());
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        ctl.hide.lock().unwrap().kit = Some(true);
        says(&ctl, false);
        ctl.burst.lock().unwrap().recording = true;
        ctl.on_worker_event(stopped(419, 0, "stopped"));
        let notices = ctl.take_notices();
        assert!(
            notices.iter().any(|n| matches!(n, crate::controller::Notice::Toast { kind: NoticeKind::Error, message } if message.contains("419 frames") && message.contains("Local User"))),
            "{notices:?}"
        );
        assert!(
            !notices.iter().any(|n| matches!(n, crate::controller::Notice::SessionSaved { reason, .. } if reason != "restarted")),
            "no success toast: {notices:?}"
        );
        assert_eq!(ctl.view().capture.no_strip, Some(NoStrip::MaskOff));
        assert!(ctl.view().reconstruct_queue.is_empty(), "nothing to reconstruct");

        // A good session keeps its toast, and tells nothing of the kind.
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        ctl.on_worker_event(stopped(748, 521, "stopped"));
        let notices = ctl.take_notices();
        assert!(notices.iter().any(|n| matches!(n, crate::controller::Notice::SessionSaved { keyframes: 521, .. })), "{notices:?}");
        assert!(!notices.iter().any(|n| matches!(n, crate::controller::Notice::Toast { kind: NoticeKind::Error, .. })), "{notices:?}");

        // A part the recording goes on after is no end of anything.
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        ctl.on_worker_event(stopped(30, 0, "restarted"));
        assert!(!ctl.take_notices().iter().any(|n| matches!(n, crate::controller::Notice::Toast { kind: NoticeKind::Error, .. })));
        ctl.update(|s| s.capture.state = CaptureState::Idle);
    }

    #[test]
    fn frames_without_a_strip_are_told_once_a_stretch_with_the_reason() {
        let ctl = Controller::new(Settings::default());
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        ctl.hide.lock().unwrap().kit = Some(true);
        says(&ctl, false);
        ctl.check_strip(0.0);
        ctl.check_strip(1.9);
        assert_eq!(ctl.view().capture.no_strip, None);
        ctl.check_strip(2.0);
        assert_eq!(ctl.view().capture.no_strip, Some(NoStrip::MaskOff));
        ctl.check_strip(3.0);
        let notices = ctl.take_notices();
        assert_eq!(
            notices.iter().filter(|n| matches!(n, crate::controller::Notice::Toast { message, .. } if message.contains("Local User"))).count(),
            1,
            "{notices:?}"
        );
        // The strip came back: the page clears, and a new stretch is told again.
        ctl.check_strip(0.0);
        assert_eq!(ctl.view().capture.no_strip, None);
        ctl.check_strip(2.5);
        assert_eq!(ctl.take_notices().len(), 1);
        ctl.update(|s| s.capture.state = CaptureState::Idle);
    }
}
