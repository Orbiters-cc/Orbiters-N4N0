//! VRChat's pictures kept on this computer, in the very frame that asks for them (REQUEST28: "so I never see any loading or
//! unloaded pictures"), and one texture of each for every page of a window.
//!
//! A frame reads them itself within a budget: the first frames of a window all of what they show, later ones a few small
//! copies (a scroll's next rows); what does not fit comes from the loader threads a frame or two later. The textures are kept
//! by name and not by page or session: the sign-in's state changing, another page opened, change nothing of what is shown.

use super::profile_pictures::{Kept, Profiles, Size};
use super::Picture;
use crate::overlay::PaintTextures;
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What the first frames that ask may take together (a screen of rows, cards and banners), and how many those are.
const FIRST: Duration = Duration::from_millis(120);
const FIRST_FRAMES: u32 = 4;
/// What each later frame may.
const EACH: Duration = Duration::from_millis(4);
/// Textures kept for every page, at most: the ones asked longest ago go (a page that shows one keeps its own hold of it).
const TEXTURES_MAX: usize = 64 << 20;

#[derive(Default)]
struct Budget {
    frame: f64,
    frames: u32,
    spent: Duration,
}

#[cfg(not(test))]
fn budget<R>(f: impl FnOnce(&mut Budget) -> R) -> R {
    static BUDGET: Mutex<Budget> = Mutex::new(Budget { frame: f64::NEG_INFINITY, frames: 0, spent: Duration::ZERO });
    f(&mut BUDGET.lock().unwrap_or_else(|e| e.into_inner()))
}
/// Tests draw on many threads at once: each has its own.
#[cfg(test)]
fn budget<R>(f: impl FnOnce(&mut Budget) -> R) -> R {
    thread_local! {
        static BUDGET: std::cell::RefCell<Budget> = const { std::cell::RefCell::new(Budget { frame: f64::NEG_INFINITY, frames: 0, spent: Duration::ZERO }) };
    }
    BUDGET.with(|b| f(&mut b.borrow_mut()))
}

/// `f`, when the frame of `now` (egui's clock) has time left for it; `first_only`: only in the first frames (what decodes
/// in milliseconds, not in a fraction of one).
pub fn read<T>(now: f64, first_only: bool, f: impl FnOnce() -> T) -> Option<T> {
    let left = budget(|b| {
        if b.frame != now {
            b.frame = now;
            b.frames = b.frames.saturating_add(1);
            if b.frames > FIRST_FRAMES {
                b.spent = Duration::ZERO;
            }
        }
        let first = b.frames <= FIRST_FRAMES;
        b.spent < if first { FIRST } else { EACH } && (first || !first_only)
    });
    if !left {
        return None;
    }
    let from = Instant::now();
    let out = f();
    let took = from.elapsed();
    budget(|b| b.spent += took);
    Some(out)
}

/// The next frames are a window's first again (tests, pictures of the page: each stands for a start).
pub fn start_again() {
    budget(|b| *b = Budget { frame: f64::NEG_INFINITY, ..Budget::default() });
    textures().clear();
}

struct Texture {
    texture: TextureHandle,
    used: Instant,
}

/// By the interface they are of (a texture is one interface's alone: what holds it keeps that interface's textures, so its
/// address is never another's) and their name.
type Textures = HashMap<(usize, String), Texture>;

fn textures() -> std::sync::MutexGuard<'static, Textures> {
    static TEXTURES: OnceLock<Mutex<Textures>> = OnceLock::new();
    TEXTURES.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// What names a picture of VRChat's, whatever page, sign-in state or address asks; none for other pictures.
pub fn name(picture: &Picture) -> Option<String> {
    Some(match picture {
        Picture::Profile { scope, url, .. } => face_name(scope, url),
        Picture::World { world_id, url, .. } | Picture::Remote { world_id, url, large: false } | Picture::RemoteKept { world_id, url } => format!("r {}", super::remote_file_name(world_id, url, false)),
        Picture::Avatar { avatar_id, url, .. } => format!("a {}", super::remote_file_name(avatar_id, url, false)),
        _ => return None,
    })
}

fn face_name(scope: &crate::controller::vrchat_data::Scope, url: &str) -> String {
    format!("f {:?} {}", scope.account(), super::profile_pictures::picture_key(url))
}

/// The texture of that name, when it is here.
pub fn texture(of: &PaintTextures, name: &str) -> Option<TextureHandle> {
    let mut all = textures();
    let t = all.get_mut(&(of.id(), name.to_string()))?;
    t.used = Instant::now();
    Some(t.texture.clone())
}

pub fn keep(of: &PaintTextures, name: String, texture: &TextureHandle) {
    let mut all = textures();
    all.insert((of.id(), name), Texture { texture: texture.clone(), used: Instant::now() });
    let mut total: usize = all.values().map(|t| t.texture.byte_size()).sum();
    if total <= TEXTURES_MAX {
        return;
    }
    let mut by_use: Vec<(Instant, (usize, String), usize)> = all.iter().map(|(k, t)| (t.used, k.clone(), t.texture.byte_size())).collect();
    by_use.sort();
    for (used, k, size) in by_use {
        // What was asked a moment ago is on screen.
        if total <= TEXTURES_MAX / 4 * 3 || used.elapsed() < Duration::from_secs(2) {
            break;
        }
        all.remove(&k);
        total -= size;
    }
}

