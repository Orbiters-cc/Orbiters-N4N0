//! `orbiters-desktop --n4n0-menu <out> [--theme <theme.json>] [--strings <strings.json>]`: N4N0's menu pages as files
//! (contracts `Docs/n4n0.md`, Pages and layout; Menu themes and translations).
//!
//! - `N4N0 screen pages.png`: the picture of every page's unchanging part, one 1024 × 576 slice per page (slice 0 is
//!   the back; slice `n` is page `n`), top first: Toolkit imports it as the projected screen's page texture array.
//!   Without `--theme`, Orbiters' own look, and `N4N0 screen pages retro.png` the retro one; with `--theme`, only this
//!   file, in the theme's style and colours.
//! - `N4N0 screen layout.png`: where each page's changing parts go, a row per page, for the screen's shader (see
//!   [`layout_table`]; the same whatever the theme and language).
//! - Without `--theme`: `menu-<page>.png`: each page as the SteamVR overlay shows it with sample content, for reviewing
//!   the design; `bank-<page>.bin`: what the avatar's screen is sent for the same page (its 744 bytes, 72-byte
//!   thumbnails), to draw it with the screen's shader and compare; `bank-results-128.bin`, the results page with
//!   128-byte thumbnails (1024 bytes).
//!
//! `--strings` draws the pages' texts in another language (missing keys stay English); `orbiters-desktop
//! --n4n0-menu-strings <file>` writes the English base with each key's `max` ([`run_strings`]).

use super::menu_retro;
use super::menu_ui::{self, Skin, View};
use super::Canvas;
use anyhow::{anyhow, bail};
use egui::{pos2, ColorImage, TextureHandle, TextureId, TextureOptions};
use orbiters_core::settings::MenuLook;
use orbiters_n4n0_core::menu::layout::{self, ScreenPage, MAX_ELEMENTS, MAX_SLOTS, PICTURES};
use orbiters_n4n0_core::menu::strings::{Strings, Text};
use orbiters_n4n0_core::menu::theme::Theme;
use orbiters_n4n0_core::menu::{
    screen, Call, Card, Contact, InstanceRow, InviteAccount, InviteAnswer, KeyboardLayout, Keys, Light, Media, Menu, Page, Pages, PhotoLength, PhotoMode, StateRow, Target, Tracking,
    Volume, WorldVideo,
};
use orbiters_n4n0_core::picture;
use orbiters_n4n0_core::youtube::{Results, Video};
use std::path::{Path, PathBuf};

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

pub(super) fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// A straight RGBA picture at half size, each pixel the mean of four (colours weighted by their alpha).
fn half(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let (w, h) = (width / 2, height / 2);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((2 * y + dy) * width + 2 * x + dx) * 4;
                let a = rgba[i + 3] as u32;
                for c in 0..3 {
                    sum[c] += rgba[i + c] as u32 * a;
                }
                sum[3] += a;
            }
            let o = (y * w + x) * 4;
            for c in 0..3 {
                out[o + c] = sum[c].checked_div(sum[3]).unwrap_or(0) as u8;
            }
            out[o + 3] = (sum[3] / 4) as u8;
        }
    }
    out
}

/// Paints one frame twice (fonts and textures load on the first).
fn paint(canvas: &mut Canvas, draw: impl Fn(&egui::Painter)) -> Vec<u8> {
    let mut rgba = Vec::new();
    for _ in 0..2 {
        rgba = canvas.paint(1.0, |p| draw(p)).to_vec();
    }
    rgba
}

/// The page texture: every page's unchanging part, slice by slice, straight alpha, in Orbiters' own look or the retro
/// one (`retro`: the avatar's retro screen has these), in `skin`'s colours and texts.
pub fn page_pictures(retro: bool, skin: &Skin) -> Vec<u8> {
    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let slice = (WIDTH * HEIGHT * 4) as usize;
    let mut out = vec![0u8; slice * SLICES];
    // Slice 0: the back, what the screen shows from behind.
    let rgba = paint(&mut canvas, |p| if retro { menu_retro::draw_back(p, pos2(0.0, 0.0), skin) } else { menu_ui::draw_back(p, pos2(0.0, 0.0), skin) });
    out[..slice].copy_from_slice(&unpremultiply(&rgba));
    for page in ScreenPage::ALL {
        let l = layout::layout(page);
        let rgba =
            paint(
                &mut canvas,
                |p| if retro { menu_retro::draw_chrome(p, pos2(0.0, 0.0), &l, skin) } else { menu_ui::draw_chrome(p, pos2(0.0, 0.0), &l, skin) },
            );
        let at = page as usize * slice;
        out[at..at + slice].copy_from_slice(&unpremultiply(&rgba));
    }
    out
}

