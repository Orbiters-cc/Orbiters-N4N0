//! What a reconstruction shows where the wearer stood (crate::wearer), to look at with one's own eyes: the start view (the
//! session's first keyframe), then views around the wearer's hips, turned every 45° at 3 m and at 1.5 m, and from above, all
//! on one sheet. The same cameras for any PLY of a session, so before and after can be put side by side.
//!
//! ```text
//! cargo run --release --example wearer_views -- <session> <splats.ply> <out.png> [--width N]
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::dataset::{self, Budget};
use orbiters_reconstruct::{gpu, ply, train, wearer};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let mut width = 640u32;
    let mut rest: Vec<String> = Vec::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width N")?.parse()?,
            _ => rest.push(a),
        }
    }
    let [session, splats, out] = <[String; 3]>::try_from(rest).map_err(|_| anyhow::anyhow!("usage: wearer_views <session> <splats.ply> <out.png>"))?;
    let (session, splats, out) = (PathBuf::from(session), PathBuf::from(splats), PathBuf::from(out));
    let data = dataset::load_with(&session, 1920, Budget::default(), |_| {})?;
    let track = wearer::Track::read(&session).context("no skeleton frames")?;
    let hips = track.frames[0].hips;
    let g = ply::read(&splats)?;
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    let map = splats.with_file_name(orbiters_core::reconstruction::SKY_FILE);
    if map.is_file() {
        let sky = orbiters_reconstruct::sky::SkyMap::read(&map)?;
        gpu.set_sky(Some((sky.width, &sky.rgb)))?;
    }
    let height = width * 9 / 16;
    let factor = data.geometry.full_width / width;
    let fov = 2.0 * (0.5 * data.height as f32 / data.views[0].camera.fy).atan();
    let around = |angle: f32, distance: f32, up: f32| {
        let (s, c) = (angle.to_radians().sin(), angle.to_radians().cos());
        let eye = [hips[0] + s * distance, hips[1] + up, hips[2] + c * distance];
        Camera::look(eye, [hips[0] - eye[0], -up - 0.1, hips[2] - eye[2]], [0.0, 1.0, 0.0], fov, width, height)
    };
    let mut cameras = vec![data.views[0].camera.scaled(factor.max(1))];
    cameras.extend([0.0f32, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0].map(|a| around(a, 3.0, 0.3)));
    cameras.extend([0.0f32, 90.0, 180.0, 270.0].map(|a| around(a, 1.5, 0.2)));
    cameras.extend([45.0f32, 225.0].map(|a| around(a, 1.5, 1.5)));
    let cols = 3u32;
    let rows = (cameras.len() as u32).div_ceil(cols);
    let mut rgb = vec![0u8; (cols * width * rows * height * 3) as usize];
    for (k, cam) in cameras.iter().enumerate() {
        let (cx, cy) = (k as u32 % cols * width, k as u32 / cols * height);
        let (cw, ch) = (cam.width, cam.height);
        let img = train::render(&mut gpu, cam, [0.0; 3])?;
        for y in 0..ch.min(height) {
            for x in 0..cw.min(width) {
                let p = img[(y * cw + x) as usize];
                let at = (((cy + y) * cols * width + cx + x) * 3) as usize;
                for c in 0..3 {
                    rgb[at + c] = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(&out)?);
    let mut enc = png::Encoder::new(file, cols * width, rows * height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}
