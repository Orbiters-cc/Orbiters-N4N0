//! `--only chat-bar` (REQUEST21 3): N4N0's bar in the title strip and the chat it grows into, on made-up data (no real
//! person, friend or picture). On a clock of its own (a frame every 1/60 s), so that the morph's frames are the same
//! every run. Writes, in English: the bar at rest on three pages and at 1280, 1040, 900 and 640 (a recording's Stop
//! beside it) wide; the morph opening and closing frame by frame, each with a contact sheet; the open chat streaming,
//! asking, failing, signed out, assistant off, its action log; and `frame-times.txt`, the morph's frames timed (egui's
//! pass, then tessellation, upload and drawing until the GPU is done). With `--language fr` or `ja`, a few of them in
//! that language, named with its tag.

use super::*;
use crate::state::{AccountState, N4n0Confirmation, N4n0Line, N4n0Who};
use egui::{Key, Modifiers};
use orbiters_n4n0_core::intents::{self, LogEntry, Outcome, Via};

struct Shot<'a> {
    gpu: &'a mut Offscreen,
    ctx: egui::Context,
    app: App,
    ctl: controller::Shared,
    time: f64,
    size: Vec2,
    scale: f32,
}

impl<'a> Shot<'a> {
    fn new(gpu: &'a mut Offscreen, snap: Snapshot, page: Page, size: Vec2, scale: f32) -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let ctl = Controller::new(snap.settings.clone());
        ctl.set_snapshot(snap);
        let backend: Ctl = std::sync::Arc::new(Local::new(ctl.clone()));
        backend.attach(&ctx);
        let mut app = App::offline(backend);
        app.skip_splash();
        app.set_gpu(crate::gpu::Handles::of_offscreen(gpu));
        app.set_page(page, 0.0);
        Self { gpu, ctx, app, ctl, time: 0.0, size, scale }
    }

