# extension/gnome

The GNOME Shell extension that gives LoFi the ability to perform window and workspace actions.

## Why this exists

On Wayland, regular clients can't manipulate other apps' windows. GNOME Shell (which is the Wayland compositor) can, because it runs Mutter in-process. An extension is the only supported way to expose that capability to an external app like LoFi.

## Stack

- TypeScript, transpiled to GJS-compatible JavaScript (ESM extension format, GNOME 45+)
- [`@girs/gnome-shell`](https://www.npmjs.com/package/@girs/gnome-shell) and friends for type definitions over the Mutter/Shell/Clutter APIs
- D-Bus as the IPC surface to the LoFi launcher

## Scope

The extension is intentionally thin. It exposes a D-Bus interface and nothing else — no UI, no preferences page, no in-shell behavior. The methods cover only the actions that aren't already available through `org.gnome.Shell.Introspect`:

- Focus a window by id
- Switch to a workspace
- Move a window to a workspace
- Resize the active window
- Close a window

Listing windows and workspaces is done by the Rust side against `org.gnome.Shell.Introspect` where possible, to keep this extension small and durable across GNOME updates.

## Version targeting

A single GNOME version is supported at a time, pinned via `shell-version` in `metadata.json`. When the developer's NixOS system bumps GNOME, the extension is updated to match — there is no multi-version compatibility layer.
