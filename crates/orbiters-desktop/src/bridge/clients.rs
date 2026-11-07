//! Adding the bridge to an agent in one click: Claude Code and Codex, for this user, so that a new session has it with nothing
//! else to do. Each client keeps its own list of servers and has its own command for it, so that is what runs here (`claude mcp`
//! and `codex mcp`, from the home folder, without a shell), and what Settings shows is what the client itself answers, never
//! a memory of having added it.
//!
//! **Where the token goes.** Into each client's own file in this user's profile (`~/.claude.json`, `~/.codex/config.toml`), where
//! those clients keep every other server's credentials, and which this user, the administrators and the system can read (Codex's
//! also its own sandbox's group). Nothing here starts a shell, so it reaches no history, and neither file is in a repository. It is on the
//! command line of `claude mcp add` for the half second that runs, where only this user's own programs and administrators could
//! read it: Claude Code takes a header no other way short of writing its file by hand.
//!
//! **Why Codex's file is edited by hand, the one exception.** `codex mcp add` takes a URL and `--bearer-token-env-var`, and nothing
//! else for a token. An environment variable is no good here: set for this user, it reaches only programs started after it, so a
//! terminal or an editor already open would start Codex without it, and the server would fail with nothing to say why. A header
//! means a line in its file anyway, and Codex's own `mcp add` and `mcp remove` turned out to rewrite all of its servers: on Enzo's
//! file (0.166.0, 2026-11-06) they dropped a marker comment another tool keeps there, every `enabled = true` and `args = []`, and
//! reordered environment variables. So the entry is one block at the end of the file, and taking it away takes that block away,
//! with every other byte as it was. Codex still decides: `codex mcp get --json` reads back what it understood, and a change it
//! does not show is a failure.
//!
//! **Staying right.** A token made anew, or the port moving because another program has it, changes what a client must
//! present. The keeper asks for [`Registrations::repair`] each time the bridge opens and after a new token: an entry with old values is
//! rewritten, and the log and Settings say so, with what cannot be fixed from here: a session already open keeps what it read.

use serde::{Deserialize, Serialize};
use orbiters_i18n::tr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the bridge is called in each client: its tools are `mcp__orbiters-vrchat__…` in Claude Code.
pub const SERVER_NAME: &str = "orbiters-vrchat";
/// The longest a client's command may take. They take under a second; a client updating itself takes longer.
const COMMAND_LIMIT: Duration = Duration::from_secs(45);
/// How soon a look at the clients may follow the last one: each starts a process, and Claude Code's connects to the server.
const CHECK_AT_MOST_EVERY: Duration = Duration::from_secs(10);

/// An agent the bridge can be added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    ClaudeCode,
    Codex,
}

impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::Codex => "Codex",
        }
    }
}

/// What a person can ask of a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Adds the bridge, or replaces what the client has with what is right now.
    Add,
    Remove,
}

/// What of a client's entry is out of date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Old {
    Address,
    Token,
    /// Both.
    Everything,
}

/// A client's state, as the client itself says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Registration {
    /// Not asked yet.
    #[default]
    Unknown,
    /// The program was not found on this computer.
    NotInstalled,
    NotAdded,
    Added,
    /// Added, with an address or token that no longer works.
    Outdated(Old),
    /// Added with the right values, and the client says it cannot connect.
    NotConnecting(String),
    /// The client did not answer in a way that says.
    Unreadable(String),
}

/// A client as Settings shows it. The token is not in here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientStatus {
    pub client: Client,
    pub registration: Registration,
    /// A command of it is running.
    pub busy: bool,
    /// What the last change did, in a sentence.
    pub note: Option<String>,
    /// That last change failed.
    pub failed: bool,
}

/// What a client has under [`SERVER_NAME`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub address: Option<String>,
    /// What follows `Bearer ` in its `Authorization` header.
    pub token: Option<String>,
    /// The client tried to connect, and this is what it says went wrong.
    pub problem: Option<String>,
    /// It is in a scope that only holds for one folder, not this user's.
    pub not_for_this_user: bool,
}

/// One client's own commands.
pub trait ClientCli: Send + Sync {
    fn client(&self) -> Client;
    fn installed(&self) -> bool;
    /// What it has under [`SERVER_NAME`]: `None`, nothing.
    fn read(&self) -> Result<Option<Entry>, String>;
    /// Makes the entry. There is none when this is called.
    fn add(&self, address: &str, token: &str) -> Result<(), String>;
    /// Takes the entry away. There being none is no failure.
    fn remove(&self) -> Result<(), String>;
}

/// What the entry should say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub address: String,
    pub token: String,
    /// The bridge is listening: only then does a failed connection say something about the entry.
    pub listening: bool,
}

/// What a client's answer means.
pub fn classify(entry: Option<&Entry>, wanted: &Wanted) -> Registration {
    let Some(entry) = entry else { return Registration::NotAdded };
    let address_ok = entry.address.as_deref() == Some(wanted.address.as_str()) && !entry.not_for_this_user;
    let token_ok = entry.token.as_deref() == Some(wanted.token.as_str());
    match (address_ok, token_ok) {
        (false, false) => return Registration::Outdated(Old::Everything),
        (false, true) => return Registration::Outdated(Old::Address),
        (true, false) => return Registration::Outdated(Old::Token),
        (true, true) => {}
    }
    match entry.problem.as_ref().filter(|_| wanted.listening) {
        Some(problem) => Registration::NotConnecting(problem.clone()),
        None => Registration::Added,
    }
}

/// `text` with the token out of it: what a client says can end up on the screen and in the log.
fn scrub(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_string()
    } else {
        text.replace(token, "<token>")
    }
}

