//! Drawing the popup. Everything pointable is recorded in `hits` as it is drawn.

use crate::app::App;
use crate::gfx::{self, Align, R, Surface, Wrap, bold, display, fade, mono, r, text};
use crate::model::{Card, Facts, Item, Kind};
use crate::popup::{Hit, Mode, Popup, Row, TABS, Tab};
use crate::theme::Theme;
use std::time::Duration;

pub const HEADING: f32 = 30.0;
pub const TILE: f32 = 112.0;
const LIST_TOP: f32 = 90.0;
const FOOTER: f32 = 40.0;
const LINE_TEXT: f32 = 19.0;
const LINE_MONO: f32 = 17.0;

const ICON_SEARCH: &str = "\u{E721}";
const ICON_CLOSE: &str = "\u{E711}";
const ICON_PIN: &str = "\u{E718}";
const ICON_PINNED: &str = "\u{E840}";
const ICON_OPEN: &str = "\u{E8A7}";
const ICON_FOLDER: &str = "\u{E838}";
const ICON_SAVE: &str = "\u{E74E}";
const ICON_MORE: &str = "\u{E712}";
const ICON_LINK: &str = "\u{E71B}";
const ICON_MAIL: &str = "\u{E715}";
const ICON_PHONE: &str = "\u{E717}";
const ICON_FILE: &str = "\u{E8B7}";
const ICON_CROP: &str = "\u{E7A8}";
const ICON_BLUR: &str = "\u{E727}";
const ICON_ROTATE: &str = "\u{E7AD}";
const ICON_UNDO: &str = "\u{E7A7}";

pub fn list_width(window: f32) -> f32 {
    window - 8.0 - 8.0
}

pub fn list_height(window: f32) -> f32 {
    window - LIST_TOP - FOOTER - 1.0
}

pub fn thumb_height(view: f32, content: f32) -> f32 {
    (view * view / content.max(1.0)).clamp(28.0, view)
}

/// The size a thumbnail is shown at: within 330×90 and the column, keeping its shape.
fn thumb_size(item: &Item, column: f32) -> (f32, f32) {
    let (w, h) = (item.pixel_width as f32, item.pixel_height as f32);
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    let k = (330f32.min(column) / w).min(90.0 / h).min(1.0);
    ((w * k).max(2.0), (h * k).max(2.0))
}

fn has_lead(card: Card) -> bool {
    matches!(card, Card::Link | Card::Color | Card::Email | Card::Phone | Card::Files)
}

/// Where a normal row's text column starts, and how wide it is.
fn column(width: f32, card: Card) -> (f32, f32) {
    let x = 42.0 + if has_lead(card) { 42.0 } else { 0.0 };
    let buttons = if matches!(card, Card::Link | Card::Files | Card::Image) { 62.0 } else { 34.0 };
    (x, (width - x - buttons - 8.0).max(40.0))
}

fn body_height(item: &Item, facts: &Facts, width: f32) -> f32 {
    match facts.card {
        Card::Image => thumb_size(item, column(width, Card::Image).1).1,
        Card::Link => LINE_TEXT + 16.0,
        _ => {
            let lines = facts.preview.split('\n').count().clamp(1, 3) as f32;
            lines * if item.use_mono(facts) { LINE_MONO } else { LINE_TEXT }
        }
    }
}

pub fn row_height(item: &Item, facts: &Facts, compact: bool, width: f32) -> f32 {
    if compact {
        return 35.0;
    }
    let body = body_height(item, facts, width).max(if has_lead(facts.card) { 32.0 } else { 0.0 });
    9.0 + 18.0 + 5.0 + body + 10.0
}

/// A shortcut drawn as a key; returns the x after it.
fn keycap(s: &Surface, t: &Theme, x: f32, y: f32, label: &str, icon: bool) -> f32 {
    let f = if icon { gfx::icon(10.0) } else { text(10.5) };
    let w = (gfx::measure(label, f, 200.0, Wrap::Line).0 + 10.0).max(18.0).ceil();
    let area = r(x, y, w, 18.0);
    s.panel(area, 4.0, t.panel, t.line);
    s.text(label, area, f, t.muted, Align::Center);
    x + w
}