/// The picture as kept on this computer, read now: itself (`true`), or a person's before it.
pub fn kept(picture: &Picture, now: f64) -> Option<(ColorImage, bool)> {
    match picture {
        Picture::Profile { scope, url, user } => match read(now, false, || Profiles::of(scope.account().as_ref()).now(url, user.as_deref(), Size::Face))? {
            Kept::Exact(image) => Some((image, true)),
            Kept::Older(image) => Some((image, false)),
            Kept::Nothing => None,
        },
        Picture::World { world_id, url, .. } | Picture::RemoteKept { world_id, url } => read(now, true, || super::kept_world(world_id, url))?.map(|i| (i, true)),
        Picture::Remote { world_id, url, large: false } => read(now, true, || super::public_api().1.then(|| super::kept_world(world_id, url)).flatten())?.map(|i| (i, true)),
        Picture::Avatar { avatar_id, url, .. } => read(now, true, || super::kept_avatar(avatar_id, url))?.map(|i| (i, true)),
        _ => None,
    }
}

/// A wide picture (a group's banner, its icon) as it is here.
pub enum Wide {
    Ready(TextureHandle),
    /// Kept, and this frame has no time left: at the next one.
    Later,
    No,
}

/// The wide picture at `url` of the account of `scope`, without the network.
pub fn wide(ctx: &egui::Context, scope: &crate::controller::vrchat_data::Scope, url: &str) -> Wide {
    let Some(account) = scope.account() else { return Wide::No };
    let of = PaintTextures::of(ctx);
    let name = format!("w {account:?} {}", super::profile_pictures::picture_key(url));
    if let Some(t) = texture(&of, &name) {
        return Wide::Ready(t);
    }
    let profiles = Profiles::of(Some(&account));
    if !profiles.has(url, Size::Wide) {
        return Wide::No;
    }
    match read(ctx.input(|i| i.time), false, || profiles.now(url, None, Size::Wide)) {
        Some(Kept::Exact(image)) => {
            let t = of.load_texture(format!("kept {name}"), image, TextureOptions::LINEAR); // i18n: not shown (a texture's name)
            keep(&of, name, &t);
            Wide::Ready(t)
        }
        Some(_) => Wide::No,
        None => {
            ctx.request_repaint();
            Wide::Later
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frames_read_a_screen_and_later_ones_a_little() {
        start_again();
        // What a read takes, made up.
        let took = |ms: u64| budget(|b| b.spent += Duration::from_millis(ms));
        let read = |now: f64, first_only: bool| super::read(now, first_only, || ()).is_some();
        assert!(read(1.0, true), "a first frame reads all it shows");
        took(60);
        assert!(read(1.0, true) && read(2.0, false), "while there is time");
        took(70);
        assert!(!read(2.0, false) && !read(3.0, false), "the first frames' time is over");
        let _ = read(4.0, false);
        assert!(!read(5.0, true), "what takes long is the loaders' from here on");
        assert!(read(5.0, false));
        took(6);
        assert!(!read(5.0, false));
        assert!(read(6.0, false), "each frame has its own");
        start_again();
    }

    /// A picture kept is in the very frame that asks, for every page and whatever the sign-in's state is by then.
    #[test]
    fn a_kept_picture_is_in_the_frame_that_asks() {
        use crate::controller::vrchat_data::Scope;
        use orbiters_core::vrchat_store as store;
        use orbiters_core::launcher::VerifiedFor;
        let member = uuid::Uuid::new_v4().simple().to_string();
        let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        snap.vrchat_login.restoring = true;
        snap.vrchat_login.verified_for = Some(VerifiedFor { epoch: 1, member, user: Some("usr_test".into()) });
        let connecting = Scope::of(&snap);
        let account = connecting.account().expect("on its way back as that account");
        let url = |n: u32| format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-00000028{n:04}/1/file");
        let image = image::RgbaImage::from_pixel(64, 64, image::Rgba([9, 200, 90, 255]));
        let picture = |scope: &Scope, n: u32| Picture::Profile { scope: scope.clone(), url: url(n), user: None };
        let of = Profiles::of(Some(&account));
        assert!(!of.keep(&url(1), None, &image), "nothing of an account is kept before VRChat said it is there");
        assert!(store::begin(&store::root(), &account));
        assert!((0..30).all(|n| of.keep(&url(n), None, &image)) && of.keep_sent(&url(40), Size::Wide, &image, b"sent"));
        start_again();
        let ctx = egui::Context::default();
        let textures = PaintTextures::of(&ctx);
        let mut page = super::super::Pictures::new(&ctx);
        for n in 0..30 {
            assert!(page.get_at(&textures, 1.0, &picture(&connecting, n)).is_some(), "{n}");
        }
        assert!(!page.started() && !page.loading(), "no loader was needed");
        // Signed in (another scope), and another page: the same texture.
        snap.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_test".into(), ..Default::default() });
        (snap.vrchat_login.allowed, snap.vrchat_login.revision) = (true, 7);
        let signed_in = Scope::of(&snap);
        let first = page.get_at(&textures, 1.0, &picture(&connecting, 3)).unwrap().id();
        let mut other = super::super::Pictures::new(&ctx);
        assert_eq!(page.get_at(&textures, 9.0, &picture(&signed_in, 3)).map(|t| t.id()), Some(first));
        assert_eq!(other.get_at(&textures, 9.0, &picture(&signed_in, 3)).map(|t| t.id()), Some(first));
        // Another account's: not there.
        snap.vrchat_login.verified_for = Some(VerifiedFor { epoch: 1, member: uuid::Uuid::new_v4().simple().to_string(), user: Some("usr_test".into()) });
        let stranger = Scope::of(&snap);
        assert!(other.get_at(&textures, 10.0, &picture(&stranger, 3)).is_none());
        assert!(matches!(wide(&ctx, &connecting, &url(40)), Wide::Ready(_)) && matches!(wide(&ctx, &stranger, &url(40)), Wide::No) && matches!(wide(&ctx, &connecting, &url(41)), Wide::No));
        assert!(store::forget(&store::root(), Some("usr_test")) || !store::there(&store::root(), &account));
        start_again();
    }
}
