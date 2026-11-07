//! Capture page: three big actions (a 3D picture of people, one of the world, a free recording) and, under them, what VRChat and
//! the sources are doing, the recording's settings, live statistics and the camera path.

use super::{block, entering, Cx};
use crate::state::CaptureState;
use egui::{pos2, vec2, Align, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder, Vec2};
use orbiters_contracts::manifest::{CaptureMode, StorageQuality};
use orbiters_core::picture::Left;
use orbiters_core::settings::{Photo3dLength, PictureMode, Settings};
use orbiters_i18n::{fill, mark, tr, tr_ctx, tr_n};
use orbiters_n4n0_core::menu::strings::{Strings, FLIGHTS};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::format;
use orbiters_ui::record::{self, RecState, RecordButton};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Tone, Variant};
use orbiters_ui::{icons, trail};

#[derive(Default)]
pub struct State {
    /// The avatar worn, and whether its upload can hide itself, as its manifest says (read once per avatar).
    upload: Option<(String, Option<bool>)>,
    /// Update the Toolkit was pressed: what went wrong is told here, once it ended.
    toolkit_asked: bool,
}

const GRID_H: f32 = 212.0;

/// What each of a 3D picture's camera flights does (1 s, 4 s, 8 s), here and on Home's 3D picture page.
pub(super) const FLIGHT_HINTS: [&str; 3] = [
    mark("A gentle sweep around what the camera aims at"),
    mark("Looks around, then goes around the people or through the place, as far as it can see it goes"),
    mark("As 4 s, with more views of each person, or more of the place"),
];

/// The three flights' names (1 s, 4 s, 8 s) in the language now, as the menu on the avatar and in VR writes them (`menu-strings`).
pub(super) fn flight_labels() -> [&'static str; 3] {
    let strings = Strings::builtin(orbiters_i18n::language());
    [strings.get(FLIGHTS[0]), strings.get(FLIGHTS[1]), strings.get(FLIGHTS[2])]
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State, projects: &mut super::projects::State) {
    let from = orbiters_ui::blur::next_shape(ui);
    egui::ScrollArea::vertical().id_salt("capture").auto_shrink([false, false]).scroll_bar_rect(super::below_bar(ui, cx)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8 + cx.top as i8, bottom: 32 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            header(ui, cx);
            actions(ui, cx, state, projects);
            chips(ui, cx, projects);
            // A recording's numbers and what it records: chosen and read where recording happens, the test pattern's
            // aside.
            let here = orbiters_core::osc::VRCHAT_RUNS_HERE;
            if here || cx.snap.is_capturing() {
                grid(ui, cx);
                camera_path(ui, cx);
            }
            if here {
                performance(ui, cx);
            }
        });
    });
    // Down to where the content starts (its top margin).
    super::under_top(ui, cx, "capture", from, ui.max_rect().top() + cx.top + 8.0);
    super::projects::modals(ui, cx, projects);
}

/// The height of the three big actions, and of the card that takes their place while something is under way.
const ACTION_H: f32 = 176.0;

fn rec_state(s: CaptureState) -> RecState {
    match s {
        CaptureState::Idle => RecState::Idle,
        CaptureState::Starting => RecState::Starting,
        CaptureState::Recording => RecState::Recording,
        CaptureState::Paused => RecState::Paused,
        CaptureState::Stopping => RecState::Stopping,
        CaptureState::Importing => RecState::Importing,
    }
}

fn header(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let c = &snap.capture;
    let world = &snap.vrchat.world;
    let recording = matches!(c.state, CaptureState::Recording | CaptureState::Paused);
    block(ui, cx, 0, 72.0, |ui, rect| {
        let caption = if recording {
            tr("Live capture")
        } else if world.in_world {
            tr("VRChat world")
        } else {
            tr("Capture")
        };
        let headline = match c.state {
            CaptureState::Importing => tr("Importing a raw capture").to_string(),
            CaptureState::Paused => tr("Paused").into(),
            CaptureState::Recording => tr("Recording").into(),
            _ => world.world_name.clone().filter(|_| world.in_world).unwrap_or_else(|| tr("Ready to capture").into()),
        };
        let p = ui.painter();
        widgets::text(p, pos2(rect.left(), rect.top() + 7.0), Align2::LEFT_CENTER, &Txt::caps(caption));
        widgets::sliding_value(
            ui,
            Id::unique("capture-headline"),
            pos2(rect.left(), rect.top() + 34.0),
            Align2::LEFT_CENTER,
            &headline,
            &Txt::new("", 30.0).weight(weight::SEMIBOLD).tracking(-0.015),
            rect.width() * 0.7,
        );
        let p = ui.painter();
        let sub_y = rect.top() + 62.0;
        match &world.world_id {
            Some(id) => {
                widgets::text_truncated(p, pos2(rect.left(), sub_y), Align2::LEFT_CENTER, &Txt::new(id, 12.0).mono().color(theme::FG_3), rect.width() * 0.7);
            }
            None => {
                let hint = if !orbiters_core::osc::VRCHAT_RUNS_HERE {
                    tr("VRChat's camera comes over Spout, on Windows. Captures made there open here, in Photos & captures.")
                } else if snap.vrchat.running {
                    tr("VRChat is running; join a world to tag captures with it.")
                } else {
                    tr("Start VRChat and enable Spout streaming on the Stream camera.")
                };
                widgets::text_truncated(p, pos2(rect.left(), sub_y), Align2::LEFT_CENTER, &Txt::new(hint, 13.0).color(theme::FG_3), rect.width() * 0.7);
            }
        }
    });
}

