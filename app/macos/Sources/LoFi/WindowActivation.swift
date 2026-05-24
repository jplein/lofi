// Window activation via the Accessibility API.
//
// Why AX over `NSRunningApplication.activate()` alone: activating the
// owning app brings *some* window of that app forward, but not
// necessarily the one the user picked from the launcher. The
// Accessibility `kAXRaiseAction` raises a specific `AXUIElement` (one
// window) to the top of the z-order within its application. We do
// both:
//   1. Find the matching AX window by id (title fallback) and perform
//      `kAXRaiseAction`.
//   2. Call `NSRunningApplication.activate()` so the owning app
//      becomes key — without that, raising puts the window on top in
//      z-order but keyboard focus stays with the previous app.
//
// Active-Space scope
// ------------------
// The window list shown by the launcher is scoped to the **active
// Space** (`WindowDiscovery.discover` passes `.optionOnScreenOnly`), so
// every window we can activate is already on the user's current Space.
// That keeps activation simple: no Space-switching, no AX races, no
// gotcha 13 reconciliation surface — just the precise AX raise. See
// the README *Out of scope* section for why we don't try to drive
// cross-Space activation ourselves on macOS.
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
        // better than dropping the activation.
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
