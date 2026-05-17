# app/macos

The macOS frontend for LoFi. Swift + AppKit on top of the shared Rust core (`app/core/`) via a C ABI.

## Status

Experimental. Builds and runs on macOS 26 Tahoe with Xcode 26. `./build.sh` produces `LoFi.app`; `./run.sh` floats a borderless panel listing every `.app` under `/Applications` and `~/Applications`. The slice is intentionally a static list — no global hotkey, no search, no launching, no MRU yet (see *Out of scope* below).

## Why a separate frontend

The cross-platform core (`lofi-core`) holds the data model and pure logic — `Application`, `Entry`, `EntryRef`, fuzzy matcher, MRU store. Anything that depends on a particular window system or app-discovery mechanism lives in the platform crate. On GNOME that's `app/gnome/` (GTK4 + libadwaita + D-Bus to the Shell extension); on macOS that's this directory (AppKit + a bridging header to a `liblofi_core.a` static library).

The two frontends share nothing at the windowing-system level, so they're separate projects. Sharing the data model and logic (in `core/`) is what keeps fuzzy match, MRU, and command activation behaviour consistent between platforms.

## Why Swift drives discovery and Rust holds the list

Same pattern as `app/gnome/`: the platform layer is the gatherer, the core is the canonical store.

- `AppDiscovery.discover()` walks `/Applications` and `~/Applications`, dedups by bundle identifier (first-wins), and returns a sorted list. Mirrors GNOME's `app/gnome/src/apps.rs` first-dir-wins policy.
- `AppDelegate` pushes each discovered `.app` into the Rust-owned `EntryList` via `lofi_entries_push_application(...)`. After that point the list belongs to Rust; Swift only reads it back through `lofi_entries_len` / `lofi_entries_get_name`.

This shape leaves the matcher, MRU, and future activation logic on the Rust side without having to expose `Application`/`Entry` as Swift types. Adding an `EntryRef`-based MRU lookup in a future slice is a Rust change, not a Swift one.

## Why XcodeGen

`.xcodeproj` is generated locally, gitignored, and reviewed via the `project.yml` source of truth. Hand-maintained Xcode project files explode into merge conflicts even on trivial changes — a YAML spec keeps diffs tractable.

XcodeGen comes from the Nix devShell (`xcodegen` is in `nativeBuildInputs` for the `aarch64-darwin` shell in `flake.nix`); Swift itself comes from the user's Xcode / Command Line Tools install. Nix on Darwin does not currently provide a usable Swift toolchain, which is why the build splits across two providers.

## Layout

```
project.yml             XcodeGen spec; .xcodeproj is regenerated from this
build.sh                cargo + xcodegen + xcodebuild driver
run.sh                  opens the most recent .app bundle
BUILD.bazel             sh_binary targets :build and :launch
bazel/
  build.sh              wrapper that execs ../build.sh under bazel run
  launch.sh             wrapper that execs ../run.sh under bazel run
Sources/LoFi/
  main.swift            NSApplication boot
  AppDelegate.swift     gather apps, push into Rust, show panel
  PanelController.swift NSPanel subclass + show/center
  AppDiscovery.swift    /Applications + ~/Applications enumeration
  AppListController.swift  NSTableView data source + delegate
  RustBridge.swift      Swift wrapper around the C ABI
  LoFi-Bridging-Header.h  #include "lofi_core.h"
Resources/
  Info.plist            LSUIElement=YES, bundle id, version
  LoFi.entitlements     empty for now
```

## Build / run

Two equivalent front doors. Pick whichever fits your habits.

**Direct shell scripts:**

```sh
./build.sh    # cargo build + xcodegen + xcodebuild
./run.sh      # open the .app
```

`build.sh --rust-only` is what the Xcode pre-build phase invokes; `build.sh --no-rust` skips the Rust stage for fast incremental Swift iteration when the staticlib hasn't changed.

**Bazel:**

```sh
bazel run //app/macos:build     # same as ./build.sh
bazel run //app/macos:launch    # same as ./run.sh
```

