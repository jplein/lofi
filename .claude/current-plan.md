# macOS commands slice: window-action commands on the Mac side

## Status: COMPLETE ✅ (+ field fixes from manual testing)

## Field fixes after manual smoke testing (post-review)

Manual testing on a real Mac (Ghostty + Chrome) surfaced three runtime issues the
build/FFI tests couldn't catch; all fixed Swift-side (FFI surface unchanged):

1. **Commands no-op'd on Ghostty** — AX window was found by *title*, but terminals
   retitle between gather and activation. Fixed: match by exact `CGWindowID` via the
   private `_AXUIElementGetWindow` (`dlsym`-resolved, title fallback) in
   `AXWindowFinder.match`; used by both commands and `raise`. (This is the
   `_AXUIElementGetWindow` work the macOS README gotcha 11 had marked "out of scope".)
2. **Commands hit the wrong window (another Space)** — the command target came from the
   all-Spaces, unordered window list. Fixed: `gatherTarget` uses
   `WindowDiscovery.discover(onScreenOnly: true)` (current Space, front-to-back z-order);
   the switcher list still spans all Spaces.
3. **Resized-or-moved-but-not-both** — `AXEnhancedUserInterface` (kicked on for Firefox
   enumeration) makes geometry async so only the last set survives. Fixed:
   `WindowControl.disableEnhancedUI` before geometry + a `size → position → size` pass in
   `setFrame`.

Temporary `/tmp/lofi-wm.log` diagnostics added during debugging were removed once
confirmed working. Docs updated: `app/macos/README.md` gotchas 11/12/19/20 +
`WindowActivation.swift`/`WindowControl.swift` header comments. Verified: 49 FFI tests
pass, macOS build green.

## Orchestrator decision on coder's test recommendations

The coder recommended optional Swift unit tests (`SavedFrameStore`, coordinate-flip math).
**Decision: NOT adding Swift unit tests in this slice.** This repo has no Swift test target;
the macOS Swift layer is exercised manually by convention (per the READMEs), and all pure
cross-platform logic lives in `lofi-core` where it is tested (49 FFI tests). A Swift XCTest
harness is net-new build infrastructure beyond this feature's scope. Recorded as a future
follow-up: if a `macos_unit_test` target is later added, cover `SavedFrameStore`
(save/take/prune/malformed) and the extracted flip math.


## Problem statement

The GNOME version of LoFi surfaces 9 window-action commands (lofi-core `CommandKind`):
Center, CenterHalf, CenterTwoThirds, LeftHalf, RightHalf, StandardSize, Minimize,
ToggleMaximize, ToggleFullscreen. On GNOME these are gathered into `Entry::Command`
targeting the previously-focused non-LoFi window (with its work area + current frame),
and `launch::activate` dispatches geometry kinds through `lofi_core::compute_geometry`
-> `MoveResizeWindow`, and the 3 state kinds through dedicated D-Bus methods.

The macOS frontend (`app/macos/`, Swift + AppKit on lofi-core via C ABI) currently
only pushes Application and Window entries — it has NO command support. The shared
`CommandKind`/`Command`/`compute_geometry` already exist in lofi-core. We must add the
FFI to push/read Command entries and the Swift Accessibility (AX) code to manipulate
windows, to reach parity with GNOME.

## Scope

1. **Rust FFI** (`app/core/src/ffi/entries.rs`):
   - `lofi_entries_push_command` (kind_id + target_window_id + work_area x/y/w/h +
     current_frame x/y/w/h, mirroring the `Command` struct; validate kind_id via
     `CommandKind::from_id`).
   - `lofi_entries_get_command_id` (returns `kind.as_id()` for Command entries, null
     otherwise).
   - A way for Swift to obtain computed geometry for geometry kinds and distinguish
     state-toggle kinds (e.g. `lofi_entries_get_command_geometry(list, idx,
     out_x,out_y,out_w,out_h)->bool` calling `compute_geometry`). Keep
     `compute_geometry` the single source of geometry truth.

2. **macOS Swift**: gather the target window (most-recently-focused non-LoFi window),
   capture current frame (CGWindow bounds, top-left global coords) and the screen's
   work area (NSScreen.visibleFrame converted to top-left global coords), push the 9
   Command entries. On activation: geometry kinds -> AX set kAXPosition + kAXSize
   (unfullscreen/unzoom first); state kinds -> Minimize / ToggleFullscreen /
   ToggleMaximize.

3. Command rows appear only when there's a usable non-LoFi target window. Commands are
   MRU-tracked / fuzzy-matched like apps/windows (`EntryRef::Command(kind.as_id())`).

4. Render command rows with an SF Symbol icon per command kind.

## Product decisions (already made — do NOT re-litigate)

- **Toggle maximize**: FILL WORK AREA semantics. Resize to visible frame if not already
  filling it; otherwise restore toward a standard centered size. A toggle, identical for
  every app, independent of per-app zoom quirks. Document the divergence from GNOME's
  Mutter maximize.
- **Command icons**: SF Symbols per kind.

## Key technical constraint: coordinate systems

