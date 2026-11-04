//! Recording without the wearer (contracts' `Docs/rig-frames.md`), and saying why a recording has no strip.
//!
//! N4N0's capture kit is part of the avatar, so VRChat's camera mask "Local User" culls it with the avatar: in Enzo's
//! sessions of 2026-11-03 the same avatar gave a strip on 678 of 748 frames with the mask on and on 0 of 509 with it off.
//! No strip is no pose, no depth, no keyframe. The mask is also how a person says "leave me out", so the app keeps that
//! promise another way: it leaves Local User on while it records, and has the avatar's own animator turn off every
//! renderer but the kit's and the rig's, on the wearer's client only. Nothing of the wearer is then in the colour, the
//! depth or the rig's faces, so nothing has to be erased afterwards.
//!
//! The order matters. With the mask off the kit draws nothing, so the first frame with a strip comes only after
//! [`LOCAL_PLAYER`] is set: the avatar goes first and the mask follows [`SETTLE`] later, and no frame ever has both a pose
//! and the wearer. At the end the mask goes back first, then the avatar.
//!
//! **The avatar always comes back.** A stuck invisible avatar is the worst this can do, so the app holds it hidden rather
//! than switching it: the [`beat`](super::Controller::hide_beat) flips `N4N0/Capture/HideSelf` every [`BEAT`], and the
//! animator lets go by itself about 1.5 s after the last flip (Toolkit's `N4N0Installer.HideSelf`). A crash, a closed
//! OSC port or a frozen app all end the same way, with the avatar shown, and loading an avatar anew starts it shown.
//!
//! What VRChat really does with the message: `/usercamera/LocalPlayer` is documented as "Get/Toggle", and the sessions
//! show a bool message on these toggles sets rather than flips (`SmoothMovement` sent false at 0 stayed 0; `Streaming`
//! sent true stayed 1), but no session has set this one. So the app reads it back after [`CONFIRM`] and, if VRChat kept
//! the mask off, lets the avatar go and says so.

use super::Shared;
use crate::controller::NoticeKind;
use crate::state::NoStrip;
use orbiters_core::osc::{OscValue, HIDE_SELF_PARAMETER, PARAMETERS_PREFIX, RECORD_PARAMETER};
use std::time::{Duration, Instant};

/// VRChat's camera mask "Local User" (read and set; VRChat 2025.3.3 and later).
pub const LOCAL_PLAYER: &str = "/usercamera/LocalPlayer";
/// How often the avatar is told again to stay hidden: six of these fit in the animator's 1.5 s, so a datagram lost or a
/// busy moment shows nothing.
const BEAT: Duration = Duration::from_millis(250);
/// From the first beat to the mask: past one flip (a beat) and a few animator frames at any frame rate.
const SETTLE: Duration = Duration::from_millis(350);
/// How long VRChat gets to show it took the mask.
const CONFIRM: Duration = Duration::from_millis(1200);
/// A person turning the mask off again is answered this often at most.
const REASSERT: Duration = Duration::from_secs(1);
/// Frames without a strip for this long are told, with the reason.
pub const NO_STRIP_AFTER_S: f32 = 2.0;

const WITHOUT_YOU: &str = "Recording without your avatar, as Local User is off in VRChat's camera: it is hidden on your side while this records, and comes back after. Others still see you.";
const WITH_YOU: &str = "Your avatar IS in this recording. The capture kit cannot draw with Local User off, so it was turned on for now, and this upload of your avatar cannot hide itself yet: update Orbiters Toolkit and upload the avatar again to record without it.";
const NOT_TAKEN: &str = "VRChat kept Local User off, so your avatar is shown again and this recording has no keyframes. Turn Local User on in the camera's Mask menu.";
const UPLOAD_CANNOT_HIDE: &str = "The avatar you switched to cannot hide itself while recording (an older upload), so it is in the picture. Update Orbiters Toolkit and upload it again to record without it.";

/// What a recording does about the mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Mask on or unknown, or an avatar without the kit, or the camera is not the app's to change.
    Leave,
    /// Mask off: the avatar hidden, then Local User on.
    HideThenTurnOn,
    /// Mask off, an upload without the layer: Local User on, the avatar in the picture and the person told.
    TurnOnShown,
}

