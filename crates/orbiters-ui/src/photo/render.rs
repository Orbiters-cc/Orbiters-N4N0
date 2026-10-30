//! The edit made into a picture: each source cropped and turned to the size asked, a multi-layer picture's layers each
//! through its own effects and stacked back to front, then the layers over the whole picture. A [`Renderer`] kept
//! between previews keeps the turned sources and the picture just before the layer being changed, so that dragging a
//! slider redoes that layer and the ones over it, not the whole stack.

use super::{blur, effects, geometry, text, tone, Crop, Edit, Effect, Image, Layer, Mask, Rgba8};
use rayon::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// What is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The cropped picture with its layers.
    Full,
    /// The picture cropped and turned, without its layers (the comparison with the original).
    Plain,
    /// The straightened picture whole, without its layers: what the crop is drawn on.
    Uncropped,
}

/// The largest a lens blur's disc gets, a directional blur's line and a gaussian's deviation, in shares of the height.
pub const LENS_MAX: f32 = 0.04;
pub const DIRECTIONAL_MAX: f32 = 0.1;
pub const GAUSSIAN_MAX: f32 = 0.05;

/// The picture's size at full size for `stage`, for a first source of `full` pixels.
pub fn full_size(edit: &Edit, full: (u32, u32), stage: Stage) -> (u32, u32) {
    match stage {
        Stage::Uncropped => geometry::turned_size(full.0, full.1, edit.geometry.turns),
        _ => edit.geometry.output_size(full.0, full.1),
    }
}

/// `size` scaled down to fit inside `max`×`max` pixels, its shape kept.
pub fn fit_size(size: (u32, u32), max_w: u32, max_h: u32) -> (u32, u32) {
    let k = (max_w as f64 / size.0 as f64).min(max_h as f64 / size.1 as f64).min(1.0);
    (((size.0 as f64 * k).round() as u32).max(1), ((size.1 as f64 * k).round() as u32).max(1))
}

/// One layer over `img`: its effect, mixed with the picture before it by its opacity and mask.
pub fn apply(img: &mut Image, layer: &Layer) {
    if !layer.visible || layer.opacity <= 0.0 {
        return;
    }
    if let Effect::Text(t) = &layer.effect {
        text::composite(img, t, layer.opacity);
        return;
    }
    let whole = layer.opacity >= 1.0 && layer.mask == Mask::Everywhere;
    let before = (!whole).then(|| img.clone());
    effect(img, &layer.effect);
    if let Some(before) = before {
        mix(img, &before, layer.opacity.clamp(0.0, 1.0), &layer.mask);
    }
}

fn effect(img: &mut Image, e: &Effect) {
    let h = img.height as f32;
    match e {
        Effect::Light(l) => tone::light(img, l),
        Effect::Color(c) => tone::color(img, c),
        Effect::Curves(c) => tone::curves(img, c),
        Effect::Vignette(v) => effects::vignette(img, v),
        Effect::Aberration(a) => effects::aberration(img, a),
        Effect::Comic(c) => effects::comic(img, c),
        Effect::Blur(b) => match b.kind {
            super::BlurKind::Gaussian => blur::gaussian(img, b.radius.clamp(0.0, GAUSSIAN_MAX) * h),
            super::BlurKind::Lens => blur::lens(img, b.radius.clamp(0.0, LENS_MAX) * h, b.bokeh),
            super::BlurKind::Directional => blur::directional(img, b.radius.clamp(0.0, DIRECTIONAL_MAX) * h, b.angle),
        },
        Effect::Text(_) => {}
    }
}

/// `img` back towards `before` where the layer shows less.
fn mix(img: &mut Image, before: &Image, opacity: f32, mask: &Mask) {
    let (w, h) = (img.width as usize, img.height as usize);
    let aspect = w as f32 / h as f32;
    img.data.par_chunks_mut(w).zip(before.data.par_chunks(w)).enumerate().for_each(|(y, (row, was))| {
        let v = (y as f32 + 0.5) / h as f32;
        for (x, (p, b)) in row.iter_mut().zip(was).enumerate() {
            let k = opacity * mask.weight((x as f32 + 0.5) / w as f32, v, aspect);
            for c in 0..4 {
                p[c] = b[c] + (p[c] - b[c]) * k;
            }
        }
    });
}

/// `top` at `opacity` over `under` (premultiplied).
fn stack(under: &mut Image, top: &Image, opacity: f32) {
    under.data.par_iter_mut().zip(top.data.par_iter()).for_each(|(u, t)| {
        let a = t[3] * opacity;
        for c in 0..4 {
            u[c] = t[c] * opacity + u[c] * (1.0 - a);
        }
    });
}

fn hash(v: &impl std::fmt::Debug) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}

