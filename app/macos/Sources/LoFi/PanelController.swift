// Owns the launcher's `NSPanel` and the small set of properties that
// make it behave as a floating launcher overlay (not a normal window).
//
// Design notes (each one has a story behind it; see `app/macos/README.md`
// for the deeper rationale):
//
// - `.borderless | .nonactivatingPanel` — no titlebar/chrome, no
//   activation of LoFi as the front-most app on click. Important:
//   borderless panels return `canBecomeKey = false` by default, which
//   silently breaks keyboard events. The `LoFiPanel` subclass below
//   overrides that.
// - `.floating` level — sits above ordinary application windows.
// - `[.canJoinAllSpaces, .fullScreenAuxiliary]` — the panel follows the
//   user across spaces and overlays full-screen apps the same way the
//   system Spotlight does.
// - `hidesOnDeactivate = true` — dismisses the panel as soon as focus
//   leaves it (e.g. user clicks another window). Matches Spotlight UX.
// - Fixed 640×400; centered after sizing.

import AppKit

private let kPanelWidth: CGFloat = 640
private let kPanelHeight: CGFloat = 400

final class PanelController {
    let panel: NSPanel

    init(content: NSView) {
        let frame = NSRect(x: 0, y: 0, width: kPanelWidth, height: kPanelHeight)
        let panel = LoFiPanel(
            contentRect: frame,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isMovableByWindowBackground = false
        panel.hidesOnDeactivate = true
        panel.isOpaque = false
        panel.backgroundColor = .windowBackgroundColor

        content.frame = panel.contentView?.bounds ?? frame
        content.autoresizingMask = [.width, .height]
        panel.contentView?.addSubview(content)

        self.panel = panel
    }

    func show() {
        panel.center()
        panel.makeKeyAndOrderFront(nil)
    }
}

/// `NSPanel` returns `canBecomeKey = false` for borderless panels by
/// default. That silently breaks keyboard event delivery — typing into
/// what looks like a focused panel just sends events to whatever app
/// was previously frontmost. Overriding to `true` is mandatory for any
/// borderless launcher UI.
final class LoFiPanel: NSPanel {
    override var canBecomeKey: Bool { true }
}