/// The first line of a client's complaint that says anything, without its token.
fn complaint(said: &str, token: &str) -> String {
    let line = said.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("WARNING")).unwrap_or_else(|| tr("It gave no reason."));
    scrub(line, token).chars().take(300).collect()
}

/// Reads `claude mcp get`'s answer. Its `Status` comes from really connecting, which is why this is asked rather than its file.
pub fn parse_claude_get(said: &str) -> Entry {
    let mut entry = Entry::default();
    let mut status_failed = None;
    let mut issue = None;
    for line in said.lines().map(str::trim) {
        if let Some(scope) = line.strip_prefix("Scope:") {
            entry.not_for_this_user = !scope.trim_start().starts_with("User");
        } else if let Some(status) = line.strip_prefix("Status:") {
            let status = status.trim();
            if !status.contains("Connected") {
                status_failed = Some(status.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).to_string());
            }
        } else if let Some(text) = line.strip_prefix("Issue:") {
            issue = Some(text.trim().to_string());
        } else if let Some(url) = line.strip_prefix("URL:") {
            entry.address = Some(url.trim().to_string());
        } else if let Some(token) = line.strip_prefix("Authorization: Bearer ") {
            entry.token = Some(token.trim().to_string());
        }
    }
    entry.problem = status_failed.map(|status| issue.unwrap_or(status));
    entry
}

/// Reads `codex mcp get --json`'s answer.
pub fn parse_codex_get(said: &str) -> Result<Entry, String> {
    let json: serde_json::Value = serde_json::from_str(said).map_err(|_| tr("Codex's answer could not be read.").to_string())?;
    let transport = &json["transport"];
    let token = transport["http_headers"]["Authorization"].as_str().and_then(|h| h.strip_prefix("Bearer ")).map(str::to_string); // i18n: not shown
    let disabled = json["enabled"].as_bool() == Some(false);
    Ok(Entry {
        address: transport["url"].as_str().map(str::to_string),
        token,
        problem: disabled.then(|| tr("It is turned off in Codex's settings.").to_string()),
        not_for_this_user: false,
    })
}

/// `config` (a Codex `config.toml`) without the bridge's entry. Only its own lines go: a comment there may be another tool's.
pub fn without_codex_entry(config: &str) -> String {
    let table = format!("[mcp_servers.{SERVER_NAME}]");
    let inner = format!("[mcp_servers.{SERVER_NAME}.");
    let mut kept = String::with_capacity(config.len());
    let mut ours = false;
    let mut pending_blank = String::new();
    for line in config.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            ours = trimmed == table || trimmed.starts_with(&inner);
            if ours {
                // The empty line that set it apart goes with it.
                pending_blank.clear();
                continue;
            }
        }
        if ours && !trimmed.starts_with('#') {
            continue;
        }
        if trimmed.is_empty() {
            pending_blank.push_str(line);
            continue;
        }
        kept.push_str(&pending_blank);
        pending_blank.clear();
        kept.push_str(line);
    }
    kept.push_str(&pending_blank);
    kept
}

/// `config` with the bridge's entry at its end, in place of any it had, and every other byte as it was. Line ends as the file has them.
pub fn with_codex_entry(config: &str, address: &str, token: &str) -> String {
    let mut text = without_codex_entry(config);
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(newline);
    }
    if !text.is_empty() {
        text.push_str(newline);
    }
    text.push_str(&format!("[mcp_servers.{SERVER_NAME}]{newline}url = \"{address}\"{newline}http_headers = {{ Authorization = \"Bearer {token}\" }}{newline}"));
    text
}

/// What a command said.
struct Ran {
    success: bool,
    out: String,
    err: String,
}

/// Runs a client's command, without a window and without a shell, from the home folder: a server added for one folder is that
/// folder's, and this is about this user's.
fn run(exe: &Path, args: &[&str], home: Option<&Path>, environment: &[(&str, &std::ffi::OsStr)], limit: Duration) -> Result<Ran, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut command = Command::new(exe);
    command.args(args).envs(environment.iter().copied()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(home) = home {
        command.current_dir(home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| tr!("It could not be started: {error}", error = e))?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = sender.send(bytes);
        });
        receiver
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let begin = Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if begin.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(tr!("It did not answer within {seconds} s.", seconds = limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(tr!("It could not be followed: {error}", error = e)),
        }
    };
    // A helper it left behind may hold a pipe open: what was said by now is what counts.
    let text = |receiver: std::sync::mpsc::Receiver<Vec<u8>>| String::from_utf8_lossy(&receiver.recv_timeout(Duration::from_secs(2)).unwrap_or_default()).into_owned();
    Ok(Ran { success, out: text(out), err: text(err) })
}

/// A program's place: on `PATH`, else where its installer puts it (a tray service started at sign-in may have an older `PATH`
/// than a terminal opened now).
fn find_program(name: &str, known: &[PathBuf]) -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    on_path.flat_map(|dir| [dir.join(format!("{name}.exe")), dir.join(format!("{name}.cmd"))]).chain(known.iter().cloned()).find(|p| p.is_file())
}

