//! Installing the Unity editor a project needs, on a Mac, as far as a program can: Unity Hub when it is missing, then the editor through the Hub's own command line, with the
//! module a VRChat project builds for a PC with, checked once it is there. What only a person can do (a Unity account, its licence) is told apart and waited for.
//!
//! **How, and why so** (looked at on Unity Hub 4.0.2, 2026-11-25): the Hub's `-- --headless install` is gone from it ("The native Unity Hub no longer runs these headless commands"),
//! and its bundle carries Unity's command line instead, `Contents/Resources/unity`: `unity install <version> --changeset <c> --architecture arm64 --module windows-mono
//! --non-interactive --yes --format ndjson`, which says its progress as a line of JSON each second, keeps what it downloaded when interrupted, and goes on from there when asked
//! again. A Hub 3 has no such program and still takes `-- --headless`: both are known here. Where neither can, `unityhub://<version>/<changeset>` has the Hub itself install it.
//!
//! **It survives the app.** The command runs in a group of its own with its words in a file, and a record beside it names the process: a window opened later follows it on, and one
//! that finds it ended asks again, from where it stopped. The editor found by [`crate::unity6::editors`], of that version, that build and for Apple silicon, is the one proof.
//!
//! **Versions.** The version is the project's own (its `ProjectVersion.txt`), its changeset that file's or VRChat's release list's ([`crate::unity6::matrix`]), else Unity's
//! release data: none is written here.

// (what a Mac's install alone uses)
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

use crate::unity6::{budget, editors, matrix};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Open in Unity's own refusal when the editor a project needs is not here: what the person may be offered instead. Its words are a sentence, as any other error's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotInstalled {
    pub version: String,
    /// The build the project names.
    pub revision: Option<String>,
    /// A program that is another version, or whose version could not be read: never used.
    pub found: Option<PathBuf>,
    pub words: String,
}

impl std::fmt::Display for NotInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.words)
    }
}

impl std::error::Error for NotInstalled {}

/// The editor to install.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wanted {
    pub version: String,
    pub changeset: Option<String>,
    /// The project it is for, opened once it is there, and its name.
    pub project: PathBuf,
    pub name: String,
}

impl Wanted {
    /// The changeset the release list has for its version, when the project names none. Only words that are a version and a build go on.
    fn checked(&self, matrix: &matrix::Matrix) -> Option<Self> {
        let changeset = self.changeset.clone().or_else(|| matrix.row_of_editor(&self.version).map(|r| r.changeset.clone()));
        let hex = |c: &String| (8..=16).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit());
        let version = vrc_get_vpm::version::UnityVersion::parse(&self.version).is_some() && self.version.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
        (version && changeset.as_ref().is_none_or(hex)).then(|| Self { changeset, ..self.clone() })
    }

    /// Unity Hub's link that installs exactly this editor.
    pub fn hub_link(&self) -> Option<String> {
        let w = self.checked(&matrix::Matrix::built_in())?;
        Some(format!("unityhub://{}/{}", w.version, w.changeset?))
    }
}

/// What it takes, from Unity's release data or VRChat's release list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Release {
    pub changeset: Option<String>,
    pub download: u64,
    pub installed: u64,
}

/// The module a VRChat project on a Mac uploads for a PC with.
pub const MODULE: &str = "windows-mono";
/// What that module takes when Unity's data could not be read (2022.3.22f1's: 181 and 458 MB).
const MODULE_SIZE: [u64; 2] = [200_000_000, 500_000_000];

