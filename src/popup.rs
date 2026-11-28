//! The clipboard window: its state and what the keyboard and mouse do. Drawing is in `paint`.

use crate::app::{self, App, FieldClip, run_later, with, with_popup};
use crate::gfx::{R, Surface};
use crate::images::{Area, Bitmap};
use crate::model::{Card, Item, Kind};
use crate::search::{self, Query};
use crate::settings::{self, OpenAt};
use crate::textbox::{Did, TextBox};
use crate::textops;
use crate::win::wide;
use crate::{clip, menu, overlay, paint, settings_ui, system};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

static WINDOW: AtomicIsize = AtomicIsize::new(0);

const TIMER_CARET: usize = 1;
const TIMER_ANIMATE: usize = 2;
const TIMER_TOOLTIP: usize = 3;
const TIMER_FOCUS: usize = 4;

pub fn handle() -> HWND {
    HWND(WINDOW.load(Ordering::SeqCst) as _)
}

/// The window's own flag: `IsWindowVisible` also asks about the desktop, which is never visible
/// where the automated test runs.
pub fn visible() -> bool {
    unsafe { GetWindowLongW(handle(), GWL_STYLE) as u32 & WS_VISIBLE.0 != 0 }
}

pub fn repaint() {
    unsafe {
        let _ = InvalidateRect(Some(handle()), None, false);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    All,
    Text,
    Images,
    Files,
    Pinned,
}

pub const TABS: [(Tab, &str); 5] = [(Tab::All, "All"), (Tab::Text, "Text"), (Tab::Images, "Images"), (Tab::Files, "Files"), (Tab::Pinned, "Pinned")];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    List,
    Preview,
    Edit,
    Image,
}

/// Something that can be pointed at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Search,
    Clear,
    Tab(usize),
    Row(usize),
    Action(usize),
    Pin(usize),
    More,
    /// Empty space that moves the window.
    Drag,
    Scrollbar,
    Preview,
    Editor,
    Cancel,
    Save,
    SavePaste,
    ToolCrop,
    ToolBlur,
    Rotate,
    UndoImage,
    Canvas,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PasteMode {
    Normal,
    Plain,
    CopyOnly,
}

/// A block in the list, in list coordinates (before scrolling).
pub enum Row {
    Heading(&'static str, f32),
    Item { index: usize, area: R },
}

impl Row {
    pub fn top(&self) -> f32 {
        match self {
            Row::Heading(_, y) => *y,
            Row::Item { area, .. } => area.y,
        }
    }
}

pub struct ImageEditor {
    pub id: String,
    pub current: Bitmap,
    history: Vec<Bitmap>,
    pub blur: bool,
    /// The dragged selection, in screen design units.
    pub drag: Option<((f32, f32), (f32, f32))>,
    /// Where the picture was last drawn.
    pub shown: R,
    /// Bumped on every change, to redraw the prepared picture.
    pub version: u32,
    pub prepared: Option<(u32, std::rc::Rc<crate::gfx::Image>)>,
}

impl ImageEditor {
    pub fn edited(&self) -> bool {
        !self.history.is_empty()
    }

    fn apply(&mut self, next: Bitmap) {
        self.history.push(std::mem::replace(&mut self.current, next));
        if self.history.len() > 20 {
            self.history.remove(0);
        }
        self.version += 1;
    }

    fn undo(&mut self) {
        if let Some(b) = self.history.pop() {
            self.current = b;
            self.version += 1;
        }
    }
}

pub struct Popup {
    pub hwnd: HWND,
    pub surface: Option<Surface>,
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    pub search: TextBox,
    pub tab: Tab,
    pub query: Query,
    pub rows: Vec<Row>,
    /// Item ids in the order shown.
    pub items: Vec<String>,
    pub content_height: f32,
    /// Where the list was last drawn.
    pub list: R,
    pub selected: usize,
    pub scroll: f32,
    scroll_to: f32,
    pub mode: Mode,
    pub editor: TextBox,
    editing: Option<String>,
    pub preview: TextBox,
    preview_of: Option<(String, u64)>,
    pub image: Option<ImageEditor>,
    pub hits: Vec<(R, Hit)>,
    pub hover: Option<Hit>,
    mouse: (f32, f32),
    pressed: Option<(Hit, f32, f32)>,
    /// A click count for text fields: (when, where, how many).
    clicks: (Instant, (f32, f32), u32),
    scrollbar_grab: Option<f32>,
    target: HWND,
    /// A menu, dialog or drag of ours is up: losing focus doesn't close the window.
    keep_open: bool,
    opened: Instant,
    moved: bool,
    pub caret_on: bool,
    deleted: Vec<String>,
    /// Opening animation: when it began and whether the window is above the anchor.
    pub appearing: Option<(Instant, bool)>,
    swallow_char: bool,
    tracking_mouse: bool,
}

impl Popup {
    pub fn new(app: &mut App) -> Popup {
        let hwnd = create_window();
        let dark: windows_core::BOOL = app.theme.dark.into();
        style_window(hwnd, dark);
        Popup {
            hwnd,
            surface: None,
            scale: 1.0,
            width: app.settings.popup_width as f32,
            height: app.settings.popup_height as f32,
            search: TextBox::new(false),
            tab: Tab::All,
            query: Query::default(),
            rows: vec![],
            items: vec![],
            content_height: 0.0,
            list: R::default(),
            selected: 0,
            scroll: 0.0,
            scroll_to: 0.0,
            mode: Mode::List,
            editor: TextBox::new(true),
            editing: None,
            preview: TextBox::new(true),
            preview_of: None,
            image: None,
            hits: vec![],
            hover: None,
            mouse: (-1.0, -1.0),
            pressed: None,
            clicks: (Instant::now(), (0.0, 0.0), 0),
            scrollbar_grab: None,
            target: HWND::default(),
            keep_open: false,
            opened: Instant::now(),
            moved: false,
            caret_on: true,
            deleted: vec![],
            appearing: None,
            swallow_char: false,
            tracking_mouse: false,
        }
    }

    // ---------- showing and hiding ----------

    /// The shortcut: open near the text cursor, or close if already open.
    pub fn toggle(&mut self, app: &mut App, front: HWND) {
        if visible() {
            return self.dismiss();
        }
        self.target = if front == self.hwnd { HWND::default() } else { front };
        self.open(app, false);
    }

    /// A manual start: in the middle of the screen; choosing copies but doesn't paste.
    pub fn show_centred(&mut self, app: &mut App) {
        if visible() {
            return system::force_foreground(self.hwnd);
        }
        self.target = HWND::default();
        self.open(app, true);
    }

