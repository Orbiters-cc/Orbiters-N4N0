//! Tracking smoothing (N4N0's Tracking page, `settings.tracking`): Orbiters - N4N0's SteamVR driver
//! (`orbiters-smooth-driver`, shipped beside the app as `driver_orbiters_smooth.dll`) smooths each device's pose as
//! SteamVR passes it on, damping SteamVR's prediction, by the strength the tray service writes for it in their shared
//! control block (`orbiters_smooth`): the headset, the controllers and the trackers each their own, by what SteamVR
//! says each device is.
//!
//! As with OVR-SmoothTracking, whose setup puts its driver in SteamVR's drivers folder once, the driver is registered
//! for SteamVR once, at the app's first start whatever the setting (and again at each start, which changes nothing
//! but a new build's DLL), so SteamVR loads it at every start of its own: it does nothing (patches nothing) until the
//! app turns smoothing on, and reads the control block every frame, so switching on or off and every strength change
//! is live. Only a first registration made while SteamVR already runs needs SteamVR's next start, once ever; the note
//! says so. The driver smooths only while the tray service runs: it owns the control block.
//!
//! Which device is which comes from the SteamVR overlay process, connected to SteamVR anyway ([`DeviceKindsWatch`]):
//! the tray service asking SteamVR itself every few seconds, with the overlay's own app key, disturbed the overlay
//! (SteamVR reloaded its input bindings and dropped the app's process at each of those connections). The service asks
//! SteamVR only while no overlay runs.

use orbiters_core::settings::TrackingSettings;
use orbiters_i18n::tr;
use orbiters_openvr::DeviceClass;
use orbiters_n4n0_core::menu::SMOOTHING_STRENGTHS;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// SteamVR's device indices.
const MAX_DEVICES: usize = 64;
/// The overlay reads the devices' kinds this often (and at once when one comes, goes or changes role).
const KINDS_READ_EVERY: Duration = Duration::from_secs(5);
/// ... and sends them at least this often, so the service knows they are fresh.
const KINDS_RESEND_EVERY: Duration = Duration::from_secs(10);
/// Kinds the overlay sent are trusted this long.
const KINDS_FRESH_FOR: Duration = Duration::from_secs(30);
/// Without an overlay this long while SteamVR runs, the service reads the kinds itself, this often.
const NO_OVERLAY_FOR: Duration = Duration::from_secs(20);
const SELF_READ_EVERY: Duration = Duration::from_secs(15);
/// A failed registration (SteamVR never started on this computer yet) is tried again this often.
const INSTALL_RETRY: Duration = Duration::from_secs(60);
/// SteamVR started this recently: its drivers may still be starting.
const STEAMVR_STARTING: Duration = Duration::from_secs(20);
/// Smoothing turned on this recently: the driver hooks SteamVR's tracking on its next frame.
const TURNING_ON: Duration = Duration::from_secs(3);

/// What a device index is, for its strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Headset,
    Controller,
    Tracker,
    Other,
}

impl DeviceKind {
    pub fn of(class: DeviceClass) -> Self {
        match class {
            DeviceClass::Hmd => Self::Headset,
            DeviceClass::Controller => Self::Controller,
            DeviceClass::GenericTracker => Self::Tracker,
            _ => Self::Other,
        }
    }
}

/// Every device index's kind, without the trailing ones that are nothing.
fn read_kinds(class_of: impl Fn(u32) -> DeviceClass) -> Vec<DeviceKind> {
    let mut kinds: Vec<DeviceKind> = (0..MAX_DEVICES as u32).map(|i| DeviceKind::of(class_of(i))).collect();
    while kinds.last() == Some(&DeviceKind::Other) {
        kinds.pop();
    }
    kinds
}

/// Before anything said which device is which: SteamVR's index 0 is always the headset.
fn default_kinds() -> Vec<DeviceKind> {
    vec![DeviceKind::Headset]
}

/// The overlay's side: SteamVR's devices' kinds, read through its own connection and handed to the service when
/// they change (and now and then, to say they are still so).
#[derive(Default)]
pub struct DeviceKindsWatch {
    kinds: Option<Vec<DeviceKind>>,
    read_at: Option<Instant>,
    sent_at: Option<Instant>,
    dirty: bool,
}

impl DeviceKindsWatch {
    /// A device came, went or changed role: read again on the next turn.
    pub fn changed(&mut self) {
        self.dirty = true;
    }

