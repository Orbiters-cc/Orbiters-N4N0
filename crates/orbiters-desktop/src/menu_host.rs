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

/// A thumbnail as textures of the context that draws the menu: the picture, and the same as the retro look's green dots (crisp, so drawn with nearest pixels), made the first time
/// that look draws it.
pub struct Thumbnail {
    name: String,
    image: Arc<ColorImage>,
    photo: TextureHandle,
    dots: std::sync::OnceLock<TextureHandle>,
    textures: crate::overlay::PaintTextures,
}

impl Thumbnail {
    fn shown(&self, retro: bool) -> TextureId {
        if !retro {
            return self.photo.id();
        }
        self.dots.get_or_init(|| self.textures.load_texture(format!("{}-dots", self.name), menu_retro::phosphor(&self.image), TextureOptions::LINEAR)).id()
    }
}

/// Where thumbnails' textures are made.
pub struct Textures {
    textures: crate::overlay::PaintTextures,
    /// A copy of what was fetched, by name: other canvases (a panel's own context) make their textures from it.
    images: Arc<std::sync::Mutex<Kept>>,
}

/// The copies of fetched pictures, by name, at most [`Kept::MOST`]: past it the oldest goes, one at a time (all of them went at
/// the 65th before, the page's newest thumbnails with them, OVERLAY-16).
#[derive(Default)]
struct Kept {
    images: std::collections::HashMap<String, Arc<ColorImage>>,
    /// Names, oldest first.
    order: std::collections::VecDeque<String>,
}

impl Kept {
    const MOST: usize = 64;

    fn insert(&mut self, name: &str, image: Arc<ColorImage>) {
        if self.images.insert(name.to_string(), image).is_some() {
            self.order.retain(|n| n != name);
        }
        self.order.push_back(name.to_string());
        while self.order.len() > Self::MOST {
            if let Some(oldest) = self.order.pop_front() {
                self.images.remove(&oldest);
            }
        }
    }

    fn get(&self, name: &str) -> Option<&Arc<ColorImage>> {
        self.images.get(name)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn names(&self) -> impl Iterator<Item = &String> {
        self.order.iter()
    }
}

impl Textures {
    /// One copy of its pixels, whoever keeps it: the copies for other canvases, the texture and what paints it share it.
    fn of(&self, name: &str, image: ColorImage) -> Thumbnail {
        let image = Arc::new(image);
        if let Ok(mut images) = self.images.lock() {
            images.insert(name, image.clone());
        }
        let photo = self.textures.load_texture(name, image.clone(), TextureOptions::LINEAR);
        Thumbnail { name: name.to_string(), image, photo, dots: Default::default(), textures: self.textures.clone() }
    }
}

impl Pictures for Textures {
    type Picture = Thumbnail;

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
        Self::with_media(ctx, crate::media::watcher().clone())
    }

    /// The same, showing what `media` reads: a test's own watcher, which plays what the test says and never what the computer does.
    pub fn with_media(ctx: egui::Context, media: Arc<crate::media::Watcher>) -> Self {
        let host = shared::MenuHost::new(Textures { textures: crate::overlay::PaintTextures::of(&ctx), images: Default::default() }, Arc::new(crate::device::WindowsLocale), media);
        // The 3D captures' pictures are the sessions' thumbnails, on this computer.
        Self(host.with_capture_pictures(Arc::new(crate::controller::menu_shares::capture_picture)))
    }

    /// The same for an interface that draws only when something changed (the window's Home): what the menu fetches
    /// asks `ctx` for a frame when it arrives, so the page need not draw itself to look.
    pub fn waking(ctx: egui::Context) -> Self {
        let woken = ctx.clone();
        let Self(host) = Self::new(ctx);
        Self(host.with_waker(Arc::new(move || orbiters_ui::anim::request_frame(&woken))))
    }

    /// The same menu as `holder`'s: the QR code scanner and the friends it asks for are its own (MENU-13).
    pub fn holding(self, holder: orbiters_n4n0::MenuHolder) -> Self {
        Self(self.0.with_holder(holder))
    }

    /// Its page is no longer seen (a sheet closed, the window's Home left): the scanner and the friends it holds are
    /// let go at once.
    pub fn let_go(&mut self, backend: &Ctl) {
        self.0.let_go(&Service(backend));
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
    pub fn image(&self, key: &str) -> Option<Arc<ColorImage>> {
        self.0.pictures().images.lock().ok()?.get(key).cloned()
    }

    /// A thumbnail by its key once loaded, as the look draws it (`retro`: in its green).
    pub fn picture(&self, key: &str, retro: bool) -> Option<TextureId> {
        self.0.picture(key).map(|picture| picture.shown(retro))
    }

    /// Keeps a picture fetched elsewhere (the screenshot review's stand-ins) under `key`, as a fetched one.
    pub fn put_picture(&mut self, key: &str, image: ColorImage) {
        let picture = self.0.pictures().of(key, image);
        self.0.put_picture(key, picture);
    }
}

impl MenuHost {
    pub(crate) fn paint_pictures(&self, retro: bool) -> std::collections::HashMap<String, egui::TextureId> {
        // Every loaded picture, from the host: the CPU copies are at most 64, and the textures of those let go are still there
        // (verify2: a revisited first result showed no thumbnail).
        self.0.picture_keys().into_iter().filter_map(|key| self.picture(&key, retro).map(|id| (key, id))).collect()
    }
}

#[cfg(test)]
mod kept_tests {
    use super::Kept;
    use egui::ColorImage;

    /// OVERLAY-16: past 64 pictures the oldest goes, one at a time; the newest (and one asked again) stay.
    #[test]
    fn the_oldest_picture_goes_first() {
        let mut kept = Kept::default();
        for n in 0..64 {
            kept.insert(&format!("p{n}"), ColorImage::filled([1, 1], egui::Color32::BLACK).into());
        }
        kept.insert("p0", ColorImage::filled([1, 1], egui::Color32::WHITE).into());
        kept.insert("p64", ColorImage::filled([1, 1], egui::Color32::BLACK).into());
        assert!(kept.get("p1").is_none(), "the oldest went");
        assert!(kept.get("p0").is_some() && kept.get("p2").is_some() && kept.get("p64").is_some());
        assert_eq!(kept.names().count(), 64);
        assert_eq!(kept.names().last().map(String::as_str), Some("p64"));
    }

    /// DOV-7: a thumbnail made three copies of itself (its texture, the retro look's, one for other canvases), that look chosen or not.
    #[test]
    fn a_thumbnail_is_kept_once_and_its_dots_are_made_when_drawn() {
        let ctx = egui::Context::default();
        let textures = super::Textures { textures: crate::overlay::PaintTextures::of(&ctx), images: Default::default() };
        let before = ctx.tex_manager().read().num_allocated();
        let thumb = textures.of("video", ColorImage::filled([320, 180], egui::Color32::RED));
        assert_eq!(ctx.tex_manager().read().num_allocated(), before + 1);
        let kept = textures.images.lock().unwrap().get("video").unwrap().clone();
        assert!(std::sync::Arc::ptr_eq(&kept, &thumb.image), "one copy of its pixels");
        assert_eq!(thumb.shown(false), thumb.photo.id());
        assert_eq!(ctx.tex_manager().read().num_allocated(), before + 1, "nothing of the retro look yet");
        let dots = thumb.shown(true);
        assert!(dots != thumb.photo.id() && dots == thumb.shown(true));
        assert_eq!(ctx.tex_manager().read().num_allocated(), before + 2);
    }
}
