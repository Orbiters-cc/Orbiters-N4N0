//! Tray icon of the service: show the window, start or stop recording, quit. Closing the window leaves the app
//! here, capturing in the background.

#[cfg(windows)]
pub use win::Tray;

#[cfg(windows)]
mod win {
    use crate::controller::NoticeKind;
    use crate::service::Service;
    use orbiters_i18n::{language, tr, Language};
    use std::sync::Arc;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        _icon: TrayIcon,
        show: MenuItem,
        toggle: MenuItem,
        quit: MenuItem,
        capturing: bool,
        /// The language the entries were last written in.
        language: Language,
    }

    fn icon() -> anyhow::Result<tray_icon::Icon> {
        let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/32x32.png")));
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().unwrap_or(32 * 32 * 4)];
        let info = reader.next_frame(&mut buf)?;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            png::ColorType::Rgb => buf[..info.buffer_size()].as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            other => anyhow::bail!("unexpected icon format {other:?}"), // i18n: not shown (a bundled icon, logged)
        };
        Ok(tray_icon::Icon::from_rgba(rgba, info.width, info.height)?)
    }

    impl Tray {
        /// Creates the icon; its events arrive on this thread's message loop.
        pub fn new(service: Arc<Service>) -> anyhow::Result<Self> {
            let show = MenuItem::with_id("show", tr("Show Orbiters - N4N0"), true, None);
            let toggle = MenuItem::with_id("toggle", tr("Start recording"), true, None);
            let quit = MenuItem::with_id("quit", tr("Quit Orbiters - N4N0"), true, None);
            let menu = Menu::with_items(&[&show, &toggle, &PredefinedMenuItem::separator(), &quit])?;
            let tray =
                TrayIconBuilder::new().with_tooltip("Orbiters - N4N0").with_menu(Box::new(menu)).with_menu_on_left_click(false).with_icon(icon()?).build()?;

            let svc = service.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| match e.id.0.as_str() {
                "show" => svc.show_window(false),
                "toggle" => {
                    // Starting reads VRChat's values over HTTP: not on the thread of the tray's message loop.
                    let svc = svc.clone();
                    std::thread::spawn(move || {
                        let ctl = &svc.ctl;
                        let r = if ctl.is_capturing() { ctl.stop_capture() } else { ctl.start_capture(false) };
                        if let Err(err) = r {
                            ctl.notify(NoticeKind::Error, err.to_string());
                            svc.show_window(false);
                        }
                    });
                }
                "quit" => svc.quit(),
                _ => {}
            }));
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    service.show_window(false);
                }
            }));
            Ok(Self { _icon: tray, show, toggle, quit, capturing: false, language: language() })
        }

        /// Keeps the menu's start/stop entry in step with the capture.
        pub fn update(&mut self, capturing: bool) {
            // Also when the language changed: the account can bring a new one after the icon was made.
            if capturing != self.capturing || language() != self.language {
                self.capturing = capturing;
                self.language = language();
                self.toggle.set_text(if capturing { tr("Stop recording") } else { tr("Start recording") });
                self.show.set_text(tr("Show Orbiters - N4N0"));
                self.quit.set_text(tr("Quit Orbiters - N4N0"));
            }
        }
    }
}
