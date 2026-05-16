# app/gnome

The Linux/GNOME implementation of LoFi.

## Stack

- Rust
- GTK4 via [`gtk4-rs`](https://gtk-rs.org/gtk4-rs/)
- libadwaita via [`libadwaita-rs`](https://gtk-rs.org/gtk4-rs/git/docs/libadwaita/) for the launcher window styling
- [`gio-unix`](https://docs.rs/gio-unix) for `DesktopAppInfo` (Unix-only, not re-exported from the cross-platform `gtk::gio`)
- [`zbus`](https://docs.rs/zbus) for talking to GNOME Shell over D-Bus (planned; the client code is not yet wired up — see "Sources of window/workspace data" below)

This crate is built as both a library (`lofi_gnome`) and a binary (`lofi`) so integration tests can link against the library.

## Modules

- `apps` — enumerates installed applications by parsing `.desktop` files via `gio_unix::DesktopAppInfo`.
  - `application_directories()` returns the XDG-driven search list: `$XDG_DATA_HOME` (falling back to `$HOME/.local/share`), then each entry of `$XDG_DATA_DIRS` (falling back to `/usr/local/share:/usr/share`), each with `applications` appended.
  - `gather_applications(dirs)` reads the supplied directories, skips missing ones silently, and returns a `Vec<lofi_core::Application>`. Entries that fail `should_show()` (per the freedesktop spec) are filtered out. Non-recursive. Each returned `Application` includes `icon: Option<String>` populated from `DesktopAppInfo::icon()` via `gio::IconExt::to_string` — the freedesktop serializer (`g_icon_to_string`) for the icon GObject. The value is an icon **identifier**, not bytes: rendering is deferred to the GTK image widget at draw time, where the icon theme, scale, and target size are known. `gather_applications` guarantees that every `Application::desktop_id` is canonical — always ends in `.desktop`. The integration test pins this invariant. Canonicalization matters because `desktop_id` is the payload of `EntryRef::Application` (see `lofi-core`) and therefore the stable history/MRU key; a bare stem would break round-tripping with previously persisted references.
- `ui` — the launcher window. Public entry point `ui::build(app, entries)` constructs an `adw::ApplicationWindow` containing a `SearchEntry` over a scrolled `ListBox` and presents it. Internally holds the full gathered set in an `Rc<RefCell<UiState>>` alongside a `visible: Vec<usize>` of indices into that set. On every `search-changed` the list is fully torn down (`while let Some(child) = list_box.first_child()`) and rebuilt — simpler than diffing and fast enough at the scale of an application gather. An empty/whitespace query passes through; otherwise `lofi_core::search` ranks and filters. If the result is empty the list shows a single non-selectable "No matches" row.
- `launch` — `launch::activate(&Entry)` resolves the entry's desktop id via `gio_unix::DesktopAppInfo::new`, builds a `gdk::Display::default().app_launch_context()`, and calls `info.launch(&[], context.as_ref())`. Errors are logged to stderr and swallowed: there's no useful recovery from "the desktop file vanished between gather and click" at the UI layer.

### Keyboard

- **Up / Down** — move the selection in the list. Focus stays on the search entry, so typing continues to filter without an extra Tab.
- **Enter** (Return or KP_Enter) — `launch::activate` the selected entry and close the window. A no-op when the "No matches" row is the only thing on screen, because that row is not selectable.
- **Escape** — close the window without launching.
- Everything else propagates to the search entry, so normal text editing keeps working.

The list is rebuilt from scratch on every `search-changed`. There is no incremental diff and no debounce; both are unnecessary at application-gather scale.

Integration tests live in `tests/` and build their own `.desktop` fixtures inside a `tempfile::tempdir()`. The gatherer takes directories as a parameter, so tests never mutate process environment variables. `ui` and `launch` are exercised manually — they need a Wayland session and a running compositor to be meaningful.

## Sources of window/workspace data

Window, workspace, and display data will come from the LoFi GNOME extension (see `extension/gnome/`), which exposes `dev.jplein.LoFi.Shell.WindowManager` on the session bus. The extension covers both reads (list windows / workspaces / displays, get active window, get active display) and actions (focus, close, move-to-workspace, move/resize, maximize, activate workspace).

The extension is required because Wayland clients can't enumerate or manipulate other apps' windows directly, and Mutter doesn't implement `wlr-foreign-toplevel-management`. `org.gnome.Shell.Introspect` exists and is read-only, but its schema is too narrow (no workspace assignment, no per-window geometry, no monitor scale) to drive the launcher, so the extension publishes its own listing surface rather than the Rust side splitting reads across two D-Bus endpoints.

The Rust D-Bus client that calls into the extension has not landed yet; it arrives in a later iteration along with the window / workspace `Entry` variants in `lofi-core`.

## GNOME version support

LoFi targets exactly one version of GNOME at a time — whatever is current on the developer's NixOS system. There is no compatibility shim for older or newer GNOME releases; the extension's `shell-version` in `metadata.json` is the source of truth.
