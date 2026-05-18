// Permission gates for the two macOS TCC services LoFi needs to enumerate
// and activate other applications' windows.
//
// Why this file exists: window discovery via `CGWindowListCopyWindowInfo`
// returns titleless / empty dictionaries when Screen Recording is denied,
// and `AXUIElementPerformAction` silently fails when Accessibility is
// denied. Both are TCC-gated and both cache their state at process start —
// granting at runtime does not take effect until the next launch. We need
// a single source of truth for "do we have these permissions?" so the
// AppDelegate's enumeration step can be gated on the same predicate the
// activation step would later be.
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
    /// `true` when LoFi can read window titles via `CGWindowList…`.
    /// Reflects the state captured at process start; granting takes
    /// effect on the next launch.
    static func screenRecording() -> Bool {
        CGPreflightScreenCaptureAccess()
    }

    /// Trigger the system Screen Recording prompt. No-op if already
    /// granted. Non-blocking; the dialog opens System Settings. Discards
    /// the return because the relevant signal is the user's eventual
    /// decision in System Settings, not the synchronous outcome.
    static func requestScreenRecording() {
        _ = CGRequestScreenCaptureAccess()
    }

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
