//! How the app looks (Settings, Appearance): what the settings say, taken up in what is drawn next, in the window and in
//! the overlay alike, and the window's background with the rules of when it may move.
//!
//! In the headset and beside VRChat's menus: the colours and the font, never the background. Those panels keep their own
//! ink (they lie on VRChat's menus), and the accent they follow is the app's unless one was picked for them.

use egui::{Color32, Id, Painter, Rect};
use orbiters_core::appearance::{AppearanceSettings, Background, BackgroundColors, Tint, ACCENT};
use orbiters_core::settings::{OrbitSettings, Settings};
use orbiters_ui::background::{self, Backdrop, Laid, Scene, Time};
use orbiters_ui::look::{self, Look, Oklch};
use orbiters_ui::{anim, faces, theme};

pub fn tint(t: Tint) -> look::Tint {
    match t {
        Tint::Zinc => look::Tint::Zinc,
        Tint::Graphite => look::Tint::Graphite,
        Tint::Slate => look::Tint::Slate,
        Tint::Moss => look::Tint::Moss,
        Tint::Stone => look::Tint::Stone,
        Tint::Plum => look::Tint::Plum,
        Tint::Black => look::Tint::Black,
    }
}

pub const TINTS: [Tint; 7] = [Tint::Zinc, Tint::Graphite, Tint::Slate, Tint::Moss, Tint::Stone, Tint::Plum, Tint::Black];

fn color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The colours chosen, before they are made readable ([`look::passing`]).
pub fn look(a: &AppearanceSettings) -> Look {
    Look { tint: tint(a.tint), accent: color(a.accent) }
}

/// A look chosen whole: surfaces, accent and what a background is drawn in.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Its name in the person's language is `orbiters_i18n::tr` of it.
    pub name: &'static str,
    pub tint: Tint,
    pub accent: [u8; 3],
    pub colors: BackgroundColors,
}

impl Palette {
    pub fn look(&self) -> Look {
        Look { tint: tint(self.tint), accent: color(self.accent) }
    }

    pub fn is(&self, a: &AppearanceSettings) -> bool {
        a.tint == self.tint && a.accent == self.accent
    }

    pub fn apply(&self, a: &mut AppearanceSettings) {
        (a.tint, a.accent, a.background_colors) = (self.tint, self.accent, self.colors);
    }
}

use orbiters_i18n::mark;

/// Orbiters' own first; then the website's four palettes for profiles (their signature colour), then the app's.
pub static PALETTES: [Palette; 8] = [
    Palette { name: "Orbiters", tint: Tint::Zinc, accent: ACCENT, colors: BackgroundColors::Own },
    Palette { name: mark("Aurora"), tint: Tint::Plum, accent: [0xb7, 0x8a, 0xff], colors: BackgroundColors::Aurora },
    Palette { name: mark("Ember"), tint: Tint::Stone, accent: [0xff, 0xab, 0x76], colors: BackgroundColors::Ember },
    Palette { name: mark("Lagoon"), tint: Tint::Slate, accent: [0x64, 0xde, 0xc3], colors: BackgroundColors::Lagoon },
    Palette { name: mark("Daydream"), tint: Tint::Graphite, accent: [0xed, 0x91, 0xbe], colors: BackgroundColors::Daydream },
    Palette { name: mark("Midnight"), tint: Tint::Black, accent: [0x6f, 0xa8, 0xff], colors: BackgroundColors::Accent },
    Palette { name: mark("Meadow"), tint: Tint::Moss, accent: [0xa8, 0xd8, 0x6c], colors: BackgroundColors::Accent },
    Palette { name: mark("Moonlight"), tint: Tint::Graphite, accent: [0xd9, 0xdc, 0xe3], colors: BackgroundColors::Accent },
];

