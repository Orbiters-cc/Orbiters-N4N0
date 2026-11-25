//! The commit a release is made of. It is named before anything else, and everything after is about that commit, never
//! about whatever `HEAD` is by then: its sources are what gets built (the checkout is the commit's, to the last file, before
//! and after the build), its name is what gets signed, and its tag is what gets pushed.
//!
//! On 2026-11-24 a release's checkout was one another session was editing: its commit took that session's files in, and the
//! tag, the push and the signed `commit` were all `HEAD` at the end of a build of half an hour.

use anyhow::Context as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

/// Where files nobody committed would still be built in (a catalogue's folder, a module): nothing untracked may be there.
const BUILT: &[&str] = &["crates", "xtask", ".cargo", "Cargo.toml", "Cargo.lock"];

/// A checkout, and the commit it was at when asked.
#[derive(Debug)]
pub struct Source {
    root: PathBuf,
    pub commit: String,
    /// Nothing in it differed from the commit.
    clean: bool,
    since: SystemTime,
}

impl Source {
    /// The checkout at `root` as a release's source, or why it is none: the commit it is at, with nothing changed in it.
    pub fn clean(root: PathBuf) -> anyhow::Result<Self> {
        let source = Self::any(root)?;
        let changes = source.changes()?;
        anyhow::ensure!(
            changes.is_empty(),
            "this checkout is not {}: nothing was done. Commit what is yours, or release from a clean worktree (git worktree add --detach <folder> origin/main, git submodule update \
             --init there):\n{}",
            source.short(),
            changes.join("\n")
        );
        Ok(source)
    }

    /// The checkout at `root` as it stands, for a build that is no release.
    pub fn any(root: PathBuf) -> anyhow::Result<Self> {
        let mut source = Self { root, commit: String::new(), clean: false, since: SystemTime::now() };
        source.commit = source.git(&["rev-parse", "--verify", "HEAD^{commit}"])?;
        source.clean = source.changes()?.is_empty();
        Ok(source)
    }

    pub fn short(&self) -> &str {
        &self.commit[..self.commit.len().min(8)]
    }

    /// The commit with its subject, as the release is announced.
    pub fn named(&self) -> String {
        format!("{} ({})", self.commit, self.git(&["log", "-1", "--format=%s", &self.commit]).unwrap_or_default())
    }

    fn git(&self, args: &[&str]) -> anyhow::Result<String> {
        crate::output(Command::new("git").current_dir(&self.root).args(args)).map(|s| s.trim().to_string())
    }

    /// What in the checkout is not the commit's: its tracked files (the contracts submodule's too) and files nobody committed
    /// where the build looks.
    fn changes(&self) -> anyhow::Result<Vec<String>> {
        let mut out: Vec<String> = self.git(&["diff", "--name-status", "--no-renames", "--ignore-submodules=untracked", &self.commit, "--"])?.lines().map(String::from).collect();
        let others = [&["ls-files", "--others", "--exclude-standard", "--"], BUILT].concat();
        out.extend(self.git(&others)?.lines().map(|f| format!("?\t{f}")));
        Ok(out)
    }

    /// The tracked files written since the release began.
    fn written(&self) -> anyhow::Result<Vec<String>> {
        let files = self.git(&["ls-files", "-z", "--recurse-submodules"])?;
        let written = |f: &&str| std::fs::metadata(self.root.join(f)).and_then(|m| m.modified()).is_ok_and(|at| at > self.since);
        Ok(files.split('\0').filter(|f| !f.is_empty()).filter(written).map(String::from).collect())
    }

    /// What a build made since [`Self::clean`] was made of, as its signed release names it: the commit, once nothing in the
    /// checkout differs from it or was written meanwhile (a file changed and put back was still built as it was). A build that
    /// is no release (`any`) says that it holds changes, and is refused by [`is_commit`] where releases are put out.
    pub fn built(&self) -> anyhow::Result<String> {
        let mut wrong = self.changes()?;
        wrong.extend(self.written()?.into_iter().map(|f| format!("written\t{f}")));
        if wrong.is_empty() && self.clean {
            return Ok(self.commit.clone());
        }
        anyhow::ensure!(
            !self.clean,
            "the checkout changed while {} was being built: the build may hold other sources, so nothing was signed or published:\n{}",
            self.short(),
            wrong.join("\n")
        );
        Ok(format!("{}+changes", self.commit))
    }