/// A folder of this user's that Windows names in the environment.
fn named_folder(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> Option<PathBuf> {
    named_folder("USERPROFILE")
}

/// Claude Code's `claude mcp`.
pub struct ClaudeCli {
    /// Its `CLAUDE_CONFIG_DIR`, for the test that runs the real one with its files elsewhere.
    pub config_dir: Option<PathBuf>,
}

impl ClaudeCli {
    fn program(&self) -> Option<PathBuf> {
        let mut known = Vec::new();
        if let Some(home) = home() {
            known.push(home.join(".local").join("bin").join("claude.exe"));
            known.push(home.join(".claude").join("local").join("claude.cmd"));
        }
        if let Some(data) = named_folder("APPDATA") {
            known.push(data.join("npm").join("claude.cmd"));
        }
        find_program("claude", &known)
    }

    fn run(&self, args: &[&str]) -> Result<Ran, String> {
        let exe = self.program().ok_or_else(|| tr("Claude Code is not installed on this computer."))?;
        let mut environment = self.config_dir.iter().map(|d| ("CLAUDE_CONFIG_DIR", d.as_os_str())).collect::<Vec<_>>();
        // `mcp get` connects to say how it went: 30 s by default when something else holds the old port and never answers, measured,
        // twice for a repair. The bridge answers at once, so a short wait says the same.
        environment.push(("MCP_TIMEOUT", std::ffi::OsStr::new("5000")));
        run(&exe, args, home().as_deref(), &environment, COMMAND_LIMIT)
    }
}

impl ClientCli for ClaudeCli {
    fn client(&self) -> Client {
        Client::ClaudeCode
    }

    fn installed(&self) -> bool {
        self.program().is_some()
    }

    fn read(&self) -> Result<Option<Entry>, String> {
        let ran = self.run(&["mcp", "get", SERVER_NAME])?;
        if ran.success {
            return Ok(Some(parse_claude_get(&ran.out)));
        }
        let said = format!("{}\n{}", ran.err, ran.out);
        if said.contains("No MCP server") {
            return Ok(None);
        }
        Err(complaint(&said, ""))
    }

    fn add(&self, address: &str, token: &str) -> Result<(), String> {
        let header = format!("Authorization: Bearer {token}"); // i18n: not shown
        let ran = self.run(&["mcp", "add", "--scope", "user", "--transport", "http", SERVER_NAME, address, "--header", &header])?;
        if ran.success {
            Ok(())
        } else {
            Err(complaint(&format!("{}\n{}", ran.err, ran.out), token))
        }
    }

    fn remove(&self) -> Result<(), String> {
        for scope in ["user", "local"] {
            let ran = self.run(&["mcp", "remove", SERVER_NAME, "--scope", scope])?;
            let said = format!("{}\n{}", ran.err, ran.out);
            if !ran.success && !said.contains("No MCP server") {
                return Err(complaint(&said, ""));
            }
        }
        Ok(())
    }
}

/// Codex's `codex mcp`, and the one line of its settings it has no flag for.
pub struct CodexCli {
    /// Its `CODEX_HOME`, for the test that runs the real one with its files elsewhere.
    pub home: Option<PathBuf>,
}

impl CodexCli {
    fn program(&self) -> Option<PathBuf> {
        let mut known = Vec::new();
        if let Some(local) = named_folder("LOCALAPPDATA") {
            known.push(local.join("Programs").join("OpenAI").join("Codex").join("bin").join("codex.exe"));
        }
        if let Some(data) = named_folder("APPDATA") {
            known.push(data.join("npm").join("codex.cmd"));
        }
        find_program("codex", &known)
    }

    /// The settings file Codex reads: `CODEX_HOME`, else `.codex` in the home folder.
    fn config_file(&self) -> Option<PathBuf> {
        let dir = self.home.clone().or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from)).or_else(|| home().map(|h| h.join(".codex")))?;
        Some(dir.join("config.toml"))
    }

    fn run(&self, args: &[&str]) -> Result<Ran, String> {
        let exe = self.program().ok_or_else(|| tr("Codex is not installed on this computer."))?;
        let environment = self.home.iter().map(|h| ("CODEX_HOME", h.as_os_str())).collect::<Vec<_>>();
        run(&exe, args, home().as_deref(), &environment, COMMAND_LIMIT)
    }
}

impl ClientCli for CodexCli {
    fn client(&self) -> Client {
        Client::Codex
    }

    fn installed(&self) -> bool {
        self.program().is_some()
    }

    fn read(&self) -> Result<Option<Entry>, String> {
        let ran = self.run(&["mcp", "get", SERVER_NAME, "--json"])?;
        if ran.success {
            return parse_codex_get(&ran.out).map(Some);
        }
        if ran.err.contains("No MCP server") || ran.out.contains("No MCP server") {
            return Ok(None);
        }
        Err(complaint(&format!("{}\n{}", ran.err, ran.out), ""))
    }

    fn add(&self, address: &str, token: &str) -> Result<(), String> {
        self.change_config(|config| with_codex_entry(config, address, token))
    }

    fn remove(&self) -> Result<(), String> {
        self.change_config(without_codex_entry)
    }
}

impl CodexCli {
    /// Changes its settings file: beside it and renamed over it, so that a Codex reading it finds either whole file; made in the same
    /// folder, it gets that folder's permissions, as the file it replaces had. Nothing to change writes nothing.
    fn change_config(&self, change: impl FnOnce(&str) -> String) -> Result<(), String> {
        let file = self.config_file().ok_or_else(|| tr("Codex's settings file was not found."))?;
        let config = match std::fs::read_to_string(&file) {
            Ok(config) => config,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(tr!("Codex's settings could not be read: {error}", error = e)),
        };
        let changed = change(&config);
        if changed == config {
            return Ok(());
        }
        let temporary = file.with_extension("toml.orbiters-tmp");
        std::fs::write(&temporary, changed).map_err(|e| tr!("Codex's settings could not be written: {error}", error = e))?;
        std::fs::rename(&temporary, &file).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            tr!("Codex's settings could not be written: {error}", error = e)
        })
    }
}

