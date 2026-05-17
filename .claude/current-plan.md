# Window-action Commands — port the window-commands set to LoFi

## Context

The user's separate `window-commands` repo defines nine focused-window actions implemented as GNOME-extension methods (each one a self-contained `impl(): boolean` that reads `global.display.focus_window`, computes geometry from `window.get_work_area_current_monitor()`, and calls `move_resize_frame` / `minimize` / `maximize` / etc.). The user wants these exposed as launcher entries in LoFi so they can be triggered by typing a name.

The nine commands are:

| Kind                | Display name        | Icon name                  | Action                                   |
|--                   |--                   |--                          |--                                        |
| `Center`            | Center              | `focus-windows-symbolic`   | Keep size, center in work area           |
| `CenterHalf`        | Center half         | `view-dual-symbolic`       | width/2 × full height, centered          |
| `CenterTwoThirds`   | Center two-thirds   | `sidebar-show-symbolic`    | width*2/3 × full height, centered        |
| `LeftHalf`          | Left half           | `view-dual-symbolic`       | width/2 × full height, left edge         |
| `RightHalf`         | Right half          | `view-dual-symbolic`       | width/2 × full height, right edge        |
| `StandardSize`      | Standard size       | `focus-windows-symbolic`   | width*2/3 × height*2/3, centered         |
| `Minimize`          | Minimize            | `window-minimize-symbolic` | minimize                                 |
| `ToggleMaximize`    | Toggle maximize     | `window-maximize-symbolic` | flip maximized state                     |
| `ToggleFullscreen`  | Toggle fullscreen   | `view-fullscreen-symbolic` | flip fullscreen state                    |

Architectural shape: Rust does the logic, the extension is a thin D-Bus wire over Mutter — matching the rest of the codebase. Commands target windows by **id** (not "active") to dodge the focus-race issue: LoFi itself is the focused window while open, so any `*ActiveWindow` call from inside LoFi would operate on LoFi. The target id is the previously-focused user window, captured at gather time from `ListWindowsMRU`.

Bounds checking is not needed; every geometry command computes against `work_area` which already excludes panel/dock struts, and all positions/sizes are `work_area.x + nonneg` / `fraction * work_area.width`. Mutter clamps to app min-size hints, which the original code accepts (we do too).

## Confirmed decisions

1. **By-id targeting only** — every window action takes a `u64` id. No more `*ActiveWindow` methods (LoFi focus race).
2. **Remove the existing active-window methods**: `MoveActiveWindowToNextWorkspace`, `MoveActiveWindowToPreviousWorkspace`, `MoveResizeActiveWindow`, `MaximizeActiveWindow`, `UnmaximizeActiveWindow`. They aren't called from Rust today; removing now avoids dead surface. (The `GetActiveWindow` query stays — it's a read, not an action, and may serve other queries later.)
3. **Geometry math in Rust** — `lofi_core` gets a pure `compute_geometry(kind, work_area) -> Option<(x,y,w,h)>` function. Returns `None` for state-toggle commands (Minimize, ToggleMaximize, ToggleFullscreen).
4. **`MoveResizeWindow(id, ...)` always unmaximizes + unfullscreens first** — matches the originals, and is what every caller would want.
5. **Toggle state lives in the extension** for `ToggleMaximizeWindow(id)` and `ToggleFullscreenWindow(id)`. Mutter has the live state; Rust would have to capture-then-act with a stale-state window. One round-trip vs. two.
6. **Work area by id**: new extension method `GetWindowWorkArea(id) -> a{sv}` returning `{x, y, width, height}`. We can't rely on "active monitor" because LoFi may be on a different monitor than the target window.
7. **Commands appear in the list only when there's a previously-focused user window**. `gather_commands()` returns empty otherwise. (Matches the original's `if (!window) return false`.)
8. **LoFi is filtered out** when picking the target window. We compare `app_desktop_id == "dev.jplein.LoFi.desktop"` and take the first non-LoFi window from `gather_windows()` (MRU-ordered).
9. **Command entries persist in MRU** like every other `EntryRef::*`. `EntryRef::Command(String)` with snake_case id (`"center"`, `"center_half"`, etc.) — stable across sessions.

