# extension

Desktop-environment shell extensions that give LoFi capabilities the regular client APIs don't expose.

## Why this directory exists

Some things a launcher wants to do — focusing a specific window, moving a window to another workspace, closing a window it doesn't own — aren't available to ordinary clients on a modern Linux desktop. The compositor is the only process that can perform them, and the supported way to reach into the compositor from outside is a shell extension.

This directory holds those extensions, one per desktop environment. They are shims into the compositor, not launchers in their own right. The launcher lives in `app/`; the extensions exist solely so `app/` has a D-Bus surface to call when the standard APIs fall short.

## Layout

The structure parallels `app/`: each subdirectory targets a single desktop environment.

- `gnome/` — GNOME Shell extension exposing a thin D-Bus interface for window and workspace actions Mutter doesn't otherwise expose to regular apps.

There is deliberately no `macos/` here. AppKit's accessibility APIs and Apple Events give a regular app equivalent capabilities, so the macOS launcher doesn't need a shell-side counterpart.

## What belongs here

- Compositor-side code that exposes capabilities the launcher can't get any other way.
- A D-Bus (or equivalent IPC) surface scoped tightly to those capabilities.

## What does not belong here

- Launcher logic. Matching, ranking, UI, configuration, application enumeration via `.desktop` files — all of that is in `app/`. Extensions stay thin so they remain easy to keep working across desktop-environment updates.
- Anything the launcher can already do through standard, non-privileged APIs (e.g. listing windows via `org.gnome.Shell.Introspect`).