fn hint(s: &Surface, t: &Theme, x: f32, y: f32, key: &str, icon: bool, what: &str) -> f32 {
    let x = keycap(s, t, x, y, key, icon) + 5.0;
    let w = gfx::measure(what, text(11.0), 300.0, Wrap::Line).0.ceil() + 1.0;
    s.text(what, r(x, y, w, 18.0), text(11.0), t.muted, Align::Left);
    x + w + 10.0
}

fn icon_button(p: &mut Popup, s: &Surface, t: &Theme, area: R, glyph: &str, colour: u32, hit: Hit) {
    if p.hover == Some(hit) {
        s.fill(area, 6.0, t.hover);
        s.fill(area, 6.0, t.hover);
    }
    s.glyph(glyph, area, if area.w < 28.0 { 12.0 } else { 13.0 }, colour);
    p.hits.push((area, hit));
}

/// A labelled button; returns its width.
fn button(p: &mut Popup, s: &Surface, t: &Theme, right: f32, y: f32, label: &str, primary: bool, hit: Hit) -> f32 {
    let w = gfx::measure(label, bold(12.5), 300.0, Wrap::Line).0.ceil() + 28.0;
    let area = r(right - w, y, w, 30.0);
    if primary {
        s.fill(area, 6.0, t.accent);
        if p.hover == Some(hit) {
            s.fill(area, 6.0, 0x22FF_FFFF);
        }
    } else {
        s.panel(area, 6.0, t.panel, t.line);
        if p.hover == Some(hit) {
            s.fill(area, 6.0, t.hover);
        }
    }
    s.text(label, area, bold(12.5), if primary { t.on_accent } else { t.fg }, Align::Center);
    p.hits.push((area, hit));
    w
}

pub fn popup(p: &mut Popup, app: &mut App, s: &Surface) {
    let t = app.theme;
    let (w, h) = (p.width, p.height);
    p.hits.clear();
    s.begin(p.scale, t.surface);
    // a very short fade and slide toward the anchor
    let appearing = p.appearing.map(|(at, above)| {
        let elapsed = at.elapsed().min(Duration::from_millis(90)).as_secs_f32();
        let k = 1.0 - (1.0 - elapsed / 0.09).powi(3);
        let opacity = 0.4 + 0.6 * (elapsed / 0.06).min(1.0);
        (if above { 4.0 } else { -4.0 } * (1.0 - k), opacity)
    });
    if let Some((dy, opacity)) = appearing {
        s.offset(0.0, dy);
        s.push_opacity(opacity);
    }

    search_box(p, s, &t, w);
    tabs(p, s, &t, w);
    let view = r(8.0, LIST_TOP, list_width(w), list_height(h));
    p.list = view;
    match p.mode {
        Mode::List => list(p, app, s, &t, view),
        Mode::Preview => preview(p, app, s, &t, r(12.0, LIST_TOP + 4.0, w - 24.0, view.h - 10.0)),
        Mode::Edit => editor(p, s, &t, r(12.0, LIST_TOP + 4.0, w - 24.0, view.h - 10.0)),
        Mode::Image => image_editor(p, s, &t, r(12.0, LIST_TOP + 4.0, w - 24.0, view.h - 10.0)),
    }
    footer(p, s, &t, w, h);

    if appearing.is_some() {
        s.pop_layer();
    }
    s.end();
}

fn search_box(p: &mut Popup, s: &Surface, t: &Theme, w: f32) {
    let area = r(12.0, 12.0, w - 24.0, 36.0);
    s.panel(area, 8.0, t.panel, t.line);
    p.hits.push((area, Hit::Search));
    s.glyph(ICON_SEARCH, r(area.x + 6.0, area.y, 24.0, area.h), 13.0, t.muted);
    let clear = !p.search.is_empty();
    let field = r(area.x + 32.0, area.y + 2.0, area.w - 32.0 - if clear { 34.0 } else { 8.0 }, area.h - 4.0);
    let focused = matches!(p.mode, Mode::List | Mode::Preview);
    if p.search.is_empty() {
        s.text("Search, or filter: app:chrome  type:link  today", r(field.x + 2.0, field.y, field.w - 2.0, field.h), text(13.0), t.muted, Align::Left);
    }
    p.search.draw(s, field, text(13.0), t.fg, t.mark, focused && p.caret_on);
    if clear {
        icon_button(p, s, t, r(area.right() - 28.0, area.y + 6.0, 24.0, 24.0), ICON_CLOSE, t.muted, Hit::Clear);
    }
}

