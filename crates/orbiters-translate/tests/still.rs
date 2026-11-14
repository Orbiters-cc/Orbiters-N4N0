//! Live translation on still pictures: a sign of known text in five scripts, placed in the room at a known pose and seen
//! by two eyes as SteamVR's compositor would hand them over (drawn with the interface's own fonts and painter, seen through
//! the eyes' own fields), is read, placed, followed and translated. The tests that read text need the PP-OCR models
//! (`ORBITERS_TRANSLATE_MODELS`, which `cargo test -- --ignored` uses); the others need nothing.

use orbiters_translate::core::{Activity, Core, Reader};
use orbiters_translate::geometry::{self, compose, dot, norm, rotation, scale, sub, EyeView, Mirror, M34, V3};
use orbiters_translate::ocr::TextBox;
use orbiters_translate::picture::{draw_text, plain_room, render_eye, Picture, Sign, Text};
use orbiters_translate::pipeline::{self, Look, Shown};
use orbiters_translate::place::{self, Pair, FALLBACK_DISTANCE};
use orbiters_translate::provider::{self, Batch, Failure, Item, LanModel, Translator};
use orbiters_translate::track::{Seen, Tracker};
use std::sync::{Arc, Mutex};

/// Quest Pro's eyes as SteamVR gives them (left; the right is its mirror image), 64 mm apart.
const LEFT_RAW: [f32; 4] = [-1.376, 0.839, -1.376, 0.900];
const RIGHT_RAW: [f32; 4] = [-0.839, 1.376, -1.376, 0.900];
const IPD: f32 = 0.064;
/// The sandbox's eye size (Enzo's headset: 3168 x 3456).
const SIZE: [u32; 2] = [1620, 1800];

const LINES: [&str; 5] = ["出口はこちらです", "欢迎来到我们的世界", "안녕하세요 친구들", "Добро пожаловать", "Welcome to the club"];
const LINE_TOP: f32 = 30.0;
const LINE_STEP: f32 = 110.0;
const LINE_SIZE: f32 = 72.0;

fn sign_picture() -> Picture {
    let texts: Vec<Text> = LINES.iter().enumerate().map(|(i, t)| Text { text: t.to_string(), at: [40.0, LINE_TOP + i as f32 * LINE_STEP], size: LINE_SIZE, colour: [20, 20, 20] }).collect();
    draw_text(900, 600, [235, 228, 210], &texts)
}

/// The sign `distance` metres ahead of a head at the origin (eyes at 1.6 m), turned `yaw` and rolled `roll` degrees.
fn sign(distance: f32, yaw: f32, roll: f32) -> Sign {
    let pose = compose(&rotation([0.0, 1.0, 0.0], yaw.to_radians(), [-0.1, 1.55, -distance]), &rotation([0.0, 0.0, 1.0], roll.to_radians(), [0.0; 3]));
    Sign { picture: sign_picture(), pose, width: 1.2 * distance / 2.2 }
}

fn views(head: &M34) -> [EyeView; 2] {
    let eye = |x: f32| [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    [
        EyeView { eye: compose(head, &eye(-IPD / 2.0)), mirror: Mirror::whole_eye(LEFT_RAW), size: [SIZE[0] as f32, SIZE[1] as f32] },
        EyeView { eye: compose(head, &eye(IPD / 2.0)), mirror: Mirror::whole_eye(RIGHT_RAW), size: [SIZE[0] as f32, SIZE[1] as f32] },
    ]
}

fn head_at(x: f32, yaw_deg: f32) -> M34 {
    rotation([0.0, 1.0, 0.0], yaw_deg.to_radians(), [x, 1.6, 0.0])
}

fn render(views: &[EyeView; 2], signs: &[Sign]) -> [Picture; 2] {
    [render_eye(&views[0], SIZE, &plain_room, signs), render_eye(&views[1], SIZE, &plain_room, signs)]
}

fn pair<'a>(pictures: &'a [Picture; 2], views: [EyeView; 2]) -> Pair<'a> {
    Pair { pictures: [pictures[0].view(), pictures[1].view()], from: [[0, 0]; 2], views }
}

/// Line `i` of the sign as the left eye sees it: the rectangle its letters take on the sign (generously wide), projected.
fn line_box(sign: &Sign, view: &EyeView, i: usize) -> TextBox {
    let chars = LINES[i].chars().count() as f32;
    let wide = if LINES[i].is_ascii() || i == 3 { 0.5 } else { 1.0 };
    let (x0, y0) = (32.0, LINE_TOP + i as f32 * LINE_STEP - 6.0);
    let (x1, y1) = (40.0 + chars * LINE_SIZE * wide + 12.0, y0 + LINE_SIZE + 20.0);
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| view.project(sign.at(p)).unwrap());
    TextBox { corners, score: 1.0, vertical: false }
}