/// Accents to press: Orbiters green, then around the wheel, then none.
pub const ACCENTS: [[u8; 3]; 10] = [
    ACCENT,
    [0x64, 0xde, 0xc3],
    [0x48, 0xc8, 0xff],
    [0x6f, 0xa8, 0xff],
    [0x9b, 0x9c, 0xff],
    [0xb7, 0x8a, 0xff],
    [0xed, 0x91, 0xbe],
    [0xff, 0xab, 0x76],
    [0xa8, 0xd8, 0x6c],
    [0xd9, 0xdc, 0xe3],
];

pub fn scene(b: Background) -> Scene {
    match b {
        Background::None => Scene::None,
        Background::AeroShards => Scene::AeroShards,
        Background::Grainient => Scene::Grainient,
        Background::GradientWaves => Scene::GradientWaves,
        Background::PatternWaves => Scene::PatternWaves,
        Background::GradientAscii => Scene::GradientAscii,
        Background::LeafShadow => Scene::WindowLight,
    }
}

pub const BACKGROUNDS: [Background; 7] =
    [Background::None, Background::AeroShards, Background::Grainient, Background::GradientWaves, Background::PatternWaves, Background::GradientAscii, Background::LeafShadow];

/// A background in the colours `colors` gives it under the app's accent.
pub fn backdrop_in(scene: Scene, colors: BackgroundColors, accent: [u8; 3]) -> Backdrop {
    let hex = |v: u32| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
    // The website's palettes (`AppearanceControls.jsx`): their three colours, and the signature colour.
    let palette = |primary: u32, c: [u32; 3]| Backdrop { scene, colors: c.map(hex), primary: hex(primary) };
    match colors {
        BackgroundColors::Own => Backdrop::of(scene),
        BackgroundColors::Aurora => palette(0xb78aff, [0x171420, 0x643c91, 0x257a84]),
        BackgroundColors::Ember => palette(0xffab76, [0x201019, 0xb95349, 0xad793f]),
        BackgroundColors::Lagoon => palette(0x64dec3, [0x081b27, 0x246779, 0x457b68]),
        BackgroundColors::Daydream => palette(0xed91be, [0x241527, 0x976080, 0x565aa4]),
        BackgroundColors::Accent => {
            let primary = color(accent);
            let k = Oklch::of(primary);
            let of = |l: f32, most: f32| Oklch { l, c: k.c.min(most), h: k.h }.color();
            // As the website's own colours step: near black, a muted middle, the colour itself.
            let colors = if scene == Scene::WindowLight { [of(0.13, 0.02), of(0.68, 0.15), of(0.25, 0.08)] } else { [of(0.18, 0.025), of(0.58, 0.12), of(0.64, 0.2)] };
            Backdrop { scene, colors, primary }
        }
    }
}

pub fn backdrop(a: &AppearanceSettings) -> Backdrop {
    backdrop_in(scene(a.background), a.background_colors, a.accent)
}

/// Takes up the colours and the font `settings` ask for in this process; `woken` once a font being read is there.
pub fn follow(settings: &Settings, woken: impl FnOnce() + Send + 'static) {
    theme::set_look(&look(&settings.appearance));
    faces::choose(&settings.appearance.font, woken);
}

/// What anything drawn now is drawn in: something kept is out of date once this differs.
pub fn drawn_in() -> (u32, u32) {
    (theme::generation(), faces::generation())
}

/// The menus beside VRChat's, in the app's accent unless one was picked for them.
pub fn orbit(settings: &Settings) -> OrbitSettings {
    let mut orbit = settings.orbit;
    if !orbit.has_own_accent() {
        let c = look::passing(color(settings.appearance.accent), tint(settings.appearance.tint));
        orbit.accent = [c.r(), c.g(), c.b()];
    }
    orbit
}

/// Whether the system asks for less motion (Windows' Animation effects off).
pub fn system_reduces_motion() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static READ: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    if cfg!(test) {
        return false;
    }
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    match *read {
        Some((at, reduced)) if at.elapsed() < Duration::from_secs(5) => reduced,
        _ => {
            let reduced = read_system_motion();
            *read = Some((Instant::now(), reduced));
            reduced
        }
    }
}