macOS AX (`kAXPositionAttribute`) and CGWindow bounds use **top-left global display
coordinates** (y down, origin at primary display top-left). `NSScreen.frame`/`visibleFrame`
use **Cocoa bottom-left** (y up). The work area passed to `compute_geometry` and the rect
applied via AX must be in the SAME (top-left) coordinate space, so `NSScreen.visibleFrame`
must be flipped using the primary screen height. Document the flip and why.

## Permissions

Window control needs Accessibility (already gated via `Permissions.accessibility()`);
Screen Recording is needed to read titles/bounds. Reuse the existing `canSeeWindows` gate
in `AppDelegate`.

## Build / test commands (Bazel, NOT cargo)

- `bazelisk test //app/core:ffi_test` — Rust FFI integration tests (add new tests here
  in the existing extern-C style).
- `bazelisk build //app/macos:LoFi` — Swift app build must stay green.
Baseline: both currently pass (38 FFI tests today).

## Documentation (READMEs are source of truth per CLAUDE.md)

Keep in sync: `app/core/README.md` (FFI surface + function count + new symbols),
`app/macos/README.md` (new window-control file, command support, AX gotchas, coordinate
flip, out-of-scope cleanup), `app/README.md` and top-level `README.md` if their feature
lists need updating. Document WHY, not just how.

## Files to study

- `app/core/src/lib.rs` (CommandKind/Command/WorkArea/Entry)
- `app/core/src/commands.rs` (compute_geometry)
- `app/core/src/ffi/entries.rs`, `app/core/tests/ffi.rs`
- `app/macos/Sources/LoFi/{AppDelegate,AppListController,WindowDiscovery,WindowActivation,RustBridge,Permissions}.swift`
- `app/gnome/src/{commands,windows,launch}.rs`, `extension/gnome/src/service.ts`

---

## Architect's plan

### 1. Summary

Add the nine GNOME-style window-action commands to the macOS frontend. The shared
`CommandKind`/`Command`/`WorkArea`/`compute_geometry` already live in `lofi-core`; the
macOS side has no command support yet. Add three C-ABI symbols to
`app/core/src/ffi/entries.rs` (`lofi_entries_push_command`, `lofi_entries_get_command_id`,
`lofi_entries_get_command_geometry`), then build the Swift side: AX window manipulation
(move/resize/minimize/toggle-fullscreen/toggle-maximize), command-target gathering (first
non-LoFi window + its current frame + its screen's work area, all in top-left global
coords), `RustBridge` wrappers, push at startup, an activation branch in
`AppListController`, and per-kind SF Symbol rendering. The single hard constraint is the
coordinate flip: AX and CGWindow bounds are top-left global; `NSScreen.visibleFrame` is
Cocoa bottom-left, so the work area must be flipped before it reaches `compute_geometry`,
whose pure arithmetic then yields a top-left rect usable directly as an AX position/size.

### 2. Files to create

**2a. `app/macos/Sources/LoFi/WindowControl.swift`** — AX manipulation layer. `enum
WindowControl` (namespace), mirroring `WindowActivation`'s shape.

Shared helper extraction (recommended): add `enum AXWindowFinder` in this file exposing
`static func windowsForApp(pid: pid_t) -> [AXUIElement]` (kicks `AXEnhancedUserInterface`
then copies `kAXWindowsAttribute`) and `static func find(pid:title:) -> AXUIElement?`
(windowsForApp + `titleMatches` loop). Move `titleMatches` here as `internal` so both
files share the one brittle matcher. `WindowActivation.raise` then uses `windowsForApp`
for its empty-check + `running.activate()` fallback, and the `titleMatches` loop (or
`find`) for the match — preserving the existing empty-vs-no-match behavior.

Functions in `WindowControl`:
- `static func move(pid:title:x:y:width:height:) -> Bool` — find AX window (nil ⇒ false).
  **Clear fullscreen first** (read `"AXFullScreen"`; if true set false) so the resize
  takes effect (mirrors GNOME's MoveResizeWindow unmaximize/unfullscreen; setting explicit
  position+size de-zooms in practice — there is no public "is zoomed" attribute). Then set
  **position then size**: `var p = CGPoint(...); AXValueCreate(.cgPoint, &p)` →
  `AXUIElementSetAttributeValue(win, kAXPositionAttribute as CFString, v!)`; `var s =
  CGSize(...); AXValueCreate(.cgSize, &s)` → `kAXSizeAttribute`. x/y/w/h arrive as the
  top-left-global rect from `compute_geometry` (already correct space). Return true iff
  both sets `.success`.
- `static func minimize(pid:title:) -> Bool` — set `kAXMinimizedAttribute` to
  `true as CFTypeRef`.
- `static func toggleFullscreen(pid:title:) -> Bool` — read `"AXFullScreen"` (undeclared
  bare CFString, same pattern as `AXEnhancedUserInterface`), coerce via `(value as? Bool)`,
  set the negation.
