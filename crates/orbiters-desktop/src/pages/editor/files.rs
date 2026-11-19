//! What the editor reads and writes: the picture (and a VRChat multi-layer picture's layers), the edited copy beside
//! it, and the edit saved next to the copy so that opening the copy again edits it again, from the original.
//!
//! The copy is `<name> (edited).png` in the original's folder (`(edited 2)` and so on when taken), with the original's
//! XMP packet (VRChat's world, its name and when it was taken), so Photos & captures lists it with the original's
//! world and instance. The edit is `<copy's name>.orbiters-edit.json` beside it. The original is never written.
//!
//! A picture of Orbiters (shared with the member, or of a community's or an event's gallery, [`Remote`]) is
//! downloaded at its full size into the app's cache first ([`fetch`]); its copy goes into VRChat's pictures folder,
//! in the month it was taken, with its world and time as VRChat writes them, so Photos & captures lists it there.

use orbiters_ui::photo::{Edit, Rgba8, SourceStack};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The picture edited: a file, or a VRChat multi-layer picture (its own file, VRChat's composite, and its layers), or a
/// picture of Orbiters downloaded to its file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Source {
    pub picture: PathBuf,
    /// VRChat's layers, back to front ("Environment", "Player", "UI"); empty for a plain picture.
    #[serde(default)]
    pub layers: Vec<SourceLayer>,
    /// A picture of Orbiters: `picture` is where it is downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<Remote>,
}

/// Where a picture of Orbiters comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteKind {
    /// A picture shared through Photos & captures (`/captures/picture/:id/file`).
    #[default]
    Shared,
    /// A picture of a community's or an event's gallery, or of an asset's showcase (its full picture's link renewed by
    /// `/galleries/images/:id/source`, `/assets/:asset/showcase-images/:id/source`).
    Gallery,
    /// A public picture by its link alone (an asset's on Orbiters).
    Link,
}

