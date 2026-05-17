# MRU cache — SQLite-backed activation history, sole sort key

## Context

The launcher currently shows entries in gather order: Applications in `.desktop`-file order, then Windows in their `ListWindowsMRU` order. The fuzzy matcher (`lofi-core::matcher::search`) sorts matches by fuzzy score during typing. Both ordering rules are unstable from the user's perspective — typing "Foo" then "Foob" then "Foobar" can shift the selected row mid-keystroke (the classic Raycast "Center two-thirds" → "Center" jump).

The fix: a persistent activation history. Whenever the user activates an entry, write its `EntryRef` to a SQLite store with the current time. At launch, read the store and **use that recency order as the sole sort key** for the launcher list. The fuzzy matcher becomes a filter — does the entry match? — with no influence on order.

Outside the MRU cache, order is undefined: an entry never activated falls below all MRU rows in whatever order the gather produced. This is the price of stable selection during typing.

## Confirmed decisions

1. **One row per `EntryRef::*`** — Applications and Windows today, plus future Workspace/Command/etc. The schema is generic over the tagged-enum.
2. **Recency only** — `last_used` timestamp (Unix epoch millis, `i64`). No pick count. No frecency math.
3. **MRU is the sole sort key** — fuzzy matcher filters, MRU orders. Non-MRU entries trail behind, undefined order.
4. **Window entries persist in MRU** — even though their u64 ids are session-ephemeral. Dead rows from prior sessions are dead weight, not bugs. Cleanup is **out of scope** for this change; a future pass will trim to N once we have a feel for steady-state size.
5. **Window activation does NOT bump the underlying Application** — the Window row itself bumps, the App row does not. (Picking the "Chrome — github.com" Window means Chrome-the-window is recent, not Chrome-the-app.)
6. **WAL + 5s busy_timeout** for concurrency. SQLite handles its own cross-process locking via OS file locks; no PID lockfile.
7. **File location**: `$XDG_STATE_HOME/lofi/mru.sqlite`, falling back to `$HOME/.local/state/lofi/mru.sqlite`. Parent dir is created on first write.
8. **Best-effort writes**: failures (corrupt DB, missing dir, disk full) log via `eprintln!` and continue. Launcher still works with an empty in-memory MRU index. No panics.

## Schema

```sql
CREATE TABLE IF NOT EXISTS mru (
    entry_ref TEXT NOT NULL PRIMARY KEY,
    last_used INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_mru_last_used ON mru(last_used DESC);
```

`entry_ref` is the existing JSON serialization of `EntryRef` (e.g. `{"type":"application","id":"firefox.desktop"}`, `{"type":"window","id":12345}`). PRIMARY KEY enforces dedup. Write is an UPSERT:

```sql
INSERT INTO mru (entry_ref, last_used) VALUES (?, ?)
  ON CONFLICT(entry_ref) DO UPDATE SET last_used = excluded.last_used;
```

Pragmas applied on every open:
```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
```

## File-by-file

### `app/core/Cargo.toml`

- Add `rusqlite = { version = "0.32", features = ["bundled"] }` — `bundled` means no system libsqlite dep, the crate compiles its own. This keeps `nix build` simple (no `pkgs.sqlite` add).
- Promote `serde_json` from `[dev-dependencies]` to `[dependencies]` (the MRU module needs it at runtime to serialize/deserialize `EntryRef`).

### `app/core/src/mru.rs` (new)

```rust
pub struct MruStore { conn: rusqlite::Connection }

pub enum MruError {
    Io(std::io::Error),
    Sql(rusqlite::Error),
    Json(serde_json::Error),
}

impl MruStore {
    /// Open or create the SQLite file at `path`. Creates parent dir.
    /// Applies pragmas and runs the idempotent migration.
    pub fn open(path: &std::path::Path) -> Result<Self, MruError>;

    /// Read all rows, most-recent first. Bad-JSON rows are skipped with
    /// an eprintln (the table isn't a place to assert serde invariants).
    pub fn read_all(&self) -> Result<Vec<EntryRef>, MruError>;

    /// UPSERT the row with `last_used = now()`. Best-effort; the caller
    /// logs and continues on Err.
    pub fn bump(&self, r: &EntryRef) -> Result<(), MruError>;
}
```

