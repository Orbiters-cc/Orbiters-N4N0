//! Request 18 probe: how long a code's page takes from the code read to its first picture, with the engine started then (cold, as 0.3.8-dev.1
//! opened every link) and with one started ahead and ready (warm, as the camera's QR mode now keeps one). A private, throwaway profile each time;
//! nothing of the person's goes to the sites. `cargo run -p orbiters-web --example open_times -- [urls...]`.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use orbiters_web::{Load, WebPage};
    use std::time::{Duration, Instant};
    let mut urls: Vec<String> = std::env::args().skip(1).collect();
    if urls.is_empty() {
        urls = ["https://qrco.de/bdeKjG", "https://linktr.ee/", "https://en.wikipedia.org/wiki/Virtual_reality", "https://store.steampowered.com/", "https://github.com/"]
            .map(String::from)
            .to_vec();
    }
    // From `from`: the first picture and the load (ms), 30 s at most.
    let timeline = |page: &mut WebPage, from: Instant| -> (Option<u128>, Option<u128>, Option<String>) {
        let (mut first, mut loaded, mut failed) = (None, None, None);
        while from.elapsed() < Duration::from_secs(30) && (first.is_none() || loaded.is_none()) && failed.is_none() {
            if let Some((s, f, _)) = page.poll() {
                if f.is_some() && first.is_none() {
                    first = Some(from.elapsed().as_millis());
                }
                match s.load {
                    Load::Loaded if loaded.is_none() => loaded = Some(from.elapsed().as_millis()),
                    Load::Failed(why) => failed = Some(why),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        (first, loaded, failed)
    };
    for url in &urls {
        let read = Instant::now();
        let mut cold = WebPage::new(820, 940)?;
        cold.navigate(url)?;
        let (c_first, c_loaded, c_failed) = timeline(&mut cold, read);
        cold.close();
        let started = Instant::now();
        let mut warm = WebPage::new(820, 940)?;
        while !warm.ready() && started.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let ready = started.elapsed().as_millis();
        let read = Instant::now();
        warm.navigate(url)?;
        let (w_first, w_loaded, w_failed) = timeline(&mut warm, read);
        println!("{url}\n  cold: first picture {c_first:?} ms, loaded {c_loaded:?} ms{}\n  warm (ready {ready} ms after its start): first picture {w_first:?} ms, loaded {w_loaded:?} ms{}", c_failed.map(|f| format!(", failed: {f}")).unwrap_or_default(), w_failed.map(|f| format!(", failed: {f}")).unwrap_or_default());
        println!("  warm engine's notes: {:?}", warm.notes());
        warm.close();
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}
