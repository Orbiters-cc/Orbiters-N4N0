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
/// At most this many captures of one session: stalls (a stall told is told again only after the game showed a frame or
/// VRChat logged again), and one each of a quit that does not finish and of a lost renderer, whatever stalls came first.
pub(crate) const STALLS_PER_SESSION: u32 = 2;
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
    /// The game's own counters as read at this look, `None` when one could not be read this time: the frames its
    /// record says it has shown, the presents its guest driver says (`neptune unixlib wsi (wsd): <n> presents`, which
    /// grow behind a locked screen or a covered window too), and the length of VRChat's own log.
    pub shown: Option<u64>,
    pub presents: Option<u64>,
    pub vrchat_len: Option<u64>,
    /// The engine's log's length: kept in the capture, never taken for the game's progress (processes around a parked
    /// game write into it too).
    pub engine_len: Option<u64>,
    /// VRChat's log said it quits (`HandleApplicationQuit`), or the engine's window server logged a quit asked of it.
    pub quitting: bool,
    /// Seconds since the session's supervisor accepted a Quit (its record's `quit_accepted`, obelisk-cli 0.1.8): what
    /// arms the stuck-quit detection whatever the game or its window server can still log.
    pub quit_since: Option<f64>,
    /// The graphics went for this run: confirmed (`picture`), the one decision Home and the bridge use too.
    pub renderer_lost: bool,
}

/// Tells incidents from the looks at one session (one Game session UUID: a new session gets a new detector), each once.
#[derive(Debug, Default)]
pub(crate) struct Detector {
    /// The last valid reading of each of the game's counters: a reading that failed leaves it as it was.
    frames: Option<u64>,
    presents: Option<u64>,
    vrchat: Option<u64>,
    progress_at: Option<f64>,
    stall_told: bool,
    pub(crate) quit_at: Option<f64>,
    quit_told: bool,
    lost_told: bool,
    stalls: u32,
}

/// Whether a counter grew from its last valid reading, and that reading kept: a missing reading changes nothing; a
/// smaller one is a counter that started again (a new swapchain, a log replaced): its new base, never progress.
fn advanced(last: &mut Option<u64>, now: Option<u64>) -> bool {
    let Some(now) = now else { return false };
    let grew = last.is_some_and(|before| now > before);
    *last = Some(now);
    grew
}

impl Detector {
    /// What this look shows that was not told yet.
    pub(crate) fn look(&mut self, look: &Look) -> Option<Kind> {
        // Progress is the game's own only: a frame shown, a frame presented, a line of VRChat's. The engine's log grows
        // from processes around a parked game too (a quit asked of its window server, a client of it connecting).
        let a = advanced(&mut self.frames, look.shown);
        let b = advanced(&mut self.presents, look.presents);
        let c = advanced(&mut self.vrchat, look.vrchat_len);
        if a || b || c {
            self.progress_at = Some(look.at);
            self.stall_told = false;
        }
        if !look.running {
            return None;
        }
        // A game that quits closes its renderer too (`destroying Neptune context` follows the engine's `quit requested`): no
        // loss. A quit that then does not finish is told as such.
        let quitting = look.quitting || look.quit_since.is_some();
        if look.renderer_lost && !self.lost_told && !quitting && self.quit_at.is_none() {
            self.lost_told = true;
            return Some(Kind::RendererLost);
        }
        if quitting {
            let first = *self.quit_at.get_or_insert(look.at);
            // from the supervisor's acceptance when the record says it, else from the first sign seen
            let waited = look.quit_since.unwrap_or(0.0).max(look.at - first);
            if waited >= QUIT_AFTER && !self.quit_told {
                self.quit_told = true;
                return Some(Kind::StuckQuit);
            }
            // A game quitting stops drawing: that is no stall.
            return None;
        }
        match self.progress_at {
            Some(at) if look.at - at >= STALL_AFTER && !self.stall_told && self.stalls < STALLS_PER_SESSION => {
                self.stall_told = true;
                self.stalls += 1;
                Some(Kind::Stall)
            }
            _ => None,
        }
    }
}

/// A line with what could be a credential taken out (whatever follows a word that names one), and what says where the
/// person was or who they are: VRChat links (any case, encoded too), a location's whole world, instance and access parts
/// (`wrld_…:…~private(…)~nonce(…)`, whether its parts are closed, cut short or encoded), account and group ids (`usr_…`,
/// `grp_…`) and a nonce (or an instance's secure or short name) given as a value, however its name and value are set apart.
/// Whatever is looked for is looked for under its encodings: percent-encoded, up to [`LAYERS`] times over, in part or whole,
/// and as JSON escapes it.
pub(crate) fn redact(line: &str) -> String {
    private(&credentials(line))
}

/// How many times over a value may be percent-encoded and still be read.
const LAYERS: usize = 4;

/// The character at the start of `s` as it reads once decoded, and how many bytes of `s` it takes: `%5F`, `%255F` (encoded
/// again, up to [`LAYERS`] times), `_` and `\/` are what they say.
fn logical(s: &str) -> Option<(char, usize)> {
    let bytes = s.as_bytes();
    let hex = |at: usize, digits: usize| -> Option<u32> {
        let text = s.get(at..at + digits)?;
        text.bytes().all(|b| b.is_ascii_hexdigit()).then(|| u32::from_str_radix(text, 16).ok()).flatten()
    };
    let mut decoded = match bytes.first()? {
        b'%' => {
            let mut at = 1;
            let mut layers = 1;
            // `%25` is a percent sign: what follows it is encoded once more.
            while layers < LAYERS && s.get(at..at + 2) == Some("25") && hex(at + 2, 2).is_some() {
                at += 2;
                layers += 1;
            }
            hex(at, 2).and_then(char::from_u32).filter(char::is_ascii).map(|c| (c, at + 2))
        }
        b'\\' if bytes.get(1) == Some(&b'u') => hex(2, 4).and_then(char::from_u32).filter(char::is_ascii).map(|c| (c, 6)),
        b'\\' if matches!(bytes.get(1), Some(b'/' | b'"' | b'\'')) => Some((bytes[1] as char, 2)),
        _ => None,
    };
    // A JSON escape of a percent sign: what it begins is encoded too.
    if let Some(('%', n)) = decoded {
        if let Some((c, more)) = hex(n, 2).and_then(char::from_u32).filter(char::is_ascii).map(|c| (c, 2)) {
            decoded = Some((c, n + more));
        }
    }
    decoded.or_else(|| s.chars().next().map(|c| (c, c.len_utf8())))
}

/// `s` reads as beginning with `word` (ASCII case ignored, under any encoding): how many bytes of `s` that takes.
fn reads(s: &str, word: &str) -> Option<usize> {
    let mut at = 0;
    for expected in word.chars() {
        let (c, n) = logical(&s[at..])?;
        if !c.eq_ignore_ascii_case(&expected) {
            return None;
        }
        at += n;
    }
    Some(at)
}

/// How many bytes at the start of `s` read as characters `is` takes.
fn run_of(s: &str, is: impl Fn(char) -> bool) -> usize {
    let mut at = 0;
    while let Some((c, n)) = logical(&s[at..]) {
        if !is(c) {
            break;
        }
        at += n;
    }
    at
}

/// Characters a location or a link is made of.
fn location_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_-:~().%+=&?/#!$*@,;|^<>[]{}".contains(c)
}

/// The length of the run of location characters at the start of `s`.
fn location_run(s: &str) -> usize {
    run_of(s, location_char)
}

/// An instance part at the start of `s` (`~name(`): its name and the length up to the value.
fn instance_part(s: &str) -> Option<(String, usize)> {
    let tilde = reads(s, "~")?;
    let mut name = String::new();
    let mut at = tilde;
    while let Some((c, n)) = logical(&s[at..]).filter(|(c, _)| c.is_ascii_alphabetic()) {
        name.push(c.to_ascii_lowercase());
        at += n;
    }
    let open = reads(&s[at..], "(")?;
    (!name.is_empty()).then_some((name, at + open))
}

