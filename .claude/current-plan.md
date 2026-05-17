# Application running indicator + focus-MRU-window activation

## Context

LoFi currently lists Applications (from `.desktop` files) and Windows (from the extension) in one mixed list. Picking an Application always launches a new instance via `gio::DesktopAppInfo::launch()`, even if the app is already running.

The user wants:
1. A small dot under the icon of an Application entry that's already running (visual mirror of the GNOME dock's running-indicator dot).
2. Selecting a running Application focuses its **most recently used** window instead of launching a new instance. Non-running Applications still launch via gio.
3. Window entries continue to appear in the list unchanged.

This needs:
- MRU window ordering from the extension (which currently uses stacking order).
- An app↔window mapping via `Shell.App.get_id()` (the canonical `.desktop`-suffixed id from `Shell.WindowTracker`).
- The `Application` data type to carry a "most-recent-window" identifier when running.
- A branch in `launch::activate` for the Application path.
- A small CSS-styled dot widget in the row, below the icon.

## Confirmed decisions

1. **Additive on the extension side**: new `ListWindowsMRU` method, leave `ListWindows` (stacking order) untouched.
2. **New Window-dict field `app_desktop_id`** (`s`): the canonical desktop id of the app `Shell.WindowTracker` resolves for this window, populated from `Shell.App.get_id()`. Empty if unresolved. Same source as the existing `app_id`/`app_name` fields, separate value (the suffixed-canonical form, suitable for direct equality against `Application.desktop_id`).
3. **`lofi_core::Application` gains `recent_window_id: Option<u64>`**. Runtime-only state, not persisted, not part of `EntryRef`. `is_running` is just `recent_window_id.is_some()`.
4. **`lofi_core::Window` gains `app_desktop_id: Option<String>`** — propagated from the extension, used by the combine step to compute the MRU map.
5. **Rust side uses MRU exclusively** for now: `windows::gather_windows()` switches from calling `list_windows()` to `list_windows_mru()`. We add both proxy methods on the Rust side for completeness, but only the MRU path is currently consumed. Public Rust API surface (`pub fn gather_windows`) is unchanged in signature — only the ordering changes, documented in the function's doc comment.
6. **Combine step** in `main.rs`: after gathering apps + (MRU-ordered) windows, build a `HashMap<String, u64>` keyed by `desktop_id`, populated by walking the windows in order and inserting `(window.app_desktop_id, window.id)` for the **first** occurrence of each app id (later entries skipped). Then set `app.recent_window_id = mru_map.get(&app.desktop_id).copied()` for each Application.
7. **Activation branching** in `launch::activate`:
   - `Entry::Application(app)` with `app.recent_window_id == Some(id)` → `windows::focus_window(id)`.
   - `Entry::Application(app)` with `app.recent_window_id == None` → existing `DesktopAppInfo::launch()` path.
   - `Entry::Window(w)` unchanged.
8. **Visual indicator**: a 6×6 CSS-styled circle directly below the icon, inside a small vertical `gtk::Box` that replaces the bare `gtk::Image` in the row. Hidden via `set_visible(false)` when not running. CSS loaded once per session via `gtk::CssProvider` + `style_context_add_provider_for_display`.

## File-by-file

### Extension

#### `extension/gnome/dbus-interface.xml`
Add one method declaration after `ListWindows`:
```xml
<method name="ListWindowsMRU">
  <arg type="aa{sv}" direction="out" name="windows"/>
</method>
```

#### `extension/gnome/src/windows.ts`
- In `serialize(win)`: derive `appDesktopId` from `Shell.WindowTracker.get_default().get_window_app(win)?.get_id() ?? ''`. Add `app_desktop_id: GLib.Variant.new_string(appDesktopId)` to the dict.
- Refactor `resolveAppInfo(win)` (existing helper) to ALSO return `desktop_id: string` — same `Shell.App` lookup, just one more property.
- Add a `listMRU()` function:
  ```ts
  export function listMRU(): WindowDict[] {
      const display = global.display;
      const list = display.get_tab_list(Meta.TabList.NORMAL_ALL, null);
      return list.filter(w => !w.is_override_redirect()).map(serialize);
  }
  ```

