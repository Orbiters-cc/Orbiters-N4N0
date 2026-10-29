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

/// `project`'s repository, when it is one (`None` without `.git` or Git).
pub fn read(project: &Path) -> Option<Repo> {
    if !project.join(".git").exists() {
        return None;
    }
    let git = |args: &[&str]| -> Option<String> {
        let o = crate::hidden_command("git").arg("-C").arg(project).args(args).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let log = git(&["log", &format!("-{COMMITS}"), "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%b%x1e"])?;
    let mut repo = parse_status(&git(&["status", "--porcelain=v2", "--branch", "--untracked-files=normal"]).unwrap_or_default());
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
