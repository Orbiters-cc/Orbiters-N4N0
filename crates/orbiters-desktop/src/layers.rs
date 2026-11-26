//! The layers of this crate, and that no file reaches above its own (XCT-2). The crate is to be split along them (XCT-1:
//! state, service and bridge, surface, overlay | pages, the programs); the compiler will hold them then. Until it does,
//! this test does: what already reaches up is listed, and the list only shrinks.

use std::collections::BTreeMap;

/// The modules of the crate, from what knows nobody up to the programs. A module may use those of its own layer and of
/// any layer below. Two layers side by side ([`BESIDE`]) use neither each other.
const LAYERS: &[(&str, &[&str])] = &[
    // Data and what is on this computer: no service, no window.
    ("state", &[
        "audio_devices", "booster", "device", "gpu", "ipc", "keys", "language", "linux", "mac_display", "mailbox", "media", "member", "perf", "single", "startup", "state", "steamvr_start", "streams", "tracking", "views",
        "words", "worker",
    ]),
    ("bridge", &["bridge"]),
    // What the service does, with no interface.
    ("service", &["capture_picture", "controller", "mac_update", "mixer_inbox", "reconstruct", "upload_prep", "usage"]),
    // What the window and the overlay share.
    ("surface", &["appearance", "avatar_gun_picker", "avatar_gun_ui", "backend", "drag_out", "mac_cover", "mismatch", "shared_view_ui", "wake_offer", "window_rest", "world_notice", "worlds"]),
    ("overlay", &["coverage_window", "menu_host", "overlay", "qr"]),
    ("pages", &["chat_bar", "pages", "splat_export"]),
    // The programs: the service's process, the window's.
    ("programs", &["app", "mac", "service", "tray", "window"]),
    // What developers run.
    ("developer", &["background_bench", "bench", "harness", "idle_frames", "idle_service", "link_between", "link_smoke", "screenshot"]),
];

const BESIDE: (&str, &str) = ("overlay", "pages");

/// What reached above its layer on 2026-11-25: the file, the module above, and in how many places. A number only goes
/// down, and a line goes once it reaches none. Nothing is added: what a file needs from above belongs below it.
const KNOWN: &[(&str, &str, usize)] = &[
    ("chat_bar/panel.rs", "overlay", 3),
    ("controller.rs", "pages", 2),
    ("controller/assistant.rs", "overlay", 1),
    ("controller/assistant.rs", "pages", 2),
    ("controller/bridge_game.rs", "worlds", 1),
    ("controller/camera_tools.rs", "qr", 1),
    ("controller/launcher.rs", "pages", 1),
    ("controller/launcher.rs", "worlds", 2),
    ("controller/menu_shares.rs", "pages", 1),
    ("controller/n4n0.rs", "qr", 1),
    ("controller/perf.rs", "overlay", 1),
    ("controller/shares.rs", "worlds", 14),
    ("device.rs", "controller", 1),
    ("ipc.rs", "backend", 1),
    ("ipc.rs", "bridge", 1),
    ("ipc.rs", "controller", 31),
    ("ipc.rs", "usage", 1),
    ("linux/tray.rs", "service", 1),
    ("member.rs", "backend", 1),
    ("overlay/orbit/mod.rs", "pages", 2),
    ("overlay/orbit/paint_bench.rs", "screenshot", 2),
    ("overlay/orbit/panels/mic_sheet.rs", "pages", 1),
    ("overlay/ui.rs", "pages", 2),
    ("pages/avatar_gun.rs", "app", 1),
    ("pages/community.rs", "app", 2),
    ("pages/events.rs", "app", 1),
    ("pages/gallery.rs", "app", 1),
    ("pages/home.rs", "menu_host", 1),
    ("pages/home.rs", "overlay", 1),
    ("pages/launcher/destinations.rs", "overlay", 4),
    ("pages/projects/preview.rs", "app", 1),
    ("pages/projects/preview_gpu.rs", "overlay", 1),
    ("pages/sessions.rs", "app", 2),
    ("pages/sharing.rs", "app", 2),
    ("pages/states.rs", "overlay", 1),
    ("pages/translate_card.rs", "overlay", 6),
    ("pages/worlds.rs", "app", 1),
    ("state.rs", "bridge", 1),
    ("state.rs", "controller", 7),
    ("usage/mod.rs", "backend", 3),
    ("window_rest.rs", "overlay", 3),
    ("worlds.rs", "overlay", 2),
    ("worlds.rs", "pages", 1),
    ("worlds/at_once.rs", "overlay", 1),
];