/// What the page is for: a 3D picture of people, a 3D picture of the world, or a free recording. While one of them is under way,
/// what it is doing and how to stop it take their place. Before the capture notice is accepted, the notice does.
fn actions(ui: &mut Ui, cx: &mut Cx, state: &mut State, projects: &mut super::projects::State) {
    let snap = cx.snap;
    if !snap.settings.consent_acknowledged {
        return consent(ui, cx);
    }
    if snap.is_capturing() || snap.capture.state == CaptureState::Stopping {
        return under_way(ui, cx);
    }
    if !orbiters_core::osc::VRCHAT_RUNS_HERE {
        return recording_elsewhere(ui, cx);
    }
    // The worn avatar's upload, read once per avatar: a picture of the world says beforehand that it would show.
    let worn = snap.n4n0.avatar_id.clone();
    if state.upload.as_ref().map(|(id, _)| id) != worn.as_ref() {
        state.upload = worn.map(|id| {
            let can = crate::controller::upload_can_hide(&id);
            (id, can)
        });
    }
    let cannot_hide = state.upload.as_ref().is_some_and(|(_, can)| *can == Some(false));
    let vrchat = snap.vrchat.running;
    let w = ui.available_width();
    let gap = 16.0;
    let card_w = (w - 2.0 * gap) / 3.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, ACTION_H), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut row = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-actions"));
    row.multiply_opacity(alpha);
    let cell = |i: usize| Rect::from_min_size(rect.min + vec2(i as f32 * (card_w + gap), 0.0), vec2(card_w, ACTION_H));
    let not_running = tr("Start VRChat first: its camera takes the picture");
    let world_note = if cannot_hide { tr("Goes through the place; your avatar is taken out afterwards") } else { tr("Goes through the place, avatars left out") };
    // (A text written as `field: tr("…")` would not be read as a key by tests/coverage.rs's scan: a `let` is.)
    let people_title = tr("3D picture of people");
    let people_note = tr("Goes around the people here");
    let place_title = tr("3D picture of the world");
    let record_title = tr("Record");
    let record_note = tr("You move the camera. Press again to stop");
    let people = BigAction {
        glyph: icons::USERS,
        tint: theme::VIOLET,
        title: people_title,
        note: people_note,
        warn: false,
        enabled: vrchat,
        why_not: not_running,
    };
    let place = BigAction {
        glyph: icons::GLOBE_HEMISPHERE_WEST,
        tint: theme::ACCENT,
        title: place_title,
        note: world_note,
        warn: false,
        enabled: vrchat,
        why_not: not_running,
    };
    let record = BigAction {
        glyph: icons::RECORD,
        tint: theme::REC,
        title: record_title,
        note: record_note,
        warn: false,
        enabled: true,
        why_not: "",
    };
    let chosen = [
        people.show(&mut row, cell(0), "capture-people").then_some(PictureMode::People),
        place.show(&mut row, cell(1), "capture-world").then_some(PictureMode::World),
    ];
    if let Some(mode) = chosen.into_iter().flatten().next() {
        cx.ctl.n4n0(crate::ipc::N4n0Command::Photo3DOf { mode });
    }
    if record.show(&mut row, cell(2), record::LABEL) {
        cx.ctl.start_capture();
    }
    why_last(ui, cx, state, projects);
    quiet_row(ui, cx);
}

/// In place of the three big actions where VRChat does not run: they all fly or read its camera. The test pattern stays, and
/// tries the whole path from the capture worker to a reconstruction.
fn recording_elsewhere(ui: &mut Ui, cx: &mut Cx) {
    let mut test = false;
    super::section(ui, cx, 1, icons::CUBE_FOCUS, tr("3D pictures and recordings"), Some(tr("Taken on the computer that runs VRChat")), |ui| {
        let note = tr("Without a VRChat here, the test pattern records a generated picture through the capture worker, to try the whole path. It shows in Free up space, not among your pictures.");
        widgets::field(ui, tr("Test pattern"), Some(note), 150.0, |ui| {
            test = Button::new(tr("Test pattern")).icon(icons::FLASK).size(Size::Sm).show(ui).clicked();
        });
    });
    if test {
        cx.ctl.start_test_capture();
    }
}

/// A big, clear button: a filled card in its colour, its icon, what it does in a few words.
struct BigAction<'a> {
    glyph: &'a str,
    tint: egui::Color32,
    title: &'a str,
    note: &'a str,
    warn: bool,
    enabled: bool,
    why_not: &'a str,
}

impl BigAction<'_> {
    fn show(&self, ui: &mut Ui, rect: Rect, label: &str) -> bool {
        let ctx = ui.ctx().clone();
        let id = Id::unique(("capture-action", label));
        let resp = ui.interact(rect, id, if self.enabled { Sense::click() } else { Sense::hover() });
        widgets::note_control(&ctx, label, rect, self.enabled);
        let hover = anim::toggle(&ctx, id.with("hover"), self.enabled && resp.hovered(), Spring::SOFT);
        let press = anim::toggle(&ctx, id.with("press"), self.enabled && resp.is_pointer_button_down_on(), Spring::SNAPPY);
        let clicked = self.enabled && widgets::pressed(&resp);
        if self.enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        } else if !self.why_not.is_empty() {
            resp.on_hover_text(self.why_not);
        }
        let r = Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.02 * press));
        let mut p = ui.painter().clone();
        if !self.enabled {
            p.multiply_opacity(0.5);
        }
        widgets::frame(&p, r, theme::CARD_RADIUS, theme::mix(theme::surface(), self.tint, 0.10 + 0.08 * hover));
        let chip = Rect::from_min_size(r.min + vec2(22.0, 22.0), vec2(56.0, 56.0));
        p.rect_filled(chip, theme::radius(17.0), theme::alpha(self.tint, 0.18));
        widgets::icon(&p, chip.center(), self.glyph, 28.0, self.tint);
        widgets::text_truncated(
            &p,
            pos2(r.left() + 22.0, r.bottom() - 62.0),
            Align2::LEFT_CENTER,
            &Txt::new(self.title, 20.0).weight(weight::SEMIBOLD),
            r.width() - 44.0,
        );
        let note = if self.enabled { self.note } else { self.why_not };
        let (color, line) = if self.warn && self.enabled { (theme::WARN, 18.0) } else { (theme::FG_2, 18.0) };
        let galley = p.layout_job(Txt::new(note, 13.5).color(color).line_height(line).job(r.width() - 44.0));
        p.galley(pos2(r.left() + 22.0, r.bottom() - 46.0), galley, color);
        clicked
    }
}