    /// The kinds to send at `now`, if they are due (read through `class_of`, SteamVR's class of a device index).
    pub fn due(&mut self, now: Instant, class_of: impl Fn(u32) -> DeviceClass) -> Option<Vec<DeviceKind>> {
        let read = self.dirty || self.read_at.is_none_or(|t| now.saturating_duration_since(t) >= KINDS_READ_EVERY);
        if !read {
            return None;
        }
        self.dirty = false;
        self.read_at = Some(now);
        let kinds = read_kinds(class_of);
        let send = self.kinds.as_ref() != Some(&kinds) || self.sent_at.is_none_or(|t| now.saturating_duration_since(t) >= KINDS_RESEND_EVERY);
        self.kinds = Some(kinds.clone());
        send.then(|| {
            self.sent_at = Some(now);
            kinds
        })
    }
}

/// What [`Smoothing::keep`] needs of the app's state.
pub struct World<'a> {
    pub settings: &'a TrackingSettings,
    /// SteamVR's server runs, and its process.
    pub steamvr: bool,
    pub steamvr_pid: Option<u32>,
    /// The SteamVR overlay process is connected (it sends the devices' kinds).
    pub overlay: bool,
}

#[derive(Default)]
pub struct Smoothing {
    #[cfg(windows)]
    control: Option<orbiters_smooth::Control>,
    /// The driver registered for SteamVR in this run (or why it could not be), and when that was tried.
    installed: Option<(Result<(), String>, Instant)>,
    /// Each device index's kind, from the overlay (and when it last said) or read here.
    kinds: Vec<DeviceKind>,
    kinds_from_overlay: Option<Instant>,
    kinds_read: Option<Instant>,
    overlay_absent_since: Option<Instant>,
    /// Smoothing was on at the last look, and when it was last turned on.
    on: bool,
    turned_on_at: Option<Instant>,
}

/// The driver as the app ships it: beside its program.
fn shipped_driver() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name(orbiters_smooth::install::DLL_NAME)).filter(|p| p.is_file())
}

/// Each device index's strength, from its kind and the settings' levels (headset, controllers, trackers).
fn strengths(kinds: &[DeviceKind], levels: [u8; 3]) -> [f32; MAX_DEVICES] {
    let level = |i: usize| SMOOTHING_STRENGTHS.get(levels[i] as usize).copied().unwrap_or(0.0);
    let mut out = [0.0; MAX_DEVICES];
    for (slot, kind) in out.iter_mut().zip(kinds) {
        *slot = match kind {
            DeviceKind::Headset => level(0),
            DeviceKind::Controller => level(1),
            DeviceKind::Tracker => level(2),
            DeviceKind::Other => 0.0,
        };
    }
    out
}

/// Why SteamVR, running, shows no heartbeat of the driver, as the Tracking page says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotLoaded {
    /// SteamVR's Safe Mode turned it off after a crash.
    BlockedBySafeMode,
    /// Turned off in SteamVR's Manage Add-ons.
    TurnedOff,
    /// Registered after this SteamVR started (the first registration ever, SteamVR running): its next start.
    RegisteredSince,
    /// SteamVR is still starting.
    Starting,
    /// None of these: SteamVR did not load it.
    Unknown,
}

fn not_loaded(add_on: orbiters_smooth::AddOnState, registered: Option<SystemTime>, steamvr_started: Option<SystemTime>, now: SystemTime) -> NotLoaded {
    use orbiters_smooth::AddOnState;
    match add_on {
        AddOnState::BlockedBySafeMode => return NotLoaded::BlockedBySafeMode,
        AddOnState::TurnedOff => return NotLoaded::TurnedOff,
        AddOnState::On => {}
    }
    match (registered, steamvr_started) {
        (Some(registered), Some(started)) if registered > started => NotLoaded::RegisteredSince,
        (_, Some(started)) if now.duration_since(started).is_ok_and(|d| d < STEAMVR_STARTING) => NotLoaded::Starting,
        _ => NotLoaded::Unknown,
    }
}

fn not_loaded_note(why: NotLoaded) -> String {
    match why {
        NotLoaded::BlockedBySafeMode => tr("SteamVR's Safe Mode turned the driver off: turn orbiters_smooth on in Manage Add-ons").into(),
        NotLoaded::TurnedOff => tr("Turned off in SteamVR: turn orbiters_smooth on in its Manage Add-ons").into(),
        NotLoaded::RegisteredSince => tr("Just installed: smoothing starts at SteamVR's next start (only this once)").into(),
        NotLoaded::Starting => tr("Waiting for SteamVR to start its smoothing driver").into(),
        NotLoaded::Unknown => tr("SteamVR did not load the smoothing driver: restart SteamVR once").into(),
    }
}

