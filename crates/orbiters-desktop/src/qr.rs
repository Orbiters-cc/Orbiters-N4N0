//! N4N0's QR code scanner on this computer (`orbiters_n4n0::Host::qr_scan`): a thread of its own, below normal priority,
//! reads VRChat's stream camera through Spout (`VRCSender1`, or Settings' Spout sender) about three times a second,
//! makes a grey copy of the frame no wider than [`MOST_WIDTH`] (the whole picture one time, its middle at full detail
//! the next, for a code further away) and decodes the QR codes in it with `rqrr`. N4N0 decides what opens.

use orbiters_n4n0::{QrEvents, QrScan, QrSeen};
use orbiters_i18n::tr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The time between two looks at the camera.
pub const EVERY: Duration = Duration::from_millis(330);
/// A grey copy is at most this wide (and as many rows as keep the picture's shape).
pub const MOST_WIDTH: usize = 960;
/// VRChat's camera's Spout sender.
#[cfg(windows)]
const VRCHAT_SENDER: &str = "VRCSender1";
/// No frame for this long (from the start, or since the last): the camera is said to send none.
#[cfg(windows)]
const NO_PICTURE_AFTER: Duration = Duration::from_millis(2500);

/// A frame's pixels as Spout lends them: 4 bytes a pixel, `stride` bytes a row.
pub struct Pixels<'a> {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub data: &'a [u8],
    /// Blue first (BGRA) rather than red.
    pub bgra: bool,
}

/// Which part of a frame a look decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The whole picture, shrunk to [`MOST_WIDTH`].
    Whole,
    /// Its middle, half as wide and high, at full detail (shrunk only past [`MOST_WIDTH`]).
    Middle,
}

/// A grey copy of `view` of `frame`: (width, height, one byte a pixel). Each grey pixel is the mean of the square of
/// pixels it covers (Rec. 601 luma of the sRGB codes, which is what a QR decoder thresholds anyway).
pub fn grey(frame: &Pixels<'_>, view: View) -> (usize, usize, Vec<u8>) {
    let (x0, y0, w, h) = match view {
        View::Whole => (0, 0, frame.width, frame.height),
        View::Middle => (frame.width / 4, frame.height / 4, frame.width / 2, frame.height / 2),
    };
    let step = w.div_ceil(MOST_WIDTH).max(1);
    let (gw, gh) = (w / step, h / step);
    let (r, b) = if frame.bgra { (2, 0) } else { (0, 2) };
    let mut out = vec![0u8; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let mut sum = 0u32;
            for dy in 0..step {
                let row = (y0 + gy * step + dy) * frame.stride;
                for dx in 0..step {
                    let i = row + (x0 + gx * step + dx) * 4;
                    let p = &frame.data[i..i + 4];
                    sum += 77 * p[r] as u32 + 150 * p[1] as u32 + 29 * p[b] as u32;
                }
            }
            out[gy * gw + gx] = (sum / (256 * (step * step) as u32)) as u8;
        }
    }
    (gw, gh, out)
}

/// The texts of the QR codes in a grey picture.
pub fn decode(width: usize, height: usize, grey: &[u8]) -> Vec<String> {
    if width < 21 || height < 21 {
        return Vec::new();
    }
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| grey[y * width + x]);
    prepared.detect_grids().into_iter().filter_map(|grid| grid.decode().ok().map(|(_, text)| text)).collect()
}

/// The scan's handle: dropped, its thread stops at its next look.
struct Scan(Arc<AtomicBool>);

impl QrScan for Scan {}

