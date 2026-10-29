//! Reusable upload preparation for public previews and private captures. One atomic, checksummed cache beside the
//! reconstruction holds the three trees and their metadata. Inputs are content-hashed, including cameras and the
//! report that chooses the start view. A retry still gets a new tree key so VRChat can detect a replaced slot.
//!
//! The cache fills in the background right after a reconstruction finishes ([`queue`]), so that sharing it is a cache
//! hit: one `orbiters-desktop --prepare-upload <session>` process at a time ([`run`]), paced as the reconstruction
//! worker is (its GPU queue at low priority, its steps between VRChat's frames while VRChat runs, its process class
//! following VRChat: orbiters_reconstruct::priority). The service starts it only when nothing records or reconstructs,
//! and beside VRChat only while SteamVR runs, whose frame timing paces it ([`start_next`]); a capture, a reconstruction
//! or VRChat started without SteamVR stops it and the session waits again ([`pause`]); a share of a session waits for
//! that session's own preparation rather than starting a second one ([`claim`]).

use orbiters_core::{contracts::manifest, maps::PreviewFiles, reconstruction, visited::sha256_file};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// Change the format/policy marker whenever preparation changes its outputs or tuning policy.
const MAGIC: &[u8; 8] = b"N4PREP02";
const FILE: &str = "upload-prep-v1.bin";
const MAX_BYTES: u64 = 100 << 20;
pub static PREPARING: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fingerprint(Vec<Option<String>>);

impl Fingerprint {
    pub fn read(session: &Path, result: &Path) -> anyhow::Result<Self> {
        let paths = [
            result.join(reconstruction::PLY_FILE),
            session.join(manifest::FRAMES_FILE),
            result.join(reconstruction::REPORT_FILE),
            session.join(manifest::MANIFEST_FILE),
        ];
        let hashes = paths
            .iter()
            .enumerate()
            .map(|(i, path)| match sha256_file(path) {
                Ok(hash) => Ok(Some(hash)),
                Err(e) if i >= 2 && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self(hashes))
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    fingerprint: Fingerprint,
    lengths: [usize; 3],
    metadata: serde_json::Value,
}

pub fn tree_key() -> u32 {
    // Random per upload rather than per cached reconstruction; all three files receive this same key.
    u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap())
}

fn rekey(files: &mut PreviewFiles, key: u32) -> Option<()> {
    for bytes in [&mut files.vrchat_pc, &mut files.vrchat_quest, &mut files.vrchat_coarse] {
        if bytes.get(..8)? != orbiters_reconstruct::tree::MAGIC {
            return None;
        }
        bytes.get_mut(20..24)?.copy_from_slice(&key.to_le_bytes());
    }
    Some(())
}

fn decode(bytes: &[u8], fingerprint: &Fingerprint, key: u32) -> Option<(PreviewFiles, serde_json::Value)> {
    if bytes.get(..8)? != MAGIC || bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let payload = bytes.get(40..)?;
    if &Sha256::digest(payload)[..] != bytes.get(8..40)? {
        return None;
    }
    let length = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?) as usize;
    if length > 16_384 {
        return None;
    }
    let header: Header = serde_json::from_slice(payload.get(4..4 + length)?).ok()?;
    if &header.fingerprint != fingerprint {
        return None;
    }
    let mut rest = payload.get(4 + length..)?;
    let mut next = |n: usize| {
        let file = rest.get(..n)?.to_vec();
        rest = rest.get(n..)?;
        Some(file)
    };
    let [pc, quest, coarse] = header.lengths;
    let mut files = PreviewFiles { vrchat_pc: next(pc)?, vrchat_quest: next(quest)?, vrchat_coarse: next(coarse)? };
    if !rest.is_empty() {
        return None;
    }
    rekey(&mut files, key)?;
    Some((files, header.metadata))
}

pub fn load(result: &Path, fingerprint: &Fingerprint, key: u32) -> Option<(PreviewFiles, serde_json::Value)> {
    use std::io::Read;
    let file = std::fs::File::open(result.join(FILE)).ok()?;
    if file.metadata().ok()?.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    decode(&bytes, fingerprint, key)
}

pub fn save(result: &Path, fingerprint: &Fingerprint, files: &PreviewFiles, metadata: &serde_json::Value) -> anyhow::Result<()> {
    let parts = [&files.vrchat_pc, &files.vrchat_quest, &files.vrchat_coarse];
    let header = serde_json::to_vec(&Header { fingerprint: fingerprint.clone(), lengths: parts.map(|p| p.len()), metadata: metadata.clone() })?;
    let mut bytes = Vec::with_capacity(44 + header.len() + files.len() as usize);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&[0; 32]);
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    for part in parts {
        bytes.extend_from_slice(part);
    }
    anyhow::ensure!(bytes.len() as u64 <= MAX_BYTES, "upload preparation exceeds the cache limit");
    let digest = Sha256::digest(&bytes[40..]);
    bytes[8..40].copy_from_slice(&digest);
    orbiters_core::write_atomic(&result.join(FILE), &bytes)?;
    Ok(())
}

