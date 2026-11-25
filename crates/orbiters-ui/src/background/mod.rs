//! The backgrounds a person can lay behind the window's pages (Settings, Appearance): the ones orbiters.cc offers for
//! profiles (`frontend/src/components/personalization`), the same scenes with the same parameters, motion and timing, in
//! WGSL ([`scenes`]), laid behind the page as the website lays them (`composite.wgsl`).
//!
//! A scene is drawn into a texture of its own, and only when its time moves on: the interface can draw as often as it
//! likes over the same picture, and a scene that stands still costs one textured triangle a frame. Its time is the
//! caller's ([`Time`]), and so is asking for frames.

mod made;
mod scenes;

use crate::look;
use egui::{Color32, Id, Painter, Rect, Vec2};
use egui_wgpu::wgpu;
use scenes::{Onto, Pipelines, Scenic};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

/// The website's backgrounds, in its order (`backgroundCatalog.js`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scene {
    /// Orbiters' own glow, as before there were others.
    #[default]
    None,
    AeroShards,
    Grainient,
    GradientWaves,
    PatternWaves,
    GradientAscii,
    WindowLight,
}

impl Scene {
    pub const ALL: [Scene; 7] = [Scene::None, Scene::AeroShards, Scene::Grainient, Scene::GradientWaves, Scene::PatternWaves, Scene::GradientAscii, Scene::WindowLight];

    /// The website's id.
    pub fn id(self) -> &'static str {
        match self {
            Scene::None => "none",
            Scene::AeroShards => "aero-shards",
            Scene::Grainient => "grainient",
            Scene::GradientWaves => "gradient-waves",
            Scene::PatternWaves => "pattern-waves",
            Scene::GradientAscii => "gradient-ascii",
            Scene::WindowLight => "leaf-shadow",
        }
    }

    pub fn of_id(id: &str) -> Option<Scene> {
        Scene::ALL.into_iter().find(|s| s.id() == id)
    }

    /// How present it is behind a page (`personalization.css`).
    fn opacity(self) -> f32 {
        match self {
            Scene::WindowLight => 0.58,
            Scene::GradientAscii => 0.5,
            _ => 0.38,
        }
    }

    /// Its colours unless others are chosen: the website's defaults (Window Light has its own).
    pub fn colors(self) -> [Color32; 3] {
        match self {
            Scene::WindowLight => [Color32::from_rgb(0x07, 0x06, 0x0b), Color32::from_rgb(0x8f, 0x7f, 0xf0), Color32::from_rgb(0x24, 0x1a, 0x45)],
            _ => [Color32::from_rgb(0x12, 0x0f, 0x17), Color32::from_rgb(0x89, 0x6a, 0xbd), Color32::from_rgb(0xa8, 0x55, 0xf7)],
        }
    }
}

/// The website's signature colour unless another is chosen (Gradient ASCII's accent).
pub const PRIMARY: Color32 = Color32::from_rgb(0x33, 0x8e, 0xf7);

/// A background as chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Backdrop {
    pub scene: Scene,
    pub colors: [Color32; 3],
    pub primary: Color32,
}

impl Backdrop {
    /// `scene` in the website's own colours.
    pub fn of(scene: Scene) -> Self {
        Self { scene, colors: scene.colors(), primary: PRIMARY }
    }

    /// The brightest it can be over the width of a word: what [`Backdrop::veil`] reckons with.
    fn peak(&self) -> Color32 {
        let [a, b, c] = self.colors;
        let brightest = |list: &[Color32]| list.iter().copied().max_by(|x, y| look::luminance(*x).total_cmp(&look::luminance(*y))).unwrap_or(a);
        let lit = |x: Color32, t: f32| crate::theme::mix(x, Color32::WHITE, t);
        match self.scene {
            Scene::None => Color32::BLACK,
            // Its core: the light colour most of the way to white, under the glyphs' own light.
            Scene::GradientAscii => brightest(&[lit(c, 0.85), a, b]),
            // Lit facets and their bloom.
            Scene::AeroShards => brightest(&[lit(c, 0.4), a, b]),
            // Its contrast pushes the brighter colours up.
            Scene::Grainient => brightest(&[lit(a, 0.15), lit(b, 0.15), lit(c, 0.15)]),
            Scene::WindowLight => lit(b, 0.2),
            Scene::GradientWaves | Scene::PatternWaves => brightest(&[a, b, c]),
        }
    }

