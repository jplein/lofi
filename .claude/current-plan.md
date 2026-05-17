# macOS port — first slice: NSPanel + `.app` list backed by Rust core

## Context

LoFi today is a GNOME launcher built from a platform-clean Rust core (`app/core/`) and a GTK4/D-Bus frontend (`app/gnome/`). We are adding a Swift/AppKit macOS frontend at `app/macos/` that, eventually, will have the same behaviour as the GNOME one.

This slice is the smallest meaningful step that proves the seam: a Swift/AppKit app that renders a floating `NSPanel` showing the list of `.app` bundles discovered under `/Applications` and `~/Applications` (recursively). The **Rust core owns the canonical list**; Swift talks to it over a C ABI. The mechanics put in place here (FFI, build, panel, discovery) become the spine for future slices (search, MRU, hotkey, launching, icons).

**Out of scope this slice** (each is a follow-up): global hotkey, search field + matcher integration, MRU persistence, launching the selected app, icon rendering, window/workspace/power commands, `/System/Applications` discovery.

## Decisions (confirmed with user before implementation)

- **UI:** pure AppKit `NSTableView` (not SwiftUI in `NSHostingView`).
- **FFI:** manual C-ABI on the existing `lofi-core` crate (not uniffi).
- **Xcode project:** XcodeGen (YAML in git; `.xcodeproj` generated, gitignored).
- **Direction of data flow:** Swift discovers, pushes into Rust; Rust holds the canonical entry list. Mirrors the GNOME pattern where `app/gnome/src/apps.rs` does platform discovery and `lofi-core` owns the `Vec<Application>`.

## Rust changes — `app/core/`

### `app/core/Cargo.toml`
- Add `crate-type = ["staticlib", "rlib"]`.
- Add `[features] ffi = []` (default off).
- Add `[build-dependencies] cbindgen = "0.27"`.

### `app/core/build.rs` (new)
- When the `ffi` feature is on, run cbindgen and emit `app/core/include/lofi_core.h`. Otherwise no-op.

### `app/core/cbindgen.toml` (new)
- C language, `#pragma once`, `lofi_` prefix, opaque pointer for `EntryList`.

### `app/core/include/` (new dir, gitignored)
- Generated header lives here.

### `app/core/src/lib.rs`
- Add `#[cfg(feature = "ffi")] pub mod ffi;`.

### `app/core/src/ffi/mod.rs` and `app/core/src/ffi/entries.rs` (new)

Minimum FFI surface for this slice (everything `#[no_mangle] extern "C"`):

- `lofi_entries_new() -> *mut EntryList`
- `lofi_entries_free(*mut EntryList)`
- `lofi_entries_push_application(list: *mut EntryList, name: *const c_char, bundle_id: *const c_char, icon: *const c_char /* nullable */) -> bool`
- `lofi_entries_len(list: *const EntryList) -> usize`
- `lofi_entries_get_name(list: *const EntryList, idx: usize) -> *const c_char` (borrow valid until next mutation or `free`)

`EntryList` is an opaque newtype wrapping `Vec<Entry>` behind a heap `Box`. `push_application` constructs `Application` from the provided UTF-8 C strings (copy in) and wraps in `Entry::Application(...)`. Null `icon` argument maps to `None`. Returns `false` if any required pointer is null or invalid UTF-8.

**`desktop_id` policy on macOS (temporary):** store the macOS bundle identifier (e.g. `com.apple.Terminal`) verbatim in `Application::desktop_id`. The GNOME `.desktop`-suffix invariant does not apply on macOS; the field is just an opaque stable identifier. Document in `app/core/README.md` so future MRU work revisits it.

### `app/core/README.md` (update)
Add an "FFI surface" section: the `staticlib`+`ffi` feature, the opaque-handle pattern, the push-based ownership model (Swift produces, Rust holds), and the `desktop_id`-as-bundle-id temporary policy. Also note that `rusqlite` keeps its `bundled` feature on Mac and Swift must not link `libsqlite3.tbd` (avoids duplicate-symbol errors).

## Swift project — `app/macos/`

### Layout
```
app/macos/
  README.md
  project.yml                # XcodeGen spec
  build.sh                   # cargo + xcodegen + xcodebuild
  run.sh                     # opens the built .app
  .gitignore                 # LoFi.xcodeproj, build/, DerivedData/
  Sources/LoFi/
    main.swift               # NSApplication.shared.run() boot
    AppDelegate.swift        # creates PanelController on launch
    PanelController.swift    # NSPanel subclass + show/hide
    AppDiscovery.swift       # .app enumeration + Info.plist read
    AppListController.swift  # NSTableView delegate + data source
    RustBridge.swift         # Swift wrapper around C API
    LoFi-Bridging-Header.h   # #include "lofi_core.h"
  Resources/
    Info.plist               # LSUIElement=YES, bundle id, version
    LoFi.entitlements        # empty for now
```