impl Release {
    /// From Unity's public release data (`services.api.unity.com/unity/editor/release/v1`), the one download for Apple silicon.
    pub fn from_json(text: &str, version: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let r = v["results"].as_array()?.iter().find(|r| r["version"] == version)?;
        let d = r["downloads"].as_array()?.iter().find(|d| d["platform"] == "MAC_OS" && d["architecture"] == "ARM64")?;
        let size = |v: &serde_json::Value, k: &str| v[k]["value"].as_f64().map(|n| n as u64);
        let module = d["modules"].as_array().and_then(|m| m.iter().find(|m| m["id"] == MODULE));
        let of = |k: &str, else_: u64| module.and_then(|m| size(m, k)).unwrap_or(else_);
        Some(Self {
            changeset: r["shortRevision"].as_str().map(str::to_owned),
            download: size(d, "downloadSize")? + of("downloadSize", MODULE_SIZE[0]),
            installed: size(d, "installedSize")? + of("installedSize", MODULE_SIZE[1]),
        })
    }

    fn from_list(matrix: &matrix::Matrix, version: &str) -> Option<Self> {
        let row = matrix.row_of_editor(version)?;
        let size = row.editor.get("macos-arm64")?;
        Some(Self { changeset: Some(row.changeset.clone()), download: size.download + MODULE_SIZE[0], installed: size.installed + MODULE_SIZE[1] })
    }
}

/// Where it is at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Unity's data, the disk and the Hub are looked at.
    Checking,
    /// Unity Hub comes, this much of it.
    Hub { have: u64, total: u64 },
    /// It is checked and put in place.
    PlacingHub,
    /// The Hub has the editor (`bytes` to download) and its module come, or put in place.
    Editor { percent: u8, installing: bool, bytes: u64 },
    /// Another install of the Hub's runs first.
    Waiting,
    /// The person installs it in Unity Hub, which was opened on it.
    InHub,
    Verifying,
    /// Only the person can: sign in (`signed_in` false), have a licence.
    Licence { signed_in: bool },
}

/// How it ended before the editor was there and usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    Stopped,
    /// Not enough room on the disk of `at`.
    Room { at: PathBuf, need: u64, free: Option<u64> },
    Failed(String),
}

/// The state of Unity's licence here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Licence {
    Here,
    /// None; whether an account is signed in, when that could be told.
    None { signed_in: bool },
}

/// A line of the command's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Progress { percent: u8, installing: bool },
    Waiting,
    Result { ok: bool, why: Option<String> },
}

/// What a line of the Hub's command line says: JSON of Unity's own (Hub 4), or a Hub 3's text.
pub fn line(text: &str) -> Option<Line> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        return match v["type"].as_str()? {
            "progress" => {
                let phase = v["phase"].as_str().unwrap_or_default();
                if phase.contains("wait") || phase.contains("queue") {
                    return Some(Line::Waiting);
                }
                let percent = v["pct"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0) as u8;
                Some(Line::Progress { percent, installing: !phase.is_empty() && phase != "download" })
            }
            "result" => {
                let why = v["errors"].as_array().and_then(|e| e.first()).and_then(|e| e["message"].as_str()).map(str::to_owned);
                Some(Line::Result { ok: v["success"] == true, why })
            }
            _ => None,
        };
    }
    let low = text.to_ascii_lowercase();
    if low.contains("all tasks completed successfully") {
        return Some(Line::Result { ok: true, why: None });
    }
    if low.contains("completed with errors") || low.starts_with("error") {
        return Some(Line::Result { ok: false, why: Some(text.trim().to_owned()) });
    }
    let percent = text.split_whitespace().rev().find_map(|w| w.strip_suffix('%')?.parse::<f32>().ok());
    let installing = low.contains("install") && !low.contains("download");
    percent.map(|p| Line::Progress { percent: p.clamp(0.0, 100.0) as u8, installing }).or_else(|| installing.then_some(Line::Progress { percent: 100, installing: true }))
}

/// Unity Hub's own programs: what it is installed with, whatever its version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubCli {
    /// Unity's command line, carried by the Hub from its version 4.
    Unity(PathBuf),
    /// The Hub itself, headless (until its version 3).
    Headless(PathBuf),
}

