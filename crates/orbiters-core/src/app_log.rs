//! The app's own log (XCT-27): what went wrong in the service, the window, the overlay and the workers, on this computer
//! alone. An installed Windows build has no stderr; before this, whatever was written there was nowhere.
//!
//! **What it holds**, and never more: when, which process and component, the words of the app's own message, error kinds
//! and codes, counts, and files by their place in the app's own folders. **What it never holds**: tokens, cookies, account
//! names and ids, friends' and worlds' names, anything anyone wrote, VRChat's own log lines, a person's paths, the query of
//! a URL. It is held by how a line is made, not by searching for what must not be there: the file gets the message as it
//! stands in the source and, for each value in it, only what its *type* is known to say ([`Plain`]: numbers, durations,
//! error kinds and codes, the app's own files). A value of any other type (a text above all) is `…`. [`redacted`] goes over
//! the line once more, for what would get through a mistake in that.
//!
//! **Where it goes**: `<logs>/app/n4n0.log`, with `n4n0.1.log` to `n4n0.4.log` before it: [`FILES`] files of [`LARGEST`],
//! [`DAYS`] days. The app never sends it anywhere: a person opens its folder or copies its last errors (Settings, App &
//! updates).
//!
//! **How**: [`log_error!`](crate::log_error), [`log_warn!`](crate::log_warn), [`log_info!`](crate::log_info), written like
//! `eprintln!`, and [`log_failed!`](crate::log_failed) for a result nobody waits for. A value is given after the message
//! (`"{}: {e:#}", n, e = e`): one that is only captured (`"{e}"`) is `…` in the file. The whole line still goes to stderr,
//! where a developer or a worker's parent reads it. Nothing is written until [`open`] (no test ever opens one); then a line
//! is put in a queue, and a thread made for the first one writes it: no thread and no file while all is well.

use std::fmt;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::time::Duration;

/// A file is set aside before it grows past this.
pub const LARGEST: u64 = 2 << 20;
/// How many files there are, with the one being written.
pub const FILES: usize = 5;
/// Nothing in them is older.
pub const DAYS: i64 = 14;
/// A file is set aside once its first line is this old, so that what is older than [`DAYS`] goes a piece at a time.
const DAYS_A_FILE: i64 = 2;
const FILE: &str = "n4n0.log";
/// A line is cut there.
const LONGEST: usize = 1000;
/// What waits to be written; more is counted as lost.
const QUEUE: usize = 512;
/// What one place in the source writes in an hour; then it is counted.
const AN_HOUR: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
        }
    }
}

/// One place in the source that writes to the log.
pub struct Site {
    module: &'static str,
    line: u32,
    text: &'static str,
    /// A line's: the hour (since 1970) above, its count below. A failure's: how many times.
    count: AtomicU64,
}

impl Site {
    pub const fn new(module: &'static str, line: u32, text: &'static str) -> Self {
        Self { module, line, text, count: AtomicU64::new(0) }
    }

    /// Whether a line is written, and what its count adds: the first [`AN_HOUR`] of an hour are, and how many more there
    /// were is said with the next hour's first.
    fn often(&self, hour: u64) -> Option<String> {
        let mut before = 0;
        let _ = self.count.try_update(Ordering::Relaxed, Ordering::Relaxed, |was| {
            before = was;
            Some(if was >> 32 != hour { hour << 32 | 1 } else if was as u32 == u32::MAX { was } else { was + 1 })
        });
        if before >> 32 != hour {
            let more = (before as u32).saturating_sub(AN_HOUR);
            return Some(if more > 0 { format!(" ({more} more before)") } else { String::new() });
        }
        ((before as u32) < AN_HOUR).then(String::new)
    }

    /// Whether a failure is written, and what its count adds: the first, then the 10th, the 100th…
    fn once(&self) -> Option<String> {
        let n = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        let mut power = 1u64;
        while power < n {
            power = power.saturating_mul(10);
        }
        (power == n).then(|| if n == 1 { String::new() } else { format!(" ({n} times)") })
    }

