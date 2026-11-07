//! The Mac's side of the app's two processes. The tray service is an accessory application (no Dock tile of its own, a
//! status item in the menu bar) that runs AppKit's event loop on its main thread, and opens the window when the app is
//! opened again from the Finder or the Dock. The window process is a regular application, whose menu's Quit quits the app
//! as a whole: closing the window only closes the window.

use crate::backend::Ctl;
use crate::service::Service;
use orbiters_i18n::tr;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSApplicationTerminateReply};
use objc2_foundation::{NSActivityOptions, NSNotification, NSProcessInfo, NSString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// The service while it runs: AppKit's callbacks reach it from here.
static SERVICE: OnceLock<Arc<Service>> = OnceLock::new();

struct Ivars {
    /// The status item, for as long as the application runs.
    tray: std::cell::OnceCell<TrayIcon>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OrbitersServiceDelegate"]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        /// A status item needs the application running.
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _: &NSNotification) {
            match tray() {
                Ok(icon) => {
                    let _ = self.ivars().tray.set(icon);
                }
                Err(e) => eprintln!("menu bar item: {e:#}"),
            }
        }

        /// The app opened again while it runs (from the Finder, the Dock or Spotlight): its window.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _: &NSApplication, _: bool) -> bool {
            if let Some(service) = SERVICE.get() {
                service.show_window(false);
            }
            false
        }

        /// The system asks the app to quit (logging out, restarting): it saves what it records first, off this thread,
        /// and tells AppKit when it is done.
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _: &NSApplication) -> NSApplicationTerminateReply {
            if let Some(service) = SERVICE.get() {
                let service = service.clone();
                std::thread::spawn(move || service.quit());
                return NSApplicationTerminateReply::TerminateLater;
            }
            NSApplicationTerminateReply::TerminateNow
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { tray: Default::default() });
        // SAFETY: NSObject's init, on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The menu bar's mark, black and clear: macOS colours it for the bar.
fn template_icon() -> anyhow::Result<tray_icon::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/menu-bar.png")));
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf)?;
    anyhow::ensure!(info.color_type == png::ColorType::Rgba, "unexpected icon format {:?}", info.color_type); // i18n: not shown (a bundled icon, logged)
    buf.truncate(info.buffer_size());
    Ok(tray_icon::Icon::from_rgba(buf, info.width, info.height)?)
}

/// The status item and its menu, as the system's others: a click opens the menu.
fn tray() -> anyhow::Result<TrayIcon> {
    let show = MenuItem::with_id("show", tr("Show Orbiters - N4N0"), true, None);
    let quit = MenuItem::with_id("quit", tr("Quit Orbiters - N4N0"), true, None);
    let menu = Menu::with_items(&[&show, &PredefinedMenuItem::separator(), &quit])?;
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        let Some(service) = SERVICE.get() else { return };
        match e.id.0.as_str() {
            "show" => service.show_window(false),
            // Quitting saves what records: off the thread that keeps the menu bar alive.
            "quit" => {
                let service = service.clone();
                std::thread::spawn(move || service.quit());
            }
            _ => {}
        }
    }));
    Ok(TrayIconBuilder::new()
        .with_tooltip("Orbiters - N4N0")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_icon_templated(template_icon()?)
        .build()?)
}

/// Keeps macOS's App Nap off this process for as long as it runs. A service without a window would otherwise nap (OSC and
/// mDNS answered minutes late), and so would a window behind others, which then does not hear that the app quit. A process
/// that has nothing to do uses nothing either way.
pub fn stay_awake() {
    let reason = NSString::from_str("Talks with VRChat and the app's other processes"); // i18n: not shown
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &reason);
    // For the life of the process: ending it would let the nap back.
    std::mem::forget(activity);
}

/// Runs the service's side of AppKit on this, the main, thread until the process exits: the service leaves when asked
/// to quit ([`leave`]).
pub fn run_service(service: Arc<Service>) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| anyhow::anyhow!("not on the main thread"))?; // i18n: not shown (a programming error, logged)
    let _ = SERVICE.set(service);
    stay_awake();
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    drop(delegate);
    Ok(())
}

/// The service is done: the process ends. AppKit's loop has no other way out from another thread, and nothing is left for
/// it to do; what must be removed on the way out is passed in.
pub fn leave(socket_guard: impl FnOnce()) -> ! {
    socket_guard();
    std::process::exit(0)
}

static QUIT_ASKED: AtomicBool = AtomicBool::new(false);

/// The window process's menu bar, as Mac apps have: the app's menu (its Quit quits the service too, as Settings' Quit
/// does; Cmd+Q) and the Window menu. No Edit menu: its key equivalents would take Cmd+C, Cmd+V and the others from egui,
/// which handles them itself.
pub fn install_window_menu(ctl: Ctl) {
    if let Err(e) = window_menu(ctl) {
        eprintln!("the window's menu: {e:#}");
    }
}

fn window_menu(ctl: Ctl) -> anyhow::Result<()> {
    let app = Submenu::new("Orbiters - N4N0", true);
    let quit = MenuItem::with_id("window-quit", tr("Quit Orbiters - N4N0"), true, Some("Cmd+Q".parse()?));
    app.append_items(&[
        &PredefinedMenuItem::about(Some(tr("About Orbiters - N4N0")), None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(None),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;
    let window = Submenu::new(tr("Window"), true);
    window.append_items(&[
        &PredefinedMenuItem::minimize(None),
        &PredefinedMenuItem::maximize(None),
        &PredefinedMenuItem::fullscreen(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::close_window(None),
    ])?;
    let menu = Menu::with_items(&[&app, &window])?;
    menu.init_for_nsapp();
    window.set_as_windows_menu_for_nsapp();
    // The menu outlives every window: it stays until the process goes.
    std::mem::forget(menu);
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id.0 == "window-quit" && !QUIT_ASKED.swap(true, Ordering::Relaxed) {
            ctl.quit();
        }
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_menu_bar_icon_is_a_template() {
        let icon = super::template_icon();
        assert!(icon.is_ok());
    }
}