    fn open(&mut self, app: &mut App, centre: bool) {
        app.theme = crate::theme::Theme::current(&app.settings);
        style_window(self.hwnd, app.theme.dark.into());
        self.reset_mode();
        self.search.set("");
        self.query = Query::default();
        self.tab = Tab::All;
        self.hover = None;
        self.pressed = None;
        self.moved = false;
        self.caret_on = true;
        let above = self.place(app, centre);
        self.rebuild(app, None);
        self.scroll = 0.0;
        self.scroll_to = 0.0;
        self.opened = Instant::now();
        self.appearing = Some((Instant::now(), above));
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            SetTimer(Some(self.hwnd), TIMER_ANIMATE, 10, None);
            SetTimer(Some(self.hwnd), TIMER_CARET, 530, None);
        }
        let hwnd = self.hwnd;
        run_later(move || {
            unsafe {
                let _ = UpdateWindow(hwnd); // draw the first frame now, with nothing borrowed
            }
            system::force_foreground(hwnd);
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
        });
    }

    /// Sizes and positions the window. Returns true when it opens above the anchor.
    fn place(&mut self, app: &mut App, centre: bool) -> bool {
        let s = &app.settings;
        let (mut width, mut height) = (s.popup_width.clamp(360.0, 1100.0) as f32, s.popup_height.clamp(400.0, 1500.0) as f32);
        let mut mouse = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut mouse);
        }
        let caret = (!centre && s.open_at == OpenAt::TextCursor).then(|| system::caret_rect(self.target)).flatten();
        let last = (!centre && s.open_at == OpenAt::LastPosition).then(|| s.popup_left.zip(s.popup_top)).flatten();
        let anchor = match (&caret, last) {
            (_, Some((x, y))) => POINT { x: x as i32, y: y as i32 },
            (Some(c), _) => POINT { x: c.left, y: c.bottom },
            _ => mouse,
        };
        let (work, scale) = system::monitor_at(anchor);
        self.scale = scale;
        width = width.min((work.right - work.left) as f32 / scale);
        height = height.min((work.bottom - work.top) as f32 / scale);
        self.width = width;
        self.height = height;
        let (w, h) = ((width * scale) as i32, (height * scale) as i32);
        let gap = (6.0 * scale) as i32;
        let mut above = false;
        let (x, y) = if last.is_some() {
            (anchor.x, anchor.y)
        } else if centre {
            ((work.left + work.right - w) / 2, (work.top + work.bottom - h) / 2)
        } else if let Some(c) = caret {
            let mut y = c.bottom + gap;
            if y + h > work.bottom {
                y = c.top - gap - h; // no room below: above the line
                above = true;
            }
            (c.left, y)
        } else {
            above = mouse.y + h > work.bottom;
            (mouse.x, mouse.y)
        };
        let x = x.min(work.right - w).max(work.left);
        let y = y.min(work.bottom - h).max(work.top);
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        }
        above
    }

    pub fn close(&mut self, app: &mut App) {
        overlay::hide_tooltip();
        if !visible() {
            return;
        }
        unsafe {
            for t in [TIMER_CARET, TIMER_ANIMATE, TIMER_TOOLTIP, TIMER_FOCUS] {
                let _ = KillTimer(Some(self.hwnd), t);
            }
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.save_placement(app);
        // nothing on screen: drop what only the screen needed
        self.surface = None;
        self.image = None;
        self.preview.set("");
        self.preview_of = None;
        app.cache.rest();
        app::popup_hidden();
    }

    pub fn dismiss(&mut self) {
        let target = self.target;
        run_later(move || {
            with_popup(|p, a| p.close(a));
            if !target.0.is_null() {
                unsafe {
                    let _ = SetForegroundWindow(target);
                }
            }
        });
    }

    /// After a menu: if another window has the focus now, we're done.
    fn close_if_left(&mut self, app: &mut App) {
        if unsafe { GetForegroundWindow() } != self.hwnd {
            self.close(app);
        }
    }

    fn save_placement(&mut self, app: &mut App) {
        let mut s = app.settings.clone();
        s.popup_width = f64::from(self.width.round());
        s.popup_height = f64::from(self.height.round());
        if self.moved {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut r);
            }
            s.popup_left = Some(f64::from(r.left));
            s.popup_top = Some(f64::from(r.top));
        }
        let resized = (s.popup_width - app.settings.popup_width).abs() > 0.5 || (s.popup_height - app.settings.popup_height).abs() > 0.5;
        if resized || self.moved {
            app.settings = s;
            app.settings.save();
        }
        self.moved = false;
    }

    fn reset_placement(&mut self, app: &mut App) {
        let mut s = app.settings.clone();
        (s.popup_width, s.popup_height, s.popup_left, s.popup_top) = (440.0, 580.0, None, None);
        app.settings = s;
        app.settings.save();
        let centre = self.target.0.is_null();
        self.place(app, centre);
        self.moved = false;
        self.rebuild(app, None);
        repaint();
    }

    // ---------- the list ----------

    /// Applies search, tab and order, lays the rows out and keeps a sensible selection.
    pub fn rebuild(&mut self, app: &mut App, keep: Option<String>) {
        let now = chrono::Local::now();
        let searching = !self.query.words.is_empty();
        let mut found: Vec<(u8, &Item)> = Vec::new();
        for item in &app.store.items {
            let ok = match self.tab {
                Tab::All => true,
                Tab::Text => item.kind == Kind::Text,
                Tab::Images => item.kind == Kind::Image,
                Tab::Files => item.kind == Kind::Files,
                Tab::Pinned => item.pinned,
            };
            if !ok {
                continue;
            }
            if searching || !self.query.filters.is_empty() {
                let info = app.cache.info(item, app.store.revision(&item.id));
                if !search::passes(item, info.facts.card, &self.query.filters, now) {
                    continue;
                }
                let rank = info.index.score(&self.query.words);
                if rank > 0 {
                    found.push((rank, item));
                }
            } else {
                found.push((2, item));
            }
        }
        // searching: best matches first; otherwise pinned first; newest first within both
        found.sort_by(|a, b| {
            let first = if searching { b.0.cmp(&a.0) } else { b.1.pinned.cmp(&a.1.pinned) };
            first.then(b.1.copied_at.cmp(&a.1.copied_at))
        });
        let grid = self.tab == Tab::Images;
        let compact = app.settings.compact && !grid;
        let width = paint::list_width(self.width);
        let mut rows = Vec::new();
        let mut items = Vec::new();
        let mut y = 0.0;
        let mut heading = "";
        for (index, (_, item)) in found.iter().enumerate() {
            if !searching && !grid {
                let group = item.group(now);
                if group != heading {
                    heading = group;
                    rows.push(Row::Heading(group, y));
                    y += paint::HEADING;
                }
            }
            let area = if grid {
                let cell = width / 3.0;
                let area = R { x: (index % 3) as f32 * cell, y: (index / 3) as f32 * paint::TILE, w: cell, h: paint::TILE };
                y = area.y + area.h;
                area
            } else {
                let facts = &app.cache.info(item, app.store.revision(&item.id)).facts;
                let h = paint::row_height(item, facts, compact, width);
                let area = R { x: 0.0, y, w: width, h };
                y += h;
                area
            };
            rows.push(Row::Item { index, area });
            items.push(item.id.clone());
        }
        self.rows = rows;
        self.items = items;
        self.content_height = y;
        self.selected = keep.and_then(|id| self.items.iter().position(|i| *i == id)).unwrap_or(0);
        self.ensure_visible(false);
    }

    /// The history changed while we're open (or closed): show it, keeping the selection.
    pub fn history_changed(&mut self, app: &mut App) {
        if !visible() {
            return;
        }
        let keep = self.selected_id();
        let scroll = self.scroll_to;
        self.rebuild(app, keep);
        self.scroll_to = scroll.min(self.max_scroll());
        self.scroll = self.scroll_to;
        if self.mode == Mode::Preview && self.items.is_empty() {
            self.reset_mode();
        }
        repaint();
    }

    pub fn selected_id(&self) -> Option<String> {
        self.items.get(self.selected).cloned()
    }

    fn max_scroll(&self) -> f32 {
        (self.content_height - paint::list_height(self.height)).max(0.0)
    }

    fn ensure_visible(&mut self, smooth: bool) {
        let view = paint::list_height(self.height);
        if let Some((at, area)) = self.rows.iter().enumerate().find_map(|(at, row)| match row {
            Row::Item { index, area } if *index == self.selected => Some((at, *area)),
            _ => None,
        }) {
            // keep the heading above the first row of a group in view too
            let top = match at.checked_sub(1).map(|i| &self.rows[i]) {
                Some(Row::Heading(_, y)) => *y,
                _ => area.y,
            };
            self.scroll_to = self.scroll_to.max(area.y + area.h - view).min(top).clamp(0.0, self.max_scroll());
        } else {
            self.scroll_to = 0.0;
        }
        if smooth {
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_ANIMATE, 10, None);
            }
        } else {
            self.scroll = self.scroll_to;
        }
    }

    fn select(&mut self, index: isize) {
        if self.items.is_empty() {
            return;
        }
        self.selected = index.clamp(0, self.items.len() as isize - 1) as usize;
        self.ensure_visible(true);
    }

    fn search_changed(&mut self, app: &mut App) {
        self.query = search::parse(&self.search.text());
        self.rebuild(app, None);
        self.scroll = 0.0;
        self.scroll_to = 0.0;
    }

    fn set_tab(&mut self, app: &mut App, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.rebuild(app, None);
            self.scroll = 0.0;
            self.scroll_to = 0.0;
        }
    }

    // ---------- preview and editors ----------

    pub fn reset_mode(&mut self) {
        self.mode = Mode::List;
        self.editing = None;
        self.image = None;
        self.preview_of = None;
    }

    /// Keeps the preview showing the selected item.
    pub fn sync_preview(&mut self, app: &mut App) {
        if self.mode != Mode::Preview {
            return;
        }
        let Some(item) = self.selected_id().and_then(|id| app.store.get(&id).cloned()) else { return };
        let key = (item.id.clone(), app.store.revision(&item.id));
        if self.preview_of.as_ref() != Some(&key) {
            let text: String = item.text.as_deref().unwrap_or("").chars().take(100_000).collect();
            self.preview.set(&text);
            self.preview.caret = 0;
            self.preview.anchor = 0;
            self.preview.scroll_y = 0.0;
            self.preview_of = Some(key);
        }
    }

    fn toggle_preview(&mut self) {
        if self.mode == Mode::Preview {
            self.reset_mode();
            self.ensure_visible(false);
        } else if !self.items.is_empty() {
            self.mode = Mode::Preview;
        }
    }

    pub fn open_editor(&mut self, app: &mut App, id: &str) {
        let Some(item) = app.store.get(id).cloned() else { return };
        match item.kind {
            Kind::Text => {
                self.reset_mode();
                self.editor.set(item.text.as_deref().unwrap_or(""));
                self.editing = Some(item.id);
                self.mode = Mode::Edit;
                self.caret_on = true;
            }
            Kind::Image => {
                let Some(b) = app.store.image_path(&item).and_then(|p| Bitmap::load(&p, None).ok()) else {
                    return overlay::toast("Couldn't open the image", self.area(), None);
                };
                self.reset_mode();
                self.image = Some(ImageEditor { id: item.id, current: b, history: vec![], blur: false, drag: None, shown: R::default(), version: 0, prepared: None });
                self.mode = Mode::Image;
            }
            Kind::Files => {}
        }
    }

    fn save_edit(&mut self, app: &mut App, paste: bool) {
        let id = match self.mode {
            Mode::Edit => {
                let Some(id) = self.editing.clone() else { return };
                let text = self.editor.text();
                if app.store.get(&id).and_then(|i| i.text.clone()) != Some(text.clone()) {
                    app.store.update_text(&id, text);
                }
                id
            }
            Mode::Image => {
                let Some(e) = &self.image else { return };
                let id = e.id.clone();
                if e.edited() && app.store.update_image(&id, &e.current).is_err() {
                    return overlay::toast("Couldn't save the image", self.area(), None);
                }
                id
            }
            _ => return,
        };
        self.reset_mode();
        self.rebuild(app, Some(id.clone()));
        app.changed();
        if paste {
            self.paste(app, &id, PasteMode::Normal, None);
        } else {
            overlay::toast("Saved", self.area(), None);
        }
    }

    // ---------- actions ----------

    /// The window's place on screen, for messages shown near it.
    pub fn area(&self) -> Option<RECT> {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r).ok()? };
        Some(r)
    }

    fn target_exe(&self) -> Option<String> {
        system::exe_of(self.target)
    }

    fn target_wants_plain(&self, app: &App) -> bool {
        self.target_exe().map(|p| system::exe_name(&p)).is_some_and(|n| app.settings.plain_text_apps.iter().any(|a| a.eq_ignore_ascii_case(&n)))
    }

    /// Puts an item (or `replacement` text) on the clipboard and pastes it where you came from.
    pub fn paste(&mut self, app: &mut App, id: &str, mut mode: PasteMode, replacement: Option<String>) {
        let Some(item) = app.store.get(id).cloned() else { return };
        if mode == PasteMode::Normal && self.target_wants_plain(app) {
            mode = PasteMode::Plain;
        }
        let formats = match &replacement {
            Some(text) => Some(clip::text_formats(text)),
            None => clip::formats_for(&item, app.store.image_path(&item), mode == PasteMode::Plain, false),
        };
        let Some(formats) = formats else { return overlay::toast("That item can't be read any more", self.area(), None) };
        app.put_on_clipboard(&formats);
        if replacement.is_none() {
            app.store.mark_copied(id);
        }
        if mode != PasteMode::CopyOnly && app.settings.sounds {
            system::play(true);
        }
        let (target, area) = (self.target, self.area());
        self.close(app);
        app.changed();
        if mode == PasteMode::CopyOnly {
            overlay::toast("Copied to clipboard", area, None);
        }
        if target.0.is_null() {
            return;
        }
        run_later(move || unsafe {
            let _ = SetForegroundWindow(target);
            if mode == PasteMode::CopyOnly {
                return;
            }
            // paste as soon as the window really has the focus (usually at once)
            for _ in 0..40 {
                if GetForegroundWindow() == target {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(10)); // let it focus its text box
            clip::press_paste();
        });
    }

    fn toggle_pin(&mut self, app: &mut App, id: &str) {
        let Some(pinned) = app.store.get(id).map(|i| !i.pinned) else { return };
        app.store.set_pinned(id, pinned);
        self.rebuild(app, Some(id.to_string()));
        app.changed();
        overlay::toast(if pinned { "Pinned" } else { "Unpinned" }, self.area(), None);
    }

    fn delete(&mut self, app: &mut App, id: &str) {
        let index = self.items.iter().position(|i| i == id).unwrap_or(self.selected);
        self.deleted = app.store.remove(id);
        self.rebuild(app, None);
        self.select(index as isize);
        if self.items.is_empty() && self.mode == Mode::Preview {
            self.reset_mode();
        }
        app.changed();
        overlay::toast("Deleted", self.area(), Some(("Undo", Box::new(|| {
            with_popup(|p, a| p.undo_delete(a));
        }))));
    }

    pub fn undo_delete(&mut self, app: &mut App) {
        if self.deleted.is_empty() {
            return;
        }
        let ids = std::mem::take(&mut self.deleted);
        let n = app.store.restore(&ids);
        self.rebuild(app, ids.first().cloned());
        app.changed();
        repaint();
        let text = if n == 1 { "Restored".to_string() } else { format!("Restored {n} items") };
        overlay::toast(&text, self.area(), None);
    }

    /// Open a link, show a file in Explorer, or save a picture.
    fn quick_action(&mut self, app: &mut App, id: &str) {
        let Some(item) = app.store.get(id).cloned() else { return };
        match app.cache.info(&item, app.store.revision(id)).facts.card {
            Card::Link => {
                if system::open_url(item.text.as_deref().unwrap_or("").trim()) {
                    self.close(app);
                } else {
                    overlay::toast("Couldn't open the link", self.area(), None);
                }
            }
            Card::Files => match item.file_paths().into_iter().find(|p| std::path::Path::new(p).exists()) {
                Some(path) => {
                    system::reveal(path);
                    self.close(app);
                }
                None => overlay::toast("That file no longer exists", self.area(), None),
            },
            Card::Image => self.save_to_file(id.to_string()),
            _ => {}
        }
    }

    fn save_to_file(&mut self, id: String) {
        self.keep_open = true;
        let owner = self.hwnd;
        run_later(move || {
            let Some(Some((item, image))) = with(|a| a.store.get(&id).map(|i| (i.clone(), a.store.image_path(i)))) else { return };
            let outcome = if item.kind == Kind::Image {
                let name = format!("Image {}.png", item.copied_at.format("%Y-%m-%d %H%M%S"));
                system::save_dialog(owner, &name, &[("PNG image", "*.png"), ("JPEG image", "*.jpg")]).map(|(mut path, kind)| {
                    if kind == 2 && !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg")) {
                        path.set_extension("jpg");
                    }
                    let png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
                    let saved = match image {
                        Some(from) if png => std::fs::copy(from, &path).map(|_| ()).map_err(|e| e.to_string()),
                        Some(from) => Bitmap::load(&from, None).and_then(|b| b.save(&path)).map_err(|e| e.to_string()),
                        None => Err("the picture is gone".into()),
                    };
                    (path, saved)
                })
            } else {
                let name = format!("{}.txt", textops::suggest_file_name(item.text.as_deref().unwrap_or("")));
                system::save_dialog(owner, &name, &[("Text file", "*.txt"), ("All files", "*.*")]).map(|(path, _)| {
                    let saved = std::fs::write(&path, item.text.as_deref().unwrap_or("")).map_err(|e| e.to_string());
                    (path, saved)
                })
            };
            with_popup(|p, _| {
                p.keep_open = false;
                if visible() {
                    system::force_foreground(p.hwnd);
                }
                if let Some((path, saved)) = outcome {
                    let message = match saved {
                        Ok(()) => format!("Saved {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                        Err(e) => format!("Couldn't save: {e}"),
                    };
                    overlay::toast(&message, p.area(), None);
                }
            });
        });
    }

    fn clear_history(&mut self) {
        self.keep_open = true;
        let owner = self.hwnd;
        run_later(move || {
            let yes = system::confirm(owner, "Delete all unpinned clipboard history? This can't be undone.");
            with_popup(|p, a| {
                p.keep_open = false;
                if yes {
                    p.deleted = a.store.clear_unpinned();
                    p.rebuild(a, None);
                    a.changed();
                    repaint();
                    let n = p.deleted.len();
                    overlay::toast(&format!("Cleared {n} items"), p.area(), Some(("Undo", Box::new(|| {
                        with_popup(|p, a| p.undo_delete(a));
                    }))));
                }
                system::force_foreground(p.hwnd);
            });
        });
    }

    fn drag_out(&mut self, app: &mut App, id: &str) {
        let Some(item) = app.store.get(id).cloned() else { return };
        let Some(formats) = clip::formats_for(&item, app.store.image_path(&item), false, true) else { return };
        self.keep_open = true;
        overlay::hide_tooltip();
        run_later(move || {
            let dropped = menu::drag(&formats);
            with_popup(|p, a| {
                p.keep_open = false;
                p.pressed = None;
                if dropped {
                    p.close(a);
                }
            });
        });
    }

    /// The menu for an item (right-click or the menu key), at the mouse.
    fn item_menu(&mut self, app: &mut App, id: &str) {
        let Some(item) = app.store.get(id).cloned() else { return };
        let facts = app.cache.info(&item, app.store.revision(id)).facts.clone();
        self.keep_open = true;
        overlay::hide_tooltip();
        let (owner, id, dark) = (self.hwnd, id.to_string(), app.theme.dark);
        run_later(move || {
            let chosen = menu::item(owner, &item, &facts, dark);
            with_popup(|p, a| {
                p.keep_open = false;
                match chosen {
                    Some(command) => p.command(a, &id, command),
                    None => p.close_if_left(a),
                }
                repaint();
            });
        });
    }

    fn command(&mut self, app: &mut App, id: &str, command: menu::Item) {
        use menu::Item as M;
        let Some(item) = app.store.get(id).cloned() else { return };
        let copy = |p: &mut Popup, a: &mut App, text: String| p.paste(a, id, PasteMode::CopyOnly, Some(text));
        match command {
            M::Paste => self.paste(app, id, PasteMode::Normal, None),
            M::PastePlain => self.paste(app, id, PasteMode::Plain, None),
            M::Copy => self.paste(app, id, PasteMode::CopyOnly, None),
            M::Transform(t) => {
                let text = textops::apply(t, item.text.as_deref().unwrap_or(""));
                self.paste(app, id, PasteMode::Normal, Some(text));
            }
            M::Open => self.quick_action(app, id),
            M::ColorAs(kind) => {
                if let Some(c) = item.color() {
                    let text = match kind {
                        1 => crate::model::color_rgb(c),
                        2 => crate::model::color_hsl(c),
                        _ => crate::model::color_hex(c),
                    };
                    copy(self, app, text);
                }
            }
            M::OpenQr => {
                if system::open_url(item.qr_text.as_deref().unwrap_or("")) {
                    self.close(app);
                }
            }
            M::CopyQr => copy(self, app, item.qr_text.clone().unwrap_or_default()),
            M::CopyImageText => copy(self, app, item.ocr_text.as_deref().unwrap_or("").trim().to_string()),
            M::SaveFile => self.save_to_file(id.to_string()),
            M::Preview => {
                if let Some(i) = self.items.iter().position(|i| i == id) {
                    self.selected = i;
                }
                if self.mode != Mode::Preview {
                    self.toggle_preview();
                }
            }
            M::Edit => self.open_editor(app, id),
            M::Pin => self.toggle_pin(app, id),
            M::Delete => self.delete(app, id),
        }
    }

    fn more_menu(&mut self, app: &mut App) {
        self.keep_open = true;
        let ready = crate::net::UPDATE.lock().unwrap().ready.clone();
        let target = self.target_exe().map(|p| (system::app_name(&p), self.target_wants_plain(app)));
        let (owner, compact, dark) = (self.hwnd, app.settings.compact, app.theme.dark);
        run_later(move || {
            let chosen = menu::more(owner, ready, compact, target, dark);
            with_popup(|p, a| {
                p.keep_open = false;
                let Some(chosen) = chosen else { return p.close_if_left(a) };
                use menu::More as M;
                match chosen {
                    M::Update => run_later(|| {
                        with(|a| a.save());
                        if crate::net::apply_and_restart() {
                            app::quit();
                        }
                    }),
                    M::Compact => {
                        let mut s = a.settings.clone();
                        s.compact = !s.compact;
                        a.apply_settings(s);
                        let keep = p.selected_id();
                        p.rebuild(a, keep);
                    }
                    M::PlainHere => {
                        let Some(exe) = p.target_exe().map(|e| system::exe_name(&e)) else { return };
                        let mut s = a.settings.clone();
                        let before = s.plain_text_apps.len();
                        s.plain_text_apps.retain(|x| !x.eq_ignore_ascii_case(&exe));
                        if s.plain_text_apps.len() == before {
                            s.plain_text_apps.push(exe);
                        }
                        a.apply_settings(s);
                        let message = if p.target_wants_plain(a) { "Pastes here will be plain text" } else { "Pastes here keep formatting" };
                        overlay::toast(message, p.area(), None);
                    }
                    M::Reset => p.reset_placement(a),
                    M::Settings => {
                        p.close(a);
                        run_later(settings_ui::show);
                    }
                    M::Clear => p.clear_history(),
                    M::Folder => system::open_folder(&settings::dir()),
                    M::Quit => run_later(app::quit),
                }
                repaint();
            });
        });
    }

    // ---------- input ----------

    /// Returns true when the key was used (so its character must not be typed too).
    pub fn key(&mut self, app: &mut App, vk: VIRTUAL_KEY) -> bool {
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(k.0)) } < 0;
        let (ctrl, shift, alt) = (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU));
        overlay::hide_tooltip();
        self.caret_on = true;
        let mut clipboard = FieldClip(self.hwnd);
        match self.mode {
            Mode::Edit => {
                return match vk {
                    VK_ESCAPE => {
                        self.reset_mode();
                        true
                    }
                    VK_S if ctrl => {
                        self.save_edit(app, false);
                        true
                    }
                    VK_RETURN if ctrl => {
                        self.save_edit(app, true);
                        true
                    }
                    _ => self.editor.key(vk, ctrl, shift, &mut clipboard) != Did::Nothing,
                };
            }
            Mode::Image => {
                match vk {
                    VK_ESCAPE => self.reset_mode(),
                    VK_Z if ctrl => {
                        if let Some(e) = &mut self.image {
                            e.undo();
                        }
                    }
                    VK_R if !ctrl => self.image_rotate(),
                    VK_C | VK_B if !ctrl => {
                        if let Some(e) = &mut self.image {
                            e.blur = vk == VK_B;
                        }
                    }
                    VK_S if ctrl => self.save_edit(app, false),
                    VK_RETURN if ctrl => self.save_edit(app, true),
                    _ => return false,
                }
                return true;
            }
            _ => {}
        }
        if ctrl && vk == VK_Z && !self.deleted.is_empty() && !self.search.can_undo() {
            self.undo_delete(app);
            return true;
        }
        let empty = self.search.is_empty();
        let grid = self.tab == Tab::Images;
        let step: isize = if grid { 3 } else { 1 };
        let selected = self.selected as isize;
        // quick paste: 1–9 while the search box is empty, Alt+1–9 any time
        let digit = match vk.0 {
            0x31..=0x39 => vk.0 - 0x30,
            0x61..=0x69 => vk.0 - 0x60,
            _ => 0,
        };
        if digit > 0 && (alt || (empty && !ctrl && !shift)) {
            if let Some(id) = self.items.get(usize::from(digit) - 1).cloned() {
                self.paste(app, &id, PasteMode::Normal, None);
            }
            return true;
        }
        let id = self.selected_id();
        match vk {
            VK_ESCAPE => {
                if self.mode == Mode::Preview {
                    self.toggle_preview();
                } else if !empty {
                    self.search.set("");
                    self.search_changed(app);
                } else {
                    self.dismiss();
                }
            }
            VK_SPACE if empty || shift => self.toggle_preview(),
            VK_TAB => {
                let at = TABS.iter().position(|t| t.0 == self.tab).unwrap_or(0) as isize;
                let next = (at + if shift { -1 } else { 1 }).rem_euclid(TABS.len() as isize);
                self.set_tab(app, TABS[next as usize].0);
            }
            VK_F2 => {
                if let Some(id) = id {
                    self.open_editor(app, &id);
                }
            }
            VK_DOWN => self.select(selected + step),
            VK_UP => self.select(selected - step),
            VK_RIGHT if grid && empty => self.select(selected + 1),
            VK_LEFT if grid && empty => self.select(selected - 1),
            VK_NEXT => self.select(selected + 6 * step),
            VK_PRIOR => self.select(selected - 6 * step),
            VK_RETURN => {
                if let Some(id) = id {
                    let mode = if ctrl { PasteMode::CopyOnly } else if shift { PasteMode::Plain } else { PasteMode::Normal };
                    self.paste(app, &id, mode, None);
                }
            }
            VK_O if ctrl => {
                if let Some(item) = id.and_then(|i| app.store.get(&i).cloned()) {
                    if item.kind != Kind::Image {
                        self.quick_action(app, &item.id);
                    }
                }
            }
            VK_S if ctrl => {
                if let Some(item) = id.and_then(|i| app.store.get(&i).cloned()) {
                    if item.kind != Kind::Files {
                        self.save_to_file(item.id);
                    }
                }
            }
            VK_P if ctrl => {
                if let Some(id) = id {
                    self.toggle_pin(app, &id);
                }
            }
            VK_DELETE if shift || empty => {
                if let Some(id) = id {
                    self.delete(app, &id);
                }
            }
            VK_APPS => {
                if let Some(id) = id {
                    self.item_menu(app, &id);
                }
            }
            VK_F10 if shift => {
                if let Some(id) = id {
                    self.item_menu(app, &id);
                }
            }
            // everything else edits the search
            _ => {
                return match self.search.key(vk, ctrl, shift, &mut clipboard) {
                    Did::Changed => {
                        self.search_changed(app);
                        true
                    }
                    Did::Moved => true,
                    Did::Nothing => false,
                };
            }
        }
        true
    }

    pub fn character(&mut self, app: &mut App, c: u16) {
        self.caret_on = true;
        match self.mode {
            Mode::Edit => {
                self.editor.character(c);
            }
            Mode::Image => {}
            _ => {
                if self.search.character(c) == Did::Changed {
                    if self.mode == Mode::Preview {
                        self.mode = Mode::List;
                    }
                    self.search_changed(app);
                }
            }
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(area, _)| area.contains(x, y)).map(|h| h.1)
    }

    fn row_id(&self, index: usize) -> Option<String> {
        self.items.get(index).cloned()
    }

    fn move_to(&mut self, app: &mut App, x: f32, y: f32) {
        self.mouse = (x, y);
        if let Some(grab) = self.scrollbar_grab {
            let view = self.list.h;
            let thumb = paint::thumb_height(view, self.content_height);
            let k = (y - self.list.y - grab) / (view - thumb).max(1.0);
            self.scroll_to = (k * self.max_scroll()).clamp(0.0, self.max_scroll());
            self.scroll = self.scroll_to;
            return repaint();
        }
        if let Some((hit, px, py)) = self.pressed {
            match hit {
                Hit::Search => self.search.drag(x, y),
                Hit::Editor => self.editor.drag(x, y),
                Hit::Preview => self.preview.drag(x, y),
                Hit::Canvas => {
                    if let Some(e) = &mut self.image {
                        if let Some(d) = &mut e.drag {
                            d.1 = (x, y);
                        }
                    }
                }
                Hit::Row(i) if (x - px).abs() > 4.0 || (y - py).abs() > 4.0 => {
                    self.pressed = None;
                    if let Some(id) = self.row_id(i) {
                        self.drag_out(app, &id);
                    }
                }
                _ => {}
            }
            return repaint();
        }
        let hover = self.hit(x, y);
        if hover != self.hover {
            self.hover = hover;
            overlay::hide_tooltip();
            unsafe {
                if matches!(hover, Some(Hit::Row(_))) {
                    SetTimer(Some(self.hwnd), TIMER_TOOLTIP, 900, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_TOOLTIP);
                }
            }
            repaint();
        }
    }

    fn press(&mut self, app: &mut App, x: f32, y: f32) {
        overlay::hide_tooltip();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TOOLTIP);
        }
        let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
        let (when, at, n) = self.clicks;
        let again = when.elapsed() < Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() })) && (x - at.0).abs() < 4.0 && (y - at.1).abs() < 4.0;
        let count = if again { n + 1 } else { 1 };
        self.clicks = (Instant::now(), (x, y), count);
        let Some(hit) = self.hit(x, y) else { return };
        self.pressed = Some((hit, x, y));
        self.caret_on = true;
        match hit {
            Hit::Search => self.search.press(x, y, shift, count),
            Hit::Editor => self.editor.press(x, y, shift, count),
            Hit::Preview => self.preview.press(x, y, shift, count),
            Hit::Canvas => {
                if let Some(e) = &mut self.image {
                    e.drag = Some(((x, y), (x, y)));
                }
            }
            Hit::Scrollbar => {
                let view = self.list.h;
                let thumb = paint::thumb_height(view, self.content_height);
                let top = self.list.y + (view - thumb) * self.scroll / self.max_scroll().max(1.0);
                self.scrollbar_grab = Some(if y >= top && y < top + thumb { y - top } else { thumb / 2.0 });
                self.move_to(app, x, y);
            }
            _ => {}
        }
        repaint();
    }

    fn release(&mut self, app: &mut App, x: f32, y: f32) {
        self.scrollbar_grab = None;
        let Some((hit, ..)) = self.pressed.take() else { return };
        if hit == Hit::Canvas {
            self.image_selected(x, y);
            return repaint();
        }
        if self.hit(x, y) != Some(hit) {
            return repaint();
        }
        match hit {
            Hit::Clear => {
                self.search.set("");
                self.search_changed(app);
            }
            Hit::Tab(i) => {
                if self.mode != Mode::List {
                    self.reset_mode();
                }
                self.set_tab(app, TABS[i].0);
            }
            Hit::Row(i) => {
                if let Some(id) = self.row_id(i) {
                    self.selected = i;
                    self.paste(app, &id, PasteMode::Normal, None);
                }
            }
            Hit::Action(i) => {
                if let Some(id) = self.row_id(i) {
                    self.quick_action(app, &id);
                }
            }
            Hit::Pin(i) => {
                if let Some(id) = self.row_id(i) {
                    self.toggle_pin(app, &id);
                }
            }
            Hit::More => self.more_menu(app),
            Hit::Cancel => self.reset_mode(),
            Hit::Save => self.save_edit(app, false),
            Hit::SavePaste => self.save_edit(app, true),
            Hit::ToolCrop | Hit::ToolBlur => {
                if let Some(e) = &mut self.image {
                    e.blur = hit == Hit::ToolBlur;
                }
            }
            Hit::Rotate => self.image_rotate(),
            Hit::UndoImage => {
                if let Some(e) = &mut self.image {
                    e.undo();
                }
            }
            _ => {}
        }
        repaint();
    }

    fn image_rotate(&mut self) {
        if let Some(e) = &mut self.image {
            let turned = e.current.rotated();
            e.apply(turned);
        }
    }

    /// A finished drag over the picture: crop to it, or blur it.
    fn image_selected(&mut self, x: f32, y: f32) {
        let Some(e) = &mut self.image else { return };
        let Some((start, _)) = e.drag.take() else { return };
        if e.shown.w <= 0.0 {
            return;
        }
        let k = e.current.w as f32 / e.shown.w;
        let px = |(a, b): (f32, f32)| ((a - e.shown.x) * k, (b - e.shown.y) * k);
        if let Some(area) = Area::between(px(start), px((x, y)), e.current.w, e.current.h) {
            let next = if e.blur { e.current.pixelated(area) } else { e.current.cropped(area) };
            e.apply(next);
        }
    }

    fn wheel(&mut self, notches: f32) {
        overlay::hide_tooltip();
        let mut lines = 3u32;
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut _ as _), Default::default());
        }
        let amount = notches * 18.0 * lines.min(20) as f32;
        match self.mode {
            Mode::List => {
                self.scroll_to = (self.scroll_to - amount).clamp(0.0, self.max_scroll());
                unsafe {
                    SetTimer(Some(self.hwnd), TIMER_ANIMATE, 10, None);
                }
            }
            Mode::Preview => self.preview.wheel(amount),
            Mode::Edit => self.editor.wheel(amount),
            Mode::Image => {}
        }
        repaint();
    }

    fn timer(&mut self, app: &mut App, id: usize) {
        match id {
            TIMER_CARET => {
                self.caret_on = !self.caret_on;
                repaint();
            }
            TIMER_ANIMATE => {
                let mut busy = false;
                if self.appearing.is_some_and(|(at, _)| at.elapsed() >= Duration::from_millis(90)) {
                    self.appearing = None;
                    busy = true; // one last, settled frame
                }
                busy |= self.appearing.is_some();
                let gap = self.scroll_to - self.scroll;
                if gap.abs() > 0.0 {
                    self.scroll = if gap.abs() < 0.7 { self.scroll_to } else { self.scroll + gap * 0.35 };
                    busy = true;
                }
                if busy {
                    repaint();
                } else {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), TIMER_ANIMATE);
                    }
                }
            }
            TIMER_TOOLTIP => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_TOOLTIP);
                }
                if let Some(Hit::Row(i)) = self.hover {
                    self.show_tooltip(app, i);
                }
            }
            TIMER_FOCUS => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_FOCUS);
                }
                if !self.keep_open && unsafe { GetForegroundWindow() } != self.hwnd {
                    self.close(app);
                }
            }
            _ => {}
        }
    }

    /// More of the text, or a bigger picture, beside the hovered row.
    fn show_tooltip(&mut self, app: &mut App, index: usize) {
        let Some(item) = self.row_id(index).and_then(|id| app.store.get(&id).cloned()) else { return };
        let Some(row) = self.hits.iter().find(|h| h.1 == Hit::Row(index)).map(|h| h.0) else { return };
        let Some(window) = self.area() else { return };
        let anchor = RECT { left: window.left, top: window.top + (row.y * self.scale) as i32, right: window.right, bottom: window.top + (row.bottom() * self.scale) as i32 };
        if item.kind == Kind::Image {
            if let Some(picture) = app.cache.full(&item, &app.store, (480.0 * self.scale) as u32) {
                overlay::tooltip_picture(&app.theme, picture, anchor, self.scale);
            }
        } else {
            let text = item.text.as_deref().unwrap_or("");
            let mut cut: String = text.lines().take(25).collect::<Vec<_>>().join("\n").chars().take(1500).collect();
            if cut.len() < text.trim_end().len() {
                cut = format!("{}\n…", cut.trim_end());
            }
            overlay::tooltip_text(&app.theme, &cut, anchor, self.scale);
        }
    }

    pub fn paint(&mut self, app: &mut App) {
        let (w, h) = ((self.width * self.scale).round() as i32, (self.height * self.scale).round() as i32);
        if self.surface.as_ref().is_none_or(|s| s.w != w || s.h != h) {
            self.surface = Surface::new(w, h);
        }
        let Some(surface) = self.surface.take() else { return };
        paint::popup(self, app, &surface);
        self.surface = Some(surface);
    }
}

