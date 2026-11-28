//! The running app: the hidden message window (shortcut, clipboard changes, timers), background
//! work, and the shared state every window uses.
//!
//! The state lives in one `RefCell`. Nothing that runs its own message loop (menus, dialogs,
//! drag-and-drop) is ever called while it is borrowed: handlers queue a `Task` instead.

use crate::cache::Cache;
use crate::images::Bitmap;
use crate::settings::{self, Settings};
use crate::store::{self, SavedImage, Source, Store};
use crate::theme::Theme;
use crate::win::{self, wide};
use crate::{clip, net, overlay, popup, settings_ui, system, textbox, vision};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

pub enum Event {
    UpdateStatus,
    /// A copied picture was saved to disk.
    ImageSaved(Box<SavedImage>, Source),
    /// A copied picture turned out to be in the history already.
    ImageKnown(String),
    /// A picture was read: (id, file, text, QR text).
    Read(String, String, Option<String>, Option<String>),
    Favicon(String, Option<Bitmap>),
    /// A second copy was started: show the window.
    Show,
}

static QUEUE: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
static HOST: AtomicIsize = AtomicIsize::new(0);
const WM_EVENT: u32 = WM_APP;
/// Run the queued tasks (menus, dialogs) outside any borrow.
const WM_TASKS: u32 = WM_APP + 1;

const TIMER_SAVE: usize = 1;
const TIMER_HIDDEN: usize = 2;
const TIMER_UPDATE: usize = 3;
const TIMER_RETRY_EVENTS: usize = 4;
const TIMER_RETRY_CAPTURE: usize = 5;

pub fn post(event: Event) {
    QUEUE.lock().unwrap().push_back(event);
    let host = HOST.load(Ordering::SeqCst);
    if host != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(host as _)), WM_EVENT, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn host() -> HWND {
    HWND(HOST.load(Ordering::SeqCst) as _)
}

/// Something that must run outside a borrow of the state.
pub type Task = Box<dyn FnOnce()>;

pub struct App {
    pub settings: Settings,
    pub store: Store,
    pub cache: Cache,
    pub theme: Theme,
    /// Jobs for the picture-reading thread: (id, file, path, needs text, needs QR).
    reader: Sender<(String, String, std::path::PathBuf, bool, bool)>,
    reading: bool,
    /// Our own last clipboard write, so it isn't captured again.
    own_sequence: u32,
    seen_version: u64,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static POPUP: RefCell<Option<popup::Popup>> = const { RefCell::new(None) };
    static TASKS: RefCell<VecDeque<Task>> = const { RefCell::new(VecDeque::new()) };
}

/// The shared state. `None` if it's already in use further up the stack (or not made yet).
pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

/// The popup together with the shared state.
pub fn with_popup<R>(f: impl FnOnce(&mut popup::Popup, &mut App) -> R) -> Option<R> {
    POPUP.with(|p| p.try_borrow_mut().ok().and_then(|mut p| p.as_mut().and_then(|p| with(|a| f(p, a)))))
}

pub fn run_later(task: impl FnOnce() + 'static) {
    TASKS.with_borrow_mut(|t| t.push_back(Box::new(task)));
    unsafe {
        let _ = PostMessageW(Some(host()), WM_TASKS, WPARAM(0), LPARAM(0));
    }
}

/// Clipboard access for text fields.
pub struct FieldClip(pub HWND);

impl textbox::Clip for FieldClip {
    fn get(&mut self) -> Option<String> {
        clip::read_text(self.0)
    }
    fn set(&mut self, text: &str) {
        // not marked as ours: text copied in our fields goes into the history like any other copy
        clip::write(self.0, &clip::text_formats(text));
    }
}

impl App {
    /// Call after anything changes the history: saves soon, and reads new pictures.
    pub fn changed(&mut self) {
        if self.store.version == self.seen_version {
            return;
        }
        self.seen_version = self.store.version;
        unsafe {
            SetTimer(Some(host()), TIMER_SAVE, 600, None);
        }
        self.read_next();
        self.cache.prune(&self.store);
        run_later(|| {
            with_popup(|p, a| p.history_changed(a));
        });
    }

    /// Hands the next unread picture to the reading thread.
    fn read_next(&mut self) {
        if self.reading {
            return;
        }
        let Some((id, file, ocr, qr)) = self.store.unread_image() else { return };
        let path = self.store.images().join(&file);
        self.reading = self.reader.send((id, file, path, ocr, qr)).is_ok();
    }

