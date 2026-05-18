# macOS windows slice: enumerate + activate open windows

## Context

The launcher currently lists `.app` bundles only. Window switching is the second-most-used Spotlight/Raycast feature after app launching; we mirror what the GNOME side already does. The Rust core already supports `Entry::Window(Window)`, `EntryRef::Window(u64)`, and the matcher haystack (`"{title} {app_name}"`); `Entry::name()` returns the title, `Entry::reference()` returns `EntryRef::Window(id)`, MRU + the existing four `get_*` accessors all work polymorphically. So this slice is mostly Swift (enumeration + permissions + activation) plus a small FFI extension (push windows, fetch a row's `CGWindowID` for activation lookup).

After this slice: with Screen Recording **and** Accessibility granted, every open application window appears in the list as a `[icon] Title — App` row with category "Window". Enter/click raises that specific window via the AX API; otherwise the existing app-launch path runs.

## Decisions (confirmed with user)

- **`CGWindowListCopyWindowInfo`** for enumeration — Core Graphics, synchronous, simple dict shape, well-tested. Skip ScreenCaptureKit; the async machinery is overkill for a static enumeration.
- **Screen Recording denied → drop windows from the list entirely** (don't show titleless placeholders). Logs once; doesn't nag.
- **Accessibility denied → drop windows from the list entirely** (don't show entries we can't activate). Symmetric to the Screen Recording denial path.
- **Permission requests happen once at first launch**. `CGRequestScreenCaptureAccess()` triggers the system dialog; `AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt: true])` does the same for AX. Both are non-blocking. The user must grant in System Settings and **relaunch LoFi** to see windows (TCC state is captured at process start for these APIs).
- **Window-side state stays Swift-side.** The Rust `Window` struct (`id`, `title`, `app_name`, `icon`, `workspace`, `app_desktop_id`) doesn't have a PID field, and we don't want to add one for a single platform. Swift maintains a `[CGWindowID: (pid_t, title)]` aux map for activation; Rust just stores the cross-platform shape.

## Rust FFI changes — `app/core/src/ffi/entries.rs`

Two new `#[unsafe(no_mangle)] pub unsafe extern "C" fn` symbols:

### `lofi_entries_push_window`

```
bool lofi_entries_push_window(
    EntryList *list,
    uint64_t id,
    const char *title,
    const char *app_name,        // nullable
    const char *icon,            // nullable
    int32_t workspace,
    const char *app_desktop_id   // nullable
);
```

- Null/UTF-8 validation on each provided string; nullable args remain nullable (mapped to `None`).
- Construct `Window { id, title, app_name, icon, workspace, app_desktop_id }`, wrap in `Entry::Window`, call `EntryList::push` (which already clears caches and recomputes the filter — same path as `push_application`).
- Returns `false` on null list, invalid UTF-8, or NULL required arg (`title` is required; the optionals are genuinely optional).

### `lofi_entries_get_window_id`

```
uint64_t lofi_entries_get_window_id(const EntryList *list, uintptr_t idx);
```

- Returns the `CGWindowID` for an `Entry::Window` at the filtered idx; returns `0` for anything else (non-Window variant, null list, OOB index).
- The 0-sentinel is safe because real `CGWindowID`s on macOS are always > 0 for application windows. The Swift caller is expected to gate on `category(at:) == "Window"` before reading; this is just a robustness fallback.
- No caching needed — `u64` round-trips through the FFI by value.

### No changes to existing accessors

- `get_name` already returns the title via `Entry::name()` (lib.rs:302).
- `get_icon` already returns the window's icon identifier or null.
- `get_category` already returns `"Window"` for Window variants.
- `get_bundle_id` currently returns null for non-Application variants — that's fine (Swift doesn't need it for window-row activation).
- MRU bump / apply_mru already polymorphic over Entry variants via `Entry::reference()`.

### `ffi/mod.rs` and `cbindgen.toml`

No structural changes — the new functions land in `entries.rs` and the existing `pub use entries::*;` re-export carries them. cbindgen regenerates the header automatically.

## Rust tests — append to `app/core/tests/ffi.rs`

Six new `#[test]` cases, gated by the existing `#![cfg(feature = "ffi")]`. Re-declare the new C signatures in the existing extern block.

1. **`push_window_round_trips`** — push a window with all fields populated (id=42, title="Untitled — TextEdit", app_name="TextEdit", icon=None, workspace=0, app_desktop_id="com.apple.TextEdit"). Assert `len == 1`, `get_name(0) == "Untitled — TextEdit"`, `get_category(0) == "Window"`, `get_window_id(list, 0) == 42`.
2. **`push_window_with_nil_optionals`** — push with `app_name`/`icon`/`app_desktop_id` all null. Assert success; `get_name`, `get_window_id`, `get_category` still work; `get_icon` returns null.
3. **`push_window_null_title_returns_false`** — null `title` pointer rejected; len stays 0.
4. **`push_window_invalid_utf8_title_returns_false`** — invalid bytes in title; rejected.
5. **`get_window_id_returns_zero_for_application`** — push an Application; assert `get_window_id(list, 0) == 0`. Symmetric: push a Window then an Application; `get_window_id(list, 0) == window_id`, `get_window_id(list, 1) == 0`.
6. **`mixed_list_search_then_window_id`** — push two apps and one window; set_query to a term that matches only the window. Assert `len == 1`, `get_window_id(list, 0)` returns the window's id (filtered-index resolution still routes through to the right underlying entry).

## Swift changes

### `app/macos/Sources/LoFi/Permissions.swift` (new)

```swift
import AppKit
import ApplicationServices

enum Permissions {
    /// `true` when LoFi can read window titles via `CGWindowList…`.
    /// Cached at process start; granting takes effect on next launch.
    static func screenRecording() -> Bool { CGPreflightScreenCaptureAccess() }

    /// Trigger the system Screen Recording prompt. No-op if already granted.
    /// Non-blocking; the dialog opens System Settings.
    static func requestScreenRecording() { _ = CGRequestScreenCaptureAccess() }

    /// `true` when LoFi can drive other processes via AX (read window
    /// titles, raise specific windows). Cached at process start.
    static func accessibility() -> Bool {
        AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeRetainedValue() as String: false] as CFDictionary)
    }

    /// Trigger the Accessibility prompt — passes `prompt = true` so
    /// the system shows a sheet directing the user to System Settings.
    static func requestAccessibility() {
        _ = AXIsProcessTrustedWithOptions([kAXTrustedCheckOptionPrompt.takeRetainedValue() as String: true] as CFDictionary)
    }
}
```

### `app/macos/Sources/LoFi/WindowDiscovery.swift` (new)

```swift
struct DiscoveredWindow {
    let id: CGWindowID
    let title: String
    let ownerName: String        // "Safari", "TextEdit"
    let ownerPid: pid_t
    let ownerBundleId: String?   // "com.apple.Safari" or nil if NSRunningApplication can't resolve
    let workspace: Int32         // always 0 on macOS — Mutter-style workspaces don't exist
}

enum WindowDiscovery {
    /// Returns the user-relevant on-screen windows owned by other
    /// applications. Filters:
    ///   - kCGWindowLayer == 0 (regular app windows, not menus / panels / system UI)
    ///   - kCGWindowOwnerPID != our own PID (don't list LoFi.app)
    ///   - non-empty title (a window with no title is uninteresting in a launcher and
    ///     also a strong signal Screen Recording is denied)
    /// Caller must hold both Screen Recording and Accessibility permissions; this
    /// function does not check — gating belongs in AppDelegate.
    static func discover() -> [DiscoveredWindow]
}
```

Implementation: call `CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)`; cast to `[[String: Any]]`; for each dict filter by layer/owner-pid/title; build `DiscoveredWindow`. Look up `ownerBundleId` via `NSRunningApplication(processIdentifier: pid)?.bundleIdentifier`.

### `app/macos/Sources/LoFi/WindowActivation.swift` (new)

```swift
import AppKit
import ApplicationServices

enum WindowActivation {
    /// Raise the window with the given title belonging to the process at `pid`.
    /// Title-matching is brittle when an app has multiple identically-titled
    /// windows (e.g. "Untitled" in TextEdit) — first match wins. Future slice
    /// can use the private `_AXUIElementGetWindow` API to disambiguate by
    /// CGWindowID. Returns true on success, false if the window can't be
    /// found or AX rejects the call.
    static func raise(pid: pid_t, title: String) -> Bool
}
```

Implementation:
1. `AXUIElementCreateApplication(pid)`
2. `AXUIElementCopyAttributeValue(app, kAXWindowsAttribute, &windows)` → `[AXUIElement]`
3. For each AXWindow, `AXUIElementCopyAttributeValue(window, kAXTitleAttribute, ...)`; on match call `AXUIElementPerformAction(window, kAXRaiseAction)`
4. Also `NSRunningApplication(processIdentifier: pid)?.activate()` to bring the owning app forward (raise alone doesn't switch app focus).

### `app/macos/Sources/LoFi/RustBridge.swift`

`EntryList` gains:

```swift
@discardableResult
func pushWindow(
    id: UInt64,
    title: String,
    appName: String?,
    icon: String?,
    workspace: Int32,
    appDesktopId: String?
) -> Bool

func windowId(at idx: Int) -> UInt64
```

`pushWindow` follows the nested `withCString` pattern from `pushApplication`. Optional strings collapse to `nil` C pointers when `nil` Swift. `windowId(at:)` is a direct call to `lofi_entries_get_window_id(handle, UInt(idx))`.

### `app/macos/Sources/LoFi/AppDelegate.swift`

Property additions:
```swift
private var windowAux: [UInt64: (pid: pid_t, title: String)] = [:]
```

In `applicationDidFinishLaunching` after the existing app push loop, before `applyMru`:

```swift
// Window enumeration is gated on TWO permissions. If either is
// missing, request it (non-blocking prompt) and proceed without
// windows for this session. The user grants in System Settings and
// relaunches LoFi to pick up windows.
let canSeeWindows = Permissions.screenRecording() && Permissions.accessibility()
if canSeeWindows {
    for w in WindowDiscovery.discover() {
        _ = entries.pushWindow(
            id: UInt64(w.id),
            title: w.title,
            appName: w.ownerName,
            icon: w.ownerBundleId,                // bundle path resolved by NSWorkspace at draw time
            workspace: w.workspace,
            appDesktopId: w.ownerBundleId
        )
        windowAux[UInt64(w.id)] = (w.ownerPid, w.title)
    }
} else {
    // Trigger the system dialogs once. The state captured by
    // `CGPreflightScreenCaptureAccess` is set at process start, so
    // the user has to relaunch to pick up a freshly-granted permission.
    if !Permissions.screenRecording() { Permissions.requestScreenRecording() }
    if !Permissions.accessibility() { Permissions.requestAccessibility() }
}
```

Pass the `windowAux` map into `AppListController(entries:mruStore:windowAux:)`.

### `app/macos/Sources/LoFi/AppListController.swift`

Add a stored property `private let windowAux: [UInt64: (pid: pid_t, title: String)]`. Extend the `init` signature.

`launchRow(_ row: Int)` branches:

```swift
private func launchRow(_ row: Int) {
    guard row >= 0, row < entries.count else { return }
    if let store = mruStore {
        entries.bumpMru(store: store, at: row)
    }
    if entries.category(at: row) == "Window" {
        let id = entries.windowId(at: row)
        if let aux = windowAux[id] {
            _ = WindowActivation.raise(pid: aux.pid, title: aux.title)
        }
    } else {
        guard let path = entries.icon(at: row) else { return }
        NSWorkspace.shared.open(URL(fileURLWithPath: path))
    }
    NSApp.terminate(nil)
}
```

MRU bump still happens before activation (same race-rationale as the MRU slice). The `category` string check matches the stable English category names from the FFI.

### `app/macos/Resources/Info.plist`

Add one new key (Screen Recording prompt wording). Accessibility doesn't need an Info.plist key — only an explicit `AXIsProcessTrustedWithOptions` call.

```xml
<key>NSScreenCaptureUsageDescription</key>
<string>LoFi lists your open windows so you can switch to one. Window titles are only readable with Screen Recording access.</string>
```

## Permission UX flow

| State | What the user sees |
|---|---|
| Cold first launch | Panel appears with apps only. System dialog prompts for Screen Recording. After grant + relaunch, dialog prompts for Accessibility (or both at once depending on order). |
| Both granted | Panel has apps + windows; activation works. |
| Either denied | Panel has apps only; no nag dialogs on subsequent launches (TCC remembers the denial). User can flip the toggles in System Settings → Privacy & Security and relaunch. |

## Critical files

**Modify (Rust):**
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/entries.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/tests/ffi.rs`

**Create (Swift):**
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/Permissions.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/WindowDiscovery.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/WindowActivation.swift`

**Modify (Swift):**
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/RustBridge.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppDelegate.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Sources/LoFi/AppListController.swift`
- `/Users/jplein/Git/jplein/lofi/app/macos/Resources/Info.plist`

**READMEs (technical-writer pass):**
- `/Users/jplein/Git/jplein/lofi/app/macos/README.md` — drop "Window / workspace / power commands" from out-of-scope (partial — windows land here, workspaces/power deferred); add a Permissions section; add a Gotcha about TCC state captured at process start.
- `/Users/jplein/Git/jplein/lofi/app/core/README.md` — FFI count 13 → 15.

## Verification

1. `bazel test //app/core:ffi_test` — old 32 cases plus 6 new = 38 pass.
2. `bazel build //app/macos:LoFi` — succeeds.
3. `bazel run //app/macos:launch` on a Mac with **neither** permission yet — panel appears with apps only; system dialogs prompt for Screen Recording and Accessibility.
4. Grant both, relaunch — `bazel run //app/macos:launch` shows windows mixed in with apps. Title row shows window title, category column shows "Window".
5. Type a window title fragment; the list filters down. Enter raises the window and quits LoFi.
6. Verify MRU: launch, pick a window, relaunch — that window's row is at top.

## Risks / gotchas

1. **TCC state captured at process start**: `CGPreflightScreenCaptureAccess` and `AXIsProcessTrusted...` reflect the state at process launch. Granting permission while LoFi is running won't enable windows for that session — the user has to relaunch. Document in the macOS README.
2. **Title matching for AX raise** is brittle when an app has multiple windows with identical titles ("Untitled — TextEdit"). First match wins. Acceptable for v1; revisit with `_AXUIElementGetWindow` private API or a more robust matching scheme if it bites.
3. **`NSScreenCaptureUsageDescription`** must be in `Info.plist` *before* the first call to a Screen-Recording-gated API, otherwise the prompt's wording defaults to a generic message. Add the key before any code changes that call into `CGRequestScreenCaptureAccess`.
4. **`NSRunningApplication(processIdentifier:)`** can return nil (e.g. for system processes), so `ownerBundleId` is rightly `String?`. Window entries with nil bundle ID still appear in the list — the activation path uses pid + title, not bundle ID.
5. **0 is a valid `usize` but not a valid `CGWindowID`** for app windows; using 0 as the "not a window" sentinel from `lofi_entries_get_window_id` is robust. Document.
6. **AX raise without `NSRunningApplication.activate()`** raises the window in z-order but doesn't switch the focused app — the user sees their target window's chrome above other windows but keyboard input still goes to their previous app. Call `.activate()` on the `NSRunningApplication` after `AXRaise`.

## Workflow status

- [x] Plan written
- [x] Test-writer pass — 6 new FFI tests appended (`push_window_*`, `get_window_id_returns_zero_for_application`, `mixed_list_search_then_window_id`)
- [x] Coder pass 1 — Rust FFI symbols added; three new Swift files created (`Permissions.swift`, `WindowDiscovery.swift`, `WindowActivation.swift`); first attempt timed out before integration
- [x] Coder pass 2 (continuation) — `RustBridge.swift`, `AppDelegate.swift`, `AppListController.swift`, `Info.plist` integration; `bazel test //app/core:ffi_test` 38/38 ✅, `bazel build //app/macos:LoFi` ✅
- [x] Reviewer pass — approved with two MINORs (both originated from this plan's wording, not coder bugs):
  - Window icon row would render generic doc icon (plan said pass `ownerBundleId` to the icon slot, which is a bundle identifier not a path)
  - `kAXTrustedCheckOptionPrompt.takeRetainedValue()` should be `.takeUnretainedValue()` per Apple's `extern const CFStringRef` convention
- [x] Coder pass 3 (fix) — `WindowDiscovery.swift` gains `ownerBundlePath: String?` from `NSRunningApplication.bundleURL?.path`; AppDelegate now pushes `w.ownerBundlePath` into `icon:`; both `Permissions.swift` AX call sites switched to `.takeUnretainedValue()`. Tests still 38/38, build still green.
- [x] Technical-writer pass — `app/macos/README.md` gains a Permissions section + two new gotchas (TCC-state-at-process-start, AX title-matching brittleness); `app/core/README.md` FFI count 13 → 15, function enumeration extended, borrow contract clarifies `get_window_id` is exempt (returns `u64` by value).

Outstanding (non-blocking, flagged by technical-writer):
- macOS README line 22 still says "Swift only reads it back through `lofi_entries_len` / `lofi_entries_get_name`" — was stale after the search slice already. A future doc pass should genericize to "the `lofi_entries_get_*` accessors."
