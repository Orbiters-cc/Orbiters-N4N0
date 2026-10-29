//! Dragging files out of the window into any app (Explorer, the desktop, Discord, a browser, Photoshop) through Windows'
//! own drag and drop, as if they came from Explorer: the shell's data object for the files (CF_HDROP, the Shell IDList
//! Array and file names: what Explorer itself hands out), offered as a copy only (a drop never moves the originals out
//! of VRChat's folder), "Preferred DropEffect" copy, a drag image of our own and Explorer's drop descriptions ("Copy to
//! Desktop").
//!
//! egui and winit have no drag source. A page asks for a drag with [`start`] once its threshold is crossed, in the frame
//! that draws its cards lifted; the next frame's raw input hook ([`run_pending`], from `App::raw_input_hook`) runs
//! Windows' modal drag loop after that frame is on screen. The window does not repaint until the drop (winit keeps its
//! events for later). Windows took the mouse button's release, so egui is told about it afterwards, and where the
//! pointer is now.
use std::path::PathBuf;
use std::sync::Mutex;

/// The picture under the cursor while dragging: straight RGBA, `cursor` the point of it under the pointer.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub cursor: [i32; 2],
}

pub struct Drag {
    pub files: Vec<PathBuf>,
    pub image: Option<Image>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Dropped on another app, which took a copy.
    Copied,
    /// Let go over this window, or Escape.
    Cancelled,
    /// Windows refused it (the files could not be offered).
    Failed,
}

static PENDING: Mutex<Option<Drag>> = Mutex::new(None);
static LAST: Mutex<Option<Outcome>> = Mutex::new(None);

/// Asks for a drag of these files, run before the next frame.
pub fn start(drag: Drag) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(drag);
}

/// How the last drag ended, once.
pub fn take_outcome() -> Option<Outcome> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Runs the drag asked for, if any (modal: returns once dropped or cancelled), then gives egui the button's release.
pub fn run_pending(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let Some(drag) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    #[cfg(windows)]
    let (outcome, inside) = win::drag(&drag, ctx.pixels_per_point());
    #[cfg(not(windows))]
    let (outcome, inside) = (Outcome::Failed, None::<egui::Pos2>);
    let pos = inside.or_else(|| ctx.input(|i| i.pointer.latest_pos()));
    if let Some(pos) = pos {
        let modifiers = ctx.input(|i| i.modifiers);
        raw.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers });
    }
    match inside {
        Some(pos) => raw.events.push(egui::Event::PointerMoved(pos)),
        None => raw.events.push(egui::Event::PointerGone),
    }
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
    ctx.request_repaint();
}

#[cfg(windows)]
mod win {
    use super::{Drag, Image, Outcome};
    use std::mem::ManuallyDrop;
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Foundation::{COLORREF, HGLOBAL, HWND, POINT, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateDIBSection, DeleteObject, ScreenToClient, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{IDropSource, OleInitialize, DROPEFFECT_COPY};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, CLSID_DragDropHelper, IDragSourceHelper, IDragSourceHelper2, ILFree, SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
        SHParseDisplayName, DSH_ALLOWDROPDESCRIPTIONTEXT, SHDRAGIMAGE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, GetSystemMetrics, GetWindowThreadProcessId, WindowFromPoint, SM_SWAPBUTTON};

    /// The files' absolute item ids, freed when dropped.
    struct Ids(Vec<*mut ITEMIDLIST>);
    impl Drop for Ids {
        fn drop(&mut self) {
            for id in &self.0 {
                // SAFETY: each was allocated by SHParseDisplayName and is freed once.
                unsafe { ILFree(Some(*id as *const _)) };
            }
        }
    }

