use super::*;
use base64::Engine;
use std::{
    cell::RefCell,
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
unsafe fn cdp(web: &ICoreWebView2, method: &str, args: serde_json::Value) -> windows::core::Result<()> {
    web.CallDevToolsProtocolMethod(
        PCWSTR(wide(method).as_ptr()),
        PCWSTR(wide(&args.to_string()).as_ptr()),
        &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|e, _| e)),
    )
}

pub fn run(w: u32, h: u32, rx: Receiver<Command>, shared: Arc<Mutex<Shared>>, closed: Arc<AtomicBool>) -> anyhow::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = run_sta(w, h, rx, shared, closed);
        CoUninitialize();
        result
    }
}
unsafe fn run_sta(w: u32, h: u32, rx: Receiver<Command>, shared: Arc<Mutex<Shared>>, closed: Arc<AtomicBool>) -> anyhow::Result<()> {
    let profile = tempfile::Builder::new().prefix("orbiters-web-private-").tempdir()?;
    let path = wide(&profile.path().to_string_lossy());
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();
    // No remote debugging port, account integration or ambient Windows authentication.
    options.SetAdditionalBrowserArguments(PCWSTR(wide("--disable-gpu --disable-extensions --disable-background-networking --no-first-run").as_ptr()))?;
    options.SetAllowSingleSignOnUsingOSPrimaryAccount(false)?;
    let (tx, envrx) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |cb| {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), PCWSTR(path.as_ptr()), &options, &cb).map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |e, env| {
            e?;
            let _ = tx.send(env);
            Ok(())
        }),
    )
    .map_err(|e| anyhow::anyhow!("WebView2 runtime unavailable or startup failed: {e}"))?;
    let env = envrx.recv()?.ok_or_else(|| anyhow::anyhow!("WebView2 environment missing"))?;
    // Message-only HWND has no desktop surface, taskbar entry or activation path.
    let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WINDOW_STYLE(0), 0, 0, w as i32, h as i32, Some(HWND_MESSAGE), None, None, None)?;
    let (tx, crx) = mpsc::channel();
    let env10: ICoreWebView2Environment10 = env.cast()?;
    let private = env10.CreateCoreWebView2ControllerOptions()?;
    private.SetIsInPrivateModeEnabled(true)?;
    CreateCoreWebView2CompositionControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |cb| env10.CreateCoreWebView2CompositionControllerWithOptions(hwnd, &private, &cb).map_err(webview2_com::Error::WindowsError)),
        Box::new(move |e, c| {
            e?;
            let _ = tx.send(c);
            Ok(())
        }),
    )
    .map_err(|e| anyhow::anyhow!("WebView2 composition startup: {e}"))?;
    let composition = crx.recv()?.ok_or_else(|| anyhow::anyhow!("WebView2 controller missing"))?;
    let controller: ICoreWebView2Controller = composition.cast()?;
    // Keep captured pixels and Chromium input pixels identical in DPI-aware hosts.
    let controller3: ICoreWebView2Controller3 = controller.cast()?;
    controller3.SetShouldDetectMonitorScaleChanges(false)?;
    controller3.SetRasterizationScale(1.0)?;
    controller3.SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)?;
    controller.SetBounds(RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 })?;
    controller.SetIsVisible(true)?;
    let web = controller.CoreWebView2()?;
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
    AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
        Box::new(move |cb| guard_web.AddScriptToExecuteOnDocumentCreated(w!("for (const name of ['print','showOpenFilePicker','showSaveFilePicker','showDirectoryPicker']) { Object.defineProperty(window, name, {value:()=>undefined, writable:false, configurable:false}); }"), &cb).map_err(webview2_com::Error::WindowsError)),
        Box::new(|e,_|e),
    ).map_err(|e|anyhow::anyhow!("page dialog guards: {e}"))?;
    let mut token = 0;
    let s = shared.clone();
    web.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                let url = string(|p| a.Uri(p))?;
                if validate(&url).is_err() {
                    a.SetCancel(true)?;
                } else {
                    let mut s = s.lock().unwrap();
                    s.state.url = url;
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
    web.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                let mut ok = BOOL(0);
                a.IsSuccess(&mut ok)?;
                s.lock().unwrap().state.load = if ok.as_bool() {
                    Load::Loaded
                } else {
                    let mut reason = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                    a.WebErrorStatus(&mut reason)?;
                    Load::Failed(format!("navigation error {}", reason.0))
                };
            }
            Ok(())
        })),
        &mut token,
    )?;
    let s = shared.clone();
    web.add_DocumentTitleChanged(
        &DocumentTitleChangedEventHandler::create(Box::new(move |web, _| {
            if let Some(web) = web {
                s.lock().unwrap().state.title = string(|p| web.DocumentTitle(p))?;
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
                    s.lock().unwrap().state.url = url;
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
    web.add_ProcessFailed(
        &ProcessFailedEventHandler::create(Box::new(move |_, _| {
            s.lock().unwrap().state.load = Load::Failed("WebView2 page process failed".into());
            Ok(())
        })),
        &mut token,
    )?;
    let pending = Rc::new(RefCell::new(None));
    let p = pending.clone();
    let events = web.GetDevToolsProtocolEventReceiver(w!("Page.screencastFrame"))?;
    events.add_DevToolsProtocolEventReceived(
        &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
            if let Some(a) = args {
                *p.borrow_mut() = Some(string(|v| a.ParameterObjectAsJson(v))?);
            }
            Ok(())
        })),
        &mut token,
    )?;
    cdp(&web, "Page.setInterceptFileChooserDialog", serde_json::json!({"enabled":true}))?;
    cdp(&web, "Page.startScreencast", serde_json::json!({"format":"png","maxWidth":w,"maxHeight":h,"everyNthFrame":1}))?;
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut seq = 0;
    let mut pressed = false;
    loop {
        if closed.load(Ordering::Relaxed) {
            break;
        }
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(Command::Navigate(url)) => web.Navigate(PCWSTR(wide(&url).as_ptr()))?,
                Ok(Command::Resize(w, h)) => {
                    controller.SetBounds(RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 })?;
                    cdp(&web, "Page.stopScreencast", serde_json::json!({}))?;
                    cdp(&web, "Page.startScreencast", serde_json::json!({"format":"png","maxWidth":w,"maxHeight":h}))?;
                }
                Ok(Command::Input(i)) => {
                    let (kind, x, y, dx, dy) = match i {
                        Input::Move { x, y } => ("mouseMoved", x, y, 0., 0.),
                        Input::Press { x, y } => {
                            pressed = true;
                            ("mousePressed", x, y, 0., 0.)
                        }
                        Input::Release { x, y } => {
                            pressed = false;
                            ("mouseReleased", x, y, 0., 0.)
                        }
                        Input::Wheel { x, y, dx, dy } => ("mouseWheel", x, y, dx, dy),
                    };
                    cdp(
                        &web,
                        "Input.dispatchMouseEvent",
                        serde_json::json!({"type":kind,"x":x,"y":y,"button":if kind=="mousePressed"||kind=="mouseReleased"{"left"}else{"none"},"buttons":if pressed{1}else{0},"clickCount":if kind=="mousePressed"||kind=="mouseReleased"{1}else{0},"deltaX":dx,"deltaY":dy}),
                    )?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    closed.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
        // Back-pressure Chromium by acknowledging at most ten frames per second. Static pages emit nothing.
        if last.elapsed() >= Duration::from_millis(100) {
            if let Some(raw) = pending.borrow_mut().take() {
                let v: serde_json::Value = serde_json::from_str(&raw)?;
                cdp(&web, "Page.screencastFrameAck", serde_json::json!({"sessionId":v["sessionId"]}))?;
                let bytes = base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap_or(""))?;
                let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
                decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
                let mut reader = decoder.read_info()?;
                let mut rgba = vec![0; reader.output_buffer_size().unwrap_or(0)];
                let info = reader.next_frame(&mut rgba)?;
                rgba.truncate(info.buffer_size());
                let mut s = shared.lock().unwrap();
                if s.frame.as_ref().is_none_or(|f| f.rgba.as_ref() != rgba.as_slice()) {
                    seq += 1;
                    s.frame = Some(Frame { width: info.width, height: info.height, rgba: rgba.into(), sequence: seq });
                }
                last = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    cdp(&web, "Page.stopScreencast", serde_json::json!({}))?;
    controller.Close()?;
    drop(controller3);
    drop(web18);
    drop(web5);
    drop(web14);
    drop(web10);
    drop(web4);
    drop(settings4);
    drop(settings);
    drop(web);
    drop(controller);
    drop(composition);
    drop(events);
    drop(env);
    DestroyWindow(hwnd)?;
    // Browser exits asynchronously; retry deletion of our unique throwaway directory only.
    let path = profile.keep();
    for _ in 0..200 {
        if std::fs::remove_dir_all(&path).is_ok() {
            shared.lock().unwrap().state.load = Load::Closed;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!("private browser profile cleanup timed out: {}", path.display())
}
