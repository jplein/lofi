# app/macos

The macOS frontend for LoFi. Swift + AppKit on top of the shared Rust core (`app/core/`) via a C ABI, built by Bazel.

## Status

Experimental. Builds and runs on macOS 26 Tahoe with Xcode 26. `bazel run //app/macos:launch` floats a borderless panel listing every `.app` under `/System/Applications`, `/Applications`, and `~/Applications`. The panel now has a focused search field at the top that fuzzy-filters as the user types, and each row renders as `[icon] Name … [Category]` with the category dimmed and trailing-aligned. Activation history persists across launches via the SQLite store at `~/Library/Application Support/dev.jplein.lofi/mru.sqlite`, so the apps you actually use bubble to the top on every subsequent launch. With Screen Recording **and** Accessibility permissions granted, every open application window also appears in the list as a `[icon] Title — App` row with category `"Window"`; pressing Enter (or clicking) raises the window via the AX API and quits LoFi, and MRU bumps windows on the same code path as apps. See *Permissions* below for how the two TCC grants interact.

Still pending: global hotkey to summon the panel (see *Out of scope* below).

## Why a separate frontend

The cross-platform core (`lofi-core`) holds the data model and pure logic — `Application`, `Entry`, `EntryRef`, fuzzy matcher, MRU store. Anything that depends on a particular window system or app-discovery mechanism lives in the platform crate. On GNOME that's `app/gnome/` (GTK4 + libadwaita + D-Bus to the Shell extension); on macOS that's this directory (AppKit linking the `liblofi_core.a` produced by `rules_rust`, with `lofi_core.h` exposed as a Clang module via `rules_swift`).

The two frontends share nothing at the windowing-system level, so they're separate projects. Sharing the data model and logic (in `core/`) is what keeps fuzzy match, MRU, and command activation behaviour consistent between platforms.

## Why Swift drives discovery and Rust holds the list

Same pattern as `app/gnome/`: the platform layer is the gatherer, the core is the canonical store.

