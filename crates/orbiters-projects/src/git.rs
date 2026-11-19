//! A project's Git repository as the panel shows it: its branch, how far it is ahead of or behind its upstream, the
//! changes not committed yet, its last commits and the web page of its remote. Reads only (`git status`, `git log`,
//! `git remote`), on a worker.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Commits the panel lists.
pub const COMMITS: usize = 6;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub email: String,
    /// When it was committed (Unix seconds).
    pub time: u64,
    pub subject: String,
    /// The rest of its message (empty for a one-line message).
    pub body: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Repo {
    /// The branch checked out (none on a detached HEAD).
    pub branch: Option<String>,
    /// Its upstream (`origin/main`), and the commits it is ahead of and behind it.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Files changed, added or not tracked yet.
    pub changes: u32,
    pub commits: Vec<Commit>,
    /// The remote's page in a browser (`https://github.com/owner/repo`), and the remote's host (`github.com`).
    pub web: Option<String>,
    pub host: Option<String>,
}

/// Git, if there is one. On the Mac `/usr/bin/git` is a stub that puts up the system's offer to install the developer
/// tools when they are not there, which looking at a project must not do, and an app started from the Dock does not have
/// Homebrew on its `PATH`.
fn program() -> Option<std::path::PathBuf> {
    #[cfg(not(target_os = "macos"))]
    return Some("git".into());
    #[cfg(target_os = "macos")]
    {
        static FOUND: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
        FOUND
            .get_or_init(|| {
                let tools = || std::process::Command::new("/usr/bin/xcode-select").arg("-p").output().is_ok_and(|o| o.status.success());
                ["/opt/homebrew/bin/git", "/usr/local/bin/git"].iter().map(std::path::PathBuf::from).find(|p| p.is_file()).or_else(|| tools().then(|| "/usr/bin/git".into()))
            })
            .clone()
    }
}

/// `project`'s repository, when it is one (`None` without `.git` or Git).
pub fn read(project: &Path) -> Option<Repo> {
    if !project.join(".git").exists() || !inert(project) {
        return None;
    }
    let program = program()?;
    // A project can come from anyone (a world shared as a whole folder, a package's sample project) with its `.git`
    // prepared: looking at it must not run what its configuration names (PROJECTS-26). Every key that makes `status` or
    // `log` start a program is turned off for these calls, and nothing takes the index's lock that the person's own Git
    // client may want at the same moment.
    let mut off: Vec<String> = SAFE.iter().map(|s| s.to_string()).collect();
    let run = |off: &[String], args: &[&str]| -> Option<String> {
        let mut c = crate::hidden_command(&program);
        c.arg("--no-optional-locks").arg("--no-pager").arg("-C").arg(project);
        for o in off {
            c.arg("-c").arg(o);
        }
        c.args(args).env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0").env("GIT_NO_LAZY_FETCH", "1");
        output(c, LIMIT)
    };
    // Filters (`.gitattributes` with `filter=x`, `filter.x.clean = <program>`) are run by `status` on files whose times
    // changed: each one the configuration names is emptied. Listing the configuration runs nothing.
    let names = run(&off, &["config", "--list", "--name-only"])?;
    off.extend(filter_keys(&names).into_iter().map(|k| format!("{k}=")));
    let git = |args: &[&str]| run(&off, args);
    let log = git(&["log", &format!("-{COMMITS}"), "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%b%x1e"])?;
    let mut repo = parse_status(&git(&["status", "--porcelain=v2", "--branch", "--untracked-files=normal", "--ignore-submodules=all"]).unwrap_or_default());
    repo.commits = parse_log(&log);
    let remote = git(&["remote"]).and_then(|r| {
        let names: Vec<&str> = r.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        // The upstream's remote, else origin, else the first.
        let upstream = repo.upstream.as_deref().and_then(|u| u.split_once('/')).map(|(r, _)| r).filter(|r| names.contains(r));
        upstream.or_else(|| names.iter().copied().find(|n| *n == "origin")).or_else(|| names.first().copied()).map(str::to_string)
    });
    if let Some(url) = remote.and_then(|r| git(&["remote", "get-url", &r])) {
        repo.web = web_url(url.trim());
        repo.host = repo.web.as_deref().and_then(|w| url::Url::parse(w).ok()).and_then(|u| u.host_str().map(str::to_string));
    }
    Some(repo)
}

