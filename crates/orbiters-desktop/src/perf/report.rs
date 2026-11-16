//! The Performance report (Settings › Beta feedback): the self-check's last hour ([`super::Line`]) with what this computer
//! is (processor and its kinds of cores, graphics cards and drivers, their memory, RAM, displays and their refresh rates,
//! the headset and the VR runtimes, Windows' build, power, other VR and overlay programs running, SteamVR's add-ons, the
//! antivirus) and the settings that change what N4N0 costs. Made only when the person presses the button: it watches the
//! graphics card's counters for a few seconds then. Saved in the logs' folder, shown, and sent to Orbiters only when the
//! person presses Send (`POST /n4n0/perf-reports`, no account, as the beta's usage statistics are).
//!
//! Nothing personal goes in: no paths (they hold the Windows user's name), no account, no world, no people; settings are
//! taken field by field from a list, never the whole file.

use super::Line;
use crate::state::Snapshot;
use orbiters_i18n::tr;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// The largest request sent (the server takes up to 1 MB): the oldest lines go first.
pub const MOST_BYTES: usize = 900 * 1024;

/// Programs that matter beside VRChat in VR: overlays, VR tools, streamers, recorders, other game overlays.
pub const KNOWN_PROGRAMS: &[&str] = &[
    "OVR Toolkit", "XSOverlay", "AdvancedSettings", "OVRAdvancedSettings", "SpaceCalibrator", "fpsVR", "OVRdrop", "DesktopPlus", "VRCX", "VRCFaceTracking", "VRCOSC", "VirtualDesktop.Streamer",
    "VirtualDesktop.Service", "OVRServer_x64", "OculusClient", "SlimeVR", "slimevr", "Standable", "VRChatOSCRouter", "Wallpaper64", "wallpaper32", "obs64", "Discord", "MSIAfterburner", "RTSS",
    "NVIDIA Overlay", "NVIDIA Share", "GameBar", "XboxGameBarWidgets", "Medal", "Overwolf", "Steam", "vrmonitor", "VRCVideoCacher", "ALVR", "PimaxClient", "Bigscreen", "Kinect", "Spout",
    "VoiceMeeter", "voicemeeter8x64", "NVBroadcast", "iCUE", "LGHUB", "SignalRgb", "ArmouryCrate", "lghub_agent", "Razer Synapse 3", "WMIRegistrationService",
];

/// The settings that change what N4N0 costs, each by name (never the whole file: it holds paths and words of the person's).
pub fn settings(snap: &Snapshot) -> Value {
    let s = &snap.settings;
    json!({
        "steamvr_overlay": s.steamvr_overlay,
        "orbit": { "wheel": s.orbit.wheel, "quick_menu": s.orbit.quick_menu, "camera_tiles": s.orbit.camera_tiles, "camera_pattern": s.orbit.camera_pattern,
                   "immersive_turn": s.orbit.immersive_turn, "vr_motion": s.orbit.vr_motion, "layout": s.orbit.layout, "theme": s.orbit.theme, "display": s.orbit.display, "vr_size": s.orbit.vr_size },
        "floating": { "watch": s.floating.watch },
        "n4n0": { "enabled": s.n4n0.enabled, "assistant": s.n4n0.assistant, "wake_word": s.n4n0.wake_word, "calls": s.n4n0.calls, "controller_input": s.n4n0.controller_input, "text_step_ms": s.n4n0.text_step_ms },
        "osc": { "enabled": s.osc.enabled, "record_parameters": s.osc.record_parameters, "drive_camera": s.osc.drive_camera },
        "booster": s.booster.level,
        "walk_boost": { "enabled": s.walk_boost.enabled, "through_driver": s.walk_boost.through_driver },
        "tracking": { "enabled": s.tracking.enabled, "ms": s.tracking.ms },
        "translate": { "enabled": s.translate.enabled, "provider": s.translate.provider },
        "window_gpu": s.window_gpu, "view_gpu": s.view_gpu, "reconstruct_while_playing": s.reconstruct_while_playing,
        "world_performance": s.world_performance, "rig_views": s.rig_views, "coverage_overlay": s.coverage_overlay, "dolly_streams": s.dolly_streams,
        "reduce_motion": s.reduce_motion, "vrchat_bridge": s.experimental.vrchat_bridge, "usage": s.usage.enabled, "development": s.development, "language": s.language,
    })
}