`From<rusqlite::Error>`, `From<serde_json::Error>`, `From<std::io::Error>` for `MruError`. `Display` impl for logging.

### `app/core/src/lib.rs`

- `pub mod mru;`
- Re-export `MruStore`, `MruError` from the crate root.

### `app/core/src/matcher.rs`

Change `pub fn search(entries: &[Entry], query: &str) -> Vec<&Entry>` to filter only — no score-based sort. Implementation: iterate `entries`, keep those where `SkimMatcherV2::fuzzy_match(haystack, query).is_some()`, return in input order.

The existing matcher unit tests (`matcher_finds_application_by_name`, `matcher_finds_window_by_title`, `matcher_finds_window_by_app_name`, `matcher_window_with_no_app_name_matches_title_only`, etc.) need their assertions updated from "result at index N is X" to "result set contains X" — since `search` no longer guarantees an order, only membership.

### `app/gnome/src/main.rs`

- Compute the state path: `$XDG_STATE_HOME` falling back to `$HOME/.local/state`, then `/lofi/mru.sqlite`. Mirror the manual XDG pattern in `apps::application_directories`.
- Open the MruStore once per launcher invocation. On error, log and proceed with `None` (UI handles `Option<MruStore>` and `Vec<EntryRef>::new()` as the empty index).
- Read `mru_index: Vec<EntryRef>` once.
- Pass both into `ui::build`.

### `app/gnome/src/ui.rs`

- `build(app, entries, mru_store, mru_index)` — new params.
- Build a `HashMap<EntryRef, usize>` from `mru_index` (key → rank, 0 = most recent) at construction time. Store on `UiState`.
- `populate_list` sort: stable_sort_by_key with `mru_position.get(entry.reference()?).copied().unwrap_or(usize::MAX)`. In-MRU items rise to the top in MRU order; non-MRU items fall to the bottom in input order.
- In both `connect_activate` (Enter) and `connect_row_activated` (click): before calling `launch::activate(&entry)`, call `mru_store.bump(&entry.reference())` if both are `Some`. Errors logged via `eprintln!`, not surfaced to the user.

### `app/gnome/src/launch.rs`

No changes. The bump happens in `ui.rs` so `launch::activate` stays platform-pure (just gio/D-Bus side effects).

## Read/write timing

**Read**: once per launcher process, immediately after `MruStore::open`. The result is a snapshot — if another LoFi process bumps the DB concurrently, this process's UI doesn't see the change until the next session. Fine; concurrent launches are rare.

**Write**: synchronously, immediately before invoking the side effect (gio launch / D-Bus focus). Single UPSERT, microseconds; invisible to the user. We don't fire-and-forget because the connection lives in the closure that's about to be dropped when the window closes — synchronous is simpler and safe.

## Tests

### `app/core/src/mru.rs` `mod tests`

All use `tempfile::tempdir()` for the SQLite path. No process-wide env vars touched.

1. `open_creates_db_and_parent_dir` — `open(temp/nested/dir/mru.sqlite)` succeeds; the file and intermediate dirs exist after.
2. `read_all_returns_empty_for_fresh_db` — `read_all` on a freshly-opened store returns `Ok(vec![])`.
3. `bump_then_read_round_trips_application_ref` — `bump(EntryRef::Application("firefox.desktop".into()))` then `read_all` returns that ref.
4. `bump_then_read_round_trips_window_ref` — same with `EntryRef::Window(12345)`. Verifies window ids persist with their u64 type intact.
5. `read_all_orders_by_recency_desc` — bump A, sleep 1ms, bump B, sleep 1ms, bump C; `read_all` returns `[C, B, A]`.
6. `bump_existing_ref_updates_timestamp_in_place` — bump A, bump B, bump A; `read_all` returns `[A, B]` (A was updated, not duplicated). Row count is 2.
7. `bump_survives_reopen` — bump A in one MruStore, drop it, open a new one against the same path, `read_all` still returns A. Persistence.
8. `concurrent_bumps_serialize_via_busy_timeout` — open two `MruStore`s against the same path. Bump from each; both succeed; final `read_all` shows both rows.

