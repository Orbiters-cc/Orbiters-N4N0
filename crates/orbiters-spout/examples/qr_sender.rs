//! A Spout sender showing one QR code, as VRChat's stream camera would show a poster (K-qr-page, sandbox runs): a debug build
//! of Orbiters reads it in place of VRChat's camera when `ORBITERS_QR_TEST_SENDER` names it. Windows only.
//! `cargo run -p orbiters-spout --example qr_sender -- <name> <text> [module pixels] [negative] [seconds]`
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().expect("qr_sender NAME TEXT [MODULE] [negative] [SECONDS]");
    let text = args.next().expect("qr_sender NAME TEXT [MODULE] [negative] [SECONDS]");
    let module: usize = args.next().and_then(|m| m.parse().ok()).unwrap_or(8);
    let negative = args.next().is_some_and(|n| n == "negative");
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(600);
    let (w, h) = (1280usize, 720usize);
    let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
    let n = code.width();
    let colors = code.to_colors();
    let side = (n + 8) * module;
    let (x0, y0) = ((w - side) / 2, (h - side) / 2);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            // A dim world around a card.
            let mut v = (70 + (x * 7 + y * 3) % 30) as u8;
            if (x0..x0 + side).contains(&x) && (y0..y0 + side).contains(&y) {
                let (mx, my) = ((x - x0) / module, (y - y0) / module);
                let dark = (4..n + 4).contains(&mx) && (4..n + 4).contains(&my) && colors[(my - 4) * n + mx - 4] == qrcode::Color::Dark;
                v = if dark != negative { 25 } else { 235 };
            }
            rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    let mut sender = orbiters_spout::Sender::new(&name, w as u32, h as u32).expect("the Spout sender");
    println!("sending \"{text}\" as \"{name}\" ({w} x {h}, {module} px a module{})", if negative { ", negative" } else { "" });
    let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let mut k = 0u8;
    while std::time::Instant::now() < until {
        // A pixel in a probed row changes, as a live camera's would.
        k = k.wrapping_add(1);
        rgba[(h / 5 * w) * 4] = k;
        sender.send_rgba(&rgba).expect("a frame sent");
        std::thread::sleep(std::time::Duration::from_millis(33));
    }
}
