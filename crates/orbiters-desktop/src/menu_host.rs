//! N4N0's menu (`orbiters_orb::menu`) run where it is drawn: the SteamVR overlay's menu in the play space, and Home,
//! the window's main page. Both keep it in step with the tray service (listening, fast walk, calls, the light, the 3D
//! picture's countdown), fetch YouTube's suggestions, results and thumbnails here, read the music of this process's
//! media watcher, and send the rest (calls, the clipboard, a 3D picture, fast walk, the light) to the service.

use crate::backend::Ctl;
use crate::controller::screen as shared;
use crate::ipc::OrbCommand;
use crate::overlay::menu_retro;
use crate::state::{CallLink, CallPhase, Snapshot};
use egui::{ColorImage, TextureHandle, TextureId, TextureOptions};
use orbiters_orb::menu::{Call, Contact, Effect, Menu, Page, SuggestQueue, Target};
use orbiters_orb::{picture, youtube};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A thumbnail that failed is tried again after this (times the tries), so many times.
const THUMB_RETRY: Duration = Duration::from_secs(3);
const THUMB_TRIES: u8 = 3;

enum Done {
    Suggestions { id: u64, text: String, result: anyhow::Result<Vec<String>> },
    Results { id: u64, more: bool, result: anyhow::Result<youtube::Results> },
    Thumbnail { id: String, tries: u8, image: Option<ColorImage> },
}

enum Thumb {
    Loading,
    /// The picture, and the same as the retro look's green dots.
    Ready(TextureHandle, TextureHandle),
    /// The last try failed at `at`.
    Failed {
        at: Instant,
        tries: u8,
    },
}

/// Sets `slot` to `value`, saying whether that changed it.
fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

pub struct MenuHost {
    pub menu: Menu,
    /// Where thumbnails become textures: the context that draws the menu.
    ctx: egui::Context,
    thumbs: HashMap<String, Thumb>,
    suggest: SuggestQueue,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
    /// The media reading the menu shows (its version: [`crate::media::Watcher::reading`]).
    media_seen: u64,
    /// Something the menu shows changed since it was last drawn.
    pub dirty: bool,
}

