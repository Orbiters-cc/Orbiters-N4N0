//! VR sessions: a side session's VRChat in VR mode inside the developer VR sandbox (`tools/vr-sandbox`, docs/vr-sandbox.md),
//! driven through emulated SteamVR controllers instead of the keyboard and the mouse (nofocus, 2026-11-15).
//!
//! Why: in desktop mode VRChat takes its keyboard and mouse only from raw input of the foreground window. Posted window
//! messages, with or without a faked activation, did nothing (measured on profile 1's VRChat, 13:29), and OSC cannot open
//! the camera (`/usercamera/Mode` is not even stored while it is closed) while the camera closes at every world change. So a
//! desktop tour needs the person away from the computer for every world. In VR, the controllers come from the sandbox's
//! driver: no focus, no cursor, nothing of the person's. Measured live at 13:36-13:38: a profile-2 VRChat in VR beside the
//! person's work, the Action Menu's camera route opened the camera, OSC switched it to Stream (Spout `VRCSender1` up) and
//! `/usercamera/Pose` moved it (echoed on its out port), with the person's program in front all along.
//!
//! The sandbox is development tooling, not shipped: a VR session exists only where the person set
//! `experimental.vrchat_bridge_vr_sandbox` to that folder (Windows, Python with psutil).
// i18n: not shown (what an AI client reads in the tools' answers)
use super::game::Failure;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A sandbox turn ends by itself after 10 minutes without a request (docs/vr-sandbox.md): renewed well before.
const RENEW_EVERY: Duration = Duration::from_secs(120);

/// What a VR session needs of the sandbox. The real one runs `sandbox.py`; the lab's records the calls.
pub trait VrRig: Send + Sync {
    /// Takes the session's turn on the shared sandbox, starting SteamVR with the emulated headset if none runs, leaving the
    /// installed app running (`--keep-app`). Refused while the person uses VR (`vr_busy`, with the sandbox's reason).
    fn take(&self) -> Result<(), Failure>;
    /// Keeps the turn (called on every VR call and now and then while idle).
    fn keep(&self);
    /// Opens VRChat's camera through the Action Menu (right hand: hold B, Tools, Camera).
    fn camera(&self) -> Result<(), Failure>;
    /// Aims the right controller's laser through `pixel` of a `size` picture of the left eye; with `click`, pulls the trigger.
    fn point(&self, pixel: (f64, f64), size: (u32, u32), click: bool) -> Result<(), Failure>;
    /// The left eye's picture as SteamVR composited it: PNG bytes.
    fn eye(&self) -> Result<Vec<u8>, Failure>;
    /// Gives the turn back (the session closed).
    fn release(&self);
}

/// The sandbox folder the person set for VR sessions, when it holds `sandbox.py`.
pub fn sandbox_root(setting: &str) -> Option<PathBuf> {
    let root = PathBuf::from(setting.trim());
    (!setting.trim().is_empty() && root.join("sandbox.py").is_file()).then_some(root)
}

/// `sandbox.py` through its command line, as `owner` (one per bridge session: `bridge-<session id>`).
pub struct SandboxRig {
    root: PathBuf,
    owner: String,
    renewed: Mutex<Option<Instant>>,
}

impl SandboxRig {
    pub fn new(root: PathBuf, session: &str) -> Self {
        Self { root, owner: format!("bridge-{session}"), renewed: Mutex::new(None) }
    }