impl Smoothing {
    /// The overlay said which device is which.
    pub fn set_kinds(&mut self, kinds: Vec<DeviceKind>) {
        self.kinds = kinds;
        self.kinds_from_overlay = Some(Instant::now());
    }

    /// Puts the driver in step with the settings and SteamVR and says what it does now, for the Tracking page.
    pub fn keep(&mut self, w: &World) -> String {
        #[cfg(not(windows))]
        {
            let _ = w;
            tr("Smoothing needs Windows").into()
        }
        #[cfg(windows)]
        self.keep_windows(w)
    }

    #[cfg(windows)]
    fn keep_windows(&mut self, w: &World) -> String {
        let now = Instant::now();
        // Registered once and for all, whatever the setting: SteamVR then loads the driver at each of its starts and
        // smoothing turns on and off live. Again at each start of the app (a new build's DLL; nothing else changes).
        if self.installed.as_ref().is_none_or(|(r, at)| r.is_err() && now.saturating_duration_since(*at) >= INSTALL_RETRY) {
            let result = match shipped_driver() {
                Some(dll) => orbiters_smooth::install::install(&dll).map(drop).map_err(|e| format!("{e:#}")),
                None => Err(tr("this copy of Orbiters - N4N0 has no smoothing driver beside it").into()),
            };
            self.installed = Some((result, now));
        }
        // The block exists whatever the setting, so that the driver finds it, off, from SteamVR's start.
        if self.control.is_none() {
            match orbiters_smooth::Control::open_or_create() {
                Ok(control) => self.control = Some(control),
                Err(e) => return tr!("The driver cannot be reached: {error}", error = e),
            }
        }
        self.keep_kinds(w, now);
        let Some(control) = &self.control else { return String::new() };
        // Should another process have opened the block (and gone), smoothing would have stopped with it.
        control.claim();
        let enabled = w.settings.enabled;
        if enabled && !self.on {
            self.turned_on_at = Some(now);
        }
        self.on = enabled;
        let wanted = strengths(&self.kinds, w.settings.levels);
        let current = control.settings();
        // What the block holds, not what this process last wrote: nothing written over it is left standing.
        if current.strengths != wanted {
            control.set_all(&wanted);
        }
        if current.enabled != enabled {
            control.set_enabled(enabled);
        }
        if !enabled {
            return tr("Off: SteamVR tracks every device as it is").into();
        }
        let heartbeat = control.driver_heartbeat();
        if let (Some((Err(e), _)), None) = (&self.installed, &heartbeat) {
            return tr!("The driver could not be installed: {error}", error = e);
        }
        match (w.steamvr, heartbeat) {
            (false, _) => tr("Ready: smoothing starts with SteamVR").into(),
            (true, Some(status)) if status.hooks > 0 => tr("Smoothing your devices now").into(),
            (true, Some(_)) if self.turned_on_at.is_some_and(|t| now.saturating_duration_since(t) < TURNING_ON) => tr("Turning smoothing on").into(),
            (true, Some(_)) => tr("The driver runs but could not reach SteamVR's tracking").into(),
            (true, None) => {
                let install = orbiters_smooth::Installation::for_user().ok();
                let add_on = install.as_ref().map_or(orbiters_smooth::AddOnState::On, |i| i.add_on_state());
                let registered = install.as_ref().and_then(|i| i.registered_since());
                let started = w.steamvr_pid.and_then(orbiters_smooth::win::process_started);
                not_loaded_note(not_loaded(add_on, registered, started, SystemTime::now()))
            }
        }
    }