/// What the last capture could not do, until the next one starts: the toast that said so is gone, and the person may have been
/// in a headset.
fn why_last(ui: &mut Ui, cx: &mut Cx, state: &mut State, projects: &mut super::projects::State) {
    let ctx = ui.ctx().clone();
    if state.toolkit_asked && !projects.toolkit_running() {
        state.toolkit_asked = false;
        if let Some(e) = projects.take_error() {
            cx.toasts.push(&ctx, orbiters_ui::toast::Kind::Error, e);
        }
    }
    let Some(why) = cx.snap.capture.left_out.why.clone() else { return };
    // The worn avatar could not hide: Update the Toolkit, where its project is on this computer.
    let wearer = cx.snap.capture.left_out.wearer == Left::InPicture;
    let worn = cx.snap.n4n0.avatar_id.clone().filter(|_| wearer);
    if worn.is_some() {
        projects.toolkit_look(&ctx);
    }
    let offer = worn.as_deref().map(|id| projects.toolkit_offer(id));
    let name = worn.as_deref().and_then(crate::controller::uploaded_avatar_name);
    let tone = if wearer { Tone::Info.color() } else { theme::WARN };
    let (button, note) = match &offer {
        Some(super::projects::ToolkitOffer::Project(name)) => (Some(tr!("Update the Toolkit in {name}", name = name)), None),
        Some(super::projects::ToolkitOffer::NotHere) => (None, Some(super::projects::toolkit_refusal(name.as_deref()))),
        _ => (None, None),
    };
    let text = note.map_or(why.clone(), |n| format!("{why} {n}"));
    let w = ui.available_width();
    let ink = if wearer { theme::FG } else { tone };
    let galley = ui.painter().layout_job(Txt::new(text, 13.5).color(ink).line_height(19.0).job(w - 68.0));
    let button_h = if button.is_some() { 44.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 24.0 + button_h), Sense::hover());
    let p = ui.painter();
    widgets::frame(p, rect, 14.0, theme::alpha(tone, 0.10));
    widgets::icon(p, pos2(rect.left() + 26.0, rect.top() + 12.0 + 9.5), if wearer { icons::INFO } else { icons::WARNING }, 18.0, tone);
    let text_bottom = rect.top() + 12.0 + galley.size().y;
    p.galley(pos2(rect.left() + 48.0, rect.top() + 12.0), galley, ink);
    if let (Some(label), Some(id)) = (button, worn) {
        let running = projects.toolkit_running();
        let b = Button::new(&label)
            .icon(icons::ARROW_UP)
            .variant(Variant::Secondary)
            .size(Size::Sm)
            .enabled(!running)
            .id_salt("update-toolkit")
            .hint(tr("Updates Orbiters Toolkit in that project and opens it in Unity. You upload from VRChat's SDK panel: N4N0 uploads nothing by itself."));
        let bw = b.desired_width(ui);
        let at = Rect::from_min_size(pos2(rect.left() + 48.0, text_bottom + 8.0), vec2(bw, 32.0));
        if widgets::activated(&b.show_at(ui, at)) {
            projects.update_toolkit(&ctx, &id, name.as_deref());
            state.toolkit_asked = true;
        }
    }
}

