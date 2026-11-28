//! The clipboard history: kept in memory, saved to history.json, images beside it.

use crate::images::Bitmap;
use crate::model::{Item, Kind};
use crate::win;
use chrono::Local;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Longest side of the small copy saved next to each image, so lists never decode full screenshots.
pub const THUMB: u32 = 480;

pub struct Store {
    dir: PathBuf,
    /// Newest first is not guaranteed: sort by `copied_at` for display.
    pub items: Vec<Item>,
    pub max_unpinned: usize,
    /// Deleted items keep their image files for a while, so Undo can bring them back.
    deleted: Vec<(Item, Instant)>,
    /// When there are unsaved changes: since when.
    pub dirty: Option<Instant>,
    /// Bumped whenever an item is added, removed or changed.
    pub version: u64,
    /// Per item, bumped when its content changes (cached facts and pictures are then stale).
    changed: HashMap<String, u64>,
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02X}")).collect()
}

pub fn text_hash(text: &str) -> String {
    format!("t:{}", sha(text.as_bytes()))
}

pub fn image_hash(b: &Bitmap) -> String {
    format!("i:{}x{}:{}", b.w, b.h, sha(&b.px))
}

/// A random 32-digit id, like the C# version's.
pub fn new_id() -> String {
    unsafe { windows::Win32::System::Com::CoCreateGuid() }
        .map(|g| format!("{:032x}", g.to_u128()))
        .unwrap_or_else(|_| format!("{:032x}", std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos())))
}

/// Who copied something.
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub name: Option<String>,
    pub path: Option<String>,
}

