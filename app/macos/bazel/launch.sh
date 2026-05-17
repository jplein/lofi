#!/usr/bin/env bash
# Open the Bazel-built `LoFi.app` via `open`, which goes through Launch
# Services. This is the canonical macOS launch path — different from
# `bazel run //app/macos:LoFi`, which execs the binary directly and
# can subtly misbehave for `LSUIElement=YES` apps (activation state
# isn't set up the same way).
#
# rules_apple's primary `macos_application` output is `LoFi.zip`. The
# unarchived `LoFi_archive-root/LoFi.app` tree is also produced when
# the bundle target itself is built, but Bazel sometimes prunes it on
# dependent-target builds. We unzip on demand to a stable local cache
# so subsequent invocations are fast and `open` has a real .app to
# hand to Launch Services.

set -euo pipefail
: "${BUILD_WORKSPACE_DIRECTORY:?must be invoked via 'bazel run'}"

# The .zip is in our sh_binary's runfiles tree, not at the bazel-bin
# path the macos_application target normally lives at — Bazel doesn't
# promise to materialize a dependency's outputs at bazel-bin when only
# the dependent target is being run.
RUNFILES_DIR="${RUNFILES_DIR:-${0}.runfiles}"
ZIP="${RUNFILES_DIR}/_main/app/macos/LoFi.zip"
CACHE_DIR="${BUILD_WORKSPACE_DIRECTORY}/bazel-bin/app/macos/LoFi.launch-cache"

# Re-extract whenever the zip is newer than the cached bundle.
if [ ! -d "${CACHE_DIR}/LoFi.app" ] || [ "${ZIP}" -nt "${CACHE_DIR}/LoFi.app" ]; then
    rm -rf "${CACHE_DIR}"
    mkdir -p "${CACHE_DIR}"
    unzip -qq "${ZIP}" -d "${CACHE_DIR}"
fi

open "${CACHE_DIR}/LoFi.app"
