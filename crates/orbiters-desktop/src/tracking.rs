//! Tracking smoothing (N4N0's Tracking page, `settings.tracking`): Orbiters - N4N0's SteamVR driver
//! (`orbiters-smooth-driver`, shipped beside the app as `driver_orbiters_smooth.dll`) smooths each device's pose as
//! SteamVR passes it on, damping SteamVR's prediction, by the strength the tray service writes for it in their shared
//! control block (`orbiters_smooth`): the headset, the controllers and the trackers each their own, by what SteamVR
//! says each device is. Switched on, the driver is installed for SteamVR (its folder and `openvrpaths.vrpath`), which
//! loads it at its next start; switched off, it stays installed and passes every pose on untouched. The driver smooths
//! only while the tray service runs: it owns the control block.

use orbiters_core::settings::TrackingSettings;
use orbiters_orb::menu::SMOOTHING_STRENGTHS;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// SteamVR's devices are asked their kind this often while smoothing is on (a tracker switched on later).
const CLASSES_EVERY: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Smoothing {
    #[cfg(windows)]
    control: Option<orbiters_smooth::Control>,
    /// The driver was installed in this run (or why it could not be).
    installed: Option<Result<(), String>>,
    /// The strengths last written, and when the devices' kinds were last read.
    written: Option<[f32; orbiters_smooth::MAX_DEVICES]>,
    classes_read: Option<Instant>,
    kinds: Vec<Kind>,
}

/// What a device index is, for its strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Headset,
    Controller,
    Tracker,
    Other,
}

/// The driver as the app ships it: beside its program.
fn shipped_driver() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name(orbiters_smooth::install::DLL_NAME)).filter(|p| p.is_file())
}

/// Each device index's strength, from its kind and the settings' levels (headset, controllers, trackers).
fn strengths(kinds: &[Kind], levels: [u8; 3]) -> [f32; orbiters_smooth::MAX_DEVICES] {
    let level = |i: usize| SMOOTHING_STRENGTHS.get(levels[i] as usize).copied().unwrap_or(0.0);
    let mut out = [0.0; orbiters_smooth::MAX_DEVICES];
    for (slot, kind) in out.iter_mut().zip(kinds) {
        *slot = match kind {
            Kind::Headset => level(0),
            Kind::Controller => level(1),
            Kind::Tracker => level(2),
            Kind::Other => 0.0,
        };
    }
    out
}

impl Smoothing {
    /// Puts the driver in step with `settings` (SteamVR running or not, `steamvr`) and says what it does now, for the
    /// Tracking page.
    pub fn keep(&mut self, settings: &TrackingSettings, steamvr: bool) -> String {
        #[cfg(not(windows))]
        {
            let _ = (settings, steamvr);
            "Smoothing needs Windows".into()
        }
        #[cfg(windows)]
        self.keep_windows(settings, steamvr)
    }

    #[cfg(windows)]
    fn keep_windows(&mut self, settings: &TrackingSettings, steamvr: bool) -> String {
        if !settings.enabled {
            if let Some(control) = &self.control {
                control.set_enabled(false);
            }
            return "Off: SteamVR tracks every device as it is".into();
        }
        // Installed once a run (an update brings a new driver; SteamVR takes it at its next start).
        if self.installed.is_none() {
            self.installed = Some(match shipped_driver() {
                Some(dll) => orbiters_smooth::install::install(&dll).map(drop).map_err(|e| format!("{e:#}")),
                None => Err("this copy of Orbiters - N4N0 has no smoothing driver beside it".into()),
            });
        }
        if let Some(Err(e)) = &self.installed {
            return format!("The driver could not be installed: {e}");
        }
        if self.control.is_none() {
            match orbiters_smooth::Control::open_or_create() {
                Ok(control) => self.control = Some(control),
                Err(e) => return format!("The driver cannot be reached: {e}"),
            }
        }
        let Some(control) = &self.control else { return String::new() };
        control.set_enabled(true);
        if steamvr && self.classes_read.is_none_or(|t| t.elapsed() >= CLASSES_EVERY) {
            if let Ok(client) = orbiters_openvr::Client::connect(orbiters_openvr::AppType::Background) {
                use orbiters_openvr::DeviceClass;
                self.kinds = (0..orbiters_smooth::MAX_DEVICES as u32)
                    .map(|i| match client.device_class(i) {
                        DeviceClass::Hmd => Kind::Headset,
                        DeviceClass::Controller => Kind::Controller,
                        DeviceClass::GenericTracker => Kind::Tracker,
                        _ => Kind::Other,
                    })
                    .collect();
                self.classes_read = Some(Instant::now());
            }
        }
        let wanted = strengths(&self.kinds, settings.levels);
        if self.written != Some(wanted) {
            control.set_all(&wanted);
            self.written = Some(wanted);
        }
        match (steamvr, control.driver_heartbeat()) {
            (true, Some(status)) if status.hooks > 0 => "Smoothing your devices now".into(),
            (true, Some(_)) => "The driver runs but SteamVR gave it no device to smooth".into(),
            (true, None) => "Restart SteamVR to start smoothing (it loads drivers when it starts)".into(),
            (false, _) => "Ready: smoothing starts with SteamVR".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_gets_its_strength() {
        let kinds = [Kind::Headset, Kind::Controller, Kind::Controller, Kind::Other, Kind::Tracker];
        let s = strengths(&kinds, [0, 2, 3]);
        assert_eq!(&s[..5], &[0.0, SMOOTHING_STRENGTHS[2], SMOOTHING_STRENGTHS[2], 0.0, SMOOTHING_STRENGTHS[3]]);
        assert!(s[5..].iter().all(|v| *v == 0.0), "devices not seen are left alone");
        assert_eq!(strengths(&kinds, [9, 9, 9])[0], 0.0, "a level the menu has no choice for is off");
    }
}