#### `extension/gnome/src/service.ts`
Add a `ListWindowsMRU` method delegating to `windows.listMRU()`.

#### `extension/gnome/README.md`
- Window-dict table: add `app_desktop_id` (`s`) row.
- Methods section: add `ListWindowsMRU() -> aa{sv}` with a one-line note that it's the same shape as `ListWindows` but sorted by most-recently-focused first (the order Alt+Tab cycles through).

### `lofi-core`

#### `app/core/src/lib.rs`
- `Application` adds `pub recent_window_id: Option<u64>`. Update derive list — no new derives needed (`Option<u64>` is `Clone`/`Eq`/etc).
- `Window` adds `pub app_desktop_id: Option<String>`.
- Update existing `make_application` test helper to take an `Option<u64>` for the new field — or simpler, default it to `None` inside the helper and add a sibling `make_application_running(name, desktop_id, icon, window_id)` for the new tests.
- Add one new test in `mod tests`:
  - `entry_application_running_round_trips` — build `Application` with `recent_window_id: Some(42)`, wrap in `Entry`, assert the entry's `name`/`icon`/`kind`/`reference` are unchanged (the new field doesn't affect them; `EntryRef` is still `Application(desktop_id)`).

#### `app/core/src/matcher.rs`
No changes — haystack already uses `name`/`desktop_id` for `Application`, and `name`/`title`/`app_name` for `Window`. The new `recent_window_id` and `app_desktop_id` fields don't enter the haystack.

### `lofi-gnome`

#### `app/gnome/src/windows.rs`
- Add `app_desktop_id: String` field to `DbusWindow`.
- Add `list_windows_mru` method to the `WindowManager` zbus proxy trait — same return type as `list_windows`.
- In `map_dbus_window`, propagate `app_desktop_id` from the dict to `lofi_core::Window.app_desktop_id`, coercing empty string to `None` like the other optional string fields.
- Change `gather_windows()` to call `list_windows_mru` instead of `list_windows`. Update the doc comment to note "MRU order, most recent first".

#### `app/gnome/src/main.rs`
- After `let windows = windows::gather_windows();`, build the MRU map:
  ```rust
  use std::collections::HashMap;
  let mut mru: HashMap<String, u64> = HashMap::new();
  for w in &windows {
      if let Some(ref id) = w.app_desktop_id
          && !mru.contains_key(id)
      {
          mru.insert(id.clone(), w.id);
      }
  }
  ```
  (Use `let-chains` form if 2024 edition allows; otherwise nested `if let`.)
- Set `app.recent_window_id` on each gathered application:
  ```rust
  let mut applications = applications;
  for app in &mut applications {
      app.recent_window_id = mru.get(&app.desktop_id).copied();
  }
  ```
- Then proceed with the existing combine into `Vec<Entry>`.

#### `app/gnome/src/launch.rs`
Update `Entry::Application` arm:
```rust
Entry::Application(app) => {
    if let Some(window_id) = app.recent_window_id {
        windows::focus_window(window_id);
    } else {
        // existing gio launch path, unchanged
    }
}
```

`Entry::Window` arm unchanged.

#### `app/gnome/src/ui.rs`
- At module scope, define a `const RUNNING_DOT_CSS: &str` with the styling:
  ```css
  .running-indicator {
      background-color: alpha(@theme_fg_color, 0.8);
      border-radius: 9999px;
      min-width: 6px;
      min-height: 6px;
  }
  ```
- In `build()` (or a new private helper `install_styles()` called once from `build()`), construct a `gtk::CssProvider`, `load_from_string(RUNNING_DOT_CSS)`, and register via `gtk::style_context_add_provider_for_display` with `gtk::STYLE_PROVIDER_PRIORITY_APPLICATION`. Use `if let Some(display) = gtk::gdk::Display::default()` (no `expect`/`unwrap`).
  - **Guard against duplicate registration**: `build()` is called on every `connect_activate` firing. Either install once at module-init via `std::sync::OnceLock<()>` to gate, or trust that re-registering the same provider is idempotent. The OnceLock approach is cleaner.
