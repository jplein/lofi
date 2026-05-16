# app/gnome

The Linux/GNOME implementation of LoFi.

## Stack

- Rust
- GTK4 via [`gtk4-rs`](https://gtk-rs.org/gtk4-rs/)
- libadwaita via [`libadwaita-rs`](https://gtk-rs.org/gtk4-rs/git/docs/libadwaita/) for the launcher window styling
- [`zbus`](https://docs.rs/zbus) for talking to GNOME Shell over D-Bus

## Sources of window/workspace data

LoFi gets information about windows and workspaces from two D-Bus interfaces:

1. `org.gnome.Shell.Introspect` — built into GNOME Shell. Used for read-only listing of running applications and windows where it's sufficient.
2. The LoFi GNOME extension (see `extension/gnome/`) — used for actions that aren't available through `Introspect`: focusing a window, switching workspaces, moving windows between workspaces, resizing the active window, and closing windows.

The extension is required because Wayland clients can't enumerate or manipulate other apps' windows directly, and Mutter doesn't implement `wlr-foreign-toplevel-management`.

## GNOME version support

LoFi targets exactly one version of GNOME at a time — whatever is current on the developer's NixOS system. There is no compatibility shim for older or newer GNOME releases; the extension's `shell-version` in `metadata.json` is the source of truth.
