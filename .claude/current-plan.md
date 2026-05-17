# Window entries in the launcher

## Summary

Open windows become entries in LoFi's launcher list. Extension adds `app_name` + `icon` to its Window dict (via `Shell.WindowTracker`). `lofi-core` gains `Window` struct + `Entry::Window` + `EntryRef::Window`. `lofi-gnome` gains `zbus` dep and a `windows` module with `gather_windows()` + `focus_window(id)`. `launch.rs` dispatches `Entry::Window` to `windows::focus_window`. `main.rs` combines apps + windows. 7 new core unit tests; no automated gnome-side tests (live D-Bus).

## Files

### Create
- `app/gnome/src/windows.rs` — zbus blocking proxy + `gather_windows()` + `focus_window(id)`.

### Modify
- `extension/gnome/src/windows.ts` — add `resolveAppInfo` + two new dict fields.
- `extension/gnome/README.md` — Window-dict table additions.
- `app/core/src/lib.rs` — `Window` struct + 2nd variants on `Entry`/`EntryKind`/`EntryRef` + extended accessors + 4 new unit tests + `make_window` helper.
- `app/core/src/matcher.rs` — `haystack` Window arm + 3 new unit tests + `win` helper.
- `app/core/README.md` — `Window` subsection + matcher/Entry updates.
- `app/gnome/Cargo.toml` — add `zbus = "5"` and `serde = { version = "1", features = ["derive"] }`.
- `app/gnome/src/lib.rs` — `pub mod windows;`.
- `app/gnome/src/launch.rs` — add `Entry::Window` arm calling `windows::focus_window(w.id)`.
- `app/gnome/src/main.rs` — combine apps + windows into one Vec<Entry>.
- `app/gnome/src/ui.rs` — extend `kind_to_str`.
- `app/gnome/README.md` — update zbus / windows / launch sections.
- `app/README.md` — touch `core/` and `gnome/` bullets.

## Key shapes

### `lofi-core` Window
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub title: String,
    pub app_name: Option<String>,
    pub icon: Option<String>,
    pub workspace: i32,
}
```

### Variant additions
- `Entry::Window(Window)`
- `EntryKind::Window`
- `EntryRef::Window(u64)` — JSON shape `{"type":"window","id":12345}` (snake_case from existing container attr).

### Matcher haystack arm
```rust
Entry::Window(w) => match &w.app_name {
    Some(app) => format!("{} {}", w.title, app),
    None => w.title.clone(),
}
```

### Window-dict additions in extension
- `app_name: GLib.Variant.new_string(info.name)`
- `icon: GLib.Variant.new_string(info.icon)`
Where `info` comes from `resolveAppInfo(win)` using `Shell.WindowTracker.get_default().get_window_app(win)?.{get_name(), get_icon() → IconExt::to_string()}`.

### Rust D-Bus surface
```rust
#[zbus::proxy(
    interface = "dev.jplein.LoFi.Shell.WindowManager",
    default_service = "dev.jplein.LoFi.Shell",
    default_path = "/dev/jplein/LoFi/Shell",
    gen_blocking = true,
    gen_async = false,
)]
trait WindowManager {
    fn list_windows(&self) -> zbus::Result<Vec<DbusWindow>>;
    fn focus_window(&self, id: u64) -> zbus::Result<()>;
}

#[derive(Debug, serde::Deserialize, zbus::zvariant::Type, zbus::zvariant::DeserializeDict)]
#[zvariant(signature = "a{sv}")]
struct DbusWindow {
    id: u64,
    title: String,
    app_name: String,
    icon: String,
    workspace: i32,
}
```

Coerce empty `app_name` / `icon` strings to `None` in `map_dbus_window`. On any `zbus::Error`, log via `eprintln!` and return `Vec::new()` or `()`. No `unwrap`/`expect` in library code.

## Tests (new)

In `app/core/src/lib.rs` `mod tests`:
1. `entry_window_reference_round_trips` — `Entry::Window(...)` → `EntryRef::Window(id)`; `resolve` round-trip. Include id=0.
2. `resolve_finds_window_by_reference` — mixed Apps + Windows; resolve a specific Window; assert `name() == title` and cross-variant non-matching.
3. `entry_ref_window_serializes_to_tagged_json` — exact equality with `r#"{"type":"window","id":12345}"#`; round-trip.
4. `entry_window_methods_return_window_data` — `.name`, `.icon` (Some + None), `.kind == EntryKind::Window`.

Add `fn make_window(id, title, app_name, icon) -> Window` helper.

In `app/core/src/matcher.rs` `mod tests`:
5. `matcher_finds_window_by_title` — query in title matches the window, not unrelated apps.
6. `matcher_finds_window_by_app_name` — query matches windows by their app_name only.
7. `matcher_window_with_no_app_name_matches_title_only` — `app_name: None`; title match works; unrelated query doesn't crash.

Add `fn win(id, title, app_name) -> Entry` helper. Update the `use crate::{...}` line to include `Window`.

## Implementation order

1. Extension TS first (`extension/gnome/src/windows.ts`) — extension rebuild + reinstall is the user's manual step.
2. `lofi-core` types (`app/core/src/lib.rs` Window + variants + accessors). Workspace fails to compile elsewhere — that's expected.
3. `lofi-core` matcher (`app/core/src/matcher.rs` haystack arm). `cargo build -p lofi-core` clean.
4. `lofi-core` tests (4 in lib.rs + 3 in matcher.rs + helpers). `cargo test -p lofi-core` 12 tests pass.
5. `app/gnome/Cargo.toml` — add `zbus = "5"`, `serde = { version = "1", features = ["derive"] }`.
6. Create `app/gnome/src/windows.rs`.
7. `app/gnome/src/lib.rs` — `pub mod windows;`.
8. `app/gnome/src/launch.rs` — `Entry::Window` arm.
9. `app/gnome/src/ui.rs` — `EntryKind::Window` arm in `kind_to_str`.
10. `app/gnome/src/main.rs` — combine apps + windows.
11. `cargo build --workspace` clean.
12. `cargo test --workspace` clean.
13. `cargo clippy --workspace --all-targets -- -D warnings` clean.
14. `cargo fmt --all -- --check` clean.
15. `nix build` clean; `nix build .#extension` clean.
16. READMEs.

## Verification

- `cargo test -p lofi-core` — 7 new + 5 existing matcher + 5 existing app tests = 17 total (depends on existing count; the key point is +7).
- `cargo test --workspace` — `tests/apps.rs` integration still passes.
- `cargo clippy` / `cargo fmt --check` clean.
- `nix build` / `nix build .#extension` clean.
- **Manual** (user, on Wayland w/ extension installed): launch LoFi, type a window title and verify Window entry; Enter → window raises + workspace switches; type an app name, both the app entry AND its windows appear.

## Lint / style

- No `unwrap`/`expect` in `windows.rs`. Use `match`/`if let`/`?` with `eprintln!` early returns.
- All `match` on `Entry`/`EntryKind` exhaustive (no `_` arm).
- TS strict mode unchanged; the `as Shell.App | null` widening matches the existing `appIdFor` pattern.
- If `clippy` complains about the `#[zbus::proxy]`-generated code (e.g., `missing_errors_doc`), scope a narrow `#[allow(...)]` above the macro invocation. Don't apply pre-emptively.

## Out of scope

Workspace entries; display entries; resize/move/maximize from launcher; window MRU; workspace context in row; app-icon fallback (already covered by WindowTracker); tile-snapping; filtering by kind; reusable D-Bus connection; async gather.