/// `mask`: Local User as VRChat says. `kit`: the worn avatar has the capture kit, `None` when VRChat does not say.
pub fn plan(may_drive: bool, mask: Option<bool>, kit: Option<bool>, can_hide: bool) -> Plan {
    match (may_drive, mask, kit, can_hide) {
        (true, Some(false), Some(true), true) => Plan::HideThenTurnOn,
        (true, Some(false), Some(true), false) => Plan::TurnOnShown,
        _ => Plan::Leave,
    }
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
    /// The camera is the app's to change in this recording.
    may_drive: bool,
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

    /// A recording starts: `vrchat` is VRChat's tree as read for it.
    pub(super) fn start_hide_self(self: &Shared, vrchat: &[(String, OscValue)], may_drive: bool) {
        let (kit, can_hide) = avatar_support(vrchat);
        let mask = value_of(vrchat, LOCAL_PLAYER).map(OscValue::truthy).or_else(|| self.local_user());
        {
            let mut hide = self.hide.lock().unwrap();
            hide.may_drive = may_drive;
            hide.kit = kit;
            hide.mask = mask;
            hide.told = false;
        }
        self.apply_plan(plan(may_drive, mask, kit, can_hide));
    }

    fn apply_plan(self: &Shared, plan: Plan) {
        match plan {
            Plan::Leave => {}
            Plan::TurnOnShown => {
                self.turn_local_user_on(Duration::ZERO);
                self.notify(NoticeKind::Warning, WITH_YOU);
            }
            Plan::HideThenTurnOn => {
                self.begin_beat();
                self.turn_local_user_on(SETTLE);
                self.notify(NoticeKind::Success, WITHOUT_YOU);
            }
        }
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
            if still(&ctl) && ctl.local_user_says_off() {
                ctl.mask_not_taken();
            }
        });
        if let Err(e) = spawned {
            eprintln!("turning Local User on: {e}");
        }
    }

    /// Whether VRChat, asked anew, says the mask is off: its tree first, else what it last sent.
    fn local_user_says_off(&self) -> bool {
        let query = self.with_osc(|o| o.vrchat_query()).flatten();
        let asked = query.map(|addr| orbiters_core::osc::vrchat_camera_values(addr, Duration::from_millis(500))).unwrap_or_default();
        let mask = value_of(&asked, LOCAL_PLAYER).map(OscValue::truthy).or_else(|| self.osc_values.lock().unwrap().latest.get(LOCAL_PLAYER).and_then(|(v, _)| v.first().map(OscValue::truthy)));
        mask == Some(false)
    }

    /// VRChat kept the mask off: no strip comes, so there is nothing to hide the avatar for.
    fn mask_not_taken(self: &Shared) {
        {
            let mut hide = self.hide.lock().unwrap();
            hide.turned_on = false;
            hide.mask = Some(false);
        }
        self.end_beat();
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

    /// The recording is over: the mask as the person left it, then the avatar back.
    pub(super) fn restore_hide_self(&self) {
        let turned_on = std::mem::take(&mut self.hide.lock().unwrap().turned_on);
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
        let (turned_on, may_drive, due) = {
            let mut hide = self.hide.lock().unwrap();
            let due = hide.last_reassert.is_none_or(|t| t.elapsed() >= REASSERT);
            if due {
                hide.last_reassert = Some(Instant::now());
            }
            (hide.turned_on, hide.may_drive, due)
        };
        if !may_drive || !due {
            return;
        }
        if turned_on {
            let _ = self.with_osc(|o| o.send(LOCAL_PLAYER, vec![rosc::OscType::Bool(true)]));
            return;
        }
        let vrchat = self.vrchat_values();
        let (kit, can_hide) = avatar_support(&vrchat);
        self.hide.lock().unwrap().kit = kit;
        self.apply_plan(plan(may_drive, Some(false), kit, can_hide));
    }

    /// Another avatar was worn while recording and settled: its upload may hide itself or not.
    pub(super) fn hide_self_avatar_changed(self: &Shared) {
        let vrchat = self.vrchat_values();
        let (kit, can_hide) = avatar_support(&vrchat);
        let (hiding, turned_on) = {
            let mut hide = self.hide.lock().unwrap();
            hide.kit = kit;
            hide.told = false;
            (hide.hiding, hide.turned_on)
        };
        if !turned_on {
            return;
        }
        if can_hide && !hiding {
            self.begin_beat();
        } else if !can_hide && hiding {
            self.end_beat();
            self.notify(NoticeKind::Warning, UPLOAD_CANNOT_HIDE);
        }
    }

    /// Stats came: frames without a strip for a while are told once a stretch, with the reason, and shown on the page.
    pub(super) fn check_strip(self: &Shared, missing_s: f32) {
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

    #[test]
    fn the_mask_off_means_hide_and_turn_on_only_with_an_upload_that_can() {
        assert_eq!(plan(true, Some(false), Some(true), true), Plan::HideThenTurnOn);
        assert_eq!(plan(true, Some(false), Some(true), false), Plan::TurnOnShown);
        // On: the avatar stays in the picture, as it was asked to.
        assert_eq!(plan(true, Some(true), Some(true), true), Plan::Leave);
        // VRChat does not say: nothing is changed on a guess.
        assert_eq!(plan(true, None, Some(true), true), Plan::Leave);
        assert_eq!(plan(true, Some(false), None, true), Plan::Leave);
        // No kit: turning the mask on would give nothing.
        assert_eq!(plan(true, Some(false), Some(false), false), Plan::Leave);
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

    fn vrchat_stand_in() -> (Shared, std::net::UdpSocket) {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        (ctl, vrchat)
    }

    fn heard(socket: &std::net::UdpSocket, wait: Duration) -> Vec<(Instant, String, rosc::OscType)> {
        let mut out = Vec::new();
        let until = Instant::now() + wait;
        let mut buf = [0u8; 1024];
        while Instant::now() < until {
            if let Ok(n) = socket.recv(&mut buf) {
                if let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                    out.push((Instant::now(), m.addr, m.args[0].clone()));
                }
            }
        }
        out
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

    #[test]
    fn with_the_mask_off_the_avatar_goes_first_and_comes_back_last() {
        let (ctl, vrchat) = vrchat_stand_in();
        let t0 = Instant::now();
        ctl.start_hide_self(&tree(false, true), true);
        let first = heard(&vrchat, Duration::from_millis(900));
        let hide = parameter(HIDE_SELF_PARAMETER);
        // The beat first, off then on, then the mask, then the beat goes on.
        assert_eq!((first[0].1.as_str(), &first[0].2), (hide.as_str(), &rosc::OscType::Bool(false)), "{first:?}");
        assert_eq!((first[1].1.as_str(), &first[1].2), (hide.as_str(), &rosc::OscType::Bool(true)), "{first:?}");
        let mask = first.iter().position(|(_, a, _)| a == LOCAL_PLAYER).expect("the mask");
        assert!(mask >= 2, "a flip reached the animator before the mask: {first:?}");
        assert_eq!(first[mask].2, rosc::OscType::Bool(true));
        assert!(first[mask].0 - t0 >= SETTLE);
        // VRChat echoes it, and nothing is taken back.
        says(&ctl, true);
        let beats = heard(&vrchat, Duration::from_millis(1600));
        let values: Vec<_> = beats.iter().filter(|(_, a, _)| a == &hide).map(|(_, _, v)| v.clone()).collect();
        assert!(values.len() >= 5, "{beats:?}");
        assert!(values.windows(2).all(|w| w[0] != w[1]), "it flips: the animator's clock only restarts on a change: {values:?}");
        assert!(!beats.iter().any(|(_, a, _)| a == LOCAL_PLAYER), "{beats:?}");
        // The end: the mask as it was, then the avatar back with no more beats.
        ctl.restore_hide_self();
        let end = heard(&vrchat, Duration::from_millis(700));
        let at = end.iter().position(|(_, a, _)| a == LOCAL_PLAYER).expect("the mask back");
        assert_eq!(end[at].2, rosc::OscType::Bool(false));
        assert_eq!(end.last().map(|(_, a, v)| (a.clone(), v.clone())), Some((hide.clone(), rosc::OscType::Bool(false))));
        assert!(end.iter().filter(|(_, a, _)| a == &hide).count() <= 2, "the beat ended: {end:?}");
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
    }

    #[test]
    fn an_older_upload_gets_the_mask_on_no_beat_and_a_plain_warning() {
        let (ctl, vrchat) = vrchat_stand_in();
        ctl.start_hide_self(&tree(false, false), true);
        let sent = heard(&vrchat, Duration::from_millis(400));
        assert_eq!(sent.iter().map(|(_, a, v)| (a.clone(), v.clone())).collect::<Vec<_>>(), vec![(LOCAL_PLAYER.to_string(), rosc::OscType::Bool(true))]);
        says(&ctl, true);
        assert!(ctl.take_notices().iter().any(|n| matches!(n, crate::controller::Notice::Toast { kind: NoticeKind::Warning, message } if message.contains("IS in this recording"))));
        ctl.restore_hide_self();
        let sent = heard(&vrchat, Duration::from_millis(200));
        assert_eq!(sent.iter().map(|(_, a, v)| (a.clone(), v.clone())).collect::<Vec<_>>(), vec![(LOCAL_PLAYER.to_string(), rosc::OscType::Bool(false))]);
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
    }

    #[test]
    fn with_the_mask_on_nothing_is_touched() {
        let (ctl, vrchat) = vrchat_stand_in();
        ctl.start_hide_self(&tree(true, true), true);
        assert!(heard(&vrchat, Duration::from_millis(700)).is_empty());
        ctl.restore_hide_self();
        assert!(heard(&vrchat, Duration::from_millis(100)).is_empty());
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
    }

    #[test]
    fn vrchat_keeping_the_mask_off_lets_the_avatar_go_and_says_so() {
        let (ctl, vrchat) = vrchat_stand_in();
        says(&ctl, false);
        ctl.start_hide_self(&tree(false, true), true);
        // VRChat never echoes anything but off.
        let sent = heard(&vrchat, SETTLE + CONFIRM + Duration::from_millis(900));
        let hide = parameter(HIDE_SELF_PARAMETER);
        let last = sent.iter().filter(|(_, a, _)| a == &hide).last().expect("beats");
        assert_eq!(last.2, rosc::OscType::Bool(false), "the last word lets it go");
        assert!(heard(&vrchat, Duration::from_millis(500)).is_empty(), "the beat ended");
        let notices = ctl.take_notices();
        assert!(notices.iter().any(|n| matches!(n, crate::controller::Notice::Toast { kind: NoticeKind::Error, message } if message.contains("kept Local User off"))));
        // And it does not try to turn the mask off at the end: it never took it on.
        ctl.restore_hide_self();
        assert!(heard(&vrchat, Duration::from_millis(100)).is_empty());
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
    }

    #[test]
    fn turning_the_mask_off_during_a_hidden_recording_puts_it_back_on() {
        let (ctl, vrchat) = vrchat_stand_in();
        ctl.start_hide_self(&tree(false, true), true);
        says(&ctl, true);
        heard(&vrchat, Duration::from_millis(600));
        says(&ctl, false);
        ctl.local_user_went_off();
        let sent = heard(&vrchat, Duration::from_millis(200));
        assert!(sent.iter().any(|(_, a, v)| a == LOCAL_PLAYER && *v == rosc::OscType::Bool(true)), "{sent:?}");
        says(&ctl, true);
        ctl.restore_hide_self();
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
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
        assert!(!notices.iter().any(|n| matches!(n, crate::controller::Notice::SessionSaved { reason, .. } if reason != "restarted")), "no success toast: {notices:?}");
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
        assert_eq!(notices.iter().filter(|n| matches!(n, crate::controller::Notice::Toast { message, .. } if message.contains("Local User"))).count(), 1, "{notices:?}");
        // The strip came back: the page clears, and a new stretch is told again.
        ctl.check_strip(0.0);
        assert_eq!(ctl.view().capture.no_strip, None);
        ctl.check_strip(2.5);
        assert_eq!(ctl.take_notices().len(), 1);
        ctl.update(|s| s.capture.state = CaptureState::Idle);
    }
}