- `static func toggleMaximize(pid:title:windowId:workArea:fallbackRect:store:) -> Bool` —
  TRUE TOGGLE with previous-size restore (USER DECISION, supersedes the earlier
  fill↔StandardSize plan; diverges from GNOME only in *mechanism* — GNOME gets the saved
  frame from Mutter, we persist it ourselves because LoFi is short-lived). Read current
  top-left-global rect via AX (`kAXPositionAttribute`+`kAXSizeAttribute`, unwrap with
  `AXValueGetValue` into CGPoint/CGSize). Discriminator = "approximately fills the work
  area" iff all four edges within `kMaximizeFillTolerance = 2` points of `workArea`.
    - If NOT filling ⇒ **maximize**: `store.save(windowId: windowId, frame: currentFrame)`
      (overwriting any prior saved frame for this id — it tracks the size right before the
      most recent maximize), then move/resize to `workArea` (clear fullscreen first, like
      `move`).
    - If filling ⇒ **un-maximize**: `let restore = store.take(windowId: windowId) ??
      fallbackRect`; move/resize to `restore`. `take` reads-and-removes so the saved entry
      lifecycle is save-on-maximize → consume-on-restore. `fallbackRect` (the StandardSize
      rect, single-sourced from Rust `compute_geometry`) is used ONLY when there is no saved
      frame (e.g. the window was already maximized by other means before LoFi ever saw it).
  All rects top-left global. The geometry discriminator (not "does a saved frame exist")
  is deliberate: if the user manually shrinks a LoFi-maximized window and presses again,
  it's no longer filling, so we maximize again (and overwrite the saved frame) — the
  intuitive result.

AX wrapping notes for the coder: position via `AXValueCreate(.cgPoint,&p)`; size via
`AXValueCreate(.cgSize,&s)`; read via `AXUIElementCopyAttributeValue` then
`AXValueGetValue(out as! AXValue, .cgPoint/.cgSize, &dst)`. `AXValueType` Swift cases are
`.cgPoint`, `.cgSize`. Fullscreen attr is the bare string `"AXFullScreen"`.

**2b. `app/macos/Sources/LoFi/WindowCommands.swift`** — command-target gathering (macOS
analog of GNOME `gather_commands`). `enum WindowCommands`.

```
struct CommandTarget {
    let windowId: UInt64      // CGWindowID of target window
    let pid: pid_t            // owning process (AX dispatch)
    let title: String         // AX title matching
    let workArea: CGRect      // top-left global, visibleFrame flipped
    let currentFrame: CGRect  // top-left global, from kCGWindowBounds
    var standardRect: CGRect  // StandardSize restore rect (filled post-push; see 3e/3f)
}
```
- `static func gatherTarget() -> CommandTarget?` — call `WindowDiscovery.discover()`
  (already excludes LoFi via pid==getpid()). Pick the **first** entry (frontmost non-LoFi
  window; CGWindowList returns front-to-back — the analog of GNOME's first non-LoFi
  `ListWindowsMRU` entry). Belt-and-suspenders: also skip `ownerBundleId ==
  "dev.jplein.lofi"`. current frame = the window's `bounds` (from `kCGWindowBounds`,
  already top-left global). work area = `workAreaTopLeft(forWindowBounds:)`. Return nil if
  no non-LoFi window or no screen ⇒ no command rows (GNOME parity). `standardRect` is
  filled later (a placeholder at gather time, set in AppDelegate post-push — see 3f).
- private `static func workAreaTopLeft(forWindowBounds bounds: CGRect) -> CGRect?` —
  `guard let primary = NSScreen.screens.first else { return nil }`. Pick the target screen
  by **center-containment** (the NSScreen whose flipped frame contains the window's
  center), falling back to `NSScreen.main` then `primary` for off-screen windows. Take
  `screen.visibleFrame` (Cocoa, already excludes menu bar+Dock), flip to top-left global
  (formula in §8), return CGRect.

Document at top: WHY first-non-LoFi (GNOME parity), WHY top-left everywhere
(`compute_geometry` is coordinate-agnostic, so a top-left work area yields top-left rects),
empty-result behavior.