    /// Runs the drag; the outcome and, when the pointer is over this window now, where (in points).
    pub fn drag(drag: &Drag, pixels_per_point: f32) -> (Outcome, Option<egui::Pos2>) {
        // SAFETY: COM and shell calls on the window's thread (an OLE apartment: winit's drop target initialised it);
        // every pointer handed out is owned by its wrapper or freed above.
        unsafe {
            // The window holds the mouse while its button is down; let go already (between two frames), no drag.
            let hwnd = GetCapture();
            let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
            if hwnd.is_invalid() || (GetAsyncKeyState(i32::from(button.0)) as u16) & 0x8000 == 0 {
                return (Outcome::Cancelled, inside(hwnd, pixels_per_point));
            }
            let _ = OleInitialize(None);
            let outcome = match offer(drag) {
                Some(data) => match SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT_COPY) {
                    Ok(effect) if effect == DROPEFFECT_COPY && !over_self() => Outcome::Copied,
                    Ok(_) => Outcome::Cancelled,
                    Err(_) => Outcome::Failed,
                },
                None => Outcome::Failed,
            };
            (outcome, inside(hwnd, pixels_per_point))
        }
    }

    /// The shell's own data object for the files (as Explorer's), copy preferred, with the drag image.
    unsafe fn offer(drag: &Drag) -> Option<IDataObject> {
        unsafe {
            let mut ids = Ids(Vec::new());
            for file in &drag.files {
                let mut id = std::ptr::null_mut();
                if SHParseDisplayName(&HSTRING::from(file.as_os_str()), None, &mut id, 0, None).is_ok() && !id.is_null() {
                    ids.0.push(id);
                }
            }
            if ids.0.is_empty() {
                return None;
            }
            let list: Vec<*const ITEMIDLIST> = ids.0.iter().map(|id| *id as *const _).collect();
            let array = SHCreateShellItemArrayFromIDLists(&list).ok()?;
            let data: IDataObject = array.BindToHandler(None, &BHID_DataObject).ok()?;
            prefer_copy(&data);
            if let Some(image) = &drag.image {
                set_image(&data, image);
            }
            Some(data)
        }
    }

    /// "Preferred DropEffect": copy (Explorer would move files dropped on the same drive otherwise).
    unsafe fn prefer_copy(data: &IDataObject) {
        unsafe {
            let format = RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect"));
            let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, 4) else { return };
            let at = GlobalLock(memory) as *mut u32;
            if at.is_null() {
                return;
            }
            *at = DROPEFFECT_COPY.0;
            let _ = GlobalUnlock(memory);
            let f = FORMATETC { cfFormat: format as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
            let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: HGLOBAL(memory.0) }, pUnkForRelease: ManuallyDrop::new(None) };
            // The data object owns the memory from here on (fRelease).
            let _ = data.SetData(&f, &medium, true);
        }
    }

    /// The drag image, through the shell's drag helper (it shows over every window, with drop descriptions).
    unsafe fn set_image(data: &IDataObject, image: &Image) {
        unsafe {
            let Ok(helper) = CoCreateInstance::<_, IDragSourceHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) else { return };
            if let Ok(helper2) = helper.cast::<IDragSourceHelper2>() {
                let _ = helper2.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32);
            }
            let Some(bitmap) = bitmap(image) else { return };
            let picture = SHDRAGIMAGE {
                sizeDragImage: SIZE { cx: image.width as i32, cy: image.height as i32 },
                ptOffset: POINT { x: image.cursor[0], y: image.cursor[1] },
                hbmpDragImage: bitmap,
                crColorKey: COLORREF(0xFFFF_FFFF),
            };
            // The helper owns the bitmap once it took it.
            if helper.InitializeFromBitmap(&picture, data).is_err() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
            }
        }
    }

    /// A bottom-up 32-bit DIB of premultiplied BGRA, as the drag helper takes them.
    unsafe fn bitmap(image: &Image) -> Option<HBITMAP> {
        unsafe {
            let (w, h) = (image.width as usize, image.height as usize);
            if w == 0 || h == 0 || image.rgba.len() != w * h * 4 {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: h as i32,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let out = std::slice::from_raw_parts_mut(bits as *mut u8, w * h * 4);
            for y in 0..h {
                let row = &image.rgba[(h - 1 - y) * w * 4..(h - y) * w * 4];
                for (x, p) in row.chunks_exact(4).enumerate() {
                    let a = u32::from(p[3]);
                    let o = &mut out[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    o[0] = ((u32::from(p[2]) * a + 127) / 255) as u8;
                    o[1] = ((u32::from(p[1]) * a + 127) / 255) as u8;
                    o[2] = ((u32::from(p[0]) * a + 127) / 255) as u8;
                    o[3] = p[3];
                }
            }
            Some(bitmap)
        }
    }

    /// Whether the drop landed on one of this app's own windows (it takes nothing: not a copy).
    unsafe fn over_self() -> bool {
        unsafe {
            let mut at = POINT::default();
            if GetCursorPos(&mut at).is_err() {
                return false;
            }
            let window = WindowFromPoint(at);
            let mut process = 0u32;
            GetWindowThreadProcessId(window, Some(&mut process));
            process == GetCurrentProcessId()
        }
    }

    /// The pointer in the window's client area, in points, when it is inside it.
    unsafe fn inside(hwnd: HWND, pixels_per_point: f32) -> Option<egui::Pos2> {
        unsafe {
            if hwnd.is_invalid() {
                return None;
            }
            let mut at = POINT::default();
            GetCursorPos(&mut at).ok()?;
            let _ = ScreenToClient(hwnd, &mut at);
            let mut client = Default::default();
            GetClientRect(hwnd, &mut client).ok()?;
            let windows::Win32::Foundation::RECT { right, bottom, .. } = client;
            (at.x >= 0 && at.y >= 0 && at.x < right && at.y < bottom).then(|| egui::pos2(at.x as f32 / pixels_per_point, at.y as f32 / pixels_per_point))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::System::Com::IDataObject;
        use windows::Win32::System::Ole::{IDropTarget, ReleaseStgMedium, CF_HDROP};
        use windows::Win32::System::SystemServices::MK_LBUTTON;
        use windows::Win32::UI::Shell::{DragQueryFileW, IShellFolder, SHBindToParent, HDROP};
        use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};

        fn format(cf: u16) -> FORMATETC {
            FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
        }

        /// What the drag hands out, dropped by hand into Explorer's own drop handler for a folder (as a drop on that folder's
        /// window would): the files from two folders come out as CF_HDROP, copy is preferred, the drag image is attached,
        /// and Explorer copies them, leaving the originals.
        #[test]
        #[ignore = "runs Explorer's copy engine on a temporary folder: cargo test -p orbiters-desktop drag_out -- --ignored"]
        fn explorer_copies_what_is_dragged_out() {
            let root = std::env::temp_dir().join(format!("orbiters-drag-out-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let (pictures, captures, folder) = (root.join("VRChat 2026-10"), root.join("dragged"), root.join("Dropped here"));
            for d in [&pictures, &captures, &folder] {
                std::fs::create_dir_all(d).unwrap();
            }
            let a = pictures.join("VRChat_2026-10-29_21-30-00.000_1920x1080.png");
            let b = captures.join("Silent Dawn 2026-10-28 21-00-00.png");
            std::fs::write(&a, b"first picture").unwrap();
            std::fs::write(&b, b"a capture's picture").unwrap();
            let image = Image { width: 40, height: 30, rgba: vec![200; 40 * 30 * 4], cursor: [20, 15] };
            // SAFETY: COM on this test's thread, every handle released below.
            unsafe {
                let _ = OleInitialize(None);
                let data: IDataObject = offer(&Drag { files: vec![a.clone(), b.clone()], image: Some(image) }).expect("the shell's data object");

                let mut medium = data.GetData(&format(CF_HDROP.0)).expect("CF_HDROP");
                let drop = HDROP(medium.u.hGlobal.0);
                assert_eq!(DragQueryFileW(drop, u32::MAX, None), 2);
                let mut names = Vec::new();
                for i in 0..2 {
                    let mut buf = [0u16; 520];
                    let n = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    names.push(String::from_utf16_lossy(&buf[..n]));
                }
                ReleaseStgMedium(&mut medium);
                assert_eq!(names, vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()]);

                let preferred = RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect")) as u16;
                let mut medium = data.GetData(&format(preferred)).expect("Preferred DropEffect");
                assert_eq!(*(GlobalLock(medium.u.hGlobal) as *const u32), DROPEFFECT_COPY.0);
                let _ = GlobalUnlock(medium.u.hGlobal);
                ReleaseStgMedium(&mut medium);
                let image = RegisterClipboardFormatW(windows::core::w!("DragImageBits")) as u16;
                assert!(data.QueryGetData(&format(image)).is_ok(), "the drag image travels with the files");

                let mut id = std::ptr::null_mut();
                SHParseDisplayName(&HSTRING::from(folder.as_os_str()), None, &mut id, 0, None).unwrap();
                let ids = Ids(vec![id]);
                let mut last = std::ptr::null_mut();
                let parent: IShellFolder = SHBindToParent(ids.0[0], Some(&mut last)).unwrap();
                let target: IDropTarget = parent.GetUIObjectOf(HWND::default(), &[last as *const _], None).unwrap();
                let at = windows::Win32::Foundation::POINTL { x: 0, y: 0 };
                let mut effect = DROPEFFECT_COPY;
                target.DragEnter(&data, MK_LBUTTON, at, &mut effect).unwrap();
                assert_eq!(effect, DROPEFFECT_COPY, "Explorer offers a copy");
                let mut effect = DROPEFFECT_COPY;
                target.Drop(&data, MK_LBUTTON, at, &mut effect).unwrap();
                // Explorer may copy on its own thread: wait for the files, answering this thread's messages.
                let copied = [folder.join(a.file_name().unwrap()), folder.join(b.file_name().unwrap())];
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while !copied.iter().all(|p| p.is_file()) && std::time::Instant::now() < until {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                assert!(copied.iter().all(|p| p.is_file()), "Explorer copied both files");
                assert_eq!(std::fs::read(&copied[1]).unwrap(), b"a capture's picture");
                assert!(a.is_file() && b.is_file(), "the originals stay where they were");
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_waits_for_the_next_frame_and_is_taken_once() {
        start(Drag { files: vec![PathBuf::from("nothing.png")], image: None });
        let taken = PENDING.lock().unwrap().take();
        assert!(taken.is_some_and(|d| d.files.len() == 1));
        assert!(PENDING.lock().unwrap().is_none());
    }
}