    fn input(&mut self, events: Vec<Event>) -> RawInput {
        self.time += 1.0 / 60.0;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(self.time), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(self.scale), focused: Some(true), ..Default::default() });
        input.events = events;
        input
    }

    fn frame(&mut self, events: Vec<Event>, capture: bool) -> anyhow::Result<Option<egui::ColorImage>> {
        let input = self.input(events);
        let app = &mut self.app;
        self.gpu.frame(
            &self.ctx,
            input,
            |ui| {
                app.logic_offline(ui.ctx());
                app.draw(ui);
            },
            capture,
        )
    }

    /// One frame timed (no picture): egui's pass, then the drawing until the GPU is done, in milliseconds.
    fn timed(&mut self, events: Vec<Event>) -> anyhow::Result<(f64, f64)> {
        let input = self.input(events);
        let app = &mut self.app;
        let (ui, draw) = self.gpu.frame_timed(&self.ctx, input, |ui| {
            app.logic_offline(ui.ctx());
            app.draw(ui);
        })?;
        Ok((ui.as_secs_f64() * 1000.0, draw.as_secs_f64() * 1000.0))
    }

    fn wait(&mut self, seconds: f64) -> anyhow::Result<()> {
        for _ in 0..(seconds * 60.0).round() as u32 {
            self.frame(Vec::new(), false)?;
        }
        Ok(())
    }

    fn shoot(&mut self, out: &Path, name: &str) -> anyhow::Result<egui::ColorImage> {
        let image = self.frame(Vec::new(), true)?.context("no picture")?;
        save_png(&out.join(format!("{name}.png")), &image)?;
        println!("{}", out.join(format!("{name}.png")).display());
        Ok(image)
    }

    /// The bar takes the keyboard (Ctrl+J) and `text` is typed into it.
    fn type_in(&mut self, text: &str) -> anyhow::Result<()> {
        self.frame(vec![key(Key::J, Modifiers::COMMAND)], false)?;
        self.frame(Vec::new(), false)?;
        self.frame(vec![Event::Text(text.into())], false)?;
        Ok(())
    }

    fn set(&mut self, f: impl FnOnce(&mut Snapshot)) {
        let mut s = (*self.ctl.view()).clone();
        f(&mut s);
        self.ctl.set_snapshot(s);
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

fn line(who: N4n0Who, text: &str, minutes_ago: u64) -> N4n0Line {
    N4n0Line { who, text: text.into(), at_ms: orbiters_n4n0::unix_ms().saturating_sub(minutes_ago * 60_000), streaming: false }
}

/// The words of a scene in the language drawn.
fn said(en: &str, fr: &str, ja: &str) -> String {
    match orbiters_i18n::language() {
        orbiters_i18n::Language::Fr => fr.into(),
        orbiters_i18n::Language::Ja => ja.into(),
        _ => en.into(),
    }
}

/// Signed in (a made-up member), the assistant on, its ears installed, a world playing.
fn ready(base: &Snapshot) -> Snapshot {
    let mut s = base.clone();
    s.settings.n4n0.enabled = true;
    s.settings.n4n0.assistant = true;
    s.account.state = AccountState::SignedIn { username: "orbit_sample".into() };
    s.n4n0.voice.installed = true;
    s.n4n0.quota = Some(orbiters_n4n0_core::assistant::Quota { used: 6, limit: 40, resets_at: None, own_key: false });
    s
}

/// A short conversation: a command done offline, then a question with a longer answer.
fn conversation(s: &mut Snapshot) {
    s.n4n0.lines = vec![
        line(N4n0Who::You, &said("Turn on fast walk", "Active le pas rapide", "早歩きをオンにして"), 6),
        line(N4n0Who::Note, &said("Done: Fast walk on", "Fait : Pas rapide activé", "完了：早歩きオン"), 6),
        line(N4n0Who::You, &said("Any cozy world for a movie night with friends?", "Un monde cosy pour une soirée film entre amis ?", "友達と映画を観られる落ち着いたワールドはある？"), 2),
        line(
            N4n0Who::N4n0,
            &said(
                "Lantern Garden has a big screen, soft seats and a quiet sky: good for a group. Neon Symphony is livelier, with a screen in its lounge. Want me to open a Friends+ instance of one of them?",
                "Lantern Garden a un grand écran, des sièges moelleux et un ciel calme : parfait pour un groupe. Neon Symphony est plus animé, avec un écran dans son salon. Je t’ouvre une instance Friends+ de l’un des deux ?",
                "Lantern Gardenには大きなスクリーンと柔らかい座席、静かな空があって、グループにぴったりです。Neon Symphonyはもっとにぎやかで、ラウンジにスクリーンがあります。どちらかのFriends+インスタンスを開きましょうか？",
            ),
            2,
        ),
    ]
    .into();
}

pub(super) fn shots(gpu: &mut Offscreen, out: &Path, settings: &Settings, scale: f32) -> anyhow::Result<()> {
    let sessions = controller::list_sessions(&settings.sessions_dir);
    let base = ready(&idle_snapshot(settings, &sessions));
    let tag = match orbiters_i18n::language() {
        orbiters_i18n::Language::En => String::new(),
        l => format!("-{}", l.tag()),
    };
    let wide = vec2(1280.0, 820.0);
    let english = tag.is_empty();

    // The bar at rest, on three pages (one with a world's name beside the app's), and at narrower widths.
    let pages: &[(Page, &str)] = if english { &[(Page::Home, "home"), (Page::Worlds, "worlds"), (Page::Settings, "settings")] } else { &[(Page::Home, "home")] };
    for (page, name) in pages {
        let mut snap = base.clone();
        if *page == Page::Worlds {
            snap.vrchat.world.world_name = Some("Lantern Garden".into());
        }
        let mut s = Shot::new(gpu, snap, *page, wide, scale);
        s.wait(1.6)?;
        s.shoot(out, &format!("bar-rest-{name}-1280{tag}"))?;
    }
    if english {
        for width in [1040.0, 900.0] {
            let mut s = Shot::new(gpu, base.clone(), Page::Home, vec2(width, 820.0), scale);
            s.wait(1.6)?;
            s.shoot(out, &format!("bar-rest-home-{width}"))?;
        }
        // The narrowest window while a recording runs: the bar is a button.
        let mut rec = base.clone();
        rec.capture.state = CaptureState::Recording;
        let mut s = Shot::new(gpu, rec, Page::Home, vec2(640.0, 820.0), scale);
        s.wait(1.6)?;
        s.shoot(out, "bar-rest-home-640-recording")?;
        // Focused, then a conversation to go back to (an answer came while it was closed).
        let mut s = Shot::new(gpu, base.clone(), Page::Home, wide, scale);
        s.wait(1.2)?;
        s.frame(vec![key(Key::J, Modifiers::COMMAND)], false)?;
        s.wait(0.3)?;
        s.shoot(out, "bar-focused")?;
        let mut talked = base.clone();
        conversation(&mut talked);
        let mut s = Shot::new(gpu, talked, Page::Home, wide, scale);
        s.wait(1.2)?;
        s.shoot(out, "bar-conversation-waiting")?;
        let mut off = base.clone();
        off.account.state = AccountState::SignedOut;
        let mut s = Shot::new(gpu, off, Page::Home, wide, scale);
        s.wait(1.2)?;
        s.shoot(out, "bar-signed-out")?;
    }

    // The morph, frame by frame: typing grows the bar into the chat; Esc shrinks it back.
    if english {
        let mut talked = base.clone();
        conversation(&mut talked);
        let mut s = Shot::new(gpu, talked, Page::Home, wide, scale);
        s.wait(1.6)?;
        s.frame(vec![key(Key::J, Modifiers::COMMAND)], false)?;
        s.frame(Vec::new(), false)?;
        let mut frames = Vec::new();
        frames.push(s.shoot(out, "morph-open-00")?);
        s.frame(vec![Event::Text("Lantern Garden please".into())], false)?;
        for f in 1..=16 {
            frames.push(s.shoot(out, &format!("morph-open-{f:02}"))?);
        }
        sheet(&frames, &out.join("morph-open-strip.png"))?;
        s.wait(0.5)?;
        s.shoot(out, "chat-open-typed")?;
        s.frame(vec![key(Key::Escape, Modifiers::NONE)], false)?;
        let mut frames = Vec::new();
        for f in 0..=16 {
            frames.push(s.shoot(out, &format!("morph-close-{f:02}"))?);
        }
        sheet(&frames, &out.join("morph-close-strip.png"))?;
        // Reversed half-way: typed, then Esc after 5 frames.
        s.wait(0.5)?;
        s.frame(vec![key(Key::J, Modifiers::COMMAND)], false)?;
        s.frame(vec![Event::Text("!".into())], false)?;
        let mut frames = Vec::new();
        for f in 0..16 {
            if f == 5 {
                s.frame(vec![key(Key::Escape, Modifiers::NONE)], false)?;
            }
            frames.push(s.frame(Vec::new(), true)?.context("no picture")?);
        }
        sheet(&frames, &out.join("morph-reverse-strip.png"))?;
        println!("{}", out.join("morph-reverse-strip.png").display());
    }

    // The open chat in its states.
    let open = |gpu: &mut Offscreen, snap: Snapshot, name: &str, wait: f64, staged: &dyn Fn(&mut Shot<'_>)| -> anyhow::Result<()> {
        let mut s = Shot::new(gpu, snap, Page::Home, wide, scale);
        s.wait(1.2)?;
        s.frame(vec![key(Key::J, Modifiers::COMMAND)], false)?;
        staged(&mut s);
        s.wait(wait)?;
        s.shoot(out, &format!("{name}{tag}"))?;
        Ok(())
    };
    let mut streaming = base.clone();
    conversation(&mut streaming);
    if let Some(last) = streaming.n4n0.lines.last_mut() {
        let cut = last.text.char_indices().nth(last.text.chars().count() * 3 / 5).map_or(last.text.len(), |(i, _)| i);
        last.text.truncate(cut);
        last.streaming = true;
    }
    streaming.n4n0.thinking = true;
    open(gpu, streaming, "chat-streaming", 0.8, &|_| {})?;

    let question = said("N4N0 will do this if you say yes: Boop \"Mira\".", "N4N0 le fera si tu dis oui : Boop à « Mira ».", "「はい」と言えばN4N0が実行します：「Mira」にBoop。");
    let mut asking = base.clone();
    asking.n4n0.lines = vec![
        line(N4n0Who::You, &said("Boop Mira", "Boop Mira", "MiraにBoopして"), 1),
        line(N4n0Who::N4n0, &said("Sure, Mira is in your world. One yes from you and I boop her.", "D’accord, Mira est dans ton monde. Un oui de ta part et je la boop.", "了解、Miraは同じワールドにいます。「はい」と言えばBoopします。"), 1),
        line(N4n0Who::Note, &question, 1),
    ]
    .into();
    asking.n4n0.confirmations = vec![N4n0Confirmation { id: "1".into(), title: String::new(), summary: question.clone(), approve_url: format!("{}1", orbiters_n4n0::status::LOCAL_APPROVAL) }];
    open(gpu, asking, "chat-confirm", 0.6, &|_| {})?;

    if english {
        let mut failed = base.clone();
        failed.n4n0.lines = vec![
            line(N4n0Who::You, "Save this video", 1),
            line(N4n0Who::Error, "Can't reach Orbiters: check your connection, then ask again.", 0),
        ]
        .into();
        open(gpu, failed, "chat-error", 0.6, &|_| {})?;

        let mut signed_out = base.clone();
        signed_out.account.state = AccountState::SignedOut;
        open(gpu, signed_out, "chat-signed-out", 0.6, &|_| {})?;

        let mut off = base.clone();
        off.settings.n4n0.assistant = false;
        open(gpu, off, "chat-assistant-off", 0.6, &|_| {})?;

        open(gpu, base.clone(), "chat-empty", 0.6, &|_| {})?;

        let mut thinking = base.clone();
        thinking.n4n0.lines = vec![line(N4n0Who::You, "What's the weather like in Lantern Garden?", 0)].into();
        thinking.n4n0.thinking = true;
        thinking.n4n0.assistant.doing = Some("Searching worlds for \"Lantern Garden\"".into());
        open(gpu, thinking, "chat-thinking", 0.6, &|_| {})?;

        // What N4N0 did, from the chat's head.
        let mut logged = base.clone();
        conversation(&mut logged);
        let now = orbiters_n4n0::unix_ms();
        let entry = |intent: &intents::Intent, outcome: Outcome, via: Via, minutes: u64| LogEntry {
            at_ms: now - minutes * 60_000,
            intent: intent.name().into(),
            said: intent.says(),
            risk: intent.risk(),
            outcome,
            why: None,
            via,
        };
        logged.n4n0.assistant.log = vec![
            entry(&intents::Intent::FastWalk { on: true }, Outcome::Done, Via::Quick, 6),
            entry(&intents::Intent::Boop { friend: Some("Mira".into()) }, Outcome::Declined, Via::Model, 4),
            entry(&intents::Intent::Boop { friend: Some("Mira".into()) }, Outcome::Done, Via::Model, 3),
        ];
        open(gpu, logged, "chat-log", 0.6, &|s| s.app.chat_mut().show_log(true))?;
    }

    // A long conversation, scrolled to its end (the one scroll region).
    if english {
        let mut long = base.clone();
        conversation(&mut long);
        let more: Vec<N4n0Line> = (0..6)
            .flat_map(|i| {
                [
                    line(N4n0Who::You, &format!("And a world for game night number {}?", i + 1), 1),
                    line(N4n0Who::N4n0, "Try Lantern Garden's games room: a pool table, cards and a board for drawing together, with seats for twelve.", 1),
                ]
            })
            .collect();
        long.n4n0.lines.extend(more);
        open(gpu, long, "chat-long", 0.8, &|_| {})?;
    }

    // Frame times of the morph at this scale: opening and closing five times, every frame timed.
    if english {
        let mut talked = base.clone();
        conversation(&mut talked);
        let mut s = Shot::new(gpu, talked, Page::Home, wide, scale);
        s.wait(1.6)?;
        let mut rest = Vec::new();
        for _ in 0..30 {
            rest.push(s.timed(Vec::new())?);
        }
        let mut morph = Vec::new();
        let mut open_rest = Vec::new();
        for _ in 0..5 {
            // Ctrl+J with a conversation under way: the chat grows at once.
            for f in 0..24 {
                morph.push(s.timed(if f == 0 { vec![key(Key::J, Modifiers::COMMAND)] } else { Vec::new() })?);
            }
            for _ in 0..10 {
                open_rest.push(s.timed(Vec::new())?);
            }
            for f in 0..24 {
                morph.push(s.timed(if f == 0 { vec![key(Key::Escape, Modifiers::NONE)] } else { Vec::new() })?);
            }
            s.wait(0.3)?;
        }
        let stats = |name: &str, v: &[(f64, f64)]| -> String {
            let total: Vec<f64> = v.iter().map(|(a, b)| a + b).collect();
            let mut sorted = total.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let pick = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
            let mean = |f: &dyn Fn(&(f64, f64)) -> f64| v.iter().map(f).sum::<f64>() / v.len() as f64;
            format!(
                "{name}: {} frames; egui pass {:.2} ms mean; tessellate+upload+draw until the GPU is done {:.2} ms mean; total median {:.2} ms, 95th {:.2} ms, max {:.2} ms\n",
                v.len(),
                mean(&|x| x.0),
                mean(&|x| x.1),
                pick(0.5),
                pick(0.95),
                pick(1.0)
            )
        };
        let adapter = gpu_name(&s);
        let mut text = format!("Frame times, {} x {} points at scale {scale} ({} x {} pixels), on {adapter} (offscreen, debug or release as built)\n", wide.x, wide.y, wide.x * scale, wide.y * scale);
        text += &stats("bar at rest, chat closed", &rest);
        text += &stats("morph frames (opening and closing, 5 times each, 24 frames each way)", &morph);
        text += &stats("chat open at rest (backdrop composited, page copy cached)", &open_rest);
        std::fs::write(out.join("frame-times.txt"), &text)?;
        print!("{text}");
        s.ctl.shutdown();
    }
    Ok(())
}

fn gpu_name(s: &Shot<'_>) -> String {
    s.gpu.adapter().get_info().name
}

/// A contact sheet of `frames`, four to a row, each a quarter of its size (box-filtered).
fn sheet(frames: &[egui::ColorImage], path: &Path) -> anyhow::Result<()> {
    let Some(first) = frames.first() else { return Ok(()) };
    let k = 4usize;
    let (w, h) = (first.width() / k, first.height() / k);
    let cols = 4usize;
    let rows = frames.len().div_ceil(cols);
    let gap = 6usize;
    let (sw, sh) = (cols * w + (cols + 1) * gap, rows * h + (rows + 1) * gap);
    let mut sheet = egui::ColorImage::filled([sw, sh], egui::Color32::from_rgb(40, 40, 46));
    for (i, f) in frames.iter().enumerate() {
        let (ox, oy) = (gap + (i % cols) * (w + gap), gap + (i / cols) * (h + gap));
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 3];
                for dy in 0..k {
                    for dx in 0..k {
                        let c = f.pixels[(y * k + dy) * f.width() + x * k + dx];
                        acc[0] += c.r() as u32;
                        acc[1] += c.g() as u32;
                        acc[2] += c.b() as u32;
                    }
                }
                let n = (k * k) as u32;
                sheet.pixels[(oy + y) * sw + ox + x] = egui::Color32::from_rgb((acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8);
            }
        }
    }
    save_png(path, &sheet)
}
