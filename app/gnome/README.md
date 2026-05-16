# app/gnome

The Linux/GNOME implementation of LoFi.

## Stack

- Rust
- GTK4 via [`gtk4-rs`](https://gtk-rs.org/gtk4-rs/)
- libadwaita via [`libadwaita-rs`](https://gtk-rs.org/gtk4-rs/git/docs/libadwaita/) for the launcher window styling
- [`gio-unix`](https://docs.rs/gio-unix) for `DesktopAppInfo` (Unix-only, not re-exported from the cross-platform `gtk::gio`)
- [`zbus`](https://docs.rs/zbus) for talking to GNOME Shell over D-Bus

This crate is built as both a library (`lofi_gnome`) and a binary (`lofi`) so integration tests can link against the library.

## Modules

- `apps` — enumerates installed applications by parsing `.desktop` files via `gio_unix::DesktopAppInfo`.
  - `application_directories()` returns the XDG-driven search list: `$XDG_DATA_HOME` (falling back to `$HOME/.local/share`), then each entry of `$XDG_DATA_DIRS` (falling back to `/usr/local/share:/usr/share`), each with `applications` appended.
  - `gather_applications(dirs)` reads the supplied directories, skips missing ones silently, and returns a `Vec<lofi_core::Application>`. Entries that fail `should_show()` (per the freedesktop spec) are filtered out. Non-recursive.

Integration tests live in `tests/` and build their own `.desktop` fixtures inside a `tempfile::tempdir()`. The gatherer takes directories as a parameter, so tests never mutate process environment variables.

## Sources of window/workspace data

LoFi gets information about windows and workspaces from two D-Bus interfaces:

1. `org.gnome.Shell.Introspect` — built into GNOME Shell. Used for read-only listing of running applications and windows where it's sufficient.
2. The LoFi GNOME extension (see `extension/gnome/`) — used for actions that aren't available through `Introspect`: focusing a window, switching workspaces, moving windows between workspaces, resizing the active window, and closing windows.

The extension is required because Wayland clients can't enumerate or manipulate other apps' windows directly, and Mutter doesn't implement `wlr-foreign-toplevel-management`.

## GNOME version support

LoFi targets exactly one version of GNOME at a time — whatever is current on the developer's NixOS system. There is no compatibility shim for older or newer GNOME releases; the extension's `shell-version` in `metadata.json` is the source of truth.
