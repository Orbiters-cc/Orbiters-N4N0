//! Orbiters interface toolkit, shared by the desktop window and the VR overlay: the Orbiters look (theme, fonts,
//! icons), springs, the glow background shader, painting helpers, widgets, the record button, the trail map,
//! toasts, an offscreen GPU renderer (screenshots) and a CPU painter (the VR overlay).

pub mod anim;
pub mod fonts;
pub mod format;
pub mod glow;
pub mod icons;
pub mod logo;
pub mod offscreen;
pub mod paint;
pub mod record;
pub mod software;
pub mod theme;
pub mod toast;
pub mod trail;
pub mod widgets;

pub use egui;
pub use egui_wgpu;

/// Fonts and style for a context (once, before the first frame).
pub fn install(ctx: &egui::Context) {
    fonts::install(ctx);
    theme::apply(ctx);
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
}