/// The layer of `module`, by its place from the bottom.
fn layer(module: &str) -> Option<usize> {
    LAYERS.iter().position(|(_, modules)| modules.contains(&module))
}

/// Whether what is in layer `from` reaches above it, or beside it, with `to`.
fn up(from: usize, to: usize) -> bool {
    let beside = (LAYERS[from].0, LAYERS[to].0);
    to > from || beside == (BESIDE.1, BESIDE.0)
}

/// Every reference of `files` that goes up: the file and the module above, with the lines.
fn upward(files: &[(String, orbiters_core::source::File)]) -> BTreeMap<(String, String), Vec<usize>> {
    let mut out: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (name, file) in files {
        let Some(from) = file.module.first().and_then(|m| layer(m)) else { continue };
        for (line, path) in orbiters_core::source::references(file) {
            let Some(to) = path.first() else { continue };
            if layer(to).is_some_and(|l| up(from, l)) {
                out.entry((name.clone(), to.clone())).or_default().push(line);
            }
        }
    }
    out
}

fn files() -> Vec<(String, orbiters_core::source::File)> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    orbiters_core::source::files(&src).into_iter().map(|f| (f.path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/"), f)).collect()
}

#[test]
fn no_file_reaches_above_its_layer() {
    let files = files();
    assert!(files.len() > 200, "the crate's sources are read: {}", files.len());
    // Every module has its layer: a new one is put in one here.
    let unplaced: Vec<&str> = files.iter().filter(|(_, f)| f.module.len() == 1 && layer(&f.module[0]).is_none()).map(|(name, _)| name.as_str()).collect();
    assert!(unplaced.is_empty(), "modules of no layer (layers.rs, LAYERS): {unplaced:?}");
    let found = upward(&files);
    let mut wrong = Vec::new();
    for ((file, module), lines) in &found {
        let known: usize = KNOWN.iter().filter(|(f, m, _)| f == file && m == module).map(|(.., n)| n).sum();
        if lines.len() > known {
            let layers = (LAYERS[layer(files.iter().find(|(n, _)| n == file).unwrap().1.module[0].as_str()).unwrap()].0, LAYERS[layer(module).unwrap()].0);
            wrong.push(format!("NEW: {file} ({}) uses crate::{module} ({}) {} times, {known} known; lines {lines:?}", layers.0, layers.1, lines.len()));
        }
    }
    for (file, module, n) in KNOWN {
        let now = found.get(&(file.to_string(), module.to_string())).map_or(0, Vec::len);
        if now < *n {
            wrong.push(format!("FEWER: {file} uses crate::{module} {now} times now, {n} known: write {now} in KNOWN (or take its line out)"));
        }
    }
    if !wrong.is_empty() {
        let list: String = found.iter().map(|((f, m), l)| format!("    (\"{f}\", \"{m}\", {}),\n", l.len())).collect();
        panic!("{}\n\nEvery reference that goes up now:\n{list}", wrong.join("\n"));
    }
}

/// The test of the test: what goes up is found however it is written, what goes down or stays is not, and what only tests
/// do is theirs.
#[test]
fn a_reference_upward_is_found_however_it_is_written() {
    let file = |module: &[&str], text: &str| {
        let name = module.join("/");
        (name.clone(), orbiters_core::source::File { path: name.into(), module: module.iter().map(|m| m.to_string()).collect(), text: orbiters_core::source::production(text) })
    };
    let found = upward(&[
        file(&["controller", "maps"], "use crate::{state::Snapshot, pages::{self, home}};\nfn f(x: super::super::overlay::Ui) -> super::Controller { crate::worker::spawn() }\n#[cfg(test)]\nmod tests { use crate::window; }\nfn after() { crate::window::open() }\n"),
        file(&["pages"], "fn f() { crate::overlay::ui(); crate::controller::go(); self::home::draw(); super::app::App::new(); }"),
        file(&["overlay"], "fn f() { let s = \"crate::pages::x\"; /* crate::pages::y */ crate::worlds::z() }"),
    ]);
    let found: Vec<(&str, &str, &[usize])> = found.iter().map(|((f, m), l)| (f.as_str(), m.as_str(), l.as_slice())).collect();
    assert_eq!(
        found,
        [
            ("controller/maps", "overlay", &[2][..]),
            ("controller/maps", "pages", &[1, 1][..]),
            ("controller/maps", "window", &[5][..]),
            ("pages", "app", &[1][..]),
            ("pages", "overlay", &[1][..]),
        ]
    );
}
