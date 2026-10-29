use super::state::{server, Modal, Outcome, State};
use crate::pages::Cx;
use egui::{pos2, vec2, Align2, Id, Rect, Sense, Ui, UiBuilder};
use orbiters_projects::{
    avatars::Avatar,
    discovery::{self, Project},
    setup, templates, vpm,
};
use orbiters_ui::{
    anim::{self, Spring},
    icons,
    theme::{self, weight, Txt},
    widgets::{self, Button, Size, Variant},
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn text(ui: &mut Ui, s: &str, size: f32, color: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), size + 9.), Sense::hover());
    widgets::text_truncated(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &Txt::new(s, size).color(color), r.width());
}
fn title(ui: &mut Ui, s: &str) {
    text(ui, s, 18., theme::FG);
}
fn button(ui: &mut Ui, label: &str, icon: &str, primary: bool, enabled: bool) -> bool {
    widgets::activated(
        &Button::new(label).icon(icon).size(Size::Sm).variant(if primary { Variant::Primary } else { Variant::Secondary }).enabled(enabled).show(ui),
    )
}
fn error(ui: &mut Ui, state: &mut State) {
    if let Some(e) = state.error.clone() {
        egui::Frame::NONE.fill(theme::alpha(theme::DANGER, 0.09)).corner_radius(12.).inner_margin(12).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::DANGER, &e);
                if button(ui, "Dismiss", icons::X, false, true) {
                    state.error = None;
                }
            });
        });
    }
}
fn progress(ui: &mut Ui, state: &State) {
    if state.action.running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(&state.action_label);
        });
        let p = state.copy_progress.lock().unwrap().clone();
        if p.total > 0 {
            ui.add(egui::ProgressBar::new(p.copied as f32 / p.total as f32).text(format!("{} · {} / {}", p.phase, p.copied, p.total)));
            text(ui, &p.file, 12., theme::FG_3);
            if p.phase == "Copying project" && button(ui, "Cancel copy", icons::X, false, true) {
                state.copy_cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    state.ensure(&ctx);
    if state.visit != cx.visit {
        state.visit = cx.visit;
        if state.scan_rx.is_none() {
            state.refresh(&ctx, false);
        }
    }
    state.poll(&ctx, cx);
    let from = orbiters_ui::blur::next_shape(ui);
    let rect = ui.max_rect();
    let head = Rect::from_min_size(pos2(rect.left() + 32., rect.top() + cx.top + 8.), vec2(rect.width() - 64., 90.));
    egui::ScrollArea::vertical().id_salt("unity-projects").auto_shrink([false, false]).scroll_bar_rect(crate::pages::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 32 }).show(ui, |ui| {
            ui.add_space(cx.top + 108.);
            ui.spacing_mut().item_spacing = vec2(12., 16.);
            ui.add_enabled_ui(state.selected.is_none(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if button(ui, "Add existing project", icons::FOLDER_OPEN, false, !state.action.running()) {
                        state.task(&ctx, "Choose a Unity project…", move || {
                            if let Some(p) = rfd::FileDialog::new().set_title("Add a Unity project").pick_folder() {
                                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &p, true))?;
                                Ok(Outcome::Changed("Project added. VCC and ALCOM can see it too.".into()))
                            } else {
                                Ok(Outcome::Picked(None))
                            }
                        });
                    }
                    if button(ui, "Create new project", icons::PLUS, true, !state.action.running()) {
                        state.modal = Some(Modal::New);
                    }
                    if button(ui, "Manage package repos", icons::STACK, false, !state.action.running()) {
                        state.modal = Some(Modal::Repos);
                        state.catalog(&ctx, server(cx));
                    }
                    if button(ui, if state.scan_rx.is_some() { "Scanning…" } else { "Refresh" }, icons::ARROW_COUNTER_CLOCKWISE, false, state.scan_rx.is_none())
                    {
                        state.refresh(&ctx, true);
                        state.load_view(&ctx, true);
                        state.thumb_ids.clear();
                    }
                });
                error(ui, state);
                progress(ui, state);
                ui.horizontal(|ui| {
                    widgets::search_field(ui, "project-search", &mut state.query, "Find a project", ui.available_width().min(360.));
                });
                if !state.scan.warnings.is_empty() {
                    for warning in &state.scan.warnings {
                        ui.colored_label(theme::WARN, warning);
                    }
                }
                let q = state.query.to_lowercase();
                let projects: Vec<_> = state
                    .scan
                    .projects
                    .iter()
                    .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.path.to_string_lossy().to_lowercase().contains(&q))
                    .cloned()
                    .collect();
                let registered: Vec<_> = projects.iter().filter(|p| p.registered).cloned().collect();
                let proposals: Vec<_> = projects.iter().filter(|p| !p.registered).cloned().collect();
                title(ui, &format!("Your projects · {}", registered.len()));
                if registered.is_empty() {
                    empty(
                        ui,
                        if state.scan_rx.is_some() { "Finding your Unity projects…" } else { "Your avatar starts here" },
                        "Add a Unity project or create one from a VRChat template.",
                    );
                }
                grid(ui, cx, state, &registered, false);
                if !proposals.is_empty() {
                    ui.add_space(12.);
                    title(ui, &format!("Found on this computer · {}", proposals.len()));
                    text(ui, "Add a project to manage its avatars and packages here, in VCC and in ALCOM.", 13., theme::FG_3);
                    grid(ui, cx, state, &proposals, true);
                }
                if state.scan_rx.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Looking through local disks in the background. Your projects are ready to use.");
                    });
                }
            });
        });
    });
    crate::pages::under_top(ui, cx, "unity-projects", from, head.bottom() + 8.);
    crate::pages::heading(ui, head, "Create & prepare", "Unity projects", Some("Your avatars, worlds and packages. Everything in one place."));
    if state.selected.is_some() {
        panel(ui, cx, state);
    }
    modals(ui, cx, state);
}
fn empty(ui: &mut Ui, heading: &str, sub: &str) {
    egui::Frame::NONE.fill(theme::surface()).corner_radius(18.).inner_margin(24).show(ui, |ui| {
        title(ui, heading);
        ui.label(egui::RichText::new(sub).color(theme::FG_3));
    });
}
fn grid(ui: &mut Ui, cx: &mut Cx, state: &mut State, projects: &[Project], proposal: bool) {
    let columns = (ui.available_width() / 250.).floor().max(1.) as usize;
    let width = (ui.available_width() - 16. * (columns - 1) as f32) / columns as f32;
    for row in projects.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.;
            for p in row {
                ui.push_id((&p.path, proposal), |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(width, 236.), Sense::hover());
                    let response = ui.interact(r, ui.scope_id().with("card"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    let hover = anim::toggle(ui.ctx(), response.id.with("hover"), response.hovered(), Spring::SNAPPY);
                    let mut card = ui.new_child(UiBuilder::new().max_rect(r));
                    if proposal {
                        card.multiply_opacity(0.63);
                    }
                    widgets::frame(card.painter(), r, 18., theme::mix(theme::surface(), theme::INK_750, hover * 0.45));
                    let image = Rect::from_min_max(r.min + vec2(4., 4.), pos2(r.right() - 4., r.top() + 142.));
                    project_image(&card, state, p, image);
                    widgets::text_truncated(
                        card.painter(),
                        pos2(r.left() + 16., r.top() + 164.),
                        Align2::LEFT_CENTER,
                        &Txt::new(&p.name, 16.).weight(weight::MEDIUM),
                        r.width() - 32.,
                    );
                    widgets::text_truncated(
                        card.painter(),
                        pos2(r.left() + 16., r.top() + 187.),
                        Align2::LEFT_CENTER,
                        &Txt::new(format!("Unity {} · {}", p.unity, p.kind.label()), 12.).color(theme::FG_3),
                        r.width() - 32.,
                    );
                    widgets::text_truncated(
                        card.painter(),
                        pos2(r.left() + 16., r.top() + 215.),
                        Align2::LEFT_CENTER,
                        &Txt::new(if p.missing { "Folder unavailable".into() } else { crate::pages::local_time_unix(p.modified) }, 11.).color(theme::FG_3),
                        r.width() - if proposal { 96. } else { 32. },
                    );
                    let mut add = false;
                    if proposal {
                        let b = Rect::from_min_size(pos2(r.right() - 81., r.bottom() - 37.), vec2(66., 28.));
                        add = widgets::activated(
                            &Button::new("Add +").variant(Variant::Secondary).size(Size::Sm).enabled(!state.action.running()).show_at(&mut card, b),
                        );
                    }
                    if add {
                        let path = p.path.clone();
                        state.task(ui.ctx(), "Adding project…", move || {
                            orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, true))?;
                            Ok(Outcome::Changed("Project added".into()))
                        });
                    } else if widgets::activated(&response) {
                        state.open(ui.ctx(), p.path.clone());
                    }
                });
            }
        });
        ui.add_space(8.);
    }
    let _ = cx;
}
fn project_image(ui: &Ui, state: &State, p: &Project, r: Rect) {
    let count = p.avatars.len().min(9);
    if count == 0 {
        widgets::frame(ui.painter(), r, 14., theme::INK_800);
        widgets::icon(ui.painter(), r.center(), if p.kind == discovery::Kind::Worlds { icons::GLOBE } else { icons::CUBE }, 42., theme::FG_4);
        return;
    }
    let cols = if count == 1 {
        1
    } else if count <= 4 {
        2
    } else {
        3
    };
    let rows = count.div_ceil(cols);
    let gap = 3.;
    let size = vec2((r.width() - gap * (cols - 1) as f32) / cols as f32, (r.height() - gap * (rows - 1) as f32) / rows as f32);
    for (i, a) in p.avatars.iter().take(count).enumerate() {
        let tile = Rect::from_min_size(r.min + vec2((i % cols) as f32 * (size.x + gap), (i / cols) as f32 * (size.y + gap)), size);
        avatar_image(ui, state, a, tile);
    }
}
fn avatar_image(ui: &Ui, state: &State, a: &Avatar, r: Rect) {
    widgets::frame(ui.painter(), r, 12., theme::INK_800);
    if let Some(t) = state.thumbs.get(&a.id) {
        let size = t.size_vec2();
        let ratio = r.aspect_ratio();
        let source = size.x / size.y;
        let uv = if source > ratio {
            let w = ratio / source;
            Rect::from_min_max(pos2((1. - w) / 2., 0.), pos2((1. + w) / 2., 1.))
        } else {
            let h = source / ratio;
            Rect::from_min_max(pos2(0., (1. - h) / 2.), pos2(1., (1. + h) / 2.))
        };
        egui::Image::new(t).uv(uv).corner_radius(12.).paint_at(ui, r);
    } else {
        widgets::icon(ui.painter(), r.center(), icons::PERSON_ARMS_SPREAD, 32., theme::FG_4);
    }
}

