//! What can be done with a project from its menus: the panel's "…" menu and the right-click menus of the project
//! cards and of the projects found on the computer (`orbiters_ui::popup::context_menu`) share these actions.

use orbiters_i18n::tr;
use super::art::glyph;
use super::state::{server, Outcome, State};
use crate::pages::Cx;
use egui::{Rect, Ui};
use orbiters_projects::{avatars::Avatar, discovery::Project, vpm};
use orbiters_ui::popup::{self, Item};
use orbiters_ui::{icons, theme};

/// Phosphor's eye, which the shared icon list does not have.
pub(super) const EYE: &str = "\u{E220}";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ProjectAct {
    Open,
    /// Open in Unity already: its window to the front, quit (Unity asks to save), ended (after the panel's confirmation).
    ShowUnity,
    SaveExit,
    Kill,
    Add,
    Folder,
    CopyPath,
    Duplicate,
    Hide,
    Remove,
}

/// The actions `project` offers; `opening`: with Open in Unity (or Add to my projects) and Open the folder first (the
/// cards' menus: the panel has buttons for them).
pub(super) fn project_items(state: &State, project: &Project, opening: bool) -> Vec<(Item<'static>, ProjectAct)> {
    let busy = state.running();
    let mut acts = Vec::new();
    if opening {
        let session = state.session(&project.path);
        if project.registered && session.open() {
            acts.push((Item::new(tr("Show in Unity")).icon(icons::ARROW_SQUARE_OUT), ProjectAct::ShowUnity));
            if matches!(session, orbiters_projects::editor::Session::Open { .. }) {
                acts.push((Item::new(tr("Save & Exit Unity")).icon(icons::FLOPPY_DISK), ProjectAct::SaveExit));
                acts.push((Item::new(tr("Kill Unity…")).icon(icons::X_CIRCLE).danger(), ProjectAct::Kill));
            }
        } else if project.registered && !project.missing && !busy && !state.u6.waiting(&project.path) {
            acts.push((Item::new(tr("Open in Unity")).icon(icons::PLAY), ProjectAct::Open));
        } else if !project.registered && !busy {
            acts.push((Item::new(tr("Add to my projects")).icon(icons::PLUS), ProjectAct::Add));
        }
        if !project.missing {
            acts.push((Item::new(tr("Open the folder")).icon(icons::FOLDER_OPEN), ProjectAct::Folder));
        }
    }
    // The panel's folder button gives its place to Unity's controls while the project is open.
    if !opening && !project.missing && state.session(&project.path).open() {
        acts.push((Item::new(tr("Open the folder")).icon(icons::FOLDER_OPEN), ProjectAct::Folder));
    }
    acts.push((Item::new(tr("Copy the folder's path")).icon(icons::COPY), ProjectAct::CopyPath));
    if !project.missing && !busy {
        acts.push((Item::new(tr("Duplicate…")).icon(icons::COPY_SIMPLE), ProjectAct::Duplicate));
    }
    acts.push((
        if state.is_hidden(&project.path) { Item::new(tr("Show this project")).icon(EYE) } else { Item::new(tr("Hide this project")).icon(icons::EYE_SLASH) },
        ProjectAct::Hide,
    ));
    if project.registered && !busy {
        acts.push((Item::new(tr("Remove from the list")).icon(glyph::TRASH_SIMPLE).danger(), ProjectAct::Remove));
    }
    acts
}

/// Does `act` to `project`; a dialog it opens grows out of `anchor`. `sheet`: the panel slides over the cards (a
/// project hidden from it slides it away rather than closing it at once).
pub(super) fn project_act(ctx: &egui::Context, cx: &mut Cx, state: &mut State, project: &Project, act: ProjectAct, anchor: Rect, sheet: bool) {
    match act {
        ProjectAct::Open => {
            state.open_unity(ctx, project, false);
        }
        ProjectAct::ShowUnity => state.show_editor(ctx, project),
        ProjectAct::SaveExit => state.save_and_exit(ctx, project),
        // Its confirmation is the panel's: the project opens there, Kill asking first.
        ProjectAct::Kill => {
            state.kill_confirm = Some(project.path.clone());
            state.open(ctx, project.path.clone());
        }
        ProjectAct::Add => {
            let path = project.path.clone();
            state.task(ctx, tr("Adding the project…"), move || {
                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, true))?;
                Ok(Outcome::Added(path, tr("Project added: VCC and ALCOM list it too").into()))
            });
        }
        ProjectAct::Folder => {
            let path = project.path.clone();
            state.task(ctx, tr("Opening the folder…"), move || {
                orbiters_projects::reveal(&path)?;
                Ok(Outcome::Picked(None))
            });
        }
        ProjectAct::CopyPath => {
            ctx.copy_text(project.path.display().to_string());
            cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, tr("Path copied"));
        }
        ProjectAct::Duplicate => super::duplicate::open(ctx, state, project, anchor),
        ProjectAct::Hide => {
            let hidden = state.is_hidden(&project.path);
            state.set_hidden(&project.path, !hidden);
            if hidden {
                cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, tr!("{name} is listed again", name = project.name));
            } else {
                cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr!("{name} is hidden: Hidden, above the cards, shows it again", name = project.name));
                // Gone from the list: the panel showing it moves on (a sheet slides away).
                if !state.show_hidden && state.selected.as_ref() == Some(&project.path) {
                    if sheet {
                        state.panel_closing = true;
                    } else {
                        state.selected = None;
                    }
                }
            }
        }
        ProjectAct::Remove => {
            let path = project.path.clone();
            if state.selected.as_ref() == Some(&path) {
                state.selected = None;
            }
            state.task(ctx, tr("Removing from the list…"), move || {
                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, false))?;
                Ok(Outcome::Changed(tr("Removed from the shared project list. Its files stay.").into()))
            });
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RowAct {
    OpenVrchat,
    CopyId,
    ChangePicture,
    AddN4n0,
    MyAvatar,
    /// The linked Blender file of this index.
    Blender(usize),
}

