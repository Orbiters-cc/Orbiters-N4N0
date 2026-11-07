//! `orbiters-desktop --eye-picture <out.png> [--right]`: what the headset shows now, one eye, as SteamVR's compositor has
//! it (`orbiters_openvr::mirror`): the picture N4N0's phone camera will take its photos from, without VRChat's camera.
//! Needs SteamVR running; nothing else starts.

use anyhow::Context;
use orbiters_openvr::compositor::Eye;
use orbiters_openvr::mirror::EyeReader;
use std::path::Path;
use std::time::Instant;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out = Path::new(args.first().context("usage: --eye-picture <out.png> [--right]")?);
    let eye = if args.iter().any(|a| a == "--right") { Eye::Right } else { Eye::Left };
    let client = super::connect_steamvr()?.context("SteamVR is not running")?; // i18n: not shown (a developer's command line, printed on a terminal in English like its usage line)
    let reader = EyeReader::new(client.dxgi_adapter_index()).map_err(anyhow::Error::msg)?;
    let compositor = client.compositor().map_err(|e| anyhow::anyhow!("{e}"))?;
    let start = Instant::now();
    let picture = reader.read(&compositor, eye).map_err(anyhow::Error::msg)?;
    println!("{:?} eye: {} × {} read in {} ms", eye, picture.width, picture.height, start.elapsed().as_millis());
    super::menu_pages::write_png(out, picture.width, picture.height, &picture.rgba)?;
    println!("{}", out.display());
    Ok(())
}