### `project.yml`
- One macOS app target `LoFi`, deployment target Tahoe (macOS 15.0+).
- `LSUIElement = YES`.
- Header search path: `$(SRCROOT)/../core/include`.
- Library search paths: `$(SRCROOT)/../target/aarch64-apple-darwin/release` (Release), `…/debug` (Debug).
- `OTHER_LDFLAGS = -llofi_core`.
- Bridging header: `Sources/LoFi/LoFi-Bridging-Header.h`.
- Pre-build Run Script Phase invokes `../macos/build.sh --rust-only`.

### `PanelController.swift` — NSPanel setup
- Subclass `NSPanel`, style mask `[.borderless, .nonactivatingPanel]`.
- `level = .floating`.
- `collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]`.
- `isMovableByWindowBackground = false`, `hidesOnDeactivate = true`.
- **Override `canBecomeKey` to return `true`** (borderless panels return false by default — this silently breaks event delivery).
- Fixed size 640×400, centered via `self.center()` after sizing.
- On `applicationDidFinishLaunching`: call `NSApp.activate(ignoringOtherApps: true)` then `panel.makeKeyAndOrderFront(nil)`. Without `activate`, an `LSUIElement` app starts background-only.

### `AppDiscovery.swift`
- `FileManager.default.enumerator(at:includingPropertiesForKeys:options:)` with `[.skipsPackageDescendants, .skipsHiddenFiles]`.
- Roots: `/Applications` and `FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications")`.
- For each URL with `pathExtension == "app"`: `Bundle(url:)`, read `CFBundleDisplayName` → `CFBundleName` → basename fallback. Read `CFBundleIdentifier`; skip if absent.
- Dedup by bundle identifier (first-wins, mirrors GNOME first-dir-wins in `app/gnome/src/apps.rs:51-119`).
- Synchronous on launch (gather, then show panel). Async/progressive is a future slice.

### `RustBridge.swift`
- `final class EntryList` wrapping `OpaquePointer`. `init()` → `lofi_entries_new()`. `deinit` → `lofi_entries_free`.
- `pushApplication(name:bundleId:icon:)` bridges Swift `String` via `withCString`. Optional `icon` becomes `nil` C pointer.
- `count` and `name(at:)` mirror the C accessors. `name(at:)` copies the borrowed `*const c_char` into a Swift `String` so callers never see the borrow.

### `AppListController.swift`
- Single-column `NSTableView` inside an `NSScrollView` filling the panel `contentView`.
- `NSTableViewDataSource.numberOfRows(in:)` reads `entryList.count`.
- `NSTableViewDelegate.tableView(_:viewFor:row:)` returns an `NSTableCellView` with an `NSTextField` showing `entryList.name(at: row)`. No icons this slice.

### `LoFi-Bridging-Header.h`
- `#include "lofi_core.h"`.

### `Info.plist`
- `LSUIElement = YES`, bundle id `dev.jplein.lofi` (or analogous; align with `lofi-shell@jplein.dev` style).
- `CFBundleShortVersionString` matches workspace version.

### `LoFi.entitlements`
- Empty `<dict/>` for now; entitlements come with later slices (Accessibility for global hotkey, etc.).

## Build wiring

### `app/macos/build.sh`
Stages (gateable by `--rust-only`, `--no-rust`, etc.):
1. `cargo build --release -p lofi-core --features ffi --target aarch64-apple-darwin` (from repo root).
2. `xcodegen generate` (in `app/macos/`).
3. `xcodebuild -scheme LoFi -configuration Debug -derivedDataPath build build`.

**PATH gotcha:** Xcode Run Script Phase runs with a minimal `PATH` and won't find Nix-installed `cargo`/`cbindgen`. `build.sh` must set `PATH` explicitly (e.g. prepend `$HOME/.nix-profile/bin` and `/run/current-system/sw/bin` if present). Cover this in `app/macos/README.md`.

### `flake.nix` — Darwin devShell
Add `xcodegen` to the `aarch64-darwin` devShell's `nativeBuildInputs`. Swift itself stays out of the Nix devShell — it comes from the user's Xcode/Command Line Tools install.

## READMEs (per CLAUDE.md: READMEs are source of truth)