**2c. `app/macos/Sources/LoFi/SavedFrameStore.swift`** (new) — persists each window's
pre-maximize frame so `toggleMaximize` can restore the exact previous size across LoFi
runs (LoFi quits after each activation, so in-memory state can't survive). macOS-specific
(CGWindowID + the divergent toggle policy), so it stays out of the platform-clean
lofi-core — do NOT put this in the Rust core or the SQLite MRU store.

```
final class SavedFrameStore {
    func save(windowId: UInt64, frame: CGRect)
    func take(windowId: UInt64) -> CGRect?      // read-and-remove
    func prune(liveWindowIds: Set<UInt64>)      // drop entries for ids no longer present
}
```
- Backed by **`UserDefaults.standard`** under a single key (e.g. `"savedFrames"`) holding a
  `[String: [Double]]` dictionary: `String(windowId)` → `[x, y, w, h]` (top-left global).
  UserDefaults is the idiomatic small-state store for a bundled macOS app and needs no file
  I/O / serialization boilerplate; it works for an `LSUIElement` background app.
- `save` overwrites the entry for that id. `take` returns the CGRect and removes the entry
  (write-back the pruned dict). Malformed entries (wrong array length) are ignored/treated
  as absent — same bad-row tolerance as the MRU store.
- `prune(liveWindowIds:)` removes saved entries whose id is not in the current live set,
  bounding accumulation from windows closed while still maximized. Called once at startup
  from `AppDelegate` (which already has the live id set from `WindowDiscovery.discover()`).
- Document at top: WHY persistence is needed (short-lived process), WHY UserDefaults (small
  key-value macOS state), WHY keyed by CGWindowID (session-stable for a window's lifetime),
  and the staleness tolerance (CGWindowID can be reused after a window closes; the
  save→consume lifecycle + startup prune keep the window of risk tiny, and a single
  wrong-size restore is a benign worst case).

### 3. Files to modify

**3a. `app/core/src/ffi/entries.rs`** — three FFI fns + a `command_id_cstr` helper.
- Imports: add `Command`, `CommandKind`, `WorkArea` to the `use crate::{...}`; add
  `use crate::compute_geometry;` (re-exported at crate root).
- **`command_id_cstr`**: use `c"..."` C-string literals (toolchain is Rust 1.95, so `c"..."`
  is available) returning `&'static CStr` per kind, exhaustively matched over `CommandKind`
  (no `_` arm), strings EXACTLY equal to `CommandKind::as_id()` (doc-comment the
  cross-reference). No new cache field, no RefCell — the returned pointer is process-lifetime
  and never dangles. Fallback if `c"..."` somehow unavailable: `CStr::from_bytes_with_nul(b"center\0").unwrap()`.
- **`lofi_entries_push_command(list, kind_id: *const c_char, target_window_id: u64, wa_x,
  wa_y, wa_w, wa_h: i32, frame_x, frame_y, frame_w, frame_h: i32) -> bool`**: reject null
  list / null kind_id (false); `CStr::from_ptr(kind_id).to_str()` reject invalid UTF-8;
  `CommandKind::from_id(s)` None ⇒ reject (unknown id, no push); build `Command { kind,
  target_window_id, work_area: WorkArea{...}, current_frame: (frame_x,frame_y,frame_w,frame_h) }`
  and `list_ref.push(Entry::Command(cmd))` (push already clears caches + recomputes filter).
  Standard `# Safety` block + invalidation note.
- **`lofi_entries_get_command_id(list, idx) -> *const c_char`**: null list ⇒ null; use
  `resolve_filtered_index(idx)` (OOB ⇒ null); match `Entry::Command(c) ⇒
  command_id_cstr(c.kind).as_ptr()`, every other variant ⇒ null (exhaustive). Doc: returns
  `CommandKind::as_id` for Command, null otherwise; pointer is process-lifetime `&'static
  CStr` (still copy on the Swift side for uniformity).
- **`lofi_entries_get_command_geometry(list, idx, out_x, out_y, out_w, out_h: *mut i32) ->
  bool`**: defensively guard null out-pointers (return false). null list / OOB / non-Command
  / state-toggle kind ⇒ false **with out-params left untouched** (documented contract).
  `resolve_filtered_index`; match `Entry::Command(c)` ⇒ `compute_geometry(c.kind,
  &c.work_area, c.current_frame)`: `Some((x,y,w,h))` ⇒ write the four outs (SAFETY comment,
  non-null guard) + return true; `None` ⇒ false. Exempt from the borrow contract (by value).
- No change to `get_name`/`get_category`/`get_icon`/`apply_mru`/`bump_entry` — already
  polymorphic over `Entry::Command` (`get_icon` already groups Command into the None arm;
  `Entry::reference()` → `EntryRef::Command(as_id)`). State this; do not edit.
- Update the module-level borrow-contract doc to note `get_command_id` returns a
  process-lifetime pointer (not invalidated by mutations) and `get_command_geometry` is
  by-value/out-param (exempt), like `get_window_id`.

**3b. `app/macos/Sources/LoFi/RustBridge.swift`** — three wrappers. Keep this file
CoreGraphics-free: `pushCommand` takes plain Int32 params (caller does CGRect→Int32):
- `@discardableResult func pushCommand(kindId: String, targetWindowId: UInt64, waX: Int32,
  waY: Int32, waW: Int32, waH: Int32, frameX: Int32, frameY: Int32, frameW: Int32, frameH:
  Int32) -> Bool` — `kindId.withCString { lofi_entries_push_command(handle, $0,
  targetWindowId, waX, ...) }`.
- `func commandId(at idx: Int) -> String?` — copy-into-String like `name(at:)`.
- `func commandGeometry(at idx: Int) -> (x: Int32, y: Int32, w: Int32, h: Int32)?` — four
  local `Int32` outs, `lofi_entries_get_command_geometry(handle, UInt(idx), &x,&y,&w,&h)`;
  nil on false (state-toggle/non-Command), tuple on true. Doc: nil ⇒ "dispatch by
  commandId instead."

**3c. `app/macos/Sources/LoFi/WindowDiscovery.swift`** — add `let bounds: CGRect` to
`DiscoveredWindow` (from `kCGWindowBounds`, already top-left global — no flip). In
`discover()`: `guard let boundsValue = dict[kCGWindowBounds as String], let rect =
CGRect(dictionaryRepresentation: boundsValue as! CFDictionary) else { continue }`; pass
`bounds: rect` to the initializer. Skip the window if bounds can't be read (consistent
with the existing skip-on-missing-field pattern).

**3d. `app/macos/Sources/LoFi/WindowActivation.swift`** — refactor `raise` to use the
shared `AXWindowFinder`/`titleMatches` from `WindowControl.swift` (minimal extraction; keep
the empty-array `running.activate()` fallback in `raise`). Comment pointing at the shared
finder.

**3e. `app/macos/Sources/LoFi/AppDelegate.swift`** — add `private var commandTarget:
WindowCommands.CommandTarget?` and `private let savedFrameStore = SavedFrameStore()` (held
for the process lifetime, like `mruStore`, so the toggle-maximize save/restore is consistent
between gather and activation). Inside the existing `if canSeeWindows { ... }` branch:
prune stale saved frames once using the live id set from the window enumeration —
`savedFrameStore.prune(liveWindowIds: Set(discoveredWindows.map { UInt64($0.id) }))` (capture
the discovered list in a local rather than re-enumerating). Then, AFTER the window push loop
and BEFORE `applyMru`:
1. `var target = WindowCommands.gatherTarget()`.
2. If non-nil, push the nine commands in display order using a Swift constant id array
   `["center","center_half","center_two_thirds","left_half","right_half","standard_size",
   "minimize","toggle_maximize","toggle_fullscreen"]` (matching `CommandKind::as_id`), each
   via `entries.pushCommand(kindId: id, targetWindowId: target.windowId, waX:
   Int32(target.workArea.minX.rounded()), ... frameX: Int32(target.currentFrame.minX.rounded()),
   ...)`.
3. Fill `target.standardRect` (see 3f), assign `self.commandTarget = target`.
Confirm `applyMru` runs AFTER the command push so commands participate in MRU. Thread the
whole `CommandTarget?` AND the `savedFrameStore` into the new
`AppListController.init(...commandTarget:savedFrameStore:)`.

**3f. StandardSize fallback rect single-sourcing (ORCHESTRATOR DECISION — locked):** Do NOT
add a stateless geometry FFI in this slice. After the nine `pushCommand` calls and BEFORE
`applyMru`/any query (push order, no filter), scan rows for `commandId(at:) ==
"standard_size"` and read its `commandGeometry(at:)`; store that rect (as a CGRect, top-left
global) into `target.standardRect`. This reuses Rust's `compute_geometry` (the pushed
standard_size entry already holds the computed rect) so Swift never duplicates the 2/3
math. Scan **by id** (not a fixed index) so it is robust to ordering. `toggleMaximize` uses
`commandTarget.standardRect` ONLY as the fallback when no saved previous frame exists for
the target window (the normal restore path uses the persisted previous frame from
`SavedFrameStore` — see §2c and §9).

