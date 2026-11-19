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
//! that session's own preparation rather than starting a second one ([`claim`]). Being stopped costs the stage it was in:
//! each stage finished goes into a file beside the cache ([`PartialFile`], orbiters_reconstruct::tree_tune::Keep), and
//! the next preparation of the same reconstruction, in the background or for a share, goes on after it.

use orbiters_core::{contracts::manifest, maps::PreviewFiles, reconstruction, visited::sha256_file};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// Change the format/policy marker whenever preparation changes its outputs or tuning policy. 03: splat trees of version 2
// (20-byte records, OrbitersContracts Docs/splat-tree.md) tuned as the file stores them; caches of version 1 trees
// prepare again (the file keeps its name, so they are replaced rather than left beside the new one). 04: floaters far
// from the world stay out of the trees (orbiters_reconstruct::tree::MAX_REACH): files prepared with them are refused.
const MAGIC: &[u8; 8] = b"N4PREP04";
const FILE: &str = "upload-prep-v1.bin";
/// What a preparation that was stopped has finished so far (orbiters_reconstruct::tree_tune::Stages), beside [`FILE`].
const PARTIAL_FILE: &str = "upload-prep-partial.bin";
const PARTIAL_MAGIC: &[u8; 8] = b"N4PREPP1";
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
        let mut hashes: Vec<Option<String>> = paths
            .iter()
            .enumerate()
            .map(|(i, path)| match sha256_file(path) {
                Ok(hash) => Ok(Some(hash)),
                Err(e) if i >= 2 && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            })
            .collect::<Result<_, _>>()?;
        // The splat edits too, as what they leave out (orbiters_reconstruct::edit::generation): trees prepared before a
        // splat was hidden never answer after, and the picture's flag (drawn again or not) changes nothing. Edits that
        // cannot be read have no fingerprint: nothing prepared answers for them.
        hashes.push(Some(format!("edits:{}", orbiters_reconstruct::edit::generation(result)?)));
        Ok(Self(hashes))
    }

    /// The generation of the splat edits it holds.
    pub fn generation(&self) -> Option<&str> {
        self.0.last()?.as_deref()?.strip_prefix("edits:")
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
        // Only trees of the version this build writes answer (the magic above changes with it; this holds them to it).
        if bytes.get(..8)? != orbiters_reconstruct::tree::MAGIC || bytes.get(8..12)? != orbiters_reconstruct::tree::VERSION.to_le_bytes() {
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
    anyhow::ensure!(bytes.len() as u64 <= MAX_BYTES, "{}", orbiters_i18n::tr("upload preparation exceeds the cache limit"));
    let digest = Sha256::digest(&bytes[40..]);
    bytes[8..40].copy_from_slice(&digest);
    orbiters_core::write_atomic(&result.join(FILE), &bytes)?;
    forget_partial(result);
    Ok(())
}

/// How far a stopped preparation of the result in a folder got, for the same result only.
pub struct PartialFile {
    path: PathBuf,
    fingerprint: Fingerprint,
}

impl PartialFile {
    pub fn new(result: &Path, fingerprint: &Fingerprint) -> Self {
        Self { path: result.join(PARTIAL_FILE), fingerprint: fingerprint.clone() }
    }

    /// `bytes` of its file, when they are whole and of this result.
    fn open<'a>(&self, bytes: &'a [u8]) -> Option<&'a [u8]> {
        if bytes.get(..8)? != PARTIAL_MAGIC {
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
        let fingerprint: Fingerprint = serde_json::from_slice(payload.get(4..4 + length)?).ok()?;
        (fingerprint == self.fingerprint).then(|| payload.get(4 + length..)).flatten()
    }
}

impl orbiters_reconstruct::tree_tune::Keep for PartialFile {
    fn load(&mut self) -> Option<orbiters_reconstruct::tree_tune::Partial> {
        let bytes = std::fs::read(&self.path).ok()?;
        let partial = self.open(&bytes).and_then(orbiters_reconstruct::tree_tune::Partial::from_bytes);
        if partial.is_none() {
            eprintln!("{} is of another result, or damaged: preparing from the start", self.path.display());
        }
        partial
    }

    fn save(&mut self, bytes: Vec<u8>) {
        let Ok(header) = serde_json::to_vec(&self.fingerprint) else { return };
        let mut out = Vec::with_capacity(44 + header.len() + bytes.len());
        out.extend_from_slice(PARTIAL_MAGIC);
        out.extend_from_slice(&[0; 32]);
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&bytes);
        let digest = Sha256::digest(&out[40..]);
        out[8..40].copy_from_slice(&digest);
        if let Err(e) = orbiters_core::write_atomic(&self.path, &out) {
            eprintln!("could not keep how far the upload preparation got: {e}");
        }
    }
}

