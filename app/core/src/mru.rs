//! SQLite-backed MRU (most-recently-used) activation history.
//!
//! `MruStore` is a thin wrapper around a `rusqlite::Connection` that stores
//! one row per activated `EntryRef`. The schema is:
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS mru (
//!     entry_ref TEXT NOT NULL PRIMARY KEY,
//!     last_used INTEGER NOT NULL
//! ) STRICT;
//! ```
//!
//! `entry_ref` is the JSON serialization of an `EntryRef` (the same
//! serialization used elsewhere in the workspace via `serde_json`). The
//! PRIMARY KEY enforces dedup; writes are UPSERTs that update `last_used`
//! in place. `read_all` returns rows ordered by `last_used DESC` so the
//! launcher can use the result directly as the recency-sorted index.
//!
//! Errors are surfaced via `MruError`; the launcher logs and continues on
//! failure rather than panicking.

use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::EntryRef;

/// Busy-timeout (milliseconds) applied via PRAGMA on every open. Five seconds
/// is generous enough that a concurrent writer holding the WAL lock has time
/// to finish before we give up; the launcher's own writes are microseconds.
const BUSY_TIMEOUT_MS: u32 = 5000;

/// SQLite-backed activation history. One row per `EntryRef`; `last_used` is
/// updated in place on repeat bumps. The connection stays open for the life
/// of the value; `Drop` closes it.
pub struct MruStore {
    conn: Connection,
}

/// All failure modes the MRU store can return. The launcher converts these to
/// `eprintln!` and continues with an empty in-memory index — corruption,
/// permission errors, and disk-full are never fatal to the launcher itself.
#[derive(Debug)]
pub enum MruError {
    Io(std::io::Error),
    Sql(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for MruError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MruError::Io(e) => write!(f, "mru io error: {e}"),
            MruError::Sql(e) => write!(f, "mru sql error: {e}"),
            MruError::Json(e) => write!(f, "mru json error: {e}"),
        }
    }
}

impl std::error::Error for MruError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MruError::Io(e) => Some(e),
            MruError::Sql(e) => Some(e),
            MruError::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for MruError {
    fn from(e: std::io::Error) -> Self {
        MruError::Io(e)
    }
}

impl From<rusqlite::Error> for MruError {
    fn from(e: rusqlite::Error) -> Self {
        MruError::Sql(e)
    }
}

impl From<serde_json::Error> for MruError {
    fn from(e: serde_json::Error) -> Self {
        MruError::Json(e)
    }
}

impl MruStore {
    /// Open (or create) the SQLite database at `path`. Creates any missing
    /// parent directories, applies the WAL and busy-timeout pragmas, and
    /// runs the idempotent schema migration. Safe to call against an
    /// existing file written by a prior process.
    pub fn open(path: &Path) -> Result<Self, MruError> {
        if let Some(parent) = path.parent() {
            // Skip the empty parent that `Path::new("mru.sqlite").parent()`
            // returns — `create_dir_all("")` errors with "No such file or
            // directory" on some platforms.
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let conn = Connection::open(path)?;

        // `journal_mode = WAL` lets readers and writers proceed concurrently;
        // combined with `busy_timeout` it tolerates two LoFi processes writing
        // to the same DB without surfacing SQLITE_BUSY to the caller.
        // `query_row` instead of `execute` because PRAGMA journal_mode returns
        // a result row.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS mru (\n                entry_ref TEXT NOT NULL PRIMARY KEY,\n                last_used INTEGER NOT NULL\n            ) STRICT",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mru_last_used ON mru(last_used DESC)",
            [],
        )?;

        Ok(MruStore { conn })
    }

