#!/usr/bin/env bash
# Bazel wrapper around `app/macos/build.sh`.
#
# `bazel run` sets `BUILD_WORKSPACE_DIRECTORY` to the source repo root.
# The canonical `build.sh` resolves its own location via
# `BASH_SOURCE[0]`, which under Bazel points at the runfiles tree
# instead of the source tree — so we exec the real script through
# `BUILD_WORKSPACE_DIRECTORY` to give it the directory layout it
# expects.

set -euo pipefail
: "${BUILD_WORKSPACE_DIRECTORY:?must be invoked via 'bazel run'}"
exec "${BUILD_WORKSPACE_DIRECTORY}/app/macos/build.sh" "$@"
