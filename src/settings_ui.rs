//! The settings window. Nothing is applied until Save. Mouse and keyboard (Tab, Shift+Tab,
//! Space, Enter, Esc) both work.

use crate::app::{self, FieldClip, run_later, with};
use crate::gfx::{self, Align, R, Surface, Wrap, bold, display, r, text};
use crate::popup::{set_ime_position, style_window};
use crate::settings::{self, OpenAt, Settings, Theme as Choice};
use crate::textbox::TextBox;
use crate::theme::Theme;
use crate::win::wide;
use crate::{net, system};
use std::cell::RefCell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::PCWSTR;

const W: f32 = 660.0;
const H: f32 = 470.0;
/// The scrolling part of a page.
const VIEW: R = R { x: 202.0, y: 58.0, w: 458.0, h: 352.0 };
const CARD_W: f32 = 440.0;
const PAGES: [(&str, &str); 4] = [("General", "\u{E713}"), ("Appearance", "\u{E790}"), ("History", "\u{E81C}"), ("Updates", "\u{E895}")];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Nav(usize),
    Close,
    Hotkey,
    Open(OpenAt),
    CleanLinks,
    PlainApps,
    Startup,
    Theme(Choice),
    Accent,
    LinkIcons,
    Compact,
    Sounds,
    History,
    CheckNow,
    Install,
    WhatsNew,
    AutoUpdate,
    Cancel,
    Save,
}

/// One line of a settings card.
enum Part {
    Label(String),
    Hint(String),
    Gap(f32),
    Hotkey,
    Chips(Vec<(&'static str, bool, Control)>),
    Check(&'static str, bool, Control),
    PlainApps,
    History,
    Buttons(Vec<(&'static str, bool, Control)>),
}

impl Part {
    fn height(&self, width: f32) -> f32 {
        match self {
            Part::Label(_) => 26.0,
            Part::Hint(s) => gfx::measure(s, text(12.0), width, Wrap::Words).1.ceil() + 7.0,
            Part::Gap(h) => *h,
            Part::Hotkey | Part::PlainApps | Part::History => 32.0,
            Part::Chips(_) | Part::Buttons(_) => 30.0,
            Part::Check(..) => 22.0,
        }
    }
}

struct Ui {
    hwnd: HWND,
    surface: Option<Surface>,
    scale: f32,
    /// The settings being edited; copied over the real ones on Save.
    draft: Settings,
    startup: bool,
    history: TextBox,
    apps: TextBox,
    page: usize,
    scroll: f32,
    content_height: f32,
    hotkey_hint: &'static str,
    focus: Option<Control>,
    /// Show where the keyboard is (after Tab; hidden again by the mouse).
    ring: bool,
    hits: Vec<(R, Control)>,
    /// Every control in Tab order, with where it is (also when scrolled out of view).
    order: Vec<(R, Control)>,
    hover: Option<Control>,
    pressed: Option<Control>,
    caret_on: bool,
    swallow_char: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static OPEN: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

const HINT_IDLE: &str = "Click the box, then press the new key combination.";
const HINT_RECORDING: &str = "Press the new shortcut now…";

fn handle() -> HWND {
    HWND(OPEN.get() as _)
}

pub fn open_now() -> bool {
    OPEN.get() != 0
}

pub fn repaint() {
    if open_now() {
        unsafe {
            let _ = InvalidateRect(Some(handle()), None, false);
        }
    }
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut u| u.as_mut().map(f)))
}

/// Opens the window (or brings it forward). Call from a task, with nothing borrowed.
pub fn show() {
    if open_now() {
        return system::force_foreground(handle());
    }
    let Some((s, dark)) = with(|a| (a.settings.clone(), a.theme.dark)) else { return };
    let mut history = TextBox::new(false);
    history.digits_only = true;
    history.set(&s.max_history.to_string());
    let mut apps = TextBox::new(false);
    apps.set(&s.plain_text_apps.join(", "));

    let class = wide("ClipVaultSettings");
    let title = wide("ClipVault settings");
    let mut mouse = POINT::default();
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        let icon = LoadIconW(instance, PCWSTR(1 as _)).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.unwrap_or_default(), lpszClassName: PCWSTR(class.as_ptr()), hIcon: icon, style: CS_DROPSHADOW, ..Default::default() };
        RegisterClassW(&wc);
        let _ = GetCursorPos(&mut mouse);
        CreateWindowExW(WS_EX_TOPMOST | WS_EX_APPWINDOW, PCWSTR(class.as_ptr()), PCWSTR(title.as_ptr()), WS_POPUP | WS_THICKFRAME, 0, 0, 1, 1, None, None, instance, None).unwrap_or_default()
    };
    if hwnd.0.is_null() {
        return;
    }
    style_window(hwnd, dark.into());
    let (work, scale) = system::monitor_at(mouse);
    let (w, h) = ((W * scale) as i32, (H * scale) as i32);
    let ui = Ui::new(hwnd, s, scale, history, apps);
    UI.with_borrow_mut(|u| *u = Some(ui));
    OPEN.set(hwnd.0 as isize);
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), (work.left + work.right - w) / 2, (work.top + work.bottom - h) / 2, w, h, SWP_FRAMECHANGED);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        SetTimer(Some(hwnd), 1, 530, None);
    }
    system::force_foreground(hwnd);
}

