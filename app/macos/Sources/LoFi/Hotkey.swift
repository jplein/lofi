// Global hotkey registration via Carbon's `RegisterEventHotKey`.
//
// Why Carbon
// ----------
// `RegisterEventHotKey` is the only macOS API that can both register
// a system-wide hotkey AND consume the keystroke (so the host app
// the user was in doesn't also see it). The two AppKit alternatives
// fall short:
//
//   - `NSEvent.addGlobalMonitorForEvents(matching: .keyDown, ...)`
//     requires Accessibility permission and **cannot consume** the
//     event — Alt+Space would still pass through to whatever app has
//     focus.
//   - `CGEventTap` is overkill (one tap per hotkey, latency on every
//     key event) and also needs Accessibility.
//
// Carbon-the-framework is broadly deprecated, but `RegisterEventHotKey`
// specifically has no AppKit replacement and is what every shipping
// macOS launcher uses (Alfred, Raycast, Rectangle, Hammerspoon,
// BetterTouchTool). It works on Apple Silicon, works on macOS 26
// Tahoe, and requires no entitlement. The Sindre Sorhus
// `KeyboardShortcuts` Swift package is a thin wrapper around the
// same API — we don't pull it in because the call site here is ~50
// lines and adding a SwiftPM dependency to a Bazel build is friction
// we don't need.
//
// Lifecycle and userData safety
// -----------------------------
// The Carbon event handler is a C function pointer, so we can't
// capture `self` in a closure. We pass `Unmanaged.passUnretained(self)
// .toOpaque()` as the handler's `userData` and unwrap it back inside
// the C trampoline. Unretained is correct: the `GlobalHotkey`
// instance is owned by `AppDelegate` for the entire process
// lifetime, and `deinit` unregisters before the pointer would
// dangle. If we ever move to per-screen or per-window hotkey
// lifetimes, this needs revisiting.
//
// Hotkey ID
// ---------
// `RegisterEventHotKey` takes an `EventHotKeyID` (a 4-byte signature
// + 32-bit id). The signature is conventionally a four-CC code — we
// use `'LFHI'` (LoFi HotkeyID) for namespacing within the host
// process. Multiple hotkeys in the same process must use distinct
// `id` values; we only register one (`id = 1`), so this is just
// for future-proofing.

import AppKit
import Carbon.HIToolbox

/// Registers a single system-wide hotkey through Carbon's
/// `RegisterEventHotKey`. The press handler is invoked on the main
/// thread; the keystroke is consumed (does not reach the host app).
///
/// Pass `keyCode` from `Carbon.HIToolbox.Events` (e.g.
/// `UInt32(kVK_Space)`) and `modifiers` as the bitwise OR of the
/// Carbon flags (`UInt32(optionKey)`, `cmdKey`, `shiftKey`,
/// `controlKey`). These are **not** the modern `NSEvent.ModifierFlags`
/// bits — Carbon's encoding is `optionKey = 1 << 11`, etc.
final class GlobalHotkey {
    private var hotKeyRef: EventHotKeyRef?
    private var handlerRef: EventHandlerRef?
    private let onPress: () -> Void

    init(keyCode: UInt32, modifiers: UInt32, onPress: @escaping () -> Void) {
        self.onPress = onPress
        installHandler()
        registerHotKey(keyCode: keyCode, modifiers: modifiers)
    }

    deinit {
        if let h = handlerRef { RemoveEventHandler(h) }
        if let r = hotKeyRef { UnregisterEventHotKey(r) }
    }

    /// Install the Carbon event handler that translates `kEventHotKeyPressed`
    /// into a call to `onPress`. The trampoline is a `@convention(c)`
    /// closure (Carbon API contract); it unwraps `self` from the
    /// `userData` pointer and invokes the Swift closure on the main
    /// thread (which is where Carbon event dispatch already runs).
    private func installHandler() {
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )
        let selfPtr = Unmanaged.passUnretained(self).toOpaque()
        let status = InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData in
                guard let userData else { return noErr }
                let hk = Unmanaged<GlobalHotkey>.fromOpaque(userData)
                    .takeUnretainedValue()
                hk.onPress()
                return noErr
            },
            1,
            &spec,
            selfPtr,
            &handlerRef
        )
        // A failed install means the hotkey silently never fires (the launcher
        // then only responds to the `:activate` reopen path) — log so it's
        // diagnosable rather than a mystery.
        if status != noErr {
            NSLog("LoFi: InstallEventHandler failed (OSStatus \(status))")
        }
    }

    private func registerHotKey(keyCode: UInt32, modifiers: UInt32) {
        // 'LFHI' = 0x4C46_4849. The id field (`1`) is the per-process
        // hotkey identifier; if we ever register multiple hotkeys
        // here, give each its own id.
        let id = EventHotKeyID(signature: 0x4C46_4849, id: 1)
        let status = RegisterEventHotKey(
            keyCode,
            modifiers,
            id,
            GetApplicationEventTarget(),
            0,
            &hotKeyRef
        )
        // Registration fails if the combo is already claimed; log rather than
        // leave the user with a dead hotkey and no signal why.
        if status != noErr {
            NSLog("LoFi: RegisterEventHotKey failed (OSStatus \(status))")
        }
    }
}
