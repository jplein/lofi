#!/usr/bin/env bash
# Quit any running LoFi.app instance. Tries a graceful AppleScript quit
# first so the app can shut down its SQLite stores cleanly, falls back
# to SIGTERM via `pkill` if the app hasn't exited within ~500ms. "No
# instance running" is success — `:close` is intended to be safely
# idempotent.

set -euo pipefail

osascript -e 'tell application id "dev.jplein.lofi" to quit' 2>/dev/null || true

for _ in 1 2 3 4 5; do
    pgrep -x LoFi >/dev/null 2>&1 || exit 0
    sleep 0.1
done
pkill -x LoFi 2>/dev/null || true
