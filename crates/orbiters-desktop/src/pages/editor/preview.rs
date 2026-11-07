//! The editor's preview, made on a thread of its own from the picture scaled to about the canvas (its long side at
//! most [`PROXY`] pixels): the interface asks for the edit as it is now, and a request asked while one is made
//! replaces it and stops it at its next layer (the newest settings win). Each layer works on every core (rayon).

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
    pub image: Rgba8,
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
            self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).asked = Some(ask);
            self.shared.wake.notify_one();
        }
    }

    /// The preview made since the last call.
    pub fn take(&self) -> Option<Made> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).made.take()
    }

    /// Whether the last preview asked is not made yet.
    pub fn busy(&self) -> bool {
        let s = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        s.asked.is_some()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        self.shared.wake.notify_one();
    }
}

fn run(shared: &Shared, ctx: &egui::Context, inputs: &[Rgba8], full: (u32, u32)) {
    let mut renderer = Renderer::preview();
    let refs: Vec<&Rgba8> = inputs.iter().collect();
    loop {
        let ask = {
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if s.quit {
                    return;
                }
                if let Some(a) = s.asked.clone() {
                    break a;
                }
                s = shared.wake.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        // Stopped as soon as something newer is asked.
        let mut still_wanted = |_: f32| {
            let s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            !s.quit && s.asked.as_ref() == Some(&ask)
        };
        let image = renderer.render(&ask.edit, &refs, full, ask.stage, ask.size, &mut still_wanted);
        let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(image) = image {
            if s.asked.as_ref() == Some(&ask) {
                s.asked = None;
            }
            drop(s);
            let image = image.to_rgba8();
            let histogram = orbiters_ui::photo::histogram(&image, 64);
            let mut s = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            s.made = Some(Made { ask, image, histogram });
            drop(s);
            ctx.request_repaint();
        }
    }
}
