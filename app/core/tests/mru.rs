//! Integration tests for the MRU store.
//!
//! These exercise `MruStore` through the public crate API only — they
//! complement the in-module unit tests in `src/mru.rs` and double as the
//! place a future contributor will look to see what the MRU module
//! guarantees end-to-end on a real SQLite file.

use lofi_core::{EntryRef, MruStore};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

/// 2ms minimum gap between bumps so the wall-clock-millisecond timestamps
/// land in distinct buckets and recency ordering is deterministic across
/// CI runners; 1ms has been observed to race.
const BUMP_GAP: Duration = Duration::from_millis(2);

fn app_ref(id: &str) -> EntryRef {
    EntryRef::Application(id.to_string())
}

fn win_ref(id: u64) -> EntryRef {
    EntryRef::Window(id)
}

/// Build an MRU SQLite path under a fresh tempdir. The intermediate dirs
/// are intentionally absent so `MruStore::open` exercises its parent-dir
/// creation logic.
fn temp_db_path() -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir().expect("tempdir should be creatable");
    let path = dir.path().join("state").join("lofi").join("mru.sqlite");
    (dir, path)
}

#[test]
fn mru_round_trips_through_disk() {
    let (_guard, path) = temp_db_path();

    let a = app_ref("alpha.desktop");
    let b = win_ref(42);
    let c = app_ref("gamma.desktop");

    {
        let store = MruStore::open(&path).expect("first open should succeed");
        store.bump(&a).expect("bump a should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&b).expect("bump b should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&c).expect("bump c should succeed");
        // store dropped here.
    }

    let reopened = MruStore::open(&path).expect("reopen should succeed");
    let rows = reopened.read_all().expect("read_all should succeed");
    assert_eq!(
        rows,
        vec![c.clone(), b.clone(), a.clone()],
        "after drop+reopen, rows should still appear in most-recent-first order; got {rows:?}",
    );
}

#[test]
fn mru_dedupes_repeated_bumps_on_same_ref() {
    // The schema's PRIMARY KEY on entry_ref + UPSERT means bumping A twice
    // produces one row whose last_used is the more recent of the two bumps.
    const EXPECTED_ROW_COUNT: usize = 2;

    let (_guard, path) = temp_db_path();

    let a = app_ref("a.desktop");
    let b = app_ref("b.desktop");

    {
        let store = MruStore::open(&path).expect("open should succeed");
        store.bump(&a).expect("bump a (1st) should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&b).expect("bump b should succeed");
        thread::sleep(BUMP_GAP);
        store.bump(&a).expect("bump a (2nd) should succeed");
    }

    let reopened = MruStore::open(&path).expect("reopen should succeed");
    let rows = reopened.read_all().expect("read_all should succeed");
    assert_eq!(
        rows.len(),
        EXPECTED_ROW_COUNT,
        "repeated bumps on the same ref must not duplicate rows; got {rows:?}",
    );
    assert_eq!(
        rows,
        vec![a.clone(), b.clone()],
        "after bump(A), bump(B), bump(A) the order should be [A, B]; got {rows:?}",
    );
}

#[test]
fn mru_skips_corrupt_rows_on_read() {
    // We inject a malformed row directly via rusqlite — the public API would
    // (correctly) refuse to write a non-JSON entry_ref. Then we verify that
    // read_all silently drops the bad row and returns only the well-formed
    // refs, preserving the "bad data on disk shouldn't crash the launcher"
    // invariant called out in the plan.
    let (_guard, path) = temp_db_path();

    let good = app_ref("good.desktop");

    let store = MruStore::open(&path).expect("open should succeed");
    store.bump(&good).expect("bump should succeed");

    // Inject a garbage row using a separate sqlite connection against the
    // same file. The MruStore handle stays open in parallel; WAL allows the
    // concurrent write.
    let raw = rusqlite::Connection::open(&path).expect("raw connection should open");
    raw.execute(
        "INSERT INTO mru (entry_ref, last_used) VALUES (?1, ?2)",
        rusqlite::params!["not-json", 0_i64],
    )
    .expect("garbage insert should succeed at the SQL level");
    drop(raw);

    let rows = store.read_all().expect("read_all should succeed");
    assert_eq!(
        rows,
        vec![good.clone()],
        "read_all should silently skip the corrupt row and return only the well-formed ref; got {rows:?}",
    );
}

#[test]
fn mru_two_stores_against_same_file_serialize_writes() {
    // Two MruStore handles simulate two LoFi launcher processes writing
    // concurrently. WAL + the busy_timeout pragma should let both writes
    // succeed; a fresh handle opened afterwards must see both rows.
    const EXPECTED_ROW_COUNT: usize = 2;

    let (_guard, path) = temp_db_path();

    let a = app_ref("one.desktop");
    let b = app_ref("two.desktop");

    {
        let store_one = MruStore::open(&path).expect("first open should succeed");
        let store_two = MruStore::open(&path).expect("second open should succeed");

        store_one
            .bump(&a)
            .expect("bump from store_one should succeed");
        thread::sleep(BUMP_GAP);
        store_two
            .bump(&b)
            .expect("bump from store_two should succeed");
    }

    let observer = MruStore::open(&path).expect("third open should succeed");
    let rows = observer.read_all().expect("read_all should succeed");
    assert_eq!(
        rows.len(),
        EXPECTED_ROW_COUNT,
        "both writes from separate handles should be visible; got rows {rows:?}",
    );
    assert_eq!(
        rows,
        vec![b.clone(), a.clone()],
        "with store_two bumping after store_one, B should appear more recent than A; got {rows:?}",
    );
}
