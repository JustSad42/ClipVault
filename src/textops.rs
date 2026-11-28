//! Text work: removing tracking from links, the "Paste as…" changes, and file names.

use crate::model::web_host;

const TRACKING: [&str; 42] = [
    "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "twclid", "ttclid", "igshid", "igsh", "mc_cid",
    "mc_eid", "mkt_tok", "_hsenc", "_hsmi", "hsctatracking", "li_fat_id", "vero_id", "vero_conv", "oly_anon_id", "oly_enc_id",
    "wickedid", "rb_clickid", "s_cid", "ref_src", "ref_url", "_ga", "_gl", "trk", "trkcampaign", "sc_campaign", "sc_channel",
    "sc_content", "sc_medium", "sc_outcome", "sc_geo", "sc_country", "cmpid", "srsltid", "si_source",
];
const PREFIXES: [&str; 6] = ["utm_", "pk_", "hsa_", "pd_rd_", "pf_rd_", "mtm_"];
/// "si" tracks shares on these sites, but is a real setting elsewhere.
const SI_HOSTS: [&str; 5] = ["youtube.com", "youtu.be", "spotify.com", "instagram.com", "threads.net"];

/// Percent-decoding, enough to recognise a parameter's name.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The link without tracking parameters, or `None` when there was nothing to remove.
pub fn clean_link(text: &str) -> Option<String> {
    let t = text.trim();
    if t.len() > 4000 {
        return None;
    }
    let host = web_host(t)?;
    let q = t.find('?')?;
    let hash = t[q..].find('#').map(|i| q + i);
    let query = &t[q + 1..hash.unwrap_or(t.len())];
    let fragment = hash.map_or("", |h| &t[h..]);
    let si_tracks = SI_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")));
    let mut kept = Vec::new();
    let mut removed = false;
    for part in query.split('&').filter(|p| !p.is_empty()) {
        let name = unescape(part.split('=').next().unwrap_or("")).to_ascii_lowercase();
        let track = TRACKING.contains(&name.as_str()) || PREFIXES.iter().any(|p| name.starts_with(p)) || (si_tracks && name == "si");
        if track { removed = true } else { kept.push(part) }
    }
    if !removed {
        return None;
    }
    let query = if kept.is_empty() { String::new() } else { format!("?{}", kept.join("&")) };
    Some(format!("{}{query}{fragment}", &t[..q]))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transform {
    Upper,
    Lower,
    Title,
    Trim,
    SingleLine,
}

pub fn apply(kind: Transform, text: &str) -> String {
    match kind {
        Transform::Upper => text.to_uppercase(),
        Transform::Lower => text.to_lowercase(),
        Transform::Title => {
            let mut start = true;
            text.chars()
                .map(|c| {
                    let out = if start { c.to_uppercase().collect::<String>() } else { c.to_lowercase().collect() };
                    start = !(c.is_alphanumeric() || c == '\'' || c == '’');
                    out
                })
                .collect()
        }
        // trims each line and collapses repeated spaces, keeping the line breaks
        Transform::Trim => text
            .replace("\r\n", "\n")
            .split('\n')
            .map(|l| {
                let mut out = String::new();
                let mut gap = String::new();
                for c in l.trim().chars() {
                    if c == ' ' || c == '\t' {
                        gap.push(c);
                        continue;
                    }
                    if !gap.is_empty() {
                        out.push_str(if gap.chars().count() > 1 { " " } else { &gap });
                        gap.clear();
                    }
                    out.push(c);
                }
                out
            })
            .collect::<Vec<_>>()
            .join("\r\n")
            .trim()
            .to_string(),
        Transform::SingleLine => text.trim().lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "),
    }
}

/// First few words of the text, made safe for a file name.
pub fn suggest_file_name(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let cleaned: String = line.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c }).collect();
    let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 40 {
        name = name.chars().take(40).collect::<String>().trim_end().to_string();
    }
    let name = name.trim_end_matches(['.', ' ']).to_string();
    if name.is_empty() { "Clipboard text".into() } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_is_removed() {
        assert_eq!(clean_link("https://x.com/a?utm_source=1&id=2&fbclid=3#top").as_deref(), Some("https://x.com/a?id=2#top"));
        assert_eq!(clean_link("https://youtu.be/abc?si=zz").as_deref(), Some("https://youtu.be/abc"));
        assert_eq!(clean_link("https://example.com/a?si=zz"), None);
        assert_eq!(clean_link("https://x.com/a?q=utm_"), None);
        assert_eq!(clean_link("not a link ?utm_source=1"), None);
        assert_eq!(clean_link("https://x.com/a?%75tm_medium=e").as_deref(), Some("https://x.com/a"));
    }

    #[test]
    fn transforms() {
        assert_eq!(apply(Transform::Title, "hELLO there, it's-me"), "Hello There, It's-Me");
        assert_eq!(apply(Transform::Trim, "  a   b \n\tc  d  "), "a b\r\nc d");
        assert_eq!(apply(Transform::SingleLine, " a \r\n\r\n b "), "a b");
        assert_eq!(suggest_file_name("  What? A <name>.\nsecond"), "What A name");
        assert_eq!(suggest_file_name("///"), "Clipboard text");
    }
}