- **New** `app/macos/README.md`: layout; Swift-pushes-into-Rust data-flow rationale; XcodeGen rationale; the Nix-doesn't-provide-Swift seam; build/run; out-of-scope items; NSPanel design (style mask, `canBecomeKey` override, `NSApp.activate` requirement); the Xcode Run Script PATH gotcha.
- **Update** `app/core/README.md`: FFI section.
- **Update** `app/README.md`: macOS frontend exists; data flow note.
- **Update** root `README.md`: macOS section moves from "(Planned)" to "Experimental".

## Critical files

**Modify:**
- `/Users/jplein/Git/jplein/lofi/app/core/Cargo.toml`
- `/Users/jplein/Git/jplein/lofi/app/core/src/lib.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/README.md`
- `/Users/jplein/Git/jplein/lofi/app/README.md`
- `/Users/jplein/Git/jplein/lofi/README.md`
- `/Users/jplein/Git/jplein/lofi/flake.nix`

**Create (Rust):**
- `/Users/jplein/Git/jplein/lofi/app/core/build.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/cbindgen.toml`
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/mod.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/src/ffi/entries.rs`
- `/Users/jplein/Git/jplein/lofi/app/core/.gitignore` (for `include/`)

**Create (Swift):** see `app/macos/` tree above.

## Testing strategy

- **Rust:** integration tests at `app/core/tests/ffi.rs` that exercise the FFI surface from a separate `extern "C"` perspective. Cover: round-trip push + len + get_name; null-handling for the `icon` argument; null-handling for required args (returns `false`); UTF-8 invalid bytes return `false`; multiple-push ordering preserved; free does not crash.
- **Swift:** not unit-testable without an XCTest target. Manual end-to-end run is the verification.
- **GNOME regression:** existing tests must still pass after the `crate-type`/`feature` changes.

## Risks / gotchas

1. **`NSPanel.canBecomeKey` returns false** for borderless panels by default — must override. Combined with `LSUIElement=YES`, the process is background-only at launch; must call `NSApp.activate(ignoringOtherApps: true)` before showing the panel.
2. **`rusqlite` bundled SQLite symbols** — once `lofi-core` ships as `.a` and Swift links it, Swift code must not link `libsqlite3.tbd` (duplicate symbols). Note in `app/core/README.md`.
3. **Xcode Run Script PATH** doesn't include Nix paths. `build.sh` sets PATH; document.
4. **`crate-type = ["staticlib", "rlib"]`** can change link behavior workspace-wide; verify GNOME build still works.

## Verification

1. GNOME regression: `cargo build -p lofi-gnome` on Linux still succeeds.
2. Rust artifacts on macOS: `cargo build --release -p lofi-core --features ffi --target aarch64-apple-darwin` produces `liblofi_core.a` and `app/core/include/lofi_core.h`.
3. FFI tests pass: `cargo test -p lofi-core --features ffi`.
4. `./app/macos/build.sh` exits 0; `LoFi.app` exists in build output.
5. `./app/macos/run.sh` launches: borderless floating panel centered; list shows `Safari`, `Terminal` (under `Utilities/`), `Calculator`, and any third-party apps in `~/Applications`.
6. `Cmd-Q` exits cleanly (`lofi_entries_free` deinit path exercised).
7. Running twice produces the same set of apps in the same order.

## Workflow status

- [x] Initial plan written to `.claude/current-plan.md`
- [x] Test-writer pass 1 — 10 FFI tests
- [x] Coder pass 1 — Rust FFI + Swift project + build wiring; 67 tests pass; macOS staticlib + header generated
- [x] Reviewer pass — approved with minor notes (no blockers)
- [x] Test-writer pass 2 — `extern crate lofi_core as _;` + 2 more UTF-8 tests (bundle_id, icon)
- [x] Coder pass 2 — simplified `build.rs` (dropped nested staticlib build); 12/12 FFI tests pass
- [x] Technical-writer pass — READMEs reconciled with the simplified build.rs; "Status: implemented but Xcode build unverified in agent env" added to macOS README

Remaining (not blocking this slice):
- Manual end-to-end verification: user runs `./app/macos/build.sh` and `./app/macos/run.sh` to confirm the panel renders the list.
- GNOME regression check: user runs `cargo build -p lofi-gnome` on Linux to confirm the `crate-type = ["staticlib", "rlib"]` change didn't break the GNOME side.
- Optional: add the cbindgen-0.29 rationale to `app/core/README.md` Dependencies line so a future version bump doesn't downgrade and silently break header generation.