#[cfg(windows)]
fn read_system_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let mut animates = windows::core::BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through the pointer given, which lives past the call.
    let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some((&raw mut animates).cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    read.is_ok() && !animates.as_bool()
}

#[cfg(not(windows))]
fn read_system_motion() -> bool {
    false
}

/// What decides whether a background moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// The settings' switches.
    pub animate: bool,
    pub while_playing: bool,
    /// Reduced motion, the app's or the system's.
    pub reduced: bool,
    /// The window is in front, to be seen.
    pub focused: bool,
    /// VRChat or SteamVR runs.
    pub playing: bool,
    /// Nobody touched the window for a while ([`anim::REST_AFTER`]).
    pub resting: bool,
}

impl Motion {
    /// It never moves: it shows its still frame.
    pub fn still(&self) -> bool {
        self.reduced || !self.animate
    }

    pub fn moves(&self) -> bool {
        !self.still() && self.focused && !self.resting && (!self.playing || self.while_playing)
    }
}

/// How long one background takes to come over another (the website's).
const CROSSFADE: f64 = 0.6;
/// How fast a scene gets going or comes to rest, instead of stopping dead.
const EASE: f64 = 0.35;
/// A scene that never got to move shows itself as it is by then: some draw themselves in over their first seconds.
const SHOWN_BY: f64 = 3.0;

/// The window's background: which scene shows and which it came over, and how far each has moved.
#[derive(Default)]
pub struct Wallpaper {
    shown: Option<Layer>,
    leaving: Option<Layer>,
    count: u64,
    /// When it was last painted, and how fast it moves (0 at rest, 1).
    last: Option<f64>,
    speed: f64,
    /// What the settings say it is, worked out when they or the colours change: the settings, the colours, the background, its veil.
    worked_out: Option<(AppearanceSettings, u32, Backdrop, f32)>,
}

struct Layer {
    backdrop: Backdrop,
    veil: f32,
    id: Id,
    since: f64,
    moved: f64,
}

impl Wallpaper {
    /// The background the settings ask for and its veil.
    fn wanted(&mut self, a: &AppearanceSettings) -> (Backdrop, f32) {
        let drawn_in = theme::generation();
        match &self.worked_out {
            Some((of, at, b, veil)) if of == a && *at == drawn_in => (*b, *veil),
            _ => {
                let b = backdrop(a);
                let veil = b.veil();
                self.worked_out = Some((a.clone(), drawn_in, b, veil));
                (b, veil)
            }
        }
    }

    /// It hides what is under it: nothing else need be drawn behind the pages.
    pub fn covers(&self, a: &AppearanceSettings) -> bool {
        a.background != Background::None && self.leaving.is_none() && self.shown.as_ref().is_some_and(|s| s.backdrop.scene == scene(a.background))
    }

    /// [`Wallpaper::paint_as`] for the settings.
    pub fn paint(&mut self, painter: &Painter, rect: Rect, a: &AppearanceSettings, motion: Motion) -> bool {
        let (wanted, veil) = self.wanted(a);
        self.paint_as(painter, rect, wanted, veil, motion)
    }

