// Window manipulation via the Accessibility (AX) API.
//
// This is the macOS analog of GNOME's Mutter `MoveResizeWindow` /
// `Minimize` / `Maximize` / `MakeFullscreen` calls: it moves, resizes,
// minimizes, and toggles the fullscreen/maximize state of a *specific*
// window owned by another process. The geometry kinds
// (center/halves/two-thirds/standard) all funnel through `move`; the
// three state kinds (`minimize`, `toggleFullscreen`, `toggleMaximize`)
// have dedicated entry points because they have no precomputed rectangle.
//
// Coordinate system
// -----------------
// AX `kAXPositionAttribute` and `kAXSizeAttribute` are in **top-left
// global display coordinates** (origin top-left of the primary display,
// y down) — the same space as `kCGWindowBounds`. `compute_geometry` is
// pure arithmetic on a work area with no notion of an origin, so a
// top-left work area in yields a top-left rect out, usable directly as
// an AX position/size. Every `x`/`y`/`width`/`height` that reaches this
// file is therefore already in the right space; nothing here flips. The
// one flip in the whole feature happens in `WindowCommands` where
// `NSScreen.visibleFrame` (Cocoa bottom-left) is converted before it ever
// reaches Rust.
//
// AX set order and de-zoom
// ------------------------
// AX silently ignores geometry changes on a window that is fullscreen
// (and, in practice, on a "zoomed"/green-button-maximized window), so
// `move` and `toggleMaximize` clear `"AXFullScreen"` *before* setting
// position/size. There is no public "is zoomed" attribute, so we rely on
// the explicit position+size set to de-zoom in practice — the same shape
// GNOME's MoveResizeWindow takes (it unmaximizes/unfullscreens first).
//
// Two things make AX geometry actually stick (see `setFrame` /
// `disableEnhancedUI`):
//   1. Disable `AXEnhancedUserInterface` first. `windowsForApp` turns it on
//      to wake sleeping AX runtimes (Firefox) for enumeration, but with it
//      on the window server applies geometry asynchronously/animated and
//      only the LAST attribute set survives — so position and size fight
//      (resize-or-move-but-not-both, the bug we hit on Ghostty).
//   2. Set **size -> position -> size**. Even with sets synchronous, a move
//      is clamped against the window's size at that instant (and a resize
//      against its position); resizing first, moving, then re-asserting size
//      lands both for shrink and grow. Same dance Rectangle uses.
//
// The AX window is found by `pid` + exact `CGWindowID` (via the private
// `_AXUIElementGetWindow` bridge below), falling back to `titleMatches`
// only when an app's AX windows don't expose an id. See `AXWindowFinder`
// and `WindowActivation`.

import AppKit
import ApplicationServices

/// `_AXUIElementGetWindow` is a private (un-headered) HIServices function
/// that returns the `CGWindowID` backing an `AXUIElement` window. It is the
/// only reliable way to bridge our captured `CGWindowID` to a specific AX
/// window: title matching (the original approach) breaks for apps whose
/// titles change between the gather snapshot and activation — terminals
/// (Ghostty) rewrite their title on every command/cwd change, and browsers
/// retitle on tab switches — which made `WindowControl` commands silently
/// no-op. yabai, Hammerspoon, and AeroSpace all rely on this same symbol.
///
/// Resolved via `dlsym` (rather than `@_silgen_name`) so that if a future
/// macOS ever drops the symbol we degrade gracefully to title matching
/// instead of failing to launch with an unresolved-symbol dyld error.
private typealias AXUIElementGetWindowFunc =
    @convention(c) (AXUIElement, UnsafeMutablePointer<CGWindowID>) -> AXError

private let axUIElementGetWindow: AXUIElementGetWindowFunc? = {
    // RTLD_DEFAULT (-2) searches every already-loaded image; HIServices is
    // linked transitively via `import ApplicationServices`.
    let rtldDefault = UnsafeMutableRawPointer(bitPattern: -2)
    guard let sym = dlsym(rtldDefault, "_AXUIElementGetWindow") else { return nil }
    return unsafeBitCast(sym, to: AXUIElementGetWindowFunc.self)
}()

/// Tolerance, in points, used by `toggleMaximize` to decide whether a
/// window already "fills" its work area. AX-reported geometry can be off
/// from the requested rect by a point or two (window-server rounding,
/// title-bar insets), so an exact `==` comparison would never register a
/// maximized window as filled. 2pt is generous enough to absorb that
/// noise without false-positiving a window that is genuinely a little
/// smaller than the work area. `5` would already trip the `mnd`-style
/// "magic number" threshold elsewhere; this small a constant is kept
/// named purely so the toggle's intent reads clearly.
private let kMaximizeFillTolerance: CGFloat = 2

