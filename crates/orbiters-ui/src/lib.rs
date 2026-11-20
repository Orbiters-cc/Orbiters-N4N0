//! Orbiters interface toolkit, shared by the desktop window and the VR overlay: the Orbiters look (theme, fonts,
//! icons), springs, the glow background shader, the progressive blur under bars, painting helpers, widgets, the
//! record button, the trail map, toasts, the window that grows out of a card ([`viewer`]) and
//! how such morphs land back on their source ([`morph`]), an offscreen GPU renderer
//! (screenshots), a CPU painter (the VR overlay) and the photo editor's pixels ([`photo`]).

pub mod anim;
pub mod autoscroll;
pub mod blender_logo;
pub mod blur;
pub mod dialog;
pub mod feedback;
pub mod fonts;
pub mod format;
pub mod glow;
pub mod icons;
pub mod layout;
pub mod logo;
pub mod morph;
pub mod n4n0_face;
pub mod n4n0_logo;
pub mod offscreen;
pub mod paint;
pub mod photo;
pub mod popup;
pub mod record;
pub mod scrollbars;
pub mod software;
pub mod steps;
pub mod text_audit;
pub mod theme;
pub mod toast;
pub mod trail;
pub mod viewer;
pub mod widgets;

pub use egui;
pub use egui_wgpu;

/// Fonts and style for a context (once, before the first frame).
pub fn install(ctx: &egui::Context) {
    fonts::install(ctx);
    theme::apply(ctx);
    scrollbars::install(ctx);
}

/// What a renderer draws into, for paint callbacks that build their own pipelines (kept in its callback resources).
#[derive(Clone, Copy, Debug)]
pub struct RenderTarget {
    pub format: egui_wgpu::wgpu::TextureFormat,
    pub msaa_samples: u32,
}

/// GPU pipelines for a renderer drawing to `format` (once per renderer).
pub fn install_wgpu(device: &egui_wgpu::wgpu::Device, format: egui_wgpu::wgpu::TextureFormat, msaa_samples: u32, resources: &mut egui_wgpu::CallbackResources) {
    resources.insert(RenderTarget { format, msaa_samples });
    glow::install(device, format, msaa_samples, resources);
    blur::install(device, format, msaa_samples, resources);
}
