# macOS MRU slice: persistent activation history wins ordering

## Context

The matcher returns entries in input order — filter-only, no ranking. Today the input order is "alphabetical by name from `AppDiscovery.discover()`", so the user sees the same list every launch regardless of which app they actually use most. The `lofi-core` crate already implements an MRU store (`app/core/src/mru.rs`, public API `MruStore::open/read_all/bump`, SQLite WAL backing, 5s busy_timeout, bad-row-tolerant `read_all`). The GNOME side consumes it: read once at startup, build a `HashMap<EntryRef, usize>` rank map, stable-sort visible results by rank with `usize::MAX` falling to the bottom in input order. We mirror that for macOS, gated by a new chunk of FFI surface.

After this slice: an Enter or click bumps the activated entry's `EntryRef` in the SQLite file. On the next launch, that entry sorts to the top. The matcher's filter-only behavior stays unchanged — when a query narrows the list, the in-list MRU order is preserved because the underlying `entries: Vec<Entry>` has already been MRU-sorted in place.

## Decisions

- **Sort happens once, in the Rust `EntryList`**, after the Swift side finishes pushing. A new `lofi_entries_apply_mru(list, store)` call reorders the underlying vector in place, clears all caches (so any borrow-contract-protected pointers are correctly invalidated), and recomputes the filter. Subsequent `len`/`get_*` calls naturally read in MRU order, through whatever filter is active.
- **Bump on activation, before launching.** The Enter / click handler in `AppListController.launchRow(_:)` calls `mru.bump(list:, at: row)`, then `NSWorkspace.open(...)`, then `NSApp.terminate(nil)`. Bumping first is robust against the small window where `open` succeeds but `terminate` is interrupted; the worst case is a redundant write, never a missed one.
- **Storage path: `~/Library/Application Support/dev.jplein.lofi/mru.sqlite`** — macOS's bundle-id-namespaced conventional location, the macOS analog of GNOME's `$XDG_STATE_HOME/lofi/mru.sqlite`. The Rust side already creates missing parent directories on `MruStore::open`.
- **Failures degrade gracefully.** A store that fails to open returns null from `lofi_mru_open`; the Swift wrapper's `init` returns `nil` and the app proceeds without MRU ordering. `apply_mru` and `bump_entry` return `false` on any failure. Same shape as the existing FFI null-pointer pattern.
- **The Swift API hangs off `EntryList`**, not a separate "MRU manager" Swift class. `entries.applyMru(store: mru)` and `entries.bumpMru(store: mru, at: row)` keep the Swift-side surface tight and match how `setQuery` is structured.

## Rust changes — `app/core/`

### `src/ffi/mod.rs`

Add `pub mod mru;` and `pub use mru::*;` next to the existing entries re-export.

### `src/ffi/mru.rs` (new file)

Two new opaque-handle functions plus the bump variant:

- `lofi_mru_open(path: *const c_char) -> *mut MruStore`
  - Validate `path` is non-null and UTF-8.
  - Call `mru::MruStore::open(Path::new(path_str))`. Wrap the result in `Box::into_raw` on Ok; return `ptr::null_mut()` on Err (log via `eprintln!` for visibility during dev).
- `lofi_mru_free(store: *mut MruStore)`
  - Null-safe; `drop(Box::from_raw(store))` otherwise.
- `lofi_mru_bump_entry(store: *const MruStore, list: *const EntryList, idx: usize) -> bool`
  - Null checks on both pointers.
  - Resolve `idx` to the underlying entry via the filtered-index resolver in `EntryList` (the helper that maps filtered index → underlying index, added in the search slice).
  - Compute `entry.reference()`.
  - Call `store.bump(&reference)`. Return true on Ok, false on Err.

`MruStore` itself stays as-is in `src/mru.rs` — no changes needed.

### `src/ffi/entries.rs`

Add a new FFI function plus the supporting machinery to access internal state from the new `mru` module:

