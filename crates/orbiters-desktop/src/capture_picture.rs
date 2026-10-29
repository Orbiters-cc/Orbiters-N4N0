//! A 3D capture's picture: its own 3D view as the reconstruction rendered it (contracts `Docs/capture-sharing.md`,
//! Capture picture), else the session's thumbnail. Sent with a capture's upload (the link previews of its public page and
//! the website's cards show it), and what dragging a capture out of Photos & captures hands other apps.
use std::path::{Path, PathBuf};

/// The picture as top-down RGBA: (width, height, pixels).
pub fn rgba(session: &Path) -> Option<(u32, u32, Vec<u8>)> {
    orbiters_core::reconstruction::read_preview(session).or_else(|| orbiters_core::bundle::read_thumbnail(session))
}

fn rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

/// As a JPEG for Orbiters (about 300 KB for a 1920 by 1080 view).
pub fn jpeg(session: &Path) -> Option<Vec<u8>> {
    let (w, h, rgba) = rgba(session)?;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode(&rgb(&rgba), w, h, image::ExtendedColorType::Rgb8).ok()?;
    Some(out)
}

/// As a PNG file named after the capture (`<name>.png`) in `dir`, written again only when the capture's picture
/// changed since (a new reconstruction): what a capture dragged out of the app is.
pub fn png_file(session: &Path, dir: &Path, name: &str) -> Option<PathBuf> {
    let source = [
        orbiters_core::reconstruction::result_dir(session).join(orbiters_core::reconstruction::PREVIEW_FILE),
        session.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE),
    ]
    .into_iter()
    .find_map(|p| std::fs::metadata(p).ok()?.modified().ok());
    let file = dir.join(format!("{name}.png"));
    let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|made| source.is_some_and(|s| made >= s));
    if fresh {
        return Some(file);
    }
    let (w, h, rgba) = rgba(session)?;
    std::fs::create_dir_all(dir).ok()?;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb(&rgba), w, h, image::ExtendedColorType::Rgb8).ok()?;
    orbiters_core::write_atomic(&file, &out).ok()?;
    Some(file)
}

use image::ImageEncoder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_without_pictures_has_none_and_one_with_a_thumbnail_gives_jpeg_and_png() {
        let root = std::env::temp_dir().join(format!("orbiters-capture-picture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(jpeg(&root).is_none());
        let pixels: Vec<u8> = (0..32 * 18).flat_map(|i| [(i % 255) as u8, 80, 160, 255]).collect();
        std::fs::write(root.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE), orbiters_core::contracts::qoi::encode(&pixels, 32, 18, 4)).unwrap();
        let bytes = jpeg(&root).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
        let out = root.join("out");
        let file = png_file(&root, &out, "World 2026-10-29").unwrap();
        assert_eq!(file.file_name().unwrap(), "World 2026-10-29.png");
        let made = std::fs::metadata(&file).unwrap().modified().unwrap();
        // Unchanged capture: the file is kept as it is.
        assert_eq!(png_file(&root, &out, "World 2026-10-29").unwrap(), file);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), made);
        let _ = std::fs::remove_dir_all(&root);
    }
}