/// The right-click menu of an avatar's or a world's row in the panel (`row`: the whole row): its page on VRChat, its
/// ID, a new picture on VRChat when it is the member's, Add N4N0 and My Avatar for an avatar of a listed project, and
/// its Blender files.
pub(super) fn row_menu(ui: &Ui, cx: &mut Cx, state: &mut State, project: &Project, a: &Avatar, row: &egui::Response) {
    let world = a.is_world();
    let links = super::blend::links(state, a);
    let mut acts: Vec<(String, &'static str, RowAct)> = vec![
        (tr("Open on VRChat").into(), icons::ARROW_SQUARE_OUT, RowAct::OpenVrchat),
        (if world { tr("Copy the world's ID") } else { tr("Copy the avatar's ID") }.into(), icons::COPY, RowAct::CopyId),
    ];
    if super::vrchat_picture::can_change(cx, state, &a.id) {
        acts.push((tr("Change the picture on VRChat…").into(), icons::UPLOAD_SIMPLE, RowAct::ChangePicture));
    }
    if !world && project.registered && !state.running() {
        acts.push((tr("Add N4N0").into(), icons::N4N0_MARK, RowAct::AddN4n0));
        acts.push((tr("Install My Avatar only").into(), icons::PERSON, RowAct::MyAvatar));
    }
    if !state.running() {
        for (k, l) in links.iter().enumerate() {
            let label = match l.blend.file_name() {
                Some(n) if links.len() > 1 => tr!("Open {file} in Blender", file = n.to_string_lossy()),
                _ => tr("Open in Blender").into(),
            };
            acts.push((label, icons::BLENDER, RowAct::Blender(k)));
        }
    }
    let items: Vec<Item> = acts.iter().map(|(label, g, _)| Item::new(label).icon(g)).collect();
    let Some(k) = popup::context_menu(ui, row, ("row", &a.id), &items) else { return };
    let ctx = ui.ctx().clone();
    match acts[k].2 {
        RowAct::OpenVrchat => {
            let url = format!("https://vrchat.com/home/{}/{}", if world { "world" } else { "avatar" }, a.id);
            if let Err(e) = crate::controller::open_url(&url) {
                cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, tr!("The page could not open: {error}", error = e));
            }
        }
        RowAct::CopyId => {
            ctx.copy_text(a.id.clone());
            cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Success, tr("ID copied"));
        }
        RowAct::ChangePicture => super::vrchat_picture::change(&ctx, cx, state, a),
        RowAct::AddN4n0 | RowAct::MyAvatar => {
            let n4n0 = acts[k].2 == RowAct::AddN4n0;
            state.modal_fill = if n4n0 { theme::control() } else { theme::INK_850 };
            state.install(&ctx, project.clone(), a.clone(), n4n0, server(cx), row.rect);
        }
        RowAct::Blender(i) => super::blend::open(&ctx, state, &links[i]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn project(registered: bool, missing: bool) -> Project {
        Project { path: PathBuf::from("H:/Unity projects/A"), name: "A".into(), registered, missing, ..Default::default() }
    }

    fn acts(state: &State, p: &Project, opening: bool) -> Vec<ProjectAct> {
        project_items(state, p, opening).into_iter().map(|(_, a)| a).collect()
    }

    #[test]
    fn menus_offer_what_the_project_allows() {
        let state = State::default();
        use ProjectAct::*;
        assert_eq!(acts(&state, &project(true, false), true), [Open, Folder, CopyPath, Duplicate, Hide, Remove]);
        assert_eq!(acts(&state, &project(true, false), false), [CopyPath, Duplicate, Hide, Remove], "the panel's own menu");
        assert_eq!(acts(&state, &project(false, false), true), [Add, Folder, CopyPath, Duplicate, Hide], "found on the computer");
        assert_eq!(acts(&state, &project(true, true), true), [CopyPath, Hide, Remove], "its folder is missing");
    }
}
