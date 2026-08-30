# extension

Desktop-environment shell extensions that give LoFi capabilities the regular client APIs don't expose.

## Why this directory exists

Some things a launcher wants to do — focusing a specific window, moving a window to another workspace, closing a window it doesn't own — aren't available to ordinary clients on a modern Linux desktop. The compositor is the only process that can perform them, and the supported way to reach into the compositor from outside is a shell extension.

This directory holds those extensions, one per desktop environment **that needs one**. They are shims into the compositor, not launchers in their own right. The launcher lives in `app/`; the extensions exist solely so `app/` has a D-Bus surface to call when the standard APIs fall short. A desktop whose own APIs are sufficient gets no subdirectory here at all — see the layout note below.

## Layout

The structure parallels `app/`: each subdirectory targets a single desktop environment.

- `gnome/` — GNOME Shell extension exposing a D-Bus interface (`dev.jplein.LoFi.Shell.WindowManager`) for window, workspace, and display introspection and actions Mutter doesn't otherwise expose to regular apps.

There is deliberately no `macos/` here. AppKit's accessibility APIs and Apple Events give a regular app equivalent capabilities, so the macOS launcher doesn't need a shell-side counterpart.

There is deliberately no `niri/` either, for the same reason in a different form: Niri ships a complete window/workspace IPC surface in the compositor itself, on a unix socket at `$NIRI_SOCKET`. LoFi's Niri backend talks to that directly (`app/linux/src/backend/niri/`), so there is nothing for a shim to add. The rule below — "anything the launcher can already do through standard, non-privileged APIs does not belong here" — is what decides this: GNOME needs an extension because `org.gnome.Shell.Introspect` is too narrow, and Niri does not because its IPC is not.

## What belongs here

- Compositor-side code that exposes capabilities the launcher can't get any other way.
- A D-Bus (or equivalent IPC) surface scoped tightly to those capabilities.

## What does not belong here

- Launcher logic. Matching, ranking, UI, configuration, application enumeration via `.desktop` files — all of that is in `app/`. Extensions stay thin so they remain easy to keep working across desktop-environment updates.
- Anything the launcher can already do through standard, non-privileged APIs (e.g. enumerating installed applications from `.desktop` files). The extension does expose its own window/workspace/display reads despite `org.gnome.Shell.Introspect` existing, because that built-in surface is read-only and too narrow (no workspace info, no per-window geometry, no monitor scale) to drive the launcher.
