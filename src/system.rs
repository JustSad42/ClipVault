//! Talking to the rest of Windows: which app copied something, where its text cursor is, the
//! theme, sounds, dialogs and opening things.

use crate::images::Bitmap;
use crate::store::Source;
use crate::win::{from_wide, wide};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use windows::UI::ViewManagement::{UIColorType, UISettings};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::DataExchange::GetClipboardOwner;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{Interface, PCWSTR, PWSTR, w};

/// The full path of the program that owns a window (never our own).
pub fn exe_of(window: HWND) -> Option<String> {
    if window.0.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// "code" for C:\…\Code.exe
pub fn exe_name(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The program's friendly name (its file description), else the file's name.
pub fn app_name(path: &str) -> String {
    let fallback = || Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    description(path).map(|d| d.trim().to_string()).filter(|d| !d.is_empty() && d.chars().count() <= 40).unwrap_or_else(fallback)
}

fn description(path: &str) -> Option<String> {
    let file = wide(path);
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(file.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(file.as_ptr()), None, size, data.as_mut_ptr() as _).ok()?;
        let query = |q: &str| -> Option<(*const u8, u32)> {
            let q = wide(q);
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            VerQueryValueW(data.as_ptr() as _, PCWSTR(q.as_ptr()), &mut p, &mut len).as_bool().then_some((p as *const u8, len))
        };
        // the first language the file lists, then US English
        let mut languages = Vec::new();
        if let Some((p, len)) = query(r"\VarFileInfo\Translation") {
            if len >= 4 {
                let pair = std::slice::from_raw_parts(p as *const u16, 2);
                languages.push(format!("{:04x}{:04x}", pair[0], pair[1]));
            }
        }
        languages.extend(["040904b0".to_string(), "040904e4".to_string()]);
        for lang in languages {
            if let Some((p, len)) = query(&format!(r"\StringFileInfo\{lang}\FileDescription")) {
                if len > 0 {
                    return Some(from_wide(std::slice::from_raw_parts(p as *const u16, len as usize)));
                }
            }
        }
        None
    }
}

/// Who put the current clipboard content there: its owner, else the window in front.
pub fn copy_source() -> Source {
    let path = unsafe { GetClipboardOwner().ok().and_then(exe_of).or_else(|| exe_of(GetForegroundWindow())) };
    Source { name: path.as_deref().map(app_name), path }
}

/// The icon Windows shows for a program, file or folder.
pub fn shell_icon(path: &str, small: bool) -> Option<Bitmap> {
    // don't wait on a network drive for an icon
    if path.starts_with(r"\\") {
        return None;
    }
    let file = wide(path);
    let mut info = SHFILEINFOW::default();
    let flags = if small { SHGFI_ICON | SHGFI_SMALLICON } else { SHGFI_ICON };
    let ok = unsafe { SHGetFileInfoW(PCWSTR(file.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut info), size_of::<SHFILEINFOW>() as u32, flags) };
    (ok != 0 && !info.hIcon.is_invalid()).then(|| Bitmap::from_icon(info.hIcon).ok()).flatten()
}

/// Where the text cursor of the window you're typing in is, in screen pixels.
/// Tries the classic Windows caret, then the accessibility caret (Chrome, Edge, Office…).
pub fn caret_rect(target: HWND) -> Option<RECT> {
    if target.0.is_null() {
        return None;
    }
    unsafe {
        let thread = GetWindowThreadProcessId(target, None);
        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        GetGUIThreadInfo(thread, &mut info).ok()?;
        if !info.hwndCaret.0.is_null() && info.rcCaret.bottom > info.rcCaret.top {
            let mut tl = POINT { x: info.rcCaret.left, y: info.rcCaret.top };
            let mut br = POINT { x: info.rcCaret.right, y: info.rcCaret.bottom };
            let _ = ClientToScreen(info.hwndCaret, &mut tl);
            let _ = ClientToScreen(info.hwndCaret, &mut br);
            return Some(RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y });
        }
        let focus = if info.hwndFocus.0.is_null() { target } else { info.hwndFocus };
        const OBJID_CARET: u32 = 0xFFFF_FFF8;
        let mut raw = std::ptr::null_mut();
        AccessibleObjectFromWindow(focus, OBJID_CARET, &IAccessible::IID, &mut raw).ok()?;
        if raw.is_null() {
            return None;
        }
        let acc = IAccessible::from_raw(raw);
        let (mut l, mut t, mut w, mut h) = (0, 0, 0, 0);
        acc.accLocation(&mut l, &mut t, &mut w, &mut h, &VARIANT::from(0i32)).ok()?;
        ((l != 0 || t != 0) && h > 0).then_some(RECT { left: l, top: t, right: l + w, bottom: t + h })
    }
}

/// The usable area of the monitor at a point, and its scale (1.0 = 96 dpi).
pub fn monitor_at(point: POINT) -> (RECT, f32) {
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut x, mut y) = (96, 96);
        let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(monitor, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut x, &mut y);
        (info.rcWork, x as f32 / 96.0)
    }
}

/// Brings a window to the front even when Windows' focus-stealing protection would block it, by
/// briefly sharing input state with the window now in front.
pub fn force_foreground(hwnd: HWND) {
    unsafe {
        let front = GetForegroundWindow();
        let other = GetWindowThreadProcessId(front, None);
        let mine = GetCurrentThreadId();
        // never attach to a hung window: that would hang us with it
        if !front.0.is_null() && other != mine && !IsHungAppWindow(front).as_bool() && AttachThreadInput(mine, other, true).as_bool() {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(mine, other, false);
        } else {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub fn system_is_dark() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut value as *mut _ as _), Some(&mut size))
    };
    ok.is_err() || value == 0
}