struct Slot {
    cli: Arc<dyn ClientCli>,
    status: ClientStatus,
}

/// The clients as they stand, and the one command at a time each of them runs, on threads of their own: Settings never waits.
pub struct Registrations {
    slots: Mutex<Vec<Slot>>,
    last_check: Mutex<Option<Instant>>,
    /// Where what happened is told, besides Settings.
    log: Box<dyn Fn(bool, String) + Send + Sync>,
    /// Off in the tests that look at each step.
    threads: AtomicBool,
}

/// What a step ends with: the state, the sentence for it, and whether that sentence is a failure.
struct Outcome {
    registration: Registration,
    note: Option<String>,
    failed: bool,
}

impl Registrations {
    pub fn new(clis: Vec<Arc<dyn ClientCli>>, log: Box<dyn Fn(bool, String) + Send + Sync>) -> Arc<Self> {
        let slots = clis
            .into_iter()
            .map(|cli| {
                let client = cli.client();
                Slot { cli, status: ClientStatus { client, registration: Registration::Unknown, busy: false, note: None, failed: false } }
            })
            .collect();
        Arc::new(Self { slots: Mutex::new(slots), last_check: Mutex::new(None), log, threads: AtomicBool::new(true) })
    }

    /// Claude Code and Codex, for real.
    pub fn real(log: Box<dyn Fn(bool, String) + Send + Sync>) -> Arc<Self> {
        Self::new(vec![Arc::new(ClaudeCli { config_dir: None }), Arc::new(CodexCli { home: None })], log)
    }

    pub fn statuses(&self) -> Vec<ClientStatus> {
        self.slots.lock().unwrap().iter().map(|s| s.status.clone()).collect()
    }

    /// Takes the turn of a client, or of all of them: those with a command running are left out.
    fn begin(&self, client: Option<Client>) -> Vec<Arc<dyn ClientCli>> {
        let mut slots = self.slots.lock().unwrap();
        slots
            .iter_mut()
            .filter(|s| client.is_none_or(|c| c == s.status.client) && !s.status.busy)
            .map(|s| {
                s.status.busy = true;
                s.cli.clone()
            })
            .collect()
    }

    fn end(&self, client: Client, outcome: Outcome) {
        let mut slots = self.slots.lock().unwrap();
        if let Some(slot) = slots.iter_mut().find(|s| s.status.client == client) {
            slot.status.busy = false;
            slot.status.registration = outcome.registration;
            // A plain look leaves the last sentence as it was.
            if outcome.note.is_some() {
                slot.status.note = outcome.note;
                slot.status.failed = outcome.failed;
            }
        }
    }

    fn go(self: &Arc<Self>, step: impl FnOnce() + Send + 'static) {
        if self.threads.load(Ordering::Relaxed) {
            let _ = std::thread::Builder::new().name("bridge-client".into()).spawn(step);
        } else {
            step();
        }
    }

    /// Looks at what each client has. Often asked (each visit of Settings), so at most so often.
    pub fn check(self: &Arc<Self>, wanted: Wanted) {
        {
            let mut last = self.last_check.lock().unwrap();
            if last.is_some_and(|at| at.elapsed() < CHECK_AT_MOST_EVERY) {
                return;
            }
            *last = Some(Instant::now());
        }
        for cli in self.begin(None) {
            let (this, wanted) = (self.clone(), wanted.clone());
            self.go(move || {
                let outcome = Self::look(cli.as_ref(), &wanted);
                this.end(cli.client(), outcome);
            });
        }
    }

    fn look(cli: &dyn ClientCli, wanted: &Wanted) -> Outcome {
        if !cli.installed() {
            return Outcome { registration: Registration::NotInstalled, note: None, failed: false };
        }
        match cli.read() {
            Ok(entry) => Outcome { registration: classify(entry.as_ref(), wanted), note: None, failed: false },
            Err(why) => Outcome { registration: Registration::Unreadable(why), note: None, failed: false },
        }
    }

    /// What a person pressed.
    pub fn change(self: &Arc<Self>, client: Client, change: Change, wanted: Wanted) {
        for cli in self.begin(Some(client)) {
            let (this, wanted) = (self.clone(), wanted.clone());
            self.go(move || {
                let outcome = match change {
                    Change::Add => Self::add_or_replace(cli.as_ref(), &wanted),
                    Change::Remove => Self::take_away(cli.as_ref(), &wanted),
                };
                if let Some(note) = &outcome.note {
                    (this.log)(!outcome.failed, format!("{}: {note}", cli.client().name()));
                }
                this.end(cli.client(), outcome);
            });
        }
    }

    /// The address or the token changed, or the bridge just opened: an entry with old values is written again, as a person would
    /// otherwise have to find out why their agent stopped working. What has no entry stays without one.
    pub fn repair(self: &Arc<Self>, wanted: Wanted) {
        for cli in self.begin(None) {
            let (this, wanted) = (self.clone(), wanted.clone());
            self.go(move || {
                let looked = Self::look(cli.as_ref(), &wanted);
                let outcome = if matches!(looked.registration, Registration::Outdated(_)) {
                    let mut outcome = Self::add_or_replace(cli.as_ref(), &wanted);
                    if !outcome.failed {
                        outcome.note = Some(tr!("Its address or token was old, and is updated now. Restart any {name} session already open.", name = cli.client().name()));
                    }
                    if let Some(note) = &outcome.note {
                        (this.log)(!outcome.failed, format!("{}: {note}", cli.client().name()));
                    }
                    outcome
                } else {
                    looked
                };
                this.end(cli.client(), outcome);
            });
        }
    }