impl MenuHost {
    pub fn new(ctx: egui::Context) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self { menu: Menu::default(), ctx, thumbs: HashMap::new(), suggest: SuggestQueue::default(), done_tx, done_rx, media_seen: 0, dirty: true }
    }

    /// Opens the menu on Home with the settings it starts from (keyboard, the 3D picture's timer and length). It has
    /// its six tiles: both hosts draw every page.
    pub fn open(&mut self, snap: &Snapshot) {
        let settings = &snap.settings;
        self.menu.layout = shared::keyboard(settings.orb.menu.keyboard);
        self.menu.photo_timer = settings.photo_3d_timer;
        self.menu.photo_length = shared::photo_length(settings.photo_3d);
        self.menu.six_tiles = true;
        self.menu.open();
        self.dirty = true;
    }

    /// A press on `target`: what it does follows from the effects it returns.
    pub fn press(&mut self, target: Target) -> Vec<Effect> {
        self.dirty = true;
        self.menu.press(target)
    }

    /// Keeps the menu's picture of what the service knows (calls, fast walk, listening, the picture's countdown),
    /// field by field: the menu (its results and suggestions included) is not copied every frame to find a change.
    pub fn follow(&mut self, snap: &Snapshot) {
        let m = &mut self.menu;
        let mut changed = set(&mut m.listening, snap.orb.listening);
        // What N4N0 says shows on Home too.
        if let Some(panel) = &snap.orb.panel {
            if m.heard_private || !m.heard.as_deref().is_some_and(|h| h.split_whitespace().eq(panel.split_whitespace())) {
                m.heard = Some(panel.split_whitespace().collect::<Vec<_>>().join(" "));
                m.heard_private = false;
                changed = true;
            }
        }
        changed |= set(&mut m.fast_walk, snap.settings.walk_boost.enabled);
        changed |= set(&mut m.desktop_mode, snap.orb.desktop_mode);
        changed |= set(&mut m.light, snap.orb.light.map(|l| orbiters_orb::menu::Light { on: l.on, color: l.color, level: l.level }));
        changed |= set(&mut m.countdown, snap.orb.photo_countdown);
        changed |= set(&mut m.photo_timer, snap.settings.photo_3d_timer);
        changed |= set(&mut m.photo_length, shared::photo_length(snap.settings.photo_3d));
        let calls = &snap.orb.calls;
        let (ready, note) = match &calls.link {
            CallLink::Linked { .. } => (true, ""),
            CallLink::Off => (false, "Calls are off: Orbiters - N4N0 › N4N0 › Calls"),
            CallLink::SignedOut => (false, "Sign in to Orbiters to call friends"),
            CallLink::NoAccount => (false, "VRChat's account here is unknown: open VRChat"),
            CallLink::NotLinked { .. } => (false, "Link this VRChat account on Orbiters to call"),
        };
        let same_contacts = m.contacts.len() == calls.contacts.len() && m.contacts.iter().zip(&calls.contacts).all(|(a, b)| a.id == b.id && a.name == b.name);
        if !same_contacts || m.calls_ready != ready || m.call_note != note {
            m.set_contacts(calls.contacts.iter().map(|c| Contact { id: c.id.clone(), name: c.name.clone() }).collect(), ready, note);
            changed = true;
        }
        if calls.call.is_some() || m.call.is_some() {
            let call = calls.call.as_ref().map(|c| match c.phase {
                CallPhase::Ringing => Call::Ringing { name: c.name.clone() },
                CallPhase::Calling | CallPhase::Connecting => Call::Calling { name: c.name.clone() },
                CallPhase::Connected => Call::Connected {
                    name: c.name.clone(),
                    seconds: c.connected_at_ms.map_or(0, |at| crate::controller::unix_ms().saturating_sub(at) / 1000),
                    muted: c.muted,
                },
            });
            if m.call != call {
                m.set_call(call);
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    /// The music the menu shows, from this process's media reading (read only while a menu wants it), and its art as a
    /// texture.
    pub fn follow_media(&mut self) {
        let media = crate::media::watcher();
        media.want();
        let (version, reading) = media.reading();
        if version == self.media_seen {
            return;
        }
        self.media_seen = version;
        self.menu.media = reading.media;
        self.menu.volume = reading.volume;
        if let Some(art) = reading.art {
            if !matches!(self.thumbs.get(&art.key), Some(Thumb::Ready(..))) {
                let rgba: Vec<u8> = art.rgb.pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
                let image = ColorImage::from_rgba_unmultiplied([art.rgb.width, art.rgb.height], &rgba);
                let ready = self.ready(art.key.clone(), image);
                self.thumbs.insert(art.key.clone(), ready);
            }
        }
        self.dirty = true;
    }

    /// What came back from YouTube, the suggestions once typing paused (one request at a time), and the thumbnails of
    /// the results shown that failed, once their wait is over. Returns what follows from them.
    pub fn poll(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        while let Ok(done) = self.done_rx.try_recv() {
            match done {
                Done::Suggestions { id, text, result } => {
                    self.suggest.answered(id);
                    match result {
                        Ok(list) => self.menu.suggestions_arrived(id, &text, list),
                        Err(e) => eprintln!("orb menu: YouTube suggestions: {e:#}"),
                    }
                }
                Done::Results { id, more, result: Ok(results) } => effects.extend(self.menu.results_arrived(id, results, more)),
                Done::Results { id, result: Err(e), .. } => self.menu.results_failed(id, &e.to_string()),
                Done::Thumbnail { id, tries, image } => {
                    let thumb = match image {
                        Some(image) => self.ready(format!("yt-{id}"), image),
                        None => Thumb::Failed { at: Instant::now(), tries: tries + 1 },
                    };
                    self.thumbs.insert(id, thumb);
                }
            }
            self.dirty = true;
        }
        if let Some((id, text)) = self.suggest.due(Instant::now()) {
            let (tx, lang) = (self.done_tx.clone(), language_now());
            std::thread::spawn(move || {
                let result = youtube::suggestions(&text, &lang);
                let _ = tx.send(Done::Suggestions { id, text, result });
            });
        }
        if self.menu.page == Page::Results {
            let now = Instant::now();
            let retry: Vec<String> = self
                .menu
                .page_results()
                .iter()
                .filter(
                    |v| matches!(self.thumbs.get(&v.id), Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32),
                )
                .map(|v| v.id.clone())
                .collect();
            if !retry.is_empty() {
                self.thumbnails(retry);
            }
        }
        effects
    }

    /// Fetches the thumbnails among `ids` not here yet (one that failed is tried again a little later, a few times).
    fn thumbnails(&mut self, ids: Vec<String>) {
        if self.thumbs.len() > 60 {
            self.thumbs.retain(|id, _| ids.contains(id));
        }
        let now = Instant::now();
        for id in ids {
            let tries = match self.thumbs.get(&id) {
                None => 0,
                Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32 => *tries,
                Some(_) => continue,
            };
            self.thumbs.insert(id.clone(), Thumb::Loading);
            let tx = self.done_tx.clone();
            std::thread::spawn(move || {
                let image = youtube::thumbnail(&id).ok().and_then(|jpeg| picture::rgb_jpeg(&jpeg).ok()).map(|rgb| {
                    let rgba: Vec<u8> = rgb.pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
                    ColorImage::from_rgba_unmultiplied([rgb.width, rgb.height], &rgba)
                });
                let _ = tx.send(Done::Thumbnail { id, tries, image });
            });
        }
    }

    /// Does what the menu asked; returns whether it asked to close (its close button, or idle), which the host does.
    pub fn effects(&mut self, effects: Vec<Effect>, snap: &Snapshot, backend: &Ctl) -> bool {
        if effects.is_empty() {
            return false;
        }
        self.dirty = true;
        let mut close = false;
        for effect in effects {
            match effect {
                // Asked for once typing pauses, one request at a time ([`MenuHost::poll`] sends it).
                Effect::Suggest { id, text } => self.suggest.want(id, text, Instant::now()),
                Effect::Search { id, query } => {
                    let (tx, lang) = (self.done_tx.clone(), language_now());
                    std::thread::spawn(move || {
                        let result = youtube::search(&query, &lang);
                        let _ = tx.send(Done::Results { id, more: false, result });
                    });
                }
                Effect::More { id, token, .. } => {
                    let (tx, lang) = (self.done_tx.clone(), language_now());
                    std::thread::spawn(move || {
                        let result = youtube::more(&token, &lang);
                        let _ = tx.send(Done::Results { id, more: true, result });
                    });
                }
                Effect::Thumbnails(ids) => self.thumbnails(ids),
                Effect::Copy { link, .. } => backend.orb(OrbCommand::CopyText { text: link }),
                Effect::FastWalk(on) => {
                    let mut settings = snap.settings.clone();
                    settings.walk_boost.enabled = on;
                    backend.save_changes(&snap.settings, &settings);
                }
                Effect::CallContact { id } => backend.orb(OrbCommand::Call { contact: id }),
                Effect::Answer => backend.orb(OrbCommand::AnswerCall),
                Effect::HangUp => backend.orb(OrbCommand::EndCall),
                Effect::PhotoChoice { timer, length } | Effect::Photo3d { timer, length } => {
                    let take = matches!(effect, Effect::Photo3d { .. });
                    let mut settings = snap.settings.clone();
                    settings.photo_3d_timer = timer;
                    settings.photo_3d = shared::photo_setting(length);
                    backend.save_changes(&snap.settings, &settings);
                    if take {
                        backend.orb(OrbCommand::Photo3DIn { seconds: timer });
                    }
                }
                Effect::Media(command) => crate::media::watcher().command(command),
                Effect::Light(light) => backend.orb(OrbCommand::Light { on: light.on, color: light.color, level: light.level }),
                Effect::Close => close = true,
            }
        }
        close
    }

    /// A thumbnail by its key once loaded, as the look draws it (`retro`: its green dots).
    pub fn picture(&self, key: &str, retro: bool) -> Option<TextureId> {
        match self.thumbs.get(key) {
            Some(Thumb::Ready(photo, dots)) => Some(if retro { dots.id() } else { photo.id() }),
            _ => None,
        }
    }

    /// A picture ready to show: its texture, and its green dots for the retro look (crisp, so drawn with nearest pixels).
    fn ready(&self, name: String, image: ColorImage) -> Thumb {
        let dots = menu_retro::dots(&image, 1);
        Thumb::Ready(self.ctx.load_texture(name.clone(), image, TextureOptions::LINEAR), self.ctx.load_texture(name + "-dots", dots, TextureOptions::NEAREST))
    }
}

/// The language YouTube is asked in (`fr`): `LANG` when set, else Windows' locale.
fn language_now() -> String {
    let locale = std::env::var("LANG").unwrap_or_default();
    let code = locale.split(['-', '_', '.']).next().unwrap_or("").to_lowercase();
    if code.len() == 2 {
        code
    } else {
        sys_language()
    }
}

/// This computer's language, for YouTube (`fr`), from Windows' locale.
fn sys_language() -> String {
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buf = [0u16; 85];
        // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide, as the call requires.
        let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
        if n > 1 {
            let name = String::from_utf16_lossy(&buf[..n as usize - 1]);
            if let Some(code) = name.split('-').next().filter(|c| c.len() == 2) {
                return code.to_lowercase();
            }
        }
    }
    "en".into()
}
