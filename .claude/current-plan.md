# Entry abstraction + EntryRef persistence handle

## Summary

Add a sum-type `Entry` enum to `lofi-core` that unifies launcher items behind a single runtime type. Add `EntryRef` — the serializable `{type, id}` handle — for future history/MRU storage. Add `EntryKind` discriminant and `resolve` lookup function. Normalize `Application::desktop_id` to canonical form (always `.desktop`-suffixed) so it can serve as a stable history key. Add `serde` (runtime) and `serde_json` (dev) to `lofi-core`.

## File-by-file changes

### 1. `app/core/Cargo.toml` — modify

```toml
[package]
name = "lofi-core"
version = "0.1.0"
edition = "2024"

[lib]
name = "lofi_core"
path = "src/lib.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
serde_json = "1"
```

Both already exist transitively in `Cargo.lock` at `1.0.228` / `1.0.149`; adding as direct deps pulls no new versions.

### 2. `app/core/src/lib.rs` — modify

Top-of-file: `use serde::{Deserialize, Serialize};`

**Types**:

- **`Application`** — unchanged. Derives `Debug, Clone, PartialEq, Eq`. NO serde derives. Three fields: `name`, `desktop_id`, `icon`.

- **`EntryKind`** — `pub enum`, derives `Debug, Clone, Copy, PartialEq, Eq, Hash`. One variant: `Application`. No serde derives.

- **`Entry`** — `pub enum`, derives `Debug, Clone, PartialEq, Eq`. One variant: `Application(Application)`. NO serde derives.

- **`EntryRef`** — `pub enum`, derives `Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize`. Container attr: `#[serde(tag = "type", content = "id", rename_all = "snake_case")]`. One variant: `Application(String)`.

**`impl Entry`** — four match-dispatched accessors:
- `pub fn name(&self) -> &str` → `app.name.as_str()`
- `pub fn icon(&self) -> Option<&str>` → `app.icon.as_deref()`
- `pub fn kind(&self) -> EntryKind` → `EntryKind::Application`
- `pub fn reference(&self) -> EntryRef` → `EntryRef::Application(app.desktop_id.clone())`

All four use exhaustive `match` (not `if let`).

**`resolve`** — free function:
```rust
pub fn resolve<'a>(entries: &'a [Entry], reference: &EntryRef) -> Option<&'a Entry> {
    entries.iter().find(|e| &e.reference() == reference)
}
```

**Test module** `#[cfg(test)] mod tests { use super::*; ... }` at the bottom, five tests:

1. **`entry_reference_round_trips_application`** — build `Application`, wrap in `Entry`, call `.reference()`, assert equality with `EntryRef::Application(desktop_id)`. Then call `resolve(&[entry.clone()], &entry.reference())` → `Some(&entry)`.

2. **`resolve_finds_application_by_reference`** — three distinct entries with ids `alpha.desktop`, `beta.desktop`, `gamma.desktop`. `EntryRef::Application("beta.desktop".into())` → returns the `Beta` entry. Confirms linear scan picks the right element, not just the first.

3. **`resolve_returns_none_for_missing_reference`** — same entries, ref `"missing.desktop"` → `None`. Also test empty-slice case.

4. **`entry_ref_serializes_to_tagged_json`** — `serde_json::to_string(&EntryRef::Application("firefox.desktop".into()))` → `r#"{"type":"application","id":"firefox.desktop"}"#`. Round-trip via `from_str`.

5. **`entry_methods_return_application_data`** — verify `.name()`, `.icon()`, `.kind()`. Include a second entry with `icon: None` to cover the `as_deref` branch.

### 3. `app/gnome/src/apps.rs` — modify

Single change: the desktop_id fallback branch (lines ~88-95). Current:
```rust
} else if let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) {
    stem
}
```

Replacement: extract the stem, then if it doesn't already end in `.desktop`, append it. Defensive check before appending. Express as a single expression bound to `desktop_id`. No helper extraction; no other code touched.