fn tabs(p: &mut Popup, s: &Surface, t: &Theme, w: f32) {
    let y = 56.0;
    p.hits.push((r(0.0, y - 4.0, w, 34.0), Hit::Drag));
    let mut x = 12.0;
    for (i, (tab, label)) in TABS.iter().enumerate() {
        let width = gfx::measure(label, bold(12.0), 200.0, Wrap::Line).0.ceil() + 24.0;
        let area = r(x, y, width, 26.0);
        let on = p.tab == *tab;
        if on {
            s.fill(area, 13.0, t.accent_soft);
        } else if p.hover == Some(Hit::Tab(i)) {
            s.fill(area, 13.0, t.hover);
        }
        s.text(label, area, bold(12.0), if on { t.accent } else { t.muted }, Align::Center);
        p.hits.push((area, Hit::Tab(i)));
        x += width + 2.0;
    }
}

fn footer(p: &mut Popup, s: &Surface, t: &Theme, w: f32, h: f32) {
    let top = h - FOOTER;
    p.hits.push((r(0.0, top, w, FOOTER), Hit::Drag));
    s.fill(r(0.0, top, w, 1.0), 0.0, t.line);
    let y = top + 11.0;
    s.push_clip(r(0.0, top, w - 40.0, FOOTER));
    let mut x = 12.0;
    x = hint(s, t, x, y, "\u{E751}", true, "Paste");
    x = hint(s, t, x, y, "\u{E752} \u{E751}", true, "Plain");
    x = hint(s, t, x, y, "Space", false, "Preview");
    x = hint(s, t, x, y, "F2", false, "Edit");
    hint(s, t, x, y, "Tab", false, "Filter");
    s.pop_clip();
    icon_button(p, s, t, r(w - 34.0, top + 6.0, 28.0, 28.0), ICON_MORE, t.muted, Hit::More);
}

fn empty_state(p: &Popup, app: &App, s: &Surface, t: &Theme, view: R) {
    let searching = !p.query.words.is_empty() || !p.query.filters.is_empty();
    let (glyph, title, hint) = if app.store.items.is_empty() {
        ("\u{E77F}", "Nothing here yet", "Copy some text, an image or a file and it will show up here.")
    } else if searching {
        let hint = if p.tab == Tab::All { "Try fewer or different words. Text inside images is searched too." } else { "Press Tab to look in the other tabs." };
        (ICON_SEARCH, "No matches", hint)
    } else {
        match p.tab {
            Tab::Pinned => (ICON_PIN, "No pinned items", "Select an item and press Ctrl+P to keep it here for good."),
            Tab::Images => ("\u{EB9F}", "No images yet", "Screenshots and copied pictures show up here."),
            Tab::Files => (ICON_FILE, "No files yet", "Files you copy in File Explorer show up here."),
            _ => ("\u{E8D2}", "No text yet", "Text you copy shows up here."),
        }
    };
    let cx = view.x + view.w / 2.0;
    let y = view.y + (view.h - 140.0).max(0.0) / 2.0;
    let circle = r(cx - 28.0, y, 56.0, 56.0);
    s.panel(circle, 28.0, t.panel, t.line);
    s.glyph(glyph, circle, 22.0, t.muted);
    s.text(title, r(view.x, y + 68.0, view.w, 22.0), display(15.0), t.fg, Align::Center);
    let width = 280f32.min(view.w - 40.0);
    s.paragraph(hint, r(cx - width / 2.0, y + 94.0, width, 60.0), text(12.5), t.muted, Align::Center);
}