fn close() {
    if open_now() {
        unsafe {
            let _ = DestroyWindow(handle());
        }
    }
}

impl Ui {
    fn new(hwnd: HWND, draft: Settings, scale: f32, history: TextBox, apps: TextBox) -> Ui {
        Ui {
            hwnd,
            surface: None,
            scale,
            startup: settings::startup_enabled(),
            draft,
            history,
            apps,
            page: 0,
            scroll: 0.0,
            content_height: 0.0,
            hotkey_hint: HINT_IDLE,
            focus: None,
            ring: false,
            hits: vec![],
            order: vec![],
            hover: None,
            pressed: None,
            caret_on: true,
            swallow_char: false,
        }
    }

    fn set_focus(&mut self, to: Option<Control>) {
        if self.focus == to {
            return;
        }
        // the shortcut is off while recording, so pressing it here doesn't open the popup
        if self.focus == Some(Control::Hotkey) {
            run_later(|| {
                with(|a| a.register_hotkey());
            });
            if self.hotkey_hint == HINT_RECORDING {
                self.hotkey_hint = HINT_IDLE;
            }
        }
        self.focus = to;
        if to == Some(Control::Hotkey) {
            app::suspend_hotkey();
            self.hotkey_hint = HINT_RECORDING;
        }
        self.caret_on = true;
    }

    fn go_to(&mut self, page: usize) {
        if self.page != page {
            self.page = page;
            self.scroll = 0.0;
        }
    }

    fn activate(&mut self, c: Control) {
        let flip = |b: &mut bool| *b = !*b;
        match c {
            Control::Nav(i) => self.go_to(i),
            Control::Close | Control::Cancel => run_later(close),
            Control::Save => self.save(),
            Control::Open(o) => self.draft.open_at = o,
            Control::Theme(t) => self.draft.theme = t,
            Control::CleanLinks => flip(&mut self.draft.clean_links),
            Control::Startup => flip(&mut self.startup),
            Control::Accent => flip(&mut self.draft.use_system_accent),
            Control::LinkIcons => flip(&mut self.draft.link_icons),
            Control::Compact => flip(&mut self.draft.compact),
            Control::Sounds => flip(&mut self.draft.sounds),
            Control::AutoUpdate => flip(&mut self.draft.auto_update),
            Control::CheckNow => net::check_in_background(true),
            Control::Install => run_later(|| {
                with(|a| a.save());
                if net::apply_and_restart() {
                    app::quit();
                }
            }),
            Control::WhatsNew => {
                system::open_url(&format!("https://github.com/{}/releases", net::REPO));
            }
            Control::Hotkey | Control::PlainApps | Control::History => {}
        }
    }