/// What rarely changes: how long a picture flies, the longer scan and the test pattern. In reach and out of the way.
fn quiet_row(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let mut scan = false;
    let mut test = false;
    block(ui, cx, 2, 36.0, |ui, rect| {
        let p = ui.painter();
        let label = widgets::text(p, pos2(rect.left() + 2.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Picture length"), 13.0).color(theme::FG_3));
        let mut length = snap.settings.photo_3d;
        let lengths = [Photo3dLength::Quick, Photo3dLength::Standard, Photo3dLength::Full]
            .into_iter()
            .zip(flight_labels().into_iter().zip(FLIGHT_HINTS))
            .map(|(length, (label, hint))| SegOption::new(length, label).hint(tr(hint)))
            .collect::<Vec<_>>();
        let seg_rect = Rect::from_min_size(pos2(label.right() + 12.0, rect.top()), vec2(150.0, 36.0));
        let mut seg = ui.new_child(UiBuilder::new().max_rect(seg_rect).layout(Layout::left_to_right(Align::Center)));
        if widgets::segmented(&mut seg, "photo-3d-length", &mut length, &lengths, true) {
            super::form::save(cx.ctl, &snap.settings, &Settings { photo_3d: length, ..snap.settings.clone() });
        }
        let consent = snap.settings.consent_acknowledged;
        let t = Button::new(tr("Test pattern"))
            .icon(icons::FLASK)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(consent)
            .hint(tr("Record a generated test pattern (no VRChat needed)"));
        let tw = t.desired_width(ui);
        let test_at = Rect::from_min_size(pos2(rect.right() - tw, rect.top() + 2.0), vec2(tw, 32.0));
        test = t.show_at(ui, test_at).clicked();
        // The camera looks around, then circles at three heights, looking in and out: views a walk never gives.
        let s = Button::new(tr("Scan"))
            .icon(icons::PATH)
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(consent && snap.vrchat.running)
            .hint(tr("VRChat's camera looks around, then circles where it stands at three heights, looking in and out (about a minute); the recording is then reconstructed in 3D"));
        let sw = s.desired_width(ui);
        let sr = Rect::from_min_size(pos2(test_at.left() - 8.0 - sw, rect.top() + 2.0), vec2(sw, 32.0));
        scan = s.show_at(ui, sr).clicked();
    });
    if test {
        cx.ctl.start_test_capture();
    }
    if scan {
        cx.ctl.scan();
    }
}

/// What a capture under way is doing, who it left out, and how to stop it: in the place of the three actions.
fn under_way(ui: &mut Ui, cx: &mut Cx) {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let c = &snap.capture;
    let recording = matches!(c.state, CaptureState::Recording | CaptureState::Paused);
    // Who is out of the picture, or why not, or why no keyframes come.
    let warning = c.left_out.why.clone().or_else(|| c.no_strip.map(|n| n.message().to_string()));
    let w = ui.available_width();
    // Room for that as well as a free recording's Pause.
    let extra = if warning.is_some() && recording && c.picture.is_none() { 36.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(w, ACTION_H + extra), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-under-way"));
    ui.multiply_opacity(alpha);
    let tint = match c.picture {
        Some(PictureMode::People) => theme::VIOLET,
        Some(PictureMode::World) => theme::ACCENT,
        None => theme::REC,
    };
    widgets::frame(ui.painter(), rect, theme::CARD_RADIUS, theme::mix(theme::surface(), tint, 0.10));

    // The button at the right: the same one, now stopping.
    let size = ACTION_H - 40.0;
    let button = Rect::from_min_size(pos2(rect.right() - size - 28.0, rect.top() + 12.0), Vec2::splat(size));
    let health = c.stats.as_ref().map(|s| if s.frames > 0 { s.strip_ok as f32 / s.frames as f32 } else { 1.0 }).unwrap_or(1.0);
    let rb = RecordButton { state: rec_state(c.state), disabled: c.state == CaptureState::Importing, health };
    let (_, toggled) = rb.show_at(&mut ui, button);
    if toggled && snap.is_capturing() {
        cx.ctl.stop_capture();
    }
    let stop = match c.state {
        CaptureState::Stopping => tr("Saving…"),
        CaptureState::Importing => tr("Importing…"),
        CaptureState::Starting => tr("Starting…"),
        _ if c.picture.is_some() => tr("Stop this picture"),
        _ => tr("Stop recording"),
    };
    widgets::text(
        ui.painter(),
        pos2(button.center().x, rect.bottom() - 16.0),
        Align2::CENTER_CENTER,
        &Txt::new(stop, 13.0).weight(weight::MEDIUM).color(theme::FG_2),
    );

    // What is happening.
    let x = rect.left() + 26.0;
    let room = button.left() - 24.0 - x;
    let title = match c.picture {
        Some(PictureMode::People) => tr("Taking a 3D picture of people"),
        Some(PictureMode::World) => tr("Taking a 3D picture of the world"),
        None if c.state == CaptureState::Importing => tr("Importing a raw capture"),
        None if c.state == CaptureState::Paused => tr("Recording paused"),
        None => tr("Recording"),
    };
    let p = ui.painter().clone();
    if recording {
        let dot = pos2(x + 5.0, rect.top() + 28.0);
        widgets::status_dot(&p, dot, Tone::Rec, c.state == CaptureState::Recording);
        let elapsed = format::duration(c.stats.as_ref().map(|s| s.elapsed_s).unwrap_or(0.0));
        let kept = c.stats.as_ref().map_or(0, |s| s.keyframes);
        let line = fill(&tr_n("{elapsed}   ·   {count} keyframe", "{elapsed}   ·   {count} keyframes", kept as u64), &[("elapsed", &elapsed)]);
        widgets::text(&p, pos2(x + 18.0, rect.top() + 28.0), Align2::LEFT_CENTER, &Txt::new(line, 13.0).weight(weight::MEDIUM).color(theme::FG));
    }
    widgets::text_truncated(&p, pos2(x, rect.top() + 60.0), Align2::LEFT_CENTER, &Txt::new(title, 24.0).weight(weight::SEMIBOLD).tracking(-0.01), room);
    let doing = match (c.picture, c.state) {
        (_, CaptureState::Stopping) => tr("Saving the session…").to_string(),
        (Some(PictureMode::People), _) => tr("VRChat's camera goes around the people here, then comes back where it was").to_string(),
        (Some(PictureMode::World), _) => tr("VRChat's camera goes through the place, then comes back where it was").to_string(),
        // Nothing comes before VRChat's camera streams (the worker names the sender once it is connected).
        _ if snap.sources.is_empty() && c.stats.as_ref().is_none_or(|s| s.frames == 0) => {
            if snap.settings.osc.enabled && snap.settings.osc.drive_camera {
                tr("Waiting for VRChat's camera: Orbiters asked for Stream mode with Spout Stream on (OSC must be on in VRChat)").to_string()
            } else {
                tr("Waiting for VRChat's camera: open it in Stream mode and turn Spout Stream on").to_string()
            }
        }
        _ => c.source.clone().unwrap_or_default(),
    };
    widgets::text_truncated(&p, pos2(x, rect.top() + 88.0), Align2::LEFT_CENTER, &Txt::new(doing, 13.5).color(theme::FG_2), room);
    if let Some(warning) = warning {
        // The wearer comes out afterwards: nothing is going wrong, so it reads as a note.
        let ink = if c.left_out.wearer == Left::InPicture { theme::FG_2 } else { theme::WARN };
        let galley = p.layout_job(Txt::new(warning, 13.0).color(ink).line_height(17.0).job(room));
        p.galley(pos2(x, rect.top() + 104.0), galley, ink);
    } else {
        // Whole sentences for each case: a joined list could not be put in order or punctuated in every language.
        let out = match (c.left_out.wearer == Left::Out, c.left_out.others == Left::Out) {
            (true, true) => Some(tr("Your avatar is hidden on your side, other players are hidden from the camera")),
            (true, false) => Some(tr("Your avatar is hidden on your side")),
            (false, true) => Some(tr("other players are hidden from the camera")),
            (false, false) => None,
        };
        if let Some(out) = out {
            widgets::text_truncated(&p, pos2(x, rect.top() + 114.0), Align2::LEFT_CENTER, &Txt::new(out, 13.5).color(theme::ACCENT), room);
        }
    }
    // A free recording's pause, and its folder.
    if recording && c.picture.is_none() {
        let paused = c.state == CaptureState::Paused;
        let pause = Button::new(if paused { tr("Resume") } else { tr("Pause") }).icon(if paused { icons::PLAY } else { icons::PAUSE }).size(Size::Sm);
        let folder = c.session_dir.as_ref().map(|d| (d.clone(), format::folder_name(d)));
        let folder_btn = folder.as_ref().map(|(_, name)| Button::new(name).icon(icons::FOLDER_OPEN).variant(Variant::Ghost).size(Size::Sm));
        let pw = pause.desired_width(&ui);
        let y = rect.bottom() - 46.0;
        if pause.show_at(&mut ui, Rect::from_min_size(pos2(x, y), vec2(pw, 32.0))).clicked() {
            cx.ctl.pause_capture(!paused);
        }
        if let (Some(b), Some((dir, _))) = (folder_btn, folder.as_ref()) {
            let fw = b.desired_width(&ui).min(220.0).min(room - pw - 8.0);
            if b.width(fw).show_at(&mut ui, Rect::from_min_size(pos2(x + pw + 8.0, y), vec2(fw, 32.0))).clicked() {
                let r = crate::controller::open_path(dir);
                cx.run(&ctx, r);
            }
        }
    }
}

/// What capturing needs, as status cards: VRChat, N4N0 on the avatar (the one card to act on: it opens the avatar
/// picker), the Spout source, the Action Menu and the storage. Five on a row, or three then two on narrow windows.
fn chips(ui: &mut Ui, cx: &mut Cx, projects: &mut super::projects::State) {
    let snap = cx.snap;
    let world = &snap.vrchat.world;
    let source = if snap.settings.spout_sender.is_empty() {
        snap.sources.iter().find(|s| s.name.to_lowercase().contains("vrchat")).or(snap.sources.first())
    } else {
        snap.sources.iter().find(|s| s.name == snap.settings.spout_sender)
    };
    let osc = &snap.osc;
    let storage = &snap.storage;
    let chips = [
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            (
                icons::MONITOR_PLAY,
                "VRChat",
                if snap.vrchat.running {
                    if world.in_world {
                        tr("In a world").to_string()
                    } else {
                        tr("Running").into()
                    }
                } else {
                    tr("Not running").into()
                },
                if snap.vrchat.running { Tone::Ok } else { Tone::Off },
            )
        } else {
            let (text, tone) = super::network::sidebar_status(&snap.lan);
            (icons::MONITOR_PLAY, "VRChat", text, tone)
        },
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            (
                icons::BROADCAST,
                tr("Spout source"),
                source.map(|s| format!("{} · {}×{}", s.name, s.width, s.height)).unwrap_or_else(|| tr("VRChat's camera: Stream mode, Spout Stream on").into()),
                if source.is_some() { Tone::Ok } else { Tone::Warn },
            )
        } else {
            (icons::BROADCAST, tr_ctx("the noun", "Recording"), tr("Where VRChat runs").into(), Tone::Off)
        },
        (
            icons::SPARKLE,
            tr("Action Menu"),
            osc.last_event
                .clone()
                .unwrap_or_else(|| if osc.listening { tr("N4N0 › Record").into() } else { osc.error.clone().unwrap_or_else(|| tr("OSC off").into()) }),
            if osc.listening {
                Tone::Ok
            } else if osc.enabled {
                Tone::Warn
            } else {
                Tone::Off
            },
        ),
        (
            icons::HARD_DRIVE,
            tr("Storage"),
            match storage.free_bytes {
                Some(free) => tr!("{used} used · {free} free", used = format::bytes(storage.used_bytes as f64), free = format::bytes(free as f64)),
                None => tr!("{used} used", used = format::bytes(storage.used_bytes as f64)),
            },
            if storage.free_bytes.is_some_and(|f| f < 20_000_000_000) { Tone::Warn } else { Tone::Ok },
        ),
    ];
    // Each card as wide as its words want, the row filled; three then two on windows too narrow for that.
    let width = ui.available_width();
    let want = |label: &str, value: &str| {
        14.0 + 36.0 + 12.0 + widgets::text_size(ui, &Txt::caps(label)).x.max(widgets::text_size(ui, &Txt::new(value, 13.0).weight(weight::MEDIUM)).x) + 14.0
    };
    let mut wanted: Vec<f32> = chips.iter().map(|(_, label, value, _)| want(label, value)).collect();
    wanted.insert(1, want("N4N0", tr("Add to my avatar")) + 26.0);
    // One row only while the words fit whole (below 1 they are cut): a language with longer words (French) takes two rows rather than cut them.
    let one_row = orbiters_ui::layout::share_scale(&wanted, width, 12.0) >= 1.0;
    let rows: Vec<std::ops::Range<usize>> = if one_row { vec![0..5] } else { vec![0..3, 3..5] };
    let (rect, _) = ui.allocate_exact_size(vec2(width, rows.len() as f32 * 74.0 - 12.0), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut cells = Vec::with_capacity(5);
    for (r, range) in rows.iter().enumerate() {
        // Each row filled, as wide as its cards' words want (the second row's two cards included).
        let widths = orbiters_ui::layout::share(&wanted[range.clone()], rect.width(), 12.0);
        let mut x = rect.left();
        for w in widths {
            cells.push(Rect::from_min_size(pos2(x, rect.top() + r as f32 * 74.0), vec2(w, 62.0)));
            x += w + 12.0;
        }
    }
    let cell = |i: usize| cells[i];
    let mut cu = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-chips"));
    cu.multiply_opacity(alpha);
    // N4N0 second, after VRChat.
    for (i, (glyph, label, value, tone)) in chips.iter().enumerate() {
        let slot = if i == 0 { 0 } else { i + 1 };
        widgets::status_chip(&mut cu, cell(slot), Id::unique(("capture-chip", i)), glyph, label, value, *tone);
    }
    super::projects::capture_card(&mut cu, cx, projects, cell(1));
}

fn grid(ui: &mut Ui, cx: &mut Cx) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), GRID_H), Sense::hover());
    tiles(ui, cx, rect);
    recording_settings(ui, cx);
}

