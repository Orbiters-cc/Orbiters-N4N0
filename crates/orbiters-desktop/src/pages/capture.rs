//! Capture page: what VRChat and the sources are doing, the recorder, live statistics and the camera path.

use super::{block, entering, Cx};
use crate::state::CaptureState;
use egui::{pos2, vec2, Align, Align2, Id, Layout, Rect, Sense, Ui, UiBuilder, Vec2};
use orbiters_contracts::manifest::{CaptureMode, StorageQuality};
use orbiters_core::settings::Settings;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::format;
use orbiters_ui::record::{self, RecState, RecordButton};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, SegOption, Size, Tone, Variant};
use orbiters_ui::{icons, trail};

#[derive(Default)]
pub struct State;

const GRID_H: f32 = 500.0;

pub fn show(ui: &mut Ui, cx: &mut Cx, _state: &mut State) {
    egui::ScrollArea::vertical().id_salt("capture").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 8, bottom: 32 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            header(ui, cx);
            chips(ui, cx);
            grid(ui, cx);
            performance(ui, cx);
            camera_path(ui, cx);
        });
    });
}

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
    let capturing = snap.is_capturing() || c.state == CaptureState::Stopping;
    let consent = snap.settings.consent_acknowledged;
    let mut test = false;
    block(ui, cx, 0, 72.0, |ui, rect| {
        let caption = if recording {
            "Live capture"
        } else if world.in_world {
            "VRChat world"
        } else {
            "Capture"
        };
        let headline = match c.state {
            CaptureState::Importing => "Importing a raw capture".to_string(),
            CaptureState::Paused => "Paused".into(),
            CaptureState::Recording => "Recording".into(),
            _ => world.world_name.clone().filter(|_| world.in_world).unwrap_or_else(|| "Ready to capture".into()),
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
                let hint = if snap.vrchat.running {
                    "VRChat is running; join a world to tag captures with it."
                } else {
                    "Start VRChat and enable Spout streaming on the Stream camera."
                };
                widgets::text_truncated(p, pos2(rect.left(), sub_y), Align2::LEFT_CENTER, &Txt::new(hint, 13.0).color(theme::FG_3), rect.width() * 0.7);
            }
        }
        if !capturing {
            let b = Button::new("Test pattern")
                .icon(icons::FLASK)
                .variant(Variant::Ghost)
                .size(Size::Sm)
                .enabled(consent)
                .hint("Record a generated test pattern (no VRChat needed)");
            let w = b.desired_width(ui);
            let r = Rect::from_min_size(pos2(rect.right() - w, rect.bottom() - 34.0), vec2(w, 32.0));
            test = b.show_at(ui, r).clicked();
        }
    });
    if test {
        cx.ctl.start_test_capture();
    }
}

fn chips(ui: &mut Ui, cx: &mut Cx) {
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
        (
            icons::MONITOR_PLAY,
            "VRChat",
            if snap.vrchat.running {
                if world.in_world {
                    "In a world".to_string()
                } else {
                    "Running".into()
                }
            } else {
                "Not running".into()
            },
            if snap.vrchat.running { Tone::Ok } else { Tone::Off },
        ),
        (
            icons::BROADCAST,
            "Spout source",
            source.map(|s| format!("{} · {}×{}", s.name, s.width, s.height)).unwrap_or_else(|| "VRChat's camera: Stream mode, Spout Stream on".into()),
            if source.is_some() { Tone::Ok } else { Tone::Warn },
        ),
        (
            icons::SPARKLE,
            "Action Menu",
            osc.last_event.clone().unwrap_or_else(|| {
                if osc.listening {
                    format!("“{}” toggle", snap.settings.osc.record_parameter)
                } else {
                    osc.error.clone().unwrap_or_else(|| "OSC off".into())
                }
            }),
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
            "Storage",
            format!(
                "{} used{}",
                format::bytes(storage.used_bytes as f64),
                storage.free_bytes.map(|f| format!(" · {} free", format::bytes(f as f64))).unwrap_or_default()
            ),
            if storage.free_bytes.is_some_and(|f| f < 20_000_000_000) { Tone::Warn } else { Tone::Ok },
        ),
    ];
    block(ui, cx, 1, 62.0, |ui, rect| {
        let w = (rect.width() - 36.0) / 4.0;
        for (i, (glyph, label, value, tone)) in chips.iter().enumerate() {
            let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (w + 12.0), rect.top()), vec2(w, rect.height()));
            widgets::status_chip(ui, r, Id::unique(("capture-chip", i)), glyph, label, value, *tone);
        }
    });
}

