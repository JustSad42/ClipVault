//! The item and ⋯ menus (native, so they cost nothing until opened), and dragging an item out.
//! Both run their own message loop: only call them from a task, never while the state is borrowed.

use crate::clip::{self, Formats};
use crate::model::{self, Card, Facts, Item as ClipItem, Kind};
use crate::textops::Transform;
use crate::win::wide;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::SHCreateDataObject;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::{BOOL, HRESULT, PCSTR, PCWSTR, implement, w};

/// Choices for one item.
#[derive(Clone, Copy)]
pub enum Item {
    Paste,
    PastePlain,
    Transform(Transform),
    Copy,
    Open,
    /// 0 hex, 1 rgb, 2 hsl.
    ColorAs(u8),
    OpenQr,
    CopyQr,
    CopyImageText,
    SaveFile,
    Preview,
    Edit,
    Pin,
    Delete,
}

/// Choices from the ⋯ button.
#[derive(Clone, Copy)]
pub enum More {
    Update,
    Compact,
    PlainHere,
    Reset,
    Settings,
    Clear,
    Folder,
    Quit,
}

/// A menu being built; each line's id is its place in `choices`, plus one.
struct Menu<T> {
    handle: HMENU,
    choices: Vec<T>,
}

impl<T: Copy> Menu<T> {
    fn new() -> Menu<T> {
        Menu { handle: unsafe { CreatePopupMenu() }.unwrap_or_default(), choices: vec![] }
    }

    fn line(&mut self, to: HMENU, label: &str, flags: MENU_ITEM_FLAGS, choice: T) {
        self.choices.push(choice);
        // "&" would underline the next letter
        let label = wide(&label.replace('&', "&&"));
        unsafe {
            let _ = AppendMenuW(to, MF_STRING | flags, self.choices.len(), PCWSTR(label.as_ptr()));
        }
    }

    fn add(&mut self, label: &str, choice: T) {
        self.line(self.handle, label, MF_ENABLED, choice);
    }

    fn separator(&mut self) {
        unsafe {
            let _ = AppendMenuW(self.handle, MF_SEPARATOR, 0, None);
        }
    }

    fn submenu(&mut self, label: &str, enabled: bool, lines: &[(&str, T)]) {
        let Ok(sub) = (unsafe { CreatePopupMenu() }) else { return };
        for (text, choice) in lines {
            self.line(sub, text, MF_ENABLED, *choice);
        }
        let label = wide(label);
        let state = if enabled { MF_ENABLED } else { MF_GRAYED };
        unsafe {
            // the parent menu destroys its submenus
            let _ = AppendMenuW(self.handle, MF_POPUP | state, sub.0 as usize, PCWSTR(label.as_ptr()));
        }
    }

    /// Shows the menu at the mouse and waits for a choice.
    fn run(self, owner: HWND, dark: bool) -> Option<T> {
        set_menu_theme(dark);
        let mut at = POINT::default();
        let id = unsafe {
            let _ = GetCursorPos(&mut at);
            let id = TrackPopupMenuEx(self.handle, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.x, at.y, owner, None);
            let _ = DestroyMenu(self.handle);
            id.0 as usize
        };
        self.choices.get(id.checked_sub(1)?).copied()
    }
}

/// Dark or light menus. Windows has no public call for this: uxtheme's ordinals 135 and 136 are
/// what Explorer and others use (Windows 10 1903 and newer).
fn set_menu_theme(dark: bool) {
    unsafe {
        let Ok(uxtheme) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        let ordinal = |n: usize| GetProcAddress(uxtheme, PCSTR(n as *const u8));
        if let Some(set) = ordinal(135) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set);
            set(if dark { 2 } else { 3 }); // force dark, force light
        }
        if let Some(flush) = ordinal(136) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            flush();
        }
    }
}