/// How a free recording is kept: what it does while recording, and how much of it stays. On one row: it rarely changes.
fn recording_settings(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let settings = &snap.settings;
    block(ui, cx, 9, 76.0, |ui, card| {
        widgets::card(ui.painter(), card, theme::CARD_RADIUS);
        let p = ui.painter().clone();
        widgets::text(&p, pos2(card.left() + 22.0, card.top() + 28.0), Align2::LEFT_CENTER, &Txt::caps(tr_ctx("the noun", "Recording")));
        let sub = if snap.osc.listening { tr("Also from your Action Menu") } else { tr("Saved as a session") };
        widgets::text_truncated(&p, pos2(card.left() + 22.0, card.top() + 50.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::FG_3), 180.0);
        let mut mode = settings.capture_mode;
        let mut quality = settings.storage_quality;
        let modes = [
            SegOption::new(CaptureMode::RecordOnly, tr("Record")).hint(tr("Record now, reconstruct afterwards with the whole GPU")),
            SegOption::new(CaptureMode::LiveRefine, tr("Live + refine")).hint(tr("Map live while playing, refine afterwards")).disabled(tr("soon")),
            SegOption::new(CaptureMode::LiveOnly, tr("Live only")).hint(tr("The live map is the result")).disabled(tr("soon")),
        ];
        let qualities = [
            SegOption::new(StorageQuality::Lossless, tr("Lossless")).hint(tr("Every frame saved losslessly (very large)")),
            SegOption::new(StorageQuality::High, tr("High")).hint(tr("Lossless keyframes, poses and depth")),
            SegOption::new(StorageQuality::Compact, tr("Compact")).hint(tr("Fewer keyframes, smaller sessions")),
        ];
        let area = Rect::from_min_max(pos2(card.left() + 200.0, card.top()), pos2(card.right() - 20.0, card.bottom()));
        let mut seg = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)));
        seg.spacing_mut().item_spacing = vec2(16.0, 0.0);
        let quality_changed = widgets::segmented(&mut seg, "storage-quality", &mut quality, &qualities, true);
        let mode_changed = widgets::segmented(&mut seg, "capture-mode", &mut mode, &modes, false);
        if mode_changed || quality_changed {
            super::form::save(cx.ctl, settings, &Settings { capture_mode: mode, storage_quality: quality, ..settings.clone() });
        }
    });
}

