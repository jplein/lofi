#!/usr/bin/env bash
# Launch the most recently built LoFi.app.
#
# Used after `./build.sh` to verify end-to-end behavior: the borderless
# floating panel should appear centered, listing every .app discovered
# under `/Applications` and `~/Applications`.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_PATH="${SCRIPT_DIR}/build/Build/Products/Debug/LoFi.app"

if [ ! -d "$APP_PATH" ]; then
  echo "run.sh: $APP_PATH not found." >&2
  echo "Run ./build.sh first to produce the bundle." >&2
  exit 1
fi

open "$APP_PATH"
