//! A picture of one window, never of the desktop: nothing that covers it shows, and nothing else can. Windows Graphics
//! Capture is asked for that window by its handle, as the capture picker of any streaming app does, and draws no more
//! than its visual; `PrintWindow` asks Windows for the same window's content when that does not work. Neither touches the
//! game's process.
//!
//! Windows Graphics Capture needs a Direct3D device, and the tray service never creates one (`service.rs`: it stays small and
//! loads no GPU driver), so a screenshot runs in a short-lived process of its own (`--bridge-shot`), which answers on its
//! standard output and goes.

use super::game::{Failure, Shot};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// A screenshot's limit, in the child and for waiting on it.
const SHOT_LIMIT: Duration = Duration::from_secs(8);

/// Top-down BGRA.
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// `frame` at most `max_width` wide as a JPEG, and whether it is (almost) black.
pub fn encode(frame: &Bgra, max_width: u32) -> Result<Shot, String> {
    if frame.width == 0 || frame.height == 0 {
        return Err("the window has no pixels".into());
    }
    let mut rgb = Vec::with_capacity(frame.pixels.len() / 4 * 3);
    let mut sum = 0u64;
    for p in frame.pixels.chunks_exact(4) {
        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
        sum += p[0] as u64 + p[1] as u64 + p[2] as u64;
    }
    let all_black = sum / (frame.width as u64 * frame.height as u64 * 3).max(1) < 2;
    let image = image::RgbImage::from_raw(frame.width, frame.height, rgb).ok_or("the frame's size does not match its pixels")?;
    let max_width = max_width.max(64);
    let image = if frame.width > max_width {
        let height = ((frame.height as u64 * max_width as u64) / frame.width as u64).max(1) as u32;
        image::imageops::resize(&image, max_width, height, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let (width, height) = image.dimensions();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
        .encode(image.as_raw(), width, height, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("the picture could not be encoded: {e}"))?;
    Ok(Shot { jpeg, width, height, all_black })
}

/// The picture in this process, for the child and for tests.
pub fn shot(handle: isize, max_width: u32) -> Result<Shot, String> {
    encode(&grab(handle)?, max_width)
}

/// `--bridge-shot <window> <max width>`: answers on standard output with a line of JSON and the JPEG's bytes after it.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let handle: isize = args.first().and_then(|a| a.parse().ok()).ok_or_else(|| anyhow::anyhow!("--bridge-shot <window> <max width>"))?;
    let max_width: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(1280);
    let mut out = std::io::stdout().lock();
    match shot(handle, max_width) {
        Ok(shot) => {
            let head = serde_json::json!({ "width": shot.width, "height": shot.height, "all_black": shot.all_black, "bytes": shot.jpeg.len() });
            writeln!(out, "{head}")?;
            out.write_all(&shot.jpeg)?;
        }
        Err(e) => writeln!(out, "{}", serde_json::json!({ "error": e }))?,
    }
    out.flush()?;
    Ok(())
}

/// Reads what [`run`] answered.
pub fn parse_answer(bytes: &[u8]) -> Result<Shot, String> {
    let at = bytes.iter().position(|b| *b == b'\n').ok_or("no answer")?;
    let head: serde_json::Value = serde_json::from_slice(&bytes[..at]).map_err(|_| "an answer that is not JSON")?;
    if let Some(error) = head["error"].as_str() {
        return Err(error.to_string());
    }
    let jpeg = bytes[at + 1..].to_vec();
    if head["bytes"].as_u64() != Some(jpeg.len() as u64) {
        return Err("a picture cut short".into());
    }
    Ok(Shot {
        jpeg,
        width: head["width"].as_u64().unwrap_or(0) as u32,
        height: head["height"].as_u64().unwrap_or(0) as u32,
        all_black: head["all_black"].as_bool().unwrap_or(false),
    })
}

/// The picture from a process of its own.
pub fn shot_in_child(exe: &std::path::Path, handle: isize, max_width: u32) -> Result<Shot, Failure> {
    let failed = |why: String| {
        Failure::new(
            "screenshot_failed",
            format!("VRChat's window could not be captured: {why}"),
            "Check that VRChat's window is not minimized, then try again.",
        )
    };
    let mut command = std::process::Command::new(exe);
    command.args(["--bridge-shot", &handle.to_string(), &max_width.to_string()]);
    command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| failed(e.to_string()))?;
    let mut stdout = child.stdout.take().ok_or_else(|| failed("no output".into()))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let begin = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if begin.elapsed() < SHOT_LIMIT => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed("it took too long".into()));
            }
        }
    }
    parse_answer(&reader.join().unwrap_or_default()).map_err(failed)
}

