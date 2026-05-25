#!/usr/bin/env bash
#
# Swift formatting gate for the macOS frontend.
#
# Uses Apple's swift-format, which ships *inside* the Xcode toolchain
# (`xcrun swift-format`) — there is no separate tool to install or pin. That is
# the same reasoning behind running clippy/rustfmt through the rules_rust
# toolchain rather than a parallel cargo install (see app/core/README.md):
# reuse the toolchain the build already depends on. swift-format is both the
# formatter and the linter, so this one script covers both roles.
#
# Usage:
#   ./check.sh         Lint only — check formatting, non-zero exit on any
#                      deviation. This is the gate (CI / pre-commit).
#   ./check.sh --fix   Reformat the sources in place.
#
# Not wired into `bazel test`: rules_swift has no swift-format aspect, so this
# runs standalone. The Rust gates remain `bazelisk test //app/...`.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
config="$here/.swift-format"
sources="$here/Sources"

if [[ "${1:-}" == "--fix" ]]; then
    exec xcrun swift-format format --in-place --parallel --recursive \
        --configuration "$config" "$sources"
fi

exec xcrun swift-format lint --strict --parallel --recursive \
    --configuration "$config" "$sources"