    /// Read every row, most-recent-first. Per-row JSON parse errors are
    /// logged via `eprintln!` and skipped — a single corrupt row must not
    /// prevent the rest of the history from loading.
    pub fn read_all(&self) -> Result<Vec<EntryRef>, MruError> {
        let mut stmt = self
            .conn
            .prepare("SELECT entry_ref FROM mru ORDER BY last_used DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        let mut out = Vec::new();
        for row in rows {
            let json = row?;
            match serde_json::from_str::<EntryRef>(&json) {
                Ok(r) => out.push(r),
                Err(err) => {
                    eprintln!("mru: skipping corrupt row: {err}");
                    continue;
                }
            }
        }
        Ok(out)
    }

    /// Record `r` as just-activated. UPSERTs the row with `last_used = now`
    /// (Unix epoch milliseconds). Repeat bumps on the same ref update the
    /// timestamp in place rather than inserting a duplicate.
    pub fn bump(&self, r: &EntryRef) -> Result<(), MruError> {
        let now_millis = i64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| MruError::Io(std::io::Error::other(e)))?
                .as_millis(),
        )
        .map_err(|e| MruError::Io(std::io::Error::other(e)))?;

        let json = serde_json::to_string(r)?;

        self.conn.execute(
            "INSERT INTO mru (entry_ref, last_used) VALUES (?1, ?2) \
             ON CONFLICT(entry_ref) DO UPDATE SET last_used = excluded.last_used",
            rusqlite::params![json, now_millis],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{EntryRef, MruStore};
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    /// Most tests sleep between bumps so the wall-clock-millisecond timestamps
    /// land in distinct buckets and `read_all`'s recency ordering is
    /// deterministic. 2ms is the smallest value that has been observed to be
    /// reliable across CI runners; smaller (e.g. 1ms) occasionally races.
    const BUMP_GAP: Duration = Duration::from_millis(2);

    /// Helper: build an `EntryRef::Application` from a borrowed desktop id.
    fn app_ref(id: &str) -> EntryRef {
        EntryRef::Application(id.to_string())
    }

    /// Helper: build an `EntryRef::Window` from a Mutter window id.
    fn win_ref(id: u64) -> EntryRef {
        EntryRef::Window(id)
    }

    /// Helper: build a fresh SQLite path under a tempdir, nested under a
    /// directory that does not yet exist so we exercise the parent-dir-create
    /// behaviour of `MruStore::open`.
    fn temp_db_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().expect("tempdir should be creatable");
        let path = dir.path().join("nested").join("inner").join("mru.sqlite");
        (dir, path)
    }

    #[test]
    fn open_creates_db_and_parent_dir() {
        let (_guard, path) = temp_db_path();
        // The intermediate directories don't exist yet; open() should create
        // them along with the SQLite file itself.
        assert!(
            !path.exists(),
            "precondition: SQLite path should not exist yet; got {path:?}",
        );
        assert!(
            !path.parent().unwrap().exists(),
            "precondition: parent dir should not exist yet; got {:?}",
            path.parent()
        );

        let _store = MruStore::open(&path).expect("open should succeed");

        assert!(
            path.exists(),
            "open should create the SQLite file at {path:?}",
        );
        assert!(
            path.parent().unwrap().is_dir(),
            "open should create the parent directory at {:?}",
            path.parent()
        );
    }

    #[test]
    fn read_all_returns_empty_for_fresh_db() {
        let (_guard, path) = temp_db_path();
        let store = MruStore::open(&path).expect("open should succeed");

        let rows = store.read_all().expect("read_all should succeed");
        assert!(
            rows.is_empty(),
            "fresh DB should have no rows; got {rows:?}",
        );
    }

    #[test]
    fn bump_then_read_round_trips_application_ref() {
        let (_guard, path) = temp_db_path();
        let store = MruStore::open(&path).expect("open should succeed");

        let r = app_ref("firefox.desktop");
        store.bump(&r).expect("bump should succeed");

        let rows = store.read_all().expect("read_all should succeed");
        assert_eq!(
            rows,
            vec![r.clone()],
            "read_all should return the one ref we bumped; got {rows:?}",
        );
    }