    pub fn put_on_clipboard(&mut self, formats: &clip::Formats) {
        if let Some(sequence) = clip::write(host(), formats) {
            self.own_sequence = sequence;
        }
    }

    pub fn register_hotkey(&self) -> bool {
        register_hotkey(self.settings.hotkey_modifiers, self.settings.hotkey_key)
    }

    /// Saves new settings and applies them everywhere.
    pub fn apply_settings(&mut self, s: Settings) {
        self.settings = s;
        self.settings.save();
        self.store.max_unpinned = self.settings.max_history;
        self.store.trim();
        self.theme = Theme::current(&self.settings);
        self.changed();
    }

    fn capture(&mut self) {
        if clip::sequence() == self.own_sequence {
            return;
        }
        let source = system::copy_source();
        let Some(content) = clip::read(host()) else { return };
        match content {
            clip::Content::Files(files) => self.store.add_files(&files, source),
            clip::Content::Text { text, html, rtf } => {
                // a copied link with tracking junk: keep the clean version, on the clipboard too
                match self.settings.clean_links.then(|| crate::textops::clean_link(&text)).flatten() {
                    Some(clean) => {
                        self.store.add_text(clean.clone(), None, None, source, true);
                        self.put_on_clipboard(&clip::text_formats(&clean));
                    }
                    None => self.store.add_text(text, html, rtf, source, false),
                }
            }
            clip::Content::Image(bytes) => save_image(bytes, source, &self.store),
        }
        if self.settings.sounds {
            system::play(false);
        }
        self.changed();
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::ImageSaved(saved, source) => self.store.add_image(*saved, source),
            Event::ImageKnown(hash) => {
                self.store.bump_image(&hash);
            }
            Event::Read(id, file, ocr, qr) => {
                self.reading = false;
                self.store.set_read(&id, &file, ocr, qr);
                self.read_next();
            }
            Event::Favicon(host, icon) => self.cache.favicon_arrived(&host, icon),
            Event::UpdateStatus | Event::Show => {}
        }
        self.changed();
    }

    pub fn save(&mut self) {
        if self.store.dirty.is_some() {
            self.store.save();
        }
    }
}

/// Hashing and PNG encoding take a few hundred milliseconds for a 4K screenshot: off the UI thread.
fn save_image(bytes: Vec<u8>, source: Source, store: &Store) {
    let images = store.images();
    let known: std::collections::HashSet<String> = store.items.iter().filter(|i| i.kind == crate::model::Kind::Image).map(|i| i.hash.clone()).collect();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let Ok(bitmap) = Bitmap::decode(&bytes, None) else { return };
        drop(bytes);
        let hash = store::image_hash(&bitmap);
        if known.contains(&hash) {
            return post(Event::ImageKnown(hash));
        }
        match store::write_image(&images, &store::new_id(), &bitmap) {
            Ok(saved) => post(Event::ImageSaved(Box::new(saved), source)),
            Err(e) => win::log(&format!("a copied picture couldn't be saved: {e}")),
        }
    });
}

fn reader_thread() -> Sender<(String, String, std::path::PathBuf, bool, bool)> {
    let (send, receive) = channel::<(String, String, std::path::PathBuf, bool, bool)>();
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        for (id, file, path, ocr, qr) in receive {
            let bitmap = Bitmap::load(&path, None).ok();
            // an unreadable picture counts as read, or it would be tried forever
            let text = ocr.then(|| bitmap.as_ref().map(vision::read_text).unwrap_or_default());
            let code = qr.then(|| bitmap.as_ref().map(vision::read_qr).unwrap_or_default());
            post(Event::Read(id, file, text, code));
        }
    });
    send
}

pub fn register_hotkey(modifiers: u32, vk: u32) -> bool {
    unsafe {
        let _ = UnregisterHotKey(Some(host()), 1);
        RegisterHotKey(Some(host()), 1, HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT, vk).is_ok()
    }
}

pub fn suspend_hotkey() {
    unsafe {
        let _ = UnregisterHotKey(Some(host()), 1);
    }
}

pub fn quit() {
    unsafe {
        let _ = DestroyWindow(host());
    }
}