    fn save(&mut self) {
        let Some(max) = self.history.text().trim().parse::<usize>().ok().filter(|n| *n >= 10) else {
            self.go_to(2);
            let hwnd = self.hwnd;
            return run_later(move || system::warn(Some(hwnd), "History size must be a number of at least 10."));
        };
        self.set_focus(None);
        let mut draft = self.draft.clone();
        draft.max_history = max.min(100_000) as _;
        let mut names: Vec<String> = Vec::new();
        for name in self.apps.text().split([',', ';', ' ']).filter(|n| !n.is_empty()) {
            let name = name.to_lowercase();
            let name = name.strip_suffix(".exe").unwrap_or(&name).to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        draft.plain_text_apps = names;
        let (startup, hwnd) = (self.startup, self.hwnd);
        run_later(move || apply(draft, startup, hwnd));
    }

    fn key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let down = |k: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(k.0)) } < 0;
        let (ctrl, shift, alt) = (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU));
        let win = down(VK_LWIN) || down(VK_RWIN);
        self.caret_on = true;
        if self.focus == Some(Control::Hotkey) && !matches!(vk, VK_TAB | VK_ESCAPE) {
            if matches!(vk, VK_CONTROL | VK_MENU | VK_SHIFT | VK_LWIN | VK_RWIN | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU | VK_LSHIFT | VK_RSHIFT) {
                return true; // wait for the main key
            }
            let mut mods = 0;
            for (held, bit) in [(ctrl, settings::MOD_CONTROL_), (alt, settings::MOD_ALT_), (shift, settings::MOD_SHIFT_), (win, settings::MOD_WIN_)] {
                if held {
                    mods |= bit;
                }
            }
            if mods & !settings::MOD_SHIFT_ == 0 {
                // Enter and Space alone still work the window
                if mods == 0 && vk == VK_RETURN {
                    self.save();
                } else {
                    self.hotkey_hint = "Include Ctrl, Alt or Win in the shortcut.";
                }
                return true;
            }
            self.draft.hotkey_modifiers = mods;
            self.draft.hotkey_key = u32::from(vk.0);
            self.hotkey_hint = "Click Save to use this shortcut.";
            return true;
        }
        match vk {
            VK_ESCAPE => run_later(close),
            VK_TAB => {
                let n = self.order.len() as isize;
                if n > 0 {
                    let at = self.focus.and_then(|f| self.order.iter().position(|o| o.1 == f)).map(|i| i as isize);
                    let next = match at {
                        Some(i) => (i + if shift { -1 } else { 1 }).rem_euclid(n),
                        None => {
                            if shift { n - 1 } else { 0 }
                        }
                    };
                    let (area, control) = self.order[next as usize];
                    self.set_focus(Some(control));
                    self.ring = true;
                    // scroll it into view
                    if area.x >= VIEW.x && !matches!(control, Control::Close | Control::Cancel | Control::Save) {
                        if area.y < VIEW.y + 8.0 {
                            self.scroll -= VIEW.y + 8.0 - area.y;
                        } else if area.bottom() > VIEW.bottom() - 8.0 {
                            self.scroll += area.bottom() - VIEW.bottom() + 8.0;
                        }
                    }
                }
            }
            VK_RETURN => match self.focus {
                Some(c @ (Control::Nav(_) | Control::Close | Control::Cancel | Control::CheckNow | Control::Install | Control::WhatsNew)) => self.activate(c),
                _ => self.save(),
            },
            _ => {
                let mut clipboard = FieldClip(self.hwnd);
                match self.focus {
                    Some(Control::History) => {
                        self.history.key(vk, ctrl, shift, &mut clipboard);
                        return false;
                    }
                    Some(Control::PlainApps) => {
                        self.apps.key(vk, ctrl, shift, &mut clipboard);
                        return false;
                    }
                    Some(c) if vk == VK_SPACE => self.activate(c),
                    _ => return false,
                }
            }
        }
        true
    }

    fn parts(&self) -> Vec<Vec<Part>> {
        use Part::*;
        let d = &self.draft;
        let label = |s: &str| Label(s.into());
        let hint = |s: &str| Hint(s.into());
        match self.page {
            0 => vec![
                vec![label("Shortcut to open ClipVault"), Hotkey, hint(self.hotkey_hint)],
                vec![
                    label("Open the popup at"),
                    Chips([("Text cursor", OpenAt::TextCursor), ("Mouse", OpenAt::Mouse), ("Where I left it", OpenAt::LastPosition)].map(|(n, o)| (n, d.open_at == o, Control::Open(o))).into()),
                    hint("Text cursor falls back to the mouse when an app doesn't report its cursor. To move the popup, drag the empty space next to the tabs or in the footer."),
                ],
                vec![
                    label("Pasting"),
                    Check("Remove tracking from copied links", d.clean_links, Control::CleanLinks),
                    hint("Strips things like ?utm_source= and &fbclid= the moment you copy a link."),
                    Gap(14.0),
                    label("Always paste plain text into these apps"),
                    PlainApps,
                    hint("App names separated by commas, e.g. code, notepad, windowsterminal. You can also tick this from the ⋯ menu in the popup."),
                ],
                vec![Check("Start ClipVault when Windows starts", self.startup, Control::Startup)],
            ],
            1 => vec![
                vec![label("Theme"), Chips([("Follow Windows", Choice::System), ("Dark", Choice::Dark), ("Light", Choice::Light)].map(|(n, c)| (n, d.theme == c, Control::Theme(c))).into())],
                vec![Check("Use my Windows accent color", d.use_system_accent, Control::Accent), hint("Highlights follow the color picked in Windows Settings › Personalization › Colors.")],
                vec![Check("Show website icons on links", d.link_icons, Control::LinkIcons), hint("Downloads each site's icon once, directly from that site.")],
                vec![Check("Compact list (one line per item)", d.compact, Control::Compact), Gap(12.0), Check("Play a soft sound when copying and pasting", d.sounds, Control::Sounds)],
            ],
            2 => vec![vec![label("History size"), History, hint("Pinned items are always kept and don't count toward this.")]],
            _ => {
                let (status, ready) = {
                    let u = net::UPDATE.lock().unwrap();
                    (u.status.clone(), u.ready.is_some())
                };
                let status = if status.is_empty() { "Not checked yet.".to_string() } else { status };
                let mut buttons = vec![("Check now", false, Control::CheckNow)];
                if ready {
                    buttons.push(("Restart and update", true, Control::Install));
                }
                buttons.push(("What's new", false, Control::WhatsNew));
                vec![
                    vec![Label(format!("ClipVault {}", net::VERSION)), Hint(status), Gap(12.0), Buttons(buttons)],
                    vec![Check("Update automatically", d.auto_update, Control::AutoUpdate), hint("Checks GitHub every few hours. New versions are verified, then installed quietly the next time the popup closes.")],
                ]
            }
        }
    }

    /// Records a control for the mouse (clipped to `within`) and the Tab key.
    fn control(&mut self, area: R, c: Control, within: Option<R>) {
        let shown = within.map_or(area, |v| area.intersect(v));
        if shown.w > 0.0 && shown.h > 0.0 {
            self.hits.push((shown, c));
        }
        self.order.push((area, c));
    }

    fn ring_around(&self, s: &Surface, t: &Theme, area: R, c: Control) {
        if self.ring && self.focus == Some(c) {
            s.border(area.inset(-3.0), 8.0, t.accent, 2.0);
        }
    }

    fn button(&mut self, s: &Surface, t: &Theme, area: R, label: &str, primary: bool, c: Control, within: Option<R>) {
        if primary {
            s.fill(area, 6.0, t.accent);
        } else {
            s.panel(area, 6.0, t.panel, t.line);
        }
        if self.hover == Some(c) {
            s.fill(area, 6.0, if primary { 0x22FF_FFFF } else { t.hover });
        }
        s.text(label, area, bold(12.5), if primary { t.on_accent } else { t.fg }, Align::Center);
        self.ring_around(s, t, area, c);
        self.control(area, c, within);
    }

    fn field(&mut self, s: &Surface, t: &Theme, area: R, c: Control) {
        let focused = self.focus == Some(c);
        s.panel(area, 6.0, t.panel, if focused { t.accent } else { t.line });
        let inner = r(area.x + 10.0, area.y + 2.0, area.w - 20.0, area.h - 4.0);
        let caret = focused && self.caret_on;
        let field = if c == Control::History { &mut self.history } else { &mut self.apps };
        field.draw(s, inner, text(13.0), t.fg, t.mark, caret);
        self.control(area, c, Some(VIEW));
    }

    fn part(&mut self, s: &Surface, t: &Theme, part: Part, x: f32, y: f32, w: f32, h: f32) {
        match part {
            Part::Label(label) => s.text(&label, r(x, y, w, 18.0), bold(13.0), t.fg, Align::Left),
            Part::Hint(hint) => s.paragraph(&hint, r(x, y + 7.0, w, h), text(12.0), t.muted, Align::Left),
            Part::Gap(_) => {}
            Part::Hotkey => {
                let area = r(x, y, w, 32.0);
                let c = Control::Hotkey;
                s.panel(area, 6.0, t.panel, if self.focus == Some(c) { t.accent } else { t.line });
                let shown = settings::hotkey_text(self.draft.hotkey_modifiers, self.draft.hotkey_key);
                s.text(&shown, r(x + 10.0, y, w - 20.0, 32.0), bold(13.0), t.fg, Align::Left);
                self.control(area, c, Some(VIEW));
            }
            Part::Chips(chips) => {
                let mut cx = x;
                for (label, on, c) in chips {
                    let cw = gfx::measure(label, bold(12.0), 200.0, Wrap::Line).0.ceil() + 26.0;
                    let area = r(cx, y + 1.0, cw, 28.0);
                    if on {
                        s.panel(area, 14.0, t.accent_soft, t.accent);
                    } else {
                        s.panel(area, 14.0, if self.hover == Some(c) { t.hover } else { 0 }, t.line);
                    }
                    s.text(label, area, bold(12.0), if on { t.accent } else { t.muted }, Align::Center);
                    self.ring_around(s, t, area, c);
                    self.control(area, c, Some(VIEW));
                    cx += cw + 6.0;
                }
            }
            Part::Check(label, on, c) => {
                let tick = r(x, y + 2.0, 18.0, 18.0);
                if on {
                    s.fill(tick, 4.0, t.accent);
                    s.glyph("\u{E73E}", tick, 11.0, t.on_accent);
                } else {
                    s.panel(tick, 4.0, if self.hover == Some(c) { t.hover } else { t.panel }, t.muted);
                }
                let lw = gfx::measure(label, text(13.0), w - 28.0, Wrap::Line).0.ceil() + 1.0;
                s.text(label, r(x + 28.0, y, lw, 22.0), text(13.0), t.fg, Align::Left);
                let area = r(x, y, 28.0 + lw, 22.0);
                self.ring_around(s, t, area, c);
                self.control(area, c, Some(VIEW));
            }
            Part::PlainApps => self.field(s, t, r(x, y, w, 32.0), Control::PlainApps),
            Part::History => {
                self.field(s, t, r(x, y, 90.0, 32.0), Control::History);
                s.text("items kept", r(x + 100.0, y, w - 100.0, 32.0), text(13.0), t.muted, Align::Left);
            }
            Part::Buttons(buttons) => {
                let mut bx = x;
                for (label, primary, c) in buttons {
                    let bw = gfx::measure(label, bold(12.5), 200.0, Wrap::Line).0.ceil() + 28.0;
                    self.button(s, t, r(bx, y, bw, 30.0), label, primary, c, Some(VIEW));
                    bx += bw + 8.0;
                }
            }
        }
    }

    fn paint(&mut self, s: &Surface) {
        let t = Theme::last();
        self.hits.clear();
        self.order.clear();
        s.begin(self.scale, t.surface);

        // sidebar
        let logo = r(20.0, 14.0, 26.0, 26.0);
        s.fill(logo, 7.0, t.accent);
        s.glyph("\u{E77F}", logo, 13.0, t.on_accent);
        s.text("ClipVault", r(56.0, 14.0, 120.0, 26.0), display(15.0), t.fg, Align::Left);
        for (i, (name, glyph)) in PAGES.iter().enumerate() {
            let c = Control::Nav(i);
            let area = r(12.0, 62.0 + i as f32 * 39.0, 170.0, 36.0);
            if self.page == i {
                s.fill(area, 6.0, t.selected);
                s.fill(r(area.x, area.y + 10.0, 3.0, 16.0), 1.5, t.accent);
            } else if self.hover == Some(c) {
                s.fill(area, 6.0, t.hover);
            }
            s.glyph(glyph, r(area.x + 10.0, area.y, 18.0, 36.0), 14.0, t.fg);
            s.text(name, r(area.x + 38.0, area.y, 120.0, 36.0), text(13.0), t.fg, Align::Left);
            self.ring_around(s, &t, area, c);
            self.control(area, c, None);
        }
        s.text(&format!("Version {}", net::VERSION), r(22.0, H - 36.0, 160.0, 18.0), text(11.0), t.muted, Align::Left);

        s.text(PAGES[self.page].0, r(206.0, 22.0, 360.0, 32.0), display(22.0), t.fg, Align::Left);
        let close = r(W - 44.0, 0.0, 44.0, 34.0);
        if self.hover == Some(Control::Close) {
            s.fill(close, 0.0, t.hover);
        }
        s.glyph("\u{E8BB}", close, 10.0, t.muted);

        // the page, scrolled
        let cards = self.parts();
        let inner = CARD_W - 32.0;
        let heights: Vec<Vec<f32>> = cards.iter().map(|c| c.iter().map(|p| p.height(inner)).collect()).collect();
        self.content_height = 10.0 + heights.iter().map(|c| c.iter().sum::<f32>() + 28.0 + 10.0).sum::<f32>();
        self.scroll = self.scroll.clamp(0.0, (self.content_height - VIEW.h).max(0.0));
        s.push_clip(VIEW);
        let mut y = VIEW.y + 10.0 - self.scroll.round();
        for (card, sizes) in cards.into_iter().zip(heights) {
            let h = sizes.iter().sum::<f32>() + 28.0;
            s.panel(r(VIEW.x, y, CARD_W, h), 8.0, t.panel, t.line);
            let mut py = y + 14.0;
            for (part, size) in card.into_iter().zip(sizes) {
                self.part(s, &t, part, VIEW.x + 16.0, py, inner, size);
                py += size;
            }
            y += h + 10.0;
        }
        s.pop_clip();
        if self.content_height > VIEW.h + 0.5 {
            let thumb = (VIEW.h * VIEW.h / self.content_height).max(28.0);
            let top = VIEW.y + (VIEW.h - thumb) * self.scroll / (self.content_height - VIEW.h);
            s.fill(r(W - 10.0, top, 4.0, thumb), 2.0, t.thumb);
        }

        self.button(s, &t, r(W - 18.0 - 168.0, H - 48.0, 80.0, 30.0), "Cancel", false, Control::Cancel, None);
        self.button(s, &t, r(W - 18.0 - 80.0, H - 48.0, 80.0, 30.0), "Save", true, Control::Save, None);
        // reachable with the keyboard too, after everything else
        self.ring_around(s, &t, close, Control::Close);
        self.control(close, Control::Close, None);
        s.end();
    }

    fn hit(&self, x: f32, y: f32) -> Option<Control> {
        self.hits.iter().rev().find(|(a, _)| a.contains(x, y)).map(|h| h.1)
    }
}