    fn add_or_replace(cli: &dyn ClientCli, wanted: &Wanted) -> Outcome {
        let name = cli.client().name();
        let failed = |registration, why: String| Outcome { registration, note: Some(tr!("It was not added: {reason}", reason = why)), failed: true };
        if !cli.installed() {
            return Outcome { registration: Registration::NotInstalled, note: None, failed: false };
        }
        let before = match cli.read() {
            Ok(before) => before,
            Err(why) => return failed(Registration::Unreadable(why.clone()), why),
        };
        if before.is_some() {
            if let Err(why) = cli.remove() {
                return failed(classify(before.as_ref(), wanted), why);
            }
        }
        if let Err(why) = cli.add(&wanted.address, &wanted.token) {
            return failed(Self::look(cli, wanted).registration, why);
        }
        // Believed when the client says it.
        match cli.read() {
            Ok(after) => {
                let registration = classify(after.as_ref(), wanted);
                match &registration {
                    Registration::Added => Outcome {
                        registration,
                        note: Some(if before.is_some() {
                            tr!("Updated. Restart any {name} session already open.", name = name)
                        } else {
                            tr!("A new {name} session has it; one already open has to be restarted.", name = name)
                        }),
                        failed: false,
                    },
                    Registration::NotConnecting(why) => {
                        Outcome { note: Some(tr!("It was added, and {name} cannot connect: {reason}", name = name, reason = why)), registration, failed: true }
                    }
                    _ => failed(registration, tr!("{name} does not show what was added.", name = name)),
                }
            }
            Err(why) => failed(Registration::Unreadable(why.clone()), why),
        }
    }

