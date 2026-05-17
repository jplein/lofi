# app/core

The platform-agnostic shared crate (`lofi-core`). Defines the cross-platform data model that every platform implementation populates and the UI consumes.

## Why this crate exists

LoFi runs on both GNOME (Rust + GTK4) and macOS (Swift + AppKit on top of a Rust core exposed via a C ABI; experimental, see `app/macos/README.md`). The two platforms share nothing at the windowing-system level, but they do share the *shape* of the things a launcher cares about: applications, windows, workspaces, commands.

`core` holds those shared types and nothing else. Keeping it free of platform dependencies is the whole point:

- A macOS build of `core` must compile without pulling in `gtk`, `gio`, or any Linux-only crate. Otherwise the C-ABI surface for the Swift side either breaks or has to be `cfg`-forked.
- Conversely, a GNOME build must not need anything from `objc2` or the Apple frameworks.

If a type or function needs a platform crate to exist, it does not belong here. It belongs in `app/gnome/` or `app/macos/`.

## What belongs here

- Data types that describe items the launcher can show or act on, independent of how they were gathered.
- Pure logic that operates on those types (matching, ranking, configuration parsing) once it materializes.

## What does not belong here

- Gatherers. Parsing `.desktop` files, querying `org.gnome.Shell.Introspect`, walking `/Applications` — all of that is platform-specific and lives in the platform crate. `core` defines the destination type; platforms produce values of it.
- UI code.
- Anything that touches a window system, D-Bus, AppKit, or a specific filesystem layout.

## Current contents

### `Application`

A struct with four fields:

- `name` — the human-readable display string (e.g. `"Firefox"`).
- `desktop_id` — the stable identifier used to launch the app or refer to it across runs (e.g. `"firefox.desktop"`). **Invariant**: always ends in `.desktop`. The platform gatherer is responsible for normalizing this — see `app/gnome/src/apps.rs`.
- `icon` — `Option<String>` carrying an icon **identifier**, not bytes. Typically a freedesktop themed-icon name like `"firefox"`, or, less commonly, an absolute filesystem path when the `.desktop` file's `Icon=` line points at a literal file. `None` means the `.desktop` file had no usable `Icon=` line.
- `recent_window_id` — `Option<u64>`. **Runtime-only state**: not persisted, not part of `EntryRef`, and not produced by `apps::gather_applications`. The platform layer (`lofi-gnome::main`) sets this after gathering windows, populating it with the most-recently-focused window id for apps that have at least one open window. The UI uses it to render a running-indicator dot, and `launch::activate` uses it to focus the existing window instead of launching a fresh instance. `is_running` is just `recent_window_id.is_some()` — we don't store a separate boolean because the id is the only non-redundant piece of information at the point we'd check it.

The identifier-not-bytes choice is deliberate: rendering happens in the UI layer where the icon theme, scale factor, and target pixel size are all known. Resolving icons here would force eager I/O at gather time and lock in answers that go stale the moment the user switches themes or moves a window between monitors with different scales.

`recent_window_id` is deliberately not on `EntryRef`. The reference is the persistence handle for a launcher item, and a window id from the current shell session has no meaning after a shell restart (see `Window::id` below). Recency is recomputed from a fresh `gather_windows` on every launcher invocation, so there's nothing to persist.

### `Window`

A struct describing an open window. Six fields:

- `id` — `u64`, the Mutter window id. Session-stable but not persistent: window ids do not survive a shell restart, so they are appropriate as the payload of `EntryRef::Window` only for the lifetime of a session.
- `title` — `String`, the window title as reported by Mutter. May be empty.
- `app_name` — `Option<String>`, the human-readable name of the owning application (e.g. `"Firefox"`).
- `icon` — `Option<String>`, an icon **identifier** in the same shape as `Application::icon` (a freedesktop themed-icon name, or occasionally an absolute path). The identifier-not-bytes rationale from `Application` applies unchanged.
- `workspace` — `i32`, the workspace index. **`-1` means the window is sticky / on all workspaces**, matching the convention the extension uses on the wire.
- `app_desktop_id` — `Option<String>`, the canonical `.desktop`-suffixed id of the owning application as resolved by the platform's window tracker (in GNOME, `Shell.WindowTracker.get_window_app(win).get_id()`). The platform layer uses this to build the app-to-most-recent-window map that drives `Application::recent_window_id`; it is *not* used by the matcher (the haystack still keys off `app_name`/`title`). `None` when the tracker could not resolve a `Shell.App` for the window — the extension reports an empty string in that case and the Rust D-Bus client coerces it to `None`, like the other optional string fields.

