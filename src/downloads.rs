//! Offline library: chapters saved to disk for reading without a connection.
//!
//! Layout: `<root>/<source>/<work>/entry.json`, `cover.img`, `<chapter>/<page:04>.img`. Folder names
//! are sanitised ids plus a hash, so any source id is a safe, stable path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::models::{Chapter, Manga, MangaKey, PageRef};

/// Token prefix of pages and covers that live on disk.
pub const FILE: &str = "file:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedChapter {
    pub chapter: Chapter,
    pub pages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub manga: Manga,
    /// Reading order, as the source listed them.
    pub chapters: Vec<DownloadedChapter>,
    /// Bytes on disk, filled in by [`Downloads::list`].
    #[serde(skip)]
    pub size: u64,
}

impl Entry {
    pub fn has(&self, chapter_id: &str) -> bool {
        self.chapters.iter().any(|c| c.chapter.id == chapter_id)
    }
}

pub struct Downloads {
    root: PathBuf,
}

/// Progress of one chapter download, shown on the details and Downloads pages.
#[derive(Debug, Clone, PartialEq)]
pub enum DlStatus {
    Queued,
    Running { done: usize, total: usize },
    Failed(String),
}

/// Key for [`DlStatus`] maps: one chapter of one work.
pub fn job_key(key: &MangaKey, chapter_id: &str) -> String {
    format!("{}\u{1}{}\u{1}{chapter_id}", key.source, key.id)
}

/// FNV-1a: tiny, and stable across Rust releases (unlike `DefaultHasher`).
fn fnv(s: &str) -> u64 {
    s.bytes()
        .fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn dir_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(40)
        .collect();
    format!("{safe}-{:012x}", fnv(id) & 0xffff_ffff_ffff)
}

fn dir_size(p: &Path) -> u64 {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

impl Downloads {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn work_dir(&self, key: &MangaKey) -> PathBuf {
        self.root.join(dir_name(&key.source)).join(dir_name(&key.id))
    }

    pub fn chapter_dir(&self, key: &MangaKey, chapter_id: &str) -> PathBuf {
        self.work_dir(key).join(dir_name(chapter_id))
    }

    pub fn page_path(&self, key: &MangaKey, chapter_id: &str, index: usize) -> PathBuf {
        self.chapter_dir(key, chapter_id).join(format!("{index:04}.img"))
    }

    pub fn cover_path(&self, key: &MangaKey) -> PathBuf {
        self.work_dir(key).join("cover.img")
    }

    pub fn load(&self, key: &MangaKey) -> Option<Entry> {
        let json = fs::read_to_string(self.work_dir(key).join("entry.json")).ok()?;
        serde_json::from_str(&json).ok()
    }

    /// Every downloaded work, by title.
    pub fn list(&self) -> Vec<Entry> {
        let mut out: Vec<Entry> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|src| fs::read_dir(src.path()).into_iter().flatten().flatten())
            .filter_map(|work| {
                let json = fs::read_to_string(work.path().join("entry.json")).ok()?;
                let mut e: Entry = serde_json::from_str(&json).ok()?;
                e.size = dir_size(&work.path());
                Some(e)
            })
            .collect();
        out.sort_by(|a, b| a.manga.titles.display().cmp(b.manga.titles.display()));
        out
    }

    /// Adds (or replaces) a finished chapter in the work's entry, keeping the source's order.
    pub fn record(&self, manga: &Manga, all: &[Chapter], chapter: &Chapter, pages: usize) -> io::Result<()> {
        let dir = self.work_dir(&manga.key);
        fs::create_dir_all(&dir)?;
        let mut entry = self.load(&manga.key).unwrap_or(Entry {
            manga: manga.clone(),
            chapters: vec![],
            size: 0,
        });
        entry.manga = manga.clone();
        entry.chapters.retain(|c| c.chapter.id != chapter.id);
        entry.chapters.push(DownloadedChapter { chapter: chapter.clone(), pages });
        let order = |id: &str| all.iter().position(|c| c.id == id).unwrap_or(usize::MAX);
        entry.chapters.sort_by_key(|c| order(&c.chapter.id));
        let json = serde_json::to_string_pretty(&entry).map_err(io::Error::other)?;
        // write-then-rename so a crash never leaves a half-written entry
        let tmp = dir.join("entry.json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(tmp, dir.join("entry.json"))
    }

    /// Page manifest for a downloaded chapter; tokens point at the files.
    pub fn local_pages(&self, key: &MangaKey, chapter_id: &str) -> Option<Vec<PageRef>> {
        let entry = self.load(key)?;
        let n = entry.chapters.iter().find(|c| c.chapter.id == chapter_id)?.pages;
        Some(
            (0..n)
                .map(|i| PageRef {
                    chapter_id: chapter_id.into(),
                    index: i as u32,
                    token: format!("{FILE}{}", self.page_path(key, chapter_id, i).display()),
                })
                .collect(),
        )
    }

    pub fn remove(&self, key: &MangaKey) -> io::Result<()> {
        match fs::remove_dir_all(self.work_dir(key)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Titles;

    fn manga(id: &str) -> Manga {
        Manga {
            key: MangaKey { source: "src/x".into(), id: id.into() },
            titles: Titles { english: Some(id.into()), ..Default::default() },
            description: None,
            artists: vec![],
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags: vec![],
            language: None,
            published: None,
            page_count: None,
            cover_url: None,
            source_url: None,
            links: vec![],
            provenance: Default::default(),
        }
    }

    fn chapter(m: &Manga, id: &str) -> Chapter {
        Chapter {
            manga: m.key.clone(),
            id: id.into(),
            title: None,
            volume: None,
            number: Some(id.into()),
            language: None,
            page_count: None,
            scanlators: vec![],
        }
    }

    #[test]
    fn records_lists_and_removes_in_source_order() {
        let root = std::env::temp_dir().join(format!("kuma-dl-test-{}", std::process::id()));
        let d = Downloads::new(root.clone());
        let m = manga("a/b?c");
        let all = [chapter(&m, "1"), chapter(&m, "2"), chapter(&m, "3")];
        fs::create_dir_all(d.chapter_dir(&m.key, "3")).unwrap();
        fs::write(d.page_path(&m.key, "3", 0), b"x").unwrap();
        d.record(&m, &all, &all[2], 1).unwrap();
        d.record(&m, &all, &all[0], 0).unwrap();
        let list = d.list();
        assert_eq!(list.len(), 1);
        let ids: Vec<_> = list[0].chapters.iter().map(|c| c.chapter.id.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
        assert!(list[0].size > 0);
        let pages = d.local_pages(&m.key, "3").unwrap();
        assert!(pages[0].token.starts_with(FILE));
        assert_eq!(fs::read(&pages[0].token[FILE.len()..]).unwrap(), b"x");
        d.remove(&m.key).unwrap();
        assert!(d.list().is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