/// The capture notice, in the place of the actions until it is accepted.
fn consent(ui: &mut Ui, cx: &mut Cx) {
    let w = ui.available_width();
    let mut bullets = vec![
        tr("Capture your own worlds, or worlds whose creators agreed, and keep the results private."),
        tr("Other players are left out: a 3D picture of the world hides them from the Stream camera, or turn Remote Players off."),
    ];
    // With VRChat sign-in open to the member, the app can hold their VRChat session: the promise would not hold.
    if !cx.snap.vrchat_login.allowed {
        bullets.push(tr("Orbiters - N4N0 never asks for your VRChat login and only reads VRChat's local log, OSC and Spout."));
    }
    let galleys: Vec<_> = bullets.iter().map(|b| ui.painter().layout_job(Txt::new(*b, 13.0).color(theme::FG_2).line_height(20.0).job(w - 300.0))).collect();
    let text_h: f32 = galleys.iter().map(|g| g.size().y + 8.0).sum();
    let h = (24.0 + 36.0 + 16.0 + text_h + 16.0).max(ACTION_H);
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let (rect, alpha) = entering(ui, cx, 1, rect);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("capture-consent"));
    ui.multiply_opacity(alpha);
    let p = ui.painter().clone();
    widgets::card(&p, rect, theme::CARD_RADIUS);
    let tile = Rect::from_min_size(rect.min + vec2(24.0, 24.0), vec2(36.0, 36.0));
    p.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.12));
    widgets::icon(&p, tile.center(), icons::SHIELD_CHECK, 18.0, theme::ACCENT);
    widgets::text(&p, pos2(tile.right() + 10.0, tile.center().y), Align2::LEFT_CENTER, &Txt::new(tr("Before you capture"), 16.0).weight(weight::SEMIBOLD));
    let mut y = tile.bottom() + 16.0;
    for g in galleys {
        p.circle_filled(pos2(rect.left() + 30.0, y + 10.0), 2.0, theme::FG_3);
        let gh = g.size().y;
        p.galley(pos2(rect.left() + 24.0 + 16.0, y), g, theme::FG_2);
        y += gh + 8.0;
    }
    let button = Rect::from_min_size(pos2(rect.right() - 24.0 - 200.0, rect.center().y - 22.0), vec2(200.0, 44.0));
    if Button::new(tr("I understand")).variant(Variant::Primary).width(button.width()).show_at(&mut ui, button).clicked() {
        super::form::save(cx.ctl, &cx.snap.settings, &Settings { consent_acknowledged: true, ..cx.snap.settings.clone() });
    }
}

