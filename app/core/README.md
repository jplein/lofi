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

The macOS frontend (`app/macos/`) consumes `lofi-core` as a static library through a C ABI. The Rust-side surface lives under `src/ffi/`; the generated C header is `include/lofi_core.h` (gitignored — Rust is the source of truth, cbindgen the regenerator). Bazel's `//app/core:lofi_core_cc` target exposes the header to Swift as the `LoFiCore` Clang module; Swift `import LoFiCore` rather than going through an Xcode-style bridging header.

Why a hand-written C ABI rather than uniffi:

- The surface is tiny (eighteen functions today, growing slowly with each slice). A uniffi binding would generate hundreds of lines of glue we'd then have to read every time something broke.
- We control both sides — Swift calls the C functions directly, no Kotlin / Python / etc. The marginal benefit of uniffi's multi-language support is zero here.
- The opaque-handle pattern (`EntryList`) is easier to reason about as plain Rust than as a uniffi `Object`.

### Crate types

`[lib] crate-type = ["staticlib", "rlib"]`. The `rlib` is what the GNOME crate (and the workspace's other consumers) link against. The `staticlib` is `liblofi_core.a`, which Bazel's `cc_library` wraps and `swift_library` links into the macOS app. Both are emitted unconditionally — adding a feature flag to gate the staticlib would only complicate the build pipeline; the unused output is cheap.

### Ownership model — Swift produces, Rust holds

Mirrors the GNOME pattern. Swift's `AppDiscovery` enumerates `.app` bundles and pushes each into a Rust-owned `EntryList` via `lofi_entries_push_application(...)`. After the push loop the list belongs to Rust; Swift's read path uses the eight accessors `lofi_entries_len`, `lofi_entries_get_name`, `lofi_entries_get_bundle_id`, `lofi_entries_get_category`, `lofi_entries_get_icon`, `lofi_entries_get_window_id`, `lofi_entries_get_command_id`, and `lofi_entries_get_command_geometry` (wrapped Swift-side as `count`, `name(at:)`, `bundleId(at:)`, `category(at:)`, `icon(at:)`, `windowId(at:)`, `commandId(at:)`, `commandGeometry(at:)`). The Swift mutation path is five more calls: `lofi_entries_push_window` (wrapped as `pushWindow`) for the macOS window-enumeration slice, `lofi_entries_push_command` (wrapped as `pushCommand`) for the window-action commands slice, `lofi_entries_set_query` (wrapped as `setQuery`) on each keystroke, `lofi_entries_apply_mru` (wrapped as `applyMru`) once at startup after the push loop, and `lofi_mru_bump_entry` (wrapped as `bumpMru`) on every Enter/click. The MRU store itself is held opaque on the Swift side via `lofi_mru_open` / `lofi_mru_free`.

Window entries follow the same Swift-produces / Rust-holds shape as Application: Swift's `WindowDiscovery` enumerates open windows via `CGWindowListCopyWindowInfo`, then pushes each one into the Rust list. The `Window::id` (a `CGWindowID` on macOS) is the cross-platform stable identifier — analogous to `Application::desktop_id` — and is the field Swift reads back via `lofi_entries_get_window_id` to drive activation. Because the Rust `Window` struct deliberately doesn't carry platform-specific fields like the owning process's PID (see `Window` above for the rationale), Swift maintains a side-table `[CGWindowID: (pid_t, title)]` populated at push time. The Rust core stays platform-clean; Swift looks up the PID it already knew at gather time when it's ready to call `AXUIElementPerformAction`.

The eighteen functions in the current surface: `lofi_entries_new`, `lofi_entries_free`, `lofi_entries_push_application`, `lofi_entries_push_window`, `lofi_entries_push_command`, `lofi_entries_len`, `lofi_entries_get_name`, `lofi_entries_set_query`, `lofi_entries_get_bundle_id`, `lofi_entries_get_category`, `lofi_entries_get_icon`, `lofi_entries_get_window_id`, `lofi_entries_get_command_id`, `lofi_entries_get_command_geometry`, `lofi_entries_apply_mru`, `lofi_mru_open`, `lofi_mru_free`, `lofi_mru_bump_entry`.

The four accessors added in the search-field slice:

- `lofi_entries_get_bundle_id(list, idx)` — returns the underlying `Application::desktop_id` (on macOS, the bundle identifier, e.g. `com.apple.Terminal`). Null for non-Application variants once those exist on macOS.
- `lofi_entries_get_category(list, idx)` — returns one of five stable English strings (`"Application"`, `"Window"`, `"Workspace"`, `"Command"`, `"PowerCommand"`). Chosen over exposing the `EntryKind` discriminant because a stable string is cheaper across the FFI boundary than threading an enum value plus a Swift-side translation table; localization, if needed, can come later as a UI override.
- `lofi_entries_get_icon(list, idx)` — returns the icon payload (`Option<String>`) for the entry at `idx` as a `const char *` or null. Both `Entry::Application` and `Entry::Window` carry an icon identifier; the function reads `app.icon` or `w.icon` accordingly. On macOS the identifier is the `.app` bundle path (for Windows, the *owning* app's bundle path resolved at discovery time from the window's PID) that Swift then resolves via `NSWorkspace.shared.icon(forFile:)` — same icon-identifier-not-bytes rule as GNOME's themed-icon names. Workspace, Command, and PowerCommand variants have no icon today and return null. (Regression note: an earlier pass only matched `Entry::Application` and silently dropped Window icons; the round-trip is now covered by `push_window_round_trips` in `tests/ffi.rs`.)

The four symbols added in the MRU persistence slice:

- `lofi_entries_apply_mru(list, store)` — reorders the underlying `Vec<Entry>` by recency from the MRU store, clears every `CString` cache, and recomputes the active filter against the freshly reordered list. Returns true on success, false on null arguments or a `read_all` failure (degraded mode: leave order untouched). Called once after the push loop and before showing the panel; the matcher's filter-only semantics then preserve the MRU order through any subsequent `set_query`.
- `lofi_mru_open(path)` — opens or creates the SQLite-backed `MruStore` at `path` (parents auto-created on the Rust side, WAL + 5s busy_timeout applied on open, migration idempotent). Returns an opaque `*mut MruStore` on success or null on any I/O / SQLite failure. Same null-pointer degraded-mode contract as the other openers — a Swift caller whose `init?` returns nil simply runs without MRU.
- `lofi_mru_free(store)` — null-safe deallocator for the handle from `lofi_mru_open`.
- `lofi_mru_bump_entry(store, list, idx)` — records the activation of the filtered-row entry under the active filter. Resolves the filtered index to the underlying `Entry`, computes its `Entry::reference()`, and writes through `MruStore::bump`. Returns true on success, false on any null pointer, out-of-bounds index, or SQLite error. Called *before* `NSWorkspace.open` on Enter / click so a fast local SQLite write completes ahead of the non-blocking LaunchServices call — double-bumping on a failed launch is preferable to missing a successful one.

The two symbols added in the macOS windows slice:

- `lofi_entries_push_window(list, id, title, app_name, icon, workspace, app_desktop_id) -> bool` — push an open window into the entry list. `title` is required; `app_name`, `icon`, and `app_desktop_id` are nullable (genuinely optional on the wire and mapped to `None` Rust-side when null). Returns `false` on null list, null `title`, or invalid UTF-8 in any provided string. Null-validation and UTF-8 validation mirror `lofi_entries_push_application` byte-for-byte; the only structural difference is the variant the call constructs (`Entry::Window` vs `Entry::Application`). Cache-clear-and-refilter semantics are identical — the underlying `EntryList::push` is shared between both push paths.
- `lofi_entries_get_window_id(list, idx) -> u64` — return the `CGWindowID` for a Window entry at the filtered index. Returns `0` for non-Window variants, null list, or out-of-bounds index. The 0-sentinel works because real `CGWindowID`s on macOS are always strictly greater than 0 for application windows, so we don't lose any representable id by reserving 0 as "not a window / not addressable". Swift callers are expected to gate on `category(at:) == "Window"` before reading the id, which makes the sentinel a robustness fallback rather than the primary discrimination path. This is the only `get_*` accessor that doesn't return a `const char *` — `u64` round-trips through the FFI by value, so there's no `CString` cache and no borrow contract to honor.

The three symbols added in the macOS commands slice (the nine GNOME-parity window-action commands on the Mac side):

- `lofi_entries_push_command(list, kind_id, target_window_id, wa_x, wa_y, wa_w, wa_h, frame_x, frame_y, frame_w, frame_h) -> bool` — push an `Entry::Command`, mirroring the `Command` struct field-for-field: `kind_id` selects the `CommandKind` (validated via `CommandKind::from_id`), `target_window_id` is the window the command acts on at activation, the `wa_*` quadruple is the target window's monitor work area, and the `frame_*` quadruple is the target window's current frame at gather time. Returns `false` on a null list, a null `kind_id`, invalid UTF-8, or an **unknown id** (`from_id` returns `None`) — in the unknown-id case nothing is pushed, so the list length is unchanged, surfacing a Swift typo or a stale id as a `false` rather than a silently-broken row. The eight geometry integers are stored verbatim in whatever coordinate space the caller hands them in (on macOS: top-left global; see `app/macos/README.md`) — the FFI does no flipping. Same cache-clear-and-refilter (`EntryList::push`) as `push_application` / `push_window`.
- `lofi_entries_get_command_id(list, idx) -> *const c_char` — return the `CommandKind::as_id()` snake_case string (`"center"`, `"center_half"`, …) for a Command at the filtered index, null for every other variant (or null list / out-of-bounds index). Unlike every other string accessor the returned pointer is a **process-lifetime `&'static CStr`** — one per kind, built from `c"..."` literals selected by an internal `command_id_cstr` helper — so it is **never invalidated by a mutation**. There is no cache slot or `RefCell` behind it. The bytes are byte-for-byte equal to `CommandKind::as_id` (`app/core/src/lib.rs`), guarded against drift by the `command_id_matches_as_id_for_all_kinds` FFI test; the match is exhaustive (no `_` arm) so adding a `CommandKind` is a compile error until both maps are extended. Swift still copies the pointer into a `String` for uniformity with the other accessors.
- `lofi_entries_get_command_geometry(list, idx, out_x, out_y, out_w, out_h) -> bool` — for a Command at the filtered index, call `compute_geometry(kind, &work_area, current_frame)` (the single source of geometry truth shared with GNOME, so Swift never duplicates the half / two-thirds math), write the four out-params, and return `true`. This returns `true` only for the **six geometry kinds** (`Center`, `CenterHalf`, `CenterTwoThirds`, `LeftHalf`, `RightHalf`, `StandardSize`). It returns `false` and leaves **all four out-params untouched** (a documented contract — the null out-pointers are guarded first, so if we cannot write all four we write none) for the **three state-toggle kinds** (`Minimize`, `ToggleMaximize`, `ToggleFullscreen`, where `compute_geometry` returns `None`), for non-Command entries, for an out-of-bounds index, for a null list, or when any out-pointer is null. Swift dispatches the state-toggle kinds by `lofi_entries_get_command_id` instead. Like `get_window_id`, the result crosses the FFI by value (through caller-owned out-params), so this accessor is exempt from the borrow contract.

`lofi_entries_set_query(list, query)` recomputes the filter. A null `query` clears the filter (identity passthrough); a non-null UTF-8 `query` is whitespace-tokenized and intersected against each entry's per-variant haystack, exactly matching `matcher::search`'s semantics. After `set_query` returns, `lofi_entries_len` and every `get_*` accessor read through the filter — Swift's table view sees a contiguous, post-filter list and doesn't need to know which underlying indices survived.

`EntryList` is an opaque heap-allocated wrapper around `Vec<Entry>`. The Rust-side layout (the vector, the per-accessor `CString` caches for the borrow contract, the current query string, the optional filter index vector — see below) is intentionally not exposed in the header; cbindgen emits `typedef struct EntryList EntryList;` and nothing more.

### Borrow contract on the `get_*` accessors

Every string-returning `lofi_entries_get_*` function (`_name`, `_bundle_id`, `_category`, `_icon`) returns a `const char *` borrowed out of a per-accessor `CString` cache held inside `EntryList`. The pointer is valid until the next mutating call on the list — any `push_*`, `lofi_entries_set_query`, `lofi_entries_apply_mru`, or `lofi_entries_free`. Callers must copy the bytes into their own storage before doing anything that could invalidate the borrow. The Swift wrapper (`RustBridge.swift::EntryList.name(at:)` and the parallel `bundleId` / `category` / `icon` accessors) copies into a Swift `String` immediately, so application code never sees the raw pointer.

Three accessors stand outside that cache-backed contract:

- `lofi_entries_get_window_id` returns a `u64` by value, with no cache and no borrow.
- `lofi_entries_get_command_geometry` writes its result into caller-owned out-params by value, so likewise there is no pointer into the list to invalidate.
- `lofi_entries_get_command_id` *does* return a `const char *`, but it points at a process-lifetime `&'static CStr` (a `c"..."` literal, not a cache slot), so a mutation never invalidates it — it is effectively exempt. Swift still copies it for uniformity with the genuinely cache-backed accessors.

`set_query` is on the invalidation list — not on the `push_*` list — for a specific reason: the cached `CString`s key off filtered indices, and recomputing the filter can change which underlying entry sits at a given index (or remove an index entirely). A pointer handed out before `set_query` may, after the call, refer to a slot whose `CString` has been dropped because the entry is no longer reachable through the filter. Rather than try to detect which subset of cached pointers survives a query change, every cache clears together on any mutation. The Swift side already copies eagerly, so this conservative invalidation costs nothing in practice and keeps the contract trivially statable: "no mutating call between a `get_*` and the read of its bytes."

`apply_mru` joins the invalidation list for a structurally identical reason: the caches key off positions in the underlying `entries: Vec<Entry>`, and an MRU reorder moves entries between positions. A pointer handed out before `apply_mru` would, after the reorder, refer to a slot whose `CString` was built from a *different* `Entry` — not stale text but actively wrong text, pointing at the previous occupant of that index. The fact that the caches survive across `get_*` calls is what makes them caches; the fact that any structural change (push, filter, reorder) drops them is what keeps them sound. Clearing all four caches on `apply_mru` is the same blanket policy as `set_query` and for the same reason: cheaper and more obviously correct than a per-index validity-tracking scheme.

### `desktop_id` policy on macOS (temporary)

On macOS we store the bundle identifier (e.g. `com.apple.Terminal`) verbatim in `Application::desktop_id`. The `.desktop`-suffix invariant from the GNOME platform layer does not apply — on macOS the field is just an opaque stable identifier used as the MRU key and the persistence handle. This is temporary in the sense that once cross-platform MRU lands we may want a more carefully namespaced key (e.g. `macos:com.apple.Terminal`), but for the first macOS slice the bundle id alone is sufficient because there's nothing else writing to the store.

### `rusqlite` bundled SQLite on macOS

`rusqlite`'s `bundled` feature is still on — the macOS Swift code must not also link `libsqlite3.tbd` from the macOS SDK or the link step fails with duplicate-symbol errors on `sqlite3_*`. If you ever need SQLite from Swift on macOS, do it through the Rust core, not directly.

### Header generation paths

Two ways the header gets produced, depending on the driving build system:

- **Bazel** (the macOS path): a `genrule` in `app/core/BUILD.bazel` runs the cbindgen binary (built from the same `Cargo.lock` via `crate_universe`) and writes the header into Bazel's output tree. `build.rs` is *not* invoked.
- **Cargo** (the `cargo build -p lofi-core --features ffi` path, useful for non-Bazel environments): `build.rs` runs cbindgen at compile time and writes `include/lofi_core.h` into the source tree.

The two paths invoke cbindgen differently to handle the `feature = "ffi"` gate on `pub mod ffi`:

- The Bazel `genrule` passes a single source file (`src/lib.rs`) to the cbindgen CLI, which selects cbindgen's file-mode parser. File-mode walks `mod` declarations on disk without ever calling `cargo metadata`, so the action is hermetic (no `~/.cargo/registry`, no network). Cbindgen still records the `#[cfg(feature = "ffi")]` on items it discovers — but with no `[defines]` mapping in `cbindgen.toml`, `to_condition` returns `None` and the items are emitted unconditionally. That matches reality: Bazel's `rust_static_library` for `lofi_core` already pins `crate_features = ["ffi"]`, so the actual `.a` always exports these symbols, and an unconditional C declaration matches the linker surface.
- The Cargo path calls `cbindgen::Builder::with_crate(&crate_dir)` from `build.rs`, which internally shells out to `cargo metadata --all-features`. That gives cbindgen full feature info and the items emit unconditionally for the same reason. The `ffi` feature toggle only gates whether `build.rs` runs cbindgen at all (so the GNOME `cargo build` path is a pure no-op).

The cbindgen file-mode path emits a `Missing [defines] entry for "feature = ffi"` warning per discovered item. This is expected; see the comment block above the `lofi_core_header` genrule in `BUILD.bazel`.

### How the FFI integration tests link the symbols

The integration test in `tests/ffi.rs` reaches each FFI function through an `extern "C"` declaration. With no Rust-side reference into `lofi_core::*`, rustc would drop the rlib from the linker's input list and the `lofi_entries_*` symbols would come out undefined. The test file pulls the rlib in explicitly with `extern crate lofi_core as _;` at the top — works under both `bazel test //app/core:ffi_test` and `cargo test -p lofi-core --features ffi`. No nested staticlib build, no `rustc-link-arg-tests` directive, no out-of-tree target directory.
