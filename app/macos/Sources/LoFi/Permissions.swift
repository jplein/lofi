// Permission gates for the macOS TCC service LoFi needs to enumerate
// and activate other applications' windows.
//
// Why this file exists: `AXUIElementCopyAttributeValue` and
// `AXUIElementPerformAction` silently fail when Accessibility is denied,
// so the AppDelegate's enumeration step needs to gate the panel-shape
// decision on the same predicate the activation step would later use.
// We need a single source of truth for "do we have it?"
//
// LoFi used to also gate on Screen Recording (because the older
// `CGWindowListCopyWindowInfo`-based `WindowDiscovery` needed it for
// window titles). After switching to AX-only window discovery, that
// grant is no longer required and Screen Recording has been removed
// from this file and from `Info.plist`. Accessibility is the single TCC
// surface LoFi requests today.
//
// Gotcha: `kAXTrustedCheckOptionPrompt` is an `Unmanaged<CFString>` in
// Swift, so we bridge with `.takeUnretainedValue() as String` to use it
// as a dictionary key. `.takeUnretainedValue()` is the right call for
// `extern const CFStringRef` globals: the runtime owns the constant, so
// taking ownership (`.takeRetainedValue()`) would over-balance the
// reference count and risk a delayed over-release crash. The trailing
// `as CFDictionary` is the cast the `AXIsProcessTrustedWithOptions` C
// signature wants.

import AppKit
import ApplicationServices

enum Permissions {
    /// `true` when LoFi can drive other processes via AX (read window
    /// titles, raise specific windows). Passes `prompt = false` so this
    /// is a pure query — no dialog. Reflects the state captured at
    /// process start.
    static func accessibility() -> Bool {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        let options = [key: false] as CFDictionary
        return AXIsProcessTrustedWithOptions(options)
    }

    /// Trigger the Accessibility prompt — passes `prompt = true` so the
    /// system shows a sheet directing the user to System Settings.
    /// Non-blocking; the user must grant the permission and relaunch
    /// LoFi to pick it up.
    static func requestAccessibility() {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        let options = [key: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
    }
}
