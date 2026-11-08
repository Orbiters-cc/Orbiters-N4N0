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

/// What the headset really shows, kept for judging a round of the menus (`ORBITERS_VR_PICTURES=1`): each time what is up
/// settles for a moment, the left eye's picture goes to `vr-pictures` in the logs' folder, at most [`Moments::MOST`] a
/// session. Renders cannot say how the menus sit in a world, at its depth and in its light: these can. Off unless asked,
/// and kept on this computer only.
pub struct Moments {
    on: bool,
    now: String,
    since: Instant,
    kept_for: String,
    kept: u32,
}

impl Default for Moments {
    fn default() -> Self {
        Self { on: std::env::var_os("ORBITERS_VR_PICTURES").is_some(), now: String::new(), since: Instant::now(), kept_for: String::new(), kept: 0 }
    }
}

impl Moments {
    pub const MOST: u32 = 60;
    /// How long what is up stays the same before it is kept: past a laser's sweep and a panel's spring.
    const SETTLED: std::time::Duration = std::time::Duration::from_millis(1400);

    /// What is up now, said as a few words (`state`): kept once it has stayed so for a moment, and once for each.
    pub fn see(&mut self, steamvr: &orbiters_openvr::Client, state: String) {
        if !self.on || self.kept >= Self::MOST {
            return;
        }
        if state != self.now {
            self.now = state;
            self.since = Instant::now();
            return;
        }
        if self.now == self.kept_for || self.since.elapsed() < Self::SETTLED {
            return;
        }
        self.kept_for = self.now.clone();
        self.kept += 1;
        let Ok(compositor) = steamvr.compositor() else { return };
        let Ok(reader) = EyeReader::new(steamvr.dxgi_adapter_index()) else { return };
        let Ok(picture) = reader.read(&compositor, Eye::Left) else { return };
        let name: String = self.now.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        let file = orbiters_core::paths::logs_dir().join("vr-pictures").join(format!("{:02}-{name}.png", self.kept));
        // Written away from the headset's own loop: a large picture takes a while.
        std::thread::spawn(move || {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = super::menu_pages::write_png(&file, picture.width, picture.height, &picture.rgba);
        });
    }
}
