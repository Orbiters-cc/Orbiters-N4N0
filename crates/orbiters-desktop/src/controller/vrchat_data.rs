//! Read-only data requests, and the one change a page can ask for: a new picture for an avatar or world of the
//! signed-in VRChat account ([`Controller::vrchat_picture`]). Only workers call this module; session credentials never
//! cross IPC.
use super::{Controller, NoticeKind, Shared};
use crate::state::Snapshot;
use orbiters_core::{
    account::Server,
    vrchat_api::{Kind, Step, User},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    development: bool,
    member: Option<String>,
    allowed: bool,
    user: Option<String>,
    revision: u64,
}
impl Scope {
    pub fn of(s: &Snapshot) -> Self {
        Self {
            development: s.settings.development,
            member: s.account.state.username().map(str::to_owned),
            allowed: s.vrchat_login.allowed,
            user: s.vrchat_login.user.as_ref().map(|u| u.id.clone()),
            revision: s.vrchat_login.revision,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    Avatars {
        ids: Vec<String>,
    },
    /// Worlds' thumbnails (at most 8), answered as [`Response::Avatars`].
    Worlds {
        ids: Vec<String>,
    },
    Friends,
    Instance {
        world: String,
        instance: String,
    },
    Image {
        url: String,
    },
    /// Which of these avatars and worlds (at most [`OWNED_BATCH`]) the signed-in VRChat account uploaded, answered as
    /// [`Response::Owned`]: none without a personal session.
    Owned {
        ids: Vec<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Avatars { pictures: Vec<(String, super::vrchat_login::Picture)>, errors: Vec<String> },
    Friends(Vec<User>),
    Instance(Option<bool>),
    Image(Vec<u8>),
    Owned(Vec<String>),
}
/// IDs one [`Request::Owned`] may ask about (VRChat's answers are kept a day, so a page asks once).
pub const OWNED_BATCH: usize = 16;
pub type Result = std::result::Result<Response, String>;

impl Controller {
    pub fn vrchat_data(&self, scope: Scope, request: Request) -> Result {
        if scope != Scope::of(&self.view()) {
            return Err("The account changed. Please try again.".into());
        }
        let source = self.vrchat_source();
        let result = (|| -> anyhow::Result<Response> {
            Ok(match request {
                Request::Avatars { ids } => {
                    anyhow::ensure!(ids.len() <= 8, "Too many avatar thumbnails requested.");
                    let mut pictures = Vec::new();
                    let mut missing = Vec::new();
                    let mut errors = Vec::new();
                    for id in ids {
                        if let Some(bytes) = orbiters_projects::avatars::local_thumbnail(&id).and_then(|p| std::fs::read(p).ok()) {
                            if let Some(p) = avatar_picture(&bytes) {
                                pictures.push((id, p));
                                continue;
                            }
                        }
                        missing.push(id);
                    }
                    if source.direct_available() {
                        for id in missing {
                            let result = (|| -> anyhow::Result<_> {
                                let Some(a) = source.avatar(&id)? else { return Ok(None) };
                                let url = if a.thumbnail_image_url.is_empty() { &a.image_url } else { &a.thumbnail_image_url };
                                Ok(avatar_picture(&source.image(url)?))
                            })();
                            match result {
                                Ok(Some(p)) => pictures.push((id, p)),
                                Ok(None) => {}
                                Err(e) => errors.push(format!("{id}: {e}")),
                            }
                        }
                    } else {
                        let server = if scope.development { Server::Development } else { Server::Production };
                        match orbiters_projects::network::thumbnails(server, missing) {
                            Ok(paths) => {
                                for (id, t) in paths {
                                    if let Some(p) = std::fs::read(t.path).ok().and_then(|b| avatar_picture(&b)) {
                                        pictures.push((id, p));
                                    }
                                }
                            }
                            Err(e) => errors.push(e.to_string()),
                        }
                    }
                    Response::Avatars { pictures, errors }
                }
                Request::Worlds { ids } => {
                    anyhow::ensure!(ids.len() <= 8, "Too many world thumbnails requested.");
                    // Kept a day: a world's thumbnail changes with its uploads.
                    let cache = orbiters_projects::data_dir().join("world-thumbnails");
                    let mut pictures = Vec::new();
                    let mut errors = Vec::new();
                    for id in ids {
                        let file = cache.join(format!("{}.image", orbiters_projects::avatars::filename(&id)));
                        let fresh =
                            std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 86400);
                        if fresh {
                            if let Some(p) = std::fs::read(&file).ok().and_then(|b| avatar_picture(&b)) {
                                pictures.push((id, p));
                                continue;
                            }
                        }
                        let result = (|| -> anyhow::Result<_> {
                            let Some(url) = source.world_thumbnail(&id)? else { return Ok(None) };
                            let bytes = source.image(&url)?;
                            let picture = avatar_picture(&bytes);
                            if picture.is_some() {
                                std::fs::create_dir_all(&cache)?;
                                let tmp = file.with_extension("tmp");
                                std::fs::write(&tmp, &bytes)?;
                                std::fs::rename(tmp, &file)?;
                            }
                            Ok(picture)
                        })();
                        match result {
                            Ok(Some(p)) => pictures.push((id, p)),
                            // Not known (a private world without a VRChat sign-in): what was kept, however old.
                            Ok(None) => {
                                if let Some(p) = std::fs::read(&file).ok().and_then(|b| avatar_picture(&b)) {
                                    pictures.push((id, p));
                                }
                            }
                            Err(e) => {
                                if let Some(p) = std::fs::read(&file).ok().and_then(|b| avatar_picture(&b)) {
                                    pictures.push((id, p));
                                } else {
                                    errors.push(format!("{id}: {e}"));
                                }
                            }
                        }
                    }
                    Response::Avatars { pictures, errors }
                }
                Request::Friends => Response::Friends(source.friends()?),
                Request::Instance { world, instance } => Response::Instance(source.instance_alive(&world, &instance)?),
                Request::Image { url } => Response::Image(source.image(&url)?),
                Request::Owned { ids } => {
                    anyhow::ensure!(ids.len() <= OWNED_BATCH, "Too many avatars and worlds asked about.");
                    Response::Owned(source.owned(&ids)?)
                }
            })
        })()
        .map_err(|e| e.to_string());
        if scope != Scope::of(&self.view()) {
            return Err("The account changed. Please try again.".into());
        }
        result
    }
}

/// The largest picture file a page may pick.
const PICTURE_FILE_LIMIT: u64 = 50 * 1024 * 1024;
/// VRChat's picture size for avatars and worlds (its SDK's camera takes them at 1200 × 900).
pub const PICTURE_SIZE: (u32, u32) = (1200, 900);
/// The shortest side a picked picture may have.
const PICTURE_MIN: u32 = 128;

impl Controller {
    /// Changes the picture of `id`, an avatar or a world of the signed-in VRChat account, to the picture file at `path`.
    /// Answers at once: the file is read, checked and made VRChat's size, then uploaded, on a thread of its own; its
    /// step and outcome are in [`Snapshot::vrchat_login`]'s `upload`, the outcome a notice too. One picture at a time.
    pub fn vrchat_picture(self: &Shared, scope: Scope, id: String, path: PathBuf) {
        use super::vrchat_login::PictureUpload;
        // Never repeats, even after an account change empties the snapshot's record: a page takes each outcome once.
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        if scope != Scope::of(&self.view()) {
            return self.notify(NoticeKind::Error, "The account changed. Please try again.");
        }
        if Kind::of(&id).is_none() {
            return self.notify(NoticeKind::Error, "Only avatars' and worlds' pictures can be changed.");
        }
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        let mut refused = None;
        self.update_if(|s| {
            let login = &mut s.vrchat_login;
            if !login.allowed || login.user.is_none() {
                refused = Some("Sign in to VRChat in Settings to change pictures.");
            } else if login.upload.as_ref().is_some_and(PictureUpload::running) {
                refused = Some("A picture is already uploading to VRChat. Try again once it is done.");
            } else {
                login.upload = Some(PictureUpload { serial, id: id.clone(), step: Some(Step::Checking), outcome: None });
            }
            refused.is_none()
        });
        if let Some(message) = refused {
            return self.notify(NoticeKind::Error, message);
        }
        let ctl = self.clone();
        let finish = move |ctl: &Controller, result: std::result::Result<(), String>| {
            let ours = ctl.update_if(|s| match s.vrchat_login.upload.as_mut().filter(|u| u.serial == serial) {
                Some(u) => {
                    u.step = None;
                    u.outcome = Some(result.clone());
                    true
                }
                None => false,
            });
            // An account change meanwhile discarded it with the rest of the session's state.
            if ours {
                match result {
                    Ok(()) => ctl.notify(NoticeKind::Success, "Picture changed on VRChat"),
                    Err(e) => ctl.notify(NoticeKind::Error, format!("The picture could not be changed: {e}")),
                }
            }
        };
        let spawned = std::thread::Builder::new().name("vrchat-picture".into()).spawn({
            let finish = finish.clone();
            move || {
                let result = ctl.replace_picture(&scope, serial, &id, &path).map_err(|e| format!("{e:#}"));
                finish(&ctl, result);
            }
        });
        if spawned.is_err() {
            finish(self, Err("The upload could not start.".into()));
        }
    }
    fn replace_picture(&self, scope: &Scope, serial: u64, id: &str, path: &Path) -> anyhow::Result<()> {
        let png = read_picture(path)?;
        anyhow::ensure!(*scope == Scope::of(&self.view()), "The account changed. Please try again.");
        self.vrchat_source().replace_image(id, &png, |step| {
            self.update_if(|s| match s.vrchat_login.upload.as_mut().filter(|u| u.serial == serial) {
                Some(u) => {
                    u.step = Some(step);
                    true
                }
                None => false,
            });
        })?;
        keep_picture(&orbiters_projects::data_dir(), id, &png);
        Ok(())
    }
}

/// Reads a picked picture file (at most 50 MB) and makes it VRChat's ([`prepare_picture`]).
fn read_picture(path: &Path) -> anyhow::Result<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|_| anyhow::anyhow!("The picture file could not be found."))?;
    anyhow::ensure!(meta.is_file(), "Pick a picture file.");
    anyhow::ensure!(meta.len() > 0, "That file is empty.");
    anyhow::ensure!(meta.len() <= PICTURE_FILE_LIMIT, "That picture is over 50 MB. Pick a smaller one.");
    prepare_picture(&std::fs::read(path).map_err(|_| anyhow::anyhow!("The picture file could not be read."))?)
}

