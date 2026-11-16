use super::*;

fn line(mode: &str, cores: u32, overlay_busy: f32, part: (&str, f32)) -> Line {
    Line {
        mode: mode.into(),
        logical_processors: cores,
        turns: vec![ProcessTurns { process: "overlay".into(), busy_ms_per_s: overlay_busy, parts: vec![("steamvr".into(), 9.0), (part.0.into(), part.1)], ..Default::default() }],
        ..Default::default()
    }
}

#[test]
fn a_minute_of_a_heavy_overlay_names_its_heaviest_part() {
    // 4 threads: a fifth of a core (200 ms a second here is 20 %) is too much.
    let lines: Vec<Line> = (0..6).map(|_| line("vr", 4, 210.0, ("orbit", 150.0))).collect();
    assert_eq!(hint(&lines), Some(Hint { key: "overlay_cpu".into(), part: Some("orbit".into()) }));
    // The same cost on 20 threads is under 2 % of the processor: nothing said.
    let lines: Vec<Line> = (0..6).map(|_| line("vr", 20, 210.0, ("orbit", 150.0))).collect();
    assert_eq!(hint(&lines), None);
}

#[test]
fn a_moment_of_work_says_nothing() {
    let mut lines: Vec<Line> = (0..5).map(|_| line("vr", 4, 300.0, ("orbit", 250.0))).collect();
    lines.push(line("vr", 4, 40.0, ("orbit", 10.0)));
    assert_eq!(hint(&lines), None, "one calm line in the minute");
    assert_eq!(hint(&lines[..5]), None, "less than a minute");
    let mut mixed: Vec<Line> = (0..6).map(|_| line("vr", 4, 300.0, ("orbit", 250.0))).collect();
    mixed[0].mode = "desktop".into();
    assert_eq!(hint(&mixed), None, "VR started within the minute");
}

#[test]
fn reconstruction_is_named_when_vrchat_is_short_of_graphics_time() {
    let mut l = line("vr", 8, 30.0, ("orbit", 5.0));
    l.ours.push(ProcUse { name: "orbiters-reconstruct.exe".into(), cpu: 40.0, ..Default::default() });
    l.turns[0].vrchat = Some(orbiters_booster::frames::Summary { late_gpu_share: 0.3, ..Default::default() });
    let lines = vec![l; 6];
    assert_eq!(hint(&lines).map(|h| h.key), Some("reconstruct_gpu".into()));
    let (title, advice) = hint(&lines).unwrap().words();
    assert!(title.contains("3D picture") && advice.contains("After VRChat"), "{title} / {advice}");
}

#[test]
fn uses_are_per_second_and_new_processes_count_from_their_start() {
    let a = Raw { pid: 7, name: "orbiters-desktop.exe".into(), created: 1, cpu: 1_000_000, write: 0, ..Default::default() };
    let b = Raw { cpu: 1_000_000 + 2_000_000, write: 20 * 1024 * 10, working: 100 * 1048576, ..a.clone() };
    let fresh = Raw { pid: 9, name: "VRChat.exe".into(), created: 5, cpu: 5_000_000, ..Default::default() };
    let used = uses(&[a], &[b, fresh], 10.0);
    // 0.2 s of processor in 10 s: 2 % of one core.
    assert_eq!(used[0].cpu, 2.0);
    assert_eq!(used[0].write_kb_s, 20.0);
    assert_eq!(used[0].working_mb, 100.0);
    assert_eq!(used[1].cpu, 5.0);
}

#[test]
fn a_pid_used_again_is_another_process() {
    let a = Raw { pid: 7, created: 1, cpu: 9_000_000, ..Default::default() };
    let b = Raw { pid: 7, created: 2, cpu: 1_000_000, ..Default::default() };
    assert_eq!(uses(&[a], &[b], 10.0)[0].cpu, 1.0);
}

#[test]
fn the_meter_sums_parts_per_second_and_waits_for_its_window() {
    let start = Instant::now();
    let mut m = TurnMeter::new("overlay", start);
    m.parts[Part::Orbit as usize] = Duration::from_millis(500);
    for _ in 0..900 {
        m.turn(Duration::from_millis(1));
    }
    m.turn(Duration::from_millis(80));
    assert!(m.take(start + Duration::from_secs(5)).is_none());
    let t = m.take(start + Duration::from_secs(10)).unwrap();
    assert_eq!(t.turns_per_s, 90.1);
    assert_eq!(t.slow_turns, 1);
    assert_eq!(t.longest_ms, 80.0);
    assert_eq!(t.parts, vec![("orbit".to_string(), 50.0)]);
    assert_eq!(t.busy_ms_per_s, 98.0);
    // A new window starts from nothing.
    let again = m.take(start + Duration::from_secs(20)).unwrap();
    assert_eq!(again.turns_per_s, 0.0);
    assert!(again.parts.is_empty());
}

