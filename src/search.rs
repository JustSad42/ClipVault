//! Search that forgives typos: every word you type must appear in the item, either exactly or
//! as a close match ("invc" or "invioce" finds "invoice"). Plus filters like `app:chrome`.

use crate::model::{Card, Item, Kind};
use chrono::{DateTime, Datelike, Local};
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    App(String),
    Type(TypeFilter),
    /// Copied on or after (days ago, days ago), inclusive.
    Age(i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypeFilter {
    Card(Card),
    Image,
    Files,
    Text,
    Pinned,
    Qr,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Search words, lower case.
    pub words: Vec<Vec<char>>,
    pub filters: Vec<Filter>,
}

/// One character in lower case, keeping the character count (so match positions stay valid).
pub fn lower(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

fn lowered(s: &str) -> Vec<char> {
    s.chars().map(lower).collect()
}

fn type_filter(value: &str) -> Option<TypeFilter> {
    use TypeFilter as T;
    const TYPES: [(&str, T); 16] = [
        ("link", T::Card(Card::Link)),
        ("url", T::Card(Card::Link)),
        ("code", T::Card(Card::Code)),
        ("color", T::Card(Card::Color)),
        ("colour", T::Card(Card::Color)),
        ("email", T::Card(Card::Email)),
        ("mail", T::Card(Card::Email)),
        ("phone", T::Card(Card::Phone)),
        ("image", T::Image),
        ("img", T::Image),
        ("picture", T::Image),
        ("screenshot", T::Image),
        ("file", T::Files),
        ("text", T::Text),
        ("pinned", T::Pinned),
        ("qr", T::Qr),
    ];
    let v = value.trim_end_matches('s');
    if v.chars().count() < 2 {
        return None;
    }
    TYPES.iter().find(|(name, _)| name.starts_with(v) || v.starts_with(name)).map(|t| t.1)
}

fn date_filter(value: &str) -> Option<Filter> {
    Some(match value {
        "today" => Filter::Age(0, 0),
        "yesterday" => Filter::Age(1, 1),
        "week" => Filter::Age(0, 6),
        "month" => Filter::Age(0, 30),
        _ => return None,
    })
}

/// Splits a query into words and filters: `app:x`, `type:link|code|…`, `today`, `yesterday`,
/// `date:today|yesterday|week|month`. Anything else is a search word.
pub fn parse(text: &str) -> Query {
    let mut q = Query::default();
    for w in text.split_whitespace() {
        let lw: String = w.chars().map(lower).collect();
        if let Some((key, value)) = lw.split_once(':').filter(|(k, v)| !k.is_empty() && !v.is_empty()) {
            let filter = match key {
                "app" | "from" => Some(Filter::App(value.to_string())),
                "type" | "is" => type_filter(value).map(Filter::Type),
                "date" | "when" => date_filter(value),
                _ => None,
            };
            if let Some(f) = filter {
                q.filters.push(f);
                continue;
            }
        }
        if let Some(f) = (lw == "today" || lw == "yesterday").then(|| date_filter(&lw)).flatten() {
            q.filters.push(f);
            continue;
        }
        q.words.push(lowered(w));
    }
    q
}

/// What an item is searched by, worked out once and kept until the item changes.
#[derive(Clone, Debug, Default)]
pub struct Index {
    /// Text, app name, image text and QR text, lower case, separated so words can't span them.
    haystack: String,
    /// Distinct words, for typo matching.
    tokens: HashSet<Vec<char>>,
}

impl Index {
    pub fn new(item: &Item) -> Index {
        let mut haystack = String::new();
        let mut tokens = HashSet::new();
        let image = (item.kind == Kind::Image).then_some("image");
        for (n, part) in [item.text.as_deref(), item.source_name.as_deref(), item.ocr_text.as_deref(), item.qr_text.as_deref(), image].into_iter().enumerate() {
            let Some(part) = part else { continue };
            haystack.extend(part.chars().map(lower));
            haystack.push('\u{1}');
            // typo matching looks at the start of very long texts only
            let limit = if n == 0 { 20_000 } else { usize::MAX };
            for t in part.chars().take(limit).collect::<String>().split(|c: char| !c.is_alphanumeric()) {
                if !t.is_empty() && t.chars().count() <= 40 {
                    tokens.insert(lowered(t));
                }
            }
        }
        Index { haystack, tokens }
    }

    /// 0 = no match, 1 = matched with typos, 2 = every word found exactly.
    pub fn score(&self, words: &[Vec<char>]) -> u8 {
        let mut score = 2;
        for w in words {
            let exact: String = w.iter().collect();
            if self.haystack.contains(&exact) {
                continue;
            }
            if w.len() >= 3 && self.tokens.iter().any(|t| close(w, t)) {
                score = 1;
                continue;
            }
            return 0;
        }
        score
    }
}

pub fn passes(item: &Item, card: Card, filters: &[Filter], now: DateTime<Local>) -> bool {
    filters.iter().all(|f| match f {
        Filter::App(a) => item.source_name.as_deref().is_some_and(|s| s.to_lowercase().contains(a)),
        Filter::Type(t) => match t {
            TypeFilter::Card(c) => card == *c,
            TypeFilter::Image => item.kind == Kind::Image,
            TypeFilter::Files => item.kind == Kind::Files,
            TypeFilter::Text => item.kind == Kind::Text,
            TypeFilter::Pinned => item.pinned,
            TypeFilter::Qr => item.has_qr(),
        },
        Filter::Age(from, to) => {
            let days = now.date_naive().num_days_from_ce() - item.copied_at.date_naive().num_days_from_ce();
            // a clock change can put an item "in the future": count it as today
            (*from..=*to).contains(&days.max(0))
        }
    })
}

/// True when a word is a close match for a token: letters in order with a few skipped, or a typo.
pub fn close(w: &[char], t: &[char]) -> bool {
    if t.is_empty() || w.is_empty() || t[0] != w[0] {
        return false;
    }
    if t.len() >= w.len() && t.len() <= w.len() * 2 + 1 && subsequence(w, t) {
        return true;
    }
    let max = if w.len() >= 7 { 2 } else if w.len() >= 4 { 1 } else { 0 };
    max > 0 && t.len().abs_diff(w.len()) <= max && distance(w, t, max) <= max
}

fn subsequence(w: &[char], t: &[char]) -> bool {
    let mut j = 0;
    for c in t {
        if j < w.len() && *c == w[j] {
            j += 1;
        }
    }
    j == w.len()
}

/// Edit distance with swapped letters counting as one edit; stops early past the limit.
fn distance(a: &[char], b: &[char], limit: usize) -> usize {
    let width = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * width];
    for i in 0..=a.len() {
        d[i * width] = i;
    }
    for (j, cell) in d.iter_mut().enumerate().take(width) {
        *cell = j;
    }
    for i in 1..=a.len() {
        let mut row_min = usize::MAX;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[(i - 1) * width + j] + 1).min(d[i * width + j - 1] + 1).min(d[(i - 1) * width + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[(i - 2) * width + j - 2] + 1);
            }
            d[i * width + j] = v;
            row_min = row_min.min(v);
        }
        if row_min > limit {
            return limit + 1;
        }
    }
    d[a.len() * width + b.len()]
}