    #[test]
    fn bump_then_read_round_trips_window_ref() {
        // Windows are keyed by u64 ids; the schema stores entry_ref as TEXT
        // (JSON), so this test verifies the numeric type survives the
        // serialize/deserialize trip without truncation or sign-flipping.
        const WINDOW_ID: u64 = 12345;
        let (_guard, path) = temp_db_path();
        let store = MruStore::open(&path).expect("open should succeed");

        let r = win_ref(WINDOW_ID);
        store.bump(&r).expect("bump should succeed");

        let rows = store.read_all().expect("read_all should succeed");
        assert_eq!(
            rows,
            vec![r.clone()],
            "read_all should return the Window ref we bumped (u64 round-trip via TEXT JSON); got {rows:?}",
        );
    }

    #[test]
    fn read_all_orders_by_recency_desc() {
        let (_guard, path) = temp_db_path();
        let store = MruStore::open(&path).expect("open should succeed");

        let a = app_ref("a.desktop");
        let b = app_ref("b.desktop");
        let c = app_ref("c.desktop");

        store.bump(&a).expect("bump a should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&b).expect("bump b should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&c).expect("bump c should succeed");

        let rows = store.read_all().expect("read_all should succeed");
        assert_eq!(
            rows,
            vec![c.clone(), b.clone(), a.clone()],
            "read_all should return rows in most-recent-first order; got {rows:?}",
        );
    }

    #[test]
    fn bump_existing_ref_updates_timestamp_in_place() {
        // The schema uses entry_ref as PRIMARY KEY with an ON CONFLICT UPSERT;
        // bumping the same ref twice must not insert a duplicate row, only
        // update last_used. After bump(A), bump(B), bump(A), A is now more
        // recent than B and there are exactly 2 rows.
        const EXPECTED_ROW_COUNT: usize = 2;

        let (_guard, path) = temp_db_path();
        let store = MruStore::open(&path).expect("open should succeed");

        let a = app_ref("a.desktop");
        let b = app_ref("b.desktop");

        store.bump(&a).expect("bump a (1st) should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&b).expect("bump b should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&a).expect("bump a (2nd) should succeed");

        let rows = store.read_all().expect("read_all should succeed");
        assert_eq!(
            rows.len(),
            EXPECTED_ROW_COUNT,
            "bumping an existing ref must update in place, not insert; got rows {rows:?}",
        );
        assert_eq!(
            rows,
            vec![a.clone(), b.clone()],
            "after bump(A), bump(B), bump(A) the order should be [A, B]; got {rows:?}",
        );
    }

    #[test]
    fn bump_survives_reopen() {
        let (_guard, path) = temp_db_path();

        let a = app_ref("persistent.desktop");
        {
            let store = MruStore::open(&path).expect("first open should succeed");
            store.bump(&a).expect("bump should succeed");
            // store dropped here; the SQLite connection closes.
        }

        let reopened = MruStore::open(&path).expect("reopen should succeed");
        let rows = reopened.read_all().expect("read_all should succeed");
        assert_eq!(
            rows,
            vec![a.clone()],
            "rows written before drop must persist across reopen; got {rows:?}",
        );
    }

    #[test]
    fn concurrent_bumps_serialize_via_busy_timeout() {
        // Two MruStore handles against the same SQLite file simulate two
        // concurrent LoFi launcher processes. WAL + busy_timeout (set in
        // open()) must let both writes succeed without SQLITE_BUSY errors.
        const EXPECTED_ROW_COUNT: usize = 2;

        let (_guard, path) = temp_db_path();
        let store_one = MruStore::open(&path).expect("first open should succeed");
        let store_two = MruStore::open(&path).expect("second open should succeed");

        let a = app_ref("one.desktop");
        let b = app_ref("two.desktop");

        store_one
            .bump(&a)
            .expect("bump from store_one should succeed");
        thread::sleep(BUMP_GAP);
        store_two
            .bump(&b)
            .expect("bump from store_two should succeed");

        let rows = store_one.read_all().expect("read_all should succeed");
        assert_eq!(
            rows.len(),
            EXPECTED_ROW_COUNT,
            "both writes (from separate handles) should be visible; got rows {rows:?}",
        );
        assert_eq!(
            rows,
            vec![b.clone(), a.clone()],
            "with store_two bumping after store_one, B should be more recent than A; got {rows:?}",
        );
    }
}