    /// What the file gets of a line: where it is from and the message as it stands in the source, its values as their
    /// types say them.
    pub fn words(&self, values: &[(&'static str, String)]) -> String {
        let said = redacted(&filled(self.text, values)).replace(['\r', '\n'], " ");
        let module = self.module.strip_prefix("orbiters_desktop::").or_else(|| self.module.strip_prefix("orbiters_")).unwrap_or(self.module);
        let mut out = format!("{module}:{} {said}", self.line);
        if let Some((at, _)) = out.char_indices().nth(LONGEST) {
            out.truncate(at);
        }
        out
    }
}

/// `text`, a format string, with each place in it as `values` says it: by name, or one after the other. One that is not
/// among them is `…`.
fn filled(text: &str, values: &[(&'static str, String)]) -> String {
    let mut unnamed = values.iter().filter(|(name, _)| name.is_empty());
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    while let Some(at) = rest.find(['{', '}']) {
        out.push_str(&rest[..at]);
        let mark = rest.as_bytes()[at];
        rest = &rest[at + 1..];
        if rest.as_bytes().first() == Some(&mark) {
            out.push(mark as char);
            rest = &rest[1..];
        } else if mark == b'{' {
            let end = rest.find('}').unwrap_or(rest.len());
            let name = rest[..end].split(':').next().unwrap_or_default().trim();
            let value = match name.parse::<usize>() {
                Ok(n) => values.iter().filter(|(name, _)| name.is_empty()).nth(n),
                Err(_) if name.is_empty() => unnamed.next(),
                Err(_) => values.iter().find(|(n, _)| *n == name),
            };
            out.push_str(value.map_or("…", |(_, said)| said));
            rest = rest.get(end + 1..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    out
}

// What a value's type is known to say.

/// What may be in the file as it is. Never for a text that comes from anywhere but the source.
pub trait Plain {
    fn plain(&self) -> String;
}

macro_rules! as_shown {
    ($($t:ty),* $(,)?) => { $(impl Plain for $t { fn plain(&self) -> String { self.to_string() } })* };
}
as_shown!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, bool, std::process::ExitStatus);

macro_rules! as_debug {
    ($($t:ty),* $(,)?) => { $(impl Plain for $t { fn plain(&self) -> String { format!("{self:?}") } })* };
}
as_debug!(
    Duration,
    std::io::ErrorKind,
    std::num::ParseIntError,
    std::num::ParseFloatError,
    std::str::Utf8Error,
    std::sync::mpsc::RecvError,
    std::sync::mpsc::RecvTimeoutError,
    std::sync::mpsc::TryRecvError,
    std::time::SystemTimeError,
);

macro_rules! as_number {
    ($($t:ty),*) => { $(impl Plain for $t { fn plain(&self) -> String { format!("{self:.3}") } })* };
}
as_number!(f32, f64);

impl Plain for std::string::FromUtf8Error {
    fn plain(&self) -> String {
        self.utf8_error().plain()
    }
}

impl<T> Plain for std::sync::mpsc::SendError<T> {
    fn plain(&self) -> String {
        "nobody receives".into() // i18n: not shown (the log)
    }
}

impl<T> Plain for TrySendError<T> {
    fn plain(&self) -> String {
        match self {
            TrySendError::Full(_) => "the queue is full".into(), // i18n: not shown (the log)
            TrySendError::Disconnected(_) => "nobody receives".into(), // i18n: not shown (the log)
        }
    }
}

impl<T: Plain> Plain for Option<T> {
    fn plain(&self) -> String {
        self.as_ref().map_or_else(|| "none".into(), Plain::plain)
    }
}

macro_rules! as_inside {
    ($($t:ty),*) => { $(impl<T: Plain + ?Sized> Plain for $t { fn plain(&self) -> String { (**self).plain() } })* };
}
as_inside!(&T, &mut T, Box<T>, std::sync::Arc<T>);

/// A text of the source itself (a step's name, a kind), never one made while running.
#[derive(Clone, Copy, Debug)]
pub struct Word(pub &'static str);

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Plain for Word {
    fn plain(&self) -> String {
        self.0.into()
    }
}

/// A place in the app's source.
impl Plain for std::panic::Location<'_> {
    fn plain(&self) -> String {
        format!("{}:{}", source_file(self.file()), self.line())
    }
}

/// A source's path from the crates' folder on (a build's own folder is a person's).
pub fn source_file(file: &str) -> String {
    let file = file.replace('\\', "/");
    match file.rsplit_once("crates/") {
        Some((_, inside)) => inside.into(),
        None => file.rsplit('/').next().unwrap_or_default().into(),
    }
}

impl Plain for std::io::Error {
    fn plain(&self) -> String {
        match (self.raw_os_error(), self.get_ref().and_then(|inside| of_error(inside))) {
            (Some(code), _) => format!("{:?} (os {code})", self.kind()),
            (None, Some(inside)) => format!("{:?}: {inside}", self.kind()),
            (None, None) => format!("{:?}", self.kind()),
        }
    }
}

impl Plain for serde_json::Error {
    fn plain(&self) -> String {
        match self.io_error_kind() {
            Some(kind) => format!("JSON: {kind:?}"),
            None => format!("JSON: {:?} at {}:{}", self.classify(), self.line(), self.column()),
        }
    }
}

impl Plain for ureq::Error {
    fn plain(&self) -> String {
        match self {
            ureq::Error::StatusCode(code) => format!("HTTP {code}"),
            ureq::Error::Io(e) => format!("HTTP: {}", e.plain()),
            ureq::Error::Timeout(_) => "HTTP: timed out".into(), // i18n: not shown (the log)
            // Its kind alone: the rest may hold an address.
            other => format!("HTTP: {}", kind(other)),
        }
    }
}

impl Plain for rusqlite::Error {
    fn plain(&self) -> String {
        match self.sqlite_error_code() {
            Some(code) => format!("SQLite: {code:?}"),
            None => format!("SQLite: {}", kind(self)),
        }
    }
}

#[cfg(windows)]
impl Plain for windows::core::Error {
    fn plain(&self) -> String {
        format!("{:#010X}", self.code().0)
    }
}

/// The kinds and codes of what caused it, from the outside in: never its words.
impl Plain for anyhow::Error {
    fn plain(&self) -> String {
        let mut said: Vec<String> = self.chain().filter_map(of_error).collect();
        said.dedup();
        if said.is_empty() {
            return "an error of words alone".into(); // i18n: not shown (the log)
        }
        said.join(": ")
    }
}

/// The name of an error's kind, as its `Debug` begins.
fn kind(e: &dyn fmt::Debug) -> String {
    format!("{e:?}").chars().take_while(|c| c.is_ascii_alphanumeric()).take(40).collect()
}

/// The kind and code of an error of a type known here.
fn of_error(e: &(dyn std::error::Error + 'static)) -> Option<String> {
    macro_rules! any_of {
        ($($t:ty),* $(,)?) => { $(if let Some(e) = e.downcast_ref::<$t>() { return Some(e.plain()); })* };
    }
    any_of!(std::io::Error, serde_json::Error, ureq::Error, rusqlite::Error, std::num::ParseIntError, std::num::ParseFloatError, std::str::Utf8Error, std::string::FromUtf8Error);
    any_of!(std::sync::mpsc::RecvTimeoutError, std::sync::mpsc::RecvError);
    #[cfg(windows)]
    any_of!(windows::core::Error);
    None
}

/// Where a file is, as far as the app's own folders go: its folder and its name there, and only the kind of what lies
/// deeper (folders and files there are named after worlds, accounts and the person's own things). Any other file: whether
/// it is the person's, and its kind.
impl Plain for Path {
    fn plain(&self) -> String {
        use crate::paths;
        let own = [
            ("cache", paths::cache_dir()),
            ("logs", paths::logs_dir()),
            ("data", paths::local_data_root()),
            ("settings", paths::config_dir()),
            ("documents", paths::documents_root()),
        ];
        let app = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
        let own = own.into_iter().chain(app.map(|dir| ("app", dir)));
        let kind = self.extension().map(|e| e.to_string_lossy().to_lowercase()).filter(|e| e.len() <= 5 && e.bytes().all(|b| b.is_ascii_alphanumeric())).map_or_else(String::new, |e| format!(".{e}"));
        for (name, root) in own {
            let Ok(inside) = self.strip_prefix(&root) else { continue };
            let parts: Vec<String> = inside.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            return match parts.as_slice() {
                [] => format!("<{name}>"),
                [one] if name != "documents" && ours(one) => format!("<{name}>/{one}"),
                [folder, _, ..] if ours(folder) => format!("<{name}>/{folder}/…{kind}"),
                _ => format!("<{name}>/…{kind}"),
            };
        }
        let of_the_person = dirs::home_dir().is_some_and(|home| self.starts_with(home));
        format!("{}/…{kind}", if of_the_person { "~" } else { "" })
    }
}

/// A name the app gives: lower case, hardly a digit (an id, a date and a person's words are not).
fn ours(name: &str) -> bool {
    name.len() <= 40
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_'))
        && name.bytes().filter(u8::is_ascii_digit).count() <= 2
        && !IDS.iter().any(|id| name.contains(id))
}

impl Plain for PathBuf {
    fn plain(&self) -> String {
        self.as_path().plain()
    }
}

impl Plain for std::path::Display<'_> {
    fn plain(&self) -> String {
        Path::new(&self.to_string()).plain()
    }
}

/// A value in a line, by what its type says: [`Plain`]'s, an error's type, or nothing. The macros call `shown()` on
/// `&&&Arg`, and the compiler takes the first of the three there is.
pub struct Arg<'a, T: ?Sized>(pub &'a T);

pub trait ByPlain {
    fn shown(&self) -> String;
}

impl<T: Plain + ?Sized> ByPlain for &&Arg<'_, T> {
    fn shown(&self) -> String {
        self.0.plain()
    }
}

pub trait ByError {
    fn shown(&self) -> String;
}

impl<T: std::error::Error + ?Sized> ByError for &Arg<'_, T> {
    fn shown(&self) -> String {
        let name = std::any::type_name::<T>().trim_start_matches('&');
        let mut said = vec![name.split('<').next().unwrap_or(name).to_string()];
        let mut cause = self.0.source();
        while let Some(e) = cause {
            said.extend(of_error(e));
            cause = e.source();
        }
        said.join(": ")
    }
}

pub trait ByNothing {
    fn shown(&self) -> String;
}

impl<T: ?Sized> ByNothing for Arg<'_, T> {
    fn shown(&self) -> String {
        "…".into()
    }
}

/// What the macros expand to.
#[macro_export]
macro_rules! log_line {
    (@bind $level:ident $text:literal [$(($($name:ident)? , $bound:ident))*] []) => {{
        static SITE: $crate::app_log::Site = $crate::app_log::Site::new(module_path!(), line!(), $text);
        $crate::app_log::line(&SITE, $crate::app_log::Level::$level, format_args!($text $(, $($name =)? $bound)*), &|| {
            #[allow(unused_imports)]
            use $crate::app_log::{ByError as _, ByNothing as _, ByPlain as _};
            vec![$((concat!($(stringify!($name))?), (&&&$crate::app_log::Arg($bound)).shown())),*]
        })
    }};
    (@bind $level:ident $text:literal [$($done:tt)*] [$name:ident = $value:expr $(, $($rest:tt)*)?]) => {
        match &$value { bound => $crate::log_line!(@bind $level $text [$($done)* ($name , bound)] [$($($rest)*)?]) }
    };
    (@bind $level:ident $text:literal [$($done:tt)*] [$value:expr $(, $($rest:tt)*)?]) => {
        match &$value { bound => $crate::log_line!(@bind $level $text [$($done)* (, bound)] [$($($rest)*)?]) }
    };
    ($level:ident $text:literal $(, $($values:tt)*)?) => { $crate::log_line!(@bind $level $text [] [$($($values)*)?]) };
}

/// Something broke, or what a person asked for did not work. Written like `eprintln!` ([`app_log`](crate::app_log)).
#[macro_export]
macro_rules! log_error { ($($line:tt)*) => { $crate::log_line!(Error $($line)*) }; }

/// Something did not work and the app goes on without it.
#[macro_export]
macro_rules! log_warn { ($($line:tt)*) => { $crate::log_line!(Warn $($line)*) }; }

/// Something worth knowing when reading what came after.
#[macro_export]
macro_rules! log_info { ($($line:tt)*) => { $crate::log_line!(Info $($line)*) }; }

/// A result nobody waits for, of something whose failure says something (a file not written, a thread not started): the
/// failure is said once, then with its count at the 10th, the 100th… `what` says what did not work.
#[macro_export]
macro_rules! log_failed {
    ($result:expr, $what:literal $(,)?) => {
        if let Err(failed) = $result {
            static SITE: $crate::app_log::Site = $crate::app_log::Site::new(module_path!(), line!(), $what);
            $crate::app_log::failed(&SITE, &|| {
                #[allow(unused_imports)]
                use $crate::app_log::{ByError as _, ByNothing as _, ByPlain as _};
                (&&&$crate::app_log::Arg(&failed)).shown()
            });
        }
    };
}

/// What the line macros call: the whole line to stderr, what may be kept of it to the log.
pub fn line(site: &'static Site, level: Level, whole: fmt::Arguments<'_>, values: &dyn Fn() -> Vec<(&'static str, String)>) {
    // (Never `eprintln!`: it panics when stderr is closed.)
    let _ = writeln!(std::io::stderr(), "{whole}");
    let Some(sink) = SINK.get() else { return };
    let hour = time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64 / 3600;
    if let Some(count) = site.often(hour) {
        sink.send(level, &format!("{}{count}", site.words(&values())));
    }
}

/// What [`log_failed!`](crate::log_failed) calls.
pub fn failed(site: &'static Site, why: &dyn Fn() -> String) {
    let Some(count) = site.once() else { return };
    let said = format!("{}: {}{count}", site.words(&[]), why());
    let _ = writeln!(std::io::stderr(), "{said}");
    if let Some(sink) = SINK.get() {
        sink.send(Level::Warn, &said);
    }
}

/// A thread's panic ([`crate::panic_log`]): where, and nothing of its message.
pub(crate) fn panicked(place: &crate::panic_log::Place) {
    if let Some(sink) = SINK.get() {
        sink.send(Level::Error, &redacted(&format!("a thread panicked: {}", place.plain())));
    }
}

// The line's way out.

enum Msg {
    Line(String),
    Flush(SyncSender<()>),
}

struct Sink {
    dir: PathBuf,
    role: String,
    tx: OnceLock<Option<SyncSender<Msg>>>,
    lost: AtomicU64,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Lines of this process go to the log in `logs` from now on, as `role`'s (`window`, `overlay`). Once, in `main`; never in
/// a test.
pub fn open(logs: &Path, role: &str) {
    let _ = SINK.set(Sink::new(logs, role));
}

/// The log's folder in `logs`.
pub fn folder(logs: &Path) -> PathBuf {
    logs.join("app")
}

/// Waits until what was said is written, for `most` at the most: before the process ends.
pub fn flush(most: Duration) {
    if let Some(sink) = SINK.get() {
        sink.flush(most);
    }
}

impl Sink {
    fn new(logs: &Path, role: &str) -> Self {
        let known = !role.is_empty() && role.len() <= 24 && role.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
        Self { dir: folder(logs), role: if known { role.into() } else { "app".into() }, tx: OnceLock::new(), lost: AtomicU64::new(0) }
    }

    fn send(&self, level: Level, said: &str) {
        let line = |level: Level, said: &str| Msg::Line(format!("{} {} {} {said}\n", crate::now_utc_iso(), level.name(), self.role));
        let tx = self.tx.get_or_init(|| {
            let (tx, rx) = sync_channel(QUEUE);
            let dir = self.dir.clone();
            std::thread::Builder::new().name("app-log".into()).spawn(move || write_all(&dir, &rx)).ok().map(|_| tx)
        });
        let sent = tx.as_ref().is_some_and(|tx| {
            let lost = self.lost.swap(0, Ordering::Relaxed);
            if lost > 0 && tx.try_send(line(Level::Warn, &format!("{lost} lines were lost"))).is_err() {
                self.lost.fetch_add(lost, Ordering::Relaxed);
            }
            tx.try_send(line(level, said)).is_ok()
        });
        if !sent {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self, most: Duration) {
        let Some(tx) = self.tx.get().and_then(Option::as_ref) else { return };
        let (done, written) = sync_channel(1);
        if tx.try_send(Msg::Flush(done)).is_ok() {
            let _ = written.recv_timeout(most);
        }
    }
}

/// The log's thread: ends with the process.
fn write_all(dir: &Path, rx: &Receiver<Msg>) {
    while let Ok(first) = rx.recv() {
        let (mut lines, mut waiting) = (String::new(), Vec::new());
        for msg in std::iter::once(first).chain(rx.try_iter().take(QUEUE)) {
            match msg {
                Msg::Line(line) => lines.push_str(&line),
                Msg::Flush(done) => waiting.push(done),
            }
        }
        // A log that cannot be written is no reason to fail what was being done.
        let _ = write(dir, &lines, time::OffsetDateTime::now_utc());
        for done in waiting {
            let _ = done.try_send(());
        }
    }
}

/// The `n`th file, 0 the one being written.
fn file(dir: &Path, n: usize) -> PathBuf {
    if n == 0 {
        dir.join(FILE)
    } else {
        dir.join(format!("n4n0.{n}.log"))
    }
}

/// When the first line of a file was written.
fn first(path: &Path) -> Option<time::OffsetDateTime> {
    let mut head = [0; 20];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    time::OffsetDateTime::parse(std::str::from_utf8(&head).ok()?, &time::format_description::well_known::Rfc3339).ok()
}

/// Adds `lines`. The processes of the app share the files: one at a time, through a lock the system lets go of by itself
/// when its process ends.
fn write(dir: &Path, lines: &str, now: time::OffsetDateTime) -> std::io::Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let held = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("n4n0.lock"))?;
    held.lock()?;
    let path = file(dir, 0);
    let grown = |path: &Path| std::fs::metadata(path).map_or(0, |m| m.len());
    let days = |path: &Path| first(path).map(|at| (now - at).whole_days());
    if grown(&path) > 0 && (grown(&path) + lines.len() as u64 > LARGEST || days(&path).is_none_or(|d| d >= DAYS_A_FILE)) {
        let _ = std::fs::remove_file(file(dir, FILES - 1));
        for n in (0..FILES - 1).rev() {
            let _ = std::fs::rename(file(dir, n), file(dir, n + 1));
        }
        if grown(&path) + lines.len() as u64 > LARGEST {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
    }
    for n in 1..FILES {
        let path = file(dir, n);
        if path.exists() && days(&path).is_none_or(|d| d >= DAYS) {
            let _ = std::fs::remove_file(path);
        }
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(lines.as_bytes())
}

/// The newest `most` errors and warnings of the log in `logs`, the oldest of them first.
pub fn last_errors(logs: &Path, most: usize) -> Vec<String> {
    let dir = folder(logs);
    let mut out: Vec<String> = Vec::new();
    for n in 0..FILES {
        let text = std::fs::read(file(&dir, n)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let told = |l: &&str| matches!(l.split(' ').nth(1), Some("error" | "warn"));
        out.extend(text.lines().rev().filter(told).take(most - out.len()).map(str::to_string));
        if out.len() >= most {
            break;
        }
    }
    out.reverse();
    out
}

// The second look, for what would get through a mistake.

const IDS: &[&str] = &[
    "usr_", "wrld_", "grp_", "avtr_", "file_", "inst_", "not_", "frq_", "prnt_", "inv_", "gmem_", "gpos_", "authcookie_", "eyJ", "gho_", "ghp_", "github_pat_", "sk-",
];

/// `text` without what looks like a token, an id, an address beyond its server, a person's folder, or a line of VRChat's log.
pub fn redacted(text: &str) -> String {
    let text = without_folders(text);
    if let Some(at) = vrchat_line(&text) {
        return format!("{}(a line of VRChat's log)", redacted(&text[..at])); // i18n: not shown (the log)
    }
    let parting = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')' | '<' | '>' | '[' | ']' | '{' | '}' | ',' | ';' | '|');
    let mut out = String::with_capacity(text.len());
    let mut secret = false;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let (word, after) = rest.split_at(rest.find(parting).unwrap_or(rest.len()));
        if !word.is_empty() {
            let name = word.trim_end_matches([':', '=']).to_ascii_lowercase();
            let scheme = matches!(name.as_str(), "bearer" | "basic");
            if secret && !scheme {
                out.push('…');
                secret = false;
            } else {
                out.push_str(&word_redacted(word));
                // `Bearer x`, `cookie: x`: what follows.
                secret = scheme || (word.len() > name.len() && matches!(name.as_str(), "token" | "cookie" | "set-cookie" | "auth" | "authorization" | "password" | "secret" | "key"));
            }
        }
        let parted = after.chars().next().map_or(0, char::len_utf8);
        out.push_str(&after[..parted]);
        rest = &after[parted..];
    }
    out
}

/// A person's own folder as `~`: this one's, and anyone's (another account's, another computer's text).
fn without_folders(text: &str) -> String {
    let mut text = text.to_string();
    if let Some(home) = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).filter(|h| h.len() > 3) {
        let back = home.replace('/', "\\");
        for home in [home.replace('\\', "/"), back.replace('\\', "\\\\"), back] {
            let mut from = 0;
            while let Some(at) = text[from..].to_ascii_lowercase().find(&home.to_ascii_lowercase()).map(|at| from + at) {
                text.replace_range(at..at + home.len(), "~");
                from = at + 1;
            }
        }
    }
    for users in ["\\Users\\", "\\\\Users\\\\", "/Users/", "/home/"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(users).map(|at| from + at) {
            let name = at + users.len();
            let end = text[name..].find(['\\', '/', '"', '\'', ')', ',', '\n']).map_or(text.len(), |n| name + n);
            // (With its disk: `D:`, `\\?\C:`.)
            let start = text[..at].rfind(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '(' | '`')).map_or(0, |s| s + text[s..].chars().next().map_or(1, char::len_utf8));
            text.replace_range(start..end, "~");
            from = start + 1;
        }
    }
    text
}

/// Where a line of VRChat's own log begins in `text`: `2026.11.25 21:24:42`.
fn vrchat_line(text: &str) -> Option<usize> {
    let shape = b"0000.00.00 00:00:00";
    text.as_bytes().windows(shape.len()).position(|w| w.iter().zip(shape).all(|(c, s)| if *s == b'0' { c.is_ascii_digit() } else { c == s }))
}

fn word_redacted(word: &str) -> String {
    if let Some((scheme, rest)) = word.split_once("://") {
        // The server alone: a path names what was looked at, a query may hold a key, what is before an `@` a password.
        let whole = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let server = whole.rsplit('@').next().unwrap_or_default();
        return format!("{scheme}://{server}{}", if rest.len() > whole.len() { "/…" } else { "" });
    }
    if word.contains('@') && word.contains('.') {
        return "…".into();
    }
    // An id at the start of a word or after a sign (`not_a_file_x` has none in its middle).
    if let Some(at) = IDS.iter().filter_map(|id| word.find(id)).filter(|at| *at == 0 || !word.as_bytes()[at - 1].is_ascii_alphanumeric() && word.as_bytes()[at - 1] != b'_').min() {
        return format!("{}…", &word[..at]);
    }
    if let Some((name, value)) = word.split_once('=') {
        if !value.is_empty() && value.parse::<f64>().is_err() {
            return format!("{name}=…");
        }
    }
    // A key: long, of one piece, with digits in it.
    let long = |part: &str| part.len() >= 20 && part.bytes().any(|b| b.is_ascii_digit()) && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+'));
    if word.split(['/', '\\', '.', ':']).any(long) {
        return "…".into();
    }
    word.into()
}

#[cfg(test)]
#[path = "app_log_tests.rs"]
mod tests;
