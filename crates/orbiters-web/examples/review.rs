#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use orbiters_web::{Load, WebPage};
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "crash-child" {
        let mut page = WebPage::new(640, 480)?;
        page.navigate("http://127.0.0.1:9/")?;
        let t = Instant::now();
        loop {
            if let Some((s, _, _)) = page.poll() {
                if matches!(s.load, Load::Failed(_)) {
                    println!("ready");
                    std::io::stdout().flush()?;
                    break;
                }
            }
            anyhow::ensure!(t.elapsed() < Duration::from_secs(30), "startup timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_secs(120));
        return Ok(());
    }
    for immediate in [true, false] {
        let mut page = WebPage::new(640, 480)?;
        page.navigate("http://127.0.0.1:9/")?;
        if !immediate {
            let t = Instant::now();
            loop {
                if let Some((s, _, _)) = page.poll() {
                    if let Load::Failed(e) = s.load {
                        println!("network failure={e}");
                        break;
                    }
                }
                anyhow::ensure!(t.elapsed() < Duration::from_secs(30), "load timeout");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        page.close();
        let t = Instant::now();
        loop {
            if let Some((s, _, _)) = page.poll() {
                if s.load == Load::Closed {
                    break;
                }
            }
            anyhow::ensure!(t.elapsed() < Duration::from_secs(30), "close timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        println!("close immediate={immediate} passed");
    }
    Ok(())
}