/// The report: this computer, the settings, what runs, and the self-check's lines (oldest first). Takes a few seconds (the
/// graphics card's counters are watched for three).
pub fn make(lines: &[Line], snap: &Snapshot) -> Value {
    let running = super::scan();
    let names: Vec<String> = running.iter().map(|p| p.name.trim_end_matches(".exe").to_string()).collect();
    let mut known: Vec<&str> = KNOWN_PROGRAMS.iter().copied().filter(|k| names.iter().any(|n| n.eq_ignore_ascii_case(k))).collect();
    known.dedup();
    let pids: Vec<u32> = running.iter().filter(|p| super::is_ours(&p.name) || p.name.eq_ignore_ascii_case("vrchat.exe") || p.name.eq_ignore_ascii_case("vrcompositor.exe")).map(|p| p.pid).collect();
    let by_pid: Vec<(u32, String)> = running.iter().filter(|p| pids.contains(&p.pid)).map(|p| (p.pid, p.name.clone())).collect();
    json!({
        "kind": "n4n0-performance-report",
        "version": 1,
        "app_version": orbiters_core::APP_VERSION,
        "made": orbiters_core::now_utc_iso(),
        "machine": machine(),
        "windows": windows_facts(&by_pid),
        "steamvr": steamvr_facts(),
        "vrchat_running": snap.vrchat.running,
        "steamvr_running": snap.steamvr.running,
        "headset_in_use": snap.steamvr.headset_in_use,
        "programs_running": known,
        "features": super::features(snap),
        "settings": settings(snap),
        "hint": lines.last().and_then(|l| l.hint.clone()),
        "lines": lines,
    })
}

/// The report made to fit [`MOST_BYTES`]: its oldest lines go first.
pub fn fitted(mut report: Value) -> Value {
    loop {
        let size = serde_json::to_vec(&report).map_or(usize::MAX, |v| v.len());
        let Some(lines) = report.get_mut("lines").and_then(Value::as_array_mut) else { return report };
        if size <= MOST_BYTES || lines.is_empty() {
            return report;
        }
        let drop = lines.len().div_ceil(4).max(1);
        lines.drain(..drop);
    }
}

