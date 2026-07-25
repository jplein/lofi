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
import os

// `os.Logger`, not `NSLog`: Tahoe redacts every NSLog message body to
// `<private>` in the unified log, and an `LSUIElement` daemon launched via
// `open` / launchd has no stdout to fall back to (README gotcha 25). The
// failure call sites below log at `.error` on purpose — `.debug`/`.info`
// aren't persisted at the default level, and the whole reason this logging
// exists is post-hoc diagnosis of the intermittent "window-action command
// rows missing under load" bug on a machine we can't attach a debugger to.
// Read it back with:
//   log show --predicate 'subsystem == "dev.jplein.lofi"' --info --last 15m
private let log = Logger(subsystem: "dev.jplein.lofi", category: "window-discovery")

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

    // Retry policy for the AX discovery chain (README gotcha 33). Every
    // cross-process AX read below has to be serviced by the *target* app's
    // main run loop. On a slow / loaded machine — background security
    // scanners stealing CPU, a busy foreground app not pumping its run loop
    // promptly — any single read can return `.cannotComplete`, the AX "the
    // other process didn't answer in time" error. A one-shot chain then
    // collapses to nil, and because `AppDelegate.pushCommands` bails on a nil
    // target, EVERY window-action command row (Toggle maximize included)
    // silently vanishes for that summon. Retrying the whole chain turns a
    // transient miss into a brief extra hop instead of a dropped command set.
    // Only `.cannotComplete`-class failures retry; a settled "no focused
    // window" answer returns immediately (a retry can't change it).
    private static let maxAttempts = 3
    // Backoff between attempts. Short because this runs synchronously on the
    // main thread during a summon (the panel isn't shown until it returns);
    // with the per-message timeout below, the rare all-fail path is bounded
    // at roughly `maxAttempts × (messagingTimeout + backoff)` — sub-second.
    private static let retryBackoffMicros: useconds_t = 60_000

    /// Per-message AX timeout, applied process-wide by
    /// `installMessagingTimeout()` and again on the discovery hot-path
    /// elements. Without it a wedged foreground app could block a summon for
    /// the AX *default* (~6s). 250ms is far above normal AX latency
    /// (sub-millisecond) yet short enough to fail fast and hand off to the
    /// retry loop. Referenced from `AXWindowFinder.windowsForApp` too.
    static let messagingTimeoutSeconds: Float = 0.25

    /// Install a process-global AX messaging timeout so no AX call can hang
    /// the launcher for the AX default. Passing the system-wide element sets
    /// the default for every element messaged from this process (Apple:
    /// passing the system-wide object "sets the timeout globally for this
    /// process"). Call once at launch, before the first summon.
    static func installMessagingTimeout() {
        let systemWide = AXUIElementCreateSystemWide()
        _ = AXUIElementSetMessagingTimeout(systemWide, messagingTimeoutSeconds)
    }

    /// Return the user's **frontmost non-LoFi window** via AX. Used by
    /// `WindowCommands.gatherTarget` as the target for every window-action
    /// command (center, halves, minimize, toggle*, move-to-display).
    ///
    /// Retries the AX chain up to `maxAttempts` times on transient
    /// `.cannotComplete`-class failures (README gotcha 33) so a single
    /// AX hiccup under load doesn't drop the whole command set. Returns nil
    /// only when the target is *settled* absent — no foreground app, the
    /// foreground app IS LoFi, or the app genuinely has no focused window /
    /// no usable `CGWindowID` (gotchas 11, 12) — or when the transient
    /// retries are exhausted (logged so it's diagnosable rather than silent).
    static func frontmostNonLoFi() -> DiscoveredWindow? {
        var lastReason = "unknown"
        for attempt in 1...maxAttempts {
            switch attemptFrontmostNonLoFi() {
            case .found(let window):
                // A recovery means the first pass would have dropped the
                // command rows on the old one-shot code path — worth a line.
                if attempt > 1 {
                    log.error(
                        "AX discovery recovered on attempt \(attempt, privacy: .public)"
                    )
                }
                return window
            case .noTarget(let reason):
                // Settled and usually benign (e.g. the frontmost app IS LoFi
                // on a re-summon). Debug keeps the default log quiet.
                log.debug("no command target: \(reason, privacy: .public)")
                return nil
            case .retryable(let reason):
                lastReason = reason
                log.error(
                    "AX discovery attempt \(attempt, privacy: .public)/\(maxAttempts, privacy: .public) failed: \(reason, privacy: .public)"
                )
                if attempt < maxAttempts { usleep(retryBackoffMicros) }
            }
        }
        log.error(
            "AX discovery gave up after \(maxAttempts, privacy: .public) attempts — command rows dropped this summon (last: \(lastReason, privacy: .public))"
        )
        return nil
    }

    /// Outcome of a single discovery pass, so `frontmostNonLoFi` can tell a
    /// transient AX hiccup (retry) apart from a settled "nothing to target"
    /// answer (return nil now). The associated `String` is the reason, used
    /// only for logging.
    private enum Outcome {
        case found(DiscoveredWindow)
        case noTarget(String)  // settled: no window to act on — do not retry
        case retryable(String)  // transient AX failure — worth another pass
    }

    /// One pass of the AX discovery chain. Classifies each failure as
    /// `.retryable` (`.cannotComplete` — the loaded-machine timeout) or
    /// `.noTarget` (a settled absence), so the caller retries only what a
    /// retry could fix.
    private static func attemptFrontmostNonLoFi() -> Outcome {
        guard let app = NSWorkspace.shared.frontmostApplication else {
            // The foreground app can be momentarily nil mid app-switch —
            // treat as transient and give it another pass.
            return .retryable("no frontmost application")
        }
        let pid = app.processIdentifier
        if pid == getpid() || app.bundleIdentifier == lofiBundleId {
            return .noTarget("frontmost app is LoFi")
        }

        // Side-effect-only call: wake the AX runtime if needed (Firefox /
        // Chromium derivatives ship with it asleep — gotcha 12) so the
        // `kAXFocusedWindowAttribute` read below sees a populated app.
        // Returned list is intentionally discarded.
        _ = AXWindowFinder.windowsForApp(pid: pid)

        let appElement = AXUIElementCreateApplication(pid)
        // Belt-and-suspenders alongside the process-global timeout: bound
        // this element's messages explicitly so the focused-window read
        // fails fast rather than blocking the summon.
        _ = AXUIElementSetMessagingTimeout(appElement, messagingTimeoutSeconds)

        var focused: CFTypeRef?
        let err = AXUIElementCopyAttributeValue(
            appElement,
            kAXFocusedWindowAttribute as CFString,
            &focused
        )
        guard err == .success, let focused else {
            if err == .cannotComplete {
                return .retryable("focused-window read: cannotComplete")
            }
            // `.noValue` / `.attributeUnsupported` etc. mean the app genuinely
            // has no focused window right now — settled. (`.apiDisabled`
            // shouldn't reach here since `AppDelegate` gates on Accessibility,
            // but if it did it's also settled — a retry can't help.)
            return .noTarget("no focused window (AXError \(err.rawValue))")
        }
        // CFTypeRef → AXUIElement is a CoreFoundation downcast (no
        // runtime check); the `as!` mirrors `WindowControl.readFrame`.
        let window = focused as! AXUIElement
        _ = AXUIElementSetMessagingTimeout(window, messagingTimeoutSeconds)

        guard let windowId = AXWindowFinder.cgWindowId(of: window) else {
            // The id bridge itself doesn't message the app, but the window
            // handle can be transiently stale under churn; a fresh pass
            // often resolves it, so retry rather than drop the target.
            return .retryable("no CGWindowID for focused window")
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
            if posErr == .cannotComplete || sizeErr == .cannotComplete {
                return .retryable(
                    "geometry read: cannotComplete (pos \(posErr.rawValue), size \(sizeErr.rawValue))"
                )
            }
            return .noTarget(
                "no geometry (pos \(posErr.rawValue), size \(sizeErr.rawValue))"
            )
        }
        var origin = CGPoint.zero
        var size = CGSize.zero
        guard AXValueGetValue(posValue as! AXValue, .cgPoint, &origin),
            AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
        else {
            return .noTarget("malformed AXValue geometry")
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

        return .found(
            DiscoveredWindow(
                id: windowId,
                title: title,
                ownerName: app.localizedName ?? "",
                ownerPid: pid,
                ownerBundleId: bundleId,
                ownerBundlePath: bundlePath,
                workspace: 0,
                bounds: bounds
            )
        )
    }
}
