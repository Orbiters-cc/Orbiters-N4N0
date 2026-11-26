//! The editor's preview, made on a thread of its own from the picture scaled to about the canvas (its long side at
//! most [`PROXY`] pixels): the interface asks for the edit as it is now, and a request asked while one is made
//! replaces it and stops it at its next layer (the newest settings win). Each layer works on every core (rayon).

use orbiters_core::locks::*;
use orbiters_ui::photo::render::{Renderer, Stage};
use orbiters_ui::photo::{Edit, Rgba8};
use std::sync::{Arc, Condvar, Mutex};

/// The long side of the pictures the preview is made from.
pub const PROXY: u32 = 2048;

/// What the interface asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub edit: Edit,
    pub stage: Stage,
    pub size: (u32, u32),
}

/// A preview made.
pub struct Made {
    pub ask: Ask,
    /// As the interface's texture takes it, made on the preview's thread (PAGES-UI-17): the interface thread made it, a pass
    /// over the whole picture and its size again in memory, at every preview of a slider's drag.
    pub image: egui::ColorImage,
    /// Its luma histogram ([`orbiters_ui::photo::histogram`]), for the curves.
    pub histogram: Vec<u32>,
}

#[derive(Default)]
struct State {
    asked: Option<Ask>,
    made: Option<Made>,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct Preview {
    shared: Arc<Shared>,
    last: Option<Ask>,
}

impl Preview {
    /// Makes previews of `inputs` (the files of [`super::files::Source::files`], scaled), the first `full` pixels at its
    /// full size; `ctx` repaints when one is made.
    pub fn start(ctx: &egui::Context, inputs: Arc<Vec<Rgba8>>, full: (u32, u32)) -> Self {
        let shared = Arc::new(Shared::default());
        let (s, ctx) = (shared.clone(), ctx.clone());
        std::thread::Builder::new().name("photo editor preview".into()).spawn(move || run(&s, &ctx, &inputs, full)).expect("thread"); // i18n: not shown
        Self { shared, last: None }
    }

    /// Asks for this preview, unless it is the one asked last.
    pub fn ask(&mut self, ask: Ask) {
        if self.last.as_ref() != Some(&ask) {
            self.last = Some(ask.clone());
            self.shared.state.locked().asked = Some(ask);
            self.shared.wake.notify_one();
        }
    }

    /// The preview made since the last call.
    pub fn take(&self) -> Option<Made> {
        self.shared.state.locked().made.take()
    }

    /// Whether the last preview asked is not made yet.
    pub fn busy(&self) -> bool {
        let s = self.shared.state.locked();
        s.asked.is_some()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.shared.state.locked().quit = true;
        self.shared.wake.notify_one();
    }
}

fn run(shared: &Shared, ctx: &egui::Context, inputs: &[Rgba8], full: (u32, u32)) {
    let mut renderer = Renderer::preview();
    let refs: Vec<&Rgba8> = inputs.iter().collect();
    loop {
        let ask = {
            let mut s = shared.state.locked();
            loop {
                if s.quit {
                    return;
                }
                if let Some(a) = s.asked.clone() {
                    break a;
                }
                s = shared.wake.wait_locked(s);
            }
        };
        // Stopped as soon as something newer is asked.
        let mut still_wanted = |_: f32| {
            let s = shared.state.locked();
            !s.quit && s.asked.as_ref() == Some(&ask)
        };
        let image = renderer.render(&ask.edit, &refs, full, ask.stage, ask.size, &mut still_wanted);
        let mut s = shared.state.locked();
        if let Some(image) = image {
            if s.asked.as_ref() == Some(&ask) {
                s.asked = None;
            }
            drop(s);
            let rgba = image.to_rgba8();
            let histogram = orbiters_ui::photo::histogram(&rgba, 64);
            let image = egui::ColorImage::from_rgba_unmultiplied([rgba.width as usize, rgba.height as usize], &rgba.pixels);
            drop(rgba);
            let mut s = shared.state.locked();
            s.made = Some(Made { ask, image, histogram });
            drop(s);
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PAGES-UI-17: a preview comes as the texture's picture, of the size asked, with its histogram.
    #[test]
    fn a_preview_comes_as_the_texture_s_picture() {
        let ctx = egui::Context::default();
        let input = Rgba8 { width: 4, height: 2, pixels: [90u8, 120, 150, 255].repeat(8) };
        let mut preview = Preview::start(&ctx, Arc::new(vec![input]), (4, 2));
        preview.ask(Ask { edit: Edit::default(), stage: Stage::Full, size: (4, 2) });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let made = loop {
            if let Some(made) = preview.take() {
                break made;
            }
            assert!(std::time::Instant::now() < until, "no preview came");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert_eq!(made.image.size, [4, 2]);
        assert_eq!(made.image.pixels.len(), 8);
        assert_eq!(made.histogram.len(), 64);
    }
}