- In `build_row(entry)`, replace the current `let image = ...; image.set_pixel_size(ICON_SIZE); hbox.append(&image);` with:
  ```rust
  let icon_column = gtk::Box::builder()
      .orientation(gtk::Orientation::Vertical)
      .spacing(2)
      .valign(gtk::Align::Center)
      .build();
  
  let image = /* same Image-from-icon-or-file logic as before */;
  image.set_pixel_size(ICON_SIZE);
  icon_column.append(&image);
  
  let dot = gtk::Box::builder()
      .orientation(gtk::Orientation::Horizontal)
      .halign(gtk::Align::Center)
      .build();
  dot.add_css_class("running-indicator");
  dot.set_size_request(6, 6);
  let is_running = matches!(entry, Entry::Application(a) if a.recent_window_id.is_some());
  dot.set_visible(is_running);
  icon_column.append(&dot);
  
  hbox.append(&icon_column);
  ```

### READMEs

- `extension/gnome/README.md` — Window-dict table + `ListWindowsMRU` method (covered above).
- `app/core/README.md` — add `recent_window_id` to the Application field list (note it's runtime state, not persisted, set by platform layers); add `app_desktop_id` to the Window field list.
- `app/gnome/README.md` — `windows` module section: note that `gather_windows()` returns MRU-ordered results; document the combine step in `main.rs` that sets `recent_window_id`; note that `launch.rs` branches on `recent_window_id` to focus an existing window instead of launching.
- `app/README.md` — no change.

## Implementation order

1. **Extension** first: add `app_desktop_id` to `serialize`, add `listMRU()`, add `ListWindowsMRU` D-Bus method, update XML. Reinstall via `nix run .#install-extension`, log out / back in.
2. **`lofi-core`**: add the two fields to `Application` and `Window`. Update `make_application` test helper. Add the new test. `cargo test -p lofi-core` clean.
3. **`lofi-gnome::windows.rs`**: add `app_desktop_id` to `DbusWindow`, add `list_windows_mru` proxy method, switch `gather_windows()` to call it, propagate the field.
4. **`lofi-gnome::main.rs`**: build the MRU map, set `recent_window_id` on each Application.
5. **`lofi-gnome::launch.rs`**: branch the `Entry::Application` arm.
6. **`lofi-gnome::ui.rs`**: add the CSS const, the OnceLock-gated style installer, and the dot widget in `build_row`.
7. `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `nix build` — all clean.
8. READMEs.

## Verification

- `cargo test -p lofi-core` — 1 new + existing tests pass.
- `cargo test --workspace` — existing integration tests still pass.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- `nix build` and `nix build .#extension` clean.
- **Manual** (deferred to user; requires Wayland session + extension reinstalled + log out/in):
  - Open Chrome (or any app), then launch LoFi. Verify the dot appears under the Chrome icon.
  - Press Enter on Chrome — verify it focuses the existing Chrome window (and switches workspace if needed) instead of launching a new instance.
  - Close all Chrome windows, re-launch LoFi — verify the dot is gone and pressing Enter launches Chrome via gio.
  - Open two Chrome windows, switch focus to the second one explicitly, then launch LoFi — verify selecting Chrome focuses the second (most recent), not the first.
  - Verify Window entries still appear in the list as before.

## Out of scope

- Persisting MRU across sessions (Mutter's MRU resets on shell restart).
- Modifier keys for "launch a new instance even if running" (Shift+Enter or similar).
- A window count badge next to the dot for apps with multiple windows.
- Right-click / context menu surfaces for windows of an app.
- Hiding the duplicate-feel of Application + Window entries for the same app — they remain separate by design.
- Animations on the dot.
- Using `recent_window_id` in `EntryRef` (still keyed by `desktop_id`; the recent window is runtime-derived, not persisted history).
- Falling back to gio launch if `focus_window(id)` fails (just log and return; the window race is real but rare).
