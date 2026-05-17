#!/usr/bin/env bash
# Build the LoFi macOS app end-to-end.
#
# Stages:
#   1. cargo build the Rust static library + C header (`lofi-core` with
#      the `ffi` feature) for `aarch64-apple-darwin`.
#   2. `xcodegen generate` to materialize `LoFi.xcodeproj` from
#      `project.yml`.
#   3. `xcodebuild` Debug → `build/Build/Products/Debug/LoFi.app`.
#
# Flags:
#   --rust-only   Stop after stage 1. Used by the Xcode pre-build Run
#                 Script Phase, which must produce the Rust artifacts
#                 but must not recurse back into xcodebuild.
#   --no-rust     Skip stage 1. Useful when you've already built the
#                 staticlib by hand and want a fast incremental Xcode
#                 build.
#
# PATH gotcha: Xcode Run Script Phases run with a minimal PATH that
# does NOT include `$HOME/.nix-profile/bin` (where cargo, cbindgen,
# xcodegen all live on a Nix-on-Darwin setup). We prepend the known
# Nix and Homebrew locations explicitly so the script works whether
# invoked from a shell or from Xcode.

set -euo pipefail

# Prepend the locations where cargo/xcodegen are likely installed so
# Xcode's stripped-down PATH still finds them. Existing PATH stays at
# the end so user-installed tools keep working.
export PATH="$HOME/.nix-profile/bin:/run/current-system/sw/bin:/usr/local/bin:/opt/homebrew/bin:${PATH:-}"

# Resolve the repo root from the script's own location so the script
# works no matter where it's invoked from (shell cwd, Xcode SRCROOT,
# CI checkout).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

DO_RUST=1
DO_XCODE=1
for arg in "$@"; do
  case "$arg" in
    --rust-only)
      DO_RUST=1
      DO_XCODE=0
      ;;
    --no-rust)
      DO_RUST=0
      DO_XCODE=1
      ;;
    *)
      echo "build.sh: unknown argument: $arg" >&2
      echo "usage: build.sh [--rust-only | --no-rust]" >&2
      exit 2
      ;;
  esac
done

if [ "$DO_RUST" = "1" ]; then
  echo "==> Building lofi-core (Rust)"
  (
    cd "${REPO_ROOT}/app"
    cargo build \
      --release \
      -p lofi-core \
      --features ffi \
      --target aarch64-apple-darwin
  )
fi

if [ "$DO_XCODE" = "1" ]; then
  echo "==> xcodegen generate"
  (
    cd "${SCRIPT_DIR}"
    xcodegen generate
  )

  echo "==> xcodebuild"
  (
    cd "${SCRIPT_DIR}"
    xcodebuild \
      -scheme LoFi \
      -configuration Debug \
      -derivedDataPath build \
      build
  )
fi
