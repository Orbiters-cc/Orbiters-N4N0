//! VRChat on a Mac, when it hangs or will not quit (docs/vrchat-bridge.md, "Incidents"): the engine's faults that nobody can
//! cause on demand are kept as they happen. While N4N0 runs it follows whatever VRChat session the engine runs (Home's,
//! the bridge's, one started from a terminal): its record's frames, the engine's log and VRChat's own log. When all three
//! stop together while the session lives (a stall), when VRChat said it quits and the session is still there long after
//! (a quit that does not finish), or when the engine's log says the game's graphics went, it captures once per incident,
//! without touching the game: the session, copies of both logs before the next start replaces them, the engine's process
//! tree with CPU, two short stack samples (`/usr/bin/sample`) of the game's process, the render server and the window
//! server (and of the busiest process when it is another), memory pressure, and for a quit the times the engine's launch
//! script says its two waits returned. Into a dated folder under the logs' folder (`incidents/`), a few kept, bounded in
//! size, credentials taken out of every line; Settings' Log says where.
//!
//! The detection ([`Detector`]) and what a capture keeps ([`redact`], [`retain`], [`tree`]) are pure and tested on every
//! computer; the following and the capture run on a Mac only.
use std::path::{Path, PathBuf};

/// Nothing grew for this long while the session runs: a stall (the game draws at least every few seconds, its window
/// server says its presents every 60 frames or so, VRChat logs every few seconds in a world).
pub(crate) const STALL_AFTER: f64 = 30.0;
/// VRChat said it quits and its session still runs this long after: a quit that does not finish (clean quits on this
/// Mac ended within 4 s of `HandleApplicationQuit`).
pub(crate) const QUIT_AFTER: f64 = 45.0;
/// At most this many captures of one session.
pub(crate) const PER_SESSION: u32 = 3;
/// How many incident folders are kept, and how much they may weigh together.
pub(crate) const KEPT: usize = 6;
pub(crate) const KEPT_BYTES: u64 = 200 << 20;
/// How much of each log is copied (its end).
const LOG_COPY: u64 = 24 << 20;
/// How often the session is looked at.
const EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// How much of a log is read per look for its markers; more is passed over (its end is what counts).
const SCAN_AT_MOST: u64 = 2 << 20;

/// What kind of incident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Frames and both logs stopped while the session lives.
    Stall,
    /// VRChat said it quits and the session did not end.
    StuckQuit,
    /// The engine's log said the game's graphics server dropped it.
    RendererLost,
}

impl Kind {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Kind::Stall => "stall",
            Kind::StuckQuit => "stuck-quit",
            Kind::RendererLost => "renderer-lost",
        }
    }
}

/// One look at a running session.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Look {
    /// Seconds on a monotonic clock.
    pub at: f64,
    /// The session runs (its record says so and its game's process lives).
    pub running: bool,
    /// The frames its record says it has shown, and the lengths of the engine's and VRChat's logs.
    pub shown: Option<u64>,
    pub engine_len: Option<u64>,
    pub vrchat_len: Option<u64>,
    /// VRChat's log said it quits (`HandleApplicationQuit`).
    pub quitting: bool,
    /// The engine's log said the graphics went.
    pub renderer_lost: bool,
}

/// Tells incidents from the looks at one session, each once.
#[derive(Debug, Default)]
pub(crate) struct Detector {
    last: Option<(Option<u64>, Option<u64>, Option<u64>)>,
    progress_at: Option<f64>,
    stall_told: bool,
    quit_at: Option<f64>,
    quit_told: bool,
    lost_told: bool,
    captures: u32,
}

