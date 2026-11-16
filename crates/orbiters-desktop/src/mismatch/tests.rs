use super::*;

#[test]
fn the_ring_keeps_the_last_forty_seconds_and_never_grows_past_its_bound() {
    let mut ring = Ring::default();
    let t0 = 1_800_000_000_000u64;
    // A line every 10 ms for two minutes.
    for i in 0..12_000u64 {
        ring.push(t0 + i * 10, "input", format!("line {i}"));
    }
    assert!(ring.len() <= MOST, "bounded: {}", ring.len());
    let last = t0 + 11_999 * 10;
    let kept = ring.between(0, u64::MAX);
    assert_eq!(kept.last().unwrap().text, "line 11999");
    // At a line every 10 ms, 6,000 lines are 60 s: the 40 s keep is what bounds it.
    assert!(kept.first().unwrap().at >= last - KEEP.as_millis() as u64, "older than KEEP let go");
    assert!(kept.windows(2).all(|w| w[0].at <= w[1].at), "in time order");
    // Many lines in a moment: the count bounds it, the oldest go.
    let mut burst = Ring::default();
    for i in 0..(MOST as u64 + 500) {
        burst.push(t0, "log", format!("{i}"));
    }
    assert_eq!(burst.len(), MOST);
    assert_eq!(burst.between(0, u64::MAX)[0].text, "500");
}

#[test]
fn a_report_holds_thirty_seconds_before_the_press_and_a_few_after() {
    let mut ring = Ring::default();
    let pressed = 1_800_000_100_000u64;
    for s in -38i64..=8 {
        ring.push((pressed as i64 + s * 1000) as u64, "decide", format!("at {s}s"));
    }
    let lines = ring.between(pressed - BEFORE.as_millis() as u64, pressed + AFTER.as_millis() as u64);
    let context = Context { surface: "vr".into(), controllers: vec![Some("oculus_touch".into()), Some("oculus_touch".into())], bindings: vec![("oculus_touch".into(), "ours".into())], state: vec![("open".into(), "Wheel(1)".into())] };
    let report = build(Kind::WheelShouldBeOn, "abc".into(), pressed, context, serde_json::json!({"orbit": {}}), lines, Vec::new(), true);
    let ts: Vec<i64> = report.timeline.iter().map(|e| e.t).collect();
    assert_eq!(ts.first(), Some(&-30_000));
    assert_eq!(ts.last(), Some(&4_000));
    assert_eq!(report.timeline.len(), 35);
    assert_eq!(report.kind, Kind::WheelShouldBeOn);
    assert_eq!(report.format, FORMAT);
    assert_eq!(report.build, env!("ORBITERS_VERSION"));
    assert_eq!(report.picture.as_deref(), Some(PICTURE));
    // The JSON an agent reads: the kind by its key, and every part named.
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["kind"], "wheel-should-be-on");
    for field in ["pressed_at", "pressed_ms", "surface", "controllers", "bindings", "state", "settings", "timeline", "vrchat_log", "language", "os"] {
        assert!(json.get(field).is_some(), "{field}");
    }
}