/// A stand-in thumbnail: bands of colour (160 × 90, RGBA), with a disc.
pub(crate) fn sample_rgba(seed: usize) -> (usize, usize, Vec<u8>) {
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

/// A stand-in for music art: a square, as most art is, whole on the menu's 16:9 card (as the media reading gives it).
pub(crate) fn sample_art() -> picture::Rgb {
    let size = 160usize;
    let mut pixels = Vec::with_capacity(size * size * 3);
    for y in 0..size {
        for x in 0..size {
            let ring = ((x as f32 - 80.0).powi(2) + (y as f32 - 80.0).powi(2)).sqrt();
            let k = if (ring - 44.0).abs() < 10.0 { 1.0 } else { 0.35 + 0.4 * (y as f32 / size as f32) };
            pixels.extend([(250.0 * k) as u8, (96.0 * k) as u8, (170.0 * k) as u8]);
        }
    }
    picture::letterbox(&picture::Rgb { width: size, height: size, pixels }, crate::media::ART_WIDTH, crate::media::ART_HEIGHT, [0x10, 0x10, 0x12])
}

/// The sample music art's key.
pub(crate) const SAMPLE_ART: &str = "art:sample";

/// Suggestions for "lofi hi".
pub(crate) fn sample_suggestions() -> Vec<String> {
    ["lofi hip hop", "lofi hip hop radio", "lofi hip hop beats to study", "lofi hits", "lofi hindi"].map(String::from).to_vec()
}

/// Results for "lofi hip hop", their thumbnails [`sample_rgba`] by their order (`sample00000`…).
pub(crate) fn sample_videos() -> Vec<Video> {
    let titles = [
        ("lofi hip hop radio 📚 beats to relax/study to", "Lofi Girl", None, true),
        ("Best of lofi hip hop 2021 ✨ [beats to relax/study to]", "Lofi Girl", Some("6:10:58"), false),
        ("Chillhop Radio - jazzy & lofi hip hop beats 🐾", "Chillhop Music", None, true),
        ("1 A.M Study Session 📚 [lofi hip hop]", "Lofi Girl", Some("1:01:14"), false),
        ("Why the rush? - lo-fi beats for work/study / cat jazz", "chill chill journal", Some("3:31:19"), false),
    ];
    titles
        .iter()
        .enumerate()
        .map(|(i, (t, c, d, live))| Video {
            id: format!("sample{i:05}"),
            title: t.to_string(),
            channel: c.to_string(),
            duration: d.map(str::to_string),
            live: *live,
        })
        .collect()
}

/// A song playing, with [`sample_art`].
pub(crate) fn sample_media() -> Media {
    Media {
        title: "Midnight City".into(),
        artist: "M83".into(),
        album: "Hurry Up, We're Dreaming".into(),
        app: "Spotify".into(),
        playing: true,
        can_play_pause: true,
        can_previous: true,
        can_next: true,
        can_seek: true,
        position: Some(83),
        duration: Some(243),
        art: Some(SAMPLE_ART.into()),
        video: None,
    }
}

/// Menus with sample content, one per page, with the element hovered and pressed.
pub(crate) fn samples() -> Vec<(ScreenPage, Menu, Option<usize>, Option<usize>)> {
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
    search.suggestions_arrived(1, "lofi hi", sample_suggestions());
    out.push((ScreenPage::SearchQwerty, search.clone(), Some(14), None));
    let mut azerty = search.clone();
    azerty.layout = KeyboardLayout::Azerty;
    out.push((ScreenPage::SearchAzerty, azerty, None, Some(3)));
    let mut symbols = search.clone();
    symbols.keys = Keys::Symbols;
    out.push((ScreenPage::SearchSymbols, symbols, None, None));
    let mut results = base();
    results.pages = Pages::ALL;
    results.can_drop = true;
    results.search_for("lofi hip hop");
    results.results_arrived(1, Results { videos: sample_videos(), continuation: Some("next".into()) }, false);
    // The second result's link copied: its row says so and offers Drop; a card is out (Take back).
    results.press(Target::Copy(1));
    results.card_out = true;
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
    // The overlay's Home and screens from version 3: six tiles, music playing, N4N0's light on.
    let more = || {
        let mut m = base();
        m.pages = Pages::Six;
        m.media = Some(sample_media());
        m.volume = Some(Volume { percent: 40, muted: false });
        m.light = Some(Light { on: true, color: 1, level: 0.6 });
        m
    };
    let mut home = more();
    home.fast_walk = true;
    // As in VRChat's desktop mode: fast walk greyed out.
    home.desktop_mode = true;
    home.heard = Some("play some music".into());
    out.push((ScreenPage::HomeSix, home, Some(4), None));
    // The music with its timeline (screens from version 9 and the overlay), a card out (Take back), a place over the
    // timeline hovered.
    let mut music = more();
    music.pages = Pages::ALL;
    music.card_out = true;
    music.page = Page::Music;
    let hovered = layout::cached(ScreenPage::Music).elements.iter().position(|e| e.target == Target::Seek(9));
    out.push((ScreenPage::Music, music, hovered, None));
    let mut light = more();
    light.page = Page::Light;
    out.push((ScreenPage::Light, light, Some(5), None));
    // The overlay's Home and screens from version 5: seven tiles, the avatar's saved states, what N4N0 says in its place.
    let seven = || {
        let mut m = more();
        m.pages = Pages::Seven;
        m.can_drop = true;
        m.set_states(sample_states(), true, "");
        m
    };
    let mut home = seven();
    home.fast_walk = true;
    home.heard = Some("Done, hoodie on. Now go find your friends instead of talking to a floating ball.".into());
    out.push((ScreenPage::HomeSeven, home, Some(6), None));
    let mut states = seven();
    states.press(Target::States);
    out.push((ScreenPage::States, states, Some(5), None));
    let mut card = seven();
    let video = &sample_videos()[3];
    card.show_card(Card { link: video.link(), title: video.title.clone(), video: Some(video.id.clone()), left: 11 });
    out.push((ScreenPage::Card, card, None, None));
    // The overlay's Home and screens from version 6: ten places (Tracking, Suggest features), the versions, New state,
    // the keyboard for names and ideas, Tracking and Suggest features.
    let ten = || {
        let mut m = seven();
        m.pages = Pages::Ten;
        m.tracking = Some(Tracking { on: true, levels: [0, 1, 2], note: "Smoothing your devices now".into() });
        m.versions = "N4N0 1.0.9 · App 0.4.0".into();
        m.world_video = Some(WorldVideo { url: sample_videos()[0].link(), title: Some(sample_videos()[0].title.clone()), at: None });
        m
    };
    let mut home = ten();
    home.fast_walk = true;
    home.heard = Some("Done, hoodie on. Now go find your friends instead of talking to a floating ball.".into());
    out.push((ScreenPage::HomeTen, home, Some(8), None));
    let mut type_qwerty = ten();
    type_qwerty.press(Target::Suggest);
    type_qwerty.press(Target::SuggestType);
    type_qwerty.type_text("A dance mode where N4N0 bounces to the music the world plays");
    out.push((ScreenPage::TypeQwerty, type_qwerty.clone(), Some(14), None));
    let mut type_azerty = type_qwerty.clone();
    type_azerty.layout = KeyboardLayout::Azerty;
    out.push((ScreenPage::TypeAzerty, type_azerty, None, Some(5)));
    let mut type_symbols = type_qwerty;
    type_symbols.keys = Keys::Symbols;
    out.push((ScreenPage::TypeSymbols, type_symbols, None, None));
    let mut new_state = ten();
    new_state.press(Target::States);
    new_state.press(Target::StateSave);
    new_state.draft.name = "Party outfit".into();
    new_state.draft.default = true;
    out.push((ScreenPage::NewState, new_state, Some(5), None));
    let mut tracking = ten();
    tracking.press(Target::Tracking);
    out.push((ScreenPage::Tracking, tracking, Some(7), None));
    let mut suggest = ten();
    suggest.press(Target::Suggest);
    suggest.suggestion = "A dance mode where N4N0 bounces to the music the world plays, with a switch on its card".into();
    out.push((ScreenPage::Suggest, suggest, Some(4), None));
    // The overlay's Home and screens from version 10: the simple Home with music playing and without, More, Previous
    // instances, Add Orbiters as a friend.
    let simple = || {
        let mut m = ten();
        m.pages = Pages::Simple;
        m.set_instances(sample_instances(), true, "");
        m
    };
    let mut home = simple();
    home.fast_walk = true;
    home.heard = Some("Done, hoodie on. Now go find your friends.".into());
    // A screen of versions 10 to 15 keeps its Home; from version 16 it has Stay here.
    let mut stay = home.clone();
    stay.on_screen = true;
    stay.pages = Pages::ALL;
    home.pages = Pages::TwoPictures;
    let hovered_stay = layout::cached(ScreenPage::HomeSimpleStay).elements.iter().position(|e| e.target == Target::StayHere);
    out.push((ScreenPage::HomeSimpleStay, stay, hovered_stay, None));
    out.push((ScreenPage::HomeSimple, home, Some(1), None));
    let mut quiet = simple();
    quiet.media = None;
    quiet.listening = true;
    let mut pinned = quiet.clone();
    pinned.on_screen = true;
    pinned.pages = Pages::ALL;
    pinned.pinned = true;
    quiet.pages = Pages::TwoPictures;
    out.push((ScreenPage::HomeQuietStay, pinned, Some(0), None));
    out.push((ScreenPage::HomeQuiet, quiet, Some(0), None));
    let mut more = simple();
    more.press(Target::More);
    out.push((ScreenPage::More, more, Some(2), None));
    let mut instances = simple();
    instances.page = Page::Instances;
    out.push((ScreenPage::Instances, instances, Some(7), None));
    let mut friend = simple();
    friend.page = Page::Instances;
    friend.press(Target::InviteMe(0));
    let account = InviteAccount { name: "Orbiters".into(), link: "https://vrchat.com/home/user/usr_0c5b8f2e-3a41-4d7c-9e2b-5f6a7b8c9d0e".into() };
    friend.invite_answered(InviteAnswer::NeedsFriend { account, message: "Add Orbiters' VRChat account as a friend on VRChat first, then try again.".into() });
    out.push((ScreenPage::AddFriend, friend, Some(3), None));
    // Screens from version 11: More with the QR code scanner, and the scanner having opened a link.
    let qr = || {
        let mut m = simple();
        m.pages = Pages::Qr;
        m
    };
    let mut more = qr();
    more.press(Target::More);
    out.push((ScreenPage::MoreQr, more, Some(4), None));
    let mut scanner = qr();
    scanner.page = Page::Qr;
    scanner.qr = sample_qr();
    out.push((ScreenPage::Qr, scanner, Some(3), None));
    // Screens from version 12 (and the overlay): the last capture ready to share, History, Share to … with two friends
    // chosen, the warning before an explicit capture goes public.
    let share = || {
        let mut m = simple();
        m.pages = Pages::ALL;
        m.set_captures(sample_captures());
        m
    };
    let hovered = |page: ScreenPage, target: Target| layout::cached(page).elements.iter().position(|e| e.target == target);
    let mut photo = share();
    photo.press(Target::Photo);
    // A screen of versions 12 to 14 keeps its one Take and the line that changes it.
    let mut older = photo.clone();
    older.pages = Pages::Mirror;
    out.push((ScreenPage::PhotoShare, older, hovered(ScreenPage::PhotoShare, Target::ShareLatest), None));
    out.push((ScreenPage::PhotoTwo, photo, hovered(ScreenPage::PhotoTwo, Target::TakeOf(PhotoMode::World)), None));
    let mut history = share();
    history.press(Target::Photo);
    history.press(Target::History);
    out.push((ScreenPage::History, history.clone(), hovered(ScreenPage::History, Target::ShareTo(0)), None));
    let mut friends = history.clone();
    friends.press(Target::ShareTo(0));
    friends.set_friends(orbiters_n4n0_core::captures::Friends::ready(sample_friends()));
    friends.press(Target::Friend(1));
    friends.press(Target::Friend(3));
    out.push((ScreenPage::ShareTo, friends, hovered(ScreenPage::ShareTo, Target::SendShare), None));
    let mut explicit = history;
    explicit.press(Target::SharePublic(2));
    out.push((ScreenPage::Explicit, explicit, hovered(ScreenPage::Explicit, Target::CancelShare), None));
    // Screens from version 13 (and the overlay): a world's feedback with the player's like and labels, its labels page,
    // its comments, More with World feedback.
    let feedback = || {
        let mut m = share();
        m.pages = Pages::Feedback;
        m.set_feedback(sample_feedback());
        m
    };
    let mut more = feedback();
    more.press(Target::More);
    out.push((ScreenPage::MoreFeedback, more, hovered(ScreenPage::MoreFeedback, Target::WorldFeedback), None));
    let mut world = feedback();
    world.press(Target::More);
    world.press(Target::WorldFeedback);
    out.push((ScreenPage::Feedback, world.clone(), hovered(ScreenPage::Feedback, Target::WriteComment), None));
    let mut labels = world.clone();
    labels.press(Target::Labels);
    out.push((ScreenPage::FeedbackLabels, labels, hovered(ScreenPage::FeedbackLabels, Target::LabelChip(7)), None));
    let mut comments = world;
    comments.press(Target::Comments);
    out.push((ScreenPage::Comments, comments, hovered(ScreenPage::Comments, Target::DeleteComment(0)), None));
    // Screens from version 14 (and the overlay): More with the screen mirror, and the mirror's page with a camera placed
    // at a zoom (its picture's place as everyone but its owner sees it).
    let mirror = || {
        use orbiters_n4n0_core::mirror::{MirrorView, Phase, Reach};
        let mut m = share();
        m.set_feedback(sample_feedback());
        m.mirror = MirrorView { phase: Phase::Placed, zoom: 4, reach: Reach::Screen };
        m
    };
    let mut more = mirror();
    more.press(Target::More);
    out.push((ScreenPage::MoreMirror, more, hovered(ScreenPage::MoreMirror, Target::Mirror), None));
    let mut page = mirror();
    page.press(Target::More);
    page.press(Target::Mirror);
    out.push((ScreenPage::Mirror, page, hovered(ScreenPage::Mirror, Target::MirrorZoomIn), None));
    out
}

/// The sample player's world feedback: the world they are in (liked, two labels of theirs, comments of theirs and
/// others'), and two worlds to step to.
pub(crate) fn sample_feedback() -> orbiters_n4n0_core::feedback::FeedbackView {
    use orbiters_n4n0_core::feedback::{CommentRow, FeedbackView, LabelCount, Subject, WorldFeedback, Why};
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let here = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
    let label = |id: &str, count: u32, mine: bool| LabelCount { id: id.into(), count, mine };
    let comment = |id: u64, by: &str, text: &str, ago: u64, mine: bool| CommentRow { id, by: by.into(), text: text.into(), at: now - ago, mine };
    let world = WorldFeedback {
        world: here.into(),
        likes: 41,
        liked: true,
        labels: vec![label("meetup", 12, true), label("mirror", 9, false), label("photo-spot", 6, true), label("bright", 4, false), label("pens", 3, false), label("video-player", 2, false), label("chill", 1, false)],
        comments: vec![
            comment(381, "Enzo", "The mirror room upstairs is perfect for photos, and the lighting at sunset is unreal.", 7200, true),
            comment(377, "Kasumi", "Great place for meetups! The video player works with every link we tried.", 26 * 3600, false),
            comment(360, "Lyra Nightfall", "Cozy. Wish the pens were near the spawn.", 3 * 86_400, false),
            comment(352, "Bob the Builder", "We held our build club here, everyone loved the rooftop.", 5 * 86_400, false),
        ],
        comment_count: 7,
        ..Default::default()
    };
    FeedbackView {
        signed_in: true,
        subjects: vec![
            Subject { world: here.into(), name: "The Black Cat".into(), why: Why::Here },
            Subject { world: "wrld_9936b6ff-e523-4066-9e40-a333320ff5d3".into(), name: "Japan Shrine".into(), why: Why::Capture },
            Subject { world: "wrld_a7e4d38f-8fac-47ab-9e30-628bf015e62b".into(), name: "Palm Point - Night".into(), why: Why::Instance { at: now - 26 * 3600 } },
        ],
        worlds: vec![world],
    }
}

/// The sample player's 3D captures: the last one ready to share, History's in every state (one flagged explicit), each
/// picture a sample thumbnail ([`sample_rgba`] by the number in its key).
pub(crate) fn sample_captures() -> orbiters_n4n0_core::captures::CapturesView {
    use orbiters_n4n0_core::captures::{CapturePhase, CaptureRow, CapturesView, Sharing};
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let row = |n: usize, world: &str, ago: u64| CaptureRow {
        key: format!("sample{n}"),
        world: world.into(),
        taken: now - ago,
        phase: CapturePhase::Ready,
        ..Default::default()
    };
    let latest = row(1, "The Black Cat", 90);
    let mut making = row(2, "Japan Shrine", 1800);
    making.phase = CapturePhase::Making { percent: 62 };
    making.sharing = Sharing::Waiting { public: true, people: 0 };
    let mut flagged = row(3, "Club Orion", 3 * 3600);
    flagged.explicit = true;
    flagged.explicit_reasons = vec!["osc-parameter:NSFW/Toggle".into(), "avatar-label:sexually_explicit".into()];
    let mut public = row(4, "Movie Night", 2 * 86_400);
    public.public = true;
    public.people = 2;
    let mut friends = row(0, "Midnight Rooftops", 5 * 86_400);
    friends.sharing = Sharing::Done { public: false, people: 3 };
    let history = vec![latest.clone(), making, flagged, public, friends, row(2, "Japan Shrine", 9 * 86_400), row(3, "The Great Pug", 12 * 86_400)];
    CapturesView { latest: Some(latest), history, listed: true, note: String::new() }
}

/// The sample player's VRChat friends.
pub(crate) fn sample_friends() -> Vec<orbiters_n4n0_core::captures::Friend> {
    ["Kasumi", "Lyra Nightfall", "Bob the Builder", "Ana", "Mika", "Zephyr", "Juniper", "Orion"]
        .iter()
        .enumerate()
        .map(|(i, n)| orbiters_n4n0_core::captures::Friend { id: format!("usr_{i:04}"), name: n.to_string() })
        .collect()
}

/// The QR code scanner having opened a link, looking for more.
pub(crate) fn sample_qr() -> orbiters_n4n0_core::qr::QrView {
    orbiters_n4n0_core::qr::QrView {
        phase: orbiters_n4n0_core::qr::QrPhase::Looking,
        note: String::new(),
        link: Some("https://orbiters.cc/maps/wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b".into()),
        opened: 1,
    }
}

/// Instances the sample player was in, the newest first.
pub(crate) fn sample_instances() -> Vec<InstanceRow> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    [
        ("wrld_7281543e-6888-4676-896a-fdd090552fc4:41822~hidden(usr_a)~region(eu)", "Exoplanet Journey", 300),
        ("wrld_9936b6ff-e523-4066-9e40-a333320ff5d3:12093~region(jp)", "New Generation", 2 * 3600),
        ("wrld_a7e4d38f-8fac-47ab-9e30-628bf015e62b:77001~private(usr_a)~canRequestInvite~region(us)", "Palm Point - Night", 26 * 3600),
        ("wrld_266523e8-9161-40da-acd0-6bd82e075833:5512~group(grp_x)~groupAccessType(plus)~region(use)", "Popcorn Palace", 3 * 86400),
        ("wrld_99123966-6b37-46d9-835c-25c2f6e0706c:9040~friends(usr_a)~region(eu)", "VRC Volleyball", 9 * 86400),
        ("wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:1", "VRChat Home", 12 * 86400),
    ]
    .into_iter()
    .map(|(location, world, ago)| InstanceRow { location: location.into(), world: world.into(), at: now - ago })
    .collect()
}