/// Makes pictures of an edit, keeping what the next one can reuse.
#[derive(Default)]
pub struct Renderer {
    /// Keep the picture before the layer being changed (previews; a single full-size render needs none).
    keep: bool,
    /// The sources cropped and turned, for the key they were made for.
    turned: Option<(u64, Vec<Image>)>,
    /// The picture after the first `.1` layers of the whole picture's stack, for the key of that point.
    checkpoint: Option<(u64, usize, Image)>,
    /// The last whole-picture stack drawn, layer by layer: the first that differs next time is being changed.
    last: Vec<u64>,
}

impl Renderer {
    /// For previews: keeps what speeds up the next one.
    pub fn preview() -> Self {
        Self { keep: true, ..Default::default() }
    }

    /// The edit of `inputs` (one per [`Edit::sources`], or the picture alone; the first `full` pixels at its full size,
    /// the others of its shape) at `size` pixels for `stage`. `progress` hears how far it is (0 to 1) and stops it by
    /// returning false: then None.
    pub fn render(&mut self, edit: &Edit, inputs: &[&Rgba8], full: (u32, u32), stage: Stage, size: (u32, u32), progress: &mut dyn FnMut(f32) -> bool) -> Option<Image> {
        let crop = match stage {
            Stage::Uncropped => Crop::FULL,
            _ => edit.geometry.effective_crop(full.0, full.1),
        };
        let shown = |l: &[Layer]| l.iter().filter(|l| l.visible).count();
        let layers_total = if stage == Stage::Full { shown(&edit.layers) + edit.sources.iter().map(|s| shown(&s.layers)).sum::<usize>() } else { 0 };
        let mut steps = Steps { done: 0, total: edit.sources.len().max(1) + layers_total, progress };
        let inputs_key = inputs.iter().map(|i| (i.pixels.as_ptr() as usize, i.width, i.height)).collect::<Vec<_>>();
        let turned_key = hash(&(&edit.geometry, crop, size, &inputs_key));
        if self.turned.as_ref().is_none_or(|(k, _)| *k != turned_key) {
            let turned: Vec<Image> = inputs.iter().map(|src| geometry::transform(src, &edit.geometry, crop, size.0, size.1)).collect();
            self.turned = Some((turned_key, turned));
            self.checkpoint = None;
        }
        let turned = &self.turned.as_ref().expect("made above").1;
        if turned.is_empty() {
            return Some(Image::new(size.0, size.1));
        }
        if stage != Stage::Full {
            return composite(edit, turned, stage, size, &mut steps);
        }

        // The whole picture's layers, from the picture kept before the one being changed when it still applies.
        let base_key = hash(&(turned_key, &edit.sources));
        let layers = &edit.layers;
        let keys: Vec<u64> = layers.iter().map(hash).collect();
        let prefix: Vec<u64> = (0..=layers.len()).map(|k| hash(&(base_key, &keys[..k]))).collect();
        let changed = keys.iter().zip(&self.last).position(|(a, b)| a != b).unwrap_or(keys.len().min(self.last.len()));
        self.last = keys;
        let resume = self.checkpoint.as_ref().filter(|(h, k, _)| *k <= layers.len() && prefix[*k] == *h).map(|(_, k, img)| (*k, img.clone()));
        let (start, mut img) = match resume {
            Some((k, img)) => {
                // The sources and the layers under it count as done.
                steps.done += edit.sources.len().max(1) + shown(&layers[..k]) + edit.sources.iter().map(|s| shown(&s.layers)).sum::<usize>();
                (k, img)
            }
            None => (0, composite(edit, turned, stage, size, &mut steps)?),
        };
        for (i, l) in layers.iter().enumerate().skip(start) {
            if self.keep && i == changed && self.checkpoint.as_ref().is_none_or(|(h, _, _)| *h != prefix[i]) {
                self.checkpoint = Some((prefix[i], i, img.clone()));
            }
            apply(&mut img, l);
            if l.visible && !steps.next() {
                return None;
            }
        }
        Some(img)
    }
}

/// How far a render is, told to its caller (who may stop it).
struct Steps<'a> {
    done: usize,
    total: usize,
    progress: &'a mut dyn FnMut(f32) -> bool,
}

impl Steps<'_> {
    fn next(&mut self) -> bool {
        self.done += 1;
        (self.progress)((self.done as f32 / self.total.max(1) as f32).min(1.0))
    }
}

