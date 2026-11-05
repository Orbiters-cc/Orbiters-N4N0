//! Lab: renders a reconstruction from cameras given by where they stand and what they look at (a free view no keyframe
//! has, to compare runs off the recorded path, or the images of a simulated capture).
//!
//! ```text
//! cargo run --release --example posed -- <splats.ply> <poses.json> <out dir> [--width N] [--height N]
//! ```
//! poses.json: `[{"name": "left", "eye": [x, y, z], "at": [x, y, z], "fov_y": degrees}, ...]`, or a recorded camera:
//! `{"name": "k", "camera_to_world": [12 values], "projection": [4 values], "width": w, "height": h}`. A `sky.png` beside the
//! PLY is drawn behind the splats. A `.n4tree` in place of the PLY draws its leaves as the file stores them.

use anyhow::Context as _;
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::{gpu, ply, train};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let (mut width, mut height) = (1920u32, 1080u32);
    let mut rest: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width N")?.parse()?,
            "--height" => height = it.next().context("--height N")?.parse()?,
            _ => rest.push(a),
        }
    }
    let [splats, poses, out] = <[String; 3]>::try_from(rest).map_err(|_| anyhow::anyhow!("usage: posed <splats.ply> <poses.json> <out dir> [--width N] [--height N]"))?;
    let (splats, out) = (PathBuf::from(splats), PathBuf::from(out));
    let list: serde_json::Value = serde_json::from_slice(&std::fs::read(&poses)?)?;
    let g = if splats.extension().is_some_and(|e| e == "n4tree") {
        orbiters_reconstruct::tree_file::parse(&std::fs::read(&splats)?)?.select_stored(|n| n.leaf)
    } else {
        ply::read(&splats)?
    };
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    let map = splats.with_file_name(orbiters_core::reconstruction::SKY_FILE);
    if map.is_file() {
        let sky = orbiters_reconstruct::sky::SkyMap::read(&map)?;
        gpu.set_sky(Some((sky.width, &sky.rgb)))?;
    }
    std::fs::create_dir_all(&out)?;
    let background = train::Settings::default().background;
    let vector = |v: &serde_json::Value| -> [f32; 3] { std::array::from_fn(|i| v[i].as_f64().unwrap_or(0.0) as f32) };
    for pose in list.as_array().context("a list of poses")? {
        let camera = if pose["camera_to_world"].is_array() {
            let m: [f32; 12] = std::array::from_fn(|i| pose["camera_to_world"][i].as_f64().unwrap_or(0.0) as f32);
            let p: [f32; 4] = std::array::from_fn(|i| pose["projection"][i].as_f64().unwrap_or(0.0) as f32);
            Camera::from_strip(&m, &p, pose["width"].as_u64().unwrap_or(width as u64) as u32, pose["height"].as_u64().unwrap_or(height as u64) as u32, 1.0)
        } else {
            let (eye, at) = (vector(&pose["eye"]), vector(&pose["at"]));
            let fov = pose["fov_y"].as_f64().unwrap_or(47.27) as f32;
            Camera::look(eye, [at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]], [0.0, 1.0, 0.0], fov.to_radians(), width, height)
        };
        let image = train::render(&mut gpu, &camera, background)?;
        let rgb: Vec<u8> = image.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
        let name = pose["name"].as_str().context("a pose's name")?;
        let file = std::io::BufWriter::new(std::fs::File::create(out.join(format!("{name}.png")))?);
        let mut encoder = png::Encoder::new(file, camera.width, camera.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&rgb)?;
    }
    Ok(())
}