#[test]
fn a_written_report_is_found_counted_and_deleted() {
    let root = std::env::temp_dir().join(format!("menu-reports-test-{}", uuid::Uuid::new_v4().simple()));
    let report = build(Kind::MenuShouldBeOff, "id1".into(), 1_800_000_000_000, Context { surface: "desktop".into(), ..Default::default() }, serde_json::Value::Null, Vec::new(), Vec::new(), true);
    let picture = Picture::small(800, 600, &vec![200u8; 800 * 600 * 4]).unwrap();
    let dir = root.join("2027-01-15T08-00-00Z-menu-should-be-off-id1");
    write(&dir, &report, Some(&picture)).unwrap();
    // A folder still being written is not a report.
    std::fs::create_dir_all(root.join("2027-01-15T08-00-01Z-wheel-should-be-on-x")).unwrap();
    let kept = kept_in(&root);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].kind, Some(Kind::MenuShouldBeOff));
    assert!(!kept[0].sent);
    let back: Report = serde_json::from_slice(&std::fs::read(dir.join(REPORT)).unwrap()).unwrap();
    assert_eq!(back.id, "id1");
    let png = image::open(dir.join(PICTURE)).unwrap();
    assert_eq!((png.width(), png.height()), (400, 300));
    std::fs::write(dir.join(SENT), "x").unwrap();
    assert!(kept_in(&root)[0].sent);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_picture_is_small_and_blurred() {
    // A 1 px checkerboard: names drawn this fine are gone at the picture's size.
    let (w, h) = (2016u32, 2240u32);
    let rgba: Vec<u8> = (0..w * h).flat_map(|i| if (i % w + i / w) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
    let p = Picture::small(w, h, &rgba).unwrap();
    assert!(p.width <= PICTURE_WIDTH, "{}", p.width);
    assert!(p.rgba.chunks_exact(4).all(|c| (100..=155).contains(&c[0])), "fine detail averaged away");
    assert!(Picture::small(0, 10, &[]).is_none());
}

#[test]
fn vrchat_s_log_gives_menus_and_worlds_never_people() {
    let keep = [
        ("2026.11.16 10:22:31 Log        -  [Behaviour] Joining wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b:12345~private(usr_aaaa)~region(eu)", "[Behaviour] Joining wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b"),
        ("2026.11.16 10:22:32 Log        -  [Behaviour] Entering Room: The Black Cat", "[Behaviour] Entering Room: The Black Cat"),
        ("2026.11.16 10:22:33 Log        -  [UiManager] QuickMenu opened", "[UiManager] QuickMenu opened"),
    ];
    for (line, said) in keep {
        assert_eq!(vrchat_said(line).as_deref(), Some(said), "{line}");
    }
    for line in [
        "2026.11.16 10:22:33 Log        -  [Behaviour] OnPlayerJoined Somebody (usr_1234)",
        "2026.11.16 10:22:33 Log        -  [Behaviour] Initialized PlayerAPI \"Somebody\" is remote",
        "2026.11.16 10:22:33 Log        -  [Network] Some unrelated line about textures",
        "2026.11.16 10:22:33 Log        -  [ModerationManager] friend request from someone opened in menu",
    ] {
        assert_eq!(vrchat_said(line), None, "{line}");
    }
    // Only the report's time, read in VRChat's local time.
    let offset = time::UtcOffset::from_hms(1, 0, 0).unwrap();
    // 10:22:33 at +01:00 is 09:22:33 UTC.
    let pressed = time::macros::datetime!(2026-11-16 09:22:33 UTC).unix_timestamp() as u64 * 1000;
    let text = "2026.11.16 10:21:00 Log        -  [UiManager] QuickMenu opened\n2026.11.16 10:22:30 Log        -  [UiManager] QuickMenu closed\n2026.11.16 10:22:36 Log        -  [UiManager] ActionMenu opened\n2026.11.16 10:22:50 Log        -  [UiManager] QuickMenu opened\nnot a line";
    let got = vrchat_lines_in(text, pressed, offset);
    assert_eq!(got.iter().map(|e| e.t).collect::<Vec<_>>(), [-3000, 3000]);
}

#[test]
fn a_report_too_big_loses_its_oldest_lines_then_its_picture() {
    let lines: Vec<Line> = (0..6_000u64).map(|i| Line { at: 1_800_000_000_000 - 30_000 + i * 5, source: "log".into(), text: "x".repeat(250) }).collect();
    let report = build(Kind::QuickMenuShouldBeOn, "big".into(), 1_800_000_000_000, Context::default(), serde_json::Value::Null, lines, Vec::new(), true);
    let body = send::body(report, Some(vec![7u8; 200 * 1024]));
    let size = serde_json::to_vec(&body).unwrap().len();
    assert!(size <= send::MOST_BYTES, "{size}");
    assert!(body["picture"].is_string(), "the picture stays while old lines can go");
    let timeline = body["report"]["timeline"].as_array().unwrap();
    assert!(timeline.last().unwrap()["t"].as_i64().unwrap() > -1_000, "the lines next to the press stay");
}

#[test]
fn every_kind_has_its_own_words_key_and_key_on_the_desktop() {
    let mut keys = std::collections::HashSet::new();
    let mut digits = std::collections::HashSet::new();
    for k in Kind::ALL {
        assert!(keys.insert(k.key()));
        assert!(digits.insert(k.function_key()));
        assert!((7..=12).contains(&k.function_key()), "F7 onwards: F1 to F5 are VRChat's own, Alt+F4 closes it");
        assert_eq!(Kind::from_key(k.key()), Some(k));
        assert!(!k.words().is_empty());
    }
    assert_eq!(Kind::ALL.map(Kind::words), ["Menu should be off", "Wheel should be on", "Quick menu should be on"]);
}

#[test]
fn nothing_is_kept_while_the_switch_is_off() {
    // `note` keeps nothing in tests (they run side by side); the switch's own rule: turning it off empties the ring.
    ring(|r| r.push(1, "x", "y".into()));
    set_on(true);
    set_on(false);
    assert!(ring(|r| r.is_empty()));
    assert!(!on());
}

/// Send reports as the Settings card's button does (`send::unsent`), against a server under test: `ORBITERS_MENU_REPORTS_API` (e.g.
/// `http://127.0.0.1:4100/`) and a folder of real reports in `ORBITERS_MENU_REPORTS_DIR` (copied first: the folder itself is not marked).
/// Run with `--ignored`.
#[test]
#[ignore]
fn kept_reports_reach_a_server_under_test() {
    let (Some(base), Some(from)) = (std::env::var_os("ORBITERS_MENU_REPORTS_API"), std::env::var_os("ORBITERS_MENU_REPORTS_DIR")) else {
        panic!("set ORBITERS_MENU_REPORTS_API and ORBITERS_MENU_REPORTS_DIR");
    };
    let root = std::env::temp_dir().join(format!("menu-reports-send-{}", uuid::Uuid::new_v4().simple()));
    for k in kept_in(std::path::Path::new(&from)) {
        let to = root.join(k.dir.file_name().unwrap());
        std::fs::create_dir_all(&to).unwrap();
        for f in [REPORT, PICTURE] {
            let _ = std::fs::copy(k.dir.join(f), to.join(f));
        }
    }
    let kept = kept_in(&root);
    assert!(!kept.is_empty(), "reports to send");
    let out = send::unsent(&orbiters_core::account::Api::at(&base.to_string_lossy()), &kept);
    eprintln!("sent {} of {}: {out:?}", out.sent, kept.len());
    assert_eq!(out.sent, kept.len(), "{out:?}");
    assert!(kept_in(&root).iter().all(|k| k.sent), "each marked sent");
    assert_eq!(send::unsent(&orbiters_core::account::Api::at(&base.to_string_lossy()), &kept_in(&root)).sent, 0, "nothing sent twice");
    let _ = std::fs::remove_dir_all(&root);
}