/// A picture made VRChat's ([`fit_picture`]), as a PNG.
pub(crate) fn prepare_picture(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder as _;
    let rgb = fit_picture(bytes)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(&mut png, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive)
        .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
    Ok(png)
}

/// A picture made VRChat's: a PNG, JPEG, WebP or GIF (its first frame) of at least 128 pixels a side, turned the way
/// its camera was held, cropped about its centre to 4:3 and made 1200 × 900, opaque (any transparency over black).
/// Cropped rather than letterboxed: VRChat shows these pictures edge to edge at 4:3 (its menus, the website, the SDK's
/// own camera pictures), where bars would show as part of the picture.
pub(crate) fn fit_picture(bytes: &[u8]) -> anyhow::Result<image::RgbImage> {
    use image::ImageDecoder as _;
    let not_a_picture = || anyhow::anyhow!("That file is not a picture VRChat can show (PNG, JPEG, WebP or GIF).");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    if reader.format().is_none() {
        return Err(not_a_picture());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| not_a_picture())?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).map_err(|_| not_a_picture())?;
    picture.apply_orientation(orientation);
    let (w, h) = (picture.width(), picture.height());
    anyhow::ensure!(w >= PICTURE_MIN && h >= PICTURE_MIN, "That picture is {w} × {h} pixels: pick one of at least {PICTURE_MIN} on each side.");
    let (tw, th) = PICTURE_SIZE;
    let fitted = picture.resize_to_fill(tw, th, image::imageops::FilterType::Lanczos3).to_rgba8();
    Ok(image::RgbImage::from_fn(tw, th, |x, y| {
        let p = fitted.get_pixel(x, y).0;
        let a = p[3] as u32;
        image::Rgb([0, 1, 2].map(|i| ((p[i] as u32 * a + 127) / 255) as u8))
    }))
}

