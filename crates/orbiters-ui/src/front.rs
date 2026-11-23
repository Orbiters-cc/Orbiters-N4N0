//! What is in front of the page: every window layer over it (a sheet, a dialog, a list, a viewer, a toast) says that it is
//! shown, and whether it holds the foreground (the keyboard and the pointer are its own), as it is shown and through one
//! place ([`area`], [`layer`]; [`hold`] for a sheet drawn in the page's own layer). What must stand aside under it (N4N0's
//! chat, verify-r21 major 7 and verify2-r21 M5) asks [`held`] rather than a list of owners that a new one is missing from.
//! [`missing`] finds the layers that did not say: the screenshots' scenes are walked with it, and the sources are read
//! for an `Area` made elsewhere.

use egui::{Area, Context, Id, LayerId, Order};

/// A layer shown, whether it holds the foreground, and in which pass.
#[derive(Clone, Copy)]
struct Shown {
    layer: LayerId,
    holds: bool,
    pass: u64,
}

fn key() -> Id {
    Id::unique("orbiters-front-layers")
}

fn note(ctx: &Context, layer: LayerId, holds: bool) {
    // The pass is read first: the context cannot be read while its memory is locked for writing.
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let mut all = d.get_temp::<Vec<Shown>>(key()).unwrap_or_default();
        all.retain(|s| s.pass + 1 >= pass && !(s.layer == layer && s.pass == pass));
        all.push(Shown { layer, holds, pass });
        d.insert_temp(key(), all);
    });
}

/// An area over the page (the only way one is made). `holds`: the keyboard and the pointer are its own while it shows (a
/// sheet, a dialog, a list, not closing); false for what only floats there (a toast, an offer's card, a bar of actions, a
/// layer growing back into its button).
pub fn area(ctx: &Context, id: Id, order: Order, holds: bool) -> Area {
    note(ctx, LayerId::new(order, id), holds);
    Area::new(id).order(order)
}

/// [`area`] for a layer drawn without one (`UiBuilder::layer_id`).
pub fn layer(ctx: &Context, layer: LayerId, holds: bool) -> LayerId {
    note(ctx, layer, holds);
    layer
}

/// A sheet of the page's own layer holds the foreground (no layer of its own: a page's side panel as a sheet, an edit in
/// place).
pub fn hold(ctx: &Context, id: Id) {
    note(ctx, LayerId::new(Order::Background, id), true);
}

/// Whether anything holds the foreground now (said in this pass or the last one).
pub fn held(ctx: &Context) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<Vec<Shown>>(key())).is_some_and(|all| all.iter().any(|s| s.holds && s.pass + 1 >= pass))
}

/// The layers over the page that took a press or a key in the pass that just ended without saying they were shown
/// (tooltips, which take neither, are none of them).
pub fn missing(ctx: &Context) -> Vec<LayerId> {
    let pass = ctx.cumulative_pass_nr();
    let shown: Vec<LayerId> = ctx.data(|d| d.get_temp::<Vec<Shown>>(key())).unwrap_or_default().iter().filter(|s| s.pass + 1 >= pass).map(|s| s.layer).collect();
    ctx.viewport(|v| {
        let layers = v.prev_pass.widgets.layers();
        let over = layers.filter(|(l, _)| matches!(l.order, Order::Middle | Order::Foreground | Order::Tooltip));
        over.filter(|(_, widgets)| widgets.iter().any(|w| w.enabled && w.sense.interactive())).map(|(l, _)| *l).filter(|l| !shown.contains(l)).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect, Sense};

    fn frame(ctx: &Context, ui: impl FnMut(&mut egui::Ui)) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 300.0))), ..Default::default() };
        let _ = ctx.run_ui(input, ui);
    }

    #[test]
    fn a_layer_that_takes_presses_says_it_is_shown() {
        let ctx = Context::default();
        let press = |ui: &mut egui::Ui| drop(ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click()));
        frame(&ctx, press);
        assert!(missing(&ctx).is_empty() && !held(&ctx), "the page itself");
        // One that says nothing is found.
        let silent = Id::unique("silent");
        for _ in 0..2 {
            frame(&ctx, |ui| drop(Area::new(silent).order(Order::Foreground).show(ui.ctx(), press)));
        }
        assert_eq!(missing(&ctx), vec![LayerId::new(Order::Foreground, silent)]);
        assert!(!held(&ctx));
        // A sheet that says so holds the foreground, then no more a pass after it went.
        for _ in 0..2 {
            frame(&ctx, |ui| drop(area(ui.ctx(), Id::unique("sheet"), Order::Foreground, true).show(ui.ctx(), press)));
        }
        assert!(missing(&ctx).is_empty(), "{:?}", missing(&ctx));
        assert!(held(&ctx));
        frame(&ctx, press);
        frame(&ctx, press);
        assert!(!held(&ctx));
        // A toast floats: known, and holds nothing.
        for _ in 0..2 {
            frame(&ctx, |ui| drop(area(ui.ctx(), Id::unique("toast"), Order::Foreground, false).show(ui.ctx(), press)));
        }
        assert!(missing(&ctx).is_empty() && !held(&ctx));
        // A sheet of the page's own layer.
        frame(&ctx, |ui| hold(ui.ctx(), Id::unique("panel")));
        assert!(held(&ctx));
    }
}
