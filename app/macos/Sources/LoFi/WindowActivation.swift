// Window activation via the Accessibility API.
//
// Why AX over `NSRunningApplication.activate()` alone: activating the
// owning app brings *some* window of that app forward, but not
// necessarily the one the user picked from the launcher. The
// Accessibility `kAXRaiseAction` raises a specific `AXUIElement` (one
// window) to the top of the z-order within its application. We do
// both:
//   1. Find the matching AX window by title, perform `kAXRaiseAction`.
//   2. Call `NSRunningApplication.activate()` so the owning app
//      becomes key — without that, raising puts the window on top in
//      z-order but keyboard focus stays with the previous app.
//
// Cross-Space limitation
// ----------------------
// When the target window is on another macOS Space, this code raises
// it in-place (on its own Space) but does not move the user there.
// macOS exposes no public API to programmatically switch Spaces:
// `NSRunningApplication.activate()` honors the user's "When switching
// to an application, switch to a Space with open windows for the
// application" preference (System Settings → Desktop & Dock) — when
// that's on, macOS follows; when off, the user stays put.
//
// The private SkyLight call `SLSManagedDisplaySetCurrentSpace` is what
// Yabai et al. use to force a Space switch. We tried it on macOS 26
// (Tahoe) and found it interacts badly with `kAXMainAttribute` set on
// a background-app window from a foreground caller: the system
// sometimes "fixes" the inconsistency by yanking the target window
// onto the current Space instead of moving the user to it, leaving
// the window in a broken Mission Control state. Until that's tracked
// down, we leave Space-following to macOS preference handling.
//
// AX-disabled apps
// ----------------
// Firefox (and some Gecko/Chromium derivatives) ship with the AX
// runtime asleep and report zero windows from `kAXWindowsAttribute`
// even when the app clearly has windows open. We kick it via
// `AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface",
// true)` before reading the windows list — the same wakeup
// VoiceOver and Mac scripting tools use. If the list is still empty
// after the kick, we fall back to bare `running.activate()`: the app
// comes forward and macOS picks one of its windows. With multiple
// windows we cannot target a specific one, but that's strictly
// better than dropping the activation.
//
// Finding the AX window
// ---------------------
// We bridge from our captured `CGWindowID` to an `AXUIElement` by
// exact id, via the private `_AXUIElementGetWindow` (see
// `AXWindowFinder.match` in `WindowControl.swift`), falling back to
// title matching only when an app's AX windows don't expose an id.
// Id matching is what makes this robust for apps that retitle (a
// terminal rewriting its title between gather and activation would
// defeat title matching). The AX-window enumeration (the
// `AXEnhancedUserInterface` kick + the `kAXWindowsAttribute` copy),
// the id bridge, and the title-fallback matcher all live in
// `AXWindowFinder` so `raise` and the command dispatch share one
// implementation.

import AppKit
import ApplicationServices

enum WindowActivation {
    /// Raise the window with the given title belonging to the process
    /// at `pid`. Returns `true` on success, `false` if the AX call
    /// chain fails or no AX window with the given title exists.
    ///
    /// Calls `NSRunningApplication.activate()` after the raise so the
    /// owning app becomes key (without that, raising puts the window
    /// on top in z-order but keyboard focus stays with the previous
    /// app).
    static func raise(pid: pid_t, windowId: CGWindowID, title: String) -> Bool {
        // Shared finder: kicks `AXEnhancedUserInterface` (Firefox /
        // Gecko / Chromium ship with the AX runtime asleep) and copies
        // `kAXWindowsAttribute`. See `AXWindowFinder` in
        // `WindowControl.swift`.
        let windowsArray = AXWindowFinder.windowsForApp(pid: pid)

        // Fast path: AX list is empty (Firefox without AX, sandboxed
        // app, etc.). We can't target a specific window — but we can
        // still bring the owning app forward, which is strictly
        // better than dropping the activation. Space-switching then
        // falls to macOS's "switch to Space with open windows"
        // preference (System Settings → Desktop & Dock).
        if windowsArray.isEmpty {
            guard let running = NSRunningApplication(processIdentifier: pid) else {
                return false
            }
            return running.activate()
        }

        // Match by CGWindowID first (immune to title drift), title as a
        // fallback. Same shared matcher the window-action commands use.
        guard let window = AXWindowFinder.match(
            in: windowsArray,
            windowId: windowId,
            title: title
        ) else {
            return false
        }
        let raiseErr = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        guard raiseErr == .success else {
            return false
        }
        // The raise put this window on top in its app's z-order; now
        // bring the owning app to the foreground so keyboard input
        // follows.
        guard let running = NSRunningApplication(processIdentifier: pid) else {
            // Window raised but we can't bring the app forward —
            // treat as partial success and return false so the
            // caller can decide.
            return false
        }
        running.activate()
        return true
    }
}