/// Where line `i`'s middle is on the sign, in the room.
fn line_centre(sign: &Sign, i: usize) -> V3 {
    let b = |p| sign.at(p);
    let chars = LINES[i].chars().count() as f32;
    let wide = if LINES[i].is_ascii() || i == 3 { 0.5 } else { 1.0 };
    let (x0, y0) = (32.0, LINE_TOP + i as f32 * LINE_STEP - 6.0);
    let (x1, y1) = (40.0 + chars * LINE_SIZE * wide + 12.0, y0 + LINE_SIZE + 20.0);
    b([(x0 + x1) / 2.0, (y0 + y1) / 2.0])
}

fn angle_between(a: V3, b: V3) -> f32 {
    dot(geometry::normalized(a), geometry::normalized(b)).clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn both_eyes_place_each_line_where_it_is_with_the_signs_tilt() {
    for (distance, yaw, roll) in [(1.0, 0.0, 0.0), (2.2, 25.0, 6.0), (4.0, -35.0, -4.0)] {
        let sign = sign(distance, yaw, roll);
        let head = head_at(0.0, 0.0);
        let views = views(&head);
        let pictures = render(&views, std::slice::from_ref(&sign));
        let pair = pair(&pictures, views);
        let found: Vec<place::Found> = (0..LINES.len()).map(|i| place::measure(&pair, &line_box(&sign, &views[0], i))).collect();
        let placed = place::place_all(&views[0], &found);
        let normal = geometry::axis(&sign.pose, 2);
        for (i, p) in placed.iter().enumerate() {
            assert!(p.measured, "line {i} at {distance} m is measured by both eyes");
            let off = norm(sub(p.centre(), line_centre(&sign, i)));
            // A distance from two eyes 64 mm apart: within 2 % of it, and a few millimetres.
            assert!(off < 0.02 * distance + 0.01, "line {i} at {distance} m, yaw {yaw}: {off:.3} m off");
            let tilt = angle_between(geometry::axis(&p.pose, 2), normal);
            assert!(tilt < 10.0, "line {i} at {distance} m, yaw {yaw}: its face is {tilt:.1} degrees off the sign's");
            let roll_err = angle_between(geometry::axis(&p.pose, 0), geometry::axis(&sign.pose, 0));
            assert!(roll_err < 4.0, "line {i}: along the text {roll_err:.1} degrees off");
        }
    }
}

#[test]
fn a_line_the_eyes_cannot_match_still_appears_in_its_direction() {
    let sign = sign(2.2, 25.0, 6.0);
    let views = views(&head_at(0.0, 0.0));
    // The right eye sees nothing of the sign (an occluder, a reflection): no match.
    let left = render_eye(&views[0], SIZE, &plain_room, std::slice::from_ref(&sign));
    let right = render_eye(&views[1], SIZE, &plain_room, &[]);
    let pictures = [left, right];
    let pair = pair(&pictures, views);
    let b = line_box(&sign, &views[0], 0);
    let found = place::measure(&pair, &b);
    assert!(found.halves.is_none(), "nothing to match");
    let placed = &place::place_all(&views[0], &[found])[0];
    assert!(!placed.measured);
    let (o, d) = views[0].ray(b.centre());
    let to = sub(placed.centre(), o);
    let off_ray = norm(sub(to, scale(d, dot(to, d))));
    // The rectangle's middle is its corners' middle, a few millimetres from the ray through the box's middle at 2 m.
    assert!(off_ray < 0.01, "on the eye's ray through the line: {off_ray}");
    assert!((norm(to) - FALLBACK_DISTANCE).abs() < 0.1);
    // Its angular size is the line's: its corners project back onto the box.
    let back: Vec<[f32; 2]> = placed.corners().iter().map(|c| views[0].project(*c).unwrap()).collect();
    let span = |c: &[[f32; 2]]| ((c[0][0] - c[1][0]).powi(2) + (c[0][1] - c[1][1]).powi(2)).sqrt();
    let ratio = span(&back) / b.length();
    assert!((ratio - 1.0).abs() < 0.05, "as wide as it looks: {ratio}");
}

#[test]
fn translations_stay_put_while_the_head_moves() {
    let sign = sign(2.2, 25.0, 6.0);
    let mut tracker = Tracker::default();
    let mut firsts: Vec<V3> = Vec::new();
    let mut worst = 0f32;
    for (k, (x, yaw)) in [(0.0, 0.0), (0.04, 2.0), (-0.03, -3.0), (0.06, 4.0), (0.0, 1.0)].iter().enumerate() {
        let views = views(&head_at(*x, *yaw));
        let pictures = render(&views, std::slice::from_ref(&sign));
        let pair = pair(&pictures, views);
        let found: Vec<place::Found> = (0..LINES.len()).map(|i| place::measure(&pair, &line_box(&sign, &views[0], i))).collect();
        let placed = place::place_all(&views[0], &found);
        let sightings = placed
            .into_iter()
            .enumerate()
            .map(|(i, p)| orbiters_translate::track::Sighting { text: LINES[i].into(), confidence: 0.99, placed: p, colours: orbiters_translate::track::Colours { background: [235, 228, 210], text: [20; 3] }, context: vec![], in_eye: [0.0; 3] })
            .collect();
        tracker.update(k as f64 * 0.5, sightings, &|_| Seen::Open);
        assert_eq!(tracker.tracks.len(), LINES.len(), "look {k}: one track a line, no duplicates");
        if k == 0 {
            firsts = tracker.tracks.iter().map(|t| t.placed.centre()).collect();
        } else {
            for (t, first) in tracker.tracks.iter().zip(&firsts) {
                worst = worst.max(norm(sub(t.placed.centre(), *first)));
            }
        }
    }
    assert!(worst < 0.02, "a translation moves less than 2 cm while the head moves and turns: {worst:.4} m");
}

#[test]
fn our_own_translations_are_masked_and_known_as_covered() {
    let sign = sign(2.2, 25.0, 6.0);
    let views = views(&head_at(0.0, 0.0));
    let pictures = render(&views, std::slice::from_ref(&sign));
    let pair = pair(&pictures, views);
    let b = line_box(&sign, &views[0], 0);
    let placed = place::place_all(&views[0], &[place::measure(&pair, &b)])[0];
    let shown = [Shown { pose: placed.pose, width: placed.width, height: placed.height, track: Some(1) }];
    let outline = shown[0].outline(&views[0]).unwrap();
    assert!(outline.contains(b.centre()) && b.corners.iter().all(|c| outline.contains(*c)), "the whole line is under its translation's outline");
    let look = Look { pair, area: [0, 0, SIZE[0], SIZE[1]], shown: &shown, zoom: None };
    let mut track = orbiters_translate::track::Track {
        id: 1,
        text: LINES[0].into(),
        key: LINES[0].into(),
        confidence: 1.0,
        placed,
        colours: orbiters_translate::track::Colours { background: [235, 228, 210], text: [20; 3] },
        context: pipeline::context(&look.pair, &placed).unwrap(),
        first: 0.0,
        last_seen: 0.0,
        in_view: 0.0,
        moved_at: [0.0; 2],
        in_eye: [0.0; 3],
        sightings: 2,
        misses: 0,
    };
    assert_eq!(pipeline::seen(&look, &track), Seen::Covered { changed: false });
    track.id = 2;
    assert_eq!(pipeline::seen(&look, &track), Seen::Edge, "under another overlay of ours: on screen, nothing can be told");
    // The sign gone (the same look without it): its surroundings changed.
    let empty = [render_eye(&views[0], SIZE, &plain_room, &[]), render_eye(&views[1], SIZE, &plain_room, &[])];
    track.id = 1;
    let look = Look { pair: Pair { pictures: [empty[0].view(), empty[1].view()], from: [[0, 0]; 2], views }, area: [0, 0, SIZE[0], SIZE[1]], shown: &shown, zoom: None };
    assert_eq!(pipeline::seen(&look, &track), Seen::Covered { changed: true });
}

#[test]
fn colours_are_the_signs() {
    let sign = sign(2.2, 25.0, 6.0);
    let views = views(&head_at(0.0, 0.0));
    let pictures = render(&views, std::slice::from_ref(&sign));
    let pair = pair(&pictures, views);
    let c = pipeline::colours(&pair, &line_box(&sign, &views[0], 4));
    let near = |a: [u8; 3], b: [u8; 3], tol: i32| (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol);
    assert!(near(c.background, [235, 228, 210], 12), "{:?}", c.background);
    assert!(near(c.text, [20, 20, 20], 60), "{:?}", c.text);
}

/// A stand-in OpenAI-compatible server on loopback: answers `/v1/models` and `/v1/chat/completions` with what `answer`
/// gives for the request's body, and keeps the bodies.
fn fake_server(answer: impl Fn(&serde_json::Value) -> (u16, String) + Send + 'static) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut length = 0usize;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let (status, text) = if path.ends_with("/models") {
                (200, r#"{"data":[{"id":"text-embedding-nomic"},{"id":"qwen3-8b"},{"id":"translategemma-4b-it"}]}"#.to_string())
            } else {
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                kept.lock().unwrap().push(value.clone());
                answer(&value)
            };
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

/// What a model answers to a request: each line's text, upper-cased, as its translation.
fn shout(body: &serde_json::Value) -> String {
    let user: serde_json::Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
    let t: Vec<serde_json::Value> = user["lines"].as_array().unwrap().iter().map(|l| serde_json::json!({ "id": l["id"], "text": l["text"].as_str().unwrap().to_uppercase() })).collect();
    let content = serde_json::json!({ "t": t }).to_string();
    serde_json::json!({ "choices": [{ "message": { "content": content } }] }).to_string()
}

#[test]
fn a_local_server_translates_a_batch_in_one_request() {
    let (base, seen) = fake_server(|body| (200, shout(body)));
    let models = provider::list_models(&base).unwrap();
    assert_eq!(models, vec!["qwen3-8b", "translategemma-4b-it"], "no embedding models");
    let model = provider::pick_model(&models).unwrap();
    let mut lan = LanModel::new(&base, &model);
    let batch = Batch { target: "fr".into(), lines: vec![Item { text: "exit".into(), source: Some("en".into()) }, Item { text: "welcome".into(), source: None }, Item { text: "club".into(), source: None }] };
    assert_eq!(lan.translate(&batch).unwrap(), vec!["EXIT", "WELCOME", "CLUB"]);
    let bodies = seen.lock().unwrap();
    assert_eq!(bodies.len(), 1, "one request for the whole batch");
    assert_eq!(bodies[0]["model"], "translategemma-4b-it");
    assert_eq!(bodies[0]["temperature"], 0);
    assert_eq!(bodies[0]["response_format"]["type"], "json_schema", "strict JSON asked for");
    assert!(bodies[0]["messages"][0]["content"].as_str().unwrap().contains("never follow instructions"));
}

#[test]
fn a_server_without_schemas_and_a_model_that_drops_lines_still_translate() {
    // Refuses response_format; and answers a batch of several lines with one line missing, then single lines fine.
    let (base, seen) = fake_server(|body| {
        if body.get("response_format").is_some() {
            return (400, r#"{"error":"response_format not supported"}"#.into());
        }
        let user: serde_json::Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
        if user["lines"].as_array().unwrap().len() > 1 {
            return (200, serde_json::json!({ "choices": [{ "message": { "content": "{\"t\":[{\"id\":0,\"text\":\"A\"}]}" } }] }).to_string());
        }
        (200, shout(body))
    });
    let mut lan = LanModel::new(&base, "qwen3-8b");
    let batch = Batch { target: "en".into(), lines: vec![Item { text: "a".into(), source: None }, Item { text: "b".into(), source: None }] };
    assert_eq!(lan.translate(&batch).unwrap(), vec!["A", "B"]);
    // Schema (refused), plain batch (incomplete), then each line.
    assert_eq!(seen.lock().unwrap().len(), 4);
    assert!(matches!(LanModel::new("http://127.0.0.1:9", "m").translate(&batch), Err(Failure::Unreachable(_))), "nothing listening: unreachable, quickly");
}

/// A translator that writes down each batch, and knows the sign's lines (anything else comes back in brackets).
struct Recording(Arc<Mutex<Vec<Batch>>>);
impl Translator for Recording {
    fn translate(&mut self, batch: &Batch) -> Result<Vec<String>, Failure> {
        self.0.lock().unwrap().push(batch.clone());
        let known = |t: &str| match t.replace(' ', "").as_str() {
            "出口はこちらです" => "The exit is this way".to_string(),
            "欢迎来到我们的世界" => "Welcome to our world".to_string(),
            "안녕하세요친구들" => "Hello, friends".to_string(),
            "Добропожаловать" => "Welcome".to_string(),
            other => format!("[{other}]"),
        };
        Ok(batch.lines.iter().map(|l| known(&l.text)).collect())
    }
}

#[test]
fn lines_are_asked_once_in_a_batch_and_painted_once_translated() {
    let batches = Arc::new(Mutex::new(Vec::new()));
    let mut core = Core::new(None, Box::new(Recording(batches.clone())), Reader { target: "en".into(), reads: vec![] });
    let sign = sign(2.2, 25.0, 6.0);
    let views = views(&head_at(0.0, 0.0));
    let pictures = render(&views, std::slice::from_ref(&sign));
    let pair = pair(&pictures, views);
    let found: Vec<place::Found> = (0..4).map(|i| place::measure(&pair, &line_box(&sign, &views[0], i))).collect();
    let placed = place::place_all(&views[0], &found);
    let gaze = (geometry::origin(&views[0].eye), [0.0, 0.0, -1.0]);
    let mut scene = None;
    for k in 0..40 {
        let sightings = placed
            .iter()
            .enumerate()
            .map(|(i, p)| orbiters_translate::track::Sighting { text: LINES[i].into(), confidence: 0.99, placed: *p, colours: orbiters_translate::track::Colours { background: [235, 228, 210], text: [20; 3] }, context: vec![], in_eye: [0.0; 3] })
            .collect();
        core.tracker.update(k as f64 * 0.1, sightings, &|_| Seen::Open);
        let s = core.after_look(k as f64 * 0.1, gaze, Default::default());
        if s.quads.len() == 4 {
            scene = Some(s);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let scene = scene.expect("four translations shown");
    assert_eq!(scene.activity, Activity::Looking);
    let batches = batches.lock().unwrap();
    assert_eq!(batches.len(), 1, "one request for the four lines, none again once cached");
    assert_eq!(batches[0].lines.len(), 4);
    // In the order of how near they are to the gaze (no longer reading order): each with the language of its letters.
    let source = |text: &str| batches[0].lines.iter().find(|l| l.text == text).and_then(|l| l.source.as_deref());
    assert_eq!(source(LINES[0]), Some("ja"));
    assert_eq!(source(LINES[2]), Some("ko"));
    for q in &scene.quads {
        assert_eq!(q.picture.rgba.len() as u32, q.picture.width * q.picture.height * 4);
        assert!(q.width > 0.05 && q.height > 0.01);
    }
}

/// The PP-OCR models, from `ORBITERS_TRANSLATE_MODELS`.
fn engine() -> pipeline::Engine {
    assert!(std::env::var_os("ORBITERS_TRANSLATE_MODELS").is_some(), "set ORBITERS_TRANSLATE_MODELS to the models' folder");
    pipeline::Engine::load(2).expect("the models load")
}

#[test]
#[ignore]
fn five_scripts_are_read_at_an_angle_and_placed() {
    let mut engine = engine();
    for (distance, yaw, roll) in [(1.5, 0.0, 0.0), (2.2, 25.0, 6.0), (3.0, -30.0, -5.0)] {
        let sign = sign(distance, yaw, roll);
        let views = views(&head_at(0.0, 0.0));
        let pictures = render(&views, std::slice::from_ref(&sign));
        let look = Look { pair: pair(&pictures, views), area: [243, 270, 1134, 1260], shown: &[], zoom: None };
        let (lines, timings) = engine.read(&look).unwrap();
        println!("{distance} m, yaw {yaw}: {lines:?} {timings:?}", lines = lines.iter().map(|l| (&l.text, l.confidence)).collect::<Vec<_>>());
        for want in LINES {
            assert!(lines.iter().any(|l| l.text.replace(' ', "") == want.replace(' ', "")), "{want} read at {distance} m, yaw {yaw}: {:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
        }
        let sightings = pipeline::sightings(&look, &lines);
        for (l, s) in lines.iter().zip(&sightings) {
            let i = LINES.iter().position(|w| w.replace(' ', "") == l.text.replace(' ', "")).unwrap();
            let off = norm(sub(s.placed.centre(), line_centre(&sign, i)));
            assert!(s.placed.measured, "{} measured", l.text);
            // The detector's box is not the test's: its middle may sit a little along the line.
            assert!(off < 0.05 * distance, "{} at {distance} m: {off:.3} m off", l.text);
        }
    }
}

#[test]
#[ignore]
fn our_own_translation_is_not_read_again() {
    let mut engine = engine();
    let sign = sign(2.2, 25.0, 6.0);
    let views = views(&head_at(0.0, 0.0));
    let pictures = render(&views, std::slice::from_ref(&sign));
    let p = pair(&pictures, views);
    let placed = place::place_all(&views[0], &[place::measure(&p, &line_box(&sign, &views[0], 0))])[0];
    let shown = [Shown { pose: placed.pose, width: placed.width, height: placed.height, track: Some(1) }];
    let look = Look { pair: p, area: [0, 0, SIZE[0], SIZE[1]], shown: &shown, zoom: None };
    let (lines, _) = engine.read(&look).unwrap();
    assert!(!lines.iter().any(|l| l.text.contains("出口")), "{:?}", lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(lines.iter().any(|l| l.text.contains("欢迎")), "the rest is read");
}

#[test]
#[ignore]
fn a_whole_look_gives_translations_over_the_sign() {
    let batches = Arc::new(Mutex::new(Vec::new()));
    let mut core = Core::new(Some(engine()), Box::new(Recording(batches.clone())), Reader { target: "en".into(), reads: vec!["en".into()] });
    let sign = sign(2.2, 25.0, 6.0);
    let mut last = None;
    for k in 0..6 {
        let views = views(&head_at(0.01 * k as f32, 0.5 * k as f32));
        let pictures = render(&views, std::slice::from_ref(&sign));
        let look = Look { pair: pair(&pictures, views), area: [243, 270, 1134, 1260], shown: &[], zoom: None };
        let gaze = (geometry::origin(&views[0].eye), scale(geometry::axis(&views[0].eye, 2), -1.0));
        let scene = core.step(k as f64 * 0.5, &look, gaze).unwrap();
        println!("look {k}: {} quads, {:?}", scene.quads.len(), scene.timings);
        last = Some(scene);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let scene = last.unwrap();
    assert_eq!(scene.quads.len(), 4, "every line but the English one, which the reader reads");
    // What the eyes see then: the translations composited over the sign as SteamVR would, kept to be looked at.
    let views = views(&head_at(0.05, 2.5));
    let mut pictures = render(&views, std::slice::from_ref(&sign));
    for (k, view) in views.iter().enumerate() {
        for q in &scene.quads {
            orbiters_translate::picture::composite(&mut pictures[k], view, &q.pose, q.width, q.height, &q.picture.rgba, q.picture.width, q.picture.height);
        }
    }
    if let Some(dir) = std::env::var_os("ORBITERS_TRANSLATE_STILLS") {
        let dir = std::path::PathBuf::from(dir);
        pictures[0].save_png(&dir.join("whole-look-left.png")).unwrap();
        pictures[1].save_png(&dir.join("whole-look-right.png")).unwrap();
        pictures[0].crop([500, 450, 760, 560]).save_png(&dir.join("whole-look-left-sign.png")).unwrap();
    }
    let texts: Vec<String> = batches.lock().unwrap().iter().flat_map(|b| b.lines.iter().map(|l| l.text.clone())).collect();
    assert!(!texts.iter().any(|t| t.contains("Welcome")), "English is left as it is: {texts:?}");
}

#[test]
#[ignore]
fn the_built_in_translator_translates_on_the_processor() {
    use orbiters_translate::builtin::Builtin;
    assert!(Builtin::installed(), "the built-in translator's files in ORBITERS_TRANSLATE_MODELS");
    let mut b = Builtin::load(2).unwrap();
    for (text, source, target, want) in [("出口はこちらです", "ja", "en", "exit"), ("欢迎来到我们的世界", "zh", "en", "welcome"), ("안녕하세요 친구들", "ko", "en", "hello"), ("Добро пожаловать", "ru", "fr", "bienvenue")] {
        let t = std::time::Instant::now();
        let out = b.line(text, source, target).unwrap();
        println!("{source}->{target} {:.0} ms: {text} -> {out}", t.elapsed().as_secs_f64() * 1000.0);
        assert!(out.to_lowercase().contains(want), "{text} -> {out}");
    }
    let batch = Batch { target: "en".into(), lines: vec![Item { text: "出口".into(), source: None }, Item { text: "Добро пожаловать".into(), source: None }] };
    assert_eq!(b.translate(&batch).unwrap().len(), 2, "a batch, the source guessed from the letters");
}

/// A fixture of `testdata`: a part of a real eye picture from the VR sandbox (1620 x 1800 eyes, world "Japan Street").
fn fixture(name: &str) -> orbiters_translate::picture::Picture {
    orbiters_translate::picture::Picture::load_png(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(name)).unwrap()
}

/// Reads a whole fixture, at its size and scaled up `zoom` times, as one eye of a look.
fn read_fixture(engine: &mut pipeline::Engine, picture: &orbiters_translate::picture::Picture, zoom: u32) -> Vec<orbiters_translate::ocr::Line> {
    let p = if zoom > 1 { picture.resized(picture.width * zoom, picture.height * zoom) } else { picture.clone() };
    let view = views(&head_at(0.0, 0.0))[0];
    let view = EyeView { size: [p.width as f32, p.height as f32], ..view };
    let look = Look { pair: Pair { pictures: [p.view(), p.view()], from: [[0, 0]; 2], views: [view, view] }, area: [0, 0, p.width, p.height], shown: &[], zoom: None };
    engine.read(&look).unwrap().0
}

/// The two posters of the station the first sandbox run did not translate: what is read of them at the sandbox's eye
/// resolution, and twice as large (what a closer look of the same detail would give the detector; it cannot add detail
/// the picture does not have).
#[test]
#[ignore]
fn the_stations_posters() {
    let mut engine = engine();
    for name in ["station-poster.png", "station-leaflet.png"] {
        let p = fixture(name);
        for zoom in [1, 2] {
            let lines = read_fixture(&mut engine, &p, zoom);
            println!("{name} x{zoom}: {:?}", lines.iter().map(|l| (l.text.as_str(), (l.confidence * 100.0).round(), runs_letter_height(&l.boxed) / zoom as f32)).collect::<Vec<_>>());
        }
    }
}

fn runs_letter_height(b: &TextBox) -> f32 {
    b.thickness() / orbiters_translate::runs::RUN_HEIGHT
}

/// The station's sign, captured with translation off: "← 西口 West Exit | ← [toilets] | 北口 North Exit ↑". Its runs are
/// the words, the arrows and the pictogram are covered by none of them, and no run is taller than its letters.
#[test]
#[ignore]
fn the_station_sign_is_cut_to_its_words() {
    let mut engine = engine();
    let p = fixture("station-sign.png");
    let lines = read_fixture(&mut engine, &p, 1);
    println!("{:?}", lines.iter().map(|l| (l.text.as_str(), l.boxed.bounds())).collect::<Vec<_>>());
    let texts: Vec<String> = lines.iter().map(|l| l.text.replace(' ', "")).collect();
    assert!(texts.iter().any(|t| t == "西口"), "西口 alone: {texts:?}");
    assert!(texts.iter().any(|t| t == "北口"), "北口 alone: {texts:?}");
    assert!(!texts.iter().any(|t| t.contains('←') || t.contains('↑')), "no arrow is in a run: {texts:?}");
    // The pictogram's columns in the fixture (measured on the picture): nothing covers them.
    let (p0, p1) = PICTOGRAM_COLUMNS;
    for l in &lines {
        let b = l.boxed.bounds();
        assert!(b[2] <= p0 || b[0] >= p1, "{} covers the pictogram: {b:?}", l.text);
    }
}

#[test]
#[ignore]
fn the_built_in_translators_first_translation() {
    use orbiters_translate::builtin::Builtin;
    let t = std::time::Instant::now();
    let mut b = Builtin::load(2).unwrap();
    let loaded = t.elapsed();
    b.warm_up();
    let warmed = t.elapsed();
    let first = b.line("西口", "ja", "en").unwrap();
    println!("built-in: loaded in {:.0} ms, warmed up by {:.0} ms, first sign translated by {:.0} ms: {first}", loaded.as_secs_f64() * 1000.0, warmed.as_secs_f64() * 1000.0, t.elapsed().as_secs_f64() * 1000.0);
}

/// Where the toilets' pictogram lies across `testdata/station-sign.png`, pixels (measured on the picture).
const PICTOGRAM_COLUMNS: (f32, f32) = (198.0, 236.0);

/// Lines read together in one pass of the built-in translator give what each gives alone, in not much more time than
/// the longest alone (on the processor a step's time is the reading of the weights). Prints the times at 2 and 4
/// threads, for one line and for 1, 3, 6 and 8 together.
#[test]
#[ignore]
fn the_built_in_translator_translates_lines_together() {
    use orbiters_translate::builtin::Builtin;
    let signs = [
        ("西口", "ja"),
        ("北口", "ja"),
        ("出口はこちらです", "ja"),
        ("関係者以外立入禁止", "ja"),
        ("欢迎来到我们的世界", "zh"),
        ("안녕하세요 친구들", "ko"),
        ("今日はみんなでカラオケしよう！", "ja"),
        ("CITY SCENERY WORLD in VRCHAT", "en"),
    ];
    for (threads, spinning) in [(2usize, false), (4, false), (2, true), (4, true)] {
        println!("threads {threads}, spinning {spinning}:");
        let mut b = Builtin::load_with(threads, spinning).unwrap();
        b.warm_up();
        let mut alone = Vec::new();
        let t = std::time::Instant::now();
        for (text, source) in signs {
            let t = std::time::Instant::now();
            alone.push(b.line(text, source, "fr").unwrap());
            println!("threads {threads} alone {:.0} ms: {text} -> {}", t.elapsed().as_secs_f64() * 1000.0, alone.last().unwrap());
        }
        println!("threads {threads}: 8 lines one after the other {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        for n in [1usize, 3, 6, 8] {
            let mut together = vec![String::new(); n];
            let mut when = vec![0.0; n];
            let t = std::time::Instant::now();
            b.lines(&signs[..n], "fr", &mut |i, s| {
                together[i] = s;
                when[i] = t.elapsed().as_secs_f64() * 1000.0;
            })
            .unwrap();
            println!("threads {threads}: {n} together {:.0} ms in {} steps, each done at {:?}", t.elapsed().as_secs_f64() * 1000.0, b.steps, when.iter().map(|w| w.round()).collect::<Vec<_>>());
            // The int8 model's activations are scaled over the whole pass, so a line read with others may come out a
            // little differently (sandbox 2026-11-14: "La sortie est ici." together, "L'entrée est ici." alone).
            let same = together.iter().zip(&alone).filter(|(a, b)| a == b).count();
            println!("    {same} of {n} as alone: {together:?}");
            assert!(together.iter().all(|t| !t.is_empty()), "every line translated: {together:?}");
            assert!(same * 2 >= n, "mostly as alone: {together:?} against {alone:?}");
        }
    }
}

/// The whole chain on rendered eyes with the real text reader and the built-in translator, as the overlay runs it:
/// looks 0.6 s apart, translations shown as they come (polled every 30 ms). Prints, for five fresh starts, how long
/// after the first look's picture each translation showed.
#[test]
#[ignore]
fn the_whole_chain_with_the_built_in_translator() {
    use orbiters_translate::builtin::Builtin;
    let sign = sign(2.2, 25.0, 6.0);
    let mut all = Vec::new();
    for run in 0..5 {
        let mut core = Core::new(Some(engine()), Box::new(Builtin::load(4).unwrap()), Reader { target: "fr".into(), reads: vec![] });
        std::thread::sleep(std::time::Duration::from_secs(3));
        let start = std::time::Instant::now();
        core.clock = start;
        let mut next_look = 0.0f64;
        let mut k = 0;
        while start.elapsed().as_secs_f64() < 8.0 {
            let now = start.elapsed().as_secs_f64();
            let views = views(&head_at(0.002 * k as f32, 0.1 * k as f32));
            let gaze = (geometry::origin(&views[0].eye), scale(geometry::axis(&views[0].eye, 2), -1.0));
            if now >= next_look {
                let pictures = render(&views, std::slice::from_ref(&sign));
                let taken = start.elapsed().as_secs_f64();
                let look = Look { pair: pair(&pictures, views), area: [243, 270, 1134, 1260], shown: &[], zoom: None };
                let (lines, timings) = core.engine.as_mut().unwrap().read(&look).unwrap();
                core.follow(taken, &look, lines, timings, gaze);
                let cost = start.elapsed().as_secs_f64() - taken;
                next_look = start.elapsed().as_secs_f64() + (cost * 1.5).clamp(0.3, 1.5);
                k += 1;
            } else {
                core.poll(now, gaze);
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }
        let chains = std::mem::take(&mut core.chains);
        println!("run {run}: {}", chains.iter().map(|c| format!("read {:.2} translated {:.2} shown {:.2}", c.read, c.translated, c.shown)).collect::<Vec<_>>().join(" | "));
        all.extend(chains.iter().map(|c| c.shown));
        assert_eq!(chains.len(), 5, "every line shown");
    }
    all.sort_by(f32::total_cmp);
    println!("shown after the first picture: median {:.2} s, p90 {:.2} s, longest {:.2} s over {} lines", all[all.len() / 2], all[all.len() * 9 / 10], all[all.len() - 1], all.len());
}

/// This process's working set, megabytes (Windows' tasklist).
fn working_set_mb() -> f64 {
    let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {}", std::process::id()), "/FO", "CSV", "/NH"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let field = text.trim().rsplit("\",\"").next().unwrap_or("0").trim_matches('"').replace([',', '.', ' ', 'K', '\u{a0}'], "");
    field.parse::<f64>().unwrap_or(0.0) / 1024.0
}

/// The reader and the built-in translator's memory: loaded, then after a minute of looks at signs of every width and
/// batches of every size. `ORBITERS_TEST_NO_ARENA=1` turns ONNX Runtime's arena off, to compare (run each alone: memory
/// given back is not always returned to Windows), `ORBITERS_TEST_EYE=1620x1800` another eye size.
#[test]
#[ignore]
fn memory_of_the_reader_and_the_built_in_translator() {
    use orbiters_translate::builtin::Builtin;
    let arena = std::env::var_os("ORBITERS_TEST_NO_ARENA").is_none();
    // Enzo's eyes (Quest Pro through Virtual Desktop) unless told otherwise.
    let size: [u32; 2] = std::env::var("ORBITERS_TEST_EYE").ok().and_then(|s| s.split_once('x').map(|(w, h)| [w.parse().unwrap(), h.parse().unwrap()])).unwrap_or([3168, 3456]);
    let at = |v: [EyeView; 2]| v.map(|e| EyeView { size: [size[0] as f32, size[1] as f32], ..e });
    let area = [size[0] * 15 / 100, size[1] * 15 / 100, size[0] * 70 / 100, size[1] * 70 / 100];
    orbiters_translate::ocr::CPU_ARENA.store(arena, std::sync::atomic::Ordering::Relaxed);
    let before = working_set_mb();
    let mut core = Core::new(Some(engine()), Box::new(Builtin::load(4).unwrap()), Reader { target: "fr".into(), reads: vec![] });
    core.no_cache = true;
    std::thread::sleep(std::time::Duration::from_secs(3));
    let loaded = working_set_mb();
    let start = std::time::Instant::now();
    let mut most = loaded;
    let mut k = 0;
    while start.elapsed().as_secs_f64() < 60.0 {
        // Signs at other distances and angles each look: other boxes, other widths.
        let s = sign(1.5 + (k % 7) as f32 * 0.4, -30.0 + (k % 9) as f32 * 7.0, (k % 5) as f32 * 3.0 - 6.0);
        let views = at(views(&head_at(0.0, 0.0)));
        let gaze = (geometry::origin(&views[0].eye), scale(geometry::axis(&views[0].eye, 2), -1.0));
        let pictures = [render_eye(&views[0], size, &plain_room, std::slice::from_ref(&s)), render_eye(&views[1], size, &plain_room, std::slice::from_ref(&s))];
        let zoom = [size[0] / 2 - 480, size[1] / 2 - 480, 960, 960];
        let look = Look { pair: pair(&pictures, views), area, shown: &[], zoom: if k % 2 == 0 && size[0] > 1500 { Some(zoom) } else { None } };
        let now = start.elapsed().as_secs_f64();
        let _ = core.step(now, &look, gaze).unwrap();
        // Every few looks the lines go (out of view), so that they are read and translated again.
        if k % 3 == 2 {
            core.tracker.update(now + 10.0, vec![], &|_| Seen::Away);
            core.tracker.update(now + 20.0, vec![], &|_| Seen::Away);
        }
        for _ in 0..10 {
            core.poll(start.elapsed().as_secs_f64(), gaze);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        most = most.max(working_set_mb());
        k += 1;
    }
    println!("eyes {size:?}, arena {arena}: {before:.0} MB before, {loaded:.0} MB loaded, {most:.0} MB at most over {k} looks, {:.0} MB at the end", working_set_mb());
}
