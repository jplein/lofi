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
struct DiscoveredWindow {
    let id: CGWindowID
    let title: String
    let ownerName: String
    let ownerPid: pid_t
    let ownerBundleId: String?
    let ownerBundlePath: String?
    let workspace: Int32
}

enum WindowDiscovery {
    /// Returns the user-relevant on-screen windows owned by other
    /// applications. Filters:
    ///   - `kCGWindowLayer == 0` (regular app windows, not menus / panels /
    ///     system UI — those live at non-zero layers).
    ///   - `kCGWindowOwnerPID != getpid()` (don't list LoFi.app's own panel).
    ///   - non-empty `kCGWindowName` (a titleless window is uninteresting
    ///     in a launcher, and also a strong signal that Screen Recording
    ///     is denied).
    /// Caller must hold both Screen Recording and Accessibility
    /// permissions; this function does not gate on them.
    static func discover() -> [DiscoveredWindow] {
        let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
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
            // Single `NSRunningApplication` lookup, two derived fields:
            // `bundleIdentifier` (stable id, used for `EntryRef`) and
            // `bundleURL.path` (used by the UI to resolve the icon via
            // `NSWorkspace.shared.icon(forFile:)`). Both can be nil for
            // system processes.
            let runningApp = NSRunningApplication(processIdentifier: pidValue)
            let bundleId = runningApp?.bundleIdentifier
            let bundlePath = runningApp?.bundleURL?.path

            results.append(
                DiscoveredWindow(
                    id: windowId,
                    title: title,
                    ownerName: ownerName,
                    ownerPid: pidValue,
                    ownerBundleId: bundleId,
                    ownerBundlePath: bundlePath,
                    workspace: 0
                )
            )
        }

        return results
    }
}
