// On-screen window enumeration via Core Graphics.
//
// Why CGWindowList over ScreenCaptureKit: ScreenCaptureKit is async and
// streaming; we want a one-shot snapshot at launch. `CGWindowListCopyWindowInfo`
// is the simpler, synchronous API and returns enough metadata
// (kCGWindowName, kCGWindowOwnerName, kCGWindowOwnerPID, kCGWindowNumber)
// to populate the launcher row plus the Swift-side `windowAux` activation
// map.
//
// Caller contract: this function does *not* check Screen Recording or
// Accessibility itself — that's the AppDelegate's job. Without Screen
// Recording, `kCGWindowName` is nil or empty on every entry, so the
// non-empty-title filter below effectively drops every window. We keep
// that as a robustness fallback rather than the primary gate.

import AppKit
import ApplicationServices

/// One on-screen window's metadata, sufficient to render a launcher row
/// and to later activate the window via `WindowActivation.raise(pid:title:)`.
///
/// `ownerBundleId` is optional because `NSRunningApplication(processIdentifier:)`
/// can return nil for system processes (e.g. WindowServer-owned shells).
/// The Window entry still appears in the list — `WindowActivation` works
/// off pid+title, not bundle id.
///
/// Two related but distinct fields about the owning app, mirroring the
/// `AppDiscovery.swift` split:
///   - `ownerBundlePath` — absolute filesystem path to the owning app's
///     `.app` bundle. This is the *icon-resolution input*: the Swift UI
///     hands it to `NSWorkspace.shared.icon(forFile:)` at draw time.
///     Optional because `NSRunningApplication.bundleURL` can be nil for
///     system processes.
///   - `ownerBundleId` — the stable identifier (`CFBundleIdentifier`)
///     used for `EntryRef` / cross-platform parity. Not a path; never
///     pass this to `NSWorkspace.shared.icon(forFile:)`.
///
/// `workspace` is always 0 on macOS: there's no Mutter-style workspace
/// concept. The field exists for cross-platform parity with the GNOME
/// pipeline, which uses `Shell.WindowTracker.get_workspace().index`.
///
/// `bounds` is the window's on-screen rectangle from `kCGWindowBounds`,
/// already in **top-left global display coordinates** (origin top-left of
/// the primary display, y growing downward). This is the same coordinate
/// space the Accessibility `kAXPositionAttribute`/`kAXSizeAttribute` use,
/// so the rect can be handed straight to `compute_geometry` (as the
/// command target's `current_frame`) and back to AX without a flip. The
/// work area, by contrast, comes from `NSScreen.visibleFrame` (Cocoa
/// bottom-left) and *does* need flipping — see `WindowCommands`.
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
    /// Returns the user-relevant on-screen windows owned by other
    /// applications on the **active macOS Space**, in reliable
    /// front-to-back z-order. Filters:
    ///   - `kCGWindowLayer == 0` (regular app windows, not menus / panels /
    ///     system UI — those live at non-zero layers).
    ///   - `kCGWindowOwnerPID != getpid()` (don't list LoFi.app's own panel).
    ///   - non-empty `kCGWindowName` (a titleless window is uninteresting,
    ///     and also a strong signal that Screen Recording is denied).
    /// Caller must hold both Screen Recording and Accessibility
    /// permissions; this function does not gate on them.
    ///
    /// **Used only by the window-action command target** today.
    /// `WindowCommands.gatherTarget` calls this to pick the frontmost
    /// non-LoFi window as the target for center/halves/standard-size/
    /// minimize/toggle-* / next-display / previous-display commands.
    /// `SavedFrameStore.prune` also reads the live-id list to
    /// garbage-collect dropped frame records. The window *switcher*
    /// (per-window launcher rows) used to call this too but is
    /// disabled on macOS — see README gotchas 13-14 for the macOS
    /// limitations that ruled it out.
    ///
    /// **No active-display filter.** An earlier iteration filtered
    /// by the cursor's display, which was needed by the (now-disabled)
    /// window switcher to avoid the cross-display focus problem. For
    /// command targeting it was actively wrong: picking the frontmost
    /// non-LoFi window *on the cursor's display* targets a different
    /// window than the user expects whenever a previous command has
    /// moved the foreground window to a different display (e.g.
    /// "Previous display" moves Ghostty to display 1, cursor stays on
    /// display 0, user summons LoFi expecting "Next display" to move
    /// Ghostty back — but `gatherTarget` returns whatever happens to
    /// be on display 0 instead). The right scope for command targeting
    /// is the global frontmost non-LoFi window (GNOME parity), and
    /// the command's *work area* is derived from the *target window's*
    /// display via `WindowCommands.workAreaTopLeft`, not from the
    /// cursor's display.
    ///
    /// `.optionOnScreenOnly` gives the front-to-back z-order
    /// `gatherTarget` depends on AND restricts the result to the
    /// active Space (the only Space we can reliably activate anything
    /// on — see gotcha 13).
    static func discover() -> [DiscoveredWindow] {
        let options: CGWindowListOption = [.excludeDesktopElements, .optionOnScreenOnly]
        guard let rawList = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
            return []
        }
        guard let dicts = rawList as? [[String: Any]] else {
            return []
        }

        let ourPid = getpid()
        var results: [DiscoveredWindow] = []
        results.reserveCapacity(dicts.count)

        for dict in dicts {
            // Skip non-window layers (menu bar, dock items, system UI).
            // `kCGWindowLayer` is documented as `CFNumber` (int32).
            guard let layer = dict[kCGWindowLayer as String] as? Int, layer == 0 else {
                continue
            }
            // Skip our own panel.
            guard let pidValue = dict[kCGWindowOwnerPID as String] as? pid_t else {
                continue
            }
            if pidValue == ourPid {
                continue
            }
            // Empty / missing title -> drop. Without Screen Recording every
            // entry hits this branch.
            guard let title = dict[kCGWindowName as String] as? String,
                !title.isEmpty
            else {
                continue
            }
            let ownerName =
                (dict[kCGWindowOwnerName as String] as? String) ?? ""
            // `kCGWindowNumber` is `CFNumber` (int32). Cast through `UInt32`
            // because `CGWindowID` is a `UInt32` alias and `Int` -> `UInt32`
            // is a runtime check we don't need to take.
            guard let numberRaw = dict[kCGWindowNumber as String] as? UInt32 else {
                continue
            }
            let windowId = CGWindowID(numberRaw)
            // `kCGWindowBounds` is a CFDictionary (`{X, Y, Width, Height}`),
            // not a raw CGRect — bridge it via
            // `CGRect(dictionaryRepresentation:)`. The rect is already in
            // top-left global coordinates (see `DiscoveredWindow.bounds`).
            // Skip the window if bounds can't be read, mirroring the
            // skip-on-missing-field pattern above: a window surfaced to the
            // launcher must always carry a usable `bounds` so a command
            // targeting it has a real `current_frame`.
            guard let boundsDict = dict[kCGWindowBounds as String] as? NSDictionary,
                let bounds = CGRect(dictionaryRepresentation: boundsDict as CFDictionary)
            else {
                continue
            }
            // Single `NSRunningApplication` lookup, two derived fields:
            // `bundleIdentifier` (stable id, used for `EntryRef`) and
            // `bundleURL.path` (used by the UI to resolve the icon via
            // `NSWorkspace.shared.icon(forFile:)`). Both can be nil for
            // system processes.
            let runningApp = NSRunningApplication(processIdentifier: pidValue)
            let bundleId = runningApp?.bundleIdentifier
            // Path first, bundle-id fallback. `bundleURL` is normally
            // populated for ordinary apps; the fallback covers edge cases
            // where the running-app lookup gives us a bundle id but no
            // URL (some system processes), in which case we ask
            // LaunchServices to translate the bundle id to a path.
            let bundlePath: String? =
                runningApp?.bundleURL?.path
                ?? bundleId.flatMap {
                    NSWorkspace.shared.urlForApplication(
                        withBundleIdentifier: $0
                    )?.path
                }

            results.append(
                DiscoveredWindow(
                    id: windowId,
                    title: title,
                    ownerName: ownerName,
                    ownerPid: pidValue,
                    ownerBundleId: bundleId,
                    ownerBundlePath: bundlePath,
                    workspace: 0,
                    bounds: bounds
                )
            )
        }

        return results
    }
}