impl Detector {
    /// What this look shows that was not told yet.
    pub(crate) fn look(&mut self, look: &Look) -> Option<Kind> {
        let now = (look.shown, look.engine_len, look.vrchat_len);
        // Progress: any of them grew, or a log was replaced (shorter): the game still did something.
        let moved = match self.last {
            None => false,
            Some((shown, engine, vrchat)) => {
                let grew = |before: Option<u64>, after: Option<u64>| matches!((before, after), (Some(b), Some(a)) if a != b) || (before.is_none() && after.is_some());
                grew(shown, now.0) || grew(engine, now.1) || grew(vrchat, now.2)
            }
        };
        self.last = Some(now);
        if moved {
            self.progress_at = Some(look.at);
            self.stall_told = false;
        }
        if !look.running || self.captures >= PER_SESSION {
            return None;
        }
        if look.renderer_lost && !self.lost_told {
            self.lost_told = true;
            return self.told(Kind::RendererLost);
        }
        if look.quitting {
            let since = *self.quit_at.get_or_insert(look.at);
            if look.at - since >= QUIT_AFTER && !self.quit_told {
                self.quit_told = true;
                return self.told(Kind::StuckQuit);
            }
            // A game quitting stops drawing: that is no stall.
            return None;
        }
        match self.progress_at {
            Some(at) if look.at - at >= STALL_AFTER && !self.stall_told => {
                self.stall_told = true;
                self.told(Kind::Stall)
            }
            _ => None,
        }
    }

    fn told(&mut self, kind: Kind) -> Option<Kind> {
        self.captures += 1;
        Some(kind)
    }
}

/// A line with what could be a credential taken out: whatever follows a word that names one.
pub(crate) fn redact(line: &str) -> String {
    const WORDS: [&str; 12] = ["password", "passwd", "token", "secret", "cookie", "authorization", "auth=", "authcookie", "apikey", "api_key", "session_key", "ticket"]; // i18n: not shown (words looked for in logs)
    let lower = line.to_ascii_lowercase();
    match WORDS.iter().filter_map(|w| lower.find(w).map(|i| i + w.len())).min() {
        // The byte positions of the lowercase copy are the line's own (ASCII lowercasing keeps every length).
        Some(end) if line.is_char_boundary(end) => format!("{}[removed]", &line[..end]),
        Some(_) => "[removed]".into(),
        None => line.to_string(),
    }
}

/// The incident folders of `root` to remove so that at most [`KEPT`] stay and they weigh at most [`KEPT_BYTES`]
/// together: the oldest first (`folders` are (name, bytes); names begin with their date, so they sort by age).
pub(crate) fn retain(mut folders: Vec<(String, u64)>) -> Vec<String> {
    folders.sort();
    let mut removed = Vec::new();
    let mut total: u64 = folders.iter().map(|f| f.1).sum();
    let mut left = folders.len();
    for (name, bytes) in folders {
        if left <= KEPT && total <= KEPT_BYTES {
            break;
        }
        // The newest is always kept, whatever it weighs.
        if left == 1 {
            break;
        }
        removed.push(name);
        total -= bytes;
        left -= 1;
    }
    removed
}

/// One process as `ps` lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub cpu: f32,
    pub rss_kb: u64,
    pub state: String,
    pub command: String,
}

/// Reads `ps -axww -o pid=,ppid=,pcpu=,rss=,state=,command=`.
pub(crate) fn parse_ps(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let cpu = parts.next()?.parse().ok()?;
            let rss_kb = parts.next()?.parse().ok()?;
            let state = parts.next()?.to_string();
            let command = parts.collect::<Vec<_>>().join(" ");
            Some(Proc { pid, ppid, cpu, rss_kb, state, command })
        })
        .collect()
}

/// The processes of the engine's game launched as `launcher`: the launcher and every process under it, and any other
/// process that runs from the engine's `library` (a helper re-parented after its parent went).
pub(crate) fn tree(all: &[Proc], launcher: u32, library: &str) -> Vec<Proc> {
    let mut kept: Vec<u32> = vec![launcher];
    loop {
        let more: Vec<u32> = all.iter().filter(|p| kept.contains(&p.ppid) && !kept.contains(&p.pid)).map(|p| p.pid).collect();
        if more.is_empty() {
            break;
        }
        kept.extend(more);
    }
    all.iter().filter(|p| kept.contains(&p.pid) || (!library.is_empty() && p.command.contains(library))).cloned().collect()
}

