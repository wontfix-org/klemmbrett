//! Which records were picked, when, and after typing what, for ranking.
//!
//! Per namespace and key, the store keeps the number of picks, the last one,
//! and the times of the most recent [`PICKS_KEPT`]. The same is kept per typed
//! query for learned ranking. Old rows go once a namespace has too many.

use rusqlite::{params, Connection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Pick times kept per key (and per key and query) for frecency.
pub const PICKS_KEPT: usize = 16;
/// Rows kept per namespace before the least recently used go.
pub const NAMESPACE_LIMIT: usize = 2000;
/// Command lines remembered per widget, newest kept.
pub const COMMANDS_KEPT: usize = 256;
/// Typed queries are kept up to this many characters.
const QUERY_MAX_CHARS: usize = 32;

/// `$XDG_DATA_HOME/klemmbrett/usage.sqlite`, next to the clipboard history.
pub fn default_usage_path() -> PathBuf {
    crate::history::default_db_path().with_file_name("usage.sqlite")
}

/// A query as it is stored and compared: trimmed, lowercase, and shortened.
pub fn normalize_query(query: &str) -> String {
    query
        .trim()
        .to_lowercase()
        .chars()
        .take(QUERY_MAX_CHARS)
        .collect()
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How one key was picked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyUsage {
    pub count: u64,
    pub last_used: i64,
    /// The most recent pick times, at most [`PICKS_KEPT`].
    pub picks: Vec<i64>,
}

impl KeyUsage {
    /// The kept picks, each halved every `half_life` seconds, scaled up to
    /// the full count so older picks that were dropped still count.
    pub fn frecency(&self, now: i64, half_life: f64) -> f64 {
        if self.picks.is_empty() {
            return 0.0;
        }
        let decayed: f64 = self
            .picks
            .iter()
            .map(|at| 0.5_f64.powf((now - at).max(0) as f64 / half_life))
            .sum();
        decayed * self.count as f64 / self.picks.len() as f64
    }
}

/// Everything recorded under one namespace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NamespaceUsage {
    pub keys: HashMap<String, KeyUsage>,
    /// (normalized query, key, usage) for picks made after typing something.
    pub queries: Vec<(String, String, KeyUsage)>,
}

pub struct UsageStore {
    conn: Connection,
    limit: usize,
    command_limit: usize,
}

impl UsageStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                crate::journal::error(&format!("failed to create {}: {err}", parent.display()));
            }
        }
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage (
                namespace TEXT NOT NULL,
                query TEXT NOT NULL,
                key TEXT NOT NULL,
                count INTEGER NOT NULL,
                last_used INTEGER NOT NULL,
                PRIMARY KEY (namespace, query, key)
            );
            CREATE TABLE IF NOT EXISTS picks (
                namespace TEXT NOT NULL,
                query TEXT NOT NULL,
                key TEXT NOT NULL,
                at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS picks_by_key ON picks (namespace, query, key, at);
            CREATE TABLE IF NOT EXISTS commands (
                namespace TEXT NOT NULL,
                command TEXT NOT NULL,
                last_used INTEGER NOT NULL,
                PRIMARY KEY (namespace, command)
            );",
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(1))?;
        Ok(Self {
            conn,
            limit: NAMESPACE_LIMIT,
            command_limit: COMMANDS_KEPT,
        })
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self
    }

    pub fn with_command_limit(mut self, limit: usize) -> Self {
        self.command_limit = limit.max(1);
        self
    }

    /// Notes that `key` was picked at `at`, after typing `query` (may be empty).
    pub fn record(
        &mut self,
        namespace: &str,
        key: &str,
        query: &str,
        at: i64,
    ) -> rusqlite::Result<()> {
        let query = normalize_query(query);
        let tx = self.conn.transaction()?;
        let queries: &[&str] = if query.is_empty() {
            &[""]
        } else {
            &["", &query]
        };
        for typed in queries {
            tx.execute(
                "INSERT INTO usage (namespace, query, key, count, last_used) VALUES (?1, ?2, ?3, 1, ?4)
                 ON CONFLICT (namespace, query, key)
                 DO UPDATE SET count = count + 1, last_used = max(last_used, ?4)",
                params![namespace, typed, key, at],
            )?;
            tx.execute(
                "INSERT INTO picks (namespace, query, key, at) VALUES (?1, ?2, ?3, ?4)",
                params![namespace, typed, key, at],
            )?;
            tx.execute(
                "DELETE FROM picks WHERE rowid IN (
                    SELECT rowid FROM picks WHERE namespace = ?1 AND query = ?2 AND key = ?3
                    ORDER BY at DESC, rowid DESC LIMIT -1 OFFSET ?4
                )",
                params![namespace, typed, key, PICKS_KEPT as i64],
            )?;
        }
        tx.execute(
            "DELETE FROM picks WHERE (namespace, query, key) IN (
                SELECT namespace, query, key FROM usage WHERE namespace = ?1
                ORDER BY last_used DESC LIMIT -1 OFFSET ?2
            )",
            params![namespace, self.limit as i64],
        )?;
        tx.execute(
            "DELETE FROM usage WHERE rowid IN (
                SELECT rowid FROM usage WHERE namespace = ?1
                ORDER BY last_used DESC LIMIT -1 OFFSET ?2
            )",
            params![namespace, self.limit as i64],
        )?;
        tx.commit()
    }

    /// Everything recorded under `namespace`.
    pub fn namespace(&self, namespace: &str) -> rusqlite::Result<NamespaceUsage> {
        let mut picks: HashMap<(String, String), Vec<i64>> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT query, key, at FROM picks WHERE namespace = ?1 ORDER BY at DESC")?;
        for row in stmt.query_map([namespace], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (query, key, at) = row?;
            picks.entry((query, key)).or_default().push(at);
        }
        let mut used = NamespaceUsage::default();
        let mut stmt = self
            .conn
            .prepare("SELECT query, key, count, last_used FROM usage WHERE namespace = ?1")?;
        for row in stmt.query_map([namespace], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })? {
            let (query, key, count, last_used) = row?;
            let usage = KeyUsage {
                count: count.max(0) as u64,
                last_used,
                picks: picks
                    .remove(&(query.clone(), key.clone()))
                    .unwrap_or_default(),
            };
            if query.is_empty() {
                used.keys.insert(key, usage);
            } else {
                used.queries.push((query, key, usage));
            }
        }
        used.queries.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        Ok(used)
    }

    /// Forgets everything recorded under `namespace`.
    pub fn clear(&mut self, namespace: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM usage WHERE namespace = ?1", [namespace])?;
        self.conn
            .execute("DELETE FROM picks WHERE namespace = ?1", [namespace])?;
        self.conn
            .execute("DELETE FROM commands WHERE namespace = ?1", [namespace])?;
        Ok(())
    }

    /// Remembers `command` under `namespace`, keeping the newest [`COMMANDS_KEPT`].
    ///
    /// `loginctl lock-session` is stored as that line. A later accept of the
    /// same line only moves it forward. Blank lines are ignored.
    pub fn remember_command(
        &mut self,
        namespace: &str,
        command: &str,
        at: i64,
    ) -> rusqlite::Result<()> {
        let command = command.trim();
        if namespace.is_empty() || command.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO commands (namespace, command, last_used) VALUES (?1, ?2, ?3)
             ON CONFLICT (namespace, command)
             DO UPDATE SET last_used = max(last_used, ?3)",
            params![namespace, command, at],
        )?;
        tx.execute(
            "DELETE FROM commands WHERE rowid IN (
                SELECT rowid FROM commands WHERE namespace = ?1
                ORDER BY last_used DESC, command ASC LIMIT -1 OFFSET ?2
            )",
            params![namespace, self.command_limit as i64],
        )?;
        tx.commit()
    }

    /// Drops one remembered command line. A missing line is not an error.
    pub fn forget_command(&mut self, namespace: &str, command: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM commands WHERE namespace = ?1 AND command = ?2",
            params![namespace, command.trim()],
        )?;
        Ok(())
    }

    /// Remembered command lines, newest first.
    pub fn commands(&self, namespace: &str) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT command FROM commands WHERE namespace = ?1
             ORDER BY last_used DESC, command ASC",
        )?;
        let rows = stmt.query_map([namespace], |row| row.get(0))?;
        rows.collect()
    }
}

