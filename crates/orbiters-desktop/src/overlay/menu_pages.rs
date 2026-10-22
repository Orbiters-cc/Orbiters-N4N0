//! `orbiters-desktop --orb-menu <out>`: Orb's menu pages as files.
//!
//! - `Orb screen pages.png`: the picture of every page's unchanging part, one 1024 × 576 slice per page (slice 0 is
//!   empty; slice `n` is page `n`), top first: Toolkit imports it as the projected screen's page texture array.
//! - `Orb screen layout.png`: where each page's changing parts go, a row per page, for the screen's shader (see
//!   [`layout_table`]).
//! - `menu-<page>.png`: each page as the SteamVR overlay shows it with sample content, for reviewing the design;
//!   `bank-<page>.bin`: what the avatar's screen is sent for the same page (its 744 bytes, 72-byte thumbnails), to draw
//!   it with the screen's shader and compare; `bank-results-128.bin`, the results page with 128-byte thumbnails (1024
//!   bytes).

use super::menu_ui::{self, View};
use super::Canvas;
use egui::{pos2, ColorImage, TextureHandle, TextureId, TextureOptions};
use orbiters_orb::menu::layout::{self, ScreenPage, MAX_ELEMENTS, MAX_SLOTS, PICTURES};
use orbiters_orb::menu::{screen, Call, Contact, KeyboardLayout, Keys, Menu, Page, PhotoLength, Target};
use orbiters_orb::picture;
use orbiters_orb::youtube::{Results, Video};
use std::path::Path;

pub const WIDTH: u32 = layout::WIDTH as u32;
pub const HEIGHT: u32 = layout::HEIGHT as u32;
/// Slices of the page texture: the empty one and every page.
pub const SLICES: usize = ScreenPage::ALL.len() + 1;
/// The layout table's width in texels.
pub const TABLE_WIDTH: usize = 160;
/// Where the table's parts start in a row.
pub const TABLE_SLOTS: usize = 1;
pub const TABLE_PICTURES: usize = TABLE_SLOTS + 2 * MAX_SLOTS;
pub const TABLE_ELEMENTS: usize = 40;
pub const TABLE_VERSION: u8 = 1;
const _: () = assert!(TABLE_PICTURES + PICTURES <= TABLE_ELEMENTS && TABLE_ELEMENTS + 2 * MAX_ELEMENTS <= TABLE_WIDTH);

/// The layout table, RGBA8, one row per page id (row 0 empty). Positions and sizes are in 4-pixel units, a byte each,
/// so 1020 pixels at most: nothing on a page is as wide as the page itself (1024), which the layout's test checks.
///
/// - Texel 0: version, slots, pictures, elements.
/// - Texels 1 to 32, two per text slot: (x, y, columns, cell height / 2), (text offset low byte, high byte,
///   style + 16 × align, 0).
/// - Texels 33 to 37, one per picture: (x, y, width, height).
/// - Texels 40 to 135, two per element: (x, y, width, height), (corner radius in pixels, look, 0, 0).
pub fn layout_table() -> Vec<u8> {
    let mut table = vec![0u8; TABLE_WIDTH * SLICES * 4];
    for page in ScreenPage::ALL {
        let l = layout::layout(page);
        let row = page as usize * TABLE_WIDTH * 4;
        let mut put = |texel: usize, v: [u8; 4]| table[row + texel * 4..row + texel * 4 + 4].copy_from_slice(&v);
        // A value past 1020 pixels would wrap to a small one: refused rather than written wrong.
        let g = |v: u16| u8::try_from(v / layout::GRID).expect("the layout table holds positions and sizes up to 1020 pixels");
        put(0, [TABLE_VERSION, l.slots.len() as u8, l.pictures.len() as u8, l.elements.len() as u8]);
        for (i, s) in l.slots.iter().enumerate() {
            put(TABLE_SLOTS + 2 * i, [g(s.x), g(s.y), s.cols, s.size / 2]);
            put(TABLE_SLOTS + 2 * i + 1, [(s.offset & 0xff) as u8, (s.offset >> 8) as u8, s.style as u8 + 16 * s.align as u8, 0]);
        }
        for (i, p) in l.pictures.iter().enumerate() {
            put(TABLE_PICTURES + i, [g(p.rect.x), g(p.rect.y), g(p.rect.w), g(p.rect.h)]);
        }
        for (i, e) in l.elements.iter().enumerate() {
            put(TABLE_ELEMENTS + 2 * i, [g(e.rect.x), g(e.rect.y), g(e.rect.w), g(e.rect.h)]);
            put(TABLE_ELEMENTS + 2 * i + 1, [e.radius() as u8, e.look as u8, 0, 0]);
        }
    }
    table
}