/// The processes to sample in `tree`: the game's own (the largest in memory among the engine's Wine processes, VRChat
/// holding gigabytes), the render server and the window server (the busiest of each name), and the busiest of all when it
/// is none of those. (name, pid), at most once each.
pub(crate) fn sample_targets(tree: &[Proc]) -> Vec<(&'static str, u32)> {
    let busiest = |f: &dyn Fn(&Proc) -> bool| tree.iter().filter(|p| f(p)).max_by(|a, b| a.cpu.total_cmp(&b.cpu)).map(|p| p.pid);
    let program = |p: &Proc| p.command.split_whitespace().next().unwrap_or_default().rsplit('/').next().unwrap_or_default().to_string();
    let game = tree.iter().filter(|p| !matches!(program(p).as_str(), "wsd" | "virgl_render_server" | "virgl_test_server" | "obelisk-input" | "sh" | "bash")).max_by_key(|p| p.rss_kb).map(|p| p.pid);
    let mut targets: Vec<(&'static str, u32)> = Vec::new();
    let mut add = |name: &'static str, pid: Option<u32>| {
        if let Some(pid) = pid.filter(|pid| !targets.iter().any(|(_, p)| p == pid)) {
            targets.push((name, pid));
        }
    };
    add("game", game);
    add("render-server", busiest(&|p| program(p) == "virgl_render_server"));
    add("wsd", busiest(&|p| program(p) == "wsd"));
    add("busiest", busiest(&|_| true));
    targets
}

/// The times the engine's launch script says its waits returned (`obelisk-wait: …` lines, obelisk-cli 0.1.6), from a log.
pub(crate) fn wait_marks(log: &str) -> Vec<String> {
    log.lines().filter(|l| l.contains("obelisk-wait:")).map(|l| l.trim().to_string()).take(16).collect() // i18n: not shown (the engine's log lines)
}

/// The folder an incident of `kind` of session `session` seen at `at` (local time) goes to, under `logs`.
pub(crate) fn folder(logs: &Path, at: time::OffsetDateTime, kind: Kind, session: &str) -> PathBuf {
    let stamp = format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", at.year(), at.month() as u8, at.day(), at.hour(), at.minute(), at.second());
    let short: String = session.chars().filter(|c| c.is_ascii_hexdigit() || *c == '-').take(8).collect();
    logs.join("incidents").join(format!("{stamp}-{}-{short}", kind.word()))
}

/// Copies the end of `from` (at most `most` bytes, from a whole line) into `to`, every line [`redact`]ed.
pub(crate) fn copy_tail(from: &Path, to: &Path, most: u64) -> std::io::Result<u64> {
    use std::io::{Read as _, Seek as _, Write as _};
    let mut file = std::fs::File::open(from)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(most);
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(most).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 { text.split_once('\n').map_or("", |(_, rest)| rest) } else { &text };
    let mut out = std::io::BufWriter::new(std::fs::File::create(to)?);
    if start > 0 {
        writeln!(out, "[the first {start} bytes of {} are left out]", from.display())?; // i18n: not shown (a log copy)
    }
    for line in text.lines() {
        writeln!(out, "{}", redact(line))?;
    }
    out.flush()?;
    Ok(len)
}