fn grid(ui: &mut Ui, cx: &mut Cx) {
    // The recorder card grows by a row of buttons while recording.
    let recording = matches!(cx.snap.capture.state, CaptureState::Recording | CaptureState::Paused);
    let h = anim::value(ui.ctx(), Id::unique("capture-grid-h"), if recording { GRID_H + 46.0 } else { GRID_H }, Spring::SOFT);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let left_w = ((rect.width() - 20.0) * 1.05 / 2.05).max(420.0);
    let card = Rect::from_min_size(rect.min, vec2(left_w, rect.height()));
    let right = Rect::from_min_max(pos2(card.right() + 20.0, rect.top()), rect.max);
    recorder(ui, cx, card);
    tiles(ui, cx, right);
}

fn recorder(ui: &mut Ui, cx: &mut Cx, card: Rect) {
    let ctx = ui.ctx().clone();
    let snap = cx.snap;
    let c = &snap.capture;
    let settings = &snap.settings;
    let recording = matches!(c.state, CaptureState::Recording | CaptureState::Paused);
    let consent = settings.consent_acknowledged;
    let (card, alpha) = entering(ui, cx, 2, card);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(card).id_salt("recorder"));
    ui.multiply_opacity(alpha);
    widgets::card(ui.painter(), card, theme::CARD_RADIUS);

    // Header: caption and the REC badge.
    let p = ui.painter();
    widgets::text(p, pos2(card.left() + 24.0, card.top() + 36.0), Align2::LEFT_CENTER, &Txt::caps("Recorder"));
    let badge_k = anim::toggle(&ctx, Id::unique("rec-badge"), recording, Spring::BOUNCY);
    if badge_k > 0.01 {
        let label = if c.state == CaptureState::Paused { "PAUSED" } else { "REC" };
        let elapsed = format::duration(c.stats.as_ref().map(|s| s.elapsed_s).unwrap_or(0.0));
        let lt = Txt::new(label, 12.0).weight(weight::SEMIBOLD).color(theme::REC);
        let et = Txt::new(&elapsed, 12.0).weight(weight::MEDIUM).color(theme::FG);
        let w = 12.0 + 10.0 + 8.0 + widgets::text_size(&ui, &lt).x + 8.0 + widgets::text_size(&ui, &et).x + 12.0;
        let full = Rect::from_min_size(pos2(card.right() - 24.0 - w, card.top() + 23.0), vec2(w, 26.0));
        let r = Rect::from_center_size(full.center(), full.size() * (0.8 + 0.2 * badge_k));
        let mut bp = ui.painter().clone();
        bp.multiply_opacity(badge_k.clamp(0.0, 1.0));
        bp.rect_filled(r, theme::radius(13.0), theme::alpha(theme::REC, 0.15));
        widgets::status_dot(&bp, pos2(r.left() + 17.0, r.center().y), Tone::Rec, c.state == CaptureState::Recording);
        let tr = widgets::text(&bp, pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, &lt);
        widgets::text(&bp, pos2(tr.right() + 8.0, r.center().y), Align2::LEFT_CENTER, &et);
    }

    // The button.
    let button_rect = Rect::from_center_size(pos2(card.center().x, card.top() + 68.0 + record::SIZE * 0.5), Vec2::splat(record::SIZE));
    let health = c.stats.as_ref().map(|s| if s.frames > 0 { s.strip_ok as f32 / s.frames as f32 } else { 1.0 }).unwrap_or(1.0);
    let rb = RecordButton { state: rec_state(c.state), disabled: !consent || c.state == CaptureState::Importing, health };
    let (_, toggled) = rb.show_at(&mut ui, button_rect);
    if toggled {
        if snap.is_capturing() {
            cx.ctl.stop_capture();
        } else {
            cx.ctl.start_capture();
        }
    }

    // What the button does now.
    let title = match c.state {
        CaptureState::Idle => "Start recording",
        CaptureState::Starting => "Starting…",
        CaptureState::Recording | CaptureState::Paused => "Stop recording",
        CaptureState::Stopping => "Saving the session…",
        CaptureState::Importing => "Importing…",
    };
    let sub = match c.state {
        CaptureState::Idle if snap.osc.listening => "or use the Record toggle in your Action Menu".to_string(),
        CaptureState::Idle => "Spout frames are decoded and saved as a session".to_string(),
        // Nothing comes before VRChat's camera streams (the worker names the sender once it is connected).
        _ if snap.sources.is_empty() && c.stats.as_ref().is_none_or(|s| s.frames == 0) => {
            if snap.settings.osc.enabled && snap.settings.osc.drive_camera {
                "Waiting for VRChat's camera: Orbiters asked for Stream mode with Spout Stream on (OSC must be on in VRChat)".to_string()
            } else {
                "Waiting for VRChat's camera: open it in Stream mode and turn Spout Stream on".to_string()
            }
        }
        _ => c.source.clone().unwrap_or_default(),
    };
    let y = button_rect.bottom() + 8.0;
    widgets::sliding_value(
        &ui,
        Id::unique("rec-title"),
        pos2(card.center().x, y + 10.0),
        Align2::CENTER_CENTER,
        title,
        &Txt::new("", 15.0).weight(weight::SEMIBOLD),
        card.width() - 48.0,
    );
    widgets::sliding_value(
        &ui,
        Id::unique("rec-sub"),
        pos2(card.center().x, y + 31.0),
        Align2::CENTER_CENTER,
        &sub,
        &Txt::new("", 12.0).color(theme::FG_3),
        card.width() - 48.0,
    );

    // Mode and quality.
    let mut mode = settings.capture_mode;
    let mut quality = settings.storage_quality;
    let seg_top = y + 60.0;
    let mut seg = ui.new_child(
        UiBuilder::new().max_rect(Rect::from_min_size(pos2(card.left(), seg_top), vec2(card.width(), 140.0))).layout(Layout::top_down(Align::Center)),
    );
    seg.spacing_mut().item_spacing = vec2(0.0, 12.0);
    let modes = [
        SegOption::new(CaptureMode::RecordOnly, "Record").hint("Record now, reconstruct afterwards with the whole GPU"),
        SegOption::new(CaptureMode::LiveRefine, "Live + refine").hint("Map live while playing, refine afterwards").disabled("soon"),
        SegOption::new(CaptureMode::LiveOnly, "Live only").hint("The live map is the result").disabled("soon"),
    ];
    let qualities = [
        SegOption::new(StorageQuality::Lossless, "Lossless").hint("Every frame saved losslessly (very large)"),
        SegOption::new(StorageQuality::High, "High").hint("Lossless keyframes, poses and depth"),
        SegOption::new(StorageQuality::Compact, "Compact").hint("Fewer keyframes, smaller sessions"),
    ];
    let mode_changed = widgets::segmented(&mut seg, "capture-mode", &mut mode, &modes, false);
    let quality_changed = widgets::segmented(&mut seg, "storage-quality", &mut quality, &qualities, true);
    if mode_changed || quality_changed {
        cx.ctl.save_settings(Settings { capture_mode: mode, storage_quality: quality, ..settings.clone() });
    }

    if !consent {
        consent_overlay(&mut ui, cx, card);
        return;
    }

    // Pause and the session folder while recording.
    let row_k = anim::toggle(&ctx, Id::unique("rec-row"), recording, Spring::SOFT).clamp(0.0, 1.0);
    if recording {
        ui.multiply_opacity(row_k);
        let paused = c.state == CaptureState::Paused;
        let pause = Button::new(if paused { "Resume" } else { "Pause" }).icon(if paused { icons::PLAY } else { icons::PAUSE }).size(Size::Sm);
        let folder = c.session_dir.as_ref().map(|d| (d.clone(), format::folder_name(d)));
        let folder_btn = folder.as_ref().map(|(_, name)| Button::new(name).icon(icons::FOLDER_OPEN).variant(Variant::Ghost).size(Size::Sm));
        let pw = pause.desired_width(&ui);
        let fw = folder_btn.as_ref().map(|b| b.desired_width(&ui).min(220.0) + 8.0).unwrap_or(0.0);
        let row_y = seg_top + 40.0 + 12.0 + 36.0 + 14.0;
        let x0 = card.center().x - (pw + fw) * 0.5;
        if pause.show_at(&mut ui, Rect::from_min_size(pos2(x0, row_y), vec2(pw, 32.0))).clicked() {
            cx.ctl.pause_capture(!paused);
        }
        if let (Some(b), Some((dir, _))) = (folder_btn, folder.as_ref()) {
            if b.width(fw - 8.0).show_at(&mut ui, Rect::from_min_size(pos2(x0 + pw + 8.0, row_y), vec2(fw - 8.0, 32.0))).clicked() {
                let r = crate::controller::open_path(dir);
                cx.run(&ctx, r);
            }
        }
    }
}