/// A picture of Orbiters to edit, and what its copy says: its world and when it was taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Remote {
    pub kind: RemoteKind,
    pub id: u64,
    /// The asset whose showcase a [`RemoteKind::Gallery`] picture is in (0: a community gallery's).
    #[serde(skip_serializing_if = "is_zero")]
    pub asset: u64,
    /// On the development environment's server.
    pub development: bool,
    pub world_id: Option<String>,
    pub world_name: Option<String>,
    /// Unix seconds (0: unknown, now when saved).
    pub taken: i64,
    /// What its copy is called, before " (edited).png".
    pub name: String,
    /// Its link: a [`RemoteKind::Link`]'s picture, a gallery picture's link as listed (tried when Orbiters renews none).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Remote {
    /// Where it is downloaded (the app's cache).
    pub fn file(&self) -> PathBuf {
        let server = if self.development { "dev" } else { "prod" };
        let name = match self.kind {
            RemoteKind::Shared => format!("shared-{server}-{}", self.id),
            // An asset's showcase pictures are numbered apart from the community galleries' (another route on
            // Orbiters): each kind has a name of its own, and neither is the old unqualified one, under which an
            // earlier version may have kept the other kind's picture of the same number (it is never read again).
            RemoteKind::Gallery if self.asset != 0 => format!("gallery-{server}-asset-{}-{}", self.asset, self.id),
            RemoteKind::Gallery => format!("gallery-{server}-community-{}", self.id),
            RemoteKind::Link => {
                // Its link, hashed (a link is the picture's only name).
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                self.url.hash(&mut h);
                format!("link-{:016x}", h.finish())
            }
        };
        orbiters_core::paths::cache_dir().join("editor-pictures").join(format!("{name}.img"))
    }

    fn taken(&self) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(self.taken).ok().filter(|_| self.taken > 0).unwrap_or_else(time::OffsetDateTime::now_utc)
    }

    /// The XMP chunk its copy carries: its world and when it was taken, as VRChat writes them.
    pub fn xmp(&self) -> Vec<u8> {
        xmp_for(self.world_id.as_deref().filter(|id| id.starts_with("wrld_")), self.world_name.as_deref().filter(|n| !n.is_empty()), self.taken())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLayer {
    pub name: String,
    pub path: PathBuf,
}

impl Source {
    pub fn picture(path: impl Into<PathBuf>) -> Self {
        Self { picture: path.into(), layers: Vec::new(), remote: None }
    }

    /// A picture of Orbiters, downloaded to edit ([`fetch`]).
    pub fn remote(remote: Remote) -> Self {
        Self { picture: remote.file(), layers: Vec::new(), remote: Some(remote) }
    }

    /// Where a new copy of it goes: beside the original, or for a picture of Orbiters in VRChat's pictures folder in
    /// the month it was taken.
    pub fn copy_path(&self) -> PathBuf {
        let Some(r) = &self.remote else { return copy_path(&self.picture) };
        let taken = r.taken();
        let dir = pictures_folder().join(format!("{:04}-{:02}", taken.year(), taken.month() as u8));
        let name: String = r.name.chars().map(|c| if c.is_alphanumeric() || " -_.,'()&".contains(c) { c } else { ' ' }).collect();
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let name: String = if name.is_empty() { format!("Orbiters picture {}", r.id) } else { name.chars().take(80).collect() }; // i18n: not shown
        copy_path(&dir.join(format!("{name}.png")))
    }

    /// What its copy carries: a picture of Orbiters' world and time, else the original's XMP chunk.
    pub fn xmp(&self) -> Option<Vec<u8>> {
        match &self.remote {
            Some(r) => Some(r.xmp()),
            None => xmp_chunk(&self.picture).or_else(|| self.files().first().and_then(|f| xmp_chunk(f))),
        }
    }

    /// A multi-layer picture of `picture` (VRChat's composite, or its bottom layer when that is gone) and its `layers`
    /// back to front ([`crate::controller::shares::Entry::layers`]).
    pub fn with_layers(picture: impl Into<PathBuf>, layers: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        Self { picture: picture.into(), layers: layers.into_iter().map(|(name, path)| SourceLayer { name, path }).collect(), remote: None }
    }

    /// The files stacked, back to front, each with its own effects: VRChat's layers when it has its world among them
    /// (the people and the interface over it); without the world, VRChat's composite at the bottom and the other
    /// layers over it (their effects then cover the composite's own people). Empty for a plain picture.
    pub fn stacks(&self) -> Vec<SourceLayer> {
        if self.layers.is_empty() {
            return Vec::new();
        }
        let world = self.layers.iter().any(|l| l.name.eq_ignore_ascii_case("Environment"));
        let own = self.layers.iter().any(|l| l.path == self.picture);
        let mut out = Vec::new();
        if !world && !own {
            out.push(SourceLayer { name: "Picture".into(), path: self.picture.clone() }); // i18n: not shown
        }
        out.extend(self.layers.iter().cloned());
        out
    }

    /// The files read, in [`Source::stacks`] order (the picture alone for a plain picture).
    pub fn files(&self) -> Vec<PathBuf> {
        let stacks = self.stacks();
        if stacks.is_empty() {
            vec![self.picture.clone()]
        } else {
            stacks.into_iter().map(|s| s.path).collect()
        }
    }

    /// A new edit of it: a stack for each of its layers.
    pub fn new_edit(&self) -> Edit {
        Edit { sources: self.stacks().into_iter().map(|s| SourceStack { name: s.name, ..Default::default() }).collect(), ..Default::default() }
    }
}

/// What is saved next to an edited copy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub format: String,
    pub version: u32,
    pub source: Source,
    pub edit: Edit,
    /// For a picture edited in a 3D capture's view (Edit in 3D, [`super::super::splats`]): where the camera was, so
    /// that editing it again in 3D returns there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<View3d>,
}

pub const FORMAT: &str = "orbiters-photo-edit";

impl Saved {
    pub fn new(source: Source, edit: Edit) -> Self {
        Self { format: FORMAT.into(), version: 1, source, edit, view: None }
    }
}

/// The view of a 3D capture a picture was made of: the capture, the orbiting camera (its pivot, turn, tilt and
/// distance, its vertical field of view; its eye and where it looks, for other readers), the picture's size and how
/// the view showed what was poorly seen or far away.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct View3d {
    pub capture: PathBuf,
    pub pivot: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub size: [u32; 2],
    /// "shown", "faded", "blurred" or "hazed", and how strongly.
    pub poorly_seen: String,
    pub amount: f32,
    /// "shown", "fog" or "crop", and from how far (metres).
    pub far: String,
    pub far_distance: f32,
}