impl Drop for Scan {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Starts reading `wanted` (Settings' Spout sender; empty: VRChat's) for QR codes, telling `events` what it sees.
pub fn start(wanted: String, events: QrEvents) -> Result<Box<dyn QrScan>, String> {
    #[cfg(windows)]
    {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("qr-scanner".into())
            .spawn(move || {
                lower_priority();
                run(&wanted, &events, &flag);
            })
            .map_err(|e| tr!("The scanner could not start: {error}", error = e))?;
        Ok(Box::new(Scan(stop)))
    }
    #[cfg(not(windows))]
    {
        let _ = (wanted, events);
        Err(tr("Reading VRChat's camera needs Spout, on Windows").into())
    }
}

/// Below normal priority: VRChat's frames come first.
#[cfg(windows)]
fn lower_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    // SAFETY: the pseudo-handle of the calling thread.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

/// The sender to read: `wanted` when it sends, else VRChat's camera's.
#[cfg(windows)]
fn sender(wanted: &str) -> Option<String> {
    let senders = orbiters_spout::list_senders();
    [wanted, VRCHAT_SENDER].into_iter().filter(|n| !n.is_empty()).find(|n| senders.iter().any(|s| s == n)).map(str::to_string)
}

/// The scanner's thread, until `stop`.
#[cfg(windows)]
fn run(wanted: &str, events: &QrEvents, stop: &AtomicBool) {
    use std::time::Instant;
    let mut receiver = match orbiters_spout::Receiver::new() {
        Ok(r) => r,
        Err(e) => {
            events(QrSeen::Failed(tr!("VRChat's camera cannot be read here: {error}", error = e)));
            return;
        }
    };
    let started = Instant::now();
    let mut connected = false;
    let mut last_frame: Option<Instant> = None;
    // What was last said of the camera (nothing yet).
    let mut said: Option<bool> = None;
    let mut look = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let begun = Instant::now();
        if !connected {
            if let Some(name) = sender(wanted) {
                match receiver.connect(&name) {
                    Ok(info) => {
                        connected = true;
                        // A few one-pixel rows tell a new frame from the last (VRChat's sender counts none).
                        let (w, h) = (info.width, info.height);
                        receiver.set_probe((1..=4).map(|k| h * k / 5).filter(|&y| y < h).map(|y| [0, y, w, y + 1]).collect());
                    }
                    Err(e) => eprintln!("qr: \"{name}\" could not be opened: {e}"),
                }
            }
        }
        let mut found = Vec::new();
        if connected {
            let view = if look % 2 == 0 { View::Whole } else { View::Middle };
            let read = receiver.receive(Duration::from_millis(250), |f| {
                let pixels = Pixels {
                    width: f.width as usize,
                    height: f.height as usize,
                    stride: f.stride,
                    data: f.data,
                    bgra: f.order == orbiters_spout::ChannelOrder::Bgra,
                };
                grey(&pixels, view)
            });
            match read {
                Ok(Some((w, h, picture))) => {
                    last_frame = Some(Instant::now());
                    look += 1;
                    found = decode(w, h, &picture);
                }
                Ok(None) => {}
                Err(e) => {
                    if !matches!(e, orbiters_spout::SpoutError::NotFound(_)) {
                        eprintln!("qr: reading VRChat's camera failed: {e}");
                    }
                    receiver.disconnect();
                    connected = false;
                }
            }
        }
        let frames = last_frame.unwrap_or(started).elapsed() < NO_PICTURE_AFTER && last_frame.is_some();
        // Frames come, or none came for a while (from the start too).
        let camera = if frames { Some(true) } else { (started.elapsed() >= NO_PICTURE_AFTER).then_some(false) };
        if let Some(camera) = camera.filter(|c| said != Some(*c)) {
            said = Some(camera);
            events(QrSeen::Camera(camera));
        }
        for text in found {
            events(QrSeen::Code(text));
        }
        std::thread::sleep(EVERY.saturating_sub(begun.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame `width` × `height` (BGRA, rows padded as a mapped texture's) of a grey world with `text`'s QR code drawn
    /// `module` pixels a module with its top left at (`x`, `y`), and the stride.
    fn frame_with_code(text: &str, width: usize, height: usize, x: usize, y: usize, module: usize) -> (Vec<u8>, usize) {
        let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
        let n = code.width();
        let colors = code.to_colors();
        let stride = width * 4 + 64;
        let mut data = vec![0u8; stride * height];
        for py in 0..height {
            for px in 0..width {
                // A dim, slightly uneven world around a white card with its quiet zone.
                let mut v = (90 + (px * 7 + py * 3) % 40) as u8;
                let (cx, cy) = (px as isize - x as isize, py as isize - y as isize);
                let quiet = 4 * module as isize;
                let side = (n * module) as isize;
                if cx >= -quiet && cy >= -quiet && cx < side + quiet && cy < side + quiet {
                    v = 235;
                    if cx >= 0 && cy >= 0 && cx < side && cy < side {
                        let (mx, my) = (cx as usize / module, cy as usize / module);
                        if colors[my * n + mx] == qrcode::Color::Dark {
                            v = 20;
                        }
                    }
                }
                let i = py * stride + px * 4;
                data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        (data, stride)
    }

    #[test]
    fn a_code_in_a_stream_frame_is_read_from_its_grey_copy() {
        let link = "https://orbiters.cc/maps/wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
        let (w, h) = (1920, 1080);
        let (data, stride) = frame_with_code(link, w, h, 1180, 300, 8);
        let frame = Pixels { width: w, height: h, stride, data: &data, bgra: true };
        let (gw, gh, picture) = grey(&frame, View::Whole);
        assert_eq!((gw, gh), (960, 540), "shrunk to at most {MOST_WIDTH} wide, its shape kept");
        assert_eq!(decode(gw, gh, &picture), [link]);
        // A frame without a code reads nothing.
        let (empty, stride) = frame_with_code(link, 64, 64, 1000, 1000, 1);
        let blank = Pixels { width: 64, height: 64, stride, data: &empty, bgra: false };
        let (gw, gh, picture) = grey(&blank, View::Whole);
        assert!(decode(gw, gh, &picture).is_empty());
    }

    #[test]
    fn a_small_code_in_the_middle_is_read_at_full_detail() {
        let link = "https://example.com/a-rather-long-path/that-makes-a-denser-code?with=query&and=more";
        let (w, h) = (1920, 1080);
        // Two pixels a module: too fine once the whole picture is halved, read in the middle at full size.
        let (data, stride) = frame_with_code(link, w, h, 900, 480, 2);
        let frame = Pixels { width: w, height: h, stride, data: &data, bgra: false };
        let (gw, gh, picture) = grey(&frame, View::Middle);
        assert_eq!((gw, gh), (960, 540));
        assert_eq!(decode(gw, gh, &picture), [link]);
    }

    /// The scanner's thread on a real Spout sender (a test's own name, as Settings' Spout sender): frames come, the code
    /// is read, and once the sender goes the camera is said to send none.
    #[cfg(windows)]
    #[test]
    fn the_scanner_reads_a_spout_sender_and_says_when_it_stops() {
        use std::sync::mpsc;
        let link = "https://orbiters.cc/n4n0";
        let name = format!("OrbitersQrTest{}", std::process::id());
        let (w, h) = (1280usize, 720usize);
        let (bgra, stride) = frame_with_code(link, w, h, 500, 200, 8);
        // Spout wants tightly packed RGBA rows.
        let rgba: Vec<u8> = (0..h).flat_map(|y| bgra[y * stride..y * stride + w * 4].to_vec()).collect();
        let stop_sending = Arc::new(AtomicBool::new(false));
        let sending = {
            let (name, stop) = (name.clone(), stop_sending.clone());
            std::thread::spawn(move || {
                let mut sender = orbiters_spout::Sender::new(&name, w as u32, h as u32).unwrap();
                let mut frame = rgba;
                let mut k = 0u8;
                while !stop.load(Ordering::Relaxed) {
                    // A pixel in a probed row changes, as a live camera's would.
                    k = k.wrapping_add(1);
                    frame[(h / 5 * w) * 4] = k;
                    sender.send_rgba(&frame).unwrap();
                    std::thread::sleep(Duration::from_millis(16));
                }
            })
        };
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let scan = start(name, Box::new(move |seen| tx.lock().unwrap().send(seen).unwrap())).unwrap();
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !seen.contains(&QrSeen::Code(link.into())) && std::time::Instant::now() < deadline {
            if let Ok(s) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.push(s);
            }
        }
        assert_eq!(seen.first(), Some(&QrSeen::Camera(true)), "{seen:?}");
        assert!(seen.contains(&QrSeen::Code(link.into())), "{seen:?}");
        stop_sending.store(true, Ordering::Relaxed);
        sending.join().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut gone = None;
        while gone.is_none() && std::time::Instant::now() < deadline {
            gone = rx.recv_timeout(Duration::from_millis(200)).ok().filter(|s| matches!(s, QrSeen::Camera(_)));
        }
        assert_eq!(gone, Some(QrSeen::Camera(false)));
        drop(scan);
    }

    #[test]
    fn a_4k_frame_shrinks_by_whole_steps() {
        let data = vec![200u8; 3840 * 4 * 2160];
        let frame = Pixels { width: 3840, height: 2160, stride: 3840 * 4, data: &data, bgra: true };
        let (gw, gh, picture) = grey(&frame, View::Whole);
        assert_eq!((gw, gh), (960, 540));
        assert!(picture.iter().all(|&v| v == 200));
        let (mw, mh, _) = grey(&frame, View::Middle);
        assert_eq!((mw, mh), (960, 540));
    }
}
