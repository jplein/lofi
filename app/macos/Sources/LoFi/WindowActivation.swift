// Window activation via the Accessibility API.
//
// Why AX over `NSRunningApplication.activate()` alone: activating the
// owning app brings *some* window of that app forward, but not
// necessarily the one the user picked from the launcher. The
// Accessibility API addresses windows directly via `AXUIElement`s.
// The sequence is:
//
//   1. Find the matching AX window by `CGWindowID` (title fallback).
//   2. `NSRunningApplication.activate()` first — bring the owning
//      app to the foreground so keyboard focus follows. This MUST
//      happen before the per-window AX writes; doing it after makes
//      `activate()` restore the app's previously-focused window and
//      silently undo the raise (see gotcha 14).
//   3. `kAXRaiseAction` on the picked window — moves it to the top
//      of its app's z-order.
//   4. `kAXMain = true` and `kAXFocused = true` on the picked window
//      — covers the union of AppKit-style and Gecko/Chromium-style
//      "current window" conventions, so multi-window non-AppKit apps
//      (Firefox in particular) surface the window we picked rather
//      than their internally-tracked current window.
//
// Active-Space scope
// ------------------
// The window list shown by the launcher is scoped to the **active
// Space** (`WindowDiscovery.discover` passes `.optionOnScreenOnly`), so
// every window we can activate is already on the user's current Space.
// That keeps activation simple: no Space-switching, no AX races, no
// gotcha 13 reconciliation surface — just the precise AX raise + the
// main/focused writes. See the README *Out of scope* section for why
// we don't try to drive cross-Space activation ourselves on macOS.
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

        // Order: **activate the app first, then raise + set
        // main/focused on the picked window.** Doing it the other way
        // round (raise then activate) makes `NSRunningApplication.
        // activate()` restore whichever window the app considered
        // "main" before LoFi grabbed focus, which on multi-display
        // setups silently undoes the AX raise. Observed symptoms:
        //
        //   - Finder: picking "Dustbin" on display 2 raised Dustbin
        //     to the top of display 2's stack (good), but activate()
        //     also raised "Blue Steel HD" to the top of display 1's
        //     stack and made *Blue Steel HD* the focused window — not
        //     the one we picked.
        //   - Firefox: picking either of two windows consistently
        //     brought up the same Firefox window (the one that was
        //     front before LoFi), regardless of which the user
        //     chose.
        //
        // Activating first lets macOS settle the app's "previous"
        // main-window state, then the raise + main/focused writes
        // land as the final state. This is the order Hammerspoon,
        // Rectangle, and other AX-driven launchers use.
        guard let running = NSRunningApplication(processIdentifier: pid) else {
            return false
        }
        running.activate()

        let raiseErr = AXUIElementPerformAction(window, kAXRaiseAction as CFString)
        guard raiseErr == .success else {
            return false
        }

        // For native AppKit apps the raise above is sufficient; for
        // Gecko (Firefox) and a few other non-AppKit toolkits it is
        // not — they track "current window" independently of macOS
        // z-order. Setting both `kAXMain` and `kAXFocused` covers
        // the union of conventions: AppKit-style apps respect
        // `kAXMain` for the "this is the active document window"
        // semantics, while some Gecko/Chromium builds only honor
        // `kAXFocused` for keyboard-focus selection. Best-effort —
        // we don't check the return values because an app that
        // doesn't implement these attributes is the expected shape
        // for some non-AppKit toolkits, not a bug.
        //
        // The README's gotcha 13 warning against `kAXMainAttribute`
        // applies specifically to the (removed) cross-Space
        // SpaceManager flow where the write triggers macOS to yank
        // the target window onto the originating Space. LoFi is now
        // active-Space scoped (gotcha 13 / *Out of scope*), so there
        // is no Space inconsistency for macOS to reconcile and the
        // write is safe.
        _ = AXUIElementSetAttributeValue(
            window, kAXMainAttribute as CFString, true as CFTypeRef
        )
        _ = AXUIElementSetAttributeValue(
            window, kAXFocusedAttribute as CFString, true as CFTypeRef
        )
        return true
    }
}
