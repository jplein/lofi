# GNOME Shell extension — window manager D-Bus surface

## Summary

Implement the `lofi-shell@jplein.dev` GNOME 49 extension (TypeScript) that exposes a `dev.jplein.LoFi.Shell.WindowManager` D-Bus interface for window/workspace/display introspection and actions. Approved design at `/home/jplein/.claude/plans/great-now-let-s-start-mellow-dolphin.md`. No automated tests — verification is `tsc --noEmit` + `npm run build`.

## File-by-file

### 1. `metadata.json`

```json
{
  "uuid": "lofi-shell@jplein.dev",
  "name": "LoFi Shell",
  "description": "D-Bus surface for the LoFi launcher: window, workspace, and display management.",
  "shell-version": ["49"],
  "url": "https://github.com/jplein/lofi"
}
```

### 2. `package.json`

```json
{
  "name": "lofi-shell-extension",
  "private": true,
  "type": "module",
  "scripts": {
    "typecheck": "tsc --noEmit",
    "build": "./build.sh"
  },
  "devDependencies": {
    "@girs/gjs": "^4.0.0-rc.9",
    "@girs/gnome-shell": "^49.0.0",
    "typescript": "^5.6.0"
  }
}
```

Coder may add explicit peer pins (`@girs/meta-16`, `@girs/clutter-16`, `@girs/st-16`, `@girs/gobject-2.0`, `@girs/gio-2.0`, `@girs/glib-2.0`) if `npm install` requires them. Adjust to whatever resolves cleanly; do not invent versions.

### 3. `tsconfig.json`

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2022"],
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "noImplicitOverride": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src",
    "declaration": false,
    "sourceMap": false,
    "types": []
  },
  "include": ["src/**/*.ts", "ambient.d.ts"]
}
```

### 4. `ambient.d.ts`

Bring in `@girs/gnome-shell` ambient declarations and declare the build-emitted `./dbus-xml.js`:

```ts
import '@girs/gjs';
import '@girs/gjs/dom';
import '@girs/gnome-shell/ambient';
import '@girs/gnome-shell/extensions/global';

declare module './dbus-xml.js' {
    export const dbusXml: string;
}
```

Fallback local declarations for `resource:///org/gnome/shell/extensions/extension.js` and `resource:///org/gnome/shell/ui/main.js` only if `@girs/gnome-shell` doesn't already ship them — try without the fallbacks first.

### 5. `dbus-interface.xml`

Full XML with `<node>` wrapper and one `<interface name="dev.jplein.LoFi.Shell.WindowManager">` containing every method:

- Reads: `ListWindows() → aa{sv}`, `GetActiveWindow() → a{sv}`, `ListWorkspaces() → aa{sv}`, `ListDisplays() → aa{sv}`, `GetActiveDisplay() → a{sv}`.
- Active-window actions: `MoveActiveWindowToNextWorkspace`, `MoveActiveWindowToPreviousWorkspace`, `MoveResizeActiveWindow(i x, i y, i w, i h)`, `MaximizeActiveWindow`, `UnmaximizeActiveWindow`.
- By-id actions: `FocusWindow(t id)`, `MoveWindowToWorkspace(t id, i target_index)`, `MoveResizeWindow(t id, i x, i y, i w, i h)`, `CloseWindow(t id)`.
- Workspace: `ActivateWorkspace(i index)`.

### 6. `src/extension.ts`

```ts
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import { WindowManagerService } from './service.js';

export default class LofiShellExtension extends Extension {
    private service: WindowManagerService | null = null;

    override enable(): void {
        this.service = new WindowManagerService();
        this.service.export();
    }

    override disable(): void {
        this.service?.unexport();
        this.service = null;
    }
}
```

### 7. `src/service.ts`

Top-level constants: `BUS_NAME = 'dev.jplein.LoFi.Shell'`, `OBJECT_PATH = '/dev/jplein/LoFi/Shell'`, `MUTTER_SCHEMA_ID = 'org.gnome.mutter'`, `DYNAMIC_WORKSPACES_KEY = 'dynamic-workspaces'`.

`WindowManagerService`:
- Fields `exported: Gio.DBusExportedObject | null = null`, `busOwnerId = 0`.
- `export()`: `wrapJSObject(dbusXml, this)` → `.export(Gio.DBus.session, OBJECT_PATH)` → `Gio.bus_own_name(SESSION, BUS_NAME, NONE, null, null, null)`.
- `unexport()`: `bus_unown_name` if owner != 0, then `exportedObject.unexport()`, reset both fields.
- Reads delegate to leaf modules (`windows.list()`, etc.); `GetActiveWindow` returns `windows.active() ?? {}`.
- Each action method follows the architect's spec literally.

**Boundary semantics in `service.ts`** (architect spelled these out precisely):

`MoveActiveWindowToNextWorkspace`:
1. `win = global.display.focus_window`; null → throw `noActiveWindow()`.
2. `current = is_on_all_workspaces() ? active_index : get_workspace()?.index() ?? active_index`.
3. `target = current + 1`.
4. If `target >= n_workspaces`:
   - Read `org.gnome.mutter` `dynamic-workspaces`. True → `append_new_workspace(false, current_time)`. False → silent `return`.
