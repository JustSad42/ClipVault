//! A text field (search box, editor, settings fields): caret, selection, mouse, keyboard,
//! undo and copy/paste, drawn with DirectWrite. Positions are UTF-16 code units.

use crate::gfx::{Align, Font, Layout, R, Surface, Wrap, r};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// What a key or click did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Did {
    Nothing,
    /// The caret or selection moved.
    Moved,
    /// The text changed.
    Changed,
}

/// The field's way to the clipboard.
pub trait Clip {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: &str);
}

#[derive(Default)]
pub struct TextBox {
    text: Vec<u16>,
    pub caret: usize,
    pub anchor: usize,
    pub multiline: bool,
    pub digits_only: bool,
    scroll_x: f32,
    pub scroll_y: f32,
    undo: Vec<(Vec<u16>, usize)>,
    redo: Vec<(Vec<u16>, usize)>,
    /// The last edit was ordinary typing, so the next letter joins its undo step.
    typing: bool,
    /// Layout of the text for the width and font last drawn.
    cache: Option<(f32, Font, Layout)>,
    /// Where it was last drawn (design units), for the mouse and the input method.
    pub area: R,
    /// Keep the caret in view on the next draw.
    follow: bool,
    /// Remembered column when moving up and down.
    goal_x: Option<f32>,
}

fn is_word(c: u16) -> bool {
    char::from_u32(u32::from(c)).is_none_or(|c| c.is_alphanumeric() || c == '_')
}