## File-by-file

### Extension

#### `extension/gnome/dbus-interface.xml`

- **Remove**: `MoveActiveWindowToNextWorkspace`, `MoveActiveWindowToPreviousWorkspace`, `MoveResizeActiveWindow`, `MaximizeActiveWindow`, `UnmaximizeActiveWindow`.
- **Add**:
  ```xml
  <method name="MinimizeWindow">
    <arg type="t" name="id" direction="in"/>
  </method>
  <method name="ToggleMaximizeWindow">
    <arg type="t" name="id" direction="in"/>
  </method>
  <method name="ToggleFullscreenWindow">
    <arg type="t" name="id" direction="in"/>
  </method>
  <method name="GetWindowWorkArea">
    <arg type="t" name="id" direction="in"/>
    <arg type="a{sv}" name="work_area" direction="out"/>
  </method>
  ```

#### `extension/gnome/src/service.ts`

- Drop the five removed-action implementations and any helpers used only by them (`dynamicWorkspacesEnabled`, `currentWorkspaceIndexFor`, `moveWindowToWorkspaceIndex` — verify usage; `moveWindowToWorkspaceIndex` may still be needed by `MoveWindowToWorkspace` so keep it if so).
- Add the four new method implementations:
  - `MinimizeWindow(id)`: `lookupWindow(id).minimize()`.
  - `ToggleMaximizeWindow(id)`: read `is_maximized()`, call `maximize()` or `unmaximize()` accordingly.
  - `ToggleFullscreenWindow(id)`: read `is_fullscreen()`, call `make_fullscreen()` or `unmake_fullscreen()` accordingly.
  - `GetWindowWorkArea(id)`: `lookupWindow(id).get_work_area_current_monitor()` → `{x, y, width, height}` as `a{sv}` of `i` variants.
- **Modify** `MoveResizeWindow(id, x, y, w, h)`: prepend `if (win.is_fullscreen()) win.unmake_fullscreen(); win.unmaximize();` before the `move_resize_frame` call. Comment explaining why: every caller wants this and the original window-commands set already does it.

#### `extension/gnome/README.md`

- Methods table: remove the five active-window action entries; add the four new by-id methods; note the unmaximize/unfullscreen prefix on `MoveResizeWindow`.

### `lofi-core`

#### `app/core/src/lib.rs`

Add:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Center,
    CenterHalf,
    CenterTwoThirds,
    LeftHalf,
    RightHalf,
    StandardSize,
    Minimize,
    ToggleMaximize,
    ToggleFullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub kind: CommandKind,
    pub target_window_id: u64,
    pub work_area: WorkArea,
}
```

Variant additions:
- `EntryKind::Command`
- `Entry::Command(Command)`
- `EntryRef::Command(String)` — JSON `{"type":"command","id":"center"}`. The `String` is the snake-case form of `CommandKind` (i.e. what `serde_json::to_string(&kind).unwrap()` would emit, sans quotes). Provide `CommandKind::as_id(&self) -> &'static str` to produce it without serde at runtime.

Accessor arms:
- `Entry::name()` → display name from `kind.display_name()`.
- `Entry::icon()` → icon name from `kind.icon_name()`.
- `Entry::kind()` → `EntryKind::Command`.
- `Entry::reference()` → `EntryRef::Command(kind.as_id().to_string())`.

New `CommandKind` methods:
```rust
impl CommandKind {
    pub fn as_id(&self) -> &'static str { ... }       // "center", "center_half", ...
    pub fn display_name(&self) -> &'static str { ... } // "Center", "Center half", ...
    pub fn icon_name(&self) -> &'static str { ... }    // "focus-windows-symbolic", ...
    pub fn from_id(id: &str) -> Option<CommandKind> { ... } // parse for resolve / MRU rehydrate
}
```

#### `app/core/src/commands.rs` (new)