/// Names whose value is a key to a private instance.
const KEYS: [&str; 3] = ["nonce", "securename", "shortname"];

/// One of [`KEYS`] at the start of `s` given a value (`nonce=…`, `nonce = …`, `"nonce":  "…"`, `nonce => '…'`, `nonce(…)`,
/// encoded or not): the name and the length up to the value's own characters.
fn keyed(s: &str) -> Option<(&'static str, usize)> {
    let (key, mut at) = KEYS.iter().find_map(|key| Some((*key, reads(s, key)?)))?;
    let space = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '`' | '+');
    at += run_of(&s[at..], space);
    let (separator, n) = logical(&s[at..])?;
    if !matches!(separator, ':' | '=' | '(' | '[') {
        return None;
    }
    at += n;
    at += run_of(&s[at..], |c| space(c) || matches!(c, ':' | '=' | '>' | '(' | '['));
    Some((key, at))
}

/// Whatever says where or who, taken out.
fn private(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while !rest.is_empty() {
        // a VRChat link, as it is or encoded: its scheme only
        if let Some(n) = reads(rest, "vrchat:/") {
            out.push_str("vrchat://[removed]");
            rest = &rest[n..];
            rest = &rest[location_run(rest)..];
            continue;
        }
        // a location: the world, its instance and every part after them
        if reads(rest, "wrld_").is_some() {
            out.push_str("[location removed]");
            rest = &rest[location_run(rest)..];
            continue;
        }
        // an instance part on its own (~region(eu) is kept: a region says nothing of the person)
        if let Some((name, n)) = instance_part(rest) {
            if name != "region" {
                out.push('~');
                out.push_str(&name);
                out.push_str("([removed])");
                let value = &rest[n..];
                // up to its close, or to the end of the run when it is cut short
                let inside = run_of(value, |c| c != ')' && location_char(c));
                rest = &value[inside + reads(&value[inside..], ")").unwrap_or(0)..];
                continue;
            }
        }
        // account and group ids
        if let Some((name, n)) = ["usr_", "grp_"].iter().find_map(|p| Some((*p, reads(rest, p)?))) {
            let after = &rest[n..];
            let end = run_of(after, |c| c.is_ascii_hexdigit() || c == '-');
            let digits = after[..end].len();
            if digits >= 8 {
                out.push_str(name);
                out.push_str("[removed]");
                rest = &after[end..];
                continue;
            }
        }
        // a nonce, or an instance's key, given as a value: the whole field
        if let Some((name, n)) = keyed(rest) {
            out.push_str(name);
            out.push_str("=[removed]");
            let value = &rest[n..];
            rest = &value[location_run(value)..];
            continue;
        }
        // One whole character as it reads: never the middle of an encoding, from which nothing would be found.
        let n = logical(rest).map_or(rest.len(), |(_, n)| n);
        out.push_str(&rest[..n]);
        rest = &rest[n..];
    }
    out
}

/// A line with whatever follows a word that names a credential taken out.
fn credentials(line: &str) -> String {
    const WORDS: [&str; 12] = ["password", "passwd", "token", "secret", "cookie", "authorization", "auth=", "authcookie", "apikey", "api_key", "session_key", "ticket"]; // i18n: not shown (words looked for in logs)
    let lower = line.to_ascii_lowercase();
    match WORDS.iter().filter_map(|w| lower.find(w).map(|i| i + w.len())).min() {
        // The byte positions of the lowercase copy are the line's own (ASCII lowercasing keeps every length).
        Some(end) if line.is_char_boundary(end) => return format!("{}[removed]", &line[..end]),
        Some(_) => return "[removed]".into(),
        None => {}
    }
    // The same words under an encoding.
    if line.contains('%') || line.contains("\\u") {
        for (at, _) in line.char_indices() {
            if let Some(n) = WORDS.iter().find_map(|word| reads(&line[at..], word)) {
                return format!("{}[removed]", &line[..at + n]);
            }
        }
    }
    line.to_string()
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

/// One process: its executable, never its arguments (they can name where the person is). Everything of it was read as one
/// occupant of its number: its birth is the same before and after.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// When it began (seconds, microseconds).
    pub birth: (u64, u32),
    pub cpu: f32,
    pub rss_kb: u64,
    pub state: String,
    /// The executable's whole path.
    pub exe: String,
}

/// What a process of the engine's was started with, read to decide whose it is and never kept.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Named {
    pub pid: u32,
    pub birth: (u64, u32),
    pub exe: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
}

/// Whether `named` is one of the engine's own helpers of the launch whose launcher is `launcher`, by the exact marker that
/// kind of helper is started with: its window server and its input service (`--dir obelisk-engine-<launcher>`), its graphics
/// server (`--socket-path obelisk-engine-<launcher>/vtest.sock`), every engine process (`FEXDARWIN_SUPERVISOR_PID=<launcher>`
/// in its environment). A program of another kind that names the same folder (a `tail` of it, an editor) is nothing of it.
fn helper_of(named: &Named, launcher: u32) -> bool {
    let folder = format!("obelisk-engine-{launcher}");
    let option = |name: &str, value: &str| named.args.iter().skip(1).zip(named.args.iter().skip(2)).any(|(a, b)| a == name && b == value);
    let program = named.exe.rsplit('/').next().unwrap_or_default();
    match program {
        "wsd" => named.exe.contains("/obelisk-apps/") && named.exe.ends_with(".app/Contents/MacOS/wsd") && option("--dir", &folder),
        "obelisk-input" => named.exe.ends_with("/linux/engine/obelisk-input") && option("--dir", &folder),
        "virgl_test_server" => named.exe.contains("/linux/graphics/") && option("--socket-path", &format!("{folder}/vtest.sock")),
        "fexdarwin-run" => named.exe.ends_with("/linux/engine/fexdarwin-run") && named.env.contains(&format!("FEXDARWIN_SUPERVISOR_PID={launcher}")),
        _ => false,
    }
}

/// The processes of `named` that are the engine's helpers of the launch whose launcher is `launcher`, born at `birth`: still
/// attributed to the session after their parent went. None is older than the launcher.
pub(crate) fn attributed(named: &[Named], launcher: u32, birth: (u64, u32)) -> Vec<(u32, (u64, u32))> {
    named.iter().filter(|n| n.birth >= birth && helper_of(n, launcher)).map(|n| (n.pid, n.birth)).collect()
}

/// The processes of the engine's game launched as `launcher` (born at `birth`): the launcher and every process under it, and
/// the processes [`attributed`] to this launch (`also`) with every process under them: a helper re-parented after its parent
/// went is still the session's. A process of another session, or of nobody's, is never one, whatever its executable (another
/// game's engine in the same library, an editor with the library open). A number used again is another process: neither the
/// launcher, nor a helper, nor the parent of one born before it.
pub(crate) fn tree(all: &[Proc], launcher: u32, birth: (u64, u32), also: &[(u32, (u64, u32))]) -> Vec<Proc> {
    let mut kept: Vec<(u32, (u64, u32))> = std::iter::once((launcher, birth)).chain(also.iter().copied().filter(|(pid, _)| *pid > 1)).collect();
    kept.retain(|(pid, birth)| all.iter().any(|p| p.pid == *pid && p.birth == *birth));
    loop {
        let more: Vec<(u32, (u64, u32))> = all
            .iter()
            .filter(|p| !kept.iter().any(|(pid, _)| *pid == p.pid))
            .filter(|p| kept.iter().any(|(pid, birth)| *pid == p.ppid && *birth <= p.birth))
            .map(|p| (p.pid, p.birth))
            .collect();
        if more.is_empty() {
            break;
        }
        kept.extend(more);
    }
    all.iter().filter(|p| kept.contains(&(p.pid, p.birth))).cloned().collect()
}

