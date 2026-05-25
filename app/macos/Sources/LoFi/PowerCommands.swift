// System-level power commands: Lock, Log Out, Sleep, Restart, Shutdown.
//
// Each command corresponds to a `PowerCommandKind` from the shared core
// (`app/core/src/lib.rs`). The launcher pushes a row per kind unconditionally
// — they don't depend on the focused window or any runtime state — and this
// file dispatches by id when the user activates one.
//
// Dispatch mechanism
// ------------------
// On GNOME each command is a one-shot D-Bus method call (see
// `app/gnome/src/power.rs`). On macOS the equivalents are split across two
// surfaces:
//
//   - **Lock**: synthesize the ⌃⌘Q keystroke via `CGEvent`. ⌃⌘Q is
//     macOS's standard Lock Screen shortcut (Apple menu → Lock Screen)
//     and `CGEvent.post(tap:)` is the documented way to inject input
//     events. Works with the Accessibility permission LoFi already holds
//     for the window-action commands. The older `CGSession -suspend`
//     binary lived at `/System/Library/CoreServices/Menu Extras/User.menu/
//     Contents/Resources/CGSession` but was removed in macOS Sonoma /
//     Tahoe; there is no stable command-line replacement.
//
//   - **Sleep**: shell out to `pmset sleepnow`. `pmset` accepts `sleepnow`
//     for the active user without sudo (unlike `shutdown` which needs
//     root). No confirmation dialog — matches the GNOME `Suspend` flow
//     (which also skips the polkit prompt via `interactive=false`).
//
//   - **Log Out / Restart / Shutdown**: NSAppleScript sending raw Apple
//     events directly to `loginwindow`. The four-letter event codes are
//     standard `kAE*` constants (`rlgo` / `rrst` / `rsdn`) that
//     loginwindow has honored since the classic Mac OS era. Routing
//     through loginwindow (rather than `tell application "System Events"
//     to ...`) is intentional: the System Events path requires the user
//     to grant **Automation** TCC permission to LoFi, which is an extra
//     prompt on top of Screen Recording + Accessibility. Direct Apple
//     events to loginwindow don't go through that gate, so all three
//     commands work on first use with no additional prompts. macOS still
//     shows its own native "Are you sure?" dialog with the 60-second
//     countdown for these three — matching GNOME's behavior where the
//     same three commands raise the standard confirmation dialog (we
//     route through GNOME SessionManager for the same reason).
//
// We use `NSAppleScript` in-process rather than shelling out to
// `osascript` for the Apple-event paths because `osascript` would itself
// be the source app for Automation TCC, which would require granting
// /usr/bin/osascript access (or LoFi access, depending on how TCC
// attributes the call) — confusing for the user. In-process keeps the
// caller identity stable (LoFi itself) and avoids the extra hop.

import AppKit
import Carbon.HIToolbox
import CoreGraphics
import Foundation

enum PowerCommands {
    /// Dispatch the power command identified by `id` — a stable
    /// `PowerCommandKind::as_id` string (e.g. `"lock_session"`,
    /// `"shutdown"`). Returns `true` when the underlying call returned
    /// success; `false` for unrecognized ids or a failure from the
    /// AppleScript / Process layer. The launcher already hides the panel
    /// when an entry is activated, so a `false` here is effectively
    /// degrade-silently.
    @discardableResult
    static func activate(id: String) -> Bool {
        switch id {
        case "lock_session":
            return lockScreen()
        case "suspend":
            return runProcess(path: "/usr/bin/pmset", args: ["sleepnow"])
        case "logout":
            // `aevtrlgo` is `kAELogOut` — loginwindow's standard logout
            // path, raises the system "Are you sure?" dialog with the
            // 60-second countdown (matching GNOME parity).
            return runAppleEvent(event: "aevtrlgo")
        case "restart":
            // `aevtrrst` is `kAERestart` — same flow as logout but
            // restarts the machine.
            return runAppleEvent(event: "aevtrrst")
        case "shutdown":
            // `aevtrsdn` is `kAEShutDown` — same flow as logout/restart
            // but shuts the machine down.
            return runAppleEvent(event: "aevtrsdn")
        default:
            return false
        }
    }