- `lofi_entries_apply_mru(list: *mut EntryList, store: *const MruStore) -> bool`
  - Null checks.
  - Call `store.read_all()` → `Vec<EntryRef>`. On Err, return false (do not panic; degraded mode is fine).
  - Build `HashMap<EntryRef, usize>` from the result, rank 0 = most recent.
  - Stable-sort `list.entries` by `(rank, original_position)` where `rank` is `usize::MAX` for entries not in the map (so they sort below known entries while preserving their relative push order).
  - Clear `name_cache`, `bundle_id_cache`, `category_cache`, `icon_cache` — pointers handed out before this call now reference moved entries.
  - Recompute the filter so the active query still works against the freshly reordered list.
  - Return true.

The `EntryList` type does not need new fields. Implementation lives in `entries.rs` because that's where `EntryList` and its caches/filter live; making fields/helpers `pub(super)` so `mru.rs` can read them is the smallest cross-module surface.

To support `lofi_mru_bump_entry`, expose a `pub(super) fn resolve_filtered_index(&self, idx: usize) -> Option<&Entry>` method on `EntryList`. Mirrors the existing private resolution but as a borrow returning the entry itself, ready for `reference()`.

### `cbindgen.toml`

No changes. The genrule auto-picks the new symbols.

## Rust tests — `app/core/tests/ffi.rs`

Add cases (gated by `#![cfg(feature = "ffi")]`, all using `extern "C"` only — no imports from `lofi_core`):

1. `mru_open_creates_file_and_can_be_freed` — pass a path under `tempfile::tempdir()`, assert non-null return, free. The file should exist on disk after.
2. `mru_open_invalid_path_returns_null` — pass an unwritable path (`/dev/null/cannot_create`); assert null return.
3. `mru_bump_then_apply_promotes_entry` — push three apps, open a fresh store, bump entry at idx 2, call `apply_mru`, assert `get_name(0)` returns the bumped entry's name.
4. `apply_mru_with_empty_store_preserves_input_order` — push three apps, open an empty store, apply_mru, len + get_name still match insertion order.
5. `mru_persists_across_open` — open store, bump idx 1, free; open same path again, apply_mru against the same pushed entries, assert idx 0 is the bumped one.
6. `apply_mru_invalidates_caches` — push, get_name(0) (warms cache), bump+apply_mru that reorders, copy the original bytes, then call get_name(0) which must return the **new** top entry's name (not the cached old one). Document the contract through the test.
7. `apply_mru_with_query_active_keeps_filter` — push three apps, set_query to match two of them, apply_mru, assert len still 2 (filter recomputed against new order).
8. `mru_bump_entry_null_args_return_false` — null store, null list, both null; each returns false without crashing.
9. `mru_apply_null_args_return_false` — same shape.
10. `mru_bump_out_of_bounds_returns_false` — bump idx 999 against a 3-entry list; false, no crash.

## Swift changes — `app/macos/Sources/LoFi/`

### `RustBridge.swift`

New class `MruStore`:

```swift
final class MruStore {
    fileprivate let handle: OpaquePointer
    init?(path: String) {
        guard let p = path.withCString({ lofi_mru_open($0) }) else { return nil }
        handle = OpaquePointer(p)
    }
    deinit { lofi_mru_free(handle) }
}
```

`EntryList` gains two methods:

```swift
@discardableResult
func applyMru(store: MruStore) -> Bool {
    lofi_entries_apply_mru(handle, store.handle)
}

@discardableResult
func bumpMru(store: MruStore, at idx: Int) -> Bool {
    lofi_mru_bump_entry(store.handle, handle, UInt(idx))
}
```

Match the OpaquePointer-passes-directly pattern from the first slice. `UInt(idx)` for `uintptr_t`.

### `AppDelegate.swift`

After the push loop and before constructing the panel:

```swift
let storePath = MruStore.defaultPath()
if let store = MruStore(path: storePath) {
    self.mruStore = store
    entries.applyMru(store: store)
}
```

`MruStore` storage held on the delegate. The list controller needs the store to bump on activation — passed in via initializer.

Add a small `defaultPath()` static helper on `MruStore`:

```swift
static func defaultPath() -> String {
    let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
    let dir = appSupport.appendingPathComponent("dev.jplein.lofi")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.appendingPathComponent("mru.sqlite").path
}
```

The Rust side also creates the directory, but doing it from Swift means the SQLite WAL files end up beside the main DB in a predictable place even if the Rust side decides to do something else later.