pub fn item(owner: HWND, item: &ClipItem, facts: &Facts, dark: bool) -> Option<Item> {
    let mut m = Menu::new();
    let text_like = item.kind != Kind::Image;
    let grey = |on: bool| if on { MF_ENABLED } else { MF_GRAYED };
    m.add("Paste\tEnter", Item::Paste);
    m.line(m.handle, "Paste as plain text\tShift+Enter", grey(text_like), Item::PastePlain);
    m.submenu("Paste as", text_like, &[
        ("UPPERCASE", Item::Transform(Transform::Upper)),
        ("lowercase", Item::Transform(Transform::Lower)),
        ("Title Case", Item::Transform(Transform::Title)),
        ("Trimmed spaces", Item::Transform(Transform::Trim)),
        ("Single line", Item::Transform(Transform::SingleLine)),
    ]);
    m.add("Copy only\tCtrl+Enter", Item::Copy);
    m.separator();
    match facts.card {
        Card::Link => m.add("Open link\tCtrl+O", Item::Open),
        Card::Files => m.add("Show in File Explorer\tCtrl+O", Item::Open),
        _ => {}
    }
    if let (Card::Color, Some(c)) = (facts.card, item.color()) {
        let (hex, rgb, hsl) = (model::color_hex(c), model::color_rgb(c), model::color_hsl(c));
        m.submenu("Copy color as", true, &[(&hex, Item::ColorAs(0)), (&rgb, Item::ColorAs(1)), (&hsl, Item::ColorAs(2))]);
    }
    if item.qr_host().is_some() {
        m.add("Open QR code link", Item::OpenQr);
    }
    if item.has_qr() {
        m.add("Copy QR code text", Item::CopyQr);
    }
    if item.kind == Kind::Image {
        m.line(m.handle, "Copy text from image", grey(item.has_ocr_text()), Item::CopyImageText);
    }
    if item.kind != Kind::Files {
        m.add("Save to file…\tCtrl+S", Item::SaveFile);
    }
    m.separator();
    m.add("Preview\tSpace", Item::Preview);
    m.line(m.handle, "Edit…\tF2", grey(item.kind != Kind::Files), Item::Edit);
    m.add("Pin / unpin\tCtrl+P", Item::Pin);
    m.separator();
    m.add("Delete\tDel", Item::Delete);
    m.run(owner, dark)
}

/// `ready`: a downloaded update's version. `target`: the app you came from, and whether it
/// always gets plain text.
pub fn more(owner: HWND, ready: Option<String>, compact: bool, target: Option<(String, bool)>, dark: bool) -> Option<More> {
    let mut m = Menu::new();
    let tick = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    if let Some(version) = ready {
        m.add(&format!("Restart to update to {version}"), More::Update);
        unsafe {
            let _ = SetMenuDefaultItem(m.handle, 0, 1); // shown bold
        }
        m.separator();
    }
    m.line(m.handle, "Compact list", tick(compact), More::Compact);
    if let Some((name, plain)) = target {
        m.line(m.handle, &format!("Always paste plain text into {name}"), tick(plain), More::PlainHere);
    }
    m.add("Reset popup size and position", More::Reset);
    m.separator();
    m.add("Settings…", More::Settings);
    m.add("Clear history (keeps pinned)", More::Clear);
    m.add("Open data folder", More::Folder);
    m.separator();
    m.add("Quit ClipVault", More::Quit);
    m.run(owner, dark)
}

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, esc: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if esc.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// The item as other programs receive it: the shell's data object, filled with our formats.
fn data_object(formats: &Formats) -> Option<IDataObject> {
    unsafe {
        let data: IDataObject = SHCreateDataObject(None, None, None::<&IDataObject>).ok()?;
        for (format, bytes) in &formats.0 {
            let Some(g) = clip::global(bytes) else { continue };
            let how = FORMATETC { cfFormat: *format as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
            let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: g }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
            // true: the object now owns the memory
            if data.SetData(&how, &medium, true).is_err() {
                let _ = GlobalFree(Some(g));
            }
        }
        Some(data)
    }
}

/// Runs a drag of the item. Returns true if it was dropped somewhere.
pub fn drag(formats: &Formats) -> bool {
    let Some(data) = data_object(formats) else { return false };
    let source: IDropSource = Source.into();
    let mut effect = DROPEFFECT(0);
    let result = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect) };
    result == DRAGDROP_S_DROP && effect.0 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What we offer for a drag can be read back by the program receiving it.
    #[test]
    fn drag_data_holds_the_formats() {
        unsafe {
            let _ = OleInitialize(None);
        }
        let formats = clip::text_formats("héllo");
        let data = data_object(&formats).expect("data object");
        let how = FORMATETC { cfFormat: clip::CF_UNICODETEXT as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let mut medium = unsafe { data.GetData(&how) }.expect("text is offered");
        unsafe {
            let p = windows::Win32::System::Memory::GlobalLock(medium.u.hGlobal) as *const u16;
            let got: Vec<u16> = std::slice::from_raw_parts(p, 6).to_vec();
            assert_eq!(String::from_utf16_lossy(&got), "héllo\0");
            let _ = windows::Win32::System::Memory::GlobalUnlock(medium.u.hGlobal);
            ReleaseStgMedium(&mut medium);
        }
    }
}