fn tiles(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let snap = cx.snap;
    let s = snap.capture.stats.as_ref();
    let health = s.map(|s| if s.frames > 0 { s.strip_ok as f64 / s.frames as f64 } else { 1.0 }).unwrap_or(1.0);
    let w = (rect.width() - 24.0) / 3.0;
    let h = (rect.height() - 12.0) / 2.0;
    let n = |ui: &Ui, i: usize, v: f64| widgets::animated_number(ui, Id::unique(("capture-number", i)), v);
    let frames = n(ui, 0, s.map(|s| s.frames as f64).unwrap_or(0.0));
    let decoded = n(ui, 1, health * 100.0);
    let keyframes = n(ui, 2, s.map(|s| s.keyframes as f64).unwrap_or(0.0));
    let depth = n(ui, 3, s.map(|s| s.depth_frames as f64).unwrap_or(0.0));
    let fps = n(ui, 4, s.map(|s| s.fps as f64).unwrap_or(0.0));
    let written = n(ui, 5, s.map(|s| s.bytes as f64).unwrap_or(0.0));
    let accent = s.map(|_| {
        if health > 0.97 {
            theme::OK
        } else if health > 0.8 {
            theme::WARN
        } else {
            theme::DANGER
        }
    });
    let tiles: [(&str, &str, String, String, Option<egui::Color32>); 6] = [
        (icons::STACK, tr("Frames"), format::integer(frames), s.map(|s| format!("{}×{}", s.width, s.height)).unwrap_or_else(|| "–".into()), None),
        (
            icons::SCAN,
            tr("Strip decoded"),
            s.map(|_| if decoded > 99.95 { format!("{decoded:.0}%") } else { format!("{decoded:.1}%") }).unwrap_or_else(|| "–".into()),
            match s {
                // Gone for a while, and why: no keyframes come meanwhile.
                Some(s) if s.strip_missing_s >= 2.0 => {
                    tr!(
                        "none for {seconds} s: {reason}",
                        seconds = format!("{:.0}", s.strip_missing_s),
                        reason = snap.capture.no_strip.unwrap_or(crate::state::NoStrip::Unknown).short()
                    )
                }
                Some(s) => tr!("{count} unreadable", count = format::integer(s.strip_failed as f64)),
                None => tr("pose strip in every frame").into(),
            },
            if s.is_some_and(|s| s.strip_missing_s >= 2.0) { Some(theme::WARN) } else { accent },
        ),
        (
            icons::CUBE,
            tr("Keyframes"),
            format::integer(keyframes),
            match s {
                Some(s) if s.dropped > 0 => tr!("{count} dropped", count = s.dropped),
                _ => tr("lossless images").into(),
            },
            None,
        ),
        (
            icons::CPU,
            tr("Depth frames"),
            format::integer(depth),
            match s {
                // Decoding fell behind (a slow disk or a busy processor): the recording went on without those.
                Some(s) if s.depth_dropped > 0 => tr!("{count} dropped", count = format::integer(s.depth_dropped as f64)),
                Some(s) => tr!("{percent} of frames", percent = format::percent(s.depth_frames as f64, s.frames as f64)),
                None => tr("exact depth samples").into(),
            },
            s.filter(|s| s.depth_dropped > 0).map(|_| theme::WARN),
        ),
        (
            icons::GAUGE,
            tr("Frame rate"),
            s.map(|_| format!("{fps:.1} fps")).unwrap_or_else(|| "–".into()),
            s.map(|s| tr!("decode {ms} ms", ms = format!("{:.1}", s.decode_ms))).unwrap_or_else(|| tr("per frame").into()),
            None,
        ),
        (
            icons::HARD_DRIVE,
            tr("Written"),
            s.map(|_| format::bytes(written)).unwrap_or_else(|| "–".into()),
            s.map(|s| tr!("{duration} recorded", duration = format::duration(s.elapsed_s))).unwrap_or_else(|| tr("this session").into()),
            None,
        ),
    ];
    for (i, (glyph, label, number, sub, accent)) in tiles.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + (i % 3) as f32 * (w + 12.0), rect.top() + (i / 3) as f32 * (h + 12.0)), vec2(w, h));
        let (r, alpha) = entering(ui, cx, 3 + i, r);
        let mut child = ui.new_child(UiBuilder::new().max_rect(r.expand(8.0)).id_salt(("tile", i)));
        child.multiply_opacity(alpha);
        widgets::stat_tile(&mut child, r, Id::unique(("capture-tile", i)), &widgets::Stat { glyph, label, number, sub, accent: *accent });
    }
}

