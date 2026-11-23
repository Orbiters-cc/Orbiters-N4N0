//! Update the Toolkit, from the message that says an avatar could not hide itself ([`crate::controller::hide_self`]): the project
//! that holds the avatar (found by its blueprint ID, as Add to my avatar finds it) gets the current release of Orbiters Toolkit
//! and opens in Unity, ready to upload from VRChat's own SDK panel. **Nothing is uploaded by this app**: an upload stays the
//! person's press, in VRChat's window. An avatar from no project on this computer gets a polite word instead of a button that
//! could only fail: it still works, the app takes it out of its pictures in software (orbiters_reconstruct::wearer).

use orbiters_i18n::tr;
use super::state::{Outcome, State};
use orbiters_projects::discovery::Project;
use orbiters_projects::vpm;

/// What can be done for an avatar, as far as the projects found so far tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolkitOffer {
    /// Its project, by name.
    Project(String),
    /// Still looking through this computer's projects.
    Looking,
    /// No project here holds it.
    NotHere,
    /// Not offered, and why: several projects hold it, or only a Unity 6 trial does (verification's blocker 3).
    Refused(String),
}

/// Among `projects`, the one that holds the avatar `id`, never a Unity 6 trial (`trial`): a trial shares its original's
/// avatar IDs, and an upload from Unity 6 would end the avatar's uploads from Unity 2022. A project VCC lists comes before
/// one merely found on a disk. Several listed projects holding it is not decided here: `Err` names them.
pub fn project_of<'a>(projects: &'a [Project], id: &str, trial: impl Fn(&Project) -> bool) -> Result<Option<&'a Project>, Vec<String>> {
    let holds = |p: &&Project| !p.missing && p.avatars.iter().any(|a| a.id == id) && !trial(p);
    let listed: Vec<&Project> = projects.iter().filter(|p| p.registered).filter(holds).collect();
    match listed.len() {
        0 => Ok(projects.iter().find(holds)),
        1 => Ok(Some(listed[0])),
        _ => Err(listed.iter().map(|p| p.name.clone()).collect()),
    }
}

/// What the person reads when no project here holds the avatar.
pub fn refusal(avatar: Option<&str>) -> String {
    match avatar {
        Some(avatar) => tr!("{avatar} is not in a Unity project on this PC, so there is nothing here to update. Nothing needs doing: it still comes out of your pictures this way.", avatar = avatar),
        None => tr("This avatar is not in a Unity project on this PC, so there is nothing here to update. Nothing needs doing: it still comes out of your pictures this way.").to_string(),
    }
}

impl State {
    /// [`ToolkitOffer`] of the avatar `id`.
    pub fn toolkit_offer(&self, id: &str) -> ToolkitOffer {
        let is_trial = |p: &Project| self.u6.trial_at(&p.path).is_some();
        match project_of(&self.scan.projects, id, is_trial) {
            Ok(Some(p)) => ToolkitOffer::Project(p.name.clone()),
            Err(names) => ToolkitOffer::Refused(tr!("This avatar is in several of your projects ({names}): update the Toolkit in the one you upload from, in Unity projects.", names = names.join(", "))),
            Ok(None) if self.scan.projects.iter().any(|p| !p.missing && is_trial(p) && p.avatars.iter().any(|a| a.id == id)) => ToolkitOffer::Refused(
                tr("Only a Unity 6 trial holds this avatar here. Uploading it from Unity 6 would end its uploads from Unity 2022, so Orbiters does not prepare an upload from it.").to_string(),
            ),
            Ok(None) if self.scanning() || !self.started => ToolkitOffer::Looking,
            Ok(None) => ToolkitOffer::NotHere,
        }
    }

    /// Starts this computer's look through its projects, when nothing has yet: an idle Capture page must not start one for a
    /// message that may never need it, so the card calls this only while it shows.
    pub fn toolkit_look(&mut self, ctx: &egui::Context) {
        self.ensure(ctx);
    }

