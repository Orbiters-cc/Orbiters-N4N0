//! Writes the app's icons from the N4N0 mark (`orbiters_ui::n4n0_logo::icon`) into a folder:
//! `cargo run -p orbiters-ui --example n4n0_icon -- crates/orbiters-desktop/assets` writes `32x32.png` (the tray),
//! `128x128@2x.png` (the window, 256 pixels) and `icon.ico` (the executable and the setup: 16 to 256 pixels, each a PNG).

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
    Ok(())
}