fn panel(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let Some(project) = state.scan.projects.iter().find(|p| Some(&p.path) == state.selected.as_ref()).cloned() else {
        state.selected = None;
        return;
    };
    let page = ui.max_rect();
    let width = (page.width() * 0.72).clamp(420., 700.).min(page.width());
    let v = anim::spring_from(ui.ctx(), Id::unique(("project-panel", state.panel_serial)), [0.], [1.], Spring::SOFT)[0];
    let r = Rect::from_min_max(pos2(page.right() - width + (1. - v) * width, page.top() + cx.top + 6.), page.max + vec2((1. - v) * width, 0.));
    ui.painter().rect_filled(page, 0., theme::black(0.45 * v));
    widgets::frame(ui.painter(), r, 20., theme::INK_850);
    let mut panel = ui.new_child(UiBuilder::new().max_rect(r.shrink(24.)).id_salt("project-panel"));
    panel.set_clip_rect(r.intersect(ui.clip_rect()));
    panel.spacing_mut().item_spacing = vec2(10., 14.);
    panel.horizontal(|ui| {
        title(ui, &project.name);
    });
    let close = Rect::from_min_size(pos2(r.right() - 56., r.top() + 18.), vec2(32., 32.));
    if widgets::activated(&widgets::IconButton::new(icons::X, "Close project").show_at(&mut panel, close)) {
        state.selected = None;
        return;
    }
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().id_salt("project-detail").show(&mut panel, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(project.path.display().to_string()).color(theme::FG_3).size(12.)).wrap());
        text(ui, &format!("Unity {} · {}", project.unity, project.kind.label()), 13., theme::FG_2);
        ui.horizontal_wrapped(|ui| {
            if button(ui, "Open project", icons::PLAY, true, !project.missing && !state.action.running()) {
                let p = project.clone();
                state.task(&ctx, "Opening Unity…", move || {
                    orbiters_projects::open_project(&p)?;
                    Ok(Outcome::Changed("Project sent to Unity".into()))
                });
            }
            if button(ui, "Open folder", icons::FOLDER_OPEN, false, !project.missing && !state.action.running()) {
                let path = project.path.clone();
                state.task(&ctx, "Opening folder…", move || {
                    orbiters_projects::hidden_command("explorer.exe").arg(path).spawn()?;
                    Ok(Outcome::Picked(None))
                });
            }
            if button(ui, "Unity Hub", icons::CUBE, false, true) {
                state.task(&ctx, "Opening Unity Hub…", move || {
                    orbiters_projects::open_hub()?;
                    Ok(Outcome::Picked(None))
                });
            }
        });
        error(ui, state);
        progress(ui, state);
        if !state.packages_only {
            title(ui, "Avatars in this project");
            if project.avatars.is_empty() {
                ui.label(egui::RichText::new("No uploaded avatar IDs were found in this project's text scenes.").color(theme::FG_3));
            }
            for a in &project.avatars {
                ui.push_id((&a.id, &a.scene, &a.path), |ui| {
                    egui::Frame::NONE.fill(theme::surface_soft()).corner_radius(14.).inner_margin(14).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(76., 76.), Sense::hover());
                            avatar_image(ui, state, a, r);
                            ui.vertical(|ui| {
                                text(ui, &a.name, 16., theme::FG);
                                text(ui, &a.scene, 12., theme::FG_3);
                            });
                        });
                        ui.horizontal_wrapped(|ui| {
                            if button(ui, "Add N4N0", icons::N4N0, true, project.registered && !state.action.running()) {
                                state.install(&ctx, project.clone(), a.clone(), true, server(cx));
                            }
                            if button(ui, "Add My Avatar", icons::PERSON, false, project.registered && !state.action.running()) {
                                state.install(&ctx, project.clone(), a.clone(), false, server(cx));
                            }
                        });
                    });
                });
            }
            if !state.commits.is_empty() {
                title(ui, "Recent commits");
                for commit in &state.commits {
                    let parts: Vec<_> = commit.split('\x1f').collect();
                    if parts.len() == 4 {
                        text(ui, &format!("{}  {}", parts[0], parts[1]), 13., theme::FG_2);
                        text(ui, &format!("{} · {}", parts[2], parts[3]), 11., theme::FG_3);
                    }
                }
            }
        }
        packages(ui, cx, state, &project);
        ui.add_space(12.);
        if project.registered && button(ui, "Remove from project list", icons::TRASH, false, !state.action.running()) {
            let path = project.path.clone();
            state.selected = None;
            state.task(&ctx, "Removing from list…", move || {
                orbiters_projects::runtime()?.block_on(vpm::register(&orbiters_projects::vcc_dir(), &path, false))?;
                Ok(Outcome::Changed("Removed from the shared project list. Files were kept.".into()))
            });
        }
    });
}
fn packages(ui: &mut Ui, _cx: &mut Cx, state: &mut State, project: &Project) {
    title(ui, "Packages");
    let ctx = ui.ctx().clone();
    if state.view_job.running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Reading packages…");
        });
    }
    ui.horizontal_wrapped(|ui| {
        if button(ui, "Resolve missing packages", icons::ARROW_COUNTER_CLOCKWISE, false, !state.action.running() && !project.missing) {
            package_action(state, &ctx, project, vpm::Change::Resolve, "Restoring packages…");
        }
        ui.checkbox(&mut state.prerelease, "Show pre-release");
    });
    let installed: Vec<_> = state.view.packages.iter().filter(|p| p.installed.is_some()).cloned().collect();
    for p in installed {
        ui.push_id(&p.name, |ui| {
            egui::Frame::NONE.fill(theme::surface_soft()).corner_radius(12.).inner_margin(12).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.;
                ui.horizontal_wrapped(|ui| {
                    let names_width = (ui.available_width() - 284.).max(130.);
                    ui.allocate_ui_with_layout(vec2(names_width, 44.), egui::Layout::top_down(egui::Align::Min), |ui| {
                        text(ui, &p.display_name, 14., theme::FG);
                        text(ui, &p.name, 11., theme::FG_3);
                    });
                    ui.horizontal_wrapped(|ui| {
                        let installed = p.installed.as_deref().unwrap();
                        let mut selected = installed.to_string();
                        egui::ComboBox::from_id_salt("version").selected_text(installed).width(150.).show_ui(ui, |ui| {
                            for version in &p.versions {
                                if state.prerelease || !version.contains('-') || version == installed {
                                    ui.selectable_value(&mut selected, version.clone(), version);
                                }
                            }
                        });
                        if selected != installed {
                            package_action(
                                state,
                                &ctx,
                                project,
                                vpm::Change::Install { names: vec![(p.name.clone(), Some(selected))], prerelease: state.prerelease },
                                "Changing package version…",
                            );
                        }
                        if button(ui, "Remove", icons::MINUS, false, !state.action.running()) {
                            package_action(state, &ctx, project, vpm::Change::Remove(p.name.clone()), "Removing package…");
                        }
                    });
                });
            });
        });
    }
    ui.add_space(10.);
    title(ui, "Available packages");
    widgets::search_field(ui, "package-search", &mut state.package_query, "Search packages", ui.available_width());
    let q = state.package_query.to_lowercase();
    let available: Vec<_> = state
        .view
        .packages
        .iter()
        .filter(|p| p.installed.is_none() && (q.is_empty() || p.name.to_lowercase().contains(&q) || p.display_name.to_lowercase().contains(&q)))
        .cloned()
        .collect();
    for p in available {
        let versions: Vec<_> = p.versions.iter().filter(|v| state.prerelease || !v.contains('-')).collect();
        if versions.is_empty() {
            continue;
        }
        ui.push_id(&p.name, |ui| {
            egui::Frame::NONE.fill(theme::surface_soft()).corner_radius(12.).inner_margin(12).show(ui, |ui| {
                text(ui, &p.display_name, 14., theme::FG);
                text(ui, &p.name, 11., theme::FG_3);
                ui.horizontal_wrapped(|ui| {
                    let id = ui.scope_id().with("available-version");
                    let mut chosen = ui.ctx().data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| versions[0].clone());
                    egui::ComboBox::from_id_salt("available-version").selected_text(&chosen).width(150.).show_ui(ui, |ui| {
                        for v in &versions {
                            ui.selectable_value(&mut chosen, (*v).clone(), *v);
                        }
                    });
                    ui.ctx().data_mut(|d| d.insert_temp(id, chosen.clone()));
                    if button(ui, "Add", icons::PLUS, false, !state.action.running()) {
                        package_action(
                            state,
                            &ctx,
                            project,
                            vpm::Change::Install { names: vec![(p.name.clone(), Some(chosen))], prerelease: state.prerelease },
                            "Installing package…",
                        );
                    }
                });
            });
        });
    }
}
fn package_action(state: &mut State, ctx: &egui::Context, p: &Project, change: vpm::Change, label: &str) {
    let path = p.path.clone();
    state.task(ctx, label, move || {
        orbiters_projects::runtime()?.block_on(vpm::change(&orbiters_projects::vcc_dir(), &path, change))?;
        Ok(Outcome::Changed("Packages updated".into()))
    });
}

