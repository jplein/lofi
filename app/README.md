# app

The LoFi launcher application, written in Rust.

Code in this directory (outside of `linux/` and `macos/`) is shared between platforms: the core data model, fuzzy matching, MRU persistence, and anything else that doesn't depend on a specific window system or desktop environment.

Configuration is only half among them. Each platform has its own file with its own keys, and the two share no struct: the Linux file (`linux/src/config.rs`, `~/.config/lofi/config.toml`) carries a lock command and the border/corner-radius/shadow the launcher has to draw for itself on a `wlr-layer-shell` surface — GTK-typed values with no macOS counterpart, since the `NSPanel` gets its chrome from AppKit — while the macOS file (`~/Library/Preferences/LoFi/config.toml`, written with commented defaults on first use if absent) carries a window-command `gaps` preference. The macOS file's *parser* nevertheless lives in `core/` (`core/src/config.rs`), not in `macos/`: Swift has no TOML parser, and the Rust core already has `serde`, already reads the launcher's other files, and already has the C ABI to hand the values back. The Linux parser stays in `linux/` because its types (GTK colours) belong to GTK. If a key ever becomes genuinely shared, the shared part moves to `core/` and both files embed it.

## Layout

- `core/` — platform-agnostic shared crate (`lofi-core`). Holds the cross-platform data model (`Application`, `Window`, `Entry`, `EntryKind`, `EntryRef`), the `resolve` helper that pairs persisted references back to live entries, `matcher::search` (Skim-style fuzzy ranking over `&[Entry]`), and the parser for the macOS launcher's `config.toml` (`config`). Also exposes a C ABI (the `ffi` Cargo feature) consumed by the macOS frontend — see `core/README.md` for the runtime/persistence type split and the FFI surface.
- `linux/` — Linux-specific code, serving **both GNOME and Niri from one `lofi` binary**: the GTK4 + libadwaita launcher window (`ui`), `.desktop` enumeration (`apps`), the activation dispatch point (`launch`), the user's `~/.config/lofi/config.toml` (`config`), and `backend/` — the desktop seam. `backend::detect` picks between a blocking `zbus` client for the LoFi GNOME extension (whose shell-side half lives in `extension/gnome/`) and a JSON-over-unix-socket client for Niri's own IPC, at run time, from `$NIRI_SOCKET` / `$XDG_CURRENT_DESKTOP`. See `linux/README.md`.
- `macos/` — macOS-specific code. Swift + AppKit on top of `lofi-core` as a `staticlib`, built by Bazel (`rules_rust` + `rules_swift` + `rules_apple`). Shows a borderless `NSPanel` listing `.app` bundles under `/System/Applications`, `/Applications`, and `~/Applications`, with a fuzzy-filtering search field. With the Accessibility TCC grant in place it also surfaces the fourteen window-action commands (move/resize/minimize/fullscreen/maximize on the foreground non-LoFi window) — the target window is read via AX rather than CGWindowList, so Screen Recording is deliberately not requested. Reads the user's `~/Library/Preferences/LoFi/config.toml` on every summon, through the core's parser. Same data-flow pattern as Linux: the platform layer discovers and pushes into the Rust-owned entry list. See `macos/README.md`.

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

## Rust toolchain version

Two places pin the Rust version and they must agree:

- `rust-toolchain.toml` at the repo root — read by rustup-style tooling, by the Nix flake (`rust-bin.fromRustupToolchainFile`), and therefore by the whole Linux/Cargo path.
- rules_rust's **default** toolchain version on the macOS/Bazel path. `MODULE.bazel` does not call `rust.toolchain(versions = ...)`, so Bazel builds with whatever `DEFAULT_RUST_VERSION` the pinned `rules_rust` release ships (0.74.0 → Rust 1.98.0). Bumping Rust on macOS therefore means bumping `rules_rust`, not editing a version string.

**The floor is Rust 1.98.** Rust 1.95 through 1.97 cannot build this repo on macOS 27 with Xcode 27: Cargo's release profile (and rules_rust's `opt` exec configuration, which is what builds proc-macro crates "for tool") strips debuginfo with rustc's bundled LLVM `rust-objcopy`, and that objcopy wrote the Mach-O `__LINKEDIT` string pool at a 4-byte-aligned offset when Xcode 27's linker emitted an odd-length indirect symbol table. macOS 27's dyld rejects such a dylib ("mis-aligned LINKEDIT string pool"), so rustc cannot `dlopen` any stripped proc-macro — `thiserror_impl`, `serde_derive`, … — and every build dies with `error[E0463]: can't find crate for \`thiserror_impl\``. The misleading part is the message: the dylib exists on disk, rustc just could not load it. Rust 1.98 ships the fixed objcopy. The alternative workaround (`strip = "none"` / `strip_level` on the exec toolchain) was rejected because it only papers over the symptom on one platform; moving the pin fixes it everywhere and keeps Nix and Bazel on the same compiler.
