//! SQLite storage. `Database` is not `Sync`; later phases move it onto a
//! dedicated worker thread so queries never run on the GUI thread.

pub mod library;
mod migrations;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating data dir {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> anyhow::Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::run(&mut conn).context("running migrations")?;
        Ok(Self { conn })
    }

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    /// Cached JSON for `key` if it was stored less than `max_age` ago.
    pub fn cache_get(&self, key: &str, max_age: Duration) -> rusqlite::Result<Option<String>> {
        let min_time = now_secs() - max_age.as_secs() as i64;
        self.conn
            .query_row(
                "SELECT json FROM metadata_cache WHERE cache_key = ?1 AND fetched_at >= ?2",
                params![key, min_time],
                |r| r.get(0),
            )
            .optional()
    }

    pub fn cache_put(&self, key: &str, source_id: &str, json: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO metadata_cache (cache_key, source_id, json, fetched_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(cache_key) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at",
            params![key, source_id, json, now_secs()],
        )?;
        Ok(())
    }

    /// Clears the metadata cache. Never touches history, bookmarks or user edits.
    pub fn cache_clear(&self) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM metadata_cache", [])?;
        Ok(())
    }

    /// Ids of sources the user switched off. Sources without a row are enabled.
    pub fn disabled_sources(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM sources WHERE enabled = 0")?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }

    pub fn set_source_enabled(&self, id: &str, enabled: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO sources (id, enabled) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled",
            params![id, enabled],
        )?;
        Ok(())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_upsert() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.setting("reader.direction").unwrap(), None);
        db.set_setting("reader.direction", "rtl").unwrap();
        db.set_setting("reader.direction", "ltr").unwrap();
        assert_eq!(
            db.setting("reader.direction").unwrap().as_deref(),
            Some("ltr")
        );
    }

    #[test]
    fn source_enable_toggle() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.disabled_sources().unwrap().is_empty());
        db.set_source_enabled("anilist", false).unwrap();
        db.set_source_enabled("mangadex", true).unwrap();
        assert_eq!(db.disabled_sources().unwrap(), ["anilist"]);
        db.set_source_enabled("anilist", true).unwrap();
        assert!(db.disabled_sources().unwrap().is_empty());
    }

    #[test]
    fn metadata_cache_expires_and_clears() {
        let db = Database::open_in_memory().unwrap();
        db.cache_put("k", "src", "{}").unwrap();
        let hour = Duration::from_secs(3600);
        assert_eq!(db.cache_get("k", hour).unwrap().as_deref(), Some("{}"));
        db.conn
            .execute(
                "UPDATE metadata_cache SET fetched_at = fetched_at - 7200",
                [],
            )
            .unwrap();
        assert_eq!(db.cache_get("k", hour).unwrap(), None);
        db.cache_clear().unwrap();
        assert_eq!(db.cache_get("k", Duration::MAX / 4).unwrap(), None);
    }

    #[test]
    fn opens_file_database_and_creates_dir() {
        let dir = std::env::temp_dir().join(format!("kuma-test-{}", std::process::id()));
        let path = dir.join("nested/db.sqlite3");
        Database::open(&path)
            .unwrap()
            .set_setting("k", "v")
            .unwrap();
        assert_eq!(
            Database::open(&path)
                .unwrap()
                .setting("k")
                .unwrap()
                .as_deref(),
            Some("v")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
