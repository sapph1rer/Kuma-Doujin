use rusqlite::Connection;

/// Ordered schema migrations. Index + 1 is the `PRAGMA user_version` after applying it.
/// Never edit a shipped entry; append a new one.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Per-source user configuration (enabled flag, opt-ins). Not the catalog itself.
CREATE TABLE sources (
    id          TEXT PRIMARY KEY,
    enabled     INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL DEFAULT '{}'
);

-- Metadata cache: serialized `Manga` / search pages with fetch time for expiry.
CREATE TABLE metadata_cache (
    cache_key  TEXT PRIMARY KEY,
    source_id  TEXT NOT NULL,
    json       TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
);
CREATE INDEX metadata_cache_fetched ON metadata_cache(fetched_at);

-- One row per work: where the user is (resume reading + history).
CREATE TABLE reading_progress (
    source_id   TEXT NOT NULL,
    manga_id    TEXT NOT NULL,
    chapter_id  TEXT NOT NULL,
    page_index  INTEGER NOT NULL,
    page_count  INTEGER,
    title       TEXT NOT NULL,
    cover_url   TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (source_id, manga_id)
);
CREATE INDEX reading_progress_updated ON reading_progress(updated_at DESC);

CREATE TABLE bookmarks (
    id          INTEGER PRIMARY KEY,
    source_id   TEXT NOT NULL,
    manga_id    TEXT NOT NULL,
    chapter_id  TEXT NOT NULL,
    page_index  INTEGER NOT NULL,
    note        TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE favorites (
    source_id  TEXT NOT NULL,
    manga_id   TEXT NOT NULL,
    title      TEXT NOT NULL,
    cover_url  TEXT,
    added_at   INTEGER NOT NULL,
    PRIMARY KEY (source_id, manga_id)
);

-- User edits to metadata; merged over source data and never overwritten by refreshes.
CREATE TABLE metadata_overrides (
    source_id  TEXT NOT NULL,
    manga_id   TEXT NOT NULL,
    field      TEXT NOT NULL,
    value_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source_id, manga_id, field)
);
"#,
    // 2: what resume/bookmark lists need to show and reopen a chapter without extra requests.
    r#"
ALTER TABLE reading_progress ADD COLUMN chapter_label TEXT NOT NULL DEFAULT '';
ALTER TABLE reading_progress ADD COLUMN chapter_lang TEXT NOT NULL DEFAULT '';
ALTER TABLE bookmarks ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE bookmarks ADD COLUMN cover_url TEXT;
ALTER TABLE bookmarks ADD COLUMN chapter_label TEXT NOT NULL DEFAULT '';
ALTER TABLE bookmarks ADD COLUMN chapter_lang TEXT NOT NULL DEFAULT '';
CREATE INDEX bookmarks_created ON bookmarks(created_at DESC);
"#,
    // 3: read marks per chapter, recently viewed titles, favorite shelves.
    r#"
CREATE TABLE read_chapters (
    source_id  TEXT NOT NULL,
    manga_id   TEXT NOT NULL,
    chapter_id TEXT NOT NULL,
    read_at    INTEGER NOT NULL,
    PRIMARY KEY (source_id, manga_id, chapter_id)
);
CREATE TABLE recently_viewed (
    source_id  TEXT NOT NULL,
    manga_id   TEXT NOT NULL,
    title      TEXT NOT NULL,
    cover_url  TEXT,
    viewed_at  INTEGER NOT NULL,
    PRIMARY KEY (source_id, manga_id)
);
CREATE INDEX recently_viewed_at ON recently_viewed(viewed_at DESC);
ALTER TABLE favorites ADD COLUMN shelf TEXT NOT NULL DEFAULT 'reading';
"#,
];

pub fn run(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
        tracing::info!(version = i + 1, "applied database migration");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        run(&mut conn).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }
}
