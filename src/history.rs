//! Persistent clipboard history backed by SQLite.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default number of rows kept in the database.
pub const DEFAULT_HISTORY_LIMIT: usize = 50;
/// Default maximum characters shown for a history item in the popup.
pub const DEFAULT_LABEL_MAX: usize = 64;

pub fn default_db_path() -> PathBuf {
    db_path_from_env(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

pub fn db_path_from_env(xdg_data_home: Option<&std::ffi::OsStr>, home: Option<&std::ffi::OsStr>) -> PathBuf {
    let data_home = xdg_data_home
        .map(PathBuf::from)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    data_home.join("klemmbrett").join("history.sqlite")
}

/// Collapse whitespace and truncate to `max_chars`, appending an ellipsis
/// when the original (collapsed) text is longer.
pub fn shorten_label(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_chars == 0 {
        return String::new();
    }
    let count = collapsed.chars().count();
    if count <= max_chars {
        return collapsed;
    }
    let take = max_chars.saturating_sub(1);
    let mut out: String = collapsed.chars().take(take).collect();
    out.push('…');
    out
}

pub struct HistoryStore {
    conn: Connection,
    limit: usize,
}

impl HistoryStore {
    pub fn open(path: &Path, limit: usize) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn, limit)
    }

    pub fn open_in_memory(limit: usize) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, limit)
    }

    fn from_connection(conn: Connection, limit: usize) -> rusqlite::Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        let store = Self {
            conn,
            limit: limit.max(1),
        };
        store.trim()?;
        Ok(store)
    }

    pub fn record(&mut self, text: &str) -> rusqlite::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if self.newest_text()?.as_deref() == Some(text) {
            return Ok(());
        }
        // Re-copying an older item moves it to the top instead of duplicating it.
        self.conn
            .execute("DELETE FROM history WHERE text = ?1", [text])?;
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO history (text, created_at) VALUES (?1, ?2)",
            params![text, created_at],
        )?;
        self.trim()
    }

    pub fn entries(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT text FROM history ORDER BY id DESC")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    fn newest_text(&self) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT text FROM history ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
    }

    fn trim(&self) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM history WHERE id NOT IN (
                SELECT id FROM history ORDER BY id DESC LIMIT ?1
            )",
            params![self.limit as i64],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_leaves_short_text_alone() {
        assert_eq!(shorten_label("hello", 64), "hello");
    }

    #[test]
    fn shorten_collapses_whitespace() {
        assert_eq!(shorten_label("a\n\tb   c", 64), "a b c");
    }

    #[test]
    fn shorten_truncates_with_ellipsis() {
        assert_eq!(shorten_label("abcdefghij", 7), "abcdef…");
    }

    #[test]
    fn shorten_counts_unicode_chars() {
        assert_eq!(shorten_label("ääääää", 4), "äää…");
    }

    #[test]
    fn records_and_returns_newest_first() {
        let mut store = HistoryStore::open_in_memory(10).unwrap();
        store.record("one").unwrap();
        store.record("two").unwrap();
        assert_eq!(store.entries().unwrap(), vec!["two".to_owned(), "one".to_owned()]);
    }

    #[test]
    fn consecutive_duplicates_are_ignored() {
        let mut store = HistoryStore::open_in_memory(10).unwrap();
        store.record("one").unwrap();
        store.record("one").unwrap();
        assert_eq!(store.entries().unwrap().len(), 1);
    }

    #[test]
    fn recopying_an_older_item_moves_it_to_the_top() {
        let mut store = HistoryStore::open_in_memory(10).unwrap();
        store.record("one").unwrap();
        store.record("two").unwrap();
        store.record("one").unwrap();
        assert_eq!(store.entries().unwrap(), vec!["one".to_owned(), "two".to_owned()]);
        assert_eq!(store.entries().unwrap().len(), 2);
    }

    #[test]
    fn old_rows_are_deleted_when_over_the_limit() {
        let mut store = HistoryStore::open_in_memory(2).unwrap();
        store.record("one").unwrap();
        store.record("two").unwrap();
        store.record("three").unwrap();
        assert_eq!(
            store.entries().unwrap(),
            vec!["three".to_owned(), "two".to_owned()]
        );
        assert_eq!(store.entries().unwrap().len(), 2);
    }

    #[test]
    fn empty_text_is_not_stored() {
        let mut store = HistoryStore::open_in_memory(10).unwrap();
        store.record("").unwrap();
        assert!(store.entries().unwrap().is_empty());
    }

    #[test]
    fn shorten_empty_max_yields_empty_label() {
        assert_eq!(shorten_label("hello", 0), "");
    }

    #[test]
    fn xdg_data_home_wins_over_home() {
        use std::ffi::OsStr;
        assert_eq!(
            db_path_from_env(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/me"))),
            PathBuf::from("/xdg/klemmbrett/history.sqlite")
        );
    }

    #[test]
    fn home_is_used_when_xdg_data_home_is_unset() {
        use std::ffi::OsStr;
        assert_eq!(
            db_path_from_env(None, Some(OsStr::new("/home/me"))),
            PathBuf::from("/home/me/.local/share/klemmbrett/history.sqlite")
        );
    }

    #[test]
    fn tmp_is_used_when_no_home_is_known() {
        assert_eq!(
            db_path_from_env(None, None),
            PathBuf::from("/tmp/klemmbrett/history.sqlite")
        );
    }

    #[test]
    fn a_zero_limit_still_keeps_one_row() {
        let mut store = HistoryStore::open_in_memory(0).unwrap();
        store.record("one").unwrap();
        store.record("two").unwrap();
        assert_eq!(store.entries().unwrap(), vec!["two".to_owned()]);
    }

    #[test]
    fn history_survives_reopening_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        {
            let mut store = HistoryStore::open(&path, 10).unwrap();
            store.record("one").unwrap();
            store.record("two").unwrap();
        }
        let store = HistoryStore::open(&path, 10).unwrap();
        assert_eq!(
            store.entries().unwrap(),
            vec!["two".to_owned(), "one".to_owned()]
        );
    }

    #[test]
    fn reopening_with_a_smaller_limit_deletes_old_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        {
            let mut store = HistoryStore::open(&path, 5).unwrap();
            store.record("one").unwrap();
            store.record("two").unwrap();
            store.record("three").unwrap();
        }
        let store = HistoryStore::open(&path, 2).unwrap();
        assert_eq!(
            store.entries().unwrap(),
            vec!["three".to_owned(), "two".to_owned()]
        );
    }
}
