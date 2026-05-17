#!/usr/bin/env bash
# Bazel wrapper around `app/macos/run.sh`. Same rationale as
# `app/macos/bazel/build.sh`: `bazel run` rewrites the script's
# location, so we redirect through `BUILD_WORKSPACE_DIRECTORY`.

set -euo pipefail
: "${BUILD_WORKSPACE_DIRECTORY:?must be invoked via 'bazel run'}"
exec "${BUILD_WORKSPACE_DIRECTORY}/app/macos/run.sh" "$@"