```rust
use crate::{CommandKind, WorkArea};

/// Compute the target geometry for a geometry command given the work area.
/// Returns `None` for commands that don't move/resize the window (Minimize,
/// ToggleMaximize, ToggleFullscreen).
pub fn compute_geometry(
    kind: CommandKind,
    work_area: &WorkArea,
) -> Option<(i32, i32, i32, i32)> {
    match kind {
        CommandKind::Center => None,            // see note below
        CommandKind::CenterHalf => { ... },
        CommandKind::CenterTwoThirds => { ... },
        CommandKind::LeftHalf => { ... },
        CommandKind::RightHalf => { ... },
        CommandKind::StandardSize => { ... },
        CommandKind::Minimize
        | CommandKind::ToggleMaximize
        | CommandKind::ToggleFullscreen => None,
    }
}
```

Note on `Center`: it needs the window's *current* frame size to recenter without resizing. Two options:
- Extend the signature: `compute_geometry(kind, work_area, current_frame)`. `current_frame` is unused for the others.
- Special-case `Center` in the activation path: read the live frame size via a new D-Bus call (`GetWindowFrame(id)`) and compute in Rust.

Pick **the signature-extension** option — `compute_geometry(kind, work_area, frame: (i32, i32, i32, i32))` where `frame` is `(x, y, w, h)`. For commands other than `Center` the frame is ignored, but every command has a captured target id so the platform layer can pass the captured frame at gather time. The `Command` struct gains a `pub current_frame: (i32, i32, i32, i32)` field (or named `pub frame: Rect`).

#### `app/core/src/matcher.rs`

Haystack gains the `Entry::Command(c) => c.kind.display_name().to_string()` arm. Match on display name only.

### `lofi-gnome`

#### `app/gnome/src/windows.rs`

Extend the zbus proxy trait:
- Remove the no-longer-existent methods from the trait — actually nothing references the active-window methods in Rust today, so the trait stays as is.
- Add:
  ```rust
  fn minimize_window(&self, id: u64) -> zbus::Result<()>;
  fn toggle_maximize_window(&self, id: u64) -> zbus::Result<()>;
  fn toggle_fullscreen_window(&self, id: u64) -> zbus::Result<()>;
  fn get_window_work_area(&self, id: u64) -> zbus::Result<DbusWorkArea>;
  ```
- Add the dict struct:
  ```rust
  #[derive(Debug, Type, DeserializeDict)]
  #[zvariant(signature = "a{sv}")]
  struct DbusWorkArea { x: i32, y: i32, width: i32, height: i32 }
  ```
- Public wrappers: `pub fn minimize_window(id)`, `pub fn toggle_maximize_window(id)`, `pub fn toggle_fullscreen_window(id)`, `pub fn get_window_work_area(id) -> Option<WorkArea>`. Same `eprintln!`-and-degrade pattern as the existing functions.

Also need: a way to read a window's current frame for the `Center` command. The wire `DbusWindow` already includes the position+size? Let me re-check — looking at the existing `DbusWindow`:
```rust
struct DbusWindow {
    id: u64,
    title: String,
    app_name: String,
    app_desktop_id: String,
    icon: String,
    workspace: i32,
}
```
No frame. The extension emits one but we don't decode it. We can add `x, y, width, height: i32` fields to `DbusWindow` and to `lofi_core::Window` (or just to the dict-decode struct + as needed in commands.rs). Verify the extension emits these — looking at `extension/gnome/src/windows.ts` may be needed; the dict almost certainly includes geometry.

Actually simpler than threading the frame through `Window`: add `GetWindowFrame(id)` returning `{x, y, width, height}`. Symmetric with `GetWindowWorkArea`. Avoids polluting the Window type. **Decision: add `GetWindowFrame(id)` to the extension** and a Rust wrapper.

#### `app/gnome/src/commands.rs` (new)

```rust
use lofi_core::{Command, CommandKind, WorkArea};
use crate::windows;

const LOFI_DESKTOP_ID: &str = "dev.jplein.LoFi.desktop";

/// Gather the static command set, populated with the captured target window
/// and its monitor's work area. Returns empty if there is no usable target
/// (no windows other than LoFi itself, or the work-area / frame query fails).
pub fn gather_commands() -> Vec<Command> { ... }
```

