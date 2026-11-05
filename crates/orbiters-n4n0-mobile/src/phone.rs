//! What the app asks of the phone (each app's own), and N4N0's platform made of it (`orbiters_n4n0::Platform`). A
//! computer running the tests or the screenshots has [`NoPhone`].

use orbiters_n4n0::media::{NoMedia, Watcher};
use orbiters_n4n0::{Browser, Clipboard, Ears, Locale, Platform, Sounds, VoiceEnded, VoiceEvents, VoiceWorker};
use std::sync::Arc;

/// The OSC services as the system's Bonjour should announce them ([`Phone::advertise`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertised {
    /// The instance name, the same in both services (`Orbiters-N4N0-iOS-41237`).
    pub instance: String,
    /// `_osc._udp`'s port.
    pub osc_port: u16,
    /// `_oscjson._tcp`'s port.
    pub query_port: u16,
}

/// The phone, as the app and N4N0 use it.
pub trait Phone: Send + Sync + 'static {
    /// Opens a web page in the person's browser.
    fn open_url(&self, url: &str) -> anyhow::Result<()>;
    /// Copies text to the clipboard.
    fn copy(&self, text: &str) -> anyhow::Result<()>;
    /// The language and region (`fr-FR`).
    fn locale(&self) -> String;
    /// What the website calls this phone while it signs in ("Google Pixel 9").
    fn device_name(&self) -> String;
    /// Opens the page of a sign-in, whose Connect comes back to this app's loopback. In the browser, unless the phone
    /// has a better place for it, where the app stays alive while it is shown.
    fn open_sign_in(&self, url: &str) -> anyhow::Result<()> {
        self.open_url(url)
    }
    /// The sign-in ended, however it did: a page [`Phone::open_sign_in`] still shows goes.
    fn close_sign_in(&self) {}
    /// [`orbiters_core::osc::Discovery::Platform`]: the system's Bonjour announces these services from now on (`None`:
    /// no more).
    fn advertise(&self, _services: Option<Advertised>) {}
    /// [`orbiters_core::osc::Discovery::Platform`]: the system's Bonjour looks for VRChat's OSCQuery service (`_oscjson._tcp`)
    /// or stops, what it sees coming back through [`crate::app::Core::saw`].
    fn browse(&self, _on: bool) {}
    /// Whether Android lets the app run without battery restrictions (`None`: cannot tell).
    fn battery_unrestricted(&self) -> Option<bool> {
        None
    }
    /// Opens Android's battery optimisation settings.
    fn open_battery_settings(&self) -> anyhow::Result<()> {
        anyhow::bail!("No battery settings here.")
    }
    /// Ends the app: the service, its notification and the window.
    fn quit(&self) {}
}

/// No phone: a computer running the tests or the screenshots.
pub struct NoPhone;

impl Phone for NoPhone {
    fn open_url(&self, _url: &str) -> anyhow::Result<()> {
        anyhow::bail!("No browser here.")
    }
    fn copy(&self, _text: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn locale(&self) -> String {
        "en-US".into()
    }
    fn device_name(&self) -> String {
        "Phone".into()
    }
}

struct PhoneClipboard(Arc<dyn Phone>);

impl Clipboard for PhoneClipboard {
    fn copy(&self, text: &str) -> Result<(), String> {
        self.0.copy(text).map_err(|e| format!("Could not copy: {e}"))
    }
}

struct PhoneBrowser(Arc<dyn Phone>);

impl Browser for PhoneBrowser {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        self.0.open_url(url)
    }
}

/// No sounds of N4N0's own on the phone yet.
struct Quiet;

impl Sounds for Quiet {
    fn press(&self) {}
}

struct PhoneLocale(Arc<dyn Phone>);

impl Locale for PhoneLocale {
    fn locale(&self) -> String {
        self.0.locale()
    }
}

/// N4N0's ears are not on the phone yet: starting them says so.
struct NoEars;

impl Ears for NoEars {
    fn spawn(&self, _events: VoiceEvents, _ended: VoiceEnded) -> anyhow::Result<Box<dyn VoiceWorker>> {
        anyhow::bail!("N4N0 does not listen on a phone yet.")
    }
}

/// N4N0's platform on `phone`: its clipboard, browser and language; no sounds, ears, music or mouse yet.
pub fn platform(phone: Arc<dyn Phone>) -> Platform {
    Platform {
        clipboard: Box::new(PhoneClipboard(phone.clone())),
        browser: Box::new(PhoneBrowser(phone.clone())),
        sounds: Box::new(Quiet),
        locale: Box::new(PhoneLocale(phone)),
        ears: Box::new(NoEars),
        media: Arc::new(Watcher::new(|| Box::new(NoMedia))),
        mouse: None,
    }
}