/// Premultiplied RGBA to straight alpha (what PNG files hold).
fn unpremultiply(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            if a == 0 {
                [0, 0, 0, 0]
            } else {
                let c = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
                [c(p[0]), c(p[1]), c(p[2]), p[3]]
            }
        })
        .collect()
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Paints one frame twice (fonts and textures load on the first).
fn paint(canvas: &mut Canvas, draw: impl Fn(&egui::Painter)) -> Vec<u8> {
    let mut rgba = Vec::new();
    for _ in 0..2 {
        rgba = canvas.paint(1.0, |p| draw(p)).to_vec();
    }
    rgba
}

/// The page texture: every page's unchanging part, slice by slice, straight alpha.
pub fn page_pictures() -> Vec<u8> {
    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let slice = (WIDTH * HEIGHT * 4) as usize;
    let mut out = vec![0u8; slice * SLICES];
    for page in ScreenPage::ALL {
        let l = layout::layout(page);
        let rgba = paint(&mut canvas, |p| menu_ui::draw_chrome(p, pos2(0.0, 0.0), &l));
        let at = page as usize * slice;
        out[at..at + slice].copy_from_slice(&unpremultiply(&rgba));
    }
    out
}

/// A stand-in thumbnail: bands of colour (160 × 90, RGBA), with a disc.
fn sample_rgba(seed: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = (160usize, 90usize);
    let palette = [[226, 120, 82], [92, 140, 220], [120, 200, 140], [200, 110, 200], [230, 200, 90]];
    let base = palette[seed % palette.len()];
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let t = (x as f32 / w as f32 + y as f32 / h as f32 * 0.5 + seed as f32 * 0.13).fract();
            let disc = ((x as f32 - 50.0 - seed as f32 * 12.0).powi(2) + (y as f32 - 45.0).powi(2)).sqrt() < 26.0;
            let k = if disc { 1.0 } else { 0.25 + 0.55 * t };
            rgba.extend([(base[0] as f32 * k).min(255.0) as u8, (base[1] as f32 * k).min(255.0) as u8, (base[2] as f32 * k).min(255.0) as u8, 255]);
        }
    }
    (w, h, rgba)
}

fn sample_thumbnail(ctx: &egui::Context, seed: usize) -> TextureHandle {
    let (w, h, rgba) = sample_rgba(seed);
    ctx.load_texture(format!("thumb{seed}"), ColorImage::from_rgba_unmultiplied([w, h], &rgba), TextureOptions::LINEAR)
}

/// The stand-in thumbnail as the avatar's screen gets it (as a real one: fitted and encoded).
fn sample_picture(seed: usize) -> screen::Picture {
    let (w, h, rgba) = sample_rgba(seed);
    let rgb = picture::Rgb { width: w, height: h, pixels: rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect() };
    screen::picture_from_rgb(&rgb)
}