/// Shared AX-window lookup used by both `WindowActivation` (raise) and
/// `WindowControl` (move/resize/state). Centralizes the two brittle bits
/// every AX caller needs: waking a sleeping AX runtime
/// (`AXEnhancedUserInterface`) before reading the window list, and the
/// title matcher that bridges a CGWindow title to an `AXUIElement`.
enum AXWindowFinder {
    /// Match a CGWindow-side title (`kCGWindowName`) against an AX-side
    /// title (`kAXTitleAttribute`). Moved here from `WindowActivation` so
    /// both files share the one matcher — see the long rationale at the
    /// call sites and gotcha 11 in the README.
    ///
    /// Plain `==` is wrong because the two layers disagree on long-title
    /// rendering: CGWindow truncates with a Unicode ellipsis (`…`,
    /// U+2026) while AX keeps the full text, and AX often appends the app
    /// name as a suffix (Chrome: CGWindow `"Hacker News"` vs AX
    /// `"Hacker News - Google Chrome"`). We take the pre-ellipsis prefix
    /// of the CGWindow title and accept any AX title that *starts with*
    /// it, which handles both shapes with one rule.
    static func titleMatches(cgWindowTitle: String, axTitle: String) -> Bool {
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

    /// Return the AX windows of the application at `pid`. Kicks
    /// `AXEnhancedUserInterface` first so Gecko/Chromium apps (which ship
    /// with the AX runtime asleep and report zero windows until woken)
    /// surface their real window list — the same nudge VoiceOver and Mac
    /// scripting tools use. Apps that already expose AX just no-op the
    /// write. Returns an empty array on any failure (AX disabled, no
    /// windows).
    static func windowsForApp(pid: pid_t) -> [AXUIElement] {
        let app = AXUIElementCreateApplication(pid)
        // Best-effort wakeup; undeclared attribute, hence the bare string.
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
        guard copyErr == .success else { return [] }
        return (windowsValue as? [AXUIElement]) ?? []
    }

    /// The `CGWindowID` backing an AX window, via the private
    /// `_AXUIElementGetWindow` bridge, or `nil` if the symbol is unavailable
    /// or the window doesn't expose an id.
    static func cgWindowId(of element: AXUIElement) -> CGWindowID? {
        guard let fn = axUIElementGetWindow else { return nil }
        var wid = CGWindowID(0)
        guard fn(element, &wid) == .success, wid != 0 else { return nil }
        return wid
    }

    /// Pick the AX window matching our captured `windowId`, preferring an
    /// exact `CGWindowID` match (immune to title drift) and falling back to
    /// `titleMatches` only for apps whose AX windows don't expose a
    /// `CGWindowID` via the private bridge. `nil` when nothing matches.
    static func match(in windows: [AXUIElement], windowId: CGWindowID, title: String)
        -> AXUIElement?
    {
        // Exact id match first — the reliable path.
        for window in windows where cgWindowId(of: window) == windowId {
            return window
        }
        // Title fallback (legacy behavior) for windows with no id bridge.
        for window in windows {
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
            return window
        }
        return nil
    }

    /// Find the AX window owned by `pid` matching `windowId` (id-first, title
    /// fallback — see `match`). Returns `nil` when the app exposes no windows
    /// or none match.
    static func find(pid: pid_t, windowId: CGWindowID, title: String) -> AXUIElement? {
        match(in: windowsForApp(pid: pid), windowId: windowId, title: title)
    }
}

enum WindowControl {
    /// Move and resize the window with `title` owned by `pid` to the
    /// top-left-global rect `(x, y, width, height)`. The rect comes from
    /// `compute_geometry` (via the FFI), so it is already in AX's
    /// coordinate space — no flip here.
    ///
    /// Returns `false` if the AX window can't be found, `true` iff both
    /// the position and size sets report `.success`. Fixed-size /
    /// Electron / AWT windows may refuse a resize, in which case this
    /// returns false; the launcher quits anyway.
    static func move(
        pid: pid_t,
        title: String,
        windowId: UInt64,
        x: Int32,
        y: Int32,
        width: Int32,
        height: Int32
    ) -> Bool {
        guard let window = AXWindowFinder.find(pid: pid, windowId: CGWindowID(windowId), title: title)
        else {
            return false
        }
        clearFullscreen(window)
        disableEnhancedUI(pid: pid)
        return setFrame(
            window,
            origin: CGPoint(x: CGFloat(x), y: CGFloat(y)),
            size: CGSize(width: CGFloat(width), height: CGFloat(height))
        )
    }

    /// Minimize the window with `title` owned by `pid` by setting
    /// `kAXMinimizedAttribute` to true. Returns `false` if the window
    /// can't be found or the set fails.
    static func minimize(pid: pid_t, title: String, windowId: UInt64) -> Bool {
        guard let window = AXWindowFinder.find(pid: pid, windowId: CGWindowID(windowId), title: title)
        else {
            return false
        }
        let err = AXUIElementSetAttributeValue(
            window,
            kAXMinimizedAttribute as CFString,
            true as CFTypeRef
        )
        return err == .success
    }