/// Command lines stored for `namespace`, or nothing if the database cannot be read.
pub fn remembered_commands(namespace: &str) -> Vec<String> {
    let path = default_usage_path();
    match UsageStore::open(&path) {
        Ok(store) => match store.commands(namespace) {
            Ok(commands) => commands,
            Err(err) => {
                crate::journal::error(&format!("failed to read command history: {err}"));
                Vec::new()
            }
        },
        Err(err) => {
            crate::journal::error(&format!("failed to open {}: {err}", path.display()));
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_are_counted_per_key_and_per_query() {
        let mut store = UsageStore::open_in_memory().unwrap();
        store.record("run", "firefox", "", 10).unwrap();
        store.record("run", "firefox", " FI ", 20).unwrap();
        store.record("run", "foot", "fo", 30).unwrap();
        store.record("other", "firefox", "", 40).unwrap();
        let used = store.namespace("run").unwrap();
        assert_eq!(
            used.keys["firefox"],
            KeyUsage {
                count: 2,
                last_used: 20,
                picks: vec![20, 10]
            }
        );
        assert_eq!(used.keys["foot"].count, 1);
        let queries: Vec<_> = used
            .queries
            .iter()
            .map(|(q, k, u)| (q.as_str(), k.as_str(), u.count))
            .collect();
        assert_eq!(queries, [("fi", "firefox", 1), ("fo", "foot", 1)]);
        assert_eq!(store.namespace("other").unwrap().keys.len(), 1);
        assert!(store.namespace("none").unwrap().keys.is_empty());
    }

    #[test]
    fn only_the_latest_picks_are_kept_but_the_count_is_not_lost() {
        let mut store = UsageStore::open_in_memory().unwrap();
        for at in 0..40 {
            store.record("run", "a", "", at).unwrap();
        }
        let used = &store.namespace("run").unwrap().keys["a"];
        assert_eq!(used.count, 40);
        assert_eq!(used.picks.len(), PICKS_KEPT);
        assert_eq!(used.picks[0], 39);
        assert!((used.frecency(39, 1e12) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn frecency_halves_every_half_life() {
        let used = KeyUsage {
            count: 1,
            last_used: 0,
            picks: vec![0],
        };
        assert!((used.frecency(0, 10.0) - 1.0).abs() < 1e-9);
        assert!((used.frecency(10, 10.0) - 0.5).abs() < 1e-9);
        assert!((used.frecency(30, 10.0) - 0.125).abs() < 1e-9);
        assert_eq!(KeyUsage::default().frecency(0, 10.0), 0.0);
    }

    #[test]
    fn least_recently_used_rows_go_past_the_limit() {
        let mut store = UsageStore::open_in_memory().unwrap().with_limit(2);
        store.record("run", "a", "", 1).unwrap();
        store.record("run", "b", "", 2).unwrap();
        store.record("run", "c", "", 3).unwrap();
        store.record("other", "z", "", 0).unwrap();
        let mut keys: Vec<_> = store.namespace("run").unwrap().keys.into_keys().collect();
        keys.sort();
        assert_eq!(keys, ["b", "c"]);
        let left: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM picks WHERE namespace = 'run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 2);
        assert_eq!(store.namespace("other").unwrap().keys.len(), 1);
        store.clear("run").unwrap();
        assert!(store.namespace("run").unwrap().keys.is_empty());
    }

    #[test]
    fn queries_are_normalized() {
        assert_eq!(normalize_query("  FireFox "), "firefox");
        assert_eq!(normalize_query(&"x".repeat(50)).len(), QUERY_MAX_CHARS);
    }

    #[test]
    fn remembered_commands_keep_the_newest_and_can_be_forgotten() {
        let mut store = UsageStore::open_in_memory()
            .unwrap()
            .with_command_limit(2);
        store
            .remember_command("run", "  loginctl lock-session ", 1)
            .unwrap();
        store.remember_command("run", "foot -e htop", 2).unwrap();
        store
            .remember_command("run", "loginctl lock-session", 4)
            .unwrap();
        store.remember_command("run", "notify-send hi", 3).unwrap();
        store.remember_command("other", "keep", 9).unwrap();
        store.remember_command("run", "   ", 5).unwrap();
        assert_eq!(
            store.commands("run").unwrap(),
            vec![
                "loginctl lock-session".to_string(),
                "notify-send hi".to_string(),
            ]
        );
        store.forget_command("run", "notify-send hi").unwrap();
        store.forget_command("run", "missing").unwrap();
        assert_eq!(
            store.commands("run").unwrap(),
            vec!["loginctl lock-session".to_string()]
        );
        assert_eq!(store.commands("other").unwrap(), vec!["keep".to_string()]);
        store.clear("run").unwrap();
        assert!(store.commands("run").unwrap().is_empty());
        assert_eq!(store.commands("other").unwrap(), vec!["keep".to_string()]);
    }

    #[test]
    fn the_store_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("usage.sqlite");
        UsageStore::open(&path)
            .unwrap()
            .record("run", "a", "a", 5)
            .unwrap();
        let used = UsageStore::open(&path).unwrap().namespace("run").unwrap();
        assert_eq!(used.keys["a"].last_used, 5);
        assert_eq!(used.queries.len(), 1);
    }
}