#[cfg(not(windows))]
pub fn grab(_handle: isize) -> Result<Bgra, String> {
    Err("window capture is for Windows".into())
}

/// Whether a frame is all but black.
fn looks_black(frame: &Bgra) -> bool {
    let sum: u64 = frame.pixels.chunks_exact(4).map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64).sum();
    sum / (frame.width as u64 * frame.height as u64 * 3).max(1) < 2
}

/// The window's client area: through Windows Graphics Capture, else `PrintWindow`, which also gets its turn when the first
/// gives a black frame (a window off every screen, some swap chains): a black picture is only kept when both say so.
#[cfg(windows)]
pub fn grab(handle: isize) -> Result<Bgra, String> {
    match imp::graphics_capture(handle) {
        Ok(frame) if !looks_black(&frame) => Ok(frame),
        Ok(frame) => Ok(imp::print_window(handle).ok().filter(|other| !looks_black(other)).unwrap_or(frame)),
        Err(first) => imp::print_window(handle).map_err(|second| format!("{first}; {second}")),
    }
}

#[cfg(windows)]
pub(crate) mod imp {
    use super::*;
    use windows::core::Interface;
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::{
        ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsIconic};

    /// How long a frame may take to come.
    const FIRST_FRAME: Duration = Duration::from_millis(2500);
    /// `PW_CLIENTONLY | PW_RENDERFULLCONTENT`: the client area, with what the GPU drew.
    const PRINT_FLAGS: u32 = 0x1 | 0x2;

    /// Where the client area is within the frame Windows Graphics Capture gives (the window's visual bounds): its corner
    /// and size, inside the frame.
    fn client_in_frame(window: HWND, frame_width: u32, frame_height: u32) -> (u32, u32, u32, u32) {
        let (mut client, mut corner, mut bounds) = (RECT::default(), POINT::default(), RECT::default());
        // SAFETY: the outputs and their sizes are passed; a window gone meanwhile only makes these fail, and the whole
        // frame is used then.
        let known = unsafe {
            GetClientRect(window, &mut client).is_ok()
                && ClientToScreen(window, &mut corner).as_bool()
                && DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS, &mut bounds as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok()
        };
        if !known {
            return (0, 0, frame_width, frame_height);
        }
        let x = (corner.x - bounds.left).max(0) as u32;
        let y = (corner.y - bounds.top).max(0) as u32;
        let width = ((client.right - client.left).max(0) as u32).min(frame_width.saturating_sub(x));
        let height = ((client.bottom - client.top).max(0) as u32).min(frame_height.saturating_sub(y));
        if width == 0 || height == 0 {
            (0, 0, frame_width, frame_height)
        } else {
            (x, y, width, height)
        }
    }