/// Parts of `text` to highlight, as (start, length) in characters: exact occurrences, or whole
/// words that closely match. Sorted, overlaps merged.
pub fn ranges(text: &str, words: &[Vec<char>]) -> Vec<(usize, usize)> {
    if words.is_empty() {
        return vec![];
    }
    let t = lowered(text);
    let mut found: Vec<(usize, usize)> = Vec::new();
    for w in words {
        let before = found.len();
        let mut i = 0;
        while !w.is_empty() && i + w.len() <= t.len() {
            if t[i..i + w.len()] == w[..] {
                found.push((i, w.len()));
                i += w.len();
            } else {
                i += 1;
            }
        }
        if found.len() > before || w.len() < 3 {
            continue;
        }
        let mut start = None;
        for i in 0..=t.len() {
            let letter = i < t.len() && t[i].is_alphanumeric();
            match (letter, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    if close(w, &t[s..i]) {
                        found.push((s, i - s));
                    }
                    start = None;
                }
                _ => {}
            }
        }
    }
    found.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, l) in found {
        match merged.last_mut() {
            Some(last) if s <= last.0 + last.1 => last.1 = (last.0 + last.1).max(s + l) - last.0,
            _ => merged.push((s, l)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn typos_are_forgiven() {
        let item = Item { text: Some("Invoice for the Keyboard order".into()), source_name: Some("Google Chrome".into()), ..Default::default() };
        let i = Index::new(&item);
        assert_eq!(i.score(&parse("keyboard INVOICE").words), 2);
        assert_eq!(i.score(&parse("invc").words), 1);
        assert_eq!(i.score(&parse("invioce").words), 1);
        assert_eq!(i.score(&parse("chrome").words), 2);
        assert_eq!(i.score(&parse("monitor").words), 0);
        assert!(!close(&w("cat"), &w("dog")));
    }

    #[test]
    fn filters() {
        let q = parse("app:chrome type:links today budget date:week type:x");
        assert_eq!(q.words, vec![w("budget"), w("type:x")]);
        assert_eq!(q.filters, vec![Filter::App("chrome".into()), Filter::Type(TypeFilter::Card(Card::Link)), Filter::Age(0, 0), Filter::Age(0, 6)]);
        assert_eq!(parse("type:img").filters, vec![Filter::Type(TypeFilter::Image)]);
        let now = Local::now();
        let item = Item { copied_at: now - chrono::Duration::days(1), source_name: Some("Chrome".into()), ..Default::default() };
        assert!(passes(&item, Card::Plain, &parse("yesterday app:chr").filters, now));
        assert!(!passes(&item, Card::Plain, &parse("today").filters, now));
    }

    #[test]
    fn highlights() {
        assert_eq!(ranges("The invoice, the INVOICE", &parse("invoice").words), vec![(4, 7), (17, 7)]);
        assert_eq!(ranges("an invoice", &parse("invioce").words), vec![(3, 7)]);
        assert_eq!(ranges("abcabc", &parse("abc bca").words), vec![(0, 6)]);
    }
}
