//! What the setup asks of Windows: the processes running from the install folder and the app's quit request, the
//! installed-apps entry and the start with Windows (the person's own part of the registry), the shortcuts, free
//! space, and starting a program on its own.

use orbiters_update::{APP_EXE, DISPLAY_NAME, PUBLISHER, RUN_KEY, RUN_VALUE, SETUP_EXE, UNINSTALL_KEY};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{w, Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE,
    REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    OpenEventW, OpenProcess, QueryFullProcessImageNameW, SetEvent, TerminateProcess, WaitForSingleObject, EVENT_MODIFY_STATE, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

/// A handle closed when it goes.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// A path as Windows compares them: absolute, lower case, `\` between folders.
fn key(path: &Path) -> String {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    path.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// The processes whose program is in `dir` (or a folder in it), but this one: the app, its window and overlay, and the
/// workers it started (the capture worker may still be saving a recording after the app quit).
pub fn processes_in(dir: &Path) -> Vec<u32> {
    running_from(dir).into_iter().map(|(pid, _)| pid).collect()
}

/// Whether the app itself (`orbiters-desktop.exe`, the tray service, its window or overlay) runs from `dir`: the
/// one a quit request reaches is then this install's.
pub fn app_runs_from(dir: &Path) -> bool {
    running_from(dir).iter().any(|(_, image)| image.file_name().is_some_and(|n| n.eq_ignore_ascii_case(APP_EXE)))
}

/// The processes running from `dir`, with their programs.
fn running_from(dir: &Path) -> Vec<(u32, PathBuf)> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let prefix = format!("{}\\", key(dir));
    let me = std::process::id();
    let mut found = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
        let snapshot = Owned(snapshot);
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
        while more {
            let pid = entry.th32ProcessID;
            if pid != me && pid > 4 {
                if let Some(image) = image_of(pid) {
                    if key(&image).starts_with(&prefix) {
                        found.push((pid, image));
                    }
                }
            }
            more = Process32NextW(snapshot.0, &mut entry).is_ok();
        }
    }
    found
}

/// The program a process runs.
fn image_of(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Asks the running app to quit as its own Quit does (it finishes saving a recording first): what
/// `orbiters-desktop --quit` sends. False when no app runs to hear it.
pub fn ask_app_to_quit() -> bool {
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\OrbitersDesktop.Quit")) {
            Ok(event) => {
                let event = Owned(event);
                SetEvent(event.0).is_ok()
            }
            Err(_) => false,
        }
    }
}

/// Waits up to `limit` for process `pid` to end; true once it has (or when there is no such process).
pub fn wait_for_exit(pid: u32, limit: Duration) -> bool {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
        let process = Owned(process);
        WaitForSingleObject(process.0, limit.as_millis().min(u32::MAX as u128 - 1) as u32) == WAIT_OBJECT_0
    }
}

/// Waits for every process running from `dir` to end, up to `limit`; true once none is left. `stop` ends it early.
pub fn wait_for_folder(dir: &Path, limit: Duration, stop: impl Fn() -> bool) -> bool {
    let until = Instant::now() + limit;
    loop {
        let running = processes_in(dir);
        if running.is_empty() {
            return true;
        }
        if Instant::now() >= until || stop() {
            return false;
        }
        for pid in running {
            if wait_for_exit(pid, Duration::from_millis(250)) {
                break;
            }
        }
    }
}

/// Ends every process running from `dir` (the person chose to close what would not close).
pub fn terminate_folder(dir: &Path) {
    for pid in processes_in(dir) {
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) {
                let process = Owned(process);
                let _ = TerminateProcess(process.0, 1);
                WaitForSingleObject(process.0, 5000);
            }
        }
    }
}

/// Bytes free for this person on the drive of `dir` (or the nearest folder above it that exists).
pub fn free_space(dir: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let existing = dir.ancestors().find(|d| d.is_dir())?;
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(existing.as_os_str()), Some(&mut free), None, None).ok()? };
    Some(free)
}

/// Starts `program` on its own (not a child that ends with the setup), from its folder.
pub fn launch(program: &Path, args: &[&str]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if let Some(dir) = program.parent() {
        cmd.current_dir(dir);
    }
    // A new process group: Ctrl+C in a console the setup was started from does not reach it.
    cmd.creation_flags(0x0000_0200);
    cmd.spawn().map(drop)
}

/// Removes `file` once this program has ended (a program cannot remove itself while it runs): a hidden `cmd` waits a
/// couple of seconds, then deletes it.
pub fn delete_after_exit(file: &Path) {
    use std::os::windows::process::CommandExt;
    let line = format!("/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"", file.display());
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let _ = std::process::Command::new(system.join("System32").join("cmd.exe"))
        .raw_arg(line)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn();
}

/// A registry key of the person's, opened for writing (made when missing), closed when it goes.
struct Key(HKEY);

impl Key {
    fn create(path: &str) -> anyhow::Result<Self> {
        let mut key = HKEY::default();
        unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None).ok()?
        };
        Ok(Self(key))
    }

    fn set_text(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
        unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, REG_SZ, Some(&bytes)).ok()? };
        Ok(())
    }

    fn set_number(&self, name: &str, value: u32) -> anyhow::Result<()> {
        unsafe { RegSetValueExW(self.0, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())).ok()? };
        Ok(())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// A text value of the person's registry.
