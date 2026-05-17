# Bazel migration — option 2 (native rules)

## Context

The first Bazel pass wrapped `build.sh` and `run.sh` as `sh_binary` targets — a beachhead, but it bought nothing beyond a nicer command surface. This slice replaces that with real Bazel rules so we get caching, incrementality, hermeticity, and a real build graph.

Scope is **macOS only**. The Linux/GNOME build keeps its `cargo build -p lofi-gnome` path through Crane in `flake.nix`. Bazel reads `app/Cargo.lock` via `crate_universe` so dependency versions stay coherent between the two paths.

## Decisions baked in

- **rules_rust + crate_universe** for Rust deps. Reads `app/Cargo.lock`.
- **rules_swift + rules_apple** for Swift and the `.app` bundle.
- **cbindgen runs as a Bazel rule**, not via `build.rs`. The cbindgen binary itself is built from `Cargo.lock` (it's already a build-dependency of `lofi-core`).
- **Retire `build.sh` / `run.sh` / `project.yml` / xcodegen / xcodebuild.** One front door (Bazel).
- **No `rules_xcodeproj` yet** — opening the project in Xcode for debugging is a follow-up. Day-to-day editing happens in the editor of choice; building happens via `bazel build`.

## Architecture

### `MODULE.bazel`

Add bazel_deps:
- `rules_rust` (latest stable; check release notes for compatible version)
- `rules_swift`
- `rules_apple`
- `rules_cc`

Configure `crate_universe`:
- `from_cargo` extension pointing at `app/Cargo.lock` and `app/Cargo.toml`
- Generate a lockfile `MODULE.bazel.lock` plus a `Cargo.Bazel.lock` for crate_universe's regeneration step
- Define an alias for the `cbindgen` binary so we can `bazel run @crates//:cbindgen__cli` (or whatever the conventional name is)

### `app/core/BUILD.bazel`

- `rust_static_library` for `lofi_core`
  - `srcs` glob over `src/**/*.rs`
  - `crate_features = ["ffi"]`
  - `edition = "2024"`
  - `deps = ["@crates//:serde", "@crates//:serde_json", "@crates//:fuzzy-matcher", "@crates//:rusqlite"]` (exact target names per crate_universe output)
  - Skip the existing `build.rs` — cbindgen runs separately as a genrule, and Cargo build script directives don't apply under Bazel
- `genrule` to run cbindgen
  - `tools = ["@crates//:cbindgen__cli"]`
  - `srcs` = `glob(["src/**/*.rs"])` + `cbindgen.toml`
  - `outs = ["include/lofi_core.h"]`
  - `cmd = "$(execpath @crates//:cbindgen__cli) --config $(execpath cbindgen.toml) --crate lofi-core --output $@ $(execpath src/lib.rs)"`
- `cc_library` named `lofi_core_cc`
  - `hdrs = [":include/lofi_core.h"]` (the genrule's output)
  - `includes = ["include"]`
  - `linkstatic = True`
  - `srcs` includes the `liblofi_core.a` produced by the `rust_static_library` (via `:lofi_core.a` or a filegroup)
- `rust_test` for `tests/ffi.rs`
  - `crate_features = ["ffi"]`
  - Same deps as `lofi_core` plus `lofi_core` itself

### `app/macos/BUILD.bazel`

- `swift_library` named `LoFiLib`
  - `srcs = glob(["Sources/LoFi/*.swift"])`
  - `module_name = "LoFi"`
  - `objc_copts = []` and `swiftc_inputs` if needed for the bridging header
  - `deps = ["//app/core:lofi_core_cc"]`
  - `swift_settings = { "OBJC_BRIDGING_HEADER": "Sources/LoFi/LoFi-Bridging-Header.h" }` (exact attr per rules_swift conventions)
- `macos_application` named `LoFi`
  - `bundle_id = "dev.jplein.lofi"`
  - `infoplists = ["Resources/Info.plist"]`
  - `entitlements = "Resources/LoFi.entitlements"`
  - `minimum_os_version = "15.0"`
  - `deps = [":LoFiLib"]`
- `sh_binary` named `launch`
  - `srcs = ["bazel/launch.sh"]` (rewrite to invoke the Bazel-built `.app` from `bazel-bin/...`)

### Files to delete

- `app/macos/build.sh`
- `app/macos/run.sh`
- `app/macos/project.yml`
- `app/macos/bazel/build.sh` (the existing thin wrapper)
- `app/macos/LoFi.xcodeproj` (generated; was gitignored anyway)
- `app/core/build.rs` (only ran cbindgen)
- `app/core/cbindgen.toml` — *keep* (Bazel cbindgen genrule still uses it)

### Files to update

- `flake.nix` — remove `xcodegen` from the Darwin devShell (no longer used). Keep `bazelisk`.
- `app/core/Cargo.toml` — remove `cbindgen` from `[build-dependencies]` if we want a clean separation, OR keep it for the (now-defunct) build.rs and let cargo+crane ignore it. **Decision: keep** so `cargo build -p lofi-core --features ffi` still produces a header for anyone who wants to use the Cargo path (e.g. when porting to a non-Bazel environment). The Bazel build will use its own cbindgen invocation.
- `app/core/build.rs` — keep but gate the cbindgen call so Bazel doesn't trigger it (Bazel doesn't run build.rs for the workspace-root crate; this is moot)
- `app/macos/README.md` — rewrite the build/run section, document what changed.
- `app/core/README.md` — note the Bazel build path.
- `app/README.md` — note that macOS frontend is now Bazel-driven.
- Root `README.md` — mention the Bazel build for macOS.

## Build script tour for cbindgen under Bazel

cbindgen needs:
- The crate's source tree (`src/**/*.rs`)
- A config file (`cbindgen.toml`)
- Knowledge of the `ffi` feature being on (cbindgen has `--features ffi` flag)

Output: `lofi_core.h`.

The `genrule` reads sources, runs cbindgen, emits the header into Bazel's output tree. The `cc_library` exposes it to Swift via `hdrs` + `includes`.

## Verification

1. `nix develop` on Darwin, then `bazel build //app/macos:LoFi` — produces `bazel-bin/app/macos/LoFi.app`.
2. `bazel run //app/macos:launch` — opens the bundle.
3. `bazel test //app/core:ffi_test` — runs the 12 FFI integration tests; expect all pass.
4. Linux regression: `cargo build -p lofi-gnome` from inside the existing Crane-driven shell still succeeds (Bazel changes do not touch GNOME).
5. Open `LoFi.app` from Finder — same UI behaviour as the xcodebuild path produced.

## Risks / gotchas

1. **rusqlite-bundled compiles SQLite from C source** at build time via its build.rs. rules_rust's `cargo_build_script` machinery has to find a cc toolchain — Xcode CLT should provide it on macOS, but the cross-platform-target dance (`x86_64-apple-darwin` vs `aarch64-apple-darwin`) sometimes trips it.
2. **rules_apple version drift** — code signing defaults have changed between minor versions. With `LoFi.entitlements` empty and `LSUIElement=YES`, we're not exercising the harder cases, but pin the version.
3. **edition = "2024"** — confirm the chosen `rules_rust` version supports edition 2024 (introduced in Rust 1.85). Older rules_rust versions cap at 2021.
4. **cbindgen's `--features` arg** must be plumbed through so the FFI module is visible to the parser. The current build.rs handles this implicitly via cargo's `CARGO_FEATURE_FFI`; the Bazel genrule must pass it explicitly.
5. **Bridging header path** — Swift sees Rust types via `LoFi-Bridging-Header.h`, which `#include "lofi_core.h"`. The cc_library's `includes` attribute has to put the genrule's output dir on the search path so `#include "lofi_core.h"` resolves.

## Workflow status

- [x] MODULE.bazel + crate_universe wiring (versions pinned against actual BCR registry)
- [x] app/core/BUILD.bazel (rust_static_library + cbindgen genrule + cc_library + swift_interop_hint + rust_test)
- [x] app/macos/BUILD.bazel (swift_library + macos_application + launch)
- [x] Delete script-driven path (build.sh, run.sh, project.yml; xcodegen removed from flake)
- [x] Verify: `bazel build //app/macos:LoFi` (succeeds), `bazel run //app/macos:launch` (panel renders with 51 apps), `bazel test //app/core:ffi_test` (12/12 pass)
- [x] README updates (root, app/, app/core/, app/macos/)

Remaining (not blocking this slice):
- Linux GNOME regression check: `cargo build -p lofi-gnome` from inside the Linux Crane shell still succeeds. Not verifiable from the macOS environment; flag for the next time the user is on Linux.
- `DEVELOPER_DIR` override in `.envrc` so Bazel doesn't pick up the Nix-provided partial SDK. (Done.)
- Optional follow-up slice: `rules_xcodeproj` to regenerate a debuggable Xcode project from the Bazel graph.