    /// The commit `tag` is at here, and at `origin` (which must answer).
    fn tagged(&self, tag: &str) -> anyhow::Result<(Option<String>, Option<String>)> {
        let full = format!("refs/tags/{tag}");
        let here = self.git(&["for-each-ref", "--format=%(objectname) %(*objectname)", &full])?;
        let there = self.git(&["ls-remote", "origin", &full, &format!("{full}^{{}}")])?;
        // An annotated tag is asked for the commit it is on: the last of the two.
        let here = here.split_whitespace().last().map(String::from);
        let there = there.lines().filter_map(|l| l.split_whitespace().next()).next_back().map(String::from);
        Ok((here, there))
    }

    /// Refuses a `tag` that is already on another commit, here or at `origin`: a version is released once, and an old tag is
    /// never taken for this one's.
    pub fn may_tag(&self, tag: &str) -> anyhow::Result<()> {
        let (here, there) = self.tagged(tag)?;
        for (at, place) in [(&here, "here"), (&there, "at origin")] {
            if let Some(at) = at.as_ref().filter(|at| **at != self.commit) {
                anyhow::bail!("{tag} is on {at} {place}, not on {}: bump the version first (cargo xtask bump)", self.commit);
            }
        }
        Ok(())
    }

    /// Puts `tag` on the commit, here and at `origin`. Someone else's of the same name, made meanwhile, is never moved: this
    /// fails instead.
    pub fn tag(&self, tag: &str, message: &str) -> anyhow::Result<()> {
        self.may_tag(tag)?;
        let (here, there) = self.tagged(tag)?;
        if here.is_none() && there.is_none() {
            self.git(&["tag", "--annotate", tag, &self.commit, "--message", message])?;
        }
        if there.is_none() {
            let full = format!("refs/tags/{tag}");
            if let Err(e) = self.git(&["push", "origin", &format!("{full}:{full}")]) {
                if here.is_none() {
                    let _ = self.git(&["tag", "--delete", tag]);
                }
                return Err(e.context(format!("{tag} could not be pushed (taken meanwhile?)")));
            }
        }
        let (_, there) = self.tagged(tag)?;
        anyhow::ensure!(there.as_deref() == Some(&self.commit), "{tag} at origin is on {there:?}, not on {}", self.commit);
        println!("Tagged {tag} ({}).", self.short());
        Ok(())
    }

    /// `origin`'s branches, as they are now; a fetch that fails is no answer.
    fn fetch(&self) -> anyhow::Result<()> {
        self.git(&["fetch", "--quiet", "--no-tags", "origin"]).map(drop).context("origin could not be asked what it has")
    }

    /// Refuses a commit `origin`'s main does not have.
    pub fn on_main(&self) -> anyhow::Result<()> {
        self.fetch()?;
        let on = Command::new("git").current_dir(&self.root).args(["merge-base", "--is-ancestor", &self.commit, "origin/main"]).status()?.success();
        anyhow::ensure!(on, "the stable channel is released from main, and origin/main does not have {}: push it first", self.short());
        Ok(())
    }

    /// Makes sure `origin` has the commit: it is on one of its branches, or it is pushed to the branch this checkout is on. A
    /// checkout on no branch (a worktree at a commit) has none to push to: its commit must be there already.
    pub fn push(&self) -> anyhow::Result<()> {
        self.fetch()?;
        if !self.git(&["branch", "--remotes", "--contains", &self.commit])?.is_empty() {
            return Ok(());
        }
        let branch = self.git(&["branch", "--show-current"])?;
        anyhow::ensure!(!branch.is_empty(), "{} is on no branch of origin, and this checkout is on no branch to push: push it first", self.short());
        self.git(&["push", "origin", &format!("{}:refs/heads/{branch}", self.commit)])?;
        println!("Pushed {} to {branch}.", self.short());
        Ok(())
    }

    /// Makes sure the contracts' own `origin` has the commit of theirs this one names: on one of its branches, or pushed to its
    /// main.
    pub fn push_contracts(&self) -> anyhow::Result<()> {
        let pinned = self.git(&["rev-parse", "--verify", &format!("{}:contracts", self.commit)])?;
        let contracts = |args: &[&str]| self.git(&[&["-C", "contracts"], args].concat());
        contracts(&["fetch", "--quiet", "--no-tags", "origin"]).context("the contracts' origin could not be asked what it has")?;
        if contracts(&["branch", "--remotes", "--contains", &pinned])?.is_empty() {
            contracts(&["push", "origin", &format!("{pinned}:refs/heads/main")])?;
            println!("Pushed the contracts' {}.", &pinned[..pinned.len().min(8)]);
        }
        Ok(())
    }
}