pub fn sky(result: &Path) -> Option<PathBuf> {
    Some(result.join(reconstruction::SKY_FILE)).filter(|p| p.is_file())
}

/// `orbiters-desktop --prepare-upload <session folder> [--gpu integrated]`: the background preparation's process.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let session = PathBuf::from(args.first().context("usage: orbiters-desktop --prepare-upload <session folder> [--gpu integrated]")?);
    let integrated = args.windows(2).any(|w| w[0] == "--gpu" && w[1] == "integrated");
    // Above normal while VRChat is closed, normal while it runs, and VRChat's frames pace the GPU's steps
    // (orbiters_reconstruct::gpu::Gpu::follow_vrchat), as the reconstruction worker.
    orbiters_reconstruct::priority::govern();
    let device = orbiters_reconstruct::gpu::DeviceOptions { low_priority: true, low_power: integrated, ..Default::default() };
    let start = Instant::now();
    crate::controller::prepare_upload(&session, device)?;
    eprintln!("prepared {} for sharing in {:.1} s", session.display(), start.elapsed().as_secs_f32());
    Ok(())
}

/// The background preparation: the sessions waiting, and the one being prepared.
#[derive(Default)]
struct Background {
    queue: VecDeque<PathBuf>,
    running: Option<Running>,
}

struct Running {
    dir: PathBuf,
    child: Arc<Mutex<Child>>,
    /// Set once the process has exited (and `running` cleared).
    done: Arc<(Mutex<bool>, Condvar)>,
    /// Stopped by [`pause`] or [`forget`]: not a failure.
    stopped: Arc<AtomicBool>,
}

static BACKGROUND: Mutex<Option<Background>> = Mutex::new(None);

fn background<R>(f: impl FnOnce(&mut Background) -> R) -> R {
    f(BACKGROUND.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Background::default))
}

fn stop(child: &Mutex<Child>) {
    let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
}

/// How long a share waits for its session's background preparation before stopping it and preparing at once.
const CLAIM_WAIT: Duration = Duration::from_secs(120);

/// Prepares `dir` in the background once nothing else runs (a reconstruction of it just finished).
pub fn queue(dir: PathBuf) {
    background(|b| {
        if !b.queue.contains(&dir) && b.running.as_ref().is_none_or(|r| r.dir != dir) {
            b.queue.push_back(dir);
        }
    });
}

/// `dir` is about to change (a reconstruction of it starts): a preparation of it would be out of date.
pub fn forget(dir: &Path) {
    let running = background(|b| {
        b.queue.retain(|d| d != dir);
        let r = b.running.as_ref().filter(|r| r.dir == dir)?;
        r.stopped.store(true, Ordering::SeqCst);
        Some(r.child.clone())
    });
    if let Some(child) = running {
        stop(&child);
    }
}

/// Stops the running preparation (a capture or a reconstruction needs the GPU); its session waits first in line.
pub fn pause() {
    let running = background(|b| {
        let r = b.running.as_ref()?;
        r.stopped.store(true, Ordering::SeqCst);
        let dir = r.dir.clone();
        let child = r.child.clone();
        b.queue.retain(|d| *d != dir);
        b.queue.push_front(dir);
        Some(child)
    });
    if let Some(child) = running {
        stop(&child);
    }
}

/// Starts the first waiting session's preparation when none runs, on the integrated GPU with `integrated`. `on_log`
/// gets its log lines; `on_exit` runs once it has exited (the service then looks for its next work).
pub fn start_next(integrated: bool, on_log: impl Fn(String) + Send + 'static, on_exit: impl FnOnce() + Send + 'static) -> anyhow::Result<bool> {
    use std::io::{BufRead, BufReader};
    let mut guard = BACKGROUND.lock().unwrap_or_else(|e| e.into_inner());
    let b = guard.get_or_insert_with(Background::default);
    if b.running.is_some() {
        return Ok(false);
    }
    let Some(dir) = b.queue.pop_front() else { return Ok(false) };
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("--prepare-upload").arg(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    if integrated {
        cmd.args(["--gpu", "integrated"]);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW; it picks its own priority class (orbiters_reconstruct::priority), as the reconstruction does.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn()?;
    crate::worker::keep_with_service(&child);
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let stopped = Arc::new(AtomicBool::new(false));
    b.running = Some(Running { dir: dir.clone(), child: child.clone(), done: done.clone(), stopped: stopped.clone() });
    drop(guard);
    if let Some(stderr) = stderr {
        std::thread::Builder::new().name("upload-prep-log".into()).spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                on_log(line);
            }
        })?;
    }
    std::thread::Builder::new().name("upload-prep".into()).spawn(move || {
        // Polled rather than waited on: pause and claim kill it through the same handle.
        let status = loop {
            match child.lock().unwrap_or_else(|e| e.into_inner()).try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        if !stopped.load(Ordering::SeqCst) && !status.is_some_and(|s| s.success()) {
            eprintln!("background upload preparation of {} failed ({status:?}): a share prepares it then", dir.display());
        }
        background(|b| b.running = None);
        let (flag, wake) = &*done;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
        on_exit();
    })?;
    Ok(true)
}

