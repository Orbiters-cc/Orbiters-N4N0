//! Writes the app's icons from the N4N0 mark (`orbiters_ui::n4n0_logo::icon`) into a folder:
//! `cargo run -p orbiters-ui --example n4n0_icon -- crates/orbiters-desktop/assets` writes `32x32.png` (the tray),
//! `128x128@2x.png` (the window, 256 pixels) and `icon.ico` (the executable and the setup: 16 to 256 pixels, each a PNG).
//! On a Mac also `AppIcon.icns` (the application, through the system's `iconutil`) and `menu-bar.png` (the mark alone,
//! black on clear, which macOS colours for the menu bar: 22 points at two pixels each, the mark two points in).

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: n4n0_icon <folder>"))?);
    std::fs::create_dir_all(&dir)?;
    let png = |size: u32| -> anyhow::Result<Vec<u8>> { Ok(orbiters_ui::n4n0_logo::icon(size).encode_png()?) };
    std::fs::write(dir.join("32x32.png"), png(32)?)?;
    std::fs::write(dir.join("128x128@2x.png"), png(256)?)?;
    // An ICO of PNG images: its header, a directory entry per size, then the images.
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let images = sizes.iter().map(|&s| png(s)).collect::<anyhow::Result<Vec<_>>>()?;
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&size, image) in sizes.iter().zip(&images) {
        // 256 is written as 0.
        let side = if size >= 256 { 0 } else { size as u8 };
        ico.extend_from_slice(&[side, side, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in &images {
        ico.extend_from_slice(image);
    }
    std::fs::write(dir.join("icon.ico"), ico)?;
    println!("wrote 32x32.png, 128x128@2x.png and icon.ico in {}", dir.display());
    #[cfg(target_os = "macos")]
    mac(&dir, &png)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn mac(dir: &std::path::Path, png: &impl Fn(u32) -> anyhow::Result<Vec<u8>>) -> anyhow::Result<()> {
    let set = std::env::temp_dir().join(format!("orbiters-{}.iconset", std::process::id()));
    std::fs::create_dir_all(&set)?;
    for points in [16u32, 32, 128, 256, 512] {
        std::fs::write(set.join(format!("icon_{points}x{points}.png")), png(points)?)?;
        std::fs::write(set.join(format!("icon_{points}x{points}@2x.png")), png(points * 2)?)?;
    }
    let status = std::process::Command::new("iconutil").arg("-c").arg("icns").arg(&set).arg("-o").arg(dir.join("AppIcon.icns")).status()?;
    std::fs::remove_dir_all(&set)?;
    anyhow::ensure!(status.success(), "iconutil failed ({status})");

    let side = 44u32;
    let width = side - 8;
    let size = orbiters_ui::n4n0_logo::size_for_width(width as f32);
    let (w, h) = (size.x.round() as usize, size.y.round() as usize);
    let mark = orbiters_ui::n4n0_logo::rasterize(w as u32, h as u32, egui::Color32::BLACK);
    let mut rgba = vec![0u8; (side * side * 4) as usize];
    let (x0, y0) = ((side as usize - w) / 2, (side as usize - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let at = ((y + y0) * side as usize + x + x0) * 4;
            rgba[at + 3] = mark.pixels[y * w + x].a();
        }
    }
    let file = std::fs::File::create(dir.join("menu-bar.png"))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), side, side);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    println!("wrote AppIcon.icns and menu-bar.png in {}", dir.display());
    Ok(())
}