/// Menus with sample content, one per page, with the element hovered and pressed.
fn samples() -> Vec<(ScreenPage, Menu, Option<usize>, Option<usize>)> {
    let mut out = Vec::new();
    let base = || {
        let mut m = Menu::new(KeyboardLayout::Qwerty, 5, PhotoLength::Standard);
        m.open();
        m.calls_ready = true;
        m.contacts = ["Kasumi", "Lyra Nightfall", "Bob the Builder", "Ana"]
            .iter()
            .enumerate()
            .map(|(i, n)| Contact { id: format!("u{i}"), name: n.to_string() })
            .collect();
        m
    };
    let mut home = base();
    home.listening = true;
    home.fast_walk = true;
    home.heard = Some("put my hoodie on".into());
    out.push((ScreenPage::Home, home, Some(1), None));
    let mut search = base();
    search.press(Target::YouTube);
    search.type_text("lofi hi");
    search.suggestions_arrived(
        1,
        "lofi hi",
        vec!["lofi hip hop".into(), "lofi hip hop radio".into(), "lofi hip hop beats to study".into(), "lofi hits".into(), "lofi hindi".into()],
    );
    out.push((ScreenPage::SearchQwerty, search.clone(), Some(14), None));
    let mut azerty = search.clone();
    azerty.layout = KeyboardLayout::Azerty;
    out.push((ScreenPage::SearchAzerty, azerty, None, Some(3)));
    let mut symbols = search.clone();
    symbols.keys = Keys::Symbols;
    out.push((ScreenPage::SearchSymbols, symbols, None, None));
    let mut results = base();
    results.search_for("lofi hip hop");
    let titles = [
        ("lofi hip hop radio 📚 beats to relax/study to", "Lofi Girl", None, true),
        ("Best of lofi hip hop 2021 ✨ [beats to relax/study to]", "Lofi Girl", Some("6:10:58"), false),
        ("Chillhop Radio - jazzy & lofi hip hop beats 🐾", "Chillhop Music", None, true),
        ("1 A.M Study Session 📚 [lofi hip hop]", "Lofi Girl", Some("1:01:14"), false),
        ("Why the rush? - lo-fi beats for work/study / cat jazz", "chill chill journal", Some("3:31:19"), false),
    ];
    let videos = titles
        .iter()
        .enumerate()
        .map(|(i, (t, c, d, live))| Video {
            id: format!("sample{i:05}"),
            title: t.to_string(),
            channel: c.to_string(),
            duration: d.map(str::to_string),
            live: *live,
        })
        .collect();
    results.results_arrived(1, Results { videos, continuation: Some("next".into()) }, false);
    out.push((ScreenPage::Results, results, Some(9), None));
    let mut calls = base();
    calls.page = Page::Call;
    out.push((ScreenPage::CallList, calls.clone(), Some(4), None));
    let mut ringing = calls.clone();
    ringing.set_call(Some(Call::Ringing { name: "Lyra Nightfall".into() }));
    out.push((ScreenPage::CallRinging, ringing, Some(1), None));
    let mut active = calls;
    active.set_call(Some(Call::Connected { name: "Kasumi".into(), seconds: 83, muted: false }));
    out.push((ScreenPage::CallActive, active, None, None));
    let mut photo = base();
    photo.page = Page::Photo;
    photo.countdown = Some(3);
    out.push((ScreenPage::Photo, photo, Some(2), None));
    out
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out = Path::new(args.first().map(String::as_str).unwrap_or("orb-menu"));
    std::fs::create_dir_all(out)?;
    let pages = page_pictures();
    let path = out.join("Orb screen pages.png");
    write_png(&path, WIDTH, HEIGHT * SLICES as u32, &pages)?;
    println!("{}", path.display());
    let path = out.join("Orb screen layout.png");
    write_png(&path, TABLE_WIDTH as u32, SLICES as u32, &layout_table())?;
    println!("{}", path.display());

    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let thumbs: Vec<TextureHandle> = (0..PICTURES).map(|i| sample_thumbnail(canvas.ctx(), i)).collect();
    for (page, menu, hover, pressed) in samples() {
        assert_eq!(ScreenPage::of(&menu), page, "the sample shows its page");
        let picture =
            |id: &str| -> Option<TextureId> { id.strip_prefix("sample").and_then(|n| n.parse::<usize>().ok()).and_then(|i| thumbs.get(i)).map(|t| t.id()) };
        let rgba = paint(&mut canvas, |p| menu_ui::draw(p, pos2(0.0, 0.0), &View { menu: &menu, hover, pressed, picture: &picture, time: 0.4 }));
        let path = out.join(format!("menu-{}.png", page.name()));
        write_png(&path, WIDTH, HEIGHT, &unpremultiply(&rgba))?;
        println!("{}", path.display());
        let pictures = |id: &str| -> Option<screen::Picture> { id.strip_prefix("sample").and_then(|n| n.parse::<usize>().ok()).map(sample_picture) };
        let flags = screen::Flags { listening: menu.listening, loading: menu.loading(), thinking: false };
        let bank = screen::Frame::new(&menu, hover, pressed, flags, 72, &pictures).bank;
        std::fs::write(out.join(format!("bank-{}.bin", page.name())), &bank[..screen::bank_bytes(72)])?;
        if page == ScreenPage::Results {
            let bank = screen::Frame::new(&menu, hover, pressed, flags, 128, &pictures).bank;
            std::fs::write(out.join(format!("bank-{}-128.bin", page.name())), &bank[..screen::bank_bytes(128)])?;
        }
    }
    Ok(())
}
