# app/gnome

The Linux/GNOME implementation of LoFi.

## Stack

- Rust
- GTK4 via [`gtk4-rs`](https://gtk-rs.org/gtk4-rs/)
- libadwaita via [`libadwaita-rs`](https://gtk-rs.org/gtk4-rs/git/docs/libadwaita/) for the launcher window styling
- [`gio-unix`](https://docs.rs/gio-unix) for `DesktopAppInfo` (Unix-only, not re-exported from the cross-platform `gtk::gio`)
- [`zbus`](https://docs.rs/zbus) for talking to the LoFi GNOME extension over the session bus. The blocking proxy (`gen_blocking = true`, `gen_async = false`) is used deliberately: the GTK main thread is synchronous and the gather happens once at startup, so the cost of an async runtime would buy nothing here.

This crate is built as both a library (`lofi_gnome`) and a binary (`lofi`) so integration tests can link against the library.

## Modules

- `apps` — enumerates installed applications by parsing `.desktop` files via `gio_unix::DesktopAppInfo`.
  - `application_directories()` returns the XDG-driven search list: `$XDG_DATA_HOME` (falling back to `$HOME/.local/share`), then each entry of `$XDG_DATA_DIRS` (falling back to `/usr/local/share:/usr/share`), each with `applications` appended.
  - `gather_applications(dirs)` reads the supplied directories, skips missing ones silently, and returns a `Vec<lofi_core::Application>`. Entries that fail `should_show()` (per the freedesktop spec) are filtered out. Non-recursive. Each returned `Application` includes `icon: Option<String>` populated from `DesktopAppInfo::icon()` via `gio::IconExt::to_string` — the freedesktop serializer (`g_icon_to_string`) for the icon GObject. The value is an icon **identifier**, not bytes: rendering is deferred to the GTK image widget at draw time, where the icon theme, scale, and target size are known. `gather_applications` guarantees that every `Application::desktop_id` is canonical — always ends in `.desktop`. The integration test pins this invariant. Canonicalization matters because `desktop_id` is the payload of `EntryRef::Application` (see `lofi-core`) and therefore the stable history/MRU key; a bare stem would break round-tripping with previously persisted references. Results are deduped by canonical `desktop_id` with first-directory-wins semantics — this is the XDG shadowing convention, and the dir order from `application_directories()` already produces the right precedence (`$XDG_DATA_HOME` shadows `$XDG_DATA_DIRS`), so the dedup belongs here rather than at a caller; a user installing Ghostty via both the Nix system profile and `~/.local/share/applications` would not expect it to appear twice in the launcher.
- `launch` — `launch::activate(&Entry)` is the single dispatch point for "the user pressed Enter on this row". An exhaustive `match` routes entries:
  - `Entry::Application(app)` branches on `app.recent_window_id`. When `Some(id)`, the app is currently running and `activate` calls `windows::focus_window(id)` — raising the most-recently-used window of the app, mirroring the GNOME dock's "click a running app's icon = raise its window" behaviour. When `None`, it falls back to the original `gio_unix::DesktopAppInfo::new` lookup + `info.launch(&[], context.as_ref())` (the `gdk::Display::default().app_launch_context()` carries the launching display so the new app starts on the right monitor). We deliberately do **not** fall back from focus to launch when `focus_window` fails: the gather-vs-click race is real but rare, and a phantom second instance would be more surprising than a silent no-op.
  - `Entry::Window(w)` routes to `windows::focus_window(w.id)`, unchanged — a single D-Bus call that raises the window **and** switches the workspace if needed (the extension's `FocusWindow` is implemented via `meta_window.activate(time)`).
  - `Entry::Workspace(w)` routes to `workspaces::activate_workspace(w.index)` — a single D-Bus call that switches GNOME to the workspace with that index (the extension's `ActivateWorkspace` calls `workspace.activate(time)`).

  Errors at any branch are logged to stderr and swallowed: there's no useful recovery from "the desktop file vanished between gather and click", "the window id no longer resolves", or "the workspace was removed between gather and click" at the UI layer.
- `windows` — the Rust side of the LoFi GNOME extension. Defines a `#[zbus::proxy]` blocking client for `dev.jplein.LoFi.Shell.WindowManager` and exposes two free functions:
  - `gather_windows()` calls **`ListWindowsMRU`** (not `ListWindows`) and maps each returned dict into a `lofi_core::Window`. The MRU-ordered result is what makes the combine step in `main` (below) correct: the first window encountered for a given `app_desktop_id` is, by construction, the most recently focused one for that app. Empty `app_name` / `icon` / `app_desktop_id` strings on the wire become `None`; see `app/core/README.md`'s `Window` subsection for why. On any `zbus::Error` it logs via `eprintln!` and returns an empty `Vec`.
  - `focus_window(id)` calls `FocusWindow` with the same error policy.

  The proxy trait declares **both** `list_windows` and `list_windows_mru` for completeness, but only the MRU path is currently consumed. `list_windows_mru` carries an explicit `#[zbus(name = "ListWindowsMRU")]` attribute: zbus uses `heck` to map Rust `snake_case` method names to PascalCase wire names, and heck would otherwise produce `ListWindowsMru` (treating `MRU` as a regular word), which the extension does not export. The other methods don't need an explicit name because their snake_case names round-trip cleanly through heck.

  The current implementation opens a fresh blocking session connection per call; reusing a single connection is a deliberate non-goal until profiling shows it matters. No `unwrap`/`expect` in this module — error paths are all `match` / `if let` / `?` with `eprintln!` early returns.
- `workspaces` — the Rust side of the extension's workspace surface. Defines its own `#[zbus::proxy]` blocking client for `dev.jplein.LoFi.Shell.WindowManager` and exposes two free functions:
  - `gather_workspaces()` calls `ListWorkspaces` and maps each returned dict into a `lofi_core::Workspace`. The extension also emits `active` and `n_windows` per dict; zvariant ignores dict keys not declared on the target struct, so they are silently dropped on decode — adding either field back later is a one-line change in `DbusWorkspace`. On any `zbus::Error` it logs via `eprintln!` and returns an empty `Vec`, same policy as `windows`.
  - `activate_workspace(index)` calls `ActivateWorkspace` with the same error policy.

  The proxy trait is declared **independently** from `WindowManager` in `windows.rs` even though both target the same D-Bus interface, service, and path. Sharing the proxy across modules would couple them for no win: the wire surface happens to be one object, but the two concerns (window listing/focus vs workspace listing/switch) are otherwise unrelated and the cleaner module boundary is worth the duplicate `#[zbus::proxy]` block. As with `windows`, connections are opened per call and `eprintln!`-and-degrade is the only error policy.
- `ui` — the launcher window. Public entry point `ui::build(app, entries, mru_store, mru_index)` constructs an `adw::ApplicationWindow` containing a `SearchEntry` over a scrolled `ListBox` and presents it. Internally holds the full gathered set in an `Rc<RefCell<UiState>>` alongside a `visible: Vec<usize>` of indices into that set and a `mru_position: HashMap<EntryRef, usize>` (rank 0 = most recent) built from `mru_index` at construction time. On every `search-changed` the list is fully torn down (`while let Some(child) = list_box.first_child()`) and rebuilt — simpler than diffing and fast enough at the scale of an application gather. An empty/whitespace query passes through; otherwise `lofi_core::search` filters (no scoring — see `app/core/README.md`'s matcher section). The resulting index list is then **stable-sorted by MRU rank**, with `usize::MAX` as the fallback for entries absent from the persisted index, so in-MRU entries rise in most-recent-first order and the rest sink to the bottom in input order. If the result is empty the list shows a single non-selectable "No matches" row.

  MRU is the **sole** sort key — not "tiebreak after fuzzy score". This is what keeps the selected row stable while the user types: with the matcher reduced to a filter, narrowing the query can shrink the visible set but never reorder it, so "Foo" → "Foob" → "Foobar" never moves the row under the cursor.

  Each row's icon column is a vertical `gtk::Box` containing the `gtk::Image` plus a small CSS-styled `gtk::Box` for the running-indicator dot (6x6, circular via `border-radius: 9999px`, coloured `alpha(@theme_fg_color, 0.8)` so it adapts to light/dark themes). The dot widget is always added but hidden via `set_visible(false)` for entries other than running Applications — keeping it in the layout regardless means rows never shift horizontally when a single row's running state changes. CSS for the dot is registered once per process via `install_styles()`, gated by an `OnceLock<()>` latch (`STYLES_INSTALLED`) because `build()` runs on every `connect_activate` firing and re-registering the same provider would stack identical priority entries. `install_styles` is a silent no-op when there is no default `gdk::Display` (headless tests, broken environment); the dot falls back to whatever GTK renders for an unstyled empty `gtk::Box` in that case.

### Keyboard

- **Up / Down** — move the selection in the list. Focus stays on the search entry, so typing continues to filter without an extra Tab.
- **Enter** (Return or KP_Enter) — `launch::activate` the selected entry and close the window. A no-op when the "No matches" row is the only thing on screen, because that row is not selectable.
- **Escape** — close the window without launching.
- Everything else propagates to the search entry, so normal text editing keeps working.

The list is rebuilt from scratch on every `search-changed`. There is no incremental diff and no debounce; both are unnecessary at application-gather scale.

Integration tests live in `tests/` and build their own `.desktop` fixtures inside a `tempfile::tempdir()`. The gatherer takes directories as a parameter, so tests never mutate process environment variables. `ui` and `launch` are exercised manually — they need a Wayland session and a running compositor to be meaningful.

## Sources of window/workspace data

Window, workspace, and display data come from the LoFi GNOME extension (see `extension/gnome/`), which exposes `dev.jplein.LoFi.Shell.WindowManager` on the session bus. The extension covers both reads (list windows / workspaces / displays, get active window, get active display) and actions (focus, close, move-to-workspace, move/resize, maximize, activate workspace).

The extension is required because Wayland clients can't enumerate or manipulate other apps' windows directly, and Mutter doesn't implement `wlr-foreign-toplevel-management`. `org.gnome.Shell.Introspect` exists and is read-only, but its schema is too narrow (no workspace assignment, no per-window geometry, no monitor scale) to drive the launcher, so the extension publishes its own listing surface rather than the Rust side splitting reads across two D-Bus endpoints.

The Rust clients for the window and workspace slices of that surface live in the `windows` and `workspaces` modules above: `gather_windows()` and `gather_workspaces()` are called from `main` alongside `apps::gather_applications`, the three `Vec`s are concatenated into the single `Vec<Entry>` the matcher and UI consume, and `launch::activate` dispatches `Entry::Window` and `Entry::Workspace` back through their respective proxies. The display client is still future work and will follow the same shape (blocking zbus proxy, empty-Vec-on-error policy, no shared connection until profiling motivates one).

### App-to-recent-window combine step

`main.rs::on_activate` does one piece of cross-cutting work between the two gatherers: it stamps each `Application` with the id of its most recently focused window (if any) so the UI and `launch` can act on it. The shape is:

1. Walk `windows` in order (which is MRU, because `gather_windows` calls `ListWindowsMRU`).
2. Build a `HashMap<String, u64>` keyed by `app_desktop_id`, inserting only on the **first** occurrence of each id — later entries are less recent and must not clobber the earlier one. The let-chain guard with `!mru.contains_key(id)` enforces this.
3. For each `Application`, set `app.recent_window_id = mru.get(&app.desktop_id).copied()`.

This needs to live in `main` rather than in either gatherer module because the two `Vec`s are otherwise independent — `apps::gather_applications` is platform-agnostic enough that it shouldn't know about `Shell.WindowTracker`, and `windows::gather_windows` doesn't have the application list to annotate. The combine step is the cheapest possible glue: one map allocation and one linear pass per `Vec`.

## MRU activation history

The launcher persists an MRU (most-recently-used) record of activations and uses that recency as the only sort key for the displayed list. The store itself — schema, write/read API, locking strategy — lives in `lofi-core::mru` (see `app/core/README.md`); this section covers only what the GNOME platform layer does with it.

### Path resolution

`main::mru_state_path` returns `$XDG_STATE_HOME/lofi/mru.sqlite` when `$XDG_STATE_HOME` is set and non-empty, otherwise `$HOME/.local/state/lofi/mru.sqlite`, otherwise `None`. The shape mirrors `apps::application_directories` deliberately — the launcher already does manual XDG resolution there, so a second crate (`xdg`, `directories`) would be the bigger dependency than the duplicated logic. Returning `None` instead of panicking is the same policy as everywhere else in this binary: a missing-`HOME` environment is degraded but not fatal.

### Open and read once per invocation

`main::on_activate` opens the `MruStore` immediately after gathering applications and windows. Both the `open` and the subsequent `read_all` are wrapped in `.map_err(|e| eprintln!(...)).ok()`, so any failure (no resolvable path, permission denied on the parent dir, corrupt SQLite header, disk full) downgrades to logging and leaves the UI with `None` for the store and an empty `Vec<EntryRef>` for the index. The launcher never refuses to come up because the history is broken; the worst case is "first run after a corrupt DB shows no recency order this session".

The read is a snapshot: if another LoFi process bumps the DB concurrently, this process's UI does not see the change until its next session. That's fine — concurrent launches are rare, and re-reading on every keystroke would be solving a problem we don't have.

### Sorting and bumping in `ui::build`

`ui::build` accepts `Option<Rc<MruStore>>` and `Vec<EntryRef>` alongside `entries`. It converts the index into a `HashMap<EntryRef, usize>` keyed on rank (0 = most recent) and stores it on `UiState`. `populate_list` then stable-sorts the visible index list by `mru_position.get(entry.reference()).copied().unwrap_or(usize::MAX)`. The matcher decides what's visible; MRU decides the order.

On both Enter (`SearchEntry::connect_activate`) and click (`ListBox::connect_row_activated`) the helper `bump_mru` runs **synchronously, immediately before `launch::activate`**. The UPSERT is microseconds — the user never notices — and synchronous is simpler than fire-and-forget when the connection lives in a closure that's about to be dropped as the window closes. If the bump fails (disk full, the DB went away between open and click) we `eprintln!` and proceed with the launch; surfacing a "could not record history" error to the user would be obnoxious for something this peripheral.

### Window vs. Application bumping

A `Entry::Window` activation only bumps that Window's row. It does **not** also bump the underlying Application: the two are independent rows in the same table, and the launcher treats "picked the Chrome — github.com window" as evidence that window is recent, not as evidence Chrome-the-app is recent. The opposite (bumping an app while also bumping its most recent window) was rejected for the same reason: coupling would muddle the recency signal the user gives us per row.

### Workspace activations and MRU

`Entry::Workspace` activations flow through the **same** `EntryRef`-keyed MRU path as Applications and Windows — no special-casing in `ui.rs::bump_mru`. `Entry::reference()` already returns `EntryRef::Workspace(index)` for the Workspace variant, the MRU SQLite table is generic over the tagged-enum serialization of `EntryRef` (see `app/core/README.md`'s `mru::MruStore` section), and the sort key in `populate_list` keys off `EntryRef` regardless of variant. The result: picking "Workspace 2" bumps `{"type":"workspace","id":1}` (index 1 = 1-based "Workspace 2"), and on the next launcher invocation that row sorts to the top alongside any other recent picks. This is also why no MRU plumbing was needed when the Workspace variant landed.

### Persistence note: stale window rows

Window rows from prior shell sessions are dead weight. Mutter window ids are session-ephemeral (see `Window::id` in `app/core/README.md`), so a `EntryRef::Window(12345)` written yesterday will never resolve against today's gather. The launcher tolerates this — `resolve` simply returns `None` for those refs and the UI ignores them — and the rows just accumulate in the table. Periodic cleanup (delete oldest N when the table exceeds 2N rows, or drop unresolved Window refs at startup) is a future pass; we don't yet have enough sense of the steady-state size to commit to a policy.

## GNOME version support

LoFi targets exactly one version of GNOME at a time — whatever is current on the developer's NixOS system. There is no compatibility shim for older or newer GNOME releases; the extension's `shell-version` in `metadata.json` is the source of truth.