/// A few seconds after the popup closes: install a downloaded update while nothing is on screen,
/// and delete the files of removed items.
pub fn popup_hidden() {
    unsafe {
        SetTimer(Some(host()), TIMER_HIDDEN, 3000, None);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wp.0 == 1 => {
            let front = unsafe { GetForegroundWindow() };
            with_popup(|p, a| p.toggle(a, front));
        }
        windows::Win32::UI::WindowsAndMessaging::WM_CLIPBOARDUPDATE => {
            if with(App::capture).is_none() {
                unsafe {
                    SetTimer(Some(h), TIMER_RETRY_CAPTURE, 30, None);
                }
            }
        }
        WM_EVENT => {
            loop {
                // while the state is busy (a call that pumps messages), events wait their turn
                if with(|_| ()).is_none() {
                    unsafe {
                        SetTimer(Some(h), TIMER_RETRY_EVENTS, 30, None);
                    }
                    break;
                }
                let next = QUEUE.lock().unwrap().pop_front();
                let Some(event) = next else { break };
                match event {
                    Event::Show => {
                        with_popup(|p, a| p.show_centred(a));
                    }
                    Event::UpdateStatus => settings_ui::repaint(),
                    other => {
                        with(|a| a.event(other));
                    }
                }
            }
        }
        WM_TASKS => loop {
            let Some(task) = TASKS.with_borrow_mut(|t| t.pop_front()) else { break };
            task();
        },
        WM_TIMER => match wp.0 {
            TIMER_SAVE => {
                unsafe {
                    let _ = KillTimer(Some(h), TIMER_SAVE);
                }
                with(App::save);
            }
            TIMER_HIDDEN => {
                unsafe {
                    let _ = KillTimer(Some(h), TIMER_HIDDEN);
                }
                if !popup::visible() && !settings_ui::open_now() {
                    with(|a| {
                        a.store.purge_deleted(Duration::from_secs(10));
                        a.save();
                    });
                    if net::apply_and_restart() {
                        quit();
                    }
                }
            }
            TIMER_RETRY_EVENTS | TIMER_RETRY_CAPTURE => {
                unsafe {
                    let _ = KillTimer(Some(h), wp.0);
                    let message = if wp.0 == TIMER_RETRY_EVENTS { WM_EVENT } else { windows::Win32::UI::WindowsAndMessaging::WM_CLIPBOARDUPDATE };
                    let _ = PostMessageW(Some(h), message, WPARAM(0), LPARAM(0));
                }
            }
            TIMER_UPDATE => {
                // shortly after starting, then every 6 hours
                unsafe {
                    SetTimer(Some(h), TIMER_UPDATE, 6 * 3600 * 1000, None);
                }
                if with(|a| a.settings.auto_update) == Some(true) {
                    net::check_in_background(false);
                    popup_hidden();
                }
            }
            _ => {}
        },
        WM_SETTINGCHANGE => {
            // theme or accent changed in Windows
            with(|a| a.theme = Theme::current(&a.settings));
            popup::repaint();
            settings_ui::repaint();
        }
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION => {
            with(App::save);
        }
        WM_DESTROY => unsafe {
            PostQuitMessage(0);
        },
        _ => return unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
    LRESULT(0)
}

fn create_host() -> HWND {
    let class = wide("ClipVaultMessages");
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.unwrap_or_default(), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
        RegisterClassW(&wc);
        // top-level (not message-only), so it also hears about theme changes and shutdown
        CreateWindowExW(WS_EX_TOOLWINDOW, PCWSTR(class.as_ptr()), PCWSTR(class.as_ptr()), WS_POPUP, 0, 0, 0, 0, None, None, instance, None).unwrap_or_default()
    }
}

/// The names that make sure only one copy runs. The C# version used the same ones, so the two
/// never run side by side.
fn names() -> (Vec<u16>, Vec<u16>) {
    let suffix = if settings::is_test() { ".Test" } else { "" };
    (wide(&format!(r"Local\ClipVault.SingleInstance{suffix}")), wide(&format!(r"Local\ClipVault.Show{suffix}")))
}

