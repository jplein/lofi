# app/macos

The macOS frontend for LoFi. Swift + AppKit on top of the shared Rust core (`app/core/`) via a C ABI.

## Status

Implemented but not yet end-to-end verified. The Rust FFI surface and its integration tests pass on Linux and macOS; the Swift sources, `project.yml`, and `build.sh` are in place; but the Xcode-driven build (`xcodegen generate` + `xcodebuild`) has not been run yet in this environment because the toolchain is not available here. Manual verification — run `./build.sh` on a Mac with Xcode / Command Line Tools and confirm `./run.sh` floats a panel — is the next step.

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

```sh
./build.sh    # cargo build + xcodegen + xcodebuild
./run.sh      # open the .app
```

`build.sh --rust-only` is what the Xcode pre-build phase invokes; `build.sh --no-rust` skips the Rust stage for fast incremental Swift iteration when the staticlib hasn't changed.

## NSPanel design — three things that bite

These cost real time to figure out and are worth calling out:

1. **`canBecomeKey` returns `false` for borderless `NSPanel` by default.** Without overriding, the panel renders but never receives keyboard input — typing into what looks like a focused launcher just goes to whatever app was previously frontmost. `LoFiPanel: NSPanel` overrides this to `true` (see `PanelController.swift`).
2. **`LSUIElement=YES` launches the process in the background.** No Dock icon, no menu bar — but also, the panel never becomes key on its own. `AppDelegate` must call `NSApp.activate(ignoringOtherApps: true)` before showing the panel; without that step the borderless window appears but stays inert.
3. **Xcode Run Script Phases run with a stripped-down PATH** that doesn't include `$HOME/.nix-profile/bin`. `build.sh` explicitly prepends Nix and Homebrew paths so cargo / cbindgen / xcodegen all resolve regardless of how the script is invoked.

## Out of scope this slice

Each is a follow-up:

- Global hotkey to summon the panel (requires Accessibility entitlement).
- Search field + matcher integration.
- MRU persistence.
- Launching the selected app.
- Icon rendering.
- Window / workspace / power commands.
- `/System/Applications` discovery.