/// A share of `dir` is about to prepare it: it leaves the queue, and when its background preparation runs, the share
/// waits for it (its cache then answers at once), at most [`CLAIM_WAIT`] before stopping it.
pub fn claim(dir: &Path) {
    let running = background(|b| {
        b.queue.retain(|d| d != dir);
        b.running.as_ref().filter(|r| r.dir == dir).map(|r| (r.child.clone(), r.done.clone(), r.stopped.clone()))
    });
    let Some((child, done, stopped)) = running else { return };
    let (flag, wake) = &*done;
    let finished = flag.lock().unwrap_or_else(|e| e.into_inner());
    let (finished, _) = wake.wait_timeout_while(finished, CLAIM_WAIT, |finished| !*finished).unwrap_or_else(|e| e.into_inner());
    if !*finished {
        drop(finished);
        stopped.store(true, Ordering::SeqCst);
        stop(&child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue, a share claiming a waiting session, a capture pausing the running one (back first in line) and the
    /// exit handing over. The child is this test executable, which exits at once on `--prepare-upload`.
    #[test]
    fn background_preparation_waits_its_turn_and_gives_way() {
        let (a, b) = (PathBuf::from("session-a"), PathBuf::from("session-b"));
        let queued = || background(|bg| bg.queue.iter().cloned().collect::<Vec<_>>());
        queue(a.clone());
        queue(b.clone());
        queue(a.clone());
        assert_eq!(queued(), vec![a.clone(), b.clone()], "each session once");
        claim(&b);
        assert_eq!(queued(), vec![a.clone()], "a share prepares its session itself");
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(start_next(false, |_| {}, move || tx.send(()).unwrap()).unwrap());
        assert!(!start_next(false, |_| {}, || {}).unwrap(), "one at a time");
        assert!(queued().is_empty());
        pause();
        assert_eq!(queued(), vec![a.clone()], "paused: first in line again");
        rx.recv_timeout(Duration::from_secs(30)).expect("the process exits and says so");
        assert!(background(|bg| bg.running.is_none()));
        forget(&a);
        assert!(queued().is_empty());
        claim(&a);
    }

    #[test]
    fn cached_files_are_exact_and_all_trees_get_the_new_upload_key() {
        let dir = std::env::temp_dir().join(format!("orbiters-prep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut g = orbiters_reconstruct::gaussians::Gaussians::default();
        for i in 0..1024 {
            g.push([i as f32 * 0.01, 0.0, 1.0], [-3.0; 3], [1.0, 0.0, 0.0, 0.0], 1.0, [0.5; 3]);
        }
        let trees = orbiters_reconstruct::tree_tune::vrchat_files(&g, &[], None, 7, |_| {}).unwrap();
        let files = PreviewFiles { vrchat_pc: trees.pc, vrchat_quest: trees.quest, vrchat_coarse: trees.coarse };
        let fingerprint = Fingerprint(vec![Some("fixture".into())]);
        let metadata = serde_json::json!({"quality":33.0,"view":{"position":[1,2,3]}});
        save(&dir, &fingerprint, &files, &metadata).unwrap();
        let (loaded, found) = load(&dir, &fingerprint, 9).unwrap();
        assert_eq!(found, metadata);
        for (a, b) in [(&loaded.vrchat_pc, &files.vrchat_pc), (&loaded.vrchat_quest, &files.vrchat_quest), (&loaded.vrchat_coarse, &files.vrchat_coarse)] {
            assert_eq!(&a[..20], &b[..20]);
            assert_eq!(&a[20..24], &9u32.to_le_bytes());
            assert_eq!(&a[24..], &b[24..]);
        }
        assert!(load(&dir, &Fingerprint(vec![]), 9).is_none());
        let mut bytes = std::fs::read(dir.join(FILE)).unwrap();
        bytes[50] ^= 1;
        assert!(decode(&bytes, &fingerprint, 9).is_none());
        assert!(decode(&bytes[..30], &fingerprint, 9).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_source_is_content_hashed_even_when_the_length_stays_the_same() {
        let dir = std::env::temp_dir().join(format!("orbiters-prep-{}", uuid::Uuid::new_v4()));
        let result = dir.join("reconstruction");
        std::fs::create_dir_all(&result).unwrap();
        let sources = [
            result.join(reconstruction::PLY_FILE),
            dir.join(manifest::FRAMES_FILE),
            result.join(reconstruction::REPORT_FILE),
            dir.join(manifest::MANIFEST_FILE),
        ];
        for path in &sources {
            std::fs::write(path, b"old").unwrap();
        }
        let original = Fingerprint::read(&dir, &result).unwrap();
        for path in &sources {
            std::fs::write(path, b"new").unwrap();
            assert_ne!(Fingerprint::read(&dir, &result).unwrap(), original);
            std::fs::write(path, b"old").unwrap();
        }
        std::fs::remove_file(&sources[2]).unwrap();
        assert_ne!(Fingerprint::read(&dir, &result).unwrap(), original);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