/// The total size of the files in `dir` (not below).
fn weight(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|d| d.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Removes the incident folders of `logs` that [`retain`] says go.
pub(crate) fn prune(logs: &Path) {
    let root = logs.join("incidents");
    let folders: Vec<(String, u64)> = std::fs::read_dir(&root).map(|d| d.flatten().filter(|e| e.path().is_dir()).map(|e| (e.file_name().to_string_lossy().into_owned(), weight(&e.path()))).collect()).unwrap_or_default();
    for name in retain(folders) {
        let _ = std::fs::remove_dir_all(root.join(name));
    }
}

/// The session followed: what is needed to look at it, and to capture.
#[derive(Clone, Debug)]
pub(crate) struct Followed {
    pub session: String,
    pub launcher: u32,
    pub library: PathBuf,
    pub engine_log: PathBuf,
    pub vrchat_log: Option<PathBuf>,
}

/// Captures an incident of `kind` of `followed` into a new folder of `logs`: where it is.
pub(crate) fn capture(logs: &Path, kind: Kind, followed: &Followed, look: &Look, detail: &str) -> std::io::Result<PathBuf> {
    let at = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let dir = folder(logs, at, kind, &followed.session);
    std::fs::create_dir_all(&dir)?;
    let run = |program: &str, args: &[&str]| -> String {
        run_bounded(program, args, std::time::Duration::from_secs(15)).map(|t| t.lines().map(redact).collect::<Vec<_>>().join("\n")).unwrap_or_else(|e| format!("{program}: {e}"))
    };
    // The processes first: the moment the incident was seen.
    let ps = run("/bin/ps", &["-axww", "-o", "pid=,ppid=,pcpu=,rss=,state=,command="]);
    let all = parse_ps(&ps);
    let library = followed.library.to_string_lossy().into_owned();
    let tree = tree(&all, followed.launcher, &library);
    let hot = tree.iter().max_by(|a, b| a.cpu.total_cmp(&b.cpu)).cloned();
    let mut listed = String::from("pid ppid %cpu rss_kb state command\n"); // i18n: not shown (a file for whoever looks into it)
    for p in &tree {
        listed.push_str(&format!("{} {} {:.1} {} {} {}\n", p.pid, p.ppid, p.cpu, p.rss_kb, p.state, redact(&p.command)));
    }
    std::fs::write(dir.join("processes.txt"), listed)?;
    let memory = format!(
        "kern.memorystatus_vm_pressure_level: {}\n\n{}\n\n{}\n",
        run("/usr/sbin/sysctl", &["-n", "kern.memorystatus_vm_pressure_level"]).trim(),
        run("/usr/bin/memory_pressure", &["-Q"]),
        run("/usr/bin/vm_stat", &[])
    );
    std::fs::write(dir.join("memory.txt"), memory)?;
    // The logs, before the next start replaces them.
    let engine_len = copy_tail(&followed.engine_log, &dir.join("engine.log"), LOG_COPY).ok();
    let vrchat_len = followed.vrchat_log.as_ref().and_then(|log| copy_tail(log, &dir.join("vrchat.log"), LOG_COPY).ok());
    // Two short samples of each, a few seconds apart; each one at most 2 s of the process's time, never stopping it.
    let targets = sample_targets(&tree);
    for round in 1..=2 {
        let handles: Vec<_> = targets
            .iter()
            .map(|(name, pid)| {
                let file = dir.join(format!("sample-{round}-{name}-{pid}.txt"));
                let pid = pid.to_string();
                std::thread::spawn(move || run_bounded("/usr/bin/sample", &[&pid, "2", "-mayDie", "-file", &file.to_string_lossy()], std::time::Duration::from_secs(20)).err())
            })
            .collect();
        let failed: Vec<String> = handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect();
        if !failed.is_empty() {
            std::fs::write(dir.join(format!("sample-{round}-errors.txt")), failed.join("\n"))?;
        }
        if round == 1 {
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }
    let marks = std::fs::read_to_string(dir.join("engine.log")).map(|t| wait_marks(&t)).unwrap_or_default();
    let boot = super::obelisk_cli::boot_clock();
    let summary = serde_json::json!({
        "kind": kind.word(),
        "session": followed.session,
        "launcher": followed.launcher,
        "seen_at": at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "boot": { "session": boot.boot, "seconds": boot.seconds },
        "detail": detail,
        "look": { "shown_frames": look.shown, "engine_log_bytes": look.engine_len, "vrchat_log_bytes": look.vrchat_len, "quitting": look.quitting, "renderer_lost": look.renderer_lost },
        "hot": hot.as_ref().map(|p| serde_json::json!({ "pid": p.pid, "cpu": p.cpu, "command": redact(&p.command) })),
        "sampled": targets.iter().map(|(n, p)| serde_json::json!({ "name": n, "pid": p })).collect::<Vec<_>>(),
        "logs": { "engine": followed.engine_log, "engine_bytes": engine_len, "vrchat": followed.vrchat_log, "vrchat_bytes": vrchat_len },
        "wrapper_waits": marks,
    });
    std::fs::write(dir.join("incident.json"), serde_json::to_string_pretty(&summary).unwrap_or_default())?;
    prune(logs);
    Ok(dir)
}

/// Runs `program` with `args`, its output read on a thread, for at most `limit`: its standard output, or why not.
fn run_bounded(program: &str, args: &[&str], limit: std::time::Duration) -> Result<String, String> {
    use std::io::Read as _;
    let mut child = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut out).take(8 << 20).read_to_end(&mut text);
        text
    });
    let until = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // The process is gone (ended or killed): its output ends unless a child of its own holds it, which is let go.
    let text = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
    match status {
        Some(s) if s.success() => Ok(text),
        Some(s) => Err(format!("{program} ended with {s}")), // i18n: not shown (a file for whoever looks into it)
        None => Err(format!("{program} did not end within {} s", limit.as_secs())), // i18n: not shown
    }
}