**3g. `app/macos/Sources/LoFi/AppListController.swift`** —
1. Add `private let commandTarget: WindowCommands.CommandTarget?` + `private let
   savedFrameStore: SavedFrameStore` properties and init parameters; update
   `init(entries:mruStore:windowAux:)` →
   `init(entries:mruStore:windowAux:commandTarget:savedFrameStore:)`.
2. `launchRow(_:)` new arm for `category == "Command"` (alongside the existing `"Window"`
   arm; MRU bump at the top already covers commands via `EntryRef::Command`):
   - `guard let commandId = entries.commandId(at: row) else { ... terminate }`.
   - `guard let target = commandTarget else { ... terminate }`.
   - If `let geo = entries.commandGeometry(at: row)` ⇒ geometry kind: `_ =
     WindowControl.move(pid: target.pid, title: target.title, x: geo.x, y: geo.y, width:
     geo.w, height: geo.h)`.
   - Else (nil ⇒ state toggle) dispatch by `commandId`: `"minimize"` ⇒
     `WindowControl.minimize`; `"toggle_fullscreen"` ⇒ `WindowControl.toggleFullscreen`;
     `"toggle_maximize"` ⇒ `WindowControl.toggleMaximize(pid: target.pid, title:
     target.title, windowId: target.windowId, workArea: target.workArea, fallbackRect:
     target.standardRect, store: savedFrameStore)`.
   - Then `NSApp.terminate(nil)` (unchanged).
3. SF Symbols for command rows (since `get_icon` is null for Command): a Swift
   `commandSymbolName(for id: String) -> String` map:
   - `center` → `"rectangle.center.inset.filled"`
   - `center_half` → `"rectangle.split.2x1"`
   - `center_two_thirds` → `"rectangle.split.3x1"`
   - `left_half` → `"rectangle.lefthalf.filled"`
   - `right_half` → `"rectangle.righthalf.filled"`
   - `standard_size` → `"rectangle.inset.filled"`
   - `minimize` → `"minus.rectangle"`
   - `toggle_maximize` → `"arrow.up.left.and.arrow.down.right"`
   - `toggle_fullscreen` → `"arrow.up.left.and.arrow.down.right.rectangle"`
   - unknown → `"macwindow"` (defensive fallback).
   Extend `EntryRowView.init` → `init(name:category:iconPath:symbolName:)`. Body: iconPath
   non-nil ⇒ existing `NSWorkspace.shared.icon(forFile:)`; iconPath nil + symbolName
   non-nil ⇒ `imageView.image = NSImage(systemSymbolName: symbolName,
   accessibilityDescription: nil)` with a symbolConfiguration (size ~`kSearchGlyphSize` or
   a new const) tinted `.secondaryLabelColor`; both nil ⇒ no image. Keep the 24×24 box. In
   `tableView(_:viewFor:row:)`: `let symbolName: String? = (category == "Command") ?
   commandSymbolName(for: entries.commandId(at: row) ?? "") : nil`; pass to the row view.
4. No other Swift changes — commands flow through `count`/`name(at:)`/`category(at:)`/
   `setQuery`/`bumpMru` like any entry (matcher haystack for Command is `display_name`).