/// The sources stacked back to front, each through its own layers (only with [`Stage::Full`]).
fn composite(edit: &Edit, turned: &[Image], stage: Stage, size: (u32, u32), steps: &mut Steps) -> Option<Image> {
    if edit.sources.is_empty() {
        return steps.next().then(|| turned[0].clone());
    }
    let plain = stage != Stage::Full;
    let mut acc = Image::new(size.0, size.1);
    for (i, s) in edit.sources.iter().enumerate() {
        let Some(t) = turned.get(i) else { continue };
        if !plain && !s.visible {
            continue;
        }
        let mut img = t.clone();
        if !plain {
            for l in &s.layers {
                apply(&mut img, l);
                if l.visible && !steps.next() {
                    return None;
                }
            }
        }
        stack(&mut acc, &img, if plain { 1.0 } else { s.opacity.clamp(0.0, 1.0) });
        if !steps.next() {
            return None;
        }
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo::{Kind, Light, SourceStack, Text, TextStyle};

    fn flat(w: u32, h: u32, rgba: [u8; 4]) -> Rgba8 {
        Rgba8::new(w, h, rgba.iter().copied().cycle().take((w * h * 4) as usize).collect())
    }

    fn go(edit: &Edit, inputs: &[&Rgba8], size: (u32, u32)) -> Image {
        Renderer::default().render(edit, inputs, (inputs[0].width, inputs[0].height), Stage::Full, size, &mut |_| true).unwrap()
    }

    #[test]
    fn layers_apply_bottom_first() {
        let src = flat(4, 4, [128, 128, 128, 255]);
        let mut edit = Edit::default();
        // Darken to black, then lift the black point back: order matters.
        let down = edit.layer(Effect::Light(Light { exposure: -4.0, ..Default::default() }));
        let faded = edit.layer(Effect::Light(Light { black_point: -0.25, ..Default::default() }));
        edit.layers = vec![down.clone(), faded.clone()];
        let a = go(&edit, &[&src], (4, 4)).data[0][0];
        edit.layers = vec![faded, down];
        let b = go(&edit, &[&src], (4, 4)).data[0][0];
        assert!(a > b * 2.0, "{a} {b}");
    }

    #[test]
    fn hidden_faded_and_masked_layers_show_as_much_as_they_should() {
        let src = flat(20, 10, [200, 200, 200, 255]);
        let mut edit = Edit::default();
        let mut dark = edit.layer(Effect::Light(Light { exposure: -4.0, ..Default::default() }));
        let plain = go(&edit, &[&src], (20, 10)).data[0][0];
        dark.visible = false;
        edit.layers = vec![dark.clone()];
        assert_eq!(go(&edit, &[&src], (20, 10)).data[0][0], plain);
        dark.visible = true;
        dark.opacity = 0.5;
        edit.layers = vec![dark.clone()];
        let half = go(&edit, &[&src], (20, 10)).data[0][0];
        assert!((half - plain * (0.5 + 0.5 / 16.0)).abs() < 0.01, "{half} {plain}");
        dark.opacity = 1.0;
        dark.mask = Mask::Radial { center: [0.25, 0.5], radius: 0.2, feather: 0.01, invert: false };
        edit.layers = vec![dark];
        let img = go(&edit, &[&src], (20, 10));
        assert!(img.data[5 * 20 + 5][0] < plain * 0.1, "darkened inside its circle");
        assert!((img.data[5 * 20 + 18][0] - plain).abs() < 1e-4, "untouched outside");
    }

    #[test]
    fn multi_layer_pictures_stack_back_to_front_each_with_its_effects() {
        let world = flat(8, 8, [0, 0, 255, 255]);
        // The people: a red square in the middle, transparent around it.
        let mut people = flat(8, 8, [0, 0, 0, 0]);
        for y in 2..6 {
            for x in 2..6 {
                people.pixels[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let mut edit = Edit::default();
        let dim = edit.layer(Effect::Light(Light { exposure: -2.0, ..Default::default() }));
        edit.sources = vec![
            SourceStack { name: "Environment".into(), layers: vec![dim], ..Default::default() },
            SourceStack { name: "Player".into(), ..Default::default() },
        ];
        let img = go(&edit, &[&world, &people], (8, 8));
        // The world dimmed, the people over it as they were.
        assert!((img.data[0][2] - 0.25).abs() < 0.01, "{:?}", img.data[0]);
        assert!((img.data[4 * 8 + 4][0] - 1.0).abs() < 1e-3 && img.data[4 * 8 + 4][2] < 1e-3, "{:?}", img.data[36]);
        // A hidden layer leaves the one under it.
        edit.sources[1].visible = false;
        let img = go(&edit, &[&world, &people], (8, 8));
        assert!(img.data[4 * 8 + 4][0] < 1e-3);
        // Its order is the stack's: the world over the people hides them.
        edit.sources[1].visible = true;
        edit.sources.swap(0, 1);
        let img = go(&edit, &[&people, &world], (8, 8));
        assert!(img.data[4 * 8 + 4][0] < 1e-3);
    }

    #[test]
    fn a_kept_preview_redoes_only_what_changed_and_draws_the_same() {
        let pixels: Vec<u8> = (0..64 * 48).flat_map(|i| [(i % 251) as u8, (i % 97) as u8 * 2, 90, 255]).collect();
        let src = Rgba8::new(64, 48, pixels);
        let mut edit = Edit::default();
        for k in Kind::ALL {
            let mut l = edit.layer(k.effect());
            if let Effect::Text(t) = &mut l.effect {
                *t = Text { value: "Hi".into(), style: TextStyle::Glow, ..Default::default() };
            }
            edit.layers.push(l);
        }
        let mut kept = Renderer::preview();
        let size = (64, 48);
        let first = kept.render(&edit, &[&src], (64, 48), Stage::Full, size, &mut |_| true).unwrap();
        // The vignette changes twice: the second time starts from the picture before it.
        for amount in [0.2f32, -0.4] {
            if let Effect::Vignette(v) = &mut edit.layers[3].effect {
                v.amount = amount;
            }
            let mut calls = 0;
            let again = kept.render(&edit, &[&src], (64, 48), Stage::Full, size, &mut |_| {
                calls += 1;
                true
            }).unwrap();
            let fresh = go(&edit, &[&src], size);
            assert_eq!(again, fresh);
            assert_ne!(again, first);
            if amount < 0.0 {
                assert_eq!(calls, Kind::ALL.len() - 3, "only the vignette and what is over it");
            }
        }
        let mut stopped = Renderer::default();
        assert!(stopped.render(&edit, &[&src], (64, 48), Stage::Full, size, &mut |_| false).is_none());
    }

    /// `cargo test -p orbiters-ui photo::render::tests::a_4k_picture -- --ignored --nocapture`: how long each layer takes
    /// on a 3840×2160 picture, and a preview of it.
    #[test]
    #[ignore]
    fn a_4k_picture_through_every_kind_of_layer() {
        use crate::photo::{Blur, BlurKind, Comic};
        let (w, h) = (3840u32, 2160u32);
        let src = Rgba8::new(w, h, (0..w * h).flat_map(|i| [(i % w / 16) as u8, (i / w / 9) as u8, ((i * 7) % 255) as u8, 255]).collect());
        let mut edit = Edit::default();
        let mut effects: Vec<Effect> = Kind::ALL.iter().map(|k| k.effect()).collect();
        effects.push(Effect::Blur(Blur { kind: BlurKind::Lens, radius: 0.02, bokeh: 0.8, ..Default::default() }));
        effects.push(Effect::Blur(Blur { kind: BlurKind::Directional, radius: 0.05, ..Default::default() }));
        effects.push(Effect::Comic(Comic { dots: true, ..Default::default() }));
        for e in effects {
            let mut one = edit.clone();
            let l = one.layer(e);
            let name = format!("{:?}", l.effect.kind());
            one.layers = vec![l.clone()];
            let start = std::time::Instant::now();
            go(&one, &[&src], (w, h));
            eprintln!("{name:>12}: {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
            edit.layers.push(l);
        }
        let start = std::time::Instant::now();
        go(&edit, &[&src], (w, h));
        eprintln!("   the whole stack at 3840x2160: {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
        let small = src.fit(2048);
        let start = std::time::Instant::now();
        Renderer::preview().render(&edit, &[&small], (w, h), Stage::Full, fit_size((w, h), 1400, 800), &mut |_| true).unwrap();
        eprintln!("   a 1400x788 preview of it: {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
    }

    #[test]
    fn stages_show_the_picture_without_its_layers() {
        let src = flat(40, 20, [100, 150, 200, 255]);
        let mut edit = Edit::default();
        let l = edit.layer(Effect::Light(Light { exposure: 2.0, ..Default::default() }));
        edit.layers.push(l);
        edit.geometry.crop = Crop { x: 0.5, y: 0.0, w: 0.5, h: 1.0 };
        assert_eq!(full_size(&edit, (40, 20), Stage::Full), (20, 20));
        assert_eq!(full_size(&edit, (40, 20), Stage::Uncropped), (40, 20));
        let mut r = Renderer::default();
        let plain = r.render(&edit, &[&src], (40, 20), Stage::Plain, (20, 20), &mut |_| true).unwrap();
        assert_eq!(plain.to_rgba8().pixels[..4], [100, 150, 200, 255]);
        let whole = r.render(&edit, &[&src], (40, 20), Stage::Uncropped, (40, 20), &mut |_| true).unwrap();
        assert_eq!((whole.width, whole.height), (40, 20));
        assert_eq!(fit_size((4000, 2000), 1000, 1000), (1000, 500));
        assert_eq!(fit_size((400, 200), 1000, 1000), (400, 200));
    }
}
