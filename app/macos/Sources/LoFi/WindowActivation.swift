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
// Title matching
// --------------
// We bridge from a CGWindow title (`kCGWindowName`) to an AXUIElement
// by walking the AX windows of the owning app and comparing their
// `kAXTitleAttribute` against ours. Plain string equality is *not*
// enough — see `titleMatches(cgWindowTitle:axTitle:)` below. Two
// disagreements between the layers:
//
//   - CGWindow truncates long titles with a Unicode ellipsis (`…`);
//     AX keeps the full text.
//   - AX often suffixes the app name (Chrome: `"Hacker News" → AX
//     "Hacker News - Google Chrome"`). AppKit apps mostly don't.
//
// We take the pre-ellipsis prefix of the CGWindow title and accept
// any AX title that starts with it. This handles both shapes with
// one rule.
//
// Brittleness: when an app has two windows with similar long
// prefixes (the truncated portion happens to be identical), the
// first-match-wins behavior picks whichever AX iteration order
// surfaces first. macOS does not expose a stable mapping from
// CGWindowID to AXUIElement through the public API; the private
// `_AXUIElementGetWindow` can do it, but pulling that in is out of
// scope for this slice.

import AppKit
import ApplicationServices

/// Match a CGWindow-side title (what we got from
/// `CGWindowListCopyWindowInfo[kCGWindowName]`) against an AX-side
/// title (what `kAXTitleAttribute` returned on an `AXUIElement`).
///
/// Plain string equality is wrong because the two layers disagree on
/// long-title rendering in ways that show up in practice:
///
///   - **CGWindow truncates with a Unicode ellipsis (`…`, U+2026).**
///     AX keeps the full text. So CGWindow `"Long title that runs…"`
///     never `==` AX `"Long title that runs on and on - Foo"`.
///   - **AX often appends the app name as a suffix.** Chrome is the
///     egregious case: CGWindow `"Hacker News"` vs AX
///     `"Hacker News - Google Chrome"`. Some Electron apps do the
///     same. AppKit apps usually don't.
///
/// We handle both by taking the pre-ellipsis prefix of the CGWindow
/// title and accepting any AX title that *starts with* that prefix.
/// `hasPrefix("Hacker News")` swallows the `" - Google Chrome"` suffix
/// for free; pre-ellipsis prefix matching handles the truncation case.
///
/// Tradeoff: if two windows of the same app share a long-enough
/// pre-ellipsis prefix, this picks whichever AX iteration order
/// surfaces first. That's no worse than the existing first-match-wins
/// behavior for exact-titled siblings (see gotcha 11 in the README);
/// the principled fix is `_AXUIElementGetWindow` to disambiguate by
/// CGWindowID. Out of scope for this slice.
private func titleMatches(cgWindowTitle: String, axTitle: String) -> Bool {
    if cgWindowTitle == axTitle { return true }
    let prefix: String = {
        if let r = cgWindowTitle.range(of: "…") {
            return String(cgWindowTitle[..<r.lowerBound])
                .trimmingCharacters(in: .whitespaces)
        }
        return cgWindowTitle
    }()
    guard !prefix.isEmpty else { return false }
    return axTitle.hasPrefix(prefix)
}

enum WindowActivation {
    /// Raise the window with the given title belonging to the process
    /// at `pid`. Returns `true` on success, `false` if the AX call
    /// chain fails or no AX window with the given title exists.
    ///
    /// Calls `NSRunningApplication.activate()` after the raise so the
    /// owning app becomes key (without that, raising puts the window
    /// on top in z-order but keyboard focus stays with the previous
    /// app).
    static func raise(pid: pid_t, title: String) -> Bool {
        let app = AXUIElementCreateApplication(pid)

        // Firefox (and other Gecko/Chromium-derived apps) ship with
        // accessibility disabled and report zero AX windows until the
        // runtime is explicitly woken up. `AXEnhancedUserInterface` is
        // the documented kick used by VoiceOver and Mac scripting
        // tools; the attribute is undeclared (no `kAX` constant) but
        // accepts the same boolean-set pattern as other AX attributes.
        // Best-effort — apps that already expose AX just no-op the
        // write.
        _ = AXUIElementSetAttributeValue(
            app,
            "AXEnhancedUserInterface" as CFString,
            true as CFTypeRef
        )

        var windowsValue: CFTypeRef?
        let copyErr = AXUIElementCopyAttributeValue(
            app,
            kAXWindowsAttribute as CFString,
            &windowsValue
        )
        guard copyErr == .success else { return false }
        let windowsArray = (windowsValue as? [AXUIElement]) ?? []

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

        for window in windowsArray {
            var titleValue: CFTypeRef?
            let titleErr = AXUIElementCopyAttributeValue(
                window,
                kAXTitleAttribute as CFString,
                &titleValue
            )
            guard titleErr == .success,
                  let axTitle = titleValue as? String,
                  titleMatches(cgWindowTitle: title, axTitle: axTitle)
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