/// The accent colour picked in Windows, in the shade Windows uses for this theme (0xRRGGBB).
pub fn accent(dark: bool) -> Option<u32> {
    let kind = if dark { UIColorType::AccentLight2 } else { UIColorType::AccentDark1 };
    let c = UISettings::new().ok()?.GetColorValue(kind).ok()?;
    Some(u32::from(c.R) << 16 | u32::from(c.G) << 8 | u32::from(c.B))
}

// ---------- sounds ----------

/// A small WAV of gentle sine tones with soft starts and endings.
fn tones(notes: &[(f64, f64)]) -> Vec<u8> {
    const RATE: u32 = 44_100;
    let mut samples: Vec<i16> = Vec::new();
    for &(hz, seconds) in notes {
        let n = (f64::from(RATE) * seconds) as usize;
        for i in 0..n {
            let t = i as f64 / f64::from(RATE);
            let fade_in = (i as f64 / (f64::from(RATE) * 0.004)).min(1.0);
            let envelope = fade_in * (1.0 - i as f64 / n as f64).powi(2);
            samples.push(((2.0 * std::f64::consts::PI * hz * t).sin() * envelope * 0.16 * f64::from(i16::MAX)) as i16);
        }
    }
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(RATE.to_le_bytes());
    out.extend((RATE * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend(data.to_le_bytes());
    out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    out
}

pub fn play(paste: bool) {
    static COPY: LazyLock<Vec<u8>> = LazyLock::new(|| tones(&[(1320.0, 0.045)]));
    static PASTE: LazyLock<Vec<u8>> = LazyLock::new(|| tones(&[(880.0, 0.04), (1175.0, 0.06)]));
    let wav: &'static [u8] = if paste { &PASTE } else { &COPY };
    // the buffer is static, so it outlives the asynchronous playback
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

// ---------- opening things and asking ----------

/// Opens a web address with the default browser. Only http and https.
pub fn open_url(url: &str) -> bool {
    if crate::model::web_host(url).is_none() {
        return false;
    }
    let w = wide(url);
    unsafe { ShellExecuteW(None, w!("open"), PCWSTR(w.as_ptr()), None, None, SW_SHOWNORMAL).0 as usize > 32 }
}

pub fn open_folder(path: &Path) {
    let w = wide(&path.to_string_lossy());
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(w.as_ptr()), None, None, SW_SHOWNORMAL);
    }
}

/// Opens File Explorer with the file selected.
pub fn reveal(path: &str) {
    let w = wide(path);
    unsafe {
        let item = ILCreateFromPathW(PCWSTR(w.as_ptr()));
        if !item.is_null() {
            let _ = SHOpenFolderAndSelectItems(item, None, 0);
            ILFree(Some(item));
        }
    }
}

/// A save dialog. `types`: (label, pattern). Returns the chosen path and the chosen type's number.
pub fn save_dialog(owner: HWND, suggested: &str, types: &[(&str, &str)]) -> Option<(PathBuf, u32)> {
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let owned: Vec<(Vec<u16>, Vec<u16>)> = types.iter().map(|(n, p)| (wide(n), wide(p))).collect();
        let specs: Vec<COMDLG_FILTERSPEC> = owned.iter().map(|(n, p)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(p.as_ptr()) }).collect();
        dialog.SetFileTypes(&specs).ok()?;
        let name = wide(suggested);
        dialog.SetFileName(PCWSTR(name.as_ptr())).ok()?;
        if let Some(ext) = Path::new(suggested).extension() {
            let ext = wide(&ext.to_string_lossy());
            let _ = dialog.SetDefaultExtension(PCWSTR(ext.as_ptr()));
        }
        let _ = dialog.SetTitle(w!("Save to file"));
        dialog.Show(Some(owner)).ok()?;
        let result = dialog.GetResult().ok()?;
        let path = result.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let out = PathBuf::from(path.to_string().unwrap_or_default());
        CoTaskMemFree(Some(path.0 as _));
        Some((out, dialog.GetFileTypeIndex().unwrap_or(1)))
    }
}

pub fn confirm(owner: HWND, text: &str) -> bool {
    let w = wide(text);
    unsafe { MessageBoxW(Some(owner), PCWSTR(w.as_ptr()), w!("ClipVault"), MB_OKCANCEL | MB_ICONWARNING) == IDOK }
}

pub fn warn(owner: Option<HWND>, text: &str) {
    let w = wide(text);
    unsafe {
        MessageBoxW(owner, PCWSTR(w.as_ptr()), w!("ClipVault"), MB_OK | MB_ICONWARNING | MB_SETFOREGROUND);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sounds() {
        assert_eq!(exe_name(r"C:\x\Code.exe"), "code");
        assert_eq!(app_name(r"C:\Windows\explorer.exe"), "Windows Explorer");
        assert_eq!(app_name(r"C:\nope\thing.exe"), "thing");
        let w = tones(&[(440.0, 0.01)]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize, w.len() - 44);
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        assert!(shell_icon(r"C:\Windows\explorer.exe", true).is_some_and(|b| b.w >= 16));
    }
}
