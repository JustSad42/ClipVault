//! Drawing: Direct2D (software) and DirectWrite into a memory bitmap that windows copy to the
//! screen. No graphics device is created, so no GPU driver is loaded into the process.
//! The bitmap keeps per-pixel transparency, so Windows' acrylic shows through.
//!
//! All coordinates are design units (96 dpi); `begin` sets the scale. Adapted from Tempo.

use crate::win::wide;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows_core::PCWSTR;
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const fn r(x: f32, y: f32, w: f32, h: f32) -> R {
    R { x, y, w, h }
}

impl R {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn inset(&self, d: f32) -> R {
        r(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    /// The overlap of two rectangles (empty when they don't meet).
    pub fn intersect(&self, o: R) -> R {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        r(x, y, (self.right().min(o.right()) - x).max(0.0), (self.bottom().min(o.bottom()) - y).max(0.0))
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

/// `colour` (0xAARRGGBB) with its alpha multiplied by `opacity`.
pub fn fade(colour: u32, opacity: f32) -> u32 {
    let a = ((colour >> 24) as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
    (colour & 0x00FF_FFFF) | (a << 24)
}

fn color(c: u32) -> D2D1_COLOR_F {
    let f = |s: u32| (c >> s & 255) as f32 / 255.0;
    D2D1_COLOR_F { r: f(16), g: f(8), b: f(0), a: f(24) }
}

/// A picture ready to draw: premultiplied BGRA.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    Text,
    Display,
    Icon,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub face: Face,
    pub size: f32,
    pub weight: u32,
}

pub const fn text(size: f32) -> Font {
    Font { face: Face::Text, size, weight: 400 }
}
pub const fn bold(size: f32) -> Font {
    Font { face: Face::Text, size, weight: 600 }
}
pub const fn display(size: f32) -> Font {
    Font { face: Face::Display, size, weight: 600 }
}
pub const fn icon(size: f32) -> Font {
    Font { face: Face::Icon, size, weight: 400 }
}
pub const fn mono(size: f32) -> Font {
    Font { face: Face::Mono, size, weight: 400 }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

struct Factories {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    /// Family names that exist here, in `Face` order.
    families: [Vec<u16>; 4],
    formats: RefCell<HashMap<(Face, u32, u32), IDWriteTextFormat>>,
}

impl Factories {
    fn new() -> windows_core::Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut fonts = None;
            dw.GetSystemFontCollection(&mut fonts, false)?;
            let pick = |names: &[&str]| -> Vec<u16> {
                let exists = |n: &str| {
                    let (mut index, mut found) = (0, Default::default());
                    fonts.as_ref().is_some_and(|f| f.FindFamilyName(PCWSTR(wide(n).as_ptr()), &mut index, &mut found).is_ok() && found.into())
                };
                wide(names.iter().find(|n| exists(n)).unwrap_or(&names[names.len() - 1]))
            };
            let families = [
                pick(&["Segoe UI Variable Text", "Segoe UI"]),
                pick(&["Segoe UI Variable Display", "Segoe UI"]),
                pick(&["Segoe Fluent Icons", "Segoe MDL2 Assets"]),
                pick(&["Cascadia Mono", "Consolas"]),
            ];
            Ok(Factories { d2d, dw, families, formats: Default::default() })
        }
    }

    fn format(&self, f: Font) -> Option<IDWriteTextFormat> {
        let key = (f.face, (f.size * 10.0) as u32, f.weight);
        if let Some(x) = self.formats.borrow().get(&key) {
            return Some(x.clone());
        }
        let made = unsafe {
            let family = PCWSTR(self.families[f.face as usize].as_ptr());
            let locale = wide("en-us");
            self.dw
                .CreateTextFormat(family, None, DWRITE_FONT_WEIGHT(f.weight as i32), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, f.size, PCWSTR(locale.as_ptr()))
                .ok()?
        };
        self.formats.borrow_mut().insert(key, made.clone());
        Some(made)
    }
}

thread_local!(static FACT: &'static Factories = Box::leak(Box::new(Factories::new().expect("Direct2D"))));

fn fact() -> &'static Factories {
    FACT.with(|f| *f)
}

/// How a block of text is laid out.
#[derive(Clone, Copy, PartialEq)]
pub enum Wrap {
    /// One line, vertically centred, trimmed with an ellipsis.
    Line,
    /// Wrapped to the width, from the top.
    Words,
}

/// A laid-out block of text, with the questions an editor asks about it. Positions are UTF-16.
pub struct Layout(pub IDWriteTextLayout);

impl Layout {
    pub fn new(s: &[u16], f: Font, w: f32, h: f32, align: Align, wrap: Wrap) -> Option<Layout> {
        let format = fact().format(f)?;
        unsafe {
            let l = fact().dw.CreateTextLayout(s, &format, w.max(0.0), h.max(0.0)).ok()?;
            let _ = l.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            match wrap {
                Wrap::Words => {
                    let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
                }
                Wrap::Line => {
                    let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                    let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                    if let Ok(ellipsis) = fact().dw.CreateEllipsisTrimmingSign(&format) {
                        let _ = l.SetTrimming(&trim, &ellipsis);
                    }
                }
            }
            Some(Layout(l))
        }
    }

    /// (width including trailing spaces, height)
    pub fn size(&self) -> (f32, f32) {
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = self.0.GetMetrics(&mut m);
        }
        (m.widthIncludingTrailingWhitespace, m.height)
    }

    /// The text position nearest a point.
    pub fn position_at(&self, x: f32, y: f32) -> usize {
        let (mut trailing, mut inside) = (Default::default(), Default::default());
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.0.HitTestPoint(x, y, &mut trailing, &mut inside, &mut m);
        }
        m.textPosition as usize + if trailing.as_bool() { m.length as usize } else { 0 }
    }