    /// Toggle the native fullscreen state of the window with `title`
    /// owned by `pid`. Reads the undeclared `"AXFullScreen"` attribute
    /// (bare CFString, same pattern as `AXEnhancedUserInterface`), coerces
    /// the CFBoolean to a Swift `Bool`, and writes the negation. Returns
    /// `false` if the window can't be found or the set fails. Some apps
    /// don't support `"AXFullScreen"`; the set then isn't `.success` and
    /// we return false (best-effort).
    static func toggleFullscreen(pid: pid_t, title: String, windowId: UInt64) -> Bool {
        guard let window = AXWindowFinder.find(pid: pid, windowId: CGWindowID(windowId), title: title)
        else {
            return false
        }
        // A window that has never been fullscreen may not return the
        // attribute at all; treat a failed/absent read as "not
        // fullscreen" so the toggle still drives it into fullscreen.
        let current = readFullscreen(window) ?? false
        let err = AXUIElementSetAttributeValue(
            window,
            "AXFullScreen" as CFString,
            (!current) as CFTypeRef
        )
        return err == .success
    }

    /// TRUE TOGGLE of the maximize state, with previous-size restore.
    ///
    /// GNOME uses Mutter `maximize()`/`unmaximize()`, and Mutter
    /// remembers the pre-maximize frame so un-maximize restores the exact
    /// previous size. macOS AX gives us neither an app-independent
    /// maximize nor a saved frame, and LoFi is a short-lived process that
    /// quits after each activation — so we persist the previous frame
    /// ourselves (`SavedFrameStore`, UserDefaults-backed) to deliver the
    /// same faithful toggle. The divergence from GNOME is *only* in
    /// mechanism (we store the frame; Mutter does); the user-visible
    /// behavior matches.
    ///
    /// The discriminator is geometry, NOT "does a saved frame exist":
    /// the window "fills" the work area iff all four edges are within
    /// `kMaximizeFillTolerance` of `workArea`.
    ///   - NOT filling -> **maximize**: save the current frame (so the
    ///     next toggle can restore it), then resize to `workArea`.
    ///   - filling -> **un-maximize**: restore to the saved previous
    ///     frame (read-and-removed from the store), or `fallbackRect`
    ///     (the StandardSize rect, single-sourced from Rust's
    ///     `compute_geometry`) when no frame was ever saved — e.g. the
    ///     window was already maximized before LoFi first saw it.
    ///
    /// Choosing geometry as the discriminator means a window the user
    /// manually shrinks after a LoFi-maximize is no longer "filling", so
    /// the next press re-maximizes it (overwriting the saved frame) — the
    /// intuitive result.
    ///
    /// All rects are top-left global. Returns `false` if the window can't
    /// be found or the AX reads/sets fail.
    static func toggleMaximize(
        pid: pid_t,
        title: String,
        windowId: UInt64,
        workArea: CGRect,
        fallbackRect: CGRect,
        store: SavedFrameStore
    ) -> Bool {
        guard let window = AXWindowFinder.find(
            pid: pid,
            windowId: CGWindowID(windowId),
            title: title
        ),
            let currentFrame = readFrame(window)
        else {
            return false
        }

        clearFullscreen(window)
        disableEnhancedUI(pid: pid)
        if framesFill(currentFrame, workArea) {
            // Un-maximize: restore the saved previous frame, falling back
            // to the StandardSize rect when none was saved.
            let restore = store.take(windowId: windowId) ?? fallbackRect
            return setFrame(window, origin: restore.origin, size: restore.size)
        } else {
            // Maximize: remember where the window was (overwriting any
            // prior saved frame — we track the size right before the most
            // recent maximize), then fill the work area.
            store.save(windowId: windowId, frame: currentFrame)
            return setFrame(window, origin: workArea.origin, size: workArea.size)
        }
    }

    // MARK: - AX primitives

    /// Move + resize a window to a top-left-global origin + size, using a
    /// **size → position → size** sequence.
    ///
    /// Callers MUST disable `AXEnhancedUserInterface` first (see
    /// `disableEnhancedUI`) — with it on, geometry sets apply asynchronously
    /// and only the last attribute set sticks, so position and size fight.
    /// With it off the sets are synchronous, but a single pass can still be
    /// clamped: the server constrains a move using the window's size at that
    /// instant (and a resize using its position). Setting size first, then
    /// position, then size again handles both directions — the leading size
    /// shrinks a too-wide window so the move isn't clamped against an edge,
    /// and the trailing size re-asserts the dimensions once the window is at
    /// its target origin (covering the grow case). This is the same dance
    /// Rectangle uses. AX values are wrapped via
    /// `AXValueCreate(.cgPoint/.cgSize, ...)`. Returns true iff the final
    /// position set and at least one size set report `.success`.
    private static func setFrame(_ window: AXUIElement, origin: CGPoint, size: CGSize) -> Bool {
        let size1 = setSize(window, size)
        let posOk = setPosition(window, origin)
        let size2 = setSize(window, size)
        return posOk && (size1 || size2)
    }

