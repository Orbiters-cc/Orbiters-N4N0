//! What crate::wearer takes out of a session, to look at with one's own eyes: no GPU. Sheets of views with the left-out pixels
//! tinted, the depth samples it took (points in the world seen from the side and from above), and its numbers.
//!
//! ```text
//! cargo run --release --example wearer_masks -- <session> <out dir> [--width N] [--views N]
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::{dataset, prepare, wearer};
use std::path::{Path, PathBuf};

fn write_png(path: &Path, rgb: &[u8], w: u32, h: u32) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgb)?;
    Ok(())
}

/// A view at `w`×`h`, pixels left out tinted magenta, in a `tw`×`th` cell of a sheet.
fn cell(view: &dataset::View, w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; (tw * th * 3) as usize];
    let scale = (w as f32 / tw as f32).max(h as f32 / th as f32);
    for y in 0..th {
        for x in 0..tw {
            let (sx, sy) = ((x as f32 * scale) as u32, (y as f32 * scale) as u32);
            if sx >= w || sy >= h {
                continue;
            }
            let p = view.image[(sy * w + sx) as usize];
            let rgb = [p & 255, (p >> 8) & 255, (p >> 16) & 255];
            let px = if wearer::is_out(p) { [(rgb[0] / 2 + 120).min(255), rgb[1] / 3, (rgb[2] / 2 + 120).min(255)] } else { rgb };
            let at = ((y * tw + x) * 3) as usize;
            out[at] = px[0] as u8;
            out[at + 1] = px[1] as u8;
            out[at + 2] = px[2] as u8;
        }
    }
    out
}

fn sheet(cells: &[Vec<u8>], tw: u32, th: u32, cols: u32) -> (Vec<u8>, u32, u32) {
    let rows = (cells.len() as u32).div_ceil(cols);
    let (w, h) = (tw * cols, th * rows);
    let mut out = vec![0u8; (w * h * 3) as usize];
    for (k, c) in cells.iter().enumerate() {
        let (cx, cy) = (k as u32 % cols * tw, k as u32 / cols * th);
        for y in 0..th {
            let from = (y * tw * 3) as usize;
            let to = (((cy + y) * w + cx) * 3) as usize;
            out[to..to + (tw * 3) as usize].copy_from_slice(&c[from..from + (tw * 3) as usize]);
        }
    }
    (out, w, h)
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut out, mut width, mut count) = (None, None, 1920u32, 24usize);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width")?.parse()?,
            "--views" => count = it.next().context("--views")?.parse()?,
            _ if session.is_none() => session = Some(PathBuf::from(a)),
            _ => out = Some(PathBuf::from(a)),
        }
    }
    let (session, out) = (session.context("session")?, out.context("out dir")?);
    std::fs::create_dir_all(&out)?;
    let start = std::time::Instant::now();
    if let Some(track) = wearer::Track::read(&session) {
        let keep = prepare::prepare(&session, &prepare::Options { width, wearer: prepare::WearerOut::Keep, aberration: false, ..Default::default() }, |_| {})?;
        let hips = track.frames[0].hips;
        let rows: Vec<String> = wearer::labelled(&keep.data, &track).iter().map(|(p, n, l)| format!("{} {} {} {} {}", p[0] - hips[0], p[1] - hips[1], p[2] - hips[2], n, l)).collect();
        std::fs::write(out.join("voxels.txt"), rows.join("\n"))?;
    }
    let mut prepared = prepare::prepare(&session, &prepare::Options { width, wearer: prepare::WearerOut::Remove, ..Default::default() }, |_| {})?;
    let seconds = start.elapsed().as_secs_f32();
    for line in prepared.describe() {
        println!("{line}");
    }
    println!("prepared in {seconds:.1} s");
    let data = &mut prepared.data;
    let (w, h) = (data.width, data.height);
    let (tw, th) = (480u32, 270u32);
    let step = (data.views.len() / count.max(1)).max(1);
    let cells: Vec<Vec<u8>> = data.views.iter().step_by(step).map(|v| cell(v, w, h, tw, th)).collect();
    let (rgb, sw, sh) = sheet(&cells, tw, th, 4);
    write_png(&out.join("views.png"), &rgb, sw, sh)?;
    if let Some(f) = &data.faces {
        let step = (f.views.len() / count.max(1)).max(1);
        let (fw, fh) = f.sizes.first().map_or((270, 270), |s| (s.width, s.height));
        let cells: Vec<Vec<u8>> = f.views.iter().step_by(step).map(|v| cell(v, fw, fh, 270, 270)).collect();
        let (rgb, sw, sh) = sheet(&cells, 270, 270, 6);
        write_png(&out.join("faces.png"), &rgb, sw, sh)?;
    }
    Ok(())
}