### `app/core/src/matcher.rs` `mod tests`

Update existing tests' assertions to use `iter().any(|e| e.name() == "...")` or set-based comparison instead of index-based positional asserts. No new tests.

### `app/core/tests/mru.rs` (new)

Integration tests that exercise `MruStore` through the public crate API, with a real SQLite file in a `tempfile::tempdir()`. These complement the in-module unit tests by validating the re-exports and end-to-end flows; they're the place a future contributor will look first to see what the MRU module guarantees.

1. `mru_round_trips_through_disk` — Open a store, bump three different `EntryRef`s (a mix of Application and Window variants) with ~1ms gaps, drop the store, reopen against the same path, `read_all` returns the three refs in most-recent-first order.
2. `mru_dedupes_repeated_bumps_on_same_ref` — Bump A, bump B, bump A again; reopen; `read_all` returns `[A, B]` (two rows, not three). Confirms the UPSERT and PRIMARY KEY do their job through the on-disk path.
3. `mru_skips_corrupt_rows_on_read` — Manually insert a row with garbage JSON via raw SQL on the same connection, then call `read_all`; the corrupt row is silently skipped (with an `eprintln!`) and the well-formed rows return as expected. Validates the "bad data on disk shouldn't crash the launcher" invariant.
4. `mru_two_stores_against_same_file_serialize_writes` — Open two `MruStore` handles against the same path (simulating two LoFi processes). Bump from each; both succeed; reopening once more shows both rows. Exercises the WAL + busy_timeout path.

### `app/gnome/tests/`

No new integration tests. The UI sort and launch-time bump are exercised manually (Wayland + extension required, like the rest of `lofi-gnome`).

## Implementation order

1. `app/core/Cargo.toml` — add `rusqlite` (bundled), promote `serde_json`.
2. `app/core/src/mru.rs` — implement `MruStore`, `MruError`. Add tests as we go.
3. `app/core/src/lib.rs` — re-export.
4. `cargo test -p lofi-core` clean (mru tests + existing).
5. `app/core/src/matcher.rs` — drop score-based ordering. Update tests.
6. `cargo test -p lofi-core` clean again.
7. `app/gnome/src/main.rs` — XDG_STATE_HOME path, open store, read index, thread into UI.
8. `app/gnome/src/ui.rs` — accept new params, MRU-sort `populate_list`, bump on activation.
9. `cargo build/test/clippy/fmt --workspace` clean.
10. `nix build` clean (verifies the bundled rusqlite compiles in the sandbox).
11. READMEs (`app/core/README.md` for the MRU module + schema rationale; `app/gnome/README.md` for the path resolution and sort behavior).

## Out of scope

- Cleanup (delete oldest N when row count > 2N). Will be a follow-up once we see real steady-state size.
- Per-entry-type filtering or surfaces (e.g. "show me only recent Applications").
- Cross-machine sync.
- Migration from a different on-disk format (there isn't one yet).
- Frecency scoring, pick counts, decay functions.
- Window→App MRU coupling (Window bumps Window only).
- Bumping on hover / partial-typing / preview. Only explicit activation (Enter or click).
- Surfacing MRU age in the UI (no "2 days ago" labels).
- A "clear history" UI action.

## Verification

- `cargo test -p lofi-core` — 8 new MRU tests + updated matcher tests + existing lib tests, all pass.
- `cargo test --workspace` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `nix build` — clean (this is the real test that bundled rusqlite compiles in the Nix sandbox).
- **Manual**: open LoFi, launch Firefox, close LoFi, reopen LoFi → Firefox is at the top. Type "fi" → Firefox is still the only thing selected even mid-typing. Launch a different app, reopen → that app is now top, Firefox is second.
