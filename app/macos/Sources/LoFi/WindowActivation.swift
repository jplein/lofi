// Window activation via the Accessibility API.
//
// Why AX over `NSRunningApplication.activate()` alone: activating the
// owning app brings *some* window of that app forward, but not
// necessarily the one the user picked from the launcher. The Accessibility
// `kAXRaiseAction` raises a specific `AXUIElement` (one window) to the
// top of the z-order within its application. We do both:
//   1. Find the matching AX window by title, perform `kAXRaiseAction`.
//   2. Call `NSRunningApplication.activate()` so the owning app becomes
//      key — without this, the raised window's chrome is on top but
//      keyboard input still goes to whatever the previous focused app
//      was.
//
// Brittleness: title matching is exact-string. When an app has two
// windows with the same title (e.g. "Untitled — TextEdit"), the first
// one returned by `kAXWindowsAttribute` wins. macOS does not expose a
// stable mapping from CGWindowID to AXUIElement through the public API;
// the private `_AXUIElementGetWindow` can do it, but pulling that in is
// out of scope for this slice. If first-match-wins bites users in
// practice, that's the next direction to explore.

import AppKit
import ApplicationServices

enum WindowActivation {
    /// Raise the window with the given title belonging to the process at
    /// `pid`. Returns `true` on success, `false` if the AX call chain
    /// fails or no AX window with the given title exists.
    ///
    /// Calls `NSRunningApplication.activate()` after the raise so the
    /// owning app becomes key (without that, raising puts the window on
    /// top in z-order but keyboard focus stays with the previous app).
    static func raise(pid: pid_t, title: String) -> Bool {
        let app = AXUIElementCreateApplication(pid)

        var windowsValue: CFTypeRef?
        let copyErr = AXUIElementCopyAttributeValue(
            app,
            kAXWindowsAttribute as CFString,
            &windowsValue
        )
        guard copyErr == .success, let windowsArray = windowsValue as? [AXUIElement] else {
            return false
        }

        for window in windowsArray {
            var titleValue: CFTypeRef?
            let titleErr = AXUIElementCopyAttributeValue(
                window,
                kAXTitleAttribute as CFString,
                &titleValue
            )
            guard titleErr == .success,
                  let windowTitle = titleValue as? String,
                  windowTitle == title
            else {
                continue
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

        return false
    }
}
