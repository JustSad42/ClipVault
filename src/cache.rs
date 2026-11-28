//! What the list needs about each item, worked out once and kept until the item changes:
//! its card, preview and search words; thumbnails, site icons and program icons.

use crate::gfx::Image;
use crate::images::Bitmap;
use crate::model::{Facts, FileStats, Item, Kind};
use crate::search::Index;
use crate::store::Store;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

pub struct Info {
    revision: u64,
    pub facts: Facts,
    pub index: Index,
    files: Option<(FileStats, Instant)>,
}

pub fn drawable(b: Bitmap) -> Rc<Image> {
    Rc::new(Image { w: b.w, h: b.h, px: b.premultiplied() })
}

/// Pictures kept ready to draw; the least recently used go first.
#[derive(Default)]
struct Recent {
    map: HashMap<String, (Option<Rc<Image>>, u64)>,
    clock: u64,
}

impl Recent {
    const MAX: usize = 48;

    fn get(&mut self, key: &str, load: impl FnOnce() -> Option<Rc<Image>>) -> Option<Rc<Image>> {
        self.clock += 1;
        if let Some(e) = self.map.get_mut(key) {
            e.1 = self.clock;
            return e.0.clone();
        }
        if self.map.len() >= Self::MAX {
            if let Some(oldest) = self.map.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| k.clone()) {
                self.map.remove(&oldest);
            }
        }
        let image = load();
        self.map.insert(key.to_string(), (image.clone(), self.clock));
        image
    }
}

#[derive(Default)]
pub struct Cache {
    info: HashMap<String, Info>,
    pictures: Recent,
    /// Program icons by exe path (small), file icons by path (large).
    icons: HashMap<(String, bool), Option<Rc<Image>>>,
    favicons: HashMap<String, Option<Rc<Image>>>,
    pending: HashSet<String>,
}

impl Cache {
    pub fn info(&mut self, item: &Item, revision: u64) -> &Info {
        let stale = self.info.get(&item.id).is_none_or(|i| i.revision != revision);
        if stale {
            let made = Info { revision, facts: item.facts(), index: Index::new(item), files: None };
            self.info.insert(item.id.clone(), made);
        }
        &self.info[&item.id]
    }

    /// Size and date of copied files, re-read every 30 seconds.
    pub fn files(&mut self, item: &Item, revision: u64) -> Option<FileStats> {
        if item.kind != Kind::Files {
            return None;
        }
        self.info(item, revision);
        let info = self.info.get_mut(&item.id)?;
        if info.files.as_ref().is_none_or(|(_, at)| at.elapsed().as_secs() >= 30) {
            info.files = Some((item.file_stats(chrono::Local::now()), Instant::now()));
        }
        info.files.as_ref().map(|(s, _)| s.clone())
    }

    /// The small picture of an image item (from its thumbnail file, made if missing).
    pub fn thumbnail(&mut self, item: &Item, store: &Store) -> Option<Rc<Image>> {
        let file = item.image_file.as_deref()?;
        let thumb = store.thumb_path(file);
        let full = store.images().join(file);
        self.pictures.get(file, || {
            if let Ok(b) = Bitmap::load(&thumb, None) {
                return Some(drawable(b));
            }
            let b = Bitmap::load(&full, Some(crate::store::THUMB)).ok()?;
            if item.pixel_width.max(item.pixel_height) > crate::store::THUMB {
                if let (Some(dir), Ok(png)) = (thumb.parent(), b.png()) {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = crate::win::write_atomic(&thumb, &png);
                }
            }
            Some(drawable(b))
        })
    }

    /// A photo among copied files, small.
    pub fn photo(&mut self, path: &str) -> Option<Rc<Image>> {
        if path.starts_with(r"\\") {
            return None;
        }
        self.pictures.get(path, || Bitmap::load(&PathBuf::from(path), Some(96)).ok().map(drawable))
    }

    /// A larger picture for the preview and tooltip (not kept long).
    pub fn full(&mut self, item: &Item, store: &Store, max: u32) -> Option<Rc<Image>> {
        let path = store.image_path(item)?;
        let key = format!("{}:{max}", path.display());
        self.pictures.get(&key, || Bitmap::load(&path, Some(max)).ok().map(drawable))
    }

    pub fn icon(&mut self, path: &str, small: bool) -> Option<Rc<Image>> {
        if self.icons.len() > 300 {
            self.icons.clear();
        }
        self.icons.entry((path.to_string(), small)).or_insert_with(|| crate::system::shell_icon(path, small).map(drawable)).clone()
    }

    /// A site's icon if it's ready; otherwise starts fetching it (when allowed) and returns
    /// `None` for now: `Event::Favicon` arrives later.
    pub fn favicon(&mut self, scheme: &str, host: &str, download: bool) -> Option<Rc<Image>> {
        if let Some(i) = self.favicons.get(host) {
            return i.clone();
        }
        let file = crate::net::favicon_file(host)?;
        if let Ok(bytes) = std::fs::read(&file) {
            let icon = Bitmap::decode_icon(&bytes, 32).ok().map(drawable);
            self.favicons.insert(host.to_string(), icon.clone());
            return icon;
        }
        if download && !std::path::Path::new(&format!("{}.none", file.display())).exists() && self.pending.insert(host.to_string()) {
            let (scheme, host) = (scheme.to_string(), host.to_string());
            std::thread::spawn(move || {
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                }
                let icon = crate::net::fetch_favicon(&scheme, &host);
                crate::app::post(crate::app::Event::Favicon(host, icon));
            });
        }
        None
    }

    pub fn favicon_arrived(&mut self, host: &str, icon: Option<Bitmap>) {
        self.pending.remove(host);
        self.favicons.insert(host.to_string(), icon.map(drawable));
    }

    /// Forgets items that are gone.
    pub fn prune(&mut self, store: &Store) {
        if self.info.len() > store.items.len() + 200 {
            let live: HashSet<&str> = store.items.iter().map(|i| i.id.as_str()).collect();
            self.info.retain(|id, _| live.contains(id.as_str()));
        }
    }
}
