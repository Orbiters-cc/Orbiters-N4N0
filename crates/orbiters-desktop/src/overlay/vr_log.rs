//! `vr-test.log` in the logs' folder: what a headset test needs to settle what no test here can see. What was submitted, what each stick said, what a press did. A line a second at most for
//! each thing, and only while that thing is going on. English only: it is read by whoever fixes the code.
//!
//! In the SteamVR overlay's process the overlay's logs (`vr-test.log`, `overlay-stalls.log`, `translate.log`,
//! `camera-tiles.log`) are written by one writer thread ([`Queued`]): a line is stamped with its time where it is said and
//! queued, so the loop that reads the controllers and places the menus makes no file-system call (LIGHT-VR-7). The writer
//! does what a line did before (create the folder, look at the size, rotate, open, append, close), for each batch of
//! lines it finds queued. Every other process (the service, the app) writes at once, as before.

use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `vr-test.log`'s and `translate.log`'s size before it becomes `<name>.old.log`.
pub const ROTATE_AT: u64 = 1 << 20;
/// `camera-tiles.log`'s (it had no limit: 21.5 MB on Enzo's disk, LIGHT-VR-8; two looks a second while the camera is looked for).
pub const CAMERA_ROTATE_AT: u64 = 4 << 20;

/// A line now. Never from tests: their scanners, pages and overlays would write into the person's own `vr-test.log`
/// (found 2026-11-15: a desktop test run's lines among Enzo's).
pub fn line(text: &str) {
    // Every line is also a clue for a menu mismatch report, while those are on.
    crate::mismatch::note("vr-test.log", || text.to_owned());
    if cfg!(test) {
        let _ = text;
        return;
    }
    append("vr-test.log", Some(ROTATE_AT), format!("{} {text}\n", orbiters_core::now_utc_iso()));
}