    /// Paints it over `rect` (the whole window, over its glow). Says whether it moves now.
    fn paint_as(&mut self, painter: &Painter, rect: Rect, wanted: Backdrop, veil: f32, motion: Motion) -> bool {
        let ctx = painter.ctx();
        let now = anim::time(ctx);
        let dt = self.last.map_or(0.0, |l| (now - l).clamp(0.0, 0.1));
        self.last = Some(now);
        theme::set_over_background(wanted.scene != Scene::None);

        let fading = if motion.reduced { 0.0 } else { CROSSFADE };
        if self.shown.as_ref().is_none_or(|s| s.backdrop.scene != wanted.scene) {
            self.count += 1;
            // The first is there at once, with the window; the others come over the one before.
            let since = if self.shown.is_none() { now - CROSSFADE } else { now };
            let moved = if motion.moves() { 0.0 } else { SHOWN_BY };
            let next = Layer { backdrop: wanted, veil, id: Id::unique(("orbiters-wallpaper", self.count % 2)), since, moved };
            self.leaving = self.shown.replace(next);
        }
        let Some(shown) = self.shown.as_mut() else { return false };
        // Its colours may change under way (a palette pressed): the scene goes on.
        (shown.backdrop, shown.veil) = (wanted, veil);

        // It eases to rest and off again.
        let moves = motion.moves() && wanted.scene != Scene::None;
        let goal = if moves { 1.0 } else { 0.0 };
        self.speed += (goal - self.speed) * (1.0 - (-dt / EASE).exp());
        if (self.speed - goal).abs() < 0.02 {
            self.speed = goal;
        }
        // Away from the window's eyes it stops at once.
        if !motion.focused || motion.still() || (motion.playing && !motion.while_playing) {
            self.speed = 0.0;
        }
        shown.moved += dt * self.speed;
        if self.speed == 0.0 {
            shown.moved = shown.moved.max(SHOWN_BY);
        }
        let running = self.speed > 0.0;

        let fade = if fading <= 0.0 { 1.0 } else { ((now - shown.since) / fading).clamp(0.0, 1.0) as f32 };
        let time = |l: &Layer| if motion.still() { Time::STILL } else { Time { moved: l.moved, still: false } };
        if let Some(l) = &self.leaving {
            background::paint(painter, l.id, rect, &l.backdrop, time(l), Laid { fade: 1.0 - anim::ease::out_cubic(fade), veil: l.veil, ..Laid::PAGE });
        }
        if fade >= 1.0 {
            self.leaving = None;
        }
        let laid = Laid { fade: anim::ease::out_cubic(fade), veil, ..Laid::PAGE };
        background::paint(painter, shown.id, rect, &wanted, time(shown), laid);
        if wanted.scene == Scene::None && self.leaving.is_none() {
            background::tidy(painter, rect);
        }
        if self.leaving.is_some() && motion.focused {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(1.0 / 60.0));
        } else if running {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(1.0 / wanted.scene.fps()));
        }
        running
    }

    /// Screenshots and the bench: it starts `seconds` in.
    pub fn at(&mut self, seconds: f64) {
        if let Some(s) = self.shown.as_mut() {
            s.moved = seconds;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, RawInput};
    use std::time::Duration;

    const MOVING: Motion = Motion { animate: true, while_playing: false, reduced: false, focused: true, playing: false, resting: false };

    #[test]
    fn a_background_moves_only_where_it_is_looked_at_and_costs_a_game_nothing() {
        assert!(MOVING.moves());
        for still in [
            Motion { focused: false, ..MOVING },
            Motion { resting: true, ..MOVING },
            Motion { playing: true, ..MOVING },
            Motion { reduced: true, ..MOVING },
            Motion { animate: false, ..MOVING },
            Motion { playing: true, while_playing: true, focused: false, ..MOVING },
            Motion { playing: true, while_playing: true, reduced: true, ..MOVING },
        ] {
            assert!(!still.moves(), "{still:?}");
        }
        assert!(Motion { playing: true, while_playing: true, ..MOVING }.moves());
    }

    #[test]
    fn every_palette_is_readable_as_it_is_and_orbiters_is_the_look_there_was() {
        assert_eq!(look(&AppearanceSettings::default()), Look::ORBITERS);
        assert!(PALETTES[0].is(&AppearanceSettings::default()));
        for p in &PALETTES {
            assert_eq!(look::passing(color(p.accent), tint(p.tint)), color(p.accent), "{}: its accent is used as it is", p.name);
            let mut a = AppearanceSettings::default();
            p.apply(&mut a);
            assert!(p.is(&a));
        }
        for a in ACCENTS {
            for t in TINTS {
                assert_eq!(look::passing(color(a), tint(t)), color(a), "{a:?} on {t:?}");
            }
        }
        let distinct: std::collections::HashSet<_> = PALETTES.iter().map(|p| (p.look(), p.name)).collect();
        assert_eq!(distinct.len(), PALETTES.len());
    }

    #[test]
    fn the_menus_beside_vrchat_s_follow_the_accent_until_they_have_their_own() {
        let mut s = Settings::default();
        assert_eq!(orbit(&s), s.orbit, "as they were");
        s.appearance.accent = [0xb7, 0x8a, 0xff];
        assert_eq!(orbit(&s).accent, [0xb7, 0x8a, 0xff]);
        // A dark accent is theirs as it is drawn in the app.
        s.appearance.accent = [0x10, 0x10, 0x60];
        assert!(look::contrast(color(orbit(&s).accent), theme::base::INK_750) >= look::AA);
        // One picked for them stays, the first green too.
        s.orbit.accent_own = true;
        assert_eq!(orbit(&s).accent, OrbitSettings::FIRST_ACCENT);
        s.orbit.accent_own = false;
        s.orbit.accent = [1, 2, 3];
        assert_eq!(orbit(&s).accent, [1, 2, 3], "picked before they could follow");
    }

    fn frame(ctx: &egui::Context, t: f64, focused: bool, mut f: impl FnMut(&Painter) -> bool) -> (bool, Duration) {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let mut input = RawInput { time: Some(t), screen_rect: Some(rect), focused, ..Default::default() };
        input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { focused: Some(focused), ..Default::default() });
        let mut running = false;
        let mut out = ctx.run_ui(input, |ui| running = f(ui.painter()));
        out.textures_delta.clear();
        (running, out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay))
    }

    /// The whole contract of the frames it asks for: at most thirty a second while it moves, none once it may not.
    #[test]
    fn it_asks_for_thirty_frames_a_second_while_it_moves_and_for_none_at_rest() {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let mut w = Wallpaper::default();
        let b = Backdrop::of(Scene::AeroShards);
        let mut t = 0.0;
        let mut go = |w: &mut Wallpaper, motion: Motion, seconds: f64| {
            let mut last = (false, Duration::MAX);
            for _ in 0..(seconds * 60.0) as usize {
                t += 1.0 / 60.0;
                last = frame(&ctx, t, motion.focused, |p| w.paint_as(p, rect, b, 0.0, motion));
            }
            last
        };
        let (running, delay) = go(&mut w, MOVING, 2.0);
        assert!(running && delay <= Duration::from_millis(34) && delay >= Duration::from_millis(20), "{delay:?}");
        assert!(Scene::ALL.iter().all(|s| s.fps() <= 30.0));
        let moved = w.shown.as_ref().unwrap().moved;
        assert!((1.0..2.05).contains(&moved), "{moved}");
        // It eases to rest, then asks for nothing.
        let (running, delay) = go(&mut w, Motion { resting: true, ..MOVING }, 4.0);
        assert!(!running && delay > Duration::from_secs(1), "{delay:?}");
        for stop in [Motion { focused: false, ..MOVING }, Motion { playing: true, ..MOVING }, Motion { reduced: true, ..MOVING }] {
            go(&mut w, MOVING, 1.0);
            let before = w.shown.as_ref().unwrap().moved;
            let (running, delay) = go(&mut w, stop, 0.5);
            assert!(!running && delay > Duration::from_secs(1), "{stop:?} {delay:?}");
            assert_eq!(w.shown.as_ref().unwrap().moved, before, "{stop:?}: it stands where it was");
        }
        // No background asks for nothing either.
        let (running, delay) = frame(&ctx, 100.0, true, |p| w.paint_as(p, rect, Backdrop::of(Scene::None), 0.0, MOVING));
        assert!(!running, "{delay:?}");
        for k in 0..60 {
            frame(&ctx, 100.0 + k as f64 / 30.0, true, |p| w.paint_as(p, rect, Backdrop::of(Scene::None), 0.0, MOVING));
        }
        let (running, delay) = frame(&ctx, 103.0, true, |p| w.paint_as(p, rect, Backdrop::of(Scene::None), 0.0, MOVING));
        assert!(!running && delay > Duration::from_secs(1), "{delay:?}");
        theme::set_over_background(false);
    }
}
