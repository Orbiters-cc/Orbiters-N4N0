//! Camera-scoped consumers. No browser or shared QR page actions run here.
use super::{Controller, Shared};
use serde::{Deserialize, Serialize};

/// Canonical deployment configuration; absent means no switch is offered.
pub const DEMO_AVATAR: Option<&str> = option_env!("ORBITERS_N4N0_DEMO_AVATAR");

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Qr {
    #[default]
    Off,
    Looking,
    NoStream,
    Found { url: String },
    Error(String),
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Demo {
    #[default]
    Idle,
    Busy,
    Done,
    Error(String),
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub qr: Qr,
    pub demo: Demo,
    pub demo_avatar: Option<String>,
    pub qr_generation: u64,
}
impl Default for Status {
    fn default() -> Self { Self { qr: Qr::Off, demo: Demo::Idle, demo_avatar: DEMO_AVATAR.map(str::to_owned), qr_generation: 0 } }
}
fn scan_event(state: &mut Qr, event: orbiters_n4n0::QrSeen) {
    use orbiters_n4n0::QrSeen;
    match event {
        QrSeen::Code(url) => {
            if let Ok(url) = orbiters_n4n0_core::qr::web_link(&url) {
                *state = Qr::Found { url };
            }
        }
        QrSeen::Camera(on) if !matches!(state, Qr::Found { .. }) => *state = if on { Qr::Looking } else { Qr::NoStream },
        QrSeen::Failed(e) => *state = Qr::Error(e),
        _ => {}
    }
}
impl Controller {
    pub fn camera_qr(self: &Shared, active: bool) {
        let mut scan = self.camera_scan.lock().unwrap();
        scan.take();
        self.update(|s| {
            s.camera_tools.qr_generation += 1;
            s.camera_tools.qr = if active { Qr::Looking } else { Qr::Off };
        });
        if !active { return; }
        if self.view().camera_open_said != Some(true) {
            self.update(|s| s.camera_tools.qr = Qr::NoStream);
            return;
        }
        let generation = self.view().camera_tools.qr_generation;
        let weak = std::sync::Arc::downgrade(self);
        let events = Box::new(move |event| {
            if let Some(ctl) = weak.upgrade() {
                ctl.update(|s| {
                    if s.camera_tools.qr_generation == generation { scan_event(&mut s.camera_tools.qr, event); }
                });
            }
        });
        match crate::qr::start(self.settings().spout_sender.clone(), events) {
            Ok(handle) => *scan = Some(handle),
            Err(e) => self.update(|s| s.camera_tools.qr = Qr::Error(e)),
        }
    }

    pub fn switch_demo_avatar(self: &Shared) {
        let Some(id) = DEMO_AVATAR else { return };
        if self.view().live_recording_running() { return; }
        let mut start = false;
        self.update(|s| {
            if s.camera_tools.demo != Demo::Busy { s.camera_tools.demo = Demo::Busy; start = true; }
        });
        if !start { return; }
        let ctl = self.clone();
        let account = ctl.view().vrchat_login.user.as_ref().map(|u|u.id.clone());
        std::thread::spawn(move || {
            let requested = orbiters_core::clock::now_ns();
            if let Err(e) = ctl.vrchat_source().select_avatar(id) {
                ctl.update(|s| s.camera_tools.demo = Demo::Error(e.to_string()));
                return;
            }
            let began = std::time::Instant::now();
            while began.elapsed() < std::time::Duration::from_secs(30) {
                if ctl.view().vrchat_login.user.as_ref().map(|u|u.id.clone()) != account {
                    ctl.update(|s| s.camera_tools.demo = Demo::Error(orbiters_i18n::tr("The account changed. Please try again.").into()));
                    return;
                }
                let observed = ctl.osc_values.lock().unwrap().latest.get(super::AVATAR_CHANGE).is_some_and(|(v, at)| *at >= requested && matches!(v.first(), Some(orbiters_core::osc::OscValue::Text(a)) if a == id));
                if observed && ctl.view().worn_kit == Some(true) {
                    ctl.update(|s| s.camera_tools.demo = Demo::Done);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            ctl.update(|s| s.camera_tools.demo = Demo::Error(orbiters_i18n::tr("VRChat did not confirm the capture avatar. Try again.").into()));
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scan_only_accepts_web_links_and_retains_found_result() {
        let mut state = Qr::Looking;
        scan_event(&mut state, orbiters_n4n0::QrSeen::Code("file:///private".into()));
        assert_eq!(state, Qr::Looking);
        scan_event(&mut state, orbiters_n4n0::QrSeen::Code("https://example.com/".into()));
        scan_event(&mut state, orbiters_n4n0::QrSeen::Camera(false));
        assert!(matches!(state, Qr::Found { .. }));
    }
}