    /// Turn `AXEnhancedUserInterface` OFF on the owning app so geometry sets
    /// apply synchronously. `AXWindowFinder.windowsForApp` turns it ON to
    /// wake sleeping AX runtimes (Firefox) for enumeration, but leaving it on
    /// makes the window server apply position/size changes asynchronously and
    /// animated — so the two sets in `setFrame` fight and only whichever was
    /// set LAST survives (the cause of resize-or-move-but-not-both). Rectangle
    /// and yabai disable it for exactly this reason. We leave it off rather
    /// than restore it: off is the normal state and LoFi quits immediately
    /// after. Undeclared attribute, hence the bare string.
    private static func disableEnhancedUI(pid: pid_t) {
        let app = AXUIElementCreateApplication(pid)
        _ = AXUIElementSetAttributeValue(
            app,
            "AXEnhancedUserInterface" as CFString,
            false as CFTypeRef
        )
    }

    /// Set just `kAXPositionAttribute`. Returns true iff the set reports
    /// `.success` (which does NOT guarantee the window honored it — the
    /// server may clamp; see `setFrame`).
    private static func setPosition(_ window: AXUIElement, _ origin: CGPoint) -> Bool {
        var p = origin
        guard let posValue = AXValueCreate(.cgPoint, &p) else { return false }
        return AXUIElementSetAttributeValue(
            window,
            kAXPositionAttribute as CFString,
            posValue
        ) == .success
    }

    /// Set just `kAXSizeAttribute`. Returns true iff the set reports
    /// `.success`. Some windows (terminals snapping to a character grid,
    /// fixed-size dialogs) honor it only approximately or not at all.
    private static func setSize(_ window: AXUIElement, _ size: CGSize) -> Bool {
        var s = size
        guard let sizeValue = AXValueCreate(.cgSize, &s) else { return false }
        return AXUIElementSetAttributeValue(
            window,
            kAXSizeAttribute as CFString,
            sizeValue
        ) == .success
    }

    /// Read a window's current top-left-global frame via AX. Returns `nil`
    /// if either the position or size read fails (or unwraps wrong). Used
    /// by `toggleMaximize` to decide fill state and to capture the
    /// pre-maximize frame.
    private static func readFrame(_ window: AXUIElement) -> CGRect? {
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
        // `AXValueGetValue` unwraps the opaque AXValue into the requested
        // CG type; it returns false if the AXValue isn't of that type.
        guard AXValueGetValue(posValue as! AXValue, .cgPoint, &origin),
              AXValueGetValue(sizeValue as! AXValue, .cgSize, &size)
        else {
            return nil
        }
        return CGRect(origin: origin, size: size)
    }

    /// Read the undeclared `"AXFullScreen"` boolean. Returns `nil` when
    /// the attribute is absent or the read fails (apps that have never
    /// been fullscreen often don't report it).
    private static func readFullscreen(_ window: AXUIElement) -> Bool? {
        var value: CFTypeRef?
        let err = AXUIElementCopyAttributeValue(
            window,
            "AXFullScreen" as CFString,
            &value
        )
        guard err == .success else { return nil }
        return value as? Bool
    }

    /// Clear native fullscreen if it's currently on, so a subsequent
    /// position/size set actually takes effect (AX ignores geometry on a
    /// fullscreen window). Best-effort: a window that doesn't report the
    /// attribute is left alone.
    private static func clearFullscreen(_ window: AXUIElement) {
        guard readFullscreen(window) == true else { return }
        _ = AXUIElementSetAttributeValue(
            window,
            "AXFullScreen" as CFString,
            false as CFTypeRef
        )
    }

    /// True iff `frame` approximately fills `workArea` — every edge within
    /// `kMaximizeFillTolerance` points. The maximize/un-maximize
    /// discriminator (see `toggleMaximize`).
    private static func framesFill(_ frame: CGRect, _ workArea: CGRect) -> Bool {
        abs(frame.minX - workArea.minX) <= kMaximizeFillTolerance
            && abs(frame.minY - workArea.minY) <= kMaximizeFillTolerance
            && abs(frame.maxX - workArea.maxX) <= kMaximizeFillTolerance
            && abs(frame.maxY - workArea.maxY) <= kMaximizeFillTolerance
    }
}