#[test]
fn the_log_stays_small() {
    let dir = std::env::temp_dir().join(format!("orbiters-perf-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("perf.jsonl");
    let big = Line { others: vec![ProcUse { name: "x".repeat(2000), ..Default::default() }; 50], ..Default::default() };
    for _ in 0..30 {
        append(&path, &big);
    }
    let now = std::fs::metadata(&path).unwrap().len();
    let old = std::fs::metadata(dir.join("perf.jsonl.1")).unwrap().len();
    assert!(now <= LOG_MAX && old <= LOG_MAX, "{now} {old}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_keeper_keeps_an_hour_and_samples_this_computer() {
    let dir = std::env::temp_dir().join(format!("orbiters-perf-keeper-{}", std::process::id()));
    let mut k = Keeper::new(dir.join("perf.jsonl"));
    let start = Instant::now();
    assert!(k.sample(start, "idle", vec![]).is_none(), "the first scan is only compared with");
    k.turns(ProcessTurns { process: "overlay".into(), busy_ms_per_s: 12.0, ..Default::default() });
    let l = k.sample(start + Duration::from_secs(10), "vr", vec!["watch".into()]).unwrap();
    assert_eq!(l.turns.len(), 1);
    assert_eq!(l.features, vec!["watch".to_string()]);
    if cfg!(windows) {
        assert!(l.ours.iter().any(|p| p.pid == std::process::id()) || l.ours.is_empty(), "the test runs as orbiters-desktop-<hash>.exe");
        assert!(l.ram_total_mb > 0.0 && l.logical_processors > 0);
    }
    for i in 0..(KEPT + 10) {
        k.lines.push_back(Line { seconds: i as f32, ..Default::default() });
        if k.lines.len() > KEPT {
            k.lines.pop_front();
        }
    }
    assert_eq!(k.lines().len(), KEPT);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cause_is_told_once_and_never_when_muted() {
    let mut k = Keeper::new(std::env::temp_dir().join("unused.jsonl"));
    let h = Hint { key: "overlay_cpu".into(), part: None };
    assert!(k.to_tell(&h, &[]));
    assert!(!k.to_tell(&h, &[]));
    let other = Hint { key: "panels_gpu".into(), part: None };
    assert!(!k.to_tell(&other, &["panels_gpu".to_string()]));
}

#[test]
fn the_report_holds_no_paths_and_fits_the_server() {
    let snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
    let settings = report::settings(&snap).to_string();
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    assert!(!home.is_empty() && !settings.contains(&home.replace('\\', "\\\\")) && !settings.contains("sessions_dir"), "{settings}");
    let lines: Vec<Line> = (0..5000).map(|_| Line { others: vec![ProcUse { name: "a".repeat(200), ..Default::default() }], ..Default::default() }).collect();
    let fitted = report::fitted(serde_json::json!({ "lines": lines }));
    assert!(serde_json::to_vec(&fitted).unwrap().len() <= report::MOST_BYTES);
    assert!(!fitted["lines"].as_array().unwrap().is_empty());
}

#[test]
fn the_summary_reads_in_every_language() {
    let report = serde_json::json!({
        "machine": { "cpu": "Test CPU", "physical_cores": 4, "logical_processors": 4, "core_kinds": [{"cores": 2}, {"cores": 2}], "gpus": [{"name": "GPU", "dedicated_mb": 4096.0, "driver": "1.2.3.4"}], "ram_total_mb": 8192.0, "ram_free_mb": 1024.0, "displays": [{"width": 1920, "height": 1080, "hz": 60}] },
        "windows": { "os": "Windows 10", "antivirus": ["Defender"], "on_battery": true },
        "steamvr": { "drivers": ["orbiters_smooth"] },
        "programs_running": ["OVR Toolkit"],
        "features": ["watch"],
        "hint": "overlay_cpu",
        "lines": [line("vr", 4, 210.0, ("orbit", 150.0))],
    });
    for language in orbiters_i18n::Language::ALL {
        let lines = orbiters_i18n::in_language(language, || report::summary(&report));
        assert!(lines.len() >= 12, "{language:?}: {lines:?}");
        assert!(lines.iter().all(|l| !l.contains('{')), "{language:?}: a value not filled: {lines:?}");
    }
}

#[test]
fn reports_have_ids_of_their_own() {
    let a = report::report_id();
    let b = report::report_id();
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

/// What one look at every process costs, through sysinfo as the N4N0 page's footprint does every 3 s, and through the one
/// system call the self-check makes. Printed for the report (`--nocapture`); the system call must not cost more.
#[cfg(windows)]
#[test]
fn a_scan_costs_less_than_sysinfo_s_refresh() {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
    let time = |f: &mut dyn FnMut()| {
        let mut runs: Vec<f64> = (0..15)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        runs.sort_by(f64::total_cmp);
        runs[runs.len() / 2]
    };
    let sys = time(&mut || { system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory()); });
    let ours = time(&mut || drop(scan()));
    println!("one look at {} processes: sysinfo {sys:.2} ms, NtQuerySystemInformation {ours:.2} ms", scan().len());
    assert!(ours <= sys * 1.5 + 0.5, "sysinfo {sys} ms, scan {ours} ms");
}
