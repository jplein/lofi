# app

The LoFi launcher application, written in Rust.

Code in this directory (outside of `linux/` and `macos/`) is shared between platforms: the core data model, fuzzy matching, MRU persistence, and anything else that doesn't depend on a specific window system or desktop environment.

Configuration is deliberately *not* among them. The only settings LoFi has today are the Linux launcher's lock command and the border/corner-radius/shadow it has to draw for itself on a `wlr-layer-shell` surface — a Niri-shaped problem with no macOS counterpart, since the `NSPanel` gets its chrome from AppKit. So the config file lives in `linux/` (`linux/src/config.rs`), and moving it up here is a decision to make if and when a second platform wants to read one.

## Layout

- `core/` — platform-agnostic shared crate (`lofi-core`). Holds the cross-platform data model (`Application`, `Window`, `Entry`, `EntryKind`, `EntryRef`), the `resolve` helper that pairs persisted references back to live entries, and `matcher::search` (Skim-style fuzzy ranking over `&[Entry]`). Also exposes a C ABI (the `ffi` Cargo feature) consumed by the macOS frontend — see `core/README.md` for the runtime/persistence type split and the FFI surface.
- `linux/` — Linux-specific code, serving **both GNOME and Niri from one `lofi` binary**: the GTK4 + libadwaita launcher window (`ui`), `.desktop` enumeration (`apps`), the activation dispatch point (`launch`), the user's `~/.config/lofi/config.toml` (`config`), and `backend/` — the desktop seam. `backend::detect` picks between a blocking `zbus` client for the LoFi GNOME extension (whose shell-side half lives in `extension/gnome/`) and a JSON-over-unix-socket client for Niri's own IPC, at run time, from `$NIRI_SOCKET` / `$XDG_CURRENT_DESKTOP`. See `linux/README.md`.
- `macos/` — macOS-specific code. Swift + AppKit on top of `lofi-core` as a `staticlib`, built by Bazel (`rules_rust` + `rules_swift` + `rules_apple`). Shows a borderless `NSPanel` listing `.app` bundles under `/System/Applications`, `/Applications`, and `~/Applications`, with a fuzzy-filtering search field. With the Accessibility TCC grant in place it also surfaces the fourteen window-action commands (move/resize/minimize/fullscreen/maximize on the foreground non-LoFi window) — the target window is read via AX rather than CGWindowList, so Screen Recording is deliberately not requested. Same data-flow pattern as Linux: the platform layer discovers and pushes into the Rust-owned entry list. See `macos/README.md`.

## Shared concerns

The shared layer defines the uniform item type that the platform layers populate and the UI renders:

- Applications (launchable desktop entries / `.app` bundles)
- Open windows
- Workspaces
- Commands (window actions, workspace moves, power management, lock screen)

Each platform implementation gathers these into the shared type so the presentation and matching logic stays platform-agnostic.

The one place the shared layer has to know that platforms *disagree* is `CommandKind`. A window action means different things on a floating desktop and a scrolling tiler, so the enum is the union of every platform's vocabulary and each platform declares the subset it emits. See `core/README.md`.

## Checks

The Rust toolchain comes from a different place on each platform, so the check commands differ. **Bazel is macOS-only** — on Linux the toolchain (and the reproducible build) come from Nix via direnv + `flake.nix`, and Bazel is not installed at all. (On the macOS/Bazel path `cargo` is editor tooling only, not the build/check front door.)

### Linux — Cargo (direnv + `flake.nix`)

Run from `app/`. These cover the whole workspace, including the Linux-only `linux` crate:

- `cargo test` — unit tests + `tests/mru.rs` (add `-p lofi-core --features ffi` to also run `tests/ffi.rs`)
- `cargo clippy --all-targets`
- `cargo fmt --check`

### macOS — Bazel

One command runs the full check matrix — every Rust unit test, the FFI integration test, clippy (warnings promoted to errors), rustfmt, **and** swift-format lint over the Swift frontend:

- `bazelisk test //app/...`

That single invocation is the gate; there is no separate Rust-only or Swift-only step on the macOS path. Under the hood it's wired three ways: rules_rust aspects (`//app/core:rustfmt`, clippy promoted via build-time flag, `cargo_test`-style tests for `ffi_test` / `lofi_core_lib_test` / `mru_test`), and a coarse `sh_test` for Swift formatting (`//app/macos:swift_format_test`, sandboxed `xcrun swift-format lint --strict` over `app/macos/Sources/`). For how the Bazel Rust targets are wired (and why), see [core/README.md](core/README.md#tests-clippy-and-rustfmt); for the Swift sh_test's `DEVELOPER_DIR` plumbing, see [macos/README.md](macos/README.md#formatting--linting).

Only `core` builds under Bazel; the `linux` crate is Linux-only (gtk4 / libadwaita / gtk4-layer-shell) and has no Bazel target, so it is covered by the Cargo path above.

`app/macos/check.sh --fix` is a companion to the Bazel gate, not a separate check: it runs `swift-format format --in-place` to *rewrite* the sources, which the Bazel `sh_test` (sandboxed, read-only) cannot do. Bare `./check.sh` is no longer a lint entry point.