/// The first copy gets the mutex (held for its lifetime); a later one asks it to show and gets `None`.
fn single_instance() -> Option<HANDLE> {
    let (mutex, show) = names();
    unsafe {
        let held = CreateMutexW(None, true, PCWSTR(mutex.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(ASFW_ANY);
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(show.as_ptr())) {
                let _ = SetEvent(event);
            }
            return None;
        }
        if let Ok(event) = CreateEventW(None, false, false, PCWSTR(show.as_ptr())) {
            let event = event.0 as isize;
            std::thread::spawn(move || {
                while WaitForSingleObject(HANDLE(event as _), INFINITE) == WAIT_OBJECT_0 {
                    post(Event::Show);
                }
            });
        }
        Some(held)
    }
}

pub fn run(args: &[String]) {
    // after an update, the new version waits for the old one to finish closing
    if let Some(pid) = args.iter().skip_while(|a| *a != "--wait-for").nth(1).and_then(|p| p.parse::<u32>().ok()) {
        unsafe {
            if let Ok(old) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                WaitForSingleObject(old, 15_000);
                let _ = CloseHandle(old);
            }
        }
    }
    unsafe {
        let _ = OleInitialize(None);
    }
    let Some(_instance) = single_instance() else { return };
    let window = create_host();
    HOST.store(window.0 as isize, Ordering::SeqCst);
    net::clean_up();

    let settings = Settings::load();
    keep_old_history();
    let store = Store::load(settings::dir(), settings.max_history);
    let theme = Theme::current(&settings);
    let app = App { reader: reader_thread(), reading: false, own_sequence: 0, seen_version: store.version, cache: Cache::default(), settings, store, theme };
    APP.with_borrow_mut(|a| *a = Some(app));
    let popup = with(|a| popup::Popup::new(a));
    POPUP.with_borrow_mut(|p| *p = popup);
    with(App::read_next);

    unsafe {
        let _ = AddClipboardFormatListener(window);
        SetTimer(Some(window), TIMER_UPDATE, 30_000, None);
    }
    if with(|a| a.register_hotkey()) != Some(true) {
        let text = with(|a| a.settings.hotkey_text()).unwrap_or_default();
        system::warn(None, &format!("{text} is already used by another app, so the shortcut won't work.\n\nClipVault will still record your clipboard. Start ClipVault again to open it, then pick a different shortcut in Settings (the ⋯ button)."));
    }
    if args.iter().any(|a| a == "--updated") {
        overlay::toast(&format!("ClipVault updated to {}", net::VERSION), None, None);
    }
    if !args.iter().any(|a| a == "--background") {
        with_popup(|p, a| p.show_centred(a));
    }

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = RemoveClipboardFormatListener(window);
        let _ = UnregisterHotKey(Some(window), 1);
    }
    with(|a| {
        a.save();
        a.store.purge_deleted(Duration::ZERO);
    });
}

/// Before the native version first touches the history, keeps a copy of what the old one wrote.
fn keep_old_history() {
    let dir = settings::dir();
    let (from, to) = (dir.join("history.json"), dir.join("history.before-2.0.json"));
    if from.exists() && !to.exists() {
        let _ = std::fs::copy(from, to);
    }
}

/// Draws every screen, from made-up items, into PNG files in %CV_SNAP% to be looked at:
/// `cargo test snapshots -- --ignored`
#[cfg(test)]
mod snapshots {
    use super::*;
    use crate::gfx::Surface;
    use crate::images::Bitmap;
    use crate::popup::{Hit, Mode, Popup, Tab};

    fn save(s: &Surface, path: std::path::PathBuf, dark: bool) {
        // the glass fill is see-through: put it over a plain backdrop
        let under: u32 = if dark { 0x30 } else { 0xD0 };
        let px = s.pixels().chunks_exact(4).flat_map(|p| {
            let over = |c: u8| (u32::from(c) + under * (255 - u32::from(p[3])) / 255).min(255) as u8;
            [over(p[0]), over(p[1]), over(p[2]), 255]
        });
        Bitmap { w: s.w as u32, h: s.h as u32, px: px.collect() }.save(&path).unwrap();
    }

