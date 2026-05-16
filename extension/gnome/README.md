# extension/gnome

The GNOME Shell extension that gives LoFi its window-management surface. It
exposes a single D-Bus interface that the Rust launcher calls into for
window, workspace, and display operations.

## Why this exists

On Wayland, regular clients can't manipulate other apps' windows. GNOME Shell
(which is the Wayland compositor) can, because it runs Mutter in-process. An
extension is the only supported way to expose that capability to an external
app like LoFi. `org.gnome.Shell.Introspect` is read-only and exposes a very
narrow schema, so we publish our own surface here.

## Stack

- TypeScript (strict mode, `noUncheckedIndexedAccess`, `noImplicitOverride`),
  transpiled to GJS-compatible ESM JavaScript.
- [`@girs/gnome-shell`](https://www.npmjs.com/package/@girs/gnome-shell) +
  [`@girs/gjs`](https://www.npmjs.com/package/@girs/gjs) for type definitions
  over the Mutter / Shell / Clutter / GIO APIs.
- D-Bus as the IPC surface to the LoFi launcher (Gio.DBusExportedObject).

## Version targeting

A single GNOME version is supported at a time, pinned via `shell-version` in
`metadata.json`. The current pin is GNOME **49**. When the developer's NixOS
system bumps GNOME, the extension is updated to match; there is no
multi-version compatibility layer.

## D-Bus surface

- **Bus name**: `dev.jplein.LoFi.Shell`
- **Object path**: `/dev/jplein/LoFi/Shell`
- **Interface**: `dev.jplein.LoFi.Shell.WindowManager`

The full XML is in `dbus-interface.xml`. Returned dictionaries use `a{sv}` so
fields can be added without breaking the wire format.

### Window dict (`a{sv}`)

| Key                 | Type | Notes                                              |
|---------------------|------|----------------------------------------------------|
| `id`                | `t`  | Mutter window id (session-stable, unsigned 64).    |
| `title`             | `s`  | Empty string if Mutter returns null.               |
| `app_id`            | `s`  | From `Shell.WindowTracker`; empty if not resolved. |
| `workspace`         | `i`  | Workspace index, `-1` if sticky / on-all.          |
| `monitor`           | `i`  | Monitor index.                                     |
| `x`, `y`            | `i`  | Frame origin in logical pixels (global coords).    |
| `width`, `height`   | `i`  | Frame size in logical pixels.                      |
| `focused`           | `b`  | True if this is `display.focus_window`.            |
| `minimized`         | `b`  |                                                    |
| `maximized`         | `b`  | True iff horizontally AND vertically maximized.    |
| `fullscreen`        | `b`  |                                                    |
| `on_all_workspaces` | `b`  | "Sticky" in mutter parlance.                       |

### Workspace dict (`a{sv}`)

| Key         | Type | Notes                                                       |
|-------------|------|-------------------------------------------------------------|
| `index`     | `i`  | Zero-based.                                                 |
| `name`      | `s`  | Synthesized as `"Workspace N+1"` for now.                   |
| `active`    | `b`  | True for the currently-active workspace.                    |
| `n_windows` | `i`  | Count from `workspace.list_windows().length`.               |

### Display dict (`a{sv}`)

| Key       | Type | Notes                                                     |
|-----------|------|-----------------------------------------------------------|
| `index`   | `i`  | Monitor index in `Main.layoutManager.monitors`.            |
| `name`    | `s`  | Synthesized as `"Monitor N+1"` for now.                    |
| `x`, `y`  | `i`  | Origin in the global multi-monitor coordinate space.       |
| `width`, `height` | `i` | Logical pixels.                                       |
| `scale`   | `d`  | From `display.get_monitor_scale(index)`.                   |
| `primary` | `b`  | Matches `display.get_primary_monitor()`.                   |
| `active`  | `b`  | Monitor of the focused window (else under-pointer).        |

### Methods

Reads:

- `ListWindows() -> aa{sv}` — filters out `is_override_redirect()` windows.
- `GetActiveWindow() -> a{sv}` — empty dict if no focused window.
- `ListWorkspaces() -> aa{sv}`
- `ListDisplays() -> aa{sv}`
- `GetActiveDisplay() -> a{sv}`

Active-window actions (operate on `display.focus_window`; throw
`NoActiveWindow` if none):

- `MoveActiveWindowToNextWorkspace()`
- `MoveActiveWindowToPreviousWorkspace()`
- `MoveResizeActiveWindow(i x, i y, i width, i height)`
- `MaximizeActiveWindow()`
- `UnmaximizeActiveWindow()`

By-id actions (throw `WindowNotFound` if the id doesn't resolve):

- `FocusWindow(t id)`
- `MoveWindowToWorkspace(t id, i target_index)`
- `MoveResizeWindow(t id, i x, i y, i width, i height)`
- `CloseWindow(t id)`

Workspace action:

- `ActivateWorkspace(i index)` — throws `WorkspaceOutOfRange` if out of range.

### Errors

All errors are `GLib.Error` instances in the namespace
`dev.jplein.LoFi.Shell.Error.*`:

- `NoActiveWindow`
- `WindowNotFound`
- `WorkspaceOutOfRange`

## Workspace boundary semantics

The relative active-window movers (`MoveActiveWindowToNext/Previous-Workspace`)
match GNOME's built-in `move-to-workspace-next/prev` keybindings: the user's
view follows the window to the target workspace (`follow = true`).
`MoveWindowToWorkspace` (explicit target) does **not** follow — the caller
asked to move a specific window to a specific workspace, not to navigate.

- `MoveActiveWindowToNextWorkspace` at the last workspace:
  - If `org.gnome.mutter dynamic-workspaces` is true, the extension calls
    `wm.append_new_workspace(false, current_time)` to grow the count, then
    moves the window.
  - If dynamic workspaces are off (static count), the call is a silent no-op.
- `MoveActiveWindowToPreviousWorkspace` at workspace 0 is a silent no-op.
- `MoveWindowToWorkspace(id, -1)` raises `WorkspaceOutOfRange`.
- `MoveWindowToWorkspace(id, n_workspaces)` with dynamic workspaces appends
  and moves; with static workspaces it raises `WorkspaceOutOfRange`.
- Any move-to-workspace operation on a sticky window calls `win.unstick()`
  first, then performs the move.

## Files

```
extension/gnome/
  metadata.json           - GNOME extension manifest (uuid, shell-version)
  package.json            - TS toolchain devDeps (private)
  package-lock.json       - committed; required by Nix's buildNpmPackage
  tsconfig.json           - strict mode, ES2022, bundler resolution
  ambient.d.ts            - pulls in @girs/* ambient declarations
  dbus-interface.xml      - D-Bus introspection XML
  build.sh                - tsc + zip pipeline; produces .shell-extension.zip
  src/
    extension.ts          - Extension subclass; enable/disable lifecycle
    service.ts            - D-Bus method implementations
    windows.ts            - MetaWindow -> a{sv} serializer / lookup
    workspaces.ts         - Workspace serializer
    displays.ts           - Monitor serializer
    errors.ts             - GLib.Error helpers under our error namespace
    dbus-xml.d.ts         - Declares the build-emitted dist/dbus-xml.js module
```

## Build

### From the dev shell

```
nix develop
cd extension/gnome
npm install      # first time only, regenerates node_modules from the lock file
npm run typecheck
npm run build
```

`npm run build` runs `./build.sh`, which:

1. Compiles `src/*.ts` to `dist/*.js` via `tsc`.
2. Copies `metadata.json` and `dbus-interface.xml` into `dist/`.
3. Generates `dist/dbus-xml.js` (a tiny ES module that exports the XML string
   under the name `dbusXml`).
4. Zips the contents of `dist/` into
   `lofi-shell@jplein.dev.shell-extension.zip` at the directory root.

### Via Nix

```
nix build .#extension
```

Output: `result/lofi-shell@jplein.dev.shell-extension.zip`. The derivation
uses `pkgs.buildNpmPackage`, so `package-lock.json` must be present in the
source tree (it is committed). If you ever update `package.json`, regenerate
the lock file with `npm install`, then update `flake.nix`'s `npmDepsHash`:
temporarily set it to `pkgs.lib.fakeHash`, run `nix build .#extension`, and
paste the real hash that Nix prints back into `flake.nix`.

## Install / enable

```
gnome-extensions install --force ./lofi-shell@jplein.dev.shell-extension.zip
gnome-extensions enable lofi-shell@jplein.dev
```

Then log out and back in (on Wayland) so the shell picks up the new
extension. On Xorg you can also `Alt+F2` and run `r` to restart the shell
in-place.

## Smoke testing with gdbus

Once enabled, every method is reachable from the session bus:

```
busctl --user introspect dev.jplein.LoFi.Shell /dev/jplein/LoFi/Shell

gdbus call --session \
  --dest dev.jplein.LoFi.Shell \
  --object-path /dev/jplein/LoFi/Shell \
  --method dev.jplein.LoFi.Shell.WindowManager.ListWindows

gdbus call --session \
  --dest dev.jplein.LoFi.Shell \
  --object-path /dev/jplein/LoFi/Shell \
  --method dev.jplein.LoFi.Shell.WindowManager.MoveActiveWindowToNextWorkspace

gdbus call --session \
  --dest dev.jplein.LoFi.Shell \
  --object-path /dev/jplein/LoFi/Shell \
  --method dev.jplein.LoFi.Shell.WindowManager.MoveResizeActiveWindow \
  0 0 960 1080
```

## Scope (deliberately small)

In:

- One D-Bus interface, one object, exported in `enable()` and torn down in
  `disable()`.
- Window / workspace / display reads.
- Window focus / close / move / resize / maximize.
- Move window to specific workspace, plus relative next/prev movers.
- Activate workspace.

Out (later iterations):

- Change-signals — the Rust client polls on each operation.
- Multi-GNOME-version compatibility.
- Tile-snapping (callers compute geometry and call `MoveResizeWindow`).
- Real monitor connector/product names (synthesized as `Monitor N+1`).
- Minimize / always-on-top / above / below.
- Per-monitor workspace edge cases (`workspaces-only-on-primary`).
