// Command-target gathering — the macOS analog of GNOME's
// `app/gnome/src/commands.rs::gather_commands`.
//
// The nine window-action commands (Center, halves, two-thirds, Standard
// size, Minimize, Toggle maximize, Toggle fullscreen) all act on a single
// *target window*: the most-recently-focused user window that isn't LoFi
// itself. This file picks that target and captures everything the
// activation path will need so dispatch is one AX round-trip with no
// further reads:
//   - `windowId` / `pid` / `title` — to find and drive the AX window.
//   - `currentFrame` — the window's frame at gather time (only `center`
//     reads it, to recenter without resizing).
//   - `workArea` — the window's screen's visible frame, used as the
//     bounding box by every geometry command via `compute_geometry`.
//
// WHY "first non-LoFi window" (GNOME parity)
// ------------------------------------------
// `CGWindowListCopyWindowInfo` returns windows front-to-back, so the
// first non-LoFi entry is the frontmost other window — the analog of
// GNOME's first non-LoFi `ListWindowsMRU` entry. `WindowDiscovery`
// already excludes LoFi (it skips `pid == getpid()`), and we
// belt-and-suspenders against the bundle id as well.
//
// WHY top-left coordinates everywhere
// -----------------------------------
// `compute_geometry` is coordinate-agnostic pure arithmetic on a work
// area, so feeding it a top-left work area yields top-left rects that go
// straight back to AX (which is also top-left). `currentFrame` is already
// top-left (from `kCGWindowBounds`). The ONLY value that needs flipping
// is the work area, because it comes from `NSScreen.visibleFrame`, which
// is in Cocoa bottom-left coordinates — see `workAreaTopLeft`.
//
// Empty result
// ------------
// `gatherTarget()` returns nil when there is no non-LoFi window or no
// screen. The caller (`AppDelegate`) then pushes zero command entries, so
// the command rows simply don't appear — matching GNOME, where an empty
// `gather_commands` drops the rows entirely.

import AppKit

enum WindowCommands {
    /// Bundle identifier of LoFi itself, used as a second guard (beyond
    /// `WindowDiscovery`'s pid filter) so no command ever targets the
    /// launcher's own window.
    private static let lofiBundleId = "dev.jplein.lofi"

    /// Everything the command activation path needs about the target
    /// window, captured once at gather time. Mirrors the fields GNOME's
    /// `Command` carries (target id + work area + current frame) plus the
    /// macOS-only `pid`/`title` for AX dispatch.
    ///
    /// `standardRect` is the StandardSize restore rectangle, single-sourced
    /// from Rust's `compute_geometry` (so the 2/3 math isn't duplicated in
    /// Swift). It can't be known at gather time — it depends on the pushed
    /// `standard_size` command's computed geometry — so it is filled by
    /// `AppDelegate` after the push, before any query. It is used ONLY as
    /// the `toggleMaximize` fallback when no previous frame was saved for
    /// the target window (see `WindowControl.toggleMaximize`).
    struct CommandTarget {
        let windowId: UInt64
        let pid: pid_t
        let title: String
        let workArea: CGRect
        let currentFrame: CGRect
        var standardRect: CGRect
    }

    /// Pick the command target (frontmost non-LoFi window) and capture its
    /// frame + work area, all in top-left global coordinates. Returns nil
    /// when there's no usable target (no non-LoFi window, or no screen to
    /// derive a work area from), which drops the command rows entirely.
    ///
    /// `standardRect` is initialized to `.zero` here as a placeholder;
    /// `AppDelegate` overwrites it post-push with the computed StandardSize
    /// rect (see `CommandTarget.standardRect`).
    static func gatherTarget() -> CommandTarget? {
        // `onScreenOnly: true` gives the current Space's windows in reliable
        // front-to-back z-order, so the first non-LoFi entry is the genuinely
        // frontmost window the user was just using. (The window LIST uses the
        // all-Spaces variant; the command TARGET must not pick a window on
        // another Space — see `WindowDiscovery.discover`.) `WindowDiscovery`
        // already excludes LoFi by pid; skip the LoFi bundle id too as a
        // belt-and-suspenders guard.
        let target = WindowDiscovery.discover(onScreenOnly: true).first { window in
            window.ownerBundleId != lofiBundleId
        }
        guard let target else { return nil }
        guard let workArea = workAreaTopLeft(forWindowBounds: target.bounds) else {
            return nil
        }
        return CommandTarget(
            windowId: UInt64(target.id),
            pid: target.ownerPid,
            title: target.title,
            workArea: workArea,
            currentFrame: target.bounds,
            standardRect: .zero
        )
    }

    /// Compute the target window's screen's work area in **top-left global
    /// coordinates** from the window's top-left-global `bounds`.
    ///
    /// The work area is `NSScreen.visibleFrame` (the screen rect minus the
    /// menu bar and Dock), which AppKit reports in **Cocoa bottom-left**
    /// coordinates (origin bottom-left of the primary display, y up). AX
    /// and CGWindow are **top-left** (origin top-left, y down), so the
    /// visible frame must be flipped before it can be used as the bounding
    /// box for `compute_geometry` (whose output goes straight to AX).
    ///
    /// Screen selection is by **center-containment**: we pick the
    /// `NSScreen` whose (flipped) frame contains the window's center, so a
    /// command run from a launcher on monitor A correctly resizes a target
    /// window on monitor B. Off-screen windows fall back to `NSScreen.main`
    /// then the primary screen.
    ///
    /// Returns nil when there are no screens at all (an empty
    /// `NSScreen.screens`), which propagates up to "no command rows".
    private static func workAreaTopLeft(forWindowBounds bounds: CGRect) -> CGRect? {
        guard let primary = NSScreen.screens.first else { return nil }
        // The primary (menu-bar) screen's height defines the global origin
        // for BOTH coordinate spaces. We must use *this* height for every
        // flip, even on a secondary monitor — using the target screen's
        // own height would mis-place the rect on multi-monitor setups (the
        // y-axis is anchored to the primary display, not the local one).
        let primaryHeight = primary.frame.height

        // Find the screen whose flipped frame contains the window center.
        // We flip each screen's *frame* (not visibleFrame) to top-left to
        // compare against the top-left window center.
        let center = CGPoint(x: bounds.midX, y: bounds.midY)
        let screen =
            NSScreen.screens.first { candidate in
                let f = candidate.frame
                let topLeftY = primaryHeight - f.origin.y - f.height
                let topLeftFrame = CGRect(
                    x: f.origin.x,
                    y: topLeftY,
                    width: f.width,
                    height: f.height
                )
                return topLeftFrame.contains(center)
            } ?? NSScreen.main ?? primary

        // Flip the chosen screen's visible frame (Cocoa bottom-left) to
        // top-left global. `vf.origin.y` is the distance from the primary
        // display's bottom to the rect's bottom edge; the top-left y is the
        // distance from the primary display's top to the rect's TOP edge =
        // primaryHeight - (vf.origin.y + vf.height). x is unchanged.
        let vf = screen.visibleFrame
        let topLeftY = primaryHeight - vf.origin.y - vf.height
        return CGRect(
            x: vf.origin.x,
            y: topLeftY,
            width: vf.width,
            height: vf.height
        )
    }
}