fn consent_overlay(ui: &mut Ui, cx: &mut Cx, card: Rect) {
    let ctx = ui.ctx().clone();
    let id = Id::unique("consent");
    // Swallow clicks on the recorder below.
    ui.interact(card, id.with("shield"), Sense::click());
    let k = anim::spring_from(&ctx, id.with("in"), [0.0], [1.0], Spring::BOUNCY)[0];
    let p = ui.painter();
    widgets::frame(p, card, theme::CARD_RADIUS, theme::alpha(theme::INK_950, 0.78 * k.clamp(0.0, 1.0)));
    let w = (card.width() - 48.0).min(420.0);
    let bullets = [
        "Capture your own worlds, or worlds whose creators agreed, and keep the results private.",
        "Other players are left out: hide them in the Stream camera (Remote Players off).",
        "Orbiters desktop never asks for your VRChat login and only reads VRChat's local log, OSC and Spout.",
    ];
    let galleys: Vec<_> = bullets.iter().map(|b| p.layout_job(Txt::new(*b, 13.0).color(theme::FG_2).line_height(20.0).job(w - 48.0 - 16.0))).collect();
    let text_h: f32 = galleys.iter().map(|g| g.size().y + 8.0).sum();
    let h = 24.0 + 36.0 + 16.0 + text_h + 12.0 + 40.0 + 24.0;
    let base = Rect::from_center_size(card.center(), vec2(w, h));
    let dialog = Rect::from_center_size(base.center() + vec2(0.0, 16.0 * (1.0 - k)), base.size() * (0.92 + 0.08 * k));
    let mut dp = p.clone();
    dp.multiply_opacity(k.clamp(0.0, 1.0));
    widgets::panel(&dp, dialog, 24.0);
    let tile = Rect::from_min_size(dialog.min + vec2(24.0, 24.0), vec2(36.0, 36.0));
    dp.rect_filled(tile, theme::radius(12.0), theme::alpha(theme::ACCENT, 0.12));
    widgets::icon(&dp, tile.center(), icons::SHIELD_CHECK, 18.0, theme::ACCENT);
    widgets::text(&dp, pos2(tile.right() + 10.0, tile.center().y), Align2::LEFT_CENTER, &Txt::new("Before you capture", 16.0).weight(weight::SEMIBOLD));
    let mut y = tile.bottom() + 16.0;
    for g in galleys {
        dp.circle_filled(pos2(dialog.left() + 30.0, y + 10.0), 2.0, theme::FG_3);
        let gh = g.size().y;
        dp.galley(pos2(dialog.left() + 24.0 + 16.0, y), g, theme::FG_2);
        y += gh + 8.0;
    }
    let button = Rect::from_min_size(pos2(dialog.left() + 24.0, dialog.bottom() - 24.0 - 40.0), vec2(dialog.width() - 48.0, 40.0));
    if Button::new("I understand").variant(Variant::Primary).width(button.width()).show_at(ui, button).clicked() {
        cx.ctl.save_settings(Settings { consent_acknowledged: true, ..cx.snap.settings.clone() });
    }
}