Implementation:
1. `let windows = windows::gather_windows();`
2. Find the first `w` where `w.app_desktop_id != Some(LOFI_DESKTOP_ID.into())`. None → return `vec![]`.
3. Fetch `WorkArea` via `windows::get_window_work_area(w.id)`; on `None` return `vec![]`.
4. Fetch frame via `windows::get_window_frame(w.id)`; on `None` return `vec![]`.
5. Build nine `Command` instances, one per `CommandKind`, all sharing the same `target_window_id`, `work_area`, `current_frame`.
6. Return.

#### `app/gnome/src/launch.rs`

Add the `Entry::Command(cmd)` arm:

```rust
Entry::Command(cmd) => {
    use lofi_core::CommandKind;
    use lofi_core::commands::compute_geometry;
    let id = cmd.target_window_id;
    match cmd.kind {
        CommandKind::Minimize => windows::minimize_window(id),
        CommandKind::ToggleMaximize => windows::toggle_maximize_window(id),
        CommandKind::ToggleFullscreen => windows::toggle_fullscreen_window(id),
        kind => {
            if let Some((x, y, w, h)) = compute_geometry(kind, &cmd.work_area, cmd.current_frame) {
                windows::move_resize_window(id, x, y, w, h);
            }
        }
    }
}
```

(There is no existing `move_resize_window` wrapper — `windows.rs` only exposes `focus_window`. The plan adds `move_resize_window` too. Check the proxy trait — it has `MoveResizeWindow` per the XML, so a wrapper is one new function.)

#### `app/gnome/src/main.rs`

After workspaces gather, add:
```rust
let command_vec = commands::gather_commands();
```
Extend `entries`:
```rust
entries.extend(command_vec.into_iter().map(Entry::Command));
```
Update `Vec::with_capacity` to include `command_vec.len()`.

Import the new module: `use lofi_gnome::{apps, commands, ui, windows, workspaces};`.

#### `app/gnome/src/lib.rs`

Add `pub mod commands;`.

#### `app/gnome/src/ui.rs`

`kind_to_str` gains `EntryKind::Command => "Command"`.

### READMEs

Done after the work lands by the technical writer. Affected:
- `extension/gnome/README.md` (method-table changes).
- `app/core/README.md` (Command + WorkArea + CommandKind subsection; matcher haystack; EntryRef serialization).
- `app/gnome/README.md` (`commands` module + Command activation arm in `launch`; LoFi-filter rationale; the `MoveResizeWindow`-now-unmaximizes-first behavior).

## Tests

### `app/core/src/commands.rs` `mod tests`

Pure unit tests for `compute_geometry`. No D-Bus, no GTK. Use a fixed `WorkArea { x: 100, y: 50, width: 1800, height: 1000 }` so the offsets aren't trivially zero — catches both relative-position and absolute-position bugs.

1. `center_returns_none_geometry_is_computed_from_current_frame` — `compute_geometry(Center, &wa, (200, 60, 800, 600))` returns `Some((100 + (1800-800)/2, 50 + (1000-600)/2, 800, 600))`. The Center command DOES return a geometry; it uses the current frame's size and the work area's bounds. Fix the doc comment in the `compute_geometry` outline above (Center is not a None case).
2. `center_half_geometry` — `Some((100 + 1800/4, 50, 1800/2, 1000))`.
3. `center_two_thirds_geometry` — `Some((100 + (1800 - 1800*2/3)/2, 50, 1800*2/3, 1000))`.
4. `left_half_geometry` — `Some((100, 50, 1800/2, 1000))`.
5. `right_half_geometry` — `Some((100 + 1800/2, 50, 1800/2, 1000))`.
6. `standard_size_geometry` — centered, 2/3 × 2/3.
7. `minimize_returns_none` — state-toggle command.
8. `toggle_maximize_returns_none`.
9. `toggle_fullscreen_returns_none`.

(Adjustment to the plan above: `Center` is a geometry command. `compute_geometry` returns `Some(...)` for all 6 geometry kinds and `None` only for the 3 state-toggle kinds.)

### `app/core/src/lib.rs` `mod tests`

Add `make_command(kind) -> Command` helper using a fixed work area + frame.