fn list(p: &mut Popup, app: &mut App, s: &Surface, t: &Theme, view: R) {
    if p.items.is_empty() {
        return empty_state(p, app, s, t, view);
    }
    s.push_clip(view);
    let grid = p.tab == Tab::Images;
    let compact = app.settings.compact && !grid;
    let now = chrono::Local::now();
    let scroll = p.scroll.round();
    let rows = std::mem::take(&mut p.rows);
    // the first row that reaches into view
    let first = rows.partition_point(|row| row.top() + TILE.max(220.0) < scroll);
    for row in rows.iter().skip(first) {
        let top = view.y + row.top() - scroll;
        if top > view.bottom() {
            break;
        }
        match row {
            Row::Heading(label, _) => s.text(label, r(view.x + 12.0, top + 9.0, view.w - 24.0, 18.0), bold(11.5), t.muted, Align::Left),
            Row::Item { index, area } => {
                let area = r(view.x + area.x, top, area.w, area.h);
                if area.bottom() < view.y {
                    continue;
                }
                let Some(item) = p.items.get(*index).and_then(|id| app.store.get(id)).cloned() else { continue };
                let hit = area.intersect(view);
                p.hits.push((hit, Hit::Row(*index)));
                if grid {
                    tile(p, app, s, t, &item, *index, area);
                } else {
                    row_of(p, app, s, t, &item, *index, area, compact, now, view);
                }
            }
        }
    }
    p.rows = rows;
    s.pop_clip();
    if p.content_height > view.h + 0.5 {
        let thumb = thumb_height(view.h, p.content_height);
        let y = view.y + (view.h - thumb) * (p.scroll / (p.content_height - view.h)).clamp(0.0, 1.0);
        let grab = r(view.right() - 4.0, view.y, 12.0, view.h);
        let wide = p.hover == Some(Hit::Scrollbar);
        s.fill(r(view.right() + if wide { 0.0 } else { 2.0 }, y, if wide { 6.0 } else { 3.0 }, thumb), 3.0, t.thumb);
        p.hits.push((grab, Hit::Scrollbar));
    }
}

/// Hovering a row's buttons still counts as hovering the row.
fn row_hovered(p: &Popup, index: usize) -> bool {
    matches!(p.hover, Some(Hit::Row(i) | Hit::Pin(i) | Hit::Action(i)) if i == index)
}

fn badge(s: &Surface, t: &Theme, area: R, index: usize, solid: bool) {
    if index >= 9 {
        return;
    }
    s.panel(area, 4.0, if solid { t.bg } else { t.panel }, t.line);
    s.text(&(index + 1).to_string(), area, text(10.5), t.muted, Align::Center);
}

fn tile(p: &mut Popup, app: &mut App, s: &Surface, t: &Theme, item: &Item, index: usize, area: R) {
    let frame = area.inset(2.0);
    let inner = frame.inset(4.0);
    s.fill(inner, 6.0, t.panel);
    if let Some(picture) = app.cache.thumbnail(item, &app.store) {
        s.cover(&picture, inner, 6.0);
    }
    if p.selected == index {
        s.border(frame, 9.0, t.accent, 2.0);
    } else if row_hovered(p, index) {
        s.border(frame, 9.0, t.line, 2.0);
    }
    badge(s, t, r(inner.x + 5.0, inner.y + 5.0, 18.0, 18.0), index, true);
    if item.pinned {
        let c = r(inner.right() - 27.0, inner.y + 5.0, 22.0, 22.0);
        s.circle(c.x + 11.0, c.y + 11.0, 11.0, t.bg);
        s.glyph(ICON_PINNED, c, 11.0, t.accent);
    }
}