`app_name`, `icon`, and `app_desktop_id` are all `Option` because the extension may emit empty strings when `Shell.WindowTracker` cannot resolve an owning app for the window (typically system surfaces and override-redirect windows). The `lofi-gnome` D-Bus client coerces those empty strings to `None` when it builds the `Window`, so consumers see only fully-populated values or `None` — never `Some("")`.

### `Workspace`

A struct describing a GNOME workspace surfaced by the Shell extension. Two fields:

- `index` — `i32`, the 0-based workspace index used by Mutter. **Session-stable** in the sense that it identifies the same workspace for the duration of a shell session, but **not durable**: the index of a given workspace can shift when the user adds or removes workspaces above it. This is the same trade-off `Window::id` has — `EntryRef::Workspace(index)` is the MRU key, and a stale row matching a different-but-same-index workspace is acceptable dead weight rather than a correctness problem. The user only ever sees MRU as ordering, never as identity.
- `name` — `String`, the human-readable workspace label. The extension currently hardcodes `"Workspace N"` (1-based), but a custom naming extension that overrides Mutter's workspace names would flow its label through here verbatim. This is also the entire matcher haystack for `Entry::Workspace` (see the matcher section below) — typing `"2"`, `"work"`, or `"workspace 2"` all match the default-labelled second workspace.

There is deliberately no `icon` field. Workspaces don't have per-instance icons — the extension doesn't emit one and there's nothing visual to vary on. `Entry::icon()` returns `Some("view-grid-symbolic")` for the `Workspace` arm as a hardcoded `&'static str` constant; threading an always-`Some` field through the gatherer would be pure ceremony.

The wire dict produced by the extension also carries `active` and `n_windows`, but those are dropped on decode (zvariant's dict decoder ignores keys not declared on the target struct). Adding either back later is a one-line change in the platform layer; we drop them today because nothing in `core` or the UI uses them yet.

### `Command`

A struct representing a launcher entry that runs a window action (center, half-width tile, minimize, toggle fullscreen, etc.). Four fields:

- `kind` — `CommandKind`, the discriminant. See the `CommandKind` subsection below for the variant list and accessor methods.
- `target_window_id` — `u64`, the Mutter id of the window the command acts on. Captured at **gather time** (the previously-focused user window — the first non-LoFi entry of `ListWindowsMRU`) so the command runs on the right window regardless of focus state at activation time. By the time the user presses Enter, LoFi itself is the focused window, so reading `display.focus_window` in the extension would target the launcher; capturing the id up front sidesteps the race.
- `work_area` — `WorkArea`, the work area of the monitor that owns the target window. Also captured at gather time via the extension's `GetWindowWorkArea(id)`. Storing it on the struct (rather than re-reading at activation) makes the activation path race-free and lets `compute_geometry` stay a pure function.
- `current_frame` — `(i32, i32, i32, i32)`, the target window's `(x, y, width, height)` frame at gather time. Captured via the extension's `GetWindowFrame(id)`. Only `CommandKind::Center` reads it (Center keeps the current size and recenters); the other geometry commands compute purely from the work area. It's still captured unconditionally because the platform layer doesn't branch on kind — one `gather_commands` call builds all nine `Command` entries with the same target / work area / frame.

`Command` is the only entry kind that has different runtime data per launcher invocation. `Window` has a stable id for the lifetime of a session; `Application` has a stable `desktop_id` across sessions; `Workspace` has a stable index for the session — but `Command` captures fresh target state every gather, because the "previously-focused user window" answer changes every time the launcher opens.

### `WorkArea`

A struct with four `i32` fields — `x`, `y`, `width`, `height` — describing the work area of a monitor (the monitor rectangle minus panel/dock struts). Used as the bounding box for every geometry command. The platform layer fills it from the extension's `GetWindowWorkArea(id)` and bakes it into every `Command` at gather time, so `compute_geometry` is a pure function over `(CommandKind, &WorkArea, current_frame)` with no D-Bus dependency.

### `CommandKind`

An enum naming the nine static window-action commands surfaced by the launcher:

- `Center` — keep size, recenter in work area.
- `CenterHalf` — width/2 × full height, centered.
- `CenterTwoThirds` — width*2/3 × full height, centered.
- `LeftHalf` — width/2 × full height, flush left.
- `RightHalf` — width/2 × full height, flush right.
- `StandardSize` — width*2/3 × height*2/3, centered.
- `Minimize`, `ToggleMaximize`, `ToggleFullscreen` — state-toggle commands; no geometry.

`CommandKind` is `Copy + Hash + Serialize + Deserialize` with `#[serde(rename_all = "snake_case")]`. Four accessor methods:

