//! Little windows that never take the focus: the confirmation bubble ("Deleted · Undo") and the
//! hover preview. Both are per-pixel transparent layered windows, made when needed and destroyed
//! afterwards.

use crate::gfx::{self, Align, Image, R, Surface, Wrap, bold, fade, mono, r, text};
use crate::system;
use crate::theme::Theme;
use crate::win::wide;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

/// Room around a bubble for its shadow.
const MARGIN: f32 = 14.0;
const TIMER: usize = 1;

pub type Action = Box<dyn FnOnce()>;

struct Toast {
    hwnd: HWND,
    surface: Surface,
    at: POINT,
    shown: Instant,
    hold: u128,
    /// The button, in design units, and what it does.
    button: Option<(R, Action)>,
    scale: f32,
}

struct Tooltip {
    hwnd: HWND,
}

thread_local! {
    static TOAST: RefCell<Option<Toast>> = const { RefCell::new(None) };
    static TOOLTIP: RefCell<Option<Tooltip>> = const { RefCell::new(None) };
}

fn window(extra: WINDOW_EX_STYLE) -> HWND {
    let class = wide("ClipVaultOverlay");
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let hand = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.unwrap_or_default(), lpszClassName: PCWSTR(class.as_ptr()), hCursor: hand, ..Default::default() };
        RegisterClassW(&wc);
        let style = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | extra;
        CreateWindowExW(style, PCWSTR(class.as_ptr()), PCWSTR::null(), WS_POPUP, 0, 0, 1, 1, None, None, instance, None).unwrap_or_default()
    }
}

/// Puts the surface on screen at `at`, faded to `opacity`.
fn present(hwnd: HWND, surface: &Surface, at: POINT, opacity: f32) {
    let size = SIZE { cx: surface.w, cy: surface.h };
    let from = POINT::default();
    let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0) as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
    unsafe {
        let _ = UpdateLayeredWindow(hwnd, None, Some(&at), Some(&size), Some(surface.dc), Some(&from), COLORREF(0), Some(&blend), ULW_ALPHA);
    }
}

/// A rounded box with a soft shadow.
fn bubble(s: &Surface, t: &Theme, area: R, radius: f32) {
    for step in 1..=5 {
        let d = step as f32 * 2.2;
        s.fill(r(area.x - d, area.y - d + 2.0, area.w + 2.0 * d, area.h + 2.0 * d), radius + d, 0x0C00_0000);
    }
    s.panel(area, radius, t.bg, t.line);
}

// ---------- toast ----------

/// A confirmation near the bottom of `area` (screen pixels), or of the screen. With a button,
/// it stays up longer.
pub fn toast(message: &str, area: Option<RECT>, action: Option<(&str, Action)>) {
    hide_toast();
    let t = Theme::last();
    let mut mouse = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut mouse);
    }
    let anchor = area.map(|a| POINT { x: (a.left + a.right) / 2, y: (a.top + a.bottom) / 2 }).unwrap_or(mouse);
    let (work, scale) = system::monitor_at(anchor);
    let area = area.unwrap_or(work);

    let text_w = gfx::measure(message, text(13.0), 460.0, Wrap::Line).0.ceil() + 1.0;
    let button_w = action.as_ref().map(|(label, _)| gfx::measure(label, bold(12.5), 200.0, Wrap::Line).0.ceil() + 24.0);
    let width = 16.0 + 20.0 + text_w + button_w.map_or(18.0, |b| b + 22.0);
    let height = 38.0;
    let (w, h) = (((width + 2.0 * MARGIN) * scale).ceil() as i32, ((height + 2.0 * MARGIN) * scale).ceil() as i32);
    let Some(surface) = Surface::new(w, h) else { return };
    let pill = r(MARGIN, MARGIN, width, height);
    surface.begin(scale, 0);
    bubble(&surface, &t, pill, 18.0);
    surface.glyph("\u{E73E}", r(pill.x + 14.0, pill.y, 16.0, height), 12.0, t.accent);
    surface.text(message, r(pill.x + 36.0, pill.y, text_w, height), text(13.0), t.fg, Align::Left);
    let button = action.zip(button_w).map(|((label, run), bw)| {
        let area = r(pill.x + 36.0 + text_w + 12.0, pill.y + 6.0, bw, height - 12.0);
        surface.panel(area, 6.0, t.panel, t.line);
        surface.text(label, area, bold(12.5), t.accent, Align::Center);
        (area, run)
    });
    surface.end();

    // 56 px up from the bottom of the area, inside the screen
    let x = (area.left + (area.right - area.left - w) / 2).min(work.right - w).max(work.left);
    let y = (area.bottom - h - ((56.0 - MARGIN) * scale) as i32).min(work.bottom - h).max(work.top);
    let hwnd = window(WINDOW_EX_STYLE(0));
    let at = POINT { x, y };
    present(hwnd, &surface, at, 0.0);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER, 15, None);
    }
    let hold = if button.is_some() { 5000 } else { 1300 };
    TOAST.with_borrow_mut(|slot| *slot = Some(Toast { hwnd, surface, at, shown: Instant::now(), hold, button, scale }));
}