#[allow(clippy::too_many_arguments)]
fn row_of(p: &mut Popup, app: &mut App, s: &Surface, t: &Theme, item: &Item, index: usize, area: R, compact: bool, now: chrono::DateTime<chrono::Local>, view: R) {
    let revision = app.store.revision(&item.id);
    let facts = app.cache.info(item, revision).facts.clone();
    let files = app.cache.files(item, revision);
    let card = facts.card;
    let selected = p.selected == index;
    let hovered = row_hovered(p, index);
    let body = r(area.x, area.y, area.w - 4.0, area.h);
    if selected {
        s.fill(body, 6.0, t.selected);
    }
    if hovered {
        s.fill(body, 6.0, t.hover);
    }
    s.fill(r(body.x + 14.0, body.bottom() - 1.0, body.w - 20.0, 1.0), 0.0, t.line);
    let (colour, soft) = t.kind(card, card == Card::Plain && facts.technical);
    s.fill(r(body.x + 2.0, body.y + 8.0, 3.0, body.h - 17.0), 1.5, fade(colour, if selected { 1.0 } else { 0.55 }));
    let words = p.query.words.clone();
    let marks = |line: &str| crate::search::ranges(line, &words);
    let mono_text = item.use_mono(&facts);
    let ago = crate::model::ago(item.copied_at, now);
    let ago_w = gfx::measure(&ago, text(11.0), 120.0, Wrap::Line).0.ceil() + 2.0;
    let action = match card {
        Card::Link => Some(ICON_OPEN),
        Card::Files => Some(ICON_FOLDER),
        Card::Image => Some(ICON_SAVE),
        _ => None,
    };
    let pin_glyph = if item.pinned { ICON_PINNED } else { ICON_PIN };
    let pin_colour = if item.pinned { t.accent } else { t.muted };

    // the little box with a site icon, colour, file picture or type sign
    let lead = |p: &mut Popup, app: &mut App, area: R, glyph_size: f32, icon_size: f32| {
        let _ = p;
        let glyph = match card {
            Card::Link => ICON_LINK,
            Card::Email => ICON_MAIL,
            Card::Phone => ICON_PHONE,
            Card::Files => ICON_FILE,
            _ => "",
        };
        let radius = if area.w > 24.0 { 7.0 } else { 5.0 };
        if card == Card::Color {
            if let Some((r8, g, b, a)) = item.color() {
                s.fill(area, radius, u32::from(a) << 24 | u32::from(r8) << 16 | u32::from(g) << 8 | u32::from(b));
                s.border(area, radius, t.line, 1.0);
            }
            return;
        }
        s.panel(area, radius, t.panel, t.line);
        let (cx, cy) = area.center();
        let centred = r(cx - icon_size / 2.0, cy - icon_size / 2.0, icon_size, icon_size);
        match card {
            Card::Link => {
                let host = item.link_host().and_then(|_| crate::model::web_host(item.text.as_deref().unwrap_or("").trim()));
                let scheme = if item.text.as_deref().unwrap_or("").trim().to_ascii_lowercase().starts_with("https") { "https" } else { "http" };
                if let Some(icon) = host.and_then(|h| app.cache.favicon(scheme, &h, app.settings.link_icons)) {
                    return s.image(&icon, centred, 1.0);
                }
            }
            Card::Files => {
                if let Some(first) = item.file_paths().first() {
                    if item.first_file_is_photo() {
                        if let Some(photo) = app.cache.photo(first) {
                            return s.cover(&photo, area, radius);
                        }
                    }
                    if files.as_ref().is_some_and(|f| f.first_exists) {
                        if let Some(icon) = app.cache.icon(first, false) {
                            return s.image(&icon, centred, 1.0);
                        }
                    }
                }
            }
            _ => {}
        }
        s.glyph(glyph, area, glyph_size, t.muted);
    };

    if compact {
        let y = body.y + 6.0;
        badge(s, t, r(body.x + 14.0, y + 3.0, 18.0, 18.0), index, false);
        let mut x = body.x + 42.0;
        if card == Card::Image {
            let thumb = r(x, y + 1.0, 36.0, 22.0);
            s.fill(thumb, 4.0, t.panel);
            if let Some(picture) = app.cache.thumbnail(item, &app.store) {
                s.cover(&picture, thumb, 4.0);
            }
            s.border(thumb, 4.0, t.line, 1.0);
            x += 45.0;
        } else if has_lead(card) {
            lead(p, app, r(x, y + 1.0, 22.0, 22.0), 11.0, 14.0);
            x += 31.0;
        }
        let buttons = if action.is_some() { 52.0 } else { 26.0 };
        let right = body.right() - 6.0 - buttons;
        s.text(&ago, r(right - ago_w - 4.0, y, ago_w, 24.0), text(11.0), t.muted, Align::Right);
        let line: String = item.preview_line(&facts).chars().take(400).collect();
        let font = if mono_text { mono(12.0) } else { text(13.0) };
        s.marked(&line, r(x, y, right - ago_w - 14.0 - x, 24.0), font, t.fg, &marks(&line), t.mark, Wrap::Line);
        let show = hovered || selected;
        let mut bx = body.right() - 6.0 - 24.0;
        if show || item.pinned {
            let b = r(bx, y, 24.0, 24.0);
            icon_button(p, s, t, b, pin_glyph, pin_colour, Hit::Pin(index));
        }
        bx -= 26.0;
        if let Some(glyph) = action.filter(|_| show) {
            icon_button(p, s, t, r(bx, y, 24.0, 24.0), glyph, t.muted, Hit::Action(index));
        }
        clip_hits(p, view);
        return;
    }

    // header: badge, app, type, info tags, time
    let y = body.y + 9.0;
    badge(s, t, r(body.x + 14.0, y, 18.0, 18.0), index, false);
    let mut x = body.x + 42.0;
    if let Some(icon) = item.source_path.as_deref().and_then(|path| app.cache.icon(path, true)) {
        s.image(&icon, r(x, y + 3.0, 12.0, 12.0), 0.85);
        x += 17.0;
    }
    if let Some(name) = item.source_name.as_deref().filter(|n| !n.trim().is_empty()) {
        let label = name.to_uppercase();
        let w = gfx::measure(&label, bold(10.5), 150.0, Wrap::Line).0.ceil() + 1.0;
        s.text(&label, r(x, y, w, 18.0), bold(10.5), t.muted, Align::Left);
        x += w + 8.0;
    }
    let time_x = body.right() - 6.0 - ago_w;
    s.text(&ago, r(time_x, y, ago_w, 18.0), text(11.0), t.muted, Align::Right);
    s.push_clip(r(x, y, (time_x - 8.0 - x).max(0.0), 18.0));
    let label = item.type_label(&facts, files.as_ref());
    let w = gfx::measure(label, bold(10.5), 120.0, Wrap::Line).0.ceil() + 12.0;
    s.fill(r(x, y + 1.0, w, 16.0), 4.0, soft);
    s.text(label, r(x, y, w, 18.0), bold(10.5), colour, Align::Center);
    x += w + 4.0;
    for tag in item.tags(&facts, files.as_ref()) {
        let w = gfx::measure(&tag, text(10.5), 200.0, Wrap::Line).0.ceil() + 12.0;
        s.panel(r(x, y + 1.0, w, 16.0), 4.0, t.panel, t.line);
        s.text(&tag, r(x, y, w, 18.0), text(10.5), t.muted, Align::Center);
        x += w + 4.0;
    }
    s.pop_clip();

    let top = y + 23.0;
    let inner_height = body.bottom() - 10.0 - top;
    let (cx, cw) = column(area.w, card);
    let cx = body.x + cx;
    if has_lead(card) {
        lead(p, app, r(body.x + 42.0, top, 32.0, 32.0), 14.0, 18.0);
    }
    match card {
        Card::Image => {
            let (w, h) = thumb_size(item, cw);
            let thumb = r(cx, top, w, h);
            s.fill(thumb, 6.0, t.panel);
            if let Some(picture) = app.cache.thumbnail(item, &app.store) {
                s.cover(&picture, thumb, 6.0);
            }
            s.border(thumb, 6.0, t.line, 1.0);
        }
        Card::Link => {
            let ty = top + (inner_height - LINE_TEXT - 16.0).max(0.0) / 2.0;
            let host = item.link_host().unwrap_or_default();
            s.text(&host, r(cx, ty, cw, LINE_TEXT), bold(14.0), t.fg, Align::Left);
            let line: String = item.preview_line(&facts).chars().take(400).collect();
            s.marked(&line, r(cx, ty + LINE_TEXT, cw, 16.0), text(12.0), t.muted, &marks(&line), t.mark, Wrap::Line);
        }
        _ => {
            let (font, line_height) = if mono_text { (mono(13.0), LINE_MONO) } else if card == Card::Color { (mono(14.0), LINE_TEXT) } else { (text(14.0), LINE_TEXT) };
            let shown: Vec<String> = facts.preview.split('\n').take(3).map(|l| l.chars().take(400).collect()).collect();
            let ty = top + (inner_height - shown.len() as f32 * line_height).max(0.0) / 2.0;
            for (n, line) in shown.iter().enumerate() {
                s.marked(line, r(cx, ty + n as f32 * line_height, cw, line_height), font, t.fg, &marks(line), t.mark, Wrap::Line);
            }
        }
    }
    let show = hovered || selected;
    let by = top + (inner_height - 28.0) / 2.0;
    let mut bx = body.right() - 6.0 - 28.0;
    if show || item.pinned {
        icon_button(p, s, t, r(bx, by, 28.0, 28.0), pin_glyph, pin_colour, Hit::Pin(index));
    }
    bx -= 30.0;
    if let Some(glyph) = action.filter(|_| show) {
        icon_button(p, s, t, r(bx, by, 28.0, 28.0), glyph, t.muted, Hit::Action(index));
    }
    clip_hits(p, view);
}