impl TextBox {
    pub fn new(multiline: bool) -> TextBox {
        TextBox { multiline, ..Default::default() }
    }

    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replaces everything, with no undo history; the caret goes to the end.
    pub fn set(&mut self, text: &str) {
        self.text = text.encode_utf16().collect();
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
        self.cache = None;
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
        self.follow = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// One step left or right, never splitting a surrogate pair or a CR LF.
    fn step(&self, from: usize, forward: bool) -> usize {
        let t = &self.text;
        let pair = |a: usize| a + 1 < t.len() && ((0xD800..0xDC00).contains(&t[a]) && (0xDC00..0xE000).contains(&t[a + 1]) || t[a] == 13 && t[a + 1] == 10);
        if forward {
            if from >= t.len() { t.len() } else if pair(from) { from + 2 } else { from + 1 }
        } else if from >= 2 && pair(from - 2) {
            from - 2
        } else {
            from.saturating_sub(1)
        }
    }

    fn word_step(&self, from: usize, forward: bool) -> usize {
        let t = &self.text;
        let space = |c: u16| c == 32 || c == 9 || c == 10 || c == 13;
        let mut i = from;
        if forward {
            if i < t.len() {
                let word = is_word(t[i]);
                while i < t.len() && !space(t[i]) && is_word(t[i]) == word {
                    i += 1;
                }
                while i < t.len() && space(t[i]) {
                    i += 1;
                }
            }
        } else {
            while i > 0 && space(t[i - 1]) {
                i -= 1;
            }
            if i > 0 {
                let word = is_word(t[i - 1]);
                while i > 0 && !space(t[i - 1]) && is_word(t[i - 1]) == word {
                    i -= 1;
                }
            }
        }
        i
    }

    /// Double-click: the word (or run of spaces) around a position.
    pub fn select_word(&mut self, at: usize) {
        let t = &self.text;
        let at = at.min(t.len());
        let kind = |c: u16| if c == 10 || c == 13 { 0 } else if c == 32 || c == 9 { 1 } else if is_word(c) { 2 } else { 3 };
        let probe = if at < t.len() && kind(t[at]) != 0 { at } else if at > 0 { at - 1 } else { at };
        let Some(&c) = t.get(probe) else { return };
        let k = kind(c);
        let (mut a, mut b) = (probe, probe + 1);
        while a > 0 && kind(t[a - 1]) == k {
            a -= 1;
        }
        while b < t.len() && kind(t[b]) == k {
            b += 1;
        }
        self.anchor = a;
        self.caret = b;
    }

    /// Triple-click: the whole line (everything in a single-line field).
    pub fn select_line(&mut self, at: usize) {
        let at = at.min(self.text.len());
        let start = self.text[..at].iter().rposition(|&c| c == 10).map_or(0, |i| i + 1);
        let end = self.text[at..].iter().position(|&c| c == 10).map_or(self.text.len(), |i| at + i + 1);
        self.anchor = start;
        self.caret = end;
    }

    fn remember(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push((self.text.clone(), self.caret));
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
        self.typing = typing;
        self.redo.clear();
    }

    /// Replaces the selection.
    fn replace(&mut self, with: &[u16], typing: bool) -> Did {
        let (a, b) = self.selection();
        if a == b && with.is_empty() {
            return Did::Nothing;
        }
        self.remember(typing && a == b);
        self.text.splice(a..b, with.iter().copied());
        self.caret = a + with.len();
        self.anchor = self.caret;
        self.changed()
    }

    fn changed(&mut self) -> Did {
        self.cache = None;
        self.follow = true;
        self.goal_x = None;
        Did::Changed
    }

    pub fn insert(&mut self, s: &str) -> Did {
        let mut s = if self.multiline {
            s.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n")
        } else {
            s.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ")
        };
        s.retain(|c| c != '\0');
        if self.digits_only {
            s.retain(|c| c.is_ascii_digit());
        }
        self.replace(&s.encode_utf16().collect::<Vec<_>>(), false)
    }

    /// A typed character (from `WM_CHAR`, so surrogate pairs arrive as two calls).
    pub fn character(&mut self, c: u16) -> Did {
        // control characters are keys, handled in `key`
        if c < 32 || c == 127 || (self.digits_only && !(48..58).contains(&c)) {
            return Did::Nothing;
        }
        let boundary = c == 32;
        let did = self.replace(&[c], true);
        if boundary {
            self.typing = false;
        }
        did
    }

    fn go(&mut self, to: usize, shift: bool) -> Did {
        self.caret = to.min(self.text.len());
        if !shift {
            self.anchor = self.caret;
        }
        self.typing = false;
        self.follow = true;
        Did::Moved
    }

    fn line_move(&mut self, lines: i32, shift: bool) -> Did {
        let Some((_, _, layout)) = &self.cache else { return Did::Nothing };
        let (x, y, h) = layout.caret(self.caret);
        let x = *self.goal_x.get_or_insert(x);
        let target = y + h * lines as f32 + h / 2.0;
        let to = if target < 0.0 { 0 } else if target > layout.size().1 { self.text.len() } else { layout.position_at(x, target) };
        self.go(to, shift)
    }

    /// A key press. `Did::Nothing` means the key isn't the field's business.
    pub fn key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool, clip: &mut dyn Clip) -> Did {
        let (a, b) = self.selection();
        if !matches!(vk, VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT) {
            self.goal_x = None;
        }
        match vk {
            VK_LEFT if !shift && !ctrl && a != b => self.go(a, false),
            VK_RIGHT if !shift && !ctrl && a != b => self.go(b, false),
            VK_LEFT => self.go(if ctrl { self.word_step(self.caret, false) } else { self.step(self.caret, false) }, shift),
            VK_RIGHT => self.go(if ctrl { self.word_step(self.caret, true) } else { self.step(self.caret, true) }, shift),
            VK_UP if self.multiline => self.line_move(-1, shift),
            VK_DOWN if self.multiline => self.line_move(1, shift),
            VK_PRIOR if self.multiline => self.line_move(-((self.area.h / 17.0) as i32).max(1), shift),
            VK_NEXT if self.multiline => self.line_move(((self.area.h / 17.0) as i32).max(1), shift),
            VK_HOME | VK_END => {
                let to = match (&self.cache, vk == VK_HOME) {
                    (Some((_, _, l)), home) if self.multiline && !ctrl => {
                        let (_, y, h) = l.caret(self.caret);
                        l.position_at(if home { -1.0 } else { 1e6 }, y + h / 2.0)
                    }
                    (_, true) => 0,
                    (_, false) => self.text.len(),
                };
                // End on a line that ends in CR LF stays before the break
                let to = if vk == VK_END && to > 0 && self.text.get(to - 1) == Some(&10) && to < self.text.len() { self.step(to, false) } else { to };
                self.go(to, shift)
            }
            VK_BACK if a == b => {
                self.anchor = if ctrl { self.word_step(self.caret, false) } else { self.step(self.caret, false) };
                self.replace(&[], false)
            }
            VK_DELETE if a == b => {
                self.anchor = if ctrl { self.word_step(self.caret, true) } else { self.step(self.caret, true) };
                self.replace(&[], false)
            }
            VK_BACK | VK_DELETE => self.replace(&[], false),
            VK_RETURN if self.multiline && !ctrl => self.replace(&[13, 10], false),
            VK_TAB if self.multiline && !ctrl => self.replace(&[9], false),
            VK_A if ctrl => {
                self.select_all();
                self.follow = false;
                Did::Moved
            }
            VK_C | VK_X | VK_INSERT if ctrl && a != b => {
                clip.set(&String::from_utf16_lossy(&self.text[a..b]));
                if vk == VK_X { self.replace(&[], false) } else { Did::Moved }
            }
            VK_C | VK_X if ctrl => Did::Moved,
            VK_V if ctrl => clip.get().map_or(Did::Moved, |t| self.insert(&t)),
            VK_INSERT if shift => clip.get().map_or(Did::Moved, |t| self.insert(&t)),
            VK_Z if ctrl && !shift => self.history(false),
            VK_Y if ctrl => self.history(true),
            VK_Z if ctrl => self.history(true),
            _ => Did::Nothing,
        }
    }