    /// Where the caret sits before `position`: (x, y, line height).
    pub fn caret(&self, position: usize) -> (f32, f32, f32) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.0.HitTestTextPosition(position as u32, false, &mut x, &mut y, &mut m);
        }
        (x, y, m.height)
    }

    /// The boxes covering a range, one or more per line.
    pub fn boxes(&self, start: usize, len: usize) -> Vec<R> {
        if len == 0 {
            return vec![];
        }
        let mut got = 0u32;
        let mut m = vec![DWRITE_HIT_TEST_METRICS::default(); 16];
        unsafe {
            if self.0.HitTestTextRange(start as u32, len as u32, 0.0, 0.0, Some(&mut m), &mut got).is_err() {
                m.resize(got as usize, Default::default());
                if self.0.HitTestTextRange(start as u32, len as u32, 0.0, 0.0, Some(&mut m), &mut got).is_err() {
                    return vec![];
                }
            }
        }
        m.truncate(got as usize);
        m.iter().map(|m| r(m.left, m.top, m.width, m.height)).collect()
    }
}

/// Size of a text block in design units.
pub fn measure(s: &str, f: Font, max_width: f32, wrap: Wrap) -> (f32, f32) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let Some(l) = Layout::new(&wide, f, max_width, 100_000.0, Align::Left, wrap) else { return (0.0, f.size * 1.4) };
    let (w, h) = l.size();
    (w.min(max_width), h)
}

