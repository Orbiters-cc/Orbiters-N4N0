//! Timing and reading of the text detector and recognisers on real VRChat eye pictures (the sandbox's captures) and on
//! signs of known text in five scripts seen at an angle, at the sandbox's and at Enzo's headset's resolution. Run by hand:
//! `cargo test -p orbiters-translate --test bench -- --ignored --nocapture` with the models in `ORBITERS_TRANSLATE_MODELS`.

use orbiters_translate::geometry::{self, EyeView, Mirror};
use orbiters_translate::ocr::{det_size, DetInput, Detector, Reader, Recogniser, TextBox};
use orbiters_translate::picture::{draw_text, plain_room, render_eye, Picture, Sign, Text};
use std::path::PathBuf;
use std::time::Instant;

fn models() -> PathBuf {
    std::env::var_os("ORBITERS_TRANSLATE_MODELS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:/Users/Enzo/.orbiters-build/vr-feedback-1111/translate3/models"))
}
fn frames() -> PathBuf {
    PathBuf::from("C:/Users/Enzo/.orbiters-build/vr-feedback-1111/sandbox-camera")
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn rec(name: &str, reader: Reader, threads: usize) -> Recogniser {
    let dir = models().join(name);
    Recogniser::load(&dir.join("inference.onnx"), &dir.join("inference.yml"), reader, threads).unwrap()
}

/// Quest Pro's left eye as SteamVR gives it (the overlay's tests use the same numbers).
const QUEST_PRO: [f32; 4] = [-1.376, 0.839, -1.376, 0.900];

fn signs() -> Vec<Sign> {
    let lines = [("出口はこちらです", [20, 20, 20]), ("欢迎来到我们的世界", [20, 20, 20]), ("안녕하세요 친구들", [20, 20, 20]), ("Добро пожаловать", [20, 20, 20]), ("Welcome to the club", [20, 20, 20])];
    let texts: Vec<Text> = lines.iter().enumerate().map(|(i, (t, c))| Text { text: t.to_string(), at: [40.0, 30.0 + i as f32 * 110.0], size: 72.0, colour: *c }).collect();
    let picture = draw_text(900, 600, [235, 228, 210], &texts);
    // 2.2 m ahead, a little to the left, turned 25 degrees and rolled 6.
    let pose = geometry::compose(&geometry::rotation([0.0, 1.0, 0.0], 25f32.to_radians(), [-0.25, 1.55, -2.2]), &geometry::rotation([0.0, 0.0, 1.0], 6f32.to_radians(), [0.0; 3]));
    vec![Sign { picture, pose, width: 1.2 }]
}

fn eye(size: [u32; 2]) -> EyeView {
    let eye = [[1.0, 0.0, 0.0, -0.032], [0.0, 1.0, 0.0, 1.6], [0.0, 0.0, 1.0, 0.0]];
    EyeView { eye, mirror: Mirror::whole_eye(QUEST_PRO), size: [size[0] as f32, size[1] as f32] }
}

fn detect(det: &mut Detector, p: &Picture, area: [u32; 4], long: u32) -> (Vec<TextBox>, f64) {
    let size = det_size(area, long);
    let t = Instant::now();
    let boxes = det.detect(&DetInput { picture: p.view(), area, size, hidden: &[] }).unwrap();
    (boxes, ms(t))
}

#[test]
#[ignore]
fn benchmark() {
    let mut log = String::new();
    let mut say = |s: String| {
        println!("{s}");
        log.push_str(&s);
        log.push('\n');
    };
    say(format!("models {}", models().display()));
    let mut synthetic = Vec::new();
    for size in [[1620u32, 1800u32], [3168, 3456]] {
        let t = Instant::now();
        let p = render_eye(&eye(size), size, &plain_room, &signs());
        say(format!("rendered a {}x{} eye in {:.0} ms", size[0], size[1], ms(t)));
        p.save_png(&models().join(format!("../bench-sign-{}.png", size[0]))).ok();
        synthetic.push((format!("sign-{}", size[0]), p));
    }
    let mut real: Vec<(String, Picture)> = ["camera-tab-click-left.png", "camera-grab-move-left.png", "current-left.png", "camera-icon-aim-left.png"]
        .iter()
        .filter_map(|f| Picture::load_png(&frames().join(f)).ok().map(|p| (f.to_string(), p)))
        .collect();
    real.extend(synthetic);
    for det_name in ["PP-OCRv5_mobile_det_onnx", "PP-OCRv6_small_det_onnx"] {
        for threads in [1usize, 2, 4] {
            let mut det = Detector::load(&models().join(det_name).join("inference.onnx"), threads).unwrap();
            for (name, p) in &real {
                let whole = [0, 0, p.width, p.height];
                // The middle of the eye, where things are looked at (60 % each way).
                let middle = [p.width / 5, p.height / 5, p.width * 3 / 5, p.height * 3 / 5];
                for (label, area, long) in [("whole@640", whole, 640), ("whole@960", whole, 960), ("whole@1280", whole, 1280), ("middle@960", middle, 960), ("middle@1280", middle, 1280)] {
                    let _ = detect(&mut det, p, area, long); // warm
                    let mut times = Vec::new();
                    let mut n = 0;
                    for _ in 0..3 {
                        let (b, t) = detect(&mut det, p, area, long);
                        n = b.len();
                        times.push(t);
                    }
                    times.sort_by(|a, b| a.total_cmp(b));
                    say(format!("det {det_name} threads {threads} {name} {label} input {:?}: {n} lines, median {:.0} ms", det_size(area, long), times[1]));
                }
            }
        }
    }
    // Reading: v6 small (zh, ja, en, Latin), v6 tiny (no Japanese), Korean and East Slavic v5 on the lines found at 1280.
    let mut det = Detector::load(&models().join("PP-OCRv5_mobile_det_onnx").join("inference.onnx"), 2).unwrap();
    for (rec_name, reader) in [("PP-OCRv6_small_rec_onnx", Reader::Main), ("PP-OCRv6_tiny_rec_onnx", Reader::Main), ("korean_PP-OCRv5_mobile_rec_onnx", Reader::Korean), ("eslav_PP-OCRv5_mobile_rec_onnx", Reader::Cyrillic)] {
        for threads in [1usize, 2] {
            let mut r = rec(rec_name, reader, threads);
            for (name, p) in &real {
                let (boxes, _) = detect(&mut det, p, [0, 0, p.width, p.height], 1280);
                let _ = r.read(p.view(), &boxes);
                let t = Instant::now();
                let lines = r.read(p.view(), &boxes).unwrap();
                let took = ms(t);
                say(format!("rec {rec_name} threads {threads} {name}: {} lines in {took:.0} ms ({:.1} ms a line)", lines.len(), took / lines.len().max(1) as f64));
                if threads == 2 {
                    for l in &lines {
                        let c = l.boxed.centre();
                        say(format!("    {:.2} [{:.0},{:.0}] h{:.0} {}{}", l.confidence, c[0], c[1], l.boxed.thickness(), if l.boxed.vertical { "(vertical) " } else { "" }, l.text));
                    }
                }
            }
        }
    }
    std::fs::write(models().join("../bench.txt"), log).unwrap();
}