/// Saves the report in the logs' folder (`perf-report-<time>.json`), where "Show the file" opens it.
pub fn save(report: &Value) -> std::io::Result<PathBuf> {
    let dir = orbiters_core::paths::logs_dir();
    std::fs::create_dir_all(&dir)?;
    let stamp: String = orbiters_core::now_utc_iso().chars().filter(|c| c.is_ascii_digit()).take(14).collect();
    let path = dir.join(format!("perf-report-{stamp}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(report).unwrap_or_default())?;
    Ok(path)
}

/// Sends it to Orbiters. `Err` says why, in English for the log.
pub fn send(report: &Value) -> Result<(), String> {
    let api = orbiters_core::account::Api::new(orbiters_core::account::Server::current());
    let answer = api
        .agent()
        .post(&api.url("n4n0/perf-reports"))
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_json(json!({ "report": fitted(report.clone()) }));
    match answer {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => Err(format!("Orbiters answered {}", r.status().as_u16())),
        Err(e) => Err(format!("Orbiters not reached: {e}")),
    }
}

/// What the report says, line by line, in the app's language: what the person sees before sending it.
pub fn summary(report: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let m = &report["machine"];
    let text = |v: &Value| v.as_str().unwrap_or("?").to_string();
    out.push(tr!("Processor: {name}, {cores} cores, {threads} threads", name = text(&m["cpu"]), cores = m["physical_cores"].as_u64().unwrap_or(0), threads = m["logical_processors"].as_u64().unwrap_or(0)));
    if let Some(classes) = m["core_kinds"].as_array().filter(|c| c.len() > 1) {
        let kinds: Vec<String> = classes.iter().map(|c| c["cores"].as_u64().unwrap_or(0).to_string()).collect();
        out.push(tr!("Kinds of cores (efficiency first): {kinds}", kinds = kinds.join(" + ")));
    }
    for g in m["gpus"].as_array().into_iter().flatten() {
        out.push(tr!("Graphics: {name}, {memory} GB, driver {driver}", name = text(&g["name"]), memory = format!("{:.1}", g["dedicated_mb"].as_f64().unwrap_or(0.0) / 1024.0), driver = text(&g["driver"])));
    }
    out.push(tr!("Memory: {total} GB, {free} GB free", total = format!("{:.1}", m["ram_total_mb"].as_f64().unwrap_or(0.0) / 1024.0), free = format!("{:.1}", m["ram_free_mb"].as_f64().unwrap_or(0.0) / 1024.0)));
    let displays: Vec<String> = m["displays"].as_array().into_iter().flatten().map(|d| format!("{}×{} {} Hz", d["width"], d["height"], d["hz"])).collect();
    if !displays.is_empty() {
        out.push(tr!("Displays: {list}", list = displays.join(", ")));
    }
    let w = &report["windows"];
    if let Some(os) = w["os"].as_str() {
        out.push(tr!("Windows: {os}", os = os));
    }
    if let Some(av) = w["antivirus"].as_array().filter(|a| !a.is_empty()) {
        out.push(tr!("Antivirus: {list}", list = av.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
    }
    if w["on_battery"].as_bool() == Some(true) {
        out.push(tr("Running on battery: the processor and graphics card may be slowed").to_string());
    }
    let vr = &report["steamvr"];
    if let Some(hz) = report["lines"].as_array().and_then(|l| l.iter().rev().find_map(|l| l["turns"].as_array()?.iter().find_map(|t| t["refresh_hz"].as_f64()))) {
        out.push(tr!("Headset refresh rate: {hz} Hz", hz = format!("{hz:.0}")));
    }
    let drivers: Vec<String> = vr["drivers"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
    if !drivers.is_empty() {
        out.push(tr!("SteamVR add-ons: {list}", list = drivers.join(", ")));
    }
    let programs: Vec<String> = report["programs_running"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
    if !programs.is_empty() {
        out.push(tr!("Other programs running: {list}", list = programs.join(", ")));
    }
    let lines: Vec<Line> = report["lines"].as_array().into_iter().flatten().filter_map(|l| serde_json::from_value(l.clone()).ok()).collect();
    if !lines.is_empty() {
        let minutes = lines.iter().map(|l| l.seconds).sum::<f32>() / 60.0;
        let ours = lines.iter().map(Line::ours_cpu).sum::<f32>() / lines.len() as f32;
        let busiest = lines.iter().flat_map(|l| l.ours.iter()).max_by(|a, b| a.cpu.total_cmp(&b.cpu));
        out.push(tr!("Over the last {minutes} minutes, N4N0 used {ours} % of one processor core on average", minutes = format!("{minutes:.0}"), ours = format!("{ours:.1}")));
        if let Some(p) = busiest {
            out.push(tr!("Its busiest moment: {name} at {cpu} % of a core", name = p.name.trim_end_matches(".exe"), cpu = format!("{:.0}", p.cpu)));
        }
        let frames: Vec<orbiters_booster::frames::Summary> = lines.iter().filter_map(|l| l.overlay().and_then(|o| o.vrchat)).collect();
        if !frames.is_empty() {
            let n = frames.len() as f32;
            let fps = frames.iter().map(|f| f.real_fps).sum::<f32>() / n;
            let late = 100.0 * frames.iter().map(|f| f.late_cpu_share.max(f.late_gpu_share)).sum::<f32>() / n;
            let dropped: u32 = frames.iter().map(|f| f.dropped).sum();
            out.push(tr!("VRChat in the headset: {fps} frames a second, {late} % late, {dropped} dropped", fps = format!("{fps:.0}"), late = format!("{late:.0}"), dropped = dropped));
        }
        let others: Vec<String> = lines.last().map(|l| l.others.iter().filter(|p| p.cpu >= 10.0).map(|p| format!("{} {:.0} %", p.name.trim_end_matches(".exe"), p.cpu)).collect()).unwrap_or_default();
        if !others.is_empty() {
            out.push(tr!("Other programs using the processor now: {list}", list = others.join(", ")));
        }
    } else {
        out.push(tr("No measurement yet: keep N4N0 running with VRChat for a minute, then make the report again").to_string());
    }
    let on: Vec<String> = report["features"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
    out.push(tr!("Features on: {list}", list = on.join(", ")));
    if let Some(key) = report["hint"].as_str() {
        let (title, advice) = super::Hint { key: key.to_string(), part: None }.words();
        out.push(format!("{title}. {advice}"));
    }
    out
}

/// The processor, its cores, memory, graphics cards and displays.
fn machine() -> Value {
    let mut system = sysinfo::System::new();
    system.refresh_cpu_all();
    let cpu = system.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
    let (ram_total_mb, ram_free_mb) = super::memory_mb().unwrap_or((0.0, 0.0));
    json!({
        "cpu": cpu,
        "physical_cores": sysinfo::System::physical_core_count(),
        "logical_processors": std::thread::available_parallelism().map_or(0, |n| n.get()),
        "core_kinds": core_kinds(),
        "ram_total_mb": ram_total_mb,
        "ram_free_mb": ram_free_mb,
        "gpus": gpus(),
        "displays": displays(),
    })
}

/// How many cores of each efficiency class (Windows' numbering: 0 the most efficient), when the processor has kinds.
#[cfg(windows)]
fn core_kinds() -> Value {
    use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationProcessorCore};
    let mut size = 0u32;
    // SAFETY: the first call only asks the size; the second fills a buffer of that size.
    unsafe {
        let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut size);
        if size == 0 {
            return json!([]);
        }
        let mut buffer = vec![0u8; size as usize];
        if GetLogicalProcessorInformationEx(RelationProcessorCore, Some(buffer.as_mut_ptr().cast()), &mut size).is_err() {
            return json!([]);
        }
        // Each record: Relationship (u32), Size (u32), then PROCESSOR_RELATIONSHIP { Flags (u8), EfficiencyClass (u8), … }.
        let mut classes = std::collections::BTreeMap::<u8, u32>::new();
        let mut at = 0usize;
        while at + 10 <= size as usize {
            let record = u32::from_le_bytes(buffer[at + 4..at + 8].try_into().unwrap_or([0; 4])) as usize;
            *classes.entry(buffer[at + 9]).or_default() += 1;
            if record == 0 {
                break;
            }
            at += record;
        }
        json!(classes.into_iter().map(|(class, cores)| json!({"efficiency_class": class, "cores": cores})).collect::<Vec<_>>())
    }
}

#[cfg(not(windows))]
fn core_kinds() -> Value {
    json!([])
}

/// Every graphics adapter but Windows' software one: name, its own memory, the driver's version.
#[cfg(windows)]
fn gpus() -> Value {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
    let mut out = Vec::new();
    // SAFETY: plain DXGI factory and adapter queries.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return json!(out) };
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let driver = adapter.CheckInterfaceSupport(&IDXGIDevice::IID).ok().map(|v| {
                let v = v as u64;
                format!("{}.{}.{}.{}", v >> 48, (v >> 32) & 0xffff, (v >> 16) & 0xffff, v & 0xffff)
            });
            out.push(json!({
                "name": String::from_utf16_lossy(&desc.Description[..end]).trim(),
                "vendor": format!("{:04x}", desc.VendorId),
                "dedicated_mb": desc.DedicatedVideoMemory as f64 / 1048576.0,
                "shared_mb": desc.SharedSystemMemory as f64 / 1048576.0,
                "driver": driver,
            }));
        }
    }
    json!(out)
}

#[cfg(not(windows))]
fn gpus() -> Value {
    json!([])
}

/// Each display's size and refresh rate now.
#[cfg(windows)]
fn displays() -> Value {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, ENUM_CURRENT_SETTINGS};
    let mut out = Vec::new();
    // SAFETY: structures with their sizes set, as the calls ask.
    unsafe {
        let mut i = 0;
        loop {
            let mut device = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !EnumDisplayDevicesW(PCWSTR::null(), i, &mut device, 0).as_bool() {
                break;
            }
            i += 1;
            if device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
            if EnumDisplaySettingsW(PCWSTR(device.DeviceName.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode).as_bool() {
                out.push(json!({ "width": mode.dmPelsWidth, "height": mode.dmPelsHeight, "hz": mode.dmDisplayFrequency }));
            }
        }
    }
    json!(out)
}

#[cfg(not(windows))]
fn displays() -> Value {
    json!([])
}

/// What Windows says through its management tools, in one PowerShell call of at most 40 s: its name and build, the
/// antivirus, the power plan and battery, the OpenXR runtime, and three seconds of the graphics card's counters for
/// Orbiters', VRChat's and SteamVR's compositor's processes (`by_pid`).
#[cfg(windows)]
fn windows_facts(by_pid: &[(u32, String)]) -> Value {
    use std::os::windows::process::CommandExt;
    let pids = by_pid.iter().map(|(p, _)| p.to_string()).collect::<Vec<_>>().join(",");
    let script = format!(
        r#"$ErrorActionPreference='SilentlyContinue'
$o=[ordered]@{{}}
$os=Get-CimInstance Win32_OperatingSystem
$o.os="$($os.Caption) $($os.Version)"
$r=Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$o.build="$($r.DisplayVersion) $($r.CurrentBuild).$($r.UBR)"
$o.antivirus=@(Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct | ForEach-Object {{ $_.displayName }})
$o.power_plan=((powercfg /getactivescheme) -replace '^.*\(','' -replace '\)\s*$','')
$b=Get-CimInstance Win32_Battery
$o.battery=[bool]$b
$o.on_battery=[bool]($b -and $b.BatteryStatus -eq 1)
$o.openxr_runtime=(Get-ItemProperty 'HKLM:\SOFTWARE\Khronos\OpenXR\1').ActiveRuntime -replace '^.*\\',''
$o.game_mode=(Get-ItemProperty 'HKCU:\Software\Microsoft\GameBar').AutoGameModeEnabled
$o.hags=(Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers').HwSchMode
$pids=@({pids})
$c=Get-Counter -Counter '\GPU Engine(*)\Utilization Percentage','\GPU Process Memory(*)\Dedicated Usage','\GPU Adapter Memory(*)\Dedicated Usage' -SampleInterval 1 -MaxSamples 3
$g=@{{}}
foreach($s in $c.CounterSamples){{ if($s.InstanceName -match 'pid_(\d+)_'){{ $p=[int]$Matches[1]; if($pids -contains $p){{ $k="$p"; if(-not $g[$k]){{ $g[$k]=@{{gpu_percent=0.0;dedicated_mb=0.0}} }}; if($s.Path -like '*utilization percentage'){{ $g[$k].gpu_percent+=$s.CookedValue/3 }} else {{ $g[$k].dedicated_mb=[math]::Max($g[$k].dedicated_mb,$s.CookedValue/1MB) }} }} }} elseif($s.Path -like '*gpu adapter memory*'){{ $o.adapter_dedicated_used_mb=[math]::Max([double]$o.adapter_dedicated_used_mb,$s.CookedValue/1MB) }} }}
$o.gpu_by_pid=$g
$o | ConvertTo-Json -Depth 4 -Compress"#
    );
    let child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(0x0800_0000)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return json!({}) };
    let began = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if began.elapsed() < Duration::from_secs(40) => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                return json!({ "error": "timed out" });
            }
        }
    }
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        use std::io::Read;
        let _ = out.read_to_string(&mut text);
    }
    let mut v: Value = serde_json::from_str(text.trim()).unwrap_or_else(|_| json!({}));
    // The counters by process name, not by pid alone: a pid means nothing to the reader.
    if let Some(g) = v.get_mut("gpu_by_pid").and_then(Value::as_object_mut) {
        for (pid, entry) in g.iter_mut() {
            if let Some(name) = by_pid.iter().find(|(p, _)| p.to_string() == *pid).map(|(_, n)| n.clone()) {
                entry["name"] = json!(name);
            }
        }
    }
    v
}

#[cfg(not(windows))]
fn windows_facts(_: &[(u32, String)]) -> Value {
    json!({})
}

/// SteamVR's drivers (its own and the add-ons registered with it) and the headset's model as SteamVR's settings remember it.
fn steamvr_facts() -> Value {
    let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return json!({}) };
    let Ok(text) = std::fs::read_to_string(base.join("openvr").join("openvrpaths.vrpath")) else { return json!({}) };
    let paths: Value = serde_json::from_str(&text).unwrap_or_default();
    let folder_name = |p: &str| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned());
    let mut drivers: Vec<String> = paths["external_drivers"].as_array().into_iter().flatten().filter_map(Value::as_str).filter_map(folder_name).collect();
    if let Some(runtime) = paths["runtime"].as_array().and_then(|r| r.first()).and_then(Value::as_str) {
        if let Ok(dir) = std::fs::read_dir(std::path::Path::new(runtime).join("drivers")) {
            drivers.extend(dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !matches!(n.as_str(), "gamepad" | "htc" | "indexhmd" | "lighthouse" | "null" | "oculus" | "vrlink" | "generic_hmd")));
        }
    }
    drivers.sort();
    drivers.dedup();
    json!({ "drivers": drivers })
}
