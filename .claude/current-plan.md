# macOS UI slice: search field + icon/name/category rows

## Context

The first macOS slice landed a static list — every `.app` under `/Applications` and `~/Applications`, one column of plain text. Time to make it feel like a launcher: a search field at the top that fuzzy-filters as the user types, and rows that look like `[icon] Name … [Category]` with the category dimmed and right-aligned.

The matcher already exists on the Rust side (`app/core/src/matcher.rs::search`) — filter-only, whitespace-tokenized, case-insensitive, intersection semantics. The icon machinery on macOS is `NSWorkspace.shared.icon(forFile:)`, which takes a `.app` bundle path and returns an `NSImage`. The category is `Entry::kind()` → `EntryKind` (Application / Window / Workspace / Command / PowerCommand); only Application surfaces in this slice but the wiring works for all.

## Decisions

- **Filtering lives in Rust.** `EntryList` grows a current-query field; `lofi_entries_set_query` recomputes a filtered index vector. `len` and `get_*` accessors then read through the filter. Empty query = identity passthrough (matches existing tests' expectations).
- **Icons travel as bundle paths through the existing `icon: Option<String>` field.** No new icon-bytes plumbing — Swift passes the `.app` URL's path as the `icon` arg to `pushApplication`; Rust stores it verbatim; `lofi_entries_get_icon` returns it; Swift resolves via `NSWorkspace`. This mirrors GNOME's "icon identifier, not bytes" rule.
- **Category is exposed as a stable English string** (`"Application"`, `"Window"`, …) via a new `lofi_entries_get_category`. Cheaper than threading the enum discriminant + a Swift-side translation table; localization can come later as a UI-side override.
- **Borrow contract extends to `set_query`.** Any `get_*` pointer is invalidated by the next mutating call — currently `push_*` and `free`, now also `set_query`. Document and test.

## Rust changes — `app/core/`

### `src/ffi/entries.rs`

`EntryList` gains:
- `query: String` (empty = no filter)
- `filter: Option<Vec<usize>>` — `Some(indices)` when a non-empty query is active, `None` for the passthrough case. Avoids reallocating an `(0..len).collect()` vector for the common no-filter case.

Helper `EntryList::resolve(idx)` returns the underlying `entries` slot for a "filtered" index, going through `filter` when present.

`push` clears `name_cache` (existing) **and** recomputes the filter so the new entry shows up if it matches the current query.

New functions:
- `lofi_entries_set_query(list, query) -> bool` — null query = empty (no filter). Stores the string, recomputes `filter` using the matcher. On success returns true; null `list` returns false.
- `lofi_entries_get_bundle_id(list, idx) -> *const c_char` — returns `Application::desktop_id` (or null for non-Application variants once they exist).
- `lofi_entries_get_category(list, idx) -> *const c_char` — returns one of the five stable strings.
- `lofi_entries_get_icon(list, idx) -> *const c_char` — returns `Application::icon` if `Some`, null if `None`.

Modified:
- `lofi_entries_len`, `lofi_entries_get_name` — read through the filter when present.

Each new accessor needs its own backing cache (`bundle_id_cache`, `category_cache`, `icon_cache`) to honor the borrow contract. All caches clear together on any mutation (push or set_query).

### `src/matcher.rs`

Factor out a tiny helper `pub(crate) fn matches(entry: &Entry, tokens: &[&str], matcher: &SkimMatcherV2) -> bool` so `set_query` in the FFI can run the same matching logic against a single entry without materializing a `Vec<&Entry>`.

### `src/ffi/mod.rs`

Re-export the new symbols.

### `cbindgen.toml`

No changes — cbindgen regenerates the header from the Rust source; Bazel's genrule picks up the new functions automatically.

## Rust tests — `app/core/tests/ffi.rs`

Add cases:
1. `set_query_filters_to_match` — push three apps with distinct names, set_query to a substring, len returns 1, get_name returns the matching app.
2. `set_query_empty_restores_all` — after filtering down, set_query("") restores full count and order.
3. `set_query_intersection_semantics` — whitespace-separated tokens both required.
4. `set_query_case_insensitive` — query matches regardless of case.
5. `set_query_invalidates_get_name_borrow` — call get_name, copy bytes, set_query, the copy is still valid (documents the contract via use).
6. `get_bundle_id_round_trips` — push with a known bundleId, get_bundle_id returns it.
7. `get_category_returns_application` — Application entries return "Application".
8. `get_icon_returns_pushed_value` — non-null pushed icon comes back; null pushed icon returns null.
9. `set_query_null_clears_filter` — symmetric with set_query of empty string.
10. `push_recomputes_filter` — push while a query is active, the new entry appears in len iff it matches.

## Swift changes — `app/macos/Sources/LoFi/`

### `AppDiscovery.swift`

`DiscoveredApp` gains `bundlePath: String` (the `.app` URL's `path`).

### `AppDelegate.swift`

Push call becomes:
```swift
entries.pushApplication(name: app.name, bundleId: app.bundleId, icon: app.bundlePath)
```

### `RustBridge.swift`

`EntryList` gains:
- `setQuery(_ query: String)` → `lofi_entries_set_query`
- `bundleId(at idx: Int) -> String?`
- `category(at idx: Int) -> String?`
- `icon(at idx: Int) -> String?`

Same copy-into-Swift-String pattern as `name(at:)`.

### `PanelController.swift`

`init(content:)` becomes `init(searchField:listView:)`. Layout:
- `NSStackView` vertical, holding `NSSearchField` on top and `NSScrollView` filling the rest.
- `panel.initialFirstResponder = searchField` so typing starts immediately.

### `AppListController.swift`

Becomes the `NSSearchFieldDelegate`. Owns the `NSSearchField`. New:
- `controlTextDidChange(_:)` calls `entries.setQuery(searchField.stringValue)` then `tableView.reloadData()`.
- `view` getter returns the composed stack (search field + scroll view); `AppDelegate` only sees one root view.

Cell rendering rewritten to show:
- `NSImageView` (24×24, icon resolved via `NSWorkspace.shared.icon(forFile: bundlePath)`)
- `NSTextField` (name, `.labelColor`)
- flexible spacer
- `NSTextField` (category, `.secondaryLabelColor` or `.tertiaryLabelColor`, smaller font, trailing-aligned)

Row height bumps from 28 to ~36.

## Critical files

**Modify (Rust):**
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/entries.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/mod.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/src/matcher.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/tests/ffi.rs`

**Modify (Swift):**
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppDiscovery.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppDelegate.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/RustBridge.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/PanelController.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppListController.swift`

**README updates:**
- `/Users/jplein/Git/jplein/lofi/app/macos/README.md` — out-of-scope list shrinks (search, icons); add a gotcha about NSSearchField first responder.
- `/Users/jplein/Git/jplein/lofi/app/core/README.md` — FFI surface list grows.

## Verification

1. `bazel test //app/core:ffi_test` — all old tests pass, ten new ones added pass.
2. `bazel build //app/macos:LoFi` — succeeds.
3. `bazel run //app/macos:launch` — panel opens with the search field focused; rows show app icons + names; "Application" appears right-aligned and dimmed.
4. Type into the search field: list filters as expected. Typing `"saf"` shows Safari; deleting back to empty restores the full list. Multi-token `"web safari"` still matches Safari.

## Risks / gotchas

1. **`NSSearchField`'s built-in cancel button** can fight panel theming. Settle for the system default look in this slice.
2. **First responder under a borderless `.nonactivatingPanel`** — `panel.initialFirstResponder = searchField` needs to be set before `makeKeyAndOrderFront`.
3. **The borrow contract grows.** `lofi_entries_set_query` invalidates all in-flight `get_*` pointers. The Swift wrapper copies into `String` immediately, so this is fine in practice — but it's worth a clear note in the FFI module doc.
4. **`NSWorkspace.icon(forFile:)` is synchronous** and may hit disk. For 50 apps on first paint it's fine; revisit if the list grows past hundreds.
5. **Filter recomputation on push** — currently academic (all pushes happen before any set_query) but the code path has to be right for the eventual async-discovery slice.

## Workflow status

- [x] Plan written
- [x] Test-writer pass 1 — 10 new FFI tests added to `tests/ffi.rs`
- [x] Coder pass 1 — Rust FFI (set_query + 3 accessors), Swift UI (search field, icon/name/category rows), `bazel build //app/macos:LoFi` green; `bazel test //app/core:ffi_test` 21/22 (one fixture false-positive)
- [x] Test-writer pass 2 — fixed `push_recomputes_filter` fixture (bundle IDs no longer contain `com.example`, removing the accidental `o-m-e` subsequence match)
- [x] Coder pass 2 — `bazel test //app/core:ffi_test` 22/22 ✅, `bazel build //app/macos:LoFi` ✅
- [x] Reviewer pass — approved with notes (all notes were README staleness, fixed in the next step)
- [x] Technical-writer pass — `app/macos/README.md` and `app/core/README.md` updated: status paragraph, gotchas (initial-first-responder, stack-width pin), FFI surface count (5 → 9), borrow contract extended to include `set_query`, test count (12 → 22)

Outstanding (minor, non-blocking; flagged by technical-writer):
- macOS README Layout block (`AppListController.swift` description) still says "NSTableView data source + delegate" — also now the `NSSearchFieldDelegate`.
- macOS README "Why Swift drives discovery and Rust holds the list" still lists only `lofi_entries_len` / `lofi_entries_get_name` for the readback path — also `get_bundle_id`, `get_category`, `get_icon` now.

These are documentation-symmetry items; the substance is captured elsewhere.