/// Where the edit of the copy `png` is kept.
pub fn sidecar(png: &Path) -> PathBuf {
    let stem = png.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    png.with_file_name(format!("{stem}.orbiters-edit.json"))
}

/// The edit of `png` when it is an edited copy whose original is still there: its source and edit.
pub fn reopen(png: &Path) -> Option<Saved> {
    let text = std::fs::read_to_string(sidecar(png)).ok()?;
    let saved: Saved = serde_json::from_str(&text).ok()?;
    (saved.format == FORMAT && saved.source.files().iter().all(|f| f.is_file())).then_some(saved)
}

/// A new copy's name beside `original`: `<name> (edited).png`, `(edited 2)` and on when taken.
pub fn copy_path(original: &Path) -> PathBuf {
    let stem = original.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Picture".into()); // i18n: not shown
    let dir = original.parent().map(Path::to_path_buf).unwrap_or_default();
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem} (edited).png") } else { format!("{stem} (edited {n}).png") }))
        .find(|p| !p.exists() && !sidecar(p).exists())
        .expect("a free name")
}

/// Downloads a picture of Orbiters at its full size to [`Source::picture`] when it is not there yet (nothing to do for
/// a picture on this computer): a shared picture's file, or a gallery picture's full picture by a link Orbiters renews
/// (Discord's expire), asked again as failed when the first does not load.
pub fn fetch(source: &Source) -> anyhow::Result<()> {
    let Some(r) = &source.remote else { return Ok(()) };
    if source.picture.is_file() {
        return Ok(());
    }
    use orbiters_core::account::{Api, Server};
    let server = if r.development { Server::Development } else { Server::Production };
    // A public link needs no account.
    let token = match orbiters_core::account::read(server) {
        Some(a) => a.token,
        None if r.kind == RemoteKind::Link => String::new(),
        None => anyhow::bail!("{}", orbiters_i18n::tr("Sign in to Orbiters in Settings to edit its pictures")),
    };
    let api = Api::new(server);
    if let Some(dir) = source.picture.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = source.picture.with_extension("download");
    let result = match r.kind {
        RemoteKind::Shared => api.capture_file(&token, orbiters_core::shares::Kind::Picture, r.id, &temp),
        RemoteKind::Gallery => {
            let get = |failed: bool| -> anyhow::Result<Vec<u8>> {
                let links = api.picture_source(&token, r.asset, r.id, failed)?;
                let url = links.full_url.or(links.src).filter(|u| u.starts_with("https://")).ok_or_else(|| anyhow::anyhow!("{}", orbiters_i18n::tr("Orbiters has no link to this picture")))?;
                get_link(&api, &url)
            };
            get(false).or_else(|_| get(true)).or_else(|e| if r.url.starts_with("https://") { get_link(&api, &r.url) } else { Err(e) }).and_then(|bytes| Ok(std::fs::write(&temp, bytes)?))
        }
        RemoteKind::Link => get_link(&api, &r.url).and_then(|bytes| Ok(std::fs::write(&temp, bytes)?)),
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    std::fs::rename(&temp, &source.picture)?;
    Ok(())
}

/// A picture's bytes from an `https` link (up to 96 MB).
fn get_link(api: &orbiters_core::account::Api, url: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(url.starts_with("https://"), "{}", orbiters_i18n::tr("not an https link"));
    let response = api.agent().get(url).config().https_only(true).build().call()?;
    anyhow::ensure!(response.status().is_success(), "{}", orbiters_i18n::tr!("the picture's link answered {status}", status = response.status()));
    Ok(response.into_body().with_config().limit(96 << 20).read_to_vec()?)
}

/// A picture of Orbiters already fetched elsewhere (`from`: Photos & captures fetches what is dragged or copied) put
/// where the editor downloads it, so it is not downloaded again.
pub fn seed(source: &Source, from: &Path) {
    if source.remote.is_some() && !source.picture.is_file() && from.is_file() {
        if let Some(dir) = source.picture.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temp = source.picture.with_extension("seeding");
        if std::fs::copy(from, &temp).is_ok() {
            let _ = std::fs::rename(&temp, &source.picture);
        }
    }
}

/// A picture file as RGBA8 (PNG, JPEG, WebP).
pub fn load(path: &Path) -> anyhow::Result<Rgba8> {
    let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok(Rgba8::new(w, h, img.into_raw()))
}

/// The data of a PNG's XMP chunk (an `iTXt` with the keyword `XML:com.adobe.xmp`), as it is in the file.
pub fn xmp_chunk(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    let mut at = 8usize;
    if bytes.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    while at + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind = &bytes[at + 4..at + 8];
        let data = bytes.get(at + 8..at + 8 + len)?;
        if kind == b"iTXt" && data.starts_with(b"XML:com.adobe.xmp\0") {
            return Some(data.to_vec());
        }
        if kind == b"IEND" {
            break;
        }
        at += 12 + len;
    }
    None
}

