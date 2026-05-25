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
- [ESLint](https://eslint.org/) (flat config + `typescript-eslint`) for linting
  and [Prettier](https://prettier.io/) for formatting — see *Linting and
  formatting* below.

## Version targeting

A single GNOME version is supported at a time, pinned via `shell-version` in
`metadata.json`. The current pin is GNOME **49**. When the developer's NixOS
system bumps GNOME, the extension is updated to match; there is no
multi-version compatibility layer.

## Linting and formatting

Three checks guard the TypeScript, run from this directory (`npm run check` runs
all three). `CLAUDE.md` at the repo root lists them as the required gates before
a task is considered done:

| Script | Tool | What it does |
|--------|------|--------------|
| `npm run typecheck` | `tsc --noEmit` | Strict type check (no output). |
| `npm run lint` / `npm run lint:fix` | ESLint | Catches real mistakes (unused vars, accidental `any`, shadowing). |
| `npm run format:check` / `npm run format` | Prettier | Enforces / applies formatting over `**/*.ts`. |

**ESLint** uses flat config (`eslint.config.js`): `@eslint/js` recommended plus
`typescript-eslint` recommended, with `eslint-config-prettier` applied **last**
so ESLint owns correctness and Prettier owns formatting (the two never fight
over style). The base `no-undef` rule is switched **off for `.ts` files** on
purpose: GJS code reaches for ambient globals like `global` that ESLint can't
see, while TypeScript already proves every identifier is defined via the
`@girs` ambient declarations (`ambient.d.ts`) — a second, weaker check would
only produce false positives. This is typescript-eslint's own recommendation.
Linting is intentionally close to the recommended presets rather than a bespoke
ruleset: this is a small extension and the payoff is catching bugs, not
enforcing house style.

**Prettier** config lives in `.prettierrc.json`: `tabWidth: 4` and
`singleQuote: true`, chosen to match the code that already existed so adopting
the formatter was a near-no-op rather than a churn-everything rewrite (Prettier's
other defaults — semicolons, `trailingComma: "all"`, 80-col width — already
matched). `.prettierignore` keeps it off build output, dependencies, the lock
file, and the packaged `.zip`.

Neither tool is wired into `build.sh` or the Nix build (there is no CI); they
are developer-run gates per `CLAUDE.md`. Note that the ESLint/Prettier packages
are `devDependencies`, so adding them changed `package-lock.json` and therefore
`flake.nix`'s `npmDepsHash` — see the *Build → Via Nix* note about regenerating
that hash.

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
| `app_name`          | `s`  | Human-readable app name from `Shell.WindowTracker`; empty if unresolved. |
| `app_desktop_id`    | `s`  | Canonical `.desktop`-suffixed id of the owning app, as returned by `Shell.App.get_id()`; empty if `Shell.WindowTracker` couldn't resolve a `Shell.App`. Same lookup as `app_id`/`app_name`, exposed as a separate field so the Rust side can match windows directly against `lofi_core::Application::desktop_id` without re-doing the `.desktop` suffix dance. |
| `icon`              | `s`  | Freedesktop icon identifier (themed name or absolute path) from the tracked `Shell.App.get_icon()`; empty if unresolved. |
| `workspace`         | `i`  | Workspace index, `-1` if sticky / on-all.          |
| `monitor`           | `i`  | Monitor index.                                     |
| `x`, `y`            | `i`  | Frame origin in logical pixels (global coords).    |
| `width`, `height`   | `i`  | Frame size in logical pixels.                      |
| `focused`           | `b`  | True if this is `display.focus_window`.            |
| `minimized`         | `b`  |                                                    |
| `maximized`         | `b`  | True iff horizontally AND vertically maximized.    |
| `fullscreen`        | `b`  |                                                    |
| `on_all_workspaces` | `b`  | "Sticky" in mutter parlance.                       |

`app_name`, `app_desktop_id`, and `icon` are derived from the same `Shell.WindowTracker.get_window_app(win)` lookup that produces `app_id` — the canonical app-to-window mapping GNOME's own overview uses — so all four fields are populated or empty together. The launcher consumes them to render window rows that visually match their owning application, and (via `app_desktop_id`) to build the app-to-most-recent-window map that powers the running-indicator dot and focus-instead-of-launch behaviour.

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

- `ListWindows() -> aa{sv}` — filters out `is_override_redirect()` windows. Order is `global.get_window_actors()` order (Mutter's stacking order).
- `ListWindowsMRU() -> aa{sv}` — same dict shape and override-redirect filter as `ListWindows`, but sorted most-recently-focused first (the order Alt+Tab cycles through). Backed by `global.display.get_tab_list(Meta.TabList.NORMAL_ALL, null)`, which is Mutter's canonical MRU source. The Rust launcher consumes this method exclusively today (see `app/gnome/src/windows.rs`); `ListWindows` is kept alongside it because the stacking-order list is a useful read for ad-hoc `gdbus` probing and any future caller that wants z-order rather than focus order.
- `GetActiveWindow() -> a{sv}` — empty dict if no focused window.
- `GetWindowWorkArea(t id) -> a{sv}` — work area of the monitor that owns the window with `id`, as `{x, y, width, height}` of `i` (int32) variants. The work area is the monitor rectangle minus panel/dock struts — the bounding box every geometry command computes against. Throws `WindowNotFound` if the id doesn't resolve.
- `GetWindowFrame(t id) -> a{sv}` — current frame rectangle of the window with `id`, same `{x, y, width, height}` int32 dict shape as `GetWindowWorkArea`. Only the `Center` window-action command reads this (it keeps the window's current size and recenters); the other geometry commands compute purely from the work area. Throws `WindowNotFound` if the id doesn't resolve.
- `ListWorkspaces() -> aa{sv}`
- `ListDisplays() -> aa{sv}`
- `GetActiveDisplay() -> a{sv}`

By-id actions (throw `WindowNotFound` if the id doesn't resolve):

- `FocusWindow(t id)`
- `MoveWindowToWorkspace(t id, i target_index)`
- `MoveResizeWindow(t id, i x, i y, i width, i height)` — unmaximizes (and unfullscreens if needed) before calling `move_resize_frame`. Mutter ignores `move_resize_frame` on a maximized or fullscreen window, and every caller of this method is a geometry command (center, half-width, etc.) that conceptually replaces the window's state with a precise rectangle — so the unmaximize/unfullscreen prefix is what every caller wants. Matches the behaviour of the user's original `window-commands` set.
- `MinimizeWindow(t id)` — minimize the window.
- `ToggleMaximizeWindow(t id)` — flip the maximized state. The toggle is resolved in the extension (reads `is_maximized()` and dispatches to `maximize()` or `unmaximize()`) because Mutter holds the live state; a Rust-side capture-then-act would race against external changes and cost an extra round-trip.
- `ToggleFullscreenWindow(t id)` — flip the fullscreen state. Same rationale as `ToggleMaximizeWindow` for resolving on the extension side.
- `CloseWindow(t id)`

Every window action is by-id rather than active-window. LoFi itself takes focus when its window opens, so any `*ActiveWindow` action invoked from inside the launcher would operate on LoFi's own window. The Rust caller (`app/gnome/src/commands.rs::gather_commands`) captures the previously-focused user window's id at gather time — by walking the MRU list and skipping `dev.jplein.LoFi.desktop` — and every by-id method addresses that captured id explicitly. This is also why the earlier `MoveActiveWindowToNextWorkspace`, `MoveActiveWindowToPreviousWorkspace`, `MoveResizeActiveWindow`, `MaximizeActiveWindow`, and `UnmaximizeActiveWindow` methods were removed: no Rust caller could use them safely.

Workspace action:

- `ActivateWorkspace(i index)` — throws `WorkspaceOutOfRange` if out of range.

### Errors

All errors are `GLib.Error` instances in the namespace
`dev.jplein.LoFi.Shell.Error.*`:

- `WindowNotFound`
- `WorkspaceOutOfRange`

## Launcher window animation

GNOME Shell's default open/close animation (a brief zoom/fade) is suppressed
specifically for the LoFi launcher window. The launcher is modal,
focus-driven, and dismisses on focus loss, so the standard animation adds
perceptible latency to a flow that's supposed to feel instantaneous. All
other windows keep their normal animations.

The window is identified primarily by its **GApplication id**
(`dev.jplein.LoFi`, set in `app/gnome/src/main.rs`) via
`Meta.Window.get_gtk_application_id()`. Identifying by the GApplication id
rather than by a `Shell.App` id means the match works even when no
`dev.jplein.LoFi.desktop` file is installed — which is the common case for
ad-hoc invocations and dev builds. `src/launcher.ts` falls back to
`WM_CLASS` (X11 / non-GTK path) and then to a `Shell.WindowTracker`
lookup against `dev.jplein.LoFi.desktop` as a last resort.

Implementation lives in `src/launcher.ts`, hooked into `Shell.WM`'s `map`
and `destroy` signals. We don't prevent Shell from starting the animation;
we let it start and immediately `remove_all_transitions()` on the window
actor, snapping it to its terminal state. On `destroy` we also call
`WM.completed_destroy(actor)` because Shell otherwise keeps the destroy
gated behind the (now zero-duration) animation it thinks is still running.

This avoids monkey-patching the private `Main.wm._mapWindow` /
`_destroyWindow` paths, which shift between GNOME versions.

## Workspace boundary semantics

`MoveWindowToWorkspace` (explicit target) does **not** follow the window to
its destination — the caller asked to move a specific window to a specific
workspace, not to navigate.

- `MoveWindowToWorkspace(id, -1)` raises `WorkspaceOutOfRange`.
- `MoveWindowToWorkspace(id, n_workspaces)` with dynamic workspaces
  (`org.gnome.mutter dynamic-workspaces = true`) calls
  `wm.append_new_workspace(false, current_time)` to grow the count, then
  moves the window. With static workspaces it raises `WorkspaceOutOfRange`.
- Any move-to-workspace operation on a sticky window calls `win.unstick()`
  first, then performs the move.

## Files

```
extension/gnome/
  metadata.json           - GNOME extension manifest (uuid, shell-version)
  package.json            - TS toolchain devDeps + check/build scripts (private)
  package-lock.json       - committed; required by Nix's buildNpmPackage
  tsconfig.json           - strict mode, ES2022, bundler resolution
  eslint.config.js        - ESLint flat config (typescript-eslint + prettier)
  .prettierrc.json        - Prettier config (4-space, single-quote)
  .prettierignore         - keeps Prettier off dist / deps / lock / zip
  ambient.d.ts            - pulls in @girs/* ambient declarations
  dbus-interface.xml      - D-Bus introspection XML
  build.sh                - tsc + zip pipeline; produces .shell-extension.zip
  src/
    extension.ts          - Extension subclass; enable/disable lifecycle
    service.ts            - D-Bus method implementations
    windows.ts            - MetaWindow -> a{sv} serializer / lookup
    workspaces.ts         - Workspace serializer
    displays.ts           - Monitor serializer
    launcher.ts           - Suppresses Shell's open/close animation for LoFi's own window
    errors.ts             - GLib.Error helpers under our error namespace
    dbus-xml.d.ts         - Declares the build-emitted dist/dbus-xml.js module
```

## Build

### From the dev shell

```
nix develop
cd extension/gnome
npm install      # first time only, regenerates node_modules from the lock file
npm run check    # typecheck + eslint + prettier (see "Linting and formatting")
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

Three options, ordered from least to most declarative.

### One-shot via `nix run`

From a clone of the repo:

```
nix run .#install-extension
```

This unpacks the freshly-built `.zip` into `~/.local/share/gnome-shell/extensions/lofi-shell@jplein.dev/` and adds the UUID to `org.gnome.shell.enabled-extensions` (via `gnome-extensions enable`). On Wayland you still need to log out and back in for the shell to load it the first time. A companion `nix run .#uninstall-extension` disables + uninstalls.

### Manual

```
nix build .#extension   # or `npm run build`
gnome-extensions install --force ./result/lofi-shell@jplein.dev.shell-extension.zip
gnome-extensions enable lofi-shell@jplein.dev
```

Log out / back in on Wayland; on Xorg, `Alt+F2` and run `r` to restart the shell in-place.

### Declaratively via home-manager

The flake exposes `homeManagerModules.lofi`. In your home-manager flake:

```nix
{
  inputs.lofi.url = "github:jplein/lofi";   # or path:/path/to/checkout

  outputs = { self, nixpkgs, home-manager, lofi, ... }: {
    homeConfigurations."you@host" = home-manager.lib.homeManagerConfiguration {
      pkgs = nixpkgs.legacyPackages.x86_64-linux;
      modules = [
        lofi.homeManagerModules.lofi
        {
          programs.lofi.enable = true;
        }
      ];
    };
  };
}
```

`programs.lofi.enable = true` installs the launcher binary, symlinks the extension into `~/.local/share/gnome-shell/extensions/lofi-shell@jplein.dev`, and adds the UUID to dconf. Knobs:

- `programs.lofi.package` — override the launcher binary package.
- `programs.lofi.extensionPackage` — override the extension package.
- `programs.lofi.enableShellExtension = false` — install the binary only, skip the extension and dconf changes.

Caveat: if your home-manager config sets `dconf.settings."org/gnome/shell".enabled-extensions` elsewhere, the merge will conflict. Either keep all your extensions in one place (`programs.lofi.enableShellExtension = false` and add the UUID to your existing list), or use `lib.mkForce` on a combined list.

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
  --method dev.jplein.LoFi.Shell.WindowManager.GetActiveWindow

# Replace 12345 with a real window id from ListWindows / GetActiveWindow.
gdbus call --session \
  --dest dev.jplein.LoFi.Shell \
  --object-path /dev/jplein/LoFi/Shell \
  --method dev.jplein.LoFi.Shell.WindowManager.MoveResizeWindow \
  12345 0 0 960 1080
```

## Scope (deliberately small)

In:

- One D-Bus interface, one object, exported in `enable()` and torn down in
  `disable()`.
- Window / workspace / display reads, plus per-window work-area and current-frame
  reads (used by the launcher's window-action commands).
- Window focus / close / minimize / move / resize / toggle-maximize /
  toggle-fullscreen.
- Move window to specific workspace.
- Activate workspace.

Out (later iterations):

- Change-signals — the Rust client polls on each operation.
- Multi-GNOME-version compatibility.
- Tile-snapping (callers compute geometry and call `MoveResizeWindow`).
- Real monitor connector/product names (synthesized as `Monitor N+1`).
- Always-on-top / above / below.
- Per-monitor workspace edge cases (`workspaces-only-on-primary`).