/// How far a stopped preparation got goes, and any write a kill cut short.
fn forget_partial(result: &Path) {
    let Ok(entries) = std::fs::read_dir(result) else { return };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with(PARTIAL_FILE) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Whether a preparation of this session stopped halfway: its session waits in line again when the app opens.
pub fn has_partial(session: &Path) -> bool {
    reconstruction::result_dir(session).join(PARTIAL_FILE).is_file()
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
    // A start that fails before the child runs keeps the session first in line (CHALLENGER-C5): the next look tries
    // again, nothing is lost.
    let spawned = (|| -> anyhow::Result<std::process::Child> {
        fail_point(0)?;
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
        Ok(cmd.spawn()?)
    })();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            b.queue.push_front(dir);
            return Err(e);
        }
    };
    crate::worker::keep_with_service(&child);
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new((Mutex::new(false), Condvar::new()));
    let stopped = Arc::new(AtomicBool::new(false));
    b.running = Some(Running { dir: dir.clone(), child: child.clone(), done: done.clone(), stopped: stopped.clone() });
    drop(guard);
    // A helper thread that cannot start leaves no running child behind and no slot taken (CHALLENGER-C5): the child is
    // stopped and reaped, the slot freed, whoever waits on it woken, and the session goes back first in line.
    let abandon = |e: anyhow::Error, child: &Arc<Mutex<std::process::Child>>, done: &Arc<(Mutex<bool>, Condvar)>| {
        stop(child);
        let _ = child.lock().unwrap_or_else(|e| e.into_inner()).wait();
        background(|b| {
            b.running = None;
            b.queue.retain(|d| *d != dir);
            b.queue.push_front(dir.clone());
        });
        let (flag, wake) = &**done;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
        e
    };
    if let Some(stderr) = stderr {
        let logging = fail_point(1).and_then(|()| {
            std::thread::Builder::new()
                .name("upload-prep-log".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        on_log(line);
                    }
                })
                .map_err(anyhow::Error::from)
        });
        if let Err(e) = logging {
            return Err(abandon(e, &child, &done));
        }
    }
    if let Err(e) = fail_point(2) {
        return Err(abandon(e, &child, &done));
    }
    let (child_w, done_w, dir_w) = (child.clone(), done.clone(), dir.clone());
    let waiter = std::thread::Builder::new().name("upload-prep".into()).spawn(move || {
        let (child, done, dir) = (child_w, done_w, dir_w);
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
    });
    if let Err(e) = waiter {
        return Err(abandon(e.into(), &child, &done));
    }
    Ok(true)
}

/// Tests make a start fail at a chosen point (0: before the child, 1: its log thread, 2: its waiter).
#[cfg(test)]
static FAIL_AT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(u8::MAX);