/// `text` (a whole line with its time and its end of line) at the end of `file` in the logs' folder: through the writer
/// when this process has one, else at once. Above `rotate_at` the file becomes `<stem>.old.log` first.
pub fn append(file: &'static str, rotate_at: Option<u64>, text: String) {
    let item = Item { file, rotate_at, text };
    let sender = WRITER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(sender) = sender {
        match sender.try_send(Msg::Line(item)) {
            Ok(()) => return,
            Err(TrySendError::Full(Msg::Line(item))) => {
                // Never wait on the loop: the line is counted and said lost, in its own file, with the next batch.
                lose(&LOST, item.file, item.rotate_at);
                return;
            }
            Err(TrySendError::Full(_)) => return,
            // The writer is gone (the process is ending): at once, as without one.
            Err(TrySendError::Disconnected(Msg::Line(item))) => return write_batch(&orbiters_core::paths::logs_dir(), &[item]),
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
    write_batch(&orbiters_core::paths::logs_dir(), &[item]);
}

/// The overlay's writer, while it runs.
static WRITER: Mutex<Option<SyncSender<Msg>>> = Mutex::new(None);
/// Lines not queued because the queue was full, since the last said: by file (with its limit), how many.
type Lost = Mutex<Vec<(&'static str, Option<u64>, u64)>>;
static LOST: Lost = Mutex::new(Vec::new());

fn lose(lost: &Lost, file: &'static str, rotate_at: Option<u64>) {
    let mut lost = lost.lock().unwrap_or_else(|e| e.into_inner());
    match lost.iter_mut().find(|(f, _, _)| *f == file) {
        Some((_, _, n)) => *n += 1,
        None => lost.push((file, rotate_at, 1)),
    }
}
/// Lines the queue holds at most (a few minutes of the busiest logging).
const QUEUE: usize = 4096;
/// How long an end waits for the queue to be written.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

struct Item {
    file: &'static str,
    rotate_at: Option<u64>,
    text: String,
}

enum Msg {
    Line(Item),
    Flush(SyncSender<()>),
}

/// This process's writer thread, from [`Queued::start`] until it is dropped (on return and on a panic's unwinding), which
/// writes what is still queued first.
pub struct Queued {
    sender: SyncSender<Msg>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// This one is the process's (`start`), not a test's.
    installed: bool,
}

impl Queued {
    /// The overlay's logs go through a writer thread from now on, into the logs' folder.
    pub fn start() -> Queued {
        let mut queued = Queued::start_in(orbiters_core::paths::logs_dir(), &LOST);
        queued.installed = true;
        *WRITER.lock().unwrap_or_else(|e| e.into_inner()) = Some(queued.sender.clone());
        queued
    }

    fn start_in(dir: std::path::PathBuf, lost: &'static Lost) -> Queued {
        let (sender, receiver) = std::sync::mpsc::sync_channel(QUEUE);
        let thread = std::thread::Builder::new().name("overlay logs".into()).spawn(move || writer(&dir, &receiver, lost)).ok(); // i18n: not shown (a thread's name)
        Queued { sender, thread, installed: false }
    }

    /// Waits (at most about [`FLUSH_WAIT`], also when the writer is stuck on a disk) until everything queued before is
    /// written.
    pub fn flush(&self) {
        let until = Instant::now() + FLUSH_WAIT;
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        let mut ask = Msg::Flush(done);
        loop {
            match self.sender.try_send(ask) {
                Ok(()) => break,
                Err(TrySendError::Full(again)) if Instant::now() < until => {
                    ask = again;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
        let _ = wait.recv_timeout(until.saturating_duration_since(Instant::now()));
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        if self.installed {
            // Lines said from now on are written at once.
            *WRITER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        self.flush();
        drop(self.thread.take());
    }
}

fn writer(dir: &Path, receiver: &Receiver<Msg>, lost: &Lost) {
    while let Ok(first) = receiver.recv() {
        let mut batch = Vec::new();
        let mut flushes = Vec::new();
        let mut next = Some(first);
        while let Some(msg) = next {
            match msg {
                Msg::Line(item) => batch.push(item),
                Msg::Flush(done) => flushes.push(done),
            }
            next = receiver.try_recv().ok();
        }
        let dropped = std::mem::take(&mut *lost.lock().unwrap_or_else(|e| e.into_inner()));
        for (file, rotate_at, n) in dropped {
            let text = format!("{} {n} lines lost: the log's queue was full\n", orbiters_core::now_utc_iso()); // i18n: not shown (a log's line)
            batch.push(Item { file, rotate_at, text });
        }
        // In the order said, one open per run of lines for the same file.
        let mut start = 0;
        while start < batch.len() {
            let end = batch[start..].iter().position(|i| i.file != batch[start].file).map_or(batch.len(), |n| start + n);
            write_batch(dir, &batch[start..end]);
            start = end;
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

/// Lines for one file, as a line was always written: folder made, size looked at (rotated above its limit), opened for
/// appending, written, closed.
fn write_batch(dir: &Path, items: &[Item]) {
    let Some(first) = items.first() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(first.file);
    if let Some(limit) = first.rotate_at {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > limit) {
            let old = path.with_extension("old.log");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&path, old);
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        use std::io::Write;
        let text: String = items.iter().map(|i| i.text.as_str()).collect();
        let _ = file.write_all(text.as_bytes());
    }
}

/// Says whether it is time for another line.
#[derive(Clone, Copy, Debug, Default)]
pub struct Every {
    last: Option<Instant>,
}

impl Every {
    pub fn due(&mut self, every: Duration) -> bool {
        if self.last.is_some_and(|at| at.elapsed() < every) {
            return false;
        }
        self.last = Some(Instant::now());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-vr-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn item(file: &'static str, rotate_at: Option<u64>, text: &str) -> Item {
        Item { file, rotate_at, text: format!("{text}\n") }
    }

    /// The writer thread writes the same lines in the same order into the same files as writing at once did.
    #[test]
    fn queued_lines_reach_their_files_in_order() {
        static LOST_HERE: Lost = Mutex::new(Vec::new());
        let dir = scratch("order");
        let queued = Queued::start_in(dir.clone(), &LOST_HERE);
        for n in 0..300 {
            let file = if n % 3 == 0 { "b.log" } else { "a.log" };
            queued.sender.send(Msg::Line(item(file, None, &format!("line {n}")))).unwrap();
        }
        queued.flush();
        let a = std::fs::read_to_string(dir.join("a.log")).unwrap();
        let b = std::fs::read_to_string(dir.join("b.log")).unwrap();
        let want_a: String = (0..300).filter(|n| n % 3 != 0).map(|n| format!("line {n}\n")).collect();
        let want_b: String = (0..300).filter(|n| n % 3 == 0).map(|n| format!("line {n}\n")).collect();
        assert_eq!(a, want_a);
        assert_eq!(b, want_b);
        // Dropping writes what is left.
        queued.sender.send(Msg::Line(item("a.log", None, "last"))).unwrap();
        drop(queued);
        assert!(std::fs::read_to_string(dir.join("a.log")).unwrap().ends_with("last\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Above its limit a log becomes `<stem>.old.log` (the old one replaced), as `vr-test.log` and `translate.log` did, and
    /// now `camera-tiles.log` too (LIGHT-VR-8). A log without a limit only grows.
    #[test]
    fn a_log_above_its_limit_is_rotated_to_old() {
        let dir = scratch("rotate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("camera-tiles.log"), vec![b'x'; 200]).unwrap();
        std::fs::write(dir.join("camera-tiles.old.log"), "older").unwrap();
        write_batch(&dir, &[item("camera-tiles.log", Some(100), "fresh")]);
        assert_eq!(std::fs::read_to_string(dir.join("camera-tiles.log")).unwrap(), "fresh\n");
        assert_eq!(std::fs::read(dir.join("camera-tiles.old.log")).unwrap().len(), 200);
        write_batch(&dir, &[item("camera-tiles.log", Some(100), "more")]);
        assert_eq!(std::fs::read_to_string(dir.join("camera-tiles.log")).unwrap(), "fresh\nmore\n", "under the limit it appends");
        std::fs::write(dir.join("vr-test.log"), vec![b'x'; 200]).unwrap();
        std::fs::create_dir_all(dir.join("vr-test.old.log").join("in the way")).unwrap();
        write_batch(&dir, &[item("vr-test.log", Some(100), "kept")]);
        assert_eq!(std::fs::read(dir.join("vr-test.log")).unwrap().len(), 205, "a rotation that fails keeps the log and appends");
        std::fs::write(dir.join("overlay-stalls.log"), vec![b'x'; 200]).unwrap();
        write_batch(&dir, &[item("overlay-stalls.log", None, "stall")]);
        assert_eq!(std::fs::read(dir.join("overlay-stalls.log")).unwrap().len(), 206);
        assert!(!dir.join("overlay-stalls.old.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A full queue never blocks the caller; what was not queued is said in `vr-test.log` with the next batch.
    #[test]
    fn lines_lost_to_a_full_queue_are_counted_and_said() {
        static LOST_HERE: Lost = Mutex::new(Vec::new());
        let dir = scratch("lost");
        for _ in 0..7 {
            lose(&LOST_HERE, "vr-test.log", Some(ROTATE_AT));
        }
        lose(&LOST_HERE, "camera-tiles.log", Some(CAMERA_ROTATE_AT));
        let queued = Queued::start_in(dir.clone(), &LOST_HERE);
        queued.sender.send(Msg::Line(item("vr-test.log", Some(ROTATE_AT), "after"))).unwrap();
        queued.flush();
        let text = std::fs::read_to_string(dir.join("vr-test.log")).unwrap();
        assert!(text.starts_with("after\n"), "{text}");
        assert!(text.contains(" 7 lines lost"), "{text}");
        let camera = std::fs::read_to_string(dir.join("camera-tiles.log")).unwrap();
        assert!(camera.contains(" 1 lines lost"), "each file says its own losses: {camera}");
        assert!(LOST_HERE.lock().unwrap().is_empty());
        drop(queued);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