/// An XMP chunk as VRChat writes one: the world, its name and when the picture was taken (for a picture of a 3D
/// capture's view, which Photos & captures then lists with that world and instance).
pub fn xmp_for(world_id: Option<&str>, world_name: Option<&str>, taken: time::OffsetDateTime) -> Vec<u8> {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let date = taken.format(&time::format_description::well_known::Rfc3339).unwrap_or_default();
    let mut fields = format!("<xmp:CreatorTool>Orbiters - N4N0</xmp:CreatorTool><xmp:CreateDate>{date}</xmp:CreateDate>");
    if let Some(id) = world_id {
        fields += &format!("<vrc:WorldID>{}</vrc:WorldID>", escape(id));
    }
    if let Some(name) = world_name {
        fields += &format!("<vrc:WorldDisplayName>{}</vrc:WorldDisplayName>", escape(name));
    }
    let packet = format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" \
         xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmlns:vrc=\"http://ns.vrchat.com/vrc/1.0/\">{fields}</rdf:Description></rdf:RDF></x:xmpmeta>"
    );
    // Keyword, no compression, no language, no translated keyword, the text.
    let mut data = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    data.extend_from_slice(packet.as_bytes());
    data
}

/// Writes `img` as a PNG at `path` with `xmp` ([`xmp_chunk`], [`xmp_for`]), through a temporary file beside it.
pub fn write_png(path: &Path, img: &Rgba8, xmp: Option<&[u8]>) -> anyhow::Result<()> {
    let opaque = img.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    let mut bytes = Vec::with_capacity(img.pixels.len() / 2);
    {
        let mut enc = png::Encoder::new(&mut bytes, img.width, img.height);
        enc.set_color(if opaque { png::ColorType::Rgb } else { png::ColorType::Rgba });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header()?;
        if let Some(x) = xmp {
            writer.write_chunk(png::chunk::ChunkType(*b"iTXt"), x)?;
        }
        if opaque {
            let rgb: Vec<u8> = img.pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            writer.write_image_data(&rgb)?;
        } else {
            writer.write_image_data(&img.pixels)?;
        }
        writer.finish()?;
    }
    let tmp = path.with_extension("png.saving");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Where VRChat saves its pictures (its `picture_output_folder`, else `Pictures/VRChat`, made when missing): what
/// Photos & captures lists.
fn pictures_folder() -> PathBuf {
    // Another folder for runs that must not write into VRChat's (screenshots).
    if let Some(d) = std::env::var_os("ORBITERS_EDITOR_PICTURES").map(PathBuf::from) {
        return d;
    }
    if let Some(d) = orbiters_core::visited::screenshot_dirs().into_iter().next() {
        return d;
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_else(orbiters_core::paths::documents_root);
    home.join("Pictures").join("VRChat")
}

/// Saves `img`, a view of the 3D capture `session`, as a picture where VRChat saves its own (the month's folder of
/// when the capture was made), with the capture's world and time in it as VRChat writes them: Photos & captures lists
/// it with the capture's world and instance. Returns where.
pub fn save_view(session: &Path, img: &Rgba8) -> anyhow::Result<PathBuf> {
    use time::format_description::well_known::Rfc3339;
    let manifest = orbiters_core::bundle::read_manifest(session).ok();
    let world = manifest.as_ref().and_then(|m| m.world.clone()).unwrap_or_default();
    let now = time::OffsetDateTime::now_utc();
    let taken = manifest.as_ref().and_then(|m| time::OffsetDateTime::parse(&m.created_utc, &Rfc3339).ok()).unwrap_or(now);
    let local = time::UtcOffset::current_local_offset().map(|o| now.to_offset(o)).unwrap_or(now);
    let dir = pictures_folder().join(format!("{:04}-{:02}", taken.year(), taken.month() as u8));
    std::fs::create_dir_all(&dir)?;
    let stamp = format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", local.year(), local.month() as u8, local.day(), local.hour(), local.minute(), local.second());
    let path = (1..)
        .map(|n| dir.join(if n == 1 { format!("Orbiters 3D view {stamp}.png") } else { format!("Orbiters 3D view {stamp} {n}.png") }))
        .find(|p| !p.exists())
        .expect("a free name");
    let xmp = xmp_for(world.id.as_deref().filter(|id| id.starts_with("wrld_")), world.name.as_deref(), taken);
    write_png(&path, img, Some(&xmp))?;
    Ok(path)
}

/// Saves the edit of the copy `png` beside it.
pub fn write_sidecar(png: &Path, saved: &Saved) -> anyhow::Result<()> {
    orbiters_core::write_atomic(&sidecar(png), &serde_json::to_vec_pretty(saved)?)?;
    Ok(())
}

/// Shows `path` selected in its folder (Explorer, Finder), or opens its folder.
pub fn reveal(path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn()?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg("-R").arg(path).spawn()?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        crate::controller::open_path(path.parent().unwrap_or(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_ui::photo::{Effect, Kind, Light, Mask, TextStyle};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-editor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_edit_round_trips_through_its_json() {
        let source = Source::with_layers("a/VRChat_x_1920x1080.png", [("Environment".into(), "a/e.png".into()), ("Player".into(), "a/p.png".into())]);
        let mut edit = source.new_edit();
        assert_eq!(edit.sources.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Environment", "Player"]);
        for k in Kind::ALL {
            let mut l = edit.layer(k.effect());
            l.opacity = 0.7;
            if k == Kind::Blur {
                l.mask = Mask::radial();
            }
            if let Effect::Text(t) = &mut l.effect {
                t.value = "Hi \"there\"\nsecond line".into();
                t.style = TextStyle::Glow;
            }
            edit.layers.push(l);
        }
        let l = edit.layer(Effect::Light(Light { exposure: -1.5, ..Default::default() }));
        edit.sources[1].layers.push(l);
        edit.geometry.turns = 3;
        edit.geometry.straighten = -4.5;
        edit.geometry.crop = orbiters_ui::photo::Crop { x: 0.1, y: 0.2, w: 0.5, h: 0.4 };
        let saved = Saved::new(source, edit);
        let json = serde_json::to_string_pretty(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);
        // An older or newer edit with fields missing still opens.
        let partial: Saved = serde_json::from_str(r#"{"format":"orbiters-photo-edit","version":1,"source":{"picture":"x.png"},"edit":{"layers":[{"effect":{"type":"vignette","amount":-0.5}}]}}"#).unwrap();
        let Effect::Vignette(v) = &partial.edit.layers[0].effect else { panic!() };
        assert_eq!((v.amount, v.size, v.center), (-0.5, 1.0, [0.5, 0.5]));
        assert!(partial.edit.layers[0].visible);
    }

    /// A depth blur (Edit in 3D) keeps its focus, distance and lens through its JSON; an edit saved before it existed
    /// opens with none, a blur saved then is the blur it was, and a flat picture is left as it is by one.
    #[test]
    fn a_depth_blur_round_trips_and_older_edits_have_none() {
        use orbiters_ui::photo::{Blur, BlurKind, Focus};
        let mut edit = Edit::default();
        let blur = Blur { focus: Focus::Manual, point: [0.3, 0.6], distance: 7.5, aperture: 0.65, near: 0.25, far: 0.9, ..Blur::depth() };
        let l = edit.layer(Effect::Blur(blur.clone()));
        edit.layers.push(l);
        let saved = Saved::new(Source::picture("a/Orbiters 3D view.png"), edit);
        let json = serde_json::to_string(&saved).unwrap();
        assert!(json.contains(r#""kind":"depth""#) && json.contains(r#""focus":"manual""#), "{json}");
        let back: Saved = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);
        let (b, opacity) = back.edit.depth_blur().expect("a depth blur");
        assert_eq!((b, opacity), (&blur, 1.0));
        // Saved before depth blurs: a lens blur, with none of their fields.
        let old: Saved = serde_json::from_str(r#"{"format":"orbiters-photo-edit","version":1,"source":{"picture":"x.png"},"edit":{"layers":[{"effect":{"type":"blur","kind":"lens","radius":0.02,"bokeh":0.7}}]}}"#).unwrap();
        let Effect::Blur(b) = &old.edit.layers[0].effect else { panic!() };
        assert_eq!((b.kind, b.radius, b.bokeh), (BlurKind::Lens, 0.02, 0.7));
        assert!(old.edit.depth_blur().is_none(), "no depth of field in an old edit");
        assert!(!old.edit.layers[0].effect.on_splats());
        // A depth blur with only its kind: auto focus on the middle, the usual lens.
        let bare: Saved = serde_json::from_str(r#"{"format":"orbiters-photo-edit","version":1,"source":{"picture":"x.png"},"edit":{"layers":[{"effect":{"type":"blur","kind":"depth"}}]}}"#).unwrap();
        let (b, _) = bare.edit.depth_blur().expect("on");
        assert_eq!((b.focus, b.point, b.aperture, b.near, b.far), (Focus::Auto, [0.5, 0.5], 0.4, 1.0, 1.0));
        // Hidden, or no aperture: off; the topmost shown one counts, its opacity with it.
        let mut two = Edit::default();
        let mut low = two.layer(Effect::Blur(Blur { distance: 2.0, focus: Focus::Manual, ..Blur::depth() }));
        low.opacity = 0.5;
        let mut high = two.layer(Effect::Blur(Blur { distance: 9.0, focus: Focus::Manual, ..Blur::depth() }));
        high.visible = false;
        two.layers.extend([low, high]);
        assert_eq!(two.depth_blur().map(|(b, o)| (b.distance, o)), Some((2.0, 0.5)));
        two.layers[0].effect = Effect::Blur(Blur { aperture: 0.0, ..Blur::depth() });
        assert!(two.depth_blur().is_none());
        // On a picture's pixels it changes nothing.
        let mut img = orbiters_ui::photo::Image::from_rgba8(&Rgba8::new(4, 2, (0..32).map(|i| (i * 7) as u8).collect()));
        let before = img.clone();
        let layer = Edit::default().layer(Effect::Blur(Blur::depth()));
        orbiters_ui::photo::render::apply(&mut img, &layer);
        assert_eq!(img.data, before.data);
    }

    #[test]
    fn multi_layer_pictures_stack_the_world_first_or_the_composite() {
        let with_world = Source::with_layers("p.png", [("Environment".into(), "e.png".into()), ("Player".into(), "pl.png".into()), ("UI".into(), "u.png".into())]);
        assert_eq!(with_world.files(), [PathBuf::from("e.png"), "pl.png".into(), "u.png".into()]);
        let without = Source::with_layers("p.png", [("Player".into(), "pl.png".into())]);
        assert_eq!(without.stacks().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Picture", "Player"]);
        // VRChat's composite deleted: its file is the bottom layer already.
        let gone = Source::with_layers("pl.png", [("Player".into(), "pl.png".into()), ("UI".into(), "u.png".into())]);
        assert_eq!(gone.files(), [PathBuf::from("pl.png"), "u.png".into()]);
        assert_eq!(Source::picture("x.png").files(), [PathBuf::from("x.png")]);
        assert!(Source::picture("x.png").new_edit().sources.is_empty());
    }

    #[test]
    fn copies_are_named_beside_the_original_and_reopen_their_edit() {
        let dir = temp("copies");
        let original = dir.join("VRChat_2026-10-29_21-04-11.532_3840x2160.png");
        let first = copy_path(&original);
        assert_eq!(first.file_name().unwrap(), "VRChat_2026-10-29_21-04-11.532_3840x2160 (edited).png");
        // The gallery's VRChat names: a copy is a picture of its own, not one of the original's layers.
        assert!(crate::worlds::shot_name(&first.file_name().unwrap().to_string_lossy()).is_none());
        let px = Rgba8::new(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 255]);
        let taken = time::OffsetDateTime::from_unix_timestamp(1_792_348_661).unwrap();
        write_png(&original, &px, Some(&xmp_for(Some("wrld_1"), Some("Tom & Jerry"), taken))).unwrap();
        let saved = Saved::new(Source::picture(&original), Edit::default());
        write_png(&first, &px, xmp_chunk(&original).as_deref()).unwrap();
        write_sidecar(&first, &saved).unwrap();
        assert_eq!(copy_path(&original).file_name().unwrap(), "VRChat_2026-10-29_21-04-11.532_3840x2160 (edited 2).png");
        assert_eq!(reopen(&first), Some(saved));
        assert_eq!(reopen(&original), None);
        // The copy says what the original said, for the gallery's index.
        let info = orbiters_core::visited::read_shot(&first).expect("a picture");
        assert_eq!((info.world_id.as_deref(), info.world_name.as_deref(), info.taken), (Some("wrld_1"), Some("Tom & Jerry"), 1_792_348_661));
        assert_eq!(load(&first).unwrap(), px);
        // Its original gone, a copy is no longer reopened as an edit.
        std::fs::remove_file(&original).unwrap();
        assert_eq!(reopen(&first), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn edits_saved_by_the_first_version_open_with_their_crop_and_shape() {
        use orbiters_ui::photo::Aspect;
        // As the first version wrote it: no perspective, no picture of Orbiters, the shapes by their first names.
        let json = r#"{
          "format": "orbiters-photo-edit", "version": 1,
          "source": {"picture": "C:/Pictures/VRChat/VRChat_2026-10-29_21-04-11.532_3840x2160.png", "layers": []},
          "edit": {
            "geometry": {"turns": 1, "flip_h": false, "flip_v": true, "straighten": -4.5,
                         "crop": {"x": 0.1, "y": 0.2, "w": 0.5, "h": 0.4}, "aspect": "portrait"},
            "sources": [],
            "layers": [{"id": 1, "visible": true, "opacity": 1.0, "mask": {"shape": "everywhere"},
                        "effect": {"type": "light", "exposure": 0.5, "brightness": 0.0, "contrast": 0.0,
                                   "highlights": 0.0, "shadows": 0.0, "black_point": 0.0}}],
            "next_id": 2
          }
        }"#;
        let saved: Saved = serde_json::from_str(json).unwrap();
        let g = &saved.edit.geometry;
        assert_eq!((g.turns, g.flip_v, g.straighten, g.aspect), (1, true, -4.5, Aspect::Portrait));
        assert_eq!((g.vertical, g.horizontal), (0.0, 0.0), "no tilt");
        assert_eq!(g.aspect.name(), "4:5");
        assert!(saved.source.remote.is_none());
        // The first version's shapes keep their names and their ratios.
        let wide = 16.0 / 9.0;
        for (name, ratio) in [("free", None), ("original", Some(wide)), ("square", Some(1.0)), ("portrait", Some(0.8)), ("classic", Some(1.5)), ("wide", Some(wide)), ("story", Some(9.0 / 16.0))] {
            let a: Aspect = serde_json::from_value(serde_json::Value::String(name.into())).unwrap();
            assert_eq!(a.ratio(wide), ratio, "{name}");
            assert_eq!(serde_json::to_value(a).unwrap(), name);
        }
        // Written again: as it was, the new fields beside the old (a picture on this computer has no "remote").
        let back = serde_json::to_value(&saved).unwrap();
        assert_eq!(back["edit"]["geometry"]["aspect"], "portrait");
        assert_eq!(back["edit"]["geometry"]["vertical"], 0.0);
        assert!(back["source"].get("remote").is_none());
        assert_eq!(serde_json::from_value::<Saved>(back).unwrap(), saved);
        // The new shapes and the tilts round trip.
        let mut edit = saved.edit.clone();
        edit.geometry.aspect = Aspect::R7x5;
        edit.geometry.vertical = 12.5;
        edit.geometry.horizontal = -3.0;
        let again = Saved::new(saved.source.clone(), edit);
        let json = serde_json::to_string(&again).unwrap();
        assert!(json.contains("\"aspect\":\"7:5\""), "{json}");
        assert_eq!(serde_json::from_str::<Saved>(&json).unwrap(), again);
    }

    #[test]
    fn a_picture_of_orbiters_is_downloaded_once_and_its_copy_goes_among_vrchat_s() {
        let r = Remote {
            kind: RemoteKind::Gallery,
            asset: 0,
            id: 42,
            development: true,
            world_id: Some("wrld_1".into()),
            world_name: Some("Rooftop".into()),
            taken: 1_792_348_661,
            name: "Saturday night: by Alex/<x>".into(),
            url: String::new(),
        };
        let s = Source::remote(r.clone());
        assert!(s.picture.ends_with("editor-pictures/gallery-dev-42.img"), "{}", s.picture.display());
        assert_eq!(s.files(), [s.picture.clone()]);
        // In VRChat's pictures, the month it was taken, named after it (what a file name cannot hold left out).
        let copy = s.copy_path();
        let taken = time::OffsetDateTime::from_unix_timestamp(r.taken).unwrap();
        assert_eq!(copy.file_name().unwrap(), "Saturday night by Alex x (edited).png");
        assert_eq!(copy.parent().unwrap().file_name().unwrap().to_string_lossy(), format!("{:04}-{:02}", taken.year(), taken.month() as u8));
        // Its XMP says its world and when it was taken; the source round trips through the edit's JSON.
        let xmp = s.xmp().unwrap();
        let text = String::from_utf8_lossy(&xmp);
        assert!(text.contains("<vrc:WorldID>wrld_1</vrc:WorldID>") && text.contains("Rooftop") && text.contains("2026-"), "{text}");
        let saved = Saved::new(s.clone(), Edit::default());
        assert_eq!(serde_json::from_str::<Saved>(&serde_json::to_string(&saved).unwrap()).unwrap(), saved);
        // Already downloaded: nothing is asked of Orbiters.
        let dir = temp("remote");
        let there = Source { picture: dir.join("gallery-dev-42.img"), ..s.clone() };
        write_png(&there.picture, &Rgba8::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]), None).unwrap();
        fetch(&there).unwrap();
        assert_eq!(load(&there.picture).unwrap().width, 2);
        // Fetched elsewhere already (a drag out of Photos & captures): put in place, not downloaded again.
        let seeded = Source { picture: dir.join("seeded.img"), ..s.clone() };
        seed(&seeded, &there.picture);
        assert_eq!(load(&seeded.picture).unwrap().width, 2);
        // An asset's picture by its link alone: named after the link, one file per link.
        let link = |url: &str| Source::remote(Remote { kind: RemoteKind::Link, url: url.into(), ..Default::default() }).picture;
        assert_ne!(link("https://orbiters.cc/a.webp"), link("https://orbiters.cc/b.webp"));
        // Night PG2-7: a community gallery's picture 12 and an asset's showcase picture 12 are two files, and neither
        // is the old unqualified name an earlier version kept either of them under (night3, verifier point 3).
        let gallery = |asset: u64| Remote { kind: RemoteKind::Gallery, id: 12, asset, ..Default::default() }.file();
        assert_ne!(gallery(0), gallery(5));
        assert_ne!(gallery(5), gallery(6));
        for asset in [0, 5] {
            assert!(!gallery(asset).ends_with("gallery-prod-12.img"), "{asset}: not the old name, whose bytes may be the other kind's");
        }
        assert!(link("https://orbiters.cc/a.webp").file_name().unwrap().to_string_lossy().starts_with("link-"));
                // A shared picture with no name nor world is named by its id.
        let plain = Source::remote(Remote { kind: RemoteKind::Shared, id: 7, ..Default::default() });
        assert!(plain.copy_path().file_name().unwrap().to_string_lossy().starts_with("Orbiters picture 7 (edited"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