    /// Updates the Toolkit in the project that holds the avatar `id`, then opens it in Unity (or brings it forward).
    pub fn update_toolkit(&mut self, ctx: &egui::Context, id: &str, avatar: Option<&str>) {
        let is_trial = |p: &Project| self.u6.trial_at(&p.path).is_some();
        let Ok(Some(project)) = project_of(&self.scan.projects, id, is_trial).map(|p| p.cloned()) else { return };
        let name = project.name.clone();
        let avatar = avatar.map(str::to_string);
        self.task(ctx, &tr!("Updating Orbiters Toolkit in {name}…", name = name), move || {
            match orbiters_projects::runtime()?.block_on(vpm::install_orbiters(&orbiters_projects::vcc_dir(), &project.path, &["orbiters.toolkit"])) {
                Ok(()) => {}
                // Not cancelled: the plan making it coherent, in its dialog.
                Err(e) => match e.downcast::<orbiters_projects::coherent::Conflicted>() {
                    Ok(c) => return Ok(Outcome::Conflict(project.path.clone(), c.plan)),
                    Err(e) => return Err(e),
                },
            }
            orbiters_projects::open_project(&project)?;
            Ok(Outcome::Changed(match &avatar {
                Some(avatar) => tr!("{name} has the current Orbiters Toolkit and is open in Unity. Upload {avatar} from VRChat's SDK panel when you're ready: N4N0 uploads nothing by itself.", name = name, avatar = avatar),
                None => tr!("{name} has the current Orbiters Toolkit and is open in Unity. Upload your avatar from VRChat's SDK panel when you're ready: N4N0 uploads nothing by itself.", name = name),
            }))
        });
    }

    /// The update under way, for its button to say so.
    pub fn toolkit_running(&self) -> bool {
        self.running()
    }

    /// What went wrong with the last action, once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_projects::avatars::Avatar;

    fn project(name: &str, registered: bool, missing: bool, ids: &[&str]) -> Project {
        Project {
            name: name.into(),
            path: name.into(),
            registered,
            missing,
            avatars: ids.iter().map(|id| Avatar { id: id.to_string(), ..Default::default() }).collect(),
            ..Default::default()
        }
    }

    /// The project VCC lists comes first; one whose folder is gone never; none holds it: none.
    #[test]
    fn the_avatar_is_found_in_its_project() {
        let projects = [
            project("found on a disk", false, false, &["avtr_a"]),
            project("gone", true, true, &["avtr_a"]),
            project("listed", true, false, &["avtr_a", "avtr_b"]),
            project("other", true, false, &["avtr_c"]),
        ];
        let none = |_: &Project| false;
        assert_eq!(project_of(&projects, "avtr_a", none).unwrap().map(|p| p.name.as_str()), Some("listed"));
        assert_eq!(project_of(&projects[..2], "avtr_a", none).unwrap().map(|p| p.name.as_str()), Some("found on a disk"));
        assert!(project_of(&projects, "avtr_z", none).unwrap().is_none());
    }

    /// Verification blocker 3: a Unity 6 trial sharing the avatar's ID is never the one prepared for an upload, and two
    /// listed projects holding it are not decided by order.
    #[test]
    fn a_trial_or_an_ambiguous_avatar_is_never_picked_for_an_upload() {
        let projects = [project("listed", true, false, &["avtr_a"]), project("listed (Unity 6)", true, false, &["avtr_a"])];
        let trial = |p: &Project| p.name.ends_with("(Unity 6)");
        assert_eq!(project_of(&projects, "avtr_a", trial).unwrap().map(|p| p.name.as_str()), Some("listed"));
        assert!(project_of(&projects[1..], "avtr_a", trial).unwrap().is_none(), "a trial alone is not offered");
        let two = [project("one", true, false, &["avtr_a"]), project("two", true, false, &["avtr_a"])];
        assert_eq!(project_of(&two, "avtr_a", |_: &Project| false).unwrap_err(), vec!["one".to_string(), "two".to_string()]);
    }

    /// A polite no, which does not send the person looking for a problem.
    #[test]
    fn an_avatar_from_nowhere_here_is_refused_politely() {
        let no = refusal(Some("The 1MB Ultirex"));
        assert!(no.contains("The 1MB Ultirex") && no.contains("not in a Unity project on this PC") && no.contains("still comes out"), "{no}");
        assert!(refusal(None).starts_with("This avatar"));
    }
}
