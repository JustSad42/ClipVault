//! One history entry, stored in history.json exactly as the C# version wrote it, and everything
//! the list shows about it.

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone};
use regex_lite::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    Text,
    Image,
    Files,
}

/// How an item is shown: special cards for colours, links, emails, phone numbers and code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Plain,
    Code,
    Link,
    Color,
    Email,
    Phone,
    Files,
    Image,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    /// Plain text. For files: one full path per line.
    pub text: Option<String>,
    /// Formatted versions kept alongside the text (web pages, Word…).
    pub html: Option<String>,
    pub rtf: Option<String>,
    pub image_file: Option<String>,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// Text recognised inside an image. `None` until it has been read.
    pub ocr_text: Option<String>,
    /// Text of a QR code in the image ("" = none). `None` until looked for.
    pub qr_text: Option<String>,
    pub link_cleaned: bool,
    pub source_name: Option<String>,
    pub source_path: Option<String>,
    pub hash: String,
    #[serde(serialize_with = "write_time", deserialize_with = "read_time")]
    pub copied_at: DateTime<Local>,
    pub pinned: bool,
}

fn write_time<S: Serializer>(t: &DateTime<Local>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&t.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string())
}

/// Accepts what .NET writes: with an offset, or local time without one.
pub fn parse_time(s: &str) -> Option<DateTime<Local>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Local));
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    Local.from_local_datetime(&naive).earliest()
}

fn read_time<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Local>, D::Error> {
    let s = String::deserialize(d)?;
    parse_time(&s).ok_or_else(|| serde::de::Error::custom("not a date"))
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^\s@<>]+@[^\s@<>]+\.[A-Za-z]{2,}$").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\+?[\d\s().-]{7,22}$").unwrap());
static CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)[;{}]\s*$|=>|^\s*(def|class|function|const|let|var|import|from|using|public|private|return|if|for|while|#include|<\w+|SELECT|FROM)\b|^\s*[\]}),]+;?\s*$").unwrap()
});
static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#([0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$").unwrap());
static RGB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*(?:,\s*([\d.]+%?)\s*)?\)$").unwrap());
static TECHNICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*([A-Za-z]:\\|\\\\|~?/[\w.-]+/|\$ |PS [A-Za-z]:\\|>\s|(git|cd|ls|dir|npm|npx|pnpm|yarn|dotnet|curl|wget|pip|python|py|node|docker|kubectl|ssh|scp|winget|choco|cargo|rustup|sudo|apt|nslookup|ping|ipconfig|tracert|traceroute|netstat|nmap|brew|pwsh|powershell|cmd|gh|Get-|Set-|New-|Remove-|Invoke-|Start-|Stop-)(\b|\s))").unwrap()
});
/// A path, but not a Command Prompt line like "C:\Users\me>dir".
static PATH_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*([A-Za-z]:\\|\\\\|~?/[\w.-]+/)").unwrap());
const TERMINALS: [&str; 8] = ["terminal", "powershell", "command prompt", "conhost", "wezterm", "alacritty", "putty", "mobaxterm"];
const PHOTOS: [&str; 9] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico"];

