#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use orbiters_web::{Frame, Input, Load, WebPage};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    fn wait(page: &mut WebPage, predicate: impl Fn(&orbiters_web::State, &Option<Frame>) -> bool) -> anyhow::Result<(orbiters_web::State, Option<Frame>)> {
        let start = Instant::now();
        loop {
            if let Some((s, f, _)) = page.poll() {
                if predicate(&s, &f) {
                    return Ok((s, f));
                }
                if let Load::Failed(e) = s.load {
                    anyhow::bail!("browser failed: {e}");
                }
            }
            anyhow::ensure!(start.elapsed() < Duration::from_secs(30), "browser timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn save(dir: &std::path::Path, name: &str, f: &Frame) -> anyhow::Result<()> {
        let mut encoder = png::Encoder::new(std::fs::File::create(dir.join(name))?, f.width, f.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&f.rgba)?;
        Ok(())
    }
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("proof OUTPUT_DIRECTORY"));
    std::fs::create_dir_all(&dir)?;
    if std::env::args().any(|a| a == "--expect-unavailable") {
        let mut page = WebPage::new(960, 720)?;
        let start = Instant::now();
        loop {
            if let Some((s, _, _)) = page.poll() {
                if let Load::Failed(reason) = s.load {
                    anyhow::ensure!(reason.contains("runtime unavailable"), "unexpected failure: {reason}");
                    println!("unavailable={reason}");
                    page.close();
                    return Ok(());
                }
            }
            anyhow::ensure!(start.elapsed() < Duration::from_secs(10), "runtime failure not reported");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let server = TcpListener::bind("127.0.0.1:0")?;
    let port = server.local_addr()?.port();
    server.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            match server.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let mut b = [0; 4096];
                    let mut n = 0;
                    while n < b.len() {
                        match stream.read(&mut b[n..]) {
                            Ok(0) | Err(_) => break,
                            Ok(k) => {
                                n += k;
                                if b[..n].windows(4).any(|s| s == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let request = String::from_utf8_lossy(&b[..n]);
                    let html = if request.starts_with("GET /next") {
                        "<title>Link reached</title><body style='background:#135;color:white;font:40px sans-serif'>LINK NAVIGATION WORKED</body>".to_owned()
                    } else {
                        include_str!("long.html").to_owned()
                    };
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    let start = Instant::now();
    let mut page = WebPage::new(960, 720)?;
    page.navigate(&format!("http://127.0.0.1:{port}/"))?;
    fn pixel(f: &Frame, x: usize, y: usize, rgb: &[u8]) -> bool {
        f.rgba.get((y * f.width as usize + x) * 4..(y * f.width as usize + x) * 4 + 3) == Some(rgb)
    }
    let (_, f) = wait(&mut page, |s, f| s.load == Load::Loaded && f.as_ref().is_some_and(|f| pixel(f, 400, 400, &[16, 40, 60])))?;
    let first = f.unwrap();
    save(&dir, "01-loaded.png", &first)?;
    println!("first_content_ms={}", start.elapsed().as_millis());
    let scroll = Instant::now();
    page.input(Input::Wheel { x: 400., y: 400., dx: 0., dy: 650. })?;
    let (_, f) = wait(&mut page, |_, f| f.as_ref().is_some_and(|f| f.sequence > first.sequence && pixel(f, 400, 400, &[123, 59, 24])))?;
    let second = f.unwrap();
    save(&dir, "02-scrolled.png", &second)?;
    println!("scroll_frame_ms={}", scroll.elapsed().as_millis());
    anyhow::ensure!(first.rgba != second.rgba, "scroll did not change pixels");
    // The fixed link remains at (40,35) after scrolling.
    page.input(Input::Move { x: 70., y: 35. })?;
    page.input(Input::Press { x: 70., y: 35. })?;
    page.input(Input::Release { x: 70., y: 35. })?;
    let (_, f) = wait(&mut page, |s, f| {
        s.load == Load::Loaded && s.url.ends_with("/next") && s.title == "Link reached" && f.as_ref().is_some_and(|f| f.sequence > second.sequence)
    })?;
    save(&dir, "03-link.png", &f.unwrap())?;
    println!("link_navigation=passed");
    std::thread::sleep(Duration::from_millis(500));
    let _ = page.poll();
    std::thread::sleep(Duration::from_millis(500));
    let (_, _, changed) = page.poll().expect("worker snapshot");
    anyhow::ensure!(!changed, "static page kept publishing frames");
    println!("static_changed=false");
    page.resize(800, 600)?;
    wait(&mut page, |_, f| f.as_ref().is_some_and(|f| f.width == 800 && f.height == 600))?;
    println!("resize=passed");
    // Reserve then release an unused port: connection must fail, without external traffic.
    let unavailable = TcpListener::bind("127.0.0.1:0")?;
    let bad = unavailable.local_addr()?.port();
    drop(unavailable);
    page.navigate(&format!("http://127.0.0.1:{bad}/fail"))?;
    let t = Instant::now();
    loop {
        if let Some((s, _, _)) = page.poll() {
            if let Load::Failed(reason) = s.load {
                println!("failure={reason}");
                break;
            }
        }
        anyhow::ensure!(t.elapsed() < Duration::from_secs(30), "failure not reported");
        std::thread::sleep(Duration::from_millis(10));
    }
    page.close();
    let cleanup = Instant::now();
    loop {
        if let Some((s, _, _)) = page.poll() {
            if s.load == Load::Closed {
                break;
            }
        }
        anyhow::ensure!(cleanup.elapsed() < Duration::from_secs(10), "close cleanup timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
    println!("closed=passed total_ms={}", start.elapsed().as_millis());
    Ok(())
}
