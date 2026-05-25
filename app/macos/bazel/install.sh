#!/usr/bin/env bash
# Install LoFi.app into ~/Applications and register a per-user
# LaunchAgent so the long-running daemon starts at login. Idempotent:
# re-running unloads any existing agent, replaces the .app, and
# bootstraps a fresh agent.
#
# Why this is a Bazel target rather than a Nix / home-manager module:
# the macOS build pipeline lives entirely in Bazel + Xcode (the Nix
# devShell on Darwin is just bazelisk + a Rust toolchain — Nix on
# macOS does not ship a usable Swift toolchain). A Nix derivation
# that drove `bazelisk` would have to be impure (`__noChroot`) to
# reach Xcode and the network. Keeping the install path inside Bazel
# avoids that compromise; the LaunchAgent plist is the only macOS-
# specific config we're writing.

set -euo pipefail
: "${BUILD_WORKSPACE_DIRECTORY:?must be invoked via 'bazel run'}"

APP_DIR="${HOME}/Applications"
APP_PATH="${APP_DIR}/LoFi.app"
AGENT_LABEL="dev.jplein.lofi"
AGENT_PATH="${HOME}/Library/LaunchAgents/${AGENT_LABEL}.plist"

RUNFILES_DIR="${RUNFILES_DIR:-${0}.runfiles}"
ZIP="${RUNFILES_DIR}/_main/app/macos/LoFi.zip"

# 1. Unload any existing agent. `bootout` deregisters the service from
#    launchd's table *and* sends SIGTERM to its running process, so a
#    previously-installed LoFi exits cleanly before we replace its
#    bundle. Ignore failures — the service may not be loaded.
launchctl bootout "gui/$UID/${AGENT_LABEL}" 2>/dev/null || true

# 2. Belt-and-suspenders: if a LoFi instance was started outside
#    launchd (e.g. via `:launch` for dev), kill it too. Same logic
#    as `:close` — graceful osascript quit, 500ms grace, SIGTERM
#    fallback.
osascript -e "tell application id \"${AGENT_LABEL}\" to quit" 2>/dev/null || true
for _ in 1 2 3 4 5; do
    pgrep -x LoFi >/dev/null 2>&1 || break
    sleep 0.1
done
pkill -x LoFi 2>/dev/null || true

# 3. Install the freshly-built bundle into ~/Applications. (Using the
#    user's Applications dir rather than /Applications keeps the
#    install sudo-free; the LaunchAgent runs as the user anyway, so
#    a user-local install matches the agent's privilege scope.)
mkdir -p "$APP_DIR"
rm -rf "$APP_PATH"
unzip -qq "$ZIP" -d "$APP_DIR"

# 4. Write the LaunchAgent plist.
#
# `RunAtLoad = true` is what makes the daemon start at login (and
# again whenever the agent is freshly bootstrapped).
#
# `KeepAlive = { SuccessfulExit = false }` is the nuance: launchd
# will restart LoFi if it *crashes* (non-zero exit), but a clean
# exit (Cmd-Q, `:close`, `osascript quit`) stays quit until next
# login or a `launchctl kickstart`. The plain `KeepAlive = true`
# form would fight Cmd-Q — every quit would resurrect the daemon
# immediately, which is more annoying than helpful for a launcher
# the user can re-summon any time.
#
# `ProcessType = Interactive` tells launchd this is a user-facing
# process (not a batch worker), so QoS / nice scheduling matches
# foreground apps rather than background daemons.
mkdir -p "$(dirname "$AGENT_PATH")"
cat > "$AGENT_PATH" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyLists-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${APP_PATH}/Contents/MacOS/LoFi</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
EOF

# 5. Register the agent. `bootstrap` is the modern equivalent of the
#    legacy `launchctl load -w`; it adds the service to launchd's
#    user-domain table and, because of `RunAtLoad`, starts the
#    process immediately.
launchctl bootstrap "gui/$UID" "$AGENT_PATH"

cat <<EOF

LoFi installed.
  App:     ${APP_PATH}
  Agent:   ${AGENT_PATH}
  Running: yes (launchd will resume at next login)

Press Option+Space to summon.
Cmd-Q quits cleanly — launchd won't restart until next login.
Re-launch manually with:  launchctl kickstart gui/\$UID/${AGENT_LABEL}
EOF