### 4. Rust FFI tests — `app/core/tests/ffi.rs`

Add the three extern-C decls (signatures per 3a). Add helpers mirroring the existing style:
`push_command_kind(list, kind_id: &str, target_window_id, wa: (i32,i32,i32,i32), frame:
(i32,i32,i32,i32)) -> bool`; `command_id_at(list, idx) -> String`; `command_geometry_at(list,
idx) -> Option<(i32,i32,i32,i32)>` (init outs to a sentinel, return Some on true / None on
false).

Use known constants reused from `commands.rs` unit tests: `WA = (x:100, y:50, w:1800,
h:1000)`, `FRAME = (200, 60, 800, 600)`. Expected geometry from `compute_geometry`:
`center ⇒ (600,250,800,600)`, `center_half ⇒ (550,50,900,1000)`, `center_two_thirds ⇒
(400,50,1200,1000)`, `left_half ⇒ (100,50,900,1000)`, `right_half ⇒ (1000,50,900,1000)`,
`standard_size ⇒ (400,217,1200,666)`.

Tests:
1. `push_command_round_trips_geometry_kind` — push `center_half` w/ WA+FRAME; len==1;
   name=="Center half"; category=="Command"; command_id=="center_half"; geometry==Some((550,
   50,900,1000)); `get_icon`==null; `get_window_id`==0; `get_bundle_id`==null.
2. `push_command_center_uses_current_frame` — push `center`; geometry==Some((600,250,800,600))
   (proves current_frame plumbed through).
3. `command_geometry_false_for_state_toggle_kinds` — push `minimize`,`toggle_maximize`,
   `toggle_fullscreen` (idx 0/1/2); each geometry==None but command_id + name still surface.
4. `command_geometry_leaves_out_params_untouched_on_false` — push `minimize`; pre-fill outs
   with `-12345`; raw call returns false AND all four outs unchanged. Also exercise
   non-Command idx, OOB idx, null list (outs untouched / false).
5. `push_command_unknown_kind_id_rejected` — `"not_a_command"` ⇒ false; len==0.
6. `push_command_null_kind_id_returns_false` — null kind_id ⇒ false; len==0; null list ⇒ false.
7. `push_command_invalid_utf8_kind_id_returns_false` — bytes `[0xFF,0x00]` ⇒ false; len==0.
8. `get_command_id_null_for_non_command` — push app+window; command_id null for both; OOB
   null; null list null.
