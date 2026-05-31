// AX-based lookup of the user's frontmost non-LoFi window.
//
// Why AX over CGWindowListCopyWindowInfo
// --------------------------------------
// CGWindowList is the natural API for an across-process snapshot of every
// on-screen window, but it gates `kCGWindowName` (window titles) behind
// the **Screen Recording** TCC service — even though LoFi never records
// anything. That extra grant is jarring for a launcher (Raycast / Alfred
// don't ask for it), so we drive the lookup entirely through the
// **Accessibility** grant we already need for raise/move/resize, and drop
// the Screen Recording dependency.
//
// The cross-process z-order CGWindowList gave us for free isn't actually
// needed: the only consumer is `WindowCommands.gatherTarget`, which wants
// "the window the user was just using" — i.e. the focused window of the
// foreground process. `NSWorkspace.frontmostApplication` (the OS-level
// foreground app) plus AX `kAXFocusedWindowAttribute` on that app's
// AXApplication element is the direct expression of that intent, and
// runs against permissions we already hold. The summon path always
// gathers BEFORE `NSApp.activate(...)`, so at the moment this is called
// LoFi is still in the background and `frontmostApplication` is the
// user's previous app, not us.
//
// Caller contract: this function does *not* check the Accessibility
// grant itself — that's the AppDelegate's job. When Accessibility is
// denied, the AX reads here return `kAXErrorAPIDisabled`, no focused
// window resolves, and we return nil — which is fine because AppDelegate
// doesn't call us in that state anyway.

import AppKit
import ApplicationServices

/// One on-screen window's metadata, sufficient to populate a
/// `WindowCommands.CommandTarget` and later drive AX from
/// `WindowControl`.
///
/// Two related but distinct fields about the owning app, mirroring the
/// `AppDiscovery.swift` split:
///   - `ownerBundlePath` — absolute filesystem path to the owning app's
///     `.app` bundle. Optional because `NSRunningApplication.bundleURL`
///     can be nil for system processes.
///   - `ownerBundleId` — the stable identifier (`CFBundleIdentifier`),
///     used as a belt-and-suspenders LoFi filter alongside the pid check.
///     Not a path; never pass this to `NSWorkspace.shared.icon(forFile:)`.
///
/// `workspace` is always 0 on macOS: there's no Mutter-style workspace
/// concept. The field exists for cross-platform parity with the GNOME
/// pipeline.
///
/// `bounds` is the window's on-screen rectangle from
/// `kAXPositionAttribute` + `kAXSizeAttribute`, in **top-left global
/// display coordinates** (origin top-left of the primary display, y
/// growing downward) — the same coordinate space AX uses for sets, so
/// the rect can be handed straight to `compute_geometry` and back to
/// AX without a flip. The work area, by contrast, comes from
/// `NSScreen.visibleFrame` (Cocoa bottom-left) and *does* need flipping
/// — see `WindowCommands`.
struct DiscoveredWindow {
    let id: CGWindowID
    let title: String
    let ownerName: String
    let ownerPid: pid_t
    let ownerBundleId: String?
    let ownerBundlePath: String?
    let workspace: Int32
    let bounds: CGRect
}

enum WindowDiscovery {
    /// LoFi's bundle identifier, used as the foreground-app filter so the
    /// launcher never targets its own window when LoFi happens to be the
    /// frontmost process (e.g. a second summon without a prior dismiss).
    private static let lofiBundleId = "dev.jplein.lofi"