    fn take_away(cli: &dyn ClientCli, wanted: &Wanted) -> Outcome {
        match cli.remove() {
            Ok(()) => {
                let looked = Self::look(cli, wanted);
                if looked.registration == Registration::NotAdded {
                    Outcome { note: Some(tr("Removed.").into()), ..looked }
                } else {
                    Outcome { note: Some(tr!("{name} still shows it.", name = cli.client().name())), failed: true, ..looked }
                }
            }
            Err(why) => Outcome { note: Some(tr!("It was not removed: {reason}", reason = why)), failed: true, ..Self::look(cli, wanted) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_given_up_on_says_so_in_the_language_now() {
        use orbiters_i18n::{in_language, Language};
        if !cfg!(windows) {
            return;
        }
        let said = |language| in_language(language, || run(Path::new("cmd"), &["/c", "ping -n 30 127.0.0.1 >nul"], None, &[], Duration::from_secs(1)).err().expect("it hangs"));
        assert_eq!(said(Language::En), "It did not answer within 1 s.");
        assert_eq!(said(Language::Fr), "Il n’a pas répondu en 1\u{a0}s.");
        assert_eq!(said(Language::Ja), "1秒以内に応答がありませんでした。");
    }

    const WANTED: (&str, &str) = ("http://127.0.0.1:47615/mcp", "goodtoken");

    fn wanted() -> Wanted {
        Wanted { address: WANTED.0.into(), token: WANTED.1.into(), listening: true }
    }

    /// A client's list of servers, in memory, whose steps can fail.
    #[derive(Default)]
    struct Fake {
        missing: bool,
        entry: Mutex<Option<Entry>>,
        add_fails: Option<&'static str>,
        remove_fails: Option<&'static str>,
        read_fails: Option<&'static str>,
        /// What it answers about connecting.
        problem: Option<&'static str>,
        /// The add seems to work, and nothing is there after it.
        add_loses_it: bool,
        calls: Mutex<Vec<String>>,
    }

    impl ClientCli for Fake {
        fn client(&self) -> Client {
            Client::Codex
        }
        fn installed(&self) -> bool {
            !self.missing
        }
        fn read(&self) -> Result<Option<Entry>, String> {
            self.calls.lock().unwrap().push("read".into());
            if let Some(why) = self.read_fails {
                return Err(why.into());
            }
            Ok(self.entry.lock().unwrap().clone())
        }
        fn add(&self, address: &str, token: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push("add".into());
            if let Some(why) = self.add_fails {
                return Err(why.into());
            }
            if !self.add_loses_it {
                *self.entry.lock().unwrap() =
                    Some(Entry { address: Some(address.into()), token: Some(token.into()), problem: self.problem.map(str::to_string), not_for_this_user: false });
            }
            Ok(())
        }
        fn remove(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("remove".into());
            if let Some(why) = self.remove_fails {
                return Err(why.into());
            }
            *self.entry.lock().unwrap() = None;
            Ok(())
        }
    }

    type Logged = Arc<Mutex<Vec<(bool, String)>>>;

    fn registrations(fake: Arc<Fake>) -> (Arc<Registrations>, Logged) {
        let logged = Arc::new(Mutex::new(Vec::new()));
        let sink = logged.clone();
        let this = Registrations::new(vec![fake], Box::new(move |ok, line| sink.lock().unwrap().push((ok, line))));
        this.threads.store(false, Ordering::Relaxed);
        (this, logged)
    }

    fn entry(address: &str, token: &str) -> Option<Entry> {
        Some(Entry { address: Some(address.into()), token: Some(token.into()), ..Default::default() })
    }

    #[test]
    fn what_a_client_has_says_which_state_it_is_in() {
        let w = wanted();
        assert_eq!(classify(None, &w), Registration::NotAdded);
        assert_eq!(classify(entry(WANTED.0, WANTED.1).as_ref(), &w), Registration::Added);
        assert_eq!(classify(entry(WANTED.0, "old").as_ref(), &w), Registration::Outdated(Old::Token));
        assert_eq!(classify(entry("http://127.0.0.1:1/mcp", WANTED.1).as_ref(), &w), Registration::Outdated(Old::Address));
        assert_eq!(classify(entry("http://127.0.0.1:1/mcp", "old").as_ref(), &w), Registration::Outdated(Old::Everything));
        // An entry with an environment variable for a token has none here: it is replaced, as it fails in what started before it.
        assert_eq!(classify(Some(&Entry { address: Some(WANTED.0.into()), ..Default::default() }), &w), Registration::Outdated(Old::Token));
    }

    #[test]
    fn a_failed_connection_only_counts_while_the_bridge_listens() {
        let failing = Entry { address: Some(WANTED.0.into()), token: Some(WANTED.1.into()), problem: Some("refused".into()), not_for_this_user: false };
        assert_eq!(classify(Some(&failing), &wanted()), Registration::NotConnecting("refused".into()));
        assert_eq!(classify(Some(&failing), &Wanted { listening: false, ..wanted() }), Registration::Added);
    }

    #[test]
    fn claude_codes_answers_are_read() {
        // As Claude Code 2.1.318 says it.
        let good = "orbiters-vrchat:\n  Scope: User config (available in all your projects)\n  Status: \u{2714} Connected\n  Type: http\n  URL: http://127.0.0.1:47615/mcp\n  Headers:\n    Authorization: Bearer abc\n\nTo remove this server, run: claude mcp remove orbiters-vrchat -s user\n";
        let entry = parse_claude_get(good);
        assert_eq!(entry.address.as_deref(), Some("http://127.0.0.1:47615/mcp"));
        assert_eq!(entry.token.as_deref(), Some("abc"));
        assert_eq!(entry.problem, None);
        assert!(!entry.not_for_this_user);
        let refused = "orbiters-vrchat:\n  Scope: User config (available in all your projects)\n  Status: \u{2718} Failed to connect\n  Issue: ECONNREFUSED: Unable to connect. Is the computer able to access the url?\n  Type: http\n  URL: http://127.0.0.1:1/mcp\n  Headers:\n    Authorization: Bearer wrong\n";
        assert!(parse_claude_get(refused).problem.unwrap().starts_with("ECONNREFUSED"));
        // Failing with nothing more to say says its status, without its mark.
        assert_eq!(parse_claude_get("  Status: \u{2718} Failed to connect\n").problem.as_deref(), Some("Failed to connect"));
        // A server added for one folder is not this user's.
        assert!(parse_claude_get("  Scope: Local config (private to you in this project)\n").not_for_this_user);
    }

    #[test]
    fn codexs_answers_are_read() {
        let json = r#"{"name":"orbiters-vrchat","enabled":true,"transport":{"type":"streamable_http","url":"http://127.0.0.1:47615/mcp","bearer_token_env_var":null,"http_headers":{"Authorization":"Bearer abc"}}}"#;
        let entry = parse_codex_get(json).unwrap();
        assert_eq!((entry.address.as_deref(), entry.token.as_deref(), entry.problem), (Some("http://127.0.0.1:47615/mcp"), Some("abc"), None));
        let by_variable = r#"{"enabled":true,"transport":{"url":"http://x/mcp","bearer_token_env_var":"T","http_headers":null}}"#;
        assert_eq!(parse_codex_get(by_variable).unwrap().token, None);
        assert!(parse_codex_get(r#"{"enabled":false,"transport":{}}"#).unwrap().problem.is_some());
        assert!(parse_codex_get("not json").is_err());
    }

    #[test]
    fn codexs_entry_comes_and_goes_with_every_other_byte_as_it_was() {
        // As Enzo's file has it: what Codex's own `mcp add` took away from it is the point of this.
        let config = "# mine\nmodel = \"x\"\n\n[mcp_servers.other]\nenabled = true\nargs = []\nhttp_headers = { A = \"b\" }\n\n# <<< a tool's block <<<\n[mcp_servers.heroui]\nargs = [ \"-y\" ]\n";
        let added = with_codex_entry(config, "http://a/mcp", "tok");
        assert_eq!(
            added,
            format!("{config}\n[mcp_servers.orbiters-vrchat]\nurl = \"http://a/mcp\"\nhttp_headers = {{ Authorization = \"Bearer tok\" }}\n")
        );
        assert_eq!(without_codex_entry(&added), config, "byte for byte");
        // Written twice: one entry, the new one.
        let again = with_codex_entry(&added, "http://b/mcp", "new");
        assert_eq!(again.matches("orbiters-vrchat").count(), 1);
        assert!(again.contains("Bearer new") && again.contains("http://b/mcp") && !again.contains("Bearer tok"));
        assert_eq!(without_codex_entry(&again), config);
        // Line ends as the file has them, and a file without an end to its last line.
        let windows = with_codex_entry("a = 1\r\n", "u", "t");
        assert_eq!(windows, "a = 1\r\n\r\n[mcp_servers.orbiters-vrchat]\r\nurl = \"u\"\r\nhttp_headers = { Authorization = \"Bearer t\" }\r\n");
        assert!(with_codex_entry("a = 1", "u", "t").starts_with("a = 1\n\n[mcp_servers"));
        // No file yet.
        assert!(with_codex_entry("", "u", "t").starts_with("[mcp_servers.orbiters-vrchat]"));
        // In the form Codex writes when it rewrites the file: its headers in a table of their own, in the middle.
        let rewritten = "[mcp_servers.orbiters-vrchat]\nurl = \"u\"\n\n[mcp_servers.orbiters-vrchat.http_headers]\nAuthorization = \"Bearer t\"\n\n# kept\n[mcp_servers.z]\nurl = \"z\"\n";
        assert_eq!(without_codex_entry(rewritten), "# kept\n[mcp_servers.z]\nurl = \"z\"\n");
        // Another server whose name starts the same stays.
        let similar = "[mcp_servers.orbiters-vrchat-2]\nurl = \"z\"\n";
        assert_eq!(without_codex_entry(similar), similar);
    }

    #[test]
    fn a_token_is_never_repeated() {
        assert_eq!(complaint("WARNING: x\n\nBad header Bearer sekret\n", "sekret"), "Bad header Bearer <token>");
        assert_eq!(complaint("", "sekret"), "It gave no reason.");
    }

    #[test]
    fn adding_a_client_says_so_and_says_about_open_sessions() {
        let fake = Arc::new(Fake::default());
        let (this, logged) = registrations(fake.clone());
        this.change(Client::Codex, Change::Add, wanted());
        let status = &this.statuses()[0];
        assert_eq!(status.registration, Registration::Added);
        assert!(!status.busy && !status.failed);
        assert!(status.note.as_ref().unwrap().contains("already open"), "{status:?}");
        assert_eq!(*fake.calls.lock().unwrap(), ["read", "add", "read"], "nothing to take away first");
        assert!(logged.lock().unwrap()[0].0);
    }

    #[test]
    fn an_update_takes_the_old_entry_away_first() {
        let fake = Arc::new(Fake { entry: Mutex::new(entry(WANTED.0, "old")), ..Default::default() });
        let (this, _) = registrations(fake.clone());
        this.change(Client::Codex, Change::Add, wanted());
        assert_eq!(this.statuses()[0].registration, Registration::Added);
        assert_eq!(*fake.calls.lock().unwrap(), ["read", "remove", "add", "read"]);
        assert_eq!(fake.entry.lock().unwrap().as_ref().unwrap().token.as_deref(), Some(WANTED.1));
    }

    #[test]
    fn a_program_that_is_not_there_is_said_and_never_run() {
        let fake = Arc::new(Fake { missing: true, ..Default::default() });
        let (this, _) = registrations(fake.clone());
        this.check(wanted());
        assert_eq!(this.statuses()[0].registration, Registration::NotInstalled);
        this.change(Client::Codex, Change::Add, wanted());
        assert_eq!(this.statuses()[0].registration, Registration::NotInstalled);
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn failures_say_why_and_show_what_the_client_really_has() {
        let fake = Arc::new(Fake { add_fails: Some("the disk is full"), ..Default::default() });
        let (this, logged) = registrations(fake);
        this.change(Client::Codex, Change::Add, wanted());
        let status = &this.statuses()[0];
        assert_eq!(status.registration, Registration::NotAdded);
        assert!(status.failed && status.note.as_ref().unwrap().contains("the disk is full"), "{status:?}");
        assert!(!logged.lock().unwrap()[0].0);

        // The client said it added it, and does not show it: not believed.
        let (this, _) = registrations(Arc::new(Fake { add_loses_it: true, ..Default::default() }));
        this.change(Client::Codex, Change::Add, wanted());
        let status = &this.statuses()[0];
        assert!(status.failed && status.registration == Registration::NotAdded, "{status:?}");

        // It went in, and the client cannot connect: said, with its own words.
        let (this, _) = registrations(Arc::new(Fake { problem: Some("HTTP 401"), ..Default::default() }));
        this.change(Client::Codex, Change::Add, wanted());
        let status = &this.statuses()[0];
        assert_eq!(status.registration, Registration::NotConnecting("HTTP 401".into()));
        assert!(status.failed && status.note.as_ref().unwrap().contains("HTTP 401"));

        // A client that answers nonsense.
        let (this, _) = registrations(Arc::new(Fake { read_fails: Some("broken"), ..Default::default() }));
        this.check(wanted());
        assert_eq!(this.statuses()[0].registration, Registration::Unreadable("broken".into()));

        // Taking it away fails: it still shows as what it is.
        let (this, _) = registrations(Arc::new(Fake { entry: Mutex::new(entry(WANTED.0, WANTED.1)), remove_fails: Some("locked"), ..Default::default() }));
        this.change(Client::Codex, Change::Remove, wanted());
        let status = &this.statuses()[0];
        assert_eq!(status.registration, Registration::Added);
        assert!(status.failed && status.note.as_ref().unwrap().contains("locked"));
    }

    #[test]
    fn removing_takes_it_away() {
        let fake = Arc::new(Fake { entry: Mutex::new(entry(WANTED.0, WANTED.1)), ..Default::default() });
        let (this, _) = registrations(fake.clone());
        this.change(Client::Codex, Change::Remove, wanted());
        let status = &this.statuses()[0];
        assert_eq!((&status.registration, status.note.as_deref(), status.failed), (&Registration::NotAdded, Some("Removed."), false));
        assert!(fake.entry.lock().unwrap().is_none());
    }

    #[test]
    fn a_new_token_or_port_fixes_an_old_entry_and_adds_none_that_was_not_there() {
        let fake = Arc::new(Fake { entry: Mutex::new(entry("http://127.0.0.1:1/mcp", "old")), ..Default::default() });
        let (this, logged) = registrations(fake.clone());
        this.repair(wanted());
        let status = &this.statuses()[0];
        assert_eq!(status.registration, Registration::Added);
        assert!(status.note.as_ref().unwrap().contains("session already open"), "{status:?}");
        assert_eq!(fake.entry.lock().unwrap().as_ref().unwrap().address.as_deref(), Some(WANTED.0));
        assert_eq!(logged.lock().unwrap().len(), 1, "and it is said");

        // A repair that cannot be done says so: it does not silently stay broken.
        let fake = Arc::new(Fake { entry: Mutex::new(entry(WANTED.0, "old")), remove_fails: Some("locked"), ..Default::default() });
        let (this, logged) = registrations(fake);
        this.repair(wanted());
        let status = &this.statuses()[0];
        assert_eq!(status.registration, Registration::Outdated(Old::Token));
        assert!(status.failed && status.note.as_ref().unwrap().contains("locked"));
        assert!(!logged.lock().unwrap()[0].0);

        let fake = Arc::new(Fake::default());
        let (this, logged) = registrations(fake.clone());
        this.repair(wanted());
        assert_eq!(this.statuses()[0].registration, Registration::NotAdded);
        assert!(fake.entry.lock().unwrap().is_none(), "nobody asked for this one");
        assert!(logged.lock().unwrap().is_empty());

        let fake = Arc::new(Fake { entry: Mutex::new(entry(WANTED.0, WANTED.1)), ..Default::default() });
        let (this, logged) = registrations(fake.clone());
        this.repair(wanted());
        assert_eq!(*fake.calls.lock().unwrap(), ["read"], "nothing to do");
        assert!(logged.lock().unwrap().is_empty());
    }

    #[test]
    fn looks_are_rationed_and_a_busy_client_is_left_alone() {
        let fake = Arc::new(Fake::default());
        let (this, _) = registrations(fake.clone());
        this.check(wanted());
        this.check(wanted());
        assert_eq!(fake.calls.lock().unwrap().len(), 1, "the second look was too soon");
        // A command running: another one does not start beside it.
        this.slots.lock().unwrap()[0].status.busy = true;
        this.change(Client::Codex, Change::Add, wanted());
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_program_that_hangs_is_given_up_on() {
        if !cfg!(windows) {
            return;
        }
        let begin = Instant::now();
        let said = run(Path::new("cmd"), &["/c", "ping -n 30 127.0.0.1 >nul"], None, &[], Duration::from_millis(300));
        assert!(said.is_err_and(|e| e.contains("did not answer")));
        assert!(begin.elapsed() < Duration::from_secs(5));
        let said = run(Path::new("cmd"), &["/c", "echo hello & echo there 1>&2 & exit 3"], None, &[], Duration::from_secs(20)).unwrap();
        assert!(!said.success && said.out.contains("hello") && said.err.contains("there"));
        assert!(run(Path::new("no-such-program-here"), &[], None, &[], Duration::from_secs(1)).is_err());
    }

    /// The real programs, with their files in a folder of the test: run by hand when either of them updates
    /// (`cargo test -p orbiters-desktop real_clients -- --ignored --nocapture`).
    #[test]
    #[ignore = "runs the real Claude Code and Codex"]
    fn real_clients_add_update_and_remove() {
        let dir = std::env::temp_dir().join(format!("orbiters-clients-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("codex")).unwrap();
        std::fs::create_dir_all(dir.join("claude")).unwrap();
        let before = "# mine\nmodel = \"x\"\n\n[mcp_servers.other]\nenabled = true\nurl = \"http://x/mcp\"\n\n# <<< block\n[mcp_servers.another]\nargs = [ \"-y\" ]\ncommand = \"npx\"\n";
        std::fs::write(dir.join("codex").join("config.toml"), before).unwrap();
        let clis: Vec<Arc<dyn ClientCli>> = vec![Arc::new(ClaudeCli { config_dir: Some(dir.join("claude")) }), Arc::new(CodexCli { home: Some(dir.join("codex")) })];
        for cli in clis {
            let name = cli.client().name();
            assert!(cli.installed(), "{name}");
            assert_eq!(cli.read().unwrap(), None, "{name}");
            cli.remove().unwrap_or_else(|e| panic!("{name}: removing nothing is no failure: {e}"));
            let this = Registrations::new(vec![cli.clone()], Box::new(|_, _| {}));
            this.threads.store(false, Ordering::Relaxed);
            let wanted = Wanted { address: "http://127.0.0.1:1/mcp".into(), token: "firsttoken".into(), listening: false };
            this.change(cli.client(), Change::Add, wanted.clone());
            let status = this.statuses()[0].clone();
            assert_eq!(status.registration, Registration::Added, "{name}: {status:?}");
            let read = cli.read().unwrap().unwrap();
            assert_eq!((read.address.as_deref(), read.token.as_deref()), (Some("http://127.0.0.1:1/mcp"), Some("firsttoken")), "{name}");
            // A new token, then a new port: repaired, by the client's own commands.
            let token = Wanted { token: "secondtoken".into(), ..wanted.clone() };
            assert_eq!(classify(Some(&read), &token), Registration::Outdated(Old::Token), "{name}");
            this.repair(token.clone());
            assert_eq!(this.statuses()[0].registration, Registration::Added, "{name}");
            let port = Wanted { address: "http://127.0.0.1:2/mcp".into(), ..token };
            this.repair(port.clone());
            let read = cli.read().unwrap().unwrap();
            assert_eq!((read.address.as_deref(), read.token.as_deref()), (Some("http://127.0.0.1:2/mcp"), Some("secondtoken")), "{name}");
            this.change(cli.client(), Change::Remove, port);
            assert_eq!(this.statuses()[0].registration, Registration::NotAdded, "{name}");
            assert_eq!(cli.read().unwrap(), None, "{name}");
        }
        assert_eq!(std::fs::read_to_string(dir.join("codex").join("config.toml")).unwrap(), before, "Codex's file, byte for byte");
        let _ = std::fs::remove_dir_all(dir);
    }
}