/// "#RGB", "#RRGGBB", "#RRGGBBAA" or "rgb()/rgba()" as (r, g, b, a).
pub fn parse_color(t: &str) -> Option<(u8, u8, u8, u8)> {
    if let Some(m) = HEX.captures(t) {
        let mut h = m.get(1)?.as_str().to_string();
        if h.len() <= 4 {
            h = h.chars().flat_map(|c| [c, c]).collect();
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        return Some((p(0)?, p(2)?, p(4)?, if h.len() == 8 { p(6)? } else { 255 }));
    }
    let m = RGB.captures(t)?;
    let n = |i: usize| m.get(i).and_then(|v| v.as_str().parse::<u32>().ok()).filter(|v| *v <= 255).map(|v| v as u8);
    let mut a = 255u8;
    if let Some(v) = m.get(4) {
        let v = v.as_str();
        if let Some(pct) = v.strip_suffix('%').and_then(|p| p.parse::<f64>().ok()) {
            a = (pct * 2.55).clamp(0.0, 255.0) as u8;
        } else if let Ok(f) = v.parse::<f64>() {
            a = (f * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
    Some((n(1)?, n(2)?, n(3)?, a))
}

pub fn color_hex((r, g, b, a): (u8, u8, u8, u8)) -> String {
    if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
}

pub fn color_rgb((r, g, b, a): (u8, u8, u8, u8)) -> String {
    if a == 255 {
        return format!("rgb({r}, {g}, {b})");
    }
    let alpha = (a as f64 / 255.0 * 100.0).round() / 100.0;
    format!("rgba({r}, {g}, {b}, {alpha})")
}

pub fn color_hsl((r, g, b, _): (u8, u8, u8, u8)) -> String {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let (mut h, mut s) = (0.0, 0.0);
    if max != min {
        let d = max - min;
        s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
        h = if max == r { (g - b) / d + if g < b { 6.0 } else { 0.0 } } else if max == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
        h *= 60.0;
    }
    format!("hsl({}, {}%, {}%)", h.round(), (s * 100.0).round(), (l * 100.0).round())
}

/// The host of an http(s) address (lower case, no user or port), if the whole text is one.
pub fn web_host(t: &str) -> Option<String> {
    if t.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = t.get(..8.min(t.len()))?.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") { &t[8..] } else if lower.starts_with("http://") { &t[7..] } else { return None };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = match host.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    (!host.is_empty()).then(|| host.to_lowercase())
}

pub fn format_size(bytes: u64) -> String {
    let trim = |v: f64, places: usize| {
        let s = format!("{v:.places$}");
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", trim(bytes as f64 / 1024.0, 1)),
        1_048_576..1_073_741_824 => format!("{} MB", trim(bytes as f64 / 1_048_576.0, 1)),
        _ => format!("{} GB", trim(bytes as f64 / 1_073_741_824.0, 2)),
    }
}

/// 1234567 as "1,234,567".
pub fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn ago(t: DateTime<Local>, now: DateTime<Local>) -> String {
    let s = (now - t).num_seconds();
    match s {
        i64::MIN..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86_400 => format!("{}h ago", s / 3600),
        86_400..604_800 => format!("{}d ago", s / 86_400),
        _ => t.format("%b %-d, %Y").to_string(),
    }
}

/// What copied files look like on disk right now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileStats {
    pub text: String,
    /// The first path is a folder.
    pub first_is_dir: bool,
    pub first_exists: bool,
}

/// Facts about an item's text that are costly to work out, kept until the text changes.
#[derive(Clone, Debug)]
pub struct Facts {
    pub card: Card,
    /// Up to three lines for the list.
    pub preview: String,
    pub line_count: usize,
    pub technical: bool,
}

impl Item {
    pub fn file_paths(&self) -> Vec<&str> {
        self.text.as_deref().unwrap_or("").split(['\r', '\n']).filter(|p| !p.is_empty()).collect()
    }

    pub fn has_formatting(&self) -> bool {
        self.html.is_some() || self.rtf.is_some()
    }

    pub fn has_qr(&self) -> bool {
        self.qr_text.as_deref().is_some_and(|q| !q.is_empty())
    }

    pub fn qr_host(&self) -> Option<String> {
        web_host(self.qr_text.as_deref()?)
    }

    pub fn has_ocr_text(&self) -> bool {
        self.ocr_text.as_deref().is_some_and(|t| !t.trim().is_empty())
    }

    /// Site name for link cards, e.g. "github.com".
    pub fn link_host(&self) -> Option<String> {
        let h = web_host(self.text.as_deref()?.trim())?;
        Some(h.strip_prefix("www.").map(String::from).unwrap_or(h))
    }

    pub fn color(&self) -> Option<(u8, u8, u8, u8)> {
        parse_color(self.text.as_deref()?.trim())
    }

    pub fn first_file_is_photo(&self) -> bool {
        self.kind == Kind::Files
            && self.file_paths().first().is_some_and(|p| {
                std::path::Path::new(p).extension().and_then(|e| e.to_str()).is_some_and(|e| PHOTOS.contains(&e.to_ascii_lowercase().as_str()))
            })
    }

    fn detect_card(&self, line_count: usize) -> Card {
        match self.kind {
            Kind::Image => return Card::Image,
            Kind::Files => return Card::Files,
            Kind::Text => {}
        }
        let t = self.text.as_deref().unwrap_or("").trim();
        if t.is_empty() {
            return Card::Plain;
        }
        let chars = |max: usize| t.len() <= max || t.chars().take(max + 1).count() <= max;
        if chars(40) && parse_color(t).is_some() {
            return Card::Color;
        }
        if chars(2000) && !t.contains([' ', '\n']) && web_host(t).is_some_and(|h| h.contains('.')) {
            return Card::Link;
        }
        if chars(254) && EMAIL.is_match(t) {
            return Card::Email;
        }
        if chars(22) && PHONE.is_match(t) {
            let digits = t.chars().filter(char::is_ascii_digit).count();
            if (7..=15).contains(&digits) && (t.starts_with('+') || t.find([' ', '-', '(', '.']).is_some_and(|i| i > 0)) {
                return Card::Phone;
            }
        }
        if line_count >= 2 {
            let sample: String = t.chars().take(4000).collect();
            let lines = sample.matches('\n').count() + 1;
            if CODE.find_iter(&sample).count() * 3 >= lines {
                return Card::Code;
            }
        }
        Card::Plain
    }

    pub fn facts(&self) -> Facts {
        let line_count = self.text.as_deref().unwrap_or("").trim().matches('\n').count() + 1;
        let preview = if self.kind == Kind::Files { files_preview(&self.file_paths()) } else { text_preview(self.text.as_deref().unwrap_or("")) };
        let card = self.detect_card(line_count);
        let head: String = preview.chars().take(300).collect();
        let technical = TECHNICAL.is_match(&head);
        Facts { card, preview, line_count, technical }
    }

    pub fn type_label(&self, f: &Facts, files: Option<&FileStats>) -> &'static str {
        match f.card {
            Card::Files if self.file_paths().len() == 1 => if files.is_some_and(|s| s.first_is_dir) { "Folder" } else { "File" },
            Card::Files => "Files",
            Card::Plain if !f.technical => "Text",
            Card::Plain => {
                let path = PATH_START.is_match(&f.preview) && !f.preview.lines().next().unwrap_or("").contains('>');
                if path { "Path" } else { "Command" }
            }
            Card::Code => "Code",
            Card::Link => "Link",
            Card::Color => "Color",
            Card::Email => "Email",
            Card::Phone => "Phone",
            Card::Image => "Image",
        }
    }

    /// Code font for code, colours, paths, commands and anything copied from a terminal.
    pub fn use_mono(&self, f: &Facts) -> bool {
        matches!(f.card, Card::Code | Card::Color)
            || (f.card == Card::Plain
                && (f.technical || self.source_name.as_deref().is_some_and(|s| TERMINALS.iter().any(|t| s.to_lowercase().contains(t)))))
    }

    /// Small tags after the type: "67 lines", "1920×1080", "QR", "2.3 MB", "formatted"…
    pub fn tags(&self, f: &Facts, files: Option<&FileStats>) -> Vec<String> {
        let mut t = Vec::new();
        match self.kind {
            Kind::Image => {
                t.push(format!("{}×{}", self.pixel_width, self.pixel_height));
                if self.has_qr() {
                    t.push("QR".into());
                }
            }
            Kind::Files => {
                let n = self.file_paths().len();
                if n > 1 {
                    t.push(format!("{n} items"));
                }
                if let Some(s) = files.filter(|s| !s.text.is_empty()) {
                    t.push(s.text.split(" · ").next().unwrap_or("").to_string());
                }
            }
            Kind::Text => {
                let len = self.text.as_deref().map_or(0, |s| s.chars().count());
                if f.line_count > 1 {
                    t.push(format!("{} lines", grouped(f.line_count)));
                } else if len > 80 {
                    t.push(format!("{} chars", grouped(len)));
                }
            }
        }
        if self.has_formatting() {
            t.push("formatted".into());
        }
        if self.link_cleaned {
            t.push("cleaned".into());
        }
        t
    }

    /// The line above the full preview.
    pub fn meta(&self, f: &Facts, files: Option<&FileStats>, now: DateTime<Local>) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(s) = self.source_name.as_deref().filter(|s| !s.is_empty()) {
            parts.push(s.into());
        }
        match self.kind {
            Kind::Image => {
                parts.push(format!("Image {}×{}", self.pixel_width, self.pixel_height));
                if self.has_qr() {
                    let q = self.qr_text.as_deref().unwrap_or("");
                    let short = self.qr_host().unwrap_or_else(|| if q.chars().count() > 28 { q.chars().take(28).collect::<String>() + "…" } else { q.into() });
                    parts.push(format!("QR: {short}"));
                }
            }
            Kind::Files => {
                let n = self.file_paths().len();
                parts.push(if n == 1 { "1 file".into() } else { format!("{n} files") });
                if let Some(s) = files.filter(|s| !s.text.is_empty()) {
                    parts.push(s.text.clone());
                }
            }
            Kind::Text => parts.push(if f.line_count > 1 {
                format!("{} lines", grouped(f.line_count))
            } else {
                format!("{} chars", grouped(self.text.as_deref().map_or(0, |s| s.chars().count())))
            }),
        }
        if self.has_formatting() {
            parts.push("formatted".into());
        }
        if self.link_cleaned {
            parts.push("tracking removed".into());
        }
        parts.push(ago(self.copied_at, now));
        parts.join(" · ")
    }

    /// One line for compact rows.
    pub fn preview_line(&self, f: &Facts) -> String {
        match self.kind {
            Kind::Image if self.has_qr() => format!(
                "Image {}×{} · QR: {}",
                self.pixel_width,
                self.pixel_height,
                self.qr_host().unwrap_or_else(|| self.qr_text.clone().unwrap_or_default())
            ),
            Kind::Image => format!("Image {}×{}", self.pixel_width, self.pixel_height),
            Kind::Files => f.preview.replace('\n', ", "),
            Kind::Text => f.preview.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string(),
        }
    }

    /// Heading this item is listed under.
    pub fn group(&self, now: DateTime<Local>) -> &'static str {
        if self.pinned {
            return "Pinned";
        }
        let days = now.date_naive().num_days_from_ce() - self.copied_at.date_naive().num_days_from_ce();
        match days {
            i32::MIN..=0 => "Today",
            1 => "Yesterday",
            2..7 => "Earlier this week",
            _ => "Older",
        }
    }

    /// Reads the copied files' size and date from disk.
    pub fn file_stats(&self, now: DateTime<Local>) -> FileStats {
        let paths = self.file_paths();
        let (mut total, mut found, mut folders, mut modified) = (0u64, 0usize, 0usize, None);
        let mut out = FileStats::default();
        for (i, p) in paths.iter().take(200).enumerate() {
            // network paths can stall for seconds: don't touch them here
            if p.starts_with(r"\\") {
                continue;
            }
            let Ok(m) = std::fs::metadata(p) else { continue };
            found += 1;
            if i == 0 {
                out.first_exists = true;
                out.first_is_dir = m.is_dir();
            }
            if m.is_dir() {
                folders += 1;
            } else {
                total += m.len();
                modified = m.modified().ok().map(DateTime::<Local>::from);
            }
        }
        if paths.iter().any(|p| p.starts_with(r"\\")) && found == 0 {
            return out;
        }
        if found == 0 {
            out.text = if paths.len() == 1 { "missing".into() } else { "all missing".into() };
            return out;
        }
        let mut parts = vec![if folders == found { if folders == 1 { "folder".to_string() } else { "folders".to_string() } } else { format_size(total) }];
        if let (1, Some(m)) = (paths.len(), modified) {
            let date = if m.year() == now.year() { m.format("%b %-d") } else { m.format("%b %-d, %Y") };
            parts.push(format!("modified {date}"));
        }
        if found < paths.len() && paths.len() > 1 {
            parts.push(format!("{} missing", paths.len() - found));
        }
        out.text = parts.join(" · ");
        out
    }
}