    fn sample(store: &mut Store) {
        let src = |name: &str| Source { name: Some(name.into()), path: Some(r"C:\Windows\explorer.exe".into()) };
        store.add_text("hello@example.com".into(), None, None, src("Outlook"), false);
        store.add_text("#3D5AFE".into(), None, None, src("Figma"), false);
        store.add_files(&[r"C:\Windows\win.ini".into(), r"C:\Windows\explorer.exe".into()], src("File Explorer"));
        let (w, h) = (640u32, 360u32);
        let px = (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x / 3) as u8, (y / 2) as u8, 180, 255])).collect();
        let picture = Bitmap { w, h, px };
        std::fs::create_dir_all(store.images()).unwrap();
        let saved = store::write_image(&store.images(), &store::new_id(), &picture).unwrap();
        store.add_image(saved, src("Snipping Tool"));
        store.add_text("https://github.com/JustSad42/ClipVault/releases".into(), None, None, src("Chrome"), true);
        store.add_text("fn main() {\n    let total: u32 = (1..=10).sum();\n    println!(\"{total}\");\n}".into(), None, None, src("Code"), false);
        store.add_text("Meeting moved to Thursday at 3 pm.\nBring the printed schedule and the notes from last week, we'll go through them together.".into(), Some("<b>x</b>".into()), None, src("Discord"), false);
        store.add_text("git push origin main".into(), None, None, src("Windows Terminal"), false);
        let id = store.items.iter().find(|i| i.text.as_deref() == Some("#3D5AFE")).unwrap().id.clone();
        store.set_pinned(&id, true);
    }

    #[test]
    #[ignore]
    fn snapshots() {
        let out = std::path::PathBuf::from(std::env::var("CV_SNAP").expect("CV_SNAP"));
        let data = out.join("data");
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        // this test is run on its own
        unsafe {
            std::env::set_var("CLIPVAULT_DATA", &data);
            let _ = OleInitialize(None);
        }
        for (theme, tag) in [(settings::Theme::Dark, "dark"), (settings::Theme::Light, "light")] {
            let mut s = Settings::load();
            s.theme = theme;
            s.link_icons = false;
            let mut store = Store::load(data.join(tag), s.max_history);
            sample(&mut store);
            let theme = Theme::current(&s);
            let mut a = App { reader: reader_thread(), reading: false, own_sequence: 0, seen_version: store.version, cache: Cache::default(), settings: s, store, theme };
            let mut p = Popup::new(&mut a);
            let shot = |p: &mut Popup, a: &mut App, name: &str| {
                p.sync_preview(a);
                p.paint(a);
                // a second pass: hover positions come from the first
                p.paint(a);
                save(p.surface.as_ref().unwrap(), out.join(format!("{tag}-{name}.png")), theme.dark);
            };
            for scale in [1.0, 1.5] {
                p.scale = scale;
                p.rebuild(&mut a, None);
                p.hover = Some(Hit::Row(1));
                shot(&mut p, &mut a, &format!("list-{scale}"));
            }
            p.scale = 1.0;
            p.hover = None;
            p.search.set("the");
            p.query = crate::search::parse("the");
            p.rebuild(&mut a, None);
            shot(&mut p, &mut a, "search");
            p.search.set("zzzz");
            p.query = crate::search::parse("zzzz");
            p.rebuild(&mut a, None);
            shot(&mut p, &mut a, "nomatch");
            p.search.set("");
            p.query = Default::default();
            p.tab = Tab::Images;
            p.rebuild(&mut a, None);
            shot(&mut p, &mut a, "grid");
            p.mode = Mode::Preview;
            shot(&mut p, &mut a, "preview-image");
            p.mode = Mode::List;
            p.tab = Tab::All;
            a.settings.compact = true;
            p.rebuild(&mut a, None);
            shot(&mut p, &mut a, "compact");
            a.settings.compact = false;
            p.rebuild(&mut a, None);
            p.selected = 1;
            p.mode = Mode::Preview;
            shot(&mut p, &mut a, "preview-text");
            p.mode = Mode::Edit;
            p.editor.set("An edited note\nwith a second line");
            shot(&mut p, &mut a, "edit");
            let id = a.store.items.iter().find(|i| i.kind == crate::model::Kind::Image).unwrap().id.clone();
            p.reset_mode();
            p.open_editor(&mut a, &id);
            if let Some(e) = &mut p.image {
                e.drag = Some(((120.0, 190.0), (260.0, 250.0)));
            }
            shot(&mut p, &mut a, "image-editor");
            for page in 0..4 {
                save(&crate::settings_ui::snapshot(a.settings.clone(), page, 1.0), out.join(format!("{tag}-settings-{page}.png")), theme.dark);
            }
            unsafe {
                let _ = DestroyWindow(p.hwnd);
            }
        }
    }
}