impl HubCli {
    /// Of the Hub whose program is `program`.
    pub fn of(program: &Path) -> Self {
        let unity = program.parent().and_then(Path::parent).map(|contents| contents.join("Resources/unity"));
        match unity.filter(|p| p.is_file()) {
            Some(p) => Self::Unity(p),
            None => Self::Headless(program.to_path_buf()),
        }
    }

    pub fn program(&self) -> &Path {
        match self {
            Self::Unity(p) | Self::Headless(p) => p,
        }
    }

    /// What installs `version`, of the build `changeset`, for Apple silicon.
    pub fn install(&self, version: &str, changeset: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = match self {
            Self::Unity(_) => vec!["install".into(), version.into()],
            Self::Headless(_) => ["--", "--headless", "install", "--version", version].map(String::from).to_vec(),
        };
        if let Some(c) = changeset {
            args.extend(["--changeset".into(), c.into()]);
        }
        args.extend(["--architecture", "arm64", "--module", MODULE].map(String::from));
        if matches!(self, Self::Unity(_)) {
            args.extend(["--non-interactive", "--yes", "--no-banner", "--format", "ndjson"].map(String::from));
        }
        args
    }
}

/// The install under way, or cut short, as it is kept beside its words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub wanted: Wanted,
    /// The command's process, and when it began (the system's own time of it): another process of that number is not it.
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub began: Option<u64>,
    #[serde(default)]
    pub download: u64,
}

/// The folder of the record and of the command's words.
pub fn folder() -> PathBuf {
    crate::data_dir().join("unity-install")
}

/// What an earlier window left: an install that runs on, or one cut short.
pub fn left(folder: &Path) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(folder.join(RECORD)).ok()?).ok()
}

/// Nothing of it any more.
pub fn forget(folder: &Path) {
    let _ = crate::disk::remove_file(folder.join(RECORD));
    let _ = crate::disk::remove_file(folder.join(WORDS));
}

const RECORD: &str = "install.json";
const WORDS: &str = "install.log";