fn fail_point(_at: u8) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAIL_AT.load(Ordering::SeqCst) == _at {
        anyhow::bail!("failure injected at {_at}"); // i18n: not shown
    }
    Ok(())
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
        // CHALLENGER-C5: a start failing before its child or at either helper thread keeps the session first in line and
        // leaves no slot taken; the next start runs it.
        for at in 0..3u8 {
            queue(a.clone());
            FAIL_AT.store(at, Ordering::SeqCst);
            assert!(start_next(false, |_| {}, || {}).is_err(), "{at}");
            FAIL_AT.store(u8::MAX, Ordering::SeqCst);
            assert_eq!(queued(), vec![a.clone()], "{at}: still first in line");
            assert!(background(|bg| bg.running.is_none()), "{at}: no slot left taken");
        }
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(start_next(false, |_| {}, move || tx.send(()).unwrap()).unwrap());
        rx.recv_timeout(Duration::from_secs(30)).expect("the process exits and says so");
        forget(&a);
        assert!(queued().is_empty());
    }

    /// A stopped preparation goes on only on the very result it was made from, and its file goes once it is done.
    #[test]
    fn how_far_a_preparation_got_is_kept_for_its_own_result_only() {
        use orbiters_reconstruct::tree_tune::Keep;
        let dir = std::env::temp_dir().join(format!("orbiters-prep-{}", uuid::Uuid::new_v4()));
        let result = dir.join(reconstruction::DIR);
        std::fs::create_dir_all(&result).unwrap();
        let fingerprint = Fingerprint(vec![Some("fixture".into())]);
        let mut kept = PartialFile::new(&result, &fingerprint);
        assert!(kept.load().is_none(), "nothing yet");
        // The smallest export there is: stage 3, no splats, no boxes, no Quest tree.
        let mut bytes = 3u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0; 4 * 5]);
        kept.save(bytes.clone());
        assert!(has_partial(&dir));
        assert_eq!(kept.load().map(|p| p.done), Some(3));
        assert!(PartialFile::new(&result, &Fingerprint(vec![Some("another".into())])).load().is_none());
        let mut damaged = std::fs::read(result.join(PARTIAL_FILE)).unwrap();
        *damaged.last_mut().unwrap() ^= 1;
        std::fs::write(result.join(PARTIAL_FILE), &damaged).unwrap();
        assert!(kept.load().is_none());
        kept.save(bytes);
        let files = PreviewFiles { vrchat_pc: tree_fixture(), vrchat_quest: tree_fixture(), vrchat_coarse: tree_fixture() };
        save(&result, &fingerprint, &files, &serde_json::json!({})).unwrap();
        assert!(!has_partial(&dir), "prepared: gone");
        std::fs::remove_dir_all(&dir).unwrap();
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
        // Trees of version 1 in a cache (written before the 20-byte records, under the same magic) never answer.
        let mut quest = files.vrchat_quest.clone();
        quest[8..12].copy_from_slice(&1u32.to_le_bytes());
        let old = PreviewFiles { vrchat_pc: files.vrchat_pc.clone(), vrchat_quest: quest, vrchat_coarse: files.vrchat_coarse.clone() };
        save(&dir, &fingerprint, &old, &metadata).unwrap();
        assert!(load(&dir, &fingerprint, 9).is_none(), "a version 1 tree is prepared again");
        save(&dir, &fingerprint, &files, &metadata).unwrap();
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

    /// Trees prepared before splats were hidden or deleted in the 3D view never answer after (nor after the edit is
    /// changed or undone): the edit file is one of the sources.
    #[test]
    fn a_splat_edit_makes_prepared_trees_stale() {
        let dir = std::env::temp_dir().join(format!("orbiters-prep-{}", uuid::Uuid::new_v4()));
        let result = dir.join("reconstruction");
        std::fs::create_dir_all(&result).unwrap();
        std::fs::write(result.join(reconstruction::PLY_FILE), b"ply").unwrap();
        std::fs::write(dir.join(manifest::FRAMES_FILE), b"frames").unwrap();
        let unedited = Fingerprint::read(&dir, &result).unwrap();
        assert_eq!(unedited.generation(), Some(orbiters_reconstruct::edit::UNEDITED));
        use orbiters_reconstruct::edit::{DELETED, Edit, HIDDEN, Identity};
        let mut edit = Edit::new(10, Identity { bytes: 1 << 20, hash: 5 });
        edit.marks[1] = DELETED;
        edit.save(&result).unwrap();
        let edited = Fingerprint::read(&dir, &result).unwrap();
        assert_ne!(edited, unedited);
        assert_eq!(edited.generation(), Some(edit.generation().as_str()));
        let files = PreviewFiles { vrchat_pc: tree_fixture(), vrchat_quest: tree_fixture(), vrchat_coarse: tree_fixture() };
        save(&result, &unedited, &files, &serde_json::json!({})).unwrap();
        assert!(load(&result, &edited, 1).is_none(), "prepared before the edit: not used");
        // The picture drawn again (its flag) or hidden instead of deleted: the same outputs, the same fingerprint.
        edit.picture = false;
        edit.marks[1] = HIDDEN;
        edit.save(&result).unwrap();
        assert_eq!(Fingerprint::read(&dir, &result).unwrap(), edited);
        edit.marks[2] = DELETED;
        edit.save(&result).unwrap();
        assert_ne!(Fingerprint::read(&dir, &result).unwrap(), edited);
        // Damaged: no fingerprint, so nothing prepared answers.
        std::fs::write(result.join(reconstruction::EDIT_FILE), b"damaged").unwrap();
        assert!(Fingerprint::read(&dir, &result).is_err());
        std::fs::remove_file(result.join(reconstruction::EDIT_FILE)).unwrap();
        assert_eq!(Fingerprint::read(&dir, &result).unwrap(), unedited, "undone: the trees as trained answer again");
        assert!(load(&result, &unedited, 1).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn tree_fixture() -> Vec<u8> {
        let mut bytes = orbiters_reconstruct::tree::MAGIC.to_vec();
        bytes.extend_from_slice(&orbiters_reconstruct::tree::VERSION.to_le_bytes());
        bytes.resize(32, 0);
        bytes
    }
}