/// Saved states of the sample avatar: the second the default.
pub(crate) fn sample_states() -> Vec<StateRow> {
    [
        ("s1", "Hoodie on", false, false, false),
        ("s2", "Cosy evening", true, true, false),
        ("s3", "Tall and sitting", false, true, true),
        ("s4", "State 4", false, false, false),
    ]
    .into_iter()
    .map(|(id, name, default, pose, size)| StateRow { id: id.into(), name: name.into(), default, pose, size })
    .collect()
}

/// What `--n4n0-menu` was asked for.
struct Options {
    out: PathBuf,
    theme: Option<Theme>,
    strings: Strings,
}

/// Reads `<out> [--theme <file>] [--strings <file>]` (in any order), and the files, telling what they leave out.
fn options(args: &[String]) -> anyhow::Result<Options> {
    let (mut out, mut theme, mut strings) = (None, None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--theme" | "--strings" => {
                let path = args.next().ok_or_else(|| anyhow!("{arg} needs a file: {arg} <file.json>"))?; // i18n: not shown (a documented command line for theme authors, in English like its usage text)
                if arg == "--theme" {
                    let (t, warnings) = Theme::load(Path::new(path)).map_err(|e| anyhow!("--theme: {e}"))?;
                    for w in warnings {
                        eprintln!("warning: {path}: {w}");
                    }
                    theme = Some(t);
                } else {
                    let parsed = Strings::load(Path::new(path)).map_err(|e| anyhow!("--strings: {e}"))?;
                    for w in parsed.warnings.iter().cloned().chain(too_long(&parsed.strings)) {
                        eprintln!("warning: {path}: {w}");
                    }
                    strings = Some(parsed.strings);
                }
            }
            flag if flag.starts_with("--") => bail!("unknown option {flag} (--n4n0-menu <out> [--theme <theme.json>] [--strings <strings.json>])"), // i18n: not shown (a documented command line for theme authors, in English like its usage text)
            path if out.is_none() => out = Some(PathBuf::from(path)),
            extra => bail!("one output folder only ({extra:?} is a second)"), // i18n: not shown (a documented command line for theme authors, in English like its usage text)
        }
    }
    Ok(Options { out: out.unwrap_or_else(|| PathBuf::from("n4n0-menu")), theme, strings: strings.unwrap_or_else(|| Strings::english().clone()) })
}

