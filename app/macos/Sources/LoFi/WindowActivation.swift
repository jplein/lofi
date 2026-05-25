// Window activation via the Accessibility API.
//
// Why AX over `NSRunningApplication.activate()` alone: activating the
// owning app brings *some* window of that app forward, but not
// necessarily the one the user picked from the launcher. The
// Accessibility API addresses windows directly via `AXUIElement`s.
// The sequence is:
//
//   1. Find the matching AX window by `CGWindowID` (title fallback).
//   2. `NSRunningApplication.activate()` — bring the owning app to
//      the foreground so keyboard focus follows.
//   3. `kAXRaiseAction` on the picked window — moves it to the top
//      of its app's z-order.
//   4. `kAXMain = true` and `kAXFocused = true` on the picked window
//      — covers the union of AppKit-style and Gecko/Chromium-style
//      "current window" conventions, so multi-window non-AppKit apps
//      (Firefox in particular) surface the window we picked rather
//      than their internally-tracked current window.
//
// Active-Space + active-display scope
// -----------------------------------
// The window list shown by the launcher is scoped to the active
// Space *and* the active display (`WindowDiscovery.discover` filters
// both), so every window we can activate is already on the user's
// current Space and current display. That keeps activation simple:
// no Space-switching, no cross-display focus-routing, no AX races
// — just the precise AX raise + the per-window main/focused writes,
// all of which the OS lets us do for a window on the same display
// as the foreground caller (us). See the README *Out of scope*
// section and gotchas 13-14 for the macOS limitations that drive
// each scope choice.
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
        guard
            let window = AXWindowFinder.match(
                in: windowsArray,
                windowId: windowId,
                title: title
            )
        else {
            return false
        }

        // Order: activate, raise, then per-window kAXMain +
        // kAXFocused. This is the order Hammerspoon and Rectangle
        // use; doing it the other way round (raise then activate)
        // makes `NSRunningApplication.activate()` restore the app's
        // previously-focused window and silently undo the raise.
        guard let running = NSRunningApplication(processIdentifier: pid) else {
            return false
        }
        running.activate()
        let raiseErr = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        guard raiseErr == .success else {
            return false
        }
        _ = AXUIElementSetAttributeValue(
            window, kAXMainAttribute as CFString, true as CFTypeRef
        )
        _ = AXUIElementSetAttributeValue(
            window, kAXFocusedAttribute as CFString, true as CFTypeRef
        )
        return true
    }
}
