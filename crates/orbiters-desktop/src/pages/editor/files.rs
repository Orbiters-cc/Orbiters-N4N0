//! What the editor reads and writes: the picture (and a VRChat multi-layer picture's layers), the edited copy beside
//! it, and the edit saved next to the copy so that opening the copy again edits it again, from the original.
//!
//! The copy is `<name> (edited).png` in the original's folder (`(edited 2)` and so on when taken), with the original's
//! XMP packet (VRChat's world, its name and when it was taken), so Photos & captures lists it with the original's
//! world and instance. The edit is `<copy's name>.orbiters-edit.json` beside it. The original is never written.

use orbiters_ui::photo::{Edit, Rgba8, SourceStack};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The picture edited: a file, or a VRChat multi-layer picture (its own file, VRChat's composite, and its layers).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Source {
    pub picture: PathBuf,
    /// VRChat's layers, back to front ("Environment", "Player", "UI"); empty for a plain picture.
    #[serde(default)]
    pub layers: Vec<SourceLayer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLayer {
    pub name: String,
    pub path: PathBuf,
}

impl Source {
    pub fn picture(path: impl Into<PathBuf>) -> Self {
        Self { picture: path.into(), layers: Vec::new() }
    }

    /// A multi-layer picture of `picture` (VRChat's composite, or its bottom layer when that is gone) and its `layers`
    /// back to front ([`crate::controller::shares::Entry::layers`]).
    pub fn with_layers(picture: impl Into<PathBuf>, layers: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        Self { picture: picture.into(), layers: layers.into_iter().map(|(name, path)| SourceLayer { name, path }).collect() }
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
            out.push(SourceLayer { name: "Picture".into(), path: self.picture.clone() });
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
}

pub const FORMAT: &str = "orbiters-photo-edit";

impl Saved {
    pub fn new(source: Source, edit: Edit) -> Self {
        Self { format: FORMAT.into(), version: 1, source, edit }
    }
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
    let stem = original.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Picture".into());
    let dir = original.parent().map(Path::to_path_buf).unwrap_or_default();
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem} (edited).png") } else { format!("{stem} (edited {n}).png") }))
        .find(|p| !p.exists() && !sidecar(p).exists())
        .expect("a free name")
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

/// Shows `path` selected in its folder (Explorer), or opens its folder.
pub fn reveal(path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn()?;
        Ok(())
    }
    #[cfg(not(windows))]
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
}