/// The processes to sample in `tree`: the game's own (the largest in memory among the engine's Wine processes, VRChat
/// holding gigabytes), the render server and the window server (the busiest of each name), and the busiest of all when it
/// is none of those. (name, pid), at most once each.
pub(crate) fn sample_targets(tree: &[Proc]) -> Vec<(&'static str, u32)> {
    let busiest = |f: &dyn Fn(&Proc) -> bool| tree.iter().filter(|p| f(p)).max_by(|a, b| a.cpu.total_cmp(&b.cpu)).map(|p| p.pid);
    let program = |p: &Proc| p.exe.rsplit('/').next().unwrap_or_default().to_string();
    let game = tree.iter().filter(|p| !matches!(program(p).as_str(), "wsd" | "virgl_render_server" | "virgl_test_server" | "obelisk-input" | "sh" | "bash" | "awk")).max_by_key(|p| p.rss_kb).map(|p| p.pid);
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

/// A process chosen for sampling: its pid, birth and executable taken together at the moment it was chosen, and checked
/// again before each sample (a pid used again is another birth, or another program).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub name: &'static str,
    pub pid: u32,
    pub birth: (u64, u32),
    pub exe: String,
}

impl Target {
    /// Whether the process is still the one chosen.
    pub(crate) fn same(&self, birth: Option<(u64, u32)>, exe: Option<&str>) -> bool {
        birth == Some(self.birth) && exe == Some(self.exe.as_str())
    }
}

/// The lines of voile-lkd's recent-event ring (`LKD_EVENT_RING`, a header line then fixed 128-byte lines, the newest
/// overwriting the oldest), in the order they happened, unwritten ones left out, each [`redact`]ed.
pub(crate) fn ring_lines(bytes: &[u8]) -> Vec<String> {
    const LINE: usize = 128;
    let mut lines: Vec<(u64, String)> = bytes
        .chunks(LINE)
        .skip(1)
        .filter(|chunk| chunk.len() == LINE && chunk[0].is_ascii_digit())
        .filter_map(|chunk| {
            let text = String::from_utf8_lossy(chunk).trim_end().to_string();
            let seq = text.get(..12)?.parse().ok()?;
            Some((seq, redact(&text)))
        })
        .collect();
    lines.sort_by_key(|(seq, _)| *seq);
    lines.into_iter().map(|(_, text)| text).collect()
}

/// The ring voile-lkd keeps beside the engine's log `engine_log` (obelisk-cli 0.1.8 names it so).
pub(crate) fn ring_of(engine_log: &Path) -> PathBuf {
    engine_log.with_extension("lkd-events")
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

/// Reads up to `most` bytes of `file` from `at`, wherever its own position is (it is shared with the follower).
fn read_at(file: &std::fs::File, at: u64, most: u64) -> std::io::Result<Vec<u8>> {
    let len = file.metadata()?.len();
    let mut bytes = vec![0; usize::try_from(len.saturating_sub(at).min(most)).unwrap_or(0)];
    let mut read = 0;
    while read < bytes.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut bytes[read..], at + read as u64)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut bytes[read..], at + read as u64)?;
        if n == 0 {
            break;
        }
        read += n;
    }
    bytes.truncate(read);
    Ok(bytes)
}