### `AppListController.swift`

`init` gains an optional `mruStore: MruStore?` parameter. Stored on the controller.

`launchRow(_ row: Int)` becomes:

```swift
private func launchRow(_ row: Int) {
    guard row >= 0, row < entries.count else { return }
    if let store = mruStore {
        entries.bumpMru(store: store, at: row)
    }
    guard let path = entries.icon(at: row) else { return }
    NSWorkspace.shared.open(URL(fileURLWithPath: path))
    NSApp.terminate(nil)
}
```

Bump *before* open: the bump is a fast local SQLite write that completes before terminate; the open call goes through LaunchServices and is non-blocking. If we ever lose the race we'd rather double-bump than miss-bump.

## Critical files

**Modify (Rust):**
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/mod.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/entries.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/tests/ffi.rs`

**Create (Rust):**
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/mru.rs`

**Modify (Swift):**
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/RustBridge.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppDelegate.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppListController.swift`

**README updates:**
- `/Users/jplein/Git/jplein/lofi/app/macos/README.md` — drop "MRU persistence" from the out-of-scope list; one-line note that activation history is now persistent.
- `/Users/jplein/Git/jplein/lofi/app/core/README.md` — FFI surface list grows from 9 to 12 (open, free, apply_mru, bump_entry).

## Verification

1. `bazel test //app/core:ffi_test` — old 22 cases still pass, 10 new MRU cases pass (total 32).
2. `bazel build //app/macos:LoFi` — succeeds.
3. `bazel run //app/macos:launch` — first run looks identical (alphabetical), pick something via Enter, app launches and quits. `bazel run //app/macos:launch` again — that something is at the top.
4. `ls -la ~/Library/Application\ Support/dev.jplein.lofi/` — `mru.sqlite` plus WAL files present.
5. `sqlite3 ~/Library/Application\ Support/dev.jplein.lofi/mru.sqlite "SELECT entry_ref, last_used FROM mru ORDER BY last_used DESC"` — manual spot-check shows the JSON-encoded `EntryRef`s with millisecond timestamps.

## Risks / gotchas

1. **The Swift `init?(path:)` failure path**: `withCString` returns the result of the closure, so the optional unwrap pattern works but reads slightly oddly. Comment it.
2. **Bump-vs-launch ordering**: as above, bump first. A failed `open` still bumps, which the user could perceive as a "ghost" — but the ghost is correctly attributed to "I tried to launch this." Acceptable.
3. **Caches must clear on `apply_mru`** — same borrow-contract growth as the search slice. Document in the FFI module header.
4. **SQLite WAL files (`mru.sqlite-wal`, `-shm`)** are normal next to the main DB; do not delete them or the next open will reset to a clean state.
5. **`Entry::reference()` exhaustiveness** — only Application entries exist on macOS today, but the match in `lofi_mru_bump_entry` should still go through `Entry::reference()` rather than special-casing, so future variants compile-error rather than silently no-op.
6. **Empty MRU file on first run** — `apply_mru` on an empty store leaves entry order unchanged (all ranks = `usize::MAX`, stable-sort by original position). No special-case needed.

## Workflow status

- [x] Plan written
- [x] Test-writer pass — 10 new MRU FFI tests appended to `tests/ffi.rs`
- [x] Coder pass — `ffi/mru.rs` created; `ffi/mod.rs` and `ffi/entries.rs` extended; Swift `MruStore` + `EntryList.applyMru/bumpMru` + AppDelegate/AppListController wiring all in place; `bazel test //app/core:ffi_test` 32/32 ✅; `bazel build //app/macos:LoFi` ✅
- [x] Reviewer pass — approved, all items PASS, only minors were README staleness (fixed in next step)
- [x] Technical-writer pass — `app/macos/README.md` and `app/core/README.md` updated (status, FFI surface count 9 → 13, borrow contract extended with `apply_mru`, test count 22 → 32)

Note: the plan said "9 → 12" but the actual count is 13 — `lofi_entries_apply_mru` lives in `entries.rs` (the plan grouped it visually with the three `mru.rs` symbols). The READMEs reflect the correct count.