pub fn capture_banner(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    state.poll(&ctx, cx);
    ui.horizontal_wrapped(|ui| {
        if button(ui, "Add to my avatar", icons::N4N0, false, !state.action.running()) {
            state.ensure(&ctx);
            state.preselected = cx.snap.orb.avatar_id.clone();
            state.modal = Some(Modal::Avatars);
        }
        if cx.snap.orb.manifest && cx.snap.orb.prop_sync.is_some() {
            ui.colored_label(theme::ACCENT, "N4N0 is on your avatar, you're ready to capture");
        } else {
            ui.label(egui::RichText::new("Put N4N0 on your avatar to capture in VRChat.").color(theme::FG_3));
        }
        if state.action.running() {
            ui.spinner();
            ui.label(&state.action_label);
        } else if state.flow.is_some() && button(ui, "View setup", icons::ARROW_SQUARE_OUT, false, true) {
            state.modal = Some(Modal::Setup);
        }
    });
}

pub fn modals(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let Some(modal) = state.modal.clone() else { return };
    let ctx = ui.ctx().clone();
    let width = (ctx.content_rect().width() - 72.).clamp(300., 760.);
    let max_h = (ctx.content_rect().height() - 100.).max(200.);
    let modal_id = Id::unique("unity-project-modal");
    let response = egui::Modal::new(modal_id)
        .area(egui::Modal::default_area(modal_id).default_size(vec2(width + 48., max_h)))
        .frame(egui::Frame::NONE.fill(theme::INK_850).corner_radius(24.).inner_margin(24))
        .show(&ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(10., 14.);
            ui.visuals_mut().selection.bg_fill = theme::INK_700;
            ui.visuals_mut().selection.stroke.color = theme::FG;
            let heading = match &modal {
                Modal::Repos => "Package repositories",
                Modal::New => "Create a Unity project",
                Modal::Avatars => "Add N4N0 to my avatar",
                Modal::ChooseProject(_) => "Which project should we use?",
                Modal::Setup => {
                    if state.flow.as_ref().is_some_and(|f| f.packages_only) {
                        "Add My Avatar"
                    } else {
                        "Add N4N0 to your avatar"
                    }
                }
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(heading).size(24.).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::activated(&widgets::IconButton::new(icons::X, "Close dialog").show(ui)) {
                        state.modal = None;
                    }
                });
            });
            egui::ScrollArea::vertical().id_salt("project-modal-scroll").max_height(max_h - 60.).show(ui, |ui| {
                error(ui, state);
                progress(ui, state);
                match modal {
                    Modal::Repos => repos(ui, cx, state),
                    Modal::New => new_project(ui, state),
                    Modal::Avatars => avatar_picker(ui, cx, state),
                    Modal::ChooseProject(id) => project_choice(ui, cx, state, &id),
                    Modal::Setup => setup_flow(ui, cx, state),
                }
            });
        });
    if response.should_close() {
        state.modal = None;
    }
}
fn repos(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    ui.label(egui::RichText::new("Shared with VCC and ALCOM. Disabled repositories stay saved but their packages are excluded here.").color(theme::FG_3));
    if button(ui, "Refresh repositories", icons::ARROW_COUNTER_CLOCKWISE, false, !state.action.running() && !state.view_job.running()) {
        state.load_view(&ctx, true);
        state.catalog(&ctx, server(cx));
    }
    for r in state.view.repos.clone() {
        ui.push_id(&r.id, |ui| {
            egui::Frame::NONE.fill(theme::surface_soft()).corner_radius(12.).inner_margin(12).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut enabled = r.enabled;
                    if ui.add_enabled(!state.action.running(), egui::Checkbox::new(&mut enabled, &r.name)).changed() {
                        if let Some(repo) = state.view.repos.iter_mut().find(|repo| repo.id == r.id) {
                            repo.enabled = enabled;
                        }
                        let id = r.id.clone();
                        state.task(&ctx, "Updating repository…", move || {
                            orbiters_projects::runtime()?.block_on(vpm::set_enabled(&orbiters_projects::vcc_dir(), &id, enabled))?;
                            Ok(Outcome::Changed("Repository updated".into()))
                        });
                    }
                    if !r.builtin && button(ui, "Remove", icons::MINUS, false, !state.action.running()) {
                        let id = r.id.clone();
                        state.task(&ctx, "Removing repository…", move || {
                            orbiters_projects::runtime()?.block_on(vpm::remove_repo(&orbiters_projects::vcc_dir(), &id))?;
                            Ok(Outcome::Changed("Repository removed".into()))
                        });
                    }
                });
                text(ui, &r.url, 12., theme::FG_3);
            });
        });
    }
    title(ui, "Add a repository");
    widgets::text_field(ui, "repo-url", &mut state.repo_url, "https://example.com/index.json", ui.available_width(), false);
    ui.collapsing("Optional request headers", |ui| {
        ui.label("One header per line, for example Authorization: Bearer …");
        ui.add(egui::TextEdit::multiline(&mut state.repo_headers).desired_width(f32::INFINITY).desired_rows(2));
    });
    if button(ui, "Add repository", icons::PLUS, true, !state.repo_url.trim().is_empty() && !state.action.running()) {
        let url = state.repo_url.trim().to_string();
        let mut headers = BTreeMap::new();
        let mut valid = true;
        for line in state.repo_headers.lines().filter(|l| !l.trim().is_empty()) {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_string(), v.trim().to_string());
            } else {
                state.error = Some("Headers need a name and value separated by a colon.".into());
                valid = false;
            }
        }
        if valid {
            add_repo(state, &ctx, url, headers);
            state.repo_headers.clear();
        }
    }
    for (heading, list) in [("Known VPM repositories", state.catalog.known.clone()), ("Repositories on Orbiters", state.catalog.public.clone())] {
        title(ui, heading);
        if list.is_empty() {
            ui.label(egui::RichText::new(if state.catalog_job.running() { "Loading…" } else { "No repositories to show." }).color(theme::FG_3));
        }
        for r in list {
            ui.push_id((&r.url, heading), |ui| {
                egui::Frame::NONE.fill(theme::surface_soft()).corner_radius(12.).inner_margin(12).show(ui, |ui| {
                    text(ui, &r.name, 14., theme::FG);
                    ui.label(egui::RichText::new(&r.description).size(12.).color(theme::FG_3));
                    let added = state.view.repos.iter().any(|repo| repo.url.trim_end_matches('/') == r.url.trim_end_matches('/'));
                    if button(ui, if added { "Added" } else { "Add" }, icons::PLUS, false, !added && !state.action.running()) {
                        add_repo(state, &ctx, r.url.clone(), BTreeMap::new());
                    }
                });
            });
        }
    }
    if !state.catalog.errors.is_empty() {
        ui.colored_label(theme::WARN, "Orbiters' suggestions could not be loaded.");
        if button(ui, "Retry suggestions", icons::ARROW_COUNTER_CLOCKWISE, false, !state.catalog_job.running()) {
            state.catalog(&ctx, server(cx));
        }
    }
}
fn add_repo(state: &mut State, ctx: &egui::Context, url: String, headers: BTreeMap<String, String>) {
    state.task(ctx, "Adding repository…", move || {
        orbiters_projects::runtime()?.block_on(vpm::add_repo(&orbiters_projects::vcc_dir(), &url, headers))?;
        Ok(Outcome::Changed("Repository added".into()))
    });
}
fn new_project(ui: &mut Ui, state: &mut State) {
    let ctx = ui.ctx().clone();
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut state.duplicate, false, "From a template");
        ui.selectable_value(&mut state.duplicate, true, "Duplicate an existing project");
    });
    let projects: Vec<_> = state.scan.projects.iter().filter(|p| p.registered && !p.missing).cloned().collect();
    if state.duplicate {
        egui::ComboBox::from_id_salt("duplicate-source")
            .selected_text(projects.get(state.duplicate_index).map(|p| p.name.as_str()).unwrap_or("Choose a project"))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (i, p) in projects.iter().enumerate() {
                    ui.selectable_value(&mut state.duplicate_index, i, &p.name);
                }
            });
        ui.label(egui::RichText::new("Copies Assets, Packages and settings. Generated files and Git history stay behind.").color(theme::FG_3));
    } else {
        title(ui, "Project template");
        if state.templates.is_empty() {
            ui.label("No installed VCC templates were found in Templates or VRCTemplates.");
        }
        for (i, t) in state.templates.iter().enumerate() {
            ui.selectable_value(
                &mut state.template_index,
                i,
                format!("{}{}", t.name, if t.unity.is_empty() { String::new() } else { format!(" · Unity {}", t.unity) }),
            );
        }
    }
    title(ui, "Project name");
    widgets::text_field(ui, "new-name", &mut state.new_name, "My avatar", ui.available_width(), false);
    title(ui, "Location");
    widgets::text_field(ui, "new-location", &mut state.new_location, "Choose a folder", ui.available_width(), false);
    if button(ui, "Choose location", icons::FOLDER_OPEN, false, !state.action.running()) {
        state.task(&ctx, "Choose a location…", move || Ok(Outcome::Picked(rfd::FileDialog::new().pick_folder())));
    }
    let template = state.templates.get(state.template_index).cloned();
    if !state.duplicate && template.as_ref().is_some_and(|t| t.unity.is_empty()) {
        title(ui, "Unity version");
        widgets::text_field(ui, "new-unity", &mut state.new_unity, "Exact installed editor version", ui.available_width(), false);
    }
    if let Err(e) = templates::validate_name(&state.new_name) {
        if !state.new_name.is_empty() {
            ui.colored_label(theme::WARN, e.to_string());
        }
    }
    let source = if state.duplicate {
        projects.get(state.duplicate_index).map(|p| (p.path.clone(), p.unity.clone()))
    } else {
        template.map(|t| (t.path, if t.unity.is_empty() { state.new_unity.clone() } else { t.unity }))
    };
    let enabled = source.is_some() && !state.new_name.is_empty() && !state.new_location.is_empty() && !state.action.running();
    if button(ui, if state.duplicate { "Duplicate project" } else { "Create project" }, icons::PLUS, true, enabled) {
        let (source, unity) = source.unwrap();
        let name = state.new_name.clone();
        let location = PathBuf::from(&state.new_location);
        let duplicate = state.duplicate;
        state.copy_cancel = Arc::new(AtomicBool::new(false));
        *state.copy_progress.lock().unwrap() = Default::default();
        let cancel = state.copy_cancel.clone();
        let progress = state.copy_progress.clone();
        state.task(&ctx, "Creating project…", move || {
            let path = orbiters_projects::runtime()?.block_on(templates::create(
                &orbiters_projects::vcc_dir(),
                &source,
                &location,
                &name,
                &unity,
                duplicate,
                &cancel,
                &progress,
            ))?;
            Ok(Outcome::Changed(format!("Project created at {}", path.display())))
        });
    }
}
fn avatar_picker(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    ui.label(egui::RichText::new("Choose the avatar you want to capture with. N4N0 will ask for confirmation inside Unity.").color(theme::FG_3));
    if state.scan_rx.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Finding avatars in your registered projects…");
        });
    }
    let mut avatars: BTreeMap<String, (Project, Avatar, usize)> = BTreeMap::new();
    for p in state.scan.projects.iter().filter(|p| p.registered) {
        for a in &p.avatars {
            avatars.entry(a.id.clone()).and_modify(|v| v.2 += 1).or_insert((p.clone(), a.clone(), 1));
        }
    }
    let mut list: Vec<_> = avatars.into_values().collect();
    list.sort_by_key(|(_, a, _)| (Some(&a.id) != state.preselected.as_ref(), a.name.clone()));
    if list.is_empty() {
        empty(ui, "No uploaded avatars found yet", "Add an avatar project on the Unity projects page. Save its scene in Unity after the first upload.");
    }
    let cols = (ui.available_width() / 200.).floor().max(1.) as usize;
    let width = (ui.available_width() - (cols - 1) as f32 * 12.) / cols as f32;
    for row in list.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.;
            for (p, a, n) in row {
                ui.push_id(&a.id, |ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(width, 216.), Sense::hover());
                    let response = ui.interact(r, ui.scope_id().with("avatar"), Sense::click());
                    widgets::frame(ui.painter(), r, 16., if Some(&a.id) == state.preselected.as_ref() { theme::INK_700 } else { theme::surface_soft() });
                    avatar_image(ui, state, a, Rect::from_min_size(r.min + vec2(6., 6.), vec2(width - 12., 134.)));
                    widgets::text_truncated(ui.painter(), pos2(r.left() + 12., r.top() + 158.), Align2::LEFT_CENTER, &Txt::new(&a.name, 15.), width - 24.);
                    widgets::text_truncated(
                        ui.painter(),
                        pos2(r.left() + 12., r.top() + 180.),
                        Align2::LEFT_CENTER,
                        &Txt::new(if *n > 1 { format!("{n} project locations") } else { p.name.clone() }, 12.).color(theme::FG_3),
                        width - 24.,
                    );
                    if Some(&a.id) == cx.snap.orb.avatar_id.as_ref() {
                        widgets::text(
                            ui.painter(),
                            pos2(r.left() + 12., r.top() + 201.),
                            Align2::LEFT_CENTER,
                            &Txt::new("Wearing in VRChat", 11.).color(theme::ACCENT),
                        );
                    }
                    if widgets::activated(&response) {
                        state.choose_avatar(ui.ctx(), a.id.clone(), server(cx));
                    }
                });
            }
        });
        ui.add_space(8.);
    }
}
fn project_choice(ui: &mut Ui, cx: &mut Cx, state: &mut State, id: &str) {
    ui.label(egui::RichText::new("This avatar is saved in more than one project or scene. Choose the copy to prepare.").color(theme::FG_3));
    let choices: Vec<_> =
        state.scan.projects.iter().filter(|p| p.registered).flat_map(|p| p.avatars.iter().filter(|a| a.id == id).map(|a| (p.clone(), a.clone()))).collect();
    for (p, a) in choices {
        ui.push_id((&p.path, &a.scene, &a.path), |ui| {
            egui::Frame::NONE.fill(theme::surface_soft()).corner_radius(14.).inner_margin(16).show(ui, |ui| {
                title(ui, &p.name);
                ui.label(egui::RichText::new(p.path.display().to_string()).color(theme::FG_3));
                text(ui, &format!("{} · {}", a.scene, crate::pages::local_time_unix(p.modified)), 12., theme::FG_3);
                if button(ui, "Use this project", icons::ARROW_SQUARE_OUT, true, !state.action.running()) {
                    state.install(ui.ctx(), p.clone(), a.clone(), true, server(cx));
                }
            });
        });
    }
    if button(ui, "Back to avatars", icons::ARROW_LEFT, false, !state.action.running()) {
        state.modal = Some(Modal::Avatars);
    }
}
fn setup_flow(ui: &mut Ui, _cx: &mut Cx, state: &mut State) {
    let Some(flow) = state.flow.as_ref() else {
        if !state.action.running() {
            ui.label("Choose an avatar to start setup.");
        }
        return;
    };
    let project = flow.project.clone();
    let name = flow.avatar.name.clone();
    let packages_only = flow.packages_only;
    let unity_open = flow.unity_open;
    let result = flow.result.clone();
    text(ui, &format!("{} · {}", name, project.name), 16., theme::FG);
    let (glyph, heading, message) = if let Some(result) = result {
        match result.status {
            setup::Status::Done => (
                icons::CHECK_CIRCLE,
                "N4N0 is ready in Unity",
                format!("{}\nUpload the avatar from the VRChat SDK panel that just opened, then come back here.", result.message),
            ),
            setup::Status::Declined => (icons::CLOCK, "Setup was declined", result.message),
            setup::Status::Failed => (icons::WARNING, "Setup needs attention", result.message),
        }
    } else if packages_only {
        (icons::CHECK_CIRCLE, "My Avatar is installed", "Open the project to use My Avatar in Unity.".into())
    } else if unity_open {
        (icons::CLOCK, "Confirm in Unity", "Unity is open on this project: switch to it to confirm".into())
    } else {
        (icons::CUBE, "Open Unity to continue", "My Avatar and Toolkit are installed. Open this project and confirm adding N4N0 to your avatar.".into())
    };
    ui.horizontal(|ui| {
        widgets::icon(ui.painter(), ui.cursor().min + vec2(20., 20.), glyph, 30., theme::FG_2);
        ui.add_space(52.);
        ui.vertical(|ui| {
            title(ui, heading);
            ui.label(egui::RichText::new(message).color(theme::FG_2));
        });
    });
    if button(ui, "Open project", icons::PLAY, true, !state.action.running()) {
        state.task(ui.ctx(), "Opening Unity…", move || {
            orbiters_projects::open_project(&project)?;
            Ok(Outcome::Picked(None))
        });
    }
    if button(ui, "Open Unity Hub", icons::CUBE, false, true) {
        state.task(ui.ctx(), "Opening Unity Hub…", move || {
            orbiters_projects::open_hub()?;
            Ok(Outcome::Picked(None))
        });
    }
    state.acknowledge_result(ui.ctx());
}
