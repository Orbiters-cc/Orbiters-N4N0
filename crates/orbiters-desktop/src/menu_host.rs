//! N4N0's menu (`orbiters_n4n0_core::menu`) run where it is drawn: the SteamVR overlay's menu in the play space, and Home,
//! the window's main page. `orbiters_n4n0::menu_host` keeps it in step with the tray service and fetches what it
//! shows; here its pictures are egui textures (in the retro look's green), and what it asks goes to the service
//! over the backend.

use crate::backend::Ctl;
use crate::ipc::N4n0Command;
use crate::overlay::menu_retro;
use crate::state::Snapshot;
use egui::{ColorImage, TextureHandle, TextureId, TextureOptions};
use orbiters_core::settings::Settings;
use orbiters_n4n0::menu_host::{self as shared, MenuService, Pictures};
use orbiters_n4n0_core::menu::Effect;
use orbiters_n4n0_core::picture::Rgb;
use std::sync::Arc;

/// Thumbnails as textures of the context that draws the menu: the picture, and the same as the retro look's green dots
/// (crisp, so drawn with nearest pixels).
pub struct Textures {
    ctx: egui::Context,
    /// A copy of what was fetched, by name: other canvases (a panel's own context) make their textures from it.
    images: Arc<std::sync::Mutex<std::collections::HashMap<String, ColorImage>>>,
}

impl Textures {
    fn of(&self, name: &str, image: ColorImage) -> (TextureHandle, TextureHandle) {
        if let Ok(mut images) = self.images.lock() {
            if images.len() > 64 {
                images.clear();
            }
            images.insert(name.to_string(), image.clone());
        }
        let green = menu_retro::phosphor(&image);
        (self.ctx.load_texture(name, image, TextureOptions::LINEAR), self.ctx.load_texture(format!("{name}-dots"), green, TextureOptions::LINEAR))
    }
}

impl Pictures for Textures {
    type Picture = (TextureHandle, TextureHandle);

    fn make(&self, name: &str, image: &Rgb) -> Self::Picture {
        let rgba: Vec<u8> = image.pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
        self.of(name, ColorImage::from_rgba_unmultiplied([image.width, image.height], &rgba))
    }
}

/// What the menu asks of the service, over the backend.
struct Service<'a>(&'a Ctl);

impl MenuService for Service<'_> {
    fn command(&self, command: N4n0Command) {
        self.0.n4n0(command);
    }

    fn save_settings(&self, before: &Settings, after: &Settings) {
        self.0.save_changes(before, after);
    }
}

/// The shared menu host, followed from the service's snapshot.
pub struct MenuHost(shared::MenuHost<Textures>);

impl std::ops::Deref for MenuHost {
    type Target = shared::MenuHost<Textures>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for MenuHost {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl MenuHost {
    /// A menu drawn by `ctx`, in this computer's language and keyboard, showing this process's music.
    pub fn new(ctx: egui::Context) -> Self {
        let host = shared::MenuHost::new(Textures { ctx, images: Default::default() }, Arc::new(crate::device::WindowsLocale), crate::media::watcher().clone());
        // The 3D captures' pictures are the sessions' thumbnails, on this computer.
        Self(host.with_capture_pictures(Arc::new(crate::controller::menu_shares::capture_picture)))
    }

    /// Opens the menu on Home with the settings it starts from (keyboard, the 3D picture's timer and length).
    pub fn open(&mut self, snap: &Snapshot) {
        self.0.open(&snap.settings);
    }

    /// Keeps the menu's picture of what the service knows.
    pub fn follow(&mut self, snap: &Snapshot) {
        self.0.follow(&snap.n4n0, &snap.settings);
    }

    /// Does what the menu asked; returns whether it asked to close (its close button, or idle), which the host does.
    pub fn effects(&mut self, effects: Vec<Effect>, snap: &Snapshot, backend: &Ctl) -> bool {
        self.0.effects(effects, &snap.settings, &Service(backend))
    }

    /// A fetched picture's pixels by its name, for a canvas of another context.
    pub fn image(&self, key: &str) -> Option<ColorImage> {
        self.0.pictures().images.lock().ok()?.get(key).cloned()
    }

    /// A thumbnail by its key once loaded, as the look draws it (`retro`: in its green).
    pub fn picture(&self, key: &str, retro: bool) -> Option<TextureId> {
        self.0.picture(key).map(|(photo, dots)| if retro { dots.id() } else { photo.id() })
    }

    /// Keeps a picture fetched elsewhere (the screenshot review's stand-ins) under `key`, as a fetched one.
    pub fn put_picture(&mut self, key: &str, image: ColorImage) {
        let picture = self.0.pictures().of(key, image);
        self.0.put_picture(key, picture);
    }
}
