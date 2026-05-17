# app/core

The platform-agnostic shared crate (`lofi-core`). Defines the cross-platform data model that every platform implementation populates and the UI consumes.

## Why this crate exists

LoFi runs on both GNOME (Rust + GTK4) and macOS (planned: Swift UI on top of a Rust core exposed via a C ABI). The two platforms share nothing at the windowing-system level, but they do share the *shape* of the things a launcher cares about: applications, windows, workspaces, commands.

`core` holds those shared types and nothing else. Keeping it free of platform dependencies is the whole point:

- A macOS build of `core` must compile without pulling in `gtk`, `gio`, or any Linux-only crate. Otherwise the C-ABI surface for the Swift side either breaks or has to be `cfg`-forked.
- Conversely, a GNOME build must not need anything from `objc2` or the Apple frameworks.

If a type or function needs a platform crate to exist, it does not belong here. It belongs in `app/gnome/` (or the future `app/macos/`).

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

### `Entry`, `EntryKind`, `EntryRef`, and `resolve`

`Entry` is the runtime sum type the UI consumes. Today its variants are `Entry::Application(Application)`, `Entry::Window(Window)`, and `Entry::Workspace(Workspace)`. `Command` becomes an additional variant as that feature lands.

`EntryKind` is the matching unit discriminant (`Copy`/`Hash`), useful for grouping or filtering without holding the payload.

`EntryRef` is the **persistence handle**: an enum-shaped `{type, id}` tagged with `#[serde(tag = "type", content = "id", rename_all = "snake_case")]`. Today it has three variants — `EntryRef::Application(String)` carrying a canonical `desktop_id`, `EntryRef::Window(u64)` carrying a Mutter window id, and `EntryRef::Workspace(i32)` carrying a workspace index. The window id is session-scoped (see `Window::id` above), so a persisted `EntryRef::Window` only resolves within the same shell session that produced it; cross-session window history is out of scope here. The workspace index has the weaker session-stable-but-can-shift property described in the `Workspace` section above — same dead-weight tolerance applies.

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

The "haystack" — the text we match against — is built per-variant by an exhaustive `match` on `Entry` inside a private `haystack` function. For `Entry::Application` it is `"{name} {desktop_id}"`, so typing either the display name or the desktop id works. For `Entry::Window` it is `"{title} {app_name}"` when `app_name` is `Some`, and just `title` when it is `None`. The practical consequence is that typing an app name (e.g. `"firefox"`) matches both the Firefox application entry and every open Firefox window in the same gather. For `Entry::Workspace` the haystack is `name` alone — no second field worth concatenating, and the default `"Workspace N"` label already makes `"work"`, `"2"`, and `"workspace 2"` all match the right row; a custom workspace-naming extension flows its label through unchanged. Future `Entry` variants force this function to be updated (no `_` arm).

The fuzzy implementation is [`fuzzy-matcher`](https://docs.rs/fuzzy-matcher)'s `SkimMatcherV2` configured with `ignore_case()`. It's the same algorithm `skim` uses, which is in turn a port of fzf's scoring. `fuzzy-matcher` is one direct dependency of this crate, alongside `serde`, `serde_json`, and `rusqlite`.

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

`Command` will land here as its corresponding feature is built out.
