//! Tray icon: show the window, start or stop recording, quit. Closing the window hides it here; the app keeps
//! capturing in the background. Tray events arrive on the window's thread even while the window is hidden.

use egui::ViewportCommand;

/// Brings the window back from the tray (or from behind other windows).
pub fn show_window(ctx: &egui::Context) {
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
    ctx.request_repaint();
}

#[cfg(windows)]
pub use win::Tray;

/// No tray yet on this platform: closing the window quits.
#[cfg(not(windows))]
pub struct Tray;

#[cfg(not(windows))]
impl Tray {
    pub fn update(&mut self, _capturing: bool) {}
}

#[cfg(windows)]
mod win {
    use super::show_window;
    use crate::controller::Ctl;
    use egui::ViewportCommand;
    use orbiters_ui::toast;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        _icon: TrayIcon,
        toggle: MenuItem,
        capturing: bool,
    }

    fn icon() -> anyhow::Result<tray_icon::Icon> {
        let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/32x32.png")));
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().unwrap_or(32 * 32 * 4)];
        let info = reader.next_frame(&mut buf)?;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            png::ColorType::Rgb => buf[..info.buffer_size()].as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            other => anyhow::bail!("unexpected icon format {other:?}"),
        };
        Ok(tray_icon::Icon::from_rgba(rgba, info.width, info.height)?)
    }

    impl Tray {
        pub fn new(ctl: Ctl, ctx: egui::Context, quitting: Arc<AtomicBool>) -> anyhow::Result<Self> {
            let show = MenuItem::with_id("show", "Show Orbiters desktop", true, None);
            let toggle = MenuItem::with_id("toggle", "Start recording", true, None);
            let quit = MenuItem::with_id("quit", "Quit Orbiters desktop", true, None);
            let menu = Menu::with_items(&[&show, &toggle, &PredefinedMenuItem::separator(), &quit])?;
            let tray =
                TrayIconBuilder::new().with_tooltip("Orbiters desktop").with_menu(Box::new(menu)).with_menu_on_left_click(false).with_icon(icon()?).build()?;

            let menu_ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
                "show" => show_window(&menu_ctx),
                "toggle" => {
                    let r = if ctl.is_capturing() { ctl.stop_capture() } else { ctl.start_capture(false) };
                    if let Err(err) = r {
                        ctl.notify(toast::Kind::Error, err.to_string());
                        show_window(&menu_ctx);
                    }
                }
                "quit" => {
                    quitting.store(true, Ordering::Relaxed);
                    ctl.shutdown();
                    menu_ctx.send_viewport_cmd(ViewportCommand::Close);
                    menu_ctx.request_repaint();
                }
                _ => {}
            }));
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    show_window(&ctx);
                }
            }));
            Ok(Self { _icon: tray, toggle, capturing: false })
        }

        /// Keeps the menu's start/stop entry in step with the capture (call on the window's thread).
        pub fn update(&mut self, capturing: bool) {
            if capturing != self.capturing {
                self.capturing = capturing;
                self.toggle.set_text(if capturing { "Stop recording" } else { "Start recording" });
            }
        }
    }
}