fn tiles(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let snap = cx.snap;
    let s = snap.capture.stats.as_ref();
    let health = s.map(|s| if s.frames > 0 { s.strip_ok as f64 / s.frames as f64 } else { 1.0 }).unwrap_or(1.0);
    let w = (rect.width() - 12.0) / 2.0;
    let h = (rect.height() - 24.0) / 3.0;
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
        (icons::STACK, "Frames", format::integer(frames), s.map(|s| format!("{}×{}", s.width, s.height)).unwrap_or_else(|| "–".into()), None),
        (
            icons::SCAN,
            "Strip decoded",
            s.map(|_| if decoded > 99.95 { format!("{decoded:.0}%") } else { format!("{decoded:.1}%") }).unwrap_or_else(|| "–".into()),
            s.map(|s| format!("{} unreadable", format::integer(s.strip_failed as f64))).unwrap_or_else(|| "pose strip in every frame".into()),
            accent,
        ),
        (
            icons::CUBE,
            "Keyframes",
            format::integer(keyframes),
            match s {
                Some(s) if s.dropped > 0 => format!("{} dropped", s.dropped),
                _ => "lossless images".into(),
            },
            None,
        ),
        (
            icons::CPU,
            "Depth frames",
            format::integer(depth),
            s.map(|s| format!("{} of frames", format::percent(s.depth_frames as f64, s.frames as f64))).unwrap_or_else(|| "exact depth samples".into()),
            None,
        ),
        (
            icons::GAUGE,
            "Frame rate",
            s.map(|_| format!("{fps:.1} fps")).unwrap_or_else(|| "–".into()),
            s.map(|s| format!("decode {:.1} ms", s.decode_ms)).unwrap_or_else(|| "per frame".into()),
            None,
        ),
        (
            icons::HARD_DRIVE,
            "Written",
            s.map(|_| format::bytes(written)).unwrap_or_else(|| "–".into()),
            s.map(|s| format!("{} recorded", format::duration(s.elapsed_s))).unwrap_or_else(|| "this session".into()),
            None,
        ),
    ];
    for (i, (glyph, label, number, sub, accent)) in tiles.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + (i % 2) as f32 * (w + 12.0), rect.top() + (i / 2) as f32 * (h + 12.0)), vec2(w, h));
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
        widgets::text(&p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps("Performance"));
        let vr = &snap.steamvr;
        let right = match (&vr.calibration, vr.running, vr.overlay) {
            _ if capturing => "Switches apply to the next recording".to_string(),
            (Some(c), _, _) => format!("T-pose: eyes at {:.2} m, hands {:.2} m apart", c.head_height_m, c.arm_span_m),
            (None, true, true) => "Calibrate in the Orbiters tab of SteamVR's dashboard".into(),
            (None, true, false) => "SteamVR is running".into(),
            (None, false, _) => "Not calibrated: start SteamVR, then calibrate in its dashboard".into(),
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
        cx.ctl.save_settings(s);
    }
}