/// What the session records besides the frames: avatar parameters, voice, VRChat's sound, body tracking, and the
/// SteamVR tab with the latest T-pose calibration.
fn performance(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let capturing = snap.is_capturing();
    let mut changed = None;
    block(ui, cx, 9, 164.0, |ui, rect| {
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter().clone();
        widgets::text(&p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps(tr("Performance")));
        let vr = &snap.steamvr;
        let right = match (&vr.calibration, vr.running, vr.overlay) {
            _ if capturing => tr("Switches apply to the next recording").to_string(),
            (Some(c), _, _) => tr!("T-pose: eyes at {eyes} m, hands {hands} m apart", eyes = format!("{:.2}", c.head_height_m), hands = format!("{:.2}", c.arm_span_m)),
            // VRChat sends its head and hand poses: Mixer needs no T-pose for its sessions.
            _ if snap.vrchat.running => tr("No T-pose needed: VRChat sends its head and hand poses").into(),
            (None, true, true) => tr("Calibrate in the Orbiters tab of SteamVR's dashboard").into(),
            (None, true, false) => tr("SteamVR is running").into(),
            (None, false, _) => tr("Not calibrated: start SteamVR, then calibrate in its dashboard").into(),
        };
        widgets::text_truncated(
            &p,
            pos2(rect.right() - 20.0, rect.top() + 26.0),
            Align2::RIGHT_CENTER,
            &Txt::new(right, 12.5).color(theme::FG_3),
            rect.width() * 0.6,
        );
        let rows = crate::streams::streams(snap);
        let gap = 12.0;
        let w = (rect.width() - 32.0 - gap * 3.0) / 4.0;
        for (i, row) in rows.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(rect.left() + 16.0 + i as f32 * (w + gap), rect.top() + 48.0), vec2(w, rect.height() - 64.0));
            widgets::frame(&p, cell, 14.0, theme::white(0.03));
            let tile = Rect::from_min_size(cell.min + vec2(14.0, 14.0), vec2(36.0, 36.0));
            p.rect_filled(tile, theme::radius(11.0), theme::alpha(row.tone.color(), 0.12));
            widgets::icon(&p, tile.center(), row.glyph, 17.0, if row.tone == Tone::Off { theme::FG_3 } else { row.tone.color() });
            widgets::text(&p, pos2(cell.left() + 14.0, tile.bottom() + 18.0), Align2::LEFT_CENTER, &Txt::new(row.name, 14.0).weight(weight::SEMIBOLD));
            widgets::text_truncated(
                &p,
                pos2(cell.left() + 14.0, tile.bottom() + 38.0),
                Align2::LEFT_CENTER,
                &Txt::new(&row.status, 12.0).color(theme::FG_3),
                cell.width() - 28.0,
            );
            if let Some(level) = row.level {
                crate::streams::meter(
                    ui,
                    Id::unique(("capture-meter", i)),
                    Rect::from_min_size(pos2(tile.right() + 12.0, tile.center().y - 2.5), vec2(cell.width() - 36.0 - 14.0 - 12.0 - 62.0, 5.0)),
                    level,
                );
            }
            if let Some(on) = row.switch {
                let mut value = on;
                let mut sw = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(pos2(cell.right() - 14.0 - 48.0, tile.center().y - 14.0), vec2(48.0, 28.0)))
                        .id_salt(("capture-switch", i)),
                );
                if capturing {
                    sw.disable();
                }
                if widgets::toggle(&mut sw, ("capture-switch", i), &mut value) {
                    changed = Some(crate::streams::with_switch(&snap.settings, i, value));
                }
            }
        }
    });
    if let Some(s) = changed {
        super::form::save(cx.ctl, &snap.settings, &s);
    }
}

fn camera_path(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let c = &snap.capture;
    let s = c.stats.as_ref();
    let empty_text = tr("The camera path appears here while you record");
    block(ui, cx, 10, 314.0, |ui, rect| {
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter();
        widgets::text(p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps(tr("Camera path")));
        let sub = match s.and_then(|s| s.position) {
            Some([x, y, z]) => format!("x {x:.1}   y {y:.1}   z {z:.1} m"),
            None => tr("Exact positions from the capture strip, top-down").into(),
        };
        widgets::text(p, pos2(rect.left() + 20.0, rect.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::FG_3));
        // What went wrong: the capture's error, else what the recording goes on without (a microphone that could not
        // be opened, files that could not be written), as the worker last said them.
        let warnings = s.map(|s| s.warnings.join(" · ")).filter(|w| !w.is_empty());
        if let Some((text, color)) = c.error.clone().map(|e| (e, theme::DANGER)).or_else(|| warnings.map(|w| (w, theme::WARN))) {
            widgets::text_truncated(
                p,
                pos2(rect.right() - 20.0, rect.top() + 36.0),
                Align2::RIGHT_CENTER,
                &Txt::new(text, 12.0).color(color),
                rect.width() * 0.55,
            );
        }
        let map = Rect::from_min_max(pos2(rect.left() + 16.0, rect.top() + 68.0), pos2(rect.right() - 16.0, rect.bottom() - 16.0));
        let coverage = c.coverage.as_ref().map(|m| trail::Coverage {
            x0: m.x0,
            z0: m.z0,
            cell: m.cell_m,
            width: m.width,
            depth: m.depth,
            cells: &m.cells,
            obstacle: orbiters_core::protocol::coverage::OBSTACLE,
        });
        trail::show_with(
            ui,
            map,
            Id::unique("capture-trail"),
            &trail::Trail {
                points: &c.trail,
                heading: s.and_then(|s| s.heading_deg),
                live: c.state == CaptureState::Recording,
                empty_text,
            },
            coverage.as_ref(),
        );
        // What to do next, over the map's top; else what the colours mean.
        if let Some(m) = &c.coverage {
            let p = ui.painter();
            if m.hints.is_empty() {
                let mut x = map.left() + 12.0;
                let y = map.top() + 14.0;
                widgets::text(p, pos2(x, y), Align2::LEFT_CENTER, &Txt::new(tr("Seen from"), 11.5).color(theme::FG_3));
                x += 62.0;
                for (directions, label) in [(1u8, tr("1–2 directions")), (3, "3–5"), (6, tr("6 or more"))] {
                    p.rect_filled(Rect::from_center_size(pos2(x + 5.0, y), vec2(10.0, 10.0)), 2.0, theme::alpha(trail::coverage_colour(directions), 0.7));
                    widgets::text(p, pos2(x + 14.0, y), Align2::LEFT_CENTER, &Txt::new(label, 11.5).color(theme::FG_3));
                    // Wide letters (Japanese) take about twice a Latin letter's room.
                    x += 24.0 + label.chars().map(|ch| if ch.is_ascii() || ch.len_utf8() < 3 { 6.5 } else { 11.5 }).sum::<f32>();
                }
            } else {
                for (k, hint) in m.hints.iter().take(2).enumerate() {
                    let y = map.top() + 16.0 + 22.0 * k as f32;
                    p.circle_filled(pos2(map.left() + 16.0, y), 3.0, theme::WARN);
                    widgets::text_truncated(p, pos2(map.left() + 26.0, y), Align2::LEFT_CENTER, &Txt::new(hint, 12.5).color(theme::FG), map.width() - 40.0);
                }
            }
        }
    });
}
