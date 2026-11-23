//! VRChat's move from Unity 2022 to Unity 6 (REQUEST22; design DESIGN22-unity6.md, its "Revision after the attack"):
//! where each project stands, from its own files ([`standing`]); VRChat's release matrix ([`matrix`]); the exact editor
//! ([`editors`]); what can be said of a project's packages and content before a try ([`compat`]); the disk a trial
//! needs ([`budget`]); the trial itself, handed to Creator Companion for the migration ([`trial`], [`vcc`]); and what
//! each opening in Unity 6 showed ([`outcome`]).

pub mod budget;
pub mod compat;
pub mod editors;
pub mod matrix;
pub mod outcome;
pub mod standing;
pub mod trial;
pub mod vcc;

use orbiters_i18n::tr;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The release key the served release list must be signed with.
pub fn key() -> Option<[u8; 32]> {
    orbiters_update::public_key()
}

/// An original and its Unity 6 trial, both folders still the ones recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub original: PathBuf,
    pub trial: PathBuf,
    pub job: String,
}

/// What the page shows of the transition, read beside a scan: the release list kept, each project's standing, the
/// trials and where they stand. It only reads: no network, no resolver, no write anywhere.
#[derive(Clone, Debug)]
pub struct Look {
    /// The list shown (built in, or a verified served copy): display only.
    pub matrix: matrix::Matrix,
    /// The projects this look was asked for, in order: an answer for another list is not applied.
    pub projects: Vec<PathBuf>,
    /// Standings by the built-in list only: they decide actions, so a served row never does (verification's major 7).
    pub standings: Vec<(PathBuf, standing::Standing)>,
    /// Each standing's place by [`crate::discovery::key`] of its path.
    pub index: HashMap<String, usize>,
    pub trials: Vec<(trial::Job, trial::Status)>,
    pub pairs: Vec<Pair>,
    /// Whether the record can be trusted.
    pub health: trial::Health,
    /// Keys of trial folders that are there now (the page never asks the disk while drawing).
    pub present: HashSet<String>,
}

impl Look {
    pub fn standing(&self, path: &Path) -> Option<&standing::Standing> {
        self.index.get(&crate::discovery::key(path)).map(|&i| &self.standings[i].1)
    }
}

/// [`Look`] for `projects`, with the record in `dir` (app data). `running`: the trials this window is preparing.
pub fn look(dir: &Path, projects: &[PathBuf], running: &[String]) -> Look {
    let matrix = matrix::load(dir, key().as_ref());
    let policy = matrix::Matrix::built_in();
    let standings: Vec<(PathBuf, standing::Standing)> = projects.iter().map(|p| (p.clone(), standing::read(p, &policy))).collect();
    let index = standings.iter().enumerate().map(|(i, (p, _))| (crate::discovery::key(p), i)).collect();
    let record = trial::record(dir);
    let trials: Vec<(trial::Job, trial::Status)> = record
        .jobs
        .into_iter()
        .map(|j| {
            let s = trial::status(&j, running.contains(&j.id));
            (j, s)
        })
        .collect();
    // Lineage needs a positive match of both folders (verification's major 10).
    let pairs = trials
        .iter()
        .filter(|(_, s)| !matches!(s, trial::Status::Replaced | trial::Status::Missing | trial::Status::Failed { .. } | trial::Status::Interrupted { complete: false }))
        .filter(|(j, _)| j.source_verified())
        .map(|(j, _)| Pair { original: j.source.path.clone(), trial: j.unity6().path.clone(), job: j.id.clone() })
        .collect();
    let present = trials
        .iter()
        .flat_map(|(j, _)| [Some(&j.trial), j.migrated.as_ref()])
        .flatten()
        .filter(|p| p.path.exists())
        .map(|p| crate::discovery::key(&p.path))
        .collect();
    Look { matrix, projects: projects.to_vec(), standings, index, trials, pairs, health: record.health, present }
}

/// What is in the way of a trial, said in a sentence each.
pub fn describe_blockers(blockers: &[trial::Blocker]) -> String {
    blockers
        .iter()
        .map(|b| match b {
            trial::Blocker::NotOn2022(_) => tr("A trial starts from a VRChat project on Unity 2022 with its SDK installed.").to_string(),
            trial::Blocker::Open => tr("Unity has this project open: save your work and close Unity first.").to_string(),
            trial::Blocker::Incomplete => tr("Some of its packages are missing or not at the version its lock names: restore them first.").to_string(),
            trial::Blocker::OnlyInLibrary(names) => {
                tr!("Only its Library holds {packages}: a copy without Library would lose them. Open it once in Unity 2022 so they are installed.", packages = names.join(", "))
            }
            trial::Blocker::LocalMissing(names) => tr!("Local packages it uses are not where it says: {packages}.", packages = names.join(", ")),
            trial::Blocker::Unsupported(names) => tr!("A copy could not keep these packages whole: {packages}. Make them ordinary packages of the project first.", packages = names.join(", ")),
            trial::Blocker::Trial => tr("This project is itself a Unity 6 trial, or the trials record cannot be read.").to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every file under `root` with its bytes, and every folder: equal before and after means nothing was written.
    pub(crate) fn snapshot(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .flatten()
            .map(|e| {
                let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned();
                (rel, e.file_type().is_file().then(|| std::fs::read(e.path()).unwrap()))
            })
            .collect()
    }

    fn set_read_only(root: &Path, on: bool) {
        for e in walkdir::WalkDir::new(root).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            let mut perm = std::fs::metadata(e.path()).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(on);
            std::fs::set_permissions(e.path(), perm).unwrap();
        }
    }

    /// M10: what the gallery reads beside a scan writes nothing in the projects (read-only ones too) nor in app data, and
    /// takes no network: the release list is the one kept, a served copy only ever comes through [`matrix::refresh`].
    #[test]
    fn a_look_beside_the_scan_only_reads() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let projects: Vec<PathBuf> = (0..3).map(|i| temp.path().join(format!("P{i}"))).collect();
        standing::tests::project(&projects[0], Some(standing::tests::U2022), &[("com.vrchat.base", "3.10.6")], &[("com.vrchat.base", "3.10.6", "2022.3")]);
        standing::tests::project(&projects[1], Some(standing::tests::U6), &[("com.vrchat.base", "3.11.0-beta.1")], &[("com.vrchat.base", "3.11.0-beta.1", "6000.0")]);
        standing::tests::project(&projects[2], Some(standing::tests::U2022), &[], &[]);
        set_read_only(temp.path(), true);
        let before = snapshot(temp.path());
        let l = look(&app, &projects, &[]);
        let after = snapshot(temp.path());
        set_read_only(temp.path(), false);
        assert_eq!(before, after, "nothing written anywhere");
        let states: Vec<_> = l.standings.iter().map(|(_, s)| s.state).collect();
        assert_eq!(states, [standing::State::On2022, standing::State::OnUnity6, standing::State::NotVrchat]);
        assert_eq!(l.matrix.origin, matrix::Origin::BuiltIn);
        assert!(l.trials.is_empty() && l.pairs.is_empty());
    }

    #[test]
    fn blockers_are_said_in_the_language_of_the_moment() {
        let text = describe_blockers(&[trial::Blocker::Open, trial::Blocker::OnlyInLibrary(vec!["com.example.git".into()])]);
        assert!(text.contains("close Unity") && text.contains("com.example.git"), "{text}");
        let french = orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || describe_blockers(&[trial::Blocker::Open]));
        assert!(!french.contains("close Unity"), "{french}");
    }
}