impl Store {
    pub fn load(dir: PathBuf, max_unpinned: usize) -> Store {
        let mut s = Store { dir, items: vec![], max_unpinned, deleted: vec![], dirty: None, version: 0, changed: HashMap::new() };
        let _ = std::fs::create_dir_all(s.images());
        let path = s.dir.join("history.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return s };
        match serde_json::from_str::<Vec<Item>>(text.trim_start_matches('\u{feff}')) {
            Ok(items) => {
                let images = s.images();
                s.items = items.into_iter().filter(|i| i.kind != Kind::Image || i.image_file.as_ref().is_some_and(|f| images.join(f).exists())).collect();
            }
            Err(e) => {
                // keep a copy of the unreadable file instead of silently overwriting it
                win::log(&format!("history.json couldn't be read: {e}"));
                let _ = std::fs::copy(&path, s.dir.join("history.json.bad"));
            }
        }
        s
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn images(&self) -> PathBuf {
        self.dir.join("images")
    }

    pub fn image_path(&self, item: &Item) -> Option<PathBuf> {
        item.image_file.as_ref().map(|f| self.images().join(f))
    }

    pub fn thumb_path(&self, file: &str) -> PathBuf {
        self.images().join("thumbs").join(std::path::Path::new(file).with_extension("png"))
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn at(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    /// Changes whenever the item's content does.
    pub fn revision(&self, id: &str) -> u64 {
        self.changed.get(id).copied().unwrap_or(0)
    }

    fn touch(&mut self) {
        self.version += 1;
        self.dirty.get_or_insert_with(Instant::now);
    }

    fn touch_item(&mut self, id: &str) {
        self.touch();
        self.changed.insert(id.to_string(), self.version);
    }

    pub fn save(&mut self) {
        let _ = std::fs::create_dir_all(&self.dir);
        match serde_json::to_vec(&self.items) {
            Ok(bytes) => match win::write_atomic(&self.dir.join("history.json"), &bytes) {
                Ok(()) => self.dirty = None,
                // disk full or locked: try again on the next change
                Err(e) => win::log(&format!("history couldn't be saved: {e}")),
            },
            Err(e) => win::log(&format!("history couldn't be saved: {e}")),
        }
    }

    fn add(&mut self, mut item: Item) {
        item.copied_at = Local::now();
        let id = item.id.clone();
        self.items.insert(0, item);
        self.touch_item(&id);
        self.trim();
    }

    /// If this content is already there, moves it to the top instead of adding a copy.
    fn bump(&mut self, hash: &str) -> bool {
        let Some(i) = self.items.iter().position(|i| i.hash == hash) else { return false };
        self.items[i].copied_at = Local::now();
        self.touch();
        true
    }

    pub fn add_text(&mut self, text: String, html: Option<String>, rtf: Option<String>, source: Source, link_cleaned: bool) {
        let hash = text_hash(&text);
        if let Some(i) = self.items.iter().position(|i| i.hash == hash) {
            // the same text again: move it up and keep the newest formatting
            let item = &mut self.items[i];
            if html.is_some() || rtf.is_some() {
                item.html = html;
                item.rtf = rtf;
            }
            item.link_cleaned |= link_cleaned;
            item.copied_at = Local::now();
            let id = item.id.clone();
            self.touch_item(&id);
            return;
        }
        self.add(Item { id: new_id(), kind: Kind::Text, text: Some(text), html, rtf, hash, source_name: source.name, source_path: source.path, link_cleaned, ..Default::default() });
    }

    pub fn add_files(&mut self, paths: &[String], source: Source) {
        let text = paths.join("\r\n");
        let hash = format!("f:{}", sha(text.as_bytes()));
        if !self.bump(&hash) {
            self.add(Item { id: new_id(), kind: Kind::Files, text: Some(text), hash, source_name: source.name, source_path: source.path, ..Default::default() });
        }
    }

    /// True if a picture with this hash is already stored (and has now moved to the top).
    pub fn bump_image(&mut self, hash: &str) -> bool {
        self.bump(hash)
    }

    /// Adds an image whose files `write_image` has already saved.
    pub fn add_image(&mut self, saved: SavedImage, source: Source) {
        if self.bump(&saved.hash) {
            self.delete_files(&saved.file);
            return;
        }
        self.add(Item {
            id: saved.id,
            kind: Kind::Image,
            image_file: Some(saved.file),
            pixel_width: saved.width,
            pixel_height: saved.height,
            hash: saved.hash,
            source_name: source.name,
            source_path: source.path,
            ..Default::default()
        });
    }

    /// Replaces an item's text after editing. Formatting is dropped: it no longer matches.
    pub fn update_text(&mut self, id: &str, text: String) {
        let Some(i) = self.at(id) else { return };
        let item = &mut self.items[i];
        item.html = None;
        item.rtf = None;
        item.hash = text_hash(&text);
        item.text = Some(text);
        self.touch_item(id);
    }

    /// Replaces an item's picture after editing.
    pub fn update_image(&mut self, id: &str, bitmap: &Bitmap) -> windows_core::Result<()> {
        let Some(i) = self.at(id) else { return Ok(()) };
        let saved = write_image(&self.images(), &format!("{id}-{}", Local::now().timestamp_micros()), bitmap)?;
        let item = &mut self.items[i];
        let old = item.image_file.replace(saved.file);
        item.pixel_width = saved.width;
        item.pixel_height = saved.height;
        item.hash = saved.hash;
        // read the edited picture again
        item.ocr_text = None;
        item.qr_text = None;
        self.touch_item(id);
        if let Some(old) = old {
            self.delete_files(&old);
        }
        Ok(())
    }

    /// What reading an image found (text in it, QR code).
    pub fn set_read(&mut self, id: &str, file: &str, ocr: Option<String>, qr: Option<String>) {
        let Some(i) = self.at(id) else { return };
        let item = &mut self.items[i];
        // the picture may have been edited while it was being read
        if item.image_file.as_deref() != Some(file) {
            return;
        }
        if let Some(t) = ocr {
            item.ocr_text = Some(t);
        }
        if let Some(q) = qr {
            item.qr_text = Some(q);
        }
        self.touch_item(id);
    }

    /// The next image that hasn't been read yet: (id, file, needs text, needs QR).
    pub fn unread_image(&self) -> Option<(String, String, bool, bool)> {
        self.items.iter().filter(|i| i.kind == Kind::Image && (i.ocr_text.is_none() || i.qr_text.is_none())).find_map(|i| {
            Some((i.id.clone(), i.image_file.clone()?, i.ocr_text.is_none(), i.qr_text.is_none()))
        })
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) {
        let Some(i) = self.at(id) else { return };
        self.items[i].pinned = pinned;
        self.touch_item(id);
    }

    pub fn mark_copied(&mut self, id: &str) {
        let Some(i) = self.at(id) else { return };
        self.items[i].copied_at = Local::now();
        self.touch();
    }

    pub fn remove(&mut self, id: &str) -> Vec<String> {
        let Some(i) = self.at(id) else { return vec![] };
        let item = self.items.remove(i);
        self.deleted.push((item, Instant::now()));
        self.touch();
        vec![id.to_string()]
    }

    /// Removes everything that isn't pinned; returns what went, for Undo.
    pub fn clear_unpinned(&mut self) -> Vec<String> {
        let (keep, gone): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.items).into_iter().partition(|i| i.pinned);
        self.items = keep;
        let ids = gone.iter().map(|i| i.id.clone()).collect();
        let now = Instant::now();
        self.deleted.extend(gone.into_iter().map(|i| (i, now)));
        self.touch();
        ids
    }

    /// Puts removed items back (Undo); returns how many came back.
    pub fn restore(&mut self, ids: &[String]) -> usize {
        let mut n = 0;
        for id in ids {
            let Some(at) = self.deleted.iter().position(|(i, _)| &i.id == id) else { continue };
            let (item, _) = self.deleted.remove(at);
            if item.kind == Kind::Image && !self.image_path(&item).is_some_and(|p| p.exists()) {
                continue;
            }
            self.items.push(item);
            n += 1;
        }
        self.touch();
        n
    }

    /// Deletes the image files of items removed more than `older` ago.
    pub fn purge_deleted(&mut self, older: Duration) {
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.deleted).into_iter().partition(|(_, when)| when.elapsed() >= older);
        self.deleted = keep;
        for (item, _) in due {
            self.changed.remove(&item.id);
            if let Some(f) = item.image_file {
                self.delete_files(&f);
            }
        }
    }

    /// Drops the oldest unpinned items beyond the limit. Pinned items are never dropped.
    pub fn trim(&mut self) {
        let mut unpinned: Vec<(chrono::DateTime<Local>, String)> = self.items.iter().filter(|i| !i.pinned).map(|i| (i.copied_at, i.id.clone())).collect();
        if unpinned.len() <= self.max_unpinned {
            return;
        }
        unpinned.sort_by(|a, b| b.0.cmp(&a.0));
        let extra: std::collections::HashSet<String> = unpinned.split_off(self.max_unpinned).into_iter().map(|(_, id)| id).collect();
        let (gone, keep): (Vec<Item>, Vec<Item>) = std::mem::take(&mut self.items).into_iter().partition(|i| extra.contains(&i.id));
        self.items = keep;
        for item in gone {
            self.changed.remove(&item.id);
            if let Some(f) = item.image_file {
                self.delete_files(&f);
            }
        }
        self.touch();
    }

    fn delete_files(&self, file: &str) {
        let _ = std::fs::remove_file(self.images().join(file));
        let _ = std::fs::remove_file(self.thumb_path(file));
    }
}

pub struct SavedImage {
    pub id: String,
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub hash: String,
}

/// Saves a picture and its thumbnail as `<name>.png`. Safe to run on a background thread.
pub fn write_image(images: &std::path::Path, name: &str, bitmap: &Bitmap) -> windows_core::Result<SavedImage> {
    let io = |e: std::io::Error| windows_core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string());
    std::fs::create_dir_all(images).map_err(io)?;
    let file = format!("{name}.png");
    let png = bitmap.png()?;
    win::write_atomic(&images.join(&file), &png).map_err(io)?;
    if bitmap.w.max(bitmap.h) > THUMB {
        // thumbnails are optional: the full picture is used when one is missing
        if let Ok(small) = bitmap.scaled_to_fit(THUMB).and_then(|b| b.png()) {
            let dir = images.join("thumbs");
            let _ = std::fs::create_dir_all(&dir);
            let _ = win::write_atomic(&dir.join(&file), &small);
        }
    }
    Ok(SavedImage { id: name.split('-').next().unwrap_or(name).to_string(), file, width: bitmap.w, height: bitmap.h, hash: image_hash(bitmap) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("clipvault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn add_bump_trim_undo_and_reload() {
        let dir = temp("store");
        let mut s = Store::load(dir.clone(), 2);
        for t in ["one", "two", "one"] {
            s.add_text(t.into(), None, None, Source::default(), false);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(s.items.len(), 2, "the same text moves up instead of being added twice");
        let two = s.items.iter().find(|i| i.text.as_deref() == Some("two")).unwrap().id.clone();
        s.set_pinned(&two, true);
        for t in ["three", "four"] {
            s.add_text(t.into(), None, None, Source::default(), false);
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut texts: Vec<_> = s.items.iter().filter_map(|i| i.text.clone()).collect();
        texts.sort();
        assert_eq!(texts, ["four", "three", "two"], "the oldest unpinned went; the pinned one stayed");
        let removed = s.clear_unpinned();
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.restore(&removed), 2);
        s.save();
        assert!(s.dirty.is_none());
        let again = Store::load(dir.clone(), 10);
        assert_eq!(again.items.len(), 3);
        std::fs::write(dir.join("history.json"), "not json").unwrap();
        assert!(Store::load(dir.clone(), 10).items.is_empty());
        assert_eq!(std::fs::read_to_string(dir.join("history.json.bad")).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn images() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let dir = temp("images");
        let mut s = Store::load(dir.clone(), 10);
        let b = Bitmap { w: 600, h: 10, px: vec![200; 600 * 10 * 4] };
        let id = new_id();
        let saved = write_image(&s.images(), &id, &b).unwrap();
        assert!(s.thumb_path(&saved.file).exists());
        s.add_image(saved, Source::default());
        // the same picture again only moves up, and its extra files are removed
        let again = write_image(&s.images(), &new_id(), &b).unwrap();
        let extra = s.images().join(&again.file);
        s.add_image(again, Source::default());
        assert_eq!(s.items.len(), 1);
        assert!(!extra.exists());
        let (rid, file, ocr, qr) = s.unread_image().unwrap();
        assert!(ocr && qr && rid == id);
        s.set_read(&rid, &file, Some(String::new()), Some(String::new()));
        assert!(s.unread_image().is_none());
        s.update_image(&id, &b.rotated()).unwrap();
        assert_eq!(s.items[0].pixel_width, 10);
        assert!(!s.images().join(&file).exists(), "the old file went");
        let gone = s.remove(&id);
        s.purge_deleted(Duration::ZERO);
        assert_eq!(s.restore(&gone), 0);
        assert_eq!(std::fs::read_dir(s.images()).unwrap().filter(|e| e.as_ref().unwrap().path().is_file()).count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