- `as_id(&self) -> &'static str` — stable snake_case identifier (`"center"`, `"center_half"`, ...). Used as the payload of `EntryRef::Command(String)` and therefore the persistent MRU key, so it must remain backwards-compatible across releases; adding a variant is fine, renaming an existing one would invalidate stored history.
- `display_name(&self) -> &'static str` — human-readable label (`"Center"`, `"Center half"`, ...). Shown in the UI **and** used as the matcher haystack — typing `"center"` matches both `Center` and `CenterHalf`, typing `"toggle"` matches both toggles.
- `icon_name(&self) -> &'static str` — Adwaita symbolic icon name picked to communicate either the geometry shape (`view-dual-symbolic` for the halves) or the action (`window-minimize-symbolic` for Minimize).
- `from_id(id: &str) -> Option<CommandKind>` — inverse of `as_id`. Used at MRU-rehydrate time to re-materialize stored `EntryRef::Command(id)` rows. Returns `None` for unknown ids so stale rows in MRU silently fall off rather than panic.

### `PowerCommand`

A struct representing a launcher entry for a system-level power action (Lock, Log Out, Suspend, Restart, Shutdown). One field:

- `kind` — `PowerCommandKind`, the discriminant. See `PowerCommandKind` below for the variant list and accessor methods.

The wrapping struct (rather than a bare `PowerCommandKind` on `Entry`) is deliberate. It parallels `Application`/`Window`/`Workspace`/`Command` so future per-instance state (a custom display name override, a feature flag, a config-driven enable bit) can be added without renaming the `Entry` variant or breaking the `EntryRef` shape. Today `kind` is the only field, but the wrapper costs nothing and removes a future migration.

Unlike `Command` (window actions), `PowerCommand` does **not** carry a target window id, work area, or captured frame. These are system-level actions that always apply, regardless of focus state or even whether any user window is open. The gatherer (`app/gnome/src/power.rs::gather_power_commands`) returns the same five entries unconditionally on every launcher invocation — no focused-window guard, no display dependency, no D-Bus call at gather time.

### `PowerCommandKind`

An enum naming the five power commands surfaced by the launcher, ordered to mirror GNOME's system menu (Lock → Log Out → Suspend → Restart → Shut Down):