fn files_preview(paths: &[&str]) -> String {
    let mut names: Vec<String> = paths
        .iter()
        .take(3)
        .map(|p| p.trim_end_matches('\\').rsplit(['\\', '/']).next().filter(|n| !n.is_empty()).unwrap_or(p).to_string())
        .collect();
    if paths.len() > 3 {
        names.push(format!("+{} more", paths.len() - 3));
    }
    names.join("\n")
}

fn text_preview(text: &str) -> String {
    let src: String = text.chars().take(4000).collect();
    let src = src.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let mut lines: Vec<&str> = src.split('\n').map(str::trim_end).collect();
    while lines.first() == Some(&"") {
        lines.remove(0);
    }
    while lines.last() == Some(&"") {
        lines.pop();
    }
    // collapse runs of blank lines so the three preview lines show content
    let mut shown: Vec<&str> = Vec::new();
    for l in lines {
        if shown.len() == 3 {
            break;
        }
        if !l.is_empty() || shown.last().is_some_and(|p| !p.is_empty()) {
            shown.push(l);
        }
    }
    // remove indentation shared by all shown lines (code copied from inside a function)
    let indent = shown.iter().filter(|l| !l.is_empty()).map(|l| l.len() - l.trim_start_matches(' ').len()).min().unwrap_or(0);
    shown.iter().map(|l| l.get(indent..).unwrap_or(l)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Item {
        Item { text: Some(s.into()), ..Default::default() }
    }

    #[test]
    fn cards() {
        let card = |s: &str| text(s).facts().card;
        assert_eq!(card("#1e90ff"), Card::Color);
        assert_eq!(card("rgb(1, 2, 3)"), Card::Color);
        assert_eq!(card("https://github.com/x?y=1"), Card::Link);
        assert_eq!(card("http://localhost:80"), Card::Plain, "no dot in the host");
        assert_eq!(card("someone@example.org"), Card::Email);
        assert_eq!(card("+20 100 123 4567"), Card::Phone);
        assert_eq!(card("12345678"), Card::Plain);
        assert_eq!(card("fn main() {\n    let x = 1;\n}"), Card::Code);
        assert_eq!(card("two ordinary\nlines of prose"), Card::Plain);
    }

    #[test]
    fn colours() {
        assert_eq!(parse_color("#abc"), Some((170, 187, 204, 255)));
        assert_eq!(parse_color("rgba(0, 0, 0, 50%)"), Some((0, 0, 0, 127)));
        assert_eq!(parse_color("rgb(256, 0, 0)"), None);
        assert_eq!(color_hsl((255, 0, 0, 255)), "hsl(0, 100%, 50%)");
        assert_eq!(color_rgb((1, 2, 3, 128)), "rgba(1, 2, 3, 0.5)");
    }

    #[test]
    fn previews_and_labels() {
        let i = text("\n\n    one\n\n\n    two\n    three\n    four");
        assert_eq!(i.facts().preview, "one\n\ntwo");
        let cmd = text(r"C:\Users\me>dir");
        let f = cmd.facts();
        assert_eq!(cmd.type_label(&f, None), "Command");
        let path = text(r"C:\Users\me\file.txt");
        assert_eq!(path.type_label(&path.facts(), None), "Path");
        assert_eq!(grouped(1234567), "1,234,567");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(web_host("https://user@Example.com:8080/a"), Some("example.com".into()));
    }

    #[test]
    fn reads_what_dotnet_wrote() {
        let json = r#"[{"Id":"a","Kind":"Files","Text":"C:\\a.txt","Html":null,"Rtf":null,"ImageFile":null,"PixelWidth":0,"PixelHeight":0,
            "OcrText":null,"QrText":null,"LinkCleaned":false,"SourceName":"Explorer","SourcePath":null,"Hash":"f:AB",
            "CopiedAt":"2026-11-27T21:04:05.1234567+02:00","Pinned":true},
            {"Id":"b","Kind":"Text","Text":"x","CopiedAt":"2026-11-27T21:04:05","Unknown":1}]"#;
        let items: Vec<Item> = serde_json::from_str(json).unwrap();
        assert_eq!(items[0].kind, Kind::Files);
        assert!(items[0].pinned);
        let again = serde_json::to_string(&items).unwrap();
        assert!(again.contains(r#""CopiedAt":"2026-11-"#) && again.contains(r#""PixelWidth":0"#));
        let back: Vec<Item> = serde_json::from_str(&again).unwrap();
        assert_eq!(back[0].copied_at.timestamp_micros(), items[0].copied_at.timestamp_micros());
    }
}