9. `command_geometry_false_for_non_command` — push app; geometry==None (may fold into #4).
10. `mixed_list_filtered_command_geometry_and_id` — push two apps (names without a
    `l-e-f-t` subsequence, e.g. "Chrome"/"Notes") + one `left_half` command; `set_query("left")`
    ⇒ len==1; command_id=="left_half"; geometry==Some((100,50,900,1000)); category=="Command".
11. (recommended) `command_id_matches_as_id_for_all_kinds` — push all nine; assert each
    command_id equals the expected snake_case id (guards `command_id_cstr` against drift).

Keep the `#![cfg(feature="ffi")]` gate, `extern crate lofi_core as _;`, and the existing
helper/style.

### 5. Implementation order (avoid compile breaks)

1. Rust FFI (`entries.rs`) — imports, `command_id_cstr`, three fns.
2. Rust FFI tests (`tests/ffi.rs`) → `bazelisk test //app/core:ffi_test` green before Swift.
3. `WindowDiscovery.swift` — `bounds` field + `kCGWindowBounds` read (additive).
4. `WindowControl.swift` (new) — AX helpers + `AXWindowFinder` (+ move `titleMatches` here).
5. `WindowActivation.swift` — refactor `raise` to use `AXWindowFinder`.
6. `WindowCommands.swift` (new) — `CommandTarget` + `gatherTarget()` + flip helper.
7. `SavedFrameStore.swift` (new) — UserDefaults-backed save/take/prune.
8. `RustBridge.swift` — three wrappers (needs regenerated header from step 1).
9. `AppListController.swift` — command branch (incl. toggle-maximize via store) + SF Symbols
   + `EntryRowView` symbol param + init params (`commandTarget`, `savedFrameStore`).
10. `AppDelegate.swift` — own `savedFrameStore`, prune, gather + push nine + fill
    fallback standardRect + thread target & store (last, so the new init exists).
11. `bazelisk build //app/macos:LoFi` green.

### 6/7. Dependencies & lint

No new external packages: `AppKit`, `ApplicationServices` (AX), `CoreGraphics`/`Foundation`,
`lofi-core`. Rust adds no crates. `c"..."` literals need Rust ≥1.77 (toolchain is 1.95 — OK).
No Go / `.golangci.yml` in this repo — the Go-lint notes in the brief are inapplicable
boilerplate; nothing to add. Rust: keep `# Safety` doc blocks + `// SAFETY:` comments
matching existing fns; exhaustive matches (no `_`). Swift: `enum` namespaces, `_ =` to
discard Bool returns (codebase already does `_ = WindowActivation.raise(...)`),
`@discardableResult` on push wrappers.

### 8. Coordinate system (explicit)

AX (`kAXPosition`/`kAXSize`) and CGWindow (`kCGWindowBounds`) = **top-left global** (origin
top-left of primary display, y down). `NSScreen.frame`/`visibleFrame` = **Cocoa
bottom-left** (origin bottom-left, y up). `compute_geometry` is pure arithmetic on the work
area (no origin notion) ⇒ a top-left work area in ⇒ top-left rects out, directly usable as
AX position/size. Only the **work area** needs flipping (current frame is already top-left
from `kCGWindowBounds`).

Flip (in `WindowCommands.workAreaTopLeft`):
```
let primaryHeight = NSScreen.screens[0].frame.height   // primary (menu-bar) screen, Cocoa
let vf = screen.visibleFrame                            // Cocoa, bottom-left
let topLeftY = primaryHeight - vf.origin.y - vf.height
let workArea = CGRect(x: vf.origin.x, y: topLeftY, width: vf.width, height: vf.height)
```
WHY: Cocoa `vf.origin.y` = distance from primary bottom to the rect's bottom edge; top-left
y = distance from primary top to the rect's top edge = `primaryHeight - (vf.origin.y +
vf.height)`. x unchanged. Use `screens[0]` height (defines the global origin for both
spaces); using the target screen's height mis-places rects on secondary monitors. Edge
cases: empty `screens` ⇒ nil ⇒ no commands; off-screen window ⇒ fall back to
`NSScreen.main` then `screens[0]`.

### 9. Toggle-maximize — TRUE TOGGLE with previous-size restore (USER DECISION)

GNOME uses Mutter `maximize()`/`unmaximize()`, where Mutter remembers the pre-maximize
frame. macOS AX gives us no app-independent maximize and no saved-frame, and LoFi is a
short-lived process — so we persist the previous frame ourselves (`SavedFrameStore`, §2c)
to deliver a faithful toggle: maximize fills the work area; un-maximize restores the exact
frame the window had before it was maximized via LoFi.

Algorithm (`WindowControl.toggleMaximize`): read current top-left-global rect via AX;
"fills" iff all four edges within `kMaximizeFillTolerance = 2` pts of `workArea`.
- not-filling ⇒ `store.save(windowId, currentFrame)`; resize to `workArea` (clear
  fullscreen first).
- filling ⇒ `restore = store.take(windowId) ?? fallbackRect`; resize to `restore`.
  `fallbackRect` (StandardSize from Rust) is used only when no previous frame was saved
  (window was already maximized before LoFi saw it).

The discriminator is geometry ("is it filling?"), not "does a saved frame exist," so a
manually-shrunk LoFi-maximized window re-maximizes on the next press (intuitive). The
divergence from GNOME is only in *mechanism* (we store the frame; Mutter does). Document
the persistence, the UserDefaults backing, the staleness tolerance, the tolerance constant,
and the no-saved-frame fallback in `WindowControl.swift` / `SavedFrameStore.swift` + the
macOS README.

### 10. Full files list

Create: `WindowControl.swift`, `WindowCommands.swift`, `SavedFrameStore.swift`.
Modify (code): `app/core/src/ffi/entries.rs`, `app/core/tests/ffi.rs`, `WindowDiscovery.swift`,
`WindowActivation.swift`, `RustBridge.swift`, `AppDelegate.swift`, `AppListController.swift`.
Docs (technical-writer pass): `app/core/README.md` (FFI count 15→18, document the three
symbols incl. the out-param-untouched-on-false contract + process-lifetime `&'static CStr`
for command ids; bump FFI test count from 38), `app/macos/README.md` (add the two new files
to the layout, command support, AX set order, `"AXFullScreen"`, coordinate flip + why,
toggle-maximize true-toggle behavior (previous-size restore via `SavedFrameStore` +
UserDefaults, staleness/prune, fallback) + tolerance, "no target window ⇒ no command rows";
move window
commands out of the out-of-scope list — workspaces/power stay out), `app/README.md` +
top-level `README.md` (update macOS feature lists only if they enumerate capabilities).
**No BUILD.bazel edit** (`glob(["Sources/LoFi/*.swift"])` auto-picks new files; cbindgen
regenerates the header). **No Info.plist edit** (Screen Recording + Accessibility already
handled; commands reuse `canSeeWindows`).

### 11. Verification

`bazelisk test //app/core:ffi_test` (new command tests; was 38) green; `bazelisk build
//app/macos:LoFi` green. Manual smoke (human, post-coder): with both permissions + a
non-LoFi window open, nine command rows with SF Symbols appear; geometry kinds move/resize
the frontmost non-LoFi window correctly (incl. correct screen on multi-monitor); state
kinds toggle; no other window open ⇒ no command rows.

### 12. Risks / gotchas

1. AX ignores geometry on a fullscreen window ⇒ clear `"AXFullScreen"` before resize; set
   position then size.
2. `"AXFullScreen"` undeclared (bare CFString); read-back is CFBoolean (`as? Bool`); some
   apps don't support it (best-effort, set may not `.success`).
3. Fixed-size / Electron / AWT windows may refuse resize ⇒ `move` returns false; launcher
   quits anyway; acceptable.
4. Title-matching brittleness (gotcha 11) applies to the command target too (pid+title).
5. No target window ⇒ no command rows; `AppListController` must tolerate nil `commandTarget`.
6. Multi-monitor flip: primary height for the origin, target screen's `visibleFrame` for the
   rect — don't mix them.