    fn run(&self, args: &[&str], limit: Duration) -> Result<Value, Failure> {
        let fail = |words: String| Failure::new("vr_sandbox", format!("The VR sandbox answered: {words}"), "Call get_state; if the session is gone, open it again, or use a desktop session.");
        let mut command = std::process::Command::new("python");
        command.arg(self.root.join("sandbox.py")).args(args).arg("--owner").arg(&self.owner);
        command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|e| fail(e.to_string()))?;
        let begin = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if begin.elapsed() < limit => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(fail(format!("no answer within {} s", limit.as_secs())));
                }
            }
        }
        let out = child.wait_with_output().map_err(|e| fail(e.to_string()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        // `acquire` prints waiting lines before its answer: the last JSON line is the answer.
        let last = text.lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok());
        match (out.status.success(), last) {
            (true, Some(answer)) => {
                *self.renewed.lock().unwrap() = Some(Instant::now());
                Ok(answer)
            }
            _ => {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(fail(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no answer").trim().to_string()))
            }
        }
    }

    fn call(&self, args: Value, limit: Duration) -> Result<Value, Failure> {
        self.run(&["call", &args.to_string()], limit)
    }
}

impl VrRig for SandboxRig {
    fn take(&self) -> Result<(), Failure> {
        self.run(&["acquire", "--keep-app", "--render", "default", "--wait", "20"], Duration::from_secs(120)).map(|_| ()).map_err(|f| {
            Failure::new(
                "vr_busy",
                format!("No VR session now: {}", f.message),
                "The person's own VR (their headset, their SteamVR or their VRChat) wins, and so does another agent's turn. Use a desktop session, or wait and ask again later.",
            )
        })
    }

    fn keep(&self) {
        let due = self.renewed.lock().unwrap().is_none_or(|t| t.elapsed() >= RENEW_EVERY);
        if due {
            let _ = self.run(&["renew"], Duration::from_secs(20));
        }
    }

    fn camera(&self) -> Result<(), Failure> {
        self.call(json!({"action": "camera", "device": "right"}), Duration::from_secs(30)).map(|_| ())
    }

    fn point(&self, pixel: (f64, f64), size: (u32, u32), click: bool) -> Result<(), Failure> {
        self.call(json!({"action": "point", "device": "right", "pixel": [pixel.0, pixel.1], "size": [size.0, size.1], "click": click}), Duration::from_secs(30)).map(|_| ())
    }

    fn eye(&self) -> Result<Vec<u8>, Failure> {
        let prefix = std::env::temp_dir().join(format!("orbiters-vr-eye-{}", std::process::id()));
        let answer = self.call(json!({"action": "capture", "output_prefix": prefix.to_string_lossy()}), Duration::from_secs(30))?;
        let left = answer["paths"][0].as_str().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{}-left.png", prefix.to_string_lossy())));
        let bytes = std::fs::read(&left).map_err(|e| Failure::new("vr_sandbox", format!("The eye picture could not be read: {e}"), "Try again."));
        let _ = std::fs::remove_file(&left);
        let _ = std::fs::remove_file(left.with_file_name(left.file_name().unwrap_or_default().to_string_lossy().replace("-left", "-right")));
        bytes
    }

    fn release(&self) {
        let _ = self.run(&["release"], Duration::from_secs(150));
    }
}

/// A PNG (the eye) as the screenshot's picture, at most `max_width` wide.
pub fn eye_shot(png: &[u8], max_width: u32) -> Result<super::game::Shot, Failure> {
    let failed = |why: String| Failure::new("screenshot_failed", format!("The headset's picture could not be read: {why}"), "Try again.");
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|e| failed(e.to_string()))?.to_rgba8();
    let (width, height) = image.dimensions();
    let mut pixels = image.into_raw();
    for p in pixels.as_chunks_mut::<4>().0 {
        p.swap(0, 2); // RGBA to the BGRA the window capture gives
    }
    super::capture::encode(&super::capture::Bgra { width, height, pixels }, max_width).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_folder_counts_only_with_its_script() {
        let dir = std::env::temp_dir().join(format!("vr-root-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(sandbox_root(""), None, "not set: no VR sessions");
        assert_eq!(sandbox_root(&dir.to_string_lossy()), None, "a folder without sandbox.py");
        std::fs::write(dir.join("sandbox.py"), "").unwrap();
        assert_eq!(sandbox_root(&dir.to_string_lossy()), Some(dir.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_eye_is_a_picture_like_a_window_capture() {
        let mut png = Vec::new();
        let image = image::RgbaImage::from_pixel(1620, 1800, image::Rgba([200, 40, 10, 255]));
        image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let shot = eye_shot(&png, 810).unwrap();
        assert_eq!((shot.width, shot.height), (810, 900), "scaled to the asked width, its shape kept");
        assert!(!shot.all_black);
        let decoded = image::load_from_memory(&shot.jpeg).unwrap().to_rgb8();
        let p = decoded.get_pixel(400, 400);
        assert!(p[0] > 150 && p[2] < 60, "colours kept (not swapped): {p:?}");
    }
}
