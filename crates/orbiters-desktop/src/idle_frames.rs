//! `orbiters-desktop --idle-frames [--seconds <s>] [--warmup <s>] [--focused] [--pages home,sessions,…]` counts the
//! frames each page of the window draws while nobody touches it and nothing changes: the measure of "a still
//! interface costs nothing" (`orbiters_ui::anim`). No window and no GPU: each page runs in an egui context of its own,
//! over the stand-in controller of `--gpu-bench` (the screenshots' made-up snapshot, never started), and a frame is
//! run exactly when egui was asked for one (`request_repaint`, `request_repaint_after`), as eframe would. The pages
//! run side by side, each on its thread, so a run lasts the warm-up plus the measure whatever their number.
//!
//! Unfocused by default (the window left open behind VRChat: springs jump, looping decorations stand still), so what
//! is counted is what the pages ask for themselves. `--focused` measures a focused window instead; its warm-up should
//! then pass `orbiters_ui::anim::REST_AFTER`, after which the decorations rest.
//!
//! Each page prints its frames a minute and the lines that asked for them, the most frequent first. The settings and
//! caches are a folder of the run's own (as the screenshots'), so nothing of the person's is written; their pictures,
//! sessions and Unity projects also stay within the run's private roots.
//!
//! `states` is not a page of the sidebar: it is Home on the avatars' states, their cards showing, with something
//! asked that the service never answers (what was asked there once kept the window drawing).
//!
//! `orbiters-desktop --idle-frames --local-revision` times instead what Photos & captures' watch does every fifteen
//! seconds on its thread (`controller::shares::LocalRevision`), on its private fixture library, which it only reads.

use crate::app::App;
use crate::backend::{Ctl, Local};
use crate::controller::{self, Controller};
use crate::pages::Page;
use anyhow::Context as _;
use egui::{vec2, Pos2, RawInput, Rect, ViewportId, ViewportInfo};
use orbiters_core::settings::Settings;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const PAGES: [(&str, Page); 10] = [
    ("home", Page::Home),
    (STATES, Page::Home),
    ("n4n0", Page::N4n0),
    ("capture", Page::Capture),
    ("projects", Page::Projects),
    ("sessions", Page::Sessions),
    ("worlds", Page::Worlds),
    ("events", Page::Events),
    ("videos", Page::Videos),
    ("settings", Page::Settings),
];

/// Home on the avatars' states (see the module's words).
const STATES: &str = "states";

