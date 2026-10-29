//! Read-only data requests. Only workers call this module; session credentials never cross IPC.
use super::Controller;
use crate::state::Snapshot;
use orbiters_core::{account::Server, vrchat_api::User};
use serde::{Deserialize, Serialize};

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
    Avatars { ids: Vec<String> },
    Friends,
    Instance { world: String, instance: String },
    Image { url: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Avatars { pictures: Vec<(String, super::vrchat_login::Picture)>, errors: Vec<String> },
    Friends(Vec<User>),
    Instance(Option<bool>),
    Image(Vec<u8>),
}
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
                Request::Friends => Response::Friends(source.friends()?),
                Request::Instance { world, instance } => Response::Instance(source.instance_alive(&world, &instance)?),
                Request::Image { url } => Response::Image(source.image(&url)?),
            })
        })()
        .map_err(|e| e.to_string());
        if scope != Scope::of(&self.view()) {
            return Err("The account changed. Please try again.".into());
        }
        result
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
}
