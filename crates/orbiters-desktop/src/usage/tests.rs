use super::spool::{Event, Sent, Spool, MAX_KEPT};
use super::*;
use crate::ipc::Role;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

#[test]
fn names_keep_nothing_but_names() {
    assert_eq!(stable_name("Page(Key('a'))"), "Page.Key", "a key typed is never in a name");
    assert_eq!(stable_name("Capture(3)"), "Capture");
    assert_eq!(stable_name("Invite(2, 1)"), "Invite");
    assert_eq!(stable_name("Set(Language(true))"), "Set.Language.true");
    assert_eq!(stable_name(r#"WorldPanel(GoTo { world: "wrld_1234-abcd", instance: "86271~private(usr_x)" })"#), "WorldPanel.GoTo");
    assert_eq!(stable_name(r#"WorldPanel(Feedback(Comment("j'adore \"ce\" monde")))"#), "WorldPanel.Feedback.Comment");
    assert_eq!(stable_name(r#"WorldPanel(OpenWeb("https://vrchat.com/home/world/wrld_1"))"#), "WorldPanel.OpenWeb");
    assert_eq!(stable_name("Menu(Suggestion(4))"), "Menu.Suggestion");
    assert_eq!(stable_name("Pictures"), "Pictures");
    // The last guard, for whatever reaches it.
    for bad in ["https://orbiters.cc/x", "see www.example.com", "wrld_123456", "wrld_ab", "4cf554b4-430c-4f8d", "me@example.com", "a\nb", "", &"x".repeat(MAX_NAME + 1)] {
        assert_eq!(clean(bad), None, "{bad:?}");
    }
    assert_eq!(clean("Open in browser"), Some("Open in browser"));
    assert_eq!(clean("qm/Menu/Music"), Some("qm/Menu/Music"));
}

#[test]
fn notes_where_the_person_goes_and_what_they_press() {
    reset();
    set_enabled(false);
    enter("Home");
    press("Settings");
    assert!(take().is_empty(), "off: nothing noted");
    set_enabled(true);
    enter("Home");
    enter("Home");
    press("Open Settings");
    enter("Settings");
    press("Beta usage statistics");
    let steps = take();
    let seen: Vec<(Kind, &str, Option<&str>, Option<&str>)> = steps.iter().map(|s| (s.kind, s.screen.as_str(), s.from.as_deref(), s.control.as_deref())).collect();
    assert_eq!(
        seen,
        vec![
            (Kind::Enter, "Home", None, None),
            (Kind::Press, "Home", None, Some("Open Settings")),
            (Kind::Enter, "Settings", Some("Home"), None),
            (Kind::Press, "Settings", None, Some("Beta usage statistics")),
        ]
    );
    assert!(take().is_empty(), "taken once");
    // Turned off, what waited is gone at once.
    press("Save");
    set_enabled(false);
    assert!(take().is_empty());
    reset();
}

#[test]
fn the_window_counts_controls_by_their_english_in_every_language() {
    reset();
    set_enabled(true);
    orbiters_ui::widgets::on_press(widget_pressed);
    for language in orbiters_i18n::Language::ALL {
        orbiters_i18n::in_language(language, || orbiters_ui::widgets::note_press(orbiters_i18n::tr("Beta usage statistics")));
    }
    // A label that is not a text of the app (a world's, a person's name) is never counted.
    orbiters_ui::widgets::note_press("Café de Nuit");
    orbiters_ui::widgets::note_press("blackorbit");
    let controls: Vec<String> = take().into_iter().filter_map(|s| s.control).collect();
    assert_eq!(controls, vec!["Beta usage statistics"; 5]);
    set_enabled(false);
    reset();
}

fn step(kind: Kind, screen: &str, from: Option<&str>, control: Option<&str>) -> Step {
    Step { at: 1_700_000_000_000, kind, screen: screen.into(), from: from.map(Into::into), control: control.map(Into::into) }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("orbiters-usage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_link_is_a_session_from_its_first_step_to_its_end() {
    let dir = temp_dir("session");
    let spool = Spool::default();
    spool.open(dir.clone());
    spool.linked(1, Role::Overlay);
    spool.linked(2, Role::Window);
    spool.steps(1, vec![step(Kind::Enter, "qm", Some("closed"), None), step(Kind::Press, "qm", None, Some("Worlds")), step(Kind::Enter, "qm/Worlds", Some("qm"), None)], true);
    // A client cannot make a name of something else.
    spool.steps(1, vec![step(Kind::Press, "qm/Worlds", None, Some("https://vrchat.com/home/world/wrld_1"))], true);
    spool.unlinked(1, true);
    // A link that noted nothing has no session.
    spool.unlinked(2, true);
    let events = spool.waiting();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["start", "enter", "press", "enter", "end"]);
    assert!(events.iter().all(|e| e.surface == "vr" && e.session == events[0].session && e.build == env!("ORBITERS_VERSION")));
    assert_eq!(events[4].screen.as_deref(), Some("qm/Worlds"), "where it ended");
    assert_eq!(events[3].from.as_deref(), Some("qm"));
    // Kept on disk: a new run sends what the last one could not.
    let again = Spool::default();
    again.open(dir.clone());
    assert_eq!(again.waiting(), events);
    // Turned off: all of it forgotten, files too.
    again.forget();
    assert!(again.waiting().is_empty() && !dir.join("waiting.jsonl").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_is_kept_while_off_and_the_disk_stays_bounded() {
    let dir = temp_dir("bounded");
    let spool = Spool::default();
    spool.open(dir.clone());
    spool.linked(1, Role::Window);
    spool.steps(1, vec![step(Kind::Enter, "Home", None, None)], false);
    assert!(spool.waiting().is_empty(), "off");
    let many: Vec<Step> = (0..MAX_KEPT + 500).map(|i| step(Kind::Press, "Home", None, Some(if i % 2 == 0 { "Save" } else { "Cancel" }))).collect();
    spool.steps(1, many, true);
    assert_eq!(spool.waiting().len(), MAX_KEPT);
    let on_disk = std::fs::read_to_string(dir.join("waiting.jsonl")).unwrap().lines().count();
    assert_eq!(on_disk, MAX_KEPT);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server that answers each request with `status` and keeps what it was sent.
fn server(status: u16) -> (String, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let got = Arc::new(std::sync::Mutex::new(Vec::new()));
    let keep = got.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            let mut line = String::new();
            let mut path = String::new();
            let mut auth = None;
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if path.is_empty() {
                    path = line.split_whitespace().nth(1).unwrap_or_default().to_string();
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                if line.to_ascii_lowercase().starts_with("authorization:") {
                    auth = Some(line["authorization:".len()..].trim().to_string());
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let mut value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            value["path"] = path.into();
            if let Some(auth) = auth {
                value["authorization"] = auth.into();
            }
            keep.lock().unwrap().push(value);
            let mut out = stream;
            let _ = write!(out, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
        }
    });
    (base, got)
}

fn filled(dir: &std::path::Path) -> Spool {
    let spool = Spool::default();
    spool.open(dir.to_path_buf());
    spool.linked(7, Role::Orbit);
    spool.steps(7, vec![step(Kind::Enter, "wheel", Some("closed"), None), step(Kind::Press, "wheel", None, Some("Pictures"))], true);
    spool
}

#[test]
fn orbiters_takes_them_refuses_them_or_is_not_there() {
    // Taken: gone from the waiting list, and what went holds the fields of the contract and nothing else.
    let dir = temp_dir("taken");
    let spool = filled(&dir);
    let (base, got) = server(200);
    assert_eq!(spool.send(&orbiters_core::account::Api::at(&base), None), Sent::Taken(3));
    assert!(spool.waiting().is_empty());
    let sent = got.lock().unwrap()[0].clone();
    assert_eq!(sent["path"], "/n4n0/metrics");
    let install = sent["install"].as_str().unwrap();
    assert!(install.len() == 32 && install.bytes().all(|b| b.is_ascii_hexdigit()), "a random id: {install}");
    let allowed = ["session", "surface", "build", "os", "lang", "at", "kind", "screen", "from", "control", "debug", "sandbox", "agent"];
    for event in sent["events"].as_array().unwrap() {
        for key in event.as_object().unwrap().keys() {
            assert!(allowed.contains(&key.as_str()), "{key} is not a field of the contract");
        }
        assert_eq!(event["surface"], "vrchat-desktop");
    }
    assert!(sent.as_object().unwrap().keys().all(|k| ["install", "events", "path"].contains(&k.as_str())), "no account, no token: {sent}");
    let _: Vec<Event> = serde_json::from_value(sent["events"].clone()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    // Refused (an older server without the route): dropped, without a word.
    let dir = temp_dir("refused");
    let spool = filled(&dir);
    let (base, _) = server(404);
    assert_eq!(spool.send(&orbiters_core::account::Api::at(&base), None), Sent::Dropped(3));
    assert!(spool.waiting().is_empty());
    let _ = std::fs::remove_dir_all(&dir);

    // Busy, or not there at all: kept for later.
    let dir = temp_dir("kept");
    let spool = filled(&dir);
    let (base, _) = server(503);
    assert_eq!(spool.send(&orbiters_core::account::Api::at(&base), None), Sent::Kept);
    assert_eq!(spool.send(&orbiters_core::account::Api::at("http://127.0.0.1:9/"), None), Sent::Kept);
    assert_eq!(spool.waiting().len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The performance figures (`usage::perf`): windows wait beside the events, on disk, go in the same request, and
/// are forgotten with them; while the statistics are off nothing is summed.
#[test]
fn performance_windows_go_with_the_events_and_are_forgotten_with_them() {
    use super::perf::{Context, Sample};
    let dir = temp_dir("perf");
    let spool = filled(&dir);
    let (sessions, screens) = spool.context();
    assert_eq!(sessions.len(), 1);
    assert_eq!(screens.get("vrchat-desktop").map(String::as_str), Some("wheel"), "the screen the session is on");
    let sample = |at: u64, screen: &str| Sample {
        at,
        seconds: 10.0,
        context: Context { mode: "desktop".into(), screens: [("vrchat-desktop".to_string(), screen.to_string())].into(), sessions: sessions.clone(), active: vec![] },
        cpu_system: 20.0,
        cores: 8,
        ..Default::default()
    };
    spool.perf(sample(1_000_000, "wheel"), false);
    spool.perf(sample(1_010_000, "wheel"), false);
    spool.perf(sample(1_020_000, "wheel/Pictures"), false);
    assert!(spool.perf_waiting().is_empty(), "off: nothing summed");
    spool.origin(true);
    spool.perf(sample(1_000_000, "wheel"), true);
    spool.perf(sample(1_010_000, "wheel"), true);
    spool.perf(sample(1_020_000, "wheel/Pictures"), true);
    let waiting = spool.perf_waiting();
    assert_eq!(waiting.len(), 1, "the window closed by the change of screen");
    assert!(waiting[0].agent && waiting[0].samples == 2 && waiting[0].sessions == sessions);
    // Kept on disk.
    let again = Spool::default();
    again.open(dir.clone());
    assert_eq!(again.perf_waiting(), waiting);
    // One request: the events and the window.
    let (base, got) = server(200);
    assert_eq!(spool.send(&orbiters_core::account::Api::at(&base), None), Sent::Taken(4));
    assert!(spool.perf_waiting().is_empty() && spool.waiting().is_empty());
    let sent = got.lock().unwrap()[0].clone();
    assert!(sent.get("team").is_none() && sent.get("authorization").is_none(), "no account code, no credential: {sent}");
    assert_eq!(sent["perf"].as_array().unwrap().len(), 1);
    assert_eq!(sent["perf"][0]["screens"]["vrchat-desktop"], "wheel");
    assert_eq!(sent["perf"][0]["cpu_system"]["mean"], 20.0);
    // Forgotten with the events, file too.
    spool.perf(sample(1_030_000, "wheel"), true);
    spool.perf(sample(1_100_000, "wheel"), true);
    assert_eq!(spool.perf_waiting().len(), 2, "the Pictures window, then a minute of the wheel");
    spool.forget();
    assert!(spool.perf_waiting().is_empty() && !dir.join("perf-waiting.jsonl").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 2026-11-19 (Enzo: "identify the usages by user"): what was counted before the notice that says it is linked to the account goes
/// without a credential and under the install id of before; what was counted after it, signed in, goes with that account's own
/// credential (the server learns the account from it) under a new install id; a session never spans two stamps; signed out, or for
/// another account or another server, no credential goes; the stamps never leave the computer.
#[test]
fn only_what_was_counted_after_the_new_notice_goes_with_the_account() {
    use super::spool::{Signed, Stamp};
    let dir = temp_dir("who");
    let spool = Spool::default();
    spool.open(dir.clone());
    spool.linked(7, Role::Window);
    let (base, got) = server(200);
    let api = orbiters_core::account::Api::at(&base);
    let here = || Some(("42".to_string(), api.url("")));
    // Signed in all along, the old notice only: the account is not even kept.
    spool.collecting(Stamp::now(false, here()));
    spool.steps(7, vec![step(Kind::Enter, "Home", None, None)], true);
    assert!(spool.waiting().iter().all(|e| e.who.is_none()), "{:?}", spool.waiting());
    // The new notice closed: the session ends under the old stamp and starts again under a new id.
    spool.collecting(Stamp::now(true, here()));
    spool.steps(7, vec![step(Kind::Enter, "Settings", Some("Home"), None), step(Kind::Press, "Settings", None, Some("Beta usage statistics"))], true);
    // Signed out: again a new session, linked to nobody.
    spool.collecting(Stamp::now(true, None));
    spool.steps(7, vec![step(Kind::Enter, "Home", Some("Settings"), None)], true);
    let kinds: Vec<String> = spool.waiting().iter().map(|e| format!("{} {}", e.kind, e.who.account.as_deref().unwrap_or(if e.who.told { "told" } else { "old" }))).collect();
    assert_eq!(kinds, ["start old", "enter old", "end old", "start 42", "enter 42", "press 42", "end 42", "start told", "enter told"]);
    // What waits on disk keeps its stamp.
    let again = Spool::default();
    again.open(dir.clone());
    assert_eq!(again.waiting(), spool.waiting());

    let signed = Signed { account: "42".into(), base: api.url(""), token: "orbit-secret".into() };
    while let Sent::Taken(_) = spool.send(&api, Some(&signed)) {}
    assert!(spool.waiting().is_empty());
    let sent = got.lock().unwrap().clone();
    assert_eq!(sent.len(), 3, "one request per stamp: {sent:?}");
    let sessions = |v: &serde_json::Value| v["events"].as_array().unwrap().iter().map(|e| e["session"].as_str().unwrap().to_string()).collect::<std::collections::BTreeSet<_>>();
    let (old, linked, nobody) = (&sent[0], &sent[1], &sent[2]);
    assert_eq!(old["events"].as_array().unwrap().len(), 3);
    assert!(old.get("authorization").is_none(), "counted under the old notice: no credential, ever");
    assert_eq!(linked["authorization"], "Bearer orbit-secret");
    assert_eq!(linked["events"].as_array().unwrap().len(), 4);
    assert!(nobody.get("authorization").is_none(), "signed out: nobody");
    assert_ne!(old["install"], linked["install"], "a new install id after the notice: the old rows are not joined to the account");
    assert_eq!(linked["install"], nobody["install"]);
    assert!(sessions(old).is_disjoint(&sessions(linked)) && sessions(linked).is_disjoint(&sessions(nobody)), "no session spans two stamps");
    for request in &sent {
        let text = request.to_string();
        let mut body = request.clone();
        body.as_object_mut().unwrap().remove("authorization");
        assert!(!body.to_string().contains("orbit-secret") && !body.to_string().contains("\"42\""), "neither credential nor account id in the body: {text}");
        assert!(request["events"].as_array().unwrap().iter().all(|e| e.get("who").is_none()), "the stamp stays here: {text}");
    }
    // Nothing counted before the notice waits any more: the old install id is gone from the computer.
    assert!(!dir.join("install-id").exists() && dir.join("install-id-2").exists());

    // Stamped for 42, but now another account is signed in, or the credential is for another server: no credential goes.
    spool.collecting(Stamp::now(true, here()));
    spool.steps(7, vec![step(Kind::Enter, "Worlds", None, None)], true);
    let other = Signed { account: "43".into(), ..signed.clone() };
    while let Sent::Taken(_) = spool.send(&api, Some(&other)) {}
    spool.steps(7, vec![step(Kind::Enter, "Home", Some("Worlds"), None)], true);
    let elsewhere = Signed { base: "https://orbiters.cc/api/".into(), ..signed.clone() };
    while let Sent::Taken(_) = spool.send(&api, Some(&elsewhere)) {}
    assert!(spool.waiting().is_empty());
    let sent = got.lock().unwrap().clone();
    let later: usize = sent[3..].iter().map(|r| r["events"].as_array().unwrap().len()).sum();
    assert_eq!(later, 4, "the signed-out session's end, then Worlds' start and enter, then Home: {:?}", &sent[3..]);
    assert!(sent[3..].iter().all(|r| r.get("authorization").is_none()), "{:?}", &sent[3..]);
    // Night4 (verify2): counted for account 42 of ANOTHER server (the other environment's 42 is another person), sent here
    // while this server's 42 is signed in: no credential, never that person's.
    spool.collecting(Stamp::now(true, Some(("42".into(), "http://127.0.0.1:4100/".into()))));
    spool.steps(7, vec![step(Kind::Enter, "Videos", None, None)], true);
    while let Sent::Taken(_) = spool.send(&api, Some(&signed)) {}
    let sent = got.lock().unwrap().clone();
    assert!(sent.last().unwrap().get("authorization").is_none(), "another server's 42 is not this one's: {:?}", sent.last());
    // A stamp from before stamps named their server: never with a credential.
    assert!(!Stamp { told: true, account: Some("42".into()), server: None }.sends_as(&signed, &api.url("")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The performance windows follow the same rule: a window sums samples of one stamp, and goes in the batch of its stamp.
#[test]
fn performance_windows_are_split_by_whom_they_were_counted_for() {
    use super::perf::{Context, Sample};
    use super::spool::{Signed, Stamp};
    let dir = temp_dir("who-perf");
    let spool = Spool::default();
    spool.open(dir.clone());
    let sample = |at: u64| Sample { at, seconds: 10.0, context: Context { mode: "idle".into(), ..Default::default() }, cpu_system: 5.0, cores: 8, ..Default::default() };
    let (base, got) = server(200);
    let api = orbiters_core::account::Api::at(&base);
    let here = || Some(("42".to_string(), api.url("")));
    spool.collecting(Stamp::now(false, here()));
    spool.perf(sample(1_000_000), true);
    spool.perf(sample(1_010_000), true);
    spool.collecting(Stamp::now(true, here()));
    spool.perf(sample(1_020_000), true);
    spool.perf(sample(1_030_000), true);
    spool.collecting(Stamp::now(true, None));
    spool.perf(sample(1_040_000), true);
    let waiting = spool.perf_waiting();
    assert_eq!(waiting.len(), 2, "closed at each change of stamp");
    assert_eq!((waiting[0].samples, waiting[0].who.clone()), (2, Stamp::default()));
    assert_eq!((waiting[1].samples, waiting[1].who.account.as_deref()), (2, Some("42")));
    let signed = Signed { account: "42".into(), base: api.url(""), token: "orbit-secret".into() };
    while let Sent::Taken(_) = spool.send(&api, Some(&signed)) {}
    let sent = got.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].get("authorization").is_none());
    assert_eq!(sent[1]["authorization"], "Bearer orbit-secret");
    assert!(sent.iter().all(|r| r["perf"][0].get("who").is_none()), "{sent:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Opt-in, against a server under test (`ORBITERS_USAGE_API=http://127.0.0.1:4100 cargo test -p orbiters-desktop -- --ignored
/// performance_reaches --nocapture`): this computer's real samples (the self-check's system call and Windows' GPU counters, as the
/// service takes them) summed into windows by the real spool while a session goes from the Quick Menu to Worlds, and sent over HTTP
/// as the app sends them. Prints what each part of a sample cost. `ORBITERS_PERF_EVERY` (seconds, default 10) and
/// `ORBITERS_PERF_SAMPLES` (default 9) change the pace.
#[test]
#[ignore]
fn performance_reaches_a_server_under_test() {
    use std::time::{Duration, Instant};
    let Ok(base) = std::env::var("ORBITERS_USAGE_API") else { return };
    let every = std::env::var("ORBITERS_PERF_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(10u64);
    let samples = std::env::var("ORBITERS_PERF_SAMPLES").ok().and_then(|v| v.parse().ok()).unwrap_or(9usize);
    let dir = temp_dir("perf-e2e");
    let spool = Spool::default();
    spool.open(dir.clone());
    spool.linked(1, Role::Overlay);
    let now_step = |kind, screen: &str, from: Option<&str>, control: Option<&str>| Step { at: now_ms(), ..step(kind, screen, from, control) };
    spool.steps(1, vec![now_step(Kind::Enter, "qm", Some("closed"), None)], true);
    let mut keeper = crate::perf::Keeper::new(dir.join("perf.jsonl"));
    let mut gpu = crate::perf::gpu::Gpu::open();
    eprintln!("GPU counters: {}", if gpu.is_some() { "open" } else { "none" });
    let _ = keeper.sample(Instant::now(), "desktop", vec![]);
    let _ = gpu.as_mut().and_then(crate::perf::gpu::Gpu::sample);
    let (mut scan_us, mut gpu_us, mut sum_us) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..samples {
        std::thread::sleep(Duration::from_secs(every));
        if i == samples / 2 {
            spool.steps(1, vec![now_step(Kind::Press, "qm", None, Some("Worlds")), now_step(Kind::Enter, "qm/Worlds", Some("qm"), None)], true);
        }
        let began = Instant::now();
        let line = keeper.sample(Instant::now(), "desktop", vec![]).expect("a line");
        let scanned = began.elapsed();
        let now = gpu.as_mut().and_then(crate::perf::gpu::Gpu::sample);
        let counted = began.elapsed() - scanned;
        let mode = if line.vrchat.is_some() { "desktop" } else { "idle" };
        let (sessions, screens) = spool.context();
        let active = super::perf::active(&crate::state::Snapshot::new(orbiters_core::settings::Settings::default()), &line, &screens);
        let context = super::perf::Context { mode: mode.into(), screens, sessions, active };
        let s = super::perf::sample(&line, now.as_ref(), context, now_ms(), began.elapsed());
        spool.perf(s, true);
        let total = began.elapsed();
        scan_us.push(scanned.as_micros());
        gpu_us.push(counted.as_micros());
        sum_us.push((total - scanned - counted).as_micros());
    }
    eprintln!("sample cost, µs: system call {scan_us:?}; GPU counters {gpu_us:?}; summing and keeping {sum_us:?}");
    let waiting = spool.waiting().len() + spool.perf_waiting().len();
    for w in spool.perf_waiting() {
        eprintln!("{}", serde_json::to_string(&w).unwrap());
    }
    assert!(!spool.perf_waiting().is_empty());
    assert_eq!(spool.send(&orbiters_core::account::Api::at(&base), None), Sent::Taken(waiting));
    let _ = std::fs::remove_dir_all(&dir);
}