/// Buttons of a half-scrolled row can't be clicked outside the list.
fn clip_hits(p: &mut Popup, view: R) {
    for (area, hit) in p.hits.iter_mut().rev().take(3) {
        if matches!(hit, Hit::Pin(_) | Hit::Action(_)) {
            *area = area.intersect(view);
        }
    }
}

fn preview(p: &mut Popup, app: &mut App, s: &Surface, t: &Theme, area: R) {
    let Some(item) = p.selected_id().and_then(|id| app.store.get(&id).cloned()) else { return };
    let revision = app.store.revision(&item.id);
    let facts = app.cache.info(&item, revision).facts.clone();
    let files = app.cache.files(&item, revision);
    let keys = area.right() - 150.0;
    s.text(&item.meta(&facts, files.as_ref(), chrono::Local::now()), r(area.x, area.y, area.w - 156.0, 18.0), text(11.0), t.muted, Align::Left);
    let x = hint(s, t, keys, area.y, "\u{E74A} \u{E74B}", true, "Next");
    hint(s, t, x, area.y, "Space", false, "Close");
    let panel = r(area.x, area.y + 26.0, area.w, area.h - 26.0);
    s.panel(panel, 8.0, t.panel, t.line);
    if item.kind == Kind::Image {
        let max = (panel.w.max(panel.h) * p.scale) as u32;
        if let Some(picture) = app.cache.full(&item, &app.store, max.min(1600)) {
            let room = panel.inset(10.0);
            // never enlarged beyond its own pixels
            let k = (room.w / picture.w as f32).min(room.h / picture.h as f32).min(1.0 / p.scale);
            let (w, h) = (picture.w as f32 * k, picture.h as f32 * k);
            s.image(&picture, r(room.x + (room.w - w) / 2.0, room.y + (room.h - h) / 2.0, w, h), 1.0);
        }
    } else {
        let field = r(panel.x + 12.0, panel.y + 10.0, panel.w - 20.0, panel.h - 20.0);
        p.preview.draw(s, field, mono(12.0), t.fg, t.mark, false);
        p.hits.push((panel, Hit::Preview));
    }
}