/// After a picture changed on VRChat, the copies this computer keeps (under `data`) show the new one at once: an
/// avatar's Toolkit picture and Orbiters' cached thumbnails of it, each only where one was kept, and a world's cached
/// thumbnail.
fn keep_picture(data: &Path, id: &str, png: &[u8]) {
    let name = orbiters_projects::avatars::filename(id);
    let files: Vec<PathBuf> = match Kind::of(id) {
        Some(Kind::Avatar) => std::iter::once(data.join("Avatars").join(format!("{name}.png")))
            .chain(["production", "development"].map(|server| data.join("avatar-thumbnails").join(server).join(format!("{name}.image"))))
            .filter(|f| f.is_file())
            .collect(),
        Some(Kind::World) => vec![data.join("world-thumbnails").join(format!("{name}.image"))],
        None => vec![],
    };
    for file in files {
        let tmp = file.with_extension("tmp");
        let written =
            file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&tmp, png)).and_then(|()| std::fs::rename(&tmp, &file));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn avatar_picture(bytes: &[u8]) -> Option<super::vrchat_login::Picture> {
    let rgb = orbiters_orb::picture::rgb_any(bytes).ok()?;
    let image = orbiters_orb::picture::letterbox(&rgb, 480, 360, [24, 24, 27]);
    let rgba = image.pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    Some(super::vrchat_login::Picture { width: 480, height: 360, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projects_avatar_pictures_keep_the_source_aspect_ratio() {
        let input = image::RgbImage::from_pixel(4, 2, image::Rgb([200, 80, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(input).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let p = avatar_picture(bytes.get_ref()).unwrap();
        assert_eq!((p.width, p.height), (480, 360));
        assert_eq!(&p.rgba[..4], &[24, 24, 27, 255]);
        let middle = (180 * 480 + 240) * 4;
        assert_eq!(&p.rgba[middle..middle + 4], &[200, 80, 40, 255]);
        assert!(avatar_picture(b"broken image").is_none());
    }
    #[test]
    fn data_scope_tracks_account_environment_and_session_revocation() {
        let mut s = Snapshot::new(orbiters_core::settings::Settings::default());
        let base = Scope::of(&s);
        s.settings.development = !s.settings.development;
        assert_ne!(base, Scope::of(&s));
        s.settings.development = !s.settings.development;
        s.vrchat_login.allowed = true;
        s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() });
        let signed_in = Scope::of(&s);
        s.vrchat_login.user = None;
        assert_ne!(signed_in, Scope::of(&s));
        let c = Controller::new(s.settings);
        assert!(c.vrchat_data(signed_in, Request::Friends).unwrap_err().contains("account changed"));
    }
    #[test]
    fn unavailable_instance_keeps_unknown_without_using_a_session() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        assert!(matches!(
            c.vrchat_data(Scope::of(&c.view()), Request::Instance { world: "wrld_test".into(), instance: "123".into() }),
            Ok(Response::Instance(None))
        ));
    }
    #[test]
    fn ownership_is_none_without_a_personal_session_and_asked_in_batches() {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        let scope = Scope::of(&c.view());
        assert!(
            matches!(c.vrchat_data(scope.clone(), Request::Owned { ids: vec!["avtr_a".into(), "wrld_b".into()] }), Ok(Response::Owned(ids)) if ids.is_empty())
        );
        let many = (0..=OWNED_BATCH).map(|i| format!("avtr_{i}")).collect();
        assert!(c.vrchat_data(scope, Request::Owned { ids: many }).unwrap_err().contains("Too many"));
    }
    fn encoded(picture: image::RgbaImage) -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(picture).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
    fn near(a: image::Rgb<u8>, b: [u8; 3]) -> bool {
        a.0.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 6)
    }
    #[test]
    fn pictures_are_cropped_about_their_centre_to_vrchats_size() {
        use image::GenericImageView as _;
        let (red, green, blue) = (image::Rgba([220, 30, 30, 255]), image::Rgba([30, 200, 60, 255]), image::Rgba([30, 60, 220, 255]));
        // 1920 × 900 is 1200 × 900 with 360 pixels more on each side: bands of 300 at the edges fall off.
        let wide = image::RgbaImage::from_fn(1920, 900, |x, _| {
            if x < 300 {
                red
            } else if x >= 1620 {
                blue
            } else {
                green
            }
        });
        // 600 × 1200 scales to 1200 × 2400, of which the middle 900 stay: bands of 200 (400 scaled) fall off.
        let tall = image::RgbaImage::from_fn(600, 1200, |_, y| {
            if y < 200 {
                red
            } else if y >= 1000 {
                blue
            } else {
                green
            }
        });
        for source in [wide, tall] {
            let png = prepare_picture(&encoded(source)).unwrap();
            assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png);
            let out = image::load_from_memory(&png).unwrap();
            assert_eq!(out.dimensions(), PICTURE_SIZE);
            let out = out.to_rgb8();
            for (x, y) in [(0, 0), (1199, 0), (0, 899), (1199, 899), (600, 450)] {
                assert!(near(*out.get_pixel(x, y), [30, 200, 60]), "{:?} at {x}, {y}", out.get_pixel(x, y));
            }
        }
        // Opaque: transparency over black.
        let clear = prepare_picture(&encoded(image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 0])))).unwrap();
        assert_eq!(image::load_from_memory(&clear).unwrap().to_rgb8().get_pixel(10, 10).0, [0, 0, 0]);
        // Too small, or not a picture at all.
        assert!(prepare_picture(&encoded(image::RgbaImage::new(100, 400))).unwrap_err().to_string().contains("100 × 400"));
        assert!(prepare_picture(b"definitely not a picture").unwrap_err().to_string().contains("not a picture"));
    }
    #[test]
    fn picked_files_are_checked_before_anything_is_decoded() {
        let dir = std::env::temp_dir().join(format!("orbiters-picture-file-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_picture(&dir.join("missing.png")).unwrap_err().to_string().contains("could not be found"));
        assert!(read_picture(&dir).unwrap_err().to_string().contains("Pick a picture file"));
        std::fs::write(dir.join("empty.png"), b"").unwrap();
        assert!(read_picture(&dir.join("empty.png")).unwrap_err().to_string().contains("empty"));
        std::fs::write(dir.join("ok.png"), encoded(image::RgbaImage::from_pixel(320, 240, image::Rgba([1, 2, 3, 255])))).unwrap();
        assert!(read_picture(&dir.join("ok.png")).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn a_changed_picture_replaces_only_the_copies_kept_of_it() {
        let data = std::env::temp_dir().join(format!("orbiters-picture-test-{}", uuid::Uuid::new_v4()));
        let toolkit = data.join("Avatars").join("avtr_a.png");
        std::fs::create_dir_all(toolkit.parent().unwrap()).unwrap();
        std::fs::write(&toolkit, b"old").unwrap();
        keep_picture(&data, "avtr_a", b"new");
        assert_eq!(std::fs::read(&toolkit).unwrap(), b"new");
        // No copy of the thumbnails was kept: none is made.
        assert!(!data.join("avatar-thumbnails").exists());
        keep_picture(&data, "avtr_other", b"new");
        assert!(!data.join("Avatars").join("avtr_other.png").exists());
        keep_picture(&data, "wrld_b", b"world");
        assert_eq!(std::fs::read(data.join("world-thumbnails").join("wrld_b.image")).unwrap(), b"world");
        assert_eq!(std::fs::read_dir(data.join("Avatars")).unwrap().count(), 1);
        std::fs::remove_dir_all(&data).unwrap();
    }
    #[test]
    fn pictures_need_a_vrchat_session_and_go_one_at_a_time() {
        use crate::state::AccountState;
        use orbiters_core::account::Server;
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.vrchat_picture(Scope::of(&c.view()), "avtr_a".into(), "picture.png".into());
        assert!(c.view().vrchat_login.upload.is_none());
        assert!(c.take_notices().iter().any(|n| format!("{n:?}").contains("Sign in to VRChat")));
        c.update(|s| {
            s.account.state = AccountState::SignedIn { username: "tester".into() };
            s.account.development = Server::current() == Server::Development;
            s.vrchat_login.allowed = true;
            s.vrchat_login.user = Some(User { id: "usr_test".into(), ..Default::default() });
            s.vrchat_login.upload =
                Some(super::super::vrchat_login::PictureUpload { serial: 0, id: "avtr_b".into(), step: Some(Step::Uploading), outcome: None });
        });
        c.vrchat_picture(Scope::of(&c.view()), "avtr_a".into(), "picture.png".into());
        assert_eq!(c.view().vrchat_login.upload.as_ref().unwrap().id, "avtr_b");
        assert!(c.take_notices().iter().any(|n| format!("{n:?}").contains("already uploading")));
        c.vrchat_picture(Scope::of(&c.view()), "usr_test".into(), "picture.png".into());
        assert!(c.take_notices().iter().any(|n| format!("{n:?}").contains("Only avatars")));
    }
}
