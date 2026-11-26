//! A 3D capture's picture: its own 3D view as the reconstruction rendered it (contracts `Docs/capture-sharing.md`,
//! Capture picture), else the session's thumbnail. Sent with a capture's upload (the link previews of its public page and
//! the website's cards show it), and what dragging a capture out of Photos & captures hands other apps.
//!
//! With splat edits (orbiters_reconstruct::edit) the picture leaves the app only once it was drawn again for the edits
//! there are ([`orbiters_reconstruct::edit::picture_state`]): none while it may still show what they removed (just
//! edited, its redraw failed, edits that cannot be applied), and never the recording's own thumbnail in its place.
use std::path::{Path, PathBuf};

/// The picture as top-down RGBA: (width, height, pixels); None while none may leave the app.
pub fn rgba(session: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let state = orbiters_reconstruct::edit::picture_state(&orbiters_core::reconstruction::result_dir(session));
    if state.preview_ok() {
        if let Some(p) = orbiters_core::reconstruction::read_preview(session) {
            return Some(p);
        }
    }
    state.thumbnail_ok().then(|| orbiters_core::bundle::read_thumbnail(session)).flatten()
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

/// When the picture a PNG of the capture is made from was last written (its 3D view, else its thumbnail).
fn source_written(session: &Path) -> Option<std::time::SystemTime> {
    [
        orbiters_core::reconstruction::result_dir(session).join(orbiters_core::reconstruction::PREVIEW_FILE),
        session.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE),
    ]
    .into_iter()
    .find_map(|p| std::fs::metadata(p).ok()?.modified().ok())
}

/// Whether `file`, the PNG [`png_file`] made of `session`, may still be handed out now: the capture's picture may still
/// leave the app (no edit since that it is not drawn for), and `file` is made from the picture there is now (none
/// written after it; the capture still there). What a drag or a copy looks at as it hands out a PNG made earlier on
/// a worker (review 1124, W56-verify B4).
pub fn still_handed(session: &Path, file: &Path) -> bool {
    let state = orbiters_reconstruct::edit::picture_state(&orbiters_core::reconstruction::result_dir(session));
    if !state.preview_ok() {
        return false;
    }
    let made = std::fs::metadata(file).and_then(|m| m.modified());
    matches!((source_written(session), made), (Some(source), Ok(made)) if made >= source)
}

/// As a PNG file named after the capture (`<name>.png`) in `dir`, written again only when the capture's picture
/// changed since (a new reconstruction): what a capture dragged out of the app is.
pub fn png_file(session: &Path, dir: &Path, name: &str) -> Option<PathBuf> {
    // A PNG made before the edits is not handed out while the picture is not drawn for them.
    let state = orbiters_reconstruct::edit::picture_state(&orbiters_core::reconstruction::result_dir(session));
    if !state.preview_ok() {
        return None;
    }
    let source = source_written(session);
    let file = dir.join(format!("{name}.png"));
    let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|made| source.is_some_and(|s| made >= s));
    if fresh {
        return Some(file);
    }
    let (w, h, rgba) = rgba(session)?;
    std::fs::create_dir_all(dir).ok()?;
    #[cfg(test)]
    ENCODED.with(|n| n.set(n.get() + 1));
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb(&rgba), w, h, image::ExtendedColorType::Rgb8).ok()?;
    orbiters_core::write_atomic(&file, &out).ok()?;
    Some(file)
}

use image::ImageEncoder;

#[cfg(test)]
thread_local! {
    /// Tests: the PNGs [`png_file`] encoded on this thread.
    pub static ENCODED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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

    /// Splats deleted and the view closed at once (the picture not drawn again yet, or its redraw failed): no picture
    /// leaves the app, not the preview as trained, not the recording's thumbnail, not a PNG dragged out before; drawn
    /// again for the edits, it does; damaged edits, none.
    #[test]
    fn an_edited_capture_gives_no_picture_until_it_is_drawn_for_its_edits() {
        use orbiters_core::reconstruction as rec;
        use orbiters_reconstruct::edit::{DELETED, Edit, Identity};
        let root = std::env::temp_dir().join(format!("orbiters-capture-picture-edited-{}", uuid::Uuid::new_v4()));
        let result = rec::dir(&root);
        std::fs::create_dir_all(&result).unwrap();
        let mut g = orbiters_reconstruct::gaussians::Gaussians::default();
        g.push([0.0, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        g.push([1.0, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        orbiters_reconstruct::ply::write(&result.join(rec::PLY_FILE), &g).unwrap();
        std::fs::write(result.join(rec::PREVIEW_FILE), orbiters_core::contracts::qoi::encode(&[200u8; 8 * 4 * 3], 8, 4, 3)).unwrap();
        std::fs::write(root.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE), orbiters_core::contracts::qoi::encode(&[90u8; 8 * 4 * 4], 8, 4, 4)).unwrap();
        let out = root.join("out");
        assert!(jpeg(&root).is_some() && png_file(&root, &out, "Before").is_some(), "no edits: the picture as trained");
        let mut edit = Edit::new(2, Identity::read(&result.join(rec::PLY_FILE)).unwrap());
        edit.marks[1] = DELETED;
        edit.picture = false;
        edit.save(&result).unwrap();
        assert!(rgba(&root).is_none() && jpeg(&root).is_none(), "edited, not drawn again: nothing");
        assert!(png_file(&root, &out, "Before").is_none(), "not even the PNG made before");
        std::fs::remove_file(result.join(rec::PREVIEW_FILE)).unwrap();
        assert!(rgba(&root).is_none(), "and never the recording's thumbnail instead");
        std::fs::write(result.join(rec::PREVIEW_FILE), orbiters_core::contracts::qoi::encode(&[30u8; 8 * 4 * 3], 8, 4, 3)).unwrap();
        edit.picture = true;
        edit.save(&result).unwrap();
        assert_eq!(rgba(&root).map(|(_, _, p)| p[0]), Some(30), "drawn for the edits: given");
        std::fs::write(result.join(rec::EDIT_FILE), b"damaged").unwrap();
        assert!(rgba(&root).is_none() && png_file(&root, &out, "After").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