- `LockSession` — lock the session via `org.gnome.ScreenSaver.Lock`. Display name `"Lock"`, icon `system-lock-screen-symbolic`.
- `Logout` — log out via `org.gnome.SessionManager.Logout(0)` (mode 0 = with confirmation, matching the system-menu UX). Display name `"Log Out"`, icon `system-log-out-symbolic`.
- `Suspend` — suspend the system via `org.freedesktop.login1.Manager.Suspend`. Display name `"Suspend"`, icon `weather-clear-night-symbolic`.
- `Restart` — restart via `org.gnome.SessionManager.Reboot` (so GNOME's standard 60-second confirmation dialog fires). Display name `"Restart"`, icon `system-reboot-symbolic`.
- `Shutdown` — shut down via `org.gnome.SessionManager.Shutdown` (same confirmation rationale as Restart). Display name `"Shutdown"`, icon `system-shutdown-symbolic`.

`PowerCommandKind` is `Copy + Hash + Serialize + Deserialize` with `#[serde(rename_all = "snake_case")]`. Four accessor methods, in the same shape as `CommandKind`:

- `as_id(&self) -> &'static str` — stable snake_case identifier (`"lock_session"`, `"logout"`, `"suspend"`, `"restart"`, `"shutdown"`). Used as the payload of `EntryRef::PowerCommand(String)` and therefore the persistent MRU key, so it must remain backwards-compatible across releases.
- `display_name(&self) -> &'static str` — short verb-like label matching the GNOME system menu (`"Lock"`, `"Log Out"`, `"Suspend"`, `"Restart"`, `"Shutdown"`). Shown in the UI **and** used as the matcher haystack — typing `"lock"` matches `LockSession`, typing `"log"` matches `Logout`, typing `"suspend"` matches `Suspend`.
- `icon_name(&self) -> &'static str` — Adwaita/freedesktop-symbolic icon name per the table above.
- `from_id(id: &str) -> Option<PowerCommandKind>` — inverse of `as_id`. Returns `None` for unknown ids so stale `EntryRef::PowerCommand` rows in MRU silently fall off rather than panic. Mirrors `CommandKind::from_id`.

### `Entry`, `EntryKind`, `EntryRef`, and `resolve`

`Entry` is the runtime sum type the UI consumes. Its variants are `Entry::Application(Application)`, `Entry::Window(Window)`, `Entry::Workspace(Workspace)`, `Entry::Command(Command)`, and `Entry::PowerCommand(PowerCommand)`.

`EntryKind` is the matching unit discriminant (`Copy`/`Hash`), useful for grouping or filtering without holding the payload.

`EntryRef` is the **persistence handle**: an enum-shaped `{type, id}` tagged with `#[serde(tag = "type", content = "id", rename_all = "snake_case")]`. Its five variants are `EntryRef::Application(String)` carrying a canonical `desktop_id`, `EntryRef::Window(u64)` carrying a Mutter window id, `EntryRef::Workspace(i32)` carrying a workspace index, `EntryRef::Command(String)` carrying a snake_case `CommandKind` id (`"center"`, `"center_half"`, etc. — exactly what `CommandKind::as_id()` returns), and `EntryRef::PowerCommand(String)` carrying a snake_case `PowerCommandKind` id (`"lock_session"`, `"logout"`, `"suspend"`, `"restart"`, `"shutdown"` — exactly what `PowerCommandKind::as_id()` returns; serialized JSON is `{"type":"power_command","id":"suspend"}`). The window id is session-scoped (see `Window::id` above), so a persisted `EntryRef::Window` only resolves within the same shell session that produced it; cross-session window history is out of scope here. The workspace index has the weaker session-stable-but-can-shift property described in the `Workspace` section above — same dead-weight tolerance applies. The Command and PowerCommand ids are durable across sessions because both `CommandKind` and `PowerCommandKind` are closed enums with stable snake_case mappings; the set of valid ids only grows. The Command and PowerCommand id spaces are **distinct EntryRef variants** — `EntryRef::Command("suspend")` and `EntryRef::PowerCommand("suspend")` are different rows that resolve to different entries (and the former is not even a valid `CommandKind` id today).

`resolve(&[Entry], &EntryRef) -> Option<&Entry>` is a linear scan that pairs `EntryRef`s back to the live `Entry`s from a gather.

`Entry` provides four match-dispatched accessors: `name()`, `icon()`, `kind()`, and `reference()`. They use exhaustive `match` (not `if let`) so that adding an `Entry` variant is a compile error until every accessor is updated.

### `matcher::search`

Signature:

```rust
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry>
```

Behavior:

- An empty or whitespace-only `query` is a passthrough: every entry is returned in input slice order. This makes the matcher safe to call unconditionally from the UI on every keystroke including the initial empty one.
- A non-empty query is split on whitespace into tokens. Each token must fuzzy-match the entry's haystack (intersection semantics).
- `search` is **filter-only**: matching entries are returned in input order. The matcher does not rank or score — once the MRU store exists (see `mru` below), ordering is the caller's job, and combining two ordering policies in this function would only obscure which one is winning. This is a deliberate split so the launcher can apply MRU (or any other order) without the fuzzy score fighting it. The classic Raycast-style "selection shifts mid-keystroke" is what filter-only + caller-sorted prevents: typing "Foo", "Foob", "Foobar" can change which rows are visible but not their order relative to each other.

The "haystack" — the text we match against — is built per-variant by an exhaustive `match` on `Entry` inside a private `haystack` function. For `Entry::Application` it is `"{name} {desktop_id}"`, so typing either the display name or the desktop id works. For `Entry::Window` it is `"{title} {app_name}"` when `app_name` is `Some`, and just `title` when it is `None`. The practical consequence is that typing an app name (e.g. `"firefox"`) matches both the Firefox application entry and every open Firefox window in the same gather. For `Entry::Workspace` the haystack is `name` alone — no second field worth concatenating, and the default `"Workspace N"` label already makes `"work"`, `"2"`, and `"workspace 2"` all match the right row; a custom workspace-naming extension flows its label through unchanged. For `Entry::Command` the haystack is `kind.display_name()` alone — the kind id (e.g. `"center_half"`) is a persistence detail, not a user-visible string, and matching on it would let typos in old MRU rows surface as ghost matches. For `Entry::PowerCommand` the haystack is also `kind.display_name()` alone, for the same reason — the snake_case ids (`"lock_session"`, etc.) are persistence-only, and matching on the display name (`"Lock"`, `"Suspend"`, ...) is the user-facing surface. Future `Entry` variants force this function to be updated (no `_` arm).

The fuzzy implementation is [`fuzzy-matcher`](https://docs.rs/fuzzy-matcher)'s `SkimMatcherV2` configured with `ignore_case()`. It's the same algorithm `skim` uses, which is in turn a port of fzf's scoring. `fuzzy-matcher` is one direct dependency of this crate, alongside `serde`, `serde_json`, and `rusqlite`.

### `commands::compute_geometry`

Signature:

```rust
pub fn compute_geometry(
    kind: CommandKind,
    work_area: &WorkArea,
    current_frame: (i32, i32, i32, i32),
) -> Option<(i32, i32, i32, i32)>
```

Pure geometry math for the window-action commands — no D-Bus, no GTK, no I/O. Re-exported at the crate root (`lofi_core::compute_geometry`).

- Returns `Some((x, y, w, h))` for the six geometry kinds (`Center`, `CenterHalf`, `CenterTwoThirds`, `LeftHalf`, `RightHalf`, `StandardSize`) — the rectangle the platform layer then feeds to the extension's `MoveResizeWindow`.
- Returns `None` for the three state-toggle kinds (`Minimize`, `ToggleMaximize`, `ToggleFullscreen`). The platform layer dispatches those to dedicated D-Bus methods (`MinimizeWindow`, `ToggleMaximizeWindow`, `ToggleFullscreenWindow`) instead of `MoveResizeWindow`, so there's no rectangle to compute.

`current_frame` is `(x, y, width, height)` of the target window at gather time. Only `Center` reads it (Center keeps the current size and recenters within the work area); the other kinds ignore the frame and compute from `work_area` alone. Pushing the frame into the signature — rather than having `Center` special-case a live D-Bus read — keeps this function pure and trivially unit-testable, which is the whole point of doing the math here instead of in the extension.

### `mru::MruStore`

SQLite-backed activation history. The store is the launcher's persistent record of which `EntryRef`s the user has activated, with a recency timestamp per ref. The launcher reads it once at startup, uses the result as the sole sort key for the displayed list, and writes back synchronously on every activation.

Public surface:

- `MruStore::open(path: &Path) -> Result<Self, MruError>` — open or create the SQLite file at `path`, create any missing parent directories, apply pragmas (WAL + 5s `busy_timeout`), and run the idempotent migration. Safe to call against a file written by a prior process.
- `MruStore::read_all(&self) -> Result<Vec<EntryRef>, MruError>` — return every row, most-recent-first.
- `MruStore::bump(&self, r: &EntryRef) -> Result<(), MruError>` — UPSERT the row with `last_used = now()` in Unix epoch milliseconds. Repeat bumps on the same ref update the timestamp in place rather than inserting a duplicate.

Schema (one table, applied on `open`):

```sql
CREATE TABLE IF NOT EXISTS mru (
    entry_ref TEXT NOT NULL PRIMARY KEY,
    last_used INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_mru_last_used ON mru(last_used DESC);
```

`entry_ref` is the JSON serialization of an `EntryRef` (e.g. `{"type":"application","id":"firefox.desktop"}` or `{"type":"window","id":12345}`). The PRIMARY KEY is what enforces dedup; the write is `INSERT ... ON CONFLICT(entry_ref) DO UPDATE SET last_used = excluded.last_used`. The descending index on `last_used` keeps `read_all`'s `ORDER BY last_used DESC` cheap as the table grows.

#### Why SQLite

- Cross-process safe via OS file locks — two LoFi launchers on the same machine writing to the same `mru.sqlite` serialize cleanly without a PID lockfile or any custom locking layer.
- WAL journal mode + a 5s `busy_timeout` applied on every `open` is enough for that serialization: concurrent writers wait out the brief contention rather than surfacing `SQLITE_BUSY` to the caller.
- `rusqlite`'s `bundled` feature builds SQLite from source inside the crate, so there is no system `libsqlite` dependency to declare. `nix build` stays simple: the Nix sandbox doesn't need a `pkgs.sqlite` add.

#### Why one table for all `EntryRef` variants

The schema is generic over the tagged-enum serialization of `EntryRef`. `EntryRef::Application(String)` and `EntryRef::Window(u64)` share the same row shape today; `EntryRef::Workspace`, `EntryRef::Command`, etc. plug in with no migration because the discriminant lives inside the JSON `type` field, not in the SQL column structure. Future entry kinds inherit the dedup, recency ordering, and write path automatically.

#### Bad-row tolerance

`read_all` skips rows whose `entry_ref` text does not parse as `EntryRef`, logs via `eprintln!`, and continues. A corrupt row — written by a future version, hand-edited, or otherwise out of shape — must not prevent the rest of the history from loading. The launcher's invariant is that stale or malformed history is never fatal: degraded mode is "we forget what you used recently", not "we crash".

#### Errors

`MruError` is an enum wrapping `io::Error`, `rusqlite::Error`, and `serde_json::Error` with `From` impls and a `Display` for logging. Nothing in this module panics; callers (the GNOME launcher, in particular) log and continue on `Err`. The store deliberately surfaces typed errors rather than silently swallowing them so the platform layer can decide the logging shape — `eprintln!` with file path context belongs at the call site, not inside `MruStore`.

### Why two types for one concept

Display fields drift between sessions: locale changes the display name, the user switches icon themes, an app gets renamed or its `.desktop` file moves. A history or MRU store that pickled the whole `Application` would either accumulate stale strings or have to re-key itself on every change.

`EntryRef` is the minimum information needed to point at "the same thing" across runs. Persistence layers serialize `EntryRef`. The UI receives `&[Entry]` from a fresh gather, and `resolve` rebuilds the link.

`Application` and `Entry` are **deliberately not** `Serialize`/`Deserialize`. Only `EntryRef` is. Do not "helpfully" add serde derives to the other types — that would invite callers to persist values that are guaranteed to go stale.

### Dependencies

- `serde` (with `derive`) — `EntryRef`'s tagged-enum representation.
- `serde_json` — runtime dependency now (not dev-only): the `mru` module serializes and deserializes `EntryRef` to/from the SQLite `entry_ref TEXT` column.
- `fuzzy-matcher` — `matcher::search` (Skim-style fuzzy scoring).
- `rusqlite` with the `bundled` feature — the `mru` module's SQLite connection. `bundled` ships SQLite as C sources inside the crate so we don't need a system `libsqlite` and `nix build` stays self-contained.
- `cbindgen` (build-dependency only, gated on `feature = "ffi"`) — generates the C header consumed by Swift.

## FFI surface (`feature = "ffi"`)

The macOS frontend (`app/macos/`) consumes `lofi-core` as a static library through a C ABI. The Rust-side surface lives under `src/ffi/`; the generated C header is `include/lofi_core.h` (gitignored — Rust is the source of truth, cbindgen the regenerator).

Why a hand-written C ABI rather than uniffi:

- The surface is tiny (five functions today, growing slowly with each slice). A uniffi binding would generate hundreds of lines of glue we'd then have to read every time something broke.
- We control both sides — Swift calls the C functions through a bridging header, no Kotlin / Python / etc. The marginal benefit of uniffi's multi-language support is zero here.
- The opaque-handle pattern (`EntryList`) is easier to reason about as plain Rust than as a uniffi `Object`.

### Crate types

`[lib] crate-type = ["staticlib", "rlib"]`. The `rlib` is what the GNOME crate (and the workspace's other consumers) link against. The `staticlib` is `liblofi_core.a`, which the macOS Xcode project links via `OTHER_LDFLAGS = -llofi_core`. Both are emitted unconditionally — adding a feature flag to gate the staticlib would only complicate the build pipeline; the unused output is cheap.

### Ownership model — Swift produces, Rust holds

Mirrors the GNOME pattern. Swift's `AppDiscovery` enumerates `.app` bundles and pushes each into a Rust-owned `EntryList` via `lofi_entries_push_application(...)`. After the push loop the list belongs to Rust; Swift only reads back through `lofi_entries_len` / `lofi_entries_get_name`. Future MRU and matcher work happens on the Rust side and surfaces back to Swift the same way.

`EntryList` is an opaque heap-allocated wrapper around `Vec<Entry>`. The Rust-side layout (the vector, the name cache for the borrow contract — see below) is intentionally not exposed in the header; cbindgen emits `typedef struct EntryList EntryList;` and nothing more.

### Borrow contract on `lofi_entries_get_name`

The function returns a `const char *` borrowed out of an internal CString cache. The pointer is valid until the next mutation of the list (any `push_*` call) or `lofi_entries_free`. Callers must copy the bytes into their own storage before doing anything that could invalidate the borrow. The Swift wrapper (`RustBridge.swift::EntryList.name(at:)`) copies into a Swift `String` immediately, so application code never sees the raw pointer.

### `desktop_id` policy on macOS (temporary)

On macOS we store the bundle identifier (e.g. `com.apple.Terminal`) verbatim in `Application::desktop_id`. The `.desktop`-suffix invariant from the GNOME platform layer does not apply — on macOS the field is just an opaque stable identifier used as the MRU key and the persistence handle. This is temporary in the sense that once cross-platform MRU lands we may want a more carefully namespaced key (e.g. `macos:com.apple.Terminal`), but for the first macOS slice the bundle id alone is sufficient because there's nothing else writing to the store.

### `rusqlite` bundled SQLite on macOS

`rusqlite`'s `bundled` feature is still on — the macOS Swift code must not also link `libsqlite3.tbd` from the macOS SDK or the link step fails with duplicate-symbol errors on `sqlite3_*`. If you ever need SQLite from Swift on macOS, do it through the Rust core, not directly.

### Build script (`build.rs`)

When `feature = "ffi"` is on, `build.rs` runs cbindgen and writes `include/lofi_core.h`. With the feature off it returns immediately so the GNOME build (and the default `cargo test -p lofi-core` invocation) doesn't depend on cbindgen at all.

### How `cargo test -p lofi-core --features ffi` links the FFI symbols

The integration test in `tests/ffi.rs` reaches each FFI function through an `extern "C"` declaration. With no Rust-side reference into `lofi_core::*`, rustc would otherwise drop the rlib from the linker's input list and the `lofi_entries_*` symbols would come out undefined. The test file pulls the rlib in explicitly with `extern crate lofi_core as _;` at the top, which is enough — no nested staticlib build, no `rustc-link-arg-tests` directive, no out-of-tree target directory. This is why the build script can stay as small as it is.
