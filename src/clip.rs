//! The Windows clipboard: reading what was copied, writing history items back, drag-and-drop data,
//! and pressing Ctrl+V.

use crate::images::Bitmap;
use crate::model::{Item, Kind};
use crate::win::wide;
use std::path::PathBuf;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP};
use windows_core::PCWSTR;

pub const CF_DIB: u32 = 8;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_HDROP: u32 = 15;
pub const CF_DIBV5: u32 = 17;

pub fn format(name: &str) -> u32 {
    let w = wide(name);
    unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
}

/// What is on the clipboard now, in the order ClipVault prefers it.
pub enum Content {
    Files(Vec<String>),
    Text { text: String, html: Option<String>, rtf: Option<String> },
    /// Raw picture bytes (a PNG, or a BMP file built from the clipboard's DIB).
    Image(Vec<u8>),
}

/// The clipboard, held open; closing happens when dropped.
struct Open;

impl Open {
    /// Another app may still have it open: try a few times.
    fn new(owner: Option<HWND>) -> Option<Open> {
        for attempt in 0..8 {
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
        }
        None
    }

    fn bytes(&self, format: u32) -> Option<Vec<u8>> {
        unsafe {
            let handle = GetClipboardData(format).ok()?;
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global);
            // a quarter gigabyte is far past anything worth keeping
            if size == 0 || size > 1 << 28 {
                return None;
            }
            let p = GlobalLock(global) as *const u8;
            if p.is_null() {
                return None;
            }
            let out = std::slice::from_raw_parts(p, size).to_vec();
            let _ = GlobalUnlock(global);
            Some(out)
        }
    }

    /// A NUL-terminated byte string format (HTML, RTF).
    fn narrow(&self, format: u32) -> Option<String> {
        let mut b = self.bytes(format)?;
        if let Some(end) = b.iter().position(|c| *c == 0) {
            b.truncate(end);
        }
        (!b.is_empty()).then(|| String::from_utf8_lossy(&b).into_owned())
    }

    fn text(&self) -> Option<String> {
        let b = self.bytes(CF_UNICODETEXT)?;
        let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(crate::win::from_wide(&units))
    }

    fn files(&self) -> Vec<String> {
        unsafe {
            let Ok(handle) = GetClipboardData(CF_HDROP) else { return vec![] };
            let drop = HDROP(handle.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            (0..n.min(10_000))
                .filter_map(|i| {
                    let len = DragQueryFileW(drop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    (DragQueryFileW(drop, i, Some(&mut buf)) > 0).then(|| crate::win::from_wide(&buf))
                })
                .collect()
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn available(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

/// A .bmp file made of a clipboard DIB, which Windows' codecs can read.
pub fn bmp_from_dib(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([dib[i], dib[i + 1], dib[i + 2], dib[i + 3]]);
    let header = u32_at(0) as usize;
    let bits = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32_at(16);
    let mut colours = u32_at(32) as usize;
    if colours == 0 && bits <= 8 {
        colours = 1 << bits;
    }
    // BI_BITFIELDS with the small header keeps its three masks after it
    let masks = if compression == 3 && header == 40 { 12 } else { 0 };
    let offset = 14usize.checked_add(header)?.checked_add(masks)?.checked_add(colours.checked_mul(4)?)?;
    if offset > 14 + dib.len() {
        return None;
    }
    let mut out = Vec::with_capacity(14 + dib.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// Reads the clipboard. `None`: nothing ClipVault keeps (or it couldn't be opened).
pub fn read(owner: HWND) -> Option<Content> {
    let open = Open::new(Some(owner))?;
    if available(CF_HDROP) {
        let files = open.files();
        if !files.is_empty() {
            return Some(Content::Files(files));
        }
    }
    if available(CF_UNICODETEXT) {
        let text = open.text()?;
        if text.trim().is_empty() {
            return None;
        }
        let (html, rtf) = (format("HTML Format"), format("Rich Text Format"));
        return Some(Content::Text { text, html: available(html).then(|| open.narrow(html)).flatten(), rtf: available(rtf).then(|| open.narrow(rtf)).flatten() });
    }
    // PNG keeps transparency; the standard bitmap is the fallback
    let png = format("PNG");
    if available(png) {
        if let Some(b) = open.bytes(png) {
            return Some(Content::Image(b));
        }
    }
    for f in [CF_DIBV5, CF_DIB] {
        if available(f) {
            if let Some(b) = open.bytes(f).and_then(|d| bmp_from_dib(&d)) {
                return Some(Content::Image(b));
            }
        }
    }
    None
}

/// Just the text on the clipboard (for pasting into our own fields).
pub fn read_text(owner: HWND) -> Option<String> {
    let open = Open::new(Some(owner))?;
    available(CF_UNICODETEXT).then(|| open.text()).flatten()
}

/// The formats an item is offered in, as (format, bytes).
pub struct Formats(pub Vec<(u32, Vec<u8>)>);

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn narrow(text: &str) -> Vec<u8> {
    text.bytes().chain([0]).collect()
}

/// A DROPFILES block naming these files.
pub fn drop_files(paths: &[&str]) -> Vec<u8> {
    let header = DROPFILES { pFiles: size_of::<DROPFILES>() as u32, fWide: true.into(), ..Default::default() };
    let mut out = unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<DROPFILES>()) }.to_vec();
    for p in paths {
        out.extend(utf16(p));
    }
    out.extend([0, 0]);
    out
}

/// A 32-bit DIB with an alpha channel (CF_DIBV5), bottom-up.
fn dib_v5(b: &Bitmap) -> Vec<u8> {
    let mut h = Vec::with_capacity(124 + b.px.len());
    let put = |h: &mut Vec<u8>, v: u32| h.extend_from_slice(&v.to_le_bytes());
    put(&mut h, 124);
    put(&mut h, b.w);
    put(&mut h, b.h);
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&32u16.to_le_bytes());
    put(&mut h, 3); // BI_BITFIELDS
    put(&mut h, b.px.len() as u32);
    for v in [0u32, 0, 0, 0] {
        put(&mut h, v);
    }
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        put(&mut h, mask);
    }
    put(&mut h, 0x7352_4742); // sRGB
    h.resize(124, 0);
    h[108..112].copy_from_slice(&4u32.to_le_bytes()); // LCS_GM_IMAGES
    for row in b.px.chunks_exact(b.w as usize * 4).rev() {
        h.extend_from_slice(row);
    }
    h
}

/// The clipboard (and drag-and-drop) formats for a history item. `plain`: text only.
/// `drag`: images also carry a real .png file, which most apps accept when dropped.
pub fn formats_for(item: &Item, image: Option<PathBuf>, plain: bool, drag: bool) -> Option<Formats> {
    let mut f = Vec::new();
    match item.kind {
        Kind::Text => {
            f.push((CF_UNICODETEXT, utf16(item.text.as_deref().unwrap_or(""))));
            if !plain {
                if let Some(h) = &item.html {
                    f.push((format("HTML Format"), narrow(h)));
                }
                if let Some(r) = &item.rtf {
                    f.push((format("Rich Text Format"), narrow(r)));
                }
            }
        }
        Kind::Files => {
            f.push((CF_UNICODETEXT, utf16(item.text.as_deref().unwrap_or(""))));
            if !plain {
                f.push((CF_HDROP, drop_files(&item.file_paths())));
            }
        }
        Kind::Image => {
            let path = image?;
            let bytes = std::fs::read(&path).ok()?;
            let bitmap = Bitmap::decode(&bytes, None).ok()?;
            f.push((CF_DIBV5, dib_v5(&bitmap)));
            f.push((format("PNG"), bytes.clone()));
            if drag {
                // a readable name for the dropped file instead of the internal id
                let dir = std::env::temp_dir().join("ClipVault");
                let _ = std::fs::create_dir_all(&dir);
                let named = dir.join(format!("Image {}.png", item.copied_at.format("%Y-%m-%d %H%M%S")));
                std::fs::write(&named, bytes).ok()?;
                f.push((CF_HDROP, drop_files(&[&named.to_string_lossy()])));
            }
        }
    }
    Some(Formats(f))
}

pub fn text_formats(text: &str) -> Formats {
    Formats(vec![(CF_UNICODETEXT, utf16(text))])
}

pub fn global(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        Some(g)
    }
}

/// Replaces the clipboard. Returns the new sequence number, to recognise our own change.
pub fn write(owner: HWND, formats: &Formats) -> Option<u32> {
    {
        let _open = Open::new(Some(owner))?;
        unsafe {
            EmptyClipboard().ok()?;
            for (format, bytes) in &formats.0 {
                let Some(g) = global(bytes) else { continue };
                // on success the clipboard owns the memory
                if SetClipboardData(*format, Some(HANDLE(g.0))).is_err() {
                    let _ = GlobalFree(Some(g));
                }
            }
        }
    }
    Some(sequence())
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Presses Ctrl+V in the focused window, releasing Shift, Alt and Win first so only Ctrl+V arrives.
pub fn press_paste() {
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() } },
    };
    let inputs = [
        key(VK_SHIFT, true),
        key(VK_MENU, true),
        key(VK_LWIN, true),
        key(VK_RWIN, true),
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_round_trip() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let b = Bitmap { w: 3, h: 2, px: (0..24).map(|i| if i % 4 == 3 { 255 } else { i * 10 }).collect() };
        let bmp = bmp_from_dib(&dib_v5(&b)).unwrap();
        let back = Bitmap::decode(&bmp, None).unwrap();
        assert_eq!((back.w, back.h), (3, 2));
        assert_eq!(back.px[..3], b.px[..3], "rows come back the right way up");
        assert!(bmp_from_dib(&[0; 10]).is_none());
        let d = drop_files(&["C:\\a"]);
        assert_eq!(&d[d.len() - 4..], &[0, 0, 0, 0]);
    }
}
