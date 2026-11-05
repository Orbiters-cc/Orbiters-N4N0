//! Lab: renders of a reconstruction from a session's keyframes and from beside them, as reconstruct-lab3's `lab.rs`
//! drew them (the review's SPL-3 floater at Exoplanet's keyframe 60 shows 0.6 m above it): each keyframe's own view,
//! "up" (0.6 m higher) and "side" (1 m to its right), both looking at the point 4 m ahead of the keyframe.
//!
//! ```text
//! cargo run --release --example novel -- <session> <splats.ply> <out dir> <keyframe>[,<keyframe>...] [--width N]
//! ```
//! A `sky.png` beside the PLY (orbiters_reconstruct::sky::SkyMap) is drawn behind the splats. A VRChat splat tree
//! (`pc.n4tree`, from `orbiters-reconstruct --vrchat`) in place of the PLY draws its leaves as the file stores them.

use anyhow::Context as _;
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::dataset::{self, Budget};
use orbiters_reconstruct::{gpu, ply, train};
use std::path::{Path, PathBuf};

fn write_png(path: &Path, img: &[[f32; 4]], w: u32, h: u32) -> anyhow::Result<()> {
    let rgb: Vec<u8> = img.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| path.display().to_string())?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let mut width = 1920u32;
    let mut rest: Vec<String> = Vec::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width N")?.parse()?,
            _ => rest.push(a),
        }
    }
    let usage = "usage: novel <session> <splats.ply> <out dir> <keyframe>[,<keyframe>...] [--width N]";
    let [session, splats, out, keys] = <[String; 4]>::try_from(rest).map_err(|_| anyhow::anyhow!(usage))?;
    let (session, splats, out) = (PathBuf::from(session), PathBuf::from(splats), PathBuf::from(out));
    let keys: Vec<i32> = keys.split(',').map(|k| k.trim().parse()).collect::<Result<_, _>>()?;
    let data = dataset::load_with(&session, width, Budget::default(), |_| {})?;
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
    for key in keys {
        let Some(view) = data.views.iter().find(|v| v.keyframe == key) else {
            eprintln!("no keyframe {key}");
            continue;
        };
        let cam = &view.camera;
        write_png(&out.join(format!("render_{key:06}.png")), &train::render(&mut gpu, cam, background)?, data.width, data.height)?;
        let (eye, fwd) = (cam.position(), cam.forward());
        let target = [eye[0] + 4.0 * fwd[0], eye[1] + 4.0 * fwd[1], eye[2] + 4.0 * fwd[2]];
        let right = cam.r[0];
        for (tag, moved) in [("up", [eye[0], eye[1] + 0.6, eye[2]]), ("side", [eye[0] + right[0], eye[1] + right[1], eye[2] + right[2]])] {
            let look = [target[0] - moved[0], target[1] - moved[1], target[2] - moved[2]];
            let novel = Camera::look(moved, look, [0.0, 1.0, 0.0], cam.fov_y(), data.width, data.height);
            write_png(&out.join(format!("novel_{tag}_{key:06}.png")), &train::render(&mut gpu, &novel, background)?, data.width, data.height)?;
        }
    }
    Ok(())
}