    /// What the words of the pages in `tokens` need over it: how dark a veil, and the brightest they then lie on. The scene is
    /// left as bright as can be: the quieter words get lighter first ([`look::Tokens::over`], up to the second text step), and only
    /// what that step cannot be read on is darkened, with as little black as brings it to AA, after the website's own
    /// wash. The same rule for every scene, whatever its colours.
    pub fn under(&self, tokens: &look::Tokens) -> (f32, Color32) {
        if self.scene == Scene::None {
            return (0.0, tokens.ink_950);
        }
        let page = tokens.ink_950;
        // The wash is a quarter at the top of the window: least where the page's headings are.
        let washed = crate::theme::mix(self.peak(), page, 0.25);
        let seen = crate::theme::mix(page, washed, self.scene.opacity());
        let veil = look::veil_for(tokens.fg_2, seen, look::AA);
        (veil, look::over(crate::theme::black(veil), seen))
    }
}

/// Where a scene's motion is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Time {
    /// Seconds it moved for since it was shown.
    pub moved: f64,
    /// Its still frame: what it shows with reduced motion (the website's, where it has one).
    pub still: bool,
}

impl Time {
    pub const STILL: Time = Time { moved: 0.0, still: true };
}

/// How it is laid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Laid {
    /// 0 to 1, over the background's own opacity.
    pub fade: f32,
    /// [`Backdrop::veil`], or none.
    pub veil: f32,
    /// A picture of it alone: at full opacity, without the wash (the picker's tiles).
    pub picture: bool,
    /// The scene's size in points when it is not where it is painted (a picture of a page-sized one, made smaller).
    pub size: Option<Vec2>,
    /// Corner radius.
    pub radius: f32,
}

impl Laid {
    pub const PAGE: Laid = Laid { fade: 1.0, veil: 0.0, picture: false, size: None, radius: 0.0 };
}

impl Scene {
    /// How often it is drawn a second while it moves, at most: the slow ones look the same at a third fewer frames.
    pub fn fps(self) -> f64 {
        match self {
            Scene::AeroShards => 30.0,
            _ => 20.0,
        }
    }
}

/// Paints `backdrop` over `rect`. `id` names its textures: keep it stable. Nothing is drawn for [`Scene::None`].
pub fn paint(painter: &Painter, id: Id, rect: Rect, backdrop: &Backdrop, time: Time, laid: Laid) {
    if backdrop.scene == Scene::None || rect.width() < 1.0 || rect.height() < 1.0 || laid.fade <= 0.002 {
        return;
    }
    let ctx = painter.ctx();
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(Id::unique(PAINTED), pass));
    let frame = if time.still { -1 } else { (time.moved * backdrop.scene.fps()).floor() as i64 };
    let of = |c: Color32| [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0];
    let [a, b, c] = backdrop.colors;
    let over = Over {
        frame: [rect.width(), rect.height(), if laid.picture { laid.fade } else { backdrop.scene.opacity() * laid.fade }, laid.veil.clamp(0.0, 1.0) * laid.fade],
        extra: [0.0, ctx.pixels_per_point(), 1.0, if laid.picture { 1.0 } else { 0.0 }],
        a: of(a),
        b: of(b),
        c: of(c),
        page: { let [r, g, b, _] = of(crate::theme::ink_950()); [r, g, b, laid.radius] },
    };
    let size = laid.size.unwrap_or(rect.size());
    let callback = BackgroundCallback { id, backdrop: *backdrop, time, frame, css: [size.x, size.y], ppp: ctx.pixels_per_point(), over, pass, ctx: ctx.clone() };
    painter.add(egui_wgpu::Callback::new_paint_callback(rect, callback));
}