### 4. `app/gnome/tests/apps.rs` — modify

1. **Remove dead import**: `use std::collections::BTreeSet;` is no longer used. Keep `use std::path::{Path, PathBuf};` — `Path` is still used by `write_desktop`.

2. **Replace stems block** (lines ~82-106). New:
   ```rust
   let desktop_ids: Vec<String> = apps.iter().map(|a| a.desktop_id.clone()).collect();
   assert_eq!(
       desktop_ids,
       vec![
           "alpha.desktop".to_string(),
           "beta.desktop".to_string(),
           "gamma.desktop".to_string(),
       ],
       "desktop_ids should be canonical .desktop-suffixed names sorted; got {desktop_ids:?}"
   );
   ```

   `apps` is already sorted by `desktop_id` earlier (line ~66), so the vector is sorted by construction.

3. Count/names/icons assertions stay unchanged. Alphabetical order of canonical ids matches the existing sort.

### 5. `app/core/README.md` — modify

- Rewrite "Current contents" to describe `Application`, `Entry`, `EntryKind`, `EntryRef`, and `resolve`.
- Add a "Why two types for one concept" note explaining the `Entry` (runtime) vs `EntryRef` (persistence handle) split. Reasoning: display fields drift between sessions (locale, theme, app rename).
- State the canonical-`.desktop` invariant on `Application::desktop_id` — "always ends in `.desktop`; the platform gatherer normalizes."
- Note `Application` and `Entry` are deliberately NOT `Serialize`/`Deserialize`; only `EntryRef` is. Document so a contributor doesn't "helpfully" add derives.
- Mention the new `serde` dependency exists solely for `EntryRef`.

### 6. `app/gnome/README.md` — modify

In the `apps` module bullet, add a sentence: `gather_applications` guarantees `Application::desktop_id` is canonical (always ends in `.desktop`). Enforced by the integration test. The canonicalization matters because `desktop_id` is the payload of `EntryRef::Application` and therefore the stable history key.

### Unchanged

- `app/Cargo.toml`, `app/gnome/Cargo.toml`, `app/gnome/src/lib.rs`, `app/gnome/src/main.rs`, `flake.nix`, `rust-toolchain.toml`.
- The `lofi-gnome` re-exports in `lib.rs` continue to work; **do not** re-export the new `Entry`/`EntryRef`/`EntryKind`/`resolve` items yet — wait for the first consumer.

## Implementation order

1. `app/core/Cargo.toml` — add deps.
2. `app/core/src/lib.rs` — add new types, methods, `resolve`, unit tests.
3. `app/gnome/src/apps.rs` — canonicalize fallback.
4. `app/gnome/tests/apps.rs` — drop `BTreeSet` import; swap stems assertion for canonical-id assertion. Steps 3 and 4 must land together — step 3 alone breaks the existing stems assertion.
5. `app/core/README.md`.
6. `app/gnome/README.md`.

## Verification (from `/home/jplein/Git/jplein/lofi/app/`)

1. `cargo build --workspace`
2. `cargo test -p lofi-core` — five unit tests pass.
3. `cargo test -p lofi-gnome` — integration test passes with new canonical-id assertion.
4. `cargo test --workspace` — both pass together.
5. `cargo clippy --workspace --all-targets -- -D warnings`
6. `cargo fmt --all -- --check`

## Out of scope

- History persistence (file format, SQLite, write paths)
- MRU sorting logic
- UI list view consuming `&[Entry]`
- Launch wiring (`gio::DesktopAppInfo::new(...).launch()`)
- `Exec=` parsing or storage
- New `Entry`/`EntryRef` variants (Window, Workspace, Command)
- `HashMap<EntryRef, usize>` index — `Hash` is derived but unused
- Making `Application` / `Entry` serializable
- Re-exporting new types from `lofi-gnome`
- Cross-platform (macOS) work
