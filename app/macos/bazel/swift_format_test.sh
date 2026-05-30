#!/usr/bin/env bash
# Bazel sh_test wrapper around `xcrun swift-format lint`. Wires Swift
# formatting into `bazelisk test //app/...` so the macOS gate matches
# the Rust path (which gates clippy + rustfmt via rules_rust aspects).
#
# Why a coarse sh_test rather than a per-target aspect: rules_swift has
# no swift-format aspect. A whole-tree lint is fine here — the macOS
# crate is small enough that re-running it on any Swift change is
# essentially free, and the coarser cache key keeps the wiring trivial.
#
# Runfiles layout: `data = glob([...Sources/**/*.swift]) + [".swift-format"]`
# in the BUILD file puts the source tree and the config under
# `$RUNFILES_DIR/_main/app/macos/` (same `_main` repo prefix
# `bazel/launch.sh` relies on).
set -euo pipefail

# Bazel's test sandbox does not export DEVELOPER_DIR, so `xcrun` fails with
# "unable to find sdk: 'macosx'" when run unmodified. Fall back to whatever
# `xcode-select -p` reports on the host. The BUILD file also `env_inherit`s
# DEVELOPER_DIR so a developer or CI can override the toolchain explicitly.
if [[ -z "${DEVELOPER_DIR:-}" ]]; then
    DEVELOPER_DIR="$(/usr/bin/xcode-select -p)"
    export DEVELOPER_DIR
fi

root="${RUNFILES_DIR}/_main/app/macos"

exec xcrun swift-format lint --strict --parallel --recursive \
    --configuration "${root}/.swift-format" \
    "${root}/Sources"
