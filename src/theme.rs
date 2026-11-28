//! Colours (0xAARRGGBB). Most are see-through so they sit well on the frosted-glass background.

use crate::model::Card;
use crate::settings::{Settings, Theme as Choice};
use crate::system;
use std::cell::Cell;
use std::sync::LazyLock;

thread_local! {
    static LAST: Cell<Option<Theme>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Solid background: menus, tooltips, toasts.
    pub bg: u32,
    /// The window fill: tinted glass, or `bg` where Windows has no acrylic.
    pub surface: u32,
    pub panel: u32,
    pub line: u32,
    pub fg: u32,
    pub muted: u32,
    pub hover: u32,
    pub selected: u32,
    pub thumb: u32,
    pub accent: u32,
    pub accent_soft: u32,
    /// Behind matched search letters.
    pub mark: u32,
    pub on_accent: u32,
}

/// Acrylic needs Windows 11 22H2 or newer.
pub fn glass() -> bool {
    static GLASS: LazyLock<bool> = LazyLock::new(|| {
        use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
        let mut buf = [0u16; 16];
        let mut size = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                windows_core::w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
                windows_core::w!("CurrentBuildNumber"),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as _),
                Some(&mut size),
            )
        };
        ok.is_ok() && crate::win::from_wide(&buf).parse::<u32>().is_ok_and(|b| b >= 22_621)
    });
    *GLASS
}

impl Theme {
    pub fn current(s: &Settings) -> Theme {
        let dark = match s.theme {
            Choice::Dark => true,
            Choice::Light => false,
            Choice::System => system::system_is_dark(),
        };
        let accent = s.use_system_accent.then(|| system::accent(dark)).flatten();
        let theme = Theme::build(dark, accent, glass());
        LAST.set(Some(theme));
        theme
    }

    /// The theme in use, for windows that can't reach the shared state.
    pub fn last() -> Theme {
        LAST.get().unwrap_or_else(|| Theme::build(true, None, false))
    }

    pub fn build(dark: bool, accent: Option<u32>, glass: bool) -> Theme {
        let pick = |d: u32, l: u32| if dark { d } else { l };
        let accent = accent.unwrap_or(pick(0x8AA2FF, 0x3D5AFE)) & 0x00FF_FFFF;
        let bg = pick(0xFF26_272D, 0xFFF9_F9FB);
        let (r, g, b) = ((accent >> 16) & 255, (accent >> 8) & 255, accent & 255);
        // dark text on light accents (Windows does the same), white on dark ones
        let luminance = (299 * r + 587 * g + 114 * b) / 1000;
        Theme {
            dark,
            bg,
            surface: if glass { pick(0xA61C_1D22, 0xB8F5_F5F7) } else { bg },
            panel: pick(0x14FF_FFFF, 0xB3FF_FFFF),
            line: pick(0x1FFF_FFFF, 0x1A00_0000),
            fg: pick(0xFFEC_ECF0, 0xFF1B_1B1F),
            muted: pick(0xFF9A_9CA6, 0xFF62_646E),
            hover: pick(0x0FFF_FFFF, 0x0A00_0000),
            selected: pick(0x1CFF_FFFF, 0x1200_0000),
            thumb: pick(0x40FF_FFFF, 0x4000_0000),
            accent: 0xFF00_0000 | accent,
            accent_soft: pick(0x40, 0x2E) << 24 | accent,
            mark: pick(0x5C, 0x45) << 24 | accent,
            on_accent: if luminance > 153 { 0xFF11_1114 } else { 0xFFFF_FFFF },
        }
    }

    /// The colour of a kind of item (stripe and type tag), and its soft background.
    pub fn kind(&self, card: Card, command: bool) -> (u32, u32) {
        let (d, l) = match card {
            Card::Plain if command => (0x4FD1A5, 0x0F8A66),
            Card::Plain => (0xA99BFF, 0x6B5BD6),
            Card::Code => (0x4FD1A5, 0x0F8A66),
            Card::Link => (0x6CB4FF, 0x1F6FD1),
            Card::Color => (0xF58BB8, 0xC2386F),
            Card::Email | Card::Phone => (0xF59F7A, 0xC0532A),
            Card::Image => (0xF2B84B, 0xA86A0A),
            Card::Files => (0xA8A9B3, 0x6A6C77),
        };
        let c = if self.dark { d } else { l };
        (0xFF00_0000 | c, if self.dark { 0x30 } else { 0x24 } << 24 | c)
    }
}