/// What the demands on each disk are, and the first disk without the room.
fn shortage(release: &Release, install_at: PathBuf, download_at: PathBuf, room: &dyn budget::Room) -> Option<End> {
    let demand = |need, at: &Path, bytes| budget::Demand { need, at: at.to_path_buf(), bytes, low: None };
    let needs = vec![
        demand(budget::Need::Editor, &install_at, release.installed),
        demand(budget::Need::Download, &download_at, release.download),
        demand(budget::Need::Margin, &install_at, budget::MARGIN),
    ];
    let plan = budget::plan(needs, room, false);
    plan.volumes.iter().find(|v| !v.fits()).map(|v| End::Room { at: v.at.clone(), need: v.total(), free: v.free })
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
    use std::os::unix::process::CommandExt as _;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// Unity's signature: what is put in place or run carries it.
    const UNITY: &str = "anchor apple generic and certificate leaf[subject.OU] = \"9QW8UQUTAA\" and identifier \"com.unity3d.unityhub\""; // i18n: not shown (a code signing requirement)
    const HUB_FEED: &str = "https://public-cdn.cloud.unity3d.com/hub/prod/latest-mac.yml";
    const RELEASES: &str = "https://services.api.unity.com/unity/editor/release/v1/releases";
    /// How long an interrupted command is given to let its download go cleanly.
    const ENDS_WITHIN: Duration = Duration::from_secs(8);

    /// The computer it is done on. (A test's has its own.)
    pub struct World {
        /// Unity Hub's program, when it is here.
        pub hub: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
        pub editor: Box<dyn Fn(&str, Option<&str>) -> editors::Found + Send + Sync>,
        pub licence: Box<dyn Fn(Option<&HubCli>) -> Licence + Send + Sync>,
        pub room: Box<dyn budget::Room + Send + Sync>,
        /// Whether a Hub at this path is Unity's.
        pub genuine: Box<dyn Fn(&Path) -> bool + Send + Sync>,
        /// Opens a link or an application for the person.
        pub open: Box<dyn Fn(&str) -> bool + Send + Sync>,
        pub hub_feed: String,
        pub releases: String,
        /// Where the Hub goes, the first that can be written.
        pub applications: Vec<PathBuf>,
        pub install_at: PathBuf,
        /// Where the Hub downloads.
        pub download_at: PathBuf,
        pub folder: PathBuf,
        pub every: Duration,
        pub licence_every: Duration,
        pub agent: ureq::Agent,
    }

    impl World {
        pub fn here() -> Self {
            let hub_data = crate::hub_data_dir();
            Self {
                hub: Box::new(crate::unity_hub),
                editor: Box::new(|v, b| editors::find_build(v, b)),
                licence: Box::new(licence),
                room: Box::new(budget::Disks),
                genuine: Box::new(|app| codesigned(app)),
                open: Box::new(|what| crate::mac::open_command().arg(what).status().is_ok_and(|s| s.success())),
                hub_feed: HUB_FEED.into(),
                releases: RELEASES.into(),
                applications: [Some(PathBuf::from("/Applications")), dirs::home_dir().map(|h| h.join("Applications"))].into_iter().flatten().collect(),
                install_at: budget::editor_install_folder(),
                download_at: hub_data.unwrap_or_else(std::env::temp_dir),
                folder: folder(),
                every: Duration::from_millis(300),
                licence_every: Duration::from_secs(3),
                // (long enough for the Hub's 55 MB on a slow connection; what was cut goes on from there)
                agent: orbiters_core::http_agent(Some(Duration::from_secs(20)), Some(Duration::from_secs(30 * 60))),
            }
        }
    }

    fn quiet(command: &mut Command) -> bool {
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    }

    fn codesigned(app: &Path) -> bool {
        quiet(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict", &format!("-R={UNITY}")]).arg(app))
    }

    /// The files Unity keeps a licence in: each written by its own licensing once one is active.
    fn licence_files() -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_default();
        vec![
            home.join("Library/Unity/licenses/UnityEntitlementLicense.xml"),
            PathBuf::from("/Library/Unity/licenses/UnityEntitlementLicense.xml"),
            PathBuf::from("/Library/Application Support/Unity/Unity_lic.ulf"),
        ]
    }

    /// A company's licence server is set on this Mac: the editor asks it itself. (The same file may hold other settings alone.)
    fn licence_server() -> bool {
        std::fs::read("/Library/Application Support/Unity/config/services-config.json")
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| v["licensingServiceBaseUrl"].as_str().is_some_and(|u| !u.is_empty()))
    }

    /// Whether Unity has a licence here, as its files or its command line say; whether somebody is signed in.
    pub fn licence(cli: Option<&HubCli>) -> Licence {
        if licence_files().iter().any(|f| f.is_file()) || licence_server() {
            return Licence::Here;
        }
        let Some(HubCli::Unity(program)) = cli else { return Licence::None { signed_in: false } };
        if asked(program, &["license"]).is_some_and(|v| v["data"]["items"].as_array().is_some_and(|i| !i.is_empty())) {
            return Licence::Here;
        }
        Licence::None { signed_in: asked(program, &["auth", "status"]).is_some_and(|v| v["data"]["loggedIn"] == true) }
    }

    /// What Unity's command line answers, as JSON, within a few seconds.
    fn asked(program: &Path, args: &[&str]) -> Option<serde_json::Value> {
        let mut child = Command::new(program)
            .args(args)
            .args(["--non-interactive", "--no-banner", "--format", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut out = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::by_ref(&mut out).take(1 << 20).read_to_string(&mut text);
            text
        });
        let until = Instant::now() + Duration::from_secs(10);
        while child.try_wait().ok()?.is_none() {
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        serde_json::from_str(reader.join().ok()?.trim()).ok()
    }

    /// When process `pid` began, in microseconds, as the system keeps it: `None` when there is none.
    fn began(pid: u32) -> Option<u64> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        // SAFETY: the kernel fills at most the structure's size, which is passed.
        let n = unsafe { libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
        (n == size && info.pbi_status != 5).then_some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }

    /// Whether the command of `record` still runs.
    pub fn running(record: &Record) -> bool {
        matches!((record.pid, record.began), (Some(pid), Some(at)) if began(pid) == Some(at))
    }

    fn save(folder: &Path, record: &Record) -> Result<(), End> {
        crate::write_json(&folder.join(RECORD), record).map_err(|e| End::Failed(format!("{e:#}")))
    }

    /// The editor, of that version and build and for Apple silicon, is here.
    fn there(world: &World, w: &Wanted) -> bool {
        match (world.editor)(&w.version, w.changeset.as_deref()) {
            editors::Found::Exact(c) => c.identity.is_some_and(|i| i.arch.contains(&editors::Arch::Arm64)),
            _ => false,
        }
    }

    /// Everything between the press and an editor the project can be opened with. `say` hears each step; `stop` ends it, keeping what was downloaded; `anyway`: the person goes
    /// on without a licence being seen. `in_hub`: the person installs it in Unity Hub, which is opened on it.
    pub fn run(world: &World, wanted: &Wanted, in_hub: bool, say: &dyn Fn(Step), stop: &AtomicBool, anyway: &AtomicBool) -> Result<(), End> {
        let stopped = || stop.load(Ordering::Relaxed);
        say(Step::Checking);
        let list = matrix::Matrix::built_in();
        let mut w = wanted.checked(&list).ok_or_else(|| End::Failed(tr!("{name} does not name a Unity version that can be installed.", name = wanted.name)))?;
        crate::disk::create_dir_all(&world.folder).map_err(|e| End::Failed(e.to_string()))?;
        if !there(world, &w) {
            let before = left(&world.folder).filter(|r| r.wanted.version == w.version && running(r));
            if let Some(record) = before {
                // An install an earlier window began, still at it.
                w = record.wanted.clone();
                follow(world, &record, None, say, stop)?;
            } else {
                let release = release(world, &list, &w.version);
                w.changeset = w.changeset.or_else(|| release.as_ref().and_then(|r| r.changeset.clone()));
                let release = release.ok_or_else(|| End::Failed(tr!("What Unity {version} takes could not be read from Unity. Check the connection and try again.", version = w.version)))?;
                // Looked at before anything is downloaded, and again before the large part.
                shortage(&release, world.install_at.clone(), world.download_at.clone(), &*world.room).map_or(Ok(()), Err)?;
                save(&world.folder, &Record { wanted: w.clone(), pid: None, began: None, download: release.download })?;
                let hub = match (world.hub)() {
                    Some(hub) => hub,
                    None => get_hub(world, say, stop)?,
                };
                if stopped() {
                    return Err(End::Stopped);
                }
                if in_hub {
                    let link = w.hub_link().unwrap_or_else(|| "unityhub://".into());
                    say(Step::InHub);
                    let opened = (world.open)(&link);
                    if !opened {
                        return Err(End::Failed(tr("Unity Hub could not be opened.").into()));
                    }
                    while !there(world, &w) {
                        if wait(world.every.max(Duration::from_millis(50)) * 3, stop) {
                            return Err(End::Stopped);
                        }
                    }
                } else {
                    shortage(&release, world.install_at.clone(), world.download_at.clone(), &*world.room).map_or(Ok(()), Err)?;
                    let (record, child) = start(world, &HubCli::of(&hub), &w, release.download)?;
                    follow(world, &record, Some(child), say, stop)?;
                }
            }
            say(Step::Verifying);
            if !there(world, &w) {
                return Err(End::Failed(tr!("Unity Hub ended without Unity {version} for Apple silicon being found here.", version = w.version)));
            }
        }
        // A licence is the person's. It is waited for, and nothing is claimed ready before.
        let cli = || (world.hub)().map(|h| HubCli::of(&h));
        let mut said = None;
        while !anyway.load(Ordering::Relaxed) {
            let Licence::None { signed_in } = (world.licence)(cli().as_ref()) else { break };
            if said != Some(signed_in) {
                said = Some(signed_in);
                say(Step::Licence { signed_in });
            }
            let until = Instant::now() + world.licence_every;
            while Instant::now() < until && !anyway.load(Ordering::Relaxed) {
                if wait(Duration::from_millis(50).min(world.licence_every), stop) {
                    return Err(End::Stopped);
                }
            }
        }
        forget(&world.folder);
        Ok(())
    }

    /// Waits `long`, or less when it is to stop: whether it is.
    fn wait(long: Duration, stop: &AtomicBool) -> bool {
        let until = Instant::now() + long;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20).min(long));
        }
        stop.load(Ordering::Relaxed)
    }

    fn release(world: &World, list: &matrix::Matrix, version: &str) -> Option<Release> {
        let url = format!("{}?version={version}&platform=MAC_OS&architecture=ARM64", world.releases);
        let text = || -> Option<String> { world.agent.get(&url).call().ok()?.body_mut().with_config().limit(8 << 20).read_to_string().ok() };
        text().and_then(|t| Release::from_json(&t, version)).or_else(|| Release::from_list(list, version))
    }

    /// Starts the Hub's command for `w`, its words in a file: it goes on whatever becomes of this process.
    fn start(world: &World, cli: &HubCli, w: &Wanted, download: u64) -> Result<(Record, Child), End> {
        let failed = |e: std::io::Error| End::Failed(tr!("Unity Hub's installer could not be started: {why}", why = e));
        let words = crate::disk::create_file(world.folder.join(WORDS)).map_err(failed)?;
        let also = words.try_clone().map_err(failed)?;
        let child = Command::new(cli.program())
            .args(cli.install(&w.version, w.changeset.as_deref()))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(words)
            .stderr(also)
            .process_group(0)
            .spawn()
            .map_err(failed)?;
        let record = Record { wanted: w.clone(), pid: Some(child.id()), began: began(child.id()), download };
        save(&world.folder, &record)?;
        Ok((record, child))
    }

    #[cfg(test)]
    pub fn start_for_tests(world: &World, cli: &HubCli, w: &Wanted, download: u64) -> (Record, Child) {
        start(world, cli, w, download).unwrap()
    }

    /// Follows the command of `record` (its own child, or one an earlier window began) to its end.
    fn follow(world: &World, record: &Record, mut child: Option<Child>, say: &dyn Fn(Step), stop: &AtomicBool) -> Result<(), End> {
        let (mut read, mut pending) = (0u64, String::new());
        let mut result: Option<(bool, Option<String>)> = None;
        say(Step::Editor { percent: 0, installing: false, bytes: record.download });
        loop {
            let ended = match &mut child {
                Some(c) => c.try_wait().map(|s| s.is_some()).unwrap_or(true),
                None => !running(record),
            };
            if let Ok(mut file) = std::fs::File::open(world.folder.join(WORDS)) {
                let mut more = Vec::new();
                if file.seek(SeekFrom::Start(read)).is_ok() && std::io::Read::by_ref(&mut file).take(1 << 20).read_to_end(&mut more).is_ok() {
                    read += more.len() as u64;
                    pending.push_str(&String::from_utf8_lossy(&more));
                }
            }
            // (a Hub 3 ends some of its lines with a carriage return alone)
            while let Some(end) = pending.find(['\n', '\r']) {
                let text: String = pending.drain(..=end).collect();
                match line(text.trim()) {
                    Some(Line::Progress { percent, installing }) => say(Step::Editor { percent, installing, bytes: record.download }),
                    Some(Line::Waiting) => say(Step::Waiting),
                    Some(Line::Result { ok, why }) => result = Some((ok, why)),
                    None => {}
                }
            }
            if ended {
                break;
            }
            if wait(world.every, stop) {
                interrupt(record, child.as_mut());
                return Err(End::Stopped);
            }
        }
        match result.or_else(|| line(pending.trim()).and_then(|l| if let Line::Result { ok, why } = l { Some((ok, why)) } else { None })) {
            Some((false, why)) => Err(End::Failed(match why {
                Some(why) => tr!("Unity Hub could not install Unity {version}: {why}", version = record.wanted.version, why = why),
                None => tr!("Unity Hub could not install Unity {version}.", version = record.wanted.version),
            })),
            // (whatever it says, the editor is looked for)
            _ => Ok(()),
        }
    }

    /// Ends the command as a person's Ctrl+C does: it keeps what it downloaded.
    fn interrupt(record: &Record, child: Option<&mut Child>) {
        let Some(pid) = record.pid else { return };
        let mut child = child;
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGKILL] {
            if !running(record) && child.is_none() {
                return;
            }
            // SAFETY: the group of a process of ours, made at its start; proven the same process by when it began.
            unsafe { libc::kill(-(pid as i32), signal) };
            let until = Instant::now() + ENDS_WITHIN;
            while Instant::now() < until {
                let gone = match child.as_deref_mut() {
                    Some(c) => !matches!(c.try_wait(), Ok(None)),
                    None => !running(record),
                };
                if gone {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    /// Unity Hub from Unity's own public downloads, checked (its size, its SHA-512, its signature), in the first Applications folder that can be written: no administrator's
    /// password.
    fn get_hub(world: &World, say: &dyn Fn(Step), stop: &AtomicBool) -> Result<PathBuf, End> {
        let failed = |e: String| End::Failed(tr!("Unity Hub could not be downloaded: {why}", why = e));
        say(Step::Hub { have: 0, total: 0 });
        let feed = world.agent.get(&world.hub_feed).call().map_err(|e| failed(e.to_string()))?.body_mut().with_config().limit(256 << 10).read_to_string().map_err(|e| failed(e.to_string()))?;
        let file = HubFile::of(&feed).ok_or_else(|| failed(tr("Unity's list of downloads has none for Apple silicon.").into()))?;
        let base = world.hub_feed.rsplit_once('/').map_or("", |(b, _)| b);
        let url = format!("{base}/{}", file.url);
        let archive = world.folder.join(format!("hub-{}.zip", file.name()));
        download(world, &url, &file, &archive, say, stop)?;
        say(Step::PlacingHub);
        let applications = world.applications.iter().find(|d| writable(d)).ok_or_else(|| End::Failed(tr("No Applications folder can be written by this user.").into()))?;
        crate::disk::create_dir_all(applications).map_err(|e| failed(e.to_string()))?;
        // Beside its place, so that it comes there whole.
        let work = applications.join(format!(".orbiters-unity-hub-{}", std::process::id()));
        let _ = crate::disk::remove_dir_all(&work);
        crate::disk::create_dir_all(&work).map_err(|e| failed(e.to_string()))?;
        let placed = (|| {
            if !quiet(Command::new("/usr/bin/ditto").arg("-xk").arg(&archive).arg(&work)) {
                return Err(failed(tr("The download could not be unpacked.").into()));
            }
            let app = work.join("Unity Hub.app");
            if !(world.genuine)(&app) {
                return Err(failed(tr("It is not signed by Unity: nothing was installed.").into()));
            }
            let to = applications.join("Unity Hub.app");
            if !to.exists() {
                crate::disk::rename(&app, &to).map_err(|e| failed(e.to_string()))?;
            }
            Ok(crate::mac::bundle_program(&to))
        })();
        let _ = crate::disk::remove_dir_all(&work);
        let _ = crate::disk::remove_file(&archive);
        let program = placed?;
        Ok((world.hub)().unwrap_or(program))
    }

    /// Whether this user can put something in `dir`, or make it.
    fn writable(dir: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt as _;
        let mut at = dir;
        while !at.exists() {
            let Some(parent) = at.parent() else { return false };
            at = parent;
        }
        // SAFETY: a valid C string.
        std::ffi::CString::new(at.as_os_str().as_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
    }

    /// The Hub's archive for Apple silicon in Unity's list.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct HubFile {
        pub url: String,
        pub sha512: Vec<u8>,
        pub size: u64,
    }

    impl HubFile {
        pub fn of(feed: &str) -> Option<Self> {
            use base64::Engine as _;
            let lines: Vec<&str> = feed.lines().map(str::trim).collect();
            let at = lines.iter().position(|l| l.strip_prefix("- url:").is_some_and(|u| u.trim().ends_with("arm64.zip")))?;
            let url = lines[at].strip_prefix("- url:")?.trim().to_owned();
            let field = |name: &str| lines[at + 1..].iter().take_while(|l| !l.starts_with("- ")).find_map(|l| l.strip_prefix(name)).map(|v| v.trim().to_owned());
            let sha512 = base64::engine::general_purpose::STANDARD.decode(field("sha512:")?).ok().filter(|s| s.len() == 64)?;
            let size = field("size:")?.parse().ok().filter(|s| (1..1 << 30).contains(s))?;
            (!url.contains("..") && !url.contains("://")).then_some(Self { url, sha512, size })
        }

        fn name(&self) -> String {
            self.sha512.iter().take(8).map(|b| format!("{b:02x}")).collect()
        }
    }

    /// `url` into `to`, going on from what an earlier try left, of its size and SHA-512 or not at all.
    fn download(world: &World, url: &str, file: &HubFile, to: &Path, say: &dyn Fn(Step), stop: &AtomicBool) -> Result<(), End> {
        let failed = |e: String| End::Failed(tr!("Unity Hub could not be downloaded: {why}", why = e));
        let whole = |path: &Path| -> bool {
            let Ok(mut f) = std::fs::File::open(path) else { return false };
            let mut hash = ring::digest::Context::new(&ring::digest::SHA512);
            let mut buffer = vec![0u8; 1 << 20];
            loop {
                match f.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => hash.update(&buffer[..n]),
                    Err(_) => return false,
                }
            }
            hash.finish().as_ref() == file.sha512.as_slice()
        };
        let mut have = std::fs::metadata(to).map_or(0, |m| m.len());
        if have > file.size {
            let _ = crate::disk::remove_file(to);
            have = 0;
        }
        if have < file.size {
            let mut request = world.agent.get(url);
            if have > 0 {
                request = request.header("Range", format!("bytes={have}-"));
            }
            let answer = request.call().map_err(|e| failed(e.to_string()))?;
            let goes_on = answer.status() == 206;
            if !goes_on {
                have = 0;
            }
            let mut out = crate::disk::open(std::fs::File::options().write(true).create(true).truncate(!goes_on).append(goes_on), to).map_err(|e| failed(e.to_string()))?;
            let mut body = answer.into_body().into_reader();
            let mut buffer = vec![0u8; 256 << 10];
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Err(End::Stopped);
                }
                let n = body.read(&mut buffer).map_err(|e| failed(e.to_string()))?;
                if n == 0 {
                    break;
                }
                have += n as u64;
                if have > file.size {
                    let _ = crate::disk::remove_file(to);
                    return Err(failed(tr("It is not what Unity's list says.").into()));
                }
                out.write_all(&buffer[..n]).map_err(|e| failed(e.to_string()))?;
                say(Step::Hub { have, total: file.size });
            }
        }
        if have < file.size {
            return Err(failed(tr("The connection was cut. What came is kept.").into()));
        }
        if !whole(to) {
            let _ = crate::disk::remove_file(to);
            return Err(failed(tr("It is not what Unity's list says.").into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