/// Saves what the window holds. Runs as a task, so message boxes are safe.
fn apply(draft: Settings, startup: bool, hwnd: HWND) {
    let Some(old) = with(|a| a.settings.clone()) else { return };
    if (draft.hotkey_modifiers, draft.hotkey_key) != (old.hotkey_modifiers, old.hotkey_key) && !app::register_hotkey(draft.hotkey_modifiers, draft.hotkey_key) {
        app::register_hotkey(old.hotkey_modifiers, old.hotkey_key); // put the old one back
        let text = settings::hotkey_text(draft.hotkey_modifiers, draft.hotkey_key);
        return system::warn(Some(hwnd), &format!("{text} is already used by Windows or another app. Try a different one."));
    }
    with(|a| {
        // the popup's size and place may have changed while this window was open
        let mut new = a.settings.clone();
        new.hotkey_modifiers = draft.hotkey_modifiers;
        new.hotkey_key = draft.hotkey_key;
        new.max_history = draft.max_history;
        new.theme = draft.theme;
        new.open_at = draft.open_at;
        new.use_system_accent = draft.use_system_accent;
        new.link_icons = draft.link_icons;
        new.clean_links = draft.clean_links;
        new.plain_text_apps = draft.plain_text_apps;
        new.compact = draft.compact;
        new.sounds = draft.sounds;
        new.auto_update = draft.auto_update;
        a.apply_settings(new);
        a.register_hotkey();
    });
    if startup != settings::startup_enabled() {
        settings::set_startup(startup);
    }
    close();
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let at = |ui: &Ui| (f32::from(lp.0 as i16) / ui.scale, f32::from((lp.0 >> 16) as i16) / ui.scale);
    let handled = match msg {
        WM_NCCALCSIZE if wp.0 != 0 => Some(0),
        WM_NCACTIVATE => Some(1),
        WM_NCHITTEST => with_ui(|ui| {
            let mut p = POINT { x: i32::from(lp.0 as i16), y: i32::from((lp.0 >> 16) as i16) };
            unsafe {
                let _ = ScreenToClient(h, &mut p);
            }
            let (x, y) = (p.x as f32 / ui.scale, p.y as f32 / ui.scale);
            // the top strip drags the window
            if y < 44.0 && ui.hit(x, y).is_none() { HTCAPTION as isize } else { HTCLIENT as isize }
        }),
        WM_ERASEBKGND => Some(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(h, &mut ps) };
            with_ui(|ui| {
                let (w, height) = ((W * ui.scale) as i32, (H * ui.scale) as i32);
                if ui.surface.as_ref().is_none_or(|s| s.w != w || s.h != height) {
                    ui.surface = Surface::new(w, height);
                }
                if let Some(s) = ui.surface.take() {
                    ui.paint(&s);
                    unsafe {
                        let _ = BitBlt(dc, 0, 0, s.w, s.h, Some(s.dc), 0, 0, SRCCOPY);
                    }
                    ui.surface = Some(s);
                }
            });
            unsafe {
                let _ = EndPaint(h, &ps);
            }
            Some(0)
        }
        WM_DPICHANGED => {
            let dpi = unsafe { GetDpiForWindow(h) };
            with_ui(|ui| ui.scale = dpi as f32 / 96.0);
            let r = unsafe { &*(lp.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(h, None, r.left, r.top, (W * dpi as f32 / 96.0) as i32, (H * dpi as f32 / 96.0) as i32, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            Some(0)
        }
        WM_ACTIVATE => {
            let active = (wp.0 & 0xFFFF) as u32 != WA_INACTIVE;
            with_ui(|ui| {
                if ui.focus == Some(Control::Hotkey) {
                    if active {
                        app::suspend_hotkey();
                    } else {
                        ui.set_focus(None);
                    }
                }
            });
            repaint();
            None
        }
        WM_TIMER => with_ui(|ui| {
            ui.caret_on = !ui.caret_on;
            if matches!(ui.focus, Some(Control::History | Control::PlainApps)) {
                repaint();
            }
            0
        }),
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            let used = with_ui(|ui| {
                // Alt+F4 closes the window unless a shortcut is being recorded
                if vk == VK_F4 && msg == WM_SYSKEYDOWN && ui.focus != Some(Control::Hotkey) {
                    return false;
                }
                let used = ui.key(vk);
                ui.swallow_char = used;
                repaint();
                used
            });
            (used == Some(true)).then_some(0)
        }
        WM_CHAR => with_ui(|ui| {
            if !std::mem::take(&mut ui.swallow_char) {
                match ui.focus {
                    Some(Control::History) => {
                        ui.history.character(wp.0 as u16);
                    }
                    Some(Control::PlainApps) => {
                        ui.apps.character(wp.0 as u16);
                    }
                    _ => {}
                }
                repaint();
            }
            0
        }),
        WM_SYSCHAR => Some(0),
        WM_IME_STARTCOMPOSITION => {
            with_ui(|ui| {
                let field = if ui.focus == Some(Control::History) { &ui.history } else { &ui.apps };
                let (x, y, _) = field.caret_point();
                set_ime_position(h, (x * ui.scale) as i32, (y * ui.scale) as i32);
            });
            None
        }
        WM_MOUSEMOVE => with_ui(|ui| {
            let (x, y) = at(ui);
            match ui.pressed {
                Some(Control::History) => ui.history.drag(x, y),
                Some(Control::PlainApps) => ui.apps.drag(x, y),
                _ => {
                    let over = ui.hit(x, y);
                    if over == ui.hover {
                        return 0;
                    }
                    ui.hover = over;
                }
            }
            repaint();
            0
        }),
        WM_LBUTTONDOWN => {
            unsafe {
                SetCapture(h);
            }
            with_ui(|ui| {
                let (x, y) = at(ui);
                let hit = ui.hit(x, y);
                ui.pressed = hit;
                ui.ring = false;
                ui.set_focus(hit);
                let shift = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
                match hit {
                    Some(Control::History) => ui.history.press(x, y, shift, 1),
                    Some(Control::PlainApps) => ui.apps.press(x, y, shift, 1),
                    _ => {}
                }
                repaint();
            });
            Some(0)
        }
        WM_LBUTTONDBLCLK => Some(0),
        WM_LBUTTONUP => {
            with_ui(|ui| {
                let (x, y) = at(ui);
                if let Some(c) = ui.pressed.take() {
                    if ui.hit(x, y) == Some(c) {
                        ui.activate(c);
                    }
                }
                repaint();
            });
            unsafe {
                let _ = ReleaseCapture();
            }
            Some(0)
        }
        WM_MOUSEWHEEL => with_ui(|ui| {
            ui.scroll = (ui.scroll - f32::from((wp.0 >> 16) as i16) / 120.0 * 54.0).clamp(0.0, (ui.content_height - VIEW.h).max(0.0));
            repaint();
            0
        }),
        WM_DESTROY => {
            OPEN.set(0);
            UI.with_borrow_mut(|u| *u = None);
            run_later(|| {
                with(|a| a.register_hotkey());
                app::popup_hidden();
            });
            Some(0)
        }
        _ => None,
    };
    match handled {
        Some(v) => LRESULT(v),
        None => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

/// One page of the window, drawn without opening it.
#[cfg(test)]
pub fn snapshot(draft: Settings, page: usize, scale: f32) -> Surface {
    let mut history = TextBox::new(false);
    history.set(&draft.max_history.to_string());
    let mut ui = Ui::new(HWND::default(), draft, scale, history, TextBox::new(false));
    ui.page = page;
    ui.ring = true;
    ui.focus = Some(Control::Save);
    let surface = Surface::new((W * scale) as i32, (H * scale) as i32).expect("surface");
    ui.paint(&surface);
    surface
}