    /// Lock the screen by synthesizing the system-wide ⌃⌘Q shortcut.
    ///
    /// ⌃⌘Q is the macOS Lock Screen shortcut (Apple menu → Lock Screen);
    /// the menu item invokes the same internal path. `CGEvent.post(tap:)`
    /// is the documented input-injection API and works with the
    /// Accessibility permission LoFi already holds — synthetic keyboard
    /// events posted to the HID event tap are gated by Accessibility,
    /// the same TCC category that lets us drive other apps' windows via
    /// AX.
    ///
    /// Why two events: macOS Lock Screen fires on the *key-down* of the
    /// shortcut, but posting only the down event leaves the keys "held"
    /// from the window server's perspective (any subsequent typing
    /// inherits the modifier flags). Posting the matching key-up
    /// immediately after restores normal state for the resumed session.
    /// We post the modifier flags on the Q event itself rather than
    /// posting separate flagsChanged events for Control and Command —
    /// `CGEvent` interprets the `flags` field as "modifiers active for
    /// this event," and the lock handler reads it from there.
    ///
    /// Returns `false` only on `CGEvent` allocation failure (out of
    /// memory) — `.post` itself has no return path; the lock either
    /// happens immediately or the user lacks Accessibility permission
    /// (in which case nothing visible happens and the launcher quietly
    /// returns to the background — same shape as the AX-permission
    /// fallback for the window-action commands).
    private static func lockScreen() -> Bool {
        let source = CGEventSource(stateID: .hidSystemState)
        let keyQ = CGKeyCode(kVK_ANSI_Q)
        guard
            let down = CGEvent(keyboardEventSource: source, virtualKey: keyQ, keyDown: true),
            let up = CGEvent(keyboardEventSource: source, virtualKey: keyQ, keyDown: false)
        else {
            return false
        }
        down.flags = [.maskControl, .maskCommand]
        up.flags = [.maskControl, .maskCommand]
        down.post(tap: .cghidEventTap)
        up.post(tap: .cghidEventTap)
        return true
    }

    /// Run an in-process `NSAppleScript` against `loginwindow` with the
    /// given four-byte raw Apple event code (e.g. `"aevtrsdn"` for
    /// shutdown). Going through `loginwindow` directly — rather than
    /// `tell application "System Events" to ...` — avoids the Automation
    /// TCC prompt that the System Events path would trigger on first
    /// use, since loginwindow is exempt from that gate. The standard
    /// macOS "Are you sure?" confirmation dialog still appears for
    /// destructive actions (logout/restart/shutdown).
    private static func runAppleEvent(event: String) -> Bool {
        let source = "tell application \"loginwindow\" to «event \(event)»"
        guard let script = NSAppleScript(source: source) else {
            return false
        }
        var errInfo: NSDictionary?
        let result = script.executeAndReturnError(&errInfo)
        if let err = errInfo {
            NSLog("PowerCommands.runAppleEvent(\(event)): \(err)")
            return false
        }
        // `result` is a valid (possibly empty) descriptor on success;
        // the absence of `errInfo` is the success signal.
        _ = result
        return true
    }

    /// Spawn `path` with `args` and return whether the process launched
    /// cleanly. We do not wait for termination — `Process.run` is enough
    /// to fire-and-forget commands like `pmset sleepnow` and
    /// `CGSession -suspend` that take effect on the OS side. Failure to
    /// launch (binary missing, permission denied) returns false and is
    /// logged for diagnosis.
    private static func runProcess(path: String, args: [String]) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = args
        do {
            try process.run()
            return true
        } catch {
            NSLog("PowerCommands.runProcess(\(path)): \(error)")
            return false
        }
    }
}