/// Copies the end of the held log `file` (at most `most` bytes, from a whole line) into `to`, every line [`redact`]ed.
pub(crate) fn copy_tail(file: &std::fs::File, to: &Path, most: u64) -> std::io::Result<u64> {
    use std::io::Write as _;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(most);
    let bytes = read_at(file, start, most)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 { text.split_once('\n').map_or("", |(_, rest)| rest) } else { &text };
    let mut out = std::io::BufWriter::new(std::fs::File::create(to)?);
    if start > 0 {
        writeln!(out, "[the first {start} bytes are left out]")?; // i18n: not shown (a log copy)
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

/// The session followed: what is needed to look at it, and to capture. Its logs are held open from the moment they were
/// found to be its own: whatever is put at their paths later (the next launch's) is never read as this session's.
#[derive(Debug)]
pub(crate) struct Followed {
    pub session: String,
    pub launcher: u32,
    pub birth: (u64, u32),
    pub engine_log: PathBuf,
    pub engine: Option<std::fs::File>,
    pub vrchat: Option<std::fs::File>,
}

impl Followed {
    /// The same, holding the same files: taken when a capture is scheduled.
    pub(crate) fn held(&self) -> Followed {
        let same = |file: &Option<std::fs::File>| file.as_ref().and_then(|f| f.try_clone().ok());
        Followed { session: self.session.clone(), launcher: self.launcher, birth: self.birth, engine_log: self.engine_log.clone(), engine: same(&self.engine), vrchat: same(&self.vrchat) }
    }
}

/// Whether a log made at `made` (Unix seconds) is the log of a launch whose launcher was born at `birth`: the launcher opens
/// it new as it becomes the game's. An earlier one is the last launch's, a later one the next one's.
pub(crate) fn log_is_of(made: u64, birth: u64) -> bool {
    made + 2 >= birth && made <= birth + 60
}

/// A log followed as it grows, by its open file: whole lines only, one cut at the end of a read kept for the next.
#[derive(Debug, Default)]
pub(crate) struct Lines {
    read: u64,
    partial: Vec<u8>,
    /// The line under way began where it was not read: what is left of it is no line.
    passed_over: bool,
}

/// A line longer than this is no marker of anything looked for.
const LINE_AT_MOST: usize = 64 << 10;

impl Lines {
    /// The whole lines `file` gained (the end of them when it gained more than [`SCAN_AT_MOST`]), and its length; `None`
    /// when it could not be read this time. A file cut shorter is read from its start.
    pub(crate) fn gained(&mut self, file: &std::fs::File) -> Option<(u64, String)> {
        let len = file.metadata().ok()?.len();
        if len < self.read {
            *self = Lines::default();
        }
        if len - self.read > SCAN_AT_MOST {
            self.read = len - SCAN_AT_MOST;
            self.partial.clear();
            self.passed_over = true;
        }
        let bytes = read_at(file, self.read, SCAN_AT_MOST).ok()?;
        self.read += bytes.len() as u64;
        self.partial.extend_from_slice(&bytes);
        let Some(end) = self.partial.iter().rposition(|b| *b == b'\n') else {
            if self.partial.len() > LINE_AT_MOST {
                self.partial.clear();
                self.passed_over = true;
            }
            return Some((len, String::new()));
        };
        let rest = self.partial.split_off(end + 1);
        let whole = std::mem::replace(&mut self.partial, rest);
        let text = String::from_utf8_lossy(&whole);
        let text = if std::mem::take(&mut self.passed_over) { text.split_once('\n').map_or("", |(_, after)| after) } else { &text };
        Some((len, text.to_owned()))
    }
}
/// Captures an incident of `kind` of `followed` into a new folder of `logs`: where it is.
pub(crate) fn capture(logs: &Path, kind: Kind, followed: &Followed, look: &Look, detail: &str) -> std::io::Result<PathBuf> {
    let at = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let dir = folder(logs, at, kind, &followed.session);
    std::fs::create_dir_all(&dir)?;
    let run = |program: &str, args: &[&str]| -> String {
        run_bounded(program, args, std::time::Duration::from_secs(15)).map(|t| t.lines().map(redact).collect::<Vec<_>>().join("\n")).unwrap_or_else(|e| format!("{program}: {e}"))
    };
    // The processes first: the moment the incident was seen. Each was read whole as one occupant of its number (parent,
    // birth, executable: `processes`); which are this session's is decided from their parents and from the exact marker a
    // helper of the engine's was started with (read, never kept); the ones to sample are chosen from those very readings.
    let all = processes();
    let also = attributed(&named(&all), followed.launcher, followed.birth);
    let tree = tree(&all, followed.launcher, followed.birth, &also);
    let targets: Vec<Target> = sample_targets(&tree)
        .into_iter()
        .filter_map(|(name, pid)| tree.iter().find(|p| p.pid == pid).map(|p| Target { name, pid, birth: p.birth, exe: p.exe.clone() }))
        .collect();
    let hot = tree.iter().max_by(|a, b| a.cpu.total_cmp(&b.cpu)).cloned();
    let mut listed = String::from("pid ppid %cpu rss_kb state executable\n"); // i18n: not shown (a file for whoever looks into it)
    for p in &tree {
        listed.push_str(&format!("{} {} {:.1} {} {} {}\n", p.pid, p.ppid, p.cpu, p.rss_kb, p.state, redact(&p.exe)));
    }
    std::fs::write(dir.join("processes.txt"), listed)?;
    let memory = format!(
        "kern.memorystatus_vm_pressure_level: {}\n\n{}\n\n{}\n",
        run("/usr/sbin/sysctl", &["-n", "kern.memorystatus_vm_pressure_level"]).trim(),
        run("/usr/bin/memory_pressure", &["-Q"]),
        run("/usr/bin/vm_stat", &[])
    );
    std::fs::write(dir.join("memory.txt"), memory)?;
    // The logs, by the files held since they were found to be this session's, whatever is at their paths now; and the engine's recent events (attaches, ptrace and group stops).
    let engine_len = followed.engine.as_ref().and_then(|log| copy_tail(log, &dir.join("engine.log"), LOG_COPY).ok());
    let vrchat_len = followed.vrchat.as_ref().and_then(|log| copy_tail(log, &dir.join("vrchat.log"), LOG_COPY).ok());
    // (the ring too only when it was made for this launch: each start makes a new one at its path)
    let ring = engine_log(&ring_of(&followed.engine_log), followed.birth.0).and_then(|file| read_at(&file, 0, 4 << 20).ok()).map(|bytes| ring_lines(&bytes));
    if let Some(lines) = &ring {
        std::fs::write(dir.join("lkd-events.txt"), lines.join("\n") + "\n")?;
    }
    // Two short samples of each, a few seconds apart; each one at most 2 s of the process's time, never stopping it. Each
    // is of the very process chosen (birth and executable checked before each round), read from `sample`'s output, never
    // from a file of its own, and written only once redacted: no raw sample is ever on disk.
    for round in 1..=2 {
        let handles: Vec<_> = targets
            .iter()
            .filter(|t| t.same(birth_of(t.pid), exe_of(t.pid).as_deref()))
            .map(|t| {
                let file = dir.join(format!("sample-{round}-{}-{}.txt", t.name, t.pid));
                let target = (*t).clone();
                let pid = t.pid.to_string();
                std::thread::spawn(move || match run_bounded("/usr/bin/sample", &[&pid, "2", "-mayDie", "-file", "/dev/stdout"], std::time::Duration::from_secs(20)) {
                    // The number may have been used again while it was sampled: then it is nobody's sample.
                    Ok(_) if birth_of(target.pid).is_some_and(|birth| birth != target.birth) => Some(format!("sample of {pid}: another process took its number")), // i18n: not shown (a file for whoever looks into it)
                    Ok(text) => {
                        let clean = text.lines().map(redact).collect::<Vec<_>>().join("\n");
                        std::fs::write(&file, clean).err().map(|e| format!("{}: {e}", file.display()))
                    }
                    Err(e) => Some(format!("sample of {pid}: {e}")), // i18n: not shown (a file for whoever looks into it)
                })
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
        "look": { "shown_frames": look.shown, "presents": look.presents, "engine_log_bytes": look.engine_len, "vrchat_log_bytes": look.vrchat_len, "quitting": look.quitting, "quit_accepted_seconds_ago": look.quit_since, "renderer_lost": look.renderer_lost },
        "hot": hot.as_ref().map(|p| serde_json::json!({ "pid": p.pid, "cpu": p.cpu, "executable": redact(&p.exe) })),
        "sampled": targets.iter().map(|t| serde_json::json!({ "name": t.name, "pid": t.pid, "birth": [t.birth.0, t.birth.1], "executable": redact(&t.exe) })).collect::<Vec<_>>(),
        "attributed_by_name": also.iter().map(|(pid, _)| pid).collect::<Vec<_>>(),
        "logs": { "engine_bytes": engine_len, "vrchat_bytes": vrchat_len, "lkd_events": ring.as_ref().map(Vec::len) },
        "wrapper_waits": marks,
    });
    std::fs::write(dir.join("incident.json"), serde_json::to_string_pretty(&summary).unwrap_or_default())?;
    prune(logs);
    Ok(dir)
}

/// This computer's processes, each read whole as one occupant of its number (a Mac only).
fn processes() -> Vec<Proc> {
    #[cfg(target_os = "macos")]
    return super::bridge_mac::processes();
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// What those of `all` that are a helper of the engine's by their program were started with.
fn named(all: &[Proc]) -> Vec<Named> {
    let helper = |p: &&Proc| matches!(p.exe.rsplit('/').next(), Some("wsd" | "obelisk-input" | "virgl_test_server" | "fexdarwin-run"));
    all.iter()
        .filter(helper)
        .filter_map(|p| {
            #[cfg(target_os = "macos")]
            let started = super::bridge_mac::started_with(p.pid, p.birth);
            #[cfg(not(target_os = "macos"))]
            let started: Option<(Vec<String>, Vec<String>)> = None;
            let (args, env) = started?;
            Some(Named { pid: p.pid, birth: p.birth, exe: p.exe.clone(), args, env })
        })
        .collect()
}

/// When process `pid` began (a pid used again by another process is another birth); `None` when it is gone.
fn birth_of(pid: u32) -> Option<(u64, u32)> {
    #[cfg(target_os = "macos")]
    return super::bridge_mac::born(pid);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

/// The executable of process `pid` now; `None` when it is gone.
fn exe_of(pid: u32) -> Option<String> {
    #[cfg(target_os = "macos")]
    return super::bridge_mac::executable(pid);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
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

/// The first of VRChat's own logs in `dir` begun at or after `since` (Unix seconds), held open: this session's, which is the
/// only one running (a later one is a later session's).
fn vrchat_log(dir: &Path, since: u64) -> Option<std::fs::File> {
    let mut logs: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
        .filter_map(|e| Some((made(&e.metadata().ok()?)?, e.path())))
        .filter(|(made, _)| made + 2 >= since)
        .collect();
    logs.sort();
    let (at, path) = logs.into_iter().next()?;
    let file = std::fs::File::open(path).ok()?;
    // The file found is the one opened.
    (made(&file.metadata().ok()?)? == at).then_some(file)
}

/// When a file was made (Unix seconds).
fn made(meta: &std::fs::Metadata) -> Option<u64> {
    meta.created().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// The engine's log at `path`, held open, when it is the one of the launch born at `birth`.
fn engine_log(path: &Path, birth: u64) -> Option<std::fs::File> {
    let file = std::fs::File::open(path).ok()?;
    log_is_of(made(&file.metadata().ok()?)?, birth).then_some(file)
}

/// Follows the engine's sessions in `library` for as long as N4N0 runs (a Mac only), capturing each incident once.
#[cfg(target_os = "macos")]
pub(crate) fn follow(library: PathBuf, home: PathBuf, logs: PathBuf) {
    let clock = std::time::Instant::now();
    let mut current: Option<(Followed, Detector, Lines, Lines, super::picture::Tracker)> = None;
    loop {
        std::thread::sleep(EVERY);
        // A pointer or record that cannot be read for a moment (being rewritten) keeps what is followed: only a record that
        // says its session is over, or another session, starts afresh.
        let pointer = std::fs::read_to_string(orbiters_core::obelisk::sessions_dir(&library).join(orbiters_core::obelisk::POINTER))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned));
        let Some(id) = pointer else { continue };
        let Ok(record) = orbiters_core::obelisk::session_record_of(&library, &id) else { continue };
        let Some(record) = record.filter(|r| !r.over()) else {
            current = None;
            super::picture::publish(None);
            continue;
        };
        // Which process it is, is its number and its birth together: a record that does not say both names none.
        let (Some(launcher), Some(birth)) = (record.game, record.game_birth) else { continue };
        let alive = super::bridge_mac::born(launcher) == Some(birth);
        if current.as_ref().is_none_or(|(f, ..)| f.session != record.id || f.launcher != launcher || f.birth != birth) {
            let engine_log = orbiters_core::obelisk::engine_log(&home);
            let followed = Followed { session: record.id.clone(), launcher, birth, engine_log, engine: None, vrchat: None };
            current = Some((followed, Detector::default(), Lines::default(), Lines::default(), super::picture::Tracker::default()));
        }
        let Some((followed, detector, engine_lines, vrchat_lines, picture)) = current.as_mut() else { continue };
        // Held once found to be this launch's (the engine's log is a new file each start; VRChat's begins a while after).
        if followed.engine.is_none() {
            followed.engine = engine_log(&followed.engine_log, birth.0);
        }
        if followed.vrchat.is_none() {
            followed.vrchat = vrchat_log(&orbiters_core::obelisk::vrchat_log_dir(&library), birth.0);
        }
        // Seconds since the supervisor accepted a Quit, from its record (both on the boot clock).
        let quit_since = record.quit_accepted.as_ref().and_then(|(boot, millis)| {
            let now = super::obelisk_cli::boot_clock();
            (now.boot == *boot).then(|| (now.seconds - *millis as f64 / 1000.0).max(0.0))
        });
        let shown = record.shown.filter(|s| s.continuous);
        let mut look = Look { at: clock.elapsed().as_secs_f64(), running: alive, shown: record.shown.map(|s| s.frames), quit_since, ..Default::default() };
        let mut graphics = None;
        if let Some((len, text)) = followed.engine.as_ref().and_then(|log| engine_lines.gained(log)) {
            look.engine_len = Some(len);
            look.presents = orbiters_core::obelisk::presents_last(&text);
            graphics = Some(orbiters_core::obelisk::graphics_lines(&text));
            // The engine accepted a quit (`steam quit` asks the window server to close the game's windows): a parked game
            // cannot log its own.
            look.quitting = text.contains("wsd: quit requested"); // i18n: not shown (the engine's log line)
        }
        if let Some((len, text)) = followed.vrchat.as_ref().and_then(|log| vrchat_lines.gained(log)) {
            look.vrchat_len = Some(len);
            look.quitting |= text.contains("VRCApplication: HandleApplicationQuit"); // i18n: not shown (VRChat's log line)
        }
        // Once VRChat said it quits, it stays quitting.
        look.quitting |= detector.quit_at.is_some();
        // Its picture: one decision, for Home (the restart it offers), the bridge and this capture (R1: only a confirmed loss
        // of this very session is ever acted on).
        let seen = picture.look(&super::picture::Look {
            at: look.at,
            wall: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()),
            running: look.running,
            log: graphics,
            shown: shown.map(|s| super::picture::Shown { frames: s.frames, at: Some(s.at as f64) }),
            quitting: look.quitting || look.quit_since.is_some(),
        });
        super::picture::publish(Some((record.id.clone(), seen)));
        look.renderer_lost = seen == super::picture::Picture::Lost;
        // Once the graphics went, that is said once.
        let Some(kind) = detector.look(&look) else { continue };
        let detail = match kind {
            Kind::Stall => format!("No frame, no line in either log for {STALL_AFTER} s while the session runs."), // i18n: not shown (a file for whoever looks into it)
            Kind::StuckQuit => format!("VRChat said it quits and its session still runs {QUIT_AFTER} s later."), // i18n: not shown
            Kind::RendererLost => "The engine's log says the game's graphics server dropped it, and no frame came since.".to_string(), // i18n: not shown
        };
        let (followed, look, logs) = (followed.held(), look.clone(), logs.clone());
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

    /// A list as `ps -axww -o pid=,ppid=,pcpu=,rss=,state=,comm=` prints it, each born when its number says.
    fn parse_ps(text: &str) -> Vec<Proc> {
        text.lines()
            .map(|line| {
                let mut rest = line.trim_start();
                let mut field = || {
                    let (word, after) = rest.split_once(char::is_whitespace).unwrap();
                    rest = after.trim_start();
                    word
                };
                let (pid, ppid) = (field().parse().unwrap(), field().parse().unwrap());
                let (cpu, rss_kb, state) = (field().parse().unwrap(), field().parse().unwrap(), field().to_string());
                Proc { pid, ppid, birth: (u64::from(pid), 0), cpu, rss_kb, state, exe: rest.trim_end().to_string() }
            })
            .collect()
    }

    fn started(pid: u32, exe: &str, args: &[&str], env: &[&str]) -> Named {
        let list = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        Named { pid, birth: (u64::from(pid), 0), exe: exe.into(), args: [list(&[exe]), list(args)].concat(), env: list(env) }
    }

    /// A look with the game's three counters: shown frames, the guest's presents, VRChat's log length.
    fn look(at: f64, shown: u64, presents: u64, vrchat: u64) -> Look {
        Look { at, running: true, shown: Some(shown), presents: Some(presents), vrchat_len: Some(vrchat), ..Default::default() }
    }

    /// M15: the game's counters all stopped while the session lives is a stall, told once; any of them moving is no stall
    /// (a covered window or a locked screen still presents: the guest's presents grow); a game that stopped is none.
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
        // Only the presents grow (a covered window, the lock screen): no stall.
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
        // Stalls are bounded per session; a quit or a lost renderer is still told after them.
        let mut d = Detector { stalls: STALLS_PER_SESSION, ..Default::default() };
        assert_eq!(d.look(&Look { renderer_lost: true, ..look(0.0, 1, 1, 1) }), Some(Kind::RendererLost), "not used up by stalls");
    }

    /// 2026-11-23 05:24: a graceful quit by session was captured as a lost renderer (the renderer closes with the game).
    #[test]
    fn a_renderer_closed_by_a_quit_is_no_loss() {
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        assert_eq!(d.look(&Look { quitting: true, renderer_lost: true, ..look(5.0, 1, 2, 1) }), None);
        assert_eq!(d.look(&Look { renderer_lost: true, ..look(10.0, 1, 3, 1) }), None, "quitting stays quitting");
        let mut d = Detector::default();
        assert_eq!(d.look(&Look { renderer_lost: true, ..look(0.0, 1, 1, 1) }), Some(Kind::RendererLost), "a loss while it plays");
    }

    /// Finding 6 of the third verification: at 03:43:52 and 03:50:18 one parked game was told as two stalls, because a
    /// quit asked of its window server wrote 193 bytes into the engine's log. A stall told is told again only after the
    /// game itself moved; the engine's accepted quit arms the stuck-quit detection, which a parked game cannot log.
    #[test]
    fn a_parked_game_s_quit_is_a_stuck_quit_not_a_second_stall() {
        let mut d = Detector::default();
        d.look(&look(0.0, 651, 56000, 131714));
        d.look(&look(5.0, 652, 56159, 131714));
        assert_eq!(d.look(&look(40.0, 652, 56159, 131714)), Some(Kind::Stall));
        // The engine's log grows (a client of the window server, a quit asked): no recovery.
        assert_eq!(d.look(&Look { engine_len: Some(56300), ..look(300.0, 652, 56159, 131714) }), None);
        let quit = |at: f64| Look { quitting: true, engine_len: Some(56352), ..look(at, 652, 56159, 131714) };
        assert_eq!(d.look(&quit(380.0)), None);
        assert_eq!(d.look(&quit(430.0)), Some(Kind::StuckQuit), "45 s after the engine accepted the quit");
        assert_eq!(d.look(&Look { engine_len: Some(56400), ..look(500.0, 652, 56159, 131714) }), None, "never a second stall without the game's own progress");
        // A game that draws again is a new incident when it stops again.
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        d.look(&look(5.0, 2, 2, 2));
        assert_eq!(d.look(&look(40.0, 2, 2, 2)), Some(Kind::Stall));
        assert_eq!(d.look(&look(45.0, 3, 3, 2)), None, "a frame: recovered");
        assert_eq!(d.look(&look(80.0, 3, 3, 2)), Some(Kind::Stall));
        assert_eq!(d.look(&look(85.0, 4, 4, 2)), None);
        assert_eq!(d.look(&look(120.0, 4, 4, 2)), None, "two stalls per session at most");
    }

    #[test]
    fn credentials_never_reach_a_capture() {
        assert_eq!(redact("steam: refresh_token=abc.def.ghi"), "steam: refresh_token[removed]");
        assert_eq!(redact("Cookie: auth=authcookie_123"), "Cookie[removed]");
        assert_eq!(redact("PASSWORD hunter2"), "PASSWORD[removed]");
        assert_eq!(redact("wsd: window 1 overlay: frame held 100.0 ms"), "wsd: window 1 overlay: frame held 100.0 ms");
        assert_eq!(redact("été ticket=1"), "été ticket[removed]", "a line that is not ASCII");
    }

    /// Finding 5: no private destination or account id in a capture (the 03:43 incident kept a private instance's owner and
    /// nonce in its command lines and logs).
    #[test]
    fn private_destinations_and_account_ids_never_reach_a_capture() {
        let arg = "sh C:\\steam.exe --no-vr vrchat://launch?ref=orbiters.cc&id=wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:63402~private(usr_1339e525-d75d-4832-9104-98dd89287681)~region(eu)";
        let out = redact(arg);
        assert!(!out.contains("usr_1339") && !out.contains("wrld_4432") && !out.contains("63402"), "{out}");
        assert!(out.contains("vrchat://[removed]"), "{out}");
        let line = "[Behaviour] Joining wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:63402~private(usr_1339e525-d75d-4832-9104-98dd89287681)~nonce(4c1f0e7a-2d3e-4b0a-9f01-7a2b3c4d5e6f)~region(eu)";
        let out = redact(line);
        assert_eq!(out, "[Behaviour] Joining [location removed]", "the whole location: world, instance and every part");
        assert_eq!(redact("OnPlayerJoined blackorbit (usr_1339e525-d75d-4832-9104-98dd89287681)"), "OnPlayerJoined blackorbit (usr_[removed])");
        assert_eq!(redact("wsd: window 1 overlay: frame held 100.0 ms"), "wsd: window 1 overlay: frame held 100.0 ms");
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
                  92600 1  0.0  4000 S  /bin/bash\n\
                  92621 92600 3.1 90000 S /x/T/obelisk-apps/VRChat/VRChat.app/Contents/MacOS/wsd\n\
                  92705 92600 0.0 30000 S /L L/Voile/v/linux/graphics/libexec/virgl_render_server\n\
                  92706 92705 45.0 900000 R /L L/Voile/v/linux/graphics/libexec/virgl_render_server\n\
                  92640 92600 1.0 20000 S /L L/Voile/v/linux/engine/fexdarwin-run\n\
                  92685 92640 99.0 6000000 R /L L/Voile/v/linux/engine/fexdarwin-run\n\
                  99999 1 0.5 2000 S /L L/Voile/v/linux/engine/obelisk-input\n\
                  55555 1 1.0 9000000 S /Applications/Editor.app/Contents/MacOS/Editor\n\
                  12345 1 80.0 2000 R /usr/bin/someone-else\n";
        let all = parse_ps(ps);
        assert_eq!(all.len(), 10);
        assert_eq!(all[3].exe, "/L L/Voile/v/linux/graphics/libexec/virgl_render_server", "a path with a space is whole");
        // The editor has the library open (its arguments would name it) but is no process of the engine's; the input helper
        // left alone is this session's because its arguments name the launch's folder.
        let input = started(99999, "/L L/Voile/v/linux/engine/obelisk-input", &["--dir", "obelisk-engine-92600"], &[]);
        let t = tree(&all, 92600, (92600, 0), &attributed(&[input], 92600, (92600, 0)));
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
        let engine_log = dir.join("engine.log");
        let followed = Followed {
            session: "f16d947e-7d8f-4331-8731-bede005d29fe".into(),
            launcher: launcher.id(),
            birth: super::super::bridge_mac::born(launcher.id()).unwrap(),
            engine: Some(std::fs::File::open(&engine_log).unwrap()),
            engine_log: engine_log.clone(),
            vrchat: None,
        };
        // The next launch's log at the same path, while this session's is captured: this session's is.
        std::fs::rename(&engine_log, dir.join("engine.previous.log")).unwrap();
        std::fs::write(&engine_log, "the next launch's\n").unwrap();
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
        assert!(!std::fs::read_to_string(out.join("engine.log")).unwrap().contains("the next launch's"));
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
        let held = std::fs::File::open(&from).unwrap();
        copy_tail(&held, &dir.join("all.log"), 1 << 20).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("all.log")).unwrap(), "first line\nsecond token[removed]\nthird\n");
        copy_tail(&held, &dir.join("end.log"), 12).unwrap();
        let end = std::fs::read_to_string(dir.join("end.log")).unwrap();
        assert!(end.ends_with("third\n") && !end.contains("first line") && end.starts_with("[the first"), "{end}");
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// Fourth verification, finding 8: the forms that survived before (a percent-encoded nonce, an unterminated
    /// `~nonce(`, an upper-case link) and their neighbours (doubly encoded, a nonce as a value, a group id, an instance
    /// part alone) are all taken out; text around them stays.
    #[test]
    fn encoded_truncated_and_upper_case_private_values_are_taken_out() {
        let secrets = ["4c1f0e7a", "1339e525", "4432ea9b", "63402", "0b9a"];
        let cases = [
            "join wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd%3A63402%7Eprivate%28usr_1339e525-d75d-4832-9104-98dd89287681%29%7Enonce%284c1f0e7a-2d3e-4b0a-9f01-7a2b3c4d5e6f%29 end",
            "id=wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd%253A63402%257Enonce%25284c1f0e7a-2d3e end",
            "cut ~nonce(4c1f0e7a-2d3e-4b0a-9f01",
            "cut %7Enonce%284c1f0e7a-2d3e",
            "OPEN VRCHAT://LAUNCH?ID=WRLD_4432EA9B:63402~NONCE(4c1f0e7a) end",
            "open Vrchat%3A%2F%2Flaunch%3Fid%3Dwrld_4432ea9b%3A63402 end",
            r#"{"nonce":"4c1f0e7a-2d3e","x":1}"#,
            "token nonce=4c1f0e7a-2d3e end",
            "owner ~group(grp_0b9a1234-5678-4abc-9def-0123456789ab)~groupAccessType(members) end",
            "member grp_0b9a1234-5678-4abc-9def-0123456789ab end",
            "[Behaviour] Joining or Creating Room: wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:63402~private(usr_1339e525-d75d-4832-9104-98dd89287681",
        ];
        for line in cases {
            let out = redact(line);
            for secret in secrets {
                assert!(!out.to_ascii_lowercase().contains(secret), "{secret} survived in {out:?} (from {line:?})");
            }
        }
        assert_eq!(redact("cut ~nonce(4c1f0e7a-2d3e-4b0a-9f01"), "cut ~nonce([removed])");
        assert_eq!(redact("OPEN VRCHAT://LAUNCH?ID=WRLD_4432EA9B end"), "OPEN vrchat://[removed] end");
        assert_eq!(redact("member grp_0b9a1234-5678-4abc-9def-0123456789ab end"), "member grp_[removed] end");
        assert_eq!(redact("in ~region(eu) still"), "in ~region(eu) still", "a region says nothing of the person");
        assert_eq!(redact("neptune unixlib wsi (wsd): 6480 presents"), "neptune unixlib wsi (wsd): 6480 presents");
    }

    /// Finding 9: a reading missing between two equal frame counts (651) is no progress; a shorter (replaced) log is
    /// none; the engine's log growing alone never postpones the first stall; the supervisor's acceptance of a Quit arms the
    /// stuck-quit detection when nothing logs it (a parked game, a window server that died).
    #[test]
    fn only_the_game_s_own_progress_counts_and_an_accepted_quit_arms_the_stuck_quit() {
        let mut d = Detector::default();
        d.look(&look(0.0, 650, 100, 1000));
        d.look(&look(5.0, 651, 100, 1000));
        assert_eq!(d.look(&look(40.0, 651, 100, 1000)), Some(Kind::Stall));
        assert_eq!(d.look(&Look { shown: None, presents: None, vrchat_len: None, ..look(45.0, 0, 0, 0) }), None, "nothing read");
        assert_eq!(d.look(&look(50.0, 651, 100, 1000)), None, "651 again after a failed read: no progress");
        assert_eq!(d.look(&look(100.0, 651, 100, 1000)), None, "so no second stall either");
        assert_eq!(d.look(&look(105.0, 651, 100, 200)), None, "a shorter log: replaced, not progress");
        assert_eq!(d.look(&look(150.0, 651, 100, 200)), None, "still the same incident");
        // the engine's log alone (other clients of the window server) never moves the first stall
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        d.look(&look(5.0, 2, 2, 2));
        for (k, at) in [10.0, 20.0, 30.0].iter().enumerate() {
            assert_eq!(d.look(&Look { engine_len: Some(1000 + k as u64 * 100), ..look(*at, 2, 2, 2) }), None);
        }
        assert_eq!(d.look(&Look { engine_len: Some(5000), ..look(36.0, 2, 2, 2) }), Some(Kind::Stall));
        // a Quit the supervisor accepted 46 s ago, with no line of anyone's: a stuck quit
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        assert_eq!(d.look(&Look { quit_since: Some(10.0), ..look(5.0, 1, 1, 1) }), None);
        assert_eq!(d.look(&Look { quit_since: Some(46.0), ..look(41.0, 1, 1, 1) }), Some(Kind::StuckQuit));
        assert_eq!(d.look(&Look { quit_since: Some(80.0), ..look(75.0, 1, 1, 1) }), None, "once");
        // a quit accepted that finishes is none; a renderer closing during it is no loss
        let mut d = Detector::default();
        d.look(&look(0.0, 1, 1, 1));
        assert_eq!(d.look(&Look { quit_since: Some(1.0), renderer_lost: true, ..look(5.0, 1, 1, 1) }), None);
        assert_eq!(d.look(&Look { running: false, quit_since: Some(60.0), ..look(60.0, 1, 1, 1) }), None);
    }

    /// Finding 7: the session's processes are the launcher's tree and the ones whose own arguments or environment name
    /// this launch (a helper re-parented); another session's engine processes, or an editor, are never chosen, whatever
    /// their memory. The executable and birth chosen are the ones checked before each sample.
    #[test]
    fn processes_are_the_session_s_by_ancestry_or_by_name_only() {
        let ps = "  1     0  0.0  1000 Ss /sbin/launchd\n\
                  92600 1  0.0  4000 S  /bin/bash\n\
                  92640 92600 1.0 20000 S /L L/Voile/v/linux/engine/fexdarwin-run\n\
                  92685 92640 99.0 6000000 R /L L/Voile/v/linux/engine/fexdarwin-run\n\
                  92621 1 3.1 90000 S /x/T/obelisk-apps/VRChat/VRChat.app/Contents/MacOS/wsd\n\
                  92700 92621 0.0 100 S /x/T/child-of-a-reparented\n\
                  93000 1 1.0 9000000 R /L L/Voile/v/linux/engine/fexdarwin-run\n\
                  93001 1 1.0 9500000 R /x/T/obelisk-apps/Other/Other.app/Contents/MacOS/wsd\n\
                  55555 1 1.0 9900000 S /Applications/Editor.app/Contents/MacOS/Editor\n";
        let all = parse_ps(ps);
        let fexdarwin = "/L L/Voile/v/linux/engine/fexdarwin-run";
        let named = [
            started(92621, "/x/T/obelisk-apps/VRChat/VRChat.app/Contents/MacOS/wsd", &["--dir", "obelisk-engine-92600"], &[]),
            started(93000, fexdarwin, &["--cwd", "/opt/game"], &["FEXDARWIN_SUPERVISOR_PID=926001", "HOME=/x"]),
            started(93001, "/x/T/obelisk-apps/Other/Other.app/Contents/MacOS/wsd", &["--dir", "obelisk-engine-926001"], &[]),
            started(55555, "/Applications/Editor.app/Contents/MacOS/Editor", &[fexdarwin], &[]),
            started(92685, fexdarwin, &["--cwd", "/opt/game"], &["FEXDARWIN_SUPERVISOR_PID=92600"]),
        ];
        let also = attributed(&named, 92600, (92600, 0));
        assert_eq!(also, [(92621, (92621, 0)), (92685, (92685, 0))], "by name: this launch's only (926001 is another)");
        let t = tree(&all, 92600, (92600, 0), &also);
        let pids: Vec<u32> = t.iter().map(|p| p.pid).collect();
        assert_eq!(pids, [92600, 92640, 92685, 92621, 92700]);
        assert_eq!(sample_targets(&t)[0], ("game", 92685), "the largest of this session, not another session's 9 GB");
        let target = Target { name: "game", pid: 92685, birth: (10, 5), exe: "/L L/Voile/v/linux/engine/fexdarwin-run".into() };
        assert!(target.same(Some((10, 5)), Some("/L L/Voile/v/linux/engine/fexdarwin-run")));
        assert!(!target.same(Some((11, 0)), Some("/L L/Voile/v/linux/engine/fexdarwin-run")), "a pid used again");
        assert!(!target.same(Some((10, 5)), Some("/usr/bin/other")), "another program");
        assert!(!target.same(None, None), "gone");
    }

    /// The engine's recent-event ring is copied in the order things happened, unwritten lines left out, redacted.
    #[test]
    fn the_event_ring_is_copied_in_order_and_redacted() {
        let line = |seq: u64, text: &str| {
            let mut l = format!("{seq:012} {:012} {:6} {text}", seq * 10, 42).into_bytes();
            l.resize(127, b' ');
            l.push(b'\n');
            l
        };
        let mut bytes = vec![0u8; 128];
        bytes.extend(line(7, "ptrace: attach 7: tracer 5 -> pid 3 tid 3, link 0 gen 9, wake 0"));
        bytes.extend(vec![0u8; 128]);
        bytes.extend(line(5, "sig: SIGCONT resumes process 3 wrld_4432ea9b:1~nonce(x)"));
        let lines = ring_lines(&bytes);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("000000000005") && lines[0].contains("[location removed]") && !lines[0].contains("4432"), "{lines:?}");
        assert!(lines[1].contains("ptrace: attach 7"));
        assert_eq!(ring_of(Path::new("/L/wine-438100.log")), Path::new("/L/wine-438100.lkd-events"));
    }

    /// Fifth verification, finding 10: the forms that survived, and their neighbours under the same reading: any separator
    /// with any white space around it, a JSON field, up to four encodings over one another, in part or whole, escaped by
    /// JSON, the other keys of an instance.
    #[test]
    fn no_spelling_of_a_private_value_survives() {
        let secret = "abcdef12-3456";
        let cases = [
            "nonce = abcdef12-3456",
            "nonce%253Dabcdef12-3456",
            "\"nonce\":  \"abcdef12-3456\"",
            "wrld%5Fabcdef12-3456%3A56",
            "nonce\t:\tabcdef12-3456",
            "nonce => 'abcdef12-3456'",
            "nonce%20%3D%20abcdef12-3456",
            "nonce%2520%253D%2520abcdef12-3456",
            "nonce%2525253Dabcdef12-3456",
            "%6Eonce=abcdef12-3456",
            "NONCE%3A%22abcdef12-3456%22",
            r#"{\"nonce\":\"abcdef12-3456\"}"#,
            r#"{"nonce" : "abcdef12-3456", "x": 1}"#,
            r"\u006eonce\u003dabcdef12-3456",
            "wrld%255Fabcdef12-3456%253A56",
            "wrld%25255Fabcdef12-3456",
            "%77rld_abcdef12-3456:1",
            r"wrld\u005fabcdef12-3456",
            "usr%5Fabcdef12-3456-7890",
            "GRP%255Fabcdef12-3456-7890",
            "%7Eprivate%28usr%5Fabcdef12-3456%29",
            "%257Enonce%2528abcdef12-3456",
            r"vrchat:\/\/launch?id=abcdef12-3456",
            "vrchat%3A%2F%2Flaunch%3Fid=abcdef12-3456",
            "\"secureName\": \"abcdef12-3456\"",
            "shortName=abcdef12-3456",
            "nonce(abcdef12-3456)",
            "pass%77ord=abcdef12-3456",
            "%74oken: abcdef12-3456",
        ];
        for line in cases {
            for line in [line.to_string(), format!("2026.11.23 08:06:37 Debug - before {line}"), format!("é{line} after")] {
                let out = redact(&line);
                assert!(!out.contains(secret), "{out:?} (from {line:?})");
                assert!(!out.contains("abcdef12"), "{out:?} (from {line:?})");
            }
        }
        // What says nothing of anybody stays, percent signs and all.
        for line in ["GPU 88.5 % busy", "100%", "%", "\\u", "announce the nonce", "frame held 100.0 ms", "C:\\users\\x 50%25 off", "ünïcode %C3%A9"] {
            assert_eq!(redact(line), line);
        }
    }

    /// Finding 9: a program that names the session's folder is not its helper (a `tail` of it, an editor, a shell, a copy of
    /// the helper elsewhere); a number used again is another process, whoever's child or parent it is.
    #[test]
    fn only_the_engine_s_own_helpers_by_their_exact_marker_are_the_session_s() {
        let (wsd, fexdarwin) = ("/x/T/obelisk-apps/VRChat/VRChat.app/Contents/MacOS/wsd", "/L/Voile/v/linux/engine/fexdarwin-run");
        let mark = ["--dir", "obelisk-engine-92600"];
        let env = ["FEXDARWIN_SUPERVISOR_PID=92600"];
        let test_server = "/L/Voile/v/linux/graphics/bin/virgl_test_server";
        let yes = [
            started(92700, wsd, &mark, &[]),
            started(92701, "/L/Voile/v/linux/engine/obelisk-input", &mark, &[]),
            started(92702, test_server, &["--no-virgl", "--no-fork", "--socket-path", "obelisk-engine-92600/vtest.sock"], &[]),
            started(92703, fexdarwin, &[], &env),
        ];
        assert_eq!(attributed(&yes, 92600, (92600, 0)).len(), yes.len());
        let no = [
            started(92710, "/usr/bin/tail", &["-f", "/x/T/obelisk-engine-92600/log"], &[]),
            started(92711, "/usr/bin/tail", &["--dir", "obelisk-engine-92600"], &env),
            started(92712, "/bin/bash", &["-c", "wsd --dir obelisk-engine-92600"], &env),
            started(92713, "/tmp/wsd", &mark, &env),
            started(92714, wsd, &["--dir", "obelisk-engine-926001"], &[]),
            started(92715, wsd, &["--dir obelisk-engine-92600"], &[]),
            started(92716, wsd, &["obelisk-engine-92600"], &[]),
            started(92717, fexdarwin, &["FEXDARWIN_SUPERVISOR_PID=92600"], &["X=FEXDARWIN_SUPERVISOR_PID=92600", "FEXDARWIN_SUPERVISOR_PID=926001"]),
            started(92718, test_server, &["--socket-path", "/x/obelisk-engine-92600/vtest.sock"], &[]),
            // Older than the launcher: the last occupant's.
            started(92000, wsd, &mark, &[]),
        ];
        assert_eq!(attributed(&no, 92600, (92600, 0)), []);
        let proc = |pid: u32, ppid: u32, birth: u64| Proc { pid, ppid, birth: (birth, 0), cpu: 0.0, rss_kb: 0, state: "S".into(), exe: "/x".into() };
        // A child of the launcher's number born before it is not its child; one of a helper's likewise.
        let all = [proc(92600, 1, 500), proc(92601, 92600, 600), proc(92602, 92600, 400), proc(92700, 1, 700), proc(92701, 92700, 650), proc(92702, 92700, 800)];
        let pids = |launcher: (u64, u32), also: &[(u32, (u64, u32))]| tree(&all, 92600, launcher, also).iter().map(|p| p.pid).collect::<Vec<_>>();
        assert_eq!(pids((500, 0), &[(92700, (700, 0))]), [92600, 92601, 92700, 92702]);
        // Its number used again since the record was written: nothing is the session's, nor is a helper's.
        assert_eq!(pids((499, 0), &[(92700, (699, 0))]), [] as [u32; 0]);
    }

    /// Finding 9: the log followed is this launch's, by the file held, whole lines only.
    #[test]
    fn a_followed_log_is_its_own_file_and_whole_lines() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("orbiters-incident-lines-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wine.log");
        let mut log = std::fs::OpenOptions::new().append(true).create(true).open(&path).unwrap();
        let held = std::fs::File::open(&path).unwrap();
        let mut lines = Lines::default();
        let mut gained = |lines: &mut Lines, text: &[u8]| {
            log.write_all(text).unwrap();
            lines.gained(&held).unwrap().1
        };
        // A marker cut between two reads is found once, whole.
        assert_eq!(gained(&mut lines, b"one\nVRCApplication: HandleAppl"), "one\n");
        assert_eq!(gained(&mut lines, b""), "");
        assert_eq!(gained(&mut lines, b"icationQuit\ntw"), "VRCApplication: HandleApplicationQuit\n");
        // A character cut between two reads too.
        assert_eq!(gained(&mut lines, b"o \xc3"), "");
        assert_eq!(gained(&mut lines, b"\xa9\n"), "two \u{e9}\n");
        // The next launch's log at that path is not read.
        std::fs::rename(&path, dir.join("wine.previous.log")).unwrap();
        std::fs::write(&path, "the next launch's\n").unwrap();
        assert_eq!(gained(&mut lines, b"still this one's\n"), "still this one's\n");
        // More than is read at once: from a whole line on, never from the middle of one.
        let big = [vec![b'x'; SCAN_AT_MOST as usize], b"\nafter\nha".to_vec()].concat();
        assert_eq!(gained(&mut lines, &big), "after\n");
        assert_eq!(gained(&mut lines, b"lf\n"), "half\n");
        // Cut shorter: read from its start.
        std::fs::OpenOptions::new().write(true).open(dir.join("wine.previous.log")).unwrap().set_len(0).unwrap();
        assert_eq!(gained(&mut lines, b"anew\n"), "anew\n");
        let _ = std::fs::remove_dir_all(&dir);
        // The launcher opens it new as it becomes the game's.
        assert!(log_is_of(1000, 1000) && log_is_of(1001, 1000) && log_is_of(998, 1000));
        assert!(!log_is_of(900, 1000), "the last launch's");
        assert!(!log_is_of(1200, 1000), "the next launch's");
    }
}