    fn history(&mut self, redo: bool) -> Did {
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let Some((text, caret)) = from.pop() else { return Did::Moved };
        to.push((std::mem::replace(&mut self.text, text), self.caret));
        self.caret = caret.min(self.text.len());
        self.anchor = self.caret;
        self.typing = false;
        self.changed()
    }

    /// The text position under a point (design units).
    pub fn position_at(&self, x: f32, y: f32) -> usize {
        let Some((_, _, l)) = &self.cache else { return 0 };
        let ly = if self.multiline { y - self.area.y + self.scroll_y } else { self.area.h / 2.0 };
        l.position_at(x - self.area.x + self.scroll_x, ly)
    }

    pub fn press(&mut self, x: f32, y: f32, shift: bool, clicks: u32) {
        let at = self.position_at(x, y);
        match clicks {
            2 => self.select_word(at),
            3.. => self.select_line(at),
            _ => {
                self.go(at, shift);
                self.follow = false;
            }
        }
        self.goal_x = None;
    }

    pub fn drag(&mut self, x: f32, y: f32) {
        self.caret = self.position_at(x, y);
        self.follow = true;
        self.typing = false;
    }

    pub fn wheel(&mut self, delta: f32) {
        self.scroll_y -= delta;
        self.follow = false;
    }

    /// The caret's top-left on screen (design units), for the input method's window.
    pub fn caret_point(&self) -> (f32, f32, f32) {
        let Some((_, _, l)) = &self.cache else { return (self.area.x, self.area.y, 16.0) };
        let (x, y, h) = l.caret(self.caret);
        let y = if self.multiline { y - self.scroll_y } else { (self.area.h - h) / 2.0 };
        (self.area.x + x - self.scroll_x, self.area.y + y, h)
    }