/// Gives back what backgrounds no longer painted hold, when none is: nothing else would look after the last of them. To call
/// each frame nothing else here is painted; it costs one look at egui's memory.
pub fn tidy(painter: &Painter, rect: Rect) {
    let ctx = painter.ctx();
    let pass = ctx.cumulative_pass_nr();
    let due = ctx.data_mut(|d| {
        let due = d.get_temp::<u64>(Id::unique(PAINTED)).is_some_and(|last| pass >= last + FORGET_AFTER);
        if due {
            d.remove_temp::<u64>(Id::unique(PAINTED));
        }
        due
    });
    if due {
        painter.add(egui_wgpu::Callback::new_paint_callback(rect, Tidy { pass }));
    }
}

/// Where a context notes the last pass a background was painted in.
const PAINTED: &str = "orbiters-background-painted";

struct Tidy {
    pass: u64,
}

impl egui_wgpu::CallbackTrait for Tidy {
    fn prepare(&self, _: &wgpu::Device, _: &wgpu::Queue, _: &egui_wgpu::ScreenDescriptor, _: &mut wgpu::CommandEncoder, resources: &mut egui_wgpu::CallbackResources) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<BackgroundResources>() {
            res.shown.retain(|_, s| s.pass + FORGET_AFTER > self.pass);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, _pass: &mut wgpu::RenderPass<'static>, _resources: &egui_wgpu::CallbackResources) {}
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Over {
    frame: [f32; 4],
    extra: [f32; 4],
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    page: [f32; 4],
}

/// GPU objects of the backgrounds, kept in egui-wgpu's callback resources: pipelines made the first time a scene shows,
/// textures while it does.
pub struct BackgroundResources {
    pipelines: Pipelines,
    sampler: wgpu::Sampler,
    linear_target: bool,
    shown: HashMap<Id, Shown>,
    /// Scenes drawn since the start (tests, the bench).
    drawn: u64,
}

struct Shown {
    scene: Scenic,
    /// The backdrop and the frame of its time in the texture.
    drawn: Option<(Backdrop, i64)>,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    pass: u64,
}

/// Passes of the context after which what was not painted is given back.
const FORGET_AFTER: u64 = 4;

fn sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

/// Room for the backgrounds of render targets of `format` (call once per renderer): nothing is compiled until one shows.
pub fn install(device: &wgpu::Device, format: wgpu::TextureFormat, msaa_samples: u32, resources: &mut egui_wgpu::CallbackResources) {
    let pipelines = Pipelines::new(Onto { format, samples: msaa_samples }, false);
    resources.insert(BackgroundResources { pipelines, sampler: sampler(device), linear_target: format.is_srgb(), shown: HashMap::new(), drawn: 0 });
}

/// A renderer whose every frame is kept (screenshots): scenes are there in the first frame that asks for them.
pub fn at_once(resources: &mut egui_wgpu::CallbackResources) {
    if let Some(res) = resources.get_mut::<BackgroundResources>() {
        res.pipelines.at_once = true;
    }
}

/// How many scene pictures were drawn, and how many backgrounds hold textures.
pub fn counts(resources: &egui_wgpu::CallbackResources) -> (u64, usize) {
    resources.get::<BackgroundResources>().map_or((0, 0), |r| (r.drawn, r.shown.len()))
}

/// A scene without an interface over it: the bench, and pictures to compare with the website's.
pub struct Alone {
    pipelines: Pipelines,
    sampler: wgpu::Sampler,
    scene: Option<Scenic>,
}

impl Alone {
    pub fn new(device: &wgpu::Device) -> Self {
        let onto = Onto { format: wgpu::TextureFormat::Rgba8Unorm, samples: 1 };
        Self { pipelines: Pipelines::new(onto, true), sampler: sampler(device), scene: None }
    }

    /// Encodes one picture of it at `css` points; the (RGBA8) texture it is in.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, backdrop: &Backdrop, time: Time, css: [f32; 2], ppp: f32) -> Option<wgpu::Texture> {
        if self.scene.as_ref().is_none_or(|s| !s.fits(backdrop.scene, css, ppp)) {
            let made = self.pipelines.of(device, backdrop.scene, || {})?;
            self.scene = Scenic::new(device, queue, made, &self.sampler, backdrop.scene, css, ppp);
        }
        let scene = self.scene.as_mut()?;
        scene.draw(queue, encoder, backdrop, time);
        Some(scene.output().clone())
    }
}

struct BackgroundCallback {
    id: Id,
    backdrop: Backdrop,
    time: Time,
    frame: i64,
    css: [f32; 2],
    ppp: f32,
    over: Over,
    pass: u64,
    ctx: egui::Context,
}

impl BackgroundCallback {
    /// `None` while the scene's pipelines are being made.
    fn begin(&self, device: &wgpu::Device, queue: &wgpu::Queue, res: &mut BackgroundResources) -> Option<Shown> {
        let ctx = self.ctx.clone();
        let made = res.pipelines.of(device, self.backdrop.scene, move || ctx.request_repaint())?;
        let scene = Scenic::new(device, queue, made, &res.sampler, self.backdrop.scene, self.css, self.ppp)?;
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
            contents: bytemuck::bytes_of(&self.over),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let view = scene.output().create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
            layout: &scene.composite()?.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&res.sampler) },
            ],
        });
        Some(Shown { scene, drawn: None, uniforms, bind, pass: self.pass })
    }
}