fn camera_path(ui: &mut Ui, cx: &mut Cx) {
    let snap = cx.snap;
    let c = &snap.capture;
    let s = c.stats.as_ref();
    block(ui, cx, 10, 314.0, |ui, rect| {
        widgets::card(ui.painter(), rect, theme::CARD_RADIUS);
        let p = ui.painter();
        widgets::text(p, pos2(rect.left() + 20.0, rect.top() + 26.0), Align2::LEFT_CENTER, &Txt::caps("Camera path"));
        let sub = match s.and_then(|s| s.position) {
            Some([x, y, z]) => format!("x {x:.1}   y {y:.1}   z {z:.1} m"),
            None => "Exact positions from the capture strip, top-down".into(),
        };
        widgets::text(p, pos2(rect.left() + 20.0, rect.top() + 46.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::FG_3));
        if let Some(err) = &c.error {
            widgets::text_truncated(
                p,
                pos2(rect.right() - 20.0, rect.top() + 36.0),
                Align2::RIGHT_CENTER,
                &Txt::new(err, 12.0).color(theme::DANGER),
                rect.width() * 0.55,
            );
        }
        let map = Rect::from_min_max(pos2(rect.left() + 16.0, rect.top() + 68.0), pos2(rect.right() - 16.0, rect.bottom() - 16.0));
        trail::show(
            ui,
            map,
            Id::unique("capture-trail"),
            &trail::Trail {
                points: &c.trail,
                heading: s.and_then(|s| s.heading_deg),
                live: c.state == CaptureState::Recording,
                empty_text: "The camera path appears here while you record",
            },
        );
    });
}