    /// The devices' kinds: the overlay's while it sends them; read here only while no overlay runs (then nothing of
    /// this app's is connected to SteamVR to disturb).
    #[cfg(windows)]
    fn keep_kinds(&mut self, w: &World, now: Instant) {
        if w.overlay {
            self.overlay_absent_since = None;
        } else if self.overlay_absent_since.is_none() {
            self.overlay_absent_since = Some(now);
        }
        let fresh = self.kinds_from_overlay.is_some_and(|t| now.saturating_duration_since(t) < KINDS_FRESH_FOR);
        if !w.steamvr {
            self.kinds_read = None;
            if !fresh {
                self.kinds = default_kinds();
            }
            return;
        }
        let absent = self.overlay_absent_since.is_some_and(|t| now.saturating_duration_since(t) >= NO_OVERLAY_FOR);
        if !fresh && absent && w.settings.enabled && self.kinds_read.is_none_or(|t| now.saturating_duration_since(t) >= SELF_READ_EVERY) {
            self.kinds_read = Some(now);
            if let Ok(client) = orbiters_openvr::Client::connect(orbiters_openvr::AppType::Background) {
                self.kinds = read_kinds(|i| client.device_class(i));
            }
        }
        if self.kinds.is_empty() {
            self.kinds = default_kinds();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_gets_its_strength() {
        use DeviceKind::*;
        let kinds = [Headset, Controller, Controller, Other, Tracker];
        let s = strengths(&kinds, [0, 2, 3]);
        assert_eq!(&s[..5], &[0.0, SMOOTHING_STRENGTHS[2], SMOOTHING_STRENGTHS[2], 0.0, SMOOTHING_STRENGTHS[3]]);
        assert!(s[5..].iter().all(|v| *v == 0.0), "devices not seen are left alone");
        assert_eq!(strengths(&kinds, [9, 9, 9])[0], 0.0, "a level the menu has no choice for is off");
        assert_eq!(strengths(&default_kinds(), [2, 2, 2])[0], SMOOTHING_STRENGTHS[2], "before any word, index 0 is the headset");
    }

    #[test]
    fn the_overlay_sends_kinds_when_they_change_and_now_and_then() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let controllers = std::cell::Cell::new(2u32);
        let class_of = |i: u32| match i {
            0 => DeviceClass::Hmd,
            1 | 2 if i <= controllers.get() => DeviceClass::Controller,
            4 => DeviceClass::TrackingReference,
            _ => DeviceClass::Invalid,
        };
        let mut watch = DeviceKindsWatch::default();
        let first = watch.due(at(0), class_of).expect("the first read is sent");
        assert_eq!(first, vec![DeviceKind::Headset, DeviceKind::Controller, DeviceKind::Controller], "base stations and empty slots are nothing");
        assert_eq!(watch.due(at(100), class_of), None, "not read again so soon");
        assert_eq!(watch.due(at(5000), class_of), None, "read again, the same: not sent");
        // A controller went: SteamVR's event makes the next turn read and send.
        controllers.set(1);
        watch.changed();
        assert_eq!(watch.due(at(5100), class_of), Some(vec![DeviceKind::Headset, DeviceKind::Controller]));
        // Unchanged, still sent every ten seconds so the service knows it is fresh.
        assert_eq!(watch.due(at(10_200), class_of), None);
        assert!(watch.due(at(15_200), class_of).is_some());
    }

    #[test]
    fn why_steamvr_has_not_loaded_it() {
        use orbiters_smooth::AddOnState;
        let now = SystemTime::now();
        let ago = |s: u64| now - Duration::from_secs(s);
        // Registered while this SteamVR ran (the first registration ever): its next start, once.
        assert_eq!(not_loaded(AddOnState::On, Some(ago(10)), Some(ago(600)), now), NotLoaded::RegisteredSince);
        // Registered long before SteamVR started: it should be there, unless SteamVR is still starting.
        assert_eq!(not_loaded(AddOnState::On, Some(ago(3600)), Some(ago(5)), now), NotLoaded::Starting);
        assert_eq!(not_loaded(AddOnState::On, Some(ago(3600)), Some(ago(600)), now), NotLoaded::Unknown);
        assert_eq!(not_loaded(AddOnState::On, None, None, now), NotLoaded::Unknown);
        // SteamVR's own settings say why first.
        assert_eq!(not_loaded(AddOnState::BlockedBySafeMode, Some(ago(10)), Some(ago(600)), now), NotLoaded::BlockedBySafeMode);
        assert_eq!(not_loaded(AddOnState::TurnedOff, Some(ago(3600)), Some(ago(600)), now), NotLoaded::TurnedOff);
        for why in [NotLoaded::BlockedBySafeMode, NotLoaded::TurnedOff, NotLoaded::RegisteredSince, NotLoaded::Starting, NotLoaded::Unknown] {
            assert!(not_loaded_note(why).len() <= 90, "a note fits N4N0's Tracking page: {}", not_loaded_note(why));
        }
    }

    #[test]
    fn kinds_go_to_the_service_as_json() {
        let json = serde_json::to_string(&vec![DeviceKind::Headset, DeviceKind::Tracker]).unwrap();
        assert_eq!(json, r#"["headset","tracker"]"#);
        assert_eq!(serde_json::from_str::<Vec<DeviceKind>>(&json).unwrap(), vec![DeviceKind::Headset, DeviceKind::Tracker]);
    }
}