/// The configuration [`read`] gives every call: no file-system monitor (a program), no signature check (`gpg.program`),
/// no hooks, nothing from submodules (their own configuration), no transport at all (a partial clone fetches missing
/// objects lazily, through `core.sshCommand` or a credential helper).
const SAFE: [&str; 6] = [
    "core.fsmonitor=false",
    "log.showSignature=false",
    "core.hooksPath=/dev/null",
    "status.submoduleSummary=false",
    "diff.ignoreSubmodules=all",
    "protocol.allow=never",
];
/// How long one Git call may take: a large repository on a hard disk must not hold the panel's packages for ever.
const LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether the part of `project`'s repository that came with the project (its `.git/config`) holds only settings that
/// make no Git command run a program (PROJECTS-26). Git has no switch that makes a repository's own configuration
/// harmless, so a repository is read only when every key of it is on this list: a plain clone's configuration (core
/// options, remotes, branches, who commits, Git LFS's standard filter). Anything else (an include, `core.fsmonitor`, a
/// filter of its own, a partial clone, a `.git` file pointing elsewhere, a worktree's own configuration) and the panel
/// does not read the repository. The person's own (global, system) configuration is theirs and is trusted.
pub fn inert(project: &Path) -> bool {
    let dir = project.join(".git");
    if !dir.is_dir() || dir.join("config.worktree").exists() {
        return false;
    }
    match std::fs::read_to_string(dir.join("config")) {
        Ok(text) => config_is_inert(&text),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// [`inert`] on the text of a Git configuration file.
pub fn config_is_inert(text: &str) -> bool {
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        // A continued value could hide anything: not read.
        if line.ends_with('\\') {
            return false;
        }
        let rest = if let Some(head) = line.strip_prefix('[') {
            let Some(end) = head.find(']') else { return false };
            let inside = head[..end].trim();
            // The old `[section.subsection]` spelling: not what a plain clone writes.
            if inside.split_whitespace().next().is_some_and(|name| name.contains('.')) {
                return false;
            }
            section = match inside.split_once(char::is_whitespace) {
                Some((name, sub)) => format!("{}.{}", name.to_ascii_lowercase(), sub.trim().trim_matches('"')),
                None => inside.to_ascii_lowercase(),
            };
            head[end + 1..].trim()
        } else {
            line
        };
        if rest.is_empty() || rest.starts_with('#') || rest.starts_with(';') {
            continue;
        }
        let (key, value) = match rest.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"')),
            None => (rest.to_ascii_lowercase(), "true"),
        };
        if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || !inert_key(&section, &key, value) {
            return false;
        }
    }
    true
}

fn inert_key(section: &str, key: &str, value: &str) -> bool {
    let (name, sub) = match section.split_once('.') {
        Some((n, s)) => (n, Some(s)),
        None => (section, None),
    };
    match (name, sub) {
        ("core", None) => matches!(
            key,
            "repositoryformatversion" | "filemode" | "bare" | "logallrefupdates" | "symlinks" | "ignorecase" | "precomposeunicode" | "autocrlf"
                | "eol" | "safecrlf" | "longpaths" | "quotepath" | "checkstat" | "trustctime" | "untrackedcache" | "fscache"
        ),
        ("extensions", None) => key == "objectformat",
        ("remote", Some(_)) => matches!(key, "url" | "pushurl" | "fetch" | "push" | "tagopt" | "prune" | "prunetags" | "mirror" | "skipdefaultupdate"),
        ("branch", Some(_)) => matches!(key, "remote" | "merge" | "rebase" | "pushremote" | "description"),
        ("user", None) => matches!(key, "name" | "email"),
        ("lfs", None) => matches!(key, "repositoryformatversion"),
        ("lfs", Some(_)) => matches!(key, "access" | "locksverify"),
        // `git lfs install --local` writes these, exactly.
        ("filter", Some("lfs")) => match key {
            "clean" => value == "git-lfs clean -- %f",
            "smudge" => value == "git-lfs smudge -- %f",
            "process" => value == "git-lfs filter-process",
            "required" => value == "true",
            _ => false,
        },
        ("submodule", Some(_)) => matches!(key, "url" | "active" | "path" | "branch"),
        ("pull", None) => matches!(key, "rebase" | "ff"),
        ("push", None) => matches!(key, "default" | "autosetupremote"),
        _ => false,
    }
}