// ---------- the window ----------

fn create_window() -> HWND {
    let class = wide("ClipVaultPopup");
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let icon = LoadIconW(instance, PCWSTR(1 as _)).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.unwrap_or_default(), lpszClassName: PCWSTR(class.as_ptr()), hIcon: icon, style: CS_DROPSHADOW, ..Default::default() };
        RegisterClassW(&wc);
        let title = wide("ClipVault");
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, PCWSTR(class.as_ptr()), PCWSTR(title.as_ptr()), WS_POPUP | WS_THICKFRAME, 0, 0, 440, 580, None, None, instance, None).unwrap_or_default();
        WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
        // applies WM_NCCALCSIZE, which removes the frame
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        hwnd
    }
}

/// Rounded corners, the acrylic backdrop and dark or light edges.
pub fn style_window(hwnd: HWND, dark: windows_core::BOOL) {
    unsafe {
        let set = |attribute: DWMWINDOWATTRIBUTE, value: i32| {
            let _ = DwmSetWindowAttribute(hwnd, attribute, &value as *const _ as _, 4);
        };
        set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark.0);
        set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0);
        if crate::theme::glass() {
            set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW.0);
            let all = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &all);
        }
    }
}

fn point(lp: LPARAM) -> (i32, i32) {
    (i32::from(lp.0 as i16), i32::from((lp.0 >> 16) as i16))
}