    /// Return the user's **frontmost non-LoFi window** via AX. Used by
    /// `WindowCommands.gatherTarget` as the target for every window-action
    /// command (center, halves, minimize, toggle*, move-to-display).
    ///
    /// Returns nil when:
    ///   - There is no foreground application (system mid-transition).
    ///   - The foreground application IS LoFi.
    ///   - The foreground app has no AX focused window. The AX runtime may
    ///     be asleep on first contact (Firefox, some Chromium derivatives —
    ///     gotcha 12); `AXWindowFinder.windowsForApp` does the wakeup
    ///     before we read the focused window, so a nil here means AX is
    ///     genuinely closed to us (sandboxed app, AX-hostile toolkit).
    ///   - The focused window has no usable `CGWindowID` via the private
    ///     `_AXUIElementGetWindow` bridge (gotcha 11). Without an id we
    ///     can't key into `SavedFrameStore` for toggle-maximize, and the
    ///     id-first AX match in `WindowControl` would fail too — refuse
    ///     the target rather than ship a partially-functional row set.
    static func frontmostNonLoFi() -> DiscoveredWindow? {
        guard let app = NSWorkspace.shared.frontmostApplication else {
            return nil
        }
        let pid = app.processIdentifier
        if pid == getpid() || app.bundleIdentifier == lofiBundleId {
            return nil
        }

        // Side-effect-only call: wake the AX runtime if needed (Firefox /
        // Chromium derivatives ship with it asleep — gotcha 12) so the
        // `kAXFocusedWindowAttribute` read below sees a populated app.
        // Returned list is intentionally discarded.
        _ = AXWindowFinder.windowsForApp(pid: pid)

        let appElement = AXUIElementCreateApplication(pid)
        var focused: CFTypeRef?
        let err = AXUIElementCopyAttributeValue(
            appElement,
            kAXFocusedWindowAttribute as CFString,
            &focused
        )
        guard err == .success, let focused else { return nil }
        // CFTypeRef → AXUIElement is a CoreFoundation downcast (no
        // runtime check); the `as!` mirrors `WindowControl.readFrame`.
        let window = focused as! AXUIElement

        guard let windowId = AXWindowFinder.cgWindowId(of: window) else {
            return nil
        }

        // Title via AX. Unlike the old CGWindowList path, titlelessness
        // here is not a permission-denied signal — accept "" if the read
        // misses, since the id is what `WindowControl` actually matches
        // on (the title is only a fallback for apps with no id bridge).
        var titleValue: CFTypeRef?
        let titleErr = AXUIElementCopyAttributeValue(
            window,
            kAXTitleAttribute as CFString,
            &titleValue
        )
        let title: String =
            (titleErr == .success ? (titleValue as? String) : nil) ?? ""

        // Geometry via AX. Same `as!` pattern as `WindowControl.readFrame`:
        // a malformed AXValue degrades to nil via `AXValueGetValue`,
        // never traps.
        var posValue: CFTypeRef?
        let posErr = AXUIElementCopyAttributeValue(
            window,
            kAXPositionAttribute as CFString,
            &posValue
        )
        var sizeValue: CFTypeRef?
        let sizeErr = AXUIElementCopyAttributeValue(
            window,
            kAXSizeAttribute as CFString,
            &sizeValue
        )
        guard posErr == .success, sizeErr == .success,
            let posValue, let sizeValue
        else {
            return nil
        }
        var origin = CGPoint.zero
        var size = CGSize.zero
        guard AXValueGetValue(posValue as! AXValue, .cgPoint, &origin),
            AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
        else {
            return nil
        }
        let bounds = CGRect(origin: origin, size: size)

        // Owner metadata — bundle id (for the GNOME-parity `EntryRef`
        // shape) and bundle path (for icon resolution). Both can be nil
        // for system processes; we still return the window.
        let bundleId = app.bundleIdentifier
        let bundlePath: String? =
            app.bundleURL?.path
            ?? bundleId.flatMap {
                NSWorkspace.shared.urlForApplication(
                    withBundleIdentifier: $0
                )?.path
            }

        return DiscoveredWindow(
            id: windowId,
            title: title,
            ownerName: app.localizedName ?? "",
            ownerPid: pid,
            ownerBundleId: bundleId,
            ownerBundlePath: bundlePath,
            workspace: 0,
            bounds: bounds
        )
    }
}