fn editor(p: &mut Popup, s: &Surface, t: &Theme, area: R) {
    let mut x = area.x;
    x = hint(s, t, x, area.y, "Ctrl+S", false, "Save");
    x = hint(s, t, x, area.y, "Ctrl+Enter", false, "Save and paste");
    hint(s, t, x, area.y, "Esc", false, "Cancel");
    let panel = r(area.x, area.y + 26.0, area.w, area.h - 26.0 - 40.0);
    s.panel(panel, 8.0, t.panel, t.accent_soft);
    p.hits.push((panel, Hit::Editor));
    p.editor.draw(s, r(panel.x + 10.0, panel.y + 8.0, panel.w - 16.0, panel.h - 16.0), mono(12.0), t.fg, t.mark, p.caret_on);
    let y = area.bottom() - 30.0;
    let mut right = area.right();
    right -= button(p, s, t, right, y, "Save and paste", true, Hit::SavePaste) + 6.0;
    right -= button(p, s, t, right, y, "Save", false, Hit::Save) + 6.0;
    button(p, s, t, right, y, "Cancel", false, Hit::Cancel);
}

fn image_editor(p: &mut Popup, s: &Surface, t: &Theme, area: R) {
    let blur = p.image.as_ref().is_some_and(|e| e.blur);
    let mut x = area.x;
    for (glyph, label, hit, on) in [(ICON_CROP, "Crop", Hit::ToolCrop, !blur), (ICON_BLUR, "Blur", Hit::ToolBlur, blur)] {
        let w = gfx::measure(label, bold(12.0), 100.0, Wrap::Line).0.ceil() + 40.0;
        let chip = r(x, area.y, w, 26.0);
        if on {
            s.fill(chip, 13.0, t.accent_soft);
        } else if p.hover == Some(hit) {
            s.fill(chip, 13.0, t.hover);
        }
        let c = if on { t.accent } else { t.muted };
        s.glyph(glyph, r(chip.x + 8.0, chip.y, 18.0, 26.0), 12.0, c);
        s.text(label, r(chip.x + 28.0, chip.y, w - 30.0, 26.0), bold(12.0), c, Align::Left);
        p.hits.push((chip, hit));
        x += w + 2.0;
    }
    x += 6.0;
    icon_button(p, s, t, r(x, area.y - 1.0, 28.0, 28.0), ICON_ROTATE, t.muted, Hit::Rotate);
    icon_button(p, s, t, r(x + 30.0, area.y - 1.0, 28.0, 28.0), ICON_UNDO, t.muted, Hit::UndoImage);
    let panel = r(area.x, area.y + 34.0, area.w, area.h - 34.0 - 40.0);
    s.panel(panel, 8.0, t.panel, t.line);
    let scale = p.scale;
    if let Some(e) = &mut p.image {
        let info = format!("{} × {}{}", e.current.w, e.current.h, if e.edited() { " · edited" } else { "" });
        s.text(&info, r(area.right() - 160.0, area.y, 160.0, 26.0), text(11.0), t.muted, Align::Right);
        if e.prepared.as_ref().is_none_or(|(v, _)| *v != e.version) {
            let max = (panel.w.max(panel.h) * scale) as u32;
            let shown = e.current.scaled_to_fit(max.clamp(64, 2400)).unwrap_or_else(|_| e.current.clone());
            e.prepared = Some((e.version, crate::cache::drawable(shown)));
        }
        let room = panel.inset(8.0);
        let k = (room.w / e.current.w as f32).min(room.h / e.current.h as f32).min(1.0 / scale);
        let (w, h) = (e.current.w as f32 * k, e.current.h as f32 * k);
        e.shown = r(room.x + (room.w - w) / 2.0, room.y + (room.h - h) / 2.0, w, h);
        if let Some((_, picture)) = &e.prepared {
            s.image(picture, e.shown, 1.0);
        }
        if let Some((a, b)) = e.drag {
            let selection = r(a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs()).intersect(e.shown);
            s.fill(selection, 0.0, t.accent_soft);
            s.dashed(selection, t.accent);
        }
        p.hits.push((e.shown, Hit::Canvas));
    }
    let y = area.bottom() - 30.0;
    let hint_text = if blur { "Drag over anything private to blur it" } else { "Drag over the image to crop it" };
    let mut right = area.right();
    right -= button(p, s, t, right, y, "Save and paste", true, Hit::SavePaste) + 6.0;
    right -= button(p, s, t, right, y, "Save", false, Hit::Save) + 6.0;
    right -= button(p, s, t, right, y, "Cancel", false, Hit::Cancel) + 8.0;
    s.paragraph(hint_text, r(area.x, y, (right - area.x).max(0.0), 32.0), text(11.0), t.muted, Align::Left);
}