/// The filter keys among `git config --list --name-only`: `filter.<driver>.clean`, `.smudge` and `.process`.
fn filter_keys(names: &str) -> Vec<String> {
    names
        .lines()
        .map(str::trim)
        .filter(|k| {
            let k = k.to_ascii_lowercase();
            k.starts_with("filter.") && (k.ends_with(".clean") || k.ends_with(".smudge") || k.ends_with(".process"))
        })
        .map(str::to_string)
        .collect()
}

/// What `command` writes, if it ends well within `limit` (killed otherwise).
fn output(mut command: std::process::Command, limit: std::time::Duration) -> Option<String> {
    use std::io::Read;
    let mut child = command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().ok()?;
    status.success().then(|| String::from_utf8_lossy(&out).into_owned())
}

/// `git log` with the format [`read`] asks for.
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\x1e')
        .filter_map(|record| {
            let f: Vec<&str> = record.trim_start_matches(['\n', '\r']).splitn(7, '\x1f').collect();
            if f.len() < 6 || f[0].is_empty() {
                return None;
            }
            Some(Commit {
                hash: f[0].into(),
                short: f[1].into(),
                author: f[2].into(),
                email: f[3].into(),
                time: f[4].trim().parse().unwrap_or(0),
                subject: f[5].into(),
                body: f.get(6).map(|b| b.trim().to_string()).unwrap_or_default(),
            })
        })
        .collect()
}

/// `git status --porcelain=v2 --branch`: the branch, its upstream and the count of changed files.
pub fn parse_status(text: &str) -> Repo {
    let mut repo = Repo::default();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            repo.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            repo.upstream = Some(up.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    repo.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    repo.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            repo.changes += 1;
        }
    }
    repo
}