fn hide_toast() -> Option<Action> {
    let old = TOAST.with_borrow_mut(Option::take)?;
    unsafe {
        let _ = DestroyWindow(old.hwnd);
    }
    old.button.map(|b| b.1)
}

/// Fades in over 140 ms, waits, fades out over 300 ms.
fn toast_tick(hwnd: HWND) {
    let gone = TOAST.with_borrow(|slot| {
        let Some(t) = slot.as_ref().filter(|t| t.hwnd == hwnd) else { return false };
        let age = t.shown.elapsed().as_millis();
        let (opacity, next) = if age < 140 {
            (age as f32 / 140.0, 15)
        } else if age < t.hold {
            // nothing moves until the fade out: sleep until then
            (1.0, (t.hold - age) as u32)
        } else if age < t.hold + 300 {
            (1.0 - (age - t.hold) as f32 / 300.0, 15)
        } else {
            return true;
        };
        present(t.hwnd, &t.surface, t.at, opacity);
        unsafe {
            SetTimer(Some(hwnd), TIMER, next.max(10), None);
        }
        false
    });
    if gone {
        hide_toast();
    }
}

fn over_button(hwnd: HWND, lp: LPARAM) -> bool {
    TOAST.with_borrow(|slot| {
        slot.as_ref().filter(|t| t.hwnd == hwnd).is_some_and(|t| {
            let (x, y) = (f32::from(lp.0 as i16) / t.scale, f32::from((lp.0 >> 16) as i16) / t.scale);
            t.button.as_ref().is_some_and(|(area, _)| area.contains(x, y))
        })
    })
}

// ---------- hover preview ----------

pub fn hide_tooltip() {
    if let Some(old) = TOOLTIP.with_borrow_mut(Option::take) {
        unsafe {
            let _ = DestroyWindow(old.hwnd);
        }
    }
}

/// Shows a prepared box beside `anchor` (the hovered row, in screen pixels): right of it if
/// there's room, otherwise left.
fn tooltip(width: f32, height: f32, anchor: RECT, scale: f32, t: &Theme, content: impl FnOnce(&Surface, R)) {
    hide_tooltip();
    let (w, h) = (((width + 2.0 * MARGIN) * scale).ceil() as i32, ((height + 2.0 * MARGIN) * scale).ceil() as i32);
    let Some(surface) = Surface::new(w, h) else { return };
    let area = r(MARGIN, MARGIN, width, height);
    surface.begin(scale, 0);
    bubble(&surface, t, area, 8.0);
    content(&surface, area);
    surface.end();
    let (work, _) = system::monitor_at(POINT { x: anchor.right, y: anchor.top });
    let pad = (MARGIN * scale) as i32;
    let gap = (4.0 * scale) as i32;
    let mut x = anchor.right + gap - pad;
    if x + w - pad > work.right {
        x = anchor.left - gap - w + pad;
    }
    let x = x.max(work.left - pad);
    let y = (anchor.top - pad).min(work.bottom - h + pad).max(work.top - pad);
    let hwnd = window(WS_EX_TRANSPARENT);
    present(hwnd, &surface, POINT { x, y }, 1.0);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    TOOLTIP.with_borrow_mut(|slot| *slot = Some(Tooltip { hwnd }));
}

pub fn tooltip_text(t: &Theme, content: &str, anchor: RECT, scale: f32) {
    let font = mono(12.0);
    let (w, h) = gfx::measure(content, font, 500.0, Wrap::Words);
    let (w, h) = (w.ceil().min(500.0) + 1.0, h.ceil().min(520.0));
    let fg = fade(t.fg, 1.0);
    tooltip(w + 20.0, h + 16.0, anchor, scale, t, |s, area| {
        let inner = r(area.x + 10.0, area.y + 8.0, w, h);
        s.push_clip(inner);
        s.paragraph(content, inner, font, fg, Align::Left);
        s.pop_clip();
    });
}

pub fn tooltip_picture(t: &Theme, picture: Rc<Image>, anchor: RECT, scale: f32) {
    let k = (480.0 / picture.w as f32).min(360.0 / picture.h as f32).min(1.0 / scale);
    let (w, h) = (picture.w as f32 * k, picture.h as f32 * k);
    tooltip(w + 12.0, h + 12.0, anchor, scale, t, |s, area| {
        s.image(&picture, r(area.x + 6.0, area.y + 6.0, w, h), 1.0);
    });
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER => {
            toast_tick(h);
            LRESULT(0)
        }
        WM_SETCURSOR if over_button(h, cursor_in(h)) => {
            unsafe {
                SetCursor(LoadCursorW(None, IDC_HAND).ok());
            }
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            if over_button(h, lp) {
                if let Some(action) = hide_toast() {
                    action();
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

/// The mouse position inside a window, packed the way mouse messages carry it.
fn cursor_in(h: HWND) -> LPARAM {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(h, &mut p);
    }
    LPARAM(((p.y as isize) << 16) | (p.x as isize & 0xFFFF))
}