1. `entry_command_reference_round_trips` — every variant; assert `Entry::Command(c).reference() == EntryRef::Command(c.kind.as_id().into())`; round-trip via `resolve`.
2. `resolve_finds_command_by_reference` — mixed entries; resolve a specific kind; cross-variant guard.
3. `entry_ref_command_serializes_to_tagged_json` — exact `r#"{"type":"command","id":"center_half"}"#`.
4. `entry_command_methods_return_command_data` — name, icon, kind for at least Center, ToggleMaximize, and one other.
5. `command_kind_id_round_trips_through_from_id` — `CommandKind::from_id(k.as_id()) == Some(k)` for every variant.

### `app/core/src/matcher.rs` `mod tests`

Add `cmd(kind) -> Entry` helper.

1. `matcher_finds_command_by_name` — entries include `Center`, `CenterHalf`, `LeftHalf`; query `"center"` matches Center AND CenterHalf (both names contain "center"). Query `"left"` matches LeftHalf only. Query `"toggle"` matches both toggles when present.

### `app/core/tests/commands.rs` (optional integration test file)

Skip — no end-to-end value beyond the in-module tests since there's no public-API gap to validate.

### `app/gnome/`

No new tests. `commands::gather_commands`, `windows::get_window_work_area`, `windows::get_window_frame` are all live-D-Bus. Manual verification per the plan's manual-test section.

## Implementation order

1. **Extension** first — XML, service.ts, README. Reinstall via `nix run .#install-extension`, log out / in.
2. `app/core/src/lib.rs` — `Command`, `CommandKind`, `WorkArea`, variants, accessors, helpers, 5 tests.
3. `app/core/src/commands.rs` — pure geometry function + 9 tests.
4. `app/core/src/matcher.rs` — haystack arm + 1 test.
5. `app/core/src/lib.rs` — re-export `compute_geometry` (or the whole `commands` module) at crate root.
6. `cargo test -p lofi-core` clean.
7. `app/gnome/src/windows.rs` — proxy methods + public wrappers for `minimize_window`, `toggle_maximize_window`, `toggle_fullscreen_window`, `move_resize_window`, `get_window_work_area`, `get_window_frame`.
8. `app/gnome/src/commands.rs` — `gather_commands` + LoFi filter.
9. `app/gnome/src/lib.rs` — `pub mod commands;`.
10. `app/gnome/src/launch.rs` — `Entry::Command` arm.
11. `app/gnome/src/main.rs` — gather and extend.
12. `app/gnome/src/ui.rs` — `kind_to_str` arm.
13. `cargo build/test/clippy/fmt --workspace` clean.
14. `nix build` and `nix build .#extension` clean.
15. READMEs.

## Verification

- `cargo test -p lofi-core` — 5 lib + 1 matcher + 9 commands tests pass, plus existing.
- `cargo test --workspace` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `nix build` — clean. `nix build .#extension` — clean (verify `npmDepsHash` doesn't need regen).
- **Manual** (Wayland; extension reinstalled; log out/in):
  - Open Firefox on a workspace, switch to a different app. Launch LoFi, type "left half", press Enter → Firefox? No — the previously-focused user window (the "different app") should snap to the left half.
  - With a window focused, launch LoFi, type "minimize" → window minimizes.
  - Launch LoFi with no windows open (just LoFi itself in the list) → Command entries should NOT appear.
  - Type "fullscreen" → ToggleFullscreen runs, window goes fullscreen.
  - Type "center half" twice → window goes center-half, then stays (idempotent).
  - Maximize a window manually, launch LoFi, type "left half" → window unmaximizes AND snaps to left half (the new `MoveResizeWindow` unmaximize-first behavior).

## Out of scope

- "Move active window to next/previous workspace" launcher commands (the user's earlier deferred request — separate task).
- Per-monitor command variants ("center on monitor 2").
- Configurable keyboard shortcuts (LoFi is a launcher, not a shortcut daemon).
- Animations or visual feedback during the resize.
- Tracking multiple "previously focused" windows (e.g. an Alt+Tab-like UI choosing which window the commands apply to).
- Cleanup of stale `EntryRef::Command(...)` rows in MRU — the set of valid command ids is closed; stale ids never match a current command at resolve time, so they're harmless dead weight.
- Sound effects on activation. (Why would we.)
