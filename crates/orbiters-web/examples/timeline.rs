//! K-qr-page probe (2026-11-15): opens real pages in the camera's page engine at the size the overlay uses (820 × 940)
//! and times what a person waits for: the first state, the page loaded, its first picture, then a turn of the wheel and a
//! click until the picture changes. `cargo run -p orbiters-web --example timeline -- <out folder> [urls...]`.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use orbiters_web::{Input, Load, WebPage};
    use std::time::{Duration, Instant};
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("timeline OUTPUT_DIRECTORY [URL...]"));
    std::fs::create_dir_all(&dir)?;
    let mut urls: Vec<String> = args.collect();
    if urls.is_empty() {
        urls = [
            "https://vrcidlinker.com/",
            "https://qrco.de/bdeKjG",
            "https://linktr.ee/the_real_nagia",
            "http://github.com/",
            "https://en.wikipedia.org/wiki/Virtual_reality",
            "https://httpbin.org/delay/6",
        ]
        .map(String::from)
        .to_vec();
    }
    let save = |name: &str, f: &orbiters_web::Frame| -> anyhow::Result<()> {
        let mut e = png::Encoder::new(std::fs::File::create(dir.join(name))?, f.width, f.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(&f.rgba)?;
        Ok(())
    };
    for (n, url) in urls.iter().enumerate() {
        println!("== {url}");
        let start = Instant::now();
        let mut page = WebPage::new(820, 940)?;
        page.navigate(url)?;
        let ms = |t: Instant| t.elapsed().as_millis();
        let mut last_load: Option<Load> = None;
        let mut first_frame: Option<u64> = None;
        let mut loaded_at = None;
        let mut sequence = 0;
        // Up to 30 s for the page and its first picture.
        while start.elapsed() < Duration::from_secs(30) {
            if let Some((s, f, _)) = page.poll() {
                if last_load.as_ref() != Some(&s.load) {
                    println!("  {:>6} ms  {:?}  url={} title={:?}", ms(start), s.load, s.url, s.title);
                    if s.load == Load::Loaded {
                        loaded_at = Some(ms(start));
                    }
                    last_load = Some(s.load.clone());
                }
                if let Some(f) = f {
                    if first_frame.is_none() {
                        println!("  {:>6} ms  first picture {} x {}", ms(start), f.width, f.height);
                        save(&format!("{n:02}-first.png"), &f)?;
                        first_frame = Some(f.sequence);
                    }
                    sequence = f.sequence;
                }
                if matches!(s.load, Load::Failed(_)) || (first_frame.is_some() && loaded_at.is_some()) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if first_frame.is_none() {
            println!("  STUCK: no picture after {} ms (state {:?})", ms(start), last_load);
            page.close();
            continue;
        }
        // Let it settle, then a turn of the wheel: how long until the picture shows it.
        std::thread::sleep(Duration::from_millis(1500));
        if let Some((_, Some(f), _)) = page.poll() {
            sequence = f.sequence;
        }
        for (what, inputs) in [
            ("wheel 400 px", vec![Input::Wheel { x: 410., y: 600., dx: 0., dy: 400. }]),
            ("click", vec![Input::Move { x: 410., y: 300. }, Input::Press { x: 410., y: 300. }, Input::Release { x: 410., y: 300. }]),
            ("wheel back", vec![Input::Wheel { x: 410., y: 600., dx: 0., dy: -400. }]),
        ] {
            let at = Instant::now();
            for i in inputs {
                page.input(i)?;
            }
            let mut changed = None;
            while at.elapsed() < Duration::from_secs(5) {
                if let Some((_, Some(f), _)) = page.poll() {
                    if f.sequence != sequence {
                        sequence = f.sequence;
                        changed = Some(at.elapsed().as_millis());
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            println!("  {what}: picture changed after {changed:?} ms");
            std::thread::sleep(Duration::from_millis(1200));
            if let Some((_, Some(f), _)) = page.poll() {
                sequence = f.sequence;
            }
        }
        // A drag's worth of wheel steps (a laser sliding the page at 90 Hz for a second): pictures that came.
        let at = Instant::now();
        let mut pictures = 0;
        for _ in 0..90 {
            let _ = page.input(Input::Wheel { x: 410., y: 600., dx: 0., dy: 12. });
            let until = Instant::now() + Duration::from_millis(11);
            while Instant::now() < until {
                if let Some((_, Some(f), _)) = page.poll() {
                    if f.sequence != sequence {
                        sequence = f.sequence;
                        pictures += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        println!("  slide for {} ms: {pictures} new pictures", at.elapsed().as_millis());
        if let Some((_, Some(f), _)) = page.poll() {
            save(&format!("{n:02}-after.png"), &f)?;
        }
        page.close();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if page.poll().is_some_and(|(s, _, _)| s.load == Load::Closed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