7. Accessibility-denied: commands only pushed inside `canSeeWindows`; AX calls return false
   if unavailable at activation; TCC frozen at process start (relaunch to pick up grants).
8. `kCGWindowBounds` is a CFDictionary ⇒ `CGRect(dictionaryRepresentation:)`; skip the
   window on read failure so a surfaced window always has a usable `bounds`.
9. StandardSize fallback rect: scan by id (not fixed index) before any filter; keeps 2/3
   math in Rust. Used only when no saved previous frame exists.
10. CGRect components are CGFloat ⇒ `.rounded()` before `Int32(...)` to avoid pixel drift.
11. Saved-frame staleness: CGWindowID is reused after a window closes, so a saved frame can
    in principle restore a *different* window to a wrong size. Mitigated by the
    save→consume lifecycle (entry removed on un-maximize) and the startup `prune(liveWindowIds:)`.
    A single benign wrong-size restore is the worst case; acceptable. UserDefaults persists
    across runs (required — LoFi quits between the two presses).
12. `SavedFrameStore` is macOS-only and must NOT leak into lofi-core (keep the core
    platform-clean per `app/core/README.md`).

---

## Workflow status

- [x] Plan written (architect) — orchestrator locked the StandardSize single-sourcing
  decision (§3f: scan-by-id, no new FFI) and the SF Symbol mapping.
- [x] REVISION (user decision): toggle-maximize is now a TRUE TOGGLE that restores the
  window's previous frame (persisted via the new `SavedFrameStore`, §2c), not a fixed
  StandardSize. StandardSize is now only the no-saved-frame fallback. This is entirely
  Swift-side — **the FFI surface and the FFI tests (§4) are UNCHANGED.**
- [x] Test-writer pass — `app/core/tests/ffi.rs` now has 3 extern decls + 3 helpers
  (`push_command_kind`, `command_id_at`, `command_geometry_at`) + constants (WA, FRAME,
  GEOMETRY_SENTINEL) + 11 `#[test]` cases (lines ~1733-2276). Red until the coder adds the
  FFI symbols.
- [x] Coder pass — Rust FFI (`command_id_cstr` + 3 symbols + borrow-contract doc) added;
  Swift files created (`WindowControl.swift`, `WindowCommands.swift`, `SavedFrameStore.swift`)
  and modified (`WindowDiscovery`, `WindowActivation`, `RustBridge`, `AppListController`,
  `AppDelegate`). **`bazelisk test //app/core:ffi_test` = 49/49 PASS** (verified by
  orchestrator via test.log); **`bazelisk build //app/macos:LoFi` = PASS** (verified).
  SourceKit "cannot find type / no such module" diagnostics are false positives (standalone
  Swift indexer without Bazel's module graph) — the authoritative Bazel compile is clean.
  Coder removed a speculative `#[allow(clippy::too_many_arguments)]` (clippy doesn't run on
  this path; build green without it).
  Coder RECOMMENDS (orchestrator deferred — see below) optional Swift unit tests for
  `SavedFrameStore` + the coordinate-flip math (no Swift test target exists today).
- [x] Reviewer pass — **APPROVED**, no blockers/majors. Verified coordinate flip (§8),
  true-toggle maximize (§9), SavedFrameStore, FFI correctness (command_id_cstr == as_id for
  all 9, out-param-untouched contract), gather/dispatch parity, AX mechanics, SF Symbols,
  rounding, conventions. MINOR M1: `gatherTarget` calls `WindowDiscovery.discover()` a third
  time at startup — plan-conformant (§2b) and benign; orchestrator leaves as-is (avoids API
  coupling). NIT N1: README doc-sync pending → the technical-writer pass below.
- [x] Technical-writer pass — READMEs synced to the implemented code (verified against
  source, not just the plan). `app/core/README.md`: FFI count 15→18, ownership-model accessor
  + mutation lists extended with the three command symbols, a new "three symbols added in the
  macOS commands slice" subsection (incl. the out-param-untouched-on-false contract and the
  process-lifetime `&'static CStr` for command ids), and the borrow-contract section now lists
  all three exemptions (`get_window_id` by-value, `get_command_geometry` by-out-param,
  `get_command_id` process-lifetime). `app/macos/README.md`: Status now mentions the nine
  window-action command rows; Layout lists `WindowControl.swift` / `WindowCommands.swift` /
  `SavedFrameStore.swift` (+ `bounds` on `WindowDiscovery`); FFI test count 38→49; gotchas 11
  and 12 re-pointed at the shared `AXWindowFinder` (`WindowControl.swift`); four new gotchas
  (17 coordinate flip, 18 true-toggle maximize + SavedFrameStore/UserDefaults/prune/fallback,
  19 `"AXFullScreen"` + clear-before-resize + position-then-size, 20 no-target ⇒ no rows),
  Bazel/Temporary gotchas renumbered to 21-26; Out-of-scope qualified (window commands now
  implemented; workspaces/power stay out). `app/README.md`: macos bullet now notes window +
  window-command gathering/activation. Top-level `README.md` left as-is (its macOS paragraph
  does not enumerate window/command capabilities; the pre-existing stale "no MRU/launching"
  line is out of this slice's scope). No discrepancies found between plan and implemented code.