5. `if (win.is_on_all_workspaces()) win.unstick();`
6. `win.change_workspace_by_index(target, /* follow */ true);`

`MoveActiveWindowToPreviousWorkspace`: target = current − 1; < 0 → silent return; else unstick + `change_workspace_by_index(target, true)`.

`MoveWindowToWorkspace(id, target_index)`:
1. `win = windows.byId(id)`; null → `windowNotFound(id)`.
2. `target_index < 0` → `workspaceOutOfRange`.
3. Dynamic on: `> n_workspaces` → range error; `== n_workspaces` → `append_new_workspace`.
4. Dynamic off: `>= n_workspaces` → range error.
5. Unstick if sticky. `change_workspace_by_index(target_index, /* follow */ false)`.

Other methods: `FocusWindow`/`CloseWindow`/`MoveResizeWindow` look up by id; `ActivateWorkspace` checks range; `MaximizeActiveWindow`/`UnmaximizeActiveWindow`/`MoveResizeActiveWindow` operate on `focus_window` (throw `NoActiveWindow` if null).

### 8. `src/windows.ts`

```ts
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';

export type WindowDict = Record<string, GLib.Variant>;
export function list(): WindowDict[];
export function active(): WindowDict | null;
export function byId(id: bigint): Meta.Window | null;
export function serialize(win: Meta.Window): WindowDict;
```

- `list()`: `global.get_window_actors().map(a => a.meta_window).filter(w => w !== null && !w.is_override_redirect()).map(serialize)`.
- `active()`: serialize `global.display.focus_window` or null.
- `byId(id)`: iterate window actors, compare `BigInt(w.get_id()) === BigInt(id)`.
- `serialize(win)`: build the dict with `GLib.Variant.new_*` for each field (see architect's plan §2.8 for exact field-by-field mapping).

### 9. `src/workspaces.ts`

```ts
import GLib from 'gi://GLib';

export type WorkspaceDict = Record<string, GLib.Variant>;
export function list(): WorkspaceDict[];
export function active(): WorkspaceDict | null;
```

Iterate `0..wm.n_workspaces`, build dicts with `index`, `name` (synthesize `"Workspace N+1"`), `active` (matches `wm.get_active_workspace_index()`), `n_windows` (`ws.list_windows().length`).

### 10. `src/displays.ts`

```ts
import GLib from 'gi://GLib';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

export type DisplayDict = Record<string, GLib.Variant>;
export function list(): DisplayDict[];
export function active(): DisplayDict;
```

Iterate `Main.layoutManager.monitors`; per-monitor scale via `global.display.get_monitor_scale(i)`; primary via `global.display.get_primary_monitor()`; active by focused-window's `get_monitor()` with `global.display.get_current_monitor()` fallback. Synthesize `name = "Monitor N+1"` for now (Mutter's MonitorManager connector/product API can land later).

### 11. `src/errors.ts`

```ts
import GLib from 'gi://GLib';

const ERROR_DOMAIN_PREFIX = 'dev.jplein.LoFi.Shell.Error';

function dbusError(name: string, message: string): GLib.Error {
    return new GLib.Error(
        GLib.quark_from_string(`${ERROR_DOMAIN_PREFIX}.${name}`),
        0,
        message,
    );
}

export function noActiveWindow(): GLib.Error;
export function windowNotFound(id: bigint | number): GLib.Error;
export function workspaceOutOfRange(idx: number): GLib.Error;
```

### 12. `build.sh` (executable)

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

rm -rf dist
npx tsc

cp metadata.json dist/
cp dbus-interface.xml dist/

node -e "
const fs = require('fs');
const xml = fs.readFileSync('dbus-interface.xml', 'utf8');
fs.writeFileSync('dist/dbus-xml.js', 'export const dbusXml = ' + JSON.stringify(xml) + ';\n');
"

# Plain zip — gnome-extensions install --force accepts any zip with metadata.json
# at the root. Works equivalently inside the Nix sandbox and the dev shell.
(cd dist && zip -r "../lofi-shell@jplein.dev.shell-extension.zip" .)
```

`chmod +x build.sh` at creation. We don't use `gnome-extensions pack` because it requires gnome-shell tooling that's tedious to surface in the Nix sandbox; plain `zip` produces an equivalent installable artifact.

### 13. `flake.nix` — devShell + extension derivation

**Two changes** to `/home/jplein/Git/jplein/lofi/flake.nix`:

1. **Add `nodejs` to the devShell** so `npm` is available when developers enter `nix develop`.

2. **Add an `extension` package** that produces the `.zip` artifact via `pkgs.buildNpmPackage`:

```nix
# In the `let` block, alongside `crateInfo`:
extension = pkgs.buildNpmPackage {
  pname = "lofi-shell-extension";
  version = crateInfo.version;
  src = ./extension/gnome;