/// Whether `text` names a commit, and nothing beside it ([`Source::built`]).
pub fn is_commit(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The checkout `root`'s repository lives beside: the main one's folder, when `root` is a worktree of it.
pub fn main_checkout(root: &std::path::Path) -> Option<PathBuf> {
    let common = crate::output(Command::new("git").current_dir(root).args(["rev-parse", "--path-format=absolute", "--git-common-dir"])).ok()?;
    let common = PathBuf::from(common.trim());
    (common.file_name()? == ".git").then(|| common.parent().map(PathBuf::from)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn git(at: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(at)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "core.autocrlf=false", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// A checkout of a commit, with a bare repository on this disk as its `origin`.
    struct Place(PathBuf);
    impl Place {
        fn new(name: &str) -> Self {
            let place = Self(std::env::temp_dir().join(format!("xtask-commit-{name}-{}", std::process::id())));
            let _ = std::fs::remove_dir_all(&place.0);
            std::fs::create_dir_all(place.0.join("work/crates")).unwrap();
            git(&place.0, &["init", "-q", "--bare", "origin.git"]);
            git(&place.work(), &["init", "-q", "-b", "main"]);
            for (key, value) in [("user.name", "t"), ("user.email", "t@example.invalid"), ("core.autocrlf", "false"), ("tag.gpgsign", "false")] {
                git(&place.work(), &["config", key, value]);
            }
            git(&place.work(), &["remote", "add", "origin", place.0.join("origin.git").to_str().unwrap()]);
            place.commit("crates/a.rs", "one");
            git(&place.work(), &["push", "-q", "origin", "main"]);
            place
        }
        fn work(&self) -> PathBuf {
            self.0.join("work")
        }
        fn commit(&self, file: &str, text: &str) -> String {
            std::fs::write(self.work().join(file), text).unwrap();
            git(&self.work(), &["add", "-A"]);
            git(&self.work(), &["commit", "-q", "-m", text]);
            git(&self.work(), &["rev-parse", "HEAD"])
        }
        fn at_origin(&self, what: &str) -> String {
            git(&self.0.join("origin.git"), &["rev-parse", what])
        }
    }
    impl Drop for Place {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A second, so that what is written next is later than what was.
    fn later() {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    /// A checkout with anything that is not the commit's is refused before anything is done, and says which files.
    #[test]
    fn a_release_is_of_a_commit_and_nothing_else() {
        let place = Place::new("clean");
        let commit = git(&place.work(), &["rev-parse", "HEAD"]);
        assert_eq!(Source::clean(place.work()).unwrap().commit, commit);
        // Notes beside the sources are nobody's business.
        std::fs::write(place.work().join("notes.md"), "mine").unwrap();
        Source::clean(place.work()).unwrap();
        for (file, staged) in [("crates/a.rs", false), ("crates/a.rs", true), ("crates/untracked.rs", false)] {
            std::fs::write(place.work().join(file), "another session's").unwrap();
            if staged {
                git(&place.work(), &["add", file]);
            }
            let refused = format!("{:#}", Source::clean(place.work()).expect_err(file));
            assert!(refused.contains(file) && refused.contains("nothing was done"), "{refused}");
            // A build that is no release goes on, and says what it is.
            let any = Source::any(place.work()).unwrap().built().unwrap();
            assert_eq!(any, format!("{commit}+changes"));
            assert!(!is_commit(&any) && is_commit(&commit));
            git(&place.work(), &["reset", "-q", "--hard"]);
            let _ = std::fs::remove_file(place.work().join("crates/untracked.rs"));
        }
    }

    /// What is signed is the commit named at the start, wherever HEAD is by the end; a checkout whose files changed
    /// meanwhile (put back or not) signs nothing.
    #[test]
    fn what_is_signed_is_the_commit_named_at_the_start() {
        let place = Place::new("built");
        let source = Source::clean(place.work()).unwrap();
        let commit = source.commit.clone();
        assert_eq!(source.built().unwrap(), commit);
        // HEAD moves, the files do not.
        git(&place.work(), &["commit", "-q", "--allow-empty", "-m", "elsewhere"]);
        assert_ne!(git(&place.work(), &["rev-parse", "HEAD"]), commit);
        assert_eq!(source.built().unwrap(), commit);
        // Another session's commit: its files are not what was named.
        later();
        place.commit("crates/b.rs", "another session's");
        let refused = format!("{:#}", source.built().expect_err("another commit's files"));
        assert!(refused.contains("crates/b.rs") && refused.contains("nothing was signed"), "{refused}");
        git(&place.work(), &["reset", "-q", "--hard", &commit]);
        // Changed and put back while it was being built.
        let source = Source::clean(place.work()).unwrap();
        later();
        std::fs::write(place.work().join("crates/a.rs"), "two").unwrap();
        std::fs::write(place.work().join("crates/a.rs"), "one").unwrap();
        assert!(format!("{:#}", source.built().expect_err("put back")).contains("crates/a.rs"));
    }

    /// The tag goes on that commit, whatever HEAD is, and is pushed there; an old one of the name that is elsewhere, here or
    /// at origin, stops the release.
    #[test]
    fn the_tag_is_on_that_commit_or_there_is_none() {
        let place = Place::new("tag");
        let source = Source::clean(place.work()).unwrap();
        let commit = source.commit.clone();
        let later = place.commit("crates/a.rs", "later");
        source.may_tag("desktop-v1").unwrap();
        source.tag("desktop-v1", "one").unwrap();
        assert_eq!(place.at_origin("desktop-v1^{commit}"), commit);
        // Done again (a release that was cut): the same.
        source.tag("desktop-v1", "one").unwrap();
        // An old local tag on another commit, never pushed: pushed as it was, it would have named that commit.
        git(&place.work(), &["tag", "--annotate", "desktop-v2", &later, "--message", "old"]);
        assert!(format!("{:#}", source.may_tag("desktop-v2").expect_err("elsewhere here")).contains(&later));
        assert!(source.tag("desktop-v2", "two").is_err());
        assert!(place.at_origin("refs/tags/desktop-v1").len() == 40 && git(&place.0.join("origin.git"), &["tag", "--list", "desktop-v2"]).is_empty(), "nothing was pushed");
        // One at origin alone (another session's), light this time.
        git(&place.work(), &["push", "-q", "origin", &format!("{later}:refs/tags/desktop-v3")]);
        assert!(format!("{:#}", source.may_tag("desktop-v3").expect_err("elsewhere at origin")).contains("at origin"));
        // Origin does not answer: no tag is said to be free.
        git(&place.work(), &["remote", "set-url", "origin", place.0.join("gone.git").to_str().unwrap()]);
        assert!(source.may_tag("desktop-v4").is_err());
    }

    /// From a worktree at a commit, on no branch: a commit origin has is released as it is, one it has not is refused before
    /// the build, and on a branch that very commit is pushed (not what HEAD became).
    #[test]
    fn a_worktree_on_no_branch_releases_what_origin_has() {
        let place = Place::new("worktree");
        let detached = place.0.join("detached");
        git(&place.work(), &["worktree", "add", "-q", "--detach", detached.to_str().unwrap()]);
        let source = Source::clean(detached.clone()).unwrap();
        source.push().unwrap();
        source.on_main().unwrap();
        source.tag("desktop-v1", "one").unwrap();
        assert_eq!(place.at_origin("desktop-v1^{commit}"), source.commit);
        // The server's folder is beside the main checkout, not beside the worktree.
        assert_eq!(main_checkout(&detached).unwrap().canonicalize().unwrap(), place.work().canonicalize().unwrap());
        assert_eq!(main_checkout(&place.work()).unwrap().canonicalize().unwrap(), place.work().canonicalize().unwrap());
        // A commit of its own that origin has not.
        std::fs::write(detached.join("crates/a.rs"), "two").unwrap();
        git(&detached, &["commit", "-q", "-am", "two"]);
        let source = Source::clean(detached).unwrap();
        assert!(source.on_main().is_err());
        assert!(format!("{:#}", source.push().expect_err("nowhere to push")).contains("push it first"));
        // On a branch it is pushed, by its name.
        let named = place.commit("crates/a.rs", "three");
        let source = Source::clean(place.work()).unwrap();
        place.commit("crates/a.rs", "four");
        source.push().unwrap();
        assert_eq!(place.at_origin("main"), named);
        // Origin does not answer: nothing is taken for pushed.
        git(&place.work(), &["remote", "set-url", "origin", place.0.join("gone.git").to_str().unwrap()]);
        assert!(source.push().is_err() && source.on_main().is_err());
    }
}