/// Reads what a log gained since `read` (at most [`SCAN_AT_MOST`], its end when it gained more): the new position and the
/// text. A log shorter than `read` was replaced: read from its start.
fn gained(path: &Path, read: u64) -> Option<(u64, String)> {
    use std::io::{Read as _, Seek as _};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = if len < read { 0 } else { read.max(len.saturating_sub(SCAN_AT_MOST)) };
    file.seek(std::io::SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(SCAN_AT_MOST).read_to_end(&mut bytes).ok()?;
    Some((len, String::from_utf8_lossy(&bytes).into_owned()))
}

/// The newest of VRChat's own logs in `dir` begun at or after `since` (Unix seconds).
fn vrchat_log(dir: &Path, since: u64) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(m, _)| m.duration_since(std::time::UNIX_EPOCH).is_ok_and(|d| d.as_secs() + 2 >= since))
        .max()
        .map(|(_, p)| p)
}

/// Follows the engine's sessions in `library` for as long as N4N0 runs (a Mac only), capturing each incident once.
#[cfg(target_os = "macos")]
pub(crate) fn follow(library: PathBuf, home: PathBuf, logs: PathBuf) {
    let clock = std::time::Instant::now();
    let mut current: Option<(Followed, Detector, u64, u64)> = None;
    loop {
        std::thread::sleep(EVERY);
        let record = std::fs::read_to_string(orbiters_core::obelisk::sessions_dir(&library).join(orbiters_core::obelisk::POINTER))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned))
            .and_then(|id| orbiters_core::obelisk::session_record_of(&library, &id).ok().flatten());
        let Some(record) = record.filter(|r| !r.over()) else {
            current = None;
            continue;
        };
        let Some(launcher) = record.game else { continue };
        let alive = record.game_birth.is_some_and(|birth| super::bridge_mac::born(launcher) == Some(birth));
        if current.as_ref().is_none_or(|(f, ..)| f.session != record.id || f.launcher != launcher) {
            // The engine's log is this start's from its beginning (a new one each start); VRChat's is found once it begins.
            let engine_log = orbiters_core::obelisk::engine_log(&home);
            let followed = Followed { session: record.id.clone(), launcher, library: library.clone(), engine_log, vrchat_log: None };
            current = Some((followed, Detector::default(), 0, 0));
        }
        let Some((followed, detector, engine_read, vrchat_read)) = current.as_mut() else { continue };
        if followed.vrchat_log.is_none() || followed.vrchat_log.as_ref().is_some_and(|l| !l.exists()) {
            let since = record.game_birth.map_or(0, |b| b.0);
            followed.vrchat_log = vrchat_log(&orbiters_core::obelisk::vrchat_log_dir(&library), since);
            *vrchat_read = 0;
        }
        let mut look = Look { at: clock.elapsed().as_secs_f64(), running: alive, shown: record.shown.map(|s| s.frames), ..Default::default() };
        if let Some((len, text)) = gained(&followed.engine_log, *engine_read) {
            look.engine_len = Some(len);
            look.renderer_lost = orbiters_core::obelisk::renderer_lost(&text);
            *engine_read = len;
        }
        if let Some((len, text)) = followed.vrchat_log.as_ref().and_then(|l| gained(l, *vrchat_read)) {
            look.vrchat_len = Some(len);
            look.quitting = text.contains("VRCApplication: HandleApplicationQuit"); // i18n: not shown (VRChat's log line)
            *vrchat_read = len;
        }
        // Once VRChat said it quits, it stays quitting.
        look.quitting |= detector.quit_at.is_some();
        // Once the graphics went, that is said once.
        let Some(kind) = detector.look(&look) else { continue };
        let detail = match kind {
            Kind::Stall => format!("No frame, no line in either log for {STALL_AFTER} s while the session runs."), // i18n: not shown (a file for whoever looks into it)
            Kind::StuckQuit => format!("VRChat said it quits and its session still runs {QUIT_AFTER} s later."), // i18n: not shown
            Kind::RendererLost => "The engine's log says the game's graphics server dropped it.".to_string(), // i18n: not shown
        };
        let (followed, look, logs) = (followed.clone(), look.clone(), logs.clone());
        let _ = std::thread::Builder::new().name("vrchat-incident".into()).spawn(move || {
            let at = time::OffsetDateTime::now_utc().unix_timestamp();
            match capture(&logs, kind, &followed, &look, &detail) {
                Ok(dir) => orbiters_core::notice_log::append(&logs, "warning", &orbiters_i18n::tr!("VRChat on this Mac stopped responding ({kind}): what was going on is kept in {path}", kind = kind.word(), path = dir.display().to_string()), at),
                Err(e) => orbiters_core::notice_log::append(&logs, "detail", &format!("VRChat incident ({}) could not be kept: {e}", kind.word()), at), // i18n: not shown (for the log)
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(at: f64, shown: u64, engine: u64, vrchat: u64) -> Look {
        Look { at, running: true, shown: Some(shown), engine_len: Some(engine), vrchat_len: Some(vrchat), ..Default::default() }
    }

    /// M15: frames and both logs stopped together while the session lives is a stall, told once; any of them moving is no
    /// stall (a covered window or a locked screen still presents, so the engine's log grows); a game that stopped is none.
    #[test]
    fn a_stall_is_all_three_stopped_while_the_session_lives_told_once() {
        let mut d = Detector::default();
        assert_eq!(d.look(&look(0.0, 10, 100, 100)), None);
        assert_eq!(d.look(&look(5.0, 20, 110, 105)), None, "drawing");
        assert_eq!(d.look(&look(30.0, 20, 110, 105)), None, "25 s still: not yet");
        assert_eq!(d.look(&look(35.0, 20, 110, 105)), Some(Kind::Stall));
        assert_eq!(d.look(&look(60.0, 20, 110, 105)), None, "once per incident");
        // It comes back, then stops again: another incident.
        assert_eq!(d.look(&look(65.0, 30, 110, 105)), None);
        assert_eq!(d.look(&look(96.0, 30, 110, 105)), Some(Kind::Stall));
        // Only the engine's log grows (a covered window): no stall.
        let mut d = Detector::default();
        for i in 0..20 {
            assert_eq!(d.look(&look(i as f64 * 5.0, 10, 100 + i, 50)), None);
        }
        // A session that is not running is never an incident; nor one never seen moving.
        let mut d = Detector::default();
        assert_eq!(d.look(&Look { running: false, ..look(0.0, 1, 1, 1) }), None);
        assert_eq!(d.look(&Look { running: false, ..look(100.0, 1, 1, 1) }), None);
        let mut d = Detector::default();
        assert_eq!(d.look(&look(0.0, 1, 1, 1)), None);
        assert_eq!(d.look(&look(100.0, 1, 1, 1)), None, "no progress ever seen: nothing to say it stopped");
    }

    /// M16: VRChat said it quits and the session is still there 45 s on: told once; a quit that ends in time is none; a
    /// quitting game that stops drawing is no stall. The graphics lost is told at once, once; and no more than three per
    /// session.
    #[test]
    fn a_quit_that_does_not_finish_and_a_lost_renderer_are_told_once_each() {
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        d.look(&look(5.0, 2, 2, 2));
        let quit = |at: f64| Look { quitting: true, ..look(at, 2, 2, 2) };
        assert_eq!(d.look(&quit(10.0)), None);
        assert_eq!(d.look(&quit(50.0)), None, "40 s: not yet; and not a stall");
        assert_eq!(d.look(&quit(55.0)), Some(Kind::StuckQuit));
        assert_eq!(d.look(&quit(90.0)), None);
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        assert_eq!(d.look(&Look { quitting: true, ..look(5.0, 1, 1, 1) }), None);
        assert_eq!(d.look(&Look { running: false, quitting: true, ..look(60.0, 1, 1, 1) }), None, "it ended: no incident");
        let mut d = Detector::default();
        assert_eq!(d.look(&Look { renderer_lost: true, ..look(0.0, 1, 1, 1) }), Some(Kind::RendererLost));
        assert_eq!(d.look(&Look { renderer_lost: true, ..look(5.0, 2, 2, 2) }), None);
        let mut d = Detector { captures: PER_SESSION, ..Default::default() };
        assert_eq!(d.look(&Look { renderer_lost: true, ..look(0.0, 1, 1, 1) }), None, "bounded per session");
    }

    #[test]
    fn credentials_never_reach_a_capture() {
        assert_eq!(redact("steam: refresh_token=abc.def.ghi"), "steam: refresh_token[removed]");
        assert_eq!(redact("Cookie: auth=authcookie_123"), "Cookie[removed]");
        assert_eq!(redact("PASSWORD hunter2"), "PASSWORD[removed]");
        assert_eq!(redact("wsd: window 1 overlay: frame held 100.0 ms"), "wsd: window 1 overlay: frame held 100.0 ms");
        assert_eq!(redact("été ticket=1"), "été ticket[removed]", "a line that is not ASCII");
    }

    #[test]
    fn captures_are_bounded_in_count_and_size_oldest_first() {
        let names = |n: usize, bytes: u64| (0..n).map(|i| (format!("2026-11-23_01-0{i}-00-stall-x"), bytes)).collect::<Vec<_>>();
        assert!(retain(names(KEPT, 1)).is_empty());
        assert_eq!(retain(names(KEPT + 2, 1)), ["2026-11-23_01-00-00-stall-x", "2026-11-23_01-01-00-stall-x"]);
        assert_eq!(retain(names(3, KEPT_BYTES / 2)), ["2026-11-23_01-00-00-stall-x"], "by weight too");
        assert!(retain(names(1, KEPT_BYTES * 4)).is_empty(), "the newest stays, whatever it weighs");
        // And on disk: the folder's name says its date, kind and session.
        let at = time::macros::datetime!(2026-11-23 01:02:03 UTC);
        assert!(folder(Path::new("/L"), at, Kind::StuckQuit, "f16d947e-7d8f-4331-8731-bede005d29fe").ends_with("incidents/2026-11-23_01-02-03-stuck-quit-f16d947e"));
        assert!(folder(Path::new("/L"), at, Kind::Stall, "../../x").ends_with("incidents/2026-11-23_01-02-03-stall-"), "nothing but the UUID's own characters");
    }

    /// The engine's processes as `ps` lists them (as on this Mac, 2026-11-23): the tree under the launcher and the ones
    /// sampled, the game being the one with gigabytes.
    #[test]
    fn the_engine_s_tree_and_the_processes_to_sample() {
        let ps = "  1     0  0.0  1000 Ss /sbin/launchd\n\
                  92600 1  0.0  4000 S  /bin/bash /x/launch.sh\n\
                  92621 92600 3.1 90000 S /x/VRChat.app/Contents/MacOS/wsd --dir s\n\
                  92705 92600 0.0 30000 S /L/Voile/v/linux/graphics/libexec/virgl_render_server --socket-fd 5\n\
                  92706 92705 45.0 900000 R /L/Voile/v/linux/graphics/libexec/virgl_render_server --worker\n\
                  92640 92600 1.0 20000 S /L/Voile/v/linux/engine/fexdarwin-run --cwd x /bin/sh\n\
                  92685 92640 99.0 6000000 R /L/Voile/v/linux/engine/fexdarwin-run VRChat.exe\n\
                  99999 1 0.5 2000 S /L/Voile/v/linux/engine/obelisk-input --dir s\n\
                  12345 1 80.0 2000 R /usr/bin/someone-else\n";
        let all = parse_ps(ps);
        assert_eq!(all.len(), 9);
        let t = tree(&all, 92600, "/L/Voile");
        let pids: Vec<u32> = t.iter().map(|p| p.pid).collect();
        assert_eq!(pids, [92600, 92621, 92705, 92706, 92640, 92685, 99999], "the launcher's, and the engine's helper left alone");
        assert_eq!(sample_targets(&t), [("game", 92685), ("render-server", 92706), ("wsd", 92621)], "the busiest is the game already");
        assert_eq!(wait_marks("x\nobelisk-wait: wine returned 0 at 1.5\ny\nobelisk-wait: wineserver -w returned at 2.0\n"), ["obelisk-wait: wine returned 0 at 1.5", "obelisk-wait: wineserver -w returned at 2.0"]);
    }

    /// The whole capture against a live tree of processes of the test's (`-- --ignored`, outside any sandbox: `sample`
    /// needs to examine them): every part is written, the samples are there, the folder is bounded.
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn a_capture_of_a_live_tree_keeps_every_part() {
        let dir = std::env::temp_dir().join(format!("orbiters-incident-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let waiter = dir.join("waiter.c");
        std::fs::write(&waiter, "#include <unistd.h>\nint main(){sleep(60);return 0;}\n").unwrap();
        assert!(std::process::Command::new("/usr/bin/cc").arg("-o").arg(dir.join("wsd")).arg(&waiter).status().unwrap().success());
        let mut launcher = std::process::Command::new("/bin/sh").arg("-c").arg(format!("{} & {} & wait", dir.join("wsd").display(), dir.join("wsd").display())).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        std::fs::write(dir.join("engine.log"), "neptune unixlib wsi (wsd): 5 presents\nobelisk-wait: wine returned 0 at 1.0\n").unwrap();
        let followed = Followed { session: "f16d947e-7d8f-4331-8731-bede005d29fe".into(), launcher: launcher.id(), library: dir.join("no-library"), engine_log: dir.join("engine.log"), vrchat_log: None };
        let logs = dir.join("logs");
        let out = capture(&logs, Kind::StuckQuit, &followed, &Look::default(), "test").unwrap();
        let _ = launcher.kill();
        let _ = launcher.wait();
        let names: Vec<String> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        for part in ["incident.json", "processes.txt", "memory.txt", "engine.log"] {
            assert!(names.iter().any(|n| n == part), "{part} in {names:?}");
        }
        assert_eq!(names.iter().filter(|n| n.starts_with("sample-") && !n.contains("errors")).count(), 2, "two samples of the window server: {names:?}");
        let summary = std::fs::read_to_string(out.join("incident.json")).unwrap();
        assert!(summary.contains("obelisk-wait: wine returned 0"), "{summary}");
        let _ = std::process::Command::new("/usr/bin/pkill").arg("-f").arg(dir.join("wsd")).status();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_is_copied_from_its_end_whole_lines_and_redacted() {
        let dir = std::env::temp_dir().join(format!("orbiters-incident-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let from = dir.join("wine.log");
        std::fs::write(&from, "first line\nsecond token=abc\nthird\n").unwrap();
        copy_tail(&from, &dir.join("all.log"), 1 << 20).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("all.log")).unwrap(), "first line\nsecond token[removed]\nthird\n");
        copy_tail(&from, &dir.join("end.log"), 12).unwrap();
        let end = std::fs::read_to_string(dir.join("end.log")).unwrap();
        assert!(end.ends_with("third\n") && !end.contains("first line") && end.starts_with("[the first"), "{end}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