/// Each key's `max`: the characters that fit its places ([`menu_ui::max_length`]).
pub fn max_lengths() -> Vec<(Text, usize)> {
    let places = menu_ui::all_places();
    Text::ALL.iter().map(|&t| (t, menu_ui::max_length(t, &places))).collect()
}

/// The texts of `strings` longer than their key's `max` (they are drawn smaller to fit).
fn too_long(strings: &Strings) -> Vec<String> {
    max_lengths()
        .into_iter()
        .filter_map(|(t, max)| {
            let n = strings.given(t)?.chars().count();
            (n > max).then(|| format!("{:?} has {n} characters, more than its {max}: it is drawn smaller to fit", t.key())) // i18n: not shown (a warning for whoever writes a translation file)
        })
        .collect()
}

/// The avatar's screen's colours for `theme` (`N4N0 screen colors.json`), by the screen shader's property names: what it
/// draws the changing parts with (texts, a hovered or chosen element, a picture's place, the retro look's dim lines and
/// hot colour), as the page pictures were drawn, and the theme's own `screen` colours (the projected plate and edge)
/// passed through, with its glow. The base themes give the shader's own defaults (Orbiters' look) and Toolkit's retro
/// colours.
pub fn screen_colors(theme: &Theme) -> String {
    let skin = Skin::new(theme, Strings::english().clone());
    let retro = theme.style == MenuLook::Retro;
    let hex = |c: egui::Color32| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
    let hexa = |c: [u8; 4]| format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3]);
    let (pal, rp) = (&skin.palette, &skin.retro);
    let colours = [
        ("_Card", hex(if retro { rp.bg } else { pal.card })),
        ("_Text", hex(if retro { rp.text } else { pal.text })),
        ("_Text2", hex(if retro { rp.text2 } else { pal.muted })),
        ("_Accent", hex(if retro { rp.line } else { pal.accent })),
        ("_Placeholder", hex(if retro { rp.panel } else { pal.raised })),
        ("_Dim", hex(rp.dim)),
        ("_Hot", hex(rp.hot)),
        ("_Plate", hexa(theme.screen.tint)),
        ("_Edge", hexa(theme.screen.edge)),
    ];
    let mut out = format!("{{\n  \"schema\": 1,\n  \"style\": \"{}\",\n  \"shader\": {{\n", if retro { "retro" } else { "modern" });
    for (i, (name, value)) in colours.iter().enumerate() {
        out += &format!("    \"{name}\": \"{value}\"{}\n", if i + 1 < colours.len() { "," } else { "" });
    }
    out += &format!("  }},\n  \"glow\": {}\n}}\n", theme.screen.glow); // i18n: not shown (a JSON file)
    out
}