fn read_text(path: &str, name: &str) -> Option<String> {
    let mut buf = vec![0u16; 4096];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(path), &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if status.is_err() {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Where Windows' installed apps say Orbiters - N4N0 is.
pub fn registered_dir() -> Option<PathBuf> {
    read_text(UNINSTALL_KEY, "InstallLocation").filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// The app's entry in Windows' installed apps (Settings, Apps): its name, icon, version, size, and how to uninstall.
pub fn register(dir: &Path, version: &str, size_bytes: u64) -> anyhow::Result<()> {
    let key = Key::create(UNINSTALL_KEY)?;
    let setup = dir.join(SETUP_EXE);
    key.set_text("DisplayName", DISPLAY_NAME)?;
    key.set_text("DisplayVersion", version)?;
    key.set_text("Publisher", PUBLISHER)?;
    key.set_text("DisplayIcon", &format!("{},0", dir.join(APP_EXE).display()))?;
    key.set_text("InstallLocation", &dir.display().to_string())?;
    key.set_text("UninstallString", &format!("\"{}\" --uninstall", setup.display()))?;
    key.set_text("QuietUninstallString", &format!("\"{}\" --uninstall --silent", setup.display()))?;
    key.set_text("URLInfoAbout", "https://orbiters.cc")?;
    key.set_text("HelpLink", "https://orbiters.cc")?;
    let today = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    key.set_text("InstallDate", &format!("{:04}{:02}{:02}", today.year(), today.month() as u8, today.day()))?;
    // In kilobytes.
    key.set_number("EstimatedSize", size_bytes.div_ceil(1024).min(u32::MAX as u64) as u32)?;
    key.set_number("NoModify", 1)?;
    key.set_number("NoRepair", 1)?;
    Ok(())
}

/// Takes the app out of Windows' installed apps.
pub fn unregister() {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY));
        let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY));
    }
}

/// Starts the app (in the tray, without its window) when the person signs in to Windows, or stops doing so.
pub fn set_start_with_windows(dir: &Path, on: bool) -> anyhow::Result<()> {
    if on {
        Key::create(RUN_KEY)?.set_text(RUN_VALUE, &format!("\"{}\" --hidden", dir.join(APP_EXE).display()))
    } else {
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_VALUE));
        }
        Ok(())
    }
}

/// Whether the start with Windows points into `dir` (left alone otherwise: another install's).
pub fn starts_with_windows_from(dir: &Path) -> bool {
    let Some(line) = read_text(RUN_KEY, RUN_VALUE) else { return false };
    let line = line.trim();
    // `"<program>" --hidden`, or a program without quotes.
    let program = match line.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => line.split(' ').next().unwrap_or_default(),
    };
    key(Path::new(program)).starts_with(&format!("{}\\", key(dir)))
}

/// The person's Start menu programs folder, or desktop.
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text.map(PathBuf::from)
    }
}

/// The app's shortcut in the Start menu.
pub fn start_menu_shortcut() -> Option<PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Programs).map(|d| d.join(format!("{DISPLAY_NAME}.lnk")))
}

/// The app's shortcut on the desktop.
pub fn desktop_shortcut() -> Option<PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop).map(|d| d.join(format!("{DISPLAY_NAME}.lnk")))
}

/// Writes a shortcut at `at` to the app in `dir`.
pub fn write_shortcut(at: &Path, dir: &Path) -> anyhow::Result<()> {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    let target = dir.join(APP_EXE);
    unsafe {
        // Once per thread; a second time only says it already is.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
        link.SetDescription(&HSTRING::from("Capture VRChat worlds, talk with N4N0 and call friends"))?;
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)?;
        }
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(at.as_os_str()), true)?;
    }
    Ok(())
}

/// Tells Explorer the shortcuts and icons changed (the Start menu shows the new one at once).
pub fn refresh_shell() {
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// A plain Windows message box, for when the setup's own window cannot open (no usable graphics). `question` asks
/// with OK and Cancel; true for OK.
pub fn message(text: &str, question: bool, error: bool) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDOK, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_OKCANCEL, MB_SETFOREGROUND};
    let style = (if question { MB_OKCANCEL } else { MB_OK }) | (if error { MB_ICONERROR } else { MB_ICONINFORMATION }) | MB_SETFOREGROUND;
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(format!("{DISPLAY_NAME} setup")), style) == IDOK }
}