impl egui_wgpu::CallbackTrait for BackgroundCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<BackgroundResources>() else { return Vec::new() };
        res.shown.retain(|_, s| s.pass + FORGET_AFTER > self.pass);
        let kept = res.shown.remove(&self.id).filter(|s| s.scene.fits(self.backdrop.scene, self.css, self.ppp));
        let mut shown = match kept {
            Some(s) => s,
            None => match self.begin(device, queue, res) {
                Some(s) => s,
                None => return Vec::new(),
            },
        };
        shown.pass = self.pass;
        if shown.drawn != Some((self.backdrop, self.frame)) {
            shown.scene.draw(queue, encoder, &self.backdrop, self.time);
            shown.drawn = Some((self.backdrop, self.frame));
            res.drawn += 1;
        }
        let mut over = self.over;
        over.extra[0] = if res.linear_target { 1.0 } else { 0.0 };
        queue.write_buffer(&shown.uniforms, 0, bytemuck::bytes_of(&over));
        res.shown.insert(self.id, shown);
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(res) = resources.get::<BackgroundResources>() else { return };
        let Some(shown) = res.shown.get(&self.id) else { return };
        let Some(composite) = shown.scene.composite() else { return };
        pass.set_pipeline(composite);
        pass.set_bind_group(0, &shown.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shader_parses_and_is_valid() {
        for (name, source) in scenes::sources() {
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
            let mut validator = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty());
            validator.validate(&module).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        }
    }

    #[test]
    fn scenes_have_the_website_s_ids_and_words_are_readable_over_each() {
        for s in Scene::ALL {
            assert_eq!(Scene::of_id(s.id()), Some(s));
            let b = Backdrop::of(s);
            for tint in look::Tint::ALL {
                for accent in ["#00da6d", "#b78aff", "#338ef7"] {
                    let tokens = look::Tokens::of(&look::Look { tint, accent: look::parse_hex(accent).unwrap() });
                    let (veil, ground) = b.under(&tokens);
                    if s == Scene::None {
                        assert_eq!(veil, 0.0);
                        continue;
                    }
                    // Bright colours get a darker veil, never an unreadable page.
                    let white = Backdrop { colors: [Color32::WHITE; 3], ..b };
                    let (darker, bright) = white.under(&tokens);
                    assert!(darker >= veil && darker < 0.9, "{s:?} {darker}");
                    for ground in [ground, bright] {
                        let t = tokens.over(ground);
                        for text in [t.fg, t.fg_2, t.fg_3, t.accent] {
                            assert!(look::contrast(text, ground) >= look::AA - 0.02, "{s:?} {tint:?} {accent}: {text:?} on {ground:?} is {:.2}", look::contrast(text, ground));
                        }
                        // A card lets a little of it through.
                        let card = look::over(crate::theme::alpha(t.ink_850, crate::theme::OVER_BACKGROUND_ALPHA), ground);
                        assert!(look::contrast(t.fg_3, card) >= look::AA, "{s:?} {tint:?}: {:.2} on a card", look::contrast(t.fg_3, card));
                    }
                }
            }
        }
        assert_eq!(Scene::of_id("starfield"), None);
    }

    /// A scene is drawn when its time moves on, its colours change or its size does, and not when the page over it draws
    /// again; what is no longer painted is given back. On the offscreen GPU (skipped without one).
    #[test]
    fn a_scene_is_drawn_only_when_it_moves_and_goes_when_it_is_not_painted() {
        let Ok(mut gpu) = crate::offscreen::Offscreen::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let ctx = egui::Context::default();
        crate::install(&ctx);
        let textures = gpu.textures().clone();
        let mut t = 0.0;
        let mut run = |gpu: &mut crate::offscreen::Offscreen, w: f32, what: Option<(Backdrop, Time)>| {
            t += 0.01;
            let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 120.0));
            let input = egui::RawInput { screen_rect: Some(rect), time: Some(t), ..Default::default() };
            let draw = |ui: &mut egui::Ui| {
                if let Some((b, at)) = what {
                    paint(ui.painter(), Id::unique("bg"), rect, &b, at, Laid::PAGE);
                }
            };
            let image = gpu.frame(&ctx, input, draw, true).expect("a frame").expect("its picture");
            (counts(&textures.read().callback_resources), image)
        };
        for scene in Scene::ALL.into_iter().skip(1) {
            let b = Backdrop::of(scene);
            let before = run(&mut gpu, 200.0, None).0 .0;
            let ((n, held), first) = run(&mut gpu, 200.0, Some((b, Time { moved: 3.0, still: false })));
            assert_eq!((n - before, held), (1, 1), "{scene:?}");
            assert!(first.pixels.iter().any(|p| p.r() as u32 + p.g() as u32 + p.b() as u32 > 24), "{scene:?} draws something");
            assert_eq!(run(&mut gpu, 200.0, Some((b, Time { moved: 3.001, still: false }))).0 .0, n, "{scene:?}: the page alone drew again");
            assert_eq!(run(&mut gpu, 200.0, Some((b, Time { moved: 3.2, still: false }))).0 .0, n + 1, "{scene:?}: it moved");
            assert_eq!(run(&mut gpu, 200.0, Some((b, Time::STILL))).0 .0, n + 2);
            assert_eq!(run(&mut gpu, 200.0, Some((b, Time::STILL))).0 .0, n + 2, "{scene:?}: its still frame stands");
            assert_eq!(run(&mut gpu, 220.0, Some((b, Time::STILL))).0 .0, n + 3, "{scene:?}: another size");
            let other = Backdrop { colors: [Color32::from_rgb(8, 27, 39), Color32::from_rgb(36, 103, 121), Color32::from_rgb(69, 123, 104)], ..b };
            assert_eq!(run(&mut gpu, 220.0, Some((other, Time::STILL))).0 .0, n + 4, "{scene:?}: other colours");
        }
        for _ in 0..FORGET_AFTER {
            run(&mut gpu, 220.0, None);
        }
        // With no other background to look after it: tidied.
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0));
        let input = egui::RawInput { screen_rect: Some(rect), time: Some(99.0), ..Default::default() };
        gpu.frame(&ctx, input, |ui| tidy(ui.painter(), rect), true).expect("a frame");
        assert_eq!(counts(&textures.read().callback_resources).1, 0);
    }
}