- `AppDiscovery.discover()` walks `/System/Applications`, `/Applications`, and `~/Applications`, dedups by bundle identifier (first-wins, in that root order so Apple's stock apps shadow any same-bundle-id third-party installs), and returns a sorted list. Mirrors GNOME's `app/gnome/src/apps.rs` first-dir-wins policy.
- `AppDelegate` pushes each discovered `.app` into the Rust-owned `EntryList` via `lofi_entries_push_application(...)`. After that point the list belongs to Rust; Swift only reads it back through `lofi_entries_len` / `lofi_entries_get_name`.

This shape leaves the matcher, MRU, and future activation logic on the Rust side without having to expose `Application`/`Entry` as Swift types. Adding an `EntryRef`-based MRU lookup in a future slice is a Rust change, not a Swift one.

## Why Bazel

Bazel owns the macOS build graph end to end. `rules_rust` + `crate_universe` consume `app/Cargo.lock` and produce `liblofi_core.a`; a `genrule` runs the (Bazel-built) `cbindgen` binary to emit `lofi_core.h`; `cc_library` + `swift_interop_hint` expose the header to Swift as the `LoFiCore` Clang module; `rules_swift` compiles the `.swift` sources; `rules_apple`'s `macos_application` packages everything into `LoFi.app`.

The earlier xcodegen + xcodebuild + cargo + bash-script pipeline is gone — one front door (Bazel), one build graph, one set of incrementality rules. The Linux GNOME crate still goes through Cargo + Crane in `flake.nix`; Bazel is macOS-only for now. Both paths read the same `Cargo.lock` so dependency versions stay coherent.

## Layout

```
BUILD.bazel             swift_library + macos_application + launch + xcodeproj
bazel/
  launch.sh             extracts the bundle from LoFi.zip and `open`s it
Sources/LoFi/
  main.swift            NSApplication boot
  AppDelegate.swift     gather apps, push into Rust, show panel
  PanelController.swift NSPanel subclass + show/center
  AppDiscovery.swift    /System/Applications + /Applications + ~/Applications enumeration
  AppListController.swift  NSTableView data source + delegate
  RustBridge.swift      Swift wrapper around the C ABI; `import LoFiCore`
  Permissions.swift     Screen Recording + Accessibility helpers
  WindowDiscovery.swift CGWindowList enumeration of open windows
  WindowActivation.swift AXUIElement-based window raise
Resources/
  Info.plist            LSUIElement=YES, bundle id, version
  LoFi.entitlements     empty for now
```

## Build / run

```sh
bazelisk build //app/macos:LoFi       # produce bazel-bin/app/macos/LoFi.zip
bazelisk run   //app/macos:launch     # unzip + `open` the bundle
bazelisk test  //app/core:ffi_test    # run the 38 FFI integration tests
bazelisk run   //app/macos:xcodeproj  # regenerate app/macos/LoFi.xcodeproj
```

Always invoke the build through `bazelisk`, not `bazel`. The Nix devShell installs `bazelisk` (a thin launcher that reads `.bazelversion` and downloads the pinned Bazel release on demand); it does *not* install a `bazel` binary directly. Calling `bazel` outside the devShell will either fail with "command not found" or pick up a system Bazel that doesn't match `.bazelversion`.

`bazelisk run //app/macos:LoFi` also "works" — it invokes rules_apple's stock launcher script, which extracts the bundle to `/tmp` and execs the binary directly. The downside is it bypasses Launch Services, so `LSUIElement=YES` activation can behave subtly differently. `:launch` routes through `open`, which matches the production launch path.

For Xcode-based debugging, run `bazelisk run //app/macos:xcodeproj` and open `app/macos/LoFi.xcodeproj`. The project is generated by `rules_xcodeproj` from the Bazel graph — Bazel still drives the actual compile; Xcode is the IDE and debugger surface only. The `.xcodeproj` is gitignored; regenerate after Bazel-graph changes.

First-time build downloads Bazel (per `.bazelversion`), then the rule stacks, then resolves the Cargo lockfile via `crate_universe`. Subsequent builds hit the action cache and finish in seconds.

`DEVELOPER_DIR` must point at the user's Xcode install (`/Applications/Xcode.app/Contents/Developer`) before invoking `bazelisk` — `.envrc` does this. Without that override the Nix devShell leaves `DEVELOPER_DIR` pointing at a partial nix-store SDK and `rules_swift` bails with "Could not determine Xcode version at all."

## Permissions

Window enumeration and activation depend on two separate macOS TCC (Transparency, Consent, and Control) grants. Apps (the `.app` enumeration path) need neither — `AppDiscovery` reads bundle directories that are world-readable, and `NSWorkspace.open` does not require special entitlements. The launcher therefore degrades gracefully: if either window permission is denied, the panel still lists every installed app and Enter still launches it. The window-row affordance is the only thing that disappears.

Why two permissions rather than one: macOS treats *reading* on-window state (titles, geometry) and *driving* other processes (raising a specific window) as distinct privacy surfaces, gated by different TCC categories.

- **Screen Recording** (`NSScreenCaptureUsageDescription` in `Info.plist`) — required for `CGWindowListCopyWindowInfo` to return `kCGWindowName` strings. Without it the API still returns one entry per on-screen window, but titles come back as empty strings, which makes the launcher rows useless ("— TextEdit", "— Safari"). LoFi's policy is to drop windows from the list entirely in this case rather than show titleless placeholders.
- **Accessibility** (no Info.plist key; `AXIsProcessTrustedWithOptions` is the API contract) — required for `AXUIElementPerformAction` to raise a specific window. Without it, `AXUIElementCopyAttributeValue(app, kAXWindowsAttribute, ...)` returns `kAXErrorAPIDisabled` and raise is impossible. Symmetric to the Screen Recording denial path: windows are dropped from the list because we can't act on them.

To grant: System Settings → Privacy & Security → Screen Recording (toggle LoFi on) and Privacy & Security → Accessibility (toggle LoFi on). The first launch of LoFi triggers both system prompts (`CGRequestScreenCaptureAccess` and `AXIsProcessTrustedWithOptions(prompt: true)`); subsequent launches won't re-prompt because TCC remembers the user's decision.

**Relaunch to pick up newly-granted permissions.** This is the most surprising part of the UX: `CGPreflightScreenCaptureAccess` and `AXIsProcessTrustedWithOptions` capture TCC state at process start, so granting a permission while LoFi is running has no effect until the next launch. This is Apple's design — TCC permissions are baked into the process's sandbox context at exec time so a long-running daemon can't be silently granted new capabilities mid-session. The practical consequence: the first-launch flow is *launch, grant, relaunch, grant, relaunch* in the worst case where the user grants Screen Recording first, relaunches, then grants Accessibility. The Gotchas section below calls this out again as item 10.

## Gotchas worth calling out

Each cost real time to figure out the first time; each is permanent in the code or build setup with a comment pointing back here. If something in this list starts looking redundant, double-check it really is — the cell-rendering bug below was three separate issues stacked, and stripping any one of them brings the blank panel back.

### AppKit

1. **`canBecomeKey` returns `false` for borderless `NSPanel` by default.** Without overriding, the panel renders but never receives keyboard input — typing into what looks like a focused launcher just goes to whatever app was previously frontmost. `LoFiPanel: NSPanel` overrides this to `true` (see `PanelController.swift`).
2. **`LSUIElement=YES` launches the process in the background.** No Dock icon, no menu bar — but also, the panel never becomes key on its own. `AppDelegate` must call `NSApp.activate(ignoringOtherApps: true)` before showing the panel; without that step the borderless window appears but stays inert.
3. **`LSUIElement=YES` also suppresses the system Application menu, so Cmd-Q has no handler.** `AppDelegate.installHiddenMenu()` installs a minimal `NSMenu` containing only a `Quit` item with `keyEquivalent: "q"`. The menu never becomes visible (still LSUIElement), but its key equivalent fires.
4. **`NSScrollView` does not auto-resize its `documentView`.** A bare `NSTableView()` set as `documentView` sits at 0×0 inside the scroll view and never asks for cell views — the table is alive (clicks select rows, the scroll wheel "scrolls") but draws nothing. `AppListController` constructs the table with an explicit non-zero `frame` and pairs it with `columnAutoresizingStyle = .uniformColumnAutoresizingStyle`.
5. **`NSTableView.dataSource` and `.delegate` are weak.** If the only strong reference to the list controller is a local variable inside `applicationDidFinishLaunching`, the controller deallocates when that method returns and the table silently stops calling `viewFor:row:` — rows scroll and select normally because `numberOfRows` is cached, but cells render blank. `AppDelegate` keeps a strong `listController` property; do not "simplify" it away.
6. **`??` does not fall through empty strings, only `nil`.** Some apps set `CFBundleDisplayName` to `""` rather than omitting the key, which a naive `(displayName as? String) ?? (bundleName as? String) ?? basename` accepts as a valid empty string. `AppDiscovery.discover()` uses a `nonEmpty()` helper to coerce empty-string Info.plist values to `nil` so the fallback chain works.
7. **`panel.initialFirstResponder = searchField` must be set *before* `makeKeyAndOrderFront(_:)`.** Setting it afterwards is a silent no-op — by the time the panel orders front it has already picked a default responder, and assigning `initialFirstResponder` later does not retroactively re-route the focus. The user sees the panel appear with the cursor "in" the search field visually but typing goes nowhere. `PanelController.swift`'s init wires this up in the right order; do not move the assignment.
8. **`NSStackView` with `.leading` alignment leaves the search field at its intrinsic narrow width.** A stack view in vertical orientation sizes its arranged subviews to their intrinsic content size on the cross axis, and `NSSearchField`'s intrinsic width is ~100pt — far narrower than the panel. The panel pins the search field's leading and trailing anchors to the stack so the field spans the full panel width.
9. **Safari hides in a Cryptex, and the `/Applications/Safari.app` symlink is invisible to bundled apps.** Modern macOS keeps Safari in `/System/Cryptexes/App/System/Applications/Safari.app` (Rapid Security Response can update Safari/WebKit without an OS update). There's a symlink at `/Applications/Safari.app` pointing into the Cryptex, but two things conspire to hide it from a naive enumeration: (a) `FileManager.enumerator(at:options:)` silently skips symlinks-to-directories regardless of options, and (b) the running `LoFi.app` gets a TCC-filtered listing of `/Applications` that omits the Cryptex symlink even when reading via `contentsOfDirectory(at:)`. `AppDiscovery` does a manual recursion (so it would catch *any* symlinks not in the Cryptex case) **and** scans `/System/Cryptexes/App/System/Applications` directly as a fourth root, which is readable from bundled apps and contains Safari (resolved via `/System/Volumes/Preboot/...`).
10. **TCC state for Screen Recording and Accessibility is captured at process start.** `CGPreflightScreenCaptureAccess` and `AXIsProcessTrustedWithOptions` return whatever the state was when LoFi launched, not whatever the state happens to be when we call them. A user who grants either permission while LoFi is already running won't see windows appear in the panel until the next launch — the running process is still operating against the TCC context it was forked with. This is Apple's design (a long-running daemon can't be silently granted new privileges mid-session) and is mechanically the same behavior Raycast, Alfred, and CleanShot exhibit. It's still non-obvious for users coming from GNOME where permission checks happen dynamically against the polkit/Wayland session. The Permissions section above documents this for end users; the gotcha here is so we don't "fix" it by, e.g., polling the API or re-checking after `NSApp.activate` — the API answer is frozen.
11. **AX `kAXRaiseAction` matches windows by title, and titles are not unique.** `WindowActivation.raise(pid:title:)` walks the AX windows array for the target process and picks the first one whose `kAXTitleAttribute` equals the requested title. This is fine for most apps (one "Inbox — Mail", one "Preferences"), but breaks on apps that genuinely have multiple identically-titled windows ("Untitled — TextEdit", three blank Safari tabs all named the bundle's default title). First match wins, which means the wrong window can come forward. The robust fix uses the private `_AXUIElementGetWindow` API (which returns the `CGWindowID` for an `AXUIElement`) to disambiguate the AX window against the `CGWindowID` we already have in `windowAux`; not done in this slice because the private-API dance is more code than the title heuristic and the failure mode is "wrong window of N identical ones gets raised" rather than "raise fails".

### Bazel

12. **`DEVELOPER_DIR` set by the Nix devShell points at a partial Darwin SDK** in the nix store, which doesn't contain a usable Swift toolchain. `rules_swift` walks `xcrun --find swiftc` against `DEVELOPER_DIR` and bails. `.envrc` explicitly re-exports `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` after `use flake` to override.
13. **`apple_support` must appear above `rules_cc` in `MODULE.bazel`.** Module ordering determines toolchain registration order; if `rules_cc` registers first, rules_swift picks up the generic CC toolchain (target triple "local") and fails. Reordering is non-obvious from the error message.
14. **`swift_interop_hint` auto-generates the Clang module map** — do not also put a hand-written `module.modulemap` in `cc_library.hdrs`. It gets included as a C header in the auto-generated map and the parser barfs on module-map syntax.
15. **`crate_universe`'s binary targets are named `<crate>__<bin>`**, e.g. `@crates//:cbindgen__cbindgen`, not `__cli` or `__bin`. `gen_binaries` takes a list of binary names, not a `True` boolean.
16. **cbindgen 0.29 has no `--features` CLI flag** — features are discovered by running `cargo metadata --all-features` internally. Passing `--features ffi` to the binary errors out; the right approach is to let cbindgen compute it.

### Temporary for this slice

17. **`hidesOnDeactivate = false`** in `PanelController.swift`. Spotlight-style "dismiss on focus loss" is the eventual UX, but with no global hotkey yet to bring the panel back, a hide-on-deactivate panel vanishes the moment `open LoFi.app` returns control to the launching terminal. Flip back to `true` once the hotkey slice lands.

## Out of scope this slice

Each is a follow-up:

- Global hotkey to summon the panel (requires Accessibility entitlement).
- Workspace + power commands. Mutter-style workspaces have no direct macOS analog (Spaces are the closest, but they're not first-class targets the way Mutter workspaces are). Power commands could be driven via `osascript "tell application System Events to ..."` for Shut Down / Restart / Log Out / Sleep, but the wiring isn't in this slice.