/// A top-down 32-bit premultiplied bitmap with a Direct2D target on it.
pub struct Surface {
    pub dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    #[cfg_attr(not(test), allow(dead_code))]
    bits: *const u8,
    pub w: i32,
    pub h: i32,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    scale: Cell<f32>,
    /// Uploaded images, keyed by the image they came from (kept alive so the key stays unique).
    images: RefCell<HashMap<usize, (Rc<Image>, ID2D1Bitmap)>>,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Option<Surface> {
        let (w, h) = (w.max(1), h.max(1));
        unsafe {
            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let old = SelectObject(dc, bitmap.into());
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let rt = fact().d2d.CreateDCRenderTarget(&props).ok()?;
            rt.BindDC(dc, &RECT { left: 0, top: 0, right: w, bottom: h }).ok()?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&color(0xFFFF_FFFF), None).ok()?;
            Some(Surface { dc, bitmap, old, bits: bits as *const u8, w, h, rt, brush, scale: 1.0.into(), images: Default::default() })
        }
    }

    /// The pixels as drawn (valid after `end`).
    #[cfg(test)]
    pub fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bits, (self.w * self.h * 4) as usize) }
    }

    pub fn begin(&self, scale: f32, clear: u32) {
        self.scale.set(scale);
        unsafe {
            self.rt.BeginDraw();
            self.rt.SetTransform(&Matrix3x2::identity());
            self.rt.Clear(Some(&color(clear)));
        }
        self.offset(0.0, 0.0);
    }

    /// Everything after this is drawn moved by (dx, dy).
    pub fn offset(&self, dx: f32, dy: f32) {
        let s = self.scale.get();
        let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: (dx * s).round(), M32: (dy * s).round() };
        unsafe { self.rt.SetTransform(&m) }
    }

    pub fn end(&self) {
        unsafe {
            let _ = self.rt.EndDraw(None, None);
            let _ = GdiFlush();
        }
    }

    fn solid(&self, c: u32) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&color(c)) };
        &self.brush
    }

    fn rounded(rect: R, radius: f32) -> D2D1_ROUNDED_RECT {
        let radius = radius.min(rect.w / 2.0).min(rect.h / 2.0);
        D2D1_ROUNDED_RECT { rect: rect.d2d(), radiusX: radius, radiusY: radius }
    }

    pub fn fill(&self, rect: R, radius: f32, c: u32) {
        if rect.w > 0.0 && rect.h > 0.0 && c >> 24 != 0 {
            unsafe { self.rt.FillRoundedRectangle(&Self::rounded(rect, radius), self.solid(c)) }
        }
    }

    /// A 1-unit-wide (or `width`) border just inside `rect`.
    pub fn border(&self, rect: R, radius: f32, c: u32, width: f32) {
        if c >> 24 != 0 {
            let rr = Self::rounded(rect.inset(width / 2.0), (radius - width / 2.0).max(0.0));
            unsafe { self.rt.DrawRoundedRectangle(&rr, self.solid(c), width, None) }
        }
    }

    /// A box with a fill and a hairline border.
    pub fn panel(&self, rect: R, radius: f32, fill: u32, line: u32) {
        self.fill(rect, radius, fill);
        self.border(rect, radius, line, 1.0);
    }

    pub fn dashed(&self, rect: R, c: u32) {
        let dash = D2D1_STROKE_STYLE_PROPERTIES { dashStyle: D2D1_DASH_STYLE_DASH, ..Default::default() };
        unsafe {
            if let Ok(style) = fact().d2d.CreateStrokeStyle(&dash, None) {
                self.rt.DrawRectangle(&rect.d2d(), self.solid(c), 1.5, &style);
            }
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: u32) {
        if c >> 24 != 0 {
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
            unsafe { self.rt.FillEllipse(&e, self.solid(c)) }
        }
    }

    fn upload(&self, img: &Rc<Image>) -> Option<ID2D1Bitmap> {
        let key = Rc::as_ptr(img) as usize;
        if let Some((_, b)) = self.images.borrow().get(&key) {
            return Some(b.clone());
        }
        if img.w == 0 || img.h == 0 {
            return None;
        }
        let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
        let made = unsafe { self.rt.CreateBitmap(D2D_SIZE_U { width: img.w, height: img.h }, Some(img.px.as_ptr() as _), img.w * 4, &props).ok()? };
        let mut cache = self.images.borrow_mut();
        let held: usize = cache.values().map(|(i, _)| i.px.len()).sum();
        if cache.len() >= 128 || held > 48 << 20 {
            cache.clear();
        }
        cache.insert(key, (img.clone(), made.clone()));
        Some(made)
    }

    /// Draws the whole image stretched into `rect`.
    pub fn image(&self, img: &Rc<Image>, rect: R, opacity: f32) {
        if let Some(b) = self.upload(img) {
            unsafe { self.rt.DrawBitmap(&b, Some(&rect.d2d()), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) }
        }
    }

    /// Fills a rounded rectangle with the image, scaled to cover it and centred.
    pub fn cover(&self, img: &Rc<Image>, rect: R, radius: f32) {
        let Some(b) = self.upload(img) else { return };
        let k = (rect.w / img.w as f32).max(rect.h / img.h as f32);
        let (dx, dy) = (rect.x + (rect.w - img.w as f32 * k) / 2.0, rect.y + (rect.h - img.h as f32 * k) / 2.0);
        let bitmap = D2D1_BITMAP_BRUSH_PROPERTIES { extendModeX: D2D1_EXTEND_MODE_CLAMP, extendModeY: D2D1_EXTEND_MODE_CLAMP, interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR };
        let brush = D2D1_BRUSH_PROPERTIES { opacity: 1.0, transform: Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: dx, M32: dy } };
        unsafe {
            if let Ok(bb) = self.rt.CreateBitmapBrush(&b, Some(&bitmap), Some(&brush)) {
                self.rt.FillRoundedRectangle(&Self::rounded(rect, radius), &bb);
            }
        }
    }

    /// Clips everything until `pop_clip` to a rectangle.
    pub fn push_clip(&self, rect: R) {
        unsafe { self.rt.PushAxisAlignedClip(&rect.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn pop_clip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() }
    }

    /// Draws everything until `pop_layer` at this opacity.
    pub fn push_opacity(&self, opacity: f32) {
        let p = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F { left: -1e6, top: -1e6, right: 1e6, bottom: 1e6 },
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity,
            ..Default::default()
        };
        unsafe {
            let layer = self.rt.CreateLayer(None).ok();
            self.rt.PushLayer(&p, layer.as_ref());
        }
    }

    pub fn pop_layer(&self) {
        unsafe { self.rt.PopLayer() }
    }

    pub fn layout(&self, l: &Layout, x: f32, y: f32, c: u32) {
        unsafe { self.rt.DrawTextLayout(Vector2 { X: x, Y: y }, &l.0, self.solid(c), D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT) }
    }

    fn draw(&self, s: &str, rect: R, f: Font, c: u32, align: Align, wrap: Wrap) {
        if c >> 24 == 0 || rect.w <= 0.0 {
            return;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        if let Some(l) = Layout::new(&w, f, rect.w, rect.h, align, wrap) {
            self.layout(&l, rect.x, rect.y, c);
        }
    }

    /// One line, vertically centred, trimmed with an ellipsis.
    pub fn text(&self, s: &str, rect: R, f: Font, c: u32, align: Align) {
        self.draw(s, rect, f, c, align, Wrap::Line)
    }

    /// Wrapped from the top of `rect`.
    pub fn paragraph(&self, s: &str, rect: R, f: Font, c: u32, align: Align) {
        self.draw(s, rect, f, c, align, Wrap::Words)
    }

    /// Text with some parts marked (character ranges): a coloured background and bolder letters.
    pub fn marked(&self, s: &str, rect: R, f: Font, c: u32, marks: &[(usize, usize)], mark: u32, wrap: Wrap) {
        let w: Vec<u16> = s.encode_utf16().collect();
        let Some(l) = Layout::new(&w, f, rect.w, rect.h, Align::Left, wrap) else { return };
        // character positions to UTF-16 positions
        let mut at = Vec::with_capacity(s.len() + 1);
        let mut n = 0;
        for ch in s.chars() {
            at.push(n);
            n += ch.len_utf16();
        }
        at.push(n);
        for &(start, len) in marks {
            let (Some(&a), Some(&b)) = (at.get(start), at.get(start + len)) else { continue };
            unsafe {
                let _ = l.0.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_RANGE { startPosition: a as u32, length: (b - a) as u32 });
            }
            for bx in l.boxes(a, b - a) {
                if bx.x < rect.w {
                    self.fill(r(rect.x + bx.x, rect.y + bx.y, bx.w.min(rect.w - bx.x), bx.h), 2.0, mark);
                }
            }
        }
        self.layout(&l, rect.x, rect.y, c);
    }

    /// An icon-font glyph centred in `rect`.
    pub fn glyph(&self, g: &str, rect: R, size: f32, c: u32) {
        self.draw(g, rect, icon(size), c, Align::Center, Wrap::Line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_without_a_window() {
        let s = Surface::new(40, 20).unwrap();
        s.begin(1.0, 0);
        s.fill(r(0.0, 0.0, 20.0, 20.0), 0.0, 0x80FF_0000);
        s.marked("hello", r(0.0, 0.0, 40.0, 20.0), text(12.0), 0xFFFF_FFFF, &[(1, 2)], 0x4000_00FF, Wrap::Line);
        s.end();
        assert_eq!(&s.pixels()[(10 * 40 + 19) * 4 + 3..][..1], &[0x80], "half-transparent pixels stay half-transparent");
        let l = Layout::new(&"ab\ncd".encode_utf16().collect::<Vec<_>>(), text(12.0), 100.0, 100.0, Align::Left, Wrap::Words).unwrap();
        let (_, y, h) = l.caret(4);
        assert!(y > 0.0 && h > 0.0, "the caret after 'c' is on the second line");
        assert_eq!(l.position_at(0.0, y + 1.0), 3);
    }
}