The Bazel targets are thin `sh_binary` wrappers that exec the canonical scripts (see `app/macos/bazel/`). Cargo and Xcode still do the real work — Bazel is just the entry-point driver. A future slice could swap this for `rules_rust` + `rules_apple` targets, but the wrap-the-script shape stays drop-in compatible with everything else in the repo. Bazelisk is provided by the Darwin Nix devShell; `.bazelversion` at the repo root pins the Bazel release.

## Gotchas worth calling out

Each cost real time to figure out the first time; each is permanent in the code or build setup with a comment pointing back here. If something in this list starts looking redundant, double-check it really is — the cell-rendering bug below was three separate issues stacked, and stripping any one of them brings the blank panel back.

### AppKit

1. **`canBecomeKey` returns `false` for borderless `NSPanel` by default.** Without overriding, the panel renders but never receives keyboard input — typing into what looks like a focused launcher just goes to whatever app was previously frontmost. `LoFiPanel: NSPanel` overrides this to `true` (see `PanelController.swift`).
2. **`LSUIElement=YES` launches the process in the background.** No Dock icon, no menu bar — but also, the panel never becomes key on its own. `AppDelegate` must call `NSApp.activate(ignoringOtherApps: true)` before showing the panel; without that step the borderless window appears but stays inert.
3. **`LSUIElement=YES` also suppresses the system Application menu, so Cmd-Q has no handler.** `AppDelegate.installHiddenMenu()` installs a minimal `NSMenu` containing only a `Quit` item with `keyEquivalent: "q"`. The menu never becomes visible (still LSUIElement), but its key equivalent fires.
4. **`NSScrollView` does not auto-resize its `documentView`.** A bare `NSTableView()` set as `documentView` sits at 0×0 inside the scroll view and never asks for cell views — the table is alive (clicks select rows, the scroll wheel "scrolls") but draws nothing. `AppListController` constructs the table with an explicit non-zero `frame` and pairs it with `columnAutoresizingStyle = .uniformColumnAutoresizingStyle`.
5. **`NSTableView.dataSource` and `.delegate` are weak.** If the only strong reference to the list controller is a local variable inside `applicationDidFinishLaunching`, the controller deallocates when that method returns and the table silently stops calling `viewFor:row:` — rows scroll and select normally because `numberOfRows` is cached, but cells render blank. `AppDelegate` keeps a strong `listController` property; do not "simplify" it away.
6. **`?? `does not fall through empty strings, only `nil`.** Some apps set `CFBundleDisplayName` to `""` rather than omitting the key, which a naive `(displayName as? String) ?? (bundleName as? String) ?? basename` accepts as a valid empty string. `AppDiscovery.discover()` uses a `nonEmpty()` helper to coerce empty-string Info.plist values to `nil` so the fallback chain works.

### Build / toolchain

7. **Xcode Run Script Phases run with a stripped-down `PATH`** that doesn't include `$HOME/.nix-profile/bin`. `build.sh` explicitly prepends Nix and Homebrew paths so cargo / cbindgen / xcodegen all resolve regardless of how the script is invoked.
8. **Xcode 26 / MacOSX26.5 SDK invokes `ld` directly with clang-driver flags it does not understand** (`-Xlinker`, `-isysroot`, `-dynamiclib`, `-rdynamic`, `-fobjc-link-runtime`), and the link fails. `project.yml` pins `LD: $(DT_TOOLCHAIN_DIR)/usr/bin/clang` so clang is the link driver, which translates those flags before invoking the actual linker. Related: `ENABLE_DEBUG_DYLIB: "NO"` opts out of the Xcode 15.3+ debug-dylib split-binary flow that triggered the same family of breakage on first contact.

### Temporary for this slice

9. **`hidesOnDeactivate = false`** in `PanelController.swift`. Spotlight-style "dismiss on focus loss" is the eventual UX, but with no global hotkey yet to bring the panel back, a hide-on-deactivate panel vanishes the moment `open LoFi.app` returns control to the launching terminal. Flip back to `true` once the hotkey slice lands.

## Out of scope this slice

Each is a follow-up:

- Global hotkey to summon the panel (requires Accessibility entitlement).
- Search field + matcher integration.
- MRU persistence.
- Launching the selected app.
- Icon rendering.
- Window / workspace / power commands.
- `/System/Applications` discovery.
