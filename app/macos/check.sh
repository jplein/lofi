#!/usr/bin/env bash
#
# In-place Swift reformat for the macOS frontend.
#
# The Swift lint *gate* runs under Bazel as `//app/macos:swift_format_test`
# (gated as part of `bazelisk test //app/...`). This script is only the
# `--fix` companion: swift-format's `format --in-place` mode rewrites
# sources, which a Bazel `sh_test` (sandboxed, read-only) cannot do.
#
# Uses Apple's swift-format from inside the Xcode toolchain
# (`xcrun swift-format`) — same reasoning as the Rust gates going through
# the rules_rust toolchain rather than a parallel cargo install: reuse the
# toolchain the build already depends on.
#
# Usage:
#   ./check.sh --fix   Reformat the sources in place.
set -euo pipefail

if [[ "${1:-}" != "--fix" ]]; then
    echo "usage: $(basename "$0") --fix" >&2
    echo "" >&2
    echo "Lint runs under Bazel; use 'bazelisk test //app/macos:swift_format_test'." >&2
    exit 2
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec xcrun swift-format format --in-place --parallel --recursive \
    --configuration "$here/.swift-format" "$here/Sources"
