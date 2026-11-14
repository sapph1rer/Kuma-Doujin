//! The user's library: reading progress (history + resume), bookmarks and favorites.

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Database, now_secs};

/// Unix time in milliseconds: progress is saved on every page turn, so seconds would tie.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
use crate::core::models::MangaKey;

/// Where to reopen a work: chapter (with the translation language its list was filtered by)
/// and 0-based page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePoint {
    pub key: MangaKey,
    pub chapter_id: String,
    pub chapter_lang: String,
    pub page: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub at: ResumePoint,
    pub chapter_label: String,
    pub page_count: Option<u32>,
    pub title: String,
    pub cover_url: Option<String>,
    /// Unix milliseconds.
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub id: i64,
    pub at: ResumePoint,
    pub chapter_label: String,
    pub title: String,
    pub cover_url: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Favorite {
    pub key: MangaKey,
    pub title: String,
    pub cover_url: Option<String>,
    pub added_at: i64,
    /// One of [`SHELVES`] (stored id).
    pub shelf: String,
}

/// Favorite shelves: (stored id, label).
pub const SHELVES: [(&str, &str); 3] = [
    ("reading", "Reading"),
    ("completed", "Completed"),
    ("plan", "Plan to read"),
];

/// Recently opened titles kept for the Home row.
const RECENT_KEEP: i64 = 30;

const PROGRESS_COLS: &str = "source_id, manga_id, chapter_id, chapter_lang, page_index, \
    chapter_label, page_count, title, cover_url, updated_at";