    pub(crate) fn graphics_capture(handle: isize) -> Result<Bgra, String> {
        let text = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
        let window = HWND(handle as *mut _);
        // SAFETY: a plain query.
        if unsafe { IsIconic(window) }.as_bool() {
            return Err("the window is minimized".into());
        }
        // SAFETY: WinRT in this thread's apartment; it stays for the process, which ends after this picture.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(|e| text("capture item", e))?;
        // SAFETY: a window handle; asking for one that is gone only fails.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }.map_err(|e| text("capture item", e))?;
        let size = item.Size().map_err(|e| text("size", e))?;
        let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
        // SAFETY: the outputs are valid; the device lives in this function.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| text("Direct3D", e))?;
        let (device, context) = (device.ok_or("no Direct3D device")?, context.ok_or("no Direct3D context")?);
        let dxgi: IDXGIDevice = device.cast().map_err(|e| text("DXGI", e))?;
        // SAFETY: the DXGI device is alive for the call.
        let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.and_then(|d| d.cast()).map_err(|e| text("device", e))?;
        let pool =
            Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, size).map_err(|e| text("frame pool", e))?;
        let session = pool.CreateCaptureSession(&item).map_err(|e| text("session", e))?;
        // Menus need the pointer in the picture; Windows' yellow border is left off where it allows that.
        let _ = session.SetIsCursorCaptureEnabled(true);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|e| text("start", e))?;
        let begin = Instant::now();
        let frame = loop {
            match pool.TryGetNextFrame() {
                Ok(frame) => break frame,
                Err(_) if begin.elapsed() < FIRST_FRAME => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => return Err(text("no frame came", e)),
            }
        };
        let surface = frame.Surface().map_err(|e| text("surface", e))?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|e| text("surface", e))?;
        // SAFETY: the surface is a Direct3D 11 texture, as the pool was made for.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(|e| text("texture", e))?;
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the output is valid.
        unsafe { texture.GetDesc(&mut description) };
        let (frame_width, frame_height) = (description.Width, description.Height);
        let staging_description = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: D3D11_BIND_FLAG(0).0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0).0 as u32,
            ..description
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        // SAFETY: the description and the output are valid.
        unsafe { device.CreateTexture2D(&staging_description, None, Some(&mut staging)) }.map_err(|e| text("staging", e))?;
        let staging = staging.ok_or("no staging texture")?;
        // SAFETY: both textures are this device's, of one size and format; the mapping is read within its stride and height,
        // and unmapped before anything else happens to it.
        let pixels = unsafe {
            context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| text("map", e))?;
            let (x, y, width, height) = client_in_frame(window, frame_width, frame_height);
            let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
            for row in y..y + height {
                let start = (mapped.pData as *const u8).add(row as usize * mapped.RowPitch as usize + x as usize * 4);
                pixels.extend_from_slice(std::slice::from_raw_parts(start, width as usize * 4));
            }
            context.Unmap(&staging, 0);
            Bgra { width, height, pixels }
        };
        let _ = session.Close();
        let _ = pool.Close();
        Ok(pixels)
    }

    pub(crate) fn print_window(handle: isize) -> Result<Bgra, String> {
        let window = HWND(handle as *mut _);
        let mut client = RECT::default();
        // SAFETY: the output is valid.
        unsafe { GetClientRect(window, &mut client) }.map_err(|e| format!("window size: {}", e.message()))?;
        let (width, height) = ((client.right - client.left).max(0), (client.bottom - client.top).max(0));
        if width == 0 || height == 0 {
            return Err("the window has no pixels".into());
        }
        // The first try of a process has come out black while the window has content: a second try after a moment does not.
        for attempt in 0..3 {
            let frame = print_once(window, width, height)?;
            if attempt == 2 || !looks_black(&frame) {
                return Ok(frame);
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        unreachable!("the last try returns")
    }

    fn print_once(window: HWND, width: i32, height: i32) -> Result<Bgra, String> {
        // SAFETY: device contexts and a bitmap of this function, released below; the output buffer is one 32-bit row per
        // line of the bitmap, as the header says.
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let before = SelectObject(dc, bitmap.into());
            let printed = PrintWindow(window, dc, PRINT_WINDOW_FLAGS(PRINT_FLAGS)).as_bool();
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            let lines = GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
            SelectObject(dc, before);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            if !printed || lines == 0 {
                return Err("PrintWindow did not answer".into());
            }
            Ok(Bgra { width: width as u32, height: height as u32, pixels })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, pixel: [u8; 4]) -> Bgra {
        Bgra { width, height, pixels: pixel.repeat((width * height) as usize) }
    }

    #[test]
    fn a_large_window_comes_at_the_asked_width_and_its_shape() {
        let shot = encode(&frame(2560, 1440, [40, 80, 160, 255]), 1280).unwrap();
        assert_eq!((shot.width, shot.height), (1280, 720));
        assert!(!shot.all_black);
        let decoded = image::load_from_memory(&shot.jpeg).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (1280, 720));
        let p = decoded.get_pixel(640, 360);
        assert!((p[0] as i32 - 160).abs() < 6 && (p[2] as i32 - 40).abs() < 6, "red and blue the right way round: {p:?}");
    }

    #[test]
    fn a_small_window_stays_its_size_so_pixels_are_coordinates() {
        let shot = encode(&frame(800, 600, [1, 2, 3, 255]), 1280).unwrap();
        assert_eq!((shot.width, shot.height), (800, 600));
    }

    #[test]
    fn a_black_picture_says_so() {
        assert!(encode(&frame(64, 64, [0, 0, 0, 255]), 1280).unwrap().all_black);
        assert!(encode(&Bgra { width: 0, height: 0, pixels: vec![] }, 1280).is_err());
    }

    #[test]
    fn the_childs_answer_is_read_or_refused() {
        let shot = encode(&frame(64, 64, [9, 9, 200, 255]), 64).unwrap();
        let head = serde_json::json!({ "width": 64, "height": 64, "all_black": false, "bytes": shot.jpeg.len() });
        let mut bytes = format!("{head}\n").into_bytes();
        bytes.extend_from_slice(&shot.jpeg);
        assert_eq!(parse_answer(&bytes).unwrap().jpeg, shot.jpeg);
        assert_eq!(parse_answer(&bytes[..bytes.len() - 1]).err().unwrap(), "a picture cut short");
        assert_eq!(parse_answer(b"{\"error\":\"the window is minimized\"}\n").err().unwrap(), "the window is minimized");
        assert!(parse_answer(b"").is_err());
    }
}