    /// Draws the text into `area`: the selection, the text and (if `caret`) the caret.
    pub fn draw(&mut self, s: &Surface, area: R, font: Font, colour: u32, selection: u32, caret: bool) {
        self.area = area;
        let width = if self.multiline { area.w - 2.0 } else { 100_000.0 };
        if !self.cache.as_ref().is_some_and(|(w, f, _)| *w == width && *f == font) {
            let wrap = if self.multiline { Wrap::Words } else { Wrap::Line };
            // single lines are centred by hand, so no trimming sign appears while typing
            let height = if self.multiline { 1_000_000.0 } else { area.h };
            self.cache = Layout::new(&self.text, font, width, height, Align::Left, wrap).map(|l| (width, font, l));
        }
        let Some((_, _, layout)) = &self.cache else { return };
        let (cx, cy, ch) = layout.caret(self.caret);
        if self.multiline {
            let height = layout.size().1;
            if self.follow {
                self.scroll_y = self.scroll_y.min(cy).max(cy + ch - area.h);
            }
            self.scroll_y = self.scroll_y.clamp(0.0, (height - area.h).max(0.0));
        } else {
            let length = layout.size().0;
            if self.follow {
                self.scroll_x = self.scroll_x.min(cx).max(cx - area.w + 2.0);
            }
            self.scroll_x = self.scroll_x.clamp(0.0, (length - area.w + 2.0).max(0.0));
        }
        self.follow = false;
        let (ox, oy) = (area.x - self.scroll_x, area.y - if self.multiline { self.scroll_y } else { 0.0 });
        s.push_clip(area);
        let (a, b) = self.selection();
        for bx in layout.boxes(a, b - a) {
            s.fill(r(ox + bx.x, oy + bx.y, bx.w.max(4.0), bx.h), 2.0, selection);
        }
        s.layout(layout, ox, oy, colour);
        if caret {
            s.fill(r((ox + cx).round(), oy + cy, 1.2, ch), 0.0, colour);
        }
        s.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake(String);
    impl Clip for Fake {
        fn get(&mut self) -> Option<String> {
            Some(self.0.clone())
        }
        fn set(&mut self, t: &str) {
            self.0 = t.into();
        }
    }

    #[test]
    fn editing() {
        let mut t = TextBox::new(true);
        let mut c = Fake::default();
        for ch in "hello world".encode_utf16() {
            t.character(ch);
        }
        t.key(VK_BACK, true, false, &mut c);
        assert_eq!(t.text(), "hello ");
        t.key(VK_RETURN, false, false, &mut c);
        t.insert("a\nb😀");
        assert_eq!(t.text(), "hello \r\na\r\nb😀");
        t.key(VK_BACK, false, false, &mut c);
        assert_eq!(t.text(), "hello \r\na\r\nb", "a surrogate pair goes as one");
        t.key(VK_LEFT, false, false, &mut c);
        t.key(VK_BACK, false, false, &mut c);
        assert_eq!(t.text(), "hello \r\nab", "so does a line break");
        t.key(VK_A, true, false, &mut c);
        t.key(VK_X, true, false, &mut c);
        assert!(t.is_empty());
        t.key(VK_V, true, false, &mut c);
        assert_eq!(c.0, t.text());
        t.key(VK_Z, true, false, &mut c);
        assert!(t.is_empty(), "undo the paste");
        t.key(VK_Y, true, false, &mut c);
        assert_eq!(t.text(), "hello \r\nab");
    }

    #[test]
    fn single_line_words_and_digits() {
        let mut t = TextBox::new(false);
        t.insert("one  two\r\nthree");
        assert_eq!(t.text(), "one  two three");
        t.select_word(1);
        assert_eq!(t.selection(), (0, 3));
        t.select_word(3);
        assert_eq!(t.selection(), (3, 5));
        let mut d = TextBox::new(false);
        d.digits_only = true;
        d.insert("12ab3");
        assert_eq!(d.character(u16::from(b'x')), Did::Nothing);
        assert_eq!(d.text(), "123");
    }
}