  # First-time build: set to pkgs.lib.fakeHash; nix build will fail with the
  # real hash, which we paste back here. Required by buildNpmPackage to verify
  # the offline npm install in the sandbox.
  npmDepsHash = pkgs.lib.fakeHash;

  nativeBuildInputs = [ pkgs.zip ];

  # buildNpmPackage's default installPhase wants to install node_modules into
  # the output. We only want the .zip the build script produces.
  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp lofi-shell@jplein.dev.shell-extension.zip $out/
    runHook postInstall
  '';
};
```

Then in the `packages` set:

```nix
packages = {
  default = lofi;
  lofi = lofi;
  extension = extension;
};
```

And the devShell change:

```nix
devShells.default = pkgs.mkShell {
  nativeBuildInputs = nativeBuildInputs ++ [ rustToolchain pkgs.nodejs ];
  inherit buildInputs;
};
```

**Prerequisite for the derivation**: `extension/gnome/package-lock.json` must exist and be committed. The coder generates it by running `npm install` inside `nix develop` before attempting the Nix build. The lock file is what `buildNpmPackage` reads to know what to install offline.

**The `npmDepsHash` dance**: on first `nix build .#extension`, Nix prints the real hash. The coder substitutes the real value for `pkgs.lib.fakeHash` and rebuilds. (Standard Nix-npm pattern; documented in nixpkgs `buildNpmPackage` docs.)

**Verification of the derivation**:
- `nix build .#extension` produces `result/lofi-shell@jplein.dev.shell-extension.zip`.
- Diff against `extension/gnome/lofi-shell@jplein.dev.shell-extension.zip` from `npm run build` — they should be byte-identical or at least functionally equivalent.

### 14. README update (`extension/gnome/README.md`)

After build passes, rewrite to reflect the as-built D-Bus surface, the workspace-boundary semantics (next/prev follow=true, explicit follow=false; dynamic-vs-static; sticky unstick-then-move), install/build flow, and gdbus smoke-test examples. Remove the stale claim that listing comes from `org.gnome.Shell.Introspect` — listing is now this extension's responsibility.

## Implementation order

1. **Flake first**: add `pkgs.nodejs` to the devShell so subsequent `npm` calls work. Don't add the `extension` package derivation yet — we need `package-lock.json` for it. Verify with `nix develop --command which npm`.
2. Manifests/config: `metadata.json`, `package.json`, `tsconfig.json`, `ambient.d.ts`, `dbus-interface.xml`, `build.sh` (chmod +x).
3. `cd extension/gnome && npm install` (inside `nix develop`). Adjust `@girs/*` versions if resolution fails. Commits both `package.json` and the generated `package-lock.json`.
4. `src/errors.ts`.
5. `src/windows.ts`, `src/workspaces.ts`, `src/displays.ts` (any order).
6. `src/service.ts`.
7. `src/extension.ts`.
8. `npm run typecheck` clean.
9. `npm run build` produces `lofi-shell@jplein.dev.shell-extension.zip` (verify it appears in `extension/gnome/`).
10. **Now add the `extension` derivation to `flake.nix`** with `npmDepsHash = pkgs.lib.fakeHash`. Run `nix build .#extension` — it fails with the real hash; substitute it in.
11. `nix build .#extension` clean; `result/lofi-shell@jplein.dev.shell-extension.zip` exists.
12. Update `extension/gnome/README.md`.

## Verification

- `nix develop --command which npm` — npm on PATH via the updated devShell.
- `npm install` from `extension/gnome/` — produces `package-lock.json`.
- `npm run typecheck` — zero errors.
- `npm run build` — `.zip` artifact produced in `extension/gnome/`.
- `nix build .#extension` — derivation builds, `result/lofi-shell@jplein.dev.shell-extension.zip` exists.
- Manual install / smoke testing deferred to the user.

## Out of scope (do not grow into these)

- Rust D-Bus client (next iteration).
- Change-signals.
- Multi-GNOME-version support.
- Tile-snapping.
- Real connector/product monitor names (synthesized for now).
- Per-monitor workspaces edge cases.
- Minimize / always-on-top / above / below.
- Automated tests.

## Edge cases the coder must handle in code (no test file)

- `GetActiveWindow` with no focused window → `{}`.
- `ListWindows` filters `is_override_redirect()`.
- `MoveActiveWindowToNextWorkspace` at last workspace: dynamic on → append + move; off → silent no-op.
- `MoveActiveWindowToPreviousWorkspace` at workspace 0 → silent no-op.
- `MoveWindowToWorkspace(id, -1)` → `WorkspaceOutOfRange`.
- `MoveWindowToWorkspace(id, n_workspaces)` dynamic → append + move; static → out-of-range.
- Sticky in any move-to-workspace path → `unstick()` first.
- `FocusWindow`/`CloseWindow`/`MoveResizeWindow` with bad id → `WindowNotFound`.
- `ActivateWorkspace(out_of_range)` → `WorkspaceOutOfRange`.
- `MaximizeActiveWindow`/`UnmaximizeActiveWindow`/`MoveResizeActiveWindow` with no focused window → `NoActiveWindow`.
