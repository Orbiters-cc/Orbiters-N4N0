use super::*;
use base64::Engine;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
    core::*,
    Win32::{Foundation::*, System::Com::*, UI::WindowsAndMessaging::*},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe fn string(get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> windows::core::Result<String> {
    let mut p = PWSTR::null();
    get(&mut p)?;
    let s = p.to_string();
    CoTaskMemFree(Some(p.0.cast()));
    Ok(s?)
}
fn pump_wait<T>(rx: &Receiver<windows::core::Result<T>>, closed: &AtomicBool, timeout: Duration) -> windows::core::Result<T> {
    let start = Instant::now();
    loop {
        if closed.load(Ordering::Relaxed) || start.elapsed() >= timeout {
            return Err(windows::core::Error::from_hresult(HRESULT(0x80004004u32 as i32)));
        }
        if let Ok(value) = rx.try_recv() {
            return value;
        }
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
unsafe fn cdp(web: &ICoreWebView2, method: &str, args: serde_json::Value, closed: &AtomicBool) -> windows::core::Result<String> {
    let (tx, rx) = mpsc::channel();
    web.CallDevToolsProtocolMethod(
        PCWSTR(wide(method).as_ptr()),
        PCWSTR(wide(&args.to_string()).as_ptr()),
        &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |e, raw| {
            let result =
                e.and_then(
                    |_| {
                        if raw.contains("\"error\"") {
                            Err(windows::core::Error::from_hresult(HRESULT(0x80004005u32 as i32)))
                        } else {
                            Ok(raw.to_owned())
                        }
                    },
                );
            let _ = tx.send(result);
            Ok(())
        })),
    )?;
    pump_wait(&rx, closed, Duration::from_secs(15))
}
/// A DevTools call not waited for: a page's input and the pictures' acknowledgements must never hold up the loop (each
/// waited call cost a round trip, and a laser sliding the page sends 90 a second: K-qr-page, 2026-11-15). Failures are
/// counted.
unsafe fn cdp_async(web: &ICoreWebView2, method: &str, args: serde_json::Value, failures: &Rc<Cell<u32>>) -> windows::core::Result<()> {
    let failures = failures.clone();
    web.CallDevToolsProtocolMethod(
        PCWSTR(wide(method).as_ptr()),
        PCWSTR(wide(&args.to_string()).as_ptr()),
        &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |e, raw| {
            if e.is_err() || raw.contains("\"error\"") {
                failures.set(failures.get() + 1);
            }
            Ok(())
        })),
    )
}
/// A page's picture, as the screencast sends it: JPEG, decoded to RGBA.
fn decode_jpeg(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgba = decoder.decode().map_err(|e| anyhow::anyhow!("page picture: {e:?}"))?;
    let info = decoder.info().ok_or_else(|| anyhow::anyhow!("page picture without a size"))?;
    anyhow::ensure!(rgba.len() == info.width as usize * info.height as usize * 4, "page picture of the wrong size");
    Ok((info.width as u32, info.height as u32, rgba))
}
/// How long a navigation has run, for the notes.
fn since(started: &Cell<Option<Instant>>) -> u128 {
    started.get().map_or(0, |t| t.elapsed().as_millis())
}
struct Resources {
    controller: Option<ICoreWebView2Controller>,
    hwnd: Option<HWND>,
}
impl Drop for Resources {
    fn drop(&mut self) {
        unsafe {
            if let Some(c) = self.controller.take() {
                let _ = c.Close();
            }
            if let Some(h) = self.hwnd.take() {
                let _ = DestroyWindow(h);
            }
        }
    }
}
pub fn run(w: u32, h: u32, rx: Receiver<Command>, shared: Arc<Mutex<Shared>>, closed: Arc<AtomicBool>, loopback: bool) -> anyhow::Result<()> {
    // How long the engine's start takes, part by part (Request 18: Enzo's pages waited 9 to 12 s for it, and one 30 s, before
    // the first "opening" line, which counts from the navigation only).
    let begun = Instant::now();
    let profile = match lifetime::Profile::new(&closed) {
        Ok(p) => p,
        Err(_) if closed.load(Ordering::Relaxed) => {
            shared.lock().unwrap().state.load = Load::Closed;
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    shared.lock().unwrap().note(format!("engine: private profile ready after {} ms", begun.elapsed().as_millis())); // i18n: not shown (a log's line)
    let proxy = network::Proxy::with(loopback)?;
    let result = unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = run_sta(w, h, rx, shared.clone(), closed.clone(), &profile, proxy.port, begun);
        CoUninitialize();
        result
    };
    let path = profile.path.clone();
    drop(profile);
    if result.is_ok() || closed.load(Ordering::Relaxed) {
        anyhow::ensure!(!path.exists(), "private browser cleanup pending");
        shared.lock().unwrap().state.load = Load::Closed;
        return Ok(());
    }
    result
}
unsafe fn run_sta(
    w: u32,
    h: u32,
    rx: Receiver<Command>,
    shared: Arc<Mutex<Shared>>,
    closed: Arc<AtomicBool>,
    profile: &lifetime::Profile,
    proxy_port: u16,
    begun: Instant,
) -> anyhow::Result<()> {
    let mut resources = Resources { controller: None, hwnd: None };
    let path = wide(&profile.path.to_string_lossy());
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();
    let arguments=format!("--disable-gpu --disable-extensions --disable-background-networking --no-first-run --proxy-server=http://127.0.0.1:{proxy_port} --proxy-bypass-list=<-loopback> --disable-quic --force-webrtc-ip-handling-policy=disable_non_proxied_udp --disable-features=WebTransport --enable-automation");
    options.SetAdditionalBrowserArguments(PCWSTR(wide(&arguments).as_ptr()))?;
    options.SetAllowSingleSignOnUsingOSPrimaryAccount(false)?;
    let (tx, envrx) = mpsc::channel();
    CreateCoreWebView2EnvironmentWithOptions(
        PCWSTR::null(),
        PCWSTR(path.as_ptr()),
        &options,
        &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |e, env| {
            let _ = tx.send(e.map(|_| env));
            Ok(())
        })),
    )?;
    let env = pump_wait(&envrx, &closed, Duration::from_secs(15))
        .map_err(|e| anyhow::anyhow!("WebView2 runtime unavailable or startup failed: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("environment missing"))?;
    shared.lock().unwrap().note(format!("engine: browser started after {} ms", begun.elapsed().as_millis())); // i18n: not shown (a log's line)
    let env7: ICoreWebView2Environment7 = env.cast()?;
    let actual = string(|p| env7.UserDataFolder(p))?;
    verify_profile(std::path::Path::new(&actual), &profile.path)?;
    // Message-only HWND has no desktop surface, taskbar entry or activation path.
    let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WINDOW_STYLE(0), 0, 0, w as i32, h as i32, Some(HWND_MESSAGE), None, None, None)?;
    resources.hwnd = Some(hwnd);
    let (tx, crx) = mpsc::channel();
    let env10: ICoreWebView2Environment10 = env.cast()?;
    let private = env10.CreateCoreWebView2ControllerOptions()?;
    private.SetIsInPrivateModeEnabled(true)?;
    env10.CreateCoreWebView2CompositionControllerWithOptions(
        hwnd,
        &private,
        &CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(move |e, c| {
            let _ = tx.send(e.map(|_| c));
            Ok(())
        })),
    )?;
    let composition = pump_wait(&crx, &closed, Duration::from_secs(15))?.ok_or_else(|| anyhow::anyhow!("controller missing"))?;
    let controller: ICoreWebView2Controller = composition.cast()?;
    resources.controller = Some(controller.clone());
    // Keep captured pixels and Chromium input pixels identical in DPI-aware hosts.
    let controller3: ICoreWebView2Controller3 = controller.cast()?;
    controller3.SetShouldDetectMonitorScaleChanges(false)?;
    controller3.SetRasterizationScale(1.0)?;
    controller3.SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)?;
    controller.SetBounds(RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 })?;
    controller.SetIsVisible(true)?;
    let web = controller.CoreWebView2()?;
    let command_line = cdp(&web, "Browser.getBrowserCommandLine", serde_json::json!({}), &closed)?;
    let command_line: serde_json::Value = serde_json::from_str(&command_line)?;
    let args = command_line["arguments"].as_array().ok_or_else(|| anyhow::anyhow!("browser arguments unavailable"))?;
    for prefix in ["--proxy-server=", "--proxy-bypass-list=", "--force-webrtc-ip-handling-policy=", "--disable-features="] {
        let expected = arguments.split_whitespace().find(|a| a.starts_with(prefix)).unwrap();
        let found: Vec<_> = args.iter().filter_map(|v| v.as_str()).filter(|a| a.starts_with(prefix)).collect();
        anyhow::ensure!(found == [expected], "browser network argument override rejected");
    }
    anyhow::ensure!(
        !args.iter().filter_map(|v| v.as_str()).any(|a| a.starts_with("--remote-debugging")
            || a == "--no-proxy-server"
            || a.starts_with("--proxy-pac-url")
            || a == "--no-sandbox"
            || a == "--disable-web-security"
            || a.starts_with("--ignore-certificate-errors")
            || a.starts_with("--allow-file-access")),
        "unsafe browser argument override rejected"
    );
    let settings = web.Settings()?;
    settings.SetAreDevToolsEnabled(false)?;
    settings.SetAreDefaultContextMenusEnabled(false)?;
    settings.SetAreDefaultScriptDialogsEnabled(false)?;
    settings.SetIsStatusBarEnabled(false)?;
    settings.SetIsWebMessageEnabled(false)?;
    settings.SetAreHostObjectsAllowed(false)?;
    settings.SetIsBuiltInErrorPageEnabled(false)?;
    let settings4: ICoreWebView2Settings4 = settings.cast()?;
    settings4.SetIsGeneralAutofillEnabled(false)?;
    settings4.SetIsPasswordAutosaveEnabled(false)?;
    let guard_web = web.clone();
    let (tx, scriptrx) = mpsc::channel();
    guard_web.AddScriptToExecuteOnDocumentCreated(w!("for (const name of ['print','showOpenFilePicker','showSaveFilePicker','showDirectoryPicker']) { Object.defineProperty(window, name, {value:()=>undefined, writable:false, configurable:false}); }"),
        &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |e,_|{let _=tx.send(e);Ok(())})))?;
    pump_wait(&scriptrx, &closed, Duration::from_secs(15))?;
    let mut token = 0;
    // When the current navigation started, and whether its first picture was noted.
    let started: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    let drawn = Rc::new(Cell::new(false));
    let failures: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let navigation = Rc::new(RefCell::new(0u64));
    let pending: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let capture_navigation = Rc::new(RefCell::new(u64::MAX));
    let active = Rc::new(RefCell::new(false));
    let nav = navigation.clone();
    let p = pending.clone();
    let live = active.clone();
    let s = shared.clone();
    let (at, first) = (started.clone(), drawn.clone());
    web.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                let url = string(|p| a.Uri(p))?;
                if validate(&url).is_err() {
                    a.SetCancel(true)?;
                } else {
                    let mut id = 0;
                    a.NavigationId(&mut id)?;
                    let mut redirected = BOOL(0);
                    a.IsRedirected(&mut redirected)?;
                    // A redirect keeps its navigation (and its time); a new one starts both again.
                    if id != *nav.borrow() || at.get().is_none() {
                        at.set(Some(Instant::now()));
                        first.set(false);
                    }
                    *nav.borrow_mut() = id;
                    *p.borrow_mut() = None;
                    *live.borrow_mut() = false;
                    let mut s = s.lock().unwrap();
                    let host = url::Url::parse(&url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
                    s.note(format!("{} {host} ({} ms into it)", if redirected.as_bool() { "redirected to" } else { "opening" }, since(&at))); // i18n: not shown (a log's line)
                    s.state.url = display_text(&url, 8192);
                    s.state.title.clear();
                    s.state.load = Load::Loading;
                    s.frame = None;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let s = shared.clone();
    let nav = navigation.clone();
    let live = active.clone();
    let p = pending.clone();
    let at = started.clone();
    web.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                let mut id = 0;
                a.NavigationId(&mut id)?;
                if id != *nav.borrow() {
                    return Ok(());
                }
                *p.borrow_mut() = None;

                let mut ok = BOOL(0);
                a.IsSuccess(&mut ok)?;
                *live.borrow_mut() = ok.as_bool();
                let mut s = s.lock().unwrap();
                s.state.load = if ok.as_bool() {
                    s.note(format!("loaded after {} ms", since(&at))); // i18n: not shown (a log's line)
                    Load::Loaded
                } else {
                    let mut reason = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                    a.WebErrorStatus(&mut reason)?;
                    s.note(format!("failed after {} ms: error {}", since(&at), reason.0)); // i18n: not shown (a log's line)
                    Load::Failed(format!("navigation error {}", reason.0))
                };
            }
            Ok(())
        })),
        &mut token,
    )?;
    // The page's pictures from the moment its document is committed (Request 18): it paints as it reads, before its whole
    // content is read (a page whose scripts hold DOMContentLoaded showed nothing for as long). Pictures of a blank page
    // before it first paints are left out below.
    let nav = navigation.clone();
    let live = active.clone();
    let at = started.clone();
    let s = shared.clone();
    web.add_ContentLoading(
        &ContentLoadingEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                let mut id = 0;
                a.NavigationId(&mut id)?;
                if id == *nav.borrow() {
                    *live.borrow_mut() = true;
                    s.lock().unwrap().note(format!("committed after {} ms", since(&at))); // i18n: not shown (a log's line)
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    // And at the latest from the moment its content is read, not from its load event: a page whose last picture or
    // tracker never finishes loading showed nothing, "Opening the page..." for as long as it took (linktr.ee: 16 s).
    let web2: ICoreWebView2_2 = web.cast()?;
    let s = shared.clone();
    let nav = navigation.clone();
    let live = active.clone();
    let at = started.clone();
    web2.add_DOMContentLoaded(
        &DOMContentLoadedEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                let mut id = 0;
                a.NavigationId(&mut id)?;
                if id == *nav.borrow() {
                    *live.borrow_mut() = true;
                    s.lock().unwrap().note(format!("content read after {} ms", since(&at))); // i18n: not shown (a log's line)
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let s = shared.clone();
    web.add_DocumentTitleChanged(
        &DocumentTitleChangedEventHandler::create(Box::new(move |web, _| {
            if let Some(web) = web {
                s.lock().unwrap().state.title = display_text(&string(|p| web.DocumentTitle(p))?, 512);
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_PermissionRequested(
        &PermissionRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_NewWindowRequested(
        &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetHandled(true)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    web.add_FrameNavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                if validate(&string(|p| a.Uri(p))?).is_err() {
                    a.SetCancel(true)?;
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let s = shared.clone();
    web.add_SourceChanged(
        &SourceChangedEventHandler::create(Box::new(move |web, _| {
            if let Some(web) = web {
                let url = string(|p| web.Source(p))?;
                if validate(&url).is_ok() {
                    s.lock().unwrap().state.url = display_text(&url, 8192);
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    let web4: ICoreWebView2_4 = web.cast()?;
    web4.add_DownloadStarting(
        &DownloadStartingEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetCancel(true)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let web10: ICoreWebView2_10 = web.cast()?;
    let web5: ICoreWebView2_5 = web.cast()?;
    web5.add_ClientCertificateRequested(
        &ClientCertificateRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetCancel(true)?;
                a.SetHandled(true)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    web10.add_BasicAuthenticationRequested(
        &BasicAuthenticationRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetCancel(true)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let web14: ICoreWebView2_14 = web.cast()?;
    web14.add_ServerCertificateErrorDetected(
        &ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let web18: ICoreWebView2_18 = web.cast()?;
    web18.add_LaunchingExternalUriScheme(
        &LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| {
            if let Some(a) = args {
                a.SetCancel(true)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let s = shared.clone();
    let failed = Rc::new(RefCell::new(false));
    let process_failed = failed.clone();
    web.add_ProcessFailed(
        &ProcessFailedEventHandler::create(Box::new(move |_, _| {
            let mut s = s.lock().unwrap();
            s.note("the page's process failed".into()); // i18n: not shown (a log's line)
            s.state.load = Load::Failed("WebView2 page process failed".into());
            *process_failed.borrow_mut() = true;
            Ok(())
        })),
        &mut token,
    )?;
    let p = pending.clone();
    let live = active.clone();
    let nav = navigation.clone();
    let captured = capture_navigation.clone();
    let events = web.GetDevToolsProtocolEventReceiver(w!("Page.screencastFrame"))?;
    events.add_DevToolsProtocolEventReceived(
        &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                if *live.borrow() && *nav.borrow() == *captured.borrow() {
                    *p.borrow_mut() = Some(string(|v| a.ParameterObjectAsJson(v))?);
                }
            }
            Ok(())
        })),
        &mut token,
    )?;
    cdp(&web, "Page.setInterceptFileChooserDialog", serde_json::json!({"enabled":true}), &closed)?;
    {
        let mut s = shared.lock().unwrap();
        s.ready = true;
        s.note(format!("engine ready after {} ms", begun.elapsed().as_millis())); // i18n: not shown (a log's line)
    }
    let mut capturing = false;
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut seq = 0;
    let mut pressed = false;
    let mut dimensions = (w, h);
    loop {
        anyhow::ensure!(!*failed.borrow(), "WebView2 page process failed");
        if closed.load(Ordering::Relaxed) {
            break;
        }
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if capturing && (!*active.borrow() || *capture_navigation.borrow() != *navigation.borrow()) {
            cdp(&web, "Page.stopScreencast", serde_json::json!({}), &closed)?;
            capturing = false;
            *pending.borrow_mut() = None;
        }
        if !capturing && *active.borrow() {
            *pending.borrow_mut() = None;
            *capture_navigation.borrow_mut() = *navigation.borrow();
            cdp(&web, "Page.startScreencast", serde_json::json!({"format":"jpeg","quality":88,"maxWidth":dimensions.0,"maxHeight":dimensions.1}), &closed)?;
            capturing = true;
        }
        let mut wheel: Option<(f64, f64, f64, f64)> = None;
        for _ in 0..256 {
            match rx.try_recv() {
                #[cfg(test)]
                Ok(Command::Fixture(html)) => {
                    let tree = cdp(&web, "Page.getFrameTree", serde_json::json!({}), &closed)?;
                    let tree: serde_json::Value = serde_json::from_str(&tree)?;
                    cdp(&web, "Page.setDocumentContent", serde_json::json!({"frameId":tree["frameTree"]["frame"]["id"],"html":html}), &closed)?;
                    *active.borrow_mut() = true;
                    shared.lock().unwrap().state.load = Load::Loaded;
                }
                #[cfg(test)]
                Ok(Command::FailCdp) => {
                    cdp(&web, "Page.thisMethodDoesNotExist", serde_json::json!({}), &closed)?;
                }
                Ok(Command::Navigate(url)) => {
                    *active.borrow_mut() = false;
                    *pending.borrow_mut() = None;
                    shared.lock().unwrap().frame = None;
                    web.Navigate(PCWSTR(wide(&url).as_ptr()))?;
                }
                Ok(Command::Resize(w, h)) => {
                    dimensions = (w, h);
                    *pending.borrow_mut() = None;
                    shared.lock().unwrap().frame = None;
                    controller.SetBounds(RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 })?;
                    if capturing {
                        cdp(&web, "Page.stopScreencast", serde_json::json!({}), &closed)?;
                        capturing = false;
                    }
                    *pending.borrow_mut() = None;
                }
                Ok(Command::Input(i)) => {
                    // Wheel steps that follow each other are one: a slide sends one a turn.
                    if let Input::Wheel { x, y, dx, dy } = i {
                        wheel = Some(match wheel {
                            Some((_, _, wx, wy)) => (x, y, wx + dx, wy + dy),
                            None => (x, y, dx, dy),
                        });
                        continue;
                    }
                    if let Some((x, y, dx, dy)) = wheel.take() {
                        cdp_async(&web, "Input.dispatchMouseEvent", serde_json::json!({"type":"mouseWheel","x":x,"y":y,"button":"none","buttons":if pressed{1}else{0},"deltaX":dx,"deltaY":dy}), &failures)?;
                    }
                    let (kind, x, y) = match i {
                        Input::Move { x, y } => ("mouseMoved", x, y),
                        Input::Press { x, y } => {
                            pressed = true;
                            ("mousePressed", x, y)
                        }
                        Input::Release { x, y } => {
                            pressed = false;
                            ("mouseReleased", x, y)
                        }
                        Input::Wheel { .. } => unreachable!("taken above"),
                    };
                    let click = kind == "mousePressed" || kind == "mouseReleased";
                    cdp_async(
                        &web,
                        "Input.dispatchMouseEvent",
                        serde_json::json!({"type":kind,"x":x,"y":y,"button":if click{"left"}else{"none"},"buttons":if pressed{1}else{0},"clickCount":if click{1}else{0}}),
                        &failures,
                    )?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    closed.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        if let Some((x, y, dx, dy)) = wheel.take() {
            cdp_async(&web, "Input.dispatchMouseEvent", serde_json::json!({"type":"mouseWheel","x":x,"y":y,"button":"none","buttons":if pressed{1}else{0},"deltaX":dx,"deltaY":dy}), &failures)?;
        }
        if failures.get() > 0 {
            shared.lock().unwrap().note(format!("{} page calls failed", failures.replace(0))); // i18n: not shown (a log's line)
        }
        // The newest picture, as soon as one came, at most 40 a second: acknowledging it is what lets Chromium send the next
        // (at most one waits), so a page that does not change sends nothing, and one that scrolls is seen scrolling.
        if last.elapsed() >= Duration::from_millis(25) {
            let raw = pending.borrow_mut().take();
            if let Some(raw) = raw {
                let frame_navigation = *capture_navigation.borrow();
                // The picture's text is read where it lies, not copied into a JSON value first (it is up to a few MB).
                let v: Cast = serde_json::from_str(&raw)?;
                cdp_async(&web, "Page.screencastFrameAck", serde_json::json!({"sessionId":v.session_id}), &failures)?;
                last = Instant::now();
                let bytes = base64::engine::general_purpose::STANDARD.decode(v.data.as_bytes())?;
                let (width, height, rgba) = decode_jpeg(&bytes)?;
                if !current_frame(frame_navigation, *navigation.borrow(), *active.borrow()) {
                    continue;
                }
                let mut s = shared.lock().unwrap();
                // A blank page (one flat colour) before the page is loaded is the moment between its commit and its first
                // paint: not shown, the panel goes on saying it opens.
                if !drawn.get() && s.state.load != Load::Loaded && flat(&rgba) {
                    continue;
                }
                if !drawn.replace(true) {
                    s.note(format!("first picture {width} x {height} after {} ms", since(&started))); // i18n: not shown (a log's line)
                }
                if s.frame.as_ref().is_none_or(|f| f.rgba.as_slice() != rgba.as_slice()) {
                    seq += 1;
                    s.frame = Some(Frame { width, height, rgba: Arc::new(rgba), sequence: seq });
                }
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Close runs unconditionally through Resources, even if CDP or a browser process failed.
    Ok(())
}
/// Whether a picture (RGBA) is one flat colour: a page not painted yet. 24 x 24 points of it, within 3 levels of the first.
fn flat(rgba: &[u8]) -> bool {
    let n = rgba.len() / 4;
    if n == 0 {
        return true;
    }
    let first = &rgba[..3];
    (0..576).map(|k| (k * (n - 1) / 575) * 4).all(|i| rgba[i..i + 3].iter().zip(first).all(|(a, b)| a.abs_diff(*b) <= 3))
}

/// `Page.screencastFrame`'s parameters: the picture (JPEG in base64) and the id to acknowledge it with.
#[derive(serde::Deserialize)]
struct Cast<'a> {
    #[serde(borrow, default)]
    data: std::borrow::Cow<'a, str>,
    #[serde(rename = "sessionId", default)]
    session_id: serde_json::Value,
}

fn verify_profile(actual: &std::path::Path, owned: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(std::fs::canonicalize(actual)? == std::fs::canonicalize(owned)?, "effective profile override rejected");
    Ok(())
}
/// A picture is shown only for the navigation it was taken of. Its size is whatever Chromium sent (the overlay fits it to
/// the page's place): a picture rejected for its size left the page on "Opening the page..." for good.
fn current_frame(frame_nav: u64, nav: u64, active: bool) -> bool {
    active && frame_nav == nav
}
#[cfg(test)]
mod tests {
    #[test]
    fn rejects_effective_profile_override_without_deleting_it() {
        let owned = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        assert!(super::verify_profile(external.path(), owned.path()).is_err());
        assert!(external.path().exists());
        assert!(super::verify_profile(owned.path(), owned.path()).is_ok());
    }
    #[test]
    fn a_blank_picture_is_flat_and_a_painted_one_is_not() {
        let mut picture = vec![255u8; 64 * 64 * 4];
        assert!(super::flat(&picture));
        picture[40 * 64 * 4 + 20 * 4] = 0;
        picture[40 * 64 * 4 + 20 * 4 + 1] = 0;
        // One point the 24 x 24 grid may not land on: a painted page has far more than one dark pixel.
        for i in (0..picture.len()).step_by(4 * 7) {
            picture[i] = 30;
        }
        assert!(!super::flat(&picture));
        assert!(super::flat(&[]));
    }
    #[test]
    fn delayed_navigation_frames_are_rejected_and_any_size_is_shown() {
        assert!(!super::current_frame(1, 2, true));
        assert!(!super::current_frame(2, 2, false));
        assert!(super::current_frame(2, 2, true));
    }
    use super::*;
    #[test]
    fn stalled_startup_is_cancelled_and_deadlined() {
        let (_tx, rx) = mpsc::channel::<windows::core::Result<()>>();
        let flag = AtomicBool::new(true);
        let t = Instant::now();
        assert!(pump_wait(&rx, &flag, Duration::from_secs(15)).is_err());
        assert!(t.elapsed() < Duration::from_millis(100));
        flag.store(false, Ordering::Relaxed);
        assert!(pump_wait(&rx, &flag, Duration::from_millis(20)).is_err());
    }
    #[test]
    fn async_completion_failure_is_not_submission_success() {
        let (tx, rx) = mpsc::channel::<windows::core::Result<()>>();
        tx.send(Err(windows::core::Error::from_hresult(HRESULT(0x80004005u32 as i32)))).unwrap();
        assert!(pump_wait(&rx, &AtomicBool::new(false), Duration::from_secs(1)).is_err());
    }
}