/// Which resize edge a point near the border belongs to.
fn edge(x: f32, y: f32, w: f32, h: f32) -> Option<u32> {
    const GRIP: f32 = 6.0;
    let (l, r, t, b) = (x < GRIP, x >= w - GRIP, y < GRIP, y >= h - GRIP);
    Some(match (l, r, t, b) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    })
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let design = |p: &Popup| {
        let (x, y) = point(lp);
        (x as f32 / p.scale, y as f32 / p.scale)
    };
    let handled = match msg {
        // the whole window is client area: no frame of Windows' own
        WM_NCCALCSIZE if wp.0 != 0 => Some(0),
        WM_NCHITTEST => with_popup(|p, _| {
            let (sx, sy) = point(lp);
            let mut pt = POINT { x: sx, y: sy };
            unsafe {
                let _ = ScreenToClient(h, &mut pt);
            }
            let (x, y) = (pt.x as f32 / p.scale, pt.y as f32 / p.scale);
            if let Some(e) = edge(x, y, p.width, p.height) {
                return e as isize;
            }
            if p.hit(x, y) == Some(Hit::Drag) { HTCAPTION as isize } else { HTCLIENT as isize }
        }),
        WM_NCACTIVATE => Some(1),
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(h, &mut ps) };
            with_popup(|p, a| {
                p.sync_preview(a);
                p.paint(a);
                if let Some(s) = &p.surface {
                    unsafe {
                        let _ = BitBlt(dc, 0, 0, s.w, s.h, Some(s.dc), 0, 0, SRCCOPY);
                    }
                }
            });
            unsafe {
                let _ = EndPaint(h, &ps);
            }
            Some(0)
        }
        WM_SIZE => {
            let (w, height) = point(lp);
            with_popup(|p, a| {
                if w > 0 && height > 0 && visible() {
                    let (nw, nh) = (w as f32 / p.scale, height as f32 / p.scale);
                    if (nw - p.width).abs() > 0.5 || (nh - p.height).abs() > 0.5 {
                        p.width = nw;
                        p.height = nh;
                        let keep = p.selected_id();
                        p.rebuild(a, keep);
                        repaint();
                    }
                }
            });
            Some(0)
        }
        WM_GETMINMAXINFO => with_popup(|p, _| {
            let info = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
            let px = |v: f32| (v * p.scale) as i32;
            info.ptMinTrackSize = POINT { x: px(360.0), y: px(400.0) };
            info.ptMaxTrackSize = POINT { x: px(1100.0), y: px(1500.0) };
            0
        }),
        WM_DPICHANGED => {
            let dpi = unsafe { GetDpiForWindow(h) };
            // busy means we are placing the window ourselves, and our size is already right
            if with_popup(|p, _| p.scale = dpi as f32 / 96.0).is_some() {
                let r = unsafe { &*(lp.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            Some(0)
        }
        WM_ENTERSIZEMOVE => with_popup(|p, _| {
            overlay::hide_tooltip();
            p.moved = true;
            0
        }),
        WM_ACTIVATE => {
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                with_popup(|p, a| {
                    if p.keep_open {
                        return;
                    }
                    // Windows sometimes bounces focus back right after opening: look again shortly
                    if p.opened.elapsed() < Duration::from_millis(150) {
                        unsafe {
                            SetTimer(Some(h), TIMER_FOCUS, 150, None);
                        }
                    } else {
                        p.close(a);
                    }
                });
            }
            None
        }
        WM_CLOSE => {
            with_popup(|p, a| p.close(a));
            Some(0)
        }
        WM_TIMER => {
            with_popup(|p, a| p.timer(a, wp.0));
            Some(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            // Alt+F4 and Alt+Space stay Windows' business
            if msg == WM_SYSKEYDOWN && matches!(vk, VK_F4 | VK_SPACE) {
                None
            } else {
                let used = with_popup(|p, a| {
                    let used = p.key(a, vk);
                    p.swallow_char = used;
                    repaint();
                    used
                });
                (used == Some(true)).then_some(0)
            }
        }
        WM_CHAR => with_popup(|p, a| {
            if !std::mem::take(&mut p.swallow_char) {
                p.character(a, wp.0 as u16);
                repaint();
            }
            0
        }),
        WM_SYSCHAR => Some(0),
        WM_IME_STARTCOMPOSITION => {
            with_popup(|p, _| {
                let field = if p.mode == Mode::Edit { &p.editor } else { &p.search };
                let (x, y, _) = field.caret_point();
                set_ime_position(h, (x * p.scale) as i32, (y * p.scale) as i32);
            });
            None
        }
        WM_MOUSEMOVE => with_popup(|p, a| {
            if !p.tracking_mouse {
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                p.tracking_mouse = unsafe { TrackMouseEvent(&mut t).is_ok() };
            }
            let (x, y) = design(p);
            p.move_to(a, x, y);
            0
        }),
        WM_MOUSELEAVE => with_popup(|p, _| {
            p.tracking_mouse = false;
            if p.pressed.is_none() {
                p.hover = None;
                overlay::hide_tooltip();
                unsafe {
                    let _ = KillTimer(Some(h), TIMER_TOOLTIP);
                }
                repaint();
            }
            0
        }),
        WM_LBUTTONDOWN => {
            unsafe {
                SetCapture(h);
            }
            with_popup(|p, a| {
                let (x, y) = design(p);
                p.press(a, x, y);
            });
            Some(0)
        }
        WM_LBUTTONUP => {
            with_popup(|p, a| {
                let (x, y) = design(p);
                p.release(a, x, y);
            });
            unsafe {
                let _ = ReleaseCapture();
            }
            Some(0)
        }
        WM_CAPTURECHANGED => with_popup(|p, _| {
            p.scrollbar_grab = None;
            0
        }),
        WM_RBUTTONUP => with_popup(|p, a| {
            let (x, y) = design(p);
            if let Some(Hit::Row(i) | Hit::Pin(i) | Hit::Action(i)) = p.hit(x, y) {
                p.selected = i;
                if let Some(id) = p.row_id(i) {
                    p.item_menu(a, &id);
                }
                repaint();
            }
            0
        }),
        WM_MOUSEWHEEL => with_popup(|p, _| {
            p.wheel(f32::from((wp.0 >> 16) as i16) / 120.0);
            0
        }),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => with_popup(|p, _| {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(h, &mut pt);
            }
            let hit = p.hit(pt.x as f32 / p.scale, pt.y as f32 / p.scale);
            let cursor = match hit {
                Some(Hit::Search | Hit::Editor | Hit::Preview) => IDC_IBEAM,
                Some(Hit::Canvas) => IDC_CROSS,
                Some(Hit::Drag | Hit::Scrollbar) | None => IDC_ARROW,
                Some(_) => IDC_HAND,
            };
            unsafe {
                SetCursor(LoadCursorW(None, cursor).ok());
            }
            1
        }),
        _ => None,
    };
    match handled {
        Some(v) => LRESULT(v),
        None => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

/// Puts the input method's composition window at the caret.
pub fn set_ime_position(h: HWND, x: i32, y: i32) {
    unsafe {
        let context = ImmGetContext(h);
        if !context.is_invalid() {
            let form = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: POINT { x, y }, ..Default::default() };
            let _ = ImmSetCompositionWindow(context, &form);
            let _ = ImmReleaseContext(h, context);
        }
    }
}
