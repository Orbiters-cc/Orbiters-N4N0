use super::*;

/// What [`crate::log_warn!`] puts in the file for a line, through the macro's own expansion.
macro_rules! kept {
    (@bind $text:literal [$(($($name:ident)? , $bound:ident))*] []) => {{
        #[allow(unused_imports)]
        use crate::app_log::{ByError as _, ByNothing as _, ByPlain as _};
        // (The whole line still formats: the same arguments.)
        let whole = format!($text $(, $($name =)? $bound)*);
        assert!(!whole.is_empty());
        Site::new("orbiters_desktop::controller::shares", 12, $text).words(&[$((concat!($(stringify!($name))?), (&&&Arg($bound)).shown())),*])
    }};
    (@bind $text:literal [$($done:tt)*] [$name:ident = $value:expr $(, $($rest:tt)*)?]) => {
        match &$value { bound => kept!(@bind $text [$($done)* ($name , bound)] [$($($rest)*)?]) }
    };
    (@bind $text:literal [$($done:tt)*] [$value:expr $(, $($rest:tt)*)?]) => {
        match &$value { bound => kept!(@bind $text [$($done)* (, bound)] [$($($rest)*)?]) }
    };
    ($text:literal $(, $($values:tt)*)?) => { kept!(@bind $text [] [$($($values)*)?]) };
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("orbiters-app-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn at(text: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).unwrap()
}

/// XCT-27: what is never in the log, through the lines the app writes. Every value here is what one of its lines holds.
#[test]
fn a_line_holds_no_name_no_token_no_id_no_path_and_no_text() {
    let name = String::from("Mira Vale");
    let friend: &str = "someone's friend";
    let world = Some(String::from("The Great Pug"));
    let token = String::from("authcookie_0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0");
    let url = String::from("https://api.vrchat.cloud/api/1/users/usr_0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0?apiKey=JlE5Jldo5Jibnk5O5hTx6XVqsJu4WJ26");
    let log_line = String::from("2026.11.25 21:24:42 Debug      -  [Behaviour] OnPlayerJoined Mira Vale");
    let chat = String::from("meet me at the usual place");
    let error = anyhow::anyhow!("{name} is not in {}", world.as_ref().unwrap()).context(format!("asking {url}"));
    let io = std::io::Error::from_raw_os_error(5);
    let io_text = std::io::Error::other(format!("{name} could not be read"));
    let home = dirs::home_dir().unwrap().join("Documents").join("My secret project").join("notes.txt");
    let session = crate::paths::documents_root().join("Sessions").join("The Great Pug 2026-11-25").join("frames.bin");
    let lines = [
        kept!("could not invite {name}: {e:#}", name = name, e = error),
        kept!("{} and {} in {:?}", name, friend, world),
        kept!("signing in with {}: {io}", token, io = io),
        kept!("{}: {}", url, io_text),
        kept!("VRChat said: {log_line}", log_line = log_line),
        kept!("not sent: {chat:?}", chat = chat),
        kept!("{} could not be read: {}", home.display(), io),
        kept!("{:?} could not be read: {}", session, io),
        kept!("{} and {}", name.as_str(), format!("{token}")),
        // Only captured: nothing of it is known here.
        Site::new("orbiters_core::shares", 1, "{name} left {world:?}").words(&[]),
    ];
    for line in &lines {
        for never in ["Mira", "Vale", "friend", "Pug", "authcookie", "0f1e2d3c", "usr_", "apiKey", "JlE5", "OnPlayerJoined", "Behaviour", "usual place", "secret", "notes", "Documents", "Sessions", "vrchat.cloud"] {
            assert!(!line.contains(never), "{never} in {line}");
        }
        if let Some(user) = dirs::home_dir().and_then(|h| h.file_name().map(|n| n.to_string_lossy().into_owned())) {
            assert!(!line.contains(&user), "{line}");
        }
    }
    // What they do hold.
    assert_eq!(lines[0], "controller::shares:12 could not invite …: an error of words alone");
    assert_eq!(lines[2], "controller::shares:12 signing in with …: PermissionDenied (os 5)".replace("PermissionDenied (os 5)", &io.plain()));
    assert_eq!(lines[3], "controller::shares:12 …: Other");
    assert!(lines[6].starts_with("controller::shares:12 ~/….txt could not be read: "), "{}", lines[6]);
    assert!(lines[7].starts_with("controller::shares:12 <documents>/….bin could not be read: "), "{}", lines[7]);
    assert_eq!(lines[9], "core::shares:1 … left …");
}

#[test]
fn a_line_says_kinds_codes_counts_and_the_app_s_own_files() {
    let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
    assert_eq!(kept!("{n} of {} in {:?}, {{braces}}: {e}", 7, Duration::from_millis(1500), n = 3u8, e = refused), "controller::shares:12 3 of 7 in 1.5s, {braces}: ConnectionRefused");
    assert_eq!(kept!("answered {}", ureq::Error::StatusCode(429)), "controller::shares:12 answered HTTP 429");
    let caused = anyhow::Error::from(ureq::Error::StatusCode(500)).context("asking for someone's picture");
    assert_eq!(kept!("{e:#}", e = caused), "controller::shares:12 HTTP 500");
    let json = serde_json::from_str::<u8>("\"Mira\"").unwrap_err();
    assert_eq!(kept!("{}", json), "controller::shares:12 JSON: Data at 1:6");
    assert_eq!(kept!("{}", Word("the picture")), "controller::shares:12 the picture");
    assert_eq!(kept!("{:?}", Some(4)), "controller::shares:12 4");
    assert_eq!(kept!("{}", crate::paths::config_dir().join("settings.json").display()), "controller::shares:12 <settings>/settings.json");
    assert_eq!(kept!("{}", crate::paths::local_data_root().join("avatars").join("usr_1234").join("x.json").display()), "controller::shares:12 <data>/avatars/….json");
    assert_eq!(kept!("{}", crate::paths::local_data_root().join("usr_1234.json").display()), "controller::shares:12 <data>/….json");
    // An error of a type nobody knows here says its type, and the kinds of its causes.
    #[derive(Debug)]
    struct Odd(std::io::Error);
    impl fmt::Display for Odd {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Mira Vale's computer said no")
        }
    }
    impl std::error::Error for Odd {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    let odd = kept!("{}", Odd(std::io::ErrorKind::TimedOut.into()));
    assert!(odd.ends_with("Odd: TimedOut") && !odd.contains("Mira"), "{odd}");
}

/// The second look, on what a mistake in a line's making would let through.
#[test]
fn what_looks_like_a_secret_or_a_person_s_is_taken_out() {
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    for (from, to) in [
        ("Authorization: Bearer abc.def", "Authorization: … …"),
        ("token: s3cr3t and more", "token: … and more"),
        ("auth=authcookie_11112222-3333", "auth=…"),
        ("for usr_11112222-3333-4444-5555-666677778888, in wrld_11112222:123~private(usr_1)", "for …, in …(…)"),
        ("signed eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.x", "signed …"),
        ("GET https://me:pass@orbiters.cc/api/x?token=1#y failed", "GET https://orbiters.cc/… failed"),
        ("(http://127.0.0.1:4100)", "(http://127.0.0.1:4100)"),
        ("wrote to mira.vale@example.com", "wrote to …"),
        ("key 0123456789abcdef0123456789abcdef", "key …"),
        (r"D:\Users\Mira Vale\AppData\x.json is gone", r"~\AppData\x.json is gone"),
        ("/home/mira/x.json: 3 of 4", "~/x.json: 3 of 4"),
        ("then 2026.11.25 21:24:42 Log - [Network] joined The Great Pug", "then (a line of VRChat's log)"),
        ("nothing of that: 3 times in 4.5s at shares.rs:120 (os 5)", "nothing of that: 3 times in 4.5s at shares.rs:120 (os 5)"),
        ("pictures were cannot_found_here, e=5", "pictures were cannot_found_here, e=5"),
    ] {
        assert_eq!(redacted(from), to, "{from}");
    }
    let mine = redacted(&format!("{home}\\x and {} and {}", home.to_uppercase(), home.replace('\\', "/")));
    assert!(!mine.to_lowercase().contains(&home.to_lowercase()) && mine.starts_with('~'), "{mine}");
}

#[test]
fn the_files_are_set_aside_and_never_more_than_five_of_two_megabytes_and_fourteen_days() {
    let dir = temp("files");
    let line = |day: &str, n: usize| format!("{day}T10:00:00Z warn window x {n} {}\n", "y".repeat(200));
    let batch: String = (0..3000).map(|n| line("2026-11-01", n)).collect();
    for _ in 0..20 {
        write(&dir, &batch, at("2026-11-01T10:00:00Z")).unwrap();
    }
    let sizes: Vec<u64> = (0..FILES).map(|n| std::fs::metadata(file(&dir, n)).unwrap().len()).collect();
    assert!(sizes.iter().all(|s| *s <= LARGEST), "{sizes:?}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "log")).count(), FILES);
    // A day on it is the same file; the third day begins another.
    write(&dir, &line("2026-11-02", 1), at("2026-11-02T10:00:00Z")).unwrap();
    assert!(std::fs::metadata(file(&dir, 0)).unwrap().len() > 1000, "the same file");
    write(&dir, &line("2026-11-03", 1), at("2026-11-03T10:00:00Z")).unwrap();
    assert!(std::fs::read_to_string(file(&dir, 0)).unwrap().starts_with("2026-11-03"));
    // Two weeks after, nothing of the first days is left.
    write(&dir, &line("2026-11-15", 1), at("2026-11-15T10:00:00Z")).unwrap();
    let left: Vec<String> = (0..FILES).filter_map(|n| std::fs::read_to_string(file(&dir, n)).ok()).map(|t| t[..10].to_string()).collect();
    assert_eq!(left, ["2026-11-15", "2026-11-03"]);
    // What is no log of ours (cut by a power loss) does not stop it, and goes.
    std::fs::write(file(&dir, 0), [0; 64]).unwrap();
    write(&dir, &line("2026-11-15", 2), at("2026-11-15T10:00:00Z")).unwrap();
    assert!(std::fs::read_to_string(file(&dir, 0)).unwrap().starts_with("2026-11-15"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn several_processes_at_once_lose_no_line() {
    let dir = temp("together");
    let now = at("2026-11-25T10:00:00Z");
    std::thread::scope(|s| {
        for who in 0..4 {
            let dir = &dir;
            s.spawn(move || (0..50).for_each(|n| write(dir, &format!("2026-11-25T10:00:00Z info window {who} {n}\n"), now).unwrap()));
        }
    });
    let text = std::fs::read_to_string(file(&dir, 0)).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 200);
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), 200, "whole lines, each once");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_line_goes_through_the_queue_to_the_file_and_nothing_is_made_before_the_first() {
    let logs = temp("queue");
    let sink = Sink::new(&logs, "window");
    assert!(sink.tx.get().is_none() && !logs.exists(), "no thread and no file while all is well");
    sink.send(Level::Info, "x:1 all is well");
    sink.send(Level::Warn, "x:2 not so well");
    sink.send(Level::Error, "x:3 not at all");
    sink.flush(Duration::from_secs(60));
    let text = std::fs::read_to_string(file(&folder(&logs), 0)).unwrap();
    let lines: Vec<&str> = text.lines().map(|l| &l[21..]).collect();
    assert_eq!(lines, ["info window x:1 all is well", "warn window x:2 not so well", "error window x:3 not at all"]);
    assert!(time::OffsetDateTime::parse(&text[..20], &time::format_description::well_known::Rfc3339).is_ok());
    // What a person copies: the errors and the warnings, the newest last, across the files.
    let older: String = (0..5).map(|n| format!("2026-11-25T10:00:00Z error overlay y:{n} before\n")).collect();
    std::fs::write(file(&folder(&logs), 1), older).unwrap();
    let told: Vec<String> = last_errors(&logs, 4).iter().map(|l| l.rsplit_once(' ').unwrap().0[21..].to_string() + "|" + l.rsplit(' ').next().unwrap()).collect();
    assert_eq!(told, ["error overlay y:3|before", "error overlay y:4|before", "warn window x:2 not so|well", "error window x:3 not at|all"]);
    assert!(last_errors(&temp("none"), 4).is_empty());
    // A role that is no word of the source is not written.
    assert_eq!(Sink::new(&logs, "--Mira Vale").role, "app");
    std::fs::remove_dir_all(&logs).unwrap();
}

#[test]
fn what_is_said_again_and_again_is_counted() {
    let often = Site::new("x", 1, "x");
    assert_eq!((0..25).filter(|_| often.often(100).is_some()).count(), AN_HOUR as usize);
    assert_eq!(often.often(101).as_deref(), Some(" (15 more before)"));
    assert_eq!(often.often(102).as_deref(), Some(""));
    let once = Site::new("x", 1, "x");
    let said: Vec<(u32, String)> = (1..=1000).filter_map(|n| once.once().map(|c| (n, c))).collect();
    assert_eq!(said, [(1, String::new()), (10, " (10 times)".into()), (100, " (100 times)".into()), (1000, " (1000 times)".into())]);
}

/// The macros compile with what `eprintln!` takes, and evaluate a value once.
#[test]
fn the_macros_take_what_eprintln_takes() {
    let calls = std::cell::Cell::new(0);
    let next = || {
        calls.set(calls.get() + 1);
        calls.get()
    };
    crate::log_info!("nothing more");
    crate::log_info!("{} and {name:>4} and {0:?}", next(), name = "a text",);
    let captured = 3;
    crate::log_info!("{captured}");
    assert_eq!(calls.get(), 1);
    let failed: Result<(), std::io::Error> = Err(std::io::ErrorKind::NotFound.into());
    crate::log_failed!(failed, "a file of a test could not be written");
    let text: Result<(), String> = Err("words".into());
    crate::log_failed!(text, "a text of a test could not be written");
}
