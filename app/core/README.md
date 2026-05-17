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

### `Entry`, `EntryKind`, `EntryRef`, and `resolve`

`Entry` is the runtime sum type the UI consumes. Today its variants are `Entry::Application(Application)` and `Entry::Window(Window)`. `Workspace` and `Command` become additional variants as those features land.

`EntryKind` is the matching unit discriminant (`Copy`/`Hash`), useful for grouping or filtering without holding the payload.

`EntryRef` is the **persistence handle**: an enum-shaped `{type, id}` tagged with `#[serde(tag = "type", content = "id", rename_all = "snake_case")]`. Today it has two variants — `EntryRef::Application(String)` carrying a canonical `desktop_id`, and `EntryRef::Window(u64)` carrying a Mutter window id. The window id is session-scoped (see `Window::id` above), so a persisted `EntryRef::Window` only resolves within the same shell session that produced it; cross-session window history is out of scope here.

`resolve(&[Entry], &EntryRef) -> Option<&Entry>` is a linear scan that pairs `EntryRef`s back to the live `Entry`s from a gather.

`Entry` provides four match-dispatched accessors: `name()`, `icon()`, `kind()`, and `reference()`. They use exhaustive `match` (not `if let`) so that adding an `Entry` variant is a compile error until every accessor is updated.

### `matcher::search`

Signature:

```rust
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry>
```

Behavior:

- An empty or whitespace-only `query` is a passthrough: every entry is returned in input slice order. This makes the matcher safe to call unconditionally from the UI on every keystroke including the initial empty one.
- A non-empty query is split on whitespace into tokens. Each token must fuzzy-match the entry's haystack (intersection semantics); per-token scores are summed.
- Results sort by score **descending**, with ascending name as the tiebreaker. The tiebreaker keeps a stable visual order when two entries score the same; otherwise rerunning the same query could shuffle ties.

The "haystack" — the text we match against — is built per-variant by an exhaustive `match` on `Entry` inside a private `haystack` function. For `Entry::Application` it is `"{name} {desktop_id}"`, so typing either the display name or the desktop id works. For `Entry::Window` it is `"{title} {app_name}"` when `app_name` is `Some`, and just `title` when it is `None`. The practical consequence is that typing an app name (e.g. `"firefox"`) matches both the Firefox application entry and every open Firefox window in the same gather. Future `Entry` variants force this function to be updated (no `_` arm).

The fuzzy implementation is [`fuzzy-matcher`](https://docs.rs/fuzzy-matcher)'s `SkimMatcherV2` configured with `ignore_case()`. It's the same algorithm `skim` uses, which is in turn a port of fzf's scoring. `fuzzy-matcher` is the second direct dependency of this crate, alongside `serde`.

### Why two types for one concept

Display fields drift between sessions: locale changes the display name, the user switches icon themes, an app gets renamed or its `.desktop` file moves. A history or MRU store that pickled the whole `Application` would either accumulate stale strings or have to re-key itself on every change.

`EntryRef` is the minimum information needed to point at "the same thing" across runs. Persistence layers serialize `EntryRef`. The UI receives `&[Entry]` from a fresh gather, and `resolve` rebuilds the link.

`Application` and `Entry` are **deliberately not** `Serialize`/`Deserialize`. Only `EntryRef` is. Do not "helpfully" add serde derives to the other types — that would invite callers to persist values that are guaranteed to go stale.

### Dependencies

`serde` (with `derive`) is a direct dependency solely for `EntryRef`'s tagged-enum representation. `fuzzy-matcher` is a direct dependency for `matcher::search` (Skim-style fuzzy scoring). `serde_json` is a dev-dependency for the JSON round-trip test.

`Workspace` and `Command` will land here as their corresponding features are built out.
