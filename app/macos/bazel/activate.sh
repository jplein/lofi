#!/usr/bin/env bash
# Bring an already-running LoFi.app to the foreground. `open -b
# BUNDLEID` notices the bundle is already running and sends a re-open
# event (the Launch Services equivalent of a Dock click), which the
# AppDelegate will handle by summoning the panel once the global-hotkey
# slice lands.
#
# No-op when no instance is running — `:close` followed by `:launch` is
# the way to start one. We print a hint to stderr rather than
# transparently launching, because conflating activate-existing with
# launch-fresh would defeat the point of having separate targets.

set -euo pipefail

if ! pgrep -x LoFi >/dev/null 2>&1; then
    echo "lofi: no running instance; use ':launch' first" >&2
    exit 0
fi

open -b dev.jplein.lofi