/// Times the look of Photos & captures' watch on this computer's library (read only): how many captures it counts,
/// and how long a look takes the first time, again, and with the visit history read and parsed anew, which a look
/// does when the service wrote the history since the last one (every ten seconds while VRChat writes its log).
fn local_revision() -> anyhow::Result<()> {
    use crate::controller::shares::LocalRevision;
    use orbiters_core::account::Server;
    use orbiters_core::visited;
    let settings = Settings::load();
    let server = if settings.development { Server::Development } else { Server::Production };
    let root = &settings.sessions_dir;
    let history = visited::history_file();
    let ms = |from: Instant| from.elapsed().as_secs_f64() * 1e3;
    let middle = |mut times: Vec<f64>| {
        times.sort_by(f64::total_cmp);
        (times[0], times[times.len() / 2], times[times.len() - 1])
    };
    println!("captures under {}: {}", root.display(), LocalRevision::captures(root));
    println!("visit history: {} bytes", std::fs::metadata(&history).map(|m| m.len()).unwrap_or(0));
    let mut revision = LocalRevision::default();
    let from = Instant::now();
    let first = revision.look(root, server);
    println!("first look of the process: {:.2} ms", ms(from));
    let mut same = true;
    let again = middle(
        (0..50)
            .map(|_| {
                let from = Instant::now();
                same &= revision.look(root, server) == first;
                ms(from)
            })
            .collect(),
    );
    println!("the look again, the history not read (50 times): {:.2} ms at least, {:.2} in the middle, {:.2} at most", again.0, again.1, again.2);
    let parsed = middle(
        (0..50)
            .map(|_| {
                let from = Instant::now();
                same &= LocalRevision::default().look(root, server) == first;
                ms(from)
            })
            .collect(),
    );
    println!("the look with the history read and parsed (50 times): {:.2} ms at least, {:.2} in the middle, {:.2} at most", parsed.0, parsed.1, parsed.2);
    let alone = middle(
        (0..50)
            .map(|_| {
                let from = Instant::now();
                std::hint::black_box(visited::History::load(&history).revision());
                ms(from)
            })
            .collect(),
    );
    println!("the history alone, read, parsed and counted (50 times): {:.2} ms at least, {:.2} in the middle, {:.2} at most", alone.0, alone.1, alone.2);
    println!("the same revision every time: {same}");
    Ok(())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    if args.first().map(String::as_str) == Some("--local-revision") {
        return local_revision();
    }
    let (mut seconds, mut warmup, mut focused) = (60.0f64, 8.0f64, false);
    let mut pages: Vec<(&str, Page)> = PAGES.to_vec();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => seconds = it.next().context("--seconds <s>")?.parse()?,
            "--warmup" => warmup = it.next().context("--warmup <s>")?.parse()?,
            "--focused" => focused = true,
            "--pages" => {
                let names = it.next().context("--pages <name,…>")?;
                pages = names
                    .split(',')
                    .map(|n| PAGES.iter().copied().find(|(name, _)| *name == n).with_context(|| format!("no page named {n}")))
                    .collect::<anyhow::Result<_>>()?;
            }
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    // Before any path is asked for: what the pages keep (the pictures' index, thumbnails, world pages) goes to a
    // folder of this run.

    let settings = Settings { consent_acknowledged: true, ..Default::default() };
    let sessions = controller::list_sessions(&settings.sessions_dir);
    let snapshot = crate::screenshot::idle_snapshot(&settings, &sessions);
    println!("{} the window, {warmup:.0} s to settle, then {seconds:.0} s counted", if focused { "focused" } else { "behind another" });
    let threads: Vec<_> = pages
        .into_iter()
        .map(|(name, page)| {
            let (settings, snapshot) = (settings.clone(), snapshot.clone());
            // A page's frame is deep in a debug build (the screenshots run on such a stack too).
            std::thread::Builder::new().name(format!("idle-{name}")).stack_size(16 << 20).spawn(move || {
                let ctl = Controller::new(settings);
                ctl.set_snapshot(snapshot);
                let backend: Ctl = Arc::new(Local::new(ctl));
                (name, count(backend, page, name == STATES, warmup, seconds, focused))
            })
        })
        .collect::<Result<_, _>>()?;
    for thread in threads {
        let (name, counted) = thread.join().map_err(|_| anyhow::anyhow!("a page stopped on a panic"))?; // i18n: not shown (a developer's command line, in English)
        let per_minute = counted.frames as f64 * 60.0 / seconds;
        println!("{name}: {} frames in {seconds:.0} s ({per_minute:.0} a minute)", counted.frames);
        let mut causes: Vec<_> = counted.causes.into_iter().collect();
        causes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (cause, n) in causes.into_iter().take(6) {
            println!("    {n:>6}  {cause}");
        }
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    Ok(())
}

struct Counted {
    frames: u32,
    /// Who asked for the frames counted (`file:line`), and how many times.
    causes: HashMap<String, u32>,
}

/// Runs `page` for `warmup` seconds, then counts its frames for `seconds`. `states`: Home, on the avatars' states
/// with something asked of them.
fn count(backend: Ctl, page: Page, states: bool, warmup: f64, seconds: f64, focused: bool) -> Counted {
    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    // When the next frame was asked for: what an integration is told (only requests that bring it nearer).
    let (tx, rx) = mpsc::channel::<Instant>();
    ctx.set_request_repaint_callback(move |info| {
        let _ = tx.send(Instant::now() + info.delay);
    });
    backend.attach(&ctx);
    let mut app = App::offline(backend);
    app.skip_splash();
    let start = Instant::now();
    app.set_page(page, 0.0);
    let measure_from = start + Duration::from_secs_f64(warmup);
    let end = measure_from + Duration::from_secs_f64(seconds);
    let mut counted = Counted { frames: 0, causes: HashMap::new() };
    let mut staged = false;
    let mut next = Some(start);
    // A display shows so many frames a second at most: frames asked for "now" come at its rate.
    let display = Duration::from_secs_f64(1.0 / 60.0);
    let mut last = start - display;
    loop {
        // Sleeps until the frame asked for, or an earlier one asked meanwhile (a worker's).
        let now = Instant::now();
        let wait = next.map_or(end, |at| at.min(end)).saturating_duration_since(now);
        match rx.recv_timeout(wait) {
            Ok(at) => {
                let at = at.max(last + display);
                next = Some(next.map_or(at, |n| n.min(at)));
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= end || next.is_none() {
            break;
        }
        next = None;
        last = Instant::now();
        let mut input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))),
            time: Some(start.elapsed().as_secs_f64()),
            focused,
            ..Default::default()
        };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(focused), ..Default::default() });
        let mut out = ctx.run_ui(input, |ui| {
            app.logic_offline(ui.ctx());
            app.draw(ui);
        });
        // Nothing paints these frames.
        out.textures_delta.clear();
        // Home's menu is made by its first frame: then its States page, and something asked there.
        if states && !staged {
            if let Some(host) = app.home_mut().menu_host() {
                let _ = host.press(orbiters_n4n0_core::menu::Target::States);
                app.home_mut().states_mut().stage_asked("avtr_idle", start.elapsed().as_secs_f64());
                staged = true;
                ctx.request_repaint();
            }
        }
        if Instant::now() >= measure_from {
            counted.frames += 1;
            // What asked for this frame was asked during the one before, or between the two.
            for cause in ctx.repaint_causes() {
                *counted.causes.entry(format!("{}:{}", cause.file.replace('\\', "/"), cause.line)).or_default() += 1;
            }
        }
    }
    counted
}