/// A remote's address as a page in a browser: `git@github.com:owner/repo.git`, `ssh://git@host:22/owner/repo` and
/// `https://user:token@host/owner/repo.git` all give `https://host/owner/repo` (never the credentials). Local paths and
/// other schemes give none.
pub fn web_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let (host, path) = if let Some(rest) = remote.strip_prefix("ssh://").or_else(|| remote.strip_prefix("git+ssh://")) {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        (host.split(':').next()?.to_string(), path.to_string())
    } else if remote.starts_with("https://") || remote.starts_with("http://") || remote.starts_with("git://") {
        let u = url::Url::parse(remote).ok()?;
        let host = match u.port() {
            Some(p) if u.scheme() != "git" => format!("{}:{p}", u.host_str()?),
            _ => u.host_str()?.to_string(),
        };
        (host, u.path().trim_start_matches('/').to_string())
    } else {
        // scp-like: [user@]host:path (a Windows path "C:\…" is not one).
        let (authority, path) = remote.split_once(':')?;
        if authority.len() < 2 || authority.contains(['/', '\\']) || path.starts_with('\\') || path.starts_with("//") {
            return None;
        }
        (authority.rsplit('@').next()?.to_string(), path.to_string())
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    if host.is_empty() || !host.contains('.') || path.is_empty() {
        return None;
    }
    Some(format!("https://{host}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_become_their_pages() {
        for (remote, page) in [
            ("git@github.com:Orbiters-cc/Toolkit.git", Some("https://github.com/Orbiters-cc/Toolkit")),
            ("https://github.com/Orbiters-cc/Toolkit.git", Some("https://github.com/Orbiters-cc/Toolkit")),
            ("https://enzo:ghp_secret@github.com/Orbiters-cc/Toolkit", Some("https://github.com/Orbiters-cc/Toolkit")),
            ("ssh://git@gitlab.com:2222/group/sub/project.git", Some("https://gitlab.com/group/sub/project")),
            ("git@bitbucket.org:team/repo.git/", Some("https://bitbucket.org/team/repo")),
            ("https://git.example.com:8443/me/world", Some("https://git.example.com:8443/me/world")),
            ("C:\\repos\\world", None),
            ("/srv/git/world.git", None),
            ("../world", None),
            ("file:///C:/repos/world", None),
        ] {
            assert_eq!(web_url(remote).as_deref(), page, "{remote}");
        }
    }

    /// PROJECTS-26: a project's own Git configuration names programs; looking at the project runs none of them.
    #[test]
    fn looking_at_a_repository_runs_nothing_its_configuration_names() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("World");
        std::fs::create_dir_all(&project).unwrap();
        let git = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(&project).args(args).output();
        if !git(&["init", "-q"]).is_ok_and(|o| o.status.success()) {
            eprintln!("no Git here: nothing to check"); // i18n: not shown (a test)
            return;
        }
        std::fs::write(project.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
        std::fs::write(project.join("a.txt"), "a").unwrap();
        let _ = git(&["-c", "user.name=t", "-c", "user.email=t@t", "add", "."]);
        let _ = git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "first"]);
        // Written after the commit, so that only `read` can run them.
        let marker = temp.path().join("ran").display().to_string().replace('\\', "/");
        let command = format!("sh -c 'echo x >> {marker}'");
        for (key, value) in [("core.fsmonitor", command.as_str()), ("filter.evil.clean", command.as_str()), ("log.showSignature", "true"), ("gpg.program", command.as_str())] {
            assert!(git(&["config", key, value]).unwrap().status.success());
        }
        // A file whose times changed: `status` would hash it again through the filter.
        std::fs::write(project.join("a.txt"), "b").unwrap();
        assert!(read(&project).is_none(), "a repository whose configuration names programs is not read");
        assert!(!temp.path().join("ran").exists(), "a configured program ran");
        // A plain clone's configuration: read, without taking the index's lock.
        for key in ["core.fsmonitor", "filter.evil.clean", "log.showSignature", "gpg.program"] {
            assert!(git(&["config", "--unset", key]).unwrap().status.success());
        }
        assert!(git(&["config", "remote.origin.url", "https://github.com/someone/world.git"]).unwrap().status.success());
        let repo = read(&project).expect("read");
        assert_eq!(repo.commits.len(), 1);
        assert!(repo.changes >= 1);
        assert_eq!(repo.web.as_deref(), Some("https://github.com/someone/world"));
        assert!(!project.join(".git/index.lock").exists());
    }

    #[test]
    fn only_a_plain_configuration_is_inert() {
        let plain = "[core]
	repositoryformatversion = 0
	filemode = false
	bare = false
	logallrefupdates = true
	symlinks = false
	ignorecase = true
[remote \"origin\"]
	url = https://github.com/a/b.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[branch \"main\"]
	remote = origin
	merge = refs/heads/main
[lfs]
	repositoryformatversion = 0
[filter \"lfs\"]
	clean = git-lfs clean -- %f
	smudge = git-lfs smudge -- %f
	process = git-lfs filter-process
	required = true
";
        assert!(config_is_inert(plain));
        for added in [
            "[core]
	fsmonitor = run.exe
",
            "[core]
sshCommand = evil
",
            "[include]
	path = ../other
",
            "[includeIf \"gitdir:x\"]
	path = y
",
            "[filter \"lfs\"]
	clean = evil %f
",
            "[filter \"x\"]
	clean = cat
",
            "[extensions]
	partialclone = origin
",
            "[remote \"origin\"]
	uploadpack = evil
",
            "[log]
	showSignature = true
",
            "[core.fsmonitor]
",
            "[core] fsmonitor = evil
",
            "[core]
	pager = a \\
",
        ] {
            assert!(!config_is_inert(&format!("{plain}{added}")), "{added}");
        }
    }

    #[test]
    fn only_filter_programs_are_turned_off() {
        let keys = filter_keys("core.fsmonitor\nfilter.lfs.clean\nfilter.lfs.smudge\nfilter.lfs.process\nfilter.lfs.required\nuser.name\n");
        assert_eq!(keys, ["filter.lfs.clean", "filter.lfs.smudge", "filter.lfs.process"]);
    }

    #[test]
    fn status_and_log_are_read() {
        let s = parse_status(
            "# branch.oid 1234\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b Assets/A.cs\n? Assets/New.mat\n",
        );
        assert_eq!((s.branch.as_deref(), s.upstream.as_deref(), s.ahead, s.behind, s.changes), (Some("main"), Some("origin/main"), 2, 1, 2));
        assert_eq!(parse_status("# branch.head (detached)\n").branch, None);
        let log = parse_log(
            "aaaa\x1fa\x1fEnzo\x1fe@x.cc\x1f1700000000\x1fSave the scene\x1fWhy:\nbecause\n\x1e\nbbbb\x1fb\x1fEnzo\x1fe@x.cc\x1f1690000000\x1fFirst\x1f\x1e\n",
        );
        assert_eq!(log.len(), 2);
        assert_eq!((log[0].subject.as_str(), log[0].body.as_str(), log[0].time), ("Save the scene", "Why:\nbecause", 1_700_000_000));
        assert_eq!(log[1].body, "");
    }
}
