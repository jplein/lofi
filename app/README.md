# app

The LoFi launcher application, written in Rust.

Code in this directory (outside of `gnome/` and `macos/`) is shared between platforms: the core data model, fuzzy matching, configuration loading, and anything else that doesn't depend on a specific window system or desktop environment.

## Layout

- `core/` — platform-agnostic shared crate (`lofi-core`). Holds the cross-platform data model (`Application`, `Window`, `Entry`, `EntryKind`, `EntryRef`), the `resolve` helper that pairs persisted references back to live entries, and `matcher::search` (Skim-style fuzzy ranking over `&[Entry]`). Also exposes a C ABI (the `ffi` Cargo feature) consumed by the macOS frontend — see `core/README.md` for the runtime/persistence type split and the FFI surface.
- `gnome/` — Linux/GNOME-specific code: the GTK4 + libadwaita launcher window (`ui`), `.desktop` enumeration (`apps`), activation via `gio_unix::DesktopAppInfo` and the extension proxy (`launch`), and the `windows` module — a blocking `zbus` client to the LoFi GNOME extension for window enumeration and focus/activation. The extension surface itself lives in `extension/gnome/`.
- `macos/` — macOS-specific code. Swift + AppKit on top of `lofi-core` as a `staticlib`, built by Bazel (`rules_rust` + `rules_swift` + `rules_apple`). Shows a borderless `NSPanel` listing `.app` bundles under `/System/Applications`, `/Applications`, and `~/Applications`, with a fuzzy-filtering search field. With Screen Recording + Accessibility granted it also gathers open windows (raising them via the Accessibility API) and the fourteen window-action commands (move/resize/minimize/fullscreen/maximize on the frontmost non-LoFi window). Same data-flow pattern as GNOME: the platform layer discovers and pushes into the Rust-owned entry list. See `macos/README.md`.

## Shared concerns

The shared layer defines the uniform item type that the platform layers populate and the UI renders:

- Applications (launchable desktop entries / `.app` bundles)
- Open windows
- Workspaces
- Commands (power management, lock screen, arbitrary user-defined commands)

Each platform implementation gathers these into the shared type so the presentation and matching logic stays platform-agnostic.

## Checks

The Rust toolchain comes from a different place on each platform, so the check commands differ. **Bazel is macOS-only** — on Linux the toolchain (and the reproducible build) come from Nix via direnv + `flake.nix`, and Bazel is not installed at all. (On the macOS/Bazel path `cargo` is editor tooling only, not the build/check front door.)

### Linux — Cargo (direnv + `flake.nix`)

Run from `app/`. These cover the whole workspace, including the Linux-only `gnome` crate:

- `cargo test` — unit tests + `tests/mru.rs` (add `-p lofi-core --features ffi` to also run `tests/ffi.rs`)
- `cargo clippy --all-targets`
- `cargo fmt --check`

### macOS — Bazel

One command runs the full check matrix — every Rust unit test, the FFI integration test, clippy (warnings promoted to errors), rustfmt, **and** swift-format lint over the Swift frontend:

- `bazelisk test //app/...`

That single invocation is the gate; there is no separate Rust-only or Swift-only step on the macOS path. Under the hood it's wired three ways: rules_rust aspects (`//app/core:rustfmt`, clippy promoted via build-time flag, `cargo_test`-style tests for `ffi_test` / `lofi_core_lib_test` / `mru_test`), and a coarse `sh_test` for Swift formatting (`//app/macos:swift_format_test`, sandboxed `xcrun swift-format lint --strict` over `app/macos/Sources/`). For how the Bazel Rust targets are wired (and why), see [core/README.md](core/README.md#tests-clippy-and-rustfmt); for the Swift sh_test's `DEVELOPER_DIR` plumbing, see [macos/README.md](macos/README.md#formatting--linting).

Only `core` builds under Bazel; the `gnome` crate is Linux-only (gtk4 / libadwaita) and has no Bazel target, so it is covered by the Cargo path above.

`app/macos/check.sh --fix` is a companion to the Bazel gate, not a separate check: it runs `swift-format format --in-place` to *rewrite* the sources, which the Bazel `sh_test` (sandboxed, read-only) cannot do. Bare `./check.sh` is no longer a lint entry point.
