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
    Found {
        url: String,
    },
    Error(String),
    /// Still looking: a code is in view but could not be read (cut by the picture's edge, or not clear yet).
    Seen(orbiters_n4n0::QrGlimpse),
    /// Still looking: a code was read but holds no web link (Wi-Fi, text, an app's link): said, not opened.
    NotALink,
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
    fn default() -> Self {
        Self { qr: Qr::Off, demo: Demo::Idle, demo_avatar: DEMO_AVATAR.map(str::to_owned), qr_generation: 0 }
    }
}
fn scan_event(state: &mut Qr, event: orbiters_n4n0::QrSeen) {
    use orbiters_n4n0::QrSeen;
    match event {
        QrSeen::Code(url) => {
            *state = match orbiters_n4n0_core::qr::web_link(&url) {
                Ok(url) => Qr::Found { url },
                // Read, but nothing to open: said rather than nothing at all (K-qr-detect).
                Err(_) => Qr::NotALink,
            };
        }
        QrSeen::Camera(on) if !matches!(state, Qr::Found { .. }) => *state = if on { Qr::Looking } else { Qr::NoStream },
        QrSeen::Glimpse(seen) if matches!(state, Qr::Looking | Qr::Seen(_) | Qr::NotALink) => {
            *state = seen.map_or(Qr::Looking, Qr::Seen);
        }
        QrSeen::Failed(e) => *state = Qr::Error(e),
        _ => {}
    }
}
/// The Spout sender the camera's scan reads: VRChat's camera's (empty). A debug build reads the one named in
/// `qr-test-sender.txt` in its logs' folder when there is one: the sandbox shows a code there (K-qr-page), the rest of the path
/// (scan, page, laser) being the real one. Release builds never read it.
#[cfg(not(test))]
fn test_sender() -> String {
    if !cfg!(debug_assertions) {
        return String::new();
    }
    std::fs::read_to_string(orbiters_core::paths::logs_dir().join("qr-test-sender.txt")).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn begin_demo(state: &mut crate::state::Snapshot) -> bool {
    if !state.can_switch_demo_avatar() {
        return false;
    }
    state.camera_tools.demo = Demo::Busy;
    true
}
impl Controller {
    pub fn camera_qr(self: &Shared, active: bool) {
        let mut scan = self.camera_scan.lock().unwrap();
        scan.take();
        self.update(|s| {
            s.camera_tools.qr_generation += 1;
            s.camera_tools.qr = if active { Qr::Looking } else { Qr::Off };
        });
        if !active {
            self.release_camera_stream(1);
            return;
        }
        if self.require_live_camera().is_err() {
            self.update(|s| s.camera_tools.qr = Qr::NoStream);
            self.release_camera_stream(1);
            return;
        }
        let values = self.camera_values();
        if self.view().camera_open_said != Some(true) || !values.iter().any(|(p,v)|p==super::CAMERA_MODE && matches!(v,orbiters_core::osc::OscValue::Int(m) if *m != 0)) {
            self.release_camera_stream(1); self.update(|s|s.camera_tools.qr=Qr::NoStream); return;
        }
        if !self.acquire_camera_stream(1, &values) { self.update(|s|s.camera_tools.qr=Qr::NoStream); return; }
        let generation = self.view().camera_tools.qr_generation;
        let weak = std::sync::Arc::downgrade(self);
        let events: orbiters_n4n0::QrEvents = Box::new(move |event| {
            if let Some(ctl) = weak.upgrade() {
                let mut scan=ctl.camera_scan.lock().unwrap();
                let mut finished = false;
                ctl.update(|s| {
                    if s.camera_tools.qr_generation == generation && matches!(s.camera_tools.qr, Qr::Looking | Qr::NoStream | Qr::Seen(_) | Qr::NotALink) {
                        scan_event(&mut s.camera_tools.qr, event);
                        finished = matches!(s.camera_tools.qr, Qr::Found { .. } | Qr::Error(_));
                    }
                });
                if finished { scan.take(); ctl.release_camera_stream(1); }
            }
        });
        #[cfg(test)]
        { *self.test_qr_events.lock().unwrap() = Some(events); return; }
        #[cfg(not(test))]
        match crate::qr::start(test_sender(), events) {
            Ok(handle) => *scan = Some(handle),
            Err(e) => { self.update(|s| s.camera_tools.qr = Qr::Error(e)); self.release_camera_stream(1); },
        }
    }

    pub fn switch_demo_avatar(self: &Shared) {
        let Some(id) = DEMO_AVATAR else { return };
        if self.is_capturing() || self.view().capture.state == crate::state::CaptureState::Stopping {
            return;
        }
        let mut start = false;
        self.update(|s| {
            start = begin_demo(s);
        });
        if !start {
            return;
        }
        let ctl = self.clone();
        let account = ctl.view().vrchat_login.user.as_ref().map(|u| u.id.clone());
        std::thread::spawn(move || {
            let requested = orbiters_core::clock::now_ns();
            if let Err(e) = ctl.vrchat_source().select_avatar(id) {
                ctl.update(|s| s.camera_tools.demo = Demo::Error(e.to_string()));
                return;
            }
            let began = std::time::Instant::now();
            while began.elapsed() < std::time::Duration::from_secs(30) {
                if ctl.view().vrchat_login.user.as_ref().map(|u| u.id.clone()) != account {
                    ctl.update(|s| s.camera_tools.demo = Demo::Error(orbiters_i18n::tr("The account changed. Please try again.").into()));
                    return;
                }
                let observed = ctl
                    .osc_values
                    .lock()
                    .unwrap()
                    .latest
                    .get(super::AVATAR_CHANGE)
                    .is_some_and(|(v, at)| *at >= requested && matches!(v.first(), Some(orbiters_core::osc::OscValue::Text(a)) if a == id));
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
    fn demo_selection_requires_a_live_matching_session_at_the_controller() {
        let mut state = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        state.camera_tools.demo_avatar = Some("avtr_mock".into());
        state.vrchat.running = true;
        state.vrchat.world.local_player = Some("Tester".into());
        state.vrchat.world.players.insert("usr_test".into(), "Tester".into());
        state.vrchat_login.allowed = true;
        state.vrchat_login.user = Some(serde_json::from_value(serde_json::json!({"id":"usr_test","displayName":"Tester"})).unwrap());
        state.vrchat.running = false;
        assert!(!begin_demo(&mut state));
        assert_eq!(state.camera_tools.demo, Demo::Idle);
        state.vrchat.running = true;
        state.vrchat_login.user.as_mut().unwrap().id = "usr_other".into();
        assert!(!begin_demo(&mut state));
        state.vrchat_login.user.as_mut().unwrap().id = "usr_test".into();
        assert!(begin_demo(&mut state));
        assert_eq!(state.camera_tools.demo, Demo::Busy);
        assert!(!begin_demo(&mut state));
    }
    #[test]
    fn scan_only_accepts_web_links_and_retains_found_result() {
        let mut state = Qr::Looking;
        scan_event(&mut state, orbiters_n4n0::QrSeen::Code("file:///private".into()));
        assert_eq!(state, Qr::NotALink, "a code read that opens nothing is said");
        scan_event(&mut state, orbiters_n4n0::QrSeen::Code("https://example.com/".into()));
        scan_event(&mut state, orbiters_n4n0::QrSeen::Camera(false));
        assert!(matches!(state, Qr::Found { .. }));
    }

    /// K-qr-detect: a code seen but not read is said while the scan goes on (cut by the picture's edge, or not clear), and
    /// unsaid when it goes; a found page is never undone by it.
    #[test]
    fn a_glimpsed_code_is_said_until_it_is_read_or_gone() {
        use orbiters_n4n0::{QrGlimpse, QrSeen};
        let mut state = Qr::Looking;
        scan_event(&mut state, QrSeen::Glimpse(Some(QrGlimpse::Cut(None))));
        assert_eq!(state, Qr::Seen(QrGlimpse::Cut(None)));
        scan_event(&mut state, QrSeen::Glimpse(Some(QrGlimpse::Unclear)));
        assert_eq!(state, Qr::Seen(QrGlimpse::Unclear));
        scan_event(&mut state, QrSeen::Glimpse(None));
        assert_eq!(state, Qr::Looking);
        scan_event(&mut state, QrSeen::Glimpse(Some(QrGlimpse::Cut(None))));
        scan_event(&mut state, QrSeen::Code("https://vrcidlinker.com/".into()));
        assert_eq!(state, Qr::Found { url: "https://vrcidlinker.com/".into() });
        scan_event(&mut state, QrSeen::Glimpse(Some(QrGlimpse::Unclear)));
        assert!(matches!(state, Qr::Found { .. }), "a late glimpse does not undo a found page");
    }
}