/// Draws with `draw`, then tells on the error stream which texts were drawn smaller to fit their places.
fn noting_shrunk<T>(draw: impl FnOnce() -> T) -> T {
    menu_ui::watch_shrinking();
    let out = draw();
    for (text, scale) in menu_ui::shrunk() {
        eprintln!("note: {text:?} is drawn at {:.0} % of its size to fit its place", scale * 100.0);
    }
    out
}

/// The English base strings file, with each key's `max`: what `--n4n0-menu-strings` writes.
pub fn base_strings() -> String {
    let max = max_lengths();
    Strings::english().to_json(Some(&|t: Text| max.iter().find(|(k, _)| *k == t).map_or(0, |(_, m)| *m)))
}

/// `orbiters-desktop --n4n0-menu-strings <file>`: the English base strings file, with each key's `max`.
pub fn run_strings(args: &[String]) -> anyhow::Result<()> {
    let path = PathBuf::from(args.first().ok_or_else(|| anyhow!("--n4n0-menu-strings needs a file to write: --n4n0-menu-strings <file.json>"))?);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, base_strings())?;
    println!("{}", path.display());
    Ok(())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let Options { out, theme, strings } = options(args)?;
    let out = out.as_path();
    std::fs::create_dir_all(out)?;
    // Japanese (or Chinese, Korean) texts in their own language's font, whatever this computer's locale.
    orbiters_ui::fonts::prefer_language(strings.language());
    if let Some(theme) = theme {
        // A theme's pages only, under the one name the avatar's screen is given, and the layout.
        let skin = Skin::new(&theme, strings);
        let path = out.join("N4N0 screen pages.png");
        write_png(&path, WIDTH, HEIGHT * SLICES as u32, &noting_shrunk(|| page_pictures(theme.style == MenuLook::Retro, &skin)))?;
        println!("{}", path.display());
        let path = out.join("N4N0 screen layout.png");
        write_png(&path, TABLE_WIDTH as u32, SLICES as u32, &layout_table())?;
        println!("{}", path.display());
        let path = out.join("N4N0 screen colors.json");
        std::fs::write(&path, screen_colors(&theme))?;
        println!("{}", path.display());
        return Ok(());
    }
    let skin = &Skin::standard(strings);
    for (retro, name) in [(false, "N4N0 screen pages.png"), (true, "N4N0 screen pages retro.png")] { // i18n: not shown (file names)
        let path = out.join(name);
        write_png(&path, WIDTH, HEIGHT * SLICES as u32, &noting_shrunk(|| page_pictures(retro, skin)))?;
        println!("{}", path.display());
    }
    let path = out.join("N4N0 screen layout.png");
    write_png(&path, TABLE_WIDTH as u32, SLICES as u32, &layout_table())?;
    println!("{}", path.display());

    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let thumbs: Vec<TextureHandle> = (0..PICTURES).map(|i| sample_thumbnail(canvas.ctx(), i)).collect();
    // The same pictures as the retro look's green dots.
    let dots: Vec<TextureHandle> = (0..PICTURES)
        .map(|i| {
            let (w, h, rgba) = sample_rgba(i);
            let image = menu_retro::dots(&ColorImage::from_rgba_unmultiplied([w, h], &rgba), 1);
            canvas.ctx().load_texture(format!("dots{i}"), image, TextureOptions::NEAREST)
        })
        .collect();
    let art = sample_art();
    let art_image = {
        let rgba: Vec<u8> = art.pixels.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 255]).collect();
        ColorImage::from_rgba_unmultiplied([art.width, art.height], &rgba)
    };
    let art_dots = canvas.ctx().load_texture("art-dots", menu_retro::dots(&art_image, 1), TextureOptions::NEAREST);
    let art_texture = canvas.ctx().load_texture(SAMPLE_ART, art_image, TextureOptions::LINEAR);
    for (retro, name) in [(false, "menu-back.png"), (true, "menu-back-retro.png")] {
        let rgba = paint(&mut canvas, |p| if retro { menu_retro::draw_back(p, pos2(0.0, 0.0), skin) } else { menu_ui::draw_back(p, pos2(0.0, 0.0), skin) });
        let path = out.join(name);
        write_png(&path, WIDTH, HEIGHT, &unpremultiply(&rgba))?;
        println!("{}", path.display());
    }
    for (page, menu, hover, pressed) in samples() {
        assert_eq!(ScreenPage::of(&menu), page, "the sample shows its page");
        let picture = |id: &str| -> Option<TextureId> {
            if id == SAMPLE_ART {
                return Some(art_texture.id());
            }
            let id = id.strip_prefix(orbiters_n4n0_core::captures::PICTURE_PREFIX).unwrap_or(id);
            id.strip_prefix("sample").and_then(|n| n.parse::<usize>().ok()).and_then(|i| thumbs.get(i)).map(|t| t.id())
        };
        let modern = paint(&mut canvas, |p| menu_ui::draw(p, pos2(0.0, 0.0), &View { menu: &menu, hover, pressed, picture: &picture, time: 0.4, skin }));
        let path = out.join(format!("menu-{}.png", page.name()));
        write_png(&path, WIDTH, HEIGHT, &unpremultiply(&modern))?;
        println!("{}", path.display());
        let dotted = |id: &str| -> Option<TextureId> {
            if id == SAMPLE_ART {
                return Some(art_dots.id());
            }
            let id = id.strip_prefix(orbiters_n4n0_core::captures::PICTURE_PREFIX).unwrap_or(id);
            id.strip_prefix("sample").and_then(|n| n.parse::<usize>().ok()).and_then(|i| dots.get(i)).map(|t| t.id())
        };
        let rgba = paint(&mut canvas, |p| menu_retro::draw(p, pos2(0.0, 0.0), &View { menu: &menu, hover, pressed, picture: &dotted, time: 0.4, skin }));
        let path = out.join(format!("menu-{}-retro.png", page.name()));
        write_png(&path, WIDTH, HEIGHT, &unpremultiply(&rgba))?;
        println!("{}", path.display());
        // Orbiters Toolkit's pictures of the two looks, for the Look option on N4N0's card in My Avatar: the newest
        // Home, half size.
        if page == ScreenPage::HomeTen {
            for (picture, name) in [(&modern, "N4N0 preview.png"), (&rgba, "N4N0 preview retro.png")] { // i18n: not shown (file names)
                let path = out.join(name);
                write_png(&path, WIDTH / 2, HEIGHT / 2, &half(&unpremultiply(picture), WIDTH as usize, HEIGHT as usize))?;
                println!("{}", path.display());
            }
        }
        let pictures = |id: &str| -> Option<screen::Picture> {
            if id == SAMPLE_ART {
                return Some(screen::picture_from_rgb(&art));
            }
            let id = id.strip_prefix(orbiters_n4n0_core::captures::PICTURE_PREFIX).unwrap_or(id);
            id.strip_prefix("sample").and_then(|n| n.parse::<usize>().ok()).map(sample_picture)
        };
        let flags = screen::Flags { listening: menu.listening, loading: menu.loading(), ..Default::default() };
        let bank = screen::Frame::new(&menu, hover, pressed, flags, 72, &pictures).bank;
        std::fs::write(out.join(format!("bank-{}.bin", page.name())), &bank[..screen::bank_bytes(72)])?;
        if matches!(page, ScreenPage::Results | ScreenPage::Music | ScreenPage::Card | ScreenPage::HomeSimple | ScreenPage::History | ScreenPage::PhotoShare | ScreenPage::PhotoTwo) {
            let bank = screen::Frame::new(&menu, hover, pressed, flags, 128, &pictures).bank;
            std::fs::write(out.join(format!("bank-{}-128.bin", page.name())), &bank[..screen::bank_bytes(128)])?;
        }
    }
    // The two pictures as buttons while a timer counts: greyed out, for review and the screen's shader.
    {
        let mut menu = samples().into_iter().find(|s| s.0 == ScreenPage::PhotoTwo).map(|s| s.1).expect("the 3D picture sample");
        menu.countdown = Some(3);
        let none = |_: &str| -> Option<TextureId> { None };
        let rgba = paint(&mut canvas, |p| menu_ui::draw(p, pos2(0.0, 0.0), &View { menu: &menu, hover: None, pressed: None, picture: &none, time: 0.4, skin }));
        write_png(&out.join("menu-photo-two-countdown.png"), WIDTH, HEIGHT, &unpremultiply(&rgba))?;
        let flags = screen::Flags { listening: menu.listening, loading: menu.loading(), ..Default::default() };
        let none = |_: &str| -> Option<screen::Picture> { None };
        let bank = screen::Frame::new(&menu, None, None, flags, 72, &none).bank;
        std::fs::write(out.join("bank-photo-two-countdown.bin"), &bank[..screen::bank_bytes(72)])?;
    }
    // The overlay's world feedback popping (the screen keeps its picture): a like and a label just given, a few frames
    // in, for review only.
    for (target, ms) in [(Target::Like, 90u64), (Target::Like, 200), (Target::Like, 420), (Target::TopLabel(3), 120)] {
        let mut menu = samples().into_iter().find(|s| s.0 == ScreenPage::Feedback).map(|s| s.1).expect("the feedback sample");
        menu.popped = Some((target, std::time::Instant::now() - std::time::Duration::from_millis(ms)));
        let none = |_: &str| -> Option<TextureId> { None };
        let rgba = paint(&mut canvas, |p| menu_ui::draw(p, pos2(0.0, 0.0), &View { menu: &menu, hover: None, pressed: None, picture: &none, time: 0.4, skin }));
        let name = if target == Target::Like { format!("menu-feedback-like-{ms}ms.png") } else { format!("menu-feedback-label-{ms}ms.png") };
        write_png(&out.join(&name), WIDTH, HEIGHT, &unpremultiply(&rgba))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::menu_retro::{self, RetroPalette};
    use crate::overlay::menu_ui::Palette;
    use orbiters_ui::theme;

    /// A base strings file beside `orbiters-n4n0-core`.
    fn base(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../orbiters-n4n0-core/menu-strings").join(format!("{name}.json"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\r\n", "\n")
    }

    const LANGUAGES: [&str; 5] = ["en", "fr", "es", "pt", "ja"];

    #[test]
    fn the_base_themes_draw_with_todays_colours() {
        let p = Palette::new(&Theme::MODERN.colors);
        let expected = [
            (p.card, theme::INK_900),
            (p.surface, theme::INK_800),
            (p.raised, theme::INK_750),
            (p.raised2, theme::INK_700),
            (p.text, theme::FG),
            (p.text2, theme::FG_2),
            (p.muted, theme::FG_3),
            (p.faint, theme::FG_4),
            (p.accent, theme::ACCENT),
            (p.on_accent, theme::ON_ACCENT),
            (p.danger, theme::DANGER),
            (p.green, theme::ACCENT),
            (p.red, theme::REC),
            (p.blue, theme::BLUE),
            (p.violet, theme::VIOLET),
            (p.cyan, theme::CYAN),
            (p.amber, theme::WARN),
            (p.pink, menu_ui::PINK),
            (p.indigo, theme::INDIGO),
            (p.purple, theme::PURPLE),
        ];
        for (i, (got, want)) in expected.into_iter().enumerate() {
            assert_eq!(got, want, "colour {i}");
        }
        assert_eq!(RetroPalette::new(&Theme::RETRO.colors), menu_retro::BASE);
        assert_eq!(menu_ui::CARD, p.card);
    }

    #[test]
    fn a_theme_of_its_own_derives_sensible_steps() {
        // A light theme: keys a step darker than its surface, ink on a dark accent light.
        let mut colors = Theme::MODERN.colors;
        colors.background = [0xf4, 0xf4, 0xf6];
        colors.surface = [0xff, 0xff, 0xff];
        colors.text = [0x10, 0x10, 0x12];
        colors.muted = [0x60, 0x60, 0x68];
        colors.accent = [0x1a, 0x3c, 0xa0];
        let p = Palette::new(&colors);
        assert!(p.raised.r() < 0xff && p.raised2.r() < p.raised.r(), "{:?} {:?}", p.raised, p.raised2);
        assert!(p.text2.r() > 0x10 && p.text2.r() < 0x60, "{:?}", p.text2);
        assert!(p.on_accent.r() > 0xc0, "{:?}", p.on_accent);
        let retro = RetroPalette::new(&colors);
        let (lo, hi) = (colors.background[2].min(colors.accent[2]), colors.background[2].max(colors.accent[2]));
        assert!(retro.dim.b() > lo && retro.dim.b() < hi, "{:?}", retro.dim);
    }

    /// The base themes give the screen shader's own colours: its defaults for Orbiters' look, Orbiters Toolkit's retro
    /// colours for the retro one.
    #[test]
    fn screen_colours_of_the_base_themes_are_the_shaders() {
        let modern = screen_colors(&Theme::MODERN);
        for (name, value) in [("_Card", "#101012"), ("_Text", "#ECEDEE"), ("_Text2", "#8B8B94"), ("_Accent", "#00DA6D"), ("_Placeholder", "#27272A")] {
            assert!(modern.contains(&format!("\"{name}\": \"{value}\"")), "{name} {value}: {modern}");
        }
        let retro = screen_colors(&Theme::RETRO);
        for (name, value) in [
            ("_Card", "#020A04"),
            ("_Text", "#7DFFA6"),
            ("_Text2", "#2FB85E"),
            ("_Accent", "#2BE36A"),
            ("_Placeholder", "#05180B"),
            ("_Dim", "#186232"),
            ("_Hot", "#C8FFD7"),
            ("_Plate", "#020805D1"),
            ("_Edge", "#00DA6DFF"),
        ] {
            assert!(retro.contains(&format!("\"{name}\": \"{value}\"")), "{name} {value}: {retro}");
        }
        let parsed: serde_json::Value = serde_json::from_str(&retro).unwrap();
        assert_eq!(parsed["style"], "retro");
        assert_eq!(parsed["glow"], 1.0);
    }

    #[test]
    fn the_english_base_file_is_what_the_layout_gives() {
        assert_eq!(base("en"), base_strings(), "write it again: orbiters-desktop --n4n0-menu-strings crates/orbiters-n4n0-core/menu-strings/en.json");
    }

    #[test]
    fn every_key_has_a_place_and_a_max() {
        let places = menu_ui::all_places();
        for t in Text::ALL {
            assert!(places.iter().any(|(k, _, _)| k == t), "{} is drawn nowhere", t.key());
            assert!(menu_ui::max_length(*t, &places) >= t.en().chars().count());
        }
    }

    #[test]
    fn the_base_translations_keep_within_their_max() {
        for name in LANGUAGES {
            let parsed = Strings::parse(&base(name)).unwrap();
            assert!(too_long(&parsed.strings).is_empty(), "{name}: {:?}", too_long(&parsed.strings));
        }
    }

    /// Every page and the back in both looks: English is never drawn smaller (the pictures stay as they were); the base
    /// translations are, a little, where they are longer than their place (`cargo test -p orbiters-desktop texts_fit --
    /// --nocapture` lists them).
    #[test]
    fn texts_fit_their_places() {
        for name in LANGUAGES {
            let strings = Strings::parse(&base(name)).unwrap().strings;
            orbiters_ui::fonts::prefer_language(strings.language());
            let skin = Skin::standard(strings);
            menu_ui::watch_shrinking();
            for retro in [false, true] {
                page_pictures(retro, &skin);
            }
            let shrunk = menu_ui::shrunk();
            if name == "en" {
                assert!(shrunk.is_empty(), "English drawn smaller: {shrunk:?}");
            }
            for (text, scale) in &shrunk {
                println!("{name}: {text:?} at {:.0} %", scale * 100.0);
                assert!(*scale >= 0.75, "{name}: {text:?} shrinks to {:.0} %", scale * 100.0);
            }
        }
        orbiters_ui::fonts::prefer_language("en");
    }

    #[test]
    fn options_read_their_files_and_refuse_what_is_wrong() {
        let dir = std::env::temp_dir().join(format!("orbiters-n4n0-core-menu-options-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let theme = dir.join("theme.json");
        std::fs::write(&theme, r##"{ "schema": 1, "style": "retro", "colors": { "accent": "#FFC247" } }"##).unwrap();
        let strings = dir.join("strings.json");
        std::fs::write(&strings, base("fr")).unwrap();
        let bad = dir.join("bad.json");
        std::fs::write(&bad, r#"{ "schema": 1, "style": "neon" }"#).unwrap();
        let arg = |p: &Path| p.display().to_string();
        let o = options(&["out".into(), "--strings".into(), arg(&strings), "--theme".into(), arg(&theme)]).unwrap();
        assert_eq!(o.out, PathBuf::from("out"));
        let t = o.theme.unwrap();
        assert_eq!((t.style, t.colors.accent, t.colors.background), (MenuLook::Retro, [0xff, 0xc2, 0x47], Theme::RETRO.colors.background));
        assert_eq!(o.strings.language(), "fr");
        let o = options(&[]).unwrap();
        assert!(o.theme.is_none() && o.strings.language() == "en" && o.out == Path::new("n4n0-menu"));
        for (args, says) in [
            (vec!["out".to_string(), "--theme".into()], "needs a file"),
            (vec!["a".into(), "b".into()], "one output folder"),
            (vec!["--colour".into(), "x".into()], "unknown option"),
            (vec!["--theme".into(), arg(&bad)], "style"),
            (vec!["--strings".into(), arg(&dir.join("none.json"))], "none.json"),
        ] {
            let error = options(&args).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(error.contains(says), "{args:?}: {error}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