fn progress_row(r: &Row) -> rusqlite::Result<Progress> {
    Ok(Progress {
        at: ResumePoint {
            key: MangaKey {
                source: r.get(0)?,
                id: r.get(1)?,
            },
            chapter_id: r.get(2)?,
            chapter_lang: r.get(3)?,
            page: r.get::<_, i64>(4)?.max(0) as usize,
        },
        chapter_label: r.get(5)?,
        page_count: r.get(6)?,
        title: r.get(7)?,
        cover_url: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

impl Database {
    /// Upserts the single progress row for this work and stamps it with the current time.
    pub fn save_progress(&self, p: &Progress) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO reading_progress (source_id, manga_id, chapter_id, chapter_lang, page_index,
                chapter_label, page_count, title, cover_url, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                -- strictly increasing, so the most recent row never ties
                MAX(?10, (SELECT COALESCE(MAX(updated_at), 0) + 1 FROM reading_progress)))
             ON CONFLICT(source_id, manga_id) DO UPDATE SET
                chapter_id = excluded.chapter_id, chapter_lang = excluded.chapter_lang,
                page_index = excluded.page_index, chapter_label = excluded.chapter_label,
                page_count = excluded.page_count, title = excluded.title,
                cover_url = excluded.cover_url, updated_at = excluded.updated_at",
            params![
                p.at.key.source,
                p.at.key.id,
                p.at.chapter_id,
                p.at.chapter_lang,
                p.at.page as i64,
                p.chapter_label,
                p.page_count,
                p.title,
                p.cover_url,
                now_millis()
            ],
        )?;
        Ok(())
    }

    pub fn progress(&self, key: &MangaKey) -> rusqlite::Result<Option<Progress>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {PROGRESS_COLS} FROM reading_progress WHERE source_id = ?1 AND manga_id = ?2"
                ),
                params![key.source, key.id],
                progress_row,
            )
            .optional()
    }

    /// Most recently read first.
    pub fn recent_progress(&self, limit: usize) -> rusqlite::Result<Vec<Progress>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROGRESS_COLS} FROM reading_progress ORDER BY updated_at DESC LIMIT ?1"
        ))?;
        stmt.query_map([limit as i64], progress_row)?.collect()
    }

    pub fn delete_progress(&self, key: &MangaKey) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM reading_progress WHERE source_id = ?1 AND manga_id = ?2",
            params![key.source, key.id],
        )?;
        Ok(())
    }

    pub fn clear_progress(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM reading_progress", [])?;
        Ok(())
    }

    pub fn add_bookmark(&self, b: &Bookmark) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO bookmarks (source_id, manga_id, chapter_id, chapter_lang, page_index,
                chapter_label, title, cover_url, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                b.at.key.source,
                b.at.key.id,
                b.at.chapter_id,
                b.at.chapter_lang,
                b.at.page as i64,
                b.chapter_label,
                b.title,
                b.cover_url,
                now_secs()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn bookmarks(&self) -> rusqlite::Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_id, manga_id, chapter_id, chapter_lang, page_index, chapter_label,
                title, cover_url, created_at
             FROM bookmarks ORDER BY created_at DESC, id DESC",
        )?;
        stmt.query_map([], |r| {
            Ok(Bookmark {
                id: r.get(0)?,
                at: ResumePoint {
                    key: MangaKey {
                        source: r.get(1)?,
                        id: r.get(2)?,
                    },
                    chapter_id: r.get(3)?,
                    chapter_lang: r.get(4)?,
                    page: r.get::<_, i64>(5)?.max(0) as usize,
                },
                chapter_label: r.get(6)?,
                title: r.get(7)?,
                cover_url: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?
        .collect()
    }

    pub fn delete_bookmark(&self, id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_favorite(&self, fav: &Favorite, on: bool) -> rusqlite::Result<()> {
        if on {
            self.conn.execute(
                "INSERT INTO favorites (source_id, manga_id, title, cover_url, added_at, shelf)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(source_id, manga_id) DO UPDATE SET
                    title = excluded.title, cover_url = excluded.cover_url, shelf = excluded.shelf",
                params![
                    fav.key.source,
                    fav.key.id,
                    fav.title,
                    fav.cover_url,
                    now_secs(),
                    fav.shelf
                ],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM favorites WHERE source_id = ?1 AND manga_id = ?2",
                params![fav.key.source, fav.key.id],
            )?;
        }
        Ok(())
    }

    pub fn favorites(&self) -> rusqlite::Result<Vec<Favorite>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, manga_id, title, cover_url, added_at, shelf
             FROM favorites ORDER BY added_at DESC, rowid DESC",
        )?;
        stmt.query_map([], |r| {
            Ok(Favorite {
                key: MangaKey {
                    source: r.get(0)?,
                    id: r.get(1)?,
                },
                title: r.get(2)?,
                cover_url: r.get(3)?,
                added_at: r.get(4)?,
                shelf: r.get(5)?,
            })
        })?
        .collect()
    }

    /// Marks chapters of a work read or unread.
    pub fn set_chapters_read(&self, key: &MangaKey, chapter_ids: &[String], read: bool) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for id in chapter_ids {
            if read {
                tx.execute(
                    "INSERT OR IGNORE INTO read_chapters (source_id, manga_id, chapter_id, read_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![key.source, key.id, id, now_secs()],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM read_chapters WHERE source_id = ?1 AND manga_id = ?2 AND chapter_id = ?3",
                    params![key.source, key.id, id],
                )?;
            }
        }
        tx.commit()
    }

    pub fn read_chapters(&self, key: &MangaKey) -> rusqlite::Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT chapter_id FROM read_chapters WHERE source_id = ?1 AND manga_id = ?2")?;
        stmt.query_map(params![key.source, key.id], |r| r.get(0))?.collect()
    }

    /// Moves a work to the front of "recently viewed", keeping the newest [`RECENT_KEEP`].
    pub fn touch_viewed(&self, f: &Favorite) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO recently_viewed (source_id, manga_id, title, cover_url, viewed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_id, manga_id) DO UPDATE SET
                title = excluded.title, cover_url = excluded.cover_url, viewed_at = excluded.viewed_at",
            params![f.key.source, f.key.id, f.title, f.cover_url, now_millis()],
        )?;
        self.conn.execute(
            "DELETE FROM recently_viewed WHERE rowid NOT IN
                (SELECT rowid FROM recently_viewed ORDER BY viewed_at DESC, rowid DESC LIMIT ?1)",
            [RECENT_KEEP],
        )?;
        Ok(())
    }

    /// Newest first; `added_at` holds the view time (unix ms).
    pub fn recently_viewed(&self, limit: usize) -> rusqlite::Result<Vec<Favorite>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, manga_id, title, cover_url, viewed_at
             FROM recently_viewed ORDER BY viewed_at DESC, rowid DESC LIMIT ?1",
        )?;
        stmt.query_map([limit as i64], |r| {
            Ok(Favorite {
                key: MangaKey { source: r.get(0)?, id: r.get(1)? },
                title: r.get(2)?,
                cover_url: r.get(3)?,
                added_at: r.get(4)?,
                shelf: String::new(),
            })
        })?
        .collect()
    }

    pub fn clear_recently_viewed(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM recently_viewed", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: &str, page: usize) -> ResumePoint {
        ResumePoint {
            key: MangaKey {
                source: "md".into(),
                id: id.into(),
            },
            chapter_id: format!("{id}-c1"),
            chapter_lang: "en".into(),
            page,
        }
    }

    fn progress(id: &str, page: usize) -> Progress {
        Progress {
            at: point(id, page),
            chapter_label: "Ch. 1".into(),
            page_count: Some(20),
            title: format!("Title {id}"),
            cover_url: None,
            updated_at: 0,
        }
    }

    #[test]
    fn progress_upserts_orders_and_deletes() {
        let db = Database::open_in_memory().unwrap();
        db.save_progress(&progress("a", 1)).unwrap();
        db.save_progress(&progress("b", 2)).unwrap();
        db.save_progress(&progress("a", 7)).unwrap(); // update, now most recent

        let recent = db.recent_progress(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].at, point("a", 7));
        assert_eq!(recent[0].chapter_label, "Ch. 1");
        assert_eq!(db.progress(&point("b", 0).key).unwrap().unwrap().at.page, 2);

        db.delete_progress(&point("a", 0).key).unwrap();
        assert_eq!(db.recent_progress(10).unwrap().len(), 1);
        db.clear_progress().unwrap();
        assert!(db.recent_progress(10).unwrap().is_empty());
    }

    #[test]
    fn bookmarks_add_list_delete() {
        let db = Database::open_in_memory().unwrap();
        let b = Bookmark {
            id: 0,
            at: point("a", 4),
            chapter_label: "Ch. 1".into(),
            title: "A".into(),
            cover_url: Some("https://x/c.jpg".into()),
            created_at: 0,
        };
        let first = db.add_bookmark(&b).unwrap();
        let second = db.add_bookmark(&b).unwrap();
        let list = db.bookmarks().unwrap();
        assert_eq!(
            list.iter().map(|b| b.id).collect::<Vec<_>>(),
            [second, first]
        );
        assert_eq!(list[0].at.page, 4);
        db.delete_bookmark(first).unwrap();
        assert_eq!(db.bookmarks().unwrap().len(), 1);
    }

    #[test]
    fn favorites_toggle() {
        let db = Database::open_in_memory().unwrap();
        let f = Favorite {
            shelf: "reading".into(),
            key: point("a", 0).key,
            title: "A".into(),
            cover_url: None,
            added_at: 0,
        };
        db.set_favorite(&f, true).unwrap();
        db.set_favorite(&f, true).unwrap();
        assert_eq!(db.favorites().unwrap().len(), 1);
        db.set_favorite(&f, false).unwrap();
        assert!(db.favorites().unwrap().is_empty());
    }

    #[test]
    fn read_marks_and_recently_viewed() {
        let db = Database::open_in_memory().unwrap();
        let key = MangaKey { source: "s".into(), id: "m".into() };
        db.set_chapters_read(&key, &["1".into(), "2".into()], true).unwrap();
        db.set_chapters_read(&key, &["1".into()], false).unwrap();
        assert_eq!(db.read_chapters(&key).unwrap(), ["2".to_string()].into_iter().collect());
        for i in 0..35 {
            let f = Favorite {
                key: MangaKey { source: "s".into(), id: i.to_string() },
                title: format!("t{i}"),
                cover_url: None,
                added_at: 0,
                shelf: String::new(),
            };
            db.touch_viewed(&f).unwrap();
        }
        let recent = db.recently_viewed(100).unwrap();
        assert_eq!(recent.len(), RECENT_KEEP as usize);
        assert_eq!(recent[0].key.id, "34");
    }
}
