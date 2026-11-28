//! Preferences (settings.json, the C# version's format), where data lives, and "Start with Windows".

use crate::win::{self, wide};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::LazyLock;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows_core::PCWSTR;

pub const MOD_ALT_: u32 = 1;
pub const MOD_CONTROL_: u32 = 2;
pub const MOD_SHIFT_: u32 = 4;
pub const MOD_WIN_: u32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenAt {
    #[default]
    TextCursor,
    Mouse,
    LastPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Settings {
    pub hotkey_modifiers: u32,
    pub hotkey_key: u32,
    pub max_history: usize,
    pub theme: Theme,
    pub open_at: OpenAt,
    pub use_system_accent: bool,
    /// Download a link's site icon (straight from that site) for link cards.
    pub link_icons: bool,
    /// Remove tracking parameters from links as they're copied.
    pub clean_links: bool,
    /// Apps (exe names without .exe) that always get plain text.
    pub plain_text_apps: Vec<String>,
    pub sounds: bool,
    pub compact: bool,
    pub popup_width: f64,
    pub popup_height: f64,
    pub popup_left: Option<f64>,
    pub popup_top: Option<f64>,
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey_modifiers: MOD_CONTROL_ | MOD_SHIFT_,
            hotkey_key: 0x56, // V
            max_history: 1000,
            theme: Theme::System,
            open_at: OpenAt::TextCursor,
            use_system_accent: true,
            link_icons: true,
            clean_links: true,
            plain_text_apps: vec![],
            sounds: false,
            compact: false,
            popup_width: 440.0,
            popup_height: 580.0,
            popup_left: None,
            popup_top: None,
            auto_update: true,
        }
    }
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read_to_string(dir().join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(dir());
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = win::write_atomic(&dir().join("settings.json"), text.as_bytes());
        }
    }

    pub fn hotkey_text(&self) -> String {
        hotkey_text(self.hotkey_modifiers, self.hotkey_key)
    }
}

pub fn hotkey_text(mods: u32, vk: u32) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (bit, name) in [(MOD_CONTROL_, "Ctrl"), (MOD_ALT_, "Alt"), (MOD_SHIFT_, "Shift"), (MOD_WIN_, "Win")] {
        if mods & bit != 0 {
            parts.push(name.into());
        }
    }
    parts.push(key_name(vk));
    parts.join(" + ")
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap_or('?').to_string(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0xC0 => "`".into(),
        0xBD => "-".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xBA => ";".into(),
        0xDE => "'".into(),
        0xDB => "[".into(),
        0xDD => "]".into(),
        0xDC => "\\".into(),
        _ => {
            // Windows' own name for the key
            let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC_EX) };
            let extended = if scan & 0xFF00 != 0 || (0x21..=0x2E).contains(&vk) { 1 << 24 } else { 0 };
            let mut buf = [0u16; 64];
            let n = unsafe { GetKeyNameTextW(((scan & 0xFF) << 16 | extended) as i32, &mut buf) };
            if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { format!("Key {vk}") }
        }
    }
}

/// %APPDATA%\ClipVault, or CLIPVAULT_DATA for test copies.
pub fn dir() -> PathBuf {
    static DIR: LazyLock<PathBuf> = LazyLock::new(|| {
        std::env::var_os("CLIPVAULT_DATA").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
            // tests never touch the real folder
            if cfg!(test) {
                return std::env::temp_dir().join("clipvault-tests");
            }
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("ClipVault")
        })
    });
    DIR.clone()
}

/// A test copy gets its own single-instance names, so it never talks to (or replaces) the real one.
pub fn is_test() -> bool {
    cfg!(test) || std::env::var_os("CLIPVAULT_DATA").is_some_and(|d| !d.is_empty())
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn startup_enabled() -> bool {
    let (key, value) = (wide(RUN_KEY), wide("ClipVault"));
    let mut size = 0u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok() }
}

pub fn set_startup(on: bool) {
    // a test copy never touches the real setting
    if is_test() {
        return;
    }
    let (key, value) = (wide(RUN_KEY), wide("ClipVault"));
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let data = wide(&format!("\"{}\" --background", exe.display()));
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()), REG_SZ.0, Some(data.as_ptr() as _), (data.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_stay_out_of_the_real_folder() {
        assert!(is_test());
        let real = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
        assert!(!dir().starts_with(real.join("ClipVault")));
    }

    #[test]
    fn reads_the_old_settings_file() {
        let s: Settings = serde_json::from_str(
            r#"{"HotkeyModifiers":6,"HotkeyKey":86,"MaxHistory":500,"Theme":"Dark","OpenAt":"Mouse","PlainTextApps":["code"],"PopupLeft":null,"HotkeyText":"x"}"#,
        )
        .unwrap();
        assert_eq!((s.max_history, s.theme, s.open_at, s.link_icons), (500, Theme::Dark, OpenAt::Mouse, true));
        assert_eq!(s.hotkey_text(), "Ctrl + Shift + V");
        assert!(serde_json::to_string(&s).unwrap().contains(r#""PlainTextApps":["code"]"#));
    }
}
